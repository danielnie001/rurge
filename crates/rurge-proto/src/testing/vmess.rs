//! A scriptable VMess AEAD server: optionally TLS, optionally a WebSocket
//! below the protocol, the sealed request head, then a chunked relay. It never
//! resolves a name.

use super::ws::{RecordedWs, accept_bytes};
use super::{AbortOnDrop, TlsFixture};
use crate::vmess::chunk::ChunkCipher;
use crate::vmess::header::{self, Security, Session, TAG};
use crate::vmess::kdf::{self, kdf, kdf16};
use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, KeyInit};
use ring::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use rurge_net::connector::BoxedStream;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Default)]
pub struct VmessScript {
    pub uuid: [u8; 16],
    /// Expect a WebSocket handshake before the request head.
    pub ws: bool,
    /// Relay here whatever the client asked for (needed for a domain target).
    pub connect_to: Option<SocketAddr>,
}

impl VmessScript {
    /// `id` in the usual 8-4-4-4-12 form.
    pub fn new(id: &str) -> VmessScript {
        let hex: Vec<u8> = id.bytes().filter(|b| *b != b'-').collect();
        let mut uuid = [0u8; 16];
        for (i, pair) in hex.chunks(2).enumerate() {
            let text = std::str::from_utf8(pair).expect("ascii");
            uuid[i] = u8::from_str_radix(text, 16).expect("a hex id");
        }
        VmessScript {
            uuid,
            ..VmessScript::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedVmess {
    pub command: u8,
    pub options: u8,
    pub security: u8,
    pub padding: usize,
    pub atyp: u8,
    /// An IP literal, or the name exactly as it was on the wire.
    pub host: String,
    pub port: u16,
    /// The AuthID's timestamp minus the server's clock, in seconds.
    pub skew: i64,
    /// Bytes that arrived in the same read as the end of the head.
    pub early: usize,
}

pub struct FakeVmess {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedVmess>>>,
    ws_seen: Arc<Mutex<Vec<RecordedWs>>>,
    connections: Arc<AtomicUsize>,
    rejected: Arc<AtomicUsize>,
    _task: AbortOnDrop,
}

struct Shared {
    script: VmessScript,
    requests: Arc<Mutex<Vec<RecordedVmess>>>,
    ws_seen: Arc<Mutex<Vec<RecordedWs>>>,
    rejected: Arc<AtomicUsize>,
}

fn open_gcm(key: [u8; 16], iv: &[u8; 32], aad: &[u8], sealed: &mut [u8]) -> Option<usize> {
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&iv[..12]);
    LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &key).ok()?)
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad), sealed)
        .ok()
        .map(|p| p.len())
}

fn seal_gcm(key: [u8; 16], iv: &[u8; 32], plain: &[u8], out: &mut Vec<u8>) {
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&iv[..12]);
    let start = out.len();
    out.extend_from_slice(plain);
    let tag = LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &key).expect("a 16-byte key"))
        .seal_in_place_separate_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut out[start..],
        )
        .expect("a short head");
    out.extend_from_slice(tag.as_ref());
}

/// The AuthID's timestamp, when it is one of ours.
fn open_auth_id(cmd_key: &[u8; 16], auth_id: &[u8; 16]) -> Option<i64> {
    let key = kdf16(cmd_key, &[kdf::AUTH_ID_KEY]);
    let mut block = GenericArray::from(*auth_id);
    Aes128::new(&GenericArray::from(key)).decrypt_block(&mut block);
    let crc = u32::from_be_bytes([block[12], block[13], block[14], block[15]]);
    (crc32fast::hash(&block[..12]) == crc).then(|| {
        let mut time = [0u8; 8];
        time.copy_from_slice(&block[..8]);
        i64::from_be_bytes(time)
    })
}

struct Parsed {
    record: RecordedVmess,
    session: Session,
}

