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
use rurge_config::remote::origin;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Notify, watch};
use url::Url;

pub const DEFAULT_UPDATE_INTERVAL: i64 = 86_400;

/// Decides whether a body that was downloaded is taken (`get_checked`). It runs
/// on a thread of the blocking pool and under none of the manager's locks, so
/// it may call the manager: `adopt`, `get`, `statuses`. The entry keeps its
/// check for as long as it lives, so a check that calls the manager holds it
/// by a `Weak`. The reason it gives is the `last_error` of the resource, as it
/// is.
pub type Check = Arc<dyn Fn(&[u8]) -> Result<(), String> + Send + Sync>;

/// What `update_now` answers for an entry whose task is over: it was retired,
/// its manager is gone, or it never got a task.
const NO_LONGER_UPDATED: &str = "the resource is no longer updated";
const OFFLINE_NO_DOWNLOADS: &str = "offline mode: nothing is downloaded";
/// What a check that panicked comes to: the panic's own text is not repeated.
const CHECK_DID_NOT_FINISH: &str = "the check of the downloaded copy did not finish";
const NOT_MODIFIED_NOTHING_CACHED: &str =
    r#"the server answered "not modified" but nothing is cached"#;

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
    /// A download is under way, or the check of what it brought. One that was
    /// asked for and has not begun is not.
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

/// How far the attempts of a URL entry have come, for whoever waits for them.
/// It is state to wait on with a predicate (`Receiver::wait_for`), never an
/// event that can be missed: a waiter that arrives late finds the numbers as
/// they are.
struct Progress {
    /// Attempts begun. One begins when the task takes the validators for its
    /// request: an attempt that begins after a call is numbered above the
    /// `started` the call read.
    started: u64,
    /// The number of the last attempt whose result is recorded, and that
    /// result.
    finished: u64,
    last: Result<(), String>,
    /// The attempt somebody asked for: while `wanted > started` the task
    /// begins one without waiting for its timer or its backoff.
    wanted: u64,
    /// An attempt was due when the entry was registered: it had no copy, or a
    /// stale one.
    due_at_start: bool,
    /// `retire` was called.
    retired: bool,
    /// The task is over, or there never was one: nothing more will be
    /// attempted.
    ended: bool,
}

/// What an entry knows about the files of its cache, under the lock that is
/// held across them.
struct Disk {
    /// The disk may not hold what the entry holds: a write failed, and it may
    /// have got as far as the body, which the cache writes before the
    /// metadata. The next write that succeeds writes the body too.
    behind: bool,
}

/// `record` is the first lock of an entry to be taken: before `meta`, `state`
/// and the rest, and before the manager's `entries` in the one place that holds
/// both, `retire`. Nothing takes `record` while holding `entries`. Every lock
/// but `record` is held for a moment, never across file IO, the network, a
/// check or an `.await`, and `record` is held across nothing but file IO.
struct Entry {
    source: ResourceSource,
    /// `resource_key(&source)`, worked out once.
    key: String,
    kind: ResourceKind,
    /// What log lines call the resource instead of its URL (`get_labelled`).
    label: Mutex<Option<String>>,
    state: Mutex<ResourceState>,
    meta: Mutex<Option<Meta>>,
    interval: Mutex<Option<i64>>,
    next_refresh: Mutex<Option<SystemTime>>,
    updating: AtomicBool,
    tx: watch::Sender<u64>,
    /// Wakes the task of a file; a URL's task is woken through `progress`.
    kick: Notify,
    /// Decides about a body before it is taken (`get_checked`); the first one
    /// the entry is given stays.
    check: OnceLock<Check>,
    /// The one lock that is held across the files of the cache of this entry:
    /// whoever reads or writes them, and changes `meta` and `state` to go with
    /// what they hold, does so under it. A URL entry only.
    record: Mutex<Disk>,
    progress: watch::Sender<Progress>,
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

/// Tells whoever waits that the task of an entry is over, however it ends: a
/// return, a panic, the future being dropped, also before it ever ran. It is
/// the first argument of the task, so that it is the last thing of it to go.
struct Ends(Arc<Entry>);

impl Drop for Ends {
    fn drop(&mut self) {
        self.0.end();
    }
}

impl Entry {
    fn new(
        spec: &ResourceSpec,
        interval: Option<i64>,
        label: Option<&str>,
        check: Option<Check>,
    ) -> Entry {
        let (tx, _rx) = watch::channel(0u64);
        let entry = Entry {
            source: spec.source.clone(),
            key: resource_key(&spec.source),
            kind: spec.kind,
            label: Mutex::new(label.map(str::to_string)),
            state: Mutex::new(ResourceState::Missing),
            meta: Mutex::new(None),
            interval: Mutex::new(interval),
            next_refresh: Mutex::new(None),
            updating: AtomicBool::new(false),
            tx,
            kick: Notify::new(),
            check: OnceLock::new(),
            record: Mutex::new(Disk { behind: false }),
            // Due until `start` has seen the cache: whoever arrives between the
            // insertion into the map and `start` has to wait.
            progress: watch::Sender::new(Progress {
                started: 0,
                finished: 0,
                last: Ok(()),
                wanted: 0,
                due_at_start: true,
                retired: false,
                ended: false,
            }),
        };
        if let Some(check) = check {
            let _ = entry.check.set(check);
        }
        entry
    }

    fn retired(&self) -> bool {
        self.progress.borrow().retired
    }

    fn end(&self) {
        self.progress.send_modify(|p| p.ended = true);
    }

    /// Asks for an attempt that begins after this call, whatever the timer
    /// and the backoff of the task say, and gives its number. `None` when the
    /// task is over: nobody is left to make it.
    fn ask(&self) -> Option<u64> {
        let mut number = None;
        self.progress.send_if_modified(|p| {
            if p.ended {
                return false;
            }
            let next = p.started + 1;
            number = Some(next);
            if p.wanted >= next {
                return false;
            }
            p.wanted = next;
            true
        });
        number
    }

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

