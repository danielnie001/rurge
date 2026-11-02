//! The phase 4 / M5b benchmarks (phase 4 M5 design 8): what one loopback
//! request through the engine costs, in the six measurements of design 8.2 --
//! 8.7. B1 is plain HTTP/1.1 through the engine, its throughput and its
//! per-request latency; B2 is the same after MITM decryption; B3 is a thousand
//! rules that do not match under every request; B4 is the leaf certificate
//! MITM issues; B5 is the resident memory of idle connections, of a streamed
//! body and of a full leaf cache; Q1 is reading a QUIC ClientHello. They are
//! measurements, not gates: every one is `#[ignore]`d, so CI compiles them and
//! runs none of them.
//!
//! Run one at a time, in release, each in its own process (B5 reads the peak
//! of the process, and the peak only grows):
//!
//! ```text
//! cargo test -p rurge-engine --release --test bench -- --ignored --exact <name> --nocapture --test-threads=1
//! ```
//!
//! `-p` and `--release` are the exception `docs/testing.md` already records
//! for `relay_throughput`: only the engine and its dependencies are built a
//! second time, in release. Check first that the disk holding `target/` has
//! more than 20 GiB free.
//! `HTTP_BENCH_QUICK=1` shrinks every measurement in a debug build, to see
//! that it runs, not to read a number.

mod common;
use common::*;

use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::{Body, Bytes, Frame, Incoming, SizeHint};
use hyper::client::conn::{http1, http2};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode, Version, header};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Instant;

/// The `User-Agent` every request of these benchmarks carries, so that a
/// `USER-AGENT` rule can be matched on (phase 4 M5 design 8.4).
const USER_AGENT: &str = "rurge-bench/1";

/// One frame's worth of `Repeat`.
const BLOCK: usize = 64 * 1024;

/// The sizes the benchmarks run at (phase 4 M5 design 8.1 -- 8.7). The
/// default is the design's; `HTTP_BENCH_QUICK` set to any non-empty value picks
/// the smaller one, which exists to see that a benchmark runs, not to read a
/// number.
struct Scale {
    rounds: usize,
    body: u64,
    requests: usize,
    warm_up: usize,
    names: usize,
    connections: usize,
    iterations: usize,
    quick: bool,
}

fn scale() -> Scale {
    let quick = std::env::var_os("HTTP_BENCH_QUICK").is_some_and(|v| !v.is_empty());
    if quick {
        Scale {
            rounds: 1,
            body: 8 * 1024 * 1024,
            requests: 500,
            warm_up: 20,
            names: 50,
            connections: 50,
            iterations: 2_000,
            quick,
        }
    } else {
        Scale {
            rounds: 5,
            body: 256 * 1024 * 1024,
            requests: 20_000,
            warm_up: 200,
            names: 1_000,
            connections: 1_000,
            iterations: 100_000,
            quick,
        }
    }
}

impl Scale {
    /// The build and the sizes, for the first line of a benchmark.
    fn label(&self) -> String {
        format!(
            "{} {} rounds={} body={} MiB requests={} warm_up={} names={} connections={} iterations={}",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            if self.quick { "quick" } else { "full" },
            self.rounds,
            self.body / (1024 * 1024),
            self.requests,
            self.warm_up,
            self.names,
            self.connections,
            self.iterations,
        )
    }
}

/// A distribution of durations, from the samples of one measurement.
struct Latency {
    median: Duration,
    p99: Duration,
    max: Duration,
}

/// Sorts `samples` and reads the median (the upper one for an even count), the
/// p99 by nearest rank and the largest (phase 4 M5 design 8.1).
fn latency(samples: &mut [Duration]) -> Latency {
    assert!(!samples.is_empty(), "no latency samples were timed");
    samples.sort_unstable();
    let n = samples.len();
    Latency {
        median: samples[n / 2],
        p99: samples[(n * 99).div_ceil(100) - 1],
        max: samples[n - 1],
    }
}

/// The upper median of the rounds' throughputs (phase 4 M5 design 8.1).
fn median(values: &[f64]) -> f64 {
    assert!(!values.is_empty(), "no throughput samples were taken");
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

fn gbit_per_s(bytes: u64, took: Duration) -> f64 {
    bytes as f64 * 8.0 / took.as_secs_f64() / 1e9
}

fn micros(took: Duration) -> f64 {
    took.as_secs_f64() * 1e6
}

/// `len` bytes of `b'a'`, handed on as slices of one 64 KiB block: the body of
/// an upload and of a download, built once and never accumulated (phase 4 M5
/// design 8.1).
struct Repeat {
    left: u64,
    chunk: Bytes,
}

impl Repeat {
    fn new(len: u64) -> Repeat {
        Repeat {
            left: len,
            chunk: Bytes::from(vec![b'a'; BLOCK]),
        }
    }
}

impl Body for Repeat {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if this.left == 0 {
            return Poll::Ready(None);
        }
        let n = this.left.min(BLOCK as u64) as usize;
        let frame = this.chunk.slice(..n);
        this.left -= n as u64;
        Poll::Ready(Some(Ok(Frame::data(frame))))
    }

    fn is_end_stream(&self) -> bool {
        self.left == 0
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.left)
    }
}

/// The answer's body of the origin: a streamed `Repeat`, a small `Full`, or
/// none.
type BoxedBody = BoxBody<Bytes, Infallible>;

/// The loopback origin of these benchmarks and the requests it served, by
/// version. Every path is answered as HTTP/1.1 or HTTP/2, whichever the
/// connection was.
struct Origin {
    addr: SocketAddr,
    h1: AtomicU64,
    h2: AtomicU64,
}

impl Origin {
    /// The requests it served, `(HTTP/1.1, HTTP/2)`.
    fn counts(&self) -> (u64, u64) {
        (
            self.h1.load(Ordering::Relaxed),
            self.h2.load(Ordering::Relaxed),
        )
    }

    /// `GET /down`: a `down`-byte body; `GET /small`: 16 bytes;
    /// `POST /up`: the request body counted, answered `"{bytes} {first}"`;
    /// anything else: 404 (phase 4 M5 design 8.2).
    async fn serve(&self, req: Request<Incoming>, down: u64) -> Response<BoxedBody> {
        if req.version() == Version::HTTP_2 {
            self.h2.fetch_add(1, Ordering::Relaxed);
        } else {
            self.h1.fetch_add(1, Ordering::Relaxed);
        }
        let method = req.method().clone();
        let path = req.uri().path().to_owned();
        match (method.as_str(), path.as_str()) {
            ("GET", "/down") => Response::builder()
                .status(200)
                .body(Repeat::new(down).boxed())
                .unwrap(),
            ("GET", "/small") => Response::builder()
                .status(200)
                .body(Full::new(Bytes::from_static(b"0123456789abcdef")).boxed())
                .unwrap(),
            ("POST", "/up") => {
                let mut bytes = 0u64;
                let mut first: Option<u8> = None;
                let mut body = req.into_body();
                while let Some(frame) = body.frame().await {
                    let frame = frame.unwrap();
                    if let Some(data) = frame.data_ref() {
                        if first.is_none() {
                            first = data.first().copied();
                        }
                        bytes += data.len() as u64;
                    }
                }
                let line = match first {
                    Some(byte) => format!("{bytes} {byte}"),
                    None => format!("{bytes} -"),
                };
                Response::builder()
                    .status(200)
                    .body(Full::new(Bytes::from(line)).boxed())
                    .unwrap()
            }
            _ => Response::builder()
                .status(404)
                .body(Full::new(Bytes::new()).boxed())
                .unwrap(),
        }
    }
}

/// Binds a loopback origin; `tls` is offered before the HTTP connection is
/// served when given. The auto builder gets no timer: hyper then leaves its
/// default head-read timeout inactive (hyper 1.11.1 `src/common/time.rs`'s
/// `check`), so an idle connection of B5 is not closed by the origin.
async fn origin(tls: Option<tokio_rustls::TlsAcceptor>, down: u64) -> Arc<Origin> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let origin = Arc::new(Origin {
        addr,
        h1: AtomicU64::new(0),
        h2: AtomicU64::new(0),
    });
    let accepted = origin.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            stream.set_nodelay(true).unwrap();
            let tls = tls.clone();
            let origin = accepted.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let origin = origin.clone();
                    async move { Ok::<_, Infallible>(origin.serve(req, down).await) }
                });
                let builder = Builder::new(TokioExecutor::new());
                match tls {
                    Some(acceptor) => {
                        if let Ok(stream) = acceptor.accept(stream).await {
                            let _ = builder
                                .serve_connection(TokioIo::new(stream), service)
                                .await;
                        }
                    }
                    None => {
                        let _ = builder
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    }
                }
            });
        }
    });
    origin
}

