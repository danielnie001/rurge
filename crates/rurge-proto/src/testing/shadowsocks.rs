//! A scriptable Shadowsocks server: optionally simple-obfs in front, then
//! the AEAD stream (or, with `none`, the bytes as they are; or SS 2022 with
//! its headers and, for several users, identity headers), the request
//! header, and a relay. Its framing, nonce counting, headers and address
//! parsing are written apart from the client's (`crate::shadowsocks`), so
//! each checks the other; only the primitives (the ciphers and the key
//! derivations, which have vectors of their own) are shared. It never
//! resolves a name.
//!
//! As real servers do, it never tells a client with a wrong password (or
//! key, or clock, or a replayed salt) so: it stops reading into the stream,
//! closes its side and waits for the client to go away.
//!
//! UDP: a loopback socket on the TCP port's number (or, with `udp_apart`, a
//! port of its own). Each client session (SS 2022: its session id; else the
//! client's address) relays through a socket of its own, and whatever
//! reaches that socket goes back with its source — full cone, as the
//! reference servers do. Packets it cannot use are dropped and counted.

use super::AbortOnDrop;
use super::obfs::{ObfsHello, accept_obfs};
use crate::shadowsocks::cipher::{AeadCipher, AeadKind, TAG, aes_decrypt_block, aes_encrypt_block};
use crate::shadowsocks::kdf;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rurge_config::spec::{ObfsMode, SsMethod};
use rurge_net::connector::BoxedStream;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

/// The largest payload a client chunk may carry (AEAD, SS 2022).
const MAX_CHUNK: usize = 0x3FFF;
const MAX_CHUNK_2022: usize = 0xFFFF;

#[derive(Clone, Debug)]
pub struct ShadowsocksScript {
    pub method: SsMethod,
    /// The password; SS 2022: the key in Base64, or with `users` the
    /// server's identity key.
    pub password: String,
    /// SS 2022 with identity headers: the users' keys in Base64. Empty: a
    /// single-user server, no identity header.
    pub users: Vec<String>,
    /// Expect this simple-obfs camouflage in front of the protocol.
    pub obfs: Option<ObfsMode>,
    /// Relay here whatever the client asked for (needed for a domain target).
    pub connect_to: Option<SocketAddr>,
    /// SS 2022: seconds added to the timestamp of every answer.
    pub answer_skew: i64,
    /// SS 2022: answer naming a salt (UDP: a client session) that is not
    /// the request's.
    pub wrong_request_salt: bool,
    /// UDP on a port of its own rather than on the TCP port's number.
    pub udp_apart: bool,
    /// UDP: every answer goes out twice (SS 2022: the copy is a replay).
    pub udp_twice: bool,
    /// UDP: every answer goes after a copy of it with a bit flipped (with
    /// `none`, a different answer).
    pub udp_garbled_first: bool,
}

