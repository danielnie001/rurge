//! A group's members once everything it takes in is added (M3 design 5.3):
//! the members written on its line, those of the groups `include-other-group`
//! names, the profile's proxies (`include-all-proxies`) and the policies of
//! its `policy-path`. Pure: no network, no disk.

use crate::subscription::{MAX_POLICIES, Subscription};
use rurge_config::Config;
use rurge_config::diagnostic::{Diagnostic, Diagnostics, Severity, codes};
use rurge_config::policy::{PolicyKind, ProxyPolicy, parse_policy, with_params};
use rurge_config::spec::{GroupSpec, NameKind, PolicyPath, PolicySpec, SpecEnv, to_spec};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The current content of every subscription; a source that is absent has
/// not been downloaded yet.
pub type Snapshots = HashMap<PolicyPath, Arc<Subscription>>;

/// A policy a group took in through `policy-path`.
#[derive(Clone)]
pub struct Imported {
    /// Named with the group's prefix, parameters overridden by its modifier.
    pub policy: ProxyPolicy,
    /// `None`: a protocol this version does not implement; it behaves as
    /// REJECT.
    pub spec: Option<PolicySpec>,
}

/// No `Debug`: imported policies carry credentials.
#[derive(Clone, Default)]
pub struct Assembly {
    /// Every group's members, by group name.
    pub members: HashMap<String, Vec<String>>,
    /// Names unique, none of them a name of the profile.
    pub imported: Vec<Imported>,
    /// Every group cycle, through members or `include-other-group`: the
    /// groups along it, the first one repeated at the end.
    pub cycles: Vec<Vec<String>>,
    /// Warnings, each at the line of the group it concerns.
    pub diagnostics: Diagnostics,
}

impl Assembly {
    /// The members of `group`; none for a name that is not a group.
    pub fn members_of(&self, group: &str) -> &[String] {
        self.members.get(group).map(Vec::as_slice).unwrap_or(&[])
    }
}

pub fn assemble(cfg: &Config, snapshots: &Snapshots) -> Assembly {
    let mut diagnostics = Diagnostics::default();
    let mut imports = Imports::collect(cfg, snapshots, &mut diagnostics);
    imports.read_specs(cfg, &mut diagnostics);
    let mut members = members(cfg, &imports);
    imports.drop_chain_cycles(cfg, &mut members, &mut diagnostics);
    let cycles = group_cycles(cfg, &members);
    Assembly {
        members,
        imported: imports.list.into_iter().map(|(i, _)| i).collect(),
        cycles,
        diagnostics,
    }
}

/// Warning `code` at `group`'s line.
fn warn(group: &GroupSpec, code: &'static str, message: String) -> Diagnostic {
    Diagnostic::warning(code, format!("policy group `{}`: {message}", group.name))
        .at(group.span.clone())
}

/// What parsing `sub` left out; said once per source, by the first group.
fn report(group: &GroupSpec, sub: &Subscription, diags: &mut Diagnostics) {
    for (line, reason) in &sub.skipped {
        diags.push(warn(
            group,
            codes::W_SET_LINES_SKIPPED,
            format!("`policy-path` line {line} skipped: {reason}"),
        ));
    }
    if sub.truncated {
        diags.push(warn(
            group,
            codes::W_SET_TRUNCATED,
            format!("`policy-path` holds more than {MAX_POLICIES} policies; the rest are ignored"),
        ));
    }
    if sub.policies.is_empty() {
        diags.push(warn(
            group,
            codes::W_SET_LINES_SKIPPED,
            "`policy-path` holds no policy; the content may not be in Surge format (policy lines, or a profile with a `[Proxy]` section)".to_string(),
        ));
    }
}

/// The policies groups took in, each with the group that brought it.
struct Imports<'a> {
    /// In import order.
    list: Vec<(Imported, &'a GroupSpec)>,
    /// What each group took in, by group name, in file order.
    by_group: HashMap<&'a str, Vec<String>>,
}

