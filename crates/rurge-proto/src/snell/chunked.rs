//! The record stream of Snell v1 to v3 (phase 2 M8 design 4.3): the chunks
//! of Shadowsocks AEAD under Snell's own keys. Each direction starts with
//! its own salt, then chunks:
//!
//! ```text
//! sealed(payload length, u16 BE)
//! sealed(payload)                 (absent when the length is zero)
//! ```
//!
//! both sealed with the direction's key and the next values of its counting
//! nonce (the length one, the payload the next). The key comes of the same
//! Argon2id as v4's (`kdf`): v1 keys ChaCha20-Poly1305 with the whole
//! output, v2 and v3 AES-128-GCM with its first half. Nothing is padded.
//!
//! A chunk of length zero is its sealed length alone and ends the direction
//! (a half-close); reads then return end-of-file. v2 ends each direction of
//! a request so and keeps the connection: once both have ended,
//! `next_tunnel` opens the next request on the same salts, keys and nonce
//! counters. v1 and v3 give every request a connection of its own and end
//! our side with the connection's shutdown, which sends no such chunk — a
//! v1 server knows none. The server's zero-length chunk ends its side on
//! every one of these versions.
//!
//! Every chunk we write carries at most `MAX_PAYLOAD` bytes; the top two
//! bits of a length are reserved, and a reader masks them off. A write
//! reports success only after its chunk has been handed to the layer below,
//! so the stream never depends on anyone calling `flush`.
//!
//! The server's key is derived when its salt arrives, inside a read: the
//! Argon2id runs on a blocking thread whose task the read polls.
//!
//! Two contracts on the caller, as for `V4Stream`: a write that returned
//! `Pending` must be retried with the same bytes, and an error is final.

use super::kdf::{OUTPUT_LEN, Psk, SALT_LEN, key_failed};
use super::record::{
    ENDED, MAX_PAYLOAD, UNDECRYPTABLE, cut_short, invalid, new_salt, poll_fill, poll_salt,
};
use crate::shadowsocks::cipher::{AeadKind, CountingAead, TAG};
use rurge_net::connector::BoxedStream;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::task::JoinHandle;

/// A chunk's sealed length.
const LEN: usize = 2 + TAG;

/// A direction's cipher: the first bytes of Argon2id's output, as many as
/// the key takes — all of them under ChaCha20-Poly1305, half under
/// AES-128-GCM.
fn cipher(kind: AeadKind, output: &[u8; OUTPUT_LEN]) -> CountingAead {
    CountingAead::new(kind, &output[..kind.key_len()])
}

/// Appends one chunk: `payload` (at most `MAX_PAYLOAD` bytes) behind its
/// length, or, when it is empty, the length alone, which ends the direction.
fn seal_chunk(up: &mut CountingAead, payload: &[u8], out: &mut Vec<u8>) {
    let len = u16::try_from(payload.len()).expect("a chunk's payload fits two bytes");
    up.seal(&len.to_be_bytes(), out);
    if !payload.is_empty() {
        up.seal(payload, out);
    }
}

enum Reading {
    Salt {
        buf: [u8; SALT_LEN],
        filled: usize,
    },
    /// The server's key, on a blocking thread.
    Key(JoinHandle<[u8; OUTPUT_LEN]>),
    Len {
        buf: [u8; LEN],
        filled: usize,
    },
    Body {
        buf: Vec<u8>,
        filled: usize,
    },
    Payload {
        buf: Vec<u8>,
        pos: usize,
        end: usize,
    },
    /// The server's zero-length chunk: its side of this request is over.
    Ended,
    /// The connection closed between two chunks.
    Closed,
}

impl Reading {
    fn next_chunk() -> Reading {
        Reading::Len {
            buf: [0; LEN],
            filled: 0,
        }
    }
}

