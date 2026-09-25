//! The `policy-path` subscriptions of one config generation (phase 2 M3
//! design 5.1, 5.7, 5.9): registered with its resource manager, read into
//! snapshots for the assembly, watched so that an update rebuilds the
//! registry, and checked offline for `rurge check`.

use crate::engine::Engine;
use crate::runtime::Runtime;
use rurge_config::config::{LoadError, LoadOptions, Loaded};
use rurge_config::spec::PolicyPath;
use rurge_config::{Config, Diagnostics};
use rurge_net::resource::{ResourceHandle, ResourceManager, ResourceSource, ResourceSpec};
use rurge_policy::{PolicyRegistry, Snapshots, assemble, subscription};
use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Weak};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::watch;
use tokio_util::task::AbortOnDropHandle;

/// Updates that arrive within this long of each other make one rebuild.
pub const REBUILD_DEBOUNCE: Duration = Duration::from_secs(1);

pub(crate) struct Subscriptions {
    /// One per source, in the order the groups first name them.
    handles: Vec<(PolicyPath, ResourceHandle)>,
    /// For the engine's watcher, which takes them. Subscribed before the
    /// first snapshot is read, so no update in between goes unseen.
    receivers: Vec<watch::Receiver<u64>>,
}

/// One `policy-path` source, merged across every group that names it, before
/// any of them registers with the resource manager.
struct Source {
    path: PolicyPath,
    /// The first group that names it, for `get_labelled`.
    label: String,
    /// The smallest `update-interval` any group named it with; `None` when
    /// every one of them left it unset.
    interval: Option<i64>,
}

impl Subscriptions {
    /// Registers every `policy-path` with this generation's resource
    /// manager. What earlier runs cached is loaded right here, synchronously
    /// and without the network, so a start or a reload keeps the members it
    /// had (M3-D5). Log lines name a source after the first group that uses
    /// it, never by its URL (M3-D7).
    pub(crate) fn register(cfg: &Config, resources: &ResourceManager) -> Subscriptions {
        // Merged per source before anything registers: the resource
        // manager's own `merge_interval` does not wake a refresh task that
        // already computed its wait, so registering a shared source once per
        // group — the first with its own, possibly longer, interval — could
        // let the first refresh miss a later group's shorter one.
        let mut sources: Vec<Source> = Vec::new();
        for g in &cfg.group_specs {
            let Some(path) = &g.import.policy_path else {
                continue;
            };
            let interval = g
                .import
                .update_interval
                .map(|secs| i64::try_from(secs).unwrap_or(i64::MAX));
            match sources.iter_mut().find(|s| &s.path == path) {
                Some(s) => {
                    s.interval = match (s.interval, interval) {
                        (Some(cur), Some(new)) => Some(cur.min(new)),
                        (cur, new) => cur.or(new),
                    }
                }
                None => sources.push(Source {
                    path: path.clone(),
                    label: format!("policy-path of `{}`", g.name),
                    interval,
                }),
            }
        }
        let mut handles = Vec::new();
        let mut receivers = Vec::new();
        for s in sources {
            let spec = ResourceSpec {
                source: source_of(&s.path),
                update_interval: s.interval,
            };
            let handle = resources.get_labelled(&spec, &s.label);
            receivers.push(handle.subscribe());
            handles.push((s.path, handle));
        }
        Subscriptions { handles, receivers }
    }

    /// The receivers, for the one watcher of this generation.
    pub(crate) fn take_receivers(&mut self) -> Vec<watch::Receiver<u64>> {
        std::mem::take(&mut self.receivers)
    }

    /// What every subscription holds right now; one that holds nothing yet
    /// is absent.
    pub(crate) fn snapshots(&self) -> Snapshots {
        self.handles
            .iter()
            .filter_map(|(path, handle)| {
                let (data, _) = handle.current().data()?;
                let text = String::from_utf8_lossy(&data);
                Some((path.clone(), Arc::new(subscription::parse(&text))))
            })
            .collect()
    }
}

fn source_of(path: &PolicyPath) -> ResourceSource {
    match path {
        PolicyPath::Url(url) => ResourceSource::Url(url.expose().clone()),
        PolicyPath::File(file) => ResourceSource::File(file.clone()),
    }
}

