//! The HTTP/1.1 service of one client connection (`phase 4 M1 design 8`): one
//! hyper server per connection, one request at a time on HTTP/1.1, each with
//! its own session handle, its own route and its own share of the upstream
//! connections the connection keeps between its requests (`super::pool`).
//!
//! The engine stays outside this crate: which policy a request takes, how its
//! outbound connection is made and how an upgraded exchange is relayed are
//! asked through `Upstreams`. So this module never rules, never dials and
//! never opens a socket of its own; what it does own is the framing — the
//! request line and `Host` of a request that goes to an origin or to an HTTP
//! proxy, the hop-by-hop headers both ways, the idle timeouts and the guard
//! that ends a request's record whatever becomes of its future.

use super::headers::{self, Keep};
use super::pool::{Meter, Metered, Pool, Reusable};
use bytes::Bytes;
use http::{HeaderName, HeaderValue, Request, Response, StatusCode, Uri, header};
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use hyper::body::{Body, Frame, Incoming, SizeHint};
use hyper::client::conn::http1::SendRequest;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use rurge_config::HostName;
use rurge_config::rule::ProtocolKind;
use rurge_config::session::SessionInfo;
use rurge_inbound::responses::{self, BoxError, ResponseBody};
use rurge_inbound::{Close, FailKind, SessionHandle, SessionOutcome};
use rurge_net::BoxFuture;
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::time::Sleep;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Where the connection came in (`phase 4 M1 design 8.1`, 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// Sniffed as HTTP (or forced in, design 6.4): a request's target is the
    /// connection's.
    Sniffed,
    /// The HTTP listener's absolute-form requests: a request's target is its
    /// URL's.
    Listener,
}

/// One client connection the engine serves (`phase 4 M1 design 8.1`).
pub struct Conn {
    /// src, in_port, local, listener, transport and, for `Sniffed`, the target
    /// and the SNI.
    pub session: SessionInfo,
    pub entry: Entry,
    /// `idle_timeout`: waiting for a response head, and between two frames of
    /// a body (design 8.10).
    pub idle: Duration,
    /// Reading a request head, the keep-alive idle included: `idle` for
    /// `Sniffed`, the listener's `handshake_timeout` for `Listener` (design
    /// 8.10, 9).
    pub head_wait: Duration,
    /// REJECT-DROP (design 8.8).
    pub drop_hold: Duration,
    /// Cancelled by the HTTP engine when the connection has to go at once — a
    /// request killed or stopped, or a client that stopped reading (design
    /// 8.10, 8.11). Whoever serves the connection drops it then. Not a child of
    /// `token`.
    pub abort: CancellationToken,
    /// The connection's token; each request's token is its child (design 8.11).
    pub token: CancellationToken,
    /// The engine's tracker: the upstream connections' hyper tasks and the
    /// relays of upgrades run on it.
    pub tracker: TaskTracker,
}

/// What the engine says the name behind a fake address is (`phase 4 M1 design
/// 9`, R27): the listener asks it before a request is routed.
pub enum Named<L> {
    /// Not an address rurge hands out: the request's target stands.
    No,
    /// The name, and the lease that keeps the address from being handed to
    /// someone else for as long as the request lasts.
    Name(String, L),
    /// An address of rurge's own with no name behind it: the request fails.
    Unknown,
}

/// What `route` worked out (`phase 4 M1 design 8.2`).
pub enum Routed<R, K> {
    /// The rules and the policy: `route` is what `dial` needs, `key` what the
    /// pool keeps upstream connections under (`design 8.5`).
    Go { route: R, key: K, pages: Pages },
    /// A refusal, unanswered: the HTTP engine answers it and finishes the
    /// handle (`P4`).
    Refused { refusal: Refusal, pages: Pages },
}

/// What answers a refusal or a failure: the generation the request was routed
/// by says (`phase 4 M1 design 8.8`, `P17`).
#[derive(Clone, Copy, Debug)]
pub struct Pages {
    pub error: bool,
    pub reject: bool,
}

/// A request the engine will not carry, and why (`phase 4 M1 design 8.8`).
#[derive(Clone, Debug)]
pub enum Refusal {
    Reject(RejectKind),
    Fail { kind: FailKind, message: String },
}

/// What `dial` brought back (`phase 4 M1 design 8.4`).
pub struct Upstream {
    pub stream: BoxedStream,
    /// `Some` when `stream` leads to an HTTP proxy that takes the request in
    /// absolute form: the headers to put on it (`Dialed::forward`).
    pub forward: Option<Vec<(String, String)>>,
    /// Where the dial connected to (the handle's `Trail`), kept with the
    /// pooled connection for the requests that borrow it (design 8.4, 10.1).
    pub peer: Option<SocketAddr>,
}

/// What the HTTP engine asks of the engine, per client connection (`P4`,
/// `P17`).
pub trait Upstreams: Send + Sync + 'static {
    /// The pool's key (design 8.5): equal keys may share an upstream
    /// connection.
    type Key: PartialEq + Send + Sync + 'static;
    /// What `route` worked out and `dial` needs; opaque here.
    type Route: Send + 'static;
    /// Held while a request whose target was a fake address lasts (design 9,
    /// R27).
    type Lease: Send + 'static;

    /// The name behind a fake address a request names (`Entry::Listener`
    /// only).
    fn named(&self, host: &HostName) -> Named<Self::Lease>;
    /// The request's handle, made before anything is awaited, so that the
    /// request's guard holds it from the start (design 8.11): numbered,
    /// registered, its token a child of `Conn::token`, the connection's
    /// process slot.
    fn open(&self, session: SessionInfo) -> Arc<SessionHandle>;
    /// The rules and the policy (design 8.4 "选路"). Refusals come back
    /// unfinished: the HTTP engine answers, sets the status and finishes the
    /// handle (`P4`).
    fn route<'a>(
        &'a self,
        handle: &'a Arc<SessionHandle>,
        forwardable: bool,
    ) -> BoxFuture<'a, Routed<Self::Route, Self::Key>>;
    /// Dials what `route` chose (design 8.4 "拨号"), in the request's own
    /// task.
    fn dial<'a>(
        &'a self,
        handle: &'a Arc<SessionHandle>,
        route: Self::Route,
    ) -> BoxFuture<'a, Result<Upstream, Refusal>>;
    /// Relays an upgraded exchange to its end and finishes `handle` (design
    /// 8.9).
    fn relay<'a>(
        &'a self,
        client: BoxedStream,
        upstream: BoxedStream,
        handle: Arc<SessionHandle>,
    ) -> BoxFuture<'a, ()>;
}

/// The body of a request the engine sends upstream: what the client sent,
/// with the idle timeout of `Conn::idle` between its frames.
type ReqBody = BoxBody<Bytes, BoxError>;

/// An upstream connection sitting idle in the pool: its sender, who is lent
/// it, where its dial went and the headers an HTTP proxy takes the request
/// with (`phase 4 M1 design 8.4, 8.5`).
struct Pooled {
    sender: SendRequest<ReqBody>,
    meter: Meter,
    peer: Option<SocketAddr>,
    forward: Option<Vec<(String, String)>>,
}

impl Reusable for Pooled {
    fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    fn is_ready(&self) -> bool {
        self.sender.is_ready()
    }
}

/// What one client connection carries between its requests.
struct Shared<U: Upstreams> {
    conn: Conn,
    upstreams: U,
    pool: Mutex<Pool<U::Key, Pooled>>,
    /// How many requests reached `route` (`P10`): a connection that reaches no
    /// route gets a record of its own instead (`design 10.2`).
    routed: AtomicU64,
}

/// The flow of every request of one client connection (`phase 4 M1 design
/// 8.2`). Cloned per request; the clones share the connection's pool and
/// `Upstreams`.
pub struct Requests<U: Upstreams> {
    shared: Arc<Shared<U>>,
}

impl<U: Upstreams> Clone for Requests<U> {
    fn clone(&self) -> Requests<U> {
        Requests {
            shared: self.shared.clone(),
        }
    }
}

impl<U: Upstreams> Requests<U> {
    pub fn new(conn: Conn, upstreams: U) -> Requests<U> {
        Requests {
            shared: Arc::new(Shared {
                conn,
                upstreams,
                pool: Mutex::new(Pool::new()),
                routed: AtomicU64::new(0),
            }),
        }
    }

    /// The connection's token: cancelling it stops every request on it.
    pub fn token(&self) -> &CancellationToken {
        &self.shared.conn.token
    }

    /// How many requests reached `route` so far (`P10`).
    pub fn routed(&self) -> u64 {
        self.shared.routed.load(Ordering::Relaxed)
    }

