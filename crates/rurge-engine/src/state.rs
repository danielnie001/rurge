//! `state.json` (M3 design §9.1): runtime state that outlives the config.
//! M3 only reads it; from M4a on, `StateStore` is the single writer (temp
//! file + rename).

use rurge_policy::GroupSelections;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const STATE_FILE: &str = "state.json";
/// The second version made the four switches of `[MITM]` and of the later
/// stages three-state: never set, on, off (`phase 4 M2 design 11.2`).
pub const STATE_VERSION: u32 = 2;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Features {
    pub system_proxy: bool,
    pub enhanced_mode: bool,
    /// Never set, on, or off: an absent key is a switch the user never
    /// touched, and a first-version file's `false` is not a choice either
    /// (`read_state` drops it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mitm: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewrite: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scripting: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct State {
    pub version: u32,
    pub outbound_mode: Option<String>,
    pub global_policy: Option<String>,
    pub features: Features,
    /// profile file name → group name → selected member
    pub group_selections: HashMap<String, HashMap<String, String>>,
    pub system_proxy_backup: Option<serde_json::Value>,
    /// What the enhanced mode leaves behind when it is interrupted; the manager
    /// owns the shape, the engine does not read it (same as `system_proxy_backup`).
    pub enhanced_backup: Option<serde_json::Value>,
    pub current_profile: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            version: STATE_VERSION,
            outbound_mode: None,
            global_policy: None,
            features: Features::default(),
            group_selections: HashMap::new(),
            system_proxy_backup: None,
            enhanced_backup: None,
            current_profile: None,
        }
    }
}

impl State {
    pub fn selections_for(&self, profile: &str) -> GroupSelections {
        self.group_selections
            .get(profile)
            .map(|m| GroupSelections::from_map(m.clone()))
            .unwrap_or_default()
    }
}

/// Profiles are keyed by their file name (`surge.conf`), not their full path.
pub fn profile_key(main: &Path) -> String {
    main.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| main.display().to_string())
}

/// The single writer of `state.json`: keeps an in-memory copy and rewrites
/// the file atomically (temp file + rename) on every write, serialized
/// by one async mutex.
pub struct StateStore {
    path: PathBuf,
    state: tokio::sync::Mutex<State>,
}

impl StateStore {
    /// Reads the file (missing → defaults; unparsable → defaults, and the bad
    /// file is renamed to `state.json.broken` so it is not silently lost).
    pub async fn open(path: PathBuf) -> (Arc<StateStore>, State) {
        let p = path.clone();
        let state = tokio::task::spawn_blocking(move || read_state(&p))
            .await
            .unwrap_or_default();
        let store = Arc::new(StateStore {
            path,
            state: tokio::sync::Mutex::new(state.clone()),
        });
        (store, state)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn snapshot(&self) -> State {
        self.state.lock().await.clone()
    }

    /// Applies `f` to the in-memory state and writes the result to disk before
    /// returning. A write failure is logged and does not fail the caller: the
    /// running state is still updated.
    pub async fn update(&self, f: impl FnOnce(&mut State)) -> State {
        let mut guard = self.state.lock().await;
        f(&mut guard);
        let snapshot = guard.clone();
        let path = self.path.clone();
        let text = serde_json::to_string_pretty(&snapshot).expect("State serializes");
        let written = tokio::task::spawn_blocking(move || write_atomic(&path, &text)).await;
        match written {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::error!(path = %self.path.display(), error = %e, "cannot write state.json")
            }
            Err(e) => tracing::error!(error = %e, "state.json writer task failed"),
        }
        snapshot
    }

    /// Applies `f` to a copy of the state and writes the copy to disk; only once
    /// it is written does the copy become the state. A write that fails leaves
    /// the state as it was, and is the error.
    pub async fn try_update(&self, f: impl FnOnce(&mut State)) -> io::Result<State> {
        let mut guard = self.state.lock().await;
        let mut next = guard.clone();
        f(&mut next);
        let path = self.path.clone();
        let text = serde_json::to_string_pretty(&next).expect("State serializes");
        tokio::task::spawn_blocking(move || write_atomic(&path, &text))
            .await
            .map_err(io::Error::other)??;
        *guard = next.clone();
        Ok(next)
    }
}

