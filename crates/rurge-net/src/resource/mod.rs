//! External resource manager (M2 design §5.3): disk cache, conditional
//! refresh with backoff, local-file watching, one entry per source.

mod cache;
mod fetch;
mod local;

#[cfg(any(test, feature = "testing"))]
pub(crate) use cache::CacheDir;
pub use cache::Meta;

use crate::http::HttpClient;
use bytes::Bytes;
use rurge_config::ResourceRef;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Notify, watch};
use url::Url;

pub const DEFAULT_UPDATE_INTERVAL: i64 = 86_400;

/// What a resource is for. The first registration of a source fixes it, as it
/// fixes the label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    /// A rule set a `RULE-SET` rule names.
    RuleSet,
    /// A domain set a `DOMAIN-SET` rule or a `[Host]` key names.
    DomainSet,
    /// The `policy-path` of a policy group: a subscription.
    PolicyGroup,
    /// A GeoIP or ASN database.
    GeoIp,
    /// A profile that is kept up to date from a URL.
    Profile,
    /// A file a profile includes from a URL.
    Include,
    /// What the system keeps and rurge only watches: the hosts file.
    System,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum ResourceSource {
    Url(Url),
    File(PathBuf),
}

/// A URL as a `Debug` shows it: where it goes, without its path and query,
/// which often carry a token.
pub(crate) fn origin(url: &Url) -> String {
    format!(
        "{}://{}/…",
        url.scheme(),
        &url[url::Position::BeforeHost..url::Position::AfterPort]
    )
}

impl fmt::Debug for ResourceSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourceSource::Url(url) => f.debug_tuple("Url").field(&origin(url)).finish(),
            ResourceSource::File(path) => f.debug_tuple("File").field(path).finish(),
        }
    }
}

impl ResourceSource {
    pub fn key(&self) -> String {
        match self {
            ResourceSource::Url(u) => u.as_str().to_string(),
            ResourceSource::File(p) => format!("file:{}", p.display()),
        }
    }

    /// What a log line calls the source when nobody labelled it. The URL of a
    /// rule set can carry a user name and a password, which are `***` here;
    /// the path and the query, which name the set, stay.
    fn shown(&self) -> String {
        match self {
            ResourceSource::Url(u) => ResourceRef::shown_url(u.as_str()),
            ResourceSource::File(p) => p.display().to_string(),
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
    pub kind: ResourceKind,
}

#[derive(Clone, Debug)]
pub struct ResourceOptions {
    pub offline: bool,
    pub fetch_timeout: Duration,
    pub min_backoff: Duration,
    pub max_backoff: Duration,
    pub debounce: Duration,
    pub max_size: u64,
    /// The spread of the refresh times goes only later, never earlier than the
    /// interval: for a resource whose interval is the shortest time before an
    /// update may be triggered.
    pub never_early: bool,
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
            never_early: false,
        }
    }
}

#[derive(Clone)]
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

/// The body by its length: a subscription's holds its nodes' credentials.
impl fmt::Debug for ResourceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sized = |data: &Bytes| format!("{} bytes", data.len());
        match self {
            ResourceState::Missing => f.write_str("Missing"),
            ResourceState::Available {
                data,
                version,
                fetched_at,
                stale,
            } => f
                .debug_struct("Available")
                .field("data", &sized(data))
                .field("version", version)
                .field("fetched_at", fetched_at)
                .field("stale", stale)
                .finish(),
            ResourceState::Failed {
                last_error,
                since,
                cached,
            } => f
                .debug_struct("Failed")
                .field("last_error", last_error)
                .field("since", since)
                .field(
                    "cached",
                    &cached
                        .as_ref()
                        .map(|(data, version)| (sized(data), version)),
                )
                .finish(),
        }
    }
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
    /// The last successful download or "not modified" answer, also while the
    /// resource is `failed` and keeps the copy it had. `None` for a resource
    /// that never had one, and for a file that cannot be read any more: the
    /// manager does not keep the time it last read it.
    pub fetched_at: Option<SystemTime>,
    pub next_refresh: Option<SystemTime>,
    pub last_error: Option<String>,
    pub kind: ResourceKind,
    pub label: Option<String>,
    /// `resource_key` of the source.
    pub key: String,
    /// The source is a file on this machine.
    pub local: bool,
    /// The manager has a copy to give: the resource is `available`, or
    /// `failed` and keeps the copy it had.
    pub ready: bool,
    /// A download is under way.
    pub updating: bool,
}

impl ResourceStatus {
    /// The name the resource goes by outside the manager, as the log lines
    /// call it: its label, else its source with the user name and the
    /// password of a URL left out.
    pub fn shown(&self) -> String {
        shown_as(self.label.as_deref(), &self.source)
    }
}

/// How a resource is called wherever it is shown, in a log line or in its
/// status: its label, else its source (a URL without its user name and its
/// password).
fn shown_as(label: Option<&str>, source: &ResourceSource) -> String {
    label.map_or_else(|| source.shown(), str::to_string)
}

/// `source.key()` hashed with SHA-256, in 64 lower-case hex digits. For a URL
/// it names the directory of the resource's cache.
pub fn resource_key(source: &ResourceSource) -> String {
    cache::name_of(&source.key())
}

struct Entry {
    source: ResourceSource,
    kind: ResourceKind,
    /// What log lines call the resource instead of its URL (`get_labelled`).
    label: Mutex<Option<String>>,
    state: Mutex<ResourceState>,
    meta: Mutex<Option<Meta>>,
    interval: Mutex<Option<i64>>,
    next_refresh: Mutex<Option<SystemTime>>,
    updating: AtomicBool,
    tx: watch::Sender<u64>,
    kick: Notify,
}

/// Marks an entry as being downloaded while it lives, so that an attempt that
/// is dropped half-way leaves nothing that claims to be downloading for good.
struct Updating<'a>(&'a AtomicBool);

