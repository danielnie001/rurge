//! Typed view of `[Proxy]` policy parameters (phase 2 M1 design §4).

pub mod common;
pub mod http;
pub mod reader;
pub mod socks5;
pub mod tls;

pub use common::{Applies, CommonOpts, IpVersion, Tristate};
pub use http::{HeaderPart, HeaderTemplate, HttpSpec};
pub use reader::ParamReader;
pub use socks5::Socks5Spec;
pub use tls::{Sni, TlsOpts};

use crate::diagnostic::{Diagnostic, codes};
use crate::keystore::KeystoreItem;
use crate::policy::{Builtin, PolicyKind, ProxyPolicy};
use crate::span::Span;
use crate::types::HostName;
use common::{Notes, read_common};

/// What a name on a policy line refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    Policy(PolicyKind),
    Group,
    Builtin(Builtin),
}

pub struct SpecEnv<'a> {
    pub keystore: &'a [KeystoreItem],
    pub lookup: &'a dyn Fn(&str) -> Option<NameKind>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtoSpec {
    Direct,
    Reject(Builtin),
    Http(HttpSpec),
    Socks5(Socks5Spec),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicySpec {
    pub name: String,
    pub kind: PolicyKind,
    pub server: Option<HostName>,
    pub port: Option<u16>,
    pub common: CommonOpts,
    pub proto: ProtoSpec,
    pub span: Span,
}

#[derive(Debug, Default)]
pub struct SpecOutcome {
    /// `None` when the policy type has no spec yet, or when an error was found.
    pub spec: Option<PolicySpec>,
    pub diagnostics: Vec<Diagnostic>,
    /// Parameters present on the line that are parsed but do nothing yet;
    /// the caller reports each name once per load (`W0029`).
    pub inert: Vec<&'static str>,
    /// iOS-only parameters present on the line (`W0004`, once per load).
    pub ios_only: Vec<&'static str>,
}

/// Named `username=` / `password=` win over the positional pair.
fn read_credentials(r: &mut ParamReader<'_>) -> (Option<String>, Option<String>) {
    let positional = (r.positional(0), r.positional(1));
    let username = r.str("username").or(positional.0).map(str::to_string);
    let password = r.str("password").or(positional.1).map(str::to_string);
    (username, password)
}

fn check_underlying(r: &mut ParamReader<'_>, common: &mut CommonOpts, env: &SpecEnv<'_>) {
    let Some(name) = common.underlying_proxy.clone() else {
        return;
    };
    match (env.lookup)(&name) {
        None => r.error(
            codes::E_UNKNOWN_POLICY_REF,
            format!("`underlying-proxy` references unknown policy `{name}`"),
        ),
        // DIRECT is the absence of a chain
        Some(NameKind::Builtin(Builtin::Direct)) => common.underlying_proxy = None,
        Some(NameKind::Builtin(_)) => r.invalid(
            "underlying-proxy",
            &name,
            "a proxy policy or a policy group",
        ),
        Some(NameKind::Policy(_) | NameKind::Group) => {}
    }
}

