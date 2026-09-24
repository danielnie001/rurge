//! A group's members once everything it takes in is added (M3 design 5.3):
//! the members written on its line, those of the groups `include-other-group`
//! names, the profile's proxies (`include-all-proxies`) and the policies of
//! its `policy-path`. Pure: no network, no disk.

use crate::subscription::{MAX_POLICIES, Subscription};
use rurge_config::Config;
use rurge_config::diagnostic::{Diagnostic, Diagnostics, Severity, codes};
use rurge_config::policy::{PolicyKind, ProxyPolicy, parse_policy, with_params};
use rurge_config::spec::{GroupSpec, NameKind, PolicyPath, PolicySpec, SpecEnv, to_spec};
use std::collections::{HashMap, HashSet, VecDeque};
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
    /// For every group on a cycle (through members as assembled or
    /// `include-other-group`), a shortest cycle through it: the groups
    /// along it, the first one repeated at the end; each cycle once.
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

/// Why an imported line cannot be used, in words that quote nothing from the
/// line or from the modifier (M3-D7): the messages of `to_spec` quote the
/// values they reject.
fn unusable(code: &str) -> &'static str {
    match code {
        codes::E_UNKNOWN_POLICY_REF => "has an `underlying-proxy` that names no policy",
        codes::E_KEYSTORE_REF => "names a `[Keystore]` item that is missing or of another kind",
        codes::E_INVALID_POLICY_PARAM => "has a parameter whose value cannot be used",
        _ => "cannot be used",
    }
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
                        "`policy-path` line {}: policy `{}` {} ({}); skipped",
                        imported.policy.span.line,
                        imported.policy.name,
                        unusable(e.code),
                        e.code
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
    /// The graph is one name index over profile specs, imported specs and
    /// groups (an import without a spec is not a node), so this is one
    /// `on_cycles` pass rather than a walk per chained import.
    fn drop_chain_cycles(
        &mut self,
        cfg: &Config,
        members: &mut HashMap<String, Vec<String>>,
        diags: &mut Diagnostics,
    ) {
        let mut index: HashMap<String, usize> = HashMap::new();
        for s in &cfg.specs {
            let i = index.len();
            index.insert(s.name.clone(), i);
        }
        for (imported, _) in &self.list {
            if imported.spec.is_some() {
                let i = index.len();
                index.insert(imported.policy.name.clone(), i);
            }
        }
        for g in &cfg.group_specs {
            let i = index.len();
            index.insert(g.name.clone(), i);
        }
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); index.len()];
        for s in &cfg.specs {
            if let Some(under) = &s.common.underlying_proxy
                && let Some(&j) = index.get(under)
            {
                adj[index[&s.name]].push(j);
            }
        }
        for (imported, _) in &self.list {
            let Some(spec) = imported.spec.as_ref() else {
                continue;
            };
            if let Some(under) = &spec.common.underlying_proxy
                && let Some(&j) = index.get(under)
            {
                adj[index[&imported.policy.name]].push(j);
            }
        }
        for g in &cfg.group_specs {
            let i = index[&g.name];
            // every proxy member of a group with a relay is dialled through it
            for m in members.get(&g.name).into_iter().flatten() {
                if let Some(&j) = index.get(m) {
                    adj[i].push(j);
                }
            }
            if let Some(under) = &g.underlying_proxy
                && let Some(&j) = index.get(under)
            {
                adj[i].push(j);
            }
        }
        let cyclic = on_cycles(&adj);
        let mut cyclic_names: HashSet<String> = HashSet::new();
        for (imported, group) in &self.list {
            let Some(&i) = index.get(&imported.policy.name) else {
                continue;
            };
            if cyclic[i] {
                diags.push(warn(
                    group,
                    codes::W_SET_LINES_SKIPPED,
                    format!(
                        "`policy-path` line {}: the `underlying-proxy` of `{}` leads back to the policy itself; skipped",
                        imported.policy.span.line, imported.policy.name
                    ),
                ));
                cyclic_names.insert(imported.policy.name.clone());
            }
        }
        self.forget(&cyclic_names);
        for list in members.values_mut() {
            list.retain(|name| !cyclic_names.contains(name));
        }
        self.drop_dangling(cfg, members, diags);
    }

    /// An import whose `underlying-proxy` no longer names anything — its
    /// target was itself left out, by the cyclic check above or by an
    /// earlier round of this one — would dial nowhere: it is left out too,
    /// and so is every membership of it. Repeats until a round removes
    /// nothing, so a chain of any length unravels.
    fn drop_dangling(
        &mut self,
        cfg: &Config,
        members: &mut HashMap<String, Vec<String>>,
        diags: &mut Diagnostics,
    ) {
        loop {
            let kept: HashSet<&str> = self
                .list
                .iter()
                .map(|(i, _)| i.policy.name.as_str())
                .collect();
            let mut dangling: HashSet<String> = HashSet::new();
            for (imported, group) in &self.list {
                let Some(target) = imported
                    .spec
                    .as_ref()
                    .and_then(|s| s.common.underlying_proxy.as_deref())
                else {
                    continue;
                };
                if cfg.name_kind(target).is_none() && !kept.contains(target) {
                    diags.push(warn(
                        group,
                        codes::W_SET_LINES_SKIPPED,
                        format!(
                            "`policy-path` line {}: the `underlying-proxy` of `{}` names a policy that was left out; skipped",
                            imported.policy.span.line, imported.policy.name
                        ),
                    ));
                    dangling.insert(imported.policy.name.clone());
                }
            }
            if dangling.is_empty() {
                break;
            }
            self.forget(&dangling);
            for list in members.values_mut() {
                list.retain(|name| !dangling.contains(name));
            }
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
    let include_cyclic = on_cycles(&group_graph(cfg, &|g| {
        g.import.include_other_groups.clone()
    }));
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
        on_cycle: cfg
            .group_specs
            .iter()
            .zip(include_cyclic)
            .filter(|(_, cyclic)| *cyclic)
            .map(|(g, _)| g.name.clone())
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
    let adj = group_graph(cfg, &|g| {
        let mut next = members.get(&g.name).cloned().unwrap_or_default();
        next.extend(g.import.include_other_groups.iter().cloned());
        next
    });
    let cyclic = on_cycles(&adj);
    let mut seen: HashSet<Vec<usize>> = HashSet::new();
    let mut out: Vec<Vec<String>> = Vec::new();
    for (i, &is_cyclic) in cyclic.iter().enumerate() {
        if !is_cyclic {
            continue;
        }
        let path = shortest_cycle(&adj, i);
        let min_pos = (0..path.len())
            .min_by_key(|&p| path[p])
            .expect("a cycle has at least one node");
        let rotated: Vec<usize> = path[min_pos..]
            .iter()
            .chain(&path[..min_pos])
            .copied()
            .collect();
        if seen.insert(rotated.clone()) {
            let mut names: Vec<String> = rotated
                .iter()
                .map(|&n| cfg.group_specs[n].name.clone())
                .collect();
            names.push(names[0].clone());
            out.push(names);
        }
    }
    out
}

/// Which of the nodes `0..adj.len()` lie on a cycle along `adj`: those whose
/// strongly connected component holds another node too, or that have an
/// edge to themselves. Tarjan's algorithm, iterative, so that a chain of
/// 10 000 imports cannot overflow the stack.
fn on_cycles(adj: &[Vec<usize>]) -> Vec<bool> {
    const NEW: usize = usize::MAX;
    let n = adj.len();
    let mut index = vec![NEW; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut cyclic = vec![false; n];
    let mut next = 0;
    for root in 0..n {
        if index[root] != NEW {
            continue;
        }
        // (node, how many of its edges have been followed)
        let mut work = vec![(root, 0)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&(v, followed)) = work.last() {
            if let Some(&w) = adj[v].get(followed) {
                if let Some(top) = work.last_mut() {
                    top.1 += 1;
                }
                if index[w] == NEW {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(u, _)) = work.last() {
                low[u] = low[u].min(low[v]);
            }
            if low[v] == index[v] {
                let mut component = Vec::new();
                loop {
                    let w = stack.pop().expect("v is still on the stack");
                    on_stack[w] = false;
                    component.push(w);
                    if w == v {
                        break;
                    }
                }
                let round = component.len() > 1 || adj[v].contains(&v);
                for w in component {
                    cyclic[w] = round;
                }
            }
        }
    }
    cyclic
}

/// The groups as nodes (in declaration order), each with its edges along
/// `edges`; names that are not groups are left out.
fn group_graph(cfg: &Config, edges: &dyn Fn(&GroupSpec) -> Vec<String>) -> Vec<Vec<usize>> {
    let index: HashMap<&str, usize> = cfg
        .group_specs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.as_str(), i))
        .collect();
    cfg.group_specs
        .iter()
        .map(|g| {
            edges(g)
                .iter()
                .filter_map(|n| index.get(n.as_str()).copied())
                .collect()
        })
        .collect()
}

/// A shortest way from `start` back to itself, as the nodes along it,
/// `start` first and not repeated; `start` is known to be on a cycle.
fn shortest_cycle(adj: &[Vec<usize>], start: usize) -> Vec<usize> {
    let mut parent: Vec<Option<usize>> = vec![None; adj.len()];
    let mut queue = VecDeque::from([start]);
    while let Some(v) = queue.pop_front() {
        for &w in &adj[v] {
            if w == start {
                let mut path = vec![v];
                let mut at = v;
                while at != start {
                    at = parent[at].expect("every node queued has a parent");
                    path.push(at);
                }
                path.reverse();
                return path;
            }
            if parent[w].is_none() {
                parent[w] = Some(v);
                queue.push_back(w);
            }
        }
    }
    vec![start]
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
                    "policy group `G`: `policy-path` line 1: policy `Bad` has a parameter whose value cannot be used (E0018); skipped".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G`: `policy-path` line 2: policy `Up` has an `underlying-proxy` that names no policy (E0007); skipped".to_string()
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

    /// Whichever member order a group's own line lists, the same cycles are
    /// found: F1 fixed a strongly-connected-component hole where a group
    /// reached only through a cross edge (like `B` here) went unlisted.
    #[test]
    fn every_group_on_a_cycle_is_listed_whatever_the_member_order() {
        for r in ["R = select, A, B", "R = select, B, A"] {
            let cfg = profile(
                "X = http, x.test, 80",
                &format!("{r}\nA = select, R\nB = select, A, X"),
            );
            let a = assemble(&cfg, &Snapshots::new());
            assert_eq!(
                a.cycles,
                vec![vec!["R", "A", "R"], vec!["R", "B", "A", "R"]],
                "{r}"
            );
        }
    }

    /// A group on an `include-other-group` cycle gives its members to
    /// nobody — the cycle would have no end — and F1 also fixed the
    /// cross-edge hole for this graph (`B` reaches the cycle only via `A`).
    #[test]
    fn a_group_on_an_include_cycle_gives_its_members_to_nobody() {
        let cfg = profile(
            "X1 = http, x1.test, 80\nX2 = http, x2.test, 80",
            "R = select, DIRECT, include-other-group=\"A,B\"\n\
A = select, X1, include-other-group=R\nB = select, X2, include-other-group=A",
        );
        let a = assemble(&cfg, &Snapshots::new());
        assert_eq!(members(&a, "R"), ["DIRECT"]);
        assert_eq!(members(&a, "A"), ["X1"]);
        assert_eq!(members(&a, "B"), ["X2"]);
        assert_eq!(
            a.cycles,
            vec![vec!["R", "A", "R"], vec!["R", "B", "A", "R"]]
        );
    }

    /// A modifier value (here `headers=Authorization Bearer s3cr3tT0ken`)
    /// never reaches a diagnostic, whatever `to_spec` rejected it for
    /// (M3-D7): F2 replaced the quoted `to_spec` message with a fixed
    /// phrase per diagnostic code.
    #[test]
    fn a_skipped_line_is_said_without_its_values() {
        let cfg = profile(
            "A = http, a.test, 80",
            "G = select, A, policy-path=https://sub.test/g, \
external-policy-modifier=\"headers=Authorization Bearer s3cr3tT0ken\"",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[("G", "N = http, n.test, 80\nM = http, m.test, 80, tos=0x1ff")],
            ),
        );
        assert_eq!(members(&a, "G"), ["A"]);
        assert_eq!(
            warnings(&a),
            [
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G`: `policy-path` line 1: policy `N` has a parameter whose value cannot be used (E0018); skipped".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G`: `policy-path` line 2: policy `M` has a parameter whose value cannot be used (E0018); skipped".to_string()
                ),
            ]
        );
        for (_, message) in warnings(&a) {
            for secret in ["s3cr3t", "Bearer", "0x1ff"] {
                assert!(!message.contains(secret), "{message}");
            }
        }
    }

    /// An import whose relay was itself left out — because it formed a
    /// cycle, or because one of its own parameters was invalid — would dial
    /// nowhere on its own: F3 leaves it out too, in as many rounds as a
    /// chain needs.
    #[test]
    fn an_import_that_chains_through_one_left_out_is_left_out_too() {
        let cfg = profile(
            "A = http, a.test, 80",
            "Pool = select, policy-path=https://sub.test/p\n\
All = select, A, include-other-group=Pool",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[(
                    "Pool",
                    "I1 = http, i1.test, 80, underlying-proxy=I2\n\
I2 = http, i2.test, 80, underlying-proxy=I3\n\
I3 = http, i3.test, 80, underlying-proxy=I2\n\
J1 = http, j1.test, 80, underlying-proxy=J2\n\
J2 = http, j2.test, 80, tos=999\n\
Me = http, me.test, 80, underlying-proxy=Me\n\
Fine = http, f.test, 80",
                )],
            ),
        );
        assert_eq!(members(&a, "Pool"), ["Fine"]);
        assert_eq!(members(&a, "All"), ["A", "Fine"]);
        let imported: Vec<&str> = a.imported.iter().map(|i| i.policy.name.as_str()).collect();
        assert_eq!(imported, ["Fine"]);
        assert_eq!(
            warnings(&a),
            [
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `Pool`: `policy-path` line 5: policy `J2` has a parameter whose value cannot be used (E0018); skipped".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `Pool`: `policy-path` line 2: the `underlying-proxy` of `I2` leads back to the policy itself; skipped".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `Pool`: `policy-path` line 3: the `underlying-proxy` of `I3` leads back to the policy itself; skipped".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `Pool`: `policy-path` line 6: the `underlying-proxy` of `Me` leads back to the policy itself; skipped".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `Pool`: `policy-path` line 1: the `underlying-proxy` of `I1` names a policy that was left out; skipped".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `Pool`: `policy-path` line 4: the `underlying-proxy` of `J1` names a policy that was left out; skipped".to_string()
                ),
            ]
        );
    }

    /// Existing behaviour, pinned: a member taken from another group by
    /// `include-other-group` is still checked against this group's own
    /// `policy-regex-filter`.
    #[test]
    fn members_taken_from_another_group_pass_the_filter() {
        let cfg = profile(
            "HK1 = http, hk1.test, 80\nUS1 = http, us1.test, 80",
            "H = select, HK1, US1\n\
G = select, DIRECT, include-other-group=H, policy-regex-filter=^HK",
        );
        let a = assemble(&cfg, &Snapshots::new());
        assert_eq!(members(&a, "G"), ["DIRECT", "HK1"]);
    }

    #[test]
    fn group_cycles_through_members_are_listed() {
        let cfg = profile("A = http, a.test, 80", "P = select, Q, A\nQ = select, P");
        let a = assemble(&cfg, &Snapshots::new());
        assert_eq!(a.cycles, [["P", "Q", "P"]]);
    }
}
