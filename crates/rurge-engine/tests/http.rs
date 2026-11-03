//! The HTTP engine behind a sniffed connection (phase 4 M1 design 8, 10.2):
//! every request of one client connection is routed, dialed and recorded on
//! its own, and the connection the requests came in on has a record only when
//! none of them reached the rules.
//!
//! Nothing here goes beyond the loopback: the origins and the fake SOCKS5
//! upstreams are loopback servers, and the rules of these tests send every
//! session to one of them.

mod common;
use common::tun::*;
use common::*;
use rurge_config::rule::ProtocolKind;
use rurge_engine::FAKE_ADDRESS_NOT_KNOWN;
use std::time::Instant;

/// How long the sniffs of these tests wait for the first bytes: short, so
/// that a session that says nothing ends in a moment rather than after the 15
/// seconds of `SNIFF_WAIT`.
fn sniffing() -> EngineShared {
    EngineShared {
        sniff_wait: Duration::from_millis(300),
        ..EngineShared::default()
    }
}

/// The reply of the SOCKS5 listener to a CONNECT to `host:port` (the host as
/// a name), and the connection it came on: `tests/sniff.rs`'s helper of the
/// same name.
async fn socks5_connect(proxy: SocketAddr, host: &str, port: u16) -> ([u8; 10], TcpStream) {
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
    (reply, s)
}

/// One request over an established connection, and the whole response: the
/// request head and its body go out in one write (`CLAUDE.md`).
async fn request(s: &mut TcpStream, host: &str, path: &str) -> String {
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    read_response(s).await
}

/// The response head that starts at `s`, up to and including the blank line.
async fn read_head<S>(s: &mut S) -> String
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut byte))
            .await
            .expect("the response head comes within the bound")
            .unwrap();
        assert!(
            n > 0,
            "closed before the response head: {:?}",
            String::from_utf8_lossy(&head)
        );
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// The response that starts at `s`, head and body, as the length its head
/// gives says.
async fn read_response(s: &mut TcpStream) -> String {
    let head = read_head(s).await;
    let length: usize = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut body))
        .await
        .expect("the response body comes within the bound")
        .unwrap();
    format!("{head}{}", String::from_utf8_lossy(&body))
}

/// The client side of a TLS handshake to the fixture's `sni.test`, over any
/// stream — `tests/sniff.rs`'s helper of the same name.
async fn handshake<S>(stream: S) -> std::io::Result<tokio_rustls::client::TlsStream<S>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let config = rurge_net::http::tls_client_config(true).expect("a client config");
    let name = rustls::pki_types::ServerName::try_from("sni.test".to_string()).expect("a name");
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio_rustls::TlsConnector::from(config).connect(name, stream),
    )
    .await
    .expect("the handshake ends within the bound")
}

/// One TLS round trip over an established tunnel, then the tunnel goes.
async fn ping_over<S>(stream: S)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut tls = handshake(stream)
        .await
        .expect("the tunnel carries the handshake");
    tokio::time::timeout(Duration::from_secs(5), tls.write_all(b"ping"))
        .await
        .expect("the echo is written within the bound")
        .unwrap();
    let mut back = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), tls.read_exact(&mut back))
        .await
        .expect("the echo comes back within the bound")
        .unwrap();
    assert_eq!(&back, b"ping");
}

/// Two requests on one connection: each is routed, dialed and recorded on its
/// own, and the connection to the upstream one is borrowed by the second
/// (phase 4 M1 design 8.5).
#[tokio::test]
async fn a_sniffed_http_connection_has_a_record_per_request() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/b", "second");
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    let first = request(&mut tunnel, "target.test", "/a").await;
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    assert!(first.ends_with("first"), "{first}");
    let second = request(&mut tunnel, "target.test", "/b").await;
    assert!(second.starts_with("HTTP/1.1 200"), "{second}");
    assert!(second.ends_with("second"), "{second}");
    drop(tunnel);

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    for record in &records {
        assert_eq!(record.protocol, Some(ProtocolKind::Http), "{record:?}");
        assert_eq!(record.rule.as_deref(), Some("DOMAIN,target.test,A"));
        assert_eq!(record.policy, ["A"]);
        assert_eq!(record.dst, "target.test:80");
    }
    assert_eq!(
        a.requests().len(),
        1,
        "the second request borrowed the connection the first dialed"
    );
    assert_eq!(origin.connections(), 1);
}

/// A connection whose requests all reached the rules leaves no record of its
/// own (phase 4 M1 design 10.2, `P10`): the two records are the two requests'.
#[tokio::test]
async fn a_connection_whose_requests_were_routed_leaves_no_record_of_its_own() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/b", "second");
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    assert!(
        request(&mut tunnel, "target.test", "/a")
            .await
            .starts_with("HTTP/1.1 200")
    );
    assert!(
        request(&mut tunnel, "target.test", "/b")
            .await
            .starts_with("HTTP/1.1 200")
    );
    drop(tunnel);

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    // the only fixed wait here is for a record of the connection that came
    // late; 300 ms is far more than the connection's end needs to make one
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        h.engine.request_log().recent(10).len(),
        2,
        "no third record"
    );
    assert!(h.engine.request_log().active().is_empty());
}

/// Two requests on one connection, each matched by its own URL: each takes the
/// policy its rule names, and each dials its own upstream connection (phase 4
/// M1 design 8.3). Every entry does this: the sniffed connection, the HTTP
/// listener's absolute form, and the virtual adapter.
#[tokio::test]
async fn two_requests_of_one_connection_follow_their_own_rules() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/b", "second");
    let script = || Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    };
    let a = FakeSocks5::spawn(script()).await;
    let b = FakeSocks5::spawn(script()).await;
    let proxies = format!(
        "A = socks5, 127.0.0.1, {}\nB = socks5, 127.0.0.1, {}",
        a.addr().port(),
        b.addr().port()
    );
    let rig = Rig::on(
        harness_with(
            Profile {
                proxies: &proxies,
                rules: &guarded(
                    "URL-REGEX,^http://target\\.test/a,A\nURL-REGEX,^http://target\\.test/b,B",
                ),
                ..Profile::default()
            },
            sniffing(),
        )
        .await,
    )
    .await;
    let h = &rig.h;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    assert!(
        request(&mut tunnel, "target.test", "/a")
            .await
            .starts_with("HTTP/1.1 200")
    );
    assert!(
        request(&mut tunnel, "target.test", "/b")
            .await
            .starts_with("HTTP/1.1 200")
    );
    drop(tunnel);

    let rules = |records: &[rurge_engine::RequestRecord], at: usize| {
        assert_eq!(
            records[at].policy,
            ["A"],
            "the first request: {:?}",
            records[at]
        );
        assert_eq!(
            records[at + 1].policy,
            ["B"],
            "the second: {:?}",
            records[at + 1]
        );
        assert_eq!(
            records[at].rule.as_deref(),
            Some("URL-REGEX,^http://target\\.test/a,A")
        );
        assert_eq!(
            records[at + 1].rule.as_deref(),
            Some("URL-REGEX,^http://target\\.test/b,B")
        );
    };
    let records = the_records(h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    rules(&records, 0);
    assert_eq!(a.requests().len(), 1, "`A` is dialed by the first request");
    assert_eq!(b.requests().len(), 1, "`B` is dialed by the second");

    // the HTTP listener's absolute form, the two requests on one connection
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    for path in ["/a", "/b"] {
        s.write_all(
            format!("GET http://target.test{path} HTTP/1.1\r\nHost: target.test\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
        assert!(read_response(&mut s).await.starts_with("HTTP/1.1 200"));
    }
    drop(s);
    let records = the_records(h, 4).await;
    assert_eq!(records.len(), 4, "{records:?}");
    rules(&records, 2);
    assert_eq!(
        a.requests().len(),
        2,
        "the listener's first request dials `A`"
    );
    assert_eq!(b.requests().len(), 2, "and the second `B`");

    // the virtual adapter
    let (fake, _) = rig.fakes("target.test");
    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, 80))
        .await
        .expect("the handshake completes");
    for (path, body) in [("/a", "first"), ("/b", "second")] {
        tcp.write_all(format!("GET {path} HTTP/1.1\r\nHost: target.test\r\n\r\n").as_bytes())
            .await
            .unwrap();
        assert!(read_head(&mut tcp).await.starts_with("HTTP/1.1 200"));
        let mut back = vec![0u8; body.len()];
        tokio::time::timeout(WITHIN, tcp.read_exact(&mut back))
            .await
            .expect("the body comes within the bound")
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&back), body);
    }
    drop(tcp);
    let records = the_records(h, 6).await;
    assert_eq!(records.len(), 6, "{records:?}");
    rules(&records, 4);
    assert_eq!(a.requests().len(), 3, "the app's first request dials `A`");
    assert_eq!(b.requests().len(), 3, "and the second `B`");
}

/// A TLS connection is relayed as it is: one record, with the SNI its
/// ClientHello carried and `Https` as its protocol (design 7.1).
#[tokio::test]
async fn a_tls_connection_is_still_relayed_as_it_is() {
    let fixture = TlsFixture::new(&["sni.test"]);
    let origin = fixture.spawn_echo(false).await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "target.test", 443).await;
    ping_over(tunnel).await;

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(
        records[0].protocol,
        Some(ProtocolKind::Https),
        "{records:?}"
    );
    assert_eq!(records[0].sni.as_deref(), Some("sni.test"));
    assert_eq!(records[0].status, RecordStatus::Completed);
}

