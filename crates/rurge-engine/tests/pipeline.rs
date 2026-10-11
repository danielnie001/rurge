//! Profile text → Runtime → Engine → real listeners on the loopback, talking to
//! `TestServer` targets through the HTTP and SOCKS5 proxies (M3 design §10).

// not `use common::*`: this file has a harness of its own
mod common;

use rurge_config::HostName;
use rurge_config::config::{LoadOptions, from_text};
use rurge_config::rule::ProtocolKind;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_dns::Source;
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::{Doh3Script, DoqScript, FakeDoh3, FakeDoq, MockDns};
use rurge_engine::stack::StackOptions;
use rurge_engine::{
    Engine, EngineShared, ListenerSpec, RecordStatus, RequestRecord, Runtime, RuntimeOptions,
};
use rurge_inbound::{DialError, Dialer, Running, SessionHandle, SessionOutcome};
use rurge_net::connector::Target;
use rurge_net::socket::{Family, NoopSocketHook, SocketHook};
use rurge_net::testing::TestServer;
use rurge_policy::GroupSelections;
use rurge_proto::testing::{FakeHttpProxy, FakeSocks5, HttpProxyScript, Socks5Script};
use rurge_proto_quic::testing::{FakeH3Proxy, H3ProxyScript};
use rurge_rules::{GeoUrls, OutboundMode};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
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
/// does not declare Tailscale (`crates/rurge/src/capabilities.rs`), so
/// dropping it here yields the load-time W0007 warning the profiles below rely on.
fn load_options() -> LoadOptions {
    let mut opts = LoadOptions::for_tests();
    opts.capabilities
        .policy_kinds
        .remove(&rurge_config::PolicyKind::Tailscale);
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
[Proxy]\nHK = tailscale, section-name=ts\nBlock = reject-tinygif\n\
Locked = trojan, 1.2.3.4, 443, password=x\n\
[Proxy Group]\nPick = select, HK, DIRECT\n\
[Rule]\n{rules}\nFINAL,DIRECT\n",
        dns.addr()
    );
    std::fs::write(dir.path().join("t.conf"), &profile).unwrap();
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
                dns_connector: None,
                socket_hook: Arc::new(rurge_net::socket::NoopSocketHook),
            },
            outbound_mode: mode,
            idle_timeout: Duration::from_secs(600),
            shared: EngineShared::default(),
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

/// Both ways a session reaches its target carry the two timings (M3c design
/// 4.1) — a plain request the HTTP listener forwards, and a CONNECT tunnel —
/// and a rejected session has neither.
#[tokio::test]
async fn the_request_log_times_the_outbound_and_the_first_byte() {
    let h = harness("", "DOMAIN,ads.test,REJECT", OutboundMode::Rule).await;
    let port = h.target_port();
    let (head, _) = get_via_proxy(h.http(), &format!("http://target.test:{port}/hello")).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let plain = wait_for_record(&h.engine, Duration::from_secs(3), |r| {
        r.dst.starts_with("target.test:")
            && matches!(r.status, rurge_engine::RecordStatus::Completed)
    })
    .await
    .expect("the plain request's record");
    assert!(
        plain.connect_ms.is_some() && plain.first_byte_ms.is_some(),
        "{plain:?}"
    );

    let mut s = TcpStream::connect(h.http()).await.unwrap();
    let (head, _) = http_exchange(
        &mut s,
        &format!("CONNECT target.test:{port} HTTP/1.1\r\nHost: target.test:{port}\r\n\r\n"),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    s.write_all(b"GET /hello HTTP/1.1\r\nHost: target.test\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), s.read_to_end(&mut out)).await;
    drop(s);
    let tunnel = wait_for_record(&h.engine, Duration::from_secs(3), |r| {
        r.id != plain.id
            && r.dst.starts_with("target.test:")
            && matches!(r.status, rurge_engine::RecordStatus::Completed)
    })
    .await
    .expect("the tunnel's record");
    assert!(
        tunnel.connect_ms.is_some() && tunnel.first_byte_ms.is_some(),
        "{tunnel:?}"
    );

    let (head, _) = get_via_proxy(h.http(), "http://ads.test/").await;
    assert!(head.is_empty(), "{head}");
    let rejected = wait_for_record(&h.engine, Duration::from_secs(3), |r| {
        r.dst.starts_with("ads.test:")
    })
    .await
    .expect("the rejected session's record");
    assert_eq!(
        (rejected.connect_ms, rejected.first_byte_ms),
        (None, None),
        "{rejected:?}"
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
        "unsupported tailscale policy warned at load: {:?}",
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
    assert!(String::from_utf8_lossy(&body).contains("!unsupported:tailscale"));
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

/// The rule that lets the target through is written first, the flagged
/// REJECT after it: the flagged one decides. The outbound mode still takes
/// all of the rules, flagged or not, out of the way.
#[tokio::test]
async fn a_pre_matching_reject_written_last_still_rejects_first() {
    use rurge_engine::control::Mode;
    let h = harness(
        "",
        "DOMAIN,target.test,DIRECT\nDOMAIN,target.test,REJECT,pre-matching",
        OutboundMode::Rule,
    )
    .await;
    let url = format!("http://target.test:{}/hello", h.target_port());
    let (head, _) = get_via_proxy(h.http(), &url).await;
    assert!(head.is_empty(), "rejected: {head}");
    let rec = wait_for_record(&h.engine, Duration::from_secs(3), |r| {
        r.dst.starts_with("target.test:") && matches!(r.status, RecordStatus::Rejected(_))
    })
    .await
    .expect("the rejected session's record");
    assert_eq!(
        rec.rule.as_deref(),
        Some("DOMAIN,target.test,REJECT,pre-matching")
    );
    assert_eq!(h.target.requests().len(), 0, "the target is never reached");
    let hits: Vec<u64> = h.engine.rules_view().iter().map(|r| r.hits).collect();
    assert_eq!(hits[..2], [0, 1]);
    h.engine.set_mode(Mode::Direct).await;
    let (head, body) = get_via_proxy(h.http(), &url).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");
}

/// Keeps the message and the fields of every event down to DEBUG.
struct Debugging(Arc<Mutex<Vec<String>>>);

#[derive(Default)]
struct EventLine {
    message: String,
    fields: String,
}

impl tracing::field::Visit for EventLine {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields += &format!(" {}={value:?}", field.name());
        }
    }
}

impl tracing::Subscriber for Debugging {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::DEBUG
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = EventLine::default();
        event.record(&mut line);
        self.0
            .lock()
            .unwrap()
            .push(format!("{}{}", line.message, line.fields));
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// A session a set decided says which line of the set matched: in one DEBUG
/// line with the session's id, and on its record, where the list of notes
/// keeps clear of `error`.
#[tokio::test]
async fn a_rule_set_hit_is_logged_and_noted_on_the_request() {
    let lines = Arc::new(Mutex::new(Vec::new()));
    // held across the awaits: the engine's tasks run on this thread
    let _capturing = tracing::subscriber::set_default(Debugging(lines.clone()));
    // While one dispatcher alone is registered, tracing-core asks only the
    // thread that first reaches a call site whether it is wanted: a site
    // another test reaches first would stay shut on this thread too. With a
    // second one it asks every dispatcher.
    let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let sets = tempfile::tempdir().unwrap();
    let list = sets.path().join("listed.list");
    std::fs::write(&list, "DOMAIN,target.test\n").unwrap();
    let h = harness(
        "",
        &format!("RULE-SET,{},DIRECT", list.display()),
        OutboundMode::Rule,
    )
    .await;
    let port = h.target_port();
    let (head, _) = get_via_proxy(h.http(), &format!("http://target.test:{port}/hello")).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let listed = wait_for_record(&h.engine, Duration::from_secs(3), |r| {
        r.dst.starts_with("target.test:") && matches!(r.status, RecordStatus::Completed)
    })
    .await
    .expect("the listed session's record");
    let sentence = format!(
        "Sub-rule matched: DOMAIN,target.test (in {})",
        list.display()
    );
    assert_eq!(listed.notes, [sentence.as_str()]);
    assert_eq!(listed.error, None);
    let said: Vec<String> = lines
        .lock()
        .unwrap()
        .iter()
        .filter(|l| l.contains("Sub-rule matched"))
        .cloned()
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].starts_with(&sentence), "{}", said[0]);
    assert!(
        said[0].ends_with(&format!(" session={}", listed.id)),
        "{}",
        said[0]
    );
    // a session that no set had a say in has nothing to note
    let (head, _) = get_via_proxy(h.http(), &format!("http://tls.test:{port}/hello")).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let other = wait_for_record(&h.engine, Duration::from_secs(3), |r| {
        r.dst.starts_with("tls.test:") && matches!(r.status, RecordStatus::Completed)
    })
    .await
    .expect("the other session's record");
    assert!(other.notes.is_empty(), "{other:?}");
}

async fn harness_with_final_reject(mode: OutboundMode) -> Harness {
    // every rule rejects (plain REJECT: close, no body); only the outbound mode
    // can let traffic through or answer differently
    harness("", "DOMAIN-SUFFIX,test,REJECT", mode).await
}

/// Builds a Runtime for a profile whose listeners are 127.0.0.1:0; `general_extra`
/// lands in [General], `rules` before FINAL,DIRECT. Reused by later tests.
/// `shared` must be `EngineShared::default()` for a fresh engine, or the
/// running engine's own `shared()` for a `swap_runtime` reload.
async fn build_runtime(
    dir: &std::path::Path,
    dns: &MockDns,
    general_extra: &str,
    rules: &str,
    mode: OutboundMode,
    idle_timeout: Duration,
    shared: EngineShared,
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
                dns_connector: None,
                socket_hook: Arc::new(rurge_net::socket::NoopSocketHook),
            },
            outbound_mode: mode,
            idle_timeout,
            shared,
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
        EngineShared::default(),
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
    // The upstream vanishes; the upstream → client `copy_half` sees the EOF
    // and returns, but the client → upstream `copy_half` is a separate task
    // that keeps running, so the next client write hits the now-dead
    // upstream half and fails.
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

/// The traffic sampler takes a sample a second. Time that passes without it
/// running — a machine that slept, on a platform whose clock counts the
/// sleep — is not made up for: one sample follows, not one for every second
/// missed (each takes the request log's lock and sums the live sessions). The
/// rate is the change since the previous sample, so a sampler that made up
/// the missed seconds would end by reporting none.
#[tokio::test(start_paused = true)]
async fn the_traffic_sampler_does_not_make_up_for_missed_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let profile = "[General]\nipv6 = false\n[Proxy]\n[Proxy Group]\n[Rule]\nFINAL,DIRECT\n";
    let engine = engine_from_profile(dir.path(), profile).await;
    engine.start_sampler();
    // the clock moves when nothing else can run: the sampler has taken its
    // first sample, with nothing moved yet
    tokio::time::sleep(Duration::from_millis(1)).await;
    let live = SessionHandle::new(
        1,
        SessionInfo::tcp(rurge_config::HostName::parse("live.test"), 443),
    );
    engine.request_log().mark_active(&live);
    live.add_up(1000);
    // thirty seconds in which the sampler did not run
    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert_eq!(engine.traffic().rate(), (1000, 0));
}