/// A client connection to `addr` (or to rurge, or through a tunnel), handshook
/// as HTTP/1.1 or HTTP/2. The HTTP/2 half is used by B2 in this file.
enum Client {
    H1(http1::SendRequest<Repeat>),
    H2(http2::SendRequest<Repeat>),
}

async fn h1_client<I>(io: I) -> Client
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (sender, conn) = http1::handshake(TokioIo::new(io)).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    Client::H1(sender)
}

async fn h2_client<I>(io: I) -> Client
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (sender, conn) = http2::handshake(TokioExecutor::new(), TokioIo::new(io))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    Client::H2(sender)
}

/// A loopback TCP stream with `TCP_NODELAY`, as rurge's listeners and
/// `relay_throughput` set it.
async fn plain_stream(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).await.unwrap();
    stream.set_nodelay(true).unwrap();
    stream
}

/// Waits until the connection can take another request: hyper's `SendRequest`
/// wants the previous answer read and the connection empty (phase 4 M5 design
/// 8.1).
async fn ready(client: &mut Client) {
    match client {
        Client::H1(sender) => sender.ready().await.unwrap(),
        Client::H2(sender) => sender.ready().await.unwrap(),
    }
}

async fn send_request(client: &mut Client, req: Request<Repeat>) -> Response<Incoming> {
    match client {
        Client::H1(sender) => sender.send_request(req).await.unwrap(),
        Client::H2(sender) => sender.send_request(req).await.unwrap(),
    }
}

async fn send(client: &mut Client, req: Request<Repeat>) -> Response<Incoming> {
    ready(client).await;
    send_request(client, req).await
}

/// `host` is the `Host` header; `None` leaves it out, for the HTTP/2 requests
/// whose `:authority` comes from an absolute URI (phase 4 M5 design 8.3).
fn get(uri: &str, host: Option<&str>) -> Request<Repeat> {
    request("GET", uri, host, 0)
}

/// As `get`, for a `POST` of `len` bytes.
fn post(uri: &str, host: Option<&str>, len: u64) -> Request<Repeat> {
    request("POST", uri, host, len)
}

fn request(method: &str, uri: &str, host: Option<&str>, len: u64) -> Request<Repeat> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::USER_AGENT, USER_AGENT);
    if let Some(host) = host {
        builder = builder.header(header::HOST, host);
    }
    builder.body(Repeat::new(len)).unwrap()
}

/// Sends `req` and reads the answer frame by frame, counting the body and
/// keeping none of it.
async fn fetch(client: &mut Client, req: Request<Repeat>) -> (StatusCode, u64) {
    let response = send(client, req).await;
    let status = response.status();
    let mut bytes = 0u64;
    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
        let frame = frame.unwrap();
        if let Some(data) = frame.data_ref() {
            bytes += data.len() as u64;
        }
    }
    (status, bytes)
}

/// Sends a `POST` and reads the origin's short answer line, which is the only
/// body anything here accumulates. Used by B5 in this file.
async fn upload(client: &mut Client, req: Request<Repeat>) -> String {
    let response = send(client, req).await;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(body.to_vec()).unwrap()
}

/// Sends `req` and reads its answer frame by frame, timing from after the
/// connection was ready (before the request is written) until the body has
/// been read.
async fn timed_fetch(client: &mut Client, req: Request<Repeat>) -> (StatusCode, u64, Duration) {
    ready(client).await;
    let started = Instant::now();
    let response = send_request(client, req).await;
    let status = response.status();
    let mut bytes = 0u64;
    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
        let frame = frame.unwrap();
        if let Some(data) = frame.data_ref() {
            bytes += data.len() as u64;
        }
    }
    (status, bytes, started.elapsed())
}

/// Sends a `POST` and reads its answer line, timed as `timed_fetch` is.
async fn timed_upload(client: &mut Client, req: Request<Repeat>) -> (String, Duration) {
    ready(client).await;
    let started = Instant::now();
    let response = send_request(client, req).await;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (String::from_utf8(body.to_vec()).unwrap(), started.elapsed())
}

/// `warm_up` untimed `GET /small`s, then `count` timed ones, each timed from
/// after the connection was ready to the body's last frame (phase 4 M5 design
/// 8.1). Every answer is asserted to be the 16 bytes it should be.
async fn timed_gets(
    client: &mut Client,
    uri: &str,
    host: Option<&str>,
    warm_up: usize,
    count: usize,
) -> Vec<Duration> {
    for _ in 0..warm_up {
        let (status, bytes) = fetch(client, get(uri, host)).await;
        assert_eq!(status, StatusCode::OK, "the warm-up answer");
        assert_eq!(bytes, 16, "the warm-up body");
    }
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        let (status, bytes, took) = timed_fetch(client, get(uri, host)).await;
        assert_eq!(status, StatusCode::OK, "the timed answer");
        assert_eq!(bytes, 16, "the timed body");
        samples.push(took);
    }
    samples
}

/// A thousand rules, none of which matches `origin.test` or `rurge-bench/1`
/// (phase 4 M5 design 8.4): 250 of each kind, in that order, all `REJECT`.
fn thousand_rules() -> String {
    let mut rules = String::new();
    for i in 0..250 {
        rules.push_str(&format!("DOMAIN-SUFFIX,nomatch-{i:03}.test,REJECT\n"));
    }
    for i in 0..250 {
        rules.push_str(&format!("DOMAIN-KEYWORD,nomatch-kw-{i:03},REJECT\n"));
    }
    for i in 0..250 {
        rules.push_str(&format!(r"URL-REGEX,^http://nomatch-{i:03}\.test/,REJECT"));
        rules.push('\n');
    }
    for i in 0..250 {
        rules.push_str(&format!("USER-AGENT,nomatch-agent-{i:03}*,REJECT\n"));
    }
    rules
}

/// One new connection through `addr`: `GET /down` then `POST /up`, each timed.
/// `absolute` writes the URI in the absolute form the HTTP listener's requests
/// always take.
async fn b1_connection(addr: SocketAddr, absolute: bool, host: &str, body: u64) -> (f64, f64) {
    let mut client = h1_client(plain_stream(addr).await).await;
    let down_uri = if absolute {
        format!("http://{host}/down")
    } else {
        "/down".to_owned()
    };
    let (status, got, took) = timed_fetch(&mut client, get(&down_uri, Some(host))).await;
    assert_eq!(status, StatusCode::OK, "the download's answer");
    assert_eq!(got, body, "the downloaded bytes");
    let down = gbit_per_s(got, took);

    let up_uri = if absolute {
        format!("http://{host}/up")
    } else {
        "/up".to_owned()
    };
    let (line, took) = timed_upload(&mut client, post(&up_uri, Some(host), body)).await;
    assert_eq!(
        line,
        format!("{body} 97"),
        "the origin's answer to the upload"
    );
    let up = gbit_per_s(body, took);
    (down, up)
}

/// One pass on a new connection: `timed_gets` through `addr`.
async fn b1_latency_pass(
    addr: SocketAddr,
    absolute: bool,
    host: &str,
    warm_up: usize,
    requests: usize,
) -> Vec<Duration> {
    let mut client = h1_client(plain_stream(addr).await).await;
    let uri = if absolute {
        format!("http://{host}/small")
    } else {
        "/small".to_owned()
    };
    timed_gets(&mut client, &uri, Some(host), warm_up, requests).await
}

/// The `[MITM]` section of a profile: the hosts to decrypt, and the CA rurge
/// signs with, as `tests/mitm.rs` writes it (phase 4 M5 design 8.3).
fn mitm_section(hostname: &str, ca_p12: &str) -> String {
    format!("[MITM]\nhostname = {hostname}\nca-p12 = {ca_p12}\nca-passphrase = pw")
}

/// A rustls client trusting only `roots` and nothing else; `alpn` is what it
/// offers, and `resume` false disables session resumption (`tests/mitm.rs`
/// builds the same configuration for its handshakes). The server name
/// extension is sent, as the default lets it be.
fn client_tls(
    roots: Arc<rustls::RootCertStore>,
    alpn: &[&[u8]],
    resume: bool,
) -> Arc<rustls::ClientConfig> {
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("the protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    if !resume {
        config.resumption = rustls::client::Resumption::disabled();
    }
    Arc::new(config)
}

