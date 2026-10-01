//! The HTTP/2 connections of one outbound, shared by its streams (phase 2
//! M6 design 5.2): `h2-connect` and `trust-tunnel` send each request (a
//! CONNECT) as a stream of a pooled connection.
//!
//! - A request goes to the oldest connection that carries fewer than
//!   `max-streams` streams (and fewer than the server's own limit) and is
//!   not draining. `h2` would queue a stream past the server's limit and
//!   send nothing until a slot frees, so the pool counts the open streams
//!   itself; a stream's `Lease` holds its place.
//! - With no room, a new connection is dialed. One dial at a time: requests
//!   that come in meanwhile wait for it and then look again, and when it
//!   fails they share its failure instead of dialing the same thing again.
//! - A connection that received GOAWAY, failed, or ended gets no new
//!   streams; the ones it carries run to their end.
//! - A connection without streams for `IDLE_TIMEOUT` is closed (looked at
//!   on every request and by a reaper).
//!
//! The pool does not know the transport: the outbound's `Dial` opens it
//! (TCP, Shadow TLS, TLS with ALPN `h2`) and checks that `h2` was
//! negotiated.

mod stream;

pub(crate) use stream::H2Stream;

use crate::OutboundError;
use crate::task::AbortOnDrop;
use bytes::Bytes;
use h2::client::{self, SendRequest};
use h2::ext::Protocol;
use h2::{Ping, PingPong};
use http::{Request, Response};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts};
use std::io;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{oneshot, watch};
use tokio::time::Instant;

pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const REAP_EVERY: Duration = Duration::from_secs(30);
/// What a stream may receive before its reader catches up
/// (SETTINGS_INITIAL_WINDOW_SIZE). The TrustTunnel document's 128 KiB
/// would cap a tunnel at 1.3 MB/s over a 100 ms round trip.
const STREAM_WINDOW: u32 = 1 << 20;
/// What the connection's streams together may receive: the default
/// `max-streams` at a full window each, and some.
const CONNECTION_WINDOW: u32 = 4 << 20;
/// How long a connection the pool let go of may take to close cleanly
/// (GOAWAY, then the transport's shutdown).
const CLOSE_GRACE: Duration = Duration::from_secs(5);

/// Opens the transport of a new connection. No `Debug` for implementers
/// that hold credentials.
pub(crate) trait Dial: Send + Sync {
    fn dial<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>>;
}

type Conns = Mutex<Vec<Arc<Conn>>>;

/// No `Debug`: the dialer may hold credentials.
pub(crate) struct H2Pool {
    /// The protocol the error texts start with (`h2-connect`).
    label: &'static str,
    max_streams: usize,
    dial: Arc<dyn Dial>,
    /// The connections that take new streams, oldest first.
    conns: Arc<Conns>,
    /// One dial at a time.
    dialing: tokio::sync::Mutex<()>,
    /// Bumped after every dial, success or failure: tells a request that
    /// waited for `dialing` whether a dial finished meanwhile.
    attempts: AtomicU64,
    /// The latest dial's failure (`None` after a success); read and written
    /// with `dialing` held.
    failure: Mutex<Option<OutboundError>>,
    /// Started by the first request: building an outbound (a dry build
    /// included) leaves no task behind.
    reaper: OnceLock<AbortOnDrop>,
}

impl H2Pool {
    /// `max_streams` is the policy's `max-streams` (at least 1).
    pub(crate) fn new(label: &'static str, max_streams: u32, dial: Arc<dyn Dial>) -> H2Pool {
        H2Pool {
            label,
            max_streams: max_streams as usize,
            dial,
            conns: Arc::default(),
            dialing: tokio::sync::Mutex::new(()),
            attempts: AtomicU64::new(0),
            failure: Mutex::new(None),
            reaper: OnceLock::new(),
        }
    }

