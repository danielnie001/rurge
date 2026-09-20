//! A scriptable AnyTLS server behind TLS (protocol version 2, or 1 on
//! request). It never resolves a name.

use super::{AbortOnDrop, TlsFixture};
use crate::anytls::frame::{self, HEADER};
use crate::anytls::padding::Scheme;
use rurge_net::connector::BoxedStream;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

#[derive(Clone, Debug, Default)]
pub struct AnyTlsScript {
    pub password: String,
    /// Relay here whatever the client asked for (needed for a domain target).
    pub connect_to: Option<SocketAddr>,
    /// The server's padding scheme: pushed to a client whose md5 differs.
    pub scheme: Option<String>,
    /// Answer `cmdSettings` with this alert and close.
    pub alert: Option<String>,
    /// Refuse every stream with this text in its `cmdSYNACK`.
    pub refuse: Option<String>,
    /// Send a `cmdHeartRequest` after the settings.
    pub heartbeat: bool,
    /// Speak protocol version 1: no `cmdServerSettings`, no `cmdSYNACK`.
    pub v1: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedStream {
    /// Which authenticated connection carried it (0-based).
    pub session: usize,
    pub sid: u32,
    pub atyp: u8,
    pub host: String,
    pub port: u16,
}

#[derive(Default)]
struct Seen {
    sessions: AtomicUsize,
    rejected: AtomicUsize,
    heart_responses: AtomicUsize,
    fins: AtomicUsize,
    waste: AtomicUsize,
    streams: Mutex<Vec<RecordedStream>>,
    settings: Mutex<Vec<String>>,
    /// `kick`: every connection parked on its next frame is closed.
    kick: tokio::sync::Notify,
}

pub struct FakeAnyTls {
    addr: SocketAddr,
    seen: Arc<Seen>,
    _task: AbortOnDrop,
}

/// `(atyp, host, port, bytes used)` of a SOCKS5 address.
fn parse_addr(buf: &[u8]) -> Option<(u8, String, u16, usize)> {
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
            (
                String::from_utf8_lossy(buf.get(2..2 + len)?).into_owned(),
                2 + len,
            )
        }
        _ => return None,
    };
    let port = u16::from_be_bytes(buf.get(used..used + 2)?.try_into().ok()?);
    Some((atyp, host, port, used + 2))
}

struct Live {
    to_upstream: OwnedWriteHalf,
    _reader: AbortOnDrop,
}

/// After an alert: the frame is on the wire, and the connection is read to
/// its end rather than closed over unread bytes, which would reset it and
/// could cost the client the alert.
async fn alerted(
    out: mpsc::UnboundedSender<Vec<u8>>,
    writer: tokio::task::JoinHandle<()>,
    mut reader: tokio::io::ReadHalf<BoxedStream>,
) -> io::Result<()> {
    drop(out);
    let _ = writer.await;
    let mut sink = [0u8; 1024];
    while matches!(reader.read(&mut sink).await, Ok(n) if n > 0) {}
    Ok(())
}

