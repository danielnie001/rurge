//! The QUIC connection of a DNS server over QUIC or HTTP/3 (DoQ, DoH3),
//! shared by its queries — the rules of a policy's QUIC session (phase 2
//! M7 design 3.5), as DNS needs them:
//!
//! - The connection is dialed when a query first needs one; the queries
//!   that come meanwhile wait for that dial and share its failure. A dial
//!   whose query gave up on it (its future dropped) is taken over by one of
//!   those that wait.
//! - A dial tries the server's addresses in turn — a name's from the
//!   bootstrap, IPv4 first — until one connects, each on a path opened on
//!   the resolver's connector to the address itself: a direct path asks no
//!   resolver. Each gets its share of the time left, what is left divided
//!   by the addresses that are: one that never answers — QUIC has no
//!   refusal to fail it quickly — cannot take it all from those after it.
//! - A connection that has closed — its idle timeout, the server's close, a
//!   protocol error — or that is going away (an HTTP/3 GOAWAY) takes no new
//!   query: the next one dials. Nothing keeps a connection alive
//!   (`Transport::DNS`): unused, it idles out.
//! - A query on a connection that was up when it came, which finds the
//!   connection gone (`Failure::Lost`) — a server that dropped it while it
//!   idled answers with a stateless reset, an HTTP/3 server may reject the
//!   request unprocessed (H3_REQUEST_REJECTED) —, goes again, once, on a
//!   new connection: a DNS query can be asked again. The connection found
//!   gone takes no new query.
//! - A connection that went dead without a word — a server restarted
//!   without the key of its stateless resets, a path that lets nothing
//!   through — is found out by the query that ends without an answer — its
//!   deadline passed, or it was dropped — after its data went out, with
//!   nothing received on the connection since (`Progress`). It takes no new
//!   query either, instead of every query to the upstream timing out until
//!   the connection's idle timeout.
//! - The caller's deadline bounds a query: the dial, the handshake and the
//!   exchange.

use super::UpstreamError;
use crate::bootstrap::Bootstrap;
use quinn::{Connection, ConnectionError, VarInt};
use rurge_config::HostName;
use rurge_config::spec::Tristate;
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, Connector, Target, interleave};
use rurge_net::http::quic_tls_client_config;
use rurge_net::quic::{self, Path, Transport};
use std::io;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

/// What a peer may take, beyond a round trip, to acknowledge a packet: the
/// default of its `max_ack_delay` (RFC 9000 18.2).
const MAX_ACK_DELAY: Duration = Duration::from_millis(25);

/// The client configuration of the DNS servers over QUIC that speak `alpn`
/// (`doq`, `h3`): their certificates verified with the shared roots, or not
/// at all (`encrypted-dns-skip-cert-verification`), on TLS 1.3, with
/// `Transport::DNS`.
pub(super) fn client_config(skip_verify: bool, alpn: &str) -> io::Result<quinn::ClientConfig> {
    let tls = quic_tls_client_config(skip_verify, alpn).map_err(io::Error::other)?;
    quic::client_config(tls, &Transport::DNS)
}

/// A failure of QUIC, or of what runs over it, as the upstream's error, by
/// the layer its text names (phase 2 M7 design 3.6): a failed handshake's
/// (`tls: …`) is TLS's, HTTP/3's (`h3: …`) HTTP's, any other — QUIC's
/// (`quic: …`), the path's — I/O.
pub(super) fn transport_error(e: io::Error) -> UpstreamError {
    let text = e.to_string();
    if let Some(tls) = text.strip_prefix("tls: ") {
        UpstreamError::Tls(tls.to_string())
    } else if text.starts_with("h3: ") {
        UpstreamError::Http(text)
    } else {
        UpstreamError::Io(text)
    }
}

/// How a DNS server's QUIC connections are made.
pub(super) struct Dialer {
    /// The upstream's name, for the log.
    name: String,
    /// The server as configured — a name, or an address without brackets
    /// —: what the bootstrap resolves, and what TLS verifies and sends as
    /// SNI (none for an address).
    host: String,
    port: u16,
    connector: Arc<dyn Connector>,
    bootstrap: Arc<Bootstrap>,
    config: quinn::ClientConfig,
}