impl Engine {
    /// Rebuilds the registry of the current generation after every burst of
    /// subscription updates (M3 design 5.7). The task goes with the
    /// generation: the runtime keeps its handle and aborts it when dropped.
    /// `rt` must be the very generation `receivers` came from — the caller
    /// reads it under the same lock that published it, so two interleaved
    /// swaps can never attach one generation's receivers to another. Must be
    /// called inside a tokio runtime (spawns a task).
    pub(crate) fn watch_subscriptions(
        self: &Arc<Self>,
        rt: &Arc<Runtime>,
        receivers: Vec<watch::Receiver<u64>>,
    ) {
        if receivers.is_empty() {
            return;
        }
        let task = tokio::spawn(rebuild_on_change(
            Arc::downgrade(self),
            Arc::downgrade(rt),
            receivers,
        ));
        let _ = rt.watcher.set(AbortOnDropHandle::new(task));
    }

    /// Assembles `rt`'s profile anew from what its subscriptions hold now and
    /// publishes the registry built from it; every outbound whose line did
    /// not change is kept (M2 design 7.1). `false` when `rt` is no longer the
    /// current generation: then nothing is published.
    pub(crate) fn rebuild_registry(&self, rt: &Arc<Runtime>) -> bool {
        // Cheap, unlocked pre-check for a generation a swap has already
        // replaced: skips the assembly, the build and their warnings for
        // work nobody will use. The locked re-check below still catches a
        // swap that lands while this call is running.
        if !Arc::ptr_eq(&self.runtime(), rt) {
            return false;
        }
        let assembly = assemble(&rt.config, &rt.subscriptions.snapshots());
        for d in assembly.diagnostics.iter() {
            tracing::warn!("{d}");
        }
        let shared = self.shared();
        // Built outside the lock, which is never held across I/O: a
        // `previous` that goes stale meanwhile costs some reuse, nothing else.
        let previous = shared.cell.load();
        let built = PolicyRegistry::build(
            &rt.config,
            &assembly,
            rt.factory.as_ref(),
            &shared.cell,
            shared.selections.clone(),
            previous.as_deref(),
            shared.empty_group,
            &shared.auto,
        );
        let registry = match built {
            Ok(registry) => Arc::new(registry),
            Err(e) => {
                tracing::warn!(error = %e.message, "cannot rebuild the policies after a subscription update; the current ones stay");
                return true;
            }
        };
        // Computed outside the generation lock, which never does I/O: the
        // synchronous writer behind `tracing` would otherwise run under it.
        let changes = member_changes(previous.as_deref(), &registry);
        let published = {
            let _generation = self.generation_lock();
            if Arc::ptr_eq(&self.runtime(), rt) {
                shared.cell.store(registry);
                true
            } else {
                false
            }
        };
        if published {
            log_changes(&changes);
        }
        published
    }
}

/// Waits for a change, lets the burst settle, rebuilds; ends when the
/// generation is gone or replaced.
async fn rebuild_on_change(
    engine: Weak<Engine>,
    rt: Weak<Runtime>,
    mut receivers: Vec<watch::Receiver<u64>>,
) {
    while any_changed(&mut receivers).await {
        tokio::time::sleep(REBUILD_DEBOUNCE).await;
        // what changed during the pause is in this rebuild already
        for rx in &mut receivers {
            rx.mark_unchanged();
        }
        let (Some(engine), Some(rt)) = (engine.upgrade(), rt.upgrade()) else {
            return;
        };
        if !engine.rebuild_registry(&rt) {
            return;
        }
    }
}

/// Waits until one of `receivers` sees a new version; `false` once one is
/// closed: the resource manager, and with it the generation, is gone.
async fn any_changed(receivers: &mut [watch::Receiver<u64>]) -> bool {
    let mut waits: Vec<_> = receivers
        .iter_mut()
        .map(|rx| Box::pin(rx.changed()))
        .collect();
    std::future::poll_fn(|cx| {
        for wait in &mut waits {
            if let Poll::Ready(result) = wait.as_mut().poll(cx) {
                return Poll::Ready(result.is_ok());
            }
        }
        Poll::Pending
    })
    .await
}

