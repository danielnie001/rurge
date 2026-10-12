//! How the API lets go of a connection.
//!
//! A TCP connection that is closed while its peer still sends on it is
//! answered by the system with a reset, and a reset can make the peer's system
//! throw away an answer it has received and not yet read; Windows does. An
//! endpoint that does not read its body, and a request refused before its body
//! is read, are answered on the head alone, and a client that writes the head
//! and the body apart sends the body to a connection that has been closed.
//!
//! So a connection is closed in the stages RFC 9112 section 9.6 describes:
//! the write side first, so that the peer reads the end of its answer; then
//! what the peer still sends is read and thrown away, for a short time and up
//! to a limit; then the connection is closed.

use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use std::io::{self, IoSlice};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle;
use tokio_util::task::TaskTracker;
use tokio_util::task::task_tracker::TaskTrackerToken;

/// The longest a connection is read from after it was let go of, from the
/// moment the task that does it has the connection. The bodies of the API are
/// small JSON texts, which come within milliseconds on a loopback or a local
/// network; a second is twice the 500 ms that Go's `net/http` waits and a fifth
/// of nginx's `lingering_timeout`, and it is also the most that stopping the
/// API waits for a peer that does not close.
pub(crate) const LINGER_TIME: Duration = Duration::from_secs(1);

/// The most that is read away from a connection after it was let go of: what
/// Go's `net/http` reads of a body a handler did not.
pub(crate) const LINGER_BYTES: usize = 256 * 1024;

/// How many connections are read away from at once. A peer that connects over
/// and over and never closes holds this many descriptors for `LINGER_TIME`
/// each, and no more.
pub(crate) const LINGER_MAX: usize = 64;

/// What the connections of one listener share.
struct Ending {
    /// Every connection from the moment it is accepted until it is gone, the
    /// time it spends lingering included.
    tracker: TaskTracker,
    /// How many connections are lingering now.
    lingering: AtomicUsize,
    /// The most that may linger at once.
    max: usize,
    /// The longest one of them lingers.
    time: Duration,
    /// The most that is read away from one of them.
    bytes: usize,
}

impl Ending {
    /// A place among the connections that linger, if there is one. It is given
    /// back when it is dropped.
    fn room(self: &Arc<Self>) -> Option<Room> {
        self.lingering
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.max).then_some(n + 1)
            })
            .ok()
            .map(|_| Room(self.clone()))
    }
}

/// A place taken among the connections that linger. The task that lingers holds
/// it, so that it is given back however the task ends, a runtime that drops the
/// task included.
struct Room(Arc<Ending>);

impl Drop for Room {
    fn drop(&mut self) {
        self.0.lingering.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The listener of the API: a `TcpListener` whose connections linger when they
/// are let go of.
pub(crate) struct Lingering {
    listener: TcpListener,
    ending: Arc<Ending>,
}

impl Lingering {
    pub(crate) fn new(listener: TcpListener) -> Lingering {
        Lingering::with_limits(listener, LINGER_MAX, LINGER_TIME, LINGER_BYTES)
    }

    fn with_limits(listener: TcpListener, max: usize, time: Duration, bytes: usize) -> Lingering {
        Lingering {
            listener,
            ending: Arc::new(Ending {
                tracker: TaskTracker::new(),
                lingering: AtomicUsize::new(0),
                max,
                time,
                bytes,
            }),
        }
    }

    /// The connections this listener has accepted that are not gone yet. Once
    /// the server has stopped, closing it and waiting for it returns when every
    /// one of them is.
    pub(crate) fn connections(&self) -> TaskTracker {
        self.ending.tracker.clone()
    }
}

impl Listener for Lingering {
    type Io = Closing;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Closing, SocketAddr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, peer)) => {
                    // The connection is counted from here, not from the moment
                    // it is let go of. `axum` counts a connection as done when
                    // its task has left its loop, and drops the connection,
                    // and so `Closing`, only after that: a tracker that heard
                    // of the connection in `drop` could be found empty by a
                    // server that has stopped while the connection that
                    // carried `POST /v1/stop` is yet to be let go of, and the
                    // process would end with that client's answer unread.
                    let token = self.ending.tracker.token();
                    let closing = Closing {
                        stream: Some(stream),
                        token: Some(token),
                        ending: self.ending.clone(),
                    };
                    return (closing, peer);
                }
                Err(e) => failed_to_accept(&e).await,
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

/// What `axum` does with a `TcpListener` that fails to accept: a connection
/// that went away before it was accepted is no news, anything else (running
/// out of descriptors, say) is reported and waited out for a second.
async fn failed_to_accept(e: &io::Error) {
    if matches!(
        e.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    ) {
        return;
    }
    tracing::error!(error = %e, "http-api: cannot accept a connection; trying again in a second");
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// A connection of the API: the `TcpStream` as far as `hyper` can tell. When
/// `hyper` is done with it, it is handed to a task that lingers on it.
pub(crate) struct Closing {
    /// `None` once it has been handed to the task that lingers.
    stream: Option<TcpStream>,
    token: Option<TaskTrackerToken>,
    ending: Arc<Ending>,
}

impl Closing {
    fn stream(&mut self) -> Pin<&mut TcpStream> {
        Pin::new(
            self.stream
                .as_mut()
                .expect("the stream is only taken when the connection is dropped"),
        )
    }
}

impl AsyncRead for Closing {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.get_mut().stream().poll_read(cx, buf)
    }
}

impl AsyncWrite for Closing {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.get_mut().stream().poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.get_mut().stream().poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream
            .as_ref()
            .is_some_and(AsyncWrite::is_write_vectored)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().stream().poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().stream().poll_shutdown(cx)
    }
}

