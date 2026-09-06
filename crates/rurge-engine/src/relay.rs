//! The relay: a cancellable, idle-timed, byte-counting bidirectional copy
//! (M3 design §7.3). Replaces M3a's `copy_bidirectional` so a session can be
//! killed, drained on shutdown, and bounded by an idle timeout.

use rurge_inbound::{SessionHandle, SessionOutcome};
use rurge_net::connector::BoxedStream;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BUF: usize = 8 * 1024;

/// Copies bytes both ways until either side closes, the idle timer fires, or
/// the handle's token is cancelled; then finishes the handle. Counts client→
/// upstream as `up` and upstream→client as `down` as the bytes move.
pub async fn pump(
    mut client: BoxedStream,
    mut upstream: BoxedStream,
    handle: Arc<SessionHandle>,
    idle: Duration,
) {
    let mut cbuf = vec![0u8; BUF];
    let mut ubuf = vec![0u8; BUF];
    let mut client_open = true;
    let mut upstream_open = true;
    let token = handle.token().clone();
    let idle_timer = tokio::time::sleep(idle);
    tokio::pin!(idle_timer);

    let outcome = loop {
        if !client_open && !upstream_open {
            break SessionOutcome::Completed;
        }
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                break if handle.was_killed() {
                    SessionOutcome::Failed("killed".into())
                } else {
                    SessionOutcome::Completed
                };
            }
            _ = &mut idle_timer => {
                break SessionOutcome::Completed;
            }
            r = client.read(&mut cbuf), if client_open => match r {
                Ok(0) => {
                    let _ = upstream.shutdown().await;
                    client_open = false;
                }
                Ok(n) => {
                    if let Err(e) = upstream.write_all(&cbuf[..n]).await {
                        break SessionOutcome::Failed(e.to_string());
                    }
                    handle.add_up(n as u64);
                    idle_timer.as_mut().reset(tokio::time::Instant::now() + idle);
                }
                Err(e) => break SessionOutcome::Failed(e.to_string()),
            },
            r = upstream.read(&mut ubuf), if upstream_open => match r {
                Ok(0) => {
                    let _ = client.shutdown().await;
                    upstream_open = false;
                }
                Ok(n) => {
                    if let Err(e) = client.write_all(&ubuf[..n]).await {
                        break SessionOutcome::Failed(e.to_string());
                    }
                    handle.add_down(n as u64);
                    idle_timer.as_mut().reset(tokio::time::Instant::now() + idle);
                }
                Err(e) => break SessionOutcome::Failed(e.to_string()),
            },
        }
    };
    handle.finish(outcome);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use rurge_config::session::SessionInfo;
    use rurge_inbound::SessionHandle;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        assert_eq!(h.outcome(), Some(rurge_inbound::SessionOutcome::Completed));
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
        assert_eq!(
            h.outcome(),
            Some(rurge_inbound::SessionOutcome::Failed("killed".into()))
        );
    }
}
