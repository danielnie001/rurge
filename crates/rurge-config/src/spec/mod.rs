//! Typed view of `[Proxy]` policy parameters (phase 2 M1 design §4).

pub mod anytls;
pub mod common;
pub mod external;
pub mod group;
pub mod h2;
pub mod http;
pub mod hysteria2;
pub mod masque;
pub mod obfs;
pub mod quic;
pub mod reader;
pub mod secret;
pub mod shadow_tls;
pub mod snell;
pub mod socks5;
pub mod ss;
pub mod ssh;
pub mod tls;
pub mod trojan;
pub mod tuic;
pub mod vmess;
pub mod wireguard;
pub mod ws;

pub use anytls::AnyTlsSpec;
pub use common::{Applies, CommonOpts, IpVersion, Tristate};
pub use external::ExternalSpec;
pub use group::{
    GroupOutcome, GroupSpec, ImportOpts, PolicyPath, Priority, SubnetSpec, TestOpts, to_group_spec,
};
pub use h2::{H2ConnectSpec, TrustTunnelSpec};
pub use http::{HeaderPart, HeaderTemplate, HttpSpec};
pub use hysteria2::{Hysteria2Obfs, Hysteria2Spec};
pub use masque::{MasqueCredentials, MasqueSpec};
pub use obfs::{ObfsMode, ObfsOpts};
pub use quic::{PortHopping, QuicOpts};
pub use reader::ParamReader;
pub use secret::Secret;
pub use shadow_tls::{ShadowTlsOpts, ShadowTlsVersion};
pub use snell::{SnellSpec, SnellVersion};
pub use socks5::Socks5Spec;
pub use ss::{SsMethod, SsSpec};
pub use ssh::{HostKeyPin, SshSpec};
pub use tls::{Sni, TlsOpts};
pub use trojan::TrojanSpec;
pub use tuic::{TuicSpec, TuicVersion};
pub use vmess::{VmessCipher, VmessSpec};
pub use wireguard::WireGuardSpec;
pub use ws::WsOpts;

use crate::diagnostic::{Diagnostic, codes};
use crate::keystore::KeystoreItem;
use crate::policy::{Builtin, PolicyKind, ProxyPolicy};
use crate::reach::Reach;
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
    H2Connect(H2ConnectSpec),
    TrustTunnel(TrustTunnelSpec),
    Hysteria2(Hysteria2Spec),
    Tuic(TuicSpec),
    Masque(MasqueSpec),
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
            ProtoSpec::H2Connect(h2) => Some(&h2.tls),
            ProtoSpec::TrustTunnel(tt) => Some(&tt.tls),
            ProtoSpec::Hysteria2(hysteria2) => Some(&hysteria2.tls),
            ProtoSpec::Tuic(tuic) => Some(&tuic.tls),
            ProtoSpec::Masque(masque) => Some(&masque.tls),
            ProtoSpec::Direct
            | ProtoSpec::Reject(_)
            | ProtoSpec::Ssh(_)
            | ProtoSpec::WireGuard(_)
            | ProtoSpec::External(_)
            | ProtoSpec::Ss(_)
            | ProtoSpec::Snell(_) => None,
        }
    }

    /// The QUIC options of the protocol, when it runs over QUIC and takes
    /// them: a `trust-tunnel` with `h3=true` takes none (no port hopping).
    pub fn quic(&self) -> Option<&QuicOpts> {
        match self {
            ProtoSpec::Hysteria2(hysteria2) => Some(&hysteria2.quic),
            ProtoSpec::Tuic(tuic) => Some(&tuic.quic),
            ProtoSpec::Masque(masque) => Some(&masque.quic),
            ProtoSpec::Direct
            | ProtoSpec::Reject(_)
            | ProtoSpec::Http(_)
            | ProtoSpec::Socks5(_)
            | ProtoSpec::Trojan(_)
            | ProtoSpec::Vmess(_)
            | ProtoSpec::AnyTls(_)
            | ProtoSpec::Ssh(_)
            | ProtoSpec::WireGuard(_)
            | ProtoSpec::External(_)
            | ProtoSpec::Ss(_)
            | ProtoSpec::Snell(_)
            | ProtoSpec::H2Connect(_)
            | ProtoSpec::TrustTunnel(_) => None,
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
    /// `None` when the line asks for something rurge does not implement, or
    /// when an error was found.
    pub spec: Option<PolicySpec>,
    pub diagnostics: Vec<Diagnostic>,
    /// Parameters present on the line that are parsed but do nothing yet;
    /// the caller reports each name once per load (`W0029`).
    pub inert: Vec<&'static str>,
    /// iOS-only parameters present on the line (`W0004`, once per load).
    pub ios_only: Vec<&'static str>,
    /// A valid line that asks for something rurge does not implement, so it
    /// has no spec. The caller reports each distinct one once per load
    /// (`W0007`, or `W0033`).
    pub not_implemented: Option<NotImplemented>,
    /// What the line's spec makes rurge say. The caller reports each
    /// distinct one once per load, under the remark's own code.
    pub remarks: Vec<Remark>,
}

/// What a valid line asks for that rurge does not implement: the policy has
/// no spec and behaves as REJECT.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NotImplemented {
    /// An `ss` cipher Surge no longer lists, by its name (phase 2 M8 design
    /// 4.1).
    SsCipher(&'static str),
    /// A `snell` version rurge does not speak: 6 (phase 2 M8 design 4.4).
    SnellVersion(u8),
    /// An `external` policy written in downloaded text: it would start a
    /// program on this machine that nobody here wrote, so it is not started.
    DownloadedExternal,
    /// The same in the downloaded copy of a managed profile: while the copy is
    /// in effect it is the profile itself, and it is downloaded text all the
    /// same.
    ManagedExternal,
}

impl NotImplemented {
    /// What an `external` policy defined in text of this `reach` is, if it is
    /// not started: the one place that says which texts start a program.
    pub fn external_in(reach: Reach) -> Option<NotImplemented> {
        match reach {
            Reach::Anywhere => None,
            Reach::ManagedCopy => Some(NotImplemented::ManagedExternal),
            Reach::Nowhere => Some(NotImplemented::DownloadedExternal),
        }
    }

    /// The text of the load warning.
    pub fn warning(&self) -> String {
        match self {
            NotImplemented::SsCipher(method) => format!(
                "`ss` cipher `{method}` is not supported (Surge no longer lists it); such policies behave as REJECT"
            ),
            // out of scope, not a matter of time (phase 2 M8 design 4.4)
            NotImplemented::SnellVersion(n) => format!(
                "`snell` version {n} is not supported: it is a beta protocol without a public specification; such policies behave as REJECT"
            ),
            NotImplemented::DownloadedExternal => "`external` policies written in downloaded text are not started; they behave as REJECT".to_string(),
            NotImplemented::ManagedExternal => "`external` policies in the downloaded copy of a managed profile are not started; they behave as REJECT".to_string(),
        }
    }

    /// The load warning: `W0007`, or `W0033` for what a downloaded text asks
    /// of this machine.
    pub fn diagnostic(&self) -> Diagnostic {
        match self {
            NotImplemented::DownloadedExternal => {
                Diagnostic::warning(codes::W_REMOTE_LOCAL_ACCESS, self.warning())
                    .with_hint("define the policy in the local profile to use it")
            }
            NotImplemented::ManagedExternal => {
                Diagnostic::warning(codes::W_REMOTE_LOCAL_ACCESS, self.warning()).with_hint(
                    "to use an `external` policy, load a profile without `#!MANAGED-CONFIG` that takes the downloaded parts in by `#!include`",
                )
            }
            NotImplemented::SsCipher(_) | NotImplemented::SnellVersion(_) => {
                Diagnostic::warning(codes::W_PROTOCOL_NOT_IMPLEMENTED, self.warning())
            }
        }
    }

    /// What the session log says after "policy protocol not implemented: ".
    pub fn note(&self) -> String {
        match self {
            NotImplemented::SsCipher(method) => format!("ss ({method})"),
            NotImplemented::SnellVersion(n) => format!("snell v{n}"),
            NotImplemented::DownloadedExternal => "external (downloaded text)".to_string(),
            NotImplemented::ManagedExternal => "external (managed profile)".to_string(),
        }
    }

    /// How a warning about imported policies names the kind.
    pub fn imported(&self) -> String {
        match self {
            NotImplemented::SsCipher(method) => format!("`ss` with the cipher `{method}`"),
            NotImplemented::SnellVersion(n) => format!("`snell` version {n}"),
            NotImplemented::DownloadedExternal => "`external` in downloaded text".to_string(),
            NotImplemented::ManagedExternal => "`external` in a managed profile".to_string(),
        }
    }
}

/// What a valid line makes rurge say once per load and kind: the policy is
/// built all the same.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Remark {
    /// `vmess` without `vmess-aead=true`: the AEAD handshake is used anyway.
    VmessAead,
    /// An `ss` stream cipher, by its name.
    SsStreamCipher(&'static str),
}

impl Remark {
    /// The code its warning goes under.
    pub fn code(&self) -> &'static str {
        match self {
            Remark::VmessAead => codes::W_PARAM_NOT_EFFECTIVE,
            Remark::SsStreamCipher(_) => codes::W_STREAM_CIPHER,
        }
    }

    /// The load warning.
    pub fn warning(&self) -> String {
        match self {
            // one text for a line that leaves the parameter out and for one
            // that writes it `false`: it quotes neither
            Remark::VmessAead => "`vmess` without `vmess-aead=true` uses the AEAD handshake all the same: rurge has no other, and every server since V2Ray 4.28.1 accepts it".to_string(),
            Remark::SsStreamCipher(method) => format!(
                "`ss` cipher `{method}` is a stream cipher: it gives no integrity protection and is deprecated; use an AEAD cipher{}",
                rc4_keystream(method)
            ),
        }
    }

    /// The warning about a group's imported policies.
    pub fn imported(&self) -> String {
        match self {
            Remark::VmessAead => {
                "imported `vmess` policies without `vmess-aead=true` use the AEAD handshake"
                    .to_string()
            }
            // the lines are the subscription's: there is no cipher for the
            // reader to change
            Remark::SsStreamCipher(method) => format!(
                "imported `ss` policies use the stream cipher `{method}`: it gives no integrity protection and is deprecated{}",
                rc4_keystream(method)
            ),
        }
    }
}