/// `force-http-engine-hosts` brings another port into the engine, whatever
/// came in on it (design 6.4): its requests get their records, and what is no
/// request is answered by the engine and recorded as a failed carrier
/// connection. Without the setting the same bytes are relayed as they are.
#[tokio::test]
async fn force_http_engine_hosts_bring_another_port_into_the_engine() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/b", "second");
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("A = socks5, 127.0.0.1, {}", a.addr().port());
    let port = origin_addr(&origin).port();

    // the port of the setting: two requests on one connection, two records
    let general = format!("force-http-engine-hosts = target.test:{port}");
    let h = harness_with(
        Profile {
            general: &general,
            proxies: &proxies,
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;
    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", port).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    assert!(
        request(&mut tunnel, "target.test", "/a")
            .await
            .starts_with("HTTP/1.1 200")
    );
    assert!(
        request(&mut tunnel, "target.test", "/b")
            .await
            .starts_with("HTTP/1.1 200")
    );
    drop(tunnel);

    // nothing that is a request: kept in the engine all the same, hyper
    // answers 400 and closes, and the carrier connection has its one record
    let (reply, mut s) = socks5_connect(h.socks(), "target.test", port).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    s.write_all(b"hello\r\n\r\n").await.unwrap();
    let head = read_head(&mut s).await;
    assert!(head.starts_with("HTTP/1.1 400"), "{head}");
    drop(s);

    let records = the_records(&h, 3).await;
    assert_eq!(records.len(), 3, "{records:?}");
    assert_eq!(records[2].protocol, None, "nothing was judged: {records:?}");
    assert_eq!(records[2].status, RecordStatus::Failed, "{records:?}");

    // without the setting the port is not sniffed, and the bytes are relayed
    // as they are
    let echo = rurge_proto::testing::echo_server().await;
    let raw = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("A = socks5, 127.0.0.1, {}", raw.addr().port());
    let h = harness_with(
        Profile {
            proxies: &proxies,
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;
    let (reply, mut s) = socks5_connect(h.socks(), "target.test", 8080).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    let sent = b"hello\r\n\r\n";
    s.write_all(sent).await.unwrap();
    let mut back = vec![0u8; sent.len()];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut back))
        .await
        .expect("the echo comes back within the bound")
        .unwrap();
    assert_eq!(&back, sent);
}

/// A connection whose opening bytes say HTTP but whose request hyper cannot
/// read never reaches the rules: the engine answers 400, and the connection
/// gets one failed record, marked HTTP as the sniff judged it (design 10.2).
#[tokio::test]
async fn a_connection_that_never_reaches_the_rules_leaves_one_failed_record() {
    let a = FakeSocks5::spawn(Socks5Script::default()).await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut s) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    // `GET` and a space make this HTTP to the sniff, but the control byte is
    // no request hyper can read
    s.write_all(b"GET \x01\r\n\r\n").await.unwrap();
    let head = read_head(&mut s).await;
    assert!(head.starts_with("HTTP/1.1 400"), "{head}");
    drop(s);

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(records[0].protocol, Some(ProtocolKind::Http), "{records:?}");
    assert!(a.requests().is_empty(), "nothing was dialed");
}

/// A loopback origin that answers a request with 101 and then echoes what it
/// is sent: the server side of a WebSocket upgrade, which `TestServer` does
/// not do (plan P16).
async fn switching_echo() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if s.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let switched = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n";
                if s.write_all(switched).await.is_err() {
                    return;
                }
                let (mut r, mut w) = s.into_split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

/// The adapter's stop ends a request that is waiting for its answer: the app
/// is let go at once, and the request's record is the completed one a cancelled
/// session gets (`P18`).
#[tokio::test]
async fn stopping_the_adapter_ends_its_http_connections() {
    let origin = TestServer::spawn().await;
    origin.set("/slow", "late");
    // the origin holds the answer back, so the request is waiting when the
    // stop comes; the test ends far sooner than this
    origin.set_delay("/slow", Duration::from_secs(600));
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let rig = Rig::new(Profile {
        proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
        rules: &guarded("DOMAIN,target.test,A"),
        ..Profile::default()
    })
    .await;
    let (fake, _) = rig.fakes("target.test");
    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, 80))
        .await
        .expect("the handshake completes");
    tcp.write_all(b"GET /slow HTTP/1.1\r\nHost: target.test\r\n\r\n")
        .await
        .unwrap();
    // The origin notes the request before it waits out the delay: this waits
    // for the request to have reached the upstream, not merely for the record
    // to exist — that one is there before the dial, and the stop is to land
    // on a request whose answer is being waited for.
    wait_until("the origin to hold the answer back", || {
        origin.requests().len() == 1
    })
    .await;

    rig.tun.stop();
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(WITHIN, tcp.read(&mut byte))
        .await
        .expect("the stop ends the request at once");
    assert!(
        matches!(read, Ok(0) | Err(_)),
        "the app is let go, not answered: {read:?}"
    );

    let records = tun_records(&rig.h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Completed, "{records:?}");

    // an idle keep-alive connection, its request done: the stop ends it too
    let origin = TestServer::spawn().await;
    origin.set("/done", "ok");
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let rig = Rig::new(Profile {
        proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
        rules: &guarded("DOMAIN,target.test,A"),
        ..Profile::default()
    })
    .await;
    let (fake, _) = rig.fakes("target.test");
    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, 80))
        .await
        .expect("the handshake completes");
    tcp.write_all(b"GET /done HTTP/1.1\r\nHost: target.test\r\n\r\n")
        .await
        .unwrap();
    let head = read_head(&mut tcp).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let mut body = [0u8; 2];
    tokio::time::timeout(WITHIN, tcp.read_exact(&mut body))
        .await
        .expect("the body comes within the bound")
        .unwrap();
    assert_eq!(&body, b"ok");
    // the exchange is over: the connection is idle, waiting for a request
    let records = tun_records(&rig.h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Completed, "{records:?}");

    rig.tun.stop();
    let read = tokio::time::timeout(WITHIN, tcp.read(&mut byte))
        .await
        .expect("the stop ends the idle connection at once");
    assert!(
        matches!(read, Ok(0) | Err(_)),
        "the app is let go: {read:?}"
    );
}

/// A connection's own record is made only when none of its requests reached
/// the rules (phase 4 M1 design 10.2, `P10`): half a request head and a close
/// leave one failed record, and a connection whose two requests were routed
/// leaves nothing beside their two records.
#[tokio::test]
async fn the_carrier_connection_is_recorded_only_when_nothing_was_routed() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/b", "second");
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    // half a request head: judged HTTP, but no request hyper can read, so
    // nothing reaches the rules
    let (reply, mut s) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    s.write_all(b"GET /half HTTP/1.1\r\nHost: target.test")
        .await
        .unwrap();
    drop(s);

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(records[0].protocol, Some(ProtocolKind::Http), "{records:?}");
    assert!(
        records[0].method.is_none(),
        "no request was read: {records:?}"
    );
    assert!(a.requests().is_empty(), "nothing was dialed");

    // two requests that were routed: their two records, and no third
    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    for path in ["/a", "/b"] {
        assert!(
            request(&mut tunnel, "target.test", path)
                .await
                .starts_with("HTTP/1.1 200")
        );
    }
    drop(tunnel);

    let records = the_records(&h, 3).await;
    assert_eq!(records.len(), 3, "{records:?}");
    assert_eq!(records[1].status, RecordStatus::Completed, "{records:?}");
    assert_eq!(records[2].status, RecordStatus::Completed, "{records:?}");
    // The only fixed wait here is for the end of the connection the client
    // has just closed: the two requests' records are made when their answers
    // are sent, and the connection is ended by the engine once it reads the
    // client's EOF. Were it to leave a record of its own, that record is made
    // then. On the loopback that step is far shorter than the 300 ms waited.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        h.engine.request_log().recent(10).len(),
        3,
        "the connection left a record of its own beside its requests'"
    );
    assert!(h.engine.request_log().active().is_empty());
}

/// A stop ends the relay of an upgraded exchange too, which outlives the
/// connection it was upgraded from (design 8.9): the app is let go at once,
/// and the record is the completed one.
#[tokio::test]
async fn stopping_the_adapter_ends_an_upgraded_relay() {
    let origin = switching_echo().await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin),
        ..Socks5Script::default()
    })
    .await;
    let rig = Rig::new(Profile {
        proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
        rules: &guarded("DOMAIN,target.test,A"),
        ..Profile::default()
    })
    .await;
    let (fake, _) = rig.fakes("target.test");
    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, 80))
        .await
        .expect("the handshake completes");
    // one write, then the switch's answer, then the stop
    tcp.write_all(
        b"GET /ws HTTP/1.1\r\nHost: target.test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(WITHIN, tcp.read(&mut byte))
            .await
            .expect("the switch comes back within the bound")
            .unwrap();
        assert!(
            n > 0,
            "closed before the switch: {:?}",
            String::from_utf8_lossy(&head)
        );
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");

    rig.tun.stop();
    let read = tokio::time::timeout(WITHIN, tcp.read(&mut byte))
        .await
        .expect("the stop ends the relay at once");
    assert!(
        matches!(read, Ok(0) | Err(_)),
        "the app is let go, not answered: {read:?}"
    );

    let records = tun_records(&rig.h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Completed, "{records:?}");
    assert_eq!(records[0].status_code, Some(101), "{records:?}");
}

