use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn version_prints_crate_version() {
    Command::cargo_bin("rurge")
        .unwrap()
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("rurge 0.1.0"));
}

use std::fs;

const VALID: &str = "[General]\nloglevel = notify\n[Rule]\nGEOIP,CN,DIRECT\nFINAL,DIRECT\n";
const WARNING: &str = "[General]\nmystery = 1\n[Rule]\nFINAL,DIRECT\n";
const ERROR: &str = "[Rule]\nDOMAIN,a,DIRECT\n";

fn write(dir: &tempfile::TempDir, name: &str, text: &str) -> std::path::PathBuf {
    let p = dir.path().join(name);
    fs::write(&p, text).unwrap();
    p
}

#[test]
fn check_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let valid = write(&dir, "valid.conf", VALID);
    let warning = write(&dir, "warning.conf", WARNING);
    let error = write(&dir, "error.conf", ERROR);

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(&valid)
        .assert()
        .success()
        .stdout(predicate::str::contains("0 error(s)"));
    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(&warning)
        .assert()
        .success()
        .stdout(predicate::str::contains("W0001"));
    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "--strict", "-c"])
        .arg(&warning)
        .assert()
        .code(1);
    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(&error)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0010"));
    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c", "/no/such/file.conf"])
        .assert()
        .code(2)
        .stderr(predicate::function(|s: &str| {
            s.matches("cannot read").count() == 1
        }));
}

#[test]
fn bom_prefixed_config_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("bom.conf");
    fs::write(
        &p,
        b"\xEF\xBB\xBF[General]\r\nloglevel = notify\r\n[Rule]\r\nFINAL,DIRECT\r\n",
    )
    .unwrap();
    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(&p)
        .assert()
        .success()
        .stdout(predicate::str::contains("0 error(s), 0 warning(s)"));
}

#[test]
fn check_json_and_platform() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write(
        &dir,
        "p.conf",
        "[Rule]\nDOMAIN,a,REJECT #!MACOS-ONLY\nFINAL,DIRECT\n",
    );
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "--json", "--platform", "linux", "-c"])
        .arg(&conf)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["errors"], 0);
    assert!(
        v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "I0002")
    );
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "--json", "--platform", "macos", "-c"])
        .arg(&conf)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(
        !v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "I0002")
    );
}

mod rule_match {
    use assert_cmd::Command;
    use predicates::prelude::*;
    use std::path::Path;

    const CONF: &str = "\
[Proxy]
P = direct
[Rule]
RULE-SET,sets/a.list,P
IP-CIDR,10.0.0.0/8,P
DOMAIN-SUFFIX,ext.com,P,extended-matching
FINAL,DIRECT,dns-failed
";

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sets")).unwrap();
        std::fs::write(
            dir.path().join("sets").join("a.list"),
            "DOMAIN,listed.com\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("t.conf"), CONF).unwrap();
        dir
    }

    fn rule_match(dir: &Path, extra: &[&str]) -> Command {
        let mut cmd = Command::cargo_bin("rurge").unwrap();
        cmd.arg("rule")
            .arg("match")
            .arg("-c")
            .arg(dir.join("t.conf"))
            .arg("--no-network")
            .arg("--data-dir")
            .arg(dir.join("data"))
            .args(extra);
        cmd
    }

