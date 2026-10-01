//! The Snell v4 / v5 record stream (phase 2 M6 design 4.3). Each direction
//! starts with its own salt, then records:
//!
//! ```text
//! sealed(04 00 00 ‖ padding length, u16 BE ‖ payload length, u16 BE)
//! padding
//! sealed(payload)                 (absent when the payload is empty)
//! ```
//!
//! both sealed with the direction's AES-128-GCM key and the next values of
//! its counting nonce (the header one, the payload the next). Only the first
//! record of a direction is padded — 256 to 511 random bytes — and its
//! padding and payload ciphertext are mixed: the bytes at the even indices
//! below the shorter of the two trade places. A record with an empty payload
//! ends the direction (a half-close); reads then return end-of-file.
//!
//! One stream carries one request at a time (`reuse`, design 4.3): once
//! both directions have ended, `next_tunnel` opens the next request on the
//! same salts, keys and nonce counters, without padding.
//!
//! Every record we write is at most `MAX_PAYLOAD` bytes; Surge's growing
//! record sizes (and v5's dynamic record sizing on the server) only shape
//! traffic, and receivers take any length (M6-D7). A write reports success
//! only after its record has been handed to the layer below, so the stream
//! never depends on anyone calling `flush`.
//!
//! The server's key is derived when its salt arrives, inside a read: the
//! Argon2id runs on a blocking thread whose task the read polls.
//!
//! Two contracts on the caller, as for `AeadStream`: a write that returned
//! `Pending` must be retried with the same bytes, and an error is final.

use super::kdf::{KEY_LEN, Psk, SALT_LEN, key_failed};
use crate::shadowsocks::cipher::{AeadKind, CountingAead, TAG};
use rurge_net::connector::BoxedStream;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::task::JoinHandle;

/// The largest payload we put in a record.
pub(crate) const MAX_PAYLOAD: usize = 0x3FFF;
/// The version byte of v4's records, which v5 keeps.
const VERSION: u8 = 0x04;
const HEADER: usize = 7;
/// The first record's padding: `PADDING_MIN + (0..256)` bytes.
const PADDING_MIN: usize = 0x100;

const NO_ANSWER: &str = "snell: the server closed the connection without answering";
const UNDECRYPTABLE: &str = "snell: the server's data failed to decrypt (wrong psk or version?)";
const CUT_SHORT: &str = "snell: the connection ended in the middle of a record";
const UNKNOWN_VERSION: &str = "snell: the server sent a record of an unknown version";
const ENDED: &str = "snell: the request's sending side has already ended";

fn invalid(text: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

fn cut_short() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, CUT_SHORT)
}

fn no_randomness(_: getrandom::Error) -> io::Error {
    io::Error::other("snell: no randomness available")
}

/// A fresh salt.
pub(crate) fn new_salt() -> io::Result<[u8; SALT_LEN]> {
    let mut salt = [0u8; SALT_LEN];
    getrandom::fill(&mut salt).map_err(no_randomness)?;
    Ok(salt)
}

/// The first record's padding: 256 to 511 random bytes.
pub(crate) fn first_padding() -> io::Result<Vec<u8>> {
    let mut len = [0u8; 1];
    getrandom::fill(&mut len).map_err(no_randomness)?;
    let mut padding = vec![0u8; PADDING_MIN + usize::from(len[0])];
    getrandom::fill(&mut padding).map_err(no_randomness)?;
    Ok(padding)
}

/// Trades the bytes at the even indices of `padding` and `sealed` below the
/// shorter length; its own inverse.
fn mix(padding: &mut [u8], sealed: &mut [u8]) {
    for i in (0..padding.len().min(sealed.len())).step_by(2) {
        std::mem::swap(&mut padding[i], &mut sealed[i]);
    }
}

/// Appends one record: `payload` (at most `MAX_PAYLOAD` bytes; empty ends
/// the direction), after `padding`.
fn seal_record(up: &mut CountingAead, padding: &mut [u8], payload: &[u8], out: &mut Vec<u8>) {
    let padding_len = u16::try_from(padding.len()).expect("the padding is below 512 bytes");
    let payload_len = u16::try_from(payload.len()).expect("a record's payload fits two bytes");
    let mut header = [0u8; HEADER];
    header[0] = VERSION;
    header[3..5].copy_from_slice(&padding_len.to_be_bytes());
    header[5..7].copy_from_slice(&payload_len.to_be_bytes());
    up.seal(&header, out);
    let start = out.len();
    out.extend_from_slice(padding);
    if !payload.is_empty() {
        up.seal(payload, out);
        let (padding, sealed) = out[start..].split_at_mut(padding.len());
        mix(padding, sealed);
    }
}

