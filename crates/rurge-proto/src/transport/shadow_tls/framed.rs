//! The byte stream a Shadow TLS connection becomes after the camouflage
//! handshake: payload travels inside records that look like TLS
//! ApplicationData (`17 03 03 <len>`).
//!
//! - v2: the first frame a client writes starts with 8 bytes of the digest
//!   of the handshake; nothing else is authenticated. Until the server has
//!   seen that frame it keeps relaying the handshake server, so what arrives
//!   is offered to the camouflage session first: whatever that session can
//!   open (session tickets, as a rule) is not payload.
//! - v3: every frame is `<4-byte tag><payload>`, one HMAC chain per
//!   direction. Records the server was still relaying when the data phase
//!   began verify under the handshake's chain and are skipped.
//!
//! The same two contracts as `VmessStream`: a write that returned `Pending`
//! must be retried with the same bytes and without a flush or shutdown in
//! between (the parked frame was built from them, and in v3 its tag has
//! already moved the chain on; a flush would finish the parked frame, and the
//! retry would then seal the same bytes again), and a read error is final.

use super::auth::{Chain, TAG, V2_TAG, same};
use super::record::{ALERT, APPLICATION_DATA, HEADER, MAX_DATA, RecordReader, data_header};
use rurge_net::connector::BoxedStream;
use rustls::ClientConnection;
use std::io::{self, Read};
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// No `Debug`: the chains are keyed with the password.
pub(crate) enum Mode {
    V2 {
        /// The digest of the handshake, owed to the first frame written.
        first: Option<[u8; V2_TAG]>,
        /// The camouflage session, kept until the first record it cannot open.
        session: Option<Box<ClientConnection>>,
    },
    V3 {
        add: Chain,
        verify: Chain,
        /// The handshake's chain, until the first record it does not verify.
        ignore: Option<Chain>,
    },
}

