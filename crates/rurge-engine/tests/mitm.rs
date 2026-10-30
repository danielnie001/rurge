//! `[MITM]` in the engine (phase 4 M2 design 5.4, 7, 11): the authority a
//! generation decrypts with, the switch and `state.json`'s second version,
//! the sessions a list widens sniffing to, and the hand-over to the decrypted
//! HTTP engine.
//!
//! Nothing here goes beyond the loopback: the origins are TLS test servers on
//! loopback ports, the SOCKS5 upstreams are loopback servers, and the rules of
//! these tests send every session to one of them.

mod common;
use common::tun::*;
use common::*;
use rurge_config::rule::ProtocolKind;
use rurge_engine::MitmCounts;
use std::sync::Arc;

/// The `[MITM]` section of a profile: the hosts to decrypt, and the CA rurge
/// signs with as `ca-p12` with the password `pw`.
fn mitm_section(hostname: &str, ca_p12: &str) -> String {
    format!("[MITM]\nhostname = {hostname}\nca-p12 = {ca_p12}\nca-passphrase = pw")
}

/// A CA for rurge to sign with, and its trust anchors. Its subject is not the
/// origin fixture's, so that a client trusting only one of the two tells them
/// apart: webpki takes a certificate with the same subject for the issuer and
/// compares the signature, which fails as `BadSignature` rather than as
/// `UnknownIssuer` (the phase 4 / M2a plan's execution record row 27).
fn mitm_ca() -> Arc<TlsFixture> {
    TlsFixture::with_ca_name("rurge test MITM CA", &[])
}

/// A rustls client trusting `roots` and nothing else; `sni` is whether it
/// sends the server name extension.
fn client_config(
    roots: Arc<rustls::RootCertStore>,
    alpn: &[&[u8]],
    sni: bool,
) -> Arc<rustls::ClientConfig> {
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("the protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    config.enable_sni = sni;
    Arc::new(config)
}

/// The client side of a TLS handshake to `name` over any stream, bounded as
/// every TLS step of these tests is.
async fn handshake<S>(
    stream: S,
    config: Arc<rustls::ClientConfig>,
    name: &str,
) -> std::io::Result<tokio_rustls::client::TlsStream<S>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let name = rustls::pki_types::ServerName::try_from(name.to_string()).expect("a server name");
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio_rustls::TlsConnector::from(config).connect(name, stream),
    )
    .await
    .expect("the handshake ends within the bound")
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

/// The whole response at `s`, its body read by its `Content-Length`.
async fn read_response<S>(s: &mut S) -> String
where
    S: tokio::io::AsyncRead + Unpin,
{
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

/// One request over a decrypted connection, and the whole response.
async fn get<S>(s: &mut S, host: &str, path: &str) -> String
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    read_response(s).await
}

/// An engine that trusts `roots` for the origin side and sniffs for 300 ms, as
/// the tests of `sniff.rs` do.
async fn harness_mitm(p: Profile<'_>, roots: Arc<rustls::RootCertStore>) -> Harness {
    harness_with(
        p,
        EngineShared {
            roots: Some(roots),
            sniff_wait: Duration::from_millis(300),
            ..EngineShared::default()
        },
    )
    .await
}

/// The certificate of the leaf a decrypted connection was served over the
/// SOCKS5 listener to `origin.test:443`.
async fn leaf_of(h: &Harness, roots: &Arc<rustls::RootCertStore>) -> Vec<u8> {
    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let tls = handshake(
        tunnel,
        client_config(roots.clone(), &[], true),
        "origin.test",
    )
    .await
    .expect("the client trusts the MITM CA");
    tls.get_ref().1.peer_certificates().expect("a certificate")[0]
        .as_ref()
        .to_vec()
}

/// A listed host is decrypted: the client is served a certificate rurge signed
/// with `[MITM]`'s CA, its requests reach the origin over TLS, and the record
/// names the request an `https://` one.
#[tokio::test]
async fn a_listed_host_is_decrypted_through_socks5() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;
    assert_eq!(
        h.engine.mitm_ca_der().as_deref(),
        Some(mitm.ca_der()),
        "the certificate the API serves is the CA rurge signs with"
    );

    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(
        tunnel,
        client_config(mitm_roots.clone(), &[], true),
        "origin.test",
    )
    .await
    .expect("the client trusts the MITM CA, and the leaf names origin.test");
    let response = get(&mut tls, "origin.test", "/a").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("from the origin"), "{response}");

    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    assert_eq!(record.protocol, Some(ProtocolKind::Https), "{record:?}");
    assert_eq!(
        record.url.as_deref(),
        Some("https://origin.test/a"),
        "{record:?}"
    );
    assert_eq!(record.sni.as_deref(), Some("origin.test"), "{record:?}");
    assert_eq!(record.status_code, Some(200), "{record:?}");
}

/// With the switch off nothing is decrypted: a client trusting only the MITM
/// CA sees the origin's own certificate and refuses it, one trusting the
/// origin's CA is served as it always was, and switching back on decrypts the
/// next connection again (`phase 4 M2 design 11.1`).
#[tokio::test]
async fn with_the_switch_off_the_client_sees_the_origin_certificate() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;
    h.engine.set_mitm(false).await;

    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let Err(error) = handshake(
        tunnel,
        client_config(mitm_roots.clone(), &[], true),
        "origin.test",
    )
    .await
    else {
        panic!("with the switch off the origin's own certificate is served");
    };
    assert!(error.to_string().contains("UnknownIssuer"), "{error}");

    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(
        tunnel,
        client_config(origin.roots(), &[], true),
        "origin.test",
    )
    .await
    .expect("a client that trusts the origin's CA is served as before");
    let response = get(&mut tls, "origin.test", "/a").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    h.engine.set_mitm(true).await;
    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    handshake(tunnel, client_config(mitm_roots, &[], true), "origin.test")
        .await
        .expect("a new connection is decrypted again");
}

/// A client that offers only `h2` is passed through, and the one line of
/// DEBUG says why (`phase 4 M2 design 7.4`, the plan's P18).
#[tokio::test]
async fn a_client_offering_only_h2_is_passed_through() {
    let log = LogCapture::start();
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let Err(error) = handshake(
        tunnel,
        client_config(mitm_roots, &[&b"h2"[..]], true),
        "origin.test",
    )
    .await
    else {
        panic!("the client is passed through and sees the origin's certificate");
    };
    assert!(error.to_string().contains("UnknownIssuer"), "{error}");

    let lines = log.lines();
    let expected = "origin.test is listed for MITM but the client offers no protocol rurge decrypts (h2); passing it through";
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("DEBUG") && line.contains(expected)),
        "{lines:?}"
    );
}

/// A listed host whose `[MITM]` names a CA rurge cannot open: this generation
/// has no `Mitm`, so the session is passed through as it is, and the one line
/// of DEBUG says why (`phase 4 M2 design 7.1`, the plan's P18).
#[tokio::test]
async fn a_listed_host_without_a_usable_ca_is_passed_through() {
    let log = LogCapture::start();
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    // the CA is the right one, its passphrase is not: `W0039` says so at load
    // and this generation has no `Mitm`
    let section = format!(
        "[MITM]\nhostname = origin.test\nca-p12 = {}\nca-passphrase = wrong-pw",
        mitm.ca_p12_base64("pw")
    );
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &section,
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;
    assert!(
        h.engine.mitm_ca_der().is_none(),
        "a CA rurge cannot open leaves this generation without one"
    );

    // a client that trusts the origin's CA is served by it, as before
    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(
        tunnel,
        client_config(origin.roots(), &[], true),
        "origin.test",
    )
    .await
    .expect("the session is passed through to the origin");
    let response = get(&mut tls, "origin.test", "/a").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    let lines = log.lines();
    let expected =
        "origin.test is listed for MITM but this generation has no usable CA; passing it through";
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("DEBUG") && line.contains(expected)),
        "{lines:?}"
    );
}

