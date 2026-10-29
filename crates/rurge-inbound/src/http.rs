//! HTTP/1.1 proxy listener on hyper (M3 design §6.2, phase 4 M1 design 9):
//! CONNECT tunnels, Basic authentication, and the absolute-form requests an
//! HTTP proxy carries, which are the HTTP engine's (`Dialer::plain_requests`).

use crate::listener::{HttpAuth, ListenerOpts, Running, bind, canonical_peer, serve};
use crate::responses::{self, ResponseBody};
use crate::session::{
    ClientConn, Close, DialError, Dialer, FailKind, PlainRequests, ServeOpts, SessionHandle,
    SessionOutcome,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use http::{HeaderValue, Method, Request, Response, StatusCode, header};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use rurge_config::HostName;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub struct HttpListener;

struct Ctx {
    dialer: Arc<dyn Dialer>,
    opts: Arc<ListenerOpts>,
    /// The address the listener is bound to, which may be an unspecified one.
    local: SocketAddr,
    /// The address this connection was accepted on: rurge's end of it.
    here: Option<SocketAddr>,
    peer: SocketAddr,
    tracker: TaskTracker,
    /// The connection's `Conn::abort`: made here, once for every accepted
    /// connection, and handed to the engine with `ClientConn`. The engine
    /// cancels it when the connection has to go at once; the listener races
    /// the connection against it below (`phase 4 M1 design 8.10`, 8.11).
    abort: CancellationToken,
    /// The engine's requests of this connection, asked for when its first
    /// absolute-form request arrives (`P12`): a connection that only ever
    /// sends `CONNECT` needs no process lookup and no upstream pool.
    plain: OnceLock<Option<Arc<dyn PlainRequests>>>,
}

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
                    here: stream.local_addr().ok().map(canonical_peer),
                    peer,
                    tracker: tracker.clone(),
                    abort: CancellationToken::new(),
                    plain: OnceLock::new(),
                });
                async move { serve_connection(stream, ctx).await }
            },
        ))
    }
}