/// A request's record names the method, the URL and the status the client was
/// answered with: the upstream's own, or the one rurge's refusal or error page
/// carries (phase 4 M1 design 10.1, `P5`).
#[tokio::test]
async fn a_request_record_has_its_method_url_and_status() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            general: "show-error-page-for-reject = true",
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "URL-REGEX,^http://target\\.test/r,REJECT\nURL-REGEX,^http://target\\.test/t,REJECT-TINYGIF\nDOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    // the first three requests go over one connection: a refusal answered with
    // its error page keeps the connection as an answered request does
    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    let first = request(&mut tunnel, "target.test", "/a?x=1").await;
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    let refused = request(&mut tunnel, "target.test", "/r").await;
    assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
    let tiny = request(&mut tunnel, "target.test", "/t").await;
    assert!(tiny.starts_with("HTTP/1.1 200"), "{tiny}");
    drop(tunnel);

    let records = the_records(&h, 3).await;
    assert_eq!(records.len(), 3, "{records:?}");
    assert_eq!(records[0].method.as_deref(), Some("GET"), "{records:?}");
    assert_eq!(
        records[0].url.as_deref(),
        Some("http://target.test/a?x=1"),
        "{records:?}"
    );
    assert_eq!(records[0].status_code, Some(200), "{records:?}");
    assert_eq!(records[1].method.as_deref(), Some("GET"), "{records:?}");
    assert_eq!(
        records[1].url.as_deref(),
        Some("http://target.test/r"),
        "{records:?}"
    );
    assert_eq!(records[1].status_code, Some(403), "{records:?}");
    assert_eq!(records[1].status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(records[2].status_code, Some(200), "{records:?}");
    assert_eq!(
        records[2].status,
        RecordStatus::Rejected("REJECT-TINYGIF".into()),
        "{records:?}"
    );

    // with `show-error-page-for-reject` off, a refusal closes without an
    // answer, and no status is recorded
    let silent = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "URL-REGEX,^http://target\\.test/r,REJECT\nDOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;
    let (reply, mut tunnel) = socks5_connect(silent.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    tunnel
        .write_all(b"GET /r HTTP/1.1\r\nHost: target.test\r\n\r\n")
        .await
        .unwrap();
    let mut back = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), tunnel.read_to_end(&mut back))
        .await
        .expect("the connection is closed within the bound")
        .unwrap();
    assert!(
        back.is_empty(),
        "a refusal with no page answers nothing: {:?}",
        String::from_utf8_lossy(&back)
    );

    let records = the_records(&silent, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].method.as_deref(), Some("GET"), "{records:?}");
    assert_eq!(records[0].status_code, None, "{records:?}");
}

/// The URL of a request that came in absolute form with userinfo is recorded
/// without it: credentials do not go into the records (design 12, `P4-D25`).
#[tokio::test]
async fn a_url_with_userinfo_is_recorded_without_it() {
    let origin = TestServer::spawn().await;
    origin.set("/x", "a body");
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    tunnel
        .write_all(b"GET http://u:p@target.test/x HTTP/1.1\r\nHost: target.test\r\n\r\n")
        .await
        .unwrap();
    let answer = read_response(&mut tunnel).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    drop(tunnel);

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(
        records[0].url.as_deref(),
        Some("http://target.test/x"),
        "{records:?}"
    );
}

/// The record of a request carried by a borrowed connection says where that
/// connection went, and that it was ready when the request took it (design
/// 10.1): both requests name the fake SOCKS5 the one dial went to.
#[tokio::test]
async fn a_borrowed_connection_records_where_it_went() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/b", "second");
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    assert!(
        request(&mut tunnel, "target.test", "/a")
            .await
            .starts_with("HTTP/1.1 200")
    );
    assert!(
        request(&mut tunnel, "target.test", "/b")
            .await
            .starts_with("HTTP/1.1 200")
    );
    drop(tunnel);

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(a.requests().len(), 1, "the second request borrowed it");
    for record in &records {
        assert_eq!(record.remote_addr, Some(a.addr()), "{record:?}");
        assert!(record.connect_ms.is_some(), "{record:?}");
        assert!(record.first_byte_ms.is_some(), "{record:?}");
    }
}

/// The line a finished request leaves has its method and status; the one that
/// completed has its path, without the query string and without userinfo
/// (design 12, `P20`): a query string often carries a token.
#[tokio::test]
async fn the_request_log_line_shows_no_query_and_no_userinfo() {
    let log = LogCapture::start();
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            general: "show-error-page-for-reject = true",
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "URL-REGEX,^http://target\\.test/r,REJECT\nDOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    // absolute form, with userinfo and a query string: neither is to survive
    // in the line
    tunnel
        .write_all(b"GET http://u:p@target.test/a?x=1 HTTP/1.1\r\nHost: target.test\r\n\r\n")
        .await
        .unwrap();
    assert!(read_response(&mut tunnel).await.starts_with("HTTP/1.1 200"));
    assert!(
        request(&mut tunnel, "target.test", "/r")
            .await
            .starts_with("HTTP/1.1 403")
    );
    drop(tunnel);

    let _ = the_records(&h, 2).await;
    let lines = log.lines();
    let completed: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains("session completed"))
        .collect();
    assert_eq!(completed.len(), 1, "{lines:?}");
    let line = completed[0];
    assert!(line.contains(" method=GET"), "{line}");
    assert!(line.contains(" status=200"), "{line}");
    assert!(line.contains(" url=http://target.test/a"), "{line}");
    assert!(!line.contains("x=1"), "{line}");
    assert!(!line.contains("u:p"), "{line}");

    let refused: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains("session rejected by REJECT"))
        .collect();
    assert_eq!(refused.len(), 1, "{lines:?}");
    let line = refused[0];
    assert!(line.contains(" method=GET"), "{line}");
    assert!(line.contains(" status=403"), "{line}");
    assert!(!line.contains("url="), "the path is DEBUG only: {line}");
}

// ---- the HTTP listener's absolute-form requests, through the engine

/// Before a listener request reaches the origin, its absolute form is
/// rewritten: the request target's authority becomes `Host`, the hop-by-hop
/// headers and the client's `Proxy-Authorization` go, and hop-by-hop headers
/// of the response do not come back (design 8.7, 9). A second request on the
/// same connection takes the policy its own rule names.
#[tokio::test]
async fn the_listener_rewrites_a_plain_request_for_the_origin() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    // hop-by-hop headers the origin puts on its response must not come back out
    origin.set_header("/hello", "connection", "X-Down");
    origin.set_header("/hello", "x-down", "leaked");
    let port = origin_addr(&origin).port();
    let h = harness(Profile {
        rules: "URL-REGEX,ad\\.gif$,REJECT-TINYGIF",
        ..Profile::default()
    })
    .await;

    let mut s = TcpStream::connect(h.http()).await.unwrap();
    // userinfo in the request target, a lying Host, and a pile of hop-by-hop
    // headers
    s.write_all(
        format!(
            "GET http://u:p@target.test:{port}/hello HTTP/1.1\r\nHost: evil.internal\r\n\
             User-Agent: t/1\r\nProxy-Authorization: Basic eDp5\r\n\
             Proxy-Connection: keep-alive\r\nConnection: keep-alive, X-Hop\r\n\
             X-Hop: leaked\r\nKeep-Alive: timeout=5\r\nTE: trailers\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let first = read_response(&mut s).await;
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    assert!(first.ends_with("hi there"), "{first}");
    let lower = first.to_ascii_lowercase();
    assert!(
        !lower.contains("\r\nx-down:") && !lower.contains("\r\nconnection:"),
        "response hop-by-hop headers survived: {first}"
    );
    // the same connection carries the next request, which its own rule rejects
    s.write_all(
        format!(
            "GET http://target.test:{port}/ad.gif HTTP/1.1\r\nHost: target.test:{port}\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let second = read_response(&mut s).await;
    assert!(
        second.starts_with("HTTP/1.1 200")
            && second
                .to_ascii_lowercase()
                .contains("content-type: image/gif"),
        "{second}"
    );
    assert!(
        second.to_ascii_lowercase().contains("content-length: 43"),
        "the one-pixel gif, whole: {second}"
    );
    drop(s);

    let reqs = origin.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].path, "/hello");
    // RFC 9112 §3.2.2: the request target wins over whatever `Host` came in
    assert_eq!(
        reqs[0].header("host"),
        Some(format!("target.test:{port}").as_str())
    );
    for gone in [
        "proxy-authorization",
        "proxy-connection",
        "connection",
        "x-hop",
        "keep-alive",
        "te",
    ] {
        assert!(reqs[0].header(gone).is_none(), "{gone} was forwarded");
    }
    assert_eq!(reqs[0].header("user-agent"), Some("t/1"));

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].protocol, Some(ProtocolKind::Http), "{records:?}");
    assert_eq!(records[0].method.as_deref(), Some("GET"), "{records:?}");
    assert_eq!(
        records[0].url.as_deref(),
        Some(format!("http://target.test:{port}/hello").as_str()),
        "the recorded URL is rebuilt without the `u:p@`"
    );
    assert_eq!(records[0].status_code, Some(200), "{records:?}");
    assert_eq!(
        records[1].status,
        RecordStatus::Rejected("REJECT-TINYGIF".into()),
        "{records:?}"
    );
    assert_eq!(records[1].status_code, Some(200), "{records:?}");
}

/// A listener request's refusals and failures are answered as the profile
/// says (design 8.8): a REJECT closes the connection, or shows its page when
/// `show-error-page-for-reject` is on; a failure answers the 502 page, or
/// closes when `show-error-page` is off. REJECT-DROP's 30-second hold is not
/// waited for here.
#[tokio::test]
async fn the_listener_answers_refusals_and_failures_as_the_profile_says() {
    // the defaults: a reject closes, a failure shows a 502 page. The IP rule
    // makes the rules need the name's addresses, so a name with none fails
    // the lookup itself.
    let h = harness(Profile {
        rules: "DOMAIN,reject.test,REJECT\nIP-CIDR,10.0.0.0/8,REJECT",
        ..Profile::default()
    })
    .await;
    assert!(
        plain_get(h.http(), "http://reject.test/", "reject.test")
            .await
            .is_empty(),
        "a reject closes without an answer"
    );
    let page = plain_get(h.http(), "http://bad.test/", "bad.test").await;
    assert!(page.starts_with("HTTP/1.1 502"), "{page}");
    assert!(
        page.contains("DNS lookup failed") && page.contains("bad.test:80"),
        "{page}"
    );

    // with the reject page on and the error page off: a `REJECT-NO-DROP`
    // shows its rule, a failure says nothing
    let h = harness(Profile {
        general: "show-error-page = false\nshow-error-page-for-reject = true",
        rules: "DOMAIN,nodrop.test,REJECT-NO-DROP\nIP-CIDR,10.0.0.0/8,REJECT",
        ..Profile::default()
    })
    .await;
    let page = plain_get(h.http(), "http://nodrop.test/x", "nodrop.test").await;
    assert!(page.starts_with("HTTP/1.1 403"), "{page}");
    assert!(
        page.contains("REJECT-NO-DROP") && page.contains("DOMAIN,nodrop.test,REJECT-NO-DROP"),
        "{page}"
    );
    assert!(
        plain_get(h.http(), "http://bad.test/", "bad.test")
            .await
            .is_empty(),
        "a failure closes when pages are off"
    );
}