fn invalid(text: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

enum Offered {
    /// A record of the camouflage session: not payload.
    Taken,
    /// Not something the camouflage session wrote: the data phase has begun.
    Foreign,
    /// The camouflage session ended: the server never switched to data.
    Ended,
}

fn offer(session: &mut ClientConnection, record: &[u8]) -> Offered {
    let mut rest = record;
    while !rest.is_empty() {
        if session.read_tls(&mut rest).is_err() {
            return Offered::Foreign;
        }
        match session.process_new_packets() {
            Ok(state) => {
                // nobody asked the handshake server for anything
                let mut sink = vec![0; state.plaintext_bytes_to_read()];
                let _ = session.reader().read(&mut sink);
                if state.peer_has_closed() {
                    return Offered::Ended;
                }
            }
            // what rustls says before it could open the record
            Err(
                rustls::Error::DecryptError
                | rustls::Error::InvalidMessage(_)
                | rustls::Error::PeerSentOversizedRecord,
            ) => return Offered::Foreign,
            // opened, and it was an alert or something out of place
            Err(_) => return Offered::Ended,
        }
    }
    Offered::Taken
}

impl Mode {
    /// Appends one frame carrying `payload` (at most `MAX_DATA` bytes).
    fn seal(&mut self, payload: &[u8], out: &mut Vec<u8>) {
        match self {
            Mode::V2 { first, .. } => {
                let prefix = first.take();
                let prefix = prefix.as_ref().map_or(&[][..], |p| &p[..]);
                out.extend_from_slice(&data_header(prefix.len() + payload.len()));
                out.extend_from_slice(prefix);
            }
            Mode::V3 { add, .. } => {
                out.extend_from_slice(&data_header(TAG + payload.len()));
                out.extend_from_slice(&add.frame_tag(payload));
            }
        }
        out.extend_from_slice(payload);
    }

    /// Where the payload of `record` starts; `None` for a record to skip.
    fn open(&mut self, record: &[u8]) -> io::Result<Option<usize>> {
        if let Mode::V2 { session, .. } = self
            && let Some(live) = session
        {
            match offer(live, record) {
                Offered::Taken => return Ok(None),
                Offered::Ended => {
                    return Err(invalid(
                        "shadow-tls: the handshake server closed the session",
                    ));
                }
                Offered::Foreign => *session = None,
            }
        }
        match (record[0], self) {
            // a server that answers our FIN with an alert may still be
            // sending: the end of the stream is the end of the connection
            (ALERT, _) => Ok(None),
            (APPLICATION_DATA, Mode::V2 { .. }) => Ok(Some(HEADER)),
            (APPLICATION_DATA, Mode::V3 { verify, ignore, .. }) => {
                let Some((tag, payload)) = record[HEADER..].split_at_checked(TAG) else {
                    return Err(invalid("shadow-tls: a record cannot be authenticated"));
                };
                if let Some(chain) = ignore {
                    chain.update(payload);
                    if same(&chain.digest::<TAG>(), tag) {
                        return Ok(None);
                    }
                    *ignore = None;
                }
                if !same(&verify.frame_tag(payload), tag) {
                    return Err(invalid("shadow-tls: a record cannot be authenticated"));
                }
                Ok(Some(HEADER + TAG))
            }
            _ => Err(invalid("shadow-tls: unexpected record type")),
        }
    }
}

pub(crate) struct Framed {
    inner: BoxedStream,
    reader: RecordReader,
    mode: Mode,
    /// Where the unread payload of the reader's current record starts.
    payload_at: Option<usize>,
    /// The frame being written, how much of it is out, and how many payload
    /// bytes it carries.
    out: Vec<u8>,
    out_pos: usize,
    accepted: usize,
}

impl Framed {
    /// `reader` comes from the handshake: it is between two records.
    pub(crate) fn new(inner: BoxedStream, reader: RecordReader, mode: Mode) -> Framed {
        Framed {
            inner,
            reader,
            mode,
            payload_at: None,
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

impl AsyncRead for Framed {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if let Some(at) = this.payload_at {
                let record = this.reader.record();
                let n = out.remaining().min(record.len() - at);
                out.put_slice(&record[at..at + n]);
                if at + n == record.len() {
                    this.payload_at = None;
                    this.reader.consume();
                } else {
                    this.payload_at = Some(at + n);
                }
                return Poll::Ready(Ok(()));
            }
            if !ready!(this.reader.poll_record(cx, &mut this.inner))? {
                return Poll::Ready(Ok(()));
            }
            let record = this.reader.record();
            match this.mode.open(record)? {
                // an empty payload is not the end of the stream
                Some(at) if at < record.len() => this.payload_at = Some(at),
                _ => this.reader.consume(),
            }
        }
    }
}

impl AsyncWrite for Framed {
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
            let n = data.len().min(MAX_DATA);
            this.out.clear();
            this.out_pos = 0;
            this.mode.seal(&data[..n], &mut this.out);
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
        // a parked frame first: in v3 its tag is already part of the chain
        ready!(this.poll_out(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    const PASSWORD: &[u8] = b"pw";
    const RANDOM: [u8; 32] = [7; 32];

    fn chain(side: &[u8]) -> Chain {
        Chain::new(PASSWORD, &[&RANDOM, side])
    }

    /// A v3 client stream over one end of an in-memory pipe, and the other end.
    fn v3(pipe: usize) -> (Framed, DuplexStream) {
        let (near, far) = tokio::io::duplex(pipe);
        let mode = Mode::V3 {
            add: chain(b"C"),
            verify: chain(b"S"),
            ignore: Some(Chain::new(PASSWORD, &[&RANDOM])),
        };
        (
            Framed::new(Box::new(near), RecordReader::default(), mode),
            far,
        )
    }

    fn v2(first: Option<[u8; V2_TAG]>) -> (Framed, DuplexStream) {
        let (near, far) = tokio::io::duplex(1 << 16);
        let mode = Mode::V2 {
            first,
            session: None,
        };
        (
            Framed::new(Box::new(near), RecordReader::default(), mode),
            far,
        )
    }

    /// What a v3 server would write for `payload`.
    fn sealed(chain: &mut Chain, payload: &[u8]) -> Vec<u8> {
        let mut out = data_header(TAG + payload.len()).to_vec();
        out.extend_from_slice(&chain.frame_tag(payload));
        out.extend_from_slice(payload);
        out
    }

    /// A record the server was still relaying: tagged by the handshake's
    /// chain, which does not take its own tags in.
    fn relayed(chain: &mut Chain, payload: &[u8]) -> Vec<u8> {
        chain.update(payload);
        let mut out = data_header(TAG + payload.len()).to_vec();
        out.extend_from_slice(&chain.digest::<TAG>());
        out.extend_from_slice(payload);
        out
    }

    #[tokio::test]
    async fn v3_frames_carry_the_client_chain_s_tags_and_split_at_the_record_limit() {
        let (mut stream, mut far) = v3(1 << 20);
        stream.write_all(b"hello").await.unwrap();
        let big = vec![0x42u8; MAX_DATA + 10];
        stream.write_all(&big).await.unwrap();
        // an empty write is not a frame
        assert_eq!(stream.write(&[]).await.unwrap(), 0);
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        let mut expected = Vec::new();
        let mut c = chain(b"C");
        for payload in [&b"hello"[..], &big[..MAX_DATA], &big[MAX_DATA..]] {
            expected.extend_from_slice(&sealed(&mut c, payload));
        }
        assert!(wire == expected, "{} bytes on the wire", wire.len());
    }

    #[tokio::test]
    async fn v3_reads_skip_what_the_handshake_left_behind_empty_frames_and_alerts() {
        let (mut stream, mut far) = v3(1 << 16);
        let mut handshake = Chain::new(PASSWORD, &[&RANDOM]);
        let mut s = chain(b"S");
        let mut wire = Vec::new();
        wire.extend_from_slice(&relayed(&mut handshake, b"a session ticket"));
        wire.extend_from_slice(&relayed(&mut handshake, b"and another one"));
        wire.extend_from_slice(&sealed(&mut s, b"first "));
        wire.extend_from_slice(&sealed(&mut s, b""));
        wire.extend_from_slice(&[ALERT, 3, 3, 0, 2, 1, 0]);
        wire.extend_from_slice(&sealed(&mut s, b"second"));
        far.write_all(&wire).await.unwrap();
        far.shutdown().await.unwrap();
        // a small buffer: one payload is handed over in several reads
        let mut got = Vec::new();
        let mut buf = [0u8; 4];
        loop {
            let n = stream.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, b"first second");
    }

    #[tokio::test]
    async fn a_record_of_the_handshake_s_chain_is_not_accepted_once_data_has_begun() {
        let (mut stream, mut far) = v3(1 << 16);
        let mut handshake = Chain::new(PASSWORD, &[&RANDOM]);
        let mut s = chain(b"S");
        let mut wire = sealed(&mut s, b"data");
        wire.extend_from_slice(&relayed(&mut handshake, b"late"));
        far.write_all(&wire).await.unwrap();
        let mut buf = [0u8; 16];
        assert_eq!(stream.read(&mut buf).await.unwrap(), 4);
        let err = stream.read(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            err.to_string(),
            "shadow-tls: a record cannot be authenticated"
        );
    }

    #[tokio::test]
    async fn what_is_wrong_with_a_record_is_said_in_the_error() {
        let mut s = chain(b"S");
        let mut flipped = sealed(&mut s, b"payload");
        flipped[HEADER + TAG] ^= 1;
        let mut short = data_header(2).to_vec();
        short.extend_from_slice(&[0, 0]);
        let cases: [(Vec<u8>, io::ErrorKind, &str); 4] = [
            (
                flipped,
                io::ErrorKind::InvalidData,
                "shadow-tls: a record cannot be authenticated",
            ),
            (
                short,
                io::ErrorKind::InvalidData,
                "shadow-tls: a record cannot be authenticated",
            ),
            (
                vec![22, 3, 3, 0, 1, 0],
                io::ErrorKind::InvalidData,
                "shadow-tls: unexpected record type",
            ),
            (
                vec![23, 3, 3, 0, 9, 1, 2],
                io::ErrorKind::UnexpectedEof,
                "shadow-tls: the connection ended in the middle of a record",
            ),
        ];
        for (wire, kind, text) in cases {
            let (mut stream, mut far) = v3(1 << 16);
            far.write_all(&wire).await.unwrap();
            far.shutdown().await.unwrap();
            let mut buf = [0u8; 16];
            let err = stream.read(&mut buf).await.unwrap_err();
            assert_eq!((err.kind(), err.to_string().as_str()), (kind, text));
        }
    }

    #[tokio::test]
    async fn v2_puts_the_digest_in_front_of_the_first_frame_only() {
        let (mut stream, mut far) = v2(Some(*b"8 bytes!"));
        stream.write_all(b"one").await.unwrap();
        stream.write_all(b"two").await.unwrap();
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        let mut expected = data_header(V2_TAG + 3).to_vec();
        expected.extend_from_slice(b"8 bytes!one");
        expected.extend_from_slice(&data_header(3));
        expected.extend_from_slice(b"two");
        assert_eq!(wire, expected);
    }

    #[tokio::test]
    async fn v2_reads_take_every_data_record_as_payload_whatever_its_length() {
        let (mut stream, mut far) = v2(None);
        // sing-box does not split what it copies: a frame may be longer than
        // any TLS record
        let long = vec![9u8; 40_000];
        let mut wire = data_header(3).to_vec();
        wire.extend_from_slice(b"abc");
        wire.extend_from_slice(&data_header(long.len()));
        wire.extend_from_slice(&long);
        let writer = tokio::spawn(async move {
            far.write_all(&wire).await.unwrap();
            far.shutdown().await.unwrap();
        });
        let mut got = Vec::new();
        stream.read_to_end(&mut got).await.unwrap();
        writer.await.unwrap();
        assert_eq!(got.len(), 3 + long.len());
        assert_eq!(&got[..3], b"abc");
    }

    #[tokio::test]
    async fn a_frame_parked_by_a_full_pipe_goes_out_once_and_whole() {
        // 64 bytes of pipe: every frame is parked half-written many times over
        let (stream, mut far) = v3(64);
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            let mut stream = stream;
            // the relay's way: `write_all`, then `flush`
            for piece in sent.chunks(8192) {
                stream.write_all(piece).await.unwrap();
                stream.flush().await.unwrap();
            }
            stream.shutdown().await.unwrap();
        });
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        writer.await.unwrap();
        // what a server does with it: every tag verifies, in order
        let mut c = chain(b"C");
        let (mut at, mut got) = (0, Vec::new());
        while at < wire.len() {
            let len = usize::from(u16::from_be_bytes([wire[at + 3], wire[at + 4]]));
            let (tag, payload) = wire[at + HEADER..at + HEADER + len].split_at(TAG);
            assert_eq!(c.frame_tag(payload), tag, "the frame at {at}");
            got.extend_from_slice(payload);
            at += HEADER + len;
        }
        assert!(got == data);
    }
}