fn parse_head(plain: &[u8]) -> Option<Parsed> {
    let mut session = Session {
        body_iv: [0; 16],
        body_key: [0; 16],
        response_v: 0,
    };
    if *plain.first()? != 1 {
        return None;
    }
    session.body_iv.copy_from_slice(plain.get(1..17)?);
    session.body_key.copy_from_slice(plain.get(17..33)?);
    session.response_v = *plain.get(33)?;
    let options = *plain.get(34)?;
    let (padding, security) = (usize::from(plain.get(35)? >> 4), plain.get(35)? & 15);
    let command = *plain.get(37)?;
    let port = u16::from_be_bytes([*plain.get(38)?, *plain.get(39)?]);
    let atyp = *plain.get(40)?;
    let (host, used) = match atyp {
        1 => {
            let b: [u8; 4] = plain.get(41..45)?.try_into().ok()?;
            (IpAddr::V4(Ipv4Addr::from(b)).to_string(), 45)
        }
        3 => {
            let b: [u8; 16] = plain.get(41..57)?.try_into().ok()?;
            (IpAddr::V6(Ipv6Addr::from(b)).to_string(), 57)
        }
        2 => {
            let len = usize::from(*plain.get(41)?);
            (
                String::from_utf8_lossy(plain.get(42..42 + len)?).into_owned(),
                42 + len,
            )
        }
        _ => return None,
    };
    // padding, then the FNV-1a of everything before it
    let body = plain.get(..used + padding)?;
    let check: [u8; 4] = plain
        .get(used + padding..used + padding + 4)?
        .try_into()
        .ok()?;
    let fnv = body.iter().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(*b)).wrapping_mul(0x0100_0193)
    });
    (plain.len() == used + padding + 4 && fnv == u32::from_be_bytes(check)).then_some(Parsed {
        record: RecordedVmess {
            command,
            options,
            security,
            padding,
            atyp,
            host,
            port,
            skew: 0,
            early: 0,
        },
        session,
    })
}

/// Refuses the way a real server does: nothing is ever answered. The write
/// side is closed first and the rest is read to its end, so the client sees a
/// clean end of stream — closing over unread bytes would reset the connection
/// and hand the client an I/O error instead.
async fn refuse(mut stream: BoxedStream) -> io::Result<()> {
    let _ = stream.shutdown().await;
    let mut sink = [0u8; 1024];
    while matches!(stream.read(&mut sink).await, Ok(n) if n > 0) {}
    Ok(())
}

