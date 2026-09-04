//! Rule engine (M2 design §6.5): top-level rules evaluated in order, DNS on
//! demand through `LazyResolver`, FINAL / `dns-failed` fallbacks, per-rule hit
//! counters and one-time runtime notes.

use crate::matcher::{EvalCtx, GeoLookup, Matcher, ResolvedAddrs, SetLookup, SubRuleHit, Verdict};
use crate::pre_matching::{PreMatch, PreMatchingSet};
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
        })
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
        for (pos, rule) in self.rules.iter().enumerate() {
            let started = Instant::now();
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
            let v = rule
                .matcher
                .eval(&s, &mut ctx, true, rule.params.extended_matching);
            ctx.notes.clear();
            if v == Verdict::Match {
                if let PolicyRef::Builtin(b) = &rule.policy {
                    if b.is_reject() {
                        return Some(PreMatch {
                            rule: rule.index,
                            policy: rule.policy.clone(),
                        });
                    }
                }
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

    #[test]
    fn build_fails_without_final() {
        let l = from_text(
            "[Rule]\nDOMAIN,a.com,DIRECT\n",
            Path::new("t.conf"),
            &LoadOptions::for_tests(),
        );
        assert!(RuleEngine::build(&l.config, &NoNested, Arc::new(NoGeo)).is_err());
    }
}
