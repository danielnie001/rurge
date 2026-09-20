//! One direction of a VMess body (options ChunkStream + ChunkMasking): every
//! chunk is `length(2) ‖ AEAD(payload)`, the length XORed with the next two
//! bytes of SHAKE128(iv), the AEAD nonce `count(2, BE) ‖ iv[2..12]`, no
//! associated data. An empty payload ends the stream.

use super::header::{Security, TAG};
use md5::{Digest, Md5};
use ring::aead::{AES_128_GCM, Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use sha3::Shake128;
use sha3::digest::{ExtendableOutput, Update, XofReader};

/// A sealed chunk never exceeds 2^14 bytes (the limit in the protocol
/// description; sing-box reads a chunk into a 16384-byte buffer).
pub(crate) const MAX_PAYLOAD: usize = 16384 - TAG;

/// ChaCha20-Poly1305 wants 32 bytes: `MD5(key) ‖ MD5(MD5(key))`.
fn chacha_key(key: &[u8; 16]) -> [u8; 32] {
    let first: [u8; 16] = Md5::digest(key).into();
    let second: [u8; 16] = Md5::digest(first).into();
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&first);
    out[16..].copy_from_slice(&second);
    out
}

pub(crate) struct ChunkCipher {
    key: LessSafeKey,
    iv: [u8; 16],
    count: u16,
    mask: sha3::Shake128Reader,
}

impl ChunkCipher {
    pub(crate) fn new(security: Security, key: &[u8; 16], iv: &[u8; 16]) -> ChunkCipher {
        // both key lengths are fixed by the types: these cannot fail
        let key = match security {
            Security::Aes128Gcm => UnboundKey::new(&AES_128_GCM, key).expect("a 16-byte key"),
            Security::ChaCha20Poly1305 => {
                UnboundKey::new(&CHACHA20_POLY1305, &chacha_key(key)).expect("a 32-byte key")
            }
        };
        let mut shake = Shake128::default();
        shake.update(iv);
        ChunkCipher {
            key: LessSafeKey::new(key),
            iv: *iv,
            count: 0,
            mask: shake.finalize_xof(),
        }
    }

    fn next_mask(&mut self) -> u16 {
        let mut two = [0u8; 2];
        self.mask.read(&mut two);
        u16::from_be_bytes(two)
    }

    /// The counter wraps after 65536 chunks, as the reference's does.
    fn next_nonce(&mut self) -> Nonce {
        let mut nonce = [0u8; 12];
        nonce[..2].copy_from_slice(&self.count.to_be_bytes());
        nonce[2..].copy_from_slice(&self.iv[2..12]);
        self.count = self.count.wrapping_add(1);
        Nonce::assume_unique_for_key(nonce)
    }

