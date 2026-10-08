//! Snell key derivation (phase 2 M6 design 4.3, M8 design 4.3): each
//! direction of a connection starts with its own 16-byte salt, and its key
//! comes of Argon2id(PSK, salt, t = 3, m = 8 KiB, p = 1) — the PSK as the
//! UTF-8 bytes it is written with, 32 bytes of output. v1 keys
//! ChaCha20-Poly1305 with all of them, v2 to v5 AES-128-GCM with the first
//! half.
//!
//! One derivation per direction per connection: `spawn_output` and
//! `spawn_key` run it on a blocking thread, off the runtime.

use argon2::{Algorithm, Argon2, Params, Version};
use std::io;
use std::sync::Arc;
use tokio::task::JoinHandle;

pub(crate) const SALT_LEN: usize = 16;
pub(crate) const KEY_LEN: usize = 16;
pub(crate) const OUTPUT_LEN: usize = 32;

/// Memory in KiB, iterations, lanes.
const M_COST: u32 = 8;
const T_COST: u32 = 3;
const P_COST: u32 = 1;

/// Argon2id's whole output: v1 keys ChaCha20-Poly1305 with all of it, the
/// later versions AES-128-GCM with its first half.
pub(crate) fn derive(psk: &[u8], salt: &[u8; SALT_LEN]) -> [u8; OUTPUT_LEN] {
    let params = Params::new(M_COST, T_COST, P_COST, Some(OUTPUT_LEN))
        .expect("Snell's parameters are valid");
    let mut out = [0u8; OUTPUT_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(psk, salt, &mut out)
        // fails only for a salt shorter than 8 bytes or a password of 4 GiB
        .expect("a 16-byte salt");
    out
}

/// The AES-128-GCM key of the direction that starts with `salt`.
pub(crate) fn derive_key(psk: &[u8], salt: &[u8; SALT_LEN]) -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&derive(psk, salt)[..KEY_LEN]);
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

    /// `spawn_key` for the whole output of `salt`.
    pub(crate) fn spawn_output(&self, salt: [u8; SALT_LEN]) -> JoinHandle<[u8; OUTPUT_LEN]> {
        let psk = self.0.clone();
        tokio::task::spawn_blocking(move || derive(&psk, &salt))
    }

    /// `spawn_output`, awaited.
    pub(crate) async fn output(&self, salt: [u8; SALT_LEN]) -> io::Result<[u8; OUTPUT_LEN]> {
        self.spawn_output(salt).await.map_err(key_failed)
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

    /// The same Python `Argon2id`, all 32 bytes of it (scratch script of the
    /// M8b plan): v1 keys its cipher with the second half too.
    #[test]
    fn the_whole_output_is_argon2ids_32_bytes() {
        let salt: [u8; 16] = std::array::from_fn(|i| i as u8);
        assert_eq!(
            derive(b"password", &salt).to_vec(),
            hex("1ba4bb719f2afd88ee1ab71d82195effd1dcf8167811fd44c1ad63574021b88c")
        );
        assert_eq!(
            derive(b"secret", &salt).to_vec(),
            hex("2bc25e68fa4c91b2f0206cb1f088f1b85fafbbbb1bb797200ebadd32b5aaa16d")
        );
        let salt: [u8; 16] = std::array::from_fn(|i| 16 + i as u8);
        assert_eq!(
            derive("pässwörd".as_bytes(), &salt).to_vec(),
            hex("d1f4f38f2e7c23b800c99ca3f915ba270abe5169068c1e8e88995622de742421")
        );
    }

    #[tokio::test]
    async fn the_psk_derives_off_the_runtime_to_the_same_key() {
        let salt = [9u8; 16];
        let psk = Psk::new("password");
        assert_eq!(psk.key(salt).await.unwrap(), derive_key(b"password", &salt));
    }

    #[tokio::test]
    async fn the_psk_derives_off_the_runtime_to_the_same_output() {
        let salt = [9u8; 16];
        let psk = Psk::new("password");
        assert_eq!(psk.output(salt).await.unwrap(), derive(b"password", &salt));
    }
}
