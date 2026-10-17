//! Per-rule matching (M2 design §6.3). Domain rules never trigger DNS; IP
//! rules ask for resolution via `Verdict::NeedsResolve` unless the target is
//! already an IP or the rule carries `no-resolve`.

use crate::registry::MAX_NESTING;
use crate::set_format::SetKind;
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use rurge_config::rule::{
    HostnameType, Pattern, PortExpr, ProcessPattern, ProtocolKind, ResourceRef, RuleKind, SubRule,
};
use rurge_config::session::{ProcessInfo, SessionInfo, Transport};
use rurge_config::{Glob, GlobOptions, NetworkFacts, SubnetExpr};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

/// What a rule says of a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Match,
    NoMatch,
    /// The rule cannot be decided before the destination is resolved.
    NeedsResolve,
    /// The rule cannot be decided before the process that opened the session
    /// is known.
    NeedsProcess,
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

/// What a database says of an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeoResult<T> {
    /// The database is not loaded: nothing that depends on it matches.
    NotLoaded,
    /// The database is loaded and has no result for the address — what
    /// `GEOIP,UNKNOWN` and `IP-ASN,UNKNOWN` match.
    Unknown,
    Known(T),
}

pub trait GeoLookup: Send + Sync {
    fn country(&self, ip: IpAddr) -> GeoResult<[u8; 2]>;
    fn asn(&self, ip: IpAddr) -> GeoResult<u32>;
}

/// No database loaded: every GEOIP / IP-ASN rule is a miss.
pub struct NoGeo;

