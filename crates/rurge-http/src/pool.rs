//! The upstream connections one client connection keeps between its requests
//! (`phase 4 M1 design 8.5`). A pool belongs to one client connection and
//! holds at most `MAX_IDLE` connections that are ready for another request of
//! the same key; what a key is the engine decides. Nothing here reads the
//! clock: `take` and `put` are told the moment, so a test can age a connection
//! by ninety seconds without waiting for it.
//!
//! `Metered` is how a pooled stream enters the pool: HTTP/1.1 lends one
//! connection to one request at a time, so the bytes read and written count
//! onto whichever request holds it right then (`Meter`).

use rurge_inbound::SessionHandle;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// How many idle connections one client connection keeps. With more, the one
/// put first goes.
pub const MAX_IDLE: usize = 4;

/// How long an idle connection may wait for another request: one put back at
/// `t` is not lent at `t + IDLE_FOR` or after.
pub const IDLE_FOR: Duration = Duration::from_secs(90);

/// What the pool keeps has to answer two questions: is it closed (it cannot
/// carry another request) and is it ready (nothing else is on it right now).
pub trait Reusable {
    fn is_closed(&self) -> bool;
    fn is_ready(&self) -> bool;
}

/// The idle upstream connections of one client connection, by key, oldest
/// first. Taking one out hands it to the request that asks; putting one in
/// records when it became idle.
pub struct Pool<K, C> {
    idle: Vec<(K, C, Instant)>,
}

impl<K, C> Pool<K, C> {
    pub fn new() -> Pool<K, C> {
        Pool { idle: Vec::new() }
    }

    /// How many connections are idle now. Nothing is dropped to answer.
    pub fn len(&self) -> usize {
        self.idle.len()
    }

    pub fn is_empty(&self) -> bool {
        self.idle.is_empty()
    }
}

impl<K, C> Default for Pool<K, C> {
    fn default() -> Pool<K, C> {
        Pool::new()
    }
}

impl<K: PartialEq, C: Reusable> Pool<K, C> {
    /// The first idle connection of `key` that is ready, taken out of the
    /// pool. Closed and expired ones met on the way are dropped; one that is
    /// not ready stays, for a request that comes later.
    pub fn take(&mut self, key: &K, now: Instant) -> Option<C> {
        self.clear(now);
        let at = self
            .idle
            .iter()
            .position(|(k, conn, _)| k == key && conn.is_ready())?;
        Some(self.idle.remove(at).1)
    }

    /// Puts a connection back, to be lent again until `IDLE_FOR` after `now`.
    /// A closed one is dropped; so is the one put first once there are more
    /// than `MAX_IDLE`, and expired ones met on the way.
    pub fn put(&mut self, key: K, conn: C, now: Instant) {
        self.clear(now);
        if conn.is_closed() {
            return;
        }
        self.idle.push((key, conn, now));
        while self.idle.len() > MAX_IDLE {
            self.idle.remove(0);
        }
    }

    /// Drops the connections that cannot be lent again: closed ones, and those
    /// idle for `IDLE_FOR` or more. A moment earlier than the one a connection
    /// was put at is no expiry.
    fn clear(&mut self, now: Instant) {
        self.idle.retain(|(_, conn, at)| {
            !conn.is_closed() && now.saturating_duration_since(*at) < IDLE_FOR
        });
    }
}

/// Which request a pooled connection is being used by right now, `None` while
/// it sits idle. Clones share it: the pool keeps one, the `Metered` stream
/// keeps one.
#[derive(Clone, Default)]
pub struct Meter(Arc<Mutex<Option<Arc<SessionHandle>>>>);

impl Meter {
    pub fn new() -> Meter {
        Meter::default()
    }

    /// Lends the connection to `handle`, or takes it back with `None`.
    pub fn set(&self, handle: Option<Arc<SessionHandle>>) {
        *self.0.lock().expect("meter") = handle;
    }

    /// The request the connection is lent to, if any. The lock is let go
    /// before the caller counts anything.
    fn current(&self) -> Option<Arc<SessionHandle>> {
        self.0.lock().expect("meter").clone()
    }
}