    pub async fn handle(&self, req: Request<Incoming>) -> Result<Response<ResponseBody>, Close> {
        let mut req = req;
        // 1. what the rules see (design 8.3). A listener request that is not
        // an absolute `http://` one is no request of this engine's (design 9).
        let authority = match self.shared.conn.entry {
            Entry::Sniffed => None,
            // The listener handles `CONNECT` itself (its own tunnel, before a
            // request ever reaches this engine); one that reaches here has an
            // authority-form request line and no URL, and is a bad request,
            // answered before a handle is made (design 8.9).
            Entry::Listener => match req.uri().authority().cloned() {
                None => return Ok(responses::bad_request("absolute URI without a host")),
                Some(_) if req.uri().scheme() != Some(&http::uri::Scheme::HTTP) => {
                    return Ok(responses::bad_request(
                        "only http:// absolute URIs can be forwarded",
                    ));
                }
                Some(authority) => Some(authority),
            },
        };
        let prepared = self.prepare(&mut req, authority);

        // 2. the request's handle, and the guard that ends it whatever
        // happens to the future (design 8.11, P18)
        let handle = self.shared.upstreams.open(prepared.session);
        let guard = Guard::new(handle.clone(), self.shared.conn.token.clone());
        handle.set_method(req.method().as_str());
        if let Some(lease) = prepared.lease {
            handle.on_finish(move |_, _| drop(lease));
        }

        // 3. the rules and the policy. Not raced against the token: `route`
        // holds the handle, and ends it itself in its own time (P1).
        self.shared.routed.fetch_add(1, Ordering::Relaxed);
        let routed = self
            .shared
            .upstreams
            .route(&handle, prepared.forwardable)
            .await;
        if handle.token().is_cancelled() {
            return Err(Close);
        }
        let (route, key, pages) = match routed {
            Routed::Refused { refusal, pages } => {
                return self.answer(&guard, refusal, pages).await;
            }
            Routed::Go { route, key, pages } => (route, key, pages),
        };
        if prepared.unknown {
            let refusal = Refusal::Fail {
                kind: FailKind::Other,
                message: "fake address not known".to_string(),
            };
            return self.answer(&guard, refusal, pages).await;
        }

        // 4. an upstream connection: one idle in the pool under this key, or
        // a fresh dial in this task (design 8.4, 8.5)
        let borrowed = {
            let mut pool = self.shared.pool.lock().expect("pool");
            pool.take(&key, Instant::now())
        };
        // The route of a borrowed connection is still unused: a request the
        // borrowed connection does not carry is sent again on a connection
        // dialled with it (design 8.6).
        let (mut sender, mut meter, mut forward, mut peer, mut route) = match borrowed {
            Some(pooled) => {
                // a borrowed connection is ready now, and its dial is the one
                // the record's peer address and bytes belong to
                handle.mark_connected();
                if let Some(peer) = pooled.peer {
                    handle.trail().set_peer(peer);
                }
                pooled.meter.set(Some(handle.clone()));
                (
                    pooled.sender,
                    pooled.meter,
                    pooled.forward,
                    pooled.peer,
                    Some(route),
                )
            }
            None => {
                let (sender, meter, forward, peer) = match self.redial(&guard, pages, route).await {
                    Ok(parts) => parts,
                    Err(answer) => return answer,
                };
                (sender, meter, forward, peer, None)
            }
        };

        // 5. the request as its upstream takes it (design 8.7), sent with the
        // idle timeout between the frames of its body
        let sniffed = self.shared.conn.entry == Entry::Sniffed;
        // Only an upgrade keeps `Upgrade` and `Connection: upgrade`; every
        // other request drops them (design 8.7, 8.9).
        let upgrade = wants_upgrade(&req);
        headers::strip(
            req.headers_mut(),
            Keep {
                upgrade,
                proxy_authorization: sniffed,
            },
        );
        if !sniffed {
            let rewritten = match &forward {
                Some(extra) => absolute_form(&mut req, extra),
                None => origin_form(&mut req),
            };
            if let Err(e) = rewritten {
                handle.set_status(StatusCode::BAD_REQUEST.as_u16());
                handle.finish(SessionOutcome::Failed(format!("bad request uri: {e}")));
                return Ok(responses::bad_request("malformed request URI"));
            }
        }
        // A body whose length nobody named goes upstream chunked: hyper reads
        // an unknown length as no body at all for GET, HEAD and CONNECT
        // (`proto/h1/role.rs`), which would drop the body silently. A body
        // already at its end cannot reach here, and hyper drops the header
        // again for one that ends up with nothing to send.
        if !req.body().is_end_stream() && req.body().size_hint().exact().is_none() {
            req.headers_mut().insert(
                header::TRANSFER_ENCODING,
                HeaderValue::from_static("chunked"),
            );
        }
        // The client's side of an upgrade is taken before the request is
        // split: a request that asked for one, and a `CONNECT` on a sniffed
        // connection, whose 2xx answer becomes a tunnel (design 8.9).
        let tunnel = sniffed && req.method() == http::Method::CONNECT;
        // An idempotent request with no body keeps its head: if a borrowed
        // connection cuts it off after it was written, it goes again on a new
        // one (design 8.6).
        let head = (route.is_some()
            && is_idempotent(req.method())
            && (req.body().is_end_stream() || req.body().size_hint().exact() == Some(0)))
        .then(|| head_of(&req));
        let client_upgrade = (upgrade || tunnel).then(|| hyper::upgrade::on(&mut req));
        let idle = self.shared.conn.idle;
        let progress = Progress::new();
        let (parts, body) = req.into_parts();
        let req = Request::from_parts(parts, Idle::new(body, idle, progress.clone()).boxed());
        // `biased`: once the token is cancelled the answer is "closed by
        // policy", never the error page a dropped upstream would produce; a
        // response head that arrived beats the deadline that ran out
        let answered = tokio::select! {
            biased;
            _ = handle.token().cancelled() => return Err(Close),
            sent = send(&mut sender, req) => sent,
            _ = head_deadline(progress.clone(), idle) => return self.timed_out(&guard, pages),
        };
        let mut resp = match answered {
            Sent::Answered(resp) => resp,
            Sent::NotWritten(req, e) => {
                // Not a byte of the request went out (hyper hands it back
                // whole): any method goes again, whole, on a new connection
                // (design 8.6).
                let Some(route) = route.take() else {
                    return self.upstream_failed(&guard, pages, e);
                };
                meter.set(None);
                let sent = match self.retry(&guard, pages, route, req, &progress, idle).await {
                    Ok((sent, new_sender, new_meter, new_forward, new_peer)) => {
                        sender = new_sender;
                        meter = new_meter;
                        forward = new_forward;
                        peer = new_peer;
                        sent
                    }
                    Err(answer) => return answer,
                };
                match sent {
                    Sent::Answered(resp) => resp,
                    Sent::NotWritten(_, e) | Sent::Failed(e) => {
                        return self.upstream_failed(&guard, pages, e);
                    }
                }
            }
            Sent::Failed(e) => {
                // The request may have been written: only an idempotent one
                // with no body whose connection cut the answer off goes again,
                // from the head kept above (design 8.6).
                let (Some(head), Some(route), true) =
                    (head, route.take(), e.is_incomplete_message())
                else {
                    return self.upstream_failed(&guard, pages, e);
                };
                meter.set(None);
                let sent = match self
                    .retry(&guard, pages, route, with_empty_body(head), &progress, idle)
                    .await
                {
                    Ok((sent, new_sender, new_meter, new_forward, new_peer)) => {
                        sender = new_sender;
                        meter = new_meter;
                        forward = new_forward;
                        peer = new_peer;
                        sent
                    }
                    Err(answer) => return answer,
                };
                match sent {
                    Sent::Answered(resp) => resp,
                    Sent::NotWritten(_, e) | Sent::Failed(e) => {
                        return self.upstream_failed(&guard, pages, e);
                    }
                }
            }
        };
        if forward.is_some() && resp.status() == StatusCode::PROXY_AUTHENTICATION_REQUIRED {
            // the credentials in question are rurge's own, towards its
            // upstream: the client can do nothing about them, and the
            // challenge header is hop-by-hop. Both texts come from the status
            // code, never from the upstream's reason phrase.
            return self.failed(
                &guard,
                pages,
                "http proxy answered 407 Proxy Authentication Required".to_string(),
                "Upstream proxy rejected the request: 407 Proxy Authentication Required"
                    .to_string(),
            );
        }

        // 7. the answer (design 8.2): an upgrade first, then its status, its
        // headers, and a body that ends the request when it ends and puts the
        // connection back. The branch for an upgrade does not build a `Tail`:
        // a 101 and a 2xx to `CONNECT` have no body, so a `Tail` would end the
        // request at once and put the connection back, while a relay is about
        // to run on it (design 8.9).
        let switching = resp.status() == StatusCode::SWITCHING_PROTOCOLS;
        if switching && !upgrade {
            // the upstream switched protocols nobody asked it to; a proxy
            // answers 502, as Go's reverse proxy does (design 8.9)
            return self.failed(
                &guard,
                pages,
                "upstream answered 101 Switching Protocols unasked".to_string(),
                "Upstream answered 101 Switching Protocols to a request that did not ask to upgrade"
                    .to_string(),
            );
        }
        if (upgrade && switching) || (tunnel && resp.status().is_success()) {
            let upstream_upgrade = hyper::upgrade::on(&mut resp);
            let client_upgrade = client_upgrade.expect("an upgrade was asked for");
            return Ok(self.promote(guard, resp, client_upgrade, upstream_upgrade, meter, tunnel));
        }
        handle.set_status(resp.status().as_u16());
        headers::strip(resp.headers_mut(), Keep::NONE);
        let (parts, body) = resp.into_parts();
        let outing = Outing {
            key,
            sender,
            meter,
            peer,
            forward,
        };
        let tail = Tail::new(body, guard, self.shared.clone(), outing);
        Ok(Response::from_parts(parts, tail.boxed()))
    }

    /// The session of one request, before a handle is made of it: what the
    /// rules see, whether the request may be forwarded in absolute form, the
    /// lease of a fake address turned back into its name, and whether the
    /// address had no name at all (`phase 4 M1 design 8.3`, 9). `authority` is
    /// the listener request's request-line authority, if it came from there.
    fn prepare(
        &self,
        req: &mut Request<Incoming>,
        authority: Option<http::uri::Authority>,
    ) -> Prepared<U::Lease> {
        let mut session = self.shared.conn.session.clone();
        session.protocol = Some(ProtocolKind::Http);
        session.user_agent = req
            .headers()
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mut lease = None;
        let mut unknown = false;
        let forwardable = match self.shared.conn.entry {
            Entry::Sniffed => false,
            Entry::Listener => {
                if let Some(authority) = authority {
                    let port = authority.port_u16().unwrap_or(80);
                    let mut host = HostName::parse(authority.host()).canonical();
                    match self.shared.upstreams.named(&host) {
                        Named::No => {}
                        Named::Name(name, held) => {
                            rename(req, &authority, &name);
                            host = HostName::parse(&name).canonical();
                            lease = Some(held);
                        }
                        Named::Unknown => unknown = true,
                    }
                    session.dst_host = host;
                    session.dst_port = port;
                }
                true
            }
        };
        // A `CONNECT` on a sniffed connection names its target in the request
        // line and goes there as it came, so it has no URL; the host the rules
        // see is the request line's (design 8.9).
        let (url, http_host) = if req.method() == http::Method::CONNECT {
            (
                None,
                req.uri()
                    .authority()
                    .map(|authority| authority.host().to_ascii_lowercase()),
            )
        } else {
            let (url, host) = form_of(req, (&session.dst_host, session.dst_port));
            (Some(url), host)
        };
        session.url = url;
        session.http_host = http_host;
        Prepared {
            session,
            forwardable,
            lease,
            unknown,
        }
    }

    /// Answers a refusal the engine handed back unfinished (design 8.8, P4):
    /// the status first where there is a response, then the handle, then the
    /// response.
    async fn answer(
        &self,
        guard: &Guard,
        refusal: Refusal,
        pages: Pages,
    ) -> Result<Response<ResponseBody>, Close> {
        let handle = guard.handle();
        match refusal {
            Refusal::Reject(RejectKind::TinyGif) => {
                handle.set_status(StatusCode::OK.as_u16());
                handle.finish(SessionOutcome::Rejected(RejectKind::TinyGif));
                Ok(responses::tiny_gif())
            }
            Refusal::Reject(kind @ (RejectKind::Reject | RejectKind::NoDrop)) => {
                if !pages.reject {
                    handle.finish(SessionOutcome::Rejected(kind));
                    return Err(Close);
                }
                let page = self.page(
                    handle,
                    StatusCode::FORBIDDEN,
                    "Request rejected",
                    format!("The request was rejected by the {} policy.", kind.name()),
                );
                handle.set_status(StatusCode::FORBIDDEN.as_u16());
                handle.finish(SessionOutcome::Rejected(kind));
                Ok(page)
            }
            Refusal::Reject(RejectKind::Drop) => {
                handle.finish(SessionOutcome::Rejected(RejectKind::Drop));
                let hold = self.shared.conn.drop_hold;
                // a client that leaves, or a stop, ends the hold early
                tokio::select! {
                    biased;
                    _ = handle.token().cancelled() => {}
                    _ = tokio::time::sleep(hold) => {}
                }
                Err(Close)
            }
            Refusal::Fail { kind, message } => self.failed(
                guard,
                pages,
                message.clone(),
                format!("{}: {message}", failure_prefix(kind)),
            ),
        }
    }

    /// Answers with the connection failed page, or with no answer at all when
    /// the generation says no page (`phase 4 M1 design 8.8`).
    fn failed(
        &self,
        guard: &Guard,
        pages: Pages,
        record: String,
        page: String,
    ) -> Result<Response<ResponseBody>, Close> {
        let handle = guard.handle();
        if !pages.error {
            handle.finish(SessionOutcome::Failed(record));
            return Err(Close);
        }
        let response = self.page(handle, StatusCode::BAD_GATEWAY, "Connection failed", page);
        handle.set_status(StatusCode::BAD_GATEWAY.as_u16());
        handle.finish(SessionOutcome::Failed(record));
        Ok(response)
    }

    /// Answers a response head that did not come in time (`design 8.10`).
    fn timed_out(&self, guard: &Guard, pages: Pages) -> Result<Response<ResponseBody>, Close> {
        let handle = guard.handle();
        let record = "upstream response timed out".to_string();
        if !pages.error {
            handle.finish(SessionOutcome::Failed(record));
            return Err(Close);
        }
        let response = self.page(
            handle,
            StatusCode::GATEWAY_TIMEOUT,
            "Gateway timeout",
            "The upstream server did not answer in time.".to_string(),
        );
        handle.set_status(StatusCode::GATEWAY_TIMEOUT.as_u16());
        handle.finish(SessionOutcome::Failed(record));
        Ok(response)
    }

    fn page(
        &self,
        handle: &Arc<SessionHandle>,
        status: StatusCode,
        title: &'static str,
        message: String,
    ) -> Response<ResponseBody> {
        let session = handle.session();
        responses::error_page(
            status,
            &responses::ErrorPage {
                title,
                session_id: handle.id(),
                dst: format!("{}:{}", session.dst_host, session.dst_port),
                rule: handle.rule(),
                chain: handle.policy_chain(),
                message,
            },
        )
    }

    /// Dials the route and starts its hyper client on the tracker
    /// (`phase 4 M1 design 8.4`): the first send and a send after the request
    /// was not written both come through here. The dial is not raced against
    /// the request's token; what it brings back is dropped when the token
    /// fired meanwhile (`P1`).
    async fn connect(
        &self,
        handle: &Arc<SessionHandle>,
        route: U::Route,
    ) -> Result<
        (
            SendRequest<ReqBody>,
            Meter,
            Option<Vec<(String, String)>>,
            Option<SocketAddr>,
        ),
        DialError,
    > {
        let upstream = match self.shared.upstreams.dial(handle, route).await {
            Ok(upstream) => upstream,
            Err(refusal) => return Err(DialError::Refused(refusal)),
        };
        if handle.token().is_cancelled() {
            return Err(DialError::Stopped);
        }
        let meter = Meter::new();
        meter.set(Some(handle.clone()));
        let io = TokioIo::new(Metered::new(upstream.stream, meter.clone()));
        let (sender, conn) = match hyper::client::conn::http1::Builder::new()
            .preserve_header_case(true)
            .handshake(io)
            .await
        {
            Ok(pair) => pair,
            Err(e) => return Err(DialError::Handshake(e.to_string())),
        };
        // The connection runs on the engine's tracker; an inner task restores
        // the ERROR log for a panic. It finishes no handle: the response
        // body's wrapper, the request's guard or the relay of an upgrade do.
        let tracker = self.shared.conn.tracker.clone();
        tracker.spawn(async move {
            let inner = tokio::spawn(async move {
                if let Err(e) = conn.with_upgrades().await {
                    tracing::debug!(error = %e, "upstream connection ended");
                }
            });
            if let Err(e) = inner.await
                && e.is_panic()
            {
                tracing::error!("upstream connection task panicked: {e}");
            }
        });
        Ok((sender, meter, upstream.forward, upstream.peer))
    }

    /// A new connection for a request sent again, with the failures of the
    /// dial already answered for the client (`phase 4 M1 design 8.6`, `P4`).
    async fn redial(
        &self,
        guard: &Guard,
        pages: Pages,
        route: U::Route,
    ) -> Result<
        (
            SendRequest<ReqBody>,
            Meter,
            Option<Vec<(String, String)>>,
            Option<SocketAddr>,
        ),
        Result<Response<ResponseBody>, Close>,
    > {
        match self.connect(guard.handle(), route).await {
            Ok(parts) => Ok(parts),
            Err(DialError::Refused(refusal)) => Err(self.answer(guard, refusal, pages).await),
            Err(DialError::Stopped) => Err(Err(Close)),
            Err(DialError::Handshake(e)) => Err(self.failed(
                guard,
                pages,
                format!("upstream handshake failed: {e}"),
                format!("Upstream handshake failed: {e}"),
            )),
        }
    }

