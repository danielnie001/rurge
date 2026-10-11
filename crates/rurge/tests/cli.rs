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

/// clap prints `[env: NAME=<value>]` in an option's help unless told not to.
/// The API's key, the two GeoIP links (MaxMind's carry the account's licence
/// key) and the two listen addresses (an HTTP one can carry the proxy's
/// password) have their variable named in the help and the value left out.
#[test]
fn help_names_a_credential_variable_and_leaves_its_value_out() {
    let mut printed = Vec::new();
    for (command, variable, value) in [
        ("status", "RURGE_API_KEY", "S3CR3T-key"),
        ("run", "RURGE_LISTEN_HTTP", "S3CR3T-http@127.0.0.1:6152"),
        ("run", "RURGE_LISTEN_SOCKS5", "S3CR3T-socks@127.0.0.1:6153"),
        (
            "run",
            "RURGE_GEOIP_URL",
            "https://download.example/geoip?license_key=S3CR3T-geoip",
        ),
        (
            "run",
            "RURGE_GEOIP_ASN_URL",
            "https://download.example/asn?license_key=S3CR3T-asn",
        ),
    ] {
        let out = Command::cargo_bin("rurge")
            .unwrap()
            .args([command, "--help"])
            .env(variable, value)
            .output()
            .unwrap();
        assert!(out.status.success(), "{command} --help");
        let help = String::from_utf8_lossy(&out.stdout);
        assert!(help.contains(variable), "{command} --help: {help}");
        if help.contains("S3CR3T") {
            printed.push(variable);
        }
    }
    assert!(printed.is_empty(), "--help prints the value of {printed:?}");
}

/// What an option does to the profile's own listeners is said in its help: it
/// takes the place of every address of its kind, not just the first. And a
/// password given on the command line is there in the process list for anyone
/// to read, which the environment variable avoids.
#[test]
fn help_says_that_a_listen_option_replaces_every_address_of_its_kind() {
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["run", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    // clap wraps the text to the width of the terminal: read it as words
    let help = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for said in [
        "every http-listen address",
        "every socks5-listen address",
        "process list",
    ] {
        assert!(help.contains(said), "`{said}` is not in: {help}");
    }
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

/// A problem is reported with what to do about it: in the text the hint is
/// the line after its diagnostic, in the JSON it is a field of it, and a
/// code that has no hint has `null`.
#[test]
fn check_says_what_to_do_about_a_problem() {
    let dir = tempfile::tempdir().unwrap();
    let said = |code: &str| format!("  hint: {}", rurge_config::codes::hint(code).unwrap());
    let stdout = |file: &str, extra: &[&str]| {
        let out = Command::cargo_bin("rurge")
            .unwrap()
            .args(["check", "-c"])
            .arg(file)
            .args(extra)
            .output()
            .unwrap()
            .stdout;
        String::from_utf8_lossy(&out).into_owned()
    };

    let warning = write(&dir, "warning.conf", WARNING);
    let out = stdout(warning.to_str().unwrap(), &[]);
    let lines: Vec<&str> = out.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with("warning[W0001]"))
        .unwrap_or_else(|| panic!("{out}"));
    assert_eq!(lines[at + 1], said("W0001"), "{out}");

    // an error that had its hint before the table of hints
    let error = write(&dir, "error.conf", ERROR);
    let out = stdout(error.to_str().unwrap(), &[]);
    assert!(
        out.contains(
            "the [Rule] section must end with an enabled FINAL rule\n  hint: add `FINAL,DIRECT` as the last rule\n"
        ),
        "{out}"
    );

    let ios = write(
        &dir,
        "ios.conf",
        "[General]\ncompatibility-mode = true\n[Rule]\nFINAL,DIRECT\n",
    );
    let both = write(
        &dir,
        "both.conf",
        "[General]\nmystery = 1\ncompatibility-mode = true\n[Rule]\nFINAL,DIRECT\n",
    );
    let out = stdout(both.to_str().unwrap(), &["--json"]);
    let report: serde_json::Value = serde_json::from_str(&out).unwrap();
    let diagnostics = report["diagnostics"].as_array().expect("diagnostics");
    let hint_of = |code: &str| {
        diagnostics
            .iter()
            .find(|d| d["code"] == code)
            .unwrap_or_else(|| panic!("{code} is not in {out}"))["hint"]
            .clone()
    };
    assert_eq!(
        hint_of("W0001"),
        rurge_config::codes::hint("W0001").unwrap()
    );
    assert!(hint_of("W0004").is_null(), "{out}");
    // and the text of a code without one has no hint line
    let out = stdout(ios.to_str().unwrap(), &[]);
    assert!(!out.contains("hint:"), "{out}");
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

/// rurge reports `CORE_VERSION` 22, the version that brought the `smart`
/// group: a line that asks for 22 is read, one that asks for 23 is not.
#[test]
fn check_reports_core_version_22() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write(
        &dir,
        "core-version.conf",
        "[General]\n[Proxy]\nH = http, proxy.test, 8080\n[Proxy Group]\n\
#!REQUIREMENT CORE_VERSION>=22 Smart = smart, H, DIRECT\n\
#!REQUIREMENT CORE_VERSION>=23 Later = smart, H, DIRECT\n[Rule]\nFINAL,Smart\n",
    );
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "--json", "-c"])
        .arg(&conf)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let skipped: Vec<&str> = v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "I0002")
        .map(|d| d["message"].as_str().unwrap())
        .collect();
    assert_eq!(
        skipped,
        ["line disabled: requirement `CORE_VERSION>=23` not met"],
        "{v}"
    );
}

/// The one line that can give the profile its FINAL rule is on only where the
/// system has a version of its own to report, in the words of the manual's
/// example (`System Version 17.4.1 (Build 21E236)`).
const NEEDS_A_SYSTEM_VERSION: &str = "[General]\n[Rule]\n\
#!REQUIREMENT \"SYSTEM_VERSION BEGINSWITH 'System Version '\" FINAL,DIRECT\n";

/// `SYSTEM_VERSION` is read from the operating system on every platform; a
/// version that cannot be read leaves the profile without its FINAL rule.
/// Asking the system adds nothing to what `check` prints. Only on Windows,
/// macOS and Linux: any other Unix is read the way Linux is, and may have no
/// `/proc/sys/kernel/osrelease`.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
#[test]
fn check_reads_the_system_version_from_the_operating_system() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write(&dir, "system-version.conf", NEEDS_A_SYSTEM_VERSION);
    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(&conf)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "0 error(s), 0 warning(s), 0 note(s)",
        ))
        .stderr(predicate::str::is_empty());
}

/// `--platform` evaluates the profile as if on another system. This machine's
/// version is not that system's, so the line stays off.
#[test]
fn check_does_not_give_this_systems_version_to_another_platform() {
    let other = if cfg!(windows) { "linux" } else { "windows" };
    let dir = tempfile::tempdir().unwrap();
    let conf = write(&dir, "system-version.conf", NEEDS_A_SYSTEM_VERSION);
    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "--platform", other, "-c"])
        .arg(&conf)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0010"));
}

const PROXIES: &str = "[General]\n[Proxy]\nH = http, proxy.test, 8080\nS = socks5-tls, proxy.test, 443\nOld = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n";
const BROKEN_P12: &str = "[General]\n[Proxy]\nUp = https, proxy.test, 443, client-cert=cert1\n[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n[Rule]\nFINAL,DIRECT\n";

#[test]
fn check_knows_the_m1_protocols_and_runs_the_dry_build() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "proxies.conf", PROXIES))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // only `tailscale`, which rurge does not implement, is "not supported"
    // (W0007 is deduped per protocol kind and names the kind, not the
    // policy), and the text says what reaches a tailnet instead
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("policy type `tailscale` is not supported")
            && out.contains("tailscaled's SOCKS5 proxy"),
        "{out}"
    );

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "broken.conf", BROKEN_P12))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0022"))
        .stdout(predicate::str::contains("broken.conf:3"))
        .stdout(predicate::str::contains("hunter2").not());
}

const GROUPS: &str = "[General]\n[Proxy]\n\
H = http, proxy.test, 8080, test-url=http://127.0.0.1:9/, test-timeout=3\n[Proxy Group]\n\
U = url-test, H, DIRECT, tolerance=50\nF = fallback, H, DIRECT, evaluate-before-use=true\n\
L = load-balance, H, DIRECT, persistent=true\nS = smart, H, DIRECT, policy-priority=\"H:0.8\"\n\
N = subnet, default=H\n[Rule]\nFINAL,U\n";

#[test]
fn check_knows_the_automatic_groups() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "groups.conf", GROUPS))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `subnet` is still a later phase; the four automatic groups are not,
    // and the testing options are in effect (no W0029)
    assert_eq!(out.matches("W0008").count(), 1, "{out}");
    assert!(out.contains("`subnet`"), "{out}");
    assert!(!out.contains("`smart`"), "{out}");
    assert!(!out.contains("W0029"), "{out}");
}

const TROJAN: &str = "[General]\n[Proxy]\nT = trojan, proxy.test, 443, password=s3same, ws=true, ws-path=/w\nOld = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n";
const TROJAN_BAD_PATH: &str = "[General]\n[Proxy]\nT = trojan, proxy.test, 443, password=s3same, ws=true, ws-path=s3cretpath\n[Rule]\nFINAL,DIRECT\n";

#[test]
fn check_knows_trojan() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "trojan.conf", TROJAN))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `tailscale` is not supported; `trojan` is
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`tailscale`") && !out.contains("`trojan`"),
        "{out}"
    );

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", TROJAN_BAD_PATH))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0018"))
        .stdout(predicate::str::contains("bad.conf:3"))
        .stdout(predicate::str::contains("s3cretpath").not())
        .stdout(predicate::str::contains("s3same").not());
}

const VMESS_ANYTLS: &str = "[General]\n[Proxy]\n\
V = vmess, proxy.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true, tls=true, ws=true, ws-path=/w\n\
A = anytls, proxy.test, 443, password=s3same\n\
Legacy1 = vmess, proxy.test, 80, username=0233d11c-15a4-47d3-ade3-48ffca0ce119\n\
Legacy2 = vmess, proxy.test, 80, username=0233d11c-15a4-47d3-ade3-48ffca0ce119\n\
[Rule]\nFINAL,DIRECT\n";
const VMESS_BAD_ID: &str = "[General]\n[Proxy]\nV = vmess, proxy.test, 443, username=s3cretnotauuid, vmess-aead=true\n[Rule]\nFINAL,DIRECT\n";

#[test]
fn check_knows_vmess_and_anytls() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "m2b.conf", VMESS_ANYTLS))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // a line without `vmess-aead=true` gets the AEAD handshake all the same:
    // nothing is "not supported", and one W0029 says so — once, not per line
    assert_eq!(out.matches("W0007").count(), 0, "{out}");
    assert_eq!(out.matches("W0029").count(), 1, "{out}");
    assert!(
        out.contains(
            "m2b.conf:5: `vmess` without `vmess-aead=true` uses the AEAD handshake all the same: rurge has no other, and every server since V2Ray 4.28.1 accepts it\n"
        ),
        "{out}"
    );
    assert!(out.contains("0 error(s), 1 warning(s)"), "{out}");
    assert!(
        !out.contains("`anytls`") && !out.contains("0233d11c"),
        "{out}"
    );

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", VMESS_BAD_ID))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0018"))
        .stdout(predicate::str::contains("bad.conf:3"))
        .stdout(predicate::str::contains("s3cretnotauuid").not());
}

const SSH: &str = "[General]\n[Proxy]\n\
S = ssh, proxy.test, 22, username=u, password=s3same, idle-timeout=60\n\
Old = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n";
const SSH_BAD_KEY: &str = "[General]\n[Proxy]\nS = ssh, proxy.test, 22, username=u, private-key=key1\n\
[Keystore]\nkey1 = type=openssh-private-key, base64=c2VjcmV0IGtleSBtYXRlcmlhbA==\n[Rule]\nFINAL,DIRECT\n";

/// `rurge check` builds `ssh` policies: a key rurge cannot use is an error,
/// named and not quoted (M4 design 4.5).
#[test]
fn check_knows_ssh() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "ssh.conf", SSH))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `tailscale` is not supported; `ssh` is
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`tailscale`") && !out.contains("`ssh`"),
        "{out}"
    );
    assert!(!out.contains("s3same"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", SSH_BAD_KEY))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0022"))
        .stdout(predicate::str::contains("bad.conf:3"))
        .stdout(predicate::str::contains(
            "keystore item `key1` is not an OpenSSH private key",
        ))
        .stdout(predicate::str::contains("c2VjcmV0").not());
}

const WIREGUARD: &str = "[General]\n[Proxy]\nW = wireguard, section-name=home\n\
Old = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n\
[WireGuard home]\nprivate-key = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\nself-ip = 10.9.0.2\n\
peer = (public-key = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=, allowed-ips = 0.0.0.0/0, endpoint = vpn.test:51820)\n";
const WIREGUARD_BAD_KEY: &str = "[General]\n[Proxy]\nW = wireguard, section-name=home\n[Rule]\nFINAL,DIRECT\n\
[WireGuard home]\nprivate-key = c2VjcmV0IGtleSBtYXRlcmlhbA==\nself-ip = 10.9.0.2\n\
peer = (public-key = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=, allowed-ips = 0.0.0.0/0, endpoint = vpn.test:51820)\n";

/// `rurge check` knows `wireguard` policies and their sections: a key that
/// is no key is an error at its line, named and not quoted (M4 design 4.1).
#[test]
fn check_knows_wireguard() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "wg.conf", WIREGUARD))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `tailscale` is not supported; `wireguard` is
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`tailscale`") && !out.contains("`wireguard`"),
        "{out}"
    );
    assert!(!out.contains("yAnz5TF"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", WIREGUARD_BAD_KEY))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0023"))
        .stdout(predicate::str::contains(
            "bad.conf:7: [WireGuard home]: `private-key` is not a 32-byte key in Base64 or hex",
        ))
        // and the policy that names it
        .stdout(predicate::str::contains(
            "bad.conf:3: policy `W`: `section-name` names `[WireGuard home]`, which does not exist or has errors",
        ))
        .stdout(predicate::str::contains("c2VjcmV0").not());
}

