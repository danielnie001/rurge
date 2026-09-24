//! The `policy-path` subscriptions of one config generation (phase 2 M3
//! design 5.1, 5.9): registered with its resource manager, read into
//! snapshots for the assembly, and checked offline for `rurge check`.

use rurge_config::config::{LoadError, LoadOptions, Loaded};
use rurge_config::spec::PolicyPath;
use rurge_config::{Config, Diagnostics};
use rurge_net::resource::{ResourceHandle, ResourceManager, ResourceSource, ResourceSpec};
use rurge_policy::{Snapshots, assemble, subscription};
use std::path::Path;
use std::sync::Arc;

pub(crate) struct Subscriptions {
    /// One per source, in the order the groups first name them.
    handles: Vec<(PolicyPath, ResourceHandle)>,
}

impl Subscriptions {
    /// Registers every `policy-path` with this generation's resource
    /// manager. What earlier runs cached is loaded right here, synchronously
    /// and without the network, so a start or a reload keeps the members it
    /// had (M3-D5). Log lines name a source after the first group that uses
    /// it, never by its URL (M3-D7).
    pub(crate) fn register(cfg: &Config, resources: &ResourceManager) -> Subscriptions {
        let mut handles: Vec<(PolicyPath, ResourceHandle)> = Vec::new();
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
                handles.push((path.clone(), handle));
            }
        }
        Subscriptions { handles }
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
    use rurge_config::codes;
    use rurge_config::config::from_text;

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
}
