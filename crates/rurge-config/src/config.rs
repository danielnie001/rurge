//! Semantic layer: `Config` assembled from a `Profile`, plus cross validation.

use crate::deferred::DeferredSections;
use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::general::{General, parse_general};
use crate::host::{HostEntry, parse_host_entry};
use crate::keystore::{KeystoreItem, parse_keystore_item};
use crate::managed::{ManagedConfig, parse_managed};
use crate::policy::{
    Builtin, GroupKind, PolicyGroup, PolicyKind, ProxyPolicy, parse_group, parse_policy,
};
use crate::requirement::{self, Environment};
use crate::rule::{ParseCtx, PolicyRef, Rule, RuleKind, SubRule, parse_rule, parse_subrule};
use crate::span::Span;
use crate::text::include::{self, IncludeOptions};
use crate::text::{Origin, Profile, SectionKind, parse_str};
use crate::value::split_definition;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }
    pub fn system_name(&self) -> &'static str {
        match self {
            Platform::Windows => "Windows",
            Platform::Linux => "Linux",
            Platform::MacOs => "macOS",
        }
    }
    pub fn parse(s: &str) -> Option<Platform> {
        Some(match s.to_ascii_lowercase().as_str() {
            "windows" => Platform::Windows,
            "linux" => Platform::Linux,
            "macos" => Platform::MacOs,
            _ => return None,
        })
    }
}

/// What the running engine actually implements; used to warn about inactive config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub policy_kinds: HashSet<PolicyKind>,
    pub group_kinds: HashSet<GroupKind>,
    pub rule_types: HashSet<&'static str>,
}

impl Capabilities {
    pub const ALL_RULE_TYPES: [&'static str; 29] = [
        "DOMAIN",
        "DOMAIN-SUFFIX",
        "DOMAIN-KEYWORD",
        "DOMAIN-WILDCARD",
        "DOMAIN-SET",
        "IP-CIDR",
        "IP-CIDR6",
        "GEOIP",
        "IP-ASN",
        "USER-AGENT",
        "URL-REGEX",
        "PROCESS-NAME",
        "DEST-PORT",
        "SRC-PORT",
        "IN-PORT",
        "SRC-IP",
        "DEVICE-NAME",
        "MAC-ADDRESS",
        "PROTOCOL",
        "HOSTNAME-TYPE",
        "SUBNET",
        "CELLULAR-RADIO",
        "CELLULAR-CARRIER",
        "AND",
        "OR",
        "NOT",
        "SCRIPT",
        "RULE-SET",
        "FINAL",
    ];