/// An upstream stream that counts its bytes onto the request the pool has lent
/// it to (`phase 4 M1 design 8.5`): bytes read are that request's down bytes,
/// the first of them its first byte; bytes written are its up bytes. With no
/// request holding the connection, nothing is counted.
pub struct Metered<S> {
    inner: S,
    meter: Meter,
}

impl<S> Metered<S> {
    pub fn new(inner: S, meter: Meter) -> Metered<S> {
        Metered { inner, meter }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Metered<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &res {
            let n = buf.filled().len() - before;
            if n > 0
                && let Some(handle) = self.meter.current()
            {
                handle.add_down(n as u64);
                handle.mark_first_byte();
            }
        }
        res
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Metered<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, data);
        if let Poll::Ready(Ok(n)) = &res
            && let Some(handle) = self.meter.current()
        {
            handle.add_up(*n as u64);
        }
        res
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use rurge_config::session::SessionInfo;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A connection the pool can be asked about, whose two answers change
    /// while it sits in the pool: the test keeps a handle of its own.
    #[derive(Clone)]
    struct Fake {
        n: u32,
        closed: Arc<AtomicBool>,
        ready: Arc<AtomicBool>,
    }

    impl Fake {
        fn new(n: u32) -> Fake {
            Fake {
                n,
                closed: Arc::new(AtomicBool::new(false)),
                ready: Arc::new(AtomicBool::new(true)),
            }
        }

        fn closed(self) -> Fake {
            self.closed.store(true, Ordering::SeqCst);
            self
        }

        fn not_ready(self) -> Fake {
            self.ready.store(false, Ordering::SeqCst);
            self
        }
    }

    impl Reusable for Fake {
        fn is_closed(&self) -> bool {
            self.closed.load(Ordering::SeqCst)
        }

        fn is_ready(&self) -> bool {
            self.ready.load(Ordering::SeqCst)
        }
    }

    fn lent(pool: &mut Pool<&'static str, Fake>, key: &'static str, now: Instant) -> Option<u32> {
        pool.take(&key, now).map(|conn| conn.n)
    }

    /// A connection is lent to a request of its own key only, and lent once.
    #[test]
    fn a_connection_is_lent_to_its_own_key_only() {
        let now = Instant::now();
        let mut pool = Pool::new();
        pool.put("a", Fake::new(1), now);
        pool.put("b", Fake::new(2), now);
        assert_eq!(lent(&mut pool, "a", now), Some(1));
        assert_eq!(lent(&mut pool, "a", now), None, "it was lent already");
        assert_eq!(lent(&mut pool, "b", now), Some(2));
        assert_eq!(pool.len(), 0);
    }

    /// A closed connection is dropped, both when it is put and when the pool
    /// is next asked for one; it is never lent.
    #[test]
    fn a_closed_connection_is_dropped_not_lent() {
        let now = Instant::now();
        let mut pool = Pool::new();
        pool.put("a", Fake::new(1).closed(), now);
        assert_eq!(pool.len(), 0, "a closed one is not kept");
        let conn = Fake::new(2);
        pool.put("a", conn.clone(), now);
        assert_eq!(pool.len(), 1);
        conn.closed.store(true, Ordering::SeqCst);
        assert_eq!(lent(&mut pool, "a", now), None);
        assert_eq!(pool.len(), 0, "the closed one went with the look");
    }

    /// A connection that is not ready yet is kept for later, not lent.
    #[test]
    fn a_connection_that_is_not_ready_is_skipped_and_kept() {
        let now = Instant::now();
        let mut pool = Pool::new();
        let conn = Fake::new(1).not_ready();
        pool.put("a", conn.clone(), now);
        assert_eq!(lent(&mut pool, "a", now), None);
        assert_eq!(pool.len(), 1, "kept for when it is ready");
        conn.ready.store(true, Ordering::SeqCst);
        assert_eq!(lent(&mut pool, "a", now), Some(1));
    }

