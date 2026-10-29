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
