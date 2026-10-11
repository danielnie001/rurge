//! A compiled set: domain index + IP index + linear entries (M2 design §6.2),
//! behind an `ArcSwap` handle so a resource update swaps the set in place.

use crate::domain_index::{DomainIndex, DomainIndexBuilder};
use crate::ip_index::{IpIndex, IpIndexBuilder};
use crate::matcher::{
    CompiledSubRule, EvalCtx, Family, SetLookup, SetMatch, SetVerdict, Verdict, compile_subrule,
    target_ip,
};
use crate::set_format::{ParsedSet, SetKind, SetLine};
use arc_swap::ArcSwap;
use rurge_config::rule::RuleKind;
use rurge_config::session::SessionInfo;
use std::sync::Arc;

pub struct CompiledSet {
    pub name: String,
    pub kind: SetKind,
    pub needs_dns: bool,
    pub version: u64,
    domains: DomainIndex,
    ips: IpIndex,
    /// Everything that is neither a plain domain nor an indexable CIDR, in file order.
    linear: Vec<(u32, CompiledSubRule)>,
    entries: Vec<Box<str>>,
}

impl CompiledSet {
    pub fn empty(name: &str, kind: SetKind) -> CompiledSet {
        CompiledSet {
            name: name.to_string(),
            kind,
            needs_dns: false,
            version: 0,
            domains: DomainIndex::default(),
            ips: IpIndexBuilder::new().build(),
            linear: Vec::new(),
            entries: Vec::new(),
        }
    }

    pub fn compile(
        name: &str,
        kind: SetKind,
        parsed: &ParsedSet,
        sets: &dyn SetLookup,
        version: u64,
    ) -> CompiledSet {
        let mut domains = DomainIndexBuilder::new();
        let mut ips = IpIndexBuilder::new();
        let mut linear = Vec::new();
        let mut entries: Vec<Box<str>> = Vec::with_capacity(parsed.lines.len());
        let mut needs_dns = false;
        for (i, line) in parsed.lines.iter().enumerate() {
            let i = u32::try_from(i).expect("set size is bounded by MAX_ENTRIES");
            match line {
                SetLine::Domain { name, suffix } => {
                    if *suffix {
                        domains.add_suffix(name, i);
                        entries.push(format!(".{name}").into_boxed_str());
                    } else {
                        domains.add_exact(name, i);
                        entries.push(name.as_str().into());
                    }
                }
                SetLine::Rule(sub) => {
                    entries.push(sub.raw.as_str().into());
                    match &sub.kind {
                        RuleKind::Domain(d) => domains.add_exact(d, i),
                        RuleKind::DomainSuffix(d) => domains.add_suffix(d, i),
                        RuleKind::IpCidr(net) if !sub.no_resolve => {
                            needs_dns = true;
                            ips.add_v4(*net, i);
                        }
                        RuleKind::IpCidr6(net) if !sub.no_resolve => {
                            needs_dns = true;
                            ips.add_v6(*net, i);
                        }
                        other => {
                            if !sub.no_resolve
                                && matches!(
                                    other,
                                    RuleKind::IpCidr(_)
                                        | RuleKind::IpCidr6(_)
                                        | RuleKind::GeoIp(_)
                                        | RuleKind::IpAsn(_)
                                        | RuleKind::RuleSet(_)
                                        | RuleKind::DomainSet(_)
                                        | RuleKind::And(_)
                                        | RuleKind::Or(_)
                                        | RuleKind::Not(_)
                                )
                            {
                                needs_dns = true;
                            }
                            linear.push((i, compile_subrule(sub, sets)));
                        }
                    }
                }
            }
        }
        CompiledSet {
            name: name.to_string(),
            kind,
            needs_dns,
            version,
            domains: domains.build(),
            ips: ips.build(),
            linear,
            entries,
        }
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn entry(&self, i: u32) -> &str {
        self.entries
            .get(i as usize)
            .map(|e| e.as_ref())
            .unwrap_or("")
    }

    fn hit(&self, i: u32) -> SetVerdict {
        SetVerdict {
            verdict: Verdict::Match,
            entry: Some(self.entry(i).to_string()),
        }
    }

    /// Domain index first (never resolves), then linear entries in file order,
    /// then the IP index — which asks for resolution only when the set has IP
    /// entries and the line did not say `no-resolve`.
    pub fn eval(
        &self,
        s: &SessionInfo,
        ctx: &mut EvalCtx<'_>,
        no_resolve: bool,
        extended: bool,
    ) -> SetVerdict {
        if !self.domains.is_empty() {
            let mut targets: Vec<&str> = Vec::with_capacity(3);
            targets.extend(s.dst_host.as_domain());
            if extended {
                targets.extend(s.sni.as_deref());
                targets.extend(s.http_host.as_deref());
            }
            for t in targets {
                if let Some(hit) = self.domains.lookup(t) {
                    return self.hit(hit.entry);
                }
            }
        }
        let mut needs = false;
        for (i, rule) in &self.linear {
            match rule.matcher.eval(
                s,
                ctx,
                no_resolve || rule.no_resolve,
                extended || rule.extended,
            ) {
                Verdict::Match => return self.hit(*i),
                Verdict::NeedsResolve => needs = true,
                Verdict::NoMatch => {}
            }
        }
        if !self.ips.is_empty() {
            for family in [Family::V4, Family::V6] {
                match target_ip(s, ctx, no_resolve, family) {
                    Ok(ip) => {
                        if let Some(i) = self.ips.lookup(ip) {
                            return self.hit(i);
                        }
                    }
                    Err(Verdict::NeedsResolve) => needs = true,
                    Err(_) => {}
                }
            }
        }
        SetVerdict {
            verdict: if needs {
                Verdict::NeedsResolve
            } else {
                Verdict::NoMatch
            },
            entry: None,
        }
    }
}

#[derive(Clone)]
pub struct SetHandle {
    inner: Arc<ArcSwap<CompiledSet>>,
}

impl SetHandle {
    pub fn new(set: CompiledSet) -> SetHandle {
        SetHandle {
            inner: Arc::new(ArcSwap::from_pointee(set)),
        }
    }

