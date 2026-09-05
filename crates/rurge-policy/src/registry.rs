//! Name → outbound resolution (M3 design §5). Built once per config
//! generation; `resolve` is a table walk with no allocation beyond the chain.

use crate::selections::GroupSelections;
use rurge_config::rule::PolicyRef;
use rurge_config::{Builtin, Config, GroupKind, PolicyKind};
use rurge_proto::{OutboundRef, Reject, RejectKind};
use std::collections::HashMap;
use std::sync::Arc;

/// Deeper chains than this are treated as a defect (M1 rejects group cycles).
pub const MAX_DEPTH: usize = 16;

#[derive(Clone)]
pub struct Resolution {
    pub chain: Vec<String>,
    pub outbound: OutboundRef,
    /// Protocol keyword (or `DEVICE`) when the terminal policy is not implemented.
    pub unsupported: Option<String>,
}

enum Terminal {
    Direct,
    Reject(RejectKind),
}

enum Entry {
    Alias(Terminal),
    Proxy {
        kind: PolicyKind,
    },
    Group {
        members: Vec<String>,
        selected: Option<String>,
    },
}

pub struct PolicyRegistry {
    entries: HashMap<String, Entry>,
    order: Vec<String>,
    direct: OutboundRef,
    rejects: [OutboundRef; 4],
}

fn reject_slot(kind: RejectKind) -> usize {
    match kind {
        RejectKind::Reject => 0,
        RejectKind::Drop => 1,
        RejectKind::NoDrop => 2,
        RejectKind::TinyGif => 3,
    }
}

fn alias_terminal(kind: PolicyKind) -> Option<Terminal> {
    match kind {
        PolicyKind::Direct => Some(Terminal::Direct),
        PolicyKind::Reject => Some(Terminal::Reject(RejectKind::Reject)),
        PolicyKind::RejectDrop => Some(Terminal::Reject(RejectKind::Drop)),
        PolicyKind::RejectNoDrop => Some(Terminal::Reject(RejectKind::NoDrop)),
        PolicyKind::RejectTinyGif => Some(Terminal::Reject(RejectKind::TinyGif)),
        _ => None,
    }
}

impl PolicyRegistry {
    pub fn build(
        cfg: &Config,
        selections: &GroupSelections,
        direct: OutboundRef,
    ) -> PolicyRegistry {
        let mut entries = HashMap::new();
        let mut order = Vec::new();
        for p in &cfg.policies {
            let entry = match alias_terminal(p.kind) {
                Some(t) => Entry::Alias(t),
                None => Entry::Proxy { kind: p.kind },
            };
            entries.insert(p.name.clone(), entry);
            order.push(p.name.clone());
        }
        for g in &cfg.groups {
            let selected = match g.kind {
                GroupKind::Select => selections.get(&g.name).map(str::to_string),
                _ => None,
            };
            entries.insert(
                g.name.clone(),
                Entry::Group {
                    members: g.members.clone(),
                    selected,
                },
            );
            order.push(g.name.clone());
        }
        let rejects = [
            Arc::new(Reject::new(RejectKind::Reject)) as OutboundRef,
            Arc::new(Reject::new(RejectKind::Drop)) as OutboundRef,
            Arc::new(Reject::new(RejectKind::NoDrop)) as OutboundRef,
            Arc::new(Reject::new(RejectKind::TinyGif)) as OutboundRef,
        ];
        PolicyRegistry {
            entries,
            order,
            direct,
            rejects,
        }
    }

    pub fn direct(&self) -> OutboundRef {
        self.direct.clone()
    }

    pub fn reject(&self, kind: RejectKind) -> OutboundRef {
        self.rejects[reject_slot(kind)].clone()
    }

    pub fn names(&self) -> Vec<String> {
        self.order.clone()
    }

    pub fn resolve(&self, policy: &PolicyRef) -> Resolution {
        let mut chain = Vec::new();
        match policy {
            PolicyRef::Builtin(b) => self.builtin(*b, &mut chain),
            PolicyRef::Device(name) => {
                chain.push(format!("DEVICE:{name}"));
                chain.push(RejectKind::Reject.name().to_string());
                Resolution {
                    chain,
                    outbound: self.reject(RejectKind::Reject),
                    unsupported: Some("DEVICE".to_string()),
                }
            }
            PolicyRef::Named(name) => self.named(name, &mut chain, 0),
        }
    }

