//! `[Proxy]` parameters are typed and validated at load time (phase 2 M1 design §4).

use rurge_config::config::{LoadOptions, Loaded, from_text};
use rurge_config::spec::{IpVersion, ProtoSpec, Tristate};
use rurge_config::{Severity, codes};
use std::path::Path;
use std::time::Duration;

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
        "A = http, a.example, 80, ip-version=v6-only\nTs = tailscale, section-name=ts\nB = socks5, b.example, 1080\nC = direct, interface=eth0",
        "",
    );
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
    let names: Vec<&str> = loaded
        .config
        .specs
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["A", "B", "C"], "tailscale has no spec");
    assert_eq!(
        loaded.config.spec("A").unwrap().common.ip_version,
        IpVersion::V6Only
    );
    assert!(matches!(
        loaded.config.spec("B").unwrap().proto,
        ProtoSpec::Socks5(_)
    ));
    assert!(loaded.config.spec("Ts").is_none() && loaded.config.spec("nope").is_none());
}

#[test]
fn inert_and_ios_only_parameters_are_reported_once_per_name() {
    let loaded = load(
        "A = socks5, a.example, 1080, ecn=on, tfo=true\nB = socks5, b.example, 1080, ecn=on, hybrid=on\nC = http, c.example, 80, hybrid=off",
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
                "policy parameter `tfo` is parsed but has no effect in this version".to_string(),
                2
            ),
            (
                codes::W_PARAM_NOT_EFFECTIVE,
                "policy parameter `ecn` is parsed but has no effect in this version".to_string(),
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

/// `h3=true` on `trust-tunnel` is said once per load and the policy keeps
/// HTTP/2 (phase 2 M6 design 5.1).
#[test]
fn trust_tunnel_h3_is_reported_once_per_load() {
    let loaded = load(
        "A = trust-tunnel, a.example, 443, username=u, password=p, h3=true
B = trust-tunnel, b.example, 443, username=u, password=p, h3=true",
        "",
    );
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
    let inert: Vec<(String, u32)> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.code == codes::W_PARAM_NOT_EFFECTIVE)
        .map(|d| (d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        inert,
        [(
            "policy parameter `h3` is parsed but has no effect in this version".to_string(),
            2
        )]
    );
    assert!(loaded.config.spec("A").is_some() && loaded.config.spec("B").is_some());
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
fn a_trojan_policy_is_typed() {
    let loaded = load(
        "T = trojan, t.example, 443, password=p, ws=true, ws-path=/w, sni=front.example, underlying-proxy=E\nE = socks5, e.example, 1080\nBad = trojan, b.example, 443",
        "",
    );
    let errors: Vec<String> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect();
    assert_eq!(errors, ["policy `Bad`: `password` is required"]);
    let spec = loaded.config.spec("T").expect("typed");
    let ProtoSpec::Trojan(trojan) = &spec.proto else {
        panic!("{:?}", spec.proto)
    };
    assert_eq!(trojan.password.expose(), "p");
    assert_eq!(trojan.ws.as_ref().unwrap().path, "/w");
    assert_eq!(spec.common.underlying_proxy.as_deref(), Some("E"));
    assert!(
        loaded.config.spec("Bad").is_none(),
        "an error drops the spec"
    );
}

#[test]
fn a_trojan_policy_takes_part_in_the_chain_checks() {
    let loaded = load(
        "A = trojan, a.example, 443, password=p, underlying-proxy=B\nB = trojan, b.example, 443, password=p, underlying-proxy=A",
        "",
    );
    assert!(codes_of(&loaded, Severity::Error).contains(&codes::E_UNDERLYING_PROXY_CYCLE));
    let loaded = load(
        "A = trojan, a.example, 443, password=p, underlying-proxy=Ghost",
        "",
    );
    assert_eq!(
        codes_of(&loaded, Severity::Error),
        [codes::E_UNKNOWN_POLICY_REF]
    );
}

/// `hysteria2` lines are read and checked at load (phase 2 M7 design 3.1,
/// 5.1): the combinations they cannot have are errors, which leave no
/// spec; a hop interval under 5 seconds is raised to 5, and `ecn` has an
/// effect on them.
#[test]
fn hysteria2_lines_are_checked_at_load() {
    let loaded = load(
        "A = hysteria2, a.example, 443, password=p, port-hopping=5000-6000, underlying-proxy=E
B = hysteria2, b.example, 443, password=p, salamander-password=s, gecko-password=g
C = hysteria2, c.example, 443, password=p, shadow-tls-password=st
D = hysteria2, d.example, 443, password=p, ecn=off, port-hopping=5000-6000, port-hopping-interval=2
E = socks5, e.example, 1080",
        "",
    );
    let found: Vec<(&str, String, u32)> = loaded
        .diagnostics
        .iter()
        .map(|d| (d.code, d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        found,
        [
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `A`: `port-hopping` cannot be combined with `underlying-proxy`".to_string(),
                2
            ),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `B`: `salamander-password` cannot be combined with `gecko-password`"
                    .to_string(),
                3
            ),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `C`: Shadow TLS cannot be combined with a `hysteria2` policy".to_string(),
                4
            ),
            (
                codes::W_INVALID_VALUE,
                "policy `D`: `port-hopping-interval` is below the minimum of 5 seconds; using 5"
                    .to_string(),
                5
            ),
        ]
    );
    for name in ["A", "B", "C"] {
        assert!(loaded.config.spec(name).is_none(), "{name}");
    }
    let d = loaded.config.spec("D").expect("a warning leaves the spec");
    let ProtoSpec::Hysteria2(hysteria2) = &d.proto else {
        panic!("{:?}", d.proto)
    };
    assert_eq!(
        hysteria2.quic.port_hopping.as_ref().map(|h| h.interval),
        Some(Duration::from_secs(5))
    );
    assert_eq!(d.common.ecn, Tristate::Off);
}

/// `tuic` and `tuic-v5` lines are read and checked at load (phase 2 M7
/// design 3.1, 6.1): the combinations they cannot have and a credential
/// that is not one are errors, which leave no spec; a hop interval under 5
/// seconds is raised to 5, and `ecn` has an effect on them.
#[test]
fn tuic_lines_are_checked_at_load() {
    let loaded = load(
        "A = tuic, a.example, 443, token=t, port-hopping=5000-6000, underlying-proxy=E
B = tuic-v5, b.example, 443, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=p, shadow-tls-password=st
C = tuic-v5, c.example, 443, uuid=0233d11c-15a4, password=p
D = tuic-v5, d.example, 443, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=p, ecn=off, port-hopping=5000-6000, port-hopping-interval=2
E = socks5, e.example, 1080",
        "",
    );
    let found: Vec<(&str, String, u32)> = loaded
        .diagnostics
        .iter()
        .map(|d| (d.code, d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        found,
        [
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `A`: `port-hopping` cannot be combined with `underlying-proxy`".to_string(),
                2
            ),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `B`: Shadow TLS cannot be combined with a `tuic-v5` policy".to_string(),
                3
            ),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `C`: `uuid` is not a valid UUID".to_string(),
                4
            ),
            (
                codes::W_INVALID_VALUE,
                "policy `D`: `port-hopping-interval` is below the minimum of 5 seconds; using 5"
                    .to_string(),
                5
            ),
        ]
    );
    for name in ["A", "B", "C"] {
        assert!(loaded.config.spec(name).is_none(), "{name}");
    }
    let d = loaded.config.spec("D").expect("a warning leaves the spec");
    let ProtoSpec::Tuic(tuic) = &d.proto else {
        panic!("{:?}", d.proto)
    };
    assert_eq!(
        tuic.quic.port_hopping.as_ref().map(|h| h.interval),
        Some(Duration::from_secs(5))
    );
    assert_eq!(d.common.ecn, Tristate::Off);
}