/// Without an SNI the connection's target decides, by the port the list writes
/// down: 443 is decrypted with a leaf named after the target, while 8443 — a
/// port the settings force into sniffing — is passed through
/// (`phase 4 M2 design 7.3`).
#[tokio::test]
async fn without_sni_the_target_decides_on_443_only() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            general: "force-http-engine-hosts = origin.test:8443",
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    // 443: no SNI sent, and the leaf is the target's
    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(
        tunnel,
        client_config(mitm_roots.clone(), &[], false),
        "origin.test",
    )
    .await
    .expect("the target's name is what the leaf is issued for");
    let response = get(&mut tls, "origin.test", "/a").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    // 8443: a bare entry does not cover another port, so the origin answers
    let tunnel = connect_via_socks5(h.socks(), "origin.test", 8443).await;
    let Err(error) = handshake(tunnel, client_config(mitm_roots, &[], false), "origin.test").await
    else {
        panic!("8443 is passed through: the list writes down no port there");
    };
    assert!(error.to_string().contains("UnknownIssuer"), "{error}");
}

/// The profile of the tests that go straight to an origin's own port: the
/// name is pinned to the loopback address, and the rule dials it directly.
fn direct_profile<'a>(hosts: &'a str, rules: &'a str, section: &'a str) -> Profile<'a> {
    Profile {
        hosts,
        rules,
        sections: section,
        ..Profile::default()
    }
}

/// A port the list writes down makes that port sniffable and decrypted,
/// whatever the target is (`phase 4 M2 design 7.6`).
#[tokio::test]
async fn a_written_port_makes_its_port_sniffable() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let port = origin_addr(&server).port();
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let section = mitm_section(&format!("origin.test:{port}"), &ca);
    let h = harness_mitm(
        direct_profile(
            "origin.test = 127.0.0.1",
            "DOMAIN,origin.test,DIRECT",
            &section,
        ),
        origin.roots(),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "origin.test", port).await;
    let mut tls = handshake(tunnel, client_config(mitm_roots, &[], true), "origin.test")
        .await
        .expect("the written port is sniffed and decrypted");
    let response = get(&mut tls, "origin.test", "/a").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("from the origin"), "{response}");
}

/// A bare entry does not widen sniffing to another port: the session is
/// relayed as it is, and the client sees the origin's certificate
/// (`phase 4 M2 design 7.6`).
#[tokio::test]
async fn a_bare_entry_does_not_sniff_another_port() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let port = origin_addr(&server).port();
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let section = mitm_section("origin.test", &ca);
    let h = harness_mitm(
        direct_profile(
            "origin.test = 127.0.0.1",
            "DOMAIN,origin.test,DIRECT",
            &section,
        ),
        origin.roots(),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "origin.test", port).await;
    let Err(error) = handshake(tunnel, client_config(mitm_roots, &[], true), "origin.test").await
    else {
        panic!("a bare entry covers 443 only");
    };
    assert!(error.to_string().contains("UnknownIssuer"), "{error}");

    // the origin itself is what the client was talking to
    let tunnel = connect_via_socks5(h.socks(), "origin.test", port).await;
    let mut tls = handshake(
        tunnel,
        client_config(origin.roots(), &[], true),
        "origin.test",
    )
    .await
    .expect("a client that trusts the origin's CA is served");
    let response = get(&mut tls, "origin.test", "/a").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
}

/// `:0` widens sniffing to every port of the targets it matches, and to no
/// other target (`phase 4 M2 design 7.6`, the general design's corrected
/// 5.2).
#[tokio::test]
async fn a_zero_port_entry_widens_its_own_target_only() {
    let fixture = TlsFixture::new(&["origin.test", "other.test"]);
    let origin = TestServer::spawn_tls_with(fixture.acceptor(false)).await;
    origin.set("/a", "from the origin");
    let other = TestServer::spawn_tls_with(fixture.acceptor(false)).await;
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let section = mitm_section("origin.test:0", &ca);
    let h = harness_mitm(
        direct_profile(
            "origin.test = 127.0.0.1\nother.test = 127.0.0.1",
            "DOMAIN,origin.test,DIRECT\nDOMAIN,other.test,DIRECT",
            &section,
        ),
        fixture.roots(),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "origin.test", origin_addr(&origin).port()).await;
    let mut tls = handshake(
        tunnel,
        client_config(mitm_roots.clone(), &[], true),
        "origin.test",
    )
    .await
    .expect("its own target is decrypted at any port");
    let response = get(&mut tls, "origin.test", "/a").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    let tunnel = connect_via_socks5(h.socks(), "other.test", origin_addr(&other).port()).await;
    let Err(error) = handshake(tunnel, client_config(mitm_roots, &[], true), "other.test").await
    else {
        panic!("another target is not widened");
    };
    assert!(error.to_string().contains("UnknownIssuer"), "{error}");
}

/// The first bytes of a `ClientHello`, as a client sends them before it waits
/// for the answer.
fn client_hello(config: Arc<rustls::ClientConfig>, name: &str) -> Vec<u8> {
    let name = rustls::pki_types::ServerName::try_from(name.to_string()).expect("a server name");
    let mut conn = rustls::ClientConnection::new(config, name).expect("a client connection");
    let mut hello = Vec::new();
    conn.write_tls(&mut hello).expect("the hello is written");
    hello
}

/// A stop that lands while the handshake with the client is under way: the
/// engine has signed a leaf and answered the hello, and then the virtual
/// interface is stopped and the client let go (`phase 4 M2 design 8.2`). The
/// rig is left for the test to judge what the stop left behind.
async fn a_stop_in_the_middle_of_the_handshake() -> Rig {
    let origin = TlsFixture::new(&["origin.test"]);
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let section = mitm_section("origin.test", &ca);
    let rig = Rig::on(
        harness_mitm(
            Profile {
                rules: &guarded("DOMAIN,origin.test,DIRECT"),
                sections: &section,
                ..Profile::default()
            },
            origin.roots(),
        )
        .await,
    )
    .await;
    let (fake, _) = rig.fakes("origin.test");
    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, 443))
        .await
        .expect("the handshake completes");
    // one hello, and then nothing: the engine signs a leaf and waits
    tcp.write_all(&client_hello(
        client_config(mitm_roots, &[], true),
        "origin.test",
    ))
    .await
    .unwrap();
    // No origin is running for this test: the first bytes back can only be
    // rurge's own answer to the hello, which means its `accept` has signed a
    // leaf, sent its ServerHello and is waiting for the client's next step.
    let mut first = [0u8; 1];
    let read = tokio::time::timeout(WITHIN, tcp.read(&mut first))
        .await
        .expect("the engine answers the ClientHello within the bound")
        .expect("the session carries the engine's answer");
    assert_eq!(read, 1, "the engine answered before the stop");
    assert_eq!(first[0], 0x16, "a TLS handshake record");

    rig.tun.stop();
    // the client is let go at once: the rest of the handshake is read off and
    // then the stream ends, all of it within one bound
    let drain = async {
        let mut rest = [0u8; 1024];
        loop {
            match tcp.read(&mut rest).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    };
    tokio::time::timeout(WITHIN, drain)
        .await
        .expect("the stop ends the handshake at once");
    rig
}

