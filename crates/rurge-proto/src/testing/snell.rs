//! A scriptable Snell server of one version, 1 to 5: optionally simple-obfs
//! in front, then the version's record stream, requests one after another
//! on a connection (ConnectV2), and a relay. Its records, chunks, nonce
//! counting, padding and request parsing are written apart from the
//! client's (`crate::snell`), so each checks the other; only the primitives
//! (the Argon2id derivation and the two AEAD ciphers, which have vectors of
//! their own) are shared. It never resolves a name, and never connects or
//! sends to another address than 127.0.0.1 and ::1.
//!
//! Versions 4 and 5 are one wire format. As the reference server (per the
//! byte-level notes of the M6b plan) it answers a request only when the
//! target first sends: `00` and the data in one record, the direction's
//! first record padded. A target that ends before sending anything gets
//! `02 65 "Remote EOF"` (ConnectV2) or `02 ff "end of file"` (Connect) and
//! the close. When the target ends, a ConnectV2 tunnel sends its empty
//! record, forwards (or, once the target is gone, discards) the client's
//! records until the client's empty record, then reads the next request; a
//! Connect tunnel closes instead.
//!
//! Versions 1 to 3 speak the chunks of the original protocol, version 1
//! under ChaCha20-Poly1305. ConnectV2 came with version 2 and UDP with
//! version 3, but the script's version says how records are written and
//! nothing of the commands: like the reference listener of these versions,
//! which compares the request's own version byte and no more, it serves
//! Connect, ConnectV2 and UDP alike. The answer rides with the target's
//! first data here too, and a target that ends before sending anything gets
//! `02 65 "Remote EOF"` whichever command asked, as that listener answers;
//! a Connect connection is closed after it, and a ConnectV2 one kept, as
//! the listener keeps it (a client drops it after an error either way). An
//! end is a zero-length chunk — the sealed length alone — on a ConnectV2
//! tunnel, each way; a Connect tunnel takes the client's close for its end
//! as well, and closes when the target ends.
//!
//! UDP (`06`) is answered `00` at once; then every record of the client's
//! is one datagram, sent from a loopback socket of the connection's own,
//! and whatever reaches that socket goes back as `04 IPv4 port payload`
//! (full cone). Names are never resolved: a datagram for one goes to
//! `connect_to`, or nowhere.
//!
//! A wrong PSK — or a client of another record format — gets no word back,
//! only the close; or, with `reset`, the reset a server sends that closes
//! with the client's bytes still unread.

use super::obfs::{ObfsHello, accept_obfs};
use super::{AbortOnDrop, connect_upstream, is_localhost};
use crate::shadowsocks::cipher::{AeadCipher, AeadKind, TAG};
use crate::snell::kdf;
use rurge_config::spec::ObfsMode;
use rurge_net::connector::BoxedStream;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

const SALT: usize = 16;
const HEADER: usize = 7 + TAG;
const VERSION: u8 = 0x04;
const MAX_PAYLOAD: usize = 0x3FFF;
const CONNECT: u8 = 0x01;
const CONNECT_V2: u8 = 0x05;
const UDP: u8 = 0x06;

#[derive(Clone, Debug)]
pub struct SnellScript {
    pub psk: String,
    /// The version it speaks, 1 to 5: 1 to 3 in chunks, 4 and 5 in v4's
    /// records (it takes either's clients).
    pub version: u8,
    /// Expect simple-obfs `http` in front of the protocol.
    pub obfs_http: bool,
    /// Expect simple-obfs `tls` in front of the protocol (versions 1 to 3).
    pub obfs_tls: bool,
    /// Relay here whatever the client asked for (needed for a domain target).
    pub connect_to: Option<SocketAddr>,
    /// A request that arrives after this many tunnels on its connection is
    /// not answered: the connection closes (a server that retires reused
    /// connections).
    pub tunnels_per_connection: Option<usize>,
    /// A request that arrives after this many tunnels on its connection is
    /// answered with bytes that do not decrypt, then the connection closes.
    pub garble_after: Option<usize>,
    /// Answer every request with this error code and message, then close.
    pub refuse: Option<(u8, Vec<u8>)>,
    /// In front of every UDP answer, two records that are no datagram: an
    /// unknown address family and an address cut short.
    pub udp_junk: bool,
    /// Zero linger on every connection it accepts: one dropped without the
    /// orderly close is reset. A client it rejects is dropped so, and gets
    /// the reset in place of the close.
    pub reset: bool,
}

