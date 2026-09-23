//! Shadow TLS, client side (M2 design 5.3). The records, the keyed digests
//! and the framed stream live here; the handshake joins them in a later commit.

#[allow(dead_code)] // until the handshake uses it (a later commit)
pub(crate) mod auth;
#[allow(dead_code)] // until the handshake uses it (a later commit)
mod framed;
#[allow(dead_code)] // until the handshake uses it (a later commit)
pub(crate) mod record;
#[cfg(test)]
mod vectors;