/// A policy naming a protocol rurge does not implement rejects, and says so
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
                Some("policy protocol not implemented: tailscale")
            );
        }
        Err(DialError::Failed { message, .. }) => panic!("expected a reject, failed: {message}"),
        Ok(_) => panic!("expected a reject, got a stream"),
    }
}

/// A DIRECT-substituted empty group (M3-D3) that then fails to dial keeps
/// both why DIRECT stood in and why the dial itself failed: the request
/// record must not lose the failure behind the substitution note.
#[tokio::test]
async fn a_failed_dial_through_a_direct_substituted_empty_group_keeps_both_reasons() {
    let port = common::closed_tcp_port();
    let dir = tempfile::tempdir().unwrap();
    let profile = "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nipv6 = false\n\
[Proxy]\n[Proxy Group]\nG = select, policy-path=https://sub.test/g\n[Rule]\nFINAL,G\n";
    let engine = engine_from_profile(dir.path(), profile).await;
    let session = SessionInfo::tcp(rurge_config::HostName::parse("127.0.0.1"), port);
    match engine.dial(session).await {
        Err(DialError::Failed { .. }) => {}
        Err(DialError::Reject { kind, .. }) => {
            panic!("expected a connect failure through DIRECT, got a reject: {kind:?}")
        }
        Ok(_) => panic!("expected a connect failure through DIRECT, got a stream"),
    }
    let rec = wait_for_record(&engine, Duration::from_secs(3), |r| {
        r.error
            .as_deref()
            .is_some_and(|e| e.starts_with("policy group has no members; DIRECT substituted; "))
    })
    .await
    .expect("a failed record naming both the substitution and the failure");
    assert!(
        matches!(rec.status, rurge_engine::RecordStatus::Failed),
        "{rec:?}"
    );
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
                dns_connector: None,
                socket_hook: Arc::new(rurge_net::socket::NoopSocketHook),
            },
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            shared: EngineShared::default(),
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
        h.engine.shared(),
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
        h.engine.shared(),
    )
    .await;
    assert!(h.engine.swap_runtime(next), "listen addr set changed");
}

/// A loopback port nothing listens on right now: bind `:0`, read the port,
/// drop. Inherently a probe — another process can take it before the caller
/// binds — so every caller retries once.
async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

fn http_addr(listeners: &[(ListenerSpec, Running)]) -> SocketAddr {
    listeners
        .iter()
        .find(|(spec, _)| spec.kind == ListenerKind::Http)
        .map(|(_, running)| running.local_addr)
        .expect("an HTTP listener")
}

/// One `CONNECT target.test:<target_port>` through `proxy`; returns the status
/// code (or `""` if the proxy closed without answering). `credential` is the
/// already-encoded `Basic` payload.
async fn connect_status(proxy: SocketAddr, target_port: u16, credential: Option<&str>) -> String {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    let auth = match credential {
        Some(c) => format!("Proxy-Authorization: Basic {c}\r\n"),
        None => String::new(),
    };
    let (head, _) = http_exchange(
        &mut s,
        &format!(
            "CONNECT target.test:{target_port} HTTP/1.1\r\nHost: target.test:{target_port}\r\n{auth}\r\n"
        ),
    )
    .await;
    head.lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string()
}

/// `Basic` payloads for `rurge:old` / `rurge:new`; the HTTP listener's
/// `password@` form compares the password half only, so the user is free.
const CRED_OLD: &str = "cnVyZ2U6b2xk";
const CRED_NEW: &str = "cnVyZ2U6bmV3";

/// A reload that only rotates the proxy password has to take effect. Comparing
/// listen *addresses* would report "profile reloaded" while the old credential
/// kept working and the new one did not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_with_changed_password_rebinds_and_applies_it() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let target = TestServer::spawn().await;
    let target_port = target.url("/").port().unwrap();
    let dir = tempfile::tempdir().unwrap();

    // Pin both generations to the same port so the *only* difference is the
    // password. Probing races other processes, hence the retry.
    let mut bound = None;
    for _ in 0..2 {
        let port = free_port().await;
        let runtime = build_runtime(
            dir.path(),
            &dns,
            &format!("http-listen = old@127.0.0.1:{port}"),
            "",
            OutboundMode::Rule,
            Duration::from_secs(600),
            EngineShared::default(),
        )
        .await;
        let engine = Engine::new(runtime);
        if let Ok(listeners) = engine.bind_listeners().await {
            bound = Some((engine, listeners, port));
            break;
        }
    }
    let (engine, listeners, port) = bound.expect("could not bind a probed free port twice");
    let http = http_addr(&listeners);
    assert_eq!(http.port(), port);
    assert_eq!(
        connect_status(http, target_port, Some(CRED_OLD)).await,
        "200"
    );

    let next = build_runtime(
        dir.path(),
        &dns,
        &format!("http-listen = new@127.0.0.1:{port}"),
        "",
        OutboundMode::Rule,
        Duration::from_secs(600),
        engine.shared(),
    )
    .await;
    assert!(
        engine.swap_runtime(next),
        "a rotated proxy password is a listener change"
    );
    let listeners = engine
        .rebind_listeners(listeners)
        .await
        .expect("rebind onto the same address");
    assert_eq!(http_addr(&listeners), http);
    assert_eq!(
        connect_status(http, target_port, Some(CRED_OLD)).await,
        "407"
    );
    assert_eq!(
        connect_status(http, target_port, Some(CRED_NEW)).await,
        "200"
    );
}

