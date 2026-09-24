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

impl Subscriptions {
    /// Registers every `policy-path` with this generation's resource
    /// manager. What earlier runs cached is loaded right here, synchronously
    /// and without the network, so a start or a reload keeps the members it
    /// had (M3-D5). Log lines name a source after the first group that uses
    /// it, never by its URL (M3-D7).
    pub(crate) fn register(cfg: &Config, resources: &ResourceManager) -> Subscriptions {
        let mut handles: Vec<(PolicyPath, ResourceHandle)> = Vec::new();
        let mut receivers = Vec::new();
        for g in &cfg.group_specs {
            let Some(path) = &g.import.policy_path else {
                continue;
            };
            let spec = ResourceSpec {
                source: source_of(path),
                update_interval: g
                    .import
                    .update_interval
                    .map(|secs| i64::try_from(secs).unwrap_or(i64::MAX)),
            };
            // every group registers: a shared source refreshes at the
            // shortest interval any of them asks for
            let label = format!("policy-path of `{}`", g.name);
            let handle = resources.get_labelled(&spec, &label);
            if !handles.iter().any(|(p, _)| p == path) {
                receivers.push(handle.subscribe());
                handles.push((path.clone(), handle));
            }
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
    pub(crate) fn watch_subscriptions(self: &Arc<Self>, receivers: Vec<watch::Receiver<u64>>) {
        if receivers.is_empty() {
            return;
        }
        let rt = self.runtime();
        let task = tokio::spawn(rebuild_on_change(
            Arc::downgrade(self),
            Arc::downgrade(&rt),
            receivers,
        ));
        let _ = rt.watcher.set(AbortOnDropHandle::new(task));
    }

    /// Assembles `rt`'s profile anew from what its subscriptions hold now and
    /// publishes the registry built from it; every outbound whose line did
    /// not change is kept (M2 design 7.1). `false` when `rt` is no longer the
    /// current generation: then nothing is published.
    pub(crate) fn rebuild_registry(&self, rt: &Arc<Runtime>) -> bool {
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
        );
        let registry = match built {
            Ok(registry) => Arc::new(registry),
            Err(e) => {
                tracing::warn!(error = %e.message, "cannot rebuild the policies after a subscription update; the current ones stay");
                return true;
            }
        };
        let _generation = self.generation_lock();
        if !Arc::ptr_eq(&self.runtime(), rt) {
            return false;
        }
        log_member_changes(previous.as_deref(), &registry);
        shared.cell.store(registry);
        true
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

/// One INFO line per group whose members changed: its name and how many came
/// and went — never where they came from (M3-D7).
fn log_member_changes(before: Option<&PolicyRegistry>, after: &PolicyRegistry) {
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
            tracing::info!(group = %group, added, removed, "policy group members updated");
        }
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

    /// A rebuild that finishes after its generation has already been
    /// replaced by a reload must not publish: the registry in
    /// `EngineShared.cell` stays the successor's, never the stale
    /// generation's (M3 design 5.7).
    #[tokio::test]
    async fn a_rebuild_of_a_replaced_generation_publishes_nothing() {
        async fn build_generation(
            dir: &Path,
            profile: &str,
            shared: EngineShared,
        ) -> crate::runtime::Runtime {
            let loaded = from_text(profile, &dir.join("t.conf"), &LoadOptions::for_tests());
            assert!(!loaded.diagnostics.has_errors());
            crate::runtime::Runtime::build(
                loaded.config,
                RuntimeOptions {
                    stack: StackOptions {
                        data_dir: dir.to_path_buf(),
                        no_network: true,
                        geo_urls: GeoUrls::default(),
                        dns_cache_size: 2000,
                        system: Arc::new(StaticSystemDns::default()),
                        wait: std::time::Duration::ZERO,
                        dns_connector: None,
                        socket_hook: Arc::new(NoopSocketHook),
                    },
                    outbound_mode: OutboundMode::Rule,
                    idle_timeout: std::time::Duration::from_secs(600),
                    shared,
                    request_log_size: 1000,
                },
            )
            .await
            .unwrap()
        }

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("nodes.txt"), "N1 = http, n1.test, 80\n").unwrap();
        let profile = "[Proxy Group]\nSub = select, policy-path=nodes.txt\n[Rule]\nFINAL,DIRECT\n";
        let engine = crate::engine::Engine::new(
            build_generation(dir.path(), profile, EngineShared::default()).await,
        );
        let stale = engine.runtime();
        engine.swap_runtime(build_generation(dir.path(), profile, engine.shared()).await);
        assert!(!Arc::ptr_eq(&engine.runtime(), &stale));
        let before = engine.registry();
        // A change to the source after `stale` stopped being current must
        // never surface, however `stale`'s belated rebuild reads it.
        std::fs::write(
            dir.path().join("nodes.txt"),
            "N1 = http, n1.test, 80\nN2 = http, n2.test, 80\n",
        )
        .unwrap();
        assert!(!engine.rebuild_registry(&stale));
        assert!(Arc::ptr_eq(&engine.registry(), &before));
    }
}
