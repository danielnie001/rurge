//! The VMess AEAD request head and the response head (M2 design 6.4;
//! byte layout checked against v2fly/v2ray-core `proxy/vmess/aead` and
//! `proxy/vmess/encoding/client.go`).
//!
//! Every function here is pure: time and randomness are arguments, so the
//! reference vectors apply byte for byte.

use super::kdf::{self, kdf, kdf16};
use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use md5::{Digest, Md5};
use ring::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};

const VERSION: u8 = 1;
const COMMAND_TCP: u8 = 1;
/// ChunkStream | ChunkMasking: what every server accepts.
pub(crate) const OPTIONS: u8 = 0x01 | 0x04;
pub(crate) const TAG: usize = 16;
/// A sealed response head is `V Opt Cmd Len` plus at most 255 bytes of command.
const MAX_RESPONSE_HEAD: usize = 4 + 255;

const ID_MAGIC: &[u8] = b"c48619fe-8f02-49e0-b9e9-edf763e17e21";

/// The `security` nibble of the request head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Security {
    Aes128Gcm = 3,
    ChaCha20Poly1305 = 4,
}

pub(crate) fn cmd_key(uuid: &[u8; 16]) -> [u8; 16] {
    let mut h = Md5::new();
    h.update(uuid);
    h.update(ID_MAGIC);
    h.finalize().into()
}

/// `time(8) ‖ random(4) ‖ crc32(4)` under one AES-128 block.
pub(crate) fn auth_id(cmd_key: &[u8; 16], unix_time: i64, random: [u8; 4]) -> [u8; 16] {
    let mut block = [0u8; 16];
    block[..8].copy_from_slice(&unix_time.to_be_bytes());
    block[8..12].copy_from_slice(&random);
    let crc = crc32fast::hash(&block[..12]);
    block[12..].copy_from_slice(&crc.to_be_bytes());
    let key = kdf16(cmd_key, &[kdf::AUTH_ID_KEY]);
    let mut block = GenericArray::from(block);
    Aes128::new(&GenericArray::from(key)).encrypt_block(&mut block);
    block.into()
}

/// What one connection draws at random.
pub(crate) struct Session {
    pub body_iv: [u8; 16],
    pub body_key: [u8; 16],
    pub response_v: u8,
}

fn fnv1a(data: &[u8]) -> u32 {
    data.iter().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(*b)).wrapping_mul(0x0100_0193)
    })
}

/// The head before sealing. `address` is `port ‖ type ‖ address`
/// (`addr::vmess_addr`); `padding` is 0 – 15 random bytes.
pub(crate) fn request_plain(
    session: &Session,
    security: Security,
    address: &[u8],
    padding: &[u8],
) -> Vec<u8> {
    debug_assert!(padding.len() < 16);
    let mut out = Vec::with_capacity(38 + address.len() + padding.len() + 4);
    out.push(VERSION);
    out.extend_from_slice(&session.body_iv);
    out.extend_from_slice(&session.body_key);
    out.push(session.response_v);
    out.push(OPTIONS);
    out.push(((padding.len() as u8) << 4) | security as u8);
    out.push(0);
    out.push(COMMAND_TCP);
    out.extend_from_slice(address);
    out.extend_from_slice(padding);
    let check = fnv1a(&out);
    out.extend_from_slice(&check.to_be_bytes());
    out
}

fn gcm(key: [u8; 16]) -> LessSafeKey {
    // a 16-byte key is what AES-128-GCM takes: this cannot fail
    LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &key).expect("a 16-byte key"))
}

fn nonce(bytes: &[u8; 32]) -> Nonce {
    let mut n = [0u8; 12];
    n.copy_from_slice(&bytes[..12]);
    Nonce::assume_unique_for_key(n)
}