impl ShadowsocksScript {
    pub fn new(method: SsMethod, password: &str) -> ShadowsocksScript {
        ShadowsocksScript {
            method,
            password: password.to_string(),
            users: Vec::new(),
            obfs: None,
            connect_to: None,
            answer_skew: 0,
            wrong_request_salt: false,
            udp_apart: false,
            udp_twice: false,
            udp_garbled_first: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedShadowsocks {
    /// The client's salt; empty for `none`.
    pub salt: Vec<u8>,
    pub atyp: u8,
    /// An IP literal, or the name exactly as it was on the wire.
    pub host: String,
    pub port: u16,
    /// Payload after the address: the rest of the first chunk (`none`: of
    /// the read that completed the address; SS 2022: after the padding).
    pub early: Vec<u8>,
    /// SS 2022: the request's padding length.
    pub padding: usize,
    /// SS 2022 with users: which one's key the identity header named.
    pub user: Option<usize>,
}

/// A client datagram the fake relayed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedDatagram {
    /// `host:port`, the name as it was on the wire.
    pub target: String,
    pub payload: Vec<u8>,
    /// SS 2022: the client's session id and the packet's id.
    pub session: Option<(u64, u64)>,
    /// SS 2022: the packet's padding length.
    pub padding: usize,
    /// SS 2022 with users: which one's key the identity header named.
    pub user: Option<usize>,
}

pub struct FakeShadowsocks {
    addr: SocketAddr,
    udp_addr: SocketAddr,
    shared: Arc<Shared>,
    connections: Arc<AtomicUsize>,
    _task: AbortOnDrop,
    _udp: AbortOnDrop,
}

#[derive(Default)]
struct Seen {
    requests: Mutex<Vec<RecordedShadowsocks>>,
    obfs: Mutex<Vec<ObfsHello>>,
    answer_salts: Mutex<Vec<Vec<u8>>>,
    rejected: AtomicUsize,
    largest_chunk: AtomicUsize,
    /// SS 2022: every request salt accepted so far, to refuse replays.
    salts: Mutex<HashSet<Vec<u8>>>,
    datagrams: Mutex<Vec<RecordedDatagram>>,
    /// Each UDP client session's own socket, in the order they opened.
    outside: Mutex<Vec<SocketAddr>>,
    udp_rejected: AtomicUsize,
}

struct Shared {
    script: ShadowsocksScript,
    seen: Seen,
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("fake ss: {what}"))
}

/// One direction's cipher, its nonce a little-endian counter from zero.
struct Direction {
    cipher: AeadCipher,
    count: u128,
    nonce_len: usize,
}

impl Direction {
    fn new(kind: AeadKind, password: &str, salt: &[u8]) -> Direction {
        let master = kdf::evp_bytes_to_key(password.as_bytes(), kind.key_len());
        Direction::keyed(kind, &kdf::session_subkey(&master, salt))
    }

    fn new_2022(kind: AeadKind, psk: &[u8], salt: &[u8]) -> Direction {
        Direction::keyed(kind, &kdf::session_subkey_2022(psk, salt))
    }

    fn keyed(kind: AeadKind, session_key: &[u8]) -> Direction {
        Direction {
            cipher: AeadCipher::new(kind, session_key),
            count: 0,
            nonce_len: kind.nonce_len(),
        }
    }

    fn nonce(&mut self) -> [u8; 24] {
        let mut nonce = [0u8; 24];
        nonce[..16].copy_from_slice(&self.count.to_le_bytes());
        self.count += 1;
        nonce
    }

    fn seal(&mut self, plain: &[u8], out: &mut Vec<u8>) {
        let nonce = self.nonce();
        let mut data = plain.to_vec();
        let tag = self
            .cipher
            .seal_in_place(&nonce[..self.nonce_len], &mut data);
        out.extend_from_slice(&data);
        out.extend_from_slice(&tag);
    }

