//! The sources of the profile that are downloaded: the managed profile
//! (`#!MANAGED-CONFIG`) and the files a `#!include` names by a URL.
//!
//! [`ProfileSources`] lives as long as the process. It owns the resource
//! manager that downloads them, follows the sources the generation in use
//! came across, and answers the three questions the main loop asks of them:
//! did a source change (reload), is the precedence of the local file over
//! (reload), and does `strict` ask for a suspension. The manager downloads
//! through a client of its own, never through the pipeline, so a suspended
//! engine can still update.

use rurge_config::remote::origin;
use rurge_config::{LoadOptions, Remote, RemoteRef, RemoteRole, RemoteSource, SourceInfo};
use rurge_net::http::HttpClient;
use rurge_net::resource::{
    CachedCopy, Check, ResourceKind, ResourceManager, ResourceOptions, ResourceSource,
    ResourceSpec, ResourceStatus, cached_copy, resource_key,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, SystemTime};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use url::Url;

/// How long no source may have moved before `changed` reports: the sources a
/// provider changes together are one report.
const QUIET: Duration = Duration::from_secs(1);

/// The longest the sources of the executable leave a failing download alone.
/// The default is an hour, and a profile that `strict` suspended would wait
/// that long after the network is back.
const MAX_BACKOFF: Duration = Duration::from_secs(600);

/// Why a downloaded body is not taken, and why a cached one is not read: a
/// profile is text.
const NOT_UTF8: &str = "the downloaded file is not UTF-8 text";

/// What `strict` says of a process that is offline.
const OFFLINE: &str = "offline mode: the update cannot be attempted";

/// What `ProfileSources::new` is made of.
pub struct SourcesOptions {
    /// The local profile (`-c`). Its modification time is read when `outdated`
    /// and `strict` are asked.
    pub profile: PathBuf,
    /// The environment, platform and capabilities a downloaded profile is
    /// loaded with when it is judged; its `remote` is not looked at. Not read
    /// as long as a downloaded body is only checked for being UTF-8 text.
    pub load: LoadOptions,
    /// Where the downloads are kept: the cache of the other external
    /// resources is there too, and one address has one place in it.
    pub data_dir: PathBuf,
    /// Nothing is downloaded; what is on disk is all there is.
    pub offline: bool,
    pub client: Arc<HttpClient>,
    /// Back-off and time-outs. `offline` and `never_early` are decided by
    /// `ProfileSources`: a profile is not to be asked for before its
    /// interval is over.
    pub resources: ResourceOptions,
}

/// Why the engine is to refuse sessions (`strict`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suspended {
    /// When the update fell due.
    pub since: SystemTime,
    /// The reason the update did not happen.
    pub why: String,
}

/// A source the generation in use came across, and what has been told of its
/// versions.
struct Tracked {
    /// Tells this registration from an earlier one of the same address.
    id: u64,
    url: Url,
    role: RemoteRole,
    interval: Duration,
    strict: bool,
    /// `resource_key` of the address: what the manager and the API call it.
    key: String,
    /// The version that was last reported, and the last one seen.
    reported: u64,
    seen: u64,
    /// The registration this one replaced had a version nobody was told of.
    /// The new one starts from the copy on disk and cannot tell, so it keeps
    /// the fact.
    unreported: bool,
    /// The versions as the manager publishes them: what `seen` becomes when
    /// the task that follows them has run.
    versions: watch::Receiver<u64>,
    _follow: Follower,
}

impl Tracked {
    fn pending(&self) -> bool {
        self.seen != self.reported || self.unreported
    }

    /// Whether a version would go untold if this registration were dropped
    /// now: one that is pending, or one published a moment ago that the task
    /// following the source has not told of yet.
    fn leaves_unreported(&self) -> bool {
        // Copied out at once: a value borrowed from the receiver holds up the
        // next version.
        let published = *self.versions.borrow();
        self.pending() || published != self.reported
    }
}

/// Ends the task that follows a source when the source is no longer tracked.
struct Follower(Option<JoinHandle<()>>);