impl SnellScript {
    pub fn new(psk: &str) -> SnellScript {
        SnellScript {
            psk: psk.to_string(),
            version: 4,
            obfs_http: false,
            obfs_tls: false,
            connect_to: None,
            tunnels_per_connection: None,
            garble_after: None,
            refuse: None,
            udp_junk: false,
            reset: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedSnell {
    /// `01` Connect, `05` ConnectV2, `06` UDP.
    pub command: u8,
    pub client_id: Vec<u8>,
    /// Exactly as it was on the wire (an IP literal is text too); UDP has
    /// no target (empty, port 0).
    pub host: String,
    pub port: u16,
    /// Payload after the request, in the same record (UDP: after the id).
    pub early: Vec<u8>,
    /// The connection's number, from 0 in the order they were accepted.
    pub connection: usize,
    /// The request's number on its connection, from 0.
    pub tunnel: usize,
    /// The padding of the record that carried the request.
    pub padding: usize,
}

pub struct FakeSnell {
    addr: SocketAddr,
    shared: Arc<Shared>,
    _task: AbortOnDrop,
}

#[derive(Default)]
struct Seen {
    requests: Mutex<Vec<RecordedSnell>>,
    obfs: Mutex<Vec<ObfsHello>>,
    connections: AtomicUsize,
    rejected: AtomicUsize,
    unanswered: AtomicUsize,
    largest_record: AtomicUsize,
    /// Every UDP datagram's target, `host:port`, the name as on the wire.
    datagrams: Mutex<Vec<String>>,
    /// Each UDP connection's own socket, in the order they opened.
    udp_outside: Mutex<Vec<SocketAddr>>,
}

struct Shared {
    script: SnellScript,
    seen: Seen,
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("fake snell: {what}"))
}

/// One direction's cipher, its nonce a little-endian counter from zero.
struct Direction {
    cipher: AeadCipher,
    count: u64,
    /// Versions 1 to 3: chunks, not v4's records.
    chunked: bool,
}

impl Direction {
    /// Version 1 keys ChaCha20-Poly1305 with all of Argon2id's output, the
    /// later ones AES-128-GCM with its first half.
    fn new(version: u8, psk: &str, salt: &[u8; SALT]) -> Direction {
        let output = kdf::derive(psk.as_bytes(), salt);
        let cipher = if version == 1 {
            AeadCipher::new(AeadKind::ChaCha20Poly1305, &output)
        } else {
            AeadCipher::new(AeadKind::Aes128Gcm, &output[..16])
        };
        Direction {
            cipher,
            count: 0,
            chunked: version <= 3,
        }
    }

    fn nonce(&mut self) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[..8].copy_from_slice(&self.count.to_le_bytes());
        self.count += 1;
        nonce
    }

    fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let nonce = self.nonce();
        let mut sealed = plain.to_vec();
        let tag = self.cipher.seal_in_place(&nonce, &mut sealed);
        sealed.extend_from_slice(&tag);
        sealed
    }

    fn open(&mut self, sealed: &[u8]) -> Option<Vec<u8>> {
        let nonce = self.nonce();
        let (data, tag) = sealed.split_at(sealed.len().checked_sub(TAG)?);
        let mut data = data.to_vec();
        let tag: [u8; TAG] = tag.try_into().ok()?;
        self.cipher
            .open_in_place(&nonce, &mut data, &tag)
            .then_some(data)
    }
}

