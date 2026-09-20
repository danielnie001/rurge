//! Sessions that leave through real proxy outbounds: profile text → Runtime →
//! Engine → loopback listeners → scripted loopback upstreams
//! (`rurge_proto::testing`) → `TestServer` (M1 design §8).

use rurge_config::config::{LoadOptions, from_text};
use rurge_config::session::ListenerKind;
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_engine::SelectError;
use rurge_engine::stack::StackOptions;
use rurge_engine::state::{STATE_FILE, StateStore, profile_key};
use rurge_engine::{Engine, EngineShared, ListenerSpec, RecordStatus, Runtime, RuntimeOptions};
use rurge_inbound::Running;
use rurge_net::socket::NoopSocketHook;
use rurge_net::testing::TestServer;
use rurge_proto::testing::{
    AnyTlsScript, FakeAnyTls, FakeHttpProxy, FakeSocks5, FakeTrojan, FakeVmess, HttpProxyScript,
    Socks5Script, TlsFixture, TrojanScript, VmessScript,
};
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

/// One request/response over a fresh connection to rurge's HTTP listener.
async fn plain_get(proxy: SocketAddr, url: &str, host: &str) -> String {
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

fn origin_addr(origin: &TestServer) -> SocketAddr {
    format!("127.0.0.1:{}", origin.url("/").port().unwrap())
        .parse()
        .unwrap()
}

#[tokio::test]
async fn a_plain_request_goes_to_an_http_upstream_in_absolute_form() {
    let upstream = FakeHttpProxy::spawn(HttpProxyScript {
        auth: Some(("alice".into(), "s3cret".into())),
        ..HttpProxyScript::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Up = http, 127.0.0.1, {}, alice, s3cret, headers=X-Client:rurge",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,Up",
        ..Profile::default()
    })
    .await;
    let response = plain_get(
        h.http(),
        "http://u:p@target.test:8080/hello?x=1",
        "lying.internal",
    )
    .await;
    // the fake answers absolute-form requests itself
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("forwarded"), "{response}");
    let heads = upstream.heads();
    assert_eq!(heads.len(), 1, "one connection, one request, no CONNECT");
    assert_eq!(
        heads[0].request_line,
        "GET http://target.test:8080/hello?x=1 HTTP/1.1"
    );
    assert_eq!(heads[0].header("Host"), Some("target.test:8080"));
    // base64("alice:s3cret")
    assert_eq!(
        heads[0].header("Proxy-Authorization"),
        Some("Basic YWxpY2U6czNjcmV0")
    );
    assert_eq!(heads[0].header("X-Client"), Some("rurge"));
    assert_eq!(heads[0].header("Proxy-Connection"), None);
}

#[tokio::test]
async fn always_use_connect_and_other_protocols_tunnel_plain_requests() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let http_up = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(origin_addr(&origin)),
        ..HttpProxyScript::default()
    })
    .await;
    let socks_up = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Tunnel = http, 127.0.0.1, {}, always-use-connect=true\nSocks = socks5, 127.0.0.1, {}",
            http_up.addr().port(),
            socks_up.addr().port()
        ),
        rules: "DOMAIN,target.test,Tunnel\nDOMAIN,alt.test,Socks",
        ..Profile::default()
    })
    .await;
    let response = plain_get(
        h.http(),
        "http://target.test:8080/hello",
        "target.test:8080",
    )
    .await;
    assert!(response.ends_with("hi there"), "{response}");
    assert_eq!(
        http_up.heads()[0].request_line,
        "CONNECT target.test:8080 HTTP/1.1"
    );
    let response = plain_get(h.http(), "http://alt.test:8080/hello", "alt.test:8080").await;
    assert!(response.ends_with("hi there"), "{response}");
    let seen = &socks_up.requests()[0];
    assert_eq!(
        (seen.atyp, seen.host.as_str(), seen.port),
        (3, "alt.test", 8080)
    );
    // both requests reached the origin; that they were tunnelled rather than
    // forwarded is pinned by the CONNECT request line and the SOCKS5 ATYP
    // assertions above, not by this count
    assert_eq!(origin.hits("/hello"), 2);
}

#[tokio::test]
async fn a_refusing_upstream_is_a_502_that_quotes_the_proxy() {
    let upstream = FakeHttpProxy::spawn(HttpProxyScript {
        auth: Some(("alice".into(), "right".into())),
        ..HttpProxyScript::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Up = http, 127.0.0.1, {}, alice, wrong",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,Up",
        ..Profile::default()
    })
    .await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(
        b"CONNECT target.test:443 HTTP/1.1\r\nHost: target.test:443\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    // rurge closes the connection after the 502 (the request asked for
    // `Connection: close`), so this is a real bound, not a fixed sleep: a
    // regression back to keep-alive here must fail the test, not just run slow.
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf))
        .await
        .expect("rurge closes the connection after the 502");
    let response = String::from_utf8_lossy(&buf).into_owned();
    assert!(response.starts_with("HTTP/1.1 502"), "{response}");
    let log = h.engine.request_log();
    wait_until("the failed session", || !log.recent(10).is_empty()).await;
    let error = log.recent(10)[0].error.clone().unwrap_or_default();
    assert_eq!(
        error,
        "http proxy answered 407 Proxy Authentication Required"
    );
    assert!(
        !response.contains("wrong") && !error.contains("wrong"),
        "no credential leaks"
    );
}

