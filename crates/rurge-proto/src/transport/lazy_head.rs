//! A protocol's request head held back until the first payload write, so
//! both leave in one write: a lone head-sized first record is a known
//! traffic signature.
//!
//! A relay polls the read side the moment the tunnel exists, long before the
//! client's first bytes arrive, so a read must not send the head at once.
//! It waits `HEAD_GRACE` for a write instead; only an application that stays
//! silent that long (a server-speaks-first protocol: SSH, SMTP) gets the head
//! sent on its own. The write that sends the head wakes a reader parked on
//! the timer, so the grace never delays a response.
//!
//! The two halves of a split stream may be polled by tasks of their own, while
//! the stream below keeps one waker for its write side: whoever polled it last
//! replaces the other's. So while the head is unsent, or the flush it owes is
//! pending, both halves poll that side with a waker that wakes them both, and
//! whoever finishes the head or the flush — the writer's own flush or
//! shutdown does — wakes the other half.

use rurge_net::connector::BoxedStream;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker, ready};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

/// How long a read waits for the first payload before the head goes out alone.
pub const HEAD_GRACE: Duration = Duration::from_millis(100);

/// The two halves of a split stream: the side a method belongs to, not the
/// task that polls it — Snell's `SnellTunnel::drive` calls the write side
/// from its read.
#[derive(Clone, Copy)]
enum Half {
    Read,
    Write,
}

impl Half {
    fn other(self) -> Half {
        match self {
            Half::Read => Half::Write,
            Half::Write => Half::Read,
        }
    }
}

/// The wakers of the halves that wait for the head or the flush, indexed by
/// `Half`. As a waker it wakes both: it is what the stream below is polled
/// with, as it keeps only the last waker it was given.
#[derive(Default)]
struct Both(Mutex<[Option<Waker>; 2]>);

impl Both {
    fn slots(&self) -> MutexGuard<'_, [Option<Waker>; 2]> {
        // a poisoned lock still holds wakers that are fine to use
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `half` waits: `waker` is the one to wake it.
    fn park(&self, half: Half, waker: &Waker) {
        let replaced = {
            let mut slots = self.slots();
            let slot = &mut slots[half as usize];
            if slot.as_ref().is_some_and(|parked| parked.will_wake(waker)) {
                return;
            }
            slot.replace(waker.clone())
        };
        // not under the lock: dropping a waker may run anything
        drop(replaced);
    }

    /// `half` is done waiting, with the head or the flush it was driving
    /// finished or failed: the other half may wait for just that.
    fn release(&self, half: Half) {
        let (own, other) = {
            let mut slots = self.slots();
            (
                slots[half as usize].take(),
                slots[half.other() as usize].take(),
            )
        };
        drop(own);
        if let Some(other) = other {
            other.wake();
        }
    }
}

impl Wake for Both {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // not under the lock: a waker may run anything, polling this stream among it
        let parked = std::mem::take(&mut *self.slots());
        for waker in parked.into_iter().flatten() {
            waker.wake();
        }
    }
}

/// Over a `BoxedStream` unless a protocol needs its own stream back
/// (`into_inner`: Snell's reused connections).
pub struct LazyHead<S = BoxedStream> {
    inner: S,
    /// `Some` until the head (and whatever payload rides with it) is on the wire.
    head: Option<Vec<u8>>,
    /// How much of `head` has been written.
    written: usize,
    /// Set by the first `inner.poll_write` of the head: from then on `head`
    /// must not grow — an inner write that returned `Pending` is retried
    /// with the same bytes (`WsByteStream` relies on that).
    started: bool,
    /// Payload bytes appended to `head` that the writer has not been told about yet.
    coalesced: usize,
    grace: Duration,
    /// Armed by the first read that finds the head unsent.
    timer: Option<Pin<Box<Sleep>>>,
    /// Where each half waits for the head or the flush; whoever finishes
    /// either wakes the other half, even one in another task.
    both: Arc<Both>,
    /// `both` as the waker for the stream below, while the head is unsent or
    /// a flush is owed.
    woken: Waker,
    /// The reader pushed the head out and its flush has not finished: the
    /// head may still be held back below, and the next read finishes the flush
    /// before it reads — unless a flush or a shutdown of the writer's has
    /// finished it first.
    owed_flush: bool,
}

impl<S: AsyncRead + AsyncWrite + Unpin> LazyHead<S> {
    pub fn new(inner: S, head: Vec<u8>) -> LazyHead<S> {
        LazyHead::with_grace(inner, head, HEAD_GRACE)
    }