/// A stop that lands while the handshake with the client is under way leaves
/// no record: the connection had no handle to finish (`phase 4 M2 design
/// 8.2`).
#[tokio::test]
async fn a_stop_during_the_handshake_leaves_no_record() {
    let rig = a_stop_in_the_middle_of_the_handshake().await;

    // waits for a record that must not come: the stop's branch returns at
    // once, so a record it wrote would be there before it returned — 300 ms is
    // a wide margin on a branch that has nothing left to do but return
    tokio::time::sleep(Duration::from_millis(300)).await;
    let records = rig.h.engine.request_log().recent(1000);
    assert!(records.is_empty(), "{records:?}");
}

/// The same stop counts nothing either: a handshake the stop ended is neither
/// decrypted, nor failed, nor a hint about the host (`phase 4 M2 design 8.5`,
/// the plan's P21).
#[tokio::test]
async fn a_stop_during_the_handshake_counts_nothing() {
    let rig = a_stop_in_the_middle_of_the_handshake().await;

    // the same wait as the record's: a count the stop's branch wrote would be
    // there before it returned, and 300 ms is a wide margin on that
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        rig.h.engine.http_counts().mitm,
        MitmCounts::default(),
        "a stop's handshake is neither decrypted nor failed"
    );
}

/// The lines of `lines` at `level` that carry `sentence` whole, in the order
/// they were written: a line is matched by its level and by the whole text of
/// the plan's P21 sentence, so a misspelling anywhere in one is a red test.
fn lines_with(lines: &[String], level: &str, sentence: &str) -> Vec<String> {
    lines
        .iter()
        .filter(|line| line.starts_with(level) && line.contains(sentence))
        .cloned()
        .collect()
}

// The sentences of the plan's P21 with `origin.test` in them, as a session
// through a listener has them (`phase 4 M2 design 8.2`, `8.3`, `15`).
const UNTRUSTED_NAMED: &str = "the client does not trust the certificate rurge issued for origin.test: trust rurge's MITM CA on the client, or exclude the host with -origin.test in [MITM] hostname";
const UNTRUSTED_VIRTUAL: &str = "the client does not trust the certificate rurge issued for a host reached through the virtual interface (the name is in the DEBUG log and the request record): trust rurge's MITM CA on the client, or exclude the host in [MITM] hostname";
const PINNING_NAMED: &str = "origin.test is likely protected by certificate pinning: the client finished the TLS handshake and closed without a request; exclude it with -origin.test in [MITM] hostname";
const PINNING_VIRTUAL: &str = "a host reached through the virtual interface is likely protected by certificate pinning (the name is in the DEBUG log and the request record): the client finished the TLS handshake and closed without a request";
/// The line of a handshake that failed for another reason, up to its reason.
const FAILED_NAMED: &str = "the TLS handshake with the client failed for origin.test: ";
const FAILED_VIRTUAL: &str =
    "the TLS handshake with the client failed for a host reached through the virtual interface: ";

/// A client that does not trust the certificate rurge issued is refused and
/// counted every time, and warned about once an hour: the second connection's
/// line is DEBUG only (`phase 4 M2 design 8.2`, `8.4`; the plan's P21, `P22`).
#[tokio::test]
async fn an_untrusting_client_is_warned_once_an_hour_and_counted_each_time() {
    let log = LogCapture::start();
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let ca = mitm.ca_p12_base64("pw");
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    // the client trusts the origin's CA only: the leaf rurge signed with the
    // MITM CA is an unknown issuer to it
    let untrusting = client_config(origin.roots(), &[], true);
    for _ in 0..2 {
        let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
        let Err(error) = handshake(tunnel, untrusting.clone(), "origin.test").await else {
            panic!("rurge signs its leaf with the MITM CA");
        };
        assert!(error.to_string().contains("UnknownIssuer"), "{error}");
    }
    let records = the_records(&h, 2).await;
    for record in &records {
        assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
        assert_eq!(
            record.error.as_deref(),
            Some("the client does not trust the certificate rurge issued (alert unknown_ca)"),
            "{record:?}"
        );
    }

    let lines = log.lines();
    let warned = lines_with(&lines, "WARN", UNTRUSTED_NAMED);
    assert_eq!(warned.len(), 1, "{lines:?}");
    assert!(warned[0].contains("port=443"), "{warned:?}");
    assert!(warned[0].contains("alert=unknown_ca"), "{warned:?}");
    // the second time the same words are at DEBUG, and they name the host
    let quiet = lines_with(&lines, "DEBUG", UNTRUSTED_NAMED);
    assert_eq!(quiet.len(), 1, "{lines:?}");
    assert!(quiet[0].contains("port=443"), "{quiet:?}");
    assert!(quiet[0].contains("alert=unknown_ca"), "{quiet:?}");
    assert_eq!(
        h.engine.http_counts().mitm,
        MitmCounts {
            untrusted: 2,
            ..MitmCounts::default()
        },
        "two clients were refused, and nothing was decrypted"
    );
}

/// A client that finishes the handshake and closes without a request is what
/// a host behind certificate pinning looks like: a WARN once an hour, a count
/// every time, and the record says so (`phase 4 M2 design 8.3`, `8.4`).
#[tokio::test]
async fn a_client_that_closes_after_the_handshake_gets_the_pinning_hint() {
    let log = LogCapture::start();
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    for _ in 0..2 {
        let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
        let mut tls = handshake(
            tunnel,
            client_config(mitm_roots.clone(), &[], true),
            "origin.test",
        )
        .await
        .expect("the client trusts the MITM CA");
        // the handshake is done and no request follows: what a client that
        // pins its host's certificate does
        tls.shutdown().await.expect("the close_notify goes out");
        let mut rest = Vec::new();
        tokio::time::timeout(WITHIN, tls.read_to_end(&mut rest))
            .await
            .expect("the engine lets go within the bound")
            .expect("the stream ends cleanly");
    }
    let records = the_records(&h, 2).await;
    for record in &records {
        assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
        assert_eq!(
            record.error.as_deref(),
            Some("no request after the TLS handshake (likely certificate pinning)"),
            "{record:?}"
        );
    }

    let lines = log.lines();
    let warned = lines_with(&lines, "WARN", PINNING_NAMED);
    assert_eq!(warned.len(), 1, "{lines:?}");
    assert!(warned[0].contains("port=443"), "{warned:?}");
    let quiet = lines_with(&lines, "DEBUG", PINNING_NAMED);
    assert_eq!(
        quiet.len(),
        1,
        "the second hint about the same host is DEBUG only: {lines:?}"
    );
    assert!(quiet[0].contains("port=443"), "{quiet:?}");
    assert_eq!(
        h.engine.http_counts().mitm,
        MitmCounts {
            decrypted: 2,
            no_request: 2,
            ..MitmCounts::default()
        },
        "both clients finished the handshake and neither sent a request"
    );
}

