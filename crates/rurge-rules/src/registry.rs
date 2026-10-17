//! Set registry (M2 design §6.2): one `SetHandle` per (resource, kind).
//! Internal and inline sets compile once; external sets come from the
//! `ResourceManager` and are recompiled and swapped whenever the resource
//! changes. Nested references are resolved through the same registry with
//! cycle detection and a nesting limit of 8. A file or URL is read in one
//! format only: the one the profile itself gives it. A reference to it as the
//! other kind of set from inside a set file is left out with a warning.

use crate::matcher::{SetLookup, SetRef};
use crate::set::{CompiledSet, SetHandle};
use crate::set_format::{ParsedSet, SetKind, SetLine, internal_set_source, parse_set};
use rurge_config::rule::{InternalSet, ParseCtx, ResourceRef, RuleKind, SubRule};
use rurge_config::{Config, Diagnostic, Diagnostics, HostKey, Reach, codes};
use rurge_net::resource::{
    ResourceHandle, ResourceKind, ResourceManager, ResourceSource, ResourceSpec, ResourceState,
    resource_key,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::watch;
use url::Url;

pub const MAX_NESTING: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Internal(InternalSet),
    Inline(String),
    Url(String),
    File(PathBuf),
}

impl Key {
    fn from_ref(r: &ResourceRef) -> Key {
        match r {
            ResourceRef::Internal(i) => Key::Internal(*i),
            ResourceRef::Inline(n) => Key::Inline(n.clone()),
            ResourceRef::Url(u) => Key::Url(u.clone()),
            ResourceRef::File(p) => Key::File(p.clone()),
        }
    }

    /// What the text of the set may name on this machine. A set that was
    /// downloaded is not the user's own text, so it names nothing here; a
    /// file is read because text that may name one named it.
    fn reach(&self) -> Reach {
        match self {
            Key::Url(_) => Reach::Nowhere,
            Key::Internal(_) | Key::Inline(_) | Key::File(_) => Reach::Anywhere,
        }
    }

    /// What the log lines, the diagnostics, the statuses and a match call the
    /// set. The URL of a set can carry a user name and a password: they are
    /// `***` here, the path and the query stay.
    fn display(&self) -> String {
        match self {
            Key::Internal(InternalSet::System) => "SYSTEM".to_string(),
            Key::Internal(InternalSet::Lan) => "LAN".to_string(),
            Key::Inline(n) => n.clone(),
            Key::Url(u) => ResourceRef::shown_url(u),
            Key::File(p) => p.display().to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SetStatus {
    pub name: String,
    /// `rurge_net::resource::resource_key` of the URL or the file the set is
    /// read from, the key its resource goes by in the manager; `None` for a
    /// built-in or an inline set, which have no resource.
    pub key: Option<String>,
    pub kind: SetKind,
    pub entries: usize,
    pub version: u64,
    /// `internal` | `inline` | `ok` | `missing` | `failed: <error>` | `cycle`
    pub state: String,
    /// What the last load of the set's file could not use, all zero while the
    /// set has not loaded: the lines it skipped, the valid lines beyond the
    /// entry limit, and the rule lines whose parameter nobody knows.
    pub skipped: usize,
    pub truncated: usize,
    pub unknown_params: usize,
}

/// What a load of a set's file found that it could not use: the lines it
/// skipped, the valid lines beyond the entry limit, and the lines whose
/// parameter nobody knows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Unused {
    skipped: usize,
    truncated: usize,
    unknown_params: usize,
}

impl Unused {
    fn of(parsed: &ParsedSet) -> Unused {
        Unused {
            skipped: parsed.skipped_total,
            truncated: parsed.truncated,
            unknown_params: parsed.unknown_params,
        }
    }
}

/// Whether a load that found `now` is worth a line in the log, given what the
/// load before it found (`None`: the set had not loaded yet). One that found
/// nothing is not, and neither is one that found what the one before it
/// found: a list that is refreshed every day and always has three lines
/// nobody can read is not worth a warning a day. The numbers of the lines are
/// no part of it, since lines move and the count stays.
fn worth_a_warning(before: Option<Unused>, now: Unused) -> bool {
    now != Unused::default() && before != Some(now)
}

struct Entry {
    handle: SetHandle,
    resource: Option<ResourceHandle>,
    /// What the last load found it could not use; `None` while the set has
    /// not loaded.
    unused: Option<Unused>,
    state: String,
}

pub struct SetRegistry {
    resources: Arc<ResourceManager>,
    base_dir: PathBuf,
    inline: HashMap<String, Vec<SubRule>>,
    inline_names: HashSet<String>,
    entries: Mutex<HashMap<(Key, SetKind), Entry>>,
    /// The role the profile's own text gives each URL and file (`claims_of`).
    claims: HashMap<Key, SetKind>,
    diags: Mutex<Diagnostics>,
    /// `true` only while `build` is running; `push_diag` is a no-op outside
    /// that window so reloads (which already log through `tracing`) don't
    /// keep accumulating diagnostics that `build`'s caller already took and
    /// nobody reads again (F4).
    building: AtomicBool,
    /// Held by whoever parses and compiles a set of the registry that has new
    /// content, so that only one set is at it at a time: a full set needs
    /// about three times its size while it is compiled, and a few dozen get
    /// their content together when the profile starts cold or every resource
    /// is updated. The build holds it until everything the profile names is
    /// created; a reload task holds it from reading the resource to storing
    /// the set. A task keeps a clone of the `Arc` and not the registry, so one
    /// that waits for its turn does not keep a registry that was replaced.
    ///
    /// Nothing that compiles is awaited under it, and `ensure` and `finish`
    /// never ask for it: a set that names another compiles that one inside
    /// itself, on the same thread, under the lock its own compile holds.
    compile_lock: Arc<tokio::sync::Mutex<()>>,
    /// How many stretches of parsing and compiling are going on, the most
    /// there have been at once and how many have begun: what the tests ask to
    /// know that the sets of a registry are compiled one at a time, and a
    /// version once.
    #[cfg(test)]
    compiling: Compiling,
    self_weak: Mutex<Weak<SetRegistry>>,
}

/// The stretches of compiling in a registry, in a test build.
#[cfg(test)]
#[derive(Default)]
struct Compiling {
    now: AtomicUsize,
    most: AtomicUsize,
    begun: AtomicUsize,
}

/// One stretch of compiling, counted for as long as it lives.
#[cfg(test)]
struct Stretch<'a>(&'a Compiling);

#[cfg(test)]
impl Compiling {
    fn begin(&self) -> Stretch<'_> {
        let now = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        self.begun.fetch_add(1, Ordering::SeqCst);
        Stretch(self)
    }
}

#[cfg(test)]
impl Drop for Stretch<'_> {
    fn drop(&mut self) {
        self.0.now.fetch_sub(1, Ordering::SeqCst);
    }
}

thread_local! {
    /// Compile stack of the current thread: nested `lookup` calls during one
    /// compile push here, so cycles and depth are detected per compile.
    static STACK: RefCell<Vec<Key>> = const { RefCell::new(Vec::new()) };
}

/// Pops one entry off `STACK` on drop. `ensure` and the hot-reload path both
/// go through `push_key` instead of matching manual push/pop calls by hand:
/// a reload that cleared the stack and recompiled without pushing the key
/// being recompiled is exactly how a self-referencing set used to capture
/// its own handle inside itself instead of being caught as a cycle.
struct StackGuard;