    fn open(&mut self, sealed: &[u8]) -> Option<Vec<u8>> {
        let nonce = self.nonce();
        let (data, tag) = sealed.split_at(sealed.len().checked_sub(TAG)?);
        let mut data = data.to_vec();
        let tag: [u8; TAG] = tag.try_into().ok()?;
        self.cipher
            .open_in_place(&nonce[..self.nonce_len], &mut data, &tag)
            .then_some(data)
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

/// The next chunk's payload; `Ok(None)` when the client closed between
/// chunks. An error when it does not authenticate or is too long.
async fn read_chunk<R: AsyncRead + Unpin>(
    r: &mut R,
    up: &mut Direction,
    max: usize,
    seen: &Seen,
) -> io::Result<Option<Vec<u8>>> {
    let mut sealed_len = [0u8; 2 + TAG];
    if !read_full(r, &mut sealed_len).await? {
        return Ok(None);
    }
    let len = up
        .open(&sealed_len)
        .ok_or_else(|| bad("a length that does not authenticate"))?;
    let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
    if len > max {
        return Err(bad("a chunk over the largest payload"));
    }
    seen.largest_chunk.fetch_max(len, Ordering::SeqCst);
    let mut body = vec![0u8; len + TAG];
    r.read_exact(&mut body).await?;
    up.open(&body)
        .map(Some)
        .ok_or_else(|| bad("a payload that does not authenticate"))
}

/// `ATYP ADDR PORT` at the start of `buf`: the request and the bytes it
/// took, once whole.
fn parse_address(buf: &[u8]) -> Option<(RecordedShadowsocks, usize)> {
    let atyp = *buf.first()?;
    let (host, used) = match atyp {
        1 => {
            let b: [u8; 4] = buf.get(1..5)?.try_into().ok()?;
            (IpAddr::V4(Ipv4Addr::from(b)).to_string(), 5)
        }
        4 => {
            let b: [u8; 16] = buf.get(1..17)?.try_into().ok()?;
            (IpAddr::V6(Ipv6Addr::from(b)).to_string(), 17)
        }
        3 => {
            let len = usize::from(*buf.get(1)?);
            let name = buf.get(2..2 + len)?;
            (String::from_utf8_lossy(name).into_owned(), 2 + len)
        }
        _ => return None,
    };
    let port = u16::from_be_bytes(buf.get(used..used + 2)?.try_into().ok()?);
    Some((
        RecordedShadowsocks {
            salt: Vec::new(),
            atyp,
            host,
            port,
            early: buf[used + 2..].to_vec(),
            padding: 0,
            user: None,
        },
        used + 2,
    ))
}

impl Shared {
    fn record(&self, request: &RecordedShadowsocks) {
        self.seen
            .requests
            .lock()
            .expect("requests")
            .push(request.clone());
    }

    /// Where to relay a request; `None`: a name without `connect_to`.
    fn upstream(&self, request: &RecordedShadowsocks) -> Option<SocketAddr> {
        self.script.connect_to.or_else(|| {
            let ip = request.host.parse::<IpAddr>().ok()?;
            Some(SocketAddr::new(ip, request.port))
        })
    }

    /// A client it cannot understand: no word back, only the close.
    async fn reject(&self, mut stream: BoxedStream) -> io::Result<()> {
        self.seen.rejected.fetch_add(1, Ordering::SeqCst);
        stream.shutdown().await?;
        // read until the client goes: closing with unread data would reset
        let mut sink = [0u8; 4096];
        while stream.read(&mut sink).await? > 0 {}
        Ok(())
    }
}

async fn serve(tcp: TcpStream, shared: Arc<Shared>) -> io::Result<()> {
    let mut stream: BoxedStream = Box::new(tcp);
    if let Some(mode) = shared.script.obfs {
        let (inner, hello) = accept_obfs(stream, mode).await?;
        shared.seen.obfs.lock().expect("obfs").push(hello);
        stream = inner;
    }
    match AeadKind::of(shared.script.method) {
        None => serve_plain(stream, &shared).await,
        Some(kind) if shared.script.method.is_2022() => serve_2022(stream, kind, &shared).await,
        Some(kind) => serve_aead(stream, kind, &shared).await,
    }
}

async fn serve_plain(mut stream: BoxedStream, shared: &Shared) -> io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let request = loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some((request, _)) = parse_address(&buf) {
            break request;
        }
    };
    shared.record(&request);
    let Some(to) = shared.upstream(&request) else {
        return stream.shutdown().await;
    };
    let mut upstream = TcpStream::connect(to).await?;
    upstream.write_all(&request.early).await?;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
    Ok(())
}

async fn serve_aead(mut stream: BoxedStream, kind: AeadKind, shared: &Shared) -> io::Result<()> {
    let password = shared.script.password.as_str();
    let mut salt = vec![0u8; kind.key_len()];
    stream.read_exact(&mut salt).await?;
    let mut up = Direction::new(kind, password, &salt);
    let first = match read_chunk(&mut stream, &mut up, MAX_CHUNK, &shared.seen).await {
        Ok(Some(first)) => first,
        Ok(None) => return Ok(()),
        Err(_) => return shared.reject(stream).await,
    };
    // the address rides in the first chunk
    let Some((mut request, _)) = parse_address(&first) else {
        return shared.reject(stream).await;
    };
    request.salt = salt;
    shared.record(&request);
    let Some(to) = shared.upstream(&request) else {
        return stream.shutdown().await;
    };
    let mut upstream = TcpStream::connect(to).await?;
    upstream.write_all(&request.early).await?;
    let (mut client_read, mut client_write) = tokio::io::split(stream);
    let (mut target_read, mut target_write) = upstream.into_split();
    let requests = async {
        while let Ok(Some(payload)) =
            read_chunk(&mut client_read, &mut up, MAX_CHUNK, &shared.seen).await
        {
            if target_write.write_all(&payload).await.is_err() {
                return;
            }
        }
        let _ = target_write.shutdown().await;
    };
    let answers = async {
        let mut answer_salt = vec![0u8; kind.key_len()];
        getrandom::fill(&mut answer_salt).expect("randomness");
        let mut down = Direction::new(kind, password, &answer_salt);
        // the salt goes out with the first payload, never alone
        let mut pending_salt = Some(answer_salt);
        let mut buf = vec![0u8; MAX_CHUNK];
        loop {
            let n = match target_read.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut frame = Vec::with_capacity(n + 64);
            if let Some(salt) = pending_salt.take() {
                shared
                    .seen
                    .answer_salts
                    .lock()
                    .expect("salts")
                    .push(salt.clone());
                frame.extend_from_slice(&salt);
            }
            down.seal(&(n as u16).to_be_bytes(), &mut frame);
            down.seal(&buf[..n], &mut frame);
            if client_write.write_all(&frame).await.is_err() {
                return;
            }
        }
        let _ = client_write.shutdown().await;
    };
    tokio::join!(requests, answers);
    Ok(())
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after 1970")
        .as_secs()
}

fn decode_key(text: &str) -> Vec<u8> {
    STANDARD.decode(text).expect("a Base64 key in the script")
}

/// A 2022 request's variable-length header: the address, the padding
/// behind its length, the initial payload. `None` when malformed, or when
/// it has neither padding nor payload (the specification's rule).
fn parse_variable(buf: &[u8]) -> Option<RecordedShadowsocks> {
    let (mut request, used) = parse_address(buf)?;
    let rest = &buf[used..];
    let padding = usize::from(u16::from_be_bytes(rest.get(..2)?.try_into().ok()?));
    let payload = rest.get(2 + padding..)?;
    if padding == 0 && payload.is_empty() {
        return None;
    }
    request.padding = padding;
    request.early = payload.to_vec();
    Some(request)
}

/// SS 2022 (SIP022 3.1, SIP023): salt, identity header when the script has
/// users, the fixed-length header chunk (type 0, a timestamp within 30
/// seconds, a salt never seen), the variable-length one; the answer opens
/// with its own header chunk.
async fn serve_2022(mut stream: BoxedStream, kind: AeadKind, shared: &Shared) -> io::Result<()> {
    let script = &shared.script;
    let mut salt = vec![0u8; kind.key_len()];
    stream.read_exact(&mut salt).await?;
    let server_key = decode_key(&script.password);
    let (psk, user) = if script.users.is_empty() {
        (server_key, None)
    } else {
        let mut block = [0u8; 16];
        stream.read_exact(&mut block).await?;
        aes_decrypt_block(&kdf::identity_subkey(&server_key, &salt), &mut block);
        let users: Vec<Vec<u8>> = script.users.iter().map(|u| decode_key(u)).collect();
        match users.iter().position(|u| kdf::identity_hash(u) == block) {
            Some(i) => (users[i].clone(), Some(i)),
            None => return shared.reject(stream).await,
        }
    };
    let mut up = Direction::new_2022(kind, &psk, &salt);
    let mut fixed = [0u8; 1 + 8 + 2 + TAG];
    stream.read_exact(&mut fixed).await?;
    let Some(fixed) = up.open(&fixed) else {
        return shared.reject(stream).await;
    };
    let time = u64::from_be_bytes(fixed[1..9].try_into().expect("8 bytes"));
    if fixed[0] != 0 || time.abs_diff(unix_time()) > 30 {
        return shared.reject(stream).await;
    }
    if !shared
        .seen
        .salts
        .lock()
        .expect("salts")
        .insert(salt.clone())
    {
        return shared.reject(stream).await;
    }
    let len = usize::from(u16::from_be_bytes([fixed[9], fixed[10]]));
    let mut variable = vec![0u8; len + TAG];
    stream.read_exact(&mut variable).await?;
    let Some(mut request) = up.open(&variable).as_deref().and_then(parse_variable) else {
        return shared.reject(stream).await;
    };
    request.salt = salt.clone();
    request.user = user;
    shared.record(&request);
    let Some(to) = shared.upstream(&request) else {
        return stream.shutdown().await;
    };
    let mut upstream = TcpStream::connect(to).await?;
    upstream.write_all(&request.early).await?;
    let (mut client_read, mut client_write) = tokio::io::split(stream);
    let (mut target_read, mut target_write) = upstream.into_split();
    let requests = async {
        while let Ok(Some(payload)) =
            read_chunk(&mut client_read, &mut up, MAX_CHUNK_2022, &shared.seen).await
        {
            if target_write.write_all(&payload).await.is_err() {
                return;
            }
        }
        let _ = target_write.shutdown().await;
    };
    let answers = async {
        let mut answer_salt = vec![0u8; kind.key_len()];
        getrandom::fill(&mut answer_salt).expect("randomness");
        let mut down = Direction::new_2022(kind, &psk, &answer_salt);
        let mut echoed = salt.clone();
        if script.wrong_request_salt {
            echoed[0] ^= 1;
        }
        // the salt and the header go out with the first payload, never alone
        let mut pending_salt = Some(answer_salt);
        let mut buf = vec![0u8; MAX_CHUNK_2022];
        loop {
            let n = match target_read.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let len = (n as u16).to_be_bytes();
            let mut frame = Vec::with_capacity(n + 128);
            if let Some(salt) = pending_salt.take() {
                shared
                    .seen
                    .answer_salts
                    .lock()
                    .expect("salts")
                    .push(salt.clone());
                frame.extend_from_slice(&salt);
                let time = unix_time().saturating_add_signed(script.answer_skew);
                let mut header = vec![1];
                header.extend_from_slice(&time.to_be_bytes());
                header.extend_from_slice(&echoed);
                header.extend_from_slice(&len);
                down.seal(&header, &mut frame);
            } else {
                down.seal(&len, &mut frame);
            }
            down.seal(&buf[..n], &mut frame);
            if client_write.write_all(&frame).await.is_err() {
                return;
            }
        }
        let _ = client_write.shutdown().await;
    };
    tokio::join!(requests, answers);
    Ok(())
}

/// A TCP listener and a UDP socket on the same port number, or, `apart`, on
/// different ones.
async fn bind(apart: bool) -> (TcpListener, UdpSocket) {
    for _ in 0..64 {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        let udp = if apart {
            UdpSocket::bind("127.0.0.1:0")
                .await
                .ok()
                .filter(|udp| udp.local_addr().is_ok_and(|a| a.port() != port))
        } else {
            UdpSocket::bind(("127.0.0.1", port)).await.ok()
        };
        if let Some(udp) = udp {
            return (listener, udp);
        }
    }
    panic!("no loopback port for both TCP and UDP");
}

/// Which client session a datagram belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ClientKey {
    Addr(SocketAddr),
    Session(u64),
}