/// A client that finishes the handshake and then says nothing at all is not a
/// client that pins: the wait for the request head runs out, and that is no
/// hint (`phase 4 M2 design 8.3`).
#[tokio::test]
async fn a_request_head_that_times_out_is_no_pinning_hint() {
    let log = LogCapture::start();
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_timed(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        EngineShared {
            roots: Some(origin.roots()),
            sniff_wait: Duration::from_millis(300),
            ..EngineShared::default()
        },
        Duration::from_millis(300),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let tls = handshake(tunnel, client_config(mitm_roots, &[], true), "origin.test")
        .await
        .expect("the client trusts the MITM CA");
    // nothing is sent and nothing is closed: the wait for the request head is
    // what ends the connection, and the stream is held until it does
    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    let reason = record.error.as_deref().unwrap_or_default();
    assert!(
        !reason.is_empty(),
        "hyper says why the connection ended: {record:?}"
    );
    assert_ne!(
        reason, "no request after the TLS handshake (likely certificate pinning)",
        "a head that timed out is no pinning hint"
    );
    assert_eq!(
        h.engine.http_counts().mitm,
        MitmCounts {
            decrypted: 1,
            ..MitmCounts::default()
        },
        "the handshake is counted, and the ending is neither a hint nor a failure"
    );
    let lines = log.lines();
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("certificate pinning")),
        "the hint is in no line of any level: {lines:?}"
    );
    drop(tls);
}

/// A client that sends a hello and then leaves is a failed handshake, said at
/// INFO and counted as such (`phase 4 M2 design 8.2`, the plan's P21).
#[tokio::test]
async fn a_handshake_the_client_abandons_is_failed_at_info() {
    let log = LogCapture::start();
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    // a hello and then the end of the stream: the engine is left waiting for
    // the client's next step of the handshake
    let mut stream = connect_via_socks5(h.socks(), "origin.test", 443).await;
    stream
        .write_all(&client_hello(
            client_config(mitm_roots, &[], true),
            "origin.test",
        ))
        .await
        .unwrap();
    stream.shutdown().await.unwrap();

    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    assert!(
        record
            .error
            .as_deref()
            .unwrap_or_default()
            .starts_with("TLS handshake with the client failed: "),
        "{record:?}"
    );
    let lines = log.lines();
    let info = lines_with(&lines, "INFO", FAILED_NAMED);
    assert_eq!(info.len(), 1, "{lines:?}");
    assert!(info[0].contains("port=443"), "{info:?}");
    assert_eq!(
        h.engine.http_counts().mitm,
        MitmCounts {
            failed: 1,
            ..MitmCounts::default()
        },
        "the handshake failed and nothing was decrypted"
    );
}

/// The alert a client that does not trust the server's certificate ends its
/// handshake with: fatal (2), `unknown_ca` (`0x30`). The test writes it
/// itself in the clear, and rustls 0.23.43 reads a clear alert here: in TLS
/// 1.3 it takes `ContentType::Alert` messages in the clear while it has not
/// decrypted anything from the peer yet and the payload is at most two bytes
/// (`conn.rs`), and a client whose verification failed is exactly there — it
/// never sent its encrypted Finished. It is also why tokio-rustls matters
/// only for the timing: when verification fails it writes the alert it
/// queued once, in `read_io`'s last-gasp `write_io`, and drops it if the
/// socket will not take it at once; and an app socket dropped with data still
/// unsent resets the session, so the engine would see a reset rather than the
/// client's `unknown_ca`.
const UNKNOWN_CA_ALERT: [u8; 7] = [0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x30];

/// Through the virtual interface the lines of WARN and INFO name neither the
/// host nor its port: the host is in a line of DEBUG after each of them, and
/// the records keep it (`phase 4 M2 design 8.2`, `15`; stage 3 `M3-D26`).
#[tokio::test]
async fn through_the_virtual_interface_the_warnings_name_no_host() {
    let log = LogCapture::start();
    let origin = TlsFixture::new(&["origin.test"]);
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let section = mitm_section("origin.test", &ca);
    let rig = Rig::on(
        harness_mitm(
            Profile {
                rules: &guarded("DOMAIN,origin.test,DIRECT"),
                sections: &section,
                ..Profile::default()
            },
            origin.roots(),
        )
        .await,
    )
    .await;

    // a client that does not trust the MITM CA. Its alert must reach the
    // engine before its socket goes: an app socket that closes with data
    // still unsent resets the session
    let (fake, _) = rig.fakes("origin.test");
    let tcp = rig
        .app
        .connect(SocketAddr::new(fake, 443))
        .await
        .expect("the session is accepted");
    let name = rustls::pki_types::ServerName::try_from("origin.test".to_string()).unwrap();
    let failed = tokio::time::timeout(
        WITHIN,
        tokio_rustls::TlsConnector::from(client_config(origin.roots(), &[], true))
            .connect(name, tcp)
            .into_fallible(),
    )
    .await
    .expect("the handshake ends within the bound");
    let (error, mut stream) = failed.expect_err("rurge signs its leaf with the MITM CA");
    assert!(error.to_string().contains("UnknownIssuer"), "{error}");
    // the alert the client's handshake ended with, written by the test: see
    // `UNKNOWN_CA_ALERT`
    stream.write_all(&UNKNOWN_CA_ALERT).await.unwrap();
    let first = the_records(&rig.h, 1).await;
    assert_eq!(first[0].status, RecordStatus::Failed, "{first:?}");
    assert_eq!(
        first[0].error.as_deref(),
        Some("the client does not trust the certificate rurge issued (alert unknown_ca)"),
        "{first:?}"
    );
    drop(stream);

    // and one that trusts it and leaves without a request
    let (fake, _) = rig.fakes("origin.test");
    let tcp = rig
        .app
        .connect(SocketAddr::new(fake, 443))
        .await
        .expect("the session is accepted");
    let mut tls = handshake(
        tcp,
        client_config(mitm_roots.clone(), &[], true),
        "origin.test",
    )
    .await
    .expect("the client trusts the MITM CA");
    tls.shutdown().await.expect("the close_notify goes out");
    let mut rest = Vec::new();
    tokio::time::timeout(WITHIN, tls.read_to_end(&mut rest))
        .await
        .expect("the engine lets go within the bound")
        .expect("the stream ends cleanly");

    // and one that sends a hello and leaves: a handshake that failed for
    // another reason is said at INFO, and that line names no host either
    let (fake, _) = rig.fakes("origin.test");
    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, 443))
        .await
        .expect("the session is accepted");
    tcp.write_all(&client_hello(
        client_config(mitm_roots, &[], true),
        "origin.test",
    ))
    .await
    .unwrap();
    // the engine's answer to the hello means the sniff is over and `accept`
    // waits for the client's next step, as in
    // `a_stop_in_the_middle_of_the_handshake`
    let mut first = [0u8; 1];
    let read = tokio::time::timeout(WITHIN, tcp.read(&mut first))
        .await
        .expect("the engine answers the ClientHello within the bound")
        .expect("the session carries the engine's answer");
    assert_eq!(read, 1, "the engine answered before the close");
    assert_eq!(first[0], 0x16, "a TLS handshake record");
    tcp.shutdown()
        .await
        .expect("the end of the stream goes out");
    let records = the_records(&rig.h, 3).await;
    // the socket goes only now: shut down, it ends the session without a
    // reset, and the record above is the engine's own end of it
    drop(tcp);

    assert_eq!(records.len(), 3, "{records:?}");
    for record in &records {
        assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
        assert!(record.dst.contains("origin.test"), "{record:?}");
    }
    let abandoned = "TLS handshake with the client failed: ";
    assert_eq!(
        records
            .iter()
            .filter(|record| record
                .error
                .as_deref()
                .unwrap_or_default()
                .starts_with(abandoned))
            .count(),
        1,
        "the third client's record is the abandoned handshake: {records:?}"
    );

    let lines = log.lines();
    let untrusted_warn = lines_with(&lines, "WARN", UNTRUSTED_VIRTUAL);
    assert_eq!(untrusted_warn.len(), 1, "{lines:?}");
    let pinning_warn = lines_with(&lines, "WARN", PINNING_VIRTUAL);
    assert_eq!(pinning_warn.len(), 1, "{lines:?}");
    let failed_info = lines_with(&lines, "INFO", FAILED_VIRTUAL);
    assert_eq!(failed_info.len(), 1, "{lines:?}");
    for line in untrusted_warn
        .iter()
        .chain(pinning_warn.iter())
        .chain(failed_info.iter())
    {
        assert!(!line.contains("origin.test"), "{line}");
        assert!(!line.contains("port="), "{line}");
    }
    // each of the three names the host in a line of DEBUG, and none of those
    // lines has a port: `port` is only on a session through a listener
    for sentence in [UNTRUSTED_NAMED, PINNING_NAMED, FAILED_NAMED] {
        let named = lines_with(&lines, "DEBUG", sentence);
        assert_eq!(named.len(), 1, "{sentence}: {lines:?}");
        assert!(!named[0].contains("port="), "{named:?}");
    }
    assert_eq!(
        rig.h.engine.http_counts().mitm,
        MitmCounts {
            decrypted: 1,
            untrusted: 1,
            failed: 1,
            no_request: 1,
        },
        "each client ended its own way"
    );
}