/// `rc4` has no IV: the key alone makes its keystream, the same one for
/// every connection and every datagram of a policy, in both directions.
fn rc4_keystream(method: &str) -> &'static str {
    if method == "rc4" {
        "; `rc4` also repeats its keystream on every connection"
    } else {
        ""
    }
}

/// Named `username=` / `password=` win over the positional pair.
fn read_credentials(r: &mut ParamReader<'_>) -> (Option<Secret<String>>, Option<Secret<String>>) {
    let positional = (r.positional(0), r.positional(1));
    let username = r.str("username").or(positional.0).map(Secret::from);
    let password = r.str("password").or(positional.1).map(Secret::from);
    (username, password)
}

/// `headers`: a malformed list is `E0018`, its entry named by position.
fn read_headers(r: &mut ParamReader<'_>) -> Vec<HeaderTemplate> {
    let Some(v) = r.str("headers") else {
        return Vec::new();
    };
    match HeaderTemplate::parse_list(v) {
        Ok(list) => list,
        Err(why) => {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                format!("invalid `headers`: {why}"),
            );
            Vec::new()
        }
    }
}

/// A named credential that must be there; its value is never quoted.
fn required(r: &mut ParamReader<'_>, key: &str) -> Secret<String> {
    let value = r.str(key).unwrap_or_default();
    if value.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            format!("`{key}` is required"),
        );
    }
    Secret::from(value)
}

