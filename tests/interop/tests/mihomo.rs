//! rurge's `snell` and `ss` outbounds against mihomo v1.19.32's listeners
//! (phase 2 M8 design 4.1, 4.3): Snell versions 1 to 3 over TCP with one
//! chunk and with many, version 2's requests one after another on one
//! connection, version 3's UDP, and a listener of another PSK or version;
//! the `ss` stream cipher `chacha20`, which no other reference has; and
//! simple-obfs `http` and `tls` in front of both protocols, which no other
//! reference speaks. Every target is a loopback IP literal.
//!
//! A case is a policy and the listener it dials, in one table per test:
//! no test against mihomo runs without it, and the last test loads and
//! builds the policies of every table where there is none.

mod common;

use common::*;
use rurge_interop::mihomo::{Listener, Mihomo, OBFS_HOST, Obfs, mihomo_or_skip};
use rurge_proto::testing::{FakeSnell, SnellScript};
use std::time::Duration;

const PSK: &str = "sn3ll-interop";
const OTHER_PSK: &str = "an0ther-psk";
const PASSWORD: &str = "s3same";

/// What a session says when the server ended the connection without a byte
/// of its own: where that arrives as an end, and where it arrives as a reset.
const UNANSWERED: [&str; 2] = [
    "snell: the server closed the connection without answering (wrong psk or version?)",
    "snell: the server reset the connection without answering (wrong psk or version?)",
];

/// A policy and the listener it dials.
struct Case {
    name: &'static str,
    listener: Listener,
    /// The policy's line after its name, kind, server and port.
    params: String,
}

/// A `snell` listener of `version` with `PSK`, behind `obfs`, and the line
/// that dials it.
fn snell(name: &'static str, version: u8, obfs: Option<Obfs>, params: String) -> Case {
    Case {
        name,
        listener: Listener::Snell {
            version,
            psk: PSK.into(),
            obfs,
        },
        params,
    }
}

/// A `shadowsocks` listener of `cipher`, behind `obfs`, and a line of that
/// method and what `rest` adds.
fn ss(name: &'static str, cipher: &'static str, obfs: Option<Obfs>, rest: &str) -> Case {
    Case {
        name,
        listener: Listener::Shadowsocks {
            cipher,
            password: PASSWORD.into(),
            obfs,
        },
        params: format!("encrypt-method={cipher}, password={PASSWORD}{rest}"),
    }
}

/// Versions 1 to 3, each to a listener of its own.
fn tcp_cases() -> Vec<Case> {
    vec![
        // a line that names no version dials version 1
        snell("Bare", 1, None, format!("psk={PSK}")),
        snell("V1", 1, None, format!("psk={PSK}, version=1")),
        snell("V2", 2, None, format!("psk={PSK}, version=2")),
        snell("V3", 3, None, format!("psk={PSK}, version=3")),
    ]
}

fn reuse_cases() -> Vec<Case> {
    vec![snell("V2", 2, None, format!("psk={PSK}, version=2"))]
}

fn udp_cases() -> Vec<Case> {
    vec![snell("V3", 3, None, format!("psk={PSK}, version=3"))]
}

/// Lines no listener can read: another PSK on the oldest and on the newest
/// record format, and the listener's own PSK in the other format's records.
fn unanswered_cases() -> Vec<Case> {
    vec![
        snell("OtherPskV1", 1, None, format!("psk={OTHER_PSK}, version=1")),
        snell("OtherPskV4", 4, None, format!("psk={OTHER_PSK}, version=4")),
        snell("V1ToV4", 4, None, format!("psk={PSK}, version=1")),
    ]
}

fn chacha20_cases() -> Vec<Case> {
    vec![ss("Chacha20", "chacha20", None, ", udp-relay=true")]
}

/// simple-obfs in front of both of mihomo's Shadowsocks implementations:
/// the one that serves the AEAD ciphers, behind either mode, and the one
/// that serves the stream ciphers.
fn ss_obfs_cases() -> Vec<Case> {
    let (http, tls) = (
        format!(", obfs=http, obfs-host={OBFS_HOST}"),
        format!(", obfs=tls, obfs-host={OBFS_HOST}"),
    );
    vec![
        ss("Http", "aes-128-gcm", Some(Obfs::Http), &http),
        ss("Tls", "aes-128-gcm", Some(Obfs::Tls), &tls),
        ss("StreamTls", "aes-256-cfb", Some(Obfs::Tls), &tls),
    ]
}