/// Swaps `a[i]` and `b[i]` for the even `i` below the shorter length.
fn unmix(a: &mut [u8], b: &mut [u8]) {
    let mut i = 0;
    while i < a.len() && i < b.len() {
        std::mem::swap(&mut a[i], &mut b[i]);
        i += 2;
    }
}

/// `Ok(false)`: the peer closed before the first byte.
async fn read_full<R: AsyncRead + Unpin>(r: &mut R, buf: &mut [u8]) -> io::Result<bool> {
    if r.read(&mut buf[..1]).await? == 0 {
        return Ok(false);
    }
    r.read_exact(&mut buf[1..]).await?;
    Ok(true)
}

enum Record {
    Data { payload: Vec<u8>, padding: usize },
    End,
}

/// The client's next record; `Ok(None)` when it closed between records.
async fn read_record<R: AsyncRead + Unpin>(
    r: &mut R,
    up: &mut Direction,
    seen: &Seen,
) -> io::Result<Option<Record>> {
    if up.chunked {
        read_chunk(r, up, seen).await
    } else {
        read_v4_record(r, up, seen).await
    }
}

/// A chunk of versions 1 to 3: the sealed length, then the sealed payload —
/// which a length of zero, the end, does not have.
async fn read_chunk<R: AsyncRead + Unpin>(
    r: &mut R,
    up: &mut Direction,
    seen: &Seen,
) -> io::Result<Option<Record>> {
    let mut sealed = [0u8; 2 + TAG];
    if !read_full(r, &mut sealed).await? {
        return Ok(None);
    }
    let len = up
        .open(&sealed)
        .ok_or_else(|| bad("a length that does not authenticate"))?;
    // the reserved bits too: a client never sets them
    let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
    if len > MAX_PAYLOAD {
        return Err(bad("a chunk over 0x3fff"));
    }
    seen.largest_record.fetch_max(len, Ordering::SeqCst);
    if len == 0 {
        return Ok(Some(Record::End));
    }
    let mut body = vec![0u8; len + TAG];
    r.read_exact(&mut body).await?;
    let payload = up
        .open(&body)
        .ok_or_else(|| bad("a payload that does not authenticate"))?;
    Ok(Some(Record::Data {
        payload,
        padding: 0,
    }))
}

/// A record of versions 4 and 5.
async fn read_v4_record<R: AsyncRead + Unpin>(
    r: &mut R,
    up: &mut Direction,
    seen: &Seen,
) -> io::Result<Option<Record>> {
    let mut sealed = [0u8; HEADER];
    if !read_full(r, &mut sealed).await? {
        return Ok(None);
    }
    let header = up
        .open(&sealed)
        .ok_or_else(|| bad("a header that does not authenticate"))?;
    if header[0] != VERSION {
        return Err(bad("a record of another version"));
    }
    let padding = usize::from(u16::from_be_bytes([header[3], header[4]]));
    let len = usize::from(u16::from_be_bytes([header[5], header[6]]));
    if len > MAX_PAYLOAD {
        return Err(bad("a record over 0x3fff"));
    }
    seen.largest_record.fetch_max(len, Ordering::SeqCst);
    let mut body = vec![0u8; padding + if len == 0 { 0 } else { len + TAG }];
    r.read_exact(&mut body).await?;
    if len == 0 {
        return Ok(Some(Record::End));
    }
    let (pad, payload) = body.split_at_mut(padding);
    unmix(pad, payload);
    let payload = up
        .open(payload)
        .ok_or_else(|| bad("a payload that does not authenticate"))?;
    Ok(Some(Record::Data { payload, padding }))
}

/// The server's direction: its salt goes with the first record, and in v4's
/// format the first record with a payload is padded.
struct Answers {
    down: Direction,
    salt: Option<[u8; SALT]>,
    padded: bool,
}

