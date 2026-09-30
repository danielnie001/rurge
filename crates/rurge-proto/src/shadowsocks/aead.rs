//! The TCP stream of the AEAD methods (the Shadowsocks AEAD specification):
//! each direction starts with its own random salt, then chunks of
//! `sealed(length, 2 bytes big-endian) ‖ sealed(payload)`, both sealed with
//! the direction's session key and the next value of its counting nonce.
//!
//! SS 2022 (SIP022 3.1) is the same stream with larger chunks and headers:
//! the request puts the identity headers behind its salt and turns its first
//! write into a fixed-length header chunk and a variable-length one (the
//! address, padding, the initial payload); the response opens with a
//! fixed-length header chunk that names our salt and the length of its first
//! payload chunk.
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
use super::s2022;
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
    /// SS 2022: the response's fixed-length header.
    Head {
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

/// What the 2022 edition adds to a request stream.
pub(crate) struct Request2022 {
    /// The identity headers, between the salt and the first chunk.
    pub identity: Vec<u8>,
    /// The length of the address the first write starts with (a
    /// `LazyHead` above guarantees that it does).
    pub addr_len: usize,
    /// Seconds since the Unix epoch.
    pub now: fn() -> u64,
}

/// A 2022 request stream's state: its setup and its salt, which the
/// response must echo.
struct Edition2022 {
    request: Request2022,
    salt: Vec<u8>,
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
    /// `None`: the AEAD edition.
    edition_2022: Option<Edition2022>,
}

fn invalid(text: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

fn cut_short() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, CUT_SHORT)
}

