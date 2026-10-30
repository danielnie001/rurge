//! MITM: what rurge decrypts, the certificate authority it signs with, and
//! the handshake with the client.
//!
//! `authority` brings the CA of `[MITM]`: where it comes from, what makes it
//! unusable and the `W0039` a profile load reports (`phase 4 M2 design 5`),
//! and the `W0040` it reports when `http-api-tls` has the API serve HTTPS
//! with a CA it does not have (`phase 4 M2 design 14.1`), then the
//! certificate it issues per name and caches (`phase 4 M2 design 6`).
//! `decrypt` is when a connection is decrypted at all, the server-side
//! configuration, and the handshake with the client (`phase 4 M2 design 7`,
//! `8`).
//! `generate` is the four files of a new CA, which `rurge mitm ca generate`
//! writes (`phase 4 M2 design 13`).
//! `api` is the TLS of the API, which serves HTTPS with the same authority's
//! certificates (`phase 4 M2 design 14.1`).

pub mod api;
pub mod authority;
pub mod decrypt;
pub mod generate;

#[cfg(test)]
mod fixtures;

#[cfg(test)]
pub(crate) mod testing;

pub use api::ApiTls;
pub use authority::{
    API_TLS_NO_CA, Authority, BACKDATE_DAYS, CaProblem, LEAF_DAYS, LEAF_SLOTS, LeafError,
    SESSION_SLOTS, check,
};
pub use decrypt::{Accepted, Decision, HandshakeFailure, Mitm, OriginTls, Watch, source_of};
pub use generate::{CA_DAYS, GenerateError, Generated, certificate_pem, generate};