impl<'a> Imports<'a> {
    /// Filter → prefix → modifier (the manual's order), then the global
    /// namespace: the profile's names win, and between two groups the copy
    /// of the one declared first.
    fn collect(cfg: &'a Config, snapshots: &Snapshots, diags: &mut Diagnostics) -> Imports<'a> {
        let mut out = Imports {
            list: Vec::new(),
            by_group: HashMap::new(),
        };
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut reported: HashSet<&PolicyPath> = HashSet::new();
        for g in &cfg.group_specs {
            let Some(path) = &g.import.policy_path else {
                continue;
            };
            let Some(sub) = snapshots.get(path) else {
                diags.push(warn(
                    g,
                    codes::W_RESOURCE_UNAVAILABLE,
                    "`policy-path` has no content yet (never downloaded, or the file cannot be read); its imported members are unknown".to_string(),
                ));
                continue;
            };
            if reported.insert(path) {
                report(g, sub, diags);
            }
            let names = out.by_group.entry(g.name.as_str()).or_default();
            let prefix = g.import.name_prefix.as_deref().unwrap_or("");
            let modifier = g.import.modifier.expose();
            for p in &sub.policies {
                if !g.import.admits(&p.name) {
                    continue;
                }
                let line = p.span.line;
                let name = format!("{prefix}{}", p.name);
                let definition = if modifier.is_empty() {
                    p.definition.clone()
                } else {
                    with_params(&p.definition, modifier)
                };
                let Ok(policy) = parse_policy(&name, &definition, &p.span) else {
                    diags.push(warn(
                        g,
                        codes::W_SET_LINES_SKIPPED,
                        format!("`policy-path` line {line}: the line is no valid policy once modified; skipped"),
                    ));
                    continue;
                };
                if cfg.name_kind(&name).is_some() {
                    diags.push(warn(
                        g,
                        codes::W_SET_LINES_SKIPPED,
                        format!("`policy-path` line {line}: `{name}` is already a name of the profile; skipped"),
                    ));
                    continue;
                }
                match index.get(&name) {
                    None => {
                        index.insert(name.clone(), out.list.len());
                        out.list.push((Imported { policy, spec: None }, g));
                        names.push(name);
                    }
                    // the same line through another group: one policy
                    Some(&i) if out.list[i].0.policy.definition == policy.definition => {
                        names.push(name)
                    }
                    Some(&i) => diags.push(warn(
                        g,
                        codes::W_SET_LINES_SKIPPED,
                        format!(
                            "`policy-path` line {line}: `{name}` is already imported by `{}` with another definition; skipped",
                            out.list[i].1.name
                        ),
                    )),
                }
            }
        }
        out
    }

    /// The typed parameters of every import. A line with an error is
    /// skipped (M3-D6); one of a protocol this version does not implement
    /// stays, as REJECT, which is said once per protocol.
    fn read_specs(&mut self, cfg: &Config, diags: &mut Diagnostics) {
        let kinds: HashMap<String, PolicyKind> = self
            .list
            .iter()
            .map(|(i, _)| (i.policy.name.clone(), i.policy.kind))
            .collect();
        let lookup = |name: &str| {
            cfg.name_kind(name)
                .or_else(|| kinds.get(name).copied().map(NameKind::Policy))
        };
        let env = SpecEnv {
            keystore: &cfg.keystore,
            lookup: &lookup,
        };
        let mut failed: HashSet<String> = HashSet::new();
        let mut said: HashSet<String> = HashSet::new();
        for (imported, group) in &mut self.list {
            let group: &GroupSpec = group;
            let outcome = to_spec(&imported.policy, &env);
            if let Some(e) = outcome
                .diagnostics
                .iter()
                .find(|d| d.severity == Severity::Error)
            {
                diags.push(warn(
                    group,
                    codes::W_SET_LINES_SKIPPED,
                    format!(
                        "`policy-path` line {}: {}; skipped",
                        imported.policy.span.line, e.message
                    ),
                ));
                failed.insert(imported.policy.name.clone());
                continue;
            }
            imported.spec = outcome.spec;
            if imported.spec.is_none() {
                let what = if outcome.legacy_vmess {
                    "`vmess` without `vmess-aead=true` (the legacy handshake)".to_string()
                } else {
                    format!("`{}`", imported.policy.kind.keyword())
                };
                if said.insert(what.clone()) {
                    diags.push(warn(
                        group,
                        codes::W_PROTOCOL_NOT_IMPLEMENTED,
                        format!(
                            "imported policies of type {what} are not implemented in this version; they behave as REJECT"
                        ),
                    ));
                }
            }
        }
        self.forget(&failed);
    }

    /// An import whose `underlying-proxy` leads back to itself would never
    /// finish dialling: it is skipped, and so is every membership of it.
    fn drop_chain_cycles(
        &mut self,
        cfg: &Config,
        members: &mut HashMap<String, Vec<String>>,
        diags: &mut Diagnostics,
    ) {
        let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
        let imported = self.list.iter().filter_map(|(i, _)| i.spec.as_ref());
        for s in cfg.specs.iter().chain(imported) {
            if let Some(under) = &s.common.underlying_proxy {
                edges.insert(&s.name, vec![under]);
            }
        }
        for g in &cfg.group_specs {
            let mut next: Vec<&str> = members
                .get(&g.name)
                .map(|m| m.iter().map(String::as_str).collect())
                .unwrap_or_default();
            // every proxy member of a group with a relay is dialled through it
            next.extend(g.underlying_proxy.as_deref());
            edges.insert(&g.name, next);
        }
        let mut cyclic: HashSet<String> = HashSet::new();
        for (imported, group) in &self.list {
            let Some(first) = imported
                .spec
                .as_ref()
                .and_then(|s| s.common.underlying_proxy.as_deref())
            else {
                continue;
            };
            if leads_back(&edges, &imported.policy.name, first) {
                diags.push(warn(
                    group,
                    codes::W_SET_LINES_SKIPPED,
                    format!(
                        "`policy-path` line {}: the `underlying-proxy` of `{}` leads back to the policy itself; skipped",
                        imported.policy.span.line, imported.policy.name
                    ),
                ));
                cyclic.insert(imported.policy.name.clone());
            }
        }
        self.forget(&cyclic);
        for list in members.values_mut() {
            list.retain(|name| !cyclic.contains(name));
        }
    }

    fn forget(&mut self, names: &HashSet<String>) {
        if names.is_empty() {
            return;
        }
        self.list.retain(|(i, _)| !names.contains(&i.policy.name));
        for list in self.by_group.values_mut() {
            list.retain(|name| !names.contains(name));
        }
    }
}

/// Whether following `edges` from `first` reaches `start`.
fn leads_back(edges: &HashMap<&str, Vec<&str>>, start: &str, first: &str) -> bool {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut stack = vec![first];
    while let Some(name) = stack.pop() {
        if name == start {
            return true;
        }
        if seen.insert(name)
            && let Some(next) = edges.get(name)
        {
            stack.extend(next.iter().copied());
        }
    }
    false
}

/// A member list that keeps each name where it first appears.
#[derive(Default)]
struct Members {
    list: Vec<String>,
    seen: HashSet<String>,
}

impl Members {
    fn add(&mut self, name: &str) {
        if self.seen.insert(name.to_string()) {
            self.list.push(name.to_string());
        }
    }
}

/// Every group's members in the manual's order — written, then
/// `include-other-group`, then `include-all-proxies`, then `policy-path`.
fn members(cfg: &Config, imports: &Imports<'_>) -> HashMap<String, Vec<String>> {
    let expand = Expand {
        cfg,
        imports,
        groups: cfg
            .group_specs
            .iter()
            .map(|g| (g.name.as_str(), g))
            .collect(),
        // a group on an `include-other-group` cycle gives its members to
        // nobody: the cycle would have no end (5.3)
        on_cycle: cycles(cfg, &|g| g.import.include_other_groups.clone())
            .into_iter()
            .flatten()
            .collect(),
    };
    let mut done: HashMap<String, Vec<String>> = HashMap::new();
    for g in &cfg.group_specs {
        expand.group(g, &mut done);
    }
    done
}

struct Expand<'a> {
    cfg: &'a Config,
    imports: &'a Imports<'a>,
    groups: HashMap<&'a str, &'a GroupSpec>,
    on_cycle: HashSet<String>,
}

