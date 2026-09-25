//! Name → outbound resolution (M3 design §5, M1 design 6.2; phase 2 M3
//! design 5.5, 5.6). Built for every config generation and every
//! subscription update; `resolve` is a table walk with no allocation beyond
//! the chain and the group selections it reads.

use crate::assemble::Assembly;
use crate::auto::{AutoGroups, SelectCtx, Standing, fallback, load_balance, url_test};
use crate::cell::{ChainConnector, RegistryCell};
use crate::factory::{BuildError, OutboundFactory};
use crate::selections::SelectionTable;
use crate::testbook::{TestCase, TestResult};
use rurge_config::rule::PolicyRef;
use rurge_config::spec::{CommonOpts, GroupSpec, IpVersion, PolicySpec};
use rurge_config::{Builtin, Config, GroupKind, KeystoreType, PolicyKind, Span};
use rurge_net::connector::Connector;
use rurge_proto::{Direct, OutboundRef, Reject, RejectKind};
use rustls::RootCertStore;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// Deeper chains than this are treated as a defect (a group cycle resolves
/// to REJECT before it gets that deep).
pub const MAX_DEPTH: usize = 16;

/// What kind of outbound a resolution ended at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalKind {
    Direct,
    Reject,
    Proxy,
}

/// Why a resolution ended where it did, when that needs saying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// The terminal policy's protocol keyword (or `DEVICE`) is not
    /// implemented in this version: the outbound is REJECT.
    Unsupported(String),
    /// A group on a cycle of groups, written out (`A → B → A`): REJECT.
    GroupCycle(String),
    /// A group without members; `substituted` when DIRECT stood in for it.
    EmptyGroup { substituted: bool },
}

/// What the session log says (M3 design 5.6).
impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Note::Unsupported(kind) => write!(f, "policy protocol not implemented: {kind}"),
            Note::GroupCycle(cycle) => write!(f, "policy group cycle: {cycle}"),
            Note::EmptyGroup { substituted: true } => {
                f.write_str("policy group has no members; DIRECT substituted")
            }
            Note::EmptyGroup { substituted: false } => f.write_str("policy group has no members"),
        }
    }
}

/// What a group without members resolves to (M3-D3): DIRECT, as in Surge,
/// unless `--empty-group-reject` asks for REJECT.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EmptyGroup {
    #[default]
    Direct,
    Reject,
}

/// How a policy or group is written, for the control plane. Secrets
/// included: whoever shows it redacts it.
#[derive(Clone, PartialEq, Eq)]
pub struct Line {
    pub is_group: bool,
    /// A policy's type keyword or a group's kind keyword.
    pub keyword: &'static str,
    /// Right of `name =`: as written, as imported, or — for `M (via R)` —
    /// M's with `underlying-proxy` set.
    pub definition: String,
}

/// `definition` never appears: it may hold a password or a subscription
/// token (M3-D7, P18).
impl fmt::Debug for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Line")
            .field("is_group", &self.is_group)
            .field("keyword", &self.keyword)
            .finish_non_exhaustive()
    }
}

impl Line {
    fn policy(kind: PolicyKind, definition: &str) -> Line {
        Line {
            is_group: false,
            keyword: kind.keyword(),
            definition: definition.to_string(),
        }
    }
}

/// A group as the control plane shows it.
pub struct GroupInfo<'a> {
    pub kind: GroupKind,
    pub hidden: bool,
    /// As assembled.
    pub members: &'a [String],
}

#[derive(Clone)]
pub struct Resolution {
    pub chain: Vec<String>,
    pub outbound: OutboundRef,
    pub terminal: TerminalKind,
    pub note: Option<Note>,
    /// An `evaluate-before-use` group on the way that has not had its first
    /// round of tests: the dial waits for it and resolves again (M3 design
    /// 6.3). The outermost such group when there are several.
    pub pending: Option<String>,
}

enum Terminal {
    Direct,
    Reject(RejectKind),
}

enum Entry {
    /// A `direct` / `reject*` alias without options of its own.
    Alias(Terminal),
    /// A built proxy, or a `direct` alias with socket options.
    Outbound {
        outbound: OutboundRef,
        proxy: bool,
        // Boxed: `Fingerprint` embeds a whole `PolicySpec`, which would
        // otherwise make this variant much larger than the others.
        fingerprint: Box<Fingerprint>,
    },
    /// The protocol is not implemented yet: REJECT (W0007 at load).
    Unsupported { kind: PolicyKind },
    Group {
        spec: Arc<GroupSpec>,
        members: Vec<String>,
        /// The cycle it is on, written out: it resolves to REJECT.
        cycle: Option<String>,
    },
}

/// How a policy is tested (M3 design 6.1), worked out as the registry is
/// built.
struct TestSpec {
    /// `None`: the test URL does not parse, and the policy never passes.
    url: Option<Url>,
    timeout: Duration,
    /// What a result is good for (`TestCase::key`).
    key: u64,
}

impl TestSpec {
    fn new(definition: &str, url: &str, timeout: Duration) -> TestSpec {
        let mut h = DefaultHasher::new();
        (definition, url, timeout).hash(&mut h);
        TestSpec {
            url: Url::parse(url).ok(),
            timeout,
            key: h.finish(),
        }
    }
}

pub struct PolicyRegistry {
    entries: HashMap<String, Entry>,
    order: Vec<String>,
    lines: HashMap<String, Line>,
    direct: OutboundRef,
    rejects: [OutboundRef; 4],
    selections: Arc<SelectionTable>,
    empty_group: EmptyGroup,
    auto: Arc<AutoGroups>,
    /// By policy name; `DIRECT` for the built-in.
    tests: HashMap<String, TestSpec>,
    /// What verifies an `https` test URL (`OutboundFactory::roots`).
    roots: Arc<RootCertStore>,
}

/// What `build` fills in, name by name.
#[derive(Default)]
struct Table {
    entries: HashMap<String, Entry>,
    order: Vec<String>,
    lines: HashMap<String, Line>,
    tests: HashMap<String, TestSpec>,
}

impl Table {
    fn add(&mut self, name: &str, entry: Entry, line: Line) {
        self.entries.insert(name.to_string(), entry);
        self.order.push(name.to_string());
        self.lines.insert(name.to_string(), line);
    }
}

fn reject_slot(kind: RejectKind) -> usize {
    match kind {
        RejectKind::Reject => 0,
        RejectKind::Drop => 1,
        RejectKind::NoDrop => 2,
        RejectKind::TinyGif => 3,
    }
}

/// What the session log says about a policy that has no spec. Since M2b a
/// `vmess` policy of a profile that loaded is only ever without one for a
/// single reason: the line lacks `vmess-aead=true` (M2 design 4.3).
fn unsupported_text(kind: PolicyKind) -> String {
    match kind {
        PolicyKind::Vmess => "vmess (legacy handshake)".to_string(),
        other => other.keyword().to_string(),
    }
}

fn alias_terminal(kind: PolicyKind) -> Option<Terminal> {
    match kind {
        PolicyKind::Direct => Some(Terminal::Direct),
        PolicyKind::Reject => Some(Terminal::Reject(RejectKind::Reject)),
        PolicyKind::RejectDrop => Some(Terminal::Reject(RejectKind::Drop)),
        PolicyKind::RejectNoDrop => Some(Terminal::Reject(RejectKind::NoDrop)),
        PolicyKind::RejectTinyGif => Some(Terminal::Reject(RejectKind::TinyGif)),
        _ => None,
    }
}

/// Whether a `direct` alias needs a connector of its own.
/// `allow_other_interface` is not part of the predicate: it only says what to
/// do when `interface` is unavailable, so on its own it changes nothing.
fn has_socket_opts(common: &CommonOpts) -> bool {
    common.interface.is_some() || common.tos != 0 || common.ip_version != IpVersion::default()
}

/// Everything an outbound was built from. Two generations that agree on it
/// may share the outbound (M2 design 7.1). No `Debug`: it holds the policy's
/// credentials and the keystore item's.
#[derive(PartialEq)]
struct Fingerprint {
    /// Without its span: an unrelated edit above the line moves it.
    spec: PolicySpec,
    /// `client-cert`'s keystore item, by content.
    keystore: Option<(KeystoreType, String, Option<String>)>,
    environment: String,
}

