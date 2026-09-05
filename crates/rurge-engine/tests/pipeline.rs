//! Profile text → Runtime → Engine → real listeners on the loopback, talking to
//! `TestServer` targets through the HTTP and SOCKS5 proxies (M3 design §10).

use rurge_config::config::{LoadOptions, from_text};
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_engine::stack::StackOptions;
use rurge_engine::{Engine, Runtime, RuntimeOptions};
use rurge_inbound::Running;
use rurge_net::testing::TestServer;
use rurge_policy::GroupSelections;
use rurge_rules::{GeoUrls, OutboundMode};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Harness {
    _dir: tempfile::TempDir,
    _engine: Arc<Engine>,
    listeners: Vec<Running>,
    target: TestServer,
    dns: MockDns,
    diagnostics: Vec<&'static str>,
}

impl Harness {
    fn http(&self) -> SocketAddr {
        self.listeners[0].local_addr
    }
    fn socks(&self) -> SocketAddr {
        self.listeners[1].local_addr
    }
    fn target_port(&self) -> u16 {
        self.target.url("/").port().unwrap()
    }
}

/// `LoadOptions::for_tests` declares every policy kind implemented; the binary
/// declares none of the proxy protocols (`crates/rurge/src/capabilities.rs`), so
/// drop the one the profiles below use to get the load-time W0007 warning.
fn load_options() -> LoadOptions {
    let mut opts = LoadOptions::for_tests();
    opts.capabilities
        .policy_kinds
        .remove(&rurge_config::PolicyKind::Shadowsocks);
    opts
}

/// `general_extra` lands in [General]; `rules` are inserted before `FINAL,DIRECT`.
async fn harness(general_extra: &str, rules: &str, mode: OutboundMode) -> Harness {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("tls.test", &["127.0.0.1"], &[], 60);
    dns.set("cidr.test", &["10.9.9.9"], &[], 60);
    dns.set_empty("nx.test");
    let target = TestServer::spawn().await;
    target.set("/hello", "hi there");
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {}\nipv6 = false\n{general_extra}\n\
[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\nBlock = reject-tinygif\n\
[Proxy Group]\nPick = select, HK, DIRECT\n\
[Rule]\n{rules}\nFINAL,DIRECT\n",
        dns.addr()
    );
    let loaded = from_text(&profile, &dir.path().join("t.conf"), &load_options());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.code)
            .collect::<Vec<_>>()
    );
    let diagnostics: Vec<&'static str> = loaded.diagnostics.iter().map(|d| d.code).collect();
    let runtime = Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack: StackOptions {
                data_dir: dir.path().to_path_buf(),
                no_network: true,
                geo_urls: GeoUrls::default(),
                dns_cache_size: 2000,
                system: Arc::new(StaticSystemDns::default()),
                wait: Duration::ZERO,
            },
            outbound_mode: mode,
            selections: GroupSelections::new(),
        },
    )
    .await
    .unwrap();
    let engine = Engine::new(runtime);
    let listeners = engine.bind_listeners().await.unwrap();
    assert_eq!(listeners.len(), 2);
    Harness {
        _dir: dir,
        _engine: engine,
        listeners,
        target,
        dns,
        diagnostics,
    }
}