/// A listener request is a session like any other: `kill` ends it, the client
/// connection goes with no answer, and the record says why (design 8.11).
#[tokio::test]
async fn a_plain_request_is_killable() {
    let origin = TestServer::spawn().await;
    origin.set("/hang", "late");
    // the origin holds the answer back, so the request is waiting when the
    // kill comes; the test ends far sooner than this
    origin.set_delay("/hang", Duration::from_secs(600));
    let port = origin_addr(&origin).port();
    let h = harness(Profile::default()).await;

    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(
        format!("GET http://target.test:{port}/hang HTTP/1.1\r\nHost: target.test:{port}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    // The origin notes the request before it waits out the delay: this waits
    // for the answer to be held back, not merely for the record to exist —
    // that one is there before the dial, and the kill has to land on a
    // request whose upstream is waiting.
    wait_until("the origin to hold the answer back", || {
        origin.requests().len() == 1
    })
    .await;
    let id = h.engine.request_log().active()[0].id;
    assert!(h.engine.kill(id));

    // the connection ends rather than hanging or getting a 502
    let mut buf = Vec::new();
    let read = tokio::time::timeout(WITHIN, s.read_to_end(&mut buf)).await;
    assert!(read.is_ok(), "the kill ends the connection at once");
    assert!(
        buf.is_empty(),
        "the client is let go, not answered: {:?}",
        String::from_utf8_lossy(&buf)
    );
    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(records[0].error.as_deref(), Some("killed"), "{records:?}");
}

/// A client that leaves in the middle of its upload is no failure of
/// upstream's (design 8.6): the record says the client left, and the `smart`
/// member the request went through is not counted as failing. One thread, on
/// purpose: `SessionHandle::finish` runs every hook before it returns — the
/// record's, hung when the session was made, and `smart::watch`'s, hung at
/// the dial — so a record the test can see means the member has been told
/// too. On several threads the test could read between the two and stay
/// green even if the client's own error were counted.
#[tokio::test]
async fn a_client_leaving_mid_upload_is_no_upstream_failure() {
    // an HTTP proxy that reads the request and never answers
    let rude = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = rude.local_addr().unwrap();
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let mut seen_tx = Some(seen_tx);
        let mut held = Vec::new();
        while let Ok((mut s, _)) = rude.accept().await {
            if let Some(tx) = seen_tx.take() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let _ = tx.send(());
            }
            held.push(s);
        }
    });
    let proxies = format!("Rude = http, 127.0.0.1, {}", addr.port());
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, Rude",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;

    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(
        b"POST http://target.test/up HTTP/1.1\r\nHost: target.test\r\nContent-Length: 10\r\n\r\nabc",
    )
    .await
    .unwrap();
    // the request reached the proxy, so the client's leaving is what ends it
    tokio::time::timeout(WITHIN, seen_rx)
        .await
        .expect("the upstream saw the request")
        .unwrap();
    drop(s);

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    // every hook has run before the record was there (see the test's
    // comment); the client's own error must not be among them
    let registry = h.engine.registry();
    let failed = registry
        .auto()
        .smart
        .site("target.test", Instant::now())
        .failed;
    assert!(
        failed.is_empty(),
        "the client's own error counted against the member: {failed:?}"
    );
}

/// The record of a listener request names the client and the listener port,
/// as a tunnel's record does (design 8.3, 10.1).
#[tokio::test]
async fn a_plain_request_record_names_the_client_and_the_listener_port() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let port = origin_addr(&origin).port();
    let h = harness(Profile::default()).await;

    let mut s = TcpStream::connect(h.http()).await.unwrap();
    let client = s.local_addr().unwrap();
    s.write_all(
        format!("GET http://target.test:{port}/hello HTTP/1.1\r\nHost: target.test:{port}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    assert!(read_response(&mut s).await.starts_with("HTTP/1.1 200"));
    drop(s);

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].listener, ListenerKind::Http, "{records:?}");
    assert_eq!(records[0].src, client, "{records:?}");
    assert_eq!(records[0].listen_port, h.http().port(), "{records:?}");
    assert_eq!(records[0].dst, format!("target.test:{port}"), "{records:?}");
}

/// A URL host written in IPv4-mapped IPv6 form is the IPv4 address the rules
/// and the dial see, as it is on a `CONNECT` (design 9).
#[tokio::test]
async fn an_ipv4_mapped_url_host_is_dialed_as_ipv4() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let port = origin_addr(&origin).port();
    let h = harness(Profile {
        rules: "IP-CIDR,127.0.0.1/32,DIRECT",
        ..Profile::default()
    })
    .await;

    let answer = plain_get(
        h.http(),
        &format!("http://[::ffff:127.0.0.1]:{port}/hello"),
        &format!("[::ffff:127.0.0.1]:{port}"),
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("hi there"), "{answer}");

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].dst, format!("127.0.0.1:{port}"), "{records:?}");
    assert_eq!(
        records[0].rule.as_deref(),
        Some("IP-CIDR,127.0.0.1/32,DIRECT"),
        "the rules saw the IPv4 address: {records:?}"
    );
}

/// The fake address of a name, named in a listener request's URL and `Host`,
/// is the name before anything else sees it: the rules, the record and the
/// origin's `Host`, and a lease holds the address until the request ends
/// (design 9, R27). The rules are the proxy port's own (plan P24): were the
/// address not turned back, the `FINAL,DIRECT` after them must not take it to
/// a dial outside the loopback. No `guarded` is needed: the request comes in
/// at the listener, from the loopback, not from the app's address.
#[tokio::test]
async fn a_fake_address_in_a_plain_request_is_turned_back_into_its_name() {
    let origin = TestServer::spawn().await;
    origin.set("/x", "a body");
    let port = origin_addr(&origin).port();
    let rig = Rig::new(profile(&listener_rules(port))).await;
    let (fake, _) = rig.fakes("target.test");

    let answer = plain_get(
        rig.h.http(),
        &format!("http://{fake}:{port}/x"),
        &format!("{fake}:{port}"),
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("a body"), "{answer}");

    let records = the_records(&rig.h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].dst, format!("target.test:{port}"), "{records:?}");
    assert_eq!(
        records[0].url.as_deref(),
        Some(format!("http://target.test:{port}/x").as_str()),
        "{records:?}"
    );
    assert_eq!(
        records[0].rule.as_deref(),
        Some("DOMAIN,target.test,DIRECT"),
        "{records:?}"
    );
    let reqs = origin.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(
        reqs[0].header("host"),
        Some(format!("target.test:{port}").as_str()),
        "the origin is sent the name"
    );
}

/// An address of rurge's space with no name behind it fails the listener's
/// request, which says so and dials nothing (design 9). Unlike a `CONNECT`,
/// the rules see the request first — they are asked before the address is
/// known to be nameless — so the record carries the rule that matched it.
/// That rule refuses the address, but a REJECT is a refusal of the dial
/// (`route_session` does not refuse it), and the namelessness is decided
/// after the rules and before the dial: hence the 502, and no dial.
#[tokio::test]
async fn an_unknown_fake_address_in_a_plain_request_is_a_502() {
    let origin = TestServer::spawn().await;
    origin.set("/x", "a body");
    let port = origin_addr(&origin).port();
    let rig = Rig::new(profile(&listener_rules(port))).await;

    let answer = plain_get(
        rig.h.http(),
        &format!("http://198.18.200.1:{port}/x"),
        &format!("198.18.200.1:{port}"),
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 502"), "{answer}");
    assert!(answer.contains(FAKE_ADDRESS_NOT_KNOWN), "{answer}");

    let records = the_records(&rig.h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(
        records[0].dst,
        format!("198.18.200.1:{port}"),
        "{records:?}"
    );
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(records[0].status_code, Some(502), "{records:?}");
    assert_eq!(
        records[0].error.as_deref(),
        Some(FAKE_ADDRESS_NOT_KNOWN),
        "{records:?}"
    );
    assert!(records[0].remote_addr.is_none(), "nothing was dialed");
    assert_eq!(
        records[0].rule.as_deref(),
        Some("IP-CIDR,198.18.0.0/15,REJECT,no-resolve"),
        "the rules saw the request before the address was found nameless: {records:?}"
    );
}

/// An origin that answers 200 with a body of `total` bytes and writes them,
/// stopping at whatever the client's buffers will not take, then reads until
/// the client goes: `TestServer` serves a body whole, which a client that
/// never reads would take at once.
async fn streaming_origin(total: usize) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match s.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => head.push(byte[0]),
                    }
                }
                let answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n\r\n");
                if s.write_all(answer.as_bytes()).await.is_err() {
                    return;
                }
                let chunk = vec![b'x'; 16 * 1024];
                let mut sent = 0;
                while sent < total {
                    let take = chunk.len().min(total - sent);
                    if s.write_all(&chunk[..take]).await.is_err() {
                        return;
                    }
                    sent += take;
                }
                let mut sink = [0u8; 1024];
                while matches!(s.read(&mut sink).await, Ok(n) if n > 0) {}
            });
        }
    });
    addr
}

