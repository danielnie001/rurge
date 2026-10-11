//! What the test files of this directory share: the harness, the helpers and
//! the imports (re-exported, so a test file needs `use common::*` and nothing
//! else). Each test file is a crate of its own and uses a part of all this
//! only: hence the two `allow`s.

#![allow(dead_code, unused_imports)]

mod log_capture;

pub use log_capture::LogCapture;
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
    AnyTlsScript, FakeAnyTls, FakeH2Proxy, FakeHttpProxy, FakeShadowsocks, FakeSnell, FakeSocks5,
    FakeTrojan, FakeVmess, H2ProxyScript, HttpProxyScript, ShadowsocksScript, SnellScript,
    Socks5Script, TlsFixture, TrojanScript, VmessScript,
};
pub use rurge_proto_quic::testing::{
    FakeH3Proxy, FakeHysteria2, FakeTuic, H3ProxyScript, Hysteria2Script, TuicScript,
};
pub use rurge_proto_ssh::testing::{
    Algorithm, FakeSsh, FakeSshOpts, fingerprint_of, keystore_base64, random_key,
};
pub use rurge_proto_wireguard::testing::{ECHO_PORT, FakeWgPeer, PeerOpts, keypair, section_text};
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
    runtime_edited(dir, profile, shared, |_| {}).await
}

/// As `runtime`, with `edit` applied to the loaded configuration first: for
/// what the text of a profile cannot say (a hop interval under the loader's
/// minimum).
pub async fn runtime_edited(
    dir: &std::path::Path,
    profile: &str,
    shared: EngineShared,
    edit: impl FnOnce(&mut rurge_config::Config),
) -> Runtime {
    std::fs::write(dir.join("t.conf"), profile).unwrap();
    let mut loaded = from_text(profile, &dir.join("t.conf"), &LoadOptions::for_tests());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    edit(&mut loaded.config);
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

/// The test URL of the profiles here, so that no connectivity test goes to
/// the default `http://bing.com/`. Nothing is assumed of what is at the
/// port: a connection to it is refused at once (Linux, macOS), refused
/// after two seconds (Windows), or taken and never answered (a machine that
/// runs the discard service), so a member tested there fails early, late,
/// or only at the timeout. A dial through an automatic group asks for a
/// round of such tests, which then runs on its own: whatever is asserted
/// after the dial has to hold whenever the round ends. Where it would not —
/// a count of a member's failures — the members are kept out of the rounds
/// (`UNTESTED`) or given a test URL of their own.
pub const NO_TEST: &str = "http://127.0.0.1:9/";

/// A parameter that keeps the connectivity tests off a member: no round can
/// test at a URL that does not parse, so the member is known through
/// sessions only. For the tests that count the failures sessions told of: a
/// test at the harness's URL (`NO_TEST`) fails, and every member's failed
/// test would count beside them — at once, seconds later, or only after the
/// count was read, as the machine refuses a connection to that port quickly,
/// slowly, or takes it and never answers. For the tests that read the request
/// log or count connections at the members' fakes: the sessions of a test
/// round are records of the log as well, and connections at the fakes.
pub const UNTESTED: &str = "test-url=http://[";

/// The variable parts of a test profile; everything else is fixed.
#[derive(Default)]
pub struct Profile<'a> {
    pub general: &'a str,
    pub proxies: &'a str,
    pub groups: &'a str,
    pub hosts: &'a str,
    /// Inserted before `FINAL,DIRECT`.
    pub rules: &'a str,
    pub keystore: &'a str,
    /// Whole sections after `[Rule]`, such as `[WireGuard <name>]`.
    pub sections: &'a str,
}

