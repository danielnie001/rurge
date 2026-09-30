//! simple-obfs (`obfs=http` / `obfs=tls`, phase 2 M6 design 3.2): a
//! camouflage layer between the connection (or Shadow TLS) and the protocol.
//! Only the look of the bytes changes; nothing is encrypted or authenticated.
//!
//! The first packet leaves with the first non-empty write: an empty write is
//! a no-op, and a flush or a shutdown before any write sends nothing. Every
//! protocol above this layer writes its own request header first (through
//! `LazyHead`, which sends it alone after its grace), so the camouflage head
//! never needs to go out without a payload. Neither mode puts more than
//! 16 KiB of payload into the first packet: the reference server reads the
//! whole first packet into a 16 KiB buffer.
//!
//! A server that closes before sending a single byte is an ordinary EOF, so
//! the protocol above can say what that means for it (`ss` reports "closed
//! the connection without answering"); a server that answers with anything
//! else than the camouflage is an error that never quotes the answer.

mod http;
mod tls;

use crate::BuildError;
use rurge_config::HostName;
use rurge_config::spec::{ObfsMode, ObfsOpts};
use rurge_net::connector::{BoxedStream, Target};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

const NOT_OBFS: &str = "obfs: the server did not answer as an obfs server";
const MALFORMED: &str = "obfs: the server sent a malformed record";
const CUT_SHORT: &str = "obfs: the server closed the connection in the middle of a record";

/// The longest `obfs-host` written into a request or a server name.
const MAX_HOST: usize = 255;

#[derive(Clone, Debug)]
enum Kind {
    /// `host` already carries `:port` when the port is not 80.
    Http {
        uri: String,
        host: String,
    },
    Tls {
        host: String,
    },
}

/// Everything that can be prepared ahead of a connection.
pub struct ObfsClient {
    kind: Kind,
}

impl ObfsClient {
    /// `server` is the policy's own server: its name stands in for a missing
    /// `obfs-host`, and `http` appends its port unless it is 80 (whichever
    /// host is used, as the reference client does). No error text quotes the
    /// host or the path: both can identify the user.
    pub fn new(opts: &ObfsOpts, server: &Target) -> Result<ObfsClient, BuildError> {
        // the configuration layer checks both, but the fields are public and
        // a line break would end the request head early
        let printable = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_graphic());
        let host = match &opts.host {
            Some(host) if printable(host) && host.len() <= MAX_HOST => host.clone(),
            Some(_) => {
                return Err(BuildError::new(
                    "`obfs-host` cannot be written into the camouflage",
                ));
            }
            None => {
                let name = match (&server.host, opts.mode) {
                    // a server name carries no brackets
                    (HostName::Ip(ip), ObfsMode::Tls) => Some(ip.to_string()),
                    _ => crate::http::wire_host(server),
                };
                name.filter(|name| name.len() <= MAX_HOST).ok_or_else(|| {
                    BuildError::new("the server's host name cannot be written into the camouflage")
                })?
            }
        };
        let kind = match opts.mode {
            ObfsMode::Http => {
                if !opts.uri.starts_with('/') || !printable(&opts.uri) {
                    return Err(BuildError::new(
                        "`obfs-uri` cannot be written into the camouflage",
                    ));
                }
                let host = if server.port == 80 {
                    host
                } else {
                    format!("{host}:{}", server.port)
                };
                Kind::Http {
                    uri: opts.uri.clone(),
                    host,
                }
            }
            ObfsMode::Tls => Kind::Tls { host },
        };
        Ok(ObfsClient { kind })
    }

    pub fn wrap(&self, stream: BoxedStream) -> BoxedStream {
        Box::new(ObfsStream::new(stream, self.kind.clone()))
    }
}

/// Which record the server sends next (`obfs=tls`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    ServerHello,
    ChangeCipherSpec,
    /// The handshake record that carries the server's first data.
    FirstData,
    AppData,
}

impl Phase {
    fn kind(self) -> u8 {
        match self {
            Phase::ServerHello | Phase::FirstData => tls::HANDSHAKE,
            Phase::ChangeCipherSpec => tls::CHANGE_CIPHER_SPEC,
            Phase::AppData => tls::APPLICATION_DATA,
        }
    }

    fn next(self) -> Phase {
        match self {
            Phase::ServerHello => Phase::ChangeCipherSpec,
            Phase::ChangeCipherSpec => Phase::FirstData,
            Phase::FirstData | Phase::AppData => Phase::AppData,
        }
    }
}