    pub fn load(&self) -> Arc<CompiledSet> {
        self.inner.load_full()
    }

    pub fn store(&self, set: CompiledSet) {
        self.inner.store(Arc::new(set));
    }

    pub fn version(&self) -> u64 {
        self.inner.load().version
    }
}

impl SetMatch for SetHandle {
    fn name(&self) -> String {
        self.inner.load().name.clone()
    }

    fn eval(
        &self,
        s: &SessionInfo,
        ctx: &mut EvalCtx<'_>,
        no_resolve: bool,
        extended: bool,
    ) -> SetVerdict {
        let set = self.inner.load();
        set.eval(s, ctx, no_resolve, extended)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::{NoGeo, ResolvedAddrs, SetRef};
    use crate::set_format::parse_set;
    use rurge_config::HostName;
    use rurge_config::rule::{ParseCtx, ResourceRef};
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::path::Path;

    /// Stub registry: file path → handle.
    #[derive(Default)]
    struct Stub(HashMap<String, SetHandle>);
    impl SetLookup for Stub {
        fn lookup(&self, r: &ResourceRef, kind: SetKind) -> SetRef {
            let key = match r {
                ResourceRef::File(p) => p.file_name().unwrap().to_string_lossy().to_string(),
                other => panic!("unexpected {other:?}"),
            };
            Arc::new(
                self.0
                    .get(&key)
                    .unwrap_or_else(|| panic!("no set {key} ({kind:?})"))
                    .clone(),
            )
        }
    }

    fn compile(kind: SetKind, text: &str, stub: &Stub) -> CompiledSet {
        let names: HashSet<String> = HashSet::new();
        let ctx = ParseCtx {
            inline_rulesets: &names,
            base_dir: Path::new("."),
        };
        let parsed = parse_set(kind, text, &ctx);
        CompiledSet::compile("t", kind, &parsed, stub, 1)
    }

    fn session(host: &str) -> SessionInfo {
        SessionInfo::tcp(HostName::parse(host), 443)
    }

    const RULES: &str = "\
DOMAIN,exact.com
DOMAIN-SUFFIX,suffix.com
DOMAIN-KEYWORD,keyw
IP-CIDR,10.0.0.0/8
IP-CIDR6,fd00::/8
IP-CIDR,192.168.0.0/16,no-resolve
DEST-PORT,8443
";

    #[test]
    fn domain_entries_match_without_dns_and_report_the_entry() {
        let set = compile(SetKind::RuleSet, RULES, &Stub::default());
        assert_eq!(set.entry_count(), 7);
        assert!(set.needs_dns);
        let mut ctx = EvalCtx::new(&NoGeo);
        let v = set.eval(&session("exact.com"), &mut ctx, false, false);
        assert_eq!(v.verdict, Verdict::Match);
        assert_eq!(v.entry.as_deref(), Some("DOMAIN,exact.com"));
        let v = set.eval(&session("a.suffix.com"), &mut ctx, false, false);
        assert_eq!(v.entry.as_deref(), Some("DOMAIN-SUFFIX,suffix.com"));
        let v = set.eval(&session("xkeywx.org"), &mut ctx, false, false);
        assert_eq!(v.entry.as_deref(), Some("DOMAIN-KEYWORD,keyw"));
        assert!(ctx.resolved.is_none());
    }

    #[test]
    fn ip_entries_use_the_index_for_ip_targets() {
        let set = compile(SetKind::RuleSet, RULES, &Stub::default());
        let mut ctx = EvalCtx::new(&NoGeo);
        assert_eq!(
            set.eval(&session("10.1.1.1"), &mut ctx, false, false)
                .entry
                .as_deref(),
            Some("IP-CIDR,10.0.0.0/8")
        );
        assert_eq!(
            set.eval(&session("[fd00::1]"), &mut ctx, false, false)
                .entry
                .as_deref(),
            Some("IP-CIDR6,fd00::/8")
        );
        assert_eq!(
            set.eval(&session("192.168.1.1"), &mut ctx, false, false)
                .entry
                .as_deref(),
            Some("IP-CIDR,192.168.0.0/16,no-resolve")
        );
        assert_eq!(
            set.eval(&session("11.1.1.1"), &mut ctx, false, false)
                .verdict,
            Verdict::NoMatch
        );
    }

    #[test]
    fn domain_targets_ask_for_resolution_unless_no_resolve() {
        let set = compile(SetKind::RuleSet, RULES, &Stub::default());
        let s = session("other.org");
        let mut ctx = EvalCtx::new(&NoGeo);
        assert_eq!(
            set.eval(&s, &mut ctx, false, false).verdict,
            Verdict::NeedsResolve
        );
        assert_eq!(
            set.eval(&s, &mut ctx, true, false).verdict,
            Verdict::NoMatch
        );
        ctx.resolved = Some(ResolvedAddrs {
            v4: vec!["10.2.3.4".parse().unwrap()],
            v6: vec![],
        });
        assert_eq!(
            set.eval(&s, &mut ctx, false, false).entry.as_deref(),
            Some("IP-CIDR,10.0.0.0/8")
        );
        ctx.resolved = Some(ResolvedAddrs {
            v4: vec!["192.168.9.9".parse().unwrap()],
            v6: vec![],
        });
        // Already resolved by an earlier rule: a no-resolve entry is evaluated against the address.
        assert_eq!(
            set.eval(&s, &mut ctx, false, false).entry.as_deref(),
            Some("IP-CIDR,192.168.0.0/16,no-resolve")
        );
        ctx.resolved = Some(ResolvedAddrs {
            v4: vec![],
            v6: vec!["fd00::9".parse().unwrap()],
        });
        assert_eq!(
            set.eval(&s, &mut ctx, false, false).entry.as_deref(),
            Some("IP-CIDR6,fd00::/8")
        );
    }

    #[test]
    fn set_without_ip_entries_never_needs_dns() {
        let set = compile(
            SetKind::RuleSet,
            "DOMAIN,a.com\nDEST-PORT,80\nIP-CIDR,10.0.0.0/8,no-resolve\n",
            &Stub::default(),
        );
        assert!(!set.needs_dns);
        let mut ctx = EvalCtx::new(&NoGeo);
        assert_eq!(
            set.eval(&session("b.com"), &mut ctx, false, false).verdict,
            Verdict::NoMatch
        );
    }

    /// What `set` says about a session to `host` and the line it names, asked
    /// by a rule that did or did not say `no-resolve`, after an earlier rule
    /// left `resolved` behind (or nothing).
    fn ask(
        set: &CompiledSet,
        host: &str,
        resolved: Option<ResolvedAddrs>,
        no_resolve: bool,
    ) -> (Verdict, Option<String>) {
        let mut ctx = EvalCtx::new(&NoGeo);
        ctx.resolved = resolved;
        let v = set.eval(&session(host), &mut ctx, no_resolve, false);
        (v.verdict, v.entry)
    }

    fn addrs(v4: &[&str], v6: &[&str]) -> Option<ResolvedAddrs> {
        Some(ResolvedAddrs {
            v4: v4.iter().map(|a| a.parse().unwrap()).collect(),
            v6: v6.iter().map(|a| a.parse().unwrap()).collect(),
        })
    }

    fn matches_line(line: &str) -> (Verdict, Option<String>) {
        (Verdict::Match, Some(line.to_string()))
    }

    const NO_MATCH: (Verdict, Option<String>) = (Verdict::NoMatch, None);
    const ASKS: (Verdict, Option<String>) = (Verdict::NeedsResolve, None);

    #[test]
    fn a_set_of_no_resolve_ip_lines_never_asks_for_a_lookup() {
        let set = compile(
            SetKind::RuleSet,
            "IP-CIDR,10.0.0.0/8,no-resolve\nIP-CIDR6,fd00::/8,no-resolve\n",
            &Stub::default(),
        );
        assert!(!set.needs_dns);
        for no_resolve in [false, true] {
            let why = format!("no_resolve = {no_resolve}");
            // an address is matched whatever the rule said
            assert_eq!(
                ask(&set, "10.1.1.1", None, no_resolve),
                matches_line("IP-CIDR,10.0.0.0/8,no-resolve"),
                "{why}"
            );
            assert_eq!(
                ask(&set, "[fd00::1]", None, no_resolve),
                matches_line("IP-CIDR6,fd00::/8,no-resolve"),
                "{why}"
            );
            assert_eq!(ask(&set, "11.1.1.1", None, no_resolve), NO_MATCH, "{why}");
            assert_eq!(ask(&set, "[fc00::1]", None, no_resolve), NO_MATCH, "{why}");
            // a name nobody has resolved is no reason to resolve it
            assert_eq!(ask(&set, "other.org", None, no_resolve), NO_MATCH, "{why}");
            // one an earlier rule resolved is matched against its addresses
            assert_eq!(
                ask(&set, "other.org", addrs(&["10.2.3.4"], &[]), no_resolve),
                matches_line("IP-CIDR,10.0.0.0/8,no-resolve"),
                "{why}"
            );
            assert_eq!(
                ask(&set, "other.org", addrs(&[], &["fd00::9"]), no_resolve),
                matches_line("IP-CIDR6,fd00::/8,no-resolve"),
                "{why}"
            );
            assert_eq!(
                ask(
                    &set,
                    "other.org",
                    addrs(&["11.2.3.4"], &["fc00::9"]),
                    no_resolve
                ),
                NO_MATCH,
                "{why}"
            );
        }
        let mut ctx = EvalCtx::new(&NoGeo);
        for no_resolve in [false, true] {
            let v = set.eval(&session("other.org"), &mut ctx, no_resolve, false);
            assert_eq!(v.verdict, Verdict::NoMatch);
        }
        assert!(ctx.resolved.is_none());
    }

    #[test]
    fn the_same_network_with_and_without_no_resolve_still_asks() {
        let set = compile(
            SetKind::RuleSet,
            "IP-CIDR,10.0.0.0/8,no-resolve\nIP-CIDR,10.0.0.0/8\n",
            &Stub::default(),
        );
        assert!(set.needs_dns);
        assert_eq!(ask(&set, "other.org", None, false), ASKS);
        assert_eq!(ask(&set, "other.org", None, true), NO_MATCH);
        for no_resolve in [false, true] {
            assert_eq!(
                ask(&set, "10.1.1.1", None, no_resolve).0,
                Verdict::Match,
                "no_resolve = {no_resolve}"
            );
        }
    }

    #[test]
    fn a_line_of_the_other_family_that_may_ask_makes_the_set_ask() {
        let set = compile(
            SetKind::RuleSet,
            "IP-CIDR,10.0.0.0/8,no-resolve\nIP-CIDR6,fd00::/8\n",
            &Stub::default(),
        );
        assert_eq!(ask(&set, "other.org", None, false), ASKS);
        assert_eq!(ask(&set, "other.org", None, true), NO_MATCH);
        assert_eq!(
            ask(&set, "other.org", addrs(&["10.2.3.4"], &[]), false),
            matches_line("IP-CIDR,10.0.0.0/8,no-resolve")
        );
    }

    /// `needs_dns` counts a nested set whether or not it ends up asking; the
    /// set asks only when one of its own IP lines may.
    #[test]
    fn a_nested_set_beside_no_resolve_lines_does_not_make_the_set_ask() {
        let mut stub = Stub::default();
        let inner = compile(SetKind::DomainSet, ".inner.com\n", &stub);
        stub.0.insert("inner.txt".into(), SetHandle::new(inner));
        let outer = compile(
            SetKind::RuleSet,
            "IP-CIDR,10.0.0.0/8,no-resolve\nDOMAIN-SET,inner.txt\n",
            &stub,
        );
        assert!(outer.needs_dns);
        assert_eq!(ask(&outer, "other.org", None, false), NO_MATCH);
        assert_eq!(
            ask(&outer, "10.1.1.1", None, false),
            matches_line("IP-CIDR,10.0.0.0/8,no-resolve")
        );
    }

    #[test]
    fn extended_matching_applies_to_domain_entries() {
        let set = compile(
            SetKind::DomainSet,
            ".suffix.com\nexact.com\n",
            &Stub::default(),
        );
        let mut s = session("1.2.3.4");
        s.sni = Some("x.suffix.com".into());
        let mut ctx = EvalCtx::new(&NoGeo);
        assert_eq!(
            set.eval(&s, &mut ctx, false, false).verdict,
            Verdict::NoMatch
        );
        let v = set.eval(&s, &mut ctx, false, true);
        assert_eq!(v.entry.as_deref(), Some(".suffix.com"));
        assert_eq!(
            set.eval(&session("exact.com"), &mut ctx, false, false)
                .entry
                .as_deref(),
            Some("exact.com")
        );
        assert_eq!(
            set.eval(&session("a.exact.com"), &mut ctx, false, false)
                .verdict,
            Verdict::NoMatch
        );
    }

    #[test]
    fn nested_sets_are_evaluated_through_the_lookup() {
        let mut stub = Stub::default();
        let inner = compile(SetKind::DomainSet, ".inner.com\n", &stub);
        stub.0.insert("inner.txt".into(), SetHandle::new(inner));
        let outer = compile(
            SetKind::RuleSet,
            "DOMAIN,outer.com\nDOMAIN-SET,inner.txt\n",
            &stub,
        );
        let mut ctx = EvalCtx::new(&NoGeo);
        let v = outer.eval(&session("a.inner.com"), &mut ctx, false, false);
        assert_eq!(v.verdict, Verdict::Match);
        assert_eq!(v.entry.as_deref(), Some("DOMAIN-SET,inner.txt"));
        assert_eq!(
            ctx.sub_hit.as_ref().map(|h| h.entry.as_str()),
            Some(".inner.com")
        );
    }

    #[test]
    fn handle_swaps_in_place() {
        let stub = Stub::default();
        let h = SetHandle::new(compile(SetKind::DomainSet, "a.com\n", &stub));
        let h2 = h.clone();
        let mut ctx = EvalCtx::new(&NoGeo);
        assert_eq!(
            h2.eval(&session("a.com"), &mut ctx, false, false).verdict,
            Verdict::Match
        );
        let mut newer = compile(SetKind::DomainSet, "b.com\n", &stub);
        newer.version = 2;
        h.store(newer);
        assert_eq!(h2.version(), 2);
        assert_eq!(
            h2.eval(&session("a.com"), &mut ctx, false, false).verdict,
            Verdict::NoMatch
        );
        assert_eq!(
            h2.eval(&session("b.com"), &mut ctx, false, false).verdict,
            Verdict::Match
        );
        assert_eq!(h2.name(), "t");
    }

    #[test]
    fn empty_set_matches_nothing() {
        let set = CompiledSet::empty("e", SetKind::RuleSet);
        let mut ctx = EvalCtx::new(&NoGeo);
        assert_eq!(
            set.eval(&session("a.com"), &mut ctx, false, false).verdict,
            Verdict::NoMatch
        );
        assert_eq!(set.entry_count(), 0);
        assert_eq!(set.entry(5), "");
    }
}
