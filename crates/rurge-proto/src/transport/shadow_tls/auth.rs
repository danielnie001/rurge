//! The keyed digests of Shadow TLS: every one of them is HMAC-SHA1 under the
//! password, read a few bytes at a time while more data keeps going in.

use ring::hmac;
use sha2::{Digest, Sha256};

/// v3 puts 4 bytes of a digest in front of a payload; v2 sends 8, once.
pub(crate) const TAG: usize = 4;
pub(crate) const V2_TAG: usize = 8;

/// A running HMAC-SHA1. No `Debug`: it is keyed with the password. Boxed:
/// ring's context is some 300 bytes, and a stream holds three of them.
#[derive(Clone)]
pub(crate) struct Chain(Box<hmac::Context>);

impl Chain {
    pub(crate) fn new(password: &[u8], seed: &[&[u8]]) -> Chain {
        let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, password);
        let mut chain = Chain(Box::new(hmac::Context::with_key(&key)));
        for part in seed {
            chain.update(part);
        }
        chain
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    /// The first `N` bytes of the digest of everything fed so far. The chain
    /// itself goes on.
    pub(crate) fn digest<const N: usize>(&self) -> [u8; N] {
        let full = hmac::Context::clone(&self.0).sign();
        let mut out = [0u8; N];
        out.copy_from_slice(&full.as_ref()[..N]);
        out
    }

    /// A data frame of v3: the payload goes in, 4 bytes come out, and those 4
    /// bytes go in as well.
    pub(crate) fn frame_tag(&mut self, payload: &[u8]) -> [u8; TAG] {
        self.update(payload);
        let tag = self.digest::<TAG>();
        self.update(&tag);
        tag
    }
}

/// Compares every byte: no early exit on the first difference.
pub(crate) fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// v3: what a server XORs the handshake server's ApplicationData with.
pub(crate) fn xor_key(password: &[u8], server_random: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(password);
    hasher.update(server_random);
    hasher.finalize().into()
}

/// The key starts over with every record.
pub(crate) fn xor(data: &mut [u8], key: &[u8; 32]) {
    for (byte, k) in data.iter_mut().zip(key.iter().cycle()) {
        *byte ^= k;
    }
}