/// The stop → `wait_closed` → detached `join` → bind sequence `run.rs::reload`
/// drives: the old address must stop accepting and the new one must serve.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebind_moves_the_listener_to_the_new_address() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let target = TestServer::spawn().await;
    let target_port = target.url("/").port().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_runtime(
        dir.path(),
        &dns,
        "",
        "",
        OutboundMode::Rule,
        Duration::from_secs(600),
        EngineShared::default(),
    )
    .await;
    let engine = Engine::new(runtime);
    let mut listeners = engine.bind_listeners().await.unwrap();
    let old_http = http_addr(&listeners);
    assert_eq!(connect_status(old_http, target_port, None).await, "200");

    // Move the HTTP listener to a probed free port. A failed rebind leaves no
    // listeners behind, which is exactly what the retry starts from.
    let mut moved = None;
    for _ in 0..2 {
        let port = free_port().await;
        let next = build_runtime(
            dir.path(),
            &dns,
            &format!("http-listen = 127.0.0.1:{port}"),
            "",
            OutboundMode::Rule,
            Duration::from_secs(600),
            engine.shared(),
        )
        .await;
        assert!(engine.swap_runtime(next), "the listen address changed");
        match engine.rebind_listeners(listeners).await {
            Ok(l) => {
                moved = Some((l, port));
                break;
            }
            Err(_) => listeners = Vec::new(),
        }
    }
    let (listeners, port) = moved.expect("could not rebind onto a probed free port twice");

    let refused = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(old_http)).await;
    assert!(
        matches!(refused, Ok(Err(_)) | Err(_)),
        "the old address still accepts connections"
    );
    let http = http_addr(&listeners);
    assert_eq!(http.port(), port);
    assert_eq!(connect_status(http, target_port, None).await, "200");
}

/// A rebind onto a taken port fails after the old listeners are already gone,
/// so the engine is left serving nothing — and a later rebind from that empty
/// state (what `run.rs` forces when `listeners.is_empty()`) recovers it. The
/// port stays taken to the end of the test: giving it back and binding it
/// again would leave the number to whatever else on the machine asks the
/// system for a port in between. The recovery is a generation that asks for
/// a port of the system's choosing, as a profile edited off the taken port
/// does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebind_failure_leaves_no_listeners_and_a_later_rebind_recovers() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let target = TestServer::spawn().await;
    let target_port = target.url("/").port().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_runtime(
        dir.path(),
        &dns,
        "",
        "",
        OutboundMode::Rule,
        Duration::from_secs(600),
        EngineShared::default(),
    )
    .await;
    let engine = Engine::new(runtime);
    let listeners = engine.bind_listeners().await.unwrap();

    // The next generation wants a port this test holds itself.
    let squatter = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let taken = squatter.local_addr().unwrap().port();
    let next = build_runtime(
        dir.path(),
        &dns,
        &format!("http-listen = 127.0.0.1:{taken}"),
        "",
        OutboundMode::Rule,
        Duration::from_secs(600),
        engine.shared(),
    )
    .await;
    assert!(engine.swap_runtime(next), "the listen address changed");
    // (`Running` has no `Debug`, so `expect_err` is not available here.)
    let err = match engine.rebind_listeners(listeners).await {
        Ok(_) => panic!("rebinding onto a port this test holds must fail"),
        Err(e) => e,
    };
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse, "{err}");

    // Degraded: nothing is listening. The squatter stays; the next
    // generation no longer wants its port, and a rebind from empty brings
    // the daemon back.
    let next = build_runtime(
        dir.path(),
        &dns,
        "",
        "",
        OutboundMode::Rule,
        Duration::from_secs(600),
        engine.shared(),
    )
    .await;
    assert!(engine.swap_runtime(next), "the listen address changed");
    let listeners = engine
        .rebind_listeners(Vec::new())
        .await
        .expect("a later rebind recovers the daemon");
    let http = http_addr(&listeners);
    assert_ne!(http.port(), taken);
    assert_eq!(connect_status(http, target_port, None).await, "200");
    drop(squatter);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dns_upstream_follows_the_pipeline_as_an_internal_session() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\n[Proxy Group]\n[Rule]\nPROTOCOL,DNS,DIRECT\nFINAL,DIRECT\n",
        dns.addr().port()
    );
    let loaded = from_text(
        &profile,
        &dir.path().join("t.conf"),
        &LoadOptions::for_tests(),
    );
    assert!(!loaded.diagnostics.has_errors());
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
                dns_connector: None,
                socket_hook: Arc::new(rurge_net::socket::NoopSocketHook),
            },
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            request_log_size: 1000,
            shared: EngineShared::default(),
        },
    )
    .await
    .unwrap();
    let engine = Engine::new(runtime);
    let _listeners = engine.bind_listeners().await.unwrap();
    // resolving a name forces a TCP DNS query, which must go through the pipeline
    let res = engine
        .runtime()
        .stack
        .resolver
        .lookup("target.test", rurge_dns::resolver::LookupOpts::default())
        .await;
    assert!(res.is_ok(), "resolution through the pipeline: {res:?}");
    // the DNS server connection was recorded as an Internal session with DNS protocol
    let seen = engine.request_log().recent(50);
    let active = engine.request_log().active();
    assert!(
        seen.iter()
            .chain(active.iter())
            .any(|r| r.listener == ListenerKind::Internal
                && r.protocol == Some(rurge_config::rule::ProtocolKind::Dns)),
        "internal DNS session recorded: recent={seen:?} active={active:?}"
    );
    assert!(
        !dns.queries().is_empty(),
        "the mock DNS server was actually queried"
    );
}

/// An engine built from a complete profile text. The follow-mode tests below
/// need their own `[Proxy]` entries and `encrypted-dns-server`, which
/// `build_runtime`'s fixed profile shape cannot express.
async fn engine_from_profile(dir: &std::path::Path, profile: &str) -> Arc<Engine> {
    engine_with_hook(dir, profile, Arc::new(NoopSocketHook)).await
}

/// `engine_from_profile` with `hook` applying the socket settings of every
/// direct connector.
async fn engine_with_hook(
    dir: &std::path::Path,
    profile: &str,
    hook: Arc<dyn SocketHook>,
) -> Arc<Engine> {
    let loaded = from_text(profile, &dir.join("t.conf"), &LoadOptions::for_tests());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    let runtime = Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack: StackOptions {
                data_dir: dir.to_path_buf(),
                no_network: true,
                geo_urls: GeoUrls::default(),
                dns_cache_size: 2000,
                system: Arc::new(StaticSystemDns::default()),
                wait: Duration::ZERO,
                dns_connector: None,
                socket_hook: hook,
            },
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            request_log_size: 1000,
            shared: EngineShared::default(),
        },
    )
    .await
    .unwrap();
    Engine::new(runtime)
}

/// Every `Internal` session the engine has recorded so far.
fn internal_sessions(engine: &Engine) -> Vec<RequestRecord> {
    let mut all = engine.request_log().recent(50);
    all.extend(engine.request_log().active());
    all.retain(|r| r.listener == ListenerKind::Internal);
    all
}

/// The anti-loop fallback of `encrypted-dns-follow-outbound-mode` (M3 design
/// §7.4): reaching a proxy configured by HOST NAME would need the very lookup
/// the DNS session is carrying, so the engine warns and connects directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dns_session_bypasses_a_proxy_configured_by_host_name() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("proxy.test", &["127.0.0.1"], &[], 60);
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\nUp = http, proxy.test, 8080\n[Proxy Group]\n[Rule]\nPROTOCOL,DNS,Up\nFINAL,DIRECT\n",
        dns.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup must not wait for the proxy's own name to be resolved");
    assert!(res.is_ok(), "resolution through the pipeline: {res:?}");
    let internal = internal_sessions(&engine);
    assert!(
        internal.iter().any(|r| {
            r.error.as_deref()
                == Some(
                    "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
                )
                && r.policy.first().map(String::as_str) == Some("Up")
        }),
        "a bypassed internal DNS session: {internal:?}"
    );
    assert!(
        !dns.queries().is_empty(),
        "the mock DNS server was actually queried"
    );
}

/// The same for a `wireguard` policy whose peer's endpoint is a host name:
/// starting the tunnel would need the very lookup the session carries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dns_session_bypasses_a_wireguard_policy_whose_endpoint_is_a_host_name() {
    use rurge_proto_wireguard::testing::{hex, keypair};
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("wg.test", &["127.0.0.1"], &[], 60);
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\nWG = wireguard, section-name=w\n[Proxy Group]\n[Rule]\nPROTOCOL,DNS,WG\nFINAL,DIRECT\n\
[WireGuard w]\nprivate-key = {}\nself-ip = 10.9.0.2\n\
peer = (public-key = {}, allowed-ips = 0.0.0.0/0, endpoint = wg.test:51820)\n",
        dns.addr().port(),
        hex(&keypair().0),
        hex(&keypair().1)
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup must not wait for the endpoint's own name to be resolved");
    assert!(res.is_ok(), "resolution through the pipeline: {res:?}");
    let internal = internal_sessions(&engine);
    assert!(
        internal.iter().any(|r| {
            r.error.as_deref()
                == Some(
                    "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
                )
                && r.policy.first().map(String::as_str) == Some("WG")
        }),
        "a bypassed internal DNS session: {internal:?}"
    );
}

