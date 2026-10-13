//! Semantic layer: `Config` assembled from a `Profile`, plus cross validation.

use crate::deferred::DeferredSections;
use crate::diagnostic::{Diagnostic, Diagnostics, ParseError, codes};
use crate::general::{General, parse_general};
use crate::host::{HostEntry, HostKey, parse_host_entry};
use crate::keystore::{KeystoreItem, parse_keystore_item, unknown_field};
use crate::managed::{self, ManagedConfig, parse_managed};
use crate::policy::{
    Builtin, GroupKind, PolicyGroup, PolicyKind, ProxyPolicy, is_name, named, parse_group,
    parse_policy, quoted_name, shown_members,
};
use crate::reach::Reach;
use crate::remote::{Memo, RemoteRef, RemoteRole, RemoteSource};
use crate::requirement::{self, Environment};
use crate::rule::{
    ParseCtx, PolicyRef, ResourceRef, Rule, RuleKind, SubRule, parse_rule, parse_subrule,
};
use crate::span::Span;
use crate::spec::{
    GroupSpec, NameKind, NotImplemented, PolicySpec, ProtoSpec, Remark, SpecEnv, SpecOutcome,
    to_group_spec, to_spec,
};
use crate::text::include::{self, IncludeOptions};
use crate::text::{Origin, Profile, SectionKind, parse_str};
use crate::value::{split_definition, split_list};
use crate::wireguard::{WireGuardSection, parse_section};
use base64::Engine as _;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fmt;
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

#[derive(Clone)]
pub struct LoadOptions {
    pub environment: Environment,
    pub platform: Platform,
    pub capabilities: Capabilities,
    /// Where the text of what the profile names by a URL comes from. `None`
    /// for a caller with no data directory to keep downloaded texts in: a
    /// remote `#!include` is then not read, and the local file is the profile
    /// even when it is a managed one.
    pub remote: Option<Arc<dyn RemoteSource>>,
}

/// The source of downloaded text is not shown, only whether there is one.
impl fmt::Debug for LoadOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let LoadOptions {
            environment,
            platform,
            capabilities,
            remote,
        } = self;
        f.debug_struct("LoadOptions")
            .field("environment", environment)
            .field("platform", platform)
            .field("capabilities", capabilities)
            .field("remote", &remote.as_ref().map(|_| ".."))
            .finish()
    }
}

