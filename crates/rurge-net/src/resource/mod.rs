//! External resource manager (M2 design §5.3): disk cache, conditional
//! refresh with backoff, local-file watching, one entry per source.

mod cache;
mod fetch;
mod local;

pub use cache::Meta;

use crate::http::HttpClient;
use bytes::Bytes;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Notify, watch};
use url::Url;

pub const DEFAULT_UPDATE_INTERVAL: i64 = 86_400;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResourceSource {
    Url(Url),
    File(PathBuf),
}

impl ResourceSource {
    pub fn key(&self) -> String {
        match self {
            ResourceSource::Url(u) => u.as_str().to_string(),
            ResourceSource::File(p) => format!("file:{}", p.display()),
        }
    }
}

impl fmt::Display for ResourceSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourceSource::Url(u) => f.write_str(u.as_str()),
            ResourceSource::File(p) => write!(f, "{}", p.display()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResourceSpec {
    pub source: ResourceSource,
    pub update_interval: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct ResourceOptions {
    pub offline: bool,
    pub fetch_timeout: Duration,
    pub min_backoff: Duration,
    pub max_backoff: Duration,
    pub debounce: Duration,
    pub max_size: u64,
}

impl Default for ResourceOptions {
    fn default() -> Self {
        ResourceOptions {
            offline: false,
            fetch_timeout: Duration::from_secs(30),
            min_backoff: Duration::from_secs(60),
            max_backoff: Duration::from_secs(3600),
            debounce: Duration::from_millis(500),
            max_size: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ResourceState {
    Missing,
    Available {
        data: Arc<Bytes>,
        version: u64,
        fetched_at: SystemTime,
        stale: bool,
    },
    Failed {
        last_error: String,
        since: SystemTime,
        cached: Option<(Arc<Bytes>, u64)>,
    },
}

impl ResourceState {
    pub fn data(&self) -> Option<(Arc<Bytes>, u64)> {
        match self {
            ResourceState::Available { data, version, .. } => Some((data.clone(), *version)),
            ResourceState::Failed { cached, .. } => cached.clone(),
            ResourceState::Missing => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            ResourceState::Missing => "missing",
            ResourceState::Available { .. } => "available",
            ResourceState::Failed { .. } => "failed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResourceStatus {
    pub source: ResourceSource,
    pub state: &'static str,
    pub version: u64,
    pub fetched_at: Option<SystemTime>,
    pub next_refresh: Option<SystemTime>,
    pub last_error: Option<String>,
}

struct Entry {
    source: ResourceSource,
    /// What log lines call the resource instead of its URL (`get_labelled`).
    label: Mutex<Option<String>>,
    state: Mutex<ResourceState>,
    meta: Mutex<Option<Meta>>,
    interval: Mutex<Option<i64>>,
    next_refresh: Mutex<Option<SystemTime>>,
    tx: watch::Sender<u64>,
    kick: Notify,
}

impl Entry {
    /// What log lines call this resource: its label, else its source.
    fn log_name(&self) -> String {
        match &*self.label.lock().expect("label") {
            Some(label) => label.clone(),
            None => self.source.to_string(),
        }
    }

    fn version(&self) -> u64 {
        self.state
            .lock()
            .expect("state")
            .data()
            .map(|(_, v)| v)
            .unwrap_or(0)
    }

    fn set_state(&self, s: ResourceState) {
        *self.state.lock().expect("state") = s;
    }

    fn publish(&self, version: u64) {
        self.tx.send_replace(version);
    }

    fn effective_interval(&self) -> i64 {
        self.interval
            .lock()
            .expect("interval")
            .unwrap_or(DEFAULT_UPDATE_INTERVAL)
    }
}

#[derive(Clone)]
pub struct ResourceHandle {
    entry: Arc<Entry>,
}

impl ResourceHandle {
    pub fn current(&self) -> ResourceState {
        self.entry.state.lock().expect("state").clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.entry.tx.subscribe()
    }

    pub fn version(&self) -> u64 {
        self.entry.version()
    }

    pub fn source(&self) -> &ResourceSource {
        &self.entry.source
    }
}

/// One `ResourceManager` per configuration generation: there is no API to
/// retire individual entries. On reload, build a new manager and drop the
/// old one — cached files are re-read from disk, and the dropped manager's
/// background tasks (see the periodic liveness checks in `url_task` /
/// `file_task`) notice within 60 s and exit.
pub struct ResourceManager {
    root: PathBuf,
    client: Arc<HttpClient>,
    opts: ResourceOptions,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    self_weak: Mutex<Weak<ResourceManager>>,
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn is_stale(fetched_at: SystemTime, interval: i64) -> bool {
    if interval < 0 {
        return false;
    }
    fetched_at
        .elapsed()
        .map(|e| e.as_secs() >= interval.unsigned_abs())
        .unwrap_or(false)
}

fn merge_interval(entry: &Entry, new: Option<i64>) {
    let mut cur = entry.interval.lock().expect("interval");
    match (*cur, new) {
        (_, None) => {}
        (None, Some(n)) => *cur = Some(n),
        (Some(c), Some(n)) => {
            *cur = Some(match (c < 0, n < 0) {
                (true, false) => n,
                (false, true) => c,
                _ => c.min(n),
            });
        }
    }
}

/// ±10 % from a time-derived seed, so refreshes of many resources spread out.
fn jitter(d: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|x| x.subsec_nanos())
        .unwrap_or(0);
    let pct = i64::from(nanos % 21) - 10;
    let millis = i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
    let adjusted = millis.saturating_add(millis / 100 * pct).max(0);
    Duration::from_millis(u64::try_from(adjusted).unwrap_or(0))
}

/// What `get` would start a resource with, read without a manager and
/// without the network: the disk cache of a URL under `root`, the content of
/// a file. For offline checks.
pub fn cached(root: &Path, source: &ResourceSource) -> Option<Bytes> {
    match source {
        ResourceSource::Url(url) => cache::CacheDir::for_url(root, url.as_str())
            .load()
            .map(|(data, _)| data),
        ResourceSource::File(path) => std::fs::read(path).ok().map(Bytes::from),
    }
}

impl ResourceManager {
    pub fn new(root: PathBuf, client: Arc<HttpClient>) -> Arc<ResourceManager> {
        Self::with_options(root, client, ResourceOptions::default())
    }

    pub fn with_options(
        root: PathBuf,
        client: Arc<HttpClient>,
        opts: ResourceOptions,
    ) -> Arc<ResourceManager> {
        let mgr = Arc::new(ResourceManager {
            root,
            client,
            opts,
            entries: Mutex::new(HashMap::new()),
            self_weak: Mutex::new(Weak::new()),
        });
        *mgr.self_weak.lock().expect("weak") = Arc::downgrade(&mgr);
        mgr
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn get(&self, spec: &ResourceSpec) -> ResourceHandle {
        self.register(spec, None)
    }

    /// `get` for a resource whose URL must not reach the logs — a
    /// subscription URL usually carries a token (phase 2 M3-D7): log lines
    /// call it `label` instead, also when another caller shares it. The first
    /// label a resource gets is the one it keeps.
    pub fn get_labelled(&self, spec: &ResourceSpec, label: &str) -> ResourceHandle {
        self.register(spec, Some(label))
    }

    fn register(&self, spec: &ResourceSpec, label: Option<&str>) -> ResourceHandle {
        let key = spec.source.key();
        // Look up and (if absent) insert inside one critical section: releasing the lock
        // between a "not found" lookup and the insert let two concurrent first calls for the
        // same source each create their own `Entry` and background task. `start` itself is
        // called after the lock is released, and only for an entry this call actually created.
        let (entry, is_new) = {
            let mut entries = self.entries.lock().expect("entries");
            if let Some(existing) = entries.get(&key) {
                merge_interval(existing, spec.update_interval);
                if let Some(label) = label {
                    existing
                        .label
                        .lock()
                        .expect("label")
                        .get_or_insert_with(|| label.to_string());
                }
                (existing.clone(), false)
            } else {
                let (tx, _rx) = watch::channel(0u64);
                let entry = Arc::new(Entry {
                    source: spec.source.clone(),
                    label: Mutex::new(label.map(str::to_string)),
                    state: Mutex::new(ResourceState::Missing),
                    meta: Mutex::new(None),
                    interval: Mutex::new(spec.update_interval),
                    next_refresh: Mutex::new(None),
                    tx,
                    kick: Notify::new(),
                });
                entries.insert(key, entry.clone());
                (entry, true)
            }
        };
        if is_new {
            self.start(entry.clone());
        }
        ResourceHandle { entry }
    }

    /// Waits until no entry is `Missing` (or the timeout passes) and reports statuses.
    pub async fn wait_initial(&self, timeout: Duration) -> Vec<ResourceStatus> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let pending = self
                .entries
                .lock()
                .expect("entries")
                .values()
                .any(|e| matches!(*e.state.lock().expect("state"), ResourceState::Missing));
            if !pending || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.statuses()
    }

    pub fn force_update(&self, source: &ResourceSource) -> bool {
        match self.entries.lock().expect("entries").get(&source.key()) {
            Some(e) => {
                e.kick.notify_one();
                true
            }
            None => false,
        }
    }

    pub fn statuses(&self) -> Vec<ResourceStatus> {
        let entries = self.entries.lock().expect("entries");
        let mut out: Vec<ResourceStatus> = entries
            .values()
            .map(|e| {
                let state = e.state.lock().expect("state");
                let (fetched_at, last_error) = match &*state {
                    ResourceState::Available { fetched_at, .. } => (Some(*fetched_at), None),
                    ResourceState::Failed { last_error, .. } => (None, Some(last_error.clone())),
                    ResourceState::Missing => (None, None),
                };
                ResourceStatus {
                    source: e.source.clone(),
                    state: state.kind(),
                    version: state.data().map(|(_, v)| v).unwrap_or(0),
                    fetched_at,
                    next_refresh: *e.next_refresh.lock().expect("next"),
                    last_error,
                }
            })
            .collect();
        out.sort_by_key(|s| s.source.key());
        out
    }

    fn weak(&self) -> Weak<ResourceManager> {
        self.self_weak.lock().expect("weak").clone()
    }

    fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) {
        match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                h.spawn(fut);
            }
            Err(_) => tracing::debug!("no tokio runtime: resource refresh disabled"),
        }
    }

    fn start(&self, entry: Arc<Entry>) {
        match entry.source.clone() {
            ResourceSource::Url(url) => {
                let cache = cache::CacheDir::for_url(&self.root, url.as_str());
                if let Some((data, meta)) = cache.load() {
                    let fetched_at = UNIX_EPOCH + Duration::from_secs(meta.fetched_at);
                    let stale = is_stale(fetched_at, entry.effective_interval());
                    *entry.meta.lock().expect("meta") = Some(meta);
                    entry.set_state(ResourceState::Available {
                        data: Arc::new(data),
                        version: 1,
                        fetched_at,
                        stale,
                    });
                    entry.publish(1);
                }
                if self.opts.offline {
                    if matches!(*entry.state.lock().expect("state"), ResourceState::Missing) {
                        entry.set_state(ResourceState::Failed {
                            last_error: "offline mode: no cached copy".to_string(),
                            since: SystemTime::now(),
                            cached: None,
                        });
                    }
                    return;
                }
                self.spawn(url_task(self.weak(), entry));
            }
            ResourceSource::File(path) => {
                read_file(&entry, &path, 0);
                // Arm the watcher synchronously (before `get` returns) rather than inside the
                // spawned task: on a current-thread runtime a freshly spawned task doesn't run
                // until the caller's next `.await`, so a caller that writes to the file right
                // after `get` would otherwise race the watcher's own setup.
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();
                let watcher = match local::watch_file(&path, tx) {
                    Ok(w) => Some(w),
                    Err(e) => {
                        tracing::warn!(resource = %entry.log_name(), error = %e, "cannot watch file; changes need a reload");
                        None
                    }
                };
                self.spawn(file_task(self.weak(), entry, rx, watcher));
            }
        }
    }
}

fn next_due(entry: &Entry, backoff: Option<Duration>) -> Option<Duration> {
    if let Some(b) = backoff {
        return Some(b);
    }
    let interval = entry.effective_interval();
    let state = entry.state.lock().expect("state");
    match &*state {
        ResourceState::Missing | ResourceState::Failed { .. } => Some(Duration::ZERO),
        ResourceState::Available { stale: true, .. } => Some(Duration::ZERO),
        ResourceState::Available { fetched_at, .. } => {
            if interval < 0 {
                return None;
            }
            let interval = Duration::from_secs(interval.unsigned_abs());
            let elapsed = fetched_at.elapsed().unwrap_or(Duration::ZERO);
            Some(jitter(interval.saturating_sub(elapsed)))
        }
    }
}

async fn url_task(weak: Weak<ResourceManager>, entry: Arc<Entry>) {
    let ResourceSource::Url(url) = entry.source.clone() else {
        return;
    };
    let mut backoff: Option<Duration> = None;
    loop {
        let Some(mgr) = weak.upgrade() else { return };
        let opts = mgr.opts.clone();
        let client = mgr.client.clone();
        let cache = cache::CacheDir::for_url(&mgr.root, url.as_str());
        drop(mgr);
        let due = next_due(&entry, backoff);
        *entry.next_refresh.lock().expect("next") = due.map(|d| SystemTime::now() + d);
        // `due` is `None` only when auto-refresh is disabled (negative interval); bound that
        // wait too, so an orphaned task (manager dropped, nobody left to `force_update`) still
        // wakes periodically to notice `weak.upgrade()` failing instead of parking forever.
        let wait = due.unwrap_or(Duration::from_secs(60));
        let kicked = tokio::select! {
            _ = tokio::time::sleep(wait) => false,
            _ = entry.kick.notified() => true,
        };
        if weak.upgrade().is_none() {
            return;
        }
        if due.is_none() && !kicked {
            // Periodic liveness tick only; no refresh is actually due.
            continue;
        }
        let meta = entry.meta.lock().expect("meta").clone();
        match fetch::fetch(
            &client,
            &url,
            meta.as_ref(),
            opts.fetch_timeout,
            opts.max_size,
        )
        .await
        {
            Ok(fetch::Fetched::New {
                data,
                etag,
                last_modified,
            }) => {
                let version = entry.version() + 1;
                let now = SystemTime::now();
                let new_meta = Meta {
                    url: url.to_string(),
                    etag,
                    last_modified,
                    fetched_at: unix(now),
                };
                if let Err(e) = cache.store(&data, &new_meta) {
                    tracing::warn!(resource = %entry.log_name(), error = %e, "cannot write resource cache");
                }
                *entry.meta.lock().expect("meta") = Some(new_meta);
                entry.set_state(ResourceState::Available {
                    data: Arc::new(data),
                    version,
                    fetched_at: now,
                    stale: false,
                });
                entry.publish(version);
                backoff = None;
                tracing::info!(resource = %entry.log_name(), version, "resource updated");
            }
            Ok(fetch::Fetched::NotModified) => {
                let now = SystemTime::now();
                if let Some(m) = entry.meta.lock().expect("meta").as_mut() {
                    m.fetched_at = unix(now);
                    let _ = cache.store_meta(m);
                }
                let mut st = entry.state.lock().expect("state");
                if let ResourceState::Available {
                    fetched_at, stale, ..
                } = &mut *st
                {
                    *fetched_at = now;
                    *stale = false;
                } else if let Some((data, version)) = st.data() {
                    *st = ResourceState::Available {
                        data,
                        version,
                        fetched_at: now,
                        stale: false,
                    };
                }
                drop(st);
                backoff = None;
            }
            Err(e) => {
                let cached = entry.state.lock().expect("state").data();
                tracing::warn!(resource = %entry.log_name(), error = %e, "resource fetch failed");
                entry.set_state(ResourceState::Failed {
                    last_error: e,
                    since: SystemTime::now(),
                    cached,
                });
                backoff = Some(match backoff {
                    None => opts.min_backoff,
                    Some(b) => (b * 2).min(opts.max_backoff),
                });
            }
        }
    }
}

fn read_file(entry: &Entry, path: &Path, prev_version: u64) {
    match std::fs::read(path) {
        Ok(bytes) => {
            let (unchanged, failed) = {
                let st = entry.state.lock().expect("state");
                (
                    st.data()
                        .map(|(d, _)| d.as_ref() == &bytes[..])
                        .unwrap_or(false),
                    matches!(*st, ResourceState::Failed { .. }),
                )
            };
            let now = SystemTime::now();
            if unchanged && !failed {
                return;
            }
            let version = if unchanged {
                prev_version.max(1)
            } else {
                prev_version + 1
            };
            entry.set_state(ResourceState::Available {
                data: Arc::new(Bytes::from(bytes)),
                version,
                fetched_at: now,
                stale: false,
            });
            if !unchanged {
                entry.publish(version);
            }
        }
        Err(e) => {
            let cached = entry.state.lock().expect("state").data();
            entry.set_state(ResourceState::Failed {
                last_error: format!("cannot read {}: {e}", path.display()),
                since: SystemTime::now(),
                cached,
            });
        }
    }
}

async fn file_task(
    weak: Weak<ResourceManager>,
    entry: Arc<Entry>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    _watcher: Option<local::AnyWatcher>,
) {
    let ResourceSource::File(path) = entry.source.clone() else {
        return;
    };
    let debounce = weak
        .upgrade()
        .map(|m| m.opts.debounce)
        .unwrap_or(Duration::from_millis(500));
    loop {
        tokio::select! {
            r = rx.recv() => {
                if r.is_none() {
                    // No watcher: only explicit force_update wakes us. Still bound the wait so
                    // an orphaned task (manager dropped, nobody left to `force_update`) notices
                    // `weak.upgrade()` failing instead of parking forever.
                    tokio::select! {
                        _ = entry.kick.notified() => {}
                        _ = tokio::time::sleep(Duration::from_secs(60)) => {
                            if weak.upgrade().is_none() {
                                return;
                            }
                            continue;
                        }
                    }
                }
            }
            _ = entry.kick.notified() => {}
            // Same liveness tick for the common (working-watcher) path: a resource that never
            // changes and is never kicked must not keep this task alive forever.
            _ = tokio::time::sleep(Duration::from_secs(60)) => {
                if weak.upgrade().is_none() {
                    return;
                }
                continue;
            }
        }
        tokio::time::sleep(debounce).await;
        while rx.try_recv().is_ok() {}
        if weak.upgrade().is_none() {
            return;
        }
        let prev = entry.version();
        read_file(&entry, &path, prev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::{DirectConnector, SystemResolve};
    use crate::http::{HttpClient, HttpClientConfig};
    use crate::testing::TestServer;

    fn fast() -> ResourceOptions {
        ResourceOptions {
            fetch_timeout: Duration::from_secs(5),
            min_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_millis(200),
            debounce: Duration::from_millis(50),
            ..ResourceOptions::default()
        }
    }

    fn manager(root: &Path, opts: ResourceOptions) -> Arc<ResourceManager> {
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let client = Arc::new(HttpClient::new(connector, HttpClientConfig::default()).unwrap());
        ResourceManager::with_options(root.to_path_buf(), client, opts)
    }

    fn url_spec(url: Url, interval: Option<i64>) -> ResourceSpec {
        ResourceSpec {
            source: ResourceSource::Url(url),
            update_interval: interval,
        }
    }

    async fn wait_for(
        h: &ResourceHandle,
        what: &str,
        pred: impl Fn(&ResourceState) -> bool,
    ) -> ResourceState {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let s = h.current();
            if pred(&s) {
                return s;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}: {s:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn available_version(s: &ResourceState) -> Option<u64> {
        match s {
            ResourceState::Available { version, .. } => Some(*version),
            _ => None,
        }
    }

    #[tokio::test]
    async fn fetches_caches_and_publishes_version_one() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/a", "alpha");
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(server.url("/a"), None));
        let mut rx = h.subscribe();
        let s = wait_for(&h, "available", |s| available_version(s) == Some(1)).await;
        assert_eq!(&s.data().unwrap().0[..], b"alpha");
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow_and_update(), 1);
        let cache = cache::CacheDir::for_url(root.path(), server.url("/a").as_str());
        assert!(cache.path().join("data").exists() && cache.path().join("meta.json").exists());
        assert_eq!(server.hits("/a"), 1);
        let st = mgr.statuses();
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].state, "available");
        assert!(st[0].next_refresh.is_some());
    }

    #[tokio::test]
    async fn second_manager_starts_from_cache_without_fetching() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/b", "bravo");
        let first = manager(root.path(), fast());
        let h = first.get(&url_spec(server.url("/b"), None));
        wait_for(&h, "available", |s| available_version(s).is_some()).await;
        drop(h);
        drop(first);
        let second = manager(root.path(), fast());
        let h = second.get(&url_spec(server.url("/b"), None));
        let s = h.current();
        assert!(
            matches!(
                s,
                ResourceState::Available {
                    version: 1,
                    stale: false,
                    ..
                }
            ),
            "{s:?}"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(server.hits("/b"), 1);
    }

    #[tokio::test]
    async fn conditional_refresh_gets_304_and_keeps_the_version() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/c", "charlie");
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(server.url("/c"), None));
        wait_for(&h, "available", |s| available_version(s).is_some()).await;
        assert!(mgr.force_update(&ResourceSource::Url(server.url("/c"))));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while server.hits("/c") < 2 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let reqs = server.requests();
        assert_eq!(reqs.len(), 2);
        assert!(reqs[1].header("if-none-match").is_some());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(h.version(), 1);
        assert!(!mgr.force_update(&ResourceSource::Url(server.url("/nope"))));
    }

    #[tokio::test]
    async fn changed_content_bumps_the_version() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/d", "one");
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(server.url("/d"), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        server.set("/d", "two");
        mgr.force_update(&ResourceSource::Url(server.url("/d")));
        let s = wait_for(&h, "v2", |s| available_version(s) == Some(2)).await;
        assert_eq!(&s.data().unwrap().0[..], b"two");
    }

    #[tokio::test]
    async fn failure_without_cache_then_recovery_with_backoff() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(server.url("/e"), None));
        let s = wait_for(&h, "failed", |s| {
            matches!(s, ResourceState::Failed { cached: None, .. })
        })
        .await;
        assert!(
            matches!(s, ResourceState::Failed { ref last_error, .. } if last_error.contains("404"))
        );
        let st = mgr.wait_initial(Duration::from_secs(1)).await;
        assert_eq!(st[0].state, "failed");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            server.hits("/e") >= 3,
            "retries with backoff, got {}",
            server.hits("/e")
        );
        server.set("/e", "echo");
        let s = wait_for(&h, "recovered", |s| available_version(s) == Some(1)).await;
        assert_eq!(&s.data().unwrap().0[..], b"echo");
    }

    #[tokio::test]
    async fn failure_with_cache_keeps_serving_the_old_data() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/f", "foxtrot");
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(server.url("/f"), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        server.set_status("/f", 500);
        mgr.force_update(&ResourceSource::Url(server.url("/f")));
        let s = wait_for(&h, "failed with cache", |s| {
            matches!(
                s,
                ResourceState::Failed {
                    cached: Some(_),
                    ..
                }
            )
        })
        .await;
        assert_eq!(&s.data().unwrap().0[..], b"foxtrot");
        assert_eq!(s.data().unwrap().1, 1);
    }

    #[tokio::test]
    async fn offline_mode_uses_the_cache_only() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/g", "golf");
        let online = manager(root.path(), fast());
        let h = online.get(&url_spec(server.url("/g"), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        drop(h);
        drop(online);
        let offline = manager(
            root.path(),
            ResourceOptions {
                offline: true,
                ..fast()
            },
        );
        let h = offline.get(&url_spec(server.url("/g"), None));
        assert!(matches!(
            h.current(),
            ResourceState::Available { version: 1, .. }
        ));
        let none = offline.get(&url_spec(server.url("/missing"), None));
        assert!(
            matches!(none.current(), ResourceState::Failed { ref last_error, .. } if last_error.contains("offline"))
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(server.hits("/g"), 1);
        assert_eq!(server.hits("/missing"), 0);
    }

    #[tokio::test]
    async fn size_limit_is_enforced() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/h", vec![b'h'; 100]);
        let mgr = manager(
            root.path(),
            ResourceOptions {
                max_size: 8,
                ..fast()
            },
        );
        let h = mgr.get(&url_spec(server.url("/h"), None));
        let s = wait_for(&h, "too large", |s| {
            matches!(s, ResourceState::Failed { .. })
        })
        .await;
        assert!(
            matches!(s, ResourceState::Failed { ref last_error, .. } if last_error.contains("exceeds"))
        );
    }

    #[tokio::test]
    async fn same_source_shares_one_entry_and_the_smallest_interval() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/i", "india");
        let mgr = manager(root.path(), fast());
        let a = mgr.get(&url_spec(server.url("/i"), Some(3600)));
        let b = mgr.get(&url_spec(server.url("/i"), Some(600)));
        assert!(Arc::ptr_eq(&a.entry, &b.entry));
        assert_eq!(a.entry.effective_interval(), 600);
        let c = mgr.get(&url_spec(server.url("/i"), Some(-1)));
        assert_eq!(c.entry.effective_interval(), 600);
        assert_eq!(mgr.statuses().len(), 1);
    }

    /// Regression test for a `get()` check-then-act race: two concurrent first calls for the
    /// same brand-new source used to each observe the entry missing (lock released between the
    /// lookup and the insert) and create their own `Entry` + background task. `get()` is
    /// synchronous, so this uses real OS threads and a `std::sync::Barrier` to force genuine
    /// concurrent entry into it — spawning two `tokio::spawn` tasks instead was tried first and
    /// found unreliable (tokio's scheduler sometimes runs both on the same worker thread,
    /// serializing them and hiding the race).
    #[tokio::test]
    async fn concurrent_get_for_a_new_source_creates_only_one_entry() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/j", "juliett");
        let mgr = manager(root.path(), fast());
        let url = server.url("/j");
        let barrier = Arc::new(std::sync::Barrier::new(2));

        let (m1, b1, u1) = (mgr.clone(), barrier.clone(), url.clone());
        let t1 = std::thread::spawn(move || {
            b1.wait();
            m1.get(&url_spec(u1, None))
        });
        let (m2, b2, u2) = (mgr.clone(), barrier.clone(), url.clone());
        let t2 = std::thread::spawn(move || {
            b2.wait();
            m2.get(&url_spec(u2, None))
        });
        let h1 = t1.join().unwrap();
        let h2 = t2.join().unwrap();
        assert!(Arc::ptr_eq(&h1.entry, &h2.entry));
        assert_eq!(mgr.statuses().len(), 1);
    }

    #[tokio::test]
    async fn local_file_is_read_watched_and_survives_deletion() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("local.list");
        std::fs::write(&file, "one").unwrap();
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&ResourceSpec {
            source: ResourceSource::File(file.clone()),
            update_interval: None,
        });
        let s = h.current();
        assert_eq!(available_version(&s), Some(1));
        assert_eq!(&s.data().unwrap().0[..], b"one");
        std::fs::write(&file, "two").unwrap();
        let s = wait_for(&h, "v2", |s| available_version(s) == Some(2)).await;
        assert_eq!(&s.data().unwrap().0[..], b"two");
        std::fs::remove_file(&file).unwrap();
        let s = wait_for(&h, "failed with cache", |s| {
            matches!(
                s,
                ResourceState::Failed {
                    cached: Some(_),
                    ..
                }
            )
        })
        .await;
        assert_eq!(&s.data().unwrap().0[..], b"two");
        std::fs::write(&file, "three").unwrap();
        let s = wait_for(&h, "v3", |s| available_version(s) == Some(3)).await;
        assert_eq!(&s.data().unwrap().0[..], b"three");
    }

    #[tokio::test]
    async fn missing_local_file_is_failed_and_wait_initial_returns() {
        let root = tempfile::tempdir().unwrap();
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&ResourceSpec {
            source: ResourceSource::File(root.path().join("nope.list")),
            update_interval: None,
        });
        assert!(matches!(
            h.current(),
            ResourceState::Failed { cached: None, .. }
        ));
        let st = mgr.wait_initial(Duration::from_secs(1)).await;
        assert_eq!(st[0].state, "failed");
        assert!(st[0].last_error.as_deref().unwrap().contains("cannot read"));
    }

    #[test]
    fn cached_reads_what_a_manager_would_start_with() {
        let root = tempfile::tempdir().unwrap();
        let url = Url::parse("https://sub.test/nodes?token=t0k3n").unwrap();
        let remote = ResourceSource::Url(url.clone());
        assert!(cached(root.path(), &remote).is_none());
        let meta = Meta {
            url: url.to_string(),
            ..Meta::default()
        };
        cache::CacheDir::for_url(root.path(), url.as_str())
            .store(b"cached", &meta)
            .unwrap();
        assert_eq!(&cached(root.path(), &remote).unwrap()[..], b"cached");
        let file = root.path().join("nodes.txt");
        let local = ResourceSource::File(file.clone());
        assert!(cached(root.path(), &local).is_none());
        std::fs::write(&file, "local").unwrap();
        assert_eq!(&cached(root.path(), &local).unwrap()[..], b"local");
    }

    /// A subscription URL usually carries a token: once somebody labels the
    /// resource, no log line names it by its URL any more.
    #[tokio::test]
    async fn a_labelled_resource_is_logged_by_its_label() {
        let root = tempfile::tempdir().unwrap();
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(root.path(), offline);
        let url = Url::parse("https://sub.test/nodes?token=t0k3n").unwrap();
        let plain = mgr.get(&url_spec(url.clone(), None));
        assert_eq!(plain.entry.log_name(), url.as_str());
        let labelled = mgr.get_labelled(&url_spec(url.clone(), None), "policy-path of `G`");
        assert!(Arc::ptr_eq(&plain.entry, &labelled.entry));
        assert_eq!(plain.entry.log_name(), "policy-path of `G`");
        // the first label stays
        mgr.get_labelled(&url_spec(url, None), "policy-path of `H`");
        assert_eq!(plain.entry.log_name(), "policy-path of `G`");
    }
}
