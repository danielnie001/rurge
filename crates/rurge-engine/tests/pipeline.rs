//! Profile text → Runtime → Engine → real listeners on the loopback, talking to
//! `TestServer` targets through the HTTP and SOCKS5 proxies (M3 design §10).

use rurge_config::config::{LoadOptions, from_text};
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_engine::stack::StackOptions;
use rurge_engine::{Engine, ListenerSpec, Runtime, RuntimeOptions};
use rurge_inbound::{DialError, Dialer, Running, SessionHandle, SessionOutcome};
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
    engine: Arc<Engine>,
    listeners: Vec<(ListenerSpec, Running)>,
    target: TestServer,
    dns: MockDns,
    diagnostics: Vec<&'static str>,
}

impl Harness {
    fn addr_of(&self, kind: ListenerKind) -> SocketAddr {
        self.listeners
            .iter()
            .find(|(spec, _)| spec.kind == kind)
            .map(|(_, running)| running.local_addr)
            .unwrap_or_else(|| panic!("no {kind:?} listener"))
    }
    fn http(&self) -> SocketAddr {
        self.addr_of(ListenerKind::Http)
    }
    fn socks(&self) -> SocketAddr {
        self.addr_of(ListenerKind::Socks5)
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
            idle_timeout: Duration::from_secs(600),
            selections: GroupSelections::new(),
            request_log_size: 1000,
        },
    )
    .await
    .unwrap();
    let engine = Engine::new(runtime);
    let listeners = engine.bind_listeners().await.unwrap();
    assert_eq!(listeners.len(), 2);
    Harness {
        _dir: dir,
        engine,
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

/// Polls the request log until a finished record matches `pred` (≤ `timeout`).
async fn wait_for_record(
    engine: &Engine,
    timeout: Duration,
    pred: impl Fn(&rurge_engine::RequestRecord) -> bool,
) -> Option<rurge_engine::RequestRecord> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(r) = engine
            .request_log()
            .recent(50)
            .into_iter()
            .find(|r| pred(r))
        {
            return Some(r);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
    // recorded once its upstream connection ended; bytes counted
    let rec = wait_for_record(&h.engine, Duration::from_secs(3), |r| {
        r.dst.starts_with("target.test:")
            && matches!(r.status, rurge_engine::RecordStatus::Completed)
    })
    .await
    .expect("completed record for target.test");
    assert!(rec.up > 0 && rec.down > 0, "{rec:?}");
    assert!(h.engine.traffic().totals().down > 0);
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
    // the SNI the client sent through the tunnel was recorded (poll: the record
    // may be active or just finished)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let sni_seen = loop {
        let recent = h.engine.request_log().recent(20);
        let active = h.engine.request_log().active();
        if recent.iter().chain(active.iter()).any(|r| {
            r.sni.as_deref() == Some("tls.test")
                && r.protocol == Some(rurge_config::rule::ProtocolKind::Https)
        }) {
            break true;
        }
        if tokio::time::Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(sni_seen, "SNI recorded");
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
    // `Block` is reject-tinygif: an answer the rules (plain REJECT) cannot produce,
    // so a 43-byte GIF proves the mode decided and the rule never ran.
    let h = harness_with_final_reject(OutboundMode::Proxy(rurge_config::rule::PolicyRef::parse(
        "Block",
    )))
    .await;
    let (head, body) = get_via_proxy(
        h.http(),
        &format!("http://target.test:{}/hello", h.target_port()),
    )
    .await;
    assert!(
        head.starts_with("HTTP/1.1 200") && head.to_ascii_lowercase().contains("image/gif"),
        "proxy=Block mode answers with the policy's own reject: {head}"
    );
    assert_eq!(body.len(), 43);
    assert_eq!(h.target.requests().len(), 0, "the target is never reached");
}

async fn harness_with_final_reject(mode: OutboundMode) -> Harness {
    // every rule rejects (plain REJECT: close, no body); only the outbound mode
    // can let traffic through or answer differently
    harness("", "DOMAIN-SUFFIX,test,REJECT", mode).await
}

/// Builds a Runtime for a profile whose listeners are 127.0.0.1:0; `general_extra`
/// lands in [General], `rules` before FINAL,DIRECT. Reused by later tests.
async fn build_runtime(
    dir: &std::path::Path,
    dns: &MockDns,
    general_extra: &str,
    rules: &str,
    mode: OutboundMode,
    idle_timeout: Duration,
) -> Runtime {
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {}\nipv6 = false\n{general_extra}\n\
[Proxy]\nBlock = reject-tinygif\n[Proxy Group]\n[Rule]\n{rules}\nFINAL,DIRECT\n",
        dns.addr()
    );
    let loaded = from_text(&profile, &dir.join("t.conf"), &LoadOptions::for_tests());
    assert!(!loaded.diagnostics.has_errors());
    Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack: StackOptions {
                data_dir: dir.to_path_buf(),
                no_network: true,
                geo_urls: GeoUrls::default(),
                dns_cache_size: 2000,
                system: Arc::new(StaticSystemDns::default()),
                wait: Duration::ZERO,
            },
            outbound_mode: mode,
            idle_timeout,
            selections: GroupSelections::new(),
            request_log_size: 1000,
        },
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_sessions_are_closed() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let target = TestServer::spawn().await; // an HTTP server: it never speaks first
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_runtime(
        dir.path(),
        &dns,
        "",
        "", // the template already ends with FINAL,DIRECT
        OutboundMode::Rule,
        Duration::from_millis(300),
    )
    .await;
    let engine = Engine::new(runtime);
    let listeners = engine.bind_listeners().await.unwrap();
    let socks = listeners
        .iter()
        .find(|(s, _)| s.kind == ListenerKind::Socks5)
        .unwrap()
        .1
        .local_addr;
    let mut s = TcpStream::connect(socks).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut r = [0u8; 2];
    s.read_exact(&mut r).await.unwrap();
    let mut req = vec![5, 1, 0, 3, 11];
    req.extend_from_slice(b"target.test");
    req.extend_from_slice(&target.url("/").port().unwrap().to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0, "tunnel established");
    // no traffic either way → the relay closes the tunnel after the idle timeout
    let mut buf = [0u8; 1];
    let n = tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf))
        .await
        .expect("closed well before 3 s")
        .unwrap_or(0);
    assert_eq!(n, 0, "client side closed by the idle timeout");
}

