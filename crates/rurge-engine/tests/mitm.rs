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
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http2;
use hyper::{Method, Request, StatusCode, header};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rurge_config::rule::ProtocolKind;
use rurge_engine::MitmCounts;
use rurge_net::quic::testing::{
    RFC_DCID, big_client_hello, client_hello as testing_client_hello, hello_initial, hello_initials,
};
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

/// A bare entry matches the SNI on every port sniffing takes: an entry for
/// another host writes down the origin's port, which widens sniffing to it,
/// and the session to the bare entry's host there is decrypted
/// (`phase 4 M2 design 7.3`, 7.6).
#[tokio::test]
async fn a_bare_entry_decrypts_on_a_widened_port() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let port = origin_addr(&server).port();
    let mitm = mitm_ca();
    let (ca, mitm_roots) = (mitm.ca_p12_base64("pw"), mitm.roots());
    let section = mitm_section(&format!("origin.test, other.test:{port}"), &ca);
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
        .expect("the bare entry takes the SNI on the widened port");
    let response = get(&mut tls, "origin.test", "/a").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("from the origin"), "{response}");
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

/// The `[MITM]` section with `h2 = true`: a decrypted connection may be served
/// as HTTP/2, and the origin side offers `h2` first (`the phase 4 / M2b plan's
/// P15`).
fn mitm_h2_section(hostname: &str, ca_p12: &str) -> String {
    format!("{}\nh2 = true", mitm_section(hostname, ca_p12))
}

/// An engine decrypting `origin.test` with `h2 = true`, its SOCKS5 policy
/// pointing at `origin`, and the roots the client of a decrypted connection
/// trusts (the CA rurge signs with).
async fn harness_with_h2(
    origin: SocketAddr,
    roots: Arc<rustls::RootCertStore>,
    rules: &str,
    general: &str,
) -> (Harness, Arc<rustls::RootCertStore>, FakeSocks5) {
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let sections = mitm_h2_section("origin.test", &mitm.ca_p12_base64("pw"));
    let h = harness_mitm(
        Profile {
            general,
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "origin.test = 127.0.0.1",
            rules,
            sections: &sections,
            ..Profile::default()
        },
        roots,
    )
    .await;
    (h, mitm.roots(), upstream)
}

/// A decrypted connection to `origin.test:443` that offered `alpn`, and the
/// protocol the handshake settled on. The client trusts the MITM CA and
/// nothing else, so a handshake that ends says the leaf was rurge's.
async fn decrypted(
    h: &Harness,
    roots: Arc<rustls::RootCertStore>,
    alpn: &[&[u8]],
) -> (Option<String>, tokio_rustls::client::TlsStream<TcpStream>) {
    let tunnel = connect_via_socks5(h.socks(), "origin.test", 443).await;
    let tls = handshake(tunnel, client_config(roots, alpn, true), "origin.test")
        .await
        .expect("the client trusts the MITM CA");
    let settled = tls
        .get_ref()
        .1
        .alpn_protocol()
        .map(|p| String::from_utf8_lossy(p).into_owned());
    (settled, tls)
}

/// hyper's HTTP/2 client over a decrypted connection: the sender, and the task
/// that has to keep running for the connection to move (`the phase 4 / M2b
/// plan's P18`).
async fn h2_over(
    tls: tokio_rustls::client::TlsStream<TcpStream>,
) -> (
    http2::SendRequest<Full<Bytes>>,
    tokio::task::JoinHandle<hyper::Result<()>>,
) {
    let (sender, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .handshake(TokioIo::new(tls))
        .await
        .expect("an HTTP/2 handshake over the decrypted stream");
    (sender, tokio::spawn(conn))
}

/// Lets an HTTP/2 client go: its sender is dropped and the connection's task
/// ends within the bound, cleanly.
async fn close_h2(
    sender: http2::SendRequest<Full<Bytes>>,
    conn: tokio::task::JoinHandle<hyper::Result<()>>,
) {
    drop(sender);
    tokio::time::timeout(WITHIN, conn)
        .await
        .expect("the connection ends within the bound")
        .expect("the connection's task did not panic")
        .expect("the client's side of the connection ends cleanly");
}

/// A `GET` as hyper's HTTP/2 client takes it: the absolute URI names the
/// scheme and the authority, so the engine sees the request line it would see
/// on HTTP/1.1 (`the phase 4 / M2b plan's P18`).
fn h2_get(path: &str) -> Request<Full<Bytes>> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("https://origin.test{path}"))
        .body(Full::new(Bytes::new()))
        .expect("a request")
}

/// One request on an HTTP/2 connection, bounded as every wait of these tests
/// is: the status, and the whole body.
async fn h2_send(
    sender: &mut http2::SendRequest<Full<Bytes>>,
    request: Request<Full<Bytes>>,
) -> (StatusCode, Vec<u8>) {
    let response = tokio::time::timeout(WITHIN, sender.send_request(request))
        .await
        .expect("the answer comes within the bound")
        .expect("the request goes through");
    let status = response.status();
    let body = tokio::time::timeout(WITHIN, response.into_body().collect())
        .await
        .expect("the body comes within the bound")
        .expect("a response body")
        .to_bytes()
        .to_vec();
    (status, body)
}

/// What one decrypted `GET /a` left: the protocol the client settled on, the
/// version the origin recorded, and the engine's record of the request.
struct Combination {
    client: Option<String>,
    origin: String,
    record: rurge_engine::RequestRecord,
}

