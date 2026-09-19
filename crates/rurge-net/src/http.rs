//! Internal HTTP client (M2 design §5.2): hyper's legacy client over any
//! `Connector`, TLS through rustls with native roots, HTTP/2 via ALPN, body
//! size limits, timeouts and GET redirects.

use crate::BoxFuture;
use crate::connector::{BoxedStream, ConnectOpts, Connector, Target};
use bytes::Bytes;
use http::header::{LOCATION, USER_AGENT};
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, Uri};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::Incoming;
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rurge_config::HostName;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio_rustls::TlsConnector;
use url::Url;

#[derive(Clone, Debug)]
pub struct HttpClientConfig {
    pub user_agent: String,
    pub skip_cert_verification: bool,
    pub connect_timeout: Duration,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        HttpClientConfig {
            user_agent: format!("rurge/{}", env!("CARGO_PKG_VERSION")),
            skip_cert_verification: false,
            connect_timeout: Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RequestOpts {
    pub timeout: Duration,
    pub max_body: u64,
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub follow_redirects: u8,
}

impl Default for RequestOpts {
    fn default() -> Self {
        RequestOpts {
            timeout: Duration::from_secs(30),
            max_body: 64 * 1024 * 1024,
            headers: Vec::new(),
            follow_redirects: 5,
        }
    }
}

#[derive(Debug)]
pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub final_url: Url,
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("connect: {0}")]
    Connect(String),
    #[error("tls: {0}")]
    Tls(String),
    #[error("timeout")]
    Timeout,
    #[error("body exceeds {0} bytes")]
    TooLarge(u64),
    #[error("http status {0}")]
    Status(StatusCode),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("protocol: {0}")]
    Protocol(String),
}

/// A connected stream as hyper sees it.
pub struct HyperStream {
    io: TokioIo<BoxedStream>,
    h2: bool,
}

impl Read for HyperStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_read(cx, buf)
    }
}

impl Write for HyperStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().io).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_shutdown(cx)
    }
}

impl Connection for HyperStream {
    fn connected(&self) -> Connected {
        let c = Connected::new();
        if self.h2 { c.negotiated_h2() } else { c }
    }
}

#[derive(Clone)]
struct HyperConnector {
    connector: Arc<dyn Connector>,
    tls: Arc<rustls::ClientConfig>,
    timeout: Duration,
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl tower_service::Service<Uri> for HyperConnector {
    type Response = HyperStream;
    type Error = BoxError;
    type Future = BoxFuture<'static, Result<HyperStream, BoxError>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let connector = self.connector.clone();
        let tls = self.tls.clone();
        let timeout = self.timeout;
        Box::pin(async move {
            let host = uri
                .host()
                .ok_or("url has no host")?
                .trim_matches(|c| c == '[' || c == ']')
                .to_string();
            let https = uri.scheme_str() == Some("https");
            let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
            let target = Target::new(HostName::parse(&host), port);
            let opts = ConnectOpts { timeout };
            let stream = connector.connect(&target, &opts).await?;
            if !https {
                return Ok(HyperStream {
                    io: TokioIo::new(stream),
                    h2: false,
                });
            }
            let name = ServerName::try_from(host.clone())
                .map_err(|e| format!("invalid server name `{host}`: {e}"))?;
            let tls_stream = TlsConnector::from(tls).connect(name, stream).await?;
            let h2 = tls_stream.get_ref().1.alpn_protocol() == Some(b"h2");
            Ok(HyperStream {
                io: TokioIo::new(Box::new(tls_stream) as BoxedStream),
                h2,
            })
        })
    }
}

