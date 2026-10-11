//! Rule engine (M2 design §6.5): top-level rules evaluated in order — after
//! the ones flagged `pre-matching`, which are checked first and never resolve —,
//! DNS on demand through `LazyResolver`, FINAL / `dns-failed` fallbacks,
//! per-rule hit counters and one-time runtime notes.

use crate::matcher::{EvalCtx, GeoLookup, Matcher, ResolvedAddrs, SetLookup, SubRuleHit, Verdict};
use crate::pre_matching::{PreMatch, PreMatchingSet};
use crate::registry::SetRegistry;
use rurge_config::policy::Builtin;
use rurge_config::rule::{PolicyRef, RuleKind, RuleParams};
use rurge_config::session::SessionInfo;
use rurge_config::{Config, HostName, Span};
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
    }
}

impl RuleEngine {
    /// Compiles `[Rule]` up to the effective (last) FINAL; shadowed FINAL lines
    /// and rules after the last FINAL are dropped (the config already warned).
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
        let last = cfg
            .effective_final()
            .ok_or_else(|| BuildError("the [Rule] section has no FINAL rule".to_string()))?;
        let mut rules = Vec::with_capacity(last + 1);
        for (i, rule) in cfg.rules.iter().enumerate().take(last + 1) {
            if matches!(rule.kind, RuleKind::Final) && i != last {
                continue;
            }
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
            pre,
            registry: None,
        })
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

    pub async fn evaluate(
        &self,
        s: &SessionInfo,
        mode: OutboundMode,
        resolver: &dyn LazyResolver,
    ) -> Decision {
        self.run(s, mode, resolver, None).await
    }

    pub async fn evaluate_traced(
        &self,
        s: &SessionInfo,
        mode: OutboundMode,
        resolver: &dyn LazyResolver,
    ) -> (Decision, Vec<TraceStep>) {
        let mut trace = Vec::new();
        let d = self.run(s, mode, resolver, Some(&mut trace)).await;
        (d, trace)
    }

    async fn run(
        &self,
        s: &SessionInfo,
        mode: OutboundMode,
        resolver: &dyn LazyResolver,
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
        let mut ctx = EvalCtx::new(self.geo.as_ref());
        let mut notes: Vec<String> = Vec::new();
        if let Some(decision) = self.pre_pass(s, &mut ctx, &mut notes, trace.as_deref_mut()) {
            return decision;
        }
        for (pos, rule) in self.rules.iter().enumerate() {
            let started = Instant::now();
            ctx.sub_hit = None;
            let mut verdict = self.eval_rule(rule, s, &mut ctx);
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
        let mut ctx = EvalCtx::new(self.geo.as_ref());
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
    use crate::matcher::{NoGeo, SetRef};
    use crate::set::{CompiledSet, SetHandle};
    use crate::set_format::{ParsedSet, SetKind, SetLine, internal_set_text, parse_set};
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_config::rule::{ParseCtx, ResourceRef};
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
                    };
                    let parsed = parse_set(SetKind::RuleSet, internal_set_text(*i), &ctx);
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

    #[tokio::test]
    async fn last_final_wins_and_shadowed_final_is_dropped() {
        let e = engine("[Proxy]\nP = direct\n[Rule]\nFINAL,DIRECT\nDOMAIN,a.com,P\nFINAL,P\n");
        assert_eq!(e.rules().len(), 2);
        let d = e
            .evaluate(&session("a.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::Rule);
        assert_eq!(d.matched, Some(1));
        let d = e
            .evaluate(&session("b.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::Final);
        assert_eq!(d.matched, Some(2));
        assert_eq!(d.policy().map(named).as_deref(), Some("P"));
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
            "[Proxy]\nP = direct\n[Rule]\nSUBNET,SSID:Home,P\nDOMAIN-SUFFIX,apple.com,DIRECT\nFINAL,P\n",
        );
        let d = e
            .evaluate(&session("www.apple.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.notes.len(), 1);
        assert!(d.notes[0].contains("SUBNET"));
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

    /// `SUBNET` has no runtime support, so its rule notes it — once, whichever
    /// pass meets the rule first, and the note reaches the decision.
    #[tokio::test]
    async fn a_note_of_the_pre_pass_is_surfaced_once_and_reaches_the_decision() {
        const NOTING: &str = "[Proxy]\nP = direct\n[Rule]\n\
SUBNET,SSID:Home,REJECT,pre-matching\nDOMAIN,ads.example,REJECT,pre-matching\n\
DOMAIN-SUFFIX,apple.com,DIRECT\nFINAL,P\n";
        // a hit of the pre-pass, after the rule that noted
        let e = engine(NOTING);
        let d = e
            .evaluate(&session("ads.example"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::PreMatching);
        assert_eq!(d.notes.len(), 1, "{:?}", d.notes);
        assert!(d.notes[0].contains("SUBNET"));
        let d = e
            .evaluate(&session("www.apple.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.reason, Reason::Rule);
        assert!(d.notes.is_empty(), "{:?}", d.notes);
        // and a pass without a hit: the note is not said again at the rule's own place
        let e = engine(NOTING);
        let d = e
            .evaluate(&session("www.apple.com"), OutboundMode::Rule, &Panicking)
            .await;
        assert_eq!(d.notes.len(), 1, "{:?}", d.notes);
        assert_eq!(e.rules()[2].hits(), 1);
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

    /// Gives every address the same AS number.
    struct AsnOfEverything(u32);
    impl GeoLookup for AsnOfEverything {
        fn country(&self, _: IpAddr) -> Option<[u8; 2]> {
            None
        }
        fn asn(&self, _: IpAddr) -> Option<u32> {
            Some(self.0)
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

    #[test]
    fn build_fails_without_final() {
        let l = from_text(
            "[Rule]\nDOMAIN,a.com,DIRECT\n",
            Path::new("t.conf"),
            &LoadOptions::for_tests(),
        );
        assert!(RuleEngine::build(&l.config, &NoNested, Arc::new(NoGeo)).is_err());
    }

    fn assert_send<T: Send>(_: &T) {}

    /// `evaluate`'s returned future must stay `Send` (e.g. so a caller can
    /// `tokio::spawn` it on a multi-thread runtime); this pins that down
    /// without needing to actually poll it.
    #[test]
    fn evaluate_future_is_send() {
        let e = engine(CONF);
        let s = session("foo.org");
        let resolver = NoResolve;
        let fut = e.evaluate(&s, OutboundMode::Rule, &resolver);
        assert_send(&fut);
    }
}
