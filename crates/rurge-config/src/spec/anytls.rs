//! `anytls` policy parameters (manual: Policies › AnyTLS).

use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::{TlsOpts, read_tls};
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnyTlsSpec {
    /// AnyTLS always runs over TLS.
    pub tls: TlsOpts,
    pub password: Secret<String>,
    /// `reuse`: keep a session for the next stream (the protocol's default).
    pub reuse: bool,
}

/// Everything `anytls`-specific on the line. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The password is named-only (`password=`), as the manual writes it: a
/// positional value stays unread and is reported as an extra positional
/// value, never quoted.
pub fn read_anytls(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> AnyTlsSpec {
    let tls = read_tls(r, keystore);
    let password = r.str("password").unwrap_or_default();
    if password.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`password` is required".to_string(),
        );
    }
    let reuse = r.bool("reuse").unwrap_or(true);
    AnyTlsSpec {
        tls,
        password: password.into(),
        reuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::Sni;
    use std::path::Path;
    use std::sync::Arc;

    fn read(def: &str) -> (AnyTlsSpec, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let spec = read_anytls(&mut r, &[]);
        let failed = r.has_errors();
        (spec, failed, r.finish())
    }

    #[test]
    fn the_manuals_example_and_reuse() {
        let (spec, failed, diags) = read("anytls, 192.168.20.6, 443, password=pwd");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.password.expose(), "pwd");
        assert!(spec.reuse, "reuse is on unless turned off");
        let (spec, failed, _) = read("anytls, h.test, 443, password=p, reuse=false, sni=edge.test");
        assert!(!failed);
        assert!(!spec.reuse);
        assert_eq!(spec.tls.sni, Sni::Name("edge.test".into()));
    }

    #[test]
    fn a_missing_password_is_an_error_and_a_positional_one_is_not_read() {
        let (_, failed, diags) = read("anytls, h.test, 443");
        assert!(failed);
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `password` is required"
            )
        );
        let (_, failed, diags) = read("anytls, h.test, 443, hunter2");
        assert!(failed);
        let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "policy `P`: `password` is required",
                "policy `P`: unexpected positional value #1 ignored"
            ]
        );
        assert!(messages.iter().all(|m| !m.contains("hunter2")));
    }

    #[test]
    fn a_bad_boolean_is_an_error_and_the_spec_does_not_print_the_password() {
        let (_, failed, _) = read("anytls, h.test, 443, password=p, reuse=maybe");
        assert!(failed);
        let (spec, _, _) = read("anytls, h.test, 443, password=hunter2");
        let printed = format!("{spec:?}");
        assert!(
            printed.contains("Secret(***)") && !printed.contains("hunter2"),
            "{printed}"
        );
    }
}