impl GeoLookup for NoGeo {
    fn country(&self, _: IpAddr) -> GeoResult<[u8; 2]> {
        GeoResult::NotLoaded
    }
    fn asn(&self, _: IpAddr) -> GeoResult<u32> {
        GeoResult::NotLoaded
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubRuleHit {
    pub set: String,
    pub entry: String,
}

/// What an evaluation has been told of the process that opened the session,
/// besides the one the session itself carries (phase 3 M2 design 5.1).
pub(crate) enum ProcessState {
    /// Nobody has asked yet: a `PROCESS-NAME` rule can only say that it needs
    /// the process.
    NotAsked,
    /// Asked, once for the whole evaluation: the process, or `None` when it
    /// could not be found.
    Asked(Option<ProcessInfo>),
}

/// State of one `evaluate` call.
pub struct EvalCtx<'a> {
    pub resolved: Option<ResolvedAddrs>,
    /// The process of the session, once the engine has asked for it.
    pub(crate) process: ProcessState,
    /// Runtime notes (unsupported rule kinds, missing databases); the engine
    /// de-duplicates them per rule.
    pub notes: Vec<String>,
    /// Filled by `Matcher::Set` on a match (FR-RULE-15). The engine clears it
    /// before each rule; the logical matchers see to it that a rule leaves
    /// only the hit of a set that is part of why it matched.
    pub sub_hit: Option<SubRuleHit>,
    geo: &'a dyn GeoLookup,
    /// The network the machine is on, with `subnet-exp-wifi-always-match`
    /// (phase 3 M1 design 8). `None`: this evaluation was never told, and no
    /// `SUBNET` rule matches in it.
    network: Option<(&'a NetworkFacts, bool)>,
    country_cache: Option<(IpAddr, GeoResult<[u8; 2]>)>,
    asn_cache: Option<(IpAddr, GeoResult<u32>)>,
    /// How many `Matcher::Set` evaluations are currently nested. `SetRegistry`
    /// rejects cycles at compile time, but two rule-sets that independently
    /// hot-reload to reference each other can still form a live cycle in the
    /// handle graph (each reload only re-checks its own file against
    /// itself). This is the runtime backstop that guarantees evaluation
    /// still terminates if that ever happens.
    set_depth: usize,
}

impl<'a> EvalCtx<'a> {
    /// An evaluation that knows nothing of the network.
    pub fn new(geo: &'a dyn GeoLookup) -> EvalCtx<'a> {
        EvalCtx {
            resolved: None,
            process: ProcessState::NotAsked,
            notes: Vec::new(),
            sub_hit: None,
            geo,
            network: None,
            country_cache: None,
            asn_cache: None,
            set_depth: 0,
        }
    }

    /// An evaluation on `facts`, the network as it is now, which does not
    /// change under it: the caller takes a snapshot when the evaluation
    /// starts. `wifi_always_match` is `[General] subnet-exp-wifi-always-match`.
    pub fn with_network(
        geo: &'a dyn GeoLookup,
        facts: &'a NetworkFacts,
        wifi_always_match: bool,
    ) -> EvalCtx<'a> {
        EvalCtx {
            network: Some((facts, wifi_always_match)),
            ..EvalCtx::new(geo)
        }
    }

    pub fn country(&mut self, ip: IpAddr) -> GeoResult<[u8; 2]> {
        if let Some((cached_ip, code)) = self.country_cache
            && cached_ip == ip
        {
            return code;
        }
        let code = self.geo.country(ip);
        self.country_cache = Some((ip, code));
        code
    }

    pub fn asn(&mut self, ip: IpAddr) -> GeoResult<u32> {
        if let Some((cached_ip, asn)) = self.asn_cache
            && cached_ip == ip
        {
            return asn;
        }
        let asn = self.geo.asn(ip);
        self.asn_cache = Some((ip, asn));
        asn
    }

    /// Runs `eval`, the branches of an `AND` / `OR`. A hit is only the reason
    /// the rule matched when the branch that wrote it is part of the match —
    /// one that did not decide anything (an `AND` that failed on its other
    /// half) leaves `sub_hit` as it found it. The hit found on entry is moved
    /// aside and back, not cloned.
    fn in_branches(&mut self, eval: impl FnOnce(&mut Self) -> Verdict) -> Verdict {
        let found = self.sub_hit.take();
        let verdict = eval(self);
        if verdict != Verdict::Match || self.sub_hit.is_none() {
            self.sub_hit = found;
        }
        verdict
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
    GeoIpUnknown,
    IpAsn(u32),
    IpAsnUnknown,
    UserAgent(Glob),
    UrlRegex(Pattern),
    ProcessName(ProcessPattern),
    DestPort(PortExpr),
    SrcPort(PortExpr),
    InPort(PortExpr),
    SrcIp(IpNet),
    Protocol(ProtocolKind),
    HostnameType(HostnameType),
    /// Looks at the network of the evaluation's context.
    Subnet(SubnetExpr),
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
            RuleKind::GeoIpUnknown => Matcher::GeoIpUnknown,
            RuleKind::IpAsn(n) => Matcher::IpAsn(*n),
            RuleKind::IpAsnUnknown => Matcher::IpAsnUnknown,
            RuleKind::UserAgent(g) => Matcher::UserAgent(g.clone()),
            RuleKind::UrlRegex(p) => Matcher::UrlRegex(p.clone()),
            RuleKind::ProcessName(p) => {
                Matcher::ProcessName(compile_process_pattern(cfg!(windows), p))
            }
            RuleKind::DestPort(p) => Matcher::DestPort(*p),
            RuleKind::SrcPort(p) => Matcher::SrcPort(*p),
            RuleKind::InPort(p) => Matcher::InPort(*p),
            RuleKind::SrcIp(n) => Matcher::SrcIp(*n),
            RuleKind::DeviceName(_) => Matcher::Unsupported("DEVICE-NAME"),
            RuleKind::MacAddress(_) => Matcher::Unsupported("MAC-ADDRESS"),
            RuleKind::Protocol(p) => Matcher::Protocol(*p),
            RuleKind::HostnameType(t) => Matcher::HostnameType(*t),
            RuleKind::Subnet(expr) => Matcher::Subnet(expr.clone()),
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
                Ok(ip) => (ctx.country(ip) == GeoResult::Known(*code)).into(),
                Err(v) => v,
            },
            Matcher::GeoIpUnknown => match target_ip(s, ctx, no_resolve, Family::Any) {
                Ok(ip) => (ctx.country(ip) == GeoResult::Unknown).into(),
                Err(v) => v,
            },
            Matcher::IpAsn(asn) => match target_ip(s, ctx, no_resolve, Family::Any) {
                Ok(ip) => (ctx.asn(ip) == GeoResult::Known(*asn)).into(),
                Err(v) => v,
            },
            Matcher::IpAsnUnknown => match target_ip(s, ctx, no_resolve, Family::Any) {
                Ok(ip) => (ctx.asn(ip) == GeoResult::Unknown).into(),
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
                        if let Some(u) = replace_url_host(url, host)
                            && p.regex.is_match(&u).unwrap_or(false)
                        {
                            return Verdict::Match;
                        }
                    }
                }
                Verdict::NoMatch
            }
            Matcher::ProcessName(pp) => {
                // the process the caller named up front is read first, then
                // the one the evaluation was told
                let process = match (&s.process, &ctx.process) {
                    (Some(given), _) => Some(given),
                    (None, ProcessState::NotAsked) => return Verdict::NeedsProcess,
                    (None, ProcessState::Asked(found)) => found.as_ref(),
                };
                process.is_some_and(|p| process_matches(pp, p)).into()
            }
            Matcher::DestPort(p) => p.matches(s.dst_port).into(),
            Matcher::SrcPort(p) => p.matches(s.src.port()).into(),
            Matcher::InPort(p) => p.matches(s.in_port).into(),
            Matcher::SrcIp(net) => net.contains(&s.src.ip()).into(),
            // `PROTOCOL,UDP` also matches QUIC and STUN (manual), and
            // `PROTOCOL,TCP` every TCP session: the transport decides
            Matcher::Protocol(k) => (s.protocol == Some(*k)
                || matches!(
                    (k, s.transport),
                    (ProtocolKind::Tcp, Transport::Tcp) | (ProtocolKind::Udp, Transport::Udp)
                ))
            .into(),
            Matcher::HostnameType(t) => (s.hostname_type() == *t).into(),
            Matcher::Subnet(expr) => ctx
                .network
                .is_some_and(|(facts, wifi_always_match)| expr.matches(facts, wifi_always_match))
                .into(),
            Matcher::And(subs) => ctx.in_branches(|ctx| {
                let (mut needs_process, mut needs_resolve) = (false, false);
                for sub in subs {
                    match sub.eval(s, ctx) {
                        Verdict::NoMatch => return Verdict::NoMatch,
                        Verdict::NeedsProcess => needs_process = true,
                        Verdict::NeedsResolve => needs_resolve = true,
                        Verdict::Match => {}
                    }
                }
                still_needs(needs_process, needs_resolve).unwrap_or(Verdict::Match)
            }),
            Matcher::Or(subs) => ctx.in_branches(|ctx| {
                let (mut needs_process, mut needs_resolve) = (false, false);
                for sub in subs {
                    match sub.eval(s, ctx) {
                        Verdict::Match => return Verdict::Match,
                        Verdict::NeedsProcess => needs_process = true,
                        Verdict::NeedsResolve => needs_resolve = true,
                        Verdict::NoMatch => {}
                    }
                }
                still_needs(needs_process, needs_resolve).unwrap_or(Verdict::NoMatch)
            }),
            Matcher::Not(sub) => {
                // what matched inside a NOT is not what matched the rule
                let found = ctx.sub_hit.take();
                let verdict = sub.eval(s, ctx);
                ctx.sub_hit = found;
                match verdict {
                    Verdict::Match => Verdict::NoMatch,
                    Verdict::NoMatch => Verdict::Match,
                    Verdict::NeedsResolve | Verdict::NeedsProcess => verdict,
                }
            }
            Matcher::Set(set) => {
                if ctx.set_depth >= MAX_NESTING {
                    // Should be unreachable for any graph `SetRegistry` built
                    // (it rejects cycles and depth beyond `MAX_NESTING` at
                    // compile time) — this only guards against a runtime
                    // cycle formed by independent hot reloads (see
                    // `EvalCtx::set_depth`).
                    return Verdict::NoMatch;
                }
                ctx.set_depth += 1;
                let v = set.eval(s, ctx, no_resolve, extended);
                ctx.set_depth -= 1;
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

/// What a rule that none of its parts decided still waits for: the process
/// before the lookup, since the process is found on this machine at once and
/// may make the lookup unnecessary. `None`: it waits for nothing.
pub(crate) fn still_needs(process: bool, resolve: bool) -> Option<Verdict> {
    if process {
        Some(Verdict::NeedsProcess)
    } else if resolve {
        Some(Verdict::NeedsResolve)
    } else {
        None
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
    process_matches_for(cfg!(windows), pattern, p)
}

/// The `PROCESS-NAME` pattern of a rule, made for a host. `rurge-config`
/// compiled its globs from the raw value, to the letter; here a path is
/// rebuilt from the normalized source, so that it compares symmetrically with
/// the normalized path of the process (FR-RULE-01), and on Windows a name is
/// rebuilt not to mind the case of an ASCII letter (phase 3 M2 design 5.3).
fn compile_process_pattern(windows: bool, p: &ProcessPattern) -> ProcessPattern {
    match p {
        ProcessPattern::Path(g) => {
            let normalized = normalize_process_path_for(windows, g.source());
            let opts = GlobOptions {
                case_insensitive: windows,
                classes: false,
            };
            ProcessPattern::Path(Glob::new(&normalized, opts).unwrap_or_else(|_| g.clone()))
        }
        ProcessPattern::Name(g) if windows => {
            let opts = GlobOptions {
                case_insensitive: true,
                classes: false,
            };
            ProcessPattern::Name(Glob::new(g.source(), opts).unwrap_or_else(|_| g.clone()))
        }
        other => other.clone(),
    }
}

fn process_matches_for(windows: bool, pattern: &ProcessPattern, p: &ProcessInfo) -> bool {
    match pattern {
        ProcessPattern::Name(g) => g.matches(&p.name),
        ProcessPattern::Path(g) => p
            .path
            .as_deref()
            .is_some_and(|path| g.matches(&normalize_process_path_for(windows, path))),
        ProcessPattern::Prefix(prefix) => p.path.as_deref().is_some_and(|path| {
            normalize_process_path_for(windows, path)
                .starts_with(&normalize_process_path_for(windows, prefix))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::set::{CompiledSet, SetHandle};
    use crate::set_format::parse_set;
    use rurge_config::HostName;
    use rurge_config::Reach;
    use rurge_config::rule::{ParseCtx, parse_subrule};
    use rurge_config::{NetKind, Primary, WifiLink};
    use std::collections::HashSet;
    use std::path::Path;

    struct NoSets;
    impl SetLookup for NoSets {
        fn lookup(&self, _: &ResourceRef, _: SetKind) -> SetRef {
            panic!("no sets in matcher tests")
        }
    }

    /// A loaded database: it knows the addresses of 8.0.0.0/8 (a country and
    /// an AS number each) and every IPv6 address (a country, no AS number),
    /// and says `Unknown` of all others.
    struct FakeGeo;
    impl GeoLookup for FakeGeo {
        fn country(&self, ip: IpAddr) -> GeoResult<[u8; 2]> {
            match ip {
                IpAddr::V4(v) if v.octets()[0] == 8 => GeoResult::Known(*b"US"),
                IpAddr::V6(_) => GeoResult::Known(*b"JP"),
                _ => GeoResult::Unknown,
            }
        }
        fn asn(&self, ip: IpAddr) -> GeoResult<u32> {
            match ip {
                IpAddr::V4(v) if v.octets()[0] == 8 => GeoResult::Known(15169),
                _ => GeoResult::Unknown,
            }
        }
    }

    fn compile(raw: &str) -> CompiledSubRule {
        let names: HashSet<String> = HashSet::new();
        let ctx = ParseCtx {
            inline_rulesets: &names,
            base_dir: Path::new("."),
            reach: Reach::Anywhere,
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

    /// The network of a test: the primary interface (its name, its kind and
    /// its gateways) and the Wi-Fi link (its interface, SSID and BSSID).
    fn network(
        primary: Option<(&str, NetKind, &[&str])>,
        wifi: Option<(&str, Option<&str>, Option<&str>)>,
    ) -> NetworkFacts {
        NetworkFacts {
            primary: primary.map(|(interface, kind, routers)| Primary {
                interface: interface.to_string(),
                kind,
                routers: routers.iter().map(|r| r.parse().unwrap()).collect(),
            }),
            wifi: wifi.map(|(interface, ssid, bssid)| WifiLink {
                interface: interface.to_string(),
                ssid: ssid.map(str::to_string),
                bssid: bssid.map(str::to_string),
            }),
        }
    }

    /// At home on Wi-Fi: the Wi-Fi link is the way out.
    fn at_home() -> NetworkFacts {
        network(
            Some(("wlan0", NetKind::Wifi, &["192.168.1.1"])),
            Some(("wlan0", Some("Home"), Some("aa:bb:cc:dd:ee:ff"))),
        )
    }

    /// At a desk: the way out is the cable, and the Wi-Fi link is up as well.
    fn at_a_desk_with_wifi_up() -> NetworkFacts {
        network(
            Some(("eth0", NetKind::Wired, &["10.0.0.1"])),
            Some(("wlan0", Some("Home"), Some("aa:bb:cc:dd:ee:ff"))),
        )
    }

    /// `raw` on `facts`, with `subnet-exp-wifi-always-match` as given; a
    /// `SUBNET` rule is supported, so it never leaves a note.
    fn eval_on(raw: &str, facts: &NetworkFacts, wifi_always_match: bool) -> Verdict {
        let mut ctx = EvalCtx::with_network(&NoGeo, facts, wifi_always_match);
        let verdict = compile(raw).eval(&session("example.com"), &mut ctx);
        assert!(ctx.notes.is_empty(), "{raw}: {:?}", ctx.notes);
        verdict
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

    /// `DOMAIN-WILDCARD` (manual, Domain Rules): `*` takes any run of
    /// characters and crosses dots, `?` takes exactly one, `[...]` takes a
    /// class; the case of the pattern does not matter.
    #[test]
    fn domain_wildcard_takes_star_question_mark_and_classes() {
        let wild = |pattern: &str, host: &str| {
            eval(&format!("DOMAIN-WILDCARD,{pattern}"), &session(host)) == Verdict::Match
        };
        assert!(wild("*.example.com", "a.b.example.com"));
        assert!(!wild("*.example.com", "example.com"));
        assert!(wild("cdn?.example.com", "cdn1.example.com"));
        assert!(!wild("cdn?.example.com", "cdn10.example.com"));
        assert!(wild("cdn[0-9].example.com", "cdn7.example.com"));
        assert!(!wild("cdn[0-9].example.com", "cdnx.example.com"));
        assert!(wild("API-*.Example.COM", "api-v2.example.com"));
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

    /// `GEOIP,UNKNOWN` and `IP-ASN,UNKNOWN` match the address a loaded
    /// database has no answer for, and no other; a name is resolved first,
    /// like for the rules beside them.
    #[test]
    fn geoip_unknown_matches_an_address_the_database_does_not_know() {
        for raw in ["GEOIP,UNKNOWN", "IP-ASN,UNKNOWN"] {
            // 127.0.0.1 is in neither half of the database, 8.8.8.8 in both
            assert_eq!(eval(raw, &session("127.0.0.1")), Verdict::Match, "{raw}");
            assert_eq!(eval(raw, &session("8.8.8.8")), Verdict::NoMatch, "{raw}");
            // a name is resolved before it is judged, unless it says not to
            let name = session("example.com");
            assert_eq!(eval(raw, &name), Verdict::NeedsResolve, "{raw}");
            assert_eq!(
                eval(&format!("{raw},no-resolve"), &name),
                Verdict::NoMatch,
                "{raw}"
            );
            assert_eq!(
                eval_resolved(raw, &name, &["127.0.0.1"], &[]),
                Verdict::Match,
                "{raw}"
            );
            assert_eq!(
                eval_resolved(raw, &name, &["8.8.8.8"], &[]),
                Verdict::NoMatch,
                "{raw}"
            );
            // it is judged by its first IPv4 address, else its first IPv6 one
            // (2001:db8::1 has a country but no AS number)
            assert_eq!(
                eval_resolved(raw, &name, &["8.8.8.8"], &["2001:db8::1"]),
                Verdict::NoMatch,
                "{raw}"
            );
        }
        let name = session("example.com");
        assert_eq!(
            eval_resolved("GEOIP,UNKNOWN", &name, &["127.0.0.1"], &["2001:db8::1"]),
            Verdict::Match
        );
        assert_eq!(
            eval_resolved("GEOIP,UNKNOWN", &name, &[], &["2001:db8::1"]),
            Verdict::NoMatch
        );
        assert_eq!(
            eval_resolved("IP-ASN,UNKNOWN", &name, &[], &["2001:db8::1"]),
            Verdict::Match
        );
        // an IPv6 address has a country in this database and no AS number
        assert_eq!(
            eval("GEOIP,UNKNOWN", &session("[2001:db8::1]")),
            Verdict::NoMatch
        );
        assert_eq!(
            eval("IP-ASN,UNKNOWN", &session("[2001:db8::1]")),
            Verdict::Match
        );
        // in a logical rule too
        assert_eq!(
            eval(
                "AND,((GEOIP,UNKNOWN),(IP-ASN,UNKNOWN),(DEST-PORT,443))",
                &session("127.0.0.1")
            ),
            Verdict::Match
        );
        assert_eq!(
            eval("OR,((GEOIP,UNKNOWN),(IP-ASN,UNKNOWN))", &session("8.8.8.8")),
            Verdict::NoMatch
        );
        assert_eq!(
            eval("NOT,((GEOIP,UNKNOWN))", &session("8.8.8.8")),
            Verdict::Match
        );
    }

    /// While no database is loaded nothing that depends on it matches: not
    /// `UNKNOWN`, which is for the address a loaded database has no answer
    /// for, and not a country or a number either.
    #[test]
    fn unknown_does_not_match_while_the_database_is_not_loaded() {
        for raw in ["GEOIP,UNKNOWN", "IP-ASN,UNKNOWN", "GEOIP,CN", "IP-ASN,1"] {
            for host in ["127.0.0.1", "8.8.8.8", "[2001:db8::1]"] {
                let mut ctx = EvalCtx::new(&NoGeo);
                assert_eq!(
                    compile(raw).eval(&session(host), &mut ctx),
                    Verdict::NoMatch,
                    "{raw} on {host}"
                );
            }
            // a name that has been resolved is judged by its address all the same
            let mut ctx = EvalCtx::new(&NoGeo);
            ctx.resolved = Some(ResolvedAddrs {
                v4: vec!["127.0.0.1".parse().unwrap()],
                v6: vec![],
            });
            assert_eq!(
                compile(raw).eval(&session("example.com"), &mut ctx),
                Verdict::NoMatch,
                "{raw}"
            );
        }
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

    /// A pattern is searched for in the whole URL: it has to be anchored to
    /// be tied to its start.
    #[test]
    fn url_regex_without_an_anchor_matches_anywhere_in_the_url() {
        let mut s = session("h.test");
        s.url = Some("http://h.test/x/ads/y".into());
        assert_eq!(eval("URL-REGEX,/ads/", &s), Verdict::Match);
        assert_eq!(
            eval("URL-REGEX,^http://h\\.test/ads/", &s),
            Verdict::NoMatch
        );
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
        assert_eq!(eval("PROTOCOL,TCP", &s), Verdict::Match);
        assert_eq!(eval("PROTOCOL,UDP", &s), Verdict::NoMatch);
        let mut quic = SessionInfo::udp(HostName::parse("example.com"), 443);
        quic.protocol = Some(ProtocolKind::Quic);
        assert_eq!(eval("PROTOCOL,QUIC", &quic), Verdict::Match);
        assert_eq!(eval("PROTOCOL,UDP", &quic), Verdict::Match);
        assert_eq!(eval("PROTOCOL,TCP", &quic), Verdict::NoMatch);
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

    /// `raw` on `s`, in an evaluation that has asked for the process and been
    /// told `found` (`None`: it could not be found).
    fn eval_asked(raw: &str, s: &SessionInfo, found: Option<ProcessInfo>) -> Verdict {
        let mut ctx = EvalCtx::new(&FakeGeo);
        ctx.process = ProcessState::Asked(found);
        compile(raw).eval(s, &mut ctx)
    }

    /// The process a rule like `PROCESS-NAME,<value>` reads, as it is parsed,
    /// before it is compiled for a host.
    fn pattern_of(raw: &str) -> ProcessPattern {
        let names: HashSet<String> = HashSet::new();
        let ctx = ParseCtx {
            inline_rulesets: &names,
            base_dir: Path::new("."),
            reach: Reach::Anywhere,
        };
        match parse_subrule(raw, &ctx)
            .unwrap_or_else(|e| panic!("{raw}: {}", e.message))
            .kind
        {
            RuleKind::ProcessName(pattern) => pattern,
            other => panic!("{raw}: {other:?}"),
        }
    }

    /// The set `text` is the file of, asked by a rule that says nothing of
    /// `no-resolve`, in an evaluation that has not asked for the process.
    fn eval_set(text: &str, s: &SessionInfo) -> Verdict {
        let names: HashSet<String> = HashSet::new();
        let ctx = ParseCtx {
            inline_rulesets: &names,
            base_dir: Path::new("."),
            reach: Reach::Anywhere,
        };
        let parsed = parse_set(SetKind::RuleSet, text, &ctx);
        let set = CompiledSet::compile("t", SetKind::RuleSet, &parsed, &NoSets, 1);
        let mut ctx = EvalCtx::new(&FakeGeo);
        Matcher::Set(Arc::new(SetHandle::new(set))).eval(s, &mut ctx, false, false)
    }

    /// A session carries the process its caller knew of, or the evaluation
    /// has to ask for it: until it has asked, a `PROCESS-NAME` rule can only
    /// say that it needs the process, and once it has, a process that could
    /// not be found is no match.
    #[test]
    fn process_name_modes() {
        let mut s = session("example.com");
        s.process = Some(ProcessInfo {
            pid: None,
            name: "curl".into(),
            path: Some("/usr/bin/curl".into()),
        });
        assert_eq!(eval("PROCESS-NAME,curl", &s), Verdict::Match);
        assert_eq!(eval("PROCESS-NAME,wget", &s), Verdict::NoMatch);
        assert_eq!(eval("PROCESS-NAME,/usr/bin/curl", &s), Verdict::Match);
        assert_eq!(eval("PROCESS-NAME,/usr/bin/*", &s), Verdict::Match);
        // no process, and the context has not asked for one
        assert_eq!(
            eval("PROCESS-NAME,curl", &session("example.com")),
            Verdict::NeedsProcess
        );
        // it has, and the process is not known
        assert_eq!(
            eval_asked("PROCESS-NAME,curl", &session("example.com"), None),
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

    /// What the evaluation found is read when the session names no process of
    /// its own; one it does name is read first, asked for or not.
    #[test]
    fn the_process_a_session_names_is_read_before_the_one_that_was_found() {
        let found = ProcessInfo {
            pid: Some(41),
            name: "wget".into(),
            path: Some("/usr/bin/wget".into()),
        };
        let bare = session("example.com");
        for (raw, expected) in [
            ("PROCESS-NAME,wget", Verdict::Match),
            ("PROCESS-NAME,/usr/bin/wget", Verdict::Match),
            ("PROCESS-NAME,/usr/bin/", Verdict::Match),
            ("PROCESS-NAME,curl", Verdict::NoMatch),
        ] {
            assert_eq!(
                eval_asked(raw, &bare, Some(found.clone())),
                expected,
                "{raw}"
            );
        }
        let mut named = session("example.com");
        named.process = Some(ProcessInfo {
            pid: None,
            name: "curl".into(),
            path: None,
        });
        assert_eq!(eval("PROCESS-NAME,curl", &named), Verdict::Match);
        assert_eq!(
            eval_asked("PROCESS-NAME,curl", &named, Some(found.clone())),
            Verdict::Match
        );
        assert_eq!(
            eval_asked("PROCESS-NAME,wget", &named, Some(found)),
            Verdict::NoMatch
        );
    }

    /// `NeedsProcess` goes through the logical rules and the sets the way
    /// `NeedsResolve` does: a rule that cannot be decided yet says it needs
    /// the process, one that has been decided already (an `AND` with a miss,
    /// an `OR` or a set with a hit) does not, and a `NOT` hands it on.
    #[test]
    fn needing_the_process_passes_through_logic_and_sets() {
        let at = |raw: &str, host: &str| eval(raw, &session(host));
        let and = "AND,((PROCESS-NAME,x),(DOMAIN,a.test))";
        let or = "OR,((PROCESS-NAME,x),(DOMAIN,a.test))";
        assert_eq!(at(and, "a.test"), Verdict::NeedsProcess);
        assert_eq!(at(and, "b.test"), Verdict::NoMatch);
        assert_eq!(at(or, "a.test"), Verdict::Match);
        assert_eq!(at(or, "b.test"), Verdict::NeedsProcess);
        assert_eq!(
            at("NOT,((PROCESS-NAME,x))", "a.test"),
            Verdict::NeedsProcess
        );
        // the other order of the halves, and a rule inside a rule
        let and = "AND,((DOMAIN,a.test),(PROCESS-NAME,x))";
        let or = "OR,((DOMAIN,a.test),(PROCESS-NAME,x))";
        assert_eq!(at(and, "a.test"), Verdict::NeedsProcess);
        assert_eq!(at(and, "b.test"), Verdict::NoMatch);
        assert_eq!(at(or, "a.test"), Verdict::Match);
        assert_eq!(at(or, "b.test"), Verdict::NeedsProcess);
        assert_eq!(
            at("AND,((NOT,((PROCESS-NAME,x))),(DOMAIN,a.test))", "a.test"),
            Verdict::NeedsProcess
        );
        // a set: a line that matches ends it, wherever the process line stands;
        // with nothing else to say it needs the process
        let a_test = session("a.test");
        assert_eq!(
            eval_set("PROCESS-NAME,x\nDOMAIN-KEYWORD,test\n", &a_test),
            Verdict::Match
        );
        assert_eq!(
            eval_set("DOMAIN-KEYWORD,test\nPROCESS-NAME,x\n", &a_test),
            Verdict::Match
        );
        assert_eq!(eval_set("PROCESS-NAME,x\n", &a_test), Verdict::NeedsProcess);
        assert_eq!(
            eval_set("PROCESS-NAME,x\nDOMAIN-KEYWORD,other\n", &a_test),
            Verdict::NeedsProcess
        );
        assert_eq!(eval_set("DOMAIN,b.test\n", &a_test), Verdict::NoMatch);
        // once the evaluation has asked, the rules decide
        let x = || ProcessInfo {
            pid: Some(5),
            name: "x".into(),
            path: None,
        };
        let asked = |raw: &str, host: &str, found: Option<ProcessInfo>| {
            eval_asked(raw, &session(host), found)
        };
        let and = "AND,((PROCESS-NAME,x),(DOMAIN,a.test))";
        assert_eq!(asked(and, "a.test", Some(x())), Verdict::Match);
        assert_eq!(asked(and, "b.test", Some(x())), Verdict::NoMatch);
        assert_eq!(asked(and, "a.test", None), Verdict::NoMatch);
        assert_eq!(
            asked("NOT,((PROCESS-NAME,x))", "a.test", Some(x())),
            Verdict::NoMatch
        );
        assert_eq!(
            asked("NOT,((PROCESS-NAME,x))", "a.test", None),
            Verdict::Match
        );
    }

    /// When a rule is undecided for want of both, the process comes first:
    /// it is found on this machine in a moment, and it may make the lookup
    /// unnecessary.
    #[test]
    fn needing_the_process_comes_before_needing_a_lookup() {
        let name = session("example.com");
        for raw in [
            "AND,((IP-CIDR,10.0.0.0/8),(PROCESS-NAME,x))",
            "OR,((IP-CIDR,10.0.0.0/8),(PROCESS-NAME,x))",
            "AND,((PROCESS-NAME,x),(IP-CIDR,10.0.0.0/8))",
            "OR,((PROCESS-NAME,x),(IP-CIDR,10.0.0.0/8))",
        ] {
            assert_eq!(eval(raw, &name), Verdict::NeedsProcess, "{raw}");
        }
        // a set with a line of each kind: the same
        assert_eq!(
            eval_set("IP-CIDR,10.0.0.0/8\nPROCESS-NAME,x\n", &name),
            Verdict::NeedsProcess
        );
        assert_eq!(
            eval_set("PROCESS-NAME,x\nIP-CIDR,10.0.0.0/8\n", &name),
            Verdict::NeedsProcess
        );
        // and a set that needs a lookup and nothing else still does
        assert_eq!(
            eval_set("IP-CIDR,10.0.0.0/8\n", &name),
            Verdict::NeedsResolve
        );
        // the process asked for and not x: an AND is decided without the
        // lookup, an OR still needs it
        assert_eq!(
            eval_asked("AND,((IP-CIDR,10.0.0.0/8),(PROCESS-NAME,x))", &name, None),
            Verdict::NoMatch
        );
        assert_eq!(
            eval_asked("OR,((IP-CIDR,10.0.0.0/8),(PROCESS-NAME,x))", &name, None),
            Verdict::NeedsResolve
        );
    }

    /// `PROCESS-NAME` is compared the way the host's file system compares
    /// (phase 3 M2 design 5.3): on Windows without regard to the case of an
    /// ASCII letter and with `/` for `\`, elsewhere to the letter. `.exe` is
    /// never dropped from a name.
    #[test]
    fn process_comparison_follows_the_platform() {
        let process = |name: &str, path: &str| ProcessInfo {
            pid: Some(3),
            name: name.into(),
            path: Some(path.into()),
        };
        // `raw` compiled and then compared as `windows` says
        let hits = |windows: bool, raw: &str, p: &ProcessInfo| {
            let pattern = compile_process_pattern(windows, &pattern_of(raw));
            process_matches_for(windows, &pattern, p)
        };
        let chrome = process("chrome.exe", "C:\\Program Files\\Chrome\\chrome.exe");
        let app = process("app.exe", "C:\\Program Files\\App\\app.exe");
        // Windows: not the case of a letter, and `/` is `\`
        assert!(hits(true, "PROCESS-NAME,CHROME.EXE", &chrome));
        assert!(hits(true, "PROCESS-NAME,chrome.exe", &chrome));
        assert!(hits(true, "PROCESS-NAME,Chrome*", &chrome));
        assert!(hits(
            true,
            "PROCESS-NAME,chrome.exe",
            &process("CHROME.EXE", "C:\\c.exe")
        ));
        assert!(!hits(true, "PROCESS-NAME,firefox.exe", &chrome));
        assert!(hits(
            true,
            "PROCESS-NAME,c:/program files/app/APP.EXE",
            &app
        ));
        assert!(hits(
            true,
            "PROCESS-NAME,C:\\Program Files\\App\\app.exe",
            &process("APP.EXE", "c:/PROGRAM FILES/app/App.exe")
        ));
        assert!(hits(
            true,
            "PROCESS-NAME,C:\\Program Files\\*\\app.exe",
            &app
        ));
        assert!(hits(true, "PROCESS-NAME,C:\\PROGRAM FILES\\", &app));
        assert!(hits(true, "PROCESS-NAME,c:/program files/app/", &app));
        assert!(!hits(true, "PROCESS-NAME,C:\\Program Files\\Other\\", &app));
        // elsewhere: the letter counts, in a name, a path and a prefix
        let curl = process("curl", "/usr/bin/curl");
        assert!(hits(false, "PROCESS-NAME,curl", &curl));
        assert!(!hits(false, "PROCESS-NAME,Curl", &curl));
        assert!(!hits(
            false,
            "PROCESS-NAME,curl",
            &process("Curl", "/usr/bin/Curl")
        ));
        assert!(hits(false, "PROCESS-NAME,/usr/bin/curl", &curl));
        assert!(!hits(false, "PROCESS-NAME,/usr/bin/Curl", &curl));
        assert!(!hits(false, "PROCESS-NAME,/USR/bin/curl", &curl));
        assert!(hits(false, "PROCESS-NAME,/usr/bin/", &curl));
        assert!(!hits(false, "PROCESS-NAME,/USR/BIN/", &curl));
        // a path is compared as it is written: nothing is folded or turned around
        assert!(!hits(
            false,
            "PROCESS-NAME,c:/program files/app/APP.EXE",
            &app
        ));
        assert!(hits(
            false,
            "PROCESS-NAME,C:\\Program Files\\App\\app.exe",
            &app
        ));
        // `.exe` is part of the name on both
        for windows in [true, false] {
            assert!(!hits(windows, "PROCESS-NAME,chrome", &chrome), "{windows}");
            assert!(hits(windows, "PROCESS-NAME,chrome*", &chrome), "{windows}");
        }
    }

    /// A process whose path could not be read (Linux gives a name only, so
    /// does Windows for `System`) is matched by its name and by nothing that
    /// says where it lives.
    #[test]
    fn a_process_without_a_path_matches_by_name_only() {
        let app = ProcessInfo {
            pid: Some(7),
            name: "app".into(),
            path: None,
        };
        let asked = |raw: &str| eval_asked(raw, &session("example.com"), Some(app.clone()));
        assert_eq!(asked("PROCESS-NAME,app"), Verdict::Match);
        assert_eq!(asked("PROCESS-NAME,a*"), Verdict::Match);
        assert_eq!(asked("PROCESS-NAME,/usr/bin/app"), Verdict::NoMatch);
        assert_eq!(asked("PROCESS-NAME,/usr/bin/"), Verdict::NoMatch);
        assert_eq!(
            asked("PROCESS-NAME,C:\\Program Files\\App\\app.exe"),
            Verdict::NoMatch
        );
        assert_eq!(asked("PROCESS-NAME,C:\\Program Files\\"), Verdict::NoMatch);
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
            compile("DEVICE-NAME,Kids-iPad").eval(&s, &mut ctx),
            Verdict::NoMatch
        );
        assert_eq!(ctx.notes.len(), 1);
        assert!(ctx.notes[0].contains("DEVICE-NAME"));
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

    /// `CELLULAR-RADIO` and `CELLULAR-CARRIER` are iOS rules (manual,
    /// Protocol and Network Rules): on a desktop neither ever matches, and
    /// neither leaves a note at run time that it cannot, as a `DEVICE-NAME`
    /// rule does.
    #[test]
    fn the_cellular_rules_never_match_and_say_nothing() {
        for raw in [
            "CELLULAR-RADIO,LTE",
            "CELLULAR-RADIO,NR",
            "CELLULAR-CARRIER,310260",
        ] {
            let mut ctx = EvalCtx::new(&NoGeo);
            assert_eq!(
                compile(raw).eval(&session("example.com"), &mut ctx),
                Verdict::NoMatch,
                "{raw}"
            );
            assert!(ctx.notes.is_empty(), "{raw}: {:?}", ctx.notes);
        }
    }

    /// A context that was never told the network — a `[Host]` key, a rule
    /// set read in a test — has nothing for a `SUBNET` rule to look at: no
    /// match, and no note either, because the rule is supported.
    #[test]
    fn a_context_without_a_network_matches_no_subnet_rule() {
        for raw in [
            "SUBNET,TYPE:WIFI",
            "SUBNET,TYPE:WIRED",
            "SUBNET,SSID:*",
            "SUBNET,BSSID:*",
            "SUBNET,ROUTER:192.168.1.1",
            "SUBNET,Home",
        ] {
            let mut ctx = EvalCtx::new(&NoGeo);
            assert_eq!(
                compile(raw).eval(&session("example.com"), &mut ctx),
                Verdict::NoMatch,
                "{raw}"
            );
            assert!(ctx.notes.is_empty(), "{raw}: {:?}", ctx.notes);
        }
    }

    /// The conditions are read off the network the context carries.
    #[test]
    fn subnet_rules_are_evaluated_on_the_network_of_the_context() {
        let home = at_home();
        for (raw, expected) in [
            ("SUBNET,SSID:Home", Verdict::Match),
            ("SUBNET,SSID:Office", Verdict::NoMatch),
            ("SUBNET,BSSID:AA:BB:*", Verdict::Match),
            ("SUBNET,ROUTER:192.168.1.1", Verdict::Match),
            ("SUBNET,ROUTER:192.168.1.2", Verdict::NoMatch),
            ("SUBNET,TYPE:WIFI", Verdict::Match),
            ("SUBNET,TYPE:WIRED", Verdict::NoMatch),
            ("SUBNET,Home", Verdict::Match),
        ] {
            assert_eq!(eval_on(raw, &home, true), expected, "{raw}");
        }
        // the cable: only the conditions on the way out change
        let desk = at_a_desk_with_wifi_up();
        assert_eq!(eval_on("SUBNET,TYPE:WIRED", &desk, true), Verdict::Match);
        assert_eq!(eval_on("SUBNET,TYPE:WIFI", &desk, true), Verdict::NoMatch);
        assert_eq!(
            eval_on("SUBNET,ROUTER:10.0.0.1", &desk, true),
            Verdict::Match
        );
        assert_eq!(
            eval_on("SUBNET,ROUTER:192.168.1.1", &desk, true),
            Verdict::NoMatch
        );
    }

    /// `subnet-exp-wifi-always-match` travels with the network: a Wi-Fi link
    /// that is not the way out counts for the SSID and the BSSID only while
    /// it is on, and not at all for what is read off the way out.
    #[test]
    fn the_context_carries_subnet_exp_wifi_always_match() {
        let desk = at_a_desk_with_wifi_up();
        for raw in ["SUBNET,SSID:Home", "SUBNET,BSSID:aa:bb:*", "SUBNET,Home"] {
            assert_eq!(eval_on(raw, &desk, true), Verdict::Match, "{raw}");
            assert_eq!(eval_on(raw, &desk, false), Verdict::NoMatch, "{raw}");
        }
        assert_eq!(eval_on("SUBNET,TYPE:WIRED", &desk, false), Verdict::Match);
        assert_eq!(
            eval_on("SUBNET,ROUTER:10.0.0.1", &desk, false),
            Verdict::Match
        );
        // on Wi-Fi the link is the way out, whichever way the switch is set
        let home = at_home();
        assert_eq!(eval_on("SUBNET,SSID:Home", &home, false), Verdict::Match);
    }

    /// A platform that would not say the SSID or the BSSID (no permission to
    /// see them) leaves those two conditions unmatched and the others alone.
    #[test]
    fn an_ssid_the_platform_would_not_say_matches_nothing() {
        let hidden = network(
            Some(("wlan0", NetKind::Wifi, &["192.168.1.1"])),
            Some(("wlan0", None, None)),
        );
        for raw in ["SUBNET,SSID:*", "SUBNET,BSSID:*", "SUBNET,Home"] {
            assert_eq!(eval_on(raw, &hidden, true), Verdict::NoMatch, "{raw}");
        }
        assert_eq!(eval_on("SUBNET,TYPE:WIFI", &hidden, true), Verdict::Match);
        assert_eq!(
            eval_on("SUBNET,ROUTER:192.168.1.1", &hidden, true),
            Verdict::Match
        );
    }

    /// The logical rules ask the same context.
    #[test]
    fn subnet_rules_take_part_in_logical_rules() {
        let home = at_home();
        for (raw, expected) in [
            (
                "AND,((SUBNET,TYPE:WIFI),(DOMAIN-SUFFIX,example.com))",
                Verdict::Match,
            ),
            (
                "AND,((SUBNET,TYPE:WIRED),(DOMAIN-SUFFIX,example.com))",
                Verdict::NoMatch,
            ),
            (
                "OR,((SUBNET,TYPE:WIRED),(SUBNET,SSID:Home))",
                Verdict::Match,
            ),
            ("NOT,((SUBNET,TYPE:WIRED))", Verdict::Match),
            ("NOT,((SUBNET,TYPE:WIFI))", Verdict::NoMatch),
        ] {
            assert_eq!(eval_on(raw, &home, true), expected, "{raw}");
        }
    }

    #[test]
    fn geo_lookups_are_cached_per_ip() {
        // `Cell` is not Sync; the trait requires Send + Sync, so wrap the counter.
        struct SyncCounting(std::sync::Mutex<u32>, std::sync::Mutex<u32>);
        impl GeoLookup for SyncCounting {
            fn country(&self, _: IpAddr) -> GeoResult<[u8; 2]> {
                *self.0.lock().unwrap() += 1;
                GeoResult::Known(*b"US")
            }
            fn asn(&self, _: IpAddr) -> GeoResult<u32> {
                *self.1.lock().unwrap() += 1;
                GeoResult::Unknown
            }
        }
        let geo = SyncCounting(std::sync::Mutex::new(0), std::sync::Mutex::new(0));
        let mut ctx = EvalCtx::new(&geo);
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        assert_eq!(ctx.country(ip), GeoResult::Known(*b"US"));
        assert_eq!(ctx.country(ip), GeoResult::Known(*b"US"));
        assert_eq!(*geo.0.lock().unwrap(), 1);
        // what is not known is kept too
        assert_eq!(ctx.asn(ip), GeoResult::Unknown);
        assert_eq!(ctx.asn(ip), GeoResult::Unknown);
        assert_eq!(*geo.1.lock().unwrap(), 1);
    }
}