/// A killed request whose client does not read goes at once (`design 8.10`,
/// 8.11): nothing polls the response body any more, so only the listener's
/// race with `Conn::abort` can end the connection, and the record is the
/// killed one.
#[tokio::test]
async fn a_killed_plain_request_whose_client_does_not_read_is_closed() {
    // 256 MiB: more than both sides' socket buffers can take on any host (the
    // Linux loopback's `tcp_rmem`/`tcp_wmem` maxima are 6 and 4 MiB by default,
    // 16 to 32 MiB on a tuned one; macOS about 4 MiB each). Were the body to
    // fit, the request would complete instead of waiting on the client. The
    // origin writes only while the client's buffers take it, so the size costs
    // no memory.
    let origin = streaming_origin(256 * 1024 * 1024).await;
    let port = origin.port();
    let h = harness(Profile::default()).await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(
        format!("GET http://target.test:{port}/big HTTP/1.1\r\nHost: target.test:{port}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    // The client reads nothing, so the response stops inside the socket's
    // buffers. The kill comes only once the download has filled them: while
    // hyper still polls the body, the `Tail` sees the cancelled token by
    // itself and the connection ends without the listener's race; filled,
    // nothing polls it again, and only the race can end it. Two samples 200 ms
    // apart that agree say the buffers took no more.
    let deadline = tokio::time::Instant::now() + WITHIN;
    let mut last = 0u64;
    loop {
        let down = h
            .engine
            .request_log()
            .active()
            .first()
            .map(|record| record.down)
            .unwrap_or(0);
        if down > 0 && down == last {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the download never filled the client's buffers"
        );
        last = down;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let id = h.engine.request_log().active()[0].id;
    assert!(h.engine.kill(id));

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(records[0].error.as_deref(), Some("killed"), "{records:?}");

    // what was buffered is read and the connection ends; a reset is an end
    // too
    let mut sink = Vec::new();
    let read = tokio::time::timeout(WITHIN, s.read_to_end(&mut sink)).await;
    assert!(read.is_ok(), "the connection ends within the bound");
}

// ---- every entry into the engine

/// The four ways a request reaches the engine — a sniffed SOCKS5 tunnel, a
/// `CONNECT`, the virtual adapter, the HTTP listener's absolute form — each
/// bring their requests into it: two requests on one client connection leave
/// two records, with the entry's own listener, and the method, the URL and
/// the status the client was answered with (phase 4 M1 design 7, 10.1).
#[tokio::test]
async fn every_entry_brings_its_requests_into_the_engine() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/b", "second");
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let rig = Rig::on(
        harness_with(
            Profile {
                proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
                rules: &guarded("DOMAIN,target.test,A"),
                ..Profile::default()
            },
            sniffing(),
        )
        .await,
    )
    .await;

    // what one entry's two records must say
    let two = |records: &[rurge_engine::RequestRecord], at: usize, listener, first: &str| {
        for (i, path) in [first, "/b"].iter().enumerate() {
            let record = &records[at + i];
            assert_eq!(record.listener, listener, "{record:?}");
            assert_eq!(record.protocol, Some(ProtocolKind::Http), "{record:?}");
            assert_eq!(record.method.as_deref(), Some("GET"), "{record:?}");
            assert_eq!(
                record.url.as_deref(),
                Some(format!("http://target.test{path}").as_str()),
                "{record:?}"
            );
            assert_eq!(record.status_code, Some(200), "{record:?}");
            assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
        }
    };

    // a sniffed SOCKS5 tunnel to port 80
    let (reply, mut tunnel) = socks5_connect(rig.h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    for path in ["/a", "/b"] {
        assert!(
            request(&mut tunnel, "target.test", path)
                .await
                .starts_with("HTTP/1.1 200")
        );
    }
    drop(tunnel);
    two(&the_records(&rig.h, 2).await, 0, ListenerKind::Socks5, "/a");

    // a `CONNECT` tunnel through the HTTP listener, sniffed for HTTP
    let mut tunnel = connect_via_http(rig.h.http(), "target.test:80").await;
    for path in ["/a", "/b"] {
        assert!(
            request(&mut tunnel, "target.test", path)
                .await
                .starts_with("HTTP/1.1 200")
        );
    }
    drop(tunnel);
    two(&the_records(&rig.h, 4).await, 2, ListenerKind::Http, "/a");

    // the virtual adapter
    let (fake, _) = rig.fakes("target.test");
    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, 80))
        .await
        .expect("the handshake completes");
    for (path, body) in [("/a", "first"), ("/b", "second")] {
        tcp.write_all(format!("GET {path} HTTP/1.1\r\nHost: target.test\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let head = read_head(&mut tcp).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let mut back = vec![0u8; body.len()];
        tokio::time::timeout(WITHIN, tcp.read_exact(&mut back))
            .await
            .expect("the body comes within the bound")
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&back), body);
    }
    drop(tcp);
    two(&the_records(&rig.h, 6).await, 4, ListenerKind::Tun, "/a");

    // the HTTP listener's absolute form
    let mut s = TcpStream::connect(rig.h.http()).await.unwrap();
    for path in ["/a", "/b"] {
        s.write_all(
            format!("GET http://target.test{path} HTTP/1.1\r\nHost: target.test\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
        assert!(read_response(&mut s).await.starts_with("HTTP/1.1 200"));
    }
    drop(s);
    two(&the_records(&rig.h, 8).await, 6, ListenerKind::Http, "/a");

    assert_eq!(
        a.requests().len(),
        4,
        "each client connection dials its own upstream connection"
    );
}

/// One upstream connection is lent to every request of its key (phase 4 M1
/// design 8.5, `P16`): three requests of one connection reach the origin over
/// one TCP connection, dialed once through the fake SOCKS5. What a key that
/// differs does is `two_requests_of_one_connection_follow_their_own_rules`,
/// `a_reload_or_a_new_egress_table_lends_no_old_connection` and
/// `a_select_switch_lends_no_old_connection`.
#[tokio::test]
async fn the_upstream_connection_is_lent_and_counted() {
    let origin = TestServer::spawn().await;
    for path in ["/a", "/b", "/c"] {
        origin.set(path, "body");
    }
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    for path in ["/a", "/b", "/c"] {
        assert!(
            request(&mut tunnel, "target.test", path)
                .await
                .starts_with("HTTP/1.1 200")
        );
    }
    drop(tunnel);

    let records = the_records(&h, 3).await;
    assert_eq!(records.len(), 3, "{records:?}");
    for record in &records {
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
        assert_eq!(record.remote_addr, Some(a.addr()), "{record:?}");
    }
    assert_eq!(a.requests().len(), 1, "one dial for the three requests");
    assert_eq!(
        origin.connections(),
        1,
        "the three requests went over one upstream connection"
    );
}

/// A socket hook whose egress table is the table it holds now: the test
/// changes it by putting another `Arc` in its place, which is what the
/// manager does when the enhanced mode writes a new table (phase 3 M4 design
/// 7.2, phase 4 M1 design 8.5).
struct MovingEgress {
    table: std::sync::Mutex<Arc<()>>,
}

impl MovingEgress {
    fn new() -> Arc<MovingEgress> {
        Arc::new(MovingEgress {
            table: std::sync::Mutex::new(Arc::new(())),
        })
    }

    fn new_table(&self) {
        *self.table.lock().unwrap() = Arc::new(());
    }
}

impl rurge_net::socket::SocketHook for MovingEgress {
    fn bind_interface(
        &self,
        _: &socket2::Socket,
        _: &str,
        _: rurge_net::socket::Family,
    ) -> std::io::Result<()> {
        Ok(())
    }

    fn set_tos(
        &self,
        _: &socket2::Socket,
        _: rurge_net::socket::Family,
        _: u8,
    ) -> std::io::Result<()> {
        Ok(())
    }

    fn egress_route(&self) -> Option<rurge_net::connector::Route> {
        Some(rurge_net::connector::Route::egress(
            &*self.table.lock().unwrap(),
        ))
    }
}

/// A connection dialed under one generation or one egress table is not lent
/// to a request of another: a reload (another generation) and a new egress
/// table both make the next request dial its own connection (phase 4 M1
/// design 8.5).
#[tokio::test]
async fn a_reload_or_a_new_egress_table_lends_no_old_connection() {
    // a reload, between two requests of one sniffed connection
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/b", "second");
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    assert!(
        request(&mut tunnel, "target.test", "/a")
            .await
            .starts_with("HTTP/1.1 200")
    );
    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    h.engine
        .swap_runtime(runtime(h.dir.path(), &text, h.engine.shared()).await);
    assert!(
        request(&mut tunnel, "target.test", "/b")
            .await
            .starts_with("HTTP/1.1 200")
    );
    drop(tunnel);

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(
        a.requests().len(),
        2,
        "the request after the reload dialed its own connection"
    );

    // a new egress table, between two requests of one listener connection
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/b", "second");
    let port = origin_addr(&origin).port();
    let hook = MovingEgress::new();
    let h = harness_hooked(profile(&listener_rules(port)), hook.clone()).await;

    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(
        format!("GET http://target.test:{port}/a HTTP/1.1\r\nHost: target.test:{port}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    assert!(read_response(&mut s).await.starts_with("HTTP/1.1 200"));
    assert_eq!(origin.connections(), 1, "the first request dialed once");

    hook.new_table();
    s.write_all(
        format!("GET http://target.test:{port}/b HTTP/1.1\r\nHost: target.test:{port}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    assert!(read_response(&mut s).await.starts_with("HTTP/1.1 200"));
    drop(s);

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(
        origin.connections(),
        2,
        "the request after the new table dialed its own connection"
    );
}

/// A `select` group that switched members between two requests: the second
/// request does not take the connection dialed for the first member, but
/// dials its own through the new one (phase 4 M1 design 8.5).
#[tokio::test]
async fn a_select_switch_lends_no_old_connection() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/b", "second");
    let script = || Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    };
    let a = FakeSocks5::spawn(script()).await;
    let b = FakeSocks5::spawn(script()).await;
    let proxies = format!(
        "A = socks5, 127.0.0.1, {}\nB = socks5, 127.0.0.1, {}",
        a.addr().port(),
        b.addr().port()
    );
    let h = harness_with(
        Profile {
            proxies: &proxies,
            groups: "S = select, A, B",
            rules: "DOMAIN,target.test,S",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    assert!(
        request(&mut tunnel, "target.test", "/a")
            .await
            .starts_with("HTTP/1.1 200")
    );
    assert_eq!(
        (a.requests().len(), b.requests().len()),
        (1, 0),
        "the first request dialed the first member"
    );

    h.engine.select_group("S", "B").await.unwrap();
    assert!(
        request(&mut tunnel, "target.test", "/b")
            .await
            .starts_with("HTTP/1.1 200")
    );
    drop(tunnel);

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].policy, ["S", "A"], "{records:?}");
    assert_eq!(records[1].policy, ["S", "B"], "{records:?}");
    assert_eq!(records[0].remote_addr, Some(a.addr()), "{records:?}");
    assert_eq!(
        records[1].remote_addr,
        Some(b.addr()),
        "the request after the switch went through the new member: {records:?}"
    );
    assert_eq!(
        (a.requests().len(), b.requests().len()),
        (1, 1),
        "the second request dialed its own connection through `B`"
    );
}

/// The bytes of a borrowed connection are counted onto the request that has
/// it now, not onto the request that dialed it (phase 4 M1 design 8.5, `P7`):
/// two requests over one upstream connection, answered with 100 and 5000
/// bytes, count their own sizes.
#[tokio::test]
async fn each_request_counts_its_own_bytes() {
    let origin = TestServer::spawn().await;
    origin.set("/small", "x".repeat(100));
    origin.set("/big", "y".repeat(5000));
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    let small = request(&mut tunnel, "target.test", "/small").await;
    assert!(small.starts_with("HTTP/1.1 200"), "{small}");
    let big = request(&mut tunnel, "target.test", "/big").await;
    assert!(big.starts_with("HTTP/1.1 200"), "{big}");
    drop(tunnel);

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(a.requests().len(), 1, "the second request borrowed it");
    assert_eq!(origin.connections(), 1, "one upstream connection");
    assert!(
        records[0].down < 5000,
        "the first record counted the 5000 bytes that belong to the second: {records:?}"
    );
    assert!(
        records[1].down >= 5000,
        "the second record did not count its own body: {records:?}"
    );
    for record in &records {
        assert!(record.up > 0, "no request bytes counted: {record:?}");
    }
}

/// One request over a fresh sniffed SOCKS5 tunnel to `host`, and the whole
/// response.
async fn sniffed_get(h: &Harness, host: &str, path: &str) -> String {
    let (reply, mut tunnel) = socks5_connect(h.socks(), host, 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    request(&mut tunnel, host, path).await
}

/// As `sniffed_get`, for an answer that closes the connection: what the
/// client reads up to the close.
async fn sniffed_close(h: &Harness, host: &str, path: &str) -> Vec<u8> {
    let (reply, mut tunnel) = socks5_connect(h.socks(), host, 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    tunnel
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut back = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), tunnel.read_to_end(&mut back))
        .await
        .expect("the connection is closed within the bound")
        .unwrap();
    back
}

/// One listener request in absolute form, and the whole response.
async fn listener_get(h: &Harness, url: &str, host: &str) -> String {
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(format!("GET {url} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    read_response(&mut s).await
}

/// As `listener_get`, for an answer that closes the connection.
async fn listener_close(h: &Harness, url: &str, host: &str) -> Vec<u8> {
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(format!("GET {url} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut back = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut back))
        .await
        .expect("the connection is closed within the bound")
        .unwrap();
    back
}

/// A refusal or a failure is answered as the profile says on every entry
/// (phase 4 M1 design 8.8, 9): a REJECT closes the connection or shows its
/// page, `REJECT-TINYGIF` shows its pixel, and a name the rules need the
/// addresses of, which the DNS has none of, fails the lookup: the 502 page,
/// or a close when pages are off.
#[tokio::test]
async fn refusals_are_answered_on_every_entry() {
    let rules = "URL-REGEX,^http://target\\.test/r,REJECT\nURL-REGEX,^http://target\\.test/t,REJECT-TINYGIF\nIP-CIDR,10.0.0.0/8,REJECT";
    let answered_with = |answer: &str, status: &str| {
        assert!(answer.starts_with(status), "{answer}");
    };

    // the REJECT page on; the failure page on (the default)
    let h = harness_with(
        Profile {
            general: "show-error-page-for-reject = true",
            rules,
            ..Profile::default()
        },
        sniffing(),
    )
    .await;
    answered_with(&sniffed_get(&h, "target.test", "/r").await, "HTTP/1.1 403");
    let tiny = sniffed_get(&h, "target.test", "/t").await;
    assert!(tiny.starts_with("HTTP/1.1 200"), "{tiny}");
    assert!(tiny.to_ascii_lowercase().contains("image/gif"), "{tiny}");
    let failed = sniffed_get(&h, "bad.test", "/").await;
    assert!(failed.starts_with("HTTP/1.1 502"), "{failed}");
    assert!(failed.contains("DNS lookup failed"), "{failed}");

    answered_with(
        &listener_get(&h, "http://target.test/r", "target.test").await,
        "HTTP/1.1 403",
    );
    let tiny = listener_get(&h, "http://target.test/t", "target.test").await;
    assert!(tiny.starts_with("HTTP/1.1 200"), "{tiny}");
    assert!(tiny.to_ascii_lowercase().contains("image/gif"), "{tiny}");
    let failed = listener_get(&h, "http://bad.test/", "bad.test").await;
    assert!(failed.starts_with("HTTP/1.1 502"), "{failed}");
    assert!(failed.contains("DNS lookup failed"), "{failed}");

    // both pages off: a refusal and a failure close without an answer, the
    // pixel still goes
    let h = harness_with(
        Profile {
            general: "show-error-page = false\nshow-error-page-for-reject = false",
            rules,
            ..Profile::default()
        },
        sniffing(),
    )
    .await;
    assert!(sniffed_close(&h, "target.test", "/r").await.is_empty());
    let tiny = sniffed_get(&h, "target.test", "/t").await;
    assert!(tiny.starts_with("HTTP/1.1 200"), "{tiny}");
    assert!(sniffed_close(&h, "bad.test", "/").await.is_empty());

    assert!(
        listener_close(&h, "http://target.test/r", "target.test")
            .await
            .is_empty()
    );
    let tiny = listener_get(&h, "http://target.test/t", "target.test").await;
    assert!(tiny.starts_with("HTTP/1.1 200"), "{tiny}");
    assert!(
        listener_close(&h, "http://bad.test/", "bad.test")
            .await
            .is_empty()
    );
}

/// An origin that has read the request and does not answer it in time is a
/// 504 (phase 4 M1 design 8.10): the record is the failed one, with the
/// status the client was answered with; with the error page off, the
/// connection is closed instead.
#[tokio::test]
async fn a_late_response_head_is_a_504() {
    let origin = TestServer::spawn().await;
    origin.set("/late", "late");
    // the wait for the response head is `idle_timeout`; the test ends far
    // sooner than the origin's delay
    origin.set_delay("/late", Duration::from_secs(600));
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_timed(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
        Duration::from_millis(300),
    )
    .await;

    let answer = sniffed_get(&h, "target.test", "/late").await;
    assert!(answer.starts_with("HTTP/1.1 504"), "{answer}");
    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(records[0].status_code, Some(504), "{records:?}");

    // with the error page off, the client is let go rather than answered
    let h = harness_timed(
        Profile {
            general: "show-error-page = false",
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
        Duration::from_millis(300),
    )
    .await;
    assert!(sniffed_close(&h, "target.test", "/late").await.is_empty());
    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(records[0].status_code, None, "{records:?}");
}

/// A client that goes away in the middle of a response leaves no record open
/// (phase 4 M1 design 8.11, `P18`): the request is failed with the client's
/// own going as its error, and it is out of the active list.
#[tokio::test]
async fn a_client_that_leaves_mid_response_leaves_no_open_record() {
    // 256 MiB: more than both sides' buffers take (see `streaming_origin`),
    // so the response is still coming when the client leaves; the origin
    // writes only while the client's buffers take it, so the size costs no
    // memory
    let origin = streaming_origin(256 * 1024 * 1024).await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    tunnel
        .write_all(b"GET /big HTTP/1.1\r\nHost: target.test\r\n\r\n")
        .await
        .unwrap();
    let head = read_head(&mut tunnel).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let mut some = [0u8; 4096];
    tokio::time::timeout(WITHIN, tunnel.read_exact(&mut some))
        .await
        .expect("the body starts coming within the bound")
        .unwrap();
    // the client leaves while the download is still going
    drop(tunnel);

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(
        records[0].error.as_deref(),
        Some("client closed the connection"),
        "{records:?}"
    );
    wait_until("the record to leave the active list", || {
        h.engine.request_log().active().is_empty()
    })
    .await;
}

/// A request of a sniffed connection is a session like any other: `kill` ends
/// it, the client is let go rather than answered, and the record says why
/// (phase 4 M1 design 8.11).
#[tokio::test]
async fn a_request_killed_through_the_engine_closes_its_connection() {
    let origin = TestServer::spawn().await;
    origin.set("/hang", "late");
    // the origin holds the answer back, so the request is waiting when the
    // kill comes; the test ends far sooner than this
    origin.set_delay("/hang", Duration::from_secs(600));
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    tunnel
        .write_all(b"GET /hang HTTP/1.1\r\nHost: target.test\r\n\r\n")
        .await
        .unwrap();
    // the origin notes the request before it waits out the delay: this waits
    // for the request to have reached the upstream, not merely for the record
    // to exist — that one is there before the dial
    wait_until("the origin to hold the answer back", || {
        origin.requests().len() == 1
    })
    .await;
    let id = h.engine.request_log().active()[0].id;
    assert!(h.engine.kill(id));

    let mut back = Vec::new();
    let read = tokio::time::timeout(WITHIN, tunnel.read_to_end(&mut back)).await;
    assert!(read.is_ok(), "the kill ends the connection at once");
    assert!(
        back.is_empty(),
        "the client is let go, not answered: {:?}",
        String::from_utf8_lossy(&back)
    );

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(records[0].error.as_deref(), Some("killed"), "{records:?}");
    assert!(h.engine.request_log().active().is_empty());
}

/// An upgrade goes through every entry and is counted once (phase 4 M1 design
/// 8.9): after the switch, what is written and read is echoed, and the record
/// counts each byte once rather than twice.
#[tokio::test]
async fn a_websocket_goes_through_every_entry() {
    const PAYLOAD: usize = 8192;
    let origin = switching_echo().await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    // the sniffed connection
    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    tunnel
        .write_all(
            b"GET /ws HTTP/1.1\r\nHost: target.test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        )
        .await
        .unwrap();
    let head = read_head(&mut tunnel).await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    let sent = vec![b'w'; PAYLOAD];
    tunnel.write_all(&sent).await.unwrap();
    let mut back = vec![0u8; PAYLOAD];
    tokio::time::timeout(WITHIN, tunnel.read_exact(&mut back))
        .await
        .expect("the echo comes back within the bound")
        .unwrap();
    assert_eq!(back, sent);
    drop(tunnel);

    // the listener's absolute form
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(
        b"GET http://target.test/ws HTTP/1.1\r\nHost: target.test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
    )
    .await
    .unwrap();
    let head = read_head(&mut s).await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    s.write_all(&sent).await.unwrap();
    let mut back = vec![0u8; PAYLOAD];
    tokio::time::timeout(WITHIN, s.read_exact(&mut back))
        .await
        .expect("the echo comes back within the bound")
        .unwrap();
    assert_eq!(back, sent);
    drop(s);

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].listener, ListenerKind::Socks5, "{records:?}");
    assert_eq!(records[1].listener, ListenerKind::Http, "{records:?}");
    for record in &records {
        assert_eq!(record.status_code, Some(101), "{record:?}");
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
        // the head is small beside the payload, so a count made twice is at
        // least twice the payload
        assert!(
            record.up >= PAYLOAD as u64 && record.up < 2 * PAYLOAD as u64,
            "the bytes up were counted twice: {record:?}"
        );
        assert!(
            record.down >= PAYLOAD as u64 && record.down < 2 * PAYLOAD as u64,
            "the bytes down were counted twice: {record:?}"
        );
    }
}

/// One line of a chunked body, without its CRLF.
async fn chunk_line<R>(s: &mut R) -> String
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut line = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let n = s.read(&mut byte).await.expect("a readable stream");
        assert!(n > 0, "closed in the middle of a chunked body");
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
    }
    String::from_utf8_lossy(&line)
        .trim_end_matches('\r')
        .to_string()
}

/// A chunked body, whole, from the chunk sizes and the chunks that follow.
async fn read_chunked<R>(s: &mut R) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut body = Vec::new();
    loop {
        let line = chunk_line(s).await;
        let size = usize::from_str_radix(line.split(';').next().unwrap().trim(), 16)
            .unwrap_or_else(|_| panic!("no chunk size in {line:?}"));
        if size == 0 {
            // the trailer section ends with an empty line
            while !chunk_line(s).await.is_empty() {}
            return body;
        }
        let mut data = vec![0u8; size];
        s.read_exact(&mut data).await.expect("the chunk, whole");
        body.extend_from_slice(&data);
        assert_eq!(chunk_line(s).await, "", "the CRLF after a chunk");
    }
}

/// A loopback origin that reads a chunked request body and answers with the
/// same bytes in a chunked body of its own; it keeps every body it read.
async fn chunked_echo() -> (SocketAddr, Arc<std::sync::Mutex<Vec<Vec<u8>>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let seen: Arc<std::sync::Mutex<Vec<Vec<u8>>>> = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                break;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match s.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => head.push(byte[0]),
                    }
                }
                let body = read_chunked(&mut s).await;
                log.lock().unwrap().push(body.clone());
                if s.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                    .await
                    .is_err()
                {
                    return;
                }
                for piece in body.chunks(500) {
                    let head = format!("{:x}\r\n", piece.len());
                    if s.write_all(head.as_bytes()).await.is_err()
                        || s.write_all(piece).await.is_err()
                        || s.write_all(b"\r\n").await.is_err()
                    {
                        return;
                    }
                }
                let _ = s.write_all(b"0\r\n\r\n").await;
                let mut sink = [0u8; 1024];
                while matches!(s.read(&mut sink).await, Ok(n) if n > 0) {}
            });
        }
    });
    (addr, seen)
}