impl Answers {
    fn new(version: u8, psk: &str) -> Answers {
        let mut salt = [0u8; SALT];
        getrandom::fill(&mut salt).expect("randomness");
        Answers {
            down: Direction::new(version, psk, &salt),
            salt: Some(salt),
            padded: false,
        }
    }

    /// One record of `payload`; empty: the end of this tunnel.
    fn record(&mut self, payload: &[u8]) -> Vec<u8> {
        let mut out = self.salt.take().map(Vec::from).unwrap_or_default();
        if self.down.chunked {
            // the sealed length, and no payload behind a length of zero
            let len = (payload.len() as u16).to_be_bytes();
            out.extend_from_slice(&self.down.seal(&len));
            if !payload.is_empty() {
                out.extend_from_slice(&self.down.seal(payload));
            }
            return out;
        }
        let mut padding = Vec::new();
        if !payload.is_empty() && !self.padded {
            self.padded = true;
            let mut len = [0u8; 1];
            getrandom::fill(&mut len).expect("randomness");
            padding = vec![0u8; 256 + usize::from(len[0])];
            getrandom::fill(&mut padding).expect("randomness");
        }
        let mut header = [VERSION, 0, 0, 0, 0, 0, 0];
        header[3..5].copy_from_slice(&(padding.len() as u16).to_be_bytes());
        header[5..7].copy_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.down.seal(&header));
        if !payload.is_empty() {
            let mut sealed = self.down.seal(payload);
            unmix(&mut padding, &mut sealed);
            out.extend_from_slice(&padding);
            out.extend_from_slice(&sealed);
        }
        out
    }

    /// `02 code length message`.
    fn error(&mut self, code: u8, message: &[u8]) -> Vec<u8> {
        let mut payload = vec![0x02, code, message.len() as u8];
        payload.extend_from_slice(message);
        self.record(&payload)
    }
}

/// `01 command id-length id host-length host port early`, UDP's
/// `01 06 id-length id`.
fn parse_request(p: &[u8]) -> Option<RecordedSnell> {
    if *p.first()? != 0x01 {
        return None;
    }
    let command = *p.get(1)?;
    let id_len = usize::from(*p.get(2)?);
    let client_id = p.get(3..3 + id_len)?.to_vec();
    let at = 3 + id_len;
    if command == UDP {
        return Some(RecordedSnell {
            command,
            client_id,
            host: String::new(),
            port: 0,
            early: p[at..].to_vec(),
            connection: 0,
            tunnel: 0,
            padding: 0,
        });
    }
    if command != CONNECT && command != CONNECT_V2 {
        return None;
    }
    let host_len = usize::from(*p.get(at)?);
    let host = p.get(at + 1..at + 1 + host_len)?;
    let at = at + 1 + host_len;
    let port = u16::from_be_bytes(p.get(at..at + 2)?.try_into().ok()?);
    Some(RecordedSnell {
        command,
        client_id,
        host: String::from_utf8_lossy(host).into_owned(),
        port,
        early: p[at + 2..].to_vec(),
        connection: 0,
        tunnel: 0,
        padding: 0,
    })
}

/// A client datagram, `01 host-length host port payload` or
/// `01 00 04|06 address port payload`: `(host, port, payload)`.
fn parse_datagram(p: &[u8]) -> Option<(String, u16, &[u8])> {
    if *p.first()? != 0x01 {
        return None;
    }
    let (host, at) = match *p.get(1)? {
        0 => match *p.get(2)? {
            4 => {
                let b: [u8; 4] = p.get(3..7)?.try_into().ok()?;
                (Ipv4Addr::from(b).to_string(), 7)
            }
            6 => {
                let b: [u8; 16] = p.get(3..19)?.try_into().ok()?;
                (Ipv6Addr::from(b).to_string(), 19)
            }
            _ => return None,
        },
        len => {
            let len = usize::from(len);
            (
                String::from_utf8_lossy(p.get(2..2 + len)?).into_owned(),
                2 + len,
            )
        }
    };
    let port = u16::from_be_bytes(p.get(at..at + 2)?.try_into().ok()?);
    Some((host, port, &p[at + 2..]))
}

