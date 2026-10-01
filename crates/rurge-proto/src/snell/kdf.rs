//! Snell v4 / v5 key derivation (phase 2 M6 design 4.3): each direction of a
//! connection starts with its own 16-byte salt, and its AES-128-GCM key is
//! the first half of Argon2id(PSK, salt, t = 3, m = 8 KiB, p = 1) — the PSK
//! as the UTF-8 bytes it is written with, 32 bytes of output.
//!
//! One derivation per direction per connection: `spawn_key` runs it on a
//! blocking thread, off the runtime.

use argon2::{Algorithm, Argon2, Params, Version};
use std::io;
use std::sync::Arc;
use tokio::task::JoinHandle;

pub(crate) const SALT_LEN: usize = 16;
pub(crate) const KEY_LEN: usize = 16;

/// Argon2id's output; only its first `KEY_LEN` bytes are used.
const OUTPUT_LEN: usize = 32;
/// Memory in KiB, iterations, lanes.
const M_COST: u32 = 8;
const T_COST: u32 = 3;
const P_COST: u32 = 1;

/// The AES-128-GCM key of the direction that starts with `salt`.
pub(crate) fn derive_key(psk: &[u8], salt: &[u8; SALT_LEN]) -> [u8; KEY_LEN] {
    let params = Params::new(M_COST, T_COST, P_COST, Some(OUTPUT_LEN))
        .expect("Snell's parameters are valid");
    let mut out = [0u8; OUTPUT_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(psk, salt, &mut out)
        // fails only for a salt shorter than 8 bytes or a password of 4 GiB
        .expect("a 16-byte salt");
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&out[..KEY_LEN]);
    key
}

/// A policy's PSK, shared by its connections. No `Debug`.
#[derive(Clone)]
pub(crate) struct Psk(Arc<[u8]>);

impl Psk {
    pub(crate) fn new(psk: &str) -> Psk {
        Psk(Arc::from(psk.as_bytes()))
    }

    /// Derives the key of `salt` on a blocking thread. Needs a Tokio runtime.
    pub(crate) fn spawn_key(&self, salt: [u8; SALT_LEN]) -> JoinHandle<[u8; KEY_LEN]> {
        let psk = self.0.clone();
        tokio::task::spawn_blocking(move || derive_key(&psk, &salt))
    }

    /// `spawn_key`, awaited.
    pub(crate) async fn key(&self, salt: [u8; SALT_LEN]) -> io::Result<[u8; KEY_LEN]> {
        self.spawn_key(salt).await.map_err(key_failed)
    }
}

/// The blocking task panicked or the runtime is shutting down.
pub(crate) fn key_failed(_: tokio::task::JoinError) -> io::Error {
    io::Error::other("snell: deriving the key failed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::hex;

    /// Computed with Python `cryptography`'s `Argon2id(length=32,
    /// iterations=3, lanes=1, memory_cost=8)` (scratch script of the M6b
    /// plan); the first matches the vector the byte-level notes cross-check
    /// against Go's `argon2.IDKey`.
    #[test]
    fn the_key_is_the_first_half_of_argon2id() {
        let salt: [u8; 16] = std::array::from_fn(|i| i as u8);
        assert_eq!(
            derive_key(b"password", &salt).to_vec(),
            hex("1ba4bb719f2afd88ee1ab71d82195eff")
        );
        // the PSK as its UTF-8 bytes
        let salt: [u8; 16] = std::array::from_fn(|i| 16 + i as u8);
        assert_eq!(
            derive_key("pässwörd".as_bytes(), &salt).to_vec(),
            hex("d1f4f38f2e7c23b800c99ca3f915ba27")
        );
    }

    #[tokio::test]
    async fn the_psk_derives_off_the_runtime_to_the_same_key() {
        let salt = [9u8; 16];
        let psk = Psk::new("password");
        assert_eq!(psk.key(salt).await.unwrap(), derive_key(b"password", &salt));
    }
}