/// Accepts any certificate (`encrypted-dns-skip-cert-verification`); logged as insecure.
#[derive(Debug)]
struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub(crate) fn build_tls_config(skip_verify: bool) -> Result<rustls::ClientConfig, HttpError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| HttpError::Tls(e.to_string()))?;
    let mut config = if skip_verify {
        tracing::warn!("TLS certificate verification is disabled for the internal HTTP client");
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        let native = rustls_native_certs::load_native_certs();
        let (added, _ignored) = roots.add_parsable_certificates(native.certs);
        if added == 0 {
            tracing::warn!(
                errors = native.errors.len(),
                "no native root certificates loaded; using webpki-roots"
            );
            roots
                .roots
                .extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

/// Shared TLS client configuration (native roots with webpki fallback, ALPN
/// h2 + http/1.1, optional verification bypass) for other transports (DoT).
pub fn tls_client_config(skip_verify: bool) -> Result<Arc<rustls::ClientConfig>, HttpError> {
    build_tls_config(skip_verify).map(Arc::new)
}

pub struct HttpClient {
    client: Client<HyperConnector, Full<Bytes>>,
    user_agent: HeaderValue,
}

impl HttpClient {
    pub fn new(
        connector: Arc<dyn Connector>,
        cfg: HttpClientConfig,
    ) -> Result<HttpClient, HttpError> {
        let tls = tls_client_config(cfg.skip_cert_verification)?;
        let hc = HyperConnector {
            connector,
            tls,
            timeout: cfg.connect_timeout,
        };
        let client = Client::builder(TokioExecutor::new()).build::<_, Full<Bytes>>(hc);
        let user_agent = HeaderValue::from_str(&cfg.user_agent)
            .map_err(|e| HttpError::Protocol(e.to_string()))?;
        Ok(HttpClient { client, user_agent })
    }

    pub async fn get(&self, url: &Url, opts: &RequestOpts) -> Result<Response, HttpError> {
        self.request(Method::GET, url, Bytes::new(), opts).await
    }

    pub async fn post(
        &self,
        url: &Url,
        body: Bytes,
        opts: &RequestOpts,
    ) -> Result<Response, HttpError> {
        self.request(Method::POST, url, body, opts).await
    }

    /// Sends a prepared request and returns the streaming response (no redirects, no body limit).
    pub async fn send(
        &self,
        req: Request<Full<Bytes>>,
        timeout: Duration,
    ) -> Result<http::Response<Incoming>, HttpError> {
        tokio::time::timeout(timeout, self.client.request(req))
            .await
            .map_err(|_| HttpError::Timeout)?
            .map_err(map_client_error)
    }

    async fn request(
        &self,
        method: Method,
        url: &Url,
        body: Bytes,
        opts: &RequestOpts,
    ) -> Result<Response, HttpError> {
        let deadline = tokio::time::Instant::now() + opts.timeout;
        let mut url = url.clone();
        let mut redirects = 0u8;
        loop {
            let req = self.build(&method, &url, body.clone(), opts)?;
            let resp = tokio::time::timeout_at(deadline, self.client.request(req))
                .await
                .map_err(|_| HttpError::Timeout)?
                .map_err(map_client_error)?;
            let status = resp.status();
            if status.is_redirection()
                && method == Method::GET
                && let Some(loc) = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok())
            {
                if redirects >= opts.follow_redirects {
                    return Err(HttpError::TooManyRedirects);
                }
                let next = url
                    .join(loc)
                    .map_err(|e| HttpError::InvalidUrl(e.to_string()))?;
                if url.scheme() == "https" && next.scheme() != "https" {
                    return Err(HttpError::Protocol(
                        "refusing to redirect from https to http".to_string(),
                    ));
                }
                redirects += 1;
                url = next;
                continue;
            }
            let headers = resp.headers().clone();
            let limit = usize::try_from(opts.max_body).unwrap_or(usize::MAX);
            let collected =
                tokio::time::timeout_at(deadline, Limited::new(resp.into_body(), limit).collect())
                    .await
                    .map_err(|_| HttpError::Timeout)?
                    .map_err(|e| {
                        if e.downcast_ref::<LengthLimitError>().is_some() {
                            HttpError::TooLarge(opts.max_body)
                        } else {
                            HttpError::Protocol(e.to_string())
                        }
                    })?;
            return Ok(Response {
                status,
                headers,
                body: collected.to_bytes(),
                final_url: url,
            });
        }
    }

    fn build(
        &self,
        method: &Method,
        url: &Url,
        body: Bytes,
        opts: &RequestOpts,
    ) -> Result<Request<Full<Bytes>>, HttpError> {
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(HttpError::InvalidUrl(format!(
                "unsupported scheme `{}`",
                url.scheme()
            )));
        }
        let uri: Uri = url
            .as_str()
            .parse()
            .map_err(|e: http::uri::InvalidUri| HttpError::InvalidUrl(e.to_string()))?;
        let mut b = Request::builder()
            .method(method.clone())
            .uri(uri)
            .header(USER_AGENT, self.user_agent.clone());
        for (k, v) in &opts.headers {
            b = b.header(k.clone(), v.clone());
        }
        b.body(Full::new(body))
            .map_err(|e| HttpError::Protocol(e.to_string()))
    }
}