    fn json(dir: &Path, extra: &[&str]) -> (serde_json::Value, i32) {
        let out = rule_match(dir, extra).arg("--json").output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("bad json: {e}\n{}", String::from_utf8_lossy(&out.stdout)));
        (v, out.status.code().unwrap())
    }

    #[test]
    fn local_rule_set_matches_without_network() {
        let dir = workspace();
        let (v, code) = json(dir.path(), &["listed.com"]);
        assert_eq!(code, 0);
        assert_eq!(v["policy"], "P");
        assert_eq!(v["reason"], "rule");
        assert_eq!(v["matched"]["index"], 0);
        assert_eq!(v["sub_rule"]["entry"], "DOMAIN,listed.com");
    }

    #[test]
    fn no_dns_falls_back_to_final_with_dns_failed() {
        let dir = workspace();
        let (v, code) = json(dir.path(), &["other.org", "--no-dns"]);
        assert_eq!(code, 0);
        assert_eq!(v["reason"], "dns-failed-fallback");
        assert_eq!(v["policy"], "DIRECT");
    }

    #[test]
    fn no_dns_without_dns_failed_exits_one() {
        let dir = workspace();
        std::fs::write(
            dir.path().join("t.conf"),
            CONF.replace("FINAL,DIRECT,dns-failed", "FINAL,DIRECT"),
        )
        .unwrap();
        let (v, code) = json(dir.path(), &["other.org", "--no-dns"]);
        assert_eq!(code, 1);
        assert!(v["policy"].is_null());
        assert_eq!(v["reason"], "dns-failed");
    }

    #[test]
    fn resolve_override_hits_ip_rules() {
        let dir = workspace();
        let (v, code) = json(dir.path(), &["other.org", "--resolve", "10.1.2.3"]);
        assert_eq!(code, 0);
        assert_eq!(v["matched"]["index"], 1);
        assert_eq!(v["resolved"]["v4"][0], "10.1.2.3");
    }

    #[test]
    fn explain_prints_a_trace_and_extended_matching_uses_sni() {
        let dir = workspace();
        rule_match(
            dir.path(),
            &["1.2.3.4", "--sni", "API.ext.com", "--explain"],
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("policy: P"))
        .stdout(predicate::str::contains("rule #2:"))
        .stdout(predicate::str::contains("trace:"))
        .stdout(predicate::str::contains("#0 no-match"));
    }

    #[test]
    fn outbound_mode_bypasses_rules() {
        let dir = workspace();
        let (v, _) = json(dir.path(), &["listed.com", "--mode", "direct"]);
        assert_eq!(v["reason"], "outbound-mode-direct");
        let (v, _) = json(dir.path(), &["listed.com", "--mode", "proxy=P"]);
        assert_eq!(v["reason"], "outbound-mode-proxy");
        assert_eq!(v["policy"], "P");
    }

    #[test]
    fn missing_profile_exits_two() {
        let dir = workspace();
        rule_match(dir.path(), &["a.com"])
            .arg("-c")
            .arg(dir.path().join("nope.conf"))
            .assert()
            .code(2);
    }

    /// F8 regression: config-load warnings (no errors, so `load` did not
    /// exit early) used to be dropped silently — only `stack.diagnostics`
    /// was ever printed or included in `--json`.
    #[test]
    fn config_load_warnings_are_surfaced_in_text_and_json() {
        let dir = workspace();
        std::fs::write(
            dir.path().join("t.conf"),
            format!("[General]\nsome-made-up-key = 1\n{CONF}"),
        )
        .unwrap();
        rule_match(dir.path(), &["listed.com"])
            .assert()
            .success()
            .stderr(predicate::str::contains("W0001"));
        let (v, code) = json(dir.path(), &["listed.com"]);
        assert_eq!(code, 0);
        let warnings = v["warnings"].as_array().expect("warnings array");
        assert!(
            warnings.iter().any(|w| w["code"] == "W0001"),
            "{warnings:?}"
        );
    }
}

mod dns {
    use assert_cmd::Command;
    use rurge_dns::message::Qtype;
    use rurge_dns::testing::MockDns;
    use std::path::Path;

