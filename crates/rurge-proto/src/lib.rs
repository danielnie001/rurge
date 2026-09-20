//! Outbound abstraction (M3 design §4): the `Outbound` trait every policy
//! implements, plus the phase 1 built-ins `Direct` and `Reject`.

mod addr;
pub mod anytls;
pub mod build;
pub mod direct;
mod hostname;
pub mod http;
pub mod keystore;
pub mod outbound;
pub mod reject;
pub mod socks5;
mod task;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod transport;
pub mod trojan;
pub mod vmess;

pub use build::BuildError;
pub use direct::Direct;
pub use outbound::{HttpForward, Outbound, OutboundError, OutboundRef, RejectKind};
pub use reject::Reject;
