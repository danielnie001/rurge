//! rurge's `snell` outbound against sing-box's `snell` inbound (`version: 5`,
//! which takes v4 clients too) and Surge's official snell-server v5.0.1
//! (Linux only), phase 2 M6 design 4.4: v4 and v5 over TCP with one record
//! and with many, `reuse=true` with requests one after another, `obfs=http`,
//! and UDP over TCP; and what a line that is not the server's — of another
//! version, or of another PSK — meets there (phase 2 M8 design 4.3). Every
//! target is a loopback IP literal.

mod common;

use common::*;
use rurge_interop::snell_server::{SnellServer, snell_server_or_skip};

const PSK: &str = "sn3ll-interop";
const OTHER_PSK: &str = "an0ther-psk";

/// `name = snell, 127.0.0.1, <port>, psk=…, <params>`.
fn line(name: &str, port: u16, params: &str) -> String {
    format!("{name} = snell, 127.0.0.1, {port}, psk={PSK}, {params}\n")
}

fn profile(lines: &[String]) -> String {
    format!("[Proxy]\n{}[Rule]\nFINAL,DIRECT\n", lines.concat())
}

/// The policies of `mismatched`.
const MISMATCHED: [&str; 2] = ["NoVersion", "OtherPsk"];

/// Two lines a server of versions 4 and 5 on `port` cannot read. One names
/// no version: that is version 1, Surge's default, whose chunks are not
/// these servers' records — the line of somebody who never wrote `version`
/// against a server of today. The other is in the server's own format under
/// another PSK.
fn mismatched(port: u16) -> String {
    profile(&[
        format!("NoVersion = snell, 127.0.0.1, {port}, psk={PSK}\n"),
        format!("OtherPsk = snell, 127.0.0.1, {port}, psk={OTHER_PSK}, version=4\n"),
    ])
}

fn snell_inbound(obfs_http: bool) -> Inbound {
    plain(InboundKind::Snell { obfs_http }, &[("ignored", PSK)])
}

/// Each policy: one small and one large TCP round trip.
async fn tcp(profile: &str, names: &[&str]) {
    let echo = echo_server().await;
    for name in names {
        let out = outbound(profile, name, None);
        roundtrip(&out, echo).await;
        roundtrip_big(&out, echo).await;
    }
}

/// Each policy: a UDP carrier, two datagrams there and back.
async fn udp(profile: &str, names: &[&str]) {
    let echo = udp_echo_server().await;
    for name in names {
        udp_roundtrip(&outbound(profile, name, None), echo).await;
    }
}

