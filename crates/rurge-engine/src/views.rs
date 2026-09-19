//! Policy groups as the control plane sees them, and the one thing it may
//! change: the selection of a `select` group (M1 design 6.3).

use crate::engine::Engine;
use crate::state::profile_key;
use rurge_config::rule::PolicyRef;
use rurge_config::{Config, GroupKind};
use sha2::{Digest, Sha256};
use std::fmt;

pub struct MemberView {
    pub name: String,
    pub is_group: bool,
    /// A policy's type keyword, a group's kind keyword, a built-in's name.
    pub type_description: String,
    /// First 16 hex digits of the SHA-256 of the definition line.
    pub line_hash: String,
}

pub struct GroupView {
    pub name: String,
    pub kind: GroupKind,
    pub hidden: bool,
    /// In profile order.
    pub members: Vec<MemberView>,
    /// The member the group points at right now.
    pub selected: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectError {
    UnknownGroup(String),
    NotSelectable(String),
    NotAMember { group: String, member: String },
}

impl fmt::Display for SelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SelectError::UnknownGroup(g) => write!(f, "unknown policy group `{g}`"),
            SelectError::NotSelectable(g) => write!(f, "`{g}` is not a select group"),
            SelectError::NotAMember { group, member } => {
                write!(f, "`{member}` is not a member of `{group}`")
            }
        }
    }
}

impl std::error::Error for SelectError {}

fn line_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn member_view(cfg: &Config, name: &str) -> MemberView {
    if let Some(p) = cfg.policies.iter().find(|p| p.name == name) {
        return MemberView {
            name: name.to_string(),
            is_group: false,
            type_description: p.kind.keyword().to_string(),
            line_hash: line_hash(&format!("{} = {}", p.name, p.definition)),
        };
    }
    if let Some(g) = cfg.groups.iter().find(|g| g.name == name) {
        return MemberView {
            name: name.to_string(),
            is_group: true,
            type_description: g.kind.keyword().to_string(),
            line_hash: line_hash(&format!("{} = {}", g.name, g.definition)),
        };
    }
    // a built-in (or a DEVICE: reference): nothing but its name describes it
    let shown = match PolicyRef::parse(name) {
        PolicyRef::Builtin(b) => b.name().to_string(),
        _ => name.to_string(),
    };
    MemberView {
        name: name.to_string(),
        is_group: false,
        line_hash: line_hash(&shown),
        type_description: shown,
    }
}

impl Engine {
    pub fn groups_view(&self) -> Vec<GroupView> {
        let rt = self.runtime();
        rt.config
            .groups
            .iter()
            .map(|g| GroupView {
                name: g.name.clone(),
                kind: g.kind,
                hidden: g
                    .params
                    .get("hidden")
                    .is_some_and(|v| v.eq_ignore_ascii_case("true")),
                members: g
                    .members
                    .iter()
                    .map(|m| member_view(&rt.config, m))
                    .collect(),
                selected: rt.policies.current_member(&g.name),
            })
            .collect()
    }

    /// The definition of a policy or group with its secrets blanked; a
    /// built-in is described by its own name.
    pub fn policy_detail(&self, name: &str) -> Option<String> {
        let rt = self.runtime();
        if let Some(p) = rt.config.policies.iter().find(|p| p.name == name) {
            return Some(rurge_config::redact::redact_definition(&p.definition));
        }
        if let Some(g) = rt.config.groups.iter().find(|g| g.name == name) {
            return Some(rurge_config::redact::redact_definition(&g.definition));
        }
        match PolicyRef::parse(name) {
            PolicyRef::Builtin(b) => Some(b.name().to_string()),
            _ => None,
        }
    }

    /// The member `group` points at right now (any kind of group).
    pub fn group_selection(&self, group: &str) -> Result<String, SelectError> {
        let rt = self.runtime();
        if !rt.config.groups.iter().any(|g| g.name == group) {
            return Err(SelectError::UnknownGroup(group.to_string()));
        }
        Ok(rt.policies.current_member(group).unwrap_or_default())
    }

    /// Takes effect for the next connection and is written to `state.json`
    /// under the profile's file name.
    pub async fn select_group(&self, group: &str, member: &str) -> Result<(), SelectError> {
        let rt = self.runtime();
        let Some(g) = rt.config.groups.iter().find(|g| g.name == group) else {
            return Err(SelectError::UnknownGroup(group.to_string()));
        };
        if g.kind != GroupKind::Select {
            return Err(SelectError::NotSelectable(group.to_string()));
        }
        if !g.members.iter().any(|m| m == member) {
            return Err(SelectError::NotAMember {
                group: group.to_string(),
                member: member.to_string(),
            });
        }
        self.shared().selections.set(group, member);
        if let Some(store) = self.state_store() {
            let profile = profile_key(&rt.config.source.main);
            let (group, member) = (group.to_string(), member.to_string());
            store
                .update(move |s| {
                    s.group_selections
                        .entry(profile)
                        .or_default()
                        .insert(group, member);
                })
                .await;
        }
        Ok(())
    }
}