fn fingerprint(spec: &PolicySpec, cfg: &Config, environment: &str) -> Fingerprint {
    let mut spec = spec.clone();
    spec.span = Span::new(Arc::from(Path::new("")), 0);
    let keystore = spec
        .proto
        .tls()
        .and_then(|tls| tls.client_cert.as_ref())
        .and_then(|name| cfg.keystore.iter().find(|item| &item.name == name))
        .map(|item| (item.kind, item.base64.clone(), item.password.clone()));
    Fingerprint {
        spec,
        keystore,
        environment: environment.to_string(),
    }
}

fn build_one(
    spec: &PolicySpec,
    factory: &dyn OutboundFactory,
    cell: &Arc<RegistryCell>,
) -> Result<OutboundRef, BuildError> {
    let connector: Arc<dyn Connector> = match spec.common.underlying_proxy.as_deref() {
        // This policy's own `interface` / `allow-other-interface` / `tos` /
        // `ip-version` have no effect here: socket options belong to the hop
        // that opens the socket, which is `name` or a hop below it.
        Some(name) => Arc::new(ChainConnector::new(cell.clone(), name)),
        None => factory.direct_connector(&spec.common),
    };
    factory
        .build(spec, connector)
        .map_err(|e| BuildError::new(format!("policy `{}`: {}", spec.name, e.message)))
}

/// Every group on a cycle, with the cycle written out; each cycle is said
/// once per build.
fn on_cycle(assembly: &Assembly) -> HashMap<&str, String> {
    let mut out = HashMap::new();
    for cycle in &assembly.cycles {
        let text = cycle.join(" → ");
        tracing::warn!(cycle = %text, "policy group cycle; the groups on it behave as REJECT");
        // the last group is the first one again
        for group in &cycle[..cycle.len().saturating_sub(1)] {
            out.entry(group.as_str()).or_insert_with(|| text.clone());
        }
    }
    out
}