    fn builtin(&self, b: Builtin, chain: &mut Vec<String>) -> Resolution {
        chain.push(b.name().to_string());
        if b == Builtin::Direct {
            return self.done(chain, self.direct(), None);
        }
        if let Some(kind) = RejectKind::from_builtin(b) {
            return self.done(chain, self.reject(kind), None);
        }
        // CELLULAR / CELLULAR-ONLY / HYBRID / NO-HYBRID: iOS-only, DIRECT on desktop (W0009 at load).
        chain.push("DIRECT".to_string());
        self.done(chain, self.direct(), None)
    }

    fn named(&self, name: &str, chain: &mut Vec<String>, depth: usize) -> Resolution {
        chain.push(name.to_string());
        if depth > MAX_DEPTH {
            tracing::error!(
                policy = name,
                "policy chain deeper than {MAX_DEPTH}; treating as REJECT"
            );
            chain.push(RejectKind::Reject.name().to_string());
            return self.done(chain, self.reject(RejectKind::Reject), None);
        }
        match self.entries.get(name) {
            None => {
                tracing::error!(
                    policy = name,
                    "policy not found in registry; treating as REJECT"
                );
                chain.push(RejectKind::Reject.name().to_string());
                self.done(chain, self.reject(RejectKind::Reject), None)
            }
            Some(Entry::Alias(Terminal::Direct)) => {
                chain.push("DIRECT".to_string());
                self.done(chain, self.direct(), None)
            }
            Some(Entry::Alias(Terminal::Reject(kind))) => {
                chain.push(kind.name().to_string());
                self.done(chain, self.reject(*kind), None)
            }
            Some(Entry::Proxy { kind }) => {
                chain.push(format!("!unsupported:{}", kind.keyword()));
                chain.push(RejectKind::Reject.name().to_string());
                self.done(
                    chain,
                    self.reject(RejectKind::Reject),
                    Some(kind.keyword().to_string()),
                )
            }
            Some(Entry::Group {
                members, selected, ..
            }) => {
                let next = selected
                    .as_deref()
                    .filter(|m| members.iter().any(|x| x == m))
                    .or_else(|| members.first().map(String::as_str));
                match next {
                    Some(member) => match PolicyRef::parse(member) {
                        PolicyRef::Builtin(b) => self.builtin(b, chain),
                        PolicyRef::Device(d) => {
                            chain.push(format!("DEVICE:{d}"));
                            chain.push(RejectKind::Reject.name().to_string());
                            self.done(
                                chain,
                                self.reject(RejectKind::Reject),
                                Some("DEVICE".to_string()),
                            )
                        }
                        PolicyRef::Named(n) => self.named(&n, chain, depth + 1),
                    },
                    None => {
                        tracing::error!(
                            group = name,
                            "policy group has no members; treating as REJECT"
                        );
                        chain.push(RejectKind::Reject.name().to_string());
                        self.done(chain, self.reject(RejectKind::Reject), None)
                    }
                }
            }
        }
    }