impl<'a> Updating<'a> {
    fn begin(flag: &'a AtomicBool) -> Updating<'a> {
        flag.store(true, Ordering::Release);
        Updating(flag)
    }
}

impl Drop for Updating<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Entry {
    /// What log lines call this resource: its label, else its source (a URL
    /// without its user name and its password).
    fn log_name(&self) -> String {
        shown_as(self.label.lock().expect("label").as_deref(), &self.source)
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
/// background tasks exit: a URL task as soon as it waits for its next
/// refresh (`gone`), a file task within 60 s (the periodic liveness checks in
/// `file_task`).
pub struct ResourceManager {
    root: PathBuf,
    client: Arc<HttpClient>,
    opts: ResourceOptions,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    self_weak: Mutex<Weak<ResourceManager>>,
    /// Closed when the manager is dropped: its URL tasks wait on it beside
    /// their timers, so none sits out a refresh interval with its entry
    /// after the manager is gone.
    gone: watch::Sender<()>,
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

/// The seed of `jitter`: the clock's nanoseconds, which differ from one
/// resource to the next.
fn clock_seed() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|x| x.subsec_nanos())
        .unwrap_or(0)
}

/// `d` spread by ±10 %, so refreshes of many resources spread out, or with
/// `never_early` by 0 ~ +10 %. Any `seed` will do: it picks one of 21 shares.
fn jitter(d: Duration, seed: u32, never_early: bool) -> Duration {
    let share = seed % 21;
    let one_percent = d / 100;
    if never_early {
        d.saturating_add(one_percent.saturating_mul(share / 2))
    } else if share >= 10 {
        d.saturating_add(one_percent.saturating_mul(share - 10))
    } else {
        d.saturating_sub(one_percent.saturating_mul(10 - share))
    }
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

/// The cache of a URL as it lies on disk.
pub struct CachedCopy {
    pub data: Bytes,
    /// The file the body is in: `<root>/resources/<key>/data`.
    pub path: PathBuf,
    /// When the copy was last checked successfully, to the nanosecond.
    pub fetched_at: SystemTime,
}

/// The body by its length, as `ResourceState` shows it: it can hold
/// credentials.
impl fmt::Debug for CachedCopy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachedCopy")
            .field("data", &format!("{} bytes", self.data.len()))
            .field("path", &self.path)
            .field("fetched_at", &self.fetched_at)
            .finish()
    }
}

/// The cache of `url` under `root`, read without a manager and without the
/// network. `None` when one of its two files is missing or `meta.json` cannot
/// be read.
pub fn cached_copy(root: &Path, url: &Url) -> Option<CachedCopy> {
    let dir = cache::CacheDir::for_url(root, url.as_str());
    let (data, meta) = dir.load()?;
    Some(CachedCopy {
        data,
        path: dir.path().join("data"),
        fetched_at: meta.fetched_time(),
    })
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
            gone: watch::Sender::new(()),
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
        // An interval of 0 is read as none written: a refresh due again as soon as it is done
        // would make the task fetch as fast as the server answers. Here, where every caller's
        // interval enters, so that `effective_interval` never sees 0.
        let interval = spec.update_interval.filter(|&seconds| seconds != 0);
        // Look up and (if absent) insert inside one critical section: releasing the lock
        // between a "not found" lookup and the insert let two concurrent first calls for the
        // same source each create their own `Entry` and background task. `start` itself is
        // called after the lock is released, and only for an entry this call actually created.
        let (entry, is_new) = {
            let mut entries = self.entries.lock().expect("entries");
            if let Some(existing) = entries.get(&key) {
                merge_interval(existing, interval);
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
                    kind: spec.kind,
                    label: Mutex::new(label.map(str::to_string)),
                    state: Mutex::new(ResourceState::Missing),
                    meta: Mutex::new(None),
                    interval: Mutex::new(interval),
                    next_refresh: Mutex::new(None),
                    updating: AtomicBool::new(false),
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
                // For a resource that has failed since: the last check its
                // cache knows of. Read before the state is locked.
                let last_check = e
                    .meta
                    .lock()
                    .expect("meta")
                    .as_ref()
                    .map(Meta::fetched_time);
                let label = e.label.lock().expect("label").clone();
                let state = e.state.lock().expect("state");
                let (fetched_at, last_error) = match &*state {
                    ResourceState::Available { fetched_at, .. } => (Some(*fetched_at), None),
                    ResourceState::Failed { last_error, .. } => {
                        (last_check, Some(last_error.clone()))
                    }
                    ResourceState::Missing => (None, None),
                };
                ResourceStatus {
                    source: e.source.clone(),
                    state: state.kind(),
                    version: state.data().map(|(_, v)| v).unwrap_or(0),
                    fetched_at,
                    next_refresh: *e.next_refresh.lock().expect("next"),
                    last_error,
                    kind: e.kind,
                    label,
                    key: resource_key(&e.source),
                    local: matches!(e.source, ResourceSource::File(_)),
                    ready: state.data().is_some(),
                    updating: e.updating.load(Ordering::Acquire),
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
                    let fetched_at = meta.fetched_time();
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
                self.spawn(url_task(self.weak(), entry, self.gone.subscribe()));
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
                        tracing::warn!(resource = %entry.log_name(), error = ?e.kind, "cannot watch file; changes need a reload");
                        None
                    }
                };
                self.spawn(file_task(self.weak(), entry, rx, watcher));
            }
        }
    }
}

/// `SystemTime::now() + due`, without panicking when the result cannot be
/// represented: a huge `update-interval` (reachable from `policy-path` and
/// RULE-SET, both accept any positive integer) is large enough to overflow
/// `SystemTime` on Windows. `None` then, meaning simply that no next refresh
/// is scheduled — the same as `due` itself being `None`.
fn next_refresh_at(due: Option<Duration>) -> Option<SystemTime> {
    due.and_then(|d| SystemTime::now().checked_add(d))
}