/// A stop that lands after the handshake is no hint about certificate
/// pinning: the client did finish the handshake and sent nothing, and the
/// stop is what ended the connection (`phase 4 M2 design 8.3`, the plan's
/// P21).
#[tokio::test]
async fn a_stop_after_the_handshake_is_no_pinning_hint() {
    let log = LogCapture::start();
    let origin = TlsFixture::new(&["origin.test"]);
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let section = mitm_section("origin.test", &ca);
    let rig = Rig::on(
        harness_mitm(
            Profile {
                rules: &guarded("DOMAIN,origin.test,DIRECT"),
                sections: &section,
                ..Profile::default()
            },
            origin.roots(),
        )
        .await,
    )
    .await;

    let (fake, _) = rig.fakes("origin.test");
    let tcp = rig
        .app
        .connect(SocketAddr::new(fake, 443))
        .await
        .expect("the session is accepted");
    let mut tls = handshake(tcp, client_config(mitm_roots, &[], true), "origin.test")
        .await
        .expect("the client trusts the MITM CA");
    // The client's side is done once it sent its Finished; the engine's is
    // done when `accept` returned, which is what counts the handshake. The
    // stop has to land after that, or it is the stop of `8.2` that leaves
    // nothing behind at all.
    wait_until("the engine to finish its handshake", || {
        rig.h.engine.http_counts().mitm.decrypted == 1
    })
    .await;

    rig.tun.stop();
    // the client is let go at once: the stop resets the session on the app's
    // side of the adapter, as it does for an HTTP connection of the adapter
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(WITHIN, tls.read(&mut byte))
        .await
        .expect("the stop ends the connection at once");
    assert!(
        matches!(read, Ok(0) | Err(_)),
        "the app is let go: {read:?}"
    );

    let records = the_records(&rig.h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_ne!(
        records[0].error.as_deref(),
        Some("no request after the TLS handshake (likely certificate pinning)"),
        "a stop is no pinning hint: {records:?}"
    );
    assert_eq!(
        rig.h.engine.http_counts().mitm,
        MitmCounts {
            decrypted: 1,
            ..MitmCounts::default()
        },
        "the handshake is counted, and the ending is neither a hint nor a failure"
    );
    let lines = log.lines();
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("certificate pinning")),
        "the hint is in no line of any level: {lines:?}"
    );
}

/// A reload with the same material keeps the authority, and with it the
/// leaves it cached: the second connection is served the very certificate the
/// first one was (`phase 4 M2 design 5.4`).
#[tokio::test]
async fn a_reload_with_the_same_material_keeps_the_leaves() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    let first = leaf_of(&h, &mitm_roots).await;
    let before = h
        .engine
        .shared()
        .mitm
        .last_live()
        .expect("the first generation published its authority");
    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    h.engine
        .swap_runtime(runtime(h.dir.path(), &text, h.engine.shared()).await);
    let after = h
        .engine
        .shared()
        .mitm
        .last_live()
        .expect("the new generation published one as well");
    assert!(
        Arc::ptr_eq(&before, &after),
        "the same material keeps the same authority"
    );

    let second = leaf_of(&h, &mitm_roots).await;
    assert_eq!(first, second, "the authority's cached leaf is the same one");
}

/// The MITM counts stand across a reload: they belong to the engine, not to a
/// generation (`phase 4 M2 design 8.5`, the plan's P23).
#[tokio::test]
async fn the_mitm_counts_survive_a_reload() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(tunnel, client_config(mitm_roots, &[], true), "origin.test")
        .await
        .expect("the client trusts the MITM CA");
    let response = get(&mut tls, "origin.test", "/a").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    the_records(&h, 1).await;
    let decrypted = MitmCounts {
        decrypted: 1,
        ..MitmCounts::default()
    };
    assert_eq!(h.engine.http_counts().mitm, decrypted);

    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    h.engine
        .swap_runtime(runtime(h.dir.path(), &text, h.engine.shared()).await);
    assert_eq!(
        h.engine.http_counts().mitm,
        decrypted,
        "a reload does not clear the counts"
    );
}

/// The switch is kept in `state.json` as a second-version file, and a start
/// that reads the file puts it back — without writing the file itself
/// (`phase 4 M2 design 11.1`, `11.2`, the plan's P20).
#[tokio::test]
async fn the_switch_is_restored_from_state_json() {
    let mitm = mitm_ca();
    let (ca, roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let section = mitm_section("origin.test", &ca);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(STATE_FILE);
    let (store, _) = StateStore::open(path.clone()).await;
    let h = harness_mitm(
        Profile {
            sections: &section,
            ..Profile::default()
        },
        roots.clone(),
    )
    .await;
    h.engine.attach_state(store);
    h.engine.set_mitm(false).await;

    let text = std::fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["version"], serde_json::json!(2), "{text}");
    assert_eq!(json["features"]["mitm"], serde_json::json!(false), "{text}");

    let (restored, state) = StateStore::open(path.clone()).await;
    assert_eq!(state.features.mitm, Some(false));
    let other = harness_mitm(
        Profile {
            sections: &section,
            ..Profile::default()
        },
        roots,
    )
    .await;
    other.engine.attach_state(restored);
    assert!(other.engine.mitm_on(), "the switch is on unless set");
    let before = std::fs::read(&path).unwrap();
    other.engine.restore_switches(&state.features);
    assert!(!other.engine.mitm_on(), "the file's word is taken");
    let after = std::fs::read(&path).unwrap();
    assert_eq!(before, after, "a start does not write state.json");
}

// ---- end to end through every entry

/// Two requests on one decrypted connection, each answered 200.
async fn two_200s<S>(s: &mut S, host: &str, paths: [&str; 2])
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    for path in paths {
        let response = get(s, host, path).await;
        assert!(response.starts_with("HTTP/1.1 200"), "{path}: {response}");
    }
}

