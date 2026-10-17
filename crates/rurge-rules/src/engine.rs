//! Rule engine (M2 design §6.5): top-level rules evaluated in order — after
//! the ones flagged `pre-matching`, which are checked first and never resolve —,
//! DNS on demand through `LazyResolver`, the process of the session on demand
//! through `LazyProcess`, FINAL / `dns-failed` fallbacks, per-rule hit counters
//! and one-time runtime notes.

use crate::matcher::{
    EvalCtx, GeoLookup, Matcher, ProcessState, ResolvedAddrs, SetLookup, SubRuleHit, Verdict,
};
use crate::pre_matching::{PreMatch, PreMatchingSet};
use crate::registry::SetRegistry;
use rurge_config::policy::Builtin;
use rurge_config::rule::{PolicyRef, RuleParams};
use rurge_config::session::{ProcessInfo, SessionInfo};
use rurge_config::{Config, HostName, NetworkCell, NetworkFacts, Span};
use rurge_net::BoxFuture;
use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    Timeout,
    EmptyAnswer,
    Failed(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::Timeout => f.write_str("timeout"),
            ResolveError::EmptyAnswer => f.write_str("empty answer"),
            ResolveError::Failed(s) => f.write_str(s),
        }
    }
}

/// Resolves the destination the first time a rule needs an IP.
pub trait LazyResolver: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<ResolvedAddrs, ResolveError>>;
}

/// DNS disabled: every lookup fails (`rule match --no-dns`).
pub struct NoResolve;

impl LazyResolver for NoResolve {
    fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, Result<ResolvedAddrs, ResolveError>> {
        Box::pin(async { Err(ResolveError::Failed("DNS disabled".to_string())) })
    }
}

/// The same answer for every name (tests, `rule match --resolve`).
pub struct FixedResolve(pub ResolvedAddrs);

impl LazyResolver for FixedResolve {
    fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, Result<ResolvedAddrs, ResolveError>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
}

/// Finds the process that opened the session the first time a rule needs to
/// know it. `None`: it could not be found, which no `PROCESS-NAME` rule
/// matches.
pub trait LazyProcess: Send + Sync {
    fn process(&self) -> BoxFuture<'_, Option<ProcessInfo>>;
}

/// No process lookup: the process is never known (`evaluate`, `rule match`
/// without `--process`).
pub struct NoProcess;

impl LazyProcess for NoProcess {
    fn process(&self) -> BoxFuture<'_, Option<ProcessInfo>> {
        Box::pin(async { None })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutboundMode {
    Direct,
    Proxy(PolicyRef),
    Rule,
}

pub struct CompiledRule {
    /// Index into `Config.rules`.
    pub index: usize,
    pub matcher: Matcher,
    pub policy: PolicyRef,
    pub params: RuleParams,
    pub raw: String,
    pub span: Span,
    hits: AtomicU64,
    warned: AtomicBool,
}

impl CompiledRule {
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Policy(PolicyRef),
    /// Resolution failed and FINAL has no `dns-failed`: the session must fail.
    DnsFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    OutboundModeDirect,
    OutboundModeProxy,
    Rule,
    Final,
    DnsFailedFallback,
    DnsFailed,
    /// A rule flagged `pre-matching` matched ahead of the written order.
    PreMatching,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::OutboundModeDirect => "outbound-mode-direct",
            Reason::OutboundModeProxy => "outbound-mode-proxy",
            Reason::Rule => "rule",
            Reason::Final => "final",
            Reason::DnsFailedFallback => "dns-failed-fallback",
            Reason::DnsFailed => "dns-failed",
            Reason::PreMatching => "pre-matching",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Decision {
    pub outcome: Outcome,
    pub reason: Reason,
    pub matched: Option<usize>,
    pub sub_rule: Option<SubRuleHit>,
    pub resolved: Option<ResolvedAddrs>,
    pub notes: Vec<String>,
}

impl Decision {
    fn bypass(policy: PolicyRef, reason: Reason) -> Decision {
        Decision {
            outcome: Outcome::Policy(policy),
            reason,
            matched: None,
            sub_rule: None,
            resolved: None,
            notes: Vec::new(),
        }
    }