enum ReadState {
    /// `obfs=http` before the end of the server's answer head.
    HttpHead,
    /// `obfs=tls`: every byte is inside some record.
    Records {
        /// The record whose header comes next.
        phase: Phase,
        header: [u8; tls::HEADER_LEN],
        /// How much of `header` is in.
        have: usize,
        /// What is left of the current record's body.
        left: usize,
        /// Whether the current body is data (or a record only skipped).
        deliver: bool,
    },
    /// `obfs=http` after the head: what is left in the buffer, then the
    /// stream itself.
    Raw,
}

/// Bytes through the camouflage (see the module comment).
struct ObfsStream {
    inner: BoxedStream,
    /// `Some` until the first packet is built.
    first: Option<Kind>,
    tls: bool,
    /// The packet being written and how much of it is out.
    out: Vec<u8>,
    sent: usize,
    /// The caller's bytes that `out` carries: reported once all of `out` is
    /// written, even when a flush finished it (see `poll_write`).
    accepted: usize,
    read: ReadState,
    rbuf: Vec<u8>,
    rpos: usize,
    rend: usize,
}

fn invalid(text: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

impl ObfsStream {
    fn new(inner: BoxedStream, kind: Kind) -> ObfsStream {
        let (tls, read, buffer) = match kind {
            Kind::Http { .. } => (false, ReadState::HttpHead, http::MAX_RESPONSE_HEAD),
            Kind::Tls { .. } => (
                true,
                ReadState::Records {
                    phase: Phase::ServerHello,
                    header: [0; tls::HEADER_LEN],
                    have: 0,
                    left: 0,
                    deliver: false,
                },
                tls::MAX_RECORD,
            ),
        };
        ObfsStream {
            inner,
            first: Some(kind),
            tls,
            out: Vec::new(),
            sent: 0,
            accepted: 0,
            read,
            rbuf: vec![0; buffer],
            rpos: 0,
            rend: 0,
        }
    }

    /// Builds the next packet from a prefix of `data`; returns its length.
    fn encode(&mut self, data: &[u8]) -> usize {
        match self.first.take() {
            Some(Kind::Http { uri, host }) => {
                let n = data.len().min(tls::MAX_RECORD);
                self.out = http::request_head(&uri, &host, n);
                self.out.extend_from_slice(&data[..n]);
                n
            }
            Some(Kind::Tls { host }) => {
                let n = data.len().min(tls::max_first_payload(&host));
                self.out = tls::client_hello(&host, &data[..n]);
                n
            }
            None => {
                let n = data.len().min(tls::MAX_RECORD);
                tls::app_data(&mut self.out, &data[..n]);
                n
            }
        }
    }

    /// Drives `out` into the stream below.
    fn poll_out(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.sent < self.out.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.out[self.sent..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.sent += n;
        }
        self.out.clear();
        self.sent = 0;
        Poll::Ready(Ok(()))
    }

    /// Reads more into `rbuf[rend..limit]`; 0 at EOF.
    fn poll_fill(&mut self, cx: &mut Context<'_>, limit: usize) -> Poll<io::Result<usize>> {
        let mut rb = ReadBuf::new(&mut self.rbuf[self.rend..limit]);
        ready!(Pin::new(&mut self.inner).poll_read(cx, &mut rb))?;
        let n = rb.filled().len();
        self.rend += n;
        Poll::Ready(Ok(n))
    }

    /// Hands buffered bytes over.
    fn take_buffered(&mut self, buf: &mut ReadBuf<'_>) {
        let n = (self.rend - self.rpos).min(buf.remaining());
        buf.put_slice(&self.rbuf[self.rpos..self.rpos + n]);
        self.rpos += n;
    }
}

impl AsyncRead for ObfsStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        loop {
            match &mut this.read {
                ReadState::Raw => {
                    if this.rpos < this.rend {
                        this.take_buffered(buf);
                        return Poll::Ready(Ok(()));
                    }
                    return Pin::new(&mut this.inner).poll_read(cx, buf);
                }
                ReadState::HttpHead => {
                    if let Some(end) = http::head_end(&this.rbuf[..this.rend]) {
                        if !http::is_upgrade_answer(&this.rbuf[..end]) {
                            return Poll::Ready(Err(invalid(NOT_OBFS)));
                        }
                        this.rpos = end;
                        this.read = ReadState::Raw;
                        continue;
                    }
                    if this.rend == this.rbuf.len() {
                        return Poll::Ready(Err(invalid(NOT_OBFS)));
                    }
                    let limit = this.rbuf.len();
                    if ready!(this.poll_fill(cx, limit))? == 0 {
                        if this.rend == 0 {
                            // closed without a word: the protocol above says why
                            return Poll::Ready(Ok(()));
                        }
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            NOT_OBFS,
                        )));
                    }
                }
                ReadState::Records {
                    phase,
                    header,
                    have,
                    left,
                    deliver,
                } => {
                    let avail = this.rend - this.rpos;
                    if *left > 0 && avail > 0 {
                        let n = (*left).min(avail);
                        if *deliver {
                            let n = n.min(buf.remaining());
                            buf.put_slice(&this.rbuf[this.rpos..this.rpos + n]);
                            this.rpos += n;
                            *left -= n;
                            return Poll::Ready(Ok(()));
                        }
                        this.rpos += n;
                        *left -= n;
                        continue;
                    }
                    if *left == 0 && avail > 0 {
                        let n = (tls::HEADER_LEN - *have).min(avail);
                        header[*have..*have + n]
                            .copy_from_slice(&this.rbuf[this.rpos..this.rpos + n]);
                        this.rpos += n;
                        *have += n;
                        if *have == tls::HEADER_LEN {
                            let Some(len) = tls::record_len(header, phase.kind()) else {
                                let why = if *phase == Phase::AppData {
                                    MALFORMED
                                } else {
                                    NOT_OBFS
                                };
                                return Poll::Ready(Err(invalid(why)));
                            };
                            *have = 0;
                            *left = len;
                            *deliver = matches!(*phase, Phase::FirstData | Phase::AppData);
                            *phase = phase.next();
                        }
                        continue;
                    }
                    // everything buffered is consumed: start over at the front
                    let at_start = *phase == Phase::ServerHello && *have == 0;
                    let at_boundary = *phase == Phase::AppData && *have == 0 && *left == 0;
                    // the server's first data record has not begun yet
                    let handshake = *phase != Phase::AppData;
                    this.rpos = 0;
                    this.rend = 0;
                    let limit = this.rbuf.len();
                    if ready!(this.poll_fill(cx, limit))? == 0 {
                        if at_start || at_boundary {
                            return Poll::Ready(Ok(()));
                        }
                        let why = if handshake { NOT_OBFS } else { CUT_SHORT };
                        return Poll::Ready(Err(io::Error::new(io::ErrorKind::UnexpectedEof, why)));
                    }
                }
            }
        }
    }
}

