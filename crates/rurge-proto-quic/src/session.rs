//! A policy's QUIC connection, shared by its requests (phase 2 M7 design
//! 3.5) — or its connections, under a cap on the requests each carries
//! (`Session::with_stream_cap`).
//!
//! - The connection is dialed when a request first needs one, and the
//!   requests that come meanwhile wait for that dial: the path to the
//!   server, QUIC's handshake, then the protocol's own
//!   (`Protocol::handshake`: Hysteria 2's authentication). Its failure is
//!   theirs too.
//! - Under a cap (`trust-tunnel`'s `max-streams` over HTTP/3), a request
//!   takes the oldest connection that carries fewer requests than the cap,
//!   as the HTTP/2 pool does (phase 2 M6 design 16, 18). With none, it
//!   waits for the dials in flight while they have room for it — each is
//!   room for `cap` requests, its own and the ones that wait for it —,
//!   else it dials another, `MAX_DIALS` at most at once. Once no dial is in
//!   flight, a request that waited shares the latest dial's failure if that
//!   dial ended after it began to wait. Each connection then goes by the
//!   rules below on its own.
//! - A connection that has closed — an error, the idle timeout, the
//!   server's close, the loss of the UDP path under it (which quinn reports
//!   as a local close) — or that its protocol state says is going away (an
//!   HTTP/3 GOAWAY) takes no new requests: the next one dials. The requests
//!   under way on it go on; their leases keep it.
//! - So does a connection dialled through an underlying policy that leads
//!   elsewhere now (`Connector::route`: a group's member changed, a reload
//!   rebuilt the policy), for good: the next request dials the new way.
//!   Looked at whenever a connection is picked — by a request, or by a UDP
//!   session for its next datagram, which compares the route at most once
//!   a second (`Session::serves`, else `serves_without_route`) —, never by
//!   a timer.
//! - A connection that no request or UDP session has held (`Lease`) for
//!   `IDLE_TIMEOUT` is closed: looked at on every request and by a reaper.
//! - A request on a connection that was up when it came — taken at once,
//!   or, under a cap, once a place freed up while it waited —, which finds
//!   the connection gone before any of the request's own data went out,
//!   goes again, once, on a connection dialed after that one: a new one,
//!   or under a cap one with room (`Session::request`). A server that
//!   dropped the connection while it idled answers the request's first
//!   packet with a stateless reset, as it would on any connection dialed
//!   before that one.
//! - Nothing here bounds how long a request takes: the caller's
//!   `opts.timeout` covers the dial, both handshakes and the request.

use quinn::{AsyncUdpSocket, Connection, ConnectionError, VarInt};
use rurge_config::KeystoreItem;
use rurge_config::spec::{PortHopping, QuicOpts, TlsOpts, Tristate};
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, Connector, Route, Target};
use rurge_net::quic::{self, Path, Transport};
use rurge_proto::build::quic_tls;
use rurge_proto::{BuildError, OutboundError};
use rustls::RootCertStore;
use std::io;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::task::AbortOnDropHandle;

/// How long a connection may go without a lease before the session closes
/// it (as the HTTP/2 pool, phase 2 M6 design 5.2).
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// The most dials in flight at once under a cap, as in the HTTP/2 pool: a
/// burst after a network change does not open hundreds of connections
/// together.
const MAX_DIALS: usize = 8;

/// What a protocol lays over each connection's socket, outermost: over the
/// hops and the carrier too (Hysteria 2's obfuscation, design 5.4).
pub type Layer = Box<dyn Fn(Arc<dyn AsyncUdpSocket>) -> Arc<dyn AsyncUdpSocket> + Send + Sync>;

/// How a session's connections are made (design 3.2, 3.3): the UDP path to
/// the server — under what the protocol lays over it (`with_layer`) —, then
/// QUIC's handshake with the policy's TLS. Built once per outbound.
pub struct Dialer {
    connector: Arc<dyn Connector>,
    server: Target,
    hopping: Option<PortHopping>,
    ecn: Tristate,
    config: quinn::ClientConfig,
    /// What TLS verifies and sends as SNI (none for an IP address).
    server_name: String,
    layer: Option<Layer>,
}

impl Dialer {
    /// The path goes as `connector` dials: from sockets of its own when it
    /// dials directly, else over its UDP flow (`underlying-proxy`), which
    /// cannot hop. `tls` is the policy's — ALPN `h3` unless it names others
    /// — with `client-cert` from `keystore`.
    pub fn new(
        connector: Arc<dyn Connector>,
        server: Target,
        tls: &TlsOpts,
        quic: &QuicOpts,
        ecn: Tristate,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
    ) -> Result<Dialer, BuildError> {
        // the configuration refuses a line that has both (E0018), but a
        // group's `underlying-proxy` chains its members without it (`M (via
        // R)`, phase 2 M3 design 5.4): such a member is not built, and the
        // registry leaves it out
        if quic.port_hopping.is_some() && connector.as_direct().is_none() {
            return Err(BuildError::new(
                "`port-hopping` cannot be combined with `underlying-proxy`",
            ));
        }
        let tls = quic_tls(tls, &server.host, &["h3"], keystore, roots)?;
        let config = quic::client_config(tls.config, &Transport::STANDARD)
            .map_err(|e| BuildError::new(e.to_string()))?;
        Ok(Dialer {
            connector,
            server,
            hopping: quic.port_hopping.clone(),
            ecn,
            config,
            server_name: tls.server_name,
            layer: None,
        })
    }

    /// Lays `layer` over every connection's socket.
    pub fn with_layer(self, layer: Layer) -> Dialer {
        Dialer {
            layer: Some(layer),
            ..self
        }
    }

    /// A new connection to the server: nothing bounds it here but quinn's
    /// idle timer.
    async fn connect(&self, opts: &ConnectOpts) -> io::Result<Connection> {
        let mut path = Path::open(
            self.connector.as_ref(),
            &self.server,
            self.hopping.as_ref(),
            self.ecn,
            opts,
        )
        .await?;
        if let Some(layer) = &self.layer {
            path.socket = layer(path.socket);
        }
        quic::connect(path, &self.config, &self.server_name).await
    }

    /// Where the connector leads now (`Connector::route`): a connection
    /// dialled while it led elsewhere goes the old way.
    fn route(&self) -> Option<Route> {
        self.connector.route()
    }
}

/// What a protocol does on each new connection before any request uses it,
/// and what the connection's requests then share.
pub trait Protocol: Send + Sync + 'static {
    /// One connection's: what its handshake set up or learned (Hysteria 2:
    /// its HTTP/3 client, which must live as long as the connection, and
    /// whether the server relays UDP). Dropped with the connection.
    type State: Send + Sync + 'static;

    /// The protocol's name, as its log lines begin (`hysteria2`).
    const NAME: &'static str;

    /// The application error code a connection is closed with when the
    /// session lets it go (Hysteria 2: HTTP/3's H3_NO_ERROR).
    const NO_ERROR: VarInt;

    /// Runs on every new connection once QUIC's handshake is done. Its
    /// failure is the dial's, shared by the requests that waited for it;
    /// the connection is then closed with `NO_ERROR`, unless the protocol
    /// closed it with a code of its own.
    fn handshake<'a>(
        &'a self,
        connection: &'a Connection,
    ) -> BoxFuture<'a, Result<Self::State, OutboundError>>;

    /// Whether `state` says the connection takes no new requests (an HTTP/3
    /// GOAWAY); those under way go on.
    fn is_going_away(&self, state: &Self::State) -> bool {
        let _ = state;
        false
    }
}

/// A hold on one of the session's connections, for one request or UDP
/// session: the connection lives at least as long, and is not idle while
/// it does.
pub struct Lease<S> {
    conn: Arc<Conn<S>>,
}

impl<S> Lease<S> {
    pub fn connection(&self) -> &Connection {
        &self.conn.quic
    }

    /// What the protocol's handshake left for the connection's requests.
    pub fn state(&self) -> &S {
        &self.conn.state
    }
}

impl<S> Drop for Lease<S> {
    fn drop(&mut self) {
        let mut usage = self.conn.usage.lock().expect("usage");
        usage.leases -= 1;
        if usage.leases == 0 {
            usage.idle_since = Instant::now();
        }
    }
}

/// How a request on one of the session's connections failed.
#[derive(Debug)]
pub enum Failure {
    /// The connection could not take the request — it turned out closed,
    /// or going away — and nothing of the request's own data went out: on
    /// another connection the request may yet go through.
    Lost(OutboundError),
    /// Final.
    Failed(OutboundError),
}

impl Failure {
    /// The connection's end, met by a request before any of its data went
    /// out (quinn's `ConnectionLost` on one of its streams).
    pub fn lost(e: &ConnectionError) -> Failure {
        Failure::Lost(quic::connection_error(e).into())
    }

    fn into_error(self) -> OutboundError {
        match self {
            Failure::Lost(e) | Failure::Failed(e) => e,
        }
    }
}