/// What it takes to answer one client session.
#[derive(Clone)]
enum Answering {
    Plain,
    Aead(AeadKind),
    S2022 {
        kind: AeadKind,
        user: Vec<u8>,
        client: [u8; 8],
    },
}

/// A client datagram, opened.
struct Opened {
    key: ClientKey,
    answering: Answering,
    record: RecordedDatagram,
    host: String,
    port: u16,
}

/// `sealed` (its tag last) opened with `cipher` under `nonce`.
fn open_sealed(cipher: &AeadCipher, nonce: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let (data, tag) = sealed.split_at(sealed.len().checked_sub(TAG)?);
    let mut data = data.to_vec();
    let tag: [u8; TAG] = tag.try_into().ok()?;
    cipher.open_in_place(nonce, &mut data, &tag).then_some(data)
}

/// `plain` sealed with `cipher` under `nonce`, its tag last.
fn seal_plain(cipher: &AeadCipher, nonce: &[u8], plain: &[u8]) -> Vec<u8> {
    let mut data = plain.to_vec();
    let tag = cipher.seal_in_place(nonce, &mut data);
    data.extend_from_slice(&tag);
    data
}

/// The target and the payload of a datagram's plaintext.
fn target_of(plain: &[u8]) -> Option<(String, u16, Vec<u8>)> {
    let (request, _) = parse_address(plain)?;
    Some((request.host, request.port, request.early))
}

