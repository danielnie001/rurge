//! The outbounds over QUIC (phase 2 M7 design): a policy keeps one QUIC
//! connection, which its requests share (`session`) — Hysteria 2's
//! (`hysteria2`) and TUIC's (`tuic`).

pub mod hysteria2;
pub mod session;
#[cfg(test)]
mod test_log;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod tuic;
mod varint;

pub use hysteria2::Hysteria2Outbound;
pub use session::{Dialer, Failure, Layer, Lease, Protocol, Session};
pub use tuic::TuicOutbound;
