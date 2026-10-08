//! `masque` policy parameters (manual: Policies › MASQUE; phase 2 M7 design
//! 7.1): HTTP/3 over QUIC, every TCP connection a CONNECT stream and UDP
//! relayed with CONNECT-UDP (RFC 9298) whatever the line says.

use super::common::CommonOpts;
use super::quic::{QuicOpts, read_quic, read_quic_tls};
use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::TlsOpts;
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MasqueSpec {
    /// MASQUE always runs TLS over QUIC; `alpn` is `h3` unless written.
    pub tls: TlsOpts,
    pub quic: QuicOpts,
    /// `username` and `password`, sent as Basic authorization with every
    /// CONNECT request; without them no authorization is sent (manual).
    pub credentials: Option<MasqueCredentials>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MasqueCredentials {
    pub username: Secret<String>,
    pub password: Secret<String>,
}

/// Everything `masque`-specific on the line, and `tfo` taken out of
/// `common` (`read_quic`). After an error was reported the returned value
/// is meaningless: the caller checks `r.has_errors()`.
///
/// The credentials are optional but go together. They are named-only, as
/// the manual writes them, and never quoted in a diagnostic: a positional
/// value stays unread and is reported as an extra positional value. The
/// manual gives MASQUE no `headers` and no `max-streams`: here they are
/// unknown parameters, as anything else not read.
pub fn read_masque(
    r: &mut ParamReader<'_>,
    common: &mut CommonOpts,
    keystore: &[KeystoreItem],
) -> MasqueSpec {
    let tls = read_quic_tls(r, keystore);
    let quic = read_quic(r, common);
    let credentials = read_masque_credentials(r);
    if r.has("udp-relay") {
        r.touch("udp-relay");
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            "`udp-relay` does not apply to `masque` policies (UDP is always relayed); ignored"
                .to_string(),
        );
    }
    MasqueSpec {
        tls,
        quic,
        credentials,
    }
}

/// `username` and `password`: both or neither, and neither empty.
fn read_masque_credentials(r: &mut ParamReader<'_>) -> Option<MasqueCredentials> {
    let username = r.str("username");
    let password = r.str("password");
    for (key, value) in [("username", username), ("password", password)] {
        if value.is_some_and(str::is_empty) {
            r.error(codes::E_INVALID_POLICY_PARAM, format!("`{key}` is empty"));
        }
    }
    let (missing, written) = match (username, password) {
        (Some(username), Some(password)) => {
            return Some(MasqueCredentials {
                username: username.into(),
                password: password.into(),
            });
        }
        (None, None) => return None,
        (Some(_), None) => ("password", "username"),
        (None, Some(_)) => ("username", "password"),
    };
    r.error(
        codes::E_INVALID_POLICY_PARAM,
        format!("`{missing}` is required with `{written}`"),
    );
    None
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

    fn read(def: &str) -> (MasqueSpec, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut common = read_common(&mut r, Applies::Proxy, &mut Notes::default());
        let spec = read_masque(&mut r, &mut common, &[]);
        let failed = r.has_errors();
        (spec, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    fn credentials(username: &str, password: &str) -> Option<MasqueCredentials> {
        Some(MasqueCredentials {
            username: username.into(),
            password: password.into(),
        })
    }

    #[test]
    fn the_manuals_example() {
        let (spec, failed, diags) = read("masque, example.com, 443, username=user, password=pass");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.credentials, credentials("user", "pass"));
        assert_eq!(spec.quic, QuicOpts::default());
        // `h3` unless written (manual)
        assert_eq!(
            spec.tls,
            TlsOpts {
                alpn: vec!["h3".into()],
                ..TlsOpts::default()
            }
        );
    }

    #[test]
    fn every_parameter() {
        let (spec, failed, diags) = read(
            "masque, h.test, 443, username=u, password=pw, port-hopping=443;20000-30000, port-hopping-interval=60, alpn=h3-29, sni=edge.test, skip-cert-verify=true",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.credentials, credentials("u", "pw"));
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
        let (spec, failed, diags) = read("masque, h.test, 443, port-hopping=443");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            spec.quic.port_hopping.map(|hopping| hopping.interval),
            Some(DEFAULT_HOP_INTERVAL)
        );
    }

    /// Without credentials no authorization is sent (manual): nothing to
    /// say about the line.
    #[test]
    fn the_credentials_are_optional() {
        let (spec, failed, diags) = read("masque, example.com, 443");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.credentials, None);
    }

    /// One without the other, or one written empty, is an error that names
    /// the key and never quotes a value.
    #[test]
    fn the_credentials_go_together_and_are_never_quoted() {
        for (def, expected) in [
            (
                "masque, h.test, 443, username=s3cretUser",
                &["`password` is required with `username`"][..],
            ),
            (
                "masque, h.test, 443, password=hunter2",
                &["`username` is required with `password`"],
            ),
            (
                "masque, h.test, 443, username=s3cretUser, password=",
                &["`password` is empty"],
            ),
            (
                "masque, h.test, 443, username=, password=hunter2",
                &["`username` is empty"],
            ),
            (
                "masque, h.test, 443, username=, password=",
                &["`username` is empty", "`password` is empty"],
            ),
            (
                "masque, h.test, 443, username=",
                &[
                    "`username` is empty",
                    "`password` is required with `username`",
                ],
            ),
        ] {
            let (_, failed, diags) = read(def);
            assert!(failed, "{def}");
            let expected: Vec<(&str, String)> = expected
                .iter()
                .map(|text| (codes::E_INVALID_POLICY_PARAM, format!("policy `P`: {text}")))
                .collect();
            let found: Vec<(&str, String)> =
                diags.iter().map(|d| (d.code, d.message.clone())).collect();
            assert_eq!(found, expected, "{def}");
        }
    }

    /// Only the named form is read, as the manual writes it: positional
    /// values are no credentials, and are said to be ignored, unquoted.
    #[test]
    fn positional_values_are_not_the_credentials() {
        let (spec, failed, diags) = read("masque, h.test, 443, s3cretUser, hunter2");
        assert!(!failed);
        assert_eq!(spec.credentials, None);
        assert_eq!(
            messages(&diags),
            [
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

    /// UDP is relayed whatever the line says (manual), and QUIC opens no TCP
    /// connection to open fast.
    #[test]
    fn udp_relay_and_tfo_do_not_apply() {
        let (_, failed, diags) = read("masque, h.test, 443, udp-relay=true, tfo=true");
        assert!(!failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `tfo` does not apply to `masque` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `udp-relay` does not apply to `masque` policies (UDP is always relayed); ignored"
                ),
            ]
        );
    }

    /// The manual lists neither for MASQUE: no parameters of it.
    #[test]
    fn headers_and_max_streams_are_unknown_parameters() {
        let (spec, failed, diags) =
            read("masque, h.test, 443, username=u, password=pw, headers=X-A:1, max-streams=5");
        assert!(!failed);
        assert_eq!(spec.credentials, credentials("u", "pw"));
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unknown parameter `headers` ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unknown parameter `max-streams` ignored"
                ),
            ]
        );
    }

    #[test]
    fn the_spec_does_not_print_its_credentials() {
        let (spec, _, _) = read("masque, h.test, 443, username=s3cretUser, password=hunter2");
        let printed = format!("{spec:?}");
        assert!(printed.contains("Secret(***)"), "{printed}");
        assert!(
            !printed.contains("s3cretUser") && !printed.contains("hunter2"),
            "{printed}"
        );
    }
}