    fn done(
        &self,
        chain: &mut Vec<String>,
        outbound: OutboundRef,
        unsupported: Option<String>,
    ) -> Resolution {
        Resolution {
            chain: std::mem::take(chain),
            outbound,
            unsupported,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::BoxFuture;
    use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
    use rurge_proto::{Outbound, OutboundError};
    use std::path::Path;

    struct FakeDirect;

    impl Outbound for FakeDirect {
        fn name(&self) -> &str {
            "DIRECT"
        }
        fn connect_tcp<'a>(
            &'a self,
            _t: &'a Target,
            _o: &'a ConnectOpts,
        ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
            Box::pin(std::future::ready(Err(OutboundError::Timeout)))
        }
    }

    const PROFILE: &str = "[General]\nloglevel = notify\n[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\nD = direct\nBlock = reject-tinygif\n[Proxy Group]\nAuto = url-test, HK, D\nPick = select, HK, D, DIRECT\nOuter = select, Pick, Auto\nEmptyish = select, Block\n[Rule]\nFINAL,Pick\n";

    fn registry(selections: GroupSelections) -> PolicyRegistry {
        let loaded = from_text(PROFILE, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded
                .diagnostics
                .iter()
                .map(|d| d.code)
                .collect::<Vec<_>>()
        );
        PolicyRegistry::build(&loaded.config, &selections, Arc::new(FakeDirect))
    }

    fn chain(r: &Resolution) -> Vec<&str> {
        r.chain.iter().map(String::as_str).collect()
    }

    #[test]
    fn builtins_and_aliases() {
        let reg = registry(GroupSelections::new());
        let d = reg.resolve(&PolicyRef::Builtin(Builtin::Direct));
        assert_eq!(
            (chain(&d), d.outbound.name(), d.unsupported.as_deref()),
            (vec!["DIRECT"], "DIRECT", None)
        );
        let r = reg.resolve(&PolicyRef::Builtin(Builtin::RejectTinyGif));
        assert_eq!(
            (chain(&r), r.outbound.name()),
            (vec!["REJECT-TINYGIF"], "REJECT-TINYGIF")
        );
        let cell = reg.resolve(&PolicyRef::Builtin(Builtin::Cellular));
        assert_eq!(
            (chain(&cell), cell.outbound.name()),
            (vec!["CELLULAR", "DIRECT"], "DIRECT")
        );
        let alias = reg.resolve(&PolicyRef::parse("D"));
        assert_eq!(
            (chain(&alias), alias.outbound.name()),
            (vec!["D", "DIRECT"], "DIRECT")
        );
        let block = reg.resolve(&PolicyRef::parse("Block"));
        assert_eq!(
            (chain(&block), block.outbound.name()),
            (vec!["Block", "REJECT-TINYGIF"], "REJECT-TINYGIF")
        );
        assert_eq!(
            reg.names(),
            vec!["HK", "D", "Block", "Auto", "Pick", "Outer", "Emptyish"]
        );
    }

    #[test]
    fn unsupported_protocols_and_devices_reject() {
        let reg = registry(GroupSelections::new());
        let hk = reg.resolve(&PolicyRef::parse("HK"));
        assert_eq!(chain(&hk), vec!["HK", "!unsupported:ss", "REJECT"]);
        assert_eq!(hk.outbound.name(), "REJECT");
        assert_eq!(hk.unsupported.as_deref(), Some("ss"));
        let dev = reg.resolve(&PolicyRef::parse("DEVICE:Living Room"));
        assert_eq!(chain(&dev), vec!["DEVICE:Living Room", "REJECT"]);
        assert_eq!(dev.unsupported.as_deref(), Some("DEVICE"));
        let missing = reg.resolve(&PolicyRef::Named("Nope".to_string()));
        assert_eq!(chain(&missing), vec!["Nope", "REJECT"]);
    }

    #[test]
    fn groups_follow_selection_or_first_member() {
        // no persisted selection: select → first member (HK, unsupported); url-test → first member
        let reg = registry(GroupSelections::new());
        let pick = reg.resolve(&PolicyRef::parse("Pick"));
        assert_eq!(
            chain(&pick),
            vec!["Pick", "HK", "!unsupported:ss", "REJECT"]
        );
        let auto = reg.resolve(&PolicyRef::parse("Auto"));
        assert_eq!(
            chain(&auto),
            vec!["Auto", "HK", "!unsupported:ss", "REJECT"]
        );
        // persisted selections are honoured through nesting; a stale selection falls back to the first member
        let mut sel = GroupSelections::new();
        sel.set("Pick", "DIRECT");
        sel.set("Outer", "Pick");
        sel.set("Auto", "D"); // ignored: not a select group
        sel.set("Emptyish", "Gone"); // not a member any more
        let reg = registry(sel);
        let pick = reg.resolve(&PolicyRef::parse("Pick"));
        assert_eq!(
            (chain(&pick), pick.outbound.name()),
            (vec!["Pick", "DIRECT"], "DIRECT")
        );
        let outer = reg.resolve(&PolicyRef::parse("Outer"));
        assert_eq!(chain(&outer), vec!["Outer", "Pick", "DIRECT"]);
        let auto = reg.resolve(&PolicyRef::parse("Auto"));
        assert_eq!(chain(&auto)[1], "HK");
        let e = reg.resolve(&PolicyRef::parse("Emptyish"));
        assert_eq!(chain(&e), vec!["Emptyish", "Block", "REJECT-TINYGIF"]);
        assert!(e.unsupported.is_none());
    }

    #[test]
    fn selections_api() {
        let mut s = GroupSelections::new();
        assert!(s.is_empty());
        s.set("G", "A");
        assert_eq!(s.get("G"), Some("A"));
        assert_eq!(s.get("X"), None);
        let s2 = GroupSelections::from_map(HashMap::from([("G".to_string(), "B".to_string())]));
        assert_eq!(s2.get("G"), Some("B"));
    }
}