impl Drop for Closing {
    fn drop(&mut self) {
        let (Some(stream), Some(token)) = (self.stream.take(), self.token.take()) else {
            return;
        };
        // Without a runtime there is no task to hand the connection to, and
        // without room it is not lingered on: it is closed by being dropped, as
        // it always was. Here too it is closed before it stops being counted:
        // the token goes when this function returns.
        let Ok(runtime) = Handle::try_current() else {
            drop(stream);
            return;
        };
        let Some(room) = self.ending.room() else {
            drop(stream);
            return;
        };
        let (time, bytes) = (self.ending.time, self.ending.bytes);
        runtime.spawn(finish(stream, token, room, time, bytes));
    }
}

/// The task a connection lingers in. The stream is closed before the room and
/// the token are given back, so that a server that has waited for its
/// connections has none left open.
async fn finish(
    mut stream: TcpStream,
    token: TaskTrackerToken,
    room: Room,
    time: Duration,
    bytes: usize,
) {
    linger(&mut stream, time, bytes).await;
    drop(stream);
    drop(room);
    drop(token);
}

/// Shuts the write side of `stream` down, which tells the peer that all it
/// will be sent has been sent, and then reads what the peer still sends and
/// throws it away until the peer closes, a read fails, `bytes` have been thrown
/// away, or `time` has passed since the call, whichever comes first.
async fn linger(stream: &mut TcpStream, time: Duration, bytes: usize) {
    let _ = tokio::time::timeout(time, async {
        // `hyper` has shut the write side down already when it finished with
        // the connection the usual way, and a second shutdown fails on some
        // systems; that is no reason not to read.
        let _ = stream.shutdown().await;
        let mut scratch = [0u8; 4096];
        let mut left = bytes;
        while left > 0 {
            let want = scratch.len().min(left);
            match stream.read(&mut scratch[..want]).await {
                Ok(0) | Err(_) => break,
                Ok(n) => left -= n,
            }
        }
    })
    .await;
}

/// The address of the peer of a connection. `axum` has this for a `TcpListener`
/// only, and a listener of our own needs a type of our own for it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Peer(pub SocketAddr);