/// A client datagram from `from`; `None` for one it cannot use.
fn open_datagram(script: &ShadowsocksScript, packet: &[u8], from: SocketAddr) -> Option<Opened> {
    let (plain, answering) = match AeadKind::of(script.method) {
        None => (packet.to_vec(), Answering::Plain),
        Some(kind) if script.method.is_2022() => return open_datagram_2022(script, kind, packet),
        Some(kind) => {
            let salt = packet.get(..kind.key_len())?;
            let master = kdf::evp_bytes_to_key(script.password.as_bytes(), kind.key_len());
            let cipher = AeadCipher::new(kind, &kdf::session_subkey(&master, salt));
            let nonce = [0u8; 24];
            let plain = open_sealed(&cipher, &nonce[..kind.nonce_len()], &packet[salt.len()..])?;
            (plain, Answering::Aead(kind))
        }
    };
    let (host, port, payload) = target_of(&plain)?;
    Some(Opened {
        key: ClientKey::Addr(from),
        answering,
        record: RecordedDatagram {
            target: format!("{host}:{port}"),
            payload,
            session: None,
            padding: 0,
            user: None,
        },
        host,
        port,
    })
}

/// SS 2022 (SIP022 3.2, SIP023): the separate header under the server's
/// key, an identity header when the script has users, the body.
fn open_datagram_2022(script: &ShadowsocksScript, kind: AeadKind, packet: &[u8]) -> Option<Opened> {
    let server_key = decode_key(&script.password);
    let mut separate: [u8; 16] = packet.get(..16)?.try_into().ok()?;
    aes_decrypt_block(&server_key, &mut separate);
    let (psk, user, body_at) = if script.users.is_empty() {
        (server_key, None, 16)
    } else {
        let mut block: [u8; 16] = packet.get(16..32)?.try_into().ok()?;
        aes_decrypt_block(&server_key, &mut block);
        for (byte, mask) in block.iter_mut().zip(separate) {
            *byte ^= mask;
        }
        let users: Vec<Vec<u8>> = script.users.iter().map(|u| decode_key(u)).collect();
        let i = users.iter().position(|u| kdf::identity_hash(u) == block)?;
        (users[i].clone(), Some(i), 32)
    };
    let client: [u8; 8] = separate[..8].try_into().ok()?;
    let cipher = AeadCipher::new(kind, &kdf::session_subkey_2022(&psk, &client));
    let body = open_sealed(&cipher, &separate[4..], packet.get(body_at..)?)?;
    let time = u64::from_be_bytes(body.get(1..9)?.try_into().ok()?);
    if body[0] != 0 || time.abs_diff(unix_time()) > 30 {
        return None;
    }
    let padding = usize::from(u16::from_be_bytes(body.get(9..11)?.try_into().ok()?));
    let (host, port, payload) = target_of(body.get(11 + padding..)?)?;
    let session = u64::from_be_bytes(client);
    Some(Opened {
        key: ClientKey::Session(session),
        answering: Answering::S2022 {
            kind,
            user: psk,
            client,
        },
        record: RecordedDatagram {
            target: format!("{host}:{port}"),
            payload,
            session: Some((session, u64::from_be_bytes(separate[8..].try_into().ok()?))),
            padding,
            user,
        },
        host,
        port,
    })
}