/// One decrypted `GET /a`, the client offering `client_alpn` and the origin
/// speaking `origin_alpn`. rustls takes the server's list order, so both
/// offering `[h2, http/1.1]` settles on `h2` (`the phase 4 / M2b plan's P15`).
async fn one_combination(client_alpn: &[&[u8]], origin_alpn: &[&[u8]]) -> Combination {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor_with_alpn(false, origin_alpn)).await;
    server.set("/a", "from the origin");
    let (h, client_roots, _upstream) = harness_with_h2(
        origin_addr(&server),
        origin.roots(),
        "DOMAIN,origin.test,A",
        "",
    )
    .await;

    let (settled, tls) = decrypted(&h, client_roots, client_alpn).await;
    if settled.as_deref() == Some("h2") {
        let (mut sender, conn) = h2_over(tls).await;
        let (status, body) = h2_send(&mut sender, h2_get("/a")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"from the origin");
        close_h2(sender, conn).await;
    } else {
        assert_eq!(settled.as_deref(), Some("http/1.1"), "{settled:?}");
        let mut tls = tls;
        let response = get(&mut tls, "origin.test", "/a").await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("from the origin"), "{response}");
    }

    let versions: Vec<String> = server
        .requests()
        .into_iter()
        .map(|request| request.version)
        .collect();
    assert_eq!(versions.len(), 1, "{versions:?}");
    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    Combination {
        client: settled,
        origin: versions[0].clone(),
        record: records[0].clone(),
    }
}

/// The one record a decrypted `GET /a` leaves: the request an `https://` one,
/// answered 200 and completed.
fn the_one_https_record(record: &rurge_engine::RequestRecord) {
    assert_eq!(record.protocol, Some(ProtocolKind::Https), "{record:?}");
    assert_eq!(
        record.url.as_deref(),
        Some("https://origin.test/a"),
        "{record:?}"
    );
    assert_eq!(record.status_code, Some(200), "{record:?}");
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
}

/// The four combinations of what the client offers and what the origin speaks
/// are all decrypted (`the phase 4 / M2b plan's P15`): the request reaches the
/// origin in the version it settled on, and leaves one record of its own.
#[tokio::test]
async fn four_combinations_of_http1_and_h2_are_decrypted() {
    // HTTP/1.1 on both sides: what M2a served, with `h2` on
    let both_h1 = one_combination(&[&b"http/1.1"[..]], &[&b"http/1.1"[..]]).await;
    assert_eq!(both_h1.client.as_deref(), Some("http/1.1"));
    assert_eq!(both_h1.origin, "HTTP/1.1");
    the_one_https_record(&both_h1.record);

    // an HTTP/1.1 client, and an origin that speaks HTTP/2
    let h1_to_h2 = one_combination(&[&b"http/1.1"[..]], &[&b"h2"[..], &b"http/1.1"[..]]).await;
    assert_eq!(h1_to_h2.client.as_deref(), Some("http/1.1"));
    assert_eq!(h1_to_h2.origin, "HTTP/2.0");
    the_one_https_record(&h1_to_h2.record);

    // an HTTP/2 client, and an origin that speaks only HTTP/1.1
    let h2_to_h1 = one_combination(&[&b"h2"[..], &b"http/1.1"[..]], &[&b"http/1.1"[..]]).await;
    assert_eq!(h2_to_h1.client.as_deref(), Some("h2"));
    assert_eq!(h2_to_h1.origin, "HTTP/1.1");
    the_one_https_record(&h2_to_h1.record);

    // HTTP/2 on both sides
    let both_h2 = one_combination(
        &[&b"h2"[..], &b"http/1.1"[..]],
        &[&b"h2"[..], &b"http/1.1"[..]],
    )
    .await;
    assert_eq!(both_h2.client.as_deref(), Some("h2"));
    assert_eq!(both_h2.origin, "HTTP/2.0");
    the_one_https_record(&both_h2.record);
}

/// With `h2 = true` a client that offers only `h2` is decrypted: the handshake
/// settles on `h2` and its request is answered (`the phase 4 / M2b plan's
/// P15`; with `h2` off, `a_client_offering_only_h2_is_passed_through`).
#[tokio::test]
async fn an_h2_only_client_is_decrypted_when_h2_is_on() {
    let combination = one_combination(&[&b"h2"[..]], &[&b"http/1.1"[..]]).await;
    assert_eq!(
        combination.client.as_deref(),
        Some("h2"),
        "the handshake settled on what the client offered"
    );
    assert_eq!(combination.origin, "HTTP/1.1");
    the_one_https_record(&combination.record);
}

