//! The verdict on a body the manager has downloaded, before it takes it: does
//! the whole profile still load with it?
//!
//! Sources that depend on one another do not change at the same moment. A
//! body that does not load by itself may be one half of a pair, so it is kept
//! in memory and judged again together with the next body that arrives; when
//! the group loads, all of it is taken. A version that refers to a source that
//! has not been downloaded yet is kept until that source has been tried, so
//! that the profile never changes to a version with a piece missing.
//!
//! Every verdict is made under one lock, held across the load (`Verdicts`),
//! and only on a thread of the blocking pool. The order of the locks is that
//! one, then the state of the sources, then the locks of the manager: nothing
//! takes the verdict lock while it holds either of the others.

use super::{NOT_UTF8, ProfileSources, cached_text};
use crate::outbounds::load_checked;
use rurge_config::{Remote, RemoteRef, RemoteRole, RemoteSource, Severity};
use rurge_net::resource::{ResourceManager, ResourceSource, cache_file, cached_copy, resource_key};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;
use tokio::time::Instant;
use url::Url;

/// Why a version is not taken while a source it refers to has not been tried.
const WAITING: &str = "waiting for the sources this version refers to";

/// Why a body is not taken when the profile does not name its address.
const NOT_REFERRED_TO: &str = "the profile no longer refers to this address";

/// Why an included body is not taken when it is not what is included.
const NO_SECTION: &str = "lacks the section it is included into";

/// A body that was downloaded, as the text it is.
#[derive(Clone)]
struct Candidate {
    url: Url,
    text: Arc<str>,
}

/// A candidate that was not taken, judged again with the next one that
/// arrives. It is in memory only: nothing of it is written before it is
/// taken.
struct Kept {
    candidate: Candidate,
    /// The last successful check of its source when it was kept. The source
    /// answering "not modified", or sending the body it holds, since then has
    /// moved that: the server no longer gives this body.
    fetched_at: Option<SystemTime>,
    /// It was downloaded and waits for sources of the profile that have not
    /// been tried, and has no fault of its own.
    waiting: bool,
}

/// What the verdicts remember. The lock around it is the verdict lock.
///
/// Known limits, none of them solved here:
///
/// - A body that was let through, whose write to the cache then failed, with
///   the next attempt of its source already under way and no verdict in
///   between: a verdict for another source in that window still sees the body
///   although the disk does not hold it. The worst outcome is a pair on disk
///   that was never judged together, which a reload refuses, and the running
///   generation stays, until one of the two changes again.
/// - A source retired while its own check runs: the verdict may have taken
///   the others of a group, and the body in hand is never written. The same
///   outcome, until the source is registered again and downloaded.
/// - A source registered for a version whose candidate was dropped without a
///   verdict (the server went back to the old version), and whose own
///   downloads keep failing, stays registered until the next verdict or the
///   next `track` with nothing kept.
#[derive(Default)]
pub(super) struct Verdicts {
    kept: Vec<Kept>,
    /// Bodies a check let through. The manager writes one only after the
    /// check has returned, and a verdict that begins in between has to find it
    /// all the same.
    accepted: Vec<Candidate>,
}

/// What one load of the profile with some candidates comes to.
enum Conclusion {
    /// Not taken, and the reason why.
    Refused(String),
    /// It loads, but names sources that have not been tried.
    Waiting,
    Passed,
}

/// What the loads of one verdict have in common.
struct Run {
    /// The time the candidates are taken to have been downloaded at: after
    /// they are taken, that is what it is.
    now: SystemTime,
    /// The addresses the loads named, of any role.
    named: HashSet<Url>,
    /// A load read the profile.
    read: bool,
}

impl Run {
    fn new() -> Run {
        Run {
            now: SystemTime::now(),
            named: HashSet::new(),
            read: false,
        }
    }
}

