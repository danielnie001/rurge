//! The TCP stream of the AEAD methods (the Shadowsocks AEAD specification):
//! each direction starts with its own random salt, then chunks of
//! `sealed(length, 2 bytes big-endian) ‖ sealed(payload)`, both sealed with
//! the direction's session key and the next value of its counting nonce.
//!
//! A write reports success only after its whole chunk has been handed to the
//! layer below, so the stream never depends on anyone calling `flush`. The
//! salt leaves in front of the first chunk, in the same write. There is no
//! end-of-stream chunk: a shutdown passes straight through.
//!
//! Two contracts on the caller, as for `VmessStream`: a write that returned
//! `Pending` must be retried with the same bytes (the parked chunk was
//! sealed from them and is what goes out), and a read error is final (the
//! nonce has moved on).

use super::cipher::{CountingAead, MasterKey, TAG};
use rurge_net::connector::BoxedStream;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The largest payload of a chunk: the length's top two bits are reserved.
pub(crate) const MAX_PAYLOAD: usize = 0x3FFF;

/// The server says nothing before the first payload, so a wrong password (or
/// method) and a server that refuses for any other reason look the same:
/// the connection closes (phase 2 M6 design 3.3).
const NO_ANSWER: &str = "ss: the server closed the connection without answering";
const UNDECRYPTABLE: &str = "ss: the server's data failed to decrypt (wrong password or method?)";
const CUT_SHORT: &str = "ss: the connection ended in the middle of a chunk";
const TOO_LONG: &str = "ss: the server sent a chunk longer than the protocol allows";

/// Appends one chunk carrying `payload` (at most the stream's largest) to `out`.
pub(crate) fn seal_chunk(aead: &mut CountingAead, payload: &[u8], out: &mut Vec<u8>) {
    let len = u16::try_from(payload.len()).expect("a chunk's length fits two bytes");
    aead.seal(&len.to_be_bytes(), out);
    aead.seal(payload, out);
}

enum Reading {
    Salt {
        buf: Vec<u8>,
        filled: usize,
    },
    Len {
        buf: [u8; 2 + TAG],
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
    Eof,
}

/// No `Debug`: it holds the connection's keys.
pub(crate) struct AeadStream {
    inner: BoxedStream,
    key: Arc<MasterKey>,
    max_payload: usize,
    /// Our salt until it is sealed into `out` in front of the first chunk.
    salt: Option<Vec<u8>>,
    up: CountingAead,
    /// Known once the server's salt has arrived.
    down: Option<CountingAead>,
    /// The chunk being written, how much of it is out, and how many payload
    /// bytes it carries.
    out: Vec<u8>,
    out_pos: usize,
    accepted: usize,
    reading: Reading,
}

fn invalid(text: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

fn cut_short() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, CUT_SHORT)
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

impl AeadStream {
    /// `salt` is fresh randomness of `key.salt_len()` bytes; `max_payload`
    /// bounds the chunks both ways.
    pub(crate) fn new(
        inner: BoxedStream,
        key: Arc<MasterKey>,
        salt: Vec<u8>,
        max_payload: usize,
    ) -> AeadStream {
        let salt_len = key.salt_len();
        AeadStream {
            up: key.session(&salt),
            down: None,
            inner,
            key,
            max_payload,
            salt: Some(salt),
            out: Vec::new(),
            out_pos: 0,
            accepted: 0,
            reading: Reading::Salt {
                buf: vec![0; salt_len],
                filled: 0,
            },
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

impl AsyncRead for AeadStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            match &mut this.reading {
                Reading::Salt { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            NO_ANSWER,
                        )));
                    }
                    this.down = Some(this.key.session(buf));
                    this.reading = Reading::Len {
                        buf: [0; 2 + TAG],
                        filled: 0,
                    };
                }
                Reading::Len { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        this.reading = Reading::Eof;
                        continue;
                    }
                    let down = this.down.as_mut().expect("the salt came first");
                    if down.open(buf).is_none() {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    }
                    let len = usize::from(u16::from_be_bytes([buf[0], buf[1]]));
                    if len > this.max_payload {
                        return Poll::Ready(Err(invalid(TOO_LONG)));
                    }
                    this.reading = Reading::Body {
                        buf: vec![0; len + TAG],
                        filled: 0,
                    };
                }
                Reading::Body { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(cut_short()));
                    }
                    let down = this.down.as_mut().expect("the salt came first");
                    let Some(end) = down.open(buf) else {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    };
                    this.reading = if end == 0 {
                        // nothing to hand out: on to the next chunk
                        Reading::Len {
                            buf: [0; 2 + TAG],
                            filled: 0,
                        }
                    } else {
                        Reading::Payload {
                            buf: std::mem::take(buf),
                            pos: 0,
                            end,
                        }
                    };
                }
                Reading::Payload { buf, pos, end } => {
                    let n = out.remaining().min(*end - *pos);
                    out.put_slice(&buf[*pos..*pos + n]);
                    *pos += n;
                    if pos == end {
                        this.reading = Reading::Len {
                            buf: [0; 2 + TAG],
                            filled: 0,
                        };
                    }
                    return Poll::Ready(Ok(()));
                }
                Reading::Eof => return Poll::Ready(Ok(())),
            }
        }
    }
}