/// Shuts our side and reads until the client goes: closing with unread
/// data would reset.
async fn close<R, W>(r: &mut R, w: &mut W)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let _ = w.shutdown().await;
    let mut sink = [0u8; 4096];
    while let Ok(n) = r.read(&mut sink).await {
        if n == 0 {
            break;
        }
    }
}

type Reader = ReadHalf<BoxedStream>;
type Writer = WriteHalf<BoxedStream>;

impl Shared {
    /// Where to relay a request; `None`: a name without `connect_to`, or
    /// another address than 127.0.0.1 and ::1.
    fn upstream(&self, request: &RecordedSnell) -> Option<SocketAddr> {
        self.script.connect_to.or_else(|| {
            let ip = request.host.parse::<IpAddr>().ok()?;
            is_localhost(ip).then(|| SocketAddr::new(ip, request.port))
        })
    }

    /// One tunnel: `true` when both sides ended with their empty records
    /// (ConnectV2), so the next request may follow.
    async fn tunnel(
        &self,
        request: &RecordedSnell,
        reader: &mut Reader,
        up: &mut Direction,
        writer: &mut Writer,
        answers: &mut Answers,
    ) -> bool {
        let reuse = request.command == CONNECT_V2;
        let chunked = up.chunked;
        let upstream = match self.upstream(request) {
            Some(to) => connect_upstream(to).await,
            None => None,
        };
        let Some(mut upstream) = upstream else {
            let _ = writer
                .write_all(&answers.error(0x01, b"fake snell: cannot connect"))
                .await;
            close(reader, writer).await;
            return false;
        };
        // a target already gone shows in its answer
        let _ = upstream.write_all(&request.early).await;
        let (mut target_read, mut target_write) = upstream.split();
        let requests = async {
            loop {
                match read_record(reader, up, &self.seen).await {
                    // once the target is gone, the client's data is discarded
                    Ok(Some(Record::Data { payload, .. })) => {
                        let _ = target_write.write_all(&payload).await;
                    }
                    Ok(Some(Record::End)) => {
                        let _ = target_write.shutdown().await;
                        return true;
                    }
                    // versions 1 and 3 end a request with the close: no
                    // zero-length chunk, and no connection for another
                    Ok(None) if chunked => {
                        let _ = target_write.shutdown().await;
                        return false;
                    }
                    _ => return false,
                }
            }
        };
        let replies = async {
            let mut buf = vec![0u8; MAX_PAYLOAD];
            let n = target_read.read(&mut buf[1..]).await.unwrap_or(0);
            if n == 0 {
                // versions 1 to 3 say `Remote EOF` to a Connect too
                let error = if reuse || chunked {
                    answers.error(0x65, b"Remote EOF")
                } else {
                    answers.error(0xff, b"end of file")
                };
                let _ = writer.write_all(&error).await;
                // the listener of versions 1 to 3 keeps a ConnectV2
                // connection after the error: the client is the one to let
                // go of it
                if reuse && chunked {
                    return true;
                }
                let _ = writer.shutdown().await;
                return false;
            }
            // the answer rides with the target's first data
            buf[0] = 0x00;
            if writer
                .write_all(&answers.record(&buf[..1 + n]))
                .await
                .is_err()
            {
                return false;
            }
            loop {
                match target_read.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if writer.write_all(&answers.record(&buf[..n])).await.is_err() {
                            return false;
                        }
                    }
                }
            }
            if reuse {
                writer.write_all(&answers.record(&[])).await.is_ok()
            } else {
                // Connect: no empty record, the close
                let _ = writer.shutdown().await;
                false
            }
        };
        let (client_ended, server_ended) = tokio::join!(requests, replies);
        client_ended && server_ended
    }
}