impl Expand<'_> {
    fn group(&self, g: &GroupSpec, done: &mut HashMap<String, Vec<String>>) -> Vec<String> {
        if let Some(members) = done.get(&g.name) {
            return members.clone();
        }
        let mut out = Members::default();
        for m in &g.members {
            out.add(m);
        }
        for name in &g.import.include_other_groups {
            if self.on_cycle.contains(name) {
                continue;
            }
            let Some(other) = self.groups.get(name.as_str()) else {
                continue;
            };
            for m in self.group(other, done) {
                if g.import.admits(&m) {
                    out.add(&m);
                }
            }
        }
        if g.import.include_all_proxies {
            for p in &self.cfg.policies {
                if !p.kind.is_builtin_alias() && g.import.admits(&p.name) {
                    out.add(&p.name);
                }
            }
        }
        // filtered on their names before the prefix, when they were taken in
        for name in self
            .imports
            .by_group
            .get(g.name.as_str())
            .into_iter()
            .flatten()
        {
            out.add(name);
        }
        done.insert(g.name.clone(), out.list.clone());
        out.list
    }
}

/// Group cycles through members (as assembled) and `include-other-group`.
fn group_cycles(cfg: &Config, members: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    cycles(cfg, &|g| {
        let mut next = members.get(&g.name).cloned().unwrap_or_default();
        next.extend(g.import.include_other_groups.iter().cloned());
        next
    })
}

