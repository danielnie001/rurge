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
        "A = socks5, a.example, 1080, tfo=true\nB = snell, b.example, 443, psk=p, version=4, tfo=true, udp-port=8443, hybrid=on\n\
C = snell, c.example, 443, psk=p, version=5, udp-port=8443, hybrid=off",
        "",
    );
    let warnings: Vec<(&str, String, u32)> = loaded
        .diagnostics
        .iter()
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
                "policy parameter `udp-port` is parsed but has no effect in this version"
                    .to_string(),
                3
            ),
            (
                codes::W_PLATFORM_IGNORED,
                "policy parameter `hybrid` is iOS-only; ignored".to_string(),
                3
            ),
        ]
    );
}

/// `ecn` is for QUIC: on a protocol over TCP it does not apply, which every
/// line that writes it is told (`W0028`), unlike a parameter that is only
/// parsed so far (`W0029`, once per load).
#[test]
fn ecn_on_a_protocol_over_tcp_is_not_applicable_on_every_line_that_writes_it() {
    let loaded = load(
        "A = socks5, a.example, 1080, ecn=on\nB = trojan, b.example, 443, password=p, ecn=off\n\
C = hysteria2, c.example, 443, password=p, ecn=off",
        "",
    );
    let warnings: Vec<(&str, String, u32)> = loaded
        .diagnostics
        .iter()
        .map(|d| (d.code, d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        warnings,
        [
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `A`: `ecn` does not apply to `socks5` policies; ignored".to_string(),
                2
            ),
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `B`: `ecn` does not apply to `trojan` policies; ignored".to_string(),
                3
            ),
        ]
    );
    assert_eq!(loaded.config.spec("B").unwrap().common.ecn, Tristate::Auto);
    assert_eq!(loaded.config.spec("C").unwrap().common.ecn, Tristate::Off);
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
B = hysteria2, b.example, 443, password=p, salamander-password=obfs1, gecko-password=obfs2
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
/// other are errors, which leave no spec; a hop interval under 5 seconds is
/// raised to 5, and `ecn` has an effect on them.
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
    for name in ["A", "B", "C"] {
        assert!(loaded.config.spec(name).is_none(), "{name}");
    }
    let d = loaded.config.spec("D").expect("a warning leaves the spec");
    let ProtoSpec::Masque(masque) = &d.proto else {
        panic!("{:?}", d.proto)
    };
    assert_eq!(
        masque.quic.port_hopping.as_ref().map(|h| h.interval),
        Some(Duration::from_secs(5))
    );
    assert_eq!(d.common.ecn, Tristate::Off);
}

/// `trust-tunnel` lines with `h3=true` are read and checked at load (phase
/// 2 M7 design 7.2): another ALPN, `tfo` and port hopping do not apply,
/// Shadow TLS is an error, which leaves no spec, and `ecn` has an effect on
/// them; nothing is said of `h3` itself.
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

/// A name a parameter refers to is quoted when it can be a name: a comma that
/// is missing lets the next field slide into it (`Gone password=…`), as it
/// does into a value.
#[test]
fn a_name_another_field_slid_into_is_not_quoted() {
    let loaded = load(
        "Entry = http, e.example, 80
U1 = trojan, u.example, 443, password=p, underlying-proxy=Gone password=S3CR3T
U2 = ssh, s.example, 22, username=root, private-key=nokey password=S3CR3T
U3 = trojan, c.example, 443, password=p, client-cert=nocert password=S3CR3T
U4 = trojan, d.example, 443, password=p, underlying-proxy=Gone",
        "[Proxy Group]
G1 = select, Entry, underlying-proxy=Gone password=S3CR3T
G2 = select, Entry, include-other-group=nogroup password=S3CR3T
G3 = select, Entry, include-other-group=nogroup",
    );
    let errors: Vec<(String, u32)> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| (d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        errors,
        [
            ("policy `U1`: `underlying-proxy` references unknown policy".to_string(), 3),
            ("policy `U2`: `private-key` references unknown keystore item".to_string(), 4),
            ("policy `U3`: `client-cert` references unknown keystore item".to_string(), 5),
            // a name that can be one is quoted, as it was
            ("policy `U4`: `underlying-proxy` references unknown policy `Gone`".to_string(), 6),
            ("policy group `G1`: `underlying-proxy` references unknown policy".to_string(), 8),
            ("policy group `G2`: `include-other-group` references unknown policy group".to_string(), 9),
            ("policy group `G3`: `include-other-group` references unknown policy group `nogroup`".to_string(), 10),
        ]
    );
}

/// A policy or a group named by a share link pasted into the profile — all
/// that stands in front of the first `=` of its query, the password among
/// it — is not named in a diagnostic of the loader, as its subject or in
/// passing. A name that can be one is quoted, as it was.
#[test]
fn a_policy_named_by_a_link_is_not_named_in_a_diagnostic() {
    let link = "trojan://hunter2@t.example:443?sni";
    let loaded = load(
        &format!(
            "{link} = trojan, t.example, 443, password=pw, tfo=maybe
{link} = http, h.example, 80
Ext1 = external, exec=/bin/true, local-port=1080
{link}x = external, exec=/bin/true, local-port=1080
{link}y = external, exec=/bin/true, local-port=1081
Ext2 = external, exec=/bin/true, local-port=1081
{link}L = http, l.example, 80, underlying-proxy=Loop
Loop = http, l2.example, 80, underlying-proxy={link}L
{link}Hop = hysteria2, h.example, 443, password=p, port-hopping=5000-6000
{link}W = http, w.example, 80, alice, pw, mystery=1
{link}Own = external, exec=/bin/true, local-port=6152
Entry = http, e.example, 80"
        ),
        &format!(
            "[General]
http-listen = 127.0.0.1:6152
[Proxy Group]
{link}G = select, Ext1, mystery=1
{link}G = select, Ext1
{link}H = select, Nope
{link}E = url-test, Ext1, interval=soon
{link}Loop = select, Ext1, underlying-proxy={link}Loop
{link}R = select, {link}Hop, underlying-proxy=Entry
PG = select, {link}Hop, underlying-proxy=Entry"
        ),
    );
    let said: Vec<String> = loaded
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect();
    for message in &said {
        assert!(!message.contains("hunter2"), "{message}");
    }
    for expected in [
        "policy: invalid value `maybe` for `tfo` (expected true or false)",
        "duplicate policy name",
        "policy: `local-port` 1080 is also the `local-port` of policy `Ext1`",
        "policy `Ext2`: `local-port` 1081 is also the `local-port` of policy",
        "policy: `underlying-proxy` leads back to the policy itself (via `Loop`)",
        "policy `Loop`: `underlying-proxy` leads back to the policy itself (via a link)",
        "policy: unknown parameter `mystery` ignored",
        "policy: `local-port` 6152 is the port of rurge's own `http-listen`",
        "policy group: unknown parameter `mystery` ignored",
        "duplicate policy group name",
        "policy group references unknown policy `Nope`",
        "policy group: invalid value `soon` for `interval` (expected a positive number of seconds)",
        "policy group: `underlying-proxy` leads back to the group itself (via a link)",
        "policy group relays its members through `underlying-proxy`: a link uses `port-hopping`, which cannot go through it; a link is left out",
        "policy group `PG` relays its members through `underlying-proxy`: a link uses `port-hopping`, which cannot go through it; a link is left out",
    ] {
        assert!(said.iter().any(|m| m == expected), "{expected}: {said:#?}");
    }
}
