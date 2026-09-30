//! The headers of SS 2022 over TCP (SIP022 3.1, SIP023; phase 2 M6 design
//! 3.3). The stream itself is `aead::AeadStream` in its 2022 form; these are
//! the pure pieces: the identity headers, the request's two header chunks
//! and the check of the response's header. Time is an argument, so the
//! tests pin it.

use super::cipher::aes_encrypt_block;
use super::kdf;
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

/// A payload chunk of this edition carries up to 0xFFFF bytes (no 0x3FFF cap).
pub(crate) const MAX_PAYLOAD: usize = 0xFFFF;
/// The request's variable-length header is a chunk too.
pub(crate) const MAX_VARIABLE_HEADER: usize = 0xFFFF;
/// Padding of a request without an initial payload: 1 to 900 bytes.
const MAX_PADDING: u16 = 900;
/// Timestamps further apart than this are a replay.
const TIME_WINDOW: u64 = 30;
const CLIENT_STREAM: u8 = 0;
const SERVER_STREAM: u8 = 1;
/// The request's fixed-length header: type, timestamp, the next chunk's length.
pub(crate) const REQUEST_FIXED: usize = 1 + 8 + 2;

const NOT_OURS: &str = "ss: the server's answer is not for this request";

/// Seconds since the Unix epoch, the protocol's timestamps.
pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The identity layers of a multi-user key (SIP023). No `Debug`: it holds
/// keys.
pub(crate) struct Identity {
    /// Each identity key and the hash of the key after it (the next
    /// identity key, or the user key for the last).
    layers: Vec<(Vec<u8>, [u8; 16])>,
}

impl Identity {
    /// `keys` as written: the identity keys outermost first, the user key
    /// last. A single key has no layers.
    pub(crate) fn new(keys: &[Vec<u8>]) -> Identity {
        Identity {
            layers: keys
                .windows(2)
                .map(|pair| (pair[0].clone(), kdf::identity_hash(&pair[1])))
                .collect(),
        }
    }

    /// The identity headers of a request that starts with `salt`, 16 bytes
    /// a layer: the next key's hash under one AES block keyed with the
    /// identity subkey of this layer's key and the salt.
    pub(crate) fn headers(&self, salt: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 * self.layers.len());
        for (key, next) in &self.layers {
            let mut block = *next;
            aes_encrypt_block(&kdf::identity_subkey(key, salt), &mut block);
            out.extend_from_slice(&block);
        }
        out
    }
}

/// The request's fixed-length header: type 0, `now`, and the length of the
/// variable-length header that follows.
pub(crate) fn request_fixed(now: u64, variable_len: usize) -> [u8; REQUEST_FIXED] {
    let len = u16::try_from(variable_len).expect("the variable header fits a chunk");
    let mut out = [0u8; REQUEST_FIXED];
    out[0] = CLIENT_STREAM;
    out[1..9].copy_from_slice(&now.to_be_bytes());
    out[9..].copy_from_slice(&len.to_be_bytes());
    out
}

/// The request's variable-length header: the address, `padding` bytes of
/// padding behind their length, then the initial payload. The padding is
/// zeros: it is sealed like the rest, only its length shows (as the chunk's).
pub(crate) fn request_variable(addr: &[u8], padding: usize, payload: &[u8]) -> Vec<u8> {
    let len = u16::try_from(padding).expect("at most 900 bytes of padding");
    let mut out = Vec::with_capacity(addr.len() + 2 + padding + payload.len());
    out.extend_from_slice(addr);
    out.extend_from_slice(&len.to_be_bytes());
    out.resize(out.len() + padding, 0);
    out.extend_from_slice(payload);
    out
}

/// How much padding a request carries: none with an initial payload, else
/// 1 to 900 bytes (the specification requires one or the other).
pub(crate) fn padding_len(payload_len: usize) -> Result<usize, getrandom::Error> {
    if payload_len > 0 {
        return Ok(0);
    }
    let mut random = [0u8; 2];
    getrandom::fill(&mut random)?;
    Ok(usize::from(u16::from_be_bytes(random) % MAX_PADDING + 1))
}

/// The length of the response's fixed-length header: type, timestamp, the
/// request's salt, the first chunk's length.
pub(crate) fn response_fixed_len(salt_len: usize) -> usize {
    1 + 8 + salt_len + 2
}

