use assert_cmd::Command;
use predicates::prelude::*;

/// Where a `rurge` started by these tests finds its default data and
/// configuration directories: under `target/`, not where the machine keeps its
/// own, so that a start which ignores the directory it was given falls back to
/// a scratch one and not to the machine's. `HOME` is left alone (the system's
/// certificate store and other libraries read it), so on macOS, where the
/// defaults come from it, a failing run is not kept away from the machine's own.
/// The two constructors below also remove `RURGE_DATA_DIR` and
/// `RURGE_LOG_FILE` from the child's environment: a developer's shell may
/// export them, and a start that forgot `--data-dir`, or any start's log file,
/// would go where they point; a test that needs one sets it after the constructor.
fn default_directories() -> [(&'static str, std::path::PathBuf); 4] {
    let scratch = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("default-directories");
    [
        ("LOCALAPPDATA", scratch.join("local-app-data")),
        ("APPDATA", scratch.join("app-data")),
        ("XDG_DATA_HOME", scratch.join("xdg-data")),
        ("XDG_CONFIG_HOME", scratch.join("xdg-config")),
    ]
}

/// `rurge` for a test that runs it to the end and looks at what it said.
fn rurge_command() -> Command {
    let mut cmd = Command::cargo_bin("rurge").unwrap();
    cmd.envs(default_directories())
        .env_remove("RURGE_DATA_DIR")
        .env_remove("RURGE_LOG_FILE");
    cmd
}

/// `rurge` for a test that starts it as a process of its own.
fn rurge_process() -> std::process::Command {
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("rurge"));
    cmd.envs(default_directories())
        .env_remove("RURGE_DATA_DIR")
        .env_remove("RURGE_LOG_FILE");
    cmd
}

#[test]
fn version_prints_crate_version() {
    rurge_command()
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
        let out = rurge_command()
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
    let out = rurge_command().args(["run", "--help"]).output().unwrap();
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

    rurge_command()
        .args(["check", "-c"])
        .arg(&valid)
        .assert()
        .success()
        .stdout(predicate::str::contains("0 error(s)"));
    rurge_command()
        .args(["check", "-c"])
        .arg(&warning)
        .assert()
        .success()
        .stdout(predicate::str::contains("W0001"));
    rurge_command()
        .args(["check", "--strict", "-c"])
        .arg(&warning)
        .assert()
        .code(1);
    rurge_command()
        .args(["check", "-c"])
        .arg(&error)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0010"));
    rurge_command()
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
        let out = rurge_command()
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

/// The rules written after the first `FINAL` never run, the ones between two
/// `FINAL`s among them, and a `FINAL` that is not the last ends matching
/// without giving the policy: `check` says both, each with what to do.
#[test]
fn check_says_what_the_first_final_does_to_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write(
        &dir,
        "finals.conf",
        "[Rule]\nFINAL,DIRECT\nDOMAIN,a.com,REJECT\nFINAL,REJECT\n",
    );
    let out = rurge_command()
        .args(["check", "--data-dir"])
        .arg(dir.path().join("data"))
        .arg("-c")
        .arg(&conf)
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = out.lines().collect();
    for (code, said) in [
        (
            "W0019",
            "finals.conf:3: 1 rule(s) after FINAL never take effect",
        ),
        (
            "W0021",
            "finals.conf:2: this FINAL does not give the policy; the last FINAL rule does",
        ),
    ] {
        let at = lines
            .iter()
            .position(|line| line.starts_with(&format!("warning[{code}]")))
            .unwrap_or_else(|| panic!("{code} is not in {out}"));
        assert!(lines[at].ends_with(said), "{out}");
        assert_eq!(
            lines[at + 1],
            format!("  hint: {}", rurge_config::codes::hint(code).unwrap()),
            "{out}"
        );
    }
    assert!(out.contains("0 error(s), 2 warning(s)"), "{out}");
}

/// `UNKNOWN` is a value of `GEOIP` and of `IP-ASN`, in any case: `check`
/// reads both lines and has nothing to say about them.
#[test]
fn check_accepts_ip_asn_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write(
        &dir,
        "unknown.conf",
        "[Rule]\nIP-ASN,unknown,DIRECT\nGEOIP,UNKNOWN,DIRECT\nFINAL,DIRECT\n",
    );
    let out = rurge_command()
        .args(["check", "--data-dir"])
        .arg(dir.path().join("data"))
        .arg("-c")
        .arg(&conf)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert!(!out.contains("E0011"), "{out}");
    assert!(out.contains("0 error(s), 0 warning(s), 0 note(s)"), "{out}");
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
    rurge_command()
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
    let out = rurge_command()
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
    let out = rurge_command()
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

/// `SYSTEM` is `Windows`, `Linux` or `macOS`, whichever platform the profile is
/// read for: of the lines held to a system by a tag, the one for that system is
/// read and no other, and the two tags for Surge's iOS and tvOS never are.
#[test]
fn check_reads_the_line_tagged_for_the_platform_and_no_other() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write(
        &dir,
        "tags.conf",
        "[Rule]\nDOMAIN,windows.test,REJECT #!WINDOWS-ONLY\nDOMAIN,linux.test,REJECT #!LINUX-ONLY\nDOMAIN,macos.test,REJECT #!MACOS-ONLY\nDOMAIN,ios.test,REJECT #!IOS-ONLY\nDOMAIN,tvos.test,REJECT #!TVOS-ONLY\nFINAL,DIRECT\n",
    );
    // the tagged lines are lines 2 to 6 of the file
    for (platform, own) in [("windows", 2), ("linux", 3), ("macos", 4)] {
        let out = rurge_command()
            .args(["check", "--json", "--platform", platform, "-c"])
            .arg(&conf)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let mut disabled: Vec<u64> = v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["code"] == "I0002")
            .map(|d| d["span"]["line"].as_u64().unwrap())
            .collect();
        disabled.sort();
        let expected: Vec<u64> = (2..=6).filter(|line| *line != own).collect();
        assert_eq!(disabled, expected, "--platform {platform}: {v}");
    }
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
    let out = rurge_command()
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
    rurge_command()
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
    rurge_command()
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
    let out = rurge_command()
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

    rurge_command()
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
    let out = rurge_command()
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
    let out = rurge_command()
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

    rurge_command()
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
    let out = rurge_command()
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

    rurge_command()
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
    let out = rurge_command()
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

    rurge_command()
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
    let out = rurge_command()
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

    rurge_command()
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
    let out = rurge_command()
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

    rurge_command()
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
    let out = rurge_command()
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

    let out = rurge_command()
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

    rurge_command()
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

const SS_CUSTOM: &str = "[General]\n[Proxy]\n\
Old = custom, proxy.test, 8388, aes-256-gcm, s3same, https://module.test/SSEncrypt.module\n\
[Rule]\nFINAL,DIRECT\n";

/// Surge's old `custom` line is read as `ss`: `rurge check` loads it without
/// an error, says that it read it so, and prints neither the password nor
/// the link of the module.
#[test]
fn check_reads_a_legacy_custom_policy_as_ss() {
    let dir = tempfile::tempdir().unwrap();
    let out = rurge_command()
        .args(["check", "-c"])
        .arg(write(&dir, "custom.conf", SS_CUSTOM))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert!(out.contains("I0001"), "{out}");
    assert!(
        out.contains(
            "custom.conf:3: policy `Old`: legacy type `custom` read as `ss`; the module URL is ignored"
        ),
        "{out}"
    );
    assert!(!out.contains("E0004"), "{out}");
    assert!(out.contains("0 error(s)"), "{out}");
    assert!(
        !out.contains("s3same") && !out.contains("module.test"),
        "{out}"
    );
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
    let out = rurge_command()
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

    let out = rurge_command()
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

    rurge_command()
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
    let out = rurge_command()
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

    rurge_command()
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
    let out = rurge_command()
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

    rurge_command()
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
    let out = rurge_command()
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

    let out = rurge_command()
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
    let out = rurge_command()
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

    let out = rurge_command()
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
    let out = rurge_command()
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
    rurge_command()
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
    rurge_command()
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

/// A profile that is only the line that names its managed profile is checked
/// as the daemon reads it: through the copy in the data directory. `check`
/// reads that directory, and neither writes to it nor makes it.
#[test]
fn check_reads_the_downloaded_copy_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let link = url::Url::parse("https://sources.example/profile.conf").unwrap();
    let data = dir.path().join("data");
    rurge_net::testing::seed_cache(
        &data,
        &link,
        b"[Rule]\nFINAL,DIRECT\n",
        std::time::SystemTime::now() - std::time::Duration::from_secs(3600),
    );
    let pointer = write(&dir, "managed.conf", &format!("#!MANAGED-CONFIG {link}\n"));
    let tree = |root: &std::path::Path| {
        let mut found = Vec::new();
        let mut todo = vec![root.to_path_buf()];
        while let Some(dir) = todo.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    todo.push(path);
                } else {
                    found.push((path.clone(), fs::read(path).unwrap()));
                }
            }
        }
        found.sort();
        found
    };
    let before = tree(&data);
    rurge_command()
        .args(["check", "-c"])
        .arg(&pointer)
        .arg("--data-dir")
        .arg(&data)
        .assert()
        .success()
        .stdout(predicate::str::contains("I0004"));
    assert_eq!(tree(&data), before);
    // a directory that is not there is not made, and the pointer has nothing
    // to point at
    let nothing = dir.path().join("nothing");
    rurge_command()
        .args(["check", "-c"])
        .arg(&pointer)
        .arg("--data-dir")
        .arg(&nothing)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("W0022"));
    assert!(!nothing.exists());
}

mod rule_match {
    use super::rurge_command;
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
        let mut cmd = rurge_command();
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
            let mut cmd = rurge_command();
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

    /// A profile that is only the line that names its managed profile is
    /// matched by the copy in the data directory, which the command reads
    /// and never downloads.
    #[test]
    fn a_managed_profile_is_matched_by_its_downloaded_copy() {
        let dir = tempfile::tempdir().unwrap();
        let link = url::Url::parse("https://sources.example/profile.conf").unwrap();
        let data = dir.path().join("data");
        rurge_net::testing::seed_cache(
            &data,
            &link,
            format!(
                "#!MANAGED-CONFIG {link}\n[Proxy]\nP = direct\n[Rule]\nDOMAIN,copy.test,P\nFINAL,DIRECT\n"
            )
            .as_bytes(),
            std::time::SystemTime::now() - std::time::Duration::from_secs(3600),
        );
        let pointer = dir.path().join("managed.conf");
        std::fs::write(&pointer, format!("#!MANAGED-CONFIG {link}\n")).unwrap();
        let out = rurge_command()
            .arg("rule")
            .arg("match")
            .arg("-c")
            .arg(&pointer)
            .arg("--no-network")
            .arg("--data-dir")
            .arg(&data)
            .args(["copy.test", "--json"])
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["policy"], "P");
        assert_eq!(v["matched"]["raw"], "DOMAIN,copy.test,P");
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