/// Two streams of one HTTP/2 connection are routed one by one: the one whose
/// URL the `URL-REGEX` matches is refused on its own stream with the reject
/// page, the other goes to the origin, and the connection carries on
/// (`phase 4 M2 design 9.1`, `10.5`).
#[tokio::test]
async fn each_h2_stream_follows_its_own_rule() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let (h, roots, _upstream) = harness_with_h2(
        origin_addr(&server),
        origin.roots(),
        "URL-REGEX,^https://origin\\.test/b,REJECT\nDOMAIN,origin.test,A",
        "show-error-page-for-reject = true",
    )
    .await;

    let (settled, tls) = decrypted(&h, roots, &[&b"h2"[..]]).await;
    assert_eq!(settled.as_deref(), Some("h2"));
    let (mut sender, conn) = h2_over(tls).await;

    let (status, body) = h2_send(&mut sender, h2_get("/a")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"from the origin");

    let (status, body) = h2_send(&mut sender, h2_get("/b")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let page = String::from_utf8_lossy(&body).into_owned();
    assert!(
        page.contains("<code>URL-REGEX,^https://origin\\.test/b,REJECT</code>"),
        "the reject page names the rule: {page}"
    );

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(
        records[0].rule.as_deref(),
        Some("DOMAIN,origin.test,A"),
        "{records:?}"
    );
    assert_eq!(records[0].status_code, Some(200), "{records:?}");
    assert_eq!(
        records[1].rule.as_deref(),
        Some("URL-REGEX,^https://origin\\.test/b,REJECT"),
        "{records:?}"
    );
    assert_eq!(records[1].status_code, Some(403), "{records:?}");

    // the connection carries on: another stream through the same sender
    let (status, body) = h2_send(&mut sender, h2_get("/a")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"from the origin");

    let records = the_records(&h, 3).await;
    assert_eq!(records.len(), 3, "{records:?}");
    assert_eq!(
        records[2].rule.as_deref(),
        Some("DOMAIN,origin.test,A"),
        "{records:?}"
    );
    assert_eq!(records[2].status_code, Some(200), "{records:?}");
    assert_eq!(records[2].status, RecordStatus::Completed, "{records:?}");
    close_h2(sender, conn).await;
}

/// Concurrent streams of one HTTP/2 client share one upstream HTTP/2
/// connection (`phase 4 M2 design 10.2` and `M2-D15`): however many streams
/// are open at once, the origin is asked on one connection.
#[tokio::test]
async fn concurrent_h2_streams_share_one_upstream_connection() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/slow", "from the origin");
    server.set_delay("/slow", Duration::from_millis(200));
    let (h, roots, _upstream) = harness_with_h2(
        origin_addr(&server),
        origin.roots(),
        "DOMAIN,origin.test,A",
        "",
    )
    .await;

    let (settled, tls) = decrypted(&h, roots, &[&b"h2"[..]]).await;
    assert_eq!(settled.as_deref(), Some("h2"));
    let (mut sender, conn) = h2_over(tls).await;

    // one request first, so that the upstream connection is there to share
    let (status, body) = h2_send(&mut sender, h2_get("/slow")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"from the origin");

    // five streams at once, each on its own clone of the same sender
    let mut streams = tokio::task::JoinSet::new();
    for _ in 0..5 {
        let mut sender = sender.clone();
        streams.spawn(async move { h2_send(&mut sender, h2_get("/slow")).await });
    }
    while let Some(joined) = tokio::time::timeout(WITHIN, streams.join_next())
        .await
        .expect("every stream is answered within the bound")
    {
        let (status, body) = joined.expect("a stream's task did not panic");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"from the origin");
    }
    assert_eq!(server.hits("/slow"), 6, "every stream reached the origin");
    assert_eq!(
        server.connections(),
        1,
        "every stream rode the one upstream connection"
    );
    close_h2(sender, conn).await;
}

/// The `cookie` an HTTP/2 client splits into two fields reaches an HTTP/1.1
/// origin as one field, and the `:authority` it named becomes `Host` (RFC 9113
/// 8.2.3, 8.3.1; `the phase 4 / M2b plan's P8`).
#[tokio::test]
async fn the_cookies_of_an_h2_client_reach_an_http1_origin_as_one_field() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server =
        TestServer::spawn_tls_with(origin.acceptor_with_alpn(false, &[&b"http/1.1"[..]])).await;
    server.set("/a", "from the origin");
    let (h, roots, _upstream) = harness_with_h2(
        origin_addr(&server),
        origin.roots(),
        "DOMAIN,origin.test,A",
        "",
    )
    .await;

    let (settled, tls) = decrypted(&h, roots, &[&b"h2"[..]]).await;
    assert_eq!(settled.as_deref(), Some("h2"));
    let (mut sender, conn) = h2_over(tls).await;

    let mut request = h2_get("/a");
    request
        .headers_mut()
        .append(header::COOKIE, header::HeaderValue::from_static("a=1"));
    request
        .headers_mut()
        .append(header::COOKIE, header::HeaderValue::from_static("b=2"));
    let (status, body) = h2_send(&mut sender, request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"from the origin");

    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0].version, "HTTP/1.1", "{requests:?}");
    let cookies: Vec<&str> = requests[0]
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("cookie"))
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(cookies, ["a=1; b=2"], "the two fields became one, joined");
    assert_eq!(
        requests[0].header("host"),
        Some("origin.test"),
        "the `:authority` the client named is the `Host`"
    );
    close_h2(sender, conn).await;
}