impl Drop for Follower {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

#[derive(Default)]
struct State {
    /// The sources of the generation tracked last, in its order, then the
    /// ones registered since.
    tracked: Vec<Tracked>,
    next_id: u64,
    /// When a source last moved.
    moved_at: Option<Instant>,
    /// The generation tracked last is the local file, and a downloaded copy of
    /// the managed profile exists.
    local_first: bool,
    /// The check `outdated` last said yes for.
    outdated_for: Option<SystemTime>,
}

/// What the tasks that follow the sources reach: the state, and a counter of
/// the moves, which wakes a call of `changed` that is waiting.
struct Core {
    state: Mutex<State>,
    moved: watch::Sender<u64>,
}

impl Core {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The source `id` is at `version` now. Whether it is still tracked.
    fn moved_to(&self, id: u64, version: u64) -> bool {
        {
            let mut state = self.lock();
            let Some(source) = state.tracked.iter_mut().find(|t| t.id == id) else {
                return false;
            };
            if source.seen == version {
                return true;
            }
            source.seen = version;
            state.moved_at = Some(Instant::now());
        }
        self.moved
            .send_modify(|moves| *moves = moves.wrapping_add(1));
        true
    }
}

/// Tells `core` of the versions the source `id` is published as.
async fn follow(core: Weak<Core>, id: u64, mut versions: watch::Receiver<u64>) {
    while versions.changed().await.is_ok() {
        // Copied out at once: a value borrowed from the receiver holds up the
        // next version, and with it the writer of the cache of the source.
        let version = *versions.borrow_and_update();
        let Some(core) = core.upgrade() else { return };
        if !core.moved_to(id, version) {
            return;
        }
    }
}

/// The downloaded sources of the profile: one resource manager for the life of
/// the process, and the sources of the generation in use.
pub struct ProfileSources {
    manager: Arc<ResourceManager>,
    profile: PathBuf,
    offline: bool,
    core: Arc<Core>,
}

impl ProfileSources {
    pub fn new(opts: SourcesOptions) -> Arc<ProfileSources> {
        let SourcesOptions {
            profile,
            load: _,
            data_dir,
            offline,
            client,
            mut resources,
        } = opts;
        resources.offline = offline;
        resources.never_early = true;
        Arc::new(ProfileSources {
            manager: ResourceManager::with_options(data_dir, client, resources),
            profile,
            offline,
            core: Arc::new(Core {
                state: Mutex::new(State::default()),
                moved: watch::Sender::new(0),
            }),
        })
    }

    /// The sources of the executable: downloads the way the other resources
    /// are downloaded, with a back-off of ten minutes at the most.
    pub fn production(
        profile: &Path,
        load: &LoadOptions,
        data_dir: &Path,
        offline: bool,
    ) -> anyhow::Result<Arc<ProfileSources>> {
        Ok(ProfileSources::new(SourcesOptions {
            profile: profile.to_path_buf(),
            load: load.clone(),
            data_dir: data_dir.to_path_buf(),
            offline,
            client: crate::stack::resource_client()?,
            resources: ResourceOptions {
                max_backoff: MAX_BACKOFF,
                ..ResourceOptions::default()
            },
        }))
    }

    /// Where the loader reads downloaded text from: the disk, as
    /// [`disk_remote`] reads it, and for a source that has none the reason the
    /// last attempt failed.
    pub fn remote(self: &Arc<Self>) -> Arc<dyn RemoteSource> {
        Arc::new(TrackedRemote {
            root: self.manager.root().to_path_buf(),
            sources: Arc::downgrade(self),
        })
    }

    /// Follows the sources of a generation: the managed profile and the remote
    /// includes. What the generation came across on the way to the managed
    /// profile is read from its copy and never asked for. A source the
    /// generation no longer names is dropped. Registering a source whose copy
    /// the generation read, or again, is no change; a copy the generation did
    /// not read is one.
    pub fn track(&self, sources: &SourceInfo) {
        let mut state = self.core.lock();
        let mut before = std::mem::take(&mut state.tracked);
        let mut tracked = Vec::new();
        for wanted in sources
            .remote
            .iter()
            .filter(|r| r.role != RemoteRole::ManagedHop)
        {
            let held = before
                .iter()
                .position(|t| t.url == wanted.url)
                .map(|at| before.remove(at));
            match held {
                Some(mut kept) if kept.role == wanted.role && kept.interval == wanted.interval => {
                    kept.strict = wanted.strict;
                    tracked.push(kept);
                }
                replaced => {
                    let unreported = replaced.as_ref().is_some_and(Tracked::leaves_unreported);
                    if let Some(old) = replaced {
                        // The manager keeps the first interval and label an
                        // entry is given, and only takes shorter intervals: a
                        // source whose interval or role changed starts again.
                        self.retire(&old);
                    }
                    let id = state.next_id;
                    state.next_id += 1;
                    let mut new = self.register(id, wanted);
                    new.unreported = unreported;
                    tracked.push(new);
                }
            }
        }
        for gone in before {
            self.retire(&gone);
        }
        let pending = tracked.iter().any(Tracked::pending);
        state.tracked = tracked;
        state.local_first = sources.main_text == sources.main
            && sources
                .remote
                .iter()
                .any(|r| r.role == RemoteRole::Managed && r.found());
        state.outdated_for = None;
        drop(state);
        if pending {
            // A registration can begin with a version its generation was not
            // built from, which no task that follows a source will tell of:
            // a call of `changed` that waits looks again.
            self.core
                .moved
                .send_modify(|moves| *moves = moves.wrapping_add(1));
        }
    }