impl Drop for StackGuard {
    fn drop(&mut self) {
        STACK.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

/// Pushes `key` onto the current thread's compile stack; the returned guard
/// pops it back off when dropped (including on an early return or panic).
fn push_key(key: Key) -> StackGuard {
    STACK.with(|s| s.borrow_mut().push(key));
    StackGuard
}

/// The role the profile's own text gives each URL and file: the rules (the
/// sub-rules of logical rules included), the `[Host]` set keys and the lines
/// of its inline sets — the references the loader's `E0024` looks at, which
/// it guarantees to give a resource one role. Should a profile break that
/// anyway, the first role met stands.
fn claims_of(cfg: &Config) -> HashMap<Key, SetKind> {
    let mut claims = HashMap::new();
    for rule in &cfg.rules {
        claim_kind(&rule.kind, &mut claims);
    }
    for host in &cfg.hosts {
        match &host.key {
            HostKey::DomainSet(r) => claim(r, SetKind::DomainSet, &mut claims),
            HostKey::RuleSet(r) => claim(r, SetKind::RuleSet, &mut claims),
            HostKey::Pattern(_) => {}
        }
    }
    for set in &cfg.rulesets {
        for line in &set.rules {
            claim_kind(&line.kind, &mut claims);
        }
    }
    claims
}

fn claim_kind(kind: &RuleKind, claims: &mut HashMap<Key, SetKind>) {
    match kind {
        RuleKind::RuleSet(r) => claim(r, SetKind::RuleSet, claims),
        RuleKind::DomainSet(r) => claim(r, SetKind::DomainSet, claims),
        RuleKind::And(subs) | RuleKind::Or(subs) => {
            for sub in subs {
                claim_kind(&sub.kind, claims);
            }
        }
        RuleKind::Not(sub) => claim_kind(&sub.kind, claims),
        _ => {}
    }
}

fn claim(r: &ResourceRef, role: SetKind, claims: &mut HashMap<Key, SetKind>) {
    let key = Key::from_ref(r);
    if matches!(key, Key::Url(_) | Key::File(_)) {
        claims.entry(key).or_insert(role);
    }
}

impl SetRegistry {
    pub fn build(
        cfg: &Config,
        resources: Arc<ResourceManager>,
        base_dir: &Path,
    ) -> (Arc<SetRegistry>, Diagnostics) {
        let inline: HashMap<String, Vec<SubRule>> = cfg
            .rulesets
            .iter()
            .map(|r| (r.name.clone(), r.rules.clone()))
            .collect();
        let inline_names = inline.keys().cloned().collect();
        let reg = Arc::new(SetRegistry {
            resources,
            base_dir: base_dir.to_path_buf(),
            inline,
            inline_names,
            entries: Mutex::new(HashMap::new()),
            claims: claims_of(cfg),
            diags: Mutex::new(Diagnostics::default()),
            building: AtomicBool::new(true),
            compile_lock: Arc::new(tokio::sync::Mutex::new(())),
            #[cfg(test)]
            compiling: Compiling::default(),
            self_weak: Mutex::new(Weak::new()),
        });
        *reg.self_weak.lock().expect("registry lock") = Arc::downgrade(&reg);
        // Nobody else has the registry yet, so this is free. The reload tasks
        // the sets below start wait for it until everything is created.
        let turn = reg.compile_lock.try_lock().ok();
        debug_assert!(turn.is_some(), "a registry nobody has yet is at no compile");
        #[cfg(test)]
        let stretch = reg.compiling.begin();
        // Pre-create everything the profile uses so statuses are complete and
        // downloads start now. Matching ends at the first FINAL: a set that
        // only the rules after it name is never consulted, and is neither
        // read nor downloaded. The roles in `claims` are still those of every
        // rule written, the way the loader's `E0024` reads them.
        let evaluated = cfg.first_final().unwrap_or(cfg.rules.len());
        for rule in &cfg.rules[..evaluated] {
            reg.walk_kind(&rule.kind, rule.params.update_interval);
        }
        for host in &cfg.hosts {
            match &host.key {
                HostKey::DomainSet(r) => {
                    reg.ensure(r, SetKind::DomainSet, None);
                }
                HostKey::RuleSet(r) => {
                    reg.ensure(r, SetKind::RuleSet, None);
                }
                HostKey::Pattern(_) => {}
            }
        }
        let diags = std::mem::take(&mut *reg.diags.lock().expect("registry lock"));
        // `building` is over before a reload task has its turn: the first
        // thing it compiles is said in the log and not in the diagnostics
        reg.building.store(false, Ordering::Relaxed);
        #[cfg(test)]
        drop(stretch);
        drop(turn);
        (reg, diags)
    }

    fn walk_kind(&self, kind: &RuleKind, update_interval: Option<i64>) {
        match kind {
            RuleKind::RuleSet(r) => {
                self.ensure(r, SetKind::RuleSet, update_interval);
            }
            RuleKind::DomainSet(r) => {
                self.ensure(r, SetKind::DomainSet, update_interval);
            }
            RuleKind::And(subs) | RuleKind::Or(subs) => {
                for s in subs {
                    self.walk_kind(&s.kind, None);
                }
            }
            RuleKind::Not(sub) => self.walk_kind(&sub.kind, None),
            _ => {}
        }
    }

    pub fn get(&self, r: &ResourceRef, kind: SetKind) -> SetHandle {
        self.ensure(r, kind, None)
    }

    pub fn statuses(&self) -> Vec<SetStatus> {
        let entries = self.entries.lock().expect("registry lock");
        let mut out: Vec<SetStatus> = entries
            .iter()
            .map(|((key, kind), e)| {
                let set = e.handle.load();
                let unused = e.unused.unwrap_or_default();
                SetStatus {
                    name: key.display(),
                    key: self.source_of(key).ok().map(|s| resource_key(&s)),
                    kind: *kind,
                    entries: set.entry_count(),
                    version: set.version,
                    state: e.state.clone(),
                    skipped: unused.skipped,
                    truncated: unused.truncated,
                    unknown_params: unused.unknown_params,
                }
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    fn push_diag(&self, d: Diagnostic) {
        if !self.building.load(Ordering::Relaxed) {
            return;
        }
        self.diags.lock().expect("registry lock").push(d);
    }

    #[cfg(test)]
    fn diag_count(&self) -> usize {
        self.diags.lock().expect("registry lock").len()
    }

    /// How many stretches of parsing and compiling are going on right now.
    #[cfg(test)]
    fn compiling_now(&self) -> usize {
        self.compiling.now.load(Ordering::SeqCst)
    }

    /// The most stretches of parsing and compiling there have been at once.
    #[cfg(test)]
    fn most_compiling_at_once(&self) -> usize {
        self.compiling.most.load(Ordering::SeqCst)
    }

    /// How many stretches of parsing and compiling have begun so far, the one
    /// of the build among them.
    #[cfg(test)]
    fn stretches_begun(&self) -> usize {
        self.compiling.begun.load(Ordering::SeqCst)
    }

    /// Returns the (possibly still compiling) handle for a reference, creating
    /// and compiling it on first use.
    fn ensure(&self, r: &ResourceRef, kind: SetKind, update_interval: Option<i64>) -> SetHandle {
        let key = Key::from_ref(r);
        let name = key.display();
        let in_stack = STACK.with(|s| s.borrow().contains(&key));
        let depth = STACK.with(|s| s.borrow().len());
        if in_stack || depth >= MAX_NESTING {
            let why = if in_stack {
                "cycle"
            } else {
                "nesting deeper than 8"
            };
            self.push_diag(Diagnostic::warning(
                codes::W_SET_NESTING,
                format!("set `{name}` ignored: {why}"),
            ));
            tracing::warn!(set = %name, "nested set reference ignored: {why}");
            return SetHandle::new(CompiledSet::empty(&name, kind));
        }
        let other = match kind {
            SetKind::RuleSet => SetKind::DomainSet,
            SetKind::DomainSet => SetKind::RuleSet,
        };
        let loses = {
            let mut entries = self.entries.lock().expect("registry lock");
            if let Some(e) = entries.get(&(key.clone(), kind)) {
                return e.handle.clone();
            }
            // A URL or file is read in one format only. The role the profile
            // itself gives it is the one that stays, whether or not that
            // role has an entry yet: which entry comes first would depend on
            // which set files already have content while the registry is
            // built. References nested in set files that the profile does not
            // back, and that disagree with each other, are left to the order
            // in which they are registered; which comes first then depends
            // on which parent is compiled first, and that is accepted.
            let loses = matches!(key, Key::Url(_) | Key::File(_))
                && match self.claims.get(&key) {
                    Some(claimed) => *claimed != kind,
                    None => entries.contains_key(&(key.clone(), other)),
                };
            if !loses {
                entries.insert(
                    (key.clone(), kind),
                    Entry {
                        handle: SetHandle::new(CompiledSet::empty(&name, kind)),
                        resource: None,
                        unused: None,
                        state: "compiling".to_string(),
                    },
                );
            }
            loses
        };
        if loses {
            // the set being compiled is the one the reference is written in
            let parent = STACK.with(|s| s.borrow().last().map(Key::display));
            let written_in = parent
                .map(|parent| format!(" (referenced in `{parent}`)"))
                .unwrap_or_default();
            let said = format!(
                "set `{name}`{written_in} ignored: the same resource is used as a {}",
                other.keyword()
            );
            self.push_diag(Diagnostic::warning(codes::W_SET_KIND_CONFLICT, &said));
            tracing::warn!(set = %name, "{said}");
            return SetHandle::new(CompiledSet::empty(&name, kind));
        }
        let handle = self.handle_of(&key, kind);
        let _guard = push_key(key.clone());
        let outcome = match &key {
            Key::Internal(i) => {
                let text = internal_set_source(*i, cfg!(target_os = "macos"));
                let parsed = self.parse_text(&key, kind, &text, &self.base_dir);
                self.finish(&key, kind, &name, parsed, 1, "internal")
            }
            Key::Inline(n) => {
                let lines = self.inline.get(n).cloned().unwrap_or_default();
                let parsed = ParsedSet {
                    lines: lines.into_iter().map(SetLine::Rule).collect(),
                    ..ParsedSet::default()
                };
                self.finish(&key, kind, &name, parsed, 1, "inline")
            }
            Key::Url(_) | Key::File(_) => self.compile_external(&key, kind, &name, update_interval),
        };
        if let Some(set) = outcome {
            handle.store(set);
        }
        handle
    }

    fn handle_of(&self, key: &Key, kind: SetKind) -> SetHandle {
        self.entries
            .lock()
            .expect("registry lock")
            .get(&(key.clone(), kind))
            .map(|e| e.handle.clone())
            .expect("entry inserted by ensure")
    }

    /// The lines of `text`, the text of the set `key` names.
    fn parse_text(&self, key: &Key, kind: SetKind, text: &str, base_dir: &Path) -> ParsedSet {
        let ctx = ParseCtx {
            inline_rulesets: &self.inline_names,
            base_dir,
            reach: key.reach(),
        };
        parse_set(kind, text, &ctx)
    }

    /// Compile `parsed`, record what it could not use (the warnings about it
    /// are diagnostics while the registry is built, and a line of the log
    /// when that changes after), return the set.
    fn finish(
        &self,
        key: &Key,
        kind: SetKind,
        name: &str,
        parsed: ParsedSet,
        version: u64,
        state: &str,
    ) -> Option<CompiledSet> {
        if !parsed.skipped.is_empty() {
            let (line, reason) = &parsed.skipped[0];
            self.push_diag(Diagnostic::warning(
                codes::W_SET_LINES_SKIPPED,
                format!(
                    "set `{name}`: {} line(s) skipped (first: line {line}: {reason})",
                    parsed.skipped_total
                ),
            ));
        }
        if parsed.truncated > 0 {
            self.push_diag(Diagnostic::warning(
                codes::W_SET_TRUNCATED,
                format!(
                    "set `{name}`: {} line(s) beyond the 1,000,000 entry limit ignored",
                    parsed.truncated
                ),
            ));
        }
        // Only the count and the line: a set file is arbitrary text, and the
        // parameter lost may be a password on a proxy line copied into it.
        if let Some(line) = parsed.first_unknown {
            self.push_diag(Diagnostic::warning(
                codes::W_UNKNOWN_RULE_PARAM,
                format!(
                    "set `{name}`: {} line(s) carry a parameter that is not known and is ignored (first: line {line})",
                    parsed.unknown_params
                ),
            ));
        }
        let set = CompiledSet::compile(name, kind, &parsed, self, version);
        let unused = Unused::of(&parsed);
        let say = {
            let mut entries = self.entries.lock().expect("registry lock");
            entries.get_mut(&(key.clone(), kind)).is_some_and(|e| {
                // While the registry is built the diagnostics above are the
                // ones that say it, and they say it every time it is built.
                let say =
                    !self.building.load(Ordering::Relaxed) && worth_a_warning(e.unused, unused);
                e.unused = Some(unused);
                e.state = state.to_string();
                say
            })
        };
        // Written before the caller stores the set, so that whoever waits for
        // the version of the set to move finds the line, or that there is
        // none. Numbers only: no line, and no reason of a skipped one, which
        // may quote how its line begins.
        if say {
            tracing::warn!(
                set = %name,
                skipped = unused.skipped,
                truncated = unused.truncated,
                unknown_params = unused.unknown_params,
                first_skipped_line = parsed.skipped.first().map(|(line, _)| *line),
                first_unknown_line = parsed.first_unknown,
                "set loaded with lines that are not used"
            );
        }
        Some(set)
    }

    fn source_of(&self, key: &Key) -> Result<ResourceSource, String> {
        match key {
            Key::Url(u) => Url::parse(u)
                .map(ResourceSource::Url)
                .map_err(|e| e.to_string()),
            Key::File(p) => Ok(ResourceSource::File(p.clone())),
            _ => Err("not an external resource".to_string()),
        }
    }

    fn compile_external(
        &self,
        key: &Key,
        kind: SetKind,
        name: &str,
        update_interval: Option<i64>,
    ) -> Option<CompiledSet> {
        let source = match self.source_of(key) {
            Ok(s) => s,
            Err(e) => {
                self.push_diag(Diagnostic::warning(
                    codes::W_RESOURCE_UNAVAILABLE,
                    format!("set `{name}`: invalid resource: {e}"),
                ));
                self.set_state(key, kind, format!("failed: {e}"));
                return None;
            }
        };
        let resource = self.resources.get(&ResourceSpec {
            source,
            update_interval,
            kind: match kind {
                SetKind::RuleSet => ResourceKind::RuleSet,
                SetKind::DomainSet => ResourceKind::DomainSet,
            },
        });
        // Subscribe before the first `resource.current()` read below: a
        // `watch` receiver only observes changes published *after* it
        // subscribes, so subscribing later (e.g. inside the spawned reload
        // task) could miss a version published in between and leave the
        // reloader stuck on stale content until some later change.
        let rx = resource.subscribe();
        let base_dir = match key {
            Key::File(p) => p
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.base_dir.clone()),
            _ => self.base_dir.clone(),
        };
        let compiled = match resource.current() {
            ResourceState::Available { data, version, .. } => {
                let text = String::from_utf8_lossy(&data);
                let parsed = self.parse_text(key, kind, &text, &base_dir);
                self.finish(key, kind, name, parsed, version, "ok")
            }
            ResourceState::Failed {
                last_error,
                cached: Some((data, version)),
                ..
            } => {
                let text = String::from_utf8_lossy(&data);
                let parsed = self.parse_text(key, kind, &text, &base_dir);
                self.finish(
                    key,
                    kind,
                    name,
                    parsed,
                    version,
                    &format!("stale: {last_error}"),
                )
            }
            ResourceState::Failed {
                last_error,
                cached: None,
                ..
            } => {
                self.push_diag(Diagnostic::warning(
                    codes::W_RESOURCE_UNAVAILABLE,
                    format!("set `{name}`: resource unavailable ({last_error}); treated as empty until it loads"),
                ));
                self.set_state(key, kind, format!("failed: {last_error}"));
                None
            }
            ResourceState::Missing => {
                self.push_diag(Diagnostic::warning(
                    codes::W_RESOURCE_UNAVAILABLE,
                    format!(
                        "set `{name}`: resource not downloaded yet; treated as empty until it loads"
                    ),
                ));
                self.set_state(key, kind, "missing".to_string());
                None
            }
        };
        {
            let mut entries = self.entries.lock().expect("registry lock");
            if let Some(e) = entries.get_mut(&(key.clone(), kind)) {
                e.resource = Some(resource.clone());
            }
        }
        self.spawn_reloader(key.clone(), kind, base_dir, resource, rx);
        compiled
    }

    fn set_state(&self, key: &Key, kind: SetKind, state: String) {
        let mut entries = self.entries.lock().expect("registry lock");
        if let Some(e) = entries.get_mut(&(key.clone(), kind)) {
            e.state = state;
        }
    }

    /// Recompile and swap whenever the resource publishes a new version, one
    /// set of the registry at a time (`compile_lock`). `rx` must already be
    /// subscribed to `resource` (see `compile_external`) so a version
    /// published before this task starts is never missed.
    fn spawn_reloader(
        &self,
        key: Key,
        kind: SetKind,
        base_dir: PathBuf,
        resource: ResourceHandle,
        mut rx: watch::Receiver<u64>,
    ) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            tracing::debug!(set = %key.display(), "no tokio runtime: set auto-reload disabled");
            return;
        };
        let weak = self.self_weak.lock().expect("registry lock").clone();
        let compile_lock = self.compile_lock.clone();
        rt.spawn(async move {
            loop {
                // Wake periodically too: the registry may be dropped while the resource never changes again.
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(60)) => {
                        if weak.upgrade().is_none() {
                            return;
                        }
                        continue;
                    }
                }
                // The turn is waited for before anything else, and the state of
                // the resource is read after it: a version that was replaced
                // while this task waited is never compiled. Nothing below
                // awaits, so the compile stays on one thread from the key
                // pushed on its stack to the end, as it has to.
                let _turn = compile_lock.lock().await;
                let Some(reg) = weak.upgrade() else { return };
                // What the manager holds, also after an attempt that failed:
                // a version that was followed by one while this task waited
                // is the copy the failed state keeps, and nothing would
                // announce it a second time.
                let Some((data, version)) = resource.current().data() else {
                    continue;
                };
                let name = key.display();
                let handle = reg.handle_of(&key, kind);
                if handle.version() == version {
                    continue;
                }
                #[cfg(test)]
                let _stretch = reg.compiling.begin();
                let text = String::from_utf8_lossy(&data);
                // A fresh compile on this thread starts with an empty stack;
                // push the key being recompiled first so new content that
                // references itself (directly, or through a nested set) is
                // caught as a cycle instead of capturing this same handle
                // inside the set it is about to replace (F1).
                STACK.with(|s| s.borrow_mut().clear());
                let _guard = push_key(key.clone());
                let parsed = reg.parse_text(&key, kind, &text, &base_dir);
                let entries = parsed.lines.len();
                if let Some(set) = reg.finish(&key, kind, &name, parsed, version, "ok") {
                    handle.store(set);
                    tracing::info!(set = %name, version, entries, "set reloaded");
                }
            }
        });
    }
}