async fn serve(mut stream: BoxedStream, shared: Arc<Shared>) -> io::Result<()> {
    if shared.script.ws {
        stream = accept_bytes(stream, &shared.ws_seen).await?;
    }
    let cmd_key = header::cmd_key(&shared.script.uuid);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut need = 16 + 18 + 8;
    let mut head_len = None;
    let (mut parsed, skew) = loop {
        if buf.len() < need {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&chunk[..n]);
            continue;
        }
        let auth_id: [u8; 16] = buf[..16].try_into().expect("16 bytes");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let reject = |shared: &Shared| shared.rejected.fetch_add(1, Ordering::SeqCst);
        let Some(time) = open_auth_id(&cmd_key, &auth_id).filter(|t| (t - now).abs() <= 120) else {
            reject(&shared);
            return refuse(stream).await;
        };
        let nonce: [u8; 8] = buf[34..42].try_into().expect("8 bytes");
        let path = |label: &'static [u8]| [label, &auth_id[..], &nonce[..]];
        let len = match head_len {
            Some(len) => len,
            None => {
                let mut sealed: [u8; 18] = buf[16..34].try_into().expect("18 bytes");
                let opened = open_gcm(
                    kdf16(&cmd_key, &path(kdf::HEADER_LEN_KEY)),
                    &kdf(&cmd_key, &path(kdf::HEADER_LEN_NONCE)),
                    &auth_id,
                    &mut sealed,
                );
                if opened != Some(2) {
                    reject(&shared);
                    return refuse(stream).await;
                }
                let len = usize::from(u16::from_be_bytes([sealed[0], sealed[1]]));
                head_len = Some(len);
                need = 42 + len + TAG;
                len
            }
        };
        if buf.len() < need {
            continue;
        }
        let mut sealed = buf[42..need].to_vec();
        let opened = open_gcm(
            kdf16(&cmd_key, &path(kdf::HEADER_KEY)),
            &kdf(&cmd_key, &path(kdf::HEADER_NONCE)),
            &auth_id,
            &mut sealed,
        );
        match opened.and_then(|n| (n == len).then(|| parse_head(&sealed[..n])).flatten()) {
            Some(parsed) => break (parsed, time - now),
            None => {
                reject(&shared);
                return refuse(stream).await;
            }
        }
    };
    parsed.record.skew = skew;
    parsed.record.early = buf.len() - need;
    shared
        .requests
        .lock()
        .expect("requests")
        .push(parsed.record.clone());
    let security = match parsed.record.security {
        3 => Security::Aes128Gcm,
        4 => Security::ChaCha20Poly1305,
        _ => return Ok(()),
    };
    let upstream_addr = match shared.script.connect_to {
        Some(addr) => addr,
        None => match parsed.record.host.parse::<IpAddr>() {
            Ok(ip) => SocketAddr::new(ip, parsed.record.port),
            // never resolves: a name without `connect_to` is a dead end
            Err(_) => return stream.shutdown().await,
        },
    };
    let upstream = TcpStream::connect(upstream_addr).await?;
    let session = parsed.session;
    let (key, iv) = header::response_secrets(&session);
    let mut answer = Vec::new();
    let plain = [session.response_v, 0, 0, 0];
    seal_gcm(
        kdf16(&key, &[kdf::RESPONSE_LEN_KEY]),
        &kdf(&iv, &[kdf::RESPONSE_LEN_IV]),
        &(plain.len() as u16).to_be_bytes(),
        &mut answer,
    );
    seal_gcm(
        kdf16(&key, &[kdf::RESPONSE_KEY]),
        &kdf(&iv, &[kdf::RESPONSE_IV]),
        &plain,
        &mut answer,
    );
    let stream = crate::transport::prefixed::boxed(buf[need..].to_vec(), stream);
    let (mut from_client, mut to_client) = tokio::io::split(stream);
    let (mut from_upstream, mut to_upstream) = upstream.into_split();
    let mut up = ChunkCipher::new(security, &session.body_key, &session.body_iv);
    let mut down = ChunkCipher::new(security, &key, &iv);
    let upward = async {
        loop {
            let mut len = [0u8; 2];
            if from_client.read_exact(&mut len).await.is_err() {
                break;
            }
            let mut sealed = vec![0u8; up.open_len(len)];
            if from_client.read_exact(&mut sealed).await.is_err() {
                break;
            }
            match up.open(&mut sealed) {
                Some(0) | None => break,
                Some(n) => to_upstream.write_all(&sealed[..n]).await?,
            }
        }
        to_upstream.shutdown().await
    };
    let downward = async {
        to_client.write_all(&answer).await?;
        let mut buf = vec![0u8; 8192];
        loop {
            let n = from_upstream.read(&mut buf).await?;
            let mut out = Vec::new();
            down.seal(&buf[..n], &mut out);
            to_client.write_all(&out).await?;
            if n == 0 {
                return to_client.shutdown().await;
            }
        }
    };
    let _ = tokio::join!(upward, downward);
    Ok(())
}

impl FakeVmess {
    /// `tls`: speak TLS with this fixture's certificate first.
    pub async fn spawn(script: VmessScript, tls: Option<Arc<TlsFixture>>) -> FakeVmess {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let requests: Arc<Mutex<Vec<RecordedVmess>>> = Arc::default();
        let ws_seen: Arc<Mutex<Vec<RecordedWs>>> = Arc::default();
        let connections = Arc::new(AtomicUsize::new(0));
        let rejected = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Shared {
            script,
            requests: requests.clone(),
            ws_seen: ws_seen.clone(),
            rejected: rejected.clone(),
        });
        let acceptor = tls.as_ref().map(|fixture| fixture.acceptor(false));
        let count = connections.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let (shared, tls, acceptor) = (shared.clone(), tls.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let stream: BoxedStream = match (&tls, &acceptor) {
                        (Some(fixture), Some(acceptor)) => {
                            match fixture.accept(acceptor, tcp).await {
                                Ok(stream) => stream,
                                Err(_) => return,
                            }
                        }
                        _ => Box::new(tcp),
                    };
                    let _ = serve(stream, shared).await;
                });
            }
        });
        FakeVmess {
            addr,
            requests,
            ws_seen,
            connections,
            rejected,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn requests(&self) -> Vec<RecordedVmess> {
        self.requests.lock().expect("requests").clone()
    }

    pub fn ws_seen(&self) -> Vec<RecordedWs> {
        self.ws_seen.lock().expect("ws").clone()
    }

    /// TCP connections accepted so far (before TLS).
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Connections dropped without an answer (unknown id, stale timestamp, garbage).
    pub fn rejected(&self) -> usize {
        self.rejected.load(Ordering::SeqCst)
    }
}