impl Dialer {
    /// The paths to `host` are opened on `connector`, its name resolved
    /// through `bootstrap`.
    pub(super) fn new(
        name: String,
        host: &str,
        port: u16,
        connector: Arc<dyn Connector>,
        bootstrap: Arc<Bootstrap>,
        config: quinn::ClientConfig,
    ) -> Dialer {
        Dialer {
            name,
            host: host.to_string(),
            port,
            connector,
            bootstrap,
            config,
        }
    }

    /// A connection to the server: its addresses in turn, IPv4 first, until
    /// one connects, each with its share of the time left until `deadline`
    /// — what is left divided by the addresses that are —, the last with
    /// all of it. The query's deadline bounds it all the same.
    async fn connect(&self, deadline: Instant) -> Result<Connection, UpstreamError> {
        let addresses = interleave(self.bootstrap.resolve(&self.host).await?, false);
        let mut last = UpstreamError::Bootstrap(format!("no addresses for {}", self.host));
        let count = addresses.len();
        for (done, address) in addresses.into_iter().enumerate() {
            let left = u32::try_from(count - done).unwrap_or(u32::MAX);
            let share = deadline.saturating_duration_since(Instant::now()) / left;
            let server = Target::new(HostName::Ip(address), self.port);
            match tokio::time::timeout(share, self.connect_to(&server, share)).await {
                Ok(Ok(connection)) => return Ok(connection),
                Ok(Err(e)) => {
                    tracing::debug!(upstream = %self.name, %address, error = %e, "cannot connect to the server at this address");
                    last = transport_error(e);
                }
                Err(_) => {
                    tracing::debug!(upstream = %self.name, %address, "the server did not answer at this address in its share of the time");
                    last = UpstreamError::Timeout;
                }
            }
        }
        Err(last)
    }

    /// A connection to `server`; `budget` bounds the opening of the path
    /// only.
    async fn connect_to(&self, server: &Target, budget: Duration) -> io::Result<Connection> {
        let opts = ConnectOpts { timeout: budget };
        let path = Path::open(self.connector.as_ref(), server, None, Tristate::Auto, &opts).await?;
        quic::connect(path, &self.config, &self.host).await
    }
}

/// What a protocol over QUIC keeps for each connection, and how it lets one
/// go.
pub(super) trait Protocol: Send + Sync + 'static {
    /// One connection's: what its queries share (DoH3: its HTTP/3 client,
    /// which must live as long as the connection).
    type State: Send + Sync + 'static;

    /// The application error code a connection is closed with when it is
    /// let go of: DOQ_NO_ERROR, H3_NO_ERROR.
    const NO_ERROR: VarInt;

    /// What a new connection starts with, once QUIC's handshake is done.
    fn start(&self, connection: &Connection) -> Self::State;

    /// Whether `state` says the connection takes no new query (an HTTP/3
    /// GOAWAY); the queries under way on it go on.
    fn is_going_away(&self, state: &Self::State) -> bool {
        let _ = state;
        false
    }
}

/// One connection and what its protocol keeps for it. The session holds it
/// while it takes queries, each query while it runs; when the last lets go,
/// the connection is closed.
pub(super) struct Conn<S> {
    pub(super) quic: Connection,
    pub(super) state: S,
    /// `Protocol::NO_ERROR`.
    no_error: VarInt,
}

impl<S> Conn<S> {
    /// Closes the connection with `code` and `reason`, unless it has ended
    /// already: quinn would put a close of ours in place of how it ended (a
    /// stateless reset, the server's close).
    pub(super) fn close(&self, code: VarInt, reason: &str) {
        if self.quic.close_reason().is_none() {
            self.quic.close(code, reason.as_bytes());
        }
    }
}

impl<S> Drop for Conn<S> {
    fn drop(&mut self) {
        // before the state goes (DoH3's HTTP/3 client ends its control
        // stream when dropped): the server sees the connection closed, not
        // a broken protocol
        self.close(self.no_error, "");
    }
}