async fn serve_connection(stream: TcpStream, ctx: Arc<Ctx>) {
    let handshake_timeout = ctx.opts.handshake_timeout;
    let abort = ctx.abort.clone();
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
    let mut conn = std::pin::pin!(conn);
    // The engine's word that the connection has to go at once ends it here:
    // `rurge-http` cancels `abort` when a request is killed or stopped and
    // when a client stops reading its response (`Conn::abort`), and a
    // connection whose body is no longer polled could not see any of that. The
    // connection is the listener's, so dropping it is the listener's to do.
    let error = tokio::select! {
        biased;
        _ = abort.cancelled() => None,
        res = conn.as_mut() => res.err(),
    };
    if let Some(e) = error {
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

async fn handle(req: Request<Incoming>, ctx: Arc<Ctx>) -> Result<Response<ResponseBody>, Close> {
    let mut req = req;
    if let Some(auth) = &ctx.opts.auth
        && !authorized(auth, req.headers().get(header::PROXY_AUTHORIZATION))
    {
        return Ok(responses::proxy_auth_required());
    }
    // The client's credentials for rurge are this hop's alone: verified
    // above, and never passed on (`phase 4 M1 design 9`).
    req.headers_mut().remove(header::PROXY_AUTHORIZATION);
    if req.method() == Method::CONNECT {
        return connect(req, ctx).await;
    }
    if req.uri().scheme().is_some() && req.uri().authority().is_some() {
        return plain(req, ctx).await;
    }
    Ok(responses::bad_request(
        "rurge is an HTTP proxy: send CONNECT or an absolute-URI request",
    ))
}

/// An absolute-form request is the HTTP engine's (`phase 4 M1 design 9`): the
/// connection's `PlainRequests` answers it. A dialer without one — only a
/// test's — gets a 502 that says so.
async fn plain(req: Request<Incoming>, ctx: Arc<Ctx>) -> Result<Response<ResponseBody>, Close> {
    // TLS is tunnelled with CONNECT; anything else is not ours to forward (M5).
    if req.uri().scheme() != Some(&http::uri::Scheme::HTTP) {
        return Ok(responses::bad_request(
            "only http:// absolute URIs can be forwarded",
        ));
    }
    let requests = ctx.plain.get_or_init(|| {
        ctx.dialer.plain_requests(ClientConn {
            src: ctx.peer,
            in_port: ctx.local.port(),
            local: ctx.here,
            abort: ctx.abort.clone(),
        })
    });
    let Some(requests) = requests else {
        let authority = req.uri().authority().cloned();
        let dst = authority.as_ref().map_or(String::new(), |authority| {
            format!(
                "{}:{}",
                authority.host(),
                authority.port_u16().unwrap_or(80)
            )
        });
        return Ok(responses::error_page(
            StatusCode::BAD_GATEWAY,
            &responses::ErrorPage {
                title: "Connection failed",
                session_id: 0,
                dst,
                rule: None,
                chain: Vec::new(),
                message: "This listener has no HTTP engine behind it.".to_string(),
            },
        ));
    };
    requests.handle(req).await
}

fn session_for(ctx: &Ctx, host: HostName, port: u16) -> SessionInfo {
    let mut s = SessionInfo::tcp(host, port);
    s.src = ctx.peer;
    s.in_port = ctx.local.port();
    s.local = ctx.here;
    s.listener = ListenerKind::Http;
    s.transport = Transport::Tcp;
    s
}

async fn connect(
    mut req: Request<Incoming>,
    ctx: Arc<Ctx>,
) -> Result<Response<ResponseBody>, Close> {
    let Some(authority) = req.uri().authority().cloned() else {
        return Ok(responses::bad_request("CONNECT needs host:port"));
    };
    let host = HostName::parse(authority.host()).canonical();
    // RFC 7231 §4.3.6: the CONNECT target is always host:port, never a bare host.
    let Some(port) = authority.port_u16() else {
        return Ok(responses::bad_request("CONNECT needs host:port"));
    };
    let session = session_for(&ctx, host, port);
    if ctx.dialer.sniffable(&session) {
        // answered first, dialed after the client's first bytes (phase 4 M1
        // design 5.2); the tunnel runs on the tracker, as below
        let upgrade = hyper::upgrade::on(&mut req);
        let dialer = ctx.dialer.clone();
        let drop_hold = ctx.opts.drop_hold;
        ctx.tracker.spawn(async move {
            // an inner task restores the ERROR log for a panic (design §6.4)
            let inner = tokio::spawn(async move {
                match upgrade.await {
                    Ok(upgraded) => {
                        dialer
                            .serve(
                                session,
                                Box::new(TokioIo::new(upgraded)),
                                ServeOpts {
                                    stop: None,
                                    drop_hold,
                                    linger: Duration::ZERO,
                                },
                            )
                            .await;
                    }
                    // no handle yet: the session never began
                    Err(e) => tracing::debug!(
                        listener = "http",
                        error = %e,
                        "CONNECT upgrade failed before the session began"
                    ),
                }
            });
            if let Err(e) = inner.await
                && e.is_panic()
            {
                tracing::error!(listener = "http", "tunnel task panicked: {e}");
            }
        });
        return Ok(responses::connect_established());
    }
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
            Err(Close)
        }
        Err(DialError::Reject { .. }) => Err(Close),
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

fn failure_response(
    ctx: &Ctx,
    handle: &SessionHandle,
    message: &str,
) -> Result<Response<ResponseBody>, Close> {
    if !ctx.opts.show_error_page {
        return Err(Close);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeDialer, echo_server};
    use http::Uri;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::atomic::Ordering;
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

    /// A client may write an IPv4 target in IPv6 form (`::ffff:a.b.c.d`) in a
    /// CONNECT: the dialer, and with it the rules, get the IPv4 address, as
    /// they do from the SOCKS5 listener. (An absolute URI is the HTTP engine's
    /// and is pinned there, `phase 4 M1 design 9`.)
    #[tokio::test]
    async fn an_ipv4_mapped_target_is_dialed_as_ipv4() {
        let (running, dialer) = listener(ListenerOpts::default()).await;
        let (_tunnel, head) = raw(
            running.local_addr,
            "CONNECT [::ffff:127.0.0.1]:443 HTTP/1.1\r\nHost: [::ffff:127.0.0.1]:443\r\n\r\n",
        )
        .await;
        let want = HostName::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let dialed: Vec<(HostName, u16)> = dialer
            .sessions()
            .iter()
            .map(|h| (h.session().dst_host.clone(), h.session().dst_port))
            .collect();
        assert_eq!(dialed, [(want, 443)]);
        // `FakeDialer` reaches its echo server through "127.0.0.1" only:
        // dialed in another form, the CONNECT would have been refused
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    }

    /// What `SRC-IP`, `SRC-PORT` and `IN-PORT` rules read is taken off the
    /// connection: the client's own address with the port it connected from,
    /// and the port of the listener it came in on. The session also says which
    /// address of rurge the client connected to. (A plain request's record is
    /// pinned in the engine's own tests.)
    #[tokio::test]
    async fn a_session_names_the_client_and_the_listener_port() {
        let (running, dialer) = listener(ListenerOpts::default()).await;
        let (tunnel, head) = raw(
            running.local_addr,
            "CONNECT echo.test:443 HTTP/1.1\r\nHost: echo.test:443\r\n\r\n",
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let client = tunnel.local_addr().unwrap();
        let sessions = dialer.sessions();
        assert_eq!(sessions.len(), 1);
        let info = sessions[0].session();
        assert_eq!(info.src, client);
        assert_eq!(info.in_port, running.local_addr.port());
        assert_eq!(info.local, Some(running.local_addr));
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
    /// A sniffable `CONNECT` is answered 200 before anything is dialed, and
    /// a dial that then fails ends the tunnel with no second answer.
    #[tokio::test]
    async fn a_sniffable_connect_is_answered_before_it_is_dialed() {
        let (running, dialer) = listener(ListenerOpts::default()).await;
        dialer.sniff.store(true, Ordering::Relaxed);
        let (mut s, head) = raw(
            running.local_addr,
            "CONNECT fail.test:443 HTTP/1.1\r\nHost: fail.test:443\r\n\r\n",
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let mut eof = [0u8; 16];
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut eof))
            .await
            .expect("the tunnel ended within 5 s")
            .unwrap();
        assert_eq!(n, 0, "nothing more is said after the answer");
    }

    /// A sniffable `CONNECT` is dialed and relayed as before; only the answer
    /// comes before the dial.
    #[tokio::test]
    async fn a_sniffable_connect_is_relayed_after_the_answer() {
        let (running, dialer) = listener(ListenerOpts::default()).await;
        dialer.sniff.store(true, Ordering::Relaxed);
        let (mut s, head) = raw(
            running.local_addr,
            "CONNECT echo.test:443 HTTP/1.1\r\nHost: echo.test:443\r\n\r\n",
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        s.write_all(b"tunnelled").await.unwrap();
        let mut buf = [0u8; 9];
        tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut buf))
            .await
            .expect("the echo came back within 5 s")
            .unwrap();
        assert_eq!(&buf, b"tunnelled");
    }

    /// An absolute-form request is the engine's (`phase 4 M1 design 9`): a
    /// dialer without an HTTP engine behind it answers it 502, naming the
    /// request's target, and dials nothing. Only a test dialer is like that.
    #[tokio::test]
    async fn an_absolute_form_request_without_an_engine_is_a_502() {
        let (running, dialer) = listener(ListenerOpts::default()).await;
        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        s.write_all(
            b"GET http://echo.test/hello HTTP/1.1\r\nHost: echo.test\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf))
            .await
            .expect("the answer comes within the bound")
            .unwrap();
        let answer = String::from_utf8_lossy(&buf);
        assert!(answer.starts_with("HTTP/1.1 502"), "{answer}");
        assert!(answer.contains("no HTTP engine"), "{answer}");
        assert!(
            answer.contains("echo.test:80"),
            "the page names the request's target: {answer}"
        );
        assert!(dialer.sessions().is_empty(), "nothing was dialed");
    }
}
