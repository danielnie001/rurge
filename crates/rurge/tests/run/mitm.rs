//! `http-api-tls` from the command line (the phase 4 / M2c plan's P12 and
//! P14): the API speaks HTTPS with a leaf certificate its MITM CA signs, and
//! `rurge status` / `rurge stop` with `-c` reach it over HTTPS. The CA comes
//! from `rurge mitm ca generate`, written into this test's own temporary
//! directory; nothing here installs it into any trust store.

use super::*;
use std::path::Path;

/// The `[MITM]` section of a CA generated into `<dir>/ca`, which stays there:
/// it is a throwaway CA for this test alone.
fn generated_conf(dir: &Path) -> String {
    let out = dir.join("ca");
    std::fs::create_dir_all(&out).unwrap();
    rurge()
        .args(["mitm", "ca", "generate", "--out"])
        .arg(&out)
        .assert()
        .success();
    std::fs::read_to_string(out.join("rurge-mitm.conf")).unwrap()
}

/// `http-api-tls` end to end: the daemon says `api on https://`, the profile
/// names the port, and the two commands that read the profile reach the API
/// over HTTPS. `--remote` reads no profile, so it has no CA and no HTTPS.
#[test]
fn status_and_stop_reach_an_api_served_over_https() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    // a fixed port: `rurge status -c` reads the API's address from the
    // profile, and a port of 0 there names nothing it could reach
    let api = free_tcp_port();
    let conf = write_conf(
        dir.path(),
        &format!(
            "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nhttp-api = k@127.0.0.1:{api}\nloglevel = warning\nhttp-api-tls = true"
        ),
    );
    let mut text = std::fs::read_to_string(&conf).unwrap();
    text.push_str(&generated_conf(dir.path()));
    std::fs::write(&conf, text).unwrap();

    let mut daemon = spawn_command(rurge_run(&conf, &data));
    assert_eq!(
        wait_for_line(&daemon, "api on https://"),
        format!("127.0.0.1:{api}")
    );

    let out = rurge()
        .args(["status", "-c"])
        .arg(&conf)
        .arg("--data-dir")
        .arg(&data)
        .assert()
        .success();
    let status = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert_eq!(
        status.lines().next(),
        Some(format!("rurge at https://127.0.0.1:{api}").as_str()),
        "{status}"
    );

    // `--remote` names no profile and gets no CA, so it keeps to plain HTTP
    rurge()
        .args([
            "status",
            "--remote",
            &format!("127.0.0.1:{api}"),
            "--key",
            "k",
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "cannot reach rurge at http://127.0.0.1:",
        ));

    rurge()
        .args(["stop", "-c"])
        .arg(&conf)
        .arg("--data-dir")
        .arg(&data)
        .assert()
        .success();
    assert_eq!(wait_for_exit(&mut daemon, 10), Some(0), "stop exits 0");
}

/// `http-api-tls` with no usable MITM CA: the API is not started at all — it
/// never falls back to plain HTTP — and the daemon says so in one line and
/// goes on running (the phase 4 / M2c plan's P12).
#[test]
fn without_a_ca_the_api_is_not_started_and_rurge_runs() {
    const WARN: &str = "http-api-tls is on but there is no usable MITM CA: the API is not started (it never falls back to plain HTTP)";
    let dir = tempfile::tempdir().unwrap();
    let conf = write_conf(dir.path(), &format!("{API_GENERAL}\nhttp-api-tls = true"));
    let said = output_until_it_is_running(rurge_run(&conf, &dir.path().join("data")));
    assert!(
        !said.iter().any(|line| line.starts_with("api on")),
        "the API was started: {said:#?}"
    );
    assert!(
        said.iter().any(|line| line.contains(WARN)),
        "no line said the API is not started: {said:#?}"
    );
    let last = said.last().expect("the summary line");
    assert!(
        last.starts_with("rurge ") && last.contains(" running:"),
        "{last}"
    );
}