impl Shared {
    /// UDP on this connection until either side ends.
    async fn udp(
        &self,
        reader: &mut Reader,
        up: &mut Direction,
        writer: &mut Writer,
        answers: &mut Answers,
    ) -> io::Result<()> {
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        // a record carries a datagram of up to 16,374 bytes, which macOS does
        // not send from a socket with its default send buffer (9216 bytes)
        socket2::SockRef::from(&socket)
            .set_send_buffer_size(256 * 1024)
            .expect("a 256 KiB send buffer for the session's UDP socket");
        self.seen
            .udp_outside
            .lock()
            .expect("outside")
            .push(socket.local_addr()?);
        // the answer at once, in a record of its own: the direction's
        // first, which v4's format pads
        writer.write_all(&answers.record(&[0x00])).await?;
        let datagrams = async {
            while let Some(Record::Data { payload, .. }) =
                read_record(reader, up, &self.seen).await?
            {
                let (host, port, data) =
                    parse_datagram(&payload).ok_or_else(|| bad("a record that is no datagram"))?;
                self.seen
                    .datagrams
                    .lock()
                    .expect("datagrams")
                    .push(format!("{host}:{port}"));
                let to = match (host.parse::<IpAddr>(), self.script.connect_to) {
                    (Ok(ip), _) if is_localhost(ip) => SocketAddr::new(ip, port),
                    // nowhere else
                    (Ok(_), _) => continue,
                    (Err(_), Some(addr)) => addr,
                    // never resolves: a name without `connect_to` is a dead end
                    (Err(_), None) => continue,
                };
                socket.send_to(data, to).await?;
            }
            Ok::<(), io::Error>(())
        };
        let replies = async {
            let mut buf = vec![0u8; 65536];
            loop {
                let (n, from) = match socket.recv_from(&mut buf).await {
                    Ok(got) => got,
                    // an ICMP "unreachable" for an earlier datagram (Windows)
                    Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                    Err(e) => return Err(e),
                };
                if self.script.udp_junk {
                    writer
                        .write_all(&answers.record(&[5, 1, 2, 3, 4, 0, 53, b'x']))
                        .await?;
                    writer.write_all(&answers.record(&[4, 1, 2])).await?;
                }
                let mut datagram = match from.ip() {
                    IpAddr::V4(ip) => [&[4][..], &ip.octets()].concat(),
                    IpAddr::V6(ip) => [&[6][..], &ip.octets()].concat(),
                };
                datagram.extend_from_slice(&from.port().to_be_bytes());
                let room = MAX_PAYLOAD - datagram.len();
                datagram.extend_from_slice(&buf[..n.min(room)]);
                writer.write_all(&answers.record(&datagram)).await?;
            }
        };
        tokio::select! {
            done = datagrams => done,
            done = replies => done,
        }
    }
}

