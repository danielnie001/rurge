//! The stream ciphers of the original protocol (shadowsocks.org's stream
//! specification; phase 2 M8 design 4.1). The master key is
//! `EVP_BytesToKey` of the password. Each direction of a connection is a
//! random IV and then the ciphertext, both directions under the one key; a
//! datagram is the same, with an IV of its own. `rc4` has no IV: one
//! keystream for every connection and every datagram of a policy, both
//! ways.
//!
//! Nothing is authenticated. Under a wrong password (or method) everything
//! still decrypts, to garbage, on both sides, and what is altered on the way
//! goes unnoticed.
//!
//! CFB128, CTR and RC4 are written here, over the single-block AES of the
//! `aes` crate, and pinned by the standards' vectors below; ChaCha20 (with
//! either nonce) and Salsa20 are the `chacha20` and `salsa20` crates'.

use super::aead::{NO_ANSWER, RESET};
use super::kdf;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::{Aes128Enc, Aes192Enc, Aes256Enc};
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::{ChaCha20, ChaCha20Legacy};
use md5::{Digest, Md5};
use rurge_config::spec::SsMethod;
use rurge_net::connector::BoxedStream;
use salsa20::Salsa20;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

const USED_UP: &str = "ss: the cipher's keystream is used up";

/// An AES block.
const BLOCK: usize = 16;

/// AES for encryption, which is all that CFB and CTR ask of it, under a key
/// of any of its lengths. The key is expanded once, here.
enum Aes {
    Aes128(Aes128Enc),
    Aes192(Aes192Enc),
    Aes256(Aes256Enc),
}

impl Aes {
    fn new(key: &[u8]) -> Aes {
        const LEN: &str = "a key of the method's length";
        match key.len() {
            16 => Aes::Aes128(Aes128Enc::new_from_slice(key).expect(LEN)),
            24 => Aes::Aes192(Aes192Enc::new_from_slice(key).expect(LEN)),
            _ => Aes::Aes256(Aes256Enc::new_from_slice(key).expect(LEN)),
        }
    }

    fn encrypt(&self, block: &mut [u8; BLOCK]) {
        let block = GenericArray::from_mut_slice(block);
        match self {
            Aes::Aes128(aes) => aes.encrypt_block(block),
            Aes::Aes192(aes) => aes.encrypt_block(block),
            Aes::Aes256(aes) => aes.encrypt_block(block),
        }
    }
}

fn block_of(iv: &[u8]) -> [u8; BLOCK] {
    iv.try_into().expect("an IV of one AES block")
}

/// AES in CFB mode with a feedback of the whole block (CFB128): a block's
/// keystream is the encryption of the ciphertext block before it, the
/// first block's that of the IV.
struct Cfb {
    aes: Aes,
    /// From `pos` on the keystream of the block under way; before `pos` its
    /// ciphertext, which is what the next block's keystream is made of.
    block: [u8; BLOCK],
    pos: usize,
}

impl Cfb {
    fn new(key: &[u8], iv: &[u8]) -> Cfb {
        Cfb {
            aes: Aes::new(key),
            block: block_of(iv),
            pos: BLOCK,
        }
    }

    fn apply(&mut self, mut data: &mut [u8], decrypt: bool) {
        while !data.is_empty() {
            if self.pos == BLOCK {
                self.aes.encrypt(&mut self.block);
                self.pos = 0;
            }
            let n = data.len().min(BLOCK - self.pos);
            let (now, later) = data.split_at_mut(n);
            for (byte, fed) in now.iter_mut().zip(&mut self.block[self.pos..]) {
                let input = *byte;
                *byte ^= *fed;
                // the ciphertext, whichever side of the XOR it is on
                *fed = if decrypt { input } else { *byte };
            }
            self.pos += n;
            data = later;
        }
    }
}

/// AES in CTR mode: a block's keystream is the encryption of a counter
/// block, a 128-bit big-endian integer that starts at the IV.
struct Ctr {
    aes: Aes,
    /// The next block's counter. It wraps after 2^128 blocks, which no
    /// stream reaches.
    counter: u128,
    /// The keystream of the block under way, used up to `pos`.
    pad: [u8; BLOCK],
    pos: usize,
}

impl Ctr {
    fn new(key: &[u8], iv: &[u8]) -> Ctr {
        Ctr {
            aes: Aes::new(key),
            counter: u128::from_be_bytes(block_of(iv)),
            pad: [0; BLOCK],
            pos: BLOCK,
        }
    }

    fn apply(&mut self, mut data: &mut [u8]) {
        while !data.is_empty() {
            if self.pos == BLOCK {
                self.pad = self.counter.to_be_bytes();
                self.aes.encrypt(&mut self.pad);
                self.counter = self.counter.wrapping_add(1);
                self.pos = 0;
            }
            let n = data.len().min(BLOCK - self.pos);
            let (now, later) = data.split_at_mut(n);
            for (byte, pad) in now.iter_mut().zip(&self.pad[self.pos..]) {
                *byte ^= pad;
            }
            self.pos += n;
            data = later;
        }
    }
}

/// RC4 (RFC 6229 has its vectors).
struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    /// `key` is not empty: a master key, or an MD5.
    fn new(key: &[u8]) -> Rc4 {
        let mut s = [0u8; 256];
        for (i, value) in s.iter_mut().enumerate() {
            *value = i as u8;
        }
        let mut j = 0u8;
        for i in 0..s.len() {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, usize::from(j));
        }
        Rc4 { s, i: 0, j: 0 }
    }

    fn apply(&mut self, data: &mut [u8]) {
        for byte in data {
            self.i = self.i.wrapping_add(1);
            let (i, a) = (usize::from(self.i), self.s[usize::from(self.i)]);
            self.j = self.j.wrapping_add(a);
            let (j, b) = (usize::from(self.j), self.s[usize::from(self.j)]);
            self.s.swap(i, j);
            *byte ^= self.s[usize::from(a.wrapping_add(b))];
        }
    }
}

enum State {
    Cfb(Cfb),
    Ctr(Ctr),
    Rc4(Rc4),
    ChaCha20(ChaCha20),
    ChaCha20Legacy(ChaCha20Legacy),
    Salsa20(Salsa20),
}

/// One direction's keystream of a stream cipher. No `Debug`.
pub(crate) struct Keystream(State);