/// The client side of a TLS handshake to `name` over `tcp`. A benchmark puts
/// no timeout here: the command that runs one bounds it (the phase 4 / M5b
/// plan's execution record row 13's coordinator's ruling).
async fn tls_stream(
    tcp: TcpStream,
    config: Arc<rustls::ClientConfig>,
    name: &str,
) -> tokio_rustls::client::TlsStream<TcpStream> {
    let server_name = rustls::pki_types::ServerName::try_from(name.to_string())
        .unwrap_or_else(|_| panic!("{name} as a server name"));
    tokio_rustls::TlsConnector::from(config)
        .connect(server_name, tcp)
        .await
        .unwrap_or_else(|e| panic!("the TLS handshake with {name}: {e}"))
}

/// The `(uri, Host)` a B2 request takes: HTTP/2 writes the absolute form (h2
/// reads `:scheme` and `:authority` from it), HTTP/1.1 the origin form with a
/// `Host` (phase 4 M5 design 8.3, and the phase 4 / M5b plan's row 3).
fn b2_request(h2: bool, host: &str, path: &str) -> (String, Option<String>) {
    if h2 {
        (format!("https://{host}{path}"), None)
    } else {
        (path.to_owned(), Some(host.to_owned()))
    }
}

/// A TLS client for one B2 path: `tcp` is already connected, `roots` are the
/// side it talks to, `alpn` is what it offers, and the HTTP version follows
/// it. The negotiated ALPN is asserted to be `alpn`.
async fn b2_tls_client(
    tcp: TcpStream,
    roots: Arc<rustls::RootCertStore>,
    alpn: &[u8],
    h2: bool,
    name: &str,
) -> Client {
    let tls = tls_stream(
        tcp,
        client_tls(roots, std::slice::from_ref(&alpn), true),
        name,
    )
    .await;
    assert_eq!(
        tls.get_ref().1.alpn_protocol(),
        Some(alpn),
        "the ALPN the {name} client negotiated"
    );
    if h2 {
        h2_client(tls).await
    } else {
        h1_client(tls).await
    }
}

/// A B2 client through rurge's SOCKS5 listener to `origin.test:{port}`, the
/// TLS handshake done and `alpn` negotiated.
async fn b2_via_client(
    h: &Harness,
    roots: Arc<rustls::RootCertStore>,
    alpn: &[u8],
    h2: bool,
    port: u16,
) -> Client {
    let tunnel = connect_via_socks5(h.socks(), "origin.test", port).await;
    tunnel.set_nodelay(true).unwrap();
    b2_tls_client(tunnel, roots, alpn, h2, "origin.test").await
}

/// A B2 client straight to the origin, the TLS handshake done and `alpn`
/// negotiated.
async fn b2_direct_client(
    addr: SocketAddr,
    roots: Arc<rustls::RootCertStore>,
    alpn: &[u8],
    h2: bool,
) -> Client {
    b2_tls_client(plain_stream(addr).await, roots, alpn, h2, "origin.test").await
}

/// One B2 download and upload, timed as B1's are, on an already-handshook
/// `client`; the origin's answers are asserted.
async fn b2_connection(client: &mut Client, h2: bool, host: &str, body: u64) -> (f64, f64) {
    let (down_uri, down_host) = b2_request(h2, host, "/down");
    let (status, got, took) = timed_fetch(client, get(&down_uri, down_host.as_deref())).await;
    assert_eq!(status, StatusCode::OK, "the download's answer");
    assert_eq!(got, body, "the downloaded bytes");
    let down = gbit_per_s(got, took);

    let (up_uri, up_host) = b2_request(h2, host, "/up");
    let (line, took) = timed_upload(client, post(&up_uri, up_host.as_deref(), body)).await;
    assert_eq!(
        line,
        format!("{body} 97"),
        "the origin's answer to the upload"
    );
    (down, gbit_per_s(body, took))
}

/// One B2 latency pass on an already-handshook `client`.
async fn b2_latency_pass(
    client: &mut Client,
    h2: bool,
    host: &str,
    warm_up: usize,
    requests: usize,
) -> Vec<Duration> {
    let (uri, req_host) = b2_request(h2, host, "/small");
    timed_gets(client, &uri, req_host.as_deref(), warm_up, requests).await
}

/// One B2 via connection through rurge (a download and an upload).
async fn b2_via_connection(
    h: &Harness,
    ca: &TlsFixture,
    alpn: &[u8],
    h2: bool,
    port: u16,
    host: &str,
    body: u64,
) -> (f64, f64) {
    let mut client = b2_via_client(h, ca.roots(), alpn, h2, port).await;
    b2_connection(&mut client, h2, host, body).await
}

/// One B2 direct connection to the origin (a download and an upload).
async fn b2_direct_connection(
    addr: SocketAddr,
    cert: &TlsFixture,
    alpn: &[u8],
    h2: bool,
    host: &str,
    body: u64,
) -> (f64, f64) {
    let mut client = b2_direct_client(addr, cert.roots(), alpn, h2).await;
    b2_connection(&mut client, h2, host, body).await
}

/// One B2 via latency pass.
async fn b2_via_latency(
    h: &Harness,
    ca: &TlsFixture,
    alpn: &[u8],
    h2: bool,
    port: u16,
    host: &str,
    s: &Scale,
) -> Vec<Duration> {
    let mut client = b2_via_client(h, ca.roots(), alpn, h2, port).await;
    b2_latency_pass(&mut client, h2, host, s.warm_up, s.requests).await
}

/// One B2 direct latency pass.
async fn b2_direct_latency(
    addr: SocketAddr,
    cert: &TlsFixture,
    alpn: &[u8],
    h2: bool,
    host: &str,
    s: &Scale,
) -> Vec<Duration> {
    let mut client = b2_direct_client(addr, cert.roots(), alpn, h2).await;
    b2_latency_pass(&mut client, h2, host, s.warm_up, s.requests).await
}