/// Forward mode: a 407 from the upstream is rurge's own credential problem.
/// The client can do nothing about it and the challenge header is hop-by-hop,
/// so the session fails and the client gets the ordinary 502 page instead of
/// an invalid 407.
#[tokio::test]
async fn a_forward_mode_407_is_a_failed_session_and_a_502_page() {
    let upstream = FakeHttpProxy::spawn(HttpProxyScript {
        auth: Some(("alice".into(), "right".into())),
        ..HttpProxyScript::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Up = http, 127.0.0.1, {}, alice, wrong",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,Up",
        ..Profile::default()
    })
    .await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(
        b"GET http://target.test:8080/hello HTTP/1.1\r\nHost: target.test:8080\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    // `Connection: close` makes EOF the real bound: a regression back to
    // relaying the upstream's keep-alive 407 fails here instead of running slow.
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf))
        .await
        .expect("rurge closes the connection after the 502");
    let response = String::from_utf8_lossy(&buf).into_owned();
    assert!(response.starts_with("HTTP/1.1 502"), "{response}");
    let log = h.engine.request_log();
    wait_until("the failed session", || !log.recent(10).is_empty()).await;
    let record = log.recent(10).remove(0);
    let error = record.error.clone().unwrap_or_default();
    assert_eq!(
        error,
        "http proxy answered 407 Proxy Authentication Required"
    );
    assert_eq!(record.status, RecordStatus::Failed);
    assert!(
        !response.contains("wrong") && !error.contains("wrong"),
        "no credential leaks"
    );
    let heads = upstream.heads();
    assert_eq!(heads.len(), 1, "one absolute-form request, no retry");
    assert_eq!(
        heads[0].request_line,
        "GET http://target.test:8080/hello HTTP/1.1"
    );
}

#[tokio::test]
async fn a_local_host_item_reaches_the_proxy_as_an_ip() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    for (flag, expected) in [("true", (1u8, "10.1.2.3")), ("false", (3u8, "pinned.test"))] {
        let upstream = FakeSocks5::spawn(Socks5Script {
            connect_to: Some(origin_addr(&origin)),
            ..Socks5Script::default()
        })
        .await;
        let h = harness(Profile {
            general: &format!("use-local-host-item-for-proxy = {flag}"),
            proxies: &format!("Up = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "pinned.test = 10.1.2.3\nalias.test = pinned.test",
            rules: "DOMAIN-SUFFIX,test,Up",
            ..Profile::default()
        })
        .await;
        let mut tunnel = connect_via_http(h.http(), "pinned.test:80").await;
        assert!(
            get(&mut tunnel, "pinned.test", "/hello")
                .await
                .ends_with("hi there")
        );
        let seen = &upstream.requests()[0];
        assert_eq!((seen.atyp, seen.host.as_str()), expected, "flag = {flag}");
        // an alias item never changes what the proxy is asked for
        let _ = connect_via_http(h.http(), "alias.test:80").await;
        wait_until("the second request", || upstream.requests().len() == 2).await;
        assert_eq!(upstream.requests()[1].host, "alias.test", "flag = {flag}");
    }
}

/// With an IP substituted for the name, a plain request is tunnelled: the
/// request inside keeps its `Host`, the proxy connects where `[Host]` says.
#[tokio::test]
async fn a_pinned_host_turns_forwarding_into_a_tunnel() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let upstream = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(origin_addr(&origin)),
        ..HttpProxyScript::default()
    })
    .await;
    let h = harness(Profile {
        general: "use-local-host-item-for-proxy = true",
        proxies: &format!("Up = http, 127.0.0.1, {}", upstream.addr().port()),
        hosts: "pinned.test = 10.1.2.3",
        rules: "DOMAIN,pinned.test,Up",
        ..Profile::default()
    })
    .await;
    let response = plain_get(h.http(), "http://pinned.test/hello", "pinned.test").await;
    assert!(response.ends_with("hi there"), "{response}");
    assert_eq!(
        upstream.heads()[0].request_line,
        "CONNECT 10.1.2.3:80 HTTP/1.1"
    );
    assert_eq!(origin.requests()[0].header("host"), Some("pinned.test"));
}

