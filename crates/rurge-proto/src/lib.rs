//! Outbound abstraction (M3 design §4): the `Outbound` trait every policy
//! implements, plus the phase 1 built-ins `Direct` and `Reject`.

pub mod build;
pub mod direct;
pub mod keystore;
pub mod outbound;
pub mod reject;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod transport;

pub use build::BuildError;
pub use direct::Direct;
pub use outbound::{HttpForward, Outbound, OutboundError, OutboundRef, RejectKind};
pub use reject::Reject;