/// Which groups' members changed, and by how much (added, removed): pure, so
/// it can run before the generation lock is taken — the lock never does
/// I/O, and a synchronous log writer is I/O.
fn member_changes(
    before: Option<&PolicyRegistry>,
    after: &PolicyRegistry,
) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    for group in after.group_names() {
        let now: HashSet<&String> = after.members(&group).unwrap_or_default().iter().collect();
        let was: HashSet<&String> = before
            .and_then(|b| b.members(&group))
            .unwrap_or_default()
            .iter()
            .collect();
        let added = now.difference(&was).count();
        let removed = was.difference(&now).count();
        if added + removed > 0 {
            out.push((group, added, removed));
        }
    }
    out
}

/// One INFO line per group whose members changed: its name and how many came
/// and went — never where they came from (M3-D7). Called after the
/// generation lock is released, and only when the rebuild actually published.
fn log_changes(changes: &[(String, usize, usize)]) {
    for (group, added, removed) in changes {
        tracing::info!(group = %group, added, removed, "policy group members updated");
    }
}

/// The warnings an assembly from what earlier runs cached gives (M3 design
/// 5.9). Offline: reads the data directory and local files, nothing else.
fn subscription_diagnostics(cfg: &Config, data_dir: &Path) -> Diagnostics {
    let mut snapshots = Snapshots::new();
    for g in &cfg.group_specs {
        let Some(path) = &g.import.policy_path else {
            continue;
        };
        if snapshots.contains_key(path) {
            continue;
        }
        if let Some(data) = rurge_net::resource::cached(data_dir, &source_of(path)) {
            let text = String::from_utf8_lossy(&data);
            snapshots.insert(path.clone(), Arc::new(subscription::parse(&text)));
        }
    }
    assemble(cfg, &snapshots).diagnostics
}

