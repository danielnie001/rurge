//! HTTP/1.1 proxy listener on hyper (M3 design §6.2): CONNECT tunnels and
//! absolute-URI plain requests, Basic authentication, per-request dialing.

use crate::listener::{HttpAuth, ListenerOpts, Running, bind, serve};
use crate::responses::{self, ResponseBody};
use crate::session::{
    Counting, DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use http::{
    HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Uri, header,
};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use rurge_config::HostName;
use rurge_config::rule::ProtocolKind;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub struct HttpListener;

struct Ctx {
    dialer: Arc<dyn Dialer>,
    opts: Arc<ListenerOpts>,
    local: SocketAddr,
    peer: SocketAddr,
    tracker: TaskTracker,
}

/// Returned from the service to make hyper close the connection without a response.
#[derive(Debug)]
pub(crate) enum HandlerError {
    Close,
}

impl fmt::Display for HandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("connection closed by policy")
    }
}

impl std::error::Error for HandlerError {}

impl HttpListener {
    pub async fn bind(
        addr: SocketAddr,
        dialer: Arc<dyn Dialer>,
        opts: ListenerOpts,
        stop: CancellationToken,
        tracker: TaskTracker,
    ) -> io::Result<Running> {
        let listener = bind(addr).await?;
        let local = listener.local_addr()?;
        let opts = Arc::new(opts);
        Ok(serve(
            listener,
            "http",
            opts.restrict_to_lan,
            stop,
            move |stream, peer| {
                let ctx = Arc::new(Ctx {
                    dialer: dialer.clone(),
                    opts: opts.clone(),
                    local,
                    peer,
                    tracker: tracker.clone(),
                });
                async move { serve_connection(stream, ctx).await }
            },
        ))
    }
}

