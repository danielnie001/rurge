//! What tests share: an in-process HTTP(S) server (programmable routes, ETag /
//! 304, delays and request recording), a resource cache with a copy already in
//! it, and a capture of the log. Never used by production code.

use crate::resource::{CacheDir, Meta};
use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
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

impl Lines {
    /// Routes the events of this thread here until the guard is dropped.
    /// While one dispatcher alone is registered, tracing-core asks only the
    /// thread that first reaches a call site whether it is wanted: a site
    /// another test reaches first would stay shut on this thread too. With a
    /// second one it asks every dispatcher, which the second element of the
    /// result is for: it is kept for as long as the guard.
    pub fn capture(&self) -> (tracing::subscriber::DefaultGuard, tracing::Dispatch) {
        (
            tracing::subscriber::set_default(self.clone()),
            tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()),
        )
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
