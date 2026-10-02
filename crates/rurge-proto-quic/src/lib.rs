//! The outbounds over QUIC (phase 2 M7 design): a policy keeps one QUIC
//! connection, which its requests share (`session`).

pub mod session;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use session::{Dialer, Failure, Lease, Protocol, Session};