const EXTERNAL: &str = "[General]\n[Proxy]\n\
X = external, exec = \"/usr/bin/sshpass\", args = -p, args = hunter2, args = ssh, local-port = 1080\n\
Old = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n";
const EXTERNAL_SAME_PORT: &str = "[General]\n[Proxy]\n\
X = external, exec = /bin/x, local-port = 1080\nY = external, exec = /bin/y, local-port = 1080\n\
[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `external` policies and starts nothing; two on one
/// `local-port` are an error at the second (M4 design 4.4, 7.5).
#[test]
fn check_knows_external() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "x.conf", EXTERNAL))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `tailscale` is not supported (its text names `external` as a way to
    // a tailnet); `external` is
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("policy type `tailscale`") && !out.contains("policy type `external`"),
        "{out}"
    );
    assert!(!out.contains("hunter2"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "same.conf", EXTERNAL_SAME_PORT))
        .assert()
        .code(2)
        .stdout(predicate::str::contains(
            "same.conf:4: policy `Y`: `local-port` 1080 is also the `local-port` of policy `X`",
        ));
}

const SS: &str = "[General]\n[Proxy]\n\
A = ss, proxy.test, 8388, encrypt-method=chacha20-ietf-poly1305, password=s3same, obfs=http\n\
K = ss, proxy.test, 8388, encrypt-method=2022-blake3-aes-128-gcm, password=MDEyMzQ1Njc4OWFiY2RlZg==, udp-relay=true\n\
Cfb1 = ss, proxy.test, 8388, encrypt-method=aes-256-cfb, password=s3same\n\
Cfb2 = ss, proxy.test, 8388, encrypt-method=AES-256-CFB, password=s3same, udp-relay=true\n\
Rc = ss, proxy.test, 8388, encrypt-method=rc4, password=s3same\n[Rule]\nFINAL,DIRECT\n";
const SS_NOT_SUPPORTED: &str = "[General]\n[Proxy]\n\
Bf1 = ss, proxy.test, 8388, encrypt-method=bf-cfb, password=s3same\n\
Bf2 = ss, proxy.test, 8388, encrypt-method=bf-cfb, password=s3same\n[Rule]\nFINAL,DIRECT\n";
const SS_BAD_KEY: &str = "[General]\n[Proxy]\n\
K = ss, proxy.test, 8388, encrypt-method=2022-blake3-aes-128-gcm, password=c2VjcmV0IGtleSBtYXRlcmlhbA==\n\
[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `ss` (phase 2 M6 design 6) with the stream ciphers
/// of the manual (phase 2 M8 design 4.1): each is a warning of its own
/// code, once however many lines use it, and `rc4` is told what else is
/// wrong with it. A cipher Surge no longer lists is "not supported", once
/// too. An SS 2022 key of the wrong length is an error, named and not
/// quoted (M6 design 3.1).
#[test]
fn check_knows_ss() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "ss.conf", SS))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert_eq!(out.matches("W0007").count(), 0, "{out}");
    assert_eq!(out.matches("W0031").count(), 2, "{out}");
    assert!(
        out.contains(
            "ss.conf:5: `ss` cipher `aes-256-cfb` is a stream cipher: it gives no integrity protection and is deprecated; use an AEAD cipher\n"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "ss.conf:7: `ss` cipher `rc4` is a stream cipher: it gives no integrity protection and is deprecated; use an AEAD cipher; `rc4` also repeats its keystream on every connection\n"
        ),
        "{out}"
    );
    assert!(out.contains("0 error(s), 2 warning(s)"), "{out}");
    assert!(!out.contains("policy type `ss`"), "{out}");
    assert!(!out.contains("s3same") && !out.contains("MDEyMz"), "{out}");

    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "gone.conf", SS_NOT_SUPPORTED))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert_eq!(out.matches("W0031").count(), 0, "{out}");
    assert!(
        out.contains(
            "gone.conf:3: `ss` cipher `bf-cfb` is not supported (Surge no longer lists it); such policies behave as REJECT\n"
        ),
        "{out}"
    );
    assert!(!out.contains("s3same"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", SS_BAD_KEY))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0018"))
        .stdout(predicate::str::contains(
            "bad.conf:3: policy `K`: key #1 of `password` is not a Base64 key of 16 bytes, as `2022-blake3-aes-128-gcm` requires",
        ))
        .stdout(predicate::str::contains("c2VjcmV0").not());
}

const SNELL: &str = "[General]\n[Proxy]\n\
N = snell, proxy.test, 443, psk=s3cretPsk, version=4, reuse=true, obfs=http\n\
Old1 = snell, proxy.test, 443, psk=s3cretPsk\n\
Old2 = snell, proxy.test, 443, psk=s3cretPsk, version=1, obfs=tls\n\
Old3 = snell, proxy.test, 443, psk=s3cretPsk, version=2\n\
Old4 = snell, proxy.test, 443, psk=s3cretPsk, version=3\n[Rule]\nFINAL,DIRECT\n";
const SNELL_V6: &str = "[General]\n[Proxy]\n\
New1 = snell, proxy.test, 443, psk=s3cretPsk, version=6\n\
New2 = snell, proxy.test, 443, psk=s3cretPsk, version=6, mode=unshaped\n[Rule]\nFINAL,DIRECT\n";
const SNELL_BAD_VERSION: &str = "[General]\n[Proxy]\n\
N = snell, proxy.test, 443, psk=s3cretPsk, version=7\n[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `snell` versions 1 to 5 (phase 2 M6 design 4.2, M8
/// design 4.3): a line that does not say its version is version 1, and is
/// no longer "not implemented". Version 6 is "not supported", once however
/// many lines ask for it; a version out of range is an error; the PSK is
/// never printed.
#[test]
fn check_knows_snell() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "snell.conf", SNELL))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert_eq!(out.matches("W0007").count(), 0, "{out}");
    assert!(out.contains("0 error(s), 0 warning(s)"), "{out}");
    assert!(!out.contains("s3cretPsk"), "{out}");

    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "v6.conf", SNELL_V6))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains(
            "v6.conf:3: `snell` version 6 is not supported: it is a beta protocol without a public specification; such policies behave as REJECT\n"
        ),
        "{out}"
    );
    assert!(!out.contains("policy type `snell`"), "{out}");
    assert!(!out.contains("s3cretPsk"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", SNELL_BAD_VERSION))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0018"))
        .stdout(predicate::str::contains(
            "bad.conf:3: policy `N`: invalid value `7` for `version` (expected an integer from 1 to 6)",
        ))
        .stdout(predicate::str::contains("s3cretPsk").not());
}

const HTTP2: &str = "[General]\n[Proxy]\n\
H = h2-connect, proxy.test, 443, alice, s3cretPw, headers=X-Token:t0kenValue, max-streams=5, udp-relay=true\n\
T1 = trust-tunnel, proxy.test, 443, username=bob, password=s3cretPw, h3=true\n\
T2 = trust-tunnel, proxy.test, 443, username=bob, password=s3cretPw, h3=true\n\
Old = tailscale, section-name=ts\n[Rule]\nFINAL,DIRECT\n";
const HTTP2_NO_PASSWORD: &str = "[General]\n[Proxy]\n\
T = trust-tunnel, proxy.test, 443, username=bob\n[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `h2-connect` and `trust-tunnel` (phase 2 M6 design
/// 5.1): neither is "not implemented"; nothing is said of `h3=true`, which
/// takes effect (phase 2 M7 design 7.2); a `trust-tunnel` without a
/// password is an error; credentials and headers are never printed.
#[test]
fn check_knows_http2() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "http2.conf", HTTP2))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `tailscale` is not supported; the HTTP/2 family is
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`tailscale`")
            && !out.contains("`h2-connect`")
            && !out.contains("`trust-tunnel`"),
        "{out}"
    );
    assert!(!out.contains("W0029"), "{out}");
    assert!(
        !out.contains("s3cretPw") && !out.contains("alice") && !out.contains("t0kenValue"),
        "{out}"
    );

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", HTTP2_NO_PASSWORD))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0018"))
        .stdout(predicate::str::contains(
            "bad.conf:3: policy `T`: `password` is required",
        ))
        .stdout(predicate::str::contains("bob").not());
}

const HYSTERIA2: &str = "[General]\n[Proxy]\n\
H = hysteria2, proxy.test, 443, password=s3cretPw, download-bandwidth=100, salamander-password=0bfsPw, port-hopping=5000-6000, port-hopping-interval=2, ecn=off\n\
Old = tailscale, section-name=ts\nE = socks5, proxy.test, 1080\n\
[Proxy Group]\nG = select, H, underlying-proxy=E\n[Rule]\nFINAL,DIRECT\n";
const HYSTERIA2_CHAINED: &str = "[General]\n[Proxy]\n\
H = hysteria2, proxy.test, 443, password=s3cretPw, port-hopping=5000-6000, underlying-proxy=E\n\
E = socks5, proxy.test, 1080\n[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `hysteria2` (phase 2 M7 design 5.1): it is not "not
/// implemented"; `ecn` has an effect on it; a hop interval under 5 seconds
/// is raised to 5, saying so; a group that relays a member with
/// `port-hopping` says it leaves that member out; `port-hopping` over an
/// `underlying-proxy` is an error; the passwords are never printed.
#[test]
fn check_knows_hysteria2() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "hy2.conf", HYSTERIA2))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `tailscale` is not supported; `hysteria2` is
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`tailscale`") && !out.contains("`hysteria2`"),
        "{out}"
    );
    assert!(!out.contains("W0029"), "{out}");
    assert!(
        out.contains("W0012")
            && out.contains(
                "hy2.conf:3: policy `H`: `port-hopping-interval` is below the minimum of 5 seconds; using 5"
            ),
        "{out}"
    );
    assert!(
        out.contains("W0023")
            && out.contains(
                "hy2.conf:7: policy group `G` relays its members through `underlying-proxy`: `H` uses `port-hopping`, which cannot go through it; `H (via E)` is left out"
            ),
        "{out}"
    );
    assert!(
        !out.contains("s3cretPw") && !out.contains("0bfsPw"),
        "{out}"
    );

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", HYSTERIA2_CHAINED))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0018"))
        .stdout(predicate::str::contains(
            "bad.conf:3: policy `H`: `port-hopping` cannot be combined with `underlying-proxy`",
        ))
        .stdout(predicate::str::contains("s3cretPw").not());
}

const TUIC: &str = "[General]\n[Proxy]\n\
T5 = tuic-v5, proxy.test, 443, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=s3cretPw, port-hopping=5000-6000, port-hopping-interval=2, ecn=off, udp-relay=true\n\
T4 = tuic, proxy.test, 443, token=t0kenValue, tfo=true\n\
Old = tailscale, section-name=ts\nE = socks5, proxy.test, 1080\n\
[Proxy Group]\nG = select, T5, T4, underlying-proxy=E\n[Rule]\nFINAL,DIRECT\n";
const TUIC_BROKEN: &str = "[General]\n[Proxy]\n\
A = tuic-v5, proxy.test, 443, uuid=0233d11c-15a4, password=s3cretPw\n\
B = tuic, proxy.test, 443, token=t0kenValue, port-hopping=5000-6000, underlying-proxy=E\n\
C = tuic, proxy.test, 443, token=t0kenValue, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test\n\
E = socks5, proxy.test, 1080\n[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `tuic` and `tuic-v5` (phase 2 M7 design 6.1):
/// neither is "not implemented"; `ecn` has an effect on them, `tfo` and
/// `udp-relay` do not apply; a hop interval under 5 seconds is raised to 5,
/// saying so; a group that relays a member with `port-hopping` says it
/// leaves that member out; a UUID that is not one, `port-hopping` over an
/// `underlying-proxy` and Shadow TLS are errors; the credentials are never
/// printed.
#[test]
fn check_knows_tuic() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "tuic.conf", TUIC))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `tailscale` is not supported; TUIC is
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("policy type `tailscale`") && !out.contains("policy type `tuic"),
        "{out}"
    );
    assert!(!out.contains("W0029"), "{out}");
    for (code, said) in [
        (
            "W0012",
            "tuic.conf:3: policy `T5`: `port-hopping-interval` is below the minimum of 5 seconds; using 5",
        ),
        (
            "W0028",
            "tuic.conf:3: policy `T5`: `udp-relay` does not apply to `tuic-v5` policies (UDP is always relayed); ignored",
        ),
        (
            "W0028",
            "tuic.conf:4: policy `T4`: `tfo` does not apply to `tuic` policies; ignored",
        ),
        (
            "W0023",
            "tuic.conf:8: policy group `G` relays its members through `underlying-proxy`: `T5` uses `port-hopping`, which cannot go through it; `T5 (via E)` is left out",
        ),
    ] {
        let warning = format!("warning[{code}]");
        assert!(
            out.lines()
                .any(|line| line.starts_with(&warning) && line.ends_with(said)),
            "{code} {said}\n{out}"
        );
    }
    assert!(
        !out.contains("s3cretPw") && !out.contains("t0kenValue") && !out.contains("0233d11c"),
        "{out}"
    );

    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", TUIC_BROKEN))
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    for said in [
        "bad.conf:3: policy `A`: `uuid` is not a valid UUID",
        "bad.conf:4: policy `B`: `port-hopping` cannot be combined with `underlying-proxy`",
        "bad.conf:5: policy `C`: Shadow TLS cannot be combined with a `tuic` policy",
    ] {
        assert!(
            out.lines()
                .any(|line| line.starts_with("error[E0018]") && line.ends_with(said)),
            "{said}\n{out}"
        );
    }
    assert!(
        !out.contains("s3cretPw") && !out.contains("t0kenValue") && !out.contains("0233d11c"),
        "{out}"
    );
}

