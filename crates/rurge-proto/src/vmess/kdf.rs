//! The VMess AEAD key derivation: HMAC-SHA256 nested once per path element.
//!
//! The reference builds it as `hmac.New(parent.Create, element)`: the hash
//! *inside* each HMAC is the HMAC one level down, and the bottom one is
//! HMAC-SHA256 keyed with a fixed label. The `hmac` crate cannot express an
//! HMAC over an HMAC, so the construction (RFC 2104, 64-byte block, 32-byte
//! output at every level) is written out here and pinned by vectors taken
//! from the reference implementation.

use sha2::{Digest, Sha256};

const ROOT: &[u8] = b"VMess AEAD KDF";
const BLOCK: usize = 64;

pub(crate) const AUTH_ID_KEY: &[u8] = b"AES Auth ID Encryption";
pub(crate) const RESPONSE_LEN_KEY: &[u8] = b"AEAD Resp Header Len Key";
pub(crate) const RESPONSE_LEN_IV: &[u8] = b"AEAD Resp Header Len IV";
pub(crate) const RESPONSE_KEY: &[u8] = b"AEAD Resp Header Key";
pub(crate) const RESPONSE_IV: &[u8] = b"AEAD Resp Header IV";
pub(crate) const HEADER_KEY: &[u8] = b"VMess Header AEAD Key";
pub(crate) const HEADER_NONCE: &[u8] = b"VMess Header AEAD Nonce";
pub(crate) const HEADER_LEN_KEY: &[u8] = b"VMess Header AEAD Key_Length";
pub(crate) const HEADER_LEN_NONCE: &[u8] = b"VMess Header AEAD Nonce_Length";

fn hmac(hash: &dyn Fn(&[u8]) -> [u8; 32], key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&hash(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(BLOCK + message.len());
    inner.extend(block.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(message);
    let mut outer = Vec::with_capacity(BLOCK + 32);
    outer.extend(block.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&hash(&inner));
    hash(&outer)
}

/// The hash `path` names: HMAC keyed with its last element over the hash the
/// rest of it names; the empty path is HMAC-SHA256 keyed with `ROOT`.
fn hash(path: &[&[u8]], message: &[u8]) -> [u8; 32] {
    match path.split_last() {
        None => hmac(&|m| Sha256::digest(m).into(), ROOT, message),
        Some((key, rest)) => hmac(&|m| hash(rest, m), key, message),
    }
}

pub(crate) fn kdf(key: &[u8], path: &[&[u8]]) -> [u8; 32] {
    hash(path, key)
}

pub(crate) fn kdf16(key: &[u8], path: &[&[u8]]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&kdf(key, path)[..16]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::hex;

    /// v2fly/v2ray-core `proxy/vmess/aead/kdf.go`, run with these inputs.
    #[test]
    fn the_nesting_matches_the_reference() {
        assert_eq!(
            kdf(b"key", &[]).to_vec(),
            hex("385e28ac08671660f62ac976f5e64a31827aea172eff77cb2b046c52de9c08b0")
        );
        assert_eq!(
            kdf(b"key", &[b"a"]).to_vec(),
            hex("70ec70c19ef671319ed7b5493552fa2d77a53b57dba8aeb405536e79b5b50b6e")
        );
        assert_eq!(
            kdf(b"key", &[b"a", b"b"]).to_vec(),
            hex("721bea6cc9f16fac53b2afd131a33b9dc08e884e5b997a8ffac94a12cfd639ec")
        );
        assert_eq!(
            kdf(b"key", &[b"a", b"b", b"c"]).to_vec(),
            hex("7bc9030cc29018ba2c4a5bf0e32df68140fc20235fe0a1282f39e1d222279e18")
        );
    }

    #[test]
    fn a_key_longer_than_the_block_is_hashed_first() {
        // RFC 2104: only reachable with a path element above 64 bytes, which
        // VMess never uses; pinned so the branch is not dead weight
        let long = [7u8; 100];
        assert_ne!(kdf(b"key", &[&long]), kdf(b"key", &[&long[..64]]));
        assert_eq!(
            kdf16(b"key", &[b"a"]).to_vec(),
            hex("70ec70c19ef671319ed7b5493552fa2d")
        );
    }
}