    pub fn all() -> Self {
        use PolicyKind::*;
        Self {
            policy_kinds: HashSet::from([
                Http,
                Https,
                H2Connect,
                Socks5,
                Socks5Tls,
                Shadowsocks,
                Snell,
                Vmess,
                Trojan,
                Tuic,
                TuicV5,
                Hysteria2,
                Masque,
                AnyTls,
                TrustTunnel,
                Ssh,
                WireGuard,
                Tailscale,
                External,
                Direct,
                Reject,
                RejectDrop,
                RejectNoDrop,
                RejectTinyGif,
            ]),
            group_kinds: HashSet::from([
                GroupKind::Select,
                GroupKind::UrlTest,
                GroupKind::Fallback,
                GroupKind::LoadBalance,
                GroupKind::Smart,
                GroupKind::Subnet,
            ]),
            rule_types: Self::ALL_RULE_TYPES.iter().copied().collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LoadOptions {
    pub environment: Environment,
    pub platform: Platform,
    pub capabilities: Capabilities,
}

impl LoadOptions {
    pub fn for_tests() -> Self {
        Self {
            environment: Environment::fixed(),
            platform: Platform::Linux,
            capabilities: Capabilities::all(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineRuleset {
    pub name: String,
    pub rules: Vec<SubRule>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    pub main: PathBuf,
    pub includes: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub general: General,
    pub policies: Vec<ProxyPolicy>,
    pub groups: Vec<PolicyGroup>,
    pub rules: Vec<Rule>,
    pub rulesets: Vec<InlineRuleset>,
    pub hosts: Vec<HostEntry>,
    pub keystore: Vec<KeystoreItem>,
    pub deferred: DeferredSections,
    pub managed: Option<ManagedConfig>,
    pub unknown_sections: Vec<String>,
    pub source: SourceInfo,
}

pub enum PolicyTarget<'a> {
    Builtin(Builtin),
    Proxy(&'a ProxyPolicy),
    Group(&'a PolicyGroup),
}

/// Stable, serialisable overview used by snapshots and the API.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ConfigSummary {
    pub listeners: Vec<String>,
    pub policies: Vec<String>,
    pub groups: Vec<String>,
    pub rules: Vec<String>,
    pub rulesets: Vec<String>,
    pub hosts: Vec<String>,
    pub keystore: Vec<String>,
    pub deferred: Vec<String>,
    pub unknown_sections: Vec<String>,
    pub managed: Option<String>,
}

impl Config {
    pub fn resolve_policy(&self, name: &str) -> Option<PolicyTarget<'_>> {
        if let Some(p) = self.policies.iter().find(|p| p.name == name) {
            return Some(PolicyTarget::Proxy(p));
        }
        if let Some(g) = self.groups.iter().find(|g| g.name == name) {
            return Some(PolicyTarget::Group(g));
        }
        Builtin::parse(name).map(PolicyTarget::Builtin)
    }

    pub fn summary(&self) -> ConfigSummary {
        ConfigSummary {
            listeners: self
                .general
                .http_listen
                .iter()
                .map(|l| format!("http {}", l.addr))
                .chain(
                    self.general
                        .socks5_listen
                        .iter()
                        .map(|l| format!("socks5 {}", l.addr)),
                )
                .collect(),
            policies: self
                .policies
                .iter()
                .map(|p| format!("{} ({})", p.name, p.kind.keyword()))
                .collect(),
            groups: self
                .groups
                .iter()
                .map(|g| {
                    format!(
                        "{} ({}) -> [{}]",
                        g.name,
                        g.kind.keyword(),
                        g.members.join(", ")
                    )
                })
                .collect(),
            rules: self.rules.iter().map(|r| r.to_string()).collect(),
            rulesets: self
                .rulesets
                .iter()
                .map(|s| format!("{}: {} rules", s.name, s.rules.len()))
                .collect(),
            hosts: self.hosts.iter().map(|h| h.raw_key.clone()).collect(),
            keystore: self.keystore.iter().map(|k| k.name.clone()).collect(),
            deferred: self
                .deferred
                .sections
                .iter()
                .map(|s| s.name.clone())
                .collect(),
            unknown_sections: self.unknown_sections.clone(),
            managed: self.managed.as_ref().map(|m| m.url.clone()),
        }
    }

    /// Index of the FINAL rule that takes effect: the last one (manual: "if there
    /// are multiple FINAL rules, the last one is used").
    pub fn effective_final(&self) -> Option<usize> {
        self.rules
            .iter()
            .rposition(|r| matches!(r.kind, RuleKind::Final))
    }

    /// Lowercase hostnames of every proxy server; `[Host]` never applies to them.
    pub fn proxy_hostnames(&self) -> HashSet<String> {
        self.policies
            .iter()
            .filter_map(|p| p.server.as_ref()?.as_domain().map(str::to_string))
            .collect()
    }
}

#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("cannot read `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn load(path: &Path, opts: &LoadOptions) -> Result<Loaded, LoadError> {
    let text = std::fs::read_to_string(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(from_text(&text, path, opts))
}

pub fn from_text(text: &str, path: &Path, opts: &LoadOptions) -> Loaded {
    let file: Arc<Path> = Arc::from(path);
    let (mut profile, mut diags) = parse_str(text, file, Origin::Main);
    let base_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    include::expand(
        &mut profile,
        &IncludeOptions {
            base_dir: base_dir.clone(),
            max_depth: 8,
            max_files: 200,
        },
        &mut diags,
    );
    requirement::apply(&mut profile, &opts.environment, &mut diags);
    let mut loaded = from_profile(profile, &base_dir, opts);
    diags.extend(loaded.diagnostics);
    loaded.diagnostics = diags;
    loaded
}

fn definition_entries<'a>(
    profile: &'a Profile,
    section: &str,
    diags: &mut Diagnostics,
) -> Vec<(&'a str, &'a str, &'a Span)> {
    let mut out = Vec::new();
    if let Some(sec) = profile.section(section) {
        for e in sec.active_entries() {
            match split_definition(&e.raw) {
                Some((name, def)) => out.push((name, def, &e.span)),
                None => diags.push(
                    Diagnostic::error(
                        codes::E_INVALID_DEFINITION,
                        format!("[{section}]: expected `Name = ...`, found `{}`", e.raw),
                    )
                    .at(e.span.clone()),
                ),
            }
        }
    }
    out
}

/// Recursively collect `.unknown` flags from a sub-rule and any sub-rules nested
/// inside it (`AND` / `OR` / `NOT`). Ruling R12: unrecognised flags on sub-rules
/// are surfaced as `W_UNKNOWN_RULE_PARAM`, since `SubRule` carries no span of its
/// own to attach a diagnostic to directly.
fn collect_subrule_unknowns(sub: &SubRule, out: &mut Vec<String>) {
    out.extend(sub.unknown.iter().cloned());
    collect_kind_unknowns(&sub.kind, out);
}

/// Recursively collect `.unknown` flags from the sub-rules nested inside a
/// logical `AND` / `OR` / `NOT` rule kind. No-op for every other rule kind.
fn collect_kind_unknowns(kind: &RuleKind, out: &mut Vec<String>) {
    match kind {
        RuleKind::And(subs) | RuleKind::Or(subs) => {
            for s in subs {
                collect_subrule_unknowns(s, out);
            }
        }
        RuleKind::Not(sub) => collect_subrule_unknowns(sub, out),
        _ => {}
    }
}

pub fn from_profile(profile: Profile, base_dir: &Path, opts: &LoadOptions) -> Loaded {
    let mut diags = Diagnostics::default();
    let main = profile
        .main
        .as_deref()
        .map(Path::to_path_buf)
        .unwrap_or_default();

    let managed = parse_managed(&profile.header, &mut diags);
    let general = parse_general(profile.section("General"), &mut diags);

    let inline_names: HashSet<String> = profile
        .sections_with_prefix("Ruleset ")
        .map(|s| s.name["Ruleset ".len()..].trim().to_string())
        .collect();
    let ctx = ParseCtx {
        inline_rulesets: &inline_names,
        base_dir,
    };

    // Policies and groups with name registry.
    let mut names: HashMap<String, &'static str> = HashMap::new();
    let mut policies = Vec::new();
    for (name, def, span) in definition_entries(&profile, "Proxy", &mut diags) {
        match Builtin::parse(name) {
            Some(Builtin::Direct) => continue,
            Some(_) => {
                diags.push(
                    Diagnostic::error(
                        codes::E_BUILTIN_REDEFINED,
                        format!("`{name}` is a built-in policy and cannot be redefined"),
                    )
                    .at(span.clone()),
                );
                continue;
            }
            None => {}
        }
        if names.insert(name.to_string(), "policy").is_some() {
            diags.push(
                Diagnostic::error(
                    codes::E_DUPLICATE_NAME,
                    format!("duplicate policy name `{name}`"),
                )
                .at(span.clone()),
            );
            continue;
        }
        match parse_policy(name, def, span) {
            Ok(p) => policies.push(p),
            Err(e) => diags.push(Diagnostic::from_parse(e, span.clone())),
        }
    }
    let mut groups = Vec::new();
    for (name, def, span) in definition_entries(&profile, "Proxy Group", &mut diags) {
        if Builtin::parse(name).is_some() {
            diags.push(
                Diagnostic::error(
                    codes::E_BUILTIN_REDEFINED,
                    format!("`{name}` is a built-in policy and cannot be a group name"),
                )
                .at(span.clone()),
            );
            continue;
        }
        if names.insert(name.to_string(), "group").is_some() {
            diags.push(
                Diagnostic::error(
                    codes::E_DUPLICATE_NAME,
                    format!("duplicate policy group name `{name}`"),
                )
                .at(span.clone()),
            );
            continue;
        }
        match parse_group(name, def, span) {
            Ok(g) => groups.push(g),
            Err(e) => diags.push(Diagnostic::from_parse(e, span.clone())),
        }
    }

    // Inline rule sets.
    let mut rulesets = Vec::new();
    let mut ruleset_names: HashSet<String> = HashSet::new();
    for sec in profile.sections_with_prefix("Ruleset ") {
        let name = sec.name["Ruleset ".len()..].trim().to_string();
        if !ruleset_names.insert(name.clone()) {
            diags.push(
                Diagnostic::warning(
                    codes::W_DUPLICATE_RULESET,
                    format!("duplicate [Ruleset {name}] ignored; the first definition is used"),
                )
                .at(sec.span.clone()),
            );
            continue;
        }
        let mut rules = Vec::new();
        for e in sec.active_entries() {
            match parse_subrule(&e.raw, &ctx) {
                Ok(r) => {
                    // Ruling R12: unknown flags on an inline [Ruleset] line warn at that line's span.
                    let mut unknown = Vec::new();
                    collect_subrule_unknowns(&r, &mut unknown);
                    for u in &unknown {
                        diags.push(
                            Diagnostic::warning(
                                codes::W_UNKNOWN_RULE_PARAM,
                                format!("unknown [{}] parameter `{u}` ignored", sec.name),
                            )
                            .at(e.span.clone()),
                        );
                    }
                    rules.push(r);
                }
                Err(err) => diags.push(
                    Diagnostic::warning(
                        codes::W_RULESET_LINE_SKIPPED,
                        format!("[{}]: line skipped: {}", sec.name, err.message),
                    )
                    .at(e.span.clone()),
                ),
            }
        }
        rulesets.push(InlineRuleset {
            name,
            rules,
            span: sec.span.clone(),
        });
    }

    // Rules.
    let mut rules = Vec::new();
    if let Some(sec) = profile.section("Rule") {
        for e in sec.active_entries() {
            match parse_rule(&e.raw, &ctx, &e.span) {
                Ok(r) => {
                    for u in &r.params.unknown {
                        diags.push(
                            Diagnostic::warning(
                                codes::W_UNKNOWN_RULE_PARAM,
                                format!("unknown rule parameter `{u}` ignored"),
                            )
                            .at(e.span.clone()),
                        );
                    }
                    // Ruling R12: unknown flags on sub-rules nested in AND/OR/NOT warn at the parent rule's span.
                    let mut sub_unknown = Vec::new();
                    collect_kind_unknowns(&r.kind, &mut sub_unknown);
                    for u in &sub_unknown {
                        diags.push(
                            Diagnostic::warning(
                                codes::W_UNKNOWN_RULE_PARAM,
                                format!("unknown sub-rule parameter `{u}` ignored"),
                            )
                            .at(e.span.clone()),
                        );
                    }
                    rules.push(r);
                }
                Err(err) => diags.push(Diagnostic::from_parse(err, e.span.clone())),
            }
        }
    }

    // Hosts and keystore.
    let mut hosts = Vec::new();
    if let Some(sec) = profile.section("Host") {
        for e in sec.active_entries() {
            match parse_host_entry(&e.raw, &ctx, &e.span) {
                Ok(h) => hosts.push(h),
                Err(err) => diags.push(Diagnostic::from_parse(err, e.span.clone())),
            }
        }
    }
    let mut keystore = Vec::new();
    for (name, def, span) in definition_entries(&profile, "Keystore", &mut diags) {
        match parse_keystore_item(name, def, span) {
            Ok(k) => {
                // Ruling R14: unknown [Keystore] fields warn at the item's span.
                for u in &k.unknown {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_UNKNOWN_KEY,
                            format!("unknown [Keystore] field `{u}` ignored"),
                        )
                        .at(span.clone()),
                    );
                }
                keystore.push(k);
            }
            Err(err) => diags.push(Diagnostic::from_parse(err, span.clone())),
        }
    }

    // Deferred and unknown sections.
    let deferred = DeferredSections::collect(&profile);
    if !deferred.sections.is_empty() {
        let list: Vec<String> = deferred
            .sections
            .iter()
            .map(|s| format!("[{}]", s.name))
            .collect();
        diags.push(Diagnostic::warning(
            codes::W_DEFERRED_SECTION,
            format!(
                "sections parsed but inactive in this version: {}",
                list.join(", ")
            ),
        ));
    }
    let mut unknown_sections = Vec::new();
    for sec in profile
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::Unknown)
    {
        if !unknown_sections.contains(&sec.name) {
            diags.push(
                Diagnostic::warning(
                    codes::W_UNKNOWN_SECTION,
                    format!("unknown section [{}] is kept but ignored", sec.name),
                )
                .at(sec.span.clone()),
            );
            unknown_sections.push(sec.name.clone());
        }
    }

    let mut includes: Vec<PathBuf> = Vec::new();
    for sec in &profile.sections {
        for e in &sec.entries {
            if let Origin::Include(p) = &e.origin {
                let p = p.to_path_buf();
                if !includes.contains(&p) {
                    includes.push(p);
                }
            }
        }
    }

    let mut config = Config {
        general,
        policies,
        groups,
        rules,
        rulesets,
        hosts,
        keystore,
        deferred,
        managed,
        unknown_sections,
        source: SourceInfo { main, includes },
    };
    validate(&mut config, opts, &mut diags);
    Loaded {
        config,
        diagnostics: diags,
    }
}

fn validate(config: &mut Config, opts: &LoadOptions, diags: &mut Diagnostics) {
    let exists = |name: &str, config: &Config| config.resolve_policy(name).is_some();

    // Rule policy references.
    let mut ios_warned: HashSet<Builtin> = HashSet::new();
    for r in &config.rules {
        match &r.policy {
            PolicyRef::Named(n) if !exists(n, config) => {
                diags.push(
                    Diagnostic::error(
                        codes::E_UNKNOWN_POLICY_REF,
                        format!("rule references unknown policy `{n}`"),
                    )
                    .at(r.span.clone()),
                );
            }
            PolicyRef::Device(d) => {
                diags.push(
                    Diagnostic::warning(
                        codes::W_DEVICE_POLICY_AS_REJECT,
                        format!("Ponte policy `DEVICE:{d}` is not supported; treated as REJECT"),
                    )
                    .at(r.span.clone()),
                );
            }
            PolicyRef::Builtin(b) if b.is_ios_only() && ios_warned.insert(*b) => {
                diags.push(
                    Diagnostic::warning(
                        codes::W_IOS_BUILTIN_AS_DIRECT,
                        format!("`{}` is iOS-only; treated as DIRECT", b.name()),
                    )
                    .at(r.span.clone()),
                );
            }
            _ => {}
        }
    }

    // Group members, subnet conditions and `default` all reference policies.
    fn group_refs(g: &PolicyGroup) -> Vec<String> {
        g.members
            .iter()
            .cloned()
            .chain(g.conditions.iter().map(|(_, p)| p.clone()))
            .chain(g.params.get("default").map(str::to_string))
            .collect()
    }
    for g in &config.groups {
        for m in group_refs(g) {
            if !exists(&m, config) {
                diags.push(
                    Diagnostic::error(
                        codes::E_UNKNOWN_GROUP_MEMBER,
                        format!("policy group `{}` references unknown policy `{m}`", g.name),
                    )
                    .at(g.span.clone()),
                );
            }
        }
    }

    // Group cycles (DFS with colours) over every reference kind.
    let index: HashMap<&str, usize> = config
        .groups
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.as_str(), i))
        .collect();
    let mut colour = vec![0u8; config.groups.len()];
    fn dfs(
        i: usize,
        groups: &[PolicyGroup],
        index: &HashMap<&str, usize>,
        colour: &mut [u8],
        diags: &mut Diagnostics,
    ) {
        colour[i] = 1;
        for m in group_refs(&groups[i]) {
            if let Some(&j) = index.get(m.as_str()) {
                if colour[j] == 1 {
                    diags.push(
                        Diagnostic::error(
                            codes::E_GROUP_CYCLE,
                            format!(
                                "policy group `{}` and `{}` reference each other",
                                groups[i].name, groups[j].name
                            ),
                        )
                        .at(groups[i].span.clone()),
                    );
                } else if colour[j] == 0 {
                    dfs(j, groups, index, colour, diags);
                }
            }
        }
        colour[i] = 2;
    }
    for i in 0..config.groups.len() {
        if colour[i] == 0 {
            dfs(i, &config.groups, &index, &mut colour, diags);
        }
    }