    const CONF: &str = "[General]\nipv6 = false\n[Proxy]\n[Host]\nfixed.test = 1.2.3.4\ndual.test = 1.2.3.4, fd00::9\n[Rule]\nFINAL,DIRECT\n";

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("t.conf"), CONF).unwrap();
        dir
    }

    fn dns_cmd(dir: &Path, sub: &str, server: &str, extra: &[&str]) -> Command {
        let mut cmd = Command::cargo_bin("rurge").unwrap();
        cmd.arg("dns")
            .arg(sub)
            .arg("-c")
            .arg(dir.join("t.conf"))
            .arg("--server")
            .arg(server)
            .arg("--no-network")
            .arg("--data-dir")
            .arg(dir.join("data"))
            .args(extra);
        cmd
    }

    /// Runs the binary off the runtime thread so the mock upstream keeps serving.
    async fn output(mut cmd: Command) -> std::process::Output {
        tokio::task::spawn_blocking(move || cmd.output().unwrap())
            .await
            .unwrap()
    }

    fn json_of(out: &std::process::Output) -> serde_json::Value {
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("bad json: {e}\n{}", String::from_utf8_lossy(&out.stdout)))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lookup_over_udp_prints_addresses_source_and_ttl() {
        let mock = MockDns::spawn().await;
        mock.set("a.test", &["10.0.0.1", "10.0.0.2"], &[], 120);
        let dir = workspace();
        let server = mock.addr().to_string();
        let out = output(dns_cmd(
            dir.path(),
            "lookup",
            &server,
            &["a.test", "--json"],
        ))
        .await;
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v = json_of(&out);
        assert_eq!(v["addresses"], serde_json::json!(["10.0.0.1", "10.0.0.2"]));
        assert_eq!(v["source"], format!("upstream(udp://{server})"));
        assert_eq!(v["ttl_secs"], 120);
        assert!(v["error"].is_null());
        assert_eq!(v["upstreams"][0], format!("udp://{server}"));
        let text = output(dns_cmd(dir.path(), "lookup", &server, &["a.test"])).await;
        let stdout = String::from_utf8_lossy(&text.stdout);
        assert!(stdout.contains("addresses: 10.0.0.1, 10.0.0.2"), "{stdout}");
        assert!(stdout.contains("ttl: 120s"), "{stdout}");
        assert_eq!(
            mock.query_count("a.test", Qtype::Aaaa),
            0,
            "ipv6 = false asks A only"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_answer_exits_one() {
        let mock = MockDns::spawn().await;
        mock.set_empty("nx.test");
        let dir = workspace();
        let server = mock.addr().to_string();
        let out = output(dns_cmd(
            dir.path(),
            "lookup",
            &server,
            &["nx.test", "--json"],
        ))
        .await;
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(json_of(&out)["error"], "empty answer");
        let text = output(dns_cmd(dir.path(), "lookup", &server, &["nx.test"])).await;
        assert_eq!(text.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&text.stdout).contains("error: empty answer"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_entries_short_circuit_the_upstream() {
        let mock = MockDns::spawn().await;
        let dir = workspace();
        let out = output(dns_cmd(
            dir.path(),
            "lookup",
            &mock.addr().to_string(),
            &["fixed.test", "--json"],
        ))
        .await;
        let v = json_of(&out);
        assert_eq!(v["addresses"], serde_json::json!(["1.2.3.4"]));
        assert_eq!(v["source"], "host(ip)");
        assert!(mock.queries().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tcp_upstream_with_trace_shows_attempts() {
        let mock = MockDns::spawn().await;
        mock.set("t.test", &["10.0.0.7"], &[], 60);
        let dir = workspace();
        let server = format!("tcp://{}", mock.addr());
        let out = output(dns_cmd(
            dir.path(),
            "lookup",
            &server,
            &["t.test", "--type", "a", "--trace"],
        ))
        .await;
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(&format!("source: upstream({server})")),
            "{stdout}"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("rurge_dns::fanout")
                && stderr.contains("send")
                && stderr.contains("answer"),
            "{stderr}"
        );
        assert_eq!(mock.queries()[0].1, "tcp");

        // A `[Host]` entry answers with both families whatever `--type` says,
        // so `--type a` has to drop the v6 address the same way `--type aaaa`
        // drops the v4 one.
        let both = output(dns_cmd(
            dir.path(),
            "lookup",
            &server,
            &["dual.test", "--json"],
        ))
        .await;
        assert_eq!(
            json_of(&both)["addresses"],
            serde_json::json!(["1.2.3.4", "fd00::9"])
        );
        let v4_only = output(dns_cmd(
            dir.path(),
            "lookup",
            &server,
            &["dual.test", "--type", "a", "--json"],
        ))
        .await;
        assert_eq!(v4_only.status.code(), Some(0));
        assert_eq!(
            json_of(&v4_only)["addresses"],
            serde_json::json!(["1.2.3.4"]),
            "--type a must not print a v6 address"
        );
        let v6_only = output(dns_cmd(
            dir.path(),
            "lookup",
            &server,
            &["dual.test", "--type", "aaaa", "--json"],
        ))
        .await;
        assert_eq!(
            json_of(&v6_only)["addresses"],
            serde_json::json!(["fd00::9"])
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dot_and_doh_upstreams_via_server_override() {
        use rurge_dns::message::{Question, Rcode, build_response};
        use rurge_net::testing::TestServer;
        let dir = workspace();
        std::fs::write(
            dir.path().join("t.conf"),
            CONF.replace(
                "ipv6 = false",
                "ipv6 = false
encrypted-dns-skip-cert-verification = true",
            ),
        )
        .unwrap();
        let dot = MockDns::spawn_tls().await;
        dot.set("s.test", &["10.0.0.5"], &[], 60);
        let out = output(dns_cmd(
            dir.path(),
            "lookup",
            &format!("tls://{}", dot.addr()),
            &["s.test", "--json"],
        ))
        .await;
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(json_of(&out)["addresses"][0], "10.0.0.5");
        let doh = TestServer::spawn_tls().await;
        let q = Question {
            name: "s.test".to_string(),
            qtype: Qtype::A,
        };
        doh.set(
            "/dns-query",
            build_response(
                0,
                &q,
                Rcode::NoError,
                &[("10.0.0.6".parse().unwrap(), 60)],
                false,
            )
            .unwrap(),
        );
        doh.set_header("/dns-query", "content-type", "application/dns-message");
        let out = output(dns_cmd(
            dir.path(),
            "lookup",
            doh.url("/dns-query").as_str(),
            &["s.test", "--json"],
        ))
        .await;
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v = json_of(&out);
        assert_eq!(v["addresses"][0], "10.0.0.6");
        assert_eq!(v["source"], format!("upstream({})", doh.url("/dns-query")));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dns_cache_lists_warmed_entries() {
        let mock = MockDns::spawn().await;
        mock.set("c.test", &["10.0.0.3"], &[], 60);
        mock.set_empty("nx.test");
        let dir = workspace();
        let server = mock.addr().to_string();
        let out = output(dns_cmd(
            dir.path(),
            "cache",
            &server,
            &["c.test", "nx.test", "--json"],
        ))
        .await;
        assert_eq!(out.status.code(), Some(0));
        let v = json_of(&out);
        let entries = v["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        let c = entries.iter().find(|e| e["name"] == "c.test").unwrap();
        assert_eq!(c["v4"][0], "10.0.0.3");
        assert_eq!(c["negative"], false);
        let nx = entries.iter().find(|e| e["name"] == "nx.test").unwrap();
        assert_eq!(nx["negative"], true);
        let text = output(dns_cmd(dir.path(), "cache", &server, &["c.test"])).await;
        let stdout = String::from_utf8_lossy(&text.stdout);
        assert!(
            stdout.contains("entries: 1") && stdout.contains("c.test"),
            "{stdout}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invalid_server_spec_exits_two() {
        let dir = workspace();
        let out = output(dns_cmd(dir.path(), "lookup", "nonsense", &["a.test"])).await;
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("invalid --server"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rule_match_resolves_through_the_profile_resolver() {
        let mock = MockDns::spawn().await;
        mock.set("ip-rule.test", &["10.1.2.3"], &[], 60);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("t.conf"),
            format!(
                "[General]\ndns-server = {}\nipv6 = false\n[Proxy]\nP = direct\n[Rule]\nIP-CIDR,10.0.0.0/8,P\nFINAL,DIRECT\n",
                mock.addr()
            ),
        )
        .unwrap();
        let mut cmd = Command::cargo_bin("rurge").unwrap();
        cmd.arg("rule")
            .arg("match")
            .arg("-c")
            .arg(dir.path().join("t.conf"))
            .arg("--no-network")
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            .arg("ip-rule.test")
            .arg("--json");
        let out = output(cmd).await;
        let v = json_of(&out);
        assert_eq!(v["policy"], "P");
        assert_eq!(v["resolved"]["v4"][0], "10.1.2.3");
        assert_eq!(mock.query_count("ip-rule.test", Qtype::A), 1);
    }
}

mod run {
    use rurge_net::testing::TestServer;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    struct Daemon {
        child: Child,
        http: u16,
        socks: u16,
        lines: mpsc::Receiver<String>,
    }

    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn write_conf(dir: &Path, general: &str) -> std::path::PathBuf {
        let conf = dir.join("t.conf");
        std::fs::write(
            &conf,
            format!(
                "[General]\n{general}\n[Proxy]\n[Rule]\nDOMAIN,ads.test,REJECT\nFINAL,DIRECT\n"
            ),
        )
        .unwrap();
        conf
    }

    /// Spawns `rurge run` and waits for both `listening on` lines.
    fn spawn_daemon(conf: &Path, data: &Path) -> Daemon {
        spawn_daemon_with(conf, data, false, None)
    }

    /// `spawn_daemon` with `--watch`, so edits to `conf` are reloaded.
    fn spawn_daemon_watching(conf: &Path, data: &Path) -> Daemon {
        spawn_daemon_with(conf, data, true, None)
    }

    /// `spawn_daemon` with `--log-file <log>`.
    fn spawn_daemon_with_log(conf: &Path, data: &Path, log: &Path) -> Daemon {
        spawn_daemon_with(conf, data, false, Some(log))
    }

    fn spawn_daemon_with(conf: &Path, data: &Path, watch: bool, log_file: Option<&Path>) -> Daemon {
        spawn_daemon_full(conf, data, watch, log_file, &[])
    }

    fn spawn_daemon_full(
        conf: &Path,
        data: &Path,
        watch: bool,
        log_file: Option<&Path>,
        extra: &[&str],
    ) -> Daemon {
        let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("rurge"));
        cmd.arg("run")
            .arg("-c")
            .arg(conf)
            .arg("--no-network")
            .arg("--data-dir")
            .arg(data);
        if watch {
            cmd.arg("--watch");
        }
        if let Some(log) = log_file {
            cmd.arg("--log-file").arg(log);
        }
        cmd.args(extra);
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        // Own the child before anything can panic, so no path leaks the process.
        let mut daemon = Daemon {
            child,
            http: 0,
            socks: 0,
            lines: rx,
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while daemon.http == 0 || daemon.socks == 0 {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let line = daemon
                .lines
                .recv_timeout(remaining)
                .expect("rurge run printed its listening lines");
            if let Some(rest) = line.strip_prefix("listening on http://") {
                daemon.http = rest
                    .rsplit(':')
                    .next()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("listening on socks5://") {
                daemon.socks = rest
                    .rsplit(':')
                    .next()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(0);
            }
        }
        daemon
    }

    fn http_get(port: u16, url: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let host = url.trim_start_matches("http://").split('/').next().unwrap();
        write!(
            s,
            "GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    /// Next stdout line starting with `prefix`, without the prefix.
    fn wait_for_line(daemon: &Daemon, prefix: &str) -> String {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let line = daemon
                .lines
                .recv_timeout(remaining)
                .unwrap_or_else(|_| panic!("rurge run never printed a `{prefix}` line"));
            if let Some(rest) = line.strip_prefix(prefix) {
                return rest.to_string();
            }
        }
    }

    fn api_port(daemon: &Daemon) -> u16 {
        wait_for_line(daemon, "api on http://")
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .expect("api port")
    }

    /// Minimal HTTP/1.1 call against the API: (status, body).
    fn api_call(
        port: u16,
        method: &str,
        path: &str,
        key: &str,
        body: Option<&str>,
    ) -> (u16, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let body = body.unwrap_or("");
        write!(
            s,
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Key: {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        let status = out
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = out
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        (status, body)
    }

    fn wait_for_exit(daemon: &mut Daemon, secs: u64) -> Option<i32> {
        let deadline = std::time::Instant::now() + Duration::from_secs(secs);
        loop {
            if let Ok(Some(status)) = daemon.child.try_wait() {
                return status.code();
            }
            if std::time::Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_proxies_http_and_rejects_by_rule() {
        let target = TestServer::spawn().await;
        target.set("/hello", "hi from target");
        let port = target.url("/").port().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(
            dir.path(),
            "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning",
        );
        let daemon = tokio::task::spawn_blocking({
            let conf = conf.clone();
            let data = dir.path().join("data");
            move || spawn_daemon(&conf, &data)
        })
        .await
        .unwrap();
        let http_port = daemon.http;
        assert!(daemon.socks > 0);
        let ok = tokio::task::spawn_blocking(move || {
            http_get(http_port, &format!("http://127.0.0.1:{port}/hello"))
        })
        .await
        .unwrap();
        assert!(
            ok.starts_with("HTTP/1.1 200") && ok.ends_with("hi from target"),
            "{ok}"
        );
        let rejected = tokio::task::spawn_blocking(move || http_get(http_port, "http://ads.test/"))
            .await
            .unwrap();
        assert!(
            rejected.is_empty(),
            "REJECT closes the connection: {rejected}"
        );
        assert_eq!(target.requests().len(), 1);
        let summary = daemon.lines.try_iter().find(|l| l.contains("running:"));
        assert!(
            summary
                .as_deref()
                .is_some_and(|l| l.contains("outbound mode rule")),
            "{summary:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_shuts_down_gracefully_on_sigint() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(
            dir.path(),
            "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning",
        );
        let daemon = tokio::task::spawn_blocking({
            let conf = conf.clone();
            let data = dir.path().join("data");
            move || spawn_daemon(&conf, &data)
        })
        .await
        .unwrap();
        let pid = daemon.child.id().to_string();
        let sent = std::process::Command::new("kill")
            .args(["-INT", &pid])
            .status()
            .unwrap();
        assert!(sent.success(), "kill -INT");
        // exit 0 within 10 s, and the shutdown line was printed
        let finished = tokio::task::spawn_blocking(move || {
            let mut daemon = daemon;
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(Some(status)) = daemon.child.try_wait() {
                    let lines: Vec<String> = daemon.lines.try_iter().collect();
                    return Some((status, lines));
                }
                if std::time::Instant::now() > deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
        .await
        .unwrap();
        let (status, lines) = finished.expect("daemon exited within 10 s of SIGINT");
        assert!(status.success(), "exit code 0: {status:?}");
        assert!(
            lines.iter().any(|l| l.contains("shutting down")),
            "{lines:?}"
        );
    }

    /// Both profiles listen on `127.0.0.1:0`, so the listen addresses do not
    /// change and the ports read at startup stay valid across the reload.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watch_reloads_rules_on_change() {
        let target = TestServer::spawn().await;
        target.set("/hello", "reloaded");
        let tport = target.url("/").port().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("t.conf");
        std::fs::write(
            &conf,
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning\n[Rule]\nIP-CIDR,127.0.0.0/8,REJECT\nFINAL,DIRECT\n",
        )
        .unwrap();
        let daemon = tokio::task::spawn_blocking({
            let conf = conf.clone();
            let data = dir.path().join("data");
            move || spawn_daemon_watching(&conf, &data)
        })
        .await
        .unwrap();
        let http = daemon.http;
        let before = tokio::task::spawn_blocking(move || {
            http_get(http, &format!("http://127.0.0.1:{tport}/hello"))
        })
        .await
        .unwrap();
        assert!(
            !before.contains("reloaded"),
            "rejected before reload: {before}"
        );
        // allow everything; same listen addresses, so no rebind and the same port
        std::fs::write(
            &conf,
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning\n[Rule]\nFINAL,DIRECT\n",
        )
        .unwrap();
        let ok = tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            loop {
                if http_get(http, &format!("http://127.0.0.1:{tport}/hello")).contains("reloaded") {
                    return true;
                }
                if std::time::Instant::now() > deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        })
        .await
        .unwrap();
        drop(daemon);
        assert!(ok, "reload did not take effect");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn log_file_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(
            dir.path(),
            "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = notify",
        );
        let logdir = dir.path().join("logs");
        std::fs::create_dir_all(&logdir).unwrap();
        let log = logdir.join("rurge.log");
        let daemon = tokio::task::spawn_blocking({
            let conf = conf.clone();
            let data = dir.path().join("data");
            let log = log.clone();
            move || spawn_daemon_with_log(&conf, &data, &log)
        })
        .await
        .unwrap();
        // Poll the rotated file for the one INFO line the daemon writes at
        // startup: `loglevel = notify` maps to INFO, and the per-listener
        // "listening" records are DEBUG, so this is what proves the file layer
        // is wired up rather than merely created.
        // Windows note: `DirEntry::metadata()` reuses the `FindNextFileW`
        // snapshot taken when the directory was enumerated, so its cached size
        // never grows while `rurge run` still holds the file open for writing;
        // reading by path does see the live content.
        let ok = tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(rd) = std::fs::read_dir(&logdir)
                    && rd.filter_map(|e| e.ok()).any(|e| {
                        e.file_name().to_string_lossy().starts_with("rurge.log")
                            && std::fs::read_to_string(e.path())
                                .map(|t| t.contains("rurge running"))
                                .unwrap_or(false)
                    })
                {
                    return true;
                }
                if std::time::Instant::now() > deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        })
        .await
        .unwrap();
        drop(daemon);
        assert!(ok, "no rurge.log* file carried the startup INFO line");
    }

    #[test]
    fn run_exits_2_on_a_broken_profile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("t.conf"),
            "[General]\n[Proxy]\n[Rule]\nDOMAIN,a.test,DIRECT\n",
        )
        .unwrap(); // no FINAL
        let status = Command::new(assert_cmd::cargo::cargo_bin("rurge"))
            .args(["run", "-c"])
            .arg(dir.path().join("t.conf"))
            .arg("--no-network")
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(2));
    }

    #[test]
    fn run_exits_1_when_the_port_is_taken() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(
            dir.path(),
            &format!("http-listen = 127.0.0.1:{port}\nsocks5-listen = 127.0.0.1:0"),
        );
        let output = Command::new(assert_cmd::cargo::cargo_bin("rurge"))
            .args(["run", "-c"])
            .arg(&conf)
            .arg("--no-network")
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot bind listener"));
        drop(taken);
    }

    const API_GENERAL: &str = "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nhttp-api = k@127.0.0.1:0\nloglevel = warning";

    #[test]
    fn run_serves_the_api_and_persists_the_outbound_mode() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        // 1. fresh start: rule mode; change it through the API
        let d1 = spawn_daemon(&conf, &data);
        let port = api_port(&d1);
        let (status, body) = api_call(port, "GET", "/v1/outbound", "k", None);
        assert_eq!((status, body.as_str()), (200, r#"{"mode":"rule"}"#));
        assert_eq!(api_call(port, "GET", "/v1/outbound", "wrong", None).0, 401);
        assert_eq!(
            api_call(
                port,
                "POST",
                "/v1/outbound/global",
                "k",
                Some(r#"{"policy":"DIRECT"}"#)
            )
            .0,
            200
        );
        assert_eq!(
            api_call(
                port,
                "POST",
                "/v1/outbound",
                "k",
                Some(r#"{"mode":"direct"}"#)
            )
            .0,
            200
        );
        drop(d1);
        let state = std::fs::read_to_string(data.join("state.json")).unwrap();
        assert!(
            state.contains("\"outbound_mode\": \"direct\"")
                && state.contains("\"global_policy\": \"DIRECT\""),
            "{state}"
        );
        // 2. restart without flags: state.json wins over the default
        let d2 = spawn_daemon(&conf, &data);
        let port = api_port(&d2);
        assert_eq!(
            api_call(port, "GET", "/v1/outbound", "k", None).1,
            r#"{"mode":"direct"}"#
        );
        assert_eq!(
            api_call(port, "GET", "/v1/outbound/global", "k", None).1,
            r#"{"policy":"DIRECT"}"#
        );
        let summary = wait_for_line(&d2, "rurge ");
        assert!(summary.contains("outbound mode direct"), "{summary}");
        drop(d2);
        // 3. an explicit flag wins over state.json and is written back
        let d3 = spawn_daemon_full(&conf, &data, false, None, &["--outbound-mode", "rule"]);
        let port = api_port(&d3);
        assert_eq!(
            api_call(port, "GET", "/v1/outbound", "k", None).1,
            r#"{"mode":"rule"}"#
        );
        drop(d3);
        let state = std::fs::read_to_string(data.join("state.json")).unwrap();
        assert!(state.contains("\"outbound_mode\": \"rule\""), "{state}");
    }

    #[test]
    fn run_reloads_and_stops_via_the_api() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let mut daemon = spawn_daemon(&conf, &dir.path().join("data"));
        let port = api_port(&daemon);
        let (status, body) = api_call(port, "POST", "/v1/profiles/reload", "k", Some("{}"));
        assert_eq!(status, 200, "{body}");
        assert!(
            body.contains("\"ok\":true") && body.contains("\"listenersRebound\":false"),
            "{body}"
        );
        assert_eq!(
            api_call(
                port,
                "POST",
                "/v1/log/level",
                "k",
                Some(r#"{"level":"verbose"}"#)
            )
            .0,
            200
        );
        assert_eq!(
            api_call(port, "POST", "/v1/stop", "k", Some("{}")),
            (200, "{}".to_string())
        );
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(0), "stop exits 0");
    }

    #[test]
    fn run_exits_1_when_the_api_port_is_taken() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(
            dir.path(),
            &format!(
                "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nhttp-api = k@127.0.0.1:{port}\nloglevel = warning"
            ),
        );
        let mut daemon = spawn_daemon(&conf, &dir.path().join("data"));
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(1));
    }
}
