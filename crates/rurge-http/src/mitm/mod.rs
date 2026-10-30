//! MITM: what rurge decrypts, the certificate authority it signs with, and
//! the handshake with the client.
//!
//! `authority` brings the CA of `[MITM]`: where it comes from, what makes it
//! unusable and the `W0039` a profile load reports (`phase 4 M2 design 5`),
//! then the certificate it issues per name and caches (`phase 4 M2 design
//! 6`). `decrypt` is when a connection is decrypted at all, the server-side
//! configuration, and the handshake with the client (`phase 4 M2 design 7`,
//! `8`).

pub mod authority;
pub mod decrypt;

#[cfg(test)]
mod fixtures;

#[cfg(test)]
mod testing;

pub use authority::{
    Authority, BACKDATE_DAYS, CaProblem, LEAF_DAYS, LEAF_SLOTS, LeafError, SESSION_SLOTS, check,
};
pub use decrypt::{Accepted, Decision, HandshakeFailure, Mitm, OriginTls, Watch, source_of};