    pub fn policy(&self) -> Option<&PolicyRef> {
        match &self.outcome {
            Outcome::Policy(p) => Some(p),
            Outcome::DnsFailed => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TraceStep {
    pub rule: usize,
    pub verdict: &'static str,
    pub elapsed: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildError(pub String);

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BuildError {}

pub struct RuleEngine {
    rules: Vec<CompiledRule>,
    final_pos: usize,
    geo: Arc<dyn GeoLookup>,
    /// The network now, which `SUBNET` rules look at (phase 3 M1 design 8).
    /// Unknown until `with_network` hands the engine a cell somebody keeps
    /// up to date.
    network: Arc<NetworkCell>,
    /// `[General] subnet-exp-wifi-always-match` of the profile built from.
    wifi_always_match: bool,
    pre: PreMatchingSet,
    /// Keep-alive only (design §6.5): a `SetRegistry`'s hot-reload tasks hold
    /// only a `Weak` back-reference, so if nothing else keeps the `Arc` the
    /// registry passed to `build` alive, reload stops within 60 s even
    /// though the engine still holds compiled `SetHandle`s that would
    /// otherwise be fine. `None` when built via `build` from a bare
    /// `&dyn SetLookup`; set when built via `build_with_registry`.
    registry: Option<Arc<SetRegistry>>,
}

fn verdict_name(v: Verdict) -> &'static str {
    match v {
        Verdict::Match => "match",
        Verdict::NoMatch => "no-match",
        Verdict::NeedsResolve => "needs-resolve",
        Verdict::NeedsProcess => "needs-process",
    }
}

impl RuleEngine {
    /// Compiles `[Rule]` up to the first FINAL, where matching ends, and ends
    /// the table with the effective (last) FINAL, which gives the policy and
    /// the options; every rule written after the first FINAL is dropped, the
    /// other FINAL lines among them (the config already warned).
    ///
    /// `sets` is consulted only while compiling. If it wraps a `SetRegistry`
    /// whose external sets hot-reload (URL / local-file resources), the
    /// caller must keep that registry's `Arc` alive itself — the engine
    /// holds only the compiled `SetHandle`s, not the registry, so a dropped
    /// registry's background reload tasks notice within 60 s and exit, and
    /// those sets stop updating. Use `build_with_registry` to have the
    /// engine hold the `Arc` for you instead.
    pub fn build(
        cfg: &Config,
        sets: &dyn SetLookup,
        geo: Arc<dyn GeoLookup>,
    ) -> Result<RuleEngine, BuildError> {
        let (first, last) = cfg
            .first_final()
            .zip(cfg.effective_final())
            .ok_or_else(|| BuildError("the [Rule] section has no FINAL rule".to_string()))?;
        let kept = cfg.rules[..first]
            .iter()
            .enumerate()
            .chain(std::iter::once((last, &cfg.rules[last])));
        let mut rules = Vec::with_capacity(first + 1);
        for (i, rule) in kept {
            rules.push(CompiledRule {
                index: i,
                matcher: Matcher::compile(&rule.kind, sets),
                policy: rule.policy.clone(),
                params: rule.params.clone(),
                raw: rule.raw.clone(),
                span: rule.span.clone(),
                hits: AtomicU64::new(0),
                warned: AtomicBool::new(false),
            });
        }
        let final_pos = rules.len() - 1;
        let pre = PreMatchingSet::extract(&rules);
        Ok(RuleEngine {
            rules,
            final_pos,
            geo,
            network: Arc::new(NetworkCell::new(NetworkFacts::default())),
            wifi_always_match: cfg.general.subnet_exp_wifi_always_match,
            pre,
            registry: None,
        })
    }

    /// Makes the `SUBNET` rules look at `network` instead of an unknown one.
    /// The engine reads the cell once per evaluation, so a network stored in
    /// it applies from the next session on; the cell outlives the engine, and
    /// every generation of a running profile is given the same one.
    pub fn with_network(mut self, network: Arc<NetworkCell>) -> RuleEngine {
        self.network = network;
        self
    }

    /// Like `build`, but keeps `registry` alive for as long as the engine is
    /// (see `build`'s doc comment) — use this whenever the caller already
    /// owns the `SetRegistry` it is building against, which is the common
    /// case.
    pub fn build_with_registry(
        cfg: &Config,
        registry: Arc<SetRegistry>,
        geo: Arc<dyn GeoLookup>,
    ) -> Result<RuleEngine, BuildError> {
        let mut engine = Self::build(cfg, registry.as_ref(), geo)?;
        engine.registry = Some(registry);
        Ok(engine)
    }

    /// The registry kept alive by `build_with_registry`; `None` if the
    /// engine was built via `build` instead.
    pub fn registry(&self) -> Option<&Arc<SetRegistry>> {
        self.registry.as_ref()
    }

    pub fn rules(&self) -> &[CompiledRule] {
        &self.rules
    }

    pub fn final_rule(&self) -> &CompiledRule {
        &self.rules[self.final_pos]
    }

    pub fn pre_matching(&self) -> &PreMatchingSet {
        &self.pre
    }

    /// Decides `s` knowing no process but the one `s` carries.
    pub async fn evaluate(
        &self,
        s: &SessionInfo,
        mode: OutboundMode,
        resolver: &dyn LazyResolver,
    ) -> Decision {
        self.run(s, mode, resolver, &NoProcess, None).await
    }

    /// `evaluate`, finding the process that opened the session with `process`
    /// when a rule needs it: at most once per evaluation, and before a name is
    /// resolved for the same rule.
    pub async fn evaluate_with_process(
        &self,
        s: &SessionInfo,
        mode: OutboundMode,
        resolver: &dyn LazyResolver,
        process: &dyn LazyProcess,
    ) -> Decision {
        self.run(s, mode, resolver, process, None).await
    }

    pub async fn evaluate_traced(
        &self,
        s: &SessionInfo,
        mode: OutboundMode,
        resolver: &dyn LazyResolver,
    ) -> (Decision, Vec<TraceStep>) {
        let mut trace = Vec::new();
        let d = self
            .run(s, mode, resolver, &NoProcess, Some(&mut trace))
            .await;
        (d, trace)
    }

    async fn run(
        &self,
        s: &SessionInfo,
        mode: OutboundMode,
        resolver: &dyn LazyResolver,
        process: &dyn LazyProcess,
        mut trace: Option<&mut Vec<TraceStep>>,
    ) -> Decision {
        match mode {
            OutboundMode::Direct => {
                return Decision::bypass(
                    PolicyRef::Builtin(Builtin::Direct),
                    Reason::OutboundModeDirect,
                );
            }
            OutboundMode::Proxy(p) => return Decision::bypass(p, Reason::OutboundModeProxy),
            OutboundMode::Rule => {}
        }
        // one snapshot for the whole evaluation: a lookup in flight when the
        // network changes finishes on the network it started on
        let facts = self.network.load();
        let mut ctx = EvalCtx::with_network(self.geo.as_ref(), &facts, self.wifi_always_match);
        let mut notes: Vec<String> = Vec::new();
        if let Some(decision) = self.pre_pass(s, &mut ctx, &mut notes, trace.as_deref_mut()) {
            return decision;
        }
        for (pos, rule) in self.rules.iter().enumerate() {
            let started = Instant::now();
            ctx.sub_hit = None;
            let mut verdict = self.eval_rule(rule, s, &mut ctx);
            if verdict == Verdict::NeedsProcess {
                // asked for once: the rules after this one read the answer
                ctx.process = ProcessState::Asked(process.process().await);
                verdict = self.eval_rule(rule, s, &mut ctx);
            }
            if verdict == Verdict::NeedsResolve {
                let host = s.dst_host.as_domain().unwrap_or_default().to_string();
                match resolver.resolve(&host).await {
                    Ok(addrs) => {
                        ctx.resolved = Some(addrs);
                        verdict = self.eval_rule(rule, s, &mut ctx);
                    }
                    Err(e) => {
                        notes.push(format!("DNS lookup for {host} failed: {e}"));
                        Self::collect_notes(rule, &mut ctx, &mut notes);
                        if let Some(t) = trace.as_mut() {
                            t.push(TraceStep {
                                rule: rule.index,
                                verdict: "dns-failed",
                                elapsed: started.elapsed(),
                            });
                        }
                        return self.dns_failed(rule, ctx, notes);
                    }
                }
            }
            Self::collect_notes(rule, &mut ctx, &mut notes);
            if let Some(t) = trace.as_mut() {
                t.push(TraceStep {
                    rule: rule.index,
                    verdict: verdict_name(verdict),
                    elapsed: started.elapsed(),
                });
            }
            if verdict == Verdict::Match {
                rule.hits.fetch_add(1, Ordering::Relaxed);
                return Decision {
                    outcome: Outcome::Policy(rule.policy.clone()),
                    reason: if pos == self.final_pos {
                        Reason::Final
                    } else {
                        Reason::Rule
                    },
                    matched: Some(rule.index),
                    sub_rule: ctx.sub_hit.take(),
                    resolved: ctx.resolved.take(),
                    notes,
                };
            }
        }
        // FINAL always matches; kept for completeness.
        let f = self.final_rule();
        Decision {
            outcome: Outcome::Policy(f.policy.clone()),
            reason: Reason::Final,
            matched: Some(f.index),
            sub_rule: None,
            resolved: ctx.resolved.take(),
            notes,
        }
    }

    fn eval_rule(&self, rule: &CompiledRule, s: &SessionInfo, ctx: &mut EvalCtx<'_>) -> Verdict {
        rule.matcher.eval(
            s,
            ctx,
            rule.params.no_resolve,
            rule.params.extended_matching,
        )
    }

    /// Rules flagged `pre-matching` have the highest priority: each is
    /// checked once, ahead of the written order and without resolving, and
    /// the first that matches decides. A rule that misses here is not
    /// dropped — it is evaluated again where it is written, which is where
    /// an IP rule that needs a lookup can still match.
    fn pre_pass(
        &self,
        s: &SessionInfo,
        ctx: &mut EvalCtx<'_>,
        notes: &mut Vec<String>,
        mut trace: Option<&mut Vec<TraceStep>>,
    ) -> Option<Decision> {
        for &pos in self.pre.positions() {
            let rule = &self.rules[pos];
            let started = Instant::now();
            ctx.sub_hit = None;
            let matched = Self::pre_matches(rule, s, ctx);
            Self::collect_notes(rule, ctx, notes);
            if !matched {
                continue;
            }
            if let Some(t) = trace.as_mut() {
                t.push(TraceStep {
                    rule: rule.index,
                    verdict: "pre-match",
                    elapsed: started.elapsed(),
                });
            }
            rule.hits.fetch_add(1, Ordering::Relaxed);
            return Some(Decision {
                outcome: Outcome::Policy(rule.policy.clone()),
                reason: Reason::PreMatching,
                matched: Some(rule.index),
                sub_rule: ctx.sub_hit.take(),
                resolved: None,
                notes: std::mem::take(notes),
            });
        }
        None
    }

    /// Whether a flagged rule pre-matches: it is evaluated as if it had
    /// `no-resolve` (an IP rule that needs a lookup misses), and only a
    /// match of a REJECT-family rule counts.
    fn pre_matches(rule: &CompiledRule, s: &SessionInfo, ctx: &mut EvalCtx<'_>) -> bool {
        let verdict = rule
            .matcher
            .eval(s, ctx, true, rule.params.extended_matching);
        verdict == Verdict::Match && matches!(&rule.policy, PolicyRef::Builtin(b) if b.is_reject())
    }

    /// Runtime notes are surfaced once per rule (and logged once).
    fn collect_notes(rule: &CompiledRule, ctx: &mut EvalCtx<'_>, notes: &mut Vec<String>) {
        if ctx.notes.is_empty() {
            return;
        }
        if rule.warned.swap(true, Ordering::Relaxed) {
            ctx.notes.clear();
            return;
        }
        for n in ctx.notes.drain(..) {
            tracing::warn!(rule = rule.index, "{n}");
            notes.push(n);
        }
    }

    fn dns_failed(
        &self,
        rule: &CompiledRule,
        mut ctx: EvalCtx<'_>,
        notes: Vec<String>,
    ) -> Decision {
        let f = self.final_rule();
        if f.params.dns_failed {
            f.hits.fetch_add(1, Ordering::Relaxed);
            Decision {
                outcome: Outcome::Policy(f.policy.clone()),
                reason: Reason::DnsFailedFallback,
                matched: Some(f.index),
                sub_rule: None,
                resolved: ctx.resolved.take(),
                notes,
            }
        } else {
            Decision {
                outcome: Outcome::DnsFailed,
                reason: Reason::DnsFailed,
                matched: Some(rule.index),
                sub_rule: None,
                resolved: None,
                notes,
            }
        }
    }

    /// Pre-matching never resolves: every rule is evaluated as if it had `no-resolve`.
    pub fn pre_match_domain(&self, host: &str, port: u16) -> Option<PreMatch> {
        self.pre_match(SessionInfo::tcp(
            HostName::Domain(host.trim_end_matches('.').to_ascii_lowercase()),
            port,
        ))
    }

    pub fn pre_match_ip(&self, ip: IpAddr, port: u16) -> Option<PreMatch> {
        self.pre_match(SessionInfo::tcp(HostName::Ip(ip), port))
    }

    fn pre_match(&self, s: SessionInfo) -> Option<PreMatch> {
        let facts = self.network.load();
        let mut ctx = EvalCtx::with_network(self.geo.as_ref(), &facts, self.wifi_always_match);
        for &pos in self.pre.positions() {
            let rule = &self.rules[pos];
            let matched = Self::pre_matches(rule, &s, &mut ctx);
            ctx.notes.clear();
            if matched {
                return Some(PreMatch {
                    rule: rule.index,
                    policy: rule.policy.clone(),
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geoip::{COUNTRY_FILE, DbKind, GeoDb};
    use crate::matcher::{GeoResult, NoGeo, SetRef};
    use crate::set::{CompiledSet, SetHandle};
    use crate::set_format::{ParsedSet, SetKind, SetLine, internal_set_source, parse_set};
    use rurge_config::Reach;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_config::rule::{ParseCtx, ResourceRef};
    use rurge_config::{NetKind, Primary, WifiLink};
    use std::collections::{HashMap, HashSet};
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;

    fn load(text: &str) -> Config {
        let l = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        let codes: Vec<&str> = l.diagnostics.iter().map(|d| d.code).collect();
        assert!(!l.diagnostics.has_errors(), "{codes:?}");
        l.config
    }

    /// Inline `[Ruleset]` sections + internal sets; nothing else.
    struct InlineSets(HashMap<String, SetHandle>);

    struct NoNested;
    impl SetLookup for NoNested {
        fn lookup(&self, _: &ResourceRef, _: SetKind) -> SetRef {
            panic!("nested sets are not used in engine tests")
        }
    }

    impl InlineSets {
        fn from_config(cfg: &Config) -> InlineSets {
            let mut map = HashMap::new();
            for rs in &cfg.rulesets {
                let parsed = ParsedSet {
                    lines: rs.rules.iter().cloned().map(SetLine::Rule).collect(),
                    ..ParsedSet::default()
                };
                map.insert(
                    rs.name.clone(),
                    SetHandle::new(CompiledSet::compile(
                        &rs.name,
                        SetKind::RuleSet,
                        &parsed,
                        &NoNested,
                        1,
                    )),
                );
            }
            InlineSets(map)
        }
    }

    impl SetLookup for InlineSets {
        fn lookup(&self, r: &ResourceRef, kind: SetKind) -> SetRef {
            match r {
                ResourceRef::Inline(n) => Arc::new(self.0[n].clone()),
                ResourceRef::Internal(i) => {
                    let names: HashSet<String> = HashSet::new();
                    let ctx = ParseCtx {
                        inline_rulesets: &names,
                        base_dir: Path::new("."),
                        reach: Reach::Anywhere,
                    };
                    let text = internal_set_source(*i, cfg!(target_os = "macos"));
                    let parsed = parse_set(SetKind::RuleSet, &text, &ctx);
                    Arc::new(SetHandle::new(CompiledSet::compile(
                        "internal",
                        SetKind::RuleSet,
                        &parsed,
                        &NoNested,
                        1,
                    )))
                }
                _ => Arc::new(SetHandle::new(CompiledSet::empty("missing", kind))),
            }
        }
    }

    struct Counting {
        calls: AtomicUsize,
        answer: ResolvedAddrs,
    }
    impl Counting {
        fn v4(ip: &str) -> Counting {
            Counting {
                calls: AtomicUsize::new(0),
                answer: ResolvedAddrs {
                    v4: vec![ip.parse().unwrap()],
                    v6: vec![],
                },
            }
        }
    }
    impl LazyResolver for Counting {
        fn resolve<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<ResolvedAddrs, ResolveError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(self.answer.clone()) })
        }
    }

    struct Panicking;
    impl LazyResolver for Panicking {
        fn resolve<'a>(
            &'a self,
            host: &'a str,
        ) -> BoxFuture<'a, Result<ResolvedAddrs, ResolveError>> {
            panic!("unexpected DNS lookup for {host}")
        }
    }

    const CONF: &str = "\
[Proxy]
P = direct
[Rule]
DOMAIN-SUFFIX,apple.com,DIRECT
IP-CIDR,10.0.0.0/8,P
RULE-SET,Inline,REJECT
GEOIP,US,P
DOMAIN,pre.example.com,REJECT,pre-matching
IP-CIDR,192.168.0.0/16,REJECT-DROP,pre-matching
FINAL,P,dns-failed
[Ruleset Inline]
DOMAIN,inline.example.com
";

    fn engine(text: &str) -> RuleEngine {
        let cfg = load(text);
        let sets = InlineSets::from_config(&cfg);
        RuleEngine::build(&cfg, &sets, Arc::new(NoGeo)).unwrap()
    }

    fn session(host: &str) -> SessionInfo {
        SessionInfo::tcp(HostName::parse(host), 443)
    }

    fn named(p: &PolicyRef) -> String {
        p.name()
    }

    #[tokio::test]
    async fn outbound_mode_bypasses_rules_and_dns() {
        let e = engine(CONF);
        let d = e
            .evaluate(&session("foo.org"), OutboundMode::Direct, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::OutboundModeDirect);
        assert_eq!(d.policy().map(named).as_deref(), Some("DIRECT"));
        let d = e
            .evaluate(
                &session("foo.org"),
                OutboundMode::Proxy(PolicyRef::Named("P".into())),
                &Panicking,
            )
            .await;
        assert_eq!(d.reason, Reason::OutboundModeProxy);
        assert_eq!(d.policy().map(named).as_deref(), Some("P"));
        assert!(d.matched.is_none());
    }

    #[tokio::test]
    async fn domain_rules_match_without_dns() {
        let e = engine(CONF);
        let d = e
            .evaluate(&session("www.apple.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::Rule);
        assert_eq!(d.matched, Some(0));
        assert_eq!(d.policy().map(named).as_deref(), Some("DIRECT"));
        assert!(d.resolved.is_none());
    }

    #[tokio::test]
    async fn ip_rules_resolve_once_and_hand_back_the_addresses() {
        let e = engine(CONF);
        let r = Counting::v4("10.1.1.1");
        let d = e
            .evaluate(&session("foo.org"), OutboundMode::Rule, &r)
            .await;
        assert_eq!(d.matched, Some(1));
        assert_eq!(d.policy().map(named).as_deref(), Some("P"));
        assert_eq!(r.calls.load(Ordering::SeqCst), 1);
        assert_eq!(d.resolved.as_ref().map(|a| a.v4.len()), Some(1));
        let r = Counting::v4("1.2.3.4");
        let d = e
            .evaluate(&session("foo.org"), OutboundMode::Rule, &r)
            .await;
        assert_eq!(d.reason, Reason::Final);
        assert_eq!(d.matched, Some(6));
        assert_eq!(r.calls.load(Ordering::SeqCst), 1);
    }

    /// The same two networks in the rules of the profile: host bits set, and
    /// upper-case hexadecimal digits.
    #[tokio::test]
    async fn a_rule_network_with_host_bits_or_upper_case_hexadecimal_is_the_network_of_its_prefix()
    {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\nIP-CIDR,192.168.1.5/16,P,no-resolve\nIP-CIDR6,2001:DB8::/32,P,no-resolve\nFINAL,DIRECT\n",
        );
        for (host, rule) in [
            ("192.168.200.1", 0),
            ("192.168.0.0", 0),
            ("192.168.255.255", 0),
            ("[2001:db8::1]", 1),
            ("[2001:db8:ffff::]", 1),
        ] {
            let d = e
                .evaluate(&session(host), OutboundMode::Rule, &Panicking)
                .await;
            assert_eq!(d.reason, Reason::Rule, "{host}");
            assert_eq!(d.matched, Some(rule), "{host}");
            assert_eq!(d.policy().map(named).as_deref(), Some("P"), "{host}");
        }
        for host in [
            "192.167.255.255",
            "192.169.0.0",
            "[2001:db7::1]",
            "[2001:db9::]",
        ] {
            let d = e
                .evaluate(&session(host), OutboundMode::Rule, &Panicking)
                .await;
            assert_eq!(d.reason, Reason::Final, "{host}");
        }
    }

    #[tokio::test]
    async fn dns_failure_falls_back_to_final_with_dns_failed() {
        let e = engine(CONF);
        let d = e
            .evaluate(&session("foo.org"), OutboundMode::Rule, &NoResolve)
            .await;
        assert_eq!(d.reason, Reason::DnsFailedFallback);
        assert_eq!(d.matched, Some(6));
        assert_eq!(d.policy().map(named).as_deref(), Some("P"));
        assert!(
            d.notes
                .iter()
                .any(|n| n.contains("DNS lookup for foo.org failed"))
        );
    }

    #[tokio::test]
    async fn dns_failure_without_dns_failed_is_fatal() {
        let e = engine("[Proxy]\nP = direct\n[Rule]\nIP-CIDR,10.0.0.0/8,P\nFINAL,P\n");
        let d = e
            .evaluate(&session("foo.org"), OutboundMode::Rule, &NoResolve)
            .await;
        assert_eq!(d.outcome, Outcome::DnsFailed);
        assert_eq!(d.reason, Reason::DnsFailed);
        assert_eq!(d.matched, Some(0));
        assert!(d.policy().is_none());
    }

    #[tokio::test]
    async fn inline_set_hit_reports_the_sub_rule() {
        let e = engine(CONF);
        // Rule 1 (`IP-CIDR,10.0.0.0/8,P`) has no `no-resolve`, so it resolves
        // "inline.example.com" first; answer outside 10.0.0.0/8 so it falls
        // through to rule 2's RULE-SET match.
        let r = Counting::v4("1.2.3.4");
        let d = e
            .evaluate(&session("inline.example.com"), OutboundMode::Rule, &r)
            .await;
        assert_eq!(r.calls.load(Ordering::SeqCst), 1);
        assert_eq!(d.matched, Some(2));
        assert_eq!(d.policy().map(named).as_deref(), Some("REJECT"));
        let hit = d.sub_rule.expect("sub rule");
        assert_eq!(hit.set, "Inline");
        assert_eq!(hit.entry, "DOMAIN,inline.example.com");
    }

    /// The manual: "In the [Rule] section, a FINAL rule immediately terminates
    /// rule matching", and "If several enabled FINAL rules are present, the
    /// last one is the effective one." What is written between two FINALs is
    /// never evaluated, and the host it names falls to the last FINAL.
    #[tokio::test]
    async fn matching_ends_at_the_first_final_and_the_last_one_gives_the_policy() {
        let e = engine("[Proxy]\nP = direct\n[Rule]\nFINAL,DIRECT\nDOMAIN,a.com,P\nFINAL,P\n");
        let d = e
            .evaluate(&session("a.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::Final);
        assert_eq!(d.matched, Some(2));
        assert_eq!(d.policy().map(named).as_deref(), Some("P"));
        let d = e
            .evaluate(&session("b.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::Final);
        assert_eq!(d.matched, Some(2));
        assert_eq!(d.policy().map(named).as_deref(), Some("P"));
        // the table holds the FINAL that gives the policy and nothing else
        assert_eq!(e.rules().len(), 1);
        assert_eq!(e.final_rule().index, 2);
    }

    /// A rule in every stretch around three FINALs. `first` and `third` are
    /// put after the policy of the first and of the third FINAL: `,dns-failed`
    /// or nothing.
    fn three_finals(first: &str, third: &str) -> RuleEngine {
        engine(&format!(
            "[Proxy]\nP = direct\nQ = direct\nR = direct\n[Rule]\n\
DOMAIN,early.example,P\nIP-CIDR,10.0.0.0/8,P\nFINAL,Q{first}\nDOMAIN,between.example,Q\n\
FINAL,R\nDOMAIN,later.example,R\nFINAL,DIRECT{third}\nDOMAIN,tail.example,P\n"
        ))
    }

    #[tokio::test]
    async fn with_three_finals_only_the_rules_before_the_first_are_evaluated() {
        let e = three_finals("", "");
        let d = e
            .evaluate(&session("early.example"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(0)));
        for host in [
            "between.example",
            "later.example",
            "tail.example",
            "any.org",
        ] {
            // the lookup the IP rule asks for finds an address outside its network
            let d = e
                .evaluate(&session(host), OutboundMode::Rule, &Counting::v4("1.2.3.4"))
                .await;
            assert_eq!((d.reason, d.matched), (Reason::Final, Some(6)), "{host}");
            assert_eq!(d.policy().map(named).as_deref(), Some("DIRECT"), "{host}");
        }
        // the two rules before the first FINAL, then the third FINAL
        let indexes: Vec<usize> = e.rules().iter().map(|r| r.index).collect();
        assert_eq!(indexes, [0, 1, 6]);
        assert_eq!(e.final_rule().index, 6);
    }

    /// What a failed lookup does is the option of the last FINAL, whatever the
    /// first one says.
    #[tokio::test]
    async fn the_last_finals_dns_failed_decides_what_a_failed_lookup_does() {
        let e = three_finals(",dns-failed", "");
        let d = e
            .evaluate(&session("between.example"), OutboundMode::Rule, &NoResolve)
            .await;
        assert_eq!(d.outcome, Outcome::DnsFailed);
        assert_eq!(d.reason, Reason::DnsFailed);
        let e = three_finals("", ",dns-failed");
        let d = e
            .evaluate(&session("between.example"), OutboundMode::Rule, &NoResolve)
            .await;
        assert_eq!(d.reason, Reason::DnsFailedFallback);
        assert_eq!(d.matched, Some(6));
        assert_eq!(d.policy().map(named).as_deref(), Some("DIRECT"));
    }

    /// A rule flagged `pre-matching` that is written after the first FINAL is
    /// as dead as any other there: neither the pre-matching table nor the
    /// pre-pass of an evaluation has it.
    #[tokio::test]
    async fn a_pre_matching_rule_after_the_first_final_is_never_pre_matched() {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\n\
DOMAIN,before.example,REJECT,pre-matching\nFINAL,DIRECT\n\
DOMAIN,between.example,REJECT,pre-matching\nFINAL,P\n\
DOMAIN,after.example,REJECT,pre-matching\n",
        );
        let m = e
            .pre_match_domain("before.example", 443)
            .expect("the rule before the first FINAL");
        assert_eq!(m.rule, 0);
        assert!(e.pre_match_domain("between.example", 443).is_none());
        assert!(e.pre_match_domain("after.example", 443).is_none());
        let d = e
            .evaluate(&session("before.example"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::PreMatching, Some(0)));
        for host in ["between.example", "after.example"] {
            let d = e
                .evaluate(&session(host), OutboundMode::Rule, &Panicking)
                .await;
            assert_eq!((d.reason, d.matched), (Reason::Final, Some(3)), "{host}");
            assert_eq!(d.policy().map(named).as_deref(), Some("P"), "{host}");
        }
        assert_eq!(e.pre_matching().len(), 1);
    }

    /// What the manual warns about: a file included into `[Rule]` ends with a
    /// FINAL of its own and the profile goes on after it. The rules of the
    /// profile that follow the include are never evaluated, and the policy is
    /// that of the profile's own FINAL.
    #[tokio::test]
    async fn an_included_final_ends_matching_for_the_rules_that_follow_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("main.conf"),
            "[Proxy]\nP = direct\n[Rule]\nDOMAIN,first.example,DIRECT\n#!include shared.dconf\n\
DOMAIN,local.example,P\nFINAL,P\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("shared.dconf"),
            "[Rule]\nDOMAIN,included.example,DIRECT\nFINAL,DIRECT\n",
        )
        .unwrap();
        let loaded =
            rurge_config::config::load(&dir.path().join("main.conf"), &LoadOptions::for_tests())
                .unwrap();
        assert!(!loaded.diagnostics.has_errors());
        let cfg = loaded.config;
        let e = RuleEngine::build(&cfg, &InlineSets::from_config(&cfg), Arc::new(NoGeo)).unwrap();
        for (host, matched, policy) in [
            ("first.example", 0, "DIRECT"),
            ("included.example", 1, "DIRECT"),
        ] {
            let d = e
                .evaluate(&session(host), OutboundMode::Rule, &Panicking)
                .await;
            assert_eq!(
                (d.reason, d.matched),
                (Reason::Rule, Some(matched)),
                "{host}"
            );
            assert_eq!(d.policy().map(named).as_deref(), Some(policy), "{host}");
        }
        for host in ["local.example", "other.example"] {
            let d = e
                .evaluate(&session(host), OutboundMode::Rule, &Panicking)
                .await;
            assert_eq!((d.reason, d.matched), (Reason::Final, Some(4)), "{host}");
            assert_eq!(d.policy().map(named).as_deref(), Some("P"), "{host}");
        }
        let indexes: Vec<usize> = e.rules().iter().map(|r| r.index).collect();
        assert_eq!(indexes, [0, 1, 4]);
    }

    #[tokio::test]
    async fn rules_after_the_last_final_are_excluded() {
        let e = engine("[Proxy]\nP = direct\n[Rule]\nFINAL,DIRECT\nDOMAIN,a.com,P\n");
        assert_eq!(e.rules().len(), 1);
        let d = e
            .evaluate(&session("a.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::Final);
        assert_eq!(d.policy().map(named).as_deref(), Some("DIRECT"));
    }

    #[tokio::test]
    async fn unsupported_kinds_note_once_and_count_hits() {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\nDEVICE-NAME,Kids-iPad,P\nDOMAIN-SUFFIX,apple.com,DIRECT\nFINAL,P\n",
        );
        let d = e
            .evaluate(&session("www.apple.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.notes.len(), 1);
        assert!(d.notes[0].contains("DEVICE-NAME"));
        let d = e
            .evaluate(&session("www.apple.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert!(d.notes.is_empty());
        assert_eq!(e.rules()[0].hits(), 0);
        assert_eq!(e.rules()[1].hits(), 2);
    }

    #[tokio::test]
    async fn traced_evaluation_lists_every_step() {
        let e = engine(CONF);
        let (d, trace) = e
            .evaluate_traced(
                &session("foo.org"),
                OutboundMode::Rule,
                &Counting::v4("10.9.9.9"),
            )
            .await;
        assert_eq!(d.matched, Some(1));
        let steps: Vec<(usize, &str)> = trace.iter().map(|t| (t.rule, t.verdict)).collect();
        assert_eq!(steps, vec![(0, "no-match"), (1, "match")]);
        let (d, trace) = e
            .evaluate_traced(&session("foo.org"), OutboundMode::Rule, &NoResolve)
            .await;
        assert_eq!(d.reason, Reason::DnsFailedFallback);
        assert_eq!(trace.last().map(|t| t.verdict), Some("dns-failed"));
    }

    #[tokio::test]
    async fn extended_matching_flag_reaches_the_matcher() {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\nDOMAIN-SUFFIX,ext.com,P,extended-matching\nFINAL,DIRECT\n",
        );
        let mut s = session("1.2.3.4");
        s.sni = Some("x.ext.com".into());
        let d = e.evaluate(&s, OutboundMode::Rule, &Panicking).await;
        assert_eq!(d.matched, Some(0));
    }

    #[test]
    fn pre_matching_extracts_reject_rules_and_never_resolves() {
        let e = engine(CONF);
        assert_eq!(e.pre_matching().len(), 2);
        let m = e
            .pre_match_domain("PRE.example.com.", 443)
            .expect("domain pre-match");
        assert_eq!(m.rule, 4);
        assert_eq!(m.policy.name(), "REJECT");
        let m = e
            .pre_match_ip("192.168.1.1".parse().unwrap(), 80)
            .expect("ip pre-match");
        assert_eq!(m.rule, 5);
        assert_eq!(m.policy.name(), "REJECT-DROP");
        assert!(e.pre_match_domain("other.com", 443).is_none());
        assert!(e.pre_match_ip("10.0.0.1".parse().unwrap(), 80).is_none());
    }

    /// The allowing rule is written first; the flagged ones still go first.
    const PRE_CONF: &str = "\
[Proxy]
P = direct
[Rule]
DOMAIN,both.example,P
DOMAIN,both.example,REJECT,pre-matching
RULE-SET,Listed,REJECT,pre-matching
DOMAIN-SUFFIX,late.example,P
IP-CIDR,10.0.0.0/8,REJECT,pre-matching
FINAL,DIRECT
[Ruleset Listed]
DOMAIN,listed.example
";

    fn steps(trace: &[TraceStep]) -> Vec<(usize, &'static str)> {
        trace.iter().map(|t| (t.rule, t.verdict)).collect()
    }

    #[tokio::test]
    async fn a_pre_matching_rule_decides_before_the_written_order() {
        let e = engine(PRE_CONF);
        let d = e
            .evaluate(&session("both.example"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::PreMatching);
        assert_eq!(d.reason.as_str(), "pre-matching");
        assert_eq!(d.matched, Some(1));
        assert_eq!(d.policy().map(named).as_deref(), Some("REJECT"));
        assert!(d.resolved.is_none());
        assert_eq!(e.rules()[0].hits(), 0);
        assert_eq!(e.rules()[1].hits(), 1);
    }

    #[tokio::test]
    async fn a_pre_matching_rule_set_reports_the_line_it_matched() {
        let e = engine(PRE_CONF);
        let d = e
            .evaluate(&session("listed.example"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::PreMatching, Some(2)));
        let hit = d.sub_rule.expect("sub rule");
        assert_eq!(hit.set, "Listed");
        assert_eq!(hit.entry, "DOMAIN,listed.example");
    }

    /// A flagged IP rule misses in the pre-pass (which never resolves) and
    /// is evaluated again where it is written: one lookup, one hit.
    #[tokio::test]
    async fn a_pre_matching_ip_rule_only_resolves_at_its_own_place() {
        let e = engine(PRE_CONF);
        // `late.example` is decided by rule 3, before the lookup rule 4
        // would ask for: had the pre-pass resolved, `Panicking` would fire
        let d = e
            .evaluate(&session("late.example"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(3)));
        let r = Counting::v4("10.1.1.1");
        let d = e
            .evaluate(&session("foo.org"), OutboundMode::Rule, &r)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(4)));
        assert_eq!(d.policy().map(named).as_deref(), Some("REJECT"));
        assert_eq!(r.calls.load(Ordering::SeqCst), 1);
        assert!(d.resolved.is_some());
        assert_eq!(e.rules()[4].hits(), 1);
    }

    /// A logical rule's sub-rules carry their own `no-resolve`: one that
    /// needs a lookup is a miss for the pre-pass, not a reason to resolve.
    #[tokio::test]
    async fn a_pre_matching_logical_rule_with_a_lookup_in_it_misses_without_resolving() {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\nDOMAIN,early.example,P\n\
AND,((DOMAIN-SUFFIX,example),(IP-CIDR,10.0.0.0/8)),REJECT,pre-matching\nFINAL,DIRECT\n",
        );
        let d = e
            .evaluate(&session("early.example"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(0)));
        let r = Counting::v4("10.1.1.1");
        let d = e
            .evaluate(&session("other.example"), OutboundMode::Rule, &r)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(1)));
        assert_eq!(r.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn direct_and_global_modes_do_not_look_at_pre_matching() {
        let e = engine(PRE_CONF);
        let d = e
            .evaluate(&session("both.example"), OutboundMode::Direct, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::OutboundModeDirect);
        assert_eq!(d.policy().map(named).as_deref(), Some("DIRECT"));
        let d = e
            .evaluate(
                &session("both.example"),
                OutboundMode::Proxy(PolicyRef::Named("P".into())),
                &Panicking,
            )
            .await;
        assert_eq!(d.reason, Reason::OutboundModeProxy);
        assert_eq!(d.policy().map(named).as_deref(), Some("P"));
        assert_eq!(e.rules()[1].hits(), 0);
    }

    /// A set that is not there yet is an empty set: no match, no error, and
    /// no session rejected for it.
    #[tokio::test]
    async fn a_pre_matching_rule_whose_set_is_empty_matches_nothing() {
        // `InlineSets` answers a file it does not have with an empty set
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\nDOMAIN,a.example,P\n\
RULE-SET,not-there.list,REJECT,pre-matching\nFINAL,DIRECT\n",
        );
        let d = e
            .evaluate(&session("a.example"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(0)));
        let d = e
            .evaluate(&session("b.example"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::Final);
        assert_eq!(d.policy().map(named).as_deref(), Some("DIRECT"));
        assert_eq!(e.rules()[1].hits(), 0);
    }

    #[tokio::test]
    async fn the_trace_has_one_step_for_a_pre_match_and_none_for_a_miss() {
        let e = engine(PRE_CONF);
        let (d, trace) = e
            .evaluate_traced(&session("both.example"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::PreMatching);
        assert_eq!(steps(&trace), [(1, "pre-match")]);
        // a miss adds nothing: the written order is walked as it always was
        let (d, trace) = e
            .evaluate_traced(&session("late.example"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.matched, Some(3));
        assert_eq!(
            steps(&trace),
            [
                (0, "no-match"),
                (1, "no-match"),
                (2, "no-match"),
                (3, "match")
            ]
        );
        // and a flagged rule that matches at its own place is listed once
        let (_, trace) = e
            .evaluate_traced(
                &session("foo.org"),
                OutboundMode::Rule,
                &Counting::v4("10.1.1.1"),
            )
            .await;
        assert_eq!(trace.iter().filter(|t| t.rule == 4).count(), 1);
        assert_eq!(steps(&trace).last(), Some(&(4, "match")));
    }

    /// A rule without runtime support notes it — once, whichever pass meets
    /// the rule first, and the note reaches the decision. No rule type that
    /// may be flagged `pre-matching` is without runtime support any more, so
    /// the test makes one: the first rule's matcher is swapped for an
    /// unsupported kind.
    #[tokio::test]
    async fn a_note_of_the_pre_pass_is_surfaced_once_and_reaches_the_decision() {
        const NOTING: &str = "[Proxy]\nP = direct\n[Rule]\n\
DOMAIN,never.example,REJECT,pre-matching\nDOMAIN,ads.example,REJECT,pre-matching\n\
DOMAIN-SUFFIX,apple.com,DIRECT\nFINAL,P\n";
        let noting = || {
            let mut e = engine(NOTING);
            e.rules[0].matcher = Matcher::Unsupported("DEVICE-NAME");
            e
        };
        // a hit of the pre-pass, after the rule that noted
        let e = noting();
        let d = e
            .evaluate(&session("ads.example"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::PreMatching);
        assert_eq!(d.notes.len(), 1, "{:?}", d.notes);
        assert!(d.notes[0].contains("DEVICE-NAME"));
        let d = e
            .evaluate(&session("www.apple.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::Rule);
        assert!(d.notes.is_empty(), "{:?}", d.notes);
        // and a pass without a hit: the note is not said again at the rule's own place
        let e = noting();
        let d = e
            .evaluate(&session("www.apple.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.notes.len(), 1, "{:?}", d.notes);
        assert_eq!(e.rules()[2].hits(), 1);
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

    /// The rule a session to `www.example.com` ends at, and how.
    async fn decided(e: &RuleEngine) -> (Reason, Option<usize>) {
        let d = e
            .evaluate(&session("www.example.com"), OutboundMode::Rule, &Panicking)
            .await;
        (d.reason, d.matched)
    }

    /// An engine nobody gave a network to knows none: its `SUBNET` rules have
    /// nothing to look at, and they are no unsupported kind that says so.
    #[tokio::test]
    async fn an_engine_that_was_never_told_the_network_knows_none() {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\nSUBNET,TYPE:WIFI,P\nSUBNET,TYPE:WIRED,P\nFINAL,DIRECT\n",
        );
        let d = e
            .evaluate(&session("www.example.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Final, Some(2)));
        assert!(d.notes.is_empty(), "{:?}", d.notes);
        assert_eq!(e.rules()[0].hits(), 0);
    }

    /// The rules look at what the engine's cell holds when a session is
    /// evaluated, not at what it held when the engine was built: the same
    /// engine, another network, another rule.
    #[tokio::test]
    async fn subnet_rules_follow_the_network() {
        let cell = Arc::new(NetworkCell::new(NetworkFacts::default()));
        let e = engine(
            "[Proxy]\nA = direct\nB = direct\nC = direct\n[Rule]\n\
SUBNET,TYPE:WIFI,A\nSUBNET,ROUTER:192.168.1.1,B\nSUBNET,SSID:Home,C\nFINAL,DIRECT\n",
        )
        .with_network(cell.clone());
        let picked = || async {
            let d = e
                .evaluate(&session("www.example.com"), OutboundMode::Rule, &Panicking)
                .await;
            (d.matched, d.policy().map(named).unwrap())
        };
        let direct = (Some(3), "DIRECT".to_string());

        // nothing is known yet
        assert_eq!(picked().await, direct);
        // on Wi-Fi at home: the first rule
        cell.store(at_home());
        assert_eq!(picked().await, (Some(0), "A".to_string()));
        // on a cable behind the same gateway, no Wi-Fi link: the second
        cell.store(network(
            Some(("eth0", NetKind::Wired, &["192.168.1.1"])),
            None,
        ));
        assert_eq!(picked().await, (Some(1), "B".to_string()));
        // on another cable with the Wi-Fi link up: the SSID, which is the third
        cell.store(at_a_desk_with_wifi_up());
        assert_eq!(picked().await, (Some(2), "C".to_string()));
        // the platform would not say the SSID: that rule does not match, and
        // nothing complains
        cell.store(network(
            Some(("eth0", NetKind::Wired, &["10.0.0.1"])),
            Some(("wlan0", None, None)),
        ));
        assert_eq!(picked().await, direct);
        // and off every network again
        cell.store(at_home());
        assert_eq!(picked().await, (Some(0), "A".to_string()));
        cell.store(NetworkFacts::default());
        assert_eq!(picked().await, direct);
        // the hits were counted where they happened
        let hits: Vec<u64> = e.rules().iter().map(|r| r.hits()).collect();
        assert_eq!(hits, [2, 1, 1, 3]);
    }

    /// `subnet-exp-wifi-always-match` is read off the profile the engine is
    /// built from: with it off, a Wi-Fi link that is not the way out does not
    /// count for the SSID.
    #[tokio::test]
    async fn subnet_exp_wifi_always_match_reaches_the_rules() {
        const RULES: &str = "[Proxy]\nP = direct\n[Rule]\nSUBNET,SSID:Home,P\nFINAL,DIRECT\n";
        let cell = Arc::new(NetworkCell::new(at_a_desk_with_wifi_up()));
        let always = engine(RULES).with_network(cell.clone());
        let off = engine(&format!(
            "[General]\nsubnet-exp-wifi-always-match = false\n{RULES}"
        ))
        .with_network(cell.clone());
        // the Wi-Fi link is up, but the way out is the cable
        assert_eq!(decided(&always).await, (Reason::Rule, Some(0)));
        assert_eq!(decided(&off).await, (Reason::Final, Some(1)));
        // on Wi-Fi it is the way out, and both match
        cell.store(at_home());
        assert_eq!(decided(&always).await, (Reason::Rule, Some(0)));
        assert_eq!(decided(&off).await, (Reason::Rule, Some(0)));
    }

    /// A rule flagged `pre-matching` is checked ahead of the written order,
    /// and it looks at the network too — in the evaluation of a session and in
    /// the two entries that ask before a name is resolved.
    #[tokio::test]
    async fn a_pre_matching_subnet_rule_sees_the_network() {
        let cell = Arc::new(NetworkCell::new(NetworkFacts::default()));
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\nDOMAIN,ads.example,P\n\
SUBNET,TYPE:WIFI,REJECT,pre-matching\nFINAL,DIRECT\n",
        )
        .with_network(cell.clone());
        let ads = || async {
            let d = e
                .evaluate(&session("ads.example"), OutboundMode::Rule, &Panicking)
                .await;
            (d.reason, d.matched, d.policy().map(named).unwrap())
        };
        // off Wi-Fi the flagged rule misses and the written order is walked
        assert_eq!(ads().await, (Reason::Rule, Some(0), "P".to_string()));
        assert_eq!(e.pre_match_domain("ads.example", 443), None);
        assert_eq!(e.pre_match_ip("192.0.2.1".parse().unwrap(), 443), None);
        // on Wi-Fi it decides first
        cell.store(at_home());
        assert_eq!(
            ads().await,
            (Reason::PreMatching, Some(1), "REJECT".to_string())
        );
        let m = e
            .pre_match_domain("ads.example", 443)
            .expect("the pre-match sees the network");
        assert_eq!((m.rule, m.policy.name()), (1, "REJECT".to_string()));
        let m = e
            .pre_match_ip("192.0.2.1".parse().unwrap(), 443)
            .expect("the pre-match sees the network");
        assert_eq!(m.rule, 1);
        // and off it again
        cell.store(NetworkFacts::default());
        assert_eq!(ads().await, (Reason::Rule, Some(0), "P".to_string()));
        assert_eq!(e.pre_match_domain("ads.example", 443), None);
    }

    /// Answers 192.0.2.1 and, on the way, the machine joins another network.
    struct Moving {
        cell: Arc<NetworkCell>,
        to: NetworkFacts,
    }

    impl LazyResolver for Moving {
        fn resolve<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<ResolvedAddrs, ResolveError>> {
            self.cell.store(self.to.clone());
            Box::pin(async {
                Ok(ResolvedAddrs {
                    v4: vec!["192.0.2.1".parse().unwrap()],
                    v6: vec![],
                })
            })
        }
    }

    /// An evaluation takes the network once, when it starts: one that is
    /// waiting for a lookup when the machine changes networks finishes on the
    /// network it started on, and the next session starts on the new one.
    #[tokio::test]
    async fn an_evaluation_in_flight_keeps_the_network_it_started_on() {
        let cell = Arc::new(NetworkCell::new(NetworkFacts::default()));
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\nIP-CIDR,10.0.0.0/8,P\nSUBNET,TYPE:WIFI,REJECT\nFINAL,DIRECT\n",
        )
        .with_network(cell.clone());
        let lookup = Moving {
            cell: cell.clone(),
            to: at_home(),
        };
        let d = e
            .evaluate(&session("www.example.com"), OutboundMode::Rule, &lookup)
            .await;
        // the lookup moved the machine onto Wi-Fi, which this session did not see
        assert_eq!(*cell.load(), at_home());
        assert_eq!((d.reason, d.matched), (Reason::Final, Some(2)));
        // the next one does
        let d = e
            .evaluate(&session("www.example.com"), OutboundMode::Rule, &lookup)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(1)));
    }

    /// `X` lists `hit.example`; the logical rules below are written so that
    /// the set matches while the rule, or the branch the set stands in, does
    /// not.
    const HIT_CONF: &str = "\
[Proxy]
P = direct
[Rule]
AND,((RULE-SET,X),(DEST-PORT,1)),REJECT
DOMAIN,hit.example,P
FINAL,DIRECT
[Ruleset X]
DOMAIN,hit.example
";

    async fn sub_rule_of(e: &RuleEngine, host: &str) -> (Option<usize>, Option<(String, String)>) {
        let d = e
            .evaluate(&session(host), OutboundMode::Rule, &NoResolve)
            .await;
        (d.matched, d.sub_rule.map(|h| (h.set, h.entry)))
    }

    fn hit(set: &str, entry: &str) -> Option<(String, String)> {
        Some((set.to_string(), entry.to_string()))
    }

    /// A rule that did not match cannot hand the hit of its set on to the
    /// next rule that does.
    #[tokio::test]
    async fn a_rule_that_did_not_match_leaves_no_sub_rule_for_the_next() {
        let e = engine(HIT_CONF);
        assert_eq!(sub_rule_of(&e, "hit.example").await, (Some(1), None));
    }

    /// Inside one rule, a branch that did not decide the match leaves no hit
    /// either: here the set matches but the port does not, and the `DOMAIN`
    /// half is what makes the `OR` match.
    #[tokio::test]
    async fn a_branch_that_did_not_decide_the_rule_leaves_no_sub_rule() {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\n\
OR,((AND,((RULE-SET,X),(DEST-PORT,1))),(DOMAIN,hit.example)),P\nFINAL,DIRECT\n\
[Ruleset X]\nDOMAIN,hit.example\n",
        );
        assert_eq!(sub_rule_of(&e, "hit.example").await, (Some(0), None));
    }

    /// What matched inside a `NOT` is not why the rule matched.
    #[tokio::test]
    async fn a_not_carries_no_sub_rule() {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\nNOT,((RULE-SET,X)),REJECT\n\
DOMAIN,hit.example,P\nFINAL,DIRECT\n[Ruleset X]\nDOMAIN,hit.example\n",
        );
        // the set matches, so the NOT does not: the next rule has no hit of its own
        assert_eq!(sub_rule_of(&e, "hit.example").await, (Some(1), None));
        // the set misses, so the NOT matches, with nothing to name
        assert_eq!(sub_rule_of(&e, "other.example").await, (Some(0), None));
    }

    /// The hit of a pre-matching rule that missed is not the next flagged
    /// rule's.
    #[tokio::test]
    async fn the_pre_pass_does_not_hand_a_hit_on_either() {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\n\
AND,((RULE-SET,X),(DEST-PORT,1)),REJECT,pre-matching\n\
DOMAIN,hit.example,REJECT,pre-matching\nFINAL,DIRECT\n[Ruleset X]\nDOMAIN,hit.example\n",
        );
        let d = e
            .evaluate(&session("hit.example"), OutboundMode::Rule, &NoResolve)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::PreMatching, Some(1)));
        assert!(d.sub_rule.is_none());
    }

    /// The hit that survives is the one of a set that took part in the match.
    #[tokio::test]
    async fn the_sub_rule_is_the_set_that_made_the_rule_match() {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\n\
AND,((RULE-SET,X),(DOMAIN-KEYWORD,example)),P\n\
OR,((RULE-SET,X),(RULE-SET,Y)),P\nFINAL,DIRECT\n\
[Ruleset X]\nDOMAIN,x.example\n[Ruleset Y]\nDOMAIN,y.example\n",
        );
        // the other half of the AND adds nothing, and takes nothing
        assert_eq!(
            sub_rule_of(&e, "x.example").await,
            (Some(0), hit("X", "DOMAIN,x.example"))
        );
        // an OR carries the set that matched, not the one that was tried first
        assert_eq!(
            sub_rule_of(&e, "y.example").await,
            (Some(1), hit("Y", "DOMAIN,y.example"))
        );
        // a logical rule inside the AND that matches without a set of its own
        // does not take away the hit of the set beside it
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\n\
AND,((RULE-SET,Y),(OR,((DEST-PORT,1),(DEST-PORT,443)))),P\nFINAL,DIRECT\n\
[Ruleset Y]\nDOMAIN,y.example\n",
        );
        assert_eq!(
            sub_rule_of(&e, "y.example").await,
            (Some(0), hit("Y", "DOMAIN,y.example"))
        );
    }

    /// A rule that has to resolve before it can match names the line it
    /// matched afterwards.
    #[tokio::test]
    async fn a_rule_set_that_needs_a_lookup_names_the_line_the_lookup_matched() {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,Ips,P\nFINAL,DIRECT\n\
[Ruleset Ips]\nIP-CIDR,172.16.0.0/12\n",
        );
        let d = e
            .evaluate(
                &session("host.example"),
                OutboundMode::Rule,
                &Counting::v4("172.16.1.1"),
            )
            .await;
        assert_eq!(d.matched, Some(0));
        let found = d.sub_rule.expect("the line the lookup made match");
        assert_eq!(
            (found.set.as_str(), found.entry.as_str()),
            ("Ips", "IP-CIDR,172.16.0.0/12")
        );
    }

    /// A list written the usual way, `no-resolve` on every IP line: a name is
    /// no reason for the rule that refers to it to resolve, and an address in
    /// the list is a hit that names the line.
    #[tokio::test]
    async fn a_rule_set_of_no_resolve_lines_never_asks_for_a_lookup() {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,Ips,P\nFINAL,DIRECT\n\
[Ruleset Ips]\nIP-CIDR,10.0.0.0/8,no-resolve\n",
        );
        // the answer is inside the network, so a lookup would show as a hit
        let r = Counting::v4("10.1.1.1");
        let d = e
            .evaluate(&session("host.example"), OutboundMode::Rule, &r)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Final, Some(1)));
        assert_eq!(r.calls.load(Ordering::SeqCst), 0);
        assert!(d.resolved.is_none());
        assert!(d.sub_rule.is_none());
        let d = e
            .evaluate(&session("10.1.1.1"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(0)));
        assert_eq!(d.policy().map(named).as_deref(), Some("P"));
        let found = d.sub_rule.expect("the line that matched");
        assert_eq!(
            (found.set.as_str(), found.entry.as_str()),
            ("Ips", "IP-CIDR,10.0.0.0/8,no-resolve")
        );
    }

    /// A loaded database that gives every address the same AS number and no
    /// country.
    struct AsnOfEverything(u32);
    impl GeoLookup for AsnOfEverything {
        fn country(&self, _: IpAddr) -> GeoResult<[u8; 2]> {
            GeoResult::Unknown
        }
        fn asn(&self, _: IpAddr) -> GeoResult<u32> {
            GeoResult::Known(self.0)
        }
    }

    fn engine_with_asn(text: &str, asn: u32) -> RuleEngine {
        let cfg = load(text);
        let sets = InlineSets::from_config(&cfg);
        RuleEngine::build(&cfg, &sets, Arc::new(AsnOfEverything(asn))).unwrap()
    }

    /// A list of AS numbers written the usual way: the name is resolved once
    /// for all its lines, and the line that names the number of the address
    /// it resolved to is the hit.
    #[tokio::test]
    async fn a_rule_set_of_asn_lines_resolves_the_name_once_and_names_the_line() {
        let e = engine_with_asn(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,Asns,P\nFINAL,DIRECT\n\
[Ruleset Asns]\nIP-ASN,64500\nIP-ASN,64501\nIP-ASN,64502\n",
            64501,
        );
        let r = Counting::v4("192.0.2.1");
        let d = e
            .evaluate(&session("host.example"), OutboundMode::Rule, &r)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(0)));
        assert_eq!(d.policy().map(named).as_deref(), Some("P"));
        assert_eq!(r.calls.load(Ordering::SeqCst), 1);
        let found = d.sub_rule.expect("the line that matched");
        assert_eq!(
            (found.set.as_str(), found.entry.as_str()),
            ("Asns", "IP-ASN,64501")
        );
    }