enum Reading {
    Salt {
        buf: [u8; SALT_LEN],
        filled: usize,
    },
    /// The server's key, on a blocking thread.
    Key(JoinHandle<[u8; KEY_LEN]>),
    Header {
        buf: [u8; HEADER + TAG],
        filled: usize,
    },
    /// The padding and the sealed payload (none when it is empty).
    Body {
        buf: Vec<u8>,
        filled: usize,
        padding: usize,
    },
    Payload {
        buf: Vec<u8>,
        pos: usize,
        end: usize,
    },
    /// The server's empty record: its side of this request is over.
    Ended,
    /// The connection closed between two records.
    Closed,
}

/// Fills `buf[*filled..]`. `Ok(false)`: the peer closed before the first byte.
fn poll_fill(
    inner: &mut BoxedStream,
    cx: &mut Context<'_>,
    buf: &mut [u8],
    filled: &mut usize,
) -> Poll<io::Result<bool>> {
    while *filled < buf.len() {
        let mut space = ReadBuf::new(&mut buf[*filled..]);
        ready!(Pin::new(&mut *inner).poll_read(cx, &mut space))?;
        let n = space.filled().len();
        if n == 0 {
            return Poll::Ready(if *filled == 0 {
                Ok(false)
            } else {
                Err(cut_short())
            });
        }
        *filled += n;
    }
    Poll::Ready(Ok(true))
}

/// No `Debug`: it holds the connection's keys.
pub(crate) struct SnellStream {
    inner: BoxedStream,
    psk: Psk,
    /// Ours, sent in front of the first record; kept to tell a reflected
    /// stream apart.
    salt: [u8; SALT_LEN],
    salt_sent: bool,
    /// The first record's, until it has gone.
    padding: Option<Vec<u8>>,
    up: CountingAead,
    /// Known once the server's salt has arrived and its key is derived.
    down: Option<CountingAead>,
    /// The record being written, how much of it is out, and how many payload
    /// bytes it carries.
    out: Vec<u8>,
    out_pos: usize,
    accepted: usize,
    /// Our empty record of this request is sealed (maybe still in `out`).
    write_ended: bool,
    reading: Reading,
    /// An error was returned: the stream is not reusable.
    failed: bool,
}

impl SnellStream {
    /// `up_key` is `salt`'s key under `psk`; `padding` goes into the first
    /// record that carries a payload (`first_padding`).
    pub(crate) fn new(
        inner: BoxedStream,
        psk: Psk,
        salt: [u8; SALT_LEN],
        up_key: [u8; KEY_LEN],
        padding: Vec<u8>,
    ) -> SnellStream {
        SnellStream {
            inner,
            psk,
            salt,
            salt_sent: false,
            padding: Some(padding),
            up: CountingAead::new(AeadKind::Aes128Gcm, &up_key),
            down: None,
            out: Vec::new(),
            out_pos: 0,
            accepted: 0,
            write_ended: false,
            reading: Reading::Salt {
                buf: [0; SALT_LEN],
                filled: 0,
            },
            failed: false,
        }
    }

    /// A stream over `inner` with a fresh salt and padding, its key derived
    /// on a blocking thread. Nothing is written yet.
    pub(crate) async fn open(inner: BoxedStream, psk: Psk) -> io::Result<SnellStream> {
        let salt = new_salt()?;
        let padding = first_padding()?;
        let key = psk.key(salt).await?;
        Ok(SnellStream::new(inner, psk, salt, key, padding))
    }

    /// The server has ended its side of the current request.
    pub(crate) fn read_ended(&self) -> bool {
        matches!(self.reading, Reading::Ended)
    }

    /// Both sides of the current request have ended, cleanly, and our end
    /// has gone out: the next request may follow (`next_tunnel`).
    pub(crate) fn is_reusable(&self) -> bool {
        !self.failed && self.write_ended && self.out_pos == self.out.len() && self.read_ended()
    }