impl Profile<'_> {
    /// Connectivity tests go to `NO_TEST` unless `general` says otherwise:
    /// never to the default `http://bing.com/`.
    pub fn text(&self, dns: SocketAddr) -> String {
        format!(
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {dns}\nipv6 = false\n\
proxy-test-url = {NO_TEST}\ninternet-test-url = {NO_TEST}\n{}\n\
[Proxy]\n{}\n[Proxy Group]\n{}\n[Host]\n{}\n[Keystore]\n{}\n[Rule]\n{}\nFINAL,DIRECT\n{}",
            self.general,
            self.proxies,
            self.groups,
            self.hosts,
            self.keystore,
            self.rules,
            self.sections
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
    harness_edited(p, shared, |_| {}).await
}

/// As `harness_with`, with `edit` applied to the loaded configuration
/// first (`runtime_edited`).
pub async fn harness_edited(
    p: Profile<'_>,
    shared: EngineShared,
    edit: impl FnOnce(&mut rurge_config::Config),
) -> Harness {
    let dns = MockDns::spawn().await;
    for name in ["target.test", "alt.test"] {
        dns.set(name, &["127.0.0.1"], &[], 60);
    }
    let dir = tempfile::tempdir().unwrap();
    let text = p.text(dns.addr());
    let engine = Engine::new(runtime_edited(dir.path(), &text, shared, edit).await);
    let listeners = engine.bind_listeners().await.unwrap();
    Harness {
        dir,
        engine,
        listeners,
        dns,
    }
}

/// Every policy over QUIC of `cfg` hops every `interval`: the loader takes
/// no `port-hopping-interval` under 5 seconds (`harness_edited`).
pub fn hop_every(cfg: &mut rurge_config::Config, interval: Duration) {
    use rurge_config::spec::ProtoSpec;
    for spec in &mut cfg.specs {
        let quic = match &mut spec.proto {
            ProtoSpec::Hysteria2(hysteria2) => &mut hysteria2.quic,
            ProtoSpec::Tuic(tuic) => &mut tuic.quic,
            ProtoSpec::Masque(masque) => &mut masque.quic,
            _ => continue,
        };
        if let Some(hopping) = &mut quic.port_hopping {
            hopping.interval = interval;
        }
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

/// `CONNECT host:port` through rurge's SOCKS5 listener, the host as a
/// name; returns the tunnel.
pub async fn connect_via_socks5(proxy: SocketAddr, host: &str, port: u16) -> TcpStream {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut method = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut method))
        .await
        .expect("the proxy chooses a method")
        .unwrap();
    assert_eq!(method, [5, 0]);
    let mut request = vec![5, 1, 0, 3, host.len() as u8];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    s.write_all(&request).await.unwrap();
    let mut reply = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut reply))
        .await
        .expect("the proxy answers")
        .unwrap();
    assert_eq!(reply[1], 0, "{reply:?}");
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

/// The first `n` sessions' records, oldest first, once they have finished.
pub async fn the_records(h: &Harness, n: usize) -> Vec<rurge_engine::RequestRecord> {
    let log = h.engine.request_log();
    wait_until("the sessions to finish", || log.recent(10).len() >= n).await;
    let mut records = log.recent(10);
    records.reverse();
    records
}

/// The finished UDP flows' records, newest first.
pub fn udp_records(h: &Harness) -> Vec<rurge_engine::RequestRecord> {
    h.engine
        .request_log()
        .recent(4096)
        .into_iter()
        .filter(|r| r.transport == rurge_config::session::Transport::Udp)
        .collect()
}

/// One session to the echo through `h`: a round trip, then the client
/// goes and the session finishes.
pub async fn one_session(h: &Harness, n: usize, payload: &[u8]) -> rurge_engine::RequestRecord {
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, payload).await;
    drop(tunnel);
    the_records(h, n).await[n - 1].clone()
}

/// Opens a session the proxy refuses: the client's CONNECT is answered
/// with 502 within the bound, and the record says why.
pub async fn a_failed_session(h: &Harness) -> rurge_engine::RequestRecord {
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(b"CONNECT target.test:7 HTTP/1.1\r\nHost: target.test:7\r\n\r\n")
        .await
        .unwrap();
    let mut answer = [0u8; 12];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut answer))
        .await
        .expect("the CONNECT is answered within the bound")
        .unwrap();
    assert_eq!(
        &answer,
        b"HTTP/1.1 502",
        "{}",
        String::from_utf8_lossy(&answer)
    );
    let record = the_records(h, 1).await[0].clone();
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    record
}

/// A SOCKS5 UDP association through rurge's listener (phase 2 M5): the
/// control connection, the client's UDP socket, and the association's port.
pub struct UdpAssociation {
    pub control: TcpStream,
    pub socket: tokio::net::UdpSocket,
    pub relay: SocketAddr,
}

pub async fn udp_associate(socks: SocketAddr) -> UdpAssociation {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut control = TcpStream::connect(socks).await.unwrap();
    control.write_all(&[5, 1, 0]).await.unwrap();
    let mut method = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(10), control.read_exact(&mut method))
        .await
        .expect("the proxy chooses a method")
        .unwrap();
    assert_eq!(method, [5, 0]);
    let SocketAddr::V4(me) = socket.local_addr().unwrap() else {
        panic!("loopback is v4")
    };
    let mut request = vec![5, 3, 0, 1];
    request.extend_from_slice(&me.ip().octets());
    request.extend_from_slice(&me.port().to_be_bytes());
    control.write_all(&request).await.unwrap();
    let mut reply = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(10), control.read_exact(&mut reply))
        .await
        .expect("the proxy answers the UDP ASSOCIATE")
        .unwrap();
    assert_eq!((reply[1], reply[3]), (0, 1), "{reply:?}");
    let relay = SocketAddr::from((
        [reply[4], reply[5], reply[6], reply[7]],
        u16::from_be_bytes([reply[8], reply[9]]),
    ));
    UdpAssociation {
        control,
        socket,
        relay,
    }
}

