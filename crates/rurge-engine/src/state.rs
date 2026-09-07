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
pub const STATE_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Features {
    pub system_proxy: bool,
    pub enhanced_mode: bool,
    pub mitm: bool,
    pub capture: bool,
    pub rewrite: bool,
    pub scripting: bool,
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
            current_profile: None,
        }
    }
}

impl State {
    /// A missing file is the default state; a broken one is reported and ignored.
    pub fn load(path: &Path) -> State {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<State>(&text) {
                Ok(state) => state,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "state.json is malformed; using defaults");
                    State::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cannot read state.json; using defaults");
                State::default()
            }
        }
    }

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
/// the file atomically (temp file + rename) on every `update`, serialized
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
}

fn read_state(path: &Path) -> State {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<State>(&text) {
            Ok(state) => state,
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

    #[test]
    fn loads_defaults_selections_and_tolerates_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let missing = State::load(&dir.path().join("none.json"));
        assert_eq!(missing, State::default());
        assert_eq!(missing.version, 1);
        let path = dir.path().join(STATE_FILE);
        std::fs::write(
            &path,
            r#"{"version":1,"outbound_mode":"direct","group_selections":{"surge.conf":{"Pick":"HK"}},"features":{"mitm":true}}"#,
        )
        .unwrap();
        let state = State::load(&path);
        assert_eq!(state.outbound_mode.as_deref(), Some("direct"));
        assert!(state.features.mitm && !state.features.capture);
        assert_eq!(state.selections_for("surge.conf").get("Pick"), Some("HK"));
        assert!(state.selections_for("other.conf").is_empty());
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(State::load(&path), State::default());
        assert_eq!(
            profile_key(Path::new("/etc/rurge/surge.conf")),
            "surge.conf"
        );
        assert_eq!(profile_key(&Path::new("p").join("my.conf")), "my.conf");
        let round = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<State>(&round).unwrap(), state);
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