/// What the loader reads downloaded text from during a verdict: the bodies
/// under judgement, and those let through that the disk does not hold yet,
/// stand for what the disk would have after they are taken; everything else is
/// what the disk has.
struct Overlay {
    manager: Arc<ResourceManager>,
    bodies: HashMap<Url, Arc<str>>,
    now: SystemTime,
    /// The addresses the loader asked for that had no content and whose
    /// source had not been tried.
    unattempted: Arc<Mutex<HashSet<Url>>>,
}

impl RemoteSource for Overlay {
    fn get(&self, url: &Url) -> Remote {
        let root = self.manager.root();
        if let Some(text) = self.bodies.get(url) {
            return Remote::Text {
                text: text.to_string(),
                path: cache_file(root, url),
                fetched_at: self.now,
            };
        }
        // The status is read before the disk. A download that lands in
        // between is on the disk when it is read; one that fails in between
        // is waited for a little too long. Read after it, a download that
        // landed would make the line that got no text pass for tried.
        let key = key_of(url);
        let status = self
            .manager
            .statuses()
            .into_iter()
            .find(|status| status.key == key);
        match cached_text(root, url) {
            Remote::Missing { why } => {
                if status
                    .as_ref()
                    .is_none_or(|status| status.state == "missing")
                {
                    self.unattempted
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(url.clone());
                }
                Remote::Missing {
                    why: why.or_else(|| status.and_then(|status| status.last_error)),
                }
            }
            text => text,
        }
    }
}

fn key_of(url: &Url) -> String {
    resource_key(&ResourceSource::Url(url.clone()))
}

impl ProfileSources {
    /// Decides whether the manager takes `body`, the new content of `url`.
    /// Called by the manager on a thread of the blocking pool, under none of
    /// its locks. `Err` is the reason it is not taken, and the resource's
    /// `last_error`.
    pub(super) fn check(&self, url: &Url, body: &[u8]) -> Result<(), String> {
        let mut verdicts = self.verdicts.lock().unwrap_or_else(PoisonError::into_inner);
        let outcome = self.decide(&mut verdicts, url, body);
        self.mirror(&verdicts);
        outcome
    }

    fn decide(&self, v: &mut Verdicts, url: &Url, body: &[u8]) -> Result<(), String> {
        let Ok(text) = std::str::from_utf8(body) else {
            // what the server gives now is not what was kept for it
            v.kept.retain(|k| k.candidate.url != *url);
            return Err(NOT_UTF8.to_string());
        };
        let candidate = Candidate {
            url: url.clone(),
            text: Arc::from(text),
        };
        self.purge(v);
        let mut run = Run::new();
        let mut waiting = false;
        // The other candidates that are kept belong with this one, if they
        // load together.
        let others: Vec<Candidate> = v
            .kept
            .iter()
            .filter(|k| k.candidate.url != *url)
            .map(|k| k.candidate.clone())
            .collect();
        if !others.is_empty() {
            let group: Vec<&Candidate> = others.iter().chain([&candidate]).collect();
            match self.judge(&v.accepted, &group, &mut run) {
                Conclusion::Passed => return self.take_together(v, &others, candidate, &run),
                Conclusion::Waiting => waiting = true,
                Conclusion::Refused(_) => {}
            }
        }
        let refused = match self.judge(&v.accepted, &[&candidate], &mut run) {
            Conclusion::Passed => {
                v.kept.retain(|k| k.candidate.url != *url);
                accept(v, candidate);
                self.release(v, &run);
                return Ok(());
            }
            Conclusion::Waiting => {
                waiting = true;
                None
            }
            Conclusion::Refused(reason) => Some(reason),
        };
        // A candidate that is part of a group that waits is not at fault, and
        // `strict` is not to count the wait as a failure.
        let reason = refused
            .filter(|_| !waiting)
            .unwrap_or_else(|| WAITING.to_string());
        v.kept.retain(|k| k.candidate.url != *url);
        self.keep(v, candidate, waiting);
        self.release(v, &run);
        Err(reason)
    }

