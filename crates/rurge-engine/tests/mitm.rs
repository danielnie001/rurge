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

/// A stop that lands while the handshake with the client is under way leaves
/// no record: the connection had no handle to finish (`phase 4 M2 design
/// 8.2`).
#[tokio::test]
async fn a_stop_during_the_handshake_leaves_no_record() {
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

    // waits for a record that must not come: the stop's branch returns at
    // once, so a record it wrote would be there before it returned — 300 ms is
    // a wide margin on a branch that has nothing left to do but return
    tokio::time::sleep(Duration::from_millis(300)).await;
    let records = rig.h.engine.request_log().recent(1000);
    assert!(records.is_empty(), "{records:?}");
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
