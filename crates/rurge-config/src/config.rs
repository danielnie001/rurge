//! Semantic layer: `Config` assembled from a `Profile`, plus cross validation.

use crate::deferred::DeferredSections;
use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::general::{General, parse_general};
use crate::host::{HostEntry, HostKey, parse_host_entry};
use crate::keystore::{KeystoreItem, parse_keystore_item, unknown_field};
use crate::managed::{ManagedConfig, parse_managed};
use crate::policy::{
    Builtin, GroupKind, PolicyGroup, PolicyKind, ProxyPolicy, is_name, named, parse_group,
    parse_policy, quoted_name, shown_members,
};
use crate::requirement::{self, Environment};
use crate::rule::{
    ParseCtx, PolicyRef, ResourceRef, Rule, RuleKind, SubRule, parse_rule, parse_subrule,
};
use crate::span::Span;
use crate::spec::{
    GroupSpec, NameKind, NotImplemented, PolicySpec, ProtoSpec, Remark, SpecEnv, to_group_spec,
    to_spec,
};
use crate::text::include::{self, IncludeOptions};
use crate::text::{Origin, Profile, SectionKind, parse_str};
use crate::value::{split_definition, split_list};
use crate::wireguard::{WireGuardSection, parse_section};
use base64::Engine as _;
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
    /// Typed parameters of every policy whose type has a spec (same order as
    /// `policies`; policies with errors are absent).
    pub specs: Vec<PolicySpec>,
    /// Why a policy without errors has no spec, when its line asks for
    /// something rurge does not implement (`W0007`); by policy name.
    pub not_implemented: HashMap<String, NotImplemented>,
    pub groups: Vec<PolicyGroup>,
    /// Typed parameters of every group (same order as `groups`; groups
    /// with errors are absent).
    pub group_specs: Vec<GroupSpec>,
    pub rules: Vec<Rule>,
    pub rulesets: Vec<InlineRuleset>,
    pub hosts: Vec<HostEntry>,
    pub keystore: Vec<KeystoreItem>,
    /// Every `[WireGuard <name>]` section without errors, in profile order.
    pub wireguard: Vec<WireGuardSection>,
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

    pub fn spec(&self, name: &str) -> Option<&PolicySpec> {
        self.specs.iter().find(|s| s.name == name)
    }

    /// What `name` refers to, as a policy or group line sees it.
    pub fn name_kind(&self, name: &str) -> Option<NameKind> {
        Some(match self.resolve_policy(name)? {
            PolicyTarget::Builtin(b) => NameKind::Builtin(b),
            PolicyTarget::Proxy(p) => NameKind::Policy(p.kind),
            PolicyTarget::Group(_) => NameKind::Group,
        })
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
                        shown_members(&g.members).join(", ")
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

    /// Lowercase hostnames of every proxy server — WireGuard peers' endpoints
    /// among them; `[Host]` never applies to them. A server written in
    /// Unicode is here as its A-labels too: its spec holds that form, and it
    /// is the one its outbound looks up.
    pub fn proxy_hostnames(&self) -> HashSet<String> {
        let endpoints = self
            .wireguard
            .iter()
            .flat_map(|w| &w.peers)
            .map(|p| &p.endpoint.host);
        let written = self.policies.iter().filter_map(|p| p.server.as_ref());
        let looked_up = self.specs.iter().filter_map(|s| s.server.as_ref());
        written
            .chain(looked_up)
            .chain(endpoints)
            .filter_map(|host| host.as_domain().map(str::to_string))
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
                // A name never holds a comma (member lists are
                // comma-separated): one that does came from the parameters'
                // first `=` standing in for a missing one after the name.
                Some((name, def)) if !name.contains(',') => out.push((name, def, &e.span)),
                _ => diags.push(
                    Diagnostic::error(
                        codes::E_INVALID_DEFINITION,
                        // nothing of the line: once its `=` is missing, its
                        // fields slide around, a password among them (M3-D7)
                        format!("[{section}]: expected `Name = ...`"),
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
                    format!("duplicate {}", named("policy name", name)),
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
                    format!("duplicate {}", named("policy group name", name)),
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
                        Diagnostic::warning(codes::W_UNKNOWN_KEY, unknown_field(def, u))
                            .at(span.clone()),
                    );
                }
                if !is_base64(&k.base64) {
                    diags.push(
                        Diagnostic::error(
                            codes::E_KEYSTORE_BASE64,
                            format!(
                                "{}: `base64` is not valid Base64",
                                named("keystore item", &k.name)
                            ),
                        )
                        .at(span.clone()),
                    );
                }
                keystore.push(k);
            }
            Err(err) => diags.push(Diagnostic::from_parse(err, span.clone())),
        }
    }

    // [WireGuard <name>] sections: every one is checked, used or not.
    let mut wireguard = Vec::new();
    let mut wireguard_names: HashSet<String> = HashSet::new();
    for sec in profile.sections_with_prefix("WireGuard ") {
        let name = sec.name["WireGuard ".len()..].trim();
        if !wireguard_names.insert(name.to_string()) {
            diags.push(
                Diagnostic::warning(
                    codes::W_DUPLICATE_RULESET,
                    format!("duplicate [WireGuard {name}] ignored; the first definition is used"),
                )
                .at(sec.span.clone()),
            );
            continue;
        }
        wireguard.extend(parse_section(sec, &mut diags));
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
        specs: Vec::new(),
        not_implemented: HashMap::new(),
        groups,
        group_specs: Vec::new(),
        rules,
        rulesets,
        hosts,
        keystore,
        wireguard,
        deferred,
        managed,
        unknown_sections,
        source: SourceInfo { main, includes },
    };
    validate(&mut config, base_dir, opts, &mut diags);
    Loaded {
        config,
        diagnostics: diags,
    }
}

/// Group members, subnet conditions and `default` all reference policies.
fn group_refs(g: &PolicyGroup) -> Vec<String> {
    g.members
        .iter()
        .cloned()
        .chain(g.conditions.iter().map(|(_, p)| p.clone()))
        .chain(g.params.get("default").map(str::to_string))
        .collect()
}

/// `group_refs` plus the groups whose members `include-other-group` takes:
/// every edge a group cycle can run along. A `subnet` group ignores
/// `include-other-group` (`W0028`), so it is not an edge there either.
fn group_edges(g: &PolicyGroup) -> Vec<String> {
    let mut edges = group_refs(g);
    if g.kind != GroupKind::Subnet
        && let Some(v) = g.params.get("include-other-group")
    {
        edges.extend(split_list(v));
    }
    edges
}

fn is_base64(text: &str) -> bool {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
    STANDARD.decode(text).is_ok() || STANDARD_NO_PAD.decode(text).is_ok()
}

/// `E0019`: follows `underlying-proxy` edges, group membership,
/// `include-other-group` and (for a group with `include-all-proxies`) the
/// `[Proxy]` policies it takes in, from each chained policy and from each
/// group with a relay of its own; reaching the start again is a cycle.
fn underlying_cycles(
    config: &Config,
    specs: &[PolicySpec],
    groups: &[GroupSpec],
    diags: &mut Diagnostics,
) {
    let mut edges: HashMap<&str, Vec<String>> = HashMap::new();
    for s in specs {
        if let Some(u) = &s.common.underlying_proxy {
            edges.insert(s.name.as_str(), vec![u.clone()]);
        }
    }
    for g in &config.groups {
        let mut next = group_edges(g);
        if let Some(spec) = groups.iter().find(|s| s.name == g.name) {
            // every proxy member of a group with a relay is dialled through it
            if let Some(relay) = &spec.underlying_proxy {
                next.push(relay.clone());
            }
            // `include-all-proxies` takes in every un-aliased proxy its
            // filter admits (M3 design 5.3 step 3), dialled through the
            // group exactly like a written member
            if spec.import.include_all_proxies {
                next.extend(
                    config
                        .policies
                        .iter()
                        .filter(|p| !p.kind.is_builtin_alias() && spec.import.admits(&p.name))
                        .map(|p| p.name.clone()),
                );
            }
        }
        edges.insert(g.name.as_str(), next);
    }
    let leads_back = |start: &str, first: &str| {
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack = vec![first.to_string()];
        while let Some(name) = stack.pop() {
            if name == start {
                return true;
            }
            if seen.insert(name.clone())
                && let Some(next) = edges.get(name.as_str())
            {
                stack.extend(next.iter().cloned());
            }
        }
        false
    };
    for s in specs {
        if let Some(first) = &s.common.underlying_proxy
            && leads_back(&s.name, first)
        {
            diags.push(
                Diagnostic::error(
                    codes::E_UNDERLYING_PROXY_CYCLE,
                    format!(
                        "{}: `underlying-proxy` leads back to the policy itself (via {})",
                        named("policy", &s.name),
                        quoted_name(first)
                    ),
                )
                .at(s.span.clone()),
            );
        }
    }
    for g in groups {
        if let Some(first) = &g.underlying_proxy
            && leads_back(&g.name, first)
        {
            diags.push(
                Diagnostic::error(
                    codes::E_UNDERLYING_PROXY_CYCLE,
                    format!(
                        "{}: `underlying-proxy` leads back to the group itself (via {})",
                        named("policy group", &g.name),
                        quoted_name(first)
                    ),
                )
                .at(g.span.clone()),
            );
        }
    }
}