    /// Starts the next request on the same salts, keys and counters. Only
    /// when `is_reusable`.
    pub(crate) fn next_tunnel(&mut self) {
        debug_assert!(self.is_reusable(), "a request still in progress");
        self.write_ended = false;
        self.reading = Reading::Header {
            buf: [0; HEADER + TAG],
            filled: 0,
        };
    }

    /// Sends our empty record, which ends our side of the current request,
    /// and flushes; the connection stays open. Once per request.
    pub(crate) fn poll_end(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // a parked record first: it already consumed its nonces
        ready!(self.poll_out(cx))?;
        if !self.write_ended {
            self.out.clear();
            self.out_pos = 0;
            self.start_record();
            // an empty record is never padded
            seal_record(&mut self.up, &mut [], &[], &mut self.out);
            self.write_ended = true;
        }
        ready!(self.poll_out(cx))?;
        let flushed = ready!(Pin::new(&mut self.inner).poll_flush(cx));
        Poll::Ready(self.check(flushed))
    }

    /// The rest of the current record's payload, whole, or the next
    /// record's when nothing of it is left: UDP carries one datagram per
    /// record (`udp`). `None` once the server's side ended or the connection
    /// closed between records.
    pub(crate) fn poll_record(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<Option<Vec<u8>>>> {
        // no room: the reader stops at a payload without taking any of it
        let read = ready!(self.poll_read_records(cx, &mut ReadBuf::new(&mut [])));
        self.check(read)?;
        let reading = std::mem::replace(
            &mut self.reading,
            Reading::Header {
                buf: [0; HEADER + TAG],
                filled: 0,
            },
        );
        Poll::Ready(Ok(match reading {
            Reading::Payload { mut buf, pos, end } => {
                buf.truncate(end);
                buf.drain(..pos);
                Some(buf)
            }
            ended => {
                self.reading = ended;
                None
            }
        }))
    }

    /// The salt goes in front of the first record.
    fn start_record(&mut self) {
        if !self.salt_sent {
            self.out.extend_from_slice(&self.salt);
            self.salt_sent = true;
        }
    }

    fn check<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn poll_out(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.out_pos < self.out.len() {
            let written =
                ready!(Pin::new(&mut self.inner).poll_write(cx, &self.out[self.out_pos..]));
            match self.check(written) {
                Ok(0) => {
                    self.failed = true;
                    return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
                }
                Ok(n) => self.out_pos += n,
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
        Poll::Ready(Ok(()))
    }

    fn poll_read_records(
        &mut self,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            match &mut self.reading {
                Reading::Salt { buf, filled } => {
                    if !ready!(poll_fill(&mut self.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            NO_ANSWER,
                        )));
                    }
                    // our own salt back: a reflected stream, whose records
                    // would open under our own key
                    if *buf == self.salt {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    }
                    self.reading = Reading::Key(self.psk.spawn_key(*buf));
                }
                Reading::Key(task) => {
                    let key = ready!(Pin::new(task).poll(cx)).map_err(key_failed)?;
                    self.down = Some(CountingAead::new(AeadKind::Aes128Gcm, &key));
                    self.reading = Reading::Header {
                        buf: [0; HEADER + TAG],
                        filled: 0,
                    };
                }
                Reading::Header { buf, filled } => {
                    if !ready!(poll_fill(&mut self.inner, cx, buf, filled))? {
                        self.reading = Reading::Closed;
                        continue;
                    }
                    let down = self.down.as_mut().expect("the key came first");
                    if down.open(buf).is_none() {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    }
                    if buf[0] != VERSION {
                        return Poll::Ready(Err(invalid(UNKNOWN_VERSION)));
                    }
                    // the reserved bytes are not checked
                    let padding = usize::from(u16::from_be_bytes([buf[3], buf[4]]));
                    let len = usize::from(u16::from_be_bytes([buf[5], buf[6]]));
                    let sealed = if len == 0 { 0 } else { len + TAG };
                    self.reading = Reading::Body {
                        buf: vec![0; padding + sealed],
                        filled: 0,
                        padding,
                    };
                }
                Reading::Body {
                    buf,
                    filled,
                    padding,
                } => {
                    if !buf.is_empty() && !ready!(poll_fill(&mut self.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(cut_short()));
                    }
                    let padding = *padding;
                    if buf.len() == padding {
                        // an empty payload: the server's side has ended (a
                        // padding on it is tolerated)
                        self.reading = Reading::Ended;
                        continue;
                    }
                    let (pad, sealed) = buf.split_at_mut(padding);
                    mix(pad, sealed);
                    let down = self.down.as_mut().expect("the key came first");
                    let Some(n) = down.open(sealed) else {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    };
                    self.reading = Reading::Payload {
                        buf: std::mem::take(buf),
                        pos: padding,
                        end: padding + n,
                    };
                }
                Reading::Payload { buf, pos, end } => {
                    let n = out.remaining().min(*end - *pos);
                    out.put_slice(&buf[*pos..*pos + n]);
                    *pos += n;
                    if pos == end {
                        self.reading = Reading::Header {
                            buf: [0; HEADER + TAG],
                            filled: 0,
                        };
                    }
                    return Poll::Ready(Ok(()));
                }
                Reading::Ended | Reading::Closed => return Poll::Ready(Ok(())),
            }
        }
    }
}