/// How a query on one of the session's connections failed.
#[derive(Debug)]
pub(super) enum Failure {
    /// The connection ended under the query, or is going away: on another
    /// connection the query may yet get its answer.
    Lost(UpstreamError),
    /// Final.
    Failed(UpstreamError),
}

impl Failure {
    /// The connection's end, met by a query.
    pub(super) fn lost(e: &ConnectionError) -> Failure {
        Failure::Lost(transport_error(quic::connection_error(e)))
    }

    fn into_error(self) -> UpstreamError {
        match self {
            Failure::Lost(e) | Failure::Failed(e) => e,
        }
    }
}

/// What a query tells the session of itself on a connection: when its data
/// was written, and how many datagrams the connection had received by then.
/// A connection that went dead without a word — nothing keeps it alive,
/// nothing tells that it is gone — shows it so: a query ends on it without
/// an answer (`Unanswered`), and it has received nothing since the query's
/// data went out.
#[derive(Clone, Default)]
pub(super) struct Progress(Arc<Mutex<Option<Written>>>);

#[derive(Clone, Copy)]
struct Written {
    at: Instant,
    received: u64,
}

impl Progress {
    /// The query's data — all of it, to the FIN — is written on
    /// `connection`.
    pub(super) fn written(&self, connection: &Connection) {
        *self.0.lock().expect("progress") = Some(Written {
            at: Instant::now(),
            received: datagrams_received(connection),
        });
    }

    /// Whether `connection` has received nothing since the data was
    /// written, though long enough for a live one to have answered, if only
    /// with an acknowledgement: two round trips and the ack delay of its
    /// peer — the second lets a packet be lost. Never before the data went
    /// out.
    fn quiet(&self, connection: &Connection) -> bool {
        let Some(written) = *self.0.lock().expect("progress") else {
            return false;
        };
        written.at.elapsed() >= connection.rtt() * 2 + MAX_ACK_DELAY
            && datagrams_received(connection) == written.received
    }
}

/// How many datagrams `connection` has taken in so far, whatever they
/// carry: an acknowledgement is one.
fn datagrams_received(connection: &Connection) -> u64 {
    connection.stats().udp_rx.datagrams
}

/// A DNS server's QUIC connection, shared by its queries.
pub(super) struct Session<P: Protocol> {
    protocol: P,
    dialer: Dialer,
    slot: Mutex<Slot<P::State>>,
    /// A dial ended, or its query gave up on it: the queries waiting for it
    /// look again.
    dialed: Notify,
}

/// The session's connection and its dial.
struct Slot<S> {
    /// The connection new queries go to.
    current: Option<Arc<Conn<S>>>,
    /// A query is dialing.
    dialing: bool,
    /// Dials that have ended, success or failure.
    dials: u64,
    /// The latest dial's failure and the `dials` it ended as; `None` after
    /// a success.
    failure: Option<(u64, UpstreamError)>,
}

impl<P: Protocol> Session<P> {
    /// Dials nothing yet: the first query does.
    pub(super) fn new(protocol: P, dialer: Dialer) -> Session<P> {
        Session {
            protocol,
            dialer,
            slot: Mutex::new(Slot {
                current: None,
                dialing: false,
                dials: 0,
                failure: None,
            }),
            dialed: Notify::new(),
        }
    }

    /// The upstream's name.
    pub(super) fn name(&self) -> &str {
        &self.dialer.name
    }

