//! The start: the sources a profile names are downloaded before the first
//! generation is built, so that it starts with what it would have had if it had
//! been running.

use super::{ProfileSources, due_since, file_modified};
use crate::outbounds::load_checked;
use rurge_config::config::{LoadError, Loaded};
use rurge_config::{RemoteRole, Severity};
use rurge_net::resource::cached_copy;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::task::JoinSet;
use tokio::time::Instant;

/// What the start does about the sources of the profile that was loaded first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boot {
    /// Nothing: the profile is what the disk has.
    No,
    /// A source has no content, or a `strict` update is due: wait for the
    /// first attempts.
    Wait,
    /// A downloaded file does not load: every source is updated once, in the
    /// hope of a version that does.
    Refresh,
}

/// Whether the first load of the profile at the start is to be followed by a
/// round of downloads. `loaded` is that load, from the disk, and `data_dir` is
/// where the downloads are kept.
///
/// An error in a file that was downloaded is the first reason: an update may
/// bring a version that loads, and nothing else will, since no process is
/// running yet that could refresh it. An error that is all in the local file
/// is for the user to mend and no update replaces it.
pub fn boot_need(loaded: &Loaded, data_dir: &Path) -> Boot {
    let source = &loaded.config.source;
    if source.remote.is_empty() {
        return Boot::No;
    }
    let downloaded = |file: &Path| {
        source
            .remote
            .iter()
            .any(|r| r.path.as_deref() == Some(file))
    };
    if loaded.diagnostics.iter().any(|d| {
        d.severity == Severity::Error
            && d.span
                .as_ref()
                .is_some_and(|span| downloaded(span.file.as_ref()))
    }) {
        return Boot::Refresh;
    }
    let local = file_modified(&source.main);
    let now = SystemTime::now();
    let waits = source.remote.iter().any(|r| match r.role {
        RemoteRole::ManagedHop => false,
        _ if !r.found() => true,
        RemoteRole::Managed => {
            let checked = cached_copy(data_dir, &r.url).map(|copy| copy.fetched_at);
            r.strict && due_since(r.interval, checked, local, now).is_some()
        }
        RemoteRole::Include => false,
    });
    if waits { Boot::Wait } else { Boot::No }
}

/// How long the start may wait: for a round, for all of them, and how many
/// rounds there may be.
#[derive(Clone, Copy, Debug)]
pub struct BootLimits {
    pub round: Duration,
    pub total: Duration,
    pub rounds: usize,
}

impl Default for BootLimits {
    fn default() -> Self {
        BootLimits {
            round: Duration::from_secs(30),
            total: Duration::from_secs(60),
            rounds: 8,
        }
    }
}

impl ProfileSources {
    /// Downloads what the profile of `first` leads to, as far as `need` says
    /// and `limits` allow, and gives the profile as it is then. A round
    /// follows the sources of the load in hand, waits for them (`Refresh` asks
    /// every one of them for an update in the first round) and loads again
    /// from what was downloaded. The load of a round is the answer when it has
    /// no error and names no source that is not followed; or when the round
    /// changed nothing, since another would wait for the same; or when the
    /// rounds or the time are used up. It can carry errors: the caller decides
    /// what they mean. `Err` is a profile that cannot be read at all.
    pub async fn bootstrap(
        self: &Arc<Self>,
        first: Loaded,
        need: Boot,
        limits: BootLimits,
    ) -> Result<Loaded, LoadError> {
        if need == Boot::No {
            return Ok(first);
        }
        let begun = Instant::now();
        let mut loaded = first;
        for round in 0..limits.rounds {
            // no round waits longer than what is left of the whole
            let left = limits.total.saturating_sub(begun.elapsed());
            if left.is_zero() {
                break;
            }
            let wait = limits.round.min(left);
            self.track(&loaded.config.source);
            let before = self.progress();
            if round == 0 && need == Boot::Refresh {
                self.update_all(wait).await;
            } else {
                self.settle(wait).await;
            }
            loaded = self.load_again().await?;
            let untracked = self.names_untracked(&loaded);
            let unchanged = self.progress() == before;
            if !untracked && (!loaded.diagnostics.has_errors() || unchanged) {
                return Ok(loaded);
            }
        }
        Ok(loaded)
    }

    /// Asks every source that is followed for an update, all at once, and
    /// waits for the answers for `limit` at the most.
    async fn update_all(self: &Arc<Self>, limit: Duration) {
        let keys: Vec<String> = self
            .core
            .lock()
            .tracked
            .iter()
            .map(|t| t.key.clone())
            .collect();
        let mut updates = JoinSet::new();
        for key in keys {
            let sources = self.clone();
            updates.spawn(async move { sources.update(&key).await });
        }
        // what is still under way when the time is up is let go of
        let _ = tokio::time::timeout(limit, async {
            while updates.join_next().await.is_some() {}
        })
        .await;
    }

    /// The profile loaded again from what there is on disk by now, on the
    /// blocking pool.
    async fn load_again(self: &Arc<Self>) -> Result<Loaded, LoadError> {
        let profile = self.profile.clone();
        let mut opts = self.load.clone();
        opts.remote = Some(self.remote());
        tokio::task::spawn_blocking(move || load_checked(&profile, &opts))
            .await
            .unwrap_or_else(|error| std::panic::resume_unwind(error.into_panic()))
    }

    /// Where the sources are: each one's version, by its key. A round moved
    /// something when this is not what it was, whether a version changed or a
    /// source came or went.
    fn progress(&self) -> Vec<(String, u64)> {
        self.manager
            .statuses()
            .into_iter()
            .map(|status| (status.key, status.version))
            .collect()
    }

    /// Whether `loaded` names a source that is not followed, and so has not
    /// been asked for yet.
    fn names_untracked(&self, loaded: &Loaded) -> bool {
        let state = self.core.lock();
        loaded
            .config
            .source
            .remote
            .iter()
            .filter(|r| r.role != RemoteRole::ManagedHop)
            .any(|r| !state.tracked.iter().any(|t| t.url == r.url))
    }
}
