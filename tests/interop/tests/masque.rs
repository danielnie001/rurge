//! rurge's `masque` outbound against masque-server v0.13.0 (phase 2 M7
//! design 7.1, 7.4; M7-D6). The server is built for Linux alone, so the
//! cases that need it run on Linux only, where CI installs it; the
//! self-test of the paced transfer needs no server and runs everywhere.
//! The cases: TCP as classic CONNECTs (one chunk, many, and 9 MiB each way
//! at once), UDP as CONNECT-UDP with the packets in HTTP/3 datagrams (two
//! targets of one carrier, a stream for each), and a wrong password, which
//! the server refuses a tunnel and a CONNECT-UDP stream alike. The server
//! listens on a loopback UDP port with a certificate for 127.0.0.1 signed by
//! the fixture's CA, which rurge trusts; every target is a loopback IP
//! literal, which the server's target policy is told to allow. Port hopping
//! is not covered: the server would need a DNAT range.

mod common;

use common::*;
use rurge_interop::masque_server::{MasqueServer, PASSWORD, USERNAME, masque_server_or_skip};
use std::time::Duration;

/// What a refused login says, for a tunnel and a CONNECT-UDP stream alike.
const REFUSED: &str = "masque: proxy authentication required";

/// masque-server with a certificate for 127.0.0.1, and the fixture whose CA
/// signed it.
fn masque_server(bin: &Path, dir: &Path) -> (MasqueServer, Arc<TlsFixture>) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let server = MasqueServer::spawn(bin, dir, &leaf_files(&fixture, dir));
    (server, fixture)
}

/// The outbound of `M = masque, 127.0.0.1, <port>, username=test,
/// password=<password>`.
fn masque(port: u16, password: &str, fixture: &Arc<TlsFixture>) -> OutboundRef {
    let profile = format!(
        "[Proxy]\nM = masque, 127.0.0.1, {port}, username={USERNAME}, password={password}\n[Rule]\nFINAL,DIRECT\n"
    );
    outbound(&profile, "M", Some(fixture))
}

/// One carrier, two targets, a few rounds: each target gets a CONNECT-UDP
/// stream of its own with its first packet, and each answer comes from the
/// target it went to. `log` is the server's. Bounded like `roundtrip`.
async fn udp_two_targets(out: &OutboundRef, echoes: [SocketAddr; 2], log: &dyn Fn() -> String) {
    let bound = Duration::from_secs(10);
    let carrier = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
        .await
        .expect("the carrier opens within the bound")
        .expect("the carrier opens");
    for round in 0..3u8 {
        for (n, echo) in (0u8..).zip(echoes) {
            let payload = [b'i', round, n];
            carrier
                .send_to(&payload, &target(echo))
                .await
                .unwrap_or_else(|e| panic!("{e}\n{}", log()));
            let mut buf = vec![0u8; 65536];
            let (len, from) = tokio::time::timeout(bound, carrier.recv_from(&mut buf))
                .await
                .unwrap_or_else(|_| panic!("the echo comes back within the bound\n{}", log()))
                .unwrap();
            assert_eq!(
                (&buf[..len], canonical(from)),
                (&payload[..], canonical(target(echo))),
                "{}",
                log()
            );
        }
    }
}

