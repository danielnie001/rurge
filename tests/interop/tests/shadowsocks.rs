//! rurge's `ss` outbound against shadowsocks-rust's `ssserver` and sing-box's
//! `shadowsocks` inbound (phase 2 M6 design 3.4, M6-D5): TCP with one chunk
//! and with many, and UDP, for the AEAD and the 2022 methods, the latter
//! also as one of several users. Every target is a loopback IP literal.

mod common;

use common::*;
use rurge_interop::shadowsocks_rust::{SsInbound, Ssserver, ssserver_or_skip};

const SERVER_16: &str = "MDEyMzQ1Njc4OWFiY2RlZg==";
const USER_16: &str = "ZmVkY2JhOTg3NjU0MzIxMA==";
const SERVER_32: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
const USER_32: &str = "ZmVkY2JhOTg3NjU0MzIxMGZlZGNiYTk4NzY1NDMyMTA=";
const OTHER_16: &str = "dGhlIG90aGVyIHVzZXIhIQ==";
const OTHER_32: &str = "dGhlIG90aGVyIHVzZXIgaGFzIDMyIGJ5dGVzLCB0b28=";

/// `name = ss, 127.0.0.1, <port>, …` with UDP on.
fn line(name: &str, port: u16, method: &str, password: &str) -> String {
    format!(
        "{name} = ss, 127.0.0.1, {port}, encrypt-method={method}, password={password}, udp-relay=true\n"
    )
}

/// Each policy of `profile`: one small and one large TCP round trip, then UDP.
async fn every_way(profile: &str, names: &[&str]) {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    for name in names {
        let out = outbound(profile, name, None);
        roundtrip(&out, echo).await;
        roundtrip_big(&out, echo).await;
        udp_roundtrip(&out, udp_echo).await;
    }
}

fn server(method: &'static str, password: &str) -> SsInbound {
    SsInbound {
        method,
        password: password.into(),
        users: Vec::new(),
    }
}

/// The AEAD methods of ssserver's release build (`aes-192-gcm` and
/// `xchacha20-ietf-poly1305` are not in it; sing-box covers them below).
#[tokio::test]
async fn aead_methods_against_ssserver() {
    let Some(bin) = ssserver_or_skip("aead_methods_against_ssserver") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let methods = ["aes-128-gcm", "aes-256-gcm", "chacha20-ietf-poly1305"];
    let ss = Ssserver::spawn(
        &bin,
        dir.path(),
        methods.iter().map(|m| server(m, "s3same")).collect(),
    );
    let profile = format!(
        "[Proxy]\n{}{}{}[Rule]\nFINAL,DIRECT\n",
        line("A128", ss.port(0), methods[0], "s3same"),
        line("A256", ss.port(1), methods[1], "s3same"),
        line("Chacha", ss.port(2), methods[2], "s3same"),
    );
    every_way(&profile, &["A128", "A256", "Chacha"]).await;
}

/// Both 2022 methods with a single key, and `2022-blake3-aes-256-gcm` as the
/// second of two users (`serverKey:userKey`, one identity header).
#[tokio::test]
async fn ss_2022_against_ssserver_with_one_key_and_as_a_user() {
    let Some(bin) = ssserver_or_skip("ss_2022_against_ssserver_with_one_key_and_as_a_user") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let ss = Ssserver::spawn(
        &bin,
        dir.path(),
        vec![
            server("2022-blake3-aes-128-gcm", SERVER_16),
            server("2022-blake3-aes-256-gcm", SERVER_32),
            SsInbound {
                method: "2022-blake3-aes-256-gcm",
                password: SERVER_32.into(),
                users: vec![
                    ("other".into(), OTHER_32.into()),
                    ("u".into(), USER_32.into()),
                ],
            },
        ],
    );
    let profile = format!(
        "[Proxy]\n{}{}{}[Rule]\nFINAL,DIRECT\n",
        line("B128", ss.port(0), "2022-blake3-aes-128-gcm", SERVER_16),
        line("B256", ss.port(1), "2022-blake3-aes-256-gcm", SERVER_32),
        line(
            "User",
            ss.port(2),
            "2022-blake3-aes-256-gcm",
            &format!("{SERVER_32}:{USER_32}")
        ),
    );
    every_way(&profile, &["B128", "B256", "User"]).await;
}

fn ss_inbound(method: &'static str, password: &str, users: &[&str]) -> Inbound {
    let mut all = vec![("ignored".to_string(), password.to_string())];
    all.extend(
        users
            .iter()
            .enumerate()
            .map(|(i, key)| (format!("u{i}"), key.to_string())),
    );
    Inbound {
        kind: InboundKind::Shadowsocks { method },
        users: all,
        tls: None,
        ws_path: None,
    }
}

/// sing-box's `shadowsocks` inbound: the two AEAD methods ssserver's release
/// build lacks, a 2022 method with a single key and one as the second of two users.
#[tokio::test]
async fn ss_against_sing_box_aead_2022_and_a_user() {
    let Some(bin) = sing_box_or_skip("ss_against_sing_box_aead_2022_and_a_user") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![
            ss_inbound("aes-192-gcm", "s3same", &[]),
            ss_inbound("xchacha20-ietf-poly1305", "s3same", &[]),
            ss_inbound("2022-blake3-aes-256-gcm", SERVER_32, &[]),
            ss_inbound("2022-blake3-aes-128-gcm", SERVER_16, &[OTHER_16, USER_16]),
        ],
    );
    let profile = format!(
        "[Proxy]\n{}{}{}{}[Rule]\nFINAL,DIRECT\n",
        line("A192", sb.port(0), "aes-192-gcm", "s3same"),
        line("XChacha", sb.port(1), "xchacha20-ietf-poly1305", "s3same"),
        line("B256", sb.port(2), "2022-blake3-aes-256-gcm", SERVER_32),
        line(
            "User",
            sb.port(3),
            "2022-blake3-aes-128-gcm",
            &format!("{SERVER_16}:{USER_16}")
        ),
    );
    every_way(&profile, &["A192", "XChacha", "B256", "User"]).await;
}