/// `W0023`: a group with a relay chains every proxy it takes in through it
/// (`M (via R)`, phase 2 M3 design 5.4), but port hopping cannot go through
/// a chain: the registry leaves such a member out, and a group left without
/// members falls back to DIRECT unless `--empty-group-reject` (M3-D3). Said
/// here for the proxies the profile itself gives the group; what a
/// `policy-path` brings in is not known at load, and the registry leaves
/// those out with a log line only.
fn hops_under_group_relays(
    config: &Config,
    specs: &[PolicySpec],
    groups: &[GroupSpec],
    diags: &mut Diagnostics,
) {
    let hops: HashSet<&str> = specs
        .iter()
        .filter(|s| {
            s.proto
                .quic()
                .is_some_and(|quic| quic.port_hopping.is_some())
        })
        .map(|s| s.name.as_str())
        .collect();
    if hops.is_empty() {
        return;
    }
    let by_name: HashMap<&str, &GroupSpec> = groups.iter().map(|g| (g.name.as_str(), g)).collect();
    let mut taken = HashMap::new();
    for g in groups {
        let Some(relay) = &g.underlying_proxy else {
            continue;
        };
        let mut said = HashSet::new();
        for m in taken_in(config, &by_name, g, &mut taken) {
            if hops.contains(m) && said.insert(m) {
                diags.push(
                    Diagnostic::warning(
                        codes::W_SET_LINES_SKIPPED,
                        format!(
                            "{} relays its members through `underlying-proxy`: {} uses `port-hopping`, which cannot go through it; {} is left out",
                            named("policy group", &g.name),
                            quoted_name(m),
                            quoted_name(&format!("{m} (via {relay})"))
                        ),
                    )
                    .at(g.span.clone()),
                );
            }
        }
    }
}

/// The names `g` takes in from the profile itself, in the registry's order
/// (M3 design 5.3): written, then those of the groups `include-other-group`
/// names, then the proxies of `include-all-proxies`. `taken` keeps each
/// group's; a group met again on an `include-other-group` cycle gives
/// nothing more (the cycle is `W0030`).
fn taken_in<'a>(
    config: &'a Config,
    groups: &HashMap<&'a str, &'a GroupSpec>,
    g: &'a GroupSpec,
    taken: &mut HashMap<&'a str, Vec<&'a str>>,
) -> Vec<&'a str> {
    if let Some(names) = taken.get(g.name.as_str()) {
        return names.clone();
    }
    taken.insert(&g.name, Vec::new());
    let mut names: Vec<&str> = g.members.iter().map(String::as_str).collect();
    for other in &g.import.include_other_groups {
        if let Some(other) = groups.get(other.as_str()) {
            let theirs = taken_in(config, groups, other, taken);
            names.extend(theirs.into_iter().filter(|m| g.import.admits(m)));
        }
    }
    if g.import.include_all_proxies {
        names.extend(
            config
                .policies
                .iter()
                .filter(|p| !p.kind.is_builtin_alias() && g.import.admits(&p.name))
                .map(|p| p.name.as_str()),
        );
    }
    taken.insert(&g.name, names.clone());
    names
}

/// A set a profile line names: the resource, the keyword it is named by and
/// the line.
type SetUse<'a> = (&'a ResourceRef, &'static str, &'a Span);

/// The sets `kind` names — itself, or in the sub-rules of a logical rule —,
/// all of them on `span`, the line that holds the rule.
fn collect_set_uses<'a>(kind: &'a RuleKind, span: &'a Span, out: &mut Vec<SetUse<'a>>) {
    match kind {
        RuleKind::RuleSet(r) => out.push((r, "RULE-SET", span)),
        RuleKind::DomainSet(r) => out.push((r, "DOMAIN-SET", span)),
        RuleKind::And(subs) | RuleKind::Or(subs) => {
            for sub in subs {
                collect_set_uses(&sub.kind, span, out);
            }
        }
        RuleKind::Not(sub) => collect_set_uses(&sub.kind, span, out),
        _ => {}
    }
}

/// `E0024`: a URL or a file used as a `RULE-SET` and as a `DOMAIN-SET`. The
/// two read the same text in different formats, so one of them is a set
/// that is empty, and the rules after it silently do not apply.
///
/// Uses are taken in the order `Config` keeps them — the rules, the `[Host]`
/// entries, the inline sets —, which is not the order of the file. The use
/// reported, once per resource, is the later one in that order; the text
/// names the kind and the line of the use it disagrees with without saying
/// which comes first in the file, and quotes nothing of the reference, which
/// can carry a password. A set of the profile or a built-in one is no
/// resource here.
fn set_kind_conflicts(config: &Config, diags: &mut Diagnostics) {
    let mut uses = Vec::new();
    for rule in &config.rules {
        collect_set_uses(&rule.kind, &rule.span, &mut uses);
    }
    for host in &config.hosts {
        match &host.key {
            HostKey::RuleSet(r) => uses.push((r, "RULE-SET", &host.span)),
            HostKey::DomainSet(r) => uses.push((r, "DOMAIN-SET", &host.span)),
            HostKey::Pattern(_) => {}
        }
    }
    // a line of an inline set has no line of its own: the set's stands for it
    for set in &config.rulesets {
        for line in &set.rules {
            collect_set_uses(&line.kind, &set.span, &mut uses);
        }
    }
    // the use of each resource met first in that order, and whether the
    // resource has been reported
    let mut first: HashMap<&ResourceRef, (&str, &Span, bool)> = HashMap::new();
    for (resource, keyword, span) in uses {
        if !matches!(resource, ResourceRef::Url(_) | ResourceRef::File(_)) {
            continue;
        }
        let seen = first.entry(resource).or_insert((keyword, span, false));
        if seen.0 != keyword && !seen.2 {
            seen.2 = true;
            diags.push(
                Diagnostic::error(
                    codes::E_SET_KIND_CONFLICT,
                    format!(
                        "the same resource cannot be both a RULE-SET and a DOMAIN-SET \
(it is used as a {} at {})",
                        seen.0, seen.1
                    ),
                )
                .at(span.clone()),
            );
        }
    }
}