fn read_state(path: &Path) -> State {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<State>(&text) {
            Ok(mut state) => {
                // A first-version file wrote the four switches as booleans,
                // and the code that wrote them could never switch one on:
                // none of them is a choice of the user's (`phase 4 M2 design
                // 11.2`). The next write puts the second version on disk.
                if state.version < STATE_VERSION {
                    state.features.mitm = None;
                    state.features.capture = None;
                    state.features.rewrite = None;
                    state.features.scripting = None;
                    state.version = STATE_VERSION;
                }
                state
            }
            Err(e) => {
                let broken = path.with_extension("json.broken");
                tracing::warn!(path = %path.display(), error = %e, kept = %broken.display(), "state.json is malformed; using defaults");
                let _ = std::fs::rename(path, &broken);
                State::default()
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => State::default(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "cannot read state.json; using defaults");
            State::default()
        }
    }
}

fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loads_defaults_selections_and_tolerates_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        std::fs::write(
            &path,
            r#"{"version":1,"outbound_mode":"direct","group_selections":{"surge.conf":{"Pick":"HK"}},"features":{"mitm":true}}"#,
        )
        .unwrap();
        let (_, state) = StateStore::open(path).await;
        assert_eq!(state.outbound_mode.as_deref(), Some("direct"));
        // the four switches of the first version were never a choice of the
        // user's: they are read as never set (the plan's P15, P19)
        assert_eq!(state.features.mitm, None);
        assert_eq!(state.features.capture, None);
        assert_eq!(state.version, STATE_VERSION);
        assert_eq!(state.selections_for("surge.conf").get("Pick"), Some("HK"));
        assert!(state.selections_for("other.conf").is_empty());
        assert_eq!(
            profile_key(Path::new("/etc/rurge/surge.conf")),
            "surge.conf"
        );
        assert_eq!(profile_key(&Path::new("p").join("my.conf")), "my.conf");
        let round = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<State>(&round).unwrap(), state);
    }

    /// A file of the first version named the four switches; the code that
    /// wrote them could not switch any of them on, so whatever it wrote is no
    /// choice of the user's (phase 4 M2 design 11.2): all four read as never
    /// set, and the file counts as a second-version one from now on.
    #[tokio::test]
    async fn a_version_1_file_leaves_the_four_switches_unset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        std::fs::write(
            &path,
            r#"{"version":1,"features":{"mitm":false,"capture":true}}"#,
        )
        .unwrap();
        let (_, state) = StateStore::open(path).await;
        assert_eq!(state.version, 2);
        assert_eq!(state.features.mitm, None);
        assert_eq!(state.features.capture, None);
        assert_eq!(state.features.rewrite, None);
        assert_eq!(state.features.scripting, None);
    }

    /// Only the switches somebody set are written: the rest of the file is
    /// what the first version knew, and an older `rurge` reads it (phase 4 M2
    /// design 11.2).
    #[tokio::test]
    async fn only_the_switches_that_were_set_are_written() {
        let state = State {
            features: Features {
                mitm: Some(false),
                ..Features::default()
            },
            ..State::default()
        };
        let text = serde_json::to_string(&state).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["version"], serde_json::json!(2));
        assert_eq!(
            json["features"],
            serde_json::json!({"system_proxy": false, "enhanced_mode": false, "mitm": false})
        );
    }

    /// What the phase 3 code read: booleans, every key defaulted. It reads a
    /// second-version file without calling it broken, and takes the switch
    /// that was written.
    #[tokio::test]
    async fn a_version_2_file_is_what_phase_3_reads() {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct OldFeatures {
            system_proxy: bool,
            enhanced_mode: bool,
            mitm: bool,
            capture: bool,
            rewrite: bool,
            scripting: bool,
        }

        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct OldState {
            version: u32,
            outbound_mode: Option<String>,
            features: OldFeatures,
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let (store, _) = StateStore::open(path.clone()).await;
        store
            .update(|s| {
                s.outbound_mode = Some("direct".into());
                s.features.mitm = Some(true);
            })
            .await;
        let text = std::fs::read_to_string(&path).unwrap();
        let old: OldState = serde_json::from_str(&text).expect("the first version reads it");
        assert_eq!(old.version, 2);
        assert_eq!(old.outbound_mode.as_deref(), Some("direct"));
        assert!(old.features.mitm);
        assert!(!old.features.capture);
        assert!(!old.features.rewrite && !old.features.scripting);
    }

    #[tokio::test]
    async fn store_opens_updates_atomically_and_renames_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        // missing → defaults, nothing written yet
        let (store, initial) = StateStore::open(path.clone()).await;
        assert_eq!(initial, State::default());
        assert!(!path.exists());
        // update writes the file atomically (no .tmp left behind)
        let after = store
            .update(|s| {
                s.outbound_mode = Some("direct".into());
                s.global_policy = Some("HK".into());
            })
            .await;
        assert_eq!(after.outbound_mode.as_deref(), Some("direct"));
        assert!(path.exists());
        assert!(!path.with_extension("json.tmp").exists());
        let on_disk: State =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk, after);
        assert_eq!(store.snapshot().await, after);
        // reopen reads it back
        let (_store2, reread) = StateStore::open(path.clone()).await;
        assert_eq!(reread.global_policy.as_deref(), Some("HK"));
        // garbage → defaults, and the broken file is kept aside as evidence
        std::fs::write(&path, "{ not json").unwrap();
        let (_store3, from_garbage) = StateStore::open(path.clone()).await;
        assert_eq!(from_garbage, State::default());
        assert!(path.with_extension("json.broken").exists());
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn try_update_changes_nothing_when_the_file_cannot_be_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let (store, _) = StateStore::open(path.clone()).await;
        // a state.json to leave alone: written by an update that succeeds
        store
            .update(|s| s.outbound_mode = Some("direct".into()))
            .await;
        let before = store.snapshot().await;
        let on_disk = std::fs::read_to_string(&path).unwrap();
        // the temporary file's name taken by a directory: writing it fails
        let tmp = path.with_extension("json.tmp");
        std::fs::create_dir(&tmp).unwrap();
        let expected = std::fs::write(&tmp, "x").unwrap_err();
        let error = store
            .try_update(|s| s.global_policy = Some("HK".into()))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), expected.kind());
        assert_eq!(store.snapshot().await, before);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), on_disk);
    }

    #[tokio::test]
    async fn try_update_writes_before_it_changes_the_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let (store, _) = StateStore::open(path.clone()).await;
        store
            .update(|s| s.outbound_mode = Some("direct".into()))
            .await;
        // f sees the state as it was, not a default
        let after = store
            .try_update(|s| {
                s.global_policy = Some("HK".into());
                assert_eq!(s.outbound_mode.as_deref(), Some("direct"));
            })
            .await
            .unwrap();
        assert_eq!(after.global_policy.as_deref(), Some("HK"));
        assert_eq!(store.snapshot().await, after);
        let on_disk: State =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk, after);
    }

    #[tokio::test]
    async fn enhanced_backup_round_trips_and_an_old_file_reads_without_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        // a file from before the field existed
        std::fs::write(&path, r#"{"version":1,"outbound_mode":"direct"}"#).unwrap();
        let (store, old) = StateStore::open(path.clone()).await;
        assert_eq!(old.enhanced_backup, None);
        let backup = serde_json::json!({"undo": [{"rp_filter": 1}]});
        let after = store
            .try_update(|s| s.enhanced_backup = Some(backup.clone()))
            .await
            .unwrap();
        assert_eq!(after.enhanced_backup, Some(backup.clone()));
        let (_, reread) = StateStore::open(path).await;
        assert_eq!(reread.enhanced_backup, Some(backup));
    }

    #[tokio::test]
    async fn concurrent_updates_are_serialized() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = StateStore::open(dir.path().join(STATE_FILE)).await;
        let mut tasks = Vec::new();
        for i in 0..20u32 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                store
                    .update(move |s| {
                        s.group_selections
                            .entry("p.conf".into())
                            .or_default()
                            .insert(format!("g{i}"), "m".into());
                    })
                    .await;
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let s = store.snapshot().await;
        assert_eq!(s.group_selections["p.conf"].len(), 20);
        let on_disk: State =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(STATE_FILE)).unwrap())
                .unwrap();
        assert_eq!(on_disk, s);
    }
}
