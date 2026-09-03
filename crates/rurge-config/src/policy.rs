//! `[Proxy]` policies and `[Proxy Group]` groups.

use crate::diagnostic::{ParseError, codes};
use crate::glob::{Glob, GlobOptions};
use crate::span::Span;
use crate::types::HostName;
use crate::value::{ParamMap, parse_key_value, split_list};
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyPolicy {
    pub name: String,
    pub kind: PolicyKind,
    pub server: Option<HostName>,
    pub port: Option<u16>,
    pub positional: Vec<String>,
    pub params: ParamMap,
    pub span: Span,
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
            format!("policy `{name}`: unknown type `{type_kw}`"),
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
            ParseError::new(
                codes::E_SYNTAX,
                format!("policy `{name}`: invalid port `{port_str}`"),
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
        span: span.clone(),
    })
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

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    (s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix))
        .then(|| &s[prefix.len()..])
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyGroup {
    pub name: String,
    pub kind: GroupKind,
    pub members: Vec<String>,
    pub params: ParamMap,
    pub conditions: Vec<(SubnetExpr, String)>,
    pub legacy_keyword: bool,
    pub span: Span,
}

const SUBNET_GROUP_PARAMS: &[&str] = &["default", "cellular", "hidden", "icon-url"];

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
            format!("policy group `{name}`: unknown type `{type_kw}`"),
        )
    })?;
    let mut members = Vec::new();
    let mut params = ParamMap::default();
    let mut conditions = Vec::new();
    for field in &fields[1..] {
        match parse_key_value(field) {
            Some((k, v)) => {
                if kind == GroupKind::Subnet
                    && !SUBNET_GROUP_PARAMS.contains(&k.to_ascii_lowercase().as_str())
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
}