/// No `Debug`: it holds the connection's keys.
pub(crate) struct ChunkedStream {
    inner: BoxedStream,
    psk: Psk,
    kind: AeadKind,
    /// Ours, sent in front of the first chunk; kept to tell a reflected
    /// stream apart.
    salt: [u8; SALT_LEN],
    salt_sent: bool,
    up: CountingAead,
    /// Known once the server's salt has arrived and its key is derived.
    down: Option<CountingAead>,
    /// The chunk being written, how much of it is out, and how many payload
    /// bytes it carries.
    out: Vec<u8>,
    out_pos: usize,
    accepted: usize,
    /// Our zero-length chunk of this request is sealed (maybe still in `out`).
    write_ended: bool,
    reading: Reading,
    /// An error was returned: the stream is not reusable.
    failed: bool,
}

impl ChunkedStream {
    /// `kind` is the version's cipher, ChaCha20-Poly1305 (v1) or
    /// AES-128-GCM (v2, v3); `up` is Argon2id's output for `salt` under
    /// `psk`.
    pub(crate) fn new(
        inner: BoxedStream,
        psk: Psk,
        kind: AeadKind,
        salt: [u8; SALT_LEN],
        up: &[u8; OUTPUT_LEN],
    ) -> ChunkedStream {
        ChunkedStream {
            inner,
            psk,
            kind,
            salt,
            salt_sent: false,
            up: cipher(kind, up),
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

    /// A stream over `inner` with a fresh salt, its key derived on a
    /// blocking thread. Nothing is written yet.
    pub(crate) async fn open(
        inner: BoxedStream,
        psk: Psk,
        kind: AeadKind,
    ) -> io::Result<ChunkedStream> {
        let salt = new_salt()?;
        let output = psk.output(salt).await?;
        Ok(ChunkedStream::new(inner, psk, kind, salt, &output))
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
        self.reading = Reading::next_chunk();
    }

    /// Sends our zero-length chunk, which ends our side of the current
    /// request, and flushes; the connection stays open. Once per request.
    pub(crate) fn poll_end(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // a parked chunk first: it already consumed its nonces
        ready!(self.poll_out(cx))?;
        if !self.write_ended {
            self.out.clear();
            self.out_pos = 0;
            self.start_chunk();
            seal_chunk(&mut self.up, &[], &mut self.out);
            self.write_ended = true;
        }
        ready!(self.poll_out(cx))?;
        let flushed = ready!(Pin::new(&mut self.inner).poll_flush(cx));
        Poll::Ready(self.check(flushed))
    }

    /// The rest of the current chunk's payload, whole, or the next chunk's
    /// when nothing of it is left: UDP carries one datagram per chunk
    /// (`udp`). `None` once the server's side ended or the connection closed
    /// between chunks.
    pub(crate) fn poll_record(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<Option<Vec<u8>>>> {
        // no room: the reader stops at a payload without taking any of it
        let read = ready!(self.poll_read_chunks(cx, &mut ReadBuf::new(&mut [])));
        self.check(read)?;
        let reading = std::mem::replace(&mut self.reading, Reading::next_chunk());
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

    /// The salt goes in front of the first chunk.
    fn start_chunk(&mut self) {
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

    fn poll_read_chunks(
        &mut self,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            match &mut self.reading {
                Reading::Salt { buf, filled } => {
                    ready!(poll_salt(&mut self.inner, cx, buf, filled))?;
                    // our own salt back: a reflected stream, whose chunks
                    // would open under our own key
                    if *buf == self.salt {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    }
                    self.reading = Reading::Key(self.psk.spawn_output(*buf));
                }
                Reading::Key(task) => {
                    let output = ready!(Pin::new(task).poll(cx)).map_err(key_failed)?;
                    self.down = Some(cipher(self.kind, &output));
                    self.reading = Reading::next_chunk();
                }
                Reading::Len { buf, filled } => {
                    if !ready!(poll_fill(&mut self.inner, cx, buf, filled))? {
                        self.reading = Reading::Closed;
                        continue;
                    }
                    // a server of another version or with another PSK fails
                    // here, on the first bytes it sends
                    let down = self.down.as_mut().expect("the key came first");
                    if down.open(buf).is_none() {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    }
                    // the top two bits are reserved
                    let len = usize::from(u16::from_be_bytes([buf[0], buf[1]])) & MAX_PAYLOAD;
                    self.reading = if len == 0 {
                        // no payload follows: the next bytes are the next
                        // request's
                        Reading::Ended
                    } else {
                        Reading::Body {
                            buf: vec![0; len + TAG],
                            filled: 0,
                        }
                    };
                }
                Reading::Body { buf, filled } => {
                    if !ready!(poll_fill(&mut self.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(cut_short()));
                    }
                    let down = self.down.as_mut().expect("the key came first");
                    let Some(end) = down.open(buf) else {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    };
                    self.reading = Reading::Payload {
                        buf: std::mem::take(buf),
                        pos: 0,
                        end,
                    };
                }
                Reading::Payload { buf, pos, end } => {
                    let n = out.remaining().min(*end - *pos);
                    out.put_slice(&buf[*pos..*pos + n]);
                    *pos += n;
                    if pos == end {
                        self.reading = Reading::next_chunk();
                    }
                    return Poll::Ready(Ok(()));
                }
                Reading::Ended | Reading::Closed => return Poll::Ready(Ok(())),
            }
        }
    }
}

impl AsyncRead for ChunkedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = ready!(this.poll_read_chunks(cx, out));
        Poll::Ready(this.check(result))
    }
}

impl AsyncWrite for ChunkedStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        // (a parked chunk is then our zero-length one)
        if this.write_ended {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, ENDED)));
        }
        if this.out_pos == this.out.len() {
            this.out.clear();
            this.out_pos = 0;
            this.start_chunk();
            let n = data.len().min(MAX_PAYLOAD);
            seal_chunk(&mut this.up, &data[..n], &mut this.out);
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

    /// The connection's own shutdown, and no zero-length chunk before it: a
    /// v1 server knows none, and reads the close as our end. The stream is
    /// not reused; a request on a reused connection ends with `poll_end`.
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        // a parked chunk first: it already consumed its nonces
        ready!(this.poll_out(cx))?;
        let shut = ready!(Pin::new(&mut this.inner).poll_shutdown(cx));
        Poll::Ready(this.check(shut))
    }
}