impl From<OutboundError> for Failure {
    fn from(e: OutboundError) -> Failure {
        Failure::Failed(e)
    }
}

/// A policy's QUIC connection — under a cap, its connections —, shared by
/// its requests (design 3.5). No `Debug`: the protocol may hold
/// credentials.
pub struct Session<P: Protocol> {
    dialer: Dialer,
    shared: Arc<Shared<P>>,
    /// The most leases a connection carries at once (`with_stream_cap`);
    /// without a cap, the one connection takes them all.
    cap: Option<usize>,
    /// A dial ended, or its request gave up on it: the requests waiting for
    /// one look again.
    dialed: Notify,
    /// Started by the first request: building an outbound (a dry build
    /// included) leaves no task behind.
    reaper: OnceLock<AbortOnDropHandle<()>>,
}

/// What the session's requests and its reaper share.
struct Shared<P: Protocol> {
    /// The policy's name, for the log.
    policy: String,
    protocol: P,
    /// How long a connection may go without a lease.
    idle: Duration,
    slot: Mutex<Slot<P::State>>,
}

/// The session's connections and their dials.
struct Slot<S> {
    /// The connections new requests go to, oldest first: one at most
    /// without a cap.
    conns: Vec<Arc<Conn<S>>>,
    /// Dials in flight: one at most without a cap.
    dialing: usize,
    /// Requests waiting for them.
    waiting: usize,
    /// Dials that have ended, success or failure.
    dials: u64,
    /// The latest dial's failure and the `dials` it ended as; `None` after
    /// a success.
    failure: Option<(u64, OutboundError)>,
}

impl<P: Protocol> Session<P> {
    /// `policy`'s session; dials nothing yet: the first request does.
    pub fn new(policy: &str, dialer: Dialer, protocol: P) -> Session<P> {
        Session::with_idle(policy, dialer, protocol, IDLE_TIMEOUT)
    }

    fn with_idle(policy: &str, dialer: Dialer, protocol: P, idle: Duration) -> Session<P> {
        Session {
            dialer,
            shared: Arc::new(Shared {
                policy: policy.to_string(),
                protocol,
                idle,
                slot: Mutex::new(Slot {
                    conns: Vec::new(),
                    dialing: 0,
                    waiting: 0,
                    dials: 0,
                    failure: None,
                }),
            }),
            cap: None,
            dialed: Notify::new(),
            reaper: OnceLock::new(),
        }
    }

    /// The session with each connection carrying `cap` leases at most (1
    /// at least): it keeps as many connections as its requests need
    /// (`trust-tunnel`'s `max-streams` over HTTP/3, design 3.5).
    pub fn with_stream_cap(self, cap: usize) -> Session<P> {
        Session {
            cap: Some(cap.max(1)),
            ..self
        }
    }

    /// A lease on a connection of the session — under a cap, one with room
    /// —, dialing one when there is none: once for all the requests that
    /// come meanwhile, or, under a cap, as often as they need.
    pub async fn lease(&self, opts: &ConnectOpts) -> Result<Lease<P::State>, OutboundError> {
        self.take(opts, None).await.map(|(lease, _)| lease)
    }

    /// Whether `lease`'s connection still takes what is new — a request, a
    /// UDP session's next datagram —: open, not going away, and dialled the
    /// way the connector leads now. A UDP session on one that does not
    /// moves to the session's connection (`lease` again); what is under
    /// way on it goes on. Where the connector leads can cost a group's
    /// resolution: a UDP session, which asks for every datagram, asks this
    /// at most once a second, `serves_without_route` in between. Under a
    /// cap, a new request needs room on it too.
    pub fn serves(&self, lease: &Lease<P::State>) -> bool {
        let conn = &lease.conn;
        conn.quic.close_reason().is_none()
            && !self.shared.protocol.is_going_away(&conn.state)
            && self.shared.on_route(conn, self.dialer.route().as_ref())
    }

    /// `serves` without asking where the connector leads now: open, not
    /// going away, and not found off the way already by whoever asked
    /// (`on_route` remembers it).
    pub fn serves_without_route(&self, lease: &Lease<P::State>) -> bool {
        let conn = &lease.conn;
        conn.quic.close_reason().is_none()
            && !self.shared.protocol.is_going_away(&conn.state)
            && !conn.rerouted.load(Ordering::Relaxed)
    }

    /// Runs `attempt` on a lease of the session's connection; what it
    /// returns may keep the lease (a stream). A request that finds a
    /// connection that was up when it came gone before any of its own data
    /// went out (`Failure::Lost`) is attempted again, once, on a connection
    /// dialed after that one — a new one, or under a cap one with room —:
    /// any dialed before is as stale. On a connection dialed for it, or
    /// while it waited, it fails. Either way the connection found gone
    /// takes no new requests.
    pub async fn request<'a, T>(
        &'a self,
        opts: &'a ConnectOpts,
        mut attempt: impl FnMut(Lease<P::State>) -> BoxFuture<'a, Result<T, Failure>>,
    ) -> Result<T, OutboundError> {
        let (lease, was_up) = self.take(opts, None).await?;
        let conn = lease.conn.clone();
        let e = match attempt(lease).await {
            Err(Failure::Lost(e)) => e,
            done => return done.map_err(Failure::into_error),
        };
        self.forget(&conn);
        let lost = conn.dialed;
        drop(conn);
        if !was_up {
            return Err(e);
        }
        // the error says its layer (`quic: …`, or the protocol's)
        tracing::debug!("{e}; the request goes again on a new connection");
        let (lease, _) = self.take(opts, Some(lost)).await?;
        attempt(lease).await.map_err(Failure::into_error)
    }

    /// A lease on a connection that takes requests — `after` a dial, only
    /// on one dialed after it (`Conn::dialed`) —, dialing one when there is
    /// none; and whether the connection was up when the request came:
    /// dialed before the request first looked, whether it took it at once
    /// or once a place freed up on it while it waited.
    async fn take(
        &self,
        opts: &ConnectOpts,
        after: Option<u64>,
    ) -> Result<(Lease<P::State>, bool), OutboundError> {
        self.reaper.get_or_init(|| spawn_reaper(&self.shared));
        // the dials that had ended when this request first looked
        let mut first_look = None;
        // the dials that had ended when this request last began to wait
        let mut waited_since = None;
        loop {
            // registered before the look: a dial that ends after it wakes us
            let dialed = self.dialed.notified();
            let mut dialed = pin!(dialed);
            dialed.as_mut().enable();
            // where the connector leads now, asked outside the lock
            let route = self.dialer.route();
            {
                let mut slot = self.shared.slot.lock().expect("session");
                let came = *first_look.get_or_insert(slot.dials);
                if let Some(lease) = self.shared.pick(&mut slot, route.as_ref(), self.cap, after) {
                    let was_up = lease.conn.dialed < came;
                    return Ok((lease, was_up));
                }
                if let Some(since) = waited_since
                    && slot.dialing == 0
                    && let Some((at, failure)) = &slot.failure
                    && *at > since
                {
                    return Err(copy_error(failure));
                }
                if self.dials_another(&slot) {
                    slot.dialing += 1;
                    break;
                }
                slot.waiting += 1;
                waited_since = Some(slot.dials);
            }
            let _waiting = Waiting(&self.shared);
            dialed.await;
        }
        self.dial(opts).await.map(|lease| (lease, false))
    }

    /// Whether a request that found no room dials, rather than waits for
    /// the dials in flight. Without a cap, a dial is room for every request
    /// that comes meanwhile. Under one, a dial is room for its own request
    /// and `cap - 1` that wait for it, and a request that would not fit
    /// dials another, `MAX_DIALS` at most at once.
    fn dials_another(&self, slot: &Slot<P::State>) -> bool {
        match self.cap {
            None => slot.dialing == 0,
            Some(cap) => {
                slot.dialing < MAX_DIALS && slot.waiting >= slot.dialing.saturating_mul(cap - 1)
            }
        }
    }

    /// A new connection and its first lease. The dial stays in flight until
    /// it ends or its request gives up on it.
    async fn dial(&self, opts: &ConnectOpts) -> Result<Lease<P::State>, OutboundError> {
        let dialing = Dialing(self);
        let result = self.connect(opts).await;
        let lease = {
            let mut slot = self.shared.slot.lock().expect("session");
            let dialed = slot.dials;
            slot.dials += 1;
            match result {
                Ok((quic, state, route)) => {
                    slot.failure = None;
                    let conn = Arc::new(Conn {
                        dialed,
                        quic,
                        state,
                        route,
                        rerouted: AtomicBool::new(false),
                        usage: Mutex::new(Usage {
                            leases: 0,
                            idle_since: Instant::now(),
                        }),
                        no_error: P::NO_ERROR,
                    });
                    let lease = conn.lease();
                    slot.conns.push(conn);
                    Ok(lease)
                }
                Err(e) => {
                    slot.failure = Some((slot.dials, copy_error(&e)));
                    Err(e)
                }
            }
        };
        drop(dialing);
        lease
    }

    /// QUIC's handshake over the path to the server, then the protocol's:
    /// the connection, what the protocol's handshake left, and where the
    /// connector led.
    async fn connect(
        &self,
        opts: &ConnectOpts,
    ) -> Result<(Connection, P::State, Option<Route>), OutboundError> {
        // asked before the dial: should the connector lead elsewhere while
        // it goes on, the next request finds the connection off the way,
        // never the other way round
        let route = self.dialer.route();
        let quic = self.dialer.connect(opts).await?;
        match self.shared.protocol.handshake(&quic).await {
            Ok(state) => Ok((quic, state, route)),
            Err(e) => {
                close(&quic, P::NO_ERROR);
                Err(e)
            }
        }
    }

    /// Lets go of `conn` if requests still go to it.
    fn forget(&self, conn: &Arc<Conn<P::State>>) {
        let mut slot = self.shared.slot.lock().expect("session");
        slot.conns.retain(|kept| !Arc::ptr_eq(kept, conn));
    }

    /// Test support: the reaper's handle, once a request started it.
    #[cfg(test)]
    fn reaper(&self) -> Option<tokio::task::AbortHandle> {
        self.reaper.get().map(AbortOnDropHandle::abort_handle)
    }
}