    /// The one send a request sent again gets: on a new connection under the
    /// same route, with the wait for the answer started over (`phase 4 M1
    /// design 8.6`, `P1`). The new connection's parts come back with the
    /// result so that the caller keeps answering on it.
    async fn retry(
        &self,
        guard: &Guard,
        pages: Pages,
        route: U::Route,
        req: Request<ReqBody>,
        progress: &Arc<Progress>,
        idle: Duration,
    ) -> Result<
        (
            Sent,
            SendRequest<ReqBody>,
            Meter,
            Option<Vec<(String, String)>>,
            Option<SocketAddr>,
        ),
        Result<Response<ResponseBody>, Close>,
    > {
        let (mut sender, meter, forward, peer) = match self.redial(guard, pages, route).await {
            Ok(parts) => parts,
            Err(answer) => return Err(answer),
        };
        let handle = guard.handle();
        handle.mark_connected();
        if let Some(peer) = peer {
            handle.trail().set_peer(peer);
        }
        meter.set(Some(handle.clone()));
        progress.restart();
        let sent = tokio::select! {
            biased;
            _ = handle.token().cancelled() => return Err(Err(Close)),
            sent = send(&mut sender, req) => sent,
            _ = head_deadline(progress.clone(), idle) => {
                return Err(self.timed_out(guard, pages));
            }
        };
        Ok((sent, sender, meter, forward, peer))
    }

    /// The send failed for good: upstream is counted as failed unless the
    /// error was the client's own, and the client gets the failure page
    /// (`phase 4 M1 design 8.8`).
    fn upstream_failed(
        &self,
        guard: &Guard,
        pages: Pages,
        e: hyper::Error,
    ) -> Result<Response<ResponseBody>, Close> {
        if !e.is_user() {
            guard.handle().mark_upstream_failed();
        }
        self.failed(
            guard,
            pages,
            format!("upstream request failed: {e}"),
            format!("Upstream request failed: {e}"),
        )
    }

    /// Hands an upgraded exchange to a relay on the tracker (`phase 4 M1
    /// design 8.9`): the status and the headers the client needs, the bytes
    /// the upstream read ahead, and the guard, whose handle the relay ends.
    /// The upstream connection is not put back: it is the tunnel now.
    fn promote(
        &self,
        guard: Guard,
        mut resp: Response<Incoming>,
        client: hyper::upgrade::OnUpgrade,
        upstream: hyper::upgrade::OnUpgrade,
        meter: Meter,
        tunnel: bool,
    ) -> Response<ResponseBody> {
        let handle = guard.handle().clone();
        handle.set_status(resp.status().as_u16());
        if tunnel {
            headers::strip(resp.headers_mut(), Keep::NONE);
            // a 2xx to `CONNECT` carries no length: hyper's server voids the
            // answer if one is there (`proto/h1/role.rs`, RFC 9110 §9.3.6)
            resp.headers_mut().remove(header::CONTENT_LENGTH);
        } else {
            headers::strip(
                resp.headers_mut(),
                Keep {
                    upgrade: true,
                    proxy_authorization: false,
                },
            );
        }
        let (parts, _) = resp.into_parts();
        let abort = self.shared.conn.abort.clone();
        let idle = self.shared.conn.idle;
        let shared = self.shared.clone();
        let tracker = self.shared.conn.tracker.clone();
        tracker.spawn(async move {
            let inner = tokio::spawn(async move {
                // the guard lives with the relay: it is what ends the handle
                // if the task is cut off before the relay does
                let _guard = guard;
                let (client, upstream) = tokio::select! {
                    biased;
                    _ = handle.token().cancelled() => {
                        // the server may still be writing the 101; ending the
                        // client connection is what closes it
                        abort.cancel();
                        return;
                    }
                    pair = async { tokio::join!(client, upstream) } => pair,
                };
                let (client, upstream) = match (client, upstream) {
                    (Ok(client), Ok(upstream)) => (client, upstream),
                    (Err(e), _) | (_, Err(e)) => {
                        handle.finish(SessionOutcome::Failed(format!("upgrade failed: {e}")));
                        return;
                    }
                };
                // The bytes hyper read ahead while it parsed the 101 are
                // already counted as down; from here the relay counts, so the
                // meter is cleared before anything reads the upstream again
                // (design 8.9).
                meter.set(None);
                let (client, upstream): (BoxedStream, BoxedStream) = match upstream
                    .downcast::<TokioIo<Metered<BoxedStream>>>()
                {
                    Ok(parts) => {
                        let hyper::upgrade::Parts { io, read_buf, .. } = parts;
                        let mut client = TokioIo::new(client);
                        if !read_buf.is_empty() {
                            // the client is owed these before the tunnel opens
                            let wrote = tokio::select! {
                                biased;
                                _ = handle.token().cancelled() => None,
                                res = tokio::time::timeout(idle, client.write_all(&read_buf)) => {
                                    Some(res)
                                }
                            };
                            match wrote {
                                None => {
                                    abort.cancel();
                                    return;
                                }
                                Some(Ok(Ok(()))) => {}
                                Some(Ok(Err(e))) => {
                                    handle.finish(SessionOutcome::Failed(format!(
                                        "upgrade failed: {e}"
                                    )));
                                    return;
                                }
                                Some(Err(_)) => {
                                    handle.finish(SessionOutcome::Failed(
                                        "upgrade failed: writing to the client timed out"
                                            .to_string(),
                                    ));
                                    return;
                                }
                            }
                        }
                        (Box::new(client), Box::new(io.into_inner()))
                    }
                    Err(upstream) => (
                        Box::new(TokioIo::new(client)),
                        Box::new(TokioIo::new(upstream)),
                    ),
                };
                shared
                    .upstreams
                    .relay(client, upstream, handle.clone())
                    .await;
            });
            if let Err(e) = inner.await
                && e.is_panic()
            {
                tracing::error!("upgrade relay panicked: {e}");
            }
        });
        Response::from_parts(parts, responses::empty())
    }
}

/// One request's session, worked out before its handle is made.
struct Prepared<L> {
    session: SessionInfo,
    forwardable: bool,
    lease: Option<L>,
    /// The request named an address of rurge's own with no name behind it: it
    /// is answered as a failure once the rules have seen it (`design 9`).
    unknown: bool,
}

/// Why a fresh upstream connection could not be had (`phase 4 M1 design 8.4`).
enum DialError {
    /// The engine's `dial` refused, unanswered (`P4`).
    Refused(Refusal),
    /// The request's token fired while the connection was being dialled: the
    /// dial is not raced against it, but what it brings back is dropped (`P1`).
    Stopped,
    /// The stream was there, hyper's handshake was not.
    Handshake(String),
}

/// The three ways one send can end (`phase 4 M1 design 8.6`).
enum Sent {
    /// A response head came back.
    Answered(Response<Incoming>),
    /// The request was never written and is handed back whole, to be sent
    /// again on another connection.
    NotWritten(Request<ReqBody>, hyper::Error),
    /// The request may have been written; a second attempt has to be built
    /// from its head, if the request allows one at all.
    Failed(hyper::Error),
}

/// Sends one request and tells the caller whether it was written
/// (`phase 4 M1 design 8.6`).
async fn send(sender: &mut SendRequest<ReqBody>, req: Request<ReqBody>) -> Sent {
    match sender.try_send_request(req).await {
        Ok(resp) => Sent::Answered(resp),
        Err(mut e) => match e.take_message() {
            Some(req) => Sent::NotWritten(req, e.into_error()),
            None => Sent::Failed(e.into_error()),
        },
    }
}

/// The methods a request may be sent again after it was written (RFC 9110
/// §9.2.2, `phase 4 M1 design 8.6`).
fn is_idempotent(method: &http::Method) -> bool {
    matches!(
        *method,
        http::Method::GET
            | http::Method::HEAD
            | http::Method::OPTIONS
            | http::Method::TRACE
            | http::Method::PUT
            | http::Method::DELETE
    )
}

/// Whether a request asks for an upgrade (WebSocket, `phase 4 M1 design 8.9`):
/// HTTP/1.1, an `Upgrade` header, and a `Connection` that lists the `upgrade`
/// token. The same reading Go's reverse proxy makes.
fn wants_upgrade<B>(req: &Request<B>) -> bool {
    req.version() == http::Version::HTTP_11
        && req.headers().contains_key(header::UPGRADE)
        && req
            .headers()
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
}

/// A request's head alone, kept so that an idempotent request with no body can
/// be built again after its connection cut it off (`phase 4 M1 design 8.6`).
fn head_of<B>(req: &Request<B>) -> Request<()> {
    let mut head = Request::builder()
        .method(req.method().clone())
        .version(req.version())
        .uri(req.uri().clone())
        .body(())
        .expect("a request's own head is a valid request");
    *head.headers_mut() = req.headers().clone();
    head
}

/// The request of a kept head, with no body at all (`phase 4 M1 design 8.6`).
fn with_empty_body(head: Request<()>) -> Request<ReqBody> {
    let (parts, _) = head.into_parts();
    Request::from_parts(
        parts,
        http_body_util::Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed(),
    )
}

/// The three prefixes a failed connection's page and record start with, by the
/// kind of failure (`phase 4 M1 design 8.8`).
fn failure_prefix(kind: FailKind) -> &'static str {
    match kind {
        FailKind::Dns => "DNS lookup failed",
        FailKind::Timeout => "Connection timed out",
        FailKind::Connect | FailKind::Other => "Connection failed",
    }
}

/// The path and query of a URI, or `/`.
fn path_and_query(uri: &Uri) -> &str {
    uri.path_and_query().map(|p| p.as_str()).unwrap_or("/")
}

/// The URL of a request and the host the rules see (`phase 4 M1 design 8.3`,
/// P22): an absolute-form request line for both, or `Host` and the request
/// line's path for an origin-form one.
fn form_of<B>(req: &Request<B>, fallback: (&HostName, u16)) -> (String, Option<String>) {
    if req.uri().scheme().is_some()
        && let Some(authority) = req.uri().authority()
    {
        return (
            format!(
                "http://{}{}",
                headers::host_port(authority),
                path_and_query(req.uri())
            ),
            Some(authority.host().to_ascii_lowercase()),
        );
    }
    let path = path_and_query(req.uri());
    if let Some(authority) = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<http::uri::Authority>().ok())
    {
        return (
            format!("http://{}{}", headers::host_port(&authority), path),
            Some(authority.host().to_ascii_lowercase()),
        );
    }
    (
        format!(
            "http://{}{}",
            origin_authority(fallback.0, fallback.1),
            path
        ),
        None,
    )
}

/// A target as a URL can carry it: the port unless it is 80, and an IPv6
/// address in brackets (`HostName`'s own display has none, `types.rs`).
fn origin_authority(host: &HostName, port: u16) -> String {
    let host = match host {
        HostName::Domain(d) => d.clone(),
        HostName::Ip(IpAddr::V6(v6)) => format!("[{v6}]"),
        HostName::Ip(IpAddr::V4(v4)) => v4.to_string(),
    };
    if port == 80 {
        host
    } else {
        format!("{host}:{port}")
    }
}

/// Turns the fake address a listener request names back into its name (`phase
/// 4 M1 design 9`, R27): the request line and, where it names the same
/// address, the `Host` header follow.
fn rename<B>(req: &mut Request<B>, authority: &http::uri::Authority, name: &str) {
    let port = authority.port_u16();
    let host_with_port = |port: Option<u16>| match port {
        Some(p) => format!("{name}:{p}"),
        None => name.to_string(),
    };
    let line = host_with_port(port);
    if let Ok(uri) = format!("http://{line}{}", path_and_query(req.uri())).parse::<Uri>() {
        *req.uri_mut() = uri;
    }
    let named = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<http::uri::Authority>().ok())
        .filter(|host| host.host().eq_ignore_ascii_case(authority.host()))
        .map(|host| host_with_port(host.port_u16()));
    if let Some(named) = named
        && let Ok(value) = HeaderValue::from_str(&named)
    {
        req.headers_mut().insert(header::HOST, value);
    }
}

/// The parts of an upstream connection the request that used it hands back
/// to the pool once the response body is over (`design 8.5`, 10.1).
struct Outing<K> {
    key: K,
    sender: SendRequest<ReqBody>,
    meter: Meter,
    peer: Option<SocketAddr>,
    forward: Option<Vec<(String, String)>>,
}

/// Rewrites a request into the form the origin expects: origin-form URI, a
/// `Host` header matching the request target (`RFC 9112 §3.2.2`).
fn origin_form<B>(req: &mut Request<B>) -> Result<(), http::Error> {
    let authority = req.uri().authority().map(headers::host_port);
    let path = path_and_query(req.uri()).to_string();
    *req.uri_mut() = path.parse::<Uri>()?;
    // the request target wins over whatever `Host` the client sent
    if let Some(a) = authority
        && let Ok(v) = HeaderValue::from_str(&a)
    {
        req.headers_mut().insert(header::HOST, v);
    }
    Ok(())
}

