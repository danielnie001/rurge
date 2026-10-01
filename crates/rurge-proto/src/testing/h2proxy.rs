//! A scriptable HTTP/2 CONNECT proxy (phase 2 M6 design 5.5): TLS with ALPN
//! `h2`, `h2::server`, Basic authentication, and a relay to the target for
//! every CONNECT stream. It never resolves a name.

use super::{AbortOnDrop, TlsFixture};
use crate::h2pool::H2Stream;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use h2::RecvStream;
use h2::server::SendResponse;
use http::{Method, Request, Response, StatusCode};
use rurge_net::connector::BoxedStream;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug, Default)]
pub struct H2ProxyScript {
    /// Require Basic proxy credentials: any of these (user, password).
    pub users: Vec<(String, String)>,
    /// Answer every request with this status instead of serving it.
    pub refuse: Option<u16>,
    /// Wait this long before answering a request.
    pub delay: Duration,
    /// SETTINGS_MAX_CONCURRENT_STREAMS.
    pub max_concurrent_streams: Option<u32>,
    /// Send GOAWAY once a connection has accepted this many streams; those
    /// run to their end.
    pub goaway_after: Option<usize>,
    /// Advertise extended CONNECT (SETTINGS_ENABLE_CONNECT_PROTOCOL = 1).
    pub extended_connect: bool,
    /// Take no part in ALPN, like a TLS server that knows nothing of HTTP/2
    /// (a rustls server with protocols of its own would rather fail the
    /// handshake when none is the client's).
    pub no_alpn: bool,
    /// Complete the TLS handshake only with a client certificate signed by
    /// the fixture's CA.
    pub require_client_cert: bool,
    /// Tunnel here whatever the client asked for (needed for a domain target).
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
    if let Some(code) = shared.script.refuse {
        return refuse(respond, StatusCode::from_u16(code).expect("a status"));
    }
    // no extended CONNECT protocol is served
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
