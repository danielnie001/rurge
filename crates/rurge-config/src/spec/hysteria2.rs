//! `hysteria2` policy parameters (manual: Policies › Hysteria 2; phase 2 M7
//! design 5.1): TLS over QUIC, each UDP packet of which may be obfuscated
//! as well (Salamander or Gecko).

use super::common::CommonOpts;
use super::quic::{QuicOpts, read_quic, read_quic_tls};
use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::TlsOpts;
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hysteria2Spec {
    /// Hysteria 2 always runs TLS over QUIC; `alpn` is `h3` unless written.
    pub tls: TlsOpts,
    pub quic: QuicOpts,
    pub password: Secret<String>,
    /// `download-bandwidth` in Mbps, `0` when not written: a hint for the
    /// server's congestion control; the client's own sending does not
    /// follow it (M7-D10).
    pub download_bandwidth: u32,
    /// `salamander-password` or `gecko-password`.
    pub obfs: Option<Hysteria2Obfs>,
}

/// The obfuscation every UDP packet of the connection goes through (phase 2
/// M7 design 5.4), with its password.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hysteria2Obfs {
    Salamander(Secret<String>),
    Gecko(Secret<String>),
}

/// Everything `hysteria2`-specific on the line, and `tfo` taken out of
/// `common` (`read_quic`). After an error was reported the returned value
/// is meaningless: the caller checks `r.has_errors()`.
///
/// The passwords are named-only, as the manual writes them, and never
/// quoted in a diagnostic: a positional value stays unread and is reported
/// as an extra positional value.
pub fn read_hysteria2(
    r: &mut ParamReader<'_>,
    common: &mut CommonOpts,
    keystore: &[KeystoreItem],
) -> Hysteria2Spec {
    let tls = read_quic_tls(r, keystore);
    let quic = read_quic(r, common);
    let password = r.str("password").unwrap_or_default();
    if password.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`password` is required".to_string(),
        );
    }
    let download_bandwidth = r
        .number::<u32>("download-bandwidth", "a whole number of Mbps")
        .unwrap_or(0);
    let obfs = read_obfuscation(r);
    if r.has("udp-relay") {
        r.touch("udp-relay");
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            "`udp-relay` does not apply to `hysteria2` policies (UDP is always relayed); ignored"
                .to_string(),
        );
    }
    Hysteria2Spec {
        tls,
        quic,
        password: password.into(),
        download_bandwidth,
        obfs,
    }
}