pub fn to_spec(policy: &ProxyPolicy, env: &SpecEnv<'_>) -> SpecOutcome {
    let mut r = ParamReader::new(policy);
    let mut notes = Notes::default();
    let (mut common, proto) = match policy.kind {
        PolicyKind::Direct => {
            let common = read_common(&mut r, Applies::Direct, &mut notes);
            tls::refuse_tls(&mut r);
            (common, ProtoSpec::Direct)
        }
        PolicyKind::Reject
        | PolicyKind::RejectDrop
        | PolicyKind::RejectNoDrop
        | PolicyKind::RejectTinyGif => {
            let common = read_common(&mut r, Applies::Reject, &mut notes);
            let builtin = match policy.kind {
                PolicyKind::RejectDrop => Builtin::RejectDrop,
                PolicyKind::RejectNoDrop => Builtin::RejectNoDrop,
                PolicyKind::RejectTinyGif => Builtin::RejectTinyGif,
                _ => Builtin::Reject,
            };
            (common, ProtoSpec::Reject(builtin))
        }
        PolicyKind::Http | PolicyKind::Https => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let tls = if policy.kind == PolicyKind::Https {
                Some(tls::read_tls(&mut r, env.keystore))
            } else {
                tls::refuse_tls(&mut r);
                None
            };
            tls::note_shadow_tls(&mut r, &mut notes);
            let (username, password) = read_credentials(&mut r);
            let always_use_connect = r.bool("always-use-connect").unwrap_or(false);
            let mut headers = Vec::new();
            if let Some(v) = r.str("headers") {
                match HeaderTemplate::parse_list(v) {
                    Ok(list) => headers = list,
                    Err(why) => r.error(
                        codes::E_INVALID_POLICY_PARAM,
                        format!("invalid `headers`: {why}"),
                    ),
                }
            }
            (
                common,
                ProtoSpec::Http(HttpSpec {
                    tls,
                    username,
                    password,
                    always_use_connect,
                    headers,
                }),
            )
        }
        PolicyKind::Socks5 | PolicyKind::Socks5Tls => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let tls = if policy.kind == PolicyKind::Socks5Tls {
                Some(tls::read_tls(&mut r, env.keystore))
            } else {
                tls::refuse_tls(&mut r);
                None
            };
            tls::note_shadow_tls(&mut r, &mut notes);
            let (username, password) = read_credentials(&mut r);
            for (what, value) in [("username", &username), ("password", &password)] {
                if value
                    .as_ref()
                    .is_some_and(|v| v.len() > socks5::MAX_CREDENTIAL)
                {
                    // never echo the value
                    r.error(
                        codes::E_INVALID_POLICY_PARAM,
                        format!(
                            "`{what}` is longer than the {} bytes SOCKS5 allows",
                            socks5::MAX_CREDENTIAL
                        ),
                    );
                }
            }
            let udp_relay = r.bool("udp-relay").unwrap_or(false);
            if udp_relay {
                notes.inert.insert(0, "udp-relay");
            }
            (
                common,
                ProtoSpec::Socks5(Socks5Spec {
                    tls,
                    username,
                    password,
                    udp_relay,
                }),
            )
        }
        _ => return SpecOutcome::default(),
    };
    if matches!(proto, ProtoSpec::Http(_) | ProtoSpec::Socks5(_)) {
        check_underlying(&mut r, &mut common, env);
    }
    let failed = r.has_errors();
    let diagnostics = r.finish();
    let spec = (!failed).then(|| PolicySpec {
        name: policy.name.clone(),
        kind: policy.kind,
        server: policy.server.clone(),
        port: policy.port,
        common,
        proto,
        span: policy.span.clone(),
    });
    SpecOutcome {
        spec,
        diagnostics,
        inert: notes.inert,
        ios_only: notes.ios_only,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::codes;
    use crate::keystore::{KeystoreItem, KeystoreType};
    use crate::policy::{Builtin, PolicyKind, parse_policy};
    use crate::span::Span;
    use crate::types::HostName;
    use std::path::Path;
    use std::sync::Arc;

    fn span() -> Span {
        Span::new(Arc::from(Path::new("p.conf")), 3)
    }

    fn outcome(name: &str, def: &str) -> SpecOutcome {
        let p = parse_policy(name, def, &span()).unwrap();
        let keystore = vec![KeystoreItem {
            name: "cert1".into(),
            kind: KeystoreType::P12,
            base64: "AAAA".into(),
            password: Some("x".into()),
            unknown: Vec::new(),
            span: span(),
        }];
        let lookup = |n: &str| match n {
            "Entry" => Some(NameKind::Policy(PolicyKind::Socks5)),
            "Pick" => Some(NameKind::Group),
            "DIRECT" => Some(NameKind::Builtin(Builtin::Direct)),
            "REJECT" => Some(NameKind::Builtin(Builtin::Reject)),
            _ => None,
        };
        to_spec(
            &p,
            &SpecEnv {
                keystore: &keystore,
                lookup: &lookup,
            },
        )
    }

    #[test]
    fn manual_examples() {
        let o = outcome("ProxyHTTPS", "https, 1.2.3.4, 443, username, password");
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let spec = o.spec.unwrap();
        assert_eq!(spec.name, "ProxyHTTPS");
        assert_eq!(spec.kind, PolicyKind::Https);
        assert_eq!(spec.server, Some(HostName::parse("1.2.3.4")));
        assert_eq!(spec.port, Some(443));
        let ProtoSpec::Http(http) = &spec.proto else {
            panic!("{:?}", spec.proto)
        };
        assert_eq!(http.username.as_deref(), Some("username"));
        assert_eq!(http.password.as_deref(), Some("password"));
        assert_eq!(http.tls, Some(TlsOpts::default()));
        assert!(!http.always_use_connect);

        let o = outcome(
            "ProxySOCKS5TLS",
            "socks5-tls, 1.2.3.4, 443, username, password, skip-cert-verify=false",
        );
        let ProtoSpec::Socks5(s) = &o.spec.unwrap().proto else {
            panic!()
        };
        assert!(s.tls.is_some() && !s.udp_relay);

        let o = outcome(
            "Plain",
            "http, proxy.example.com, 8080, always-use-connect=true, headers=X-A:1;X-B:2",
        );
        let ProtoSpec::Http(http) = &o.spec.unwrap().proto else {
            panic!()
        };
        assert!(http.tls.is_none() && http.always_use_connect);
        assert_eq!(http.headers.len(), 2);
    }

    #[test]
    fn named_credentials_win_over_positional_ones() {
        let o = outcome(
            "P",
            "socks5, h, 1080, posuser, pospass, username=named, password=secret",
        );
        let ProtoSpec::Socks5(s) = &o.spec.unwrap().proto else {
            panic!()
        };
        assert_eq!(
            (s.username.as_deref(), s.password.as_deref()),
            (Some("named"), Some("secret"))
        );
    }

    #[test]
    fn aliases_get_a_spec_too() {
        let o = outcome("Corp", "direct, interface=utun0");
        let spec = o.spec.unwrap();
        assert_eq!(spec.proto, ProtoSpec::Direct);
        assert_eq!(spec.common.interface.as_deref(), Some("utun0"));
        assert_eq!((spec.server, spec.port), (None, None));
        let o = outcome("Block", "reject-tinygif");
        assert_eq!(
            o.spec.unwrap().proto,
            ProtoSpec::Reject(Builtin::RejectTinyGif)
        );
    }

    #[test]
    fn protocols_without_a_spec_are_left_alone() {
        let o = outcome(
            "SS",
            "ss, h, 8388, encrypt-method=aes-128-gcm, password=x, mystery=1",
        );
        assert!(o.spec.is_none() && o.diagnostics.is_empty() && o.inert.is_empty());
    }

    #[test]
    fn underlying_proxy_references() {
        assert_eq!(
            outcome("P", "http, h, 80, underlying-proxy=Pick")
                .spec
                .unwrap()
                .common
                .underlying_proxy
                .as_deref(),
            Some("Pick")
        );
        // DIRECT means "no chain"
        assert_eq!(
            outcome("P", "http, h, 80, underlying-proxy=DIRECT")
                .spec
                .unwrap()
                .common
                .underlying_proxy,
            None
        );
        let o = outcome("P", "http, h, 80, underlying-proxy=Ghost");
        assert!(o.spec.is_none());
        assert_eq!(o.diagnostics[0].code, codes::E_UNKNOWN_POLICY_REF);
        assert_eq!(
            o.diagnostics[0].message,
            "policy `P`: `underlying-proxy` references unknown policy `Ghost`"
        );
        let o = outcome("P", "http, h, 80, underlying-proxy=REJECT");
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
    }

    #[test]
    fn notes_unknowns_and_limits() {
        let o = outcome(
            "P",
            "socks5, h, 1080, udp-relay=true, shadow-tls-password=pw, mystery=1",
        );
        assert_eq!(o.inert, ["udp-relay", "shadow-tls-password"]);
        let warnings: Vec<&str> = o.diagnostics.iter().map(|d| d.code).collect();
        assert_eq!(warnings, [codes::W_UNKNOWN_KEY]);
        assert!(o.spec.is_some(), "warnings do not drop the spec");

        let long = "u".repeat(256);
        let o = outcome("P", &format!("socks5, h, 1080, {long}, pw"));
        assert!(o.spec.is_none());
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
        assert!(
            !o.diagnostics[0].message.contains(&long),
            "credentials are never echoed"
        );

        let o = outcome("P", "http, h, 80, headers=Broken");
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
    }
}