    /// The manual: "In the [Rule] section, a FINAL rule immediately terminates
    /// rule matching", and "If several enabled FINAL rules are present, the
    /// last one is the effective one." The rule between the two is not in the
    /// trace, and the policy is the last `FINAL`'s.
    #[test]
    fn matching_ends_at_the_first_final_and_the_last_one_gives_the_policy() {
        let dir = workspace();
        std::fs::write(
            dir.path().join("t.conf"),
            "[Proxy]\nX = direct\nY = direct\nA = direct\n[Rule]\nFINAL,X\nDOMAIN,a.com,A\nFINAL,Y\n",
        )
        .unwrap();
        rule_match(dir.path(), &["a.com", "--explain"])
            .assert()
            .success()
            .stdout(predicate::str::contains("policy: Y"))
            .stdout(predicate::str::contains("reason: final"))
            .stdout(predicate::str::contains("rule #2: FINAL,Y"))
            .stdout(predicate::str::contains("#2 match"))
            .stdout(predicate::str::contains("DOMAIN,a.com,A").not())
            .stdout(predicate::str::contains("#1 ").not())
            .stderr(predicate::str::contains("W0019"))
            .stderr(predicate::str::contains("W0021"));
        let (v, code) = json(dir.path(), &["a.com"]);
        assert_eq!(code, 0);
        assert_eq!(v["policy"], "Y");
        assert_eq!(v["reason"], "final");
        assert_eq!(v["matched"]["index"], 2);
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
    use super::rurge_command;
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
        let mut cmd = rurge_command();
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

    /// A profile that is only the line that names its managed profile gets
    /// the `[Host]` entries of the copy in the data directory.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_host_entry_of_a_managed_profiles_copy_answers_a_lookup() {
        let mock = MockDns::spawn().await;
        let dir = workspace();
        let link = url::Url::parse("https://sources.example/profile.conf").unwrap();
        rurge_net::testing::seed_cache(
            &dir.path().join("data"),
            &link,
            format!(
                "#!MANAGED-CONFIG {link}\n[Host]\nfixed.test = 1.2.3.4\n[Rule]\nFINAL,DIRECT\n"
            )
            .as_bytes(),
            std::time::SystemTime::now() - std::time::Duration::from_secs(3600),
        );
        std::fs::write(
            dir.path().join("t.conf"),
            format!("#!MANAGED-CONFIG {link}\n"),
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

    /// `--dns-cache-size` says how many names the cache holds: with room for
    /// two, the third name asked for pushes the one asked for first out.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_cache_holds_as_many_names_as_dns_cache_size_says() {
        let mock = MockDns::spawn().await;
        for name in ["a.test", "b.test", "c.test"] {
            mock.set(name, &["10.0.0.3"], &[], 60);
        }
        let dir = workspace();
        let server = mock.addr().to_string();
        let kept = |extra: &'static [&'static str]| {
            let (dir, server) = (dir.path().to_path_buf(), server.clone());
            async move {
                let mut args = vec!["a.test", "b.test", "c.test", "--json"];
                args.extend_from_slice(extra);
                let out = output(dns_cmd(&dir, "cache", &server, &args)).await;
                assert_eq!(out.status.code(), Some(0));
                let mut names: Vec<String> = json_of(&out)["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e["name"].as_str().unwrap().to_string())
                    .collect();
                names.sort();
                names
            }
        };
        assert_eq!(kept(&["--dns-cache-size", "2"]).await, ["b.test", "c.test"]);
        assert_eq!(kept(&[]).await, ["a.test", "b.test", "c.test"]);
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
        let mut cmd = rurge_command();
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
    use super::{rurge_command, rurge_process};
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

    /// A profile that rejects `ads.test` and sends the rest direct. `ads.test`
    /// is also given the loopback's address, so that a session which escapes
    /// the rule (the bug a test of the rule is there to catch) ends on the
    /// loopback and not in a question to the machine's DNS servers.
    fn write_conf(dir: &Path, general: &str) -> std::path::PathBuf {
        let conf = dir.join("t.conf");
        std::fs::write(
            &conf,
            format!(
                "[General]\n{general}\n[Proxy]\n[Rule]\nDOMAIN,ads.test,REJECT\nFINAL,DIRECT\n[Host]\nads.test = 127.0.0.1\n"
            ),
        )
        .unwrap();
        conf
    }

    /// What every start of `rurge run` in these tests has in common. P5: no
    /// test may reach the real system proxy, so the file backend is wired in
    /// here instead of being remembered at every call site. `network` is what
    /// goes between the profile and the data directory: whether the start
    /// goes to the network at all, and where it would fetch GeoIP databases.
    fn rurge_run_with(conf: &Path, data: &Path, network: &[&str]) -> Command {
        let mut cmd = rurge_process();
        cmd.arg("run")
            .arg("-c")
            .arg(conf)
            .args(network)
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

    /// Where on the server the two GeoIP databases of `rurge_run_online` are.
    const COUNTRY_DATABASE: &str = "/GeoLite2-Country.mmdb";
    const ASN_DATABASE: &str = "/GeoLite2-ASN.mmdb";

    /// The way a CLI test starts `rurge run` when it has nothing to do with
    /// the network.
    fn rurge_run(conf: &Path, data: &Path) -> Command {
        rurge_run_with(conf, data, &["--no-network"])
    }

    /// `rurge run` that may go to the network, and so downloads the sources
    /// of the profile. The two GeoIP databases it would fetch from the
    /// public internet are asked of `geo`, a server on the loopback that
    /// answers 404 to both.
    fn rurge_run_online(conf: &Path, data: &Path, geo: &TestServer) -> Command {
        let country = geo.url(COUNTRY_DATABASE).to_string();
        let asn = geo.url(ASN_DATABASE).to_string();
        let mut cmd = rurge_run_with(
            conf,
            data,
            &["--geoip-url", &country, "--geoip-asn-url", &asn],
        );
        cmd.env_remove("RURGE_NO_NETWORK");
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

    /// Minimal HTTP/1.1 call against the API: (status, body). The answer may
    /// take five seconds.
    fn api_call(
        port: u16,
        method: &str,
        path: &str,
        key: &str,
        body: Option<&str>,
    ) -> (u16, String) {
        api_call_within(port, method, path, key, body, Duration::from_secs(5))
    }

    /// `api_call` for a call whose answer may take `wait` to come, such as the
    /// update of a resource, which is as long as a download.
    ///
    /// The request leaves in one write, which is the simplest form of it. The
    /// API answers a request whose body comes after its head as well:
    /// `api_call_with_a_late_body` sends one.
    fn api_call_within(
        port: u16,
        method: &str,
        path: &str,
        key: &str,
        body: Option<&str>,
        wait: Duration,
    ) -> (u16, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(wait)).unwrap();
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

    /// What a daemon that has exited printed and nobody has taken yet, to the
    /// end of its output: the lines that were on their way when it exited are
    /// among them.
    fn printed_to_the_end(daemon: &Daemon) -> Vec<String> {
        let mut lines = Vec::new();
        loop {
            match daemon.lines.recv_timeout(Duration::from_secs(5)) {
                Ok(line) => lines.push(line),
                Err(mpsc::RecvTimeoutError::Disconnected) => return lines,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    panic!("the output of the daemon did not end within 5 s: {lines:?}")
                }
            }
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

    /// What `cmd` (a prepared `rurge run`) wrote to its standard output up to
    /// its summary line: the lines its log writes before that, and the lines it
    /// prints. The daemon is killed on the way out.
    fn output_until_it_is_running(mut cmd: Command) -> Vec<String> {
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let lines = forward_lines(child.stdout.take().unwrap());
        let daemon = Daemon {
            child,
            http: 0,
            socks: 0,
            lines,
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut seen = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let line = daemon.lines.recv_timeout(remaining).unwrap_or_else(|e| {
                panic!("rurge run never said it was running: {e:?}\n{seen:#?}")
            });
            let summary = line.starts_with("rurge ") && line.contains(" running:");
            seen.push(line);
            if summary {
                return seen;
            }
        }
    }

    /// `loglevel` decides which of the daemon's own lines are written, and
    /// `--log-level` decides over it: at `warning` neither the INFO line the
    /// daemon writes when it is running nor the DEBUG lines of its listeners
    /// are there, at `notify` the first is, at `info` and `verbose` both are.
    #[test]
    fn loglevel_decides_which_lines_of_its_own_the_daemon_writes() {
        let dir = tempfile::tempdir().unwrap();
        // (the profile's `loglevel`, what the command line adds, whether the
        // INFO line is written, whether the DEBUG lines are)
        let cases: [(&str, &[&str], bool, bool); 6] = [
            ("warning", &[], false, false),
            ("notify", &[], true, false),
            ("info", &[], true, true),
            ("verbose", &[], true, true),
            ("notify", &["--log-level", "warning"], false, false),
            ("warning", &["--log-level", "info"], true, true),
        ];
        for (i, (loglevel, extra, info, debug)) in cases.into_iter().enumerate() {
            let case = dir.path().join(format!("case{i}"));
            std::fs::create_dir_all(&case).unwrap();
            let conf = write_conf(
                &case,
                &format!(
                    "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = {loglevel}"
                ),
            );
            let mut cmd = rurge_run(&conf, &case.join("data"));
            cmd.env_remove("RURGE_LOG_LEVEL").args(extra);
            let said = output_until_it_is_running(cmd);
            let wrote = |level: &str, what: &str| {
                said.iter()
                    .any(|line| line.contains(level) && line.contains(what))
            };
            assert_eq!(
                wrote(" INFO ", "rurge running"),
                info,
                "{loglevel} {extra:?}: {said:#?}"
            );
            assert_eq!(
                wrote(" DEBUG ", "listening"),
                debug,
                "{loglevel} {extra:?}: {said:#?}"
            );
        }
    }

    /// The API is served where the profile configures it and nowhere else: the
    /// daemon of a profile without `http-api` starts none, and does not say
    /// `api on …` as the one of a profile with it does.
    #[test]
    fn the_api_is_served_only_where_the_profile_configures_it() {
        let dir = tempfile::tempdir().unwrap();
        let without = "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning";
        for (i, (general, served)) in [(without, false), (API_GENERAL, true)]
            .into_iter()
            .enumerate()
        {
            let case = dir.path().join(format!("case{i}"));
            std::fs::create_dir_all(&case).unwrap();
            let conf = write_conf(&case, general);
            let said = output_until_it_is_running(rurge_run(&conf, &case.join("data")));
            let api = said.iter().find(|line| line.starts_with("api on "));
            assert_eq!(api.is_some(), served, "{said:#?}");
            if let Some(line) = api {
                assert!(line.starts_with("api on http://127.0.0.1:"), "{line}");
            }
        }
    }

    /// The log file is rolled daily and seven files are kept: the file the
    /// daemon writes is named for the day (`rurge.log.2026-10-13`), and the
    /// files that were there already go, oldest first, until with the new one
    /// seven are left.
    #[test]
    fn the_log_file_is_rolled_daily_and_seven_files_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(
            dir.path(),
            "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = notify",
        );
        let logs = dir.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        // nine files of earlier days, made in the order of their days
        for day in 1..=9 {
            std::fs::write(logs.join(format!("rurge.log.2020-01-0{day}")), "a day\n").unwrap();
        }
        let mut cmd = rurge_run(&conf, &dir.path().join("data"));
        cmd.arg("--log-file").arg(logs.join("rurge.log"));
        let daemon = spawn_command(cmd);
        let names = || -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&logs)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with("rurge.log"))
                .collect();
            names.sort();
            names
        };
        wait_until("the startup line in the file of the day", || {
            names()
                .iter()
                .filter(|name| !name.starts_with("rurge.log.2020-"))
                .any(|name| {
                    std::fs::read_to_string(logs.join(name))
                        .map(|text| text.contains("rurge running"))
                        .unwrap_or(false)
                })
        });
        let names = names();
        let today = names
            .iter()
            .find(|name| !name.starts_with("rurge.log.2020-"))
            .expect("the file of the day");
        let date = today.strip_prefix("rurge.log.").expect("a dated name");
        assert!(
            date.len() == 10
                && date.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }),
            "{today}"
        );
        // which of the old ones go is the appender's to say (it goes by when
        // the files were made); that seven are left is ours
        let old: Vec<String> = (1..=9)
            .map(|day| format!("rurge.log.2020-01-0{day}"))
            .collect();
        assert_eq!(names.len(), 7, "{names:?}");
        assert!(
            names
                .iter()
                .filter(|name| *name != today)
                .all(|name| old.contains(name)),
            "{names:?}"
        );
        drop(daemon);
    }

    /// Everything the lines of `lines` come to once the sender is gone: the
    /// daemon has ended, and its pipe is read to the end.
    fn drained(lines: &mpsc::Receiver<String>) -> Vec<String> {
        let mut all = Vec::new();
        while let Ok(line) = lines.recv_timeout(Duration::from_secs(2)) {
            all.push(line);
        }
        all
    }

    /// No credential of the profile reaches what the daemon writes, at the
    /// most talkative level: its standard output, its standard error and its
    /// log file hold nothing of the password of `http-listen`, the key of
    /// `http-api` and the credentials of an upstream proxy, through a start,
    /// a request that is let in and one that is turned away, API calls with
    /// the right key and a wrong one, a reload and a stop.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_credential_of_the_profile_reaches_the_daemons_output_or_log_file() {
        use rurge_proto::testing::{FakeHttpProxy, HttpProxyScript};
        const MARKER: &str = "S3CR3T";
        let upstream = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let conf = dir.path().join("t.conf");
        std::fs::write(
            &conf,
            format!(
                "[General]\nhttp-listen = {MARKER}-listen@127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nhttp-api = {MARKER}-key@127.0.0.1:0\nloglevel = verbose\n\
[Proxy]\nUp = http, 127.0.0.1, {}, {MARKER}-user, {MARKER}-pass\n[Rule]\nDOMAIN,via.test,Up\nFINAL,DIRECT\n[Host]\nvia.test = 127.0.0.1\n",
                upstream.addr().port()
            ),
        )
        .unwrap();
        let mut cmd = rurge_run(&conf, &dir.path().join("data"));
        cmd.arg("--log-file").arg(logs.join("rurge.log"));
        let (mut daemon, errors) =
            tokio::task::spawn_blocking(move || spawn_command_reading_stderr(cmd))
                .await
                .unwrap();
        let (http, api) = (daemon.http, api_port(&daemon));
        let (served, turned_away) = tokio::task::spawn_blocking(move || {
            let key = format!("{MARKER}-key");
            let url = "http://via.test/hello";
            let served = http_get_with(
                http,
                url,
                &basic_credentials(&format!("anyone:{MARKER}-listen")),
            );
            let turned_away = http_get_with(
                http,
                url,
                &basic_credentials(&format!("anyone:{MARKER}-wrong")),
            );
            let _ = api_call(api, "GET", "/v1/requests/recent", &key, None);
            let _ = api_call(
                api,
                "GET",
                "/v1/requests/recent",
                &format!("{MARKER}-wrongkey"),
                None,
            );
            let _ = api_call(api, "POST", "/v1/profiles/reload", &key, Some("{}"));
            let _ = api_call(api, "GET", "/v1/profiles/current", &key, None);
            let _ = api_call(api, "POST", "/v1/stop", &key, Some("{}"));
            (served, turned_away)
        })
        .await
        .unwrap();
        assert!(
            served.starts_with("HTTP/1.1 200") && served.ends_with("forwarded"),
            "{served}"
        );
        assert!(turned_away.starts_with("HTTP/1.1 407"), "{turned_away}");
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(0));
        let mut written = drained(&daemon.lines);
        written.extend(drained(&errors));
        for entry in std::fs::read_dir(&logs).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            written.extend(text.lines().map(str::to_string));
        }
        // the level is the most talkative one, and the traffic is in it
        assert!(
            written
                .iter()
                .any(|line| line.contains("session completed")),
            "{written:#?}"
        );
        let leaked: Vec<&String> = written
            .iter()
            .filter(|line| line.contains(MARKER))
            .collect();
        assert!(leaked.is_empty(), "a credential was written: {leaked:#?}");
    }

    /// How many finished requests the log keeps is what `--request-log-size`
    /// or `RURGE_REQUEST_LOG_SIZE` says; without either it keeps more than the
    /// three of this test.
    #[test]
    fn the_request_log_holds_as_many_requests_as_request_log_size_says() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        for (i, how) in ["the option", "the variable", "neither"]
            .into_iter()
            .enumerate()
        {
            let mut cmd = rurge_run(&conf, &dir.path().join(format!("data{i}")));
            cmd.env_remove("RURGE_REQUEST_LOG_SIZE");
            let kept = match how {
                "the option" => {
                    cmd.args(["--request-log-size", "2"]);
                    2
                }
                "the variable" => {
                    cmd.env("RURGE_REQUEST_LOG_SIZE", "2");
                    2
                }
                _ => 3,
            };
            let daemon = spawn_command(cmd);
            let api = api_port(&daemon);
            for _ in 0..3 {
                let rejected = http_get(daemon.http, "http://ads.test/");
                assert!(rejected.is_empty(), "{how}: {rejected}");
            }
            let (status, body) = api_call(api, "GET", "/v1/requests/recent", "k", None);
            assert_eq!(status, 200, "{how}: {body}");
            let listed: serde_json::Value = serde_json::from_str(&body).unwrap();
            let ids: Vec<u64> = listed["requests"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_u64().unwrap())
                .collect();
            assert_eq!(ids.len(), kept, "{how}: {body}");
            assert!(ids.windows(2).all(|w| w[0] > w[1]), "newest first: {ids:?}");
        }
    }

    /// The data directory and the log file are named by the environment as
    /// well as by the command line: with nothing but `RURGE_DATA_DIR` and
    /// `RURGE_LOG_FILE` set, `state.json` is made in the one and the startup
    /// line is written to the other.
    #[test]
    fn the_data_directory_and_the_log_file_can_come_from_the_environment() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(
            dir.path(),
            "http-listen = 127.0.0.1:0
socks5-listen = 127.0.0.1:0
loglevel = notify",
        );
        let data = dir.path().join("data");
        let logs = dir.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let mut cmd = rurge_process();
        cmd.arg("run")
            .arg("-c")
            .arg(&conf)
            .env("RURGE_DATA_DIR", &data)
            .env("RURGE_LOG_FILE", logs.join("rurge.log"))
            .env("RURGE_NO_NETWORK", "true")
            .env(
                "RURGE_SYSTEM_PROXY_BACKEND",
                format!("file:{}", sysproxy_file(&data).display()),
            )
            .env_remove("RURGE_SYSTEM_PROXY")
            .env_remove("RURGE_EMPTY_GROUP_REJECT")
            .env_remove("RURGE_LISTEN_HTTP")
            .env_remove("RURGE_LISTEN_SOCKS5");
        let daemon = spawn_command(cmd);
        wait_until(
            "state.json in the data directory the environment names",
            || data.join("state.json").exists(),
        );
        wait_until(
            "the startup line in the log file the environment names",
            || {
                std::fs::read_dir(&logs)
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| entry.ok())
                    .any(|entry| {
                        entry.file_name().to_string_lossy().starts_with("rurge.log")
                            && std::fs::read_to_string(entry.path())
                                .map(|text| text.contains("rurge running"))
                                .unwrap_or(false)
                    })
            },
        );
        drop(daemon);
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

    /// A profile edited into one that does not load: the reload says no and
    /// why, on standard error and in its answer, and the rules the daemon
    /// serves are the ones it had; the profile repaired is taken at the next
    /// reload.
    #[test]
    fn a_reload_of_a_profile_with_errors_keeps_the_rules_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let (daemon, errors) =
            spawn_command_reading_stderr(rurge_run(&conf, &dir.path().join("data")));
        let port = api_port(&daemon);
        let rules = || api_call(port, "GET", "/v1/rules", "k", None).1;
        assert!(rules().contains("DOMAIN,ads.test,REJECT"));
        let good = std::fs::read_to_string(&conf).unwrap();
        let broken = good.replace("DOMAIN,ads.test,REJECT", "DOMAIN,new.test,NoSuchPolicy");
        assert_ne!(good, broken);
        std::fs::write(&conf, &broken).unwrap();
        let (status, body) = api_call(port, "POST", "/v1/profiles/reload", "k", Some("{}"));
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("\"ok\":false"), "{body}");
        assert!(!body.contains("\"errors\":0"), "{body}");
        wait_for_stderr_line(&errors, "reload failed, keeping current config:");
        wait_for_stderr_line(&errors, "error[E0007]");
        let served = rules();
        assert!(
            served.contains("DOMAIN,ads.test,REJECT") && !served.contains("new.test"),
            "{served}"
        );
        let repaired = good.replace("DOMAIN,ads.test,REJECT", "DOMAIN,new.test,REJECT");
        std::fs::write(&conf, repaired).unwrap();
        let (_, body) = api_call(port, "POST", "/v1/profiles/reload", "k", Some("{}"));
        assert!(body.contains("\"ok\":true"), "{body}");
        assert!(rules().contains("DOMAIN,new.test,REJECT"));
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
[Proxy]\nUp = http, 127.0.0.1, {}\n[Rule]\nDOMAIN,via.test,Up\nFINAL,DIRECT\n[Host]\nvia.test = 127.0.0.1\n",
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

    /// A call whose body comes after the answer, as it does from a client that
    /// writes the head and the body apart (CPython's `http.client` does): the
    /// head at once, the body once the first byte of the answer is in this
    /// side's receive buffer, unread, and some time has passed. The status and
    /// the body of the answer; the connection must end with its end of stream.
    fn api_call_with_a_late_body(
        port: u16,
        method: &str,
        path: &str,
        key: &str,
        body: &str,
    ) -> (u16, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Key: {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        s.write_all(head.as_bytes()).unwrap();
        let mut first = [0u8; 1];
        match s.peek(&mut first) {
            Ok(0) => panic!("{method} {path}: the connection ended before any answer"),
            Ok(_) => {}
            Err(e) => panic!("{method} {path}: no answer began to arrive within 5 s ({e})"),
        }
        // These 200 ms are not what makes the test right: whenever the body
        // comes, the answer must be whole and the connection must end with its
        // end of stream, and a daemon that reads the body away makes it so.
        // They are what lets a daemon that does not do that tear the
        // connection down first, so that this test fails on it.
        std::thread::sleep(Duration::from_millis(200));
        let wrote = s.write_all(body.as_bytes());
        let mut raw = Vec::new();
        let ended = s.read_to_end(&mut raw);
        let out = String::from_utf8_lossy(&raw).into_owned();
        let status = out
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let answer = out
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        assert!(
            ended.is_ok(),
            "{} bytes were read, then {:?}; writing the body returned {wrote:?}",
            raw.len(),
            ended
        );
        (status, answer)
    }

    /// Reads one answer off a connection that stays open: the head, and then
    /// as many bytes of body as its `Content-Length` says.
    fn read_one_answer(s: &mut TcpStream) -> String {
        let mut raw = Vec::new();
        let mut chunk = [0u8; 512];
        loop {
            let n = s.read(&mut chunk).expect("an answer within the timeout");
            assert_ne!(n, 0, "the connection ended inside an answer: {raw:?}");
            raw.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&raw).into_owned();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let length = head
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    .expect("an answer with a Content-Length");
                if body.len() >= length {
                    return text;
                }
            }
        }
    }

    /// A client that writes the head of `POST /v1/stop` and its body apart
    /// still gets the answer, and the daemon stops.
    #[test]
    fn run_stops_for_a_client_whose_body_comes_late() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let mut daemon = spawn_daemon(&conf, &dir.path().join("data"));
        let port = api_port(&daemon);
        let (status, body) = api_call_with_a_late_body(port, "POST", "/v1/stop", "k", "{}");
        assert_eq!((status, body.as_str()), (200, "{}"));
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(0), "stop exits 0");
    }

    /// A client that has had its answer and keeps its connection open does not
    /// hold up a stop: the daemon closes the connection, waits a moment for the
    /// client to do the same, and goes. It goes because the API is done, not
    /// because the grace period of the shutdown ran out, which also ends in an
    /// exit with 0 and within these 10 s.
    #[test]
    fn run_stops_though_a_client_keeps_its_connection_open() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let mut daemon = spawn_daemon(&conf, &dir.path().join("data"));
        let port = api_port(&daemon);
        let mut idle = TcpStream::connect(("127.0.0.1", port)).unwrap();
        idle.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        idle.write_all(b"GET /v1/outbound HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Key: k\r\n\r\n")
            .unwrap();
        let answer = read_one_answer(&mut idle);
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert_eq!(api_call(port, "POST", "/v1/stop", "k", Some("{}")).0, 200);
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(0), "stop exits 0");
        let lines = printed_to_the_end(&daemon);
        assert!(
            !lines.iter().any(|l| l.contains("grace period elapsed")),
            "the stop waited out the grace period: {lines:?}"
        );
        drop(idle);
    }

    /// `rurge stop` finds the daemon through a profile that names its API,
    /// prints that the stop was requested, and the daemon stops.
    #[test]
    fn rurge_stop_stops_the_daemon_a_profile_names() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        let mut daemon = spawn_daemon(&conf, &data);
        let port = api_port(&daemon);
        // the profile the daemon runs asks for port 0, which no one can call:
        // this one names the port it got
        let pointing = dir.path().join("pointing.conf");
        std::fs::write(
            &pointing,
            format!("[General]\nhttp-api = k@127.0.0.1:{port}\n[Rule]\nFINAL,DIRECT\n"),
        )
        .unwrap();
        rurge()
            .args(["stop", "-c"])
            .arg(&pointing)
            .arg("--data-dir")
            .arg(&data)
            .assert()
            .success()
            .stdout("stop requested\n");
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(0), "stop exits 0");
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
                quoted(Path::new(rurge_process().get_program())),
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
        let mut cmd = rurge_command();
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
            .arg("--data-dir")
            .arg(dir.path().join("data"))
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
            .arg("--data-dir")
            .arg(dir.path().join("data"))
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

    /// The sources of the profile that are downloaded: the managed profile
    /// (`#!MANAGED-CONFIG`) and the files a `#!include` names by a URL. Every
    /// start here may go to the network, and everything it reaches is on the
    /// loopback.
    mod remote_sources {
        use super::*;
        use std::collections::BTreeSet;
        use std::path::PathBuf;
        use std::sync::Arc;
        use std::time::{Instant, SystemTime};
        use url::Url;

        /// What a start that has sources to wait for says first.
        const WAITING: &str = "waiting for the profile's remote sources (at most 60 s)";
        const TWO_HOURS: Duration = Duration::from_secs(2 * 3600);
        /// The log line of a refusal that begins, without its `error` field,
        /// and the one of a refusal that ends.
        const REFUSING: &str = "the update that strict requires has not succeeded";
        const ACCEPTING: &str = "managed profile: new sessions are accepted again";
        const PROXIES: &str = "[Proxy]\nRemoteP = http, 127.0.0.1, 1\n";
        const RULES: &str = "[Rule]\nDOMAIN,included.test,REJECT\n";

        /// One test's world: a data directory, the server the sources of the
        /// profile are served from and the one the GeoIP databases would be.
        struct Sources {
            dir: tempfile::TempDir,
            served: Arc<TestServer>,
            geo: TestServer,
        }

        impl Sources {
            async fn new() -> Sources {
                Sources {
                    dir: tempfile::tempdir().unwrap(),
                    served: Arc::new(TestServer::spawn().await),
                    geo: TestServer::spawn().await,
                }
            }

            fn data(&self) -> PathBuf {
                self.dir.path().join("data")
            }

            fn conf(&self) -> PathBuf {
                self.dir.path().join("t.conf")
            }

            fn url(&self, path: &str) -> Url {
                self.served.url(path)
            }

            /// The profile `rurge run` is given.
            fn write(&self, text: &str) -> PathBuf {
                std::fs::write(self.conf(), text).unwrap();
                self.conf()
            }

            /// A profile that is nothing but the line that names the managed
            /// profile at `path`.
            fn point_at(&self, path: &str) -> PathBuf {
                self.write(&format!("#!MANAGED-CONFIG {}\n", self.url(path)))
            }

            /// `body` is what the cache holds of `path`, checked `ago` ago.
            fn seed(&self, path: &str, body: &str, ago: Duration) {
                rurge_net::testing::seed_cache(
                    &self.data(),
                    &self.url(path),
                    body.as_bytes(),
                    SystemTime::now() - ago,
                );
            }

            /// The start of `rurge run` for this world.
            fn start(&self) -> Command {
                rurge_run_online(&self.conf(), &self.data(), &self.geo)
            }

            /// A profile with the remote include `/rules` in its `[Rule]`.
            fn include_rules(&self) -> PathBuf {
                self.write(&format!(
                    "[General]\n{API_GENERAL}\n[Proxy]\n[Rule]\n#!include {}\nFINAL,DIRECT\n",
                    self.url("/rules")
                ))
            }
        }

        /// A managed profile as a server gives it: it names itself as the
        /// place it is updated from, listens on ports of the system's
        /// choosing and serves the API at `api`, where the test finds it.
        fn managed_text(url: &Url, line: &str, api: u16, rules: &str) -> String {
            format!(
                "#!MANAGED-CONFIG {url} {line}\n[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nhttp-api = k@127.0.0.1:{api}\nloglevel = warning\n[Proxy]\n[Rule]\n{rules}FINAL,DIRECT\n"
            )
        }

        /// A file as it is: its bytes and when it was last written.
        fn as_it_is(path: &Path) -> (Vec<u8>, SystemTime) {
            (
                std::fs::read(path).unwrap(),
                std::fs::metadata(path).unwrap().modified().unwrap(),
            )
        }

        /// The lines of the daemon's standard error up to and with the first
        /// one that has `needle` in it.
        fn stderr_up_to(lines: &mpsc::Receiver<String>, needle: &str) -> Vec<String> {
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut seen = Vec::new();
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let line = lines.recv_timeout(remaining).unwrap_or_else(|_| {
                    panic!("rurge run never wrote a line with `{needle}`; it wrote {seen:?}")
                });
                let found = line.contains(needle);
                seen.push(line);
                if found {
                    return seen;
                }
            }
        }

        /// What is left of a stream of a process that has ended, to its last
        /// line: the pipe closed with the process, and the thread that reads
        /// it is at its end or will be in a moment.
        fn the_rest(lines: &mpsc::Receiver<String>) -> String {
            let mut text = String::new();
            while let Ok(line) = lines.recv_timeout(Duration::from_secs(2)) {
                text += &line;
                text.push('\n');
            }
            text
        }

        /// A place for a session to go: a server, and the address of a page
        /// it has.
        async fn a_target() -> (TestServer, String) {
            let target = TestServer::spawn().await;
            target.set("/hello", "hi from target");
            let hello = format!("http://127.0.0.1:{}/hello", target.url("/").port().unwrap());
            (target, hello)
        }

        /// A start that ends at once, with all it said.
        struct Gave {
            code: Option<i32>,
            stdout: String,
            stderr: String,
        }

        /// `give_up`, with standard output as well.
        fn gives_up(mut cmd: Command) -> Gave {
            let start = describe_start(&cmd);
            let mut child = cmd
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let stdout = forward_lines(child.stdout.take().unwrap());
            let stderr = forward_lines(child.stderr.take().unwrap());
            let deadline = Instant::now() + Duration::from_secs(20);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("rurge run did not give up within 20 s\n  the start: {start}");
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            Gave {
                code: status.code(),
                stdout: the_rest(&stdout),
                stderr: the_rest(&stderr),
            }
        }

        /// Waits until the daemon says it is running, which is when its API
        /// is up and the loop that handles what the API asks for has begun.
        fn up(daemon: &Daemon) {
            wait_for_line(daemon, "api on http://");
            wait_for_line(daemon, "rurge ");
        }

        /// `up`, and where the API is.
        fn api_up(daemon: &Daemon) -> u16 {
            let api = api_port(daemon);
            wait_for_line(daemon, "rurge ");
            api
        }

        /// Stops the daemon through its API and waits for it to end.
        fn stop(daemon: &mut Daemon, api: u16) {
            assert_eq!(api_call(api, "POST", "/v1/stop", "k", Some("{}")).0, 200);
            assert_eq!(wait_for_exit(daemon, 10), Some(0), "stop exits 0");
        }

        /// Polls until `check` passes, for `secs` at the most.
        fn wait_for(what: &str, secs: u64, mut check: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(secs);
            while !check() {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        /// Has the daemon write a log file in `logs`.
        fn log_to_file(cmd: &mut Command, logs: &Path) {
            std::fs::create_dir_all(logs).unwrap();
            cmd.arg("--log-file").arg(logs.join("rurge.log"));
        }

        /// `log_to_file`, at a level that shows what the sources ask of the
        /// engine as it happens (`notify` is INFO).
        fn log_to(cmd: &mut Command, logs: &Path) {
            cmd.args(["--log-level", "notify"]);
            log_to_file(cmd, logs);
        }

        /// Everything the daemon has written to its log file so far.
        fn log_text(logs: &Path) -> String {
            let mut text = String::new();
            for entry in std::fs::read_dir(logs).into_iter().flatten().flatten() {
                if entry.file_name().to_string_lossy().starts_with("rurge.log") {
                    text += &std::fs::read_to_string(entry.path()).unwrap_or_default();
                }
            }
            text
        }

        fn lines_with(text: &str, needle: &str) -> Vec<String> {
            text.lines()
                .filter(|line| line.contains(needle))
                .map(str::to_string)
                .collect()
        }

        /// The paths a server was asked for.
        fn asked_for(server: &TestServer) -> BTreeSet<String> {
            server.requests().into_iter().map(|r| r.path).collect()
        }

        fn current(api: u16) -> String {
            api_call(api, "GET", "/v1/profiles/current?sensitive=1", "k", None).1
        }

        fn rules(api: u16) -> String {
            api_call(api, "GET", "/v1/rules", "k", None).1
        }

        // ---------------------------------------------------------------
        // the start waits for what the profile lacks

        /// A cache that lacks two remote includes: the start asks for both
        /// before it builds anything, says so, and runs with what they
        /// bring. The profile is not written to.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_start_downloads_the_remote_includes_the_cache_lacks() {
            let w = Sources::new().await;
            w.served.set("/proxies", PROXIES);
            w.served.set("/rules", RULES);
            let conf = w.write(&format!(
                "[General]\n{API_GENERAL}\n[Proxy]\n#!include {}\n[Rule]\n#!include {}\nFINAL,DIRECT\n",
                w.url("/proxies"),
                w.url("/rules")
            ));
            let before = as_it_is(&conf);
            let (mut daemon, errors) = spawn_command_reading_stderr(w.start());
            // The load that found the sources missing says nothing: what the
            // start says of the profile it says of the one it goes on with.
            let said = stderr_up_to(&errors, WAITING);
            assert_eq!(said, [WAITING], "{said:?}");
            let api = api_up(&daemon);
            let (_, policies) = api_call(api, "GET", "/v1/policies", "k", None);
            assert!(policies.contains("RemoteP"), "{policies}");
            assert!(rules(api).contains("DOMAIN,included.test,REJECT"));
            for (path, body) in [("/proxies", PROXIES), ("/rules", RULES)] {
                let copy = rurge_net::resource::cached_copy(&w.data(), &w.url(path))
                    .unwrap_or_else(|| panic!("the cache has no copy of {path}"));
                assert_eq!(&copy.data[..], body.as_bytes(), "{path}");
            }
            stop(&mut daemon, api);
            assert_eq!(as_it_is(&conf), before, "the profile was written to");
            // all that an online start reaches: the two sources, and the two
            // GeoIP databases, which it asks the loopback for
            let sources: BTreeSet<String> = ["/proxies", "/rules"]
                .into_iter()
                .map(String::from)
                .collect();
            let databases: BTreeSet<String> = [COUNTRY_DATABASE, ASN_DATABASE]
                .into_iter()
                .map(String::from)
                .collect();
            assert!(asked_for(&w.served).is_subset(&sources));
            assert!(asked_for(&w.geo).is_subset(&databases));
        }

        /// A profile that is nothing but the line that names the managed
        /// profile runs on the copy it downloads. What the daemon runs on is
        /// what the commands that read the profile see, through the data
        /// directory; the file is not written to.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_profile_that_is_only_a_pointer_runs_on_the_downloaded_copy() {
            let w = Sources::new().await;
            let api = free_tcp_port();
            let copy = managed_text(
                &w.url("/managed"),
                "interval=3600",
                api,
                "DOMAIN,copy.test,REJECT\n",
            );
            w.served.set("/managed", copy.clone());
            let conf = w.point_at("/managed");
            let before = as_it_is(&conf);
            let (mut daemon, errors) = spawn_command_reading_stderr(w.start());
            stderr_up_to(&errors, WAITING);
            up(&daemon);
            let (status, text) =
                api_call(api, "GET", "/v1/profiles/current?sensitive=1", "k", None);
            assert_eq!((status, text.as_str()), (200, copy.as_str()));
            assert!(rules(api).contains("DOMAIN,copy.test,REJECT"));
            // checked as a reload would check it
            let (status, body) = api_call(api, "POST", "/v1/profiles/check", "k", Some("{}"));
            assert_eq!(status, 200, "{body}");
            assert!(
                body.contains("\"ok\":true") && !body.contains("E0010"),
                "{body}"
            );
            // the commands that read the profile find the copy in the data directory
            rurge()
                .arg("check")
                .arg("-c")
                .arg(&conf)
                .arg("--data-dir")
                .arg(w.data())
                .assert()
                .success()
                .stdout(predicate::str::contains("I0004"));
            rurge()
                .args(["status", "-c"])
                .arg(&conf)
                .arg("--data-dir")
                .arg(w.data())
                .assert()
                .success()
                .stdout(predicate::str::contains(format!(
                    "rurge at http://127.0.0.1:{api}"
                )));
            stop(&mut daemon, api);
            assert_eq!(as_it_is(&conf), before, "the profile was written to");
        }

        /// What a power cut can leave of the copy is not the copy. The next
        /// start does not read it and asks the server for the whole of it:
        /// with the validators of the copy that was downloaded, the answer
        /// would be that nothing has changed, and the start would end on a
        /// profile that does not load for as long as the server keeps its
        /// version.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_copy_that_is_not_what_was_downloaded_is_downloaded_again() {
            let w = Sources::new().await;
            let api = free_tcp_port();
            let copy = managed_text(
                &w.url("/managed"),
                "interval=3600",
                api,
                "DOMAIN,copy.test,REJECT\n",
            );
            w.served.set("/managed", copy.clone());
            w.point_at("/managed");
            let (mut daemon, errors) = spawn_command_reading_stderr(w.start());
            stderr_up_to(&errors, WAITING);
            up(&daemon);
            stop(&mut daemon, api);
            let file = rurge_net::resource::cache_file(&w.data(), &w.url("/managed"));
            assert_eq!(std::fs::read(&file).unwrap(), copy.as_bytes());
            // beside it, the validator the server gave with it
            let meta = std::fs::read_to_string(file.with_file_name("meta.json")).unwrap();
            assert!(!meta.contains("\"etag\": null"), "{meta}");
            let asked = w.served.requests().len();

            // the first half of the body where the body was
            std::fs::write(&file, &copy.as_bytes()[..copy.len() / 2]).unwrap();
            let (mut daemon, errors) = spawn_command_reading_stderr(w.start());
            stderr_up_to(&errors, WAITING);
            up(&daemon);
            assert_eq!(current(api), copy);
            assert!(rules(api).contains("DOMAIN,copy.test,REJECT"));
            let again = &w.served.requests()[asked..];
            assert!(!again.is_empty(), "the server was not asked");
            assert!(again[0].header("if-none-match").is_none(), "{again:?}");
            assert_eq!(std::fs::read(&file).unwrap(), copy.as_bytes());
            stop(&mut daemon, api);
        }

        /// The managed profile cannot be had and there is nothing to run
        /// on: the start ends with the profile's own diagnostics, which say
        /// why. The link is a credential as a whole; only where it goes is
        /// shown.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_profile_that_is_only_a_pointer_does_not_start_when_its_source_fails() {
            let w = Sources::new().await;
            let port = w.url("/").port().unwrap();
            let link = Url::parse(&format!(
                "http://USERMARK:PASSMARK@127.0.0.1:{port}/PATHMARK/managed.conf?token=QUERYMARK"
            ))
            .unwrap();
            w.served
                .set("/PATHMARK/managed.conf", "[Rule]\nFINAL,DIRECT\n");
            w.served.set_status("/PATHMARK/managed.conf", 500);
            w.write(&format!("#!MANAGED-CONFIG {link}\n"));
            let gave = gives_up(w.start());
            assert_eq!(gave.code, Some(2), "{}", gave.stderr);
            assert!(gave.stderr.contains(WAITING), "{}", gave.stderr);
            // said once, by the load the start ends with
            assert_eq!(gave.stderr.matches("W0022").count(), 1, "{}", gave.stderr);
            assert!(gave.stderr.contains("http status 500"), "{}", gave.stderr);
            assert!(w.served.hits("/PATHMARK/managed.conf") >= 1);
            for marker in ["USERMARK", "PASSMARK", "PATHMARK", "QUERYMARK"] {
                assert!(!gave.stdout.contains(marker), "{marker}: {}", gave.stdout);
                assert!(!gave.stderr.contains(marker), "{marker}: {}", gave.stderr);
            }
        }

        /// One run of the daemon over the sources of `w`, at the most detailed
        /// log level: all it says and all it gives, and what the commands that
        /// read the profile say of it. The run is stopped when `until` holds of
        /// its log.
        fn everything_a_run_shows(
            w: &Sources,
            conf: &Path,
            api: u16,
            run: u32,
            until: impl Fn(&str) -> bool,
        ) -> String {
            let logs = w.dir.path().join(format!("logs-{run}"));
            let mut cmd = w.start();
            cmd.args(["--log-level", "verbose"]);
            log_to_file(&mut cmd, &logs);
            let (mut daemon, errors) = spawn_command_reading_stderr(cmd);
            up(&daemon);
            wait_for("the log to say what the run waits for", 30, || {
                until(&log_text(&logs))
            });
            let mut shown = String::new();
            for (method, path, body) in [
                ("GET", "/v1/profiles/current", None),
                ("GET", "/v1/rules", None),
                ("POST", "/v1/profiles/check", Some("{}")),
            ] {
                shown += &api_call(api, method, path, "k", body).1;
            }
            for command in [
                vec!["check", "-c"],
                vec!["status", "-c"],
                vec!["rule", "match", "first.test", "--no-network", "-c"],
            ] {
                let mut cmd = rurge();
                cmd.args(&command).arg(conf).arg("--data-dir").arg(w.data());
                let out = cmd.output().unwrap();
                shown += &String::from_utf8_lossy(&out.stdout);
                shown += &String::from_utf8_lossy(&out.stderr);
            }
            stop(&mut daemon, api);
            shown += &the_rest(&errors);
            shown += &the_rest(&daemon.lines);
            shown + &log_text(&logs)
        }

        /// When the link works, it is still shown as where it goes and
        /// nothing more: not in the daemon's log at its most detailed, not in
        /// what it prints, not in what its API gives, not in the commands
        /// that read the profile. The first run downloads the profile before
        /// it has a log, the second finds the copy old and downloads a new
        /// version while it runs.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn the_link_to_the_managed_profile_is_shown_as_where_it_goes_and_nothing_more() {
            let w = Sources::new().await;
            let api = free_tcp_port();
            let port = w.url("/").port().unwrap();
            let link = Url::parse(&format!(
                "http://USERMARK:PASSMARK@127.0.0.1:{port}/PATHMARK/managed.conf?token=QUERYMARK"
            ))
            .unwrap();
            let text = |rules: &str| managed_text(&link, "interval=3600", api, rules);
            let conf = w.write(&format!("#!MANAGED-CONFIG {link}\n"));
            w.served
                .set("/PATHMARK/managed.conf", text("DOMAIN,first.test,REJECT\n"));
            let mut shown = everything_a_run_shows(&w, &conf, api, 1, |_| true);
            rurge_net::testing::seed_cache(
                &w.data(),
                &link,
                text("DOMAIN,old.test,REJECT\n").as_bytes(),
                SystemTime::now() - TWO_HOURS,
            );
            w.served.set(
                "/PATHMARK/managed.conf",
                text("DOMAIN,second.test,REJECT\n"),
            );
            shown += &everything_a_run_shows(&w, &conf, api, 2, |log| {
                log.contains("resource updated") && log.contains("profile reloaded")
            });
            // the second run says what it did with the link, and where it goes
            assert!(
                shown.contains(&format!("managed profile (http://127.0.0.1:{port}/…)")),
                "{shown}"
            );
            for marker in ["USERMARK", "PASSMARK", "PATHMARK", "QUERYMARK"] {
                assert!(!shown.contains(marker), "{marker}: {shown}");
            }
        }

        /// A profile that is whole in itself runs on its own text when its
        /// managed profile cannot be had; the reason is a warning.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_whole_profile_runs_on_its_own_text_when_its_source_fails() {
            let w = Sources::new().await;
            let api = free_tcp_port();
            let local = format!(
                "#!MANAGED-CONFIG {} interval=3600\n[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nhttp-api = k@127.0.0.1:{api}\nloglevel = warning\n[Proxy]\n[Rule]\nDOMAIN,local.test,REJECT\nFINAL,DIRECT\n",
                w.url("/managed")
            );
            w.served.set("/managed", "[Rule]\nFINAL,DIRECT\n");
            w.served.set_status("/managed", 500);
            w.write(&local);
            let (mut daemon, errors) = spawn_command_reading_stderr(w.start());
            let said = stderr_up_to(&errors, "W0022");
            assert!(said.iter().any(|line| line == WAITING), "{said:?}");
            assert!(
                said.last()
                    .is_some_and(|line| line.contains("http status 500")),
                "{said:?}"
            );
            up(&daemon);
            assert_eq!(current(api), local);
            assert!(rules(api).contains("DOMAIN,local.test,REJECT"));
            stop(&mut daemon, api);
        }

        /// What a running daemon reads the profile through knows why a source
        /// could not be had: a check through the API, which reads as a reload
        /// would, says it. The disk alone only knows that there is no copy.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_check_through_the_api_says_why_a_source_cannot_be_had() {
            let w = Sources::new().await;
            w.served.set("/rules", RULES);
            w.served.set_status("/rules", 500);
            w.include_rules();
            let (mut daemon, errors) = spawn_command_reading_stderr(w.start());
            stderr_up_to(&errors, "W0022");
            let api = api_up(&daemon);
            // The attempt the start waited for was made before the daemon's
            // own sources existed; it makes its own once it follows them.
            let mut said = String::new();
            wait_for("the daemon's own attempt to fail", 20, || {
                let (status, body) = api_call(api, "POST", "/v1/profiles/check", "k", Some("{}"));
                assert_eq!(status, 200, "{body}");
                said = body;
                said.contains("http status 500")
            });
            assert!(said.contains("W0022"), "{said}");
            stop(&mut daemon, api);
        }

        /// Without the network nothing is waited for or asked: what the cache
        /// lacks is a warning, as ever.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn offline_a_remote_include_the_cache_lacks_is_a_warning_and_nothing_is_waited_for() {
            let w = Sources::new().await;
            w.served.set("/rules", RULES);
            w.include_rules();
            let (mut daemon, errors) =
                spawn_command_reading_stderr(rurge_run(&w.conf(), &w.data()));
            let said = stderr_up_to(&errors, "W0022");
            assert!(
                said.iter().all(|line| !line.contains("waiting for")),
                "{said:?}"
            );
            assert!(
                said.last()
                    .is_some_and(|line| line.contains("remote include not downloaded yet")),
                "{said:?}"
            );
            let api = api_up(&daemon);
            assert!(!rules(api).contains("included.test"));
            stop(&mut daemon, api);
            assert_eq!(w.served.requests().len(), 0, "offline asks nobody");
        }

        /// An include that is slow to answer is waited for: the start does not
        /// come up with a warning that it is not downloaded yet.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_start_waits_for_an_include_that_is_slow_to_answer() {
            let w = Sources::new().await;
            w.served.set("/rules", RULES);
            w.served.set_delay("/rules", Duration::from_secs(3));
            w.include_rules();
            let begun = Instant::now();
            let (mut daemon, _errors) = spawn_command_reading_stderr(w.start());
            assert!(
                begun.elapsed() >= Duration::from_millis(2900),
                "it came up after {:?}",
                begun.elapsed()
            );
            let api = api_up(&daemon);
            assert!(rules(api).contains("DOMAIN,included.test,REJECT"));
            stop(&mut daemon, api);
        }

        /// The lock is tried before there is a log. When none can be had the
        /// start goes on without one, as it does when it has nothing to wait
        /// for, and says so once the log exists.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_start_that_cannot_lock_the_data_directory_says_so_once_it_has_a_log() {
            let w = Sources::new().await;
            // where the lock file would be there is a directory, which cannot be opened as one
            std::fs::create_dir_all(w.data().join("rurge.lock")).unwrap();
            w.served.set("/rules", RULES);
            w.include_rules();
            let logs = w.dir.path().join("logs");
            let mut cmd = w.start();
            log_to(&mut cmd, &logs);
            let (mut daemon, errors) = spawn_command_reading_stderr(cmd);
            stderr_up_to(&errors, WAITING);
            let api = api_up(&daemon);
            assert!(rules(api).contains("DOMAIN,included.test,REJECT"));
            stop(&mut daemon, api);
            let log = log_text(&logs);
            let said = lines_with(&log, "the data directory is not locked");
            assert_eq!(said.len(), 1, "{log}");
            assert!(said[0].contains(" WARN "), "{said:?}");
        }

        // ---------------------------------------------------------------
        // a start that ends before it has a log

        /// A profile that still does not load once the sources have been
        /// tried ends the start as ever, and puts back what a crashed run
        /// left behind, with the lock that was taken for the downloads.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_profile_that_does_not_load_after_the_wait_restores_a_crashed_system_proxy() {
            let w = Sources::new().await;
            let data = crashed_with_the_system_proxy_on(w.dir.path());
            w.served.set("/managed", "[Rule]\nFINAL,DIRECT\n");
            w.served.set_status("/managed", 500);
            w.point_at("/managed");
            let gave = gives_up(w.start());
            assert_eq!(gave.code, Some(2), "{}", gave.stderr);
            assert!(gave.stderr.contains(WAITING), "{}", gave.stderr);
            assert!(gave.stderr.contains("http status 500"), "{}", gave.stderr);
            assert_restored(&data, &gave.stderr);
        }

        /// The downloads write to the cache, which one instance at a time
        /// may: the data directory is locked before anything is asked for.
        /// The instance that holds it has the system proxy, and the backup
        /// in the directory is its own: the start that is refused leaves
        /// both as they are.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_start_that_must_wait_refuses_a_data_directory_another_instance_holds() {
            let w = Sources::new().await;
            let first = w.dir.path().join("first.conf");
            std::fs::write(
                &first,
                "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning\n[Rule]\nFINAL,DIRECT\n",
            )
            .unwrap();
            std::fs::create_dir_all(w.data()).unwrap();
            let proxy = sysproxy_file(&w.data());
            std::fs::write(&proxy, ORIGINAL_PROXY).unwrap();
            let holder = spawn_daemon_full(&first, &w.data(), false, None, &["--system-proxy"]);
            wait_for_line(&holder, "system proxy enabled: ");
            // up: a start writes `state.json` once more on its way to saying so
            wait_for_line(&holder, "rurge ");
            let applied = std::fs::read_to_string(&proxy).unwrap();
            assert_ne!(applied, ORIGINAL_PROXY, "the holder owns the settings");
            let state = std::fs::read_to_string(w.data().join("state.json")).unwrap();
            w.served.set("/managed", "[Rule]\nFINAL,DIRECT\n");
            w.point_at("/managed");
            let gave = gives_up(w.start());
            assert_eq!(gave.code, Some(1), "{}", gave.stderr);
            assert!(
                gave.stderr.contains("another rurge instance"),
                "{}",
                gave.stderr
            );
            assert!(!gave.stderr.contains(WAITING), "{}", gave.stderr);
            assert_eq!(
                w.served.requests().len(),
                0,
                "nothing is asked for before the lock is had"
            );
            assert!(!gave.stderr.contains("system proxy"), "{}", gave.stderr);
            assert_eq!(std::fs::read_to_string(&proxy).unwrap(), applied);
            assert_eq!(
                std::fs::read_to_string(w.data().join("state.json")).unwrap(),
                state,
                "the holder's backup is still there for it to restore"
            );
        }

        /// An override that cannot be applied ends the start as ever, once the
        /// sources have been waited for: it is applied to the profile the
        /// start goes on with and to no other, so the wait is made. What a
        /// crashed run left behind is put back.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn an_override_that_cannot_be_applied_after_the_wait_restores_a_crashed_system_proxy()
        {
            let w = Sources::new().await;
            let data = crashed_with_the_system_proxy_on(w.dir.path());
            let port = free_tcp_port();
            w.served.set("/rules", RULES);
            w.write(&format!(
                "[General]\n{API_GENERAL}\n[Proxy]\nX = external, exec = /no/such/program, local-port = {port}\n[Rule]\n#!include {}\nFINAL,DIRECT\n",
                w.url("/rules")
            ));
            let mut cmd = w.start();
            cmd.args(["--listen-socks5", &format!("127.0.0.1:{port}")]);
            let gave = gives_up(cmd);
            assert_eq!(gave.code, Some(2), "{}", gave.stderr);
            assert!(gave.stderr.contains(WAITING), "{}", gave.stderr);
            assert!(
                gave.stderr.contains(&format!(
                    "error: --listen-socks5: port {port} is the `local-port` of an `external` policy\n"
                )),
                "{}",
                gave.stderr
            );
            assert!(w.served.hits("/rules") >= 1, "the include was downloaded");
            assert_restored(&data, &gave.stderr);
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_log_file_that_cannot_be_opened_after_the_wait_restores_a_crashed_system_proxy() {
            let w = Sources::new().await;
            let data = crashed_with_the_system_proxy_on(w.dir.path());
            w.served.set("/rules", RULES);
            w.include_rules();
            let mut cmd = w.start();
            cmd.arg("--log-file")
                .arg(log_file_that_cannot_be_opened(w.dir.path()));
            let gave = gives_up(cmd);
            assert_eq!(gave.code, Some(2), "{}", gave.stderr);
            assert!(gave.stderr.contains(WAITING), "{}", gave.stderr);
            assert!(
                gave.stderr.contains("error: cannot open log file"),
                "{}",
                gave.stderr
            );
            assert!(w.served.hits("/rules") >= 1, "the include was downloaded");
            assert_restored(&data, &gave.stderr);
        }

        /// The commands that talk to the daemon find it where the profile says,
        /// and a managed profile says so in its copy, which is in the data
        /// directory. No daemon is there to answer: the address they tried is
        /// the copy's.
        #[test]
        fn the_commands_that_talk_to_the_daemon_read_its_address_from_the_copy() {
            let dir = tempfile::tempdir().unwrap();
            let link = Url::parse("https://sources.example/profile.conf").unwrap();
            let data = dir.path().join("data");
            let api = free_tcp_port();
            rurge_net::testing::seed_cache(
                &data,
                &link,
                managed_text(&link, "interval=3600", api, "").as_bytes(),
                SystemTime::now() - TWO_HOURS,
            );
            let pointer = dir.path().join("managed.conf");
            std::fs::write(&pointer, format!("#!MANAGED-CONFIG {link}\n")).unwrap();
            for command in ["status", "reload", "stop"] {
                rurge()
                    .args([command, "-c"])
                    .arg(&pointer)
                    .arg("--data-dir")
                    .arg(&data)
                    .assert()
                    .code(1)
                    .stderr(predicate::str::contains(format!(
                        "cannot reach rurge at http://127.0.0.1:{api}"
                    )));
            }
            // without the copy there is no address to find, and no directory is made
            let nothing = dir.path().join("nothing");
            rurge()
                .args(["status", "-c"])
                .arg(&pointer)
                .arg("--data-dir")
                .arg(&nothing)
                .assert()
                .code(2)
                .stderr(predicate::str::contains("http-api is not configured"));
            assert!(!nothing.exists());
        }

        /// A mistake in the user's own file is the user's to mend: no update
        /// is fetched to cover it, and the start says which text is in
        /// effect.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn the_users_own_mistake_ends_the_start_without_an_update_to_cover_it() {
            let w = Sources::new().await;
            let copy = managed_text(&w.url("/managed"), "interval=3600", free_tcp_port(), "");
            w.seed("/managed", &copy, TWO_HOURS);
            w.served.set("/managed", copy);
            // written after the copy was checked, and with a rule for a policy
            // that nothing defines
            w.write(&format!(
                "#!MANAGED-CONFIG {} interval=3600\n[General]\n[Rule]\nDOMAIN,a.test,NoSuchPolicy\nFINAL,DIRECT\n",
                w.url("/managed")
            ));
            let gave = gives_up(w.start());
            assert_eq!(gave.code, Some(2), "{}", gave.stderr);
            assert!(gave.stderr.contains("E0007"), "{}", gave.stderr);
            assert!(gave.stderr.contains("I0004"), "{}", gave.stderr);
            assert!(!gave.stderr.contains(WAITING), "{}", gave.stderr);
            assert_eq!(w.served.requests().len(), 0, "no update was asked for");
        }

        // ---------------------------------------------------------------
        // strict

        /// The update `strict` requires is not made by the time the daemon
        /// comes up (the server takes three seconds to say no): it refuses
        /// sessions from the first one, with the reason the wait ended with,
        /// and says so once. The API goes on as ever. The update the daemon
        /// makes on its own fails as well, and that is no news.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_start_that_could_not_make_a_strict_update_refuses_sessions_from_the_first_one() {
            let w = Sources::new().await;
            let (target, hello) = a_target().await;
            let api = free_tcp_port();
            let copy = managed_text(&w.url("/managed"), "interval=60 strict=true", api, "");
            w.seed("/managed", &copy, TWO_HOURS);
            w.served.set("/managed", copy);
            w.served.set_status("/managed", 500);
            w.served.set_delay("/managed", Duration::from_secs(3));
            w.point_at("/managed");
            let logs = w.dir.path().join("logs");
            let mut cmd = w.start();
            log_to(&mut cmd, &logs);
            let (daemon, _errors) = spawn_command_reading_stderr(cmd);
            // the first session, right after the listening lines
            let refused = http_get(daemon.http, &hello);
            assert!(refused.starts_with("HTTP/1.1 502"), "{refused}");
            assert!(refused.contains(rurge_engine::REFUSED_STRICT), "{refused}");
            up(&daemon);
            assert_eq!(
                api_call(api, "GET", "/v1/outbound", "k", None),
                (200, r#"{"mode":"rule"}"#.to_string())
            );
            // the daemon's own attempt fails three seconds after it began
            wait_for("the daemon's own attempt to fail", 20, || {
                lines_with(&log_text(&logs), "resource fetch failed")
                    .iter()
                    .any(|line| line.contains("managed profile"))
            });
            // The sources are looked at once a second. Two seconds and a half
            // are two looks and a half, at which a second line would have
            // been written.
            std::thread::sleep(Duration::from_millis(2500));
            let log = log_text(&logs);
            let refusals = lines_with(&log, REFUSING);
            assert_eq!(refusals.len(), 1, "{log}");
            assert!(refusals[0].contains("http status 500"), "{refusals:?}");
            assert!(lines_with(&log, ACCEPTING).is_empty(), "{log}");
            assert!(
                target.requests().is_empty(),
                "no session reached the target"
            );
        }

        /// Offline a strict update that is due cannot be made: sessions are
        /// refused from the first one, and nobody is asked.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn offline_a_strict_update_that_is_due_refuses_sessions_from_the_first_one() {
            let w = Sources::new().await;
            let (target, hello) = a_target().await;
            let api = free_tcp_port();
            let copy = managed_text(&w.url("/managed"), "interval=60 strict=true", api, "");
            w.seed("/managed", &copy, TWO_HOURS);
            w.served.set("/managed", copy);
            w.point_at("/managed");
            let logs = w.dir.path().join("logs");
            let mut cmd = rurge_run(&w.conf(), &w.data());
            log_to(&mut cmd, &logs);
            let (mut daemon, _errors) = spawn_command_reading_stderr(cmd);
            let refused = http_get(daemon.http, &hello);
            assert!(refused.starts_with("HTTP/1.1 502"), "{refused}");
            assert!(refused.contains(rurge_engine::REFUSED_STRICT), "{refused}");
            up(&daemon);
            assert_eq!(
                api_call(api, "GET", "/v1/outbound", "k", None),
                (200, r#"{"mode":"rule"}"#.to_string())
            );
            // The listing says why, although no update has failed: offline
            // none is made.
            let item = managed_item(api);
            assert_eq!(item["suspended"], true, "{item}");
            assert_eq!(
                item["error"],
                "offline mode: the update cannot be attempted"
            );
            wait_for("the refusal to be logged", 10, || {
                !lines_with(&log_text(&logs), REFUSING).is_empty()
            });
            // The sources are looked at as the loop begins and once a second
            // after: a second and a half is one look more, at which a second
            // line would have been written.
            std::thread::sleep(Duration::from_millis(1500));
            let refusals = lines_with(&log_text(&logs), REFUSING);
            assert_eq!(refusals.len(), 1, "{refusals:?}");
            assert!(refusals[0].contains("offline mode"), "{refusals:?}");
            stop(&mut daemon, api);
            assert!(target.requests().is_empty());
            assert_eq!(w.served.requests().len(), 0, "offline asks nobody");
        }

        /// The way out the user has: the update that `strict` requires is the
        /// one the line of the local file asks for while that file is in
        /// effect, and removing `strict` from it ends the refusal at the
        /// reload, which an edit that keeps it does not. Putting it back
        /// begins one, in a daemon that was accepting sessions: the update
        /// is as overdue as it was.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn strict_in_the_local_file_ends_a_refusal_and_begins_one_at_the_reload() {
            let w = Sources::new().await;
            let (_target, hello) = a_target().await;
            let api = free_tcp_port();
            let copy = managed_text(&w.url("/managed"), "interval=60 strict=true", api, "");
            w.seed("/managed", &copy, TWO_HOURS);
            w.served.set("/managed", copy);
            w.served.set_status("/managed", 500);
            let local = |line: &str| {
                format!(
                    "#!MANAGED-CONFIG {} {line}\n[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nhttp-api = k@127.0.0.1:{api}\nloglevel = warning\n[Proxy]\n[Rule]\nFINAL,DIRECT\n",
                    w.url("/managed")
                )
            };
            // a whole profile written after the copy was checked, whose update
            // is overdue and fails
            w.write(&local("interval=60 strict=true"));
            let logs = w.dir.path().join("logs");
            let mut cmd = w.start();
            log_to(&mut cmd, &logs);
            let (mut daemon, _errors) = spawn_command_reading_stderr(cmd);
            up(&daemon);
            let refused = http_get(daemon.http, &hello);
            assert!(refused.contains(rurge_engine::REFUSED_STRICT), "{refused}");
            let reload = || {
                let (status, body) = api_call(api, "POST", "/v1/profiles/reload", "k", Some("{}"));
                assert!(status == 200 && body.contains("\"ok\":true"), "{body}");
            };
            // an edit that keeps `strict` leaves the refusal: a look is made
            // every second, and two of them are the margin
            w.write(&local("interval=60 strict=true"));
            reload();
            std::thread::sleep(Duration::from_millis(2100));
            let refused = http_get(daemon.http, &hello);
            assert!(refused.contains(rurge_engine::REFUSED_STRICT), "{refused}");
            // and the one that removes it ends it
            w.write(&local("interval=60"));
            reload();
            wait_for("sessions to be accepted again", 10, || {
                http_get(daemon.http, &hello).ends_with("hi from target")
            });
            wait_for("the line that says so", 10, || {
                lines_with(&log_text(&logs), ACCEPTING).len() == 1
            });
            let log = log_text(&logs);
            assert_eq!(lines_with(&log, REFUSING).len(), 1, "{log}");
            // and the one that puts it back begins a refusal, which is said again
            w.write(&local("interval=60 strict=true"));
            reload();
            wait_for("sessions to be refused again", 10, || {
                http_get(daemon.http, &hello).contains(rurge_engine::REFUSED_STRICT)
            });
            wait_for("the line that says so", 10, || {
                lines_with(&log_text(&logs), REFUSING).len() == 2
            });
            let log = log_text(&logs);
            let refusals = lines_with(&log, REFUSING);
            assert!(
                refusals.iter().all(|line| line.contains("http status 500")),
                "{refusals:?}"
            );
            assert_eq!(lines_with(&log, ACCEPTING).len(), 1, "{log}");
            stop(&mut daemon, api);
        }

        /// The update falls due while the daemon runs, and fails: from then
        /// on sessions are refused, which the log says once. Until then they
        /// were accepted: the copy was not an interval old when the daemon
        /// came up.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_refusal_begins_when_the_update_falls_due_and_fails_while_the_daemon_runs() {
            let w = Sources::new().await;
            let (_target, hello) = a_target().await;
            let api = free_tcp_port();
            let copy = managed_text(&w.url("/managed"), "interval=60 strict=true", api, "");
            // Checked half a minute ago and due at a minute: the daemon has
            // half a minute to come up and serve a session before the update
            // is owed, which is wide for a machine that starts every daemon
            // of this module at once. The update is made later than that by
            // up to a tenth of the time that was left: three seconds.
            w.seed("/managed", &copy, Duration::from_secs(30));
            w.served.set("/managed", copy);
            w.served.set_status("/managed", 500);
            w.point_at("/managed");
            let logs = w.dir.path().join("logs");
            let mut cmd = w.start();
            log_to(&mut cmd, &logs);
            let (mut daemon, _errors) = spawn_command_reading_stderr(cmd);
            let first = http_get(daemon.http, &hello);
            assert!(
                first.ends_with("hi from target"),
                "the first session did not pass: {first}"
            );
            up(&daemon);
            wait_for("sessions to be refused", 60, || {
                http_get(daemon.http, &hello).contains(rurge_engine::REFUSED_STRICT)
            });
            wait_for("the line that says so", 10, || {
                !lines_with(&log_text(&logs), REFUSING).is_empty()
            });
            let log = log_text(&logs);
            let refusals = lines_with(&log, REFUSING);
            assert_eq!(refusals.len(), 1, "{log}");
            assert!(refusals[0].contains("http status 500"), "{refusals:?}");
            assert!(lines_with(&log, ACCEPTING).is_empty(), "{log}");
            stop(&mut daemon, api);
        }

        /// A refusal that began because the update did not finish ends with
        /// the next update that works, without a restart, and says so once.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_refusal_ends_when_the_next_update_works() {
            let w = Sources::new().await;
            let (_target, hello) = a_target().await;
            let api = free_tcp_port();
            let line = "interval=60 strict=true";
            let old = managed_text(&w.url("/managed"), line, api, "");
            let new = managed_text(&w.url("/managed"), line, api, "DOMAIN,new.test,REJECT\n");
            w.seed("/managed", &old, TWO_HOURS);
            w.served.set("/managed", old);
            w.served.set_status("/managed", 500);
            w.served.set_delay("/managed", Duration::from_secs(3));
            w.point_at("/managed");
            // The first request, the one the start makes, is on its way with
            // the answer it was given. Those after it are told that the
            // update works, three seconds later.
            let served = w.served.clone();
            let flip = tokio::spawn(async move {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
                while served.hits("/managed") == 0 && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                served.set("/managed", new);
                served.set_status("/managed", 200);
            });
            let logs = w.dir.path().join("logs");
            let mut cmd = w.start();
            log_to(&mut cmd, &logs);
            let (mut daemon, _errors) = spawn_command_reading_stderr(cmd);
            flip.await.unwrap();
            // That it came up refusing is what its log says: a session sent
            // now would race the update that is on its way.
            wait_for("sessions to be accepted again", 30, || {
                http_get(daemon.http, &hello).ends_with("hi from target")
            });
            wait_for("the line that says so", 10, || {
                lines_with(&log_text(&logs), ACCEPTING).len() == 1
            });
            let log = log_text(&logs);
            assert_eq!(lines_with(&log, REFUSING).len(), 1, "{log}");
            wait_for("the new version", 30, || {
                rules(api).contains("DOMAIN,new.test,REJECT")
            });
            stop(&mut daemon, api);
        }

        // ---------------------------------------------------------------
        // the running daemon follows its sources

        /// A new version of the managed profile that arrives while the daemon
        /// runs is taken without a signal, a request to the API or `--watch`.
        /// The daemon comes up on the copy it has, waiting for nothing, and
        /// the update that is due at once brings the new version, which a
        /// reload puts in effect.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_new_version_of_the_managed_profile_is_taken_without_being_asked() {
            let w = Sources::new().await;
            let api = free_tcp_port();
            let line = "interval=60";
            let old = managed_text(&w.url("/managed"), line, api, "DOMAIN,old.test,REJECT\n");
            let new = managed_text(&w.url("/managed"), line, api, "DOMAIN,new.test,REJECT\n");
            w.seed("/managed", &old, TWO_HOURS);
            w.served.set("/managed", new.clone());
            w.point_at("/managed");
            let logs = w.dir.path().join("logs");
            let mut cmd = w.start();
            log_to(&mut cmd, &logs);
            let (mut daemon, errors) = spawn_command_reading_stderr(cmd);
            up(&daemon);
            // The profile the API shows is the file, which is the new version
            // as soon as it is downloaded; the rules are the generation in
            // use, which is the new one when the daemon has reloaded.
            wait_for("the new version to be in effect", 30, || {
                rules(api).contains("DOMAIN,new.test,REJECT")
            });
            assert_eq!(current(api), new);
            assert!(!rules(api).contains("DOMAIN,old.test,REJECT"));
            stop(&mut daemon, api);
            // It came up on the copy it had: the start waited for nothing, and
            // what put the new version in effect was a reload.
            let said = the_rest(&errors);
            assert!(!said.contains(WAITING), "{said}");
            let log = log_text(&logs);
            assert!(!lines_with(&log, "profile reloaded").is_empty(), "{log}");
        }

        /// The local file was changed after the last update and is in effect
        /// until the next check finds the server well: from then on the copy
        /// is, which the daemon takes without being asked.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn the_copy_takes_over_from_a_local_file_once_a_check_has_succeeded() {
            let w = Sources::new().await;
            let api = free_tcp_port();
            let line = "interval=60";
            let copy = managed_text(&w.url("/managed"), line, api, "DOMAIN,copy.test,REJECT\n");
            let local = managed_text(&w.url("/managed"), line, api, "DOMAIN,local.test,REJECT\n");
            w.seed("/managed", &copy, TWO_HOURS);
            // the server gives the text the copy has: the check finds nothing new
            w.served.set("/managed", copy.clone());
            // written after the copy was checked
            w.write(&local);
            let logs = w.dir.path().join("logs");
            let mut cmd = w.start();
            log_to(&mut cmd, &logs);
            let (mut daemon, errors) = spawn_command_reading_stderr(cmd);
            up(&daemon);
            wait_for("the copy to take over", 30, || current(api) == copy);
            assert!(rules(api).contains("DOMAIN,copy.test,REJECT"));
            stop(&mut daemon, api);
            // It came up on the local file, waiting for nothing: the copy
            // took over at a reload, which nothing but the check asked for.
            let said = the_rest(&errors);
            assert!(!said.contains(WAITING), "{said}");
            let log = log_text(&logs);
            assert!(!lines_with(&log, "profile reloaded").is_empty(), "{log}");
        }

        // ---------------------------------------------------------------
        // the sources asked for an update through the API

        /// What the links of these tests hold in every place a link can hold a
        /// token. No answer of the two endpoints holds it.
        const TOKEN: &str = "S3CR3T";
        /// Where the managed profile and the included file are on the
        /// server: the token is in the path too.
        const MANAGED_AT: &str = "/S3CR3T/managed";
        const INCLUDED_AT: &str = "/S3CR3T/rules";
        /// Where the managed profile is after its link was rotated.
        const ROTATED_AT: &str = "/S3CR3T/rotated";
        /// What the log says of the sources a failed reload names, which were
        /// never downloaded.
        const NEVER_DOWNLOADED: &str = "sources that were never downloaded";
        /// What a page answered in place of a profile looks like.
        const PAGE: &str =
            "<html><head><title>Sign in</title></head><body>Welcome to the Wi-Fi</body></html>";
        /// What the update of a version that waits for a file says.
        const HELD_BACK: &str = "waiting for the sources this version refers to";

        /// A link to `path` that holds the token in its query.
        fn token_link(w: &Sources, path: &str) -> Url {
            w.url(&format!("{path}?token={TOKEN}"))
        }

        /// The link of the managed profile: its user name and its password hold
        /// the token as well.
        fn managed_link(w: &Sources) -> Url {
            let mut link = token_link(w, MANAGED_AT);
            link.set_username(TOKEN).unwrap();
            link.set_password(Some(TOKEN)).unwrap();
            link
        }

        /// A daemon over the world `w` that logs to `logs`, up.
        fn start_logging(w: &Sources, logs: &Path) -> (Daemon, mpsc::Receiver<String>) {
            let mut cmd = w.start();
            log_to(&mut cmd, logs);
            let (daemon, errors) = spawn_command_reading_stderr(cmd);
            up(&daemon);
            (daemon, errors)
        }

        /// `GET /v1/external_resources`: the items of its list.
        fn listing(api: u16) -> Vec<serde_json::Value> {
            let (status, body) = api_call(api, "GET", "/v1/external_resources", "k", None);
            assert_eq!(status, 200, "{body}");
            assert!(!body.contains(TOKEN), "{body}");
            let list: serde_json::Value = serde_json::from_str(&body).unwrap();
            list["defines"].as_array().expect("a list").clone()
        }

        /// The item of the managed profile, as a user finds the key to update.
        fn managed_item(api: u16) -> serde_json::Value {
            listing(api)
                .into_iter()
                .find(|item| item["type"] == "profile")
                .expect("the listing has the managed profile")
        }

        /// `POST /v1/external_resources/update` for `key`: what the answer maps
        /// each key to.
        fn updated(api: u16, key: &str) -> serde_json::Value {
            let body = serde_json::json!({ "key": key }).to_string();
            let (status, answer) = api_call_within(
                api,
                "POST",
                "/v1/external_resources/update",
                "k",
                Some(&body),
                Duration::from_secs(30),
            );
            assert_eq!(status, 200, "{answer}");
            assert!(!answer.contains(TOKEN), "{answer}");
            serde_json::from_str(&answer).unwrap()
        }

        /// How many times the daemon has said it reloaded the profile.
        fn reloads(logs: &Path) -> usize {
            lines_with(&log_text(logs), "profile reloaded").len()
        }

        /// A new version of the managed profile that is asked for through the
        /// API is taken, and the daemon reloads by itself: the rule is in the
        /// running profile, and the local file is as it was. The copy was
        /// checked a minute ago, so nobody is asked for it before the request.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn an_update_asked_for_through_the_api_is_taken_and_reloaded() {
            let w = Sources::new().await;
            let api = free_tcp_port();
            let link = managed_link(&w);
            let text = |rules: &str| managed_text(&link, "interval=3600", api, rules);
            let old = text("DOMAIN,old.test,REJECT\n");
            rurge_net::testing::seed_cache(
                &w.data(),
                &link,
                old.as_bytes(),
                SystemTime::now() - Duration::from_secs(60),
            );
            w.served.set(MANAGED_AT, old);
            let conf = w.write(&format!("#!MANAGED-CONFIG {link}\n"));
            let before = as_it_is(&conf);
            let logs = w.dir.path().join("logs");
            let (mut daemon, _errors) = start_logging(&w, &logs);
            assert!(rules(api).contains("DOMAIN,old.test,REJECT"));
            assert_eq!(
                w.served.requests().len(),
                0,
                "nobody was asked for the copy"
            );

            let item = managed_item(api);
            assert_eq!(item["ready"], true);
            assert!(item.get("error").is_none(), "{item}");
            let key = item["key"].as_str().unwrap().to_string();

            w.served.set(
                MANAGED_AT,
                text("DOMAIN,old.test,REJECT\nDOMAIN,new.test,REJECT\n"),
            );
            assert_eq!(
                updated(api, &key),
                serde_json::json!({ key.as_str(): "success" })
            );
            wait_for("the new version to be in effect", 30, || {
                rules(api).contains("DOMAIN,new.test,REJECT")
            });
            wait_for("the line that says the profile was reloaded", 10, || {
                reloads(&logs) >= 1
            });
            assert_eq!(as_it_is(&conf), before, "the local file was written to");
            stop(&mut daemon, api);
            // all that this online start reached: the profile it was asked to
            // update, and the two GeoIP databases, which it asks the loopback for
            let managed: BTreeSet<String> = [MANAGED_AT.to_string()].into();
            let databases: BTreeSet<String> = [COUNTRY_DATABASE, ASN_DATABASE]
                .into_iter()
                .map(String::from)
                .collect();
            assert!(asked_for(&w.served).is_subset(&managed));
            assert!(asked_for(&w.geo).is_subset(&databases));
        }

        /// The link of the managed profile is rotated while the daemon runs, and
        /// the reload that is asked for fails: the file names a copy nobody has
        /// downloaded. The daemon downloads that copy, and loads the profile
        /// again when it is here, without a restart and without an interval
        /// running out. The log says so once, and shows nothing of either link
        /// beyond where it goes.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_link_rotated_while_the_daemon_runs_is_downloaded_and_taken() {
            let w = Sources::new().await;
            let api = free_tcp_port();
            let first = managed_link(&w);
            let second = token_link(&w, ROTATED_AT);
            let text = |link: &Url, rules: &str| managed_text(link, "interval=3600", api, rules);
            // the first copy was checked a minute ago, so nobody is asked for it
            rurge_net::testing::seed_cache(
                &w.data(),
                &first,
                text(&first, "DOMAIN,old.test,REJECT\n").as_bytes(),
                SystemTime::now() - Duration::from_secs(60),
            );
            w.served
                .set(ROTATED_AT, text(&second, "DOMAIN,new.test,REJECT\n"));
            w.write(&format!("#!MANAGED-CONFIG {first}\n"));
            let logs = w.dir.path().join("logs");
            let (mut daemon, _errors) = start_logging(&w, &logs);
            assert!(rules(api).contains("DOMAIN,old.test,REJECT"));
            assert_eq!(
                w.served.requests().len(),
                0,
                "nobody was asked for the copy"
            );

            // the link is rotated; the copy it names has never been downloaded
            let conf = w.write(&format!("#!MANAGED-CONFIG {second}\n"));
            let rotated = as_it_is(&conf);
            let (status, body) = api_call(api, "POST", "/v1/profiles/reload", "k", Some("{}"));
            assert_eq!(status, 200, "{body}");
            assert!(body.contains("\"ok\":false"), "{body}");

            wait_for("the copy of the new link to be asked for", 20, || {
                asked_for(&w.served).contains(ROTATED_AT)
            });
            wait_for("the new profile to be in effect", 30, || {
                rules(api).contains("DOMAIN,new.test,REJECT")
            });
            wait_for("the line that says the profile was reloaded", 10, || {
                reloads(&logs) >= 1
            });
            assert!(!rules(api).contains("DOMAIN,old.test,REJECT"));
            assert_eq!(as_it_is(&conf), rotated, "the local file was written to");
            let log = log_text(&logs);
            assert_eq!(lines_with(&log, NEVER_DOWNLOADED).len(), 1, "{log}");
            assert!(!log.contains(TOKEN), "{log}");
            assert!(!log.contains("rotated"), "{log}");
            stop(&mut daemon, api);
            // all that this online start reached: the copy of the new link, and
            // the two GeoIP databases, which it asks the loopback for
            let copies: BTreeSet<String> = [ROTATED_AT.to_string()].into();
            let databases: BTreeSet<String> = [COUNTRY_DATABASE, ASN_DATABASE]
                .into_iter()
                .map(String::from)
                .collect();
            assert_eq!(asked_for(&w.served), copies);
            assert!(asked_for(&w.geo).is_subset(&databases));
        }

        /// The server answers 200 with a page where the profile is meant to be:
        /// it is no profile and is not taken. The answer for the key says why,
        /// the daemon goes on with the rules it has, and the listing keeps the
        /// reason beside the copy that is still there.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_page_where_the_profile_is_meant_to_be_is_refused_and_the_listing_says_why() {
            let w = Sources::new().await;
            let api = free_tcp_port();
            let link = managed_link(&w);
            let old = managed_text(&link, "interval=3600", api, "DOMAIN,old.test,REJECT\n");
            rurge_net::testing::seed_cache(
                &w.data(),
                &link,
                old.as_bytes(),
                SystemTime::now() - Duration::from_secs(60),
            );
            w.served.set(MANAGED_AT, old.clone());
            w.write(&format!("#!MANAGED-CONFIG {link}\n"));
            let logs = w.dir.path().join("logs");
            let (mut daemon, _errors) = start_logging(&w, &logs);
            let key = managed_item(api)["key"].as_str().unwrap().to_string();

            w.served.set(MANAGED_AT, PAGE);
            let answer = updated(api, &key);
            let said = answer[key.as_str()]
                .as_str()
                .expect("the key is answered")
                .to_string();
            assert!(said.starts_with("does not load: "), "{said}");
            assert!(!said.contains("Welcome"), "the page is quoted: {said}");
            // the rules are the old ones, and so is the profile the file holds
            assert!(rules(api).contains("DOMAIN,old.test,REJECT"));
            assert_eq!(current(api), old);
            let item = managed_item(api);
            assert_eq!(item["error"], said.as_str());
            assert_eq!(item["ready"], true, "the copy it had stays");
            assert!(item.get("suspended").is_none(), "{item}");
            stop(&mut daemon, api);
        }

        /// The new version names a file that was never downloaded: the update
        /// says it waits for it, and when the file is here the version is taken
        /// with it, by one reload. The version is not taken with the file
        /// missing and the file filled in a few seconds later.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_version_that_names_a_file_never_downloaded_waits_for_it_and_is_taken_with_it() {
            let w = Sources::new().await;
            let api = free_tcp_port();
            let link = managed_link(&w);
            let included = token_link(&w, INCLUDED_AT);
            let text = |rules: &str| managed_text(&link, "interval=3600", api, rules);
            let old = text("DOMAIN,old.test,REJECT\n");
            rurge_net::testing::seed_cache(
                &w.data(),
                &link,
                old.as_bytes(),
                SystemTime::now() - Duration::from_secs(60),
            );
            w.served.set(MANAGED_AT, old);
            w.served.set(INCLUDED_AT, RULES);
            // Slower than the quiet second the daemon waits after a source has
            // moved: a version taken without its file would be in the rules
            // for a while, and the file would come as a reload of its own.
            w.served.set_delay(INCLUDED_AT, Duration::from_millis(2500));
            w.write(&format!("#!MANAGED-CONFIG {link}\n"));
            let logs = w.dir.path().join("logs");
            let (mut daemon, _errors) = start_logging(&w, &logs);
            let key = managed_item(api)["key"].as_str().unwrap().to_string();

            w.served.set(
                MANAGED_AT,
                text(&format!("#!include {included}\nDOMAIN,new.test,REJECT\n")),
            );
            assert_eq!(
                updated(api, &key),
                serde_json::json!({ key.as_str(): HELD_BACK })
            );
            wait_for("the rules of the version and of the file", 30, || {
                let now = rules(api);
                let version = now.contains("DOMAIN,new.test,REJECT");
                let file = now.contains("DOMAIN,included.test,REJECT");
                assert!(!version || file, "the version without its file: {now}");
                version && file
            });
            wait_for("the first reload to be logged", 10, || reloads(&logs) >= 1);
            // The one fixed wait of these tests: that a second reload does not
            // come. It would come a quiet second (the time the sources wait for
            // another move) after a version that was reported by itself, and
            // the daemon looks at them once a second; three quiet seconds are
            // three chances for the file and the profile to have been taken
            // apart, and half a second more is the margin.
            std::thread::sleep(Duration::from_millis(3500));
            assert_eq!(reloads(&logs), 1, "{}", log_text(&logs));
            stop(&mut daemon, api);
        }

        /// The way out of a refusal that needs no editing: the update that
        /// `strict` requires fails and sessions are refused; once the server is
        /// well, the update asked for through the API is the one that works, and
        /// sessions pass again within seconds. The daemon's own next attempt is a
        /// minute away, so nothing but the request can have done it.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn the_way_out_of_a_refusal_is_an_update_asked_for_through_the_api() {
            let w = Sources::new().await;
            let (_target, hello) = a_target().await;
            let api = free_tcp_port();
            let link = managed_link(&w);
            let copy = managed_text(&link, "interval=60 strict=true", api, "");
            rurge_net::testing::seed_cache(
                &w.data(),
                &link,
                copy.as_bytes(),
                SystemTime::now() - TWO_HOURS,
            );
            w.served.set(MANAGED_AT, copy);
            w.served.set_status(MANAGED_AT, 500);
            w.write(&format!("#!MANAGED-CONFIG {link}\n"));
            let logs = w.dir.path().join("logs");
            let (mut daemon, _errors) = start_logging(&w, &logs);
            let refused = http_get(daemon.http, &hello);
            assert!(refused.contains(rurge_engine::REFUSED_STRICT), "{refused}");
            // The daemon's own attempt has failed too, and its next is a
            // minute away: from here on nothing but a request ends the
            // refusal. The listing says why sessions are refused.
            wait_for("the daemon's own attempt to fail", 20, || {
                lines_with(&log_text(&logs), "resource fetch failed")
                    .iter()
                    .any(|line| line.contains("managed profile"))
            });
            let item = managed_item(api);
            assert_eq!(item["suspended"], true, "{item}");
            assert_eq!(item["error"], "http status 500");
            assert_eq!(item["ready"], true);

            w.served.set_status(MANAGED_AT, 200);
            let key = item["key"].as_str().unwrap().to_string();
            assert_eq!(
                updated(api, &key),
                serde_json::json!({ key.as_str(): "success" })
            );
            wait_for("sessions to be accepted again", 10, || {
                http_get(daemon.http, &hello).ends_with("hi from target")
            });
            wait_for("the line that says so", 10, || {
                lines_with(&log_text(&logs), ACCEPTING).len() == 1
            });
            let log = log_text(&logs);
            assert_eq!(lines_with(&log, REFUSING).len(), 1, "{log}");
            let item = managed_item(api);
            assert!(item.get("suspended").is_none(), "{item}");
            assert!(item.get("error").is_none(), "{item}");
            stop(&mut daemon, api);
        }

        /// The local file was edited after the last update and is in effect
        /// from the reload on; a check that finds the server well, even one that
        /// answers "not modified", ends that, and the daemon reloads by itself.
        /// The start downloads the copy itself, so it has the validators a
        /// seeded cache would not have.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_local_file_edited_after_the_last_update_gives_way_to_the_next_check() {
            let w = Sources::new().await;
            let api = free_tcp_port();
            let link = managed_link(&w);
            // no `interval=` in the line: nothing is due for a day, so after
            // the start the server is asked only by the request
            let profile = |rules: &str| {
                format!(
                    "#!MANAGED-CONFIG {link}\n[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nhttp-api = k@127.0.0.1:{api}\nloglevel = warning\n[Proxy]\n[Rule]\n{rules}FINAL,DIRECT\n"
                )
            };
            w.served
                .set(MANAGED_AT, profile("DOMAIN,copy.test,REJECT\n"));
            w.write(&profile("DOMAIN,local.test,REJECT\n"));
            let logs = w.dir.path().join("logs");
            let mut cmd = w.start();
            log_to(&mut cmd, &logs);
            let (mut daemon, errors) = spawn_command_reading_stderr(cmd);
            stderr_up_to(&errors, WAITING);
            up(&daemon);
            // the copy was checked after the file was written: it is the profile
            let now = rules(api);
            assert!(now.contains("DOMAIN,copy.test,REJECT"), "{now}");
            assert!(!now.contains("DOMAIN,local.test,REJECT"), "{now}");

            // an edit is in effect from the reload on: the file is newer
            w.write(&profile(
                "DOMAIN,local.test,REJECT\nDOMAIN,edited.test,REJECT\n",
            ));
            let (status, body) = api_call(api, "POST", "/v1/profiles/reload", "k", Some("{}"));
            assert!(status == 200 && body.contains("\"ok\":true"), "{body}");
            let now = rules(api);
            assert!(now.contains("DOMAIN,edited.test,REJECT"), "{now}");
            assert!(!now.contains("DOMAIN,copy.test,REJECT"), "{now}");

            // the server answers "not modified" to the request: the check
            // succeeded, and that ends the file's precedence
            let key = managed_item(api)["key"].as_str().unwrap().to_string();
            let asked_before = w.served.hits(MANAGED_AT);
            assert_eq!(
                updated(api, &key),
                serde_json::json!({ key.as_str(): "success" })
            );
            let asked_since: Vec<_> = w
                .served
                .requests()
                .into_iter()
                .filter(|request| request.path == MANAGED_AT)
                .skip(asked_before)
                .collect();
            assert!(!asked_since.is_empty());
            assert!(
                asked_since
                    .iter()
                    .all(|request| request.header("if-none-match").is_some()),
                "{asked_since:?}"
            );
            wait_for("the copy to take over again", 30, || {
                let now = rules(api);
                now.contains("DOMAIN,copy.test,REJECT")
                    && !now.contains("DOMAIN,edited.test,REJECT")
            });
            wait_for("a further reload to be logged", 10, || reloads(&logs) >= 2);
            stop(&mut daemon, api);
        }

        /// A daemon that does not go to the network lists what its cache has of
        /// the sources of the profile, with no GeoIP database to list, and
        /// every update it is asked for says that nothing is downloaded. The
        /// server is not asked at all.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn offline_the_endpoints_list_the_cache_and_update_nothing() {
            let w = Sources::new().await;
            let api = free_tcp_port();
            let link = managed_link(&w);
            let included = token_link(&w, INCLUDED_AT);
            let copy = managed_text(
                &link,
                "interval=3600",
                api,
                &format!("#!include {included}\n"),
            );
            let checked = SystemTime::now() - Duration::from_secs(60);
            rurge_net::testing::seed_cache(&w.data(), &link, copy.as_bytes(), checked);
            rurge_net::testing::seed_cache(&w.data(), &included, RULES.as_bytes(), checked);
            w.served.set(MANAGED_AT, copy);
            w.served.set(INCLUDED_AT, RULES);
            w.write(&format!("#!MANAGED-CONFIG {link}\n"));
            let (mut daemon, _errors) =
                spawn_command_reading_stderr(rurge_run(&w.conf(), &w.data()));
            up(&daemon);

            let items = listing(api);
            let kinds: Vec<&str> = items
                .iter()
                .map(|item| item["type"].as_str().unwrap())
                .collect();
            assert_eq!(kinds, ["profile", "include"], "{items:?}");
            assert!(items.iter().all(|item| item["ready"] == true), "{items:?}");
            assert!(
                items.iter().all(|item| item.get("error").is_none()),
                "{items:?}"
            );
            let keys: Vec<String> = items
                .iter()
                .map(|item| item["key"].as_str().unwrap().to_string())
                .collect();
            let offline = "offline mode: nothing is downloaded";
            assert_eq!(
                updated(api, "all"),
                serde_json::json!({ keys[0].as_str(): offline, keys[1].as_str(): offline })
            );
            assert_eq!(
                updated(api, &keys[0]),
                serde_json::json!({ keys[0].as_str(): offline })
            );
            stop(&mut daemon, api);
            assert_eq!(w.served.requests().len(), 0, "offline asks nobody");
            assert_eq!(w.geo.requests().len(), 0, "offline asks nobody");
        }
    }
}

mod service {
    use super::rurge_command;
    use assert_cmd::Command;
    use predicates::prelude::*;

    /// `SUDO_UID` is removed: `service ... --user` refuses to run under sudo,
    /// and these dry runs must not depend on how the suite was started.
    fn rurge() -> Command {
        let mut cmd = rurge_command();
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