const MASQUE: &str = "[General]\n[Proxy]\n\
M = masque, proxy.test, 443, username=us3rName, password=s3cretPw, port-hopping=5000-6000, port-hopping-interval=2, ecn=off, udp-relay=true\n\
N = masque, proxy.test, 443, tfo=true\n\
Old = tailscale, section-name=ts\nE = socks5, proxy.test, 1080\n\
[Proxy Group]\nG = select, M, N, underlying-proxy=E\n[Rule]\nFINAL,DIRECT\n";
const MASQUE_BROKEN: &str = "[General]\n[Proxy]\n\
A = masque, proxy.test, 443, username=us3rName\n\
B = masque, proxy.test, 443, port-hopping=5000-6000, underlying-proxy=E\n\
C = masque, proxy.test, 443, username=us3rName, password=s3cretPw, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test\n\
E = socks5, proxy.test, 1080\n[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `masque` (phase 2 M7 design 7.1): it is not "not
/// implemented"; `ecn` has an effect on it, `tfo` and `udp-relay` do not
/// apply; a hop interval under 5 seconds is raised to 5, saying so; a group
/// that relays a member with `port-hopping` says it leaves that member out;
/// one credential without the other, `port-hopping` over an
/// `underlying-proxy` and Shadow TLS are errors; the credentials are never
/// printed.
#[test]
fn check_knows_masque() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "masque.conf", MASQUE))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `tailscale` is not supported; `masque` is
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("policy type `tailscale`") && !out.contains("policy type `masque`"),
        "{out}"
    );
    assert!(!out.contains("W0029"), "{out}");
    for (code, said) in [
        (
            "W0012",
            "masque.conf:3: policy `M`: `port-hopping-interval` is below the minimum of 5 seconds; using 5",
        ),
        (
            "W0028",
            "masque.conf:3: policy `M`: `udp-relay` does not apply to `masque` policies (UDP is always relayed); ignored",
        ),
        (
            "W0028",
            "masque.conf:4: policy `N`: `tfo` does not apply to `masque` policies; ignored",
        ),
        (
            "W0023",
            "masque.conf:8: policy group `G` relays its members through `underlying-proxy`: `M` uses `port-hopping`, which cannot go through it; `M (via E)` is left out",
        ),
    ] {
        let warning = format!("warning[{code}]");
        assert!(
            out.lines()
                .any(|line| line.starts_with(&warning) && line.ends_with(said)),
            "{code} {said}\n{out}"
        );
    }
    assert!(
        !out.contains("s3cretPw") && !out.contains("us3rName"),
        "{out}"
    );

    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", MASQUE_BROKEN))
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    for said in [
        "bad.conf:3: policy `A`: `password` is required with `username`",
        "bad.conf:4: policy `B`: `port-hopping` cannot be combined with `underlying-proxy`",
        "bad.conf:5: policy `C`: Shadow TLS cannot be combined with a `masque` policy",
    ] {
        assert!(
            out.lines()
                .any(|line| line.starts_with("error[E0018]") && line.ends_with(said)),
            "{said}\n{out}"
        );
    }
    assert!(
        !out.contains("s3cretPw") && !out.contains("us3rName"),
        "{out}"
    );
}

const SUBSCRIBED: &str = "[General]\n[Proxy Group]\nLocal = select, DIRECT, policy-path=nodes.txt\n\
Remote = select, DIRECT, policy-path=https://sub.test/nodes?token=t0k3n\n[Rule]\nFINAL,Local\n";

/// Offline: the local file is read and the URL looked for in the data
/// directory's cache — and the URL is never printed (M3 design 5.9, M3-D7).
#[test]
fn check_assembles_the_subscriptions_it_has() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir, "nodes.txt", "N1 = http, n1.test, 80\nnot a policy\n");
    let data = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "--data-dir"])
        .arg(data.path())
        .arg("-c")
        .arg(write(&dir, "sub.conf", SUBSCRIBED))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert!(
        out.contains("W0023") && out.contains("policy group `Local`: `policy-path` line 2 skipped"),
        "{out}"
    );
    assert!(
        out.contains("W0022")
            && out.contains("policy group `Remote`: `policy-path` has no content yet"),
        "{out}"
    );
    assert!(!out.contains("t0k3n"), "{out}");
}