/// Keeps a request in absolute form for an upstream HTTP proxy
/// (`always-use-connect = false`): the URI is rebuilt without userinfo,
/// `Host` follows the request target, and `extra` — the upstream's
/// `Proxy-Authorization` and configured headers — replaces same-name headers,
/// `Host` included (manual: Policies › HTTP).
fn absolute_form<B>(req: &mut Request<B>, extra: &[(String, String)]) -> Result<(), http::Error> {
    let authority = req
        .uri()
        .authority()
        .map(headers::host_port)
        .unwrap_or_default();
    let path = path_and_query(req.uri()).to_string();
    *req.uri_mut() = format!("http://{authority}{path}").parse::<Uri>()?;
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
            tracing::debug!(name = ?name, "dropped an upstream proxy header");
            continue;
        };
        req.headers_mut().insert(name, value);
    }
    Ok(())
}

/// A body that went `Conn::idle` without a frame (`phase 4 M1 design 8.10`).
/// hyper takes an error out of a body it writes as a user error, so a client's
/// upload that stalls is no failure of upstream's.
#[derive(Debug)]
struct Stalled(&'static str);

impl std::fmt::Display for Stalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Stalled {}

/// How far a body got, for the two idle timeouts of a request (`phase 4 M1
/// design 8.10`): when it last handed a frame on, and whether its last poll
/// found its source empty. hyper polls a body again whenever it can take more
/// (`proto/h1/dispatch.rs`, `poll_write`), so a body whose last poll handed a
/// frame on waits for its reader, and one whose last poll found nothing waits
/// for its source.
struct Progress {
    /// The clock every other field counts from. It never moves, so that a
    /// request sent again on a new connection can start its count over
    /// without a lock (`restart`).
    epoch: tokio::time::Instant,
    /// Milliseconds since `epoch` at which this attempt started. Zero for the
    /// first one.
    started: AtomicU64,
    /// Milliseconds since `started` of the last frame handed on, or of the
    /// end; zero until the first frame moves it.
    last: AtomicU64,
    /// The last poll found the source empty.
    waiting_on_source: AtomicBool,
}

impl Progress {
    fn new() -> Arc<Progress> {
        Arc::new(Progress {
            epoch: tokio::time::Instant::now(),
            started: AtomicU64::new(0),
            last: AtomicU64::new(0),
            waiting_on_source: AtomicBool::new(false),
        })
    }

    /// How long this attempt has been going.
    fn since_started(&self) -> Duration {
        self.epoch
            .elapsed()
            .saturating_sub(Duration::from_millis(self.started.load(Ordering::Relaxed)))
    }

    /// A request sent again on a new connection starts its wait for the
    /// response head over: the dial of the new connection is no upload and no
    /// late answer (`phase 4 M1 design 8.6`).
    fn restart(&self) {
        self.started
            .store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
        self.last.store(0, Ordering::Relaxed);
        self.waiting_on_source.store(false, Ordering::Relaxed);
    }

    /// A frame was handed on, or the body reached its end.
    fn moved(&self) {
        self.last
            .store(self.since_started().as_millis() as u64, Ordering::Relaxed);
        self.waiting_on_source.store(false, Ordering::Relaxed);
    }

    /// The last poll found the source empty.
    fn waiting(&self) {
        self.waiting_on_source.store(true, Ordering::Relaxed);
    }

    fn is_waiting(&self) -> bool {
        self.waiting_on_source.load(Ordering::Relaxed)
    }

    /// How much of `idle` is left before the body counts as stopped.
    fn remaining(&self, idle: Duration) -> Duration {
        let last = Duration::from_millis(self.last.load(Ordering::Relaxed));
        (last + idle).saturating_sub(self.since_started())
    }
}

/// Waits for the response head with the request's body in mind (`phase 4 M1
/// design 8.10`): a head that does not come while the body is not moving is
/// late; one that does not come while the body waits for the client is not,
/// because `Idle` times that wait itself.
async fn head_deadline(progress: Arc<Progress>, idle: Duration) {
    loop {
        if progress.is_waiting() {
            // the body waits for the client: `Idle` is the one to time that
            tokio::time::sleep(idle).await;
            continue;
        }
        let remaining = progress.remaining(idle);
        if remaining.is_zero() {
            return;
        }
        tokio::time::sleep(remaining).await;
    }
}

/// The body of a request being sent upstream, with `Conn::idle` allowed
/// between two of its frames (`phase 4 M1 design 8.10`). `is_end_stream` and
/// `size_hint` are the inner body's: hyper's client decides by them whether
/// there is a body to frame, and a wrapper that hid an empty body would send
/// a `GET` chunked.
struct Idle<B> {
    inner: B,
    idle: Duration,
    /// Started by the first poll, not by `new`: a request sent again on a new
    /// connection must not count the dial of that connection as an upload
    /// that stopped (`phase 4 M1 design 8.6`).
    deadline: Option<Pin<Box<Sleep>>>,
    /// What the wait for the response head reads (`design 8.10`).
    progress: Arc<Progress>,
}

impl<B> Idle<B> {
    fn new(inner: B, idle: Duration, progress: Arc<Progress>) -> Idle<B> {
        Idle {
            inner,
            idle,
            deadline: None,
            progress,
        }
    }
}

impl<B> Body for Idle<B>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        if this.inner.is_end_stream() {
            return Poll::Ready(None);
        }
        let idle = this.idle;
        let deadline = this
            .deadline
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(idle)));
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                deadline
                    .as_mut()
                    .reset(tokio::time::Instant::now() + this.idle);
                this.progress.moved();
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e.into()))),
            Poll::Ready(None) => {
                this.progress.moved();
                Poll::Ready(None)
            }
            Poll::Pending => {
                this.progress.waiting();
                if deadline.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Some(Err(Box::new(Stalled("request body stalled")))));
                }
                Poll::Pending
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// The body of a response on its way to the client (`phase 4 M1 design 8.2,
/// 8.10`): it ends the request when the body is read, puts the upstream
/// connection back into the pool, and ends both when the body stalls or the
/// request is cancelled.
struct Tail<U: Upstreams> {
    inner: Incoming,
    idle: Duration,
    deadline: Pin<Box<Sleep>>,
    /// The request's token: `kill` and a stop end the exchange here, because
    /// ending one response on HTTP/1.1 means ending the client connection.
    cancelled: Pin<Box<tokio_util::sync::WaitForCancellationFutureOwned>>,
    /// How far the body got, for the watchdog that watches the client
    /// (`design 8.10`).
    progress: Arc<Progress>,
    /// Fired when the body is over (or the body is dropped): the watchdog
    /// stops caring.
    watchdog: CancellationToken,
    guard: Guard,
    shared: Arc<Shared<U>>,
    /// The upstream connection, until the body is over and it goes back.
    outing: Option<Box<Outing<U::Key>>>,
    done: bool,
}

impl<U: Upstreams> Tail<U> {
    fn new(
        inner: Incoming,
        guard: Guard,
        shared: Arc<Shared<U>>,
        outing: Outing<U::Key>,
    ) -> Tail<U> {
        let idle = shared.conn.idle;
        let cancelled = Box::pin(guard.handle().token().clone().cancelled_owned());
        let progress = Progress::new();
        let watchdog = CancellationToken::new();
        let mut tail = Tail {
            inner,
            idle,
            deadline: Box::pin(tokio::time::sleep(idle)),
            cancelled,
            progress,
            watchdog,
            guard,
            shared,
            outing: Some(Box::new(outing)),
            done: false,
        };
        // hyper drops a body whose `is_end_stream` is true without polling it
        // (`proto/h1/dispatch.rs`): a response that arrives with no body at
        // all is read to its end here, where it can still be ended properly
        if tail.inner.is_end_stream() {
            tail.complete();
        } else {
            tail.watch_the_client();
        }
        tail
    }

    /// Watches the client while this body is on its way to it (`design 8.10,
    /// 8.11`): a client that stopped reading never polls the body again, so
    /// `kill`, a stop and the idle timeout could not be seen from `poll_frame`.
    /// The watchdog ends them: the connection goes at once, and the guard says
    /// how the request ended.
    fn watch_the_client(&self) {
        let progress = self.progress.clone();
        let handle = self.guard.handle().clone();
        let request = handle.token().clone();
        let abort = self.shared.conn.abort.clone();
        let done = self.watchdog.clone();
        let idle = self.idle;
        let tracker = self.shared.conn.tracker.clone();
        tracker.spawn(async move {
            let inner = tokio::spawn(async move {
                loop {
                    // What to sleep for, and whether that sleep is a deadline
                    // at all: a body waiting for upstream is `Tail`'s to time,
                    // so the watchdog only sleeps and looks again.
                    let sleep_for = if progress.is_waiting() {
                        idle
                    } else {
                        let remaining = progress.remaining(idle);
                        if remaining.is_zero() {
                            handle.finish(SessionOutcome::Failed(
                                "client stopped reading".to_string(),
                            ));
                            tracing::debug!("a client stopped reading its response");
                            abort.cancel();
                            return;
                        }
                        remaining
                    };
                    tokio::select! {
                        biased;
                        _ = done.cancelled() => return,
                        _ = request.cancelled() => {
                            abort.cancel();
                            return;
                        }
                        _ = tokio::time::sleep(sleep_for) => {}
                    }
                }
            });
            if let Err(e) = inner.await
                && e.is_panic()
            {
                tracing::error!("response watchdog panicked: {e}");
            }
        });
    }

    /// The body was read to its end: the request is complete, and the
    /// upstream connection goes back to the pool unless it closed.
    fn complete(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        self.watchdog.cancel();
        self.guard.handle().finish(SessionOutcome::Completed);
        if let Some(outing) = self.outing.take() {
            let Outing {
                key,
                sender,
                meter,
                peer,
                forward,
            } = *outing;
            meter.set(None);
            if !sender.is_closed() {
                let pooled = Pooled {
                    sender,
                    meter,
                    peer,
                    forward,
                };
                self.shared
                    .pool
                    .lock()
                    .expect("pool")
                    .put(key, pooled, Instant::now());
            }
        }
    }

    /// The body failed or stalled: the request fails, and the upstream
    /// connection is not lent again (dropping the sender closes it).
    fn fail(&mut self, message: String) {
        if self.done {
            return;
        }
        self.done = true;
        self.watchdog.cancel();
        self.outing.take();
        self.guard.handle().finish(SessionOutcome::Failed(message));
    }

    /// The request's token fired: the guard says how it ends.
    fn stop(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        self.watchdog.cancel();
        self.outing.take();
        self.guard.retire();
    }
}

impl<U: Upstreams> Drop for Tail<U> {
    /// The body went away without a word (hyper dropped it): there is nothing
    /// left for the watchdog to watch.
    fn drop(&mut self) {
        self.watchdog.cancel();
    }
}