    /// Appends one chunk carrying `payload` (at most `MAX_PAYLOAD` bytes;
    /// empty = end of stream) to `out`.
    pub(crate) fn seal(&mut self, payload: &[u8], out: &mut Vec<u8>) {
        debug_assert!(payload.len() <= MAX_PAYLOAD);
        let sealed_len = (payload.len() + TAG) as u16;
        out.extend_from_slice(&(self.next_mask() ^ sealed_len).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(payload);
        let nonce = self.next_nonce();
        let tag = self
            .key
            .seal_in_place_separate_tag(nonce, Aad::empty(), &mut out[start..])
            // fails only above 2^36 bytes
            .expect("a chunk is at most 16 KiB");
        out.extend_from_slice(tag.as_ref());
    }

    /// How many sealed bytes follow these two length bytes.
    pub(crate) fn open_len(&mut self, masked: [u8; 2]) -> usize {
        usize::from(self.next_mask() ^ u16::from_be_bytes(masked))
    }

    /// Opens a sealed chunk in place; the payload is `sealed[..n]`.
    pub(crate) fn open(&mut self, sealed: &mut [u8]) -> Option<usize> {
        let nonce = self.next_nonce();
        self.key
            .open_in_place(nonce, Aad::empty(), sealed)
            .ok()
            .map(|plain| plain.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::header::response_secrets;
    use crate::vmess::vectors::{self, hex};

    fn sealed(cipher: &mut ChunkCipher, payloads: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in payloads {
            cipher.seal(p, &mut out);
        }
        out
    }

    /// Splits `wire` back into payloads with a fresh cipher.
    fn opened(mut cipher: ChunkCipher, wire: &[u8]) -> Vec<Vec<u8>> {
        let (mut at, mut out) = (0, Vec::new());
        while at < wire.len() {
            let len = cipher.open_len([wire[at], wire[at + 1]]);
            let mut chunk = wire[at + 2..at + 2 + len].to_vec();
            let n = cipher.open(&mut chunk).expect("authentic");
            out.push(chunk[..n].to_vec());
            at += 2 + len;
        }
        out
    }

    #[test]
    fn aes_chunks_match_the_reference_in_both_directions() {
        let s = vectors::session();
        let long: Vec<u8> = (0u8..40).collect();
        let mut up = ChunkCipher::new(Security::Aes128Gcm, &s.body_key, &s.body_iv);
        assert_eq!(
            sealed(&mut up, &[b"hello", &long, b""]),
            hex(vectors::REQUEST_CHUNKS_AES)
        );
        let (key, iv) = response_secrets(&s);
        let mut down = ChunkCipher::new(Security::Aes128Gcm, &key, &iv);
        assert_eq!(
            sealed(&mut down, &[b"world", b""]),
            hex(vectors::RESPONSE_CHUNKS_AES)
        );
    }

    #[test]
    fn chacha_chunks_match_the_independent_derivation() {
        let s = vectors::session();
        assert_eq!(
            chacha_key(&s.body_key).to_vec(),
            hex("bdf2930f973f722e24a3773d61889501c3c2e371e23677b71c00a97d736d5e0f")
        );
        let long: Vec<u8> = (0u8..40).collect();
        let mut up = ChunkCipher::new(Security::ChaCha20Poly1305, &s.body_key, &s.body_iv);
        assert_eq!(
            sealed(&mut up, &[b"hello", &long, b""]),
            hex(vectors::REQUEST_CHUNKS_CHACHA)
        );
        let (key, iv) = response_secrets(&s);
        let mut down = ChunkCipher::new(Security::ChaCha20Poly1305, &key, &iv);
        assert_eq!(
            sealed(&mut down, &[b"world", b""]),
            hex(vectors::RESPONSE_CHUNKS_CHACHA)
        );
    }

    #[test]
    fn the_length_masks_are_the_shake128_stream_of_the_iv() {
        let s = vectors::session();
        let mut c = ChunkCipher::new(Security::Aes128Gcm, &s.body_key, &s.body_iv);
        let masks: Vec<u8> = (0..8).flat_map(|_| c.next_mask().to_be_bytes()).collect();
        assert_eq!(masks, hex("e48822357582b1941d2dc3ca28b23952"));
    }

    #[test]
    fn what_was_sealed_opens_and_a_flipped_bit_does_not() {
        let s = vectors::session();
        let wire = hex(vectors::REQUEST_CHUNKS_AES);
        let long: Vec<u8> = (0u8..40).collect();
        let cipher = ChunkCipher::new(Security::Aes128Gcm, &s.body_key, &s.body_iv);
        assert_eq!(opened(cipher, &wire), [b"hello".to_vec(), long, Vec::new()]);
        let mut cipher = ChunkCipher::new(Security::Aes128Gcm, &s.body_key, &s.body_iv);
        let len = cipher.open_len([wire[0], wire[1]]);
        assert_eq!(len, 5 + TAG);
        let mut chunk = wire[2..2 + len].to_vec();
        chunk[0] ^= 1;
        assert_eq!(cipher.open(&mut chunk), None);
    }

    #[test]
    fn the_largest_chunk_fits_the_length_field() {
        let s = vectors::session();
        let mut c = ChunkCipher::new(Security::Aes128Gcm, &s.body_key, &s.body_iv);
        let mut out = Vec::new();
        c.seal(&vec![7u8; MAX_PAYLOAD], &mut out);
        assert_eq!(out.len(), 2 + 16384);
    }
}
