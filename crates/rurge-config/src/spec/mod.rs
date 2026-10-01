//! Typed view of `[Proxy]` policy parameters (phase 2 M1 design §4).

pub mod anytls;
pub mod common;
pub mod external;
pub mod group;
pub mod http;
pub mod obfs;
pub mod reader;
pub mod secret;
pub mod shadow_tls;
pub mod snell;
pub mod socks5;
pub mod ss;
pub mod ssh;
pub mod tls;
pub mod trojan;
pub mod vmess;
pub mod wireguard;
pub mod ws;

pub use anytls::AnyTlsSpec;
pub use common::{Applies, CommonOpts, IpVersion, Tristate};
pub use external::ExternalSpec;
pub use group::{
    GroupOutcome, GroupSpec, ImportOpts, PolicyPath, Priority, TestOpts, to_group_spec,
};
pub use http::{HeaderPart, HeaderTemplate, HttpSpec};
pub use obfs::{ObfsMode, ObfsOpts};
pub use reader::ParamReader;
pub use secret::Secret;
pub use shadow_tls::{ShadowTlsOpts, ShadowTlsVersion};
pub use snell::{SnellSpec, SnellVersion};
pub use socks5::Socks5Spec;
pub use ss::{SsMethod, SsSpec};
pub use ssh::{HostKeyPin, SshSpec};
pub use tls::{Sni, TlsOpts};
pub use trojan::TrojanSpec;
pub use vmess::{VmessCipher, VmessSpec};
pub use wireguard::WireGuardSpec;
pub use ws::WsOpts;

use crate::diagnostic::{Diagnostic, codes};
use crate::keystore::KeystoreItem;
use crate::policy::{Builtin, PolicyKind, ProxyPolicy};
use crate::span::Span;
use crate::types::HostName;
use crate::wireguard::WireGuardSection;
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
    /// The `[WireGuard <name>]` sections `section-name` may name.
    pub wireguard: &'a [WireGuardSection],
    pub lookup: &'a dyn Fn(&str) -> Option<NameKind>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtoSpec {
    Direct,
    Reject(Builtin),
    Http(HttpSpec),
    Socks5(Socks5Spec),
    Trojan(TrojanSpec),
    Vmess(VmessSpec),
    AnyTls(AnyTlsSpec),
    Ssh(SshSpec),
    WireGuard(WireGuardSpec),
    External(ExternalSpec),
    Ss(SsSpec),
    Snell(SnellSpec),
}

impl ProtoSpec {
    /// The TLS options of the protocol, when it runs over TLS.
    pub fn tls(&self) -> Option<&TlsOpts> {
        match self {
            ProtoSpec::Http(http) => http.tls.as_ref(),
            ProtoSpec::Socks5(socks) => socks.tls.as_ref(),
            ProtoSpec::Trojan(trojan) => Some(&trojan.tls),
            ProtoSpec::Vmess(vmess) => vmess.tls.as_ref(),
            ProtoSpec::AnyTls(anytls) => Some(&anytls.tls),
            ProtoSpec::Direct
            | ProtoSpec::Reject(_)
            | ProtoSpec::Ssh(_)
            | ProtoSpec::WireGuard(_)
            | ProtoSpec::External(_)
            | ProtoSpec::Ss(_)
            | ProtoSpec::Snell(_) => None,
        }
    }

