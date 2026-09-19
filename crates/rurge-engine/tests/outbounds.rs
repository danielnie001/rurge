//! Sessions that leave through real proxy outbounds: profile text → Runtime →
//! Engine → loopback listeners → scripted loopback upstreams
//! (`rurge_proto::testing`) → `TestServer` (M1 design §8).

use rurge_config::config::{LoadOptions, from_text};
use rurge_config::session::ListenerKind;
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_engine::stack::StackOptions;
use rurge_engine::{Engine, EngineShared, ListenerSpec, Runtime, RuntimeOptions};
use rurge_inbound::Running;
use rurge_net::socket::NoopSocketHook;
use rurge_net::testing::TestServer;
use rurge_proto::testing::{FakeHttpProxy, HttpProxyScript};
use rurge_rules::{GeoUrls, OutboundMode};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Harness {
    dir: tempfile::TempDir,
    engine: Arc<Engine>,
    listeners: Vec<(ListenerSpec, Running)>,
    dns: MockDns,
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
    // Unused by this task's own tests; part of the harness for the SOCKS5
    // outbound tests later tasks add to this file.
    #[allow(dead_code)]
    fn socks(&self) -> SocketAddr {
        self.addr_of(ListenerKind::Socks5)
    }
}

fn stack_options(dir: &std::path::Path) -> StackOptions {
    StackOptions {
        data_dir: dir.to_path_buf(),
        no_network: true,
        geo_urls: GeoUrls::default(),
        dns_cache_size: 2000,
        system: Arc::new(StaticSystemDns::default()),
        wait: Duration::ZERO,
        dns_connector: None,
        socket_hook: Arc::new(NoopSocketHook),
    }
}

async fn runtime(dir: &std::path::Path, profile: &str, shared: EngineShared) -> Runtime {
    std::fs::write(dir.join("t.conf"), profile).unwrap();
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
    Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack: stack_options(dir),
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            shared,
            request_log_size: 1000,
        },
    )
    .await
    .unwrap()
}

/// The variable parts of a test profile; everything else is fixed.
#[derive(Default)]
struct Profile<'a> {
    general: &'a str,
    proxies: &'a str,
    groups: &'a str,
    hosts: &'a str,
    /// Inserted before `FINAL,DIRECT`.
    rules: &'a str,
}

impl Profile<'_> {
    fn text(&self, dns: SocketAddr) -> String {
        format!(
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {dns}\nipv6 = false\n{}\n\
[Proxy]\n{}\n[Proxy Group]\n{}\n[Host]\n{}\n[Rule]\n{}\nFINAL,DIRECT\n",
            self.general, self.proxies, self.groups, self.hosts, self.rules
        )
    }
}

async fn harness(p: Profile<'_>) -> Harness {
    let dns = MockDns::spawn().await;
    for name in ["target.test", "alt.test"] {
        dns.set(name, &["127.0.0.1"], &[], 60);
    }
    let dir = tempfile::tempdir().unwrap();
    let text = p.text(dns.addr());
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
    let listeners = engine.bind_listeners().await.unwrap();
    Harness {
        dir,
        engine,
        listeners,
        dns,
    }
}

/// `CONNECT host:port` through rurge's HTTP listener; returns the tunnel.
async fn connect_via_http(proxy: SocketAddr, authority: &str) -> TcpStream {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut byte))
            .await
            .expect("the proxy answers")
            .unwrap();
        assert!(
            n > 0,
            "closed before the CONNECT response: {:?}",
            String::from_utf8_lossy(&head)
        );
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    s
}

/// One `GET` over an established tunnel (or any stream to an origin).
async fn get(stream: &mut TcpStream, host: &str, path: &str) -> String {
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf)).await;
    String::from_utf8_lossy(&buf).into_owned()
}

async fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_connect_leaves_through_the_http_upstream_with_the_name_unresolved() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let origin_addr: SocketAddr = format!("127.0.0.1:{}", origin.url("/").port().unwrap())
        .parse()
        .unwrap();
    let upstream = FakeHttpProxy::spawn(HttpProxyScript {
        auth: Some(("alice".into(), "s3cret".into())),
        connect_to: Some(origin_addr),
        ..HttpProxyScript::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Up = http, 127.0.0.1, {}, alice, s3cret",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,Up",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let response = get(&mut tunnel, "target.test", "/hello").await;
    assert!(response.ends_with("hi there"), "{response}");
    let head = &upstream.heads()[0];
    // the proxy resolves the name: rurge never looked it up
    assert_eq!(head.request_line, "CONNECT target.test:8080 HTTP/1.1");
    assert!(
        h.dns.queries().is_empty(),
        "a domain rule and a remote-resolving proxy need no DNS"
    );
    // The origin closing (Connection: close) only ends the download half; the
    // relay needs both directions closed before it finishes the session, so
    // the client side of the tunnel has to close too.
    drop(tunnel);
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    assert_eq!(log.recent(10)[0].policy, ["Up"]);
}

#[tokio::test]
async fn dropping_the_engine_empties_the_cell() {
    let h = harness(Profile {
        proxies: "Up = http, 127.0.0.1, 9",
        ..Profile::default()
    })
    .await;
    let shared = h.engine.shared();
    assert!(shared.cell.load().is_some());
    let Harness {
        engine,
        listeners,
        dir,
        ..
    } = h;
    drop(listeners);
    drop(engine);
    // the accept loops hold the last references and are aborted asynchronously
    wait_until("the engine to go away", || shared.cell.load().is_none()).await;
    drop(dir); // the profile outlives the engine that read it
}
