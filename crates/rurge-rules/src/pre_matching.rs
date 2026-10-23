//! Rules flagged `pre-matching` (M2 design §6.5): REJECT-family decisions that
//! are taken ahead of everything else, without resolving. The config layer
//! already guarantees that such rules carry a REJECT-family policy and a
//! supported rule type.

use crate::engine::CompiledRule;
use crate::matcher::SubRuleHit;
use rurge_config::rule::PolicyRef;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreMatch {
    /// Index into `Config.rules`.
    pub rule: usize,
    pub policy: PolicyRef,
    /// The line of the set that made the rule match, when the rule refers to
    /// one.
    pub sub_rule: Option<SubRuleHit>,
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
