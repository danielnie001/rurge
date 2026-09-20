//! `vmess` outbound (manual: Policies › VMess). The codec lives here; the
//! outbound and its stream join it in the next commit.

#[allow(dead_code)] // until the outbound uses it (next commit)
pub(crate) mod chunk;
#[allow(dead_code)] // until the outbound uses it (next commit)
pub(crate) mod header;
#[allow(dead_code)] // until the outbound uses it (next commit)
pub(crate) mod kdf;
#[cfg(test)]
pub(crate) mod vectors;