impl AsyncWrite for ObfsStream {
    /// Success means the packet carrying the bytes reached the stream below.
    /// A packet that the stream below took only in part is finished first;
    /// every caller here retries with the same buffer (see `WsByteStream`).
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if self.out.is_empty() && self.accepted == 0 {
            if self.first.is_none() && !self.tls {
                return Pin::new(&mut self.inner).poll_write(cx, data);
            }
            self.accepted = self.encode(data);
        }
        ready!(self.poll_out(cx))?;
        // a flush may have finished the packet: the retry learns it here
        let n = self.accepted.min(data.len());
        self.accepted = 0;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.poll_out(cx))?;
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.poll_out(cx))?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ObfsHello, accept_obfs};
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::net::{TcpListener, TcpStream};

    const BOTH: [ObfsMode; 2] = [ObfsMode::Http, ObfsMode::Tls];

    fn opts(mode: ObfsMode, host: Option<&str>, uri: &str) -> ObfsOpts {
        ObfsOpts {
            mode,
            host: host.map(str::to_string),
            uri: uri.to_string(),
        }
    }

    fn edge(port: u16) -> Target {
        Target::new(HostName::parse("edge.example"), port)
    }

    fn client(mode: ObfsMode) -> ObfsClient {
        ObfsClient::new(&opts(mode, Some("cdn.example"), "/path"), &edge(8388)).unwrap()
    }

    /// A wrapped client end and the raw server end.
    fn pipe(mode: ObfsMode, capacity: usize) -> (BoxedStream, DuplexStream) {
        let (near, far) = tokio::io::duplex(capacity);
        (client(mode).wrap(Box::new(near)), far)
    }

    /// The server's first packet (hand-written, independent of the fake).
    fn answer(mode: ObfsMode, first: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        match mode {
            ObfsMode::Http => out.extend_from_slice(
                b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n",
            ),
            ObfsMode::Tls => {
                out.extend_from_slice(&[0x16, 0x03, 0x01, 0x00, 0x5b, 0x02, 0x00, 0x00, 0x57]);
                out.extend_from_slice(&[0u8; 87]);
                out.extend_from_slice(&[0x14, 0x03, 0x03, 0x00, 0x01, 0x01]);
                out.extend_from_slice(&[0x16, 0x03, 0x03]);
                out.extend_from_slice(&(first.len() as u16).to_be_bytes());
            }
        }
        out.extend_from_slice(first);
        out
    }

    fn record(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x17, 0x03, 0x03];
        out.extend_from_slice(&(data.len() as u16).to_be_bytes());
        out.extend_from_slice(data);
        out
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len as u32).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn the_host_defaults_to_the_server_and_http_adds_the_port() {
        let host = |mode, host: Option<&str>, server: Target| match ObfsClient::new(
            &opts(mode, host, "/"),
            &server,
        )
        .unwrap()
        .kind
        {
            Kind::Http { host, .. } | Kind::Tls { host } => host,
        };
        let v6 = |port| Target::new(HostName::parse("2001:db8::1"), port);
        assert_eq!(host(ObfsMode::Http, None, edge(8388)), "edge.example:8388");
        assert_eq!(host(ObfsMode::Http, None, edge(80)), "edge.example");
        assert_eq!(host(ObfsMode::Http, Some("cdn.test"), edge(80)), "cdn.test");
        assert_eq!(
            host(ObfsMode::Http, Some("cdn.test"), edge(443)),
            "cdn.test:443"
        );
        assert_eq!(host(ObfsMode::Http, None, v6(80)), "[2001:db8::1]");
        assert_eq!(host(ObfsMode::Tls, None, v6(443)), "2001:db8::1");
        assert_eq!(host(ObfsMode::Tls, None, edge(8388)), "edge.example");
        assert_eq!(
            host(ObfsMode::Tls, Some("cdn.test"), edge(8388)),
            "cdn.test"
        );
    }

    #[test]
    fn what_cannot_be_written_is_a_build_error_that_quotes_nothing() {
        let long = "a".repeat(256);
        for (o, server, expected) in [
            (
                opts(ObfsMode::Http, Some("a b"), "/"),
                edge(80),
                "`obfs-host` cannot be written into the camouflage",
            ),
            (
                opts(ObfsMode::Tls, Some(&long), "/"),
                edge(80),
                "`obfs-host` cannot be written into the camouflage",
            ),
            (
                opts(ObfsMode::Http, None, "no-slash"),
                edge(80),
                "`obfs-uri` cannot be written into the camouflage",
            ),
            (
                opts(ObfsMode::Http, None, "/a\r\nX: 1"),
                edge(80),
                "`obfs-uri` cannot be written into the camouflage",
            ),
            (
                opts(ObfsMode::Tls, None, "/"),
                Target::new(HostName::Domain("a@b.test".into()), 443),
                "the server's host name cannot be written into the camouflage",
            ),
        ] {
            let err = ObfsClient::new(&o, &server).err().expect("refused");
            assert_eq!(err.message, expected);
        }
    }

    #[tokio::test]
    async fn http_the_first_write_carries_the_head_and_later_ones_are_raw() {
        let (mut stream, mut server) = pipe(ObfsMode::Http, 64 * 1024);
        stream.write_all(b"first").await.unwrap();
        let mut seen = Vec::new();
        while !seen.ends_with(b"\r\n\r\nfirst") {
            let mut byte = [0u8; 1];
            server.read_exact(&mut byte).await.unwrap();
            seen.push(byte[0]);
        }
        let head = String::from_utf8(seen).unwrap();
        assert!(
            head.starts_with("GET /path HTTP/1.1\r\nHost: cdn.example:8388\r\nUser-Agent: curl/7."),
            "{head}"
        );
        assert!(
            head.contains("\r\nContent-Length: 5\r\n\r\nfirst"),
            "{head}"
        );
        stream.write_all(b"second").await.unwrap();
        let mut raw = [0u8; 6];
        server.read_exact(&mut raw).await.unwrap();
        assert_eq!(&raw, b"second");
        // the answer head is taken off, whatever follows it is data
        server
            .write_all(&answer(ObfsMode::Http, b"hello"))
            .await
            .unwrap();
        server.write_all(b"world").await.unwrap();
        let mut back = [0u8; 10];
        stream.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"helloworld");
        // a half-close reaches the server and the other way stays open
        stream.shutdown().await.unwrap();
        let mut rest = Vec::new();
        server.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
        server.write_all(b"late").await.unwrap();
        let mut late = [0u8; 4];
        stream.read_exact(&mut late).await.unwrap();
        assert_eq!(&late, b"late");
    }

    #[tokio::test]
    async fn tls_the_first_write_rides_in_the_ticket_and_later_ones_split_at_16384() {
        let (mut stream, mut server) = pipe(ObfsMode::Tls, 256 * 1024);
        stream.write_all(b"first").await.unwrap();
        let mut hello = vec![0u8; tls::HELLO_OVERHEAD + 5 + "cdn.example".len()];
        server.read_exact(&mut hello).await.unwrap();
        assert_eq!(&hello[138..142], &[0x00, 0x23, 0x00, 0x05]);
        assert_eq!(&hello[142..147], b"first");
        assert_eq!(&hello[156..167], b"cdn.example");
        let data = pattern(40_000);
        stream.write_all(&data).await.unwrap();
        let mut got = Vec::new();
        let mut lengths = Vec::new();
        while got.len() < data.len() {
            let mut header = [0u8; 5];
            server.read_exact(&mut header).await.unwrap();
            assert_eq!(&header[..3], &[0x17, 0x03, 0x03]);
            let len = usize::from(u16::from_be_bytes([header[3], header[4]]));
            let mut body = vec![0u8; len];
            server.read_exact(&mut body).await.unwrap();
            lengths.push(len);
            got.extend_from_slice(&body);
        }
        assert_eq!(lengths, [16384, 16384, 7232]);
        assert_eq!(got, data);
        stream.shutdown().await.unwrap();
        let mut rest = Vec::new();
        server.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty(), "no close_notify or anything else");
    }

    #[tokio::test]
    async fn the_first_packet_carries_at_most_16_kib_and_an_empty_write_sends_nothing() {
        for (mode, expected) in [
            (ObfsMode::Http, 16384),
            (ObfsMode::Tls, tls::max_first_payload("cdn.example")),
        ] {
            let (mut stream, _server) = pipe(mode, 256 * 1024);
            assert_eq!(stream.write(&[1u8; 20_000]).await.unwrap(), expected);
            let (mut idle, mut server) = pipe(mode, 1024);
            assert_eq!(idle.write(&[]).await.unwrap(), 0);
            idle.flush().await.unwrap();
            idle.shutdown().await.unwrap();
            let mut rest = Vec::new();
            server.read_to_end(&mut rest).await.unwrap();
            assert!(rest.is_empty(), "{mode:?}");
        }
    }

    #[tokio::test]
    async fn the_servers_answer_is_read_whatever_the_slicing() {
        let big = pattern(20_000);
        for mode in BOTH {
            let mut wire = answer(mode, b"hello ");
            match mode {
                ObfsMode::Http => {
                    wire.extend_from_slice(b"obfs ");
                    wire.extend_from_slice(&big);
                }
                ObfsMode::Tls => {
                    wire.extend_from_slice(&record(b""));
                    wire.extend_from_slice(&record(b"obfs "));
                    wire.extend_from_slice(&record(&big[..16384]));
                    wire.extend_from_slice(&record(&big[16384..]));
                }
            }
            let mut expected = b"hello obfs ".to_vec();
            expected.extend_from_slice(&big);
            // (bytes the pipe holds, bytes per read): one byte at a time,
            // odd slices, and reads larger than a record
            for (capacity, chunk) in [(1usize, 1usize), (7, 3), (4096, 20_000)] {
                let (mut stream, mut server) = pipe(mode, capacity);
                let to_send = wire.clone();
                let writer = tokio::spawn(async move {
                    server.write_all(&to_send).await.unwrap();
                    server.shutdown().await.unwrap();
                    server
                });
                let mut got = Vec::new();
                let mut buf = vec![0u8; chunk];
                loop {
                    let n = stream.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                assert!(got == expected, "{mode:?} {capacity} {chunk}");
                drop(writer.await.unwrap());
            }
        }
    }

    #[tokio::test]
    async fn a_server_that_is_not_obfs_fails_without_quoting_it() {
        let mut after_hello = answer(ObfsMode::Tls, b"x");
        after_hello.extend_from_slice(&[0x15, 0x03, 0x03, 0x00, 0x02, b's', b'e']);
        let mut cut = answer(ObfsMode::Tls, b"x");
        // what arrived of a record cut short is still data
        cut.extend_from_slice(&[0x17, 0x03, 0x03, 0x00, 0x0a, b'a', b'b', b'c']);
        for (mode, wire, expected) in [
            (
                ObfsMode::Http,
                b"HTTP/1.1 400 Bad Request\r\n\r\nsecret".to_vec(),
                NOT_OBFS,
            ),
            (ObfsMode::Http, vec![b'a'; 9000], NOT_OBFS),
            (ObfsMode::Http, b"HTTP/1.1 101 OK\r\nsec".to_vec(), NOT_OBFS),
            (
                ObfsMode::Tls,
                b"HTTP/1.1 200 OK\r\n\r\nsecret".to_vec(),
                NOT_OBFS,
            ),
            (ObfsMode::Tls, vec![0x16, 0x03, 0x01, 0x40, 0x01], NOT_OBFS),
            (
                ObfsMode::Tls,
                vec![0x16, 0x03, 0x01, 0x00, 0x5b, 0x02],
                NOT_OBFS,
            ),
            (ObfsMode::Tls, after_hello, MALFORMED),
            (ObfsMode::Tls, cut, CUT_SHORT),
        ] {
            let (mut stream, mut server) = pipe(mode, 64 * 1024);
            server.write_all(&wire).await.unwrap();
            server.shutdown().await.unwrap();
            let mut got = Vec::new();
            let err = stream.read_to_end(&mut got).await.expect_err("not obfs");
            assert_eq!(err.to_string(), expected, "{mode:?}");
            assert!(!got.windows(3).any(|w| w == b"sec"), "{mode:?}");
        }
    }

    #[tokio::test]
    async fn a_server_that_closes_before_a_word_is_a_plain_eof() {
        for mode in BOTH {
            let (mut stream, mut server) = pipe(mode, 1024);
            server.shutdown().await.unwrap();
            let mut got = Vec::new();
            assert_eq!(stream.read_to_end(&mut got).await.unwrap(), 0, "{mode:?}");
        }
    }

    /// Accepts one connection, echoes through the fake, returns the hello.
    async fn fake_echo(mode: ObfsMode) -> (SocketAddr, tokio::task::JoinHandle<ObfsHello>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let (stream, hello) = accept_obfs(Box::new(tcp), mode).await.unwrap();
            let (mut rd, mut wr) = tokio::io::split(stream);
            tokio::io::copy(&mut rd, &mut wr).await.unwrap();
            wr.shutdown().await.unwrap();
            hello
        });
        (addr, task)
    }

    #[tokio::test]
    async fn round_trips_through_the_fake_server_with_a_half_close() {
        tokio::time::timeout(Duration::from_secs(30), async {
            for mode in BOTH {
                let (addr, fake) = fake_echo(mode).await;
                let tcp = TcpStream::connect(addr).await.unwrap();
                let stream = client(mode).wrap(Box::new(tcp));
                let payload = pattern(1 << 20);
                let (mut rd, mut wr) = tokio::io::split(stream);
                let to_send = payload.clone();
                let writer = tokio::spawn(async move {
                    for chunk in to_send.chunks(50_000) {
                        wr.write_all(chunk).await.unwrap();
                    }
                    wr.shutdown().await.unwrap();
                });
                let mut back = Vec::new();
                rd.read_to_end(&mut back).await.unwrap();
                writer.await.unwrap();
                assert!(back == payload, "{mode:?}: the echo differs");
                let hello = fake.await.unwrap();
                let first = match mode {
                    ObfsMode::Http => {
                        assert_eq!(hello.host, "cdn.example:8388");
                        assert_eq!(hello.uri.as_deref(), Some("/path"));
                        assert!(hello.user_agent.unwrap().starts_with("curl/7."));
                        16384
                    }
                    ObfsMode::Tls => {
                        assert_eq!(hello.host, "cdn.example");
                        tls::max_first_payload("cdn.example")
                    }
                };
                assert!(hello.first_payload == payload[..first], "{mode:?}");
            }
        })
        .await
        .expect("the round trips finished within the bound");
    }
}