    /// Sends `request` on a stream of a pooled connection and waits for the
    /// response head; the body is the stream. A request carrying
    /// `h2::ext::Protocol` (extended CONNECT, RFC 8441) waits until the
    /// server's SETTINGS are known and fails unless they enabled it.
    ///
    /// Any status is returned: what a non-2xx one means is the caller's to
    /// say. Dropping the stream resets it.
    pub(crate) async fn open(
        &self,
        request: Request<()>,
        opts: &ConnectOpts,
    ) -> Result<Response<H2Stream>, OutboundError> {
        self.reaper.get_or_init(|| spawn_reaper(&self.conns));
        let lease = self.lease(opts).await?;
        let conn = lease.0.clone();
        if request.extensions().get::<Protocol>().is_some() {
            conn.settled().await;
            if !conn.send.is_extended_connect_protocol_enabled() {
                return Err(OutboundError::Proxy(format!(
                    "{}: the server does not support extended CONNECT",
                    self.label
                )));
            }
        }
        // a fresh handle: another request's queued stream cannot hold it up
        let (response, send) = conn
            .send
            .clone()
            .send_request(request, false)
            .map_err(|e| self.error(&e))?;
        let response = response.await.map_err(|e| self.error(&e))?;
        let (head, recv) = response.into_parts();
        let stream = H2Stream::new(self.label, send, recv).leased(lease);
        Ok(Response::from_parts(head, stream))
    }

    fn error(&self, e: &h2::Error) -> OutboundError {
        OutboundError::Proxy(format!("{}: {}", self.label, describe(e)))
    }

    /// A place on a connection with room, dialing one when there is none.
    async fn lease(&self, opts: &ConnectOpts) -> Result<Lease, OutboundError> {
        if let Some(lease) = self.take() {
            return Ok(lease);
        }
        // read before contending for the lock: tells us whether a dial
        // finished while we waited for it
        let attempt = self.attempts.load(Ordering::SeqCst);
        let _dialing = self.dialing.lock().await;
        if let Some(lease) = self.take() {
            return Ok(lease);
        }
        if self.attempts.load(Ordering::SeqCst) != attempt
            && let Some(failure) = self.failure.lock().expect("failure").as_ref()
        {
            return Err(copy_error(failure));
        }
        let result = self.connect(opts).await;
        self.attempts.fetch_add(1, Ordering::SeqCst);
        let conn = match result {
            Ok(conn) => Arc::new(conn),
            Err(e) => {
                *self.failure.lock().expect("failure") = Some(copy_error(&e));
                return Err(e);
            }
        };
        *self.failure.lock().expect("failure") = None;
        let lease = conn.lease(self.max_streams);
        self.conns.lock().expect("conns").push(conn);
        lease.ok_or_else(|| {
            OutboundError::Proxy(format!("{}: the server accepts no streams", self.label))
        })
    }

    /// The oldest connection with room; the ones that are draining or have
    /// idled too long are let go on the way.
    fn take(&self) -> Option<Lease> {
        let mut conns = self.conns.lock().expect("conns");
        prune(&mut conns, Instant::now());
        conns.iter().find_map(|c| c.lease(self.max_streams))
    }

    async fn connect(&self, opts: &ConnectOpts) -> Result<Conn, OutboundError> {
        let io = self.dial.dial(opts).await?;
        let mut builder = client::Builder::new();
        builder
            .initial_window_size(STREAM_WINDOW)
            .initial_connection_window_size(CONNECTION_WINDOW)
            .enable_push(false);
        // writes the preface; the server's SETTINGS come later
        let (send, mut connection) = builder.handshake::<_, Bytes>(io).await.map_err(|e| {
            OutboundError::Proxy(format!(
                "{}: the HTTP/2 handshake failed: {}",
                self.label,
                describe(&e)
            ))
        })?;
        let ping = connection.ping_pong();
        let (settled_tx, settled) = watch::channel(false);
        let (closing, closed) = oneshot::channel();
        let ended = Arc::new(AtomicBool::new(false));
        tokio::spawn(drive(
            self.label,
            connection,
            ping,
            settled_tx,
            closed,
            ended.clone(),
        ));
        Ok(Conn {
            send,
            settled,
            ended,
            usage: Mutex::new(Usage {
                open: 0,
                idle_since: Instant::now(),
            }),
            _closing: closing,
        })
    }

