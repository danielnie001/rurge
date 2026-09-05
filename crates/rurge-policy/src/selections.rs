//! Persisted `select` group choices for the current profile (read from
//! `state.json` by the engine, written by the M4 API).

use std::collections::HashMap;

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
