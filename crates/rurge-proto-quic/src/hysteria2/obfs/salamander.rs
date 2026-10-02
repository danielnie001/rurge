//! Salamander (Hysteria 2 protocol, "Salamander"; phase 2 M7 design 5.4):
//! every packet goes as `salt ‖ payload ^ key`, the salt 8 random bytes of
//! its own, the key `BLAKE2b-256(password ‖ salt)` repeated over the
//! payload. BLAKE2b-256 is BLAKE2b with a digest length of 32 — Go's
//! `blake2b.Sum256`, which the reference keys with
//! (`extras/obfs/salamander.go`) —, not BLAKE2b-512 cut short. Nothing in a
//! packet says it was changed on the way: only one too short to hold a salt
//! and a byte does not open; anything else opens, to garbage if it must,
//! which QUIC's own protection then drops.

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};

/// The salt in front of every packet.
pub(super) const SALT: usize = 8;
/// What the key repeats over.
const KEY: usize = 32;

type Blake2b256 = Blake2b<U32>;

/// Salamander keyed with a password. No `Debug`: it holds the password.
#[derive(Clone)]
pub(super) struct Salamander {
    /// BLAKE2b-256 that has taken the password: each packet's key is a copy
    /// of it finished with the packet's salt.
    keyed: Blake2b256,
}

impl Salamander {
    pub(super) fn new(password: &[u8]) -> Salamander {
        Salamander {
            keyed: Blake2b256::new_with_prefix(password),
        }
    }

    /// The key of the packets salted with `salt`.
    fn key(&self, salt: &[u8]) -> [u8; KEY] {
        self.keyed.clone().chain_update(salt).finalize().into()
    }

    /// `datagram` — room for the salt, then the payload — sealed in place:
    /// a new random salt, the payload masked with its key.
    pub(super) fn seal(&self, datagram: &mut [u8]) {
        let mut salt = [0; SALT];
        // obfuscation, not a secret: without a random source, a salt of
        // zeros
        let _ = getrandom::fill(&mut salt);
        self.seal_with(salt, datagram);
    }

    fn seal_with(&self, salt: [u8; SALT], datagram: &mut [u8]) {
        datagram[..SALT].copy_from_slice(&salt);
        let key = self.key(&salt);
        mask(&mut datagram[SALT..], &key);
    }

    /// The payload of a sealed `datagram`, unmasked in place; `None` for
    /// one that holds no more than a salt (as the reference reads it).
    pub(super) fn open<'a>(&self, datagram: &'a mut [u8]) -> Option<&'a mut [u8]> {
        if datagram.len() <= SALT {
            return None;
        }
        let (salt, payload) = datagram.split_at_mut(SALT);
        let key = self.key(salt);
        mask(payload, &key);
        Some(payload)
    }
}

