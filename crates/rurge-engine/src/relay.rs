//! The relay: a cancellable, idle-timed, byte-counting bidirectional copy
//! (M3 design §7.3). Each direction runs as its own future so a stalled write
//! on one side never blocks the other (no head-of-line blocking), and every
//! read and every write races against a stop token so `kill`, graceful
//! shutdown and the idle timeout all end a stuck session promptly. Replaces
//! M3a's `copy_bidirectional`.

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
            let r = tokio::join!(
                copy_half(
                    cr,
                    uw,
                    stop.clone(),
                    a_up,
                    started,
                    move |n| h_up.add_up(n),
                    sniff_first,
                ),
                copy_half(
                    ur,
                    cw,
                    stop.clone(),
                    a_down,
                    started,
                    move |n| h_down.add_down(n),
                    None,
                ),
            );
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
            let _ = writer.shutdown().await; // TCP FIN: does not wait on the peer
            return Ok(());
        }
        if let Some(f) = first.take() {
            f(&buf[..n]);
        }
        tokio::select! {
            biased;
            _ = &mut cancelled => return Ok(()),
            w = writer.write_all(&buf[..n]) => w?,
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
}