/// A WebSocket upgrade from an HTTP/1.1 client goes over HTTP/1.1 to an origin
/// that speaks `h2`: the echo is the same as M2a's, and the origin's handshake
/// settled on `http/1.1` (`the phase 4 / M2b plan's P12`).
#[tokio::test]
async fn a_websocket_from_an_http1_client_goes_over_http1_where_the_origin_speaks_h2() {
    let origin = TlsFixture::new(&["origin.test"]);
    // `switching_echo_over_tls` accepts with `origin.acceptor(false)`, which
    // offers `[h2, http/1.1]`
    let server = switching_echo_over_tls(&origin).await;
    let (h, roots, _upstream) =
        harness_with_h2(server, origin.roots(), "DOMAIN,origin.test,A", "").await;

    let (settled, mut tls) = decrypted(&h, roots, &[&b"http/1.1"[..]]).await;
    assert_eq!(settled.as_deref(), Some("http/1.1"));
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

    let seen = origin.seen_at_least(1).await;
    assert_eq!(
        seen[0].alpn.as_deref(),
        Some("http/1.1"),
        "the upgrade dials HTTP/1.1 only"
    );

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

/// HTTP/2's `CONNECT` is answered 501 and leaves one record of its own: the
/// carrier connection records nothing beside it (`the phase 4 / M2b plan's
/// P6`).
#[tokio::test]
async fn a_connect_over_h2_has_one_record_and_a_501() {
    let origin = TlsFixture::new(&["origin.test"]);
    // nothing is dialled here: a `CONNECT` is answered before it would be
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    let (h, roots, _upstream) = harness_with_h2(
        origin_addr(&server),
        origin.roots(),
        "DOMAIN,origin.test,A",
        "",
    )
    .await;

    let (settled, tls) = decrypted(&h, roots, &[&b"h2"[..]]).await;
    assert_eq!(settled.as_deref(), Some("h2"));
    let (mut sender, conn) = h2_over(tls).await;

    // the request line is the authority alone, as RFC 9113 8.5 gives a
    // `CONNECT`: hyper writes `:authority` from it, and no `:scheme`/`:path`
    let request = Request::builder()
        .method(Method::CONNECT)
        .uri("origin.test:443")
        .body(Full::new(Bytes::new()))
        .expect("a request");
    let (status, body) = h2_send(&mut sender, request).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body, b"CONNECT over HTTP/2 is not supported");

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(
        records[0].error.as_deref(),
        Some("CONNECT over HTTP/2 is not supported"),
        "{records:?}"
    );
    assert_eq!(records[0].status_code, Some(501), "{records:?}");
    // the `CONNECT` named an authority and no URL
    assert_eq!(records[0].url, None, "{records:?}");

    close_h2(sender, conn).await;

    // The only fixed wait here is for the end of the connection the client has
    // just closed, and for the record it would make if it made one. Its record
    // is made when the engine reads the client's EOF — far shorter than the
    // 300 ms waited on the loopback.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        h.engine.request_log().recent(10).len(),
        1,
        "the carrier connection left a record of its own beside its request's"
    );
    assert!(h.engine.request_log().active().is_empty());
}

/// The API's TLS follows the last CA a generation had (the phase 4 / M2c
/// plan's P9): a reload that brings another CA has the API sign its next
/// connections with it, a reload that leaves the profile without a CA keeps
/// the CA the API had, and an engine that never had one has no TLS at all.
#[tokio::test]
async fn api_tls_is_the_last_ca_a_generation_had() {
    let a = mitm_ca();
    let b = TlsFixture::with_ca_name("rurge test MITM CA 2", &[]);
    let section = mitm_section("origin.test", &a.ca_p12_base64("pw"));
    let h = harness_mitm(
        Profile {
            sections: &section,
            ..Profile::default()
        },
        a.roots(),
    )
    .await;
    assert_eq!(
        h.engine
            .api_tls()
            .map(|tls| tls.ca_der().to_vec())
            .as_deref(),
        Some(a.ca_der()),
        "the API signs with the CA of the generation in use"
    );

    // a generation without a CA: the API keeps the one it had, while the
    // generation itself decrypts nothing
    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    assert_eq!(text.matches(&section).count(), 1, "one [MITM] section");
    swap_to(&h, text.replace(&section, "")).await;
    assert_eq!(
        h.engine.mitm_ca_der(),
        None,
        "the generation has no CA to decrypt with"
    );
    assert_eq!(
        h.engine
            .api_tls()
            .map(|tls| tls.ca_der().to_vec())
            .as_deref(),
        Some(a.ca_der()),
        "the API goes on with the last CA it had"
    );

    // a reload with another CA: the API follows it
    let new_section = mitm_section("origin.test", &b.ca_p12_base64("pw"));
    swap_to(&h, text.replace(&section, &new_section)).await;
    assert_eq!(
        h.engine
            .api_tls()
            .map(|tls| tls.ca_der().to_vec())
            .as_deref(),
        Some(b.ca_der()),
        "the API signs with the CA of the new generation"
    );
    assert_eq!(
        h.engine.mitm_ca_der().as_deref(),
        Some(b.ca_der()),
        "the generation decrypts with the new CA"
    );

    // an engine that never had a CA has no TLS to serve the API with
    let without = harness_mitm(
        Profile {
            sections: "",
            ..Profile::default()
        },
        a.roots(),
    )
    .await;
    assert!(
        without.engine.api_tls().is_none(),
        "nothing to sign the API's certificates with"
    );
}

// ---- why a TLS session was not decrypted (plan task 3)

/// What a record says about why its session was not decrypted: only the notes
/// of the phase 4 / M4b plan's P6, in the order they were written.
fn reasons(record: &rurge_engine::RequestRecord) -> Vec<&str> {
    record
        .notes
        .iter()
        .map(String::as_str)
        .filter(|note| note.starts_with("Passed through:") || note.starts_with("Not decrypted:"))
        .collect()
}

/// Asserts that `record`'s reason is exactly `note`, and nothing else that
/// says why the session was passed through (`None`: no reason at all).
fn assert_reason(record: &rurge_engine::RequestRecord, note: Option<&str>) {
    let wanted: Vec<&str> = note.into_iter().collect();
    assert_eq!(reasons(record), wanted, "{record:?}");
}