    pub fn with_grace(inner: S, head: Vec<u8>, grace: Duration) -> LazyHead<S> {
        let both = Arc::new(Both::default());
        LazyHead {
            inner,
            head: Some(head),
            written: 0,
            started: false,
            coalesced: 0,
            grace,
            timer: None,
            woken: Waker::from(both.clone()),
            both,
            owed_flush: false,
        }
    }

    /// The stream below; write-side polls made through it bypass the shared
    /// waker while the head is unsent or a flush is owed, so a caller polls
    /// the write side itself only after `poll_flush` returned `Ready(Ok(()))`,
    /// which leaves neither, as Snell's `poll_end` does.
    pub fn get_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// The stream below; a head not yet sent is dropped with its payload.
    pub fn into_inner(self) -> S {
        self.inner
    }

    /// Drives the pending head (and whatever payload rides with it) out, on
    /// behalf of `half`.
    fn poll_head(&mut self, cx: &mut Context<'_>, half: Half) -> Poll<io::Result<()>> {
        while let Some(head) = &self.head {
            if self.written == head.len() {
                self.head = None;
                self.timer = None;
                // the other half may be waiting for just this
                self.both.release(half);
                break;
            }
            self.started = true;
            // parked before the poll: the stream below may wake right away
            self.both.park(half, cx.waker());
            let mut shared = Context::from_waker(&self.woken);
            let written =
                match Pin::new(&mut self.inner).poll_write(&mut shared, &head[self.written..]) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(0)) => Err(io::ErrorKind::WriteZero.into()),
                    Poll::Ready(result) => result,
                };
            match written {
                Ok(n) => self.written += n,
                Err(error) => {
                    self.both.release(half);
                    return Poll::Ready(Err(error));
                }
            }
        }
        Poll::Ready(Ok(()))
    }

    /// `poll` on the stream below for a write-side call of `half` (a write, a
    /// flush, the shutdown): with the waker of both halves while the head is
    /// unsent or a flush is owed, else with `cx`.
    fn poll_write_side<R>(
        &mut self,
        half: Half,
        cx: &mut Context<'_>,
        poll: impl FnOnce(Pin<&mut S>, &mut Context<'_>) -> Poll<R>,
    ) -> Poll<R> {
        if self.head.is_none() && !self.owed_flush {
            return poll(Pin::new(&mut self.inner), cx);
        }
        self.both.park(half, cx.waker());
        poll(
            Pin::new(&mut self.inner),
            &mut Context::from_waker(&self.woken),
        )
    }

    /// A flush or a shutdown of `half` on the stream below is over, with
    /// success or an error. If the reader's flush was owed, that was it: the
    /// other half may stop waiting for it. A write is not a flush and
    /// settles nothing.
    fn settle_flush(&mut self, half: Half) {
        if self.owed_flush {
            self.owed_flush = false;
            self.both.release(half);
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for LazyHead<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.head.is_some() {
            if !self.started {
                // nothing sent yet: give the first payload a moment to arrive
                let grace = self.grace;
                let timer = self
                    .timer
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(grace)));
                if timer.as_mut().poll(cx).is_pending() {
                    // the write that sends the head wakes us
                    self.both.park(Half::Read, cx.waker());
                    return Poll::Pending;
                }
            }
            // whoever finishes the head wakes us, even from another task
            ready!(self.poll_head(cx, Half::Read))?;
            self.owed_flush = true;
        }
        if self.owed_flush {
            // `head` is gone, so a flush that pends is picked up again here
            let flushed =
                ready!(self.poll_write_side(Half::Read, cx, |inner, cx| inner.poll_flush(cx)));
            self.settle_flush(Half::Read);
            flushed?;
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for LazyHead<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.head.is_none() && self.coalesced == 0 && !self.owed_flush {
            return Pin::new(&mut self.inner).poll_write(cx, data);
        }
        if !self.started && self.coalesced == 0 {
            // the first write, and the head has not started: the payload rides with it
            if let Some(head) = &mut self.head {
                head.extend_from_slice(data);
                self.coalesced = data.len();
            }
        }
        // a no-op once the head is out — possibly finished by a concurrent read
        ready!(self.poll_head(cx, Half::Write))?;
        if self.coalesced > 0 {
            // these bytes left with the head, whoever drove it: never again
            let n = self.coalesced.min(data.len());
            self.coalesced = 0;
            return Poll::Ready(Ok(n));
        }
        self.poll_write_side(Half::Write, cx, |inner, cx| inner.poll_write(cx, data))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.poll_head(cx, Half::Write))?;
        let flushed =
            ready!(self.poll_write_side(Half::Write, cx, |inner, cx| inner.poll_flush(cx)));
        self.settle_flush(Half::Write);
        Poll::Ready(flushed)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.poll_head(cx, Half::Write))?;
        let shut =
            ready!(self.poll_write_side(Half::Write, cx, |inner, cx| inner.poll_shutdown(cx)));
        self.settle_flush(Half::Write);
        Poll::Ready(shut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// Accepts one connection; returns (what the first read got, the rest).
    /// Says `banner` right after the first read.
    async fn peer() -> (
        std::net::SocketAddr,
        tokio::task::JoinHandle<(Vec<u8>, Vec<u8>)>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut first = vec![0u8; 256];
            let n = tcp.read(&mut first).await.unwrap();
            first.truncate(n);
            // the client may already be gone
            let _ = tcp.write_all(b"banner").await;
            let mut rest = Vec::new();
            let _ = tcp.read_to_end(&mut rest).await;
            (first, rest)
        });
        (addr, task)
    }

    #[tokio::test]
    async fn the_head_leaves_with_the_first_payload() {
        let (addr, task) = peer().await;
        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut lazy = LazyHead::new(Box::new(tcp), b"HEAD|".to_vec());
        lazy.write_all(b"payload").await.unwrap();
        let mut banner = [0u8; 6];
        lazy.read_exact(&mut banner).await.unwrap();
        lazy.write_all(b"+more").await.unwrap();
        lazy.shutdown().await.unwrap();
        drop(lazy);
        let (first, rest) = task.await.unwrap();
        assert_eq!(first, b"HEAD|payload", "one write, so one segment");
        assert_eq!(rest, b"+more");
    }

    #[tokio::test]
    async fn a_read_that_is_already_pending_does_not_send_the_head_alone() {
        // what a relay does: the read half is polled first, the payload follows
        let (addr, task) = peer().await;
        let tcp = TcpStream::connect(addr).await.unwrap();
        // a grace far longer than the test: only the writer can release the reader
        let lazy = LazyHead::with_grace(Box::new(tcp), b"HEAD|".to_vec(), Duration::from_secs(30));
        let (mut rd, mut wr) = tokio::io::split(lazy);
        let reader = tokio::spawn(async move {
            let mut banner = [0u8; 6];
            rd.read_exact(&mut banner).await.unwrap();
            (rd, banner)
        });
        // not a synchronisation: this only biases the interleaving towards
        // "the reader parked first", the case worth exercising. Whichever
        // half runs first, every assertion below holds.
        tokio::time::sleep(Duration::from_millis(20)).await;
        wr.write_all(b"payload").await.unwrap();
        let (rd, banner) = tokio::time::timeout(Duration::from_secs(5), reader)
            .await
            .expect("the write released the parked reader, not the 30 s timer")
            .unwrap();
        assert_eq!(&banner, b"banner");
        let mut lazy = rd.unsplit(wr);
        lazy.shutdown().await.unwrap();
        drop(lazy);
        let (first, _) = task.await.unwrap();
        assert_eq!(
            first, b"HEAD|payload",
            "coalesced although a read was pending"
        );
    }

    #[tokio::test]
    async fn an_application_that_stays_silent_gets_the_head_out_after_the_grace() {
        // a server-speaks-first protocol (SSH, SMTP): without this the two
        // sides would wait for each other forever
        let (addr, task) = peer().await;
        let tcp = TcpStream::connect(addr).await.unwrap();
        let grace = Duration::from_millis(50);
        let mut lazy = LazyHead::with_grace(Box::new(tcp), b"HEAD|".to_vec(), grace);
        let started = std::time::Instant::now();
        let mut banner = [0u8; 6];
        tokio::time::timeout(Duration::from_secs(5), lazy.read_exact(&mut banner))
            .await
            .expect("the head went out, so the peer answered")
            .unwrap();
        assert_eq!(&banner, b"banner");
        assert!(started.elapsed() >= grace, "the grace was honoured");
        lazy.write_all(b"later").await.unwrap();
        lazy.shutdown().await.unwrap();
        drop(lazy);
        let (first, rest) = task.await.unwrap();
        assert_eq!(
            (first.as_slice(), rest.as_slice()),
            (&b"HEAD|"[..], &b"later"[..])
        );
    }

    #[tokio::test]
    async fn a_shutdown_before_anything_else_still_sends_the_head() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut all = Vec::new();
            tcp.read_to_end(&mut all).await.unwrap();
            all
        });
        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut lazy = LazyHead::new(Box::new(tcp), b"HEAD|".to_vec());
        lazy.shutdown().await.unwrap();
        drop(lazy);
        assert_eq!(server.await.unwrap(), b"HEAD|");
    }

    use std::task::Waker;
    use tokio::io::DuplexStream;

    /// A duplex pipe whose `poll_write` follows a script: `Take(n)` accepts at
    /// most `n` bytes, `Park` returns `Pending` without taking anything; once
    /// the script is used up everything passes through.
    enum Step {
        Take(usize),
        Park,
    }

    struct Scripted {
        pipe: DuplexStream,
        script: std::collections::VecDeque<Step>,
    }

    impl Scripted {
        fn boxed(pipe: DuplexStream, script: impl IntoIterator<Item = Step>) -> BoxedStream {
            Box::new(Scripted {
                pipe,
                script: script.into_iter().collect(),
            })
        }
    }

    impl AsyncRead for Scripted {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.pipe).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for Scripted {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            match self.script.pop_front() {
                Some(Step::Park) => {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
                Some(Step::Take(n)) => {
                    let n = n.min(data.len());
                    Pin::new(&mut self.pipe).poll_write(cx, &data[..n])
                }
                None => Pin::new(&mut self.pipe).poll_write(cx, data),
            }
        }
        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.pipe).poll_flush(cx)
        }
        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.pipe).poll_shutdown(cx)
        }
    }

    async fn drain(mut far: DuplexStream) -> Vec<u8> {
        let mut all = Vec::new();
        far.read_to_end(&mut all).await.unwrap();
        all
    }

    #[tokio::test]
    async fn a_read_that_finishes_the_head_does_not_make_the_write_send_its_payload_again() {
        // the relay polls both halves of a split in one task: the write parks
        // with its payload already behind the head, the read drives both out
        let mut cx = Context::from_waker(Waker::noop());
        let (near, far) = tokio::io::duplex(4096);
        // two bytes of the head get out, then the inner write parks once;
        // the grace plays no part: the head is under way
        let mut lazy = LazyHead::with_grace(
            Scripted::boxed(near, [Step::Take(2), Step::Park]),
            b"HEAD|".to_vec(),
            Duration::from_secs(30),
        );
        assert!(
            Pin::new(&mut lazy)
                .poll_write(&mut cx, b"payload")
                .is_pending()
        );
        let mut space = [0u8; 8];
        let mut buf = ReadBuf::new(&mut space);
        // finishes the head, payload included; nothing to read yet
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut cx, &mut buf)
                .is_pending()
        );
        assert!(matches!(
            Pin::new(&mut lazy).poll_write(&mut cx, b"payload"),
            Poll::Ready(Ok(7))
        ));
        assert!(matches!(
            Pin::new(&mut lazy).poll_write(&mut cx, b"+more"),
            Poll::Ready(Ok(5))
        ));
        assert!(Pin::new(&mut lazy).poll_shutdown(&mut cx).is_ready());
        drop(lazy);
        assert_eq!(drain(far).await, b"HEAD|payload+more", "exactly once");
    }

    #[tokio::test]
    async fn the_head_does_not_grow_under_an_inner_write_that_is_pending() {
        let mut cx = Context::from_waker(Waker::noop());
        let (near, far) = tokio::io::duplex(4096);
        let mut lazy = LazyHead::with_grace(
            Scripted::boxed(near, [Step::Park]),
            b"HEAD|".to_vec(),
            Duration::from_secs(30),
        );
        // a flush starts the head on its own; the inner write parks with those 5 bytes
        assert!(Pin::new(&mut lazy).poll_flush(&mut cx).is_pending());
        assert!(matches!(
            Pin::new(&mut lazy).poll_write(&mut cx, b"payload"),
            Poll::Ready(Ok(7))
        ));
        assert!(Pin::new(&mut lazy).poll_shutdown(&mut cx).is_ready());
        drop(lazy);
        assert_eq!(drain(far).await, b"HEAD|payload");
    }

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A waker that counts how often it is woken.
    #[derive(Default)]
    struct Count(AtomicUsize);

    impl Count {
        fn waker() -> (Arc<Count>, Waker) {
            let count = Arc::new(Count::default());
            let waker = Waker::from(count.clone());
            (count, waker)
        }

        fn woken(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }
    }

    impl std::task::Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A stream that, like a socket, keeps one waker for its write direction:
    /// a write or a flush that finds its gate shut records the caller's waker,
    /// replacing whoever's was there, and returns `Pending`. A read never
    /// finishes.
    struct OneSlot {
        write_open: bool,
        flush_open: bool,
        /// Every write fails with this, gate or no gate.
        write_error: Option<io::ErrorKind>,
        /// Every flush fails with this, gate or no gate.
        flush_error: Option<io::ErrorKind>,
        slot: Option<Waker>,
        taken: Vec<u8>,
        flushes: usize,
    }

    impl OneSlot {
        fn new(write_open: bool, flush_open: bool) -> OneSlot {
            OneSlot {
                write_open,
                flush_open,
                write_error: None,
                flush_error: None,
                slot: None,
                taken: Vec::new(),
                flushes: 0,
            }
        }

        /// Wakes whoever the stream last recorded.
        fn wake_the_slot(&mut self) {
            self.slot.take().expect("someone parked").wake();
        }
    }

    impl AsyncRead for OneSlot {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    impl AsyncWrite for OneSlot {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            if let Some(kind) = self.write_error {
                return Poll::Ready(Err(kind.into()));
            }
            if self.write_open {
                self.taken.extend_from_slice(data);
                return Poll::Ready(Ok(data.len()));
            }
            self.slot = Some(cx.waker().clone());
            Poll::Pending
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            if let Some(kind) = self.flush_error {
                return Poll::Ready(Err(kind.into()));
            }
            if self.flush_open {
                self.flushes += 1;
                return Poll::Ready(Ok(()));
            }
            self.slot = Some(cx.waker().clone());
            Poll::Pending
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_writer_parked_on_the_head_is_woken_when_a_read_finishes_it() {
        // a split stream, its halves in tasks of their own: the writer's
        // poll_write owes it Ready(7) once the head, payload included, is out
        let (writer, writer_waker) = Count::waker();
        let (_, reader_waker) = Count::waker();
        let mut wcx = Context::from_waker(&writer_waker);
        let mut rcx = Context::from_waker(&reader_waker);
        let mut lazy = LazyHead::with_grace(
            OneSlot::new(false, true),
            b"HEAD|".to_vec(),
            Duration::from_secs(30),
        );
        // the writer starts the head and parks on the shut stream
        assert!(
            Pin::new(&mut lazy)
                .poll_write(&mut wcx, b"payload")
                .is_pending()
        );
        // the reader drives the same head and parks after it; the stream
        // keeps one write waker, whoever polled last
        let mut space = [0u8; 8];
        let mut buf = ReadBuf::new(&mut space);
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        // the stream can take bytes again and wakes whoever it kept
        lazy.get_mut().write_open = true;
        lazy.get_mut().wake_the_slot();
        // the woken reader finishes the head; there is nothing to read yet
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        assert_eq!(lazy.get_mut().taken, b"HEAD|payload");
        assert!(
            writer.woken() >= 1,
            "the writer was woken {} times, and its poll_write still owes it Ready(7)",
            writer.woken()
        );
    }

    #[tokio::test]
    async fn a_writer_is_woken_though_the_reader_that_parked_last_never_polls_again() {
        let (writer, writer_waker) = Count::waker();
        let (_, reader_waker) = Count::waker();
        let mut wcx = Context::from_waker(&writer_waker);
        let mut rcx = Context::from_waker(&reader_waker);
        let mut lazy = LazyHead::with_grace(
            OneSlot::new(false, true),
            b"HEAD|".to_vec(),
            Duration::from_secs(30),
        );
        assert!(
            Pin::new(&mut lazy)
                .poll_write(&mut wcx, b"payload")
                .is_pending()
        );
        let mut space = [0u8; 8];
        let mut buf = ReadBuf::new(&mut space);
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        lazy.get_mut().write_open = true;
        lazy.get_mut().wake_the_slot();
        // the reader is not polled again: the writer alone has to see it through
        assert!(
            writer.woken() >= 1,
            "the writer was woken {} times",
            writer.woken()
        );
        assert!(matches!(
            Pin::new(&mut lazy).poll_write(&mut wcx, b"payload"),
            Poll::Ready(Ok(7))
        ));
        assert_eq!(lazy.get_mut().taken, b"HEAD|payload");
    }

    #[tokio::test]
    async fn a_reader_whose_flush_is_pending_is_woken_though_a_write_parks_after_it() {
        let (_, writer_waker) = Count::waker();
        let (reader, reader_waker) = Count::waker();
        let mut wcx = Context::from_waker(&writer_waker);
        let mut rcx = Context::from_waker(&reader_waker);
        // neither the writes nor the flush get through yet
        let mut lazy = LazyHead::with_grace(
            OneSlot::new(false, false),
            b"HEAD|".to_vec(),
            Duration::from_secs(30),
        );
        assert!(
            Pin::new(&mut lazy)
                .poll_write(&mut wcx, b"payload")
                .is_pending()
        );
        let mut space = [0u8; 8];
        let mut buf = ReadBuf::new(&mut space);
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        // writes get through, the flush does not: the reader pushes the head
        // out and then waits on the flush it owes
        lazy.get_mut().write_open = true;
        lazy.get_mut().wake_the_slot();
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        assert_eq!(lazy.get_mut().taken, b"HEAD|payload");
        let woken_before = reader.woken();
        // the writer is told its payload left with the head
        assert!(matches!(
            Pin::new(&mut lazy).poll_write(&mut wcx, b"payload"),
            Poll::Ready(Ok(7))
        ));
        // and parks on the next write, after the reader parked on its flush
        lazy.get_mut().write_open = false;
        assert!(
            Pin::new(&mut lazy)
                .poll_write(&mut wcx, b"more")
                .is_pending()
        );
        lazy.get_mut().write_open = true;
        lazy.get_mut().flush_open = true;
        lazy.get_mut().wake_the_slot();
        assert!(
            reader.woken() > woken_before,
            "the reader waits on its flush and was woken {} times since",
            reader.woken() - woken_before
        );
        // the woken reader sees the flush through before it reads
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        assert!(
            lazy.get_mut().flushes >= 1,
            "the head was never flushed through"
        );
        assert!(matches!(
            Pin::new(&mut lazy).poll_write(&mut wcx, b"more"),
            Poll::Ready(Ok(4))
        ));
        assert_eq!(lazy.get_mut().taken, b"HEAD|payloadmore");
    }

    #[tokio::test]
    async fn a_read_that_finishes_the_head_unannounced_wakes_the_writer_parked_on_it() {
        // nobody says the stream can take bytes again: the reader polls on
        // its own and finishes the head, and only it can tell the writer
        let (writer, writer_waker) = Count::waker();
        let (_, reader_waker) = Count::waker();
        let mut wcx = Context::from_waker(&writer_waker);
        let mut rcx = Context::from_waker(&reader_waker);
        let mut lazy = LazyHead::with_grace(
            OneSlot::new(false, true),
            b"HEAD|".to_vec(),
            Duration::from_secs(30),
        );
        assert!(
            Pin::new(&mut lazy)
                .poll_write(&mut wcx, b"payload")
                .is_pending()
        );
        let mut space = [0u8; 8];
        let mut buf = ReadBuf::new(&mut space);
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        lazy.get_mut().write_open = true;
        assert_eq!(writer.woken(), 0, "nothing has woken the writer yet");
        // the reader finishes the head; there is nothing to read yet
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        assert_eq!(lazy.get_mut().taken, b"HEAD|payload");
        assert!(
            writer.woken() >= 1,
            "the writer was woken {} times, and its poll_write still owes it Ready(7)",
            writer.woken()
        );
        assert!(matches!(
            Pin::new(&mut lazy).poll_write(&mut wcx, b"payload"),
            Poll::Ready(Ok(7))
        ));
    }

    #[tokio::test]
    async fn a_head_that_fails_wakes_the_other_half_too() {
        let (writer, writer_waker) = Count::waker();
        let (_, reader_waker) = Count::waker();
        let mut wcx = Context::from_waker(&writer_waker);
        let mut rcx = Context::from_waker(&reader_waker);
        let mut lazy = LazyHead::with_grace(
            OneSlot::new(false, true),
            b"HEAD|".to_vec(),
            Duration::from_secs(30),
        );
        // both halves wait on the head
        assert!(
            Pin::new(&mut lazy)
                .poll_write(&mut wcx, b"payload")
                .is_pending()
        );
        let mut space = [0u8; 8];
        let mut buf = ReadBuf::new(&mut space);
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        // writes fail from now on, and nobody is told
        lazy.get_mut().write_error = Some(io::ErrorKind::BrokenPipe);
        assert_eq!(writer.woken(), 0, "nothing has woken the writer yet");
        assert!(matches!(
            Pin::new(&mut lazy).poll_read(&mut rcx, &mut buf),
            Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::BrokenPipe
        ));
        // the writer hears of it and finds the same error
        assert!(
            writer.woken() >= 1,
            "the writer was woken {} times, and its poll_write has not seen the failure",
            writer.woken()
        );
        assert!(matches!(
            Pin::new(&mut lazy).poll_write(&mut wcx, b"payload"),
            Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::BrokenPipe
        ));
    }

    #[tokio::test]
    async fn a_flush_that_finishes_unannounced_wakes_the_writer_parked_behind_it() {
        let (writer, writer_waker) = Count::waker();
        let (_, reader_waker) = Count::waker();
        let mut wcx = Context::from_waker(&writer_waker);
        let mut rcx = Context::from_waker(&reader_waker);
        // as in the test of the reader's pending flush: the head leaves, the
        // reader waits on the flush it owes, and a later write parks behind it
        let mut lazy = LazyHead::with_grace(
            OneSlot::new(false, false),
            b"HEAD|".to_vec(),
            Duration::from_secs(30),
        );
        assert!(
            Pin::new(&mut lazy)
                .poll_write(&mut wcx, b"payload")
                .is_pending()
        );
        let mut space = [0u8; 8];
        let mut buf = ReadBuf::new(&mut space);
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        lazy.get_mut().write_open = true;
        lazy.get_mut().wake_the_slot();
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        assert!(matches!(
            Pin::new(&mut lazy).poll_write(&mut wcx, b"payload"),
            Poll::Ready(Ok(7))
        ));
        lazy.get_mut().write_open = false;
        assert!(
            Pin::new(&mut lazy)
                .poll_write(&mut wcx, b"more")
                .is_pending()
        );
        let woken_before = writer.woken();
        // both gates open, and nobody is told
        lazy.get_mut().write_open = true;
        lazy.get_mut().flush_open = true;
        // the reader finishes its flush; there is still nothing to read
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        assert!(lazy.get_mut().flushes >= 1, "the flush was not finished");
        assert!(
            writer.woken() > woken_before,
            "the writer waits on a write behind that flush and was woken {} times since",
            writer.woken() - woken_before
        );
        assert!(matches!(
            Pin::new(&mut lazy).poll_write(&mut wcx, b"more"),
            Poll::Ready(Ok(4))
        ));
        assert_eq!(lazy.get_mut().taken, b"HEAD|payloadmore");
    }

    /// A reader that has pushed the head and waits on the flush it owes, over
    /// a stream whose writes go through and whose flush does not. The grace
    /// is nil, but a grace is a timer like any other: the reader parks on it
    /// first, with a waker nobody counts, and finds it run out once the
    /// driver has had its turn.
    async fn reader_owing_a_flush(rcx: &mut Context<'_>) -> LazyHead<OneSlot> {
        let mut lazy =
            LazyHead::with_grace(OneSlot::new(true, false), b"HEAD|".to_vec(), Duration::ZERO);
        let mut space = [0u8; 8];
        let mut buf = ReadBuf::new(&mut space);
        let mut elsewhere = Context::from_waker(Waker::noop());
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut elsewhere, &mut buf)
                .is_pending()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        // the head goes out alone, and the flush behind it pends
        assert!(Pin::new(&mut lazy).poll_read(rcx, &mut buf).is_pending());
        assert_eq!(lazy.get_mut().taken, b"HEAD|");
        assert_eq!(lazy.get_mut().flushes, 0);
        lazy
    }

    #[tokio::test]
    async fn a_reader_waiting_on_its_flush_is_woken_when_the_writers_flush_finishes_it() {
        let (reader, reader_waker) = Count::waker();
        let (_, writer_waker) = Count::waker();
        let mut wcx = Context::from_waker(&writer_waker);
        let mut rcx = Context::from_waker(&reader_waker);
        let mut lazy = reader_owing_a_flush(&mut rcx).await;
        // a write is not a flush: it finishes nothing the reader waits for
        assert!(matches!(
            Pin::new(&mut lazy).poll_write(&mut wcx, b"more"),
            Poll::Ready(Ok(4))
        ));
        assert_eq!(reader.woken(), 0, "a write that finished woke the reader");
        // the flush gets through, and nobody is told
        lazy.get_mut().flush_open = true;
        assert!(matches!(
            Pin::new(&mut lazy).poll_flush(&mut wcx),
            Poll::Ready(Ok(()))
        ));
        assert_eq!(
            reader.woken(),
            1,
            "the reader waits on a flush the writer finished and was woken {} times",
            reader.woken()
        );
        // it goes on to read, and does not flush what is flushed
        let mut space = [0u8; 8];
        let mut buf = ReadBuf::new(&mut space);
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        assert_eq!(
            lazy.get_mut().flushes,
            1,
            "the reader flushed the stream again"
        );
    }

    #[tokio::test]
    async fn a_reader_waiting_on_its_flush_is_woken_when_the_writers_shutdown_finishes_it() {
        let (reader, reader_waker) = Count::waker();
        let (_, writer_waker) = Count::waker();
        let mut wcx = Context::from_waker(&writer_waker);
        let mut rcx = Context::from_waker(&reader_waker);
        let mut lazy = reader_owing_a_flush(&mut rcx).await;
        assert!(matches!(
            Pin::new(&mut lazy).poll_shutdown(&mut wcx),
            Poll::Ready(Ok(()))
        ));
        assert_eq!(
            reader.woken(),
            1,
            "the reader waits on a flush the writer's shutdown finished and was woken {} times",
            reader.woken()
        );
        // the write side is closed, and the reader leaves it alone
        lazy.get_mut().flush_open = true;
        let mut space = [0u8; 8];
        let mut buf = ReadBuf::new(&mut space);
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
        assert_eq!(
            lazy.get_mut().flushes,
            0,
            "the reader flushed a stream whose write side is closed"
        );
    }

    #[tokio::test]
    async fn a_reader_waiting_on_its_flush_is_woken_when_the_writers_flush_fails() {
        let (reader, reader_waker) = Count::waker();
        let (_, writer_waker) = Count::waker();
        let mut wcx = Context::from_waker(&writer_waker);
        let mut rcx = Context::from_waker(&reader_waker);
        let mut lazy = reader_owing_a_flush(&mut rcx).await;
        // every flush fails from now on, and nobody is told
        lazy.get_mut().flush_error = Some(io::ErrorKind::BrokenPipe);
        assert!(matches!(
            Pin::new(&mut lazy).poll_flush(&mut wcx),
            Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::BrokenPipe
        ));
        assert_eq!(
            reader.woken(),
            1,
            "the reader waits on a flush that failed for the writer and was woken {} times",
            reader.woken()
        );
        // the error is the writer's: the reader goes on to the read side
        // and does not meet it again
        let mut space = [0u8; 8];
        let mut buf = ReadBuf::new(&mut space);
        assert!(
            Pin::new(&mut lazy)
                .poll_read(&mut rcx, &mut buf)
                .is_pending()
        );
    }

    /// What the hung tests did, with a pipe for the stream: the halves of a
    /// split stream in tasks of their own, the whole payload riding with the
    /// head, and a pipe far too small for it. Like a socket's buffer, the
    /// pipe keeps one write waker.
    #[tokio::test]
    async fn a_split_stream_whose_halves_run_in_two_tasks_gets_the_payload_through() {
        const BOUND: Duration = Duration::from_secs(10);
        let payload: Vec<u8> = (0..64 * 1024u32).map(|i| (i % 251) as u8).collect();
        let mut expected = b"HEAD|".to_vec();
        expected.extend_from_slice(&payload);
        let total = expected.len();

        let (near, far) = tokio::io::duplex(8);
        // a grace that never runs out: only the payload sends the head
        let lazy = LazyHead::with_grace(near, b"HEAD|".to_vec(), Duration::from_secs(30));
        let (mut rd, mut wr) = tokio::io::split(lazy);
        // the tasks start in this order: the writer begins the head before
        // the reader's first poll, and the peer drains after both
        let writer = tokio::spawn(async move {
            wr.write_all(&payload).await.unwrap();
            wr.flush().await.unwrap();
            wr
        });
        let reader = tokio::spawn(async move {
            let mut back = vec![0u8; total];
            rd.read_exact(&mut back).await.unwrap();
            (rd, back)
        });
        // the peer reads in small pieces and echoes them back; it drains
        // whatever the echo does, as the reader takes no answer before the
        // head is out
        let (mut far_rd, mut far_wr) = tokio::io::split(far);
        let peer = tokio::spawn(async move {
            let (queue, mut echoes) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let drain = async move {
                let mut small = [0u8; 3];
                let mut seen = 0;
                while seen < total {
                    let n = far_rd.read(&mut small).await.unwrap();
                    assert!(n > 0, "the stream ended after {seen} of {total} bytes");
                    seen += n;
                    queue.send(small[..n].to_vec()).unwrap();
                }
            };
            let echo = async move {
                while let Some(piece) = echoes.recv().await {
                    far_wr.write_all(&piece).await.unwrap();
                }
                far_wr
            };
            tokio::join!(drain, echo).1
        });

        tokio::time::timeout(BOUND, writer)
            .await
            .expect("the writer did not finish within the bound")
            .unwrap();
        let (_, back) = tokio::time::timeout(BOUND, reader)
            .await
            .expect("the reader did not finish within the bound")
            .unwrap();
        assert!(back == expected, "the echo differs");
        tokio::time::timeout(BOUND, peer)
            .await
            .expect("the peer did not finish within the bound")
            .unwrap();
    }
}