    /// The versions, as they are published. A value borrowed from the
    /// receiver (`borrow`, `borrow_and_update`) is not to be held on to: the
    /// next version waits for it, and with it whoever writes the cache of
    /// this resource.
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

/// A manager serves one configuration generation, or lives as long as the
/// process. A generation's: on reload, build a new manager and drop the old
/// one — cached files are re-read from disk, and the dropped manager's
/// background tasks exit: a URL task as soon as it waits for its next refresh
/// (`gone`), a file task within 60 s (the periodic liveness checks in
/// `follow_file`). The process's: an entry nobody needs any more is retired
/// (`retire`). It is no longer listed and its task ends at once, also
/// half-way through an attempt, which then writes and publishes nothing; a
/// handle that is still held reads the content the entry had last, and
/// registering the source again starts a new entry from the cache.
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

/// The file the body of `url` is kept in under `root`: where [`cached_copy`]
/// reads it from, and where a download of it will be written, which is known
/// before there is one.
pub fn cache_file(root: &Path, url: &Url) -> PathBuf {
    cache::CacheDir::for_url(root, url.as_str())
        .path()
        .join("data")
}

/// The cache of `url` under `root`, read without a manager and without the
/// network. `None` when one of its two files is missing, `meta.json` cannot
/// be read, or the body is not the one `meta.json` was written for.
pub fn cached_copy(root: &Path, url: &Url) -> Option<CachedCopy> {
    let (data, meta) = cache::CacheDir::for_url(root, url.as_str()).load()?;
    Some(CachedCopy {
        data,
        path: cache_file(root, url),
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
        self.register(spec, None, None)
    }

    /// `get` for a resource whose URL must not reach the logs — a
    /// subscription URL usually carries a token (phase 2 M3-D7): log lines
    /// call it `label` instead, also when another caller shares it. The first
    /// label a resource gets is the one it keeps.
    pub fn get_labelled(&self, spec: &ResourceSpec, label: &str) -> ResourceHandle {
        self.register(spec, Some(label), None)
    }

    /// `get_labelled` for a resource whose body is taken only after `check`
    /// has seen it. The check sees a body that is new: not the answer to a
    /// request for an update that says nothing changed, not a body the entry
    /// holds already, and not what the cache held when the entry started (it
    /// passed when it was written). A body it refuses leaves the cache, the
    /// version and the validators as they were; the resource is `failed`, with
    /// the reason as its `last_error`, and backs off as it does after any
    /// failed download; the next attempt downloads and checks again. The first
    /// check an entry is given stays, also when the entry was registered
    /// without one: that entry takes the first one it is given later. A file
    /// is not downloaded: it is registered as `get_labelled` registers it, and
    /// the check is never called.
    pub fn get_checked(&self, spec: &ResourceSpec, label: &str, check: Check) -> ResourceHandle {
        self.register(spec, Some(label), Some(check))
    }

    fn register(
        &self,
        spec: &ResourceSpec,
        label: Option<&str>,
        check: Option<Check>,
    ) -> ResourceHandle {
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
            // An entry that is retired and not yet taken out of the map by
            // `retire` is not there: the new one takes its place.
            if let Some(existing) = entries.get(&key).filter(|existing| !existing.retired()) {
                merge_interval(existing, interval);
                if let Some(label) = label {
                    existing
                        .label
                        .lock()
                        .expect("label")
                        .get_or_insert_with(|| label.to_string());
                }
                if let Some(check) = check {
                    let _ = existing.check.set(check);
                }
                (existing.clone(), false)
            } else {
                let entry = Arc::new(Entry::new(spec, interval, label, check));
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

    /// Asks for an update of `source` and does not wait for it. Asking twice
    /// before the task has begun one makes one; asked while an attempt is
    /// under way, one more follows it.
    pub fn force_update(&self, source: &ResourceSource) -> bool {
        let Some(entry) = self.entry_of(&source.key()) else {
            return false;
        };
        match entry.source {
            ResourceSource::Url(_) => {
                entry.ask();
            }
            ResourceSource::File(_) => entry.kick.notify_one(),
        }
        true
    }

    /// An update of the resource `key` names (`resource_key`), awaited: the
    /// first attempt that begins after this call, whatever its timer and its
    /// backoff say, and not one that is under way when it is made. `Ok` when
    /// the attempt brought a new version, a body the entry holds already, or a
    /// "not modified"; otherwise the reason, as `last_error` has it. `None`
    /// when there is no such entry. A file is read again and the answer is
    /// `Ok` at once; a manager that is offline downloads nothing and says so.
    /// An entry whose task is over, or that is retired while this waits,
    /// is an error too. The wait has no limit of its own: an attempt ends
    /// within `fetch_timeout` plus the time of its check, and a caller that
    /// wants less puts its own limit around the call.
    pub async fn update_now(&self, key: &str) -> Option<Result<(), String>> {
        let entry = self.entry_by_key(key)?;
        if matches!(entry.source, ResourceSource::File(_)) {
            entry.kick.notify_one();
            return Some(Ok(()));
        }
        if self.opts.offline {
            return Some(Err(OFFLINE_NO_DOWNLOADS.to_string()));
        }
        let mut progress = entry.progress.subscribe();
        let Some(number) = entry.ask() else {
            return Some(Err(NO_LONGER_UPDATED.to_string()));
        };
        // `finished` and `last` change together: when the attempt is the one
        // that finished last, so is its result
        let finished = progress
            .wait_for(|p| p.finished >= number || p.ended)
            .await
            .map(|p| (p.finished >= number).then(|| p.last.clone()));
        match finished {
            Ok(Some(last)) => Some(last),
            Ok(None) | Err(_) => Some(Err(NO_LONGER_UPDATED.to_string())),
        }
    }

    /// Waits until the entry `key` names has no attempt that is due and not
    /// done: one has ended since it was registered, or the copy it started
    /// from was not due. Returns at once for no such entry, a file, an entry
    /// that is retired or has no task (offline, or no runtime to run it on).
    pub async fn attempted(&self, key: &str) {
        let Some(entry) = self.entry_by_key(key) else {
            return;
        };
        if matches!(entry.source, ResourceSource::File(_)) {
            return;
        }
        let _ = entry
            .progress
            .subscribe()
            .wait_for(|p| p.ended || p.finished >= 1 || !p.due_at_start)
            .await;
    }

    /// Takes `data` as if it had just been downloaded from `source`: it goes
    /// into the cache with the time now and no validators, so that the next
    /// refresh fetches the whole body. If the source is registered, it is also
    /// the content of its entry: `available`, published, and a new version
    /// unless the entry holds that very body already. No check is made (the
    /// caller has made it), and a download that is under way is none of this
    /// call's business: it finds the body in hand when it brings the same,
    /// and a version more when it brings another. A file cannot be adopted.
    /// When the cache cannot be written the error is returned and the entry
    /// stays as it was. It is synchronous, so that a check can call it. Two
    /// adoptions at once of a source that nobody has registered are the
    /// caller's to keep apart: nothing here does.
    pub fn adopt(&self, source: &ResourceSource, data: Bytes) -> io::Result<()> {
        let ResourceSource::Url(url) = source else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only a resource that is downloaded can be adopted",
            ));
        };
        let cache = cache::CacheDir::for_url(&self.root, url.as_str());
        let now = SystemTime::now();
        let mut meta = Meta {
            url: url.to_string(),
            ..Meta::default()
        };
        meta.stamp(now);
        let entry = self
            .entries
            .lock()
            .expect("entries")
            .get(&source.key())
            .filter(|entry| !entry.retired())
            .cloned();
        // Not registered: the cache is all there is to write. A registration
        // that races this is harmless: the entry starts from whichever copy it
        // reads, and its next refresh brings the same body and finds nothing
        // new, or one new version.
        let Some(entry) = entry else {
            return cache.store(&data, &mut meta);
        };
        let mut disk = entry.record.lock().expect("record");
        if entry.retired() {
            // retired since the lookup
            return cache.store(&data, &mut meta);
        }
        if let Err(e) = cache.store(&data, &mut meta) {
            // it may have got as far as the body
            disk.behind = true;
            return Err(e);
        }
        disk.behind = false;
        let held = entry.state.lock().expect("state").data();
        let (version, new) = match held {
            // the same rule as for a download that brings the body in hand
            Some((held, version)) if held.as_ref() == &data => (version, false),
            held => (held.map_or(0, |(_, version)| version) + 1, true),
        };
        *entry.meta.lock().expect("meta") = Some(meta);
        entry.set_state(ResourceState::Available {
            data: Arc::new(data),
            version,
            fetched_at: now,
            stale: false,
        });
        if new {
            entry.publish(version);
        }
        Ok(())
    }

    /// Takes the entry of `source` out of service: it is no longer listed, its
    /// task ends at once (also one that waits for its next refresh), an attempt
    /// that is under way writes and publishes nothing, a handle that is still
    /// held reads the content the entry had last, and registering the source
    /// again starts a new entry from the cache. Whether this call retired it:
    /// `false` when there is no such entry, or somebody else retired it first.
    pub fn retire(&self, source: &ResourceSource) -> bool {
        let key = source.key();
        let Some(entry) = self.entries.lock().expect("entries").get(&key).cloned() else {
            return false;
        };
        // Under the lock the task records under, so that an attempt either is
        // recorded in full before this, or finds the entry retired and writes
        // nothing. The entry leaves the map in the same hold: a registration
        // that gets in between has put a new entry there already.
        let _disk = entry.record.lock().expect("record");
        let retired = entry
            .progress
            .send_if_modified(|p| !std::mem::replace(&mut p.retired, true));
        let mut entries = self.entries.lock().expect("entries");
        if entries
            .get(&key)
            .is_some_and(|held| Arc::ptr_eq(held, &entry))
        {
            entries.remove(&key);
        }
        retired
    }

    /// The entry of the source whose `ResourceSource::key` is `source_key`.
    fn entry_of(&self, source_key: &str) -> Option<Arc<Entry>> {
        self.entries
            .lock()
            .expect("entries")
            .get(source_key)
            .cloned()
    }

    /// The entry whose `resource_key` is `key`.
    fn entry_by_key(&self, key: &str) -> Option<Arc<Entry>> {
        self.entries
            .lock()
            .expect("entries")
            .values()
            .find(|entry| entry.key == key)
            .cloned()
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
                    key: e.key.clone(),
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
                {
                    let _disk = entry.record.lock().expect("record");
                    // `adopt` may have been here first, between the insertion
                    // into the map and now: what it put in is the copy
                    let missing =
                        matches!(*entry.state.lock().expect("state"), ResourceState::Missing);
                    if missing && let Some((data, meta)) = cache.load() {
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
                    if self.opts.offline
                        && matches!(*entry.state.lock().expect("state"), ResourceState::Missing)
                    {
                        entry.set_state(ResourceState::Failed {
                            last_error: "offline mode: no cached copy".to_string(),
                            since: SystemTime::now(),
                            cached: None,
                        });
                    }
                    // An attempt is due when there is no copy, or it is stale
                    let due = !matches!(
                        *entry.state.lock().expect("state"),
                        ResourceState::Available { stale: false, .. }
                    );
                    if !due {
                        entry.progress.send_modify(|p| p.due_at_start = false);
                    }
                }
                if self.opts.offline {
                    entry.end();
                    return;
                }
                let progress = entry.progress.subscribe();
                // Without a runtime the task is dropped as it is, and says so
                // as it goes
                self.spawn(url_task(
                    Ends(entry.clone()),
                    self.weak(),
                    entry.clone(),
                    self.gone.subscribe(),
                    progress,
                ));
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
                let progress = entry.progress.subscribe();
                self.spawn(file_task(self.weak(), entry, rx, watcher, progress));
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

/// What an attempt came to, before it is recorded.
enum Outcome {
    /// Nothing new: the server says nothing changed, or it sent the body the
    /// entry holds. A 200 comes with validators, which are the ones to send
    /// next time.
    Same {
        validators: Option<(Option<String>, Option<String>)>,
    },
    /// A body the entry does not hold, which its check, if it has one, took.
    New {
        data: Bytes,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    Failed(Failure),
}

enum Failure {
    /// The download failed.
    Fetch(String),
    /// The check refused the body.
    Check(String),
}

/// What the log says about an attempt that failed in some way, one line for
/// what failed.
enum Warning {
    Fetch(String),
    NotAdopted(String),
    Cache(String),
}

/// What is left to do once an attempt is recorded and its lock released.
struct Recorded {
    result: Result<(), String>,
    warning: Option<Warning>,
    /// The version a new body was published as.
    updated: Option<u64>,
}

/// One request for the resource and what comes of the answer: for a body that
/// is new, the verdict of the check the entry has. Nothing is written here, and
/// no lock is held while it waits.
async fn attempt(
    entry: &Entry,
    client: &HttpClient,
    url: &Url,
    validators: Option<&Meta>,
    opts: &ResourceOptions,
) -> Outcome {
    let fetched = fetch::fetch(client, url, validators, opts.fetch_timeout, opts.max_size).await;
    match fetched {
        Err(e) => Outcome::Failed(Failure::Fetch(e)),
        // a server that answers "not modified" to a request that carried no validator: there
        // is nothing to refresh, which is a failed fetch like any other
        Ok(fetch::Fetched::NotModified) if entry.state.lock().expect("state").data().is_none() => {
            Outcome::Failed(Failure::Fetch(NOT_MODIFIED_NOTHING_CACHED.to_string()))
        }
        Ok(fetch::Fetched::NotModified) => Outcome::Same { validators: None },
        // A body that is what the entry holds is no new version: a server that sends no
        // validators, or ones that do not hold, answers every refresh with the whole body, and
        // nothing downstream has anything to reload. It is a check that found nothing new, and
        // the validators it came with are the ones to send next time.
        Ok(fetch::Fetched::New {
            data,
            etag,
            last_modified,
        }) if holds(entry, &data) => Outcome::Same {
            validators: Some((etag, last_modified)),
        },
        Ok(fetch::Fetched::New {
            data,
            etag,
            last_modified,
        }) => {
            let new = Outcome::New {
                data: data.clone(),
                etag,
                last_modified,
            };
            let Some(check) = entry.check.get().cloned() else {
                return new;
            };
            // On a thread of the blocking pool, where it may take as long as loading a
            // profile; no lock is held while it runs or while it is awaited.
            match tokio::task::spawn_blocking(move || check(&data[..])).await {
                Ok(Ok(())) => new,
                Ok(Err(reason)) => Outcome::Failed(Failure::Check(reason)),
                // it panicked: what it said is not repeated
                Err(_) => Outcome::Failed(Failure::Check(CHECK_DID_NOT_FINISH.to_string())),
            }
        }
    }
}

/// A failed attempt: the entry is `failed` and keeps the copy it had.
fn failed_attempt(entry: &Entry, last_error: String, warning: Warning) -> Recorded {
    let cached = entry.state.lock().expect("state").data();
    entry.updating.store(false, Ordering::Release);
    entry.set_state(ResourceState::Failed {
        last_error: last_error.clone(),
        since: SystemTime::now(),
        cached,
    });
    Recorded {
        result: Err(last_error),
        warning: Some(warning),
        updated: None,
    }
}

/// An attempt that cannot go into the cache, for an entry whose readers go by
/// the disk.
fn not_written(entry: &Entry, error: &io::Error) -> Recorded {
    failed_attempt(
        entry,
        format!("cannot write the cache: {error}"),
        Warning::Cache(error.to_string()),
    )
}

/// What a write to the cache comes to. A failed one is `Err` when the readers
/// of the entry go by the disk: it must not be given what the disk does not
/// hold. For the others the entry goes on with what it has. Either way the
/// disk is marked as behind, and the next write that succeeds writes the body
/// too: a write of both files that fails may have replaced the body already.
fn wrote(
    disk: &mut Disk,
    written: io::Result<()>,
    by_disk: bool,
) -> Result<Option<Warning>, io::Error> {
    match written {
        Ok(()) => {
            disk.behind = false;
            Ok(None)
        }
        Err(e) => {
            disk.behind = true;
            if by_disk {
                Err(e)
            } else {
                Ok(Some(Warning::Cache(e.to_string())))
            }
        }
    }
}

/// Records what an attempt came to, in the cache and then in the entry, and
/// publishes a new version, all under the lock of the entry's cache files, so
/// that `adopt`, `retire` and the task never write over one another. `None`
/// when the entry is retired: nothing is written or changed then.
fn record_attempt(
    entry: &Entry,
    cache: &cache::CacheDir,
    url: &Url,
    outcome: Outcome,
) -> Option<Recorded> {
    let mut disk = entry.record.lock().expect("record");
    if entry.retired() {
        return None;
    }
    let now = SystemTime::now();
    // An entry with a check has readers that go by the disk
    let by_disk = entry.check.get().is_some();
    // `adopt` may have put this very body in while the answer, or its check,
    // was under way
    let outcome = match outcome {
        Outcome::New {
            data,
            etag,
            last_modified,
        } if holds(entry, &data) => Outcome::Same {
            validators: Some((etag, last_modified)),
        },
        outcome => outcome,
    };
    Some(match outcome {
        Outcome::Failed(Failure::Fetch(e)) => failed_attempt(entry, e.clone(), Warning::Fetch(e)),
        Outcome::Failed(Failure::Check(reason)) => {
            failed_attempt(entry, reason.clone(), Warning::NotAdopted(reason))
        }
        // What the entry holds is as current as now. The disk is written first, so that
        // whoever sees the new time in memory finds it there too.
        Outcome::Same { validators } => {
            let Some((data, version)) = entry.state.lock().expect("state").data() else {
                let e = NOT_MODIFIED_NOTHING_CACHED.to_string();
                return Some(failed_attempt(entry, e.clone(), Warning::Fetch(e)));
            };
            let held = entry.meta.lock().expect("meta").clone();
            // The body goes with the metadata unless the disk holds it, intact: a
            // write may have failed, there may never have been one, and the files
            // may have been damaged or replaced since. A cache from before the
            // digest was kept gets one this way.
            let intact = held
                .as_ref()
                .and_then(|meta| meta.sha256.as_deref())
                .is_some_and(|digest| cache.holds(digest));
            let whole = disk.behind || !intact;
            let mut meta = held.unwrap_or_else(|| Meta {
                url: url.to_string(),
                ..Meta::default()
            });
            meta.stamp(now);
            if let Some((etag, last_modified)) = validators {
                meta.etag = etag;
                meta.last_modified = last_modified;
            }
            let written = if whole {
                cache.store(&data, &mut meta)
            } else {
                cache.store_meta(&meta)
            };
            let warning = match wrote(&mut disk, written, by_disk) {
                Ok(warning) => warning,
                Err(e) => return Some(not_written(entry, &e)),
            };
            entry.updating.store(false, Ordering::Release);
            *entry.meta.lock().expect("meta") = Some(meta);
            entry.set_state(ResourceState::Available {
                data,
                version,
                fetched_at: now,
                stale: false,
            });
            Recorded {
                result: Ok(()),
                warning,
                updated: None,
            }
        }
        Outcome::New {
            data,
            etag,
            last_modified,
        } => {
            let version = entry.version() + 1;
            let mut meta = Meta {
                url: url.to_string(),
                etag,
                last_modified,
                ..Meta::default()
            };
            meta.stamp(now);
            let warning = match wrote(&mut disk, cache.store(&data, &mut meta), by_disk) {
                Ok(warning) => warning,
                Err(e) => return Some(not_written(entry, &e)),
            };
            // Down before anyone is told: whoever the new version wakes finds a download
            // that is over.
            entry.updating.store(false, Ordering::Release);
            *entry.meta.lock().expect("meta") = Some(meta);
            entry.set_state(ResourceState::Available {
                data: Arc::new(data),
                version,
                fetched_at: now,
                stale: false,
            });
            entry.publish(version);
            Recorded {
                result: Ok(()),
                warning,
                updated: Some(version),
            }
        }
    })
}

/// Ends when the entry is retired.
async fn until_retired(progress: &mut watch::Receiver<Progress>) {
    let _ = progress.wait_for(|p| p.retired).await;
}

async fn url_task(
    _ends: Ends,
    weak: Weak<ResourceManager>,
    entry: Arc<Entry>,
    mut gone: watch::Receiver<()>,
    mut progress: watch::Receiver<Progress>,
) {
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
        let asked = tokio::select! {
            biased;
            // Retired, or an attempt somebody asked for: a state that is looked at, so that one
            // asked for before the task got here is found.
            changed = progress.wait_for(|p| p.retired || p.wanted > p.started) => {
                // owned before anything else is done: it holds the channel's read lock
                match changed.map(|p| p.retired) {
                    Ok(false) => true,
                    Ok(true) | Err(_) => return,
                }
            }
            // nothing is ever sent: this is the manager going
            _ = gone.changed() => return,
            _ = tokio::time::sleep(wait) => false,
        };
        if weak.upgrade().is_none() {
            return;
        }
        if due.is_none() && !asked {
            // nothing is due: look at the interval again
            continue;
        }
        let number = {
            let mut number = 0;
            entry.progress.send_modify(|p| {
                p.started += 1;
                number = p.started;
            });
            number
        };
        let validators = entry.meta.lock().expect("meta").clone();
        // Held until the result is recorded below, or until the attempt is
        // dropped
        let _updating = Updating::begin(&entry.updating);
        // A retired entry's task ends wherever it is: in the request, in the check
        let outcome = tokio::select! {
            biased;
            _ = until_retired(&mut progress) => return,
            outcome = attempt(&entry, &client, &url, validators.as_ref(), &opts) => outcome,
        };
        let Some(recorded) = record_attempt(&entry, &cache, &url, outcome) else {
            return;
        };
        match &recorded.warning {
            Some(Warning::Fetch(error)) => {
                tracing::warn!(resource = %entry.log_name(), error = %error, "resource fetch failed");
            }
            Some(Warning::NotAdopted(error)) => {
                tracing::warn!(resource = %entry.log_name(), error = %error, "downloaded resource not adopted");
            }
            Some(Warning::Cache(error)) => {
                tracing::warn!(resource = %entry.log_name(), error = %error, "cannot write resource cache");
            }
            None => {}
        }
        if let Some(version) = recorded.updated {
            tracing::info!(resource = %entry.log_name(), version, "resource updated");
        }
        backoff = match &recorded.result {
            Ok(()) => None,
            Err(_) => Some(match backoff {
                None => opts.min_backoff,
                Some(b) => (b * 2).min(opts.max_backoff),
            }),
        };
        entry.progress.send_modify(|p| {
            p.finished = number;
            p.last = recorded.result;
        });
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

/// Follows the file of an entry until its manager is gone or the entry is
/// retired. The watcher is kept for as long as that lasts.
async fn file_task(
    weak: Weak<ResourceManager>,
    entry: Arc<Entry>,
    rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    _watcher: Option<local::AnyWatcher>,
    mut progress: watch::Receiver<Progress>,
) {
    tokio::select! {
        _ = until_retired(&mut progress) => {}
        _ = follow_file(weak, entry, rx) => {}
    }
}

async fn follow_file(
    weak: Weak<ResourceManager>,
    entry: Arc<Entry>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<()>,
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
    use crate::BoxFuture;
    use crate::connector::{DirectConnector, Resolve};
    use std::net::IpAddr;

    /// Refuses every name: a test here downloads from a `TestServer` by its
    /// address, so a name that is asked for would take it off the loopback.
    struct NoNames;

    impl Resolve for NoNames {
        fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(async move {
                Err(io::Error::other(format!(
                    "a test tried to resolve the name {host}"
                )))
            })
        }
    }

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

    /// The copy on disk is shown the same way: its body by its length.
    #[test]
    fn the_debug_of_a_cached_copy_shows_its_body_by_its_length() {
        let copy = CachedCopy {
            data: Bytes::from_static(b"A = trojan, password=S3CR3T-body"),
            path: PathBuf::from("resources/0a1b/data"),
            fetched_at: UNIX_EPOCH,
        };
        let shown = format!("{copy:?}");
        assert!(!shown.contains("S3CR3T"), "{shown}");
        for told in ["32 bytes", "0a1b"] {
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
        let connector = Arc::new(DirectConnector::new(Arc::new(NoNames)));
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
        let mut meta = Meta {
            url: url.to_string(),
            ..Meta::default()
        };
        cache::CacheDir::for_url(root.path(), url.as_str())
            .store(b"cached", &mut meta)
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
            .store(b"cached", &mut meta)
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
            .store(b"cached", &mut meta)
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

    /// A resource that failed and kept its copy is `available` again when a
    /// check succeeds, also when all the server has to send is the body the
    /// resource holds: no new version, nobody told, the failure gone from
    /// the status, and the next refresh an interval away again.
    #[tokio::test]
    async fn the_body_in_hand_brings_a_failed_resource_back() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/b", "bravo");
        server.set_validators("/b", false);
        let url = server.url("/b");
        let source = ResourceSource::Url(url.clone());
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url, None));
        let mut rx = h.subscribe();
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        rx.borrow_and_update();
        let succeeded = status_of(&mgr, &source).fetched_at.expect("a time");

        server.set_status("/b", 500);
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

        // the resource is backing off and tries again by itself
        server.set_status("/b", 200);
        let s = wait_for(&h, "available again", |s| available_version(s).is_some()).await;
        assert_eq!(available_version(&s), Some(1));
        assert_eq!(&s.data().unwrap().0[..], b"bravo");
        assert!(!rx.has_changed().unwrap(), "somebody was told");
        let status = status_of(&mgr, &source);
        assert_eq!(status.state, "available");
        assert_eq!(status.last_error, None);
        assert!(status.fetched_at > Some(succeeded), "{status:?}");
        // the backoff is over: the interval is a day, less at most a tenth
        let in_an_hour = SystemTime::now() + Duration::from_secs(3600);
        wait_until("the next refresh to be an interval away", || {
            status_of(&mgr, &source).next_refresh > Some(in_an_hour)
        })
        .await;
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

    /// The validators belong to the body they came with. A body that was
    /// damaged on disk is not that body: the entry is not read, the request
    /// carries no validators, and the server sends the whole body. With the
    /// validators its answer would be that nothing has changed, for as long
    /// as it keeps its version.
    #[tokio::test]
    async fn a_damaged_cache_is_fetched_whole() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/d", "delta, the whole of it");
        let url = server.url("/d");
        let source = ResourceSource::Url(url.clone());
        {
            let mgr = manager(root.path(), fast());
            let h = mgr.get(&url_spec(url.clone(), None));
            wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        }
        let file = cache_file(root.path(), &url);
        // what a power cut can leave of a body: its beginning
        std::fs::write(&file, "delta, the").unwrap();
        assert!(cached_copy(root.path(), &url).is_none());
        assert!(cached(root.path(), &source).is_none());

        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        let s = wait_for(&h, "the body", |s| available_version(s).is_some()).await;
        assert_eq!(&s.data().unwrap().0[..], b"delta, the whole of it");
        let reqs = server.requests();
        assert_eq!(reqs.len(), 2, "{reqs:?}");
        assert!(reqs[1].header("if-none-match").is_none(), "{reqs:?}");
        assert_eq!(std::fs::read(&file).unwrap(), b"delta, the whole of it");
        assert_eq!(
            &cached_copy(root.path(), &url).expect("a cache").data[..],
            b"delta, the whole of it"
        );
    }

    /// An answer that nothing has changed is about the body in hand. When the
    /// disk holds that body, intact, the metadata alone is written; when it
    /// does not, the body is written again with it.
    #[tokio::test]
    async fn an_unchanged_answer_writes_the_body_again_when_the_disk_does_not_hold_it() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/r", "romeo");
        let url = server.url("/r");
        let key = resource_key(&ResourceSource::Url(url.clone()));
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let dir = cache_of(root.path(), &url);
        let file = dir.path().join("data");
        let (_, written) = dir.load().expect("a cache");
        // one more request, with validators, that brings nothing new
        let unchanged = async |what: &str| {
            let asked = server.requests().len();
            assert_eq!(mgr.update_now(&key).await, Some(Ok(())), "{what}");
            let reqs = server.requests();
            assert_eq!(reqs.len(), asked + 1, "{what}");
            assert!(reqs[asked].header("if-none-match").is_some(), "{what}");
            assert_eq!(h.version(), 1, "{what}");
            let (data, meta) = dir.load().unwrap_or_else(|| panic!("no cache: {what}"));
            assert_eq!(&data[..], b"romeo", "{what}");
            assert_eq!(meta.sha256, written.sha256, "{what}");
            assert!(meta.fetched_time() > written.fetched_time(), "{what}");
        };

        // intact: the body is not written again, which its time shows
        let long_ago = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        unchanged("an intact cache").await;
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            long_ago
        );

        std::fs::write(&file, "rom").unwrap();
        assert!(dir.load().is_none());
        unchanged("a body cut short").await;

        std::fs::remove_file(&file).unwrap();
        unchanged("no body").await;

        let mut other = written.clone();
        other.sha256 = Some(format!("{:x}", Sha256::digest(b"another")));
        dir.store_meta(&other).unwrap();
        assert!(dir.load().is_none());
        unchanged("the metadata of another body").await;

        std::fs::remove_file(dir.path().join("meta.json")).unwrap();
        unchanged("no metadata").await;

        // a body that another writer put there whole, with its own metadata
        let mut theirs = Meta {
            url: url.to_string(),
            ..Meta::default()
        };
        dir.store(b"somebody else's", &mut theirs).unwrap();
        assert!(dir.load().is_some());
        unchanged("the whole entry of another writer").await;
    }

    /// A server that cannot say whether a body changed sends the whole body
    /// every time. The body in hand, sent again, is a check that found nothing
    /// new, and it is written as one: the metadata alone when the disk holds
    /// that body, intact, and the body with it when it does not.
    #[tokio::test]
    async fn a_body_sent_again_is_written_only_when_the_disk_does_not_hold_it() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/w", "whiskey");
        server.set_validators("/w", false);
        let url = server.url("/w");
        let key = resource_key(&ResourceSource::Url(url.clone()));
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let dir = cache_of(root.path(), &url);
        let file = dir.path().join("data");
        let (_, written) = dir.load().expect("a cache");
        assert_eq!((&written.etag, &written.last_modified), (&None, &None));
        // one more request, which has no validator to carry, and its answer
        // of the same body
        let again = async |what: &str| {
            let asked = server.requests().len();
            assert_eq!(mgr.update_now(&key).await, Some(Ok(())), "{what}");
            let reqs = server.requests();
            assert_eq!(reqs.len(), asked + 1, "{what}");
            assert!(reqs[asked].header("if-none-match").is_none(), "{what}");
            assert_eq!(h.version(), 1, "{what}");
            let (data, meta) = dir.load().unwrap_or_else(|| panic!("no cache: {what}"));
            assert_eq!(&data[..], b"whiskey", "{what}");
            assert_eq!(meta.sha256, written.sha256, "{what}");
            assert!(meta.fetched_time() > written.fetched_time(), "{what}");
        };

        // intact: the body is not written again, which its time shows
        let long_ago = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        again("an intact cache").await;
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            long_ago
        );

        std::fs::write(&file, "whis").unwrap();
        assert!(dir.load().is_none());
        again("a body cut short").await;
    }

