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
//!   resolver.
//! - A connection that has closed — its idle timeout, the server's close, a
//!   protocol error — or that is going away (an HTTP/3 GOAWAY) takes no new
//!   query: the next one dials. Nothing keeps a connection alive
//!   (`Transport::DNS`): unused, it idles out.
//! - A query on a connection that was up when it came, which finds the
//!   connection gone (`Failure::Lost`) — a server that dropped it while it
//!   idled answers with a stateless reset —, goes again, once, on a new
//!   connection: a DNS query can be asked again.
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
use tokio::sync::Notify;
use tokio::time::Instant;

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
    /// one connects. The query's deadline bounds it; here, `deadline` bounds
    /// only the opening of each path.
    async fn connect(&self, deadline: Instant) -> Result<Connection, UpstreamError> {
        let addresses = self.bootstrap.resolve(&self.host).await?;
        let mut last = UpstreamError::Bootstrap(format!("no addresses for {}", self.host));
        for address in interleave(addresses, false) {
            let server = Target::new(HostName::Ip(address), self.port);
            match self.connect_to(&server, deadline).await {
                Ok(connection) => return Ok(connection),
                Err(e) => {
                    tracing::debug!(upstream = %self.name, %address, error = %e, "cannot connect to the server at this address");
                    last = transport_error(e);
                }
            }
        }
        Err(last)
    }

    async fn connect_to(&self, server: &Target, deadline: Instant) -> io::Result<Connection> {
        let opts = ConnectOpts {
            timeout: deadline.saturating_duration_since(Instant::now()),
        };
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

    /// Runs `attempt` on the session's connection, all of it by `deadline`.
    /// A query that finds a connection it took at once gone
    /// (`Failure::Lost`) is attempted again, once, on a new connection; on
    /// a connection it waited for, it fails.
    pub(super) async fn query<'a>(
        &'a self,
        deadline: Instant,
        attempt: impl Fn(Arc<Conn<P::State>>) -> BoxFuture<'a, Result<Vec<u8>, Failure>>,
    ) -> Result<Vec<u8>, UpstreamError> {
        let query = async {
            let (conn, taken) = self.take(deadline).await?;
            let e = match attempt(conn).await {
                Err(Failure::Lost(e)) if taken => e,
                done => return done.map_err(Failure::into_error),
            };
            // the error says its layer (`quic: …`, `h3: …`)
            tracing::debug!(upstream = %self.dialer.name, "{e}; the query goes again on a new connection");
            let (conn, _) = self.take(deadline).await?;
            attempt(conn).await.map_err(Failure::into_error)
        };
        tokio::time::timeout_at(deadline, query)
            .await
            .unwrap_or(Err(UpstreamError::Timeout))
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

/// A dial in flight; when it ends — or its query gives up on it —, the
/// queries waiting for it look again.
struct Dialing<'a, P: Protocol>(&'a Session<P>);

impl<P: Protocol> Drop for Dialing<'_, P> {
    fn drop(&mut self) {
        self.0.slot.lock().expect("session").dialing = false;
        self.0.dialed.notify_waiters();
    }
}
