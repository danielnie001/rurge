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
async fn read_head(s: &mut TcpStream) -> String {
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
/// M1 design 8.3).
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
    let h = harness_with(
        Profile {
            proxies: &proxies,
            rules: "URL-REGEX,^http://target\\.test/a,A\nURL-REGEX,^http://target\\.test/b,B",
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
    assert_eq!(records[0].policy, ["A"], "{records:?}");
    assert_eq!(records[1].policy, ["B"], "{records:?}");
    assert_eq!(
        records[0].rule.as_deref(),
        Some("URL-REGEX,^http://target\\.test/a,A")
    );
    assert_eq!(
        records[1].rule.as_deref(),
        Some("URL-REGEX,^http://target\\.test/b,B")
    );
    assert_eq!(a.requests().len(), 1, "`A` is dialed by the first request");
    assert_eq!(b.requests().len(), 1, "`B` is dialed by the second");
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
    wait_until("the request to reach the rules", || {
        !rig.h.engine.request_log().active().is_empty()
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
    // the status of the answer (101) is one of the fields task 5 adds to the
    // record; what the record says today is that the upgrade completed
    assert_eq!(records[0].status, RecordStatus::Completed, "{records:?}");
}
