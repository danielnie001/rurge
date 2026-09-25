//! Policy registry (M3 design §5, M1 design 6.1 – 6.3): resolves a `PolicyRef`
//! through aliases and groups to a concrete `Outbound`, recording the chain
//! it took; the factory trait real outbounds come from; the cell and the
//! selection table that outlive a config generation; what a `policy-path`
//! subscription holds and the members a group assembles from everything it
//! takes in (phase 2 M3 design 5.2, 5.3); and the connectivity tests and the
//! automatic groups that pick by them (`probe`, `testbook`, `auto`; phase 2
//! M3 design §6).

pub mod assemble;
pub mod auto;
pub mod cell;
pub mod factory;
pub mod probe;
pub mod registry;
pub mod selections;
pub mod subscription;
pub mod testbook;
#[cfg(test)]
pub(crate) mod testing;

pub use assemble::{Assembly, Snapshots, assemble};
pub use cell::{ChainConnector, RegistryCell};
pub use factory::{BuildError, OutboundFactory};
pub use registry::{EmptyGroup, GroupInfo, Line, Note, PolicyRegistry, Resolution, TerminalKind};
pub use selections::{GroupSelections, SelectionTable};
