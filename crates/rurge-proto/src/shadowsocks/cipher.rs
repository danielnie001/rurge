//! The AEAD constructions of Shadowsocks (phase 2 M6 design 3.3): which
//! cipher a method uses, the cipher itself, and the counting nonce of a
//! stream. No associated data anywhere in the protocol.

use super::kdf;
use aes::cipher::{BlockDecrypt, BlockEncrypt};
use aes::{Aes128, Aes256};
use aes_gcm::aead::consts::U12;
use aes_gcm::aes::Aes192;
use aes_gcm::{AeadInOut, Aes128Gcm, Aes256Gcm, AesGcm, KeyInit};
use chacha20poly1305::aead::generic_array::GenericArray;
// the ChaCha ciphers (and the `aes` block ciphers) are of the older generation,
// with traits of their own
use chacha20poly1305::{AeadInPlace, ChaCha20Poly1305, KeyInit as _, XChaCha20Poly1305};
use rurge_config::spec::SsMethod;

/// Every method's tag is 16 bytes.
pub(crate) const TAG: usize = 16;

/// The longest nonce (XChaCha20's).
const MAX_NONCE: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AeadKind {
    Aes128Gcm,
    Aes192Gcm,
    Aes256Gcm,
    ChaCha20Poly1305,
    XChaCha20Poly1305,
}

impl AeadKind {
    /// `None` for `none`, which encrypts nothing. SS 2022 names the AES-GCM
    /// it seals with.
    pub(crate) fn of(method: SsMethod) -> Option<AeadKind> {
        match method {
            SsMethod::None => None,
            SsMethod::Aes128Gcm | SsMethod::Blake3Aes128Gcm => Some(AeadKind::Aes128Gcm),
            SsMethod::Aes192Gcm => Some(AeadKind::Aes192Gcm),
            SsMethod::Aes256Gcm | SsMethod::Blake3Aes256Gcm => Some(AeadKind::Aes256Gcm),
            SsMethod::ChaCha20IetfPoly1305 => Some(AeadKind::ChaCha20Poly1305),
            SsMethod::XChaCha20IetfPoly1305 => Some(AeadKind::XChaCha20Poly1305),
        }
    }

    /// The key length, which is also the salt length.
    pub(crate) fn key_len(self) -> usize {
        match self {
            AeadKind::Aes128Gcm => 16,
            AeadKind::Aes192Gcm => 24,
            AeadKind::Aes256Gcm | AeadKind::ChaCha20Poly1305 | AeadKind::XChaCha20Poly1305 => 32,
        }
    }

    pub(crate) fn nonce_len(self) -> usize {
        match self {
            AeadKind::XChaCha20Poly1305 => 24,
            _ => 12,
        }
    }
}

/// One keyed cipher. No `Debug`: it holds a key.
pub(crate) enum AeadCipher {
    Aes128Gcm(Aes128Gcm),
    Aes192Gcm(AesGcm<Aes192, U12>),
    Aes256Gcm(Aes256Gcm),
    ChaCha20Poly1305(ChaCha20Poly1305),
    XChaCha20Poly1305(XChaCha20Poly1305),
}

/// The GCM ciphers take a 12-byte nonce.
fn gcm_nonce(nonce: &[u8]) -> &aes_gcm::Nonce<U12> {
    nonce.try_into().expect("a 12-byte nonce")
}

impl AeadCipher {
    /// `key` is `kind.key_len()` bytes long: the callers derive it so.
    pub(crate) fn new(kind: AeadKind, key: &[u8]) -> AeadCipher {
        const LEN: &str = "a key of the method's length";
        match kind {
            AeadKind::Aes128Gcm => {
                AeadCipher::Aes128Gcm(Aes128Gcm::new_from_slice(key).expect(LEN))
            }
            AeadKind::Aes192Gcm => {
                AeadCipher::Aes192Gcm(AesGcm::<Aes192, U12>::new_from_slice(key).expect(LEN))
            }
            AeadKind::Aes256Gcm => {
                AeadCipher::Aes256Gcm(Aes256Gcm::new_from_slice(key).expect(LEN))
            }
            AeadKind::ChaCha20Poly1305 => {
                AeadCipher::ChaCha20Poly1305(ChaCha20Poly1305::new_from_slice(key).expect(LEN))
            }
            AeadKind::XChaCha20Poly1305 => {
                AeadCipher::XChaCha20Poly1305(XChaCha20Poly1305::new_from_slice(key).expect(LEN))
            }
        }
    }