impl<U: Upstreams> Body for Tail<U> {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        if this.inner.is_end_stream() {
            this.complete();
            return Poll::Ready(None);
        }
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                this.deadline
                    .as_mut()
                    .reset(tokio::time::Instant::now() + this.idle);
                this.progress.moved();
                // the frame just handed out was the last one: hyper will drop
                // the body now instead of polling it again
                if this.inner.is_end_stream() {
                    this.complete();
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(e))) => {
                this.fail(format!("upstream body failed: {e}"));
                Poll::Ready(Some(Err(Box::new(e))))
            }
            Poll::Ready(None) => {
                this.progress.moved();
                this.complete();
                Poll::Ready(None)
            }
            Poll::Pending => {
                this.progress.waiting();
                if this.cancelled.as_mut().poll(cx).is_ready() {
                    this.stop();
                    return Poll::Ready(Some(Err(Box::new(Close))));
                }
                if this.deadline.as_mut().poll(cx).is_ready() {
                    this.fail("upstream body stalled".to_string());
                    return Poll::Ready(Some(Err(Box::new(Stalled("upstream body stalled")))));
                }
                Poll::Pending
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
/// The guard of one request (`phase 4 M1 design 8.11`, P18): while the future
/// lives it holds the handle; dropped first, it ends the handle the way the
/// request ended — killed, stopped with the connection, or left by the client.
struct Guard {
    handle: Arc<SessionHandle>,
    /// The connection's token: a request ended by it was not left behind.
    stop: CancellationToken,
}

impl Guard {
    fn new(handle: Arc<SessionHandle>, stop: CancellationToken) -> Guard {
        Guard { handle, stop }
    }

    fn handle(&self) -> &Arc<SessionHandle> {
        &self.handle
    }

    /// How a request nobody finished ended (`P18`; `cancelled_outcome` before
    /// it).
    fn outcome(&self) -> SessionOutcome {
        if self.handle.was_killed() {
            SessionOutcome::Failed("killed".to_string())
        } else if self.stop.is_cancelled() {
            SessionOutcome::Completed
        } else {
            SessionOutcome::Failed("client closed the connection".to_string())
        }
    }

    /// Ends the handle the way the guard would, and from now on the guard
    /// leaves it alone.
    fn retire(&self) {
        self.handle.finish(self.outcome());
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.handle.is_finished() {
            self.handle.finish(self.outcome());
        }
    }
}

/// How a served connection ended (`phase 4 M1 design 8.1, 10.2`).
pub struct Served {
    /// How many requests reached `route`.
    pub routed: u64,
    /// hyper's own error for the connection, if it had one: a request head
    /// that could not be read, a request head that timed out, a client that
    /// half-sent one and left.
    pub error: Option<String>,
}

/// Serves `client` until it ends or `requests`' connection token fires, and
/// then until the connection has let go (`phase 4 M1 design 8.1, 8.11`).
pub async fn serve_connection<U: Upstreams>(client: BoxedStream, requests: Requests<U>) -> Served {
    let head_wait = requests.shared.conn.head_wait;
    let token = requests.token().clone();
    let abort = requests.shared.conn.abort.clone();
    let counter = requests.clone();
    let service = service_fn(move |req: Request<Incoming>| {
        let requests = requests.clone();
        async move { requests.handle(req).await }
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .preserve_header_case(true)
        // hyper only enforces `header_read_timeout` when a timer is installed.
        .timer(TokioTimer::new())
        .header_read_timeout(head_wait)
        .keep_alive(true)
        .serve_connection(TokioIo::new(client), service)
        .with_upgrades();
    let mut conn = std::pin::pin!(conn);
    // `abort` is the engine's word that the connection has to go at once (a
    // killed request, a client that stopped reading): dropping the connection
    // is what ends it then, and no error is recorded for it.
    let error = tokio::select! {
        biased;
        _ = abort.cancelled() => None,
        res = conn.as_mut() => res.err().map(|e| e.to_string()),
        _ = token.cancelled() => {
            conn.as_mut().graceful_shutdown();
            tokio::select! {
                biased;
                _ = abort.cancelled() => None,
                res = conn.as_mut() => res.err().map(|e| e.to_string()),
            }
        }
    };
    Served {
        routed: counter.routed(),
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::testing::TestServer;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    /// What the fake engine answers for one path of a request's URL.
    #[derive(Clone)]
    enum Step {
        /// A route and the key it and the request's pool entry share.
        Go(u32),
        /// Unfinished, as `route` hands refusals back.
        Refused { refusal: Refusal, pages: Pages },
    }

    /// What `route` was given for one request.
    #[derive(Clone)]
    struct Seen {
        forwardable: bool,
        session: SessionInfo,
    }

    struct Inner {
        token: CancellationToken,
        target: SocketAddr,
        script: Mutex<HashMap<String, Step>>,
        dial_refusals: Mutex<HashMap<u32, Refusal>>,
        /// Addresses of rurge's own: `Some(name)` when one is behind it.
        names: Mutex<HashMap<String, Option<String>>>,
        next_id: AtomicU64,
        dials: AtomicUsize,
        handles: Mutex<Vec<Arc<SessionHandle>>>,
        seen: Mutex<Vec<Seen>>,
        routed: AtomicUsize,
        relayed: AtomicUsize,
    }

    /// The engine side of `Upstreams`, scripted by the path of each request’s
    /// URL: its dials all go to one loopback origin.
    #[derive(Clone)]
    struct Fake(Arc<Inner>);

    impl Fake {
        fn new(token: CancellationToken, target: SocketAddr) -> Fake {
            Fake(Arc::new(Inner {
                token,
                target,
                script: Mutex::new(HashMap::new()),
                dial_refusals: Mutex::new(HashMap::new()),
                names: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
                dials: AtomicUsize::new(0),
                handles: Mutex::new(Vec::new()),
                seen: Mutex::new(Vec::new()),
                routed: AtomicUsize::new(0),
                relayed: AtomicUsize::new(0),
            }))
        }

        /// Answers `path` with `step`; every other path goes the way of
        /// `Step::Go(1)`.
        fn script(&self, path: &str, step: Step) -> &Fake {
            self.0.script.lock().unwrap().insert(path.to_string(), step);
            self
        }

        /// What the name behind an address of rurge's own is: `None` for no
        /// name at all.
        fn name(&self, address: &str, name: Option<&str>) {
            self.0
                .names
                .lock()
                .unwrap()
                .insert(address.to_string(), name.map(str::to_string));
        }

        fn dials(&self) -> usize {
            self.0.dials.load(Ordering::Relaxed)
        }

        fn seen(&self) -> Vec<Seen> {
            self.0.seen.lock().unwrap().clone()
        }

        /// The `n`th handle, waiting for it to be made.
        async fn handle(&self, n: usize) -> Arc<SessionHandle> {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(h) = self.0.handles.lock().unwrap().get(n) {
                        return h.clone();
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the handle was made within 5 s")
        }

        /// The `n`th handle, waiting for it to finish.
        async fn finished(&self, n: usize) -> Arc<SessionHandle> {
            let handle = self.handle(n).await;
            tokio::time::timeout(Duration::from_secs(5), async {
                while !handle.is_finished() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the handle finished within 5 s");
            handle
        }
    }

    impl Upstreams for Fake {
        type Key = u32;
        type Route = u32;
        type Lease = ();

        fn named(&self, host: &HostName) -> Named<()> {
            match self.0.names.lock().unwrap().get(&host.to_string()) {
                Some(Some(name)) => Named::Name(name.clone(), ()),
                Some(None) => Named::Unknown,
                None => Named::No,
            }
        }

        fn open(&self, session: SessionInfo) -> Arc<SessionHandle> {
            let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
            let handle = SessionHandle::new_with_token(id, session, self.0.token.child_token());
            self.0.handles.lock().unwrap().push(handle.clone());
            handle
        }

        fn route<'a>(
            &'a self,
            handle: &'a Arc<SessionHandle>,
            forwardable: bool,
        ) -> BoxFuture<'a, Routed<u32, u32>> {
            self.0.seen.lock().unwrap().push(Seen {
                forwardable,
                session: handle.session().clone(),
            });
            self.0.routed.fetch_add(1, Ordering::Relaxed);
            let path = path_of(handle.session().url.as_deref().unwrap_or("/"));
            let step = self
                .0
                .script
                .lock()
                .unwrap()
                .get(&path)
                .cloned()
                .unwrap_or(Step::Go(1));
            Box::pin(async move {
                let pages = Pages {
                    error: true,
                    reject: true,
                };
                match step {
                    Step::Go(key) => Routed::Go {
                        route: key,
                        key,
                        pages,
                    },
                    Step::Refused { refusal, pages } => Routed::Refused { refusal, pages },
                }
            })
        }

        fn dial<'a>(
            &'a self,
            handle: &'a Arc<SessionHandle>,
            route: u32,
        ) -> BoxFuture<'a, Result<Upstream, Refusal>> {
            Box::pin(async move {
                if let Some(refusal) = self.0.dial_refusals.lock().unwrap().get(&route).cloned() {
                    return Err(refusal);
                }
                self.0.dials.fetch_add(1, Ordering::Relaxed);
                let stream = tokio::net::TcpStream::connect(self.0.target)
                    .await
                    .map_err(|e| Refusal::Fail {
                        kind: FailKind::Connect,
                        message: e.to_string(),
                    })?;
                let peer = stream.peer_addr().ok();
                handle.mark_connected();
                Ok(Upstream {
                    stream: Box::new(stream),
                    forward: None,
                    peer,
                })
            })
        }

        fn relay<'a>(
            &'a self,
            client: BoxedStream,
            upstream: BoxedStream,
            handle: Arc<SessionHandle>,
        ) -> BoxFuture<'a, ()> {
            self.0.relayed.fetch_add(1, Ordering::Relaxed);
            Box::pin(async move {
                let mut client = client;
                let mut upstream = upstream;
                if let Ok((up, down)) =
                    tokio::io::copy_bidirectional(&mut client, &mut upstream).await
                {
                    // the relay counts what it moves: client to upstream is
                    // up, upstream to client is down (`design 8.9`, 10.1)
                    handle.add_up(up);
                    handle.add_down(down);
                }
                handle.finish(SessionOutcome::Completed);
            })
        }
    }

    /// The path of a URL, for picking the scripted step.
    fn path_of(url: &str) -> String {
        let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
        match rest.find('/') {
            Some(at) => rest[at..].to_string(),
            None => "/".to_string(),
        }
    }

    /// The client's end of the connection the engine serves, and a reader
    /// that keeps what it has read of it.
    struct Client {
        stream: DuplexStream,
        buf: Vec<u8>,
    }

    impl Client {
        async fn send(&mut self, bytes: &str) -> std::io::Result<()> {
            self.stream.write_all(bytes.as_bytes()).await
        }

        /// Reads more into `buf`; false at EOF.
        async fn fill(&mut self) -> bool {
            let mut chunk = [0u8; 4096];
            let n = tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut chunk))
                .await
                .expect("a read within 5 s")
                .expect("a read");
            if n == 0 {
                return false;
            }
            self.buf.extend_from_slice(&chunk[..n]);
            true
        }

        /// One response: its head and its body, by `Content-Length`.
        async fn response(&mut self) -> (String, Vec<u8>) {
            let at = loop {
                if let Some(at) = find(&self.buf, b"\r\n\r\n") {
                    break at;
                }
                assert!(self.fill().await, "the connection closed before a head");
            };
            let head = String::from_utf8_lossy(&self.buf[..at]).to_string();
            let len = head
                .lines()
                .find_map(|l| {
                    l.split_once(':')
                        .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while self.buf.len() < at + 4 + len {
                assert!(self.fill().await, "the connection closed mid-body");
            }
            let body = self.buf[at + 4..at + 4 + len].to_vec();
            self.buf.drain(..at + 4 + len);
            (head, body)
        }

        /// One response head and nothing else: a HEAD's answer names a body
        /// it does not send.
        async fn response_head(&mut self) -> String {
            let at = loop {
                if let Some(at) = find(&self.buf, b"\r\n\r\n") {
                    break at;
                }
                assert!(self.fill().await, "the connection closed before a head");
            };
            let head = String::from_utf8_lossy(&self.buf[..at]).to_string();
            self.buf.drain(..at + 4);
            head
        }

        /// Reads exactly `n` bytes, however many reads it takes.
        async fn take(&mut self, n: usize) -> Vec<u8> {
            while self.buf.len() < n {
                assert!(self.fill().await, "the connection closed early");
            }
            self.buf.drain(..n).collect()
        }

        /// Everything the engine says until it closes the connection.
        async fn until_eof(&mut self) -> String {
            while self.fill().await {}
            String::from_utf8_lossy(&self.buf).to_string()
        }
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    /// A client connection being served, the engine side of it, and the task
    /// that serves it.
    struct Rig {
        client: Client,
        fake: Fake,
        token: CancellationToken,
        served: tokio::task::JoinHandle<Served>,
    }

    impl Rig {
        fn stop(&self) {
            self.token.cancel();
        }

        async fn served(self) -> Served {
            tokio::time::timeout(Duration::from_secs(5), self.served)
                .await
                .expect("the connection ended within 5 s")
                .expect("the serving task did not panic")
        }
    }

    /// A connection the engine serves over a duplex stream, with an origin to
    /// dial.
    async fn connected(entry: Entry, target: SocketAddr) -> Rig {
        rig_timed(
            entry,
            target,
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .await
    }

    async fn rig_timed(
        entry: Entry,
        target: SocketAddr,
        idle: Duration,
        drop_hold: Duration,
    ) -> Rig {
        let token = CancellationToken::new();
        let fake = Fake::new(token.clone(), target);
        let session = SessionInfo::tcp(HostName::parse("target.test"), 80);
        let conn = Conn {
            session,
            entry,
            idle,
            head_wait: Duration::from_secs(5),
            drop_hold,
            abort: CancellationToken::new(),
            token: token.clone(),
            tracker: TaskTracker::new(),
        };
        let requests = Requests::new(conn, fake.clone());
        let (server_end, client_end) = tokio::io::duplex(64 * 1024);
        let served = tokio::spawn(serve_connection(Box::new(server_end), requests));
        Rig {
            client: Client {
                stream: client_end,
                buf: Vec::new(),
            },
            fake,
            token,
            served,
        }
    }

    fn addr_of(server: &TestServer) -> SocketAddr {
        format!("127.0.0.1:{}", server.url("/").port().unwrap())
            .parse()
            .unwrap()
    }

    #[tokio::test]
    async fn a_request_goes_through_and_its_handle_finishes() {
        let origin = TestServer::spawn().await;
        origin.set("/hello", "hi there");
        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        rig.client
            .send("GET /hello HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body, b"hi there");
        let handle = rig.fake.finished(0).await;
        assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
        assert_eq!(handle.method().as_deref(), Some("GET"));
        assert_eq!(handle.status(), Some(200));
        let (up, down) = handle.bytes();
        assert!(up > 0 && down > 0, "up {up}, down {down}");
        assert!(handle.connect_time().is_some());
        assert!(handle.first_byte_time().is_some());
        assert_eq!(origin.hits("/hello"), 1);
    }

    #[tokio::test]
    async fn two_requests_on_one_connection_share_an_upstream_connection() {
        let origin = TestServer::spawn().await;
        origin.set("/one", "first");
        origin.set("/two", "second");
        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        for path in ["/one", "/two"] {
            rig.client
                .send(&format!("GET {path} HTTP/1.1\r\nHost: target.test\r\n\r\n"))
                .await
                .unwrap();
            let (head, _) = rig.client.response().await;
            assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        }
        assert_eq!(rig.fake.dials(), 1, "the second request borrows");
        assert_eq!(origin.connections(), 1);
        let second = rig.fake.finished(1).await;
        assert!(second.connect_time().is_some());
        assert_eq!(second.trail().peer(), Some(addr_of(&origin)));
    }

    #[tokio::test]
    async fn a_connection_is_lent_to_its_own_key_only() {
        let origin = TestServer::spawn().await;
        origin.set("/a", "one");
        origin.set("/b", "two");
        let rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        let fake = rig.fake.clone();
        fake.script("/a", Step::Go(7)).script("/b", Step::Go(8));
        let mut rig = rig;
        for path in ["/a", "/b"] {
            rig.client
                .send(&format!("GET {path} HTTP/1.1\r\nHost: target.test\r\n\r\n"))
                .await
                .unwrap();
            let (head, _) = rig.client.response().await;
            assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        }
        assert_eq!(fake.dials(), 2, "another key is another connection");
        assert_eq!(origin.connections(), 2);
    }

    #[tokio::test]
    async fn the_session_of_a_request_says_what_the_rules_see() {
        let origin = TestServer::spawn().await;
        origin.set("/a/b", "one");
        origin.set("/x", "two");
        origin.set("/p", "three");

        // sniffed, origin form: the target is the connection's
        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        rig.client
            .send("GET /a/b?q=1 HTTP/1.1\r\nHost: Target.Test:8080\r\nUser-Agent: t/1\r\n\r\n")
            .await
            .unwrap();
        let (head, _) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let seen = rig.fake.seen();
        let s = &seen[0];
        assert!(!s.forwardable, "a sniffed request is never forwarded on");
        // the URL is rebuilt from the `Host` header as the client wrote it;
        // the host the rules read (`http_host`) is the lowercase one
        assert_eq!(
            s.session.url.as_deref(),
            Some("http://Target.Test:8080/a/b?q=1")
        );
        assert_eq!(s.session.http_host.as_deref(), Some("target.test"));
        assert_eq!(s.session.dst_host, HostName::parse("target.test"));
        assert_eq!(s.session.dst_port, 80, "the connection's target");
        assert_eq!(s.session.protocol, Some(ProtocolKind::Http));
        assert_eq!(s.session.user_agent.as_deref(), Some("t/1"));

        // sniffed, absolute form: the URL is the request line's, the host the
        // request line's too (P22), the target still the connection's
        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        rig.client
            .send("GET http://other.test:8081/x?y=2 HTTP/1.1\r\nHost: lying.test\r\n\r\n")
            .await
            .unwrap();
        let (head, _) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let seen = rig.fake.seen();
        let s = &seen[0];
        assert!(!s.forwardable, "a client's own proxy target is not ours");
        assert_eq!(
            s.session.url.as_deref(),
            Some("http://other.test:8081/x?y=2")
        );
        assert_eq!(s.session.http_host.as_deref(), Some("other.test"));
        assert_eq!(s.session.dst_host, HostName::parse("target.test"));
        // a request line and a `Host` the client wrote for its own proxy go on
        // exactly as they came
        let requests = origin.requests();
        let last = &requests[requests.len() - 1];
        assert_eq!(last.authority.as_deref(), Some("other.test:8081"));
        assert_eq!(last.header("host"), Some("lying.test"));

        // the listener's absolute form: the target is the URL's, the port 80
        // when the URL names none, without the userinfo
        let mut rig = connected(Entry::Listener, addr_of(&origin)).await;
        rig.client
            .send("GET http://user:pw@site.test/p?q=3 HTTP/1.1\r\nHost: site.test\r\n\r\n")
            .await
            .unwrap();
        let (head, _) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let seen = rig.fake.seen();
        let s = &seen[0];
        assert!(
            s.forwardable,
            "the listener's plain request may be forwarded"
        );
        assert_eq!(s.session.url.as_deref(), Some("http://site.test/p?q=3"));
        assert_eq!(s.session.http_host.as_deref(), Some("site.test"));
        assert_eq!(s.session.dst_host, HostName::parse("site.test"));
        assert_eq!(s.session.dst_port, 80);
        // to an origin the request goes in origin form, its `Host` the target
        let requests = origin.requests();
        let last = &requests[requests.len() - 1];
        assert_eq!(last.path, "/p");
        assert_eq!(last.authority, None);
        assert_eq!(last.header("host"), Some("site.test"));
    }

    #[tokio::test]
    async fn a_fake_address_is_turned_back_into_its_name() {
        let origin = TestServer::spawn().await;
        origin.set("/where", "here");
        let mut rig = connected(Entry::Listener, addr_of(&origin)).await;
        rig.fake.name("10.1.2.3", Some("site.test"));
        rig.client
            .send("GET http://10.1.2.3/where HTTP/1.1\r\nHost: 10.1.2.3\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body, b"here");
        let seen = rig.fake.seen();
        let s = &seen[0];
        assert_eq!(s.session.dst_host, HostName::parse("site.test"));
        assert_eq!(s.session.url.as_deref(), Some("http://site.test/where"));
        assert_eq!(s.session.http_host.as_deref(), Some("site.test"));
        let requests = origin.requests();
        assert_eq!(requests[0].header("host"), Some("site.test"));

        // an address with no name behind it: a failure, and nothing dialed
        let mut rig = connected(Entry::Listener, addr_of(&origin)).await;
        rig.fake.name("10.9.9.9", None);
        rig.client
            .send("GET http://10.9.9.9/x HTTP/1.1\r\nHost: 10.9.9.9\r\n\r\n")
            .await
            .unwrap();
        let (head, _) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        let handle = rig.fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Failed("fake address not known".into()))
        );
        assert_eq!(rig.fake.dials(), 0);
    }

    #[tokio::test]
    async fn hop_by_hop_headers_are_dropped_both_ways() {
        let origin = TestServer::spawn().await;
        origin.set("/hop", "there");
        origin.set_header("/hop", "connection", "X-Down");
        origin.set_header("/hop", "x-down", "leaked");
        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        rig.client
            .send(
                "GET /hop HTTP/1.1\r\nHost: target.test\r\n\
                 Connection: keep-alive, X-Hop\r\nX-Hop: leaked\r\n\
                 Keep-Alive: timeout=5\r\nProxy-Connection: keep-alive\r\n\
                 TE: trailers\r\n\r\n",
            )
            .await
            .unwrap();
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body, b"there");
        let requests = origin.requests();
        for gone in [
            "connection",
            "x-hop",
            "keep-alive",
            "proxy-connection",
            "te",
        ] {
            assert!(requests[0].header(gone).is_none(), "{gone} was forwarded");
        }
        let lower = head.to_ascii_lowercase();
        assert!(
            !lower.contains("\r\nconnection:") && !lower.contains("x-down"),
            "a hop-by-hop response header came back: {head}"
        );
    }

    #[tokio::test]
    async fn proxy_authorization_is_forwarded_only_on_a_sniffed_connection() {
        let origin = TestServer::spawn().await;
        origin.set("/up", "there");
        for (entry, forwarded) in [(Entry::Sniffed, true), (Entry::Listener, false)] {
            let mut rig = connected(entry, addr_of(&origin)).await;
            let line = match entry {
                Entry::Sniffed => "/up".to_string(),
                Entry::Listener => format!("http://target.test:{}/up", addr_of(&origin).port()),
            };
            rig.client
                .send(&format!(
                    "GET {line} HTTP/1.1\r\nHost: target.test\r\n\
                     Proxy-Authorization: Basic dXA=\r\n\r\n"
                ))
                .await
                .unwrap();
            let (head, _) = rig.client.response().await;
            assert!(head.starts_with("HTTP/1.1 200"), "{head}");
            let requests = origin.requests();
            let seen = &requests[requests.len() - 1];
            assert_eq!(
                seen.header("proxy-authorization"),
                forwarded.then_some("Basic dXA="),
                "{entry:?}"
            );
        }
    }

    #[tokio::test]
    async fn refusals_are_answered_as_the_pages_say() {
        let origin = TestServer::spawn().await;
        let pages = Pages {
            error: true,
            reject: true,
        };

        // REJECT with the page on
        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        rig.fake.script(
            "/reject",
            Step::Refused {
                refusal: Refusal::Reject(RejectKind::Reject),
                pages,
            },
        );
        rig.client
            .send("GET /reject HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 403"), "{head}");
        let page = String::from_utf8_lossy(&body);
        assert!(
            page.contains("Request rejected")
                && page.contains("The request was rejected by the REJECT policy."),
            "{page}"
        );
        let handle = rig.fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Rejected(RejectKind::Reject))
        );
        assert_eq!(handle.status(), Some(403));

        // REJECT with the page off: no answer, the connection closes
        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        rig.fake.script(
            "/reject",
            Step::Refused {
                refusal: Refusal::Reject(RejectKind::NoDrop),
                pages: Pages {
                    error: true,
                    reject: false,
                },
            },
        );
        rig.client
            .send("GET /reject HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(rig.client.until_eof().await, "");
        let handle = rig.fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Rejected(RejectKind::NoDrop))
        );
        assert_eq!(handle.status(), None, "no answer, no status");

        // TINYGIF is one pixel, answered by rurge itself
        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        rig.fake.script(
            "/gif",
            Step::Refused {
                refusal: Refusal::Reject(RejectKind::TinyGif),
                pages,
            },
        );
        rig.client
            .send("GET /gif HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body.len(), 43);
        let handle = rig.fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Rejected(RejectKind::TinyGif))
        );
        assert_eq!(handle.status(), Some(200));

        // a failure with the page on, and with it off
        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        rig.fake.script(
            "/fail",
            Step::Refused {
                refusal: Refusal::Fail {
                    kind: FailKind::Dns,
                    message: "dns.test:80".to_string(),
                },
                pages,
            },
        );
        rig.client
            .send("GET /fail HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        let page = String::from_utf8_lossy(&body);
        assert!(
            page.contains("Connection failed") && page.contains("DNS lookup failed: dns.test:80"),
            "{page}"
        );
        let handle = rig.fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Failed("dns.test:80".into()))
        );
        assert_eq!(handle.status(), Some(502));

        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        rig.fake.script(
            "/fail",
            Step::Refused {
                refusal: Refusal::Fail {
                    kind: FailKind::Connect,
                    message: "refused".to_string(),
                },
                pages: Pages {
                    error: false,
                    reject: true,
                },
            },
        );
        rig.client
            .send("GET /fail HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(rig.client.until_eof().await, "");
        let handle = rig.fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Failed("refused".into()))
        );
        assert_eq!(handle.status(), None);
    }

    #[tokio::test]
    async fn a_dropped_request_is_held_then_closed() {
        let origin = TestServer::spawn().await;
        let mut rig = rig_timed(
            Entry::Sniffed,
            addr_of(&origin),
            Duration::from_secs(5),
            Duration::from_millis(300),
        )
        .await;
        rig.fake.script(
            "/drop",
            Step::Refused {
                refusal: Refusal::Reject(RejectKind::Drop),
                pages: Pages {
                    error: true,
                    reject: true,
                },
            },
        );
        let started = std::time::Instant::now();
        rig.client
            .send("GET /drop HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        // the handle is ended before the hold begins
        let handle = rig.fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Rejected(RejectKind::Drop))
        );
        assert!(started.elapsed() < Duration::from_millis(300));
        assert_eq!(rig.client.until_eof().await, "");
        let held = started.elapsed();
        assert!(
            held >= Duration::from_millis(300) && held < Duration::from_secs(5),
            "held {held:?}"
        );
    }

    /// One request as a loopback origin read it.
    #[derive(Clone)]
    struct Recorded {
        head: String,
        body: Vec<u8>,
    }

    /// Reads until at least `len` bytes are in `buf`; false when the
    /// connection ended first.
    async fn fill_to(buf: &mut Vec<u8>, stream: &mut tokio::net::TcpStream, len: usize) -> bool {
        while buf.len() < len {
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        true
    }

    /// Reads a `Transfer-Encoding: chunked` body out of `buf` and `stream`,
    /// unwrapped; `None` when the connection ended before its end.
    async fn read_chunked(
        buf: &mut Vec<u8>,
        stream: &mut tokio::net::TcpStream,
    ) -> Option<Vec<u8>> {
        let mut body = Vec::new();
        loop {
            let at = loop {
                if let Some(at) = find(buf, b"\r\n") {
                    break at;
                }
                let mut chunk = [0u8; 4096];
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => return None,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            };
            let line = String::from_utf8_lossy(&buf[..at]).to_string();
            buf.drain(..at + 2);
            let size =
                usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).ok()?;
            if !fill_to(buf, stream, size + 2).await {
                return None;
            }
            body.extend_from_slice(&buf[..size]);
            buf.drain(..size + 2);
            if size == 0 {
                return Some(body);
            }
        }
    }

    /// An origin that reads every request whole, records it and answers `ok`,
    /// on as many connections and requests as it is given.
    async fn recording_origin() -> (SocketAddr, Arc<Mutex<Vec<Recorded>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let records: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
        let rec = records.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let rec = rec.clone();
                tokio::spawn(async move {
                    let mut buf: Vec<u8> = Vec::new();
                    loop {
                        let at = loop {
                            if let Some(at) = find(&buf, b"\r\n\r\n") {
                                break at;
                            }
                            let mut chunk = [0u8; 4096];
                            match stream.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        };
                        let head = String::from_utf8_lossy(&buf[..at]).to_string();
                        buf.drain(..at + 4);
                        let body = if head
                            .to_ascii_lowercase()
                            .contains("transfer-encoding: chunked")
                        {
                            let Some(body) = read_chunked(&mut buf, &mut stream).await else {
                                return;
                            };
                            body
                        } else {
                            let len = content_length(&head);
                            if !fill_to(&mut buf, &mut stream, len).await {
                                return;
                            }
                            buf.drain(..len).collect()
                        };
                        rec.lock().unwrap().push(Recorded { head, body });
                        if stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                });
            }
        });
        (addr, records)
    }

    /// An origin that reads each request and then says `answer` and stops
    /// talking, holding the connection open; it counts the connections it
    /// accepted and those whose peer has ended them.
    struct Holding {
        addr: SocketAddr,
        accepted: Arc<AtomicUsize>,
        closed: Arc<AtomicUsize>,
    }

    /// How large the answer of `streaming_origin` is: far more than the
    /// rig's duplex (64 KiB) and hyper's write buffer, so whoever serves it
    /// stops reading it once the client stops taking bytes.
    const BIG_BODY: usize = 4 << 20;

    /// An origin that answers 200 with `BIG_BODY` bytes and writes them until
    /// the client stops taking them. It counts the connections it accepted
    /// and those whose peer ended them.
    async fn streaming_origin() -> Holding {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let (a, c) = (accepted.clone(), closed.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                a.fetch_add(1, Ordering::Relaxed);
                let c = c.clone();
                tokio::spawn(async move {
                    let mut buf: Vec<u8> = Vec::new();
                    loop {
                        if find(&buf, b"\r\n\r\n").is_some() {
                            break;
                        }
                        let mut chunk = [0u8; 4096];
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => {
                                c.fetch_add(1, Ordering::Relaxed);
                                return;
                            }
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {BIG_BODY}\r\n\r\n");
                    if stream.write_all(head.as_bytes()).await.is_err() {
                        c.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    let chunk = vec![b'x'; 16 * 1024];
                    let mut sent = 0;
                    while sent < BIG_BODY {
                        if stream.write_all(&chunk).await.is_err() {
                            c.fetch_add(1, Ordering::Relaxed);
                            return;
                        }
                        sent += chunk.len();
                    }
                    let mut chunk = [0u8; 1024];
                    while matches!(stream.read(&mut chunk).await, Ok(n) if n > 0) {}
                    c.fetch_add(1, Ordering::Relaxed);
                });
            }
        });
        Holding {
            addr,
            accepted,
            closed,
        }
    }

    /// Waits (5 s at most) until `done` holds.
    async fn wait_until(mut done: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !done() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the condition held within 5 s");
    }

    async fn holding_origin(answer: &'static [u8]) -> Holding {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let (a, c) = (accepted.clone(), closed.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                a.fetch_add(1, Ordering::Relaxed);
                let c = c.clone();
                tokio::spawn(async move {
                    let mut buf: Vec<u8> = Vec::new();
                    loop {
                        if find(&buf, b"\r\n\r\n").is_some() {
                            break;
                        }
                        let mut chunk = [0u8; 4096];
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => {
                                c.fetch_add(1, Ordering::Relaxed);
                                return;
                            }
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    if !answer.is_empty() && stream.write_all(answer).await.is_err() {
                        c.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    let mut chunk = [0u8; 1024];
                    while matches!(stream.read(&mut chunk).await, Ok(n) if n > 0) {}
                    c.fetch_add(1, Ordering::Relaxed);
                });
            }
        });
        Holding {
            addr,
            accepted,
            closed,
        }
    }

    /// Reads until the end of a request head, returning where it ends; `None`
    /// when the connection ended first.
    async fn read_head(buf: &mut Vec<u8>, stream: &mut tokio::net::TcpStream) -> Option<usize> {
        loop {
            if let Some(at) = find(buf, b"\r\n\r\n") {
                return Some(at);
            }
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
    }

    /// An origin that reads one request head, writes `answer` in one go, and
    /// then echoes whatever it reads; it counts every byte it read and every
    /// byte it wrote, and keeps the head of every request it was sent
    /// (`design 8.9`).
    async fn upgrading_origin(
        answer: &'static [u8],
    ) -> (
        SocketAddr,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        Arc<Mutex<Vec<String>>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let read = Arc::new(AtomicUsize::new(0));
        let written = Arc::new(AtomicUsize::new(0));
        let heads: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (r, w, h) = (read.clone(), written.clone(), heads.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let (r, w, h) = (r.clone(), w.clone(), h.clone());
                tokio::spawn(async move {
                    let mut buf: Vec<u8> = Vec::new();
                    let Some(at) = read_head(&mut buf, &mut stream).await else {
                        return;
                    };
                    h.lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&buf[..at]).to_string());
                    r.fetch_add(at + 4, Ordering::Relaxed);
                    buf.drain(..at + 4);
                    if stream.write_all(answer).await.is_err() {
                        return;
                    }
                    w.fetch_add(answer.len(), Ordering::Relaxed);
                    // anything that came with the request head belongs to the
                    // exchange that follows it
                    if !buf.is_empty() {
                        r.fetch_add(buf.len(), Ordering::Relaxed);
                        if stream.write_all(&buf).await.is_err() {
                            return;
                        }
                        w.fetch_add(buf.len(), Ordering::Relaxed);
                        buf.clear();
                    }
                    let mut chunk = [0u8; 4096];
                    loop {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => {
                                r.fetch_add(n, Ordering::Relaxed);
                                if stream.write_all(&chunk[..n]).await.is_err() {
                                    return;
                                }
                                w.fetch_add(n, Ordering::Relaxed);
                            }
                        }
                    }
                });
            }
        });
        (addr, read, written, heads)
    }

    /// An origin whose first connection answers its first request, reads its
    /// second whole and then closes without an answer; every later connection
    /// answers every request (`design 8.6`).
    async fn cutting_origin() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut first = true;
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let cut = std::mem::replace(&mut first, false);
                tokio::spawn(async move {
                    let mut buf: Vec<u8> = Vec::new();
                    let mut answered = 0u32;
                    loop {
                        let Some(at) = read_head(&mut buf, &mut stream).await else {
                            return;
                        };
                        let head = String::from_utf8_lossy(&buf[..at]).to_string();
                        buf.drain(..at + 4);
                        let len = content_length(&head);
                        if !fill_to(&mut buf, &mut stream, len).await {
                            return;
                        }
                        buf.drain(..len);
                        answered += 1;
                        if cut && answered >= 2 {
                            // the request was read whole; closing now is what
                            // the engine has to recover from
                            return;
                        }
                        if stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                });
            }
        });
        addr
    }

    /// An origin that reads one request whole and closes without an answer.
    async fn cuts_after_reading() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf: Vec<u8> = Vec::new();
                if let Some(at) = read_head(&mut buf, &mut stream).await {
                    let head = String::from_utf8_lossy(&buf[..at]).to_string();
                    buf.drain(..at + 4);
                    let len = content_length(&head);
                    let _ = fill_to(&mut buf, &mut stream, len).await;
                }
            }
        });
        addr
    }

    fn content_length(head: &str) -> usize {
        head.lines()
            .find_map(|l| {
                l.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .map(|(_, v)| v.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn a_late_response_head_is_a_504() {
        let origin = holding_origin(b"").await;
        let mut rig = rig_timed(
            Entry::Sniffed,
            origin.addr,
            Duration::from_millis(300),
            Duration::from_secs(5),
        )
        .await;
        rig.client
            .send("GET /slow HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 504"), "{head}");
        assert!(String::from_utf8_lossy(&body).contains("Gateway timeout"));
        let handle = rig.fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Failed("upstream response timed out".into()))
        );
        assert_eq!(handle.status(), Some(504));
    }

    #[tokio::test]
    async fn a_stalled_body_fails_the_request() {
        let origin = holding_origin(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc").await;
        let mut rig = rig_timed(
            Entry::Sniffed,
            origin.addr,
            Duration::from_millis(300),
            Duration::from_secs(5),
        )
        .await;
        rig.client
            .send("GET /stall HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let seen = rig.client.until_eof().await;
        assert!(seen.starts_with("HTTP/1.1 200"), "{seen}");
        assert!(seen.ends_with("abc"), "{seen}");
        let handle = rig.fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Failed("upstream body stalled".into()))
        );
        // the upstream connection went with it, not back into the pool
        tokio::time::timeout(Duration::from_secs(5), async {
            while origin.closed.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the upstream connection was closed");
        assert_eq!(origin.accepted.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn a_request_hyper_drops_is_finished_by_its_guard() {
        let origin = holding_origin(b"").await;
        let rig = connected(Entry::Sniffed, origin.addr).await;
        let Rig {
            mut client, fake, ..
        } = rig;
        client
            .send("GET /leave HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        // the request is in flight: the record exists
        fake.handle(0).await;
        drop(client);
        let handle = fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Failed(
                "client closed the connection".into()
            ))
        );
        assert_eq!(handle.status(), None);
    }

    #[tokio::test]
    async fn a_killed_request_closes_its_connection() {
        let origin = holding_origin(b"").await;
        let mut rig = connected(Entry::Sniffed, origin.addr).await;
        rig.client
            .send("GET /hang HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let handle = rig.fake.handle(0).await;
        handle.kill();
        assert_eq!(rig.client.until_eof().await, "");
        let handle = rig.fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Failed("killed".into()))
        );
    }

    /// What `serve_connection` returned, within 5 s.
    async fn served_of(served: tokio::task::JoinHandle<Served>) -> Served {
        tokio::time::timeout(Duration::from_secs(5), served)
            .await
            .expect("the connection ended within 5 s")
            .expect("the serving task did not panic")
    }

    #[tokio::test]
    async fn stopping_a_connection_ends_its_requests() {
        // a request waiting for its upstream
        let origin = holding_origin(b"").await;
        let Rig {
            mut client,
            fake,
            token,
            served,
        } = connected(Entry::Sniffed, origin.addr).await;
        client
            .send("GET /wait HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let handle = fake.handle(0).await;
        assert!(!handle.is_finished(), "the request was in flight");
        token.cancel();
        let served = served_of(served).await;
        assert_eq!(served.routed, 1);
        let handle = fake.finished(0).await;
        assert!(handle.is_finished());
        assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));

        // an idle keep-alive connection
        let origin = holding_origin(b"").await;
        let rig = connected(Entry::Sniffed, origin.addr).await;
        rig.stop();
        let served = rig.served().await;
        assert_eq!(served.routed, 0);

        // an answer on its way to a client that does not read it: the
        // graceful shutdown would wait for it to be written
        let origin = streaming_origin().await;
        let Rig {
            mut client,
            fake,
            token,
            served,
        } = rig_timed(
            Entry::Sniffed,
            origin.addr,
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .await;
        client
            .send("GET /big HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let handle = fake.handle(0).await;
        wait_until(|| handle.status() == Some(200) && handle.bytes().1 > 0).await;
        token.cancel();
        let served = served_of(served).await;
        assert_eq!(served.routed, 1);
        let handle = fake.finished(0).await;
        assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
    }

    #[tokio::test]
    async fn a_connection_that_reaches_no_route_says_why() {
        let origin = TestServer::spawn().await;
        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        rig.client.send("GET \u{1}\r\n\r\n").await.unwrap();
        let (head, _) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 400"), "{head}");
        let served = rig.served().await;
        assert_eq!(served.routed, 0);
        assert!(served.error.is_some(), "hyper says what it could not read");

        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;
        origin.set("/one", "first");
        origin.set("/two", "second");
        for (path, close) in [("/one", ""), ("/two", "Connection: close\r\n")] {
            rig.client
                .send(&format!(
                    "GET {path} HTTP/1.1\r\nHost: target.test\r\n{close}\r\n"
                ))
                .await
                .unwrap();
            let (head, _) = rig.client.response().await;
            assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        }
        let served = rig.served().await;
        assert_eq!(served.routed, 2);
        assert_eq!(served.error, None, "the client ended the connection");
    }

    #[tokio::test]
    async fn a_response_without_a_body_completes_its_request() {
        let origin = TestServer::spawn().await;
        origin.set("/head", "never sent for a HEAD");
        origin.set("/empty", "");
        origin.set_status("/empty", 204);
        origin.set("/again", "third");
        let mut rig = connected(Entry::Sniffed, addr_of(&origin)).await;

        rig.client
            .send("HEAD /head HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let head = rig.client.response_head().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(
            head.to_ascii_lowercase().contains("content-length: 21"),
            "the origin named a body the HEAD does not send: {head}"
        );
        let first = rig.fake.finished(0).await;
        assert_eq!(first.outcome(), Some(SessionOutcome::Completed));
        assert_eq!(first.status(), Some(200));

        rig.client
            .send("GET /empty HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 204"), "{head}");
        assert!(body.is_empty());
        let second = rig.fake.finished(1).await;
        assert_eq!(second.outcome(), Some(SessionOutcome::Completed));
        assert_eq!(second.status(), Some(204));

        rig.client
            .send("GET /again HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body, b"third");
        assert_eq!(rig.fake.dials(), 1, "the third request borrows too");
        assert_eq!(origin.connections(), 1);
    }

    #[tokio::test]
    async fn a_request_body_keeps_its_framing() {
        let (addr, records) = recording_origin().await;
        let mut rig = connected(Entry::Sniffed, addr).await;
        rig.client
            .send("GET /plain HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body, b"ok");
        {
            let records = records.lock().unwrap();
            let plain = records[0].head.to_ascii_lowercase();
            assert!(
                !plain.contains("transfer-encoding") && !plain.contains("content-length"),
                "a GET with no body was framed: {}",
                records[0].head
            );
        }

        rig.client
            .send("POST /post HTTP/1.1\r\nHost: target.test\r\nContent-Length: 5\r\n\r\nabcde")
            .await
            .unwrap();
        let (head, _) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        {
            let records = records.lock().unwrap();
            let post = &records[1];
            assert!(
                post.head.to_ascii_lowercase().contains("content-length: 5"),
                "{}",
                post.head
            );
            assert_eq!(post.body, b"abcde");
        }

        // A body-less `POST` is no body at all, not a chunked empty one:
        // hyper frames an unknown length as chunked for every method but
        // GET, HEAD and CONNECT (`proto/h1/role.rs`), so a wrapper that hid
        // the end of the body would frame this request.
        rig.client
            .send("POST /none HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let (head, _) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        {
            let recorded = records.lock().unwrap();
            let none = recorded[2].head.to_ascii_lowercase();
            assert!(
                !none.contains("transfer-encoding") && !none.contains("content-length"),
                "a POST with no body was framed: {}",
                recorded[2].head
            );
        }

        // A body the client never gave a length for keeps its framing even on
        // a method hyper would otherwise send with no body at all.
        rig.client
            .send(
                "GET /chunked HTTP/1.1\r\nHost: target.test\r\n\
                 Transfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
            )
            .await
            .unwrap();
        let (head, _) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let recorded = records.lock().unwrap();
        assert!(
            recorded[3]
                .head
                .to_ascii_lowercase()
                .contains("transfer-encoding: chunked"),
            "the body lost its framing: {}",
            recorded[3].head
        );
        assert_eq!(recorded[3].body, b"hello");
    }

    /// A body that arrives slower than `Conn::idle`: the wait for the response
    /// head counts from the last frame of the request's body, not from the
    /// send (`phase 4 M1 design 8.10`). The body is written in pieces on
    /// purpose — that is what the test is about — and `docs/testing.md` allows
    /// it for a test of a body that arrives late.
    #[tokio::test]
    async fn a_slow_upload_is_not_a_late_response() {
        let (addr, records) = recording_origin().await;
        let mut rig = rig_timed(
            Entry::Sniffed,
            addr,
            Duration::from_millis(400),
            Duration::from_secs(5),
        )
        .await;
        rig.client
            .send("POST /slow HTTP/1.1\r\nHost: target.test\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await
            .unwrap();
        for _ in 0..8 {
            // the engine may answer early: the write that finds it closed is no failure of the test
            let _ = rig.client.send("1\r\nx\r\n").await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let _ = rig.client.send("0\r\n\r\n").await;
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body, b"ok");
        let handle = rig.fake.finished(0).await;
        assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
        let records = records.lock().unwrap();
        assert_eq!(records[0].body, b"xxxxxxxx");
    }

    /// An origin-form request with no `Host` is named by the connection's
    /// target, written as a URL can carry it (`design 8.3`): the port only
    /// when it is not 80, an IPv6 address in brackets.
    #[test]
    fn a_request_without_host_is_named_by_its_target() {
        for (host, port, want) in [
            ("target.test", 80, "http://target.test/n"),
            ("target.test", 8080, "http://target.test:8080/n"),
            ("::1", 8080, "http://[::1]:8080/n"),
            ("::1", 80, "http://[::1]/n"),
        ] {
            let req = Request::builder().uri("/n").body(()).unwrap();
            let (url, http_host) = form_of(&req, (&HostName::parse(host), port));
            assert_eq!(url, want, "{host}:{port}");
            assert_eq!(http_host, None, "{host}:{port}");
        }
    }

    /// A client that takes the answer and then stops reading it: hyper stops
    /// polling the body once its write buffer is full, so `kill`, a stop and
    /// the idle timeout could not be seen from the body — the watchdog is what
    /// ends such a request (`design 8.10`, 8.11).
    #[tokio::test]
    async fn a_client_that_stops_reading_fails_after_idle() {
        let origin = streaming_origin().await;
        let mut rig = rig_timed(
            Entry::Sniffed,
            origin.addr,
            Duration::from_millis(300),
            Duration::from_secs(5),
        )
        .await;
        rig.client
            .send("GET /big HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        // the client reads nothing at all
        let handle = rig.fake.handle(0).await;
        wait_until(|| handle.status() == Some(200) && handle.bytes().1 > 0).await;
        let handle = rig.fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Failed("client stopped reading".into()))
        );
        assert_eq!(handle.status(), Some(200));
        let served = rig.served().await;
        assert_eq!(served.routed, 1);
        assert_eq!(
            served.error, None,
            "the engine dropped the connection itself"
        );
        wait_until(|| origin.closed.load(Ordering::Relaxed) > 0).await;
        assert_eq!(origin.accepted.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn a_killed_response_whose_client_does_not_read_is_closed() {
        let origin = streaming_origin().await;
        let mut rig = rig_timed(
            Entry::Sniffed,
            origin.addr,
            // long enough that the idle deadline is not the one that ends it
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .await;
        rig.client
            .send("GET /big HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let handle = rig.fake.handle(0).await;
        wait_until(|| handle.status() == Some(200) && handle.bytes().1 > 0).await;
        handle.kill();
        let handle = rig.fake.finished(0).await;
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Failed("killed".into()))
        );
        let served = rig.served().await;
        assert_eq!(served.routed, 1);
    }

    /// A body with nothing in it, for a request built directly.
    fn no_body() -> ReqBody {
        http_body_util::Empty::<Bytes>::new()
            .map_err(|never| -> BoxError { match never {} })
            .boxed()
    }

    #[tokio::test]
    async fn a_websocket_upgrade_is_relayed_and_counted_once() {
        let answer = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\nhello websocket";
        let (addr, read, written, heads) = upgrading_origin(answer).await;
        let mut rig = connected(Entry::Sniffed, addr).await;
        rig.client
            .send(
                "GET /ws HTTP/1.1\r\nHost: target.test\r\n\
                 Upgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
            )
            .await
            .unwrap();
        let head = rig.client.response_head().await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let lower = head.to_ascii_lowercase();
        assert!(lower.contains("upgrade: websocket"), "{head}");
        assert!(lower.contains("connection: upgrade"), "{head}");
        // the greeting was written with the 101, and may already be in `buf`
        assert_eq!(rig.client.take(15).await, b"hello websocket");
        rig.client.send("ping").await.unwrap();
        assert_eq!(rig.client.take(4).await, b"ping");
        rig.client.stream.shutdown().await.unwrap();
        while rig.client.fill().await {}
        let handle = rig.fake.finished(0).await;
        assert_eq!(handle.status(), Some(101));
        assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
        // what the origin was sent: the upgrade went on, and `Connection`
        // came back naming exactly it (design 8.7, 8.9)
        let heads = heads.lock().unwrap().clone();
        assert_eq!(heads.len(), 1, "one request reached the origin");
        let received = heads[0].to_ascii_lowercase();
        assert!(received.contains("upgrade: websocket"), "{received}");
        let connection = heads[0]
            .lines()
            .find_map(|l| {
                l.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case("connection"))
                    .map(|(_, v)| v.trim())
            })
            .expect("the origin was told the connection upgrades");
        assert!(
            connection.eq_ignore_ascii_case("upgrade"),
            "connection: {connection}"
        );
        let (up, down) = handle.bytes();
        assert_eq!(
            (up, down),
            (
                read.load(Ordering::Relaxed) as u64,
                written.load(Ordering::Relaxed) as u64
            ),
            "the bytes of the request and of the tunnel, each counted once"
        );
    }

    #[tokio::test]
    async fn a_connect_on_a_sniffed_connection_goes_to_its_target() {
        let (addr, _, _, heads) =
            upgrading_origin(b"HTTP/1.1 200 Connection Established\r\nContent-Length: 0\r\n\r\n")
                .await;
        let mut rig = connected(Entry::Sniffed, addr).await;
        rig.client
            .send("CONNECT x.test:443 HTTP/1.1\r\nHost: x.test:443\r\n\r\n")
            .await
            .unwrap();
        let head = rig.client.response_head().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        rig.client.send("ping").await.unwrap();
        assert_eq!(rig.client.take(4).await, b"ping");
        rig.client.stream.shutdown().await.unwrap();
        while rig.client.fill().await {}
        let seen = rig.fake.seen();
        let s = &seen[0];
        // the target is still the connection's; the request line names the
        // client's own proxy, and it has no URL (design 8.9)
        assert_eq!(s.session.dst_host, HostName::parse("target.test"));
        assert_eq!(s.session.dst_port, 80);
        assert_eq!(s.session.url, None);
        assert_eq!(s.session.http_host.as_deref(), Some("x.test"));
        let handle = rig.fake.finished(0).await;
        assert_eq!(handle.status(), Some(200));
        assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
        // the `CONNECT` reached the connection's target as it came (design
        // 8.9)
        let heads = heads.lock().unwrap().clone();
        assert_eq!(heads.len(), 1, "one request reached the origin");
        assert!(
            heads[0].starts_with("CONNECT x.test:443 HTTP/1.1"),
            "{}",
            heads[0]
        );
    }

    #[tokio::test]
    async fn a_connect_refused_by_the_target_is_answered_as_it_is() {
        let origin =
            holding_origin(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 4\r\n\r\nnope").await;
        let mut rig = connected(Entry::Sniffed, origin.addr).await;
        rig.client
            .send("CONNECT x.test:443 HTTP/1.1\r\nHost: x.test:443\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 403"), "{head}");
        assert_eq!(body, b"nope");
        let handle = rig.fake.finished(0).await;
        assert_eq!(handle.status(), Some(403));
        assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
    }

    #[tokio::test]
    async fn a_101_the_request_did_not_ask_for_is_a_502() {
        let origin = holding_origin(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        )
        .await;
        let mut rig = connected(Entry::Sniffed, origin.addr).await;
        rig.client
            .send("GET /plain HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let (head, body) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        assert!(
            String::from_utf8_lossy(&body).contains(
                "Upstream answered 101 Switching Protocols to a request that did not ask to upgrade"
            ),
            "{}",
            String::from_utf8_lossy(&body)
        );
        let handle = rig.fake.finished(0).await;
        assert_eq!(handle.status(), Some(502));
        assert_eq!(
            handle.outcome(),
            Some(SessionOutcome::Failed(
                "upstream answered 101 Switching Protocols unasked".into()
            ))
        );
    }

    #[tokio::test]
    async fn a_request_not_yet_written_is_sent_again_on_a_new_connection() {
        // A connection nobody polls: its dispatch is gone, so `try_send`
        // refuses the request on the spot and hands it back whole. That is
        // the `NotWritten` a second attempt is built on (design 8.6).
        let (server, _client) = tokio::io::duplex(64);
        let (mut sender, conn) = hyper::client::conn::http1::Builder::new()
            .handshake(TokioIo::new(server))
            .await
            .unwrap();
        drop(conn);
        let req = Request::builder()
            .method("POST")
            .uri("http://target.test/x")
            .body(no_body())
            .unwrap();
        match send(&mut sender, req).await {
            Sent::NotWritten(req, e) => {
                assert_eq!(req.method(), http::Method::POST);
                assert_eq!(req.uri(), &"http://target.test/x".parse::<Uri>().unwrap());
                assert!(e.is_canceled(), "{e}");
            }
            Sent::Answered(_) => panic!("a connection nobody polled answered"),
            Sent::Failed(e) => panic!("the request was not handed back: {e}"),
        }
    }

    #[tokio::test]
    async fn an_idempotent_request_cut_off_is_sent_again_once() {
        let addr = cutting_origin().await;
        let mut rig = connected(Entry::Sniffed, addr).await;
        for path in ["/one", "/two"] {
            rig.client
                .send(&format!("GET {path} HTTP/1.1\r\nHost: target.test\r\n\r\n"))
                .await
                .unwrap();
            let (head, body) = rig.client.response().await;
            assert!(head.starts_with("HTTP/1.1 200"), "{head}");
            assert_eq!(body, b"ok");
        }
        assert_eq!(rig.fake.dials(), 2, "the cut-off request dials again");
        let second = rig.fake.finished(1).await;
        assert_eq!(second.outcome(), Some(SessionOutcome::Completed));
    }

    #[tokio::test]
    async fn a_post_cut_off_after_it_was_written_is_a_502() {
        let addr = cutting_origin().await;
        let mut rig = connected(Entry::Sniffed, addr).await;
        rig.client
            .send("GET /one HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let (head, _) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        rig.client
            .send(
                "POST /two HTTP/1.1\r\nHost: target.test\r\n\
                 Content-Length: 5\r\n\r\nabcde",
            )
            .await
            .unwrap();
        let (head, _) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        assert_eq!(rig.fake.dials(), 1, "a request with a body goes again");
        let second = rig.fake.finished(1).await;
        assert!(matches!(second.outcome(), Some(SessionOutcome::Failed(_))));
    }

    #[tokio::test]
    async fn a_request_on_a_new_connection_is_not_sent_again() {
        let addr = cuts_after_reading().await;
        let mut rig = connected(Entry::Sniffed, addr).await;
        rig.client
            .send("GET /once HTTP/1.1\r\nHost: target.test\r\n\r\n")
            .await
            .unwrap();
        let (head, _) = rig.client.response().await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        assert_eq!(rig.fake.dials(), 1, "a fresh connection gets one send");
        let handle = rig.fake.finished(0).await;
        assert!(matches!(handle.outcome(), Some(SessionOutcome::Failed(_))));
    }
}