/// One resource used as both a `RULE-SET` and a `DOMAIN-SET` is an error of
/// the profile — the one thing `check` has to say about it, quoting nothing
/// of the reference (a URL can carry a password).
#[test]
fn check_refuses_one_resource_as_two_kinds_of_set() {
    let dir = tempfile::tempdir().unwrap();
    let url = "https://al1ce:S3CR3T@sets.example/u.list";
    let conf = write(
        &dir,
        "kinds.conf",
        &format!("[Rule]\nRULE-SET,{url},DIRECT\nDOMAIN-SET,{url},REJECT\nFINAL,DIRECT\n"),
    );
    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(&conf)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("error[E0024] "))
        .stdout(predicate::str::contains(
            "kinds.conf:3: the same resource cannot be both a RULE-SET and a DOMAIN-SET",
        ))
        .stdout(predicate::str::contains("kinds.conf:2)"))
        .stdout(predicate::str::contains("1 error(s)"))
        .stdout(predicate::str::contains("W0032").not())
        .stdout(predicate::str::contains("S3CR3T").not())
        .stdout(predicate::str::contains("sets.example").not());
    // the same resource in one role twice is fine
    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(
            &dir,
            "once.conf",
            &format!("[Rule]\nRULE-SET,{url},DIRECT\nRULE-SET,{url},REJECT\nFINAL,DIRECT\n"),
        ))
        .assert()
        .success()
        .stdout(predicate::str::contains("E0024").not());
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

    /// A switch's environment variable takes the usual spellings of yes
    /// (and of no: `run::an_empty_group_goes_direct_unless_told_to_reject`);
    /// anything else is an error that names the switch.
    #[test]
    fn a_switchs_environment_variable_takes_the_usual_spellings() {
        let dir = workspace();
        let with = |value: &str| {
            let mut cmd = Command::cargo_bin("rurge").unwrap();
            cmd.arg("rule")
                .arg("match")
                .arg("-c")
                .arg(dir.path().join("t.conf"))
                .arg("--data-dir")
                .arg(dir.path().join("data"))
                .arg("listed.com")
                .env("RURGE_NO_NETWORK", value);
            cmd
        };
        for value in ["1", "true", "yes", "on", "TRUE", "y"] {
            with(value)
                .assert()
                .success()
                .stdout(predicate::str::contains("P"));
        }
        with("maybe")
            .assert()
            .failure()
            .stderr(predicate::str::contains("--no-network"));
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

    /// The rule that lets `both.example` through is written first; the
    /// flagged ones go before it, and a flagged IP rule still gets its turn
    /// where it is written.
    const PRE_CONF: &str = "\
[Proxy]
P = direct
[Rule]
DOMAIN,both.example,P
DOMAIN,both.example,REJECT,pre-matching
IP-CIDR,10.0.0.0/8,REJECT,pre-matching
FINAL,DIRECT
";

    #[test]
    fn a_pre_matching_rule_decides_before_the_written_order() {
        let dir = workspace();
        std::fs::write(dir.path().join("t.conf"), PRE_CONF).unwrap();
        let (v, code) = json(dir.path(), &["both.example"]);
        assert_eq!(code, 0);
        assert_eq!(v["reason"], "pre-matching");
        assert_eq!(v["policy"], "REJECT");
        assert_eq!(v["matched"]["index"], 1);
        rule_match(dir.path(), &["both.example", "--explain"])
            .assert()
            .success()
            .stdout(predicate::str::contains("reason: pre-matching"))
            .stdout(predicate::str::contains(
                "rule #1: DOMAIN,both.example,REJECT,pre-matching",
            ))
            // one step, and the rule written first is never evaluated
            .stdout(predicate::str::contains("#1 pre-match "))
            .stdout(predicate::str::contains("#0 ").not());
        // direct and global modes do not look at the flagged rules
        let (v, _) = json(dir.path(), &["both.example", "--mode", "direct"]);
        assert_eq!(v["reason"], "outbound-mode-direct");
    }

    #[test]
    fn a_pre_matching_ip_rule_that_misses_is_evaluated_once_in_its_place() {
        let dir = workspace();
        std::fs::write(dir.path().join("t.conf"), PRE_CONF).unwrap();
        let args = ["other.example", "--resolve", "10.1.2.3"];
        let (v, _) = json(dir.path(), &args);
        assert_eq!(v["reason"], "rule");
        assert_eq!(v["matched"]["index"], 2);
        assert_eq!(v["resolved"]["v4"][0], "10.1.2.3");
        let out = rule_match(dir.path(), &[args[0], args[1], args[2], "--explain"])
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            out.lines()
                .filter(|l| l.trim_start().starts_with("#2 "))
                .count(),
            1,
            "{out}"
        );
        assert!(!out.contains("pre-match "), "{out}");
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
        let out = rule_match(dir.path(), &["listed.com"])
            .assert()
            .success()
            .stderr(predicate::str::contains("W0001"))
            .get_output()
            .stderr
            .clone();
        // printed for a person: the hint is the line after its diagnostic
        let stderr = String::from_utf8_lossy(&out);
        let lines: Vec<&str> = stderr.lines().collect();
        let at = lines
            .iter()
            .position(|line| line.starts_with("warning[W0001]"))
            .unwrap_or_else(|| panic!("{stderr}"));
        assert_eq!(
            lines[at + 1],
            format!(
                "  hint: {}",
                rurge_config::codes::hint("W0001").expect("W0001 has a hint")
            ),
            "{stderr}"
        );
        let (v, code) = json(dir.path(), &["listed.com"]);
        assert_eq!(code, 0);
        let warnings = v["warnings"].as_array().expect("warnings array");
        let unknown_key = warnings
            .iter()
            .find(|w| w["code"] == "W0001")
            .unwrap_or_else(|| panic!("{warnings:?}"));
        // the hint is there as in `check --json`
        assert_eq!(
            unknown_key["hint"],
            rurge_config::codes::hint("W0001").expect("W0001 has a hint")
        );
    }

    /// MaxMind's own download link carries the account's licence key, so a
    /// link that cannot be read (here: its scheme is missing) is said to be
    /// ignored without being quoted.
    #[test]
    fn an_unreadable_geoip_link_is_ignored_without_being_quoted() {
        let dir = workspace();
        std::fs::write(
            dir.path().join("t.conf"),
            format!(
                "[General]\ngeoip-maxmind-url = download.example/geoip?license_key=S3CR3T-geoip\n{CONF}"
            ),
        )
        .unwrap();
        rule_match(dir.path(), &["listed.com"])
            .assert()
            .success()
            .stderr(predicate::str::contains("ignoring geoip-maxmind-url"))
            .stderr(predicate::str::contains("S3CR3T").not());
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

    /// The warnings of a profile are in `--json` the way `check --json` has
    /// them: with the hint of their code, and `null` where the code has none.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_json_warnings_of_a_lookup_carry_the_hint_of_their_code() {
        let mock = MockDns::spawn().await;
        let dir = workspace();
        std::fs::write(
            dir.path().join("t.conf"),
            CONF.replace(
                "ipv6 = false",
                "ipv6 = false\nsome-made-up-key = 1\ncompatibility-mode = true",
            ),
        )
        .unwrap();
        let out = output(dns_cmd(
            dir.path(),
            "lookup",
            &mock.addr().to_string(),
            &["fixed.test", "--json"],
        ))
        .await;
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v = json_of(&out);
        let warnings = v["warnings"].as_array().expect("warnings array");
        let warning = |code: &str| {
            warnings
                .iter()
                .find(|w| w["code"] == code)
                .unwrap_or_else(|| panic!("{code} is not in {warnings:?}"))
        };
        assert_eq!(
            warning("W0001")["hint"],
            rurge_config::codes::hint("W0001").expect("W0001 has a hint")
        );
        // `W0004` is a code without a hint: the key is there all the same
        assert_eq!(warning("W0004").get("hint"), Some(&serde_json::Value::Null));
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
    use predicates::prelude::*;
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

    /// The one way a CLI test starts `rurge run`. P5: no test may reach the
    /// real system proxy, so the file backend is wired in here instead of
    /// being remembered at every call site.
    fn rurge_run(conf: &Path, data: &Path) -> Command {
        let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("rurge"));
        cmd.arg("run")
            .arg("-c")
            .arg(conf)
            .arg("--no-network")
            .arg("--data-dir")
            .arg(data)
            .env(
                "RURGE_SYSTEM_PROXY_BACKEND",
                format!("file:{}", sysproxy_file(data).display()),
            )
            .env_remove("RURGE_SYSTEM_PROXY")
            .env_remove("RURGE_EMPTY_GROUP_REJECT")
            .env_remove("RURGE_LISTEN_HTTP")
            .env_remove("RURGE_LISTEN_SOCKS5");
        cmd
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
        let mut cmd = rurge_run(conf, data);
        if watch {
            cmd.arg("--watch");
        }
        if let Some(log) = log_file {
            cmd.arg("--log-file").arg(log);
        }
        cmd.args(extra);
        spawn_command(cmd)
    }

    /// Spawns a prepared `rurge_run` command and waits for both `listening
    /// on` lines.
    fn spawn_command(cmd: Command) -> Daemon {
        spawn_command_with(cmd, Stdio::inherit()).0
    }

    /// `spawn_command`, with the daemon's standard error read as well: the
    /// receiver has its lines.
    fn spawn_command_reading_stderr(cmd: Command) -> (Daemon, mpsc::Receiver<String>) {
        let (daemon, errors) = spawn_command_with(cmd, Stdio::piped());
        (daemon, errors.expect("standard error was piped"))
    }

    /// The lines `reader` yields, on a channel. A thread of its own reads
    /// them, so that a pipe nobody is reading from does not fill up and stop
    /// the daemon.
    fn forward_lines(reader: impl Read + Send + 'static) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        rx
    }

    /// What the daemon wrote to standard error, for the end of a panic message:
    /// all of it once the daemon has ended and closed it, what came in the
    /// first two seconds if it has not.
    fn standard_error_so_far(errors: Option<&mpsc::Receiver<String>>) -> String {
        let Some(errors) = errors else {
            return String::new();
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut text = String::new();
        while let Ok(line) =
            errors.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        {
            text.push('\n');
            text.push_str(&line);
        }
        if text.is_empty() {
            return "\nit wrote nothing to standard error".to_string();
        }
        format!("\nwhat it wrote to standard error:{text}")
    }

    fn spawn_command_with(
        mut cmd: Command,
        stderr: Stdio,
    ) -> (Daemon, Option<mpsc::Receiver<String>>) {
        let mut child = cmd.stdout(Stdio::piped()).stderr(stderr).spawn().unwrap();
        let lines = forward_lines(child.stdout.take().unwrap());
        let errors = child.stderr.take().map(forward_lines);
        // Own the child before anything can panic, so no path leaks the process.
        let mut daemon = Daemon {
            child,
            http: 0,
            socks: 0,
            lines,
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while daemon.http == 0 || daemon.socks == 0 {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let line = match daemon.lines.recv_timeout(remaining) {
                Ok(line) => line,
                Err(e) => panic!(
                    "rurge run did not print its listening lines: {e:?}{}",
                    standard_error_so_far(errors.as_ref())
                ),
            };
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
        (daemon, errors)
    }

    fn http_get(port: u16, url: &str) -> String {
        http_get_with(port, url, "")
    }

    /// `http_get` with more header lines, each ending in `\r\n`.
    fn http_get_with(port: u16, url: &str, headers: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let host = url.trim_start_matches("http://").split('/').next().unwrap();
        // one write, as in `api_call`
        let request =
            format!("GET {url} HTTP/1.1\r\nHost: {host}\r\n{headers}Connection: close\r\n\r\n");
        s.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    /// The `Proxy-Authorization` line for `user:password` (RFC 7617).
    fn basic_credentials(user_and_password: &str) -> String {
        const DIGITS: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::new();
        for chunk in user_and_password.as_bytes().chunks(3) {
            let bits = chunk.iter().enumerate().fold(0u32, |bits, (i, byte)| {
                bits | (u32::from(*byte) << (16 - 8 * i))
            });
            for i in 0..4 {
                encoded.push(if i <= chunk.len() {
                    DIGITS[((bits >> (18 - 6 * i)) & 63) as usize] as char
                } else {
                    '='
                });
            }
        }
        format!("Proxy-Authorization: Basic {encoded}\r\n")
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
    ///
    /// The request leaves in one write. Written in pieces (`write!` on a
    /// socket sends once per piece) it can arrive in several segments, the
    /// body after the head. An endpoint that does not read its body, `POST
    /// /v1/stop` among them, answers on the head alone and closes; a body that
    /// then reaches the closed connection is met with a reset, and on Windows
    /// the reset makes the client's system discard the answer it has not read
    /// yet: nothing is read and the status comes out as 0.
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
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Key: {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(request.as_bytes()).unwrap();
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

    /// Where the test backend keeps the "system proxy" of a daemon.
    fn sysproxy_file(data: &Path) -> std::path::PathBuf {
        data.join("sysproxy.json")
    }

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Polls until `check` passes or 10 s elapse.
    fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !check() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A daemon that ends before it says where it listens fails the test that
    /// started it with what the daemon wrote to standard error, not with a
    /// bare `Disconnected`.
    #[test]
    #[should_panic(expected = "error: cannot read")]
    fn a_daemon_that_cannot_start_fails_its_test_with_its_own_words() {
        let dir = tempfile::tempdir().unwrap();
        let [_, (unreadable, _)] = profiles_that_do_not_load(dir.path());
        spawn_command_reading_stderr(rurge_run(&unreadable, &dir.path().join("data")));
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

    /// A group without members goes DIRECT, as in Surge; the switch, and its
    /// environment variable, make it reject instead (M3-D3).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_empty_group_goes_direct_unless_told_to_reject() {
        let target = TestServer::spawn().await;
        target.set("/hello", "hi from target");
        let url = format!("http://127.0.0.1:{}/hello", target.url("/").port().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("t.conf");
        std::fs::write(
            &conf,
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning\n\
[Proxy Group]\nSub = select, policy-path=missing.txt\n[Rule]\nFINAL,Sub\n",
        )
        .unwrap();
        for (case, (switch, env, rejects)) in [
            (None, None, false),
            (Some("--empty-group-reject"), None, true),
            (None, Some("true"), true),
            (None, Some("1"), true),
            (None, Some("0"), false),
            (None, Some("off"), false),
        ]
        .into_iter()
        .enumerate()
        {
            let (conf, url) = (conf.clone(), url.clone());
            let data = dir.path().join(format!("data-{case}"));
            let answer = tokio::task::spawn_blocking(move || {
                let mut cmd = rurge_run(&conf, &data);
                cmd.args(switch);
                if let Some(value) = env {
                    cmd.env("RURGE_EMPTY_GROUP_REJECT", value);
                }
                let daemon = spawn_command(cmd);
                http_get(daemon.http, &url)
            })
            .await
            .unwrap();
            if rejects {
                assert!(answer.is_empty(), "REJECT closes the connection: {answer}");
            } else {
                assert!(
                    answer.starts_with("HTTP/1.1 200") && answer.ends_with("hi from target"),
                    "{answer}"
                );
            }
        }
        assert_eq!(target.requests().len(), 3);
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
            move || {
                let daemon = spawn_daemon(&conf, &data);
                // the handlers for the signals are in place once this line is printed
                wait_for_line(&daemon, "rurge ");
                daemon
            }
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
        // `rurge run` arms the watch after it says where it listens and
        // before it says it is running: a profile written in between is a
        // change that nothing ever sees.
        wait_for_line(&daemon, "rurge ");
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
        // What is waited for is the reload, after a quiet half second and a
        // load of the profile; a slow machine takes as long as it takes.
        let ok = tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
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

    /// SIGHUP makes the daemon load its profile again: with no `--watch` and
    /// no call to the API, a rule added to the file shows among the rules the
    /// daemon serves.
    #[cfg(unix)]
    #[test]
    fn sighup_reloads_the_profile() {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;

        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let daemon = spawn_daemon(&conf, &dir.path().join("data"));
        let port = api_port(&daemon);
        // The summary line comes after the daemon has taken SIGHUP over; one
        // that arrived earlier would end the process.
        wait_for_line(&daemon, "rurge ");
        let rules = || api_call(port, "GET", "/v1/rules", "k", None).1;
        assert!(!rules().contains("DOMAIN,added.test,REJECT"));
        std::fs::write(
            &conf,
            format!(
                "[General]\n{API_GENERAL}\n[Proxy]\n[Rule]\nDOMAIN,ads.test,REJECT\nDOMAIN,added.test,REJECT\nFINAL,DIRECT\n"
            ),
        )
        .unwrap();
        let pid = Pid::from_raw(i32::try_from(daemon.child.id()).expect("a process id"));
        kill(pid, Signal::SIGHUP).expect("SIGHUP reaches the daemon");
        wait_until(
            "the rule added to the profile among the rules served",
            || rules().contains("DOMAIN,added.test,REJECT"),
        );
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
        let data = dir.path().join("data");
        let status = rurge_run(&dir.path().join("t.conf"), &data)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(2));
    }

    #[test]
    fn run_refuses_a_policy_that_cannot_be_built() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("t.conf"), super::BROKEN_P12).unwrap();
        let output = rurge_run(&dir.path().join("t.conf"), &dir.path().join("data"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains("E0022"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
    }

    /// M1 design 6.4: a reload whose profile holds a policy that cannot be
    /// built keeps the running generation.
    #[test]
    fn a_reload_with_an_unbuildable_policy_keeps_the_current_config() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let daemon = spawn_daemon(&conf, &dir.path().join("data"));
        let port = api_port(&daemon);
        let good = std::fs::read_to_string(&conf).unwrap();
        let broken = good.replace(
            "[Proxy]\n",
            "[Proxy]\nUp = https, proxy.test, 443, client-cert=cert1\n[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n",
        );
        assert_ne!(good, broken);
        std::fs::write(&conf, broken).unwrap();
        let (status, body) = api_call(port, "POST", "/v1/profiles/reload", "k", Some("{}"));
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("\"ok\":false"), "{body}");
        assert!(!body.contains("hunter2"), "{body}");
        // the daemon is still up and takes the repaired profile
        std::fs::write(&conf, good).unwrap();
        let (_, body) = api_call(port, "POST", "/v1/profiles/reload", "k", Some("{}"));
        assert!(body.contains("\"ok\":true"), "{body}");
    }

    /// The whole way: `rurge run` → HTTP listener → a real `http` policy in
    /// forward mode → a scripted loopback upstream.
    #[tokio::test]
    async fn run_routes_through_an_http_upstream() {
        use rurge_proto::testing::{FakeHttpProxy, HttpProxyScript};
        let upstream = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("t.conf");
        std::fs::write(
            &conf,
            format!(
                "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning\n\
[Proxy]\nUp = http, 127.0.0.1, {}\n[Rule]\nDOMAIN,via.test,Up\nFINAL,DIRECT\n",
                upstream.addr().port()
            ),
        )
        .unwrap();
        let daemon = tokio::task::spawn_blocking({
            let (conf, data) = (conf.clone(), dir.path().join("data"));
            move || spawn_daemon(&conf, &data)
        })
        .await
        .unwrap();
        let port = daemon.http;
        let answer = tokio::task::spawn_blocking(move || http_get(port, "http://via.test/hello"))
            .await
            .unwrap();
        assert!(
            answer.starts_with("HTTP/1.1 200") && answer.ends_with("forwarded"),
            "{answer}"
        );
        assert_eq!(
            upstream.heads()[0].request_line,
            "GET http://via.test/hello HTTP/1.1"
        );
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
        let data = dir.path().join("data");
        let output = rurge_run(&conf, &data)
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
        // 3 s, not 10: `POST /v1/stop` cancels the API token and drains at
        // once, so a `stop` that fell back on the shutdown grace period would
        // take much longer — and would print the line asserted against below.
        assert_eq!(wait_for_exit(&mut daemon, 3), Some(0), "stop exits 0");
        let lines: Vec<String> = daemon.lines.try_iter().collect();
        assert!(
            !lines.iter().any(|l| l.contains("grace period elapsed")),
            "stop must not wait out the grace period: {lines:?}"
        );
    }

    /// rurge does not rewrite the profile, nor a file the profile includes: a
    /// reload, a change of a group's selection (which `state.json` keeps) and
    /// a stop leave both as they were. They are written the way nothing would
    /// write them back (CRLF, comments, spaces around a value, no newline at
    /// the end), so a rewrite could not leave them as they are. Their
    /// modification times stay too: writing the bytes that are there is still
    /// a write to the user's file, which can fail halfway and which `--watch`
    /// takes for a change.
    #[test]
    fn run_leaves_the_profile_and_the_files_it_includes_as_they_are() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("t.conf");
        let included = dir.path().join("rules.dconf");
        let profile = [
            "# a profile as it was written",
            "[General]",
            "http-listen =  127.0.0.1:0",
            "socks5-listen = 127.0.0.1:0 ",
            "http-api = k@127.0.0.1:0",
            "loglevel = warning",
            "",
            "[Proxy Group]",
            "Pick = select, DIRECT, REJECT",
            "",
            "[Rule]",
            "#!include rules.dconf",
            "FINAL,Pick",
        ]
        .join("\r\n");
        let rules = [
            "[Rule]",
            "// kept as written",
            "DOMAIN,ads.test,REJECT  ",
            "",
            "DOMAIN-SUFFIX,tracker.test,REJECT",
        ]
        .join("\r\n");
        std::fs::write(&conf, &profile).unwrap();
        std::fs::write(&included, &rules).unwrap();
        let modified = |path: &Path| std::fs::metadata(path).unwrap().modified().unwrap();
        let (conf_modified, included_modified) = (modified(&conf), modified(&included));
        let as_written = |when: &str| {
            assert_eq!(
                std::fs::read_to_string(&conf).unwrap(),
                profile,
                "the profile was rewritten {when}"
            );
            assert_eq!(
                std::fs::read_to_string(&included).unwrap(),
                rules,
                "the included file was rewritten {when}"
            );
            assert_eq!(
                modified(&conf),
                conf_modified,
                "the profile was written to {when}"
            );
            assert_eq!(
                modified(&included),
                included_modified,
                "the included file was written to {when}"
            );
        };
        let data = dir.path().join("data");
        let mut daemon = spawn_daemon(&conf, &data);
        let port = api_port(&daemon);
        // what the daemon runs on has the included file in it
        let (_, served) = api_call(port, "GET", "/v1/rules", "k", None);
        assert!(
            served.contains("DOMAIN-SUFFIX,tracker.test,REJECT"),
            "{served}"
        );
        as_written("by the start");
        let (status, body) = api_call(port, "POST", "/v1/profiles/reload", "k", Some("{}"));
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("\"ok\":true"), "{body}");
        as_written("by the reload");
        let (status, body) = api_call(
            port,
            "POST",
            "/v1/policy_groups/select",
            "k",
            Some(r#"{"group_name":"Pick","policy":"REJECT"}"#),
        );
        assert_eq!(status, 200, "{body}");
        wait_until("state.json to hold the selection", || {
            std::fs::read_to_string(data.join("state.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .is_some_and(|state| state["group_selections"]["t.conf"]["Pick"] == "REJECT")
        });
        as_written("by the selection");
        assert_eq!(
            api_call(port, "POST", "/v1/stop", "k", Some("{}")).0,
            200,
            "stop"
        );
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(0), "stop exits 0");
        as_written("by the stop");
    }

    /// `rurge run` starts an `external` policy's program on first use and
    /// stops it on the way out (phase 2 M4 design 7.3, 8.2). The program is
    /// a second rurge: a SOCKS5 proxy on the policy's `local-port`. The outer
    /// rurge is started with a setting of its own in the environment, which
    /// the program does not get.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_starts_an_external_program_and_stops_it_on_exit() {
        let target = TestServer::spawn().await;
        target.set("/hello", "hi from target");
        let port = target.url("/").port().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let inner_port = free_tcp_port();
        let inner = dir.path().join("inner.conf");
        std::fs::write(
            &inner,
            format!("[General]\nsocks5-listen = 127.0.0.1:{inner_port}\nloglevel = warning\n[Rule]\nFINAL,DIRECT\n"),
        )
        .unwrap();
        let quoted = |p: &Path| format!("\"{}\"", p.to_string_lossy().replace('\\', "\\\\"));
        let conf = dir.path().join("t.conf");
        std::fs::write(
            &conf,
            format!(
                "[General]\n{API_GENERAL}\n[Proxy]\n\
Inner = external, exec = {}, args = run, args = -c, args = {}, args = --no-network, args = --data-dir, args = {}, local-port = {inner_port}\n\
[Rule]\nFINAL,Inner\n",
                quoted(&assert_cmd::cargo::cargo_bin("rurge")),
                quoted(&inner),
                quoted(&dir.path().join("inner-data")),
            ),
        )
        .unwrap();
        let data = dir.path().join("data");
        let mut daemon = tokio::task::spawn_blocking({
            let (conf, data) = (conf.clone(), data.clone());
            move || {
                let mut cmd = rurge_run(&conf, &data);
                // The program is a second rurge: handed this setting of the
                // first, it would listen where the variable says and not on
                // the `local-port` its profile names.
                cmd.env("RURGE_LISTEN_SOCKS5", "127.0.0.1:0");
                spawn_command(cmd)
            }
        })
        .await
        .unwrap();
        let api = api_port(&daemon);
        let http_port = daemon.http;
        let ok = tokio::task::spawn_blocking(move || {
            http_get(http_port, &format!("http://127.0.0.1:{port}/hello"))
        })
        .await
        .unwrap();
        assert!(
            ok.starts_with("HTTP/1.1 200") && ok.ends_with("hi from target"),
            "{ok}"
        );
        let log = std::fs::read_to_string(data.join("external").join("Inner.log")).unwrap();
        assert!(
            log.contains(&format!("listening on socks5://127.0.0.1:{inner_port}")),
            "{log}"
        );
        assert_eq!(api_call(api, "POST", "/v1/stop", "k", Some("{}")).0, 200);
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(0), "stop exits 0");
        // the program is gone with rurge: its port no longer answers
        wait_until("the inner rurge to end", || {
            TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], inner_port)),
                Duration::from_millis(500),
            )
            .is_err()
        });
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

    fn rurge() -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::cargo_bin("rurge").unwrap();
        cmd.env_remove("RURGE_API_KEY");
        cmd
    }

    #[test]
    fn control_commands_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let mut daemon = spawn_daemon(&conf, &dir.path().join("data"));
        let port = api_port(&daemon);
        let remote = format!("127.0.0.1:{port}");
        // status via flags
        let out = rurge()
            .args(["status", "--remote", &remote, "--key", "k"])
            .assert()
            .success();
        let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
        assert!(
            text.starts_with(&format!(
                "rurge at http://{remote}\nmode: rule (global policy: none)\n"
            )),
            "{text}"
        );
        assert!(
            text.contains("policies: 5   rules: 2   active requests: 0"),
            "{text}"
        );
        assert!(
            text.contains("traffic: in 0 B, out 0 B (in 0 B/s, out 0 B/s)"),
            "{text}"
        );
        // --json and the env var
        let out = rurge()
            .args(["status", "--remote", &remote, "--json"])
            .env("RURGE_API_KEY", "k")
            .assert()
            .success();
        let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
        assert_eq!(v["outbound"]["mode"], "rule");
        assert_eq!(v["policies"]["policy-groups"], serde_json::json!([]));
        // wrong key → 2, missing --key → 2, not configured → 2
        rurge()
            .args(["status", "--remote", &remote, "--key", "nope"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unauthorized"));
        rurge()
            .args(["reload", "--remote", &remote])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("--key"));
        let plain_dir = dir.path().join("plain");
        std::fs::create_dir_all(&plain_dir).unwrap();
        let plain = write_conf(&plain_dir, "http-listen = 127.0.0.1:0");
        rurge()
            .args(["reload", "-c"])
            .arg(&plain)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("http-api is not configured"));
        // resolution through a profile that names the live port
        let pointing = dir.path().join("pointing.conf");
        std::fs::write(
            &pointing,
            format!("[General]\nhttp-api = k@0.0.0.0:{port}\n[Rule]\nFINAL,DIRECT\n"),
        )
        .unwrap();
        rurge()
            .args(["reload", "-c"])
            .arg(&pointing)
            .assert()
            .success()
            .stdout(predicate::str::starts_with("reloaded: 0 error(s), "));
        // stop: the daemon exits 0
        rurge()
            .args(["stop", "--remote", &remote, "--key", "k"])
            .assert()
            .success()
            .stdout("stop requested\n");
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(0));
        // gone → 1
        rurge()
            .args(["status", "--remote", &remote, "--key", "k"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("cannot reach rurge"));
    }

    const ORIGINAL_PROXY: &str = "the user's own proxy settings";

    #[test]
    fn run_system_proxy_is_applied_switched_and_restored() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let file = sysproxy_file(&data);
        std::fs::write(&file, ORIGINAL_PROXY).unwrap();
        let mut daemon = spawn_daemon_full(&conf, &data, false, None, &["--system-proxy"]);
        let port = api_port(&daemon);
        let enabled = wait_for_line(&daemon, "system proxy enabled: ");
        assert_eq!(
            enabled,
            format!(
                "http 127.0.0.1:{}, socks 127.0.0.1:{}",
                daemon.http, daemon.socks
            )
        );
        let applied = read_json(&file);
        assert_eq!(applied["http"], format!("127.0.0.1:{}", daemon.http));
        assert_eq!(applied["https"], applied["http"]);
        assert_eq!(applied["socks"], format!("127.0.0.1:{}", daemon.socks));
        let state = read_json(&data.join("state.json"));
        assert_eq!(state["system_proxy_backup"]["previous"], ORIGINAL_PROXY);
        assert_eq!(state["features"]["system_proxy"], true);
        assert_eq!(
            api_call(port, "GET", "/v1/features/system_proxy", "k", None).1,
            r#"{"enabled":true}"#
        );
        // off and on again through the API
        assert_eq!(
            api_call(
                port,
                "POST",
                "/v1/features/system_proxy",
                "k",
                Some(r#"{"enabled":false}"#)
            ),
            (200, "{}".to_string())
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ORIGINAL_PROXY);
        assert!(read_json(&data.join("state.json"))["system_proxy_backup"].is_null());
        assert_eq!(
            api_call(port, "GET", "/v1/features/system_proxy", "k", None).1,
            r#"{"enabled":false}"#
        );
        assert_eq!(
            api_call(
                port,
                "POST",
                "/v1/features/system_proxy",
                "k",
                Some(r#"{"enabled":true}"#)
            )
            .0,
            200
        );
        assert_eq!(
            read_json(&file)["http"],
            format!("127.0.0.1:{}", daemon.http)
        );
        // a graceful stop puts the original back
        assert_eq!(api_call(port, "POST", "/v1/stop", "k", Some("{}")).0, 200);
        wait_for_line(&daemon, "system proxy restored");
        assert_eq!(wait_for_exit(&mut daemon, 5), Some(0));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ORIGINAL_PROXY);
        let state = read_json(&data.join("state.json"));
        assert!(state["system_proxy_backup"].is_null());
        assert_eq!(state["features"]["system_proxy"], false);
    }

    #[test]
    fn run_restores_the_system_proxy_a_crashed_run_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let file = sysproxy_file(&data);
        std::fs::write(&file, ORIGINAL_PROXY).unwrap();
        let crashed = spawn_daemon_full(&conf, &data, false, None, &["--system-proxy"]);
        wait_for_line(&crashed, "system proxy enabled: ");
        drop(crashed); // `Daemon::drop` kills the process: no cleanup runs
        assert_ne!(
            std::fs::read_to_string(&file).unwrap(),
            ORIGINAL_PROXY,
            "still pointing at the dead daemon"
        );
        assert!(!read_json(&data.join("state.json"))["system_proxy_backup"].is_null());
        // the next start, without --system-proxy, cleans up
        let next = spawn_daemon(&conf, &data);
        wait_for_line(&next, "rurge ");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ORIGINAL_PROXY);
        let state = read_json(&data.join("state.json"));
        assert!(state["system_proxy_backup"].is_null());
        assert_eq!(state["features"]["system_proxy"], false);
    }

    /// The port a crashed run held may well be why the restart cannot bind:
    /// recovery must not wait behind anything that can exit.
    #[test]
    fn run_restores_a_crashed_system_proxy_even_when_it_cannot_start() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let file = sysproxy_file(&data);
        std::fs::write(&file, ORIGINAL_PROXY).unwrap();
        let crashed = spawn_daemon_full(&conf, &data, false, None, &["--system-proxy"]);
        wait_for_line(&crashed, "system proxy enabled: ");
        drop(crashed); // `Daemon::drop` kills the process: no cleanup runs
        assert!(!read_json(&data.join("state.json"))["system_proxy_backup"].is_null());
        // the restart cannot get its listener: something else holds the port
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let conf = write_conf(
            dir.path(),
            &format!("http-listen = 127.0.0.1:{port}\nsocks5-listen = 127.0.0.1:0"),
        );
        let mut child = rurge_run(&conf, &data)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Ok(Some(status)) = child.try_wait() {
                break status;
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("rurge run did not exit within 20 s of failing to bind");
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert_eq!(status.code(), Some(1));
        assert!(stderr.contains("cannot bind listener"), "{stderr}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            ORIGINAL_PROXY,
            "the crashed run's settings are undone even though this run cannot start"
        );
        assert!(read_json(&data.join("state.json"))["system_proxy_backup"].is_null());
        drop(taken);
    }

    /// The other side of recovery: a LIVE first instance is not a crashed one.
    /// Without the data-directory lock the second start restored the first
    /// one's backup and cleared it, so the first run went on reporting the
    /// system proxy as enabled while the operating system no longer pointed at
    /// it.
    #[test]
    fn run_refuses_a_second_instance_on_the_same_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let file = sysproxy_file(&data);
        std::fs::write(&file, ORIGINAL_PROXY).unwrap();
        let mut first = spawn_daemon_full(&conf, &data, false, None, &["--system-proxy"]);
        let port = api_port(&first);
        wait_for_line(&first, "system proxy enabled: ");
        let applied = std::fs::read_to_string(&file).unwrap();
        assert_ne!(applied, ORIGINAL_PROXY, "the first run owns the settings");
        // a second start on the same data directory, while the first one runs
        let mut second = rurge_run(&conf, &data)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Ok(Some(status)) = second.try_wait() {
                break status;
            }
            if std::time::Instant::now() > deadline {
                let _ = second.kill();
                let _ = second.wait();
                panic!("the second rurge run did not exit within 20 s");
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let mut stderr = String::new();
        second
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert_eq!(status.code(), Some(1));
        assert!(stderr.contains("another rurge instance"), "{stderr}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            applied,
            "the second instance left the first one's system proxy alone"
        );
        assert!(
            !read_json(&data.join("state.json"))["system_proxy_backup"].is_null(),
            "the first run's backup is still there to restore"
        );
        // the first instance still owns the settings and puts them back
        assert_eq!(api_call(port, "POST", "/v1/stop", "k", Some("{}")).0, 200);
        wait_for_line(&first, "system proxy restored");
        assert_eq!(wait_for_exit(&mut first, 5), Some(0));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ORIGINAL_PROXY);
    }

    #[test]
    fn run_reapplies_the_system_proxy_after_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        let daemon = spawn_daemon_full(&conf, &data, false, None, &["--system-proxy"]);
        let port = api_port(&daemon);
        wait_for_line(&daemon, "system proxy enabled: ");
        let file = sysproxy_file(&data);
        assert_eq!(read_json(&file)["bypass"], serde_json::json!([]));
        write_conf(
            dir.path(),
            &format!("{API_GENERAL}\nskip-proxy = localhost, example.internal"),
        );
        let (status, body) = api_call(port, "POST", "/v1/profiles/reload", "k", Some("{}"));
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("\"ok\":true"), "{body}");
        // tolerant read: the daemon may be rewriting the file at this instant
        wait_until("the bypass list to follow the reload", || {
            std::fs::read_to_string(&file)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .is_some_and(|v| {
                    v["bypass"] == serde_json::json!(["localhost", "example.internal"])
                })
        });
        assert_eq!(
            read_json(&file)["http"],
            format!("127.0.0.1:{}", daemon.http)
        );
    }

    #[test]
    fn run_exits_1_when_the_system_proxy_cannot_be_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        // The backend's parent directory does not exist: `snapshot` (nothing
        // to read yet) and the rollback `restore` (nothing to remove) both
        // succeed, but `apply` fails trying to create the file.
        let backend = dir.path().join("no-such-dir").join("sysproxy.json");
        let mut cmd = rurge_run(&conf, &data);
        // the guard's own backend path, replaced by one that cannot be written
        cmd.arg("--system-proxy")
            .env(
                "RURGE_SYSTEM_PROXY_BACKEND",
                format!("file:{}", backend.display()),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Ok(Some(status)) = child.try_wait() {
                break status;
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("rurge run did not exit within 20 s of a system-proxy apply failure");
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let mut stdout = String::new();
        let mut stderr = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut stdout)
            .unwrap();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert_eq!(status.code(), Some(1));
        assert!(
            stderr.contains("cannot enable the system proxy"),
            "{stderr}"
        );
        assert!(!stdout.contains("rurge "), "{stdout}");
        let state = read_json(&data.join("state.json"));
        assert!(state["system_proxy_backup"].is_null());
        assert_eq!(state["features"]["system_proxy"], false);
    }

    const RESTORED: &str = "warning: a previous run left the system proxy pointing at rurge; the saved settings were restored\n";

    /// The two ways `rurge run` gives up before it has a profile — the text
    /// has an error, the file cannot be read — each with what its exit says
    /// on standard error. Both exit 2.
    fn profiles_that_do_not_load(dir: &Path) -> [(std::path::PathBuf, &'static str); 2] {
        let broken = dir.join("broken.conf");
        std::fs::write(
            &broken,
            "[General]\n[Proxy]\n[Rule]\nDOMAIN,a.test,DIRECT\n",
        )
        .unwrap(); // no FINAL
        [
            (broken, "error[E0010]"),
            (dir.join("no-such.conf"), "error: cannot read"),
        ]
    }

    /// Runs a `rurge run` that is expected to give up at once: its exit code
    /// and standard error. A start that does not end in time says which start
    /// it was and what it had printed, and whether waiting for it failed.
    fn give_up(mut cmd: Command) -> (Option<i32>, String) {
        let start = describe_start(&cmd);
        let mut child = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut wait_error = None;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => wait_error = Some(e),
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                let mut printed = String::new();
                let read = child.stderr.take().unwrap().read_to_string(&mut printed);
                panic!(
                    "rurge run did not give up within 20 s\n  the start: {start}\n  what it wrote to standard error: {printed:?}{}{}",
                    read.err()
                        .map(|e| format!(" (reading it failed: {e})"))
                        .unwrap_or_default(),
                    wait_error
                        .map(|e| format!(
                            "\n  waiting for it failed at least once, the last time with: {e}"
                        ))
                        .unwrap_or_default(),
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        (status.code(), stderr)
    }

    /// A start as it was made: the environment it sets, the program and its
    /// arguments.
    fn describe_start(cmd: &Command) -> String {
        let environment: String = cmd
            .get_envs()
            .filter_map(|(name, value)| {
                value.map(|value| format!("{}={value:?} ", name.to_string_lossy()))
            })
            .collect();
        let arguments: String = cmd.get_args().map(|arg| format!(" {arg:?}")).collect();
        format!("{environment}{:?}{arguments}", cmd.get_program())
    }

    /// A data directory as a crashed `rurge run --system-proxy` leaves it: the
    /// "system proxy" points at the dead daemon and `state.json` holds what
    /// was there before.
    fn crashed_with_the_system_proxy_on(dir: &Path) -> std::path::PathBuf {
        let conf = write_conf(dir, API_GENERAL);
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let file = sysproxy_file(&data);
        std::fs::write(&file, ORIGINAL_PROXY).unwrap();
        let crashed = spawn_daemon_full(&conf, &data, false, None, &["--system-proxy"]);
        wait_for_line(&crashed, "system proxy enabled: ");
        // once it is up: a start writes `state.json` once more on its way to
        // saying so, and a kill in the middle of that write leaves a
        // half-written `state.json.tmp` behind
        wait_for_line(&crashed, "rurge ");
        drop(crashed); // `Daemon::drop` kills the process: no cleanup runs
        assert_ne!(
            std::fs::read_to_string(&file).unwrap(),
            ORIGINAL_PROXY,
            "still pointing at the dead daemon"
        );
        assert!(!read_json(&data.join("state.json"))["system_proxy_backup"].is_null());
        data
    }

    fn assert_restored(data: &Path, stderr: &str) {
        assert_eq!(stderr.matches(RESTORED).count(), 1, "{stderr}");
        assert_eq!(
            std::fs::read_to_string(sysproxy_file(data)).unwrap(),
            ORIGINAL_PROXY
        );
        let state = read_json(&data.join("state.json"));
        assert!(state["system_proxy_backup"].is_null(), "{state}");
        assert_eq!(state["features"]["system_proxy"], false);
    }

    /// A crashed run left the operating system pointing at a dead port, and
    /// by the next start the profile is broken: the load error ends the start
    /// as it always did, but the saved settings go back first. Otherwise they
    /// would wait for a profile that loads, with the machine offline until
    /// then.
    #[test]
    fn run_restores_a_crashed_system_proxy_when_the_profile_has_errors() {
        let dir = tempfile::tempdir().unwrap();
        let data = crashed_with_the_system_proxy_on(dir.path());
        let [(broken, says), _] = profiles_that_do_not_load(dir.path());
        let (code, stderr) = give_up(rurge_run(&broken, &data));
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains(says), "{stderr}");
        assert_restored(&data, &stderr);
        // once: the backup is gone, so the next attempt has nothing to say
        let (code, stderr) = give_up(rurge_run(&broken, &data));
        assert_eq!(code, Some(2), "{stderr}");
        assert!(!stderr.contains("system proxy"), "{stderr}");
        assert_eq!(
            std::fs::read_to_string(sysproxy_file(&data)).unwrap(),
            ORIGINAL_PROXY
        );
    }

    #[test]
    fn run_restores_a_crashed_system_proxy_when_the_profile_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let data = crashed_with_the_system_proxy_on(dir.path());
        let [_, (missing, says)] = profiles_that_do_not_load(dir.path());
        let (code, stderr) = give_up(rurge_run(&missing, &data));
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains(says), "{stderr}");
        assert_restored(&data, &stderr);
    }

    /// A directory rurge keeps no state in gains nothing: not even the lock
    /// file a start would create.
    #[test]
    fn a_profile_that_does_not_load_adds_nothing_to_a_data_dir_without_state() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        for (profile, says) in profiles_that_do_not_load(dir.path()) {
            let (code, stderr) = give_up(rurge_run(&profile, &data));
            assert_eq!(code, Some(2), "{stderr}");
            assert!(stderr.contains(says), "{stderr}");
            assert!(!stderr.contains("system proxy"), "{stderr}");
            let left: Vec<_> = std::fs::read_dir(&data)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert!(left.is_empty(), "{left:?}");
        }
    }

    /// What `rurge run` prints of a profile that does not load is what
    /// `rurge check` prints: the hint is the line after its error.
    #[test]
    fn a_profile_that_does_not_load_is_printed_with_its_hint() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let [(broken, _), _] = profiles_that_do_not_load(dir.path());
        let (code, stderr) = give_up(rurge_run(&broken, &data));
        assert_eq!(code, Some(2), "{stderr}");
        assert!(
            stderr.contains(
                "error[E0010]: the [Rule] section must end with an enabled FINAL rule\n  hint: add `FINAL,DIRECT` as the last rule\n"
            ),
            "{stderr}"
        );
    }

    #[test]
    fn a_profile_that_does_not_load_does_not_create_the_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        for (profile, says) in profiles_that_do_not_load(dir.path()) {
            let (code, stderr) = give_up(rurge_run(&profile, &data));
            assert_eq!(code, Some(2), "{stderr}");
            assert!(stderr.contains(says), "{stderr}");
            assert!(!stderr.contains("system proxy"), "{stderr}");
            assert!(!data.exists());
        }
    }

    /// A backup under the lock of a live instance is that instance's own:
    /// giving up on a profile beside it restores nothing and says nothing.
    #[test]
    fn a_profile_that_does_not_load_leaves_a_live_instances_system_proxy_alone() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let file = sysproxy_file(&data);
        std::fs::write(&file, ORIGINAL_PROXY).unwrap();
        let mut first = spawn_daemon_full(&conf, &data, false, None, &["--system-proxy"]);
        let port = api_port(&first);
        wait_for_line(&first, "system proxy enabled: ");
        let applied = std::fs::read_to_string(&file).unwrap();
        assert_ne!(applied, ORIGINAL_PROXY, "the first run owns the settings");
        // up: a start writes `state.json` once more on its way to saying so
        wait_for_line(&first, "rurge ");
        let state = std::fs::read_to_string(data.join("state.json")).unwrap();
        assert!(!read_json(&data.join("state.json"))["system_proxy_backup"].is_null());
        for (profile, says) in profiles_that_do_not_load(dir.path()) {
            let (code, stderr) = give_up(rurge_run(&profile, &data));
            assert_eq!(code, Some(2), "{stderr}");
            assert!(stderr.contains(says), "{stderr}");
            assert!(!stderr.contains("system proxy"), "{stderr}");
            assert_eq!(
                std::fs::read_to_string(&file).unwrap(),
                applied,
                "the first instance's system proxy is left alone"
            );
            assert_eq!(
                std::fs::read_to_string(data.join("state.json")).unwrap(),
                state,
                "the first run's backup is still there to restore"
            );
        }
        // the first instance still owns the settings and puts them back
        assert_eq!(api_call(port, "POST", "/v1/stop", "k", Some("{}")).0, 200);
        wait_for_line(&first, "system proxy restored");
        assert_eq!(wait_for_exit(&mut first, 5), Some(0));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ORIGINAL_PROXY);
    }

    /// A `RURGE_SYSTEM_PROXY_BACKEND` rurge does not know never becomes the
    /// operating system's backend: nothing is restored, the backup stays, and
    /// the exit code is still the profile's.
    #[test]
    fn run_says_so_when_an_unknown_backend_keeps_it_from_restoring() {
        let dir = tempfile::tempdir().unwrap();
        let data = crashed_with_the_system_proxy_on(dir.path());
        let applied = std::fs::read_to_string(sysproxy_file(&data)).unwrap();
        let state = std::fs::read_to_string(data.join("state.json")).unwrap();
        let [(broken, says), _] = profiles_that_do_not_load(dir.path());
        let mut cmd = rurge_run(&broken, &data);
        // the guard's own backend, replaced by a value that names none
        cmd.env("RURGE_SYSTEM_PROXY_BACKEND", "bogus");
        let (code, stderr) = give_up(cmd);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains(says), "{stderr}");
        assert!(
            stderr.contains(
                "warning: unknown RURGE_SYSTEM_PROXY_BACKEND value `bogus` (expected file:<path>); the system proxy was not restored\n"
            ),
            "{stderr}"
        );
        assert_eq!(
            std::fs::read_to_string(sysproxy_file(&data)).unwrap(),
            applied
        );
        assert_eq!(
            std::fs::read_to_string(data.join("state.json")).unwrap(),
            state
        );
        // the backup stayed, so an attempt with a backend that works restores
        let (code, stderr) = give_up(rurge_run(&broken, &data));
        assert_eq!(code, Some(2), "{stderr}");
        assert_restored(&data, &stderr);
    }

    /// A restore that fails is reported and keeps the backup for the next
    /// start; the exit code is still the profile's.
    #[test]
    fn run_keeps_the_backup_when_it_cannot_restore_it() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        // not taken by the file backend, whose `restore` therefore refuses it
        let state = r#"{"version":1,"features":{"system_proxy":true},"system_proxy_backup":{"platform":"elsewhere"}}"#;
        std::fs::write(data.join("state.json"), state).unwrap();
        for (profile, says) in profiles_that_do_not_load(dir.path()) {
            let (code, stderr) = give_up(rurge_run(&profile, &data));
            assert_eq!(code, Some(2), "{stderr}");
            assert!(stderr.contains(says), "{stderr}");
            assert!(
                stderr.contains(
                    "warning: cannot restore the system proxy: the system proxy backup was not taken by the file backend; the saved settings stay in state.json and are restored on the next start\n"
                ),
                "{stderr}"
            );
            assert_eq!(
                std::fs::read_to_string(data.join("state.json")).unwrap(),
                state
            );
            assert!(!sysproxy_file(&data).exists());
        }
    }

    /// `state.json` without a backup: nothing to put back and nothing to say.
    /// The backend is not even looked at, so a value that names none goes
    /// unreported here.
    #[test]
    fn a_profile_that_does_not_load_says_nothing_without_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let state = r#"{"version":1,"outbound_mode":"direct","system_proxy_backup":null}"#;
        std::fs::write(data.join("state.json"), state).unwrap();
        for (profile, says) in profiles_that_do_not_load(dir.path()) {
            for backend in [None, Some("bogus")] {
                let mut cmd = rurge_run(&profile, &data);
                if let Some(backend) = backend {
                    cmd.env("RURGE_SYSTEM_PROXY_BACKEND", backend);
                }
                let (code, stderr) = give_up(cmd);
                assert_eq!(code, Some(2), "{stderr}");
                assert!(stderr.contains(says), "{stderr}");
                assert!(!stderr.contains("system proxy"), "{stderr}");
                assert_eq!(
                    std::fs::read_to_string(data.join("state.json")).unwrap(),
                    state
                );
                assert!(!sysproxy_file(&data).exists());
            }
        }
    }

    /// A `state.json` that does not parse holds no backup rurge could use. A
    /// start moves such a file aside and says so in its log; a start that
    /// ends at the profile has no log yet, and leaves the file to one that
    /// has.
    #[test]
    fn a_profile_that_does_not_load_leaves_a_state_file_it_cannot_parse_alone() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("state.json"), "{ not json").unwrap();
        for (profile, says) in profiles_that_do_not_load(dir.path()) {
            let (code, stderr) = give_up(rurge_run(&profile, &data));
            assert_eq!(code, Some(2), "{stderr}");
            assert!(stderr.contains(says), "{stderr}");
            assert!(!stderr.contains("system proxy"), "{stderr}");
            assert!(
                !data.join("state.json.broken").exists(),
                "the file was moved aside"
            );
            assert_eq!(
                std::fs::read_to_string(data.join("state.json")).unwrap(),
                "{ not json"
            );
        }
    }

    /// A data directory that cannot be locked: nothing shows that the backup
    /// is a dead run's and not a live instance's. A start runs there without
    /// the lock rather than not at all; a start that is about to end anyway
    /// leaves the backup to one that gets further.
    #[test]
    fn a_profile_that_does_not_load_restores_nothing_in_a_data_dir_it_cannot_lock() {
        let dir = tempfile::tempdir().unwrap();
        let data = crashed_with_the_system_proxy_on(dir.path());
        let file = sysproxy_file(&data);
        let applied = std::fs::read_to_string(&file).unwrap();
        let state = std::fs::read_to_string(data.join("state.json")).unwrap();
        // a directory where the lock file belongs: it cannot be opened as one
        let lock = data.join("rurge.lock");
        std::fs::remove_file(&lock).unwrap();
        std::fs::create_dir(&lock).unwrap();
        for (profile, says) in profiles_that_do_not_load(dir.path()) {
            let (code, stderr) = give_up(rurge_run(&profile, &data));
            assert_eq!(code, Some(2), "{stderr}");
            assert!(stderr.contains(says), "{stderr}");
            assert!(!stderr.contains("system proxy"), "{stderr}");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), applied);
            assert_eq!(
                std::fs::read_to_string(data.join("state.json")).unwrap(),
                state
            );
        }
        // left, not lost: with a lock to take, the next attempt restores
        std::fs::remove_dir(&lock).unwrap();
        let [(broken, _), _] = profiles_that_do_not_load(dir.path());
        let (code, stderr) = give_up(rurge_run(&broken, &data));
        assert_eq!(code, Some(2), "{stderr}");
        assert_restored(&data, &stderr);
    }

    /// The settings went back, but `state.json` could not be rewritten: the
    /// backup is still in it, and the next start restores it once more, over
    /// whatever has been set since. That is what is said, not "restored".
    #[test]
    fn run_says_so_when_it_restored_but_cannot_update_the_state() {
        const NOT_UPDATED: &str = "warning: a previous run left the system proxy pointing at rurge; the saved settings were restored, but state.json could not be updated: the next start restores them again\n";
        let dir = tempfile::tempdir().unwrap();
        let data = crashed_with_the_system_proxy_on(dir.path());
        let file = sysproxy_file(&data);
        let state = std::fs::read_to_string(data.join("state.json")).unwrap();
        // the store writes `state.json.tmp` and renames it: a directory of
        // that name is in the way of every rewrite
        let in_the_way = data.join("state.json.tmp");
        std::fs::create_dir(&in_the_way).unwrap();
        for (profile, says) in profiles_that_do_not_load(dir.path()) {
            let (code, stderr) = give_up(rurge_run(&profile, &data));
            assert_eq!(code, Some(2), "{stderr}");
            assert!(stderr.contains(says), "{stderr}");
            assert_eq!(stderr.matches(NOT_UPDATED).count(), 1, "{stderr}");
            assert!(!stderr.contains(RESTORED), "{stderr}");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), ORIGINAL_PROXY);
            assert_eq!(
                std::fs::read_to_string(data.join("state.json")).unwrap(),
                state
            );
            // what the next start, true to its word, puts the backup over
            std::fs::write(&file, "set since").unwrap();
        }
        std::fs::remove_dir(&in_the_way).unwrap();
        let [(broken, _), _] = profiles_that_do_not_load(dir.path());
        let (code, stderr) = give_up(rurge_run(&broken, &data));
        assert_eq!(code, Some(2), "{stderr}");
        assert_restored(&data, &stderr);
    }

    /// A log file `rurge run` cannot open: where its directory would be there
    /// is a regular file.
    fn log_file_that_cannot_be_opened(dir: &Path) -> std::path::PathBuf {
        let in_the_way = dir.join("logs");
        std::fs::write(&in_the_way, "").unwrap();
        in_the_way.join("rurge.log")
    }

    /// The profile loads, but the log file cannot be opened: this start, too,
    /// ends before its own recovery, and for a reason that goes well with a
    /// crash (a full disk, a log directory on a mount that is gone).
    #[test]
    fn run_restores_a_crashed_system_proxy_when_the_log_file_cannot_be_opened() {
        let dir = tempfile::tempdir().unwrap();
        let data = crashed_with_the_system_proxy_on(dir.path());
        // the profile of the crashed run: it loads
        let mut cmd = rurge_run(&dir.path().join("t.conf"), &data);
        cmd.arg("--log-file")
            .arg(log_file_that_cannot_be_opened(dir.path()));
        let (code, stderr) = give_up(cmd);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("error: cannot open log file"), "{stderr}");
        assert_restored(&data, &stderr);
    }

    #[test]
    fn a_log_file_that_cannot_be_opened_says_nothing_about_the_system_proxy_without_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let state = r#"{"version":1,"outbound_mode":"direct","system_proxy_backup":null}"#;
        std::fs::write(data.join("state.json"), state).unwrap();
        let log = log_file_that_cannot_be_opened(dir.path());
        for backend in [None, Some("bogus")] {
            let mut cmd = rurge_run(&conf, &data);
            cmd.arg("--log-file").arg(&log);
            if let Some(backend) = backend {
                cmd.env("RURGE_SYSTEM_PROXY_BACKEND", backend);
            }
            let (code, stderr) = give_up(cmd);
            assert_eq!(code, Some(2), "{stderr}");
            assert!(stderr.contains("error: cannot open log file"), "{stderr}");
            assert!(!stderr.contains("system proxy"), "{stderr}");
            assert_eq!(
                std::fs::read_to_string(data.join("state.json")).unwrap(),
                state
            );
            assert!(!sysproxy_file(&data).exists());
        }
    }

    /// `http_get_with` on a thread of its own: the target server it reaches
    /// runs on the test's runtime.
    async fn http_get_off_the_runtime(port: u16, url: &str, headers: &str) -> String {
        let (url, headers) = (url.to_string(), headers.to_string());
        tokio::task::spawn_blocking(move || http_get_with(port, &url, &headers))
            .await
            .unwrap()
    }

    /// `--listen-http` takes the place of everything the profile says in
    /// `http-listen`, a password included, and keeps it across reloads. The
    /// profile's own addresses are ports somebody else holds: were one of them
    /// tried, the start would fail, and so would the reload after an edit.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn listen_http_replaces_the_profiles_http_listeners_for_good() {
        let target = TestServer::spawn().await;
        target.set("/hello", "hi from target");
        let url = format!("http://127.0.0.1:{}/hello", target.url("/").port().unwrap());
        let held: Vec<std::net::TcpListener> = (0..3)
            .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
            .collect();
        let at = |i: usize| held[i].local_addr().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let profile = |http_listen: String| {
            write_conf(
                dir.path(),
                &format!(
                    "http-listen = {http_listen}\nsocks5-listen = 127.0.0.1:0\nhttp-api = k@127.0.0.1:0\nloglevel = warning"
                ),
            )
        };
        let conf = profile(format!("{}, {}", at(0), at(1)));
        let daemon = tokio::task::spawn_blocking({
            let data = dir.path().join("data");
            move || {
                spawn_daemon_full(
                    &conf,
                    &data,
                    false,
                    None,
                    &["--listen-http", "S3CR3T@127.0.0.1:0"],
                )
            }
        })
        .await
        .unwrap();
        let (proxy, api) = (daemon.http, api_port(&daemon));
        // the profile's SOCKS5 listener is where the profile put it
        assert!(daemon.socks > 0);
        let credentials = basic_credentials("anyone:S3CR3T");
        let check = |when: &'static str| {
            let denied = http_get(proxy, &url);
            assert!(denied.starts_with("HTTP/1.1 407"), "{when}: {denied}");
            let wrong = http_get_with(proxy, &url, &basic_credentials("anyone:wrong"));
            assert!(wrong.starts_with("HTTP/1.1 407"), "{when}: {wrong}");
        };
        check("at the start");
        let served = http_get_off_the_runtime(proxy, &url, &credentials).await;
        assert!(
            served.starts_with("HTTP/1.1 200") && served.ends_with("hi from target"),
            "{served}"
        );
        let reload = |when: &'static str| {
            let (status, body) = api_call(api, "POST", "/v1/profiles/reload", "k", Some("{}"));
            assert_eq!(status, 200, "{when}: {body}");
            assert!(
                body.contains("\"ok\":true") && body.contains("\"listenersRebound\":false"),
                "{when}: {body}"
            );
        };
        reload("the profile as it was");
        check("after a reload");
        // the profile's `http-listen` is edited to another address: the
        // listeners stay, with the port that answered before
        profile(at(2).to_string());
        reload("the profile edited");
        check("after the profile's http-listen was edited");
        let served = http_get_off_the_runtime(proxy, &url, &credentials).await;
        assert!(
            served.starts_with("HTTP/1.1 200") && served.ends_with("hi from target"),
            "{served}"
        );
    }

    /// The environment does what the option does, for the other kind of
    /// listener and for an address of the other family: the SOCKS5 listener is
    /// on `::1`, and the profile's own address is not tried.
    #[test]
    fn listen_socks5_in_the_environment_replaces_the_profiles_socks5_listeners() {
        if std::net::TcpListener::bind("[::1]:0").is_err() {
            eprintln!("skipped: this machine has no IPv6 loopback");
            return;
        }
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(
            dir.path(),
            &format!(
                "http-listen = 127.0.0.1:0\nsocks5-listen = {}\nloglevel = warning",
                held.local_addr().unwrap()
            ),
        );
        let mut cmd = rurge_run(&conf, &dir.path().join("data"));
        cmd.env("RURGE_LISTEN_SOCKS5", "[::1]:0");
        let daemon = spawn_command(cmd);
        let mut socks = TcpStream::connect(("::1", daemon.socks)).unwrap();
        socks
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socks.write_all(&[5, 1, 0]).unwrap();
        let mut reply = [0u8; 2];
        socks.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [5, 0], "SOCKS5 without authentication");
        drop(held);
    }

    /// An option replaces the listeners of its own kind only. The profile's
    /// listeners of the other kind are still bound: at a port somebody else
    /// holds, the start fails.
    #[test]
    fn a_listen_option_leaves_the_other_kind_of_listener_to_the_profile() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = held.local_addr().unwrap();
        let dir = tempfile::tempdir().unwrap();
        for (general, option) in [
            (
                format!("http-listen = 127.0.0.1:0\nsocks5-listen = {taken}"),
                "--listen-http",
            ),
            (
                format!("http-listen = {taken}\nsocks5-listen = 127.0.0.1:0"),
                "--listen-socks5",
            ),
        ] {
            let conf = write_conf(dir.path(), &general);
            let mut cmd = rurge_run(&conf, &dir.path().join("data"));
            cmd.args([option, "127.0.0.1:0"]);
            let (code, stderr) = give_up(cmd);
            assert_eq!(code, Some(1), "{option}: {stderr}");
            assert!(
                stderr.contains("cannot bind listener"),
                "{option}: {stderr}"
            );
        }
        drop(held);
    }

    /// Where rurge listens is not guessed. A value it cannot use ends the
    /// start, and the message names the option and the reason, never the
    /// value: it can hold the proxy's password. An empty value counts, from the
    /// environment too.
    #[test]
    fn a_listen_value_that_cannot_be_used_ends_the_start_without_quoting_it() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        let by_argument = [
            (["--listen-http", "S3CR3T@not-an-address"], "--listen-http"),
            (["--listen-http", ",S3CR3T@nowhere"], "--listen-http"),
            (["--listen-http", ""], "--listen-http"),
            (["--listen-http", ","], "--listen-http"),
            (
                ["--listen-socks5", "S3CR3T@127.0.0.1:1080"],
                "--listen-socks5",
            ),
            (["--listen-socks5", "nowhere:1080"], "--listen-socks5"),
        ];
        let by_variable = [
            (
                "RURGE_LISTEN_HTTP",
                "S3CR3T@not-an-address",
                "--listen-http",
            ),
            ("RURGE_LISTEN_HTTP", "", "--listen-http"),
            (
                "RURGE_LISTEN_SOCKS5",
                "S3CR3T@127.0.0.1:1080",
                "--listen-socks5",
            ),
        ];
        let mut starts = Vec::new();
        for (args, option) in by_argument {
            let mut cmd = rurge_run(&conf, &data);
            cmd.args(args);
            starts.push((cmd, option, format!("{args:?}")));
        }
        for (variable, value, option) in by_variable {
            let mut cmd = rurge_run(&conf, &data);
            cmd.env(variable, value);
            starts.push((cmd, option, format!("{variable}={value:?}")));
        }
        for (cmd, option, how) in starts {
            let (code, stderr) = give_up(cmd);
            assert_eq!(code, Some(2), "{how}: {stderr}");
            assert!(
                stderr.starts_with(&format!("error: {option}: ")),
                "{how}: {stderr}"
            );
            for value in ["S3CR3T", "not-an-address", "nowhere"] {
                assert!(!stderr.contains(value), "{how}: {stderr}");
            }
            assert!(
                !data.exists(),
                "{how}: the start left a data directory behind"
            );
        }
    }

    /// The start that ends on a listen value is one of those that end before
    /// the log is open, and puts back what a crashed run left behind as they do.
    #[test]
    fn a_listen_value_that_cannot_be_used_still_restores_the_system_proxy() {
        let dir = tempfile::tempdir().unwrap();
        let data = crashed_with_the_system_proxy_on(dir.path());
        let mut cmd = rurge_run(&dir.path().join("t.conf"), &data);
        cmd.args(["--listen-http", "S3CR3T@not-an-address"]);
        let (code, stderr) = give_up(cmd);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("error: --listen-http: "), "{stderr}");
        assert!(!stderr.contains("S3CR3T"), "{stderr}");
        assert_restored(&data, &stderr);
    }

    /// The profile `state.json` in `data` names as the current one.
    fn recorded_profile(data: &Path) -> Option<String> {
        let text = std::fs::read_to_string(data.join("state.json")).ok()?;
        let state: serde_json::Value = serde_json::from_str(&text).unwrap();
        state["current_profile"].as_str().map(str::to_string)
    }

    /// Waits for the daemon to say it is running, and gives the profile
    /// `state.json` names by then: the record is written before that line is
    /// printed.
    fn wait_until_recorded(daemon: &Daemon, data: &Path) -> String {
        wait_for_line(daemon, "rurge ");
        recorded_profile(data).expect("state.json names the profile of a daemon that runs")
    }

    /// What `rurge run` was started with is the profile that is running, and
    /// `state.json` says so: the path as an absolute one, however `-c` gave it.
    #[test]
    fn a_running_daemon_records_its_profile_as_the_current_one() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        assert_eq!(recorded_profile(&data), None);
        let daemon = spawn_daemon(&conf, &data);
        assert_eq!(
            wait_until_recorded(&daemon, &data),
            conf.to_str().unwrap(),
            "the path as given, when that is an absolute one"
        );
        drop(daemon);

        // the same profile by a relative path, in a data directory of its own
        let fresh = dir.path().join("fresh");
        let mut cmd = rurge_run(Path::new("t.conf"), &fresh);
        cmd.current_dir(dir.path());
        let daemon = spawn_command(cmd);
        let recorded = wait_until_recorded(&daemon, &fresh);
        assert!(Path::new(&recorded).is_absolute(), "{recorded}");
        assert!(!recorded.starts_with(r"\\?\"), "{recorded}");
        assert_eq!(
            std::fs::canonicalize(&recorded).unwrap(),
            std::fs::canonicalize(&conf).unwrap()
        );
    }

    /// Starting the profile that is recorded already writes nothing: the file
    /// is whatever an earlier run, or somebody, left, down to its spaces.
    #[test]
    fn a_start_with_the_recorded_profile_leaves_state_json_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        // not the form the daemon writes, so that a rewrite shows
        let written = serde_json::json!({"version": 1, "current_profile": conf.to_str().unwrap()})
            .to_string();
        std::fs::write(data.join("state.json"), &written).unwrap();
        let mut daemon = spawn_daemon(&conf, &data);
        let api = api_port(&daemon);
        wait_for_line(&daemon, "rurge ");
        // gone, so that nothing it was going to write is still to come
        assert_eq!(api_call(api, "POST", "/v1/stop", "k", Some("{}")).0, 200);
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(0), "stop exits 0");
        assert_eq!(
            std::fs::read_to_string(data.join("state.json")).unwrap(),
            written
        );
    }

    /// Another profile started in the same data directory is the current one
    /// from then on.
    #[test]
    fn another_profile_becomes_the_recorded_one() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_conf(dir.path(), API_GENERAL);
        let second = dir.path().join("other.conf");
        std::fs::copy(&first, &second).unwrap();
        let data = dir.path().join("data");
        let daemon = spawn_daemon(&first, &data);
        assert_eq!(wait_until_recorded(&daemon, &data), first.to_str().unwrap());
        drop(daemon);
        let daemon = spawn_daemon(&second, &data);
        wait_for_line(&daemon, "rurge ");
        assert_eq!(recorded_profile(&data).as_deref(), second.to_str());
    }

    /// A start that does not get to the point of saying it is running did not
    /// run its profile: not when a listener or the API cannot be bound, not
    /// when the system proxy cannot be set, and whatever was recorded before
    /// stays.
    #[test]
    fn a_start_that_fails_does_not_record_its_profile() {
        let dir = tempfile::tempdir().unwrap();
        let earlier = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        let daemon = spawn_daemon(&earlier, &data);
        wait_until_recorded(&daemon, &data);
        drop(daemon);
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = held.local_addr().unwrap();
        let broken_backend = dir.path().join("no-such-dir").join("sysproxy.json");
        let fresh = dir.path().join("fresh");
        let other = dir.path().join("other.conf");
        for (what, general, system_proxy) in [
            (
                "a listener's port is taken",
                format!("http-listen = {taken}\nsocks5-listen = 127.0.0.1:0"),
                false,
            ),
            (
                "the API's port is taken",
                format!(
                    "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nhttp-api = k@{taken}"
                ),
                false,
            ),
            (
                "the system proxy cannot be set",
                API_GENERAL.to_string(),
                true,
            ),
        ] {
            std::fs::write(
                &other,
                format!("[General]\n{general}\n[Proxy]\n[Rule]\nFINAL,DIRECT\n"),
            )
            .unwrap();
            for state_dir in [&data, &fresh] {
                let mut cmd = rurge_run(&other, state_dir);
                if system_proxy {
                    cmd.arg("--system-proxy").env(
                        "RURGE_SYSTEM_PROXY_BACKEND",
                        format!("file:{}", broken_backend.display()),
                    );
                }
                let (code, stderr) = give_up(cmd);
                assert_eq!(code, Some(1), "{what}: {stderr}");
            }
            assert_eq!(
                recorded_profile(&data).as_deref(),
                earlier.to_str(),
                "{what}: the earlier one stays"
            );
            assert_eq!(recorded_profile(&fresh), None, "{what}");
        }
        drop(held);
    }

    /// The record of the profile is written on the way to saying the daemon
    /// runs, and a record that cannot be written is a line in the log and
    /// no more, as it is for every other write of `state.json`: the start goes
    /// on and the daemon serves. The obstacle is the one that stops every
    /// rewrite of the file: a directory where its temporary file goes.
    #[test]
    fn a_start_that_cannot_write_state_json_still_runs_and_serves() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        std::fs::create_dir_all(data.join("state.json.tmp")).unwrap();
        let daemon = spawn_daemon(&conf, &data);
        let api = api_port(&daemon);
        // the lines up to the one that says it runs
        let mut said = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let line = daemon
                .lines
                .recv_timeout(remaining)
                .expect("rurge run said it runs");
            let runs = line.starts_with("rurge ");
            said.push(line);
            if runs {
                break;
            }
        }
        assert!(
            said.iter()
                .any(|line| line.contains("cannot write state.json")),
            "{said:?}"
        );
        assert_eq!(
            api_call(api, "GET", "/v1/outbound", "k", None),
            (200, r#"{"mode":"rule"}"#.to_string())
        );
        assert!(!data.join("state.json").exists());
    }

    /// `t.conf` with an `external` policy whose program is to listen on `port`.
    /// The program does not exist and is never started: nothing is sent
    /// through the policy.
    fn write_conf_with_external(dir: &Path, port: u16) -> std::path::PathBuf {
        let conf = dir.join("t.conf");
        std::fs::write(
            &conf,
            format!(
                "[General]\n{API_GENERAL}\n[Proxy]\nX = external, exec = /no/such/program, local-port = {port}\n\
[Rule]\nFINAL,DIRECT\n"
            ),
        )
        .unwrap();
        conf
    }

    /// The load keeps an `external` policy's `local-port` off the listeners
    /// the profile writes. An override is put over the profile after the load,
    /// and rurge on that port would dial itself: the start ends, and says the
    /// option and the port, not the value.
    #[test]
    fn a_listen_override_on_the_port_of_an_external_policy_ends_the_start() {
        // the port only has to be a number: nothing binds it
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf_with_external(dir.path(), port);
        let data = dir.path().join("data");
        for (option, value) in [
            ("--listen-socks5", format!("127.0.0.1:{port}")),
            ("--listen-http", format!("S3CR3T@127.0.0.1:{port}")),
        ] {
            let mut cmd = rurge_run(&conf, &data);
            cmd.args([option, &value]);
            let (code, stderr) = give_up(cmd);
            assert_eq!(code, Some(2), "{option}: {stderr}");
            assert_eq!(
                stderr,
                format!(
                    "error: {option}: port {port} is the `local-port` of an `external` policy\n"
                ),
                "{option}"
            );
            assert!(!data.exists(), "{option}: the start left a data directory");
        }
        drop(held);
    }

    /// This start, too, ends before the log is open, and puts back what a
    /// crashed run left behind.
    #[test]
    fn a_listen_override_on_the_port_of_an_external_policy_still_restores_the_system_proxy() {
        let dir = tempfile::tempdir().unwrap();
        let data = crashed_with_the_system_proxy_on(dir.path());
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let conf = write_conf_with_external(dir.path(), port);
        let mut cmd = rurge_run(&conf, &data);
        cmd.args(["--listen-socks5", &format!("127.0.0.1:{port}")]);
        let (code, stderr) = give_up(cmd);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(
            stderr.contains(&format!(
                "error: --listen-socks5: port {port} is the `local-port` of an `external` policy\n"
            )),
            "{stderr}"
        );
        assert_restored(&data, &stderr);
        drop(held);
    }

    /// A port for a profile to name before rurge binds it, which rurge itself
    /// does: one that was free a moment ago, in a range the system does not
    /// hand out by itself (the dynamic ports of Windows by default, Linux and
    /// macOS are all above it), tried just before it is returned. Not one
    /// the test gave back after the system had chosen it.
    fn free_tcp_port() -> u16 {
        const PORTS: std::ops::Range<u64> = 20_000..30_000;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        // this process starts somewhere else in the range than the one before it
        let start = u64::from(std::process::id()).wrapping_mul(7919);
        for _ in 0..100 {
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let port = PORTS.start + start.wrapping_add(n) % (PORTS.end - PORTS.start);
            let port = u16::try_from(port).unwrap();
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return port;
            }
        }
        panic!("no port of {PORTS:?} is free");
    }

    /// The next line of the daemon's standard error that has `needle` in it.
    fn wait_for_stderr_line(lines: &mpsc::Receiver<String>, needle: &str) -> String {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let line = lines
                .recv_timeout(remaining)
                .unwrap_or_else(|_| panic!("rurge run never wrote a line with `{needle}`"));
            if line.contains(needle) {
                return line;
            }
        }
    }

    /// A reload puts the overrides over the profile it reads, and the profile
    /// may have gained an `external` policy on the port an override listens
    /// on: the reload fails, as one that cannot read the profile does, and the
    /// daemon goes on as it was. Here rurge itself binds the port.
    #[test]
    fn a_reload_that_puts_an_external_policy_on_an_overridden_listener_is_refused() {
        let port = free_tcp_port();
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let mut cmd = rurge_run(&conf, &dir.path().join("data"));
        cmd.args(["--listen-socks5", &format!("127.0.0.1:{port}")]);
        let (daemon, errors) = spawn_command_reading_stderr(cmd);
        assert_eq!(daemon.socks, port);
        let api = api_port(&daemon);
        write_conf_with_external(dir.path(), port);
        let (status, body) = api_call(api, "POST", "/v1/profiles/reload", "k", Some("{}"));
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("\"ok\":false"), "{body}");
        wait_for_stderr_line(
            &errors,
            &format!(
                "reload failed, keeping current config: --listen-socks5: port {port} is the `local-port` of an `external` policy"
            ),
        );
        // it goes on as it was: the SOCKS5 listener answers
        let mut socks = TcpStream::connect(("127.0.0.1", port)).unwrap();
        socks
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socks.write_all(&[5, 1, 0]).unwrap();
        let mut reply = [0u8; 2];
        socks.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [5, 0], "SOCKS5 without authentication");
    }
}

