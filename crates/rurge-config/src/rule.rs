//! `[Rule]` lines, sub-rules (logical rules, inline rule sets, set files).

use crate::diagnostic::{ParseError, codes};
use crate::glob::{Glob, GlobOptions};
use crate::policy::{Builtin, SubnetExpr};
use crate::reach::Reach;
use crate::redact::url_without_credentials;
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
    /// What the text of the line may name on this machine: a set that is a
    /// local file only if it may name any.
    pub reach: Reach,
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

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum ResourceRef {
    Internal(InternalSet),
    Inline(String),
    File(PathBuf),
    /// As written: it is what is fetched, a user name and a password in it
    /// included. Everything that shows it goes through `shown_url`.
    Url(String),
}

/// The URL of a set without the user name and the password it may carry
/// (`https://user:password@host/…`), as `Debug` shows it.
impl fmt::Debug for ResourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourceRef::Internal(set) => f.debug_tuple("Internal").field(set).finish(),
            ResourceRef::Inline(name) => f.debug_tuple("Inline").field(name).finish(),
            ResourceRef::File(path) => f.debug_tuple("File").field(path).finish(),
            ResourceRef::Url(url) => f
                .debug_tuple("Url")
                .field(&ResourceRef::shown_url(url))
                .finish(),
        }
    }
}

impl ResourceRef {
    /// `url` as a view shows the URL of a set — a `Debug`, a rule's text, a
    /// log line, a diagnostic —: the user name and the password in its
    /// authority are `***`. The rest stays as written, for a set is named by
    /// its URL, and an `@` further on is no credential
    /// (`cdn.example/gh/user/repo@main/list.txt`).
    pub fn shown_url(url: &str) -> String {
        let Some(start) = url.find("://").map(|at| at + "://".len()) else {
            return url.to_string();
        };
        let end = url[start..]
            .find(['/', '?', '#'])
            .map_or(url.len(), |at| start + at);
        let (authority, rest) = url.split_at(end);
        format!("{}{rest}", url_without_credentials(authority))
    }

    /// A `RULE-SET` value. An error (`W0033`) when it is a local file and the
    /// text it is written in may not name one.
    pub fn parse(raw: &str, ctx: &ParseCtx) -> Result<ResourceRef, ParseError> {
        let raw = raw.trim();
        match raw {
            "SYSTEM" => return Ok(ResourceRef::Internal(InternalSet::System)),
            "LAN" => return Ok(ResourceRef::Internal(InternalSet::Lan)),
            _ => {}
        }
        if ctx.inline_rulesets.contains(raw) {
            return Ok(ResourceRef::Inline(raw.to_string()));
        }
        let mut ci_matches: Vec<&String> = ctx
            .inline_rulesets
            .iter()
            .filter(|n| n.eq_ignore_ascii_case(raw))
            .collect();
        ci_matches.sort();
        if let Some(name) = ci_matches.into_iter().next() {
            return Ok(ResourceRef::Inline(name.clone()));
        }
        Self::parse_external(raw, ctx)
    }