async fn serve_connection(stream: TcpStream, ctx: Arc<Ctx>) {
    let handshake_timeout = ctx.opts.handshake_timeout;
    let service = service_fn(move |req: Request<Incoming>| {
        let ctx = ctx.clone();
        async move { handle(req, ctx).await }
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .preserve_header_case(true)
        // hyper only enforces `header_read_timeout` when a timer is installed.
        .timer(TokioTimer::new())
        .header_read_timeout(handshake_timeout)
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    if let Err(e) = conn.await {
        tracing::debug!(listener = "http", error = %e, "http connection ended");
    }
}

/// Length-independent equality that does not stop at the first difference, so
/// a wrong password cannot be recovered from the time the comparison takes.
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub(crate) fn authorized(auth: &HttpAuth, header: Option<&HeaderValue>) -> bool {
    let Some(value) = header.and_then(|h| h.to_str().ok()) else {
        return false;
    };
    // RFC 7235 §2.1: the auth scheme is case-insensitive.
    let Some((scheme, encoded)) = value.split_once(' ') else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("basic") {
        return false;
    }
    let Ok(decoded) = BASE64.decode(encoded.trim()) else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(&decoded) else {
        return false;
    };
    let (user, password) = text.split_once(':').unwrap_or(("", text));
    match auth {
        HttpAuth::Password(p) => ct_eq(password.as_bytes(), p.as_bytes()),
        HttpAuth::UserPass {
            user: u,
            password: p,
        } => ct_eq(user.as_bytes(), u.as_bytes()) & ct_eq(password.as_bytes(), p.as_bytes()),
    }
}

async fn handle(
    req: Request<Incoming>,
    ctx: Arc<Ctx>,
) -> Result<Response<ResponseBody>, HandlerError> {
    if let Some(auth) = &ctx.opts.auth
        && !authorized(auth, req.headers().get(header::PROXY_AUTHORIZATION))
    {
        return Ok(responses::proxy_auth_required());
    }
    if req.method() == Method::CONNECT {
        return connect(req, ctx).await;
    }
    if req.uri().scheme().is_some() && req.uri().authority().is_some() {
        return forward(req, ctx).await;
    }
    Ok(responses::bad_request(
        "rurge is an HTTP proxy: send CONNECT or an absolute-URI request",
    ))
}

fn session_for(ctx: &Ctx, host: HostName, port: u16) -> SessionInfo {
    let mut s = SessionInfo::tcp(host, port);
    s.src = ctx.peer;
    s.in_port = ctx.local.port();
    s.listener = ListenerKind::Http;
    s.transport = Transport::Tcp;
    s
}

async fn connect(
    mut req: Request<Incoming>,
    ctx: Arc<Ctx>,
) -> Result<Response<ResponseBody>, HandlerError> {
    let Some(authority) = req.uri().authority().cloned() else {
        return Ok(responses::bad_request("CONNECT needs host:port"));
    };
    let host = HostName::parse(authority.host());
    // RFC 7231 §4.3.6: the CONNECT target is always host:port, never a bare host.
    let Some(port) = authority.port_u16() else {
        return Ok(responses::bad_request("CONNECT needs host:port"));
    };
    let session = session_for(&ctx, host, port);
    match ctx.dialer.dial(session).await {
        Ok(dialed) => {
            let upgrade = hyper::upgrade::on(&mut req);
            let dialer = ctx.dialer.clone();
            ctx.tracker.spawn(async move {
                // The tunnel runs on the engine's tracker, outside the accept
                // loop's `JoinSet` and therefore outside its panic handler
                // (design §6.4); an inner task restores the ERROR log.
                let inner = tokio::spawn(async move {
                    match upgrade.await {
                        Ok(upgraded) => {
                            let client: BoxedStream = Box::new(TokioIo::new(upgraded));
                            dialer.relay(client, dialed.stream, dialed.handle).await;
                        }
                        Err(e) => dialed
                            .handle
                            .finish(SessionOutcome::Failed(format!("upgrade failed: {e}"))),
                    }
                });
                if let Err(e) = inner.await
                    && e.is_panic()
                {
                    tracing::error!(listener = "http", "tunnel task panicked: {e}");
                }
            });
            Ok(responses::connect_established())
        }
        Err(DialError::Reject {
            kind: RejectKind::Drop,
            ..
        }) => {
            tokio::time::sleep(ctx.opts.drop_hold).await;
            Err(HandlerError::Close)
        }
        Err(DialError::Reject { .. }) => Err(HandlerError::Close),
        Err(DialError::Failed {
            kind,
            message,
            handle,
            ..
        }) => {
            let what = match kind {
                FailKind::Dns => "DNS lookup failed",
                FailKind::Timeout => "Connection timed out",
                FailKind::Connect | FailKind::Other => "Connection failed",
            };
            failure_response(&ctx, &handle, &format!("{what}: {message}"))
        }
    }
}

/// Connection-specific headers a proxy must not pass on (RFC 7230 §6.1),
/// plus the two `Proxy-*` headers that belong to this hop only.
const HOP_BY_HOP: [HeaderName; 9] = [
    header::CONNECTION,
    HeaderName::from_static("keep-alive"),
    HeaderName::from_static("proxy-connection"),
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
    header::UPGRADE,
    header::PROXY_AUTHENTICATE,
    header::PROXY_AUTHORIZATION,
];

/// The authority without any `user:password@` prefix (RFC 7230 §5.4 forbids
/// userinfo in `Host`, and it must never reach a log or the session record).
pub(crate) fn host_port(authority: &http::uri::Authority) -> String {
    match authority.port_u16() {
        Some(p) => format!("{}:{p}", authority.host()),
        None => authority.host().to_string(),
    }
}

/// Removes every hop-by-hop header, including the ones `Connection` names.
/// hyper re-frames the body itself, so dropping `Transfer-Encoding` is safe.
pub(crate) fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let listed: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|t| HeaderName::try_from(t.trim()).ok())
        .collect();
    for name in listed {
        headers.remove(name);
    }
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
}

/// Rewrites a proxy request into the form the origin expects: origin-form
/// URI, a `Host` header matching the request target, and no hop-by-hop headers.
pub(crate) fn origin_form(req: &mut Request<Incoming>) -> Result<(), http::Error> {
    let authority = req.uri().authority().map(host_port);
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    *req.uri_mut() = path.parse::<Uri>()?;
    strip_hop_by_hop(req.headers_mut());
    // RFC 7230 §5.4: the request target wins over whatever `Host` the client sent.
    if let Some(a) = authority
        && let Ok(v) = HeaderValue::from_str(&a)
    {
        req.headers_mut().insert(header::HOST, v);
    }
    Ok(())
}