/// A TLS origin that answers a request head with 101 and then echoes every
/// byte it is sent: the server side of a WebSocket upgrade, which `TestServer`
/// does not do (`crates/rurge-engine/tests/http.rs`'s `switching_echo`, over
/// TLS).
async fn switching_echo_over_tls(origin: &Arc<TlsFixture>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let acceptor = origin.acceptor(false);
    let fixture = origin.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                break;
            };
            let (acceptor, fixture) = (acceptor.clone(), fixture.clone());
            tokio::spawn(async move {
                let Ok(mut s) = fixture.accept(&acceptor, tcp).await else {
                    return;
                };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if s.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let switched = b"HTTP/1.1 101 Switching Protocols\r\nconnection: upgrade\r\n\
                                 upgrade: websocket\r\n\r\n";
                if s.write_all(switched).await.is_err() || s.flush().await.is_err() {
                    return;
                }
                let mut buf = vec![0u8; 8192];
                loop {
                    match s.read(&mut buf).await {
                        Ok(0) => {
                            // the echo is over from the client's side: the TLS
                            // ending goes out before the stream is let go, or
                            // the relay reads an `UnexpectedEof` and stops the
                            // other direction too (the phase 4 / M2a plan's
                            // execution record row 39)
                            let _ = s.shutdown().await;
                            break;
                        }
                        Ok(n) => {
                            if s.write_all(&buf[..n]).await.is_err() || s.flush().await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }
    });
    addr
}

/// Every entry into the engine decrypts a listed host: a SOCKS5 tunnel, a
/// `CONNECT` through the HTTP listener and the virtual interface each bring
/// two requests of one connection in, and each request is a record of its own
/// with the entry's listener, `https`, and the method, URL and status the
/// client saw (`phase 4 M2 design 7.1`, `9.1`, `17`).
#[tokio::test]
async fn every_entry_decrypts_a_listed_host() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "first");
    server.set("/b", "second");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let rig = Rig::on(
        harness_mitm(
            Profile {
                proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
                hosts: "origin.test = 127.0.0.1",
                rules: &guarded("DOMAIN,origin.test,A"),
                sections: &mitm_section("origin.test", &ca),
                ..Profile::default()
            },
            origin.roots(),
        )
        .await,
    )
    .await;
    let h = &rig.h;

    // what one entry's two records must say
    let two = |records: &[rurge_engine::RequestRecord], at: usize, listener, first: &str| {
        for (i, path) in [first, "/b"].iter().enumerate() {
            let record = &records[at + i];
            assert_eq!(record.listener, listener, "{record:?}");
            assert_eq!(record.protocol, Some(ProtocolKind::Https), "{record:?}");
            assert_eq!(record.sni.as_deref(), Some("origin.test"), "{record:?}");
            assert_eq!(record.method.as_deref(), Some("GET"), "{record:?}");
            assert_eq!(
                record.url.as_deref(),
                Some(format!("https://origin.test{path}").as_str()),
                "{record:?}"
            );
            assert_eq!(record.status_code, Some(200), "{record:?}");
            assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
        }
    };

    // a sniffed SOCKS5 tunnel to 443
    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(
        tunnel,
        client_config(roots.clone(), &[], true),
        "origin.test",
    )
    .await
    .expect("the client trusts the MITM CA");
    two_200s(&mut tls, "origin.test", ["/a", "/b"]).await;
    drop(tls);
    two(&the_records(h, 2).await, 0, ListenerKind::Socks5, "/a");

    // a `CONNECT` through the HTTP listener, sniffed after its answer
    let tunnel = connect_via_http(h.http(), "origin.test:443").await;
    let mut tls = handshake(
        tunnel,
        client_config(roots.clone(), &[], true),
        "origin.test",
    )
    .await
    .expect("the client trusts the MITM CA");
    two_200s(&mut tls, "origin.test", ["/a", "/b"]).await;
    drop(tls);
    two(&the_records(h, 4).await, 2, ListenerKind::Http, "/a");

    // the virtual interface: the app's own TLS runs on the decrypted stream
    let (fake, _) = rig.fakes("origin.test");
    let tcp = rig
        .app
        .connect(SocketAddr::new(fake, 443))
        .await
        .expect("the handshake completes");
    let mut tls = handshake(tcp, client_config(roots, &[], true), "origin.test")
        .await
        .expect("the client trusts the MITM CA");
    two_200s(&mut tls, "origin.test", ["/a", "/b"]).await;
    // the app ends the session from its own side, without a reset
    tls.shutdown().await.expect("the close_notify goes out");
    let mut rest = Vec::new();
    tokio::time::timeout(WITHIN, tls.read_to_end(&mut rest))
        .await
        .expect("the engine lets go within the bound")
        .expect("the stream ends cleanly");
    two(&the_records(h, 6).await, 4, ListenerKind::Tun, "/a");
}

/// The two requests of one decrypted connection are routed one by one: the
/// first is refused by its `URL-REGEX` and answered in TLS with the reject
/// page, the second goes to the origin (`phase 4 M2 design 9.1`).
#[tokio::test]
async fn two_requests_of_one_decrypted_connection_follow_their_own_rules() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/ok", "from the origin");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            general: "show-error-page-for-reject = true",
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "URL-REGEX,^https://origin\\.test/blocked,REJECT\nDOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(tunnel, client_config(roots, &[], true), "origin.test")
        .await
        .expect("the client trusts the MITM CA");
    let blocked = get(&mut tls, "origin.test", "/blocked").await;
    assert!(blocked.starts_with("HTTP/1.1 403"), "{blocked}");
    assert!(
        blocked.contains("<code>URL-REGEX,^https://origin\\.test/blocked,REJECT</code>"),
        "the reject page names the rule: {blocked}"
    );
    let ok = get(&mut tls, "origin.test", "/ok").await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.ends_with("from the origin"), "{ok}");

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(
        records[0].rule.as_deref(),
        Some("URL-REGEX,^https://origin\\.test/blocked,REJECT"),
        "{records:?}"
    );
    assert_eq!(records[0].status_code, Some(403), "{records:?}");
    assert_eq!(
        records[1].rule.as_deref(),
        Some("DOMAIN,origin.test,A"),
        "{records:?}"
    );
    assert_eq!(records[1].status_code, Some(200), "{records:?}");
    assert_eq!(records[1].status, RecordStatus::Completed, "{records:?}");
}

