//! What outlives a config generation (M1 design 6.2, 6.3).

use rurge_policy::{GroupSelections, RegistryCell, SelectionTable};
use std::sync::Arc;

/// Created once per engine — before the first `Runtime::build`, because the
/// registry built there already needs both — and handed to every later
/// `Runtime::build` of the same engine (`Engine::shared`).
#[derive(Clone)]
pub struct EngineShared {
    /// Where chain connectors find the current generation's registry.
    pub cell: Arc<RegistryCell>,
    /// The live `select` choices of the running profile.
    pub selections: Arc<SelectionTable>,
}

impl EngineShared {
    pub fn new(initial: GroupSelections) -> EngineShared {
        EngineShared {
            cell: RegistryCell::new(),
            selections: Arc::new(SelectionTable::new(initial)),
        }
    }
}

impl Default for EngineShared {
    fn default() -> EngineShared {
        EngineShared::new(GroupSelections::new())
    }
}