/// A DNS session does not wait for an `evaluate-before-use` group's first
/// round: the round's own probes may need this very session's resolver to
/// reach a host-named member's server, so waiting here would wait on
/// itself. The session resolves with whatever the group already has — its
/// first member, `Up`, which the host-name bypass above then carries
/// directly — while the round runs on its own in the background (`choose`
/// still asks for it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dns_session_does_not_wait_for_an_evaluate_before_use_group() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("proxy.test", &["127.0.0.1"], &[], 60);
    let member = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
proxy-test-url = http://127.0.0.1:9/\ninternet-test-url = http://127.0.0.1:9/\n\
[Proxy]\nUp = http, proxy.test, {}\n[Proxy Group]\nE = fallback, Up, evaluate-before-use=true\n\
[Rule]\nPROTOCOL,DNS,E\nFINAL,DIRECT\n",
        dns.addr().port(),
        member.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
        Duration::from_secs(3),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup must not wait for the group's first round");
    assert!(res.is_ok(), "resolution through the pipeline: {res:?}");
    let internal = internal_sessions(&engine);
    assert!(
        internal.iter().any(|r| {
            r.error.as_deref()
                == Some(
                    "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
                )
                && r.policy.first().map(String::as_str) == Some("E")
        }),
        "a bypassed internal DNS session through E: {internal:?}"
    );
}

/// The guard reads the policy from the registry, not the main profile: a
/// proxy that reached the profile through a subscription (`socket_opener`
/// via `PolicyRegistry::spec`) is bypassed exactly like one written in
/// `[Proxy]`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dns_session_bypasses_a_proxy_imported_from_a_policy_path() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("proxy.test", &["127.0.0.1"], &[], 60);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("up.txt"), "Up = http, proxy.test, 8080\n").unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\n[Proxy Group]\nSub = select, policy-path=up.txt\n[Rule]\nPROTOCOL,DNS,Sub\nFINAL,DIRECT\n",
        dns.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup must not wait for the proxy's own name to be resolved");
    assert!(res.is_ok(), "resolution through the pipeline: {res:?}");
    let internal = internal_sessions(&engine);
    assert!(
        internal.iter().any(|r| {
            r.error.as_deref()
                == Some(
                    "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
                )
                && r.policy.first().map(String::as_str) == Some("Sub")
        }),
        "a bypassed internal DNS session: {internal:?}"
    );
}

/// The anti-loop guard is about where the socket is opened, not about the
/// protocol: a host-named trojan server is bypassed exactly like an http one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dns_session_bypasses_a_trojan_proxy_configured_by_host_name() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("proxy.test", &["127.0.0.1"], &[], 60);
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\nUp = trojan, proxy.test, 443, password=pw\n[Proxy Group]\n[Rule]\nPROTOCOL,DNS,Up\nFINAL,DIRECT\n",
        dns.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup must not wait for the proxy's own name to be resolved");
    assert!(res.is_ok(), "resolution through the pipeline: {res:?}");
    let internal = internal_sessions(&engine);
    assert!(
        internal.iter().any(|r| {
            r.error.as_deref()
                == Some(
                    "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
                )
                && r.policy.first().map(String::as_str) == Some("Up")
        }),
        "{internal:?}"
    );
}

/// A proxy configured by IP LITERAL needs no lookup of its own, so it really
/// carries the DNS session instead of being bypassed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dns_session_goes_through_a_proxy_configured_by_ip() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let upstream = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(dns.addr()),
        ..HttpProxyScript::default()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\nUpIp = http, 127.0.0.1, {}\n[Proxy Group]\n[Rule]\nPROTOCOL,DNS,UpIp\nFINAL,DIRECT\n",
        dns.addr().port(),
        upstream.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup goes through the proxy inside the bound");
    assert!(res.is_ok(), "resolution through the proxy: {res:?}");
    let expected = format!("CONNECT 127.0.0.1:{} HTTP/1.1", dns.addr().port());
    let heads = upstream.heads();
    assert_eq!(
        heads.first().map(|h| h.request_line.as_str()),
        Some(expected.as_str()),
        "the DNS session really went through the proxy"
    );
    let internal = internal_sessions(&engine);
    assert!(
        internal
            .iter()
            .any(|r| r.policy.first().map(String::as_str) == Some("UpIp")),
        "{internal:?}"
    );
    assert!(
        internal.iter().all(|r| r.error.is_none()),
        "an IP-configured proxy is not bypassed: {internal:?}"
    );
}

/// The hop below `Up` resolves to DIRECT, so DIRECT is what opens the socket —
/// and the name it has to resolve locally is `Up`'s own server. That is the
/// same loop as the one above, one hop further down.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dns_session_bypasses_a_host_named_proxy_above_direct() {
    for (what, proxies, groups) in [
        (
            "a direct alias",
            "Up = http, proxy.test, 8080, underlying-proxy=D\nD = direct",
            "",
        ),
        (
            "a group whose current member is DIRECT",
            "Up = http, proxy.test, 8080, underlying-proxy=Pick\nOther = http, 127.0.0.1, 9",
            "Pick = select, DIRECT, Other",
        ),
    ] {
        let dns = MockDns::spawn().await;
        dns.set("target.test", &["127.0.0.1"], &[], 60);
        dns.set("proxy.test", &["127.0.0.1"], &[], 60);
        let dir = tempfile::tempdir().unwrap();
        let profile = format!(
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\n{proxies}\n[Proxy Group]\n{groups}\n[Rule]\nPROTOCOL,DNS,Up\nFINAL,DIRECT\n",
            dns.addr().port()
        );
        let engine = engine_from_profile(dir.path(), &profile).await;
        let res = tokio::time::timeout(
            Duration::from_secs(5),
            engine
                .runtime()
                .stack
                .resolver
                .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
        )
        .await
        .unwrap_or_else(|_| {
            panic!("{what}: the lookup must not wait for the proxy's own name to be resolved")
        });
        assert!(
            res.is_ok(),
            "{what}: resolution through the pipeline: {res:?}"
        );
        let internal = internal_sessions(&engine);
        assert!(
            internal.iter().any(|r| {
                r.error.as_deref()
                    == Some(
                        "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
                    )
                    && r.policy.first().map(String::as_str) == Some("Up")
            }),
            "{what}: a bypassed internal DNS session: {internal:?}"
        );
    }
}

/// A chain whose hop below the proxy is REJECT can never carry the DNS
/// session: it is bypassed like a REJECT of the session itself, instead of
/// failing the lookup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dns_session_bypasses_a_proxy_chain_that_ends_at_a_reject() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\nUpIp = http, 127.0.0.1, 9, underlying-proxy=Pick\n\
[Proxy Group]\nPick = select, REJECT, DIRECT\n[Rule]\nPROTOCOL,DNS,UpIp\nFINAL,DIRECT\n",
        dns.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup ends inside the bound");
    assert!(res.is_ok(), "resolution around the chain: {res:?}");
    let internal = internal_sessions(&engine);
    assert!(
        internal.iter().any(|r| {
            r.error.as_deref() == Some("dns-follow: reject bypassed to keep DNS working")
                && r.policy.first().map(String::as_str) == Some("UpIp")
        }),
        "a bypassed internal DNS session: {internal:?}"
    );
}

/// The other direction: `Up` is named by host name too, but its own server
/// travels to `UpIp` as a CONNECT target and is never resolved on this
/// machine, so there is no loop and nothing to bypass. A future widening of
/// the guard to "any host-named hop in the chain" has to fail here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_named_hop_above_an_ip_proxy_is_not_bypassed() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    // second hop: whatever it is asked to reach, it lands on the DNS server
    let far = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(dns.addr()),
        ..HttpProxyScript::default()
    })
    .await;
    // first hop: whatever it is asked to reach, it lands on `far`
    let near = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(far.addr()),
        ..HttpProxyScript::default()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\nUp = http, proxy.test, 8080, underlying-proxy=UpIp\nUpIp = http, 127.0.0.1, {}\n\