    /// The kept candidates and `candidate` load together: the kept ones are
    /// taken now, and `candidate` is let through for the manager to write.
    fn take_together(
        &self,
        v: &mut Verdicts,
        others: &[Candidate],
        candidate: Candidate,
        run: &Run,
    ) -> Result<(), String> {
        let (taken, failure) = self.adopt_all(others);
        if let Some(error) = failure {
            // What was taken is on disk now. The rest and this body stay
            // kept for the next verdict.
            v.kept.retain(|k| {
                k.candidate.url != candidate.url
                    && !others[..taken]
                        .iter()
                        .any(|taken| taken.url == k.candidate.url)
            });
            self.keep(v, candidate, false);
            return Err(format!("cannot write the cache: {error}"));
        }
        v.kept.clear();
        accept(v, candidate);
        self.release(v, run);
        Ok(())
    }

    /// Keeps `candidate`, the newest of its address. A source that is not
    /// registered any more has nothing to supersede it, and nothing to wait
    /// for.
    fn keep(&self, v: &mut Verdicts, candidate: Candidate, waiting: bool) {
        if let Some(status) = self.status_of(&key_of(&candidate.url)) {
            v.kept.push(Kept {
                candidate,
                fetched_at: status.fetched_at,
                waiting,
            });
        }
    }

    /// Hands `candidates` to the manager as if they had just been downloaded:
    /// how many it took, and the error that stopped it.
    fn adopt_all(&self, candidates: &[Candidate]) -> (usize, Option<io::Error>) {
        for (taken, candidate) in candidates.iter().enumerate() {
            let body = candidate.text.as_bytes().to_vec().into();
            let source = ResourceSource::Url(candidate.url.clone());
            if let Err(error) = self.manager.adopt(&source, body) {
                return (taken, Some(error));
            }
        }
        (candidates.len(), None)
    }

    /// Lets go of what is no longer the truth. A kept candidate whose source
    /// is gone, or was confirmed since by the server, is not what the server
    /// gives. A body that was let through is dropped once the disk holds it,
    /// and when its source is not downloading any more: then the disk is what
    /// there is.
    fn purge(&self, v: &mut Verdicts) {
        let statuses = self.manager.statuses();
        let status_of = |url: &Url| {
            let key = key_of(url);
            statuses.iter().find(|status| status.key == key)
        };
        v.kept.retain(|k| {
            status_of(&k.candidate.url).is_some_and(|status| status.fetched_at == k.fetched_at)
        });
        let root = self.manager.root();
        v.accepted.retain(|a| {
            status_of(&a.url).is_some_and(|status| status.updating)
                && cached_copy(root, &a.url).is_none_or(|copy| copy.data[..] != *a.text.as_bytes())
        });
    }

    /// What no load of this verdict named is let go of: a candidate nothing
    /// refers to is not kept, and when none is kept the sources that were
    /// registered for a version are dropped unless a load named them. Without
    /// this a source that a withdrawn version named would be downloaded for
    /// ever. Nothing is let go of when no load read the profile: nothing is
    /// known then.
    fn release(&self, v: &mut Verdicts, run: &Run) {
        if !run.read {
            return;
        }
        v.kept.retain(|k| run.named.contains(&k.candidate.url));
        if !v.kept.is_empty() {
            return;
        }
        let mut state = self.core.lock();
        for source in std::mem::take(&mut state.tracked) {
            if source.provisional && !run.named.contains(&source.url) {
                self.retire(&source);
            } else {
                state.tracked.push(source);
            }
        }
    }

    /// Tells the state what is kept, for whoever must not wait for a verdict.
    fn mirror(&self, v: &Verdicts) {
        let kept = v
            .kept
            .iter()
            .map(|k| (k.candidate.url.clone(), k.waiting))
            .collect();
        self.core.lock().kept = kept;
    }

