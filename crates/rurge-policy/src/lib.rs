//! Policy registry (M3 design §5): resolves a `PolicyRef` through aliases and
//! groups to a concrete `Outbound`, recording the chain it took.

pub mod registry;
pub mod selections;

pub use registry::{PolicyRegistry, Resolution};
pub use selections::GroupSelections;
