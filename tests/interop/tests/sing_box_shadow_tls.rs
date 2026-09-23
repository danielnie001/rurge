//! Shadow TLS against sing-box: a `shadowtls` inbound that relays the
//! handshake to a TLS server of ours on the loopback (the camouflage site)
//! and hands what it unwraps to a `trojan` inbound. The same helpers as
//! `sing_box.rs`.

mod common;

use common::*;
use rurge_proto::testing::Camouflage;

const SITE: &str = "site.test";

/// sing-box with a `shadowtls` inbound (index 0) in front of a `trojan`
/// inbound (index 1), and the site the handshake is borrowed from.
async fn shadow_tls_in_front_of_trojan(
    bin: &Path,
    dir: &Path,
    fixture: &Arc<TlsFixture>,
    version: u8,
    tickets: usize,
) -> (SingBox, Camouflage) {
    let site = Camouflage::spawn(fixture, &[&rustls::version::TLS13], tickets).await;
    let front = Inbound {
        kind: InboundKind::ShadowTls {
            version,
            handshake_port: site.addr().port(),
            detour: 1,
        },
        users: vec![("u".into(), "st-pw".into())],
        tls: None,
        ws_path: None,
    };
    let sb = SingBox::spawn(
        bin,
        dir,
        vec![front, trojan_inbound(leaf_files(fixture, dir), None)],
    );
    (sb, site)
}

fn profile(port: u16, version: u8, password: &str) -> String {
    format!(
        "[Proxy]\nT = trojan, 127.0.0.1, {port}, password=s3same, shadow-tls-password={password}, shadow-tls-version={version}, shadow-tls-sni={SITE}\n[Rule]\nFINAL,DIRECT\n"
    )
}

#[tokio::test]
async fn trojan_behind_shadow_tls_v2_and_v3() {
    let Some(bin) = sing_box_or_skip("trojan_behind_shadow_tls_v2_and_v3") else {
        return;
    };
    // v2: sing-box compares the client's digest with its own as it stands
    // now or stood one write ago, so the site sends no session tickets here;
    // v3 has no such race, and its tickets exercise the records sing-box is
    // still relaying when the data phase begins
    for (version, tickets) in [(2u8, 0usize), (3, 2)] {
        let dir = tempfile::tempdir().unwrap();
        let fixture = TlsFixture::new(&["127.0.0.1", SITE]);
        let (sb, _site) =
            shadow_tls_in_front_of_trojan(&bin, dir.path(), &fixture, version, tickets).await;
        let echo = echo_server().await;
        let out = outbound(&profile(sb.port(0), version, "st-pw"), "T", Some(&fixture));
        roundtrip(&out, echo).await;
        // frames in both directions, well past one record
        roundtrip_big(&out, echo).await;
        // the site saw one handshake per connection, with the configured name
        let seen = fixture.seen_at_least(2).await;
        assert_eq!(seen[0].sni.as_deref(), Some(SITE), "version {version}");
    }
}

#[tokio::test]
async fn a_wrong_shadow_tls_password_is_not_relayed() {
    let Some(bin) = sing_box_or_skip("a_wrong_shadow_tls_password_is_not_relayed") else {
        return;
    };
    let bound = std::time::Duration::from_secs(15);
    for version in [2u8, 3] {
        let dir = tempfile::tempdir().unwrap();
        let fixture = TlsFixture::new(&["127.0.0.1", SITE]);
        let (sb, site) =
            shadow_tls_in_front_of_trojan(&bin, dir.path(), &fixture, version, 0).await;
        let echo = echo_server().await;
        let out = outbound(
            &profile(sb.port(0), version, "an0ther"),
            "T",
            Some(&fixture),
        );
        let err = tokio::time::timeout(
            bound,
            out.connect_tcp(&target(echo), &ConnectOpts::default()),
        )
        .await
        .expect("refused within the bound")
        .err()
        .expect("sing-box relays a stranger to the site, never to trojan");
        let text = err.to_string();
        assert!(!text.contains("an0ther"), "{text}");
        if version == 3 {
            // v3 notices during the handshake, and leaves like a visitor
            assert_eq!(text, "shadow-tls: the server did not authenticate itself");
            assert!(site.received().starts_with(b"GET / HTTP/1.1\r\n"));
        }
        assert!(
            matches!(
                err,
                OutboundError::Proxy(_) | OutboundError::Tls(_) | OutboundError::Io(_)
            ),
            "{err:?}"
        );
    }
}