    /// Loads the local profile with `group` as the content of its addresses
    /// (and the bodies that were let through but are not on disk yet), and
    /// says what that comes to.
    fn judge(&self, accepted: &[Candidate], group: &[&Candidate], run: &mut Run) -> Conclusion {
        let unattempted = Arc::new(Mutex::new(HashSet::new()));
        let mut bodies: HashMap<Url, Arc<str>> = accepted
            .iter()
            .map(|a| (a.url.clone(), a.text.clone()))
            .collect();
        bodies.extend(group.iter().map(|c| (c.url.clone(), c.text.clone())));
        let mut opts = self.load.clone();
        opts.remote = Some(Arc::new(Overlay {
            manager: self.manager.clone(),
            bodies,
            now: run.now,
            unattempted: unattempted.clone(),
        }));
        let loaded = match load_checked(&self.profile, &opts) {
            Ok(loaded) => loaded,
            Err(error) => return Conclusion::Refused(format!("does not load: {error}")),
        };
        run.read = true;
        let found = &loaded.config.source.remote;
        run.named.extend(found.iter().map(|r| r.url.clone()));

        if let Some(error) = loaded
            .diagnostics
            .iter()
            .find(|d| d.severity == Severity::Error)
        {
            let mut reason = format!("does not load: {}", error.message);
            // never the path of the file: the line, when it is in one of the
            // candidates
            if let Some(span) = &error.span
                && group.iter().any(|c| {
                    cache_file(self.manager.root(), &c.url).as_path() == span.file.as_ref()
                })
            {
                let _ = write!(reason, " (line {})", span.line);
            }
            return Conclusion::Refused(reason);
        }
        if group.iter().any(|c| !found.iter().any(|r| r.url == c.url)) {
            return Conclusion::Refused(NOT_REFERRED_TO.to_string());
        }
        if group
            .iter()
            .any(|c| found.iter().any(|r| r.url == c.url && r.lacks_section))
        {
            return Conclusion::Refused(NO_SECTION.to_string());
        }
        // A line that got no text, from a source that has not been tried, is
        // a line that is not expanded yet. The sources it names are
        // registered, if they are not, so that they are tried. A source that
        // was tried holds nothing back: its line is not expanded, as for any
        // load.
        let pending: Vec<&RemoteRef> = {
            let asked = unattempted.lock().unwrap_or_else(PoisonError::into_inner);
            found
                .iter()
                .filter(|r| {
                    r.role != RemoteRole::ManagedHop && !r.found() && asked.contains(&r.url)
                })
                .collect()
        };
        let mut waiting = false;
        for wanted in pending {
            waiting |= self.awaits_first_attempt(wanted);
        }
        if waiting {
            Conclusion::Waiting
        } else {
            Conclusion::Passed
        }
    }

    /// Whether the source `wanted` names is still to be tried the first
    /// time. A source nobody follows is registered, as one a verdict
    /// registered.
    fn awaits_first_attempt(&self, wanted: &RemoteRef) -> bool {
        let mut state = self.core.lock();
        if state.tracked.iter().any(|t| t.url == wanted.url) {
            // the load found it untried
            return true;
        }
        let id = state.next_id;
        state.next_id += 1;
        let mut source = self.register(id, wanted);
        source.provisional = true;
        let key = source.key.clone();
        state.tracked.push(source);
        // a copy on disk that is not text is nothing to wait for
        self.status_of(&key)
            .is_some_and(|status| status.state == "missing")
    }

    /// Whether any version is kept.
    pub(super) fn holds_versions(&self) -> bool {
        !self.core.lock().kept.is_empty()
    }

    /// Judges the versions that are kept as one group, on the blocking pool.
    /// Whether that was over by `deadline`.
    pub(super) async fn judge_versions_kept(self: &Arc<Self>, deadline: Instant) -> bool {
        let sources = self.clone();
        let judging = tokio::task::spawn_blocking(move || sources.judge_kept());
        // Past the deadline the judging goes on by itself; a panic in it is
        // nobody's to hear of here.
        tokio::time::timeout_at(deadline, judging).await.is_ok()
    }