    /// With `no-resolve` on every line a name is no reason to resolve
    /// anything, and an address needs no lookup to be judged.
    #[tokio::test]
    async fn a_rule_set_of_asn_lines_that_all_say_no_resolve_never_asks_for_a_lookup() {
        let e = engine_with_asn(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,Asns,P\nFINAL,DIRECT\n\
[Ruleset Asns]\nIP-ASN,64500,no-resolve\nIP-ASN,64501,no-resolve\n",
            64501,
        );
        // the answer has a number a line names, so a lookup would show as a hit
        let r = Counting::v4("192.0.2.1");
        let d = e
            .evaluate(&session("host.example"), OutboundMode::Rule, &r)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Final, Some(1)));
        assert_eq!(r.calls.load(Ordering::SeqCst), 0);
        assert!(d.resolved.is_none());
        assert!(d.sub_rule.is_none());
        let d = e
            .evaluate(&session("192.0.2.1"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(0)));
        let found = d.sub_rule.expect("the line that matched");
        assert_eq!(
            (found.set.as_str(), found.entry.as_str()),
            ("Asns", "IP-ASN,64501,no-resolve")
        );
    }

    /// The set of an `AND` matches before the lookup its other half asks
    /// for, and again after it: the hit is the one of the evaluation that
    /// matched.
    #[tokio::test]
    async fn a_set_beside_a_lookup_keeps_its_hit_once_the_rule_matches() {
        let e = engine(
            "[Proxy]\nP = direct\n[Rule]\nAND,((RULE-SET,X),(IP-CIDR,10.0.0.0/8)),P\nFINAL,DIRECT\n\
[Ruleset X]\nDOMAIN,hit.example\n",
        );
        let d = e
            .evaluate(
                &session("hit.example"),
                OutboundMode::Rule,
                &Counting::v4("10.9.9.9"),
            )
            .await;
        assert_eq!(d.matched, Some(0));
        let found = d.sub_rule.expect("the set of the rule that resolved");
        assert_eq!(
            (found.set.as_str(), found.entry.as_str()),
            ("X", "DOMAIN,hit.example")
        );
        // and when the lookup then says no, nothing is left of the set's hit
        let d = e
            .evaluate(
                &session("hit.example"),
                OutboundMode::Rule,
                &Counting::v4("11.9.9.9"),
            )
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Final, Some(1)));
        assert!(d.sub_rule.is_none());
    }

    /// The databases are not there when the engine is built: they come with
    /// the first download. `GEOIP,UNKNOWN` does not match until its database
    /// is loaded, and then the engine that was built before it matches, with
    /// no reload.
    #[tokio::test]
    async fn unknown_starts_matching_once_the_database_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let (geo, _) = GeoDb::open(dir.path());
        let cfg = load("[Proxy]\nA = direct\nB = direct\n[Rule]\nGEOIP,UNKNOWN,A\nFINAL,B\n");
        let e = RuleEngine::build(&cfg, &InlineSets::from_config(&cfg), geo.clone()).unwrap();
        let policy = || async {
            let d = e
                .evaluate(&session("127.0.0.1"), OutboundMode::Rule, &Panicking)
                .await;
            d.policy().map(named)
        };
        assert_eq!(policy().await.as_deref(), Some("B"));
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("GeoIP2-Country-Test.mmdb"),
            dir.path().join(COUNTRY_FILE),
        )
        .unwrap();
        geo.load(DbKind::Country).unwrap();
        // 127.0.0.1 is in no country of the database
        assert_eq!(policy().await.as_deref(), Some("A"));
    }

    #[test]
    fn build_fails_without_final() {
        let l = from_text(
            "[Rule]\nDOMAIN,a.com,DIRECT\n",
            Path::new("t.conf"),
            &LoadOptions::for_tests(),
        );
        assert!(RuleEngine::build(&l.config, &NoNested, Arc::new(NoGeo)).is_err());
    }

    /// A process lookup that answers one process (or none), and counts how
    /// often it is asked.
    struct CountedProcess {
        calls: AtomicUsize,
        answer: Option<ProcessInfo>,
    }

    impl CountedProcess {
        fn is(name: &str) -> CountedProcess {
            CountedProcess {
                calls: AtomicUsize::new(0),
                answer: Some(ProcessInfo {
                    pid: Some(4242),
                    name: name.into(),
                    path: None,
                }),
            }
        }

        fn unknown() -> CountedProcess {
            CountedProcess {
                calls: AtomicUsize::new(0),
                answer: None,
            }
        }

        fn asked(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl LazyProcess for CountedProcess {
        fn process(&self) -> BoxFuture<'_, Option<ProcessInfo>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let answer = self.answer.clone();
            Box::pin(async move { answer })
        }
    }

    /// A session to `host` decided with a way to look up its process.
    async fn decide_with(
        e: &RuleEngine,
        host: &str,
        resolver: &dyn LazyResolver,
        process: &dyn LazyProcess,
    ) -> Decision {
        e.evaluate_with_process(&session(host), OutboundMode::Rule, resolver, process)
            .await
    }

    /// The process is looked up the first time a rule needs it and not again
    /// in the same evaluation: the rules after that read the answer.
    #[tokio::test]
    async fn a_process_rule_asks_for_the_process_once() {
        let e = engine("[Rule]\nPROCESS-NAME,a,REJECT\nPROCESS-NAME,b,REJECT\nFINAL,DIRECT\n");
        let process = CountedProcess::is("b");
        let d = decide_with(&e, "example.com", &Panicking, &process).await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(1)));
        assert_eq!(d.policy().map(named).as_deref(), Some("REJECT"));
        assert_eq!(process.asked(), 1);
        assert_eq!(e.rules()[0].hits(), 0);
        assert_eq!(e.rules()[1].hits(), 1);
        // a process nobody could find is asked for once as well, and no rule matches it
        let process = CountedProcess::unknown();
        let d = decide_with(&e, "example.com", &Panicking, &process).await;
        assert_eq!((d.reason, d.matched), (Reason::Final, Some(2)));
        assert_eq!(process.asked(), 1);
    }

    /// A rule that needs the process and a lookup gets the process first: when
    /// it is not the one, the name is never resolved.
    #[tokio::test]
    async fn the_process_is_asked_before_the_name_is_resolved() {
        let e =
            engine("[Rule]\nAND,((PROCESS-NAME,x),(IP-CIDR,10.0.0.0/8)),REJECT\nFINAL,DIRECT\n");
        let resolver = Counting::v4("10.1.1.1");
        let process = CountedProcess::is("y");
        let d = decide_with(&e, "host.example", &resolver, &process).await;
        assert_eq!((d.reason, d.matched), (Reason::Final, Some(1)));
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
        assert_eq!(process.asked(), 1);
        // and when it is, the lookup follows and decides: three evaluations of one rule
        let process = CountedProcess::is("x");
        let d = decide_with(&e, "host.example", &resolver, &process).await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(0)));
        assert_eq!(d.policy().map(named).as_deref(), Some("REJECT"));
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(process.asked(), 1);
        assert!(d.resolved.is_some());
    }

    /// A rule is evaluated again once the process is known: the set beside
    /// the process keeps its hit through that, and leaves none when the rule
    /// does not match in the end.
    #[tokio::test]
    async fn a_set_beside_the_process_keeps_its_hit_once_the_rule_matches() {
        let e = engine(
            "[Rule]\nAND,((RULE-SET,X),(PROCESS-NAME,p)),REJECT\nFINAL,DIRECT\n\
[Ruleset X]\nDOMAIN,hit.example\n",
        );
        let d = decide_with(&e, "hit.example", &Panicking, &CountedProcess::is("p")).await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(0)));
        let found = d.sub_rule.expect("the set of the rule that matched");
        assert_eq!(
            (found.set.as_str(), found.entry.as_str()),
            ("X", "DOMAIN,hit.example")
        );
        let d = decide_with(&e, "hit.example", &Panicking, &CountedProcess::is("q")).await;
        assert_eq!((d.reason, d.matched), (Reason::Final, Some(1)));
        assert!(d.sub_rule.is_none());
    }

    /// A session that names its process already is not asked about it.
    #[tokio::test]
    async fn a_given_process_is_not_asked_for() {
        let e = engine("[Rule]\nPROCESS-NAME,curl,REJECT\nFINAL,DIRECT\n");
        let mut s = session("example.com");
        s.process = Some(ProcessInfo {
            pid: None,
            name: "curl".into(),
            path: None,
        });
        let process = CountedProcess::is("wget");
        let d = e
            .evaluate_with_process(&s, OutboundMode::Rule, &Panicking, &process)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(0)));
        assert_eq!(process.asked(), 0);
    }

    /// The evaluations that do not wait for anything (`evaluate`, the lookups
    /// that come before a name is resolved) know no process: a rule that needs
    /// one does not match there.
    #[tokio::test]
    async fn evaluate_knows_no_process() {
        let e = engine("[Rule]\nPROCESS-NAME,x,REJECT\nFINAL,DIRECT\n");
        let d = e
            .evaluate(&session("example.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!((d.reason, d.matched), (Reason::Final, Some(1)));
        let (d, trace) = e
            .evaluate_traced(&session("example.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.matched, Some(1));
        assert_eq!(steps(&trace), [(0, "no-match"), (1, "match")]);
    }

    /// A rule flagged `pre-matching` that takes its lines from a set with a
    /// `PROCESS-NAME` line in it: the pre-pass and the two lookups before a
    /// name is resolved do not ask for the process (they have none to give),
    /// the rule misses there and is evaluated where it is written.
    #[tokio::test]
    async fn pre_matching_does_not_ask_for_the_process() {
        let e = engine(
            "[Rule]\nRULE-SET,Marked,REJECT,pre-matching\nFINAL,DIRECT\n\
[Ruleset Marked]\nPROCESS-NAME,x\n",
        );
        assert_eq!(e.pre_matching().len(), 1);
        assert!(e.pre_match_domain("example.com", 443).is_none());
        assert!(e.pre_match_ip("192.0.2.1".parse().unwrap(), 443).is_none());
        let process = CountedProcess::is("x");
        let d = decide_with(&e, "example.com", &Panicking, &process).await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(0)));
        assert_eq!(d.policy().map(named).as_deref(), Some("REJECT"));
        assert_eq!(process.asked(), 1);
        // not the process it names: the rule is asked once and misses
        let process = CountedProcess::is("y");
        let d = decide_with(&e, "example.com", &Panicking, &process).await;
        assert_eq!((d.reason, d.matched), (Reason::Final, Some(1)));
        assert_eq!(process.asked(), 1);
    }

    /// `RULE-SET,SYSTEM` is compiled without its two `PROCESS-NAME` lines
    /// (they name services of macOS): a session that none of its domain lines
    /// takes does not wait for a process lookup for it.
    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn a_system_rule_set_never_asks_for_the_process() {
        let e = engine("[Rule]\nRULE-SET,SYSTEM,REJECT\nFINAL,DIRECT\n");
        let process = CountedProcess::is("trustd");
        let d = decide_with(&e, "example.com", &Panicking, &process).await;
        assert_eq!((d.reason, d.matched), (Reason::Final, Some(1)));
        assert_eq!(process.asked(), 0);
        // and its domain lines are there
        let d = decide_with(&e, "captive.apple.com", &Panicking, &process).await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(0)));
        assert_eq!(process.asked(), 0);
    }

    /// On macOS the two lines are there: the session is asked about once, and
    /// a process called `trustd` is taken by the set.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_system_rule_set_never_asks_for_the_process() {
        let e = engine("[Rule]\nRULE-SET,SYSTEM,REJECT\nFINAL,DIRECT\n");
        let process = CountedProcess::is("trustd");
        let d = decide_with(&e, "example.com", &Panicking, &process).await;
        assert_eq!((d.reason, d.matched), (Reason::Rule, Some(0)));
        assert_eq!(process.asked(), 1);
        let process = CountedProcess::unknown();
        let d = decide_with(&e, "example.com", &Panicking, &process).await;
        assert_eq!((d.reason, d.matched), (Reason::Final, Some(1)));
        assert_eq!(process.asked(), 1);
    }

    #[tokio::test]
    async fn no_process_answers_nothing() {
        assert_eq!(NoProcess.process().await, None);
    }

    fn assert_send<T: Send>(_: &T) {}

    /// `evaluate`'s returned future must stay `Send` (e.g. so a caller can
    /// `tokio::spawn` it on a multi-thread runtime); this pins that down
    /// without needing to actually poll it. So must the one that looks up a
    /// process.
    #[test]
    fn evaluate_future_is_send() {
        let e = engine(CONF);
        let s = session("foo.org");
        let resolver = NoResolve;
        let fut = e.evaluate(&s, OutboundMode::Rule, &resolver);
        assert_send(&fut);
        let process = NoProcess;
        let fut = e.evaluate_with_process(&s, OutboundMode::Rule, &resolver, &process);
        assert_send(&fut);
    }
}