impl AsyncRead for SnellStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = ready!(this.poll_read_records(cx, out));
        Poll::Ready(this.check(result))
    }
}

impl AsyncWrite for SnellStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        // (a parked record is then our empty one)
        if this.write_ended {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, ENDED)));
        }
        if this.out_pos == this.out.len() {
            this.out.clear();
            this.out_pos = 0;
            this.start_record();
            let n = data.len().min(MAX_PAYLOAD);
            let mut padding = this.padding.take().unwrap_or_default();
            seal_record(&mut this.up, &mut padding, &data[..n], &mut this.out);
            this.accepted = n;
        }
        ready!(this.poll_out(cx))?;
        Poll::Ready(Ok(this.accepted.min(data.len())))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_out(cx))?;
        let flushed = ready!(Pin::new(&mut this.inner).poll_flush(cx));
        Poll::Ready(this.check(flushed))
    }

    /// Our empty record, then the connection's own shutdown: the stream is
    /// not reused. A request on a reused connection ends with `poll_end`.
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_end(cx))?;
        let shut = ready!(Pin::new(&mut this.inner).poll_shutdown(cx));
        Poll::Ready(this.check(shut))
    }
}

#[cfg(test)]
mod tests {
    use super::super::kdf::derive_key;
    use super::*;
    use crate::vmess::vectors::hex;
    use std::future::poll_fn;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    /// Computed with Python `cryptography` (AES-GCM, Argon2id) from the
    /// byte-level notes alone: PSK "password", salt `00 01 … 0f`; "hello"
    /// padded with `a0 … a4`, then "world", then the empty record.
    const HELLO_WORLD: &str = "000102030405060708090a0b0c0d0e0f46904cd0db456dc6896911d75cdc876786ec38a822d80fe4a1f1a32ba015a219a48bbed0cfa9e98020abbedb61bb3dcd479ea6faaca185f1d36f60e6c6c7556418fb24871281897bfbb1b0829388a5abdda8dfae70038f19dca653d0e3b7c8c8e32fe5379ba3db0f4a59b307403b583420b32840";
    /// The same key: "hi" padded with `c0 … d3`, a padding longer than the
    /// payload's ciphertext (18 bytes), so only its first 18 bytes mix.
    const PADDED_HI: &str = "000102030405060708090a0b0c0d0e0f46904cd0ca456a55dac908474d47bd258c8e797e7e67ece4c1e9c324c576c735c94fcb24cd43cf42d1d2d3c019c212c483c69fc89dcab1cc04ce9dd03d";

    fn salt(first: u8) -> [u8; SALT_LEN] {
        std::array::from_fn(|i| first + i as u8)
    }

    /// A stream keyed with `psk` and `salt`, its key derived here.
    fn keyed(inner: BoxedStream, psk: &str, salt: [u8; SALT_LEN], padding: Vec<u8>) -> SnellStream {
        let key = derive_key(psk.as_bytes(), &salt);
        SnellStream::new(inner, Psk::new(psk), salt, key, padding)
    }

    fn boxed(stream: DuplexStream) -> BoxedStream {
        Box::new(stream)
    }