/// The switch is asked after the list and before anything else: a listed
/// session is noted as passed for it, and a session no list takes says nothing
/// at all (`phase 4 M4 design 10.2`, the phase 4 / M4b plan's P6).
#[tokio::test]
async fn a_listed_session_with_mitm_off_is_noted() {
    let origin = TlsFixture::new(&["listed.test", "other.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
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
            hosts: "listed.test = 127.0.0.1\nother.test = 127.0.0.1",
            rules: "DOMAIN,listed.test,A\nDOMAIN,other.test,A",
            sections: &mitm_section("listed.test", &ca),
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;
    h.engine.set_mitm(false).await;

    let cases = [
        ("listed.test", Some("Not decrypted: MITM is off")),
        ("other.test", None),
    ];
    let mut seen = 0;
    for (name, note) in cases {
        let tunnel = connect_via_socks5(h.socks(), name, 443).await;
        let mut tls = handshake(tunnel, client_config(origin.roots(), &[], true), name)
            .await
            .expect("the session is passed through to the origin");
        let response = get(&mut tls, name, "/a").await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        drop(tls);

        seen += 1;
        let record = the_records(&h, seen).await.remove(seen - 1);
        assert_eq!(record.dst, format!("{name}:443"), "{record:?}");
        assert_reason(&record, note);
    }
}

/// A generation without a usable CA cannot sign a leaf for a listed name, so
/// it is passed through and its record says so (`phase 4 M4 design 10.2`).
#[tokio::test]
async fn a_listed_session_without_a_usable_ca_is_noted() {
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
    drop(tls);

    let record = the_records(&h, 1).await.remove(0);
    assert_reason(&record, Some("Not decrypted: no usable CA"));
}

/// A source `[MITM] client-source-address` refuses is passed through with that
/// as its reason, and a name the list does not take is not said to be refused
/// for its source: the list is asked first (`phase 4 M4 design 10.2`).
#[tokio::test]
async fn a_listed_session_from_a_source_not_allowed_is_noted() {
    let origin = TlsFixture::new(&["listed.test", "other.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&server)),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let ca = mitm.ca_p12_base64("pw");
    // the client comes from the loopback, which is not in the list
    let section = format!(
        "[MITM]\nhostname = listed.test\nclient-source-address = 10.0.0.0/8\nca-p12 = {ca}\nca-passphrase = pw"
    );
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "listed.test = 127.0.0.1\nother.test = 127.0.0.1",
            rules: "DOMAIN,listed.test,A\nDOMAIN,other.test,A",
            sections: &section,
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;
    assert!(
        h.engine.mitm_ca_der().is_some(),
        "the CA is usable; the source is what passes these through"
    );

    let cases = [
        (
            "listed.test",
            Some("Not decrypted: the client is not in client-source-address"),
        ),
        ("other.test", None),
    ];
    let mut seen = 0;
    for (name, note) in cases {
        let tunnel = connect_via_socks5(h.socks(), name, 443).await;
        let mut tls = handshake(tunnel, client_config(origin.roots(), &[], true), name)
            .await
            .expect("the session is passed through to the origin");
        let response = get(&mut tls, name, "/a").await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        drop(tls);

        seen += 1;
        let record = the_records(&h, seen).await.remove(seen - 1);
        assert_eq!(record.dst, format!("{name}:443"), "{record:?}");
        assert_reason(&record, note);
    }
}

/// A client whose ALPN rurge does not serve is passed through with that as its
/// reason (`phase 4 M4 design 10.2`, `phase 4 M2 design 7.4`).
#[tokio::test]
async fn a_listed_session_whose_alpn_rurge_does_not_decrypt_is_noted() {
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
        client_config(mitm_roots, &[&b"spdy/3"[..]], true),
        "origin.test",
    )
    .await
    else {
        panic!("the client is passed through and sees the origin's certificate");
    };
    // the origin serves neither protocol, so the client is told so; what it is
    // told is its business, the record's note is the engine's
    let _ = error;

    let record = the_records(&h, 1).await.remove(0);
    assert_reason(
        &record,
        Some("Not decrypted: the client offers no protocol rurge decrypts"),
    );
}

/// Builds a minimal TLS ClientHello record carrying one SNI host_name — the
/// same bytes as `crates/rurge-engine/src/sniff.rs`'s test helper, which
/// `tests/` cannot reach. rustls will not send a name it takes no server name
/// for, so the hello of that case is written by hand.
fn hand_written_hello(host: &str) -> Vec<u8> {
    let sni = host.as_bytes();
    // server_name extension body: list_len(2) + type(1) + name_len(2) + name
    let mut server_name = Vec::new();
    server_name.extend_from_slice(&((sni.len() + 3) as u16).to_be_bytes());
    server_name.push(0); // host_name
    server_name.extend_from_slice(&(sni.len() as u16).to_be_bytes());
    server_name.extend_from_slice(sni);
    // extension: type 0x0000 + len + body
    let mut ext = Vec::new();
    ext.extend_from_slice(&0u16.to_be_bytes());
    ext.extend_from_slice(&(server_name.len() as u16).to_be_bytes());
    ext.extend_from_slice(&server_name);
    // handshake body
    let mut body = Vec::new();
    body.extend_from_slice(&[0x03, 0x03]); // client_version TLS1.2
    body.extend_from_slice(&[0u8; 32]); // random
    body.push(0); // session_id len
    body.extend_from_slice(&2u16.to_be_bytes()); // cipher suites len
    body.extend_from_slice(&[0x13, 0x01]);
    body.push(1); // compression len
    body.push(0);
    body.extend_from_slice(&(ext.len() as u16).to_be_bytes()); // extensions len
    body.extend_from_slice(&ext);
    // handshake header: type 0x01 + 3-byte len
    let mut hs = vec![0x01];
    let bl = body.len();
    hs.extend_from_slice(&[(bl >> 16) as u8, (bl >> 8) as u8, bl as u8]);
    hs.extend_from_slice(&body);
    // TLS record: type 0x16, version, len
    let mut rec = vec![0x16, 0x03, 0x01];
    rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
    rec.extend_from_slice(&hs);
    rec
}

/// A listed name rustls takes no server name for cannot be signed for: the
/// session is passed through with that as its reason (`phase 4 M4 design
/// 10.2`).
#[tokio::test]
async fn a_listed_name_no_certificate_can_be_made_for_is_noted() {
    let echo = rurge_proto::testing::echo_server().await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let mitm = mitm_ca();
    let ca = mitm.ca_p12_base64("pw");
    let h = harness_mitm(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "other.test = 127.0.0.1",
            rules: "DOMAIN,other.test,A",
            sections: &mitm_section("*.test", &ca),
            ..Profile::default()
        },
        mitm.roots(),
    )
    .await;

    let mut s = connect_via_socks5(h.socks(), "other.test", 443).await;
    s.write_all(&hand_written_hello(&format!("{}.test", "a".repeat(64))))
        .await
        .unwrap();
    // the client goes and the echo's answer has nobody to reach: the record
    // ends, and only what it says about the passing is asked
    drop(s);

    let record = the_records(&h, 1).await.remove(0);
    assert_reason(
        &record,
        Some("Not decrypted: no certificate can be made for this name"),
    );
}

/// A session that is decrypted carries no reason at all: none of the notes
/// about a passing is written for it (`phase 4 M4 design 10.2`).
#[tokio::test]
async fn a_decrypted_session_has_no_reason() {
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
    drop(tls);

    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(record.protocol, Some(ProtocolKind::Https), "{record:?}");
    assert_reason(&record, None);
}

/// What a QUIC flow `auto-quic-block` refused says on its record (the phase 4
/// / M5a plan's P10).
const AUTO_QUIC_BLOCKED: &str = "QUIC blocked by auto-quic-block";

/// The one Initial a QUIC client's ClientHello for `name` travels in.
fn quic_hello(name: Option<&str>) -> Vec<u8> {
    hello_initial(RFC_DCID, &testing_client_hello(name, &[b"h3"]))
}

/// The profile the `auto-quic-block` tests start from: `origin.test` pinned to
/// loopback, the `[MITM]` section that lists it, and no rule of its own.
fn quic_profile<'a>(section: &'a str) -> Profile<'a> {
    Profile {
        hosts: "origin.test = 127.0.0.1",
        sections: section,
        ..Profile::default()
    }
}