impl<P: Protocol> Shared<P> {
    /// A lease on the connection a request goes to, when one takes it: the
    /// oldest with room under `cap` — `after` a dial, of those dialed after
    /// it. A connection that takes no requests — `prune`'s, or one dialled
    /// another way than the connector leads now (`route`) — is let go of on
    /// the way. The one place a connection is chosen.
    fn pick(
        &self,
        slot: &mut Slot<P::State>,
        route: Option<&Route>,
        cap: Option<usize>,
        after: Option<u64>,
    ) -> Option<Lease<P::State>> {
        self.prune(slot, Instant::now());
        slot.conns.retain(|conn| self.on_route(conn, route));
        slot.conns
            .iter()
            .find(|conn| after.is_none_or(|after| conn.dialed > after) && conn.has_room(cap))
            .map(Conn::lease)
    }

    /// Lets go of each connection once it takes no new requests: closed,
    /// going away, or without a lease for `idle`. Its leases keep it until
    /// they go too; then it is closed. The reaper's look too, which leaves
    /// where the connector leads to the requests (`pick`).
    fn prune(&self, slot: &mut Slot<P::State>, now: Instant) {
        slot.conns.retain(|conn| {
            conn.quic.close_reason().is_none()
                && !self.protocol.is_going_away(&conn.state)
                && !conn.is_expired(now, self.idle)
        });
    }

    /// Whether `conn` goes the way the connector leads now (`route`). Once
    /// it does not — a group's member changed under it, a reload rebuilt
    /// the underlying policy —, it takes nothing new again, whichever way
    /// the connector leads later; the first to see it says so.
    fn on_route(&self, conn: &Conn<P::State>, route: Option<&Route>) -> bool {
        if conn.rerouted.load(Ordering::Relaxed) {
            return false;
        }
        if conn.route.as_ref() == route {
            return true;
        }
        if !conn.rerouted.swap(true, Ordering::Relaxed) {
            tracing::info!(
                policy = %self.policy,
                "{}: the underlying policy changed; a new connection is made",
                P::NAME
            );
        }
        false
    }
}

/// A dial in flight; when it ends, the requests waiting for it look again.
struct Dialing<'a, P: Protocol>(&'a Session<P>);

impl<P: Protocol> Drop for Dialing<'_, P> {
    fn drop(&mut self) {
        self.0.shared.slot.lock().expect("session").dialing -= 1;
        self.0.dialed.notify_waiters();
    }
}

/// A request waiting for the dials in flight.
struct Waiting<'a, P: Protocol>(&'a Shared<P>);

impl<P: Protocol> Drop for Waiting<'_, P> {
    fn drop(&mut self) {
        self.0.slot.lock().expect("session").waiting -= 1;
    }
}

/// One connection and its protocol state. The session holds it while it
/// takes requests, each lease while the lease lives; when the last of them
/// lets go, the connection is closed.
struct Conn<S> {
    /// Its place among the session's dials, by when they ended: the dials
    /// that had ended before it did (`Slot::dials`).
    dialed: u64,
    quic: Connection,
    state: S,
    /// Where the connector led when the connection was dialled.
    route: Option<Route>,
    /// The connector has led elsewhere since: nothing new goes to it.
    rerouted: AtomicBool,
    usage: Mutex<Usage>,
    /// `Protocol::NO_ERROR`.
    no_error: VarInt,
}

struct Usage {
    leases: usize,
    /// When `leases` last fell to 0, or the connection came up.
    idle_since: Instant,
}

impl<S> Conn<S> {
    /// Taken under the session's lock: the reaper, which looks under the
    /// same lock, never lets go of a connection a lease was just taken on.
    fn lease(self: &Arc<Self>) -> Lease<S> {
        self.usage.lock().expect("usage").leases += 1;
        Lease { conn: self.clone() }
    }

    /// Whether the connection carries fewer leases than `cap`; any number
    /// without one. Looked at under the session's lock, as leases are
    /// taken: only a lease let go of can change it before `lease`.
    fn has_room(&self, cap: Option<usize>) -> bool {
        cap.is_none_or(|cap| self.usage.lock().expect("usage").leases < cap)
    }

    fn is_expired(&self, now: Instant, idle: Duration) -> bool {
        let usage = self.usage.lock().expect("usage");
        usage.leases == 0 && now.duration_since(usage.idle_since) >= idle
    }
}

impl<S> Drop for Conn<S> {
    fn drop(&mut self) {
        // before the state goes (Hysteria 2's HTTP/3 client ends its control
        // stream when dropped): the server sees the connection closed, not
        // a broken protocol
        close(&self.quic, self.no_error);
    }
}

/// Closes `connection` with `code`, unless it has ended already: quinn
/// would then record its own close in place of how the connection ended
/// (a stateless reset, the server's close) for every handle still on it.
pub(crate) fn close(connection: &Connection, code: VarInt) {
    if connection.close_reason().is_none() {
        connection.close(code, b"");
    }
}

/// Lets go of the connection once it takes no new requests, every half
/// `idle`, until the session is gone: it holds the session weakly.
fn spawn_reaper<P: Protocol>(shared: &Arc<Shared<P>>) -> AbortOnDropHandle<()> {
    let every = shared.idle / 2;
    let shared = Arc::downgrade(shared);
    AbortOnDropHandle::new(tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(shared) = shared.upgrade() else {
                return;
            };
            shared.prune(&mut shared.slot.lock().expect("session"), Instant::now());
        }
    }))
}

