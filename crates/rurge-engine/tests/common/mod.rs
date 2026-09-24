//! What the test files of this directory share: the harness, the helpers and
//! the imports (re-exported, so a test file needs `use common::*` and nothing
//! else). Each test file is a crate of its own and uses a part of all this
//! only: hence the two `allow`s.

#![allow(dead_code, unused_imports)]

pub use rurge_config::config::{LoadOptions, from_text};
pub use rurge_config::session::ListenerKind;
pub use rurge_dns::system::StaticSystemDns;
pub use rurge_dns::testing::MockDns;
pub use rurge_engine::SelectError;
pub use rurge_engine::stack::StackOptions;
pub use rurge_engine::state::{STATE_FILE, StateStore, profile_key};
pub use rurge_engine::{Engine, EngineShared, ListenerSpec, RecordStatus, Runtime, RuntimeOptions};
pub use rurge_inbound::Running;
pub use rurge_net::socket::NoopSocketHook;
pub use rurge_net::testing::TestServer;
pub use rurge_proto::testing::{
    AnyTlsScript, FakeAnyTls, FakeHttpProxy, FakeSocks5, FakeTrojan, FakeVmess, HttpProxyScript,
    Socks5Script, TlsFixture, TrojanScript, VmessScript,
};
pub use rurge_rules::{GeoUrls, OutboundMode};
pub use std::net::SocketAddr;
pub use std::sync::Arc;
pub use std::time::Duration;
pub use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub use tokio::net::TcpStream;

pub struct Harness {
    pub dir: tempfile::TempDir,
    pub engine: Arc<Engine>,
    pub listeners: Vec<(ListenerSpec, Running)>,
    pub dns: MockDns,
}

impl Harness {
    pub fn addr_of(&self, kind: ListenerKind) -> SocketAddr {
        self.listeners
            .iter()
            .find(|(spec, _)| spec.kind == kind)
            .map(|(_, running)| running.local_addr)
            .unwrap_or_else(|| panic!("no {kind:?} listener"))
    }
    pub fn http(&self) -> SocketAddr {
        self.addr_of(ListenerKind::Http)
    }
    pub fn socks(&self) -> SocketAddr {
        self.addr_of(ListenerKind::Socks5)
    }
}

pub fn stack_options(dir: &std::path::Path) -> StackOptions {
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

pub async fn runtime(dir: &std::path::Path, profile: &str, shared: EngineShared) -> Runtime {
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
pub struct Profile<'a> {
    pub general: &'a str,
    pub proxies: &'a str,
    pub groups: &'a str,
    pub hosts: &'a str,
    /// Inserted before `FINAL,DIRECT`.
    pub rules: &'a str,
}

impl Profile<'_> {
    pub fn text(&self, dns: SocketAddr) -> String {
        format!(
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {dns}\nipv6 = false\n{}\n\
[Proxy]\n{}\n[Proxy Group]\n{}\n[Host]\n{}\n[Rule]\n{}\nFINAL,DIRECT\n",
            self.general, self.proxies, self.groups, self.hosts, self.rules
        )
    }
}

pub async fn harness(p: Profile<'_>) -> Harness {
    harness_with(p, EngineShared::default()).await
}

/// An engine whose outbounds trust `roots` instead of the operating system's.
pub async fn harness_trusting(p: Profile<'_>, roots: Arc<rustls::RootCertStore>) -> Harness {
    let shared = EngineShared {
        roots: Some(roots),
        ..EngineShared::default()
    };
    harness_with(p, shared).await
}

pub async fn harness_with(p: Profile<'_>, shared: EngineShared) -> Harness {
    let dns = MockDns::spawn().await;
    for name in ["target.test", "alt.test"] {
        dns.set(name, &["127.0.0.1"], &[], 60);
    }
    let dir = tempfile::tempdir().unwrap();
    let text = p.text(dns.addr());
    let engine = Engine::new(runtime(dir.path(), &text, shared).await);
    let listeners = engine.bind_listeners().await.unwrap();
    Harness {
        dir,
        engine,
        listeners,
        dns,
    }
}