fn next_due(entry: &Entry, backoff: Option<Duration>, never_early: bool) -> Option<Duration> {
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
            Some(jitter(
                interval.saturating_sub(elapsed),
                clock_seed(),
                never_early,
            ))
        }
    }
}

/// Whether `body` is, byte for byte, what the entry holds: a copy it has
/// fetched, or one it has kept through a failure.
fn holds(entry: &Entry, body: &Bytes) -> bool {
    let held = entry.state.lock().expect("state").data();
    held.is_some_and(|(held, _)| held.as_ref() == body)
}

/// A check that found nothing new: what the entry holds is as current as now.
/// The disk is written first, so that whoever sees the new time in memory
/// finds it there too. `revise` changes the rest of the metadata, if the
/// check has anything to say about it.
fn checked(entry: &Entry, cache: &cache::CacheDir, revise: impl FnOnce(&mut Meta)) {
    let now = SystemTime::now();
    if let Some(m) = entry.meta.lock().expect("meta").as_mut() {
        m.stamp(now);
        revise(m);
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
}

async fn url_task(weak: Weak<ResourceManager>, entry: Arc<Entry>, mut gone: watch::Receiver<()>) {
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
        let due = next_due(&entry, backoff, opts.never_early);
        *entry.next_refresh.lock().expect("next") = next_refresh_at(due);
        // `due` is `None` only when auto-refresh is disabled (negative interval): nothing is due
        // then, but another user of the same resource may ask for auto-refresh (`merge_interval`
        // turns the interval positive), and waking once a minute is how the task comes to see
        // it. The manager going is `gone`'s business.
        let wait = due.unwrap_or(Duration::from_secs(60));
        let kicked = tokio::select! {
            _ = tokio::time::sleep(wait) => false,
            _ = entry.kick.notified() => true,
            // nothing is ever sent: this is the manager going
            _ = gone.changed() => return,
        };
        if weak.upgrade().is_none() {
            return;
        }
        if due.is_none() && !kicked {
            // nothing is due: look at the interval again
            continue;
        }
        let meta = entry.meta.lock().expect("meta").clone();
        // Held until the result is recorded below, or until the attempt is
        // dropped
        let _updating = Updating::begin(&entry.updating);
        let fetched = fetch::fetch(
            &client,
            &url,
            meta.as_ref(),
            opts.fetch_timeout,
            opts.max_size,
        )
        .await
        .and_then(|fetched| match fetched {
            // a server that answers "not modified" to a request that carried no validator: there
            // is nothing to refresh, which is a failed fetch like any other
            fetch::Fetched::NotModified if entry.state.lock().expect("state").data().is_none() => {
                Err(r#"the server answered "not modified" but nothing is cached"#.to_string())
            }
            fetched => Ok(fetched),
        });
        match fetched {
            // A body that is what the entry holds is no new version: a server
            // that sends no validators, or ones that do not hold, answers
            // every refresh with the whole body, and nothing downstream has
            // anything to reload. It is a check that found nothing new, and
            // the validators it came with are the ones to send next time.
            Ok(fetch::Fetched::New {
                data,
                etag,
                last_modified,
            }) if holds(&entry, &data) => {
                checked(&entry, &cache, |meta| {
                    meta.etag = etag;
                    meta.last_modified = last_modified;
                });
                backoff = None;
            }
            Ok(fetch::Fetched::New {
                data,
                etag,
                last_modified,
            }) => {
                let version = entry.version() + 1;
                let now = SystemTime::now();
                let mut new_meta = Meta {
                    url: url.to_string(),
                    etag,
                    last_modified,
                    ..Meta::default()
                };
                new_meta.stamp(now);
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
                checked(&entry, &cache, |_| {});
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

    /// A resource's URL often carries a token, and its body the credentials
    /// of a subscription's nodes: a `Debug` shows where the URL goes and how
    /// long the body is, no more.
    #[test]
    fn the_debug_of_a_resource_shows_neither_its_url_nor_its_body() {
        let url: Url = "https://sub.example:8443/list?token=S3CR3T-token"
            .parse()
            .unwrap();
        let body = Arc::new(Bytes::from_static(b"A = trojan, password=S3CR3T-body"));
        let shown = format!(
            "{:?} {:?} {:?} {:?} {:?}",
            ResourceSpec {
                source: ResourceSource::Url(url.clone()),
                update_interval: None,
                kind: ResourceKind::Profile,
            },
            ResourceState::Available {
                data: body.clone(),
                version: 1,
                fetched_at: UNIX_EPOCH,
                stale: false,
            },
            ResourceState::Failed {
                last_error: "timed out".to_string(),
                since: UNIX_EPOCH,
                cached: Some((body, 1)),
            },
            Meta {
                url: url.to_string(),
                ..Meta::default()
            },
            ResourceSource::File(PathBuf::from("sets/a.list")),
        );
        assert!(!shown.contains("S3CR3T"), "{shown}");
        assert!(!shown.contains("token"), "{shown}");
        for told in ["https://sub.example:8443/", "32 bytes", "a.list"] {
            assert!(shown.contains(told), "{told}: {shown}");
        }
    }
    use crate::http::{HttpClient, HttpClientConfig};
    use crate::testing::{Lines, TestServer, seed_cache};
    use sha2::{Digest, Sha256};

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
        url_spec_of(ResourceKind::RuleSet, url, interval)
    }

    fn url_spec_of(kind: ResourceKind, url: Url, interval: Option<i64>) -> ResourceSpec {
        ResourceSpec {
            source: ResourceSource::Url(url),
            update_interval: interval,
            kind,
        }
    }

    /// A huge `update-interval` must never panic the refresh task: past what
    /// `SystemTime` can represent, there is simply no next refresh.
    #[test]
    fn next_refresh_at_never_panics_on_an_unrepresentable_duration() {
        assert!(next_refresh_at(Some(Duration::from_secs(u64::MAX))).is_none());
        assert!(next_refresh_at(None).is_none());
        assert!(next_refresh_at(Some(Duration::from_secs(1))).is_some());
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
            kind: ResourceKind::RuleSet,
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
            kind: ResourceKind::RuleSet,
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

    /// The URL of a rule set can carry a user name and a password. A resource
    /// nobody labelled is named by its URL in the log lines, without them; its
    /// path and its query, which name the set, stay.
    #[tokio::test]
    async fn an_unlabelled_url_is_logged_without_its_user_info() {
        let root = tempfile::tempdir().unwrap();
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(root.path(), offline);
        for (url, shown) in [
            (
                "https://al1ce:S3CR3T@sets.example:8443/gh/user/repo@main/a.list?v=1",
                "https://***@sets.example:8443/gh/user/repo@main/a.list?v=1",
            ),
            (
                "https://al1ce@sets.example/b.list",
                "https://***@sets.example/b.list",
            ),
            (
                "https://sets.example/gh/user/repo@main/c.list",
                "https://sets.example/gh/user/repo@main/c.list",
            ),
        ] {
            let handle = mgr.get(&url_spec(Url::parse(url).unwrap(), None));
            assert_eq!(handle.entry.log_name(), shown, "{url}");
        }
    }

    /// With auto-refresh off (`update-interval` below zero), once something is
    /// fetched, what makes a URL task fetch is a kick and what makes it end is
    /// its manager going: `force_update` makes it fetch again, and it ends
    /// when the manager is dropped. It also wakes once a minute, to look at its
    /// interval again; that wake fetches nothing.
    #[tokio::test]
    async fn a_url_task_without_auto_refresh_waits_for_a_kick_and_for_its_manager() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/k", "kilo");
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(server.url("/k"), Some(-1)));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        assert_eq!(server.hits("/k"), 1);
        server.set("/k", "kilo two");
        assert!(mgr.force_update(&ResourceSource::Url(server.url("/k"))));
        let s = wait_for(&h, "v2", |s| available_version(s) == Some(2)).await;
        assert_eq!(&s.data().unwrap().0[..], b"kilo two");
        assert_eq!(server.hits("/k"), 2);
        let entry = Arc::downgrade(&h.entry);
        drop(h);
        drop(mgr);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while entry.strong_count() > 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the task still holds its entry"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A manager that is dropped takes its URL tasks with it: a reload
    /// leaves no task behind that sits out the rest of its refresh interval
    /// — a day here — with the entry and its body.
    #[tokio::test(start_paused = true)]
    async fn a_url_task_ends_with_its_manager() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let url = server.url("/day");
        // fresh in the cache: nothing is due for a day
        let mut meta = Meta {
            url: url.to_string(),
            ..Meta::default()
        };
        meta.stamp(SystemTime::now());
        cache::CacheDir::for_url(root.path(), url.as_str())
            .store(b"cached", &meta)
            .unwrap();
        let mgr = manager(root.path(), fast());
        let handle = mgr.get(&url_spec(url, Some(86_400)));
        // the task has taken its entry and waits for the refresh
        let waiting = || mgr.statuses()[0].next_refresh.is_some();
        for _ in 0..100 {
            if waiting() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(waiting(), "the task never ran");
        let entry = Arc::downgrade(&handle.entry);
        drop(handle);
        drop(mgr);
        tokio::time::sleep(Duration::from_secs(61)).await;
        assert_eq!(entry.strong_count(), 0, "the task still holds its entry");
        assert_eq!(server.hits("/day"), 0);
    }

    /// A profile can write `update-interval=0`. Nothing says what that means;
    /// a refresh "every 0 seconds" would be due again as soon as it is done,
    /// and the task would fetch as fast as the server answers. 0 is read as
    /// not written: the default interval.
    #[tokio::test]
    async fn an_interval_of_zero_is_read_as_none_written() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/zero", "zulu");
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(server.url("/zero"), Some(0)));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        // that nothing more happens can only be seen by waiting
        tokio::time::sleep(Duration::from_millis(300)).await;
        let hits = server.hits("/zero");
        assert!(hits <= 2, "fetched {hits} times in 300 ms");
        assert_eq!(h.version(), 1);
        assert_eq!(h.entry.effective_interval(), DEFAULT_UPDATE_INTERVAL);
    }

    /// The same in the other doors of the interval: a 0 that is merged into
    /// the interval of a resource somebody else asked for changes nothing,
    /// and a resource first asked for with 0 takes the interval of the next.
    #[tokio::test]
    async fn an_interval_of_zero_does_not_merge_into_a_shared_one() {
        let root = tempfile::tempdir().unwrap();
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(root.path(), offline);
        let one = Url::parse("https://lists.test/one").unwrap();
        let first = mgr.get(&url_spec(one.clone(), Some(3600)));
        mgr.get(&url_spec(one, Some(0)));
        assert_eq!(first.entry.effective_interval(), 3600);
        let two = Url::parse("https://lists.test/two").unwrap();
        let second = mgr.get(&url_spec(two.clone(), Some(0)));
        assert_eq!(second.entry.effective_interval(), DEFAULT_UPDATE_INTERVAL);
        mgr.get(&url_spec(two.clone(), Some(600)));
        assert_eq!(second.entry.effective_interval(), 600);
        mgr.get(&url_spec(two, Some(-1)));
        assert_eq!(second.entry.effective_interval(), 600);
        // 0 is none written, and none written leaves auto-refresh off as asked
        let off = Url::parse("https://lists.test/off").unwrap();
        let third = mgr.get(&url_spec(off.clone(), Some(-1)));
        mgr.get(&url_spec(off, Some(0)));
        assert_eq!(third.entry.effective_interval(), -1);
    }

    /// A server that answers "not modified" to a request that carried no
    /// validator leaves a resource with nothing cached nothing to refresh
    /// from: a failed fetch like any other. The state says so, and the task
    /// waits out its back-off instead of asking again at once.
    #[tokio::test]
    async fn a_not_modified_with_nothing_cached_is_a_failed_fetch() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/nm", "");
        server.set_status("/nm", 304);
        // long enough that no second request fits in the wait below
        let opts = ResourceOptions {
            min_backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(5),
            ..fast()
        };
        let mgr = manager(root.path(), opts);
        let h = mgr.get(&url_spec(server.url("/nm"), None));
        wait_for(&h, "the failed fetch", |s| {
            matches!(s, ResourceState::Failed { .. })
        })
        .await;
        // that nothing more happens can only be seen by waiting
        tokio::time::sleep(Duration::from_millis(300)).await;
        let hits = server.hits("/nm");
        let state = h.current();
        assert_eq!(hits, 1, "asked {hits} times in 300 ms; {state:?}");
        assert!(
            matches!(
                state,
                ResourceState::Failed { ref last_error, cached: None, .. }
                    if last_error == r#"the server answered "not modified" but nothing is cached"#
            ),
            "{state:?}"
        );
        assert_eq!(mgr.statuses()[0].state, "failed");
    }

    /// With auto-refresh off nothing is due, but the resource may have other
    /// users: one that asks for auto-refresh turns the interval positive
    /// (`merge_interval`), and a parked task comes to see that by its wake
    /// once a minute. Without the wake such a resource is never refreshed.
    #[tokio::test(start_paused = true)]
    async fn a_parked_url_task_sees_an_interval_merged_in_later() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let url = server.url("/parked");
        // fresh in the cache: nothing is due for a day, whatever the interval
        let mut meta = Meta {
            url: url.to_string(),
            ..Meta::default()
        };
        meta.stamp(SystemTime::now());
        cache::CacheDir::for_url(root.path(), url.as_str())
            .store(b"cached", &meta)
            .unwrap();
        let mgr = manager(root.path(), fast());
        mgr.get(&url_spec(url.clone(), Some(-1)));
        // the clock moves when the task has nothing left to do: it has parked
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(mgr.statuses()[0].next_refresh, None);
        // a second user of the same URL asks for a refresh every day
        mgr.get(&url_spec(url, Some(86_400)));
        tokio::time::sleep(Duration::from_secs(61)).await;
        assert!(
            mgr.statuses()[0].next_refresh.is_some(),
            "the task never looked at the interval again"
        );
        assert_eq!(server.hits("/parked"), 0);
    }

    /// Waits, for at most ten seconds, until `done`.
    async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn status_of(mgr: &ResourceManager, source: &ResourceSource) -> ResourceStatus {
        mgr.statuses()
            .into_iter()
            .find(|s| &s.source == source)
            .expect("a status for the source")
    }

    /// What the status says about a resource besides its state: what it is
    /// for, what it is called, the key that names its cache, whether it is a
    /// file, whether there is a copy to give.
    #[tokio::test]
    async fn the_status_says_what_a_resource_is() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/s", "sierra");
        let url = server.url("/s");
        let source = ResourceSource::Url(url.clone());
        let mgr = manager(root.path(), fast());
        let spec = url_spec_of(ResourceKind::DomainSet, url.clone(), None);
        let h = mgr.get_labelled(&spec, "the set");
        // nothing has run yet on this runtime
        assert!(!status_of(&mgr, &source).ready);
        wait_for(&h, "available", |s| available_version(s) == Some(1)).await;
        let status = status_of(&mgr, &source);
        assert_eq!(status.kind, ResourceKind::DomainSet);
        assert_eq!(status.label.as_deref(), Some("the set"));
        assert_eq!(status.shown(), "the set");
        assert!(!status.local);
        assert!(status.ready);
        let digest = format!("{:x}", Sha256::digest(url.as_str().as_bytes()));
        assert_eq!(status.key, digest);
        assert_eq!(resource_key(&source), digest);
        let dir = cache::CacheDir::for_url(root.path(), url.as_str());
        assert_eq!(dir.path().file_name().unwrap(), digest.as_str());

        // a file is local, and its key is that of its own `key()`
        let file = root.path().join("local.list");
        std::fs::write(&file, "DOMAIN,local.test").unwrap();
        let local = ResourceSource::File(file.clone());
        mgr.get(&ResourceSpec {
            source: local.clone(),
            update_interval: None,
            kind: ResourceKind::System,
        });
        let status = status_of(&mgr, &local);
        assert!(status.local && status.ready, "{status:?}");
        assert_eq!(status.kind, ResourceKind::System);
        assert_eq!(status.label, None);
        assert_eq!(status.shown(), file.display().to_string());
        assert_eq!(
            status.key,
            format!("{:x}", Sha256::digest(local.key().as_bytes()))
        );
    }

    /// Without a label a URL is shown as the log lines show it: the user name
    /// and the password are `***`.
    #[tokio::test]
    async fn an_unlabelled_status_is_shown_without_the_user_info() {
        let root = tempfile::tempdir().unwrap();
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(root.path(), offline);
        let url = Url::parse("http://al1ce:S3CR3T@sets.example:8080/a.list?v=1").unwrap();
        mgr.get(&url_spec(url, None));
        let shown = mgr.statuses()[0].shown();
        assert_eq!(shown, "http://***@sets.example:8080/a.list?v=1");
        assert!(!shown.contains("S3CR3T"), "{shown}");
        assert!(!shown.contains("al1ce"), "{shown}");
    }

    /// Two callers that register one source with different kinds share one
    /// resource, and the first one's kind is the one it has.
    #[tokio::test]
    async fn the_first_registration_fixes_the_kind() {
        let root = tempfile::tempdir().unwrap();
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(root.path(), offline);
        let url = Url::parse("https://sets.test/shared.list").unwrap();
        let first = mgr.get(&url_spec_of(ResourceKind::RuleSet, url.clone(), None));
        let second = mgr.get(&url_spec_of(ResourceKind::DomainSet, url.clone(), None));
        assert!(Arc::ptr_eq(&first.entry, &second.entry));
        let statuses = mgr.statuses();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].kind, ResourceKind::RuleSet);
        // and the other way round, on a source of its own
        let other = Url::parse("https://sets.test/other.list").unwrap();
        mgr.get(&url_spec_of(ResourceKind::DomainSet, other.clone(), None));
        mgr.get(&url_spec_of(ResourceKind::RuleSet, other.clone(), None));
        assert_eq!(
            status_of(&mgr, &ResourceSource::Url(other)).kind,
            ResourceKind::DomainSet
        );
    }

    /// `updating` is on from the request to the recorded result.
    #[tokio::test]
    async fn updating_lasts_from_the_request_to_the_recorded_result() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/slow", "sloth");
        server.set_delay("/slow", Duration::from_secs(2));
        let source = ResourceSource::Url(server.url("/slow"));
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(server.url("/slow"), None));
        // the task has not run yet on this runtime
        assert!(!status_of(&mgr, &source).updating);
        // the request is at the server, which holds the answer back
        wait_until("the request to arrive", || server.hits("/slow") == 1).await;
        let status = status_of(&mgr, &source);
        assert!(status.updating, "{status:?}");
        assert!(!status.ready, "{status:?}");
        wait_for(&h, "available", |s| available_version(s) == Some(1)).await;
        wait_until("the download to be over", || {
            !status_of(&mgr, &source).updating
        })
        .await;
        assert!(status_of(&mgr, &source).ready);
    }

    /// A resource that fails after it has had a copy keeps the time of its
    /// last success and the copy; one that never had one has neither.
    #[tokio::test]
    async fn a_failed_resource_keeps_the_time_of_its_last_success() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/f", "foxtrot");
        server.set("/never", "never");
        server.set_status("/never", 500);
        let mgr = manager(root.path(), fast());
        let source = ResourceSource::Url(server.url("/f"));
        let h = mgr.get(&url_spec(server.url("/f"), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let succeeded = status_of(&mgr, &source).fetched_at.expect("a time");
        server.set_status("/f", 500);
        assert!(mgr.force_update(&source));
        wait_for(&h, "failed with a copy", |s| {
            matches!(
                s,
                ResourceState::Failed {
                    cached: Some(_),
                    ..
                }
            )
        })
        .await;
        let status = status_of(&mgr, &source);
        assert_eq!(status.state, "failed");
        assert_eq!(status.fetched_at, Some(succeeded));
        assert!(status.ready, "{status:?}");
        assert!(status.last_error.is_some(), "{status:?}");

        let never = ResourceSource::Url(server.url("/never"));
        let h = mgr.get(&url_spec(server.url("/never"), None));
        wait_for(&h, "failed", |s| matches!(s, ResourceState::Failed { .. })).await;
        let status = status_of(&mgr, &never);
        assert_eq!(status.fetched_at, None);
        assert!(!status.ready, "{status:?}");
        assert!(status.last_error.is_some(), "{status:?}");
    }

    /// A "not modified" answer moves the time of the last check forward, and
    /// the time on disk is the one in memory, to the nanosecond.
    #[tokio::test]
    async fn a_not_modified_moves_the_time_and_the_cache_says_the_same() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/c", "charlie");
        let url = server.url("/c");
        let source = ResourceSource::Url(url.clone());
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let first = status_of(&mgr, &source).fetched_at.expect("a time");
        assert!(mgr.force_update(&source));
        wait_until("the check to be recorded", || {
            status_of(&mgr, &source).fetched_at > Some(first)
        })
        .await;
        let reqs = server.requests();
        assert!(reqs[1].header("if-none-match").is_some());
        let status = status_of(&mgr, &source);
        let copy = cached_copy(root.path(), &url).expect("a cache");
        assert_eq!(Some(copy.fetched_at), status.fetched_at);
        assert_eq!(h.version(), 1);
    }

    /// A body that is what the manager holds is no new version, from a server
    /// that cannot say whether it changed: nobody is told, nothing is logged
    /// as an update, and the check still counts.
    #[tokio::test]
    async fn a_200_with_the_body_in_hand_is_not_a_new_version() {
        let lines = Lines::default();
        let _capturing = lines.capture();
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/u", "uniform");
        server.set_validators("/u", false);
        let url = server.url("/u");
        let source = ResourceSource::Url(url.clone());
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url, None));
        let mut rx = h.subscribe();
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        rx.borrow_and_update();
        let first = status_of(&mgr, &source).fetched_at.expect("a time");
        lines.forget();

        assert!(mgr.force_update(&source));
        wait_until("the check to be recorded", || {
            status_of(&mgr, &source).fetched_at > Some(first)
        })
        .await;
        assert_eq!(server.hits("/u"), 2, "the server sent the whole body again");
        assert_eq!(h.version(), 1);
        assert!(!rx.has_changed().unwrap(), "somebody was told");
        assert_eq!(status_of(&mgr, &source).state, "available");
        assert_eq!(lines.said("resource updated"), Vec::<String>::new());

        // a body that differs is a new version, as before
        server.set("/u", "different");
        assert!(mgr.force_update(&source));
        let s = wait_for(&h, "v2", |s| available_version(s) == Some(2)).await;
        assert_eq!(&s.data().unwrap().0[..], b"different");
        assert!(rx.has_changed().unwrap());
        assert_eq!(lines.said("resource updated").len(), 1);
    }

    /// The same after a restart: a copy an earlier run cached, from a server
    /// that sends no validators and the same body, stays version 1.
    #[tokio::test]
    async fn a_copy_cached_by_an_earlier_run_is_not_replaced_by_the_same_body() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/r", "romeo");
        server.set_validators("/r", false);
        let url = server.url("/r");
        let source = ResourceSource::Url(url.clone());
        let long_ago = SystemTime::now() - Duration::from_secs(2 * 86_400);
        seed_cache(root.path(), &url, b"romeo", long_ago);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        assert_eq!(h.version(), 1);
        // a day old: due at once
        wait_until("the check to be recorded", || {
            status_of(&mgr, &source).fetched_at > Some(long_ago + Duration::from_secs(86_400))
        })
        .await;
        assert_eq!(server.hits("/r"), 1);
        assert_eq!(h.version(), 1);
        let copy = cached_copy(root.path(), &url).unwrap();
        assert_eq!(Some(copy.fetched_at), status_of(&mgr, &source).fetched_at);
    }

    /// The validators of the answer replace the ones stored, also when the
    /// body is the same: the next request carries the new ones.
    #[tokio::test]
    async fn the_validators_of_an_unchanged_200_replace_the_stored_ones() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/v", "victor");
        server.set_validators("/v", false);
        let url = server.url("/v");
        let source = ResourceSource::Url(url.clone());
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url, None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        // from now on the server names the body, but still never says 304
        server.set_header("/v", "etag", "\"fresh\"");
        for asked in [2, 3] {
            let before = status_of(&mgr, &source).fetched_at;
            assert!(mgr.force_update(&source));
            wait_until("the check to be recorded", || {
                server.hits("/v") == asked && status_of(&mgr, &source).fetched_at > before
            })
            .await;
        }
        let reqs = server.requests();
        assert!(reqs[0].header("if-none-match").is_none());
        assert!(reqs[1].header("if-none-match").is_none());
        assert_eq!(reqs[2].header("if-none-match"), Some("\"fresh\""));
        assert_eq!(h.version(), 1);
    }

    /// A cache written before the nanoseconds were kept reads back, as whole
    /// seconds.
    #[tokio::test]
    async fn a_cache_from_before_the_nanoseconds_reads_back() {
        let root = tempfile::tempdir().unwrap();
        let url = Url::parse("https://sets.test/old.list").unwrap();
        let dir = cache::CacheDir::for_url(root.path(), url.as_str());
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join("data"), "old").unwrap();
        std::fs::write(
            dir.path().join("meta.json"),
            r#"{"url":"https://sets.test/old.list","etag":null,"last_modified":null,"fetched_at":1700000000}"#,
        )
        .unwrap();
        let then = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let copy = cached_copy(root.path(), &url).expect("a cache");
        assert_eq!((&copy.data[..], copy.fetched_at), (&b"old"[..], then));
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(root.path(), offline);
        let h = mgr.get(&url_spec(url, None));
        assert!(
            matches!(h.current(), ResourceState::Available { fetched_at, .. } if fetched_at == then)
        );
    }

    #[tokio::test]
    async fn cached_copy_gives_the_body_the_file_and_the_time() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/cc", "copy");
        let url = server.url("/cc");
        let source = ResourceSource::Url(url.clone());
        assert!(cached_copy(root.path(), &url).is_none());
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let copy = cached_copy(root.path(), &url).expect("a cache");
        assert_eq!(&copy.data[..], b"copy");
        let file = root
            .path()
            .join("resources")
            .join(resource_key(&source))
            .join("data");
        assert_eq!(copy.path, file);
        assert_eq!(std::fs::read(&file).unwrap(), b"copy");
        assert_eq!(Some(copy.fetched_at), status_of(&mgr, &source).fetched_at);
        drop(h);
        drop(mgr);
        // next to the body, metadata that cannot be read and none at all: no copy
        let meta = file.with_file_name("meta.json");
        std::fs::write(&meta, "{").unwrap();
        assert!(cached_copy(root.path(), &url).is_none());
        std::fs::remove_file(&meta).unwrap();
        assert!(cached_copy(root.path(), &url).is_none());
        // and the metadata without the body
        let lone = Url::parse("https://sets.test/lone.list").unwrap();
        let dir = cache::CacheDir::for_url(root.path(), lone.as_str());
        dir.store_meta(&Meta::default()).unwrap();
        assert!(cached_copy(root.path(), &lone).is_none());
    }

    #[tokio::test]
    async fn a_seeded_cache_reads_back_and_registers_as_version_one() {
        let root = tempfile::tempdir().unwrap();
        let url = Url::parse("https://sets.test/seeded.list").unwrap();
        let at = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_700);
        seed_cache(root.path(), &url, b"seeded", at);
        let copy = cached_copy(root.path(), &url).expect("a cache");
        assert_eq!((&copy.data[..], copy.fetched_at), (&b"seeded"[..], at));
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(root.path(), offline);
        let h = mgr.get(&url_spec(url, None));
        assert!(
            matches!(
                h.current(),
                ResourceState::Available { version: 1, fetched_at, .. } if fetched_at == at
            ),
            "{:?}",
            h.current()
        );
    }

    /// The spread of a refresh time: ±10 % by default, 0 ~ +10 % when the
    /// interval is the shortest time before an update may happen.
    #[test]
    fn a_refresh_is_spread_by_ten_percent_and_never_early_only_later() {
        let interval = Duration::from_secs(1000);
        for seed in 0..21 {
            let spread = jitter(interval, seed, false);
            assert!(
                spread >= interval * 9 / 10 && spread <= interval * 11 / 10,
                "{seed}: {spread:?}"
            );
            let later = jitter(interval, seed, true);
            assert!(
                later >= interval && later <= interval * 11 / 10,
                "{seed}: {later:?}"
            );
        }
        // the whole range is used, and the seed wraps
        assert_eq!(jitter(interval, 0, false), interval * 9 / 10);
        assert_eq!(jitter(interval, 10, false), interval);
        assert_eq!(jitter(interval, 20, false), interval * 11 / 10);
        assert_eq!(jitter(interval, 21, false), jitter(interval, 0, false));
        assert_eq!(jitter(interval, 0, true), interval);
        assert_eq!(jitter(interval, 20, true), interval * 11 / 10);
        // not even a fraction of a millisecond early
        let odd = Duration::from_micros(1_234_567);
        assert!((0..21).all(|seed| jitter(odd, seed, true) >= odd));
        // nothing overflows
        assert_eq!(jitter(Duration::MAX, 20, true), Duration::MAX);
        assert!(jitter(Duration::ZERO, 7, false).is_zero());
    }

    /// The option reaches the timer: none of the refresh times of a resource
    /// that was last checked a moment ago is before its interval is over.
    #[tokio::test]
    async fn never_early_reaches_the_refresh_timer() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let checked = SystemTime::now();
        let urls: Vec<Url> = (0..30).map(|n| server.url(&format!("/n{n}"))).collect();
        for url in &urls {
            seed_cache(root.path(), url, b"seeded", checked);
        }
        let opts = ResourceOptions {
            never_early: true,
            ..fast()
        };
        let mgr = manager(root.path(), opts);
        for url in &urls {
            mgr.get(&url_spec(url.clone(), Some(1000)));
        }
        wait_until("every task to schedule its refresh", || {
            mgr.statuses().iter().all(|s| s.next_refresh.is_some())
        })
        .await;
        for status in mgr.statuses() {
            let earliest = checked + Duration::from_secs(1000);
            assert!(
                status.next_refresh.unwrap() >= earliest,
                "{} is due before its interval is over",
                status.key
            );
        }
        assert_eq!(server.requests().len(), 0);
    }

    /// The URL of a profile source is a credential as a whole, and the status
    /// and the log are read by people who must not be given it: no failure
    /// says any part of it — user name, password, path, query — nor what a
    /// redirect's `Location` says.
    #[tokio::test]
    async fn a_failure_never_quotes_the_url_it_came_from() {
        let lines = Lines::default();
        let _capturing = lines.capture();
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let port = server.url("/").port().unwrap();
        server.set("/S3CR3T/status", "x");
        server.set_status("/S3CR3T/status", 500);
        server.set("/S3CR3T/big", vec![b'x'; 100]);
        server.set("/S3CR3T/slow", "x");
        server.set_delay("/S3CR3T/slow", Duration::from_secs(5));
        for (path, to) in [
            ("/S3CR3T/loop", "/S3CR3T/loop?token=S3CR3T"),
            // not a port
            ("/S3CR3T/port", "http://S3CR3T:x/S3CR3T?token=S3CR3T"),
            // a scheme nobody speaks, and whose name is the secret
            ("/S3CR3T/scheme", "S3CR3T://elsewhere/S3CR3T?token=S3CR3T"),
        ] {
            server.set(path, "");
            server.set_status(path, 302);
            server.set_header(path, "location", to);
        }
        let opts = ResourceOptions {
            fetch_timeout: Duration::from_secs(1),
            max_size: 8,
            ..fast()
        };
        let mgr = manager(root.path(), opts);
        // each way a download can end badly, and what it says first
        let cases = [
            ("http", "/S3CR3T/missing", "http status 404"),
            ("http", "/S3CR3T/status", "http status 500"),
            ("http", "/S3CR3T/big", "body exceeds 8 bytes"),
            ("http", "/S3CR3T/slow", "timeout"),
            ("http", "/S3CR3T/loop", "too many redirects"),
            ("http", "/S3CR3T/port", "invalid url"),
            ("http", "/S3CR3T/scheme", "invalid url"),
            // a server that does not speak TLS
            ("https", "/S3CR3T/tls", "connect"),
        ];
        let sources: Vec<ResourceSource> = cases
            .iter()
            .map(|(scheme, path, _)| {
                let url = format!("{scheme}://al1ce:S3CR3T@127.0.0.1:{port}{path}?token=S3CR3T");
                let source = ResourceSource::Url(Url::parse(&url).unwrap());
                let spec = ResourceSpec {
                    source: source.clone(),
                    update_interval: None,
                    kind: ResourceKind::Profile,
                };
                mgr.get_labelled(&spec, "the profile");
                source
            })
            .collect();
        wait_until("every download to fail", || {
            mgr.statuses().iter().all(|s| s.state == "failed")
        })
        .await;
        let secret = |text: &str| {
            let text = text.to_ascii_lowercase();
            text.contains("s3cr3t") || text.contains("al1ce")
        };
        let errors: Vec<(&str, String)> = cases
            .iter()
            .zip(&sources)
            .map(|((_, path, _), source)| {
                (*path, status_of(&mgr, source).last_error.expect("an error"))
            })
            .collect();
        for ((_, _, says), (path, error)) in cases.iter().zip(&errors) {
            assert!(error.starts_with(says), "{path}: {error}");
        }
        let told: Vec<&(&str, String)> = errors.iter().filter(|(_, e)| secret(e)).collect();
        assert!(told.is_empty(), "{told:#?}\nall of them: {errors:#?}");
        let logged = lines.said("resource fetch failed");
        assert!(logged.len() >= cases.len(), "{logged:#?}");
        let leaked: Vec<&String> = logged.iter().filter(|line| secret(line)).collect();
        assert!(leaked.is_empty(), "{leaked:#?}");
    }
}