/// Seals the first write of a 2022 request into its two header chunks: the
/// address, padding when there is no payload, and as much of the payload as
/// the variable-length header holds. The bytes of `data` it took.
fn seal_first_2022(
    up: &mut CountingAead,
    request: &Request2022,
    data: &[u8],
    out: &mut Vec<u8>,
) -> io::Result<usize> {
    let addr_len = request.addr_len.min(data.len());
    let room = s2022::MAX_VARIABLE_HEADER - addr_len - 2;
    let payload = &data[addr_len..data.len().min(addr_len + room)];
    let padding = s2022::padding_len(payload.len())
        .map_err(|_| io::Error::other("ss: no randomness available"))?;
    let variable = s2022::request_variable(&data[..addr_len], padding, payload);
    out.extend_from_slice(&request.identity);
    up.seal(&s2022::request_fixed((request.now)(), variable.len()), out);
    up.seal(&variable, out);
    Ok(addr_len + payload.len())
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
            edition_2022: None,
        }
    }

    /// An SS 2022 request stream: `key` is the user key's.
    pub(crate) fn new_2022(
        inner: BoxedStream,
        key: Arc<MasterKey>,
        salt: Vec<u8>,
        request: Request2022,
    ) -> AeadStream {
        let edition = Edition2022 {
            request,
            salt: salt.clone(),
        };
        AeadStream {
            edition_2022: Some(edition),
            ..AeadStream::new(inner, key, salt, s2022::MAX_PAYLOAD)
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
                    this.reading = match &this.edition_2022 {
                        Some(edition) => Reading::Head {
                            buf: vec![0; s2022::response_fixed_len(edition.salt.len()) + TAG],
                            filled: 0,
                        },
                        None => Reading::Len {
                            buf: [0; 2 + TAG],
                            filled: 0,
                        },
                    };
                }
                Reading::Head { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(cut_short()));
                    }
                    let down = this.down.as_mut().expect("the salt came first");
                    let Some(n) = down.open(buf) else {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    };
                    let edition = this.edition_2022.as_ref().expect("a 2022 stream");
                    let len =
                        s2022::check_response(&buf[..n], &edition.salt, (edition.request.now)())?;
                    this.reading = Reading::Body {
                        buf: vec![0; len + TAG],
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
            this.out.clear();
            this.out_pos = 0;
            let first = this.salt.take();
            if let Some(salt) = &first {
                this.out.extend_from_slice(salt);
            }
            this.accepted = match (&first, &this.edition_2022) {
                (Some(_), Some(edition)) => {
                    match seal_first_2022(&mut this.up, &edition.request, data, &mut this.out) {
                        Ok(n) => n,
                        Err(e) => {
                            this.out.clear();
                            return Poll::Ready(Err(e));
                        }
                    }
                }
                _ => {
                    let n = data.len().min(this.max_payload);
                    seal_chunk(&mut this.up, &data[..n], &mut this.out);
                    n
                }
            };
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

    const TIME: u64 = 1_700_000_000;
    /// 127.0.0.1:8080 as a SOCKS5 address.
    const ADDR: [u8; 7] = [1, 127, 0, 0, 1, 0x1f, 0x90];

    /// SS 2022 known answers, computed with a BLAKE3 written in Python from
    /// its specification and `cryptography`'s AES-GCM / AES-ECB: the key
    /// sets, the request after its salt `80 81 …` (identity headers, the
    /// header chunks for `ADDR` ‖ "hello" at `TIME`, then a chunk "world"),
    /// and a response (salt `90 91 …`, header at `TIME` naming the request
    /// salt, first chunk "ok").
    struct Vector2022 {
        kind: AeadKind,
        /// The identity keys, then the user key.
        keys: Vec<Vec<u8>>,
        request: &'static str,
        response: &'static str,
    }

    fn vectors_2022() -> [Vector2022; 2] {
        [
            Vector2022 {
                kind: AeadKind::Aes128Gcm,
                keys: vec![(0u8..16).collect(), (0x20u8..0x30).collect()],
                request: "efa5909821ac85519cb2bac2aebde4c208e3db4c9c568afe00f79400b7de08b99705e60672e49140157f36cb37ee7cdbb68b6f05e5b0781929dd6faa3f98821068d0dd702f5c25c7a6e3660c43fe09c778270316fdca0040824985d1e5c085edf95a3d34c77e0ea6434d2bb376e9afee",
                response: "909192939495969798999a9b9c9d9e9f17be40f377e19922ad1161db151a79ab845e3736f8b62015b85aa6a7abae3321e93c787a398be13e55aa4697fc05eae3783a4f75ac3cf0252cb9df1daa",
            },
            Vector2022 {
                kind: AeadKind::Aes256Gcm,
                keys: vec![(0x20u8..0x40).collect()],
                request: "a74482c100c255a6eb2bd1f55f1988d1c550161c7275ca6428763b418e260e6df7a2f7cc9b0ba2611e87e691d74fb7c14fc48011bddcc78f5767ccab920e6a74aefcdb72a6e91368c4eadf6185c04dfc07fddcc1fcd229291349b11478f41af7",
                response: "909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeaf8d4a9fedac53d86bcfa93ab21983ac82b38c11611d5a54f53de08cc2b0f5d7aef4fd69c51150aab7a37b0feedb3351fc511674611d46d29627514852c2567450524bf016bf3e555405e12149ca",
            },
        ]
    }

    fn request_salt(kind: AeadKind) -> Vec<u8> {
        (0x80..0x80 + kind.key_len() as u8).collect()
    }

    fn stream_2022(
        inner: BoxedStream,
        kind: AeadKind,
        keys: &[Vec<u8>],
        now: fn() -> u64,
    ) -> AeadStream {
        let salt = request_salt(kind);
        let request = Request2022 {
            identity: s2022::Identity::new(keys).headers(&salt),
            addr_len: ADDR.len(),
            now,
        };
        let key = Arc::new(MasterKey::from_psk(kind, keys.last().unwrap()));
        AeadStream::new_2022(inner, key, salt, request)
    }

    /// What a 2022 stream reads from a server that sends `wire` through a
    /// pipe of `capacity` bytes and then closes, its clock at `now`.
    async fn read_2022(
        kind: AeadKind,
        keys: &[Vec<u8>],
        wire: Vec<u8>,
        capacity: usize,
        now: fn() -> u64,
    ) -> io::Result<Vec<u8>> {
        let (near, mut far) = tokio::io::duplex(capacity);
        tokio::spawn(async move {
            let _ = far.write_all(&wire).await;
        });
        let mut stream = stream_2022(Box::new(near), kind, keys, now);
        let mut got = Vec::new();
        stream.read_to_end(&mut got).await.map(|_| got)
    }

    #[tokio::test]
    async fn ss_2022_writes_the_known_answer() {
        for vector in vectors_2022() {
            let (near, mut far) = tokio::io::duplex(64 * 1024);
            let mut stream = stream_2022(Box::new(near), vector.kind, &vector.keys, || TIME);
            // the address and the first payload: one write, as a `LazyHead` does it
            let first = [&ADDR[..], b"hello"].concat();
            assert_eq!(stream.write(&first).await.unwrap(), first.len());
            stream.write_all(b"world").await.unwrap();
            stream.shutdown().await.unwrap();
            let mut wire = Vec::new();
            far.read_to_end(&mut wire).await.unwrap();
            let mut expected = request_salt(vector.kind);
            expected.extend_from_slice(&hex(vector.request));
            assert_eq!(wire, expected, "{:?}", vector.kind);
        }
    }

    #[tokio::test]
    async fn ss_2022_reads_the_known_answer_whatever_the_slicing() {
        for vector in vectors_2022() {
            for capacity in [1, 7, 4096] {
                let got = read_2022(
                    vector.kind,
                    &vector.keys,
                    hex(vector.response),
                    capacity,
                    || TIME + 30,
                )
                .await
                .unwrap();
                assert_eq!(got, b"ok", "{:?} through {capacity}", vector.kind);
            }
        }
    }

    #[tokio::test]
    async fn an_address_alone_is_padded_and_a_long_first_write_fills_one_header() {
        let kind = AeadKind::Aes128Gcm;
        let keys = vec![vec![3u8; 16]];
        // the target speaks first: the address goes out alone, padded
        let (near, mut far) = tokio::io::duplex(1 << 20);
        let mut stream = stream_2022(Box::new(near), kind, &keys, || TIME);
        stream.write_all(&ADDR).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        let bare = 16 + (s2022::REQUEST_FIXED + TAG) + (ADDR.len() + 2 + TAG);
        assert!(
            (bare + 1..=bare + 900).contains(&wire.len()),
            "{} bytes",
            wire.len()
        );
        // with a payload, as much as the variable header holds, unpadded
        let (near, mut far) = tokio::io::duplex(1 << 20);
        let mut stream = stream_2022(Box::new(near), kind, &keys, || TIME);
        let first = [&ADDR[..], &vec![0x55; 100_000]].concat();
        let taken = stream.write(&first).await.unwrap();
        assert_eq!(taken, s2022::MAX_VARIABLE_HEADER - 2);
        stream.write_all(&first[taken..]).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        let rest = first.len() - taken;
        assert_eq!(
            wire.len(),
            16 + (s2022::REQUEST_FIXED + TAG) + (0xFFFF + TAG) + (2 + TAG + rest + TAG)
        );
    }

    #[tokio::test]
    async fn a_2022_answer_that_is_not_ours_is_an_error_that_quotes_nothing() {
        let kind = AeadKind::Aes128Gcm;
        let keys = vec![(0x20u8..0x30).collect::<Vec<u8>>()];
        let answer_salt = vec![0x90u8; 16];
        // an answer sealed right, its header as given
        let answer = |header: &[u8]| {
            let key = MasterKey::from_psk(kind, &keys[0]);
            let mut aead = key.session(&answer_salt);
            let mut wire = answer_salt.clone();
            aead.seal(header, &mut wire);
            aead.seal(b"ok", &mut wire);
            wire
        };
        let header = |kind_byte: u8, time: u64, salt: &[u8]| {
            let mut out = vec![kind_byte];
            out.extend_from_slice(&time.to_be_bytes());
            out.extend_from_slice(salt);
            out.extend_from_slice(&2u16.to_be_bytes());
            out
        };
        let ours = request_salt(kind);
        let mut other = ours.clone();
        other[15] ^= 1;
        let good = answer(&header(1, TIME, &ours));
        let not_ours = "ss: the server's answer is not for this request";
        let cases: [(&str, Vec<u8>, io::ErrorKind, &str); 6] = [
            (
                "silence",
                Vec::new(),
                io::ErrorKind::UnexpectedEof,
                NO_ANSWER,
            ),
            (
                "a salt alone",
                answer_salt.clone(),
                io::ErrorKind::UnexpectedEof,
                CUT_SHORT,
            ),
            (
                "our request played back",
                answer(&header(0, TIME, &ours)),
                io::ErrorKind::InvalidData,
                not_ours,
            ),
            (
                "another request's salt",
                answer(&header(1, TIME, &other)),
                io::ErrorKind::InvalidData,
                not_ours,
            ),
            (
                "a clock 31 seconds ahead",
                answer(&header(1, TIME + 31, &ours)),
                io::ErrorKind::InvalidData,
                "ss: the server's clock differs from ours by 31 seconds (at most 30 are allowed)",
            ),
            (
                "another key",
                {
                    let mut wire = good.clone();
                    wire[16] ^= 1;
                    wire
                },
                io::ErrorKind::InvalidData,
                UNDECRYPTABLE,
            ),
        ];
        for (case, wire, kind_of_error, text) in cases {
            let err = read_2022(kind, &keys, wire, 4096, || TIME)
                .await
                .unwrap_err();
            assert_eq!(
                (err.kind(), err.to_string().as_str()),
                (kind_of_error, text),
                "{case}"
            );
        }
        let got = read_2022(kind, &keys, good, 4096, || TIME).await.unwrap();
        assert_eq!(got, b"ok");
    }
}
