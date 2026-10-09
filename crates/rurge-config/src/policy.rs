//! `[Proxy]` policies and `[Proxy Group]` groups.

use crate::diagnostic::{ParseError, codes};
use crate::glob::{Glob, GlobOptions};
use crate::span::Span;
use crate::types::HostName;
use crate::value::{ParamMap, parse_key_value, split_list, strip_prefix_ci, unquote};
use std::collections::HashSet;
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PolicyKind {
    Http,
    Https,
    H2Connect,
    Socks5,
    Socks5Tls,
    Shadowsocks,
    Snell,
    Vmess,
    Trojan,
    Tuic,
    TuicV5,
    Hysteria2,
    Masque,
    AnyTls,
    TrustTunnel,
    Ssh,
    WireGuard,
    Tailscale,
    External,
    Direct,
    Reject,
    RejectDrop,
    RejectNoDrop,
    RejectTinyGif,
}

const POLICY_KEYWORDS: &[(&str, PolicyKind)] = &[
    ("http", PolicyKind::Http),
    ("https", PolicyKind::Https),
    ("h2-connect", PolicyKind::H2Connect),
    ("socks5", PolicyKind::Socks5),
    ("socks5-tls", PolicyKind::Socks5Tls),
    ("ss", PolicyKind::Shadowsocks),
    ("snell", PolicyKind::Snell),
    ("vmess", PolicyKind::Vmess),
    ("trojan", PolicyKind::Trojan),
    ("tuic", PolicyKind::Tuic),
    ("tuic-v5", PolicyKind::TuicV5),
    ("hysteria2", PolicyKind::Hysteria2),
    ("masque", PolicyKind::Masque),
    ("anytls", PolicyKind::AnyTls),
    ("trust-tunnel", PolicyKind::TrustTunnel),
    ("ssh", PolicyKind::Ssh),
    ("wireguard", PolicyKind::WireGuard),
    ("tailscale", PolicyKind::Tailscale),
    ("external", PolicyKind::External),
    ("direct", PolicyKind::Direct),
    ("reject", PolicyKind::Reject),
    ("reject-drop", PolicyKind::RejectDrop),
    ("reject-no-drop", PolicyKind::RejectNoDrop),
    ("reject-tinygif", PolicyKind::RejectTinyGif),
];

impl PolicyKind {
    pub fn parse(keyword: &str) -> Option<PolicyKind> {
        let kw = keyword.trim().to_ascii_lowercase();
        POLICY_KEYWORDS
            .iter()
            .find(|(k, _)| *k == kw)
            .map(|(_, v)| *v)
    }
    pub fn keyword(&self) -> &'static str {
        POLICY_KEYWORDS
            .iter()
            .find(|(_, v)| v == self)
            .map(|(k, _)| *k)
            .unwrap_or("unknown")
    }
    pub fn is_builtin_alias(&self) -> bool {
        matches!(
            self,
            PolicyKind::Direct
                | PolicyKind::Reject
                | PolicyKind::RejectDrop
                | PolicyKind::RejectNoDrop
                | PolicyKind::RejectTinyGif
        )
    }
    pub fn takes_server(&self) -> bool {
        !self.is_builtin_alias()
            && !matches!(
                self,
                PolicyKind::WireGuard | PolicyKind::Tailscale | PolicyKind::External
            )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Builtin {
    Direct,
    Reject,
    RejectDrop,
    RejectNoDrop,
    RejectTinyGif,
    Cellular,
    CellularOnly,
    Hybrid,
    NoHybrid,
}

const BUILTIN_NAMES: &[(&str, Builtin)] = &[
    ("DIRECT", Builtin::Direct),
    ("REJECT", Builtin::Reject),
    ("REJECT-DROP", Builtin::RejectDrop),
    ("REJECT-NO-DROP", Builtin::RejectNoDrop),
    ("REJECT-TINYGIF", Builtin::RejectTinyGif),
    ("CELLULAR", Builtin::Cellular),
    ("CELLULAR-ONLY", Builtin::CellularOnly),
    ("HYBRID", Builtin::Hybrid),
    ("NO-HYBRID", Builtin::NoHybrid),
];

impl Builtin {
    /// Built-in names are case-sensitive upper-case keywords.
    pub fn parse(name: &str) -> Option<Builtin> {
        BUILTIN_NAMES
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
    }
    pub fn name(&self) -> &'static str {
        BUILTIN_NAMES
            .iter()
            .find(|(_, v)| v == self)
            .map(|(k, _)| *k)
            .unwrap_or("DIRECT")
    }
    pub fn is_reject(&self) -> bool {
        matches!(
            self,
            Builtin::Reject | Builtin::RejectDrop | Builtin::RejectNoDrop | Builtin::RejectTinyGif
        )
    }
    pub fn is_ios_only(&self) -> bool {
        matches!(
            self,
            Builtin::Cellular | Builtin::CellularOnly | Builtin::Hybrid | Builtin::NoHybrid
        )
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProxyPolicy {
    pub name: String,
    pub kind: PolicyKind,
    pub server: Option<HostName>,
    pub port: Option<u16>,
    pub positional: Vec<String>,
    pub params: ParamMap,
    /// The text right of `name =` as written (API policy detail, `lineHash`).
    pub definition: String,
    pub span: Span,
}

/// `positional` and `params` are left out and `definition` redacted, as a
/// group's are: they hold the line's credentials as written.
impl std::fmt::Debug for ProxyPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyPolicy")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("server", &self.server)
            .field("port", &self.port)
            .field("span", &self.span)
            .field(
                "definition",
                &crate::redact::redact_definition(&self.definition),
            )
            .finish_non_exhaustive()
    }
}