[Proxy Group]\n[Rule]\nPROTOCOL,DNS,Up\nFINAL,DIRECT\n",
        dns.addr().port(),
        near.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup goes through the chain inside the bound");
    assert!(res.is_ok(), "resolution through the chain: {res:?}");
    // the host name left this machine as a CONNECT target, unresolved
    assert_eq!(
        near.heads().first().map(|h| h.request_line.as_str()),
        Some("CONNECT proxy.test:8080 HTTP/1.1")
    );
    let expected = format!("CONNECT 127.0.0.1:{} HTTP/1.1", dns.addr().port());
    assert_eq!(
        far.heads().first().map(|h| h.request_line.as_str()),
        Some(expected.as_str())
    );
    let internal = internal_sessions(&engine);
    assert!(
        internal
            .iter()
            .any(|r| r.policy.first().map(String::as_str) == Some("Up")),
        "{internal:?}"
    );
    assert!(
        internal.iter().all(|r| r.error.is_none()),
        "nothing in this chain is resolved locally: {internal:?}"
    );
}

/// A follow-mode profile whose encrypted DNS servers are `servers` (their
/// certificates, the fakes' own, not checked), with `proxies`, and `rules`
/// before `FINAL,DIRECT`.
fn follow_profile(servers: &str, proxies: &str, rules: &str) -> String {
    follow_profile_with("", servers, proxies, rules)
}

/// `follow_profile` with `general` — more `[General]` lines — added.
fn follow_profile_with(general: &str, servers: &str, proxies: &str, rules: &str) -> String {
    format!(
        "[General]\nencrypted-dns-follow-outbound-mode = true\n\
encrypted-dns-server = {servers}\nencrypted-dns-skip-cert-verification = true\n\
ipv6 = false\n{general}\n[Proxy]\n{proxies}\n[Proxy Group]\n[Rule]\n{rules}\nFINAL,DIRECT\n"
    )
}

/// `target.test` through the engine's resolver, within the bound.
async fn lookup_outcome(engine: &Engine) -> Result<rurge_dns::DnsResult, rurge_dns::DnsError> {
    tokio::time::timeout(
        Duration::from_secs(10),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup within the bound")
}

/// `target.test` through the engine's resolver, answered within the bound.
async fn lookup_target(engine: &Engine) -> rurge_dns::DnsResult {
    lookup_outcome(engine).await.expect("an answer")
}

/// The QUIC carriers recorded so far: the internal UDP sessions.
fn carrier_sessions(engine: &Engine) -> Vec<RequestRecord> {
    let mut all = internal_sessions(engine);
    all.retain(|r| r.transport == Transport::Udp);
    all
}

/// A socket hook that notes the TOS it is asked to set, and for which family
/// of socket: what the sockets of a direct connector carry of their policy's
/// settings.
#[derive(Default)]
struct TosHook {
    calls: Mutex<Vec<(Family, u8)>>,
}

impl SocketHook for TosHook {
    fn bind_interface(&self, _: &socket2::Socket, _: &str, _: Family) -> io::Result<()> {
        Ok(())
    }

    fn set_tos(&self, _: &socket2::Socket, family: Family, tos: u8) -> io::Result<()> {
        self.calls.lock().unwrap().push((family, tos));
        Ok(())
    }
}

/// In follow mode a DoQ server's QUIC carrier goes by the rules (phase 2 M7
/// design 7.3): `PROTOCOL,DOQ` sends it through a SOCKS5 proxy, whose UDP
/// association takes the QUIC packets to the server. The carrier is an
/// internal UDP session of the request log, its bytes counted both ways.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_follows_the_rules_through_a_socks5_proxy() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let proxy = FakeSocks5::spawn(Socks5Script::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("quic://{}", doq.addr()),
        &format!(
            "Socks = socks5, 127.0.0.1, {}, udp-relay=true",
            proxy.addr().port()
        ),
        "PROTOCOL,DOQ,Socks",
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(res.v4, [Ipv4Addr::LOCALHOST]);
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    // UDP associations only, and every packet through them to the server
    let requests = proxy.requests();
    assert!(
        !requests.is_empty() && requests.iter().all(|r| r.command == 3),
        "{requests:?}"
    );
    let server = Target::new(HostName::Ip(doq.addr().ip()), doq.addr().port());
    let datagrams = proxy.datagrams();
    assert!(
        !datagrams.is_empty() && datagrams.iter().all(|to| *to == server),
        "{datagrams:?}"
    );
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers
            .iter()
            .any(|r| r.protocol == Some(ProtocolKind::Doq)
                && r.rule.as_deref() == Some("PROTOCOL,DOQ,Socks")
                && r.policy == ["Socks"]
                && r.dst == doq.addr().to_string()
                && r.up > 0
                && r.down > 0
                && r.error.is_none()),
        "{carriers:?}"
    );
}

/// The same for a DoH3 server, sent by an IP rule. Port 443 alone tags a
/// carrier `DOH3`, and a loopback server here has another: this one is
/// tagged `DOQ` (best effort, as the TCP connections' tags are).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doh3_carrier_follows_an_ip_rule_through_a_socks5_proxy() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doh3 = FakeDoh3::spawn(&dns, Doh3Script::default());
    let proxy = FakeSocks5::spawn(Socks5Script::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("h3://{}", doh3.addr()),
        &format!(
            "Socks = socks5, 127.0.0.1, {}, udp-relay=true",
            proxy.addr().port()
        ),
        "IP-CIDR,127.0.0.1/32,Socks",
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(res.v4, [Ipv4Addr::LOCALHOST]);
    assert_eq!(
        res.source,
        Source::Upstream(format!("h3://{}/dns-query", doh3.addr()))
    );
    assert!(!doh3.requests().is_empty());
    let server = Target::new(HostName::Ip(doh3.addr().ip()), doh3.addr().port());
    let datagrams = proxy.datagrams();
    assert!(
        !datagrams.is_empty() && datagrams.iter().all(|to| *to == server),
        "{datagrams:?}"
    );
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers
            .iter()
            .any(|r| r.protocol == Some(ProtocolKind::Doq)
                && r.rule.as_deref() == Some("IP-CIDR,127.0.0.1/32,Socks")
                && r.policy == ["Socks"]
                && r.dst == doh3.addr().to_string()
                && r.up > 0
                && r.down > 0),
        "{carriers:?}"
    );
}

/// `PROTOCOL,DOQ,DIRECT`: the carrier goes out through DIRECT's own UDP —
/// with its socket settings, as a TCP DNS session goes through DIRECT's
/// TCP — and is recorded as any internal session is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_routed_to_direct_is_an_internal_udp_session() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(&format!("quic://{}", doq.addr()), "", "PROTOCOL,DOQ,DIRECT");
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    assert_eq!(doq.connections(), 1);
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers
            .iter()
            .any(|r| r.protocol == Some(ProtocolKind::Doq)
                && r.rule.as_deref() == Some("PROTOCOL,DOQ,DIRECT")
                && r.policy == ["DIRECT"]
                && r.up > 0
                && r.down > 0
                && r.connect_ms.is_some()
                && r.first_byte_ms.is_some()
                && r.error.is_none()),
        "{carriers:?}"
    );
}

/// A rule that rejects the carrier would cost the resolver its server: the
/// carrier goes directly instead, noted, as a TCP DNS session does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_routed_to_reject_goes_directly() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(&format!("quic://{}", doq.addr()), "", "PROTOCOL,DOQ,REJECT");
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    assert_eq!(doq.connections(), 1);
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers.iter().any(|r| r.policy == ["REJECT"]
            && r.error.as_deref() == Some("dns-follow: reject bypassed to keep DNS working")
            && r.up > 0
            && r.down > 0),
        "{carriers:?}"
    );
}