/// M1 design §9.1: a two-level chain whose lower level is a `select` group.
#[tokio::test]
async fn a_chain_enters_through_the_groups_current_member() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let exit = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(origin_addr(&origin)),
        ..HttpProxyScript::default()
    })
    .await;
    let entry = |to: SocketAddr| Socks5Script {
        connect_to: Some(to),
        ..Socks5Script::default()
    };
    let entry_a = FakeSocks5::spawn(entry(exit.addr())).await;
    let entry_b = FakeSocks5::spawn(entry(exit.addr())).await;
    let h = harness(Profile {
        proxies: &format!(
            "EntryA = socks5, 127.0.0.1, {}\nEntryB = socks5, 127.0.0.1, {}\nExit = http, exit.example, 8080, underlying-proxy=Hop",
            entry_a.addr().port(),
            entry_b.addr().port()
        ),
        groups: "Hop = select, EntryA, EntryB",
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    assert!(
        get(&mut tunnel, "target.test", "/hello")
            .await
            .ends_with("hi there")
    );
    // the entry is asked for the exit's server by name; the exit for the target by name
    let first = &entry_a.requests()[0];
    assert_eq!(
        (first.atyp, first.host.as_str(), first.port),
        (3, "exit.example", 8080)
    );
    assert_eq!(
        exit.heads()[0].request_line,
        "CONNECT target.test:8080 HTTP/1.1"
    );
    assert!(entry_b.requests().is_empty());

    h.engine.shared().selections.set("Hop", "EntryB");
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    assert!(
        get(&mut tunnel, "target.test", "/hello")
            .await
            .ends_with("hi there")
    );
    assert_eq!(
        entry_b.requests().len(),
        1,
        "the next connection follows the selection"
    );
    assert_eq!(entry_a.requests().len(), 1);
}

#[tokio::test]
async fn a_broken_hop_is_named_in_the_error() {
    // a port nothing listens on: bind one, note it, let it go
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let h = harness(Profile {
        proxies: &format!(
            "Entry = socks5, 127.0.0.1, {closed}\nExit = http, exit.example, 8080, underlying-proxy=Entry"
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(
        b"CONNECT target.test:443 HTTP/1.1\r\nHost: target.test:443\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    // rurge closes the connection after the 502 (the request asked for
    // `Connection: close`), so this is a real bound, not a fixed sleep: a
    // regression back to keep-alive here must fail the test, not just run slow.
    let _ = tokio::time::timeout(Duration::from_secs(15), s.read_to_end(&mut buf))
        .await
        .expect("rurge closes the connection after the 502");
    let response = String::from_utf8_lossy(&buf).into_owned();
    assert!(response.starts_with("HTTP/1.1 502"), "{response}");
    let log = h.engine.request_log();
    wait_until("the failed session", || !log.recent(10).is_empty()).await;
    let error = log.recent(10)[0].error.clone().unwrap_or_default();
    assert!(error.starts_with("via Entry: "), "{error}");
}

/// Two independent implementations check each other: engine A's upstreams
/// are engine B's HTTP and SOCKS5 listeners.
#[tokio::test]
async fn rurge_talks_to_rurge() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let port = origin.url("/").port().unwrap();
    let b = harness(Profile::default()).await; // everything DIRECT, resolves *.test itself
    let a = harness(Profile {
        proxies: &format!(
            "ViaHttp = http, 127.0.0.1, {}\nViaSocks = socks5, 127.0.0.1, {}",
            b.http().port(),
            b.socks().port()
        ),
        rules: "DOMAIN,target.test,ViaHttp\nDOMAIN,alt.test,ViaSocks",
        ..Profile::default()
    })
    .await;
    // CONNECT through A → CONNECT to B's HTTP listener → origin
    let mut tunnel = connect_via_http(a.http(), &format!("target.test:{port}")).await;
    assert!(
        get(&mut tunnel, "target.test", "/hello")
            .await
            .ends_with("hi there")
    );
    // the relay only finishes (and gets logged) once both directions close
    drop(tunnel);
    // CONNECT through A → B's SOCKS5 listener → origin
    let mut tunnel = connect_via_http(a.http(), &format!("alt.test:{port}")).await;
    assert!(
        get(&mut tunnel, "alt.test", "/hello")
            .await
            .ends_with("hi there")
    );
    drop(tunnel);
    // a plain request: absolute form from A to B, origin form from B to the origin
    let response = plain_get(
        a.http(),
        &format!("http://target.test:{port}/hello"),
        &format!("target.test:{port}"),
    )
    .await;
    assert!(response.ends_with("hi there"), "{response}");
    assert_eq!(origin.hits("/hello"), 3);
    let log = b.engine.request_log();
    wait_until("B to record three sessions", || log.recent(10).len() == 3).await;
    let via: Vec<ListenerKind> = log.recent(10).iter().map(|r| r.listener).collect();
    assert_eq!(via.iter().filter(|k| **k == ListenerKind::Http).count(), 2);
    assert_eq!(
        via.iter().filter(|k| **k == ListenerKind::Socks5).count(),
        1
    );
    // only B resolved anything: A handed the names over
    assert!(a.dns.queries().is_empty());
    assert!(!b.dns.queries().is_empty());
}

const PICK: &str = "Pick = select, A, B, DIRECT\nAuto = url-test, A, B, hidden=true";

async fn two_entries(origin: &TestServer) -> (FakeSocks5, FakeSocks5, String) {
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

#[tokio::test]
async fn a_selection_applies_to_the_next_connection_and_survives_a_restart() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (a, b, proxies) = two_entries(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: PICK,
        rules: "DOMAIN,target.test,Pick",
        ..Profile::default()
    })
    .await;
    let state_path = h.dir.path().join(STATE_FILE);
    let (store, _) = StateStore::open(state_path.clone()).await;
    h.engine.attach_state(store);

    assert_eq!(
        h.engine.group_selection("Pick").unwrap(),
        "A",
        "the first member by default"
    );
    let mut t = connect_via_http(h.http(), "target.test:80").await;
    assert!(
        get(&mut t, "target.test", "/hello")
            .await
            .ends_with("hi there")
    );
    assert_eq!((a.requests().len(), b.requests().len()), (1, 0));

    h.engine.select_group("Pick", "B").await.unwrap();
    assert_eq!(h.engine.group_selection("Pick").unwrap(), "B");
    let mut t = connect_via_http(h.http(), "target.test:80").await;
    assert!(
        get(&mut t, "target.test", "/hello")
            .await
            .ends_with("hi there")
    );
    assert_eq!((a.requests().len(), b.requests().len()), (1, 1));

    // written under the profile's file name
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(saved["group_selections"]["t.conf"]["Pick"], "B");

    // "restart": a fresh engine seeded from state.json, as `rurge run` does
    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    let (_, state) = StateStore::open(state_path).await;
    let key = profile_key(&h.dir.path().join("t.conf"));
    let shared = EngineShared::new(state.selections_for(&key));
    let restarted = Engine::new(runtime(h.dir.path(), &text, shared).await);
    assert_eq!(restarted.group_selection("Pick").unwrap(), "B");
}

#[tokio::test]
async fn only_a_member_of_a_select_group_can_be_selected() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = two_entries(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: PICK,
        ..Profile::default()
    })
    .await;
    assert_eq!(
        h.engine.select_group("Nope", "A").await,
        Err(SelectError::UnknownGroup("Nope".into()))
    );
    assert_eq!(
        h.engine.select_group("A", "B").await,
        Err(SelectError::UnknownGroup("A".into())),
        "a policy is not a group"
    );
    assert_eq!(
        h.engine.select_group("Auto", "A").await,
        Err(SelectError::NotSelectable("Auto".into()))
    );
    assert_eq!(
        h.engine.select_group("Pick", "C").await,
        Err(SelectError::NotAMember {
            group: "Pick".into(),
            member: "C".into()
        })
    );
    assert_eq!(
        h.engine.group_selection("Pick").unwrap(),
        "A",
        "nothing changed"
    );
    assert_eq!(
        h.engine.group_selection("Nope"),
        Err(SelectError::UnknownGroup("Nope".into()))
    );
    assert_eq!(
        h.engine.group_selection("Auto").unwrap(),
        "A",
        "readable for every group kind"
    );
    // the messages are what the API will show
    assert_eq!(
        SelectError::UnknownGroup("G".into()).to_string(),
        "unknown policy group `G`"
    );
    assert_eq!(
        SelectError::NotSelectable("G".into()).to_string(),
        "`G` is not a select group"
    );
    assert_eq!(
        SelectError::NotAMember {
            group: "G".into(),
            member: "M".into()
        }
        .to_string(),
        "`M` is not a member of `G`"
    );
    // no StateStore attached: the selection still applies, it just isn't persisted
    assert_eq!(h.engine.select_group("Pick", "B").await, Ok(()));
    assert_eq!(h.engine.group_selection("Pick").unwrap(), "B");
}

#[tokio::test]
async fn views_describe_groups_and_redact_policy_details() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = two_entries(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: &format!(
            "{PICK}\nHiddenByNumber = select, DIRECT, hidden=1\nHiddenByWord = select, DIRECT, hidden=yes\nNotHidden = select, DIRECT, hidden=false"
        ),
        ..Profile::default()
    })
    .await;
    let groups = h.engine.groups_view();
    assert_eq!(
        groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
        [
            "Pick",
            "Auto",
            "HiddenByNumber",
            "HiddenByWord",
            "NotHidden"
        ]
    );
    let pick = &groups[0];
    assert_eq!(
        (pick.kind.keyword(), pick.hidden, pick.selected.as_deref()),
        ("select", false, Some("A"))
    );
    assert!(groups[1].hidden);
    // the project's boolean convention (`ParamMap::bool`): "1" / "yes" count
    // as true, case-insensitively — not just the literal string "true"
    assert!(groups[2].hidden, "hidden=1");
    assert!(groups[3].hidden, "hidden=yes");
    assert!(!groups[4].hidden, "hidden=false");
    let described: Vec<(&str, bool, &str)> = pick
        .members
        .iter()
        .map(|m| (m.name.as_str(), m.is_group, m.type_description.as_str()))
        .collect();
    assert_eq!(
        described,
        [
            ("A", false, "socks5"),
            ("B", false, "socks5"),
            ("DIRECT", false, "DIRECT")
        ]
    );
    for m in &pick.members {
        assert_eq!(m.line_hash.len(), 16, "{}", m.name);
        assert!(m.line_hash.bytes().all(|b| b.is_ascii_hexdigit()));
    }
    assert_ne!(pick.members[0].line_hash, pick.members[1].line_hash);

    let detail = h.engine.policy_detail("A").expect("a configured policy");
    assert!(detail.starts_with("socks5, 127.0.0.1, "), "{detail}");
    assert!(
        !detail.contains("s3cret") && !detail.contains("alice"),
        "{detail}"
    );
    // Builtin::parse only accepts the exact upper-case name (verified against
    // rurge-config/src/policy.rs); the brief's task-8-brief.md pre-authorizes
    // using "DIRECT" here instead of "direct" for that reason.
    assert_eq!(h.engine.policy_detail("DIRECT").as_deref(), Some("DIRECT"));
    assert_eq!(
        h.engine.policy_detail("Pick").as_deref(),
        Some("select, A, B, DIRECT")
    );
    assert_eq!(h.engine.policy_detail("Nope"), None);

    // named-parameter credentials (as opposed to A's positional ones) are
    // redacted too
    let detail_b = h.engine.policy_detail("B").expect("a configured policy");
    assert!(
        !detail_b.contains("bob") && !detail_b.contains("hunter2"),
        "{detail_b}"
    );
    assert!(detail_b.contains("***"), "{detail_b}");
}