/// Keeps a proxy request in absolute form for an upstream HTTP proxy
/// (`always-use-connect = false`): the URI is rebuilt without userinfo, the
/// hop-by-hop headers go, `Host` follows the request target, and `extra` —
/// the upstream's `Proxy-Authorization` and configured headers — replaces
/// same-name headers, `Host` included (manual: Policies › HTTP).
pub(crate) fn absolute_form<B>(
    req: &mut Request<B>,
    extra: &[(String, String)],
) -> Result<(), http::Error> {
    let authority = req.uri().authority().map(host_port).unwrap_or_default();
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    *req.uri_mut() = format!("http://{authority}{path}").parse::<Uri>()?;
    strip_hop_by_hop(req.headers_mut());
    if let Ok(v) = HeaderValue::from_str(&authority) {
        req.headers_mut().insert(header::HOST, v);
    }
    for (name, value) in extra {
        let (Ok(name), Ok(value)) = (
            HeaderName::try_from(name.as_str()),
            HeaderValue::from_str(value),
        ) else {
            // the outbound validated its templates; whatever the http crate
            // still refuses is dropped rather than sent malformed. The name
            // only, Debug-escaped; the value is a credential.
            tracing::debug!(
                listener = "http",
                name = ?name,
                "dropped an upstream proxy header"
            );
            continue;
        };
        req.headers_mut().insert(name, value);
    }
    Ok(())
}

fn failure_response(
    ctx: &Ctx,
    handle: &SessionHandle,
    message: &str,
) -> Result<Response<ResponseBody>, HandlerError> {
    if !ctx.opts.show_error_page {
        return Err(HandlerError::Close);
    }
    let s = handle.session();
    Ok(responses::error_page(
        StatusCode::BAD_GATEWAY,
        &responses::ErrorPage {
            title: "Connection failed",
            session_id: handle.id(),
            dst: format!("{}:{}", s.dst_host, s.dst_port),
            rule: handle.rule(),
            chain: handle.policy_chain(),
            message: message.to_string(),
        },
    ))
}