    /// A `DOMAIN-SET` value: only URLs and files, never an inline `[Ruleset]`
    /// (the manual lets only `RULE-SET` reference inline sets) and never
    /// `SYSTEM` / `LAN`. An error (`W0033`) when it is a local file and the
    /// text it is written in may not name one: the path is never turned into
    /// a file reference, so nothing downstream can open it.
    pub fn parse_external(raw: &str, ctx: &ParseCtx) -> Result<ResourceRef, ParseError> {
        let raw = raw.trim();
        let lower = raw.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return Ok(ResourceRef::Url(raw.to_string()));
        }
        if let Some(refusal) = ctx.reach.set_refusal() {
            return Err(
                ParseError::new(codes::W_REMOTE_LOCAL_ACCESS, refusal.message)
                    .with_hint(refusal.hint),
            );
        }
        let path = Path::new(raw);
        Ok(ResourceRef::File(if path.is_absolute() {
            path.to_path_buf()
        } else {
            ctx.base_dir.join(path)
        }))
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
    /// `GEOIP,UNKNOWN`: an address the country database has no country for.
    GeoIpUnknown,
    IpAsn(u32),
    /// `IP-ASN,UNKNOWN`: an address the ASN database has no number for.
    IpAsnUnknown,
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
            RuleKind::GeoIp(_) | RuleKind::GeoIpUnknown => "GEOIP",
            RuleKind::IpAsn(_) | RuleKind::IpAsnUnknown => "IP-ASN",
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
            RuleKind::IpCidr(_)
                | RuleKind::IpCidr6(_)
                | RuleKind::GeoIp(_)
                | RuleKind::GeoIpUnknown
                | RuleKind::IpAsn(_)
                | RuleKind::IpAsnUnknown
        )
    }
    /// Whether this rule kind may carry `pre-matching`, per the Surge manual's
    /// rules/overview and rules/logical allow-list. Logical rules recurse into
    /// their sub-rules: `AND`/`OR` require every sub-rule to support it, `NOT`
    /// requires its single sub-rule to support it.
    pub fn supports_pre_matching(&self) -> bool {
        match self {
            RuleKind::Domain(_)
            | RuleKind::DomainSuffix(_)
            | RuleKind::DomainKeyword(_)
            | RuleKind::DomainWildcard(_)
            | RuleKind::DomainSet(_)
            | RuleKind::IpCidr(_)
            | RuleKind::IpCidr6(_)
            | RuleKind::GeoIp(_)
            | RuleKind::GeoIpUnknown
            | RuleKind::IpAsn(_)
            | RuleKind::IpAsnUnknown
            | RuleKind::SrcIp(_)
            | RuleKind::DestPort(_)
            | RuleKind::SrcPort(_)
            | RuleKind::Subnet(_)
            | RuleKind::CellularRadio(_)
            | RuleKind::CellularCarrier(_)
            | RuleKind::RuleSet(_) => true,
            RuleKind::And(subs) | RuleKind::Or(subs) => {
                subs.iter().all(|s| s.kind.supports_pre_matching())
            }
            RuleKind::Not(sub) => sub.kind.supports_pre_matching(),
            RuleKind::UserAgent(_)
            | RuleKind::UrlRegex(_)
            | RuleKind::ProcessName(_)
            | RuleKind::InPort(_)
            | RuleKind::DeviceName(_)
            | RuleKind::MacAddress(_)
            | RuleKind::Protocol(_)
            | RuleKind::HostnameType(_)
            | RuleKind::Script(_)
            | RuleKind::Final => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubRule {
    pub kind: RuleKind,
    pub no_resolve: bool,
    pub extended_matching: bool,
    pub unknown: Vec<String>,
    /// The line as the views show it (see `Rule::raw`).
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
    /// The line as the views show it: its `Display`, the request record, the
    /// API's rule list, `rule match --explain`. A set's URL in it has `***`
    /// for the user name and the password it carries; nothing reads the line
    /// again from here, `kind` holds the URL as written.
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

/// An `E0014`. The code has no default hint — each place wants a different
/// fix —, so every place that reports it says its own through here.
fn not_allowed_here(message: impl Into<String>, hint: &'static str) -> ParseError {
    ParseError::new(codes::E_NOT_ALLOWED_HERE, message).with_hint(hint)
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

/// Whether the value of a `GEOIP` or an `IP-ASN` rule is `UNKNOWN`, in any
/// case: the address the database has no answer for. It is checked before the
/// value is read as a country code or an AS number, so `ASUNKNOWN` is still no
/// number.
fn is_unknown(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("UNKNOWN")
}

fn parse_kind(ty: &str, value: &str, ctx: &ParseCtx, depth: usize) -> Result<RuleKind, ParseError> {
    let kind = match ty {
        "DOMAIN" => RuleKind::Domain(value.trim_end_matches('.').to_ascii_lowercase()),
        "DOMAIN-SUFFIX" => RuleKind::DomainSuffix(value.trim_matches('.').to_ascii_lowercase()),
        "DOMAIN-KEYWORD" => RuleKind::DomainKeyword(value.to_ascii_lowercase()),
        "DOMAIN-WILDCARD" => {
            RuleKind::DomainWildcard(glob(value, true, true).map_err(|e| invalid(ty, value, &e))?)
        }
        "DOMAIN-SET" => RuleKind::DomainSet(ResourceRef::parse_external(value, ctx)?),
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
        "GEOIP" if is_unknown(value) => RuleKind::GeoIpUnknown,
        "GEOIP" => {
            if value.is_empty() || !value.chars().all(|c| c.is_ascii_alphabetic()) {
                return Err(invalid(ty, value, "expected an ISO country code"));
            }
            RuleKind::GeoIp(value.to_ascii_uppercase())
        }
        "IP-ASN" if is_unknown(value) => RuleKind::IpAsnUnknown,
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
        "RULE-SET" => RuleKind::RuleSet(ResourceRef::parse(value, ctx)?),
        _ => {
            let told = quotable_type(ty).map_or(String::new(), |ty| format!(" `{ty}`"));
            return Err(ParseError::new(
                codes::E_UNKNOWN_RULE_TYPE,
                format!("unknown rule type{told}"),
            ));
        }
    };
    Ok(kind)
}

/// `ty` — the first field of a line, upper-cased — when a diagnostic may quote
/// it: when it is shaped like a rule type, ASCII letters, digits and `-`, at
/// most 32 of them. A line without a comma is all "type": a share link pasted
/// at the end of the profile, a `key = value` left in `[Rule]` by a section
/// header the parser does not take for one.
fn quotable_type(ty: &str) -> Option<&str> {
    let shaped = !ty.is_empty()
        && ty.len() <= 32
        && ty.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    shaped.then_some(ty)
}

/// What a diagnostic says of a line whose type is `ty`: `<ty>: <what>`, or
/// `<what>` when the type is not one to quote.
fn of_type(ty: &str, what: &str) -> String {
    match quotable_type(ty) {
        Some(ty) => format!("{ty}: {what}"),
        None => what.to_string(),
    }
}

/// The URLs of the sets that `kind` takes, those of its sub-rules included.
fn set_urls<'a>(kind: &'a RuleKind, urls: &mut Vec<&'a str>) {
    match kind {
        RuleKind::DomainSet(ResourceRef::Url(url)) | RuleKind::RuleSet(ResourceRef::Url(url)) => {
            urls.push(url)
        }
        RuleKind::And(subs) | RuleKind::Or(subs) => {
            subs.iter().for_each(|sub| set_urls(&sub.kind, urls))
        }
        RuleKind::Not(sub) => set_urls(&sub.kind, urls),
        _ => {}
    }
}

/// The text of a line (`raw`) that parsed to `kind`, as the views show it: the
/// URL of a set, wherever it stands in the line, without the user name and the
/// password it may carry. Nothing else of the line is touched.
fn shown_line(raw: &str, kind: &RuleKind) -> String {
    let mut urls = Vec::new();
    set_urls(kind, &mut urls);
    let mut shown = raw.to_string();
    for url in urls {
        let hidden = ResourceRef::shown_url(url);
        if hidden != url {
            shown = shown.replace(url, &hidden);
        }
    }
    shown
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
        return Err(not_allowed_here(
            "FINAL is not allowed as a sub-rule or inside a rule set",
            "use `FINAL` only as a top-level rule in `[Rule]`",
        ));
    }
    let value = fields
        .get(1)
        .ok_or_else(|| ParseError::new(codes::E_SYNTAX, of_type(&ty, "missing value")))?;
    let kind = parse_kind(&ty, value, ctx, depth)?;
    let mut sub = SubRule {
        raw: shown_line(raw.trim(), &kind),
        kind,
        no_resolve: false,
        extended_matching: false,
        unknown: Vec::new(),
    };
    for flag in &fields[2..] {
        match flag.to_ascii_lowercase().as_str() {
            "no-resolve" => sub.no_resolve = true,
            "extended-matching" => sub.extended_matching = true,
            "pre-matching" => {
                return Err(not_allowed_here(
                    "pre-matching is only allowed on top-level rules",
                    "put `pre-matching` on the top-level rule instead of the sub-rule",
                ));
            }
            _ => sub.unknown.push(flag.clone()),
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
            .ok_or_else(|| ParseError::new(codes::E_SYNTAX, of_type(&ty, "missing value")))?;
        (parse_kind(&ty, value, ctx, 1)?, 2)
    };
    let policy = fields
        .get(policy_idx)
        .ok_or_else(|| ParseError::new(codes::E_SYNTAX, of_type(&ty, "missing policy")))?;
    let policy = PolicyRef::parse(policy);
    let params = RuleParams::parse(&fields[policy_idx + 1..]);
    if params.pre_matching {
        let reject = matches!(&policy, PolicyRef::Builtin(b) if b.is_reject());
        if !reject {
            return Err(not_allowed_here(
                "pre-matching requires a REJECT-family policy",
                "give the rule a REJECT policy, or remove `pre-matching`",
            ));
        }
        if !kind.supports_pre_matching() {
            return Err(not_allowed_here(
                format!("{ty} does not support pre-matching"),
                "remove `pre-matching`: the Surge manual allows it only on domain, IP, port, SUBNET, CELLULAR and set rules, and on logical rules made of them",
            ));
        }
    }
    Ok(Rule {
        raw: shown_line(raw.trim(), &kind),
        kind,
        policy,
        params,
        span: span.clone(),
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
            reach: Reach::Anywhere,
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

    /// `UNKNOWN`, in any case, is a value of `GEOIP` and of `IP-ASN` (manual,
    /// IP Rules): the address the database has no answer for. It is read as
    /// that before the value is read as a country code or a number, so
    /// `ASUNKNOWN` is still no number.
    #[test]
    fn unknown_is_a_value_of_geoip_and_ip_asn() {
        for raw in [
            "GEOIP,UNKNOWN,P",
            "GEOIP,unknown,P",
            "GEOIP,Unknown,P,no-resolve",
        ] {
            let rule = parse(raw).unwrap_or_else(|e| panic!("{raw}: {}", e.message));
            assert_eq!(rule.kind, RuleKind::GeoIpUnknown, "{raw}");
            assert_eq!(rule.to_string(), raw);
        }
        for raw in ["IP-ASN,UNKNOWN,P", "IP-ASN,unknown,P"] {
            let rule = parse(raw).unwrap_or_else(|e| panic!("{raw}: {}", e.message));
            assert_eq!(rule.kind, RuleKind::IpAsnUnknown, "{raw}");
            assert_eq!(rule.to_string(), raw);
        }
        assert_eq!(
            parse("IP-ASN,ASUNKNOWN,P").unwrap_err().code,
            codes::E_INVALID_RULE_VALUE
        );
        // a word of three letters that is not that one loads as it did, as a
        // code no address has
        assert_eq!(
            parse("GEOIP,USA,P").unwrap().kind,
            RuleKind::GeoIp("USA".into())
        );
        // the rules are of the types they sit beside
        for (unknown, plain) in [
            ("GEOIP,UNKNOWN,P", "GEOIP,CN,P"),
            ("IP-ASN,UNKNOWN,P", "IP-ASN,1,P"),
        ] {
            let unknown = parse(unknown).unwrap().kind;
            let plain = parse(plain).unwrap().kind;
            assert_eq!(unknown.type_name(), plain.type_name());
            assert!(unknown.is_ip_based());
            assert!(plain.is_ip_based());
            assert!(unknown.supports_pre_matching());
            assert!(plain.supports_pre_matching());
        }
        assert!(parse("GEOIP,UNKNOWN,REJECT,pre-matching").is_ok());
        assert!(parse("IP-ASN,UNKNOWN,REJECT-DROP,pre-matching").is_ok());
        // and may stand in a logical rule
        let rule = parse("AND,((GEOIP,UNKNOWN),(IP-ASN,UNKNOWN),(DEST-PORT,443)),P").unwrap();
        let RuleKind::And(subs) = &rule.kind else {
            panic!("{rule:?}")
        };
        assert_eq!(subs[0].kind, RuleKind::GeoIpUnknown);
        assert_eq!(subs[1].kind, RuleKind::IpAsnUnknown);
    }

    /// The three port rules share one grammar (manual, Source and Port
    /// Rules): a number, a closed range and the four comparisons. Each form
    /// decides both ways at the edge of what it says.
    #[test]
    fn every_port_expression_the_manual_lists_decides_both_ways() {
        // (written, ports it matches, ports it does not)
        let cases: &[(&str, &[u16], &[u16])] = &[
            ("6152", &[6152], &[6151, 6153]),
            ("10000-20000", &[10000, 15000, 20000], &[9999, 20001]),
            (">=50000", &[50000, 50001, 65535], &[49999]),
            ("<=1024", &[0, 1024], &[1025, 65535]),
            (">1023", &[1024, 65535], &[0, 1023]),
            ("<1024", &[0, 1023], &[1024, 1025]),
        ];
        for (written, inside, outside) in cases {
            for ty in ["DEST-PORT", "SRC-PORT", "IN-PORT"] {
                let line = format!("{ty},{written},DIRECT");
                let rule = parse(&line).unwrap_or_else(|e| panic!("{line}: {}", e.message));
                let expr = match rule.kind {
                    RuleKind::DestPort(e) | RuleKind::SrcPort(e) | RuleKind::InPort(e) => e,
                    other => panic!("{line}: {other:?}"),
                };
                for port in *inside {
                    assert!(expr.matches(*port), "{line} matches {port}");
                }
                for port in *outside {
                    assert!(!expr.matches(*port), "{line} does not match {port}");
                }
            }
        }
        // what is no port expression is a rule with an invalid value
        for bad in ["20000-10000", ">=", "<", "abc", "65536", "1-2-3", ""] {
            let line = format!("DEST-PORT,{bad},DIRECT");
            let code = parse(&line).err().map(|e| e.code);
            assert_eq!(code, Some(codes::E_INVALID_RULE_VALUE), "{line}");
        }
    }

    /// The nine parameters the manual lists for rule lines, each read into the
    /// field of its own, whatever the case of its name; a name nobody knows is
    /// kept apart, in the order written.
    #[test]
    fn every_rule_parameter_the_manual_lists_is_read_into_its_own_field() {
        let params = |extra: &str| {
            parse(&format!("DOMAIN,a.example,DIRECT,{extra}"))
                .unwrap_or_else(|e| panic!("{extra}: {}", e.message))
                .params
        };
        let only = |change: fn(&mut RuleParams)| {
            let mut p = RuleParams::default();
            change(&mut p);
            p
        };
        assert_eq!(params("no-resolve"), only(|p| p.no_resolve = true));
        assert_eq!(params("No-Resolve"), only(|p| p.no_resolve = true));
        assert_eq!(params("dns-failed"), only(|p| p.dns_failed = true));
        assert_eq!(
            params("extended-matching"),
            only(|p| p.extended_matching = true)
        );
        assert_eq!(
            parse("DOMAIN,a.example,REJECT,pre-matching")
                .unwrap()
                .params,
            only(|p| p.pre_matching = true)
        );
        assert_eq!(
            params("requires-resolve"),
            only(|p| p.requires_resolve = true)
        );
        assert_eq!(
            params("notification-text=Example matched"),
            only(|p| p.notification_text = Some("Example matched".into()))
        );
        assert_eq!(
            params("notification-interval=600"),
            only(|p| p.notification_interval = Some(600))
        );
        assert_eq!(
            params("update-interval=43200"),
            only(|p| p.update_interval = Some(43200))
        );
        assert_eq!(
            params("update-interval=-1"),
            only(|p| p.update_interval = Some(-1))
        );
        assert_eq!(
            params("always-capture=debug-1"),
            only(|p| p.always_capture = Some("debug-1".into()))
        );
        let mixed = params("bogus,no-resolve,other=1");
        assert!(mixed.no_resolve);
        assert_eq!(mixed.unknown, ["bogus", "other=1"]);
    }

    /// Logical rules nest up to ten deep (manual, Logical Rules): ten load,
    /// the eleventh does not.
    #[test]
    fn ten_logical_rules_inside_one_another_load_and_eleven_do_not() {
        let nested = |levels: usize| {
            format!(
                "{}DOMAIN,a{},DIRECT",
                "NOT,((".repeat(levels),
                "))".repeat(levels)
            )
        };
        assert!(parse(&nested(10)).is_ok());
        assert_eq!(
            parse(&nested(11)).err().map(|e| e.code),
            Some(codes::E_NESTING_TOO_DEEP)
        );
    }

    /// The twelve `PROTOCOL` keywords (manual, Protocol and Network Rules)
    /// each mean their own kind, and they are case-sensitive: neither a
    /// lower-case spelling nor a neighbour's name reads as another keyword.
    #[test]
    fn every_protocol_keyword_the_manual_lists_means_its_own_kind() {
        let listed = [
            ("HTTP", ProtocolKind::Http),
            ("HTTPS", ProtocolKind::Https),
            ("TCP", ProtocolKind::Tcp),
            ("UDP", ProtocolKind::Udp),
            ("QUIC", ProtocolKind::Quic),
            ("STUN", ProtocolKind::Stun),
            ("MTProto", ProtocolKind::MtProto),
            ("DOH", ProtocolKind::Doh),
            ("DOH3", ProtocolKind::Doh3),
            ("DOQ", ProtocolKind::Doq),
            ("DOT", ProtocolKind::Dot),
            ("DNS", ProtocolKind::Dns),
        ];
        for (keyword, kind) in listed {
            let rule = parse(&format!("PROTOCOL,{keyword},DIRECT")).unwrap();
            assert_eq!(rule.kind, RuleKind::Protocol(kind), "{keyword}");
            assert_eq!(ProtocolKind::parse(keyword), Some(kind), "{keyword}");
        }
        for spelled in [
            "http", "Https", "mtproto", "MTPROTO", "Doh3", "dns", "DOT3", "",
        ] {
            assert_eq!(ProtocolKind::parse(spelled), None, "{spelled:?}");
            assert_eq!(
                parse(&format!("PROTOCOL,{spelled},DIRECT"))
                    .err()
                    .map(|e| e.code),
                Some(codes::E_INVALID_RULE_VALUE),
                "{spelled:?}"
            );
        }
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

    fn parse_with_inline(raw: &str, inline: &HashSet<String>) -> Result<Rule, ParseError> {
        let span = Span::new(Arc::from(Path::new("r.conf")), 1);
        let ctx = ParseCtx {
            inline_rulesets: inline,
            base_dir: Path::new("/profiles"),
            reach: Reach::Anywhere,
        };
        parse_rule(raw, &ctx, &span)
    }

    #[test]
    fn inline_ruleset_lookup_is_deterministic() {
        let inline = HashSet::from(["Streaming".to_string(), "STREAMING".to_string()]);
        let r = parse_with_inline("RULE-SET,Streaming,P", &inline).unwrap();
        assert_eq!(
            r.kind,
            RuleKind::RuleSet(ResourceRef::Inline("Streaming".into()))
        );
        for _ in 0..20 {
            // Rebuild the set each iteration so its hash-map iteration order is
            // re-randomised; the case-insensitive fallback must still pick the
            // same (lexicographically smallest) candidate every time.
            let inline = HashSet::from(["Streaming".to_string(), "STREAMING".to_string()]);
            let r = parse_with_inline("RULE-SET,streaming,P", &inline).unwrap();
            assert_eq!(
                r.kind,
                RuleKind::RuleSet(ResourceRef::Inline("STREAMING".into()))
            );
        }
    }

    /// A `RULE-SET` value is read in the order the manual gives: an internal
    /// set, then an inline section, then an external resource. A name two of
    /// them could take goes to the first.
    #[test]
    fn a_rule_set_value_is_an_internal_set_then_an_inline_one_then_an_external_resource() {
        let inline = HashSet::from(["LAN".to_string(), "lists/streaming.list".to_string()]);
        let read = |value: &str| {
            parse_with_inline(&format!("RULE-SET,{value},DIRECT"), &inline)
                .unwrap_or_else(|e| panic!("{value}: {}", e.message))
                .kind
        };
        // a section named like an internal set is the internal set
        assert_eq!(
            read("LAN"),
            RuleKind::RuleSet(ResourceRef::Internal(InternalSet::Lan))
        );
        assert_eq!(
            read("SYSTEM"),
            RuleKind::RuleSet(ResourceRef::Internal(InternalSet::System))
        );
        // a section named like a file is the section, not the file
        assert_eq!(
            read("lists/streaming.list"),
            RuleKind::RuleSet(ResourceRef::Inline("lists/streaming.list".into()))
        );
        // what no section is called is a resource: a URL, or a path from the
        // profile's directory
        assert_eq!(
            read("https://example.com/a.list"),
            RuleKind::RuleSet(ResourceRef::Url("https://example.com/a.list".into()))
        );
        assert_eq!(
            read("lists/other.list"),
            RuleKind::RuleSet(ResourceRef::File(
                Path::new("/profiles/lists/other.list").to_path_buf()
            ))
        );
    }

    /// A single address is a host route (manual, IP Rules): `IP-CIDR` reads it
    /// as `/32` and `IP-CIDR6` as `/128`.
    #[test]
    fn a_single_address_is_a_host_route() {
        let host4 = match parse("IP-CIDR,8.8.8.8,Proxy").unwrap().kind {
            RuleKind::IpCidr(net) => net,
            other => panic!("{other:?}"),
        };
        assert_eq!(host4.prefix_len(), 32);
        assert!(host4.contains(&"8.8.8.8".parse::<Ipv4Addr>().unwrap()));
        assert!(!host4.contains(&"8.8.8.9".parse::<Ipv4Addr>().unwrap()));
        let host6 = match parse("IP-CIDR6,2404:6800::,DIRECT").unwrap().kind {
            RuleKind::IpCidr6(net) => net,
            other => panic!("{other:?}"),
        };
        assert_eq!(host6.prefix_len(), 128);
        assert!(host6.contains(&"2404:6800::".parse::<Ipv6Addr>().unwrap()));
        assert!(!host6.contains(&"2404:6800::1".parse::<Ipv6Addr>().unwrap()));
    }

    #[test]
    fn subrule_unknown_flags_are_collected() {
        let r = parse("AND,((DOMAIN,a,bogus,no-resolve),(DOMAIN,b)),P").unwrap();
        let RuleKind::And(subs) = &r.kind else {
            panic!()
        };
        assert_eq!(subs[0].unknown, ["bogus"]);
        assert!(subs[0].no_resolve);
        assert!(subs[1].unknown.is_empty());
    }

    #[test]
    fn pre_matching_respects_recursive_support() {
        assert_eq!(
            parse("AND,((PROTOCOL,UDP)),REJECT,pre-matching")
                .unwrap_err()
                .code,
            codes::E_NOT_ALLOWED_HERE
        );
        assert_eq!(
            parse("USER-AGENT,x,REJECT,pre-matching").unwrap_err().code,
            codes::E_NOT_ALLOWED_HERE
        );
        assert!(parse("NOT,((DOMAIN,a)),REJECT,pre-matching").is_ok());
        assert!(parse("OR,((DOMAIN,a),(IP-CIDR,10.0.0.0/8)),REJECT-DROP,pre-matching").is_ok());
    }

    /// `E0014` is said for four different reasons, each with the fix of its own.
    #[test]
    fn each_place_a_rule_part_is_not_allowed_says_what_to_do() {
        let hint = |raw: &str| {
            let err = parse(raw).unwrap_err();
            assert_eq!(err.code, codes::E_NOT_ALLOWED_HERE, "{raw}");
            err.hint.unwrap_or_else(|| panic!("{raw} has no hint"))
        };
        let says = [
            (
                "AND,((FINAL,DIRECT),(DOMAIN,a)),DIRECT",
                "use `FINAL` only as a top-level rule in `[Rule]`",
            ),
            (
                "AND,((DOMAIN,a,pre-matching),(DOMAIN,b)),REJECT",
                "put `pre-matching` on the top-level rule instead of the sub-rule",
            ),
            (
                "DOMAIN,a,Proxy,pre-matching",
                "give the rule a REJECT policy, or remove `pre-matching`",
            ),
            (
                "USER-AGENT,x,REJECT,pre-matching",
                "remove `pre-matching`: the Surge manual allows it only on domain, IP, port, SUBNET, CELLULAR and set rules, and on logical rules made of them",
            ),
        ];
        for (raw, want) in says {
            assert_eq!(hint(raw), want, "{raw}");
        }
    }

    /// A set's URL may carry a user name and a password. The rule's text —
    /// what `Display` writes, and with it the request record, the API's rule
    /// list, the summary and `rule match --explain` — shows them as `***`
    /// and the rest of the line as it was; the set itself keeps the URL as
    /// written, for that is what is fetched.
    #[test]
    fn a_rule_is_written_without_the_user_info_of_its_set_url() {
        for (raw, shown) in [
            (
                "RULE-SET,https://al1ce:hunter2@rules.example/list.txt,DIRECT",
                "RULE-SET,https://***@rules.example/list.txt,DIRECT",
            ),
            (
                "DOMAIN-SET,https://al1ce:hunter2@rules.example:8443/d.txt?v=1#top,REJECT,update-interval=60",
                "DOMAIN-SET,https://***@rules.example:8443/d.txt?v=1#top,REJECT,update-interval=60",
            ),
            // the sets of a logical rule, at any depth
            (
                "AND,((PROTOCOL,UDP),(RULE-SET,https://al1ce:hunter2@rules.example/s.list)),REJECT",
                "AND,((PROTOCOL,UDP),(RULE-SET,https://***@rules.example/s.list)),REJECT",
            ),
            (
                "OR,((NOT,((DOMAIN-SET,https://al1ce:hunter2@rules.example/d.txt))),(DOMAIN,a.com)),Proxy",
                "OR,((NOT,((DOMAIN-SET,https://***@rules.example/d.txt))),(DOMAIN,a.com)),Proxy",
            ),
            // a field in quotes, a password with an `@` in it, a scheme in capitals
            (
                "RULE-SET,\"https://al1ce:hunter2@rules.example/list.txt\",DIRECT",
                "RULE-SET,\"https://***@rules.example/list.txt\",DIRECT",
            ),
            (
                "RULE-SET,https://al1ce:p@ss@rules.example/list.txt,DIRECT",
                "RULE-SET,https://***@rules.example/list.txt,DIRECT",
            ),
            (
                "RULE-SET,HTTPS://al1ce:hunter2@rules.example/list.txt,DIRECT",
                "RULE-SET,HTTPS://***@rules.example/list.txt,DIRECT",
            ),
            // a set is named by its URL: an `@` in the path, the query or the
            // fragment is no credential, and a line without a user name stays
            // as it was
            (
                "RULE-SET,https://cdn.example/gh/user/repo@main/list.txt,DIRECT",
                "RULE-SET,https://cdn.example/gh/user/repo@main/list.txt,DIRECT",
            ),
            (
                "RULE-SET,https://rules.example/list.txt?mail=a@b.example#c@d,DIRECT",
                "RULE-SET,https://rules.example/list.txt?mail=a@b.example#c@d,DIRECT",
            ),
            // the authority ends at a `?` or a `#` as well as at a `/`
            (
                "RULE-SET,https://rules.example?mail=a@b.example,DIRECT",
                "RULE-SET,https://rules.example?mail=a@b.example,DIRECT",
            ),
            (
                "RULE-SET,https://rules.example#c@d,DIRECT",
                "RULE-SET,https://rules.example#c@d,DIRECT",
            ),
            (
                "RULE-SET,https://al1ce:hunter2@rules.example?v=1,DIRECT",
                "RULE-SET,https://***@rules.example?v=1,DIRECT",
            ),
            (
                "RULE-SET,https://al1ce:hunter2@rules.example#top,DIRECT",
                "RULE-SET,https://***@rules.example#top,DIRECT",
            ),
        ] {
            let r = parse(raw).unwrap_or_else(|e| panic!("{raw}: {}", e.message));
            assert_eq!(r.to_string(), shown, "{raw}");
            assert_eq!(r.raw, shown, "{raw}");
            assert!(!format!("{r:?}").contains("hunter2"), "{raw}");
        }
        let r = parse("RULE-SET,https://al1ce:hunter2@rules.example/list.txt,DIRECT").unwrap();
        assert_eq!(
            r.kind,
            RuleKind::RuleSet(ResourceRef::Url(
                "https://al1ce:hunter2@rules.example/list.txt".into()
            ))
        );
    }

    /// The same for the lines of an inline rule set and of a set file, which
    /// have no policy.
    #[test]
    fn a_sub_rule_is_written_without_the_user_info_of_its_set_url() {
        let inline = HashSet::new();
        let ctx = ParseCtx {
            inline_rulesets: &inline,
            base_dir: Path::new("/profiles"),
            reach: Reach::Anywhere,
        };
        for (raw, shown) in [
            (
                "RULE-SET,https://al1ce:hunter2@rules.example/s.list,no-resolve",
                "RULE-SET,https://***@rules.example/s.list,no-resolve",
            ),
            (
                "DOMAIN-SET,https://al1ce:hunter2@rules.example/d.txt",
                "DOMAIN-SET,https://***@rules.example/d.txt",
            ),
            ("DOMAIN-SUFFIX,example.com", "DOMAIN-SUFFIX,example.com"),
        ] {
            let sub = parse_subrule(raw, &ctx).unwrap();
            assert_eq!(sub.raw, shown, "{raw}");
            assert!(!format!("{sub:?}").contains("hunter2"), "{raw}");
        }
    }

    /// `Debug` shows the URL of a set without its user name and its password,
    /// and every other kind of reference as it always showed it.
    #[test]
    fn the_debug_of_a_resource_reference_hides_the_user_info_of_a_url() {
        assert_eq!(
            format!(
                "{:?}",
                ResourceRef::Url(
                    "https://al1ce:hunter2@rules.example/gh/user/repo@main/a.list".into()
                )
            ),
            "Url(\"https://***@rules.example/gh/user/repo@main/a.list\")"
        );
        assert_eq!(
            format!(
                "{:?}",
                ResourceRef::Url("https://rules.example/a.list".into())
            ),
            "Url(\"https://rules.example/a.list\")"
        );
        assert_eq!(
            format!("{:?}", ResourceRef::Internal(InternalSet::Lan)),
            "Internal(Lan)"
        );
        assert_eq!(
            format!("{:?}", ResourceRef::Inline("Streaming".into())),
            "Inline(\"Streaming\")"
        );
        assert_eq!(
            format!("{:?}", ResourceRef::File(PathBuf::from("sets/a.list"))),
            "File(\"sets/a.list\")"
        );
        // pretty-printed, as the `Debug` of a loaded profile is
        assert_eq!(
            format!(
                "{:#?}",
                ResourceRef::Url("https://al1ce:hunter2@rules.example/a.list".into())
            ),
            "Url(\n    \"https://***@rules.example/a.list\",\n)"
        );
    }

    /// A line without a comma is all "type": a share link pasted at the end
    /// of the profile, a `key = value` that a header the parser does not take
    /// for one left in `[Rule]`. A type is quoted when it is shaped like one.
    #[test]
    fn a_rule_type_is_quoted_only_when_it_is_shaped_like_one() {
        let longest = "A".repeat(32);
        let too_long = "A".repeat(33);
        let said = |raw: &str| parse(raw).unwrap_err().message;
        for (raw, expected) in [
            ("DOMAIN-SUFFIX", "DOMAIN-SUFFIX: missing value".to_string()),
            (
                "domain-suffix,example.com",
                "DOMAIN-SUFFIX: missing policy".to_string(),
            ),
            ("FINAL", "FINAL: missing policy".to_string()),
            ("IP-CIDR6", "IP-CIDR6: missing value".to_string()),
            (
                "DOMAIN-FOO,a.com,DIRECT",
                "unknown rule type `DOMAIN-FOO`".to_string(),
            ),
            (&longest, format!("{longest}: missing value")),
            (&too_long, "missing value".to_string()),
            (
                "trojan://hunter2@t.example:443?sni=t.example#HK",
                "missing value".to_string(),
            ),
            ("CA-PASSPHRASE = hunter2", "missing value".to_string()),
            ("psk=hunter2", "missing value".to_string()),
            (
                "trojan://hunter2@t.example:443?sni=t.example#HK,a.com,DIRECT",
                "unknown rule type".to_string(),
            ),
            (
                "ca passphrase,a.com,DIRECT",
                "unknown rule type".to_string(),
            ),
        ] {
            assert_eq!(said(raw), expected, "{raw}");
        }
        // a line of a rule set has no policy; its type is quoted the same way
        let inline = HashSet::new();
        let ctx = ParseCtx {
            inline_rulesets: &inline,
            base_dir: Path::new("/profiles"),
            reach: Reach::Anywhere,
        };
        let said_sub = |raw: &str| parse_subrule(raw, &ctx).unwrap_err().message;
        assert_eq!(said_sub("DOMAIN-SUFFIX"), "DOMAIN-SUFFIX: missing value");
        assert_eq!(
            said_sub("trojan://hunter2@t.example:443?sni=t.example#HK"),
            "missing value"
        );
        assert_eq!(
            said_sub("DOMAIN-FOO,a.com"),
            "unknown rule type `DOMAIN-FOO`"
        );
        assert_eq!(said_sub("a b,c"), "unknown rule type");
    }

    /// A set that is a local file is named only by text that may name one. A
    /// URL, a built-in set and an inline set are no local files, and are named
    /// by any text. The refusal is a `W0033` with a suggestion, and the path
    /// does not get as far as being a file reference.
    #[test]
    fn a_local_file_is_a_set_only_for_text_that_may_name_one() {
        let inline = HashSet::from(["Streaming".to_string()]);
        for (reach, may) in [
            (Reach::Anywhere, true),
            (Reach::ManagedCopy, false),
            (Reach::Nowhere, false),
        ] {
            let ctx = ParseCtx {
                inline_rulesets: &inline,
                base_dir: Path::new("/profiles"),
                reach,
            };
            for raw in ["sets/a.list", "/abs/a.list", "../up.list", "C:\\a.list", ""] {
                for parse in [ResourceRef::parse, ResourceRef::parse_external] {
                    match parse(raw, &ctx) {
                        Ok(ResourceRef::File(_)) => assert!(may, "{reach:?}: {raw}"),
                        Err(e) => {
                            assert!(!may, "{reach:?}: {raw}");
                            assert_eq!(e.code, codes::W_REMOTE_LOCAL_ACCESS);
                            assert!(e.hint.is_some());
                            assert!(!e.message.contains(raw) || raw.is_empty());
                        }
                        other => panic!("{reach:?}: {raw}: {other:?}"),
                    }
                }
            }
            for raw in ["https://example.com/a.list", "HTTP://example.com/b"] {
                assert!(
                    matches!(ResourceRef::parse(raw, &ctx), Ok(ResourceRef::Url(_))),
                    "{reach:?}: {raw}"
                );
            }
            for raw in ["SYSTEM", "LAN", "Streaming"] {
                assert!(
                    !matches!(
                        ResourceRef::parse(raw, &ctx),
                        Ok(ResourceRef::File(_)) | Err(_)
                    ),
                    "{reach:?}: {raw}"
                );
            }
        }
    }

    #[test]
    fn domain_set_never_resolves_to_inline_ruleset() {
        let names: HashSet<String> = ["Foo".to_string()].into_iter().collect();
        let ctx = ParseCtx {
            inline_rulesets: &names,
            base_dir: Path::new("/base"),
            reach: Reach::Anywhere,
        };
        let r = parse_rule(
            "DOMAIN-SET,Foo,DIRECT",
            &ctx,
            &Span::new(Arc::from(Path::new("t.conf")), 1),
        )
        .unwrap();
        match r.kind {
            RuleKind::DomainSet(ResourceRef::File(p)) => {
                assert_eq!(p, Path::new("/base").join("Foo"))
            }
            other => panic!("unexpected {other:?}"),
        }
        let r = parse_rule(
            "RULE-SET,Foo,DIRECT",
            &ctx,
            &Span::new(Arc::from(Path::new("t.conf")), 2),
        )
        .unwrap();
        assert!(matches!(r.kind, RuleKind::RuleSet(ResourceRef::Inline(ref n)) if n == "Foo"));
    }
}
