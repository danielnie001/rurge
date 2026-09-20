//! The relay: a cancellable, idle-timed, byte-counting bidirectional copy
//! (M3 design §7.3). Each direction runs as its own future so a stalled write
//! on one side never blocks the other (no head-of-line blocking), and every
//! wait — the read, the write and the half-close that follows EOF — races
//! against a stop token so `kill`, graceful shutdown and the idle timeout all
//! end a stuck session promptly. Replaces M3a's `copy_bidirectional`. Every
//! write is followed by a flush (a TLS writer can hold the last record back
//! until then), and a direction that ends in error cancels the other so the
//! session ends instead of leaving a healthy direction waiting on a broken one.

use rurge_config::rule::ProtocolKind;
use rurge_inbound::{SessionHandle, SessionOutcome};
use rurge_net::connector::BoxedStream;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const BUF: usize = 8 * 1024;

/// Runs once on the first non-empty read of a `copy_half` direction (M3b SNI
/// sniffing; see `pump`).
type FirstChunkHook = Box<dyn FnOnce(&[u8]) + Send>;

/// Copies bytes both ways until either side closes, the idle timer fires, or
/// the handle's token is cancelled; then finishes the handle. Each direction is
/// its own task-like future so a stalled write on one side never blocks the
/// other, and every read and write races against the stop token so kill,
/// graceful shutdown and the idle timeout end stuck sessions promptly.
pub async fn pump(
    client: BoxedStream,
    upstream: BoxedStream,
    handle: Arc<SessionHandle>,
    idle: Duration,
) {
    let (cr, cw) = tokio::io::split(client);
    let (ur, uw) = tokio::io::split(upstream);
    // Child of the session token: kill / shutdown cancel the parent (and so
    // this); the idle watchdog cancels only this.
    let stop = handle.token().child_token();
    let started = Instant::now();
    let activity = Arc::new(AtomicU64::new(0)); // ms since `started` of the last moved byte

    let watchdog = idle_watchdog(stop.clone(), activity.clone(), started, idle);
    let halves = {
        let stop = stop.clone();
        let h_up = handle.clone();
        let h_down = handle.clone();
        let h_sniff = handle.clone();
        let a_up = activity.clone();
        let a_down = activity.clone();
        // Client → upstream only: sniffs the first chunk for a TLS ClientHello
        // SNI (M3b, observability only; routing still uses the CONNECT/SOCKS
        // target). Plain-HTTP forwarding does not go through `pump`, so it is
        // unaffected.
        let sniff_first: Option<FirstChunkHook> = Some(Box::new(move |chunk: &[u8]| {
            if let Some(sni) = crate::sniff::parse_sni(chunk) {
                h_sniff.set_sni(sni);
                h_sniff.set_protocol(ProtocolKind::Https);
            }
        }));
        async move {
            // A direction that fails ends the other one too: the tunnel is
            // broken, and the side still waiting for bytes would otherwise sit
            // there until its own peer gives up or the idle timer fires.
            let up = async {
                let r = copy_half(
                    cr,
                    uw,
                    stop.clone(),
                    a_up,
                    started,
                    move |n| h_up.add_up(n),
                    sniff_first,
                )
                .await;
                if r.is_err() {
                    stop.cancel();
                }
                r
            };
            let down = async {
                let r = copy_half(
                    ur,
                    cw,
                    stop.clone(),
                    a_down,
                    started,
                    move |n| h_down.add_down(n),
                    None,
                )
                .await;
                if r.is_err() {
                    stop.cancel();
                }
                r
            };
            let r = tokio::join!(up, down);
            stop.cancel(); // both directions done: release the watchdog
            r
        }
    };
    let (idle_fired, (r_up, r_down)) = tokio::join!(watchdog, halves);

    let outcome = if idle_fired {
        SessionOutcome::Completed
    } else if handle.was_killed() {
        SessionOutcome::Failed("killed".into())
    } else if handle.token().is_cancelled() {
        SessionOutcome::Completed // graceful shutdown
    } else if let Err(e) = r_up.and(r_down) {
        SessionOutcome::Failed(e.to_string())
    } else {
        SessionOutcome::Completed
    };
    handle.finish(outcome);
}

