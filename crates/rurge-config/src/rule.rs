//! `[Rule]` lines, sub-rules (logical rules, inline rule sets, set files).

use crate::diagnostic::{ParseError, codes};
use crate::glob::{Glob, GlobOptions};
use crate::policy::{Builtin, SubnetExpr};
use crate::span::Span;
use crate::value::split_list;
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use std::collections::HashSet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

pub const MAX_LOGICAL_DEPTH: usize = 10;

pub struct ParseCtx<'a> {
    pub inline_rulesets: &'a HashSet<String>,
    pub base_dir: &'a Path,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortExpr {
    Single(u16),
    Range(u16, u16),
    Gt(u16),
    Lt(u16),
    Ge(u16),
    Le(u16),
}

impl PortExpr {
    pub fn parse(s: &str) -> Option<PortExpr> {
        let s = s.trim();
        let num = |x: &str| x.trim().parse::<u16>().ok();
        if let Some(v) = s.strip_prefix(">=") {
            return num(v).map(PortExpr::Ge);
        }
        if let Some(v) = s.strip_prefix("<=") {
            return num(v).map(PortExpr::Le);
        }
        if let Some(v) = s.strip_prefix('>') {
            return num(v).map(PortExpr::Gt);
        }
        if let Some(v) = s.strip_prefix('<') {
            return num(v).map(PortExpr::Lt);
        }
        if let Some((a, b)) = s.split_once('-') {
            let (a, b) = (num(a)?, num(b)?);
            return (a <= b).then_some(PortExpr::Range(a, b));
        }
        num(s).map(PortExpr::Single)
    }
    pub fn matches(&self, port: u16) -> bool {
        match *self {
            PortExpr::Single(p) => port == p,
            PortExpr::Range(a, b) => (a..=b).contains(&port),
            PortExpr::Gt(p) => port > p,
            PortExpr::Lt(p) => port < p,
            PortExpr::Ge(p) => port >= p,
            PortExpr::Le(p) => port <= p,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InternalSet {
    System,
    Lan,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResourceRef {
    Internal(InternalSet),
    Inline(String),
    File(PathBuf),
    Url(String),
}

impl ResourceRef {
    pub fn parse(raw: &str, ctx: &ParseCtx) -> ResourceRef {
        let raw = raw.trim();
        match raw {
            "SYSTEM" => return ResourceRef::Internal(InternalSet::System),
            "LAN" => return ResourceRef::Internal(InternalSet::Lan),
            _ => {}
        }
        if let Some(name) = ctx
            .inline_rulesets
            .iter()
            .find(|n| n.as_str() == raw || n.eq_ignore_ascii_case(raw))
        {
            return ResourceRef::Inline(name.clone());
        }
        let lower = raw.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return ResourceRef::Url(raw.to_string());
        }
        let path = Path::new(raw);
        ResourceRef::File(if path.is_absolute() {
            path.to_path_buf()
        } else {
            ctx.base_dir.join(path)
        })
    }
}

/// A compiled regular expression that compares by source text.
#[derive(Clone)]
pub struct Pattern {
    pub source: String,
    pub regex: fancy_regex::Regex,
}

impl Pattern {
    pub fn new(source: &str) -> Result<Pattern, String> {
        fancy_regex::Regex::new(source)
            .map(|regex| Pattern {
                source: source.to_string(),
                regex,
            })
            .map_err(|e| e.to_string())
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}
impl Eq for Pattern {}
impl fmt::Debug for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pattern({:?})", self.source)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostnameType {
    IPv4,
    IPv6,
    Domain,
    Simple,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolKind {
    Http,
    Https,
    Tcp,
    Udp,
    Quic,
    Stun,
    MtProto,
    Doh,
    Doh3,
    Doq,
    Dot,
    Dns,
}

impl ProtocolKind {
    /// Keywords are case-sensitive, exactly as the manual lists them.
    pub fn parse(s: &str) -> Option<ProtocolKind> {
        Some(match s {
            "HTTP" => ProtocolKind::Http,
            "HTTPS" => ProtocolKind::Https,
            "TCP" => ProtocolKind::Tcp,
            "UDP" => ProtocolKind::Udp,
            "QUIC" => ProtocolKind::Quic,
            "STUN" => ProtocolKind::Stun,
            "MTProto" => ProtocolKind::MtProto,
            "DOH" => ProtocolKind::Doh,
            "DOH3" => ProtocolKind::Doh3,
            "DOQ" => ProtocolKind::Doq,
            "DOT" => ProtocolKind::Dot,
            "DNS" => ProtocolKind::Dns,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessPattern {
    Name(Glob),
    Path(Glob),
    Prefix(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleKind {
    Domain(String),
    DomainSuffix(String),
    DomainKeyword(String),
    DomainWildcard(Glob),
    DomainSet(ResourceRef),
    IpCidr(Ipv4Net),
    IpCidr6(Ipv6Net),
    GeoIp(String),
    IpAsn(u32),
    UserAgent(Glob),
    UrlRegex(Pattern),
    ProcessName(ProcessPattern),
    DestPort(PortExpr),
    SrcPort(PortExpr),
    InPort(PortExpr),
    SrcIp(IpNet),
    DeviceName(Glob),
    MacAddress(String),
    Protocol(ProtocolKind),
    HostnameType(HostnameType),
    Subnet(SubnetExpr),
    CellularRadio(String),
    CellularCarrier(String),
    And(Vec<SubRule>),
    Or(Vec<SubRule>),
    Not(Box<SubRule>),
    Script(String),
    RuleSet(ResourceRef),
    Final,
}

impl RuleKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            RuleKind::Domain(_) => "DOMAIN",
            RuleKind::DomainSuffix(_) => "DOMAIN-SUFFIX",
            RuleKind::DomainKeyword(_) => "DOMAIN-KEYWORD",
            RuleKind::DomainWildcard(_) => "DOMAIN-WILDCARD",
            RuleKind::DomainSet(_) => "DOMAIN-SET",
            RuleKind::IpCidr(_) => "IP-CIDR",
            RuleKind::IpCidr6(_) => "IP-CIDR6",
            RuleKind::GeoIp(_) => "GEOIP",
            RuleKind::IpAsn(_) => "IP-ASN",
            RuleKind::UserAgent(_) => "USER-AGENT",
            RuleKind::UrlRegex(_) => "URL-REGEX",
            RuleKind::ProcessName(_) => "PROCESS-NAME",
            RuleKind::DestPort(_) => "DEST-PORT",
            RuleKind::SrcPort(_) => "SRC-PORT",
            RuleKind::InPort(_) => "IN-PORT",
            RuleKind::SrcIp(_) => "SRC-IP",
            RuleKind::DeviceName(_) => "DEVICE-NAME",
            RuleKind::MacAddress(_) => "MAC-ADDRESS",
            RuleKind::Protocol(_) => "PROTOCOL",
            RuleKind::HostnameType(_) => "HOSTNAME-TYPE",
            RuleKind::Subnet(_) => "SUBNET",
            RuleKind::CellularRadio(_) => "CELLULAR-RADIO",
            RuleKind::CellularCarrier(_) => "CELLULAR-CARRIER",
            RuleKind::And(_) => "AND",
            RuleKind::Or(_) => "OR",
            RuleKind::Not(_) => "NOT",
            RuleKind::Script(_) => "SCRIPT",
            RuleKind::RuleSet(_) => "RULE-SET",
            RuleKind::Final => "FINAL",
        }
    }
    pub fn is_ip_based(&self) -> bool {
        matches!(
            self,
            RuleKind::IpCidr(_) | RuleKind::IpCidr6(_) | RuleKind::GeoIp(_) | RuleKind::IpAsn(_)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubRule {
    pub kind: RuleKind,
    pub no_resolve: bool,
    pub extended_matching: bool,
    pub raw: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuleParams {
    pub no_resolve: bool,
    pub dns_failed: bool,
    pub extended_matching: bool,
    pub pre_matching: bool,
    pub requires_resolve: bool,
    pub notification_text: Option<String>,
    pub notification_interval: Option<u32>,
    pub update_interval: Option<i64>,
    pub always_capture: Option<String>,
    pub unknown: Vec<String>,
}

impl RuleParams {
    fn parse(fields: &[String]) -> RuleParams {
        let mut p = RuleParams::default();
        for f in fields {
            let (k, v) = match f.split_once('=') {
                Some((k, v)) => (k.trim(), Some(v.trim())),
                None => (f.trim(), None),
            };
            match (k.to_ascii_lowercase().as_str(), v) {
                ("no-resolve", None) => p.no_resolve = true,
                ("dns-failed", None) => p.dns_failed = true,
                ("extended-matching", None) => p.extended_matching = true,
                ("pre-matching", None) => p.pre_matching = true,
                ("requires-resolve", None) => p.requires_resolve = true,
                ("notification-text", Some(v)) => p.notification_text = Some(v.to_string()),
                ("notification-interval", Some(v)) if v.parse::<u32>().is_ok() => {
                    p.notification_interval = v.parse().ok()
                }
                ("update-interval", Some(v)) if v.parse::<i64>().is_ok() => {
                    p.update_interval = v.parse().ok()
                }
                ("always-capture", Some(v)) => p.always_capture = Some(v.to_string()),
                _ => p.unknown.push(f.clone()),
            }
        }
        p
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyRef {
    Builtin(Builtin),
    Named(String),
    Device(String),
}

impl PolicyRef {
    pub fn parse(s: &str) -> PolicyRef {
        let s = s.trim();
        if let Some(b) = Builtin::parse(s) {
            return PolicyRef::Builtin(b);
        }
        if let Some(d) = s.strip_prefix("DEVICE:") {
            return PolicyRef::Device(d.trim().to_string());
        }
        PolicyRef::Named(s.to_string())
    }
    pub fn name(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for PolicyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyRef::Builtin(b) => f.write_str(b.name()),
            PolicyRef::Named(n) => f.write_str(n),
            PolicyRef::Device(d) => write!(f, "DEVICE:{d}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub kind: RuleKind,
    pub policy: PolicyRef,
    pub params: RuleParams,
    pub span: Span,
    pub raw: String,
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

fn invalid(ty: &str, value: &str, why: &str) -> ParseError {
    ParseError::new(
        codes::E_INVALID_RULE_VALUE,
        format!("{ty}: invalid value `{value}`: {why}"),
    )
}

fn glob(value: &str, ci: bool, classes: bool) -> Result<Glob, String> {
    Glob::new(
        value,
        GlobOptions {
            case_insensitive: ci,
            classes,
        },
    )
    .map_err(|e| e.to_string())
}

fn parse_mac(value: &str) -> Option<String> {
    let parts: Vec<&str> = value.split([':', '-']).collect();
    if parts.len() != 6
        || !parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return None;
    }
    Some(
        parts
            .iter()
            .map(|p| p.to_ascii_uppercase())
            .collect::<Vec<_>>()
            .join(":"),
    )
}

fn parse_kind(ty: &str, value: &str, ctx: &ParseCtx, depth: usize) -> Result<RuleKind, ParseError> {
    let kind = match ty {
        "DOMAIN" => RuleKind::Domain(value.trim_end_matches('.').to_ascii_lowercase()),
        "DOMAIN-SUFFIX" => RuleKind::DomainSuffix(value.trim_matches('.').to_ascii_lowercase()),
        "DOMAIN-KEYWORD" => RuleKind::DomainKeyword(value.to_ascii_lowercase()),
        "DOMAIN-WILDCARD" => {
            RuleKind::DomainWildcard(glob(value, true, true).map_err(|e| invalid(ty, value, &e))?)
        }
        "DOMAIN-SET" => RuleKind::DomainSet(ResourceRef::parse(value, ctx)),
        "IP-CIDR" => RuleKind::IpCidr(
            value
                .parse::<Ipv4Net>()
                .or_else(|_| {
                    value
                        .parse::<Ipv4Addr>()
                        .map(|ip| Ipv4Net::new(ip, 32).unwrap())
                })
                .map_err(|_| invalid(ty, value, "expected IPv4 CIDR"))?,
        ),
        "IP-CIDR6" => RuleKind::IpCidr6(
            value
                .parse::<Ipv6Net>()
                .or_else(|_| {
                    value
                        .parse::<Ipv6Addr>()
                        .map(|ip| Ipv6Net::new(ip, 128).unwrap())
                })
                .map_err(|_| invalid(ty, value, "expected IPv6 CIDR"))?,
        ),
        "GEOIP" => {
            if value.is_empty() || !value.chars().all(|c| c.is_ascii_alphabetic()) {
                return Err(invalid(ty, value, "expected an ISO country code"));
            }
            RuleKind::GeoIp(value.to_ascii_uppercase())
        }
        "IP-ASN" => {
            let digits = value
                .strip_prefix("AS")
                .or_else(|| value.strip_prefix("as"))
                .unwrap_or(value);
            RuleKind::IpAsn(
                digits
                    .parse()
                    .map_err(|_| invalid(ty, value, "expected a decimal ASN"))?,
            )
        }
        "USER-AGENT" => {
            RuleKind::UserAgent(glob(value, false, false).map_err(|e| invalid(ty, value, &e))?)
        }
        "URL-REGEX" => RuleKind::UrlRegex(Pattern::new(value).map_err(|e| invalid(ty, value, &e))?),
        "PROCESS-NAME" => RuleKind::ProcessName(
            if value.starts_with('/') && value.ends_with('/') && value.len() > 1 {
                ProcessPattern::Prefix(value.to_string())
            } else if value.starts_with('/') {
                ProcessPattern::Path(glob(value, false, false).map_err(|e| invalid(ty, value, &e))?)
            } else {
                ProcessPattern::Name(glob(value, false, false).map_err(|e| invalid(ty, value, &e))?)
            },
        ),
        "DEST-PORT" => RuleKind::DestPort(
            PortExpr::parse(value)
                .ok_or_else(|| invalid(ty, value, "expected a port expression"))?,
        ),
        "SRC-PORT" => RuleKind::SrcPort(
            PortExpr::parse(value)
                .ok_or_else(|| invalid(ty, value, "expected a port expression"))?,
        ),
        "IN-PORT" => RuleKind::InPort(
            PortExpr::parse(value)
                .ok_or_else(|| invalid(ty, value, "expected a port expression"))?,
        ),
        "SRC-IP" => RuleKind::SrcIp(
            value
                .parse::<IpNet>()
                .or_else(|_| value.parse::<IpAddr>().map(IpNet::from))
                .map_err(|_| invalid(ty, value, "expected an IP address or CIDR"))?,
        ),
        "DEVICE-NAME" => {
            RuleKind::DeviceName(glob(value, false, false).map_err(|e| invalid(ty, value, &e))?)
        }
        "MAC-ADDRESS" => RuleKind::MacAddress(
            parse_mac(value).ok_or_else(|| invalid(ty, value, "expected a MAC address"))?,
        ),
        "PROTOCOL" => RuleKind::Protocol(
            ProtocolKind::parse(value)
                .ok_or_else(|| invalid(ty, value, "unknown protocol keyword (case-sensitive)"))?,
        ),
        "HOSTNAME-TYPE" => RuleKind::HostnameType(match value {
            "IPv4" => HostnameType::IPv4,
            "IPv6" => HostnameType::IPv6,
            "DOMAIN" => HostnameType::Domain,
            "SIMPLE" => HostnameType::Simple,
            _ => {
                return Err(invalid(
                    ty,
                    value,
                    "expected IPv4, IPv6, DOMAIN or SIMPLE (case-sensitive)",
                ));
            }
        }),
        "SUBNET" => RuleKind::Subnet(SubnetExpr::parse(value)?),
        "CELLULAR-RADIO" => RuleKind::CellularRadio(value.to_string()),
        "CELLULAR-CARRIER" => RuleKind::CellularCarrier(value.to_string()),
        "AND" | "OR" | "NOT" => {
            let subs = parse_logical_value(value, ctx, depth)?;
            match ty {
                "AND" => RuleKind::And(subs),
                "OR" => RuleKind::Or(subs),
                _ => {
                    if subs.len() != 1 {
                        return Err(ParseError::new(
                            codes::E_SYNTAX,
                            "NOT takes exactly one sub-rule",
                        ));
                    }
                    RuleKind::Not(Box::new(subs.into_iter().next().unwrap()))
                }
            }
        }
        "SCRIPT" => RuleKind::Script(value.to_string()),
        "RULE-SET" => RuleKind::RuleSet(ResourceRef::parse(value, ctx)),
        _ => {
            return Err(ParseError::new(
                codes::E_UNKNOWN_RULE_TYPE,
                format!("unknown rule type `{ty}`"),
            ));
        }
    };
    Ok(kind)
}

/// `((R1),(R2),...)` -> sub-rules.
fn parse_logical_value(
    value: &str,
    ctx: &ParseCtx,
    depth: usize,
) -> Result<Vec<SubRule>, ParseError> {
    if depth > MAX_LOGICAL_DEPTH {
        return Err(ParseError::new(
            codes::E_NESTING_TOO_DEEP,
            format!("logical rules nested deeper than {MAX_LOGICAL_DEPTH}"),
        ));
    }
    let inner = value
        .trim()
        .strip_prefix('(')
        .and_then(|v| v.strip_suffix(')'))
        .ok_or_else(|| {
            ParseError::new(
                codes::E_SYNTAX,
                "logical rule value must be wrapped in parentheses",
            )
        })?;
    let mut subs = Vec::new();
    for item in split_list(inner) {
        let sub = item
            .strip_prefix('(')
            .and_then(|v| v.strip_suffix(')'))
            .ok_or_else(|| {
                ParseError::new(
                    codes::E_SYNTAX,
                    format!("sub-rule `{item}` must be wrapped in parentheses"),
                )
            })?;
        subs.push(parse_subrule_depth(sub, ctx, depth + 1)?);
    }
    if subs.is_empty() {
        return Err(ParseError::new(
            codes::E_SYNTAX,
            "logical rule needs at least one sub-rule",
        ));
    }
    Ok(subs)
}

fn parse_subrule_depth(raw: &str, ctx: &ParseCtx, depth: usize) -> Result<SubRule, ParseError> {
    let fields = split_list(raw);
    let Some(ty) = fields.first() else {
        return Err(ParseError::new(codes::E_SYNTAX, "empty sub-rule"));
    };
    let ty = ty.to_ascii_uppercase();
    if ty == "FINAL" {
        return Err(ParseError::new(
            codes::E_NOT_ALLOWED_HERE,
            "FINAL is not allowed as a sub-rule or inside a rule set",
        ));
    }
    let value = fields
        .get(1)
        .ok_or_else(|| ParseError::new(codes::E_SYNTAX, format!("{ty}: missing value")))?;
    let kind = parse_kind(&ty, value, ctx, depth)?;
    let mut sub = SubRule {
        kind,
        no_resolve: false,
        extended_matching: false,
        raw: raw.trim().to_string(),
    };
    for flag in &fields[2..] {
        match flag.to_ascii_lowercase().as_str() {
            "no-resolve" => sub.no_resolve = true,
            "extended-matching" => sub.extended_matching = true,
            "pre-matching" => {
                return Err(ParseError::new(
                    codes::E_NOT_ALLOWED_HERE,
                    "pre-matching is only allowed on top-level rules",
                ));
            }
            _ => {} // unknown flags are ignored, as Surge does
        }
    }
    Ok(sub)
}

/// Parse a rule without a policy (logical sub-rule, inline rule set line, set file line).
pub fn parse_subrule(raw: &str, ctx: &ParseCtx) -> Result<SubRule, ParseError> {
    parse_subrule_depth(raw, ctx, 1)
}

/// Parse a full `[Rule]` line: `TYPE,VALUE,POLICY[,params...]` (FINAL has no value).
pub fn parse_rule(raw: &str, ctx: &ParseCtx, span: &Span) -> Result<Rule, ParseError> {
    let fields = split_list(raw);
    let Some(ty) = fields.first() else {
        return Err(ParseError::new(codes::E_SYNTAX, "empty rule"));
    };
    let ty = ty.to_ascii_uppercase();
    let (kind, policy_idx) = if ty == "FINAL" {
        (RuleKind::Final, 1)
    } else {
        let value = fields
            .get(1)
            .ok_or_else(|| ParseError::new(codes::E_SYNTAX, format!("{ty}: missing value")))?;
        (parse_kind(&ty, value, ctx, 1)?, 2)
    };
    let policy = fields
        .get(policy_idx)
        .ok_or_else(|| ParseError::new(codes::E_SYNTAX, format!("{ty}: missing policy")))?;
    let policy = PolicyRef::parse(policy);
    let params = RuleParams::parse(&fields[policy_idx + 1..]);
    if params.pre_matching {
        let reject = matches!(&policy, PolicyRef::Builtin(b) if b.is_reject());
        if !reject {
            return Err(ParseError::new(
                codes::E_NOT_ALLOWED_HERE,
                "pre-matching requires a REJECT-family policy",
            ));
        }
        if matches!(
            kind,
            RuleKind::Protocol(_)
                | RuleKind::ProcessName(_)
                | RuleKind::Script(_)
                | RuleKind::Final
        ) {
            return Err(ParseError::new(
                codes::E_NOT_ALLOWED_HERE,
                format!("{ty} does not support pre-matching"),
            ));
        }
    }
    Ok(Rule {
        kind,
        policy,
        params,
        span: span.clone(),
        raw: raw.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::Path;
    use std::sync::Arc;

    fn ctx_and_span() -> (HashSet<String>, Span) {
        (
            HashSet::from(["Streaming".to_string()]),
            Span::new(Arc::from(Path::new("r.conf")), 1),
        )
    }

    fn parse(raw: &str) -> Result<Rule, ParseError> {
        let (inline, span) = ctx_and_span();
        let ctx = ParseCtx {
            inline_rulesets: &inline,
            base_dir: Path::new("/profiles"),
        };
        parse_rule(raw, &ctx, &span)
    }

    #[test]
    fn manual_examples_parse_with_expected_types() {
        let cases: &[(&str, &str)] = &[
            ("DOMAIN,www.apple.com,Proxy", "DOMAIN"),
            ("DOMAIN-SUFFIX,apple.com,DIRECT", "DOMAIN-SUFFIX"),
            ("DOMAIN-KEYWORD,google,Proxy", "DOMAIN-KEYWORD"),
            ("DOMAIN-WILDCARD,api-*.example.com,Proxy", "DOMAIN-WILDCARD"),
            (
                "DOMAIN-SET,https://example.com/adblock.txt,REJECT",
                "DOMAIN-SET",
            ),
            (
                "DOMAIN-SET,my-domains.txt,Proxy,update-interval=43200",
                "DOMAIN-SET",
            ),
            ("IP-CIDR,192.168.0.0/16,DIRECT,no-resolve", "IP-CIDR"),
            ("IP-CIDR,8.8.8.8,Proxy", "IP-CIDR"),
            ("IP-CIDR6,2001:db8:abcd:8000::/50,DIRECT", "IP-CIDR6"),
            ("IP-CIDR6,2404:6800::,DIRECT", "IP-CIDR6"),
            ("GEOIP,cn,DIRECT", "GEOIP"),
            ("IP-ASN,AS13335,Proxy", "IP-ASN"),
            ("USER-AGENT,Instagram*,DIRECT", "USER-AGENT"),
            (
                "URL-REGEX,\"^http://example\\.com/(a|b),?c\",Proxy",
                "URL-REGEX",
            ),
            (
                "URL-REGEX,^https://example\\.com,Proxy,extended-matching",
                "URL-REGEX",
            ),
            ("PROCESS-NAME,Google*,Proxy", "PROCESS-NAME"),
            (
                "PROCESS-NAME,/Applications/*.app/Contents/MacOS/*,Proxy",
                "PROCESS-NAME",
            ),
            (
                "PROCESS-NAME,/Applications/ChatGPT.app/,Proxy",
                "PROCESS-NAME",
            ),
            ("DEST-PORT,80-81,DIRECT", "DEST-PORT"),
            ("SRC-PORT,>=50000,DIRECT", "SRC-PORT"),
            ("IN-PORT,6152,DIRECT", "IN-PORT"),
            ("SRC-IP,192.168.20.0/24,DIRECT", "SRC-IP"),
            ("SRC-IP,192.168.20.100,DIRECT", "SRC-IP"),
            ("DEVICE-NAME,Kids-iPad,REJECT", "DEVICE-NAME"),
            ("MAC-ADDRESS,A4:83:E7:11:22:33,Proxy", "MAC-ADDRESS"),
            ("PROTOCOL,STUN,REJECT", "PROTOCOL"),
            ("PROTOCOL,MTProto,Proxy", "PROTOCOL"),
            ("HOSTNAME-TYPE,IPv6,REJECT", "HOSTNAME-TYPE"),
            ("SUBNET,SSID:Office-*,DIRECT", "SUBNET"),
            ("SUBNET,TYPE:CELLULAR,DIRECT", "SUBNET"),
            ("CELLULAR-RADIO,LTE,DIRECT", "CELLULAR-RADIO"),
            ("CELLULAR-CARRIER,310260,Proxy", "CELLULAR-CARRIER"),
            (
                "AND,((SRC-IP,192.168.1.110),(DOMAIN-SUFFIX,example.com)),DIRECT",
                "AND",
            ),
            (
                "AND,((NOT,((SRC-IP,192.168.1.110))),(DOMAIN-SUFFIX,example.com)),DIRECT",
                "AND",
            ),
            ("OR,((DOMAIN,a.com),(DOMAIN,b.com)),Proxy", "OR"),
            ("NOT,((RULE-SET,LAN)),Proxy", "NOT"),
            (
                "AND,((PROTOCOL,UDP),(RULE-SET,https://example.com/streaming.list)),REJECT",
                "AND",
            ),
            (
                "AND,((DOMAIN-SUFFIX,tracker.example.com),(DEST-PORT,443)),REJECT,pre-matching",
                "AND",
            ),
            ("SCRIPT,ssid-rule,DIRECT,requires-resolve", "SCRIPT"),
            ("RULE-SET,SYSTEM,DIRECT", "RULE-SET"),
            ("RULE-SET,LAN,DIRECT,no-resolve", "RULE-SET"),
            ("RULE-SET,Streaming,StreamingProxy", "RULE-SET"),
            (
                "RULE-SET,https://example.com/social.list,Proxy,no-resolve,extended-matching,update-interval=43200",
                "RULE-SET",
            ),
            ("RULE-SET,rules/local.list,Proxy", "RULE-SET"),
            ("DOMAIN,ad.example.com,REJECT,pre-matching", "DOMAIN"),
            (
                "DOMAIN-SUFFIX,example.com,Proxy,notification-text=Example matched,notification-interval=600",
                "DOMAIN-SUFFIX",
            ),
            ("FINAL,ProxyB,dns-failed", "FINAL"),
            ("FINAL,DIRECT", "FINAL"),
        ];
        for (raw, ty) in cases {
            let r = parse(raw).unwrap_or_else(|e| panic!("{raw}: {}", e.message));
            assert_eq!(r.kind.type_name(), *ty, "{raw}");
            assert_eq!(r.to_string(), *raw);
        }
    }

    #[test]
    fn values_and_params() {
        let r = parse("IP-CIDR,8.8.8.8,Proxy").unwrap();
        assert!(matches!(r.kind, RuleKind::IpCidr(n) if n.prefix_len() == 32));
        let r = parse("IP-ASN,AS13335,Proxy").unwrap();
        assert_eq!(r.kind, RuleKind::IpAsn(13335));
        let r = parse("GEOIP,cn,DIRECT").unwrap();
        assert_eq!(r.kind, RuleKind::GeoIp("CN".into()));
        let r = parse("DEST-PORT,10000-20000,DIRECT").unwrap();
        assert_eq!(r.kind, RuleKind::DestPort(PortExpr::Range(10000, 20000)));
        assert!(PortExpr::Ge(50000).matches(50000));
        assert!(!PortExpr::Lt(80).matches(80));
        let r = parse("RULE-SET,Streaming,P").unwrap();
        assert_eq!(
            r.kind,
            RuleKind::RuleSet(ResourceRef::Inline("Streaming".into()))
        );
        let r = parse("RULE-SET,rules/local.list,P").unwrap();
        assert_eq!(
            r.kind,
            RuleKind::RuleSet(ResourceRef::File(
                Path::new("/profiles/rules/local.list").to_path_buf()
            ))
        );
        let r = parse("RULE-SET,SYSTEM,P").unwrap();
        assert_eq!(
            r.kind,
            RuleKind::RuleSet(ResourceRef::Internal(InternalSet::System))
        );
        let r = parse("DOMAIN-SUFFIX,example.com,Proxy,notification-text=Example matched,notification-interval=600,bogus,update-interval=-1").unwrap();
        assert_eq!(
            r.params.notification_text.as_deref(),
            Some("Example matched")
        );
        assert_eq!(r.params.notification_interval, Some(600));
        assert_eq!(r.params.update_interval, Some(-1));
        assert_eq!(r.params.unknown, ["bogus"]);
        let r = parse("FINAL,ProxyB,dns-failed").unwrap();
        assert!(r.params.dns_failed);
        assert_eq!(r.policy, PolicyRef::Named("ProxyB".into()));
        let r = parse("DOMAIN,a,DEVICE:Home Mac").unwrap();
        assert_eq!(r.policy, PolicyRef::Device("Home Mac".into()));
        let r = parse("DOMAIN,a,REJECT-TINYGIF").unwrap();
        assert_eq!(r.policy, PolicyRef::Builtin(Builtin::RejectTinyGif));
        let r = parse("PROCESS-NAME,/Applications/ChatGPT.app/,Proxy").unwrap();
        assert_eq!(
            r.kind,
            RuleKind::ProcessName(ProcessPattern::Prefix("/Applications/ChatGPT.app/".into()))
        );
        let r = parse("AND,((NOT,((SRC-IP,192.168.1.110))),(DOMAIN-SUFFIX,example.com)),DIRECT")
            .unwrap();
        let RuleKind::And(subs) = &r.kind else {
            panic!()
        };
        assert_eq!(subs.len(), 2);
        assert!(
            matches!(&subs[0].kind, RuleKind::Not(inner) if matches!(inner.kind, RuleKind::SrcIp(_)))
        );
    }

    #[test]
    fn errors() {
        let code = |raw: &str| parse(raw).unwrap_err().code;
        assert_eq!(code("DOMAINZ,a,DIRECT"), codes::E_UNKNOWN_RULE_TYPE);
        assert_eq!(
            code("IP-CIDR,10.0.0.0/99,DIRECT"),
            codes::E_INVALID_RULE_VALUE
        );
        assert_eq!(
            code("URL-REGEX,\"[unclosed\",DIRECT"),
            codes::E_INVALID_RULE_VALUE
        );
        assert_eq!(code("DEST-PORT,abc,DIRECT"), codes::E_INVALID_RULE_VALUE);
        assert_eq!(code("PROTOCOL,http,DIRECT"), codes::E_INVALID_RULE_VALUE);
        assert_eq!(
            code("HOSTNAME-TYPE,ipv4,DIRECT"),
            codes::E_INVALID_RULE_VALUE
        );
        assert_eq!(code("IP-ASN,ASX,DIRECT"), codes::E_INVALID_RULE_VALUE);
        assert_eq!(
            code("MAC-ADDRESS,zz:zz,DIRECT"),
            codes::E_INVALID_RULE_VALUE
        );
        assert_eq!(code("DOMAIN,a"), codes::E_SYNTAX);
        assert_eq!(code("FINAL"), codes::E_SYNTAX);
        assert_eq!(
            code("AND,((FINAL,DIRECT),(DOMAIN,a)),DIRECT"),
            codes::E_NOT_ALLOWED_HERE
        );
        assert_eq!(
            code("AND,((DOMAIN,a,pre-matching),(DOMAIN,b)),REJECT"),
            codes::E_NOT_ALLOWED_HERE
        );
        assert_eq!(code("NOT,((DOMAIN,a),(DOMAIN,b)),DIRECT"), codes::E_SYNTAX);
        assert_eq!(
            code("DOMAIN,a,Proxy,pre-matching"),
            codes::E_NOT_ALLOWED_HERE
        );
        let deep = format!("{}DOMAIN,a{}", "NOT,((".repeat(11), "))".repeat(11));
        assert_eq!(code(&format!("{deep},DIRECT")), codes::E_NESTING_TOO_DEEP);
    }

    #[test]
    fn extra_positional_field_is_unknown_param_not_error() {
        let r = parse("IP-CIDR6,2001:db8::/50,DIRECT,no-resolve,extra").unwrap();
        assert!(r.params.no_resolve);
        assert_eq!(r.params.unknown, ["extra"]);
    }
}