fn check_underlying(r: &mut ParamReader<'_>, common: &mut CommonOpts, env: &SpecEnv<'_>) {
    let Some(name) = common.underlying_proxy.clone() else {
        return;
    };
    match (env.lookup)(&name) {
        None => r.error_on(
            "underlying-proxy",
            codes::E_UNKNOWN_POLICY_REF,
            reader::unknown_reference("underlying-proxy", "policy", &name),
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

/// The policy's server as its spec carries it: a name written in Unicode in
/// its A-label form (UTS #46), so that the TLS layer, the resolver and every
/// request that names the server take ASCII; an ASCII name and an address
/// as they are. A name without such a form is `E0018`, which does not quote
/// it: the line may be a subscription's.
fn read_server(r: &mut ParamReader<'_>) -> Option<HostName> {
    match &r.policy().server {
        Some(HostName::Domain(name)) if !name.is_ascii() => {
            // read as a written name is: a dot that UTS #46 left at its end
            // goes, and digits it made ASCII are an address
            let server = crate::hostname::to_ascii(name)
                .map(|ascii| HostName::parse(&ascii))
                // nothing but dots: no name is left of it
                .filter(|server| server.as_domain() != Some(""));
            if server.is_none() {
                r.error(
                    codes::E_INVALID_POLICY_PARAM,
                    "the server's host name cannot be written in ASCII (IDNA)".to_string(),
                );
            }
            server
        }
        written => written.clone(),
    }
}

pub fn to_spec(policy: &ProxyPolicy, env: &SpecEnv<'_>) -> SpecOutcome {
    let mut r = ParamReader::new(policy);
    let server = read_server(&mut r);
    let mut notes = Notes::default();
    let mut not_implemented = None;
    let mut remarks = Vec::new();
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
            let headers = read_headers(&mut r);
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
            // rurge speaks the AEAD handshake only: the line gets it whatever
            // it asks for (phase 2 M8 design 4.2)
            if !read.aead {
                remarks.push(Remark::VmessAead);
            }
            (common, ProtoSpec::Vmess(read.spec))
        }
        PolicyKind::Shadowsocks => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            tls::refuse_tls(&mut r);
            let read = ss::read_ss(&mut r);
            not_implemented = read.unsupported_cipher.map(NotImplemented::SsCipher);
            // built like any other method, and said to be what it is (phase
            // 2 M8 design 4.1)
            if read.spec.method.is_stream() {
                remarks.push(Remark::SsStreamCipher(read.spec.method.name()));
            }
            (common, ProtoSpec::Ss(read.spec))
        }
        PolicyKind::Snell => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            tls::refuse_tls(&mut r);
            let read = snell::read_snell(&mut r);
            not_implemented = read
                .not_implemented_version
                .map(NotImplemented::SnellVersion);
            if read.udp_port && not_implemented.is_none() {
                notes.inert.push("udp-port");
            }
            (common, ProtoSpec::Snell(read.spec))
        }
        PolicyKind::AnyTls => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let anytls = anytls::read_anytls(&mut r, env.keystore);
            (common, ProtoSpec::AnyTls(anytls))
        }
        PolicyKind::H2Connect => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let h2 = h2::read_h2_connect(&mut r, env.keystore);
            (common, ProtoSpec::H2Connect(h2))
        }
        PolicyKind::TrustTunnel => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let tt = h2::read_trust_tunnel(&mut r, &mut common, env.keystore);
            if tt.h3 {
                // over QUIC, as on `hysteria2`: `ecn` takes effect, `tfo`
                // does not apply (phase 2 M7 design 7.2)
                notes.inert.retain(|name| !matches!(*name, "ecn" | "tfo"));
            } else if r.rejected("h3") {
                // an `h3` that is an error leaves open what `tfo` does here
                notes.inert.retain(|name| *name != "tfo");
            }
            (common, ProtoSpec::TrustTunnel(tt))
        }
        PolicyKind::Hysteria2 => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let hysteria2 = hysteria2::read_hysteria2(&mut r, &mut common, env.keystore);
            // `ecn` takes effect on QUIC (phase 2 M7 design 3.4); `tfo` does
            // not apply at all (`W0028`): no second word on either
            notes.inert.retain(|name| !matches!(*name, "ecn" | "tfo"));
            (common, ProtoSpec::Hysteria2(hysteria2))
        }
        PolicyKind::Tuic | PolicyKind::TuicV5 => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let tuic = tuic::read_tuic(&mut r, &mut common, env.keystore);
            // as on `hysteria2`: `ecn` takes effect, `tfo` does not apply
            notes.inert.retain(|name| !matches!(*name, "ecn" | "tfo"));
            (common, ProtoSpec::Tuic(tuic))
        }
        PolicyKind::Masque => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let masque = masque::read_masque(&mut r, &mut common, env.keystore);
            // as on `hysteria2`: `ecn` takes effect, `tfo` does not apply
            notes.inert.retain(|name| !matches!(*name, "ecn" | "tfo"));
            (common, ProtoSpec::Masque(masque))
        }
        PolicyKind::Ssh => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let ssh = ssh::read_ssh(&mut r, env.keystore);
            (common, ProtoSpec::Ssh(ssh))
        }
        PolicyKind::WireGuard => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let wireguard = wireguard::read_wireguard(&mut r, &mut common, env.wireguard);
            // `tfo` does not apply at all (`W0028`): no second word on it;
            // `ecn` is left its "no effect yet" below
            notes.inert.retain(|name| *name != "tfo");
            (common, ProtoSpec::WireGuard(wireguard))
        }
        PolicyKind::External => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let external = external::read_external(&mut r, &mut common);
            // `tfo` does not apply at all (`W0028`): no second word on it.
            // `addresses` keeps the enhanced mode off those addresses, so it
            // is in no "no effect yet" list (M4-D21)
            notes.inert.retain(|name| *name != "tfo");
            (common, ProtoSpec::External(external))
        }
        _ => return SpecOutcome::default(),
    };
    // `ecn` marks the packets of a QUIC connection: the arms of the
    // protocols over QUIC took it off the list. One over TCP sends no
    // packets of its own to mark (manual), so there it does not apply.
    // `wireguard` alone is left its "no effect yet" (phase 2 M5 design,
    // M5-D6).
    if !matches!(proto, ProtoSpec::WireGuard(_)) && notes.inert.contains(&"ecn") {
        notes.inert.retain(|name| *name != "ecn");
        let kind = policy.kind.keyword();
        let which = match &proto {
            // over HTTP/3 it does apply
            ProtoSpec::TrustTunnel(tunnel) if !tunnel.h3 => " without `h3=true`",
            _ => "",
        };
        // an `h3` that is an error already leaves open what a `trust-tunnel`
        // runs over: nothing more is said of the line
        let open = r.rejected("h3");
        if !open {
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                format!("`ecn` does not apply to `{kind}` policies{which}; ignored"),
            );
        }
        common.ecn = Tristate::Auto;
    }
    let mut shadow_tls = None;
    if !matches!(proto, ProtoSpec::Direct | ProtoSpec::Reject(_)) {
        shadow_tls = match &proto {
            // Shadow TLS wraps a TCP connection; HTTP/3 runs over QUIC
            // (phase 2 M7 design 3.1)
            ProtoSpec::TrustTunnel(tt) if tt.h3 => {
                shadow_tls::refuse(
                    &mut r,
                    "Shadow TLS cannot be combined with `h3=true` on a `trust-tunnel` policy"
                        .to_string(),
                );
                None
            }
            _ => shadow_tls::read_shadow_tls(&mut r),
        };
        check_underlying(&mut r, &mut common, env);
        // a chain that is an error already leaves open what it conflicts
        // with and what it takes from the policy: nothing more is said of
        // either
        let chained = common.underlying_proxy.is_some() && !r.rejected("underlying-proxy");
        // the manual: `underlying-proxy` cannot be combined with the
        // `port-hopping` of the QUIC-based protocols
        if chained && proto.quic().is_some_and(|quic| quic.port_hopping.is_some()) {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "`port-hopping` cannot be combined with `underlying-proxy`".to_string(),
            );
        }
        // socket options belong to the hop that opens the socket (matrix 4.3)
        if chained {
            for key in ["interface", "allow-other-interface", "tos", "ip-version"] {
                if r.has_unrejected(key) {
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
        server,
        port: policy.port,
        common,
        proto,
        shadow_tls,
        span: policy.span.clone(),
    });
    // a remark is about how the policy is built: no policy, nothing to say
    if spec.is_none() {
        remarks.clear();
    }
    SpecOutcome {
        spec,
        diagnostics,
        inert: notes.inert,
        ios_only: notes.ios_only,
        not_implemented: not_implemented.filter(|_| !failed),
        remarks,
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
        outcome_of(&parse_policy(name, def, &span()).unwrap())
    }

    fn outcome_of(p: &ProxyPolicy) -> SpecOutcome {
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
            p,
            &SpecEnv {
                keystore: &keystore,
                wireguard: &wireguard,
                lookup: &lookup,
            },
        )
    }

    /// A server written in Unicode goes into the spec as its A-labels (UTS
    /// #46): the TLS layer, the resolver and every request that names the
    /// server take ASCII from there. Its case and a dot at its end go as an
    /// ASCII name's do.
    #[test]
    fn a_server_written_in_unicode_becomes_its_a_labels() {
        for (written, expected) in [
            ("bücher.example", "xn--bcher-kva.example"),
            ("例え.テスト", "xn--r8jz45g.xn--zckzah"),
            ("BÜCHER.Example.", "xn--bcher-kva.example"),
            // the ideographic full stop is a dot, also at the end
            ("例え。テスト。", "xn--r8jz45g.xn--zckzah"),
        ] {
            let o = outcome("T", &format!("trojan, {written}, 443, password=x"));
            assert!(o.diagnostics.is_empty(), "{written}: {:?}", o.diagnostics);
            assert_eq!(
                o.spec.expect("a spec").server,
                Some(HostName::Domain(expected.into())),
                "{written}"
            );
        }
        // digits written wide are an address, as the ASCII ones are
        let o = outcome("T", "trojan, １９２.０.２.７, 443, password=x");
        assert_eq!(
            o.spec.expect("a spec").server,
            Some(HostName::Ip("192.0.2.7".parse().unwrap()))
        );
    }

    /// An ASCII name is not touched — one that is A-labels already, one
    /// with an underscore — and neither is an address.
    #[test]
    fn an_ascii_server_and_an_address_are_left_as_they_are() {
        for written in [
            "proxy.example",
            "xn--bcher-kva.example",
            "_srv.a-b.example",
            "localhost",
            "192.0.2.7",
            "[2001:db8::1]",
        ] {
            let p =
                parse_policy("T", &format!("trojan, {written}, 443, password=x"), &span()).unwrap();
            let o = outcome_of(&p);
            assert!(o.diagnostics.is_empty(), "{written}: {:?}", o.diagnostics);
            assert_eq!(o.spec.expect("a spec").server, p.server, "{written}");
        }
        // Its case stays what the policy holds, and an ASCII name that is
        // no host name is no error here: it fails where it is used, as it
        // did.
        let mut p = parse_policy("T", "trojan, proxy.example, 443, password=x", &span()).unwrap();
        for unchanged in ["MiXed.Example", "a@b.example"] {
            p.server = Some(HostName::Domain(unchanged.into()));
            let o = outcome_of(&p);
            assert!(o.diagnostics.is_empty(), "{unchanged}: {:?}", o.diagnostics);
            assert_eq!(o.spec.expect("a spec").server, p.server, "{unchanged}");
        }
    }

    /// A name that UTS #46 cannot turn into a host name — the fullwidth `＠`
    /// folds onto `@` — is an error, which does not quote it; and so is one
    /// of nothing but dots, which leaves no name at all.
    #[test]
    fn a_server_without_an_ascii_form_is_an_error() {
        for (kind, written) in [
            ("trojan", "a＠b.example"),
            ("http", "bü cher.example"),
            ("https", "bücher：8388"),
            ("trojan", "。"),
            ("http", "．。"),
        ] {
            let o = outcome("T", &format!("{kind}, {written}, 443, password=x"));
            assert!(o.spec.is_none(), "{written}");
            let found: Vec<(&str, &str)> = o
                .diagnostics
                .iter()
                .map(|d| (d.code, d.message.as_str()))
                .collect();
            assert_eq!(
                found,
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: the server's host name cannot be written in ASCII (IDNA)"
                )],
                "{written}"
            );
        }
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

    /// A positional user name or password that holds an `=` reads as one
    /// when it is wholly in quotes (line-splitting design LS-D6), on every
    /// type that takes them from positions.
    #[test]
    fn a_positional_credential_with_an_equals_sign_reads_in_quotes() {
        for head in [
            "http, 127.0.0.1, 8080",
            "https, 127.0.0.1, 8443",
            "socks5, 127.0.0.1, 1080",
            "socks5-tls, 127.0.0.1, 1443",
            "h2-connect, 127.0.0.1, 443",
        ] {
            let o = outcome("P", &format!("{head}, \"al=ice\", \"k3y=v4lue\""));
            assert!(o.diagnostics.is_empty(), "{head}: {:?}", o.diagnostics);
            let spec = o
                .spec
                .unwrap_or_else(|| panic!("{head}: no spec was built"));
            let (username, password) = match &spec.proto {
                ProtoSpec::Http(s) => (&s.username, &s.password),
                ProtoSpec::Socks5(s) => (&s.username, &s.password),
                ProtoSpec::H2Connect(s) => (&s.username, &s.password),
                _ => panic!("{head}: not the spec of a type that reads credentials"),
            };
            assert_eq!(
                username.as_ref().map(|u| u.expose().as_str()),
                Some("al=ice"),
                "{head}"
            );
            assert_eq!(
                password.as_ref().map(|p| p.expose().as_str()),
                Some("k3y=v4lue"),
                "{head}"
            );
        }
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
        let o = outcome("T", "tailscale, section-name=ts, mystery=1");
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

    /// An `external` line: no server, no port; `tfo` and `ecn` are the only
    /// things said of it, as not applicable (M4 design 4.4), and `addresses`
    /// is in no "no effect yet" list: it takes effect while the enhanced
    /// mode is on (M4-D21).
    #[test]
    fn an_external_line() {
        let o = outcome(
            "X",
            "external, exec=/usr/bin/ssh, args=-D, args=1080, local-port=1080, addresses=10.0.0.1, udp-relay=true, tfo=true, ecn=on",
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
                    "policy `X`: `tfo` does not apply to `external` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `X`: `ecn` does not apply to `external` policies; ignored"
                ),
            ]
        );
        let spec = o.spec.expect("an external spec");
        let ProtoSpec::External(external) = &spec.proto else {
            panic!("{:?}", spec.proto);
        };
        assert_eq!(external.args.expose(), &["-D", "1080"]);
        assert_eq!((spec.server, spec.port), (None, None));
        assert!(o.inert.is_empty(), "{:?}", o.inert);
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

    /// `addresses` takes effect while the enhanced mode is on (M4-D21), so
    /// the line no longer reports it as parsed but without effect.
    #[test]
    fn external_addresses_are_no_longer_inert() {
        let o = outcome(
            "X",
            "external, exec=/bin/p, local-port=1080, addresses=10.0.0.1, addresses=fd00::1",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        let spec = o.spec.expect("an external spec");
        let ProtoSpec::External(external) = &spec.proto else {
            panic!("{:?}", spec.proto);
        };
        assert_eq!(external.addresses.len(), 2);
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

    /// rurge speaks the AEAD handshake only: a line that does not ask for it
    /// gets it all the same, and the loader says so once (phase 2 M8 design
    /// 4.2). `vmess-aead=false` is the default written out.
    #[test]
    fn a_vmess_line_without_vmess_aead_uses_the_aead_handshake() {
        let id = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
        let asked = outcome(
            "V",
            &format!("vmess, h.test, 443, username={id}, ws=true, vmess-aead=true"),
        );
        assert!(asked.remarks.is_empty(), "{:?}", asked.remarks);
        let asked = asked.spec.expect("a spec");
        for written in ["", ", vmess-aead=false"] {
            let o = outcome(
                "V",
                &format!("vmess, h.test, 443, username={id}, ws=true{written}"),
            );
            assert!(
                o.diagnostics.is_empty(),
                "the loader says it, once: {:?}",
                o.diagnostics
            );
            assert_eq!(o.not_implemented, None, "{written}");
            assert_eq!(o.remarks, [Remark::VmessAead], "{written}");
            let spec = o.spec.unwrap_or_else(|| panic!("no spec: {written}"));
            assert_eq!(spec.proto, asked.proto, "{written}");
        }
        // a broken line is an error like any other: nothing is built of it,
        // so there is nothing to remark on
        let o = outcome("V", "vmess, h.test, 443, username=nope");
        assert!(o.spec.is_none() && o.not_implemented.is_none());
        assert!(o.remarks.is_empty(), "{:?}", o.remarks);
        assert_eq!(o.diagnostics.len(), 1);
    }

    #[test]
    fn a_remark_has_a_code_and_two_texts() {
        let aead = Remark::VmessAead;
        assert_eq!(aead.code(), codes::W_PARAM_NOT_EFFECTIVE);
        // true of a line that never wrote the parameter as of one that wrote
        // it `false`
        assert_eq!(
            aead.warning(),
            "`vmess` without `vmess-aead=true` uses the AEAD handshake all the same: rurge has no other, and every server since V2Ray 4.28.1 accepts it"
        );
        assert_eq!(
            aead.imported(),
            "imported `vmess` policies without `vmess-aead=true` use the AEAD handshake"
        );
        let stream = Remark::SsStreamCipher("rc4-md5");
        assert_eq!((stream.code(), codes::W_STREAM_CIPHER), ("W0031", "W0031"));
        assert_eq!(
            stream.warning(),
            "`ss` cipher `rc4-md5` is a stream cipher: it gives no integrity protection and is deprecated; use an AEAD cipher"
        );
        assert_eq!(
            stream.imported(),
            "imported `ss` policies use the stream cipher `rc4-md5`: it gives no integrity protection and is deprecated"
        );
        // `rc4` has no IV: what else is wrong with it is said too
        let rc4 = Remark::SsStreamCipher("rc4");
        assert_eq!(rc4.code(), codes::W_STREAM_CIPHER);
        assert_eq!(
            rc4.warning(),
            "`ss` cipher `rc4` is a stream cipher: it gives no integrity protection and is deprecated; use an AEAD cipher; `rc4` also repeats its keystream on every connection"
        );
        assert_eq!(
            rc4.imported(),
            "imported `ss` policies use the stream cipher `rc4`: it gives no integrity protection and is deprecated; `rc4` also repeats its keystream on every connection"
        );
    }

    /// `ecn` marks the packets of a QUIC connection: a protocol over TCP has
    /// none of its own to mark, so it does not apply there (phase 2 M8
    /// design 4.5). It takes effect over QUIC, and `wireguard` only parses
    /// it so far.
    #[test]
    fn ecn_does_not_apply_to_a_protocol_over_tcp() {
        for (def, kind) in [
            ("trojan, h.test, 443, password=p, ecn=on", "trojan"),
            ("http, h.test, 80, ecn=off", "http"),
            ("socks5-tls, h.test, 1080, ecn=auto", "socks5-tls"),
            (
                "ss, h.test, 8388, encrypt-method=aes-128-gcm, password=p, ecn=on",
                "ss",
            ),
            ("snell, h.test, 443, psk=p, version=4, ecn=on", "snell"),
            ("anytls, h.test, 443, password=p, ecn=on", "anytls"),
            ("h2-connect, h.test, 443, ecn=on", "h2-connect"),
            ("ssh, h.test, 22, username=u, password=p, ecn=on", "ssh"),
        ] {
            let o = outcome("P", def);
            let found: Vec<(&str, &str)> = o
                .diagnostics
                .iter()
                .map(|d| (d.code, d.message.as_str()))
                .collect();
            assert_eq!(
                found,
                [(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!("policy `P`: `ecn` does not apply to `{kind}` policies; ignored")
                        .as_str()
                )],
                "{def}"
            );
            assert!(o.inert.is_empty(), "{def}: {:?}", o.inert);
            let spec = o.spec.unwrap_or_else(|| panic!("{def}: no spec"));
            assert_eq!(spec.common.ecn, Tristate::Auto, "{def}");
        }
        // over QUIC it takes effect: nothing is said
        let o = outcome("H", "hysteria2, h.test, 443, password=pw, ecn=off");
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert_eq!(o.spec.expect("a spec").common.ecn, Tristate::Off);
        // `wireguard` keeps its "parsed but has no effect"
        let o = outcome("W", "wireguard, section-name=home, ecn=on");
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert_eq!(o.inert, ["ecn"]);
        assert_eq!(o.spec.expect("a spec").common.ecn, Tristate::On);
        // a wrong value is an error wherever it is written, and the one word
        // about it: neither that it does not apply nor, on `wireguard`,
        // that it has no effect yet
        for def in [
            "trojan, h.test, 443, password=p, ecn=maybe",
            "wireguard, section-name=home, ecn=maybe",
        ] {
            let o = outcome("P", def);
            assert!(o.spec.is_none(), "{def}");
            let found: Vec<(&str, &str)> = o
                .diagnostics
                .iter()
                .map(|d| (d.code, d.message.as_str()))
                .collect();
            assert_eq!(
                found,
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: invalid value `maybe` for `ecn` (expected auto / on / off / true / false)"
                )],
                "{def}"
            );
            assert!(o.inert.is_empty(), "{def}: {:?}", o.inert);
        }
    }

    /// An `ss` line is read and checked in full. A stream cipher of the
    /// manual has a spec like any other method, and the loader is given a
    /// word to say about it; a cipher Surge no longer lists has no spec and
    /// says why (phase 2 M8 design 4.1).
    #[test]
    fn an_ss_line_is_checked_a_stream_cipher_is_remarked_on_and_an_unlisted_one_has_no_spec() {
        for (method, expected) in [
            ("aes-128-gcm", SsMethod::Aes128Gcm),
            ("RC4-MD5", SsMethod::Rc4Md5),
            ("aes-256-cfb", SsMethod::Aes256Cfb),
        ] {
            let o = outcome(
                "S",
                &format!(
                    "ss, h.test, 8388, encrypt-method={method}, password=pw, udp-relay=true, udp-port=8389, obfs=tls, obfs-host=cdn.test, shadow-tls-password=st"
                ),
            );
            assert!(o.diagnostics.is_empty(), "{method}: {:?}", o.diagnostics);
            assert!(o.inert.is_empty(), "{method}: {:?}", o.inert);
            assert_eq!(o.not_implemented, None, "{method}");
            // the loader says it once per load and cipher, not this line
            let remarks = match expected.iv_len() {
                Some(_) => vec![Remark::SsStreamCipher(expected.name())],
                None => Vec::new(),
            };
            assert_eq!(o.remarks, remarks, "{method}");
            let spec = o.spec.unwrap_or_else(|| panic!("{method}: no spec"));
            let ProtoSpec::Ss(ss) = &spec.proto else {
                panic!("{:?}", spec.proto)
            };
            assert_eq!(
                (ss.method, ss.udp_relay, ss.udp_port),
                (expected, true, Some(8389)),
                "{method}"
            );
            assert_eq!(ss.obfs.as_ref().map(|o| o.mode), Some(ObfsMode::Tls));
            assert!(spec.shadow_tls.is_some(), "{method}");
        }

        let o = outcome("S", "ss, h.test, 8388, encrypt-method=BF-CFB, password=pw");
        assert!(o.spec.is_none() && o.diagnostics.is_empty());
        assert!(o.remarks.is_empty(), "{:?}", o.remarks);
        let why = o.not_implemented.expect("a cipher that is not supported");
        assert_eq!(why, NotImplemented::SsCipher("bf-cfb"));
        assert_eq!(
            why.warning(),
            "`ss` cipher `bf-cfb` is not supported (Surge no longer lists it); such policies behave as REJECT"
        );
        assert_eq!(why.note(), "ss (bf-cfb)");
        assert_eq!(why.imported(), "`ss` with the cipher `bf-cfb`");

        // a broken line is an error like any other: neither "not supported"
        // nor anything to remark on
        for method in ["bf-cfb", "rc4-md5"] {
            let o = outcome("S", &format!("ss, h.test, 8388, encrypt-method={method}"));
            assert!(o.spec.is_none() && o.not_implemented.is_none(), "{method}");
            assert!(o.remarks.is_empty(), "{method}: {:?}", o.remarks);
            assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
        }
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

    /// `udp-port` on `snell` (the manual: "another server port number" for
    /// UDP forwarding) is not followed: UDP keeps to the policy's own port
    /// and layers, and the value is only checked.
    #[test]
    fn udp_port_on_snell_is_parsed_but_has_no_effect() {
        for version in 1..=5 {
            let o = outcome(
                "N",
                &format!("snell, h.test, 443, psk=pw, version={version}, udp-port=8443"),
            );
            assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
            assert_eq!(o.inert, ["udp-port"], "version {version}");
            assert!(o.spec.is_some(), "version {version}");
        }
        // a version that is not supported has a word of its own already
        let o = outcome("N", "snell, h.test, 443, psk=pw, version=6, udp-port=8443");
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        let o = outcome("N", "snell, h.test, 443, psk=pw, version=5, udp-port=0");
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
    }

    /// An `external` policy in downloaded text is not one rurge lacks the
    /// protocol for but one it will not start: the warning is the one about
    /// downloaded text, with a suggestion of its own, and the others stay
    /// `W0007` without one.
    #[test]
    fn a_downloaded_external_is_not_started_and_says_so_apart_from_a_missing_protocol() {
        let why = NotImplemented::DownloadedExternal;
        assert_eq!(
            why.warning(),
            "`external` policies written in downloaded text are not started; they behave as REJECT"
        );
        assert_eq!(why.note(), "external (downloaded text)");
        assert_eq!(why.imported(), "`external` in downloaded text");
        let d = why.diagnostic();
        assert_eq!(d.code, codes::W_REMOTE_LOCAL_ACCESS);
        assert_eq!(d.message, why.warning());
        assert_eq!(
            d.hint,
            Some("define the policy in the local profile to use it")
        );
        for other in [
            NotImplemented::SsCipher("bf-cfb"),
            NotImplemented::SnellVersion(6),
        ] {
            let d = other.diagnostic();
            assert_eq!(
                (d.code, d.message.as_str()),
                (codes::W_PROTOCOL_NOT_IMPLEMENTED, other.warning().as_str())
            );
            assert_eq!(d.hint, None);
        }
    }

    /// The copy of a managed profile is downloaded text of the other kind:
    /// the same refusal, in words of its own, and the way to get a program
    /// started is another one. Which text gets which is `external_in`'s to
    /// say.
    #[test]
    fn a_managed_external_has_the_words_of_the_copy_of_a_managed_profile() {
        let why = NotImplemented::ManagedExternal;
        assert_eq!(
            why.warning(),
            "`external` policies in the downloaded copy of a managed profile are not started; they behave as REJECT"
        );
        assert_eq!(why.note(), "external (managed profile)");
        assert_eq!(why.imported(), "`external` in a managed profile");
        let d = why.diagnostic();
        assert_eq!(d.code, codes::W_REMOTE_LOCAL_ACCESS);
        assert_eq!(d.message, why.warning());
        assert_eq!(
            d.hint,
            Some(
                "to use an `external` policy, load a profile without `#!MANAGED-CONFIG` that takes the downloaded parts in by `#!include`"
            )
        );
        assert_eq!(NotImplemented::external_in(Reach::Anywhere), None);
        assert_eq!(
            NotImplemented::external_in(Reach::ManagedCopy),
            Some(NotImplemented::ManagedExternal)
        );
        assert_eq!(
            NotImplemented::external_in(Reach::Nowhere),
            Some(NotImplemented::DownloadedExternal)
        );
    }

    /// A `snell` line is read and checked in full, and versions 1 to 5 have
    /// a spec; version 6 has none and says why (phase 2 M8 design 4.3).
    #[test]
    fn a_snell_line_is_checked_and_version_6_is_not_supported() {
        let o = outcome(
            "N",
            "snell, h.test, 443, psk=pw, version=5, reuse=true, obfs=http, obfs-host=cdn.test, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert_eq!(o.not_implemented, None);
        let spec = o.spec.expect("a version 5 line has a spec");
        let ProtoSpec::Snell(snell) = &spec.proto else {
            panic!("{:?}", spec.proto)
        };
        assert_eq!((snell.version, snell.reuse), (SnellVersion::V5, true));
        assert_eq!(snell.obfs.as_ref().map(|o| o.mode), Some(ObfsMode::Http));
        assert!(spec.shadow_tls.is_some());

        // without `version`: Surge's default, 1, which never reuses a
        // connection; version 2 always does
        for (def, version, reuse, obfs) in [
            ("snell, h.test, 443, psk=pw", SnellVersion::V1, false, None),
            (
                "snell, h.test, 443, psk=pw, version=2, obfs=tls",
                SnellVersion::V2,
                true,
                Some(ObfsMode::Tls),
            ),
            (
                "snell, h.test, 443, psk=pw, version=3, obfs=http",
                SnellVersion::V3,
                false,
                Some(ObfsMode::Http),
            ),
        ] {
            let o = outcome("N", def);
            assert!(o.diagnostics.is_empty(), "{def}: {:?}", o.diagnostics);
            assert_eq!(o.not_implemented, None, "{def}");
            let spec = o.spec.unwrap_or_else(|| panic!("{def}: no spec"));
            let ProtoSpec::Snell(snell) = &spec.proto else {
                panic!("{:?}", spec.proto)
            };
            assert_eq!((snell.version, snell.reuse), (version, reuse), "{def}");
            assert_eq!(snell.obfs.as_ref().map(|o| o.mode), obfs, "{def}");
        }
        // `reuse` against what the version does is said, and changes nothing
        let o = outcome("N", "snell, h.test, 443, psk=pw, reuse=true");
        let found: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `N`: `reuse` does not apply to `snell` version 1: version 2 always reuses a connection, versions 1 and 3 never do"
            )]
        );
        assert!(matches!(
            o.spec.map(|s| s.proto),
            Some(ProtoSpec::Snell(SnellSpec { reuse: false, .. }))
        ));
        // version 6 is out of scope, not a matter of time: the text says why
        let o = outcome("N", "snell, h.test, 443, psk=pw, version=6");
        assert!(o.spec.is_none() && o.diagnostics.is_empty());
        let why = o.not_implemented.expect("version 6");
        assert_eq!(why, NotImplemented::SnellVersion(6));
        assert_eq!(
            why.warning(),
            "`snell` version 6 is not supported: it is a beta protocol without a public specification; such policies behave as REJECT"
        );
        assert_eq!(why.note(), "snell v6");
        assert_eq!(why.imported(), "`snell` version 6");

        // a broken line is an error like any other, and not "not supported"
        let o = outcome("N", "snell, h.test, 443, version=6");
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
                obfs: None,
            })
            .keystore_item(),
            None
        );
    }

    /// `h2-connect` and `trust-tunnel` lines are read and checked in full,
    /// and a sound one has a spec (phase 2 M6 design 5.1).
    #[test]
    fn h2_connect_and_trust_tunnel_lines_are_checked_and_have_a_spec() {
        for def in [
            "h2-connect, 1.2.3.4, 443, max-streams=5",
            "h2-connect, h.test, 443, user, pass, udp-relay=true, client-cert=cert1, shadow-tls-password=st, underlying-proxy=Entry",
            "trust-tunnel, 192.168.20.62, 443, username=test, password=test",
            "trust-tunnel, h.test, 443, username=u, password=p, client-cert=cert1, shadow-tls-password=st, underlying-proxy=Pick",
        ] {
            let o = outcome("T", def);
            assert!(o.diagnostics.is_empty(), "{def}: {:?}", o.diagnostics);
            assert!(o.inert.is_empty(), "{def}: {:?}", o.inert);
            assert_eq!(o.not_implemented, None, "{def}");
            let spec = o.spec.unwrap_or_else(|| panic!("{def}: no spec"));
            assert!(
                matches!(
                    spec.proto,
                    ProtoSpec::H2Connect(_) | ProtoSpec::TrustTunnel(_)
                ),
                "{def}"
            );
            let layered = def.contains("shadow-tls-password");
            assert_eq!(spec.shadow_tls.is_some(), layered, "{def}");
            assert_eq!(spec.common.underlying_proxy.is_some(), layered, "{def}");
        }
        // `h3=true` takes effect: nothing is said of it
        let o = outcome(
            "T",
            "trust-tunnel, h.test, 443, username=u, password=p, h3=true",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert!(matches!(
            o.spec.map(|s| s.proto),
            Some(ProtoSpec::TrustTunnel(TrustTunnelSpec { h3: true, .. }))
        ));
        let o = outcome("T", "trust-tunnel, h.test, 443, max-streams=0");
        let found: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: `username` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: `password` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: invalid value `0` for `max-streams` (expected an integer, at least 1)"
                ),
            ]
        );
        assert!(o.spec.is_none());
        // both always run over TLS: the client certificate is theirs
        let tls = TlsOpts {
            client_cert: Some("cert1".into()),
            ..TlsOpts::default()
        };
        let h2 = ProtoSpec::H2Connect(H2ConnectSpec {
            tls: tls.clone(),
            username: None,
            password: None,
            headers: Vec::new(),
            max_streams: h2::DEFAULT_MAX_STREAMS,
            udp_relay: false,
        });
        assert_eq!(h2.keystore_item(), Some("cert1"));
        let tt = ProtoSpec::TrustTunnel(TrustTunnelSpec {
            tls,
            username: "u".into(),
            password: "p".into(),
            headers: Vec::new(),
            max_streams: h2::DEFAULT_MAX_STREAMS,
            h3: false,
        });
        assert_eq!(tt.keystore_item(), Some("cert1"));
        assert!(tt.tls().is_some());
    }

    /// A `hysteria2` line is read and checked in full, and a sound one has a
    /// spec. `ecn` takes effect on it, `tfo` and `udp-relay` do not apply
    /// (phase 2 M7 design 3.1, 5.1).
    #[test]
    fn a_hysteria2_line_is_checked_and_has_a_spec() {
        for (def, chained) in [
            (
                "hysteria2, 192.168.20.6, 443, password=pwd, download-bandwidth=100",
                false,
            ),
            (
                "hysteria2, h.test, 443, password=pw, salamander-password=obfs, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, sni=edge.test, client-cert=cert1",
                false,
            ),
            (
                "hysteria2, h.test, 443, password=pw, gecko-password=obfs, alpn=h3, underlying-proxy=Pick",
                true,
            ),
            // DIRECT is no chain: the hops stay
            (
                "hysteria2, h.test, 443, password=pw, port-hopping=5000-6000, underlying-proxy=DIRECT",
                false,
            ),
        ] {
            let o = outcome("H", def);
            assert!(o.diagnostics.is_empty(), "{def}: {:?}", o.diagnostics);
            assert!(o.inert.is_empty(), "{def}: {:?}", o.inert);
            assert_eq!(o.not_implemented, None, "{def}");
            let spec = o.spec.unwrap_or_else(|| panic!("{def}: no spec"));
            assert!(matches!(spec.proto, ProtoSpec::Hysteria2(_)), "{def}");
            assert_eq!(spec.common.underlying_proxy.is_some(), chained, "{def}");
        }
        let o = outcome(
            "H",
            "hysteria2, h.test, 443, s3cret, tfo=true, udp-relay=true, mystery=1",
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
                    "policy `H`: `tfo` does not apply to `hysteria2` policies; ignored"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `H`: `password` is required"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `H`: `udp-relay` does not apply to `hysteria2` policies (UDP is always relayed); ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `H`: unknown parameter `mystery` ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `H`: unexpected positional value #1 ignored"
                ),
            ]
        );
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert!(o.spec.is_none());
    }

    /// What a QUIC-based line cannot combine (phase 2 M7 design 3.1): port
    /// hopping with a chain (manual), and Shadow TLS, which wraps a TCP
    /// connection. `underlying-proxy=DIRECT` is no chain.
    #[test]
    fn port_hopping_over_a_chain_and_shadow_tls_are_errors_on_hysteria2() {
        for relay in ["Entry", "Pick"] {
            let o = outcome(
                "H",
                &format!(
                    "hysteria2, h.test, 443, password=pw, port-hopping=5000-6000, underlying-proxy={relay}"
                ),
            );
            let found: Vec<(&str, &str)> = o
                .diagnostics
                .iter()
                .map(|d| (d.code, d.message.as_str()))
                .collect();
            assert_eq!(
                found,
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `H`: `port-hopping` cannot be combined with `underlying-proxy`"
                )],
                "{relay}"
            );
        }
        let o = outcome(
            "H",
            "hysteria2, h.test, 443, password=pw, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test",
        );
        let found: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `H`: Shadow TLS cannot be combined with a `hysteria2` policy"
            )]
        );
    }

    /// Hysteria 2 runs TLS over QUIC: the TLS options, and with them the
    /// client certificate, and the QUIC options are its own.
    #[test]
    fn a_hysteria2_spec_has_tls_and_quic_options() {
        let hysteria2 = ProtoSpec::Hysteria2(Hysteria2Spec {
            tls: TlsOpts {
                alpn: vec!["h3".into()],
                client_cert: Some("cert1".into()),
                ..TlsOpts::default()
            },
            quic: QuicOpts {
                port_hopping: Some(PortHopping {
                    ports: vec![1234..=1234, 5000..=6000],
                    interval: quic::DEFAULT_HOP_INTERVAL,
                }),
            },
            password: "pw".into(),
            download_bandwidth: 0,
            obfs: Some(Hysteria2Obfs::Salamander("obfs".into())),
        });
        assert_eq!(hysteria2.keystore_item(), Some("cert1"));
        assert_eq!(
            hysteria2.tls().map(|tls| tls.alpn.clone()),
            Some(vec!["h3".to_string()])
        );
        assert!(
            hysteria2
                .quic()
                .is_some_and(|quic| quic.port_hopping.is_some())
        );
        let tcp = outcome("A", "anytls, h.test, 443, password=pw")
            .spec
            .unwrap();
        assert!(tcp.proto.quic().is_none() && ProtoSpec::Direct.quic().is_none());
    }

    /// `tuic` and `tuic-v5` lines are read and checked in full, and a sound
    /// one has a spec, of the version its keyword names. `ecn` takes effect
    /// on them, `tfo` and `udp-relay` do not apply, and neither version
    /// takes the other's credentials (phase 2 M7 design 3.1, 6.1).
    #[test]
    fn a_tuic_line_is_checked_and_has_a_spec() {
        let id = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
        for (def, kind, chained) in [
            (
                "tuic, 192.168.20.6, 443, token=pwd, alpn=h3".to_string(),
                PolicyKind::Tuic,
                false,
            ),
            (
                format!("tuic-v5, 192.168.20.6, 443, uuid={id}, password=pwd, alpn=h3"),
                PolicyKind::TuicV5,
                false,
            ),
            (
                format!(
                    "tuic-v5, h.test, 443, uuid={id}, password=pw, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, sni=edge.test, client-cert=cert1"
                ),
                PolicyKind::TuicV5,
                false,
            ),
            (
                "tuic, h.test, 443, token=t, ecn=on, underlying-proxy=Pick".to_string(),
                PolicyKind::Tuic,
                true,
            ),
            // DIRECT is no chain: the hops stay
            (
                "tuic, h.test, 443, token=t, port-hopping=5000-6000, underlying-proxy=DIRECT"
                    .to_string(),
                PolicyKind::Tuic,
                false,
            ),
        ] {
            let o = outcome("T", &def);
            assert!(o.diagnostics.is_empty(), "{def}: {:?}", o.diagnostics);
            assert!(o.inert.is_empty(), "{def}: {:?}", o.inert);
            assert_eq!(o.not_implemented, None, "{def}");
            let spec = o.spec.unwrap_or_else(|| panic!("{def}: no spec"));
            let ProtoSpec::Tuic(tuic) = &spec.proto else {
                panic!("{def}: {:?}", spec.proto)
            };
            assert_eq!(spec.kind, kind, "{def}");
            assert_eq!(
                matches!(tuic.version, TuicVersion::V5 { .. }),
                kind == PolicyKind::TuicV5,
                "{def}"
            );
            assert_eq!(spec.common.underlying_proxy.is_some(), chained, "{def}");
        }
        let o = outcome(
            "T",
            "tuic-v5, h.test, 443, s3cret, tfo=true, udp-relay=true, token=t, mystery=1",
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
                    "policy `T`: `tfo` does not apply to `tuic-v5` policies; ignored"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: `uuid` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: `password` is required"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `T`: `token` only applies to `tuic` policies; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `T`: `udp-relay` does not apply to `tuic-v5` policies (UDP is always relayed); ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `T`: unknown parameter `mystery` ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `T`: unexpected positional value #1 ignored"
                ),
            ]
        );
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert!(o.spec.is_none());
    }

    /// What a QUIC-based line cannot combine (phase 2 M7 design 3.1), on
    /// either TUIC version: port hopping with a chain, and Shadow TLS (the
    /// manual: a configuration error).
    #[test]
    fn port_hopping_over_a_chain_and_shadow_tls_are_errors_on_tuic() {
        let v5 = "uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=pw";
        for (kind, credentials) in [("tuic", "token=t"), ("tuic-v5", v5)] {
            for relay in ["Entry", "Pick"] {
                let o = outcome(
                    "T",
                    &format!(
                        "{kind}, h.test, 443, {credentials}, port-hopping=5000-6000, underlying-proxy={relay}"
                    ),
                );
                let found: Vec<(&str, &str)> = o
                    .diagnostics
                    .iter()
                    .map(|d| (d.code, d.message.as_str()))
                    .collect();
                assert_eq!(
                    found,
                    [(
                        codes::E_INVALID_POLICY_PARAM,
                        "policy `T`: `port-hopping` cannot be combined with `underlying-proxy`"
                    )],
                    "{kind} {relay}"
                );
            }
            let o = outcome(
                "T",
                &format!(
                    "{kind}, h.test, 443, {credentials}, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test"
                ),
            );
            let found: Vec<(&str, String)> = o
                .diagnostics
                .iter()
                .map(|d| (d.code, d.message.clone()))
                .collect();
            assert_eq!(
                found,
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!("policy `T`: Shadow TLS cannot be combined with a `{kind}` policy")
                )]
            );
        }
    }

    /// TUIC runs TLS over QUIC, whichever the version: the TLS options, and
    /// with them the client certificate, and the QUIC options are its own.
    #[test]
    fn a_tuic_spec_has_tls_and_quic_options() {
        for version in [
            TuicVersion::V4 { token: "t".into() },
            TuicVersion::V5 {
                uuid: Secret::new([7; 16]),
                password: "pw".into(),
            },
        ] {
            let tuic = ProtoSpec::Tuic(TuicSpec {
                tls: TlsOpts {
                    alpn: vec!["h3".into()],
                    client_cert: Some("cert1".into()),
                    ..TlsOpts::default()
                },
                quic: QuicOpts {
                    port_hopping: Some(PortHopping {
                        ports: vec![5000..=6000],
                        interval: quic::DEFAULT_HOP_INTERVAL,
                    }),
                },
                version,
            });
            assert_eq!(tuic.keystore_item(), Some("cert1"));
            assert_eq!(
                tuic.tls().map(|tls| tls.alpn.clone()),
                Some(vec!["h3".to_string()])
            );
            assert!(tuic.quic().is_some_and(|quic| quic.port_hopping.is_some()));
        }
    }

    /// `masque` lines are read and checked in full, and a sound one has a
    /// spec, with the credentials or without. `ecn` takes effect on them,
    /// `tfo` and `udp-relay` do not apply, the credentials go together, and
    /// what the manual does not list is unknown (phase 2 M7 design 3.1,
    /// 7.1).
    #[test]
    fn a_masque_line_is_checked_and_has_a_spec() {
        for (def, credentials, chained) in [
            (
                "masque, example.com, 443, username=user, password=pass",
                true,
                false,
            ),
            ("masque, h.test, 443", false, false),
            (
                "masque, h.test, 443, username=u, password=pw, port-hopping=1234;5000-6000, port-hopping-interval=10, ecn=off, alpn=h3, sni=edge.test, client-cert=cert1",
                true,
                false,
            ),
            (
                "masque, h.test, 443, ecn=on, underlying-proxy=Pick",
                false,
                true,
            ),
            // DIRECT is no chain: the hops stay
            (
                "masque, h.test, 443, port-hopping=5000-6000, underlying-proxy=DIRECT",
                false,
                false,
            ),
        ] {
            let o = outcome("M", def);
            assert!(o.diagnostics.is_empty(), "{def}: {:?}", o.diagnostics);
            assert!(o.inert.is_empty(), "{def}: {:?}", o.inert);
            assert_eq!(o.not_implemented, None, "{def}");
            let spec = o.spec.unwrap_or_else(|| panic!("{def}: no spec"));
            let ProtoSpec::Masque(masque) = &spec.proto else {
                panic!("{def}: {:?}", spec.proto)
            };
            assert_eq!(masque.credentials.is_some(), credentials, "{def}");
            assert_eq!(spec.common.underlying_proxy.is_some(), chained, "{def}");
        }
        let o = outcome(
            "M",
            "masque, h.test, 443, s3cret, tfo=true, udp-relay=true, username=s3cretUser, max-streams=5, headers=X-A:1",
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
                    "policy `M`: `tfo` does not apply to `masque` policies; ignored"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `M`: `password` is required with `username`"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `M`: `udp-relay` does not apply to `masque` policies (UDP is always relayed); ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `M`: unknown parameter `max-streams` ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `M`: unknown parameter `headers` ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `M`: unexpected positional value #1 ignored"
                ),
            ]
        );
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert!(o.spec.is_none());
    }

    /// What a QUIC-based line cannot combine (phase 2 M7 design 3.1), on
    /// `masque` as on the others: port hopping with a chain, and Shadow TLS
    /// (manual: it "cannot be combined with QUIC-based protocols").
    #[test]
    fn port_hopping_over_a_chain_and_shadow_tls_are_errors_on_masque() {
        for relay in ["Entry", "Pick"] {
            let o = outcome(
                "M",
                &format!("masque, h.test, 443, port-hopping=5000-6000, underlying-proxy={relay}"),
            );
            let found: Vec<(&str, &str)> = o
                .diagnostics
                .iter()
                .map(|d| (d.code, d.message.as_str()))
                .collect();
            assert_eq!(
                found,
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `M`: `port-hopping` cannot be combined with `underlying-proxy`"
                )],
                "{relay}"
            );
        }
        let o = outcome(
            "M",
            "masque, h.test, 443, username=u, password=pw, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test",
        );
        let found: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `M`: Shadow TLS cannot be combined with a `masque` policy"
            )]
        );
    }

    /// MASQUE runs TLS over QUIC: the TLS options, and with them the client
    /// certificate, and the QUIC options are its own.
    #[test]
    fn a_masque_spec_has_tls_and_quic_options() {
        let masque = ProtoSpec::Masque(MasqueSpec {
            tls: TlsOpts {
                alpn: vec!["h3".into()],
                client_cert: Some("cert1".into()),
                ..TlsOpts::default()
            },
            quic: QuicOpts {
                port_hopping: Some(PortHopping {
                    ports: vec![5000..=6000],
                    interval: quic::DEFAULT_HOP_INTERVAL,
                }),
            },
            credentials: Some(MasqueCredentials {
                username: "u".into(),
                password: "pw".into(),
            }),
        });
        assert_eq!(masque.keystore_item(), Some("cert1"));
        assert_eq!(
            masque.tls().map(|tls| tls.alpn.clone()),
            Some(vec!["h3".to_string()])
        );
        assert!(
            masque
                .quic()
                .is_some_and(|quic| quic.port_hopping.is_some())
        );
    }

    /// `h3=true` runs a `trust-tunnel` over QUIC (phase 2 M7 design 7.2):
    /// the spec says so and pins the ALPN to `h3`; `ecn` takes effect, `tfo`
    /// does not apply, and port hopping is no parameter of it. `h3` itself
    /// takes effect: nothing is said of it.
    #[test]
    fn a_trust_tunnel_line_with_h3_runs_over_quic() {
        let o = outcome(
            "T",
            "trust-tunnel, h.test, 443, username=u, password=p, h3=true, alpn=h2, ecn=off, tfo=true, port-hopping=5000-6000",
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
                    "policy `T`: `alpn` is always `h3` on `trust-tunnel` policies with `h3=true`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `T`: `tfo` does not apply to `trust-tunnel` policies with `h3=true`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `T`: `port-hopping` does not apply to `trust-tunnel` policies; ignored"
                ),
            ]
        );
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        let spec = o.spec.expect("a spec");
        let ProtoSpec::TrustTunnel(tt) = &spec.proto else {
            panic!("{:?}", spec.proto)
        };
        assert!(tt.h3);
        assert_eq!(tt.tls.alpn, ["h3"]);
        assert_eq!((spec.common.ecn, spec.common.tfo), (Tristate::Off, false));
        // no QUIC options of its own: port hopping does not apply
        assert!(spec.proto.quic().is_none());

        // over HTTP/2 `tfo` is parsed but has no effect yet, and there is
        // no QUIC packet for `ecn` to mark
        let o = outcome(
            "T",
            "trust-tunnel, h.test, 443, username=u, password=p, ecn=off, tfo=true",
        );
        let found: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `T`: `ecn` does not apply to `trust-tunnel` policies without `h3=true`; ignored"
            )]
        );
        assert_eq!(o.inert, ["tfo"]);
        let spec = o.spec.expect("a spec");
        let ProtoSpec::TrustTunnel(tt) = &spec.proto else {
            panic!("{:?}", spec.proto)
        };
        assert!(!tt.h3);
        assert_eq!(tt.tls.alpn, ["h2"]);
        assert_eq!(spec.common.ecn, Tristate::Auto);

        // an `h3` that is an error already leaves open which of the two it
        // runs over: nothing is said of `ecn`
        let o = outcome(
            "T",
            "trust-tunnel, h.test, 443, username=u, password=p, h3=maybe, ecn=off",
        );
        let found: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `T`: invalid value `maybe` for `h3` (expected true or false)"
            )]
        );
        assert!(o.spec.is_none());
    }

    /// Shadow TLS wraps a TCP connection: a `trust-tunnel` has it over
    /// HTTP/2, not over HTTP/3 (phase 2 M7 design 3.1). One error, whatever
    /// the Shadow TLS parameters say.
    #[test]
    fn shadow_tls_is_an_error_on_a_trust_tunnel_with_h3() {
        for layer in [
            "shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test",
            // errors of their own over HTTP/2
            "shadow-tls-password=, shadow-tls-version=3",
        ] {
            let o = outcome(
                "T",
                &format!("trust-tunnel, h.test, 443, username=u, password=p, h3=true, {layer}"),
            );
            let found: Vec<(&str, &str)> = o
                .diagnostics
                .iter()
                .map(|d| (d.code, d.message.as_str()))
                .collect();
            assert_eq!(
                found,
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: Shadow TLS cannot be combined with `h3=true` on a `trust-tunnel` policy"
                )],
                "{layer}"
            );
            assert!(o.spec.is_none(), "{layer}");
        }
        // over HTTP/2 the layer is the policy's
        let o = outcome(
            "T",
            "trust-tunnel, h.test, 443, username=u, password=p, h3=false, shadow-tls-password=st",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert!(o.spec.and_then(|spec| spec.shadow_tls).is_some());
    }
}