impl SetLookup for SetRegistry {
    fn lookup(&self, r: &ResourceRef, kind: SetKind) -> SetRef {
        Arc::new(self.ensure(r, kind, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{OutboundMode, Reason, RuleEngine};
    use crate::matcher::NoGeo;
    use rurge_config::HostName;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_config::session::SessionInfo;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::http::{HttpClient, HttpClientConfig};
    use rurge_net::resource::ResourceOptions;
    use rurge_net::testing::{TestServer, seed_cache};
    use std::time::{Duration, SystemTime};

    fn manager(root: &Path) -> Arc<ResourceManager> {
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let client = Arc::new(HttpClient::new(connector, HttpClientConfig::default()).unwrap());
        ResourceManager::with_options(
            root.to_path_buf(),
            client,
            ResourceOptions {
                fetch_timeout: Duration::from_secs(5),
                min_backoff: Duration::from_millis(50),
                max_backoff: Duration::from_millis(200),
                debounce: Duration::from_millis(50),
                ..ResourceOptions::default()
            },
        )
    }

    /// Nothing is fetched: a URL is unavailable unless a copy is cached.
    fn offline_manager(root: &Path) -> Arc<ResourceManager> {
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let client = Arc::new(HttpClient::new(connector, HttpClientConfig::default()).unwrap());
        ResourceManager::with_options(
            root.to_path_buf(),
            client,
            ResourceOptions {
                offline: true,
                ..ResourceOptions::default()
            },
        )
    }

    fn load(text: &str, base: &Path) -> Config {
        let l = from_text(text, &base.join("t.conf"), &LoadOptions::for_tests());
        let codes: Vec<&str> = l.diagnostics.iter().map(|d| d.code).collect();
        assert!(!l.diagnostics.has_errors(), "{codes:?}");
        l.config
    }

    fn codes_of(d: &Diagnostics) -> Vec<&'static str> {
        d.iter().map(|d| d.code).collect()
    }

    async fn decide(engine: &RuleEngine, host: &str) -> (String, Reason) {
        let d = engine
            .evaluate(
                &SessionInfo::tcp(HostName::parse(host), 443),
                OutboundMode::Rule,
                &crate::engine::NoResolve,
            )
            .await;
        (d.policy().map(|p| p.name()).unwrap_or_default(), d.reason)
    }

    async fn wait_version(handle: &SetHandle, min: u64) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while handle.version() < min {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("set did not reload in time");
    }

    #[tokio::test]
    async fn internal_inline_and_nested_inline_sets() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,SYSTEM,DIRECT\nRULE-SET,Outer,P\nAND,((RULE-SET,LAN),(DEST-PORT,443)),REJECT\nFINAL,DIRECT\n[Ruleset Outer]\nDOMAIN,outer.com\nRULE-SET,Inner\n[Ruleset Inner]\nDOMAIN-SUFFIX,inner.com\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert!(diags.is_empty(), "{:?}", codes_of(&diags));
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "captive.apple.com").await.0, "DIRECT");
        assert_eq!(decide(&engine, "x.inner.com").await.0, "P");
        assert_eq!(decide(&engine, "outer.com").await.0, "P");
        assert_eq!(decide(&engine, "10.0.0.1").await.0, "REJECT");
        let names: Vec<String> = reg.statuses().into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["Inner", "LAN", "Outer", "SYSTEM"]);
    }

    /// The registry builds `SYSTEM` from the text the host reads: without its
    /// two `PROCESS-NAME` lines (services of macOS) anywhere but on macOS.
    #[tokio::test]
    async fn the_built_in_system_set_has_its_process_lines_on_macos_only() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,SYSTEM,DIRECT\nFINAL,P\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert!(diags.is_empty(), "{:?}", codes_of(&diags));
        let statuses = reg.statuses();
        let system = statuses
            .iter()
            .find(|s| s.name == "SYSTEM")
            .expect("the built-in set");
        assert_eq!(
            system.entries,
            if cfg!(target_os = "macos") { 20 } else { 18 }
        );
    }

    #[tokio::test]
    async fn local_file_set_compiles_and_reloads_on_change() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sets")).unwrap();
        let file = dir.path().join("sets").join("a.list");
        std::fs::write(&file, "DOMAIN,a.com\n# comment\nnot a rule\n").unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,sets/a.list,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert_eq!(codes_of(&diags), vec![codes::W_SET_LINES_SKIPPED]);
        let handle = reg.get(&ResourceRef::File(file.clone()), SetKind::RuleSet);
        let v1 = handle.version();
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "a.com").await.0, "P");
        assert_eq!(decide(&engine, "b.com").await.0, "DIRECT");
        std::fs::write(&file, "DOMAIN,b.com\n").unwrap();
        wait_version(&handle, v1 + 1).await;
        assert_eq!(decide(&engine, "a.com").await.0, "DIRECT");
        assert_eq!(decide(&engine, "b.com").await.0, "P");
        let st = reg
            .statuses()
            .into_iter()
            .find(|s| s.name.ends_with("a.list"))
            .unwrap();
        assert_eq!(st.state, "ok");
        assert_eq!(st.skipped, 0);
    }

    /// A list written the way the published ones are, with a note after a
    /// number, a name and an address range, is read whole: the registry says
    /// nothing, every line is an entry, and a name followed by a note still
    /// matches its name.
    #[tokio::test]
    async fn a_file_written_the_way_the_lists_write_it_loads_whole() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.list");
        std::fs::write(
            &file,
            "\
# a header
IP-ASN,4134 // 中国电信骨干网
IP-ASN,132203 // Tencent Building, Kejizhongyi Avenue
IP-ASN,131326 // 2308 Yinlaige # 22 Jinsui Road
DOMAIN-SUFFIX,example.com // note
IP-CIDR,10.0.0.0/8,no-resolve # lan
",
        )
        .unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,a.list,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert!(diags.is_empty(), "{:?}", codes_of(&diags));
        let st = reg
            .statuses()
            .into_iter()
            .find(|s| s.name.ends_with("a.list"))
            .unwrap();
        assert_eq!(st.state, "ok");
        assert_eq!(st.skipped, 0);
        assert_eq!(st.entries, 5);
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "x.example.com").await.0, "P");
    }

    /// Lists saved by some editors start with a byte order mark: the first
    /// rule is read like any other, and the registry says nothing.
    #[tokio::test]
    async fn a_local_file_saved_with_a_byte_order_mark_loads_whole() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.list");
        let bytes = [&b"\xEF\xBB\xBF"[..], b"DOMAIN,a.com\nDOMAIN,b.com\n"].concat();
        std::fs::write(&file, bytes).unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,a.list,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert!(diags.is_empty(), "{:?}", codes_of(&diags));
        let st = reg
            .statuses()
            .into_iter()
            .find(|s| s.name.ends_with("a.list"))
            .unwrap();
        assert_eq!(st.skipped, 0);
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "a.com").await.0, "P");
    }

    /// A file given as a domain set by mistake can be a subscription or a list
    /// of proxy lines: the warning counts the lines it skipped and names the
    /// first by its number, and says nothing of what that line holds.
    #[tokio::test]
    async fn a_domain_set_given_the_wrong_file_warns_without_quoting_its_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.list");
        std::fs::write(&file, "trojan://S3CR3T@example.com:443\ngood.example.com\n").unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nDOMAIN-SET,a.list,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert_eq!(codes_of(&diags), vec![codes::W_SET_LINES_SKIPPED]);
        let message = &diags.iter().next().unwrap().message;
        assert!(!message.contains("S3CR3T"), "{message}");
        assert!(message.contains("1 line(s) skipped"), "{message}");
        assert!(message.contains("first: line 1"), "{message}");
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "good.example.com").await.0, "P");
    }

    #[tokio::test]
    async fn missing_file_is_an_empty_set_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nDOMAIN-SET,nope.txt,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert_eq!(codes_of(&diags), vec![codes::W_RESOURCE_UNAVAILABLE]);
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "a.com").await.0, "DIRECT");
        assert!(reg.statuses()[0].state.starts_with("failed"));
    }

    #[tokio::test]
    async fn external_cycle_and_depth_are_cut() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.list"), "RULE-SET,b.list\nDOMAIN,a.com\n").unwrap();
        std::fs::write(dir.path().join("b.list"), "RULE-SET,a.list\nDOMAIN,b.com\n").unwrap();
        for i in 0..12 {
            std::fs::write(
                dir.path().join(format!("d{i}.list")),
                format!("RULE-SET,d{}.list\n", i + 1),
            )
            .unwrap();
        }
        std::fs::write(dir.path().join("d12.list"), "DOMAIN,deep.com\n").unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,a.list,P\nRULE-SET,d0.list,REJECT\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        let codes = codes_of(&diags);
        assert!(
            codes.iter().filter(|c| **c == codes::W_SET_NESTING).count() >= 2,
            "{codes:?}"
        );
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "a.com").await.0, "P");
        assert_eq!(decide(&engine, "b.com").await.0, "P");
        assert_eq!(decide(&engine, "deep.com").await.0, "DIRECT");
    }

    /// Nested sets go eight deep (manual, Nested Rule Sets): in a chain of
    /// sets that name one another the eighth is followed, the ninth is cut
    /// and said so.
    #[tokio::test]
    async fn a_chain_of_eight_sets_is_followed_and_the_ninth_is_cut() {
        let dir = tempfile::tempdir().unwrap();
        for i in 1..=9 {
            let next = if i < 9 {
                format!("RULE-SET,d{}.list\n", i + 1)
            } else {
                String::new()
            };
            std::fs::write(
                dir.path().join(format!("d{i}.list")),
                format!("{next}DOMAIN,at{i}.com\n"),
            )
            .unwrap();
        }
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,d1.list,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        let cut: Vec<&str> = codes_of(&diags)
            .into_iter()
            .filter(|c| *c == codes::W_SET_NESTING)
            .collect();
        assert_eq!(cut.len(), 1, "{:?}", codes_of(&diags));
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "at1.com").await.0, "P");
        assert_eq!(
            decide(&engine, "at8.com").await.0,
            "P",
            "the eighth set is followed"
        );
        assert_eq!(
            decide(&engine, "at9.com").await.0,
            "DIRECT",
            "the ninth is cut"
        );
    }

    #[tokio::test]
    async fn url_set_downloads_and_swaps_after_force_update() {
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/a.list", "DOMAIN,a.com\n");
        let url = server.url("/a.list");
        let cfg = load(
            &format!("[Proxy]\nP = direct\n[Rule]\nRULE-SET,{url},P\nFINAL,DIRECT\n"),
            dir.path(),
        );
        let mgr = manager(dir.path());
        let (reg, diags) = SetRegistry::build(&cfg, mgr.clone(), dir.path());
        // First build: the download runs in the background, so the set starts empty.
        assert_eq!(codes_of(&diags), vec![codes::W_RESOURCE_UNAVAILABLE]);
        let handle = reg.get(&ResourceRef::Url(url.to_string()), SetKind::RuleSet);
        wait_version(&handle, 1).await;
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "a.com").await.0, "P");
        server.set("/a.list", "DOMAIN,b.com\n");
        assert!(mgr.force_update(&ResourceSource::Url(url.clone())));
        wait_version(&handle, 2).await;
        assert_eq!(decide(&engine, "a.com").await.0, "DIRECT");
        assert_eq!(decide(&engine, "b.com").await.0, "P");
        // Second manager on the same root starts from the cache: no warning.
        let (reg2, diags2) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert!(diags2.is_empty(), "{:?}", codes_of(&diags2));
        assert_eq!(reg2.statuses()[0].state, "ok");
    }

    /// The resources the manager was asked for, as URL paths. A file is none:
    /// the sets of these tests that are not URLs are never asked of it.
    fn url_paths(mgr: &ResourceManager) -> Vec<String> {
        let mut paths: Vec<String> = mgr
            .statuses()
            .into_iter()
            .map(|s| match s.source {
                ResourceSource::Url(url) => url.path().to_string(),
                ResourceSource::File(file) => panic!("a file: {}", file.display()),
            })
            .collect();
        paths.sort();
        paths
    }

    /// What a downloaded set lists is not for it to take from this machine:
    /// a line that names a local file as a set is skipped, as an unusable
    /// line of a set file is, and the file is never asked for. A name only
    /// the local file lists does not match, one the set lists itself does.
    /// Here the set is the cached copy of a download, read when the registry
    /// is built.
    #[tokio::test]
    async fn a_downloaded_set_cannot_name_a_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("local-rules.list");
        let domains = dir.path().join("local-domains.txt");
        std::fs::write(&rules, "DOMAIN,from-local-rules.example\n").unwrap();
        std::fs::write(&domains, ".from-local-domains.example\n").unwrap();
        let server = TestServer::spawn().await;
        server.set(
            "/a.list",
            format!("RULE-SET,{}\nDOMAIN-SUFFIX,example.com\n", rules.display()),
        );
        server.set(
            "/b.list",
            format!(
                "DOMAIN-SET,{}\nDOMAIN-SUFFIX,example.org\n",
                domains.display()
            ),
        );
        let (a, b) = (server.url("/a.list"), server.url("/b.list"));
        let cfg = load(
            &format!("[Proxy]\nP = direct\n[Rule]\nRULE-SET,{a},P\nRULE-SET,{b},P\nFINAL,DIRECT\n"),
            dir.path(),
        );
        // the first registry downloads both; the second starts from the cache
        let (first, _) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        for url in [&a, &b] {
            let handle = first.get(&ResourceRef::Url(url.to_string()), SetKind::RuleSet);
            wait_version(&handle, 1).await;
        }
        let mgr = manager(dir.path());
        let (reg, diags) = SetRegistry::build(&cfg, mgr.clone(), dir.path());
        let reason = "a downloaded file cannot name a local file as a set";
        let mut said: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
        said.sort();
        let mut expected: Vec<String> = [&a, &b]
            .iter()
            .map(|url| format!("set `{url}`: 1 line(s) skipped (first: line 1: {reason})"))
            .collect();
        expected.sort();
        assert_eq!(said, expected);
        assert_eq!(codes_of(&diags), [codes::W_SET_LINES_SKIPPED; 2]);
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "example.com").await.0, "P");
        assert_eq!(decide(&engine, "example.org").await.0, "P");
        assert_eq!(
            decide(&engine, "from-local-rules.example").await.0,
            "DIRECT"
        );
        assert_eq!(
            decide(&engine, "from-local-domains.example").await.0,
            "DIRECT"
        );
        // the two sets are all there is: one skipped line each, and no entry
        // and no resource for a file
        let statuses = reg.statuses();
        let names: Vec<&str> = statuses.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, [a.as_str(), b.as_str()]);
        assert_eq!(
            statuses.iter().map(|s| s.skipped).collect::<Vec<_>>(),
            [1, 1]
        );
        assert_eq!(url_paths(&mgr), ["/a.list", "/b.list"]);
    }

    /// The same after the set is downloaded again: a text that now names a
    /// local file does not get it read.
    #[tokio::test]
    async fn a_downloaded_set_that_comes_to_name_a_local_file_still_cannot_read_it() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("local-rules.list");
        std::fs::write(&local, "DOMAIN,from-local.example\n").unwrap();
        let server = TestServer::spawn().await;
        server.set("/a.list", "DOMAIN-SUFFIX,example.com\n");
        let url = server.url("/a.list");
        let cfg = load(
            &format!("[Proxy]\nP = direct\n[Rule]\nRULE-SET,{url},P\nFINAL,DIRECT\n"),
            dir.path(),
        );
        let mgr = manager(dir.path());
        let (reg, _) = SetRegistry::build(&cfg, mgr.clone(), dir.path());
        let handle = reg.get(&ResourceRef::Url(url.to_string()), SetKind::RuleSet);
        wait_version(&handle, 1).await;
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "example.com").await.0, "P");
        server.set(
            "/a.list",
            format!("RULE-SET,{}\nDOMAIN-SUFFIX,example.com\n", local.display()),
        );
        assert!(mgr.force_update(&ResourceSource::Url(url.clone())));
        wait_version(&handle, 2).await;
        assert_eq!(decide(&engine, "example.com").await.0, "P");
        assert_eq!(decide(&engine, "from-local.example").await.0, "DIRECT");
        let statuses = reg.statuses();
        assert_eq!(statuses.len(), 1, "{statuses:?}");
        assert_eq!((statuses[0].version, statuses[0].skipped), (2, 1));
        assert_eq!(url_paths(&mgr), ["/a.list"]);
    }

    /// A set file of the user's own may name another local file, by a path
    /// relative to itself or an absolute one, as it always could.
    #[tokio::test]
    async fn a_local_set_file_may_name_another_local_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sets")).unwrap();
        let absolute = dir.path().join("c.txt");
        std::fs::write(&absolute, ".c.example\n").unwrap();
        std::fs::write(
            dir.path().join("sets").join("a.list"),
            format!("RULE-SET,b.list\nDOMAIN-SET,{}\n", absolute.display()),
        )
        .unwrap();
        std::fs::write(dir.path().join("sets").join("b.list"), "DOMAIN,b.example\n").unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,sets/a.list,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert!(diags.is_empty(), "{:?}", codes_of(&diags));
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "b.example").await.0, "P");
        assert_eq!(decide(&engine, "x.c.example").await.0, "P");
        assert_eq!(reg.statuses().len(), 3);
    }

    /// The manager knows each external set for what it is, which the rule
    /// that names it says.
    #[tokio::test]
    async fn external_sets_are_registered_with_their_kind() {
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/rules.list", "DOMAIN,a.com\n");
        server.set("/domains.txt", ".b.com\n");
        let cfg = load(
            &format!(
                "[Proxy]\nP = direct\n[Rule]\nRULE-SET,{},P\nDOMAIN-SET,{},P\nFINAL,DIRECT\n",
                server.url("/rules.list"),
                server.url("/domains.txt")
            ),
            dir.path(),
        );
        let mgr = manager(dir.path());
        let (_reg, _) = SetRegistry::build(&cfg, mgr.clone(), dir.path());
        let kinds: Vec<(String, ResourceKind)> = mgr
            .statuses()
            .into_iter()
            .map(|s| match s.source {
                ResourceSource::Url(url) => (url.path().to_string(), s.kind),
                ResourceSource::File(file) => panic!("a file: {}", file.display()),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("/domains.txt".to_string(), ResourceKind::DomainSet),
                ("/rules.list".to_string(), ResourceKind::RuleSet),
            ]
        );
    }

    /// A set is named by its URL in the diagnostics and the log lines, in the
    /// statuses and in what a match reports, and the URL can carry a user
    /// name and a password: the name has `***` for them and keeps the path
    /// and the query, which name the set.
    #[tokio::test]
    async fn a_set_is_named_without_the_user_info_of_its_url() {
        assert_eq!(
            Key::Url("https://al1ce:S3CR3T@sets.example:8443/gh/user/repo@main/a.list?v=1".into())
                .display(),
            "https://***@sets.example:8443/gh/user/repo@main/a.list?v=1"
        );
        assert_eq!(
            Key::Url("https://sets.example/gh/user/repo@main/a.list".into()).display(),
            "https://sets.example/gh/user/repo@main/a.list"
        );
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/a.list", "DOMAIN,a.com\n");
        let port = server.url("/").port().unwrap();
        let cfg = load(
            &format!(
                "[Proxy]\nP = direct\n[Rule]\nRULE-SET,http://al1ce:S3CR3T@127.0.0.1:{port}/a.list,P\nFINAL,DIRECT\n"
            ),
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        let name = format!("http://***@127.0.0.1:{port}/a.list");
        assert_eq!(
            diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>(),
            [format!(
                "set `{name}`: resource not downloaded yet; treated as empty until it loads"
            )]
        );
        assert_eq!(reg.statuses()[0].name, name);
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        let handle = reg.get(
            &ResourceRef::Url(format!("http://al1ce:S3CR3T@127.0.0.1:{port}/a.list")),
            SetKind::RuleSet,
        );
        wait_version(&handle, 1).await;
        let d = engine
            .evaluate(
                &SessionInfo::tcp(HostName::parse("a.com"), 443),
                OutboundMode::Rule,
                &crate::engine::NoResolve,
            )
            .await;
        assert_eq!(d.sub_rule.map(|hit| hit.set), Some(name));
    }

    /// A flagged rule whose set has not been downloaded yet (here: offline,
    /// with nothing cached) matches nothing: the warning says so, and no
    /// session is rejected on account of it.
    #[tokio::test]
    async fn a_pre_matching_rule_set_that_is_not_downloaded_matches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,http://sets.example/a.list,REJECT,pre-matching\n\
DOMAIN,a.com,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, offline_manager(dir.path()), dir.path());
        assert_eq!(codes_of(&diags), vec![codes::W_RESOURCE_UNAVAILABLE]);
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(
            decide(&engine, "a.com").await,
            ("P".to_string(), Reason::Rule)
        );
        assert_eq!(
            decide(&engine, "b.com").await,
            ("DIRECT".to_string(), Reason::Final)
        );
    }

    #[tokio::test]
    async fn host_section_sets_are_registered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("h.txt"), ".h.com\n").unwrap();
        let cfg = load(
            "[Host]\nDOMAIN-SET:h.txt = 1.2.3.4\n[Rule]\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, _) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        let st = reg.statuses();
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].kind, SetKind::DomainSet);
        assert_eq!(st[0].entries, 1);
    }

    /// Matching ends at the first FINAL, so a set that only the rules written
    /// after it name is never consulted: it is not registered, nothing is
    /// read or downloaded for it, and nothing is said about it (the loader
    /// has said that those rules never take effect).
    #[tokio::test]
    async fn a_set_only_rules_after_the_first_final_name_is_not_registered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("live.list"), "DOMAIN,a.com\n").unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,live.list,P\nFINAL,DIRECT\n\
