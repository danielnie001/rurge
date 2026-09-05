//! HTTP/1.1 proxy listener on hyper (M3 design §6.2): CONNECT tunnels and
//! absolute-URI plain requests, Basic authentication, per-request dialing.

use crate::listener::{HttpAuth, ListenerOpts, Running, bind, serve};
use crate::responses::{self, ResponseBody};
use crate::session::{Counting, DialError, Dialer, FailKind, SessionHandle, SessionOutcome};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use http::{HeaderValue, Method, Request, Response, StatusCode, Uri, header};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
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

pub struct HttpListener;

struct Ctx {
    dialer: Arc<dyn Dialer>,
    opts: Arc<ListenerOpts>,
    local: SocketAddr,
    peer: SocketAddr,
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
    ) -> io::Result<Running> {
        let listener = bind(addr).await?;
        let local = listener.local_addr()?;
        let opts = Arc::new(opts);
        Ok(serve(
            listener,
            "http",
            opts.restrict_to_lan,
            move |stream, peer| {
                let ctx = Arc::new(Ctx {
                    dialer: dialer.clone(),
                    opts: opts.clone(),
                    local,
                    peer,
                });
                async move { serve_connection(stream, ctx).await }
            },
        ))
    }
}

async fn serve_connection(stream: TcpStream, ctx: Arc<Ctx>) {
    let service = service_fn(move |req: Request<Incoming>| {
        let ctx = ctx.clone();
        async move { handle(req, ctx).await }
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .preserve_header_case(true)
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    if let Err(e) = conn.await {
        tracing::debug!(listener = "http", error = %e, "http connection ended");
    }
}

pub(crate) fn authorized(auth: &HttpAuth, header: Option<&HeaderValue>) -> bool {
    let Some(value) = header.and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let Some(encoded) = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))
    else {
        return false;
    };
    let Ok(decoded) = BASE64.decode(encoded.trim()) else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(&decoded) else {
        return false;
    };
    let (user, password) = text.split_once(':').unwrap_or(("", text));
    match auth {
        HttpAuth::Password(p) => password == p,
        HttpAuth::UserPass {
            user: u,
            password: p,
        } => user == u && password == p,
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
    let port = authority.port_u16().unwrap_or(443);
    let session = session_for(&ctx, host, port);
    match ctx.dialer.dial(session).await {
        Ok(dialed) => {
            let upgrade = hyper::upgrade::on(&mut req);
            let dialer = ctx.dialer.clone();
            tokio::spawn(async move {
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
            Ok(responses::connect_established())
        }
        Err(DialError::Reject {
            kind: RejectKind::Drop,
            ..
        }) => {
            tokio::time::sleep(ctx.opts.drop_hold).await;
            Err(HandlerError::Close)
        }
        Err(DialError::Reject { .. }) | Err(DialError::Failed { .. }) => Err(HandlerError::Close),
    }
}

/// Rewrites a proxy request into the form the origin expects: origin-form
/// URI, a `Host` header, and no proxy-only headers.
pub(crate) fn origin_form(req: &mut Request<Incoming>) -> Result<(), http::Error> {
    let authority = req.uri().authority().map(|a| a.to_string());
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    *req.uri_mut() = path.parse::<Uri>()?;
    if !req.headers().contains_key(header::HOST)
        && let Some(a) = authority
        && let Ok(v) = HeaderValue::from_str(&a)
    {
        req.headers_mut().insert(header::HOST, v);
    }
    req.headers_mut().remove(header::PROXY_AUTHORIZATION);
    req.headers_mut().remove("proxy-connection");
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
    let host = HostName::parse(authority.host());
    let port = authority.port_u16().unwrap_or(80);
    let mut session = session_for(&ctx, host, port);
    session.protocol = Some(ProtocolKind::Http);
    session.url = Some(req.uri().to_string());
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

    let handle = dialed.handle.clone();
    let io = TokioIo::new(Counting::new(dialed.stream, handle.clone()));
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
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            conn_handle.finish(SessionOutcome::Failed(format!(
                "upstream connection error: {e}"
            )));
        } else {
            conn_handle.finish(SessionOutcome::Completed);
        }
    });
    if let Err(e) = origin_form(&mut req) {
        handle.finish(SessionOutcome::Failed(format!("bad request uri: {e}")));
        return Ok(responses::bad_request("malformed request URI"));
    }
    match sender.send_request(req).await {
        Ok(resp) => Ok(resp.map(|body| body.boxed())),
        Err(e) => {
            handle.finish(SessionOutcome::Failed(format!(
                "upstream request failed: {e}"
            )));
            failure_response(&ctx, &handle, &format!("Upstream request failed: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeDialer, echo_server};
    use rurge_net::testing::TestServer;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub(crate) async fn listener(opts: ListenerOpts) -> (Running, Arc<FakeDialer>) {
        let echo = echo_server().await;
        let dialer = FakeDialer::new(echo, None);
        let running = HttpListener::bind("127.0.0.1:0".parse().unwrap(), dialer.clone(), opts)
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
        let running = HttpListener::bind("127.0.0.1:0".parse().unwrap(), dialer.clone(), opts)
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
        assert!(head.is_empty(), "{head}");
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
    }

    #[tokio::test]
    async fn non_proxy_requests_get_400() {
        let (running, _dialer) = listener(ListenerOpts::default()).await;
        let (_s, head) = raw(
            running.local_addr,
            "GET /index.html HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 400"), "{head}");
    }

    #[tokio::test]
    async fn plain_requests_are_forwarded_per_request_with_rewritten_headers() {
        let (running, dialer, target) = listener_with_target(ListenerOpts::default()).await;
        target.set("/hello", "hi there");
        let port = target.url("/").port().unwrap();
        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        s.write_all(
            format!(
                "GET http://target.test:{port}/hello HTTP/1.1\r\nHost: target.test:{port}\r\nUser-Agent: t/1\r\nProxy-Authorization: Basic eDp5\r\nProxy-Connection: keep-alive\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        let (head, body) = read_response(&mut s).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body, b"hi there");
        let reqs = target.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].path, "/hello");
        assert!(
            reqs[0].header("proxy-authorization").is_none()
                && reqs[0].header("proxy-connection").is_none()
        );
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
}
