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

/// One small and one large TCP round trip through each policy of `cases`,
/// whatever the ones before it met.
async fn tcp(failures: &mut Failures, profile: &str, cases: &[Case]) {
    let echo = echo_server().await;
    for case in cases {
        let out = outbound(profile, case.name, None);
        let steps = async move {
            roundtrip(&out, echo).await;
            roundtrip_big(&out, echo).await;
        };
        attempt(failures, case.name, steps).await;
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
    let mut failures = Failures::default();
    tcp(&mut failures, &profile, &cases).await;
    failures.finish(|| server.log_text());
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
    let mut failures = Failures::default();
    attempt(&mut failures, "V2", steps).await;
    failures.finish(|| server.log_text());
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
    let mut failures = Failures::default();
    attempt(&mut failures, "V3", steps).await;
    failures.finish(|| server.log_text());
}

/// mihomo closes a connection whose first record it cannot open, without a
/// word of its own. It has read only the head of what the client sent by
/// then, so some systems report its close as an end and others as a reset:
/// either way the session fails at once, with one of the two sentences
/// `none_is_answered` takes.
#[tokio::test]
async fn a_snell_listener_of_another_psk_or_version_against_mihomo() {
    let Some(bin) = mihomo_or_skip("a_snell_listener_of_another_psk_or_version_against_mihomo")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let cases = unanswered_cases();
    let (server, profile) = start(&bin, dir.path(), &cases);
    let names: Vec<&str> = cases.iter().map(|case| case.name).collect();
    let mut failures = Failures::default();
    none_is_answered(&mut failures, &profile, &names, UNANSWERED_BOUND).await;
    failures.finish(|| server.log_text());
}

/// What `none_is_answered` takes and what it does not, where that can be
/// tried without a reference server: against loopback servers of four
/// kinds. One of another PSK that closes without a word of its own and one
/// that resets give the two sentences, as rurge words them, and pass. One
/// that answers with what no key opens, and one that keeps the connection
/// and says nothing — as a server may, to tell a prober nothing — are
/// failures, each told as what it was.
#[tokio::test]
async fn only_the_two_sentences_of_an_unanswered_request_are_taken() {
    let other_psk = |reset| {
        FakeSnell::spawn(SnellScript {
            reset,
            ..SnellScript::new(OTHER_PSK)
        })
    };
    let (closes, resets) = (other_psk(false).await, other_psk(true).await);
    let garbles = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ports = [
        closes.addr().port(),
        resets.addr().port(),
        garbles.local_addr().unwrap().port(),
        silent.local_addr().unwrap().port(),
    ];
    // a salt and a record's worth of bytes, then the client's to end
    tokio::spawn(async move {
        while let Ok((mut client, _)) = garbles.accept().await {
            tokio::spawn(async move {
                let _ = client.write_all(&[0x5a; 64]).await;
                let _ = client.read_to_end(&mut Vec::new()).await;
            });
        }
    });
    // accepts, and neither reads nor closes
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((client, _)) = silent.accept().await {
            held.push(client);
        }
    });
    let names = ["Closes", "Resets", "Garbles", "Silent"];
    let lines: String = names
        .iter()
        .zip(ports)
        .map(|(name, port)| format!("{name} = snell, 127.0.0.1, {port}, psk={PSK}, version=4\n"))
        .collect();
    let profile = format!("[Proxy]\n{lines}[Rule]\nFINAL,DIRECT\n");
    let echo = echo_server().await;
    for (name, sentence) in [("Closes", UNANSWERED[0]), ("Resets", UNANSWERED[1])] {
        let out = outbound(&profile, name, None);
        let said = unanswered(&out, echo, UNANSWERED_BOUND).await;
        assert_eq!(said.as_deref(), Ok(sentence), "{name}");
    }
    let mut failures = Failures::default();
    none_is_answered(&mut failures, &profile, &names[..3], UNANSWERED_BOUND).await;
    // the wait for a server that never ends the request, cut short
    let short = Duration::from_millis(300);
    none_is_answered(&mut failures, &profile, &names[3..], short).await;
    assert_eq!(
        failures.report("the server's log"),
        "2 of 4 failed\n\
         Garbles: failed with another sentence: snell: the server's data failed to decrypt (wrong psk or version?)\n\
         Silent: no failure within 300ms, 0 bytes in: the request hangs\n\
         --- the server's log ---\n\
         the server's log"
    );
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
    let mut failures = Failures::default();
    tcp(&mut failures, &profile, &cases).await;
    let (out, echo) = (
        outbound(&profile, "Chacha20", None),
        udp_echo_server().await,
    );
    let steps = async move { udp_roundtrip(&out, echo).await };
    attempt(&mut failures, "Chacha20 over UDP", steps).await;
    failures.finish(|| server.log_text());
}

#[tokio::test]
async fn ss_behind_simple_obfs_against_mihomo() {
    let Some(bin) = mihomo_or_skip("ss_behind_simple_obfs_against_mihomo") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let cases = ss_obfs_cases();
    let (server, profile) = start(&bin, dir.path(), &cases);
    let mut failures = Failures::default();
    tcp(&mut failures, &profile, &cases).await;
    failures.finish(|| server.log_text());
}

#[tokio::test]
async fn snell_behind_simple_obfs_against_mihomo() {
    let Some(bin) = mihomo_or_skip("snell_behind_simple_obfs_against_mihomo") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let cases = snell_obfs_cases();
    let (server, profile) = start(&bin, dir.path(), &cases);
    let mut failures = Failures::default();
    tcp(&mut failures, &profile, &cases).await;
    failures.finish(|| server.log_text());
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