RULE-SET,http://sets.example/after.list,P\n\
AND,((DOMAIN-SET,missing.txt),(DEST-PORT,443)),P\nFINAL,P\n",
            dir.path(),
        );
        let mgr = offline_manager(dir.path());
        let (reg, diags) = SetRegistry::build(&cfg, mgr.clone(), dir.path());
        // registered, each of the two would be a `W0022`: nothing is cached
        // for the URL, and the file does not exist
        assert!(diags.is_empty(), "{:?}", codes_of(&diags));
        let live = [dir.path().join("live.list").display().to_string()];
        let names = || -> Vec<String> { reg.statuses().into_iter().map(|s| s.name).collect() };
        let urls = || {
            mgr.statuses()
                .into_iter()
                .filter(|s| matches!(s.source, ResourceSource::Url(_)))
                .count()
        };
        assert_eq!(names(), live);
        assert_eq!(urls(), 0);
        // compiling the rules registers nothing more
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "a.com").await.0, "P");
        assert_eq!(decide(&engine, "b.com").await.0, "P");
        assert_eq!(names(), live);
        assert_eq!(urls(), 0);
    }

    /// `rule.com` is a line of a rule set and `domain-set.com` one of a domain
    /// set: each name is matched by one reading of the file only, and is a
    /// skipped line in the other.
    const BOTH_KINDS: &str = "DOMAIN,rule.com\ndomain-set.com\n";

    /// A file that is a rule set at the top of the profile and a domain set
    /// inside another set file is one set, the profile's: the nested
    /// reference is left out with a warning, and the file is not read a
    /// second time in the other format.
    #[tokio::test]
    async fn a_domain_set_nested_in_a_file_cannot_reuse_a_rule_set() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.list"), BOTH_KINDS).unwrap();
        std::fs::write(
            dir.path().join("b.list"),
            "DOMAIN-SET,a.list\nDOMAIN,b.com\n",
        )
        .unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,a.list,P\nRULE-SET,b.list,REJECT\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        // the one reading of `a.list` skips `domain-set.com`; a second one
        // would have skipped `DOMAIN,rule.com` and said so
        assert_eq!(
            codes_of(&diags),
            [codes::W_SET_LINES_SKIPPED, codes::W_SET_KIND_CONFLICT]
        );
        let a = dir.path().join("a.list").display().to_string();
        let b = dir.path().join("b.list").display().to_string();
        assert_eq!(
            diags.iter().last().map(|d| d.message.as_str()),
            Some(
                format!(
                    "set `{a}` (referenced in `{b}`) ignored: the same resource is used as a RULE-SET"
                )
                .as_str()
            )
        );
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "rule.com").await.0, "P");
        // read as a domain set, `a.list` would have matched this one
        assert_eq!(decide(&engine, "domain-set.com").await.0, "DIRECT");
        assert_eq!(decide(&engine, "b.com").await.0, "REJECT");
        let statuses: Vec<(String, SetKind)> = reg
            .statuses()
            .into_iter()
            .map(|s| (s.name, s.kind))
            .collect();
        assert_eq!(statuses, [(a, SetKind::RuleSet), (b, SetKind::RuleSet)]);
    }

    /// And the other way round: the role the profile gives the file is the
    /// one that stays.
    #[tokio::test]
    async fn a_rule_set_nested_in_a_file_cannot_reuse_a_domain_set() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.list"), BOTH_KINDS).unwrap();
        std::fs::write(dir.path().join("b.list"), "RULE-SET,a.list\nDOMAIN,b.com\n").unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nDOMAIN-SET,a.list,P\nRULE-SET,b.list,REJECT\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert_eq!(
            codes_of(&diags),
            [codes::W_SET_LINES_SKIPPED, codes::W_SET_KIND_CONFLICT]
        );
        let a = dir.path().join("a.list").display().to_string();
        let b = dir.path().join("b.list").display().to_string();
        assert_eq!(
            diags.iter().last().map(|d| d.message.as_str()),
            Some(
                format!(
                    "set `{a}` (referenced in `{b}`) ignored: the same resource is used as a DOMAIN-SET"
                )
                .as_str()
            )
        );
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "domain-set.com").await.0, "P");
        assert_eq!(decide(&engine, "rule.com").await.0, "DIRECT");
        assert_eq!(decide(&engine, "b.com").await.0, "REJECT");
        let kinds: Vec<SetKind> = reg
            .statuses()
            .into_iter()
            .filter(|s| s.name == a)
            .map(|s| s.kind)
            .collect();
        assert_eq!(kinds, [SetKind::DomainSet]);
    }

    /// The role the profile itself gives a file wins, also when a set file
    /// written above it nests the file in the other role. Which role stays
    /// must not depend on which set files have content while the registry is
    /// built, so it is the profile's own that is looked up, not the entry
    /// that happens to exist first.
    #[tokio::test]
    async fn the_role_the_profile_gives_a_file_wins_over_one_nested_above_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.list"), BOTH_KINDS).unwrap();
        std::fs::write(
            dir.path().join("b.list"),
            "DOMAIN-SET,a.list\nDOMAIN,b.com\n",
        )
        .unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,b.list,REJECT\nRULE-SET,a.list,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        // `b.list` is compiled first, and its nested reference is the one left
        // out; `a.list` is then read once, as a rule set, which skips `domain-set.com`
        assert_eq!(
            codes_of(&diags),
            [codes::W_SET_KIND_CONFLICT, codes::W_SET_LINES_SKIPPED]
        );
        let a = dir.path().join("a.list").display().to_string();
        let b = dir.path().join("b.list").display().to_string();
        assert_eq!(
            diags.iter().next().map(|d| d.message.as_str()),
            Some(
                format!(
                    "set `{a}` (referenced in `{b}`) ignored: the same resource is used as a RULE-SET"
                )
                .as_str()
            )
        );
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "rule.com").await.0, "P");
        // read as a domain set, `a.list` would have matched this one
        assert_eq!(decide(&engine, "domain-set.com").await.0, "DIRECT");
        assert_eq!(decide(&engine, "b.com").await.0, "REJECT");
        let statuses: Vec<(String, SetKind)> = reg
            .statuses()
            .into_iter()
            .map(|s| (s.name, s.kind))
            .collect();
        assert_eq!(statuses, [(a, SetKind::RuleSet), (b, SetKind::RuleSet)]);
    }

    /// The sub-rules of a logical rule give a file its role too, at any depth:
    /// a file the profile names only inside an `AND`, an `OR` or a `NOT` keeps
    /// that role against a set file written above it that nests the file in
    /// the other. `b.list` is compiled first, so without the claim the nested
    /// reference would be the one to register `a.list`, as a domain set.
    #[tokio::test]
    async fn the_role_a_logical_rule_gives_a_file_wins_too() {
        // What each rule decides for the names beside it. `rule.com` is a line
        // of `a.list` read as a rule set, so the rules that need the set to
        // match decide `P`; with the empty set a reference that loses turns
        // into, `FINAL` would decide. `NOT` is the other way round: with the
        // set it does not match `rule.com` (`FINAL` decides), with the empty
        // set it would, as it would match any name. `other.com` is not in the
        // set and shows that the `NOT` can match at all, so the `FINAL` for
        // `rule.com` is the set's doing.
        let rules: [(&str, &[(&str, &str)]); 4] = [
            (
                "AND,((RULE-SET,a.list),(DOMAIN-SUFFIX,com)),P",
                &[("rule.com", "P")],
            ),
            (
                "OR,((RULE-SET,a.list),(DOMAIN,never.example)),P",
                &[("rule.com", "P")],
            ),
            (
                "NOT,((RULE-SET,a.list)),P",
                &[("rule.com", "DIRECT"), ("other.com", "P")],
            ),
            (
                "AND,((OR,((RULE-SET,a.list),(DOMAIN,never.example))),(DOMAIN-SUFFIX,com)),P",
                &[("rule.com", "P")],
            ),
        ];
        for (rule, decides) in rules {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("a.list"), BOTH_KINDS).unwrap();
            std::fs::write(
                dir.path().join("b.list"),
                "DOMAIN-SET,a.list\nDOMAIN,b.com\n",
            )
            .unwrap();
            let cfg = load(
                &format!(
                    "[Proxy]\nP = direct\n[Rule]\nRULE-SET,b.list,REJECT\n{rule}\nFINAL,DIRECT\n"
                ),
                dir.path(),
            );
            let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
            let a = dir.path().join("a.list").display().to_string();
            let b = dir.path().join("b.list").display().to_string();
            // the reference nested in `b.list` is the one left out, and the
            // only one
            let said = format!(
                "set `{a}` (referenced in `{b}`) ignored: the same resource is used as a RULE-SET"
            );
            let left_out: Vec<&str> = diags
                .iter()
                .filter(|d| d.code == codes::W_SET_KIND_CONFLICT)
                .map(|d| d.message.as_str())
                .collect();
            assert_eq!(left_out, [said.as_str()], "{rule}");
            // and `a.list` is read once, as a rule set
            let kinds: Vec<SetKind> = reg
                .statuses()
                .into_iter()
                .filter(|s| s.name == a)
                .map(|s| s.kind)
                .collect();
            assert_eq!(kinds, [SetKind::RuleSet], "{rule}");
            let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
            for (name, policy) in decides {
                assert_eq!(decide(&engine, name).await.0, *policy, "{rule}: {name}");
            }
        }
    }

    /// The same when the profile's claim is a `[Host]` entry: the entries of
    /// `[Host]` are registered after all of the rules, so here the claim is
    /// there before the entry is.
    #[tokio::test]
    async fn the_role_a_host_entry_gives_a_file_wins_over_one_nested_in_a_rule() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.list"), BOTH_KINDS).unwrap();
        std::fs::write(
            dir.path().join("b.list"),
            "DOMAIN-SET,a.list\nDOMAIN,b.com\n",
        )
        .unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,b.list,REJECT\nFINAL,DIRECT\n\