    fn retire(&self, source: &Tracked) {
        self.manager
            .retire(&ResourceSource::Url(source.url.clone()));
    }

    fn register(&self, id: u64, wanted: &RemoteRef) -> Tracked {
        let source = ResourceSource::Url(wanted.url.clone());
        let key = resource_key(&source);
        let (kind, label) = match wanted.role {
            RemoteRole::Include => (
                ResourceKind::Include,
                format!(
                    "remote include {} ({})",
                    key.get(..8).unwrap_or(&key),
                    origin(&wanted.url)
                ),
            ),
            _ => (
                ResourceKind::Profile,
                format!("managed profile ({})", origin(&wanted.url)),
            ),
        };
        let check: Check = Arc::new(accepts);
        let handle = self.manager.get_checked(
            &ResourceSpec {
                source,
                update_interval: Some(seconds(wanted.interval)),
                kind,
            },
            &label,
            check,
        );
        // Subscribed before the version is read, so that a version published
        // after that is not missed.
        let versions = handle.subscribe();
        let seen = *versions.borrow();
        Tracked {
            id,
            url: wanted.url.clone(),
            role: wanted.role,
            interval: wanted.interval,
            strict: wanted.strict,
            key,
            reported: built_from(wanted.found(), seen),
            seen,
            unreported: false,
            versions: versions.clone(),
            _follow: self.follow(id, versions),
        }
    }

    fn follow(&self, id: u64, versions: watch::Receiver<u64>) -> Follower {
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => Follower(Some(runtime.spawn(follow(
                Arc::downgrade(&self.core),
                id,
                versions,
            )))),
            Err(_) => {
                tracing::debug!(
                    "no tokio runtime: the versions of a profile source are not followed"
                );
                Follower(None)
            }
        }
    }

    /// Waits until every source without content and every `strict` managed
    /// profile that has fallen due has had its first attempt, for `limit` at
    /// the most. Whether it did. The first attempt is the one a source is
    /// registered with: a profile that was not due then and falls due later
    /// is not waited for. A due source that is not `strict` is refreshed in
    /// the background and not waited for; a source registered while this
    /// waits is waited for too.
    pub async fn settle(&self, limit: Duration) -> bool {
        // Far enough to be never, and a time that can be added to.
        let limit = limit.min(Duration::from_secs(365 * 24 * 3600));
        let deadline = Instant::now() + limit;
        let mut waited = HashSet::new();
        loop {
            let todo = self.unsettled(&waited);
            if todo.is_empty() {
                return true;
            }
            let first_attempts = async {
                for (_, key) in &todo {
                    self.manager.attempted(key).await;
                }
            };
            if tokio::time::timeout_at(deadline, first_attempts)
                .await
                .is_err()
            {
                return false;
            }
            waited.extend(todo.into_iter().map(|(id, _)| id));
        }
    }

    /// The sources `settle` still has to wait for, other than the ones it has
    /// waited for: the registration and the key of each.
    fn unsettled(&self, waited: &HashSet<u64>) -> Vec<(u64, String)> {
        let local = file_modified(&self.profile);
        let now = SystemTime::now();
        let state = self.core.lock();
        let statuses = self.manager.statuses();
        state
            .tracked
            .iter()
            .filter(|t| !waited.contains(&t.id))
            .filter(|t| {
                let Some(status) = statuses.iter().find(|s| s.key == t.key) else {
                    return false;
                };
                !status.ready
                    || (t.role == RemoteRole::Managed
                        && t.strict
                        && due_since(t.interval, status.fetched_at, local, now).is_some())
            })
            .map(|t| (t.id, t.key.clone()))
            .collect()
    }