#[cfg(test)]
mod tests {
    use super::super::kdf::derive;
    use super::super::record::{CUT_SHORT, NO_ANSWER};
    use super::*;
    use crate::shadowsocks::cipher::AeadCipher;
    use crate::vmess::vectors::hex;
    use std::future::{Future, poll_fn};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    /// Computed with Python `cryptography` (Argon2id, AES-GCM,
    /// ChaCha20-Poly1305) from the format's description alone (scratch
    /// script of the M8b plan): PSK "secret", salt `00 01 … 0f`; the chunk
    /// "hello", then the zero-length chunk.
    const HELLO_AES: &str = "000102030405060708090a0b0c0d0e0f32fde5fc6bae1a61731c5693e0666e067510a41834effbf14b5304b44a5b2eb0aa20b133970807129ea3ad3658ea51dc181bce5f3eede88f92";
    const HELLO_CHACHA: &str = "000102030405060708090a0b0c0d0e0f0f9ca519caa5fddc736dec616e3ee903efc040fc6a4b32c20ce030462bcbbddfa9b8cd5e9f1c27e957a2d0d107cd0bb54e6d38351d2f742096";
    /// The same script: what a v4 server sends first under the same PSK and
    /// salt, the sealed header of a record that carries 5 bytes.
    const V4_HEADER: &str = "36f86576b3ff6246376fb4f2ea5e0adde7c78a27d852b6";