[Host]\nRULE-SET:a.list = 1.2.3.4\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert_eq!(
            codes_of(&diags),
            [codes::W_SET_KIND_CONFLICT, codes::W_SET_LINES_SKIPPED]
        );
        let a = dir.path().join("a.list").display().to_string();
        let b = dir.path().join("b.list").display().to_string();
        assert_eq!(
            diags.iter().next().map(|d| d.message.as_str()),
            Some(
                format!(
                    "set `{a}` (referenced in `{b}`) ignored: the same resource is used as a RULE-SET"
                )
                .as_str()
            )
        );
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "domain-set.com").await.0, "DIRECT");
        assert_eq!(decide(&engine, "b.com").await.0, "REJECT");
        // the entry's set has its line, as a rule set
        let host_set = reg
            .statuses()
            .into_iter()
            .filter(|s| s.name == a)
            .map(|s| (s.kind, s.entries))
            .collect::<Vec<_>>();
        assert_eq!(host_set, [(SetKind::RuleSet, 1)]);
    }

    /// A claim made only by a line of an inline set that no rule references
    /// holds as well, although nothing ever creates the entry it claims.
    #[tokio::test]
    async fn the_role_an_unreferenced_inline_set_gives_a_file_wins_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.list"), BOTH_KINDS).unwrap();
        std::fs::write(
            dir.path().join("b.list"),
            "DOMAIN-SET,a.list\nDOMAIN,b.com\n",
        )
        .unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,b.list,REJECT\nFINAL,DIRECT\n\
