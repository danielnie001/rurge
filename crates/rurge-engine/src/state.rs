//! `state.json` (M3 design §9.1): runtime state that outlives the config.
//! M3 only reads it; the M4 API writes it (temp file + rename).

use rurge_policy::GroupSelections;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

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
        assert_eq!(profile_key(Path::new("C:\\p\\my.conf")), "my.conf");
        let round = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<State>(&round).unwrap(), state);
    }
}