    /// A cache from before the digest was kept is taken as it is, and gets
    /// its digest with the first answer that says the body has not changed.
    #[tokio::test]
    async fn a_cache_from_before_the_digest_gets_one_when_it_is_confirmed() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/l", "lima");
        let url = server.url("/l");
        let source = ResourceSource::Url(url.clone());
        let dir = cache_of(root.path(), &url);
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join("data"), "lima").unwrap();
        // checked long ago, so that it is due; the validator is the test
        // server's, which is the digest of the body in quotes
        let digest = format!("{:x}", Sha256::digest(b"lima"));
        std::fs::write(
            dir.path().join("meta.json"),
            format!(
                r#"{{"url":"{url}","etag":"\"{digest}\"","last_modified":null,"fetched_at":1700000000}}"#
            ),
        )
        .unwrap();
        assert_eq!(dir.load().expect("a cache").1.sha256, None);

        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        assert_eq!(available_version(&h.current()), Some(1));
        wait_until("the check to be recorded", || {
            server.hits("/l") == 1
                && status_of(&mgr, &source).state == "available"
                && dir.load().is_some_and(|(_, meta)| meta.sha256.is_some())
        })
        .await;
        let reqs = server.requests();
        assert!(reqs[0].header("if-none-match").is_some(), "{reqs:?}");
        assert_eq!(h.version(), 1);
        let (data, meta) = dir.load().expect("a cache");
        assert_eq!(&data[..], b"lima");
        assert_eq!(meta.sha256, Some(digest));
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