    /// At most `MAX_IDLE` idle connections; the one put first goes.
    #[test]
    fn at_most_four_idle_connections_the_oldest_goes() {
        let now = Instant::now();
        let mut pool = Pool::new();
        for (n, key) in ["a", "b", "c", "d", "e"].into_iter().enumerate() {
            pool.put(key, Fake::new(n as u32 + 1), now);
        }
        assert_eq!(pool.len(), MAX_IDLE);
        assert_eq!(lent(&mut pool, "a", now), None, "the oldest went");
        assert_eq!(lent(&mut pool, "b", now), Some(2));
    }

    /// Ninety seconds is one too many: at eighty-nine the connection is
    /// lent, at ninety it is gone — and `put` clears the expired ones the
    /// same way.
    #[test]
    fn a_connection_idle_for_ninety_seconds_is_not_lent() {
        let t = Instant::now();
        let mut pool = Pool::new();
        pool.put("a", Fake::new(1), t);
        let last_moment = t + IDLE_FOR - Duration::from_secs(1);
        assert_eq!(lent(&mut pool, "a", last_moment), Some(1));
        pool.put("b", Fake::new(2), t);
        assert_eq!(lent(&mut pool, "b", t + IDLE_FOR), None);
        assert_eq!(pool.len(), 0);
        pool.put("c", Fake::new(3), t);
        pool.put("d", Fake::new(4), t + IDLE_FOR);
        assert_eq!(pool.len(), 1, "`put` dropped the expired one");
        assert_eq!(lent(&mut pool, "d", t + IDLE_FOR), Some(4));
    }

    /// The bytes of a pooled stream count onto the request it is lent to
    /// right then: the meter is changed under it, then cleared. The end of
    /// the stream is no byte: it neither counts nor marks the first one.
    #[tokio::test]
    async fn a_metered_stream_counts_onto_the_current_request_only() {
        let (mut client, upstream) = tokio::io::duplex(64);
        let meter = Meter::new();
        let a = SessionHandle::new(1, SessionInfo::tcp(HostName::parse("a.test"), 80));
        let b = SessionHandle::new(2, SessionInfo::tcp(HostName::parse("b.test"), 80));
        let mut metered = Metered::new(upstream, meter.clone());

        let mut buf = [0u8; 16];
        metered.write_all(b"out").await.unwrap();
        client.read_exact(&mut buf[..3]).await.unwrap();
        client.write_all(b"in").await.unwrap();
        metered.read_exact(&mut buf[..2]).await.unwrap();
        assert_eq!(
            (a.bytes(), b.bytes()),
            ((0, 0), (0, 0)),
            "no request has it lent"
        );

        meter.set(Some(a.clone()));
        metered.write_all(b"hello").await.unwrap();
        client.read_exact(&mut buf[..5]).await.unwrap();
        assert_eq!(a.bytes(), (5, 0), "bytes out belong to the request");
        assert!(!a.first_byte_seen(), "a write is not a response");
        client.write_all(b"ok").await.unwrap();
        metered.read_exact(&mut buf[..2]).await.unwrap();
        assert_eq!(a.bytes(), (5, 2));
        assert!(a.first_byte_seen());

        meter.set(Some(b.clone()));
        metered.write_all(b"x").await.unwrap();
        client.read_exact(&mut buf[..1]).await.unwrap();
        assert_eq!(b.bytes(), (1, 0), "the new request counts");
        assert_eq!(a.bytes(), (5, 2), "the old one does not");

        meter.set(None);
        metered.write_all(b"z").await.unwrap();
        client.read_exact(&mut buf[..1]).await.unwrap();
        assert_eq!(
            (a.bytes(), b.bytes()),
            ((5, 2), (1, 0)),
            "nobody has it lent"
        );

        client.shutdown().await.unwrap();
        assert_eq!(metered.read(&mut buf).await.unwrap(), 0, "the end");
        assert_eq!(b.bytes(), (1, 0));
        assert!(!b.first_byte_seen(), "the end is no first byte");
    }
}