/// A chunked body goes both ways unchanged (phase 4 M1 design 8.6): the
/// origin receives the bytes the client sent, and the client the bytes the
/// origin answered with.
#[tokio::test]
async fn chunked_bodies_go_both_ways() {
    let (origin, seen) = chunked_echo().await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let body: Vec<u8> = (0..2049u32).map(|i| b'a' + (i % 26) as u8).collect();
    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    // head and body in one write (`CLAUDE.md`); the body in chunks of its own
    let mut sent =
        b"POST /echo HTTP/1.1\r\nHost: target.test\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for piece in body.chunks(313) {
        sent.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
        sent.extend_from_slice(piece);
        sent.extend_from_slice(b"\r\n");
    }
    sent.extend_from_slice(b"0\r\n\r\n");
    tunnel.write_all(&sent).await.unwrap();

    let head = read_head(&mut tunnel).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(
        head.to_ascii_lowercase()
            .contains("transfer-encoding: chunked"),
        "{head}"
    );
    let back = read_chunked(&mut tunnel).await;
    assert_eq!(back, body, "the client got its own bytes back, whole");
    drop(tunnel);

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Completed, "{records:?}");
    assert_eq!(records[0].status_code, Some(200), "{records:?}");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0], body, "the origin got the client's bytes, whole");
}