/// `relay` must count as the bytes move, so a copy that dies half-way still
/// reports what it carried (M3 design §7.3).
#[tokio::test]
async fn relay_counts_bytes_as_they_move_and_keeps_them_on_failure() {
    let h = harness("", "", OutboundMode::Rule).await;
    let engine: Arc<Engine> = h.engine.clone();

    // (a) a clean close in both directions
    let (client_far, client_near) = tokio::io::duplex(4096);
    let (upstream_far, upstream_near) = tokio::io::duplex(4096);
    let handle = SessionHandle::new(
        1,
        SessionInfo::tcp(rurge_config::HostName::parse("a.test"), 80),
    );
    let relayed = tokio::spawn({
        let engine = engine.clone();
        let handle = handle.clone();
        async move {
            engine
                .relay(Box::new(client_near), Box::new(upstream_near), handle)
                .await
        }
    });
    let peers = tokio::spawn(async move {
        let (mut client, mut upstream) = (client_far, upstream_far);
        client.write_all(b"0123456789").await.unwrap(); // 10 bytes up
        let mut buf = [0u8; 10];
        upstream.read_exact(&mut buf).await.unwrap();
        upstream.write_all(b"abcd").await.unwrap(); // 4 bytes down
        let mut back = [0u8; 4];
        client.read_exact(&mut back).await.unwrap();
        drop(upstream);
        drop(client);
    });
    tokio::time::timeout(Duration::from_secs(5), relayed)
        .await
        .expect("relay finished")
        .unwrap();
    peers.await.unwrap();
    assert_eq!(handle.bytes(), (10, 4));
    assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));

    // (b) the upstream vanishes mid-copy: the bytes already carried stay counted
    let (mut client_far, client_near) = tokio::io::duplex(4096);
    let (upstream_far, upstream_near) = tokio::io::duplex(4096);
    let handle = SessionHandle::new(
        2,
        SessionInfo::tcp(rurge_config::HostName::parse("b.test"), 80),
    );
    let relayed = tokio::spawn({
        let engine = engine.clone();
        let handle = handle.clone();
        async move {
            engine
                .relay(Box::new(client_near), Box::new(upstream_near), handle)
                .await
        }
    });
    let mut upstream = upstream_far;
    client_far.write_all(b"12345").await.unwrap();
    let mut buf = [0u8; 5];
    upstream.read_exact(&mut buf).await.unwrap();
    upstream.write_all(b"xyz").await.unwrap();
    let mut back = [0u8; 3];
    client_far.read_exact(&mut back).await.unwrap();
    // The upstream vanishes; `pump` sees the EOF and marks that side closed,
    // but the client → upstream loop stays live, so the next client write
    // hits the now-dead half and fails.
    drop(upstream);
    let _ = client_far.write_all(b"never delivered").await;
    tokio::time::timeout(Duration::from_secs(5), relayed)
        .await
        .expect("relay finished")
        .unwrap();
    assert_eq!(
        handle.bytes(),
        (5, 3),
        "the bytes already carried stay counted; the failed write does not"
    );
    assert!(
        matches!(handle.outcome(), Some(SessionOutcome::Failed(_))),
        "{:?}",
        handle.outcome()
    );
}

