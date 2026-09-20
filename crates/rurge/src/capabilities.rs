//! What this build of rurge actually implements: the built-in alias
//! policies, the HTTP / SOCKS5 proxy family (phase 2 M1), `trojan`
//! (phase 2 M2a), `vmess` with the AEAD handshake and `anytls` (phase 2
//! M2b), and `select` groups.

use rurge_config::config::Capabilities;
use rurge_config::policy::{GroupKind, PolicyKind};
use std::collections::HashSet;

/// Reported as `CORE_VERSION` to requirement expressions (see FR-CFG-08).
pub const CORE_VERSION: u64 = 20;

const INACTIVE_RULE_TYPES: [&str; 7] = [
    "PROCESS-NAME",
    "SCRIPT",
    "DEVICE-NAME",
    "MAC-ADDRESS",
    "SUBNET",
    "CELLULAR-RADIO",
    "CELLULAR-CARRIER",
];

pub fn current() -> Capabilities {
    Capabilities {
        policy_kinds: HashSet::from([
            PolicyKind::Direct,
            PolicyKind::Reject,
            PolicyKind::RejectDrop,
            PolicyKind::RejectNoDrop,
            PolicyKind::RejectTinyGif,
            PolicyKind::Http,
            PolicyKind::Https,
            PolicyKind::Socks5,
            PolicyKind::Socks5Tls,
            PolicyKind::Trojan,
            PolicyKind::Vmess,
            PolicyKind::AnyTls,
        ]),
        group_kinds: HashSet::from([GroupKind::Select]),
        rule_types: Capabilities::ALL_RULE_TYPES
            .iter()
            .copied()
            .filter(|t| !INACTIVE_RULE_TYPES.contains(t))
            .collect(),
    }
}