/// Checks the opened response header against the request that started with
/// `request_salt`; the length of the first payload chunk when it is ours.
/// The texts never carry a salt or a timestamp.
pub(crate) fn check_response(plain: &[u8], request_salt: &[u8], now: u64) -> io::Result<usize> {
    let invalid = |text: String| io::Error::new(io::ErrorKind::InvalidData, text);
    let salt_end = 9 + request_salt.len();
    debug_assert_eq!(plain.len(), response_fixed_len(request_salt.len()));
    // our own request played back would open too: its type tells
    if plain[0] != SERVER_STREAM || plain[9..salt_end] != *request_salt {
        return Err(invalid(NOT_OURS.to_string()));
    }
    let time = u64::from_be_bytes(plain[1..9].try_into().expect("8 bytes"));
    let skew = time.abs_diff(now);
    if skew > TIME_WINDOW {
        return Err(invalid(format!(
            "ss: the server's clock differs from ours by {skew} seconds (at most {TIME_WINDOW} are allowed)"
        )));
    }
    Ok(usize::from(u16::from_be_bytes([
        plain[salt_end],
        plain[salt_end + 1],
    ])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::hex;

    /// Computed with a BLAKE3 written in Python from its specification and
    /// `cryptography`'s AES-ECB: keys `00 …`, `n …`, `2n …` (n bytes each,
    /// counting up), salt `80 81 …`.
    #[test]
    fn two_identity_layers_are_two_headers_of_the_known_answer() {
        for (n, expected) in [
            (
                16u8,
                "2f80aef230d903eae2200a9b5411cfcf17ae493aaa1743d5a7042ec3fd99ea70",
            ),
            (
                32,
                "c4d59e980ad60a7751f47db7595cc79910038a40f080e8b0931372bfefdd7142",
            ),
        ] {
            let keys: Vec<Vec<u8>> = (0..3).map(|i| (i * n..(i + 1) * n).collect()).collect();
            let salt: Vec<u8> = (0x80..0x80 + n).collect();
            assert_eq!(Identity::new(&keys).headers(&salt), hex(expected), "{n}");
            // one layer fewer: the first header alone is not the same (it
            // hides the user key's hash now)
            let one = Identity::new(&[keys[0].clone(), keys[2].clone()]).headers(&salt);
            assert_eq!(one.len(), 16);
            assert_ne!(one, hex(expected)[..16]);
        }
        assert!(Identity::new(&[vec![7; 16]]).headers(&[0; 16]).is_empty());
    }

    #[test]
    fn the_request_headers_are_laid_out_as_the_specification() {
        let fixed = request_fixed(0x0102030405060708, 0x0a0b);
        assert_eq!(fixed, [0, 1, 2, 3, 4, 5, 6, 7, 8, 0x0a, 0x0b]);
        let addr = [1, 127, 0, 0, 1, 0x1f, 0x90];
        assert_eq!(
            request_variable(&addr, 0, b"hi"),
            [&addr[..], &[0, 0], b"hi"].concat()
        );
        let padded = request_variable(&addr, 3, b"");
        assert_eq!(padded, [&addr[..], &[0, 3, 0, 0, 0]].concat());
    }

    #[test]
    fn padding_only_without_a_payload_and_then_1_to_900_bytes() {
        assert_eq!(padding_len(1).unwrap(), 0);
        for _ in 0..1000 {
            let n = padding_len(0).unwrap();
            assert!((1..=900).contains(&n), "{n}");
        }
    }

    fn response(kind: u8, time: u64, salt: &[u8], len: u16) -> Vec<u8> {
        let mut out = vec![kind];
        out.extend_from_slice(&time.to_be_bytes());
        out.extend_from_slice(salt);
        out.extend_from_slice(&len.to_be_bytes());
        out
    }

    #[test]
    fn a_response_is_ours_only_with_its_type_our_salt_and_a_close_clock() {
        let salt = [9u8; 16];
        let now = 1_700_000_000;
        assert_eq!(response_fixed_len(16), 27);
        assert_eq!(response_fixed_len(32), 43);
        for time in [now - 30, now, now + 30] {
            let got = check_response(&response(1, time, &salt, 513), &salt, now).unwrap();
            assert_eq!(got, 513);
        }
        let texts = |plain: Vec<u8>| check_response(&plain, &salt, now).unwrap_err().to_string();
        assert_eq!(
            texts(response(0, now, &salt, 5)),
            NOT_OURS,
            "a played-back request"
        );
        assert_eq!(texts(response(1, now, &[8; 16], 5)), NOT_OURS);
        assert_eq!(
            texts(response(1, now - 31, &salt, 5)),
            "ss: the server's clock differs from ours by 31 seconds (at most 30 are allowed)"
        );
        assert_eq!(
            texts(response(1, now + 3600, &salt, 5)),
            "ss: the server's clock differs from ours by 3600 seconds (at most 30 are allowed)"
        );
    }
}
