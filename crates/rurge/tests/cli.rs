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