/// A loopback origin that answers the first request of a connection with a
/// 304 and writes a few bytes after it, as if a body followed a response that
/// should have none; after that it answers normally (`the phase 4 M6 recheck
/// report item 3`).
///
/// It reads one head at a time; the first request it ever reads gets the 304
/// and the stray bytes, every later one the 200.
async fn origin_with_a_body_after_a_304() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                break;
            };
            let n = n.clone();
            tokio::spawn(async move {
                loop {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        match s.read(&mut byte).await {
                            Ok(0) | Err(_) => return,
                            Ok(_) => head.push(byte[0]),
                        }
                    }
                    let first = n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
                    let answer: &[u8] = if first {
                        // no body, then a few bytes nobody asked for
                        b"HTTP/1.1 304 Not Modified\r\n\r\nstray"
                    } else {
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"
                    };
                    if s.write_all(answer).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

/// An origin that sends a body after a 304 does not poison the next request
/// of the same client connection (`the phase 4 M6 recheck report item 3`,
/// Surge's release note: requests failed with 503): the second request gets
/// the origin's 200 and its body, not a 502 or a closed connection.
#[tokio::test]
async fn a_body_after_a_304_does_not_poison_the_next_request() {
    let origin = origin_with_a_body_after_a_304().await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    let first = request(&mut tunnel, "target.test", "/first").await;
    assert!(first.starts_with("HTTP/1.1 304"), "{first}");
    let second = request(&mut tunnel, "target.test", "/second").await;
    assert!(second.starts_with("HTTP/1.1 200"), "{second}");
    assert!(second.ends_with("ok"), "{second}");
    drop(tunnel);

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].status_code, Some(304), "{records:?}");
    assert_eq!(records[1].status_code, Some(200), "{records:?}");
}

/// One request as `path_echo` read it: its path and its body.
type SeenPath = (String, Vec<u8>);

/// A loopback origin that reads one request at a time, keeps its path and its
/// body (chunked when the head says so), and answers the path as the body, so
/// a client can tell two answers apart (`the phase 4 M6 recheck report item
/// 9`).
async fn path_echo() -> (SocketAddr, Arc<std::sync::Mutex<Vec<SeenPath>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let seen: Arc<std::sync::Mutex<Vec<SeenPath>>> = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                break;
            };
            let log = log.clone();
            tokio::spawn(async move {
                loop {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        match s.read(&mut byte).await {
                            Ok(0) | Err(_) => return,
                            Ok(_) => head.push(byte[0]),
                        }
                    }
                    let text = String::from_utf8_lossy(&head).into_owned();
                    let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let chunked = text.to_ascii_lowercase().contains("chunked");
                    let body = if chunked {
                        read_chunked(&mut s).await
                    } else {
                        Vec::new()
                    };
                    log.lock().unwrap().push((path.clone(), body));
                    let answer = path.as_bytes();
                    let out = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                        answer.len()
                    );
                    if s.write_all(out.as_bytes()).await.is_err()
                        || s.write_all(answer).await.is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    (addr, seen)
}