async fn serve(
    mut stream: BoxedStream,
    script: Arc<AnyTlsScript>,
    seen: Arc<Seen>,
) -> io::Result<()> {
    let mut auth = [0u8; 34];
    stream.read_exact(&mut auth).await?;
    if auth[..32] != Sha256::digest(script.password.as_bytes())[..] {
        // what a real server's fallback site would say
        seen.rejected.fetch_add(1, Ordering::SeqCst);
        stream
            .write_all(
                b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
            )
            .await?;
        return stream.shutdown().await;
    }
    let mut padding0 = vec![0u8; usize::from(u16::from_be_bytes([auth[32], auth[33]]))];
    stream.read_exact(&mut padding0).await?;
    seen.waste.fetch_add(padding0.len(), Ordering::SeqCst);
    let session = seen.sessions.fetch_add(1, Ordering::SeqCst);
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (out, mut queue) = mpsc::unbounded_channel::<Vec<u8>>();
    // ends by itself once every sender is gone (this function's and the streams')
    let writer = tokio::spawn(async move {
        while let Some(bytes) = queue.recv().await {
            if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
                return;
            }
        }
    });
    let mut settled = false;
    let mut live: HashMap<u32, Option<Live>> = HashMap::new();
    loop {
        let mut header = [0u8; HEADER];
        tokio::select! {
            _ = seen.kick.notified() => return Ok(()),
            read = reader.read_exact(&mut header) => {
                if read.is_err() {
                    return Ok(());
                }
            }
        }
        let (command, sid, len) = frame::parse_header(&header);
        let mut data = vec![0u8; len];
        reader.read_exact(&mut data).await?;
        match command {
            frame::WASTE => {
                seen.waste.fetch_add(len, Ordering::SeqCst);
            }
            frame::SETTINGS => {
                settled = true;
                let text = String::from_utf8_lossy(&data).into_owned();
                let theirs = text
                    .lines()
                    .find_map(|l| l.strip_prefix("padding-md5="))
                    .map(str::to_string);
                seen.settings.lock().expect("settings").push(text);
                if let Some(alert) = &script.alert {
                    let _ = out.send(frame::frame(frame::ALERT, 0, alert.as_bytes()));
                    return alerted(out, writer, reader).await;
                }
                if let Some(scheme) = &script.scheme {
                    // a scheme the client could not parse has no md5 to compare: push it anyway
                    let ours = Scheme::parse(scheme.as_bytes()).map(|s| s.md5().to_string());
                    if ours.is_none() || ours != theirs {
                        let _ = out.send(frame::frame(
                            frame::UPDATE_PADDING_SCHEME,
                            0,
                            scheme.as_bytes(),
                        ));
                    }
                }
                if !script.v1 {
                    let _ = out.send(frame::frame(frame::SERVER_SETTINGS, 0, b"v=2"));
                }
                if script.heartbeat {
                    let _ = out.send(frame::frame(frame::HEART_REQUEST, 0, &[]));
                }
            }
            frame::SYN => {
                if !settled {
                    let _ = out.send(frame::frame(
                        frame::ALERT,
                        0,
                        b"client did not send its settings",
                    ));
                    return alerted(out, writer, reader).await;
                }
                live.insert(sid, None);
            }
            frame::PSH => match live.get_mut(&sid) {
                // the first push of a stream is its target
                Some(slot @ None) => {
                    let Some((atyp, host, port, used)) = parse_addr(&data) else {
                        return Ok(());
                    };
                    seen.streams.lock().expect("streams").push(RecordedStream {
                        session,
                        sid,
                        atyp,
                        host: host.clone(),
                        port,
                    });
                    if let Some(text) = &script.refuse {
                        let _ = out.send(frame::frame(frame::SYNACK, sid, text.as_bytes()));
                        live.remove(&sid);
                        continue;
                    }
                    let addr = match (script.connect_to, host.parse::<IpAddr>()) {
                        (Some(addr), _) => Some(addr),
                        (None, Ok(ip)) => Some(SocketAddr::new(ip, port)),
                        // never resolves: a name without `connect_to` is a dead end
                        (None, Err(_)) => None,
                    };
                    let upstream = match addr {
                        Some(addr) => TcpStream::connect(addr).await.ok(),
                        None => None,
                    };
                    let Some(upstream) = upstream else {
                        if !script.v1 {
                            let _ =
                                out.send(frame::frame(frame::SYNACK, sid, b"connection refused"));
                        }
                        let _ = out.send(frame::frame(frame::FIN, sid, &[]));
                        live.remove(&sid);
                        continue;
                    };
                    if !script.v1 {
                        let _ = out.send(frame::frame(frame::SYNACK, sid, &[]));
                    }
                    let (mut from_upstream, mut to_upstream) = upstream.into_split();
                    to_upstream.write_all(&data[used..]).await?;
                    let out = out.clone();
                    let reader = tokio::spawn(async move {
                        let mut buf = vec![0u8; 8192];
                        loop {
                            match from_upstream.read(&mut buf).await {
                                Ok(n) if n > 0 => {
                                    if out.send(frame::frame(frame::PSH, sid, &buf[..n])).is_err() {
                                        return;
                                    }
                                }
                                _ => {
                                    let _ = out.send(frame::frame(frame::FIN, sid, &[]));
                                    return;
                                }
                            }
                        }
                    });
                    *slot = Some(Live {
                        to_upstream,
                        _reader: AbortOnDrop(reader),
                    });
                }
                Some(Some(stream)) => stream.to_upstream.write_all(&data).await?,
                // a stream that is gone: dropped, as the reference does
                None => {}
            },
            frame::FIN => {
                seen.fins.fetch_add(1, Ordering::SeqCst);
                live.remove(&sid);
            }
            frame::HEART_RESPONSE => {
                seen.heart_responses.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
    }
}

impl FakeAnyTls {
    pub async fn spawn(script: AnyTlsScript, fixture: Arc<TlsFixture>) -> FakeAnyTls {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let seen: Arc<Seen> = Arc::default();
        let script = Arc::new(script);
        let shared = seen.clone();
        let acceptor = fixture.acceptor(false);
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (script, seen) = (script.clone(), shared.clone());
                let (fixture, acceptor) = (fixture.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let Ok(stream) = fixture.accept(&acceptor, tcp).await else {
                        return;
                    };
                    let _ = serve(stream, script, seen).await;
                });
            }
        });
        FakeAnyTls {
            addr,
            seen,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Closes every connection that is waiting for its next frame (an idle
    /// session, as a server's own clean-up would).
    pub fn kick(&self) {
        self.seen.kick.notify_waiters();
    }

    /// Connections that authenticated.
    pub fn sessions(&self) -> usize {
        self.seen.sessions.load(Ordering::SeqCst)
    }

    /// Connections answered like a web server because the password was wrong.
    pub fn rejected(&self) -> usize {
        self.seen.rejected.load(Ordering::SeqCst)
    }

    pub fn streams(&self) -> Vec<RecordedStream> {
        self.seen.streams.lock().expect("streams").clone()
    }

    /// Every `cmdSettings` text received.
    pub fn settings(&self) -> Vec<String> {
        self.seen.settings.lock().expect("settings").clone()
    }

    pub fn heart_responses(&self) -> usize {
        self.seen.heart_responses.load(Ordering::SeqCst)
    }

    pub fn fins(&self) -> usize {
        self.seen.fins.load(Ordering::SeqCst)
    }

    /// Padding bytes received: the authentication's and every `cmdWaste`.
    pub fn waste(&self) -> usize {
        self.seen.waste.load(Ordering::SeqCst)
    }
}