    /// One verdict over everything that is kept: when it loads together, all
    /// of it is taken.
    fn judge_kept(&self) {
        let mut verdicts = self.verdicts.lock().unwrap_or_else(PoisonError::into_inner);
        let v = &mut *verdicts;
        self.purge(v);
        if !v.kept.is_empty() {
            let group: Vec<Candidate> = v.kept.iter().map(|k| k.candidate.clone()).collect();
            let refs: Vec<&Candidate> = group.iter().collect();
            let mut run = Run::new();
            if let Conclusion::Passed = self.judge(&v.accepted, &refs, &mut run) {
                let (taken, failure) = self.adopt_all(&group);
                v.kept = v.kept.split_off(taken);
                if let Some(error) = failure {
                    tracing::warn!(
                        error = %error,
                        "cannot write the cache while the downloaded versions are taken together"
                    );
                }
            }
            self.release(v, &run);
        }
        self.mirror(v);
    }
}

/// Remembers that `candidate` was let through, the newest of its address.
fn accept(v: &mut Verdicts, candidate: Candidate) {
    v.accepted.retain(|a| a.url != candidate.url);
    v.accepted.push(candidate);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{SourcesOptions, disk_remote};
    use rurge_config::config::{LoadOptions, load};
    use rurge_net::resource::ResourceOptions;
    use rurge_net::testing::{TestServer, seed_cache};
    use std::time::Duration;

    const HOUR: Duration = Duration::from_secs(3600);

    /// The policies `/a` defines.
    fn policies(names: &[&str]) -> String {
        let mut text = String::from("[Proxy]\n");
        for name in names {
            text += &format!("{name} = direct\n");
        }
        text
    }

    /// The rule of `/b`, which uses one of them.
    fn rules(policy: &str) -> String {
        format!("[Rule]\nDOMAIN,b.example,{policy}\n")
    }

    async fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// A profile that includes `/a`, the policies, and `/b`, the rules that
    /// use them; both were downloaded an hour ago.
    struct Fixture {
        dir: tempfile::TempDir,
        server: TestServer,
        sources: Arc<ProfileSources>,
    }

    impl Fixture {
        async fn new() -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let server = TestServer::spawn().await;
            let profile = dir.path().join("main.conf");
            std::fs::write(
                &profile,
                format!(
                    "[Proxy]\n#!include {}\n[Rule]\n#!include {}\nFINAL,DIRECT\n",
                    server.url("/a"),
                    server.url("/b")
                ),
            )
            .unwrap();
            let sources = ProfileSources::new(SourcesOptions {
                profile,
                load: LoadOptions::for_tests(),
                data_dir: dir.path().join("data"),
                offline: false,
                client: crate::stack::resource_client().unwrap(),
                resources: ResourceOptions {
                    fetch_timeout: Duration::from_secs(5),
                    min_backoff: Duration::from_millis(50),
                    max_backoff: Duration::from_millis(200),
                    ..ResourceOptions::default()
                },
            });
            let fixture = Fixture {
                dir,
                server,
                sources,
            };
            fixture.seed("/a", &policies(&["P1"]));
            fixture.seed("/b", &rules("P1"));
            fixture.track();
            fixture
        }

        fn data(&self) -> std::path::PathBuf {
            self.dir.path().join("data")
        }

        fn seed(&self, path: &str, body: &str) {
            seed_cache(
                &self.data(),
                &self.server.url(path),
                body.as_bytes(),
                SystemTime::now() - HOUR,
            );
        }

        /// Follows what the profile on disk names, as the main loop does.
        fn track(&self) {
            let mut opts = LoadOptions::for_tests();
            opts.remote = Some(disk_remote(&self.data()));
            let loaded = load(&self.sources.profile, &opts).unwrap();
            self.sources.track(&loaded.config.source);
        }

        /// What the disk holds for `path`.
        fn on_disk(&self, path: &str) -> String {
            let copy = cached_copy(&self.data(), &self.server.url(path)).expect("a copy");
            String::from_utf8(copy.data.to_vec()).unwrap()
        }

        /// A check, as the manager makes it: on a thread of the blocking pool.
        async fn check(&self, path: &str, body: &str) -> Result<(), String> {
            let sources = self.sources.clone();
            let (url, body) = (self.server.url(path), body.to_string());
            tokio::task::spawn_blocking(move || sources.check(&url, body.as_bytes()))
                .await
                .unwrap()
        }

        fn accepted(&self) -> Vec<Url> {
            let verdicts = self.sources.verdicts.lock().unwrap();
            verdicts.accepted.iter().map(|a| a.url.clone()).collect()
        }

        fn kept(&self) -> Vec<Url> {
            let verdicts = self.sources.verdicts.lock().unwrap();
            verdicts
                .kept
                .iter()
                .map(|k| k.candidate.url.clone())
                .collect()
        }

        fn key(&self, path: &str) -> String {
            key_of(&self.server.url(path))
        }
    }

    /// The manager writes a body only after its check has returned. A verdict
    /// that begins in between has to see the body as the disk is going to
    /// have it: `/b` needs the policy that the new `/a` defines.
    #[tokio::test]
    async fn a_body_that_is_let_through_counts_before_the_manager_has_written_it() {
        let f = Fixture::new().await;
        let (a2, b2) = (policies(&["P1", "P2"]), rules("P2"));
        // the download of `/a` is under way: its source is in an attempt
        f.server.set("/a", a2.clone());
        f.server.set_delay("/a", Duration::from_secs(5));
        let downloading = tokio::spawn({
            let sources = f.sources.clone();
            let key = f.key("/a");
            async move { sources.update(&key).await }
        });
        wait_for("the request", || f.server.hits("/a") == 1).await;

        assert_eq!(f.check("/a", &a2).await, Ok(()));
        // not on disk yet, and the verdict on `/b` is made now
        assert_eq!(f.on_disk("/a"), policies(&["P1"]));
        assert_eq!(f.check("/b", &b2).await, Ok(()));
        assert_eq!(f.accepted().len(), 2);
        downloading.abort();
    }

    /// Once the disk holds the body, the disk is what counts.
    #[tokio::test]
    async fn a_body_the_manager_has_written_is_let_go_of() {
        let f = Fixture::new().await;
        let (a2, b2) = (policies(&["P1", "P2"]), rules("P2"));
        f.server.set("/a", a2.clone());
        f.sources
            .update(&f.key("/a"))
            .await
            .expect("tracked")
            .expect("it loads alone");
        assert_eq!(f.on_disk("/a"), a2);
        assert_eq!(f.check("/b", &b2).await, Ok(()));
        // what the first check let through is not remembered once it is on disk
        assert_eq!(f.accepted(), [f.server.url("/b")]);
    }

    /// The manager records the failure of an attempt after its check has
    /// returned. A verdict that begins in between finds the candidate kept.
    #[tokio::test]
    async fn a_kept_candidate_is_found_before_the_manager_has_recorded_it() {
        let f = Fixture::new().await;
        // `/a` without `P1`, which `/b` still uses: they change together
        let (a2, b2) = (policies(&["P2"]), rules("P2"));
        assert!(f.check("/a", &a2).await.is_err());
        assert_eq!(f.kept(), [f.server.url("/a")]);
        // the manager has not recorded anything: `/a` is not `failed` yet
        assert_eq!(
            f.sources.status_of(&f.key("/a")).unwrap().state,
            "available"
        );
        assert_eq!(f.check("/b", &b2).await, Ok(()));
        assert_eq!(f.on_disk("/a"), a2);
        assert!(f.kept().is_empty());
        assert!(!f.sources.holds_versions());
    }

    /// The server confirmed the copy on disk since the candidate was kept:
    /// it is not what the server gives, and its partner does not find it.
    #[tokio::test]
    async fn a_kept_candidate_is_let_go_of_once_the_server_confirms_the_copy() {
        let f = Fixture::new().await;
        let (a2, b2) = (policies(&["P2"]), rules("P2"));
        assert!(f.check("/a", &a2).await.is_err());
        // the server gives the copy that is on disk: the check finds nothing new
        f.server.set("/a", policies(&["P1"]));
        f.sources
            .update(&f.key("/a"))
            .await
            .expect("tracked")
            .expect("not modified");
        assert!(f.check("/b", &b2).await.is_err());
        assert_eq!(f.on_disk("/a"), policies(&["P1"]));
        assert_eq!(f.kept(), [f.server.url("/b")]);
    }

    /// What the server gives now is not text: what was kept for it is not what
    /// it gives.
    #[tokio::test]
    async fn a_body_that_is_not_text_makes_the_kept_candidate_of_its_address_go() {
        let f = Fixture::new().await;
        assert!(f.check("/a", &policies(&["P2"])).await.is_err());
        assert!(f.sources.holds_versions());
        let sources = f.sources.clone();
        let url = f.server.url("/a");
        let outcome = tokio::task::spawn_blocking(move || sources.check(&url, &[0xff, 0xfe, 0x00]))
            .await
            .unwrap();
        assert_eq!(outcome, Err(NOT_UTF8.to_string()));
        assert!(f.kept().is_empty());
        assert!(!f.sources.holds_versions());
    }

    /// The source was retired by a `track` while its check waited for the
    /// verdict lock: nothing lasting is made of the verdict, and nothing is
    /// kept for a source that nobody follows.
    #[tokio::test]
    async fn a_check_that_runs_after_its_source_was_retired_leaves_nothing_kept() {
        let f = Fixture::new().await;
        let (a2, b2) = (policies(&["P2"]), rules("P2"));
        assert!(f.check("/b", &b2).await.is_err());
        // the profile stops naming `/a`
        std::fs::write(
            &f.sources.profile,
            format!("[Rule]\n#!include {}\nFINAL,DIRECT\n", f.server.url("/b")),
        )
        .unwrap();
        f.track();
        assert!(f.sources.status_of(&f.key("/a")).is_none());
        let late = f.check("/a", &a2).await;
        assert!(late.is_err(), "{late:?}");
        assert_eq!(f.kept(), [f.server.url("/b")]);
        assert!(f.accepted().is_empty());
    }

    /// The cache cannot be written for one of the kept candidates: what was
    /// taken stays taken, the rest and the body in hand are kept, and a later
    /// judging takes them once the disk is writable.
    #[tokio::test]
    async fn a_group_whose_cache_cannot_be_written_is_kept_for_the_next_verdict() {
        let f = Fixture::new().await;
        let (a2, b2) = (policies(&["P2"]), rules("P2"));
        assert!(f.check("/a", &a2).await.is_err());
        // a file where the directory of the cache of `/a` is to be
        let place = f.data().join("resources").join(f.key("/a"));
        std::fs::remove_dir_all(&place).unwrap();
        std::fs::write(&place, b"in the way").unwrap();
        let reason = f
            .check("/b", &b2)
            .await
            .expect_err("the group is not taken");
        assert!(reason.starts_with("cannot write the cache: "), "{reason}");
        assert_eq!(f.kept(), [f.server.url("/a"), f.server.url("/b")]);
        assert!(f.sources.holds_versions());

        std::fs::remove_file(&place).unwrap();
        let sources = f.sources.clone();
        tokio::task::spawn_blocking(move || sources.judge_kept())
            .await
            .unwrap();
        assert_eq!(f.on_disk("/a"), a2);
        assert_eq!(f.on_disk("/b"), b2);
        assert!(!f.sources.holds_versions());
    }
}
