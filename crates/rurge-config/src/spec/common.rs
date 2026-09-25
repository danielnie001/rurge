//! The 14 common policy parameters (compatibility matrix §4.3).

use super::reader::ParamReader;
use crate::diagnostic::codes;
use crate::general::UdpTest;
use std::net::Ipv4Addr;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IpVersion {
    #[default]
    Dual,
    V4Only,
    V6Only,
    PreferV4,
    PreferV6,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tristate {
    #[default]
    Auto,
    On,
    Off,
}

/// Which kind of policy the parameters are written on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applies {
    Proxy,
    Direct,
    /// `reject*` aliases accept the common parameters; none has any effect.
    Reject,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommonOpts {
    pub interface: Option<String>,
    pub allow_other_interface: bool,
    pub dns_follow_interface: bool,
    pub no_error_alert: bool,
    pub ip_version: IpVersion,
    pub tfo: bool,
    pub tos: u8,
    pub ecn: Tristate,
    pub block_quic: Tristate,
    pub test_url: Option<String>,
    pub test_timeout: Option<Duration>,
    pub test_udp: Option<UdpTest>,
    pub underlying_proxy: Option<String>,
}

/// Parameter names the caller reports once per load: parsed but without
/// effect in this version (`W0029`), and iOS-only (`W0004`).
#[derive(Debug, Default)]
pub(crate) struct Notes {
    pub inert: Vec<&'static str>,
    pub ios_only: Vec<&'static str>,
}

const IP_VERSIONS: [(&str, IpVersion); 5] = [
    ("dual", IpVersion::Dual),
    ("v4-only", IpVersion::V4Only),
    ("v6-only", IpVersion::V6Only),
    ("prefer-v4", IpVersion::PreferV4),
    ("prefer-v6", IpVersion::PreferV6),
];

/// `true` / `false` are accepted wherever `on` / `off` are (manual, `hybrid` and `ecn`).
const TRISTATES: [(&str, Tristate); 5] = [
    ("auto", Tristate::Auto),
    ("on", Tristate::On),
    ("off", Tristate::Off),
    ("true", Tristate::On),
    ("false", Tristate::Off),
];

/// Parameters the manual marks "proxy policies only".
const PROXY_ONLY: [&str; 3] = ["underlying-proxy", "ecn", "no-error-alert"];

fn parse_tos(value: &str) -> Option<u8> {
    let value = value.trim();
    match value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        Some(hex) => u8::from_str_radix(hex, 16).ok(),
        None => value.parse().ok(),
    }
}

fn parse_udp_test(value: &str) -> Option<UdpTest> {
    let (hostname, server) = value.split_once('@')?;
    let hostname = hostname.trim();
    if hostname.is_empty() {
        return None;
    }
    Some(UdpTest {
        hostname: hostname.to_string(),
        server: server.trim().parse::<Ipv4Addr>().ok()?,
    })
}

