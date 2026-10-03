//! rurge's TUIC outbounds against the references (phase 2 M7 design 6.4,
//! M7-D3, M7-D6): `tuic-v5` against sing-box 1.14.2's `tuic` inbound, which
//! speaks v5 only — TCP with one chunk, with many and past a stream's
//! receive window, UDP whole and in TUIC fragments, a wrong password —, and
//! `tuic` (v4) against the official tuic 0.8.5 server, on Linux CI only —
//! TCP, UDP, a wrong token. Each server listens on a loopback UDP port with
//! a certificate for 127.0.0.1 signed by the fixture's CA, which rurge
//! trusts, and selects ALPN `h3`, rurge's default; every target is a
//! loopback IP literal. Both always take QUIC datagrams, so UDP goes in
//! `native` mode: `quic` mode (a stream per packet) is the loopback fake's
//! alone. Port hopping is not covered: the server would need a DNAT range.

mod common;

use common::*;
use rurge_interop::TuicInbound;
use rurge_interop::tuic_server::{TuicServer, tuic_server_or_skip};
use std::io;
use std::time::Duration;

/// The manual's example user.
const UUID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
const PASSWORD: &str = "tuic-interop";
const TOKEN: &str = "tuic-v4-interop";

/// What both versions say once the server refused the authentication.
const REFUSED: &str = "tuic: authentication failed";

/// sing-box's `tuic` inbound for `UUID` with `PASSWORD`, its UDP port, and
/// the fixture whose CA signed its certificate for 127.0.0.1.
fn sing_box(bin: &Path, dir: &Path) -> (SingBox, u16, Arc<TlsFixture>) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let inbound = TuicInbound {
        uuid: UUID.to_string(),
        password: PASSWORD.to_string(),
        tls: leaf_files(&fixture, dir),
    };
    let (sb, port) = SingBox::spawn_tuic(bin, dir, &inbound);
    (sb, port, fixture)
}

/// tuic 0.8.5's server letting `TOKEN` in, and the fixture whose CA signed
/// its certificate for 127.0.0.1.
fn tuic_server(bin: &Path, dir: &Path) -> (TuicServer, Arc<TlsFixture>) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let server = TuicServer::spawn(bin, dir, TOKEN, &leaf_files(&fixture, dir));
    (server, fixture)
}

/// The outbound of `name = tuic-v5, 127.0.0.1, <port>, uuid=UUID,
/// password=<password>`.
fn v5(port: u16, password: &str, fixture: &Arc<TlsFixture>) -> OutboundRef {
    let profile = format!(
        "[Proxy]\nT = tuic-v5, 127.0.0.1, {port}, uuid={UUID}, password={password}\n[Rule]\nFINAL,DIRECT\n"
    );
    outbound(&profile, "T", Some(fixture))
}

/// The outbound of `name = tuic, 127.0.0.1, <port>, token=<token>`.
fn v4(port: u16, token: &str, fixture: &Arc<TlsFixture>) -> OutboundRef {
    let profile =
        format!("[Proxy]\nT = tuic, 127.0.0.1, {port}, token={token}\n[Rule]\nFINAL,DIRECT\n");
    outbound(&profile, "T", Some(fixture))
}

/// Datagrams to `echo` through a fresh carrier of `out`, one after another
/// until a send fails, and that failure. Nothing answers the
/// authentication, so the carrier comes up, and its packets go until the
/// server's refusal closes the connection. Bounded like `roundtrip`.
async fn udp_until_refused(out: &OutboundRef, echo: SocketAddr) -> io::Error {
    let bound = Duration::from_secs(10);
    let carrier = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
        .await
        .expect("the carrier opens within the bound")
        .expect("the carrier opens: nothing answers the authentication");
    let deadline = tokio::time::Instant::now() + bound;
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no send failed within the bound"
        );
        match carrier.send_to(b"interop", &target(echo)).await {
            Ok(()) => tokio::time::sleep(Duration::from_millis(20)).await,
            Err(e) => return e,
        }
    }
}

