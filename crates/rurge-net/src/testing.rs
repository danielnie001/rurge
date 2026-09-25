//! In-process HTTP(S) server for tests: programmable routes, ETag / 304,
//! delays and request recording. Never used by production code.

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
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;
use url::Url;

#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub version: String,
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
    let etag = etag_of(&route.body);
    let if_none_match = req
        .headers()
        .get("if-none-match")
        .and_then(|v| v.to_str().ok());
    let mut builder = Response::builder();
    for (k, v) in &route.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    if route.status == 200 && if_none_match == Some(etag.as_str()) {
        return Ok(builder
            .status(304)
            .header("etag", etag)
            .body(Full::new(Bytes::new()))
            .unwrap());
    }
    let status = StatusCode::from_u16(route.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if status == StatusCode::OK {
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