pub(crate) fn read_common(
    r: &mut ParamReader<'_>,
    applies: Applies,
    notes: &mut Notes,
) -> CommonOpts {
    let mut interface = None;
    if let Some(v) = r.str("interface") {
        if v.trim().is_empty() {
            r.invalid("interface", v, "a network interface name");
        } else {
            interface = Some(v.trim().to_string());
        }
    }
    let allow_other_interface = r.bool("allow-other-interface").unwrap_or(false);
    let dns_follow_interface = r.bool("dns-follow-interface").unwrap_or(false);
    let mut no_error_alert = r.bool("no-error-alert").unwrap_or(false);
    let ip_version = r.choice("ip-version", &IP_VERSIONS).unwrap_or_default();
    let tfo = r.bool("tfo").unwrap_or(false);
    let mut tos = 0;
    if let Some(v) = r.str("tos") {
        match parse_tos(v) {
            Some(n) => tos = n,
            None => r.invalid("tos", v, "0-255 or 0x00-0xff"),
        }
    }
    let ecn_present = r.has("ecn");
    let mut ecn = r.choice("ecn", &TRISTATES).unwrap_or_default();
    let block_quic_present = r.has("block-quic");
    let block_quic = r.choice("block-quic", &TRISTATES).unwrap_or_default();
    let mut test_url = None;
    if let Some(v) = r.str("test-url") {
        let lower = v.trim().to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            test_url = Some(v.trim().to_string());
        } else {
            r.invalid("test-url", v, "an http:// or https:// URL");
        }
    }
    let mut test_timeout = None;
    if let Some(v) = r.str("test-timeout") {
        match v.trim().parse::<u32>() {
            Ok(secs) if secs > 0 => test_timeout = Some(Duration::from_secs(u64::from(secs))),
            _ => r.invalid("test-timeout", v, "seconds, at least 1"),
        }
    }
    let mut test_udp = None;
    if let Some(v) = r.str("test-udp") {
        match parse_udp_test(v) {
            Some(t) => test_udp = Some(t),
            None => r.invalid("test-udp", v, "hostname@ipv4"),
        }
    }
    let mut underlying_proxy = r
        .str("underlying-proxy")
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    // iOS only: never applicable here, so the value is not ours to judge
    let hybrid_present = r.has("hybrid");
    r.touch("hybrid");

    if applies == Applies::Direct {
        for key in PROXY_ONLY {
            if r.has(key) {
                r.warn(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!("`{key}` does not apply to `direct` policies; ignored"),
                );
            }
        }
        underlying_proxy = None;
        ecn = Tristate::Auto;
        no_error_alert = false;
    }
    if applies != Applies::Reject {
        let inert = [
            ("dns-follow-interface", dns_follow_interface),
            ("tfo", tfo),
            ("test-udp", test_udp.is_some()),
            ("block-quic", block_quic_present),
            ("ecn", ecn_present && applies == Applies::Proxy),
        ];
        notes
            .inert
            .extend(inert.iter().filter(|(_, on)| *on).map(|(name, _)| *name));
        if hybrid_present {
            notes.ios_only.push("hybrid");
        }
    }
    CommonOpts {
        interface,
        allow_other_interface,
        dns_follow_interface,
        no_error_alert,
        ip_version,
        tfo,
        tos,
        ecn,
        block_quic,
        test_url,
        test_timeout,
        test_udp,
        underlying_proxy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::codes;
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::net::Ipv4Addr;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    fn read(def: &str, applies: Applies) -> (CommonOpts, Notes, Vec<crate::Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut notes = Notes::default();
        let common = read_common(&mut r, applies, &mut notes);
        (common, notes, r.finish())
    }

    #[test]
    fn every_common_parameter_is_parsed() {
        let (c, notes, diags) = read(
            "http, h, 1, interface=en0, allow-other-interface=true, dns-follow-interface=true, \
             no-error-alert=true, ip-version=prefer-v6, tfo=true, tos=0x28, ecn=on, block-quic=off, \
             test-url=http://t.example/, test-timeout=3, test-udp=apple.com@1.1.1.1, \
             underlying-proxy=Entry, hybrid=auto",
            Applies::Proxy,
        );
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(
            c,
            CommonOpts {
                interface: Some("en0".into()),
                allow_other_interface: true,
                dns_follow_interface: true,
                no_error_alert: true,
                ip_version: IpVersion::PreferV6,
                tfo: true,
                tos: 0x28,
                ecn: Tristate::On,
                block_quic: Tristate::Off,
                test_url: Some("http://t.example/".into()),
                test_timeout: Some(Duration::from_secs(3)),
                test_udp: Some(crate::general::UdpTest {
                    hostname: "apple.com".into(),
                    server: Ipv4Addr::new(1, 1, 1, 1),
                }),
                underlying_proxy: Some("Entry".into()),
            }
        );
        assert_eq!(
            notes.inert,
            [
                "dns-follow-interface",
                "tfo",
                "test-udp",
                "block-quic",
                "ecn"
            ]
        );
        assert_eq!(notes.ios_only, ["hybrid"]);
    }

    #[test]
    fn defaults_and_false_booleans_leave_no_notes() {
        let (c, notes, diags) = read(
            "http, h, 1, tfo=false, dns-follow-interface=false",
            Applies::Proxy,
        );
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(c, CommonOpts::default());
        assert!(notes.inert.is_empty() && notes.ios_only.is_empty());
        assert_eq!(c.ip_version, IpVersion::Dual);
        assert_eq!(c.tos, 0);
    }

    #[test]
    fn invalid_values_are_errors() {
        for (def, key) in [
            ("http, h, 1, tos=300", "tos"),
            ("http, h, 1, tos=0xZZ", "tos"),
            ("http, h, 1, ip-version=v5", "ip-version"),
            ("http, h, 1, interface=", "interface"),
            ("http, h, 1, test-url=ftp://x/", "test-url"),
            ("http, h, 1, test-timeout=0", "test-timeout"),
            ("http, h, 1, test-udp=apple.com", "test-udp"),
            ("http, h, 1, test-udp=apple.com@::1", "test-udp"),
            ("http, h, 1, ecn=maybe", "ecn"),
        ] {
            let (_, _, diags) = read(def, Applies::Proxy);
            assert_eq!(diags.len(), 1, "{def}: {diags:?}");
            assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM, "{def}");
            assert!(
                diags[0].message.contains(&format!("`{key}`")),
                "{def}: {}",
                diags[0].message
            );
        }
    }

    #[test]
    fn proxy_only_parameters_do_not_apply_to_direct() {
        let (c, notes, diags) = read(
            "direct, interface=utun0, underlying-proxy=Entry, ecn=on, no-error-alert=true",
            Applies::Direct,
        );
        assert_eq!(c.interface.as_deref(), Some("utun0"));
        assert_eq!(
            (c.underlying_proxy, c.ecn, c.no_error_alert),
            (None, Tristate::Auto, false)
        );
        let codes_seen: Vec<&str> = diags.iter().map(|d| d.code).collect();
        assert_eq!(codes_seen, [codes::W_PARAM_NOT_APPLICABLE; 3]);
        assert_eq!(
            diags[0].message,
            "policy `P`: `underlying-proxy` does not apply to `direct` policies; ignored"
        );
        assert!(notes.inert.is_empty(), "{:?}", notes.inert);
    }

    #[test]
    fn reject_aliases_only_have_their_values_checked() {
        let (_, notes, diags) = read(
            "reject, underlying-proxy=Entry, tfo=true, hybrid=on",
            Applies::Reject,
        );
        assert!(diags.is_empty(), "{diags:?}");
        assert!(notes.inert.is_empty() && notes.ios_only.is_empty());
        let (_, _, diags) = read("reject, tos=999", Applies::Reject);
        assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM);
    }

    #[test]
    fn hybrid_is_ios_only_so_its_value_is_never_checked() {
        for def in [
            "http, h, 1, hybrid=sometimes",
            "http, h, 1, hybrid=on",
            "direct, hybrid=",
        ] {
            let applies = if def.starts_with("direct") {
                Applies::Direct
            } else {
                Applies::Proxy
            };
            let (_, notes, diags) = read(def, applies);
            assert!(diags.is_empty(), "{def}: {diags:?}");
            assert_eq!(notes.ios_only, ["hybrid"], "{def}");
        }
    }
}
