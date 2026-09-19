//! Policy registry (M3 design §5, M1 design 6.1 – 6.3): resolves a `PolicyRef`
//! through aliases and groups to a concrete `Outbound`, recording the chain
//! it took; the factory trait real outbounds come from; the cell and the
//! selection table that outlive a config generation.

pub mod cell;
pub mod factory;
pub mod registry;
pub mod selections;
#[cfg(test)]
pub(crate) mod testing;

pub use cell::{ChainConnector, RegistryCell};
pub use factory::{BuildError, OutboundFactory};
pub use registry::{Note, PolicyRegistry, Resolution, TerminalKind};
pub use selections::{GroupSelections, SelectionTable};