async fn forward(
    mut req: Request<Incoming>,
    ctx: Arc<Ctx>,
) -> Result<Response<ResponseBody>, HandlerError> {
    let Some(authority) = req.uri().authority().cloned() else {
        return Ok(responses::bad_request("absolute URI without a host"));
    };
    // TLS is tunnelled with CONNECT; anything else is not ours to forward (M5).
    if req.uri().scheme() != Some(&http::uri::Scheme::HTTP) {
        return Ok(responses::bad_request(
            "only http:// absolute URIs can be forwarded",
        ));
    }
    let host = HostName::parse(authority.host());
    let port = authority.port_u16().unwrap_or(80);
    let mut session = session_for(&ctx, host, port);
    session.protocol = Some(ProtocolKind::Http);
    session.url = Some(format!(
        "http://{}{}",
        host_port(&authority),
        req.uri()
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/")
    ));
    session.http_host = Some(authority.host().to_ascii_lowercase());
    session.user_agent = req
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let dialed = match ctx.dialer.dial(session).await {
        Ok(d) => d,
        Err(DialError::Reject {
            kind: RejectKind::TinyGif,
            ..
        }) => return Ok(responses::tiny_gif()),
        Err(DialError::Reject {
            kind: RejectKind::Drop,
            ..
        }) => {
            tokio::time::sleep(ctx.opts.drop_hold).await;
            return Err(HandlerError::Close);
        }
        Err(DialError::Reject { kind, rule, handle }) => {
            if !ctx.opts.show_error_page_for_reject {
                return Err(HandlerError::Close);
            }
            let s = handle.session();
            return Ok(responses::error_page(
                StatusCode::FORBIDDEN,
                &responses::ErrorPage {
                    title: "Request rejected",
                    session_id: handle.id(),
                    dst: format!("{}:{}", s.dst_host, s.dst_port),
                    rule,
                    chain: handle.policy_chain(),
                    message: format!("The request was rejected by the {} policy.", kind.name()),
                },
            ));
        }
        Err(DialError::Failed {
            kind,
            message,
            handle,
            ..
        }) => {
            let what = match kind {
                FailKind::Dns => "DNS lookup failed",
                FailKind::Timeout => "Connection timed out",
                FailKind::Connect | FailKind::Other => "Connection failed",
            };
            return failure_response(&ctx, &handle, &format!("{what}: {message}"));
        }
    };

    let Dialed {
        stream: upstream,
        handle,
        forward: upstream_headers,
    } = dialed;
    let io = TokioIo::new(Counting::new(upstream, handle.clone()));
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(pair) => pair,
        Err(e) => {
            handle.finish(SessionOutcome::Failed(format!(
                "upstream handshake failed: {e}"
            )));
            return failure_response(&ctx, &handle, &format!("Upstream handshake failed: {e}"));
        }
    };
    let conn_handle = handle.clone();
    // Cancelled when this handler returns, i.e. once the code below has had
    // its say about the response head (an upstream 407 is a failed session).
    // `SessionHandle::finish` is first-wins and the driver would otherwise get
    // there first: the response head and the end of the upstream connection
    // can arrive in one and the same poll of the driver task.
    let handled = CancellationToken::new();
    let driver_waits = handled.clone();
    let _handled = handled.drop_guard();
    ctx.tracker.spawn(async move {
        // Inner task: this driver is tracked by the engine, not by the accept
        // loop's `JoinSet`, so its panics would otherwise go unreported.
        let inner = tokio::spawn(async move {
            // Race the session token: `kill` and the graceful shutdown's
            // `cancel_sessions` end the exchange by dropping the connection,
            // which is what makes a streaming `http://` response killable.
            let outcome = tokio::select! {
                biased;
                _ = conn_handle.token().cancelled() => cancelled_outcome(&conn_handle),
                res = conn => match res {
                    Ok(()) => SessionOutcome::Completed,
                    Err(e) => SessionOutcome::Failed(format!("upstream connection error: {e}")),
                },
            };
            driver_waits.cancelled().await;
            conn_handle.finish(outcome);
        });
        if let Err(e) = inner.await
            && e.is_panic()
        {
            tracing::error!(listener = "http", "upstream connection task panicked: {e}");
        }
    });
    // To an HTTP proxy the request stays in absolute form; to anything else
    // (an origin, a tunnel) it goes in origin form.
    let rewritten = match &upstream_headers {
        Some(headers) => absolute_form(&mut req, headers),
        None => origin_form(&mut req),
    };
    if let Err(e) = rewritten {
        handle.finish(SessionOutcome::Failed(format!("bad request uri: {e}")));
        return Ok(responses::bad_request("malformed request URI"));
    }
    // `biased`: once the token is cancelled the answer is always "closed by
    // policy", never the error page the dropped upstream connection would
    // otherwise produce.
    let sent = tokio::select! {
        biased;
        _ = handle.token().cancelled() => {
            handle.finish(cancelled_outcome(&handle));
            return Err(HandlerError::Close);
        }
        res = sender.send_request(req) => res,
    };
    match sent {
        Ok(mut resp) => {
            if upstream_headers.is_some()
                && resp.status() == StatusCode::PROXY_AUTHENTICATION_REQUIRED
            {
                // the credentials in question are rurge's own, towards its
                // upstream: the client can do nothing about them, and the
                // challenge header is hop-by-hop. Both texts come from the
                // status code, never from the upstream's reason phrase.
                handle.finish(SessionOutcome::Failed(
                    "http proxy answered 407 Proxy Authentication Required".to_string(),
                ));
                return failure_response(
                    &ctx,
                    &handle,
                    "Upstream proxy rejected the request: 407 Proxy Authentication Required",
                );
            }
            strip_hop_by_hop(resp.headers_mut());
            Ok(resp.map(|body| body.boxed()))
        }
        Err(e) => {
            handle.finish(SessionOutcome::Failed(format!(
                "upstream request failed: {e}"
            )));
            failure_response(&ctx, &handle, &format!("Upstream request failed: {e}"))
        }
    }
}