[Ruleset Claim]\nRULE-SET,a.list\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert_eq!(codes_of(&diags), [codes::W_SET_KIND_CONFLICT]);
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "domain-set.com").await.0, "DIRECT");
        assert_eq!(decide(&engine, "b.com").await.0, "REJECT");
        // `a.list` is claimed and never registered: `Claim` is not referenced
        let names: Vec<String> = reg.statuses().into_iter().map(|s| s.name).collect();
        assert_eq!(names, [dir.path().join("b.list").display().to_string()]);
    }

    /// So does a claim made only by a rule that stands after the first FINAL
    /// and is never evaluated: the roles are read from every rule written,
    /// the way the loader reads them, although the set of such a rule is not
    /// registered.
    #[tokio::test]
    async fn the_role_a_rule_after_the_first_final_gives_a_file_wins_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.list"), BOTH_KINDS).unwrap();
        std::fs::write(dir.path().join("b.list"), "RULE-SET,a.list\nDOMAIN,b.com\n").unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,b.list,REJECT\nFINAL,DIRECT\n\
DOMAIN-SET,a.list,P\nFINAL,P\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert_eq!(codes_of(&diags), [codes::W_SET_KIND_CONFLICT]);
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "b.com").await.0, "REJECT");
        // the nested reference is left out: `rule.com` falls to the last FINAL
        assert_eq!(decide(&engine, "rule.com").await.0, "P");
        // `a.list` is claimed and never registered: its rule is not evaluated
        let names: Vec<String> = reg.statuses().into_iter().map(|s| s.name).collect();
        assert_eq!(names, [dir.path().join("b.list").display().to_string()]);
    }

    /// A parent set that is reloaded later compiles its nested references
    /// then, and reaches the same verdict: the profile's claim is kept for
    /// the registry's whole life.
    #[tokio::test]
    async fn a_reloaded_set_loses_its_nested_reference_to_the_profiles_role_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.list"), BOTH_KINDS).unwrap();
        let b_file = dir.path().join("b.list");
        std::fs::write(&b_file, "DOMAIN,b.com\n").unwrap();
        // `Claim` is referenced by no rule: nothing registers `a.list`
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,b.list,REJECT\nFINAL,DIRECT\n\
[Ruleset Claim]\nRULE-SET,a.list\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert!(diags.is_empty(), "{:?}", codes_of(&diags));
        let handle = reg.get(&ResourceRef::File(b_file.clone()), SetKind::RuleSet);
        let v1 = handle.version();
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "domain-set.com").await.0, "DIRECT");
        std::fs::write(&b_file, "DOMAIN-SET,a.list\nDOMAIN,b.com\n").unwrap();
        wait_version(&handle, v1 + 1).await;
        assert_eq!(decide(&engine, "domain-set.com").await.0, "DIRECT");
        assert_eq!(decide(&engine, "b.com").await.0, "REJECT");
        let names: Vec<String> = reg.statuses().into_iter().map(|s| s.name).collect();
        assert_eq!(names, [b_file.display().to_string()]);
    }

    /// A file that nests itself as the other kind is a cycle, as it always
    /// was: the cycle is looked for first.
    #[tokio::test]
    async fn a_file_nesting_itself_as_the_other_kind_is_still_a_cycle() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.list"),
            "DOMAIN-SET,a.list\nDOMAIN,a.com\n",
        )
        .unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,a.list,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert_eq!(codes_of(&diags), [codes::W_SET_NESTING]);
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "a.com").await.0, "P");
    }

    /// The warning names a URL the way the statuses do: its user name and
    /// password are `***`, nowhere else.
    #[tokio::test]
    async fn the_warning_for_a_nested_url_hides_its_user_info() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://al1ce:S3CR3T@sets.example/gh/user/repo@main/a.list";
        std::fs::write(
            dir.path().join("b.list"),
            format!("DOMAIN-SET,{url}\nDOMAIN,b.com\n"),
        )
        .unwrap();
        let cfg = load(
            &format!(
                "[Proxy]\nP = direct\n[Rule]\nRULE-SET,{url},P\nRULE-SET,b.list,REJECT\nFINAL,DIRECT\n"
            ),
            dir.path(),
        );
        let (_reg, diags) = SetRegistry::build(&cfg, offline_manager(dir.path()), dir.path());
        let texts: Vec<(&str, &str)> = diags.iter().map(|d| (d.code, d.message.as_str())).collect();
        let said = format!(
            "set `https://***@sets.example/gh/user/repo@main/a.list` (referenced in `{}`) \
ignored: the same resource is used as a RULE-SET",
            dir.path().join("b.list").display()
        );
        assert!(
            texts.contains(&(codes::W_SET_KIND_CONFLICT, said.as_str())),
            "{texts:?}"
        );
        for (_, text) in texts {
            assert!(
                !text.contains("S3CR3T") && !text.contains("al1ce"),
                "{text}"
            );
        }
    }

    /// The DEBUG line of a hit, the `notes` of a request and the API are all
    /// built from two strings of the hit: the line that matched and the name
    /// of the set it was found in. Neither holds the user name or the
    /// password of a URL — not when the set was fetched from it, and not when
    /// the matching line is itself a reference to it.
    #[tokio::test]
    async fn a_hit_shows_no_user_info_in_the_set_or_in_the_line() {
        const MARKER: &str = "S3CR3T-hit";
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let port = server.url("/").port().unwrap();
        server.set("/fetched.list", "DOMAIN,fetched.com\n");
        server.set("/inner.list", "DOMAIN,inner.com\n");
        let fetched = format!("http://al1ce:{MARKER}@127.0.0.1:{port}/fetched.list");
        let inner = format!("http://al1ce:{MARKER}@127.0.0.1:{port}/inner.list");
        std::fs::write(dir.path().join("outer.list"), format!("RULE-SET,{inner}\n")).unwrap();
        let cfg = load(
            &format!(
                "[Proxy]\nP = direct\n[Rule]\nRULE-SET,{fetched},P\nRULE-SET,outer.list,P\nFINAL,DIRECT\n"
            ),
            dir.path(),
        );
        let (reg, _) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        for url in [&fetched, &inner] {
            let handle = reg.get(&ResourceRef::Url(url.clone()), SetKind::RuleSet);
            wait_version(&handle, 1).await;
        }
        let outer = dir.path().join("outer.list").display().to_string();
        let cases = [
            (
                "fetched.com",
                format!("http://***@127.0.0.1:{port}/fetched.list"),
                "DOMAIN,fetched.com".to_string(),
            ),
            (
                "inner.com",
                outer,
                format!("RULE-SET,http://***@127.0.0.1:{port}/inner.list"),
            ),
        ];
        for (host, set, entry) in cases {
            let d = engine
                .evaluate(
                    &SessionInfo::tcp(HostName::parse(host), 443),
                    OutboundMode::Rule,
                    &crate::engine::NoResolve,
                )
                .await;
            let hit = d.sub_rule.unwrap_or_else(|| panic!("{host} has no hit"));
            assert_eq!((hit.set.as_str(), hit.entry.as_str()), (&*set, &*entry));
            for shown in [&hit.set, &hit.entry] {
                assert!(
                    !shown.contains(MARKER) && !shown.contains("al1ce"),
                    "{shown}"
                );
            }
        }
    }

    /// The same resource in the same role, however often and from wherever,
    /// is one set and no conflict.
    #[tokio::test]
    async fn the_same_resource_in_the_same_role_is_one_set() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.list"), "DOMAIN,a.com\n").unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,a.list,P\nRULE-SET,a.list,REJECT\nFINAL,DIRECT\n\
[Host]\nRULE-SET:a.list = 1.2.3.4\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert!(diags.is_empty(), "{:?}", codes_of(&diags));
        assert_eq!(reg.statuses().len(), 1);
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "a.com").await.0, "P");
    }

    /// F1 regression: a hot reload used to clear the compile stack and
    /// recompile without pushing the key being recompiled, so a set whose
    /// new content references itself captured its own (still empty, about
    /// to be overwritten) handle instead of being caught as a cycle —
    /// `handle.store(set)` then made the set contain a live `Arc` cycle back
    /// to itself, and evaluating through it recursed forever.
    #[tokio::test]
    async fn hot_reload_self_reference_is_a_cycle_not_infinite_recursion() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.list");
        std::fs::write(&file, "DOMAIN,a.com\n").unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,a.list,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert!(diags.is_empty(), "{:?}", codes_of(&diags));
        let handle = reg.get(&ResourceRef::File(file.clone()), SetKind::RuleSet);
        let v1 = handle.version();
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        assert_eq!(decide(&engine, "a.com").await.0, "P");
        std::fs::write(&file, "RULE-SET,a.list\nDOMAIN,b.com\n").unwrap();
        wait_version(&handle, v1 + 1).await;
        // The self-reference in the reloaded content is caught as a cycle
        // (an empty stub, not a live handle back to itself): the sibling
        // DOMAIN line still matches, and a miss falls all the way through
        // to FINAL instead of recursing.
        assert_eq!(decide(&engine, "b.com").await.0, "P");
        assert_eq!(decide(&engine, "zzz.com").await.0, "DIRECT");
    }

    /// F1 regression, cross-file case: neither `a.list` nor `b.list` is
    /// self-referential (so each reload's own compile stack — scoped to
    /// just that file — never sees a cycle), but rewriting both to
    /// reference each other and letting both reload independently still
    /// closes a live cycle across the two handles. The per-reload cycle
    /// check cannot see this (it only knows about the file it is currently
    /// recompiling); `EvalCtx::set_depth` (matcher.rs) is the runtime
    /// backstop that keeps `evaluate` from recursing forever regardless.
    #[tokio::test]
    async fn mutual_hot_reload_cycle_terminates() {
        let dir = tempfile::tempdir().unwrap();
        let a_file = dir.path().join("a.list");
        let b_file = dir.path().join("b.list");
        std::fs::write(&a_file, "DOMAIN,a.com\n").unwrap();
        std::fs::write(&b_file, "DOMAIN,b.com\n").unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,a.list,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert!(diags.is_empty(), "{:?}", codes_of(&diags));
        let a_handle = reg.get(&ResourceRef::File(a_file.clone()), SetKind::RuleSet);
        let b_handle = reg.get(&ResourceRef::File(b_file.clone()), SetKind::RuleSet);
        let (a_v1, b_v1) = (a_handle.version(), b_handle.version());
        let engine = RuleEngine::build(&cfg, reg.as_ref(), Arc::new(NoGeo)).unwrap();
        std::fs::write(&a_file, "RULE-SET,b.list\n").unwrap();
        std::fs::write(&b_file, "RULE-SET,a.list\n").unwrap();
        wait_version(&a_handle, a_v1 + 1).await;
        wait_version(&b_handle, b_v1 + 1).await;
        // Neither set has a direct domain/IP entry left, so a miss must
        // bounce between the two handles; it should terminate (bounded by
        // `MAX_NESTING`) and fall through to FINAL rather than hang or
        // stack-overflow.
        let d = tokio::time::timeout(Duration::from_secs(5), decide(&engine, "zzz.com"))
            .await
            .expect("evaluate must terminate, not hang");
        assert_eq!(d.0, "DIRECT");
    }

    /// F4 regression: `push_diag` used to keep accumulating into the
    /// registry's own `diags` after `build` had already taken them, so every
    /// later reload that skipped a line leaked a diagnostic nobody ever read.
    #[tokio::test]
    async fn diagnostics_do_not_leak_into_the_registry_after_build() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.list");
        std::fs::write(&file, "DOMAIN,a.com\n").unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,a.list,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        assert!(diags.is_empty(), "{:?}", codes_of(&diags));
        assert_eq!(reg.diag_count(), 0);
        let handle = reg.get(&ResourceRef::File(file.clone()), SetKind::RuleSet);
        let v1 = handle.version();
        std::fs::write(&file, "DOMAIN,b.com\nnot a rule\n").unwrap();
        wait_version(&handle, v1 + 1).await;
        // `statuses()` still reports the skip; the registry's internal
        // (build-only) diagnostics stay empty.
        assert_eq!(reg.diag_count(), 0);
        let st = reg
            .statuses()
            .into_iter()
            .find(|s| s.name.ends_with("a.list"))
            .unwrap();
        assert_eq!(st.skipped, 1);
    }

    fn found(skipped: usize, truncated: usize, unknown_params: usize) -> Unused {
        Unused {
            skipped,
            truncated,
            unknown_params,
        }
    }

    /// When a load of a set is worth a line in the log, case by case.
    #[test]
    fn a_load_is_worth_a_warning_when_it_found_what_the_one_before_did_not() {
        let nothing = Unused::default();
        // a set that had not loaded
        assert!(!worth_a_warning(None, nothing));
        for now in [found(3, 0, 0), found(0, 1, 0), found(0, 0, 2)] {
            assert!(worth_a_warning(None, now), "{now:?}");
        }
        // after a load that found nothing
        assert!(!worth_a_warning(Some(nothing), nothing));
        assert!(worth_a_warning(Some(nothing), found(3, 0, 0)));
        // after one that found the same: nothing more to say
        for same in [
            found(3, 0, 0),
            found(0, 7, 0),
            found(0, 0, 2),
            found(3, 7, 2),
        ] {
            assert!(!worth_a_warning(Some(same), same), "{same:?}");
        }
        // any of the three numbers changing is news, up or down
        let before = found(3, 7, 2);
        for now in [
            found(4, 7, 2),
            found(2, 7, 2),
            found(3, 8, 2),
            found(3, 6, 2),
            found(3, 7, 3),
            found(3, 7, 1),
            found(0, 7, 2),
            found(3, 0, 2),
            found(3, 7, 0),
        ] {
            assert!(worth_a_warning(Some(before), now), "{now:?}");
        }
        // and coming back to nothing is not
        assert!(!worth_a_warning(Some(before), nothing));
    }

    /// Three lines nobody can read, then none, then three again: said twice,
    /// and not in between.
    #[test]
    fn what_goes_away_and_comes_back_is_said_each_time_it_comes() {
        let mut before = None;
        let said: Vec<bool> = [found(3, 0, 0), Unused::default(), found(3, 0, 0)]
            .into_iter()
            .map(|now| {
                let say = worth_a_warning(before, now);
                before = Some(now);
                say
            })
            .collect();
        assert_eq!(said, [true, false, true]);
    }

    /// How many lines the sets of the tests that compile one at a time have:
    /// enough for a stretch of parsing and compiling to last for long beside
    /// the few milliseconds that a download of the other set takes, and few
    /// enough for two of them in a row to be done long before the tests give
    /// up waiting, on a machine several times slower than the one they were
    /// written on.
    const MANY: usize = 100_000;

    /// A rule set of `n` lines, with names no other set of the tests has.
    fn many_lines(prefix: &str, n: usize) -> String {
        let mut text = String::with_capacity(n * 32);
        for i in 0..n {
            text += &format!("DOMAIN-SUFFIX,{prefix}{i}.example\n");
        }
        text
    }

    /// The registry is at no stretch of compiling, once the last set to be
    /// stored has left its own.
    async fn compiling_ends(reg: &SetRegistry) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while reg.compiling_now() != 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the compiling did not end");
    }

    /// A set of a million lines needs about three times its size while it is
    /// compiled, and a few dozen sets get their new content together when the
    /// profile starts cold or all of them are updated: they are compiled one
    /// after the other.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sets_that_change_together_are_compiled_one_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/a.list", "DOMAIN,old-a.example\n");
        server.set("/b.list", "DOMAIN,old-b.example\n");
        let (a, b) = (server.url("/a.list"), server.url("/b.list"));
        let cfg = load(
            &format!("[Proxy]\nP = direct\n[Rule]\nRULE-SET,{a},P\nRULE-SET,{b},P\nFINAL,DIRECT\n"),
            dir.path(),
        );
        let mgr = manager(dir.path());
        let (reg, _) = SetRegistry::build(&cfg, mgr.clone(), dir.path());
        let handle_of = |url: &Url| reg.get(&ResourceRef::Url(url.to_string()), SetKind::RuleSet);
        let (set_a, set_b) = (handle_of(&a), handle_of(&b));
        wait_version(&set_a, 1).await;
        wait_version(&set_b, 1).await;
        compiling_ends(&reg).await;

        // both are served anew, and big, and asked for together
        server.set("/a.list", many_lines("a", MANY));
        server.set("/b.list", many_lines("b", MANY));
        assert!(mgr.force_update(&ResourceSource::Url(a.clone())));
        assert!(mgr.force_update(&ResourceSource::Url(b.clone())));
        wait_version(&set_a, 2).await;
        wait_version(&set_b, 2).await;
        compiling_ends(&reg).await;

        assert_eq!(reg.most_compiling_at_once(), 1);
        assert_eq!(set_a.load().entry_count(), MANY);
        assert_eq!(set_b.load().entry_count(), MANY);
    }

    /// The reload task of a set that is downloaded while the registry is
    /// being built waits for the build, which is compiling another set.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_set_that_arrives_while_the_registry_is_built_waits_for_the_build() {
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/a.list", "DOMAIN,a.example\n");
        let a = server.url("/a.list");
        // the build asks for the URL first and compiles the big file after it,
        // while the download lands
        let big = dir.path().join("big.list");
        std::fs::write(&big, many_lines("big", MANY)).unwrap();
        let cfg = load(
            &format!(
                "[Proxy]\nP = direct\n[Rule]\nRULE-SET,{a},P\nRULE-SET,big.list,P\nFINAL,DIRECT\n"
            ),
            dir.path(),
        );
        let (reg, _) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        let set_a = reg.get(&ResourceRef::Url(a.to_string()), SetKind::RuleSet);
        let set_big = reg.get(&ResourceRef::File(big), SetKind::RuleSet);
        wait_version(&set_a, 1).await;
        compiling_ends(&reg).await;
        assert_eq!(reg.most_compiling_at_once(), 1);
        assert_eq!(set_a.load().entry_count(), 1);
        assert_eq!(set_big.load().entry_count(), MANY);
    }

    const ONE: &str = "DOMAIN,one.example\n";
    const TWO: &str = "DOMAIN,one.example\nDOMAIN,two.example\n";
    const THREE: &str = "DOMAIN,one.example\nDOMAIN,two.example\nDOMAIN,three.example\n";

    /// A registry with one rule set, served at `/a.list` with `ONE` in it and
    /// loaded: the registry, its manager, the set and the URL of the set.
    async fn one_set_loaded(
        server: &TestServer,
        dir: &Path,
    ) -> (Arc<SetRegistry>, Arc<ResourceManager>, SetHandle, Url) {
        server.set("/a.list", ONE);
        let url = server.url("/a.list");
        let cfg = load(
            &format!("[Proxy]\nP = direct\n[Rule]\nRULE-SET,{url},P\nFINAL,DIRECT\n"),
            dir,
        );
        let mgr = manager(dir);
        let (reg, _) = SetRegistry::build(&cfg, mgr.clone(), dir);
        let set = reg.get(&ResourceRef::Url(url.to_string()), SetKind::RuleSet);
        wait_version(&set, 1).await;
        compiling_ends(&reg).await;
        (reg, mgr, set, url)
    }

    /// The server gives `body` at `/a.list` from now on, and the manager is
    /// asked for it. Returns once the manager holds it as this `version`.
    async fn served_anew(
        server: &TestServer,
        mgr: &ResourceManager,
        url: &Url,
        body: &'static str,
        version: u64,
    ) {
        server.set("/a.list", body);
        assert!(mgr.force_update(&ResourceSource::Url(url.clone())));
        holds(mgr, url, version, "available").await;
    }

    /// The manager holds this version of the resource at `url`, in this state.
    async fn holds(mgr: &ResourceManager, url: &Url, version: u64, state: &str) {
        let key = resource_key(&ResourceSource::Url(url.clone()));
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let held = mgr.statuses().into_iter().find(|s| s.key == key);
                if held.is_some_and(|s| s.version == version && s.state == state) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the manager did not get there");
    }

    /// A version that is followed by a failed attempt before the set has had
    /// its turn was announced once, and is the copy a failed resource keeps
    /// from then on: the set is compiled from it all the same.
    #[tokio::test]
    async fn a_version_followed_by_a_failed_attempt_before_the_turn_of_the_set_is_compiled() {
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let (reg, mgr, set, url) = one_set_loaded(&server, dir.path()).await;
        // the turn of the set does not come while the test has it
        let turn = reg.compile_lock.lock().await;
        served_anew(&server, &mgr, &url, TWO, 2).await;
        server.set_status("/a.list", 500);
        assert!(mgr.force_update(&ResourceSource::Url(url.clone())));
        holds(&mgr, &url, 2, "failed").await;
        assert_eq!(set.version(), 1);
        drop(turn);
        wait_version(&set, 2).await;
        assert_eq!(set.load().entry_count(), 2);
    }

    /// Two versions that arrive while a set waits for its turn are one
    /// compile, of the later one: the resource is read when the turn comes,
    /// and the version that was replaced meanwhile is never compiled.
    #[tokio::test]
    async fn a_version_that_is_replaced_before_the_turn_of_the_set_is_never_compiled() {
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let (reg, mgr, set, url) = one_set_loaded(&server, dir.path()).await;
        let begun = reg.stretches_begun();
        let turn = reg.compile_lock.lock().await;
        served_anew(&server, &mgr, &url, TWO, 2).await;
        served_anew(&server, &mgr, &url, THREE, 3).await;
        assert_eq!(set.version(), 1);
        drop(turn);
        wait_version(&set, 3).await;
        compiling_ends(&reg).await;
        assert_eq!(reg.stretches_begun(), begun + 1);
        assert_eq!(set.load().entry_count(), 3);
    }

    /// The reload task of a set that waits for its turn has the lock and not
    /// the registry: a registry that was replaced goes away at once, and the
    /// task ends when its turn comes.
    #[tokio::test]
    async fn a_set_that_waits_for_its_turn_does_not_keep_a_registry_that_was_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let (reg, mgr, _set, url) = one_set_loaded(&server, dir.path()).await;
        let lock = reg.compile_lock.clone();
        let registry = Arc::downgrade(&reg);
        let turn = lock.lock().await;
        served_anew(&server, &mgr, &url, TWO, 2).await;
        // the task has been told of the version: it gets the time to come to
        // where it waits, whatever it does on the way
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        drop(reg);
        assert!(
            registry.upgrade().is_none(),
            "kept by a task that waits for its turn"
        );
        drop(turn);
        // the task finds no registry and ends: the test alone has the lock
        tokio::time::timeout(Duration::from_secs(10), async {
            while Arc::strong_count(&lock) != 1 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the task did not end");
    }

    /// The three numbers keep their places from the parser to the status.
    #[tokio::test]
    async fn the_three_numbers_keep_their_places_from_the_parser_to_the_status() {
        let parsed = ParsedSet {
            skipped_total: 1,
            truncated: 2,
            unknown_params: 3,
            ..ParsedSet::default()
        };
        assert_eq!(Unused::of(&parsed), found(1, 2, 3));

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.list"), ONE).unwrap();
        let cfg = load(
            "[Proxy]\nP = direct\n[Rule]\nRULE-SET,a.list,P\nFINAL,DIRECT\n",
            dir.path(),
        );
        let (reg, _) = SetRegistry::build(&cfg, manager(dir.path()), dir.path());
        for entry in reg.entries.lock().unwrap().values_mut() {
            entry.unused = Some(found(1, 2, 3));
        }
        let statuses = reg.statuses();
        assert_eq!(statuses.len(), 1);
        assert_eq!(
            (
                statuses[0].skipped,
                statuses[0].truncated,
                statuses[0].unknown_params
            ),
            (1, 2, 3)
        );
    }

    /// A rule set whose lines carry a parameter nobody knows: line 3 is the
    /// first, and the second is a line copied from a profile with a password
    /// in it. The file is arbitrary text, so what the lines carry is not for
    /// a message to quote.
    const UNKNOWN_PARAMS: &str = "\
# a list
DOMAIN,a.example
DOMAIN,b.example,S3CR3T

IP-CIDR,10.0.0.0/8,no-resolve,password=S3CR3T
DOMAIN,c.example,no-resolve
";

    /// The text the registry says of `UNKNOWN_PARAMS` as the set `name`.
    fn unknown_params_said_of(name: &str) -> String {
        format!(
            "set `{name}`: 2 line(s) carry a parameter that is not known and is ignored (first: line 3)"
        )
    }

    /// The `update-interval` of a `RULE-SET` line is the interval its resource
    /// is refreshed at: the next refresh falls around it, around a day when the
    /// line does not say.
    #[tokio::test]
    async fn the_update_interval_of_a_rule_line_is_the_interval_of_its_resource() {
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let (ten_minutes, a_day) = (server.url("/ten.list"), server.url("/day.list"));
        for url in [&ten_minutes, &a_day] {
            seed_cache(dir.path(), url, b"DOMAIN,a.example\n", SystemTime::now());
        }
        let cfg = load(
            &format!(
                "[Proxy]\nP = direct\n[Rule]\nRULE-SET,{ten_minutes},P,update-interval=600\nRULE-SET,{a_day},P\nFINAL,DIRECT\n"
            ),
            dir.path(),
        );
        let mgr = manager(dir.path());
        let (_registry, _) = SetRegistry::build(&cfg, mgr.clone(), dir.path());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !(mgr.statuses().len() == 2
            && mgr.statuses().iter().all(|s| s.next_refresh.is_some()))
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the refreshes were never scheduled"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let refresh_in = |url: &Url| {
            let source = rurge_net::resource::ResourceSource::Url(url.clone());
            let status = mgr.statuses().into_iter().find(|s| s.source == source);
            let at = status.expect("a status").next_refresh.expect("scheduled");
            at.duration_since(SystemTime::now()).unwrap_or_default()
        };
        // the time is spread by a tenth to either side of the interval
        let ten = refresh_in(&ten_minutes);
        assert!(
            ten >= Duration::from_secs(500) && ten <= Duration::from_secs(700),
            "{ten:?}"
        );
        assert!(refresh_in(&a_day) >= Duration::from_secs(70_000));
    }

    /// The set is in the cache (or a file) when the registry is built: one
    /// `W0003`, with the name of the set and the number of the first line and
    /// with nothing the lines carry, in the message or in the hint.
    #[tokio::test]
    async fn a_set_with_lines_that_carry_an_unknown_parameter_is_warned_of_when_built() {
        let dir = tempfile::tempdir().unwrap();
        let url = Url::parse("http://sets.example/a.list").unwrap();
        seed_cache(
            dir.path(),
            &url,
            UNKNOWN_PARAMS.as_bytes(),
            SystemTime::now() - Duration::from_secs(600),
        );
        let file = dir.path().join("b.list");
        std::fs::write(&file, UNKNOWN_PARAMS).unwrap();
        let cfg = load(
            &format!(
                "[Proxy]\nP = direct\n[Rule]\nRULE-SET,{url},P\nRULE-SET,b.list,P\nFINAL,DIRECT\n"
            ),
            dir.path(),
        );
        let (reg, diags) = SetRegistry::build(&cfg, offline_manager(dir.path()), dir.path());
        let mut said: Vec<String> = Vec::new();
        for d in diags.iter() {
            assert_eq!(d.code, codes::W_UNKNOWN_RULE_PARAM);
            assert_eq!(d.severity, rurge_config::Severity::Warning);
            // the code's own hint, which quotes nothing
            assert_eq!(d.hint, codes::hint(codes::W_UNKNOWN_RULE_PARAM));
            assert!(d.span.is_none());
            let hint = d.hint.unwrap_or_default();
            assert!(!d.message.contains("S3CR3T") && !hint.contains("S3CR3T"));
            said.push(d.message.clone());
        }
        said.sort();
        let mut expected = vec![
            unknown_params_said_of(url.as_str()),
            unknown_params_said_of(&file.display().to_string()),
        ];
        expected.sort();
        assert_eq!(said, expected);

        // the status counts them, and gives the key the resource goes by
        let statuses = reg.statuses();
        let at = |name: &str| statuses.iter().find(|s| s.name.ends_with(name)).unwrap();
        assert_eq!((at("a.list").unknown_params, at("a.list").skipped), (2, 0));
        assert_eq!(at("b.list").unknown_params, 2);
        assert_eq!(
            at("a.list").key,
            Some(resource_key(&ResourceSource::Url(url.clone())))
        );
        assert_eq!(
            at("b.list").key,
            Some(resource_key(&ResourceSource::File(file.clone())))
        );
        // the set is read whole: the line is a rule, only its parameter is lost
        assert_eq!(at("a.list").entries, 4);
    }

    /// An inline set has been read by the loader, which warned of the
    /// parameter at its line: the registry does not say it a second time. Like
    /// a built-in set, it has no resource and so no key.
    #[tokio::test]
    async fn inline_and_built_in_sets_have_no_key_and_no_second_warning() {
        let dir = tempfile::tempdir().unwrap();
        let text = "[Proxy]\nP = direct\n[Rule]\nRULE-SET,SYSTEM,DIRECT\nRULE-SET,Mine,P\nFINAL,DIRECT\n\
[Ruleset Mine]\nDOMAIN,a.example,S3CR3T\n";
        let loaded = from_text(text, &dir.path().join("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        // the loader's warning, at the line, once
        let from_the_loader = loaded
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_UNKNOWN_RULE_PARAM)
            .count();
        assert_eq!(from_the_loader, 1);
        let (reg, diags) = SetRegistry::build(&loaded.config, manager(dir.path()), dir.path());
        assert!(diags.is_empty(), "{:?}", codes_of(&diags));
        let statuses = reg.statuses();
        let names: Vec<&str> = statuses.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Mine", "SYSTEM"]);
        for status in &statuses {
            assert_eq!(status.key, None, "{}", status.name);
            assert_eq!(
                (status.skipped, status.truncated, status.unknown_params),
                (0, 0, 0),
                "{}",
                status.name
            );
        }
    }
}