/// A pipelined request sent right after a chunked body is its own request
/// (`the phase 4 M6 recheck report item 9`: Surge fixed such a request being
/// forwarded as part of the chunked body): each gets its answer, the origin
/// reads the chunked body whole and the second request as a separate `GET`,
/// and each request has a record.
#[tokio::test]
async fn a_pipelined_request_after_a_chunked_body_is_its_own_request() {
    let (origin, seen) = path_echo().await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    let upload: &[u8] = b"alpha-bravo-charlie";
    // head, the chunked body, and a pipelined `GET`, all in one write
    // (`CLAUDE.md`)
    let mut sent =
        b"POST /first HTTP/1.1\r\nHost: target.test\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for piece in upload.chunks(7) {
        sent.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
        sent.extend_from_slice(piece);
        sent.extend_from_slice(b"\r\n");
    }
    sent.extend_from_slice(b"0\r\n\r\n");
    sent.extend_from_slice(b"GET /second HTTP/1.1\r\nHost: target.test\r\n\r\n");
    tunnel.write_all(&sent).await.unwrap();

    let one = read_response(&mut tunnel).await;
    assert!(one.starts_with("HTTP/1.1 200"), "{one}");
    assert!(one.ends_with("/first"), "{one}");
    let two = read_response(&mut tunnel).await;
    assert!(two.starts_with("HTTP/1.1 200"), "{two}");
    assert!(two.ends_with("/second"), "{two}");
    drop(tunnel);

    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].0, "/first");
    assert_eq!(got[0].1, upload.to_vec(), "the chunked body, whole");
    assert_eq!(got[1].0, "/second");
    assert!(got[1].1.is_empty(), "the `GET` has no body: {:?}", got[1].1);

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].url.as_deref(), Some("http://target.test/first"));
    assert_eq!(records[1].url.as_deref(), Some("http://target.test/second"));
}

/// What an origin that reads an upload keeps of it: its request head's
/// fields, and the body.
type SeenUpload = (Vec<(String, String)>, Vec<u8>);

/// A loopback origin that reads a request with `Content-Length` and answers
/// 200, keeping every request body and head it read.
async fn upload_sink() -> (SocketAddr, Arc<std::sync::Mutex<Vec<SeenUpload>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let seen: Arc<std::sync::Mutex<Vec<SeenUpload>>> = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                break;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match s.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => head.push(byte[0]),
                    }
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let mut fields = Vec::new();
                for line in head.lines().skip(1) {
                    if let Some((name, value)) = line.split_once(':') {
                        fields.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
                    }
                }
                let length: usize = fields
                    .iter()
                    .find(|(name, _)| name == "content-length")
                    .and_then(|(_, value)| value.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0u8; length];
                if s.read_exact(&mut body).await.is_err() {
                    return;
                }
                log.lock().unwrap().push((fields, body));
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
                let mut sink = [0u8; 1024];
                while matches!(s.read(&mut sink).await, Ok(n) if n > 0) {}
            });
        }
    });
    (addr, seen)
}

/// `Expect: 100-continue` is answered by the engine itself (phase 4 M1 design
/// 8.6, `P23`): the client writes the head, reads the 100, then writes the
/// body, and the origin gets both the body and the `Expect` field. Of the
/// tests here, only this one writes a head before its body: `CLAUDE.md`'s and
/// `docs/testing.md`'s "one write" rule asks for a whole request in one write,
/// and this request is split on purpose, as the `a_late_body_does_not_cost_…`
/// group of `crates/rurge-api/tests/api.rs` splits one. A request the rules
/// refuse is answered before its body is read, so no 100 goes out first.
#[tokio::test]
async fn expect_100_continue_is_answered() {
    let (origin, seen) = upload_sink().await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            general: "show-error-page-for-reject = true",
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "URL-REGEX,^http://target\\.test/no,REJECT\nDOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let body = b"the body that waits for the go-ahead";
    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    tunnel
        .write_all(
            format!(
                "POST /up HTTP/1.1\r\nHost: target.test\r\nExpect: 100-continue\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let head = read_head(&mut tunnel).await;
    assert!(head.starts_with("HTTP/1.1 100 Continue"), "{head}");
    tunnel.write_all(body).await.unwrap();
    let answer = read_response(&mut tunnel).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    drop(tunnel);

    let uploads = seen.lock().unwrap().clone();
    assert_eq!(uploads.len(), 1, "{uploads:?}");
    assert_eq!(uploads[0].1, body, "the body arrived whole");
    assert_eq!(
        uploads[0]
            .0
            .iter()
            .find(|(name, _)| name == "expect")
            .map(|(_, value)| value.as_str()),
        Some("100-continue"),
        "the `Expect` field was forwarded: {:?}",
        uploads[0].0
    );

    // a refusal is decided before the body is read: the 403 comes first, and
    // no 100 with it
    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    tunnel
        .write_all(
            format!(
                "POST /no HTTP/1.1\r\nHost: target.test\r\nExpect: 100-continue\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let head = read_head(&mut tunnel).await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    assert!(!head.contains("100 Continue"), "{head}");
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(WITHIN, tunnel.read_to_end(&mut rest)).await;
    assert!(
        !String::from_utf8_lossy(&rest).contains("100 Continue"),
        "a 100 came after the refusal: {:?}",
        String::from_utf8_lossy(&rest)
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "the refused request uploaded nothing: only the first request is in the origin's list"
    );
}

/// A loopback origin that answers the first request of its first connection,
/// reads the next request and goes without answering it, and answers every
/// request of its later connections; it counts the connections it accepted.
async fn cut_off_origin() -> (SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
    slow_cut_off_origin(Duration::ZERO, Duration::ZERO).await
}

/// As `cut_off_origin`, with `hold` slept by the first connection after it
/// reads its second request and before it goes, and `delay` slept by every
/// later connection after it reads a request and before it answers `two`.
/// Both are skipped when zero.
async fn slow_cut_off_origin(
    hold: Duration,
    delay: Duration,
) -> (SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                break;
            };
            let n = count.fetch_add(1, Ordering::SeqCst) + 1;
            tokio::spawn(async move {
                let mut heads = 0;
                loop {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        match s.read(&mut byte).await {
                            Ok(0) | Err(_) => return,
                            Ok(_) => head.push(byte[0]),
                        }
                    }
                    heads += 1;
                    if n == 1 && heads == 2 {
                        // the request was read and no answer comes: the
                        // connection goes
                        if !hold.is_zero() {
                            tokio::time::sleep(hold).await;
                        }
                        return;
                    }
                    let answer = if n == 1 {
                        "HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\none"
                    } else {
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        "HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\ntwo"
                    };
                    if s.write_all(answer.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (addr, accepted)
}

/// A `GET` cut off after it was written, on a connection that had answered a
/// request before, is sent once more on a new connection (phase 4 M1 design
/// 8.6): the client gets its 200, and the origin accepted two connections.
#[tokio::test]
async fn an_idempotent_request_cut_off_is_sent_again_through_the_engine() {
    let (origin, accepted) = cut_off_origin().await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    let first = request(&mut tunnel, "target.test", "/a").await;
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    assert!(first.ends_with("one"), "{first}");

    // the second request goes out on the borrowed connection, which the
    // origin closes after reading it
    let second = request(&mut tunnel, "target.test", "/b").await;
    assert!(second.starts_with("HTTP/1.1 200"), "{second}");
    assert!(second.ends_with("two"), "{second}");
    drop(tunnel);

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    for record in &records {
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
        assert_eq!(record.status_code, Some(200), "{record:?}");
    }
    assert_eq!(
        accepted.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the cut-off request was sent again on a connection of its own"
    );
}

/// A `GET` sent again on a new connection gets its whole idle from that
/// connection (`phase 4 M1 design 8.6`, the phase 4 / M1b plan's deferred row
/// 4): the origin holds the first connection 1.5 s after reading the second
/// request before it goes, and holds the connection the request is sent again
/// on 1.5 s before it answers; the engine's idle is 2.5 s. The answer at about
/// 3.0 s is inside the 4.0 s that request's clock reaches and 1.0 s past the
/// 2.5 s it would have without `Progress::restart` in `retry`, which would
/// answer 504.
#[tokio::test]
async fn a_request_sent_again_waits_its_whole_idle_on_the_new_connection() {
    const IDLE: Duration = Duration::from_millis(2_500);
    const HOLD: Duration = Duration::from_millis(1_500);
    const DELAY: Duration = Duration::from_millis(1_500);
    let (origin, accepted) = slow_cut_off_origin(HOLD, DELAY).await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_timed(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
        IDLE,
    )
    .await;

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    let first = request(&mut tunnel, "target.test", "/a").await;
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    assert!(first.ends_with("one"), "{first}");

    // the borrowed connection is held and then goes; the request goes out
    // again on a connection of its own, where the clock starts over
    let sent = Instant::now();
    let second = request(&mut tunnel, "target.test", "/b").await;
    assert!(second.starts_with("HTTP/1.1 200"), "{second}");
    assert!(second.ends_with("two"), "{second}");
    assert!(
        sent.elapsed() > IDLE,
        "send to answer outlasts the whole idle: only a clock restarted on the new connection answers"
    );
    drop(tunnel);

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    for record in &records {
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
        assert_eq!(record.status_code, Some(200), "{record:?}");
    }
    assert_eq!(
        accepted.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the held request was sent again on a connection of its own"
    );
}

/// Every request that reaches the rules is counted, on every entry and
/// whether it was served or refused (phase 4 M1 design 11, the phase 4 / M1b
/// plan's P19): two on a sniffed connection, one refused on the listener.
#[tokio::test]
async fn the_requests_are_counted() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/b", "second");
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            // the refused name is pinned home too, so that a request which
            // escaped the rules could not leave the loopback
            general: "show-error-page-for-reject = true",
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            hosts: "target.test = 127.0.0.1\nads.test = 127.0.0.1",
            rules: "DOMAIN,target.test,A\nDOMAIN,ads.test,REJECT",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;
    let counts = || h.engine.http_counts().requests;
    assert_eq!(counts(), 0, "nothing is counted yet");

    let (reply, mut tunnel) = socks5_connect(h.socks(), "target.test", 80).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    let first = request(&mut tunnel, "target.test", "/a").await;
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    let second = request(&mut tunnel, "target.test", "/b").await;
    assert!(second.starts_with("HTTP/1.1 200"), "{second}");
    drop(tunnel);

    // a refused request on the listener reaches the rules as well: the
    // refusal of `REJECT` is the dial's (plan P3), after the route
    let refused = listener_get(&h, "http://ads.test/", "ads.test").await;
    assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");

    let records = the_records(&h, 3).await;
    assert_eq!(records.len(), 3, "{records:?}");
    assert_eq!(counts(), 3, "two served requests and one refused");
}