/// A DNS server at a port nothing listens on — the carrier a connected
/// socket here, gone directly around a REJECT. Windows tells such a socket
/// of the datagram nobody took as a reset of its next receive; QUIC passes
/// over that and sends its first packet again, on the same carrier, whose
/// session stays an active one: no failure goes on the record. Windows
/// only: no other system hands a receive a reset for this, and what the
/// session does with one is pinned for all of them in `dns_pipeline`'s own
/// tests.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_that_outlives_a_port_unreachable_is_no_failed_session() {
    /// What QUIC pads the datagram of its first packet to (RFC 9000 14.1):
    /// a carrier that has sent more has sent again.
    const INITIAL: u64 = 1200;
    // where the packets go, and no more: the port is never bound again
    let addr = std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(&format!("quic://{addr}"), "", "PROTOCOL,DOQ,REJECT");
    let engine = engine_from_profile(dir.path(), &profile).await;
    let asking = engine.clone();
    let lookup = tokio::spawn(async move { lookup_outcome(&asking).await });
    // the packet goes again about a second after the first: the reset,
    // which came back at once, has been read by then
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let carriers = carrier_sessions(&engine);
        assert!(
            carriers.iter().all(|r| r.status != RecordStatus::Failed),
            "{carriers:?}"
        );
        let sent_again = carriers
            .iter()
            .any(|r| r.status == RecordStatus::Active && r.up > INITIAL);
        if sent_again {
            break;
        }
        // a lookup that is over met something at the port — another test
        // took it meanwhile —: its carrier had no reset to live through
        if lookup.is_finished() {
            assert!(!carriers.is_empty(), "the lookup opened no carrier");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the first packet did not go again: {carriers:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    lookup.abort();
}

/// A proxy configured by host name would need the very lookup the carrier
/// is for: the carrier goes directly instead, noted, and the proxy's name
/// is never asked for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_bypasses_a_proxy_configured_by_host_name() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("proxy.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("quic://{}", doq.addr()),
        "Up = socks5, proxy.test, 1080, udp-relay=true",
        "PROTOCOL,DOQ,Up",
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    let questions = dns.queries();
    assert!(
        questions.iter().all(|(q, _)| q.name != "proxy.test"),
        "{questions:?}"
    );
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers.iter().any(|r| r.policy == ["Up"]
            && r.error.as_deref()
                == Some(
                    "dns-follow: proxy configured by host name bypassed to avoid a resolution loop"
                )),
        "{carriers:?}"
    );
}

/// A proxy that carries no UDP cannot take the carrier: that server fails
/// this time — not sent around the rule — while the resolver's other server
/// answers (phase 2 M7 design 7.3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_server_routed_to_a_proxy_without_udp_fails_while_another_answers() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let blocked = FakeDoq::spawn(&dns, DoqScript::default());
    let open = FakeDoq::spawn(&dns, DoqScript::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("quic://{}, quic://{}", blocked.addr(), open.addr()),
        "NoUdp = http, 127.0.0.1, 9",
        &format!("DEST-PORT,{},NoUdp", blocked.addr().port()),
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", open.addr()))
    );
    let failed = wait_for_record(&engine, Duration::from_secs(5), |r| {
        r.dst == blocked.addr().to_string() && r.status == RecordStatus::Failed
    })
    .await
    .expect("the blocked server's carrier failed");
    assert_eq!(failed.transport, Transport::Udp);
    assert_eq!(failed.policy, ["NoUdp"]);
    assert_eq!(failed.error.as_deref(), Some("policy does not support UDP"));
    assert_eq!(blocked.connections(), 0);
    assert!(blocked.queries().is_empty());
}

/// `udp-policy-not-supported-behaviour = DIRECT` goes for the carrier as it
/// goes for any UDP flow: a proxy that carries no UDP is gone around, through
/// DIRECT's UDP; the record keeps the policy the rules picked and says so,
/// and the proxy is not asked for anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_routed_to_a_proxy_without_udp_may_go_through_direct() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let proxy = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile_with(
        "udp-policy-not-supported-behaviour = DIRECT",
        &format!("quic://{}", doq.addr()),
        &format!("NoUdp = http, 127.0.0.1, {}", proxy.addr().port()),
        "PROTOCOL,DOQ,NoUdp",
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    assert_eq!(doq.connections(), 1);
    assert!(proxy.heads().is_empty(), "{:?}", proxy.heads());
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers
            .iter()
            .any(|r| r.protocol == Some(ProtocolKind::Doq)
                && r.rule.as_deref() == Some("PROTOCOL,DOQ,NoUdp")
                && r.policy == ["NoUdp"]
                && r.error.as_deref() == Some("policy does not support UDP; sent through DIRECT")
                && r.up > 0
                && r.down > 0),
        "{carriers:?}"
    );
}

/// A REJECT at the end of the chain is not a policy that carries no UDP: it
/// is gone around with its own note, whichever way
/// `udp-policy-not-supported-behaviour` goes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reject_is_still_bypassed_where_a_policy_without_udp_may_go_through_direct() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile_with(
        "udp-policy-not-supported-behaviour = DIRECT",
        &format!("quic://{}", doq.addr()),
        "",
        "PROTOCOL,DOQ,REJECT",
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers.iter().any(|r| r.policy == ["REJECT"]
            && r.error.as_deref() == Some("dns-follow: reject bypassed to keep DNS working")
            && r.up > 0
            && r.down > 0),
        "{carriers:?}"
    );
}

/// A DIRECT alias carries the carrier on its own UDP, with its socket
/// settings — here the TOS, which reaches the sockets through the hook, as
/// it reaches a TCP DNS session's — and not on a plain socket of the
/// pipeline's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_routed_to_a_direct_alias_has_its_socket_settings() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let hook = Arc::new(TosHook::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("quic://{}", doq.addr()),
        "Marked = direct, tos=0x28",
        "PROTOCOL,DOQ,Marked",
    );
    let engine = engine_with_hook(dir.path(), &profile, hook.clone()).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    let calls = hook.calls.lock().unwrap().clone();
    assert!(calls.contains(&(Family::V4, 0x28)), "{calls:?}");
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers.iter().any(|r| r.policy == ["Marked", "DIRECT"]
            && r.up > 0
            && r.down > 0
            && r.error.is_none()),
        "{carriers:?}"
    );
}

/// What the resolver hears of a carrier that failed names the policy — the
/// text a proxy fails with could pass for the DNS server's own — and says
/// that a proxy without UDP carries none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_resolver_hears_which_policy_carries_no_udp() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("quic://{}", doq.addr()),
        "NoUdp = http, 127.0.0.1, 9",
        "PROTOCOL,DOQ,NoUdp",
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let err = lookup_outcome(&engine)
        .await
        .expect_err("the only server is out of reach");
    assert!(
        err.to_string()
            .contains("via NoUdp: policy does not support UDP"),
        "{err}"
    );
    assert_eq!(doq.connections(), 0);
}

/// A proxy that carries UDP but will not open the association — it refuses
/// the policy's login —: the resolver hears the proxy's own words behind its
/// name, the record is a failed carrier, and neither tells the password the
/// policy tried.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_the_proxy_will_not_open_fails_naming_the_policy() {
    const TRIED: &str = "wr0ng-passw0rd";
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    // wants another password than the policy's
    let proxy = FakeSocks5::spawn(Socks5Script {
        auth: Some(("user".into(), "secret".into())),
        ..Socks5Script::default()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("quic://{}", doq.addr()),
        &format!(
            "Socks = socks5, 127.0.0.1, {}, user, {TRIED}, udp-relay=true",
            proxy.addr().port()
        ),
        "PROTOCOL,DOQ,Socks",
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let refusal = "socks5: authentication failed";
    let err = lookup_outcome(&engine)
        .await
        .expect_err("the proxy refuses the carrier");
    assert!(
        err.to_string().contains(&format!("via Socks: {refusal}")),
        "{err}"
    );
    let failed = wait_for_record(&engine, Duration::from_secs(5), |r| {
        r.transport == Transport::Udp && r.status == RecordStatus::Failed
    })
    .await
    .expect("the failed carrier is recorded");
    assert_eq!(failed.policy, ["Socks"]);
    assert_eq!(failed.error.as_deref(), Some(refusal));
    // the proxy's TCP connection was made before it refused the login, and it
    // is no address of a UDP record
    assert_eq!(failed.remote_addr, None, "{failed:?}");
    let told = format!("{err} {err:?} {failed:?}");
    assert!(!told.contains(TRIED), "{told}");
    assert_eq!(doq.connections(), 0);
}

/// A proxy whose carrier opens and then cannot send — a MASQUE server
/// without CONNECT-UDP says so only once the first packet is to go — fails
/// the server at once, not by a handshake left to time out: the resolver
/// hears the proxy's own words behind its name, and the carrier's record
/// is a failure with them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doq_carrier_the_proxy_cannot_send_on_fails_naming_the_policy() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let proxy = FakeH3Proxy::spawn(H3ProxyScript {
        no_extended_connect: true,
        ..H3ProxyScript::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("quic://{}", doq.addr()),
        &format!(
            "M = masque, 127.0.0.1, {}, skip-cert-verify=true",
            proxy.addr().port()
        ),
        "PROTOCOL,DOQ,M",
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let refusal = "masque: the server does not support CONNECT-UDP";
    let err = lookup_outcome(&engine)
        .await
        .expect_err("the proxy carries no UDP");
    assert!(
        err.to_string().contains(&format!("via M: {refusal}")),
        "{err}"
    );
    let failed = wait_for_record(&engine, Duration::from_secs(5), |r| {
        r.transport == Transport::Udp && r.status == RecordStatus::Failed
    })
    .await
    .expect("the failed carrier is recorded");
    assert_eq!(failed.policy, ["M"]);
    assert_eq!(failed.error.as_deref(), Some(refusal));
    assert_eq!(doq.connections(), 0);
}

