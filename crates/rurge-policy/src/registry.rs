//! Name → outbound resolution (M3 design §5, M1 design 6.2). Built once per
//! config generation; `resolve` is a table walk with no allocation beyond the
//! chain and the group selections it reads.

use crate::cell::{ChainConnector, RegistryCell};
use crate::factory::{BuildError, OutboundFactory};
use crate::selections::SelectionTable;
use rurge_config::rule::PolicyRef;
use rurge_config::spec::{CommonOpts, IpVersion, PolicySpec};
use rurge_config::{Builtin, Config, GroupKind, KeystoreType, PolicyKind, Span};
use rurge_net::connector::Connector;
use rurge_proto::{Direct, OutboundRef, Reject, RejectKind};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Deeper chains than this are treated as a defect (group cycles are load errors).
pub const MAX_DEPTH: usize = 16;

/// What kind of outbound a resolution ended at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalKind {
    Direct,
    Reject,
    Proxy,
}

/// Why a resolution ended where it did, when that needs saying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// The terminal policy's protocol keyword (or `DEVICE`) is not
    /// implemented in this version: the outbound is REJECT.
    Unsupported(String),
}

#[derive(Clone)]
pub struct Resolution {
    pub chain: Vec<String>,
    pub outbound: OutboundRef,
    pub terminal: TerminalKind,
    pub note: Option<Note>,
}

enum Terminal {
    Direct,
    Reject(RejectKind),
}

enum Entry {
    /// A `direct` / `reject*` alias without options of its own.
    Alias(Terminal),
    /// A built proxy, or a `direct` alias with socket options.
    Outbound {
        outbound: OutboundRef,
        proxy: bool,
        // Boxed: `Fingerprint` embeds a whole `PolicySpec`, which would
        // otherwise make this variant much larger than the others.
        fingerprint: Box<Fingerprint>,
    },
    /// The protocol is not implemented yet: REJECT (W0007 at load).
    Unsupported { kind: PolicyKind },
    Group {
        kind: GroupKind,
        members: Vec<String>,
    },
}

pub struct PolicyRegistry {
    entries: HashMap<String, Entry>,
    order: Vec<String>,
    direct: OutboundRef,
    rejects: [OutboundRef; 4],
    selections: Arc<SelectionTable>,
}

fn reject_slot(kind: RejectKind) -> usize {
    match kind {
        RejectKind::Reject => 0,
        RejectKind::Drop => 1,
        RejectKind::NoDrop => 2,
        RejectKind::TinyGif => 3,
    }
}