    /// The cipher, how much of Argon2id's output keys it, and its known
    /// answer: v2 and v3, then v1.
    const VERSIONS: [(AeadKind, usize, &str); 2] = [
        (AeadKind::Aes128Gcm, 16, HELLO_AES),
        (AeadKind::ChaCha20Poly1305, 32, HELLO_CHACHA),
    ];

    fn salt(first: u8) -> [u8; SALT_LEN] {
        std::array::from_fn(|i| first + i as u8)
    }

    /// A stream keyed with `psk` and `salt`, its key derived here.
    fn keyed(inner: BoxedStream, kind: AeadKind, psk: &str, salt: [u8; SALT_LEN]) -> ChunkedStream {
        let output = derive(psk.as_bytes(), &salt);
        ChunkedStream::new(inner, Psk::new(psk), kind, salt, &output)
    }

    fn boxed(stream: DuplexStream) -> BoxedStream {
        Box::new(stream)
    }

    /// `plain` sealed under `key` with the nonce that counts `count`: the
    /// cipher alone, the stream's own counting aside.
    fn sealed(kind: AeadKind, key: &[u8], count: u8, plain: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; 12];
        nonce[0] = count;
        let mut out = plain.to_vec();
        let tag = AeadCipher::new(kind, key).seal_in_place(&nonce, &mut out);
        out.extend_from_slice(&tag);
        out
    }

    /// A wait for the peer, which ends within the bound: a stream that gets
    /// a chunk wrong leaves the other side waiting for bytes that never come.
    async fn bounded<T>(wait: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(30), wait)
            .await
            .expect("the peer within the bound")
    }

    /// Everything `stream` reads up to its end.
    async fn read_all(stream: &mut ChunkedStream) -> io::Result<Vec<u8>> {
        let mut got = Vec::new();
        bounded(stream.read_to_end(&mut got)).await.map(|_| got)
    }

    /// `stream`'s next chunk, whole.
    async fn chunk(stream: &mut ChunkedStream) -> Option<Vec<u8>> {
        bounded(poll_fn(|cx| stream.poll_record(cx))).await.unwrap()
    }

    /// What a stream keyed with `psk` reads from a peer that sends `wire`
    /// through a pipe of `capacity` bytes and then closes.
    async fn read_from(
        kind: AeadKind,
        psk: &str,
        wire: Vec<u8>,
        capacity: usize,
    ) -> io::Result<Vec<u8>> {
        let (near, mut far) = tokio::io::duplex(capacity);
        tokio::spawn(async move {
            let _ = far.write_all(&wire).await;
            // dropping `far` is the close
        });
        let mut stream = keyed(boxed(near), kind, psk, [0xee; SALT_LEN]);
        read_all(&mut stream).await
    }

    /// Two streams on the two ends of a pipe of `capacity` bytes, each
    /// with its own salt.
    fn pair(kind: AeadKind, capacity: usize) -> (ChunkedStream, ChunkedStream) {
        let (near, far) = tokio::io::duplex(capacity);
        (
            keyed(boxed(near), kind, "psk", salt(0)),
            keyed(boxed(far), kind, "psk", salt(0x40)),
        )
    }

    #[tokio::test]
    async fn the_first_chunk_and_the_end_are_the_known_answer() {
        let output = derive(b"secret", &salt(0));
        for (kind, key_len, vector) in VERSIONS {
            // step by step, the stream aside: the salt, the length sealed
            // under the first nonce, the payload under the second, and the
            // zero-length chunk — its length alone — under the third
            let key = &output[..key_len];
            let mut expected = salt(0).to_vec();
            expected.extend(sealed(kind, key, 0, &[0, 5]));
            expected.extend(sealed(kind, key, 1, b"hello"));
            expected.extend(sealed(kind, key, 2, &[0, 0]));
            assert_eq!(expected, hex(vector), "{kind:?}: Python's answer");

            let (near, mut far) = tokio::io::duplex(64 * 1024);
            let mut stream = keyed(boxed(near), kind, "secret", salt(0));
            // an empty write seals nothing: a chunk of length zero would end
            // the direction
            assert_eq!(stream.write(b"").await.unwrap(), 0);
            stream.write_all(b"hello").await.unwrap();
            poll_fn(|cx| stream.poll_end(cx)).await.unwrap();
            // ending again sends nothing more
            poll_fn(|cx| stream.poll_end(cx)).await.unwrap();
            drop(stream);
            let mut wire = Vec::new();
            far.read_to_end(&mut wire).await.unwrap();
            assert_eq!(wire, expected, "{kind:?}");
        }
    }

    /// A shutdown is the connection's own and sends no zero-length chunk:
    /// v1 and v3 end a request so, and a v1 server knows no such chunk.
    #[tokio::test]
    async fn a_shutdown_closes_the_connection_without_a_zero_length_chunk() {
        for (kind, _, vector) in VERSIONS {
            let (near, mut far) = tokio::io::duplex(64 * 1024);
            let mut stream = keyed(boxed(near), kind, "secret", salt(0));
            stream.write_all(b"hello").await.unwrap();
            stream.shutdown().await.unwrap();
            // the stream is still there: the end the peer reads is the shutdown's
            let mut wire = Vec::new();
            tokio::time::timeout(Duration::from_secs(10), far.read_to_end(&mut wire))
                .await
                .expect("the shutdown reaches the peer")
                .unwrap();
            let known = hex(vector);
            assert_eq!(wire, known[..known.len() - LEN], "{kind:?}");
            assert!(!stream.is_reusable());
        }
    }

    #[tokio::test]
    async fn the_known_answers_read_back_whatever_the_slicing() {
        for (kind, _, vector) in VERSIONS {
            let known = hex(vector);
            for capacity in [1, 7, 4096] {
                // the zero-length chunk is the end
                let got = read_from(kind, "secret", known.clone(), capacity)
                    .await
                    .unwrap();
                assert_eq!(got, b"hello", "{kind:?} through {capacity}");
                // a close between two chunks is the end too
                let closed = known[..known.len() - LEN].to_vec();
                let got = read_from(kind, "secret", closed, capacity).await.unwrap();
                assert_eq!(got, b"hello", "{kind:?} through {capacity}");
            }
        }
    }

    /// `pieces` there, each piece a write of its own, and all of it back in
    /// one: the client ends with its zero-length chunk, the server with the
    /// connection's shutdown, and both are an end to the reader.
    async fn there_and_back(kind: AeadKind, capacity: usize, pieces: &[&[u8]]) {
        let (mut client, mut server) = pair(kind, capacity);
        let data = pieces.concat();
        let both = async {
            tokio::join!(
                async {
                    for piece in pieces {
                        client.write_all(piece).await.unwrap();
                    }
                    poll_fn(|cx| client.poll_end(cx)).await.unwrap();
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
            )
        };
        let (to_server, from_server) = bounded(both).await;
        assert!(to_server == data, "{kind:?} through {capacity}");
        assert!(from_server == data, "{kind:?} back through {capacity}");
    }

    #[tokio::test]
    async fn two_streams_carry_300_kib_both_ways_whatever_the_slicing() {
        let data: Vec<u8> = (0..300 * 1024u32).map(|i| (i % 251) as u8).collect();
        for (kind, ..) in VERSIONS {
            // a pipe of a prime size cuts every length and payload elsewhere
            for capacity in [61, 4096] {
                there_and_back(kind, capacity, &[&data]).await;
            }
        }
    }

    /// Many short chunks through a pipe that passes one byte at a time (and
    /// seven): every length and every tag arrives in pieces.
    #[tokio::test]
    async fn short_chunks_cross_byte_by_byte() {
        let data: Vec<u8> = (0..1200u32).map(|i| (i % 251) as u8).collect();
        let mut pieces = Vec::new();
        let mut rest = &data[..];
        for len in (1..=40).cycle() {
            if rest.is_empty() {
                break;
            }
            let (piece, later) = rest.split_at(len.min(rest.len()));
            pieces.push(piece);
            rest = later;
        }
        for (kind, ..) in VERSIONS {
            for capacity in [1, 7] {
                there_and_back(kind, capacity, &pieces).await;
            }
        }
    }

    #[tokio::test]
    async fn a_large_write_is_cut_into_chunks_of_at_most_0x3fff() {
        for (kind, ..) in VERSIONS {
            let (near, mut far) = tokio::io::duplex(1 << 20);
            let mut stream = keyed(boxed(near), kind, "psk", salt(0));
            let data = vec![0x55u8; MAX_PAYLOAD + 1];
            assert_eq!(stream.write(&data).await.unwrap(), MAX_PAYLOAD, "one chunk");
            stream.write_all(&data[MAX_PAYLOAD..]).await.unwrap();
            stream.shutdown().await.unwrap();
            let mut wire = Vec::new();
            bounded(far.read_to_end(&mut wire)).await.unwrap();
            assert_eq!(
                wire.len(),
                SALT_LEN + (LEN + MAX_PAYLOAD + TAG) + (LEN + 1 + TAG),
                "{kind:?}"
            );
            let (near, mut far) = tokio::io::duplex(1 << 20);
            tokio::spawn(async move { far.write_all(&wire).await });
            let mut peer = keyed(boxed(near), kind, "psk", salt(0x40));
            let got = read_all(&mut peer).await.unwrap();
            assert!(got == data, "{kind:?}");
        }
    }

    #[tokio::test]
    async fn the_zero_length_chunk_ends_one_direction_and_the_other_goes_on() {
        for (kind, ..) in VERSIONS {
            let (mut client, mut server) = pair(kind, 4096);
            client.write_all(b"request").await.unwrap();
            poll_fn(|cx| client.poll_end(cx)).await.unwrap();
            assert_eq!(read_all(&mut server).await.unwrap(), b"request");
            assert!(server.read_ended());
            // the end reads as end-of-file again, without touching the connection
            assert_eq!(read_all(&mut server).await.unwrap(), b"");
            let err = client.write(b"more").await.unwrap_err();
            assert_eq!(
                (err.kind(), err.to_string().as_str()),
                (io::ErrorKind::BrokenPipe, ENDED)
            );
            // the other direction is still open
            server.write_all(b"answer").await.unwrap();
            let mut got = [0u8; 6];
            bounded(client.read_exact(&mut got)).await.unwrap();
            assert_eq!(&got, b"answer");
            assert!(!client.is_reusable() && !server.is_reusable());
        }
    }

    /// The length's top two bits are reserved: a reader takes the low 14.
    #[tokio::test]
    async fn the_reserved_bits_of_a_length_are_masked_off() {
        let output = derive(b"secret", &salt(0));
        for (kind, key_len, _) in VERSIONS {
            let key = &output[..key_len];
            let mut wire = salt(0).to_vec();
            wire.extend(sealed(kind, key, 0, &[0xc0, 5]));
            wire.extend(sealed(kind, key, 1, b"hello"));
            // only the reserved bits: a length of zero, the end
            wire.extend(sealed(kind, key, 2, &[0x80, 0]));
            wire.extend_from_slice(b"what follows the end is not read");
            let (near, mut far) = tokio::io::duplex(4096);
            far.write_all(&wire).await.unwrap();
            let mut stream = keyed(boxed(near), kind, "secret", [0xee; SALT_LEN]);
            assert_eq!(read_all(&mut stream).await.unwrap(), b"hello", "{kind:?}");
            assert!(stream.read_ended());
        }
    }

    #[tokio::test]
    async fn a_chunk_is_read_whole_and_the_end_is_none() {
        for (kind, ..) in VERSIONS {
            let (mut client, mut server) = pair(kind, 64 * 1024);
            client.write_all(b"one").await.unwrap();
            client.write_all(b"two, longer").await.unwrap();
            client.write_all(b"three").await.unwrap();
            poll_fn(|cx| client.poll_end(cx)).await.unwrap();
            for expected in [&b"one"[..], b"two, longer"] {
                assert_eq!(chunk(&mut server).await.unwrap(), expected);
            }
            // what a byte read left of a chunk comes whole
            let mut first = [0u8; 2];
            bounded(server.read_exact(&mut first)).await.unwrap();
            assert_eq!(&first, b"th");
            assert_eq!(chunk(&mut server).await.unwrap(), b"ree");
            assert_eq!(chunk(&mut server).await, None);
            assert!(server.read_ended());
            // the close between two chunks is `None` as well
            let (mut client, mut server) = pair(kind, 64 * 1024);
            client.write_all(b"last").await.unwrap();
            drop(client);
            assert_eq!(chunk(&mut server).await.unwrap(), b"last");
            assert_eq!(chunk(&mut server).await, None);
            assert!(!server.read_ended());
        }
    }

    /// Moves exactly `len` bytes from one wire to the other: the chunks
    /// between the two streams, counted.
    async fn pass(from: &mut DuplexStream, to: &mut DuplexStream, len: usize) {
        let mut buf = vec![0u8; len];
        bounded(from.read_exact(&mut buf)).await.unwrap();
        to.write_all(&buf).await.unwrap();
    }

    /// One request each way: `ask` from the client, `answer` back, each
    /// side ending its direction. `first`: the salts go too.
    async fn exchange(
        client: &mut ChunkedStream,
        server: &mut ChunkedStream,
        wires: (&mut DuplexStream, &mut DuplexStream),
        (ask, answer): (&[u8], &[u8]),
        first: bool,
    ) {
        let opening = if first { SALT_LEN } else { 0 };
        client.write_all(ask).await.unwrap();
        poll_fn(|cx| client.poll_end(cx)).await.unwrap();
        // a chunk, then the zero-length one: a length alone
        pass(wires.0, wires.1, opening + LEN + ask.len() + TAG + LEN).await;
        assert_eq!(read_all(server).await.unwrap(), ask);
        server.write_all(answer).await.unwrap();
        poll_fn(|cx| server.poll_end(cx)).await.unwrap();
        pass(wires.1, wires.0, opening + LEN + answer.len() + TAG + LEN).await;
        assert_eq!(read_all(client).await.unwrap(), answer);
    }

    #[tokio::test]
    async fn two_requests_follow_each_other_on_one_stream() {
        for (kind, ..) in VERSIONS {
            // the two streams on separate pipes, the test carrying the
            // chunks across: it counts them
            let (near, mut client_wire) = tokio::io::duplex(64 * 1024);
            let (far, mut server_wire) = tokio::io::duplex(64 * 1024);
            let mut client = keyed(boxed(near), kind, "psk", salt(0));
            let mut server = keyed(boxed(far), kind, "psk", salt(0x40));
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
            assert!(!client.read_ended() && !client.is_reusable());
            // no salt the second time; the chunks open only because both
            // counters went on from where the first request left them
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
    }

    #[tokio::test]
    async fn what_the_server_gets_wrong_is_an_error_that_quotes_nothing() {
        for (kind, _, vector) in VERSIONS {
            let good = hex(vector);
            // salt, the sealed length and "hello"'s ciphertext
            let first_chunk = SALT_LEN + LEN + (5 + TAG);
            let mut flipped_length = good.clone();
            flipped_length[SALT_LEN] ^= 1;
            let mut flipped_payload = good.clone();
            flipped_payload[first_chunk - 1] ^= 1;
            let cases: [(&str, &str, Vec<u8>, io::ErrorKind, &str); 8] = [
                (
                    "silence",
                    "secret",
                    Vec::new(),
                    io::ErrorKind::UnexpectedEof,
                    NO_ANSWER,
                ),
                (
                    "half a salt",
                    "secret",
                    good[..8].to_vec(),
                    io::ErrorKind::UnexpectedEof,
                    CUT_SHORT,
                ),
                (
                    "a flipped bit in a length",
                    "secret",
                    flipped_length,
                    io::ErrorKind::InvalidData,
                    UNDECRYPTABLE,
                ),
                (
                    "a flipped bit in a payload",
                    "secret",
                    flipped_payload,
                    io::ErrorKind::InvalidData,
                    UNDECRYPTABLE,
                ),
                (
                    "another psk",
                    "0ther-psk",
                    good.clone(),
                    io::ErrorKind::InvalidData,
                    UNDECRYPTABLE,
                ),
                (
                    "the middle of a length",
                    "secret",
                    good[..SALT_LEN + 10].to_vec(),
                    io::ErrorKind::UnexpectedEof,
                    CUT_SHORT,
                ),
                (
                    "between a length and its payload",
                    "secret",
                    good[..SALT_LEN + LEN].to_vec(),
                    io::ErrorKind::UnexpectedEof,
                    CUT_SHORT,
                ),
                (
                    "the middle of a payload",
                    "secret",
                    good[..first_chunk - 1].to_vec(),
                    io::ErrorKind::UnexpectedEof,
                    CUT_SHORT,
                ),
            ];
            for (case, psk, wire, error_kind, text) in cases {
                let err = read_from(kind, psk, wire, 4096).await.unwrap_err();
                assert_eq!(
                    (err.kind(), err.to_string().as_str()),
                    (error_kind, text),
                    "{kind:?}: {case}"
                );
                assert!(!format!("{err} {err:?}").contains(psk), "{kind:?}: {case}");
            }
        }
    }

    /// A v4 server answers with a sealed header of 7 bytes: its first 18
    /// bytes are no sealed length — under v2 and v3 even the key is the
    /// same. The read fails on them at once: the server sends nothing more
    /// and keeps the connection.
    #[tokio::test]
    async fn a_v4_servers_first_record_fails_to_decrypt_without_waiting_for_more() {
        for (kind, ..) in VERSIONS {
            let (near, mut far) = tokio::io::duplex(4096);
            let wire = [&salt(0)[..], &hex(V4_HEADER)].concat();
            far.write_all(&wire).await.unwrap();
            let mut stream = keyed(boxed(near), kind, "secret", [0xee; SALT_LEN]);
            let err = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut [0u8; 8]))
                .await
                .expect("an error while the connection is still open")
                .unwrap_err();
            assert_eq!(
                (err.kind(), err.to_string().as_str()),
                (io::ErrorKind::InvalidData, UNDECRYPTABLE),
                "{kind:?}"
            );
            assert!(!stream.is_reusable());
            drop(far);
        }
    }

    #[tokio::test]
    async fn a_reflected_stream_does_not_decrypt() {
        for (kind, ..) in VERSIONS {
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
            let mut stream = keyed(boxed(near), kind, "psk", salt(0));
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
            for (kind, _, vector) in VERSIONS {
                let (release, wait) = std::sync::mpsc::channel::<()>();
                let busy = tokio::task::spawn_blocking(move || wait.recv());
                let (near, mut far) = tokio::io::duplex(4096);
                far.write_all(&hex(vector)).await.unwrap();
                let mut stream = keyed(boxed(near), kind, "secret", [0xee; SALT_LEN]);
                let mut got = [0u8; 5];
                let pending =
                    tokio::time::timeout(Duration::from_millis(200), stream.read_exact(&mut got))
                        .await;
                assert!(pending.is_err(), "no key without a blocking thread");
                release.send(()).unwrap();
                busy.await.unwrap().unwrap();
                tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut got))
                    .await
                    .expect("the key in time")
                    .unwrap();
                assert_eq!(&got, b"hello", "{kind:?}");
            }
        });
    }
}
