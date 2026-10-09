//! rurge's `ss` outbound against shadowsocks-rust's `ssserver` and sing-box's
//! `shadowsocks` inbound (phase 2 M6 design 3.4, M6-D5): TCP with one chunk
//! and with many, and UDP, for the AEAD and the 2022 methods, the latter
//! also as one of several users; and against `ssserver` the stream ciphers
//! it knows (phase 2 M8 design 4.1). Every target is a loopback IP literal.

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

/// The stream ciphers ssserver's release build knows by the manual's names
/// — it has neither `salsa20` nor the `chacha20` of an 8-byte nonce —, and
/// whether the case goes on to UDP: once with an IV of 16 bytes and once
/// with one of 12.
const STREAM_METHODS: [(&str, bool); 9] = [
    ("rc4", false),
    ("rc4-md5", false),
    ("aes-128-cfb", false),
    ("aes-192-cfb", false),
    ("aes-256-cfb", true),
    ("aes-128-ctr", false),
    ("aes-192-ctr", false),
    ("aes-256-ctr", false),
    ("chacha20-ietf", true),
];

/// A policy for each stream method, named after it, the `index`-th dialling
/// `port(index)`.
fn stream_profile(port: impl Fn(usize) -> u16) -> String {
    let lines: String = STREAM_METHODS
        .iter()
        .enumerate()
        .map(|(index, (method, _))| line(method, port(index), method, "s3same"))
        .collect();
    format!("[Proxy]\n{lines}[Rule]\nFINAL,DIRECT\n")
}

/// Nothing a stream cipher carries is authenticated, so the bytes that come
/// back are the proof that key, IV and keystream are ssserver's — over more
/// than one write each way too. ssserver takes these ciphers as it takes any
/// other, with a warning in its log. Every method is tried, whatever the
/// ones before it met.
#[tokio::test]
async fn stream_methods_against_ssserver() {
    let Some(bin) = ssserver_or_skip("stream_methods_against_ssserver") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let ss = Ssserver::spawn(
        &bin,
        dir.path(),
        STREAM_METHODS
            .iter()
            .map(|(method, _)| server(method, "s3same"))
            .collect(),
    );
    let profile = stream_profile(|index| ss.port(index));
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    let mut failures = Failures::default();
    for (method, udp) in STREAM_METHODS {
        let out = outbound(&profile, method, None);
        let steps = async move {
            roundtrip(&out, echo).await;
            roundtrip_big(&out, echo).await;
            if udp {
                udp_roundtrip(&out, udp_echo).await;
            }
        };
        attempt(&mut failures, method, steps).await;
    }
    failures.finish(|| ss.log_text());
}

/// What that case writes on rurge's side, checked without ssserver.
#[tokio::test]
async fn the_stream_method_policies_load_and_build() {
    let names: Vec<&str> = STREAM_METHODS.iter().map(|(method, _)| *method).collect();
    loads_and_builds(&stream_profile(|index| 4000 + index as u16), &names);
}

/// How that case tells what failed, checked without ssserver: a method that
/// fails does not keep the next from being tried, each failure is told
/// under its method with what its own panic said, on one line, and the
/// server's log comes after them all, once.
#[tokio::test]
async fn every_method_that_fails_is_told_not_the_first_alone() {
    let mut failures = Failures::default();
    attempt(&mut failures, "rc4", async {
        panic!("the echo differs");
    })
    .await;
    attempt(&mut failures, "rc4-md5", async {}).await;
    attempt(&mut failures, "aes-128-cfb", async {
        let sent = 7;
        panic!("the echo comes back within the bound:\n  {sent} bytes sent,\n  none back");
    })
    .await;
    // `finish` is what fails the test, with that report
    let told = std::panic::catch_unwind(|| {
        failures.finish(|| "a line of the log\nand another".to_string())
    })
    .expect_err("finish fails the test when something failed");
    assert_eq!(
        panic_text(told),
        "2 of 3 failed\n\
         rc4: the echo differs\n\
         aes-128-cfb: the echo comes back within the bound: 7 bytes sent, none back\n\
         --- the server's log ---\n\
         a line of the log\n\
         and another"
    );
    // nothing failed: nothing is said, and the log is not asked for
    let mut passed = Failures::default();
    attempt(&mut passed, "rc4", async {}).await;
    passed.finish(|| unreachable!("the log of a test that passed"));
}

/// And the test fails with that report, once, at its end.
#[tokio::test]
#[should_panic(
    expected = "1 of 2 failed\nrc4: the echo differs\n--- the server's log ---\nthe log"
)]
async fn a_method_that_fails_fails_its_test_at_the_end() {
    let mut failures = Failures::default();
    attempt(&mut failures, "rc4", async {
        panic!("the echo differs");
    })
    .await;
    attempt(&mut failures, "rc4-md5", async {}).await;
    failures.finish(|| "the log".to_string());
}

/// A test that leaves `finish` out does not pass by it: failures dropped
/// without it fail the test there and then, each one named.
#[test]
#[should_panic(expected = "1 of 1 failed\nrc4: the echo differs")]
fn failures_that_are_never_finished_fail_the_test() {
    let mut failures = Failures::default();
    failures.record("rc4", Err("the echo differs".to_string()));
}

/// With nothing recorded, or nothing that failed, there is nothing to finish.
#[test]
fn nothing_that_failed_need_not_be_finished() {
    drop(Failures::default());
    let mut passed = Failures::default();
    passed.record("rc4", Ok(()));
}

/// A test that fails for a reason of its own says that, not that it left
/// failures unfinished: a panic out of a drop in the middle of an unwind
/// aborts the process. Were the guard to do that this binary would go with
/// it, so the case runs in a child process — this test again, told so by
/// its environment — and is judged by how that ended.
#[tokio::test]
async fn failures_dropped_by_a_test_that_fails_otherwise_say_nothing() {
    const CHILD: &str = "RURGE_INTEROP_UNWINDING_CHILD";
    const SAID: &str = "unwound without a second panic";
    const NAME: &str = "failures_dropped_by_a_test_that_fails_otherwise_say_nothing";
    if std::env::var_os(CHILD).is_some() {
        let told = std::panic::catch_unwind(|| {
            let mut failures = Failures::default();
            failures.record("rc4", Err("the echo differs".to_string()));
            panic!("the test fails for a reason of its own");
        })
        .expect_err("the closure panics");
        assert_eq!(panic_text(told), "the test fails for a reason of its own");
        println!("{SAID}");
        return;
    }
    let mut command =
        tokio::process::Command::new(std::env::current_exe().expect("the path of this binary"));
    command
        .args(["--exact", NAME, "--nocapture"])
        .env(CHILD, "1")
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(60), command.output())
        .await
        .expect("the child process ends within the bound")
        .expect("the child process runs");
    let (said, stderr) = (
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        output.status.success() && said.contains(SAID),
        "the child process ended with {}\n{said}\n{stderr}",
        output.status
    );
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