impl Keystream {
    /// `key` is the master key (`evp_bytes_to_key`), `iv` of
    /// `method.iv_len()` bytes. `method` is a stream cipher: the callers
    /// have asked `iv_len`.
    pub(crate) fn new(method: SsMethod, key: &[u8], iv: &[u8]) -> Keystream {
        const SIZES: &str = "a key and an IV of the method's lengths";
        debug_assert_eq!(
            (key.len(), Some(iv.len())),
            (method.key_len(), method.iv_len())
        );
        Keystream(match method {
            SsMethod::Aes128Cfb | SsMethod::Aes192Cfb | SsMethod::Aes256Cfb => {
                State::Cfb(Cfb::new(key, iv))
            }
            SsMethod::Aes128Ctr | SsMethod::Aes192Ctr | SsMethod::Aes256Ctr => {
                State::Ctr(Ctr::new(key, iv))
            }
            // no IV: the same keystream whenever the key is the same
            SsMethod::Rc4 => State::Rc4(Rc4::new(key)),
            SsMethod::Rc4Md5 => {
                let connection_key = Md5::new().chain_update(key).chain_update(iv).finalize();
                State::Rc4(Rc4::new(&connection_key))
            }
            SsMethod::ChaCha20Ietf => {
                State::ChaCha20(ChaCha20::new_from_slices(key, iv).expect(SIZES))
            }
            SsMethod::ChaCha20 => {
                State::ChaCha20Legacy(ChaCha20Legacy::new_from_slices(key, iv).expect(SIZES))
            }
            SsMethod::Salsa20 => State::Salsa20(Salsa20::new_from_slices(key, iv).expect(SIZES)),
            SsMethod::None
            | SsMethod::Aes128Gcm
            | SsMethod::Aes192Gcm
            | SsMethod::Aes256Gcm
            | SsMethod::ChaCha20IetfPoly1305
            | SsMethod::XChaCha20IetfPoly1305
            | SsMethod::Blake3Aes128Gcm
            | SsMethod::Blake3Aes256Gcm => {
                unreachable!("`{}` is no stream cipher", method.name())
            }
        })
    }

    /// Encrypts `data` in place, wherever the last call ended.
    ///
    /// An error when the keystream is used up — `chacha20` and
    /// `chacha20-ietf` after 256 GiB, the `chacha20` crate counting their
    /// blocks in 32 bits. Nothing of `data` has been touched then: what
    /// would otherwise follow is the keystream over again.
    pub(crate) fn encrypt(&mut self, data: &mut [u8]) -> io::Result<()> {
        self.apply(data, false)
    }

    /// The inverse of `encrypt`, which differs from it only for CFB.
    pub(crate) fn decrypt(&mut self, data: &mut [u8]) -> io::Result<()> {
        self.apply(data, true)
    }

    fn apply(&mut self, data: &mut [u8], decrypt: bool) -> io::Result<()> {
        let applied = match &mut self.0 {
            State::Cfb(cfb) => {
                cfb.apply(data, decrypt);
                return Ok(());
            }
            State::Ctr(ctr) => {
                ctr.apply(data);
                return Ok(());
            }
            State::Rc4(rc4) => {
                rc4.apply(data);
                return Ok(());
            }
            // never the calls that panic at the end of the keystream
            State::ChaCha20(cipher) => cipher.try_apply_keystream(data),
            State::ChaCha20Legacy(cipher) => cipher.try_apply_keystream(data),
            State::Salsa20(cipher) => cipher.try_apply_keystream(data),
        };
        applied.map_err(|_| io::Error::other(USED_UP))
    }
}

#[cfg(test)]
impl Keystream {
    /// The keystream of `new` from its byte `position` on (the ChaCha
    /// ciphers and Salsa20): nobody pushes 256 GiB through a test to reach
    /// the end.
    fn at(method: SsMethod, key: &[u8], iv: &[u8], position: u128) -> Keystream {
        use chacha20::cipher::StreamCipherSeek;
        let mut keystream = Keystream::new(method, key, iv);
        match &mut keystream.0 {
            State::ChaCha20(cipher) => cipher.seek(position),
            State::ChaCha20Legacy(cipher) => cipher.seek(position),
            State::Salsa20(cipher) => cipher.seek(position),
            State::Cfb(_) | State::Ctr(_) | State::Rc4(_) => panic!("{method:?} does not seek"),
        }
        keystream
    }
}

/// A stream cipher under the master key of its policy: what every
/// connection and every datagram of the policy is keyed from. No `Debug`:
/// the key is as good as the password.
pub(crate) struct StreamKey {
    method: SsMethod,
    key: Vec<u8>,
    iv_len: usize,
}

impl StreamKey {
    /// The key is `EVP_BytesToKey(MD5)` of the password, as the AEAD
    /// methods' is. `None`: `method` is no stream cipher.
    pub(crate) fn from_password(method: SsMethod, password: &str) -> Option<StreamKey> {
        let iv_len = method.iv_len()?;
        Some(StreamKey {
            method,
            key: kdf::evp_bytes_to_key(password.as_bytes(), method.key_len()),
            iv_len,
        })
    }

    /// How many bytes of IV open each direction of a connection; 0 for
    /// `rc4`.
    pub(crate) fn iv_len(&self) -> usize {
        self.iv_len
    }

    fn keystream(&self, iv: &[u8]) -> Keystream {
        Keystream::new(self.method, &self.key, iv)
    }

    /// A datagram of `plain` (`seal_packet`).
    pub(crate) fn seal(&self, plain: &[u8]) -> io::Result<Vec<u8>> {
        seal_packet(self.method, &self.key, plain)
    }

    /// What a datagram holds (`open_packet`).
    pub(crate) fn open(&self, packet: &[u8]) -> io::Result<Vec<u8>> {
        open_packet(self.method, &self.key, packet)
    }
}

/// The most one write takes: it is encrypted into a buffer of the stream's,
/// which stays until the layer below has taken all of it.
const MAX_WRITE: usize = 16 * 1024;

const CUT_IV: &str = "ss: the connection ended in the middle of the server's IV";

/// The stream of a stream cipher over `inner`: our IV in front of the first
/// write, the server's IV in front of what it sends. Nothing of it is
/// authenticated: under a wrong password (or method) the server reads
/// garbage and so do we, and neither can tell.
///
/// A write reports success only after all it took has been handed to the
/// layer below, so the stream never depends on anyone calling `flush`; the
/// IV leaves in front of the first bytes, in the same write.
///
/// The same two contracts on the caller as `AeadStream`'s: a write that
/// returned `Pending` must be retried with the same bytes (what is parked
/// was encrypted from them and is what goes out), and a read error is
/// final.
///
/// No `Debug`: it holds the connection's keystreams.
pub(crate) struct CipherStream {
    inner: BoxedStream,
    key: Arc<StreamKey>,
    /// Our IV, until the first write takes it along.
    our_iv: Option<Vec<u8>>,
    up: Keystream,
    /// The server's IV as far as it has come; `rc4` has none.
    their_iv: Vec<u8>,
    filled: usize,
    /// Known once the server's IV is whole.
    down: Option<Keystream>,
    /// Whether the server has sent a byte yet.
    heard: bool,
    /// What is on its way out, how much of it has left, and how many of the
    /// caller's bytes it carries.
    out: Vec<u8>,
    out_pos: usize,
    accepted: usize,
}

