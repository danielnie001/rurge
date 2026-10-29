//! REJECT-DROP for a session that was answered "connected" already (phase 4
//! M1 design 7.4): there is no reply left to hold, so the connection itself
//! is held.

use crate::session::ServeOpts;
use rurge_net::connector::BoxedStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// REJECT-DROP for a session that was answered "connected" already (phase
/// 4 M1 design 7.4): what the peer sends is read and discarded until it
/// closes its side, reading fails, `opts.stop` fires or `drop_hold` runs
/// out; then this side is shut down, and what the peer sends is read and
/// discarded for `linger` at the most, until it closes its side. The stop
/// ends it at once at any point.
pub async fn hold_dropped(mut stream: BoxedStream, opts: &ServeOpts) {
    // What is read has no reader: one buffer serves the whole hold, on the
    // heap so the future of a held session stays small.
    let mut buf = vec![0u8; 4096];
    let stopped = async {
        match &opts.stop {
            Some(stop) => stop.cancelled().await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(stopped);

    // The hold: everything the peer sends is read and dropped, so that no
    // window fills up and no reset ends it early.
    let hold = tokio::time::sleep(opts.drop_hold);
    tokio::pin!(hold);
    loop {
        tokio::select! {
            biased;
            _ = &mut stopped => return,
            _ = &mut hold => break,
            read = stream.read(&mut buf) => match read {
                // the peer closed its side, or is gone
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            },
        }
    }

    // A FIN, which a session that was refused would give as well. The peer is
    // gone by now often enough for this to fail; nothing is left to say.
    tokio::select! {
        biased;
        _ = &mut stopped => return,
        _ = stream.shutdown() => {}
    }

    // A TUN gives its applications time to close their side of a connection
    // that was shut down, so that letting it go does not reset it
    // (`close_and_wait_for_the_app`); the listeners give none.
    if opts.linger.is_zero() {
        return;
    }
    let linger = tokio::time::sleep(opts.linger);
    tokio::pin!(linger);
    loop {
        tokio::select! {
            biased;
            _ = &mut stopped => return,
            _ = &mut linger => return,
            read = stream.read(&mut buf) => match read {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::sync::CancellationToken;

    fn opts(stop: Option<CancellationToken>, drop_hold: Duration, linger: Duration) -> ServeOpts {
        ServeOpts {
            stop,
            drop_hold,
            linger,
        }
    }

    /// A stop ends the hold there and then, whatever stage it is in.
    #[tokio::test]
    async fn the_hold_ends_at_the_stop() {
        let (_peer, server) = tokio::io::duplex(64);
        let stop = CancellationToken::new();
        let held = tokio::spawn({
            let opts = opts(
                Some(stop.clone()),
                Duration::from_secs(30),
                Duration::from_secs(30),
            );
            async move { hold_dropped(Box::new(server), &opts).await }
        });
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(1), held)
            .await
            .expect("the hold ended within 1 s of the stop")
            .unwrap();
    }

    /// The peer closing its side ends the hold well before `drop_hold`, and
    /// its own side of the connection is closed by then.
    #[tokio::test]
    async fn the_hold_ends_when_the_peer_closes() {
        let (mut peer, server) = tokio::io::duplex(64);
        let held = tokio::spawn(async move {
            let opts = opts(None, Duration::from_secs(30), Duration::ZERO);
            hold_dropped(Box::new(server), &opts).await
        });
        peer.write_all(b"early bytes").await.unwrap();
        peer.shutdown().await.unwrap();
        let mut buf = [0u8; 16];
        let n = tokio::time::timeout(Duration::from_secs(5), peer.read(&mut buf))
            .await
            .expect("the peer's side was closed within 5 s of the peer closing its own")
            .unwrap();
        assert_eq!(n, 0, "the hold closed its side");
        tokio::time::timeout(Duration::from_secs(5), held)
            .await
            .expect("the hold ended well before the 30 s drop_hold")
            .unwrap();
    }

    /// Nothing the peer does: the hold lasts exactly `drop_hold` and then
    /// closes its side.
    #[tokio::test]
    async fn the_hold_lasts_drop_hold_and_then_shuts_down() {
        let (mut peer, server) = tokio::io::duplex(64);
        let started = Instant::now();
        let held = tokio::spawn(async move {
            let opts = opts(None, Duration::from_millis(200), Duration::ZERO);
            hold_dropped(Box::new(server), &opts).await
        });
        let mut buf = [0u8; 16];
        let n = tokio::time::timeout(Duration::from_secs(5), peer.read(&mut buf))
            .await
            .expect("the peer's side was closed within 5 s")
            .unwrap();
        assert_eq!(n, 0, "the hold closed its side");
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(200), "{took:?}");
        assert!(took < Duration::from_secs(5), "{took:?}");
        tokio::time::timeout(Duration::from_secs(5), held)
            .await
            .expect("the hold ended")
            .unwrap();
    }

    /// After the hold the peer is still there: what it sends is read and
    /// discarded, and its closing ends the wait well before `linger`.
    #[tokio::test]
    async fn the_linger_reads_until_the_peer_closes() {
        let (mut peer, server) = tokio::io::duplex(64);
        let held = tokio::spawn(async move {
            let opts = opts(None, Duration::from_millis(100), Duration::from_secs(30));
            hold_dropped(Box::new(server), &opts).await
        });
        let mut buf = [0u8; 16];
        let n = tokio::time::timeout(Duration::from_secs(5), peer.read(&mut buf))
            .await
            .expect("the peer's side was closed within 5 s")
            .unwrap();
        assert_eq!(n, 0, "the hold closed its side when the hold ran out");
        peer.write_all(b"late bytes").await.unwrap();
        peer.shutdown().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), held)
            .await
            .expect("the hold ended well before the 30 s linger")
            .unwrap();
    }
}
