//! Shadow TLS parameters (manual: Policies › TLS and Shadow TLS): an
//! obfuscation layer below the policy's own protocol, for a policy that
//! reaches its server over TCP. `shadow-tls-password` switches it on.

use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::is_server_name;
use crate::diagnostic::codes;
use crate::policy::PolicyKind;
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShadowTlsVersion {
    #[default]
    V2,
    V3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowTlsOpts {
    pub password: Secret<String>,
    /// The name sent as SNI in the camouflage handshake. `None`: no SNI is
    /// sent at all (manual); v3 always has one.
    pub sni: Option<String>,
    pub version: ShadowTlsVersion,
}

const KEYS: [&str; 3] = [
    "shadow-tls-password",
    "shadow-tls-sni",
    "shadow-tls-version",
];

/// Whether Shadow TLS can wrap a policy of this kind. It wraps a TCP
/// connection, so the QUIC-based protocols and the two VPN-like ones are out
/// (manual: a configuration error).
pub fn allowed_on(kind: PolicyKind) -> bool {
    !matches!(
        kind,
        PolicyKind::Tuic
            | PolicyKind::TuicV5
            | PolicyKind::Hysteria2
            | PolicyKind::Masque
            | PolicyKind::WireGuard
            | PolicyKind::Tailscale
    )
}

/// The Shadow TLS layer of the policy, when it asks for one. After an error
/// was reported the returned value is meaningless: the caller checks
/// `r.has_errors()`.
pub fn read_shadow_tls(r: &mut ParamReader<'_>) -> Option<ShadowTlsOpts> {
    if !KEYS.iter().any(|key| r.has(key)) {
        return None;
    }
    let kind = r.policy().kind;
    if !allowed_on(kind) {
        for key in KEYS {
            r.touch(key);
        }
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            format!(
                "Shadow TLS cannot be combined with a `{}` policy",
                kind.keyword()
            ),
        );
        return None;
    }
    let Some(password) = r.str("shadow-tls-password") else {
        // no password, no Shadow TLS: the other two have nothing to act on
        for key in ["shadow-tls-sni", "shadow-tls-version"] {
            if r.has(key) {
                r.touch(key);
                r.warn(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!("`{key}` has no effect without `shadow-tls-password`; ignored"),
                );
            }
        }
        return None;
    };
    if password.is_empty() {
        // never echo the value of this one
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`shadow-tls-password` is empty".to_string(),
        );
    }
    let table = [("2", ShadowTlsVersion::V2), ("3", ShadowTlsVersion::V3)];
    let version = r.choice("shadow-tls-version", &table).unwrap_or_default();
    let mut sni = None;
    match r.str("shadow-tls-sni").map(str::trim) {
        // the SNI extension carries a DNS name, never an address
        Some(name) if is_server_name(name) && name.parse::<IpAddr>().is_err() => {
            sni = Some(name.to_string());
        }
        Some(name) => r.invalid(
            "shadow-tls-sni",
            name,
            "a host name (an IDN in its xn-- form)",
        ),
        None if version == ShadowTlsVersion::V3 => r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`shadow-tls-sni` is required when `shadow-tls-version=3`".to_string(),
        ),
        None => {}
    }
    Some(ShadowTlsOpts {
        password: password.into(),
        sni,
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::path::Path;
    use std::sync::Arc;

    fn read(def: &str) -> (Option<ShadowTlsOpts>, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        // what every protocol reads for itself
        for key in ["password", "psk", "version", "reuse"] {
            r.touch(key);
        }
        let opts = read_shadow_tls(&mut r);
        let failed = r.has_errors();
        (opts, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    #[test]
    fn the_manuals_two_examples() {
        let (opts, failed, diags) =
            read("snell, 1.2.3.4, 443, psk=pwd1, version=4, reuse=true, shadow-tls-password=pwd2");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        let opts = opts.unwrap();
        assert_eq!(opts.password.expose(), "pwd2");
        assert_eq!((opts.sni, opts.version), (None, ShadowTlsVersion::V2));
        let (opts, failed, diags) = read(
            "snell, 1.2.3.4, 443, psk=pwd1, version=4, reuse=true, shadow-tls-password=pwd2, shadow-tls-version=3, shadow-tls-sni=example.com",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        let opts = opts.unwrap();
        assert_eq!(
            (opts.sni.as_deref(), opts.version),
            (Some("example.com"), ShadowTlsVersion::V3)
        );
        // nothing written, nothing read
        let (opts, failed, diags) = read("trojan, h.test, 443, password=p");
        assert!(opts.is_none() && !failed && diags.is_empty());
    }

    #[test]
    fn what_is_wrong_with_the_three_parameters_is_an_error() {
        let (_, failed, diags) = read("trojan, h.test, 443, password=p, shadow-tls-password=");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `shadow-tls-password` is empty"
            )]
        );
        let (_, failed, diags) = read(
            "trojan, h.test, 443, password=p, shadow-tls-password=s3cret, shadow-tls-version=4",
        );
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: invalid value `4` for `shadow-tls-version` (expected 2 / 3)"
            )]
        );
        let (_, failed, diags) = read(
            "trojan, h.test, 443, password=p, shadow-tls-password=s3cret, shadow-tls-version=3",
        );
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `shadow-tls-sni` is required when `shadow-tls-version=3`"
            )]
        );
        for bad in ["not a name", "192.0.2.1", "-x.test", ""] {
            let (_, failed, diags) = read(&format!(
                "trojan, h.test, 443, password=p, shadow-tls-password=s3cret, shadow-tls-sni={bad}"
            ));
            assert!(failed, "{bad}");
            assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM, "{bad}");
        }
        // the password is never quoted back
        assert!(diags.iter().all(|d| !d.message.contains("s3cret")));
    }

    #[test]
    fn the_other_two_do_nothing_without_a_password() {
        let (opts, failed, diags) = read(
            "trojan, h.test, 443, password=p, shadow-tls-sni=example.com, shadow-tls-version=3",
        );
        assert!(opts.is_none() && !failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `shadow-tls-sni` has no effect without `shadow-tls-password`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `shadow-tls-version` has no effect without `shadow-tls-password`; ignored"
                ),
            ]
        );
    }

    #[test]
    fn it_wraps_tcp_and_nothing_else() {
        use PolicyKind::*;
        for kind in [Tuic, TuicV5, Hysteria2, Masque, WireGuard, Tailscale] {
            assert!(!allowed_on(kind), "{kind:?}");
        }
        for kind in [
            Http,
            Https,
            H2Connect,
            Socks5,
            Socks5Tls,
            Shadowsocks,
            Snell,
            Vmess,
            Trojan,
            AnyTls,
            TrustTunnel,
            Ssh,
        ] {
            assert!(allowed_on(kind), "{kind:?}");
        }
        let (opts, failed, diags) =
            read("hysteria2, h.test, 443, password=p, shadow-tls-password=s3cret");
        assert!(opts.is_none() && failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: Shadow TLS cannot be combined with a `hysteria2` policy"
            )]
        );
    }
}
