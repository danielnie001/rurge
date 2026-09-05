//! HTTP/1.1 proxy listener on hyper (M3 design §6.2): CONNECT tunnels and
//! absolute-URI plain requests, Basic authentication, per-request dialing.

use crate::listener::{HttpAuth, ListenerOpts, Running, bind, serve};
use crate::responses::{self, ResponseBody};
use crate::session::{DialError, Dialer, SessionOutcome};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use http::{HeaderValue, Method, Request, Response, StatusCode, header};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rurge_config::HostName;
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

async fn forward(
    _req: Request<Incoming>,
    _ctx: Arc<Ctx>,
) -> Result<Response<ResponseBody>, HandlerError> {
    // Task 7 replaces this with per-request forwarding.
    Ok(Response::builder()
        .status(StatusCode::NOT_IMPLEMENTED)
        .body(responses::empty())
        .expect("static response"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeDialer, echo_server};
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
}
