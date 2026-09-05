//! Inbound listeners (M3 design §6): HTTP/1.1 proxy (CONNECT and plain
//! forwarding) on hyper, SOCKS5 by hand. Listeners only speak the protocol;
//! rules, policies, outbound connections and relaying belong to the engine,
//! reached through the `Dialer` trait.

pub mod listener;
pub mod responses;
pub mod restrict;
pub mod session;
#[cfg(test)]
pub(crate) mod testing;

pub use listener::{HttpAuth, ListenerOpts, Running};
pub use session::{Counting, DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome};