impl AsyncWrite for AeadStream {
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
            let n = data.len().min(this.max_payload);
            this.out.clear();
            this.out_pos = 0;
            if let Some(salt) = this.salt.take() {
                this.out.extend_from_slice(&salt);
            }
            seal_chunk(&mut this.up, &data[..n], &mut this.out);
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
        // a parked chunk first: it already consumed its nonce
        ready!(this.poll_out(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadowsocks::cipher::AeadKind;
    use crate::vmess::vectors::hex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// `seal("hello")` then `seal(00 01 … 27)` under the password
    /// "password" and the salt `00 01 …`, computed with Python's
    /// `cryptography` (XChaCha20 through its HChaCha20 subkey) from the
    /// specification alone.
    const VECTORS: [(AeadKind, &str); 5] = [
        (
            AeadKind::Aes128Gcm,
            "5c2b27a26ad0cdf9cd7aa4f3c851b134b4b9947477b58a2f87d1affe84b78de924b5f222d5ec8cf7fff1aae7df1600106a3ca3d13ffa98f19b96d22705e6337c6f27ef7d7df12956ead1fdd856502e2aad6d8196189c21536caabf29450eb17c0122df377b5e82b6b195b78de510e5cde8",
        ),
        (
            AeadKind::Aes192Gcm,
            "863e97590d6f98066b78f274bdb5a4ad7f95f7102226764c34b2fcd7b95c2feba247473e8fd1a84e815c1195a2ece1e98b8324d252f1c1d681c8e74e4afa0c38e2bc45f99ed8ee1206199a625f2a0c5fb1f4fedeaa4c04f59adfb70f7e37f85645a7274d715413e5268a0f0e93112c5e78",
        ),
        (
            AeadKind::Aes256Gcm,
            "7ea089e1d8874f484867a34f5b648078a7379d45b3194573671c53431294750d0362127bcf86798eb8d4e434962dd61b1be92d84791ec1b1d31ff67ef4c1204f35e6ab270005b9a672651fffb9a410f2edd96ac4114ada92f823d528bf789d9c44f7f8f163e61c9d3e6938214dbc4fa77d",
        ),
        (
            AeadKind::ChaCha20Poly1305,
            "ad4d5c2599d42f6d9b26804b82a3b96dc584e8adc7498c0ff41f578989fe0c5ded753038d91134efbb23156ec73f2d258da9e30323b8caed59a798588d30c7d900092ff76b36e9d27f78881a2357bd6a2cfce948523b580a3fbac4e189be71a50e0cf17b4591cff3d261d862aa69be155b",
        ),
        (
            AeadKind::XChaCha20Poly1305,
            "7808afe0b13ac1c1e48139cc556091669eaa8a9e1627100727e6b7ad25261f8856a6c77c3e51635380cc7098a3c7212080a0a2d4009237d9780a17ff173a194e3df77e49e9dbb8eab3e5313c586119c3642563047150cfe12b6988b8c15c4bf2f979e32d185e7c674a407a1f1c0055c409",
        ),
    ];

    fn salt(kind: AeadKind) -> Vec<u8> {
        (0..kind.key_len() as u8).collect()
    }

    fn key(kind: AeadKind, password: &str) -> Arc<MasterKey> {
        Arc::new(MasterKey::from_password(kind, password))
    }

    fn long() -> Vec<u8> {
        (0u8..40).collect()
    }

    /// What a stream keyed with `password` reads from a peer that sends
    /// `wire` through a pipe of `capacity` bytes and then closes.
    async fn read_from(
        kind: AeadKind,
        password: &str,
        wire: Vec<u8>,
        capacity: usize,
    ) -> io::Result<Vec<u8>> {
        let (near, mut far) = tokio::io::duplex(capacity);
        tokio::spawn(async move {
            let _ = far.write_all(&wire).await;
            // dropping `far` is the close
        });
        let mut stream =
            AeadStream::new(Box::new(near), key(kind, password), salt(kind), MAX_PAYLOAD);
        let mut got = Vec::new();
        stream.read_to_end(&mut got).await.map(|_| got)
    }

    #[tokio::test]
    async fn every_cipher_writes_the_known_answer() {
        for (kind, vector) in VECTORS {
            let (near, mut far) = tokio::io::duplex(64 * 1024);
            let mut stream = AeadStream::new(
                Box::new(near),
                key(kind, "password"),
                salt(kind),
                MAX_PAYLOAD,
            );
            // an empty write seals nothing: an empty chunk is not a payload
            assert_eq!(stream.write(b"").await.unwrap(), 0);
            stream.write_all(b"hello").await.unwrap();
            stream.write_all(&long()).await.unwrap();
            stream.shutdown().await.unwrap();
            let mut wire = Vec::new();
            far.read_to_end(&mut wire).await.unwrap();
            let mut expected = salt(kind);
            expected.extend_from_slice(&hex(vector));
            assert_eq!(wire, expected, "{kind:?}");
        }
    }

    #[tokio::test]
    async fn the_server_stream_reads_back_whatever_the_slicing() {
        for (kind, vector) in VECTORS {
            // the answer has the request's form: its own salt, then chunks
            let mut wire = salt(kind);
            wire.extend_from_slice(&hex(vector));
            for capacity in [1, 7, 4096] {
                let got = read_from(kind, "password", wire.clone(), capacity)
                    .await
                    .unwrap();
                let mut expected = b"hello".to_vec();
                expected.extend_from_slice(&long());
                assert_eq!(got, expected, "{kind:?} through {capacity}");
            }
        }
    }

    #[tokio::test]
    async fn a_large_write_is_cut_into_chunks_of_at_most_0x3fff() {
        let kind = AeadKind::Aes128Gcm;
        let (near, mut far) = tokio::io::duplex(1 << 20);
        let mut stream = AeadStream::new(Box::new(near), key(kind, "pw"), salt(kind), MAX_PAYLOAD);
        let data = vec![0x55u8; MAX_PAYLOAD + 1];
        assert_eq!(stream.write(&data).await.unwrap(), MAX_PAYLOAD, "one chunk");
        stream.write_all(&data[MAX_PAYLOAD..]).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        assert_eq!(
            wire.len(),
            16 + (2 + TAG + MAX_PAYLOAD + TAG) + (2 + TAG + 1 + TAG)
        );
        // the peer's view: two chunks, the first of the largest length
        let got = read_from(kind, "pw", wire, 4096).await.unwrap();
        assert_eq!(got, data);
    }

    #[tokio::test]
    async fn what_the_server_gets_wrong_is_an_error_that_quotes_nothing() {
        let kind = AeadKind::Aes256Gcm;
        let (_, vector) = VECTORS[2];
        let mut good = salt(kind);
        good.extend_from_slice(&hex(vector));
        let first_chunk = 32 + (2 + TAG) + (5 + TAG);
        let mut flipped = good.clone();
        flipped[32] ^= 1;
        // a length of 0x4000, sealed correctly
        let mut too_long = salt(kind);
        let mut aead = key(kind, "password").session(&salt(kind));
        aead.seal(&0x4000u16.to_be_bytes(), &mut too_long);
        let cases: [(&str, &str, Vec<u8>, io::ErrorKind, &str); 6] = [
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
                good[..20].to_vec(),
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
                "another password",
                "other",
                good.clone(),
                io::ErrorKind::InvalidData,
                UNDECRYPTABLE,
            ),
            (
                "an oversized chunk",
                "password",
                too_long,
                io::ErrorKind::InvalidData,
                TOO_LONG,
            ),
            (
                "the middle of a chunk",
                "password",
                good[..first_chunk - 1].to_vec(),
                io::ErrorKind::UnexpectedEof,
                CUT_SHORT,
            ),
        ];
        for (case, password, wire, kind_of_error, text) in cases {
            let err = read_from(kind, password, wire, 4096).await.unwrap_err();
            assert_eq!(
                (err.kind(), err.to_string().as_str()),
                (kind_of_error, text),
                "{case}"
            );
        }
        // closed between two chunks: an ordinary end
        let got = read_from(kind, "password", good[..first_chunk].to_vec(), 4096)
            .await
            .unwrap();
        assert_eq!(got, b"hello");
    }
}