/// What the session log says about a policy that has no spec. Since M2b a
/// `vmess` policy of a profile that loaded is only ever without one for a
/// single reason: the line lacks `vmess-aead=true` (M2 design 4.3).
fn unsupported_text(kind: PolicyKind) -> String {
    match kind {
        PolicyKind::Vmess => "vmess (legacy handshake)".to_string(),
        other => other.keyword().to_string(),
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

/// Whether a `direct` alias needs a connector of its own.
/// `allow_other_interface` is not part of the predicate: it only says what to
/// do when `interface` is unavailable, so on its own it changes nothing.
fn has_socket_opts(common: &CommonOpts) -> bool {
    common.interface.is_some() || common.tos != 0 || common.ip_version != IpVersion::default()
}

/// Everything an outbound was built from. Two generations that agree on it
/// may share the outbound (M2 design 7.1). No `Debug`: it holds the policy's
/// credentials and the keystore item's.
#[derive(PartialEq)]
struct Fingerprint {
    /// Without its span: an unrelated edit above the line moves it.
    spec: PolicySpec,
    /// `client-cert`'s keystore item, by content.
    keystore: Option<(KeystoreType, String, Option<String>)>,
    environment: String,
}

fn fingerprint(spec: &PolicySpec, cfg: &Config, environment: &str) -> Fingerprint {
    let mut spec = spec.clone();
    spec.span = Span::new(Arc::from(Path::new("")), 0);
    let keystore = spec
        .proto
        .tls()
        .and_then(|tls| tls.client_cert.as_ref())
        .and_then(|name| cfg.keystore.iter().find(|item| &item.name == name))
        .map(|item| (item.kind, item.base64.clone(), item.password.clone()));
    Fingerprint {
        spec,
        keystore,
        environment: environment.to_string(),
    }
}

fn build_one(
    spec: &PolicySpec,
    factory: &dyn OutboundFactory,
    cell: &Arc<RegistryCell>,
) -> Result<OutboundRef, BuildError> {
    let connector: Arc<dyn Connector> = match spec.common.underlying_proxy.as_deref() {
        // This policy's own `interface` / `allow-other-interface` / `tos` /
        // `ip-version` have no effect here: socket options belong to the hop
        // that opens the socket, which is `name` or a hop below it.
        Some(name) => Arc::new(ChainConnector::new(cell.clone(), name)),
        None => factory.direct_connector(&spec.common),
    };
    factory
        .build(spec, connector)
        .map_err(|e| BuildError::new(format!("policy `{}`: {}", spec.name, e.message)))
}

impl PolicyRegistry {
    /// `cell` is where the chain connectors built here will look the
    /// registry up at dial time; the caller stores the result into it.
    /// `previous` is the generation being replaced: a policy whose
    /// fingerprint did not change keeps the outbound it had there, pools and
    /// all (M2 design 7.1).
    pub fn build(
        cfg: &Config,
        factory: &dyn OutboundFactory,
        cell: &Arc<RegistryCell>,
        selections: Arc<SelectionTable>,
        previous: Option<&PolicyRegistry>,
    ) -> Result<PolicyRegistry, BuildError> {
        let direct: OutboundRef = Arc::new(Direct::new(
            factory.direct_connector(&CommonOpts::default()),
        ));
        let mut entries = HashMap::new();
        let mut order = Vec::new();
        let environment = factory.environment();
        let outbound_entry = |spec: &PolicySpec, proxy: bool| -> Result<Entry, BuildError> {
            let fingerprint = fingerprint(spec, cfg, &environment);
            let kept = match previous.and_then(|p| p.entries.get(&spec.name)) {
                Some(Entry::Outbound {
                    outbound,
                    fingerprint: before,
                    ..
                }) if **before == fingerprint => Some(outbound.clone()),
                _ => None,
            };
            let outbound = match kept {
                Some(outbound) => outbound,
                None => build_one(spec, factory, cell)?,
            };
            Ok(Entry::Outbound {
                outbound,
                proxy,
                fingerprint: Box::new(fingerprint),
            })
        };
        for p in &cfg.policies {
            let entry = match (alias_terminal(p.kind), cfg.spec(&p.name)) {
                (Some(Terminal::Direct), Some(spec)) if has_socket_opts(&spec.common) => {
                    outbound_entry(spec, false)?
                }
                (Some(terminal), _) => Entry::Alias(terminal),
                (None, Some(spec)) => outbound_entry(spec, true)?,
                // no spec: a protocol of a later milestone
                (None, None) => Entry::Unsupported { kind: p.kind },
            };
            entries.insert(p.name.clone(), entry);
            order.push(p.name.clone());
        }
        for g in &cfg.groups {
            entries.insert(
                g.name.clone(),
                Entry::Group {
                    kind: g.kind,
                    members: g.members.clone(),
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
        Ok(PolicyRegistry {
            entries,
            order,
            direct,
            rejects,
            selections,
        })
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

    /// Non-allocating membership check: every configured policy / group name
    /// (not builtins). Used on the per-connection dial path, where cloning
    /// the whole table via `names()` would allocate for every session.
    pub fn contains(&self, name: &str) -> bool {
        self.order.iter().any(|n| n == name)
    }

    /// The member `group` points at right now: the live selection of a
    /// `select` group when it still names a member, else the first member.
    /// `None` when `group` is not a group or has no members.
    pub fn current_member(&self, group: &str) -> Option<String> {
        let Some(Entry::Group { kind, members }) = self.entries.get(group) else {
            return None;
        };
        let selected = (*kind == GroupKind::Select)
            .then(|| self.selections.get(group))
            .flatten()
            .filter(|m| members.contains(m));
        selected.or_else(|| members.first().cloned())
    }

    pub fn resolve(&self, policy: &PolicyRef) -> Resolution {
        let mut chain = Vec::new();
        match policy {
            PolicyRef::Builtin(b) => self.builtin(*b, &mut chain),
            PolicyRef::Device(name) => self.device(name, &mut chain),
            PolicyRef::Named(name) => self.named(name, &mut chain, 0),
        }
    }

    fn device(&self, name: &str, chain: &mut Vec<String>) -> Resolution {
        chain.push(format!("DEVICE:{name}"));
        self.rejected(chain, Some(Note::Unsupported("DEVICE".to_string())))
    }

    fn builtin(&self, b: Builtin, chain: &mut Vec<String>) -> Resolution {
        chain.push(b.name().to_string());
        if b == Builtin::Direct {
            return self.done(chain, self.direct(), TerminalKind::Direct, None);
        }
        if let Some(kind) = RejectKind::from_builtin(b) {
            return self.done(chain, self.reject(kind), TerminalKind::Reject, None);
        }
        // CELLULAR / CELLULAR-ONLY / HYBRID / NO-HYBRID: iOS-only, DIRECT on desktop (W0009 at load).
        chain.push("DIRECT".to_string());
        self.done(chain, self.direct(), TerminalKind::Direct, None)
    }

    fn named(&self, name: &str, chain: &mut Vec<String>, depth: usize) -> Resolution {
        chain.push(name.to_string());
        if depth > MAX_DEPTH {
            tracing::error!(
                policy = name,
                "policy chain deeper than {MAX_DEPTH}; treating as REJECT"
            );
            return self.rejected(chain, None);
        }
        match self.entries.get(name) {
            None => {
                tracing::error!(
                    policy = name,
                    "policy not found in registry; treating as REJECT"
                );
                self.rejected(chain, None)
            }
            Some(Entry::Alias(Terminal::Direct)) => {
                chain.push("DIRECT".to_string());
                self.done(chain, self.direct(), TerminalKind::Direct, None)
            }
            Some(Entry::Alias(Terminal::Reject(kind))) => {
                chain.push(kind.name().to_string());
                self.done(chain, self.reject(*kind), TerminalKind::Reject, None)
            }
            Some(Entry::Outbound {
                outbound,
                proxy: true,
                ..
            }) => self.done(chain, outbound.clone(), TerminalKind::Proxy, None),
            Some(Entry::Outbound {
                outbound,
                proxy: false,
                ..
            }) => {
                chain.push("DIRECT".to_string());
                self.done(chain, outbound.clone(), TerminalKind::Direct, None)
            }
            Some(Entry::Unsupported { kind }) => {
                chain.push(format!("!unsupported:{}", kind.keyword()));
                self.rejected(chain, Some(Note::Unsupported(unsupported_text(*kind))))
            }
            Some(Entry::Group { .. }) => match self.current_member(name) {
                Some(member) => match PolicyRef::parse(&member) {
                    PolicyRef::Builtin(b) => self.builtin(b, chain),
                    PolicyRef::Device(d) => self.device(&d, chain),
                    PolicyRef::Named(n) => self.named(&n, chain, depth + 1),
                },
                None => {
                    tracing::error!(
                        group = name,
                        "policy group has no members; treating as REJECT"
                    );
                    self.rejected(chain, None)
                }
            },
        }
    }

    fn rejected(&self, chain: &mut Vec<String>, note: Option<Note>) -> Resolution {
        chain.push(RejectKind::Reject.name().to_string());
        self.done(
            chain,
            self.reject(RejectKind::Reject),
            TerminalKind::Reject,
            note,
        )
    }

    fn done(
        &self,
        chain: &mut Vec<String>,
        outbound: OutboundRef,
        terminal: TerminalKind,
        note: Option<Note>,
    ) -> Resolution {
        Resolution {
            chain: std::mem::take(chain),
            outbound,
            terminal,
            note,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selections::GroupSelections;
    use crate::testing::FakeFactory;
    use rurge_config::HostName;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::connector::{ConnectOpts, Target};
    use std::path::Path;

    const PROFILE: &str = "[General]\nloglevel = notify\n[Proxy]\n\
HK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n\
D = direct\nCorp = direct, interface=eth9\nBlock = reject-tinygif\n\
EntryA = socks5, a.example, 1080\nEntryB = socks5, b.example, 1080\n\
Exit = http, exit.example, 8080, underlying-proxy=Hop\n\
Mid = socks5, m.example, 1080, underlying-proxy=EntryA\n\
Deep = http, d.example, 80, underlying-proxy=Mid\n\
[Proxy Group]\nAuto = url-test, HK, D\nPick = select, HK, D, DIRECT\nOuter = select, Pick, Auto\n\
Emptyish = select, Block\nHop = select, EntryA, EntryB\n[Rule]\nFINAL,Pick\n";

    struct Built {
        registry: Arc<PolicyRegistry>,
        factory: FakeFactory,
        table: Arc<SelectionTable>,
    }

    fn built(selections: GroupSelections) -> Built {
        let loaded = from_text(PROFILE, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        let factory = FakeFactory::new();
        let cell = RegistryCell::new();
        let table = Arc::new(SelectionTable::new(selections));
        let registry = Arc::new(
            PolicyRegistry::build(&loaded.config, &factory, &cell, table.clone(), None)
                .expect("builds"),
        );
        cell.store(registry.clone());
        Built {
            registry,
            factory,
            table,
        }
    }

    fn chain(r: &Resolution) -> Vec<&str> {
        r.chain.iter().map(String::as_str).collect()
    }

    #[test]
    fn builtins_and_aliases() {
        let reg = built(GroupSelections::new()).registry;
        let d = reg.resolve(&PolicyRef::Builtin(Builtin::Direct));
        assert_eq!(
            (chain(&d), d.outbound.name(), d.terminal, d.note.clone()),
            (vec!["DIRECT"], "DIRECT", TerminalKind::Direct, None)
        );
        let r = reg.resolve(&PolicyRef::Builtin(Builtin::RejectTinyGif));
        assert_eq!(
            (chain(&r), r.outbound.name(), r.terminal),
            (
                vec!["REJECT-TINYGIF"],
                "REJECT-TINYGIF",
                TerminalKind::Reject
            )
        );
        let cell = reg.resolve(&PolicyRef::Builtin(Builtin::Cellular));
        assert_eq!(
            (chain(&cell), cell.outbound.name()),
            (vec!["CELLULAR", "DIRECT"], "DIRECT")
        );
        // a plain alias shares the built-in DIRECT …
        let alias = reg.resolve(&PolicyRef::parse("D"));
        assert_eq!(chain(&alias), vec!["D", "DIRECT"]);
        assert!(Arc::ptr_eq(&alias.outbound, &reg.direct()));
        // … an alias with socket options owns its outbound
        let corp = reg.resolve(&PolicyRef::parse("Corp"));
        assert_eq!(
            (chain(&corp), corp.outbound.name(), corp.terminal),
            (vec!["Corp", "DIRECT"], "DIRECT", TerminalKind::Direct)
        );
        assert!(!Arc::ptr_eq(&corp.outbound, &reg.direct()));
        let block = reg.resolve(&PolicyRef::parse("Block"));
        assert_eq!(
            (chain(&block), block.outbound.name(), block.terminal),
            (
                vec!["Block", "REJECT-TINYGIF"],
                "REJECT-TINYGIF",
                TerminalKind::Reject
            )
        );
        assert_eq!(
            reg.names(),
            vec![
                "HK", "D", "Corp", "Block", "EntryA", "EntryB", "Exit", "Mid", "Deep", "Auto",
                "Pick", "Outer", "Emptyish", "Hop"
            ]
        );
        assert!(reg.contains("HK") && !reg.contains("Nope"));
    }

    #[test]
    fn a_proxy_policy_resolves_to_its_own_outbound() {
        let reg = built(GroupSelections::new()).registry;
        let a = reg.resolve(&PolicyRef::parse("EntryA"));
        assert_eq!(
            (chain(&a), a.outbound.name(), a.terminal, a.note.clone()),
            (vec!["EntryA"], "EntryA", TerminalKind::Proxy, None)
        );
        let hop = reg.resolve(&PolicyRef::parse("Hop"));
        assert_eq!(
            (chain(&hop), hop.terminal),
            (vec!["Hop", "EntryA"], TerminalKind::Proxy)
        );
    }

    #[test]
    fn unsupported_protocols_and_devices_reject_with_a_note() {
        let reg = built(GroupSelections::new()).registry;
        let hk = reg.resolve(&PolicyRef::parse("HK"));
        assert_eq!(chain(&hk), vec!["HK", "!unsupported:ss", "REJECT"]);
        assert_eq!(
            (hk.outbound.name(), hk.terminal, hk.note.clone()),
            (
                "REJECT",
                TerminalKind::Reject,
                Some(Note::Unsupported("ss".into()))
            )
        );
        let dev = reg.resolve(&PolicyRef::parse("DEVICE:Living Room"));
        assert_eq!(chain(&dev), vec!["DEVICE:Living Room", "REJECT"]);
        assert_eq!(dev.note, Some(Note::Unsupported("DEVICE".into())));
        let missing = reg.resolve(&PolicyRef::Named("Nope".to_string()));
        assert_eq!(
            (chain(&missing), missing.note.clone()),
            (vec!["Nope", "REJECT"], None)
        );
        // a group falling through to an unsupported member keeps the whole
        // chain, whichever kind of group it is
        assert_eq!(
            chain(&reg.resolve(&PolicyRef::parse("Pick"))),
            vec!["Pick", "HK", "!unsupported:ss", "REJECT"]
        );
        assert_eq!(
            chain(&reg.resolve(&PolicyRef::parse("Auto"))),
            vec!["Auto", "HK", "!unsupported:ss", "REJECT"]
        );
    }

    #[test]
    fn a_legacy_vmess_policy_says_why_it_rejects() {
        let text = "[Proxy]\nOld = vmess, a.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119\n\
SS = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n";
        let registry = generation(text, &FakeFactory::new(), None);
        let old = registry.resolve(&PolicyRef::parse("Old"));
        assert_eq!(old.terminal, TerminalKind::Reject);
        assert_eq!(
            old.note,
            Some(Note::Unsupported("vmess (legacy handshake)".into()))
        );
        assert_eq!(chain(&old), ["Old", "!unsupported:vmess", "REJECT"]);
        let ss = registry.resolve(&PolicyRef::parse("SS"));
        assert_eq!(ss.note, Some(Note::Unsupported("ss".into())));
    }

    #[test]
    fn groups_read_the_live_table() {
        let mut saved = GroupSelections::new();
        saved.set("Pick", "DIRECT");
        saved.set("Outer", "Pick");
        saved.set("Auto", "D"); // ignored: not a select group
        saved.set("Emptyish", "Gone"); // not a member any more
        let b = built(saved);
        let reg = &b.registry;
        let picked = reg.resolve(&PolicyRef::parse("Pick"));
        assert_eq!(chain(&picked), vec!["Pick", "DIRECT"]);
        assert_eq!(picked.outbound.name(), "DIRECT");
        assert_eq!(
            chain(&reg.resolve(&PolicyRef::parse("Outer"))),
            vec!["Outer", "Pick", "DIRECT"]
        );
        assert_eq!(chain(&reg.resolve(&PolicyRef::parse("Auto")))[1], "HK");
        let emptyish = reg.resolve(&PolicyRef::parse("Emptyish"));
        assert_eq!(
            (chain(&emptyish), emptyish.note.clone()),
            (vec!["Emptyish", "Block", "REJECT-TINYGIF"], None)
        );
        assert_eq!(reg.current_member("Pick").as_deref(), Some("DIRECT"));
        assert_eq!(reg.current_member("Emptyish").as_deref(), Some("Block"));
        assert_eq!(reg.current_member("Auto").as_deref(), Some("HK"));
        assert_eq!(reg.current_member("D"), None, "not a group");
        // no rebuild: the very next resolve sees the change
        b.table.set("Pick", "D");
        assert_eq!(
            chain(&reg.resolve(&PolicyRef::parse("Pick"))),
            vec!["Pick", "D", "DIRECT"]
        );
        assert_eq!(reg.current_member("Pick").as_deref(), Some("D"));
    }

    /// M1 design §8: switch the selection between two dials and the entry
    /// node of the chain follows.
    #[tokio::test]
    async fn the_entry_of_a_chain_follows_the_group_selection() {
        let b = built(GroupSelections::new());
        let exit = b.registry.resolve(&PolicyRef::parse("Exit"));
        assert_eq!(
            (chain(&exit), exit.terminal),
            (vec!["Exit"], TerminalKind::Proxy)
        );
        let target = Target::new(HostName::parse("site.example"), 443);
        exit.outbound
            .connect_tcp(&target, &ConnectOpts::default())
            .await
            .unwrap();
        b.table.set("Hop", "EntryB");
        exit.outbound
            .connect_tcp(&target, &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(
            b.factory.connector.seen(),
            [
                "Exit -> site.example:443",
                // the exit's server is reached through the entry, by name
                "EntryA -> exit.example:8080",
                "dial a.example:1080",
                "Exit -> site.example:443",
                "EntryB -> exit.example:8080",
                "dial b.example:1080",
            ]
        );
    }

    /// Three hops: each one dials the hop below it, and only the bottom hop
    /// reaches a real socket — with its own server as the target.
    #[tokio::test]
    async fn a_three_hop_chain_hands_each_server_to_the_hop_below() {
        let b = built(GroupSelections::new());
        let deep = b.registry.resolve(&PolicyRef::parse("Deep"));
        assert_eq!(
            (chain(&deep), deep.terminal),
            (vec!["Deep"], TerminalKind::Proxy)
        );
        deep.outbound
            .connect_tcp(
                &Target::new(HostName::parse("site.example"), 443),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            b.factory.connector.seen(),
            [
                "Deep -> site.example:443",
                "Mid -> d.example:80",
                "EntryA -> m.example:1080",
                "dial a.example:1080",
            ]
        );
    }

    #[test]
    fn a_policy_that_cannot_be_built_fails_the_whole_registry() {
        let loaded = from_text(PROFILE, Path::new("t.conf"), &LoadOptions::for_tests());
        let factory = FakeFactory {
            broken: Some("EntryB"),
            ..FakeFactory::new()
        };
        let e = PolicyRegistry::build(
            &loaded.config,
            &factory,
            &RegistryCell::new(),
            Arc::new(SelectionTable::default()),
            None,
        )
        .err()
        .expect("EntryB does not build");
        assert_eq!(e.message, "policy `EntryB`: boom");
    }

    fn generation(
        text: &str,
        factory: &FakeFactory,
        previous: Option<&PolicyRegistry>,
    ) -> PolicyRegistry {
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        PolicyRegistry::build(
            &loaded.config,
            factory,
            &RegistryCell::new(),
            Arc::new(SelectionTable::new(GroupSelections::new())),
            previous,
        )
        .expect("builds")
    }

    fn outbound_of(registry: &PolicyRegistry, name: &str) -> OutboundRef {
        registry.resolve(&PolicyRef::parse(name)).outbound
    }

    const REUSE: &str = "[Proxy]\nA = socks5, a.example, 1080, username=u, password=p\n\
B = https, b.example, 443, client-cert=cert1\nCorp = direct, interface=eth9\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=x\n[Rule]\nFINAL,DIRECT\n";

    #[test]
    fn an_untouched_policy_keeps_its_outbound_across_a_reload() {
        let factory = FakeFactory::new();
        let first = generation(REUSE, &factory, None);
        // an unrelated line is added above: every span moves, nothing else does
        let moved = REUSE.replace("[Proxy]\n", "[Proxy]\nNew = http, n.example, 80\n");
        let second = generation(&moved, &factory, Some(&first));
        for name in ["A", "B", "Corp"] {
            assert!(
                Arc::ptr_eq(&outbound_of(&first, name), &outbound_of(&second, name)),
                "{name} was rebuilt"
            );
        }
        // without a previous generation everything is new
        let alone = generation(&moved, &factory, None);
        assert!(!Arc::ptr_eq(
            &outbound_of(&first, "A"),
            &outbound_of(&alone, "A")
        ));
    }

    #[test]
    fn a_changed_parameter_keystore_item_or_environment_rebuilds() {
        let factory = FakeFactory::new();
        let first = generation(REUSE, &factory, None);
        // A's own parameter
        let second = generation(
            &REUSE.replace("password=p", "password=q"),
            &factory,
            Some(&first),
        );
        assert!(!Arc::ptr_eq(
            &outbound_of(&first, "A"),
            &outbound_of(&second, "A")
        ));
        assert!(Arc::ptr_eq(
            &outbound_of(&first, "B"),
            &outbound_of(&second, "B")
        ));
        // the content of the keystore item B refers to
        let second = generation(
            &REUSE.replace("base64=QUJD", "base64=QUJE"),
            &factory,
            Some(&first),
        );
        assert!(!Arc::ptr_eq(
            &outbound_of(&first, "B"),
            &outbound_of(&second, "B")
        ));
        assert!(Arc::ptr_eq(
            &outbound_of(&first, "A"),
            &outbound_of(&second, "A")
        ));
        // what the factory captured by value
        let other = FakeFactory {
            environment: "other",
            ..FakeFactory::new()
        };
        let second = generation(REUSE, &other, Some(&first));
        for name in ["A", "B", "Corp"] {
            assert!(
                !Arc::ptr_eq(&outbound_of(&first, name), &outbound_of(&second, name)),
                "{name} survived a change of environment"
            );
        }
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