/// `len` bytes each way at once through one tunnel, as `common::transfer`
/// sends them, except that the upload never runs more than 512 KiB ahead of
/// what has come back. masque-server (v0.13.0, MIT) keeps, for each tunnel,
/// the bytes the client sent and it has not yet written to the target, at most
/// `MAX_BUFFERED_CLIENT_BYTES` (1 MiB); it puts no backpressure on QUIC, and
/// past that bound it resets the stream with H3_CONNECT_ERROR. An upload that
/// only QUIC's flow-control windows hold back (the server grants 4 to 16 MiB
/// for each stream) meets that reset as soon as the target is slow to take
/// it. The target here is an echo, so what has come back is what the target
/// has taken: the bytes the server holds are at most the 512 KiB sent and not
/// yet read back. Bounded by a minute; `log` is the server's.
async fn paced_transfer(out: &OutboundRef, echo: SocketAddr, len: usize, log: &dyn Fn() -> String) {
    let bound = Duration::from_secs(60);
    let payload: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
    let stream = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo), &ConnectOpts::default()),
    )
    .await
    .unwrap_or_else(|_| panic!("the tunnel is established within the bound\n{}", log()))
    .unwrap_or_else(|e| panic!("the tunnel is established: {e}\n{}", log()));
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut back = vec![0u8; len];
    // what the upload may still send before the echo of earlier bytes is read
    let room = tokio::sync::Semaphore::new(512 * 1024);
    let upload = async {
        for chunk in payload.chunks(64 * 1024) {
            room.acquire_many(chunk.len() as u32)
                .await
                .unwrap()
                .forget();
            writer
                .write_all(chunk)
                .await
                .unwrap_or_else(|e| panic!("the upload fails: {e}\n{}", log()));
        }
    };
    let download = async {
        let mut filled = 0;
        while filled < len {
            let n = reader
                .read(&mut back[filled..])
                .await
                .unwrap_or_else(|e| panic!("the echo fails after {filled} bytes: {e}\n{}", log()));
            assert!(
                n > 0,
                "the tunnel ends after {filled} of {len} bytes\n{}",
                log()
            );
            filled += n;
            room.add_permits(n);
        }
    };
    tokio::time::timeout(bound, async { tokio::join!(upload, download) })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the payload makes it there and back within the bound\n{}",
                log()
            )
        });
    assert!(back == payload, "the echo differs\n{}", log());
}

/// One chunk, many, then 9 MiB each way at once, the upload paced (see
/// `paced_transfer`): past rurge's receive window of a stream (8 MiB, phase 2
/// M7 design 3.2), what comes back needs the credit rurge grants as it reads
/// the tunnel's DATA frames.
#[tokio::test]
async fn tcp_against_masque_server() {
    let Some(bin) = masque_server_or_skip("tcp_against_masque_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = masque_server(&bin, dir.path());
    let (out, echo) = (
        masque(server.port(), PASSWORD, &fixture),
        echo_server().await,
    );
    roundtrip(&out, echo).await;
    roundtrip_big(&out, echo).await;
    paced_transfer(&out, echo, (9 << 20) + 777, &|| server.log_text()).await;
}

/// The paced transfer itself, with no server to ask: through a plain echo it
/// ends with the bytes it began with, the last chunk being a partial one.
#[tokio::test]
async fn the_paced_transfer_ends_with_the_bytes_it_began_with() {
    let out = outbound("[Proxy]\nD = direct\n[Rule]\nFINAL,DIRECT\n", "D", None);
    paced_transfer(&out, echo_server().await, (3 << 20) + 777, &String::new).await;
}

/// The server announces extended CONNECT and HTTP datagrams; it reads no
/// capsule on HTTP/3, and rurge sends none.
#[tokio::test]
async fn udp_against_masque_server() {
    let Some(bin) = masque_server_or_skip("udp_against_masque_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = masque_server(&bin, dir.path());
    let out = masque(server.port(), PASSWORD, &fixture);
    let echoes = [udp_echo_server().await, udp_echo_server().await];
    udp_two_targets(&out, echoes, &|| server.log_text()).await;
}

/// The server answers a CONNECT without the right credentials 407: the
/// tunnel fails, and so does the first packet to a target, whose
/// CONNECT-UDP stream is refused alike.
#[tokio::test]
async fn a_wrong_masque_password_is_refused() {
    let Some(bin) = masque_server_or_skip("a_wrong_masque_password_is_refused") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = masque_server(&bin, dir.path());
    let out = masque(server.port(), "nope", &fixture);
    let bound = Duration::from_secs(10);
    let tcp = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo_server().await), &ConnectOpts::default()),
    )
    .await
    .unwrap_or_else(|_| panic!("the server answers within the bound\n{}", server.log_text()));
    let Err(tcp) = tcp else {
        panic!(
            "a wrong password must not open a tunnel\n{}",
            server.log_text()
        );
    };
    assert_eq!(tcp.to_string(), REFUSED, "{}", server.log_text());
    // the carrier opens nothing until its first packet
    let carrier = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
        .await
        .expect("the carrier opens within the bound")
        .expect("the carrier opens");
    let udp = tokio::time::timeout(
        bound,
        carrier.send_to(b"interop", &target(udp_echo_server().await)),
    )
    .await
    .unwrap_or_else(|_| panic!("the server answers within the bound\n{}", server.log_text()));
    let Err(udp) = udp else {
        panic!(
            "a wrong password must not open a CONNECT-UDP stream\n{}",
            server.log_text()
        );
    };
    assert_eq!(udp.to_string(), REFUSED, "{}", server.log_text());
}