/// XORs `key`, over and over, onto `bytes`.
fn mask(bytes: &mut [u8], key: &[u8; KEY]) {
    for chunk in bytes.chunks_mut(KEY) {
        for (byte, k) in chunk.iter_mut().zip(key) {
            *byte ^= k;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blake2::digest::consts::{U20, U48, U64};
    use blake2::digest::generic_array::ArrayLength;
    use blake2::digest::typenum::{IsLessOrEqual, LeEq, NonZero};
    use blake2::digest::{KeyInit, Mac, Update, VariableOutput};
    use blake2::{Blake2b512, Blake2bMac, Blake2bVar};

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// BLAKE2b-256 of `abc`, as CPython's `hashlib.blake2b(b"abc",
    /// digest_size=32)` gives it, is no prefix of BLAKE2b-512 of `abc` (RFC
    /// 7693 Appendix A): the digest length is a parameter of the hash, as
    /// in Go's `blake2b.Sum256`, not a cut. The key is the hash of the
    /// password and the salt one after the other.
    #[test]
    fn blake2b_256_is_blake2b_with_a_digest_length_of_32() {
        let abc = hex("bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319");
        assert_eq!(Blake2b256::digest(b"abc")[..], abc);
        assert_eq!(
            Blake2b512::digest(b"abc")[..],
            hex(concat!(
                "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d1",
                "7d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
            ))
        );
        assert_eq!(Salamander::new(b"ab").key(b"c")[..], abc);
        assert_eq!(Salamander::new(b"").key(b"abc")[..], abc);
    }

    /// RFC 7693 Appendix E: BLAKE2b of the digest lengths 20, 32, 48 and
    /// 64, keyed and not, over inputs of 0 to 1024 bytes, all hashed
    /// together by the BLAKE2b-256 Salamander keys with, gives the RFC's
    /// result.
    #[test]
    fn blake2b_256_passes_the_self_test_of_rfc_7693() {
        /// `selftest_seq`: the top byte of each term of a Fibonacci sequence.
        fn sequence(len: usize, seed: u32) -> Vec<u8> {
            let (mut a, mut b) = (0xDEAD_4BAD_u32.wrapping_mul(seed), 1u32);
            (0..len)
                .map(|_| {
                    let t = a.wrapping_add(b);
                    (a, b) = (b, t);
                    (t >> 24) as u8
                })
                .collect()
        }
        fn keyed<N>(key: &[u8], data: &[u8]) -> Vec<u8>
        where
            N: ArrayLength<u8> + IsLessOrEqual<U64>,
            LeEq<N, U64>: NonZero,
        {
            let mut mac = <Blake2bMac<N> as KeyInit>::new_from_slice(key).expect("a key");
            Mac::update(&mut mac, data);
            mac.finalize().into_bytes().to_vec()
        }
        let mut grand = Blake2b256::new();
        for len in [20, 32, 48, 64] {
            for input in [0, 3, 128, 129, 255, 1024] {
                let data = sequence(input, input as u32);
                let mut plain = Blake2bVar::new(len).expect("a digest length");
                Update::update(&mut plain, &data);
                let mut digest = vec![0; len];
                plain.finalize_variable(&mut digest).expect("the length");
                Digest::update(&mut grand, &digest);
                let key = sequence(len, len as u32);
                let digest = match len {
                    20 => keyed::<U20>(&key, &data),
                    32 => keyed::<U32>(&key, &data),
                    48 => keyed::<U48>(&key, &data),
                    _ => keyed::<U64>(&key, &data),
                };
                Digest::update(&mut grand, &digest);
            }
        }
        assert_eq!(
            grand.finalize()[..],
            hex("c23a7800d98123bd10f506c61e29da5603d763b8bbad2e737f5e765a7bccd475")
        );
    }

    /// A packet sealed with a known salt, byte for byte: computed apart, by
    /// CPython's `hashlib` (`salt + bytes(b ^ key[i % 32] …)`, `key =
    /// blake2b(password + salt, digest_size=32)`). The 70-byte payload goes
    /// past the key twice.
    #[test]
    fn a_packet_sealed_with_a_known_salt() {
        let salamander = Salamander::new(b"rurge salamander");
        let salt = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let payload: Vec<u8> = (0..70).collect();
        assert_eq!(
            salamander.key(&salt)[..],
            hex("7b21007e054064d04bbf06c42e4c538375be27ad59ab58f2a974dc1544f58c32")
        );
        let mut datagram = [&[0; SALT][..], &payload].concat();
        salamander.seal_with(salt, &mut datagram);
        assert_eq!(
            datagram,
            hex(concat!(
                "0123456789abcdef7b20027d014562d743b60ccf22415d8c65af35be4dbe4ee5",
                "b16dc60e58e8922d5b00225d216542f763962cef02617dac458f159e6d9e6ec5",
                "914de62e78c8b20d3b60423d4105"
            ))
        );
        assert_eq!(salamander.open(&mut datagram).unwrap(), &payload[..]);
    }

    /// Each packet has a salt of its own, so the same payload never goes
    /// out the same twice; each opens back to its payload.
    #[test]
    fn every_packet_has_a_salt_of_its_own() {
        let salamander = Salamander::new(b"pw");
        let payload = b"the same QUIC packet, sent twice";
        let sealed: Vec<Vec<u8>> = (0..2)
            .map(|_| {
                let mut datagram = [&[0; SALT][..], payload].concat();
                salamander.seal(&mut datagram);
                datagram
            })
            .collect();
        assert_ne!(sealed[0][..SALT], sealed[1][..SALT]);
        assert_ne!(sealed[0][SALT..], sealed[1][SALT..]);
        for mut datagram in sealed {
            assert_eq!(salamander.open(&mut datagram).unwrap(), payload);
        }
    }

    /// A datagram of the salt alone, or shorter, holds nothing: dropped.
    #[test]
    fn what_holds_no_more_than_a_salt_does_not_open() {
        let salamander = Salamander::new(b"pw");
        for len in 0..=SALT {
            assert!(salamander.open(&mut vec![7; len]).is_none(), "{len}");
        }
        assert_eq!(
            salamander.open(&mut [7; SALT + 1]).map(|p| p.len()),
            Some(1)
        );
    }

    /// Nothing tells a changed packet, or another password's: either opens,
    /// to something else than was sealed.
    #[test]
    fn a_changed_packet_or_another_password_opens_to_something_else() {
        let salamander = Salamander::new(b"pw");
        let payload = b"a QUIC packet";
        let mut sealed = [&[0; SALT][..], payload].concat();
        salamander.seal(&mut sealed);
        let mut changed = sealed.clone();
        changed[0] ^= 1;
        assert_ne!(salamander.open(&mut changed).unwrap(), payload);
        let mut other = sealed.clone();
        assert_ne!(Salamander::new(b"pW").open(&mut other).unwrap(), payload);
        assert_eq!(salamander.open(&mut sealed).unwrap(), payload);
    }
}
