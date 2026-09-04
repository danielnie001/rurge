//! Rules flagged `pre-matching` (M2 design §6.5): REJECT-family decisions M3
//! applies at the DNS / SYN stage. The config layer already guarantees that
//! such rules carry a REJECT-family policy and a supported rule type.

use crate::engine::CompiledRule;
use rurge_config::rule::PolicyRef;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreMatch {
    /// Index into `Config.rules`.
    pub rule: usize,
    pub policy: PolicyRef,
}

pub struct PreMatchingSet {
    /// Positions into the engine's compiled rule list, in rule order.
    positions: Vec<usize>,
}

impl PreMatchingSet {
    pub(crate) fn extract(rules: &[CompiledRule]) -> PreMatchingSet {
        PreMatchingSet {
            positions: rules
                .iter()
                .enumerate()
                .filter(|(_, r)| r.params.pre_matching)
                .map(|(p, _)| p)
                .collect(),
        }
    }

    pub(crate) fn positions(&self) -> &[usize] {
        &self.positions
    }

    pub fn len(&self) -> usize {
        self.positions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }
}