/// `from` as `ATYP ADDR PORT`.
fn socks_address(from: SocketAddr) -> Vec<u8> {
    let mut out = Vec::with_capacity(19);
    match from.ip() {
        IpAddr::V4(ip) => {
            out.push(1);
            out.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            out.push(4);
            out.extend_from_slice(&ip.octets());
        }
    }
    out.extend_from_slice(&from.port().to_be_bytes());
    out
}

/// The answer `payload` from `from`: packet `packet` of server session
/// `server` (SS 2022).
fn seal_answer(
    script: &ShadowsocksScript,
    answering: &Answering,
    server: &[u8; 8],
    packet: u64,
    from: SocketAddr,
    payload: &[u8],
) -> Vec<u8> {
    let addr = socks_address(from);
    match answering {
        Answering::Plain => [&addr[..], payload].concat(),
        Answering::Aead(kind) => {
            let mut salt = vec![0u8; kind.key_len()];
            getrandom::fill(&mut salt).expect("randomness");
            let master = kdf::evp_bytes_to_key(script.password.as_bytes(), kind.key_len());
            let cipher = AeadCipher::new(*kind, &kdf::session_subkey(&master, &salt));
            let nonce = [0u8; 24];
            let sealed = seal_plain(
                &cipher,
                &nonce[..kind.nonce_len()],
                &[&addr[..], payload].concat(),
            );
            [salt, sealed].concat()
        }
        Answering::S2022 { kind, user, client } => {
            let mut separate = [0u8; 16];
            separate[..8].copy_from_slice(server);
            separate[8..].copy_from_slice(&packet.to_be_bytes());
            let mut echoed = *client;
            if script.wrong_request_salt {
                echoed[0] ^= 1;
            }
            let time = unix_time().saturating_add_signed(script.answer_skew);
            let mut body = vec![1];
            body.extend_from_slice(&time.to_be_bytes());
            body.extend_from_slice(&echoed);
            body.extend_from_slice(&[0, 0]);
            body.extend_from_slice(&addr);
            body.extend_from_slice(payload);
            let cipher = AeadCipher::new(*kind, &kdf::session_subkey_2022(user, server));
            let sealed = seal_plain(&cipher, &separate[4..], &body);
            // the answers' separate headers are under the user's key
            aes_encrypt_block(user, &mut separate);
            [&separate[..], &sealed].concat()
        }
    }
}