    /// What a stream keyed with `psk` reads from a peer that sends `wire`
    /// through a pipe of `capacity` bytes and then closes.
    async fn read_from(psk: &str, wire: Vec<u8>, capacity: usize) -> io::Result<Vec<u8>> {
        let (near, mut far) = tokio::io::duplex(capacity);
        tokio::spawn(async move {
            let _ = far.write_all(&wire).await;
            // dropping `far` is the close
        });
        let mut stream = keyed(boxed(near), psk, [0xee; SALT_LEN], Vec::new());
        let mut got = Vec::new();
        stream.read_to_end(&mut got).await.map(|_| got)
    }

    /// Two streams on the two ends of a pipe of `capacity` bytes, each
    /// with its own salt and a padding of 300 bytes.
    fn pair(capacity: usize) -> (SnellStream, SnellStream) {
        let (near, far) = tokio::io::duplex(capacity);
        (
            keyed(boxed(near), "psk", salt(0), vec![0x11; 300]),
            keyed(boxed(far), "psk", salt(0x40), vec![0x22; 300]),
        )
    }

    #[tokio::test]
    async fn the_first_record_is_padded_and_mixed_as_the_known_answer() {
        let (near, mut far) = tokio::io::duplex(64 * 1024);
        let mut stream = keyed(boxed(near), "password", salt(0), (0xa0..=0xa4).collect());
        // an empty write seals nothing: an empty record would end the direction
        assert_eq!(stream.write(b"").await.unwrap(), 0);
        stream.write_all(b"hello").await.unwrap();
        stream.write_all(b"world").await.unwrap();
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        assert_eq!(wire, hex(HELLO_WORLD));

        let (near, mut far) = tokio::io::duplex(64 * 1024);
        let mut stream = keyed(boxed(near), "password", salt(0), (0xc0..=0xd3).collect());
        stream.write_all(b"hi").await.unwrap();
        drop(stream);
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        assert_eq!(wire, hex(PADDED_HI));
    }

    #[tokio::test]
    async fn the_known_answers_read_back_whatever_the_slicing() {
        for capacity in [1, 7, 4096] {
            // the empty record is the end
            let got = read_from("password", hex(HELLO_WORLD), capacity)
                .await
                .unwrap();
            assert_eq!(got, b"helloworld", "through {capacity}");
            // a close between two records is the end too
            let got = read_from("password", hex(PADDED_HI), capacity)
                .await
                .unwrap();
            assert_eq!(got, b"hi", "through {capacity}");
        }
    }