/// `masque` lines are read and checked at load (phase 2 M7 design 3.1,
/// 7.1): the combinations they cannot have and one credential without the
/// other are errors; a hop interval under 5 seconds is raised to 5, and
/// `ecn` has an effect on them. They have no spec until the engine builds
/// them (M7c task 8).
#[test]
fn masque_lines_are_checked_at_load() {
    let loaded = load(
        "A = masque, a.example, 443, username=u, password=p, port-hopping=5000-6000, underlying-proxy=E
B = masque, b.example, 443, shadow-tls-password=st
C = masque, c.example, 443, username=u
D = masque, d.example, 443, ecn=off, port-hopping=5000-6000, port-hopping-interval=2
E = socks5, e.example, 1080",
        "",
    );
    let found: Vec<(&str, String, u32)> = loaded
        .diagnostics
        .iter()
        .map(|d| (d.code, d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        found,
        [
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `A`: `port-hopping` cannot be combined with `underlying-proxy`".to_string(),
                2
            ),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `B`: Shadow TLS cannot be combined with a `masque` policy".to_string(),
                3
            ),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `C`: `password` is required with `username`".to_string(),
                4
            ),
            (
                codes::W_INVALID_VALUE,
                "policy `D`: `port-hopping-interval` is below the minimum of 5 seconds; using 5"
                    .to_string(),
                5
            ),
        ]
    );
    for name in ["A", "B", "C", "D"] {
        assert!(loaded.config.spec(name).is_none(), "{name}");
    }
}