/// Without `encrypted-dns-follow-outbound-mode` a carrier goes straight to
/// its server whatever the rules say: no internal session, no proxy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_follow_mode_a_doq_carrier_ignores_the_rules() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let proxy = FakeSocks5::spawn(Socks5Script::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nencrypted-dns-server = quic://{}\nencrypted-dns-skip-cert-verification = true\n\
ipv6 = false\n[Proxy]\nSocks = socks5, 127.0.0.1, {}, udp-relay=true\n[Proxy Group]\n\
[Rule]\nPROTOCOL,DOQ,Socks\nFINAL,REJECT\n",
        doq.addr(),
        proxy.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = lookup_target(&engine).await;
    assert_eq!(
        res.source,
        Source::Upstream(format!("quic://{}", doq.addr()))
    );
    assert_eq!(doq.connections(), 1);
    assert!(proxy.requests().is_empty());
    let internal = internal_sessions(&engine);
    assert!(internal.is_empty(), "{internal:?}");
}

#[tokio::test]
async fn persisted_group_selection_is_honored() {
    // Pick = select, HK, DIRECT; a state selecting DIRECT must resolve to DIRECT
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let target = TestServer::spawn().await;
    target.set("/hello", "hi there");
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {}\nipv6 = false\n\
[Proxy]\nHK = tailscale, section-name=ts\n[Proxy Group]\nPick = select, HK, DIRECT\n\
[Rule]\nDOMAIN,target.test,Pick\nFINAL,DIRECT\n",
        dns.addr()
    );
    let loaded = from_text(&profile, &dir.path().join("t.conf"), &load_options());
    assert!(!loaded.diagnostics.has_errors());
    let mut selections = std::collections::HashMap::new();
    selections.insert("Pick".to_string(), "DIRECT".to_string());
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
                dns_connector: None,
                socket_hook: Arc::new(rurge_net::socket::NoopSocketHook),
            },
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            request_log_size: 1000,
            shared: EngineShared::new(GroupSelections::from_map(selections)),
        },
    )
    .await
    .unwrap();
    // diagnostics() is reachable and clean here
    assert!(!runtime.diagnostics().has_errors());
    let engine = Engine::new(runtime);
    let listeners = engine.bind_listeners().await.unwrap();
    let http = listeners
        .iter()
        .find(|(s, _)| s.kind == ListenerKind::Http)
        .unwrap()
        .1
        .local_addr;
    // Pick → DIRECT (persisted) → the target is reachable
    let (head, body) = get_via_proxy(
        http,
        &format!(
            "http://target.test:{}/hello",
            target.url("/").port().unwrap()
        ),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");
    let rec = wait_for_record(&engine, Duration::from_secs(3), |r| {
        r.policy.iter().any(|p| p == "DIRECT")
    })
    .await;
    assert!(
        rec.is_some(),
        "resolved through DIRECT (persisted selection)"
    );
}

#[tokio::test]
async fn runtime_mode_and_global_policy_override_routing() {
    use rurge_engine::control::Mode;
    // rules reject the target; the runtime mode can bypass them
    let h = harness("", "DOMAIN,target.test,REJECT", OutboundMode::Rule).await;
    let url = format!("http://target.test:{}/hello", h.target_port());
    let (head, _) = get_via_proxy(h.http(), &url).await;
    assert!(head.is_empty(), "rules reject: {head}");
    assert_eq!(h.engine.mode(), Mode::Rule);
    h.engine.set_mode(Mode::Direct).await;
    let (head, body) = get_via_proxy(h.http(), &url).await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "direct mode bypasses rules: {head}"
    );
    assert_eq!(body, b"hi there");
    // proxy mode routes through the global policy (Block = reject-tinygif)
    h.engine.set_global_policy("Block").await.unwrap();
    h.engine.set_mode(Mode::Proxy).await;
    let (head, body) = get_via_proxy(h.http(), &url).await;
    assert!(
        head.starts_with("HTTP/1.1 200") && body.len() == 43,
        "{head}"
    );
    // unknown policy is refused; a builtin is accepted
    assert!(h.engine.set_global_policy("nope").await.is_err());
    h.engine.set_global_policy("DIRECT").await.unwrap();
    let (head, body) = get_via_proxy(h.http(), &url).await;
    assert!(
        head.starts_with("HTTP/1.1 200") && body == b"hi there",
        "{head}"
    );
    // proxy mode with the global policy cleared falls back to the rules
    h.engine.set_global_policy("").await.unwrap();
    assert_eq!(h.engine.global_policy(), None);
    let (head, _) = get_via_proxy(h.http(), &url).await;
    assert!(head.is_empty(), "no global policy → rules → reject: {head}");
    // views
    let pv = h.engine.policies_view();
    assert!(pv.proxies.iter().any(|p| p == "DIRECT") && pv.proxies.iter().any(|p| p == "HK"));
    assert!(pv.groups.iter().any(|g| g == "Pick"));
    let rules = h.engine.rules_view();
    assert!(
        rules
            .iter()
            .any(|r| r.rule == "DOMAIN,target.test,REJECT" && r.hits >= 1)
    );
    assert!(rules.last().unwrap().rule.starts_with("FINAL"));
    let text = h.engine.config_text(false).await.unwrap();
    assert!(
        text.contains("password=***") && !text.contains("password=x"),
        "{text}"
    );
    let full = h.engine.config_text(true).await.unwrap();
    assert!(full.contains("password=x"));
}

// What a dial leaves on the request record: the port of the listener the
// session came in on, where the socket it opened itself went, how long it
// waited for name lookups.

/// A DIRECT session says which listener it came in on and where its
/// connection went. Its target is an address, so it looked nothing up.
#[tokio::test]
async fn a_direct_session_records_its_listener_and_the_address_it_connected_to() {
    let h = common::harness(common::Profile::default()).await;
    let echo = rurge_proto::testing::echo_server().await;
    let mut over_http = common::connect_via_http(h.http(), &echo.to_string()).await;
    common::echo_through(&mut over_http, b"http").await;
    drop(over_http);
    let mut over_socks = common::connect_via_socks5(h.socks(), "127.0.0.1", echo.port()).await;
    common::echo_through(&mut over_socks, b"socks").await;
    drop(over_socks);
    let records = common::the_records(&h, 2).await;
    let by = |kind| {
        records
            .iter()
            .find(|r| r.listener == kind)
            .unwrap_or_else(|| panic!("no {kind:?} record in {records:?}"))
    };
    let (http, socks) = (by(ListenerKind::Http), by(ListenerKind::Socks5));
    assert_eq!(http.listen_port, h.http().port());
    assert_eq!(socks.listen_port, h.socks().port());
    for r in [http, socks] {
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
        assert_eq!(r.remote_addr, Some(echo));
        assert_eq!(r.dns_ms, None, "an address needs no lookup: {r:?}");
    }
}

/// A lookup the dial waits for counts, whoever answers it: here `[Host]`,
/// which answers in next to no time.
#[tokio::test]
async fn a_name_pinned_in_host_counts_as_a_lookup() {
    let h = common::harness(common::Profile {
        hosts: "pinned.test = 127.0.0.1",
        ..common::Profile::default()
    })
    .await;
    let echo = rurge_proto::testing::echo_server().await;
    let authority = format!("pinned.test:{}", echo.port());
    let mut tunnel = common::connect_via_http(h.http(), &authority).await;
    common::echo_through(&mut tunnel, b"by name").await;
    drop(tunnel);
    let r = common::the_records(&h, 1).await.remove(0);
    assert_eq!(r.remote_addr, Some(echo));
    assert!(r.dns_ms.is_some(), "{r:?}");
}