    #[tokio::test]
    async fn two_streams_carry_a_large_payload_both_ways_whatever_the_slicing() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        for capacity in [1, 7, 4096] {
            let (mut client, mut server) = pair(capacity);
            let sent = data.clone();
            let (to_server, from_server) = tokio::join!(
                async {
                    client.write_all(&sent).await.unwrap();
                    client.shutdown().await.unwrap();
                    let mut got = Vec::new();
                    client.read_to_end(&mut got).await.unwrap();
                    got
                },
                async {
                    let mut got = Vec::new();
                    server.read_to_end(&mut got).await.unwrap();
                    server.write_all(&got).await.unwrap();
                    server.shutdown().await.unwrap();
                    got
                },
            );
            assert_eq!(to_server, data, "through {capacity}");
            assert_eq!(from_server, data, "back through {capacity}");
        }
    }

    #[tokio::test]
    async fn a_large_write_is_cut_into_records_of_at_most_0x3fff() {
        let (near, mut far) = tokio::io::duplex(1 << 20);
        let mut stream = keyed(boxed(near), "psk", salt(0), vec![0; 300]);
        let data = vec![0x55u8; MAX_PAYLOAD + 1];
        assert_eq!(
            stream.write(&data).await.unwrap(),
            MAX_PAYLOAD,
            "one record"
        );
        stream.write_all(&data[MAX_PAYLOAD..]).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        // only the first record is padded; the empty record is a header alone
        let header = HEADER + TAG;
        assert_eq!(
            wire.len(),
            SALT_LEN + (header + 300 + MAX_PAYLOAD + TAG) + (header + 1 + TAG) + header
        );
        let (near, mut far) = tokio::io::duplex(1 << 20);
        tokio::spawn(async move { far.write_all(&wire).await });
        let mut peer = keyed(boxed(near), "psk", salt(0x40), Vec::new());
        let mut got = Vec::new();
        peer.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, data);
    }

    #[tokio::test]
    async fn the_empty_record_ends_one_direction_and_the_other_goes_on() {
        let (mut client, mut server) = pair(4096);
        client.write_all(b"request").await.unwrap();
        poll_fn(|cx| client.poll_end(cx)).await.unwrap();
        let mut got = Vec::new();
        server.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"request");
        assert!(server.read_ended());
        // the end reads as end-of-file again, without touching the connection
        assert_eq!(server.read(&mut [0u8; 8]).await.unwrap(), 0);
        let err = client.write(b"more").await.unwrap_err();
        assert_eq!(
            (err.kind(), err.to_string().as_str()),
            (io::ErrorKind::BrokenPipe, ENDED)
        );
        // the other direction is still open
        server.write_all(b"answer").await.unwrap();
        let mut got = [0u8; 6];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"answer");
        assert!(!client.is_reusable() && !server.is_reusable());
    }

    #[tokio::test]
    async fn a_record_is_read_whole_and_the_end_is_none() {
        let (mut client, mut server) = pair(64 * 1024);
        client.write_all(b"one").await.unwrap();
        client.write_all(b"two, longer").await.unwrap();
        client.write_all(b"three").await.unwrap();
        poll_fn(|cx| client.poll_end(cx)).await.unwrap();
        for expected in [&b"one"[..], b"two, longer"] {
            let record = poll_fn(|cx| server.poll_record(cx)).await.unwrap();
            assert_eq!(record.unwrap(), expected);
        }
        // what a byte read left of a record comes whole
        let mut first = [0u8; 2];
        server.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"th");
        assert_eq!(
            poll_fn(|cx| server.poll_record(cx)).await.unwrap().unwrap(),
            b"ree"
        );
        assert_eq!(poll_fn(|cx| server.poll_record(cx)).await.unwrap(), None);
        assert!(server.read_ended());
    }

    /// Moves exactly `len` bytes from one wire to the other: the records
    /// between the two streams, counted.
    async fn pass(from: &mut DuplexStream, to: &mut DuplexStream, len: usize) {
        let mut buf = vec![0u8; len];
        from.read_exact(&mut buf).await.unwrap();
        to.write_all(&buf).await.unwrap();
    }

    /// One request each way: `ask` from the client, `answer` back, each
    /// side ending its direction. `first`: the salts and paddings go too.
    async fn exchange(
        client: &mut SnellStream,
        server: &mut SnellStream,
        wires: (&mut DuplexStream, &mut DuplexStream),
        (ask, answer): (&[u8], &[u8]),
        first: bool,
    ) {
        let header = HEADER + TAG;
        let opening = if first { SALT_LEN + 300 } else { 0 };
        client.write_all(ask).await.unwrap();
        poll_fn(|cx| client.poll_end(cx)).await.unwrap();
        pass(
            wires.0,
            wires.1,
            opening + header + ask.len() + TAG + header,
        )
        .await;
        let mut got = Vec::new();
        server.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, ask);
        server.write_all(answer).await.unwrap();
        poll_fn(|cx| server.poll_end(cx)).await.unwrap();
        pass(
            wires.1,
            wires.0,
            opening + header + answer.len() + TAG + header,
        )
        .await;
        let mut got = Vec::new();
        client.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, answer);
    }

    #[tokio::test]
    async fn two_requests_follow_each_other_on_one_stream() {
        // the two streams on separate pipes, the test carrying the records
        // across: it counts them
        let (near, mut client_wire) = tokio::io::duplex(64 * 1024);
        let (far, mut server_wire) = tokio::io::duplex(64 * 1024);
        let mut client = keyed(boxed(near), "psk", salt(0), vec![0x11; 300]);
        let mut server = keyed(boxed(far), "psk", salt(0x40), vec![0x22; 300]);
        exchange(
            &mut client,
            &mut server,
            (&mut client_wire, &mut server_wire),
            (b"one", b"1"),
            true,
        )
        .await;
        assert!(client.is_reusable() && server.is_reusable());
        client.next_tunnel();
        server.next_tunnel();
        assert!(!client.read_ended());
        // no salt and no padding the second time; the records open only
        // because both counters went on from where the first request left them
        exchange(
            &mut client,
            &mut server,
            (&mut client_wire, &mut server_wire),
            (b"two", b"22"),
            false,
        )
        .await;
        assert!(client.is_reusable() && server.is_reusable());
    }

    #[tokio::test]
    async fn what_the_server_gets_wrong_is_an_error_that_quotes_nothing() {
        let good = hex(HELLO_WORLD);
        // salt, the header, the padding and "hello"'s ciphertext
        let first_record = SALT_LEN + (HEADER + TAG) + 5 + (5 + TAG);
        let mut flipped = good.clone();
        flipped[first_record - 1] ^= 1;
        // a correctly sealed header of version 3
        let mut version_3 = salt(0).to_vec();
        let mut up = CountingAead::new(AeadKind::Aes128Gcm, &derive_key(b"password", &salt(0)));
        up.seal(&[3, 0, 0, 0, 0, 0, 1], &mut version_3);
        let cases: [(&str, &str, Vec<u8>, io::ErrorKind, &str); 7] = [
            (
                "silence",
                "password",
                Vec::new(),
                io::ErrorKind::UnexpectedEof,
                NO_ANSWER,
            ),
            (
                "half a salt",
                "password",
                good[..8].to_vec(),
                io::ErrorKind::UnexpectedEof,
                CUT_SHORT,
            ),
            (
                "a flipped bit",
                "password",
                flipped,
                io::ErrorKind::InvalidData,
                UNDECRYPTABLE,
            ),
            (
                "another psk",
                "other",
                good.clone(),
                io::ErrorKind::InvalidData,
                UNDECRYPTABLE,
            ),
            (
                "another version",
                "password",
                version_3,
                io::ErrorKind::InvalidData,
                UNKNOWN_VERSION,
            ),
            (
                "the middle of a header",
                "password",
                good[..SALT_LEN + 10].to_vec(),
                io::ErrorKind::UnexpectedEof,
                CUT_SHORT,
            ),
            (
                "the middle of a payload",
                "password",
                good[..first_record - 1].to_vec(),
                io::ErrorKind::UnexpectedEof,
                CUT_SHORT,
            ),
        ];
        for (case, psk, wire, kind, text) in cases {
            let err = read_from(psk, wire, 4096).await.unwrap_err();
            assert_eq!(
                (err.kind(), err.to_string().as_str()),
                (kind, text),
                "{case}"
            );
        }
    }

    #[tokio::test]
    async fn a_reflected_stream_does_not_decrypt() {
        let (near, mut far) = tokio::io::duplex(4096);
        // a "server" that sends the client's bytes back
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            while let Ok(n @ 1..) = far.read(&mut buf).await {
                if far.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        });
        let mut stream = keyed(boxed(near), "psk", salt(0), vec![0; 300]);
        stream.write_all(b"hello").await.unwrap();
        let err = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut [0u8; 5]))
            .await
            .expect("an answer in time")
            .unwrap_err();
        assert_eq!(
            (err.kind(), err.to_string().as_str()),
            (io::ErrorKind::InvalidData, UNDECRYPTABLE)
        );
        assert!(!stream.is_reusable());
    }

    /// With the only blocking thread busy, the server's key cannot be
    /// derived: the read waits instead of deriving on the runtime's thread,
    /// and goes on once the thread is free.
    #[test]
    fn the_servers_key_is_derived_on_a_blocking_thread() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let busy = tokio::task::spawn_blocking(move || wait.recv());
            let (mut stream, _server) = stream_from(hex(HELLO_WORLD)).await;
            let mut got = [0u8; 10];
            let pending =
                tokio::time::timeout(Duration::from_millis(200), stream.read_exact(&mut got)).await;
            assert!(pending.is_err(), "no key without a blocking thread");
            release.send(()).unwrap();
            busy.await.unwrap().unwrap();
            tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut got))
                .await
                .expect("the key in time")
                .unwrap();
            assert_eq!(&got, b"helloworld");
        });

        /// The stream, and the server's end, which stays open.
        async fn stream_from(wire: Vec<u8>) -> (SnellStream, DuplexStream) {
            let (near, mut far) = tokio::io::duplex(4096);
            far.write_all(&wire).await.unwrap();
            (
                keyed(boxed(near), "password", [0xee; SALT_LEN], Vec::new()),
                far,
            )
        }
    }
}