/// `tls` exists on Snell's chunked versions only, `http` on every one.
fn snell_obfs_cases() -> Vec<Case> {
    vec![
        snell(
            "TlsV1",
            1,
            Some(Obfs::Tls),
            format!("psk={PSK}, version=1, obfs=tls, obfs-host={OBFS_HOST}"),
        ),
        snell(
            "HttpV4",
            4,
            Some(Obfs::Http),
            format!("psk={PSK}, version=4, obfs=http, obfs-host={OBFS_HOST}"),
        ),
    ]
}

/// The profile of `cases`, the `index`-th policy dialling `port(index)`.
fn profile(cases: &[Case], port: impl Fn(usize) -> u16) -> String {
    let lines: String = cases
        .iter()
        .enumerate()
        .map(|(index, case)| {
            let kind = match case.listener {
                Listener::Snell { .. } => "snell",
                Listener::Shadowsocks { .. } => "ss",
            };
            format!(
                "{} = {kind}, 127.0.0.1, {}, {}\n",
                case.name,
                port(index),
                case.params
            )
        })
        .collect();
    format!("[Proxy]\n{lines}[Rule]\nFINAL,DIRECT\n")
}

/// mihomo with the listeners of `cases`, and the profile whose policies
/// dial them.
fn start(bin: &Path, dir: &Path, cases: &[Case]) -> (Mihomo, String) {
    let listeners = cases.iter().map(|case| case.listener.clone()).collect();
    let server = Mihomo::spawn(bin, dir, listeners);
    let profile = profile(cases, |index| server.port(index));
    (server, profile)
}

/// One small and one large TCP round trip through each policy of `cases`.
async fn tcp(server: &Mihomo, profile: &str, cases: &[Case]) {
    let echo = echo_server().await;
    for case in cases {
        let out = outbound(profile, case.name, None);
        let steps = async move {
            roundtrip(&out, echo).await;
            roundtrip_big(&out, echo).await;
        };
        attempt(case.name, || server.log_text(), steps).await;
    }
}

/// What reading says after a request the server cannot read. The tunnel
/// comes up, since nothing is asked before the first payload; the payload
/// leaves; and then the failure must come within the bound. Whatever goes
/// otherwise is told with what the server logged (`log`).
async fn unanswered(log: &dyn Fn() -> String, out: &OutboundRef, echo: SocketAddr) -> String {
    let bound = Duration::from_secs(10);
    let (to, opts) = (target(echo), ConnectOpts::default());
    let mut stream = match tokio::time::timeout(bound, out.connect_tcp(&to, &opts)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => panic!("the tunnel comes up: {e}\n{}", log()),
        Err(_) => panic!("no tunnel within the bound\n{}", log()),
    };
    match tokio::time::timeout(bound, stream.write_all(b"interop")).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => panic!("the first payload leaves: {e}\n{}", log()),
        Err(_) => panic!("no first payload within the bound\n{}", log()),
    }
    let mut answer = Vec::new();
    match tokio::time::timeout(bound, stream.read_to_end(&mut answer)).await {
        Ok(Err(e)) => e.to_string(),
        Ok(Ok(n)) => panic!(
            "the request ended without an error, {n} bytes in\n{}",
            log()
        ),
        Err(_) => panic!("no failure within the bound: the request hangs\n{}", log()),
    }
}

/// Version 1 seals its chunks with ChaCha20-Poly1305, versions 2 and 3 with
/// AES-128-GCM; each carries the request with the first payload in one
/// chunk, then 100 000 bytes each way at once.
#[tokio::test]
async fn snell_v1_to_v3_against_mihomo() {
    let Some(bin) = mihomo_or_skip("snell_v1_to_v3_against_mihomo") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let cases = tcp_cases();
    let (server, profile) = start(&bin, dir.path(), &cases);
    tcp(&server, &profile, &cases).await;
}

/// Version 2 always reuses its connection: each direction of a request
/// ends on a zero-length chunk, and the next request follows on the same
/// connection. Had mihomo closed it in between, rurge would have sent the
/// next request again on a fresh one without a word, so it is mihomo's log
/// that tells: the three requests it relayed came from one client.
#[tokio::test]
async fn snell_v2_requests_share_a_connection_against_mihomo() {
    let Some(bin) = mihomo_or_skip("snell_v2_requests_share_a_connection_against_mihomo") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let cases = reuse_cases();
    // before mihomo is up: its log holds a line of the readiness probe too,
    // whose target must not come by the echo's port
    let echo = echo_server().await;
    let (server, profile) = start(&bin, dir.path(), &cases);
    let out = outbound(&profile, "V2", None);
    let steps = async move { requests_one_after_another(&out, echo, 3).await };
    attempt("V2", || server.log_text(), steps).await;
    let clients = server.clients_of(echo);
    assert!(
        clients.len() == 3 && clients.iter().all(|client| *client == clients[0]),
        "three requests on one connection, but mihomo relayed for {clients:?}\n{}",
        server.log_text()
    );
}

