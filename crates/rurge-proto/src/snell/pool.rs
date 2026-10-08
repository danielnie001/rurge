//! Idle Snell connections of one outbound (`reuse=true` on v4 and v5,
//! always on v2; phase 2 M6 design 4.3): the newest is reused first, at
//! most `MAX_IDLE` are kept, and one that has idled for a minute is closed
//! (as anytls's pool).
//!
//! The server may close an idle connection at any moment; `take` skips one
//! whose close has already arrived, and a request on a connection that dies
//! before its answer is sent again on a fresh one (`tunnel`).

use super::record::SnellStream;
use crate::task::AbortOnDrop;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Waker};
use std::time::Duration;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::time::Instant;

pub(crate) const REAP_EVERY: Duration = Duration::from_secs(30);
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Idle connections kept per outbound; the oldest goes first.
pub(crate) const MAX_IDLE: usize = 8;

#[derive(Default)]
pub(crate) struct Pool {
    idle: Mutex<Vec<(SnellStream, Instant)>>,
}

impl Pool {
    /// Only a stream that `is_reusable`; it is taken back ready for the next
    /// request.
    pub(crate) fn put(&self, mut stream: SnellStream) {
        stream.next_tunnel();
        let mut idle = self.idle.lock().expect("pool");
        if idle.len() == MAX_IDLE {
            idle.remove(0);
        }
        idle.push((stream, Instant::now()));
    }

    /// The newest connection that has not idled too long and has heard
    /// nothing from the server since it was pooled; the others found on the
    /// way are dropped (the look happens outside the lock).
    pub(crate) fn take(&self) -> Option<SnellStream> {
        let now = Instant::now();
        loop {
            let (mut stream, since) = self.idle.lock().expect("pool").pop()?;
            if now.duration_since(since) < IDLE_TIMEOUT && is_quiet(&mut stream) {
                return Some(stream);
            }
        }
    }

