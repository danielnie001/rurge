//! rurge's outbounds against a real sing-box on the loopback. Every target
//! is a loopback IP literal, so sing-box neither resolves names nor leaves
//! the machine. Without a sing-box binary each test prints why it is skipped
//! (`RURGE_INTEROP_REQUIRED=1` turns that into a failure).

use rurge_config::config::{LoadOptions, from_text};
use rurge_engine::EngineFactory;
use rurge_interop::{Inbound, InboundKind, SingBox, TlsFiles, sing_box_or_skip};
use rurge_net::connector::{ConnectOpts, SystemResolve, Target};
use rurge_net::socket::NoopSocketHook;
use rurge_net::testing::TestServer;
use rurge_policy::OutboundFactory;
use rurge_proto::testing::{TlsFixture, echo_server};
use rurge_proto::{OutboundError, OutboundRef};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The outbound of policy `name` in `profile`, built the way the engine
/// builds it; `fixture`'s CA is trusted when given.
fn outbound(profile: &str, name: &str, fixture: Option<&Arc<TlsFixture>>) -> OutboundRef {
    let loaded = from_text(
        profile,
        Path::new("interop.conf"),
        &LoadOptions::for_tests(),
    );
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    let cfg = loaded.config;
    let factory = match fixture {
        Some(f) => EngineFactory::with_roots(
            &cfg,
            Arc::new(SystemResolve),
            Arc::new(NoopSocketHook),
            f.roots(),
        ),
        None => EngineFactory::new(&cfg, Arc::new(SystemResolve), Arc::new(NoopSocketHook)),
    };
    let spec = cfg.spec(name).expect("the policy has a spec");
    factory
        .build(spec, factory.direct_connector(&spec.common))
        .expect("the policy builds")
}

fn target(addr: SocketAddr) -> Target {
    Target::new(rurge_config::HostName::Ip(addr.ip()), addr.port())
}

async fn roundtrip(out: &OutboundRef, echo: SocketAddr) {
    let mut stream = out
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .expect("the tunnel is established");
    stream.write_all(b"interop").await.unwrap();
    let mut buf = [0u8; 7];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"interop");
}

fn plain(kind: InboundKind, users: &[(&str, &str)]) -> Inbound {
    Inbound {
        kind,
        users: users
            .iter()
            .map(|(u, p)| (u.to_string(), p.to_string()))
            .collect(),
        tls: None,
        ws_path: None,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn http_connect_with_and_without_credentials() {
    let Some(bin) = sing_box_or_skip("http_connect_with_and_without_credentials") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![
            plain(InboundKind::Http, &[]),
            plain(InboundKind::Http, &[("alice", "s3cret")]),
        ],
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nOpen = http, 127.0.0.1, {}\nAuth = http, 127.0.0.1, {}, alice, s3cret\nWrong = http, 127.0.0.1, {}, alice, nope\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0),
        sb.port(1),
        sb.port(1)
    );
    roundtrip(&outbound(&profile, "Open", None), echo).await;
    roundtrip(&outbound(&profile, "Auth", None), echo).await;
    let refused = outbound(&profile, "Wrong", None)
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .err()
        .expect("wrong credentials are refused");
    assert!(
        matches!(&refused, OutboundError::Proxy(m) if m.starts_with("http proxy answered 407")),
        "{refused}"
    );
}

#[tokio::test]
async fn a_plain_request_in_absolute_form_is_served_by_sing_box() {
    let Some(bin) = sing_box_or_skip("a_plain_request_in_absolute_form_is_served_by_sing_box")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![plain(InboundKind::Http, &[("alice", "s3cret")])],
    );
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let port = origin.url("/").port().unwrap();
    let profile = format!(
        "[Proxy]\nUp = http, 127.0.0.1, {}, alice, s3cret\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0)
    );
    let out = outbound(&profile, "Up", None);
    let forward = out.http_forward().expect("forward mode is the default");
    let mut stream = forward.connect(&ConnectOpts::default()).await.unwrap();
    let mut request =
        format!("GET http://127.0.0.1:{port}/hello HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    for (name, value) in forward.request_headers() {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    // `Connection: close` makes EOF the real bound rather than a fixed wait.
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stream.read_to_end(&mut response),
    )
    .await
    .expect("sing-box closes the connection after the response");
    let response = String::from_utf8_lossy(&response);
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "{response}\n{}",
        sb.log_text()
    );
    assert!(response.ends_with("hi there"), "{response}");
    assert_eq!(origin.hits("/hello"), 1);
}

