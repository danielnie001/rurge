//! rurge's `hysteria2` outbound against sing-box 1.14.2's `hysteria2`
//! inbound (phase 2 M7 design 5.5, M7-D6): TCP with one chunk, with many
//! and past a stream's receive window, UDP whole and in Hysteria fragments,
//! a wrong password, and either obfuscation (Salamander, Gecko). sing-box
//! listens on a loopback UDP port with a certificate for 127.0.0.1 signed by
//! the fixture's CA, which rurge trusts; every target is a loopback IP
//! literal. Port hopping is not covered: the server would need a DNAT range.

mod common;

use common::*;
use rurge_interop::{Hysteria2Inbound, Hysteria2Obfs};
use std::time::Duration;

const PASSWORD: &str = "hy2-interop";
const OBFS_PASSWORD: &str = "obfs-interop";

/// sing-box's `hysteria2` inbound with `obfs`, its UDP port, and the fixture
/// whose CA signed its certificate for 127.0.0.1.
fn server(bin: &Path, dir: &Path, obfs: Option<Hysteria2Obfs>) -> (SingBox, u16, Arc<TlsFixture>) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let inbound = Hysteria2Inbound {
        password: PASSWORD.to_string(),
        obfs: obfs.map(|kind| (kind, OBFS_PASSWORD.to_string())),
        tls: leaf_files(&fixture, dir),
    };
    let (sb, port) = SingBox::spawn_hysteria2(bin, dir, &inbound);
    (sb, port, fixture)
}

/// `name = hysteria2, 127.0.0.1, <port>, <params>`.
fn line(name: &str, port: u16, params: &str) -> String {
    format!("{name} = hysteria2, 127.0.0.1, {port}, {params}\n")
}

fn profile(lines: &[String]) -> String {
    format!("[Proxy]\n{}[Rule]\nFINAL,DIRECT\n", lines.concat())
}

/// TCP with one chunk and with many, then UDP, on the policy's one
/// connection.
async fn tcp_and_udp(out: &OutboundRef) {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    roundtrip(out, echo).await;
    roundtrip_big(out, echo).await;
    udp_side_by_side(out, udp_echo).await;
}

/// Past rurge's receive window of a stream (8 MiB, phase 2 M7 design 3.2):
/// what comes back needs the credit rurge grants as it reads. With
/// `download-bandwidth` sing-box sends to rurge at that rate (Brutal)
/// instead of finding one (BBR).
#[tokio::test]
async fn tcp_against_sing_box() {
    let Some(bin) = sing_box_or_skip("tcp_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = server(&bin, dir.path(), None);
    let profile = profile(&[
        line("Plain", port, &format!("password={PASSWORD}")),
        line(
            "Rx",
            port,
            &format!("password={PASSWORD}, download-bandwidth=100"),
        ),
    ]);
    let echo = echo_server().await;
    for name in ["Plain", "Rx"] {
        let out = outbound(&profile, name, Some(&fixture));
        roundtrip(&out, echo).await;
        roundtrip_big(&out, echo).await;
        transfer(&out, echo, (9 << 20) + 777).await;
    }
}

/// Two sessions side by side; their 3000 bytes go in Hysteria fragments
/// both ways (sing-box cuts its answers at about 1200 bytes and sends no
/// message past 4096).
#[tokio::test]
async fn udp_against_sing_box() {
    let Some(bin) = sing_box_or_skip("udp_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = server(&bin, dir.path(), None);
    let profile = profile(&[line("H", port, &format!("password={PASSWORD}"))]);
    let out = outbound(&profile, "H", Some(&fixture));
    udp_side_by_side(&out, udp_echo_server().await).await;
}

/// sing-box answers a wrong password as its masquerade would — 404 without
/// one —: the authentication failed, for TCP and UDP alike.
#[tokio::test]
async fn a_wrong_password_fails_the_authentication_against_sing_box() {
    let Some(bin) = sing_box_or_skip("a_wrong_password_fails_the_authentication_against_sing_box")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = server(&bin, dir.path(), None);
    let profile = profile(&[line("Wrong", port, "password=nope")]);
    let out = outbound(&profile, "Wrong", Some(&fixture));
    let bound = Duration::from_secs(10);
    let tcp = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo_server().await), &ConnectOpts::default()),
    )
    .await
    .expect("sing-box answers within the bound");
    let Err(tcp) = tcp else {
        panic!("a wrong password must not open a tunnel");
    };
    let udp = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
        .await
        .expect("sing-box answers within the bound");
    let Err(udp) = udp else {
        panic!("a wrong password must not open a UDP carrier");
    };
    for err in [tcp, udp] {
        assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
        assert_eq!(err.to_string(), "hysteria2: authentication failed");
    }
}

/// Salamander over every packet (`obfs.type: salamander`).
#[tokio::test]
async fn salamander_against_sing_box() {
    let Some(bin) = sing_box_or_skip("salamander_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = server(&bin, dir.path(), Some(Hysteria2Obfs::Salamander));
    let profile = profile(&[line(
        "H",
        port,
        &format!("password={PASSWORD}, salamander-password={OBFS_PASSWORD}"),
    )]);
    tcp_and_udp(&outbound(&profile, "H", Some(&fixture))).await;
}

/// Gecko (`obfs.type: gecko`, sing-box 1.14.0 and later): the handshake's
/// packets in fragments both ways, every packet salted; sing-box's default
/// packet sizes (512–1200 bytes) are rurge's.
#[tokio::test]
async fn gecko_against_sing_box() {
    let Some(bin) = sing_box_or_skip("gecko_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = server(&bin, dir.path(), Some(Hysteria2Obfs::Gecko));
    let profile = profile(&[line(
        "H",
        port,
        &format!("password={PASSWORD}, gecko-password={OBFS_PASSWORD}"),
    )]);
    tcp_and_udp(&outbound(&profile, "H", Some(&fixture))).await;
}