    pub(crate) fn reap(&self, now: Instant) {
        self.idle
            .lock()
            .expect("pool")
            .retain(|(_, since)| now.duration_since(*since) < IDLE_TIMEOUT);
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn len(&self) -> usize {
        self.idle.lock().expect("pool").len()
    }
}

/// Between two requests the server says nothing: a read that is not
/// pending is its close, an error, or bytes out of turn. The read never
/// waits: a pending one leaves a waker that does nothing.
///
/// A peer that has sent only part of a record looks quiet too, and the part
/// stays read for the next request on the connection. When the server then
/// closes, the record is cut short: a transport failure, so within
/// `STALE_WINDOW` the request goes again on a fresh connection (`tunnel`).
/// When the rest arrives, the record is read in front of the request's
/// answer, and one that fails to decrypt fails the request, not retried.
/// A server that keeps quiet between requests never gets there.
fn is_quiet(stream: &mut SnellStream) -> bool {
    let mut byte = [0u8; 1];
    let mut buf = ReadBuf::new(&mut byte);
    let mut cx = Context::from_waker(Waker::noop());
    Pin::new(stream).poll_read(&mut cx, &mut buf).is_pending()
}

/// Reaps `pool` until it is gone. Holds it weakly: the pool dies with its outbound.
pub(crate) fn spawn_reaper(pool: &Arc<Pool>) -> AbortOnDrop {
    let pool = Arc::downgrade(pool);
    AbortOnDrop(tokio::spawn(async move {
        let mut tick = tokio::time::interval(REAP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(pool) = pool.upgrade() else {
                return;
            };
            pool.reap(Instant::now());
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::super::kdf::{Psk, derive_key};
    use super::super::record::{Format, V4Stream};
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn keyed(pipe: tokio::io::DuplexStream, salt: [u8; 16]) -> SnellStream {
        let key = derive_key(b"psk", &salt);
        SnellStream::V4(V4Stream::new(
            Box::new(pipe),
            Psk::new("psk"),
            salt,
            key,
            Vec::new(),
        ))
    }

    /// A client stream whose request has ended both ways, and the server's
    /// stream on the other end of the pipe (dropping it is the close).
    async fn ended() -> (SnellStream, SnellStream) {
        let (near, far) = tokio::io::duplex(4096);
        let (mut client, mut server) = (keyed(near, [7; 16]), keyed(far, [9; 16]));
        for side in [&mut client, &mut server] {
            std::future::poll_fn(|cx| side.poll_end(cx)).await.unwrap();
        }
        let mut buf = [0u8; 1];
        for side in [&mut client, &mut server] {
            assert_eq!(side.read(&mut buf).await.unwrap(), 0);
        }
        assert!(client.is_reusable());
        (client, server)
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_connection_expires_after_a_minute() {
        let pool = Pool::default();
        let (stream, _server) = ended().await;
        pool.put(stream);
        tokio::time::advance(IDLE_TIMEOUT - Duration::from_secs(1)).await;
        assert!(pool.take().is_some(), "still fresh");
        let (stream, _server) = ended().await;
        pool.put(stream);
        tokio::time::advance(IDLE_TIMEOUT).await;
        assert!(pool.take().is_none(), "expired");
        // the reaper's pass drops it too
        let (stream, _server) = ended().await;
        pool.put(stream);
        tokio::time::advance(IDLE_TIMEOUT).await;
        pool.reap(Instant::now());
        assert_eq!(pool.len(), 0);
    }

    #[tokio::test]
    async fn a_connection_the_server_closed_is_not_taken() {
        let pool = Pool::default();
        let (open, _server) = ended().await;
        let (closed, server) = ended().await;
        pool.put(open);
        pool.put(closed);
        drop(server);
        // the newest is closed: skipped and dropped
        assert!(pool.take().is_some());
        assert_eq!(pool.len(), 0);
    }

    /// Version 2's connections are pooled as v4's are: one that is still
    /// open and quiet is taken, one the server closed while it idled is not.
    #[tokio::test]
    async fn a_chunked_connection_is_taken_while_quiet_and_dropped_once_closed() {
        const BOUND: Duration = Duration::from_secs(30);
        async fn ended() -> (SnellStream, SnellStream) {
            let (near, far) = tokio::io::duplex(4096);
            let open = |end| SnellStream::open(Box::new(end), Psk::new("psk"), Format::ChunkedAes);
            let (mut client, mut server) = (open(near).await.unwrap(), open(far).await.unwrap());
            for side in [&mut client, &mut server] {
                std::future::poll_fn(|cx| side.poll_end(cx)).await.unwrap();
            }
            let mut buf = [0u8; 1];
            for side in [&mut client, &mut server] {
                let read = tokio::time::timeout(BOUND, side.read(&mut buf)).await;
                assert_eq!(read.expect("the peer's end within the bound").unwrap(), 0);
            }
            assert!(client.is_reusable());
            (client, server)
        }
        let pool = Pool::default();
        let (open, mut server) = ended().await;
        let (closed, gone) = ended().await;
        pool.put(open);
        pool.put(closed);
        drop(gone);
        // the newest is closed: skipped and dropped
        let mut taken = pool.take().expect("the open one");
        assert_eq!(pool.len(), 0);
        // and what was taken carries the next request
        server.next_tunnel();
        taken.write_all(b"next").await.unwrap();
        let mut got = [0u8; 4];
        tokio::time::timeout(BOUND, server.read_exact(&mut got))
            .await
            .expect("the request within the bound")
            .unwrap();
        assert_eq!(&got, b"next");
    }

    #[tokio::test]
    async fn at_most_eight_idle_the_oldest_going_first() {
        let pool = Pool::default();
        let mut servers = Vec::new();
        for _ in 0..MAX_IDLE + 2 {
            let (stream, server) = ended().await;
            pool.put(stream);
            servers.push(server);
        }
        assert_eq!(pool.len(), MAX_IDLE);
        let mut buf = [0u8; 1];
        // the oldest was closed: its server reads the close
        let oldest = &mut servers[0];
        oldest.next_tunnel();
        assert_eq!(oldest.read(&mut buf).await.unwrap(), 0);
        // the newest is still open: nothing to read
        let newest = servers.last_mut().unwrap();
        newest.next_tunnel();
        let read = tokio::time::timeout(Duration::from_millis(50), newest.read(&mut buf)).await;
        assert!(read.is_err(), "still open");
    }
}