async fn serve(tcp: TcpStream, shared: Arc<Shared>, connection: usize) -> io::Result<()> {
    let script = &shared.script;
    if script.reset {
        // closing resets the connection
        tcp.set_zero_linger()?;
    }
    let mut stream: BoxedStream = Box::new(tcp);
    let obfs = match (script.obfs_tls, script.obfs_http) {
        (true, _) => Some(ObfsMode::Tls),
        (false, true) => Some(ObfsMode::Http),
        (false, false) => None,
    };
    if let Some(mode) = obfs {
        let (inner, hello) = accept_obfs(stream, mode).await?;
        shared.seen.obfs.lock().expect("obfs").push(hello);
        stream = inner;
    }
    let mut salt = [0u8; SALT];
    if !read_full(&mut stream, &mut salt).await? {
        return Ok(());
    }
    let mut up = Direction::new(script.version, &script.psk, &salt);
    let mut answers = Answers::new(script.version, &script.psk);
    let (mut reader, mut writer) = tokio::io::split(stream);
    for tunnel in 0.. {
        let (payload, padding) = match read_record(&mut reader, &mut up, &shared.seen).await {
            Ok(Some(Record::Data { payload, padding })) => (payload, padding),
            Ok(None) => return Ok(()),
            Err(_) if tunnel == 0 => {
                // a wrong PSK: no word back
                shared.seen.rejected.fetch_add(1, Ordering::SeqCst);
                if !script.reset {
                    close(&mut reader, &mut writer).await;
                }
                return Ok(());
            }
            _ => return Ok(()),
        };
        let Some(mut request) = parse_request(&payload) else {
            return Ok(());
        };
        if script.tunnels_per_connection.is_some_and(|n| tunnel >= n) {
            shared.seen.unanswered.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
        if script.garble_after.is_some_and(|n| tunnel >= n) {
            shared.seen.unanswered.fetch_add(1, Ordering::SeqCst);
            writer.write_all(&[0x5a; HEADER]).await?;
            close(&mut reader, &mut writer).await;
            return Ok(());
        }
        request.connection = connection;
        request.tunnel = tunnel;
        request.padding = padding;
        shared
            .seen
            .requests
            .lock()
            .expect("requests")
            .push(request.clone());
        if let Some((code, message)) = &script.refuse {
            writer.write_all(&answers.error(*code, message)).await?;
            close(&mut reader, &mut writer).await;
            return Ok(());
        }
        if request.command == UDP {
            let _ = shared
                .udp(&mut reader, &mut up, &mut writer, &mut answers)
                .await;
            close(&mut reader, &mut writer).await;
            return Ok(());
        }
        let next = shared
            .tunnel(&request, &mut reader, &mut up, &mut writer, &mut answers)
            .await;
        if !next {
            close(&mut reader, &mut writer).await;
            return Ok(());
        }
    }
    Ok(())
}

impl FakeSnell {
    pub async fn spawn(script: SnellScript) -> FakeSnell {
        assert!((1..=5).contains(&script.version), "a version from 1 to 5");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let shared = Arc::new(Shared {
            script,
            seen: Seen::default(),
        });
        let serving = shared.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let connection = serving.seen.connections.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(serve(tcp, serving.clone(), connection));
            }
        });
        FakeSnell {
            addr,
            shared,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn requests(&self) -> Vec<RecordedSnell> {
        self.shared.seen.requests.lock().expect("requests").clone()
    }

    /// What each connection's camouflage said, when the script expects one.
    pub fn obfs_seen(&self) -> Vec<ObfsHello> {
        self.shared.seen.obfs.lock().expect("obfs").clone()
    }

    /// TCP connections accepted so far.
    pub fn connections(&self) -> usize {
        self.shared.seen.connections.load(Ordering::SeqCst)
    }

    /// Connections given no answer because they did not decrypt.
    pub fn rejected(&self) -> usize {
        self.shared.seen.rejected.load(Ordering::SeqCst)
    }

    /// Requests closed without an answer (`tunnels_per_connection`,
    /// `garble_after`).
    pub fn unanswered(&self) -> usize {
        self.shared.seen.unanswered.load(Ordering::SeqCst)
    }

    /// Every UDP datagram's target, `host:port`, in arrival order.
    pub fn datagrams(&self) -> Vec<String> {
        self.shared
            .seen
            .datagrams
            .lock()
            .expect("datagrams")
            .clone()
    }

    /// Where each UDP connection sends from: a datagram to one of these goes
    /// back to its client.
    pub fn udp_outside(&self) -> Vec<SocketAddr> {
        self.shared
            .seen
            .udp_outside
            .lock()
            .expect("outside")
            .clone()
    }

    /// The longest payload of any client record so far.
    pub fn largest_record(&self) -> usize {
        self.shared.seen.largest_record.load(Ordering::SeqCst)
    }
}