fn seal(key: [u8; 16], iv: &[u8; 32], aad: &[u8], plain: &[u8], out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(plain);
    let tag = gcm(key)
        .seal_in_place_separate_tag(nonce(iv), Aad::from(aad), &mut out[start..])
        // fails only above 2^36 bytes
        .expect("a request head is a few dozen bytes");
    out.extend_from_slice(tag.as_ref());
}

fn open(key: [u8; 16], iv: &[u8; 32], aad: &[u8], sealed: &mut [u8]) -> Option<usize> {
    gcm(key)
        .open_in_place(nonce(iv), Aad::from(aad), sealed)
        .ok()
        .map(|plain| plain.len())
}

/// `AuthID(16) ‖ sealed length(18) ‖ nonce(8) ‖ sealed head`.
pub(crate) fn seal_request(
    cmd_key: &[u8; 16],
    auth_id: &[u8; 16],
    connection_nonce: &[u8; 8],
    plain: &[u8],
) -> Vec<u8> {
    let path = |label: &'static [u8]| [label, &auth_id[..], &connection_nonce[..]];
    let mut out = Vec::with_capacity(16 + 18 + 8 + plain.len() + TAG);
    out.extend_from_slice(auth_id);
    seal(
        kdf16(cmd_key, &path(kdf::HEADER_LEN_KEY)),
        &kdf(cmd_key, &path(kdf::HEADER_LEN_NONCE)),
        auth_id,
        &(plain.len() as u16).to_be_bytes(),
        &mut out,
    );
    out.extend_from_slice(connection_nonce);
    seal(
        kdf16(cmd_key, &path(kdf::HEADER_KEY)),
        &kdf(cmd_key, &path(kdf::HEADER_NONCE)),
        auth_id,
        plain,
        &mut out,
    );
    out
}

/// The response direction's body key and IV: the first half of the SHA-256
/// of the request's.
pub(crate) fn response_secrets(session: &Session) -> ([u8; 16], [u8; 16]) {
    use sha2::Sha256;
    let half = |input: &[u8; 16]| {
        let mut out = [0u8; 16];
        out.copy_from_slice(&Sha256::digest(input)[..16]);
        out
    };
    (half(&session.body_key), half(&session.body_iv))
}

/// Why a response head was refused. The texts go to the session log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResponseError {
    /// Wrong id on our side, or not a VMess AEAD server at all.
    NotAuthentic,
    TooLong,
    /// The head opened but does not answer this request.
    Mismatch,
}

impl ResponseError {
    pub(crate) fn text(self) -> &'static str {
        match self {
            ResponseError::NotAuthentic => "vmess: the response cannot be authenticated",
            ResponseError::TooLong => "vmess: the response head is longer than the protocol allows",
            ResponseError::Mismatch => "vmess: the response does not answer this request",
        }
    }
}

/// Opens the 18 sealed bytes that carry the head's length; returns how many
/// sealed bytes follow (the head plus its tag).
pub(crate) fn open_response_len(
    key: &[u8; 16],
    iv: &[u8; 16],
    mut sealed: [u8; 18],
) -> Result<usize, ResponseError> {
    let n = open(
        kdf16(key, &[kdf::RESPONSE_LEN_KEY]),
        &kdf(iv, &[kdf::RESPONSE_LEN_IV]),
        &[],
        &mut sealed,
    )
    .ok_or(ResponseError::NotAuthentic)?;
    debug_assert_eq!(n, 2);
    let len = usize::from(u16::from_be_bytes([sealed[0], sealed[1]]));
    if len > MAX_RESPONSE_HEAD {
        return Err(ResponseError::TooLong);
    }
    Ok(len + TAG)
}

