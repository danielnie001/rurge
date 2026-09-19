//! Persisted `select` group choices for the current profile (read from
//! `state.json` by the engine, written by the M4 API).

use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupSelections {
    map: HashMap<String, String>,
}

impl GroupSelections {
    pub fn new() -> GroupSelections {
        GroupSelections::default()
    }

    pub fn from_map(map: HashMap<String, String>) -> GroupSelections {
        GroupSelections { map }
    }

    pub fn get(&self, group: &str) -> Option<&str> {
        self.map.get(group).map(String::as_str)
    }

    pub fn set(&mut self, group: &str, member: &str) {
        self.map.insert(group.to_string(), member.to_string());
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The live `select` choices of the running profile (M1 design 6.3). One
/// table serves every config generation: `resolve` reads it on each call, the
/// API changes it, and `GroupSelections` stays the type that is loaded from
/// and saved to `state.json`.
#[derive(Debug, Default)]
pub struct SelectionTable {
    map: RwLock<HashMap<String, String>>,
}

impl SelectionTable {
    pub fn new(initial: GroupSelections) -> SelectionTable {
        SelectionTable {
            map: RwLock::new(initial.map),
        }
    }

    /// Owned, so no lock is held while the caller walks on through the groups.
    pub fn get(&self, group: &str) -> Option<String> {
        self.map
            .read()
            .expect("selection table")
            .get(group)
            .cloned()
    }

    pub fn set(&self, group: &str, member: &str) {
        self.map
            .write()
            .expect("selection table")
            .insert(group.to_string(), member.to_string());
    }

    pub fn snapshot(&self) -> GroupSelections {
        GroupSelections::from_map(self.map.read().expect("selection table").clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_starts_from_a_snapshot_and_can_be_saved_again() {
        let mut saved = GroupSelections::new();
        saved.set("Pick", "HK");
        let table = SelectionTable::new(saved.clone());
        assert_eq!(table.get("Pick").as_deref(), Some("HK"));
        assert_eq!(table.get("Other"), None);
        table.set("Pick", "JP");
        table.set("Other", "DIRECT");
        assert_eq!(table.get("Pick").as_deref(), Some("JP"));
        let snapshot = table.snapshot();
        assert_eq!(snapshot.get("Pick"), Some("JP"));
        assert_eq!(snapshot.get("Other"), Some("DIRECT"));
        assert_ne!(snapshot, saved);
        assert_eq!(SelectionTable::default().get("Pick"), None);
    }
}