    /// Encrypts `data` in place and returns its tag. `nonce` is the kind's
    /// nonce length.
    pub(crate) fn seal_in_place(&self, nonce: &[u8], data: &mut [u8]) -> [u8; TAG] {
        // fails only for inputs of gigabytes: a chunk or a datagram is far below
        const SIZE: &str = "a chunk is far below the AEAD's limit";
        let mut tag = [0u8; TAG];
        match self {
            AeadCipher::Aes128Gcm(c) => tag.copy_from_slice(
                &c.encrypt_inout_detached(gcm_nonce(nonce), &[], data.into())
                    .expect(SIZE),
            ),
            AeadCipher::Aes192Gcm(c) => tag.copy_from_slice(
                &c.encrypt_inout_detached(gcm_nonce(nonce), &[], data.into())
                    .expect(SIZE),
            ),
            AeadCipher::Aes256Gcm(c) => tag.copy_from_slice(
                &c.encrypt_inout_detached(gcm_nonce(nonce), &[], data.into())
                    .expect(SIZE),
            ),
            AeadCipher::ChaCha20Poly1305(c) => tag.copy_from_slice(
                &c.encrypt_in_place_detached(GenericArray::from_slice(nonce), &[], data)
                    .expect(SIZE),
            ),
            AeadCipher::XChaCha20Poly1305(c) => tag.copy_from_slice(
                &c.encrypt_in_place_detached(GenericArray::from_slice(nonce), &[], data)
                    .expect(SIZE),
            ),
        }
        tag
    }

    /// Decrypts `data` in place; `false` when `tag` does not authenticate it
    /// (`data` is then garbage).
    pub(crate) fn open_in_place(&self, nonce: &[u8], data: &mut [u8], tag: &[u8; TAG]) -> bool {
        match self {
            AeadCipher::Aes128Gcm(c) => c
                .decrypt_inout_detached(gcm_nonce(nonce), &[], data.into(), &(*tag).into())
                .is_ok(),
            AeadCipher::Aes192Gcm(c) => c
                .decrypt_inout_detached(gcm_nonce(nonce), &[], data.into(), &(*tag).into())
                .is_ok(),
            AeadCipher::Aes256Gcm(c) => c
                .decrypt_inout_detached(gcm_nonce(nonce), &[], data.into(), &(*tag).into())
                .is_ok(),
            AeadCipher::ChaCha20Poly1305(c) => c
                .decrypt_in_place_detached(
                    GenericArray::from_slice(nonce),
                    &[],
                    data,
                    GenericArray::from_slice(tag),
                )
                .is_ok(),
            AeadCipher::XChaCha20Poly1305(c) => c
                .decrypt_in_place_detached(
                    GenericArray::from_slice(nonce),
                    &[],
                    data,
                    GenericArray::from_slice(tag),
                )
                .is_ok(),
        }
    }
}

/// A cipher and the nonce of one direction of a stream: a little-endian
/// counter from zero, incremented after every operation. No `Debug`.
pub(crate) struct CountingAead {
    cipher: AeadCipher,
    nonce: [u8; MAX_NONCE],
    nonce_len: usize,
}

impl CountingAead {
    pub(crate) fn new(kind: AeadKind, key: &[u8]) -> CountingAead {
        CountingAead {
            cipher: AeadCipher::new(kind, key),
            nonce: [0; MAX_NONCE],
            nonce_len: kind.nonce_len(),
        }
    }

    /// Wraps at the nonce's width, which no stream reaches.
    fn advance(&mut self) {
        for byte in &mut self.nonce[..self.nonce_len] {
            *byte = byte.wrapping_add(1);
            if *byte != 0 {
                break;
            }
        }
    }

    /// Appends `plain` sealed, its tag last, to `out`.
    pub(crate) fn seal(&mut self, plain: &[u8], out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(plain);
        let tag = self
            .cipher
            .seal_in_place(&self.nonce[..self.nonce_len], &mut out[start..]);
        out.extend_from_slice(&tag);
        self.advance();
    }

    /// Opens `sealed` (its tag last) in place; the plaintext is
    /// `sealed[..n]`. `None`: it does not authenticate. The nonce moves on
    /// either way.
    pub(crate) fn open(&mut self, sealed: &mut [u8]) -> Option<usize> {
        let n = sealed.len().checked_sub(TAG)?;
        let (data, tag) = sealed.split_at_mut(n);
        let tag: &[u8; TAG] = (&*tag).try_into().expect("the last 16 bytes");
        let ok = self
            .cipher
            .open_in_place(&self.nonce[..self.nonce_len], data, tag);
        self.advance();
        ok.then_some(n)
    }
}

/// The key a method's streams derive their session keys from. No `Debug`.
pub(crate) struct MasterKey {
    kind: AeadKind,
    key: Vec<u8>,
    /// SS 2022: the key is a PSK and sessions derive with BLAKE3.
    edition_2022: bool,
}

impl MasterKey {
    /// The AEAD methods' key: `EVP_BytesToKey(MD5)` of the password.
    pub(crate) fn from_password(kind: AeadKind, password: &str) -> MasterKey {
        MasterKey {
            kind,
            key: kdf::evp_bytes_to_key(password.as_bytes(), kind.key_len()),
            edition_2022: false,
        }
    }