/// Writes `request`, reads headers + Content-Length body (or until close).
async fn http_exchange(stream: &mut TcpStream, request: &str) -> (String, Vec<u8>) {
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut chunk)).await {
            Ok(Ok(n)) => n,
            _ => 0,
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
            let len = head
                .lines()
                .find_map(|l| {
                    l.split_once(':')
                        .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while buf.len() < pos + 4 + len {
                let n = stream.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            return (head, buf[pos + 4..].to_vec());
        }
    }
    (String::from_utf8_lossy(&buf).to_string(), Vec::new())
}

async fn get_via_proxy(proxy: SocketAddr, url: &str) -> (String, Vec<u8>) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    let host = url
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap()
        .to_string();
    http_exchange(
        &mut s,
        &format!("GET {url} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
    )
    .await
}

#[tokio::test]
async fn plain_http_is_forwarded_through_direct() {
    let h = harness("", "", OutboundMode::Rule).await;
    let (head, body) = get_via_proxy(
        h.http(),
        &format!("http://target.test:{}/hello", h.target_port()),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");
    assert_eq!(h.target.requests()[0].path, "/hello");
    assert_eq!(
        h.dns
            .query_count("target.test", rurge_dns::message::Qtype::A),
        1,
        "resolved once through the profile resolver"
    );
}

#[tokio::test]
async fn connect_tunnel_carries_tls_to_the_target() {
    let h = harness("", "", OutboundMode::Rule).await;
    let tls_target = TestServer::spawn_tls().await;
    tls_target.set("/secure", "very secret");
    let port = tls_target.url("/").port().unwrap();
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    let (head, _) = http_exchange(
        &mut s,
        &format!("CONNECT tls.test:{port} HTTP/1.1\r\nHost: tls.test:{port}\r\n\r\n"),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let config = rurge_net::http::tls_client_config(true).unwrap();
    let name = rustls::pki_types::ServerName::try_from("tls.test".to_string()).unwrap();
    let mut tls = tokio_rustls::TlsConnector::from(config)
        .connect(name, s)
        .await
        .unwrap();
    tls.write_all(b"GET /secure HTTP/1.1\r\nHost: tls.test\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), tls.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out);
    assert!(
        text.starts_with("HTTP/1.1 200") && text.ends_with("very secret"),
        "{text}"
    );
}

#[tokio::test]
async fn socks5_connect_reaches_the_target() {
    let h = harness("", "", OutboundMode::Rule).await;
    let port = h.target_port();
    let mut s = TcpStream::connect(h.socks()).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut r = [0u8; 2];
    s.read_exact(&mut r).await.unwrap();
    assert_eq!(r, [5, 0]);
    let mut req = vec![5, 1, 0, 3, 11];
    req.extend_from_slice(b"target.test");
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0);
    let (head, body) = http_exchange(
        &mut s,
        "GET /hello HTTP/1.1\r\nHost: target.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");
}

#[tokio::test]
async fn reject_rules_close_serve_gifs_or_render_pages() {
    let rules = "DOMAIN,ads.test,REJECT\nDOMAIN,gif.test,Block\nDOMAIN,hk.test,HK";
    // defaults: REJECT closes, TINYGIF answers, unsupported policy behaves as REJECT
    let h = harness("", rules, OutboundMode::Rule).await;
    assert!(
        h.diagnostics.contains(&"W0007"),
        "unsupported ss policy warned at load: {:?}",
        h.diagnostics
    );
    let (head, _) = get_via_proxy(h.http(), "http://ads.test/").await;
    assert!(head.is_empty(), "REJECT must close: {head}");
    let (head, body) = get_via_proxy(h.http(), "http://gif.test/ad.gif").await;
    assert!(
        head.starts_with("HTTP/1.1 200") && head.to_ascii_lowercase().contains("image/gif"),
        "{head}"
    );
    assert_eq!(body.len(), 43);
    let (head, _) = get_via_proxy(h.http(), "http://hk.test/").await;
    assert!(
        head.is_empty(),
        "unsupported policy closes like REJECT: {head}"
    );
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    let (head, _) = http_exchange(&mut s, "CONNECT ads.test:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.is_empty(), "CONNECT reject closes: {head}");
    let mut s = TcpStream::connect(h.socks()).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut r = [0u8; 2];
    s.read_exact(&mut r).await.unwrap();
    let mut req = vec![5, 1, 0, 3, 8];
    req.extend_from_slice(b"ads.test");
    req.extend_from_slice(&443u16.to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0x02, "SOCKS5 reports 'not allowed by ruleset'");
    // error pages on
    let h = harness(
        "show-error-page-for-reject = true",
        rules,
        OutboundMode::Rule,
    )
    .await;
    let (head, body) = get_via_proxy(h.http(), "http://ads.test/").await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    let html = String::from_utf8_lossy(&body);
    assert!(
        html.contains("DOMAIN,ads.test,REJECT") && html.contains("REJECT"),
        "{html}"
    );
    let (head, body) = get_via_proxy(h.http(), "http://hk.test/").await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    assert!(String::from_utf8_lossy(&body).contains("!unsupported:ss"));
}

#[tokio::test]
async fn keep_alive_requests_are_dialed_one_by_one() {
    let h = harness("", "DOMAIN,gif.test,Block", OutboundMode::Rule).await;
    let port = h.target_port();
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    let (head, body) = http_exchange(
        &mut s,
        &format!(
            "GET http://target.test:{port}/hello HTTP/1.1\r\nHost: target.test:{port}\r\n\r\n"
        ),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");
    let (head, body) = http_exchange(
        &mut s,
        "GET http://gif.test/x.gif HTTP/1.1\r\nHost: gif.test\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body.len(), 43);
}

#[tokio::test]
async fn outbound_modes_bypass_the_rules() {
    let h = harness_with_final_reject(OutboundMode::Direct).await;
    let (head, body) = get_via_proxy(
        h.http(),
        &format!("http://target.test:{}/hello", h.target_port()),
    )
    .await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "direct mode ignores rules: {head}"
    );
    assert_eq!(body, b"hi there");
    let h = harness_with_final_reject(OutboundMode::Proxy(rurge_config::rule::PolicyRef::parse(
        "REJECT",
    )))
    .await;
    let (head, _) = get_via_proxy(
        h.http(),
        &format!("http://target.test:{}/hello", h.target_port()),
    )
    .await;
    assert!(
        head.is_empty(),
        "proxy=REJECT mode rejects everything: {head}"
    );
}

async fn harness_with_final_reject(mode: OutboundMode) -> Harness {
    // every rule rejects; only the outbound mode can let traffic through
    harness("", "DOMAIN-SUFFIX,test,REJECT", mode).await
}

#[tokio::test]
async fn dns_failure_and_ip_rules() {
    let h = harness("", "IP-CIDR,10.0.0.0/8,Block", OutboundMode::Rule).await;
    // cidr.test resolves to 10.9.9.9 → the IP rule matches → tinygif
    let (head, body) = get_via_proxy(h.http(), "http://cidr.test/").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body.len(), 43);
    // nx.test has no records → dns failed → 502 error page (show-error-page defaults to true)
    let (head, body) = get_via_proxy(h.http(), "http://nx.test/").await;
    assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    assert!(String::from_utf8_lossy(&body).contains("DNS lookup failed"));
}

#[tokio::test]
async fn http_listener_password_from_the_profile() {
    let dns = MockDns::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = s3cret@127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {}\n[Proxy]\n[Rule]\nFINAL,DIRECT\n",
        dns.addr()
    );
    let loaded = from_text(
        &profile,
        &dir.path().join("t.conf"),
        &LoadOptions::for_tests(),
    );
    let runtime = Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack: StackOptions {
                data_dir: dir.path().to_path_buf(),
                no_network: true,
                geo_urls: GeoUrls::default(),
                dns_cache_size: 100,
                system: Arc::new(StaticSystemDns::default()),
                wait: Duration::ZERO,
            },
            outbound_mode: OutboundMode::Rule,
            selections: GroupSelections::new(),
        },
    )
    .await
    .unwrap();
    let engine = Engine::new(runtime);
    let listeners = engine.bind_listeners().await.unwrap();
    let (head, _) = get_via_proxy(listeners[0].local_addr, "http://127.0.0.1:1/").await;
    assert!(head.starts_with("HTTP/1.1 407"), "{head}");
}