/// An origin whose CA the engine was not given is a 502, and the record says
/// why; `skip-server-cert-verify = true` lets the same request through
/// (`phase 4 M2 design 9.2`, `9.3`).
#[tokio::test]
async fn an_untrusted_origin_is_a_502_and_skip_server_cert_verify_lets_it_through() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("A = socks5, 127.0.0.1, {}", upstream.addr().port());
    let mitm = mitm_ca();
    let (ca, roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    // the engine is given no roots: the origin's CA is not one the operating
    // system's store or the built-in list holds
    let h = harness_with(
        Profile {
            proxies: &proxies,
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        EngineShared {
            sniff_wait: Duration::from_millis(300),
            ..EngineShared::default()
        },
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(
        tunnel,
        client_config(roots.clone(), &[], true),
        "origin.test",
    )
    .await
    .expect("the client trusts the MITM CA");
    let failed = get(&mut tls, "origin.test", "/a").await;
    assert!(failed.starts_with("HTTP/1.1 502"), "{failed}");
    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(records[0].status_code, Some(502), "{records:?}");
    assert!(
        records[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("UnknownIssuer"),
        "the reason is the certificate's: {records:?}"
    );

    // the same request with the origin's chain and name left unchecked
    let section = format!(
        "{}\nskip-server-cert-verify = true",
        mitm_section("origin.test", &ca)
    );
    let h = harness_with(
        Profile {
            proxies: &proxies,
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &section,
            ..Profile::default()
        },
        EngineShared {
            sniff_wait: Duration::from_millis(300),
            ..EngineShared::default()
        },
    )
    .await;
    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(tunnel, client_config(roots, &[], true), "origin.test")
        .await
        .expect("the client trusts the MITM CA");
    let passed = get(&mut tls, "origin.test", "/a").await;
    assert!(passed.starts_with("HTTP/1.1 200"), "{passed}");
    assert!(passed.ends_with("from the origin"), "{passed}");
}

/// Three ways a listed host is still not decrypted — an exclusion, a disabled
/// entry, and a source the list does not take — and in each the client sees
/// the origin's own certificate (`phase 4 M2 design 4.2`, `4.3`, `7.1`).
#[tokio::test]
async fn exclusions_disabled_entries_and_sources_pass_through() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("A = socks5, 127.0.0.1, {}", upstream.addr().port());
    let mitm = mitm_ca();
    let ca = mitm.ca_p12_base64("pw");
    // each case is named rather than printed: the section carries the CA's
    // PKCS#12, private key and all
    let cases = [
        (
            "an exclusion",
            format!("[MITM]\nhostname = -origin.test, *.test\nca-p12 = {ca}\nca-passphrase = pw"),
        ),
        (
            "a disabled entry",
            format!(
                "[MITM]\nhostname = origin.test\nhostname-disabled = origin.test\nca-p12 = {ca}\nca-passphrase = pw"
            ),
        ),
        (
            "a source the list does not take",
            format!(
                "[MITM]\nhostname = origin.test\nclient-source-address = 10.0.0.0/8\nca-p12 = {ca}\nca-passphrase = pw"
            ),
        ),
    ];
    for (case, section) in &cases {
        let h = harness_mitm(
            Profile {
                proxies: &proxies,
                hosts: "origin.test = 127.0.0.1",
                rules: "DOMAIN,origin.test,A",
                sections: section.as_str(),
                ..Profile::default()
            },
            origin.roots(),
        )
        .await;
        assert!(
            h.engine.mitm_ca_der().is_some(),
            "the CA is usable; {case} is what passes this one through"
        );

        let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
        let mut tls = handshake(
            tunnel,
            client_config(origin.roots(), &[], true),
            "origin.test",
        )
        .await
        .unwrap_or_else(|e| panic!("{case}: the origin's own certificate is served: {e}"));
        let response = get(&mut tls, "origin.test", "/a").await;
        assert!(response.starts_with("HTTP/1.1 200"), "{case}: {response}");
        assert!(response.ends_with("from the origin"), "{case}: {response}");
    }
}

/// This machine's own sessions are compared as `127.0.0.1` whether they come
/// through a listener or through the virtual interface, so a source list of
/// `127.0.0.1` decrypts both and a list of `::1` neither (`phase 4 M2 design
/// 7.2`).
#[tokio::test]
async fn the_local_machine_is_compared_as_127_0_0_1() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    server.set("/b", "from the origin again");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("A = socks5, 127.0.0.1, {}", upstream.addr().port());
    let mitm = mitm_ca();
    let (ca, roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let section = |source: &str| {
        format!(
            "[MITM]\nhostname = origin.test\nclient-source-address = {source}\nca-p12 = {ca}\nca-passphrase = pw"
        )
    };

    // `127.0.0.1` takes both entries
    let local = section("127.0.0.1");
    let rig = Rig::on(
        harness_mitm(
            Profile {
                proxies: &proxies,
                hosts: "origin.test = 127.0.0.1",
                rules: &guarded("DOMAIN,origin.test,A"),
                sections: &local,
                ..Profile::default()
            },
            origin.roots(),
        )
        .await,
    )
    .await;
    let tunnel = connect_via_socks5(rig.h.socks(), "origin.test", 443).await;
    let mut tls = handshake(
        tunnel,
        client_config(roots.clone(), &[], true),
        "origin.test",
    )
    .await
    .expect("the local machine is 127.0.0.1 through the SOCKS5 listener");
    two_200s(&mut tls, "origin.test", ["/a", "/b"]).await;
    drop(tls);

    let (fake, _) = rig.fakes("origin.test");
    let tcp = rig
        .app
        .connect(SocketAddr::new(fake, 443))
        .await
        .expect("the session is accepted");
    let mut tls = handshake(tcp, client_config(roots, &[], true), "origin.test")
        .await
        .expect("the local machine is 127.0.0.1 through the virtual interface too");
    two_200s(&mut tls, "origin.test", ["/a", "/b"]).await;
    tls.shutdown().await.expect("the close_notify goes out");
    let mut rest = Vec::new();
    tokio::time::timeout(WITHIN, tls.read_to_end(&mut rest))
        .await
        .expect("the engine lets go within the bound")
        .expect("the stream ends cleanly");
    assert_eq!(
        rig.h.engine.http_counts().mitm,
        MitmCounts {
            decrypted: 2,
            ..MitmCounts::default()
        },
        "both entries were decrypted"
    );

    // `::1` is not what this machine's sessions are compared as: neither
    // entry is decrypted, and the client sees the origin's certificate
    let nowhere = section("::1");
    let rig = Rig::on(
        harness_mitm(
            Profile {
                proxies: &proxies,
                hosts: "origin.test = 127.0.0.1",
                rules: &guarded("DOMAIN,origin.test,A"),
                sections: &nowhere,
                ..Profile::default()
            },
            origin.roots(),
        )
        .await,
    )
    .await;
    let tunnel = connect_via_socks5(rig.h.socks(), "origin.test", 443).await;
    let mut tls = handshake(
        tunnel,
        client_config(origin.roots(), &[], true),
        "origin.test",
    )
    .await
    .expect("a source list of ::1 does not take 127.0.0.1");
    two_200s(&mut tls, "origin.test", ["/a", "/b"]).await;
    drop(tls);

    let (fake, _) = rig.fakes("origin.test");
    let tcp = rig
        .app
        .connect(SocketAddr::new(fake, 443))
        .await
        .expect("the session is accepted");
    let mut tls = handshake(tcp, client_config(origin.roots(), &[], true), "origin.test")
        .await
        .expect("the virtual interface is 127.0.0.1 as well");
    two_200s(&mut tls, "origin.test", ["/a", "/b"]).await;
    tls.shutdown().await.expect("the close_notify goes out");
    let mut rest = Vec::new();
    tokio::time::timeout(WITHIN, tls.read_to_end(&mut rest))
        .await
        .expect("the engine lets go within the bound")
        .expect("the stream ends cleanly");
    assert_eq!(
        rig.h.engine.http_counts().mitm,
        MitmCounts::default(),
        "nothing was decrypted"
    );
}

/// Another generation built from `new_text` over the same data directory, as
/// a reload does.
async fn swap_to(h: &Harness, new_text: String) {
    h.engine
        .swap_runtime(runtime(h.dir.path(), &new_text, h.engine.shared()).await);
}

/// A reload with another CA signs with it: a client trusting only the new CA
/// completes the handshake, one trusting only the old CA is refused
/// (`phase 4 M2 design 5.4`).
#[tokio::test]
async fn a_reload_with_another_ca_issues_from_it() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    server.set("/b", "from the origin again");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let old = mitm_ca();
    // a second CA, written differently from the first one: webpki takes a
    // certificate with the same subject for the issuer and fails on the
    // signature instead (`UnknownIssuer` is what a missing trust anchor says)
    let new = TlsFixture::with_ca_name("rurge test MITM CA 2", &[]);
    let (old_ca, old_roots) = (old.ca_p12_base64("pw"), old.roots());
    let (new_ca, new_roots) = (new.ca_p12_base64("pw"), new.roots());
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &old_ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(tunnel, client_config(old_roots, &[], true), "origin.test")
        .await
        .expect("the first CA signs the first connection's leaf");
    two_200s(&mut tls, "origin.test", ["/a", "/b"]).await;
    drop(tls);

    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    swap_to(&h, text.replace(&old_ca, &new_ca)).await;

    // the new CA is the one this generation signs with
    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(tunnel, client_config(new_roots, &[], true), "origin.test")
        .await
        .expect("the second CA signs the new connection's leaf");
    two_200s(&mut tls, "origin.test", ["/a", "/b"]).await;
    drop(tls);

    // and the old one's leaves are gone with it
    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let Err(error) = handshake(tunnel, client_config(old.roots(), &[], true), "origin.test").await
    else {
        panic!("the old CA signs nothing after the reload");
    };
    assert!(error.to_string().contains("UnknownIssuer"), "{error}");
}

/// A reload whose `ca-p12` cannot be opened leaves the connections already
/// decrypted alone, and the next connection sees the origin's certificate
/// (`phase 4 M2 design 5.4`, `7.1`).
#[tokio::test]
async fn a_reload_that_breaks_the_ca_leaves_open_connections_decrypted() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    server.set("/b", "from the origin again");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    // A: decrypted, its first request answered
    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut a = handshake(
        tunnel,
        client_config(roots.clone(), &[], true),
        "origin.test",
    )
    .await
    .expect("the client trusts the MITM CA");
    let first = get(&mut a, "origin.test", "/a").await;
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    the_records(&h, 1).await;

    // the CA this generation would load cannot be opened: `AAAA` is Base64,
    // but not a PKCS#12
    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    let broken = text.replace(&format!("ca-p12 = {ca}"), "ca-p12 = AAAA");
    swap_to(&h, broken).await;
    assert!(
        h.engine.mitm_ca_der().is_none(),
        "a CA rurge cannot open leaves this generation without one"
    );

    // A's second request goes on the connection it was decrypted on
    let second = get(&mut a, "origin.test", "/b").await;
    assert!(second.starts_with("HTTP/1.1 200"), "{second}");
    assert!(second.ends_with("from the origin again"), "{second}");
    drop(a);

    // a new connection is passed through: the origin's own certificate
    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut b = handshake(
        tunnel,
        client_config(origin.roots(), &[], true),
        "origin.test",
    )
    .await
    .expect("the origin's own certificate is served");
    let third = get(&mut b, "origin.test", "/a").await;
    assert!(third.starts_with("HTTP/1.1 200"), "{third}");
    assert!(third.ends_with("from the origin"), "{third}");
}

