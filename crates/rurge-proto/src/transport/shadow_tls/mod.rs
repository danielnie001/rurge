//! Shadow TLS, client side (M2 design 5.3). The records, the keyed digests,
//! the framed stream and the signed ClientHello of v3 live here; the handshake
//! joins them in a later commit.

#[allow(dead_code)] // until the handshake uses it (a later commit)
pub(crate) mod auth;
#[allow(dead_code)] // until the handshake uses it (a later commit)
mod framed;
#[allow(dead_code)] // until the handshake uses it (a later commit)
pub(crate) mod record;
#[allow(dead_code)] // until the handshake uses it (a later commit)
pub(crate) mod sign;
#[cfg(test)]
mod vectors;