impl Connected<IncomingStream<'_, Lingering>> for Peer {
    fn connect_info(stream: IncomingStream<'_, Lingering>) -> Peer {
        Peer(*stream.remote_addr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A connected pair: the end the server has, and the peer's.
    async fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        (server, client)
    }

    /// A listener that lingers `limit` connections at most, and a client for
    /// each connection that is accepted from it. A connection lingers until
    /// its peer closes: the time it is given is longer than any test.
    struct Rig {
        listener: Lingering,
        addr: SocketAddr,
    }

    impl Rig {
        async fn new(limit: usize) -> Rig {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let time = Duration::from_secs(600);
            Rig {
                listener: Lingering::with_limits(listener, limit, time, LINGER_BYTES),
                addr,
            }
        }

        /// A connection, as the server has it and as the peer has it.
        async fn connection(&mut self) -> (Closing, TcpStream) {
            let client = TcpStream::connect(self.addr).await.unwrap();
            let (closing, peer) = self.listener.accept().await;
            assert_eq!(peer, client.local_addr().unwrap());
            (closing, client)
        }

        fn lingering(&self) -> usize {
            self.listener.ending.lingering.load(Ordering::SeqCst)
        }

        fn open(&self) -> usize {
            self.listener.ending.tracker.len()
        }
    }

    /// Polls `f` every 10 ms for up to 5 s.
    async fn until(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < deadline, "{what} did not happen in time");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn read_one(stream: &mut TcpStream) -> io::Result<usize> {
        let mut byte = [0u8; 64];
        tokio::time::timeout(Duration::from_secs(5), stream.read(&mut byte))
            .await
            .expect("no end of stream or error within 5 s")
    }

    #[tokio::test]
    async fn a_peer_that_has_closed_ends_the_lingering_at_once() {
        let (mut server, client) = pair().await;
        drop(client);
        let started = Instant::now();
        tokio::time::timeout(
            Duration::from_secs(10),
            linger(&mut server, Duration::from_secs(5), LINGER_BYTES),
        )
        .await
        .expect("it did not end");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_peer_that_does_nothing_is_waited_for_the_time_and_no_longer() {
        let (mut server, client) = pair().await;
        let time = Duration::from_millis(500);
        let started = Instant::now();
        tokio::time::timeout(
            Duration::from_secs(10),
            linger(&mut server, time, LINGER_BYTES),
        )
        .await
        .expect("it did not end");
        let waited = started.elapsed();
        assert!(
            waited >= time.mul_f64(0.9) && waited <= time * 3,
            "{waited:?} for {time:?}"
        );
        drop(client);
    }

    /// One deadline for the whole of it, not one for each read: a peer that
    /// sends a little at a time does not get the time again with each piece.
    #[tokio::test]
    async fn a_peer_that_sends_a_little_at_a_time_gets_the_time_once() {
        let (mut server, mut client) = pair().await;
        let time = Duration::from_millis(500);
        let sender = tokio::spawn(async move {
            while client.write_all(b"x").await.is_ok() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        let started = Instant::now();
        tokio::time::timeout(
            Duration::from_secs(10),
            linger(&mut server, time, LINGER_BYTES),
        )
        .await
        .expect("it did not end");
        let waited = started.elapsed();
        assert!(
            waited >= time.mul_f64(0.9) && waited <= time * 3,
            "{waited:?} for {time:?}"
        );
        drop(server);
        tokio::time::timeout(Duration::from_secs(5), sender)
            .await
            .expect("the sender went on after the connection was closed")
            .unwrap();
    }

    #[tokio::test]
    async fn a_peer_that_sends_without_end_is_read_to_the_limit_and_no_further() {
        let (mut server, mut client) = pair().await;
        let sender = tokio::spawn(async move {
            let chunk = [0u8; 8192];
            while client.write_all(&chunk).await.is_ok() {}
        });
        let bytes = 64 * 1024;
        let started = Instant::now();
        tokio::time::timeout(
            Duration::from_secs(10),
            linger(&mut server, Duration::from_secs(5), bytes),
        )
        .await
        .expect("it did not end");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "it ended on the time, not on the limit: {:?}",
            started.elapsed()
        );
        drop(server);
        tokio::time::timeout(Duration::from_secs(5), sender)
            .await
            .expect("the sender went on after the connection was closed")
            .unwrap();
    }

    /// What was sent beyond the limit is not read, and a peer that is silent
    /// after it is not waited for either.
    #[tokio::test]
    async fn a_peer_that_sent_more_than_the_limit_and_went_quiet_is_not_waited_for() {
        let (mut server, mut client) = pair().await;
        let bytes = 16 * 1024;
        client.write_all(&vec![0u8; 2 * bytes]).await.unwrap();
        let started = Instant::now();
        tokio::time::timeout(
            Duration::from_secs(10),
            linger(&mut server, Duration::from_secs(5), bytes),
        )
        .await
        .expect("it did not end");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        drop(client);
    }

    /// What is read away is counted to the byte: one byte short of the limit
    /// the lingering goes on, and the byte that fills it ends it.
    #[tokio::test]
    async fn what_is_read_away_is_counted_to_the_byte() {
        let (mut server, mut client) = pair().await;
        let bytes = 64 * 1024;
        let lingering = tokio::spawn(async move {
            linger(&mut server, Duration::from_secs(600), bytes).await;
            server
        });
        client.write_all(&vec![0u8; bytes - 1]).await.unwrap();
        // Nothing happens here, and that is what is asserted: a lingering that
        // counted more than it read would have ended within milliseconds of
        // the last of these bytes.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!lingering.is_finished(), "it ended short of the limit");
        client.write_all(&[0u8]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), lingering)
            .await
            .expect("it did not end at the limit")
            .unwrap();
        drop(client);
    }

    /// The peer has the end of its answer before anything else happens, and
    /// what it sends after that is read away, not answered with a reset.
    #[tokio::test]
    async fn the_write_side_is_shut_down_at_once_and_what_comes_after_is_read_away() {
        let (mut server, mut client) = pair().await;
        let lingering = tokio::spawn(async move {
            linger(&mut server, Duration::from_secs(5), LINGER_BYTES).await;
            server
        });
        assert_eq!(read_one(&mut client).await.unwrap(), 0, "no end of stream");
        client.write_all(b"a body that came late").await.unwrap();
        // Nothing happens here, and that is what is asserted: a connection
        // that was closed would have answered the bytes with a reset by now,
        // and a loopback takes milliseconds to do that.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!lingering.is_finished());
        assert_eq!(
            read_one(&mut client).await.unwrap(),
            0,
            "a reset, not an end"
        );
        drop(client);
        tokio::time::timeout(Duration::from_secs(5), lingering)
            .await
            .expect("it did not end when the peer closed")
            .unwrap();
    }

    #[tokio::test]
    async fn a_connection_that_is_not_let_go_of_is_the_stream() {
        let mut rig = Rig::new(2).await;
        let (mut closing, mut client) = rig.connection().await;
        assert!(AsyncWrite::is_write_vectored(&closing));
        closing.write_all(b"one ").await.unwrap();
        let parts = [IoSlice::new(b"two "), IoSlice::new(b"three")];
        // an empty socket takes nine bytes in one piece
        assert_eq!(closing.write_vectored(&parts).await.unwrap(), 9);
        closing.flush().await.unwrap();
        let mut got = [0u8; 13];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"one two three");
        client.write_all(b"back").await.unwrap();
        let mut back = [0u8; 4];
        closing.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"back");
        closing.shutdown().await.unwrap();
        assert_eq!(read_one(&mut client).await.unwrap(), 0);
        assert_eq!(rig.lingering(), 0, "nothing has been let go of");
    }

    /// A connection that is let go of is read away from in a task, and is
    /// counted all along: from the accept to the end of the lingering.
    #[tokio::test]
    async fn a_connection_is_counted_from_the_accept_to_the_end_of_the_lingering() {
        let mut rig = Rig::new(2).await;
        let (closing, mut client) = rig.connection().await;
        assert_eq!(rig.open(), 1, "counted when it was accepted");
        assert_eq!(rig.lingering(), 0);
        drop(closing);
        assert_eq!(rig.open(), 1, "still counted: it is lingering");
        assert_eq!(rig.lingering(), 1);
        assert_eq!(read_one(&mut client).await.unwrap(), 0);
        client.write_all(b"late").await.unwrap();
        drop(client);
        until("the lingering to end", || rig.open() == 0).await;
        assert_eq!(rig.lingering(), 0, "the place was given back");
    }

    /// The limit is of the connections that are lingering, not of those that
    /// are open: three are open here, and the third is the one that does not
    /// find a place.
    #[tokio::test]
    async fn only_so_many_connections_linger_and_the_rest_are_closed_at_once() {
        let mut rig = Rig::new(2).await;
        let (first, first_peer) = rig.connection().await;
        let (second, second_peer) = rig.connection().await;
        let (third, mut third_peer) = rig.connection().await;
        assert_eq!(rig.open(), 3);
        drop(first);
        drop(second);
        assert_eq!(rig.lingering(), 2);
        drop(third);
        assert_eq!(rig.lingering(), 2, "no place for the third");
        assert_eq!(rig.open(), 2, "the third is gone");
        assert_eq!(read_one(&mut third_peer).await.unwrap(), 0);
        // a place that is given back is a place for the next one
        drop(first_peer);
        until("a place to be given back", || rig.lingering() == 1).await;
        let (fourth, fourth_peer) = rig.connection().await;
        drop(fourth);
        assert_eq!(rig.lingering(), 2);
        drop((second_peer, third_peer, fourth_peer));
        until("every place to be given back", || rig.lingering() == 0).await;
        until("every connection to be gone", || rig.open() == 0).await;
    }

    /// A thread that is in no runtime has no task to hand the connection to:
    /// it is closed at once, and neither the count nor the places are left
    /// holding it.
    #[tokio::test]
    async fn a_connection_dropped_outside_a_runtime_is_closed_at_once() {
        let mut rig = Rig::new(2).await;
        let (closing, mut client) = rig.connection().await;
        std::thread::spawn(move || drop(closing)).join().unwrap();
        assert_eq!(read_one(&mut client).await.unwrap(), 0);
        assert_eq!(rig.lingering(), 0);
        assert_eq!(rig.open(), 0);
    }

    /// A runtime that ends with a connection still lingering drops the task,
    /// and the place and the count go with it.
    #[test]
    fn a_runtime_that_ends_gives_back_what_a_lingering_connection_held() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (ending, client) = runtime.block_on(async {
            let mut rig = Rig::new(2).await;
            let (closing, client) = rig.connection().await;
            drop(closing);
            assert_eq!(rig.lingering(), 1);
            (rig.listener.ending.clone(), client)
        });
        assert_eq!(ending.lingering.load(Ordering::SeqCst), 1);
        assert_eq!(ending.tracker.len(), 1);
        drop(runtime);
        assert_eq!(ending.lingering.load(Ordering::SeqCst), 0);
        assert_eq!(ending.tracker.len(), 0);
        drop(client);
    }
}
