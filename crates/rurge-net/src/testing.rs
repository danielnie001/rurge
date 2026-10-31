//! What tests share: an in-process HTTP(S) server (programmable routes, ETag /
//! 304, delays and request recording), a resource cache with a copy already in
//! it, a capture of the log, and loopback ports that nothing listens on. Never
//! used by production code.

use crate::resource::{CacheDir, Meta};
use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::fmt::Debug;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;
use url::Url;

#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub version: String,
    /// The server the target of the request names, when the target names
    /// one: what HTTP/2 sends as `:authority`. An HTTP/1 request names a
    /// path, and the server in its `Host` field.
    pub authority: Option<String>,
    pub headers: Vec<(String, String)>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone)]
struct Route {
    body: Bytes,
    status: u16,
    delay: Duration,
    headers: Vec<(String, String)>,
    /// Whether the route sends an ETag and answers 304 to a matching
    /// If-None-Match.
    validators: bool,
}

#[derive(Default)]
struct State {
    routes: HashMap<String, Route>,
    requests: Vec<RecordedRequest>,
    /// TCP connections accepted since the server started. `requests` counts
    /// requests, this counts the connections they came in on.
    connections: usize,
}

pub struct TestServer {
    addr: SocketAddr,
    tls: bool,
    state: Arc<Mutex<State>>,
    _shutdown: oneshot::Sender<()>,
}

impl TestServer {
    pub async fn spawn() -> TestServer {
        Self::start(None).await
    }

    pub async fn spawn_tls() -> TestServer {
        Self::start(Some(tls_acceptor())).await
    }

    /// HTTPS with a certificate of the caller's making, so a client can be
    /// given the roots that trust it.
    pub async fn spawn_tls_with(acceptor: TlsAcceptor) -> TestServer {
        Self::start(Some(acceptor)).await
    }

    async fn start(acceptor: Option<TlsAcceptor>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let tls = acceptor.is_some();
        let state = Arc::new(Mutex::new(State::default()));
        let (tx, mut rx) = oneshot::channel::<()>();
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        // Counted before the connection is handed to its task,
                        // and for TLS too: the handshake may never finish, the
                        // connection was still made.
                        st.lock().unwrap().connections += 1;
                        let st = st.clone();
                        let acceptor = acceptor.clone();
                        tokio::spawn(async move {
                            let svc = service_fn(move |req| handle(req, st.clone()));
                            let builder = auto::Builder::new(TokioExecutor::new());
                            match acceptor {
                                Some(a) => {
                                    if let Ok(s) = a.accept(stream).await {
                                        let _ = builder.serve_connection(TokioIo::new(s), svc).await;
                                    }
                                }
                                None => {
                                    let _ = builder.serve_connection(TokioIo::new(stream), svc).await;
                                }
                            }
                        });
                    }
                }
            }
        });
        TestServer {
            addr,
            tls,
            state,
            _shutdown: tx,
        }
    }

    pub fn url(&self, path: &str) -> Url {
        let scheme = if self.tls { "https" } else { "http" };
        Url::parse(&format!("{scheme}://127.0.0.1:{}{path}", self.addr.port())).expect("url")
    }

    pub fn set(&self, path: &str, body: impl Into<Bytes>) {
        let mut st = self.state.lock().unwrap();
        let route = st.routes.entry(path.to_string()).or_insert(Route {
            body: Bytes::new(),
            status: 200,
            delay: Duration::ZERO,
            headers: Vec::new(),
            validators: true,
        });
        route.body = body.into();
    }

    pub fn set_status(&self, path: &str, status: u16) {
        if let Some(r) = self.state.lock().unwrap().routes.get_mut(path) {
            r.status = status;
        }
    }

    pub fn set_delay(&self, path: &str, delay: Duration) {
        if let Some(r) = self.state.lock().unwrap().routes.get_mut(path) {
            r.delay = delay;
        }
    }

    pub fn set_header(&self, path: &str, name: &str, value: &str) {
        if let Some(r) = self.state.lock().unwrap().routes.get_mut(path) {
            r.headers.push((name.to_string(), value.to_string()));
        }
    }

    /// With `on` false the route is a server that cannot say whether a body
    /// changed: it sends no ETag of its own and ignores If-None-Match, so
    /// every request gets the whole body.
    pub fn set_validators(&self, path: &str, on: bool) {
        if let Some(r) = self.state.lock().unwrap().routes.get_mut(path) {
            r.validators = on;
        }
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.state.lock().unwrap().requests.clone()
    }

    pub fn hits(&self, path: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.path == path)
            .count()
    }

    /// How many TCP connections were accepted, requests aside: one connection
    /// may carry several requests (`hits` counts those), and a connection that
    /// never sent one still counts.
    pub fn connections(&self) -> usize {
        self.state.lock().unwrap().connections
    }
}

