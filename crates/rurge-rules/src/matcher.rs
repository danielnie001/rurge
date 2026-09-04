//! Per-rule matching (M2 design §6.3). Domain rules never trigger DNS; IP
//! rules ask for resolution via `Verdict::NeedsResolve` unless the target is
//! already an IP or the rule carries `no-resolve`.

use crate::set_format::SetKind;
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use rurge_config::rule::{
    HostnameType, Pattern, PortExpr, ProcessPattern, ProtocolKind, ResourceRef, RuleKind, SubRule,
};
use rurge_config::session::{ProcessInfo, SessionInfo};
use rurge_config::{Glob, GlobOptions};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Match,
    NoMatch,
    NeedsResolve,
}

impl From<bool> for Verdict {
    fn from(b: bool) -> Verdict {
        if b { Verdict::Match } else { Verdict::NoMatch }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedAddrs {
    pub v4: Vec<Ipv4Addr>,
    pub v6: Vec<Ipv6Addr>,
}

pub trait GeoLookup: Send + Sync {
    fn country(&self, ip: IpAddr) -> Option<[u8; 2]>;
    fn asn(&self, ip: IpAddr) -> Option<u32>;
}

/// No database loaded: every GEOIP / IP-ASN rule is a miss.
pub struct NoGeo;

impl GeoLookup for NoGeo {
    fn country(&self, _: IpAddr) -> Option<[u8; 2]> {
        None
    }
    fn asn(&self, _: IpAddr) -> Option<u32> {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubRuleHit {
    pub set: String,
    pub entry: String,
}

/// State of one `evaluate` call.
pub struct EvalCtx<'a> {
    pub resolved: Option<ResolvedAddrs>,
    /// Runtime notes (unsupported rule kinds, missing databases); the engine
    /// de-duplicates them per rule.
    pub notes: Vec<String>,
    /// Filled by `Matcher::Set` on a match (FR-RULE-15).
    pub sub_hit: Option<SubRuleHit>,
    geo: &'a dyn GeoLookup,
    country_cache: Option<(IpAddr, Option<[u8; 2]>)>,
    asn_cache: Option<(IpAddr, Option<u32>)>,
}

impl<'a> EvalCtx<'a> {
    pub fn new(geo: &'a dyn GeoLookup) -> EvalCtx<'a> {
        EvalCtx {
            resolved: None,
            notes: Vec::new(),
            sub_hit: None,
            geo,
            country_cache: None,
            asn_cache: None,
        }
    }

    pub fn country(&mut self, ip: IpAddr) -> Option<[u8; 2]> {
        if let Some((cached_ip, code)) = self.country_cache {
            if cached_ip == ip {
                return code;
            }
        }
        let code = self.geo.country(ip);
        self.country_cache = Some((ip, code));
        code
    }

    pub fn asn(&mut self, ip: IpAddr) -> Option<u32> {
        if let Some((cached_ip, asn)) = self.asn_cache {
            if cached_ip == ip {
                return asn;
            }
        }
        let asn = self.geo.asn(ip);
        self.asn_cache = Some((ip, asn));
        asn
    }
}

pub struct SetVerdict {
    pub verdict: Verdict,
    pub entry: Option<String>,
}

pub trait SetMatch: Send + Sync {
    fn name(&self) -> String;
    /// `no_resolve` / `extended` come from the referencing line and apply to the whole set.
    fn eval(
        &self,
        s: &SessionInfo,
        ctx: &mut EvalCtx<'_>,
        no_resolve: bool,
        extended: bool,
    ) -> SetVerdict;
}

pub type SetRef = Arc<dyn SetMatch>;

pub trait SetLookup {
    fn lookup(&self, r: &ResourceRef, kind: SetKind) -> SetRef;
}

pub enum Matcher {
    Domain(String),
    DomainSuffix(String),
    DomainKeyword(String),
    DomainWildcard(Glob),
    IpCidr(Ipv4Net),
    IpCidr6(Ipv6Net),
    GeoIp([u8; 2]),
    IpAsn(u32),
    UserAgent(Glob),
    UrlRegex(Pattern),
    ProcessName(ProcessPattern),
    DestPort(PortExpr),
    SrcPort(PortExpr),
    InPort(PortExpr),
    SrcIp(IpNet),
    Protocol(ProtocolKind),
    HostnameType(HostnameType),
    And(Vec<CompiledSubRule>),
    Or(Vec<CompiledSubRule>),
    Not(Box<CompiledSubRule>),
    Set(SetRef),
    /// Rule kinds without runtime support in this milestone: no match + one note.
    Unsupported(&'static str),
    /// CELLULAR-RADIO / CELLULAR-CARRIER: never match, silently.
    Never,
    Final,
}

pub struct CompiledSubRule {
    pub matcher: Matcher,
    pub no_resolve: bool,
    pub extended: bool,
    pub raw: String,
}

pub fn compile_subrule(sub: &SubRule, sets: &dyn SetLookup) -> CompiledSubRule {
    CompiledSubRule {
        matcher: Matcher::compile(&sub.kind, sets),
        no_resolve: sub.no_resolve,
        extended: sub.extended_matching,
        raw: sub.raw.clone(),
    }
}

impl CompiledSubRule {
    pub fn eval(&self, s: &SessionInfo, ctx: &mut EvalCtx<'_>) -> Verdict {
        self.matcher.eval(s, ctx, self.no_resolve, self.extended)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Family {
    V4,
    V6,
    Any,
}

impl Matcher {
    pub fn compile(kind: &RuleKind, sets: &dyn SetLookup) -> Matcher {
        match kind {
            RuleKind::Domain(d) => Matcher::Domain(d.to_ascii_lowercase()),
            RuleKind::DomainSuffix(d) => {
                Matcher::DomainSuffix(d.trim_start_matches('.').to_ascii_lowercase())
            }
            RuleKind::DomainKeyword(k) => Matcher::DomainKeyword(k.to_ascii_lowercase()),
            RuleKind::DomainWildcard(g) => Matcher::DomainWildcard(g.clone()),
            RuleKind::DomainSet(r) => Matcher::Set(sets.lookup(r, SetKind::DomainSet)),
            RuleKind::IpCidr(n) => Matcher::IpCidr(*n),
            RuleKind::IpCidr6(n) => Matcher::IpCidr6(*n),
            RuleKind::GeoIp(code) => {
                let b = code.trim().as_bytes();
                if b.len() == 2 {
                    Matcher::GeoIp([b[0].to_ascii_uppercase(), b[1].to_ascii_uppercase()])
                } else {
                    Matcher::Never
                }
            }
            RuleKind::IpAsn(n) => Matcher::IpAsn(*n),
            RuleKind::UserAgent(g) => Matcher::UserAgent(g.clone()),
            RuleKind::UrlRegex(p) => Matcher::UrlRegex(p.clone()),
            RuleKind::ProcessName(p) => Matcher::ProcessName(match p {
                // The rule text's Glob is compiled once here from the raw
                // (un-normalized) value; rebuild it from normalized source so
                // it compares symmetrically with the normalized runtime path
                // `process_matches` matches against (FR-RULE-01).
                ProcessPattern::Path(g) => {
                    let normalized = normalize_process_path(g.source());
                    let opts = GlobOptions {
                        case_insensitive: cfg!(windows),
                        classes: false,
                    };
                    ProcessPattern::Path(Glob::new(&normalized, opts).unwrap_or_else(|_| g.clone()))
                }
                other => other.clone(),
            }),
            RuleKind::DestPort(p) => Matcher::DestPort(*p),
            RuleKind::SrcPort(p) => Matcher::SrcPort(*p),
            RuleKind::InPort(p) => Matcher::InPort(*p),
            RuleKind::SrcIp(n) => Matcher::SrcIp(*n),
            RuleKind::DeviceName(_) => Matcher::Unsupported("DEVICE-NAME"),
            RuleKind::MacAddress(_) => Matcher::Unsupported("MAC-ADDRESS"),
            RuleKind::Protocol(p) => Matcher::Protocol(*p),
            RuleKind::HostnameType(t) => Matcher::HostnameType(*t),
            RuleKind::Subnet(_) => Matcher::Unsupported("SUBNET"),
            RuleKind::CellularRadio(_) | RuleKind::CellularCarrier(_) => Matcher::Never,
            RuleKind::And(subs) => {
                Matcher::And(subs.iter().map(|s| compile_subrule(s, sets)).collect())
            }
            RuleKind::Or(subs) => {
                Matcher::Or(subs.iter().map(|s| compile_subrule(s, sets)).collect())
            }
            RuleKind::Not(sub) => Matcher::Not(Box::new(compile_subrule(sub, sets))),
            RuleKind::Script(_) => Matcher::Unsupported("SCRIPT"),
            RuleKind::RuleSet(r) => Matcher::Set(sets.lookup(r, SetKind::RuleSet)),
            RuleKind::Final => Matcher::Final,
        }
    }

    pub fn eval(
        &self,
        s: &SessionInfo,
        ctx: &mut EvalCtx<'_>,
        no_resolve: bool,
        extended: bool,
    ) -> Verdict {
        match self {
            Matcher::Domain(d) => domain_targets(s, extended).any(|h| h == d.as_str()).into(),
            Matcher::DomainSuffix(d) => domain_targets(s, extended).any(|h| is_suffix(h, d)).into(),
            Matcher::DomainKeyword(k) => domain_targets(s, extended)
                .any(|h| h.contains(k.as_str()))
                .into(),
            Matcher::DomainWildcard(g) => domain_targets(s, extended).any(|h| g.matches(h)).into(),
            Matcher::IpCidr(net) => match target_ip(s, ctx, no_resolve, Family::V4) {
                Ok(IpAddr::V4(ip)) => net.contains(&ip).into(),
                Ok(_) => Verdict::NoMatch,
                Err(v) => v,
            },
            Matcher::IpCidr6(net) => match target_ip(s, ctx, no_resolve, Family::V6) {
                Ok(IpAddr::V6(ip)) => net.contains(&ip).into(),
                Ok(_) => Verdict::NoMatch,
                Err(v) => v,
            },
            Matcher::GeoIp(code) => match target_ip(s, ctx, no_resolve, Family::Any) {
                Ok(ip) => (ctx.country(ip) == Some(*code)).into(),
                Err(v) => v,
            },
            Matcher::IpAsn(asn) => match target_ip(s, ctx, no_resolve, Family::Any) {
                Ok(ip) => (ctx.asn(ip) == Some(*asn)).into(),
                Err(v) => v,
            },
            Matcher::UserAgent(g) => s
                .user_agent
                .as_deref()
                .is_some_and(|ua| g.matches(ua))
                .into(),
            Matcher::UrlRegex(p) => {
                let Some(url) = s.url.as_deref() else {
                    return Verdict::NoMatch;
                };
                if p.regex.is_match(url).unwrap_or(false) {
                    return Verdict::Match;
                }
                if extended {
                    for host in [s.sni.as_deref(), s.http_host.as_deref()]
                        .into_iter()
                        .flatten()
                    {
                        if let Some(u) = replace_url_host(url, host) {
                            if p.regex.is_match(&u).unwrap_or(false) {
                                return Verdict::Match;
                            }
                        }
                    }
                }
                Verdict::NoMatch
            }
            Matcher::ProcessName(pp) => s
                .process
                .as_ref()
                .is_some_and(|p| process_matches(pp, p))
                .into(),
            Matcher::DestPort(p) => p.matches(s.dst_port).into(),
            Matcher::SrcPort(p) => p.matches(s.src.port()).into(),
            Matcher::InPort(p) => p.matches(s.in_port).into(),
            Matcher::SrcIp(net) => net.contains(&s.src.ip()).into(),
            Matcher::Protocol(k) => (s.protocol == Some(*k)).into(),
            Matcher::HostnameType(t) => (s.hostname_type() == *t).into(),
            Matcher::And(subs) => {
                let mut needs = false;
                for sub in subs {
                    match sub.eval(s, ctx) {
                        Verdict::NoMatch => return Verdict::NoMatch,
                        Verdict::NeedsResolve => needs = true,
                        Verdict::Match => {}
                    }
                }
                if needs {
                    Verdict::NeedsResolve
                } else {
                    Verdict::Match
                }
            }
            Matcher::Or(subs) => {
                let mut needs = false;
                for sub in subs {
                    match sub.eval(s, ctx) {
                        Verdict::Match => return Verdict::Match,
                        Verdict::NeedsResolve => needs = true,
                        Verdict::NoMatch => {}
                    }
                }
                if needs {
                    Verdict::NeedsResolve
                } else {
                    Verdict::NoMatch
                }
            }
            Matcher::Not(sub) => match sub.eval(s, ctx) {
                Verdict::Match => Verdict::NoMatch,
                Verdict::NoMatch => Verdict::Match,
                Verdict::NeedsResolve => Verdict::NeedsResolve,
            },
            Matcher::Set(set) => {
                let v = set.eval(s, ctx, no_resolve, extended);
                if v.verdict == Verdict::Match {
                    ctx.sub_hit = Some(SubRuleHit {
                        set: set.name(),
                        entry: v.entry.unwrap_or_default(),
                    });
                }
                v.verdict
            }
            Matcher::Unsupported(kind) => {
                ctx.notes.push(format!(
                    "{kind} rules are not supported in this version; treated as no match"
                ));
                Verdict::NoMatch
            }
            Matcher::Never => Verdict::NoMatch,
            Matcher::Final => Verdict::Match,
        }
    }
}

/// The hostnames a domain rule is checked against: the destination, plus the
/// SNI and HTTP Host when `extended-matching` is set. All lowercase by contract.
fn domain_targets(s: &SessionInfo, extended: bool) -> impl Iterator<Item = &str> {
    s.dst_host
        .as_domain()
        .into_iter()
        .chain(extended.then_some(s.sni.as_deref()).flatten())
        .chain(extended.then_some(s.http_host.as_deref()).flatten())
}

/// `host == suffix` or `host` ends with `.suffix`.
fn is_suffix(host: &str, suffix: &str) -> bool {
    host == suffix
        || (host.len() > suffix.len()
            && host.ends_with(suffix)
            && host.as_bytes()[host.len() - suffix.len() - 1] == b'.')
}

/// The IP an IP-based rule tests, or the verdict to return instead.
pub(crate) fn target_ip(
    s: &SessionInfo,
    ctx: &EvalCtx<'_>,
    no_resolve: bool,
    family: Family,
) -> Result<IpAddr, Verdict> {
    if let Some(ip) = s.dst_host.as_ip() {
        let ok = match family {
            Family::V4 => ip.is_ipv4(),
            Family::V6 => ip.is_ipv6(),
            Family::Any => true,
        };
        return if ok { Ok(ip) } else { Err(Verdict::NoMatch) };
    }
    let Some(r) = &ctx.resolved else {
        return Err(if no_resolve {
            Verdict::NoMatch
        } else {
            Verdict::NeedsResolve
        });
    };
    let pick = match family {
        Family::V4 => r.v4.first().map(|v| IpAddr::V4(*v)),
        Family::V6 => r.v6.first().map(|v| IpAddr::V6(*v)),
        Family::Any => {
            r.v4.first()
                .map(|v| IpAddr::V4(*v))
                .or_else(|| r.v6.first().map(|v| IpAddr::V6(*v)))
        }
    };
    pick.ok_or(Verdict::NoMatch)
}

/// Replace the host of `scheme://host[:port]/path` keeping the port; `None` if
/// the URL has no authority.
fn replace_url_host(url: &str, host: &str) -> Option<String> {
    let scheme_end = url.find("://")? + 3;
    let rest = &url[scheme_end..];
    let auth_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    let port = authority
        .rsplit_once(':')
        .filter(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        .map(|(_, p)| p);
    let mut out = String::with_capacity(url.len() + host.len());
    out.push_str(&url[..scheme_end]);
    out.push_str(host);
    if let Some(p) = port {
        out.push(':');
        out.push_str(p);
    }
    out.push_str(&rest[auth_end..]);
    Some(out)
}

/// Windows paths compare case-insensitively with `\` separators (FR-RULE-01).
pub fn normalize_process_path(path: &str) -> String {
    normalize_process_path_for(cfg!(windows), path)
}

fn normalize_process_path_for(windows: bool, path: &str) -> String {
    if windows {
        path.replace('/', "\\").to_ascii_lowercase()
    } else {
        path.to_string()
    }
}

fn process_matches(pattern: &ProcessPattern, p: &ProcessInfo) -> bool {
    match pattern {
        ProcessPattern::Name(g) => g.matches(&p.name),
        ProcessPattern::Path(g) => p
            .path
            .as_deref()
            .is_some_and(|path| g.matches(&normalize_process_path(path))),
        ProcessPattern::Prefix(prefix) => p.path.as_deref().is_some_and(|path| {
            normalize_process_path(path).starts_with(&normalize_process_path(prefix))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use rurge_config::rule::{ParseCtx, parse_subrule};
    use std::collections::HashSet;
    use std::path::Path;

    struct NoSets;
    impl SetLookup for NoSets {
        fn lookup(&self, _: &ResourceRef, _: SetKind) -> SetRef {
            panic!("no sets in matcher tests")
        }
    }

    struct FakeGeo;
    impl GeoLookup for FakeGeo {
        fn country(&self, ip: IpAddr) -> Option<[u8; 2]> {
            match ip {
                IpAddr::V4(v) if v.octets()[0] == 8 => Some(*b"US"),
                IpAddr::V6(_) => Some(*b"JP"),
                _ => None,
            }
        }
        fn asn(&self, ip: IpAddr) -> Option<u32> {
            matches!(ip, IpAddr::V4(v) if v.octets()[0] == 8).then_some(15169)
        }
    }

    fn compile(raw: &str) -> CompiledSubRule {
        let names: HashSet<String> = HashSet::new();
        let ctx = ParseCtx {
            inline_rulesets: &names,
            base_dir: Path::new("."),
        };
        let sub = parse_subrule(raw, &ctx).unwrap_or_else(|e| panic!("{raw}: {}", e.message));
        compile_subrule(&sub, &NoSets)
    }

    fn session(host: &str) -> SessionInfo {
        SessionInfo::tcp(HostName::parse(host), 443)
    }

    fn eval(raw: &str, s: &SessionInfo) -> Verdict {
        let mut ctx = EvalCtx::new(&FakeGeo);
        compile(raw).eval(s, &mut ctx)
    }

    fn eval_resolved(raw: &str, s: &SessionInfo, v4: &[&str], v6: &[&str]) -> Verdict {
        let mut ctx = EvalCtx::new(&FakeGeo);
        ctx.resolved = Some(ResolvedAddrs {
            v4: v4.iter().map(|a| a.parse().unwrap()).collect(),
            v6: v6.iter().map(|a| a.parse().unwrap()).collect(),
        });
        compile(raw).eval(s, &mut ctx)
    }

    #[test]
    fn domain_rules_never_need_dns() {
        let s = session("www.example.com");
        assert_eq!(eval("DOMAIN,www.example.com", &s), Verdict::Match);
        assert_eq!(eval("DOMAIN,example.com", &s), Verdict::NoMatch);
        assert_eq!(eval("DOMAIN-SUFFIX,example.com", &s), Verdict::Match);
        assert_eq!(eval("DOMAIN-SUFFIX,ample.com", &s), Verdict::NoMatch);
        assert_eq!(eval("DOMAIN-KEYWORD,exam", &s), Verdict::Match);
        assert_eq!(eval("DOMAIN-WILDCARD,www.*.com", &s), Verdict::Match);
        assert_eq!(
            eval("DOMAIN-SUFFIX,example.com", &session("1.2.3.4")),
            Verdict::NoMatch
        );
    }

    #[test]
    fn extended_matching_uses_sni_and_host() {
        let mut s = session("1.2.3.4");
        s.sni = Some("api.example.com".into());
        assert_eq!(eval("DOMAIN-SUFFIX,example.com", &s), Verdict::NoMatch);
        assert_eq!(
            eval("DOMAIN-SUFFIX,example.com,extended-matching", &s),
            Verdict::Match
        );
        s.sni = None;
        s.http_host = Some("api.example.com".into());
        assert_eq!(
            eval("DOMAIN,api.example.com,extended-matching", &s),
            Verdict::Match
        );
    }

    #[test]
    fn ip_rules_on_ip_targets_respect_family() {
        assert_eq!(
            eval("IP-CIDR,10.0.0.0/8", &session("10.1.2.3")),
            Verdict::Match
        );
        assert_eq!(
            eval("IP-CIDR,10.0.0.0/8", &session("11.1.2.3")),
            Verdict::NoMatch
        );
        assert_eq!(
            eval("IP-CIDR,10.0.0.0/8", &session("[fd00::1]")),
            Verdict::NoMatch
        );
        assert_eq!(
            eval("IP-CIDR6,fd00::/8", &session("[fd00::1]")),
            Verdict::Match
        );
        assert_eq!(
            eval("IP-CIDR6,fd00::/8", &session("10.1.2.3")),
            Verdict::NoMatch
        );
        assert_eq!(eval("GEOIP,us", &session("8.8.8.8")), Verdict::Match);
        assert_eq!(eval("GEOIP,US", &session("1.1.1.1")), Verdict::NoMatch);
        assert_eq!(eval("GEOIP,JP", &session("[2001:db8::1]")), Verdict::Match);
        assert_eq!(eval("IP-ASN,15169", &session("8.8.4.4")), Verdict::Match);
    }

    #[test]
    fn ip_rules_on_domains_ask_for_resolution_unless_no_resolve() {
        let s = session("example.com");
        assert_eq!(eval("IP-CIDR,10.0.0.0/8", &s), Verdict::NeedsResolve);
        assert_eq!(eval("IP-CIDR,10.0.0.0/8,no-resolve", &s), Verdict::NoMatch);
        assert_eq!(eval("GEOIP,US", &s), Verdict::NeedsResolve);
        assert_eq!(
            eval_resolved("IP-CIDR,10.0.0.0/8", &s, &["10.9.9.9", "11.0.0.1"], &[]),
            Verdict::Match
        );
        assert_eq!(
            eval_resolved("IP-CIDR,11.0.0.0/8", &s, &["10.9.9.9", "11.0.0.1"], &[]),
            Verdict::NoMatch
        );
        assert_eq!(
            eval_resolved("IP-CIDR6,fd00::/8", &s, &["10.9.9.9"], &["fd00::1"]),
            Verdict::Match
        );
        assert_eq!(
            eval_resolved("IP-CIDR,10.0.0.0/8", &s, &[], &["fd00::1"]),
            Verdict::NoMatch
        );
        assert_eq!(
            eval_resolved("GEOIP,JP", &s, &[], &["2001:db8::1"]),
            Verdict::Match
        );
        assert_eq!(
            eval_resolved("GEOIP,US", &s, &["8.8.8.8"], &["2001:db8::1"]),
            Verdict::Match
        );
    }

    #[test]
    fn url_regex_with_extended_matching_rewrites_host() {
        let mut s = session("1.2.3.4");
        s.url = Some("http://1.2.3.4:8080/path?q=1".into());
        assert_eq!(
            eval("URL-REGEX,^http://1\\.2\\.3\\.4:8080/path", &s),
            Verdict::Match
        );
        assert_eq!(
            eval("URL-REGEX,^http://example\\.com:8080/path", &s),
            Verdict::NoMatch
        );
        s.http_host = Some("example.com".into());
        assert_eq!(
            eval(
                "URL-REGEX,^http://example\\.com:8080/path,extended-matching",
                &s
            ),
            Verdict::Match
        );
        assert_eq!(
            replace_url_host("https://a.b/x", "c.d"),
            Some("https://c.d/x".into())
        );
        assert_eq!(
            replace_url_host("https://a.b:8443", "c.d"),
            Some("https://c.d:8443".into())
        );
        assert_eq!(replace_url_host("no-scheme", "c.d"), None);
    }

    #[test]
    fn port_source_protocol_and_hostname_type_rules() {
        let mut s = session("example.com");
        s.src = "192.168.1.9:51000".parse().unwrap();
        s.in_port = 6152;
        s.protocol = Some(ProtocolKind::Https);
        assert_eq!(eval("DEST-PORT,443", &s), Verdict::Match);
        assert_eq!(eval("DEST-PORT,>=1000", &s), Verdict::NoMatch);
        assert_eq!(eval("SRC-PORT,50000-52000", &s), Verdict::Match);
        assert_eq!(eval("IN-PORT,6152", &s), Verdict::Match);
        assert_eq!(eval("SRC-IP,192.168.1.0/24", &s), Verdict::Match);
        assert_eq!(eval("SRC-IP,192.168.1.9", &s), Verdict::Match);
        assert_eq!(eval("PROTOCOL,HTTPS", &s), Verdict::Match);
        assert_eq!(eval("PROTOCOL,HTTP", &s), Verdict::NoMatch);
        assert_eq!(eval("HOSTNAME-TYPE,DOMAIN", &s), Verdict::Match);
        assert_eq!(
            eval("HOSTNAME-TYPE,IPv4", &session("1.1.1.1")),
            Verdict::Match
        );
        assert_eq!(
            eval("HOSTNAME-TYPE,SIMPLE", &session("nas")),
            Verdict::Match
        );
        let mut ua = session("example.com");
        ua.user_agent = Some("Mozilla/5.0 (Macintosh)".into());
        assert_eq!(eval("USER-AGENT,Mozilla*", &ua), Verdict::Match);
        assert_eq!(eval("USER-AGENT,mozilla*", &ua), Verdict::NoMatch);
        assert_eq!(eval("USER-AGENT,Mozilla*", &s), Verdict::NoMatch);
    }

    #[test]
    fn process_name_modes() {
        let mut s = session("example.com");
        s.process = Some(ProcessInfo {
            name: "curl".into(),
            path: Some("/usr/bin/curl".into()),
        });
        assert_eq!(eval("PROCESS-NAME,curl", &s), Verdict::Match);
        assert_eq!(eval("PROCESS-NAME,wget", &s), Verdict::NoMatch);
        assert_eq!(eval("PROCESS-NAME,/usr/bin/curl", &s), Verdict::Match);
        assert_eq!(eval("PROCESS-NAME,/usr/bin/*", &s), Verdict::Match);
        assert_eq!(
            eval("PROCESS-NAME,curl", &session("example.com")),
            Verdict::NoMatch
        );
        assert_eq!(
            normalize_process_path_for(true, "C:/Program Files/App/app.exe"),
            "c:\\program files\\app\\app.exe"
        );
        assert_eq!(
            normalize_process_path_for(false, "/usr/bin/curl"),
            "/usr/bin/curl"
        );
    }

    #[test]
    fn logical_rules_short_circuit_and_propagate_needs_resolve() {
        let s = session("www.example.com");
        assert_eq!(
            eval("AND,((DOMAIN-SUFFIX,example.com),(DEST-PORT,443))", &s),
            Verdict::Match
        );
        assert_eq!(
            eval("AND,((DOMAIN-SUFFIX,example.com),(DEST-PORT,80))", &s),
            Verdict::NoMatch
        );
        assert_eq!(
            eval("AND,((DEST-PORT,443),(IP-CIDR,10.0.0.0/8))", &s),
            Verdict::NeedsResolve
        );
        assert_eq!(
            eval("AND,((DEST-PORT,80),(IP-CIDR,10.0.0.0/8))", &s),
            Verdict::NoMatch
        );
        assert_eq!(
            eval("OR,((IP-CIDR,10.0.0.0/8),(DOMAIN-SUFFIX,example.com))", &s),
            Verdict::Match
        );
        assert_eq!(
            eval("OR,((IP-CIDR,10.0.0.0/8),(DOMAIN-SUFFIX,other.com))", &s),
            Verdict::NeedsResolve
        );
        assert_eq!(
            eval(
                "OR,((IP-CIDR,10.0.0.0/8,no-resolve),(DOMAIN-SUFFIX,other.com))",
                &s
            ),
            Verdict::NoMatch
        );
        assert_eq!(
            eval("NOT,((DOMAIN-SUFFIX,example.com))", &s),
            Verdict::NoMatch
        );
        assert_eq!(
            eval("NOT,((IP-CIDR,10.0.0.0/8))", &s),
            Verdict::NeedsResolve
        );
    }

    #[test]
    fn unsupported_and_never_kinds() {
        let s = session("example.com");
        let mut ctx = EvalCtx::new(&NoGeo);
        assert_eq!(
            compile("SUBNET,SSID:Home").eval(&s, &mut ctx),
            Verdict::NoMatch
        );
        assert_eq!(ctx.notes.len(), 1);
        assert!(ctx.notes[0].contains("SUBNET"));
        let mut ctx = EvalCtx::new(&NoGeo);
        assert_eq!(
            compile("CELLULAR-RADIO,LTE").eval(&s, &mut ctx),
            Verdict::NoMatch
        );
        assert!(ctx.notes.is_empty());
        assert_eq!(eval("GEOIP,US", &session("8.8.8.8")), Verdict::Match);
        let mut ctx = EvalCtx::new(&NoGeo);
        assert_eq!(
            compile("GEOIP,US").eval(&session("8.8.8.8"), &mut ctx),
            Verdict::NoMatch
        );
    }

    #[test]
    fn geo_lookups_are_cached_per_ip() {
        // `Cell` is not Sync; the trait requires Send + Sync, so wrap the counter.
        struct SyncCounting(std::sync::Mutex<u32>);
        impl GeoLookup for SyncCounting {
            fn country(&self, _: IpAddr) -> Option<[u8; 2]> {
                *self.0.lock().unwrap() += 1;
                Some(*b"US")
            }
            fn asn(&self, _: IpAddr) -> Option<u32> {
                None
            }
        }
        let geo = SyncCounting(std::sync::Mutex::new(0));
        let mut ctx = EvalCtx::new(&geo);
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        ctx.country(ip);
        ctx.country(ip);
        assert_eq!(*geo.0.lock().unwrap(), 1);
    }
}