/// Opens the head itself and checks that it answers `session`. A command in
/// it (the dynamic-port instruction) is ignored, as current clients do.
pub(crate) fn open_response(
    key: &[u8; 16],
    iv: &[u8; 16],
    session: &Session,
    sealed: &mut [u8],
) -> Result<(), ResponseError> {
    let n = open(
        kdf16(key, &[kdf::RESPONSE_KEY]),
        &kdf(iv, &[kdf::RESPONSE_IV]),
        &[],
        sealed,
    )
    .ok_or(ResponseError::NotAuthentic)?;
    if n < 4 || sealed[0] != session.response_v {
        return Err(ResponseError::Mismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::{self, hex};

    #[test]
    fn the_command_key_and_the_auth_id_match_the_reference() {
        let key = cmd_key(&vectors::UUID);
        assert_eq!(key.to_vec(), hex("1f449ead3205fb33e019c7af624da5b4"));
        assert_eq!(
            auth_id(&key, 1_700_000_000, [1, 2, 3, 4]).to_vec(),
            hex("c65fe1f5eada535d481be2645132f215")
        );
    }

    #[test]
    fn the_request_head_matches_the_reference_byte_for_byte() {
        let plain = request_plain(
            &vectors::session(),
            Security::Aes128Gcm,
            &vectors::address(),
            &[0xa1, 0xa2, 0xa3],
        );
        assert_eq!(plain, hex(vectors::REQUEST_PLAIN));
        let key = cmd_key(&vectors::UUID);
        let id = auth_id(&key, 1_700_000_000, [1, 2, 3, 4]);
        let sealed = seal_request(
            &key,
            &id,
            &[0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17],
            &plain,
        );
        assert_eq!(sealed, hex(vectors::REQUEST_SEALED));
    }

    #[test]
    fn the_security_nibble_follows_the_cipher() {
        let plain = request_plain(
            &vectors::session(),
            Security::ChaCha20Poly1305,
            &vectors::address(),
            &[],
        );
        assert_eq!(plain[35], 0x04, "no padding, chacha20-poly1305");
        assert_eq!(plain[34], OPTIONS);
    }

    #[test]
    fn the_response_head_opens_and_is_checked() {
        let session = vectors::session();
        let (key, iv) = response_secrets(&session);
        assert_eq!(key.to_vec(), hex("816b9e7c25d559c5766755b3bbb36654"));
        assert_eq!(iv.to_vec(), hex("36db1adc807ac50e4c85bd86a174b4aa"));
        let wire = hex(vectors::RESPONSE_SEALED);
        let mut len = [0u8; 18];
        len.copy_from_slice(&wire[..18]);
        let rest = open_response_len(&key, &iv, len).unwrap();
        assert_eq!(rest, 4 + TAG);
        let mut head = wire[18..].to_vec();
        assert_eq!(open_response(&key, &iv, &session, &mut head), Ok(()));
        // the same head does not answer a request that drew another V
        let other = Session {
            response_v: 0x5b,
            ..vectors::session()
        };
        let mut head = wire[18..].to_vec();
        assert_eq!(
            open_response(&key, &iv, &other, &mut head),
            Err(ResponseError::Mismatch)
        );
        // one flipped bit anywhere is an authentication failure
        let mut bad = len;
        bad[3] ^= 1;
        assert_eq!(
            open_response_len(&key, &iv, bad),
            Err(ResponseError::NotAuthentic)
        );
        let mut head = wire[18..].to_vec();
        head[0] ^= 1;
        assert_eq!(
            open_response(&key, &iv, &session, &mut head),
            Err(ResponseError::NotAuthentic)
        );
    }

    #[test]
    fn an_absurd_response_length_is_refused_before_anything_is_allocated() {
        let session = vectors::session();
        let (key, iv) = response_secrets(&session);
        let mut sealed = Vec::new();
        seal(
            kdf16(&key, &[kdf::RESPONSE_LEN_KEY]),
            &kdf(&iv, &[kdf::RESPONSE_LEN_IV]),
            &[],
            &60000u16.to_be_bytes(),
            &mut sealed,
        );
        let mut len = [0u8; 18];
        len.copy_from_slice(&sealed);
        assert_eq!(
            open_response_len(&key, &iv, len),
            Err(ResponseError::TooLong)
        );
    }
}