/// How a session the cancellation token ended is recorded: an operator `kill`
/// is a failure, the graceful shutdown's `cancel_sessions` is not.
fn cancelled_outcome(handle: &SessionHandle) -> SessionOutcome {
    if handle.was_killed() {
        SessionOutcome::Failed("killed".to_string())
    } else {
        SessionOutcome::Completed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeDialer, echo_server};
    use rurge_net::testing::TestServer;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::sync::CancellationToken;
    use tokio_util::task::TaskTracker;

    pub(crate) async fn listener(opts: ListenerOpts) -> (Running, Arc<FakeDialer>) {
        let echo = echo_server().await;
        let dialer = FakeDialer::new(echo, None);
        let running = HttpListener::bind(
            "127.0.0.1:0".parse().unwrap(),
            dialer.clone(),
            opts,
            CancellationToken::new(),
            TaskTracker::new(),
        )
        .await
        .unwrap();
        (running, dialer)
    }

    async fn listener_with_target(opts: ListenerOpts) -> (Running, Arc<FakeDialer>, TestServer) {
        let target = TestServer::spawn().await;
        let target_addr: SocketAddr = format!("127.0.0.1:{}", target.url("/").port().unwrap())
            .parse()
            .unwrap();
        let echo = echo_server().await;
        let dialer = FakeDialer::new(echo, Some(target_addr));
        let running = HttpListener::bind(
            "127.0.0.1:0".parse().unwrap(),
            dialer.clone(),
            opts,
            CancellationToken::new(),
            TaskTracker::new(),
        )
        .await
        .unwrap();
        (running, dialer, target)
    }

    /// Reads one full HTTP/1.1 response (headers + Content-Length body) from `s`.
    async fn read_response(s: &mut TcpStream) -> (String, Vec<u8>) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(2), s.read(&mut chunk))
                .await
                .unwrap()
                .unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                let len = head
                    .lines()
                    .find_map(|l| {
                        l.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while buf.len() < pos + 4 + len {
                    let n = s.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                return (head, buf[pos + 4..].to_vec());
            }
        }
        (String::from_utf8_lossy(&buf).to_string(), Vec::new())
    }

    /// Sends raw bytes and reads until the connection closes or `until` matches.
    pub(crate) async fn raw(addr: SocketAddr, request: &str) -> (TcpStream, String) {
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(request.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; 8192];
        let mut out = String::new();
        loop {
            match tokio::time::timeout(Duration::from_millis(500), s.read(&mut buf)).await {
                Ok(Ok(0)) | Err(_) => break,
                Ok(Ok(n)) => {
                    out.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if out.contains("\r\n\r\n") {
                        break;
                    }
                }
                Ok(Err(_)) => break,
            }
        }
        (s, out)
    }

    #[tokio::test]
    async fn connect_tunnels_to_the_dialed_stream() {
        let (running, dialer) = listener(ListenerOpts::default()).await;
        let (mut s, head) = raw(
            running.local_addr,
            "CONNECT echo.test:443 HTTP/1.1\r\nHost: echo.test:443\r\n\r\n",
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        s.write_all(b"tunnelled").await.unwrap();
        let mut buf = [0u8; 9];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"tunnelled");
        drop(s);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let sessions = dialer.sessions();
        assert_eq!(sessions.len(), 1);
        let info = sessions[0].session();
        assert_eq!(
            (info.dst_host.clone(), info.dst_port, info.listener),
            (HostName::parse("echo.test"), 443, ListenerKind::Http)
        );
        assert_eq!(info.in_port, running.local_addr.port());
        assert!(sessions[0].is_finished());
    }

    #[tokio::test]
    async fn rejected_connect_closes_without_a_response() {
        let (running, _dialer) = listener(ListenerOpts {
            drop_hold: Duration::from_millis(200),
            ..ListenerOpts::default()
        })
        .await;
        for port in [443, 445, 446] {
            let (_s, head) = raw(
                running.local_addr,
                &format!("CONNECT reject.test:{port} HTTP/1.1\r\n\r\n"),
            )
            .await;
            assert!(head.is_empty(), "port {port}: {head}");
        }
        let (_s, head) = raw(running.local_addr, "CONNECT fail.test:443 HTTP/1.1\r\n\r\n").await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        // DROP holds the connection for drop_hold before closing
        let started = std::time::Instant::now();
        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        s.write_all(b"CONNECT reject.test:444 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 16];
        let n = tokio::time::timeout(Duration::from_secs(2), s.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, 0);
        assert!(started.elapsed() >= Duration::from_millis(180));
    }

    #[tokio::test]
    async fn basic_auth_compares_the_password_only() {
        let (running, _dialer) = listener(ListenerOpts {
            auth: Some(HttpAuth::Password("s3cret".to_string())),
            ..ListenerOpts::default()
        })
        .await;
        let (_s, head) = raw(running.local_addr, "CONNECT echo.test:443 HTTP/1.1\r\n\r\n").await;
        assert!(head.starts_with("HTTP/1.1 407"), "{head}");
        assert!(
            head.contains("Proxy-Authenticate: Basic realm=\"rurge\"")
                || head.contains("proxy-authenticate: Basic realm=\"rurge\""),
            "{head}"
        );
        let wrong = BASE64.encode("alice:nope");
        let (_s, head) = raw(
            running.local_addr,
            &format!(
                "CONNECT echo.test:443 HTTP/1.1\r\nProxy-Authorization: Basic {wrong}\r\n\r\n"
            ),
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 407"), "{head}");
        let right = BASE64.encode("anyone:s3cret");
        let (_s, head) = raw(
            running.local_addr,
            &format!(
                "CONNECT echo.test:443 HTTP/1.1\r\nProxy-Authorization: Basic {right}\r\n\r\n"
            ),
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let auth = HttpAuth::UserPass {
            user: "u".into(),
            password: "p".into(),
        };
        assert!(authorized(
            &auth,
            Some(&HeaderValue::from_str(&format!("Basic {}", BASE64.encode("u:p"))).unwrap())
        ));
        assert!(!authorized(
            &auth,
            Some(&HeaderValue::from_str(&format!("Basic {}", BASE64.encode("x:p"))).unwrap())
        ));
        assert!(!authorized(&auth, None));
        // RFC 7235 §2.1: the scheme token is case-insensitive
        for scheme in ["Basic", "basic", "BASIC", "bAsIc"] {
            assert!(
                authorized(
                    &auth,
                    Some(
                        &HeaderValue::from_str(&format!("{scheme} {}", BASE64.encode("u:p")))
                            .unwrap()
                    )
                ),
                "{scheme}"
            );
        }
        assert!(!authorized(
            &auth,
            Some(&HeaderValue::from_str(&format!("Bearer {}", BASE64.encode("u:p"))).unwrap())
        ));
    }

    #[test]
    fn constant_time_compare_matches_plain_equality() {
        assert!(ct_eq(b"s3cret", b"s3cret"));
        assert!(!ct_eq(b"s3cret", b"s3crey"));
        assert!(!ct_eq(b"s3cret", b"s3cre"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn hop_by_hop_headers_and_the_tokens_connection_names_are_dropped() {
        let mut headers = http::HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("keep-alive, X-Hop"));
        headers.insert("x-hop", HeaderValue::from_static("leaked"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("te", HeaderValue::from_static("trailers"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("upgrade", HeaderValue::from_static("websocket"));
        headers.insert("proxy-connection", HeaderValue::from_static("keep-alive"));
        headers.insert("x-keep", HeaderValue::from_static("kept"));
        strip_hop_by_hop(&mut headers);
        assert_eq!(
            headers.keys().map(|k| k.as_str()).collect::<Vec<_>>(),
            vec!["x-keep"]
        );
    }

    #[tokio::test]
    async fn non_proxy_requests_get_400() {
        let (running, _dialer) = listener(ListenerOpts::default()).await;
        for request in [
            // origin-form: not a proxy request at all
            "GET /index.html HTTP/1.1\r\nHost: localhost\r\n\r\n",
            // a scheme this listener cannot forward in the clear (M5)
            "GET https://target.test/x HTTP/1.1\r\nHost: target.test\r\n\r\n",
            "GET ftp://target.test/x HTTP/1.1\r\nHost: target.test\r\n\r\n",
            // CONNECT without a port (M6)
            "CONNECT echo.test HTTP/1.1\r\nHost: echo.test\r\n\r\n",
        ] {
            let (_s, head) = raw(running.local_addr, request).await;
            assert!(head.starts_with("HTTP/1.1 400"), "{request:?} → {head}");
        }
    }

    /// A client that connects and then stalls must be cut loose by
    /// `handshake_timeout`, not held until it disconnects.
    #[tokio::test]
    async fn stalled_http_handshakes_time_out() {
        let (running, _dialer) = listener(ListenerOpts {
            handshake_timeout: Duration::from_millis(200),
            ..ListenerOpts::default()
        })
        .await;
        for partial in ["", "GET http://a.test/ HTTP/1.1\r\nHost: a"] {
            let mut s = TcpStream::connect(running.local_addr).await.unwrap();
            s.write_all(partial.as_bytes()).await.unwrap();
            let mut buf = [0u8; 256];
            // hyper may answer 408 before closing; either way the socket ends.
            loop {
                let read = tokio::time::timeout(Duration::from_secs(2), s.read(&mut buf))
                    .await
                    .unwrap_or_else(|_| panic!("{partial:?} was not timed out by the listener"));
                if !matches!(read, Ok(n) if n > 0) {
                    break;
                }
            }
        }
    }

    #[tokio::test]
    async fn plain_requests_are_forwarded_per_request_with_rewritten_headers() {
        let (running, dialer, target) = listener_with_target(ListenerOpts::default()).await;
        target.set("/hello", "hi there");
        // a hop-by-hop header the origin puts on the response must not come back out
        target.set_header("/hello", "connection", "X-Down");
        target.set_header("/hello", "x-down", "leaked");
        let port = target.url("/").port().unwrap();
        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        // userinfo in the request target, a lying Host, and a pile of hop-by-hop headers
        s.write_all(
            format!(
                "GET http://u:p@target.test:{port}/hello HTTP/1.1\r\nHost: evil.internal\r\nUser-Agent: t/1\r\nProxy-Authorization: Basic eDp5\r\nProxy-Connection: keep-alive\r\nConnection: keep-alive, X-Hop\r\nX-Hop: leaked\r\nKeep-Alive: timeout=5\r\nTE: trailers\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        let (head, body) = read_response(&mut s).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body, b"hi there");
        let lower = head.to_ascii_lowercase();
        assert!(
            !lower.contains("x-down") && !lower.contains("\r\nconnection:"),
            "response hop-by-hop headers survived: {head}"
        );
        let reqs = target.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].path, "/hello");
        // RFC 7230 §5.4: Host comes from the request target, userinfo stripped
        assert_eq!(
            reqs[0].header("host"),
            Some(format!("target.test:{port}").as_str())
        );
        for gone in [
            "proxy-authorization",
            "proxy-connection",
            "connection",
            "x-hop",
            "keep-alive",
            "te",
        ] {
            assert!(reqs[0].header(gone).is_none(), "{gone} was forwarded");
        }
        assert_eq!(reqs[0].header("user-agent"), Some("t/1"));
        // second request on the same client connection hits a different rule (tinygif)
        s.write_all(b"GET http://reject.test:446/ad.gif HTTP/1.1\r\nHost: reject.test\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = read_response(&mut s).await;
        assert!(
            head.starts_with("HTTP/1.1 200")
                && head
                    .to_ascii_lowercase()
                    .contains("content-type: image/gif"),
            "{head}"
        );
        assert_eq!(body.len(), 43);
        drop(s);
        tokio::time::sleep(Duration::from_millis(150)).await;
        let sessions = dialer.sessions();
        assert_eq!(sessions.len(), 2);
        let first = sessions[0].session();
        assert_eq!(
            (first.protocol, first.dst_port),
            (Some(ProtocolKind::Http), port)
        );
        // the recorded URL is rebuilt without the `u:p@` the client sent (M11)
        assert_eq!(
            first.url.as_deref(),
            Some(format!("http://target.test:{port}/hello").as_str())
        );
        assert_eq!(first.http_host.as_deref(), Some("target.test"));
        assert_eq!(first.user_agent.as_deref(), Some("t/1"));
        assert!(
            sessions[0].is_finished(),
            "completed when the upstream connection closed"
        );
        let (up, down) = sessions[0].bytes();
        assert!(up > 0 && down > 0, "{up} {down}");
        assert_eq!(
            sessions[1].outcome(),
            Some(SessionOutcome::Rejected(RejectKind::TinyGif))
        );
    }

    #[tokio::test]
    async fn rejects_and_failures_render_pages_or_close() {
        // defaults: reject closes, failures show a 502 page
        let (running, _d, _t) = listener_with_target(ListenerOpts {
            drop_hold: Duration::from_millis(200),
            ..ListenerOpts::default()
        })
        .await;
        let (_s, head) = raw(
            running.local_addr,
            "GET http://reject.test/ HTTP/1.1\r\nHost: reject.test\r\n\r\n",
        )
        .await;
        assert!(head.is_empty(), "reject must close: {head}");
        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        s.write_all(b"GET http://dns.test/ HTTP/1.1\r\nHost: dns.test\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = read_response(&mut s).await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        let html = String::from_utf8_lossy(&body);
        assert!(
            html.contains("DNS lookup failed") && html.contains("dns.test:80"),
            "{html}"
        );
        let started = std::time::Instant::now();
        let (_s, head) = raw(
            running.local_addr,
            "GET http://reject.test:444/ HTTP/1.1\r\nHost: reject.test\r\n\r\n",
        )
        .await;
        assert!(
            head.is_empty() && started.elapsed() >= Duration::from_millis(180),
            "{head}"
        );
        // error page for rejects when enabled; no page for failures when disabled
        let (running, _d, _t) = listener_with_target(ListenerOpts {
            show_error_page: false,
            show_error_page_for_reject: true,
            ..ListenerOpts::default()
        })
        .await;
        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        s.write_all(b"GET http://reject.test:445/x HTTP/1.1\r\nHost: reject.test\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = read_response(&mut s).await;
        assert!(head.starts_with("HTTP/1.1 403"), "{head}");
        let html = String::from_utf8_lossy(&body);
        assert!(
            html.contains("REJECT-NO-DROP") && html.contains("FAKE,rule"),
            "{html}"
        );
        let (_s, head) = raw(
            running.local_addr,
            "GET http://fail.test/ HTTP/1.1\r\nHost: fail.test\r\n\r\n",
        )
        .await;
        assert!(
            head.is_empty(),
            "failure must close when pages are off: {head}"
        );
    }

    /// A plain `http://` exchange is a session like any other: `kill` has to
    /// end it, not report success and leave it running. The target here reads
    /// the request and never answers, which is the shape of the session an
    /// operator actually wants to kill (an endless download or SSE stream).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plain_forward_session_is_killable() {
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent_addr = silent.local_addr().unwrap();
        let (got_tx, got_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let mut got_tx = Some(got_tx);
            // Hold every accepted connection open and never write back.
            let mut held = Vec::new();
            while let Ok((mut s, _)) = silent.accept().await {
                if let Some(tx) = got_tx.take() {
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf).await;
                    let _ = tx.send(());
                }
                held.push(s);
            }
        });
        let echo = echo_server().await;
        let dialer = FakeDialer::new(echo, Some(silent_addr));
        let running = HttpListener::bind(
            "127.0.0.1:0".parse().unwrap(),
            dialer.clone(),
            ListenerOpts::default(),
            CancellationToken::new(),
            TaskTracker::new(),
        )
        .await
        .unwrap();

        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        s.write_all(b"GET http://target.test/hang HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        // the request reached the target, so `send_request` is in flight
        tokio::time::timeout(Duration::from_secs(3), got_rx)
            .await
            .expect("the target never saw the forwarded request")
            .unwrap();
        let handle = dialer.sessions().pop().expect("a forwarded session");
        handle.kill();

        // the client connection ends rather than hanging or getting a 502
        let mut buf = [0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(2), s.read(&mut buf))
            .await
            .expect("the killed session did not close the client connection")
            .unwrap();
        assert_eq!(n, 0, "{:?}", String::from_utf8_lossy(&buf[..n]));
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Failed("killed".into()))
        );
    }

    /// `connect` and `forward` hand `authority.host()` to `HostName::parse`
    /// without further checks. That is sound only because `http::Uri` cannot
    /// hold these bytes; this test pins the guarantee we rely on.
    #[test]
    fn an_http_authority_cannot_carry_control_characters_or_spaces() {
        for bad in [
            "a b.test:80",
            "a.test\r\nX-Evil: 1:80",
            "a\0.test:80",
            "a\t.test:80",
        ] {
            assert!(bad.parse::<http::uri::Authority>().is_err(), "{bad:?}");
            assert!(format!("http://{bad}/").parse::<Uri>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn absolute_form_keeps_the_uri_and_applies_the_upstream_headers() {
        let mut req = Request::builder()
            .method("GET")
            .uri("http://user:pw@example.test:8080/a?b=1")
            .header("Host", "lying.internal")
            .header("Proxy-Authorization", "Basic Y2xpZW50")
            .header("Proxy-Connection", "keep-alive")
            .header("Connection", "X-Hop")
            .header("X-Hop", "1")
            .header("X-Keep", "1")
            .body(())
            .unwrap();
        absolute_form(
            &mut req,
            &[
                ("Proxy-Authorization".to_string(), "Basic dXA=".to_string()),
                ("X-Pad".to_string(), "abc".to_string()),
            ],
        )
        .unwrap();
        // still absolute, without the userinfo
        assert_eq!(req.uri().to_string(), "http://example.test:8080/a?b=1");
        let h = req.headers();
        assert_eq!(h.get("host").unwrap(), "example.test:8080");
        // the client's credentials for this hop are gone, the upstream's are on
        assert_eq!(h.get("proxy-authorization").unwrap(), "Basic dXA=");
        assert_eq!(h.get("x-pad").unwrap(), "abc");
        assert_eq!(h.get("x-keep").unwrap(), "1");
        for gone in ["proxy-connection", "connection", "x-hop"] {
            assert!(h.get(gone).is_none(), "{gone}");
        }
    }

    #[test]
    fn the_upstream_headers_replace_same_name_headers_including_host() {
        let mut req = Request::builder()
            .uri("http://example.test/")
            .header("User-Agent", "client/1")
            .body(())
            .unwrap();
        absolute_form(
            &mut req,
            &[
                ("Host".to_string(), "edge.example".to_string()),
                ("User-Agent".to_string(), "rurge".to_string()),
                ("Bad Name".to_string(), "dropped".to_string()),
            ],
        )
        .unwrap();
        assert_eq!(req.headers().get("host").unwrap(), "edge.example");
        assert_eq!(req.headers().get_all("user-agent").iter().count(), 1);
        assert_eq!(req.headers().get("user-agent").unwrap(), "rurge");
        assert_eq!(
            req.headers().len(),
            2,
            "the malformed header is dropped, not sent"
        );
    }
}
