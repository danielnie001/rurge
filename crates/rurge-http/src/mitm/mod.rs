//! MITM: what rurge decrypts, and the certificate authority it signs with.
//!
//! `authority` brings the CA of `[MITM]`: where it comes from, what makes it
//! unusable and the `W0039` a profile load reports (`phase 4 M2 design 5`).
//! The rest of the milestone — issuing and caching a certificate per name,
//! the server-side configuration and the handshake with the client — is
//! built on it.

pub mod authority;

#[cfg(test)]
mod fixtures;

pub use authority::{Authority, CaProblem, check};
