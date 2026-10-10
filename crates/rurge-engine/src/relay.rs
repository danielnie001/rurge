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

/// What each direction reads into, and so the most it writes at once. It was
/// measured against the alternatives (phase 2 M8 design 5.11): flushing less
/// often than after every write gained nothing, and a larger buffer moved
/// more — 64 KiB about a tenth more through every encrypting stack, the two
/// directions taken together and all of it in the upload direction, on the
/// one machine it was measured on — but a session holds two buffers for as
/// long as it lives, busy or not, and 10,000 sessions are to fit in 300 MB
/// (NFR-01).
const BUF: usize = 8 * 1024;

/// Runs once on the first non-empty read of a `copy_half` direction (M3b SNI
/// sniffing, and the first byte from upstream; see `pump`).
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
    pump_sized(client, upstream, handle, idle, BUF).await
}

/// `pump`, each direction reading `buf_size` bytes at a time: the benchmark
/// runs it with sizes other than `BUF`.
async fn pump_sized(
    client: BoxedStream,
    upstream: BoxedStream,
    handle: Arc<SessionHandle>,
    idle: Duration,
    buf_size: usize,
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
        // Upstream → client: the first chunk marks the first byte as it is
        // read, before it goes on to the client (phase 2 M3c design 4.1).
        let h_first = handle.clone();
        let first_byte: Option<FirstChunkHook> = Some(Box::new(move |_: &[u8]| {
            h_first.mark_first_byte();
        }));
        // Upstream ending before it sent anything, while the client is still
        // there, is upstream's failure (M3c design 4.3); once the client's
        // side has ended, it is not — nor is upstream's silence after that.
        let h_client_gone = handle.clone();
        let h_end = handle.clone();
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
                    move || h_client_gone.mark_client_gone(),
                    buf_size,
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
                    first_byte,
                    move || {
                        if !h_end.client_gone() && !h_end.first_byte_seen() {
                            h_end.mark_upstream_failed();
                        }
                    },
                    buf_size,
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
/// onward) and is then consumed; `reader_done` runs when the reader ends —
/// at EOF or on a read error, not when `stop` ends the direction. `buf_size`
/// is how much is read, and so written, at once.
#[allow(clippy::too_many_arguments)]
async fn copy_half<R, W>(
    mut reader: R,
    mut writer: W,
    stop: CancellationToken,
    activity: Arc<AtomicU64>,
    started: Instant,
    count: impl Fn(u64),
    mut first: Option<FirstChunkHook>,
    reader_done: impl Fn(),
    buf_size: usize,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; buf_size];
    let cancelled = stop.cancelled();
    tokio::pin!(cancelled);
    loop {
        let n = tokio::select! {
            biased;
            _ = &mut cancelled => return Ok(()),
            r = reader.read(&mut buf) => match r {
                Ok(n) => n,
                Err(e) => {
                    reader_done();
                    return Err(e);
                }
            },
        };
        if n == 0 {
            reader_done();
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

    /// The first byte is what upstream sends, not what the client does (M3c
    /// design 4.1).
    #[tokio::test]
    async fn the_first_byte_is_the_first_one_upstream_sends() {
        let (mut ca, client_b) = tokio::io::duplex(1024);
        let (mut ua, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        h.mark_connected();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(30),
        ));
        ca.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        ua.read_exact(&mut buf).await.unwrap();
        assert_eq!(
            h.first_byte_time(),
            None,
            "the client's bytes are no response"
        );
        ua.write_all(b"po").await.unwrap();
        let mut buf2 = [0u8; 2];
        ca.read_exact(&mut buf2).await.unwrap();
        assert!(h.first_byte_time().is_some());
        drop(ca);
        drop(ua);
        task.await.unwrap();
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
                    || {},
                    BUF,
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

    /// A writer that takes everything but never finishes a flush: the shape
    /// of a TLS writer whose socket is full and whose last record cannot go
    /// out. `entered` fires on the first `poll_flush`, so the test needs no
    /// sleep to know it is parked.
    struct FlushParks {
        entered: Arc<tokio::sync::Notify>,
    }

    impl AsyncWrite for FlushParks {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(data.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.entered.notify_one();
            Poll::Pending // and never wakes
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// The flush that follows a write waits like the read, the write and the
    /// half-close do, so it races `stop` like them: a writer that cannot
    /// finish it would otherwise hold the direction, and with it the
    /// session, against `kill`, shutdown and the idle timeout.
    #[tokio::test]
    async fn cancelling_stop_interrupts_a_flush_that_never_completes() {
        let bound = Duration::from_secs(5);
        let stop = CancellationToken::new();
        let entered = Arc::new(tokio::sync::Notify::new());
        let writer = FlushParks {
            entered: entered.clone(),
        };
        let reader: &[u8] = b"the last bytes of a request";
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
                    || {},
                    BUF,
                )
                .await
            }
        });
        tokio::time::timeout(bound, entered.notified())
            .await
            .expect("the bytes are written and the flush is asked for");
        stop.cancel();
        tokio::time::timeout(bound, half)
            .await
            .expect("a cancelled stop token ends a flush that never completes")
            .unwrap()
            .unwrap();
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

    /// Takes every write whole and notes what is asked of it, in order.
    /// `parks`: every write is turned away once before it is taken, as by a
    /// writer whose socket is full.
    struct Noting {
        asked: Arc<std::sync::Mutex<Vec<&'static str>>>,
        parks: bool,
        parked: bool,
    }

    impl Noting {
        fn note(&self, what: &'static str) {
            // The encrypting streams seal the bytes of a write that could not
            // go out at once, and send them when it is retried: anything in
            // between would push that ciphertext out, and the retry would
            // send the same bytes again.
            assert!(!self.parked, "{what} between a parked write and its retry");
            self.asked.lock().unwrap().push(what);
        }
    }

    impl AsyncWrite for Noting {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            if self.parks && !self.parked {
                self.parked = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            self.parked = false;
            self.note("write");
            Poll::Ready(Ok(data.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.note("flush");
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.note("shutdown");
            Poll::Ready(Ok(()))
        }
    }

    /// One direction from `data`, all of it ready at once and then the end,
    /// into a `Noting` writer: what was asked of the writer, in order.
    async fn asked_of_the_writer(data: &[u8], parks: bool) -> Vec<&'static str> {
        let asked: Arc<std::sync::Mutex<Vec<&'static str>>> = Arc::default();
        let writer = Noting {
            asked: asked.clone(),
            parks,
            parked: false,
        };
        tokio::time::timeout(
            Duration::from_secs(5),
            copy_half(
                data,
                writer,
                CancellationToken::new(),
                Arc::new(AtomicU64::new(0)),
                Instant::now(),
                |_| {},
                None,
                || {},
                BUF,
            ),
        )
        .await
        .expect("the direction ends with its reader")
        .unwrap();
        asked.lock().unwrap().clone()
    }

    /// The half-close comes after everything written before it, whether or
    /// not the writer would see to that on its own.
    #[tokio::test]
    async fn a_half_close_flushes_first() {
        assert_eq!(
            asked_of_the_writer(b"the last bytes", false).await,
            ["write", "flush", "shutdown"]
        );
    }

    /// Even with the reader's next chunk already there, a write is followed
    /// by its flush before the next one: no write waits on more bytes to
    /// push it out.
    #[tokio::test]
    async fn every_write_is_followed_by_one_flush() {
        let data = vec![7u8; 3 * BUF];
        assert_eq!(
            asked_of_the_writer(&data, false).await,
            [
                "write", "flush", "write", "flush", "write", "flush", "shutdown"
            ]
        );
    }

    /// A write that could not go out at once is retried before anything
    /// else is asked of the writer (`Noting::note`).
    #[tokio::test]
    async fn nothing_comes_between_a_parked_write_and_its_retry() {
        let data = vec![7u8; 3 * BUF];
        let asked = asked_of_the_writer(&data, true).await;
        assert_eq!(asked.iter().filter(|a| **a == "write").count(), 3);
        assert_eq!(asked.last(), Some(&"shutdown"));
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

    #[tokio::test]
    async fn a_clean_eof_in_one_direction_leaves_the_other_running() {
        let (mut app, client) = tokio::io::duplex(1024);
        let (upstream, mut origin) = tokio::io::duplex(1024);
        let h = handle();
        let relay = tokio::spawn(pump(
            Box::new(client),
            Box::new(upstream),
            h.clone(),
            Duration::from_secs(600),
        ));
        let bound = Duration::from_secs(5);
        // the client sends its request and half-closes
        app.write_all(b"request").await.unwrap();
        app.shutdown().await.unwrap();
        let mut got = Vec::new();
        tokio::time::timeout(bound, origin.read_to_end(&mut got))
            .await
            .expect("the half-close reaches the upstream")
            .unwrap();
        assert_eq!(got, b"request");
        // the upstream still answers, and the answer still arrives
        origin.write_all(b"response").await.unwrap();
        origin.shutdown().await.unwrap();
        let mut back = Vec::new();
        tokio::time::timeout(bound, app.read_to_end(&mut back))
            .await
            .expect("the other direction was ended by the first one's EOF")
            .unwrap();
        assert_eq!(back, b"response");
        tokio::time::timeout(bound, relay)
            .await
            .expect("both directions done")
            .unwrap();
        assert_eq!(h.outcome(), Some(SessionOutcome::Completed));
    }

    /// Upstream ending before it answered, while the client waits, is
    /// upstream's failure (M3c design 4.3).
    #[tokio::test]
    async fn upstream_ending_silently_while_the_client_waits_is_marked() {
        let (ca, client_b) = tokio::io::duplex(1024);
        let (ua, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(30),
        ));
        drop(ua);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !h.upstream_failed() {
            assert!(Instant::now() < deadline, "not marked");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(!h.client_gone());
        drop(ca);
        task.await.unwrap();
    }

    /// The client leaving first is no failure of upstream's, and neither is
    /// upstream ending after it answered.
    #[tokio::test]
    async fn the_client_leaving_first_or_an_answer_is_no_upstream_failure() {
        let (ca, client_b) = tokio::io::duplex(1024);
        let (mut ua, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(30),
        ));
        drop(ca);
        let mut buf = [0u8; 1];
        assert_eq!(
            ua.read(&mut buf).await.unwrap(),
            0,
            "the half-close came through"
        );
        drop(ua);
        task.await.unwrap();
        assert!(h.client_gone());
        assert!(!h.upstream_failed());

        let (mut ca, client_b) = tokio::io::duplex(1024);
        let (mut ua, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(30),
        ));
        ua.write_all(b"hi").await.unwrap();
        let mut buf = [0u8; 2];
        ca.read_exact(&mut buf).await.unwrap();
        drop(ua);
        drop(ca);
        task.await.unwrap();
        assert!(!h.upstream_failed());
    }

    /// Not a test of anything: prints how fast `pump` moves bytes over
    /// loopback — to a plain TCP upstream and through three stacks that
    /// encrypt (TLS, a WebSocket inside TLS, VMess) — with each of four buffer
    /// sizes: the numbers `BUF` is chosen by. A run is one session that
    /// carries its bytes up and then as many down. The rounds are the outer
    /// loop and the sizes take turns inside it, each in every place of the
    /// order as often as any other, so that drift hits them all alike.
    ///
    /// Everything runs on one thread — the application, the relay, the fake
    /// server and the origin — so that their costs add up and what the relay
    /// saves shows in the total. With a thread for each, the slowest of them
    /// sets the pace, and behind an encrypting stack that is the fake
    /// server: nothing the relay did would show.
    ///
    /// The comparison of two flush timings was run once with an earlier form
    /// of this benchmark (commit `6752afa`) and found no difference.
    ///
    /// `cargo test -p rurge-engine --release --lib relay_throughput -- --ignored --nocapture`
    /// (`RELAY_BENCH_MIB=64` carries that many MiB each way instead of 256:
    /// for a machine on which the whole matrix would take hours.)
    #[tokio::test]
    #[ignore]
    async fn relay_throughput() {
        use rurge_config::spec::{Secret, TlsOpts, TrojanSpec, VmessCipher, VmessSpec, WsOpts};
        use rurge_net::connector::{ConnectOpts, DirectConnector, SystemResolve, Target};
        use rurge_proto::Outbound;
        use rurge_proto::testing::{FakeTrojan, FakeVmess, TlsFixture, TrojanScript, VmessScript};
        use rurge_proto::trojan::TrojanOutbound;
        use rurge_proto::vmess::VmessOutbound;
        use std::net::SocketAddr;
        use tokio::net::{TcpListener, TcpStream};

        const ROUNDS: usize = 8;
        const MIB: usize = 1024 * 1024;
        const BLOCK: usize = 64 * 1024;

        /// `TCP_NODELAY`, as the listeners and the connector set it on theirs.
        fn no_delay(stream: TcpStream) -> TcpStream {
            stream.set_nodelay(true).unwrap();
            stream
        }

        async fn pair() -> (TcpStream, TcpStream) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (a, b) = tokio::join!(TcpStream::connect(addr), listener.accept());
            (no_delay(a.unwrap()), no_delay(b.unwrap().0))
        }

        /// Writes `total` bytes into `into`, in a task of its own.
        fn send(mut into: TcpStream, total: usize) -> tokio::task::JoinHandle<TcpStream> {
            tokio::spawn(async move {
                let block = vec![7u8; BLOCK];
                for _ in 0..total / BLOCK {
                    into.write_all(&block).await.unwrap();
                }
                into
            })
        }

        /// Reads `total` bytes out of `from`.
        async fn receive(from: &mut TcpStream, total: usize) {
            let mut buf = vec![0u8; BLOCK];
            let mut got = 0;
            while got < total {
                let n = from.read(&mut buf).await.unwrap();
                assert!(n > 0, "the relay stopped at {got} bytes");
                got += n;
            }
        }

        fn target(addr: SocketAddr) -> Target {
            Target::new(HostName::Ip(addr.ip()), addr.port())
        }

        fn median(of: impl Iterator<Item = f64>) -> f64 {
            let mut sorted: Vec<f64> = of.collect();
            sorted.sort_by(f64::total_cmp);
            let mid = sorted.len() / 2;
            if sorted.len() % 2 == 0 {
                (sorted[mid - 1] + sorted[mid]) / 2.0
            } else {
                sorted[mid]
            }
        }

        let mib = std::env::var("RELAY_BENCH_MIB").map_or(256, |v| {
            v.parse::<usize>()
                .expect("RELAY_BENCH_MIB is a number of MiB")
        });
        let each_way = mib * MIB;
        let direct = || Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let mut trojan: Vec<(FakeTrojan, Box<dyn Outbound>)> = Vec::new();
        for ws in [false, true] {
            let fake = FakeTrojan::spawn(
                TrojanScript {
                    password: "pw".into(),
                    ws,
                    connect_to: None,
                },
                fixture.clone(),
            )
            .await;
            let spec = TrojanSpec {
                tls: TlsOpts::default(),
                password: Secret::from("pw"),
                ws: ws.then(|| WsOpts {
                    path: "/".into(),
                    headers: Vec::new(),
                }),
            };
            let outbound = TrojanOutbound::new(
                "T",
                target(fake.addr()),
                &spec,
                None,
                &[],
                fixture.roots(),
                direct(),
            )
            .unwrap();
            trojan.push((fake, Box::new(outbound)));
        }
        let script = VmessScript::new("0233d11c-15a4-47d3-ade3-48ffca0ce119");
        let spec = VmessSpec {
            uuid: Secret::new(script.uuid),
            cipher: VmessCipher::Aes128Gcm,
            tls: None,
            ws: None,
        };
        let fake_vmess = FakeVmess::spawn(script, None).await;
        let vmess = VmessOutbound::new(
            "V",
            target(fake_vmess.addr()),
            &spec,
            None,
            &[],
            fixture.roots(),
            direct(),
        )
        .unwrap();
        // the upstreams; `None` is the origin itself over plain TCP
        let stacks: [(&str, Option<&dyn Outbound>); 4] = [
            ("plain", None),
            ("tls", Some(trojan[0].1.as_ref())),
            ("ws", Some(trojan[1].1.as_ref())),
            ("vmess", Some(&vmess)),
        ];
        let sizes = [8, 16, 32, 64].map(|kib| kib * 1024);

        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        // seconds[stack][size]: every round's (up, down)
        let mut seconds = vec![vec![Vec::new(); sizes.len()]; stacks.len()];
        let began = std::time::Instant::now();
        for round in 0..ROUNDS {
            for s in (0..stacks.len()).map(|i| (i + round) % stacks.len()) {
                for c in (0..sizes.len()).map(|i| (i + round) % sizes.len()) {
                    let (name, outbound) = stacks[s];
                    let size = sizes[c];
                    let run = async {
                        let (app, client) = pair().await;
                        let upstream: BoxedStream = match outbound {
                            None => {
                                Box::new(no_delay(TcpStream::connect(origin_addr).await.unwrap()))
                            }
                            Some(outbound) => outbound
                                .connect_tcp(&target(origin_addr), &ConnectOpts::default())
                                .await
                                .unwrap(),
                        };
                        let relay = tokio::spawn(pump_sized(
                            Box::new(client),
                            upstream,
                            handle(),
                            Duration::from_secs(600),
                            size,
                        ));
                        let started = std::time::Instant::now();
                        let sending = send(app, each_way);
                        // a proxy connects to the origin once it has the
                        // request, and that comes with the first bytes
                        let mut far = no_delay(origin.accept().await.unwrap().0);
                        receive(&mut far, each_way).await;
                        let up = started.elapsed().as_secs_f64();
                        let mut app = sending.await.unwrap();
                        let started = std::time::Instant::now();
                        let sending = send(far, each_way);
                        receive(&mut app, each_way).await;
                        let down = started.elapsed().as_secs_f64();
                        // The application leaves first and the origin once
                        // it has seen that: every connection is then closed
                        // from the side that opened it, and none waits out
                        // its time on a listener's side, where a later run's
                        // connection from the same port would get no answer.
                        drop(app);
                        let mut far = sending.await.unwrap();
                        assert_eq!(far.read(&mut [0u8; 1]).await.unwrap(), 0);
                        drop(far);
                        relay.await.unwrap();
                        (up, down)
                    };
                    let (up, down) = tokio::time::timeout(Duration::from_secs(1800), run)
                        .await
                        .unwrap_or_else(|_| {
                            panic!("{name} {} KiB: the run never ended", size / 1024)
                        });
                    println!(
                        "round {} {name:<5} {:>2} KiB: up {:>7.1}  down {:>7.1}  MiB/s",
                        round + 1,
                        size / 1024,
                        mib as f64 / up,
                        mib as f64 / down,
                    );
                    seconds[s][c].push((up, down));
                }
            }
        }

        println!(
            "\nrelay throughput in MiB/s: {mib} MiB each way, {ROUNDS} rounds, {:.0} s in all",
            began.elapsed().as_secs_f64()
        );
        println!(
            "{:<5} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>9}  every round",
            "stack", "buffer", "median", "min", "max", "up", "down", "of BUF"
        );
        let default = sizes
            .iter()
            .position(|&size| size == BUF)
            .expect("BUF is one of the sizes");
        for (s, (name, _)) in stacks.iter().enumerate() {
            // both ways together: all the bytes over all the time
            let both = |c: usize| -> Vec<f64> {
                seconds[s][c]
                    .iter()
                    .map(|(up, down)| 2.0 * mib as f64 / (up + down))
                    .collect()
            };
            let of_default = median(both(default).into_iter());
            for (c, size) in sizes.iter().enumerate() {
                let rounds = both(c);
                let mid = median(rounds.iter().copied());
                println!(
                    "{name:<5} {:>2} KiB {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1}%  {}",
                    size / 1024,
                    mid,
                    rounds.iter().copied().fold(f64::INFINITY, f64::min),
                    rounds.iter().copied().fold(0.0, f64::max),
                    median(seconds[s][c].iter().map(|(up, _)| mib as f64 / up)),
                    median(seconds[s][c].iter().map(|(_, down)| mib as f64 / down)),
                    100.0 * mid / of_default,
                    rounds
                        .iter()
                        .map(|r| format!("{r:.1}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
        }
    }
}