    // FINAL: the last FINAL takes effect. Earlier FINAL lines are shadowed
    // (W0021); non-FINAL rules after the last FINAL never run (W0019).
    match config.effective_final() {
        None => {
            let span = config.rules.last().map(|r| r.span.clone());
            let d = Diagnostic::error(
                codes::E_MISSING_FINAL,
                "the [Rule] section must end with an enabled FINAL rule",
            )
            .with_hint("add `FINAL,DIRECT` as the last rule");
            diags.push(match span {
                Some(s) => d.at(s),
                None => d,
            });
        }
        Some(last) => {
            for r in &config.rules[..last] {
                if matches!(r.kind, RuleKind::Final) {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_DUPLICATE_FINAL,
                            "this FINAL is shadowed; the last FINAL rule takes effect",
                        )
                        .at(r.span.clone()),
                    );
                }
            }
            let dead = &config.rules[last + 1..];
            if let Some(first) = dead.first() {
                diags.push(
                    Diagnostic::warning(
                        codes::W_RULES_AFTER_FINAL,
                        format!("{} rule(s) after FINAL never take effect", dead.len()),
                    )
                    .at(first.span.clone()),
                );
            }
        }
    }

    // Capabilities.
    let mut seen_kinds: HashSet<PolicyKind> = HashSet::new();
    for p in &config.policies {
        if !opts.capabilities.policy_kinds.contains(&p.kind) && seen_kinds.insert(p.kind) {
            diags.push(Diagnostic::warning(codes::W_PROTOCOL_NOT_IMPLEMENTED, format!("policy type `{}` is not implemented in this version; such policies behave as REJECT", p.kind.keyword())).at(p.span.clone()));
        }
    }
    let mut seen_groups: HashSet<GroupKind> = HashSet::new();
    for g in &config.groups {
        if !opts.capabilities.group_kinds.contains(&g.kind) && seen_groups.insert(g.kind) {
            diags.push(Diagnostic::warning(codes::W_GROUP_NOT_IMPLEMENTED, format!("policy group type `{}` is not implemented in this version; the first member is used", g.kind.keyword())).at(g.span.clone()));
        }
    }
    let mut seen_rules: HashSet<&'static str> = HashSet::new();
    for r in &config.rules {
        let t = r.kind.type_name();
        if !opts.capabilities.rule_types.contains(t) && seen_rules.insert(t) {
            diags.push(
                Diagnostic::warning(
                    codes::W_RULE_NEVER_MATCHES,
                    format!(
                        "`{t}` rules never match on {} in this version",
                        opts.platform.system_name()
                    ),
                )
                .at(r.span.clone()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn load_text(text: &str) -> Loaded {
        from_text(
            text,
            Path::new("/profiles/t.conf"),
            &LoadOptions::for_tests(),
        )
    }

    fn codes_of(l: &Loaded) -> Vec<&'static str> {
        l.diagnostics.iter().map(|d| d.code).collect()
    }

    const QUICK_START: &str = "[General]\ndns-server = system, 1.1.1.1, 8.8.8.8\n\n[Proxy]\nProxyA = https, proxy.example.com, 443, username, password\n\n[Proxy Group]\nProxy = select, ProxyA, DIRECT\n\n[Rule]\nDOMAIN-SUFFIX,example.com,Proxy\nGEOIP,CN,DIRECT\nFINAL,Proxy\n";

    #[test]
    fn quick_start_loads_cleanly() {
        let l = load_text(QUICK_START);
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let c = l.config;
        assert_eq!(c.policies.len(), 1);
        assert_eq!(c.groups.len(), 1);
        assert_eq!(c.rules.len(), 3);
        assert!(matches!(
            c.resolve_policy("Proxy"),
            Some(PolicyTarget::Group(_))
        ));
        assert!(matches!(
            c.resolve_policy("ProxyA"),
            Some(PolicyTarget::Proxy(_))
        ));
        assert!(matches!(
            c.resolve_policy("REJECT"),
            Some(PolicyTarget::Builtin(Builtin::Reject))
        ));
        assert!(c.resolve_policy("nope").is_none());
        assert_eq!(c.source.main, Path::new("/profiles/t.conf"));
        let s = c.summary();
        assert_eq!(s.rules.len(), 3);
        assert_eq!(s.policies, ["ProxyA (https)"]);
    }

    #[test]
    fn reference_errors() {
        let l = load_text(
            "[Proxy]\nA = direct\n[Proxy Group]\nG1 = select, G2, A\nG2 = select, G1\nG3 = select, Missing\n[Rule]\nDOMAIN,a,Nope\nFINAL,DIRECT\n",
        );
        let c = codes_of(&l);
        assert!(c.contains(&codes::E_UNKNOWN_POLICY_REF));
        assert!(c.contains(&codes::E_UNKNOWN_GROUP_MEMBER));
        assert!(c.contains(&codes::E_GROUP_CYCLE));
        assert!(l.diagnostics.has_errors());
    }

    #[test]
    fn final_rules() {
        let l = load_text("[Rule]\nDOMAIN,a,DIRECT\n");
        assert!(codes_of(&l).contains(&codes::E_MISSING_FINAL));
        let l = load_text("[Rule]\nFINAL,DIRECT\nDOMAIN,a,DIRECT\n");
        assert!(!l.diagnostics.has_errors());
        assert!(codes_of(&l).contains(&codes::W_RULES_AFTER_FINAL));
        let l = load_text("[Rule]\nFINAL,DIRECT #!IOS-ONLY\n");
        assert!(codes_of(&l).contains(&codes::E_MISSING_FINAL));
    }

    #[test]
    fn dead_rules_after_last_final_are_counted() {
        // The last FINAL takes effect (manual: "if there are multiple FINAL
        // rules, the last one is used"), so a rule between two FINALs is not
        // dead — the earlier FINAL is shadowed instead (W0021).
        let l = load_text("[Rule]\nFINAL,DIRECT\nDOMAIN,a,DIRECT\nFINAL,REJECT\n");
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let codes = codes_of(&l);
        assert!(codes.contains(&codes::W_DUPLICATE_FINAL));
        assert!(!codes.contains(&codes::W_RULES_AFTER_FINAL));

        let l = load_text("[Rule]\nFINAL,DIRECT\nFINAL,REJECT\n");
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let codes = codes_of(&l);
        assert!(codes.contains(&codes::W_DUPLICATE_FINAL));
        assert!(!codes.contains(&codes::W_RULES_AFTER_FINAL));
    }

    #[test]
    fn last_final_takes_effect_and_earlier_final_is_shadowed() {
        let l = load_text("[Proxy]\nA = direct\n[Rule]\nFINAL,DIRECT\nDOMAIN,a.com,A\nFINAL,A\n");
        assert_eq!(l.config.effective_final(), Some(2));
        let codes = codes_of(&l);
        assert!(codes.contains(&codes::W_DUPLICATE_FINAL));
        assert!(!codes.contains(&codes::W_RULES_AFTER_FINAL));
    }

    #[test]
    fn rules_after_last_final_are_dead() {
        let l = load_text("[Rule]\nFINAL,DIRECT\nDOMAIN,a.com,DIRECT\n");
        let d: Vec<_> = l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_RULES_AFTER_FINAL)
            .collect();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].span.as_ref().map(|s| s.line), Some(3));
    }

    #[test]
    fn proxy_hostnames_collects_domains_only() {
        let l = load_text(
            "[Proxy]\nA = http, proxy.example.com, 8080\nB = socks5, 10.0.0.1, 1080\n[Rule]\nFINAL,DIRECT\n",
        );
        let names = l.config.proxy_hostnames();
        assert!(names.contains("proxy.example.com"));
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn names_and_builtins() {
        let l = load_text(
            "[Proxy]\nDIRECT = direct\nREJECT = direct\nA = direct\nA = reject\n[Proxy Group]\nA = select, DIRECT\n[Rule]\nFINAL,DIRECT\n",
        );
        let c = codes_of(&l);
        assert_eq!(
            l.config.policies.len(),
            1,
            "DIRECT redefinition dropped silently, duplicates rejected"
        );
        assert!(c.contains(&codes::E_BUILTIN_REDEFINED));
        assert_eq!(
            c.iter().filter(|x| **x == codes::E_DUPLICATE_NAME).count(),
            2
        );
    }

    #[test]
    fn warnings() {
        let l = load_text(
            "[General]\nloglevel = notify\n[Weird]\nx = 1\n[MITM]\nhostname = *\n[Script]\ns = type=generic, script-path=a.js\n[Ruleset Inline]\nDOMAIN,a.com\nBOGUS,b\nFINAL,DIRECT\nDOMAIN-SUFFIX,b.com,extended-matching\n[Rule]\nRULE-SET,Inline,DIRECT,mystery\nDOMAIN,a,CELLULAR\nDOMAIN,b,DEVICE:Home\nDOMAIN,c,HYBRID\nFINAL,DIRECT\n",
        );
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let c = codes_of(&l);
        assert!(c.contains(&codes::W_UNKNOWN_SECTION));
        assert_eq!(
            c.iter()
                .filter(|x| **x == codes::W_DEFERRED_SECTION)
                .count(),
            1
        );
        assert_eq!(
            c.iter()
                .filter(|x| **x == codes::W_RULESET_LINE_SKIPPED)
                .count(),
            2
        );
        assert_eq!(l.config.rulesets[0].rules.len(), 2);
        assert!(c.contains(&codes::W_UNKNOWN_RULE_PARAM));
        assert_eq!(
            c.iter()
                .filter(|x| **x == codes::W_IOS_BUILTIN_AS_DIRECT)
                .count(),
            2
        );
        assert!(c.contains(&codes::W_DEVICE_POLICY_AS_REJECT));
        assert_eq!(l.config.unknown_sections, ["Weird"]);
        assert_eq!(l.config.deferred.sections.len(), 2);
    }

    #[test]
    fn duplicate_inline_ruleset_name_warns_and_keeps_first() {
        let l = load_text(
            "[Ruleset A]\nDOMAIN,a\n[Ruleset A]\nDOMAIN,b\n[Rule]\nRULE-SET,A,DIRECT\nFINAL,DIRECT\n",
        );
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let c = codes_of(&l);
        assert_eq!(
            c.iter()
                .filter(|x| **x == codes::W_DUPLICATE_RULESET)
                .count(),
            1
        );
        assert_eq!(l.config.rulesets.len(), 1);
        assert_eq!(l.config.rulesets[0].rules.len(), 1);
        assert_eq!(l.config.rulesets[0].rules[0].raw, "DOMAIN,a");
    }

    #[test]
    fn capability_warnings() {
        let mut opts = LoadOptions::for_tests();
        opts.capabilities.policy_kinds = HashSet::from([PolicyKind::Direct]);
        opts.capabilities.group_kinds = HashSet::from([GroupKind::Select]);
        opts.capabilities.rule_types.remove("PROCESS-NAME");
        let l = from_text(
            "[Proxy]\nA = ss, 1.2.3.4, 1, encrypt-method=aes-128-gcm, password=x\nB = ss, 1.2.3.4, 2, encrypt-method=aes-128-gcm, password=x\n[Proxy Group]\nG = url-test, A, B\n[Rule]\nPROCESS-NAME,ssh,DIRECT\nPROCESS-NAME,curl,DIRECT\nFINAL,G\n",
            Path::new("/p/t.conf"),
            &opts,
        );
        let c = codes_of(&l);
        assert_eq!(
            c.iter()
                .filter(|x| **x == codes::W_PROTOCOL_NOT_IMPLEMENTED)
                .count(),
            1
        );
        assert_eq!(
            c.iter()
                .filter(|x| **x == codes::W_GROUP_NOT_IMPLEMENTED)
                .count(),
            1
        );
        assert_eq!(
            c.iter()
                .filter(|x| **x == codes::W_RULE_NEVER_MATCHES)
                .count(),
            1
        );
        assert!(!l.diagnostics.has_errors());
    }

    #[test]
    fn includes_are_recorded_in_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("main.conf"),
            "[Proxy]\n#!include p.dconf\n[Rule]\nFINAL,A\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("p.dconf"), "[Proxy]\nA = direct\n").unwrap();
        let l = load(&dir.path().join("main.conf"), &LoadOptions::for_tests()).unwrap();
        assert!(!l.diagnostics.has_errors());
        assert_eq!(l.config.source.includes.len(), 1);
        assert!(l.config.source.includes[0].ends_with("p.dconf"));
        assert!(matches!(
            load(
                Path::new("/definitely/missing.conf"),
                &LoadOptions::for_tests()
            ),
            Err(LoadError::Io { .. })
        ));
    }

    #[test]
    fn logical_rule_unknown_subrule_flag_warns() {
        // Ruling R12: an unknown flag on a sub-rule nested inside a logical
        // AND/OR/NOT rule must surface as W_UNKNOWN_RULE_PARAM at the parent
        // rule's span, even though `RuleParams::unknown` (the top-level rule's
        // own flags) is empty here.
        let l = load_text("[Rule]\nAND,((DOMAIN,a,bogus),(DOMAIN,b)),DIRECT\nFINAL,DIRECT\n");
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        assert_eq!(l.config.rules[0].params.unknown.len(), 0);
        assert_eq!(
            codes_of(&l)
                .iter()
                .filter(|x| **x == codes::W_UNKNOWN_RULE_PARAM)
                .count(),
            1
        );
    }

    #[test]
    fn keystore_item_unknown_field_warns() {
        // Ruling R14: an unrecognised `key=value`/positional field on a
        // [Keystore] item must surface as W_UNKNOWN_KEY at the item's span.
        let l =
            load_text("[Keystore]\ncert1 = type=p12, base64=AAAA, foo=bar\n[Rule]\nFINAL,DIRECT\n");
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        assert_eq!(l.config.keystore.len(), 1);
        assert_eq!(
            codes_of(&l)
                .iter()
                .filter(|x| **x == codes::W_UNKNOWN_KEY)
                .count(),
            1
        );
    }
}