/// `CONNECT host:port` through rurge's HTTP listener; returns the tunnel.
pub async fn connect_via_http(proxy: SocketAddr, authority: &str) -> TcpStream {
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
pub async fn get(stream: &mut TcpStream, host: &str, path: &str) -> String {
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

pub async fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// One request/response over a fresh connection to rurge's HTTP listener.
pub async fn plain_get(proxy: SocketAddr, url: &str, host: &str) -> String {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(
        format!(
            "GET {url} HTTP/1.1\r\nHost: {host}\r\nProxy-Connection: keep-alive\r\nConnection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf)).await;
    String::from_utf8_lossy(&buf).into_owned()
}

pub fn origin_addr(origin: &TestServer) -> SocketAddr {
    format!("127.0.0.1:{}", origin.url("/").port().unwrap())
        .parse()
        .unwrap()
}

pub const PICK: &str = "Pick = select, A, B, DIRECT\nAuto = url-test, A, B, hidden=true";

pub async fn two_entries(origin: &TestServer) -> (FakeSocks5, FakeSocks5, String) {
    let script = || Socks5Script {
        connect_to: Some(origin_addr(origin)),
        ..Socks5Script::default()
    };
    let (a, b) = (
        FakeSocks5::spawn(script()).await,
        FakeSocks5::spawn(script()).await,
    );
    let proxies = format!(
        "A = socks5, 127.0.0.1, {}, alice, s3cret\nB = socks5, 127.0.0.1, {}, username=bob, password=hunter2",
        a.addr().port(),
        b.addr().port()
    );
    (a, b, proxies)
}

/// A loopback Trojan server relaying to `to`, and the policy parameters that
/// make rurge trust it: the harness cannot inject a test CA (the runtime
/// builds its factory with the system roots), so the leaf is pinned.
pub async fn trojan_upstream(ws: bool, to: SocketAddr) -> (FakeTrojan, String) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let pin: String = fixture
        .leaf_fingerprint()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let fake = FakeTrojan::spawn(
        TrojanScript {
            password: "s3same".into(),
            ws,
            connect_to: Some(to),
        },
        fixture,
    )
    .await;
    (
        fake,
        format!("password=s3same, server-cert-fingerprint-sha256={pin}"),
    )
}

pub const VMESS_ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";

pub fn pin_of(fixture: &TlsFixture) -> String {
    fixture
        .leaf_fingerprint()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A fake VMess server relaying to `to`, and the parameters that reach it.
/// The harness trusts the OS roots, so the fixture's leaf is pinned.
pub async fn vmess_upstream(tls: bool, ws: bool, to: SocketAddr) -> (FakeVmess, String) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let mut params = format!("username={VMESS_ID}, vmess-aead=true");
    if tls {
        params += &format!(
            ", tls=true, server-cert-fingerprint-sha256={}",
            pin_of(&fixture)
        );
    }
    if ws {
        params += ", ws=true, ws-path=/v";
    }
    let script = VmessScript {
        ws,
        connect_to: Some(to),
        ..VmessScript::new(VMESS_ID)
    };
    (
        FakeVmess::spawn(script, tls.then_some(fixture)).await,
        params,
    )
}

pub async fn anytls_upstream(to: SocketAddr) -> (FakeAnyTls, String) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let params = format!(
        "password=s3same, server-cert-fingerprint-sha256={}",
        pin_of(&fixture)
    );
    let script = AnyTlsScript {
        password: "s3same".into(),
        connect_to: Some(to),
        ..AnyTlsScript::default()
    };
    (FakeAnyTls::spawn(script, fixture).await, params)
}

/// Sends `payload` through `tunnel` and expects it back (the far end echoes).
pub async fn echo_through(tunnel: &mut TcpStream, payload: &[u8]) {
    tunnel.write_all(payload).await.unwrap();
    let mut back = vec![0u8; payload.len()];
    tokio::time::timeout(Duration::from_secs(5), tunnel.read_exact(&mut back))
        .await
        .expect("the echo comes back")
        .unwrap();
    assert_eq!(back, payload);
}

pub fn outbound_now(h: &Harness, name: &str) -> rurge_proto::OutboundRef {
    h.engine
        .registry()
        .resolve(&rurge_config::rule::PolicyRef::parse(name))
        .outbound
}