/// A policy naming a protocol rurge has not implemented rejects, and says so
/// on the handle so the session log can name it (M3 design §7.2 step 4).
#[tokio::test]
async fn an_unimplemented_policy_rejects_with_an_explanation() {
    let h = harness("", "DOMAIN,hk.test,HK", OutboundMode::Rule).await;
    let session = SessionInfo::tcp(rurge_config::HostName::parse("hk.test"), 443);
    match h.engine.dial(session).await {
        Err(DialError::Reject { kind, handle, .. }) => {
            assert_eq!(kind, rurge_proto::RejectKind::Reject);
            assert_eq!(
                handle.error().as_deref(),
                Some("policy protocol not implemented: ss")
            );
        }
        Err(DialError::Failed { message, .. }) => panic!("expected a reject, failed: {message}"),
        Ok(_) => panic!("expected a reject, got a stream"),
    }
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

    // CONNECT to a name that fails DNS → 502 error page (M3b)
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    let (head, body) = http_exchange(&mut s, "CONNECT nx.test:443 HTTP/1.1\r\n\r\n").await;
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
            idle_timeout: Duration::from_secs(600),
            selections: GroupSelections::new(),
            request_log_size: 1000,
        },
    )
    .await
    .unwrap();
    let engine = Engine::new(runtime);
    let listeners = engine.bind_listeners().await.unwrap();
    let (head, _) = get_via_proxy(listeners[0].1.local_addr, "http://127.0.0.1:1/").await;
    assert!(head.starts_with("HTTP/1.1 407"), "{head}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_accepting_then_cancel_sessions_drains() {
    let h = harness("", "", OutboundMode::Rule).await; // template already ends with FINAL,DIRECT
    // open a CONNECT tunnel that stays idle (no data), then shut down
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(format!("CONNECT target.test:{} HTTP/1.1\r\n\r\n", h.target_port()).as_bytes())
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf))
        .await
        .expect("CONNECT reply within 3 s")
        .unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    // stop accepting; a new connection must be refused (listener closed)
    h.engine.stop_accepting();
    h.engine.tracker().close();
    // negative control: the idle tunnel is still tracked and relaying, so the
    // tracker must not drain yet (this would pass even if `ctx.tracker.spawn`
    // were reverted to a bare `tokio::spawn` without the assertion below).
    assert!(
        tokio::time::timeout(Duration::from_millis(200), h.engine.tracker().wait())
            .await
            .is_err(),
        "tracker must not drain while the tunnel is still relaying"
    );
    // force the idle tunnel to end and wait for the tracker to drain
    h.engine.cancel_sessions();
    tokio::time::timeout(Duration::from_secs(5), h.engine.tracker().wait())
        .await
        .expect("tracker drains after cancel");
}

#[tokio::test]
async fn repeated_rejects_escalate_to_drop() {
    let h = harness("", "DOMAIN,ads.test,REJECT", OutboundMode::Rule).await;
    let host = rurge_config::HostName::parse("ads.test");
    // the first ESCALATE_COUNT-1 rejects stay REJECT
    for _ in 0..(rurge_engine::engine::ESCALATE_COUNT - 1) {
        match h.engine.dial(SessionInfo::tcp(host.clone(), 80)).await {
            Err(DialError::Reject { kind, .. }) => {
                assert_eq!(kind, rurge_proto::RejectKind::Reject)
            }
            _ => panic!("expected a reject before the threshold"),
        }
    }
    // the next one crosses the threshold → DROP
    match h.engine.dial(SessionInfo::tcp(host.clone(), 80)).await {
        Err(DialError::Reject { kind, .. }) => assert_eq!(kind, rurge_proto::RejectKind::Drop),
        _ => panic!("expected a drop after escalation"),
    }
}

#[tokio::test]
async fn reload_swaps_rules_without_changing_listeners() {
    let h = harness("", "DOMAIN,ads.test,REJECT", OutboundMode::Rule).await;
    // ads.test currently rejects (closes)
    let (head, _) = get_via_proxy(h.http(), "http://ads.test/").await;
    assert!(head.is_empty());
    // reload with a config that instead serves a tinygif for ads.test, same listen addrs
    let next = build_runtime(
        h._dir.path(),
        &h.dns,
        "",
        "DOMAIN,ads.test,Block",
        OutboundMode::Rule,
        Duration::from_secs(600),
    )
    .await;
    let changed = h.engine.swap_runtime(next);
    assert!(!changed, "listen addrs unchanged");
    let (head, body) = get_via_proxy(h.http(), "http://ads.test/ad.gif").await;
    assert!(
        head.starts_with("HTTP/1.1 200") && body.len() == 43,
        "{head}"
    );
    // reload with a different http-listen address set → changed = true
    let next = build_runtime(
        h._dir.path(),
        &h.dns,
        "http-listen = 127.0.0.1:1\nsocks5-listen = 127.0.0.1:0",
        "",
        OutboundMode::Rule,
        Duration::from_secs(600),
    )
    .await;
    assert!(h.engine.swap_runtime(next), "listen addr set changed");
}
