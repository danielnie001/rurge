//! `tuic` and `tuic-v5` policy parameters (manual: Policies › TUIC; phase 2
//! M7 design 6.1): TLS over QUIC. The keyword is the version, and the two
//! authenticate differently — `tuic` (TUIC v4) with a token, `tuic-v5` with
//! a UUID and a password; they are not interchangeable (manual).

use super::common::CommonOpts;
use super::quic::{QuicOpts, read_quic, read_quic_tls};
use super::reader::ParamReader;
use super::required;
use super::secret::Secret;
use super::tls::TlsOpts;
use super::vmess::read_uuid;
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;
use crate::policy::PolicyKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuicSpec {
    /// TUIC always runs TLS over QUIC; `alpn` is `h3` unless written.
    pub tls: TlsOpts,
    pub quic: QuicOpts,
    /// The version the line's keyword names, with its credentials.
    pub version: TuicVersion,
}

/// A TUIC version and what it authenticates with (phase 2 M7 design 6.2,
/// 6.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TuicVersion {
    /// `tuic`: TUIC v4 and its `token`.
    V4 { token: Secret<String> },
    /// `tuic-v5`: the user's `uuid` and its `password`.
    V5 {
        uuid: Secret<[u8; 16]>,
        password: Secret<String>,
    },
}