/// Whether `s` looks enough like a hand-written keyword to quote safely in a
/// diagnostic: a stray value that ended up as the type keyword because a
/// comma or an `=` was missing — a whole URL, a password, a `vmess://`
/// link — never looks like this (M3-D7).
fn looks_like_keyword(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `unknown type` (`E0004`), the keyword quoted only when `looks_like_keyword`.
fn unknown_type(type_kw: &str) -> String {
    if looks_like_keyword(type_kw) {
        format!("unknown type `{type_kw}`")
    } else {
        "unknown type".to_string()
    }
}

pub fn parse_policy(name: &str, definition: &str, span: &Span) -> Result<ProxyPolicy, ParseError> {
    let fields = split_list(definition);
    let Some(type_kw) = fields.first() else {
        return Err(ParseError::new(
            codes::E_SYNTAX,
            format!("policy `{name}`: missing type"),
        ));
    };
    let kind = PolicyKind::parse(type_kw).ok_or_else(|| {
        ParseError::new(
            codes::E_UNKNOWN_POLICY_TYPE,
            format!("policy `{name}`: {}", unknown_type(type_kw)),
        )
    })?;
    let (server, port, rest) = if kind.takes_server() {
        let server = fields.get(1).ok_or_else(|| {
            ParseError::new(
                codes::E_SYNTAX,
                format!(
                    "policy `{name}`: expected `{}, <server>, <port>`",
                    kind.keyword()
                ),
            )
        })?;
        let port_str = fields.get(2).ok_or_else(|| {
            ParseError::new(
                codes::E_SYNTAX,
                format!(
                    "policy `{name}`: expected `{}, <server>, <port>`",
                    kind.keyword()
                ),
            )
        })?;
        let port: u16 = port_str.parse().map_err(|_| {
            // anything but digits is another field that slid into the port's
            // place when a comma was missing — a password, say (M3-D7)
            let shown = if !port_str.is_empty()
                && port_str.len() <= 10
                && port_str.bytes().all(|b| b.is_ascii_digit())
            {
                format!(" `{port_str}`")
            } else {
                String::new()
            };
            ParseError::new(
                codes::E_SYNTAX,
                format!("policy `{name}`: invalid port{shown}"),
            )
        })?;
        (Some(HostName::parse(server)), Some(port), &fields[3..])
    } else {
        (None, None, &fields[1..])
    };
    let (params, positional) = ParamMap::from_fields(rest);
    Ok(ProxyPolicy {
        name: name.to_string(),
        kind,
        server,
        port,
        positional,
        params,
        definition: definition.trim().to_string(),
        span: span.clone(),
    })
}

/// `definition` with the parameters of `overrides` in place (phase 2 M3
/// design 5.3, `external-policy-modifier`): a key the line has is replaced
/// where it first appears and dropped where it repeats, a key it lacks is
/// appended. An item's key is the one the parser reads — with the quotes
/// around a whole item taken off, without its case; every other item keeps
/// its text. The fields the parser takes for the type, and for a server and
/// a port unless the type has none, are no parameters, however they are
/// spelled.
///
/// The line then reads as it did alone, the overrides in place. What is
/// appended therefore stands in front of a last item that leaves a quote or
/// a group open: the parser gives such an item the rest of the line.
pub fn with_params(definition: &str, overrides: &[(String, String)]) -> String {
    let mut written: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    let items = crate::redact::split_top_level(definition);
    // how many fields the parser reads before the parameters; an item it
    // drops, an empty one, is none of them
    let mut head = items
        .iter()
        .find_map(|item| split_list(item).pop())
        .and_then(|keyword| PolicyKind::parse(&keyword))
        .map_or(1, |kind| if kind.takes_server() { 3 } else { 1 });
    let mut open_tail = false;
    for (i, item) in items.iter().enumerate() {
        let field = split_list(item).pop();
        let key = match field.as_deref() {
            Some(_) if head > 0 => {
                head -= 1;
                None
            }
            Some(field) => parse_key_value(field).map(|(k, _)| k.to_ascii_lowercase()),
            None => None,
        };
        let found = key
            .as_ref()
            .and_then(|k| overrides.iter().find(|(o, _)| o.eq_ignore_ascii_case(k)));
        match found {
            Some((k, v)) => {
                if written.insert(k.to_ascii_lowercase()) {
                    let lead = &item[..item.len() - item.trim_start().len()];
                    out.push(format!("{lead}{k}={}", param_value(v)));
                }
            }
            None => {
                out.push(item.to_string());
                open_tail = i + 1 == items.len() && leaves_open(item);
            }
        }
    }
    let appended: Vec<String> = overrides
        .iter()
        .filter(|(k, _)| written.insert(k.to_ascii_lowercase()))
        .map(|(k, v)| format!(" {k}={}", param_value(v)))
        .collect();
    let at = out.len() - usize::from(open_tail);
    out.splice(at..at, appended);
    out.join(",")
}

/// Whether `item` — one item of a line — ends inside a quote or a
/// parenthesised group: a comma after it would not end it.
fn leaves_open(item: &str) -> bool {
    crate::redact::split_top_level(&format!("{item},")).len() == 1
}

/// `value` as a line has to spell it to read it back the same. The parser
/// unquotes a `key="value"` item twice — as a list field, then as a
/// parameter value after trimming it — so a value that the second pass
/// would change (one wrapped in quotes, or with blanks at either end) is
/// quoted twice.
fn param_value(value: &str) -> String {
    let plain = !value.is_empty()
        && value.trim() == value
        && !value.contains([',', '"', '\'', '(', ')', '\\']);
    if plain {
        return value.to_string();
    }
    let once = quoted(value);
    if unquote(value.trim()) == value {
        once
    } else {
        quoted(&once)
    }
}

fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupKind {
    Select,
    UrlTest,
    Fallback,
    LoadBalance,
    Smart,
    Subnet,
}

impl GroupKind {
    /// Returns the kind and whether the legacy `ssid` keyword was used.
    pub fn parse(keyword: &str) -> Option<(GroupKind, bool)> {
        Some(match keyword.trim().to_ascii_lowercase().as_str() {
            "select" => (GroupKind::Select, false),
            "url-test" => (GroupKind::UrlTest, false),
            "fallback" => (GroupKind::Fallback, false),
            "load-balance" => (GroupKind::LoadBalance, false),
            "smart" => (GroupKind::Smart, false),
            "subnet" => (GroupKind::Subnet, false),
            "ssid" => (GroupKind::Subnet, true),
            _ => return None,
        })
    }
    pub fn keyword(&self) -> &'static str {
        match self {
            GroupKind::Select => "select",
            GroupKind::UrlTest => "url-test",
            GroupKind::Fallback => "fallback",
            GroupKind::LoadBalance => "load-balance",
            GroupKind::Smart => "smart",
            GroupKind::Subnet => "subnet",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetType {
    Wifi,
    Wired,
    Cellular,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubnetExpr {
    Ssid(Glob),
    Bssid(Glob),
    Router(IpAddr),
    Type(NetType),
    Mccmnc(String),
    Bare(String),
}

impl SubnetExpr {
    pub fn parse(s: &str) -> Result<SubnetExpr, ParseError> {
        let s = s.trim();
        let bad = |what: &str| {
            ParseError::new(
                codes::E_INVALID_RULE_VALUE,
                format!("invalid subnet expression `{s}`: {what}"),
            )
        };
        if let Some(v) = strip_prefix_ci(s, "SSID:") {
            return Glob::new(
                v,
                GlobOptions {
                    case_insensitive: false,
                    classes: false,
                },
            )
            .map(SubnetExpr::Ssid)
            .map_err(|e| bad(&e.to_string()));
        }
        if let Some(v) = strip_prefix_ci(s, "BSSID:") {
            return Glob::new(
                v,
                GlobOptions {
                    case_insensitive: true,
                    classes: false,
                },
            )
            .map(SubnetExpr::Bssid)
            .map_err(|e| bad(&e.to_string()));
        }
        if let Some(v) = strip_prefix_ci(s, "ROUTER:") {
            return v
                .parse::<IpAddr>()
                .map(SubnetExpr::Router)
                .map_err(|_| bad("expected an IP address"));
        }
        if let Some(v) = strip_prefix_ci(s, "TYPE:") {
            return match v.to_ascii_uppercase().as_str() {
                "WIFI" => Ok(SubnetExpr::Type(NetType::Wifi)),
                "WIRED" => Ok(SubnetExpr::Type(NetType::Wired)),
                "CELLULAR" => Ok(SubnetExpr::Type(NetType::Cellular)),
                _ => Err(bad("expected WIFI, WIRED or CELLULAR")),
            };
        }
        if let Some(v) = strip_prefix_ci(s, "MCCMNC:") {
            if v.is_empty() || !v.chars().all(|c| c.is_ascii_digit()) {
                return Err(bad("expected MCC+MNC digits"));
            }
            return Ok(SubnetExpr::Mccmnc(v.to_string()));
        }
        if s.is_empty() {
            return Err(bad("empty"));
        }
        Ok(SubnetExpr::Bare(s.to_string()))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PolicyGroup {
    pub name: String,
    pub kind: GroupKind,
    pub members: Vec<String>,
    pub params: ParamMap,
    pub conditions: Vec<(SubnetExpr, String)>,
    pub legacy_keyword: bool,
    /// The text right of `name =` as written (API `lineHash`).
    pub definition: String,
    pub span: Span,
}

/// `params` is left out and `definition` redacted (M3-D7): a group's
/// `policy-path` usually carries a subscription token, and its
/// `external-policy-modifier` can set any parameter, a password included.
impl std::fmt::Debug for PolicyGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyGroup")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("members", &self.members)
            .field("conditions", &self.conditions)
            .field("legacy_keyword", &self.legacy_keyword)
            .field("span", &self.span)
            .field(
                "definition",
                &crate::redact::redact_definition(&self.definition),
            )
            .finish_non_exhaustive()
    }
}

/// Every parameter a group line can carry. On a `subnet` group these stay
/// parameters; any other `key = value` item there is a network condition.
const GROUP_PARAMS: &[&str] = &[
    "default",
    "cellular",
    "hidden",
    "icon-url",
    "category",
    "no-alert",
    "url",
    "underlying-proxy",
    "policy-path",
    "update-interval",
    "policy-regex-filter",
    "external-policy-name-prefix",
    "external-policy-modifier",
    "include-all-proxies",
    "include-other-group",
    "interval",
    "tolerance",
    "timeout",
    "evaluate-before-use",
    "persistent",
    "policy-priority",
];

pub fn parse_group(name: &str, definition: &str, span: &Span) -> Result<PolicyGroup, ParseError> {
    let fields = split_list(definition);
    let Some(type_kw) = fields.first() else {
        return Err(ParseError::new(
            codes::E_SYNTAX,
            format!("policy group `{name}`: missing type"),
        ));
    };
    let (kind, legacy_keyword) = GroupKind::parse(type_kw).ok_or_else(|| {
        ParseError::new(
            codes::E_UNKNOWN_POLICY_TYPE,
            format!("policy group `{name}`: {}", unknown_type(type_kw)),
        )
    })?;
    let mut members = Vec::new();
    let mut params = ParamMap::default();
    let mut conditions = Vec::new();
    for field in &fields[1..] {
        match parse_key_value(field) {
            Some((k, v)) => {
                if kind == GroupKind::Subnet
                    && !GROUP_PARAMS.contains(&k.to_ascii_lowercase().as_str())
                {
                    conditions.push((SubnetExpr::parse(k)?, v.to_string()));
                } else {
                    params.insert(k, v);
                }
            }
            None => members.push(field.clone()),
        }
    }
    Ok(PolicyGroup {
        name: name.to_string(),
        kind,
        members,
        params,
        conditions,
        legacy_keyword,
        definition: definition.trim().to_string(),
        span: span.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    fn span() -> Span {
        Span::new(Arc::from(Path::new("p.conf")), 1)
    }

    /// A port slot holding anything but digits is some other field that
    /// slid over when a comma was missing — a password, say (M3-D7): the
    /// value is quoted only when it is a number.
    #[test]
    fn an_invalid_port_is_quoted_only_when_it_is_a_number() {
        let e = parse_policy("T", "trojan, t.test 443, password=s3cretPw42", &span()).unwrap_err();
        assert_eq!(e.message, "policy `T`: invalid port");
        let e = parse_policy("P", "http, p.test, 99999", &span()).unwrap_err();
        assert_eq!(e.message, "policy `P`: invalid port `99999`");
    }

    #[test]
    fn manual_policy_examples() {
        let p = parse_policy(
            "ProxyHTTPS",
            "https, 1.2.3.4, 443, username, password",
            &span(),
        )
        .unwrap();
        assert_eq!(p.kind, PolicyKind::Https);
        assert_eq!(p.server, Some(HostName::parse("1.2.3.4")));
        assert_eq!(p.port, Some(443));
        assert_eq!(p.positional, ["username", "password"]);

        let p = parse_policy("ProxySS", "ss, 1.2.3.4, 8388, encrypt-method=chacha20-ietf-poly1305, password=pwd, udp-relay=true", &span()).unwrap();
        assert_eq!(p.kind, PolicyKind::Shadowsocks);
        assert_eq!(
            p.params.get("encrypt-method"),
            Some("chacha20-ietf-poly1305")
        );
        assert_eq!(p.params.bool("udp-relay"), Some(true));

        let p = parse_policy("Office WG", "wireguard, section-name=office-wg", &span()).unwrap();
        assert_eq!(p.kind, PolicyKind::WireGuard);
        assert_eq!(p.server, None);
        assert_eq!(p.params.get("section-name"), Some("office-wg"));

        let p = parse_policy("ext", "external, exec = \"/usr/bin/ssh\", args = \"1.2.3.4\", args = \"-D\", local-port = 1080, addresses = 1.2.3.4", &span()).unwrap();
        assert_eq!(p.kind, PolicyKind::External);
        assert_eq!(p.params.get_all("args"), ["1.2.3.4", "-D"]);
        assert_eq!(p.params.u16("local-port"), Some(1080));

        let p = parse_policy("Corp-VPN", "direct, interface = utun0", &span()).unwrap();
        assert!(p.kind.is_builtin_alias());
        assert_eq!(p.params.get("interface"), Some("utun0"));

        let p = parse_policy(
            "Exit",
            "snell, exit.example.com, 443, psk=pwd, version=5, underlying-proxy=Entry",
            &span(),
        )
        .unwrap();
        assert_eq!(p.server, Some(HostName::Domain("exit.example.com".into())));
        assert_eq!(p.params.get("underlying-proxy"), Some("Entry"));
    }

    #[test]
    fn policy_errors() {
        assert_eq!(
            parse_policy("X", "vless, 1.2.3.4, 443", &span())
                .unwrap_err()
                .code,
            codes::E_UNKNOWN_POLICY_TYPE
        );
        assert_eq!(
            parse_policy("X", "ss, 1.2.3.4", &span()).unwrap_err().code,
            codes::E_SYNTAX
        );
        assert_eq!(
            parse_policy("X", "ss, 1.2.3.4, notaport, password=x", &span())
                .unwrap_err()
                .code,
            codes::E_SYNTAX
        );
        assert_eq!(
            parse_policy("X", "", &span()).unwrap_err().code,
            codes::E_SYNTAX
        );
    }

    #[test]
    fn builtin_names() {
        assert_eq!(
            Builtin::parse("REJECT-TINYGIF"),
            Some(Builtin::RejectTinyGif)
        );
        assert_eq!(Builtin::parse("reject"), None);
        assert!(Builtin::RejectDrop.is_reject());
        assert!(Builtin::CellularOnly.is_ios_only());
        assert!(!Builtin::Direct.is_ios_only());
    }

    #[test]
    fn manual_group_examples() {
        let g = parse_group("Proxy", "select, ProxyA, ProxyB, DIRECT", &span()).unwrap();
        assert_eq!(g.kind, GroupKind::Select);
        assert_eq!(g.members, ["ProxyA", "ProxyB", "DIRECT"]);

        let g = parse_group(
            "Auto",
            "url-test, ProxyA, ProxyB, interval=600, tolerance=100, no-alert=true",
            &span(),
        )
        .unwrap();
        assert_eq!(g.kind, GroupKind::UrlTest);
        assert_eq!(g.params.u32("interval"), Some(600));
        assert_eq!(g.params.bool("no-alert"), Some(true));

        let g = parse_group(
            "Smart",
            "smart, ProxyA, ProxyB, policy-priority=\"Premium:0.9;Backup:1.3\"",
            &span(),
        )
        .unwrap();
        assert_eq!(
            g.params.get("policy-priority"),
            Some("Premium:0.9;Backup:1.3")
        );

        let g = parse_group("egroup", "select, policy-path=proxies.txt, policy-regex-filter=^HK, include-other-group=\"group1,group2\"", &span()).unwrap();
        assert!(g.members.is_empty());
        assert_eq!(g.params.get("include-other-group"), Some("group1,group2"));

        let g = parse_group("Subnet Group", "subnet, default = ProxyHTTP, SSID:MyHome = ProxySOCKS5, TYPE:WIFI = ProxyHTTP, BSSID:aa:bb:cc:* = A, ROUTER:192.168.1.1 = B, MCCMNC:310260 = C, OldName = D, hidden=true", &span()).unwrap();
        assert_eq!(g.kind, GroupKind::Subnet);
        assert_eq!(g.params.get("default"), Some("ProxyHTTP"));
        assert_eq!(g.params.bool("hidden"), Some(true));
        assert_eq!(g.conditions.len(), 6);
        assert!(matches!(&g.conditions[0].0, SubnetExpr::Ssid(gl) if gl.source() == "MyHome"));
        assert_eq!(g.conditions[1].0, SubnetExpr::Type(NetType::Wifi));
        assert!(matches!(&g.conditions[2].0, SubnetExpr::Bssid(_)));
        assert_eq!(
            g.conditions[3].0,
            SubnetExpr::Router("192.168.1.1".parse().unwrap())
        );
        assert_eq!(g.conditions[4].0, SubnetExpr::Mccmnc("310260".into()));
        assert_eq!(g.conditions[5].0, SubnetExpr::Bare("OldName".into()));
        assert_eq!(g.conditions[5].1, "D");

        let g = parse_group("Old", "ssid, default = A, MyWifi = B", &span()).unwrap();
        assert_eq!(g.kind, GroupKind::Subnet);
        assert!(g.legacy_keyword);
    }

    #[test]
    fn group_errors() {
        assert_eq!(
            parse_group("G", "round-robin, A, B", &span())
                .unwrap_err()
                .code,
            codes::E_UNKNOWN_POLICY_TYPE
        );
        assert_eq!(
            parse_group("G", "subnet, default = A, TYPE:SATELLITE = B", &span())
                .unwrap_err()
                .code,
            codes::E_INVALID_RULE_VALUE
        );
        assert_eq!(
            parse_group("G", "subnet, default = A, ROUTER:not-an-ip = B", &span())
                .unwrap_err()
                .code,
            codes::E_INVALID_RULE_VALUE
        );
    }

    /// `Debug` must not leak a subscription token or a modifier's password
    /// (M3-D7): `redact_definition` blanks them, `params` is left out
    /// entirely.
    #[test]
    fn debug_of_a_policy_group_redacts_its_definition() {
        let g = parse_group(
            "G",
            "select, policy-path=https://sub.test/n?token=t0k3n, \
external-policy-modifier=\"password=hunter2\"",
            &span(),
        )
        .unwrap();
        let debug = format!("{g:?}");
        assert!(
            !debug.contains("t0k3n") && !debug.contains("hunter2"),
            "{debug}"
        );
    }

    #[test]
    fn subnet_expr_non_ascii_bare_does_not_panic() {
        assert_eq!(
            SubnetExpr::parse("中文中文中文").unwrap(),
            SubnetExpr::Bare("中文中文中文".into())
        );
    }

    #[test]
    fn the_definition_text_is_kept_as_written() {
        let p = parse_policy(
            "Up",
            "  http, proxy.test, 8080, alice, s3cret, skip-cert-verify=true ",
            &span(),
        )
        .unwrap();
        assert_eq!(
            p.definition,
            "http, proxy.test, 8080, alice, s3cret, skip-cert-verify=true"
        );
        let g = parse_group("Pick", " select, Up, DIRECT, hidden=true", &span()).unwrap();
        assert_eq!(g.definition, "select, Up, DIRECT, hidden=true");
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn with_params_overrides_in_place_and_appends_the_rest() {
        assert_eq!(
            with_params(
                "trojan, t.test, 443, password=pw, TFO=false, sni=a.test, tfo=false",
                &pairs(&[("tfo", "true"), ("test-url", "http://apple.com/")])
            ),
            "trojan, t.test, 443, password=pw, tfo=true, sni=a.test, test-url=http://apple.com/"
        );
        assert_eq!(with_params("http, h.test, 80", &[]), "http, h.test, 80");
    }

    /// A value the list splitter would act on is quoted, and the line reads
    /// back with exactly the values that were set.
    #[test]
    fn a_value_set_by_with_params_reads_back_unchanged() {
        let line = with_params(
            "http, h.test, 80, alice, s3cret",
            &pairs(&[
                ("password", "a,b\"c\\d"),
                ("headers", "X-A:1|X-B:(2)"),
                ("sni", ""),
                ("single", "'abc'"),
                ("double", "\"abc\""),
                ("spaced", " padded "),
            ]),
        );
        let p = parse_policy("P", &line, &span()).unwrap();
        assert_eq!(p.params.get("password"), Some("a,b\"c\\d"));
        assert_eq!(p.params.get("headers"), Some("X-A:1|X-B:(2)"));
        assert_eq!(p.params.get("sni"), Some(""));
        assert_eq!(p.params.get("single"), Some("'abc'"));
        assert_eq!(p.params.get("double"), Some("\"abc\""));
        assert_eq!(p.params.get("spaced"), Some(" padded "));
        assert_eq!(p.positional, ["alice", "s3cret"]);
    }

    /// What the parser reads of `line` after its type, server and port: the
    /// positional values, and each parameter's values in the order of the
    /// line. Two parameters of different names have no order: no lookup
    /// tells it.
    fn read(line: &str) -> (Vec<String>, Vec<(String, String)>) {
        let p = parse_policy("P", line, &span()).unwrap();
        let mut params: Vec<(String, String)> = p
            .params
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        params.sort_by(|a, b| a.0.cmp(&b.0));
        (p.positional, params)
    }

    /// What `line` reads as once `modifier` is in place: a parameter the
    /// modifier names has the modifier's value where the line first has it
    /// and is gone where the line repeats it, the rest of the modifier comes
    /// after.
    fn read_modified(
        line: &str,
        modifier: &[(String, String)],
    ) -> (Vec<String>, Vec<(String, String)>) {
        let p = parse_policy("P", line, &span()).unwrap();
        let mut set: HashSet<&str> = HashSet::new();
        let mut params: Vec<(String, String)> = Vec::new();
        for (k, v) in p.params.iter() {
            match modifier.iter().find(|(m, _)| m == k) {
                Some((m, value)) => {
                    if set.insert(m) {
                        params.push((k.to_string(), value.clone()));
                    }
                }
                None => params.push((k.to_string(), v.to_string())),
            }
        }
        for (m, value) in modifier {
            if set.insert(m) {
                params.push((m.clone(), value.clone()));
            }
        }
        params.sort_by(|a, b| a.0.cmp(&b.0));
        (p.positional, params)
    }

    /// `with_params` and the parser agree on what a line's items are, for
    /// every line of up to three of these after each head: the parser takes
    /// the quotes off a whole item before it looks for a key, keeps a comma
    /// inside a quoted item, compares keys without their case, and gives an
    /// unclosed quote or group the rest of the line — what is appended must
    /// not end up in it. And it takes the type, and a server and a port
    /// unless the type has none, for what they are whatever they are spelled
    /// like.
    #[test]
    fn a_modified_line_reads_as_the_line_did_with_the_modifier_in_place() {
        const HEADS: [&str; 4] = [
            "trojan, t.test, 443",
            // a server spelled like a parameter the modifier sets
            "http, tfo=false, 80",
            // a type without a server: its first item is a parameter
            "direct, tfo=false",
            // the parser drops an empty item, which is then no field
            "trojan, , , tfo=false, 443",
        ];
        const ITEMS: [&str; 16] = [
            " password=pw",
            " tfo=false",
            " Tfo = false ",
            " alice",
            "",
            // a whole item in quotes is the parameter it reads as
            " \"tfo=false\"",
            " 'TFO=false'",
            // a comma inside a quoted item is the item's
            " \"sni=a.test,b.test\"",
            " ws-path=\"/a,b\"",
            // an unclosed quote, in a value, around an item, of a positional
            " password=\"p,w",
            " \"password=p,w",
            " \"s3cret",
            " sni='a.test",
            // an unclosed group
            " password=(p,w",
            " (tfo=false",
            // a backslash that takes the closing quote
            " password=\"pw\\\"",
        ];
        let modifier = pairs(&[("tfo", "true"), ("sni", "edge.test")]);
        let mut lines: Vec<String> = Vec::new();
        for head in HEADS {
            for a in ITEMS {
                lines.push(format!("{head},{a}"));
                for b in ITEMS {
                    lines.push(format!("{head},{a},{b}"));
                    for c in ITEMS {
                        lines.push(format!("{head},{a},{b},{c}"));
                    }
                }
            }
        }
        let mut wrong: Vec<String> = Vec::new();
        for line in &lines {
            // nothing to set leaves the line as it is
            assert_eq!(&with_params(line, &[]), line);
            let modified = with_params(line, &modifier);
            if read(&modified) != read_modified(line, &modifier) {
                wrong.push(format!("{line} -> {modified}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "{} of {} lines, the first of them: {:#?}",
            wrong.len(),
            lines.len(),
            &wrong[..wrong.len().min(8)]
        );
    }

    /// How such lines are written: a quoted item is replaced whole, and what
    /// is appended stands in front of a last item that would take it for
    /// its own.
    #[test]
    fn with_params_writes_around_quotes() {
        let modifier = pairs(&[("tfo", "true"), ("sni", "edge.test")]);
        for (line, modified) in [
            (
                "trojan, t.test, 443, 'tfo=false', password=pw, \"TFO=false\"",
                "trojan, t.test, 443, tfo=true, password=pw, sni=edge.test",
            ),
            (
                "trojan, t.test, 443, \"password=p,w\", \"sni=a.test,b.test\"",
                "trojan, t.test, 443, \"password=p,w\", sni=edge.test, tfo=true",
            ),
            (
                "trojan, t.test, 443, password=\"p,w",
                "trojan, t.test, 443, tfo=true, sni=edge.test, password=\"p,w",
            ),
            // the open item is the one replaced: nothing is left open
            (
                "trojan, t.test, 443, password=pw, sni='a.test",
                "trojan, t.test, 443, password=pw, sni=edge.test, tfo=true",
            ),
        ] {
            assert_eq!(with_params(line, &modifier), modified, "{line}");
        }
        // the server and the port are no parameters, however they are
        // spelled; the first item after a type without a server is one
        let tfo = pairs(&[("tfo", "true")]);
        assert_eq!(
            with_params("http, tfo=x, 80", &tfo),
            "http, tfo=x, 80, tfo=true"
        );
        assert_eq!(with_params("direct, tfo=x", &tfo), "direct, tfo=true");
    }
}