mod service {
    use assert_cmd::Command;
    use predicates::prelude::*;

    /// `SUDO_UID` is removed: `service ... --user` refuses to run under sudo,
    /// and these dry runs must not depend on how the suite was started.
    fn rurge() -> Command {
        let mut cmd = Command::cargo_bin("rurge").unwrap();
        cmd.env_remove("SUDO_UID");
        cmd
    }

    #[test]
    fn install_dry_run_prints_the_plan_for_this_platform() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("my rurge.conf");
        std::fs::write(&conf, "[General]\n[Rule]\nFINAL,DIRECT\n").unwrap();
        let out = rurge()
            .args([
                "service",
                "install",
                "--user",
                "--system-proxy",
                "--dry-run",
                "-c",
            ])
            .arg(&conf)
            .assert()
            .success();
        let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
        assert!(text.contains("my rurge.conf"), "{text}");
        assert!(text.contains("--system-proxy"), "{text}");
        assert!(text.ends_with("dry run: nothing was changed\n"), "{text}");
        if cfg!(windows) {
            assert!(
                text.contains("run: schtasks /create /tn rurge /sc onlogon /tr "),
                "{text}"
            );
        } else if cfg!(target_os = "macos") {
            assert!(
                text.contains("io.rurge.daemon.plist:")
                    && text.contains("run: launchctl bootstrap gui/"),
                "{text}"
            );
        } else {
            assert!(
                text.contains("rurge.service:") && text.contains("ExecStart="),
                "{text}"
            );
            assert!(
                text.contains("run: systemctl --user enable --now rurge"),
                "{text}"
            );
        }
    }

    #[test]
    fn uninstall_dry_run_and_a_missing_profile() {
        let out = rurge()
            .args(["service", "uninstall", "--user", "--dry-run"])
            .assert()
            .success();
        let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
        assert!(text.contains("run: "), "{text}");
        assert!(text.ends_with("dry run: nothing was changed\n"), "{text}");
        rurge()
            .args([
                "service",
                "install",
                "--dry-run",
                "-c",
                "no-such-profile.conf",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("cannot find the profile"));
    }
}