/// `salamander-password` or `gecko-password`: not both (manual), and not
/// empty.
fn read_obfuscation(r: &mut ParamReader<'_>) -> Option<Hysteria2Obfs> {
    let salamander = r.str("salamander-password");
    let gecko = r.str("gecko-password");
    for (key, password) in [
        ("salamander-password", salamander),
        ("gecko-password", gecko),
    ] {
        if password.is_some_and(str::is_empty) {
            r.error(codes::E_INVALID_POLICY_PARAM, format!("`{key}` is empty"));
        }
    }
    match (salamander, gecko) {
        (Some(_), Some(_)) => {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "`salamander-password` cannot be combined with `gecko-password`".to_string(),
            );
            None
        }
        (Some(password), None) => Some(Hysteria2Obfs::Salamander(password.into())),
        (None, Some(password)) => Some(Hysteria2Obfs::Gecko(password.into())),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::Sni;
    use crate::spec::common::{Applies, Notes, read_common};
    use crate::spec::quic::{DEFAULT_HOP_INTERVAL, PortHopping};
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    fn read(def: &str) -> (Hysteria2Spec, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut common = read_common(&mut r, Applies::Proxy, &mut Notes::default());
        let spec = read_hysteria2(&mut r, &mut common, &[]);
        let failed = r.has_errors();
        (spec, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    #[test]
    fn the_manuals_example() {
        let (spec, failed, diags) =
            read("hysteria2, 192.168.20.6, 443, password=pwd, download-bandwidth=100");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.password.expose(), "pwd");
        assert_eq!(spec.download_bandwidth, 100);
        assert_eq!(spec.obfs, None);
        assert_eq!(spec.quic, QuicOpts::default());
        assert_eq!(
            spec.tls,
            TlsOpts {
                alpn: vec!["h3".into()],
                ..TlsOpts::default()
            }
        );
        // without `download-bandwidth` the server is told 0: unknown
        let (spec, failed, _) = read("hysteria2, h.test, 443, password=pw");
        assert!(!failed);
        assert_eq!(spec.download_bandwidth, 0);
    }

    #[test]
    fn every_parameter() {
        let (spec, failed, diags) = read(
            "hysteria2, h.test, 443, password=pw, download-bandwidth=0, gecko-password=0bfs, port-hopping=443;20000-30000, port-hopping-interval=60, alpn=h3-29, sni=edge.test, skip-cert-verify=true",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.download_bandwidth, 0);
        assert_eq!(spec.obfs, Some(Hysteria2Obfs::Gecko("0bfs".into())));
        assert_eq!(
            spec.quic.port_hopping,
            Some(PortHopping {
                ports: vec![443..=443, 20000..=30000],
                interval: Duration::from_secs(60),
            })
        );
        assert_eq!(spec.tls.alpn, ["h3-29"]);
        assert_eq!(spec.tls.sni, Sni::Name("edge.test".into()));
        assert!(spec.tls.skip_cert_verify);
        let (spec, _, _) =
            read("hysteria2, h.test, 443, password=pw, salamander-password=0bfs, port-hopping=443");
        assert_eq!(spec.obfs, Some(Hysteria2Obfs::Salamander("0bfs".into())));
        assert_eq!(
            spec.quic.port_hopping.map(|hopping| hopping.interval),
            Some(DEFAULT_HOP_INTERVAL)
        );
    }

    #[test]
    fn the_password_is_required_named_and_never_quoted() {
        let (_, failed, diags) = read("hysteria2, h.test, 443");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `password` is required"
            )]
        );
        let (_, failed, diags) = read("hysteria2, h.test, 443, hunter2, password=");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `password` is required"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #1 ignored"
                ),
            ]
        );
        assert!(diags.iter().all(|d| !d.message.contains("hunter2")));
    }

    #[test]
    fn download_bandwidth_is_a_whole_number_of_mbps() {
        for bad in ["-1", "1.5", "100Mbps", "", "4294967296"] {
            let (_, failed, diags) = read(&format!(
                "hysteria2, h.test, 443, password=pw, download-bandwidth=\"{bad}\""
            ));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `P`: invalid value `{bad}` for `download-bandwidth` (expected a whole number of Mbps)"
                    )
                    .as_str()
                )]
            );
        }
    }

    /// The two obfuscations are mutually exclusive (manual) and neither
    /// password is ever quoted.
    #[test]
    fn salamander_or_gecko_not_both_and_not_empty() {
        let (_, failed, diags) = read(
            "hysteria2, h.test, 443, password=pw, salamander-password=s3cretA, gecko-password=s3cretB",
        );
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `salamander-password` cannot be combined with `gecko-password`"
            )]
        );
        assert!(
            diags
                .iter()
                .all(|d| !d.message.contains("s3cretA") && !d.message.contains("s3cretB"))
        );
        for key in ["salamander-password", "gecko-password"] {
            let (_, failed, diags) = read(&format!("hysteria2, h.test, 443, password=pw, {key}="));
            assert!(failed, "{key}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!("policy `P`: `{key}` is empty").as_str()
                )]
            );
        }
    }

    /// UDP rides on QUIC datagrams whatever the line says (manual).
    #[test]
    fn udp_relay_does_not_apply() {
        let (_, failed, diags) = read("hysteria2, h.test, 443, password=pw, udp-relay=true");
        assert!(!failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `udp-relay` does not apply to `hysteria2` policies (UDP is always relayed); ignored"
            )]
        );
    }

    #[test]
    fn the_spec_does_not_print_its_passwords() {
        for obfs in ["salamander-password", "gecko-password"] {
            let (spec, _, _) = read(&format!(
                "hysteria2, h.test, 443, password=hunter2, {obfs}=s3cret0bfs"
            ));
            let printed = format!("{spec:?}");
            assert!(printed.contains("Secret(***)"), "{printed}");
            assert!(
                !printed.contains("hunter2") && !printed.contains("s3cret0bfs"),
                "{printed}"
            );
        }
    }
}
