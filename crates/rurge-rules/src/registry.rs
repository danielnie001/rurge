//! Set registry (M2 design §6.2): one `SetHandle` per (resource, kind).
//! Internal and inline sets compile once; external sets come from the
//! `ResourceManager` and are recompiled and swapped whenever the resource
//! changes. Nested references are resolved through the same registry with
//! cycle detection and a nesting limit of 8. A file or URL is read in one
//! format only, the one it is first registered in: a later reference to it as
//! the other kind of set is left out with a warning.

use crate::matcher::{SetLookup, SetRef};
use crate::set::{CompiledSet, SetHandle};
use crate::set_format::{ParsedSet, SetKind, SetLine, internal_set_text, parse_set};
use rurge_config::rule::{InternalSet, ParseCtx, ResourceRef, RuleKind, SubRule};
use rurge_config::{Config, Diagnostic, Diagnostics, HostKey, codes};
use rurge_net::resource::{
    ResourceHandle, ResourceManager, ResourceSource, ResourceSpec, ResourceState,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
    pub kind: SetKind,
    pub entries: usize,
    pub version: u64,
    /// `internal` | `inline` | `ok` | `missing` | `failed: <error>` | `cycle`
    pub state: String,
    pub skipped: usize,
    pub truncated: usize,
}

struct Entry {
    handle: SetHandle,
    resource: Option<ResourceHandle>,
    skipped: usize,
    truncated: usize,
    state: String,
}

pub struct SetRegistry {
    resources: Arc<ResourceManager>,
    base_dir: PathBuf,
    inline: HashMap<String, Vec<SubRule>>,
    inline_names: HashSet<String>,
    entries: Mutex<HashMap<(Key, SetKind), Entry>>,
    diags: Mutex<Diagnostics>,
    /// `true` only while `build` is running; `push_diag` is a no-op outside
    /// that window so reloads (which already log through `tracing`) don't
    /// keep accumulating diagnostics that `build`'s caller already took and
    /// nobody reads again (F4).
    building: AtomicBool,
    self_weak: Mutex<Weak<SetRegistry>>,
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
            diags: Mutex::new(Diagnostics::default()),
            building: AtomicBool::new(true),
            self_weak: Mutex::new(Weak::new()),
        });
        *reg.self_weak.lock().expect("registry lock") = Arc::downgrade(&reg);
        // Pre-create everything the profile references so statuses are complete
        // and downloads start now.
        for rule in &cfg.rules {
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
        reg.building.store(false, Ordering::Relaxed);
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
                SetStatus {
                    name: key.display(),
                    kind: *kind,
                    entries: set.entry_count(),
                    version: set.version,
                    state: e.state.clone(),
                    skipped: e.skipped,
                    truncated: e.truncated,
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
        let conflicts = {
            let mut entries = self.entries.lock().expect("registry lock");
            if let Some(e) = entries.get(&(key.clone(), kind)) {
                return e.handle.clone();
            }
            // One file or URL, two formats: the first role keeps the set.
            let conflicts = matches!(key, Key::Url(_) | Key::File(_))
                && entries.contains_key(&(key.clone(), other));
            if !conflicts {
                entries.insert(
                    (key.clone(), kind),
                    Entry {
                        handle: SetHandle::new(CompiledSet::empty(&name, kind)),
                        resource: None,
                        skipped: 0,
                        truncated: 0,
                        state: "compiling".to_string(),
                    },
                );
            }
            conflicts
        };
        if conflicts {
            let said = format!(
                "set `{name}` ignored: the same resource is already used as a {}",
                other.keyword()
            );
            self.push_diag(Diagnostic::warning(codes::W_SET_KIND_CONFLICT, &said));
            tracing::warn!("{said}");
            return SetHandle::new(CompiledSet::empty(&name, kind));
        }
        let handle = self.handle_of(&key, kind);
        let _guard = push_key(key.clone());
        let outcome = match &key {
            Key::Internal(i) => {
                let parsed = self.parse_text(kind, internal_set_text(*i), &self.base_dir);
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

    fn parse_text(&self, kind: SetKind, text: &str, base_dir: &Path) -> ParsedSet {
        let ctx = ParseCtx {
            inline_rulesets: &self.inline_names,
            base_dir,
        };
        parse_set(kind, text, &ctx)
    }

    /// Compile `parsed`, record skipped / truncated counts, return the set.
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
        let set = CompiledSet::compile(name, kind, &parsed, self, version);
        let mut entries = self.entries.lock().expect("registry lock");
        if let Some(e) = entries.get_mut(&(key.clone(), kind)) {
            e.skipped = parsed.skipped_total;
            e.truncated = parsed.truncated;
            e.state = state.to_string();
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
                let parsed = self.parse_text(kind, &text, &base_dir);
                self.finish(key, kind, name, parsed, version, "ok")
            }
            ResourceState::Failed {
                last_error,
                cached: Some((data, version)),
                ..
            } => {
                let text = String::from_utf8_lossy(&data);
                let parsed = self.parse_text(kind, &text, &base_dir);
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

    /// Recompile and swap whenever the resource publishes a new version.
    /// `rx` must already be subscribed to `resource` (see `compile_external`)
    /// so a version published before this task starts is never missed.
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
                let Some(reg) = weak.upgrade() else { return };
                let (data, version) = match resource.current() {
                    ResourceState::Available { data, version, .. } => (data, version),
                    _ => continue,
                };
                let name = key.display();
                let handle = reg.handle_of(&key, kind);
                if handle.version() == version {
                    continue;
                }
                let text = String::from_utf8_lossy(&data);
                // A fresh compile on this thread starts with an empty stack;
                // push the key being recompiled first so new content that
                // references itself (directly, or through a nested set) is
                // caught as a cycle instead of capturing this same handle
                // inside the set it is about to replace (F1).
                STACK.with(|s| s.borrow_mut().clear());
                let _guard = push_key(key.clone());
                let parsed = reg.parse_text(kind, &text, &base_dir);
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
    use rurge_net::testing::TestServer;
    use std::time::Duration;

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

    /// `rule.com` is a line of a rule set and `domain-set.com` one of a domain
    /// set: each name is matched by one reading of the file only, and is a
    /// skipped line in the other.
    const BOTH_KINDS: &str = "DOMAIN,rule.com\ndomain-set.com\n";

    /// A file that is a rule set at the top of the profile and a domain set
    /// inside another set file is one set, the first: the nested reference
    /// is left out with a warning, and the file is not read a second time
    /// in the other format.
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
        assert_eq!(
            diags.iter().last().map(|d| d.message.as_str()),
            Some(
                format!("set `{a}` ignored: the same resource is already used as a RULE-SET")
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
        assert_eq!(
            statuses,
            [
                (a, SetKind::RuleSet),
                (
                    dir.path().join("b.list").display().to_string(),
                    SetKind::RuleSet
                )
            ]
        );
    }

    /// And the other way round: the first role is the one that stays.
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
        assert_eq!(
            diags.iter().last().map(|d| d.message.as_str()),
            Some(
                format!("set `{a}` ignored: the same resource is already used as a DOMAIN-SET")
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
        assert!(
            texts.contains(&(
                codes::W_SET_KIND_CONFLICT,
                "set `https://***@sets.example/gh/user/repo@main/a.list` ignored: \
the same resource is already used as a RULE-SET"
            )),
            "{texts:?}"
        );
        for (_, text) in texts {
            assert!(
                !text.contains("S3CR3T") && !text.contains("al1ce"),
                "{text}"
            );
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
}