/// The finished record of the UDP flow to `dst`.
async fn ended(h: &Harness, dst: &str) -> rurge_engine::RequestRecord {
    wait_until("the UDP flow to finish", || {
        udp_records(h).iter().any(|r| r.dst == dst)
    })
    .await;
    udp_records(h)
        .into_iter()
        .find(|r| r.dst == dst)
        .expect("the flow's record")
}

/// A flow `auto-quic-block` blocked, as the plan's P12 has it asserted.
fn assert_blocked(record: &rurge_engine::RequestRecord) {
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(record.error.as_deref(), Some(AUTO_QUIC_BLOCKED));
    assert!(!record.policy.is_empty(), "{record:?}");
}

/// A flow `auto-quic-block` let through: its first datagram went out, which
/// the one that waited for it saw, and it does not say it was blocked.
fn assert_let_through(record: &rurge_engine::RequestRecord) {
    assert_ne!(record.error.as_deref(), Some(AUTO_QUIC_BLOCKED));
    assert_ne!(record.status, RecordStatus::Rejected("REJECT".into()));
}

/// A QUIC flow to a name `[MITM]` lists is blocked: its ClientHello names
/// `origin.test`, the rules and the policy are on its record, and it says why
/// (`phase 4 M5 design 6.1`, 6.2).
#[tokio::test]
async fn auto_quic_block_blocks_quic_to_a_listed_host() {
    let ca = mitm_ca();
    let h = harness_mitm(
        quic_profile(&mitm_section("origin.test", &ca.ca_p12_base64("pw"))),
        ca.roots(),
    )
    .await;
    let association = udp_associate(h.socks()).await;
    association
        .send("origin.test", 443, &quic_hello(Some("origin.test")))
        .await;
    let record = ended(&h, "origin.test:443").await;
    assert_blocked(&record);
    assert_eq!(record.sni.as_deref(), Some("origin.test"));
    let quic = h.engine.http_counts().quic;
    assert_eq!((quic.blocked, quic.hello), (1, 1));
}

/// `auto-quic-block = false` turns it off: the flow goes out.
#[tokio::test]
async fn auto_quic_block_off_lets_quic_through() {
    let ca = mitm_ca();
    let section = format!(
        "{}\nauto-quic-block = false",
        mitm_section("origin.test", &ca.ca_p12_base64("pw"))
    );
    let h = harness_mitm(quic_profile(&section), ca.roots()).await;
    let association = udp_associate(h.socks()).await;
    association
        .send("origin.test", 443, &quic_hello(Some("origin.test")))
        .await;
    let record = flowing(&h, "origin.test:443").await;
    assert_let_through(&record);
    assert_eq!(h.engine.http_counts().quic.blocked, 0);
}

/// MITM off is not decrypted, so nothing is blocked either.
#[tokio::test]
async fn with_mitm_off_quic_goes_through() {
    let ca = mitm_ca();
    let h = harness_mitm(
        quic_profile(&mitm_section("origin.test", &ca.ca_p12_base64("pw"))),
        ca.roots(),
    )
    .await;
    h.engine.set_mitm(false).await;
    let association = udp_associate(h.socks()).await;
    association
        .send("origin.test", 443, &quic_hello(Some("origin.test")))
        .await;
    let record = flowing(&h, "origin.test:443").await;
    assert_let_through(&record);
    assert_eq!(h.engine.http_counts().quic.blocked, 0);
}