    #[cfg(test)]
    fn connections(&self) -> usize {
        let mut conns = self.conns.lock().expect("conns");
        prune(&mut conns, Instant::now());
        conns.len()
    }
}

/// One pooled connection. Its streams' leases keep it alive after the pool
/// let go of it; when the last one goes, so does the connection.
struct Conn {
    send: SendRequest<Bytes>,
    /// Turns true once the server's SETTINGS are known (or never, when the
    /// connection ends first: the sender is dropped).
    settled: watch::Receiver<bool>,
    /// The connection's task has finished.
    ended: Arc<AtomicBool>,
    usage: Mutex<Usage>,
    /// Dropped with the connection: tells its task to close it.
    _closing: oneshot::Sender<()>,
}

struct Usage {
    open: usize,
    /// When `open` last fell to 0.
    idle_since: Instant,
}

impl Conn {
    fn lease(self: &Arc<Self>, max_streams: usize) -> Option<Lease> {
        // before the server's SETTINGS, its limit is unbounded
        let limit = max_streams.min(self.send.current_max_send_streams());
        let mut usage = self.usage.lock().expect("usage");
        if usage.open >= limit {
            return None;
        }
        usage.open += 1;
        Some(Lease(self.clone()))
    }

    /// GOAWAY received, an error, or the task ended: `h2` refuses new
    /// streams. A fresh handle is never pending, so the look never waits.
    fn is_draining(&self) -> bool {
        let mut cx = Context::from_waker(Waker::noop());
        self.ended.load(Ordering::SeqCst)
            || matches!(self.send.clone().poll_ready(&mut cx), Poll::Ready(Err(_)))
    }

    fn is_expired(&self, now: Instant) -> bool {
        let usage = self.usage.lock().expect("usage");
        usage.open == 0 && now.duration_since(usage.idle_since) >= IDLE_TIMEOUT
    }

    /// The server's SETTINGS are the first frame it sends (RFC 9113 3.4):
    /// the answer to our PING comes after them.
    async fn settled(&self) {
        let mut settled = self.settled.clone();
        let _ = settled.wait_for(|known| *known).await;
    }
}

/// A stream's place on its connection.
struct Lease(Arc<Conn>);

impl Drop for Lease {
    fn drop(&mut self) {
        let mut usage = self.0.usage.lock().expect("usage");
        usage.open -= 1;
        if usage.open == 0 {
            usage.idle_since = Instant::now();
        }
    }
}

fn prune(conns: &mut Vec<Arc<Conn>>, now: Instant) {
    conns.retain(|c| !c.is_draining() && !c.is_expired(now));
}

/// Runs a connection until it ends, or until the pool and its streams let
/// go of it and it has had `CLOSE_GRACE` to close. Meanwhile it learns the
/// server's SETTINGS with a PING round trip (`h2` has no way to wait for
/// them).
async fn drive<T>(
    label: &'static str,
    connection: client::Connection<T, Bytes>,
    ping: Option<PingPong>,
    settled: watch::Sender<bool>,
    closed: oneshot::Receiver<()>,
    ended: Arc<AtomicBool>,
) where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let mut connection = pin!(connection);
    let settle = async {
        if let Some(mut ping) = ping {
            // failing, the connection is failing too: nothing more to learn
            let _ = ping.ping(Ping::opaque()).await;
        }
        settled.send_replace(true);
        std::future::pending::<()>().await
    };
    let finished = tokio::select! {
        result = &mut connection => Some(result),
        // the sender is dropped, never used
        _ = closed => None,
        () = settle => unreachable!("settling never ends"),
    };
    let result = match finished {
        Some(result) => result,
        // no handle and no stream left: `h2` sends GOAWAY and closes
        None => tokio::time::timeout(CLOSE_GRACE, connection)
            .await
            .unwrap_or(Ok(())),
    };
    ended.store(true, Ordering::SeqCst);
    if let Err(e) = result {
        tracing::debug!("{label}: HTTP/2 connection ended: {}", describe(&e));
    }
}

