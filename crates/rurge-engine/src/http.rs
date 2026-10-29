//! The engine's side of the HTTP engine (phase 4 M1 design 8.4, 8.5, 10.2):
//! one `Upstreams` per client connection, whose requests are routed and
//! dialed on their own and share the connection's pool of upstream
//! connections. A request's routing, its dial and the `smart` group's
//! bookkeeping are the engine's (`route_session`, `dial_route`); this module
//! only carries the connection's token and process slot into them.

use crate::engine::{Engine, Refused, Way};
use rurge_config::HostName;
use rurge_config::session::SessionInfo;
use rurge_dns::fakeip::Lease;
use rurge_http::{Named, Pages, Refusal, Routed, Upstream, Upstreams};
use rurge_inbound::{Close, ProcessSlot, ResponseBody, SessionHandle};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, Route, Target};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// What a pooled upstream connection is kept under (phase 4 M1 design 8.5):
/// two requests with equal keys may share one. The generation and the
/// registry are the pair `route_session` read, the outbound and the chain the
/// first attempt's, and the egress table the one rurge's own sockets were
/// bound to when the dial was made — a route keeps its object alive, so an
/// old generation is never mistaken for a new one at the same address.
#[derive(PartialEq)]
pub(crate) struct PoolKey {
    generation: Route,
    registry: Route,
    outbound: Route,
    chain: Vec<String>,
    target: Target,
    may_forward: bool,
    egress: Option<Route>,
}

/// The `Upstreams` of one sniffed client connection (phase 4 M1 design 8.4,
/// 10.2): every request's handle is a child of the connection's token, and
/// the pool lives as long as the connection's `Requests`.
pub(crate) struct ConnUpstreams {
    engine: Arc<Engine>,
    token: CancellationToken,
    process: ProcessSlot,
    /// The lease of the fake address the connection came in on, if any: held
    /// here rather than in `serve_http`, so that an upgraded relay, which
    /// outlives that function, keeps the address the name's (the phase 4 /
    /// M1b plan's P9).
    _lease: Option<Lease>,
    /// The adapter's stop, if the connection came in through it: an upgraded
    /// relay outlives `serve_http` and still has to end when the adapter does
    /// (`rurge-tun`: a stop cancels the sessions it is relaying).
    stop: Option<CancellationToken>,
    /// The connection carries the HTTP listener's absolute-form requests
    /// (`Entry::Listener`), not a sniffed connection's (`phase 4 M1 design 9`):
    /// the two differ in whether a request's host is asked of the fake pool
    /// and in whether each request is a TCP session for `block-quic=adaptive`.
    listener: bool,
}

impl ConnUpstreams {
    pub(crate) fn new(
        engine: Arc<Engine>,
        token: CancellationToken,
        process: ProcessSlot,
        lease: Option<Lease>,
        stop: Option<CancellationToken>,
        listener: bool,
    ) -> ConnUpstreams {
        ConnUpstreams {
            engine,
            token,
            process,
            _lease: lease,
            stop,
            listener,
        }
    }
}

impl Upstreams for ConnUpstreams {
    type Key = PoolKey;
    type Route = Way;
    type Lease = Lease;

    fn named(&self, host: &HostName) -> Named<Lease> {
        if !self.listener {
            // A sniffed connection named its target before it came in: the
            // engine turns a fake address back into its name in `serve_http`,
            // once for the whole connection.
            return Named::No;
        }
        // A listener request names its target in its URL: a fake address there
        // is the name behind it, and no name at all is a failure (design 9,
        // R27).
        self.engine.named_fake_address(host)
    }

    fn open(&self, session: SessionInfo) -> Arc<SessionHandle> {
        let handle = self
            .engine
            .new_handle_under(session, self.process.clone(), &self.token);
        if self.listener {
            // One request on the listener is one session of a client's, as it
            // was before the engine carried them: a host whose QUIC the engine
            // blocked is that client falling back (phase 3 M5 design 6.3).
            self.engine
                .quic_adaptive()
                .on_tcp(&handle.session().dst_host.to_string(), Instant::now());
        }
        handle
    }