    /// Returns when a tracked source has a version that has not been reported,
    /// a quiet second after the last time one moved, so that sources that
    /// change together are one report. What it waits for is kept by the
    /// sources and not by the call: a call that is dropped and made again
    /// loses nothing, and a version that comes when nobody calls is reported
    /// by the next call.
    pub async fn changed(&self) {
        loop {
            // Subscribed before the state is looked at, so that a move after
            // that wakes the wait below.
            let mut moved = self.core.moved.subscribe();
            let quiet_until = {
                let mut state = self.core.lock();
                if state.tracked.iter().any(Tracked::pending) {
                    let until = state.moved_at.map_or_else(Instant::now, |at| at + QUIET);
                    if Instant::now() >= until {
                        for source in &mut state.tracked {
                            source.reported = source.seen;
                            source.unreported = false;
                        }
                        return;
                    }
                    Some(until)
                } else {
                    None
                }
            };
            match quiet_until {
                None => {
                    let _ = moved.changed().await;
                }
                Some(until) => {
                    tokio::select! {
                        _ = tokio::time::sleep_until(until) => {}
                        _ = moved.changed() => {}
                    }
                }
            }
        }
    }

    /// The local file is in effect although a downloaded copy of the managed
    /// profile exists, and a check has succeeded since the file was last
    /// changed: the copy is the profile from now on, which a reload sees. True
    /// once for each check, so that a reload that fails is not asked for again
    /// and again. A check that found nothing new (the server said "not
    /// modified") is one: it publishes no version.
    pub fn outdated(&self) -> bool {
        let local = file_modified(&self.profile);
        let mut state = self.core.lock();
        if !state.local_first {
            return false;
        }
        // a file whose time cannot be read is taken as just changed
        let Some(local) = local else { return false };
        let Some(managed) = state.tracked.iter().find(|t| t.role == RemoteRole::Managed) else {
            return false;
        };
        let Some(checked) = self.status_of(&managed.key).and_then(|s| s.fetched_at) else {
            return false;
        };
        // The loader keeps the local file only while it is newer than every
        // copy; at the same time the copy wins.
        if checked < local || state.outdated_for == Some(checked) {
            return false;
        }
        state.outdated_for = Some(checked);
        true
    }

    /// Whether the profile requires an update that did not happen. Three
    /// things hold together: the managed profile says `strict=true`; its last
    /// successful check is an interval ago or more (counted from the time the
    /// local file was changed when there was none; a check in the future is the
    /// clock having been set back and is not due); and the last attempt failed,
    /// or the process is offline. A source that is due and has not been
    /// attempted yet is not suspended.
    pub fn strict(&self) -> Option<Suspended> {
        let local = file_modified(&self.profile);
        let now = SystemTime::now();
        let state = self.core.lock();
        let managed = state
            .tracked
            .iter()
            .find(|t| t.role == RemoteRole::Managed && t.strict)?;
        let status = self.status_of(&managed.key)?;
        let since = due_since(managed.interval, status.fetched_at, local, now)?;
        let why = if self.offline {
            OFFLINE.to_string()
        } else if status.state == "failed" {
            status.last_error.unwrap_or_default()
        } else {
            return None;
        };
        Some(Suspended { since, why })
    }

    /// The status of each tracked source, in the order of the generation
    /// tracked last.
    pub fn statuses(&self) -> Vec<ResourceStatus> {
        let state = self.core.lock();
        let all = self.manager.statuses();
        state
            .tracked
            .iter()
            .filter_map(|t| all.iter().find(|s| s.key == t.key).cloned())
            .collect()
    }

    /// Downloads the source `key` names (`resource_key`) now and waits for
    /// the attempt; the answer is the manager's: `Ok` when it brought a new
    /// version, the body that is held, or "not modified", otherwise the reason.
    /// `None` when there is no such source.
    pub async fn update(&self, key: &str) -> Option<Result<(), String>> {
        self.manager.update_now(key).await
    }

    /// What the manager says of the source `key` names (`resource_key`).
    fn status_of(&self, key: &str) -> Option<ResourceStatus> {
        self.manager.statuses().into_iter().find(|s| s.key == key)
    }

    /// Why the manager has nothing of `url`, when it knows.
    fn failure_of(&self, url: &Url) -> Option<String> {
        self.status_of(&resource_key(&ResourceSource::Url(url.clone())))
            .and_then(|s| s.last_error)
    }
}

