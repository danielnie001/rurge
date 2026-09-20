//! A scriptable Trojan server: TLS, optionally a WebSocket below the
//! protocol, the request head, then a relay. It never resolves a name.

use super::ws::{RecordedWs, accept_bytes};
use super::{AbortOnDrop, TlsFixture};
use rurge_net::connector::BoxedStream;
use sha2::{Digest, Sha224};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug, Default)]
pub struct TrojanScript {
    pub password: String,
    /// Expect a WebSocket handshake between TLS and the request head.
    pub ws: bool,
    /// Relay here whatever the client asked for (needed for a domain target).
    pub connect_to: Option<SocketAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedTrojan {
    pub command: u8,
    pub atyp: u8,
    /// An IP literal, or the name exactly as it was on the wire.
    pub host: String,
    pub port: u16,
    /// Payload that arrived in the same read as the end of the head.
    pub early: Vec<u8>,
}

pub struct FakeTrojan {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedTrojan>>>,
    ws_seen: Arc<Mutex<Vec<RecordedWs>>>,
    connections: Arc<AtomicUsize>,
    rejected: Arc<AtomicUsize>,
    _task: AbortOnDrop,
}

/// `Some((request, bytes consumed))` once `buf` holds a whole head after the
/// 58-byte `hash CRLF` prefix.
fn parse_request(buf: &[u8]) -> Option<(RecordedTrojan, usize)> {
    let (command, atyp) = (*buf.first()?, *buf.get(1)?);
    let (host, used) = match atyp {
        1 => {
            let b: [u8; 4] = buf.get(2..6)?.try_into().ok()?;
            (IpAddr::V4(Ipv4Addr::from(b)).to_string(), 6)
        }
        4 => {
            let b: [u8; 16] = buf.get(2..18)?.try_into().ok()?;
            (IpAddr::V6(Ipv6Addr::from(b)).to_string(), 18)
        }
        _ => {
            let len = usize::from(*buf.get(2)?);
            let name = buf.get(3..3 + len)?;
            (String::from_utf8_lossy(name).into_owned(), 3 + len)
        }
    };
    let port = u16::from_be_bytes(buf.get(used..used + 2)?.try_into().ok()?);
    // the closing CRLF
    buf.get(used + 2..used + 4)?;
    Some((
        RecordedTrojan {
            command,
            atyp,
            host,
            port,
            early: Vec::new(),
        },
        used + 4,
    ))
}

struct Shared {
    script: TrojanScript,
    requests: Arc<Mutex<Vec<RecordedTrojan>>>,
    ws_seen: Arc<Mutex<Vec<RecordedWs>>>,
    rejected: Arc<AtomicUsize>,
}

async fn serve(mut stream: BoxedStream, shared: Arc<Shared>) -> io::Result<()> {
    if shared.script.ws {
        stream = accept_bytes(stream, &shared.ws_seen).await?;
    }
    let expected: String = Sha224::digest(shared.script.password.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let (mut request, consumed) = loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() < 58 {
            continue;
        }
        if &buf[..56] != expected.as_bytes() || &buf[56..58] != b"\r\n" {
            // what a real server's fallback site would say
            shared.rejected.fetch_add(1, Ordering::SeqCst);
            stream
                .write_all(
                    b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                )
                .await?;
            return stream.shutdown().await;
        }
        if let Some((request, used)) = parse_request(&buf[58..]) {
            break (request, 58 + used);
        }
    };
    request.early = buf[consumed..].to_vec();
    shared
        .requests
        .lock()
        .expect("requests")
        .push(request.clone());
    let upstream_addr = match shared.script.connect_to {
        Some(addr) => addr,
        None => match request.host.parse::<IpAddr>() {
            Ok(ip) => SocketAddr::new(ip, request.port),
            // never resolves: a name without `connect_to` is a dead end
            Err(_) => return stream.shutdown().await,
        },
    };
    let mut upstream = TcpStream::connect(upstream_addr).await?;
    upstream.write_all(&request.early).await?;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
    Ok(())
}

impl FakeTrojan {
    pub async fn spawn(script: TrojanScript, fixture: Arc<TlsFixture>) -> FakeTrojan {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let requests: Arc<Mutex<Vec<RecordedTrojan>>> = Arc::default();
        let ws_seen: Arc<Mutex<Vec<RecordedWs>>> = Arc::default();
        let connections = Arc::new(AtomicUsize::new(0));
        let rejected = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Shared {
            script,
            requests: requests.clone(),
            ws_seen: ws_seen.clone(),
            rejected: rejected.clone(),
        });
        let acceptor = fixture.acceptor(false);
        let count = connections.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let (shared, fixture, acceptor) =
                    (shared.clone(), fixture.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let Ok(stream) = fixture.accept(&acceptor, tcp).await else {
                        return;
                    };
                    let _ = serve(stream, shared).await;
                });
            }
        });
        FakeTrojan {
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

    pub fn requests(&self) -> Vec<RecordedTrojan> {
        self.requests.lock().expect("requests").clone()
    }

    pub fn ws_seen(&self) -> Vec<RecordedWs> {
        self.ws_seen.lock().expect("ws").clone()
    }

    /// TCP connections accepted so far (before TLS).
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Connections answered like a web server because the hash was wrong.
    pub fn rejected(&self) -> usize {
        self.rejected.load(Ordering::SeqCst)
    }
}
