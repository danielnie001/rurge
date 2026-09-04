//! Set registry (M2 design §6.2): one `SetHandle` per (resource, kind).
//! Internal and inline sets compile once; external sets come from the
//! `ResourceManager` and are recompiled and swapped whenever the resource
//! changes. Nested references are resolved through the same registry with
//! cycle detection and a nesting limit of 8.

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
use std::sync::{Arc, Mutex, Weak};
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

    fn display(&self) -> String {
        match self {
            Key::Internal(InternalSet::System) => "SYSTEM".to_string(),
            Key::Internal(InternalSet::Lan) => "LAN".to_string(),
            Key::Inline(n) => n.clone(),
            Key::Url(u) => u.clone(),
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
    self_weak: Mutex<Weak<SetRegistry>>,
}

thread_local! {
    /// Compile stack of the current thread: nested `lookup` calls during one
    /// compile push here, so cycles and depth are detected per compile.
    static STACK: RefCell<Vec<Key>> = const { RefCell::new(Vec::new()) };
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
        self.diags.lock().expect("registry lock").push(d);
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
        {
            let mut entries = self.entries.lock().expect("registry lock");
            if let Some(e) = entries.get(&(key.clone(), kind)) {
                return e.handle.clone();
            }
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
        let handle = self.handle_of(&key, kind);
        STACK.with(|s| s.borrow_mut().push(key.clone()));
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
        STACK.with(|s| s.borrow_mut().pop());
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
                    parsed.skipped.len()
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
            e.skipped = parsed.skipped.len();
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
        self.spawn_reloader(key.clone(), kind, base_dir, resource);
        compiled
    }

    fn set_state(&self, key: &Key, kind: SetKind, state: String) {
        let mut entries = self.entries.lock().expect("registry lock");
        if let Some(e) = entries.get_mut(&(key.clone(), kind)) {
            e.state = state;
        }
    }

    /// Recompile and swap whenever the resource publishes a new version.
    fn spawn_reloader(&self, key: Key, kind: SetKind, base_dir: PathBuf, resource: ResourceHandle) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            tracing::debug!(set = %key.display(), "no tokio runtime: set auto-reload disabled");
            return;
        };
        let weak = self.self_weak.lock().expect("registry lock").clone();
        rt.spawn(async move {
            let mut rx = resource.subscribe();
            loop {
                if rx.changed().await.is_err() {
                    return;
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
                // A fresh compile on this thread starts with an empty stack.
                STACK.with(|s| s.borrow_mut().clear());
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
}
