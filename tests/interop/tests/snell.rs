//! rurge's `snell` outbound against sing-box's `snell` inbound (`version: 5`,
//! which takes v4 clients too) and Surge's official snell-server v5.0.1
//! (Linux only), phase 2 M6 design 4.4: v4 and v5 over TCP with one record
//! and with many, `reuse=true` with requests one after another, `obfs=http`,
//! and UDP over TCP. Every target is a loopback IP literal.

mod common;

use common::*;
use rurge_interop::snell_server::{SnellServer, snell_server_or_skip};

const PSK: &str = "sn3ll-interop";

/// `name = snell, 127.0.0.1, <port>, psk=…, <params>`.
fn line(name: &str, port: u16, params: &str) -> String {
    format!("{name} = snell, 127.0.0.1, {port}, psk={PSK}, {params}\n")
}

fn profile(lines: &[String]) -> String {
    format!("[Proxy]\n{}[Rule]\nFINAL,DIRECT\n", lines.concat())
}

fn snell_inbound(obfs_http: bool) -> Inbound {
    plain(InboundKind::Snell { obfs_http }, &[("ignored", PSK)])
}

/// `count` requests one after another, each ended cleanly both ways, so
/// that with `reuse=true` each one hands its connection to the next.
/// Bounded like `roundtrip`.
async fn requests_one_after_another(out: &OutboundRef, echo: SocketAddr, count: usize) {
    let bound = std::time::Duration::from_secs(10);
    for i in 0..count {
        let mut stream = tokio::time::timeout(
            bound,
            out.connect_tcp(&target(echo), &ConnectOpts::default()),
        )
        .await
        .expect("the tunnel is established within the bound")
        .expect("the tunnel is established");
        let payload = format!("request {i}");
        let mut back = vec![0u8; payload.len()];
        let exchange = async {
            stream.write_all(payload.as_bytes()).await.unwrap();
            stream.read_exact(&mut back).await.unwrap();
            stream.shutdown().await.unwrap();
            // the target's end comes back as the server's end of the request
            let mut rest = Vec::new();
            stream.read_to_end(&mut rest).await.unwrap();
            rest
        };
        let rest = tokio::time::timeout(bound, exchange)
            .await
            .expect("the request completes within the bound");
        assert_eq!((back.as_slice(), rest.len()), (payload.as_bytes(), 0));
    }
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