    /// SS 2022's key: the (user) PSK itself, `kind.key_len()` bytes.
    pub(crate) fn from_psk(kind: AeadKind, psk: &[u8]) -> MasterKey {
        debug_assert_eq!(psk.len(), kind.key_len());
        MasterKey {
            kind,
            key: psk.to_vec(),
            edition_2022: true,
        }
    }

    pub(crate) fn salt_len(&self) -> usize {
        self.kind.key_len()
    }

    /// One direction of a stream that starts with `salt`: the session key
    /// is HKDF-SHA1 of the master key under that salt, or with SS 2022
    /// BLAKE3 `derive_key` of the PSK and the salt.
    pub(crate) fn session(&self, salt: &[u8]) -> CountingAead {
        let key = if self.edition_2022 {
            kdf::session_subkey_2022(&self.key, salt)
        } else {
            kdf::session_subkey(&self.key, salt)
        };
        CountingAead::new(self.kind, &key)
    }
}

/// One AES block encrypted in place with a 16- or 32-byte key (SS 2022's
/// identity headers and separate headers: ECB of a single block).
pub(crate) fn aes_encrypt_block(key: &[u8], block: &mut [u8; 16]) {
    let block = GenericArray::from_mut_slice(block);
    match key.len() {
        16 => Aes128::new_from_slice(key)
            .expect("16 bytes")
            .encrypt_block(block),
        _ => Aes256::new_from_slice(key)
            .expect("a 32-byte key")
            .encrypt_block(block),
    }
}

/// The inverse of `aes_encrypt_block` (the fake server's side).
#[cfg(any(test, feature = "testing"))]
pub(crate) fn aes_decrypt_block(key: &[u8], block: &mut [u8; 16]) {
    let block = GenericArray::from_mut_slice(block);
    match key.len() {
        16 => Aes128::new_from_slice(key)
            .expect("16 bytes")
            .decrypt_block(block),
        _ => Aes256::new_from_slice(key)
            .expect("a 32-byte key")
            .decrypt_block(block),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_method_has_its_cipher_and_sizes() {
        let cases = [
            (SsMethod::Aes128Gcm, AeadKind::Aes128Gcm, 16, 12),
            (SsMethod::Aes192Gcm, AeadKind::Aes192Gcm, 24, 12),
            (SsMethod::Aes256Gcm, AeadKind::Aes256Gcm, 32, 12),
            (
                SsMethod::ChaCha20IetfPoly1305,
                AeadKind::ChaCha20Poly1305,
                32,
                12,
            ),
            (
                SsMethod::XChaCha20IetfPoly1305,
                AeadKind::XChaCha20Poly1305,
                32,
                24,
            ),
            (SsMethod::Blake3Aes128Gcm, AeadKind::Aes128Gcm, 16, 12),
            (SsMethod::Blake3Aes256Gcm, AeadKind::Aes256Gcm, 32, 12),
        ];
        for (method, kind, key, nonce) in cases {
            assert_eq!(AeadKind::of(method), Some(kind));
            assert_eq!((kind.key_len(), kind.nonce_len()), (key, nonce));
            assert_eq!(kind.key_len(), method.key_len(), "{method:?}");
        }
        assert_eq!(AeadKind::of(SsMethod::None), None);
    }

    #[test]
    fn the_nonce_counts_little_endian_with_a_carry() {
        let mut aead = CountingAead::new(AeadKind::Aes128Gcm, &[0; 16]);
        aead.nonce[0] = 0xff;
        aead.advance();
        assert_eq!(aead.nonce[..3], [0, 1, 0]);
        aead.nonce[..12].fill(0xff);
        aead.advance();
        assert_eq!(aead.nonce, [0; MAX_NONCE], "wraps at its own width");
    }

    #[test]
    fn what_was_sealed_opens_once_and_a_flipped_bit_does_not() {
        for kind in [
            AeadKind::Aes128Gcm,
            AeadKind::Aes192Gcm,
            AeadKind::Aes256Gcm,
            AeadKind::ChaCha20Poly1305,
            AeadKind::XChaCha20Poly1305,
        ] {
            let key = vec![7u8; kind.key_len()];
            let mut up = CountingAead::new(kind, &key);
            let mut wire = Vec::new();
            up.seal(b"first", &mut wire);
            up.seal(b"second", &mut wire);
            let mut down = CountingAead::new(kind, &key);
            let (first, second) = wire.split_at_mut(5 + TAG);
            assert_eq!(down.open(first), Some(5));
            assert_eq!(&first[..5], b"first");
            second[0] ^= 1;
            assert_eq!(down.open(second), None, "{kind:?}");
            // out of step: the second nonce does not open the first chunk
            let mut again = Vec::new();
            CountingAead::new(kind, &key).seal(b"first", &mut again);
            let mut late = CountingAead::new(kind, &key);
            late.advance();
            assert_eq!(late.open(&mut again), None);
            assert_eq!(down.open(&mut [0u8; 3]), None, "shorter than a tag");
        }
    }
}
