//! `trojan` policy parameters (manual: Policies › Trojan).

use super::reader::ParamReader;
use super::tls::{TlsOpts, read_tls};
use super::ws::{WsOpts, read_ws};
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrojanSpec {
    /// Trojan always runs over TLS.
    pub tls: TlsOpts,
    pub password: String,
    pub ws: Option<WsOpts>,
}

/// Everything `trojan`-specific on the line. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The password is named-only (`password=`), as the manual writes it: a
/// positional value stays unread and is reported as an extra positional
/// value, never quoted.
pub fn read_trojan(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TrojanSpec {
    let tls = read_tls(r, keystore);
    let password = r.str("password").unwrap_or_default().to_string();
    if password.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`password` is required".to_string(),
        );
    }
    let ws = read_ws(r);
    TrojanSpec { tls, password, ws }
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

    fn read(def: &str) -> (TrojanSpec, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let spec = read_trojan(&mut r, &[]);
        let failed = r.has_errors();
        (spec, failed, r.finish())
    }

    #[test]
    fn the_manuals_example_and_the_tls_parameters() {
        let (spec, failed, diags) = read("trojan, 192.0.2.15, 443, password=pwd, sni=example.com");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.password, "pwd");
        assert_eq!(spec.tls.sni, Sni::Name("example.com".into()));
        assert!(spec.ws.is_none());
        let (spec, failed, _) = read("trojan, h.test, 443, password=p, ws=true, ws-path=/t");
        assert!(!failed);
        assert_eq!(spec.ws.unwrap().path, "/t");
    }

    #[test]
    fn a_missing_password_is_an_error_and_a_positional_one_is_not_read() {
        let (_, failed, diags) = read("trojan, h.test, 443");
        assert!(failed);
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `password` is required"
            )
        );
        let (_, failed, diags) = read("trojan, h.test, 443, hunter2");
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
}