fn validate(config: &mut Config, base_dir: &Path, opts: &LoadOptions, diags: &mut Diagnostics) {
    let exists = |name: &str, config: &Config| config.resolve_policy(name).is_some();

    // Rule policy references.
    let mut ios_warned: HashSet<Builtin> = HashSet::new();
    for r in &config.rules {
        match &r.policy {
            PolicyRef::Named(n) if !exists(n, config) => {
                let told = if is_name(n) {
                    format!("unknown policy `{n}`")
                } else {
                    "an unknown policy".to_string()
                };
                diags.push(
                    Diagnostic::error(
                        codes::E_UNKNOWN_POLICY_REF,
                        format!("rule references {told}"),
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

    set_kind_conflicts(config, diags);

    for g in &config.groups {
        for m in group_refs(g) {
            if !exists(&m, config) {
                let told = if is_name(&m) {
                    format!("unknown policy `{m}`")
                } else {
                    "an unknown policy".to_string()
                };
                diags.push(
                    Diagnostic::error(
                        codes::E_UNKNOWN_GROUP_MEMBER,
                        format!("{} references {told}", named("policy group", &g.name)),
                    )
                    .at(g.span.clone()),
                );
            }
        }
    }

    // Group cycles (DFS with colours) over every reference kind: a warning,
    // the groups on one behave as REJECT (phase 2 M3 design 5.6).
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
        for m in group_edges(&groups[i]) {
            if let Some(&j) = index.get(m.as_str()) {
                if colour[j] == 1 {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_GROUP_CYCLE,
                            format!(
                                "policy groups {} and {} form a cycle; they behave as REJECT",
                                quoted_name(&groups[i].name),
                                quoted_name(&groups[j].name)
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

    // Typed policy and group parameters (phase 2 M1 design §4, M3 design §4).
    let (specs, not_implemented, group_specs) = {
        let cfg: &Config = config;
        let lookup = |name: &str| cfg.name_kind(name);
        let env = SpecEnv {
            keystore: &cfg.keystore,
            wireguard: &cfg.wireguard,
            lookup: &lookup,
        };
        let mut specs = Vec::new();
        let mut inert_seen: HashSet<&'static str> = HashSet::new();
        let mut ios_seen: HashSet<&'static str> = HashSet::new();
        let mut not_implemented = HashMap::new();
        let mut not_implemented_seen: HashSet<NotImplemented> = HashSet::new();
        let mut remarks_seen: HashSet<Remark> = HashSet::new();
        // `local-port` → the `external` policy that has it
        let mut local_ports: HashMap<u16, &str> = HashMap::new();
        for p in &cfg.policies {
            let mut outcome = to_spec(p, &env);
            for d in outcome.diagnostics {
                diags.push(d);
            }
            // two programs cannot listen on one port: one policy's
            // connections would reach the other's program (M4 design 4.4)
            if let Some(PolicySpec {
                proto: ProtoSpec::External(external),
                ..
            }) = &outcome.spec
            {
                let port = external.local_port;
                if let Some(key) = own_listener(&cfg.general, port) {
                    // rurge would dial itself, round and round
                    diags.push(
                        Diagnostic::error(
                            codes::E_INVALID_POLICY_PARAM,
                            format!(
                                "{}: `local-port` {port} is the port of rurge's own `{key}`",
                                named("policy", &p.name)
                            ),
                        )
                        .at(p.span.clone()),
                    );
                    outcome.spec = None;
                } else if let Some(first) = local_ports.get(&port) {
                    diags.push(
                        Diagnostic::error(
                            codes::E_INVALID_POLICY_PARAM,
                            format!(
                                "{}: `local-port` {} is also the `local-port` of {}",
                                named("policy", &p.name),
                                port,
                                named("policy", first)
                            ),
                        )
                        .at(p.span.clone()),
                    );
                    outcome.spec = None;
                } else {
                    local_ports.insert(port, p.name.as_str());
                }
            }
            for name in outcome.inert {
                if inert_seen.insert(name) {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_PARAM_NOT_EFFECTIVE,
                            format!("policy parameter `{name}` is parsed but has no effect in this version"),
                        )
                        .at(p.span.clone()),
                    );
                }
            }
            for name in outcome.ios_only {
                if ios_seen.insert(name) {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_PLATFORM_IGNORED,
                            format!("policy parameter `{name}` is iOS-only; ignored"),
                        )
                        .at(p.span.clone()),
                    );
                }
            }
            if let Some(why) = outcome.not_implemented {
                if not_implemented_seen.insert(why.clone()) {
                    diags.push(
                        Diagnostic::warning(codes::W_PROTOCOL_NOT_IMPLEMENTED, why.warning())
                            .at(p.span.clone()),
                    );
                }
                not_implemented.insert(p.name.clone(), why);
            }
            for remark in outcome.remarks {
                if remarks_seen.insert(remark.clone()) {
                    diags.push(
                        Diagnostic::warning(remark.code(), remark.warning()).at(p.span.clone()),
                    );
                }
            }
            specs.extend(outcome.spec);
        }
        let mut group_specs = Vec::new();
        for g in &cfg.groups {
            let outcome = to_group_spec(g, base_dir, &lookup);
            for d in outcome.diagnostics {
                diags.push(d);
            }
            group_specs.extend(outcome.spec);
        }
        underlying_cycles(cfg, &specs, &group_specs, diags);
        hops_under_group_relays(cfg, &specs, &group_specs, diags);
        (specs, not_implemented, group_specs)
    };
    config.specs = specs;
    config.not_implemented = not_implemented;
    config.group_specs = group_specs;

    // Capabilities.
    let mut seen_kinds: HashSet<PolicyKind> = HashSet::new();
    for p in &config.policies {
        if !opts.capabilities.policy_kinds.contains(&p.kind) && seen_kinds.insert(p.kind) {
            diags.push(
                Diagnostic::warning(codes::W_PROTOCOL_NOT_IMPLEMENTED, unsupported_kind(p.kind))
                    .at(p.span.clone()),
            );
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

/// The load warning for a policy type rurge does not implement (`W0007`).
/// The type is out of scope rather than still to come, so the text promises
/// nothing, and it names the way that works where there is one (phase 2 M8
/// design 4.4).
fn unsupported_kind(kind: PolicyKind) -> String {
    let instead = match kind {
        PolicyKind::Tailscale => {
            " a tailnet is reached through tailscaled's SOCKS5 proxy with a `socks5` or an `external` policy;"
        }
        _ => "",
    };
    format!(
        "policy type `{}` is not supported;{instead} such policies behave as REJECT",
        kind.keyword()
    )
}

/// The listener of rurge's own (`http-listen` or `socks5-listen`) that
/// an `external` program listening on `127.0.0.1:<port>` would be: one on
/// that port bound to a loopback or unspecified address.
fn own_listener(general: &General, port: u16) -> Option<&'static str> {
    [
        ("http-listen", &general.http_listen),
        ("socks5-listen", &general.socks5_listen),
    ]
    .into_iter()
    .find_map(|(key, listeners)| {
        listeners
            .iter()
            .any(|l| {
                l.addr.port() == port && (l.addr.ip().is_loopback() || l.addr.ip().is_unspecified())
            })
            .then_some(key)
    })
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

    /// A missing comma or `=` can leave a subscription token, or a
    /// password, sitting where the type keyword is read from: `unknown
    /// type` must never echo it (M3-D7). A genuine typo is still quoted.
    #[test]
    fn an_unknown_type_never_echoes_what_looks_like_a_secret() {
        for text in [
            "[Proxy]\nA = direct\n[Proxy Group]\n\
G = select policy-path=https://sub.test/nodes?token=t0k3n\n[Rule]\nFINAL,DIRECT\n",
            "[Proxy]\nA = direct\n[Proxy Group]\n\
G select, policy-path=https://sub.test/nodes?token=t0k3n\n[Rule]\nFINAL,DIRECT\n",
            "[Proxy]\nX = http host 80 alice s3cret\n[Proxy Group]\n\
G = select, A\n[Rule]\nFINAL,DIRECT\n",
        ] {
            let l = load_text(text);
            assert!(l.diagnostics.has_errors(), "{text}");
            for d in l.diagnostics.iter() {
                assert!(!d.message.contains("t0k3n"), "{text}: {}", d.message);
                assert!(!d.message.contains("s3cret"), "{text}: {}", d.message);
            }
        }
        let l =
            load_text("[Proxy]\nA = direct\n[Proxy Group]\nG = selekt, A\n[Rule]\nFINAL,DIRECT\n");
        let found: Vec<&str> = l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::E_UNKNOWN_POLICY_TYPE)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(found, ["policy group `G`: unknown type `selekt`"]);
    }

    /// Without the `=` right after the name, a line splits at the first `=`
    /// inside its parameters: the "name" then holds positional parameters
    /// (a user, a password) and the "type" is a value (a password, a
    /// token). A name can never hold a comma — member lists are
    /// comma-separated — so such a line is an `E0017` that quotes nothing
    /// of it (M3-D7).
    #[test]
    fn a_missing_equals_after_the_name_quotes_nothing_of_the_line() {
        let l = load_text(
            "[Proxy]\nA = direct\nT trojan, t.test, 443, password=s3cretPw42\n\
S snell, s.test, 443, psk=s3cretPw42, version=4\nX http, h.test, 80, alice, s3cretPw42, tfo=true\n\
[Proxy Group]\nG select, policy-path https://sub.test/nodes?token=t0k3n\n[Rule]\nFINAL,DIRECT\n",
        );
        let errors: Vec<(&str, &str)> = l
            .diagnostics
            .iter()
            .filter(|d| d.severity == crate::Severity::Error)
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            errors,
            [
                (
                    codes::E_INVALID_DEFINITION,
                    "[Proxy]: expected `Name = ...`"
                ),
                (
                    codes::E_INVALID_DEFINITION,
                    "[Proxy]: expected `Name = ...`"
                ),
                (
                    codes::E_INVALID_DEFINITION,
                    "[Proxy]: expected `Name = ...`"
                ),
                (
                    codes::E_INVALID_DEFINITION,
                    "[Proxy Group]: expected `Name = ...`"
                ),
            ]
        );
        for d in l.diagnostics.iter() {
            let shown = d.to_string();
            for secret in ["s3cretPw42", "alice", "t0k3n"] {
                assert!(!shown.contains(secret), "{shown}");
            }
        }
    }

    #[test]
    fn reference_errors() {
        let l = load_text(
            "[Proxy]\nA = direct\n[Proxy Group]\nG1 = select, G2, A\nG2 = select, G1\nG3 = select, Missing\n[Rule]\nDOMAIN,a,Nope\nFINAL,DIRECT\n",
        );
        let c = codes_of(&l);
        assert!(c.contains(&codes::E_UNKNOWN_POLICY_REF));
        assert!(c.contains(&codes::E_UNKNOWN_GROUP_MEMBER));
        assert!(c.contains(&codes::W_GROUP_CYCLE));
        assert!(l.diagnostics.has_errors());
    }

    /// The name of a policy that a rule, a group or a parameter refers to is
    /// quoted when it is a name. A member is whatever lies between two commas
    /// and has no `=`: a link whose `=` was typed as a `:`, text an input
    /// method joined with a full-width `＝` or `，`, is none.
    #[test]
    fn an_unknown_policy_is_quoted_only_when_it_is_a_name() {
        let l = load_text(
            "[Proxy]\nA = direct\nP = http, p.test, 80, underlying-proxy=https://al1ce:hunter2@proxy.test:8080\n[Proxy Group]\nG = select, A, Missing 01, policy-path:https://sub.test/link/t0k3n, B\u{ff1d}t0k3n2, C\u{ff0c}t0k3n3\n[Rule]\nDOMAIN,a.com,Nope\nDOMAIN,b.com,policy-path\u{ff1d}https://sub.test/link/t0k3n4\nDOMAIN,c.com,P\u{ff0c}t0k3n5\nFINAL,DIRECT\n",
        );
        let said = |code: &str| -> Vec<&str> {
            l.diagnostics
                .iter()
                .filter(|d| d.code == code)
                .map(|d| d.message.as_str())
                .collect()
        };
        assert_eq!(
            said(codes::E_UNKNOWN_GROUP_MEMBER),
            [
                "policy group `G` references unknown policy `Missing 01`",
                "policy group `G` references an unknown policy",
                "policy group `G` references an unknown policy",
                "policy group `G` references an unknown policy",
            ]
        );
        assert_eq!(
            said(codes::E_UNKNOWN_POLICY_REF),
            [
                "rule references unknown policy `Nope`",
                "rule references an unknown policy",
                "rule references an unknown policy",
                "policy `P`: `underlying-proxy` references unknown policy",
            ]
        );
        for d in l.diagnostics.iter() {
            assert!(!d.message.contains("t0k3n"), "{}", d.message);
            assert!(!d.message.contains("hunter2"), "{}", d.message);
        }
    }

    /// A group whose name is none, in a cycle: the warning does not quote it.
    #[test]
    fn groups_whose_names_are_none_form_a_cycle_without_being_quoted() {
        let l = load_text(
            "[Proxy]\nA = direct\n[Proxy Group]\nG1\u{ff0c}t0k3n = select, G2\u{ff0c}t0k3n\nG2\u{ff0c}t0k3n = select, G1\u{ff0c}t0k3n\nG3 = select, G4\nG4 = select, G3\n[Rule]\nFINAL,A\n",
        );
        let found: Vec<&str> = l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_GROUP_CYCLE)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(
            found,
            [
                "policy groups a link and a link form a cycle; they behave as REJECT",
                "policy groups `G4` and `G3` form a cycle; they behave as REJECT",
            ]
        );
    }

    /// A keystore item is named by what stands in front of the `=` of its
    /// line, which can be a password that slid there.
    #[test]
    fn a_keystore_item_is_named_only_when_the_name_is_one() {
        let l = load_text(
            "[Keystore]\nk\u{ff0c}t0k3n = type=p12, base64=!!!\nk 1 = type=p12, base64=!!!\n[Rule]\nFINAL,DIRECT\n",
        );
        let found: Vec<&str> = l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::E_KEYSTORE_BASE64)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(
            found,
            [
                "keystore item: `base64` is not valid Base64",
                "keystore item `k 1`: `base64` is not valid Base64",
            ]
        );
    }

    /// The summary lists a group's members; a member that is none is `***`.
    #[test]
    fn the_summary_lists_the_members_that_are_names() {
        let l = load_text(
            "[Proxy]\nA = direct\n[Proxy Group]\nG = select, A, Hong Kong 01, policy-path:https://sub.test/link/t0k3n, B\u{ff1d}t0k3n2, DIRECT\n[Rule]\nFINAL,DIRECT\n",
        );
        assert_eq!(
            l.config.summary().groups,
            ["G (select) -> [A, Hong Kong 01, ***, ***, DIRECT]"]
        );
    }

    /// Through members and through `include-other-group` alike; no longer
    /// an error (M3 design 5.6).
    #[test]
    fn a_group_cycle_is_a_warning() {
        let l = load_text(
            "[Proxy]\nA = direct\n[Proxy Group]\nG1 = select, G2, A\nG2 = select, G1\n\
G3 = select, A, include-other-group=G4\nG4 = select, A, include-other-group=G3\n[Rule]\nFINAL,G1\n",
        );
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let found: Vec<&str> = l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_GROUP_CYCLE)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(
            found,
            [
                "policy groups `G2` and `G1` form a cycle; they behave as REJECT",
                "policy groups `G4` and `G3` form a cycle; they behave as REJECT"
            ]
        );
        let names: Vec<&str> = l
            .config
            .group_specs
            .iter()
            .map(|g| g.name.as_str())
            .collect();
        assert_eq!(names, ["G1", "G2", "G3", "G4"]);
    }

    #[test]
    fn a_group_relay_that_leads_back_to_the_group_is_an_error() {
        let l = load_text(
            "[Proxy]\nA = http, a.test, 80\n[Proxy Group]\nG = select, A, underlying-proxy=Pick\n\
Pick = select, G, DIRECT\n[Rule]\nFINAL,G\n",
        );
        let errors: Vec<String> = l
            .diagnostics
            .iter()
            .filter(|d| d.severity == crate::Severity::Error)
            .map(|d| format!("{} {}", d.code, d.message))
            .collect();
        assert_eq!(
            errors,
            [
                "E0019 policy group `G`: `underlying-proxy` leads back to the group itself (via `Pick`)"
            ]
        );
    }

    /// `include-all-proxies` takes in every un-aliased `[Proxy]` policy the
    /// group's filter admits (M3 design 5.3 step 3); those are edges too, or
    /// a chain through such a group would recurse without the plan-mandated
    /// runtime depth guard (carried item C3, plan P13).
    #[test]
    fn include_all_proxies_is_part_of_the_e0019_graph_filtered_by_the_regex() {
        let l = load_text(
            "[Proxy]\nExit = http, exit.test, 8080, underlying-proxy=Hop\n\
[Proxy Group]\nHop = select, include-all-proxies=true\n[Rule]\nFINAL,DIRECT\n",
        );
        let errors: Vec<String> = l
            .diagnostics
            .iter()
            .filter(|d| d.severity == crate::Severity::Error)
            .map(|d| format!("{} {}", d.code, d.message))
            .collect();
        assert_eq!(
            errors,
            ["E0019 policy `Exit`: `underlying-proxy` leads back to the policy itself (via `Hop`)"]
        );

        // a filter that does not admit `Exit` keeps the profile sound
        let l = load_text(
            "[Proxy]\nExit = http, exit.test, 8080, underlying-proxy=Hop\nHK1 = http, hk1.test, 80\n\
[Proxy Group]\nHop = select, include-all-proxies=true, policy-regex-filter=^HK\n[Rule]\nFINAL,DIRECT\n",
        );
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
    }

    /// P13: a policy's relay is a group, and the group's relay is that same
    /// policy — the graph must catch it from both ends.
    #[test]
    fn a_policy_and_its_group_relay_can_cycle_between_them() {
        let l = load_text(
            "[Proxy]\nA = direct\nP = http, p.test, 80, underlying-proxy=G\n\
[Proxy Group]\nG = select, A, underlying-proxy=P\n[Rule]\nFINAL,DIRECT\n",
        );
        let errors: Vec<String> = l
            .diagnostics
            .iter()
            .filter(|d| d.severity == crate::Severity::Error)
            .map(|d| format!("{} {}", d.code, d.message))
            .collect();
        assert_eq!(
            errors,
            [
                "E0019 policy `P`: `underlying-proxy` leads back to the policy itself (via `G`)",
                "E0019 policy group `G`: `underlying-proxy` leads back to the group itself (via `P`)"
            ]
        );
    }

    /// A `subnet` group ignores `include-other-group` (W0028): it must not
    /// become an edge either, or a `select` group that takes the subnet
    /// group as a member sees a cycle that is not really there.
    #[test]
    fn a_subnets_include_other_group_is_ignored_and_is_not_an_edge() {
        let l = load_text(
            "[Proxy]\nA = direct\n[Proxy Group]\nS = subnet, default=DIRECT, include-other-group=G\n\
G = select, A, S\n[Rule]\nFINAL,DIRECT\n",
        );
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let found = codes_of(&l);
        assert!(found.contains(&codes::W_PARAM_NOT_APPLICABLE), "{found:?}");
        assert!(!found.contains(&codes::W_GROUP_CYCLE), "{found:?}");
    }

    #[test]
    fn a_relative_policy_path_is_resolved_against_the_main_profile() {
        let l =
            load_text("[Proxy Group]\nG = select, policy-path=sub/nodes.txt\n[Rule]\nFINAL,G\n");
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        assert_eq!(
            l.config.group_specs[0].import.policy_path,
            Some(crate::spec::PolicyPath::File(
                Path::new("/profiles").join("sub/nodes.txt")
            ))
        );
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

    /// A system tag holds a line to its system wherever it is written. At the
    /// start of a line it has to get past the section parser first, which takes
    /// every other `#!` line for a directive of its own: dropped in a section,
    /// kept in the header before the first one.
    #[test]
    fn a_system_tag_gates_a_line_as_loaded() {
        let load_on = |system: &str, text: &str| {
            let mut opts = LoadOptions::for_tests();
            opts.environment.system = system.to_string();
            from_text(text, Path::new("/profiles/t.conf"), &opts)
        };
        for (tag, own) in [
            ("IOS-ONLY", "iOS"),
            ("LINUX-ONLY", "Linux"),
            ("MACOS-ONLY", "macOS"),
            ("TVOS-ONLY", "tvOS"),
            ("WINDOWS-ONLY", "Windows"),
        ] {
            for line in [
                format!("#!{tag} DOMAIN,a.example,DIRECT"),
                format!("DOMAIN,a.example,DIRECT #!{tag}"),
                format!("DOMAIN,a.example,DIRECT //!{tag}"),
            ] {
                for system in ["iOS", "Linux", "macOS", "tvOS", "Windows"] {
                    let l = load_on(system, &format!("[Rule]\n{line}\nFINAL,DIRECT\n"));
                    let codes = codes_of(&l);
                    assert!(
                        !l.diagnostics.has_errors(),
                        "{line:?} on {system}: {codes:?}"
                    );
                    assert!(
                        !codes.contains(&codes::W_UNKNOWN_DIRECTIVE),
                        "{line:?} on {system}: {codes:?}"
                    );
                    let active = system == own;
                    assert_eq!(
                        l.config.rules.len(),
                        if active { 2 } else { 1 },
                        "{line:?} on {system}"
                    );
                    assert_eq!(
                        codes.contains(&codes::I_LINE_DISABLED),
                        !active,
                        "{line:?} on {system}: {codes:?}"
                    );
                }
            }
            // before the first section header the line is in no section, as
            // it is for every tag, and is not a directive of the header
            let l = load_on(
                own,
                &format!("#!{tag} DOMAIN,a.example,DIRECT\n[Rule]\nFINAL,DIRECT\n"),
            );
            let codes = codes_of(&l);
            assert!(
                codes.contains(&codes::W_LINE_OUTSIDE_SECTION)
                    && !codes.contains(&codes::W_UNKNOWN_DIRECTIVE),
                "{tag}: {codes:?}"
            );
            assert_eq!(l.config.rules.len(), 1, "{tag}");
        }
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

    /// A server written in Unicode is looked up by its A-labels: that is
    /// the name `[Host]` must not apply to. The spelling it was written in
    /// stays in the list as well.
    #[test]
    fn proxy_hostnames_hold_the_a_labels_of_a_server_written_in_unicode() {
        let l = load_text("[Proxy]\nA = http, bücher.example, 8080\n[Rule]\nFINAL,DIRECT\n");
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let mut names: Vec<String> = l.config.proxy_hostnames().into_iter().collect();
        names.sort();
        assert_eq!(names, ["bücher.example", "xn--bcher-kva.example"]);
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

    /// A `vmess` line without `vmess-aead=true` is built like one with it:
    /// `W0029` once per load, at the first such line, and no `W0007` (phase
    /// 2 M8 design 4.2).
    #[test]
    fn vmess_lines_without_vmess_aead_are_remarked_on_once_per_load() {
        let id = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
        let text = format!(
            "[Proxy]\nNew = vmess, c.test, 443, username={id}, vmess-aead=true\n\
Old1 = vmess, a.test, 443, username={id}\nOld2 = vmess, b.test, 443, username={id}\n\
Off = vmess, d.test, 443, username={id}, vmess-aead=false\n[Rule]\nFINAL,DIRECT\n"
        );
        let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        let found: Vec<(&str, &str, Option<u32>)> = loaded
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str(), d.span.as_ref().map(|s| s.line)))
            .collect();
        assert_eq!(
            found,
            [(
                codes::W_PARAM_NOT_EFFECTIVE,
                "`vmess` without `vmess-aead=true` uses the AEAD handshake all the same: rurge has no other, and every server since V2Ray 4.28.1 accepts it",
                Some(3)
            )]
        );
        for name in ["New", "Old1", "Old2", "Off"] {
            assert!(loaded.config.spec(name).is_some(), "{name}");
            assert_eq!(loaded.config.not_implemented.get(name), None, "{name}");
        }
    }

    /// A policy type rurge does not implement: `W0007` once per type, in
    /// words that do not promise it for later; `tailscale` is told the way
    /// that works today (phase 2 M8 design 4.4).
    #[test]
    fn a_policy_type_that_is_not_supported_says_so_and_tailscale_how_else() {
        let mut opts = LoadOptions::for_tests();
        opts.capabilities.policy_kinds = HashSet::from([PolicyKind::Direct, PolicyKind::Http]);
        let loaded = from_text(
            "[Proxy]\nH = http, h.test, 80\nT1 = tailscale, section-name=ts\nT2 = tailscale, section-name=ts\n\
S = ssh, s.test, 22, username=u, password=p\n[Rule]\nFINAL,DIRECT\n",
            Path::new("t.conf"),
            &opts,
        );
        let found: Vec<(&str, Option<u32>)> = loaded
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_PROTOCOL_NOT_IMPLEMENTED)
            .map(|d| (d.message.as_str(), d.span.as_ref().map(|s| s.line)))
            .collect();
        assert_eq!(
            found,
            [
                (
                    "policy type `tailscale` is not supported; a tailnet is reached through tailscaled's SOCKS5 proxy with a `socks5` or an `external` policy; such policies behave as REJECT",
                    Some(3)
                ),
                (
                    "policy type `ssh` is not supported; such policies behave as REJECT",
                    Some(5)
                ),
            ]
        );
    }

    /// An `ss` stream cipher is built like any other method: `W0031` once
    /// per load and cipher, at the first line that uses it, `rc4` with a
    /// word more. A cipher Surge no longer lists is `W0007`, once too, and
    /// its policies have no spec (phase 2 M8 design 4.1).
    #[test]
    fn ss_stream_ciphers_are_remarked_on_once_per_load_and_cipher() {
        let loaded = load_text(
            "[Proxy]\nA = ss, a.test, 8388, encrypt-method=rc4-md5, password=pw\n\
B = ss, b.test, 8388, encrypt-method=aes-256-cfb, password=pw\n\
C = ss, c.test, 8388, encrypt-method=RC4-MD5, password=pw\n\
D = ss, d.test, 8388, encrypt-method=aes-256-gcm, password=pw\n\
E = ss, e.test, 8388, encrypt-method=bf-cfb, password=pw\n\
F = ss, f.test, 8388, encrypt-method=bf-cfb, password=pw\n\
G = ss, g.test, 8388, encrypt-method=rc4, password=pw\n[Rule]\nFINAL,DIRECT\n",
        );
        let found: Vec<(&str, &str, Option<u32>)> = loaded
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str(), d.span.as_ref().map(|s| s.line)))
            .collect();
        assert_eq!(
            found,
            [
                (
                    codes::W_STREAM_CIPHER,
                    "`ss` cipher `rc4-md5` is a stream cipher: it gives no integrity protection and is deprecated; use an AEAD cipher",
                    Some(2)
                ),
                (
                    codes::W_STREAM_CIPHER,
                    "`ss` cipher `aes-256-cfb` is a stream cipher: it gives no integrity protection and is deprecated; use an AEAD cipher",
                    Some(3)
                ),
                (
                    codes::W_PROTOCOL_NOT_IMPLEMENTED,
                    "`ss` cipher `bf-cfb` is not supported (Surge no longer lists it); such policies behave as REJECT",
                    Some(6)
                ),
                (
                    codes::W_STREAM_CIPHER,
                    "`ss` cipher `rc4` is a stream cipher: it gives no integrity protection and is deprecated; use an AEAD cipher; `rc4` also repeats its keystream on every connection",
                    Some(8)
                ),
            ]
        );
        for name in ["A", "B", "C", "D", "G"] {
            assert!(loaded.config.spec(name).is_some(), "{name}");
            assert_eq!(loaded.config.not_implemented.get(name), None, "{name}");
        }
        for name in ["E", "F"] {
            assert!(loaded.config.spec(name).is_none(), "{name}");
            assert_eq!(
                loaded.config.not_implemented.get(name),
                Some(&NotImplemented::SsCipher("bf-cfb")),
                "{name}"
            );
        }
    }

    /// `snell` versions 1 to 5 are built, a line without `version` as
    /// version 1; version 6 is `W0007`, once per load, at the first line
    /// that asks for it, and its policies have no spec (phase 2 M8 design
    /// 4.3).
    #[test]
    fn snell_version_6_is_reported_once_per_load_and_the_others_are_built() {
        let loaded = load_text(
            "[Proxy]\nA = snell, a.test, 443, psk=pw\nB = snell, b.test, 443, psk=pw, version=6\n\
C = snell, c.test, 443, psk=pw, version=1\nD = snell, d.test, 443, psk=pw, version=5\n\
E = snell, e.test, 443, psk=pw, version=3\nF = snell, f.test, 443, psk=pw, version=6, mode=unshaped\n\
G = snell, g.test, 443, psk=pw, version=2\n[Rule]\nFINAL,DIRECT\n",
        );
        let found: Vec<(&str, &str, Option<u32>)> = loaded
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str(), d.span.as_ref().map(|s| s.line)))
            .collect();
        assert_eq!(
            found,
            [(
                codes::W_PROTOCOL_NOT_IMPLEMENTED,
                "`snell` version 6 is not supported: it is a beta protocol without a public specification; such policies behave as REJECT",
                Some(3)
            )]
        );
        for name in ["A", "C", "D", "E", "G"] {
            assert!(loaded.config.spec(name).is_some(), "{name}");
            assert_eq!(loaded.config.not_implemented.get(name), None, "{name}");
        }
        for name in ["B", "F"] {
            assert!(loaded.config.spec(name).is_none(), "{name}");
            assert_eq!(
                loaded.config.not_implemented.get(name),
                Some(&NotImplemented::SnellVersion(6)),
                "{name}"
            );
        }
    }

    /// Two `external` policies on one `local-port`: the second is an error
    /// at its own line (M4 design 4.4).
    #[test]
    fn two_external_policies_cannot_share_a_local_port() {
        let loaded = load_text(
            "[Proxy]\nA = external, exec=/bin/a, local-port=1080\nB = external, exec=/bin/b, local-port=1081\n\
C = external, exec=/bin/c, local-port=1080\n[Rule]\nFINAL,DIRECT\n",
        );
        let errors: Vec<&Diagnostic> = loaded
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::E_INVALID_POLICY_PARAM)
            .collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(
            errors[0].message,
            "policy `C`: `local-port` 1080 is also the `local-port` of policy `A`"
        );
        assert_eq!(errors[0].span.as_ref().map(|s| s.line), Some(4));
        assert!(loaded.config.spec("A").is_some() && loaded.config.spec("B").is_some());
        assert!(loaded.config.spec("C").is_none());
    }

    /// An `external` program on the port of rurge's own listener would make
    /// rurge dial itself: an error at the policy's line, unless the
    /// listener is on another address than loopback or any.
    #[test]
    fn an_external_policy_cannot_use_the_port_of_rurges_own_listener() {
        let loaded = load_text(
            "[General]\nhttp-listen = 0.0.0.0:6152\nsocks5-listen = 127.0.0.1:6153, 192.0.2.1:7000\n\
[Proxy]\nA = external, exec=/bin/a, local-port=6153\nB = external, exec=/bin/b, local-port=6152\n\
C = external, exec=/bin/c, local-port=7000\n[Rule]\nFINAL,DIRECT\n",
        );
        let errors: Vec<(String, Option<u32>)> = loaded
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::E_INVALID_POLICY_PARAM)
            .map(|d| (d.message.clone(), d.span.as_ref().map(|s| s.line)))
            .collect();
        assert_eq!(
            errors,
            [
                (
                    "policy `A`: `local-port` 6153 is the port of rurge's own `socks5-listen`"
                        .to_string(),
                    Some(5)
                ),
                (
                    "policy `B`: `local-port` 6152 is the port of rurge's own `http-listen`"
                        .to_string(),
                    Some(6)
                ),
            ]
        );
        assert!(loaded.config.spec("A").is_none() && loaded.config.spec("B").is_none());
        assert!(loaded.config.spec("C").is_some());
    }

    const WG_PRIVATE: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
    const WG_PUBLIC: &str = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";

    fn wireguard_section(name: &str, endpoint: &str) -> String {
        format!(
            "[WireGuard {name}]
private-key = {WG_PRIVATE}
self-ip = 10.0.0.2
peer = (public-key = {WG_PUBLIC}, allowed-ips = 10.0.0.0/24, endpoint = {endpoint})
"
        )
    }

    /// A value that is an error is the one thing said of its parameter, and
    /// of a parameter that depends on it: not also that `ecn` does not apply
    /// to the protocol — or to a `trust-tunnel` whose `h3` is the error —,
    /// that it has no effect yet on `wireguard`, or that `udp-port` wants a
    /// `udp-relay=true`.
    #[test]
    fn a_parameter_that_is_an_error_already_gets_no_second_sentence() {
        let loaded = load_text(&format!(
            "[Proxy]\nT = trojan, t.test, 443, password=pw, ecn=maybe\n\
S = ss, s.test, 8388, encrypt-method=aes-128-gcm, password=pw, udp-relay=maybe, udp-port=8389\n\
W = wireguard, section-name=home, ecn=maybe\n\
H = trust-tunnel, h.test, 443, username=u, password=pw, h3=maybe, ecn=on\n{}[Rule]\nFINAL,DIRECT\n",
            wireguard_section("home", "192.0.2.1:51820")
        ));
        let found: Vec<(&str, &str, Option<u32>)> = loaded
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str(), d.span.as_ref().map(|s| s.line)))
            .collect();
        assert_eq!(
            found,
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: invalid value `maybe` for `ecn` (expected auto / on / off / true / false)",
                    Some(2)
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `S`: invalid value `maybe` for `udp-relay` (expected true or false)",
                    Some(3)
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `W`: invalid value `maybe` for `ecn` (expected auto / on / off / true / false)",
                    Some(4)
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `H`: invalid value `maybe` for `h3` (expected true or false)",
                    Some(5)
                ),
            ]
        );
    }

    const UUID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";

    /// A policy line `P` loaded alone, with a policy `Q` to chain to and a
    /// `[WireGuard home]` section to name: the code and message of every
    /// diagnostic.
    fn said_of(line: &str) -> Vec<(&'static str, String)> {
        let loaded = load_text(&format!(
            "[Proxy]\nQ = http, q.test, 80\n{line}\n{}[Rule]\nFINAL,DIRECT\n",
            wireguard_section("home", "192.0.2.1:51820")
        ));
        loaded
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect()
    }

    /// Fails with every case whose diagnostics are not the expected ones.
    fn assert_said(cases: Vec<(String, Vec<(&'static str, String)>)>) {
        let wrong: Vec<String> = cases
            .into_iter()
            .filter_map(|(line, expected)| {
                let found = said_of(&line);
                (found != expected)
                    .then(|| format!("{line}\n  expected {expected:#?}\n  found    {found:#?}"))
            })
            .collect();
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    /// The error `E0018` of a value, as a policy `P` has it.
    fn bad_value(key: &str, value: &str, expected: &str) -> (&'static str, String) {
        (
            codes::E_INVALID_POLICY_PARAM,
            format!("policy `P`: invalid value `{value}` for `{key}` (expected {expected})"),
        )
    }

    /// A value that is an error is the one thing said of its parameter, and
    /// of the parameters that depend on it — not also that it does not apply
    /// to the protocol or has no effect: wherever there is such a sentence,
    /// not only where the test above looks.
    #[test]
    fn an_error_in_a_parameter_leaves_nothing_else_to_say_of_it() {
        let flag = "true or false";
        let tristate = "auto / on / off / true / false";
        let tos = "0-255 or 0x00-0xff";
        let versions = "dual / v4-only / v6-only / prefer-v4 / prefer-v6";
        let on = |line: &str, said: Vec<(&'static str, String)>| (line.to_string(), said);
        assert_said(vec![
            // a proxy-only parameter on `direct`
            on(
                "P = direct, ecn=maybe",
                vec![bad_value("ecn", "maybe", tristate)],
            ),
            on(
                "P = direct, no-error-alert=maybe",
                vec![bad_value("no-error-alert", "maybe", flag)],
            ),
            // `tfo` on a protocol over QUIC, and on one with no TCP connection
            on(
                "P = hysteria2, h.test, 443, password=pw, tfo=maybe",
                vec![bad_value("tfo", "maybe", flag)],
            ),
            on(
                &format!("P = tuic-v5, t.test, 443, uuid={UUID}, password=pw, tfo=maybe"),
                vec![bad_value("tfo", "maybe", flag)],
            ),
            on(
                "P = masque, m.test, 443, tfo=maybe",
                vec![bad_value("tfo", "maybe", flag)],
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, h3=true, tfo=maybe",
                vec![bad_value("tfo", "maybe", flag)],
            ),
            on(
                "P = external, exec=/bin/x, local-port=1080, tfo=maybe",
                vec![bad_value("tfo", "maybe", flag)],
            ),
            // the other common parameters `external` and `wireguard` have no use for
            on(
                "P = external, exec=/bin/x, local-port=1080, interface=, allow-other-interface=maybe, tos=300, ip-version=v5",
                vec![
                    bad_value("interface", "", "a network interface name"),
                    bad_value("allow-other-interface", "maybe", flag),
                    bad_value("ip-version", "v5", versions),
                    bad_value("tos", "300", tos),
                ],
            ),
            on(
                "P = wireguard, section-name=home, interface=, allow-other-interface=maybe, tos=300",
                vec![
                    bad_value("interface", "", "a network interface name"),
                    bad_value("allow-other-interface", "maybe", flag),
                    bad_value("tos", "300", tos),
                ],
            ),
            // what `tls`, `ws` and `h3` decide
            on(
                &format!(
                    "P = vmess, v.test, 443, username={UUID}, tls=maybe, sni=edge.test, alpn=h2"
                ),
                vec![bad_value("tls", "maybe", flag)],
            ),
            on(
                "P = trojan, t.test, 443, password=pw, ws=maybe, ws-path=/w, ws-headers=X: y",
                vec![bad_value("ws", "maybe", flag)],
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, h3=maybe, alpn=h3",
                vec![bad_value("h3", "maybe", flag)],
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, h3=maybe, tfo=true",
                vec![bad_value("h3", "maybe", flag)],
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, h3=maybe, headers=Connection:close",
                vec![bad_value("h3", "maybe", flag)],
            ),
            // `interface` is what `dns-follow-interface` follows
            on(
                "P = http, h.test, 80, interface=, dns-follow-interface=true",
                vec![bad_value("interface", "", "a network interface name")],
            ),
            // a chain that is an error: whether it takes the socket
            // parameters from the policy is open
            on(
                "P = http, p.test, 80, underlying-proxy=Missing, interface=en0, allow-other-interface=true, tos=0x10, ip-version=v4-only",
                vec![(
                    codes::E_UNKNOWN_POLICY_REF,
                    "policy `P`: `underlying-proxy` references unknown policy `Missing`"
                        .to_string(),
                )],
            ),
            on(
                "P = http, p.test, 80, underlying-proxy=REJECT, interface=en0",
                vec![bad_value(
                    "underlying-proxy",
                    "REJECT",
                    "a proxy policy or a policy group",
                )],
            ),
            // nor is it said of a QUIC policy's `port-hopping` that it cannot
            // be combined with a chain that is an error
            on(
                "P = hysteria2, h.test, 443, password=pw, port-hopping=1000-2000, underlying-proxy=Missing",
                vec![(
                    codes::E_UNKNOWN_POLICY_REF,
                    "policy `P`: `underlying-proxy` references unknown policy `Missing`"
                        .to_string(),
                )],
            ),
            on(
                "P = hysteria2, h.test, 443, password=pw, port-hopping=1000-2000, underlying-proxy=REJECT",
                vec![bad_value(
                    "underlying-proxy",
                    "REJECT",
                    "a proxy policy or a policy group",
                )],
            ),
            // and a socket parameter that is an error itself
            on(
                "P = http, p.test, 80, underlying-proxy=Q, interface=, allow-other-interface=maybe, tos=300, ip-version=v5",
                vec![
                    bad_value("interface", "", "a network interface name"),
                    bad_value("allow-other-interface", "maybe", flag),
                    bad_value("ip-version", "v5", versions),
                    bad_value("tos", "300", tos),
                ],
            ),
            // one address of two is an error: not also that they do nothing yet
            on(
                "P = external, exec=/bin/x, local-port=1080, addresses=192.0.2.1, addresses=bogus",
                vec![bad_value("addresses", "bogus", "an IP address")],
            ),
        ]);
    }

    /// The same parameters with values that are fine: every sentence the
    /// previous test takes away is still said.
    #[test]
    fn the_same_parameters_with_good_values_still_get_their_sentence() {
        let not_applicable =
            |text: &str| vec![(codes::W_PARAM_NOT_APPLICABLE, format!("policy `P`: {text}"))];
        let on = |line: &str, said: Vec<(&'static str, String)>| (line.to_string(), said);
        assert_said(vec![
            on(
                "P = direct, ecn=on",
                not_applicable("`ecn` does not apply to `direct` policies; ignored"),
            ),
            on(
                "P = direct, no-error-alert=true",
                not_applicable("`no-error-alert` does not apply to `direct` policies; ignored"),
            ),
            on(
                "P = hysteria2, h.test, 443, password=pw, tfo=true",
                not_applicable("`tfo` does not apply to `hysteria2` policies; ignored"),
            ),
            on(
                &format!("P = tuic-v5, t.test, 443, uuid={UUID}, password=pw, tfo=true"),
                not_applicable("`tfo` does not apply to `tuic-v5` policies; ignored"),
            ),
            on(
                "P = masque, m.test, 443, tfo=true",
                not_applicable("`tfo` does not apply to `masque` policies; ignored"),
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, h3=true, tfo=true",
                not_applicable(
                    "`tfo` does not apply to `trust-tunnel` policies with `h3=true`; ignored",
                ),
            ),
            on(
                "P = external, exec=/bin/x, local-port=1080, tfo=true",
                not_applicable("`tfo` does not apply to `external` policies; ignored"),
            ),
            on(
                "P = external, exec=/bin/x, local-port=1080, interface=en0, allow-other-interface=true, tos=16, ip-version=v4-only",
                [
                    "interface",
                    "allow-other-interface",
                    "tos",
                    "ip-version",
                ]
                .iter()
                .map(|key| {
                    (
                        codes::W_PARAM_NOT_APPLICABLE,
                        format!("policy `P`: `{key}` does not apply to `external` policies; ignored"),
                    )
                })
                .collect(),
            ),
            on(
                "P = wireguard, section-name=home, interface=en0, allow-other-interface=true, tos=16",
                ["interface", "allow-other-interface", "tos"]
                    .iter()
                    .map(|key| {
                        (
                            codes::W_PARAM_NOT_APPLICABLE,
                            format!("policy `P`: `{key}` does not apply to `wireguard` policies; ignored"),
                        )
                    })
                    .collect(),
            ),
            // one sentence for `tfo`, not also that it has no effect yet
            on(
                "P = wireguard, section-name=home, tfo=true",
                not_applicable("`tfo` does not apply to `wireguard` policies; ignored"),
            ),
            on(
                &format!(
                    "P = vmess, v.test, 443, username={UUID}, vmess-aead=true, tls=false, sni=edge.test"
                ),
                not_applicable("`sni` has no effect without `tls=true`; ignored"),
            ),
            on(
                "P = trojan, t.test, 443, password=pw, ws=false, ws-path=/w, ws-headers=X: y",
                [
                    not_applicable("`ws-path` has no effect without `ws=true`; ignored"),
                    not_applicable("`ws-headers` has no effect without `ws=true`; ignored"),
                ]
                .concat(),
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, h3=false, alpn=h3",
                not_applicable(
                    "`alpn` is always `h2` on `trust-tunnel` policies without `h3=true`; ignored",
                ),
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, h3=false, tfo=true",
                vec![(
                    codes::W_PARAM_NOT_EFFECTIVE,
                    "policy parameter `tfo` is parsed but has no effect in this version"
                        .to_string(),
                )],
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, h3=false, headers=Connection:close",
                not_applicable("header `connection` in `headers` is not allowed in HTTP/2; ignored"),
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, h3=true, headers=Connection:close",
                not_applicable("header `connection` in `headers` is not allowed in HTTP/3; ignored"),
            ),
            on(
                "P = http, h.test, 80, dns-follow-interface=true",
                not_applicable("`dns-follow-interface` has no effect without `interface`; ignored"),
            ),
            on(
                "P = http, p.test, 80, underlying-proxy=Q, interface=en0, allow-other-interface=true, tos=0x10, ip-version=v4-only",
                ["interface", "allow-other-interface", "tos", "ip-version"]
                    .iter()
                    .map(|key| {
                        (
                            codes::W_PARAM_NOT_APPLICABLE,
                            format!("policy `P`: `{key}` has no effect on a policy with `underlying-proxy`; ignored"),
                        )
                    })
                    .collect(),
            ),
            on(
                "P = external, exec=/bin/x, local-port=1080, addresses=192.0.2.1, addresses=192.0.2.2",
                vec![(
                    codes::W_PARAM_NOT_EFFECTIVE,
                    "policy parameter `addresses` is parsed but has no effect in this version"
                        .to_string(),
                )],
            ),
        ]);
    }

    /// The silence is about the parameter that is an error, not about the
    /// line: a parameter whose value is fine still gets its sentence, before
    /// the error or after it. What a faulty `port-hopping` says of a chain is
    /// left as it is — every valid value would conflict with it —, and a
    /// faulty `port-hopping` leaves the interval's own warning alone.
    #[test]
    fn an_error_in_one_parameter_leaves_the_sentences_of_the_others() {
        let not_applicable =
            |text: &str| (codes::W_PARAM_NOT_APPLICABLE, format!("policy `P`: {text}"));
        let tos = "0-255 or 0x00-0xff";
        let on = |line: &str, said: Vec<(&'static str, String)>| (line.to_string(), said);
        assert_said(vec![
            on(
                "P = direct, ecn=maybe, no-error-alert=true",
                vec![
                    bad_value("ecn", "maybe", "auto / on / off / true / false"),
                    not_applicable("`no-error-alert` does not apply to `direct` policies; ignored"),
                ],
            ),
            on(
                "P = external, exec=/bin/x, local-port=1080, interface=, tos=16",
                vec![
                    bad_value("interface", "", "a network interface name"),
                    not_applicable("`tos` does not apply to `external` policies; ignored"),
                ],
            ),
            on(
                "P = wireguard, section-name=home, interface=, tos=16",
                vec![
                    bad_value("interface", "", "a network interface name"),
                    not_applicable("`tos` does not apply to `wireguard` policies; ignored"),
                ],
            ),
            on(
                "P = http, p.test, 80, underlying-proxy=Q, interface=, tos=0x10",
                vec![
                    bad_value("interface", "", "a network interface name"),
                    not_applicable(
                        "`tos` has no effect on a policy with `underlying-proxy`; ignored",
                    ),
                ],
            ),
            // a chain that is fine, an error in some other parameter
            on(
                "P = http, p.test, 80, underlying-proxy=Q, interface=en0, block-quic=maybe",
                vec![
                    bad_value("block-quic", "maybe", "auto / on / off / true / false"),
                    not_applicable(
                        "`interface` has no effect on a policy with `underlying-proxy`; ignored",
                    ),
                ],
            ),
            on(
                "P = http, h.test, 80, dns-follow-interface=true, tos=300",
                vec![
                    not_applicable(
                        "`dns-follow-interface` has no effect without `interface`; ignored",
                    ),
                    bad_value("tos", "300", tos),
                ],
            ),
            on(
                "P = hysteria2, h.test, 443, password=pw, tfo=true, tos=300",
                vec![
                    bad_value("tos", "300", tos),
                    not_applicable("`tfo` does not apply to `hysteria2` policies; ignored"),
                ],
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, h3=true, tfo=true, tos=300",
                vec![
                    bad_value("tos", "300", tos),
                    not_applicable(
                        "`tfo` does not apply to `trust-tunnel` policies with `h3=true`; ignored",
                    ),
                ],
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, alpn=h3, tos=300",
                vec![
                    bad_value("tos", "300", tos),
                    not_applicable(
                        "`alpn` is always `h2` on `trust-tunnel` policies without `h3=true`; ignored",
                    ),
                ],
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, headers=Connection:close, tos=300",
                vec![
                    bad_value("tos", "300", tos),
                    not_applicable("header `connection` in `headers` is not allowed in HTTP/2; ignored"),
                ],
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, ecn=on, tos=300",
                vec![
                    bad_value("tos", "300", tos),
                    not_applicable(
                        "`ecn` does not apply to `trust-tunnel` policies without `h3=true`; ignored",
                    ),
                ],
            ),
            on(
                "P = trust-tunnel, u.test, 443, username=u, password=pw, h3=false, tfo=true, tos=300",
                vec![
                    bad_value("tos", "300", tos),
                    (
                        codes::W_PARAM_NOT_EFFECTIVE,
                        "policy parameter `tfo` is parsed but has no effect in this version"
                            .to_string(),
                    ),
                ],
            ),
            on(
                &format!(
                    "P = vmess, v.test, 443, username={UUID}, vmess-aead=true, sni=edge.test, tos=300"
                ),
                vec![
                    bad_value("tos", "300", tos),
                    not_applicable("`sni` has no effect without `tls=true`; ignored"),
                ],
            ),
            on(
                "P = trojan, t.test, 443, password=pw, ws-path=/w, tos=300",
                vec![
                    bad_value("tos", "300", tos),
                    not_applicable("`ws-path` has no effect without `ws=true`; ignored"),
                ],
            ),
            on(
                "P = ss, s.test, 8388, encrypt-method=aes-128-gcm, password=pw, udp-port=8389, tos=300",
                vec![
                    bad_value("tos", "300", tos),
                    not_applicable("`udp-port` only applies with `udp-relay=true`; ignored"),
                ],
            ),
            on(
                "P = external, exec=/bin/x, local-port=0, addresses=192.0.2.1",
                vec![
                    bad_value("local-port", "0", "a port, 1-65535"),
                    (
                        codes::W_PARAM_NOT_EFFECTIVE,
                        "policy parameter `addresses` is parsed but has no effect in this version"
                            .to_string(),
                    ),
                ],
            ),
            // a valid chain is something every `port-hopping` conflicts with
            on(
                "P = hysteria2, h.test, 443, password=pw, port-hopping=1000-2000, underlying-proxy=Q",
                vec![(
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `port-hopping` cannot be combined with `underlying-proxy`"
                        .to_string(),
                )],
            ),
            on(
                "P = hysteria2, h.test, 443, password=pw, port-hopping=bogus, underlying-proxy=Q",
                vec![
                    bad_value(
                        "port-hopping",
                        "bogus",
                        "ports from 1 to 65535 and ranges such as `5000-6000`, separated by `;`",
                    ),
                    (
                        codes::E_INVALID_POLICY_PARAM,
                        "policy `P`: `port-hopping` cannot be combined with `underlying-proxy`"
                            .to_string(),
                    ),
                ],
            ),
            on(
                "P = hysteria2, h.test, 443, password=pw, port-hopping=bogus, port-hopping-interval=2",
                vec![
                    bad_value(
                        "port-hopping",
                        "bogus",
                        "ports from 1 to 65535 and ranges such as `5000-6000`, separated by `;`",
                    ),
                    (
                        codes::W_INVALID_VALUE,
                        "policy `P`: `port-hopping-interval` is below the minimum of 5 seconds; using 5"
                            .to_string(),
                    ),
                ],
            ),
        ]);
    }

    /// A `[WireGuard <name>]` section is typed, no longer a deferred one
    /// (phase 2 M4 design 4.1).
    #[test]
    fn wireguard_sections_are_typed() {
        let l = load_text(&format!(
            "{}{}[Rule]
FINAL,DIRECT
",
            wireguard_section("home", "vpn.example.com:51820"),
            wireguard_section("office", "192.0.2.1:51820")
        ));
        assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics.into_vec());
        let names: Vec<&str> = l.config.wireguard.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["home", "office"]);
        assert!(l.config.deferred.sections.is_empty());
    }

    /// Checked whether a policy uses it or not; the first of two sections
    /// of one name is used.
    #[test]
    fn every_wireguard_section_is_checked() {
        let l = load_text(&format!(
            "[WireGuard spare]
self-ip = 10.0.0.2
{}{}[Rule]
FINAL,DIRECT
",
            wireguard_section("home", "a.test:51820"),
            wireguard_section("home", "b.test:51820")
        ));
        let found: Vec<(&str, &str)> = l
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    codes::E_WIREGUARD_SECTION,
                    "[WireGuard spare]: `private-key` is required"
                ),
                (
                    codes::E_WIREGUARD_SECTION,
                    "[WireGuard spare]: at least one `peer` is required"
                ),
                (
                    codes::W_DUPLICATE_RULESET,
                    "duplicate [WireGuard home] ignored; the first definition is used"
                ),
            ]
        );
        assert_eq!(l.config.wireguard.len(), 1);
        assert_eq!(
            l.config.wireguard[0].peers[0].endpoint.to_string(),
            "a.test:51820"
        );
    }

    /// A peer's endpoint is a proxy server like any other: `[Host]` does not
    /// apply to its name (matrix 6.3).
    #[test]
    fn wireguard_endpoints_are_proxy_hostnames() {
        let l = load_text(&format!(
            "[Proxy]
A = http, proxy.example.com, 8080
{}{}[Rule]
FINAL,DIRECT
",
            wireguard_section("home", "VPN.example.com:51820"),
            wireguard_section("office", "192.0.2.1:51820")
        ));
        let mut names: Vec<String> = l.config.proxy_hostnames().into_iter().collect();
        names.sort();
        assert_eq!(names, ["proxy.example.com", "vpn.example.com"]);
    }

    const SETS: &str = "https://sets.example/u.list";

    /// What `E0024` said: `(line, message)`, in the order of the report.
    fn kind_conflicts(l: &Loaded) -> Vec<(u32, String)> {
        l.diagnostics
            .iter()
            .filter(|d| d.code == codes::E_SET_KIND_CONFLICT)
            .map(|d| {
                assert_eq!(d.severity, crate::Severity::Error);
                (d.span.as_ref().expect("a span").line, d.message.clone())
            })
            .collect()
    }

    /// The text of the report that stands on one use and names the other:
    /// its kind, and where it is.
    fn conflict(other: &str, line: u32) -> String {
        format!(
            "the same resource cannot be both a RULE-SET and a DOMAIN-SET \
(it is used as a {other} at /profiles/t.conf:{line})"
        )
    }

    /// The second use is the one at fault, whichever kind came first, and
    /// the text never quotes the reference.
    #[test]
    fn one_resource_as_a_rule_set_and_a_domain_set_is_an_error_at_the_second_use() {
        let l = load_text(&format!(
            "[Rule]\nRULE-SET,{SETS},DIRECT\nDOMAIN-SET,{SETS},REJECT\nFINAL,DIRECT\n"
        ));
        assert!(l.diagnostics.has_errors());
        assert_eq!(kind_conflicts(&l), [(3, conflict("RULE-SET", 2))]);
        let l = load_text(&format!(
            "[Rule]\nDOMAIN-SET,{SETS},REJECT\nRULE-SET,{SETS},DIRECT\nFINAL,DIRECT\n"
        ));
        assert_eq!(kind_conflicts(&l), [(3, conflict("DOMAIN-SET", 2))]);
        // a path is a resource as well
        let l =
            load_text("[Rule]\nRULE-SET,a.list,DIRECT\nDOMAIN-SET,a.list,REJECT\nFINAL,DIRECT\n");
        assert_eq!(kind_conflicts(&l), [(3, conflict("RULE-SET", 2))]);
    }

    /// The uses are taken in the order the rules, the `[Host]` entries and
    /// the inline sets are kept in, which is not the order of the file: the
    /// report stands on the later of the two in that order, and what it says
    /// of the other use is true wherever in the file the two stand.
    #[test]
    fn the_report_names_the_other_use_wherever_it_stands_in_the_file() {
        let l = load_text(&format!(
            "[Host]\nDOMAIN-SET:{SETS} = 1.2.3.4\n[Rule]\nRULE-SET,{SETS},DIRECT\nFINAL,DIRECT\n"
        ));
        assert_eq!(kind_conflicts(&l), [(2, conflict("RULE-SET", 4))]);
        let l = load_text(&format!(
            "[Ruleset X]\nDOMAIN-SET,{SETS}\n[Rule]\nRULE-SET,{SETS},DIRECT\nFINAL,DIRECT\n"
        ));
        assert_eq!(kind_conflicts(&l), [(1, conflict("RULE-SET", 4))]);
        let l = load_text(&format!(
            "[Ruleset X]\nRULE-SET,{SETS}\n[Host]\nDOMAIN-SET:{SETS} = 1.2.3.4\n[Rule]\nFINAL,DIRECT\n"
        ));
        assert_eq!(kind_conflicts(&l), [(1, conflict("DOMAIN-SET", 4))]);
    }

    /// One report per resource, however many more uses follow.
    #[test]
    fn a_resource_is_reported_once() {
        let l = load_text(&format!(
            "[Rule]\nRULE-SET,{SETS},DIRECT\nDOMAIN-SET,{SETS},REJECT\n\
DOMAIN-SET,{SETS},REJECT\nRULE-SET,{SETS},DIRECT\nFINAL,DIRECT\n"
        ));
        assert_eq!(kind_conflicts(&l), [(3, conflict("RULE-SET", 2))]);
    }

    #[test]
    fn a_host_entry_is_a_use_too() {
        let l = load_text(&format!(
            "[Rule]\nRULE-SET,{SETS},DIRECT\nFINAL,DIRECT\n[Host]\nDOMAIN-SET:{SETS} = 1.2.3.4\n"
        ));
        assert_eq!(kind_conflicts(&l), [(5, conflict("RULE-SET", 2))]);
        let l = load_text(&format!(
            "[Rule]\nDOMAIN-SET,{SETS},REJECT\nFINAL,DIRECT\n[Host]\nRULE-SET:{SETS} = 1.2.3.4\n"
        ));
        assert_eq!(kind_conflicts(&l), [(5, conflict("DOMAIN-SET", 2))]);
    }

    /// The sub-rules of a logical rule stand on the line of the rule.
    #[test]
    fn the_sub_rules_of_a_logical_rule_are_uses_on_the_line_of_the_rule() {
        let l = load_text(&format!(
            "[Rule]\nAND,((RULE-SET,{SETS}),(DEST-PORT,443)),DIRECT\nDOMAIN-SET,{SETS},REJECT\nFINAL,DIRECT\n"
        ));
        assert_eq!(kind_conflicts(&l), [(3, conflict("RULE-SET", 2))]);
        let l = load_text(&format!(
            "[Rule]\nDOMAIN-SET,{SETS},REJECT\n\
OR,((AND,((DOMAIN,a.example),(NOT,((RULE-SET,{SETS}))))),(DEST-PORT,80)),DIRECT\nFINAL,DIRECT\n"
        ));
        assert_eq!(kind_conflicts(&l), [(3, conflict("DOMAIN-SET", 2))]);
    }

    /// A line of an inline set has no line of its own: the set's header is
    /// where it stands.
    #[test]
    fn the_lines_of_an_inline_set_are_uses_on_the_line_of_the_set() {
        let l = load_text(&format!(
            "[Rule]\nRULE-SET,{SETS},DIRECT\nFINAL,DIRECT\n[Ruleset X]\nDOMAIN,a.example\nDOMAIN-SET,{SETS}\n"
        ));
        assert_eq!(kind_conflicts(&l), [(4, conflict("RULE-SET", 2))]);
        let l = load_text(&format!(
            "[Rule]\nDOMAIN-SET,{SETS},REJECT\nFINAL,DIRECT\n[Ruleset X]\n\
AND,((RULE-SET,{SETS}),(DEST-PORT,443))\n"
        ));
        assert_eq!(kind_conflicts(&l), [(4, conflict("DOMAIN-SET", 2))]);
    }

    /// Two spellings of one path are one resource; a different path is not.
    #[test]
    fn two_spellings_of_a_path_are_one_resource() {
        let l =
            load_text("[Rule]\nRULE-SET,a.list,DIRECT\nDOMAIN-SET,./a.list,REJECT\nFINAL,DIRECT\n");
        assert_eq!(kind_conflicts(&l), [(3, conflict("RULE-SET", 2))]);
        let l =
            load_text("[Rule]\nRULE-SET,a.list,DIRECT\nDOMAIN-SET,b.list,REJECT\nFINAL,DIRECT\n");
        assert!(kind_conflicts(&l).is_empty());
    }

    #[test]
    fn the_same_kind_twice_is_no_conflict() {
        let l = load_text(&format!(
            "[Rule]\nRULE-SET,{SETS},DIRECT\nRULE-SET,{SETS},REJECT\n\
DOMAIN-SET,b.list,DIRECT\nDOMAIN-SET,b.list,REJECT\n\
AND,((RULE-SET,{SETS}),(DEST-PORT,443)),DIRECT\nFINAL,DIRECT\n\
[Host]\nRULE-SET:{SETS} = 1.2.3.4\nDOMAIN-SET:b.list = 1.2.3.4\n\
[Ruleset X]\nRULE-SET,{SETS}\nDOMAIN-SET,b.list\n"
        ));
        assert!(kind_conflicts(&l).is_empty(), "{:?}", codes_of(&l));
    }

    /// Only a URL and a file are one resource under two kinds: `SYSTEM`,
    /// `LAN` and the name of an inline set are not read as the other kind
    /// (`DOMAIN-SET` takes a URL or a file, and a file it is, here).
    #[test]
    fn the_built_in_and_inline_sets_are_not_checked() {
        let l = load_text(
            "[Rule]\nRULE-SET,SYSTEM,DIRECT\nDOMAIN-SET,SYSTEM,REJECT\n\
RULE-SET,LAN,DIRECT\nDOMAIN-SET,LAN,REJECT\nRULE-SET,X,DIRECT\nDOMAIN-SET,X,REJECT\nFINAL,DIRECT\n\
[Ruleset X]\nDOMAIN,a.example\n",
        );
        assert!(kind_conflicts(&l).is_empty(), "{:?}", codes_of(&l));
    }

    /// A line that a requirement turned off is not a use: it never gets this
    /// far, for the lists the check reads hold the lines that are on.
    #[test]
    fn a_line_that_is_off_is_not_a_use() {
        let l = load_text(&format!(
            "[Rule]\nRULE-SET,{SETS},DIRECT\nDOMAIN-SET,{SETS},REJECT #!IOS-ONLY\nFINAL,DIRECT\n\
[Host]\nDOMAIN-SET:{SETS} = 1.2.3.4 #!IOS-ONLY\n[Ruleset X]\nDOMAIN-SET,{SETS} #!IOS-ONLY\n"
        ));
        assert!(codes_of(&l).contains(&codes::I_LINE_DISABLED));
        assert!(kind_conflicts(&l).is_empty(), "{:?}", codes_of(&l));
        let l = load_text(&format!(
            "[Rule]\nRULE-SET,{SETS},DIRECT\n#!REQUIREMENT CORE_VERSION>=99 DOMAIN-SET,{SETS},REJECT\nFINAL,DIRECT\n"
        ));
        assert!(codes_of(&l).contains(&codes::I_LINE_DISABLED));
        assert!(kind_conflicts(&l).is_empty(), "{:?}", codes_of(&l));
    }

    /// A URL can carry a user name and a password; neither it nor the rest
    /// of the reference is in the text.
    #[test]
    fn the_report_quotes_no_part_of_the_reference() {
        let url = "https://al1ce:S3CR3T@sets.example/gh/user/repo@main/u.list?token=t0k3n";
        let l = load_text(&format!(
            "[Rule]\nRULE-SET,{url},DIRECT\nDOMAIN-SET,{url},REJECT\nFINAL,DIRECT\n"
        ));
        assert_eq!(kind_conflicts(&l), [(3, conflict("RULE-SET", 2))]);
        for d in l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::E_SET_KIND_CONFLICT)
        {
            let shown = d.to_string();
            for part in ["S3CR3T", "al1ce", "t0k3n", "sets.example", "u.list"] {
                assert!(!shown.contains(part), "{shown}");
            }
        }
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