impl UdpAssociation {
    /// Sends `payload` to `host:port` (an IP literal or a name).
    pub async fn send(&self, host: &str, port: u16, payload: &[u8]) {
        let mut datagram = vec![0, 0, 0];
        match host.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(v4)) => {
                datagram.push(1);
                datagram.extend_from_slice(&v4.octets());
            }
            Ok(std::net::IpAddr::V6(v6)) => {
                datagram.push(4);
                datagram.extend_from_slice(&v6.octets());
            }
            Err(_) => {
                datagram.push(3);
                datagram.push(host.len() as u8);
                datagram.extend_from_slice(host.as_bytes());
            }
        }
        datagram.extend_from_slice(&port.to_be_bytes());
        datagram.extend_from_slice(payload);
        self.socket.send_to(&datagram, self.relay).await.unwrap();
    }

    /// The next datagram: where it says it came from (`ip:port`) and its payload.
    pub async fn recv(&self) -> (SocketAddr, Vec<u8>) {
        self.recv_within(Duration::from_secs(5))
            .await
            .expect("a datagram comes back")
    }

    /// As `recv`; `None` when no datagram comes within `window`.
    pub async fn recv_within(&self, window: Duration) -> Option<(SocketAddr, Vec<u8>)> {
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(window, self.socket.recv_from(&mut buf))
            .await
            .ok()?
            .unwrap();
        assert_eq!(from, self.relay);
        assert_eq!(&buf[..3], [0, 0, 0]);
        let (ip, rest): (std::net::IpAddr, usize) = match buf[3] {
            1 => (<[u8; 4]>::try_from(&buf[4..8]).unwrap().into(), 8),
            4 => (<[u8; 16]>::try_from(&buf[4..20]).unwrap().into(), 20),
            other => panic!("address type {other}"),
        };
        let port = u16::from_be_bytes([buf[rest], buf[rest + 1]]);
        Some((SocketAddr::new(ip, port), buf[rest + 2..n].to_vec()))
    }

    /// Whether nothing comes back within `window` (only to observe that
    /// nothing happens).
    pub async fn quiet_for(&self, window: Duration) -> bool {
        let mut buf = [0u8; 2048];
        tokio::time::timeout(window, self.socket.recv_from(&mut buf))
            .await
            .is_err()
    }
}

/// A loopback UDP server answering every datagram with itself; it notes
/// who wrote to it.
pub async fn udp_echo() -> (SocketAddr, Arc<std::sync::Mutex<Vec<SocketAddr>>>) {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    let seen: Arc<std::sync::Mutex<Vec<SocketAddr>>> = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        loop {
            let (n, from) = match socket.recv_from(&mut buf).await {
                Ok(got) => got,
                // Windows reports an ICMP "unreachable" on the next receive
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
                Err(_) => break,
            };
            log.lock().unwrap().push(from);
            let _ = socket.send_to(&buf[..n], from).await;
        }
    });
    (addr, seen)
}

/// A loopback port that `binds` could bind just now, and that no socket
/// another test opens can come by. A port the system handed out and the
/// test then gave back can be handed out again to anyone, a moment later;
/// this one lies in a range the system does not hand out by itself — the
/// dynamic ports of Windows (by default), Linux and macOS are all above it —
/// and is tried just before it is returned. One walk for every caller: no
/// two of them are given the same port.
fn unassigned_port(binds: impl Fn(u16) -> bool) -> u16 {
    const PORTS: std::ops::Range<u64> = 20_000..30_000;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // this process starts somewhere else in the range than the one before it
    let start = u64::from(std::process::id()).wrapping_mul(7919);
    for _ in 0..100 {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let port = PORTS.start + start.wrapping_add(n) % (PORTS.end - PORTS.start);
        let port = u16::try_from(port).expect("a port");
        if binds(port) {
            return port;
        }
    }
    panic!("no port of {PORTS:?} is free");
}

/// A loopback UDP port with nothing on it: a datagram to it is answered
/// with an ICMP "port unreachable".
pub fn closed_udp_port() -> u16 {
    unassigned_port(|port| std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok())
}

/// A loopback TCP port with nothing on it: a connection to it is refused —
/// at once on Linux and macOS, after about two seconds on Windows (measured
/// on the development machine: 2.03 to 2.05 s), which whoever connects to it
/// allows for. The socket that tried the port is closed before the number is
/// handed out, so that nothing is left on it: a socket left bound and not
/// listening refuses on Linux and Windows, and leaves the connection waiting
/// on macOS.
pub fn closed_tcp_port() -> u16 {
    unassigned_port(|port| std::net::TcpListener::bind(("127.0.0.1", port)).is_ok())
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
    anytls_upstream_run_by(AnyTlsScript::default(), to).await
}

/// As `anytls_upstream`, the server doing what else `script` says.
pub async fn anytls_upstream_run_by(script: AnyTlsScript, to: SocketAddr) -> (FakeAnyTls, String) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let params = format!(
        "password=s3same, server-cert-fingerprint-sha256={}",
        pin_of(&fixture)
    );
    let script = AnyTlsScript {
        password: "s3same".into(),
        connect_to: Some(to),
        ..script
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
