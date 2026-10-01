//! A scriptable HTTP/2 CONNECT proxy (phase 2 M6 design 5.5): TLS with ALPN
//! `h2`, `h2::server`, Basic authentication, and a relay to the target for
//! every CONNECT stream; with extended CONNECT, CONNECT-UDP (RFC 9298) too,
//! a UDP socket per stream. It never resolves a name.

use super::{AbortOnDrop, TlsFixture};
use crate::h2pool::H2Stream;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use h2::RecvStream;
use h2::server::SendResponse;
use http::{Method, Request, Response, StatusCode};
use rurge_net::connector::BoxedStream;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

#[derive(Clone, Debug, Default)]
pub struct H2ProxyScript {
    /// Require Basic proxy credentials: any of these (user, password).
    pub users: Vec<(String, String)>,
    /// Answer a request without a `user-agent` 400, as a server that
    /// follows the TrustTunnel document to the letter would.
    pub require_user_agent: bool,
    /// Answer every request with this status instead of serving it.
    pub refuse: Option<u16>,
    /// Wait this long before answering a request.
    pub delay: Duration,
    /// SETTINGS_MAX_CONCURRENT_STREAMS.
    pub max_concurrent_streams: Option<u32>,
    /// Send GOAWAY once a connection has accepted this many streams; those
    /// run to their end.
    pub goaway_after: Option<usize>,
    /// Advertise extended CONNECT (SETTINGS_ENABLE_CONNECT_PROTOCOL = 1),
    /// which CONNECT-UDP needs.
    pub extended_connect: bool,
    /// On CONNECT-UDP, put a capsule of an unknown type and a datagram with
    /// context id 2 before every datagram sent back, their varints longer
    /// than needed.
    pub udp_extra_capsules: bool,
    /// Take no part in ALPN, like a TLS server that knows nothing of HTTP/2
    /// (a rustls server with protocols of its own would rather fail the
    /// handshake when none is the client's).
    pub no_alpn: bool,
    /// Complete the TLS handshake only with a client certificate signed by
    /// the fixture's CA.
    pub require_client_cert: bool,
    /// Tunnel or relay here whatever the client asked for (needed for a
    /// domain target).
    pub connect_to: Option<SocketAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedH2Request {
    /// The connection that carried the stream, 0 for the first accepted.
    pub connection: usize,
    pub method: String,
    /// `:authority`.
    pub authority: String,
    /// `:protocol` (extended CONNECT).
    pub protocol: Option<String>,
    /// `:path`; empty for a plain CONNECT.
    pub path: String,
    /// In arrival order; HTTP/2 names are lowercase.
    pub headers: Vec<(String, String)>,
}

impl RecordedH2Request {
    /// The first header called `name` (lowercase).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

pub struct FakeH2Proxy {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedH2Request>>>,
    connections: Arc<AtomicUsize>,
    _task: AbortOnDrop,
}

struct Shared {
    script: H2ProxyScript,
    requests: Arc<Mutex<Vec<RecordedH2Request>>>,
}

fn record(request: &Request<RecvStream>, connection: usize) -> RecordedH2Request {
    let uri = request.uri();
    RecordedH2Request {
        connection,
        method: request.method().to_string(),
        authority: uri.authority().map(|a| a.to_string()).unwrap_or_default(),
        protocol: request
            .extensions()
            .get::<h2::ext::Protocol>()
            .map(|p| p.as_str().to_string()),
        path: uri
            .path_and_query()
            .map(|p| p.to_string())
            .unwrap_or_default(),
        headers: request
            .headers()
            .iter()
            .map(|(n, v)| {
                (
                    n.as_str().to_string(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect(),
    }
}

fn authorized(users: &[(String, String)], request: &RecordedH2Request) -> bool {
    users.is_empty()
        || users.iter().any(|(user, password)| {
            let expected = format!("Basic {}", STANDARD.encode(format!("{user}:{password}")));
            request.header("proxy-authorization") == Some(expected.as_str())
        })
}

/// A status and no tunnel.
fn refuse(mut respond: SendResponse<Bytes>, status: StatusCode) {
    let mut response = Response::builder().status(status);
    if status == StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        response = response.header("proxy-authenticate", "Basic realm=\"fake\"");
    }
    let _ = respond.send_response(response.body(()).expect("a response"), true);
}

async fn answer(
    request: Request<RecvStream>,
    respond: SendResponse<Bytes>,
    connection: usize,
    shared: Arc<Shared>,
) {
    let seen = record(&request, connection);
    shared.requests.lock().expect("requests").push(seen.clone());
    tokio::time::sleep(shared.script.delay).await;
    if !authorized(&shared.script.users, &seen) {
        return refuse(respond, StatusCode::PROXY_AUTHENTICATION_REQUIRED);
    }
    if shared.script.require_user_agent && seen.header("user-agent").is_none() {
        return refuse(respond, StatusCode::BAD_REQUEST);
    }
    if let Some(code) = shared.script.refuse {
        return refuse(respond, StatusCode::from_u16(code).expect("a status"));
    }
    if request.method() == Method::CONNECT && seen.protocol.as_deref() == Some("connect-udp") {
        return connect_udp(request, respond, &seen, &shared.script).await;
    }
    // no other extended CONNECT protocol is served
    if request.method() != Method::CONNECT || seen.protocol.is_some() {
        return refuse(respond, StatusCode::NOT_IMPLEMENTED);
    }
    let target = match shared.script.connect_to {
        Some(addr) => Some(addr),
        // never resolves: a name without `connect_to` is a dead end
        None => seen.authority.parse::<SocketAddr>().ok(),
    };
    let upstream = match target {
        Some(addr) => TcpStream::connect(addr).await.ok(),
        None => None,
    };
    // an unreachable target is 502, as the TrustTunnel endpoint answers
    let Some(mut upstream) = upstream else {
        return refuse(respond, StatusCode::BAD_GATEWAY);
    };
    tunnel(request, respond, &mut upstream).await;
}

/// 200, then the stream's bytes to and from `upstream`, half-closes
/// included.
async fn tunnel(
    request: Request<RecvStream>,
    mut respond: SendResponse<Bytes>,
    upstream: &mut TcpStream,
) {
    let Ok(send) = respond.send_response(Response::new(()), false) else {
        return;
    };
    let mut stream = H2Stream::new("fake-h2", send, request.into_body());
    let _ = tokio::io::copy_bidirectional(&mut stream, upstream).await;
}

/// `/.well-known/masque/udp/{host}/{port}/` (RFC 9298 2): the host
/// percent-decoded, and the port.
fn masque_target(path: &str) -> Option<(String, u16)> {
    let rest = path
        .strip_prefix("/.well-known/masque/udp/")?
        .strip_suffix('/')?;
    let (host, port) = rest.split_once('/')?;
    let mut decoded = Vec::new();
    let mut bytes = host.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            decoded.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            decoded.push(b);
        }
    }
    Some((String::from_utf8(decoded).ok()?, port.parse().ok()?))
}

/// `value` as a QUIC varint of `len` bytes (1, 2, 4 or 8), minimal or not.
fn put_varint(out: &mut Vec<u8>, value: u64, len: usize) {
    let tag = (len.trailing_zeros() as u64) << (len * 8 - 2);
    out.extend_from_slice(&(value | tag).to_be_bytes()[8 - len..]);
}

/// The first capsule in `buf` (type, value), taken out of it; `None` while
/// it is incomplete.
fn take_capsule(buf: &mut Vec<u8>) -> Option<(u64, Vec<u8>)> {
    fn varint(bytes: &[u8]) -> Option<(u64, usize)> {
        let len = 1usize << (bytes.first()? >> 6);
        let mut value = u64::from(bytes[0] & 0x3f);
        for &b in bytes.get(1..len)? {
            value = value << 8 | u64::from(b);
        }
        Some((value, len))
    }
    let (kind, a) = varint(buf)?;
    let (len, b) = varint(&buf[a..])?;
    let end = a + b + usize::try_from(len).ok()?;
    let value = buf.get(a + b..end)?.to_vec();
    buf.drain(..end);
    Some((kind, value))
}

/// A CONNECT-UDP request: 200 with `capsule-protocol: ?1`, then a UDP
/// socket relaying the stream's DATAGRAM capsules (context id 0) to the
/// target in the path and the target's datagrams back.
async fn connect_udp(
    request: Request<RecvStream>,
    mut respond: SendResponse<Bytes>,
    seen: &RecordedH2Request,
    script: &H2ProxyScript,
) {
    let target = match (script.connect_to, masque_target(&seen.path)) {
        (Some(addr), Some(_)) => Some(addr),
        (None, Some((host, port))) => host
            .parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, port)),
        (_, None) => None,
    };
    let Some(target) = target else {
        return refuse(respond, StatusCode::BAD_REQUEST);
    };
    let local: SocketAddr = if target.is_ipv4() {
        "127.0.0.1:0".parse().expect("an address")
    } else {
        "[::1]:0".parse().expect("an address")
    };
    let Ok(socket) = UdpSocket::bind(local).await else {
        return refuse(respond, StatusCode::BAD_GATEWAY);
    };
    let response = Response::builder()
        .header("capsule-protocol", "?1")
        .body(())
        .expect("a response");
    let Ok(send) = respond.send_response(response, false) else {
        return;
    };
    let stream = H2Stream::new("fake-h2", send, request.into_body());
    let (mut reader, mut writer) = tokio::io::split(stream);
    let extra = script.udp_extra_capsules;
    let socket = Arc::new(socket);
    let outbound = socket.clone();
    let up = async move {
        let (mut buf, mut chunk) = (Vec::new(), vec![0u8; 65536]);
        while let Ok(n @ 1..) = reader.read(&mut chunk).await {
            buf.extend_from_slice(&chunk[..n]);
            while let Some((kind, value)) = take_capsule(&mut buf) {
                // a DATAGRAM capsule with context id 0 (one byte)
                if kind == 0 && value.first() == Some(&0) {
                    let _ = outbound.send_to(&value[1..], target).await;
                }
            }
        }
    };
    let down = async move {
        let mut datagram = vec![0u8; 65536];
        while let Ok((n, from)) = socket.recv_from(&mut datagram).await {
            if from != target {
                continue;
            }
            let mut out = Vec::new();
            if extra {
                // type 0x2a2a in eight bytes, three bytes of value
                put_varint(&mut out, 0x2a2a, 8);
                put_varint(&mut out, 3, 4);
                out.extend_from_slice(b"???");
                // context id 2 in two bytes
                put_varint(&mut out, 0, 2);
                put_varint(&mut out, n as u64 + 2, 8);
                put_varint(&mut out, 2, 2);
                out.extend_from_slice(&datagram[..n]);
            }
            put_varint(&mut out, 0, 1);
            put_varint(&mut out, n as u64 + 1, 4);
            put_varint(&mut out, 0, 1);
            out.extend_from_slice(&datagram[..n]);
            if writer.write_all(&out).await.is_err() {
                return;
            }
        }
    };
    // the socket lives as long as the stream (RFC 9298 3.1)
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
}

async fn serve(stream: BoxedStream, connection: usize, shared: Arc<Shared>) {
    let mut builder = h2::server::Builder::new();
    if let Some(n) = shared.script.max_concurrent_streams {
        builder.max_concurrent_streams(n);
    }
    if shared.script.extended_connect {
        builder.enable_connect_protocol();
    }
    let Ok(mut conn) = builder.handshake::<_, Bytes>(stream).await else {
        return;
    };
    let mut accepted = 0;
    // `accept` also drives the connection: polled until it ends
    while let Some(Ok((request, respond))) = conn.accept().await {
        accepted += 1;
        tokio::spawn(answer(request, respond, connection, shared.clone()));
        if shared.script.goaway_after == Some(accepted) {
            conn.graceful_shutdown();
        }
    }
}

impl FakeH2Proxy {
    pub async fn spawn(script: H2ProxyScript, fixture: Arc<TlsFixture>) -> FakeH2Proxy {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let alpn: &[&[u8]] = if script.no_alpn { &[] } else { &[b"h2"] };
        let acceptor = fixture.acceptor_with_alpn(script.require_client_cert, alpn);
        let requests: Arc<Mutex<Vec<RecordedH2Request>>> = Arc::default();
        let connections = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Shared {
            script,
            requests: requests.clone(),
        });
        let count = connections.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let connection = count.fetch_add(1, Ordering::SeqCst);
                let (shared, fixture, acceptor) =
                    (shared.clone(), fixture.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let Ok(stream) = fixture.accept(&acceptor, tcp).await else {
                        return;
                    };
                    serve(stream, connection, shared).await;
                });
            }
        });
        FakeH2Proxy {
            addr,
            requests,
            connections,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Every request, in arrival order.
    pub fn requests(&self) -> Vec<RecordedH2Request> {
        self.requests.lock().expect("requests").clone()
    }

    /// TCP connections accepted so far (before TLS).
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}