/// `load_checked`, plus — when the profile itself is sound — what the
/// cached subscriptions say: what `rurge check` and `POST /v1/profiles/check`
/// report.
pub fn check_profile(
    path: &Path,
    opts: &LoadOptions,
    data_dir: &Path,
) -> Result<Loaded, LoadError> {
    let mut loaded = crate::outbounds::load_checked(path, opts)?;
    if !loaded.diagnostics.has_errors() {
        loaded
            .diagnostics
            .extend(subscription_diagnostics(&loaded.config, data_dir));
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeOptions;
    use crate::shared::EngineShared;
    use crate::stack::StackOptions;
    use rurge_config::codes;
    use rurge_config::config::from_text;
    use rurge_dns::system::StaticSystemDns;
    use rurge_net::socket::NoopSocketHook;
    use rurge_rules::{GeoUrls, OutboundMode};

    const PROFILE: &str = "[Proxy Group]\nLocal = select, DIRECT, policy-path=nodes.txt\n\
Remote = select, DIRECT, policy-path=https://sub.test/nodes?token=t0k3n\n[Rule]\nFINAL,Local\n";

    fn messages(d: &Diagnostics) -> Vec<(&'static str, String)> {
        d.iter().map(|d| (d.code, d.message.clone())).collect()
    }

    /// A URL is looked for in the cache only: never fetched, never printed.
    #[test]
    fn the_offline_check_reads_files_and_caches_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("nodes.txt"),
            "N1 = http, n1.test, 80\nnot a policy\n",
        )
        .unwrap();
        let loaded = from_text(
            PROFILE,
            &dir.path().join("t.conf"),
            &LoadOptions::for_tests(),
        );
        assert!(!loaded.diagnostics.has_errors());
        let data = tempfile::tempdir().unwrap();
        assert_eq!(
            messages(&subscription_diagnostics(&loaded.config, data.path())),
            [
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `Local`: `policy-path` line 2 skipped: not a policy line (`Name = type, ...`)".to_string()
                ),
                (
                    codes::W_RESOURCE_UNAVAILABLE,
                    "policy group `Remote`: `policy-path` has no content yet (never downloaded, or the file cannot be read); its imported members are unknown".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_profile_with_errors_gets_no_subscription_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let sound = dir.path().join("sound.conf");
        std::fs::write(&sound, PROFILE).unwrap();
        let loaded = check_profile(&sound, &LoadOptions::for_tests(), data.path()).unwrap();
        let found = messages(&loaded.diagnostics);
        assert!(
            found
                .iter()
                .any(|(code, _)| *code == codes::W_RESOURCE_UNAVAILABLE),
            "{found:?}"
        );
        assert!(found.iter().all(|(_, m)| !m.contains("t0k3n")), "{found:?}");
        let broken = dir.path().join("broken.conf");
        std::fs::write(&broken, PROFILE.replace("FINAL,Local", "FINAL,Nope")).unwrap();
        let loaded = check_profile(&broken, &LoadOptions::for_tests(), data.path()).unwrap();
        assert!(loaded.diagnostics.has_errors());
        assert!(
            loaded
                .diagnostics
                .iter()
                .all(|d| d.code != codes::W_RESOURCE_UNAVAILABLE)
        );
    }

    /// A generation of a profile whose only section besides `[Rule]` is
    /// `[Proxy Group]` with `groups`, offline.
    async fn generation(dir: &Path, groups: &str, shared: EngineShared) -> Runtime {
        let text = format!("[Proxy Group]\n{groups}\n[Rule]\nFINAL,DIRECT\n");
        let path = dir.join("t.conf");
        std::fs::write(&path, &text).unwrap();
        let loaded = from_text(&text, &path, &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        let stack = StackOptions {
            data_dir: dir.to_path_buf(),
            no_network: true,
            geo_urls: GeoUrls::default(),
            dns_cache_size: 16,
            system: Arc::new(StaticSystemDns::default()),
            wait: Duration::ZERO,
            dns_connector: None,
            socket_hook: Arc::new(NoopSocketHook),
        };
        let opts = RuntimeOptions {
            stack,
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(60),
            shared,
            request_log_size: 16,
        };
        Runtime::build(loaded.config, opts).await.unwrap()
    }

    /// The rebuild of a generation a reload replaced would put the old
    /// profile's groups back: it publishes nothing (M3 design 5.7).
    #[tokio::test]
    async fn a_rebuild_of_a_replaced_generation_publishes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("nodes.txt"), "N1 = http, n1.test, 80\n").unwrap();
        let first = generation(
            dir.path(),
            "Sub = select, policy-path=nodes.txt",
            EngineShared::default(),
        )
        .await;
        let engine = Engine::new(first);
        let first = engine.runtime();
        assert_eq!(engine.registry().members("Sub").unwrap(), ["N1"]);
        let next = generation(dir.path(), "Sub = select, DIRECT", engine.shared()).await;
        engine.swap_runtime(next);

        let before = engine.registry();
        assert!(!engine.rebuild_registry(&first));
        assert!(Arc::ptr_eq(&before, &engine.registry()));
        assert_eq!(engine.registry().members("Sub").unwrap(), ["DIRECT"]);
        // the current generation's own rebuild does publish
        assert!(engine.rebuild_registry(&engine.runtime()));
        assert!(!Arc::ptr_eq(&before, &engine.registry()));
        assert_eq!(engine.registry().members("Sub").unwrap(), ["DIRECT"]);
    }

    /// After a swap, the generation it replaced is not kept alive anywhere:
    /// no stray strong reference outlives the swap itself. A subscription on
    /// both generations makes each one actually start its watcher task,
    /// exercising the hand-over `watch_subscriptions` goes through.
    #[tokio::test]
    async fn a_swapped_out_generation_is_freed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("nodes.txt"), "N1 = http, n1.test, 80\n").unwrap();
        let first = generation(
            dir.path(),
            "Sub = select, policy-path=nodes.txt",
            EngineShared::default(),
        )
        .await;
        let engine = Engine::new(first);
        let weak = Arc::downgrade(&engine.runtime());
        let next = generation(
            dir.path(),
            "Sub = select, policy-path=nodes.txt",
            engine.shared(),
        )
        .await;
        engine.swap_runtime(next);
        assert!(
            weak.upgrade().is_none(),
            "the replaced generation is still alive"
        );
    }
}