/// Every cycle a depth-first walk along `edges` meets, as the groups along
/// it with the first repeated at the end. Every group on some cycle is on
/// one of these.
fn cycles(cfg: &Config, edges: &dyn Fn(&GroupSpec) -> Vec<String>) -> Vec<Vec<String>> {
    let mut walk = Walk {
        specs: &cfg.group_specs,
        index: cfg
            .group_specs
            .iter()
            .enumerate()
            .map(|(i, g)| (g.name.as_str(), i))
            .collect(),
        edges,
        colour: vec![Colour::New; cfg.group_specs.len()],
        stack: Vec::new(),
        found: Vec::new(),
    };
    for i in 0..cfg.group_specs.len() {
        if walk.colour[i] == Colour::New {
            walk.visit(i);
        }
    }
    walk.found
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Colour {
    New,
    OnStack,
    Done,
}

struct Walk<'a> {
    specs: &'a [GroupSpec],
    index: HashMap<&'a str, usize>,
    edges: &'a dyn Fn(&GroupSpec) -> Vec<String>,
    colour: Vec<Colour>,
    stack: Vec<usize>,
    found: Vec<Vec<String>>,
}

impl Walk<'_> {
    fn visit(&mut self, i: usize) {
        self.colour[i] = Colour::OnStack;
        self.stack.push(i);
        for next in (self.edges)(&self.specs[i]) {
            let Some(&j) = self.index.get(next.as_str()) else {
                continue;
            };
            match self.colour[j] {
                Colour::New => self.visit(j),
                Colour::OnStack => {
                    let from = self
                        .stack
                        .iter()
                        .position(|&k| k == j)
                        .expect("a group on the stack");
                    let mut cycle: Vec<String> = self.stack[from..]
                        .iter()
                        .map(|&k| self.specs[k].name.clone())
                        .collect();
                    cycle.push(self.specs[j].name.clone());
                    self.found.push(cycle);
                }
                Colour::Done => {}
            }
        }
        self.stack.pop();
        self.colour[i] = Colour::Done;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription;
    use rurge_config::config::{LoadOptions, from_text};
    use std::path::Path;

    fn profile(proxies: &str, groups: &str) -> Config {
        let text = format!("[Proxy]\n{proxies}\n[Proxy Group]\n{groups}\n[Rule]\nFINAL,DIRECT\n");
        let loaded = from_text(&text, Path::new("/p/t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        loaded.config
    }

    /// What the `policy-path` of each named group serves.
    fn snapshots(cfg: &Config, served: &[(&str, &str)]) -> Snapshots {
        served
            .iter()
            .map(|(group, text)| {
                let g = cfg.group_specs.iter().find(|g| g.name == *group).unwrap();
                let path = g.import.policy_path.clone().expect("a policy-path");
                (path, Arc::new(subscription::parse(text)))
            })
            .collect()
    }

    fn members<'a>(a: &'a Assembly, group: &str) -> Vec<&'a str> {
        a.members_of(group).iter().map(String::as_str).collect()
    }

    fn warnings(a: &Assembly) -> Vec<(&'static str, String)> {
        a.diagnostics
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect()
    }

    #[test]
    fn members_come_in_the_manual_order_each_once() {
        let cfg = profile(
            "A = http, a.test, 80\nB = http, b.test, 80\nC = http, c.test, 80\nBlock = reject",
            "H = select, B, C\n\
G = select, A, DIRECT, include-other-group=H, include-all-proxies=true, policy-path=https://sub.test/g",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[(
                    "G",
                    "S1 = http, s1.test, 80\nA = http, dup.test, 80\nS2 = http, s2.test, 80",
                )],
            ),
        );
        // `include-all-proxies` takes proxies only: `Block` is a reject alias
        assert_eq!(members(&a, "G"), ["A", "DIRECT", "B", "C", "S1", "S2"]);
        assert_eq!(members(&a, "H"), ["B", "C"]);
        assert_eq!(
            warnings(&a),
            [(
                codes::W_SET_LINES_SKIPPED,
                "policy group `G`: `policy-path` line 2: `A` is already a name of the profile; skipped"
                    .to_string()
            )]
        );
        let imported: Vec<&str> = a.imported.iter().map(|i| i.policy.name.as_str()).collect();
        assert_eq!(imported, ["S1", "S2"]);
        assert!(a.cycles.is_empty());
    }

    /// The filter spares the members written on the line and sees an
    /// imported name before the prefix is put in front of it.
    #[test]
    fn the_filter_and_the_prefix_act_in_the_manual_order() {
        let cfg = profile(
            "A = http, a.test, 80\nB = http, b.test, 80\nHK-Home = http, h.test, 80",
            "G = select, A, policy-regex-filter=^HK, external-policy-name-prefix=Sub-, \
include-all-proxies=true, policy-path=https://sub.test/g",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[("G", "HK-1 = http, hk1.test, 80\nUS-1 = http, us1.test, 80")],
            ),
        );
        assert_eq!(members(&a, "G"), ["A", "HK-Home", "Sub-HK-1"]);
        assert_eq!(a.imported[0].policy.name, "Sub-HK-1");
        assert!(a.diagnostics.is_empty());
    }

    #[test]
    fn the_modifier_rewrites_the_imported_lines_only() {
        let cfg = profile(
            "A = http, a.test, 80",
            "G = select, A, policy-path=https://sub.test/g, \
external-policy-modifier=\"tfo=true,test-url=http://apple.com/\"",
        );
        let a = assemble(
            &cfg,
            &snapshots(&cfg, &[("G", "N = http, n.test, 80, tfo=false")]),
        );
        let n = &a.imported[0];
        assert_eq!(
            n.policy.definition,
            "http, n.test, 80, tfo=true, test-url=http://apple.com/"
        );
        let spec = n.spec.as_ref().expect("an http policy has a spec");
        assert!(spec.common.tfo);
        assert_eq!(spec.common.test_url.as_deref(), Some("http://apple.com/"));
        assert!(!cfg.spec("A").unwrap().common.tfo);
    }

    #[test]
    fn include_other_group_is_recursive_and_a_cycle_gives_nothing() {
        let cfg = profile(
            "M1 = http, m.test, 80\nL1 = http, l.test, 80\nX1 = http, x1.test, 80\nX2 = http, x2.test, 80",
            "Top = select, include-other-group=Mid\nMid = select, M1, include-other-group=Low\n\
Low = select, L1\nLoop1 = select, X1, include-other-group=Loop2\n\
Loop2 = select, X2, include-other-group=Loop1\nOuter = select, DIRECT, include-other-group=Loop1",
        );
        let a = assemble(&cfg, &Snapshots::new());
        assert_eq!(members(&a, "Top"), ["M1", "L1"]);
        assert_eq!(members(&a, "Mid"), ["M1", "L1"]);
        assert_eq!(members(&a, "Outer"), ["DIRECT"]);
        assert_eq!(a.cycles, [["Loop1", "Loop2", "Loop1"]]);
    }

    #[test]
    fn two_groups_share_an_identical_import_but_not_another_definition() {
        let cfg = profile(
            "A = http, a.test, 80",
            "G1 = select, policy-path=https://sub.test/a\nG2 = select, policy-path=https://sub.test/a\n\
G3 = select, policy-path=https://sub.test/b",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[
                    ("G1", "N = http, n.test, 80"),
                    ("G3", "N = http, other.test, 80"),
                ],
            ),
        );
        assert_eq!(members(&a, "G1"), ["N"]);
        assert_eq!(members(&a, "G2"), ["N"]);
        assert!(members(&a, "G3").is_empty());
        assert_eq!(a.imported.len(), 1);
        assert_eq!(
            warnings(&a),
            [(
                codes::W_SET_LINES_SKIPPED,
                "policy group `G3`: `policy-path` line 1: `N` is already imported by `G1` with another definition; skipped"
                    .to_string()
            )]
        );
    }

    #[test]
    fn an_imported_line_that_cannot_be_used_is_skipped_by_its_number() {
        let cfg = profile(
            "A = http, a.test, 80",
            "G = select, policy-path=https://sub.test/g",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[(
                    "G",
                    "Bad = http, b.test, 80, tos=999\nUp = http, u.test, 80, underlying-proxy=Nowhere\n\
SS = ss, s.test, 8388, encrypt-method=aes-128-gcm, password=pw\n\
Old = vmess, v.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119\nGood = http, g.test, 80",
                )],
            ),
        );
        assert_eq!(members(&a, "G"), ["SS", "Old", "Good"]);
        let specs: Vec<bool> = a.imported.iter().map(|i| i.spec.is_some()).collect();
        assert_eq!(specs, [false, false, true]);
        assert_eq!(
            warnings(&a),
            [
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G`: `policy-path` line 1: policy `Bad`: invalid value `999` for `tos` (expected 0-255 or 0x00-0xff); skipped".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G`: `policy-path` line 2: policy `Up`: `underlying-proxy` references unknown policy `Nowhere`; skipped".to_string()
                ),
                (
                    codes::W_PROTOCOL_NOT_IMPLEMENTED,
                    "policy group `G`: imported policies of type `ss` are not implemented in this version; they behave as REJECT".to_string()
                ),
                (
                    codes::W_PROTOCOL_NOT_IMPLEMENTED,
                    "policy group `G`: imported policies of type `vmess` without `vmess-aead=true` (the legacy handshake) are not implemented in this version; they behave as REJECT".to_string()
                ),
            ]
        );
    }

    /// A chain that comes back to where it started would never finish
    /// dialling: the import that closes it is left out.
    #[test]
    fn an_imported_chain_that_leads_back_is_dropped() {
        let cfg = profile(
            "Entry = http, e.test, 80, underlying-proxy=Pool",
            "Pool = select, policy-path=https://sub.test/p",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[(
                    "Pool",
                    "Loop = http, l.test, 80, underlying-proxy=Entry\nFine = http, f.test, 80",
                )],
            ),
        );
        assert_eq!(members(&a, "Pool"), ["Fine"]);
        let imported: Vec<&str> = a.imported.iter().map(|i| i.policy.name.as_str()).collect();
        assert_eq!(imported, ["Fine"]);
        assert_eq!(
            warnings(&a),
            [(
                codes::W_SET_LINES_SKIPPED,
                "policy group `Pool`: `policy-path` line 1: the `underlying-proxy` of `Loop` leads back to the policy itself; skipped"
                    .to_string()
            )]
        );
    }

    /// Nothing about a subscription's URL reaches a warning: it usually
    /// carries a token.
    #[test]
    fn a_subscription_not_downloaded_yet_is_said_without_its_url() {
        let cfg = profile(
            "A = http, a.test, 80",
            "G = select, DIRECT, policy-path=https://sub.test/g?token=t0k3n",
        );
        let a = assemble(&cfg, &Snapshots::new());
        assert_eq!(members(&a, "G"), ["DIRECT"]);
        assert_eq!(
            warnings(&a),
            [(
                codes::W_RESOURCE_UNAVAILABLE,
                "policy group `G`: `policy-path` has no content yet (never downloaded, or the file cannot be read); its imported members are unknown"
                    .to_string()
            )]
        );
        assert!(a.diagnostics.iter().all(|d| !d.message.contains("t0k3n")));
    }

    #[test]
    fn a_shared_source_that_holds_nothing_is_reported_once() {
        let cfg = profile(
            "A = http, a.test, 80",
            "G1 = select, policy-path=https://sub.test/a\nG2 = select, policy-path=https://sub.test/a",
        );
        let a = assemble(&cfg, &snapshots(&cfg, &[("G1", "proxies: []")]));
        assert_eq!(
            warnings(&a),
            [
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G1`: `policy-path` line 1 skipped: not a policy line (`Name = type, ...`)".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G1`: `policy-path` holds no policy; the content may not be in Surge format (policy lines, or a profile with a `[Proxy]` section)".to_string()
                ),
            ]
        );
    }

    #[test]
    fn group_cycles_through_members_are_listed() {
        let cfg = profile("A = http, a.test, 80", "P = select, Q, A\nQ = select, P");
        let a = assemble(&cfg, &Snapshots::new());
        assert_eq!(a.cycles, [["P", "Q", "P"]]);
    }
}
