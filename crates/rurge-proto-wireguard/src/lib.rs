//! The `wireguard` outbound (phase 2 M4 design §6): a userspace WireGuard
//! tunnel — boringtun's sans-IO `Tunn` for each peer and a smoltcp TCP/IP
//! stack of its own — that TCP connections are dialled through.

pub mod routes;
pub mod stack;
pub mod wire;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use routes::Routes;
pub use stack::{Outgoing, Refusal, Stack};