fn etag_of(body: &[u8]) -> String {
    format!("\"{:x}\"", Sha256::digest(body))
}

/// Puts `data` in the cache of the resource manager under `root` as the copy
/// of `url` that was last checked at `fetched_at`, the way a download of
/// it would have left it.
pub fn seed_cache(root: &Path, url: &Url, data: &[u8], fetched_at: SystemTime) {
    let mut meta = Meta {
        url: url.to_string(),
        ..Meta::default()
    };
    meta.stamp(fetched_at);
    CacheDir::for_url(root, url.as_str())
        .store(data, &mut meta)
        .expect("seed the cache");
}

/// The events of a test, down to INFO, one line to an event: its fields, the
/// message among them. For tests that say what a log must not contain.
#[derive(Clone, Default)]
pub struct Lines(pub Arc<Mutex<Vec<String>>>);

/// Routes the events of this thread to `subscriber` until the guard is
/// dropped. While one dispatcher alone is registered, tracing-core asks only
/// the thread that first reaches a call site whether it is wanted: a site
/// another test reaches first would stay shut on this thread too. With a
/// second one it asks every dispatcher, which the second element of the
/// result is for: it is kept for as long as the guard.
fn capture_events(
    subscriber: impl tracing::Subscriber + Send + Sync + 'static,
) -> (tracing::subscriber::DefaultGuard, tracing::Dispatch) {
    (
        tracing::subscriber::set_default(subscriber),
        tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()),
    )
}

impl Lines {
    /// Routes the events of this thread here until the guard, the first
    /// element, is dropped; the second, a dispatcher that records nothing, is
    /// kept for as long as the first.
    pub fn capture(&self) -> (tracing::subscriber::DefaultGuard, tracing::Dispatch) {
        capture_events(self.clone())
    }

    /// The lines that contain `what`.
    pub fn said(&self, what: &str) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.contains(what))
            .cloned()
            .collect()
    }

    pub fn forget(&self) {
        self.0.lock().unwrap().clear();
    }
}

impl tracing::Subscriber for Lines {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::INFO
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields(String);

        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn Debug) {
                self.0 += &format!(" {}={value:?}", field.name());
            }
        }

        let mut fields = Fields(String::new());
        event.record(&mut fields);
        self.0.lock().unwrap().push(fields.0);
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// The events of a test, down to DEBUG: the target and the fields of each,
/// the message among them, as the text they were recorded as. For tests that
/// pin an event's target and field names.
#[derive(Clone, Default)]
pub struct Events(Arc<Mutex<Vec<Event>>>);

type Event = (String, BTreeMap<String, String>);

impl Events {
    /// Routes the events of this thread here until the guard, the first
    /// element, is dropped; the second, a dispatcher that records nothing, is
    /// kept for as long as the first.
    pub fn capture(&self) -> (tracing::subscriber::DefaultGuard, tracing::Dispatch) {
        capture_events(self.clone())
    }

    /// The fields of the events of `target`, in the order they came.
    pub fn of(&self, target: &str) -> Vec<BTreeMap<String, String>> {
        let events = self.0.lock().unwrap();
        events
            .iter()
            .filter(|(of, _)| of == target)
            .map(|(_, fields)| fields.clone())
            .collect()
    }
}

impl tracing::Subscriber for Events {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::DEBUG
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields(BTreeMap<String, String>);

        impl tracing::field::Visit for Fields {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                self.0.insert(field.name().to_string(), value.to_string());
            }

            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn Debug) {
                self.0
                    .insert(field.name().to_string(), format!("{value:?}"));
            }
        }

        let mut fields = Fields(BTreeMap::new());
        event.record(&mut fields);
        let target = event.metadata().target().to_string();
        self.0.lock().unwrap().push((target, fields.0));
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

async fn handle(
    req: Request<Incoming>,
    state: Arc<Mutex<State>>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let path = req.uri().path().to_string();
    let route = {
        let mut st = state.lock().unwrap();
        st.requests.push(RecordedRequest {
            method: req.method().to_string(),
            path: path.clone(),
            version: format!("{:?}", req.version()),
            authority: req.uri().authority().map(|named| named.to_string()),
            headers: req
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.to_string(),
                        String::from_utf8_lossy(v.as_bytes()).to_string(),
                    )
                })
                .collect(),
        });
        st.routes.get(&path).cloned()
    };
    let Some(route) = route else {
        return Ok(Response::builder()
            .status(404)
            .body(Full::new(Bytes::new()))
            .unwrap());
    };
    if !route.delay.is_zero() {
        tokio::time::sleep(route.delay).await;
    }
    let etag = route.validators.then(|| etag_of(&route.body));
    let if_none_match = req
        .headers()
        .get("if-none-match")
        .and_then(|v| v.to_str().ok());
    let mut builder = Response::builder();
    for (k, v) in &route.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    if route.status == 200
        && let Some(etag) = &etag
        && if_none_match == Some(etag.as_str())
    {
        return Ok(builder
            .status(304)
            .header("etag", etag.as_str())
            .body(Full::new(Bytes::new()))
            .unwrap());
    }
    let status = StatusCode::from_u16(route.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if status == StatusCode::OK
        && let Some(etag) = etag
    {
        builder = builder.header("etag", etag);
    }
    Ok(builder.status(status).body(Full::new(route.body)).unwrap())
}

