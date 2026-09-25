//! Policy groups as the control plane sees them, and the one thing it may
//! change: the selection of a `select` group (M1 design 6.3). Everything is
//! read from the registry in use, so imported and derived members show up
//! as they come and go (phase 2 M3 design 5.5).

use crate::engine::Engine;
use crate::state::profile_key;
use rurge_config::GroupKind;
use rurge_config::rule::PolicyRef;
use rurge_policy::PolicyRegistry;
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
    /// As assembled.
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

fn member_view(registry: &PolicyRegistry, name: &str) -> MemberView {
    if let Some(line) = registry.line(name) {
        return MemberView {
            name: name.to_string(),
            is_group: line.is_group,
            type_description: line.keyword.to_string(),
            line_hash: line_hash(&format!(
                "{name} = {}",
                rurge_config::redact::redact_definition(&line.definition)
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
        let registry = self.registry();
        registry
            .group_names()
            .into_iter()
            .filter_map(|name| {
                let g = registry.group(&name)?;
                Some(GroupView {
                    kind: g.kind,
                    hidden: g.hidden,
                    members: g
                        .members
                        .iter()
                        .map(|m| member_view(&registry, m))
                        .collect(),
                    selected: registry.current_member(&name),
                    name,
                })
            })
            .collect()
    }

    /// The definition of a policy or group with its secrets blanked — an
    /// imported policy's as imported, a derived one's with its relay; a
    /// built-in is described by its own name.
    pub fn policy_detail(&self, name: &str) -> Option<String> {
        if let Some(line) = self.registry().line(name) {
            return Some(rurge_config::redact::redact_definition(&line.definition));
        }
        match PolicyRef::parse(name) {
            PolicyRef::Builtin(b) => Some(b.name().to_string()),
            _ => None,
        }
    }

    /// The member `group` points at right now (any kind of group).
    pub fn group_selection(&self, group: &str) -> Result<String, SelectError> {
        let registry = self.registry();
        if registry.group(group).is_none() {
            return Err(SelectError::UnknownGroup(group.to_string()));
        }
        Ok(registry.current_member(group).unwrap_or_default())
    }

    /// Takes effect for the next connection and is written to `state.json`
    /// under the profile's file name.
    pub async fn select_group(&self, group: &str, member: &str) -> Result<(), SelectError> {
        {
            let registry = self.registry();
            let Some(g) = registry.group(group) else {
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
        }
        self.shared().selections.set(group, member);
        if let Some(store) = self.state_store() {
            let profile = profile_key(&self.runtime().config.source.main);
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
    use crate::outbounds::EngineFactory;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::connector::SystemResolve;
    use rurge_net::socket::NoopSocketHook;
    use rurge_policy::{EmptyGroup, RegistryCell, SelectionTable, Snapshots, assemble};
    use std::path::Path;
    use std::sync::Arc;

    fn registry(policy_line: &str) -> PolicyRegistry {
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
        let cfg = loaded.config;
        let factory = EngineFactory::new(&cfg, Arc::new(SystemResolve), Arc::new(NoopSocketHook));
        PolicyRegistry::build(
            &cfg,
            &assemble(&cfg, &Snapshots::new()),
            &factory,
            &RegistryCell::new(),
            Arc::new(SelectionTable::default()),
            None,
            EmptyGroup::Direct,
            &crate::shared::EngineShared::default().auto,
        )
        .expect("builds")
    }

    /// A `lineHash` served over the HTTP API must never let a client confirm a
    /// guessed credential offline: it has to be computed over the same
    /// redacted text `policy_detail` shows, not the raw definition line.
    #[test]
    fn line_hash_hides_credentials_but_changes_with_everything_else() {
        let a = registry("A = socks5, h.test, 1080, alice, s3cret");
        let same_but_password = registry("A = socks5, h.test, 1080, alice, other");
        assert_eq!(
            member_view(&a, "A").line_hash,
            member_view(&same_but_password, "A").line_hash,
            "a password-only difference must not change the hash"
        );

        // `headers=` values are credentials too, and are blanked whole
        let with_header = registry("A = http, h.test, 80, headers=X-Auth:tok3n");
        let same_but_header = registry("A = http, h.test, 80, headers=X-Auth:other");
        assert_eq!(
            member_view(&with_header, "A").line_hash,
            member_view(&same_but_header, "A").line_hash,
            "a `headers=` difference must not change the hash"
        );

        let different_port = registry("A = socks5, h.test, 1081, alice, s3cret");
        assert_ne!(
            member_view(&a, "A").line_hash,
            member_view(&different_port, "A").line_hash,
            "a port change must change the hash"
        );

        let different_name = registry("B = socks5, h.test, 1080, alice, s3cret");
        assert_ne!(
            member_view(&a, "A").line_hash,
            member_view(&different_name, "B").line_hash,
            "a name change must change the hash"
        );

        // the hash is over the literal redacted line — computed here
        // independently rather than hard-coding `***` spacing
        let expected = format!(
            "A = {}",
            rurge_config::redact::redact_definition(&a.line("A").unwrap().definition)
        );
        assert_eq!(member_view(&a, "A").line_hash, line_hash(&expected));

        // a built-in hashes its own name
        assert_eq!(member_view(&a, "DIRECT").line_hash, line_hash("DIRECT"));

        // trojan: the password, the WebSocket path and its headers are all secrets
        let trojan = registry(
            "A = trojan, t.test, 443, password=pw0rd, ws=true, ws-path=/s3cretpath, ws-headers=X-Key:k3y",
        );
        let same_but_secrets = registry(
            "A = trojan, t.test, 443, password=other, ws=true, ws-path=/elsewhere, ws-headers=X-Key:zzz",
        );
        assert_eq!(
            member_view(&trojan, "A").line_hash,
            member_view(&same_but_secrets, "A").line_hash,
            "secret-only differences must not change the hash"
        );
        let without_ws = registry("A = trojan, t.test, 443, password=pw0rd");
        assert_ne!(
            member_view(&trojan, "A").line_hash,
            member_view(&without_ws, "A").line_hash
        );

        // a password containing a comma has to be quoted: nothing of it may
        // reach the hash, tail included
        let quoted = registry("A = trojan, t.test, 443, password=\"pw0rd,x\", ws=true");
        let same_but_quoted_password =
            registry("A = trojan, t.test, 443, password=\"other,y\", ws=true");
        assert_eq!(
            member_view(&quoted, "A").line_hash,
            member_view(&same_but_quoted_password, "A").line_hash,
            "a quoted password must not change the hash either"
        );

        // the quote may open in the middle of the value, too
        let mid = registry("A = trojan, t.test, 443, password=ab\"c,d\", ws=true");
        let same_but_mid = registry("A = trojan, t.test, 443, password=ab\"c,e\", ws=true");
        assert_eq!(
            member_view(&mid, "A").line_hash,
            member_view(&same_but_mid, "A").line_hash,
            "a quote opening mid-value must not leave a tail in the hash"
        );
    }
}