/// A session through a proxy connected to the proxy, and that is the address
/// on its record. The proxy resolves the name: this machine looked up none.
#[tokio::test]
async fn a_session_through_a_proxy_records_the_proxy_it_connected_to() {
    let echo = rurge_proto::testing::echo_server().await;
    let up = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("Up = socks5, 127.0.0.1, {}", up.addr().port());
    let h = common::harness(common::Profile {
        proxies: &proxies,
        rules: "DOMAIN,target.test,Up",
        ..common::Profile::default()
    })
    .await;
    let mut tunnel = common::connect_via_http(h.http(), "target.test:7").await;
    common::echo_through(&mut tunnel, b"via the proxy").await;
    drop(tunnel);
    let r = common::the_records(&h, 1).await.remove(0);
    assert_eq!(r.policy, ["Up"]);
    assert_eq!(r.remote_addr, Some(up.addr()));
    assert_eq!(r.dns_ms, None, "{r:?}");
}

/// Of a chain the record has the first hop: the address this machine's
/// socket went to. The names behind it are the proxies' to look up.
#[tokio::test]
async fn a_chained_session_records_the_first_hop() {
    let echo = rurge_proto::testing::echo_server().await;
    let exit = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(echo),
        ..HttpProxyScript::default()
    })
    .await;
    let entry = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(exit.addr()),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!(
        "Entry = socks5, 127.0.0.1, {}\nExit = http, exit.example, 8080, underlying-proxy=Entry",
        entry.addr().port()
    );
    let h = common::harness(common::Profile {
        proxies: &proxies,
        rules: "DOMAIN,target.test,Exit",
        ..common::Profile::default()
    })
    .await;
    let mut tunnel = common::connect_via_http(h.http(), "target.test:7").await;
    common::echo_through(&mut tunnel, b"two hops").await;
    drop(tunnel);
    let r = common::the_records(&h, 1).await.remove(0);
    assert_eq!(r.policy, ["Exit"]);
    assert_eq!(r.remote_addr, Some(entry.addr()));
    assert_eq!(r.dns_ms, None, "{r:?}");
}

/// A session the rules reject opened no socket. What the rules had to look up
/// to decide counts all the same; a rule that needs no lookup leaves none.
#[tokio::test]
async fn a_rejected_session_has_no_remote_address_but_the_lookups_of_the_rules() {
    let h = common::harness(common::Profile {
        rules: "DOMAIN,ads.test,REJECT\nIP-CIDR,127.0.0.1/32,REJECT",
        ..common::Profile::default()
    })
    .await;
    common::plain_get(h.http(), "http://ads.test/", "ads.test").await;
    common::plain_get(h.http(), "http://target.test/", "target.test").await;
    let records = common::the_records(&h, 2).await;
    let of = |host: &str| {
        records
            .iter()
            .find(|r| r.dst.starts_with(host))
            .unwrap_or_else(|| panic!("no record of {host} in {records:?}"))
    };
    let (ads, target) = (of("ads.test"), of("target.test"));
    for r in [ads, target] {
        assert!(matches!(r.status, RecordStatus::Rejected(_)), "{r:?}");
        assert_eq!(r.remote_addr, None);
        assert_eq!(r.listen_port, h.http().port());
    }
    assert_eq!(ads.dns_ms, None, "a domain rule looks nothing up: {ads:?}");
    assert!(target.dns_ms.is_some(), "the IP rule did: {target:?}");
}

/// Two sessions dialling together each record their own address and their
/// own lookup time: the one that waits for a slow answer has the wait, the
/// one that connects to an address meanwhile has none.
#[tokio::test]
async fn sessions_dialling_together_record_their_own_address_and_lookup_time() {
    let h = common::harness(common::Profile::default()).await;
    h.dns.set("slow.test", &["127.0.0.1"], &[], 60);
    let delay = Duration::from_millis(400);
    h.dns.set_delay(delay);
    let (slow, quick) = (
        rurge_proto::testing::echo_server().await,
        rurge_proto::testing::echo_server().await,
    );
    let (slow_authority, quick_authority) =
        (format!("slow.test:{}", slow.port()), quick.to_string());
    let (mut slow_tunnel, mut quick_tunnel) = tokio::join!(
        common::connect_via_http(h.http(), &slow_authority),
        common::connect_via_http(h.http(), &quick_authority),
    );
    common::echo_through(&mut slow_tunnel, b"slow").await;
    common::echo_through(&mut quick_tunnel, b"quick").await;
    drop((slow_tunnel, quick_tunnel));
    let records = common::the_records(&h, 2).await;
    let of = |echo: SocketAddr| {
        records
            .iter()
            .find(|r| r.remote_addr == Some(echo))
            .unwrap_or_else(|| panic!("no record at {echo} in {records:?}"))
    };
    let (slow, quick) = (of(slow), of(quick));
    assert!(slow.dst.starts_with("slow.test:"), "{slow:?}");
    assert!(slow.dns_ms.unwrap() >= delay.as_millis() as u64, "{slow:?}");
    assert_eq!(quick.dns_ms, None, "{quick:?}");
}

/// Keeps the connectivity tests off a member: the sessions of a test round
/// are records of the request log as well, and the members' fakes would
/// count their connections.
const UNTESTED: &str = "test-url=http://[";

/// When a `smart` group moves on from a member it could not reach, the
/// record has the address of the member that connected.
#[tokio::test]
async fn after_a_smart_group_moves_on_the_record_has_the_member_that_connected() {
    let echo = rurge_proto::testing::echo_server().await;
    let good = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!(
        "Dead = socks5, 127.0.0.1, {}, {UNTESTED}\nGood = socks5, 127.0.0.1, {}, {UNTESTED}",
        common::closed_tcp_port(),
        good.addr().port()
    );
    let h = common::harness(common::Profile {
        proxies: &proxies,
        groups: "S = smart, Dead, Good",
        rules: "DOMAIN,target.test,S",
        ..common::Profile::default()
    })
    .await;
    let mut tunnel = common::connect_via_http(h.http(), "target.test:7").await;
    common::echo_through(&mut tunnel, b"second member").await;
    drop(tunnel);
    let r = common::the_records(&h, 1).await.remove(0);
    assert_eq!(r.policy, ["S", "Good"]);
    assert_eq!(r.remote_addr, Some(good.addr()));
}

/// Each attempt of a `smart` group's dial starts from nothing: a member that
/// connected and then failed in its handshake leaves no address behind when
/// the next one does not connect at all.
#[tokio::test]
async fn a_smart_group_forgets_the_address_of_a_member_that_failed() {
    let refusing = FakeSocks5::spawn(Socks5Script {
        reply: 5,
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!(
        "Refuses = socks5, 127.0.0.1, {}, {UNTESTED}\nDead = socks5, 127.0.0.1, {}, {UNTESTED}",
        refusing.addr().port(),
        common::closed_tcp_port()
    );
    let h = common::harness(common::Profile {
        proxies: &proxies,
        groups: "S = smart, Refuses, Dead",
        rules: "DOMAIN,target.test,S",
        ..common::Profile::default()
    })
    .await;
    let r = common::a_failed_session(&h).await;
    assert_eq!(refusing.requests().len(), 1, "the first member did connect");
    assert_eq!(r.remote_addr, None, "{r:?}");
}

/// DNS sessions rurge starts itself record where they went, too: the server
/// of a `tcp://` upstream. They came in on no listener.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_internal_dns_session_records_the_server_it_connected_to() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(&format!("tcp://{}", dns.addr()), "", "");
    let engine = engine_from_profile(dir.path(), &profile).await;
    lookup_target(&engine).await;
    let sessions = internal_sessions(&engine);
    let session = sessions
        .iter()
        .find(|r| r.remote_addr.is_some())
        .unwrap_or_else(|| panic!("no session of the TCP server in {sessions:?}"));
    assert_eq!(session.remote_addr, Some(dns.addr()));
    assert_eq!((session.listen_port, session.dns_ms), (0, None));
}

/// A QUIC carrier is UDP: the control connection of the SOCKS5 association
/// it rides on is no address of its record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_carrier_over_a_socks5_association_has_no_remote_address() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let proxy = FakeSocks5::spawn(Socks5Script::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let profile = follow_profile(
        &format!("quic://{}", doq.addr()),
        &format!(
            "Socks = socks5, 127.0.0.1, {}, udp-relay=true",
            proxy.addr().port()
        ),
        "PROTOCOL,DOQ,Socks",
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    lookup_target(&engine).await;
    let requests = proxy.requests();
    assert!(
        !requests.is_empty() && requests.iter().all(|r| r.command == 3),
        "the proxy was reached, for its UDP association: {requests:?}"
    );
    let carriers = carrier_sessions(&engine);
    assert!(
        carriers.iter().any(|r| r.policy == ["Socks"]),
        "{carriers:?}"
    );
    for carrier in &carriers {
        assert_eq!(carrier.remote_addr, None, "{carrier:?}");
    }
}