impl CipherStream {
    /// `iv` is fresh randomness of `key.iv_len()` bytes.
    pub(crate) fn new(inner: BoxedStream, key: Arc<StreamKey>, iv: Vec<u8>) -> CipherStream {
        CipherStream {
            inner,
            up: key.keystream(&iv),
            our_iv: Some(iv),
            their_iv: vec![0; key.iv_len()],
            filled: 0,
            down: None,
            heard: false,
            key,
            out: Vec::new(),
            out_pos: 0,
            accepted: 0,
        }
    }

    fn poll_out(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.out_pos < self.out.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.out[self.out_pos..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.out_pos += n;
        }
        Poll::Ready(Ok(()))
    }
}

/// One read from the server into `buf`: how many bytes it brought, 0 at the
/// end of the stream.
///
/// The server says nothing before the target's first bytes, so a wrong
/// password (or method) and a server that refuses for any other reason look
/// the same: it goes without a byte. That end, and that reset, are told as
/// what they likely mean — as the AEAD stream tells them.
fn poll_server(
    inner: &mut BoxedStream,
    heard: &mut bool,
    cx: &mut Context<'_>,
    buf: &mut ReadBuf<'_>,
) -> Poll<io::Result<usize>> {
    let (before, room) = (buf.filled().len(), buf.remaining() > 0);
    let read = ready!(Pin::new(inner).poll_read(cx, buf));
    let n = buf.filled().len() - before;
    Poll::Ready(match read {
        Ok(()) if n == 0 && room && !*heard => {
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, NO_ANSWER))
        }
        Ok(()) => {
            *heard |= n > 0;
            Ok(n)
        }
        Err(e)
            if !*heard
                && matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                ) =>
        {
            Err(io::Error::new(e.kind(), RESET))
        }
        Err(e) => Err(e),
    })
}