    /// The place a download of an address will have, known before there is
    /// one: it is where the copy is read from once it has been written.
    #[test]
    fn the_cache_file_of_an_address_is_where_its_copy_is_read_from() {
        let root = tempfile::tempdir().unwrap();
        let url = Url::parse("https://sets.test/placed.list").unwrap();
        let place = cache_file(root.path(), &url);
        assert_eq!(
            place,
            root.path()
                .join("resources")
                .join(resource_key(&ResourceSource::Url(url.clone())))
                .join("data")
        );
        assert!(cached_copy(root.path(), &url).is_none());
        seed_cache(root.path(), &url, b"placed", SystemTime::now());
        assert_eq!(cached_copy(root.path(), &url).unwrap().path, place);
        assert_eq!(std::fs::read(&place).unwrap(), b"placed");
        // another address, another place
        let other = Url::parse("https://sets.test/other.list").unwrap();
        assert_ne!(cache_file(root.path(), &other), place);
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

    // checks, awaited updates, adopt and retire

    use std::sync::Condvar;
    use std::sync::atomic::AtomicUsize;

    /// A check that takes a body unless it starts with `bad`, and remembers each body it was
    /// shown.
    #[derive(Default)]
    struct Picky {
        seen: Mutex<Vec<Vec<u8>>>,
    }

    impl Picky {
        fn calls(&self) -> usize {
            self.seen.lock().unwrap().len()
        }

        fn seen(&self) -> Vec<Vec<u8>> {
            self.seen.lock().unwrap().clone()
        }
    }

    fn picky_check(picky: &Arc<Picky>) -> Check {
        let picky = picky.clone();
        Arc::new(move |body: &[u8]| {
            picky.seen.lock().unwrap().push(body.to_vec());
            if body.starts_with(b"bad") {
                Err("not a profile".to_string())
            } else {
                Ok(())
            }
        })
    }

    /// What a thread of the blocking pool can wait on. It gives up after ten seconds, so that a
    /// test that has failed does not keep its runtime from shutting down.
    #[derive(Default)]
    struct Latch {
        open: Mutex<bool>,
        changed: Condvar,
    }

    impl Latch {
        fn open(&self) {
            *self.open.lock().unwrap() = true;
            self.changed.notify_all();
        }

        fn is_open(&self) -> bool {
            *self.open.lock().unwrap()
        }

        fn wait(&self) {
            let open = self.open.lock().unwrap();
            let _ = self
                .changed
                .wait_timeout_while(open, Duration::from_secs(10), |open| !*open)
                .unwrap();
        }
    }

    fn profile_spec(url: &Url) -> ResourceSpec {
        url_spec_of(ResourceKind::Profile, url.clone(), None)
    }

    fn source_of(url: &Url) -> ResourceSource {
        ResourceSource::Url(url.clone())
    }

    fn cache_of(root: &Path, url: &Url) -> cache::CacheDir {
        cache::CacheDir::for_url(root, url.as_str())
    }

    /// The two files of a cache, `None` for one that is not there. Both are text.
    fn cache_files(root: &Path, url: &Url) -> (Option<String>, Option<String>) {
        let dir = cache_of(root, url);
        let text = |name: &str| std::fs::read_to_string(dir.path().join(name)).ok();
        (text("data"), text("meta.json"))
    }

    /// A directory where the cache wants to write `<name>.tmp`: writing that fails on every
    /// platform, and the files that are there stay as they are. The directory of the cache has
    /// to exist.
    fn block_writes(root: &Path, url: &Url, name: &str) -> PathBuf {
        let blocked = cache_of(root, url).path().join(format!("{name}.tmp"));
        std::fs::create_dir(&blocked).unwrap();
        blocked
    }

    fn two_days_ago() -> SystemTime {
        SystemTime::now() - Duration::from_secs(2 * 86_400)
    }

    /// Waits until the task of the entry has recorded `attempts` attempts.
    async fn wait_finished(h: &ResourceHandle, attempts: u64) {
        wait_until("an attempt to be recorded", || {
            h.entry.progress.borrow().finished >= attempts
        })
        .await;
    }

    /// Waits until the task of the entry is over.
    async fn wait_ended(h: &ResourceHandle) {
        wait_until("the task to end", || h.entry.progress.borrow().ended).await;
    }

    /// Waits, for at most ten seconds, until a new version is published to the
    /// receiver.
    async fn told(rx: &mut watch::Receiver<u64>) {
        tokio::time::timeout(Duration::from_secs(10), rx.changed())
            .await
            .expect("nobody was told")
            .expect("the entry is gone");
    }

    /// `update_now`, for at most ten seconds. The call has no limit of its
    /// own: a caller that is never answered has to fail its test, not hang it.
    async fn updated(mgr: &ResourceManager, key: &str) -> Option<Result<(), String>> {
        tokio::time::timeout(Duration::from_secs(10), mgr.update_now(key))
            .await
            .expect("update_now did not return")
    }

    /// Waits until the task of the entry has had its turn and waits for a
    /// refresh that is more than an hour away: nothing was due.
    async fn not_due(h: &ResourceHandle) {
        wait_until("the task to wait for a refresh that is far off", || {
            let soon = SystemTime::now() + Duration::from_secs(3600);
            h.entry
                .next_refresh
                .lock()
                .unwrap()
                .is_some_and(|at| at > soon)
        })
        .await;
    }

    #[tokio::test]
    async fn a_body_that_passes_its_check_is_adopted() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "profile");
        let url = server.url("/p");
        let picky = Arc::new(Picky::default());
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&picky));
        let mut rx = h.subscribe();
        let s = wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        assert_eq!(&s.data().unwrap().0[..], b"profile");
        told(&mut rx).await;
        assert_eq!(*rx.borrow_and_update(), 1);
        let copy = cached_copy(root.path(), &url).expect("a cache");
        assert_eq!(&copy.data[..], b"profile");
        assert_eq!(picky.seen(), vec![b"profile".to_vec()]);
    }

    /// A body the check refuses is not a download that went well: the resource
    /// fails, nothing is cached or published, and the log says what failed.
    #[tokio::test]
    async fn a_first_download_that_fails_its_check_leaves_no_cache() {
        let lines = Lines::default();
        let _capturing = lines.capture();
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "bad: a login page");
        let url = server.url("/p");
        let source = source_of(&url);
        let picky = Arc::new(Picky::default());
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&picky));
        let rx = h.subscribe();
        wait_for(&h, "failed", |s| {
            matches!(s, ResourceState::Failed { cached: None, .. })
        })
        .await;
        let status = status_of(&mgr, &source);
        assert_eq!(
            (status.state, status.version, status.ready),
            ("failed", 0, false)
        );
        assert_eq!(status.last_error.as_deref(), Some("not a profile"));
        assert!(
            !cache_of(root.path(), &url).path().exists(),
            "a cache was made"
        );
        assert!(!rx.has_changed().unwrap(), "somebody was told");
        let said = lines.said("downloaded resource not adopted");
        assert!(
            !said.is_empty() && said.iter().all(|line| line.contains("not a profile")),
            "{said:?}"
        );
        assert_eq!(lines.said("resource fetch failed"), Vec::<String>::new());

        // it backs off and asks again, and the check sees the body again
        wait_until("the check to be asked again", || picky.calls() >= 2).await;
        server.set("/p", "profile");
        let s = wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        assert_eq!(&s.data().unwrap().0[..], b"profile");
        assert_eq!(
            &cached_copy(root.path(), &url).expect("a cache").data[..],
            b"profile"
        );
        assert!(rx.has_changed().unwrap());
        assert_eq!(status_of(&mgr, &source).last_error, None);
    }

    /// What the entry had stays as it was, down to the bytes of the two files.
    #[tokio::test]
    async fn a_new_body_that_fails_its_check_leaves_the_cache_as_it_was() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "bad: a login page");
        let url = server.url("/p");
        let source = source_of(&url);
        seed_cache(root.path(), &url, b"held", two_days_ago());
        let before = cache_files(root.path(), &url);
        let picky = Arc::new(Picky::default());
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&picky));
        let rx = h.subscribe();
        let ready = status_of(&mgr, &source).ready;
        assert!(ready);
        wait_for(&h, "failed with the copy", |s| {
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
        assert_eq!(
            (status.state, status.version, status.ready),
            ("failed", 1, ready)
        );
        assert_eq!(status.last_error.as_deref(), Some("not a profile"));
        assert_eq!(cache_files(root.path(), &url), before);
        assert_eq!(&h.current().data().unwrap().0[..], b"held");
        assert!(!rx.has_changed().unwrap(), "somebody was told");

        wait_until("the check to be asked again", || picky.calls() >= 2).await;
        server.set("/p", "profile");
        let s = wait_for(&h, "v2", |s| available_version(s) == Some(2)).await;
        assert_eq!(&s.data().unwrap().0[..], b"profile");
        assert_eq!(
            &cached_copy(root.path(), &url).expect("a cache").data[..],
            b"profile"
        );
        assert!(rx.has_changed().unwrap());
    }

    /// The request that follows a refusal is a request for the next version of
    /// what the entry holds: a "not modified" would leave it failed with
    /// nothing more to try.
    #[tokio::test]
    async fn the_request_after_a_failed_check_carries_the_validators_of_the_body_still_held() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "held");
        let url = server.url("/p");
        let picky = Arc::new(Picky::default());
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&picky));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let etag = cache_of(root.path(), &url)
            .load()
            .unwrap()
            .1
            .etag
            .expect("an etag");
        server.set("/p", "bad: new");
        assert!(mgr.force_update(&source_of(&url)));
        wait_until("two refusals", || picky.calls() >= 3).await;
        let requests = server.requests();
        assert!(requests[0].header("if-none-match").is_none());
        for request in &requests[1..] {
            assert_eq!(request.header("if-none-match"), Some(etag.as_str()));
        }
        assert_eq!(h.version(), 1);
    }

    #[tokio::test]
    async fn a_check_is_not_asked_about_a_not_modified() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "profile");
        let url = server.url("/p");
        let source = source_of(&url);
        let picky = Arc::new(Picky::default());
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&picky));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let first = status_of(&mgr, &source).fetched_at.expect("a time");
        assert_eq!(picky.calls(), 1);

        assert!(mgr.force_update(&source));
        wait_until("the check to be recorded", || {
            status_of(&mgr, &source).fetched_at > Some(first)
        })
        .await;
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].header("if-none-match").is_some());
        assert_eq!(picky.calls(), 1);
        assert_eq!(h.version(), 1);
    }

    #[tokio::test]
    async fn a_check_is_not_asked_about_a_body_the_entry_holds() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "profile");
        server.set_validators("/p", false);
        let url = server.url("/p");
        let source = source_of(&url);
        let picky = Arc::new(Picky::default());
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&picky));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let first = status_of(&mgr, &source).fetched_at.expect("a time");
        assert_eq!(picky.calls(), 1);

        assert!(mgr.force_update(&source));
        wait_until("the check to be recorded", || {
            status_of(&mgr, &source).fetched_at > Some(first)
        })
        .await;
        assert_eq!(server.hits("/p"), 2, "the server sent the whole body again");
        assert_eq!(picky.calls(), 1);
        assert_eq!(h.version(), 1);
    }

    /// A copy the cache holds went in after a check, or after somebody took
    /// the responsibility for it.
    #[tokio::test]
    async fn a_check_is_not_asked_about_what_the_cache_holds() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let url = server.url("/p");
        seed_cache(root.path(), &url, b"held", SystemTime::now());
        let picky = Arc::new(Picky::default());
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&picky));
        assert_eq!(available_version(&h.current()), Some(1));
        tokio::time::timeout(
            Duration::ZERO,
            mgr.attempted(&resource_key(&source_of(&url))),
        )
        .await
        .expect("nothing is due, so it returns at once");
        not_due(&h).await;
        assert_eq!(picky.calls(), 0);
        assert_eq!(server.hits("/p"), 0);
    }

    /// The check of a profile loads it, which takes as long as it takes: the
    /// other tasks of the runtime go on meanwhile, though this one has the one
    /// thread there is. The check waits for what only another task of that
    /// runtime does.
    #[tokio::test]
    async fn a_check_does_not_hold_the_thread_of_the_runtime() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "profile");
        let (entered, opened) = (Arc::new(Latch::default()), Arc::new(Latch::default()));
        let saw_it_open = Arc::new(AtomicBool::new(false));
        let check: Check = {
            let (entered, opened) = (entered.clone(), opened.clone());
            let saw_it_open = saw_it_open.clone();
            Arc::new(move |_: &[u8]| {
                entered.open();
                // gives up after ten seconds, which is how it ends when the thread that is
                // to open it is the one this waits on
                opened.wait();
                saw_it_open.store(opened.is_open(), Ordering::SeqCst);
                Ok(())
            })
        };
        // a task of the same runtime: it gets its turns only while the check leaves the
        // thread alone, and opens the latch only once the check has begun
        tokio::spawn({
            let (entered, opened) = (entered.clone(), opened.clone());
            async move {
                while !entered.is_open() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                opened.open();
            }
        });
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&server.url("/p")), "the profile", check);
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        assert!(
            saw_it_open.load(Ordering::SeqCst),
            "the check held the thread of the runtime"
        );
    }

    /// The check of a profile registers the sources the profile mentions, and
    /// adopts what it has already got for them. It runs on a thread of the
    /// blocking pool and under none of the manager's locks.
    #[tokio::test]
    async fn a_check_may_call_the_manager() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "profile");
        server.set("/other", "other");
        server.set("/known", "known");
        let (own, other, known) = (server.url("/p"), server.url("/other"), server.url("/known"));
        let mgr = manager(root.path(), fast());
        let known_handle = mgr.get(&url_spec(known.clone(), None));
        wait_for(&known_handle, "known", |s| available_version(s) == Some(1)).await;
        let check: Check = {
            // not the manager itself: it holds the entry, which holds the check
            let weak = Arc::downgrade(&mgr);
            let (own, other, known) = (source_of(&own), other.clone(), source_of(&known));
            Arc::new(move |body: &[u8]| {
                let mgr = weak.upgrade().expect("the manager");
                // the runtime's context reaches a thread of the blocking pool, so the
                // task of the new entry can be spawned
                mgr.get(&url_spec(other.clone(), None));
                mgr.adopt(&known, Bytes::from_static(b"adopted"))
                    .map_err(|e| e.to_string())?;
                mgr.adopt(&own, Bytes::copy_from_slice(body))
                    .map_err(|e| e.to_string())
            })
        };
        let h = mgr.get_checked(&profile_spec(&own), "the profile", check);
        wait_finished(&h, 1).await;
        // the source it registered has a task, and the task downloaded
        let other_handle = mgr.get(&url_spec(other, None));
        let s = wait_for(&other_handle, "other", |s| available_version(s) == Some(1)).await;
        assert_eq!(&s.data().unwrap().0[..], b"other");
        assert_eq!(server.hits("/other"), 1);
        // the one it adopted into holds what it was given
        let s = known_handle.current();
        assert_eq!(available_version(&s), Some(2));
        assert_eq!(&s.data().unwrap().0[..], b"adopted");
        // and the profile that was adopted by its own check and then downloaded is one version
        assert_eq!(h.version(), 1);
        assert_eq!(status_of(&mgr, &source_of(&own)).state, "available");
    }

    /// What the check says about itself is not what the log or the status
    /// carries; the task goes on.
    #[tokio::test]
    async fn a_check_that_panics_fails_the_attempt_and_not_the_task() {
        let lines = Lines::default();
        let _capturing = lines.capture();
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "profile");
        let calls = Arc::new(AtomicUsize::new(0));
        let check: Check = {
            let calls = calls.clone();
            Arc::new(move |_: &[u8]| {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("the check broke: S3CR3T");
                }
                Ok(())
            })
        };
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&server.url("/p")), "the profile", check);
        let s = wait_for(&h, "failed", |s| matches!(s, ResourceState::Failed { .. })).await;
        assert!(
            matches!(&s, ResourceState::Failed { last_error, .. }
                if last_error == "the check of the downloaded copy did not finish"),
            "{s:?}"
        );
        let said = lines.said("downloaded resource not adopted");
        assert!(!said.is_empty() && said.iter().all(|line| !line.contains("S3CR3T")));
        // the next attempt, whose check passes, adopts
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        assert!(calls.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn the_first_check_an_entry_is_given_stays() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "profile");
        let url = server.url("/p");
        let (first, second) = (Arc::new(Picky::default()), Arc::new(Picky::default()));
        let mgr = manager(root.path(), fast());
        // registered without one, it takes the first one it is given later
        mgr.get(&profile_spec(&url));
        let h = mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&first));
        mgr.get_checked(&profile_spec(&url), "another", picky_check(&second));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        assert_eq!((first.calls(), second.calls()), (1, 0));
        assert_eq!(
            status_of(&mgr, &source_of(&url)).label.as_deref(),
            Some("the profile")
        );

        // a file is registered as a labelled one is, and nothing of it is downloaded
        let file = root.path().join("local.list");
        std::fs::write(&file, "one").unwrap();
        let spec = ResourceSpec {
            source: ResourceSource::File(file.clone()),
            update_interval: None,
            kind: ResourceKind::RuleSet,
        };
        let local = mgr.get_checked(&spec, "a file", picky_check(&first));
        assert_eq!(available_version(&local.current()), Some(1));
        std::fs::write(&file, "two").unwrap();
        wait_for(&local, "v2", |s| available_version(s) == Some(2)).await;
        assert_eq!(first.calls(), 1);
        let status = status_of(&mgr, &ResourceSource::File(file));
        assert!(status.local);
        assert_eq!(status.label.as_deref(), Some("a file"));
    }

    /// A write that fails leaves the entry ahead of its disk. The next check
    /// that succeeds brings the disk up to date, the body included: a restart
    /// would otherwise find the old body beside the metadata of the new one,
    /// which is no entry.
    #[tokio::test]
    async fn the_disk_catches_up_after_a_write_that_failed() {
        let lines = Lines::default();
        let _capturing = lines.capture();
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/c", "B");
        let url = server.url("/c");
        let source = source_of(&url);
        seed_cache(root.path(), &url, b"A", two_days_ago());
        let blocked = block_writes(root.path(), &url, "data");
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        let s = wait_for(&h, "B", |s| available_version(s) == Some(2)).await;
        assert_eq!(&s.data().unwrap().0[..], b"B");
        assert_eq!(&cached_copy(root.path(), &url).unwrap().data[..], b"A");
        assert_eq!(lines.said("cannot write resource cache").len(), 1);

        // the way is clear again; the server says nothing changed
        std::fs::remove_dir(&blocked).unwrap();
        let before = status_of(&mgr, &source).fetched_at;
        assert!(mgr.force_update(&source));
        wait_until("the check to be recorded", || {
            status_of(&mgr, &source).fetched_at > before
        })
        .await;
        assert!(server.requests()[1].header("if-none-match").is_some());
        let copy = cached_copy(root.path(), &url).unwrap();
        assert_eq!(&copy.data[..], b"B");
        assert_eq!(Some(copy.fetched_at), status_of(&mgr, &source).fetched_at);
        assert_eq!(h.version(), 2);
        assert_eq!(lines.said("cannot write resource cache").len(), 1);
    }

    /// The same for the metadata alone: the time on disk is the one in memory
    /// again after the next check.
    #[tokio::test]
    async fn the_time_on_disk_catches_up_after_a_write_of_the_metadata_that_failed() {
        let lines = Lines::default();
        let _capturing = lines.capture();
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/m", "A");
        let url = server.url("/m");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let first = status_of(&mgr, &source).fetched_at.expect("a time");
        let blocked = block_writes(root.path(), &url, "meta");
        assert!(mgr.force_update(&source));
        wait_until("the check to be recorded", || {
            status_of(&mgr, &source).fetched_at > Some(first)
        })
        .await;
        let status = status_of(&mgr, &source);
        assert_eq!(status.state, "available");
        assert_eq!(lines.said("cannot write resource cache").len(), 1);
        assert_eq!(
            cached_copy(root.path(), &url).unwrap().fetched_at,
            first,
            "the disk has the time of the first download"
        );

        std::fs::remove_dir(&blocked).unwrap();
        let second = status.fetched_at;
        assert!(mgr.force_update(&source));
        wait_until("the check to be recorded", || {
            status_of(&mgr, &source).fetched_at > second
        })
        .await;
        let copy = cached_copy(root.path(), &url).unwrap();
        assert_eq!(Some(copy.fetched_at), status_of(&mgr, &source).fetched_at);
        assert_eq!(&copy.data[..], b"A");
        assert_eq!(h.version(), 1);
        assert_eq!(lines.said("cannot write resource cache").len(), 1);
    }

    /// The readers of an entry with a check go by the disk: what the disk
    /// cannot take, the entry does not hold either.
    #[tokio::test]
    async fn a_checked_entry_does_not_adopt_what_it_cannot_write() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/c", "B");
        let url = server.url("/c");
        let source = source_of(&url);
        let key = resource_key(&source);
        seed_cache(root.path(), &url, b"A", two_days_ago());
        let blocked = block_writes(root.path(), &url, "data");
        let picky = Arc::new(Picky::default());
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&picky));
        let rx = h.subscribe();
        wait_for(&h, "failed with the copy", |s| {
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
        assert_eq!((status.state, status.version), ("failed", 1));
        assert!(
            status
                .last_error
                .as_deref()
                .is_some_and(|error| error.starts_with("cannot write the cache: ")),
            "{status:?}"
        );
        assert_eq!(&h.current().data().unwrap().0[..], b"A");
        assert_eq!(&cached_copy(root.path(), &url).unwrap().data[..], b"A");
        assert!(!rx.has_changed().unwrap(), "somebody was told");
        let told = updated(&mgr, &key)
            .await
            .expect("an entry")
            .expect_err("the same failure");
        assert!(told.starts_with("cannot write the cache: "), "{told}");

        // the way is clear again: the next attempt adopts
        std::fs::remove_dir(&blocked).unwrap();
        assert_eq!(updated(&mgr, &key).await, Some(Ok(())));
        assert_eq!(h.version(), 2);
        assert_eq!(&h.current().data().unwrap().0[..], b"B");
        assert_eq!(&cached_copy(root.path(), &url).unwrap().data[..], b"B");
        assert!(rx.has_changed().unwrap());
    }

    /// The same for a "not modified": the time of the check goes into the
    /// cache too, and an entry whose readers go by the disk is not told it was
    /// checked when the disk does not say so.
    #[tokio::test]
    async fn a_checked_entry_that_cannot_write_a_not_modified_is_failed() {
        let lines = Lines::default();
        let _capturing = lines.capture();
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/c", "A");
        let url = server.url("/c");
        let source = source_of(&url);
        let key = resource_key(&source);
        let picky = Arc::new(Picky::default());
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&picky));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let first = status_of(&mgr, &source).fetched_at.expect("a time");
        let blocked = block_writes(root.path(), &url, "meta");
        let told = updated(&mgr, &key)
            .await
            .expect("an entry")
            .expect_err("the check cannot be written down");
        assert!(told.starts_with("cannot write the cache: "), "{told}");
        let status = status_of(&mgr, &source);
        assert_eq!((status.state, status.version), ("failed", 1));
        assert_eq!(status.last_error, Some(told));
        assert_eq!(status.fetched_at, Some(first), "the check was not recorded");
        assert_eq!(cached_copy(root.path(), &url).unwrap().fetched_at, first);
        assert!(!lines.said("cannot write resource cache").is_empty());

        std::fs::remove_dir(&blocked).unwrap();
        assert_eq!(updated(&mgr, &key).await, Some(Ok(())));
        let status = status_of(&mgr, &source);
        assert_eq!((status.state, status.version), ("available", 1));
        assert!(status.fetched_at > Some(first));
        let copy = cached_copy(root.path(), &url).unwrap();
        assert_eq!(Some(copy.fetched_at), status.fetched_at);
        assert_eq!(picky.calls(), 1);
    }

    /// The cache writes the body, then the metadata, and a write that fails
    /// may have got as far as the body. The entry has not taken that body,
    /// and beside the metadata of the one before it is no entry: whoever
    /// reads the disk finds none, and a restart would ask for the whole body.
    /// The next check that succeeds puts back the body the entry holds.
    #[tokio::test]
    async fn the_disk_is_put_right_after_a_write_that_got_as_far_as_the_body() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/c", "A");
        let url = server.url("/c");
        let source = source_of(&url);
        let key = resource_key(&source);
        let picky = Arc::new(Picky::default());
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&picky));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let blocked = block_writes(root.path(), &url, "meta");
        server.set("/c", "B");
        let told = updated(&mgr, &key)
            .await
            .expect("an entry")
            .expect_err("the metadata cannot be written");
        assert!(told.starts_with("cannot write the cache: "), "{told}");
        assert_eq!(&h.current().data().unwrap().0[..], b"A");
        let file = cache_file(root.path(), &url);
        assert_eq!(
            std::fs::read(&file).unwrap(),
            b"B",
            "the body got to the disk"
        );
        assert!(cached_copy(root.path(), &url).is_none());

        // The server is back at what the entry holds. An attempt that begins
        // after this is awaited before the way is cleared, so that none that
        // brought the other body is still to be recorded.
        server.set("/c", "A");
        updated(&mgr, &key)
            .await
            .expect("an entry")
            .expect_err("the cache still cannot be written");
        std::fs::remove_dir(&blocked).unwrap();
        assert_eq!(updated(&mgr, &key).await, Some(Ok(())));
        assert_eq!(h.version(), 1);
        assert_eq!(&h.current().data().unwrap().0[..], b"A");
        assert_eq!(&cached_copy(root.path(), &url).unwrap().data[..], b"A");
        assert_eq!(status_of(&mgr, &source).state, "available");
    }

    /// The same when it is an adoption that got as far as the body.
    #[tokio::test]
    async fn the_disk_is_put_right_after_an_adoption_that_got_as_far_as_the_body() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/c", "A");
        let url = server.url("/c");
        let source = source_of(&url);
        let key = resource_key(&source);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let blocked = block_writes(root.path(), &url, "meta");
        mgr.adopt(&source, Bytes::from_static(b"X"))
            .expect_err("the metadata cannot be written");
        assert_eq!(&h.current().data().unwrap().0[..], b"A");
        let file = cache_file(root.path(), &url);
        assert_eq!(
            std::fs::read(&file).unwrap(),
            b"X",
            "the body got to the disk"
        );
        assert!(cached_copy(root.path(), &url).is_none());

        std::fs::remove_dir(&blocked).unwrap();
        assert_eq!(updated(&mgr, &key).await, Some(Ok(())));
        assert_eq!(h.version(), 1);
        assert_eq!(&h.current().data().unwrap().0[..], b"A");
        assert_eq!(&cached_copy(root.path(), &url).unwrap().data[..], b"A");
    }

    /// A write to the cache can take as long as the disk takes, and the lock
    /// that is held across it is nobody's but the files': what only looks at
    /// the entries, or registers one, does not wait for it.
    #[tokio::test]
    async fn the_entries_can_be_looked_at_while_the_cache_is_written() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/a", "one");
        let (url, other) = (server.url("/a"), server.url("/b"));
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let writing = h.entry.record.lock().unwrap();
        // from a thread of its own, so that a wait would not stop the test
        let (done, finished) = std::sync::mpsc::channel();
        let looker = std::thread::spawn({
            let mgr = mgr.clone();
            move || {
                let listed = mgr.statuses().len();
                let asked = mgr.force_update(&source_of(&url));
                mgr.get(&url_spec(url, Some(60)));
                mgr.get(&url_spec(other, None));
                let _ = done.send((listed, asked, mgr.statuses().len()));
            }
        });
        let seen = finished
            .recv_timeout(Duration::from_secs(5))
            .expect("it waited for the write");
        assert_eq!(seen, (1, true, 2));
        drop(writing);
        looker.join().unwrap();
    }

    /// All that touches an entry at once, from several threads: nothing
    /// deadlocks, the disk and the entry agree on what the resource is when it
    /// is over, and a retirement in the middle of it all ends the waiting.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn asking_adopting_and_listing_at_once_leaves_the_disk_and_the_entry_agreeing() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/s", "served");
        let url = server.url("/s");
        let source = source_of(&url);
        let key = resource_key(&source);
        let picky = Arc::new(Picky::default());
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&picky));
        let mut jobs = tokio::task::JoinSet::new();
        for _ in 0..3 {
            let (mgr, key) = (mgr.clone(), key.clone());
            jobs.spawn(async move {
                for _ in 0..5 {
                    updated(&mgr, &key).await;
                }
            });
        }
        for n in 0..2 {
            let (mgr, source) = (mgr.clone(), source.clone());
            jobs.spawn_blocking(move || {
                for i in 0..20 {
                    mgr.adopt(&source, Bytes::from(format!("adopted {n} {i}")))
                        .unwrap();
                }
            });
        }
        {
            let (mgr, source) = (mgr.clone(), source.clone());
            jobs.spawn(async move {
                for _ in 0..100 {
                    mgr.statuses();
                    mgr.force_update(&source);
                    tokio::task::yield_now().await;
                }
            });
        }
        let all = async {
            // a job that gave up waiting has to fail the test, not just end
            while let Some(done) = jobs.join_next().await {
                done.expect("a job failed");
            }
        };
        tokio::time::timeout(Duration::from_secs(30), all)
            .await
            .expect("a deadlock");
        wait_until("the last attempt to be over", || {
            let p = h.entry.progress.borrow();
            p.finished == p.started && p.wanted <= p.started
        })
        .await;
        let held = h.current().data().expect("a copy");
        let copy = cached_copy(root.path(), &url).expect("a cache");
        assert_eq!(&copy.data[..], &held.0[..]);
        assert_eq!(Some(copy.fetched_at), status_of(&mgr, &source).fetched_at);

        // a retirement while others wait for an attempt ends their waiting
        server.set_delay("/s", Duration::from_millis(200));
        let waiting: Vec<_> = (0..3)
            .map(|_| {
                let (mgr, key) = (mgr.clone(), key.clone());
                tokio::spawn(async move { updated(&mgr, &key).await })
            })
            .collect();
        assert!(mgr.retire(&source));
        for caller in waiting {
            // answered by an attempt that had begun, by the end of the entry, or
            // finding it gone; it must not go on waiting for an attempt that will never be
            tokio::time::timeout(Duration::from_secs(10), caller)
                .await
                .expect("it is still waiting")
                .unwrap();
        }
        wait_ended(&h).await;
    }

    #[tokio::test]
    async fn update_now_waits_for_the_attempt_and_says_how_it_went() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/u", "one");
        let url = server.url("/u");
        let source = source_of(&url);
        let key = resource_key(&source);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url, None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;

        // a new version is in, and the download is over, when it returns
        server.set("/u", "two");
        assert_eq!(updated(&mgr, &key).await, Some(Ok(())));
        assert_eq!(h.version(), 2);
        assert_eq!(&h.current().data().unwrap().0[..], b"two");
        assert!(!status_of(&mgr, &source).updating);
        // a "not modified"
        assert_eq!(updated(&mgr, &key).await, Some(Ok(())));
        assert_eq!(h.version(), 2);
        // the body in hand
        server.set_validators("/u", false);
        assert_eq!(updated(&mgr, &key).await, Some(Ok(())));
        assert_eq!(h.version(), 2);
        // a failure, as the status tells it
        server.set_status("/u", 500);
        let told = updated(&mgr, &key)
            .await
            .expect("an entry")
            .expect_err("a failure");
        let status = status_of(&mgr, &source);
        assert_eq!(status.state, "failed");
        assert_eq!(status.last_error, Some(told));
    }

    #[tokio::test]
    async fn update_now_gives_the_reason_a_check_gave() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "bad: a login page");
        let url = server.url("/p");
        let source = source_of(&url);
        let picky = Arc::new(Picky::default());
        let mgr = manager(root.path(), fast());
        mgr.get_checked(&profile_spec(&url), "the profile", picky_check(&picky));
        assert_eq!(
            updated(&mgr, &resource_key(&source)).await,
            Some(Err("not a profile".to_string()))
        );
        assert_eq!(
            status_of(&mgr, &source).last_error.as_deref(),
            Some("not a profile")
        );
    }

    /// A call while an attempt is under way waits for the attempt after it:
    /// the one in flight began with the validators of before the call.
    #[tokio::test]
    async fn update_now_waits_for_an_attempt_that_begins_after_the_call() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/w", "one");
        let url = server.url("/w");
        let source = source_of(&url);
        let key = resource_key(&source);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url, None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        // the next request is held back, so that it is under way for a while
        server.set_delay("/w", Duration::from_millis(400));
        assert!(mgr.force_update(&source));
        wait_until("the request to arrive", || server.hits("/w") == 2).await;
        // it has the route as it was; the one after it finds the new body
        server.set("/w", "two");
        assert_eq!(updated(&mgr, &key).await, Some(Ok(())));
        assert_eq!(
            server.hits("/w"),
            3,
            "it waited for a request made after it"
        );
        assert_eq!(&h.current().data().unwrap().0[..], b"two");
        assert_eq!(h.version(), 2);
    }

    /// Asking for an update several times before the task has begun one makes
    /// one. Three calls: counting the calls instead would make two.
    #[tokio::test]
    async fn force_updates_during_an_attempt_make_one_more_request() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/f", "one");
        server.set_delay("/f", Duration::from_millis(300));
        let url = server.url("/f");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url, None));
        wait_until("the first request", || server.hits("/f") == 1).await;
        assert!(mgr.force_update(&source));
        assert!(mgr.force_update(&source));
        assert!(mgr.force_update(&source));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        wait_until("the second request", || server.hits("/f") == 2).await;
        wait_until("the second attempt to be over", || {
            !status_of(&mgr, &source).updating
        })
        .await;
        // that nothing more is asked can only be seen by waiting: a third request would follow
        // the second at once, and the wait is as long as an attempt takes
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(server.hits("/f"), 2);
    }

    #[tokio::test]
    async fn two_update_nows_during_an_attempt_are_answered_by_one_more_request() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/g", "one");
        server.set_delay("/g", Duration::from_millis(300));
        let url = server.url("/g");
        let source = source_of(&url);
        let key = resource_key(&source);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url, None));
        wait_until("the first request", || server.hits("/g") == 1).await;
        let (a, b) = tokio::join!(updated(&mgr, &key), updated(&mgr, &key));
        assert_eq!((a, b), (Some(Ok(())), Some(Ok(()))));
        assert_eq!(server.hits("/g"), 2);
        assert_eq!(h.version(), 1);
        // that nothing more is asked can only be seen by waiting: a third request would follow
        // the second at once, and the wait is as long as an attempt takes
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(server.hits("/g"), 2);
    }

    /// A call during the backoff does not wait it out, and with auto-refresh
    /// off nothing but a call makes an attempt.
    #[tokio::test]
    async fn update_now_does_not_wait_for_a_backoff_or_a_timer() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/b", "one");
        server.set_status("/b", 500);
        let url = server.url("/b");
        let source = source_of(&url);
        let key = resource_key(&source);
        // the backoff is a minute
        let opts = ResourceOptions {
            min_backoff: Duration::from_secs(60),
            max_backoff: Duration::from_secs(60),
            ..fast()
        };
        let mgr = manager(root.path(), opts);
        let h = mgr.get(&url_spec(url, Some(-1)));
        wait_for(&h, "failed", |s| matches!(s, ResourceState::Failed { .. })).await;
        assert_eq!(server.hits("/b"), 1);
        server.set_status("/b", 200);
        let started = std::time::Instant::now();
        assert_eq!(updated(&mgr, &key).await, Some(Ok(())));
        assert!(started.elapsed() < Duration::from_secs(30));
        assert_eq!(h.version(), 1);
        // the backoff is over; with auto-refresh off nothing is due but a call
        server.set("/b", "two");
        assert_eq!(updated(&mgr, &key).await, Some(Ok(())));
        assert_eq!(h.version(), 2);
    }

    #[tokio::test]
    async fn update_now_of_a_key_nobody_registered_is_none() {
        let root = tempfile::tempdir().unwrap();
        let mgr = manager(root.path(), fast());
        let unknown = resource_key(&ResourceSource::Url(
            Url::parse("https://lists.test/nothing").unwrap(),
        ));
        assert_eq!(updated(&mgr, &unknown).await, None);
        tokio::time::timeout(Duration::ZERO, mgr.attempted(&unknown))
            .await
            .expect("there is nothing to wait for");
    }

    #[tokio::test]
    async fn update_now_of_a_local_file_returns_at_once_and_the_file_is_read_again() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("local.list");
        std::fs::write(&file, "one").unwrap();
        let mgr = manager(root.path(), fast());
        let source = ResourceSource::File(file.clone());
        let h = mgr.get(&ResourceSpec {
            source: source.clone(),
            update_interval: None,
            kind: ResourceKind::RuleSet,
        });
        std::fs::write(&file, "two").unwrap();
        let key = resource_key(&source);
        tokio::time::timeout(Duration::from_secs(5), mgr.update_now(&key))
            .await
            .expect("it returns")
            .expect("an entry")
            .expect("nothing to fail");
        wait_for(&h, "v2", |s| available_version(s) == Some(2)).await;
        tokio::time::timeout(Duration::ZERO, mgr.attempted(&key))
            .await
            .expect("a file has no attempts to wait for");
    }

    #[tokio::test]
    async fn update_now_of_an_offline_manager_says_so() {
        let root = tempfile::tempdir().unwrap();
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(root.path(), offline);
        let source = ResourceSource::Url(Url::parse("https://lists.test/a.list").unwrap());
        mgr.get(&ResourceSpec {
            source: source.clone(),
            update_interval: None,
            kind: ResourceKind::Profile,
        });
        let key = resource_key(&source);
        assert_eq!(
            updated(&mgr, &key).await,
            Some(Err("offline mode: nothing is downloaded".to_string()))
        );
        tokio::time::timeout(Duration::ZERO, mgr.attempted(&key))
            .await
            .expect("an offline manager makes no attempts to wait for");
    }

    /// The call is over when the entry is retired, with the reason.
    #[tokio::test]
    async fn update_now_is_answered_when_the_entry_is_retired_meanwhile() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/x", "one");
        let url = server.url("/x");
        let source = source_of(&url);
        let key = resource_key(&source);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url, None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        // an attempt that takes three seconds, which is far longer than the wait below
        server.set_delay("/x", Duration::from_secs(3));
        assert!(mgr.force_update(&source));
        wait_until("the request to arrive", || server.hits("/x") == 2).await;
        let waiting = tokio::spawn({
            let (mgr, key) = (mgr.clone(), key.clone());
            async move { updated(&mgr, &key).await }
        });
        wait_until("the call to ask", || h.entry.progress.borrow().wanted == 3).await;
        assert!(mgr.retire(&source));
        let told = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .expect("it does not wait for the attempts")
            .unwrap();
        assert_eq!(
            told,
            Some(Err("the resource is no longer updated".to_string()))
        );
    }

    /// An entry that never got a task (there was no runtime to run it on) is
    /// over from the start, and nothing waits for it.
    #[tokio::test]
    async fn an_entry_without_a_task_is_over_from_the_start() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/n", "none");
        let url = server.url("/n");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        // a thread that has no runtime
        let registered = std::thread::spawn({
            let (mgr, url) = (mgr.clone(), url.clone());
            move || mgr.get(&url_spec(url, None))
        })
        .join()
        .unwrap();
        let key = resource_key(&source);
        let bound = Duration::from_secs(5);
        let told = tokio::time::timeout(bound, mgr.update_now(&key))
            .await
            .expect("it returns");
        assert_eq!(
            told,
            Some(Err("the resource is no longer updated".to_string()))
        );
        tokio::time::timeout(bound, mgr.attempted(&key))
            .await
            .expect("it returns");
        assert_eq!(server.hits("/n"), 0);
        assert!(registered.entry.progress.borrow().ended);
    }

    /// The task says it is over when the manager goes.
    #[tokio::test]
    async fn a_task_says_it_is_over_when_its_manager_goes() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/e", "end");
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(server.url("/e"), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        assert!(!h.entry.progress.borrow().ended);
        drop(mgr);
        wait_ended(&h).await;
    }

    /// A task that is thrown away before it ever ran is over too: nobody is
    /// left to make an attempt, and whoever waits for one has to be told.
    #[test]
    fn a_task_that_never_ran_says_it_is_over() {
        let url = Url::parse("https://lists.test/a.list").unwrap();
        let entry = Arc::new(Entry::new(&url_spec(url, None), None, None, None));
        let gone = watch::Sender::new(());
        let task = url_task(
            Ends(entry.clone()),
            Weak::new(),
            entry.clone(),
            gone.subscribe(),
            entry.progress.subscribe(),
        );
        assert!(!entry.progress.borrow().ended);
        drop(task);
        assert!(entry.progress.borrow().ended);
    }

    #[tokio::test]
    async fn attempted_returns_when_the_first_attempt_has_failed() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/a", "x");
        server.set_status("/a", 500);
        let url = server.url("/a");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        mgr.get(&url_spec(url, None));
        tokio::time::timeout(
            Duration::from_secs(10),
            mgr.attempted(&resource_key(&source)),
        )
        .await
        .expect("it returns");
        assert_eq!(status_of(&mgr, &source).state, "failed");
    }

    #[tokio::test]
    async fn attempted_waits_for_the_refresh_of_a_copy_that_is_due() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/s", "held");
        server.set_delay("/s", Duration::from_millis(300));
        let url = server.url("/s");
        let source = source_of(&url);
        let key = resource_key(&source);
        let long_ago = two_days_ago();
        seed_cache(root.path(), &url, b"held", long_ago);
        let mgr = manager(root.path(), fast());
        mgr.get(&url_spec(url, None));
        wait_until("the request to arrive", || server.hits("/s") == 1).await;
        assert!(
            tokio::time::timeout(Duration::ZERO, mgr.attempted(&key))
                .await
                .is_err(),
            "it returned while the refresh was under way"
        );
        tokio::time::timeout(Duration::from_secs(5), mgr.attempted(&key))
            .await
            .expect("it returns when the refresh is over");
        let status = status_of(&mgr, &source);
        assert!(
            status.fetched_at > Some(long_ago + Duration::from_secs(86_400)),
            "{status:?}"
        );
        assert!(!status.updating);
    }

    #[tokio::test]
    async fn attempted_returns_at_once_for_a_copy_that_is_not_due() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let url = server.url("/f");
        seed_cache(root.path(), &url, b"fresh", SystemTime::now());
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        tokio::time::timeout(
            Duration::ZERO,
            mgr.attempted(&resource_key(&source_of(&url))),
        )
        .await
        .expect("nothing is due");
        not_due(&h).await;
        assert_eq!(server.hits("/f"), 0);
    }

    #[tokio::test]
    async fn attempted_returns_when_the_entry_is_retired_meanwhile() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/s", "held");
        server.set_delay("/s", Duration::from_secs(3));
        let url = server.url("/s");
        let source = source_of(&url);
        let key = resource_key(&source);
        seed_cache(root.path(), &url, b"held", two_days_ago());
        let mgr = manager(root.path(), fast());
        mgr.get(&url_spec(url, None));
        wait_until("the request to arrive", || server.hits("/s") == 1).await;
        let waiting = tokio::spawn({
            let (mgr, key) = (mgr.clone(), key.clone());
            async move { mgr.attempted(&key).await }
        });
        // it is waiting: nothing has been recorded
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        assert!(mgr.retire(&source));
        tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .expect("it does not wait for the attempt")
            .unwrap();
    }

    /// What `adopt` leaves behind for a source that is registered: the body is
    /// the content, as a version of its own, and the cache says it was taken
    /// just now and that nothing is known of its validators.
    #[tokio::test]
    async fn adopting_into_a_registered_source_is_a_version_of_its_own() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/a", "one");
        let url = server.url("/a");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let mut rx = h.subscribe();
        let first = status_of(&mgr, &source).fetched_at.expect("a time");

        mgr.adopt(&source, Bytes::from_static(b"two")).unwrap();
        let s = h.current();
        assert_eq!(available_version(&s), Some(2));
        assert_eq!(&s.data().unwrap().0[..], b"two");
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), 2);
        let (data, meta) = cache_of(root.path(), &url).load().expect("a cache");
        assert_eq!(&data[..], b"two");
        assert_eq!(
            (meta.etag.as_deref(), meta.last_modified.as_deref()),
            (None, None)
        );
        let status = status_of(&mgr, &source);
        assert!(status.fetched_at > Some(first));
        assert_eq!(Some(meta.fetched_time()), status.fetched_at);

        // the next refresh fetches the whole body: it carries no validators
        server.set("/a", "two");
        assert!(mgr.force_update(&source));
        wait_until("the check to be recorded", || {
            status_of(&mgr, &source).fetched_at > status.fetched_at
        })
        .await;
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].header("if-none-match").is_none());
        assert_eq!(h.version(), 2);
    }

    /// The body the entry holds already is no new version, as for a download
    /// that brings it: the time moves, the validators go, nobody is told.
    #[tokio::test]
    async fn adopting_the_body_the_entry_holds_is_no_new_version() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/a", "one");
        let url = server.url("/a");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let rx = h.subscribe();
        let before = status_of(&mgr, &source).fetched_at.expect("a time");
        assert!(cache_of(root.path(), &url).load().unwrap().1.etag.is_some());

        mgr.adopt(&source, Bytes::from_static(b"one")).unwrap();
        assert_eq!(h.version(), 1);
        assert!(!rx.has_changed().unwrap(), "somebody was told");
        let meta = cache_of(root.path(), &url).load().unwrap().1;
        assert_eq!(
            (meta.etag.as_deref(), meta.last_modified.as_deref()),
            (None, None)
        );
        let status = status_of(&mgr, &source);
        assert!(status.fetched_at > Some(before));
        assert_eq!(Some(meta.fetched_time()), status.fetched_at);

        // also for an entry that failed and kept its copy: it is `available` again
        server.set_status("/a", 500);
        assert!(mgr.force_update(&source));
        wait_for(&h, "failed with the copy", |s| {
            matches!(
                s,
                ResourceState::Failed {
                    cached: Some(_),
                    ..
                }
            )
        })
        .await;
        mgr.adopt(&source, Bytes::from_static(b"one")).unwrap();
        let status = status_of(&mgr, &source);
        assert_eq!((status.state, status.version), ("available", 1));
        assert_eq!(status.last_error, None);
        assert!(!rx.has_changed().unwrap(), "somebody was told");
    }

    /// A source nobody registered only has its cache written; whoever registers
    /// it afterwards starts from that.
    #[tokio::test]
    async fn adopting_a_source_nobody_registered_writes_the_cache_only() {
        let root = tempfile::tempdir().unwrap();
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(root.path(), offline);
        let url = Url::parse("https://lists.test/adopted.list").unwrap();
        let source = source_of(&url);
        mgr.adopt(&source, Bytes::from_static(b"adopted")).unwrap();
        assert!(mgr.statuses().is_empty());
        let copy = cached_copy(root.path(), &url).expect("a cache");
        assert_eq!(&copy.data[..], b"adopted");
        let (_, meta) = cache_of(root.path(), &url).load().unwrap();
        assert_eq!((meta.etag, meta.last_modified), (None, None));
        let h = mgr.get(&url_spec(url, None));
        let s = h.current();
        assert_eq!(available_version(&s), Some(1));
        assert_eq!(&s.data().unwrap().0[..], b"adopted");
    }

    /// A cache that cannot be written is an error, and the entry stays as it
    /// was.
    #[tokio::test]
    async fn adopting_into_a_cache_that_cannot_be_written_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        // the root is a file
        let root = dir.path().join("root");
        std::fs::write(&root, "not a directory").unwrap();
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(&root, offline);
        let url = Url::parse("https://lists.test/adopted.list").unwrap();
        let source = source_of(&url);
        assert!(mgr.adopt(&source, Bytes::from_static(b"adopted")).is_err());
        let h = mgr.get(&url_spec(url, None));
        let rx = h.subscribe();
        let before = status_of(&mgr, &source);
        assert_eq!(before.state, "failed");
        assert!(mgr.adopt(&source, Bytes::from_static(b"adopted")).is_err());
        let after = status_of(&mgr, &source);
        assert_eq!(
            (after.state, after.version, after.last_error, after.ready),
            (
                before.state,
                before.version,
                before.last_error,
                before.ready
            )
        );
        assert!(!rx.has_changed().unwrap(), "somebody was told");
    }

    /// The same for an entry that holds a copy.
    #[tokio::test]
    async fn adopting_what_the_disk_cannot_take_leaves_the_entry_as_it_was() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/a", "one");
        let url = server.url("/a");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let rx = h.subscribe();
        let before = (status_of(&mgr, &source), cache_files(root.path(), &url));
        let blocked = block_writes(root.path(), &url, "data");
        assert!(mgr.adopt(&source, Bytes::from_static(b"two")).is_err());
        let after = (status_of(&mgr, &source), cache_files(root.path(), &url));
        assert_eq!(after.1, before.1);
        assert_eq!(
            (after.0.state, after.0.version, after.0.fetched_at),
            (before.0.state, before.0.version, before.0.fetched_at)
        );
        assert_eq!(&h.current().data().unwrap().0[..], b"one");
        assert!(!rx.has_changed().unwrap(), "somebody was told");
        // and the way clear, it goes in
        std::fs::remove_dir(&blocked).unwrap();
        mgr.adopt(&source, Bytes::from_static(b"two")).unwrap();
        assert_eq!(h.version(), 2);
    }

    #[tokio::test]
    async fn a_file_cannot_be_adopted() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("local.list");
        std::fs::write(&file, "one").unwrap();
        let mgr = manager(root.path(), fast());
        let source = ResourceSource::File(file);
        let h = mgr.get(&ResourceSpec {
            source: source.clone(),
            update_interval: None,
            kind: ResourceKind::RuleSet,
        });
        let error = mgr
            .adopt(&source, Bytes::from_static(b"two"))
            .expect_err("a file");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(&h.current().data().unwrap().0[..], b"one");
        assert_eq!(h.version(), 1);
    }

    /// An adoption while the entry's own download is under way: a download
    /// that brings the same body finds it in hand, and one that brings
    /// another is a version of its own.
    #[tokio::test]
    async fn an_adoption_during_a_download_of_the_same_body_is_one_version() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/o", "one");
        let url = server.url("/o");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let mut rx = h.subscribe();
        server.set("/o", "two");
        server.set_delay("/o", Duration::from_millis(300));
        assert!(mgr.force_update(&source));
        wait_until("the request to arrive", || server.hits("/o") == 2).await;
        mgr.adopt(&source, Bytes::from_static(b"two")).unwrap();
        assert_eq!(h.version(), 2);
        assert!(
            status_of(&mgr, &source).updating,
            "the download is not the adoption's"
        );
        wait_until("the answer to be recorded", || {
            !status_of(&mgr, &source).updating
        })
        .await;
        assert_eq!(h.version(), 2);
        assert_eq!(&h.current().data().unwrap().0[..], b"two");
        assert_eq!(*rx.borrow_and_update(), 2);
        assert!(!rx.has_changed().unwrap(), "told twice");
        let copy = cached_copy(root.path(), &url).unwrap();
        assert_eq!(&copy.data[..], b"two");
        assert_eq!(Some(copy.fetched_at), status_of(&mgr, &source).fetched_at);
    }

    #[tokio::test]
    async fn an_adoption_during_a_download_of_another_body_is_followed_by_the_download() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/o", "one");
        let url = server.url("/o");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        server.set("/o", "two");
        server.set_delay("/o", Duration::from_millis(300));
        assert!(mgr.force_update(&source));
        wait_until("the request to arrive", || server.hits("/o") == 2).await;
        mgr.adopt(&source, Bytes::from_static(b"three")).unwrap();
        assert_eq!(h.version(), 2);
        assert_eq!(&h.current().data().unwrap().0[..], b"three");
        wait_until("the answer to be recorded", || {
            !status_of(&mgr, &source).updating
        })
        .await;
        // what was downloaded is the content
        assert_eq!(h.version(), 3);
        assert_eq!(&h.current().data().unwrap().0[..], b"two");
        assert_eq!(&cached_copy(root.path(), &url).unwrap().data[..], b"two");
    }

    /// The check of an entry runs before the lock is taken for recording, so
    /// an adoption can get in: the result is looked at again when it is
    /// recorded.
    #[tokio::test]
    async fn an_adoption_during_the_check_is_looked_at_again_when_the_result_is_recorded() {
        for adopted in ["profile", "another"] {
            let root = tempfile::tempdir().unwrap();
            let server = TestServer::spawn().await;
            server.set("/p", "profile");
            let url = server.url("/p");
            let source = source_of(&url);
            let (entered, release) = (Arc::new(Latch::default()), Arc::new(Latch::default()));
            let check: Check = {
                let (entered, release) = (entered.clone(), release.clone());
                Arc::new(move |_: &[u8]| {
                    entered.open();
                    release.wait();
                    Ok(())
                })
            };
            let mgr = manager(root.path(), fast());
            let h = mgr.get_checked(&profile_spec(&url), "the profile", check);
            let mut rx = h.subscribe();
            wait_until("the check to begin", || entered.is_open()).await;
            mgr.adopt(&source, Bytes::from(adopted)).unwrap();
            assert_eq!(h.version(), 1);
            release.open();
            wait_finished(&h, 1).await;
            // the same body is the version the adoption made; another is a version more
            let expected = if adopted == "profile" { 1 } else { 2 };
            assert_eq!(h.version(), expected, "{adopted}");
            assert_eq!(&h.current().data().unwrap().0[..], b"profile");
            assert_eq!(*rx.borrow_and_update(), expected);
            assert_eq!(
                &cached_copy(root.path(), &url).unwrap().data[..],
                b"profile"
            );
        }
    }

    #[tokio::test]
    async fn a_retired_entry_is_no_longer_listed_and_asks_for_nothing_more() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/r", "r");
        // a resource that fails asks again and again
        server.set_status("/r", 500);
        let url = server.url("/r");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_until("a few requests", || server.hits("/r") >= 3).await;
        assert!(mgr.retire(&source));
        assert!(mgr.statuses().is_empty());
        assert!(!mgr.force_update(&source));
        wait_ended(&h).await;
        let asked = server.hits("/r");
        // That nothing more happens can only be seen by waiting: a resource that fails asks
        // again within 200 ms, and the wait is three times that. A request that was on its way
        // when the task ended may still arrive, which is one; a task that went on makes more.
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(server.hits("/r") <= asked + 1, "the task went on asking");
        // the second call finds nothing to retire
        assert!(!mgr.retire(&source));
        assert!(!mgr.retire(&ResourceSource::File(PathBuf::from("nothing.list"))));
    }

    /// The task of an entry that waits for its next refresh ends at once.
    #[tokio::test]
    async fn retiring_an_entry_that_waits_for_its_refresh_ends_its_task_at_once() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let url = server.url("/w");
        let source = source_of(&url);
        seed_cache(root.path(), &url, b"fresh", SystemTime::now());
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url, None));
        wait_until("the task to wait for its refresh", || {
            h.entry.next_refresh.lock().unwrap().is_some()
        })
        .await;
        assert!(mgr.retire(&source));
        // the refresh is a day away
        wait_ended(&h).await;
        assert_eq!(server.hits("/w"), 0);
    }

    #[tokio::test]
    async fn retiring_an_entry_during_an_attempt_leaves_the_cache_and_the_version_alone() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/a", "one");
        let url = server.url("/a");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url.clone(), None));
        wait_for(&h, "v1", |s| available_version(s) == Some(1)).await;
        let rx = h.subscribe();
        let before = cache_files(root.path(), &url);
        server.set("/a", "two");
        server.set_delay("/a", Duration::from_millis(300));
        assert!(mgr.force_update(&source));
        wait_until("the request to arrive", || server.hits("/a") == 2).await;
        assert!(status_of(&mgr, &source).updating);
        assert!(mgr.retire(&source));
        wait_ended(&h).await;
        assert!(!h.entry.updating.load(Ordering::SeqCst));
        // the answer comes to the server in 300 ms and is nobody's now: the wait is longer
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(cache_files(root.path(), &url), before);
        assert_eq!(h.version(), 1);
        assert_eq!(&h.current().data().unwrap().0[..], b"one");
        assert!(!rx.has_changed().unwrap(), "somebody was told");
    }

    #[tokio::test]
    async fn retiring_an_entry_during_its_check_leaves_nothing_behind() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "profile");
        let url = server.url("/p");
        let source = source_of(&url);
        let (entered, release, done) = (
            Arc::new(Latch::default()),
            Arc::new(Latch::default()),
            Arc::new(Latch::default()),
        );
        let check: Check = {
            let (entered, release, done) = (entered.clone(), release.clone(), done.clone());
            Arc::new(move |_: &[u8]| {
                entered.open();
                release.wait();
                done.open();
                Ok(())
            })
        };
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", check);
        let rx = h.subscribe();
        wait_until("the check to begin", || entered.is_open()).await;
        assert!(mgr.retire(&source));
        wait_ended(&h).await;
        // the check goes on to its end, which nobody waits for any more
        release.open();
        wait_until("the check to end", || done.is_open()).await;
        tokio::task::yield_now().await;
        assert!(!cache_of(root.path(), &url).path().exists());
        assert!(matches!(h.current(), ResourceState::Missing));
        assert!(!rx.has_changed().unwrap(), "somebody was told");
        assert!(!h.entry.updating.load(Ordering::SeqCst));
    }

    /// An answer that came just before the entry was retired is on its way to
    /// be recorded when the retirement gets the lock first: the attempt finds
    /// the entry retired, and nothing is written or told.
    #[tokio::test]
    async fn an_attempt_is_not_recorded_for_an_entry_that_is_retired() {
        let root = tempfile::tempdir().unwrap();
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(root.path(), offline);
        let url = Url::parse("https://lists.test/a.list").unwrap();
        let h = mgr.get(&url_spec(url.clone(), None));
        let rx = h.subscribe();
        h.entry.progress.send_modify(|p| p.retired = true);
        let cache = cache_of(root.path(), &url);
        let late = Outcome::New {
            data: Bytes::from_static(b"late"),
            etag: None,
            last_modified: None,
        };
        assert!(record_attempt(&h.entry, &cache, &url, late).is_none());
        assert!(!cache.path().exists());
        assert!(h.current().data().is_none());
        assert!(!rx.has_changed().unwrap(), "somebody was told");
    }

    /// A retired entry is gone for whoever looks for it, and a handle that is
    /// still held reads what the entry had last. Registering the source again
    /// is a new entry that reads the cache.
    #[tokio::test]
    async fn registering_a_retired_source_again_starts_from_its_cache() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/a", "one");
        let url = server.url("/a");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        let old = mgr.get_labelled(&url_spec(url.clone(), None), "the old one");
        wait_for(&old, "v1", |s| available_version(s) == Some(1)).await;
        assert!(mgr.retire(&source));
        mgr.adopt(&source, Bytes::from_static(b"adopted after"))
            .unwrap();
        let new = mgr.get_labelled(&url_spec(url, None), "the new one");
        assert!(!Arc::ptr_eq(&old.entry, &new.entry));
        // what the cache holds, not what the old entry had
        let s = new.current();
        assert_eq!(available_version(&s), Some(1));
        assert_eq!(&s.data().unwrap().0[..], b"adopted after");
        let statuses = mgr.statuses();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].label.as_deref(), Some("the new one"));
        assert_eq!(&old.current().data().unwrap().0[..], b"one");
        // the new entry has a task: it can be asked
        server.set("/a", "two");
        assert_eq!(updated(&mgr, &resource_key(&source)).await, Some(Ok(())));
        assert_eq!(&new.current().data().unwrap().0[..], b"two");
        assert_eq!(&old.current().data().unwrap().0[..], b"one");
    }

    /// `retire` has set the flag and not yet taken the entry out of the map:
    /// whoever registers the source then gets an entry of its own.
    #[tokio::test]
    async fn a_source_is_registered_anew_over_an_entry_that_is_retired() {
        let root = tempfile::tempdir().unwrap();
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(root.path(), offline);
        let url = Url::parse("https://lists.test/a.list").unwrap();
        let source = source_of(&url);
        let old = mgr.get(&url_spec(url.clone(), None));
        old.entry.progress.send_modify(|p| p.retired = true);
        let new = mgr.get(&url_spec(url, None));
        assert!(!Arc::ptr_eq(&old.entry, &new.entry));
        // the entry `retire` finds is the new one
        assert!(mgr.retire(&source));
        assert!(mgr.statuses().is_empty());
        assert!(new.entry.progress.borrow().retired);
    }

    /// A file's task ends too.
    #[tokio::test]
    async fn retiring_a_file_ends_its_task() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("local.list");
        std::fs::write(&file, "one").unwrap();
        let mgr = manager(root.path(), fast());
        let source = ResourceSource::File(file.clone());
        let h = mgr.get(&ResourceSpec {
            source: source.clone(),
            update_interval: None,
            kind: ResourceKind::RuleSet,
        });
        let entry = Arc::downgrade(&h.entry);
        assert!(mgr.retire(&source));
        assert_eq!(&h.current().data().unwrap().0[..], b"one");
        drop(h);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while entry.strong_count() > 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the task still holds its entry"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// `wait_until` for a test that stops a thread of its runtime, on the
    /// test's own thread and by the clock of the system. A timer of the
    /// runtime is fired by a worker that has nothing else to do; the one that
    /// did it goes on to run the task that stops it without waking another,
    /// and the other can be asleep until somebody gives it a task. A wait on
    /// such a timer would then never end.
    fn wait_here(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// `updating` is lowered before the new version is announced: whoever the
    /// version wakes finds a download that is over. The test holds a value of
    /// the version channel, which keeps the next one from being announced: the
    /// task stops right there, and by then the download has to be over.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn updating_is_down_before_a_new_version_is_announced() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/v", "one");
        let url = server.url("/v");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url, None));
        let mut rx = h.subscribe();
        // the first version has been announced: nothing of it is still on its way to the
        // channel when the value is held
        wait_until("the first version to be announced", || *rx.borrow() == 1).await;
        assert_eq!(*rx.borrow_and_update(), 1);
        server.set("/v", "two");
        // from here until the value is let go the task may stop a thread of the runtime
        let held = rx.borrow();
        assert!(mgr.force_update(&source));
        wait_here("the request to arrive", || server.hits("/v") == 2);
        // This wait is the test. Nothing is announced while the value is held, so a download
        // that is not over by now stays so until the wait gives up.
        wait_here("the download to be over", || {
            !h.entry.updating.load(Ordering::SeqCst)
        });
        assert!(!rx.has_changed().unwrap(), "the version was announced");
        drop(held);
        told(&mut rx).await;
        assert_eq!(h.version(), 2);
    }

    /// And before the attempt counts as finished: whoever `update_now` or
    /// `attempted` lets go finds a download that is over. The test holds the
    /// value of the progress channel, so the task stops where it would count
    /// the attempt.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn updating_is_down_before_an_attempt_counts_as_finished() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/p", "profile");
        let url = server.url("/p");
        let (entered, release) = (Arc::new(Latch::default()), Arc::new(Latch::default()));
        let check: Check = {
            let (entered, release) = (entered.clone(), release.clone());
            Arc::new(move |_: &[u8]| {
                entered.open();
                release.wait();
                Err("not a profile".to_string())
            })
        };
        let mgr = manager(root.path(), fast());
        let h = mgr.get_checked(&profile_spec(&url), "the profile", check);
        wait_until("the check to begin", || entered.is_open()).await;
        // from here until the value is let go the task may stop a thread of the runtime
        let held = h.entry.progress.borrow();
        assert_eq!((held.started, held.finished), (1, 0));
        assert!(h.entry.updating.load(Ordering::SeqCst));
        release.open();
        // This wait is the test. The attempt is not counted while the value is held, so a
        // download that is not over by now stays so until the wait gives up.
        wait_here("the download to be over", || {
            !h.entry.updating.load(Ordering::SeqCst)
        });
        assert_eq!(held.finished, 0, "the attempt was counted");
        drop(held);
        wait_finished(&h, 1).await;
    }

    /// And when the attempt found nothing new, which is how most of them end.
    /// Another thread keeps the lock of the cache files until the test has the
    /// value of the progress channel: the answer is recorded, and the download
    /// has to be over, with the count still to come.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn updating_is_down_before_an_attempt_that_found_nothing_new_counts_as_finished() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/s", "same");
        let url = server.url("/s");
        let source = source_of(&url);
        let mgr = manager(root.path(), fast());
        let h = mgr.get(&url_spec(url, None));
        wait_finished(&h, 1).await;
        let (holding, let_go) = (Arc::new(Latch::default()), Arc::new(Latch::default()));
        let holder = std::thread::spawn({
            let (entry, holding, let_go) = (h.entry.clone(), holding.clone(), let_go.clone());
            move || {
                let _record = entry.record.lock().unwrap();
                holding.open();
                let_go.wait();
            }
        });
        // from here until the value is let go the task may stop a thread of the runtime:
        // at that lock first, then where it would count the attempt
        wait_here("the lock of the cache files to be taken", || {
            holding.is_open()
        });
        assert!(mgr.force_update(&source));
        wait_here("the request to arrive", || server.hits("/s") == 2);
        // the answer cannot be recorded yet, so the attempt is under way for as long as it
        // takes to get hold of the value
        let held = h.entry.progress.borrow();
        assert_eq!((held.started, held.finished), (2, 1));
        assert!(h.entry.updating.load(Ordering::SeqCst));
        let_go.open();
        // This wait is the test, as above.
        wait_here("the download to be over", || {
            !h.entry.updating.load(Ordering::SeqCst)
        });
        assert_eq!(held.finished, 1, "the attempt was counted");
        drop(held);
        holder.join().unwrap();
        wait_finished(&h, 2).await;
        // "not modified": the copy and its version are the ones there were
        assert_eq!(h.version(), 1);
    }
}
