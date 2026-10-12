//! Internal HTTP client (M2 design §5.2): hyper's legacy client over any
//! `Connector`, TLS through rustls with native roots, HTTP/2 via ALPN, body
//! size limits, timeouts and GET redirects.

use crate::BoxFuture;
use crate::connector::{BoxedStream, ConnectOpts, Connector, Target};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use http::header::{AUTHORIZATION, LOCATION, USER_AGENT};
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, Uri};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::Incoming;
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::{TokioExecutor, TokioIo};
use percent_encoding::percent_decode_str;
use rurge_config::HostName;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::fmt;
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
    /// Sent with every request of the call, the ones a redirect leads to on
    /// another server included: a credential put here goes there too, unlike
    /// the user name and the password of the URL.
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

pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub final_url: Url,
}

/// Where it came from and how much it is. A body is whatever the server
/// sent, a header can be a cookie, and the user name, password, path and
/// query of a URL can be the token a provider issued.
impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("headers", &format_args!("{} fields", self.headers.len()))
            .field("body", &format_args!("{} bytes", self.body.len()))
            .field("final_url", &rurge_config::remote::origin(&self.final_url))
            .finish()
    }
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

/// A client configuration of `versions` that verifies the server with the
/// shared roots, or — `skip_verify` — not at all; no ALPN.
fn verifying_config(
    skip_verify: bool,
    versions: &[&'static rustls::SupportedProtocolVersion],
) -> Result<rustls::ClientConfig, HttpError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(versions)
        .map_err(|e| HttpError::Tls(e.to_string()))?;
    Ok(if skip_verify {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
            .with_no_client_auth()
    } else {
        builder
            .with_root_certificates(crate::tls::root_store())
            .with_no_client_auth()
    })
}

pub(crate) fn build_tls_config(skip_verify: bool) -> Result<rustls::ClientConfig, HttpError> {
    if skip_verify {
        tracing::warn!("TLS certificate verification is disabled for the internal HTTP client");
    }
    let mut config = verifying_config(skip_verify, rustls::DEFAULT_VERSIONS)?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

/// Shared TLS client configuration (native roots with webpki fallback, ALPN
/// h2 + http/1.1, optional verification bypass) for other transports (DoT).
pub fn tls_client_config(skip_verify: bool) -> Result<Arc<rustls::ClientConfig>, HttpError> {
    build_tls_config(skip_verify).map(Arc::new)
}

/// The TLS of a DNS server reached over QUIC (DoQ, DoH3): verified as
/// `tls_client_config` verifies, on TLS 1.3 alone — which QUIC requires —,
/// offering `alpn` alone.
pub fn quic_tls_client_config(
    skip_verify: bool,
    alpn: &str,
) -> Result<Arc<rustls::ClientConfig>, HttpError> {
    let mut config = verifying_config(skip_verify, &[&rustls::version::TLS13])?;
    config.alpn_protocols = vec![alpn.as_bytes().to_vec()];
    Ok(Arc::new(config))
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
                redirects += 1;
                url = redirected(&url, loc)?;
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
            // Not quoted: after a redirect the URL is the server's, not the
            // caller's
            return Err(HttpError::InvalidUrl("unsupported scheme".to_string()));
        }
        // A user name and a password are no part of what a server is asked
        // for: they go as Basic authentication. The target is the URL
        // without them, which is also what the connection pool calls a
        // connection in its log lines.
        let mut target = url.clone();
        let authorization = take_basic_authorization(&mut target);
        let uri: Uri = target
            .as_str()
            .parse()
            .map_err(|e: http::uri::InvalidUri| HttpError::InvalidUrl(e.to_string()))?;
        let mut b = Request::builder()
            .method(method.clone())
            .uri(uri)
            .header(USER_AGENT, self.user_agent.clone());
        // a caller that sends its own says who asks
        if let Some(authorization) = authorization
            && !opts.headers.iter().any(|(name, _)| name == AUTHORIZATION)
        {
            b = b.header(AUTHORIZATION, authorization);
        }
        for (k, v) in &opts.headers {
            b = b.header(k.clone(), v.clone());
        }
        b.body(Full::new(body))
            .map_err(|e| HttpError::Protocol(e.to_string()))
    }
}