/// One B4 handshake to `name` through rurge's SOCKS5 listener, timed from
/// before the tunnel is connected to the end of the handshake; `pass` is only
/// for the messages. The handshake is asserted to be a full one, and the leaf
/// certificate of `name` is returned (`phase 4 M5 design 8.5`).
async fn b4_handshake(
    h: &Harness,
    client: &Arc<rustls::ClientConfig>,
    name: &str,
    pass: usize,
) -> (Duration, Vec<u8>) {
    let started = Instant::now();
    let tunnel = connect_via_socks5(h.socks(), name, 443).await;
    tunnel.set_nodelay(true).unwrap();
    let tls = tls_stream(tunnel, client.clone(), name).await;
    let took = started.elapsed();
    let der = {
        let (_, conn) = tls.get_ref();
        assert_eq!(
            conn.handshake_kind(),
            Some(rustls::HandshakeKind::Full),
            "the handshake for {name} in pass {pass} is a full one"
        );
        conn.peer_certificates()
            .expect("the leaf certificate")
            .first()
            .expect("one certificate")
            .as_ref()
            .to_vec()
    };
    drop(tls);
    (took, der)
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b1_plain_throughput() {
    let s = scale();
    let origin = origin(None, s.body).await;
    let h = harness(Profile {
        hosts: "origin.test = 127.0.0.1",
        ..Profile::default()
    })
    .await;
    let host = format!("origin.test:{}", origin.addr.port());
    println!("b1_plain_throughput [{}]", s.label());

    let before = h.engine.http_counts().requests;
    let mut via_down = Vec::new();
    let mut via_up = Vec::new();
    let mut direct_down = Vec::new();
    let mut direct_up = Vec::new();
    for round in 0..s.rounds {
        let (vd, vu, dd, du) = if round % 2 == 0 {
            let via = b1_connection(h.http(), true, &host, s.body).await;
            let direct = b1_connection(origin.addr, false, &host, s.body).await;
            (via.0, via.1, direct.0, direct.1)
        } else {
            let direct = b1_connection(origin.addr, false, &host, s.body).await;
            let via = b1_connection(h.http(), true, &host, s.body).await;
            (via.0, via.1, direct.0, direct.1)
        };
        println!(
            "round {:>2}: via down {vd:.2} Gbit/s, via up {vu:.2}, direct down {dd:.2}, direct up {du:.2}",
            round + 1
        );
        via_down.push(vd);
        via_up.push(vu);
        direct_down.push(dd);
        direct_up.push(du);
    }

    let requests = h.engine.http_counts().requests - before;
    assert_eq!(
        requests,
        (2 * s.rounds) as u64,
        "the requests that reached the rules: expected {}, got {requests}",
        2 * s.rounds
    );
    let (served_h1, served_h2) = origin.counts();
    assert_eq!(
        (served_h1, served_h2),
        ((4 * s.rounds) as u64, 0),
        "the requests the origin served, by version"
    );

    let vd = median(&via_down);
    let vu = median(&via_up);
    let dd = median(&direct_down);
    let du = median(&direct_up);
    println!("via: down {vd:.2} Gbit/s, up {vu:.2} Gbit/s");
    println!("direct: down {dd:.2} Gbit/s, up {du:.2} Gbit/s");
    println!("direct/via: down {:.2}x, up {:.2}x", dd / vd, du / vu);
    println!("NFR-01 line: 1.00 Gbit/s (single-core loopback TCP)");
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b1_plain_latency() {
    let s = scale();
    let origin = origin(None, s.body).await;
    let h = harness(Profile {
        hosts: "origin.test = 127.0.0.1",
        ..Profile::default()
    })
    .await;
    let host = format!("origin.test:{}", origin.addr.port());
    println!("b1_plain_latency [{}]", s.label());

    let before = h.engine.http_counts().requests;
    let mut via = Vec::new();
    let mut direct = Vec::new();
    for round in 0..s.rounds {
        let (v, d) = if round % 2 == 0 {
            let v = b1_latency_pass(h.http(), true, &host, s.warm_up, s.requests).await;
            let d = b1_latency_pass(origin.addr, false, &host, s.warm_up, s.requests).await;
            (v, d)
        } else {
            let d = b1_latency_pass(origin.addr, false, &host, s.warm_up, s.requests).await;
            let v = b1_latency_pass(h.http(), true, &host, s.warm_up, s.requests).await;
            (v, d)
        };
        let mut v_copy = v.clone();
        let mut d_copy = d.clone();
        println!(
            "round {:>2}: via median {:.1} us, direct median {:.1} us",
            round + 1,
            micros(latency(&mut v_copy).median),
            micros(latency(&mut d_copy).median)
        );
        via.extend(v);
        direct.extend(d);
    }

    let requests = h.engine.http_counts().requests - before;
    assert_eq!(
        requests,
        (s.rounds * (s.warm_up + s.requests)) as u64,
        "the requests that reached the rules: expected {}, got {requests}",
        s.rounds * (s.warm_up + s.requests)
    );
    let (served_h1, served_h2) = origin.counts();
    assert_eq!(
        (served_h1, served_h2),
        ((2 * s.rounds * (s.warm_up + s.requests)) as u64, 0),
        "the requests the origin served, by version"
    );

    assert!(!via.is_empty(), "the via latency samples are empty");
    let v = latency(&mut via);
    assert!(!direct.is_empty(), "the direct latency samples are empty");
    let d = latency(&mut direct);
    println!(
        "via: median {:.1} us, p99 {:.1} us, max {:.1} us",
        micros(v.median),
        micros(v.p99),
        micros(v.max)
    );
    println!(
        "direct: median {:.1} us, p99 {:.1} us, max {:.1} us",
        micros(d.median),
        micros(d.p99),
        micros(d.max)
    );
    println!(
        "engine overhead: median {:.1} us, p99 {:.1} us",
        micros(v.median) - micros(d.median),
        micros(v.p99) - micros(d.p99)
    );
    println!(
        "NFR-01 lines: 1.00 Gbit/s (single-core loopback TCP); 100,000 rules p99 < 50 us (a rule match)"
    );
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b3_a_thousand_rules_latency() {
    let s = scale();
    let origin = origin(None, s.body).await;
    let rules = thousand_rules();
    let plain = harness(Profile {
        hosts: "origin.test = 127.0.0.1",
        ..Profile::default()
    })
    .await;
    let ruled = harness(Profile {
        hosts: "origin.test = 127.0.0.1",
        rules: &rules,
        ..Profile::default()
    })
    .await;
    let host = format!("origin.test:{}", origin.addr.port());
    println!("b3_a_thousand_rules_latency [{}]", s.label());
    let plain_before = plain.engine.http_counts().requests;
    let ruled_before = ruled.engine.http_counts().requests;

    // Before the timing: a request whose `User-Agent` matches one of the
    // thousand rules, so that its record shows the rules were consulted.
    {
        let mut client = h1_client(plain_stream(ruled.http()).await).await;
        let sanity = Request::builder()
            .method("GET")
            .uri(format!("http://{host}/sanity"))
            .header(header::HOST, &host)
            .header(header::USER_AGENT, "nomatch-agent-007/1")
            .body(Repeat::new(0))
            .unwrap();
        // Outside the timing, and rurge may close the connection or answer a
        // page, so its errors are dropped; the five seconds only bound this
        // one request, not the benchmark's timed ones.
        let exchange = async {
            if let Client::H1(sender) = &mut client {
                let _ = sender.ready().await;
                let _ = sender.send_request(sanity).await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(5), exchange).await;
    }
    let log = ruled.engine.request_log();
    let is_sanity =
        |r: &rurge_engine::RequestRecord| r.url.as_deref().is_some_and(|u| u.ends_with("/sanity"));
    wait_until("the /sanity request to be recorded", || {
        log.recent(10).iter().any(is_sanity)
    })
    .await;
    let recent = log.recent(10);
    let sanity = recent
        .iter()
        .find(|r| is_sanity(r))
        .expect("the /sanity record");
    assert_eq!(
        sanity.status,
        RecordStatus::Rejected("REJECT".into()),
        "the /sanity request was rejected by its rule"
    );
    assert!(
        sanity
            .rule
            .as_deref()
            .is_some_and(|r| r.contains("nomatch-agent-007")),
        "the rule that rejected /sanity: {:?}",
        sanity.rule
    );

    let mut plain_samples = Vec::new();
    let mut ruled_samples = Vec::new();
    for round in 0..s.rounds {
        let (p, r) = if round % 2 == 0 {
            let p = b1_latency_pass(plain.http(), true, &host, s.warm_up, s.requests).await;
            let r = b1_latency_pass(ruled.http(), true, &host, s.warm_up, s.requests).await;
            (p, r)
        } else {
            let r = b1_latency_pass(ruled.http(), true, &host, s.warm_up, s.requests).await;
            let p = b1_latency_pass(plain.http(), true, &host, s.warm_up, s.requests).await;
            (p, r)
        };
        let mut p_copy = p.clone();
        let mut r_copy = r.clone();
        println!(
            "round {:>2}: plain median {:.1} us, ruled median {:.1} us",
            round + 1,
            micros(latency(&mut p_copy).median),
            micros(latency(&mut r_copy).median)
        );
        plain_samples.extend(p);
        ruled_samples.extend(r);
    }

    let per_round = (s.warm_up + s.requests) as u64;
    let plain_requests = plain.engine.http_counts().requests - plain_before;
    assert_eq!(
        plain_requests,
        s.rounds as u64 * per_round,
        "the plain engine's requests: expected {}, got {plain_requests}",
        s.rounds as u64 * per_round
    );
    let ruled_requests = ruled.engine.http_counts().requests - ruled_before;
    assert_eq!(
        ruled_requests,
        s.rounds as u64 * per_round + 1,
        "the ruled engine's requests (its /sanity too): expected {}, got {ruled_requests}",
        s.rounds as u64 * per_round + 1
    );
    let (served_h1, served_h2) = origin.counts();
    assert_eq!(
        (served_h1, served_h2),
        (2 * s.rounds as u64 * per_round, 0),
        "the requests the origin served, by version"
    );

    assert!(
        !plain_samples.is_empty(),
        "the plain latency samples are empty"
    );
    let p = latency(&mut plain_samples);
    assert!(
        !ruled_samples.is_empty(),
        "the ruled latency samples are empty"
    );
    let r = latency(&mut ruled_samples);
    println!(
        "plain: median {:.1} us, p99 {:.1} us, max {:.1} us",
        micros(p.median),
        micros(p.p99),
        micros(p.max)
    );
    println!(
        "ruled: median {:.1} us, p99 {:.1} us, max {:.1} us",
        micros(r.median),
        micros(r.p99),
        micros(r.max)
    );
    println!(
        "a thousand rules cost: median {:.1} us, p99 {:.1} us",
        micros(r.median) - micros(p.median),
        micros(r.p99) - micros(p.p99)
    );
    println!(
        "NFR-01 line: 100,000 rules p99 < 50 us is rurge-rules' single match; here it is what one request pays"
    );
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b2_decrypted_throughput() {
    let s = scale();
    let cert = TlsFixture::new(&["origin.test"]);
    let origin = origin(Some(cert.acceptor(false)), s.body).await;
    let ca = TlsFixture::with_ca_name("rurge test MITM CA", &[]);
    let ca_p12 = ca.ca_p12_base64("pw");
    let port = origin.addr.port();
    let host = format!("origin.test:{port}");
    println!("b2_decrypted_throughput [{}]", s.label());

    for h2 in [false, true] {
        let alpn: &[u8] = if h2 { b"h2" } else { b"http/1.1" };
        let tag = if h2 { "h2" } else { "h1" };
        let section = if h2 {
            format!("{}\nh2 = true", mitm_section(&host, &ca_p12))
        } else {
            mitm_section(&host, &ca_p12)
        };
        let h = harness_with(
            Profile {
                hosts: "origin.test = 127.0.0.1",
                sections: &section,
                ..Profile::default()
            },
            EngineShared {
                roots: Some(cert.roots()),
                sniff_wait: Duration::from_millis(300),
                ..EngineShared::default()
            },
        )
        .await;
        let (h1_before, h2_before) = origin.counts();
        let decrypted_before = h.engine.http_counts().mitm.decrypted;
        let requests_before = h.engine.http_counts().requests;
        let mut via_down = Vec::new();
        let mut via_up = Vec::new();
        let mut direct_down = Vec::new();
        let mut direct_up = Vec::new();
        for round in 0..s.rounds {
            let (via, direct) = if round % 2 == 0 {
                let via = b2_via_connection(&h, &ca, alpn, h2, port, &host, s.body).await;
                let direct =
                    b2_direct_connection(origin.addr, &cert, alpn, h2, &host, s.body).await;
                (via, direct)
            } else {
                let direct =
                    b2_direct_connection(origin.addr, &cert, alpn, h2, &host, s.body).await;
                let via = b2_via_connection(&h, &ca, alpn, h2, port, &host, s.body).await;
                (via, direct)
            };
            println!(
                "{tag} round {:>2}: via down {:.2} Gbit/s, via up {:.2}, direct down {:.2}, direct up {:.2}",
                round + 1,
                via.0,
                via.1,
                direct.0,
                direct.1
            );
            via_down.push(via.0);
            via_up.push(via.1);
            direct_down.push(direct.0);
            direct_up.push(direct.1);
        }

        let (h1_after, h2_after) = origin.counts();
        let served = (4 * s.rounds) as u64;
        let seen = (h1_after - h1_before, h2_after - h2_before);
        if h2 {
            assert_eq!(
                seen,
                (0, served),
                "the origin's requests in the h2 row, by version"
            );
        } else {
            assert_eq!(
                seen,
                (served, 0),
                "the origin's requests in the h1 row, by version"
            );
        }
        wait_until("this row's decrypted connections", || {
            h.engine.http_counts().mitm.decrypted - decrypted_before >= s.rounds as u64
        })
        .await;
        assert_eq!(
            h.engine.http_counts().mitm.decrypted - decrypted_before,
            s.rounds as u64,
            "the decrypted connections of this row"
        );
        let requests = h.engine.http_counts().requests - requests_before;
        assert_eq!(
            requests,
            (2 * s.rounds) as u64,
            "the requests that reached the rules in this row: expected {}, got {requests}",
            2 * s.rounds
        );

        let vd = median(&via_down);
        let vu = median(&via_up);
        let dd = median(&direct_down);
        let du = median(&direct_up);
        println!("{tag} via: down {vd:.2} Gbit/s, up {vu:.2} Gbit/s");
        println!("{tag} direct: down {dd:.2} Gbit/s, up {du:.2} Gbit/s");
        println!("{tag} direct/via: down {:.2}x, up {:.2}x", dd / vd, du / vu);
    }
    println!("NFR-01 line: 1.00 Gbit/s (single-core loopback TCP)");
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b2_decrypted_latency() {
    let s = scale();
    let cert = TlsFixture::new(&["origin.test"]);
    let origin = origin(Some(cert.acceptor(false)), s.body).await;
    let ca = TlsFixture::with_ca_name("rurge test MITM CA", &[]);
    let ca_p12 = ca.ca_p12_base64("pw");
    let port = origin.addr.port();
    let host = format!("origin.test:{port}");
    println!("b2_decrypted_latency [{}]", s.label());

    for h2 in [false, true] {
        let alpn: &[u8] = if h2 { b"h2" } else { b"http/1.1" };
        let tag = if h2 { "h2" } else { "h1" };
        let section = if h2 {
            format!("{}\nh2 = true", mitm_section(&host, &ca_p12))
        } else {
            mitm_section(&host, &ca_p12)
        };
        let h = harness_with(
            Profile {
                hosts: "origin.test = 127.0.0.1",
                sections: &section,
                ..Profile::default()
            },
            EngineShared {
                roots: Some(cert.roots()),
                sniff_wait: Duration::from_millis(300),
                ..EngineShared::default()
            },
        )
        .await;
        let (h1_before, h2_before) = origin.counts();
        let decrypted_before = h.engine.http_counts().mitm.decrypted;
        let requests_before = h.engine.http_counts().requests;
        let mut via = Vec::new();
        let mut direct = Vec::new();
        for round in 0..s.rounds {
            let (v, d) = if round % 2 == 0 {
                let v = b2_via_latency(&h, &ca, alpn, h2, port, &host, &s).await;
                let d = b2_direct_latency(origin.addr, &cert, alpn, h2, &host, &s).await;
                (v, d)
            } else {
                let d = b2_direct_latency(origin.addr, &cert, alpn, h2, &host, &s).await;
                let v = b2_via_latency(&h, &ca, alpn, h2, port, &host, &s).await;
                (v, d)
            };
            let mut v_copy = v.clone();
            let mut d_copy = d.clone();
            println!(
                "{tag} round {:>2}: via median {:.1} us, direct median {:.1} us",
                round + 1,
                micros(latency(&mut v_copy).median),
                micros(latency(&mut d_copy).median)
            );
            via.extend(v);
            direct.extend(d);
        }

        let (h1_after, h2_after) = origin.counts();
        let per_round = (s.warm_up + s.requests) as u64;
        let served = (2 * s.rounds) as u64 * per_round;
        let seen = (h1_after - h1_before, h2_after - h2_before);
        if h2 {
            assert_eq!(
                seen,
                (0, served),
                "the origin's requests in the h2 row, by version"
            );
        } else {
            assert_eq!(
                seen,
                (served, 0),
                "the origin's requests in the h1 row, by version"
            );
        }
        wait_until("this row's decrypted connections", || {
            h.engine.http_counts().mitm.decrypted - decrypted_before >= s.rounds as u64
        })
        .await;
        assert_eq!(
            h.engine.http_counts().mitm.decrypted - decrypted_before,
            s.rounds as u64,
            "the decrypted connections of this row"
        );
        let requests = h.engine.http_counts().requests - requests_before;
        assert_eq!(
            requests,
            s.rounds as u64 * per_round,
            "the requests that reached the rules in this row: expected {}, got {requests}",
            s.rounds as u64 * per_round
        );

        assert!(!via.is_empty(), "the via latency samples are empty");
        let v = latency(&mut via);
        assert!(!direct.is_empty(), "the direct latency samples are empty");
        let d = latency(&mut direct);
        println!(
            "{tag} via: median {:.1} us, p99 {:.1} us, max {:.1} us",
            micros(v.median),
            micros(v.p99),
            micros(v.max)
        );
        println!(
            "{tag} direct: median {:.1} us, p99 {:.1} us, max {:.1} us",
            micros(d.median),
            micros(d.p99),
            micros(d.max)
        );
        println!(
            "{tag} decryption overhead: median {:.1} us, p99 {:.1} us",
            micros(v.median) - micros(d.median),
            micros(v.p99) - micros(d.p99)
        );
    }
    println!(
        "NFR-01 lines: 1.00 Gbit/s (single-core loopback TCP); 100,000 rules p99 < 50 us (a rule match)"
    );
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b4_leaf_certificates() {
    let s = scale();
    let ca = TlsFixture::with_ca_name("rurge test MITM CA", &[]);
    let ca_p12 = ca.ca_p12_base64("pw");
    let roots = TlsFixture::new(&[]).roots();
    let client = client_tls(ca.roots(), &[], false);
    let section = mitm_section("*.bench.test", &ca_p12);
    let names: Vec<String> = (0..s.names)
        .map(|i| format!("n{i:04}.bench.test"))
        .collect();
    println!("b4_leaf_certificates [{}]", s.label());

    let mut first = Vec::new();
    let mut second = Vec::new();
    for round in 0..s.rounds {
        let h = harness_with(
            Profile {
                hosts: "*.bench.test = 127.0.0.1",
                sections: &section,
                ..Profile::default()
            },
            EngineShared {
                roots: Some(roots.clone()),
                sniff_wait: Duration::from_millis(300),
                ..EngineShared::default()
            },
        )
        .await;

        let mut leaves = Vec::with_capacity(s.names);
        let mut round_first = Vec::with_capacity(s.names);
        for name in &names {
            let (took, der) = b4_handshake(&h, &client, name, 1).await;
            leaves.push(der);
            round_first.push(took);
        }
        let mut distinct = leaves.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            s.names,
            "the leaves of the first pass are all different"
        );

        let mut round_second = Vec::with_capacity(s.names);
        for (i, name) in names.iter().enumerate() {
            let (took, der) = b4_handshake(&h, &client, name, 2).await;
            assert_eq!(
                der, leaves[i],
                "the leaf for {name} in the second pass is the cached one"
            );
            round_second.push(took);
        }

        let mut r1 = round_first.clone();
        let l1 = latency(&mut r1);
        println!(
            "round {:>2}: first pass median {:.1} us, p99 {:.1} us, max {:.1} us",
            round + 1,
            micros(l1.median),
            micros(l1.p99),
            micros(l1.max)
        );
        let mut r2 = round_second.clone();
        let l2 = latency(&mut r2);
        println!(
            "round {:>2}: second pass median {:.1} us, p99 {:.1} us, max {:.1} us",
            round + 1,
            micros(l2.median),
            micros(l2.p99),
            micros(l2.max)
        );
        first.extend(round_first);
        second.extend(round_second);

        wait_until("the two passes' decrypted connections", || {
            h.engine.http_counts().mitm.decrypted >= (2 * s.names) as u64
        })
        .await;
        assert_eq!(
            h.engine.http_counts().mitm.decrypted,
            (2 * s.names) as u64,
            "the decrypted connections of both passes"
        );
    }

    assert!(!first.is_empty(), "the first pass samples are empty");
    let f = latency(&mut first);
    assert!(!second.is_empty(), "the second pass samples are empty");
    let cached = latency(&mut second);
    println!(
        "first pass: median {:.1} us, p99 {:.1} us, max {:.1} us",
        micros(f.median),
        micros(f.p99),
        micros(f.max)
    );
    println!(
        "second pass: median {:.1} us, p99 {:.1} us, max {:.1} us",
        micros(cached.median),
        micros(cached.p99),
        micros(cached.max)
    );
    println!(
        "signing a leaf certificate: median {:.1} us (first pass less second: signing on a blocking thread plus two hand-offs -- phase 4 M5 design 16.6)",
        micros(f.median) - micros(cached.median)
    );
}

/// What this process costs in memory: the peak it reached and what it holds
/// now, in bytes (phase 4 / M5b plan's P10 ①).
struct Resident {
    peak: u64,
    now: u64,
}

/// The peak and the current working set of this process, in bytes, from the
/// operating system; `None` where this platform's reading is not made.
/// Copied from `crates/rurge-engine/src/observe/memory.rs`'s
/// `peak_resident_bytes`, which reads only the peak, and reads the current
/// working set as well (phase 4 / M5b plan's P10 ①).
fn resident() -> Option<Resident> {
    #[cfg(windows)]
    {
        // a read-only system tool, started by the test itself
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command"])
            .arg(format!(
                r#"$p = Get-Process -Id {}; "$($p.PeakWorkingSet64) $($p.WorkingSet64)""#,
                std::process::id()
            ))
            .output()
            .ok()?;
        let (peak, now) = parse_pair(&String::from_utf8_lossy(&output.stdout))?;
        Some(Resident { peak, now })
    }
    #[cfg(target_os = "linux")]
    {
        let (peak, now) = parse_status(&std::fs::read_to_string("/proc/self/status").ok()?)?;
        Some(Resident { peak, now })
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

/// The two numbers a command printed on its last non-empty line, split on
/// whitespace; `None` unless there are exactly two. The shape is copied from
/// `crates/rurge-engine/src/observe/memory.rs`'s `parse_count`.
fn parse_pair(output: &str) -> Option<(u64, u64)> {
    let line = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .next_back()?;
    let mut parts = line.split_whitespace();
    let first = parts.next()?.parse().ok()?;
    let second = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((first, second))
}

/// `VmHWM` and `VmRSS` of a `/proc/<pid>/status` text, in bytes. Copied from
/// `crates/rurge-engine/src/observe/memory.rs`'s `parse_vm_hwm`, which reads
/// `VmHWM`; `VmRSS` is read the same way (phase 4 / M5b plan's P10 ①).
fn parse_status(status: &str) -> Option<(u64, u64)> {
    let peak = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?;
    let peak: u64 = peak.trim().strip_suffix("kB")?.trim().parse().ok()?;
    let rss = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?;
    let rss: u64 = rss.trim().strip_suffix("kB")?.trim().parse().ok()?;
    Some((peak * 1024, rss * 1024))
}

/// Whether a B5 measurement already ran in this process (phase 4 / M5b plan's
/// P10 ③): the peak only grows, so each B5 benchmark has to be the only one in
/// its process.
static MEASURED: AtomicBool = AtomicBool::new(false);

/// The two readings are parsed from what the system prints (phase 4 / M5b
/// plan's P10 ②). The shape is copied from
/// `crates/rurge-engine/src/observe/memory.rs`'s
/// `the_peak_is_read_from_what_the_system_prints`.
#[test]
fn the_readings_are_parsed_from_what_the_system_prints() {
    assert_eq!(parse_pair("\r\n123 456\r\n"), Some((123, 456)));
    assert_eq!(parse_pair("x"), None);
    assert_eq!(parse_pair("1"), None);
    let status = "Name:\trurge\nVmPeak:\t  900000 kB\nVmHWM:\t    2048 kB\nVmRSS:\t    1024 kB\n";
    assert_eq!(parse_status(status), Some((2_097_152, 1_048_576)));
    let without_rss = "Name:\trurge\nVmHWM:\t    2048 kB\n";
    assert_eq!(parse_status(without_rss), None);
}

/// The size of B5's body: one byte under the 32 MiB a request rewrite declines
/// to touch (`rewrite/body.rs`'s `REQUEST` is 33 554 432, and only a body
/// larger than that is left alone; phase 4 / M5b plan's P10 ⑤). It does not
/// follow `Scale` (the plan's P5 ③).
const BIG: u64 = 32 * 1024 * 1024 - 1;

/// Opens a connection with `open` and sends it one `GET /small`, asserted to
/// be the 16 bytes; the `Client` comes back still connected, for the caller to
/// keep open (phase 4 / M5b plan's P10 ④).
async fn b5_small<F, Fut>(open: &mut F, uri: &str, host: &str) -> Client
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Client>,
{
    let mut client = open().await;
    let (status, bytes) = fetch(&mut client, get(uri, Some(host))).await;
    assert_eq!(status, StatusCode::OK, "an idle connection's answer");
    assert_eq!(bytes, 16, "an idle connection's body");
    client
}

/// B5's idle-connection measurement: one warm-up connection, dropped; the
/// reading before; `count` connections each sending one `GET /small` and kept
/// open; the reading after (phase 4 / M5b plan's P10 ④). A connection's task
/// runs in the runtime, and dropping its `Client` would close it, so the
/// connections are returned for the caller to hold until the end of its test.
async fn b5_idle<F, Fut>(
    count: usize,
    uri: &str,
    host: &str,
    mut open: F,
) -> (Resident, Resident, Vec<Client>)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Client>,
{
    drop(b5_small(&mut open, uri, host).await);
    let before = resident().expect("a reading");
    let mut kept = Vec::with_capacity(count);
    for _ in 0..count {
        kept.push(b5_small(&mut open, uri, host).await);
    }
    let after = resident().expect("a reading");
    (before, after, kept)
}

/// B5's before and after lines and its per-unit increment in KiB with one
/// decimal (phase 4 / M5b plan's P5 ⑧ and P10 ④, ⑥). The increments are
/// computed on signed numbers: the current working set may shrink (`now` is
/// not monotone).
fn report_per(before: &Resident, after: &Resident, units: usize, what: &str) {
    println!(
        "before: peak {} bytes, now {} bytes",
        before.peak, before.now
    );
    println!("after: peak {} bytes, now {} bytes", after.peak, after.now);
    let peak = (after.peak as i64 - before.peak as i64) as f64 / units as f64;
    let now = (after.now as i64 - before.now as i64) as f64 / units as f64;
    println!(
        "per {what}: peak {:.1} KiB, now {:.1} KiB",
        peak / 1024.0,
        now / 1024.0
    );
}

/// B5's body-buffer lines: the increment in copies of `BIG`, with two decimals
/// (phase 4 / M5b plan's P5 ⑧ and P10 ⑤). Signed like `report_per`.
fn report_body(before: &Resident, after: &Resident, big: u64) {
    println!(
        "before: peak {} bytes, now {} bytes",
        before.peak, before.now
    );
    println!("after: peak {} bytes, now {} bytes", after.peak, after.now);
    let peak = (after.peak as i64 - before.peak as i64) as f64 / big as f64;
    let now = (after.now as i64 - before.now as i64) as f64 / big as f64;
    println!("body buffer: peak {peak:.2} copies, now {now:.2} copies");
}

/// One handshake to `name` through rurge's SOCKS5 listener, the connection
/// dropped at once (phase 4 / M5b plan's P10 ⑥): B5's leaf cache reads a
/// certificate per handshake and keeps none.
async fn b5_handshake(h: &Harness, client: &Arc<rustls::ClientConfig>, name: &str) {
    let tunnel = connect_via_socks5(h.socks(), name, 443).await;
    tunnel.set_nodelay(true).unwrap();
    drop(tls_stream(tunnel, client.clone(), name).await);
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b5_idle_connections_plain_direct() {
    assert!(
        !MEASURED.swap(true, Ordering::SeqCst),
        "one B5 measurement per process: run it alone with --exact"
    );
    if resident().is_none() {
        println!("no reading on this platform");
        return;
    }
    let s = scale();
    let origin = origin(None, 0).await;
    let host = format!("origin.test:{}", origin.addr.port());
    println!("b5_idle_connections_plain_direct [{}]", s.label());
    let addr = origin.addr;
    let open = move || async move { h1_client(plain_stream(addr).await).await };
    let (before, after, kept) = b5_idle(s.connections, "/small", &host, open).await;
    assert_eq!(
        origin.counts(),
        ((s.connections + 1) as u64, 0),
        "the origin's requests, by version"
    );
    report_per(&before, &after, s.connections, "connection");
    drop(kept);
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b5_idle_connections_plain() {
    assert!(
        !MEASURED.swap(true, Ordering::SeqCst),
        "one B5 measurement per process: run it alone with --exact"
    );
    if resident().is_none() {
        println!("no reading on this platform");
        return;
    }
    let s = scale();
    let origin = origin(None, 0).await;
    let h = harness(Profile {
        hosts: "origin.test = 127.0.0.1",
        ..Profile::default()
    })
    .await;
    let host = format!("origin.test:{}", origin.addr.port());
    let uri = format!("http://{host}/small");
    println!("b5_idle_connections_plain [{}]", s.label());
    let requests_before = h.engine.http_counts().requests;
    let http = h.http();
    let open = move || async move { h1_client(plain_stream(http).await).await };
    let (before, after, kept) = b5_idle(s.connections, &uri, &host, open).await;
    let requests = h.engine.http_counts().requests - requests_before;
    assert_eq!(
        requests,
        (s.connections + 1) as u64,
        "the requests that reached the rules: expected {}, got {requests}",
        s.connections + 1
    );
    assert_eq!(
        origin.counts(),
        ((s.connections + 1) as u64, 0),
        "the origin's requests, by version"
    );
    report_per(&before, &after, s.connections, "connection");
    drop(kept);
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b5_idle_connections_tls_direct() {
    assert!(
        !MEASURED.swap(true, Ordering::SeqCst),
        "one B5 measurement per process: run it alone with --exact"
    );
    if resident().is_none() {
        println!("no reading on this platform");
        return;
    }
    let s = scale();
    let cert = TlsFixture::new(&["origin.test"]);
    let origin = origin(Some(cert.acceptor_with_alpn(false, &[b"http/1.1"])), 0).await;
    let host = format!("origin.test:{}", origin.addr.port());
    println!("b5_idle_connections_tls_direct [{}]", s.label());
    let addr = origin.addr;
    let config = client_tls(cert.roots(), &[b"http/1.1"], true);
    let open = move || {
        let config = config.clone();
        async move { h1_client(tls_stream(plain_stream(addr).await, config, "origin.test").await).await }
    };
    let (before, after, kept) = b5_idle(s.connections, "/small", &host, open).await;
    assert_eq!(
        origin.counts(),
        ((s.connections + 1) as u64, 0),
        "the origin's requests, by version"
    );
    report_per(&before, &after, s.connections, "connection");
    drop(kept);
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b5_idle_connections_relayed_tls() {
    assert!(
        !MEASURED.swap(true, Ordering::SeqCst),
        "one B5 measurement per process: run it alone with --exact"
    );
    if resident().is_none() {
        println!("no reading on this platform");
        return;
    }
    let s = scale();
    let cert = TlsFixture::new(&["origin.test"]);
    let origin = origin(Some(cert.acceptor_with_alpn(false, &[b"http/1.1"])), 0).await;
    let port = origin.addr.port();
    let host = format!("origin.test:{port}");
    let general = format!("force-http-engine-hosts = origin.test:{port}");
    let h = harness_with(
        Profile {
            general: &general,
            hosts: "origin.test = 127.0.0.1",
            ..Profile::default()
        },
        EngineShared {
            roots: Some(cert.roots()),
            sniff_wait: Duration::from_millis(300),
            ..EngineShared::default()
        },
    )
    .await;
    println!("b5_idle_connections_relayed_tls [{}]", s.label());
    let sniffed_before = h.engine.http_counts().sniffed.tls;
    let socks = h.socks();
    let config = client_tls(cert.roots(), &[b"http/1.1"], true);
    let open = move || {
        let config = config.clone();
        async move {
            let tcp = connect_via_socks5(socks, "origin.test", port).await;
            tcp.set_nodelay(true).unwrap();
            h1_client(tls_stream(tcp, config, "origin.test").await).await
        }
    };
    let (before, after, kept) = b5_idle(s.connections, "/small", &host, open).await;
    let sniffed = h.engine.http_counts().sniffed.tls - sniffed_before;
    assert_eq!(
        sniffed,
        (s.connections + 1) as u64,
        "the TLS connections sniffed (the warm-up among them)"
    );
    assert_eq!(
        h.engine.http_counts().mitm.decrypted,
        0,
        "a profile without [MITM] decrypts nothing"
    );
    report_per(&before, &after, s.connections, "connection");
    drop(kept);
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b5_idle_connections_decrypted() {
    assert!(
        !MEASURED.swap(true, Ordering::SeqCst),
        "one B5 measurement per process: run it alone with --exact"
    );
    if resident().is_none() {
        println!("no reading on this platform");
        return;
    }
    let s = scale();
    let cert = TlsFixture::new(&["origin.test"]);
    let origin = origin(Some(cert.acceptor(false)), 0).await;
    let ca = TlsFixture::with_ca_name("rurge test MITM CA", &[]);
    let ca_p12 = ca.ca_p12_base64("pw");
    let port = origin.addr.port();
    let host = format!("origin.test:{port}");
    let section = mitm_section(&host, &ca_p12);
    let h = harness_with(
        Profile {
            hosts: "origin.test = 127.0.0.1",
            sections: &section,
            ..Profile::default()
        },
        EngineShared {
            roots: Some(cert.roots()),
            sniff_wait: Duration::from_millis(300),
            ..EngineShared::default()
        },
    )
    .await;
    println!("b5_idle_connections_decrypted [{}]", s.label());
    let requests_before = h.engine.http_counts().requests;
    let decrypted_before = h.engine.http_counts().mitm.decrypted;
    let socks = h.socks();
    let config = client_tls(ca.roots(), &[b"http/1.1"], true);
    let open = move || {
        let config = config.clone();
        async move {
            let tcp = connect_via_socks5(socks, "origin.test", port).await;
            tcp.set_nodelay(true).unwrap();
            h1_client(tls_stream(tcp, config, "origin.test").await).await
        }
    };
    let (before, after, kept) = b5_idle(s.connections, "/small", &host, open).await;
    wait_until("the decrypted connections", || {
        h.engine.http_counts().mitm.decrypted - decrypted_before >= (s.connections + 1) as u64
    })
    .await;
    let decrypted = h.engine.http_counts().mitm.decrypted - decrypted_before;
    assert_eq!(
        decrypted,
        (s.connections + 1) as u64,
        "the decrypted connections (the warm-up among them)"
    );
    let requests = h.engine.http_counts().requests - requests_before;
    assert_eq!(
        requests,
        (s.connections + 1) as u64,
        "the requests that reached the rules: expected {}, got {requests}",
        s.connections + 1
    );
    report_per(&before, &after, s.connections, "connection");
    drop(kept);
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b5_body_direct() {
    assert!(
        !MEASURED.swap(true, Ordering::SeqCst),
        "one B5 measurement per process: run it alone with --exact"
    );
    if resident().is_none() {
        println!("no reading on this platform");
        return;
    }
    let s = scale();
    let origin = origin(None, 0).await;
    let host = format!("origin.test:{}", origin.addr.port());
    println!("b5_body_direct [{}]", s.label());
    let mut client = h1_client(plain_stream(origin.addr).await).await;
    let before = resident().expect("a reading");
    let line = upload(&mut client, post("/up", Some(host.as_str()), BIG)).await;
    let after = resident().expect("a reading");
    assert_eq!(
        line,
        format!("{BIG} 97"),
        "the origin's answer to the upload"
    );
    report_body(&before, &after, BIG);
    drop(client);
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b5_body_streamed() {
    assert!(
        !MEASURED.swap(true, Ordering::SeqCst),
        "one B5 measurement per process: run it alone with --exact"
    );
    if resident().is_none() {
        println!("no reading on this platform");
        return;
    }
    let s = scale();
    let origin = origin(None, 0).await;
    let h = harness(Profile {
        hosts: "origin.test = 127.0.0.1",
        ..Profile::default()
    })
    .await;
    let host = format!("origin.test:{}", origin.addr.port());
    let uri = format!("http://{host}/up");
    println!("b5_body_streamed [{}]", s.label());
    let mut client = h1_client(plain_stream(h.http()).await).await;
    let before = resident().expect("a reading");
    let line = upload(&mut client, post(&uri, Some(host.as_str()), BIG)).await;
    let after = resident().expect("a reading");
    assert_eq!(
        line,
        format!("{BIG} 97"),
        "the origin's answer to the upload"
    );
    report_body(&before, &after, BIG);
    drop(client);
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b5_body_rewritten() {
    assert!(
        !MEASURED.swap(true, Ordering::SeqCst),
        "one B5 measurement per process: run it alone with --exact"
    );
    if resident().is_none() {
        println!("no reading on this platform");
        return;
    }
    let s = scale();
    let origin = origin(None, 0).await;
    let port = origin.addr.port();
    let host = format!("origin.test:{port}");
    let uri = format!("http://{host}/up");
    let section = format!("[Body Rewrite]\nhttp-request ^http://origin\\.test:{port}/up ^a b");
    let h = harness(Profile {
        hosts: "origin.test = 127.0.0.1",
        sections: &section,
        ..Profile::default()
    })
    .await;
    println!("b5_body_rewritten [{}]", s.label());
    let mut client = h1_client(plain_stream(h.http()).await).await;
    let before = resident().expect("a reading");
    let line = upload(&mut client, post(&uri, Some(host.as_str()), BIG)).await;
    let after = resident().expect("a reading");
    assert_eq!(
        line,
        format!("{BIG} 98"),
        "the first byte the request rewrite left"
    );
    report_body(&before, &after, BIG);
    drop(client);
}

#[tokio::test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
async fn b5_leaf_cache() {
    assert!(
        !MEASURED.swap(true, Ordering::SeqCst),
        "one B5 measurement per process: run it alone with --exact"
    );
    if resident().is_none() {
        println!("no reading on this platform");
        return;
    }
    let s = scale();
    let ca = TlsFixture::with_ca_name("rurge test MITM CA", &[]);
    let ca_p12 = ca.ca_p12_base64("pw");
    let roots = TlsFixture::new(&[]).roots();
    let client = client_tls(ca.roots(), &[], false);
    let section = mitm_section("*.bench.test", &ca_p12);
    let h = harness_with(
        Profile {
            hosts: "*.bench.test = 127.0.0.1",
            sections: &section,
            ..Profile::default()
        },
        EngineShared {
            roots: Some(roots),
            sniff_wait: Duration::from_millis(300),
            ..EngineShared::default()
        },
    )
    .await;
    println!("b5_leaf_cache [{}]", s.label());
    let decrypted_before = h.engine.http_counts().mitm.decrypted;
    // one name first: the request log fills to its capacity, and the cache
    // holds one certificate
    for _ in 0..s.names {
        b5_handshake(&h, &client, "one.bench.test").await;
    }
    let before = resident().expect("a reading");
    for i in 0..s.names {
        let name = format!("n{i:04}.bench.test");
        b5_handshake(&h, &client, &name).await;
    }
    let after = resident().expect("a reading");
    report_per(&before, &after, s.names, "certificate");
    wait_until("the two passes' decrypted connections", || {
        h.engine.http_counts().mitm.decrypted - decrypted_before >= (2 * s.names) as u64
    })
    .await;
    let decrypted = h.engine.http_counts().mitm.decrypted - decrypted_before;
    assert_eq!(
        decrypted,
        (2 * s.names) as u64,
        "the decrypted connections of both passes"
    );
}

/// Q1: reading a ClientHello that came in two QUIC Initial datagrams, through
/// the reader of M5a, timed with none of the engine in the way (phase 4 M5
/// design 8.7, and the phase 4 / M5b plan's P11).
#[test]
#[ignore = "a measurement, not a gate: run it alone, in release, with --nocapture"]
fn q1_reading_a_quic_client_hello() {
    use rurge_net::quic::initial::{Hello, read_first};
    use rurge_net::quic::testing::{Initial, RFC_DCID, big_client_hello, chrome_frames};

    let s = scale();
    let hello = big_client_hello(Some("held.test"));
    let [first, second] = chrome_frames(&hello, 7);
    let d1 = Initial::new(1, RFC_DCID, 0, first).seal(Some(1200));
    let d2 = Initial::new(1, RFC_DCID, 1, second).seal(Some(1200));
    println!("q1_reading_a_quic_client_hello [{}]", s.label());

    // not timed: the same reading and the same assertions
    for _ in 0..s.iterations / 100 {
        let Some(Hello::Partial(reader)) = read_first(std::hint::black_box(&d1)) else {
            panic!("the first datagram holds the start of a ClientHello");
        };
        let whole = reader.read(std::hint::black_box(&d2));
        let Hello::Whole(message) = whole else {
            panic!("the second datagram completes the ClientHello");
        };
        assert_eq!(message.len(), hello.len(), "the ClientHello's bytes");
        let _ = std::hint::black_box(message);
    }

    let mut samples = Vec::with_capacity(s.iterations);
    for _ in 0..s.iterations {
        let started = Instant::now();
        let Some(Hello::Partial(reader)) = read_first(std::hint::black_box(&d1)) else {
            panic!("the first datagram holds the start of a ClientHello");
        };
        let whole = reader.read(std::hint::black_box(&d2));
        let took = started.elapsed();
        let Hello::Whole(message) = whole else {
            panic!("the second datagram completes the ClientHello");
        };
        assert_eq!(message.len(), hello.len(), "the ClientHello's bytes");
        let _ = std::hint::black_box(message);
        samples.push(took);
    }

    let l = latency(&mut samples);
    println!(
        "median {:.1} us, p99 {:.1} us, max {:.1} us",
        micros(l.median),
        micros(l.p99),
        micros(l.max)
    );
    println!("phase 3 M5 design 4.2's estimate: the order of ten microseconds");
}