/// A WebSocket upgrade crosses a decrypted connection: the 101 is relayed,
/// and the bytes after it pass both ways untouched (`phase 4 M2 design 9.5`).
#[tokio::test]
async fn a_websocket_echoes_through_a_decrypted_connection() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = switching_echo_over_tls(&origin).await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(server),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(tunnel, client_config(roots, &[], true), "origin.test")
        .await
        .expect("the client trusts the MITM CA");
    tls.write_all(
        b"GET /ws HTTP/1.1\r\nHost: origin.test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
    )
    .await
    .unwrap();
    let head = read_head(&mut tls).await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(
        head.to_ascii_lowercase().contains("upgrade: websocket"),
        "{head}"
    );

    // far from the handshake and the headers, so a byte out of place shows
    let payload: Vec<u8> = (0..10_000).map(|i| (i % 251) as u8).collect();
    tls.write_all(&payload).await.unwrap();
    let mut back = vec![0u8; payload.len()];
    tokio::time::timeout(WITHIN, tls.read_exact(&mut back))
        .await
        .expect("the echo comes within the bound")
        .expect("the echo reaches the client");
    assert_eq!(back, payload, "every byte comes back in its place");

    // The client closes its own side; the relay closes the origin's write side,
    // and the origin's `close_notify` comes back the same way. So the client's
    // read ends cleanly, not with an EOF at the TLS layer.
    tls.shutdown().await.expect("the close_notify goes out");
    let mut rest = Vec::new();
    tokio::time::timeout(WITHIN, tls.read_to_end(&mut rest))
        .await
        .expect("the end comes within the bound")
        .expect("the client's side ends with a close_notify");
    assert!(rest.is_empty(), "{rest:?}");
    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status_code, Some(101), "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Completed, "{records:?}");
    assert_eq!(
        records[0].url.as_deref(),
        Some("https://origin.test/ws"),
        "{records:?}"
    );
}

/// `[MITM] hostname = *` does not reach plain HTTP: with a usable CA and a
/// list that takes every name, a request to port 80 is the engine's as it
/// always was, no handshake with a client is tried, and its record says `http`
/// (`phase 4 M2 design 7.1`, `9.1`).
#[tokio::test]
async fn plain_http_is_untouched_by_mitm() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "from the origin");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("*", &ca),
            ..Profile::default()
        },
        // nothing upstream here is TLS, so no origin certificate is ever
        // checked; the roots only have to be some, because setting MITM up
        // builds a verifier from them and an empty store is none
        roots,
    )
    .await;
    assert!(
        h.engine.mitm_ca_der().is_some(),
        "this generation has a usable CA, so MITM is on"
    );

    let mut tunnel = connect_via_socks5(h.socks(), "origin.test", 80).await;
    tunnel
        .write_all(b"GET /a HTTP/1.1\r\nHost: origin.test\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut back = Vec::new();
    tokio::time::timeout(WITHIN, tunnel.read_to_end(&mut back))
        .await
        .expect("the answer comes within the bound")
        .unwrap();
    let response = String::from_utf8_lossy(&back).into_owned();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("from the origin"), "{response}");

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].protocol, Some(ProtocolKind::Http), "{records:?}");
    assert_eq!(
        records[0].url.as_deref(),
        Some("http://origin.test/a"),
        "{records:?}"
    );
    assert_eq!(records[0].status_code, Some(200), "{records:?}");
    assert_eq!(
        h.engine.http_counts().mitm,
        MitmCounts::default(),
        "no TLS handshake with a client was tried"
    );
}

/// A decrypted request to an HTTP proxy policy is tunnelled: rurge sends the
/// proxy a `CONNECT` for the origin's name and port, and the TLS to the origin
/// happens inside the tunnel (`phase 4 M2 design 9.1`).
#[tokio::test]
async fn a_decrypted_request_tunnels_through_an_http_proxy() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let upstream = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(origin_addr(&server)),
        ..HttpProxyScript::default()
    })
    .await;
    let mitm = mitm_ca();
    let (ca, roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = http, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,A",
            sections: &mitm_section("origin.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let mut tls = handshake(tunnel, client_config(roots, &[], true), "origin.test")
        .await
        .expect("the client trusts the MITM CA");
    let response = get(&mut tls, "origin.test", "/a").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response.ends_with("from the origin"),
        "the origin answered, not the proxy's own `forwarded`: {response}"
    );

    let heads = upstream.heads();
    assert_eq!(heads.len(), 1, "{heads:?}");
    assert!(
        heads[0].request_line.starts_with("CONNECT origin.test:443"),
        "{heads:?}"
    );
}
