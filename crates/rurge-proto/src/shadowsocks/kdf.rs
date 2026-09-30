//! Key derivation of the AEAD methods (the Shadowsocks AEAD specification):
//! the master key from the password, and one session key per salt; and of
//! SS 2022 (SIP022, SIP023): BLAKE3 `derive_key` of a key and a salt.

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

const SESSION_CONTEXT: &str = "shadowsocks 2022 session subkey";
const IDENTITY_CONTEXT: &str = "shadowsocks 2022 identity subkey";

/// `blake3::derive_key(context, key ‖ salt)`, as long as `key` (the first
/// 16 bytes of the output for a 16-byte key: its extendable output).
fn derive_2022(context: &str, key: &[u8], salt: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; key.len()];
    blake3::Hasher::new_derive_key(context)
        .update(key)
        .update(salt)
        .finalize_xof()
        .fill(&mut out);
    out
}

/// SS 2022: the session key of a stream that starts with `salt` (SIP022 2.2).
pub(crate) fn session_subkey_2022(psk: &[u8], salt: &[u8]) -> Vec<u8> {
    derive_2022(SESSION_CONTEXT, psk, salt)
}

/// SIP023: the key an identity header is encrypted with, from an identity
/// key and the stream's salt.
pub(crate) fn identity_subkey(ipsk: &[u8], salt: &[u8]) -> Vec<u8> {
    derive_2022(IDENTITY_CONTEXT, ipsk, salt)
}

/// SIP023: what an identity header says, the first 16 bytes of the next
/// layer's key's BLAKE3 hash.
pub(crate) fn identity_hash(key: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&blake3::hash(key).as_bytes()[..16]);
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

    /// Computed with a BLAKE3 written in Python from its specification
    /// (checked against the specification's hashes of "" and "abc"): key
    /// `00 01 …`, salt `80 81 …`, both as long as the method's key. 32 + 32
    /// bytes of material are exactly one block.
    #[test]
    fn ss_2022_subkeys_and_the_identity_hash_are_blake3s() {
        let cases = [
            (
                16,
                "722b3033c5d021365a8521bfb41157a3",
                "9b488f206a32316bf47ef417027b4242",
                "a6a492965517a830cb75fdb713465aa4",
            ),
            (
                32,
                "11289b9d205255930f83932405c2b0a38ec32be703fe33f290ff25ffeff402f9",
                "e3ba9438b4e97ed02d0c818020755598829161aaca5dc2b65fd46238ca2148ad",
                "e528e95798037df410543d9f31e396ec",
            ),
        ];
        for (len, session, identity, hash) in cases {
            let key: Vec<u8> = (0..len).collect();
            let salt: Vec<u8> = (0x80..0x80 + len).collect();
            assert_eq!(session_subkey_2022(&key, &salt), hex(session), "{len}");
            assert_eq!(identity_subkey(&key, &salt), hex(identity), "{len}");
            assert_eq!(identity_hash(&key).to_vec(), hex(hash), "{len}");
        }
    }
}