    /// The `[Keystore]` item the protocol uses: a TLS client certificate, or
    /// an `ssh` private key.
    pub fn keystore_item(&self) -> Option<&str> {
        match self {
            ProtoSpec::Ssh(ssh) => ssh.private_key.as_deref(),
            _ => self.tls().and_then(|tls| tls.client_cert.as_deref()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicySpec {
    pub name: String,
    pub kind: PolicyKind,
    pub server: Option<HostName>,
    pub port: Option<u16>,
    pub common: CommonOpts,
    pub proto: ProtoSpec,
    /// Shadow TLS below the protocol (and below its TLS, when it has one).
    pub shadow_tls: Option<ShadowTlsOpts>,
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
    /// A valid line that asks for something not implemented yet, so it has
    /// no spec. The caller reports each distinct one once per load (`W0007`).
    pub not_implemented: Option<NotImplemented>,
}

/// What a valid line asks for that this version does not implement: the
/// policy has no spec and behaves as REJECT.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NotImplemented {
    /// `vmess` without `vmess-aead=true`: the legacy handshake (M2 design 4.3).
    LegacyVmess,
    /// An `ss` stream cipher, by its name (phase 2 M6 design 3.1).
    SsStreamCipher(&'static str),
    /// A `snell` version other than 4 and 5 (phase 2 M6 design 4.2).
    SnellVersion(u8),
}

impl NotImplemented {
    /// The load warning (`W0007`).
    pub fn warning(&self) -> String {
        match self {
            NotImplemented::LegacyVmess => "`vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet".to_string(),
            NotImplemented::SsStreamCipher(method) => format!(
                "`ss` stream cipher `{method}` is not implemented yet; such policies behave as REJECT"
            ),
            NotImplemented::SnellVersion(n) => {
                // Surge's default: many lines never say which version they are
                let default = if *n == 1 {
                    " (the default when `version` is not written)"
                } else {
                    ""
                };
                format!(
                    "`snell` version {n}{default} is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT"
                )
            }
        }
    }

    /// What the session log says after "policy protocol not implemented: ".
    pub fn note(&self) -> String {
        match self {
            NotImplemented::LegacyVmess => "vmess (legacy handshake)".to_string(),
            NotImplemented::SsStreamCipher(method) => format!("ss ({method})"),
            NotImplemented::SnellVersion(n) => format!("snell v{n}"),
        }
    }

    /// How a warning about imported policies names the kind.
    pub fn imported(&self) -> String {
        match self {
            NotImplemented::LegacyVmess => {
                "`vmess` without `vmess-aead=true` (the legacy handshake)".to_string()
            }
            NotImplemented::SsStreamCipher(method) => {
                format!("`ss` with the stream cipher `{method}`")
            }
            NotImplemented::SnellVersion(n) => format!("`snell` version {n}"),
        }
    }
}

/// Named `username=` / `password=` win over the positional pair.
fn read_credentials(r: &mut ParamReader<'_>) -> (Option<Secret<String>>, Option<Secret<String>>) {
    let positional = (r.positional(0), r.positional(1));
    let username = r.str("username").or(positional.0).map(Secret::from);
    let password = r.str("password").or(positional.1).map(Secret::from);
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
    let mut not_implemented = None;
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
            let (username, password) = read_credentials(&mut r);
            for (what, value) in [("username", &username), ("password", &password)] {
                if value
                    .as_ref()
                    .is_some_and(|v| v.expose().len() > socks5::MAX_CREDENTIAL)
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
        PolicyKind::Trojan => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let trojan = trojan::read_trojan(&mut r, env.keystore);
            (common, ProtoSpec::Trojan(trojan))
        }
        PolicyKind::Vmess => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let read = vmess::read_vmess(&mut r, env.keystore);
            // the rest of the line is still checked; it just has no spec
            if !read.aead {
                not_implemented = Some(NotImplemented::LegacyVmess);
            }
            (common, ProtoSpec::Vmess(read.spec))
        }
        PolicyKind::Shadowsocks => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            tls::refuse_tls(&mut r);
            let read = ss::read_ss(&mut r);
            not_implemented = read.stream_cipher.map(NotImplemented::SsStreamCipher);
            (common, ProtoSpec::Ss(read.spec))
        }
        PolicyKind::Snell => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            tls::refuse_tls(&mut r);
            let read = snell::read_snell(&mut r);
            not_implemented = read
                .not_implemented_version
                .map(NotImplemented::SnellVersion);
            (common, ProtoSpec::Snell(read.spec))
        }
        PolicyKind::AnyTls => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let anytls = anytls::read_anytls(&mut r, env.keystore);
            (common, ProtoSpec::AnyTls(anytls))
        }
        PolicyKind::Ssh => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let ssh = ssh::read_ssh(&mut r, env.keystore);
            (common, ProtoSpec::Ssh(ssh))
        }
        PolicyKind::WireGuard => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let wireguard = wireguard::read_wireguard(&mut r, &mut common, env.wireguard);
            (common, ProtoSpec::WireGuard(wireguard))
        }
        PolicyKind::External => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let external = external::read_external(&mut r, &mut common);
            // `tfo` does not apply at all (`W0028`): no second word on it
            notes.inert.retain(|name| *name != "tfo");
            if !external.addresses.is_empty() {
                notes.inert.push("addresses");
            }
            (common, ProtoSpec::External(external))
        }
        _ => return SpecOutcome::default(),
    };
    let mut shadow_tls = None;
    if !matches!(proto, ProtoSpec::Direct | ProtoSpec::Reject(_)) {
        shadow_tls = shadow_tls::read_shadow_tls(&mut r);
        check_underlying(&mut r, &mut common, env);
        // socket options belong to the hop that opens the socket (matrix 4.3)
        if common.underlying_proxy.is_some() {
            for key in ["interface", "allow-other-interface", "tos", "ip-version"] {
                if r.has(key) {
                    r.warn(
                        codes::W_PARAM_NOT_APPLICABLE,
                        format!(
                            "`{key}` has no effect on a policy with `underlying-proxy`; ignored"
                        ),
                    );
                }
            }
        }
    }
    let failed = r.has_errors();
    let diagnostics = r.finish();
    let spec = (!failed && not_implemented.is_none()).then(|| PolicySpec {
        name: policy.name.clone(),
        kind: policy.kind,
        server: policy.server.clone(),
        port: policy.port,
        common,
        proto,
        shadow_tls,
        span: policy.span.clone(),
    });
    SpecOutcome {
        spec,
        diagnostics,
        inert: notes.inert,
        ios_only: notes.ios_only,
        not_implemented: not_implemented.filter(|_| !failed),
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
    use crate::wireguard::WireGuardSection;
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
        let wireguard = [WireGuardSection {
            name: "home".into(),
            mtu: 1280,
            ..WireGuardSection::default()
        }];
        to_spec(
            &p,
            &SpecEnv {
                keystore: &keystore,
                wireguard: &wireguard,
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
        assert_eq!(
            http.username.as_ref().map(|u| u.expose().as_str()),
            Some("username")
        );
        assert_eq!(
            http.password.as_ref().map(|p| p.expose().as_str()),
            Some("password")
        );
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
            (
                s.username.as_ref().map(|u| u.expose().as_str()),
                s.password.as_ref().map(|p| p.expose().as_str())
            ),
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
        let o = outcome("H", "hysteria2, h, 443, password=x, mystery=1");
        assert!(o.spec.is_none() && o.diagnostics.is_empty() && o.inert.is_empty());
    }

    #[test]
    fn an_ssh_line_gets_a_spec() {
        let o = outcome(
            "S",
            "ssh, h.test, 22, username=u, password=pw, idle-timeout=60",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let spec = o.spec.expect("an ssh spec");
        assert_eq!(
            (spec.server, spec.port),
            (Some(HostName::Domain("h.test".into())), Some(22))
        );
        let ProtoSpec::Ssh(ssh) = &spec.proto else {
            panic!("{:?}", spec.proto);
        };
        assert_eq!(ssh.idle_timeout, std::time::Duration::from_secs(60));
    }

    /// The spec carries the section the line names: an edited section is
    /// another spec, and a reload builds the policy anew (M4 design 4.3).
    #[test]
    fn a_wireguard_line_carries_its_section() {
        let o = outcome("W", "wireguard, section-name=home, ecn=on");
        let spec = o.spec.expect("a wireguard spec");
        let ProtoSpec::WireGuard(wg) = &spec.proto else {
            panic!("{:?}", spec.proto);
        };
        assert_eq!(wg.section.name, "home");
        assert_eq!((spec.server, spec.port), (None, None));
        assert_eq!(o.inert, ["ecn"]);
        let o = outcome("W", "wireguard, section-name=home, shadow-tls-password=pw");
        assert!(o.spec.is_none());
        assert_eq!(
            o.diagnostics[0].message,
            "policy `W`: Shadow TLS cannot be combined with a `wireguard` policy"
        );
    }

    /// An `external` line: no server, no port; what does not work yet is said
    /// once per load (`W0029`), and `tfo` only as not applicable (M4 design 4.4).
    #[test]
    fn an_external_line() {
        let o = outcome(
            "X",
            "external, exec=/usr/bin/ssh, args=-D, args=1080, local-port=1080, addresses=10.0.0.1, udp-relay=true, tfo=true, ecn=on",
        );
        let spec = o.spec.expect("an external spec");
        let ProtoSpec::External(external) = &spec.proto else {
            panic!("{:?}", spec.proto);
        };
        assert_eq!(external.args.expose(), &["-D", "1080"]);
        assert_eq!((spec.server, spec.port), (None, None));
        assert_eq!(o.inert, ["ecn", "addresses"]);
        assert!(external.udp_relay);
        assert_eq!(spec.proto.keystore_item(), None);
        let o = outcome(
            "X",
            "external, exec=/bin/p, local-port=1080, shadow-tls-password=pw",
        );
        assert!(o.spec.is_none());
        assert_eq!(
            o.diagnostics[0].message,
            "policy `X`: Shadow TLS cannot be combined with a `external` policy"
        );
    }

    /// A chain carries UDP since M5: a `wireguard` policy over one loads
    /// without a word, its peers reached through the chain (phase 2 M5
    /// design 8.1). `DIRECT` is no chain.
    #[test]
    fn underlying_proxy_on_a_wireguard_policy_is_accepted() {
        for line in [
            "wireguard, section-name=home, underlying-proxy=Entry",
            "wireguard, section-name=home, underlying-proxy=DIRECT",
        ] {
            let o = outcome("W", line);
            assert!(o.spec.is_some());
            assert!(o.diagnostics.is_empty(), "{line}: {:?}", o.diagnostics);
        }
    }

    /// What a reload compares besides the line (M2 design 7.1): the keystore
    /// item the policy uses, whatever the protocol.
    #[test]
    fn the_keystore_item_is_a_client_certificate_or_an_ssh_key() {
        let https = outcome("H", "https, h.test, 443, client-cert=cert1");
        assert_eq!(https.spec.unwrap().proto.keystore_item(), Some("cert1"));
        let plain = outcome("P", "http, h.test, 80");
        assert_eq!(plain.spec.unwrap().proto.keystore_item(), None);
        let ssh = ProtoSpec::Ssh(SshSpec {
            username: "u".into(),
            password: None,
            private_key: Some("key1".into()),
            idle_timeout: ssh::DEFAULT_IDLE_TIMEOUT,
            host_keys: Vec::new(),
        });
        assert_eq!(ssh.keystore_item(), Some("key1"));
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
        // Shadow TLS took effect in M2c, `udp-relay` in M5a: nothing on the list
        assert!(o.inert.is_empty(), "{:?}", o.inert);
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

    #[test]
    fn shadow_tls_is_part_of_the_spec_of_every_tcp_protocol() {
        for def in [
            "http, h.test, 80",
            "https, h.test, 443",
            "socks5, h.test, 1080",
            "socks5-tls, h.test, 1080",
            "trojan, h.test, 443, password=p",
            "vmess, h.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true",
            "anytls, h.test, 443, password=p",
            "ss, h.test, 8388, encrypt-method=aes-128-gcm, password=p",
        ] {
            let o = outcome(
                "P",
                &format!(
                    "{def}, shadow-tls-password=s3cret, shadow-tls-version=3, shadow-tls-sni=site.test"
                ),
            );
            assert!(o.diagnostics.is_empty(), "{def}: {:?}", o.diagnostics);
            assert!(o.inert.is_empty(), "{def}: {:?}", o.inert);
            let layer = o.spec.unwrap().shadow_tls.expect(def);
            assert_eq!(layer.password.expose(), "s3cret");
            assert_eq!(layer.sni.as_deref(), Some("site.test"));
            assert_eq!(layer.version, ShadowTlsVersion::V3);
            // without the parameters there is no layer
            assert!(
                outcome("P", def).spec.unwrap().shadow_tls.is_none(),
                "{def}"
            );
        }
        // an error in the layer drops the spec like any other
        let o = outcome(
            "P",
            "http, h.test, 80, shadow-tls-password=s3cret, shadow-tls-version=3",
        );
        assert!(o.spec.is_none());
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
        // a policy that opens no connection to a server has no use for it
        let o = outcome("P", "direct, shadow-tls-password=s3cret");
        assert!(o.spec.unwrap().shadow_tls.is_none());
        assert_eq!(o.diagnostics[0].code, codes::W_UNKNOWN_KEY);
        assert!(!o.diagnostics[0].message.contains("s3cret"));
    }

    #[test]
    fn socket_options_under_a_chain_are_reported() {
        let o = outcome(
            "P",
            "http, h, 80, underlying-proxy=Entry, interface=eth0, allow-other-interface=true, tos=16, ip-version=v4-only",
        );
        assert!(o.spec.is_some(), "a warning, not an error");
        let messages: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            messages,
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `interface` has no effect on a policy with `underlying-proxy`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `allow-other-interface` has no effect on a policy with `underlying-proxy`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `tos` has no effect on a policy with `underlying-proxy`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `ip-version` has no effect on a policy with `underlying-proxy`; ignored"
                ),
            ]
        );
        // `underlying-proxy=DIRECT` is no chain: nothing to report
        let o = outcome("P", "http, h, 80, underlying-proxy=DIRECT, interface=eth0");
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let o = outcome("P", "http, h, 80, interface=eth0, tos=16");
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        // carried from Task 1's review: a `reject*` alias has no real chain
        // (`read_common` does not clear `underlying-proxy` for `Applies::Reject`),
        // so it must not get this warning either
        let o = outcome("P", "reject, underlying-proxy=Entry, interface=eth0");
        assert!(
            !o.diagnostics
                .iter()
                .any(|d| d.code == codes::W_PARAM_NOT_APPLICABLE),
            "{:?}",
            o.diagnostics
        );
    }

    #[test]
    fn vmess_and_anytls_lines_become_specs() {
        let id = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
        let o = outcome(
            "V",
            &format!("vmess, h.test, 443, username={id}, vmess-aead=true, tls=true"),
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert_eq!(o.not_implemented, None);
        let spec = o.spec.expect("a spec");
        let ProtoSpec::Vmess(vmess) = &spec.proto else {
            panic!("not vmess: {:?}", spec.proto);
        };
        assert!(vmess.tls.is_some() && spec.proto.tls().is_some());
        let o = outcome("A", "anytls, h.test, 443, password=pw, reuse=false");
        let spec = o.spec.expect("a spec");
        let ProtoSpec::AnyTls(anytls) = &spec.proto else {
            panic!("not anytls: {:?}", spec.proto);
        };
        assert!(!anytls.reuse && spec.proto.tls().is_some());
    }

    #[test]
    fn a_vmess_line_without_the_aead_handshake_has_no_spec() {
        let id = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
        let o = outcome("V", &format!("vmess, h.test, 443, username={id}, ws=true"));
        assert!(o.spec.is_none());
        assert_eq!(o.not_implemented, Some(NotImplemented::LegacyVmess));
        assert!(
            o.diagnostics.is_empty(),
            "the loader reports it, once: {:?}",
            o.diagnostics
        );
        // a broken legacy line is an error like any other, and not "legacy"
        let o = outcome("V", "vmess, h.test, 443, username=nope");
        assert!(o.spec.is_none() && o.not_implemented.is_none());
        assert_eq!(o.diagnostics.len(), 1);
    }

    /// An `ss` line is read and checked in full; a stream cipher has no
    /// spec and says why.
    #[test]
    fn an_ss_line_is_checked_and_a_stream_cipher_is_not_implemented() {
        let o = outcome(
            "S",
            "ss, h.test, 8388, encrypt-method=aes-128-gcm, password=pw, udp-relay=true, udp-port=8389, obfs=tls, obfs-host=cdn.test, shadow-tls-password=st",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert_eq!(o.not_implemented, None);
        let spec = o.spec.expect("an AEAD line has a spec");
        let ProtoSpec::Ss(ss) = &spec.proto else {
            panic!("{:?}", spec.proto)
        };
        assert_eq!(
            (ss.method, ss.udp_relay, ss.udp_port),
            (SsMethod::Aes128Gcm, true, Some(8389))
        );
        assert_eq!(ss.obfs.as_ref().map(|o| o.mode), Some(ObfsMode::Tls));
        assert!(spec.shadow_tls.is_some());

        let o = outcome("S", "ss, h.test, 8388, encrypt-method=RC4-MD5, password=pw");
        assert!(o.spec.is_none() && o.diagnostics.is_empty());
        let why = o.not_implemented.expect("a stream cipher");
        assert_eq!(why, NotImplemented::SsStreamCipher("rc4-md5"));
        assert_eq!(
            why.warning(),
            "`ss` stream cipher `rc4-md5` is not implemented yet; such policies behave as REJECT"
        );
        assert_eq!(why.note(), "ss (rc4-md5)");
        assert_eq!(why.imported(), "`ss` with the stream cipher `rc4-md5`");

        // a broken line is an error like any other, and not "not implemented"
        let o = outcome("S", "ss, h.test, 8388, encrypt-method=rc4-md5");
        assert!(o.not_implemented.is_none());
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
        // no TLS under `ss`; mystery parameters are unknown as anywhere
        let o = outcome(
            "S",
            "ss, h.test, 8388, encrypt-method=none, sni=edge.test, mystery=1",
        );
        let found: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `S`: `sni` does not apply to `ss` policies; ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `S`: unknown parameter `mystery` ignored"
                ),
            ]
        );
    }

    /// A `snell` line is read and checked in full; a version other than 4
    /// and 5 has no spec and says why (M6-D2).
    #[test]
    fn a_snell_line_is_checked_and_other_versions_are_not_implemented() {
        let o = outcome(
            "N",
            "snell, h.test, 443, psk=pw, version=5, reuse=true, udp-port=8443, obfs=http, obfs-host=cdn.test, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert_eq!(o.not_implemented, None);
        let spec = o.spec.expect("a version 5 line has a spec");
        let ProtoSpec::Snell(snell) = &spec.proto else {
            panic!("{:?}", spec.proto)
        };
        assert_eq!(
            (snell.version, snell.reuse, snell.udp_port),
            (SnellVersion::V5, true, Some(8443))
        );
        assert_eq!(snell.obfs.as_ref().map(|o| o.mode), Some(ObfsMode::Http));
        assert!(spec.shadow_tls.is_some());

        // without `version`: Surge's default, 1
        let o = outcome("N", "snell, h.test, 443, psk=pw");
        assert!(o.spec.is_none() && o.diagnostics.is_empty());
        let why = o.not_implemented.expect("version 1");
        assert_eq!(why, NotImplemented::SnellVersion(1));
        assert_eq!(
            why.warning(),
            "`snell` version 1 (the default when `version` is not written) is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT"
        );
        assert_eq!(why.note(), "snell v1");
        assert_eq!(why.imported(), "`snell` version 1");
        let o = outcome("N", "snell, h.test, 443, psk=pw, version=6");
        let why = o.not_implemented.expect("version 6");
        assert_eq!(
            why.warning(),
            "`snell` version 6 is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT"
        );
        assert_eq!(why.note(), "snell v6");

        // a broken line is an error like any other, and not "not implemented"
        let o = outcome("N", "snell, h.test, 443, version=2");
        assert!(o.not_implemented.is_none());
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
        // no TLS under `snell`; the PSK is never quoted
        let o = outcome(
            "N",
            "snell, h.test, 443, psk=s3cretPsk, version=4, sni=edge.test, mystery=1",
        );
        let found: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `N`: `sni` does not apply to `snell` policies; ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `N`: unknown parameter `mystery` ignored"
                ),
            ]
        );
        assert_eq!(
            ProtoSpec::Snell(SnellSpec {
                version: SnellVersion::V4,
                psk: "pw".into(),
                reuse: false,
                udp_port: None,
                obfs: None,
            })
            .keystore_item(),
            None
        );
    }
}