/// Everything `tuic`- or `tuic-v5`-specific on the line, and `tfo` taken
/// out of `common` (`read_quic`). After an error was reported the returned
/// value is meaningless: the caller checks `r.has_errors()`.
///
/// The credentials are named-only, as the manual writes them, and never
/// quoted in a diagnostic: a positional value stays unread and is reported
/// as an extra positional value. The other version's credentials do not
/// apply (`W0028`).
pub fn read_tuic(
    r: &mut ParamReader<'_>,
    common: &mut CommonOpts,
    keystore: &[KeystoreItem],
) -> TuicSpec {
    let tls = read_quic_tls(r, keystore);
    let quic = read_quic(r, common);
    let kind = r.policy().kind;
    let (version, other, others) = if kind == PolicyKind::Tuic {
        let token = required(r, "token");
        let others = ["uuid", "password"].as_slice();
        (TuicVersion::V4 { token }, PolicyKind::TuicV5, others)
    } else {
        let uuid = read_uuid(r, "uuid");
        let password = required(r, "password");
        let others = ["token"].as_slice();
        (TuicVersion::V5 { uuid, password }, PolicyKind::Tuic, others)
    };
    for key in others {
        if r.has(key) {
            r.touch(key);
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                format!(
                    "`{key}` only applies to `{}` policies; ignored",
                    other.keyword()
                ),
            );
        }
    }
    if r.has("udp-relay") {
        r.touch("udp-relay");
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            format!(
                "`udp-relay` does not apply to `{}` policies (UDP is always relayed); ignored",
                kind.keyword()
            ),
        );
    }
    TuicSpec { tls, quic, version }
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

    const ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
    const BYTES: [u8; 16] = [
        0x02, 0x33, 0xd1, 0x1c, 0x15, 0xa4, 0x47, 0xd3, 0xad, 0xe3, 0x48, 0xff, 0xca, 0x0c, 0xe1,
        0x19,
    ];

    fn read(def: &str) -> (TuicSpec, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut common = read_common(&mut r, Applies::Proxy, &mut Notes::default());
        let spec = read_tuic(&mut r, &mut common, &[]);
        let failed = r.has_errors();
        (spec, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    fn v5(uuid: [u8; 16], password: &str) -> TuicVersion {
        TuicVersion::V5 {
            uuid: Secret::new(uuid),
            password: password.into(),
        }
    }

    #[test]
    fn the_manuals_examples() {
        let (spec, failed, diags) = read("tuic, 192.168.20.6, 443, token=pwd, alpn=h3");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            spec.version,
            TuicVersion::V4 {
                token: "pwd".into()
            }
        );
        assert_eq!(spec.quic, QuicOpts::default());
        assert_eq!(
            spec.tls,
            TlsOpts {
                alpn: vec!["h3".into()],
                ..TlsOpts::default()
            }
        );
        let (spec, failed, diags) = read(&format!(
            "tuic-v5, 192.168.20.6, 443, uuid={ID}, password=pwd, alpn=h3"
        ));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.version, v5(BYTES, "pwd"));
        assert_eq!(spec.quic, QuicOpts::default());
        // `h3` without `alpn` too (manual)
        let (spec, failed, _) = read("tuic, h.test, 443, token=pwd");
        assert!(!failed);
        assert_eq!(spec.tls.alpn, ["h3"]);
    }

    #[test]
    fn every_parameter() {
        let (spec, failed, diags) = read(&format!(
            "tuic-v5, h.test, 443, uuid={}, password=pw, port-hopping=443;20000-30000, port-hopping-interval=60, alpn=h3-29, sni=edge.test, skip-cert-verify=true",
            ID.replace('-', "").to_uppercase()
        ));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        // hyphens and case do not matter, as on `vmess`
        assert_eq!(spec.version, v5(BYTES, "pw"));
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
        let (spec, failed, diags) = read("tuic, h.test, 443, token=t, port-hopping=443");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            spec.quic.port_hopping.map(|hopping| hopping.interval),
            Some(DEFAULT_HOP_INTERVAL)
        );
    }

    #[test]
    fn the_credentials_are_required_named_and_never_quoted() {
        let (_, failed, diags) = read("tuic, h.test, 443");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `token` is required"
            )]
        );
        let (_, failed, diags) = read("tuic, h.test, 443, hunter2, token=");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `token` is required"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #1 ignored"
                ),
            ]
        );
        let (_, failed, diags) = read("tuic-v5, h.test, 443");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `uuid` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `password` is required"
                ),
            ]
        );
        // positional values are not read as the credentials
        let (_, failed, diags) = read(&format!("tuic-v5, h.test, 443, {ID}, hunter2, password="));
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `uuid` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `password` is required"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #1 ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #2 ignored"
                ),
            ]
        );
    }

    #[test]
    fn a_uuid_that_is_not_one_is_an_error_that_never_quotes_it() {
        for bad in [
            "not-a-uuid",
            "0233d11c-15a4-47d3-ade3-48ffca0ce11", // one digit short
            "0233d11c15a4-47d3-ade3-48ffca0ce119-", // hyphens in the wrong places
            "0233d11c-15a4-47d3-ade3-48ffca0ce11g", // not hex
            "{0233d11c-15a4-47d3-ade3-48ffca0ce119}", // braces
            "",
        ] {
            let (_, failed, diags) = read(&format!(
                "tuic-v5, h.test, 443, uuid=\"{bad}\", password=pw"
            ));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `uuid` is not a valid UUID"
                )],
                "{bad}"
            );
        }
    }

    /// The two versions are not interchangeable (manual): what one
    /// authenticates with, the other does not use.
    #[test]
    fn the_other_versions_credentials_do_not_apply() {
        let (spec, failed, diags) = read(&format!(
            "tuic, h.test, 443, token=tok, uuid={ID}, password=pw"
        ));
        assert!(!failed);
        assert_eq!(
            spec.version,
            TuicVersion::V4 {
                token: "tok".into()
            }
        );
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `uuid` only applies to `tuic-v5` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `password` only applies to `tuic-v5` policies; ignored"
                ),
            ]
        );
        let (spec, failed, diags) = read(&format!(
            "tuic-v5, h.test, 443, uuid={ID}, password=pw, token=tok"
        ));
        assert!(!failed);
        assert_eq!(spec.version, v5(BYTES, "pw"));
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `token` only applies to `tuic` policies; ignored"
            )]
        );
        // a v5 line under the v4 keyword lacks the token
        let (_, failed, diags) = read(&format!("tuic, h.test, 443, uuid={ID}, password=pw"));
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `token` is required"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `uuid` only applies to `tuic-v5` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `password` only applies to `tuic-v5` policies; ignored"
                ),
            ]
        );
    }

    /// UDP is relayed whatever the line says (manual).
    #[test]
    fn udp_relay_does_not_apply() {
        for (def, kind) in [
            (
                "tuic, h.test, 443, token=t, udp-relay=true".to_string(),
                "tuic",
            ),
            (
                format!("tuic-v5, h.test, 443, uuid={ID}, password=pw, udp-relay=false"),
                "tuic-v5",
            ),
        ] {
            let (_, failed, diags) = read(&def);
            assert!(!failed, "{def}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!(
                        "policy `P`: `udp-relay` does not apply to `{kind}` policies (UDP is always relayed); ignored"
                    )
                    .as_str()
                )]
            );
        }
    }

    #[test]
    fn the_spec_does_not_print_its_credentials() {
        let (v4, _, _) = read("tuic, h.test, 443, token=s3cretTok");
        let (v5, _, _) = read(&format!(
            "tuic-v5, h.test, 443, uuid={ID}, password=hunter2"
        ));
        for printed in [format!("{v4:?}"), format!("{v5:?}")] {
            assert!(printed.contains("Secret(***)"), "{printed}");
            assert!(
                !printed.contains("s3cretTok") && !printed.contains("hunter2"),
                "{printed}"
            );
            // 0xd1, the third byte of the UUID, as `Debug` would print it
            assert!(!printed.contains("209"), "{printed}");
        }
    }
}