impl LoadOptions {
    pub fn for_tests() -> Self {
        Self {
            environment: Environment::fixed(),
            platform: Platform::Linux,
            capabilities: Capabilities::all(),
            remote: None,
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
    /// The local file that was loaded.
    pub main: PathBuf,
    /// The local files the text in effect includes. The files that downloaded
    /// texts are kept in are not among them.
    pub includes: Vec<PathBuf>,
    /// The file the text of the profile was read from: `main`, or the
    /// downloaded copy of a managed profile while that is in effect.
    pub main_text: PathBuf,
    /// Every remote file the load came across, one per URL, in the order it
    /// first came to them: the addresses a managed profile leads through
    /// come first.
    pub remote: Vec<RemoteRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub general: General,
    pub policies: Vec<ProxyPolicy>,
    /// Typed parameters of every policy whose type has a spec (same order as
    /// `policies`; policies with errors are absent).
    pub specs: Vec<PolicySpec>,
    /// Why a policy without errors has no spec, when its line asks for
    /// something rurge does not implement (`W0007`) or does not start
    /// (`W0033`); by policy name.
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
    /// The `#!MANAGED-CONFIG` line of the text in effect: none when it is a
    /// downloaded copy that carries no line.
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

    /// The listener of rurge's own that an `external` policy's program would
    /// be listening on, as `general` has the listeners now: the key of that
    /// listener and the port. A load reports this for the profile's own
    /// addresses (`E0018`) and does not build the policy, so a profile that
    /// loaded without errors has none; whoever replaces the listeners
    /// afterwards asks again.
    pub fn external_on_own_listener(&self) -> Option<(&'static str, u16)> {
        self.specs.iter().find_map(|spec| match &spec.proto {
            ProtoSpec::External(external) => own_listener(&self.general, external.local_port)
                .map(|key| (key, external.local_port)),
            _ => None,
        })
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
            managed: self.managed.as_ref().map(ManagedConfig::shown),
        }
    }

    /// Index of the FINAL rule that gives the policy and the options
    /// (`dns-failed`): the last one. The manual: "If several enabled FINAL
    /// rules are present, the last one is the effective one."
    pub fn effective_final(&self) -> Option<usize> {
        self.rules
            .iter()
            .rposition(|r| matches!(r.kind, RuleKind::Final))
    }

    /// Index of the FINAL rule where matching ends: the first one. The
    /// manual: "a FINAL rule immediately terminates rule matching. Rules
    /// included or defined after it are never evaluated". It is
    /// `effective_final` when there is only one.
    pub fn first_final(&self) -> Option<usize> {
        self.rules
            .iter()
            .position(|r| matches!(r.kind, RuleKind::Final))
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

/// Loads the profile in the file `path`. When its header carries a
/// `#!MANAGED-CONFIG` line and `opts.remote` is there, the text that is in
/// effect is the file itself, or the copy that was downloaded from the address
/// the line names: `managed::choose` says which. The lines of a copy stand in
/// the file that holds it, and `SourceInfo::main_text` names it.
pub fn load(path: &Path, opts: &LoadOptions) -> Result<Loaded, LoadError> {
    let text = std::fs::read_to_string(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let Some(source) = opts.remote.as_deref() else {
        return Ok(from_text(&text, path, opts));
    };
    let (mut profile, diags) = parse_str(&text, Arc::from(path), Origin::Main);
    let Some(line) = managed::find(&profile.header, &mut Diagnostics::default()) else {
        return Ok(finish(
            profile,
            diags,
            path,
            opts,
            Some(source),
            Additions::default(),
        ));
    };
    // an address gets one answer in this load, whoever asks: the walk or an
    // include of the text that comes out of it
    let source = Memo::new(source);
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let choice = managed::choose(&line, !profile.sections.is_empty(), modified, &source);
    // A copy is the profile while it is in effect, and the local file is not:
    // what its text has to say, errors included, is not reported.
    let (profile, diags, kept_in) = match choice.copy {
        Some(copy) => {
            let mut downloaded = copy.profile;
            // the file that was loaded is the local one, whichever text is in
            // effect
            downloaded.main = profile.main.take();
            (downloaded, copy.diagnostics, Some(copy.file))
        }
        None => (profile, diags, None),
    };
    let additions = Additions {
        diagnostics: choice.diagnostics,
        remote: choice.remote,
        kept_in,
    };
    Ok(finish(profile, diags, path, opts, Some(&source), additions))
}

/// Loads the text it is given as the profile in the file `path`. There is no
/// file to be changed or to be older than a download, so a `#!MANAGED-CONFIG`
/// line in the text asks for nothing: the text is what is read.
pub fn from_text(text: &str, path: &Path, opts: &LoadOptions) -> Loaded {
    let (profile, diags) = parse_str(text, Arc::from(path), Origin::Main);
    finish(
        profile,
        diags,
        path,
        opts,
        opts.remote.as_deref(),
        Additions::default(),
    )
}

/// What choosing the text of a managed profile adds to a load.
#[derive(Default)]
struct Additions {
    /// What is said of the choice, which stands before everything else.
    diagnostics: Diagnostics,
    /// The remote files the choice came across.
    remote: Vec<RemoteRef>,
    /// The file the text in effect is kept in, when it is a downloaded copy.
    kept_in: Option<PathBuf>,
}

/// The load from the point where the text in effect is read: the includes
/// are expanded, the requirements applied, and the profile typed. `path` is
/// the local file, whose directory the relative paths start from; `source`
/// answers for the remote includes.
fn finish(
    mut profile: Profile,
    mut diags: Diagnostics,
    path: &Path,
    opts: &LoadOptions,
    source: Option<&dyn RemoteSource>,
    additions: Additions,
) -> Loaded {
    let base_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let mut remote = additions.remote;
    let included = include::expand(
        &mut profile,
        &IncludeOptions {
            base_dir: base_dir.clone(),
            max_depth: 8,
            max_files: 200,
        },
        source,
        &mut diags,
    );
    // an include of an address the choice was at keeps the role it gave it
    for found in included {
        match remote.iter_mut().find(|r| r.url == found.url) {
            Some(known) => known.lacks_section |= found.lacks_section,
            None => remote.push(found),
        }
    }
    requirement::apply(&mut profile, &opts.environment, &mut diags);
    let mut loaded = from_profile(profile, &base_dir, opts, remote);
    let mut said = additions.diagnostics;
    said.extend(diags);
    said.extend(loaded.diagnostics);
    loaded.diagnostics = said;
    if let Some(file) = additions.kept_in {
        loaded.config.source.main_text = file;
    }
    loaded
}

/// The `Name = definition` entries of a section, each with where its line
/// stands and where it came from.
fn definition_entries<'a>(
    profile: &'a Profile,
    section: &str,
    diags: &mut Diagnostics,
) -> Vec<(&'a str, &'a str, &'a Span, &'a Origin)> {
    let mut out = Vec::new();
    if let Some(sec) = profile.section(section) {
        for e in sec.active_entries() {
            match split_definition(&e.raw) {
                // A name never holds a comma (member lists are
                // comma-separated): one that does came from the parameters'
                // first `=` standing in for a missing one after the name.
                Some((name, def)) if !name.contains(',') => {
                    out.push((name, def, &e.span, &e.origin))
                }
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

/// What the parse error of a `[Rule]` or `[Host]` line becomes: an error, but
/// for a line that the text it is written in may not write (`W0033`), which
/// is a warning, said once for the whole line. The profile loads without it.
fn line_failure(err: ParseError, span: Span) -> Diagnostic {
    if err.code != codes::W_REMOTE_LOCAL_ACCESS {
        return Diagnostic::from_parse(err, span);
    }
    let ignored =
        Diagnostic::warning(err.code, format!("{}; the line is ignored", err.message)).at(span);
    match err.hint {
        Some(hint) => ignored.with_hint(hint),
        None => ignored,
    }
}

/// Recursively collect `.unknown` flags from a sub-rule and any sub-rules nested
/// inside it (`AND` / `OR` / `NOT`). Ruling R12: unrecognised flags on sub-rules
/// are surfaced as `W_UNKNOWN_RULE_PARAM`, since `SubRule` carries no span of its
/// own to attach a diagnostic to directly.
fn collect_subrule_unknowns(sub: &SubRule, out: &mut Vec<String>) {
    out.extend(sub.unknown.iter().cloned());
    collect_kind_unknowns(&sub.kind, out);
}

/// What `W0003` says of an item of a rule that the parser does not know (`of`:
/// what the rule is). The item is named by what stands before its `=`, or by
/// the whole of it when it has none, and only when that is a lower-case word
/// like the parameters rules have (`no-resolve`, `update-interval`). What
/// follows an `=` is a value, and an item written any other way can be one
/// that slid into the line: the policy of a line copied into a rule set, a
/// field of another kind of line.
fn unknown_parameter(of: &str, item: &str) -> String {
    let name = item.split('=').next().unwrap_or_default().trim();
    let a_word =
        (1..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-');
    if a_word {
        format!("unknown {of} parameter `{name}` ignored")
    } else {
        format!("unknown {of} parameter ignored")
    }
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

/// `remote` is what expanding the includes of `profile` came across.
pub fn from_profile(
    profile: Profile,
    base_dir: &Path,
    opts: &LoadOptions,
    remote: Vec<RemoteRef>,
) -> Loaded {
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
    // A section can be made of lines of several texts, and what a line may
    // name on this machine is up to the text it is written in.
    let ctx_for = |origin: &Origin| ParseCtx {
        inline_rulesets: &inline_names,
        base_dir,
        reach: origin.reach(),
    };

    // Policies and groups with name registry.
    let mut names: HashMap<String, &'static str> = HashMap::new();
    let mut policies = Vec::new();
    // whether the text each `external` policy is defined in may start a
    // program: its reach, by the policy's name
    let mut external_reach: HashMap<String, Reach> = HashMap::new();
    for (name, def, span, origin) in definition_entries(&profile, "Proxy", &mut diags) {
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
            Ok(p) => {
                if p.kind == PolicyKind::External {
                    external_reach.insert(p.name.clone(), origin.reach());
                }
                policies.push(p);
            }
            Err(e) => diags.push(Diagnostic::from_parse(e, span.clone())),
        }
    }
    let mut groups = Vec::new();
    // what the text each group is defined in may name on this machine
    let mut group_reach: HashMap<String, Reach> = HashMap::new();
    for (name, def, span, origin) in definition_entries(&profile, "Proxy Group", &mut diags) {
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
            Ok(g) => {
                group_reach.insert(g.name.clone(), origin.reach());
                groups.push(g);
            }
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
            match parse_subrule(&e.raw, &ctx_for(&e.origin)) {
                Ok(r) => {
                    // Ruling R12: unknown flags on an inline [Ruleset] line warn at that line's span.
                    let mut unknown = Vec::new();
                    collect_subrule_unknowns(&r, &mut unknown);
                    for u in &unknown {
                        diags.push(
                            Diagnostic::warning(
                                codes::W_UNKNOWN_RULE_PARAM,
                                unknown_parameter(&format!("[{}]", sec.name), u),
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
            match parse_rule(&e.raw, &ctx_for(&e.origin), &e.span) {
                Ok(r) => {
                    for u in &r.params.unknown {
                        diags.push(
                            Diagnostic::warning(
                                codes::W_UNKNOWN_RULE_PARAM,
                                unknown_parameter("rule", u),
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
                                unknown_parameter("sub-rule", u),
                            )
                            .at(e.span.clone()),
                        );
                    }
                    rules.push(r);
                }
                Err(err) => diags.push(line_failure(err, e.span.clone())),
            }
        }
    }

    // Hosts and keystore.
    let mut hosts = Vec::new();
    if let Some(sec) = profile.section("Host") {
        for e in sec.active_entries() {
            match parse_host_entry(&e.raw, &ctx_for(&e.origin), &e.span) {
                Ok(h) => hosts.push(h),
                Err(err) => diags.push(line_failure(err, e.span.clone())),
            }
        }
    }
    let mut keystore = Vec::new();
    for (name, def, span, _) in definition_entries(&profile, "Keystore", &mut diags) {
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

    // The local files the profile took in. A line of a downloaded text is in
    // a file of the cache, which nobody edits and a watcher has no use for;
    // and a downloaded text takes in no local file.
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
        source: SourceInfo {
            main_text: main.clone(),
            main,
            includes,
            remote,
        },
    };
    validate(
        &mut config,
        base_dir,
        opts,
        &external_reach,
        &group_reach,
        &mut diags,
    );
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

fn validate(
    config: &mut Config,
    base_dir: &Path,
    opts: &LoadOptions,
    external_reach: &HashMap<String, Reach>,
    group_reach: &HashMap<String, Reach>,
    diags: &mut Diagnostics,
) {
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

    // FINAL: matching ends at the first FINAL, and the last one gives the
    // policy and the options, so each FINAL before it says it does not
    // (W0021); the rules after the first FINAL never run (W0019).
    match config.first_final().zip(config.effective_final()) {
        None => {
            let span = config.rules.last().map(|r| r.span.clone());
            let mut d = Diagnostic::error(
                codes::E_MISSING_FINAL,
                "the [Rule] section must end with an enabled FINAL rule",
            );
            // A profile that is only the line naming its managed profile has
            // nothing to add a rule to: a rule written into it would make the
            // local file the text in effect.
            let awaited = config
                .source
                .remote
                .iter()
                .any(|r| r.role == RemoteRole::Managed && !r.found());
            if config.rules.is_empty() && awaited {
                d = d.with_hint(
                    "the rules come with the managed profile, which has not been downloaded yet",
                );
            }
            diags.push(match span {
                Some(s) => d.at(s),
                None => d,
            });
        }
        Some((first, last)) => {
            for r in &config.rules[..last] {
                if matches!(r.kind, RuleKind::Final) {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_DUPLICATE_FINAL,
                            "this FINAL does not give the policy; the last FINAL rule does",
                        )
                        .at(r.span.clone()),
                    );
                }
            }
            let mut dead = config.rules[first + 1..]
                .iter()
                .filter(|r| !matches!(r.kind, RuleKind::Final));
            if let Some(head) = dead.next() {
                diags.push(
                    Diagnostic::warning(
                        codes::W_RULES_AFTER_FINAL,
                        format!("{} rule(s) after FINAL never take effect", 1 + dead.count()),
                    )
                    .at(head.span.clone()),
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
            // said whatever else is wrong with the line: the old reading is
            // what explains a missing `encrypt-method` or `password`
            if p.legacy_custom {
                diags.push(
                    Diagnostic::info(
                        codes::I_LEGACY_MIGRATED,
                        format!(
                            "{}: legacy type `custom` read as `ss`; the module URL is ignored",
                            named("policy", &p.name)
                        ),
                    )
                    .at(p.span.clone()),
                );
            }
            // A program that a downloaded text names is not started, so its
            // parameters are not read: nothing is said about them. An
            // `external` policy the map does not know is not the user's own.
            let unstarted = (p.kind == PolicyKind::External)
                .then(|| external_reach.get(&p.name).copied())
                .and_then(|reach| NotImplemented::external_in(reach.unwrap_or(Reach::Nowhere)));
            let mut outcome = match unstarted {
                Some(why) => SpecOutcome {
                    not_implemented: Some(why),
                    ..SpecOutcome::default()
                },
                None => to_spec(p, &env),
            };
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
                        .with_hint(
                            "choose a `local-port` that neither `http-listen` nor `socks5-listen` uses",
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
                        .with_hint("give every `external` policy a `local-port` of its own")
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
                    diags.push(why.diagnostic().at(p.span.clone()));
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
            // a group the map does not know is not the user's own
            let reach = group_reach.get(&g.name).copied().unwrap_or(Reach::Nowhere);
            let outcome = to_group_spec(g, base_dir, reach, &lookup);
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
    use crate::remote::fake::{Fake, hours};
    use crate::spec::PolicyPath;
    use std::path::Path;
    use std::time::{Duration, SystemTime};
    use url::Url;

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

    /// The hint of `E0010` reads as it did when the site gave it itself.
    #[test]
    fn a_missing_final_is_printed_with_the_hint_it_always_had() {
        let l = load_text("[Rule]\nDOMAIN,a,DIRECT\n");
        let d = l
            .diagnostics
            .iter()
            .find(|d| d.code == codes::E_MISSING_FINAL)
            .expect("E0010");
        assert_eq!(
            d.to_string(),
            "error[E0010] /profiles/t.conf:2: the [Rule] section must end with an enabled FINAL rule\n  hint: add `FINAL,DIRECT` as the last rule"
        );
    }

    /// A parse error that carries a hint of its own keeps it on its way into
    /// the report, where its code has no default to give.
    #[test]
    fn a_rule_part_in_the_wrong_place_is_reported_with_the_hint_of_its_site() {
        let l = load_text("[Rule]\nUSER-AGENT,x,REJECT,pre-matching\nFINAL,DIRECT\n");
        let d = l
            .diagnostics
            .iter()
            .find(|d| d.code == codes::E_NOT_ALLOWED_HERE)
            .expect("E0014");
        assert_eq!(codes::hint(d.code), None);
        let hint = d.hint.expect("the site's own hint");
        assert!(hint.starts_with("remove `pre-matching`"), "{hint}");
    }

    /// Whichever way a site builds its diagnostic — from a parse error, from
    /// a parameter reader, in the loader itself —, it has the hint of its
    /// code, and a code without one stays without.
    #[test]
    fn a_load_gives_each_diagnostic_the_hint_of_its_code() {
        let l = load_text(
            "stray line\n[General]\nmystery = 1\nvif-mode = on\ncompatibility-mode = true\n\
[Nowhere]\nx = 1\n[MITM]\nx = 1\n\
[Proxy]\nA = nope, h.test, 1\nB = http, h.test, 80, bogus=1\nB = http, h.test, 81\n\
[Rule]\nDOMAIN,a,Missing\nFOO,a,DIRECT\nFINAL,DIRECT\nFINAL,DIRECT\nDOMAIN,z,DIRECT\n",
        );
        for d in l.diagnostics.iter() {
            assert_eq!(d.hint, codes::hint(d.code), "{d}");
        }
        for (code, has_one) in [
            (codes::W_LINE_OUTSIDE_SECTION, true),
            (codes::W_UNKNOWN_KEY, true),
            (codes::W_VANISHED_KEY, true),
            (codes::W_UNKNOWN_SECTION, true),
            (codes::E_UNKNOWN_POLICY_TYPE, true),
            (codes::E_DUPLICATE_NAME, true),
            (codes::E_UNKNOWN_POLICY_REF, true),
            (codes::E_UNKNOWN_RULE_TYPE, true),
            (codes::W_DUPLICATE_FINAL, true),
            (codes::W_RULES_AFTER_FINAL, true),
            (codes::W_PLATFORM_IGNORED, false),
            (codes::W_DEFERRED_SECTION, false),
        ] {
            let found: Vec<&Diagnostic> = l.diagnostics.iter().filter(|d| d.code == code).collect();
            assert!(!found.is_empty(), "{code} is not reported");
            for d in found {
                assert_eq!(d.hint.is_some(), has_one, "{d}");
            }
        }
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
    fn rules_after_the_first_final_are_counted() {
        // The manual: "In the [Rule] section, a FINAL rule immediately
        // terminates rule matching. Rules included or defined after it are
        // never evaluated", and "If several enabled FINAL rules are present,
        // the last one is the effective one." A rule between two FINALs is
        // therefore dead (W0019) like one after the last, and the FINAL that
        // ends matching without giving the policy is told so (W0021).
        let l = load_text("[Rule]\nFINAL,DIRECT\nDOMAIN,a,DIRECT\nFINAL,REJECT\n");
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let found = |code: &str| -> Vec<(String, u32)> {
            l.diagnostics
                .iter()
                .filter(|d| d.code == code)
                .map(|d| (d.message.clone(), d.span.as_ref().map_or(0, |s| s.line)))
                .collect()
        };
        assert_eq!(
            found(codes::W_RULES_AFTER_FINAL),
            [("1 rule(s) after FINAL never take effect".to_string(), 3)]
        );
        assert_eq!(
            found(codes::W_DUPLICATE_FINAL),
            [(
                "this FINAL does not give the policy; the last FINAL rule does".to_string(),
                2
            )]
        );

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
    fn matching_ends_at_the_first_final_and_the_last_one_gives_the_policy() {
        let l = load_text("[Proxy]\nA = direct\n[Rule]\nFINAL,DIRECT\nDOMAIN,a.com,A\nFINAL,A\n");
        assert_eq!(l.config.first_final(), Some(0));
        assert_eq!(l.config.effective_final(), Some(2));
        let codes = codes_of(&l);
        assert!(codes.contains(&codes::W_DUPLICATE_FINAL));
        assert!(codes.contains(&codes::W_RULES_AFTER_FINAL));
    }

    /// With one FINAL it is both the one that ends matching and the one that
    /// gives the policy; with none there is neither.
    #[test]
    fn a_single_final_is_the_first_and_the_effective_one() {
        let l = load_text("[Rule]\nDOMAIN,a.com,DIRECT\nFINAL,DIRECT\n");
        assert_eq!(l.config.first_final(), Some(1));
        assert_eq!(l.config.effective_final(), Some(1));
        let l = load_text("[Rule]\nDOMAIN,a.com,DIRECT\n");
        assert_eq!(l.config.first_final(), None);
        assert_eq!(l.config.effective_final(), None);
    }

    /// Every rule after the first FINAL that is not a FINAL is counted, the
    /// ones between two FINALs and the ones after the last alike, and the
    /// warning stands on the first of them.
    #[test]
    fn the_rules_between_and_after_the_finals_are_counted_together() {
        let l = load_text(
            "[Rule]\nDOMAIN,a,DIRECT\nFINAL,DIRECT\nDOMAIN,b,DIRECT\nFINAL,REJECT\nDOMAIN,c,DIRECT\nFINAL,DIRECT\nDOMAIN,d,DIRECT\n",
        );
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let after: Vec<_> = l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_RULES_AFTER_FINAL)
            .collect();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].message, "3 rule(s) after FINAL never take effect");
        assert_eq!(after[0].span.as_ref().map(|s| s.line), Some(4));
        // one for each FINAL that does not give the policy
        let lines: Vec<u32> = l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_DUPLICATE_FINAL)
            .filter_map(|d| d.span.as_ref().map(|s| s.line))
            .collect();
        assert_eq!(lines, [3, 5]);
    }

    /// A FINAL that a requirement turns off is no FINAL: of two with
    /// requirements that exclude each other only one is there, and the
    /// profile reads as if it had been the only one written.
    #[test]
    fn a_final_a_requirement_turns_off_is_not_a_final() {
        let text = "[Rule]\nDOMAIN,a.com,DIRECT\nFINAL,DIRECT #!REQUIREMENT SYSTEM=='macOS'\nFINAL,REJECT #!REQUIREMENT SYSTEM!='macOS'\n";
        for (system, kept) in [("macOS", "FINAL,DIRECT"), ("Linux", "FINAL,REJECT")] {
            let mut opts = LoadOptions::for_tests();
            opts.environment.system = system.to_string();
            let l = from_text(text, Path::new("/profiles/t.conf"), &opts);
            let codes = codes_of(&l);
            assert!(!l.diagnostics.has_errors(), "{system}: {codes:?}");
            assert!(!codes.contains(&codes::W_DUPLICATE_FINAL), "{system}");
            assert!(!codes.contains(&codes::W_RULES_AFTER_FINAL), "{system}");
            assert_eq!(l.config.rules.len(), 2, "{system}");
            assert_eq!(l.config.rules[1].raw, kept, "{system}");
            assert_eq!(l.config.first_final(), Some(1), "{system}");
            assert_eq!(l.config.effective_final(), Some(1), "{system}");
        }
    }

    /// The warning about rules after the first FINAL stands on the rule, in
    /// the file that holds it, and the FINAL that does not give the policy is
    /// told so where it is written: a file included before a FINAL of the
    /// profile's own is the case the manual warns about.
    #[test]
    fn the_final_warnings_stand_in_the_files_that_hold_the_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("main.conf"),
            "[Proxy]\nP = direct\n[Rule]\nDOMAIN,first.example,DIRECT\n#!include shared.dconf\nDOMAIN,local.example,P\nFINAL,P\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("shared.dconf"),
            "[Rule]\nDOMAIN,included.example,DIRECT\nFINAL,DIRECT\n",
        )
        .unwrap();
        let l = load(&dir.path().join("main.conf"), &LoadOptions::for_tests()).unwrap();
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let at = |code: &str| -> Vec<(String, u32)> {
            l.diagnostics
                .iter()
                .filter(|d| d.code == code)
                .map(|d| {
                    let span = d.span.as_ref().expect("a position");
                    let file = span.file.file_name().unwrap().to_string_lossy();
                    (file.into_owned(), span.line)
                })
                .collect()
        };
        assert_eq!(
            at(codes::W_RULES_AFTER_FINAL),
            [("main.conf".to_string(), 6)]
        );
        assert_eq!(
            at(codes::W_DUPLICATE_FINAL),
            [("shared.dconf".to_string(), 3)]
        );
    }

    /// What the manual warns about, with the included file downloaded: it ends
    /// with a FINAL of its own and the profile goes on after the include. The
    /// rule that follows is never evaluated and the policy is that of the
    /// profile's own FINAL; the warnings stand in the file that holds the
    /// line each is about.
    #[test]
    fn a_downloaded_final_ends_matching_for_the_rules_that_follow_it() {
        let link = "https://example.com/shared.conf";
        let fake = Arc::new(crate::remote::fake::Fake::new().text(
            link,
            "[Rule]\nDOMAIN,included.example,DIRECT\nFINAL,DIRECT\n",
        ));
        let dir = tempfile::tempdir().unwrap();
        let l = load_with_remote(
            dir.path(),
            &format!(
                "[Proxy]\nP = direct\n[Rule]\nDOMAIN,first.example,DIRECT\n#!include {link}\nDOMAIN,local.example,P\nFINAL,P\n"
            ),
            &fake,
        );
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let raws: Vec<&str> = l.config.rules.iter().map(|r| r.raw.as_str()).collect();
        assert_eq!(
            raws,
            [
                "DOMAIN,first.example,DIRECT",
                "DOMAIN,included.example,DIRECT",
                "FINAL,DIRECT",
                "DOMAIN,local.example,P",
                "FINAL,P"
            ]
        );
        // matching ends at the downloaded FINAL, so the rule after it is dead;
        // the policy is the last FINAL's
        assert_eq!(l.config.first_final(), Some(2));
        assert_eq!(l.config.effective_final(), Some(4));
        let at = |code: &str| -> Vec<(String, u32)> {
            l.diagnostics
                .iter()
                .filter(|d| d.code == code)
                .map(|d| {
                    let span = d.span.as_ref().expect("a position");
                    (span.file.display().to_string(), span.line)
                })
                .collect()
        };
        assert_eq!(
            at(codes::W_RULES_AFTER_FINAL),
            [(dir.path().join("main.conf").display().to_string(), 6)]
        );
        assert_eq!(
            at(codes::W_DUPLICATE_FINAL),
            [(fake.path_of(link).display().to_string(), 3)]
        );
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

    /// A `[Proxy]` line that redefines `DIRECT` is ignored without a word —
    /// no diagnostic of any kind — and the built-in stays what it is.
    /// Redefining any other built-in name is an error, and defines nothing.
    #[test]
    fn redefining_direct_is_silent_and_redefining_another_built_in_is_an_error() {
        let l = load_text("[Proxy]\nDIRECT = reject\n[Rule]\nFINAL,DIRECT\n");
        assert!(codes_of(&l).is_empty(), "{:?}", codes_of(&l));
        assert!(l.config.policies.is_empty());
        for name in [
            "REJECT",
            "REJECT-DROP",
            "REJECT-NO-DROP",
            "REJECT-TINYGIF",
            "CELLULAR",
            "CELLULAR-ONLY",
            "HYBRID",
            "NO-HYBRID",
        ] {
            let l = load_text(&format!("[Proxy]\n{name} = direct\n[Rule]\nFINAL,DIRECT\n"));
            assert_eq!(codes_of(&l), [codes::E_BUILTIN_REDEFINED], "{name}");
            assert!(l.config.policies.is_empty(), "{name}");
        }
    }

    /// Each iOS-only built-in a rule names is warned of once, however often it
    /// is named, and the warning says what becomes of it. The other built-ins
    /// are not warned of.
    #[test]
    fn each_ios_only_built_in_is_warned_of_once_and_no_other_is() {
        let l = load_text(
            "[Rule]\nDOMAIN,a.test,CELLULAR\nDOMAIN,b.test,CELLULAR-ONLY\nDOMAIN,c.test,HYBRID\nDOMAIN,d.test,NO-HYBRID\nDOMAIN,e.test,CELLULAR\nDOMAIN,f.test,NO-HYBRID\nDOMAIN,g.test,REJECT\nDOMAIN,h.test,REJECT-DROP\nDOMAIN,i.test,REJECT-NO-DROP\nDOMAIN,j.test,REJECT-TINYGIF\nFINAL,DIRECT\n",
        );
        assert_eq!(codes_of(&l), [codes::W_IOS_BUILTIN_AS_DIRECT; 4]);
        let told: Vec<&str> = l.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            told,
            [
                "`CELLULAR` is iOS-only; treated as DIRECT",
                "`CELLULAR-ONLY` is iOS-only; treated as DIRECT",
                "`HYBRID` is iOS-only; treated as DIRECT",
                "`NO-HYBRID` is iOS-only; treated as DIRECT",
            ]
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

    /// `[IP Rewrite]` acts on the virtual interface, which a later version
    /// brings. Until then the section is read, its lines kept in the order
    /// they were written, and it is said to be inactive like the other
    /// sections waiting for that version; it is not an unknown section.
    #[test]
    fn ip_rewrite_is_a_known_section_kept_for_later() {
        for header in ["IP Rewrite", "ip rewrite"] {
            let l = load_text(&format!(
                "[{header}]\n10.0.0.1 = reject\n10.0.0.0/8 = drop\nfd00::/8 = reflect\n\
[Rule]\nFINAL,DIRECT\n"
            ));
            assert!(
                !l.diagnostics.has_errors(),
                "{header}: {:?}",
                l.diagnostics.iter().collect::<Vec<_>>()
            );
            let c = codes_of(&l);
            assert!(!c.contains(&codes::W_UNKNOWN_SECTION), "{header}: {c:?}");
            assert!(l.config.unknown_sections.is_empty(), "{header}");
            let inactive: Vec<&str> = l
                .diagnostics
                .iter()
                .filter(|d| d.code == codes::W_DEFERRED_SECTION)
                .map(|d| d.message.as_str())
                .collect();
            assert_eq!(
                inactive,
                [format!(
                    "sections parsed but inactive in this version: [{header}]"
                )],
                "{header}"
            );
            let section = l
                .config
                .deferred
                .get("IP Rewrite")
                .unwrap_or_else(|| panic!("{header}: the section is not kept"));
            let lines: Vec<&str> = section.entries.iter().map(|e| e.raw.as_str()).collect();
            assert_eq!(
                lines,
                [
                    "10.0.0.1 = reject",
                    "10.0.0.0/8 = drop",
                    "fd00::/8 = reflect"
                ],
                "{header}"
            );
        }
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

    /// Surge's old `custom` line loads as `ss`, and the loader says so: once
    /// for each such line, whether or not something else is wrong with it
    /// (the old reading may well be what explains it). The policy is named,
    /// and nothing else of the line is said.
    #[test]
    fn a_legacy_custom_line_is_said_to_be_read_as_ss() {
        let l = load_text(
            "[Proxy]\nOld = custom, 127.0.0.1, 8388, aes-256-gcm, s3cret, https://m.test/x.module\n\
Broken = custom, 127.0.0.1, 8388, aes-256-gcm\n\
New = ss, 127.0.0.1, 8388, encrypt-method=aes-256-gcm, password=s3cret\n\
[Rule]\nFINAL,DIRECT\n",
        );
        let told: Vec<(u32, &str)> = l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::I_LEGACY_MIGRATED)
            .map(|d| (d.span.as_ref().map_or(0, |s| s.line), d.message.as_str()))
            .collect();
        assert_eq!(
            told,
            [
                (
                    2,
                    "policy `Old`: legacy type `custom` read as `ss`; the module URL is ignored"
                ),
                (
                    3,
                    "policy `Broken`: legacy type `custom` read as `ss`; the module URL is ignored"
                ),
            ]
        );
        // the second has no password, which is an error of its own
        assert_eq!(
            l.diagnostics
                .iter()
                .filter(|d| d.severity == crate::Severity::Error)
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>(),
            ["policy `Broken`: `password` is required"]
        );
        for d in l.diagnostics.iter() {
            assert!(
                !d.message.contains("s3cret") && !d.message.contains("m.test"),
                "{d}"
            );
        }
        // and every one of the three is an `ss` policy
        assert_eq!(
            l.config
                .policies
                .iter()
                .map(|p| p.kind.keyword())
                .collect::<Vec<_>>(),
            ["ss", "ss", "ss"]
        );
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

    /// `W0003` names the item it does not know only when the item is written
    /// as a parameter is, and never what follows its `=`: that is a value,
    /// and an item of another shape can be anything that slid into the line.
    #[test]
    fn an_unknown_rule_parameter_is_named_only_when_it_is_written_as_one() {
        let said = |text: &str| -> Vec<String> {
            let l = load_text(text);
            assert!(!l.diagnostics.has_errors(), "{text}");
            l.diagnostics
                .iter()
                .filter(|d| d.code == codes::W_UNKNOWN_RULE_PARAM)
                .map(|d| d.message.clone())
                .collect()
        };
        let rule = |item: &str| {
            said(&format!(
                "[Rule]\nDOMAIN,a.com,DIRECT,{item}\nFINAL,DIRECT\n"
            ))
        };

        // a flag misspelt, and a `name=value` nobody knows: the name
        assert_eq!(
            rule("no-resolv"),
            ["unknown rule parameter `no-resolv` ignored"]
        );
        assert_eq!(
            rule("mystery=S3CR3T"),
            ["unknown rule parameter `mystery` ignored"]
        );
        // a known name with a value it cannot take is said the same way
        assert_eq!(
            rule("update-interval=S3CR3T"),
            ["unknown rule parameter `update-interval` ignored"]
        );
        // blanks around the `=` are no part of the name
        assert_eq!(
            rule("mystery = S3CR3T"),
            ["unknown rule parameter `mystery` ignored"]
        );
        // 32 letters are a name still
        let longest = "a".repeat(32);
        assert_eq!(
            rule(&longest),
            [format!("unknown rule parameter `{longest}` ignored")]
        );
        // what is written like no parameter is not quoted at all
        let long = "a".repeat(33);
        for item in [
            "S3CR3T",
            "Mystery=S3CR3T",
            "my stery",
            "aHVudGVyMg==",
            "=S3CR3T",
            "no_resolve",
            "h2",
            long.as_str(),
            // lower-case letters, and not the ASCII ones
            "пароль",
            "straße=S3CR3T",
        ] {
            assert_eq!(rule(item), ["unknown rule parameter ignored"], "{item}");
        }
        // the line is all that says which item is meant then
        let l =
            load_text("[Rule]\nDOMAIN,b.com,DIRECT\nDOMAIN,a.com,DIRECT,S3CR3T\nFINAL,DIRECT\n");
        let at: Vec<Option<u32>> = l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_UNKNOWN_RULE_PARAM)
            .map(|d| d.span.as_ref().map(|s| s.line))
            .collect();
        assert_eq!(at, [Some(3)]);

        // the sub-rules of a logical rule, and the lines of an inline rule set
        let sub = |item: &str| {
            said(&format!(
                "[Rule]\nAND,((DOMAIN,a,{item}),(DOMAIN,b)),DIRECT\nFINAL,DIRECT\n"
            ))
        };
        assert_eq!(sub("bogus"), ["unknown sub-rule parameter `bogus` ignored"]);
        assert_eq!(sub("S3CR3T"), ["unknown sub-rule parameter ignored"]);
        let inline = |item: &str| {
            said(&format!(
                "[Rule]\nRULE-SET,Mine,DIRECT\nFINAL,DIRECT\n[Ruleset Mine]\nDOMAIN,a.com,{item}\n"
            ))
        };
        assert_eq!(
            inline("bogus"),
            ["unknown [Ruleset Mine] parameter `bogus` ignored"]
        );
        assert_eq!(
            inline("Proxy"),
            ["unknown [Ruleset Mine] parameter ignored"]
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
        // the manual's page for the policy says nothing of a clash between two
        assert_eq!(
            errors[0].hint,
            Some("give every `external` policy a `local-port` of its own")
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
        for d in loaded
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::E_INVALID_POLICY_PARAM)
        {
            assert_eq!(
                d.hint,
                Some("choose a `local-port` that neither `http-listen` nor `socks5-listen` uses")
            );
        }
        assert!(loaded.config.spec("A").is_none() && loaded.config.spec("B").is_none());
        assert!(loaded.config.spec("C").is_some());
    }

    /// The load keeps an `external` policy off the listeners the profile
    /// has. Whoever puts others in their place afterwards asks again, of the
    /// listeners the configuration has by then.
    #[test]
    fn an_external_policy_is_asked_about_the_listeners_the_config_has_now() {
        use crate::general::{parse_http_listen, parse_socks5_listen};
        let loaded = load_text(
            "[General]\nhttp-listen = 127.0.0.1:6152\nsocks5-listen = 127.0.0.1:6153\n\
[Proxy]\nA = external, exec=/bin/a, local-port=7000\nB = external, exec=/bin/b, local-port=7001\n\
[Rule]\nFINAL,DIRECT\n",
        );
        assert!(!loaded.diagnostics.has_errors());
        let config = loaded.config;
        assert_eq!(config.external_on_own_listener(), None);

        for address in ["127.0.0.1:7000", "0.0.0.0:7000", "[::1]:7000", "[::]:7000"] {
            let mut on_socks5 = config.clone();
            on_socks5.general.socks5_listen = parse_socks5_listen(address).unwrap();
            assert_eq!(
                on_socks5.external_on_own_listener(),
                Some(("socks5-listen", 7000)),
                "{address}"
            );
            let mut on_http = config.clone();
            on_http.general.http_listen = parse_http_listen(address).unwrap();
            assert_eq!(
                on_http.external_on_own_listener(),
                Some(("http-listen", 7000)),
                "{address}"
            );
        }

        // the port is the policy's, and the address one that the program's
        // loopback port does not share
        let mut elsewhere = config.clone();
        elsewhere.general.socks5_listen = parse_socks5_listen("192.0.2.1:7000").unwrap();
        elsewhere.general.http_listen = parse_http_listen("192.0.2.1:7001").unwrap();
        assert_eq!(elsewhere.external_on_own_listener(), None);
        let mut other_port = config.clone();
        other_port.general.socks5_listen = parse_socks5_listen("127.0.0.1:7002").unwrap();
        assert_eq!(other_port.external_on_own_listener(), None);

        // the first policy in the profile's order that has one is the one told
        let mut both = config.clone();
        both.general.socks5_listen = parse_socks5_listen("127.0.0.1:7001").unwrap();
        both.general.http_listen = parse_http_listen("127.0.0.1:7000").unwrap();
        assert_eq!(both.external_on_own_listener(), Some(("http-listen", 7000)));
        both.general.http_listen = parse_http_listen("127.0.0.1:7002").unwrap();
        assert_eq!(
            both.external_on_own_listener(),
            Some(("socks5-listen", 7001))
        );
    }

    #[test]
    fn a_profile_without_an_external_policy_has_none_on_its_listeners() {
        use crate::general::parse_socks5_listen;
        let loaded = load_text(
            "[General]\nsocks5-listen = 127.0.0.1:6153\n[Proxy]\nA = direct\nB = http, 127.0.0.1, 7000\n\
[Rule]\nFINAL,DIRECT\n",
        );
        assert!(!loaded.diagnostics.has_errors());
        let mut config = loaded.config;
        assert_eq!(config.external_on_own_listener(), None);
        config.general.socks5_listen = parse_socks5_listen("127.0.0.1:7000").unwrap();
        assert_eq!(config.external_on_own_listener(), None);
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
            // `tos` is for the socket of the tunnel: no sentence on it
            on(
                "P = wireguard, section-name=home, interface=en0, allow-other-interface=true, tos=16",
                ["interface", "allow-other-interface"]
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
                "P = wireguard, section-name=home, interface=, tfo=true",
                vec![
                    bad_value("interface", "", "a network interface name"),
                    not_applicable("`tfo` does not apply to `wireguard` policies; ignored"),
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

    /// A load whose `#!include` lines name links, with the texts those links
    /// were downloaded as.
    fn load_with_remote(
        dir: &Path,
        profile: &str,
        fake: &Arc<crate::remote::fake::Fake>,
    ) -> Loaded {
        load_modified(dir, profile, None, fake)
    }

    /// `load_with_remote` for a local file that was last modified at
    /// `modified`, when that is given.
    fn load_modified(
        dir: &Path,
        profile: &str,
        modified: Option<SystemTime>,
        fake: &Arc<crate::remote::fake::Fake>,
    ) -> Loaded {
        let mut opts = LoadOptions::for_tests();
        opts.remote = Some(fake.clone());
        let main = dir.join("main.conf");
        std::fs::write(&main, profile).unwrap();
        if let Some(at) = modified {
            std::fs::File::options()
                .write(true)
                .open(&main)
                .unwrap()
                .set_modified(at)
                .unwrap();
        }
        load(&main, &opts).unwrap()
    }

    /// A downloaded text is no file the user edits: the list of local
    /// includes, which a watcher follows, has none, and the remote files are
    /// listed apart, in the order the load came across them.
    #[test]
    fn the_source_lists_the_local_includes_and_the_remote_files_apart() {
        let (b, a, gone) = (
            "https://example.com/b.conf",
            "https://example.com/a.conf",
            "https://example.com/gone.conf",
        );
        let fake = Arc::new(
            crate::remote::fake::Fake::new()
                .text(b, "[Rule]\nDOMAIN,b.example,A\n")
                .text(a, "[Rule]\nDOMAIN,a.example,A\n")
                .missing(gone, None),
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("p.dconf"), "[Proxy]\nA = direct\n").unwrap();
        let l = load_with_remote(
            dir.path(),
            &format!(
                "[Proxy]\n#!include p.dconf\n[Rule]\n#!include {b}\n#!include {a}\n#!include {gone}\nFINAL,A\n"
            ),
            &fake,
        );
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let source = &l.config.source;
        assert!(source.main.ends_with("main.conf"));
        assert_eq!(source.main_text, source.main);
        assert_eq!(source.includes.len(), 1);
        assert!(source.includes[0].ends_with("p.dconf"));
        let found: Vec<(&str, Option<&Path>)> = source
            .remote
            .iter()
            .map(|r| (r.url.as_str(), r.path.as_deref()))
            .collect();
        assert_eq!(
            found,
            [
                (b, Some(fake.path_of(b).as_path())),
                (a, Some(fake.path_of(a).as_path())),
                (gone, None),
            ]
        );
        assert_eq!(l.config.rules.len(), 3);
    }

    /// A line of a downloaded text that does not load is reported in the file
    /// the text is kept in, at the line it has there.
    #[test]
    fn a_line_of_a_remote_text_that_does_not_load_is_reported_where_it_is_written() {
        let link = "https://example.com/r.conf";
        let fake = Arc::new(crate::remote::fake::Fake::new().text(
            link,
            "[Rule]\nDOMAIN,a.example,DIRECT\nFOO,b.example,DIRECT\n",
        ));
        let dir = tempfile::tempdir().unwrap();
        let l = load_with_remote(
            dir.path(),
            &format!("[Rule]\n#!include {link}\nFINAL,DIRECT\n"),
            &fake,
        );
        let found: Vec<(&str, &Path, u32)> = l
            .diagnostics
            .iter()
            .map(|d| {
                let span = d.span.as_ref().expect("a position");
                (d.code, span.file.as_ref(), span.line)
            })
            .collect();
        let cached = fake.path_of(link);
        assert_eq!(found, [(codes::E_UNKNOWN_RULE_TYPE, cached.as_path(), 3)]);
        assert_eq!(l.config.rules.len(), 2);
    }

    /// A downloaded text goes through the reader of a profile as a local
    /// include does: a byte order mark is not part of the first line, comments
    /// are not part of a value, and a requirement directive in front of a line
    /// or behind it turns the line off where it is not met.
    #[test]
    fn a_remote_text_is_read_as_a_local_include_of_the_same_bytes_is() {
        let text = "\u{feff}[Rule]\r\n#!MACOS-ONLY DOMAIN,mac.example,DIRECT\r\n#!IOS-ONLY DOMAIN,ios.example,DIRECT\r\nDOMAIN,tail.example,DIRECT // a comment\r\nDOMAIN,late.example,DIRECT #!IOS-ONLY\r\nDOMAIN,keep.example,DIRECT #!REQUIREMENT CORE_VERSION<22\r\n; a comment line\r\n";
        let link = "https://example.com/r.conf";
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("inc.dconf"), text).unwrap();
        let local = load_with_remote(
            dir.path(),
            "[Rule]\n#!include inc.dconf\nFINAL,DIRECT\n",
            &Arc::new(crate::remote::fake::Fake::new()),
        );
        let fake = Arc::new(crate::remote::fake::Fake::new().text(link, text));
        let remote = load_with_remote(
            dir.path(),
            &format!("[Rule]\n#!include {link}\nFINAL,DIRECT\n"),
            &fake,
        );
        let raws =
            |l: &Loaded| -> Vec<String> { l.config.rules.iter().map(|r| r.raw.clone()).collect() };
        let told = |l: &Loaded| -> Vec<(&'static str, String)> {
            let mut told: Vec<_> = l
                .diagnostics
                .iter()
                .map(|d| (d.code, d.message.clone()))
                .collect();
            told.sort();
            told
        };
        assert_eq!(
            raws(&remote),
            [
                "DOMAIN,mac.example,DIRECT",
                "DOMAIN,tail.example,DIRECT",
                "DOMAIN,keep.example,DIRECT",
                "FINAL,DIRECT"
            ]
        );
        assert_eq!(raws(&remote), raws(&local));
        assert_eq!(told(&remote), told(&local));
        // the two lines that were turned off said so, in the file they are in
        let off: Vec<(&Path, u32)> = remote
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::I_LINE_DISABLED)
            .map(|d| {
                let span = d.span.as_ref().expect("a position");
                (span.file.as_ref(), span.line)
            })
            .collect();
        let cached = fake.path_of(link);
        assert_eq!(off, [(cached.as_path(), 3), (cached.as_path(), 5)]);
    }

    /// Two files that bring a section of one name: the profile's own wins over
    /// the files it includes, and of two included files the one included first
    /// does. The other is reported at its section, in its own file.
    #[test]
    fn of_two_sections_of_one_name_the_profiles_own_and_then_the_first_included_is_used() {
        let remote_link = "https://example.com/remote.conf";
        let fake = Arc::new(
            crate::remote::fake::Fake::new()
                .text(remote_link, "[Ruleset x]\nDOMAIN,remote.example\n"),
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("local.conf"),
            "[Ruleset x]\nDOMAIN,local.example\n",
        )
        .unwrap();
        let own = "[Ruleset x]\nDOMAIN,main.example\n";
        let tail = "[Rule]\nRULE-SET,x,DIRECT\nFINAL,DIRECT\n";
        let include = |target: &str| format!("#!include {target}\n");
        let (local, remote) = ("local.conf", remote_link);
        let cached = fake.path_of(remote_link);
        for (profile, wins, ignored) in [
            (
                format!(
                    "[Ruleset *]\n{}{}{own}{tail}",
                    include(local),
                    include(remote)
                ),
                "DOMAIN,main.example",
                vec![(dir.path().join("local.conf"), 1), (cached.clone(), 1)],
            ),
            (
                format!("[Ruleset *]\n{}{}{tail}", include(local), include(remote)),
                "DOMAIN,local.example",
                vec![(cached.clone(), 1)],
            ),
            (
                format!("[Ruleset *]\n{}{}{tail}", include(remote), include(local)),
                "DOMAIN,remote.example",
                vec![(dir.path().join("local.conf"), 1)],
            ),
        ] {
            let l = load_with_remote(dir.path(), &profile, &fake);
            assert!(
                !l.diagnostics.has_errors(),
                "{profile}: {:?}",
                l.diagnostics.clone().into_vec()
            );
            let sets: Vec<Vec<&str>> = l
                .config
                .rulesets
                .iter()
                .map(|s| s.rules.iter().map(|r| r.raw.as_str()).collect())
                .collect();
            assert_eq!(sets, [vec![wins]], "{profile}");
            let said: Vec<(&str, std::path::PathBuf, u32)> = l
                .diagnostics
                .iter()
                .filter(|d| d.code == codes::W_DUPLICATE_RULESET)
                .map(|d| {
                    let span = d.span.as_ref().expect("a position");
                    (d.message.as_str(), span.file.to_path_buf(), span.line)
                })
                .collect();
            let message = "duplicate [Ruleset x] ignored; the first definition is used";
            let expected: Vec<(&str, std::path::PathBuf, u32)> = ignored
                .into_iter()
                .map(|(file, line)| (message, file, line))
                .collect();
            assert_eq!(said, expected, "{profile}");
        }
    }

    /// An `external` policy starts a program on this machine, so one that a
    /// downloaded text defines is kept (the groups and rules that name it
    /// still hold) but is not built: it has no spec, behaves as REJECT, and
    /// takes no part in the checks of the policies that will be started. One
    /// warning says so for the load, at the first of them.
    #[test]
    fn an_external_policy_in_downloaded_text_is_not_started() {
        let link = "https://example.com/proxies.conf";
        let fake = Arc::new(crate::remote::fake::Fake::new().text(
            link,
            "[Proxy]\nE1 = external, exec=/bin/true, local-port=1081, args=-a\nE2 = external, exec=/bin/true, local-port=1081\nBare = external\nH = http, h.example, 8080\n",
        ));
        let dir = tempfile::tempdir().unwrap();
        let l = load_with_remote(
            dir.path(),
            &format!(
                "[Proxy]\n#!include {link}\nLocal = external, exec=/bin/true, local-port=1081\n[Proxy Group]\nG = select, E1, E2, Bare, H, Local\n[Rule]\nFINAL,G\n"
            ),
            &fake,
        );
        // no error: not an unknown member, not a missing `exec`, not a port
        // that two programs would want
        let told: Vec<(&str, &str)> = l
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            told,
            [(
                codes::W_REMOTE_LOCAL_ACCESS,
                "`external` policies written in downloaded text are not started; they behave as REJECT"
            )]
        );
        let warning = l.diagnostics.iter().next().unwrap();
        assert_eq!(
            warning.hint,
            Some("define the policy in the local profile to use it")
        );
        let span = warning.span.as_ref().expect("a position");
        assert_eq!(
            (span.file.as_ref(), span.line),
            (fake.path_of(link).as_path(), 2)
        );
        let config = &l.config;
        let names: Vec<&str> = config.policies.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["E1", "E2", "Bare", "H", "Local"]);
        for name in ["E1", "E2", "Bare"] {
            assert!(config.spec(name).is_none(), "{name}");
            assert_eq!(
                config.not_implemented.get(name),
                Some(&NotImplemented::DownloadedExternal),
                "{name}"
            );
        }
        for name in ["H", "Local"] {
            assert!(config.spec(name).is_some(), "{name}");
            assert_eq!(config.not_implemented.get(name), None, "{name}");
        }
        assert_eq!(config.groups[0].members.len(), 5);
        assert_eq!(config.external_on_own_listener(), None);
    }

    /// The same lines in the profile itself and in a file of its own, which
    /// the user wrote, are built as they always were.
    #[test]
    fn an_external_policy_in_a_local_file_is_built() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ext.dconf"),
            "[Proxy]\nIncluded = external, exec=/bin/true, local-port=1083\n",
        )
        .unwrap();
        let l = load_with_remote(
            dir.path(),
            "[Proxy]\n#!include ext.dconf\nMain = external, exec=/bin/true, local-port=1082\n[Rule]\nFINAL,DIRECT\n",
            &Arc::new(crate::remote::fake::Fake::new()),
        );
        assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics.into_vec());
        for name in ["Included", "Main"] {
            assert!(l.config.spec(name).is_some(), "{name}");
        }
        assert!(l.config.not_implemented.is_empty());
    }

    // Which text of a managed profile is in effect: the local file, or the
    // copy downloaded from the address its `#!MANAGED-CONFIG` line names.

    const MANAGED: &str = "https://example.com/managed.conf";
    const MOVED: &str = "https://example.com/moved.conf";
    const LOCAL_RULES: [&str; 2] = ["DOMAIN,local.example,DIRECT", "FINAL,DIRECT"];
    const COPY_RULES: [&str; 2] = ["DOMAIN,copy.example,DIRECT", "FINAL,DIRECT"];
    const LOCAL_CHANGED: &str = "managed profile: the local file was changed after the last update; it is in effect until the next one";
    const NOT_DOWNLOADED: &str = "managed profile not downloaded yet";
    const NO_LINE: &str =
        "the downloaded managed profile has no #!MANAGED-CONFIG line; it is no longer updated";
    const NO_LINE_HINT: &str =
        "to keep it updated, the profile the address serves has to carry a #!MANAGED-CONFIG line";
    const NO_WAY_OUT: &str = "the #!MANAGED-CONFIG lines of the downloaded profiles lead through too many addresses; the last copy found is in effect";
    const NO_WAY_OUT_HINT: &str = "write the address the profile is finally served from into the #!MANAGED-CONFIG line of the local file";

    fn managed_line(url: &str, interval: u64, strict: bool) -> String {
        format!("#!MANAGED-CONFIG {url} interval={interval} strict={strict}")
    }

    /// A local profile: rules of its own under the given first line.
    fn local_profile(line: &str) -> String {
        format!(
            "{line}\n[General]\nloglevel = notify\n[Rule]\nDOMAIN,local.example,DIRECT\nFINAL,DIRECT\n"
        )
    }

    /// A downloaded profile: rules the local one does not have.
    fn copy_profile(line: &str) -> String {
        format!("{line}\n[Rule]\nDOMAIN,copy.example,DIRECT\nFINAL,DIRECT\n")
    }

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn rules_of(l: &Loaded) -> Vec<&str> {
        l.config.rules.iter().map(|r| r.raw.as_str()).collect()
    }

    /// What a managed profile says about itself, in order: code and message.
    fn managed_said(l: &Loaded) -> Vec<(&'static str, String)> {
        l.diagnostics
            .iter()
            .filter(|d| {
                [
                    codes::W_RESOURCE_UNAVAILABLE,
                    codes::I_MANAGED_SOURCE,
                    codes::W_MANAGED_NOT_UPDATED,
                ]
                .contains(&d.code)
            })
            .map(|d| (d.code, d.message.clone()))
            .collect()
    }

    /// Where the diagnostics of the given codes stand: the name of the file
    /// and the line.
    fn placed(l: &Loaded, of: &[&str]) -> Vec<(String, u32)> {
        l.diagnostics
            .iter()
            .filter(|d| of.contains(&d.code))
            .map(|d| {
                let span = d.span.as_ref().expect("a position");
                let file = span.file.file_name().unwrap();
                (file.to_string_lossy().into_owned(), span.line)
            })
            .collect()
    }

    /// The remote files a load tracks, in order: address, role, whether it has
    /// the text, interval in seconds and strictness.
    fn tracked(l: &Loaded) -> Vec<(String, RemoteRole, bool, u64, bool)> {
        l.config
            .source
            .remote
            .iter()
            .map(|r| {
                (
                    r.url.to_string(),
                    r.role,
                    r.found(),
                    r.interval.as_secs(),
                    r.strict,
                )
            })
            .collect()
    }

    /// An address the walk passed through: its copy exists, and the profile is
    /// not updated from it.
    fn on_the_way(address: &str) -> (String, RemoteRole, bool, u64, bool) {
        (
            address.to_string(),
            RemoteRole::ManagedHop,
            true,
            86400,
            false,
        )
    }

    /// The address the profile is updated from.
    fn updated(
        address: &str,
        found: bool,
        seconds: u64,
        strict: bool,
    ) -> (String, RemoteRole, bool, u64, bool) {
        (
            address.to_string(),
            RemoteRole::Managed,
            found,
            seconds,
            strict,
        )
    }

    fn copy_in_effect(file: &Path) -> String {
        format!(
            "managed profile: the downloaded copy is in effect (`{}`)",
            file.display()
        )
    }

    fn leads_elsewhere(first: &Path) -> String {
        format!(
            "managed profile: the address in the local file leads to another one (see the #!MANAGED-CONFIG line of `{}`); the local file still names the old address",
            first.display()
        )
    }

    fn hint_of(l: &Loaded, code: &str) -> Vec<Option<&'static str>> {
        l.diagnostics
            .iter()
            .filter(|d| d.code == code)
            .map(|d| d.hint)
            .collect()
    }

    /// A caller with nowhere to get downloaded text from loads the local file
    /// as it always did, whatever its first line says.
    #[test]
    fn a_managed_profile_without_a_source_is_the_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let text = local_profile(&managed_line(MANAGED, 3600, true));
        let main = dir.path().join("main.conf");
        std::fs::write(&main, &text).unwrap();
        let l = load(&main, &LoadOptions::for_tests()).unwrap();
        let given = from_text(&text, &main, &LoadOptions::for_tests());
        assert_eq!(rules_of(&l), LOCAL_RULES);
        assert_eq!(l.diagnostics, given.diagnostics);
        assert!(
            l.diagnostics.is_empty(),
            "{:?}",
            l.diagnostics.clone().into_vec()
        );
        assert!(l.config.source.remote.is_empty());
        assert_eq!(l.config.source.main_text, l.config.source.main);
        let managed = l.config.managed.as_ref().expect("the line is read");
        assert_eq!((managed.interval.as_secs(), managed.strict), (3600, true));
    }

    /// The local file is the only text there is until the first download; the
    /// address is tracked all the same, so that somebody downloads it.
    #[test]
    fn a_managed_profile_that_was_never_downloaded_is_the_local_file() {
        for (why, said) in [
            (
                Some("connection refused"),
                "managed profile not downloaded yet: connection refused",
            ),
            (None, NOT_DOWNLOADED),
        ] {
            let fake = Arc::new(Fake::new().missing(MANAGED, why));
            let dir = tempfile::tempdir().unwrap();
            let l = load_modified(
                dir.path(),
                &local_profile(&managed_line(MANAGED, 3600, true)),
                Some(hours(5)),
                &fake,
            );
            assert!(
                !l.diagnostics.has_errors(),
                "{:?}",
                l.diagnostics.clone().into_vec()
            );
            assert_eq!(rules_of(&l), LOCAL_RULES);
            assert_eq!(
                managed_said(&l),
                [(codes::W_RESOURCE_UNAVAILABLE, said.to_string())]
            );
            assert_eq!(
                placed(&l, &[codes::W_RESOURCE_UNAVAILABLE]),
                [("main.conf".to_string(), 1)]
            );
            assert_eq!(tracked(&l), [updated(MANAGED, false, 3600, true)]);
            assert_eq!(l.config.source.main_text, l.config.source.main);
            assert_eq!(fake.asked(), [url(MANAGED)]);
        }
    }

    /// A local file that is only the line pointing to the managed profile is a
    /// profile without rules until something is downloaded: it says that, and
    /// that nothing was. It does not say to add a rule: a rule written into the
    /// file would make the file the profile.
    #[test]
    fn a_pointer_that_has_not_been_downloaded_yet_is_a_profile_without_rules() {
        let fake = Arc::new(Fake::new().missing(MANAGED, Some("connection refused")));
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(
            dir.path(),
            &format!("{}\n", managed_line(MANAGED, 3600, false)),
            Some(hours(5)),
            &fake,
        );
        let told: Vec<(&str, &str, Option<u32>)> = l
            .diagnostics
            .iter()
            .map(|d| {
                (
                    d.code,
                    d.message.as_str(),
                    d.span.as_ref().map(|span| span.line),
                )
            })
            .collect();
        assert_eq!(
            told,
            [
                (
                    codes::W_RESOURCE_UNAVAILABLE,
                    "managed profile not downloaded yet: connection refused",
                    Some(1)
                ),
                (
                    codes::E_MISSING_FINAL,
                    "the [Rule] section must end with an enabled FINAL rule",
                    None
                ),
            ]
        );
        assert!(l.config.rules.is_empty());
        assert_eq!(
            hint_of(&l, codes::E_MISSING_FINAL),
            [Some(
                "the rules come with the managed profile, which has not been downloaded yet"
            )]
        );
        // a local file with rules of its own that lack the last one is told to
        // add it, waiting for a download or not
        let l = load_modified(
            dir.path(),
            &format!(
                "{}\n[Rule]\nDOMAIN,local.example,DIRECT\n",
                managed_line(MANAGED, 3600, false)
            ),
            Some(hours(5)),
            &fake,
        );
        assert_eq!(
            hint_of(&l, codes::E_MISSING_FINAL),
            [codes::hint(codes::E_MISSING_FINAL)]
        );
    }

    /// The copy is the profile once it is newer than the local file: its rules,
    /// its own `#!MANAGED-CONFIG` line, and the file its lines stand in. The
    /// local file is not the profile then, and what is wrong in it is not
    /// reported.
    #[test]
    fn a_copy_newer_than_the_local_file_is_in_effect() {
        let local = format!(
            "{}\n[Rule]\nBAR,local.example,DIRECT\nFINAL,DIRECT\n",
            managed_line(MANAGED, 3600, false)
        );
        let copy = format!(
            "{}\n[Rule]\nDOMAIN,copy.example,DIRECT\nFOO,copy-bad.example,DIRECT\nFINAL,DIRECT\n",
            managed_line(MANAGED, 7200, true)
        );
        let fake = Arc::new(Fake::new().text(MANAGED, &copy).fetched(MANAGED, hours(10)));
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(dir.path(), &local, Some(hours(5)), &fake);
        let cached = fake.path_of(MANAGED);
        assert_eq!(rules_of(&l), COPY_RULES);
        // what is said stands in the file that holds the line it is about
        assert_eq!(
            placed(&l, &[codes::I_MANAGED_SOURCE]),
            [("main.conf".to_string(), 1)]
        );
        assert_eq!(
            placed(&l, &[codes::E_UNKNOWN_RULE_TYPE]),
            [("1.conf".to_string(), 4)]
        );
        assert_eq!(
            l.diagnostics.len(),
            2,
            "{:?}",
            l.diagnostics.clone().into_vec()
        );
        let source = &l.config.source;
        assert!(source.main.ends_with("main.conf"));
        assert_eq!(source.main_text, cached);
        assert_eq!(
            managed_said(&l),
            [(codes::I_MANAGED_SOURCE, copy_in_effect(&cached))]
        );
        // its line is the profile's, and what the address is updated by
        let managed = l.config.managed.as_ref().expect("the copy's line");
        assert_eq!((managed.interval.as_secs(), managed.strict), (7200, true));
        assert_eq!(tracked(&l), [updated(MANAGED, true, 7200, true)]);
        assert_eq!(source.remote[0].path.as_deref(), Some(cached.as_path()));
        assert_eq!(fake.asked(), [url(MANAGED)]);
    }

    /// A local file changed after the last download is the profile until the
    /// next successful check. What is wrong in it is reported, and the address
    /// is updated at the pace its line says, with the strictness its line says:
    /// the copy's line asked for `strict=true`, the local one does not.
    #[test]
    fn a_local_file_changed_after_the_last_update_is_in_effect() {
        let local = format!(
            "#!MANAGED-CONFIG {MANAGED} interval=3600\n[Rule]\nBAR,bad.example,DIRECT\nDOMAIN,local.example,DIRECT\nFINAL,DIRECT\n"
        );
        let copy = copy_profile(&managed_line(MANAGED, 7200, true));
        let fake = Arc::new(Fake::new().text(MANAGED, &copy).fetched(MANAGED, hours(5)));
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(dir.path(), &local, Some(hours(10)), &fake);
        assert_eq!(rules_of(&l), LOCAL_RULES);
        assert_eq!(
            placed(&l, &[codes::I_MANAGED_SOURCE, codes::E_UNKNOWN_RULE_TYPE]),
            [("main.conf".to_string(), 1), ("main.conf".to_string(), 3)]
        );
        assert_eq!(
            managed_said(&l),
            [(codes::I_MANAGED_SOURCE, LOCAL_CHANGED.to_string())]
        );
        let source = &l.config.source;
        assert_eq!(source.main_text, source.main);
        let managed = l.config.managed.as_ref().expect("the local line");
        assert_eq!((managed.interval.as_secs(), managed.strict), (3600, false));
        assert_eq!(tracked(&l), [updated(MANAGED, true, 3600, false)]);
        assert_eq!(
            source.remote[0].path.as_deref(),
            Some(fake.path_of(MANAGED).as_path())
        );
    }

    /// The profile in effect, when the downloaded copy of `MANAGED` was fetched
    /// at `fetched` and the local file `local` was last modified at
    /// `modified`: the rules, and what the load says about it.
    fn in_effect(local: &str, modified: SystemTime, fetched: SystemTime) -> (Vec<String>, String) {
        let fake = Arc::new(
            Fake::new()
                .text(MANAGED, &copy_profile(&managed_line(MANAGED, 7200, false)))
                .fetched(MANAGED, fetched),
        );
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(dir.path(), local, Some(modified), &fake);
        let rules = rules_of(&l).into_iter().map(str::to_string).collect();
        let said = managed_said(&l);
        assert_eq!(said.len(), 1, "{said:?}");
        let (code, said) = said.into_iter().next().unwrap();
        assert_eq!(code, codes::I_MANAGED_SOURCE);
        (
            rules,
            said.replace(&fake.path_of(MANAGED).display().to_string(), "<copy>"),
        )
    }

    const COPY_IN_EFFECT: &str = "managed profile: the downloaded copy is in effect (`<copy>`)";

    /// A local file that was changed before the copy was fetched has nothing
    /// the copy lacks.
    #[test]
    fn a_copy_fetched_after_the_local_file_was_changed_is_in_effect() {
        let own = local_profile(&managed_line(MANAGED, 3600, false));
        let (rules, said) = in_effect(&own, hours(5), hours(10));
        assert_eq!(rules, COPY_RULES);
        assert_eq!(said, COPY_IN_EFFECT);
    }

    /// The local file wins only when it is strictly later: a file changed at
    /// the very instant of the download cannot be told from it.
    #[test]
    fn a_local_file_changed_at_the_instant_of_the_download_is_not_in_effect() {
        let own = local_profile(&managed_line(MANAGED, 3600, false));
        let (rules, said) = in_effect(&own, hours(5), hours(5));
        assert_eq!(rules, COPY_RULES);
        assert_eq!(said, COPY_IN_EFFECT);
    }

    /// A file dated in the future (a clock that was set back, a file copied
    /// from a machine whose clock is ahead) would keep the copy out for as
    /// long as the date lasts: it is not trusted.
    #[test]
    fn a_local_file_dated_in_the_future_is_not_in_effect() {
        let own = local_profile(&managed_line(MANAGED, 3600, false));
        let tomorrow = SystemTime::now() + Duration::from_secs(24 * 3600);
        let (rules, said) = in_effect(&own, tomorrow, hours(5));
        assert_eq!(rules, COPY_RULES);
        assert_eq!(said, COPY_IN_EFFECT);
    }

    /// A local file that is only the line pointing to the address is no
    /// profile: the copy is, however new the file is.
    #[test]
    fn a_local_file_with_no_section_is_never_in_effect() {
        let pointer = format!("{}\n", managed_line(MANAGED, 3600, false));
        let (rules, said) = in_effect(&pointer, hours(10), hours(5));
        assert_eq!(rules, COPY_RULES);
        assert_eq!(said, COPY_IN_EFFECT);
    }

    /// A copy without the line is in effect and is not updated any more: the
    /// profile became an ordinary one, which the address does not say how to
    /// update. The address is tracked as a stop on the way and nothing more.
    #[test]
    fn a_copy_without_the_line_is_in_effect_and_no_longer_updated() {
        let fake = Arc::new(
            Fake::new()
                .text(
                    MANAGED,
                    "[Rule]\nDOMAIN,copy.example,DIRECT\nFINAL,DIRECT\n",
                )
                .fetched(MANAGED, hours(10)),
        );
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(
            dir.path(),
            &local_profile(&managed_line(MANAGED, 3600, true)),
            Some(hours(5)),
            &fake,
        );
        assert_eq!(rules_of(&l), COPY_RULES);
        assert_eq!(
            managed_said(&l),
            [
                (
                    codes::I_MANAGED_SOURCE,
                    copy_in_effect(&fake.path_of(MANAGED))
                ),
                (codes::W_MANAGED_NOT_UPDATED, NO_LINE.to_string()),
            ]
        );
        assert_eq!(
            placed(&l, &[codes::W_MANAGED_NOT_UPDATED]),
            [("main.conf".to_string(), 1)]
        );
        assert_eq!(
            hint_of(&l, codes::W_MANAGED_NOT_UPDATED),
            [Some(NO_LINE_HINT)]
        );
        assert_eq!(tracked(&l), [on_the_way(MANAGED)]);
        assert!(l.config.managed.is_none());
    }

    /// The same copy, with a local file edited after it was downloaded: the
    /// local file is the profile, its line says the address is the managed one
    /// and how it is updated, and nothing is said of the copy having no line.
    #[test]
    fn a_local_file_newer_than_a_copy_without_the_line_is_the_one_that_is_updated() {
        let fake = Arc::new(
            Fake::new()
                .text(
                    MANAGED,
                    "[Rule]\nDOMAIN,copy.example,DIRECT\nFINAL,DIRECT\n",
                )
                .fetched(MANAGED, hours(5)),
        );
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(
            dir.path(),
            &local_profile(&managed_line(MANAGED, 1800, true)),
            Some(hours(10)),
            &fake,
        );
        assert_eq!(rules_of(&l), LOCAL_RULES);
        assert_eq!(
            managed_said(&l),
            [(codes::I_MANAGED_SOURCE, LOCAL_CHANGED.to_string())]
        );
        assert_eq!(tracked(&l), [updated(MANAGED, true, 1800, true)]);
        assert!(l.config.managed.is_some());
    }

    /// The copy's line names another address, which was never downloaded: the
    /// copy that exists is in effect and the new address is what is updated,
    /// by the line of the copy that names it. The local file still names the
    /// old one, and is told so.
    #[test]
    fn an_address_that_moved_and_was_never_downloaded_keeps_the_old_copy_in_effect() {
        let moved_line = managed_line(MOVED, 7200, true);
        let fake = Arc::new(
            Fake::new()
                .text(MANAGED, &copy_profile(&moved_line))
                .fetched(MANAGED, hours(10))
                .missing(MOVED, Some("timed out")),
        );
        let cached = fake.path_of(MANAGED);
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(
            dir.path(),
            &local_profile(&managed_line(MANAGED, 3600, false)),
            Some(hours(5)),
            &fake,
        );
        assert_eq!(rules_of(&l), COPY_RULES);
        assert_eq!(
            managed_said(&l),
            [
                (
                    codes::W_RESOURCE_UNAVAILABLE,
                    "managed profile not downloaded yet: timed out".to_string()
                ),
                (codes::I_MANAGED_SOURCE, copy_in_effect(&cached)),
                (codes::I_MANAGED_SOURCE, leads_elsewhere(&cached)),
            ]
        );
        assert_eq!(
            tracked(&l),
            [on_the_way(MANAGED), updated(MOVED, false, 7200, true),]
        );
        assert_eq!(fake.asked(), [url(MANAGED), url(MOVED)]);
        // the profile in effect is the copy, with its line
        let managed = l.config.managed.as_ref().expect("the copy's line");
        assert_eq!(managed.url, MOVED);
    }

    /// The same, with the new address downloaded: its copy is the profile, and
    /// the old address is a stop on the way.
    #[test]
    fn the_copy_of_the_address_a_copy_moved_to_is_in_effect() {
        let fake = Arc::new(
            Fake::new()
                .text(MANAGED, &copy_profile(&managed_line(MOVED, 7200, true)))
                .fetched(MANAGED, hours(2))
                .text(
                    MOVED,
                    "#!MANAGED-CONFIG https://example.com/moved.conf interval=1800 strict=false\n[Rule]\nDOMAIN,moved.example,DIRECT\nFINAL,DIRECT\n",
                )
                .fetched(MOVED, hours(8)),
        );
        let (first, last) = (fake.path_of(MANAGED), fake.path_of(MOVED));
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(
            dir.path(),
            &local_profile(&managed_line(MANAGED, 3600, false)),
            Some(hours(5)),
            &fake,
        );
        assert_eq!(
            rules_of(&l),
            ["DOMAIN,moved.example,DIRECT", "FINAL,DIRECT"]
        );
        assert_eq!(
            managed_said(&l),
            [
                (codes::I_MANAGED_SOURCE, copy_in_effect(&last)),
                (codes::I_MANAGED_SOURCE, leads_elsewhere(&first)),
            ]
        );
        assert_eq!(
            tracked(&l),
            [on_the_way(MANAGED), updated(MOVED, true, 1800, false),]
        );
        assert_eq!(l.config.source.main_text, last);
        let managed = l.config.managed.as_ref().expect("the moved copy's line");
        assert_eq!((managed.interval.as_secs(), managed.strict), (1800, false));
    }

    /// A local file newer than the copy of the old address and older than that
    /// of the new one is older than the newest copy on the way, and that is
    /// the one that counts. Edited after all of them it is the profile, and the
    /// address the walk ended at is still the one that is updated, at the pace
    /// of the local line.
    #[test]
    fn the_newest_copy_on_the_way_decides_between_it_and_the_local_file() {
        let fake = Arc::new(
            Fake::new()
                .text(MANAGED, &copy_profile(&managed_line(MOVED, 7200, true)))
                .fetched(MANAGED, hours(2))
                .text(MOVED, &copy_profile(&managed_line(MOVED, 1800, true)))
                .fetched(MOVED, hours(8)),
        );
        let dir = tempfile::tempdir().unwrap();
        let local = local_profile(&managed_line(MANAGED, 3600, false));
        // newer than the first copy and older than the second: a copy is
        let l = load_modified(dir.path(), &local, Some(hours(5)), &fake);
        assert_eq!(rules_of(&l), COPY_RULES);
        // newer than both
        let l = load_modified(dir.path(), &local, Some(hours(10)), &fake);
        assert_eq!(rules_of(&l), LOCAL_RULES);
        assert_eq!(
            managed_said(&l),
            [(codes::I_MANAGED_SOURCE, LOCAL_CHANGED.to_string())]
        );
        assert_eq!(
            tracked(&l),
            [on_the_way(MANAGED), updated(MOVED, true, 3600, false),]
        );
    }

    /// The newest copy on the way is not always the last one: the address the
    /// local file names can have been checked again after the profile moved on.
    /// A local file changed before that check is older than the last update,
    /// though it is newer than the copy that is in effect.
    #[test]
    fn the_newest_copy_on_the_way_need_not_be_the_last_one() {
        let fake = Arc::new(
            Fake::new()
                .text(MANAGED, &copy_profile(&managed_line(MOVED, 7200, true)))
                .fetched(MANAGED, hours(8))
                .text(MOVED, &copy_profile(&managed_line(MOVED, 1800, true)))
                .fetched(MOVED, hours(2)),
        );
        let dir = tempfile::tempdir().unwrap();
        let local = local_profile(&managed_line(MANAGED, 3600, false));
        // newer than the last copy and older than the first: a copy is
        let l = load_modified(dir.path(), &local, Some(hours(5)), &fake);
        assert_eq!(rules_of(&l), COPY_RULES);
        // newer than both
        let l = load_modified(dir.path(), &local, Some(hours(10)), &fake);
        assert_eq!(rules_of(&l), LOCAL_RULES);
    }

    /// The local file edited after the last update stays the profile whatever
    /// the walk came to. The address it ended at is still the one that is
    /// updated, at the pace and with the strictness of the local line, and what
    /// is said is what holds: that the address was not downloaded, not that the
    /// copy is in effect or has no way out.
    #[test]
    fn a_local_file_in_effect_updates_the_address_the_walk_ended_at() {
        let dir = tempfile::tempdir().unwrap();
        let local = local_profile(&managed_line(MANAGED, 3600, true));
        // an address that moved and was never downloaded
        let fake = Arc::new(
            Fake::new()
                .text(MANAGED, &copy_profile(&managed_line(MOVED, 7200, false)))
                .fetched(MANAGED, hours(5))
                .missing(MOVED, Some("timed out")),
        );
        let l = load_modified(dir.path(), &local, Some(hours(10)), &fake);
        assert_eq!(rules_of(&l), LOCAL_RULES);
        assert_eq!(
            managed_said(&l),
            [
                (
                    codes::W_RESOURCE_UNAVAILABLE,
                    "managed profile not downloaded yet: timed out".to_string()
                ),
                (codes::I_MANAGED_SOURCE, LOCAL_CHANGED.to_string()),
            ]
        );
        assert_eq!(
            tracked(&l),
            [on_the_way(MANAGED), updated(MOVED, false, 3600, true),]
        );
        // addresses that lead back to each other
        let fake = Arc::new(
            Fake::new()
                .text(MANAGED, &copy_profile(&managed_line(MOVED, 7200, false)))
                .fetched(MANAGED, hours(5))
                .text(MOVED, &copy_profile(&managed_line(MANAGED, 900, false)))
                .fetched(MOVED, hours(5)),
        );
        let l = load_modified(dir.path(), &local, Some(hours(10)), &fake);
        assert_eq!(rules_of(&l), LOCAL_RULES);
        assert_eq!(
            managed_said(&l),
            [(codes::I_MANAGED_SOURCE, LOCAL_CHANGED.to_string())]
        );
        assert_eq!(
            tracked(&l),
            [on_the_way(MANAGED), updated(MOVED, true, 3600, true),]
        );
    }

    /// Two addresses that name each other lead nowhere: the last copy found
    /// is in effect, and the local file is told to name the address the
    /// profile is finally served from.
    #[test]
    fn addresses_that_lead_back_to_each_other_have_no_way_out() {
        let fake = Arc::new(
            Fake::new()
                .text(MANAGED, &copy_profile(&managed_line(MOVED, 7200, false)))
                .fetched(MANAGED, hours(10))
                .text(MOVED, &copy_profile(&managed_line(MANAGED, 900, true)))
                .fetched(MOVED, hours(10)),
        );
        let (first, last) = (fake.path_of(MANAGED), fake.path_of(MOVED));
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(
            dir.path(),
            &local_profile(&managed_line(MANAGED, 3600, false)),
            Some(hours(5)),
            &fake,
        );
        assert_eq!(rules_of(&l), COPY_RULES);
        assert_eq!(
            managed_said(&l),
            [
                (codes::I_MANAGED_SOURCE, copy_in_effect(&last)),
                (codes::I_MANAGED_SOURCE, leads_elsewhere(&first)),
                (codes::W_MANAGED_NOT_UPDATED, NO_WAY_OUT.to_string()),
            ]
        );
        assert_eq!(
            hint_of(&l, codes::W_MANAGED_NOT_UPDATED),
            [Some(NO_WAY_OUT_HINT)]
        );
        // the last address is the one updated, by its own line
        assert_eq!(
            tracked(&l),
            [on_the_way(MANAGED), updated(MOVED, true, 900, true),]
        );
        // the first address is not asked for again
        assert_eq!(fake.asked(), [url(MANAGED), url(MOVED)]);
    }

    /// A chain of ten addresses is followed through eight changes of address:
    /// the copies of the first nine are read, the ninth is the last, and the
    /// tenth is never asked for.
    #[test]
    fn a_chain_of_ten_addresses_stops_at_the_eighth_change() {
        let address = |n: u64| format!("https://example.com/hop{n}.conf");
        let mut fake = Fake::new();
        for n in 0..10 {
            fake = fake
                .text(
                    &address(n),
                    &copy_profile(&managed_line(&address(n + 1), 600 + n, false)),
                )
                .fetched(&address(n), hours(10));
        }
        let fake = Arc::new(fake);
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(
            dir.path(),
            &local_profile(&managed_line(&address(0), 3600, false)),
            Some(hours(5)),
            &fake,
        );
        let asked: Vec<Url> = (0..9).map(|n| url(&address(n))).collect();
        assert_eq!(fake.asked(), asked);
        let mut expected: Vec<_> = (0..8).map(|n| on_the_way(&address(n))).collect();
        expected.push(updated(&address(8), true, 608, false));
        assert_eq!(tracked(&l), expected);
        assert_eq!(
            managed_said(&l),
            [
                (
                    codes::I_MANAGED_SOURCE,
                    copy_in_effect(&fake.path_of(&address(8)))
                ),
                (
                    codes::I_MANAGED_SOURCE,
                    leads_elsewhere(&fake.path_of(&address(0)))
                ),
                (codes::W_MANAGED_NOT_UPDATED, NO_WAY_OUT.to_string()),
            ]
        );
    }

    /// A fragment is never sent to a server: an address with one is the
    /// address without it. A copy whose line names its own address with a
    /// fragment added has not moved, and the address is tracked without the
    /// fragment, whichever line wrote it.
    #[test]
    fn a_fragment_does_not_make_an_address_another_one() {
        let local = local_profile(&format!(
            "#!MANAGED-CONFIG {MANAGED}#local interval=3600 strict=false"
        ));
        let copy = copy_profile(&format!(
            "#!MANAGED-CONFIG {MANAGED}#copy interval=7200 strict=true"
        ));
        let fake = Arc::new(Fake::new().text(MANAGED, &copy).fetched(MANAGED, hours(10)));
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(dir.path(), &local, Some(hours(5)), &fake);
        assert_eq!(rules_of(&l), COPY_RULES);
        assert_eq!(fake.asked(), [url(MANAGED)]);
        assert_eq!(tracked(&l), [updated(MANAGED, true, 7200, true)]);
        assert_eq!(
            managed_said(&l),
            [(
                codes::I_MANAGED_SOURCE,
                copy_in_effect(&fake.path_of(MANAGED))
            )]
        );
    }

    /// The addresses a remote include of the copy in effect names are asked for
    /// once, like the ones the walk was at: here the copy of the new address
    /// includes the text of the old one. The address keeps the role the walk
    /// gave it, and what the include found out about it lands on that item.
    #[test]
    fn an_address_the_walk_was_at_is_asked_for_once_per_load() {
        let fake = Arc::new(
            Fake::new()
                .text(MANAGED, &copy_profile(&managed_line(MOVED, 600, false)))
                .fetched(MANAGED, hours(5))
                .text(
                    MOVED,
                    &format!(
                        "{}\n[Proxy]\n#!include {MANAGED}\n[Rule]\nDOMAIN,moved.example,DIRECT\nFINAL,DIRECT\n",
                        managed_line(MOVED, 900, false)
                    ),
                )
                .fetched(MOVED, hours(5)),
        );
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(
            dir.path(),
            &local_profile(&managed_line(MANAGED, 3600, false)),
            Some(hours(1)),
            &fake,
        );
        assert_eq!(fake.asked(), [url(MANAGED), url(MOVED)]);
        assert_eq!(
            tracked(&l),
            [on_the_way(MANAGED), updated(MOVED, true, 900, false),]
        );
        // the old address's text has no [Proxy] section for the line to take
        let lacks: Vec<bool> = l
            .config
            .source
            .remote
            .iter()
            .map(|r| r.lacks_section)
            .collect();
        assert_eq!(lacks, [true, false]);
        assert_eq!(
            placed(&l, &[codes::W_INCLUDE_SECTION_MISSING]),
            [("2.conf".to_string(), 3)]
        );
    }

    /// The copy of a managed profile is the profile while it is in effect,
    /// and it is downloaded text all the same: it includes no local file of
    /// the directory of the local file, whatever it does with what it would
    /// have taken in. The proxies of `other.conf`, which is there, do not
    /// become policies of the copy nor members of its group, and its rules
    /// are not the copy's. The program of an `external` policy is not
    /// started.
    #[test]
    fn a_copy_takes_in_nothing_of_the_directory_of_the_local_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("other.conf"),
            "[Proxy]\nElsewhere = http, 127.0.0.1, 8080\n[Rule]\nDOMAIN,other.example,DIRECT\n",
        )
        .unwrap();
        let copy = format!(
            "{}\n[Proxy]\n#!include other.conf\nOwn = external, exec=/bin/true, local-port=1082\n[Proxy Group]\nAll = select, include-all-proxies=true\n[Rule]\n#!include other.conf\nFINAL,All\n",
            managed_line(MANAGED, 3600, false)
        );
        let l = load_copy(dir.path(), &copy);
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.clone().into_vec()
        );
        // the two includes, and the program at its own line
        let mut expected = refused("1.conf", &[3, 8], INCLUDE_IN_COPY, INCLUDE_IN_COPY_HINT);
        expected.extend(refused(
            "1.conf",
            &[4],
            EXTERNAL_IN_COPY,
            EXTERNAL_IN_COPY_HINT,
        ));
        expected.sort_by_key(|(file, line, ..)| (file.clone(), *line));
        assert_eq!(refusals(&l), expected);
        let config = &l.config;
        let names: Vec<&str> = config.policies.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Own"]);
        assert!(config.spec("Elsewhere").is_none());
        assert_eq!(rules_of(&l), ["FINAL,All"]);
        assert!(config.source.includes.is_empty());
        assert!(config.spec("Own").is_none());
        assert_eq!(
            config.not_implemented.get("Own"),
            Some(&NotImplemented::ManagedExternal)
        );
    }

    /// A text the copy reaches through a remote include is downloaded text of
    /// the other kind: expanded, and not allowed to include a local file.
    #[test]
    fn a_remote_include_in_a_copy_is_expanded_and_includes_no_local_file() {
        let remote = "https://example.com/rules.conf";
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("shared.dconf"),
            "[Rule]\nDOMAIN,shared.example,DIRECT\n",
        )
        .unwrap();
        let fake = Arc::new(
            Fake::new()
                .text(
                    MANAGED,
                    &format!(
                        "{}\n[Rule]\n#!include {remote}\nFINAL,DIRECT\n",
                        managed_line(MANAGED, 3600, false)
                    ),
                )
                .fetched(MANAGED, hours(10))
                .text(
                    remote,
                    "[Rule]\nDOMAIN,remote.example,DIRECT\n#!include shared.dconf\n",
                ),
        );
        let l = load_modified(
            dir.path(),
            &local_profile(&managed_line(MANAGED, 3600, false)),
            Some(hours(5)),
            &fake,
        );
        assert_eq!(
            rules_of(&l),
            ["DOMAIN,remote.example,DIRECT", "FINAL,DIRECT"]
        );
        assert_eq!(
            placed(&l, &[codes::W_REMOTE_LOCAL_ACCESS]),
            [("2.conf".to_string(), 3)]
        );
        assert!(l.config.source.includes.is_empty());
        assert_eq!(
            tracked(&l),
            [
                updated(MANAGED, true, 3600, false),
                (remote.to_string(), RemoteRole::Include, true, 86400, false),
            ]
        );
    }

    // What downloaded text may name on this machine: nothing. The paths such
    // text names carry a marker, so that a search of what a load says finds a
    // path that got into a message. What is said of a line, and its hint:
    // `…_IN_DOWNLOADED` of a downloaded file's, `…_IN_COPY` of a line of the
    // copy of a managed profile.

    const PATH_MARK: &str = "marker-path";
    const SET_IN_DOWNLOADED: &str =
        "a downloaded file cannot name a local file as a set; the line is ignored";
    const SET_IN_DOWNLOADED_HINT: &str = "write the line in the local profile to use a local set";
    const SET_IN_COPY: &str = "the downloaded copy of a managed profile cannot name a local file as a set; the line is ignored";
    const SET_IN_COPY_HINT: &str = "name the set by a URL instead";
    const INCLUDE_IN_COPY: &str =
        "the downloaded copy of a managed profile cannot include a local file; the line is ignored";
    const INCLUDE_IN_COPY_HINT: &str = "to include a local file, load a profile without `#!MANAGED-CONFIG` that takes the downloaded parts in by `#!include`";
    const POLICY_PATH_IN_DOWNLOADED: &str = "policy group `G`: a downloaded file cannot name a local file as `policy-path`; the parameter is ignored";
    const POLICY_PATH_IN_DOWNLOADED_HINT: &str =
        "define the group in the local profile to import policies from a local file";
    const POLICY_PATH_IN_COPY: &str = "policy group `G`: the downloaded copy of a managed profile cannot name a local file as `policy-path`; the parameter is ignored";
    const POLICY_PATH_IN_COPY_HINT: &str = "give `policy-path` a URL instead";
    const EXTERNAL_IN_COPY: &str = "`external` policies in the downloaded copy of a managed profile are not started; they behave as REJECT";
    const EXTERNAL_IN_COPY_HINT: &str = "to use an `external` policy, load a profile without `#!MANAGED-CONFIG` that takes the downloaded parts in by `#!include`";

    /// What a load said of lines their text may not write (`W0033`), by file
    /// and line: the name of the file, the line, the message and the hint.
    fn refusals(l: &Loaded) -> Vec<(String, u32, String, Option<&'static str>)> {
        let mut found: Vec<_> = l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_REMOTE_LOCAL_ACCESS)
            .map(|d| {
                let span = d.span.as_ref().expect("a position");
                let file = span.file.file_name().unwrap().to_string_lossy();
                (file.into_owned(), span.line, d.message.clone(), d.hint)
            })
            .collect();
        found.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
        found
    }

    /// `refusals` for the lines of one file that all got the same answer.
    fn refused(
        file: &str,
        lines: &[u32],
        message: &str,
        hint: &'static str,
    ) -> Vec<(String, u32, String, Option<&'static str>)> {
        lines
            .iter()
            .map(|line| (file.to_string(), *line, message.to_string(), Some(hint)))
            .collect()
    }

    /// No message and no hint of the load holds a path that a text named.
    fn assert_no_path_is_shown(l: &Loaded) {
        for d in l.diagnostics.iter() {
            for shown in [d.to_string(), format!("{d:?}")] {
                assert!(!shown.contains(PATH_MARK), "{shown}");
            }
        }
    }

    /// A load in which the downloaded copy of the managed profile, `copy`, is
    /// in effect; its cache file is `1.conf`.
    fn load_copy(dir: &Path, copy: &str) -> Loaded {
        let fake = Arc::new(Fake::new().text(MANAGED, copy).fetched(MANAGED, hours(10)));
        load_modified(
            dir,
            &local_profile(&managed_line(MANAGED, 3600, false)),
            Some(hours(5)),
            &fake,
        )
    }

    /// A downloaded text cannot name a local file as a set: a `[Rule]` line
    /// that does is ignored whole, at any depth of a logical rule (a set that
    /// matched nothing would make a `NOT` match everything), and the lines
    /// that name a URL, a built-in set or an inline set are kept.
    #[test]
    fn a_remote_text_cannot_name_a_local_file_as_a_set_in_a_rule() {
        let link = "https://example.com/rules.conf";
        let remote = format!(
            "[Rule]\nRULE-SET,sets/{PATH_MARK}-a.list,DIRECT\nDOMAIN-SET,/abs/{PATH_MARK}-b.list,DIRECT\n\
AND,((RULE-SET,{PATH_MARK}-c.list),(DEST-PORT,443)),DIRECT\nNOT,((RULE-SET,{PATH_MARK}-d.list)),DIRECT\n\
RULE-SET,https://example.com/e.list,DIRECT\nRULE-SET,LAN,DIRECT\nRULE-SET,Inline,DIRECT\n"
        );
        let fake = Arc::new(Fake::new().text(link, &remote));
        let dir = tempfile::tempdir().unwrap();
        let l = load_with_remote(
            dir.path(),
            &format!(
                "[Rule]\n#!include {link}\nFINAL,DIRECT\n[Ruleset Inline]\nDOMAIN,inline.example\n"
            ),
            &fake,
        );
        assert_eq!(
            refusals(&l),
            refused(
                "1.conf",
                &[2, 3, 4, 5],
                SET_IN_DOWNLOADED,
                SET_IN_DOWNLOADED_HINT
            )
        );
        // nothing else is said, and the load has no error
        assert_eq!(
            l.diagnostics.len(),
            4,
            "{:?}",
            l.diagnostics.clone().into_vec()
        );
        assert_eq!(
            rules_of(&l),
            [
                "RULE-SET,https://example.com/e.list,DIRECT",
                "RULE-SET,LAN,DIRECT",
                "RULE-SET,Inline,DIRECT",
                "FINAL,DIRECT"
            ]
        );
        assert_no_path_is_shown(&l);
    }

    /// The same for the keys of `[Host]`: the line is ignored.
    #[test]
    fn a_remote_text_cannot_name_a_local_file_as_a_set_in_host() {
        let link = "https://example.com/hosts.conf";
        let fake = Arc::new(Fake::new().text(
            link,
            &format!(
                "[Host]\nDOMAIN-SET:{PATH_MARK}-a.list = 10.0.0.1\nRULE-SET:/abs/{PATH_MARK}-b.list = 10.0.0.2\n\
DOMAIN-SET:https://example.com/h.list = 10.0.0.3\n"
            ),
        ));
        let dir = tempfile::tempdir().unwrap();
        let l = load_with_remote(
            dir.path(),
            &format!("[Host]\n#!include {link}\n[Rule]\nFINAL,DIRECT\n"),
            &fake,
        );
        assert_eq!(
            refusals(&l),
            refused("1.conf", &[2, 3], SET_IN_DOWNLOADED, SET_IN_DOWNLOADED_HINT)
        );
        assert_eq!(
            l.diagnostics.len(),
            2,
            "{:?}",
            l.diagnostics.clone().into_vec()
        );
        assert!(
            matches!(
                l.config.hosts.as_slice(),
                [HostEntry { key: HostKey::DomainSet(ResourceRef::Url(url)), .. }]
                    if url == "https://example.com/h.list"
            ),
            "{:?}",
            l.config.hosts
        );
        assert_no_path_is_shown(&l);
    }

    /// A line that an inline set takes from a downloaded text and that names
    /// a local file is skipped, as a line of an inline set that cannot be
    /// used is: the warning of the section, with the reason.
    #[test]
    fn a_remote_text_cannot_name_a_local_file_as_a_set_in_an_inline_set() {
        let link = "https://example.com/inline.conf";
        let fake = Arc::new(Fake::new().text(
            link,
            &format!(
                "[Ruleset Inline]\nRULE-SET,{PATH_MARK}-local.list\nDOMAIN-SUFFIX,example.com\n"
            ),
        ));
        let dir = tempfile::tempdir().unwrap();
        let l = load_with_remote(
            dir.path(),
            &format!(
                "[Ruleset Inline]\n#!include {link}\n[Rule]\nRULE-SET,Inline,DIRECT\nFINAL,DIRECT\n"
            ),
            &fake,
        );
        let said: Vec<(&str, &str, String, u32)> = l
            .diagnostics
            .iter()
            .map(|d| {
                let span = d.span.as_ref().expect("a position");
                (
                    d.code,
                    d.message.as_str(),
                    span.file
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    span.line,
                )
            })
            .collect();
        assert_eq!(
            said,
            [(
                codes::W_RULESET_LINE_SKIPPED,
                "[Ruleset Inline]: line skipped: a downloaded file cannot name a local file as a set",
                "1.conf".to_string(),
                2
            )]
        );
        let kept: Vec<&str> = l.config.rulesets[0]
            .rules
            .iter()
            .map(|r| r.raw.as_str())
            .collect();
        assert_eq!(kept, ["DOMAIN-SUFFIX,example.com"]);
        assert_no_path_is_shown(&l);
    }

    /// A group a downloaded text defines is kept, with the members it names,
    /// but its `policy-path` is not read when it is a local file: the
    /// parameter is ignored, and that is no error. A URL is kept, and so is a
    /// local `policy-path` of the user's own group.
    #[test]
    fn a_remote_text_cannot_name_a_local_file_as_a_policy_path() {
        let link = "https://example.com/groups.conf";
        let fake = Arc::new(Fake::new().text(
            link,
            &format!(
                "[Proxy Group]\nG = select, A, policy-path={PATH_MARK}-nodes.txt\nH = select, A, policy-path=https://example.com/nodes.txt\n"
            ),
        ));
        let dir = tempfile::tempdir().unwrap();
        let l = load_with_remote(
            dir.path(),
            &format!(
                "[Proxy]\nA = http, a.example, 8080\n[Proxy Group]\n#!include {link}\nL = select, A, policy-path={PATH_MARK}-own.txt\n[Rule]\nFINAL,A\n"
            ),
            &fake,
        );
        assert_eq!(
            refusals(&l),
            refused(
                "1.conf",
                &[2],
                POLICY_PATH_IN_DOWNLOADED,
                POLICY_PATH_IN_DOWNLOADED_HINT
            )
        );
        assert_eq!(
            l.diagnostics.len(),
            1,
            "{:?}",
            l.diagnostics.clone().into_vec()
        );
        let spec = |name: &str| {
            l.config
                .group_specs
                .iter()
                .find(|g| g.name == name)
                .unwrap_or_else(|| panic!("the spec of {name}"))
        };
        // the group stands, with the member it names
        assert_eq!(spec("G").members, ["A"]);
        assert_eq!(spec("G").import.policy_path, None);
        assert!(matches!(
            spec("H").import.policy_path,
            Some(PolicyPath::Url(_))
        ));
        assert_eq!(
            spec("L").import.policy_path,
            Some(PolicyPath::File(
                dir.path().join(format!("{PATH_MARK}-own.txt"))
            ))
        );
        assert_no_path_is_shown(&l);
    }

    /// The downloaded copy of a managed profile in effect names no local
    /// file: not as an include, by whatever path, not as a set, not as a
    /// `policy-path`. What it ignores it leaves unread: the file a plain
    /// relative path names is there, and none of its rules is the copy's, and
    /// every other target of an include is a file that is not there, so that
    /// a read would be an error of its own.
    #[test]
    fn the_copy_of_a_managed_profile_names_no_local_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sets")).unwrap();
        std::fs::write(
            dir.path().join("sets").join("ok.dconf"),
            "[Rule]\nDOMAIN,included.example,DIRECT\n",
        )
        .unwrap();
        let absent = dir.path().join(format!("{PATH_MARK}-absent.dconf"));
        let copy = format!(
            "{}\n[Proxy]\nA = http, a.example, 8080\n[Proxy Group]\nG = select, A, policy-path={PATH_MARK}-nodes.txt\n\
H = select, A, policy-path=https://example.com/nodes.txt\n[Host]\nDOMAIN-SET:{PATH_MARK}-hosts.list = 10.0.0.1\n\
DOMAIN-SET:https://example.com/hosts.list = 10.0.0.2\n[Rule]\n#!include sets/ok.dconf\n#!include {}\n\
#!include ../{PATH_MARK}-up.dconf\n#!include C:\\{PATH_MARK}-x.dconf\n#!include \\\\127.0.0.1\\share\\{PATH_MARK}-x.dconf\n\
#!include nul\nRULE-SET,sets/{PATH_MARK}-a.list,DIRECT\nRULE-SET,/abs/{PATH_MARK}-a.list,DIRECT\n\
DOMAIN-SET,../{PATH_MARK}-up.list,DIRECT\nRULE-SET,https://example.com/e.list,DIRECT\nFINAL,DIRECT\n",
            managed_line(MANAGED, 3600, false),
            absent.display()
        );
        let l = load_copy(dir.path(), &copy);
        let mut expected = refused(
            "1.conf",
            &[5],
            POLICY_PATH_IN_COPY,
            POLICY_PATH_IN_COPY_HINT,
        );
        expected.extend(refused(
            "1.conf",
            &[8, 17, 18, 19],
            SET_IN_COPY,
            SET_IN_COPY_HINT,
        ));
        expected.extend(refused(
            "1.conf",
            &[11, 12, 13, 14, 15, 16],
            INCLUDE_IN_COPY,
            INCLUDE_IN_COPY_HINT,
        ));
        expected.sort_by_key(|(file, line, ..)| (file.clone(), *line));
        assert_eq!(refusals(&l), expected);
        // and nothing else but where the text comes from: no include was
        // read, so none is an error
        assert_eq!(
            managed_said(&l),
            [(
                codes::I_MANAGED_SOURCE,
                copy_in_effect(Path::new("/cache/1.conf"))
            )]
        );
        assert_eq!(l.diagnostics.len(), expected.len() + 1);
        assert_eq!(
            rules_of(&l),
            ["RULE-SET,https://example.com/e.list,DIRECT", "FINAL,DIRECT"]
        );
        assert!(l.config.source.includes.is_empty());
        assert_eq!(l.config.hosts.len(), 1);
        assert!(matches!(
            &l.config.hosts[0].key,
            HostKey::DomainSet(ResourceRef::Url(_))
        ));
        let spec = |name: &str| {
            l.config
                .group_specs
                .iter()
                .find(|g| g.name == name)
                .unwrap_or_else(|| panic!("the spec of {name}"))
        };
        assert_eq!(spec("G").import.policy_path, None);
        assert!(matches!(
            spec("H").import.policy_path,
            Some(PolicyPath::Url(_))
        ));
        assert_no_path_is_shown(&l);
    }

    /// What the user wrote is read as it always was, wherever it names a
    /// file: an include by an absolute path, sets by an absolute path and
    /// by `..`, a `[Host]` set, a `policy-path`, and a program that is
    /// started. The same lines in the profile and in a file it includes.
    fn local_text(dir: &Path, first_line: &str) -> String {
        let path = |name: &str| dir.join(name).display().to_string();
        std::fs::write(
            dir.join("absolute.dconf"),
            "[Rule]\nDOMAIN,absolute.example,DIRECT\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("inc.dconf"),
            format!(
                "[Proxy]\nInc = external, exec=/bin/true, local-port=1083\n[Proxy Group]\nIncG = select, Inc, policy-path={}\n\
[Host]\nDOMAIN-SET:{} = 10.0.0.1\n[Rule]\n#!include {}\nRULE-SET,{},DIRECT\nDOMAIN-SET,../inc-outside.list,DIRECT\n",
                path("inc-nodes.txt"),
                path("inc-hosts.list"),
                path("absolute.dconf"),
                path("inc-set.list")
            ),
        )
        .unwrap();
        format!(
            "{first_line}\n[Proxy]\n#!include inc.dconf\nOwn = external, exec=/bin/true, local-port=1082\n\
[Proxy Group]\nOwnG = select, Own, policy-path={}\n#!include inc.dconf\n[Host]\nDOMAIN-SET:{} = 10.0.0.1\n#!include inc.dconf\n\
[Rule]\n#!include {}\nRULE-SET,{},DIRECT\nDOMAIN-SET,../outside.list,DIRECT\n#!include inc.dconf\nFINAL,DIRECT\n",
            path("nodes.txt"),
            path("hosts.list"),
            path("absolute.dconf"),
            path("set.list")
        )
    }

    /// What `local_text` says is read: nothing is refused, every path stands
    /// for the file it names, and the programs are started.
    fn assert_local_text_is_read_as_written(l: &Loaded, dir: &Path) {
        // the one thing a load says of a managed profile in effect is where
        // the text comes from
        let said: Vec<_> = l
            .diagnostics
            .iter()
            .filter(|d| d.code != codes::I_MANAGED_SOURCE)
            .collect();
        assert!(said.is_empty(), "{said:?}");
        let config = &l.config;
        for name in ["Own", "Inc"] {
            assert!(config.spec(name).is_some(), "{name}");
        }
        assert!(config.not_implemented.is_empty());
        let sets: Vec<PathBuf> = config
            .rules
            .iter()
            .filter_map(|rule| match &rule.kind {
                RuleKind::RuleSet(ResourceRef::File(path))
                | RuleKind::DomainSet(ResourceRef::File(path)) => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            sets,
            [
                dir.join("set.list"),
                dir.join("../outside.list"),
                dir.join("inc-set.list"),
                dir.join("../inc-outside.list")
            ]
        );
        let hosts: Vec<PathBuf> = config
            .hosts
            .iter()
            .filter_map(|host| match &host.key {
                HostKey::DomainSet(ResourceRef::File(path)) => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(hosts, [dir.join("hosts.list"), dir.join("inc-hosts.list")]);
        let paths: Vec<(&str, Option<PolicyPath>)> = config
            .group_specs
            .iter()
            .map(|g| (g.name.as_str(), g.import.policy_path.clone()))
            .collect();
        assert_eq!(
            paths,
            [
                ("OwnG", Some(PolicyPath::File(dir.join("nodes.txt")))),
                ("IncG", Some(PolicyPath::File(dir.join("inc-nodes.txt"))))
            ]
        );
        assert_eq!(
            config.source.includes,
            [dir.join("inc.dconf"), dir.join("absolute.dconf")]
        );
    }

    /// The pin for all of the above: text that is the user's own is read as
    /// it was, in the profile and in the files it includes, and so is a
    /// local file with a `#!MANAGED-CONFIG` line while it is the text in
    /// effect.
    #[test]
    fn local_text_names_any_local_file_and_starts_its_programs() {
        let dir = tempfile::tempdir().unwrap();
        let text = local_text(dir.path(), "");
        let l = load_with_remote(dir.path(), &text, &Arc::new(Fake::new()));
        assert_local_text_is_read_as_written(&l, dir.path());
        assert!(l.diagnostics.is_empty());

        // the local file is newer than the copy: it is the profile
        let dir = tempfile::tempdir().unwrap();
        let text = local_text(dir.path(), &managed_line(MANAGED, 3600, false));
        let fake = Arc::new(
            Fake::new()
                .text(MANAGED, &copy_profile(&managed_line(MANAGED, 3600, false)))
                .fetched(MANAGED, hours(5)),
        );
        let l = load_modified(dir.path(), &text, Some(hours(10)), &fake);
        assert_eq!(l.config.source.main_text, l.config.source.main);
        assert_local_text_is_read_as_written(&l, dir.path());
    }

    /// A captive portal or an error page that answered with a 200 is a text
    /// that does not load. Whether it is adopted is not the loader's to say;
    /// the loader says what it is: no rules.
    #[test]
    fn a_page_that_is_no_profile_does_not_load_as_one() {
        let fake = Arc::new(
            Fake::new()
                .text(
                    MANAGED,
                    "<html><body>Sign in to the network</body></html>\n",
                )
                .fetched(MANAGED, hours(10)),
        );
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(
            dir.path(),
            &format!("{}\n", managed_line(MANAGED, 3600, false)),
            Some(hours(5)),
            &fake,
        );
        // the page is what was read: its line stands outside any section, in
        // the file that holds it
        assert_eq!(
            placed(&l, &[codes::W_LINE_OUTSIDE_SECTION]),
            [("1.conf".to_string(), 1)]
        );
        assert!(l.diagnostics.has_errors());
        assert!(codes_of(&l).contains(&codes::E_MISSING_FINAL));
        assert!(l.config.rules.is_empty());
    }

    /// `from_text` has no file: it reads the text it is given, whatever its
    /// first line says, and asks for no download.
    #[test]
    fn from_text_reads_the_text_it_is_given() {
        let fake = Arc::new(
            Fake::new()
                .text(MANAGED, &copy_profile(&managed_line(MANAGED, 7200, true)))
                .fetched(MANAGED, hours(10)),
        );
        let mut opts = LoadOptions::for_tests();
        opts.remote = Some(fake.clone());
        let l = from_text(
            &local_profile(&managed_line(MANAGED, 3600, false)),
            Path::new("/profiles/t.conf"),
            &opts,
        );
        assert_eq!(rules_of(&l), LOCAL_RULES);
        assert!(l.config.source.remote.is_empty());
        assert_eq!(l.config.source.main_text, l.config.source.main);
        assert!(managed_said(&l).is_empty());
        assert!(fake.asked().is_empty());
        let managed = l.config.managed.as_ref().expect("the line is read");
        assert_eq!((managed.interval.as_secs(), managed.strict), (3600, false));
    }

    /// The floor of the interval holds for the line of the profile in effect,
    /// wherever it is: it is said at the line in the file that holds it, and a
    /// local file that is not in effect says nothing.
    #[test]
    fn the_shortest_interval_is_said_for_the_line_in_effect_only() {
        let message = "interval below 60 seconds; 60 is used";
        let fake = Arc::new(Fake::new().missing(MANAGED, None));
        let dir = tempfile::tempdir().unwrap();
        let l = load_modified(
            dir.path(),
            &local_profile(&managed_line(MANAGED, 10, false)),
            Some(hours(5)),
            &fake,
        );
        let said: Vec<&str> = l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_INVALID_VALUE)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(said, [message]);
        assert_eq!(tracked(&l)[0].3, 60);
        let fake = Arc::new(
            Fake::new()
                .text(MANAGED, &copy_profile(&managed_line(MANAGED, 30, false)))
                .fetched(MANAGED, hours(10)),
        );
        let l = load_modified(
            dir.path(),
            &local_profile(&managed_line(MANAGED, 10, false)),
            Some(hours(5)),
            &fake,
        );
        assert_eq!(
            placed(&l, &[codes::W_INVALID_VALUE]),
            [("1.conf".to_string(), 1)]
        );
        assert_eq!(tracked(&l)[0].3, 60);
    }

    /// The summary shows where a managed profile comes from and no more: its
    /// path and query can be the token a provider issued.
    #[test]
    fn the_summary_shows_where_a_managed_profile_comes_from_and_no_more() {
        let l = load_text(
            "#!MANAGED-CONFIG https://al1ce:pw@sub.example:8443/s/t0k3n?x=y#f interval=3600\n[Rule]\nFINAL,DIRECT\n",
        );
        assert_eq!(
            l.config.summary().managed.as_deref(),
            Some("https://sub.example:8443/\u{2026}")
        );
        let l = load_text("[Rule]\nFINAL,DIRECT\n");
        assert_eq!(l.config.summary().managed, None);
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
