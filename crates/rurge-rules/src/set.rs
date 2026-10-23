//! A compiled set: domain index + linear entries + the AS numbers its `IP-ASN`
//! lines name + IP index, looked up in that order (M2 design §6.2), behind an
//! `ArcSwap` handle so a resource update swaps the set in place.

use crate::domain_index::{DomainIndex, DomainIndexBuilder};
use crate::ip_index::{IpIndex, IpIndexBuilder};
use crate::matcher::{
    CompiledSubRule, EvalCtx, Family, GeoResult, SetLookup, SetMatch, SetVerdict, Verdict,
    compile_subrule, still_needs, target_ip,
};
use crate::set_format::{ParsedSet, SetKind, SetLine};
use arc_swap::ArcSwap;
use rurge_config::rule::RuleKind;
use rurge_config::session::SessionInfo;
use std::collections::HashMap;
use std::sync::Arc;

pub struct CompiledSet {
    pub name: String,
    pub kind: SetKind,
    pub needs_dns: bool,
    pub version: u64,
    domains: DomainIndex,
    ips: IpIndex,
    /// Whether a line in `ips` may ask for a lookup, that is, one that did not
    /// say `no-resolve`. Not `needs_dns`, which is wider: a nested set, a
    /// logical rule, `GEOIP` and `IP-ASN` set it whether or not they end up
    /// asking.
    ips_resolve: bool,
    /// The AS numbers the set's `IP-ASN` lines name, each with the entry of
    /// the first line that names it. Every such line judges the same address,
    /// so one look-up here stands for all of them.
    asns: HashMap<u32, u32>,
    /// Whether an `IP-ASN` line may ask for a lookup, that is, one that did
    /// not say `no-resolve`. Not `needs_dns`, which is wider: a nested set, a
    /// logical rule, `GEOIP` and the IP lines set it whether or not they end
    /// up asking; and not `ips_resolve`, which is about the IP lines.
    asns_resolve: bool,
    /// Everything that is not a plain domain, a CIDR or an `IP-ASN` line that
    /// names a number, in file order: the `UNKNOWN` lines of `GEOIP` and
    /// `IP-ASN` are here, each judged on its own.
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
            ips_resolve: false,
            asns: HashMap::new(),
            asns_resolve: false,
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
        let mut ips_resolve = false;
        let mut asns = HashMap::new();
        let mut asns_resolve = false;
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
                        RuleKind::IpCidr(net) => {
                            ips.add_v4(*net, i);
                            if !sub.no_resolve {
                                needs_dns = true;
                                ips_resolve = true;
                            }
                        }
                        RuleKind::IpCidr6(net) => {
                            ips.add_v6(*net, i);
                            if !sub.no_resolve {
                                needs_dns = true;
                                ips_resolve = true;
                            }
                        }
                        RuleKind::IpAsn(asn) => {
                            asns.entry(*asn).or_insert(i);
                            if !sub.no_resolve {
                                needs_dns = true;
                                asns_resolve = true;
                            }
                        }
                        other => {
                            if !sub.no_resolve
                                && matches!(
                                    other,
                                    RuleKind::GeoIp(_)
                                        | RuleKind::GeoIpUnknown
                                        | RuleKind::IpAsnUnknown
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
            ips_resolve,
            asns,
            asns_resolve,
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

    /// Domain index first (never resolves), then linear entries in file order
    /// (the `UNKNOWN` lines among them), then the `IP-ASN` lines that name a
    /// number, then the IP index. Those `IP-ASN` lines all judge the same
    /// address, so one look-up of its AS number stands for them all; they ask
    /// for resolution only when the caller did not say `no-resolve` and some
    /// `IP-ASN` line did not either. The IP index holds
    /// every `IP-CIDR` / `IP-CIDR6` line, with or without `no-resolve`; it
    /// asks for resolution only when the caller did not say `no-resolve` and
    /// some line in it did not either (a line that said it never asks). When
    /// several lines match, the one reported is the first found in that
    /// order, which is not always the first in the file: an `IP-ASN` line and
    /// an IP line are found after the linear ones, the `IP-ASN` line first,
    /// and among the IP lines of one address family the longest prefix wins.
    /// The IPv4 address is asked first and the IPv6 one after it, so a name
    /// resolved to both, each inside a line, reports the IPv4 line whatever
    /// the prefix lengths.
    ///
    /// Before the connection exists (`EvalCtx::before_connection`) a line
    /// that matches still matches: a name the set holds is a match. A name it
    /// does not hold is `NoMatch` only if no line waits for the connection: an
    /// IP or `IP-ASN` line has no address to look at, whether it says
    /// `no-resolve` or not, and a line about ports, the transport or HTTP has
    /// nothing to look at either. With one, the answer is `NeedsConnection`.
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
        let (mut needs_connection, mut needs_process, mut needs_resolve) = (false, false, false);
        for (i, rule) in &self.linear {
            match rule.matcher.eval(
                s,
                ctx,
                no_resolve || rule.no_resolve,
                extended || rule.extended,
            ) {
                Verdict::Match => return self.hit(*i),
                Verdict::NeedsConnection => needs_connection = true,
                Verdict::NeedsProcess => needs_process = true,
                Verdict::NeedsResolve => needs_resolve = true,
                Verdict::NoMatch => {}
            }
        }
        if !self.asns.is_empty() {
            match target_ip(s, ctx, no_resolve || !self.asns_resolve, Family::Any) {
                Ok(ip) => {
                    if let GeoResult::Known(asn) = ctx.asn(ip)
                        && let Some(&i) = self.asns.get(&asn)
                    {
                        return self.hit(i);
                    }
                }
                Err(Verdict::NeedsConnection) => needs_connection = true,
                Err(Verdict::NeedsResolve) => needs_resolve = true,
                Err(_) => {}
            }
        }
        if !self.ips.is_empty() {
            for family in [Family::V4, Family::V6] {
                match target_ip(s, ctx, no_resolve || !self.ips_resolve, family) {
                    Ok(ip) => {
                        if let Some(i) = self.ips.lookup(ip) {
                            return self.hit(i);
                        }
                    }
                    Err(Verdict::NeedsConnection) => needs_connection = true,
                    Err(Verdict::NeedsResolve) => needs_resolve = true,
                    Err(_) => {}
                }
            }
        }
        SetVerdict {
            verdict: still_needs(needs_connection, needs_process, needs_resolve)
                .unwrap_or(Verdict::NoMatch),
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
    use crate::matcher::{GeoLookup, NoGeo, ResolvedAddrs, SetRef};
    use crate::set_format::parse_set;
    use rurge_config::HostName;
    use rurge_config::Reach;
    use rurge_config::rule::{ParseCtx, ResourceRef};
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::net::IpAddr;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
            reach: Reach::Anywhere,
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
        ask_with(set, &NoGeo, host, resolved, no_resolve)
    }

    /// `ask`, with a database to look the addresses up in.
    fn ask_with(
        set: &CompiledSet,
        geo: &dyn GeoLookup,
        host: &str,
        resolved: Option<ResolvedAddrs>,
        no_resolve: bool,
    ) -> (Verdict, Option<String>) {
        let mut ctx = EvalCtx::new(geo);
        ctx.resolved = resolved;
        let v = set.eval(&session(host), &mut ctx, no_resolve, false);
        (v.verdict, v.entry)
    }

    /// A loaded database that knows the AS numbers it is given, nothing else
    /// (it says `Unknown` of every other address, and of every country), and
    /// counts the questions it is asked.
    struct Asns {
        known: HashMap<IpAddr, u32>,
        asked: AtomicUsize,
    }

    impl Asns {
        fn of(known: &[(&str, u32)]) -> Asns {
            Asns {
                known: known
                    .iter()
                    .map(|(ip, asn)| (ip.parse().unwrap(), *asn))
                    .collect(),
                asked: AtomicUsize::new(0),
            }
        }

        fn asked(&self) -> usize {
            self.asked.load(Ordering::SeqCst)
        }
    }

    impl GeoLookup for Asns {
        fn country(&self, _: IpAddr) -> GeoResult<[u8; 2]> {
            GeoResult::Unknown
        }

        fn asn(&self, ip: IpAddr) -> GeoResult<u32> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.known
                .get(&ip)
                .map_or(GeoResult::Unknown, |asn| GeoResult::Known(*asn))
        }
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

    /// A network written with host bits set (`192.168.1.5/16`), or with the
    /// hexadecimal digits of an IPv6 address in upper case (`2001:DB8::/32`),
    /// is accepted in a rule-set file and is the network of its prefix.
    #[test]
    fn a_network_with_host_bits_or_upper_case_hexadecimal_is_the_network_of_its_prefix() {
        let set = compile(
            SetKind::RuleSet,
            "IP-CIDR,192.168.1.5/16,no-resolve\nIP-CIDR6,2001:DB8::/32,no-resolve\n",
            &Stub::default(),
        );
        assert_eq!(set.entry_count(), 2, "both lines are accepted");
        for (host, line) in [
            ("192.168.200.1", "IP-CIDR,192.168.1.5/16,no-resolve"),
            ("192.168.0.0", "IP-CIDR,192.168.1.5/16,no-resolve"),
            ("192.168.255.255", "IP-CIDR,192.168.1.5/16,no-resolve"),
            ("[2001:db8::1]", "IP-CIDR6,2001:DB8::/32,no-resolve"),
            ("[2001:db8:ffff::]", "IP-CIDR6,2001:DB8::/32,no-resolve"),
        ] {
            assert_eq!(ask(&set, host, None, false), matches_line(line), "{host}");
        }
        for host in [
            "192.167.255.255",
            "192.169.0.0",
            "[2001:db7::1]",
            "[2001:db9::]",
        ] {
            assert_eq!(ask(&set, host, None, false), NO_MATCH, "{host}");
        }
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

    /// A line that is walked can ask on its own account: the IP lines beside
    /// it all say `no-resolve`, and the set asks all the same.
    #[test]
    fn a_walked_line_that_may_ask_makes_the_set_ask_beside_no_resolve_ip_lines() {
        let set = compile(
            SetKind::RuleSet,
            "GEOIP,US\nIP-CIDR,10.0.0.0/8,no-resolve\n",
            &Stub::default(),
        );
        assert_eq!(ask(&set, "other.org", None, false), ASKS);
        assert_eq!(ask(&set, "other.org", None, true), NO_MATCH);
        // the IP line is not what asks: without the GEOIP line the set never does
        let ip_line_alone = compile(
            SetKind::RuleSet,
            "IP-CIDR,10.0.0.0/8,no-resolve\n",
            &Stub::default(),
        );
        assert_eq!(ask(&ip_line_alone, "other.org", None, false), NO_MATCH);
        // once the name is resolved, an address inside the network is the IP line's
        for no_resolve in [false, true] {
            assert_eq!(
                ask(&set, "other.org", addrs(&["10.2.3.4"], &[]), no_resolve),
                matches_line("IP-CIDR,10.0.0.0/8,no-resolve"),
                "no_resolve = {no_resolve}"
            );
        }
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
    fn no_resolve_ip_lines_go_into_the_index_not_the_linear_list() {
        let set = compile(
            SetKind::RuleSet,
            "IP-CIDR,10.0.0.0/8,no-resolve\nIP-CIDR6,fd00::/8,no-resolve\n",
            &Stub::default(),
        );
        assert_eq!(set.linear.len(), 0);
        assert_eq!(set.ips.len(), 2);
        // what is neither a name nor a CIDR is still walked, line by line
        let set = compile(
            SetKind::RuleSet,
            "IP-CIDR,10.0.0.0/8,no-resolve\nDOMAIN-KEYWORD,keyw\nIP-CIDR6,fd00::/8,no-resolve\n",
            &Stub::default(),
        );
        assert_eq!(set.ips.len(), 2);
        assert_eq!(set.linear.len(), 1);
        assert_eq!(set.entry(set.linear[0].0), "DOMAIN-KEYWORD,keyw");
    }

    #[test]
    fn among_no_resolve_ip_lines_the_longest_prefix_is_reported() {
        let set = compile(
            SetKind::RuleSet,
            "IP-CIDR,10.0.0.0/8,no-resolve\nIP-CIDR,10.1.0.0/16,no-resolve\n",
            &Stub::default(),
        );
        assert_eq!(
            ask(&set, "10.1.2.3", None, false),
            matches_line("IP-CIDR,10.1.0.0/16,no-resolve")
        );
    }

    /// The lines that are walked come before the IP index: a `DEST-PORT` line
    /// is the one reported wherever it stands in the file, for an IP line
    /// written either way.
    #[test]
    fn a_walked_line_is_reported_ahead_of_an_ip_line_however_it_is_written() {
        for ip_line in ["IP-CIDR,10.0.0.0/8", "IP-CIDR,10.0.0.0/8,no-resolve"] {
            let set = compile(
                SetKind::RuleSet,
                &format!("{ip_line}\nDEST-PORT,443\n"),
                &Stub::default(),
            );
            assert_eq!(
                ask(&set, "10.1.1.1", None, false),
                matches_line("DEST-PORT,443"),
                "{ip_line}"
            );
        }
    }

    /// A list of AS numbers with `no-resolve` on every line: a name nobody has
    /// resolved is no reason to look anything up, and an address is judged by
    /// the number the database gives it.
    #[test]
    fn a_set_of_no_resolve_asn_lines_never_asks_for_a_lookup() {
        let set = compile(
            SetKind::RuleSet,
            "IP-ASN,64500,no-resolve\nIP-ASN,64501,no-resolve\n",
            &Stub::default(),
        );
        assert!(!set.needs_dns);
        let db = Asns::of(&[
            ("192.0.2.1", 64501),
            ("198.51.100.1", 64999),
            ("2001:db8::1", 64500),
        ]);
        for no_resolve in [false, true] {
            let why = format!("no_resolve = {no_resolve}");
            assert_eq!(
                ask_with(&set, &db, "other.org", None, no_resolve),
                NO_MATCH,
                "{why}"
            );
            // an address whose number a line names matches, whatever the rule said
            assert_eq!(
                ask_with(&set, &db, "192.0.2.1", None, no_resolve),
                matches_line("IP-ASN,64501,no-resolve"),
                "{why}"
            );
            assert_eq!(
                ask_with(&set, &db, "[2001:db8::1]", None, no_resolve),
                matches_line("IP-ASN,64500,no-resolve"),
                "{why}"
            );
            // one whose number no line names does not, nor one the database does not know
            for host in ["198.51.100.1", "203.0.113.9"] {
                assert_eq!(
                    ask_with(&set, &db, host, None, no_resolve),
                    NO_MATCH,
                    "{host}, {why}"
                );
            }
            // a name an earlier rule resolved is judged by its address
            assert_eq!(
                ask_with(
                    &set,
                    &db,
                    "other.org",
                    addrs(&["192.0.2.1"], &[]),
                    no_resolve
                ),
                matches_line("IP-ASN,64501,no-resolve"),
                "{why}"
            );
            assert_eq!(
                ask_with(
                    &set,
                    &db,
                    "other.org",
                    addrs(&["198.51.100.1"], &[]),
                    no_resolve
                ),
                NO_MATCH,
                "{why}"
            );
            assert_eq!(
                ask_with(&set, &db, "other.org", addrs(&[], &[]), no_resolve),
                NO_MATCH,
                "{why}"
            );
        }
    }

    #[test]
    fn the_same_number_with_and_without_no_resolve_still_asks() {
        let db = Asns::of(&[("192.0.2.1", 64500)]);
        for (text, first) in [
            (
                "IP-ASN,64500,no-resolve\nIP-ASN,64500\n",
                "IP-ASN,64500,no-resolve",
            ),
            ("IP-ASN,64500\nIP-ASN,64500,no-resolve\n", "IP-ASN,64500"),
        ] {
            let set = compile(SetKind::RuleSet, text, &Stub::default());
            assert!(set.needs_dns, "{text:?}");
            assert_eq!(
                ask_with(&set, &db, "other.org", None, false),
                ASKS,
                "{text:?}"
            );
            assert_eq!(
                ask_with(&set, &db, "other.org", None, true),
                NO_MATCH,
                "{text:?}"
            );
            // the number is named twice: the first of the two lines is reported
            for no_resolve in [false, true] {
                assert_eq!(
                    ask_with(&set, &db, "192.0.2.1", None, no_resolve),
                    matches_line(first),
                    "{text:?}, no_resolve = {no_resolve}"
                );
            }
        }
    }

    /// The address an `IP-ASN` line judges is the first IPv4 address of the
    /// name, else the first IPv6 one.
    #[test]
    fn a_name_is_judged_by_its_first_ipv4_address_else_its_first_ipv6_one() {
        let db = Asns::of(&[
            ("192.0.2.1", 64500),
            ("192.0.2.2", 64501),
            ("2001:db8::1", 64502),
            ("2001:db8::2", 64500),
        ]);
        let ask_name = |text: &str, resolved| {
            let set = compile(SetKind::RuleSet, text, &Stub::default());
            ask_with(&set, &db, "other.org", resolved, false)
        };
        // of a name with both, the IPv4 address decides: the line that names the
        // number of the IPv6 address alone does not match
        let both = addrs(&["192.0.2.1"], &["2001:db8::1"]);
        assert_eq!(
            ask_name("IP-ASN,64500\nIP-ASN,64502\n", both.clone()),
            matches_line("IP-ASN,64500")
        );
        assert_eq!(ask_name("IP-ASN,64502\n", both), NO_MATCH);
        // of several IPv4 addresses, the first
        assert_eq!(
            ask_name("IP-ASN,64501\n", addrs(&["192.0.2.1", "192.0.2.2"], &[])),
            NO_MATCH
        );
        // with none, the first IPv6 one
        let v6_only = addrs(&[], &["2001:db8::1", "2001:db8::2"]);
        assert_eq!(
            ask_name("IP-ASN,64500\nIP-ASN,64502\n", v6_only.clone()),
            matches_line("IP-ASN,64502")
        );
        assert_eq!(ask_name("IP-ASN,64500\n", v6_only), NO_MATCH);
    }

    /// What the `IP-ASN` lines ask is their own: not `needs_dns`, which a
    /// nested set sets without asking anything, and not what the IP lines ask.
    #[test]
    fn asn_lines_that_say_no_resolve_are_not_made_to_ask_by_a_nested_set() {
        let db = Asns::of(&[("192.0.2.1", 64500)]);
        let mut stub = Stub::default();
        let inner = compile(SetKind::DomainSet, ".inner.com\n", &stub);
        stub.0.insert("inner.txt".into(), SetHandle::new(inner));
        let outer = compile(
            SetKind::RuleSet,
            "IP-ASN,64500,no-resolve\nDOMAIN-SET,inner.txt\n",
            &stub,
        );
        assert!(outer.needs_dns);
        assert_eq!(ask_with(&outer, &db, "other.org", None, false), NO_MATCH);
        assert_eq!(
            ask_with(&outer, &db, "192.0.2.1", None, false),
            matches_line("IP-ASN,64500,no-resolve")
        );
    }

    #[test]
    fn asn_lines_and_ip_lines_each_make_the_set_ask_on_their_own_account() {
        let db = Asns::of(&[]);
        // an IP line that may ask, beside an `IP-ASN` line that says it never does
        let ip_asks = compile(
            SetKind::RuleSet,
            "IP-ASN,64500,no-resolve\nIP-CIDR,10.0.0.0/8\n",
            &Stub::default(),
        );
        assert!(ip_asks.needs_dns);
        assert_eq!(ask_with(&ip_asks, &db, "other.org", None, false), ASKS);
        assert_eq!(ask_with(&ip_asks, &db, "other.org", None, true), NO_MATCH);
        // an `IP-ASN` line that may ask, beside an IP line that says it never does
        let asn_asks = compile(
            SetKind::RuleSet,
            "IP-CIDR,10.0.0.0/8,no-resolve\nIP-ASN,64500\n",
            &Stub::default(),
        );
        assert!(asn_asks.needs_dns);
        assert_eq!(ask_with(&asn_asks, &db, "other.org", None, false), ASKS);
        assert_eq!(ask_with(&asn_asks, &db, "other.org", None, true), NO_MATCH);
    }

    /// With no number to be had, no `IP-ASN` line matches (the number 0 is no
    /// answer), and whether a name is worth resolving is still up to the lines.
    #[test]
    fn without_a_number_from_the_database_no_asn_line_matches() {
        let knows_nothing = Asns::of(&[]);
        let may_ask = compile(
            SetKind::RuleSet,
            "IP-ASN,0\nIP-ASN,64500\nIP-ASN,64501,no-resolve\n",
            &Stub::default(),
        );
        let never_asks = compile(
            SetKind::RuleSet,
            "IP-ASN,0,no-resolve\nIP-ASN,64500,no-resolve\n",
            &Stub::default(),
        );
        let no_database: [&dyn GeoLookup; 2] = [&NoGeo, &knows_nothing];
        for db in no_database {
            for no_resolve in [false, true] {
                for set in [&may_ask, &never_asks] {
                    for host in ["192.0.2.1", "[2001:db8::1]"] {
                        assert_eq!(ask_with(set, db, host, None, no_resolve), NO_MATCH);
                    }
                    assert_eq!(
                        ask_with(set, db, "other.org", addrs(&["192.0.2.1"], &[]), no_resolve),
                        NO_MATCH
                    );
                }
            }
            assert_eq!(ask_with(&may_ask, db, "other.org", None, false), ASKS);
            assert_eq!(ask_with(&may_ask, db, "other.org", None, true), NO_MATCH);
            assert_eq!(
                ask_with(&never_asks, db, "other.org", None, false),
                NO_MATCH
            );
        }
    }

    /// The `IP-ASN` lines are looked at before the IP index, wherever either
    /// stands in the file.
    #[test]
    fn an_asn_line_is_reported_ahead_of_an_ip_line_that_also_matches() {
        let db = Asns::of(&[("10.1.1.1", 64500)]);
        for (text, reported) in [
            (
                "IP-ASN,64500\nIP-CIDR,10.0.0.0/8,no-resolve\n",
                "IP-ASN,64500",
            ),
            (
                "IP-CIDR,10.0.0.0/8,no-resolve\nIP-ASN,64500\n",
                "IP-ASN,64500",
            ),
            (
                "IP-CIDR,10.0.0.0/8\nIP-ASN,64500,no-resolve\n",
                "IP-ASN,64500,no-resolve",
            ),
        ] {
            let set = compile(SetKind::RuleSet, text, &Stub::default());
            assert_eq!(
                ask_with(&set, &db, "10.1.1.1", None, false),
                matches_line(reported),
                "{text:?}"
            );
        }
    }

    /// How many questions the database is asked while `set` judges `host`.
    fn questions(
        set: &CompiledSet,
        host: &str,
        resolved: Option<ResolvedAddrs>,
        no_resolve: bool,
    ) -> usize {
        let db = Asns::of(&[("192.0.2.1", 64999)]);
        ask_with(set, &db, host, resolved, no_resolve);
        db.asked()
    }

    /// One question for an address, whatever the number of lines; none when
    /// there is no address, or when a line walked before the `IP-ASN` lines
    /// has already matched.
    #[test]
    fn the_database_is_asked_once_for_an_address_and_not_at_all_without_one() {
        let lines = compile(
            SetKind::RuleSet,
            "IP-ASN,64500\nIP-ASN,64501,no-resolve\nIP-ASN,64502\n",
            &Stub::default(),
        );
        assert_eq!(questions(&lines, "192.0.2.1", None, false), 1);
        assert_eq!(
            questions(&lines, "other.org", addrs(&["192.0.2.1"], &[]), false),
            1
        );
        assert_eq!(
            questions(&lines, "other.org", addrs(&[], &["2001:db8::1"]), true),
            1
        );
        assert_eq!(questions(&lines, "other.org", None, false), 0);
        assert_eq!(questions(&lines, "other.org", None, true), 0);
        assert_eq!(questions(&lines, "other.org", addrs(&[], &[]), false), 0);
        let walked_first = compile(
            SetKind::RuleSet,
            "DEST-PORT,443\nIP-ASN,64500\n",
            &Stub::default(),
        );
        assert_eq!(questions(&walked_first, "192.0.2.1", None, false), 0);
        // asked again in the same context, the answer is kept
        let db = Asns::of(&[("192.0.2.1", 64500)]);
        let mut ctx = EvalCtx::new(&db);
        ctx.resolved = addrs(&["192.0.2.1"], &[]);
        for _ in 0..3 {
            lines.eval(&session("other.org"), &mut ctx, false, false);
        }
        assert_eq!(db.asked(), 1);
    }

    #[test]
    fn asn_lines_are_not_in_the_list_of_lines_walked_one_by_one() {
        let set = compile(
            SetKind::RuleSet,
            "IP-ASN,64500\nDEST-PORT,1\nIP-ASN,64501,no-resolve\nDOMAIN,a.com\n",
            &Stub::default(),
        );
        // what is neither a name, a network nor an `IP-ASN` line is walked
        assert_eq!(set.linear.len(), 1);
        assert_eq!(set.entry(set.linear[0].0), "DEST-PORT,1");
        // the `IP-ASN` lines match all the same, each by its own number
        let db = Asns::of(&[("192.0.2.1", 64500), ("192.0.2.2", 64501)]);
        assert_eq!(
            ask_with(&set, &db, "192.0.2.1", None, false),
            matches_line("IP-ASN,64500")
        );
        assert_eq!(
            ask_with(&set, &db, "192.0.2.2", None, false),
            matches_line("IP-ASN,64501,no-resolve")
        );
    }

    /// A pair of loaded databases: the addresses the first is given have a
    /// country, the ones the second is given an AS number. Every other
    /// address is `Unknown` to the database that does not have it.
    struct Databases {
        countries: HashSet<IpAddr>,
        asns: HashMap<IpAddr, u32>,
    }

    impl Databases {
        fn of(countries: &[&str], asns: &[(&str, u32)]) -> Databases {
            Databases {
                countries: countries.iter().map(|ip| ip.parse().unwrap()).collect(),
                asns: asns
                    .iter()
                    .map(|(ip, asn)| (ip.parse().unwrap(), *asn))
                    .collect(),
            }
        }
    }

    impl GeoLookup for Databases {
        fn country(&self, ip: IpAddr) -> GeoResult<[u8; 2]> {
            if self.countries.contains(&ip) {
                GeoResult::Known(*b"ZZ")
            } else {
                GeoResult::Unknown
            }
        }

        fn asn(&self, ip: IpAddr) -> GeoResult<u32> {
            self.asns
                .get(&ip)
                .map_or(GeoResult::Unknown, |asn| GeoResult::Known(*asn))
        }
    }

    /// `IP-ASN,UNKNOWN` and `GEOIP,UNKNOWN` lines are no entries for the table
    /// of AS numbers: they are judged one by one with the other lines that
    /// are walked, in the order of the file, and one that does not say
    /// `no-resolve` makes the set ask for a name to be resolved.
    #[test]
    fn unknown_lines_in_a_set_are_evaluated_in_order() {
        // 192.0.2.1: a country and the number 64500; 192.0.2.2: the number
        // 64501, no country; 192.0.2.3: a country, no number; 198.51.100.1
        // is in neither database
        let db = Databases::of(
            &["192.0.2.1", "192.0.2.3"],
            &[("192.0.2.1", 64500), ("192.0.2.2", 64501)],
        );
        let set = compile(
            SetKind::RuleSet,
            "IP-ASN,64500,no-resolve\nIP-ASN,UNKNOWN\nGEOIP,UNKNOWN,no-resolve\n",
            &Stub::default(),
        );
        // no line is skipped; only the number is in the table
        assert_eq!(set.entry_count(), 3);
        assert_eq!(set.asns.len(), 1);
        assert_eq!(set.linear.len(), 2);
        let at = |host: &str| ask_with(&set, &db, host, None, false);
        // an address no database has matches both `UNKNOWN` lines, and the
        // one reported is the first of them in the file
        assert_eq!(at("198.51.100.1"), matches_line("IP-ASN,UNKNOWN"));
        // one with a country and no number matches the line about numbers
        assert_eq!(at("192.0.2.3"), matches_line("IP-ASN,UNKNOWN"));
        // one with a number and no country matches the line about countries
        assert_eq!(at("192.0.2.2"), matches_line("GEOIP,UNKNOWN,no-resolve"));
        // one with both matches neither, and is judged by the table of numbers
        assert_eq!(at("192.0.2.1"), matches_line("IP-ASN,64500,no-resolve"));
        // the same lines the other way round report the other one
        let reversed = compile(
            SetKind::RuleSet,
            "GEOIP,UNKNOWN,no-resolve\nIP-ASN,UNKNOWN\nIP-ASN,64500,no-resolve\n",
            &Stub::default(),
        );
        assert_eq!(
            ask_with(&reversed, &db, "198.51.100.1", None, false),
            matches_line("GEOIP,UNKNOWN,no-resolve")
        );

        // the line that does not say `no-resolve` is why the set asks
        assert!(set.needs_dns);
        assert_eq!(ask_with(&set, &db, "other.org", None, false), ASKS);
        assert_eq!(ask_with(&set, &db, "other.org", None, true), NO_MATCH);
        assert_eq!(
            ask_with(&set, &db, "other.org", addrs(&["198.51.100.1"], &[]), false),
            matches_line("IP-ASN,UNKNOWN")
        );
        for (line, asks) in [
            ("IP-ASN,UNKNOWN", true),
            ("GEOIP,UNKNOWN", true),
            ("IP-ASN,UNKNOWN,no-resolve", false),
            ("GEOIP,UNKNOWN,no-resolve", false),
        ] {
            let set = compile(SetKind::RuleSet, &format!("{line}\n"), &Stub::default());
            assert_eq!(set.needs_dns, asks, "{line}");
            let want = if asks { ASKS } else { NO_MATCH };
            assert_eq!(
                ask_with(&set, &db, "other.org", None, false),
                want,
                "{line}"
            );
        }
    }

    /// A number written twice, once as `AS64500`, is one number; the line
    /// reported is the first that names it.
    #[test]
    fn a_number_named_twice_reports_the_first_of_the_two_lines() {
        let set = compile(
            SetKind::RuleSet,
            "IP-ASN,64500\nIP-ASN,64501\nIP-ASN,AS64500,no-resolve\n",
            &Stub::default(),
        );
        let db = Asns::of(&[("192.0.2.1", 64500), ("192.0.2.2", 64501)]);
        assert_eq!(
            ask_with(&set, &db, "192.0.2.1", None, false),
            matches_line("IP-ASN,64500")
        );
        assert_eq!(
            ask_with(&set, &db, "192.0.2.2", None, false),
            matches_line("IP-ASN,64501")
        );
    }

    /// The lines that are walked come before the `IP-ASN` lines, wherever
    /// either stands in the file.
    #[test]
    fn a_walked_line_that_matches_is_reported_ahead_of_an_asn_line_that_also_matches() {
        let db = Asns::of(&[("192.0.2.1", 64500)]);
        for text in [
            "IP-ASN,64500\nDEST-PORT,443\n",
            "DEST-PORT,443\nIP-ASN,64500\n",
        ] {
            let set = compile(SetKind::RuleSet, text, &Stub::default());
            assert_eq!(
                ask_with(&set, &db, "192.0.2.1", None, false),
                matches_line("DEST-PORT,443"),
                "{text:?}"
            );
        }
    }

    /// A note after a name is not part of the name: the line matches the name
    /// it is for, and is shown without the note.
    #[test]
    fn a_name_line_with_a_note_after_it_matches_its_name() {
        let names: HashSet<String> = HashSet::new();
        let ctx = ParseCtx {
            inline_rulesets: &names,
            base_dir: Path::new("."),
            reach: Reach::Anywhere,
        };
        let text = "DOMAIN-SUFFIX,example.com // note\n";
        let parsed = parse_set(SetKind::RuleSet, text, &ctx);
        assert_eq!(parsed.skipped_total, 0, "{:?}", parsed.skipped);
        let set = CompiledSet::compile("t", SetKind::RuleSet, &parsed, &Stub::default(), 1);
        assert_eq!(
            ask(&set, "www.example.com", None, false),
            matches_line("DOMAIN-SUFFIX,example.com")
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
