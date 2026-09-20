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
    /// First 16 hex digits of the SHA-256 of `<name> = <definition with its
    /// secrets blanked>`: identifies a definition line without carrying
    /// anything derived from a credential.
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
            line_hash: line_hash(&format!(
                "{} = {}",
                p.name,
                rurge_config::redact::redact_definition(&p.definition)
            )),
        };
    }
    if let Some(g) = cfg.groups.iter().find(|g| g.name == name) {
        return MemberView {
            name: name.to_string(),
            is_group: true,
            type_description: g.kind.keyword().to_string(),
            line_hash: line_hash(&format!(
                "{} = {}",
                g.name,
                rurge_config::redact::redact_definition(&g.definition)
            )),
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
                hidden: g.params.bool("hidden").unwrap_or(false),
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

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};
    use std::path::Path;

    fn config(policy_line: &str) -> Config {
        let text = format!("[General]\n[Proxy]\n{policy_line}\n[Rule]\nFINAL,DIRECT\n");
        let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        loaded.config
    }

    /// A `lineHash` served over the HTTP API must never let a client confirm a
    /// guessed credential offline: it has to be computed over the same
    /// redacted text `policy_detail` shows, not the raw definition line.
    #[test]
    fn line_hash_hides_credentials_but_changes_with_everything_else() {
        let a = config("A = socks5, h.test, 1080, alice, s3cret");
        let same_but_password = config("A = socks5, h.test, 1080, alice, other");
        assert_eq!(
            member_view(&a, "A").line_hash,
            member_view(&same_but_password, "A").line_hash,
            "a password-only difference must not change the hash"
        );

        // `headers=` values are credentials too, and are blanked whole
        let with_header = config("A = http, h.test, 80, headers=X-Auth:tok3n");
        let same_but_header = config("A = http, h.test, 80, headers=X-Auth:other");
        assert_eq!(
            member_view(&with_header, "A").line_hash,
            member_view(&same_but_header, "A").line_hash,
            "a `headers=` difference must not change the hash"
        );

        let different_port = config("A = socks5, h.test, 1081, alice, s3cret");
        assert_ne!(
            member_view(&a, "A").line_hash,
            member_view(&different_port, "A").line_hash,
            "a port change must change the hash"
        );

        let different_name = config("B = socks5, h.test, 1080, alice, s3cret");
        assert_ne!(
            member_view(&a, "A").line_hash,
            member_view(&different_name, "B").line_hash,
            "a name change must change the hash"
        );

        // the hash is over the literal redacted line — computed here
        // independently rather than hard-coding `***` spacing
        let p = a.policies.iter().find(|p| p.name == "A").unwrap();
        let expected = format!(
            "{} = {}",
            p.name,
            rurge_config::redact::redact_definition(&p.definition)
        );
        assert_eq!(member_view(&a, "A").line_hash, line_hash(&expected));

        // a built-in hashes its own name
        assert_eq!(member_view(&a, "DIRECT").line_hash, line_hash("DIRECT"));

        // trojan: the password, the WebSocket path and its headers are all secrets
        let trojan = config(
            "A = trojan, t.test, 443, password=pw0rd, ws=true, ws-path=/s3cretpath, ws-headers=X-Key:k3y",
        );
        let same_but_secrets = config(
            "A = trojan, t.test, 443, password=other, ws=true, ws-path=/elsewhere, ws-headers=X-Key:zzz",
        );
        assert_eq!(
            member_view(&trojan, "A").line_hash,
            member_view(&same_but_secrets, "A").line_hash,
            "secret-only differences must not change the hash"
        );
        let without_ws = config("A = trojan, t.test, 443, password=pw0rd");
        assert_ne!(
            member_view(&trojan, "A").line_hash,
            member_view(&without_ws, "A").line_hash
        );
    }
}