/// `trust-tunnel` lines with `h3=true` are read and checked at load (phase
/// 2 M7 design 7.2): another ALPN, `tfo` and port hopping do not apply,
/// Shadow TLS is an error, which leaves no spec, and `ecn` has an effect on
/// them. Until the engine builds them over HTTP/3 (M7c task 8), `h3` is
/// said once to have no effect yet.
#[test]
fn trust_tunnel_lines_with_h3_are_checked_at_load() {
    let loaded = load(
        "A = trust-tunnel, a.example, 443, username=u, password=p, h3=true, alpn=h2, ecn=off
B = trust-tunnel, b.example, 443, username=u, password=p, h3=true, tfo=true, port-hopping=5000-6000
C = trust-tunnel, c.example, 443, username=u, password=p, h3=true, shadow-tls-password=st",
        "",
    );
    let found: Vec<(&str, String, u32)> = loaded
        .diagnostics
        .iter()
        .map(|d| (d.code, d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        found,
        [
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `A`: `alpn` is always `h3` on `trust-tunnel` policies with `h3=true`; ignored"
                    .to_string(),
                2
            ),
            (
                codes::W_PARAM_NOT_EFFECTIVE,
                "policy parameter `h3` is parsed but has no effect in this version".to_string(),
                2
            ),
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `B`: `tfo` does not apply to `trust-tunnel` policies with `h3=true`; ignored"
                    .to_string(),
                3
            ),
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `B`: `port-hopping` does not apply to `trust-tunnel` policies; ignored"
                    .to_string(),
                3
            ),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `C`: Shadow TLS cannot be combined with `h3=true` on a `trust-tunnel` policy"
                    .to_string(),
                4
            ),
        ]
    );
    for (name, ecn) in [("A", Tristate::Off), ("B", Tristate::Auto)] {
        let spec = loaded.config.spec(name).expect("warnings leave the spec");
        let ProtoSpec::TrustTunnel(tt) = &spec.proto else {
            panic!("{:?}", spec.proto)
        };
        assert!(tt.h3, "{name}");
        assert_eq!(tt.tls.alpn, ["h3"], "{name}");
        assert_eq!((spec.common.ecn, spec.common.tfo), (ecn, false), "{name}");
    }
    assert!(loaded.config.spec("C").is_none());
}

/// A group's `underlying-proxy` chains the proxies it takes in (`M (via
/// R)`, phase 2 M3 design 5.4), but port hopping cannot go through a chain:
/// the load says, at the group's line, which member is left out — once,
/// whether the profile writes it on the line, gives it through
/// `include-other-group` or through `include-all-proxies`, as the group's
/// filter lets it in. The load goes on.
#[test]
fn a_relayed_group_says_which_member_with_port_hopping_it_leaves_out() {
    let loaded = load(
        "Hop = hysteria2, h.example, 443, password=p, port-hopping=5000-6000
Plain = hysteria2, p.example, 443, password=p
E = socks5, e.example, 1080",
        "[Proxy Group]
G = select, Hop, Plain, include-other-group=Nodes, underlying-proxy=E
Nodes = select, Hop, Plain
Via = select, include-other-group=Nodes, underlying-proxy=E
All = select, include-all-proxies=true, policy-regex-filter=^H, underlying-proxy=E
Filtered = select, include-other-group=Nodes, policy-regex-filter=^P, underlying-proxy=E
Direct = select, Hop, underlying-proxy=DIRECT",
    );
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
    let found: Vec<(&str, String, u32)> = loaded
        .diagnostics
        .iter()
        .map(|d| (d.code, d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    let left_out = |group: &str| {
        format!(
            "policy group `{group}` relays its members through `underlying-proxy`: `Hop` uses `port-hopping`, which cannot go through it; `Hop (via E)` is left out"
        )
    };
    assert_eq!(
        found,
        [
            (codes::W_SET_LINES_SKIPPED, left_out("G"), 6),
            (codes::W_SET_LINES_SKIPPED, left_out("Via"), 8),
            (codes::W_SET_LINES_SKIPPED, left_out("All"), 9),
        ]
    );
}
