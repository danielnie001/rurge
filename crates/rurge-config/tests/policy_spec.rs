//! `[Proxy]` parameters are typed and validated at load time (phase 2 M1 design §4).

use rurge_config::config::{LoadOptions, Loaded, from_text};
use rurge_config::spec::{IpVersion, ProtoSpec};
use rurge_config::{Severity, codes};
use std::path::Path;

fn load(proxy: &str, extra: &str) -> Loaded {
    let text = format!("[Proxy]\n{proxy}\n{extra}\n[Rule]\nFINAL,DIRECT\n");
    from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests())
}

fn codes_of(loaded: &Loaded, severity: Severity) -> Vec<&'static str> {
    loaded
        .diagnostics
        .iter()
        .filter(|d| d.severity == severity)
        .map(|d| d.code)
        .collect()
}

#[test]
fn specs_are_stored_in_proxy_order() {
    let loaded = load(
        "A = http, a.example, 80, ip-version=v6-only\nSS = ss, s.example, 8388, encrypt-method=aes-128-gcm, password=x\nB = socks5, b.example, 1080\nC = direct, interface=eth0",
        "",
    );
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
    let names: Vec<&str> = loaded
        .config
        .specs
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["A", "B", "C"], "ss has no spec yet");
    assert_eq!(
        loaded.config.spec("A").unwrap().common.ip_version,
        IpVersion::V6Only
    );
    assert!(matches!(
        loaded.config.spec("B").unwrap().proto,
        ProtoSpec::Socks5(_)
    ));
    assert!(loaded.config.spec("SS").is_none() && loaded.config.spec("nope").is_none());
}

#[test]
fn inert_and_ios_only_parameters_are_reported_once_per_name() {
    let loaded = load(
        "A = socks5, a.example, 1080, udp-relay=true, tfo=true\nB = socks5, b.example, 1080, udp-relay=true, hybrid=on\nC = http, c.example, 80, hybrid=off",
        "",
    );
    let warnings: Vec<(&str, String, u32)> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.code == codes::W_PARAM_NOT_EFFECTIVE || d.code == codes::W_PLATFORM_IGNORED)
        .map(|d| (d.code, d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        warnings,
        [
            (
                codes::W_PARAM_NOT_EFFECTIVE,
                "policy parameter `udp-relay` is parsed but has no effect in this version"
                    .to_string(),
                2
            ),
            (
                codes::W_PARAM_NOT_EFFECTIVE,
                "policy parameter `tfo` is parsed but has no effect in this version".to_string(),
                2
            ),
            (
                codes::W_PLATFORM_IGNORED,
                "policy parameter `hybrid` is iOS-only; ignored".to_string(),
                3
            ),
        ]
    );
}

#[test]
fn an_invalid_parameter_fails_the_load_and_leaves_no_spec() {
    let loaded = load("A = http, a.example, 80, tos=300", "");
    assert_eq!(
        codes_of(&loaded, Severity::Error),
        [codes::E_INVALID_POLICY_PARAM]
    );
    assert!(loaded.config.spec("A").is_none());
}

#[test]
fn underlying_proxy_cycles() {
    // direct cycle
    let loaded = load(
        "A = http, a.example, 80, underlying-proxy=B\nB = http, b.example, 80, underlying-proxy=A",
        "",
    );
    assert_eq!(
        codes_of(&loaded, Severity::Error),
        [codes::E_UNDERLYING_PROXY_CYCLE; 2]
    );
    // through a group that lists the policy itself
    let loaded = load(
        "A = http, a.example, 80, underlying-proxy=Pick\nB = socks5, b.example, 1080",
        "[Proxy Group]\nPick = select, B, A",
    );
    let errors: Vec<String> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{} {}", d.code, d.message))
        .collect();
    assert_eq!(
        errors,
        ["E0019 policy `A`: `underlying-proxy` leads back to the policy itself (via `Pick`)"]
    );
    // a plain chain is fine
    let loaded = load(
        "Exit = http, a.example, 80, underlying-proxy=Pick\nEntry = socks5, b.example, 1080",
        "[Proxy Group]\nPick = select, Entry, DIRECT",
    );
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
}

#[test]
fn keystore_base64_is_checked_at_load() {
    let loaded = load(
        "A = direct",
        "[Keystore]\ngood = type=p12, base64=QUJD, password=x\nnopad = base64=QUI\nbad = type=p12, base64=@@@, password=x",
    );
    let errors: Vec<(String, u32)> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.code == codes::E_KEYSTORE_BASE64)
        .map(|d| (d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        errors,
        [(
            "keystore item `bad`: `base64` is not valid Base64".to_string(),
            6
        )]
    );
}

#[test]
fn a_trojan_policy_has_no_spec_until_the_outbound_is_wired_in() {
    // M2a plan P6: the readers exist, `to_spec` does not use them yet, so
    // the registry keeps treating the policy as "not implemented"
    let loaded = load("T = trojan, t.example, 443, password=p, ws=true", "");
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
    assert!(loaded.config.spec("T").is_none());
}
