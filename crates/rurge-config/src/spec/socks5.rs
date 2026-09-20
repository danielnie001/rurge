//! `socks5` / `socks5-tls` policy parameters (manual: Policies › SOCKS5).

use super::secret::Secret;
use super::tls::TlsOpts;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Socks5Spec {
    /// `Some` for `socks5-tls`.
    pub tls: Option<TlsOpts>,
    pub username: Option<Secret<String>>,
    pub password: Option<Secret<String>>,
    /// Parsed now; UDP ASSOCIATE arrives in M5.
    pub udp_relay: bool,
}

/// RFC 1929 carries each of the two in a one-byte length field.
pub(crate) const MAX_CREDENTIAL: usize = 255;