impl AsyncRead for CipherStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        // the whole IV before anything is decrypted: it may come a byte at
        // a time, and what follows it may come with it
        while this.filled < this.their_iv.len() {
            let mut space = ReadBuf::new(&mut this.their_iv[this.filled..]);
            let n = ready!(poll_server(
                &mut this.inner,
                &mut this.heard,
                cx,
                &mut space
            ))?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::UnexpectedEof, CUT_IV)));
            }
            this.filled += n;
        }
        let down = this
            .down
            .get_or_insert_with(|| this.key.keystream(&this.their_iv));
        let before = buf.filled().len();
        ready!(poll_server(&mut this.inner, &mut this.heard, cx, buf))?;
        down.decrypt(&mut buf.filled_mut()[before..])?;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for CipherStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if this.out_pos == this.out.len() {
            this.out.clear();
            this.out_pos = 0;
            if let Some(iv) = this.our_iv.take() {
                this.out.extend_from_slice(&iv);
            }
            let start = this.out.len();
            let n = data.len().min(MAX_WRITE);
            this.out.extend_from_slice(&data[..n]);
            if let Err(e) = this.up.encrypt(&mut this.out[start..]) {
                // not a byte of it leaves as it is
                this.out.clear();
                return Poll::Ready(Err(e));
            }
            this.accepted = n;
        }
        ready!(this.poll_out(cx))?;
        Poll::Ready(Ok(this.accepted.min(data.len())))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_out(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        // what is parked first: its keystream is spent
        ready!(this.poll_out(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

/// The IV length of a stream cipher: the callers have no other method.
fn iv_len(method: SsMethod) -> usize {
    method.iv_len().expect("a stream cipher")
}

/// A datagram: a fresh IV, then `address ‖ payload` under it. `rc4` has no
/// IV: its packets are ciphertext from the first byte, under the same
/// keystream every time.
pub(crate) fn seal_packet(method: SsMethod, key: &[u8], plain: &[u8]) -> io::Result<Vec<u8>> {
    let mut packet = vec![0u8; iv_len(method) + plain.len()];
    let (iv, body) = packet.split_at_mut(iv_len(method));
    getrandom::fill(iv).map_err(|_| io::Error::other("ss: no randomness available"))?;
    body.copy_from_slice(plain);
    Keystream::new(method, key, iv).encrypt(body)?;
    Ok(packet)
}

/// What a datagram holds behind its IV: whatever the bytes decrypt to, for
/// nothing tells a packet under another key from one under ours.
pub(crate) fn open_packet(method: SsMethod, key: &[u8], packet: &[u8]) -> io::Result<Vec<u8>> {
    let Some((iv, body)) = packet.split_at_checked(iv_len(method)) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ss: the packet is shorter than the cipher's IV",
        ));
    };
    let mut plain = body.to_vec();
    Keystream::new(method, key, iv).decrypt(&mut plain)?;
    Ok(plain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::hex;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::task::Waker;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    /// Hex with its blocks set apart by spaces.
    fn h(text: &str) -> Vec<u8> {
        hex(&text.replace(' ', ""))
    }

    const STREAM: [SsMethod; 11] = [
        SsMethod::Rc4,
        SsMethod::Rc4Md5,
        SsMethod::Aes128Cfb,
        SsMethod::Aes192Cfb,
        SsMethod::Aes256Cfb,
        SsMethod::Aes128Ctr,
        SsMethod::Aes192Ctr,
        SsMethod::Aes256Ctr,
        SsMethod::Salsa20,
        SsMethod::ChaCha20,
        SsMethod::ChaCha20Ietf,
    ];

    /// `data` encrypted in one call.
    fn encrypt(method: SsMethod, key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = data.to_vec();
        Keystream::new(method, key, iv).encrypt(&mut out).unwrap();
        out
    }

    /// `data` decrypted in one call.
    fn decrypt(method: SsMethod, key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = data.to_vec();
        Keystream::new(method, key, iv).decrypt(&mut out).unwrap();
        out
    }

    /// The first `len` bytes of the keystream: zeros, encrypted.
    fn keystream(method: SsMethod, key: &[u8], iv: &[u8], len: usize) -> Vec<u8> {
        encrypt(method, key, iv, &vec![0u8; len])
    }

    /// A key and an IV of `method`'s lengths, and nothing special.
    fn key_and_iv(method: SsMethod) -> (Vec<u8>, Vec<u8>) {
        let key = (0..method.key_len() as u8).map(|i| i ^ 0x5a).collect();
        let iv = (0..method.iv_len().unwrap() as u8)
            .map(|i| i ^ 0xc3)
            .collect();
        (key, iv)
    }

    /// The keys of NIST SP 800-38A's examples, and their four blocks of
    /// plaintext.
    const NIST_KEYS: [&str; 3] = [
        "2b7e151628aed2a6abf7158809cf4f3c",
        "8e73b0f7da0e6452c810f32b809079e562f8ead2522c6b7b",
        "603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4",
    ];
    const NIST_PLAIN: &str = "6bc1bee22e409f96e93d7e117393172a ae2d8a571e03ac9c9eb76fac45af8e51 30c81c46a35ce411e5fbc1191a0a52ef f69f2445df4f9b17ad2b417be66c3710";

    /// NIST SP 800-38A F.3.13 to F.3.18 (CFB128-AES128, -AES192 and
    /// -AES256, encryption and decryption).
    #[test]
    fn aes_cfb_is_sp_800_38as_both_ways() {
        let iv = h("000102030405060708090a0b0c0d0e0f");
        let plain = h(NIST_PLAIN);
        for (method, key, cipher) in [
            (
                SsMethod::Aes128Cfb,
                NIST_KEYS[0],
                "3b3fd92eb72dad20333449f8e83cfb4a c8a64537a0b3a93fcde3cdad9f1ce58b 26751f67a3cbb140b1808cf187a4f4df c04b05357c5d1c0eeac4c66f9ff7f2e6",
            ),
            (
                SsMethod::Aes192Cfb,
                NIST_KEYS[1],
                "cdc80d6fddf18cab34c25909c99a4174 67ce7f7f81173621961a2b70171d3d7a 2e1e8a1dd59b88b1c8e60fed1efac4c9 c05f9f9ca9834fa042ae8fba584b09ff",
            ),
            (
                SsMethod::Aes256Cfb,
                NIST_KEYS[2],
                "dc7e84bfda79164b7ecd8486985d3860 39ffed143b28b1c832113c6331e5407b df10132415e54b92a13ed0a8267ae2f9 75a385741ab9cef82031623d55b1e471",
            ),
        ] {
            let (key, cipher) = (h(key), h(cipher));
            assert_eq!(encrypt(method, &key, &iv, &plain), cipher, "{method:?}");
            assert_eq!(decrypt(method, &key, &iv, &cipher), plain, "{method:?}");
        }
    }

    /// NIST SP 800-38A F.5.1, F.5.3 and F.5.5 (CTR-AES128, -AES192 and
    /// -AES256); decryption is the same operation.
    #[test]
    fn aes_ctr_is_sp_800_38as() {
        let iv = h("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let plain = h(NIST_PLAIN);
        for (method, key, cipher) in [
            (
                SsMethod::Aes128Ctr,
                NIST_KEYS[0],
                "874d6191b620e3261bef6864990db6ce 9806f66b7970fdff8617187bb9fffdff 5ae4df3edbd5d35e5b4f09020db03eab 1e031dda2fbe03d1792170a0f3009cee",
            ),
            (
                SsMethod::Aes192Ctr,
                NIST_KEYS[1],
                "1abc932417521ca24f2b0459fe7e6e0b 090339ec0aa6faefd5ccc2c6f4ce8e94 1e36b26bd1ebc670d1bd1d665620abf7 4f78a7f6d29809585a97daec58c6b050",
            ),
            (
                SsMethod::Aes256Ctr,
                NIST_KEYS[2],
                "601ec313775789a5b7a7f504bbf3d228 f443e3ca4d62b59aca84e990cacaf5c5 2b0930daa23de94ce87017ba2d84988d dfc9c58db67aada613c2dd08457941a6",
            ),
        ] {
            let (key, cipher) = (h(key), h(cipher));
            assert_eq!(encrypt(method, &key, &iv, &plain), cipher, "{method:?}");
            assert_eq!(decrypt(method, &key, &iv, &cipher), plain, "{method:?}");
        }
    }

    /// The counter is one 128-bit big-endian integer: from an IV of
    /// `ff … fe` the second block's counter is all ones, and the third
    /// block's carries out of every byte, to zero. Computed with OpenSSL
    /// 3.2.4 (`enc -aes-128-ctr`) over the NIST plaintext.
    #[test]
    fn the_ctr_counter_carries_through_all_sixteen_bytes() {
        let cipher = encrypt(
            SsMethod::Aes128Ctr,
            &h(NIST_KEYS[0]),
            &h("fffffffffffffffffffffffffffffffe"),
            &h(NIST_PLAIN),
        );
        assert_eq!(
            cipher,
            h(
                "ba76aa54d5b56067c1a7903b3fddfa89 24df0c565cf42a68978713b67ad124fd 4d3f774ab9e47da2dbb9315ea3110680 a18d5905ebfe25a803df27c2211e58d6"
            )
        );
    }

    /// RFC 6229, the 128-bit key `01 02 … 10`: the keystream from offset 0.
    #[test]
    fn rc4_is_rfc_6229s() {
        assert_eq!(
            keystream(
                SsMethod::Rc4,
                &h("0102030405060708090a0b0c0d0e0f10"),
                &[],
                32
            ),
            h("9ac7cc9a609d1ef7b2932899cde41b97 5248c4959014126a6e8a84f11d1a9e1c")
        );
    }

    /// `rc4-md5` is RC4 (pinned above) under MD5(key ‖ IV).
    #[test]
    fn rc4_md5_is_rc4_under_the_md5_of_the_key_and_the_iv() {
        use md5::{Digest, Md5};
        let (key, iv) = key_and_iv(SsMethod::Rc4Md5);
        let connection_key = Md5::new().chain_update(&key).chain_update(&iv).finalize();
        let expected = keystream(SsMethod::Rc4, &connection_key, &[], 300);
        assert_eq!(keystream(SsMethod::Rc4Md5, &key, &iv, 300), expected);
        // and not RC4 under the key alone
        assert_ne!(expected, keystream(SsMethod::Rc4, &key, &[], 300));
    }

    /// RFC 8439: the key of section 2.4.2 with its nonce, from block 0
    /// (computed with OpenSSL 3.2.4, `enc -chacha20`, whose IV starts with
    /// the block counter), and appendix A.1's first vector.
    #[test]
    fn chacha20_ietf_is_rfc_8439s_from_block_zero() {
        let key: Vec<u8> = (0u8..32).collect();
        assert_eq!(
            keystream(
                SsMethod::ChaCha20Ietf,
                &key,
                &h("000000000000004a00000000"),
                64
            ),
            h(
                "af051e40bba0354981329a806a140eaf d258a22a6dcb4bb9f6569cb3efe2deaf 837bd87ca20b5ba12081a306af0eb35c 41a239d20dfc74c81771560d9c9c1e4b"
            )
        );
        assert_eq!(
            keystream(SsMethod::ChaCha20Ietf, &[0; 32], &[0; 12], 64),
            h(
                "76b8e0ada0f13d90405d6ae55386bd28 bdd219b8a08ded1aa836efcc8b770dc7 da41597c5157488d7724e03fb8d84a37 6a43b8f41518a11cc387b669b2ee6586"
            )
        );
    }

    /// The original ChaCha20, with its nonce of 8 bytes: the key `00 01 …
    /// 1f` and the nonce `00 01 … 07` (computed with OpenSSL 3.2.4).
    #[test]
    fn chacha20_is_the_original_with_a_nonce_of_eight_bytes() {
        let key: Vec<u8> = (0u8..32).collect();
        assert_eq!(
            keystream(SsMethod::ChaCha20, &key, &h("0001020304050607"), 64),
            h(
                "f798a189f195e66982105ffb640bb775 7f579da31602fc93ec01ac56f85ac3c1 34a4547b733b46413042c94400491769 05d3be59ea1c53f15916155c2be8241a"
            )
        );
    }

    /// ECRYPT's Salsa20/20 vectors for 256-bit keys, set 1, vector 0.
    #[test]
    fn salsa20_is_ecrypts() {
        let mut key = [0u8; 32];
        key[0] = 0x80;
        assert_eq!(
            keystream(SsMethod::Salsa20, &key, &[0; 8], 64),
            h(
                "e3be8fdd8beca2e3ea8ef9475b29a6e7 003951e1097a5c38d23b7a5fad9f6844 b22c97559e2723c7cbbd3fe4fc8d9a07 44652a83e72a9c461876af4d7ef1a117"
            )
        );
    }

    /// The same vectors, set 6, vector 0, whose IV is not zeros: the one
    /// above would hold for a cipher that never took its IV. No server that
    /// speaks `salsa20` is at hand to tell, so its IV is pinned here.
    #[test]
    fn salsa20_under_an_iv_is_ecrypts_too() {
        assert_eq!(
            keystream(
                SsMethod::Salsa20,
                &h("0053a6f94c9ff24598eb3e91e4378add3083d6297ccf2275c81b6ec11467ba0d"),
                &h("0d74db42a91077de"),
                64
            ),
            h(
                "f5fad53f79f9df58c4aea0d0ed9a9601 f278112ca7180d565b420a48019670ea f24ce493a86263f677b46ace1924773d 2bb25571e1aa8593758fc382b1280b71"
            )
        );
    }

    /// A stream is encrypted as its writes come: wherever a call ends, in
    /// the middle of a block too, the next one goes on from there.
    #[test]
    fn every_cipher_goes_on_where_the_last_call_ended() {
        let data: Vec<u8> = (0..100 * 1024).map(|i: u32| (i % 251) as u8).collect();
        for method in STREAM {
            let (key, iv) = key_and_iv(method);
            let whole = encrypt(method, &key, &iv, &data);
            assert_ne!(whole, data, "{method:?}");
            let lengths = [1, 15, 16, 17, 4096, 3, 64, 65, 1000];
            let (mut up, mut down) = (
                Keystream::new(method, &key, &iv),
                Keystream::new(method, &key, &iv),
            );
            let (mut pieces, mut back) = (data.clone(), whole.clone());
            let (mut at, mut turn) = (0, 0);
            while at < data.len() {
                let end = data.len().min(at + lengths[turn % lengths.len()]);
                up.encrypt(&mut pieces[at..end]).unwrap();
                down.decrypt(&mut back[at..end]).unwrap();
                (at, turn) = (end, turn + 1);
            }
            assert!(pieces == whole, "{method:?}: encrypted in pieces");
            assert!(back == data, "{method:?}: decrypted in pieces");
            assert!(decrypt(method, &key, &iv, &whole) == data, "{method:?}");
        }
    }

    /// The ChaCha ciphers of the `chacha20` crate count their blocks in 32
    /// bits (the original counts in 64), Salsa20 in 64: where the counter
    /// ends, so does the stream — with an error, not a panic, and without
    /// a byte encrypted under a keystream that has wrapped.
    #[test]
    fn a_keystream_that_is_used_up_is_an_error_and_no_panic() {
        for (method, blocks) in [
            (SsMethod::ChaCha20Ietf, u128::from(u32::MAX)),
            (SsMethod::ChaCha20, u128::from(u32::MAX)),
            (SsMethod::Salsa20, u128::from(u64::MAX)),
        ] {
            let (key, iv) = key_and_iv(method);
            let end = blocks * 64;
            let mut keystream = Keystream::at(method, &key, &iv, end - 100);
            let mut data = [0u8; 100];
            keystream.encrypt(&mut data).unwrap();
            assert_ne!(data, [0u8; 100], "{method:?}");
            for len in [1, 64, 1000] {
                let mut more = vec![0x5au8; len];
                let err = keystream.encrypt(&mut more).unwrap_err();
                assert_eq!(err.to_string(), "ss: the cipher's keystream is used up");
                assert_eq!(more, vec![0x5au8; len], "{method:?}: left as it was");
                assert!(keystream.decrypt(&mut more).is_err(), "{method:?}");
            }
            // a call that would run over the end takes nothing either
            let mut keystream = Keystream::at(method, &key, &iv, end - 10);
            let mut data = [0x5au8; 11];
            assert!(keystream.encrypt(&mut data).is_err(), "{method:?}");
            assert_eq!(data, [0x5au8; 11], "{method:?}");
            keystream.encrypt(&mut data[..10]).unwrap();
        }
    }

    const PASSWORD: &str = "password";

    fn key(method: SsMethod) -> Arc<StreamKey> {
        Arc::new(StreamKey::from_password(method, PASSWORD).expect("a stream cipher"))
    }

    /// The master key a server derives from the password, by itself.
    fn master(method: SsMethod) -> Vec<u8> {
        kdf::evp_bytes_to_key(PASSWORD.as_bytes(), method.key_len())
    }

    /// An IV of `method`'s length: `first`, `first + 1`, …
    fn iv(method: SsMethod, first: u8) -> Vec<u8> {
        (0..method.iv_len().unwrap() as u8)
            .map(|i| first.wrapping_add(i))
            .collect()
    }

    const OURS: u8 = 0x10;
    const THEIRS: u8 = 0x80;

    /// A stream over `inner` that sends the IV `OURS`.
    fn stream<S>(method: SsMethod, inner: S) -> CipherStream
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        CipherStream::new(Box::new(inner), key(method), iv(method, OURS))
    }

    /// What a server sends for `plain`: its IV `THEIRS`, then the ciphertext.
    fn answer(method: SsMethod, plain: &[u8]) -> Vec<u8> {
        let iv = iv(method, THEIRS);
        let mut body = plain.to_vec();
        Keystream::new(method, &master(method), &iv)
            .encrypt(&mut body)
            .unwrap();
        [iv, body].concat()
    }

    fn pattern(len: usize, modulus: u32) -> Vec<u8> {
        (0..len as u32).map(|i| (i % modulus) as u8).collect()
    }

    #[test]
    fn a_stream_key_is_a_stream_ciphers_only() {
        for method in STREAM {
            assert_eq!(Some(key(method).iv_len()), method.iv_len(), "{method:?}");
        }
        for method in [
            SsMethod::None,
            SsMethod::Aes256Gcm,
            SsMethod::Blake3Aes128Gcm,
        ] {
            assert!(
                StreamKey::from_password(method, PASSWORD).is_none(),
                "{method:?}"
            );
        }
    }

    /// The sizes a `Choppy` pipe moves at a time, one after the other.
    const SIZES: [usize; 8] = [1, 2, 3, 7, 16, 17, 500, 4096];

    /// A pipe that gives and takes a few bytes at a time. Its first reads
    /// give one byte each: an IV comes as slowly as it can. Now and then a
    /// write is not ready, and the caller has to come back with the same
    /// bytes.
    struct Choppy {
        pipe: DuplexStream,
        reads: usize,
        writes: usize,
    }

    impl Choppy {
        fn new(pipe: DuplexStream) -> Choppy {
            Choppy {
                pipe,
                reads: 0,
                writes: 0,
            }
        }
    }

    impl AsyncRead for Choppy {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let size = if self.reads < 24 {
                1
            } else {
                SIZES[self.reads % SIZES.len()]
            };
            let size = size.min(buf.remaining());
            let mut space = ReadBuf::new(buf.initialize_unfilled_to(size));
            ready!(Pin::new(&mut self.pipe).poll_read(cx, &mut space))?;
            let n = space.filled().len();
            buf.advance(n);
            self.reads += 1;
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for Choppy {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.writes += 1;
            if self.writes % 5 == 0 {
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            let size = SIZES[self.writes % SIZES.len()].min(data.len());
            Pin::new(&mut self.pipe).poll_write(cx, &data[..size])
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.pipe).poll_flush(cx)
        }

        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.pipe).poll_shutdown(cx)
        }
    }

    /// Both ends speak as a client does — its IV, then its ciphertext — so
    /// two streams of one key understand each other.
    #[tokio::test]
    async fn two_streams_round_trip_whatever_the_slicing_below() {
        let (there, back) = (pattern(300 * 1024, 251), pattern(300 * 1024, 241));
        for method in STREAM {
            let (near, far) = tokio::io::duplex(64 * 1024);
            let one = CipherStream::new(Box::new(Choppy::new(near)), key(method), iv(method, OURS));
            let other =
                CipherStream::new(Box::new(Choppy::new(far)), key(method), iv(method, THEIRS));
            let (mut one_read, mut one_write) = tokio::io::split(one);
            let (mut other_read, mut other_write) = tokio::io::split(other);
            let (mut got_there, mut got_back) = (vec![0u8; there.len()], vec![0u8; back.len()]);
            let all = async {
                tokio::try_join!(
                    one_write.write_all(&there),
                    other_write.write_all(&back),
                    other_read.read_exact(&mut got_there),
                    one_read.read_exact(&mut got_back),
                )
            };
            tokio::time::timeout(Duration::from_secs(30), all)
                .await
                .expect("both ways within the bound")
                .unwrap_or_else(|e| panic!("{method:?}: {e}"));
            assert!(got_there == there, "{method:?}: there");
            assert!(got_back == back, "{method:?}: back");
        }
    }

    /// Passes everything on and keeps what each write handed down.
    struct Tap {
        pipe: DuplexStream,
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl AsyncRead for Tap {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.pipe).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for Tap {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            let n = ready!(Pin::new(&mut self.pipe).poll_write(cx, data))?;
            self.writes.lock().unwrap().push(data[..n].to_vec());
            Poll::Ready(Ok(n))
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.pipe).poll_flush(cx)
        }

        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.pipe).poll_shutdown(cx)
        }
    }

    /// The IV leaves in front of the first bytes, in the same write: a lone
    /// first segment of an IV's length would be a signature. `rc4` has no
    /// IV, and its first byte on the wire is ciphertext.
    #[tokio::test]
    async fn the_wire_is_our_iv_and_then_the_ciphertext() {
        let long = pattern(40, 251);
        for method in STREAM {
            let (near, mut far) = tokio::io::duplex(1 << 20);
            let writes = Arc::new(Mutex::new(Vec::new()));
            let tap = Tap {
                pipe: near,
                writes: writes.clone(),
            };
            let mut stream = stream(method, tap);
            // an empty write sends nothing, not the IV either
            assert_eq!(stream.write(b"").await.unwrap(), 0);
            stream.write_all(b"hello").await.unwrap();
            stream.write_all(&long).await.unwrap();
            // more than one write takes: the rest is the caller's to bring again
            let large = vec![0x55u8; MAX_WRITE + 1];
            assert_eq!(stream.write(&large).await.unwrap(), MAX_WRITE, "{method:?}");
            stream.shutdown().await.unwrap();
            let mut wire = Vec::new();
            far.read_to_end(&mut wire).await.unwrap();

            let iv = iv(method, OURS);
            let writes = writes.lock().unwrap().clone();
            let lengths: Vec<usize> = writes.iter().map(Vec::len).collect();
            assert_eq!(lengths, [iv.len() + 5, 40, MAX_WRITE], "{method:?}");
            assert_eq!(wire, writes.concat(), "{method:?}");
            assert_eq!(wire[..iv.len()], iv[..], "{method:?}");
            let mut plain = wire[iv.len()..].to_vec();
            Keystream::new(method, &master(method), &iv)
                .decrypt(&mut plain)
                .unwrap();
            let sent = [b"hello", &long[..], &large[..MAX_WRITE]].concat();
            assert!(plain == sent, "{method:?}");
        }
    }

    /// What a `Trickle` took, and how much of it was there when a flush and
    /// when a shutdown got to it.
    #[derive(Default)]
    struct Trickled {
        bytes: Vec<u8>,
        flushed_at: Vec<usize>,
        shut_at: Vec<usize>,
    }

    /// A transport that takes three bytes at a time and is not ready every
    /// other time it is asked, waking whoever asked.
    struct Trickle {
        /// The last write was told to wait: the next one is taken.
        waited: bool,
        seen: Arc<Mutex<Trickled>>,
    }

    impl AsyncRead for Trickle {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for Trickle {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            if !self.waited {
                self.waited = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            self.waited = false;
            let n = data.len().min(3);
            let mut seen = self.seen.lock().unwrap();
            seen.bytes.extend_from_slice(&data[..n]);
            Poll::Ready(Ok(n))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            let mut seen = self.seen.lock().unwrap();
            let so_far = seen.bytes.len();
            seen.flushed_at.push(so_far);
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            let mut seen = self.seen.lock().unwrap();
            let so_far = seen.bytes.len();
            seen.shut_at.push(so_far);
            Poll::Ready(Ok(()))
        }
    }

    /// A caller whose write was not taken at once may flush, or shut the
    /// stream down, with ciphertext still parked. The keystream of those
    /// bytes is spent, so they go down first — all of them, in order and
    /// once — and only then does the flush or the shutdown reach the
    /// transport.
    #[test]
    fn a_flush_or_a_shutdown_sends_what_is_parked_first() {
        let payload = pattern(40, 251);
        let mut cx = Context::from_waker(Waker::noop());
        for shutdown in [false, true] {
            for method in STREAM {
                let seen = Arc::new(Mutex::new(Trickled::default()));
                let transport = Trickle {
                    waited: false,
                    seen: seen.clone(),
                };
                let mut stream = stream(method, transport);
                // not ready; three bytes and not ready; three more and not
                // ready: the write is still the caller's to bring again
                for _ in 0..3 {
                    let write = Pin::new(&mut stream).poll_write(&mut cx, &payload);
                    assert!(write.is_pending(), "{method:?}");
                }
                assert_eq!(seen.lock().unwrap().bytes.len(), 6, "{method:?}");
                let mut waits = 0;
                loop {
                    let stream = Pin::new(&mut stream);
                    let end = if shutdown {
                        stream.poll_shutdown(&mut cx)
                    } else {
                        stream.poll_flush(&mut cx)
                    };
                    match end {
                        Poll::Ready(end) => break end.unwrap(),
                        Poll::Pending => waits += 1,
                    }
                    assert!(waits < 1000, "{method:?}: it never ends");
                }

                let seen = seen.lock().unwrap();
                let iv = iv(method, OURS);
                let whole = iv.len() + payload.len();
                // the transport had everything when the flush (the
                // shutdown) got to it, and heard of it once
                let (flushed_at, shut_at) = if shutdown {
                    (vec![], vec![whole])
                } else {
                    (vec![whole], vec![])
                };
                assert_eq!(seen.flushed_at, flushed_at, "{method:?}");
                assert_eq!(seen.shut_at, shut_at, "{method:?}");
                assert_eq!(seen.bytes.len(), whole, "{method:?}: once");
                assert_eq!(seen.bytes[..iv.len()], iv[..], "{method:?}");
                let mut plain = seen.bytes[iv.len()..].to_vec();
                Keystream::new(method, &master(method), &iv)
                    .decrypt(&mut plain)
                    .unwrap();
                assert!(plain == payload, "{method:?}: in order");
                assert!(waits > 0, "{method:?}: it had the transport to wait for");
            }
        }
    }

    /// What a stream reads from a server that sends `wire` through a pipe
    /// of `capacity` bytes and then closes.
    async fn read_from(method: SsMethod, wire: Vec<u8>, capacity: usize) -> io::Result<Vec<u8>> {
        let (near, mut far) = tokio::io::duplex(capacity);
        let all_at_once = capacity >= wire.len();
        let sender = tokio::spawn(async move {
            let _ = far.write_all(&wire).await;
            // dropping `far` is the close
        });
        if all_at_once {
            // everything is there before the first read: the IV glued to
            // what follows it
            sender.await.unwrap();
        }
        let mut stream = stream(method, near);
        let mut got = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut got))
            .await
            .expect("the end within the bound")
            .map(|_| got)
    }

    #[tokio::test]
    async fn the_servers_stream_reads_back_whatever_the_slicing() {
        let plain = pattern(5000, 251);
        for method in STREAM {
            let wire = answer(method, &plain);
            assert_eq!(wire.len(), method.iv_len().unwrap() + plain.len());
            for capacity in [1, 7, 11, 1 << 16] {
                let got = read_from(method, wire.clone(), capacity).await.unwrap();
                assert!(got == plain, "{method:?} through {capacity}");
            }
        }
    }

    /// Nothing is decrypted before the server's IV is whole, however it
    /// comes; `rc4` has none to wait for.
    #[tokio::test]
    async fn a_reader_waits_for_the_whole_iv_and_rc4_for_none() {
        let mut cx = Context::from_waker(Waker::noop());
        for method in STREAM {
            let wire = answer(method, b"x");
            let iv_len = method.iv_len().unwrap();
            let (near, mut far) = tokio::io::duplex(64);
            let mut stream = stream(method, near);
            let mut space = [0u8; 8];
            let mut buf = ReadBuf::new(&mut space);
            for byte in &wire[..iv_len] {
                assert!(
                    Pin::new(&mut stream)
                        .poll_read(&mut cx, &mut buf)
                        .is_pending(),
                    "{method:?}"
                );
                far.write_all(std::slice::from_ref(byte)).await.unwrap();
            }
            // the IV is whole (`rc4`: there is none), and nothing follows yet
            assert!(
                Pin::new(&mut stream)
                    .poll_read(&mut cx, &mut buf)
                    .is_pending(),
                "{method:?}"
            );
            assert!(buf.filled().is_empty(), "{method:?}");
            far.write_all(&wire[iv_len..]).await.unwrap();
            assert!(
                matches!(
                    Pin::new(&mut stream).poll_read(&mut cx, &mut buf),
                    Poll::Ready(Ok(()))
                ),
                "{method:?}"
            );
            assert_eq!(buf.filled(), b"x", "{method:?}");
        }
    }

    /// The server says nothing before the target's first bytes, so one that
    /// goes without a byte gave no answer: the likeliest reason is a
    /// password or a method it does not share. Once its IV has begun, an end
    /// is an end.
    #[tokio::test]
    async fn a_server_that_goes_before_its_first_byte_gave_no_answer() {
        for method in STREAM {
            let err = read_from(method, Vec::new(), 64).await.unwrap_err();
            assert_eq!(
                (err.kind(), err.to_string().as_str()),
                (
                    io::ErrorKind::UnexpectedEof,
                    "ss: the server closed the connection without answering"
                ),
                "{method:?}"
            );
            let iv = iv(method, THEIRS);
            if let Some((_, cut)) = iv.split_last() {
                let err = read_from(method, cut.to_vec(), 64).await.unwrap_err();
                assert_eq!(
                    (err.kind(), err.to_string().as_str()),
                    (
                        io::ErrorKind::UnexpectedEof,
                        "ss: the connection ended in the middle of the server's IV"
                    ),
                    "{method:?}"
                );
                // an IV and nothing behind it: an answer, and an empty one
                assert_eq!(read_from(method, iv, 64).await.unwrap(), b"", "{method:?}");
            }
            // closed after some bytes: an ordinary end
            let got = read_from(method, answer(method, b"some"), 64).await;
            assert_eq!(got.unwrap(), b"some", "{method:?}");
        }
    }

    /// A stream that reads what its script says, a step for each read, and
    /// then ends; whatever is written to it is gone.
    struct Scripted(VecDeque<io::Result<Vec<u8>>>);

    impl AsyncRead for Scripted {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            match self.0.pop_front() {
                None => Poll::Ready(Ok(())),
                Some(Err(e)) => Poll::Ready(Err(e)),
                Some(Ok(mut bytes)) => {
                    let n = bytes.len().min(buf.remaining());
                    buf.put_slice(&bytes[..n]);
                    if n < bytes.len() {
                        self.0.push_front(Ok(bytes.split_off(n)));
                    }
                    Poll::Ready(Ok(()))
                }
            }
        }
    }

    impl AsyncWrite for Scripted {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(data.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// Some servers reset a connection whose request they cannot use, and
    /// with a stream cipher that is any request under a wrong password. A
    /// reset later on is the network's, and is told as it is.
    #[tokio::test]
    async fn a_reset_before_the_first_byte_says_what_it_likely_means() {
        let reset = || io::Error::new(io::ErrorKind::ConnectionReset, "reset by the peer");
        for method in [SsMethod::Aes256Cfb, SsMethod::Rc4] {
            for kind in [
                io::ErrorKind::ConnectionReset,
                io::ErrorKind::ConnectionAborted,
            ] {
                let script = Scripted([Err(io::Error::new(kind, "by the peer"))].into());
                let err = stream(method, script)
                    .read(&mut [0u8; 8])
                    .await
                    .unwrap_err();
                assert_eq!(
                    (err.kind(), err.to_string().as_str()),
                    (
                        kind,
                        "ss: the server reset the connection without answering (wrong password or method?)"
                    ),
                    "{method:?}"
                );
            }
            // another error, and a reset after the server's first bytes
            let script = Scripted([Err(io::Error::other("the cable"))].into());
            let err = stream(method, script)
                .read(&mut [0u8; 8])
                .await
                .unwrap_err();
            assert_eq!(err.to_string(), "the cable", "{method:?}");
            let mut answered = stream(
                method,
                Scripted([Ok(answer(method, b"some")), Err(reset())].into()),
            );
            let mut got = [0u8; 4];
            answered.read_exact(&mut got).await.unwrap();
            assert_eq!(&got, b"some", "{method:?}");
            let err = answered.read(&mut got).await.unwrap_err();
            assert_eq!(
                (err.kind(), err.to_string().as_str()),
                (io::ErrorKind::ConnectionReset, "reset by the peer"),
                "{method:?}"
            );
        }
    }

    /// Past the end of its keystream a stream fails, either way, and what
    /// it was given to send and could not encrypt never reaches the wire.
    #[tokio::test]
    async fn a_stream_past_its_keystream_fails_and_sends_nothing_unencrypted() {
        const TEXT: &str = "ss: the cipher's keystream is used up";
        let end = u128::from(u32::MAX) * 64;
        for method in [SsMethod::ChaCha20Ietf, SsMethod::ChaCha20] {
            let (near, mut far) = tokio::io::duplex(1 << 16);
            let mut out = stream(method, near);
            out.write_all(b"before").await.unwrap();
            // three bytes of keystream are left
            out.up = Keystream::at(method, &master(method), &iv(method, OURS), end - 3);
            for _ in 0..2 {
                let err = out.write(b"secret").await.unwrap_err();
                assert_eq!(err.to_string(), TEXT, "{method:?}");
            }
            out.flush().await.unwrap();
            out.shutdown().await.unwrap();
            let mut wire = Vec::new();
            far.read_to_end(&mut wire).await.unwrap();
            assert_eq!(
                wire.len(),
                method.iv_len().unwrap() + b"before".len(),
                "{method:?}"
            );

            let (near, mut far) = tokio::io::duplex(1 << 16);
            let mut back = stream(method, near);
            far.write_all(&answer(method, b"some")).await.unwrap();
            let mut got = [0u8; 4];
            back.read_exact(&mut got).await.unwrap();
            assert_eq!(&got, b"some", "{method:?}");
            let theirs = iv(method, THEIRS);
            back.down = Some(Keystream::at(method, &master(method), &theirs, end - 3));
            far.write_all(b"more!").await.unwrap();
            let err = back.read(&mut [0u8; 16]).await.unwrap_err();
            assert_eq!(err.to_string(), TEXT, "{method:?}");
        }
    }

    /// 127.0.0.1:53 as a SOCKS5 address: what a datagram starts with.
    const ADDRESS: [u8; 7] = [1, 127, 0, 0, 1, 0, 53];

    #[test]
    fn a_packet_is_a_fresh_iv_and_everything_else_under_it() {
        let plain = [&ADDRESS[..], b"query"].concat();
        for method in STREAM {
            let key = master(method);
            let iv_len = method.iv_len().unwrap();
            let packet = seal_packet(method, &key, &plain).unwrap();
            assert_eq!(packet.len(), iv_len + plain.len(), "{method:?}");
            // what follows the IV decrypts under it (`rc4`: the packet is
            // ciphertext from its first byte)
            let mut body = packet[iv_len..].to_vec();
            Keystream::new(method, &key, &packet[..iv_len])
                .decrypt(&mut body)
                .unwrap();
            assert_eq!(body, plain, "{method:?}");
            assert_eq!(open_packet(method, &key, &packet).unwrap(), plain);
            // every packet has an IV of its own; `rc4` has none, and says
            // the same thing the same way every time
            let again = seal_packet(method, &key, &plain).unwrap();
            assert_eq!(again == packet, method == SsMethod::Rc4, "{method:?}");
            assert_eq!(open_packet(method, &key, &again).unwrap(), plain);
            // an address with an empty payload behind it, and nothing at all
            for short in [&ADDRESS[..], &[]] {
                let packet = seal_packet(method, &key, short).unwrap();
                assert_eq!(packet.len(), iv_len + short.len(), "{method:?}");
                assert_eq!(open_packet(method, &key, &packet).unwrap(), short);
            }
        }
    }

    #[test]
    fn a_packet_shorter_than_its_iv_is_an_error() {
        for method in STREAM {
            let key = master(method);
            let iv_len = method.iv_len().unwrap();
            for len in 0..iv_len {
                let err = open_packet(method, &key, &vec![0x5a; len]).unwrap_err();
                assert_eq!(
                    (err.kind(), err.to_string().as_str()),
                    (
                        io::ErrorKind::InvalidData,
                        "ss: the packet is shorter than the cipher's IV"
                    ),
                    "{method:?}: {len} bytes"
                );
            }
            // an IV alone: a packet of nothing
            assert!(
                open_packet(method, &key, &vec![0x5a; iv_len])
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