/// UDP came with version 3: a connection of its own asks for it, the
/// listener answers at once, and every chunk is a datagram.
#[tokio::test]
async fn snell_v3_udp_against_mihomo() {
    let Some(bin) = mihomo_or_skip("snell_v3_udp_against_mihomo") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let cases = udp_cases();
    let (server, profile) = start(&bin, dir.path(), &cases);
    let (out, echo) = (outbound(&profile, "V3", None), udp_echo_server().await);
    let steps = async move { udp_roundtrip(&out, echo).await };
    attempt("V3", || server.log_text(), steps).await;
}

/// mihomo closes a connection whose first record it cannot open, without a
/// word of its own. It has read only the head of what the client sent by
/// then, so some systems report its close as an end and others as a reset:
/// either way the session fails at once, with one of two sentences that say
/// what may be wrong and nothing of either PSK. A third sentence has the
/// same hint — the server's data failed to decrypt — and would mean that
/// mihomo had answered: it is not taken.
#[tokio::test]
async fn a_snell_listener_of_another_psk_or_version_against_mihomo() {
    let Some(bin) = mihomo_or_skip("a_snell_listener_of_another_psk_or_version_against_mihomo")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let cases = unanswered_cases();
    let (server, profile) = start(&bin, dir.path(), &cases);
    let echo = echo_server().await;
    for case in &cases {
        let out = outbound(&profile, case.name, None);
        let text = unanswered(&|| server.log_text(), &out, echo).await;
        assert!(
            UNANSWERED.contains(&text.as_str()),
            "{}: {text}\n{}",
            case.name,
            server.log_text()
        );
    }
}

/// The two sentences as rurge words them, where they can be read without
/// mihomo: from a loopback server of another PSK that closes without a word
/// of its own, and from one that resets.
#[tokio::test]
async fn the_two_sentences_of_an_unanswered_request_are_rurges_own() {
    let echo = echo_server().await;
    for (reset, sentence) in [(false, UNANSWERED[0]), (true, UNANSWERED[1])] {
        let server = FakeSnell::spawn(SnellScript {
            reset,
            ..SnellScript::new(PSK)
        })
        .await;
        let profile = format!(
            "[Proxy]\nOther = snell, 127.0.0.1, {}, psk={OTHER_PSK}, version=4\n[Rule]\nFINAL,DIRECT\n",
            server.addr().port()
        );
        let out = outbound(&profile, "Other", None);
        assert_eq!(unanswered(&String::new, &out, echo).await, sentence);
    }
}

/// The original ChaCha20, of an 8-byte nonce: over TCP, and a datagram at a
/// time over UDP.
#[tokio::test]
async fn ss_chacha20_against_mihomo() {
    let Some(bin) = mihomo_or_skip("ss_chacha20_against_mihomo") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let cases = chacha20_cases();
    let (server, profile) = start(&bin, dir.path(), &cases);
    tcp(&server, &profile, &cases).await;
    let (out, echo) = (
        outbound(&profile, "Chacha20", None),
        udp_echo_server().await,
    );
    let steps = async move { udp_roundtrip(&out, echo).await };
    attempt("Chacha20 over UDP", || server.log_text(), steps).await;
}

#[tokio::test]
async fn ss_behind_simple_obfs_against_mihomo() {
    let Some(bin) = mihomo_or_skip("ss_behind_simple_obfs_against_mihomo") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let cases = ss_obfs_cases();
    let (server, profile) = start(&bin, dir.path(), &cases);
    tcp(&server, &profile, &cases).await;
}

#[tokio::test]
async fn snell_behind_simple_obfs_against_mihomo() {
    let Some(bin) = mihomo_or_skip("snell_behind_simple_obfs_against_mihomo") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let cases = snell_obfs_cases();
    let (server, profile) = start(&bin, dir.path(), &cases);
    tcp(&server, &profile, &cases).await;
}

/// What the cases write on rurge's side, checked without mihomo: the
/// policies of every table.
#[tokio::test]
async fn the_policies_of_every_case_load_and_build() {
    for cases in [
        tcp_cases(),
        reuse_cases(),
        udp_cases(),
        unanswered_cases(),
        chacha20_cases(),
        ss_obfs_cases(),
        snell_obfs_cases(),
    ] {
        let names: Vec<&str> = cases.iter().map(|case| case.name).collect();
        loads_and_builds(&profile(&cases, |index| 4000 + index as u16), &names);
    }
}