impl PolicyRegistry {
    /// `cell` is where the chain connectors built here will look the
    /// registry up at dial time; the caller stores the result into it.
    /// `previous` is the generation being replaced: a policy whose
    /// fingerprint did not change keeps the outbound it had there, pools and
    /// all (M2 design 7.1). `previous` must have been built against this same
    /// `cell`: a reused outbound keeps the chain connectors it was built
    /// with, and they resolve through that cell. The groups take their
    /// members from `assembly`, which also brings the imported and the
    /// derived policies (M3 design 5.5). The automatic groups pick by the
    /// test results `auto` keeps (M3 design 6.4).
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        cfg: &Config,
        assembly: &Assembly,
        factory: &dyn OutboundFactory,
        cell: &Arc<RegistryCell>,
        selections: Arc<SelectionTable>,
        previous: Option<&PolicyRegistry>,
        empty_group: EmptyGroup,
        auto: &Arc<AutoGroups>,
    ) -> Result<PolicyRegistry, BuildError> {
        let direct: OutboundRef = Arc::new(Direct::new(
            factory.direct_connector(&CommonOpts::default()),
        ));
        let environment = factory.environment();
        let outbound_entry = |spec: &PolicySpec, proxy: bool| -> Result<Entry, BuildError> {
            let fingerprint = fingerprint(spec, cfg, &environment);
            let kept = match previous.and_then(|p| p.entries.get(&spec.name)) {
                Some(Entry::Outbound {
                    outbound,
                    fingerprint: before,
                    ..
                }) if **before == fingerprint => Some(outbound.clone()),
                _ => None,
            };
            let outbound = match kept {
                Some(outbound) => outbound,
                None => build_one(spec, factory, cell)?,
            };
            Ok(Entry::Outbound {
                outbound,
                proxy,
                fingerprint: Box::new(fingerprint),
            })
        };
        let policy_entry = |kind: PolicyKind, spec: Option<&PolicySpec>| {
            Ok::<Entry, BuildError>(match (alias_terminal(kind), spec) {
                (Some(Terminal::Direct), Some(spec)) if has_socket_opts(&spec.common) => {
                    outbound_entry(spec, false)?
                }
                (Some(terminal), _) => Entry::Alias(terminal),
                (None, Some(spec)) => outbound_entry(spec, true)?,
                // no spec: a protocol of a later milestone
                (None, None) => Entry::Unsupported { kind },
            })
        };
        // How each policy is tested: its own `test-url` / `test-timeout`,
        // else the profile's (M3 design 6.1). A REJECT and a protocol not
        // implemented never pass, so they have none.
        let test_spec = |kind: PolicyKind, spec: Option<&PolicySpec>, definition: &str| {
            let direct = match (alias_terminal(kind), spec) {
                (Some(Terminal::Direct), _) => true,
                (Some(Terminal::Reject(_)), _) | (None, None) => return None,
                (None, Some(_)) => false,
            };
            let common = spec.map(|s| &s.common);
            let (url, timeout) = cfg.general.test_target(
                common.and_then(|c| c.test_url.as_deref()),
                common.and_then(|c| c.test_timeout),
                direct,
            );
            Some(TestSpec::new(definition, url, timeout))
        };
        let mut table = Table::default();
        let (url, timeout) = cfg.general.test_target(None, None, true);
        table
            .tests
            .insert("DIRECT".to_string(), TestSpec::new("DIRECT", url, timeout));
        // The profile's own policies: the dry build has made a failure here a
        // load error, so one fails the whole generation.
        for p in &cfg.policies {
            let spec = cfg.spec(&p.name);
            let entry = policy_entry(p.kind, spec)?;
            table.add(&p.name, entry, Line::policy(p.kind, &p.definition));
            if let Some(t) = test_spec(p.kind, spec, &p.definition) {
                table.tests.insert(p.name.clone(), t);
            }
        }
        // What subscriptions brought in and the `M (via R)` of relayed groups:
        // a failure leaves that policy out and nothing else (M3-D6). Only the
        // name is logged: the factory's error text may quote a value of the
        // imported line or of the modifier (M3-D7).
        let left_out = |name: &str| {
            tracing::warn!(policy = %name, "policy cannot be built; it is left out");
        };
        for i in &assembly.imported {
            match policy_entry(i.policy.kind, i.spec.as_ref()) {
                Ok(entry) => {
                    table.add(
                        &i.policy.name,
                        entry,
                        Line::policy(i.policy.kind, &i.policy.definition),
                    );
                    if let Some(t) = test_spec(i.policy.kind, i.spec.as_ref(), &i.policy.definition)
                    {
                        table.tests.insert(i.policy.name.clone(), t);
                    }
                }
                Err(_) => left_out(&i.policy.name),
            }
        }
        for d in &assembly.derived {
            match outbound_entry(&d.spec, true) {
                Ok(entry) => {
                    table.add(
                        &d.spec.name,
                        entry,
                        Line::policy(d.spec.kind, &d.definition),
                    );
                    if let Some(t) = test_spec(d.spec.kind, Some(&d.spec), &d.definition) {
                        table.tests.insert(d.spec.name.clone(), t);
                    }
                }
                Err(_) => left_out(&d.spec.name),
            }
        }
        let on_cycle = on_cycle(assembly);
        let groups: HashSet<&str> = cfg.group_specs.iter().map(|g| g.name.as_str()).collect();
        for g in &cfg.group_specs {
            // a member whose policy was left out is left out too
            let members: Vec<String> = assembly
                .members_of(&g.name)
                .iter()
                .filter(|m| {
                    groups.contains(m.as_str())
                        || table.entries.contains_key(m.as_str())
                        || !matches!(PolicyRef::parse(m), PolicyRef::Named(_))
                })
                .cloned()
                .collect();
            let cycle = on_cycle.get(g.name.as_str()).cloned();
            if cycle.is_none() && members.is_empty() {
                let note = Note::EmptyGroup {
                    substituted: empty_group == EmptyGroup::Direct,
                };
                tracing::warn!(group = %g.name, "{note}");
            }
            let definition = cfg
                .groups
                .iter()
                .find(|written| written.name == g.name)
                .map(|written| written.definition.clone())
                .unwrap_or_default();
            let line = Line {
                is_group: true,
                keyword: g.kind.keyword(),
                definition,
            };
            let entry = Entry::Group {
                spec: Arc::new(g.clone()),
                members,
                cycle,
            };
            table.add(&g.name, entry, line);
        }
        let rejects = [
            Arc::new(Reject::new(RejectKind::Reject)) as OutboundRef,
            Arc::new(Reject::new(RejectKind::Drop)) as OutboundRef,
            Arc::new(Reject::new(RejectKind::NoDrop)) as OutboundRef,
            Arc::new(Reject::new(RejectKind::TinyGif)) as OutboundRef,
        ];
        Ok(PolicyRegistry {
            entries: table.entries,
            order: table.order,
            lines: table.lines,
            direct,
            rejects,
            selections,
            empty_group,
            auto: auto.clone(),
            tests: table.tests,
            roots: factory.roots(),
        })
    }

    pub fn direct(&self) -> OutboundRef {
        self.direct.clone()
    }

    pub fn reject(&self, kind: RejectKind) -> OutboundRef {
        self.rejects[reject_slot(kind)].clone()
    }

    pub fn names(&self) -> Vec<String> {
        self.order.clone()
    }

    /// Non-allocating membership check: every configured policy / group name
    /// (not builtins). Used on the per-connection dial path, where cloning
    /// the whole table via `names()` would allocate for every session.
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Every policy that is not a group, in the order they were built: the
    /// profile's, the imported ones, then the derived ones.
    pub fn policy_names(&self) -> Vec<String> {
        self.order
            .iter()
            .filter(|n| !matches!(self.entries.get(n.as_str()), Some(Entry::Group { .. })))
            .cloned()
            .collect()
    }

    /// Every group, in profile order.
    pub fn group_names(&self) -> Vec<String> {
        self.order
            .iter()
            .filter(|n| matches!(self.entries.get(n.as_str()), Some(Entry::Group { .. })))
            .cloned()
            .collect()
    }

    pub fn group(&self, name: &str) -> Option<GroupInfo<'_>> {
        match self.entries.get(name)? {
            Entry::Group { spec, members, .. } => Some(GroupInfo {
                kind: spec.kind,
                hidden: spec.hidden,
                members,
            }),
            _ => None,
        }
    }

    /// The group's definition, as the automatic groups' overrides keep it.
    pub fn group_spec(&self, name: &str) -> Option<&GroupSpec> {
        match self.entries.get(name)? {
            Entry::Group { spec, .. } => Some(spec),
            _ => None,
        }
    }

    /// The automatic groups' state this registry picks by.
    pub fn auto(&self) -> &Arc<AutoGroups> {
        &self.auto
    }

    /// The members of `group` as assembled; `None` when it is not a group.
    pub fn members(&self, group: &str) -> Option<&[String]> {
        self.group(group).map(|g| g.members)
    }

    /// How `name` is written; `None` for a built-in.
    pub fn line(&self, name: &str) -> Option<&Line> {
        self.lines.get(name)
    }

    /// What `name` was built from: a proxy, or a `direct` alias with socket
    /// options of its own.
    pub fn spec(&self, name: &str) -> Option<&PolicySpec> {
        match self.entries.get(name)? {
            Entry::Outbound { fingerprint, .. } => Some(&fingerprint.spec),
            _ => None,
        }
    }

    /// The member `group` points at right now, as the control plane shows
    /// it: nothing changes for asking (no test is started, `url-test` does
    /// not move). `None` when `group` is not a group or has no members.
    pub fn current_member(&self, group: &str) -> Option<String> {
        self.choose(group, &SelectCtx::default(), false, 0)
            .map(|(member, _)| member)
    }

    /// The member of `group`, as a dial picks it. `select`: the live
    /// selection while it names a member, else the first member. The
    /// automatic groups: an override while it names a member, else by the
    /// test results (M3 design 6.4) — and when those are older than the
    /// group's `interval`, or a member that is not a group has none, a round
    /// is asked for. `live` is a dial: only then may `url-test` move the
    /// member it holds and a round be asked for; the second value then says
    /// whether the group wants its first round before it is used
    /// (`evaluate-before-use`).
    fn choose(
        &self,
        group: &str,
        ctx: &SelectCtx,
        live: bool,
        depth: usize,
    ) -> Option<(String, bool)> {
        let Some(Entry::Group { spec, members, .. }) = self.entries.get(group) else {
            return None;
        };
        match spec.kind {
            GroupKind::Select => {
                let selected = self.selections.get(group).filter(|m| members.contains(m));
                selected
                    .or_else(|| members.first().cloned())
                    .map(|m| (m, false))
            }
            GroupKind::UrlTest | GroupKind::Fallback | GroupKind::LoadBalance => {
                // an override stands, and asks for no test (M3 design 6.3)
                if let Some(member) = self.auto.override_of(group, members) {
                    return Some((member, false));
                }
                let last = self.auto.last_round(group);
                let pending = live && spec.test.evaluate_before_use && last.is_none();
                let standings: Vec<(String, Standing)> = members
                    .iter()
                    .map(|m| (m.clone(), self.standing(m, depth + 1)))
                    .collect();
                // a member that is not a group and has no result for what it
                // is now — a new one, or one a reload or a subscription
                // update changed — asks for a round too (M3 design 6.3)
                let untested = standings.iter().any(|(m, s)| {
                    *s == Standing::Unknown
                        && !matches!(self.entries.get(m.as_str()), Some(Entry::Group { .. }))
                });
                if live && (untested || last.is_none_or(|t| t.elapsed() >= spec.test.interval)) {
                    self.auto.wake(group);
                }
                let member = match spec.kind {
                    GroupKind::UrlTest => {
                        let pick =
                            url_test(&standings, self.auto.pick(group).as_deref(), &spec.test)?;
                        if live {
                            self.auto.set_pick(group, &pick);
                        }
                        pick
                    }
                    GroupKind::Fallback => fallback(&standings, &spec.test)?,
                    _ if live => load_balance(&standings, &spec.test, ctx)?,
                    // the views: the first that passes stands for the group
                    _ => fallback(&standings, &spec.test)?,
                };
                Some((member, pending))
            }
            // `smart` (M3c) and `subnet` (phase 3): the first member
            GroupKind::Smart | GroupKind::Subnet => members.first().map(|m| (m.clone(), false)),
        }
    }

    /// What the tests say of `name`: its own last result, or — a group — its
    /// pick's, the average of those that pass for `load-balance` (M3 design
    /// 6.4). A REJECT, a protocol not implemented and a test URL that does
    /// not parse never pass.
    fn standing(&self, name: &str, depth: usize) -> Standing {
        if depth > MAX_DEPTH {
            return Standing::Failed;
        }
        if let PolicyRef::Named(n) = PolicyRef::parse(name)
            && let Some(Entry::Group {
                spec,
                members,
                cycle,
            }) = self.entries.get(&n)
        {
            if cycle.is_some() {
                return Standing::Failed;
            }
            if spec.kind == GroupKind::LoadBalance {
                let all: Vec<Standing> = members
                    .iter()
                    .map(|m| self.standing(m, depth + 1))
                    .collect();
                let passing: Vec<Duration> =
                    all.iter().filter_map(|s| s.passes(&spec.test)).collect();
                if passing.is_empty() {
                    return if all.iter().all(|s| *s == Standing::Unknown) {
                        Standing::Unknown
                    } else {
                        Standing::Failed
                    };
                }
                return Standing::Passed(passing.iter().sum::<Duration>() / passing.len() as u32);
            }
            return match self.choose(&n, &SelectCtx::default(), false, depth) {
                Some((member, _)) => self.standing(&member, depth + 1),
                None => Standing::Unknown,
            };
        }
        match self.test_case(name) {
            Some(case) => match self.auto.tests.result(&case.policy, case.key) {
                Some(result) => match result.outcome {
                    Ok(score) => Standing::Passed(score),
                    Err(_) => Standing::Failed,
                },
                None => Standing::Unknown,
            },
            None => Standing::Failed,
        }
    }

    /// How to test `name` now (M3 design 6.1): through its outbound, at its
    /// test URL. `None` for what never passes — a REJECT, a protocol not
    /// implemented, a test URL that does not parse — and for a group.
    pub fn test_case(&self, name: &str) -> Option<TestCase> {
        let (policy, outbound) = match PolicyRef::parse(name) {
            // DIRECT, and on the desktop the iOS-only built-ins that stand
            // in for it
            PolicyRef::Builtin(b) if RejectKind::from_builtin(b).is_none() => {
                ("DIRECT".to_string(), self.direct())
            }
            PolicyRef::Builtin(_) | PolicyRef::Device(_) => return None,
            PolicyRef::Named(n) => match self.entries.get(&n)? {
                Entry::Alias(Terminal::Direct) => (n, self.direct()),
                Entry::Outbound { outbound, .. } => {
                    let outbound = outbound.clone();
                    (n, outbound)
                }
                _ => return None,
            },
        };
        let test = self.tests.get(&policy)?;
        Some(TestCase {
            url: test.url.clone()?,
            timeout: test.timeout,
            key: test.key,
            roots: self.roots.clone(),
            policy,
            outbound,
        })
    }

    /// The last test result of `name` that still counts.
    pub fn test_result(&self, name: &str) -> Option<TestResult> {
        let case = self.test_case(name)?;
        self.auto.tests.result(&case.policy, case.key)
    }

    /// The members of `group` that pass their tests now.
    pub fn available(&self, group: &str) -> Vec<String> {
        let Some(Entry::Group { spec, members, .. }) = self.entries.get(group) else {
            return Vec::new();
        };
        members
            .iter()
            .filter(|m| self.standing(m, 1).passes(&spec.test).is_some())
            .cloned()
            .collect()
    }

    /// Tests every member of `group` now — the members of the groups in it
    /// too — and records the round for each of those groups; the members of
    /// `group` that pass (M3 design 6.3, 6.6). Every test runs on its own
    /// task (`TestBook::test`).
    pub async fn test_group(&self, group: &str) -> Vec<String> {
        let mut groups = Vec::new();
        let mut policies = Vec::new();
        self.gather(group, 0, &mut groups, &mut policies);
        if groups.is_empty() {
            // a reload took the group away after the round was asked for:
            // the request still ends, or it would stand in the way of the
            // next one for a group of that name
            groups.push(group.to_string());
        }
        let tests: Vec<_> = policies
            .iter()
            .filter_map(|p| self.test_case(p))
            .map(|case| {
                let book = self.auto.tests.clone();
                tokio::spawn(async move { book.test(case).await })
            })
            .collect();
        for test in tests {
            let _ = test.await;
        }
        self.auto.round_done(&groups);
        self.available(group)
    }

    /// The groups a round of `group` covers and the policies it tests.
    fn gather(
        &self,
        group: &str,
        depth: usize,
        groups: &mut Vec<String>,
        policies: &mut Vec<String>,
    ) {
        if depth > MAX_DEPTH || groups.iter().any(|g| g == group) {
            return;
        }
        let Some(Entry::Group { members, cycle, .. }) = self.entries.get(group) else {
            return;
        };
        groups.push(group.to_string());
        if cycle.is_some() {
            return;
        }
        for m in members {
            if let Some(Entry::Group { .. }) = self.entries.get(m.as_str()) {
                self.gather(m, depth + 1, groups, policies);
            } else if !policies.contains(m) {
                policies.push(m.clone());
            }
        }
    }

    pub fn resolve(&self, policy: &PolicyRef) -> Resolution {
        self.resolve_with(policy, &SelectCtx::default())
    }

    /// `resolve`, for a dial that knows its target: `load-balance` with
    /// `persistent=true` picks by the host (M3 design 6.4).
    pub fn resolve_with(&self, policy: &PolicyRef, ctx: &SelectCtx) -> Resolution {
        let mut chain = Vec::new();
        match policy {
            PolicyRef::Builtin(b) => self.builtin(*b, &mut chain),
            PolicyRef::Device(name) => self.device(name, &mut chain),
            PolicyRef::Named(name) => self.named(name, &mut chain, 0, self.empty_group, ctx),
        }
    }

    /// `name` as the `underlying-proxy` of another policy. A relay is set so
    /// that traffic does not leave directly: a group without members refuses
    /// here, whatever `EmptyGroup` says for a group dialled for itself.
    pub fn resolve_relay(&self, name: &str) -> Resolution {
        self.named(
            name,
            &mut Vec::new(),
            0,
            EmptyGroup::Reject,
            &SelectCtx::default(),
        )
    }

    fn device(&self, name: &str, chain: &mut Vec<String>) -> Resolution {
        chain.push(format!("DEVICE:{name}"));
        self.rejected(chain, Some(Note::Unsupported("DEVICE".to_string())))
    }

    fn builtin(&self, b: Builtin, chain: &mut Vec<String>) -> Resolution {
        chain.push(b.name().to_string());
        if b == Builtin::Direct {
            return self.done(chain, self.direct(), TerminalKind::Direct, None);
        }
        if let Some(kind) = RejectKind::from_builtin(b) {
            return self.done(chain, self.reject(kind), TerminalKind::Reject, None);
        }
        // CELLULAR / CELLULAR-ONLY / HYBRID / NO-HYBRID: iOS-only, DIRECT on desktop (W0009 at load).
        chain.push("DIRECT".to_string());
        self.done(chain, self.direct(), TerminalKind::Direct, None)
    }

    fn named(
        &self,
        name: &str,
        chain: &mut Vec<String>,
        depth: usize,
        empty: EmptyGroup,
        ctx: &SelectCtx,
    ) -> Resolution {
        chain.push(name.to_string());
        if depth > MAX_DEPTH {
            tracing::error!(
                policy = name,
                "policy chain deeper than {MAX_DEPTH}; treating as REJECT"
            );
            return self.rejected(chain, None);
        }
        match self.entries.get(name) {
            None => {
                tracing::error!(
                    policy = name,
                    "policy not found in registry; treating as REJECT"
                );
                self.rejected(chain, None)
            }
            Some(Entry::Alias(Terminal::Direct)) => {
                chain.push("DIRECT".to_string());
                self.done(chain, self.direct(), TerminalKind::Direct, None)
            }
            Some(Entry::Alias(Terminal::Reject(kind))) => {
                chain.push(kind.name().to_string());
                self.done(chain, self.reject(*kind), TerminalKind::Reject, None)
            }
            Some(Entry::Outbound {
                outbound,
                proxy: true,
                ..
            }) => self.done(chain, outbound.clone(), TerminalKind::Proxy, None),
            Some(Entry::Outbound {
                outbound,
                proxy: false,
                ..
            }) => {
                chain.push("DIRECT".to_string());
                self.done(chain, outbound.clone(), TerminalKind::Direct, None)
            }
            Some(Entry::Unsupported { kind }) => {
                chain.push(format!("!unsupported:{}", kind.keyword()));
                self.rejected(chain, Some(Note::Unsupported(unsupported_text(*kind))))
            }
            Some(Entry::Group {
                cycle: Some(cycle), ..
            }) => self.rejected(chain, Some(Note::GroupCycle(cycle.clone()))),
            Some(Entry::Group { .. }) => match self.choose(name, ctx, true, depth) {
                Some((member, pending)) => {
                    let mut resolution = match PolicyRef::parse(&member) {
                        PolicyRef::Builtin(b) => self.builtin(b, chain),
                        PolicyRef::Device(d) => self.device(&d, chain),
                        PolicyRef::Named(n) => self.named(&n, chain, depth + 1, empty, ctx),
                    };
                    if pending {
                        resolution.pending = Some(name.to_string());
                    }
                    resolution
                }
                None => self.empty(chain, empty),
            },
        }
    }

    /// A group without members: DIRECT stands in, or REJECT (M3-D3).
    fn empty(&self, chain: &mut Vec<String>, empty: EmptyGroup) -> Resolution {
        match empty {
            EmptyGroup::Direct => {
                chain.push("DIRECT".to_string());
                let note = Note::EmptyGroup { substituted: true };
                self.done(chain, self.direct(), TerminalKind::Direct, Some(note))
            }
            EmptyGroup::Reject => {
                self.rejected(chain, Some(Note::EmptyGroup { substituted: false }))
            }
        }
    }

    fn rejected(&self, chain: &mut Vec<String>, note: Option<Note>) -> Resolution {
        chain.push(RejectKind::Reject.name().to_string());
        self.done(
            chain,
            self.reject(RejectKind::Reject),
            TerminalKind::Reject,
            note,
        )
    }

    fn done(
        &self,
        chain: &mut Vec<String>,
        outbound: OutboundRef,
        terminal: TerminalKind,
        note: Option<Note>,
    ) -> Resolution {
        Resolution {
            chain: std::mem::take(chain),
            outbound,
            terminal,
            note,
            pending: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assemble::{Snapshots, assemble};
    use crate::selections::GroupSelections;
    use crate::subscription;
    use crate::testing::FakeFactory;
    use rurge_config::HostName;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::connector::{ConnectOpts, Target};
    use std::path::Path;

    const PROFILE: &str = "[General]\nloglevel = notify\n[Proxy]\n\
HK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n\
D = direct\nCorp = direct, interface=eth9\nBlock = reject-tinygif\n\
EntryA = socks5, a.example, 1080\nEntryB = socks5, b.example, 1080\n\
Exit = http, exit.example, 8080, underlying-proxy=Hop\n\
Mid = socks5, m.example, 1080, underlying-proxy=EntryA\n\
Deep = http, d.example, 80, underlying-proxy=Mid\n\
[Proxy Group]\nAuto = url-test, HK, D\nPick = select, HK, D, DIRECT\nOuter = select, Pick, Auto\n\
Emptyish = select, Block\nHop = select, EntryA, EntryB\n[Rule]\nFINAL,Pick\n";

    struct Built {
        registry: Arc<PolicyRegistry>,
        factory: FakeFactory,
        table: Arc<SelectionTable>,
    }

    fn built(selections: GroupSelections) -> Built {
        let loaded = from_text(PROFILE, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        let factory = FakeFactory::new();
        let cell = RegistryCell::new();
        let table = Arc::new(SelectionTable::new(selections));
        let registry = Arc::new(
            PolicyRegistry::build(
                &loaded.config,
                &assemble(&loaded.config, &Snapshots::new()),
                &factory,
                &cell,
                table.clone(),
                None,
                EmptyGroup::Direct,
                &crate::testing::auto_groups(),
            )
            .expect("builds"),
        );
        cell.store(registry.clone());
        Built {
            registry,
            factory,
            table,
        }
    }

    fn chain(r: &Resolution) -> Vec<&str> {
        r.chain.iter().map(String::as_str).collect()
    }

    #[test]
    fn builtins_and_aliases() {
        let reg = built(GroupSelections::new()).registry;
        let d = reg.resolve(&PolicyRef::Builtin(Builtin::Direct));
        assert_eq!(
            (chain(&d), d.outbound.name(), d.terminal, d.note.clone()),
            (vec!["DIRECT"], "DIRECT", TerminalKind::Direct, None)
        );
        let r = reg.resolve(&PolicyRef::Builtin(Builtin::RejectTinyGif));
        assert_eq!(
            (chain(&r), r.outbound.name(), r.terminal),
            (
                vec!["REJECT-TINYGIF"],
                "REJECT-TINYGIF",
                TerminalKind::Reject
            )
        );
        let cell = reg.resolve(&PolicyRef::Builtin(Builtin::Cellular));
        assert_eq!(
            (chain(&cell), cell.outbound.name()),
            (vec!["CELLULAR", "DIRECT"], "DIRECT")
        );
        // a plain alias shares the built-in DIRECT …
        let alias = reg.resolve(&PolicyRef::parse("D"));
        assert_eq!(chain(&alias), vec!["D", "DIRECT"]);
        assert!(Arc::ptr_eq(&alias.outbound, &reg.direct()));
        // … an alias with socket options owns its outbound
        let corp = reg.resolve(&PolicyRef::parse("Corp"));
        assert_eq!(
            (chain(&corp), corp.outbound.name(), corp.terminal),
            (vec!["Corp", "DIRECT"], "DIRECT", TerminalKind::Direct)
        );
        assert!(!Arc::ptr_eq(&corp.outbound, &reg.direct()));
        let block = reg.resolve(&PolicyRef::parse("Block"));
        assert_eq!(
            (chain(&block), block.outbound.name(), block.terminal),
            (
                vec!["Block", "REJECT-TINYGIF"],
                "REJECT-TINYGIF",
                TerminalKind::Reject
            )
        );
        assert_eq!(
            reg.names(),
            vec![
                "HK", "D", "Corp", "Block", "EntryA", "EntryB", "Exit", "Mid", "Deep", "Auto",
                "Pick", "Outer", "Emptyish", "Hop"
            ]
        );
        assert!(reg.contains("HK") && !reg.contains("Nope"));
    }

    #[test]
    fn a_proxy_policy_resolves_to_its_own_outbound() {
        let reg = built(GroupSelections::new()).registry;
        let a = reg.resolve(&PolicyRef::parse("EntryA"));
        assert_eq!(
            (chain(&a), a.outbound.name(), a.terminal, a.note.clone()),
            (vec!["EntryA"], "EntryA", TerminalKind::Proxy, None)
        );
        let hop = reg.resolve(&PolicyRef::parse("Hop"));
        assert_eq!(
            (chain(&hop), hop.terminal),
            (vec!["Hop", "EntryA"], TerminalKind::Proxy)
        );
    }

    #[test]
    fn unsupported_protocols_and_devices_reject_with_a_note() {
        let reg = built(GroupSelections::new()).registry;
        let hk = reg.resolve(&PolicyRef::parse("HK"));
        assert_eq!(chain(&hk), vec!["HK", "!unsupported:ss", "REJECT"]);
        assert_eq!(
            (hk.outbound.name(), hk.terminal, hk.note.clone()),
            (
                "REJECT",
                TerminalKind::Reject,
                Some(Note::Unsupported("ss".into()))
            )
        );
        let dev = reg.resolve(&PolicyRef::parse("DEVICE:Living Room"));
        assert_eq!(chain(&dev), vec!["DEVICE:Living Room", "REJECT"]);
        assert_eq!(dev.note, Some(Note::Unsupported("DEVICE".into())));
        let missing = reg.resolve(&PolicyRef::Named("Nope".to_string()));
        assert_eq!(
            (chain(&missing), missing.note.clone()),
            (vec!["Nope", "REJECT"], None)
        );
        // a group falling through to an unsupported member keeps the whole
        // chain, whichever kind of group it is
        assert_eq!(
            chain(&reg.resolve(&PolicyRef::parse("Pick"))),
            vec!["Pick", "HK", "!unsupported:ss", "REJECT"]
        );
        assert_eq!(
            chain(&reg.resolve(&PolicyRef::parse("Auto"))),
            vec!["Auto", "HK", "!unsupported:ss", "REJECT"]
        );
    }

    #[test]
    fn a_legacy_vmess_policy_says_why_it_rejects() {
        let text = "[Proxy]\nOld = vmess, a.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119\n\
SS = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n";
        let registry = generation(text, &FakeFactory::new(), None);
        let old = registry.resolve(&PolicyRef::parse("Old"));
        assert_eq!(old.terminal, TerminalKind::Reject);
        assert_eq!(
            old.note,
            Some(Note::Unsupported("vmess (legacy handshake)".into()))
        );
        assert_eq!(chain(&old), ["Old", "!unsupported:vmess", "REJECT"]);
        let ss = registry.resolve(&PolicyRef::parse("SS"));
        assert_eq!(ss.note, Some(Note::Unsupported("ss".into())));
    }

    #[test]
    fn groups_read_the_live_table() {
        let mut saved = GroupSelections::new();
        saved.set("Pick", "DIRECT");
        saved.set("Outer", "Pick");
        saved.set("Auto", "D"); // ignored: not a select group
        saved.set("Emptyish", "Gone"); // not a member any more
        let b = built(saved);
        let reg = &b.registry;
        let picked = reg.resolve(&PolicyRef::parse("Pick"));
        assert_eq!(chain(&picked), vec!["Pick", "DIRECT"]);
        assert_eq!(picked.outbound.name(), "DIRECT");
        assert_eq!(
            chain(&reg.resolve(&PolicyRef::parse("Outer"))),
            vec!["Outer", "Pick", "DIRECT"]
        );
        assert_eq!(chain(&reg.resolve(&PolicyRef::parse("Auto")))[1], "HK");
        let emptyish = reg.resolve(&PolicyRef::parse("Emptyish"));
        assert_eq!(
            (chain(&emptyish), emptyish.note.clone()),
            (vec!["Emptyish", "Block", "REJECT-TINYGIF"], None)
        );
        assert_eq!(reg.current_member("Pick").as_deref(), Some("DIRECT"));
        assert_eq!(reg.current_member("Emptyish").as_deref(), Some("Block"));
        assert_eq!(reg.current_member("Auto").as_deref(), Some("HK"));
        assert_eq!(reg.current_member("D"), None, "not a group");
        // no rebuild: the very next resolve sees the change
        b.table.set("Pick", "D");
        assert_eq!(
            chain(&reg.resolve(&PolicyRef::parse("Pick"))),
            vec!["Pick", "D", "DIRECT"]
        );
        assert_eq!(reg.current_member("Pick").as_deref(), Some("D"));
    }

    /// M1 design §8: switch the selection between two dials and the entry
    /// node of the chain follows.
    #[tokio::test]
    async fn the_entry_of_a_chain_follows_the_group_selection() {
        let b = built(GroupSelections::new());
        let exit = b.registry.resolve(&PolicyRef::parse("Exit"));
        assert_eq!(
            (chain(&exit), exit.terminal),
            (vec!["Exit"], TerminalKind::Proxy)
        );
        let target = Target::new(HostName::parse("site.example"), 443);
        exit.outbound
            .connect_tcp(&target, &ConnectOpts::default())
            .await
            .unwrap();
        b.table.set("Hop", "EntryB");
        exit.outbound
            .connect_tcp(&target, &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(
            b.factory.connector.seen(),
            [
                "Exit -> site.example:443",
                // the exit's server is reached through the entry, by name
                "EntryA -> exit.example:8080",
                "dial a.example:1080",
                "Exit -> site.example:443",
                "EntryB -> exit.example:8080",
                "dial b.example:1080",
            ]
        );
    }

    /// Three hops: each one dials the hop below it, and only the bottom hop
    /// reaches a real socket — with its own server as the target.
    #[tokio::test]
    async fn a_three_hop_chain_hands_each_server_to_the_hop_below() {
        let b = built(GroupSelections::new());
        let deep = b.registry.resolve(&PolicyRef::parse("Deep"));
        assert_eq!(
            (chain(&deep), deep.terminal),
            (vec!["Deep"], TerminalKind::Proxy)
        );
        deep.outbound
            .connect_tcp(
                &Target::new(HostName::parse("site.example"), 443),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            b.factory.connector.seen(),
            [
                "Deep -> site.example:443",
                "Mid -> d.example:80",
                "EntryA -> m.example:1080",
                "dial a.example:1080",
            ]
        );
    }

    #[test]
    fn a_policy_that_cannot_be_built_fails_the_whole_registry() {
        let loaded = from_text(PROFILE, Path::new("t.conf"), &LoadOptions::for_tests());
        let factory = FakeFactory {
            broken: Some("EntryB"),
            ..FakeFactory::new()
        };
        let e = PolicyRegistry::build(
            &loaded.config,
            &assemble(&loaded.config, &Snapshots::new()),
            &factory,
            &RegistryCell::new(),
            Arc::new(SelectionTable::default()),
            None,
            EmptyGroup::Direct,
            &crate::testing::auto_groups(),
        )
        .err()
        .expect("EntryB does not build");
        assert_eq!(e.message, "policy `EntryB`: boom");
    }

    fn generation(
        text: &str,
        factory: &FakeFactory,
        previous: Option<&PolicyRegistry>,
    ) -> PolicyRegistry {
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        PolicyRegistry::build(
            &loaded.config,
            &assemble(&loaded.config, &Snapshots::new()),
            factory,
            &RegistryCell::new(),
            Arc::new(SelectionTable::new(GroupSelections::new())),
            previous,
            EmptyGroup::Direct,
            &crate::testing::auto_groups(),
        )
        .expect("builds")
    }

    fn outbound_of(registry: &PolicyRegistry, name: &str) -> OutboundRef {
        registry.resolve(&PolicyRef::parse(name)).outbound
    }

    const REUSE: &str = "[Proxy]\nA = socks5, a.example, 1080, username=u, password=p\n\
B = https, b.example, 443, client-cert=cert1\nCorp = direct, interface=eth9\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=x\n[Rule]\nFINAL,DIRECT\n";

    #[test]
    fn an_untouched_policy_keeps_its_outbound_across_a_reload() {
        let factory = FakeFactory::new();
        let first = generation(REUSE, &factory, None);
        // an unrelated line is added above: every span moves, nothing else does
        let moved = REUSE.replace("[Proxy]\n", "[Proxy]\nNew = http, n.example, 80\n");
        let second = generation(&moved, &factory, Some(&first));
        for name in ["A", "B", "Corp"] {
            assert!(
                Arc::ptr_eq(&outbound_of(&first, name), &outbound_of(&second, name)),
                "{name} was rebuilt"
            );
        }
        // without a previous generation everything is new
        let alone = generation(&moved, &factory, None);
        assert!(!Arc::ptr_eq(
            &outbound_of(&first, "A"),
            &outbound_of(&alone, "A")
        ));
    }

    #[test]
    fn a_changed_parameter_keystore_item_or_environment_rebuilds() {
        let factory = FakeFactory::new();
        let first = generation(REUSE, &factory, None);
        // A's own parameter
        let second = generation(
            &REUSE.replace("password=p", "password=q"),
            &factory,
            Some(&first),
        );
        assert!(!Arc::ptr_eq(
            &outbound_of(&first, "A"),
            &outbound_of(&second, "A")
        ));
        assert!(Arc::ptr_eq(
            &outbound_of(&first, "B"),
            &outbound_of(&second, "B")
        ));
        // the content of the keystore item B refers to
        let second = generation(
            &REUSE.replace("base64=QUJD", "base64=QUJE"),
            &factory,
            Some(&first),
        );
        assert!(!Arc::ptr_eq(
            &outbound_of(&first, "B"),
            &outbound_of(&second, "B")
        ));
        assert!(Arc::ptr_eq(
            &outbound_of(&first, "A"),
            &outbound_of(&second, "A")
        ));
        // what the factory captured by value
        let other = FakeFactory {
            environment: "other",
            ..FakeFactory::new()
        };
        let second = generation(REUSE, &other, Some(&first));
        for name in ["A", "B", "Corp"] {
            assert!(
                !Arc::ptr_eq(&outbound_of(&first, name), &outbound_of(&second, name)),
                "{name} survived a change of environment"
            );
        }
    }

    const SUBSCRIBED: &str = "[Proxy]\nRelay = http, r.example, 80\nA = http, a.example, 80\n\
[Proxy Group]\nSub = select, A, policy-path=https://sub.example/nodes, underlying-proxy=Relay, hidden=true\n\
Plain = select, DIRECT, policy-path=https://sub.example/nodes\n[Rule]\nFINAL,Sub\n";

    /// One generation of `SUBSCRIBED`, the subscription serving `nodes`,
    /// built on `previous` into `cell`.
    fn subscribed(
        nodes: &str,
        factory: &FakeFactory,
        cell: &Arc<RegistryCell>,
        previous: Option<&PolicyRegistry>,
    ) -> PolicyRegistry {
        let loaded = from_text(SUBSCRIBED, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        let cfg = loaded.config;
        let path = cfg.group_specs[0].import.policy_path.clone().unwrap();
        let snapshots = Snapshots::from([(path, Arc::new(subscription::parse(nodes)))]);
        PolicyRegistry::build(
            &cfg,
            &assemble(&cfg, &snapshots),
            factory,
            cell,
            Arc::new(SelectionTable::default()),
            previous,
            EmptyGroup::Direct,
            &crate::testing::auto_groups(),
        )
        .expect("builds")
    }

    #[test]
    fn imported_and_derived_policies_are_entries_of_their_own() {
        let factory = FakeFactory::new();
        let reg = subscribed(
            "N1 = http, n1.example, 80\nN2 = socks5, n2.example, 1080",
            &factory,
            &RegistryCell::new(),
            None,
        );
        assert_eq!(
            reg.policy_names(),
            [
                "Relay",
                "A",
                "N1",
                "N2",
                "A (via Relay)",
                "N1 (via Relay)",
                "N2 (via Relay)"
            ]
        );
        assert_eq!(reg.group_names(), ["Sub", "Plain"]);
        let sub = reg.group("Sub").unwrap();
        assert_eq!(
            (sub.kind, sub.hidden, sub.members),
            (
                GroupKind::Select,
                true,
                &[
                    "A (via Relay)".to_string(),
                    "N1 (via Relay)".to_string(),
                    "N2 (via Relay)".to_string()
                ][..]
            )
        );
        assert_eq!(
            reg.members("Plain"),
            Some(&["DIRECT".to_string(), "N1".to_string(), "N2".to_string()][..])
        );
        let n2 = reg.line("N2").unwrap();
        assert_eq!(
            (n2.is_group, n2.keyword, n2.definition.as_str()),
            (false, "socks5", "socks5, n2.example, 1080")
        );
        assert_eq!(
            reg.line("N1 (via Relay)").unwrap().definition,
            "http, n1.example, 80, underlying-proxy=Relay"
        );
        assert!(reg.line("Sub").unwrap().is_group);
        assert_eq!(reg.line("DIRECT"), None);
        assert_eq!(
            reg.spec("N1 (via Relay)")
                .unwrap()
                .common
                .underlying_proxy
                .as_deref(),
            Some("Relay")
        );
        assert!(reg.spec("Sub").is_none());
    }

    /// A subscription update keeps every outbound whose line did not change,
    /// the derived ones included (M3 design 5.7).
    #[test]
    fn an_update_keeps_the_outbounds_of_lines_that_did_not_change() {
        let factory = FakeFactory::new();
        let cell = RegistryCell::new();
        let first = subscribed(
            "N1 = http, n1.example, 80\nN2 = http, n2.example, 80",
            &factory,
            &cell,
            None,
        );
        let second = subscribed(
            "N1 = http, n1.example, 80\nN2 = http, moved.example, 80\nN3 = http, n3.example, 80",
            &factory,
            &cell,
            Some(&first),
        );
        for same in ["N1", "N1 (via Relay)", "A", "A (via Relay)", "Relay"] {
            assert!(
                Arc::ptr_eq(&outbound_of(&first, same), &outbound_of(&second, same)),
                "{same} was rebuilt"
            );
        }
        for changed in ["N2", "N2 (via Relay)"] {
            assert!(!Arc::ptr_eq(
                &outbound_of(&first, changed),
                &outbound_of(&second, changed)
            ));
        }
        assert!(second.contains("N3 (via Relay)"));
    }

    #[tokio::test]
    async fn a_derived_policy_dials_through_the_group_relay() {
        let factory = FakeFactory::new();
        let cell = RegistryCell::new();
        let reg = Arc::new(subscribed(
            "N1 = http, n1.example, 80",
            &factory,
            &cell,
            None,
        ));
        cell.store(reg.clone());
        let n1 = reg.resolve(&PolicyRef::parse("Sub"));
        assert_eq!(
            (chain(&n1), n1.terminal),
            (vec!["Sub", "A (via Relay)"], TerminalKind::Proxy)
        );
        reg.resolve(&PolicyRef::parse("N1 (via Relay)"))
            .outbound
            .connect_tcp(
                &Target::new(HostName::parse("site.example"), 443),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            factory.connector.seen(),
            [
                "N1 (via Relay) -> site.example:443",
                "Relay -> n1.example:80",
                "dial r.example:80",
            ]
        );
        cell.clear();
    }

    #[test]
    fn an_imported_policy_that_cannot_be_built_is_left_out() {
        let factory = FakeFactory {
            broken: Some("N2"),
            ..FakeFactory::new()
        };
        let reg = subscribed(
            "N1 = http, n1.example, 80\nN2 = http, n2.example, 80",
            &factory,
            &RegistryCell::new(),
            None,
        );
        assert!(!reg.contains("N2"));
        assert_eq!(
            reg.members("Plain"),
            Some(&["DIRECT".to_string(), "N1".to_string()][..])
        );
    }

    /// A derived `M (via R)` that cannot be built is left out just like an
    /// imported policy: the group keeps its other members.
    #[test]
    fn a_derived_policy_that_cannot_be_built_is_left_out_of_its_group() {
        let factory = FakeFactory {
            broken: Some("A (via Relay)"),
            ..FakeFactory::new()
        };
        let reg = subscribed(
            "N1 = http, n1.example, 80",
            &factory,
            &RegistryCell::new(),
            None,
        );
        assert!(!reg.contains("A (via Relay)"));
        assert_eq!(
            reg.members("Sub"),
            Some(&["N1 (via Relay)".to_string()][..])
        );
    }

    #[test]
    fn a_group_on_a_cycle_rejects_and_names_the_cycle() {
        let text = "[Proxy]\nA = http, a.example, 80\n[Proxy Group]\nP = select, Q\nQ = select, P\n\
K = select, P, A\n[Rule]\nFINAL,K\n";
        let reg = generation(text, &FakeFactory::new(), None);
        let p = reg.resolve(&PolicyRef::parse("P"));
        assert_eq!(
            (chain(&p), p.terminal, p.note.clone()),
            (
                vec!["P", "REJECT"],
                TerminalKind::Reject,
                Some(Note::GroupCycle("P → Q → P".into()))
            )
        );
        assert_eq!(p.note.unwrap().to_string(), "policy group cycle: P → Q → P");
        let q = reg.resolve(&PolicyRef::parse("Q"));
        assert_eq!(q.note, Some(Note::GroupCycle("P → Q → P".into())));
        // a group that is not on it rejects only when it picks the member that is
        let k = reg.resolve(&PolicyRef::parse("K"));
        assert_eq!(chain(&k), vec!["K", "P", "REJECT"]);
        assert_eq!(k.note, Some(Note::GroupCycle("P → Q → P".into())));
    }

    /// A group that merely contains a cyclic member is unaffected once it
    /// picks another: only actually landing on the cycle rejects.
    #[test]
    fn a_group_that_picks_around_a_cyclic_member_is_unaffected() {
        let text = "[Proxy]\nA = http, a.example, 80\n[Proxy Group]\nP = select, Q\nQ = select, P\n\
K = select, P, A\n[Rule]\nFINAL,K\n";
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        let mut selections = GroupSelections::new();
        selections.set("K", "A");
        let reg = PolicyRegistry::build(
            &loaded.config,
            &assemble(&loaded.config, &Snapshots::new()),
            &FakeFactory::new(),
            &RegistryCell::new(),
            Arc::new(SelectionTable::new(selections)),
            None,
            EmptyGroup::Direct,
            &crate::testing::auto_groups(),
        )
        .expect("builds");
        let k = reg.resolve(&PolicyRef::parse("K"));
        assert_eq!(
            (chain(&k), k.terminal, k.note.clone()),
            (vec!["K", "A"], TerminalKind::Proxy, None)
        );
    }

    #[test]
    fn an_empty_group_stands_in_direct_or_rejects() {
        let text =
            "[Proxy Group]\nG = select, policy-path=https://sub.example/g\n[Rule]\nFINAL,G\n";
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        let cfg = loaded.config;
        let assembly = assemble(&cfg, &Snapshots::new());
        let build = |empty_group| {
            PolicyRegistry::build(
                &cfg,
                &assembly,
                &FakeFactory::new(),
                &RegistryCell::new(),
                Arc::new(SelectionTable::default()),
                None,
                empty_group,
                &crate::testing::auto_groups(),
            )
            .expect("builds")
        };
        let direct = build(EmptyGroup::Direct).resolve(&PolicyRef::parse("G"));
        assert_eq!(
            (chain(&direct), direct.terminal),
            (vec!["G", "DIRECT"], TerminalKind::Direct)
        );
        assert_eq!(
            direct.note.unwrap().to_string(),
            "policy group has no members; DIRECT substituted"
        );
        let reject = build(EmptyGroup::Reject).resolve(&PolicyRef::parse("G"));
        assert_eq!(
            (chain(&reject), reject.terminal),
            (vec!["G", "REJECT"], TerminalKind::Reject)
        );
        assert_eq!(
            reject.note.unwrap().to_string(),
            "policy group has no members"
        );
    }

    /// A group whose member is itself an empty group resolves through it to
    /// the DIRECT stand-in; picking a different, ordinary member is
    /// unaffected by that.
    #[test]
    fn a_group_whose_member_is_an_empty_group_resolves_through_the_stand_in() {
        let text = "[Proxy]\nA = http, a.example, 80\n\
[Proxy Group]\nE = select, policy-path=https://sub.example/e\nH = select, E, A\n[Rule]\nFINAL,H\n";
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        let build = |selections| {
            PolicyRegistry::build(
                &loaded.config,
                &assemble(&loaded.config, &Snapshots::new()),
                &FakeFactory::new(),
                &RegistryCell::new(),
                Arc::new(SelectionTable::new(selections)),
                None,
                EmptyGroup::Direct,
                &crate::testing::auto_groups(),
            )
            .expect("builds")
        };
        let h = build(GroupSelections::new()).resolve(&PolicyRef::parse("H"));
        assert_eq!(
            (chain(&h), h.terminal, h.note.clone()),
            (
                vec!["H", "E", "DIRECT"],
                TerminalKind::Direct,
                Some(Note::EmptyGroup { substituted: true })
            )
        );
        let mut selections = GroupSelections::new();
        selections.set("H", "A");
        let picked = build(selections).resolve(&PolicyRef::parse("H"));
        assert_eq!(
            (chain(&picked), picked.terminal, picked.note.clone()),
            (vec!["H", "A"], TerminalKind::Proxy, None)
        );
    }

    /// A relay is set so that traffic does not leave directly: an empty
    /// group used as one refuses, whatever `EmptyGroup` says — also when it
    /// is reached through a group that picks it. Dialled for itself, the
    /// group still stands in DIRECT.
    #[test]
    fn an_empty_group_as_a_relay_rejects() {
        let text = "[Proxy]\nA = http, a.example, 80\n\
[Proxy Group]\nE = select, policy-path=https://sub.example/e\nH = select, E, A\n[Rule]\nFINAL,H\n";
        let reg = generation(text, &FakeFactory::new(), None);
        let top = reg.resolve(&PolicyRef::parse("E"));
        assert_eq!(
            (top.terminal, top.note),
            (
                TerminalKind::Direct,
                Some(Note::EmptyGroup { substituted: true })
            )
        );
        for (relay, expected) in [("E", vec!["E", "REJECT"]), ("H", vec!["H", "E", "REJECT"])] {
            let r = reg.resolve_relay(relay);
            assert_eq!(
                (chain(&r), r.terminal, r.note.clone()),
                (
                    expected,
                    TerminalKind::Reject,
                    Some(Note::EmptyGroup { substituted: false })
                ),
                "{relay}"
            );
        }
        // anything else resolves as a policy would
        let a = reg.resolve_relay("A");
        assert_eq!((chain(&a), a.terminal), (vec!["A"], TerminalKind::Proxy));
    }

    const AUTO: &str = "[Proxy]\nA = http, a.example, 80\nB = http, b.example, 80\nC = http, c.example, 80\n\
[Proxy Group]\nU = url-test, A, B, C\nF = fallback, A, B, C\nL = load-balance, A, B, C, persistent=true\n\
N = fallback, A, B\nOuter = url-test, N, C\nAvg = load-balance, B, C\n\
E = url-test, A, B, evaluate-before-use=true\nS = select, E\n[Rule]\nFINAL,U\n";

    /// As if `name`'s last test had passed in `ms`, or failed.
    fn seed(reg: &PolicyRegistry, name: &str, ms: Option<u64>) {
        let case = reg.test_case(name).expect("a policy that is tested");
        let outcome = ms
            .map(Duration::from_millis)
            .ok_or_else(|| "refused".to_string());
        reg.auto().tests.record(&case.policy, case.key, outcome);
    }

    fn picked(reg: &PolicyRegistry, group: &str) -> String {
        reg.resolve(&PolicyRef::parse(group))
            .chain
            .get(1)
            .cloned()
            .unwrap_or_default()
    }

    /// Phase 2 M3 design 6.4: the fastest that passes, the first that
    /// passes, any that passes — the first member (`load-balance`: any) when
    /// nothing passes or nothing was tested yet.
    #[test]
    fn the_automatic_groups_pick_by_the_tests() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        assert_eq!(picked(&reg, "U"), "A", "nothing tested: the first");
        assert_eq!(picked(&reg, "F"), "A");
        seed(&reg, "A", None);
        seed(&reg, "B", Some(200));
        seed(&reg, "C", Some(50));
        assert_eq!(picked(&reg, "U"), "C");
        assert_eq!(picked(&reg, "F"), "B");
        let ctx = SelectCtx {
            host: Some("example.com".into()),
        };
        let first = reg.resolve_with(&PolicyRef::parse("L"), &ctx).chain[1].clone();
        assert!(first == "B" || first == "C", "{first}");
        for _ in 0..10 {
            assert_eq!(
                reg.resolve_with(&PolicyRef::parse("L"), &ctx).chain[1],
                first,
                "persistent: one host, one member"
            );
        }
        // the views answer the same, and move nothing
        assert_eq!(reg.current_member("U").as_deref(), Some("C"));
        assert_eq!(reg.current_member("L").as_deref(), Some("B"));
        assert_eq!(reg.available("U"), ["B", "C"]);
    }

    /// A group scores as its pick; a `load-balance` group as the average of
    /// the members that pass (M3 design 6.4).
    #[test]
    fn a_group_member_scores_by_its_pick() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        seed(&reg, "A", None);
        seed(&reg, "B", Some(100));
        seed(&reg, "C", Some(300));
        assert_eq!(
            reg.resolve(&PolicyRef::parse("Outer")).chain,
            ["Outer", "N", "B"],
            "N scores as B, 100 ms"
        );
        assert_eq!(
            reg.standing("Avg", 0),
            Standing::Passed(Duration::from_millis(200))
        );
    }

    /// A dial asks for a round of the group whose results are older than its
    /// `interval` (none yet: older than anything); the views ask for
    /// nothing. Once the round is in — every member with a result — nothing
    /// more is asked until the interval has passed.
    #[test]
    fn a_dial_asks_for_a_round_and_the_views_do_not() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        reg.current_member("U");
        assert!(reg.auto().requested().is_empty());
        reg.resolve(&PolicyRef::parse("U"));
        assert_eq!(reg.auto().requested(), ["U"]);
        for name in ["A", "B", "C"] {
            seed(&reg, name, Some(10));
        }
        reg.auto().round_done(&["U".to_string()]);
        reg.resolve(&PolicyRef::parse("U"));
        assert!(reg.auto().requested().is_empty());
    }

    /// A member without a result for what it is now — a new one, or one a
    /// reload or a subscription update changed — asks for a round even when
    /// the group's last round is recent (M3 design 6.3).
    #[test]
    fn a_member_without_a_result_asks_for_a_round() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        for name in ["A", "B", "C"] {
            seed(&reg, name, Some(10));
        }
        reg.auto().round_done(&["U".to_string()]);
        let _ = reg.resolve(&PolicyRef::parse("U"));
        assert!(
            reg.auto().requested().is_empty(),
            "every member tested, the round fresh"
        );
        // C's result is gone, as if its test URL had changed
        reg.auto().tests.invalidate_all();
        seed(&reg, "A", Some(10));
        seed(&reg, "B", Some(10));
        let _ = reg.resolve(&PolicyRef::parse("U"));
        assert_eq!(reg.auto().requested(), ["U"]);
    }

    /// `evaluate-before-use`: until its first round is in, a dial that goes
    /// through the group is told to wait for it (M3 design 6.3).
    #[test]
    fn evaluate_before_use_waits_for_the_first_round() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        assert_eq!(
            reg.resolve(&PolicyRef::parse("S")).pending.as_deref(),
            Some("E")
        );
        assert_eq!(reg.resolve(&PolicyRef::parse("U")).pending, None);
        reg.auto().round_done(&["E".to_string()]);
        assert_eq!(reg.resolve(&PolicyRef::parse("E")).pending, None);
    }

    /// An override stands while it names a member, and asks for no test.
    #[test]
    fn an_override_stands_and_asks_for_no_round() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        seed(&reg, "B", Some(10));
        let spec = reg.group_spec("U").expect("a group").clone();
        reg.auto().set_override(&spec, "A");
        assert_eq!(picked(&reg, "U"), "A");
        assert_eq!(reg.current_member("U").as_deref(), Some("A"));
        assert!(reg.auto().requested().is_empty());
        reg.auto().set_override(&spec, "Gone");
        assert_eq!(picked(&reg, "U"), "B");
    }

    /// A round tests every member of the group and of the groups in it,
    /// and is recorded for each of those groups.
    #[tokio::test]
    async fn a_round_tests_every_member_of_the_group_and_its_groups() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        let available = reg.test_group("Outer").await;
        // the fake outbounds lead nowhere: every test fails
        assert!(available.is_empty());
        for name in ["A", "B", "C"] {
            assert!(reg.test_result(name).is_some_and(|r| r.outcome.is_err()));
        }
        assert!(reg.auto().last_round("Outer").is_some());
        assert!(reg.auto().last_round("N").is_some());
        assert!(reg.auto().last_round("U").is_none());
        // REJECT and DIRECT: never passes, and tested like the rest
        assert!(reg.test_case("REJECT").is_none());
        assert_eq!(
            reg.test_case("DIRECT").map(|c| c.policy).as_deref(),
            Some("DIRECT")
        );
    }

    /// A round asked for a group that a reload then took away still ends
    /// the request: a group of that name is tested again when asked.
    #[tokio::test]
    async fn a_round_of_a_group_that_is_gone_ends_the_request() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        reg.auto().wake("Gone");
        assert_eq!(reg.auto().requested(), ["Gone"]);
        assert!(reg.test_group("Gone").await.is_empty());
        assert!(reg.auto().requested().is_empty());
    }

    #[test]
    fn selections_api() {
        let mut s = GroupSelections::new();
        assert!(s.is_empty());
        s.set("G", "A");
        assert_eq!(s.get("G"), Some("A"));
        assert_eq!(s.get("X"), None);
        let s2 = GroupSelections::from_map(HashMap::from([("G".to_string(), "B".to_string())]));
        assert_eq!(s2.get("G"), Some("B"));
    }
}