#[tokio::test]
async fn https_with_a_private_ca_a_pinned_fingerprint_and_a_client_certificate() {
    let Some(bin) =
        sing_box_or_skip("https_with_a_private_ca_a_pinned_fingerprint_and_a_client_certificate")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["localhost", "127.0.0.1"]);
    let write = |name: &str, text: String| {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    let (cert, key, ca) = (
        write("leaf.pem", fixture.leaf_pem()),
        write("leaf.key", fixture.leaf_key_pem()),
        write("ca.pem", fixture.ca_pem()),
    );
    let tls = |client_ca: Option<std::path::PathBuf>| Inbound {
        kind: InboundKind::Http,
        users: Vec::new(),
        tls: Some(TlsFiles {
            certificate: cert.clone(),
            key: key.clone(),
            client_ca,
        }),
        ws_path: None,
    };
    let sb = SingBox::spawn(&bin, dir.path(), vec![tls(None), tls(Some(ca))]);
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nCa = https, 127.0.0.1, {open}\nNamed = https, 127.0.0.1, {open}, sni=localhost\n\
Pinned = https, 127.0.0.1, {open}, server-cert-fingerprint-sha256={pin}\n\
WrongPin = https, 127.0.0.1, {open}, server-cert-fingerprint-sha256={zero}\n\
Mutual = https, 127.0.0.1, {mtls}, client-cert=mtls\nNoCert = https, 127.0.0.1, {mtls}\n\
[Keystore]\nmtls = type=p12, password=pw, base64={p12}\n[Rule]\nFINAL,DIRECT\n",
        open = sb.port(0),
        mtls = sb.port(1),
        pin = hex(&fixture.leaf_fingerprint()),
        zero = "0".repeat(64),
        p12 = fixture.client_p12_base64("interop client"),
    );
    for name in ["Ca", "Named", "Mutual"] {
        roundtrip(&outbound(&profile, name, Some(&fixture)), echo).await;
    }
    // pinned: no CA is trusted at all
    roundtrip(&outbound(&profile, "Pinned", None), echo).await;
    for (name, trusted) in [("WrongPin", None), ("Ca", None), ("NoCert", Some(&fixture))] {
        let e = outbound(&profile, name, trusted)
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .err()
            .unwrap_or_else(|| panic!("{name} must not get through"));
        // `Timeout` too: on a loaded runner a handshake that is going to be
        // refused can run out the clock first, and "correctly did not get
        // through" must not turn red for that.
        assert!(
            matches!(
                e,
                OutboundError::Tls(_)
                    | OutboundError::Proxy(_)
                    | OutboundError::Io(_)
                    | OutboundError::Timeout
            ),
            "{name}: {e}"
        );
    }
}

#[tokio::test]
async fn socks5_with_and_without_credentials_and_the_mixed_inbound() {
    let Some(bin) = sing_box_or_skip("socks5_with_and_without_credentials_and_the_mixed_inbound")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![
            plain(InboundKind::Socks, &[]),
            plain(InboundKind::Socks, &[("alice", "s3cret")]),
            plain(InboundKind::Mixed, &[]),
        ],
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nOpen = socks5, 127.0.0.1, {}\nAuth = socks5, 127.0.0.1, {}, alice, s3cret\nWrong = socks5, 127.0.0.1, {}, alice, nope\n\
MixedSocks = socks5, 127.0.0.1, {}\nMixedHttp = http, 127.0.0.1, {}\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0),
        sb.port(1),
        sb.port(1),
        sb.port(2),
        sb.port(2)
    );
    for name in ["Open", "Auth", "MixedSocks", "MixedHttp"] {
        roundtrip(&outbound(&profile, name, None), echo).await;
    }
    let refused = outbound(&profile, "Wrong", None)
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .err()
        .expect("wrong credentials are refused");
    assert!(matches!(&refused, OutboundError::Proxy(_)), "{refused}");
}

/// What "skipped" means must itself be tested: without a binary the helper
/// says so and returns `None`; this machine decides which branch runs.
#[test]
fn a_missing_binary_is_a_skip_unless_interop_is_required() {
    if rurge_interop::locate().is_some() {
        return; // the real tests above are running
    }
    if std::env::var(rurge_interop::REQUIRED_ENV).as_deref() == Ok("1") {
        let caught = std::panic::catch_unwind(|| sing_box_or_skip("probe"));
        assert!(caught.is_err(), "a required but missing sing-box must fail");
    } else {
        assert!(sing_box_or_skip("probe").is_none());
    }
}

/// The fixture's leaf certificate and key as PEM files in `dir`.
fn leaf_files(fixture: &TlsFixture, dir: &Path) -> TlsFiles {
    let write = |name: &str, text: String| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    TlsFiles {
        certificate: write("leaf.pem", fixture.leaf_pem()),
        key: write("leaf.key", fixture.leaf_key_pem()),
        client_ca: None,
    }
}

fn trojan_inbound(tls: TlsFiles, ws_path: Option<&str>) -> Inbound {
    Inbound {
        kind: InboundKind::Trojan,
        users: vec![("u".into(), "s3same".into())],
        tls: Some(tls),
        ws_path: ws_path.map(str::to_string),
    }
}

#[tokio::test]
async fn trojan_with_and_without_websocket() {
    let Some(bin) = sing_box_or_skip("trojan_with_and_without_websocket") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![
            trojan_inbound(leaf_files(&fixture, dir.path()), None),
            trojan_inbound(leaf_files(&fixture, dir.path()), Some("/ws")),
        ],
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nPlain = trojan, 127.0.0.1, {}, password=s3same\nWs = trojan, 127.0.0.1, {}, password=s3same, ws=true, ws-path=/ws\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0),
        sb.port(1)
    );
    roundtrip(&outbound(&profile, "Plain", Some(&fixture)), echo).await;
    roundtrip(&outbound(&profile, "Ws", Some(&fixture)), echo).await;
}

#[tokio::test]
async fn a_wrong_trojan_password_is_not_relayed() {
    let Some(bin) = sing_box_or_skip("a_wrong_trojan_password_is_not_relayed") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![trojan_inbound(leaf_files(&fixture, dir.path()), None)],
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nWrong = trojan, 127.0.0.1, {}, password=nope\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0)
    );
    // the protocol has no reply, so connecting succeeds; nothing comes back
    let mut stream = outbound(&profile, "Wrong", Some(&fixture))
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .expect("connecting succeeds");
    stream.write_all(b"interop").await.unwrap();
    let mut buf = [0u8; 7];
    let got = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read_exact(&mut buf),
    )
    .await
    .expect("sing-box closes the connection (it has no fallback configured)");
    assert!(
        got.is_err() || &buf != b"interop",
        "the payload must not be echoed"
    );
}