/// What went wrong, without the GOAWAY's debug data.
fn describe(e: &h2::Error) -> String {
    match (e.reason(), e.get_io()) {
        (Some(reason), _) if e.is_go_away() && e.is_remote() => {
            format!("the server closed the HTTP/2 connection ({reason:?})")
        }
        (Some(reason), _) if e.is_reset() && e.is_remote() => {
            format!("the server reset the stream ({reason:?})")
        }
        (Some(reason), _) => format!("HTTP/2 error ({reason:?})"),
        (None, Some(io)) => format!("the HTTP/2 connection failed: {io}"),
        (None, None) => format!("HTTP/2 error: {e}"),
    }
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

/// Lets go of idle connections until the pool is gone. Holds it weakly: the
/// pool dies with its outbound.
fn spawn_reaper(conns: &Arc<Conns>) -> AbortOnDrop {
    let conns: Weak<Conns> = Arc::downgrade(conns);
    AbortOnDrop(tokio::spawn(async move {
        let mut tick = tokio::time::interval(REAP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(conns) = conns.upgrade() else {
                return;
            };
            prune(&mut conns.lock().expect("conns"), Instant::now());
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2::server::SendResponse;
    use h2::{Reason, RecvStream};
    use http::{Method, StatusCode, Version};
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::sync::Notify;

    /// What the in-process server advertises.
    #[derive(Clone, Copy, Default)]
    struct Server {
        extended: bool,
        /// SETTINGS_INITIAL_WINDOW_SIZE: what each of our streams may send
        /// before a WINDOW_UPDATE.
        window: Option<u32>,
    }

    /// Dials an in-process `h2` server over a pipe. By the host of the
    /// CONNECT, it echoes (half-closing after us), refuses (`deny…`, 407),
    /// or resets the stream after our first bytes (`reset…`).
    #[derive(Default)]
    struct TestDial {
        server: Server,
        /// Fails every dial, after a yield.
        fail: bool,
        dials: AtomicUsize,
        /// One per dial: makes that connection's server send GOAWAY.
        goaway: Mutex<Vec<Arc<Notify>>>,
        /// Server connections that have ended.
        ended: Arc<AtomicUsize>,
    }

    impl Dial for TestDial {
        fn dial<'a>(
            &'a self,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
            Box::pin(async move {
                self.dials.fetch_add(1, Ordering::SeqCst);
                if self.fail {
                    tokio::task::yield_now().await;
                    return Err(OutboundError::Proxy("test: connection refused".into()));
                }
                let (near, far) = tokio::io::duplex(64 * 1024);
                let goaway = Arc::new(Notify::new());
                self.goaway.lock().unwrap().push(goaway.clone());
                tokio::spawn(serve(far, self.server, goaway, self.ended.clone()));
                Ok(Box::new(near) as BoxedStream)
            })
        }
    }

    async fn serve(io: DuplexStream, server: Server, goaway: Arc<Notify>, ended: Arc<AtomicUsize>) {
        let mut builder = h2::server::Builder::new();
        if let Some(window) = server.window {
            builder.initial_window_size(window);
        }
        if server.extended {
            builder.enable_connect_protocol();
        }
        if let Ok(mut conn) = builder.handshake::<_, Bytes>(io).await {
            loop {
                tokio::select! {
                    next = conn.accept() => match next {
                        Some(Ok((request, respond))) => {
                            tokio::spawn(answer(request, respond));
                        }
                        _ => break,
                    },
                    () = goaway.notified() => conn.graceful_shutdown(),
                }
            }
        }
        ended.fetch_add(1, Ordering::SeqCst);
    }

    async fn answer(request: Request<RecvStream>, mut respond: SendResponse<Bytes>) {
        let host = request.uri().host().unwrap_or_default().to_string();
        if host.starts_with("deny") {
            let refusal = Response::builder().status(407).body(()).unwrap();
            let _ = respond.send_response(refusal, true);
            return;
        }
        let Ok(mut send) = respond.send_response(Response::new(()), false) else {
            return;
        };
        let mut recv = request.into_body();
        if host.starts_with("reset") {
            let _ = recv.data().await;
            send.send_reset(Reason::CONNECT_ERROR);
            return;
        }
        let stream = H2Stream::new("server", send, recv);
        let (mut r, mut w) = tokio::io::split(stream);
        let _ = tokio::io::copy(&mut r, &mut w).await;
        let _ = w.shutdown().await;
    }

    fn pool(dial: &Arc<TestDial>, max_streams: u32) -> H2Pool {
        H2Pool::new("test", max_streams, dial.clone())
    }

    fn connect(authority: &str) -> Request<()> {
        Request::builder()
            .method(Method::CONNECT)
            .uri(authority)
            .version(Version::HTTP_2)
            .body(())
            .unwrap()
    }

    async fn tunnel(pool: &H2Pool, authority: &str) -> H2Stream {
        let response = pool
            .open(connect(authority), &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.into_body()
    }

    async fn round_trip(stream: &mut H2Stream, data: &[u8]) {
        stream.write_all(data).await.unwrap();
        let mut back = vec![0; data.len()];
        stream.read_exact(&mut back).await.unwrap();
        assert_eq!(back, data);
    }

    /// Polls `check` until it holds, for at most five seconds.
    async fn eventually(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn a_stream_carries_data_both_ways_under_flow_control() {
        // the server lets each stream send 16 KiB at a time; we get more
        // than two of our windows back, so reads must release capacity
        let dial = Arc::new(TestDial {
            server: Server {
                window: Some(16 * 1024),
                ..Server::default()
            },
            ..TestDial::default()
        });
        let pool = pool(&dial, 3);
        let stream = tunnel(&pool, "echo.test:7").await;
        let data: Vec<u8> = (0..(STREAM_WINDOW as usize * 2 + 12_345))
            .map(|i| (i % 251) as u8)
            .collect();
        let (mut r, mut w) = tokio::io::split(stream);
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            w.write_all(&sent).await.unwrap();
            w.shutdown().await.unwrap();
        });
        let mut back = Vec::new();
        // a window never handed back stalls the echo: bounded
        tokio::time::timeout(Duration::from_secs(30), r.read_to_end(&mut back))
            .await
            .expect("stalled")
            .unwrap();
        writer.await.unwrap();
        assert!(back == data, "the echo differs");
    }

    #[tokio::test]
    async fn shutdown_is_a_half_close() {
        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 3);
        let mut stream = tunnel(&pool, "echo.test:7").await;
        stream.write_all(b"hello").await.unwrap();
        stream.shutdown().await.unwrap();
        // the server saw our END_STREAM, echoed, and ended its side; we
        // could still read all of it
        let mut back = Vec::new();
        stream.read_to_end(&mut back).await.unwrap();
        assert_eq!(back, b"hello");
        let e = stream.write_all(b"more").await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::BrokenPipe);
    }

    #[tokio::test]
    async fn a_full_connection_makes_another_and_freed_places_are_reused() {
        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 2);
        let (mut a, mut b, mut c) = tokio::join!(
            tunnel(&pool, "echo.test:1"),
            tunnel(&pool, "echo.test:2"),
            tunnel(&pool, "echo.test:3"),
        );
        assert_eq!(dial.dials.load(Ordering::SeqCst), 2);
        assert_eq!(pool.connections(), 2);
        for stream in [&mut a, &mut b, &mut c] {
            round_trip(stream, b"ping").await;
        }
        drop((a, b, c));
        // the places are free again: no new dial
        let (mut d, mut e) =
            tokio::join!(tunnel(&pool, "echo.test:4"), tunnel(&pool, "echo.test:5"));
        round_trip(&mut d, b"again").await;
        round_trip(&mut e, b"again").await;
        assert_eq!(dial.dials.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_refusal_comes_back_and_frees_its_place() {
        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 1);
        let response = pool
            .open(connect("deny.test:7"), &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PROXY_AUTHENTICATION_REQUIRED);
        drop(response);
        let mut stream = tunnel(&pool, "echo.test:7").await;
        round_trip(&mut stream, b"ok").await;
        assert_eq!(dial.dials.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn after_goaway_new_streams_go_elsewhere_and_the_old_one_finishes() {
        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 3);
        let mut first = tunnel(&pool, "echo.test:1").await;
        let goaway = dial.goaway.lock().unwrap()[0].clone();
        goaway.notify_one();
        eventually(|| pool.connections() == 0).await;
        let mut second = tunnel(&pool, "echo.test:2").await;
        assert_eq!(dial.dials.load(Ordering::SeqCst), 2);
        // the stream that was open before the GOAWAY carries on to its end
        round_trip(&mut first, b"still here").await;
        first.shutdown().await.unwrap();
        let mut rest = Vec::new();
        first.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
        drop(first);
        eventually(|| dial.ended.load(Ordering::SeqCst) == 1).await;
        round_trip(&mut second, b"fine").await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_connection_idle_for_a_minute_is_closed() {
        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 3);
        drop(tunnel(&pool, "echo.test:1").await);
        tokio::time::advance(IDLE_TIMEOUT - Duration::from_secs(1)).await;
        drop(tunnel(&pool, "echo.test:2").await);
        assert_eq!(dial.dials.load(Ordering::SeqCst), 1, "still fresh");
        tokio::time::advance(IDLE_TIMEOUT).await;
        assert_eq!(pool.connections(), 0);
        // let go of, the connection closes: the server sees it end
        eventually(|| dial.ended.load(Ordering::SeqCst) == 1).await;
        drop(tunnel(&pool, "echo.test:3").await);
        assert_eq!(dial.dials.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn extended_connect_follows_the_servers_settings() {
        let udp = || {
            let mut request = Request::builder()
                .method(Method::CONNECT)
                .uri("https://proxy.test:443/.well-known/masque/udp/192.0.2.6/443/")
                .version(Version::HTTP_2)
                .header("capsule-protocol", "?1")
                .body(())
                .unwrap();
            request
                .extensions_mut()
                .insert(Protocol::from_static("connect-udp"));
            request
        };
        let dial = Arc::new(TestDial {
            server: Server {
                extended: true,
                ..Server::default()
            },
            ..TestDial::default()
        });
        let response = pool(&dial, 3)
            .open(udp(), &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body();
        round_trip(&mut stream, b"capsules").await;

        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 3);
        let Err(e) = pool.open(udp(), &ConnectOpts::default()).await else {
            panic!("extended CONNECT without the server's consent");
        };
        assert_eq!(
            e.to_string(),
            "test: the server does not support extended CONNECT"
        );
        // a plain CONNECT on the same connection is fine
        drop(tunnel(&pool, "echo.test:7").await);
        assert_eq!(dial.dials.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_reset_from_the_server_is_a_read_error() {
        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 3);
        let mut stream = tunnel(&pool, "reset.test:7").await;
        stream.write_all(b"x").await.unwrap();
        let mut buf = [0u8; 8];
        let e = stream.read(&mut buf).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(
            e.to_string(),
            "test: the server reset the stream (CONNECT_ERROR)"
        );
    }

    #[tokio::test]
    async fn a_failed_dial_is_shared_with_the_requests_that_waited_for_it() {
        let dial = Arc::new(TestDial {
            fail: true,
            ..TestDial::default()
        });
        let pool = pool(&dial, 3);
        let opts = ConnectOpts::default();
        let (a, b, c) = tokio::join!(
            pool.open(connect("echo.test:1"), &opts),
            pool.open(connect("echo.test:2"), &opts),
            pool.open(connect("echo.test:3"), &opts),
        );
        for result in [a, b, c] {
            let Err(e) = result else {
                panic!("a failed dial gave a stream");
            };
            assert_eq!(e.to_string(), "test: connection refused");
        }
        assert_eq!(dial.dials.load(Ordering::SeqCst), 1);
        // a later request tries again
        assert!(pool.open(connect("echo.test:4"), &opts).await.is_err());
        assert_eq!(dial.dials.load(Ordering::SeqCst), 2);
    }
}
