//! Key derivation of the AEAD methods (the Shadowsocks AEAD specification):
//! the master key from the password, and one session key per salt.

use hkdf::Hkdf;
use md5::{Digest, Md5};
use sha1::Sha1;

/// OpenSSL's `EVP_BytesToKey` with MD5, one round and no salt, key part
/// only: `D0 = MD5(password)`, `Di = MD5(Di-1 ‖ password)`, the first `len`
/// bytes of `D0 ‖ D1 ‖ …`.
pub(crate) fn evp_bytes_to_key(password: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 16);
    let mut previous: Option<[u8; 16]> = None;
    while out.len() < len {
        let mut md5 = Md5::new();
        if let Some(previous) = previous {
            md5.update(previous);
        }
        md5.update(password);
        let block: [u8; 16] = md5.finalize().into();
        out.extend_from_slice(&block);
        previous = Some(block);
    }
    out.truncate(len);
    out
}

/// HKDF-SHA1 (RFC 5869).
fn hkdf_sha1(ikm: &[u8], salt: &[u8], info: &[u8], okm: &mut [u8]) {
    Hkdf::<Sha1>::new(Some(salt), ikm)
        .expand(info, okm)
        // at most 255 × 20 bytes; a key is 32 at most
        .expect("a key-sized output");
}

/// The session key of a stream (or a datagram) that starts with `salt`:
/// `HKDF-SHA1(master, salt, "ss-subkey")`, as long as the master key.
pub(crate) fn session_subkey(master: &[u8], salt: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; master.len()];
    hkdf_sha1(master, salt, b"ss-subkey", &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::hex;

    /// Computed with Python's `hashlib` from the definition above.
    #[test]
    fn evp_bytes_to_key_chains_md5_over_the_password() {
        let full = "5f4dcc3b5aa765d61d8327deb882cf992b95990a9151374abd8ff8c5a7a0fe08";
        // the first block is plain MD5("password")
        assert_eq!(evp_bytes_to_key(b"password", 16), hex(&full[..32]));
        assert_eq!(evp_bytes_to_key(b"password", 24), hex(&full[..48]));
        assert_eq!(evp_bytes_to_key(b"password", 32), hex(full));
        assert!(evp_bytes_to_key(b"password", 0).is_empty());
    }

    /// RFC 5869 appendix A.4 (test case 4, SHA-1).
    #[test]
    fn hkdf_sha1_is_the_rfcs() {
        let mut okm = [0u8; 42];
        hkdf_sha1(
            &[0x0b; 11],
            &hex("000102030405060708090a0b0c"),
            &hex("f0f1f2f3f4f5f6f7f8f9"),
            &mut okm,
        );
        assert_eq!(
            okm.to_vec(),
            hex(
                "085a01ea1b10f36933068b56efa5ad81a4f14b822f5b091568a9cdd4f155fda2c22e422478d305f3f896"
            )
        );
    }

    /// Computed with Python's `hmac` from RFC 5869 and the specification's
    /// info string: salt `00 01 … 1f`, the master key of "password".
    #[test]
    fn the_session_subkey_is_as_long_as_the_master_key() {
        let master = evp_bytes_to_key(b"password", 32);
        let salt: Vec<u8> = (0u8..32).collect();
        assert_eq!(
            session_subkey(&master, &salt),
            hex("ee187aed3f87574907a39db98606f60a526114831288097cac66054b33a9464f")
        );
        let master = evp_bytes_to_key(b"password", 16);
        let salt: Vec<u8> = (0u8..16).collect();
        assert_eq!(
            session_subkey(&master, &salt),
            hex("ed2a618d9490d1701de885d82aa80616")
        );
    }
}
