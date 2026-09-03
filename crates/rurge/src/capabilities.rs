//! What this build of rurge actually implements. Milestone 1: parsing only,
//! so only the built-in alias policies and `select` groups are "implemented".

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
        ]),
        group_kinds: HashSet::from([GroupKind::Select]),
        rule_types: Capabilities::ALL_RULE_TYPES
            .iter()
            .copied()
            .filter(|t| !INACTIVE_RULE_TYPES.contains(t))
            .collect(),
    }
}