/// A loopback Trojan server relaying to `to`, and the policy parameters that
/// make rurge trust it: the harness cannot inject a test CA (the runtime
/// builds its factory with the system roots), so the leaf is pinned.
async fn trojan_upstream(ws: bool, to: SocketAddr) -> (FakeTrojan, String) {
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

const VMESS_ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";

fn pin_of(fixture: &TlsFixture) -> String {
    fixture
        .leaf_fingerprint()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A fake VMess server relaying to `to`, and the parameters that reach it.
/// The harness trusts the OS roots, so the fixture's leaf is pinned.
async fn vmess_upstream(tls: bool, ws: bool, to: SocketAddr) -> (FakeVmess, String) {
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

async fn anytls_upstream(to: SocketAddr) -> (FakeAnyTls, String) {
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

#[tokio::test]
async fn a_connect_leaves_through_a_vmess_upstream_with_the_name_unresolved() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    for (tls, ws) in [(false, false), (true, true)] {
        let (upstream, params) = vmess_upstream(tls, ws, origin_addr(&origin)).await;
        let h = harness(Profile {
            proxies: &format!("V = vmess, 127.0.0.1, {}, {params}", upstream.addr().port()),
            rules: "DOMAIN,target.test,V",
            ..Profile::default()
        })
        .await;
        let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
        let response = get(&mut tunnel, "target.test", "/hello").await;
        assert!(
            response.ends_with("hi there"),
            "tls={tls} ws={ws}: {response}"
        );
        let seen = upstream.requests();
        let first = seen.first().expect("the upstream never saw a request");
        assert_eq!(
            (first.command, first.atyp, first.host.as_str(), first.port),
            (1, 2, "target.test", 8080),
            "the server resolves the name: rurge never looked it up"
        );
        assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
        drop(tunnel);
        let log = h.engine.request_log();
        wait_until("the session to finish", || !log.recent(10).is_empty()).await;
        let record = &log.recent(10)[0];
        assert_eq!(record.policy, ["V"]);
        assert!(record.error.is_none(), "{:?}", record.error);
    }
}

#[tokio::test]
async fn a_wrong_vmess_id_ends_the_session_with_a_text_that_says_so() {
    let origin = TestServer::spawn().await;
    let (upstream, _) = vmess_upstream(false, false, origin_addr(&origin)).await;
    let h = harness(Profile {
        proxies: &format!(
            "V = vmess, 127.0.0.1, {}, username=0233d11c-15a4-47d3-ade3-48ffca0ce118, vmess-aead=true",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,V",
        ..Profile::default()
    })
    .await;
    // the tunnel comes up: the server only answers a request it accepts
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    tunnel
        .write_all(b"GET / HTTP/1.1\r\nHost: target.test\r\n\r\n")
        .await
        .unwrap();
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), tunnel.read_to_end(&mut rest))
        .await
        .expect("the tunnel closes within the bound");
    assert_eq!(upstream.rejected(), 1);
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    let error = log.recent(10)[0].error.clone().unwrap_or_default();
    assert_eq!(
        error,
        "vmess: the server closed the connection without answering"
    );
    assert!(!error.contains("0233"));
}

#[tokio::test]
async fn an_anytls_upstream_carries_two_requests_over_one_session() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (upstream, params) = anytls_upstream(origin_addr(&origin)).await;
    let h = harness(Profile {
        proxies: &format!(
            "A = anytls, 127.0.0.1, {}, {params}",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,A",
        ..Profile::default()
    })
    .await;
    let log = h.engine.request_log();
    for round in 1..=2 {
        let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
        let response = get(&mut tunnel, "target.test", "/hello").await;
        assert!(response.ends_with("hi there"), "{response}");
        drop(tunnel);
        // The session's record appears once the relay has let go of the
        // stream, and by then the AnyTLS session is back in the pool. (Not
        // the server's FIN count: `get` asks the origin to close, so it is
        // the server that ends these streams, and a FIN is not answered.)
        wait_until("the session to finish", || log.recent(10).len() == round).await;
    }
    assert_eq!(
        upstream.sessions(),
        1,
        "the second request reused the session"
    );
    assert_eq!(
        upstream.fins(),
        0,
        "both streams were ended by the server (the origin was asked to close, so the \
         server closes first); a peer's cmdFIN is not answered"
    );
    let streams = upstream.streams();
    assert_eq!(
        streams
            .iter()
            .map(|s| (s.sid, s.host.as_str(), s.port))
            .collect::<Vec<_>>(),
        [(1, "target.test", 8080), (2, "target.test", 8080)]
    );
    assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
}

#[tokio::test]
async fn the_new_protocols_work_at_either_end_of_a_chain() {
    let echo = rurge_proto::testing::echo_server().await;
    // entry: vmess; exit: anytls, reached by name through the entry
    let (exit, exit_params) = anytls_upstream(echo).await;
    let (entry, entry_params) = vmess_upstream(false, false, exit.addr()).await;
    let h = harness(Profile {
        proxies: &format!(
            "Entry = vmess, 127.0.0.1, {}, {entry_params}\nExit = anytls, exit.example, 443, {exit_params}, underlying-proxy=Entry",
            entry.addr().port()
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"vmess then anytls").await;
    let asked = &entry.requests()[0];
    assert_eq!(
        (asked.host.as_str(), asked.port),
        ("exit.example", 443),
        "the entry is asked for the exit by name"
    );
    assert_eq!(exit.streams()[0].host, "target.test");

    // and the other way round
    let (exit, exit_params) = vmess_upstream(false, false, echo).await;
    let (entry, entry_params) = anytls_upstream(exit.addr()).await;
    let h = harness(Profile {
        proxies: &format!(
            "Entry = anytls, 127.0.0.1, {}, {entry_params}\nExit = vmess, exit.example, 443, {exit_params}, underlying-proxy=Entry",
            entry.addr().port()
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"anytls then vmess").await;
    assert_eq!(entry.streams()[0].host, "exit.example");
    assert_eq!(exit.requests()[0].host, "target.test");
}

#[tokio::test]
async fn a_connect_leaves_through_a_trojan_upstream_with_the_name_unresolved() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (upstream, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let h = harness(Profile {
        proxies: &format!(
            "T = trojan, 127.0.0.1, {}, {params}",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,T",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let response = get(&mut tunnel, "target.test", "/hello").await;
    assert!(response.ends_with("hi there"), "{response}");
    // whether the head rode with the first payload depends on timing here
    // (`HEAD_GRACE`); `LazyHead`'s own tests pin that deterministically
    let seen = upstream.requests();
    let first = seen.first().expect("the upstream never saw a request");
    assert_eq!(
        (first.command, first.atyp, first.host.as_str(), first.port),
        (1, 3, "target.test", 8080),
        "the server resolves the name: rurge never looked it up"
    );
    assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
    drop(tunnel);
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    let record = &log.recent(10)[0];
    assert_eq!(record.policy, ["T"]);
    assert!(record.error.is_none(), "{:?}", record.error);
}

#[tokio::test]
async fn a_plain_request_is_tunnelled_through_trojan_over_websocket() {
    // only an HTTP proxy takes a plain request in absolute form; everything
    // else gets a tunnel
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (upstream, params) = trojan_upstream(true, origin_addr(&origin)).await;
    let h = harness(Profile {
        proxies: &format!(
            "T = trojan, 127.0.0.1, {}, {params}, ws=true, ws-path=/tunnel, ws-headers=Host:edge.test",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,T",
        ..Profile::default()
    })
    .await;
    let response = plain_get(
        h.http(),
        "http://target.test:8080/hello",
        "target.test:8080",
    )
    .await;
    assert!(response.ends_with("hi there"), "{response}");
    let ws = upstream.ws_seen();
    let first = ws
        .first()
        .expect("the upstream never saw a websocket handshake");
    assert_eq!(
        (first.path.as_str(), first.header("host")),
        ("/tunnel", Some("edge.test"))
    );
    assert_eq!(upstream.requests()[0].host, "target.test");
    assert_eq!(origin.hits("/hello"), 1);
}

#[tokio::test]
async fn a_trojan_exit_is_reached_through_a_socks5_entry_by_name() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (exit, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let entry = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(exit.addr()),
        ..Socks5Script::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Entry = socks5, 127.0.0.1, {}\nExit = trojan, exit.example, 443, {params}, underlying-proxy=Entry",
            entry.addr().port()
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    assert!(
        get(&mut tunnel, "target.test", "/hello")
            .await
            .ends_with("hi there")
    );
    // the entry is asked for the exit's server by name; the exit for the target by name
    let entry_seen = entry.requests();
    let first = entry_seen.first().expect("the entry never saw a request");
    assert_eq!(
        (first.atyp, first.host.as_str(), first.port),
        (3, "exit.example", 443)
    );
    let exit_seen = exit.requests();
    let exit_first = exit_seen.first().expect("the exit never saw a request");
    assert_eq!(exit_first.host, "target.test");
    assert!(
        h.dns.queries().is_empty(),
        "nothing on this path is resolved locally"
    );
}

/// The mirror image: trojan is the ENTRY, and the hop above it runs its own
/// handshake on top of the entry's `LazyHead` (M2a acceptance item 4).
#[tokio::test]
async fn a_socks5_exit_is_reached_through_a_trojan_entry() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let exit = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let (entry, params) = trojan_upstream(false, exit.addr()).await;
    let h = harness(Profile {
        proxies: &format!(
            "Entry = trojan, 127.0.0.1, {}, {params}\nExit = socks5, exit.example, 1080, underlying-proxy=Entry",
            entry.addr().port()
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    assert!(
        get(&mut tunnel, "target.test", "/hello")
            .await
            .ends_with("hi there")
    );
    // the entry is asked for the exit's server by name; the exit for the target by name
    let entry_seen = entry.requests();
    let first = entry_seen.first().expect("the entry never saw a request");
    assert_eq!(
        (first.atyp, first.host.as_str(), first.port),
        (3, "exit.example", 1080)
    );
    let exit_seen = exit.requests();
    let exit_first = exit_seen.first().expect("the exit never saw a request");
    assert_eq!(
        (exit_first.atyp, exit_first.host.as_str(), exit_first.port),
        (3, "target.test", 8080)
    );
    assert!(
        h.dns.queries().is_empty(),
        "nothing on this path is resolved locally"
    );
}

/// Sends `payload` through `tunnel` and expects it back (the far end echoes).
async fn echo_through(tunnel: &mut TcpStream, payload: &[u8]) {
    tunnel.write_all(payload).await.unwrap();
    let mut back = vec![0u8; payload.len()];
    tokio::time::timeout(Duration::from_secs(5), tunnel.read_exact(&mut back))
        .await
        .expect("the echo comes back")
        .unwrap();
    assert_eq!(back, payload);
}

#[tokio::test]
async fn a_reload_leaves_a_chained_session_alone_and_moves_the_next_one() {
    let echo = rurge_proto::testing::echo_server().await;
    let exit = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(echo),
        ..HttpProxyScript::default()
    })
    .await;
    let entry = |to: SocketAddr| Socks5Script {
        connect_to: Some(to),
        ..Socks5Script::default()
    };
    let entry_a = FakeSocks5::spawn(entry(exit.addr())).await;
    let entry_b = FakeSocks5::spawn(entry(exit.addr())).await;
    let proxies = |under: &str| {
        format!(
            "EntryA = socks5, 127.0.0.1, {}\nEntryB = socks5, 127.0.0.1, {}\nExit = http, exit.example, 8080, underlying-proxy={under}",
            entry_a.addr().port(),
            entry_b.addr().port()
        )
    };
    let h = harness(Profile {
        proxies: &proxies("EntryA"),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut first = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut first, b"before the reload").await;

    // the next generation enters the chain somewhere else
    let next = Profile {
        proxies: &proxies("EntryB"),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    }
    .text(h.dns.addr());
    let next = runtime(h.dir.path(), &next, h.engine.shared()).await;
    h.engine.swap_runtime(next);

    // the session in flight keeps the outbound — and the chain — it was dialled with
    echo_through(&mut first, b"after the reload").await;
    let mut second = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut second, b"a new session").await;
    assert_eq!(
        (entry_a.requests().len(), entry_b.requests().len()),
        (1, 1),
        "the old session stayed on EntryA; the new one went through EntryB"
    );
}

#[tokio::test]
async fn a_selection_whose_member_is_gone_falls_back_to_the_first_member() {
    let echo = rurge_proto::testing::echo_server().await;
    let upstream = |to: SocketAddr| Socks5Script {
        connect_to: Some(to),
        ..Socks5Script::default()
    };
    let a = FakeSocks5::spawn(upstream(echo)).await;
    let b = FakeSocks5::spawn(upstream(echo)).await;
    let c = FakeSocks5::spawn(upstream(echo)).await;
    let line = |name: &str, fake: &FakeSocks5| {
        format!("{name} = socks5, 127.0.0.1, {}", fake.addr().port())
    };
    let h = harness(Profile {
        proxies: &format!("{}\n{}", line("A", &a), line("B", &b)),
        groups: "Pick = select, A, B",
        rules: "DOMAIN,target.test,Pick",
        ..Profile::default()
    })
    .await;
    h.engine.shared().selections.set("Pick", "B");
    let mut t = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut t, b"via B").await;
    assert_eq!((a.requests().len(), b.requests().len()), (0, 1));

    // B leaves the profile; the saved selection now names nobody
    let next = Profile {
        proxies: &format!("{}\n{}", line("A", &a), line("C", &c)),
        groups: "Pick = select, A, C",
        rules: "DOMAIN,target.test,Pick",
        ..Profile::default()
    }
    .text(h.dns.addr());
    let next = runtime(h.dir.path(), &next, h.engine.shared()).await;
    h.engine.swap_runtime(next);
    let mut t = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut t, b"via the first member").await;
    assert_eq!(
        (a.requests().len(), c.requests().len()),
        (1, 0),
        "a stale selection means the first member"
    );
    // the table is not rewritten behind the user's back: if B comes back, so does the choice
    assert_eq!(
        h.engine.shared().selections.get("Pick").as_deref(),
        Some("B")
    );
    assert_eq!(
        h.engine
            .runtime()
            .policies
            .current_member("Pick")
            .as_deref(),
        Some("A")
    );
}

#[tokio::test]
async fn a_connector_built_before_a_reload_resolves_through_the_new_generation() {
    let echo = rurge_proto::testing::echo_server().await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("S = socks5, proxy.test, {}", upstream.addr().port());
    let h = harness(Profile {
        proxies: &proxies,
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    h.dns.set("proxy.test", &["127.0.0.1"], &[], 60);
    // the outbound of the first generation, connectors and all
    let old = h
        .engine
        .runtime()
        .policies
        .resolve(&rurge_config::rule::PolicyRef::parse("S"))
        .outbound;

    // the next generation asks another server
    let other = MockDns::spawn().await;
    other.set("proxy.test", &["127.0.0.1"], &[], 60);
    let next = Profile {
        proxies: &proxies,
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    }
    .text(other.addr());
    let next = runtime(h.dir.path(), &next, h.engine.shared()).await;
    h.engine.swap_runtime(next);

    let target =
        rurge_net::connector::Target::new(rurge_config::HostName::Ip(echo.ip()), echo.port());
    let mut stream = old
        .connect_tcp(&target, &rurge_net::connector::ConnectOpts::default())
        .await
        .expect("the old outbound still dials");
    stream.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");
    let asked = |dns: &MockDns| dns.queries().iter().any(|(q, _)| q.name == "proxy.test");
    assert!(asked(&other), "the new generation's resolver was asked");
    assert!(!asked(&h.dns), "the old generation's resolver was not");
}

fn outbound_now(h: &Harness, name: &str) -> rurge_proto::OutboundRef {
    h.engine
        .runtime()
        .policies
        .resolve(&rurge_config::rule::PolicyRef::parse(name))
        .outbound
}

#[tokio::test]
async fn an_unrelated_reload_keeps_an_anytls_pool_and_a_change_of_its_own_drops_it() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (upstream, params) = anytls_upstream(origin_addr(&origin)).await;
    let port = upstream.addr().port();
    let profile = |extra: &str, a_extra: &str| {
        format!("A = anytls, 127.0.0.1, {port}, {params}{a_extra}\n{extra}")
    };
    let h = harness(Profile {
        proxies: &profile("", ""),
        rules: "DOMAIN,target.test,A",
        ..Profile::default()
    })
    .await;
    let request = |h: &Harness| {
        let http = h.http();
        async move {
            let mut tunnel = connect_via_http(http, "target.test:8080").await;
            let response = get(&mut tunnel, "target.test", "/hello").await;
            assert!(response.ends_with("hi there"), "{response}");
        }
    };
    let log = h.engine.request_log();
    request(&h).await;
    // the record appears once the relay has dropped the stream: the session is pooled
    wait_until("the first session to finish", || log.recent(10).len() == 1).await;
    let before = outbound_now(&h, "A");

    // a reload that has nothing to do with A
    let next = Profile {
        proxies: &profile("Other = http, other.example, 8080", ""),
        rules: "DOMAIN,target.test,A",
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &next, h.engine.shared()).await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "A")),
        "A was rebuilt"
    );
    request(&h).await;
    wait_until("the second session to finish", || log.recent(10).len() == 2).await;
    assert_eq!(
        upstream.sessions(),
        1,
        "the idle session survived the reload"
    );

    // a reload that changes A itself
    let next = Profile {
        proxies: &profile("Other = http, other.example, 8080", ", reuse=false"),
        rules: "DOMAIN,target.test,A",
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &next, h.engine.shared()).await);
    assert!(!Arc::ptr_eq(&before, &outbound_now(&h, "A")), "A was kept");
    request(&h).await;
    assert_eq!(
        upstream.sessions(),
        2,
        "the new outbound dialled for itself"
    );
}

#[tokio::test]
async fn a_reused_outbound_resolves_through_the_new_generation() {
    let echo = rurge_proto::testing::echo_server().await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("S = socks5, proxy.test, {}", upstream.addr().port());
    let h = harness(Profile {
        proxies: &proxies,
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    h.dns.set("proxy.test", &["127.0.0.1"], &[], 60);
    let before = outbound_now(&h, "S");
    let other = MockDns::spawn().await;
    for name in ["proxy.test", "target.test"] {
        other.set(name, &["127.0.0.1"], &[], 60);
    }
    let next = Profile {
        proxies: &proxies,
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    }
    .text(other.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &next, h.engine.shared()).await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "S")),
        "only `dns-server` changed"
    );
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"through the reused outbound").await;
    let asked = |dns: &MockDns| dns.queries().iter().any(|(q, _)| q.name == "proxy.test");
    assert!(asked(&other) && !asked(&h.dns));
}