/// One chunk, many, then 9 MiB each way at once: past rurge's receive window
/// of a stream (8 MiB, phase 2 M7 design 3.2), what comes back needs the
/// credit rurge grants as it reads.
#[tokio::test]
async fn v5_tcp_against_sing_box() {
    let Some(bin) = sing_box_or_skip("v5_tcp_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = sing_box(&bin, dir.path());
    let (out, echo) = (v5(port, PASSWORD, &fixture), echo_server().await);
    roundtrip(&out, echo).await;
    roundtrip_big(&out, echo).await;
    transfer(&out, echo, (9 << 20) + 777).await;
}

/// Two associations side by side; their 3000 bytes go in TUIC fragments
/// both ways (sing-box splits its answers into datagrams of at most 1197
/// bytes).
#[tokio::test]
async fn v5_udp_against_sing_box() {
    let Some(bin) = sing_box_or_skip("v5_udp_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (_sb, port, fixture) = sing_box(&bin, dir.path());
    let out = v5(port, PASSWORD, &fixture);
    udp_side_by_side(&out, udp_echo_server().await).await;
}

/// sing-box answers no v5 authentication. It closes a connection whose token
/// does not match — code 0 and no reason, as it closes one for any failure
/// —, and often ends a Connect it holds first (STOP_SENDING 0 and a FIN).
/// So the tunnel comes up, as every v5 tunnel does at once, and then ends
/// with nothing relayed: on `tuic: authentication failed` where the close
/// comes first, on the end of the stream where the Connect's end does, and
/// a write may meet `the server stopped the stream`. UDP knows no such
/// race: the send that finds the connection closed so fails with `tuic:
/// authentication failed`.
#[tokio::test]
async fn v5_a_wrong_password_against_sing_box() {
    let Some(bin) = sing_box_or_skip("v5_a_wrong_password_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (sb, port, fixture) = sing_box(&bin, dir.path());
    let out = v5(port, "nope", &fixture);
    let bound = Duration::from_secs(10);
    let tunnel = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo_server().await), &ConnectOpts::default()),
    )
    .await
    .expect("the tunnel comes up or fails within the bound");
    match tunnel {
        // the refusal was in before the Connect went out
        Err(err) => assert_eq!(err.to_string(), REFUSED, "{}", sb.log_text()),
        Ok(mut tunnel) => {
            let write = tokio::time::timeout(bound, tunnel.write_all(b"interop"))
                .await
                .expect("the write ends within the bound");
            if let Err(e) = write {
                let text = e.to_string();
                assert!(
                    text == REFUSED || text == "tuic: the server stopped the stream (code 0x0)",
                    "{text}\n{}",
                    sb.log_text()
                );
            }
            let mut buf = [0u8; 7];
            let read = tokio::time::timeout(bound, tunnel.read(&mut buf))
                .await
                .expect("the tunnel ends within the bound");
            match read {
                Ok(n) => assert_eq!(
                    n,
                    0,
                    "a wrong password must relay nothing\n{}",
                    sb.log_text()
                ),
                Err(e) => assert_eq!(e.to_string(), REFUSED, "{}", sb.log_text()),
            }
        }
    }
    let err = udp_until_refused(&out, udp_echo_server().await).await;
    assert_eq!(
        err.kind(),
        io::ErrorKind::PermissionDenied,
        "{}",
        sb.log_text()
    );
    assert_eq!(err.to_string(), REFUSED, "{}", sb.log_text());
}

/// One chunk and many, each tunnel once the server answered its Connect
/// (`04 ff 00`).
#[tokio::test]
async fn v4_tcp_against_tuic_server() {
    let Some(bin) = tuic_server_or_skip("v4_tcp_against_tuic_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = tuic_server(&bin, dir.path());
    let (out, echo) = (v4(server.port(), TOKEN, &fixture), echo_server().await);
    roundtrip(&out, echo).await;
    roundtrip_big(&out, echo).await;
}

/// Whole packets in QUIC datagrams (v4 has no fragments). The server names
/// an IPv4 source in its IPv4-mapped IPv6 form, as its one dual-stack socket
/// reports it; rurge reads it as the IPv4 address `udp_roundtrip` expects.
#[tokio::test]
async fn v4_udp_against_tuic_server() {
    let Some(bin) = tuic_server_or_skip("v4_udp_against_tuic_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = tuic_server(&bin, dir.path());
    let out = v4(server.port(), TOKEN, &fixture);
    udp_roundtrip(&out, udp_echo_server().await).await;
}

/// tuic 0.8.5 closes a connection whose token it does not know with the
/// code for it (0xfffffff1): the Connect, which waits for its response,
/// fails with `tuic: authentication failed`, and so does the UDP send that
/// finds the connection closed so.
#[tokio::test]
async fn v4_a_wrong_token_against_tuic_server() {
    let Some(bin) = tuic_server_or_skip("v4_a_wrong_token_against_tuic_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = tuic_server(&bin, dir.path());
    let out = v4(server.port(), "nope", &fixture);
    let tcp = tokio::time::timeout(
        Duration::from_secs(10),
        out.connect_tcp(&target(echo_server().await), &ConnectOpts::default()),
    )
    .await
    .expect("the server answers within the bound");
    let Err(tcp) = tcp else {
        panic!(
            "a wrong token must not open a tunnel\n{}",
            server.log_text()
        );
    };
    assert!(
        matches!(&tcp, OutboundError::Io(e) if e.kind() == io::ErrorKind::PermissionDenied),
        "{tcp:?}\n{}",
        server.log_text()
    );
    assert_eq!(tcp.to_string(), REFUSED, "{}", server.log_text());
    let udp = udp_until_refused(&out, udp_echo_server().await).await;
    assert_eq!(
        udp.kind(),
        io::ErrorKind::PermissionDenied,
        "{}",
        server.log_text()
    );
    assert_eq!(udp.to_string(), REFUSED, "{}", server.log_text());
}