/// A listed name with no usable CA is passed through, not blocked.
#[tokio::test]
async fn without_a_usable_ca_quic_goes_through() {
    let h = harness_mitm(
        quic_profile("[MITM]\nhostname = origin.test"),
        Arc::new(rustls::RootCertStore::empty()),
    )
    .await;
    let association = udp_associate(h.socks()).await;
    association
        .send("origin.test", 443, &quic_hello(Some("origin.test")))
        .await;
    let record = flowing(&h, "origin.test:443").await;
    assert_let_through(&record);
    assert_eq!(h.engine.http_counts().quic.blocked, 0);
}

/// A listed name from a source `client-source-address` does not take is
/// passed through, not blocked.
#[tokio::test]
async fn a_source_mitm_does_not_take_lets_quic_through() {
    let ca = mitm_ca();
    let section = format!(
        "{}\nclient-source-address = 10.0.0.0/8",
        mitm_section("origin.test", &ca.ca_p12_base64("pw"))
    );
    let h = harness_mitm(quic_profile(&section), ca.roots()).await;
    let association = udp_associate(h.socks()).await;
    association
        .send("origin.test", 443, &quic_hello(Some("origin.test")))
        .await;
    let record = flowing(&h, "origin.test:443").await;
    assert_let_through(&record);
    assert_eq!(h.engine.http_counts().quic.blocked, 0);
}

/// `origin.test` excluded from a set is not listed, so it is not blocked.
#[tokio::test]
async fn a_negated_entry_lets_quic_through() {
    let ca = mitm_ca();
    let h = harness_mitm(
        quic_profile(&mitm_section(
            "-origin.test, *.test",
            &ca.ca_p12_base64("pw"),
        )),
        ca.roots(),
    )
    .await;
    let association = udp_associate(h.socks()).await;
    association
        .send("origin.test", 443, &quic_hello(Some("origin.test")))
        .await;
    let record = flowing(&h, "origin.test:443").await;
    assert_let_through(&record);
    assert_eq!(h.engine.http_counts().quic.blocked, 0);
}

/// A host in `always-raw-tcp-hosts` is not sniffed on the TCP side, so a QUIC
/// flow to it is not blocked either.
#[tokio::test]
async fn a_raw_tcp_host_is_not_blocked() {
    let ca = mitm_ca();
    let h = harness_mitm(
        Profile {
            general: "always-raw-tcp-hosts = origin.test",
            ..quic_profile(&mitm_section("origin.test", &ca.ca_p12_base64("pw")))
        },
        ca.roots(),
    )
    .await;
    let association = udp_associate(h.socks()).await;
    association
        .send("origin.test", 443, &quic_hello(Some("origin.test")))
        .await;
    let record = flowing(&h, "origin.test:443").await;
    assert_let_through(&record);
    assert_eq!(h.engine.http_counts().quic.blocked, 0);
}

/// An entry without a port does not widen sniffing to 8443, so QUIC there is
/// not blocked (`phase 4 M2 design 7.6`).
#[tokio::test]
async fn a_bare_entry_does_not_block_quic_on_8443() {
    let ca = mitm_ca();
    let h = harness_mitm(
        quic_profile(&mitm_section("origin.test", &ca.ca_p12_base64("pw"))),
        ca.roots(),
    )
    .await;
    let association = udp_associate(h.socks()).await;
    association
        .send("origin.test", 8443, &quic_hello(Some("origin.test")))
        .await;
    let record = flowing(&h, "origin.test:8443").await;
    assert_let_through(&record);
    assert_eq!(h.engine.http_counts().quic.blocked, 0);
}

/// A ClientHello that does not arrive whole is not blocked: the rules see no
/// SNI, and the record says so (`phase 4 M5 design 5.2`).
#[tokio::test]
async fn an_incomplete_hello_is_not_blocked() {
    let ca = mitm_ca();
    let h = harness_mitm(
        quic_profile(&mitm_section("origin.test", &ca.ca_p12_base64("pw"))),
        ca.roots(),
    )
    .await;
    let hello = big_client_hello(Some("origin.test"));
    let [first, _] = hello_initials(RFC_DCID, &hello, 600);
    let association = udp_associate(h.socks()).await;
    association.send("origin.test", 443, &first).await;
    let record = flowing(&h, "origin.test:443").await;
    assert_let_through(&record);
    assert!(
        record
            .notes
            .iter()
            .any(|note| note == "No SNI: the QUIC ClientHello did not arrive whole"),
        "{record:?}"
    );
    let quic = h.engine.http_counts().quic;
    assert_eq!((quic.blocked, quic.hello, quic.incomplete), (0, 0, 1));
}

/// A bare entry matches the SNI on every port sniffing takes: another entry's
/// port widens sniffing to 8443, and QUIC to the bare entry's host there is
/// blocked (`phase 4 M2 design 7.3`, `phase 4 M5 design 6.1`).
#[tokio::test]
async fn a_bare_entry_blocks_quic_on_a_widened_port() {
    let ca = mitm_ca();
    let h = harness_mitm(
        quic_profile(&mitm_section(
            "origin.test, other.test:8443",
            &ca.ca_p12_base64("pw"),
        )),
        ca.roots(),
    )
    .await;
    let association = udp_associate(h.socks()).await;
    association
        .send("origin.test", 8443, &quic_hello(Some("origin.test")))
        .await;
    let record = ended(&h, "origin.test:8443").await;
    assert_blocked(&record);
    assert_eq!(record.sni.as_deref(), Some("origin.test"));
    let quic = h.engine.http_counts().quic;
    assert_eq!((quic.blocked, quic.hello), (1, 1));
}