/// Cancels `stop` once no byte has moved for `idle`. Returns whether it fired
/// (false when `stop` was cancelled by someone else first).
async fn idle_watchdog(
    stop: CancellationToken,
    activity: Arc<AtomicU64>,
    started: Instant,
    idle: Duration,
) -> bool {
    loop {
        let last = Duration::from_millis(activity.load(Ordering::Relaxed));
        let remaining = (last + idle).saturating_sub(started.elapsed());
        if remaining.is_zero() {
            stop.cancel();
            return true;
        }
        tokio::select! {
            biased;
            _ = stop.cancelled() => return false,
            _ = tokio::time::sleep(remaining) => {}
        }
    }
}

/// One direction: read → write until EOF (then half-close the writer), an
/// error, or `stop`. Both the read and the write race against `stop`. `first`,
/// when given, runs once on the first non-empty read (before it is written
/// onward) and is then consumed.
async fn copy_half<R, W>(
    mut reader: R,
    mut writer: W,
    stop: CancellationToken,
    activity: Arc<AtomicU64>,
    started: Instant,
    count: impl Fn(u64),
    mut first: Option<FirstChunkHook>,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; BUF];
    let cancelled = stop.cancelled();
    tokio::pin!(cancelled);
    loop {
        let n = tokio::select! {
            biased;
            _ = &mut cancelled => return Ok(()),
            r = reader.read(&mut buf) => r?,
        };
        if n == 0 {
            // Half-close. A TCP writer sends a FIN and cannot block, but a TLS /
            // WebSocket writer has to push a close_notify / Close frame (and a
            // `LazyHead` may still owe its head) through a socket the peer may
            // have stopped reading: this wait races `stop` like every other one
            // here.
            tokio::select! {
                biased;
                _ = &mut cancelled => {}
                _ = writer.shutdown() => {}
            }
            return Ok(());
        }
        if let Some(f) = first.take() {
            f(&buf[..n]);
        }
        tokio::select! {
            biased;
            _ = &mut cancelled => return Ok(()),
            w = async {
                writer.write_all(&buf[..n]).await?;
                // `write_all` only says the writer took the bytes. A TLS
                // writer whose socket is full keeps its last record to itself
                // until the next write or flush — and after the final bytes of
                // a request there is no next write.
                writer.flush().await
            } => w?,
        }
        count(n as u64);
        activity.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use rurge_config::session::SessionInfo;
    use std::pin::Pin;
    use std::task::{Context, Poll, ready};
    use tokio::io::ReadBuf;

    fn handle() -> Arc<SessionHandle> {
        SessionHandle::new(1, SessionInfo::tcp(HostName::parse("a.test"), 80))
    }

    #[tokio::test]
    async fn relays_both_directions_and_counts_bytes() {
        let (client_a, client_b) = tokio::io::duplex(1024);
        let (upstream_a, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(30),
        ));
        // client → upstream
        let mut ca = client_a;
        let mut ua = upstream_a;
        ca.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        ua.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        // upstream → client
        ua.write_all(b"po").await.unwrap();
        let mut buf2 = [0u8; 2];
        ca.read_exact(&mut buf2).await.unwrap();
        assert_eq!(&buf2, b"po");
        drop(ca);
        drop(ua);
        task.await.unwrap();
        assert_eq!(h.bytes(), (4, 2));
        assert_eq!(h.outcome(), Some(SessionOutcome::Completed));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_closes_a_quiet_session() {
        let (_client_a, client_b) = tokio::io::duplex(1024);
        let (_upstream_a, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(600),
        ));
        tokio::time::advance(Duration::from_secs(601)).await;
        task.await.unwrap();
        assert!(h.is_finished());
        assert_eq!(h.outcome(), Some(SessionOutcome::Completed));
    }

    #[tokio::test]
    async fn kill_stops_the_relay() {
        let (_client_a, client_b) = tokio::io::duplex(1024);
        let (_upstream_a, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(600),
        ));
        h.kill();
        task.await.unwrap();
        assert_eq!(h.outcome(), Some(SessionOutcome::Failed("killed".into())));
    }

    /// A parent-token cancellation that is not a `kill` (the future shape of
    /// graceful shutdown, M3 design §7.3) completes cleanly and is never
    /// reported as killed.
    #[tokio::test]
    async fn graceful_cancel_without_kill_completes() {
        let (_client_a, client_b) = tokio::io::duplex(1024);
        let (_upstream_a, upstream_b) = tokio::io::duplex(1024);
        let parent = CancellationToken::new();
        let h = SessionHandle::new_with_token(
            1,
            SessionInfo::tcp(HostName::parse("a.test"), 80),
            parent.child_token(),
        );
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(600),
        ));
        parent.cancel();
        task.await.unwrap();
        assert_eq!(h.outcome(), Some(SessionOutcome::Completed));
        assert!(!h.was_killed());
    }

    /// Regression test: a write stalled on a backpressured/black-holed peer
    /// used to block the whole pump, so neither `kill` nor the idle timeout
    /// could ever reap it. `kill` must interrupt it promptly.
    #[tokio::test]
    async fn kill_interrupts_a_stalled_write() {
        let (mut client_a, client_b) = tokio::io::duplex(16);
        let (_upstream_a, upstream_b) = tokio::io::duplex(16);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(600),
        ));
        // `_upstream_a` is kept alive but never read, so the upstream write
        // half backs up almost immediately; this write then blocks pump's
        // client → upstream loop on a peer that never drains it.
        let _writer = tokio::spawn(async move {
            let buf = vec![0u8; 65536];
            let _ = client_a.write_all(&buf).await;
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while h.bytes().0 == 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        h.kill();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("pump did not stop after kill")
            .unwrap();
        assert_eq!(h.outcome(), Some(SessionOutcome::Failed("killed".into())));
    }

    /// A writer that takes everything but never finishes its shutdown: the
    /// shape of a TLS / WebSocket writer that owes the peer a close_notify /
    /// Close frame the peer never reads. `entered` fires on the first
    /// `poll_shutdown`, so the test needs no sleep to know it is parked.
    struct ShutdownParks {
        entered: Arc<tokio::sync::Notify>,
    }

    impl AsyncWrite for ShutdownParks {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            data: &[u8],
        ) -> std::task::Poll<io::Result<usize>> {
            std::task::Poll::Ready(Ok(data.len()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            self.entered.notify_one();
            std::task::Poll::Pending // and never wakes
        }
    }

    /// Regression test: the half-close after EOF used to be the one wait that
    /// raced nothing, so a writer whose shutdown blocks (TLS close_notify, a
    /// WebSocket Close frame, a `LazyHead` that still owes its head) parked
    /// the direction forever — kill, graceful shutdown and the idle watchdog
    /// all cancel `stop`, but nobody was listening.
    #[tokio::test]
    async fn cancelling_stop_interrupts_a_half_close_that_never_completes() {
        let stop = CancellationToken::new();
        let entered = Arc::new(tokio::sync::Notify::new());
        let writer = ShutdownParks {
            entered: entered.clone(),
        };
        let reader: &[u8] = &[]; // EOF on the first read
        let half = tokio::spawn({
            let stop = stop.clone();
            async move {
                copy_half(
                    reader,
                    writer,
                    stop,
                    Arc::new(AtomicU64::new(0)),
                    Instant::now(),
                    |_| {},
                    None,
                )
                .await
            }
        });
        entered.notified().await; // the shutdown is parked
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(5), half)
            .await
            .expect("a cancelled stop token ends the half-close")
            .unwrap()
            .unwrap();
    }

    /// Not a test of anything: prints how fast `pump` moves bytes over
    /// loopback TCP. Run before and after a change to the copy loop:
    /// `cargo test -p rurge-engine --release relay_throughput -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn relay_throughput() {
        use tokio::net::{TcpListener, TcpStream};
        const TOTAL: usize = 512 * 1024 * 1024;
        async fn pair() -> (TcpStream, TcpStream) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (a, b) = tokio::join!(TcpStream::connect(addr), listener.accept());
            (a.unwrap(), b.unwrap().0)
        }
        let (mut app, client) = pair().await;
        let (upstream, mut origin) = pair().await;
        let relay = tokio::spawn(pump(
            Box::new(client),
            Box::new(upstream),
            handle(),
            Duration::from_secs(600),
        ));
        let started = std::time::Instant::now();
        let send = tokio::spawn(async move {
            let block = vec![7u8; 64 * 1024];
            for _ in 0..TOTAL / block.len() {
                app.write_all(&block).await.unwrap();
            }
            app.shutdown().await.unwrap();
            app
        });
        let mut got = 0;
        let mut buf = vec![0u8; 64 * 1024];
        while got < TOTAL {
            let n = origin.read(&mut buf).await.unwrap();
            assert!(n > 0, "the relay stopped at {got} bytes");
            got += n;
        }
        let secs = started.elapsed().as_secs_f64();
        println!(
            "relay: {:.0} MiB/s",
            (TOTAL as f64 / (1024.0 * 1024.0)) / secs
        );
        drop(origin);
        drop(send.await.unwrap());
        relay.await.unwrap();
    }

    /// Accepts every write at once but only passes it on when flushed: what
    /// `AsyncWrite` allows, and what tokio-rustls does with its last record
    /// once the socket below it is full.
    struct HoldsUntilFlushed {
        inner: tokio::io::DuplexStream,
        held: Vec<u8>,
    }

    impl AsyncRead for HoldsUntilFlushed {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for HoldsUntilFlushed {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.held.extend_from_slice(data);
            Poll::Ready(Ok(data.len()))
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            let this = &mut *self;
            while !this.held.is_empty() {
                let n = ready!(Pin::new(&mut this.inner).poll_write(cx, &this.held))?;
                this.held.drain(..n);
            }
            Pin::new(&mut this.inner).poll_flush(cx)
        }

        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            ready!(self.as_mut().poll_flush(cx))?;
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    #[tokio::test]
    async fn what_was_written_is_flushed_without_waiting_for_more() {
        let (mut app, client) = tokio::io::duplex(4096);
        let (near, mut far) = tokio::io::duplex(4096);
        let upstream = HoldsUntilFlushed {
            inner: near,
            held: Vec::new(),
        };
        let relay = tokio::spawn(pump(
            Box::new(client),
            Box::new(upstream),
            handle(),
            Duration::from_secs(600),
        ));
        // the last bytes of a request: nothing follows them, and the answer
        // only comes once they have arrived
        app.write_all(b"the last bytes of a request").await.unwrap();
        let mut got = [0u8; 27];
        tokio::time::timeout(Duration::from_secs(5), far.read_exact(&mut got))
            .await
            .expect("the tail stayed behind in the writer")
            .unwrap();
        assert_eq!(&got, b"the last bytes of a request");
        relay.abort();
    }

    /// Reads fail at once; writes vanish.
    struct FailsOnRead;

    impl AsyncRead for FailsOnRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::other("boom")))
        }
    }

    impl AsyncWrite for FailsOnRead {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(data.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn an_error_in_one_direction_ends_the_other() {
        // the client stays connected and silent: only the upstream is broken
        let (_app, client) = tokio::io::duplex(1024);
        let h = handle();
        tokio::time::timeout(
            Duration::from_secs(5),
            pump(
                Box::new(client),
                Box::new(FailsOnRead),
                h.clone(),
                Duration::from_secs(600),
            ),
        )
        .await
        .expect("the healthy direction kept the broken session open");
        assert_eq!(h.outcome(), Some(SessionOutcome::Failed("boom".into())));
    }
}
