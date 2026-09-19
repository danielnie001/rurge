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
use rurge_engine::{Engine, EngineShared, ListenerSpec, Runtime, RuntimeOptions};
use rurge_inbound::Running;
use rurge_net::socket::NoopSocketHook;
use rurge_net::testing::TestServer;
use rurge_proto::testing::{FakeHttpProxy, FakeSocks5, HttpProxyScript, Socks5Script};
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
    // inside a tunnel the origin sees an ordinary origin-form request
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