/// The version of a source that the generation naming it was built from, when
/// the manager is at `version` as the source is registered. An entry begins
/// at 1 with a copy and at 0 without, and the download that registering
/// starts can have published the next by the time the version is read. A
/// generation that read a copy (`found`) read the first; one that got no
/// text read none, whatever there is by now.
fn built_from(found: bool, version: u64) -> u64 {
    if found { version.min(1) } else { 0 }
}

/// When the update of a profile fell due, if it has by `now`: an `interval`
/// after the last successful check, or after `local`, the time the local file
/// was last changed, when there was none. A time in the future is the clock
/// having been set back, and nothing is due by it. An interval that cannot be
/// added to a time is never over, however many seconds a `u64` holds.
fn due_since(
    interval: Duration,
    checked: Option<SystemTime>,
    local: Option<SystemTime>,
    now: SystemTime,
) -> Option<SystemTime> {
    let from = checked.or(local)?;
    if from > now {
        return None;
    }
    let due = from.checked_add(interval)?;
    (due <= now).then_some(due)
}

/// Whether a downloaded body is taken: a profile is text.
fn accepts(body: &[u8]) -> Result<(), String> {
    std::str::from_utf8(body)
        .map(|_| ())
        .map_err(|_| NOT_UTF8.to_string())
}

/// A number of seconds as the manager has it: what an `i64` cannot hold is
/// as long as can be.
fn seconds(interval: Duration) -> i64 {
    i64::try_from(interval.as_secs()).unwrap_or(i64::MAX)
}

/// When the local file was last changed, when that can be told.
fn file_modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// What `url` was downloaded as, as the loader wants it.
fn cached_text(root: &Path, url: &Url) -> Remote {
    match cached_copy(root, url) {
        None => Remote::Missing { why: None },
        Some(CachedCopy {
            data,
            path,
            fetched_at,
        }) => match String::from_utf8(data.to_vec()) {
            Ok(text) => Remote::Text {
                text,
                path,
                fetched_at,
            },
            Err(_) => Remote::Missing {
                why: Some(NOT_UTF8.to_string()),
            },
        },
    }
}

struct DiskRemote {
    root: PathBuf,
}

impl RemoteSource for DiskRemote {
    fn get(&self, url: &Url) -> Remote {
        cached_text(&self.root, url)
    }
}

/// The downloaded text the disk holds, for whoever has no manager (`check`,
/// the offline commands).
pub fn disk_remote(data_dir: &Path) -> Arc<dyn RemoteSource> {
    Arc::new(DiskRemote {
        root: data_dir.to_path_buf(),
    })
}

struct TrackedRemote {
    root: PathBuf,
    sources: Weak<ProfileSources>,
}