    /// Runs `attempt` on the session's connection, all of it by `deadline`;
    /// it tells its `Progress` when its data is written. A query that finds
    /// a connection it took at once gone (`Failure::Lost`) is attempted
    /// again, once, on a new connection; on a connection it waited for, it
    /// fails. The connection found gone takes no new query either way.
    pub(super) async fn query<'a>(
        &'a self,
        deadline: Instant,
        attempt: impl Fn(Arc<Conn<P::State>>, Progress) -> BoxFuture<'a, Result<Vec<u8>, Failure>>,
    ) -> Result<Vec<u8>, UpstreamError> {
        let query = async {
            let (conn, taken) = self.take(deadline).await?;
            let e = match self.attempt(&attempt, conn.clone()).await {
                Err(Failure::Lost(e)) => e,
                done => return done.map_err(Failure::into_error),
            };
            self.forget(&conn);
            drop(conn);
            if !taken {
                return Err(e);
            }
            // the error says its layer (`quic: …`, `h3: …`)
            tracing::debug!(upstream = %self.dialer.name, "{e}; the query goes again on a new connection");
            let (conn, _) = self.take(deadline).await?;
            self.attempt(&attempt, conn)
                .await
                .map_err(Failure::into_error)
        };
        tokio::time::timeout_at(deadline, query)
            .await
            .unwrap_or(Err(UpstreamError::Timeout))
    }

    /// One attempt of a query on `conn`. An attempt that ends without an
    /// answer — it failed, its deadline passed, its future was dropped —
    /// and finds `conn` quiet since its data went out (`Progress::quiet`)
    /// is the end of that connection's service.
    async fn attempt<'a>(
        &'a self,
        attempt: &impl Fn(Arc<Conn<P::State>>, Progress) -> BoxFuture<'a, Result<Vec<u8>, Failure>>,
        conn: Arc<Conn<P::State>>,
    ) -> Result<Vec<u8>, Failure> {
        let progress = Progress::default();
        let mut unanswered = Unanswered {
            session: self,
            conn: conn.clone(),
            progress: progress.clone(),
            answered: false,
        };
        let result = attempt(conn, progress).await;
        unanswered.answered = result.is_ok();
        result
    }

    /// `conn` takes no new query from now on, unless it is not the
    /// session's connection any more: the queries on it run to their end,
    /// and it closes when the last lets go.
    fn forget(&self, conn: &Arc<Conn<P::State>>) {
        let mut slot = self.slot.lock().expect("session");
        if slot
            .current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, conn))
        {
            slot.current = None;
        }
    }

    /// The connection a query goes to, dialing one when there is none; and
    /// whether the query took it at once — one that was up when it came —,
    /// rather than after a dial of its own or of another it waited for.
    async fn take(&self, deadline: Instant) -> Result<(Arc<Conn<P::State>>, bool), UpstreamError> {
        // the dials that had ended when this query began to wait
        let mut waited_since = None;
        loop {
            // registered before the look: a dial that ends after it wakes us
            let dialed = self.dialed.notified();
            let mut dialed = pin!(dialed);
            dialed.as_mut().enable();
            {
                let mut slot = self.slot.lock().expect("session");
                if let Some(conn) = &slot.current
                    && conn.quic.close_reason().is_none()
                    && !self.protocol.is_going_away(&conn.state)
                {
                    return Ok((conn.clone(), waited_since.is_none()));
                }
                // closed or going away: the queries under way keep it
                slot.current = None;
                if let Some(since) = waited_since
                    && !slot.dialing
                    && let Some((at, failure)) = &slot.failure
                    && *at > since
                {
                    return Err(failure.clone());
                }
                if !slot.dialing {
                    slot.dialing = true;
                    break;
                }
                waited_since = Some(slot.dials);
            }
            dialed.await;
        }
        self.dial(deadline).await.map(|conn| (conn, false))
    }

    /// A new connection, which becomes the session's. The dial stays in
    /// flight until it ends or its query gives up on it.
    async fn dial(&self, deadline: Instant) -> Result<Arc<Conn<P::State>>, UpstreamError> {
        let dialing = Dialing(self);
        let dialed = self.dialer.connect(deadline).await.map(|quic| {
            Arc::new(Conn {
                state: self.protocol.start(&quic),
                quic,
                no_error: P::NO_ERROR,
            })
        });
        {
            let mut slot = self.slot.lock().expect("session");
            slot.dials += 1;
            match &dialed {
                Ok(conn) => {
                    slot.current = Some(conn.clone());
                    slot.failure = None;
                }
                Err(e) => slot.failure = Some((slot.dials, e.clone())),
            }
        }
        drop(dialing);
        dialed
    }
}