fn map_client_error(e: hyper_util::client::legacy::Error) -> HttpError {
    let text = e.to_string();
    if e.is_connect() {
        HttpError::Connect(text)
    } else {
        HttpError::Protocol(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::{DirectConnector, SystemResolve};
    use crate::testing::TestServer;

    fn client(skip_verify: bool) -> HttpClient {
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        HttpClient::new(
            connector,
            HttpClientConfig {
                skip_cert_verification: skip_verify,
                ..HttpClientConfig::default()
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn get_returns_body_status_headers_and_sends_user_agent() {
        let server = TestServer::spawn().await;
        server.set("/a", "hello");
        let resp = client(false)
            .get(&server.url("/a"), &RequestOpts::default())
            .await
            .unwrap();
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(&resp.body[..], b"hello");
        assert!(resp.headers.get("etag").is_some());
        let req = &server.requests()[0];
        assert!(req.header("user-agent").unwrap().starts_with("rurge/"));
        assert_eq!(req.version, "HTTP/1.1");
    }

    #[tokio::test]
    async fn https_with_skipped_verification_negotiates_h2() {
        let server = TestServer::spawn_tls().await;
        server.set("/tls", "secure");
        let resp = client(true)
            .get(&server.url("/tls"), &RequestOpts::default())
            .await
            .unwrap();
        assert_eq!(&resp.body[..], b"secure");
        assert_eq!(server.requests()[0].version, "HTTP/2.0");
        let err = client(false)
            .get(&server.url("/tls"), &RequestOpts::default())
            .await
            .err()
            .unwrap();
        assert!(
            matches!(
                err,
                HttpError::Connect(_) | HttpError::Tls(_) | HttpError::Protocol(_)
            ),
            "{err}"
        );
    }

    #[tokio::test]
    async fn conditional_get_returns_304() {
        let server = TestServer::spawn().await;
        server.set("/c", "v1");
        let c = client(false);
        let first = c
            .get(&server.url("/c"), &RequestOpts::default())
            .await
            .unwrap();
        let etag = first.headers.get("etag").unwrap().clone();
        let opts = RequestOpts {
            headers: vec![(http::header::IF_NONE_MATCH, etag)],
            ..RequestOpts::default()
        };
        let second = c.get(&server.url("/c"), &opts).await.unwrap();
        assert_eq!(second.status, StatusCode::NOT_MODIFIED);
        assert!(second.body.is_empty());
    }

    #[tokio::test]
    async fn redirects_are_followed_for_get_up_to_the_limit() {
        let server = TestServer::spawn().await;
        server.set("/final", "done");
        server.set("/r1", "");
        server.set_status("/r1", 302);
        server.set_header("/r1", "location", "/final");
        let resp = client(false)
            .get(&server.url("/r1"), &RequestOpts::default())
            .await
            .unwrap();
        assert_eq!(&resp.body[..], b"done");
        assert!(resp.final_url.path().ends_with("/final"));
        server.set("/loop", "");
        server.set_status("/loop", 302);
        server.set_header("/loop", "location", "/loop");
        let err = client(false)
            .get(&server.url("/loop"), &RequestOpts::default())
            .await
            .err()
            .unwrap();
        assert!(matches!(err, HttpError::TooManyRedirects));
    }

    #[tokio::test]
    async fn body_limit_timeout_and_connect_errors() {
        let server = TestServer::spawn().await;
        server.set("/big", vec![b'x'; 1000]);
        let err = client(false)
            .get(
                &server.url("/big"),
                &RequestOpts {
                    max_body: 10,
                    ..RequestOpts::default()
                },
            )
            .await
            .err()
            .unwrap();
        assert!(matches!(err, HttpError::TooLarge(10)), "{err}");
        server.set("/slow", "zzz");
        server.set_delay("/slow", Duration::from_secs(3));
        let err = client(false)
            .get(
                &server.url("/slow"),
                &RequestOpts {
                    timeout: Duration::from_millis(200),
                    ..RequestOpts::default()
                },
            )
            .await
            .err()
            .unwrap();
        assert!(matches!(err, HttpError::Timeout), "{err}");
        let dead = Url::parse("http://127.0.0.1:1/").unwrap();
        let err = client(false)
            .get(&dead, &RequestOpts::default())
            .await
            .err()
            .unwrap();
        assert!(matches!(err, HttpError::Connect(_)), "{err}");
        let ftp = Url::parse("ftp://example.com/x").unwrap();
        assert!(matches!(
            client(false).get(&ftp, &RequestOpts::default()).await,
            Err(HttpError::InvalidUrl(_))
        ));
    }

    #[tokio::test]
    async fn post_sends_the_body() {
        let server = TestServer::spawn().await;
        server.set("/p", "ok");
        let resp = client(false)
            .post(
                &server.url("/p"),
                Bytes::from_static(b"payload"),
                &RequestOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(server.requests()[0].method, "POST");
    }

    #[test]
    fn tls_client_config_advertises_h2_and_http1() {
        let cfg = tls_client_config(false).unwrap();
        assert_eq!(
            cfg.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
        assert!(tls_client_config(true).is_ok());
    }
}