/// One client session's relay.
struct UdpClient {
    outside: Arc<UdpSocket>,
    /// Where the client last sent from: the answers go there.
    client: Arc<Mutex<SocketAddr>>,
    /// SS 2022: the packet ids seen.
    packets: HashSet<u64>,
    _answers: AbortOnDrop,
}

impl UdpClient {
    async fn open(
        socket: &Arc<UdpSocket>,
        shared: &Arc<Shared>,
        answering: Answering,
        from: SocketAddr,
    ) -> io::Result<UdpClient> {
        let outside = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
        shared
            .seen
            .outside
            .lock()
            .expect("outside")
            .push(outside.local_addr()?);
        let client = Arc::new(Mutex::new(from));
        let task = tokio::spawn(answer(
            socket.clone(),
            outside.clone(),
            client.clone(),
            shared.clone(),
            answering,
        ));
        Ok(UdpClient {
            outside,
            client,
            packets: HashSet::new(),
            _answers: AbortOnDrop(task),
        })
    }
}

/// Sends whatever reaches `outside`, from anyone, to the client.
async fn answer(
    socket: Arc<UdpSocket>,
    outside: Arc<UdpSocket>,
    client: Arc<Mutex<SocketAddr>>,
    shared: Arc<Shared>,
    answering: Answering,
) {
    let script = &shared.script;
    let mut server = [0u8; 8];
    getrandom::fill(&mut server).expect("randomness");
    let mut packet = 0u64;
    let mut buf = vec![0u8; 65536];
    loop {
        let (n, from) = match outside.recv_from(&mut buf).await {
            Ok(got) => got,
            // an ICMP "unreachable" for an earlier datagram (Windows)
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(_) => return,
        };
        let sealed = seal_answer(script, &answering, &server, packet, from, &buf[..n]);
        packet += 1;
        let to = *client.lock().expect("client");
        if script.udp_garbled_first {
            let mut garbled = sealed.clone();
            let last = garbled.len() - 1;
            garbled[last] ^= 1;
            let _ = socket.send_to(&garbled, to).await;
        }
        let _ = socket.send_to(&sealed, to).await;
        if script.udp_twice {
            let _ = socket.send_to(&sealed, to).await;
        }
    }
}