/// An entry with a port blocks that port only, not another one of the same
/// name.
#[tokio::test]
async fn a_port_entry_blocks_only_its_port() {
    let ca = mitm_ca();
    let h = harness_mitm(
        quic_profile(&mitm_section("origin.test:8443", &ca.ca_p12_base64("pw"))),
        ca.roots(),
    )
    .await;
    let association = udp_associate(h.socks()).await;
    let hello = quic_hello(Some("origin.test"));
    association.send("origin.test", 8443, &hello).await;
    association.send("origin.test", 443, &hello).await;
    let blocked = ended(&h, "origin.test:8443").await;
    assert_blocked(&blocked);
    let through = flowing(&h, "origin.test:443").await;
    assert_let_through(&through);
    let quic = h.engine.http_counts().quic;
    assert_eq!((quic.blocked, quic.hello), (1, 2));
}

/// `block-quic = always-allow` does not pass a flow `auto-quic-block` blocks
/// (M5-D13).
#[tokio::test]
async fn always_allow_does_not_stop_auto_quic_block() {
    let ca = mitm_ca();
    let h = harness_mitm(
        Profile {
            general: "block-quic = always-allow",
            ..quic_profile(&mitm_section("origin.test", &ca.ca_p12_base64("pw")))
        },
        ca.roots(),
    )
    .await;
    let association = udp_associate(h.socks()).await;
    association
        .send("origin.test", 443, &quic_hello(Some("origin.test")))
        .await;
    let record = ended(&h, "origin.test:443").await;
    assert_blocked(&record);
    assert_eq!(h.engine.http_counts().quic.blocked, 1);
}

/// With `block-quic = all` too, the record names `auto-quic-block`: it is
/// asked first (`phase 4 M5 design 6.3`).
#[tokio::test]
async fn with_block_quic_too_the_record_names_auto_quic_block() {
    let ca = mitm_ca();
    let h = harness_mitm(
        Profile {
            general: "block-quic = all",
            ..quic_profile(&mitm_section("origin.test", &ca.ca_p12_base64("pw")))
        },
        ca.roots(),
    )
    .await;
    let association = udp_associate(h.socks()).await;
    association
        .send("origin.test", 443, &quic_hello(Some("origin.test")))
        .await;
    let record = ended(&h, "origin.test:443").await;
    assert_blocked(&record);
    assert_eq!(h.engine.http_counts().quic.blocked, 1);
}

/// A `REJECT` rule of the target has the first word: its record is a plain
/// rejection, not `auto-quic-block`'s.
#[tokio::test]
async fn a_reject_rule_comes_before_auto_quic_block() {
    let ca = mitm_ca();
    let h = harness_mitm(
        Profile {
            rules: "DOMAIN,origin.test,REJECT",
            ..quic_profile(&mitm_section("origin.test", &ca.ca_p12_base64("pw")))
        },
        ca.roots(),
    )
    .await;
    let association = udp_associate(h.socks()).await;
    association
        .send("origin.test", 443, &quic_hello(Some("origin.test")))
        .await;
    let record = ended(&h, "origin.test:443").await;
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".into()));
    assert_ne!(record.error.as_deref(), Some(AUTO_QUIC_BLOCKED));
    let quic = h.engine.http_counts().quic;
    assert_eq!((quic.blocked, quic.hello), (0, 1));
}

/// Without an SNI the target decides (`phase 4 M5 design 6.4`): blocked at
/// 443 where the list takes it, not at 8443 where sniffing is not widened.
#[tokio::test]
async fn a_hello_without_sni_is_judged_by_its_target() {
    let ca = mitm_ca();
    let h = harness_mitm(
        quic_profile(&mitm_section("origin.test", &ca.ca_p12_base64("pw"))),
        ca.roots(),
    )
    .await;
    let association = udp_associate(h.socks()).await;
    let hello = quic_hello(None);
    association.send("origin.test", 443, &hello).await;
    association.send("origin.test", 8443, &hello).await;
    let blocked = ended(&h, "origin.test:443").await;
    assert_blocked(&blocked);
    assert_eq!(blocked.sni, None);
    let through = flowing(&h, "origin.test:8443").await;
    assert_let_through(&through);
    let quic = h.engine.http_counts().quic;
    assert_eq!((quic.blocked, quic.hello), (1, 2));
}

/// The list is matched by the name the ClientHello gives, not by the target
/// alone: a flow to an address, whose hello names a listed host, is blocked
/// (`phase 4 M5 design 6.1`).
#[tokio::test]
async fn a_flow_to_an_address_is_blocked_by_its_sni() {
    let ca = mitm_ca();
    let h = harness_mitm(
        quic_profile(&mitm_section("origin.test", &ca.ca_p12_base64("pw"))),
        ca.roots(),
    )
    .await;
    let association = udp_associate(h.socks()).await;
    // the target is an address the list does not name; the hello names the
    // listed host, so only the SNI can take it
    association
        .send("127.0.0.1", 443, &quic_hello(Some("origin.test")))
        .await;
    let record = ended(&h, "127.0.0.1:443").await;
    assert_blocked(&record);
    assert_eq!(record.sni.as_deref(), Some("origin.test"));
    let quic = h.engine.http_counts().quic;
    assert_eq!((quic.blocked, quic.hello), (1, 1));
}