impl RemoteSource for TrackedRemote {
    fn get(&self, url: &Url) -> Remote {
        match cached_text(&self.root, url) {
            Remote::Missing { why: None } => Remote::Missing {
                why: self.sources.upgrade().and_then(|s| s.failure_of(url)),
            },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::testing::seed_cache;

    fn url(path: &str) -> Url {
        Url::parse(&format!("https://sources.example/{path}")).unwrap()
    }

    fn key(path: &str) -> String {
        resource_key(&ResourceSource::Url(url(path)))
    }

    /// What a load came across and read the copy of: each file's path, role
    /// and interval in seconds.
    fn info(remote: &[(&str, RemoteRole, u64)]) -> SourceInfo {
        SourceInfo {
            main: PathBuf::from("main.conf"),
            includes: Vec::new(),
            main_text: PathBuf::from("main.conf"),
            remote: remote
                .iter()
                .map(|&(path, role, seconds)| RemoteRef {
                    url: url(path),
                    role,
                    path: Some(PathBuf::from("copy")),
                    interval: Duration::from_secs(seconds),
                    strict: false,
                    lacks_section: false,
                })
                .collect(),
        }
    }

    /// Sources that go nowhere: what moves is what is adopted.
    fn offline(dir: &Path) -> Arc<ProfileSources> {
        ProfileSources::new(SourcesOptions {
            profile: dir.join("main.conf"),
            load: LoadOptions::for_tests(),
            data_dir: dir.join("data"),
            offline: true,
            client: crate::stack::resource_client().unwrap(),
            resources: ResourceOptions::default(),
        })
    }

    /// `path` was downloaded an hour ago, as `body`.
    fn cached(dir: &Path, path: &str, body: &str) {
        seed_cache(
            &dir.join("data"),
            &url(path),
            body.as_bytes(),
            SystemTime::now() - Duration::from_secs(3600),
        );
    }

    /// `path` is taken as `body`, which is a new version when it differs.
    fn adopt(sources: &ProfileSources, path: &str, body: &str) {
        sources
            .manager
            .adopt(
                &ResourceSource::Url(url(path)),
                body.as_bytes().to_vec().into(),
            )
            .unwrap();
    }

    /// Lets the tasks that follow the sources run.
    async fn follow_up() {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// A report comes within `window` (of the clock the test pauses).
    async fn reported_within(sources: &ProfileSources, window: Duration) {
        tokio::time::timeout(window, sources.changed())
            .await
            .expect("a change is reported");
    }

    fn between(waited: Duration, least: Duration, most: Duration) {
        assert!(waited >= least && waited < most, "{waited:?}");
    }

    const SECOND: Duration = Duration::from_secs(1);

    /// The quiet second is counted from the last move.
    #[tokio::test(start_paused = true)]
    async fn the_report_comes_a_second_after_the_last_move() {
        let dir = tempfile::tempdir().unwrap();
        cached(dir.path(), "a", "one");
        let sources = offline(dir.path());
        sources.track(&info(&[("a", RemoteRole::Include, 3600)]));
        let begun = Instant::now();
        adopt(&sources, "a", "two");
        follow_up().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        adopt(&sources, "a", "three");
        reported_within(&sources, 5 * SECOND).await;
        between(
            begun.elapsed(),
            Duration::from_millis(1500),
            Duration::from_millis(1600),
        );
        // and that was one report
        let more = tokio::time::timeout(Duration::from_secs(10), sources.changed()).await;
        assert!(more.is_err());
    }

    /// The call is dropped and made again every 200 ms, as the main loop does.
    #[tokio::test(start_paused = true)]
    async fn a_call_dropped_every_200_milliseconds_is_reported_a_second_after_the_move() {
        let dir = tempfile::tempdir().unwrap();
        cached(dir.path(), "a", "one");
        let sources = offline(dir.path());
        sources.track(&info(&[("a", RemoteRole::Include, 3600)]));
        let begun = Instant::now();
        adopt(&sources, "a", "two");
        follow_up().await;
        while tokio::time::timeout(Duration::from_millis(200), sources.changed())
            .await
            .is_err()
        {
            assert!(begun.elapsed() < 3 * SECOND);
        }
        between(begun.elapsed(), SECOND, Duration::from_millis(1100));
    }

    /// A version that comes when nobody calls is told to the next call, which
    /// has nothing more to wait for if the second is over.
    #[tokio::test(start_paused = true)]
    async fn a_move_while_nobody_waits_is_reported_by_the_next_call() {
        let dir = tempfile::tempdir().unwrap();
        cached(dir.path(), "a", "one");
        let sources = offline(dir.path());
        sources.track(&info(&[("a", RemoteRole::Include, 3600)]));
        adopt(&sources, "a", "two");
        reported_within(&sources, 5 * SECOND).await;
        adopt(&sources, "a", "three");
        follow_up().await;
        tokio::time::sleep(5 * SECOND).await;
        let begun = Instant::now();
        reported_within(&sources, 5 * SECOND).await;
        assert!(begun.elapsed() < Duration::from_millis(10));
    }

    /// Registering a source again is no move, and a move that nobody was told
    /// of is not lost by it.
    #[tokio::test(start_paused = true)]
    async fn a_source_registered_again_is_no_move_and_loses_none() {
        let dir = tempfile::tempdir().unwrap();
        cached(dir.path(), "a", "one");
        cached(dir.path(), "b", "one");
        let sources = offline(dir.path());
        sources.track(&info(&[
            ("a", RemoteRole::Include, 3600),
            ("b", RemoteRole::Include, 3600),
        ]));
        sources.track(&info(&[
            ("a", RemoteRole::Include, 7200),
            ("b", RemoteRole::Include, 3600),
        ]));
        let quiet = tokio::time::timeout(Duration::from_secs(10), sources.changed()).await;
        assert!(quiet.is_err());
        adopt(&sources, "a", "two");
        follow_up().await;
        let begun = Instant::now();
        sources.track(&info(&[
            ("a", RemoteRole::Include, 3600),
            ("b", RemoteRole::Include, 3600),
        ]));
        reported_within(&sources, 5 * SECOND).await;
        between(begun.elapsed(), Duration::from_millis(900), 2 * SECOND);
    }

    /// A source that is registered while a call waits is part of what it waits
    /// for.
    #[tokio::test(start_paused = true)]
    async fn a_source_registered_while_a_call_waits_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        cached(dir.path(), "a", "one");
        cached(dir.path(), "b", "one");
        let sources = offline(dir.path());
        sources.track(&info(&[("a", RemoteRole::Include, 3600)]));
        let waiting = tokio::spawn({
            let sources = sources.clone();
            async move { sources.changed().await }
        });
        follow_up().await;
        sources.track(&info(&[
            ("a", RemoteRole::Include, 3600),
            ("b", RemoteRole::Include, 3600),
        ]));
        adopt(&sources, "b", "two");
        let reported = tokio::time::timeout(5 * SECOND, waiting).await;
        assert!(reported.is_ok());
    }

    /// What a generation was built from is what it read. A source it did not
    /// get the text of has not been reported to it, whatever the manager holds
    /// by the time the source is registered: a download can finish between
    /// the load and the registration. A call that is already waiting is told.
    #[tokio::test(start_paused = true)]
    async fn a_copy_the_generation_did_not_read_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        cached(dir.path(), "a", "one");
        let sources = offline(dir.path());
        let waiting = tokio::spawn({
            let sources = sources.clone();
            async move { sources.changed().await }
        });
        follow_up().await;
        // the load found nothing at the address; the copy came after it
        let mut loaded = info(&[("a", RemoteRole::Include, 3600)]);
        loaded.remote[0].path = None;
        sources.track(&loaded);
        let reported = tokio::time::timeout(5 * SECOND, waiting).await;
        assert!(reported.is_ok(), "the call that waits is told");
        // once: the generation after it read the copy
        sources.track(&info(&[("a", RemoteRole::Include, 3600)]));
        let more = tokio::time::timeout(Duration::from_secs(10), sources.changed()).await;
        assert!(more.is_err());
    }

    /// A version published a moment before its source is registered again,
    /// which the task following the source has not told of yet, is not lost
    /// with the registration.
    #[tokio::test(start_paused = true)]
    async fn a_move_nobody_has_seen_yet_is_not_lost_when_its_source_is_registered_again() {
        let dir = tempfile::tempdir().unwrap();
        cached(dir.path(), "a", "one");
        let sources = offline(dir.path());
        sources.track(&info(&[("a", RemoteRole::Include, 3600)]));
        // nothing is awaited between the two: no task has run
        adopt(&sources, "a", "two");
        sources.track(&info(&[("a", RemoteRole::Include, 7200)]));
        reported_within(&sources, 5 * SECOND).await;
        let more = tokio::time::timeout(Duration::from_secs(10), sources.changed()).await;
        assert!(more.is_err());
    }

    /// The local file keeps its precedence while it is newer than the last
    /// check. At the very same time the copy wins, as it does for the loader.
    #[tokio::test(start_paused = true)]
    async fn a_check_at_the_time_of_the_local_file_ends_its_precedence() {
        let dir = tempfile::tempdir().unwrap();
        // a whole second, which every file system and the cache both keep
        let since_epoch = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        let checked = SystemTime::UNIX_EPOCH + Duration::from_secs(since_epoch.as_secs() - 3600);
        seed_cache(&dir.path().join("data"), &url("m"), b"one", checked);
        let local = dir.path().join("main.conf");
        std::fs::write(&local, "local").unwrap();
        let changed_at = |at: SystemTime| {
            std::fs::File::options()
                .write(true)
                .open(&local)
                .unwrap()
                .set_modified(at)
                .unwrap();
        };
        let loaded = info(&[("m", RemoteRole::Managed, 86400)]);
        let sources = offline(dir.path());

        changed_at(checked + SECOND);
        sources.track(&loaded);
        assert!(!sources.outdated(), "the file is newer than the check");

        changed_at(checked);
        sources.track(&loaded);
        assert!(sources.outdated(), "the same time");
        assert!(!sources.outdated(), "once for a check");
    }

    /// The statuses are in the order of the generation, which is not the order
    /// the manager lists them in; a source whose role changed is labelled for
    /// the one it has now.
    #[tokio::test(start_paused = true)]
    async fn the_statuses_follow_the_generation() {
        let dir = tempfile::tempdir().unwrap();
        for path in ["a", "b", "c"] {
            cached(dir.path(), path, "one");
        }
        let sources = offline(dir.path());
        sources.track(&info(&[
            ("b", RemoteRole::Managed, 3600),
            ("a", RemoteRole::Include, 3600),
            ("c", RemoteRole::Include, 3600),
        ]));
        let keys = |sources: &ProfileSources| -> Vec<String> {
            sources.statuses().into_iter().map(|s| s.key).collect()
        };
        assert_eq!(keys(&sources), [key("b"), key("a"), key("c")]);
        let label = |sources: &ProfileSources, at: usize| sources.statuses()[at].label.clone();
        assert_eq!(
            label(&sources, 0).as_deref(),
            Some("managed profile (https://sources.example/…)")
        );
        assert_eq!(
            label(&sources, 1),
            Some(format!(
                "remote include {} (https://sources.example/…)",
                &key("a")[..8]
            ))
        );
        sources.track(&info(&[
            ("b", RemoteRole::Include, 3600),
            ("a", RemoteRole::Include, 3600),
        ]));
        assert_eq!(keys(&sources), [key("b"), key("a")]);
        assert_eq!(
            label(&sources, 0),
            Some(format!(
                "remote include {} (https://sources.example/…)",
                &key("b")[..8]
            ))
        );
        assert_eq!(sources.statuses()[0].kind, ResourceKind::Include);
    }

    /// A source is also tracked from a thread of the blocking pool.
    #[tokio::test(start_paused = true)]
    async fn sources_are_followed_when_tracked_from_a_blocking_thread() {
        let dir = tempfile::tempdir().unwrap();
        cached(dir.path(), "a", "one");
        let sources = offline(dir.path());
        tokio::task::spawn_blocking({
            let sources = sources.clone();
            move || sources.track(&info(&[("a", RemoteRole::Include, 3600)]))
        })
        .await
        .unwrap();
        adopt(&sources, "a", "two");
        let reported = tokio::time::timeout(5 * SECOND, sources.changed()).await;
        assert!(reported.is_ok());
    }

    #[test]
    fn an_interval_is_due_when_it_is_over() {
        let at = |seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        let (minute, hour) = (Duration::from_secs(60), Duration::from_secs(3600));
        // the last check, an interval ago or longer: due since the interval was over
        assert_eq!(
            due_since(minute, Some(at(1000)), None, at(5000)),
            Some(at(1060))
        );
        assert_eq!(
            due_since(minute, Some(at(1000)), None, at(1060)),
            Some(at(1060))
        );
        assert_eq!(due_since(minute, Some(at(1000)), None, at(1059)), None);
        // the local file's time stands in when there was no check, and only then
        assert_eq!(
            due_since(hour, None, Some(at(1000)), at(9000)),
            Some(at(4600))
        );
        assert_eq!(
            due_since(hour, Some(at(8000)), Some(at(1000)), at(9000)),
            None
        );
        assert_eq!(due_since(hour, None, None, at(9000)), None);
        // a time in the future is the clock having been set back
        assert_eq!(
            due_since(Duration::ZERO, Some(at(9001)), None, at(9000)),
            None
        );
        assert_eq!(
            due_since(Duration::ZERO, None, Some(at(9001)), at(9000)),
            None
        );
        // no time can be added the longest interval, and it is not due
        let longest = Duration::from_secs(u64::MAX);
        assert_eq!(due_since(longest, Some(at(1000)), None, at(9000)), None);
        assert_eq!(due_since(longest, None, Some(at(1000)), at(9000)), None);
    }

    #[test]
    fn a_registration_starts_from_what_its_generation_read() {
        // no text: nothing was read, whatever there is by now
        assert_eq!(built_from(false, 0), 0);
        assert_eq!(built_from(false, 1), 0);
        // a copy: the first version, also when the next is there already
        assert_eq!(built_from(true, 1), 1);
        assert_eq!(built_from(true, 2), 1);
        // a copy the manager did not take
        assert_eq!(built_from(true, 0), 0);
    }

    #[test]
    fn an_interval_is_as_long_as_the_manager_can_hold() {
        assert_eq!(seconds(Duration::from_secs(60)), 60);
        assert_eq!(seconds(Duration::from_secs(i64::MAX as u64)), i64::MAX);
        assert_eq!(seconds(Duration::from_secs(u64::MAX)), i64::MAX);
    }
}