/// An attempt of a query on `conn`, until it has its answer. Dropped without
/// one — it failed, its deadline passed, or its future was dropped, the
/// fan-out having no use for the upstream any more — it finds out whether
/// `conn` went dead (`Progress::quiet`), and then `conn` takes no new query.
struct Unanswered<'a, P: Protocol> {
    session: &'a Session<P>,
    conn: Arc<Conn<P::State>>,
    progress: Progress,
    answered: bool,
}

impl<P: Protocol> Drop for Unanswered<'_, P> {
    fn drop(&mut self) {
        if !self.answered && self.progress.quiet(&self.conn.quic) {
            tracing::debug!(upstream = %self.session.dialer.name, "the connection received nothing since a query went out on it; the next query dials a new one");
            self.session.forget(&self.conn);
        }
    }
}

/// A dial in flight; when it ends — or its query gives up on it —, the
/// queries waiting for it look again.
struct Dialing<'a, P: Protocol>(&'a Session<P>);

impl<P: Protocol> Drop for Dialing<'_, P> {
    fn drop(&mut self) {
        self.0.slot.lock().expect("session").dialing = false;
        self.0.dialed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fanout::FanoutOpts;
    use crate::testing::{DoqScript, FakeDoq, MockDns};
    use rurge_net::connector::{BoxedDatagram, BoxedStream, DirectConnector, SystemResolve};
    use std::future::{Future, pending, poll_fn};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::Poll;
    use std::time::Duration;
    use tokio::sync::watch;
    use tokio::time::timeout;

    const BOUND: Duration = Duration::from_secs(10);
    /// Long enough for a task that can go on to have gone on.
    const SETTLE: Duration = Duration::from_millis(100);

    /// A protocol that keeps nothing: what runs on a connection is up to
    /// the test.
    struct Plain;

    impl Protocol for Plain {
        type State = ();
        const NO_ERROR: VarInt = VarInt::from_u32(0);

        fn start(&self, _connection: &Connection) {}
    }

    /// A direct connector's UDP flows, which it opens only once it is let
    /// to; it counts the flows it was asked for, and refuses them while it
    /// is told to.
    struct Gated {
        inner: DirectConnector,
        asked: AtomicUsize,
        gate: watch::Sender<bool>,
        refusing: AtomicBool,
    }

    impl Gated {
        fn new() -> Arc<Gated> {
            Arc::new(Gated {
                inner: DirectConnector::new(Arc::new(SystemResolve)),
                asked: AtomicUsize::new(0),
                gate: watch::channel(false).0,
                refusing: AtomicBool::new(false),
            })
        }

        fn asked(&self) -> usize {
            self.asked.load(Ordering::SeqCst)
        }

        fn open(&self) {
            self.gate.send_replace(true);
        }

        fn refuse(&self, refusing: bool) {
            self.refusing.store(refusing, Ordering::SeqCst);
        }

        /// Opens the gate once the first flow was asked for.
        async fn open_once_asked(&self) {
            while self.asked() == 0 {
                tokio::task::yield_now().await;
            }
            self.open();
        }
    }

    impl Connector for Gated {
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
            Box::pin(async move {
                self.asked.fetch_add(1, Ordering::SeqCst);
                self.gate
                    .subscribe()
                    .wait_for(|open| *open)
                    .await
                    .map_err(io::Error::other)?;
                if self.refusing.load(Ordering::SeqCst) {
                    return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "refused"));
                }
                let datagram = self.inner.connect_udp(target, opts).await?;
                // a flow not yet known to be writable drops what the carrier
                // first sends on it — the carrier does not wait, and the
                // connection starts a second late, after QUIC's first
                // retransmission —: ready it with an empty datagram, which
                // the server takes for noise
                poll_fn(|cx| datagram.poll_send(cx, &[])).await?;
                Ok(datagram)
            })
        }
    }

    /// A session to `fake` through a gated connector.
    fn session(fake: &FakeDoq) -> (Session<Plain>, Arc<Gated>) {
        let gated = Gated::new();
        let dialer = Dialer::new(
            "quic://test".to_string(),
            "127.0.0.1",
            fake.addr().port(),
            gated.clone(),
            Bootstrap::new(Vec::new(), false, FanoutOpts::default()),
            client_config(true, "doq").unwrap(),
        );
        (Session::new(Plain, dialer), gated)
    }

    fn far() -> Instant {
        Instant::now() + BOUND
    }

    /// Waits until `check` holds, looking again and again until the bound.
    async fn eventually(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !check() {
            assert!(Instant::now() < deadline, "not within the bound");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// An attempt that gets its answer.
    fn answered<'a>(_: Arc<Conn<()>>, _: Progress) -> BoxFuture<'a, Result<Vec<u8>, Failure>> {
        Box::pin(async { Ok(vec![1]) })
    }

    /// An attempt that finds its connection gone.
    fn lost<'a>(_: Arc<Conn<()>>, _: Progress) -> BoxFuture<'a, Result<Vec<u8>, Failure>> {
        Box::pin(async { Err(Failure::Lost(UpstreamError::Io("lost".to_string()))) })
    }

    /// An attempt whose data went out — all that is left to wait for is the
    /// answer, which never comes. Nothing goes on the wire: a connection
    /// that was quiet stays so.
    fn written_and_waiting<'a>(
        conn: Arc<Conn<()>>,
        progress: Progress,
    ) -> BoxFuture<'a, Result<Vec<u8>, Failure>> {
        Box::pin(async move {
            progress.written(&conn.quic);
            pending().await
        })
    }

    /// An attempt that never says its data went out.
    fn waiting_to_write<'a>(
        _: Arc<Conn<()>>,
        _: Progress,
    ) -> BoxFuture<'a, Result<Vec<u8>, Failure>> {
        Box::pin(pending())
    }

    /// An attempt that sends a few bytes — the server acknowledges them
    /// when it has read them — and waits for an answer that never comes.
    fn sent_and_waiting<'a>(
        conn: Arc<Conn<()>>,
        progress: Progress,
    ) -> BoxFuture<'a, Result<Vec<u8>, Failure>> {
        Box::pin(async move {
            let (mut send, _recv) = conn.quic.open_bi().await.map_err(|e| Failure::lost(&e))?;
            send.write_all(b"ping")
                .await
                .map_err(|e| Failure::Failed(UpstreamError::Io(e.to_string())))?;
            progress.written(&conn.quic);
            pending().await
        })
    }

    /// The session's connection.
    fn current(session: &Session<Plain>) -> Arc<Conn<()>> {
        let slot = session.slot.lock().expect("session");
        slot.current.clone().expect("a connection")
    }

    /// Waits until `conn` has taken in nothing for a while: what the server
    /// has to say after the handshake is said.
    async fn settled(conn: &Connection) {
        let settle = async {
            let mut last = datagrams_received(conn);
            loop {
                tokio::time::sleep(Duration::from_millis(60)).await;
                let now = datagrams_received(conn);
                if now == last {
                    return;
                }
                last = now;
            }
        };
        timeout(BOUND, settle)
            .await
            .expect("settled within the bound");
    }

    /// How long a connection must have received nothing to be called quiet.
    fn quiet_for(conn: &Connection) -> Duration {
        conn.rtt() * 2 + MAX_ACK_DELAY
    }

    /// Polls `query` once — as far as it gets without the network — and
    /// drops it.
    async fn given_up_at_once(query: impl Future<Output = Result<Vec<u8>, UpstreamError>>) {
        let mut query = std::pin::pin!(query);
        poll_fn(|cx| {
            assert!(query.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }

    /// A session whose connection is up, and has said all it has to say.
    async fn settled_session(fake: &FakeDoq) -> (Session<Plain>, Arc<Gated>) {
        let (session, gated) = session(fake);
        gated.open();
        assert_eq!(session.query(far(), answered).await, Ok(vec![1]));
        settled(&current(&session).quic).await;
        (session, gated)
    }

    /// The query that was dialing is dropped — the fan-out has what it
    /// wants from another upstream —, and one that waited for its dial
    /// takes it over: it dials, once, in its place.
    #[tokio::test]
    async fn a_dial_dropped_with_its_query_is_taken_over_by_a_waiter() {
        let dns = MockDns::spawn().await;
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let (session, gated) = session(&fake);
        let mut first = Box::pin(session.query(far(), answered));
        let mut second = Box::pin(session.query(far(), answered));
        // the first dials and waits for its path, the second for that dial
        assert!(timeout(SETTLE, &mut first).await.is_err());
        assert!(timeout(SETTLE, &mut second).await.is_err());
        assert_eq!(gated.asked(), 1);
        drop(first);
        assert!(timeout(SETTLE, &mut second).await.is_err());
        assert_eq!(gated.asked(), 2);
        gated.open();
        let answer = timeout(BOUND, &mut second).await.expect("within the bound");
        assert_eq!(answer, Ok(vec![1]));
        eventually(|| fake.connections() == 1).await;
    }

    /// The queries that wait for a dial share its failure, and none dials
    /// again for it; the next query, which finds none waiting, does.
    #[tokio::test]
    async fn waiters_share_the_failure_of_the_dial_they_waited_for() {
        let dns = MockDns::spawn().await;
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let (session, gated) = session(&fake);
        gated.refuse(true);
        let (one, two, three, ()) = timeout(BOUND, async {
            tokio::join!(
                session.query(far(), answered),
                session.query(far(), answered),
                session.query(far(), answered),
                gated.open_once_asked(),
            )
        })
        .await
        .expect("within the bound");
        let refused = Err(UpstreamError::Io("refused".to_string()));
        assert_eq!(
            (one, two, three),
            (refused.clone(), refused.clone(), refused)
        );
        assert_eq!(gated.asked(), 1);
        gated.refuse(false);
        let next = timeout(BOUND, session.query(far(), answered)).await;
        assert_eq!(next.expect("within the bound"), Ok(vec![1]));
        assert_eq!(gated.asked(), 2);
    }

    /// A query that took the connection at once and finds it gone goes
    /// again, once, on a new one — not on the one it found gone, which
    /// takes no more queries. One that waited for the connection, or
    /// dialed it, fails with what it found.
    #[tokio::test]
    async fn only_a_query_that_took_its_connection_at_once_goes_again() {
        let dns = MockDns::spawn().await;
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let (session, gated) = session(&fake);
        let attempts = AtomicUsize::new(0);
        let lost_counted = |conn, progress| {
            attempts.fetch_add(1, Ordering::SeqCst);
            lost(conn, progress)
        };
        // the first query dials, and answers; the second waits for that
        // dial, and finds the connection gone
        let waiter = async {
            tokio::join!(
                session.query(far(), answered),
                session.query(far(), &lost_counted),
                gated.open_once_asked(),
            )
        };
        let (first, second, ()) = timeout(BOUND, waiter).await.expect("within the bound");
        assert_eq!(first, Ok(vec![1]));
        assert_eq!(second, Err(UpstreamError::Io("lost".to_string())));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(gated.asked(), 1);
        // the connection found gone takes no more: this query dials one
        let third = timeout(BOUND, session.query(far(), answered)).await;
        assert_eq!(third.expect("within the bound"), Ok(vec![1]));
        assert_eq!(gated.asked(), 2);
        // this one takes it at once, finds it gone, and goes again on a
        // new connection
        attempts.store(0, Ordering::SeqCst);
        let once = AtomicBool::new(true);
        let lost_once = |conn, progress| {
            if once.swap(false, Ordering::SeqCst) {
                lost_counted(conn, progress)
            } else {
                answered(conn, progress)
            }
        };
        let fourth = timeout(BOUND, session.query(far(), lost_once)).await;
        assert_eq!(fourth.expect("within the bound"), Ok(vec![1]));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(gated.asked(), 3);
        eventually(|| fake.connections() == 3).await;
    }

    /// A query that ends without an answer after its data went out, on a
    /// connection that has received nothing since — a live one would have
    /// acknowledged the data by now —, ends that connection's service:
    /// the next query dials a new one.
    #[tokio::test]
    async fn a_connection_that_received_nothing_since_a_query_went_out_takes_no_more() {
        let dns = MockDns::spawn().await;
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let (session, gated) = settled_session(&fake).await;
        let deadline = Instant::now() + quiet_for(&current(&session).quic) + SETTLE;
        assert_eq!(
            session.query(deadline, written_and_waiting).await,
            Err(UpstreamError::Timeout)
        );
        assert_eq!(session.query(far(), answered).await, Ok(vec![1]));
        assert_eq!(gated.asked(), 2);
    }

    /// Given up at once, it is too soon to say anything of the connection:
    /// a live one has had no time to answer.
    #[tokio::test]
    async fn a_query_given_up_at_once_says_nothing_of_its_connection() {
        let dns = MockDns::spawn().await;
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let (session, gated) = settled_session(&fake).await;
        given_up_at_once(session.query(far(), written_and_waiting)).await;
        assert_eq!(session.query(far(), answered).await, Ok(vec![1]));
        assert_eq!(gated.asked(), 1);
    }

    /// A query that never wrote anything — it waited for a stream, say —
    /// says nothing of the connection, however long it was kept waiting.
    #[tokio::test]
    async fn a_query_that_never_wrote_says_nothing_of_its_connection() {
        let dns = MockDns::spawn().await;
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let (session, gated) = settled_session(&fake).await;
        let deadline = Instant::now() + quiet_for(&current(&session).quic) + SETTLE;
        assert_eq!(
            session.query(deadline, waiting_to_write).await,
            Err(UpstreamError::Timeout)
        );
        assert_eq!(session.query(far(), answered).await, Ok(vec![1]));
        assert_eq!(gated.asked(), 1);
    }

    /// A connection that took something in since the data went out — the
    /// acknowledgement of it —, is alive, however late the answer is.
    #[tokio::test]
    async fn a_connection_that_acknowledged_a_query_takes_more() {
        let dns = MockDns::spawn().await;
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let (session, gated) = settled_session(&fake).await;
        let deadline = Instant::now() + quiet_for(&current(&session).quic) + SETTLE;
        assert_eq!(
            session.query(deadline, sent_and_waiting).await,
            Err(UpstreamError::Timeout)
        );
        assert_eq!(session.query(far(), answered).await, Ok(vec![1]));
        assert_eq!(gated.asked(), 1);
    }

    /// A query that has its answer says nothing against its connection,
    /// whatever it was like while it waited.
    #[tokio::test]
    async fn an_answered_query_says_nothing_against_its_connection() {
        let dns = MockDns::spawn().await;
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let (session, gated) = settled_session(&fake).await;
        let quiet = quiet_for(&current(&session).quic) + SETTLE;
        let slow = move |conn: Arc<Conn<()>>, progress: Progress| -> BoxFuture<'_, _> {
            Box::pin(async move {
                progress.written(&conn.quic);
                tokio::time::sleep(quiet).await;
                Ok(vec![1])
            })
        };
        assert_eq!(session.query(far(), slow).await, Ok(vec![1]));
        assert_eq!(session.query(far(), answered).await, Ok(vec![1]));
        assert_eq!(gated.asked(), 1);
    }

    /// What a query that went out on a connection finds when it ends is of
    /// that connection: one that was replaced meanwhile stays.
    #[tokio::test]
    async fn the_end_of_a_connections_service_leaves_its_replacement() {
        let dns = MockDns::spawn().await;
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let (session, gated) = settled_session(&fake).await;
        let quiet = quiet_for(&current(&session).quic);
        // two queries go out on the first connection: the second is given
        // up on long after the first, which ends its service, and after
        // the one that follows has dialed the next
        let first = session.query(Instant::now() + quiet + SETTLE, written_and_waiting);
        let second = session.query(Instant::now() + quiet + 5 * SETTLE, written_and_waiting);
        let next = async {
            tokio::time::sleep(quiet + 2 * SETTLE).await;
            session.query(far(), answered).await
        };
        let (first, second, next) = timeout(BOUND, async { tokio::join!(first, second, next) })
            .await
            .expect("within the bound");
        assert_eq!(first, Err(UpstreamError::Timeout));
        assert_eq!(second, Err(UpstreamError::Timeout));
        assert_eq!(next, Ok(vec![1]));
        assert_eq!(session.query(far(), answered).await, Ok(vec![1]));
        assert_eq!(gated.asked(), 2);
    }
}
