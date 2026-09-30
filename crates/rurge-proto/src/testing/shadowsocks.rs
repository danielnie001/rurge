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

use super::AbortOnDrop;
use super::obfs::{ObfsHello, accept_obfs};
use crate::shadowsocks::cipher::{AeadCipher, AeadKind, TAG, aes_decrypt_block};
use crate::shadowsocks::kdf;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rurge_config::spec::{ObfsMode, SsMethod};
use rurge_net::connector::BoxedStream;
use std::collections::HashSet;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

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
    /// SS 2022: answer naming a salt that is not the request's.
    pub wrong_request_salt: bool,
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

pub struct FakeShadowsocks {
    addr: SocketAddr,
    shared: Arc<Shared>,
    connections: Arc<AtomicUsize>,
    _task: AbortOnDrop,
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

impl FakeShadowsocks {
    pub async fn spawn(script: ShadowsocksScript) -> FakeShadowsocks {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
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
        FakeShadowsocks {
            addr,
            shared,
            connections,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
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