async fn serve_udp(socket: Arc<UdpSocket>, shared: Arc<Shared>) {
    let mut clients: HashMap<ClientKey, UdpClient> = HashMap::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(got) => got,
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(_) => return,
        };
        let seen = &shared.seen;
        let Some(opened) = open_datagram(&shared.script, &buf[..n], from) else {
            seen.udp_rejected.fetch_add(1, Ordering::SeqCst);
            continue;
        };
        let client = match clients.entry(opened.key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let open = UdpClient::open(&socket, &shared, opened.answering, from);
                let Ok(client) = open.await else {
                    continue;
                };
                entry.insert(client)
            }
        };
        if let Some((_, packet)) = opened.record.session
            && !client.packets.insert(packet)
        {
            seen.udp_rejected.fetch_add(1, Ordering::SeqCst);
            continue;
        }
        *client.client.lock().expect("client") = from;
        seen.datagrams
            .lock()
            .expect("datagrams")
            .push(opened.record.clone());
        let to = match (opened.host.parse::<IpAddr>(), shared.script.connect_to) {
            (Ok(ip), _) => SocketAddr::new(ip, opened.port),
            (Err(_), Some(addr)) => addr,
            // never resolves: a name without `connect_to` is a dead end
            (Err(_), None) => continue,
        };
        let _ = client.outside.send_to(&opened.record.payload, to).await;
    }
}

impl FakeShadowsocks {
    pub async fn spawn(script: ShadowsocksScript) -> FakeShadowsocks {
        let (listener, udp) = bind(script.udp_apart).await;
        let addr = listener.local_addr().expect("local addr");
        let udp_addr = udp.local_addr().expect("local addr");
        let shared = Arc::new(Shared {
            script,
            seen: Seen::default(),
        });
        let connections = Arc::new(AtomicUsize::new(0));
        let (count, serving) = (connections.clone(), shared.clone());
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(serve(tcp, serving.clone()));
            }
        });
        let udp_task = tokio::spawn(serve_udp(Arc::new(udp), shared.clone()));
        FakeShadowsocks {
            addr,
            udp_addr,
            shared,
            connections,
            _task: AbortOnDrop(task),
            _udp: AbortOnDrop(udp_task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Where it takes UDP: the TCP port's number unless `udp_apart`.
    pub fn udp_addr(&self) -> SocketAddr {
        self.udp_addr
    }

    /// Every client datagram relayed so far.
    pub fn datagrams(&self) -> Vec<RecordedDatagram> {
        self.shared
            .seen
            .datagrams
            .lock()
            .expect("datagrams")
            .clone()
    }

    /// Each UDP client session's own socket: whatever reaches it goes back
    /// to that client.
    pub fn udp_outside(&self) -> Vec<SocketAddr> {
        self.shared.seen.outside.lock().expect("outside").clone()
    }

    /// Client datagrams dropped: not decrypted, malformed, off the clock or
    /// replayed.
    pub fn udp_rejected(&self) -> usize {
        self.shared.seen.udp_rejected.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<RecordedShadowsocks> {
        self.shared.seen.requests.lock().expect("requests").clone()
    }

    /// What each connection's camouflage said, when the script expects one.
    pub fn obfs_seen(&self) -> Vec<ObfsHello> {
        self.shared.seen.obfs.lock().expect("obfs").clone()
    }

    /// The salts of the server's own streams, in the order they started.
    pub fn answer_salts(&self) -> Vec<Vec<u8>> {
        self.shared.seen.answer_salts.lock().expect("salts").clone()
    }

    /// The longest payload of any client chunk so far.
    pub fn largest_chunk(&self) -> usize {
        self.shared.seen.largest_chunk.load(Ordering::SeqCst)
    }

    /// TCP connections accepted so far.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Connections given no answer because they did not decrypt.
    pub fn rejected(&self) -> usize {
        self.shared.seen.rejected.load(Ordering::SeqCst)
    }
}