    fn route<'a>(
        &'a self,
        handle: &'a Arc<SessionHandle>,
        forwardable: bool,
    ) -> BoxFuture<'a, Routed<Way, PoolKey>> {
        Box::pin(async move {
            let (rt, registry) = self.engine.snapshot();
            // Every request of every entry reaches the rules through here, the
            // refused ones too (phase 4 M1 design 11, the phase 4 / M1b plan's
            // P19). A connection whose requests none of them reached has no
            // request at all, and is recorded as the connection it was.
            self.engine.request_count.fetch_add(1, Ordering::Relaxed);
            let routed = handle
                .trail()
                .follow(
                    self.engine
                        .route_session(&rt, &registry, handle, forwardable),
                )
                .await;
            let pages = Pages {
                error: rt.config.general.show_error_page,
                reject: rt.config.general.show_error_page_for_reject,
            };
            match routed {
                Ok(way) => {
                    let first = way.attempts.first().expect("a route has an attempt");
                    let key = PoolKey {
                        generation: Route::of(&rt),
                        registry: Route::of(&registry),
                        outbound: Route::of(&first.outbound),
                        chain: first.chain.clone(),
                        target: way.target.clone(),
                        may_forward: way.may_forward,
                        egress: rt.factory.egress_route(),
                    };
                    Routed::Go {
                        route: way,
                        key,
                        pages,
                    }
                }
                Err(refused) => {
                    crate::engine::settle(handle, &refused);
                    Routed::Refused {
                        refusal: refused.into(),
                        pages,
                    }
                }
            }
        })
    }

    fn dial<'a>(
        &'a self,
        handle: &'a Arc<SessionHandle>,
        route: Way,
    ) -> BoxFuture<'a, Result<Upstream, Refusal>> {
        Box::pin(async move {
            let dialed = handle
                .trail()
                .follow(self.engine.dial_route(handle, route))
                .await;
            match dialed {
                Ok((stream, forward)) => Ok(Upstream {
                    stream,
                    forward,
                    peer: handle.trail().peer(),
                }),
                Err(refused) => {
                    crate::engine::settle(handle, &refused);
                    Err(refused.into())
                }
            }
        })
    }

    fn relay<'a>(
        &'a self,
        client: BoxedStream,
        upstream: BoxedStream,
        handle: Arc<SessionHandle>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            // An upgraded exchange outlives `serve_http`, so the adapter's
            // stop is raced here: the pump is waited for and not dropped, as
            // `serve_sniffed` does (design 8.9).
            let token = handle.token().clone();
            let idle = self.engine.runtime().idle_timeout;
            let mut relay = std::pin::pin!(crate::relay::pump(client, upstream, handle, idle));
            tokio::select! {
                biased;
                _ = crate::serve::stopped(self.stop.as_ref()) => {
                    token.cancel();
                    relay.await;
                }
                _ = &mut relay => {}
            }
        })
    }
}

impl From<Refused> for Refusal {
    fn from(refused: Refused) -> Refusal {
        match refused {
            Refused::Reject(kind) => Refusal::Reject(kind),
            Refused::Fail { kind, message } => Refusal::Fail { kind, message },
        }
    }
}

/// The HTTP listener's requests of one accepted connection (`phase 4 M1
/// design 9`, `P12`): the engine's own request flow, one record and one pool
/// per connection, exactly as a sniffed connection's requests have.
pub(crate) struct PlainRequests {
    requests: rurge_http::Requests<ConnUpstreams>,
}

impl PlainRequests {
    pub(crate) fn new(requests: rurge_http::Requests<ConnUpstreams>) -> PlainRequests {
        PlainRequests { requests }
    }
}

impl rurge_inbound::PlainRequests for PlainRequests {
    fn handle(
        &self,
        req: rurge_inbound::Request<rurge_inbound::Incoming>,
    ) -> BoxFuture<'_, Result<rurge_inbound::Response<ResponseBody>, Close>> {
        Box::pin(async move { self.requests.handle(req).await })
    }
}