fn tls_acceptor() -> TlsAcceptor {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .expect("self-signed cert");
    let cert_der = cert.der().clone();
    let key_der: rustls::pki_types::PrivateKeyDer<'static> = signing_key.into();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .expect("server config");
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    TlsAcceptor::from(Arc::new(config))
}

/// A loopback port that `binds` could bind just now, and that no socket
/// another test opens can come by. A port the system handed out and the
/// test then gave back can be handed out again to anyone, a moment later;
/// this one lies in a range the system does not hand out by itself — the
/// dynamic ports of Windows (by default), Linux and macOS are all above it —
/// and is tried just before it is returned. One walk for every caller: no
/// two of them are given the same port.
fn unassigned_port(binds: impl Fn(u16) -> bool) -> u16 {
    const PORTS: std::ops::Range<u64> = 20_000..30_000;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // this process starts somewhere else in the range than the one before it
    let start = u64::from(std::process::id()).wrapping_mul(7919);
    for _ in 0..100 {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let port = PORTS.start + start.wrapping_add(n) % (PORTS.end - PORTS.start);
        let port = u16::try_from(port).expect("a port");
        if binds(port) {
            return port;
        }
    }
    panic!("no port of {PORTS:?} is free");
}

/// A loopback UDP port with nothing on it: a datagram to it is answered
/// with an ICMP "port unreachable".
pub fn closed_udp_port() -> u16 {
    unassigned_port(|port| std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok())
}

/// A loopback TCP port with nothing on it: a connection to it is refused —
/// at once on Linux and macOS, after about two seconds on Windows (measured
/// on the development machine: 2.03 to 2.05 s), which whoever connects to it
/// allows for. The socket that tried the port is closed before the number is
/// handed out, so that nothing is left on it: a socket left bound and not
/// listening refuses on Linux and Windows, and leaves the connection waiting
/// on macOS.
pub fn closed_tcp_port() -> u16 {
    unassigned_port(|port| std::net::TcpListener::bind(("127.0.0.1", port)).is_ok())
}

#[cfg(test)]
mod tests {
    use super::TestServer;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// Sends `request` in one write and reads one response whole: its head
    /// and as much body as its `Content-Length` names.
    async fn exchange(stream: &mut TcpStream, request: &str) -> String {
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut got: Vec<u8> = Vec::new();
        loop {
            if let Some(want) = response_end(&got).filter(|want| got.len() >= *want) {
                got.truncate(want);
                return String::from_utf8(got).expect("utf-8");
            }
            let mut chunk = [0u8; 1024];
            let n = stream.read(&mut chunk).await.expect("read");
            assert!(
                n > 0,
                "the server closed the connection early:\n{}",
                String::from_utf8_lossy(&got)
            );
            got.extend_from_slice(&chunk[..n]);
        }
    }

    /// Where the response in `got` ends once its head is there: the head and
    /// the body its `Content-Length` names.
    fn response_end(got: &[u8]) -> Option<usize> {
        let end = got.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
        let head = String::from_utf8_lossy(&got[..end]).to_ascii_lowercase();
        let length = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .map(|n| n.trim().parse::<usize>().expect("content length"))
            .unwrap_or(0);
        Some(end + length)
    }

    /// What the count is of is the TCP connection, not the request: two
    /// connections carrying three requests between them are two.
    #[tokio::test]
    async fn it_counts_every_accepted_connection() {
        let server = TestServer::spawn().await;
        server.set("/a", "one");
        server.set("/b", "two");
        let port = server.url("/a").port().expect("port");
        let talk = async {
            let mut first = TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("connect");
            exchange(&mut first, "GET /a HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").await;
            exchange(
                &mut first,
                "GET /b HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            )
            .await;
            let mut second = TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("connect");
            exchange(
                &mut second,
                "GET /a HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            )
            .await;
        };
        tokio::time::timeout(Duration::from_secs(5), talk)
            .await
            .expect("the two connections in time");
        assert_eq!(server.connections(), 2);
        assert_eq!(server.hits("/a"), 2);
        assert_eq!(server.hits("/b"), 1);
    }
}