#[tokio::test]
async fn v5_and_v4_clients_against_sing_box() {
    let Some(bin) = sing_box_or_skip("v5_and_v4_clients_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![snell_inbound(false)]);
    let profile = profile(&[
        line("V5", sb.port(0), "version=5"),
        line("V4", sb.port(0), "version=4"),
    ]);
    tcp(&profile, &["V5", "V4"]).await;
}

#[tokio::test]
async fn reuse_against_sing_box() {
    let Some(bin) = sing_box_or_skip("reuse_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![snell_inbound(false)]);
    let profile = profile(&[line("Reuse", sb.port(0), "version=5, reuse=true")]);
    let (out, echo) = (outbound(&profile, "Reuse", None), echo_server().await);
    requests_one_after_another(&out, echo, 4).await;
    roundtrip_big(&out, echo).await;
}

#[tokio::test]
async fn obfs_http_against_sing_box() {
    let Some(bin) = sing_box_or_skip("obfs_http_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![snell_inbound(true)]);
    let profile = profile(&[
        line("Obfs", sb.port(0), "version=5, obfs=http"),
        line("ObfsReuse", sb.port(0), "version=5, obfs=http, reuse=true"),
    ]);
    tcp(&profile, &["Obfs"]).await;
    let out = outbound(&profile, "ObfsReuse", None);
    requests_one_after_another(&out, echo_server().await, 3).await;
    udp(&profile, &["Obfs"]).await;
}

#[tokio::test]
async fn udp_against_sing_box() {
    let Some(bin) = sing_box_or_skip("udp_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![snell_inbound(false)]);
    let profile = profile(&[
        line("V5", sb.port(0), "version=5"),
        line("V4", sb.port(0), "version=4"),
    ]);
    udp(&profile, &["V5", "V4"]).await;
}

/// What sing-box does with a request it cannot read is this case's to tell.
/// A server that closes the connection, or resets it, fails the session at
/// once with one of the two sentences `none_is_answered` takes; one that
/// kept the connection and said nothing would leave the session to the
/// idle timeout, and fails the case with what happened instead and with
/// sing-box's log. Both lines are tried, whatever the first met.
#[tokio::test]
async fn a_line_of_another_version_or_psk_against_sing_box() {
    let Some(bin) = sing_box_or_skip("a_line_of_another_version_or_psk_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![snell_inbound(false)]);
    let mut failures = Failures::default();
    let profile = mismatched(sb.port(0));
    none_is_answered(&mut failures, &profile, &MISMATCHED, UNANSWERED_BOUND).await;
    failures.finish(|| sb.log_text());
}

#[tokio::test]
async fn v5_and_v4_clients_against_snell_server() {
    let Some(bin) = snell_server_or_skip("v5_and_v4_clients_against_snell_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = SnellServer::spawn(&bin, dir.path(), PSK, false);
    let profile = profile(&[
        line("V5", server.port(), "version=5"),
        line("V4", server.port(), "version=4"),
    ]);
    tcp(&profile, &["V5", "V4"]).await;
}

#[tokio::test]
async fn reuse_against_snell_server() {
    let Some(bin) = snell_server_or_skip("reuse_against_snell_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = SnellServer::spawn(&bin, dir.path(), PSK, false);
    let profile = profile(&[line("Reuse", server.port(), "version=5, reuse=true")]);
    let (out, echo) = (outbound(&profile, "Reuse", None), echo_server().await);
    requests_one_after_another(&out, echo, 4).await;
    roundtrip_big(&out, echo).await;
}

#[tokio::test]
async fn obfs_http_against_snell_server() {
    let Some(bin) = snell_server_or_skip("obfs_http_against_snell_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = SnellServer::spawn(&bin, dir.path(), PSK, true);
    let profile = profile(&[
        line("Obfs", server.port(), "version=5, obfs=http"),
        line(
            "ObfsReuse",
            server.port(),
            "version=5, obfs=http, reuse=true",
        ),
    ]);
    tcp(&profile, &["Obfs"]).await;
    let out = outbound(&profile, "ObfsReuse", None);
    requests_one_after_another(&out, echo_server().await, 3).await;
    udp(&profile, &["Obfs"]).await;
}

#[tokio::test]
async fn udp_against_snell_server() {
    let Some(bin) = snell_server_or_skip("udp_against_snell_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = SnellServer::spawn(&bin, dir.path(), PSK, false);
    let profile = profile(&[
        line("V5", server.port(), "version=5"),
        line("V4", server.port(), "version=4"),
    ]);
    udp(&profile, &["V5", "V4"]).await;
}

/// The same two lines against the official server, whose source nobody can
/// read: what it does with them, only a run tells.
#[tokio::test]
async fn a_line_of_another_version_or_psk_against_snell_server() {
    let Some(bin) = snell_server_or_skip("a_line_of_another_version_or_psk_against_snell_server")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = SnellServer::spawn(&bin, dir.path(), PSK, false);
    let mut failures = Failures::default();
    let profile = mismatched(server.port());
    none_is_answered(&mut failures, &profile, &MISMATCHED, UNANSWERED_BOUND).await;
    failures.finish(|| server.log_text());
}

/// What those two cases write on rurge's side, checked without a server:
/// the lines load with nothing said about them, and their outbounds build.
#[tokio::test]
async fn the_mismatched_lines_load_and_build() {
    loads_and_builds(&mismatched(4000), &MISMATCHED);
}
