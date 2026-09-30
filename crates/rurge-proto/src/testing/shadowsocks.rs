//! A scriptable Shadowsocks server: optionally simple-obfs in front, then
//! the AEAD stream (or, with `none`, the bytes as they are), the request
//! header, and a relay. Its framing, nonce counting and address parsing are
//! written apart from the client's (`crate::shadowsocks`), so each checks the
//! other; only the primitives (the ciphers and the key derivation, which
//! have vectors of their own) are shared. It never resolves a name.
//!
//! As real servers do, it never tells a client with a wrong password so:
//! it stops reading into the stream, closes its side and waits for the
//! client to go away.

use super::AbortOnDrop;
use super::obfs::{ObfsHello, accept_obfs};
use crate::shadowsocks::cipher::{AeadCipher, AeadKind, TAG};
use crate::shadowsocks::kdf;
use rurge_config::spec::{ObfsMode, SsMethod};
use rurge_net::connector::BoxedStream;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The largest payload a client chunk may carry.
const MAX_CHUNK: usize = 0x3FFF;

#[derive(Clone, Debug)]
pub struct ShadowsocksScript {
    pub method: SsMethod,
    pub password: String,
    /// Expect this simple-obfs camouflage in front of the protocol.
    pub obfs: Option<ObfsMode>,
    /// Relay here whatever the client asked for (needed for a domain target).
    pub connect_to: Option<SocketAddr>,
}

impl ShadowsocksScript {
    pub fn new(method: SsMethod, password: &str) -> ShadowsocksScript {
        ShadowsocksScript {
            method,
            password: password.to_string(),
            obfs: None,
            connect_to: None,
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
    /// the read that completed the address).
    pub early: Vec<u8>,
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
        Direction {
            cipher: AeadCipher::new(kind, &kdf::session_subkey(&master, salt)),
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
    if len > MAX_CHUNK {
        return Err(bad("a chunk over 0x3FFF bytes"));
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
    let first = match read_chunk(&mut stream, &mut up, &shared.seen).await {
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
        while let Ok(Some(payload)) = read_chunk(&mut client_read, &mut up, &shared.seen).await {
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