/// The latest dial's failure for the requests that waited for it.
fn copy_error(e: &OutboundError) -> OutboundError {
    match e {
        OutboundError::Reject(kind) => OutboundError::Reject(*kind),
        OutboundError::Unsupported(m) => OutboundError::Unsupported(m.clone()),
        OutboundError::Dns(m) => OutboundError::Dns(m.clone()),
        OutboundError::Io(e) => OutboundError::Io(io::Error::new(e.kind(), e.to_string())),
        OutboundError::Timeout => OutboundError::Timeout,
        OutboundError::Proxy(m) => OutboundError::Proxy(m.clone()),
        OutboundError::Tls(m) => OutboundError::Tls(m.clone()),
        OutboundError::Unavailable(m) => OutboundError::Unavailable(m.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ServerPort;
    use quinn::crypto::rustls::HandshakeData;
    use quinn::{ReadError, ReadToEndError, WriteError};
    use rurge_config::HostName;
    use rurge_config::spec::Sni;
    use rurge_net::connector::{BoxedDatagram, BoxedStream, Datagram, DirectConnector, Resolve};
    use rurge_net::packet_datagram::packet_datagram;
    use rurge_net::quic::testing::{TestCert, endpoint, loopback_socket};
    use std::net::IpAddr;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker};
    use tokio::io::ReadBuf;
    use tokio::sync::watch;
    use tokio::task::JoinSet;
    use tokio::time::timeout;

    const BOUND: Duration = Duration::from_secs(10);
    /// HTTP/3's H3_NO_ERROR, as Hysteria 2's.
    const NO_ERROR: VarInt = VarInt::from_u32(0x100);

    /// Answers every name with the loopback.
    struct Loopback;

    impl Resolve for Loopback {
        fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(std::future::ready(Ok(vec![IpAddr::from([127, 0, 0, 1])])))
        }
    }

    /// The protocol of these tests: a handshake that is one exchange with
    /// the server (as Hysteria 2's authentication) and fails while `refuse`
    /// is set, and a state the tests set to say the connection is going
    /// away. With `stall_first`, the first handshake never ends, as with a
    /// server that does not answer: it is told on `stalled` when it begins,
    /// and it ends only with its dial, when the caller gives up on that.
    /// While `held` says so, every handshake that begins waits, its dial in
    /// flight, until it is let go. The handshake `refuse_at` numbers, 0 for
    /// the first, is refused at once, held or not.
    #[derive(Default)]
    struct Plain {
        handshakes: AtomicUsize,
        refuse: AtomicBool,
        refuse_at: Option<usize>,
        stall_first: bool,
        stalled: Notify,
        held: watch::Sender<bool>,
    }

    impl Plain {
        fn handshakes(&self) -> usize {
            self.handshakes.load(Ordering::SeqCst)
        }
    }

    impl Protocol for Plain {
        /// Going away.
        type State = AtomicBool;

        const NAME: &'static str = "plain";

        const NO_ERROR: VarInt = NO_ERROR;

        fn handshake<'a>(
            &'a self,
            connection: &'a Connection,
        ) -> BoxFuture<'a, Result<AtomicBool, OutboundError>> {
            Box::pin(async move {
                let nth = self.handshakes.fetch_add(1, Ordering::SeqCst);
                if self.refuse_at == Some(nth) {
                    return Err(OutboundError::Proxy("plain: refused".into()));
                }
                if self.stall_first && nth == 0 {
                    self.stalled.notify_one();
                    std::future::pending::<()>().await;
                }
                let mut held = self.held.subscribe();
                let _ = held.wait_for(|held| !*held).await;
                let (mut send, mut recv) = connection
                    .open_bi()
                    .await
                    .map_err(|e| quic::connection_error(&e))?;
                send.write_all(b"hello").await.map_err(io::Error::from)?;
                let _ = send.finish();
                recv.read_to_end(64).await.map_err(io::Error::other)?;
                if self.refuse.load(Ordering::SeqCst) {
                    return Err(OutboundError::Proxy("plain: refused".into()));
                }
                Ok(AtomicBool::new(false))
            })
        }

        fn is_going_away(&self, state: &AtomicBool) -> bool {
            state.load(Ordering::SeqCst)
        }
    }

    /// The far end: endpoints on one `ServerPort` that echo each
    /// bidirectional stream once the client has finished its side, and keep
    /// the connections they accepted.
    struct Server {
        port: ServerPort,
        cert: TestCert,
        accepted: Arc<Mutex<Vec<Connection>>>,
        accepting: Mutex<Vec<AbortOnDropHandle<()>>>,
    }

    impl Server {
        fn spawn() -> Server {
            let server = Server {
                port: ServerPort::bind(),
                cert: TestCert::new(),
                accepted: Arc::default(),
                accepting: Mutex::default(),
            };
            server.restart();
            server
        }

        /// A new endpoint takes the port over, as after a restart: the
        /// connections of the one before are gone, and the client learns it
        /// from a stateless reset on its next packet.
        fn restart(&self) {
            let endpoint = self.port.endpoint(self.cert.server_config(&["h3"]));
            let accepted = self.accepted.clone();
            let task = tokio::spawn(async move {
                while let Some(incoming) = endpoint.accept().await {
                    let accepted = accepted.clone();
                    tokio::spawn(async move {
                        let Ok(connection) = incoming.await else {
                            return;
                        };
                        accepted.lock().unwrap().push(connection.clone());
                        while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                            tokio::spawn(async move {
                                if let Ok(data) = recv.read_to_end(1 << 16).await
                                    && send.write_all(&data).await.is_ok()
                                {
                                    let _ = send.finish();
                                }
                            });
                        }
                    });
                }
            });
            self.accepting
                .lock()
                .unwrap()
                .push(AbortOnDropHandle::new(task));
        }

        /// The connections accepted so far, once there are `count`.
        async fn accepted(&self, count: usize) -> Vec<Connection> {
            let deadline = Instant::now() + BOUND;
            loop {
                let accepted = self.accepted.lock().unwrap().clone();
                if accepted.len() >= count {
                    return accepted;
                }
                assert!(Instant::now() < deadline, "{} accepted", accepted.len());
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    }

    fn dialer(port: u16, host: &str, tls: &TlsOpts, roots: Arc<RootCertStore>) -> Dialer {
        let direct = Arc::new(DirectConnector::new(Arc::new(Loopback)));
        dialer_via(direct, port, host, tls, roots)
    }

    fn dialer_via(
        connector: Arc<dyn Connector>,
        port: u16,
        host: &str,
        tls: &TlsOpts,
        roots: Arc<RootCertStore>,
    ) -> Dialer {
        Dialer::new(
            connector,
            Target::new(HostName::parse(host), port),
            tls,
            &QuicOpts::default(),
            Tristate::Auto,
            &[],
            roots,
        )
        .expect("a dialer")
    }

    fn session(server: &Server, protocol: Plain) -> Session<Plain> {
        let dialer = dialer(
            server.port.addr().port(),
            "127.0.0.1",
            &TlsOpts::default(),
            server.cert.roots(),
        );
        Session::new("P", dialer, protocol)
    }

    /// The UDP flows of a chain to the server (`underlying-proxy`), one per
    /// connection, which the tests can cut.
    #[derive(Default)]
    struct Chain {
        flows: Mutex<Vec<Arc<Cut>>>,
    }

    #[derive(Default)]
    struct Cut {
        done: AtomicBool,
        reader: Mutex<Option<Waker>>,
    }

    impl Cut {
        fn now(&self) {
            self.done.store(true, Ordering::SeqCst);
            if let Some(reader) = self.reader.lock().unwrap().take() {
                reader.wake();
            }
        }
    }

    struct Flow(BoxedDatagram, Arc<Cut>);

    impl Datagram for Flow {
        fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            self.0.poll_send(cx, buf)
        }

        fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            *self.1.reader.lock().unwrap() = Some(cx.waker().clone());
            if self.1.done.load(Ordering::SeqCst) {
                return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
            }
            self.0.poll_recv(cx, buf)
        }
    }

    impl Connector for Chain {
        fn connect<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            Box::pin(std::future::ready(Err(io::Error::other("UDP only"))))
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            Box::pin(async move {
                let socket = DirectConnector::new(Arc::new(Loopback))
                    .open_udp(opts)
                    .await?;
                let to = socket.resolve(target).await?;
                let cut = Arc::<Cut>::default();
                self.flows.lock().unwrap().push(cut.clone());
                Ok(Box::new(Flow(packet_datagram(socket, to), cut)) as BoxedDatagram)
            })
        }
    }

    /// A stream's failure: the connection's end is `Lost`, anything else
    /// final.
    fn write_failure(e: WriteError) -> Failure {
        match e {
            WriteError::ConnectionLost(e) => Failure::lost(&e),
            e => Failure::Failed(OutboundError::Io(io::Error::other(e))),
        }
    }

    fn read_failure(e: ReadToEndError) -> Failure {
        match e {
            ReadToEndError::Read(ReadError::ConnectionLost(e)) => Failure::lost(&e),
            e => Failure::Failed(OutboundError::Io(io::Error::other(e))),
        }
    }

    /// The requests of these tests: `header` on a stream of its own, echoed
    /// back — the request's own exchange, as the head a proxy protocol
    /// sends before any of the caller's data. Nothing of the caller's has
    /// gone out: a connection found gone meanwhile is `Lost`.
    async fn exchange(lease: Lease<AtomicBool>, header: &[u8]) -> Result<Vec<u8>, Failure> {
        let (mut send, mut recv) = lease
            .connection()
            .open_bi()
            .await
            .map_err(|e| Failure::lost(&e))?;
        send.write_all(header).await.map_err(write_failure)?;
        let _ = send.finish();
        recv.read_to_end(1 << 16).await.map_err(read_failure)
    }

    async fn echo(
        session: &Session<Plain>,
        header: &'static [u8],
    ) -> Result<Vec<u8>, OutboundError> {
        let opts = ConnectOpts::default();
        timeout(
            BOUND,
            session.request(&opts, |lease| Box::pin(exchange(lease, header))),
        )
        .await
        .expect("an echo within the bound")
    }

    /// The client's handle on the connection requests go to now.
    async fn current(session: &Session<Plain>) -> Connection {
        let lease = session.lease(&ConnectOpts::default()).await.unwrap();
        lease.connection().clone()
    }

    /// Closed by the session: with the protocol's code, nothing else.
    async fn closed_by_us(connection: &Connection) {
        let closed = timeout(BOUND, connection.closed()).await.expect("closed");
        assert!(
            matches!(&closed, ConnectionError::ApplicationClosed(close)
                if close.error_code == NO_ERROR && close.reason.is_empty()),
            "{closed:?}"
        );
    }

    /// `session` with each connection carrying `cap` leases at most.
    fn capped(server: &Server, protocol: Plain, cap: usize) -> Session<Plain> {
        session(server, protocol).with_stream_cap(cap)
    }

    /// A lease on a connection of `session`, within the bound.
    async fn lease_on(session: &Session<Plain>) -> Lease<AtomicBool> {
        timeout(BOUND, session.lease(&ConnectOpts::default()))
            .await
            .expect("a lease within the bound")
            .unwrap()
    }

    /// A lease on a connection of `session`, asked for by a task of its
    /// own.
    fn spawn_lease(
        session: &Arc<Session<Plain>>,
    ) -> tokio::task::JoinHandle<Result<Lease<AtomicBool>, OutboundError>> {
        let session = session.clone();
        tokio::spawn(async move { session.lease(&ConnectOpts::default()).await })
    }

    /// The requests waiting for the dials in flight.
    fn waiters(session: &Session<Plain>) -> usize {
        session.shared.slot.lock().unwrap().waiting
    }

    /// The leases each connection that takes requests carries, the oldest
    /// first.
    fn carried(session: &Session<Plain>) -> Vec<usize> {
        let slot = session.shared.slot.lock().unwrap();
        slot.conns
            .iter()
            .map(|conn| conn.usage.lock().unwrap().leases)
            .collect()
    }

    /// Whether `lease` is on the connection at `n` among those that take
    /// requests, the oldest first.
    fn on(session: &Session<Plain>, lease: &Lease<AtomicBool>, n: usize) -> bool {
        Arc::ptr_eq(&lease.conn, &session.shared.slot.lock().unwrap().conns[n])
    }

    /// Polls `check` until it holds, within the bound.
    async fn until(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !check() {
            assert!(Instant::now() < deadline, "not within the bound");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn requests_that_come_together_share_one_dial() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        let (a, b, c) = tokio::join!(
            echo(&session, b"a"),
            echo(&session, b"b"),
            echo(&session, b"c")
        );
        assert_eq!(
            (a.unwrap(), b.unwrap(), c.unwrap()),
            (b"a".to_vec(), b"b".to_vec(), b"c".to_vec())
        );
        assert_eq!(session.shared.protocol.handshakes(), 1);
        // and the requests after them take the same connection
        assert_eq!(echo(&session, b"d").await.unwrap(), b"d");
        assert_eq!(session.shared.protocol.handshakes(), 1);
        assert_eq!(server.accepted(1).await.len(), 1);
    }

    #[tokio::test]
    async fn a_failed_handshake_fails_every_request_that_waited_and_the_next_dials_again() {
        let server = Server::spawn();
        let protocol = Plain {
            refuse: AtomicBool::new(true),
            ..Plain::default()
        };
        let session = session(&server, protocol);
        let (a, b, c) = tokio::join!(
            echo(&session, b"a"),
            echo(&session, b"b"),
            echo(&session, b"c")
        );
        for result in [a, b, c] {
            assert_eq!(result.unwrap_err().to_string(), "plain: refused");
        }
        assert_eq!(session.shared.protocol.handshakes(), 1);
        // the connection that was refused is closed
        closed_by_us(&server.accepted(1).await[0]).await;
        session
            .shared
            .protocol
            .refuse
            .store(false, Ordering::SeqCst);
        assert_eq!(echo(&session, b"d").await.unwrap(), b"d");
        assert_eq!(session.shared.protocol.handshakes(), 2);
    }

    /// The caller that is dialing gives up while the handshake of its dial
    /// stalls (its future is dropped, as by its timeout): a request that
    /// waited for that dial takes the dialing over — the dial is not left in
    /// flight for ever — and the session goes on with the connection that
    /// request made.
    #[tokio::test]
    async fn a_dial_its_caller_gave_up_on_is_taken_over_by_a_waiter() {
        let server = Server::spawn();
        let protocol = Plain {
            stall_first: true,
            ..Plain::default()
        };
        let session = session(&server, protocol);
        let opts = ConnectOpts::default();
        let stalled = &session.shared.protocol.stalled;
        let request = session.request(&opts, |lease| Box::pin(exchange(lease, b"a")));
        // A dials and its handshake never ends; B comes with it, finds the
        // dial in flight and waits for it. A's caller gives up as soon as the
        // handshake has begun to stall.
        let (a, b) = tokio::join!(
            async {
                tokio::select! {
                    result = request => Some(result),
                    () = stalled.notified() => None,
                }
            },
            echo(&session, b"b")
        );
        assert!(a.is_none(), "the request ended of its own: {a:?}");
        assert_eq!(b.unwrap(), b"b");
        // the dial that was given up on, and the one that took over
        assert_eq!(session.shared.protocol.handshakes(), 2);
        assert_eq!(server.accepted(2).await.len(), 2);
        // the session goes on with the connection of the second
        assert_eq!(echo(&session, b"c").await.unwrap(), b"c");
        assert_eq!(session.shared.protocol.handshakes(), 2);
    }

    #[tokio::test]
    async fn a_connection_the_server_closed_is_dialed_again() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        assert_eq!(echo(&session, b"a").await.unwrap(), b"a");
        let lease = session.lease(&ConnectOpts::default()).await.unwrap();
        server.accepted(1).await[0].close(VarInt::from_u32(7), b"bye");
        timeout(BOUND, lease.connection().closed())
            .await
            .expect("closed");
        assert!(!session.serves(&lease));
        drop(lease);
        assert_eq!(echo(&session, b"b").await.unwrap(), b"b");
        assert_eq!(session.shared.protocol.handshakes(), 2);
        assert_eq!(server.accepted(2).await.len(), 2);
    }

    /// Over a chain (`underlying-proxy`), a connection whose flow ends has
    /// lost its path, which quinn tells as a close of our own: the next
    /// request dials again, over a new flow.
    #[tokio::test]
    async fn a_connection_that_lost_its_path_is_dialed_again() {
        let server = Server::spawn();
        let chain = Arc::new(Chain::default());
        let dialer = dialer_via(
            chain.clone(),
            server.port.addr().port(),
            "127.0.0.1",
            &TlsOpts::default(),
            server.cert.roots(),
        );
        let session = Session::new("P", dialer, Plain::default());
        assert_eq!(echo(&session, b"a").await.unwrap(), b"a");
        let client = current(&session).await;
        chain.flows.lock().unwrap()[0].now();
        timeout(BOUND, client.closed()).await.expect("closed");
        assert_eq!(echo(&session, b"b").await.unwrap(), b"b");
        assert_eq!(session.shared.protocol.handshakes(), 2);
        assert_eq!(chain.flows.lock().unwrap().len(), 2);
    }

    /// A connection its state says is going away takes no new requests;
    /// the stream under way on it goes on, and when its lease goes the
    /// connection is closed.
    #[tokio::test]
    async fn a_connection_going_away_takes_no_new_requests_and_its_streams_go_on() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        let opts = ConnectOpts::default();
        let lease = session.lease(&opts).await.unwrap();
        let (mut send, mut recv) = lease.connection().open_bi().await.unwrap();
        send.write_all(b"under ").await.unwrap();
        assert!(session.serves(&lease));
        lease.state().store(true, Ordering::SeqCst);
        assert!(!session.serves(&lease));
        assert_eq!(echo(&session, b"new").await.unwrap(), b"new");
        assert_eq!(session.shared.protocol.handshakes(), 2);
        send.write_all(b"way").await.unwrap();
        send.finish().unwrap();
        let echoed = timeout(BOUND, recv.read_to_end(64)).await.unwrap().unwrap();
        assert_eq!(echoed, b"under way");
        drop((send, recv, lease));
        closed_by_us(&server.accepted(1).await[0]).await;
    }

    /// Dials through `inner`; where it leads is what `route` holds now, as
    /// a group that is the `underlying-proxy` leads to its member.
    struct Rerouted {
        inner: Arc<dyn Connector>,
        route: Mutex<Arc<u8>>,
    }

    impl Connector for Rerouted {
        fn connect<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            self.inner.connect(target, opts)
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            self.inner.connect_udp(target, opts)
        }

        fn route(&self) -> Option<Route> {
            Some(Route::of(&self.route.lock().unwrap()))
        }
    }

    /// The underlying policy leads elsewhere now (a group's member changed,
    /// a reload rebuilt it): the connection dialled the old way takes no new
    /// requests — not even once the policy leads back —, the stream under
    /// way on it goes on, and it is closed once its last lease goes; the
    /// next request dials the new way.
    #[tokio::test]
    async fn a_connection_whose_underlying_policy_changed_takes_no_new_requests() {
        let server = Server::spawn();
        let chain = Arc::new(Chain::default());
        let rerouted = Arc::new(Rerouted {
            inner: chain.clone(),
            route: Mutex::new(Arc::new(1)),
        });
        let dialer = dialer_via(
            rerouted.clone(),
            server.port.addr().port(),
            "127.0.0.1",
            &TlsOpts::default(),
            server.cert.roots(),
        );
        let session = Session::new("P", dialer, Plain::default());
        let lease = session.lease(&ConnectOpts::default()).await.unwrap();
        let (mut send, mut recv) = lease.connection().open_bi().await.unwrap();
        send.write_all(b"under ").await.unwrap();
        assert_eq!(echo(&session, b"same way").await.unwrap(), b"same way");
        assert!(session.serves(&lease));
        let first = rerouted.route.lock().unwrap().clone();
        *rerouted.route.lock().unwrap() = Arc::new(2);
        assert!(!session.serves(&lease));
        assert_eq!(echo(&session, b"new way").await.unwrap(), b"new way");
        assert_eq!(session.shared.protocol.handshakes(), 2);
        assert_eq!(chain.flows.lock().unwrap().len(), 2, "dialled anew");
        send.write_all(b"way").await.unwrap();
        send.finish().unwrap();
        let echoed = timeout(BOUND, recv.read_to_end(64)).await.unwrap().unwrap();
        assert_eq!(echoed, b"under way");
        // back the first way: a connection of its own again
        *rerouted.route.lock().unwrap() = first;
        assert!(!session.serves(&lease));
        assert_eq!(echo(&session, b"back").await.unwrap(), b"back");
        assert_eq!(session.shared.protocol.handshakes(), 3);
        drop((send, recv, lease));
        closed_by_us(&server.accepted(1).await[0]).await;
    }

    /// Held, a connection stays past the idle time; let go of, it is closed
    /// once the idle time has passed, and the next request dials again.
    #[tokio::test]
    async fn an_idle_connection_is_closed_once_no_lease_holds_it() {
        const IDLE: Duration = Duration::from_millis(400);
        let server = Server::spawn();
        let dialer = dialer(
            server.port.addr().port(),
            "127.0.0.1",
            &TlsOpts::default(),
            server.cert.roots(),
        );
        let session = Session::with_idle("P", dialer, Plain::default(), IDLE);
        let lease = session.lease(&ConnectOpts::default()).await.unwrap();
        tokio::time::sleep(IDLE * 3).await;
        let accepted = server.accepted(1).await;
        assert!(accepted[0].close_reason().is_none(), "closed while held");
        let client = lease.connection().clone();
        let since = Instant::now();
        drop(lease);
        closed_by_us(&accepted[0]).await;
        assert!(
            since.elapsed() >= IDLE,
            "closed after {:?}",
            since.elapsed()
        );
        assert!(matches!(
            client.close_reason(),
            Some(ConnectionError::LocallyClosed)
        ));
        assert_eq!(echo(&session, b"again").await.unwrap(), b"again");
        assert_eq!(session.shared.protocol.handshakes(), 2);
    }

    /// The server restarts — losing every connection — once the request
    /// has taken the connection: the request finds the connection gone (a
    /// stateless reset answers its first packet) before anything of the
    /// caller's went out, and goes again, once, on a new connection.
    #[tokio::test]
    async fn a_request_that_finds_its_connection_gone_goes_again_once_on_a_new_one() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        assert_eq!(echo(&session, b"first").await.unwrap(), b"first");
        let stale = current(&session).await;
        let attempts = AtomicUsize::new(0);
        let opts = ConnectOpts::default();
        let request = session.request(&opts, |lease| {
            let first = attempts.fetch_add(1, Ordering::SeqCst) == 0;
            let server = &server;
            Box::pin(async move {
                if first {
                    server.restart();
                }
                exchange(lease, b"second").await
            })
        });
        let echoed = timeout(BOUND, request).await.unwrap().unwrap();
        assert_eq!(echoed, b"second");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(session.shared.protocol.handshakes(), 2);
        assert!(
            matches!(stale.close_reason(), Some(ConnectionError::Reset)),
            "{:?}",
            stale.close_reason()
        );
    }

    /// Once the caller's data has gone out, a connection found gone is the
    /// request's end: it does not go again.
    #[tokio::test]
    async fn a_request_whose_data_went_out_does_not_go_again() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        assert_eq!(echo(&session, b"first").await.unwrap(), b"first");
        let attempts = AtomicUsize::new(0);
        let opts = ConnectOpts::default();
        let request = session.request(&opts, |lease| {
            attempts.fetch_add(1, Ordering::SeqCst);
            let server = &server;
            Box::pin(async move {
                let (mut send, mut recv) = lease
                    .connection()
                    .open_bi()
                    .await
                    .map_err(|e| Failure::lost(&e))?;
                send.write_all(b"the caller's")
                    .await
                    .map_err(write_failure)?;
                // from here on whatever happens is final
                server.restart();
                let _ = send.finish();
                recv.read_to_end(1 << 16)
                    .await
                    .map_err(|e| Failure::Failed(read_failure(e).into_error()))
            })
        });
        let err = timeout(BOUND, request).await.unwrap().unwrap_err();
        assert_eq!(
            err.to_string(),
            "quic: the server no longer knows the connection (stateless reset)"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(session.shared.protocol.handshakes(), 1);
        // the connection is gone: the next request dials
        assert_eq!(echo(&session, b"next").await.unwrap(), b"next");
        assert_eq!(session.shared.protocol.handshakes(), 2);
    }

    /// A connection dialed for the request is not stale: found gone, it is
    /// the request's end.
    #[tokio::test]
    async fn a_request_on_a_new_connection_found_gone_does_not_go_again() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        let attempts = AtomicUsize::new(0);
        let opts = ConnectOpts::default();
        let request = session.request(&opts, |lease| {
            attempts.fetch_add(1, Ordering::SeqCst);
            let server = &server;
            Box::pin(async move {
                server.restart();
                exchange(lease, b"lost").await
            })
        });
        let err = timeout(BOUND, request).await.unwrap().unwrap_err();
        assert_eq!(
            err.to_string(),
            "quic: the server no longer knows the connection (stateless reset)"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(session.shared.protocol.handshakes(), 1);
        assert_eq!(echo(&session, b"next").await.unwrap(), b"next");
        assert_eq!(session.shared.protocol.handshakes(), 2);
    }

    /// Dropped, the session ends its reaper and closes its connection; a
    /// lease outlives it, and the connection with it.
    #[tokio::test]
    async fn a_dropped_session_ends_its_task_and_its_connection_after_the_last_lease() {
        let server = Server::spawn();
        let session = session(&server, Plain::default());
        let lease = session.lease(&ConnectOpts::default()).await.unwrap();
        let reaper = session.reaper().expect("started by the request");
        drop(session);
        let deadline = Instant::now() + BOUND;
        while !reaper.is_finished() {
            assert!(Instant::now() < deadline, "the reaper still runs");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // the lease still has its connection
        let (mut send, mut recv) = lease.connection().open_bi().await.unwrap();
        send.write_all(b"still here").await.unwrap();
        send.finish().unwrap();
        let echoed = timeout(BOUND, recv.read_to_end(64)).await.unwrap().unwrap();
        assert_eq!(echoed, b"still here");
        drop(lease);
        closed_by_us(&server.accepted(1).await[0]).await;
    }

    /// The SNI and the ALPN the server sees.
    async fn seen(dialer: &Dialer, server: &quinn::Endpoint) -> (Option<String>, Option<Vec<u8>>) {
        let opts = ConnectOpts::default();
        let (client, accepted) = timeout(BOUND, async {
            tokio::join!(dialer.connect(&opts), async {
                server.accept().await.expect("a connection").await
            })
        })
        .await
        .expect("a handshake within the bound");
        client.expect("connected");
        let data = accepted
            .expect("accepted")
            .handshake_data()
            .expect("handshake data")
            .downcast::<HandshakeData>()
            .expect("rustls's");
        (data.server_name, data.protocol)
    }

    /// QUIC's TLS names the server as TLS over TCP does: no SNI for an
    /// address, unless `sni` names one; none with `sni=off`. ALPN is `h3`
    /// unless the policy names others.
    #[tokio::test]
    async fn the_dial_names_the_server_and_its_protocol_as_the_policy_says() {
        let cert = TestCert::new();
        let server = endpoint(cert.server_config(&["h3"]), loopback_socket());
        let port = server.local_addr().unwrap().port();
        let sni = |sni: Sni| TlsOpts {
            sni,
            ..TlsOpts::default()
        };
        for (host, tls, expected) in [
            ("127.0.0.1", TlsOpts::default(), None),
            ("localhost", TlsOpts::default(), Some("localhost")),
            (
                "127.0.0.1",
                sni(Sni::Name("localhost".into())),
                Some("localhost"),
            ),
            ("localhost", sni(Sni::Off), None),
        ] {
            let dialer = dialer(port, host, &tls, cert.roots());
            let (name, alpn) = seen(&dialer, &server).await;
            assert_eq!(name.as_deref(), expected, "{host} {:?}", tls.sni);
            assert_eq!(alpn.as_deref(), Some(&b"h3"[..]));
        }
        let own = TlsOpts {
            alpn: vec!["hq-29".into(), "h3".into()],
            ..TlsOpts::default()
        };
        let (_, alpn) = seen(&dialer(port, "localhost", &own, cert.roots()), &server).await;
        assert_eq!(alpn.as_deref(), Some(&b"h3"[..]));
    }

    /// The server's certificate is checked as the policy says.
    #[tokio::test]
    async fn the_dial_verifies_the_server_as_the_policy_says() {
        let cert = TestCert::new();
        let server = endpoint(cert.server_config(&["h3"]), loopback_socket());
        let port = server.local_addr().unwrap().port();
        let _accepting = AbortOnDropHandle::new(tokio::spawn(async move {
            while let Some(incoming) = server.accept().await {
                tokio::spawn(async move {
                    let _ = incoming.await;
                });
            }
        }));
        let opts = ConnectOpts::default();
        // trusting another certificate of the same names
        let untrusted = dialer(
            port,
            "localhost",
            &TlsOpts::default(),
            TestCert::new().roots(),
        );
        let err = timeout(BOUND, untrusted.connect(&opts))
            .await
            .unwrap()
            .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("tls: invalid peer certificate: "),
            "{err}"
        );
        let insecure = TlsOpts {
            skip_cert_verify: true,
            ..TlsOpts::default()
        };
        let roots = Arc::new(RootCertStore::empty());
        let dialer = dialer(port, "localhost", &insecure, roots);
        timeout(BOUND, dialer.connect(&opts))
            .await
            .unwrap()
            .unwrap();
    }

    /// Port hopping cannot go over an `underlying-proxy` (the manual): a
    /// connector that does not dial directly gets no dialer that hops — as
    /// a group's relay would make one (phase 2 M3 design 5.4).
    #[test]
    fn hopping_over_a_connector_that_does_not_dial_directly_is_refused() {
        let quic = QuicOpts {
            port_hopping: Some(PortHopping {
                ports: vec![5000..=6000],
                interval: Duration::from_secs(30),
            }),
        };
        let build = |connector: Arc<dyn Connector>| {
            Dialer::new(
                connector,
                Target::new(HostName::parse("127.0.0.1"), 443),
                &TlsOpts::default(),
                &quic,
                Tristate::Auto,
                &[],
                TestCert::new().roots(),
            )
            .map(|_| ())
        };
        let err = build(Arc::new(Chain::default())).unwrap_err();
        assert_eq!(
            err.message,
            "`port-hopping` cannot be combined with `underlying-proxy`"
        );
        build(Arc::new(DirectConnector::new(Arc::new(Loopback)))).expect("a direct one hops");
    }

    /// Under a cap of two, five requests at once: a dial is room for two of
    /// them, so three dials; the requests that waited fill the oldest
    /// connections first.
    #[tokio::test]
    async fn under_a_cap_requests_that_come_together_dial_the_connections_they_need() {
        let server = Server::spawn();
        let session = capped(&server, Plain::default(), 2);
        let opts = ConnectOpts::default();
        let (a, b, c, d, e) = timeout(BOUND, async {
            tokio::join!(
                session.lease(&opts),
                session.lease(&opts),
                session.lease(&opts),
                session.lease(&opts),
                session.lease(&opts)
            )
        })
        .await
        .expect("leases within the bound");
        let _leases = [a.unwrap(), b.unwrap(), c.unwrap(), d.unwrap(), e.unwrap()];
        assert_eq!(session.shared.protocol.handshakes(), 3);
        assert_eq!(server.accepted(3).await.len(), 3);
        assert_eq!(carried(&session), [2, 2, 1]);
    }

    /// Under a cap, a connection takes requests until it carries the cap,
    /// and the next one dials; a lease let go of frees its place, which the
    /// next request takes, the oldest connection with room first.
    #[tokio::test]
    async fn under_a_cap_a_place_let_go_of_is_taken_again_the_oldest_first() {
        let server = Server::spawn();
        let session = capped(&server, Plain::default(), 2);
        let mut leases = Vec::new();
        for _ in 0..5 {
            leases.push(lease_on(&session).await);
        }
        assert_eq!(carried(&session), [2, 2, 1]);
        assert_eq!(session.shared.protocol.handshakes(), 3);
        // a place freed on the newest and one on the oldest
        let newest = leases.iter().position(|l| on(&session, l, 2)).unwrap();
        drop(leases.remove(newest));
        let oldest = leases.iter().position(|l| on(&session, l, 0)).unwrap();
        drop(leases.remove(oldest));
        assert_eq!(carried(&session), [1, 2, 0]);
        let next = lease_on(&session).await;
        assert!(on(&session, &next, 0), "{:?}", carried(&session));
        leases.push(next);
        for _ in 0..2 {
            leases.push(lease_on(&session).await);
        }
        assert_eq!(carried(&session), [2, 2, 2]);
        assert_eq!(
            session.shared.protocol.handshakes(),
            3,
            "places taken again"
        );
        // every place taken: the next request dials
        leases.push(lease_on(&session).await);
        assert_eq!(carried(&session), [2, 2, 2, 1]);
        assert_eq!(session.shared.protocol.handshakes(), 4);
    }

    /// Under a cap, a dial in flight is room for `cap` requests — its own
    /// and `cap - 1` that wait for it —; a request that would not fit dials
    /// another, eight at most at once. Held at their handshakes, the dials
    /// stay in flight while the requests are counted.
    #[tokio::test]
    async fn under_a_cap_each_dial_is_room_for_cap_requests_and_eight_dial_at_most() {
        for (cap, requests, dials, connections) in [(3, 9, 3, 3), (3, 10, 4, 4), (1, 12, 8, 12)] {
            let server = Server::spawn();
            let protocol = Plain::default();
            protocol.held.send_replace(true);
            let session = Arc::new(capped(&server, protocol, cap));
            let mut leases = JoinSet::new();
            for _ in 0..requests {
                let session = session.clone();
                leases.spawn(async move { session.lease(&ConnectOpts::default()).await });
            }
            let waiting = requests - dials;
            until(|| session.shared.protocol.handshakes() == dials).await;
            until(|| session.shared.slot.lock().unwrap().waiting == waiting).await;
            // and no more while they are held
            tokio::time::sleep(Duration::from_millis(200)).await;
            let counted = {
                let slot = session.shared.slot.lock().unwrap();
                (slot.dialing, slot.waiting)
            };
            assert_eq!(counted, (dials, waiting), "cap {cap}, {requests} requests");
            assert_eq!(session.shared.protocol.handshakes(), dials);
            session.shared.protocol.held.send_replace(false);
            let leases = timeout(BOUND, leases.join_all())
                .await
                .expect("leases within the bound");
            assert!(leases.iter().all(Result::is_ok));
            let carried = carried(&session);
            assert_eq!(carried.len(), connections, "cap {cap}: {carried:?}");
            assert!(carried.iter().all(|&n| n <= cap), "cap {cap}: {carried:?}");
            assert_eq!(carried.iter().sum::<usize>(), requests);
        }
    }

    /// Under a cap, the requests that waited for a dial share its failure;
    /// the next request dials again.
    #[tokio::test]
    async fn under_a_cap_a_failed_dial_fails_the_requests_that_waited_for_it() {
        let server = Server::spawn();
        let protocol = Plain {
            refuse: AtomicBool::new(true),
            ..Plain::default()
        };
        let session = capped(&server, protocol, 3);
        let (a, b, c) = tokio::join!(
            echo(&session, b"a"),
            echo(&session, b"b"),
            echo(&session, b"c")
        );
        for result in [a, b, c] {
            assert_eq!(result.unwrap_err().to_string(), "plain: refused");
        }
        // one dial was room for the three
        assert_eq!(session.shared.protocol.handshakes(), 1);
        session
            .shared
            .protocol
            .refuse
            .store(false, Ordering::SeqCst);
        assert_eq!(echo(&session, b"d").await.unwrap(), b"d");
        assert_eq!(session.shared.protocol.handshakes(), 2);
    }

    /// Under a cap, a connection going away takes no new requests, room or
    /// not; the stream under way on it goes on, and when its lease goes the
    /// connection is closed.
    #[tokio::test]
    async fn under_a_cap_a_connection_going_away_takes_no_new_requests() {
        let server = Server::spawn();
        let session = capped(&server, Plain::default(), 2);
        let lease = lease_on(&session).await;
        let (mut send, mut recv) = timeout(BOUND, lease.connection().open_bi())
            .await
            .expect("a stream within the bound")
            .unwrap();
        timeout(BOUND, send.write_all(b"under "))
            .await
            .expect("written within the bound")
            .unwrap();
        lease.state().store(true, Ordering::SeqCst);
        let new = lease_on(&session).await;
        assert_ne!(new.connection().stable_id(), lease.connection().stable_id());
        assert_eq!(carried(&session), [1]);
        assert_eq!(session.shared.protocol.handshakes(), 2);
        timeout(BOUND, send.write_all(b"way"))
            .await
            .expect("written within the bound")
            .unwrap();
        send.finish().unwrap();
        let echoed = timeout(BOUND, recv.read_to_end(64)).await.unwrap().unwrap();
        assert_eq!(echoed, b"under way");
        drop((send, recv, lease));
        closed_by_us(&server.accepted(1).await[0]).await;
        assert!(new.connection().close_reason().is_none());
    }

    /// Under a cap, every connection dialled while the connector led
    /// elsewhere is let go of, room or not; the next request dials the new
    /// way.
    #[tokio::test]
    async fn under_a_cap_connections_dialled_the_old_way_take_no_new_requests() {
        let server = Server::spawn();
        let chain = Arc::new(Chain::default());
        let rerouted = Arc::new(Rerouted {
            inner: chain.clone(),
            route: Mutex::new(Arc::new(1)),
        });
        let dialer = dialer_via(
            rerouted.clone(),
            server.port.addr().port(),
            "127.0.0.1",
            &TlsOpts::default(),
            server.cert.roots(),
        );
        let session = Session::new("P", dialer, Plain::default()).with_stream_cap(2);
        let mut leases = Vec::new();
        for _ in 0..3 {
            leases.push(lease_on(&session).await);
        }
        assert_eq!(carried(&session), [2, 1]);
        *rerouted.route.lock().unwrap() = Arc::new(2);
        leases.push(lease_on(&session).await);
        assert_eq!(carried(&session), [1]);
        assert_eq!(chain.flows.lock().unwrap().len(), 3, "dialled anew");
        assert_eq!(session.shared.protocol.handshakes(), 3);
        assert!(leases[..3].iter().all(|lease| !session.serves(lease)));
    }

    /// Under a cap, each connection idles on its own: the one let go of is
    /// closed after the idle time while another, held, stays; let go of in
    /// turn, that one follows.
    #[tokio::test]
    async fn under_a_cap_idle_connections_are_closed_one_by_one() {
        const IDLE: Duration = Duration::from_millis(400);
        let server = Server::spawn();
        let dialer = dialer(
            server.port.addr().port(),
            "127.0.0.1",
            &TlsOpts::default(),
            server.cert.roots(),
        );
        let session = Session::with_idle("P", dialer, Plain::default(), IDLE).with_stream_cap(1);
        let first = lease_on(&session).await;
        let second = lease_on(&session).await;
        let accepted = server.accepted(2).await;
        for (lease, n) in [(first, 0), (second, 1)] {
            let since = Instant::now();
            drop(lease);
            closed_by_us(&accepted[n]).await;
            assert!(
                since.elapsed() >= IDLE,
                "closed after {:?}",
                since.elapsed()
            );
            // the one still held stays
            assert!(accepted[n + 1..].iter().all(|c| c.close_reason().is_none()));
        }
        assert!(carried(&session).is_empty());
        assert_eq!(echo(&session, b"again").await.unwrap(), b"again");
        assert_eq!(session.shared.protocol.handshakes(), 3);
    }

    /// Under a cap, the server restarts while a request is on the newer of
    /// two connections, and a place frees up on the older. The request,
    /// finding its connection gone, goes again on one dialed after it — a
    /// new one: the older, with room again, is as stale.
    #[tokio::test]
    async fn under_a_cap_a_request_goes_again_on_a_connection_dialed_after_the_one_it_lost() {
        let server = Server::spawn();
        let session = capped(&server, Plain::default(), 1);
        let older = lease_on(&session).await;
        drop(lease_on(&session).await);
        assert_eq!(carried(&session), [1, 0]);
        let older = Mutex::new(Some(older));
        let attempts = AtomicUsize::new(0);
        let opts = ConnectOpts::default();
        let request = session.request(&opts, |lease| {
            let first = attempts.fetch_add(1, Ordering::SeqCst) == 0;
            let (server, older) = (&server, &older);
            Box::pin(async move {
                if first {
                    drop(older.lock().unwrap().take());
                    server.restart();
                }
                exchange(lease, b"again").await
            })
        });
        let echoed = timeout(BOUND, request)
            .await
            .expect("an outcome within the bound")
            .unwrap();
        assert_eq!(echoed, b"again");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(session.shared.protocol.handshakes(), 3);
    }

    /// Under a cap, a request that waits for a dial in flight is woken by
    /// its end and takes the older connection, where a place freed up
    /// meanwhile: one that was up when the request came. Found gone, it is
    /// as stale as one taken at once: the request goes again, on the
    /// connection the dial made.
    #[tokio::test]
    async fn under_a_cap_a_waiter_woken_onto_a_stale_connection_goes_again() {
        let server = Server::spawn();
        let session = Arc::new(capped(&server, Plain::default(), 2));
        let freed = lease_on(&session).await;
        let _kept = lease_on(&session).await;
        assert_eq!(carried(&session), [2]);
        // a dial held at its handshake: room for the request, which waits
        session.shared.protocol.held.send_replace(true);
        let dialing = spawn_lease(&session);
        until(|| session.shared.protocol.handshakes() == 2).await;
        let attempts = AtomicUsize::new(0);
        let opts = ConnectOpts::default();
        let request = session.request(&opts, |lease| {
            let first = attempts.fetch_add(1, Ordering::SeqCst) == 0;
            let server = &server;
            Box::pin(async move {
                if first {
                    // the older connection goes, as one the server dropped
                    server.accepted(1).await[0].close(VarInt::from_u32(7), b"bye");
                }
                exchange(lease, b"woken").await
            })
        });
        let wake = async {
            until(|| waiters(&session) == 1).await;
            // a place frees up on the older connection; then the dial ends
            drop(freed);
            session.shared.protocol.held.send_replace(false);
        };
        let (echoed, ()) = timeout(BOUND, async { tokio::join!(request, wake) })
            .await
            .expect("an outcome within the bound");
        assert_eq!(echoed.unwrap(), b"woken");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            session.shared.protocol.handshakes(),
            2,
            "no dial of its own"
        );
        timeout(BOUND, dialing)
            .await
            .expect("a lease within the bound")
            .unwrap()
            .unwrap();
    }

    /// Under a cap, a request that waits and is given up on — its future
    /// dropped, as by its caller's timeout — counts no more: the next one
    /// waits for the dial in flight, as it would have, rather than dialing
    /// another.
    #[tokio::test]
    async fn under_a_cap_a_waiter_given_up_on_counts_no_more() {
        let server = Server::spawn();
        let protocol = Plain::default();
        protocol.held.send_replace(true);
        let session = Arc::new(capped(&server, protocol, 2));
        // a dial held at its handshake, room for one request more
        let dialing = spawn_lease(&session);
        until(|| session.shared.protocol.handshakes() == 1).await;
        let given_up = spawn_lease(&session);
        until(|| waiters(&session) == 1).await;
        given_up.abort();
        let ended = timeout(BOUND, given_up)
            .await
            .expect("ended within the bound");
        assert!(ended.is_err_and(|e| e.is_cancelled()));
        assert_eq!(waiters(&session), 0);
        let next = spawn_lease(&session);
        until(|| waiters(&session) == 1).await;
        // and no second dial while the first is held
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(session.shared.protocol.handshakes(), 1);
        session.shared.protocol.held.send_replace(false);
        let mut leases = Vec::new();
        for lease in [dialing, next] {
            let lease = timeout(BOUND, lease)
                .await
                .expect("a lease within the bound");
            leases.push(lease.unwrap().unwrap());
        }
        assert_eq!(carried(&session), [2]);
        assert_eq!(session.shared.protocol.handshakes(), 1);
    }

    /// Under a cap, a dial that fails while another is in flight fails its
    /// own request alone: the request that waited looks again — the dial
    /// still in flight has room for it — and takes its place on the
    /// connection that one makes.
    #[tokio::test]
    async fn under_a_cap_a_dial_that_fails_while_another_is_in_flight_fails_no_waiter() {
        let server = Server::spawn();
        let protocol = Plain {
            refuse_at: Some(1),
            ..Plain::default()
        };
        protocol.held.send_replace(true);
        let session = Arc::new(capped(&server, protocol, 2));
        // the first dial held at its handshake; the request after it waits
        let dialing = spawn_lease(&session);
        until(|| session.shared.protocol.handshakes() == 1).await;
        let waiter = spawn_lease(&session);
        until(|| waiters(&session) == 1).await;
        // a third would not fit: it dials, and its dial is refused
        let err = timeout(BOUND, session.lease(&ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .err()
            .expect("refused");
        assert_eq!(err.to_string(), "plain: refused");
        assert_eq!(session.shared.protocol.handshakes(), 2);
        // the first dial goes through: the request that waited has its place
        session.shared.protocol.held.send_replace(false);
        let mut leases = Vec::new();
        for lease in [dialing, waiter] {
            let lease = timeout(BOUND, lease)
                .await
                .expect("a lease within the bound");
            leases.push(lease.unwrap().unwrap());
        }
        assert_eq!(carried(&session), [2]);
        assert_eq!(session.shared.protocol.handshakes(), 2);
    }
}