/// Where a redirect from `url` goes, for what its `Location` says. Nothing
/// leads from HTTPS back to plain HTTP. The user name and the password of
/// `url` are for the server it names (scheme, host and port): a relative
/// reference keeps them, a target that names that server again is given them,
/// and one that names another goes without. A target that carries a user name
/// or a password of its own is used as the server wrote it.
fn redirected(url: &Url, location: &str) -> Result<Url, HttpError> {
    let mut next = url
        .join(location)
        .map_err(|e| HttpError::InvalidUrl(e.to_string()))?;
    if url.scheme() == "https" && next.scheme() != "https" {
        return Err(HttpError::Protocol(
            "refusing to redirect from https to http".to_string(),
        ));
    }
    if next.origin() == url.origin() && next.username().is_empty() && next.password().is_none() {
        // neither fails: a URL that has the origin of `url` has a host
        let _ = next.set_username(url.username());
        let _ = next.set_password(url.password());
    }
    Ok(next)
}

/// Takes the user name and the password out of `url` and returns what they
/// are sent as instead: `Basic base64(user:password)` (RFC 7617),
/// percent-decoded and marked as not to be shown. `None`, and the URL as it
/// was, when it has neither. HTTP has no other place for them: a request's
/// target takes none (RFC 9110 4.2.4), and HTTP/2 forbids them in
/// `:authority` (RFC 9113 8.3.1).
pub fn take_basic_authorization(url: &mut Url) -> Option<HeaderValue> {
    if url.username().is_empty() && url.password().is_none() {
        return None;
    }
    let mut pair = percent_decode_str(url.username()).collect::<Vec<u8>>();
    pair.push(b':');
    pair.extend(percent_decode_str(url.password().unwrap_or_default()));
    // a URL that has them has the authority they belong to: neither fails
    let _ = url.set_username("");
    let _ = url.set_password(None);
    let mut value = HeaderValue::from_str(&format!("Basic {}", STANDARD.encode(pair))).ok()?;
    value.set_sensitive(true);
    Some(value)
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

    /// A user name and a password in a URL go as `Authorization: Basic`
    /// (RFC 7617), percent-decoded, over HTTP/1 and HTTP/2 alike. A URL
    /// without them sends no such header.
    #[tokio::test]
    async fn a_urls_user_name_and_password_go_as_basic_authentication() {
        for tls in [false, true] {
            let server = if tls {
                TestServer::spawn_tls().await
            } else {
                TestServer::spawn().await
            };
            server.set("/a", "hello");
            let mut url = server.url("/a");
            url.set_username("al ice").unwrap();
            url.set_password(Some("s3cret:net")).unwrap();
            assert_eq!(url.username(), "al%20ice", "the URL holds them encoded");
            let c = client(tls);
            let resp = c.get(&url, &RequestOpts::default()).await.unwrap();
            assert_eq!(&resp.body[..], b"hello", "tls: {tls}");
            c.get(&server.url("/a"), &RequestOpts::default())
                .await
                .unwrap();
            let requests = server.requests();
            let version = if tls { "HTTP/2.0" } else { "HTTP/1.1" };
            assert_eq!(requests[0].version, version);
            assert_eq!(
                requests[0].header("authorization"),
                // base64("al ice:s3cret:net")
                Some("Basic YWwgaWNlOnMzY3JldDpuZXQ="),
                "tls: {tls}"
            );
            assert_eq!(requests[1].header("authorization"), None, "tls: {tls}");
            // HTTP/2 names the server in `:authority`, which is no header
            // field: it names the server and nothing else. An HTTP/1 request
            // names it in `Host`, and its target is a path.
            let named = format!("{}:{}", url.host_str().unwrap(), url.port().unwrap());
            for request in &requests {
                assert_eq!(
                    request.authority.as_deref(),
                    tls.then_some(named.as_str()),
                    "tls: {tls}"
                );
                for (name, value) in &request.headers {
                    assert!(
                        name == "authorization" || !value.contains("s3cret"),
                        "tls: {tls}: {name}: {value}"
                    );
                    assert!(!value.contains('@'), "tls: {tls}: {name}: {value}");
                }
            }
        }
    }

    /// The request that is built names the server alone. Its target is what
    /// the connection pool calls a connection in its log lines, and what
    /// HTTP/2 sends as `:authority` — where RFC 9113 8.3.1 allows no user
    /// name and password. The header that carries them is not shown.
    #[tokio::test]
    async fn the_target_of_a_request_has_no_user_name_and_password() {
        let c = client(false);
        let build = |url: &Url| {
            c.build(&Method::GET, url, Bytes::new(), &RequestOpts::default())
                .unwrap()
        };
        let url = Url::parse("https://us3r:s3cret@host.example:8443/dir/file?token=t").unwrap();
        let req = build(&url);
        assert_eq!(
            req.uri().to_string(),
            "https://host.example:8443/dir/file?token=t"
        );
        let authorization = req.headers().get("authorization").expect("sent as Basic");
        // base64("us3r:s3cret")
        assert_eq!(authorization, "Basic dXMzcjpzM2NyZXQ=");
        assert!(authorization.is_sensitive());
        let shown = format!("{req:?}");
        assert!(!shown.contains("dXMzcjpzM2NyZXQ"), "{shown}");
        assert!(!shown.contains("s3cret"), "{shown}");
        // a URL with neither is asked as it is written
        let plain = Url::parse("https://host.example:8443/dir/file?token=t").unwrap();
        let req = build(&plain);
        assert_eq!(req.uri().to_string(), plain.as_str());
        assert!(req.headers().get("authorization").is_none());
    }

    /// A caller that sends its own `Authorization` is the one that says who
    /// asks: the URL's user name and password leave the target, and are not
    /// sent beside it.
    #[tokio::test]
    async fn the_callers_own_authorization_is_the_one_sent() {
        let server = TestServer::spawn().await;
        server.set("/a", "hello");
        let mut url = server.url("/a");
        url.set_username("us3r").unwrap();
        url.set_password(Some("s3cret")).unwrap();
        let opts = RequestOpts {
            headers: vec![(
                http::header::AUTHORIZATION,
                HeaderValue::from_static("Bearer t0ken"),
            )],
            ..RequestOpts::default()
        };
        client(false).get(&url, &opts).await.unwrap();
        let sent: Vec<String> = server.requests()[0]
            .headers
            .iter()
            .filter(|(name, _)| name == "authorization")
            .map(|(_, value)| value.clone())
            .collect();
        assert_eq!(sent, ["Bearer t0ken"]);
    }

    /// A redirect takes the user name and password no further than the
    /// server they were written for: one that stays on it — by a relative
    /// reference, or by naming the server again — is followed with them, one
    /// that leaves is followed without.
    #[tokio::test]
    async fn a_redirect_takes_the_credentials_no_further_than_their_server() {
        let elsewhere = TestServer::spawn().await;
        elsewhere.set("/final", "elsewhere");
        let server = TestServer::spawn().await;
        server.set("/final", "here");
        for (path, location) in [
            ("/relative", "/final".to_string()),
            ("/named", server.url("/final").to_string()),
            ("/away", elsewhere.url("/final").to_string()),
        ] {
            server.set(path, "");
            server.set_status(path, 302);
            server.set_header(path, "location", &location);
        }
        let with_credentials = |path: &str| {
            let mut url = server.url(path);
            url.set_username("us3r").unwrap();
            url.set_password(Some("s3cret")).unwrap();
            url
        };
        let c = client(false);
        for (path, body) in [
            ("/relative", &b"here"[..]),
            ("/named", b"here"),
            ("/away", b"elsewhere"),
        ] {
            let resp = c
                .get(&with_credentials(path), &RequestOpts::default())
                .await
                .unwrap();
            assert_eq!(&resp.body[..], body, "{path}");
        }
        let requests = server.requests();
        assert_eq!(
            requests.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            ["/relative", "/final", "/named", "/final", "/away"]
        );
        for request in &requests {
            assert_eq!(
                request.header("authorization"),
                // base64("us3r:s3cret")
                Some("Basic dXMzcjpzM2NyZXQ="),
                "{}",
                request.path
            );
        }
        let requests = elsewhere.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].header("authorization"), None);
    }

    /// What `take_basic_authorization` took, as text: a value that is marked
    /// as not to be shown says nothing when an assertion fails.
    fn text(value: Option<HeaderValue>) -> Option<String> {
        value.map(|value| value.to_str().unwrap().to_string())
    }

    /// Where each form of `Location` leads from a URL with a user name and a
    /// password, and whether they go along. The server chooses the form: none
    /// may take them to a server the URL did not name.
    #[test]
    fn a_location_never_takes_the_credentials_to_another_server() {
        let from = Url::parse("https://us3r:s3cret@good.example/dir/x").unwrap();
        // Each is what `Location` says and where the next request goes. These
        // name the same server, and the credentials go along:
        let same = [
            // by a relative reference
            ("/y", "https://good.example/y"),
            ("y?q=1", "https://good.example/dir/y?q=1"),
            ("https:y", "https://good.example/dir/y"),
            // by naming it again
            ("https://good.example/y", "https://good.example/y"),
            ("https://good.example:443/y", "https://good.example/y"),
            ("HTTPS://GOOD.EXAMPLE/y", "https://good.example/y"),
            // a backslash ends the name of the server: what follows is a path
            // on it
            (
                "https://good.example\\@elsewhere.example/y",
                "https://good.example/@elsewhere.example/y",
            ),
        ];
        // These name another, however they write it:
        let another = [
            ("https://elsewhere.example/y", "https://elsewhere.example/y"),
            ("//elsewhere.example/y", "https://elsewhere.example/y"),
            ("/\\elsewhere.example/y", "https://elsewhere.example/y"),
            ("\\\\elsewhere.example/y", "https://elsewhere.example/y"),
            (
                "https:\\\\elsewhere.example/y",
                "https://elsewhere.example/y",
            ),
            // another port of the same host is another server
            ("https://good.example:8443/y", "https://good.example:8443/y"),
            // and so is a name that differs, by a label or by a dot at its end
            ("https://sub.good.example/y", "https://sub.good.example/y"),
            ("https://good.example./y", "https://good.example./y"),
        ];
        for (cases, with_credentials) in [(&same[..], true), (&another[..], false)] {
            for &(location, goes_to) in cases {
                let mut next = redirected(&from, location).unwrap();
                let authorization = text(take_basic_authorization(&mut next));
                assert_eq!(next.as_str(), goes_to, "{location}");
                // base64("us3r:s3cret")
                let expected = with_credentials.then_some("Basic dXMzcjpzM2NyZXQ=");
                assert_eq!(authorization.as_deref(), expected, "{location}");
            }
        }
        // The same host over HTTPS is another server than over plain HTTP: the
        // step up is taken without them.
        let plain = Url::parse("http://us3r:s3cret@good.example/x").unwrap();
        let mut next = redirected(&plain, "https://good.example/y").unwrap();
        assert_eq!(text(take_basic_authorization(&mut next)), None);
        assert_eq!(next.as_str(), "https://good.example/y");
        // And nothing leads from HTTPS back to plain HTTP.
        let back = redirected(&from, "http://good.example/y");
        assert!(matches!(back, Err(HttpError::Protocol(_))), "{back:?}");
    }

    /// A `Location` that carries a user name and a password of its own is
    /// used as the server wrote it. What it wrote is not the user's, and the
    /// user's are not put in its place.
    #[test]
    fn a_location_with_credentials_of_its_own_is_used_as_written() {
        let from = Url::parse("https://us3r:s3cret@good.example/x").unwrap();
        // what `Location` says, the server that is asked next, what it is
        // sent as Basic
        for (location, host, basic) in [
            // base64("other:pw")
            (
                "https://other:pw@good.example/y",
                "good.example",
                "b3RoZXI6cHc=",
            ),
            // The last `@` ends the user name: these two name another server
            // and call the first one a user. base64("good.example:") and
            // base64("good.example:443").
            (
                "https://good.example@elsewhere.example/y",
                "elsewhere.example",
                "Z29vZC5leGFtcGxlOg==",
            ),
            (
                "https://good.example:443@elsewhere.example/y",
                "elsewhere.example",
                "Z29vZC5leGFtcGxlOjQ0Mw==",
            ),
        ] {
            let mut next = redirected(&from, location).unwrap();
            assert_eq!(next.host_str(), Some(host), "{location}");
            let authorization = text(take_basic_authorization(&mut next));
            assert_eq!(authorization, Some(format!("Basic {basic}")), "{location}");
        }
    }

    /// A user name alone, a password alone, and bytes that are no text: each
    /// goes as the bytes it names, in the alphabet of RFC 4648 section 4.
    #[test]
    fn credentials_go_as_the_bytes_they_name() {
        for (url, basic) in [
            // base64("user:")
            ("https://user@host.example/a", "dXNlcjo="),
            // base64(":pw")
            ("https://:pw@host.example/a", "OnB3"),
            // base64 of `u:` and three bytes 0xff: `/` is in that alphabet,
            // and not in the one made for URLs
            ("https://u:%ff%ff%ff@host.example/a", "dTr///8="),
        ] {
            let mut url = Url::parse(url).unwrap();
            let authorization = text(take_basic_authorization(&mut url));
            assert_eq!(authorization, Some(format!("Basic {basic}")));
            assert_eq!(url.as_str(), "https://host.example/a");
        }
    }

    /// The `Debug` of a response says where it came from and how much it is.
    /// A body is whatever the server sent, a header can be a cookie, and the
    /// user name, password, path and query of a URL can be a token.
    #[tokio::test]
    async fn the_debug_of_a_response_shows_no_more_than_where_it_came_from() {
        let server = TestServer::spawn().await;
        server.set("/PATHMARK", "BODYMARK");
        server.set_header("/PATHMARK", "x-token", "HEADMARK");
        let mut url = server.url("/PATHMARK");
        url.set_query(Some("key=QUERYMARK"));
        url.set_username("USERMARK").unwrap();
        url.set_password(Some("PASSMARK")).unwrap();
        let resp = client(false)
            .get(&url, &RequestOpts::default())
            .await
            .unwrap();
        assert_eq!(&resp.body[..], b"BODYMARK");
        assert_eq!(resp.headers.get("x-token").unwrap(), "HEADMARK");
        let shown = format!("{resp:?}");
        for mark in [
            "USERMARK",
            "PASSMARK",
            "PATHMARK",
            "QUERYMARK",
            "BODYMARK",
            "HEADMARK",
        ] {
            assert!(!shown.contains(mark), "{mark}: {shown}");
        }
        assert!(shown.contains("200"), "{shown}");
        let from = format!("http://127.0.0.1:{}/", url.port().unwrap());
        assert!(shown.contains(&from), "{shown}");
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

    /// A DNS server over QUIC gets TLS 1.3 alone, and the one ALPN of its
    /// protocol.
    #[test]
    fn quic_tls_client_config_is_tls_1_3_with_one_alpn() {
        for skip_verify in [false, true] {
            let cfg = quic_tls_client_config(skip_verify, "doq").unwrap();
            assert_eq!(cfg.alpn_protocols, vec![b"doq".to_vec()]);
            let debug = format!("{cfg:?}");
            assert!(debug.contains("versions: [TLSv1_3]"), "{debug}");
        }
    }
}
