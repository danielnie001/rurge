//! One AnyTLS session: a task that owns the TLS connection, and the stream
//! handle that talks to it over two bounded queues.
//!
//! As in the reference client, a session carries one stream at a time; when
//! the stream is over the session goes back to the pool. The task keeps
//! reading while the session idles, so a heartbeat is answered and a closed
//! connection is noticed before anyone tries to reuse it.
//!
//! The task flushes after every batch of writes, so a stream's write is on
//! its way once it is queued: the session's task is what flushes what it
//! writes, and the stream's own `poll_flush` has nothing to do. AnyTLS has no
//! half-close: `shutdown` sends `cmdFIN`, which ends the stream in both
//! directions (sing-box does the same).
//!
//! A pooled session can be dead with no sign of it: the server is gone and
//! no reset came. As the reference client does, a session that is reused
//! expects a version 2 server's `cmdSYNACK` for the new stream within
//! `SYNACK_TIMEOUT` and is closed without it. The server answers once it
//! has reached the stream's target, so a target that takes longer than that
//! costs the session too: from here the two look the same.

use super::frame::{self, HEADER, MAX_DATA};
use super::padding::{Scheme, shape};
use super::pool::Pool;
use crate::outbound::untrusted_text;
use crate::task::AbortOnDrop;
use rurge_net::connector::BoxedStream;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker, ready};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_util::sync::PollSender;

/// Writes waiting for the task, and frames waiting for the stream's reader.
const QUEUE: usize = 8;
const CLOSED: &str = "anytls: the session is closed";
/// How long a reused session may leave a new stream without its
/// `cmdSYNACK` (the reference client's three seconds).
pub(crate) const SYNACK_TIMEOUT: Duration = Duration::from_secs(3);

/// The scheme of an outbound: shared by its sessions, replaced when the
/// server pushes a new one.
pub(crate) type SchemeCell = Arc<Mutex<Arc<Scheme>>>;

enum End {
    /// The server's `cmdFIN`.
    Fin,
    /// A refused stream (`cmdSYNACK` with a text) or a dead session.
    Error(String),
}

#[derive(Default)]
struct StreamState {
    end: Mutex<Option<End>>,
}

impl StreamState {
    fn set(&self, end: End) {
        self.end.lock().expect("stream state").get_or_insert(end);
    }

    fn finished_by_peer(&self) -> bool {
        matches!(*self.end.lock().expect("stream state"), Some(End::Fin))
    }

    fn is_over(&self) -> bool {
        self.end.lock().expect("stream state").is_some()
    }

    fn error(&self) -> Option<String> {
        match &*self.end.lock().expect("stream state") {
            Some(End::Error(text)) => Some(text.clone()),
            _ => None,
        }
    }
}

/// The stream the session is carrying right now.
struct Slot {
    sid: u32,
    incoming: mpsc::Sender<Vec<u8>>,
    state: Arc<StreamState>,
}

#[derive(Default)]
struct Shared {
    closed: AtomicBool,
    slot: Mutex<Option<Slot>>,
    /// The server's protocol version as its settings name it; 0 as long as
    /// it has named none, which a version 1 server never does.
    peer_version: AtomicU32,
    /// The stream the server owes its `cmdSYNACK`, and until when.
    owed: watch::Sender<Option<(u32, Instant)>>,
}

impl Shared {
    /// Data for a stream that is no longer there is dropped, as the
    /// reference does; a slow reader holds the whole session back, which is
    /// the back-pressure (there is only this one stream).
    async fn deliver(&self, sid: u32, data: Vec<u8>) {
        let incoming = match &*self.slot.lock().expect("slot") {
            Some(slot) if slot.sid == sid => slot.incoming.clone(),
            _ => return,
        };
        let _ = incoming.send(data).await;
    }

    fn finish(&self, sid: u32, end: End) {
        let mut slot = self.slot.lock().expect("slot");
        if slot.as_ref().is_some_and(|s| s.sid == sid)
            && let Some(slot) = slot.take()
        {
            slot.state.set(end);
        }
    }

    /// The stream let go of the session: late frames for it are dropped.
    fn release(&self, sid: u32) {
        let mut slot = self.slot.lock().expect("slot");
        if slot.as_ref().is_some_and(|s| s.sid == sid) {
            *slot = None;
        }
    }

    /// The server answered stream `sid`'s `cmdSYN`: nothing is owed any
    /// more when that is the stream the session waits for. The late answer
    /// to an earlier stream says nothing about this one (the reference
    /// takes any).
    fn answered(&self, sid: u32) {
        self.owed.send_if_modified(|owed| {
            let awaited = owed.is_some_and(|(stream, _)| stream == sid);
            if awaited {
                *owed = None;
            }
            awaited
        });
    }

    fn close(&self, why: String) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(slot) = self.slot.lock().expect("slot").take() {
            slot.state.set(End::Error(why));
        }
    }
}

/// The `v` of a settings text (`key=value` lines), when it is a number.
fn version_in(settings: &[u8]) -> Option<u32> {
    String::from_utf8_lossy(settings)
        .lines()
        .find_map(|line| line.strip_prefix("v="))?
        .parse()
        .ok()
}

/// Returns when the stream the server owes a `cmdSYNACK` has waited for it
/// past its time.
async fn unanswered(shared: &Shared) {
    let mut owed = shared.owed.subscribe();
    loop {
        let deadline = owed.borrow_and_update().map(|(_, deadline)| deadline);
        // `changed` never fails: `shared` holds the sender
        match deadline {
            // what is owed is looked at before the clock: an answer that
            // came in with the deadline is an answer
            Some(deadline) => {
                if tokio::time::timeout_at(deadline, owed.changed())
                    .await
                    .is_err()
                {
                    return;
                }
            }
            None => {
                let _ = owed.changed().await;
            }
        }
    }
}

/// A number in `0..n` (`n` ≥ 1); 0 when the system has no randomness to give.
pub(crate) fn pick(n: u32) -> u32 {
    let mut bytes = [0u8; 4];
    match getrandom::fill(&mut bytes) {
        Ok(()) => u32::from_be_bytes(bytes) % n,
        Err(_) => 0,
    }
}

async fn read_loop(
    mut io: ReadHalf<BoxedStream>,
    shared: &Shared,
    replies: &mpsc::Sender<Vec<u8>>,
    scheme: &SchemeCell,
) -> String {
    loop {
        let mut header = [0u8; HEADER];
        if io.read_exact(&mut header).await.is_err() {
            return CLOSED.to_string();
        }
        let (command, sid, len) = frame::parse_header(&header);
        // at most 65535 bytes: the length field cannot say more
        let mut data = vec![0u8; len];
        if io.read_exact(&mut data).await.is_err() {
            return CLOSED.to_string();
        }
        match command {
            frame::PSH => shared.deliver(sid, data).await,
            frame::FIN => shared.finish(sid, End::Fin),
            frame::SYNACK => {
                shared.answered(sid);
                // with a text, the answer is that the stream is refused
                if !data.is_empty() {
                    let text = untrusted_text(&String::from_utf8_lossy(&data), 256);
                    shared.finish(sid, End::Error(format!("anytls: {text}")));
                }
            }
            frame::ALERT => {
                let text = untrusted_text(&String::from_utf8_lossy(&data), 256);
                return format!("anytls: the server sent an alert: {text}");
            }
            frame::UPDATE_PADDING_SCHEME => match Scheme::parse(&data) {
                Some(new) => *scheme.lock().expect("scheme") = Arc::new(new),
                None => tracing::warn!(
                    "anytls: the server pushed a padding scheme that is not valid; keeping the current one"
                ),
            },
            frame::HEART_REQUEST => {
                // never wait here: a full queue means the writer is busy, which is life enough
                let _ = replies.try_send(frame::frame(frame::HEART_RESPONSE, sid, &[]));
            }
            frame::SERVER_SETTINGS => {
                if let Some(version) = version_in(&data) {
                    shared.peer_version.store(version, Ordering::SeqCst);
                }
            }
            // waste, a heart response, anything newer: read and dropped
            _ => {}
        }
    }
}

async fn write_loop(
    mut io: WriteHalf<BoxedStream>,
    queue: &mut mpsc::Receiver<Vec<u8>>,
    scheme: &SchemeCell,
) -> String {
    // packet 0 was the authentication
    let mut packet: u32 = 0;
    while let Some(first) = queue.recv().await {
        let mut next = Some(first);
        while let Some(bytes) = next {
            packet = packet.saturating_add(1);
            let current = scheme.lock().expect("scheme").clone();
            let records = if packet < current.stop() {
                shape(&current.pieces(packet, &mut pick), &bytes)
            } else {
                vec![bytes]
            };
            for record in records {
                // one write, one TLS record: the sizes are what the scheme is about
                if io.write_all(&record).await.is_err() {
                    return CLOSED.to_string();
                }
            }
            next = queue.try_recv().ok();
        }
        if io.flush().await.is_err() {
            return CLOSED.to_string();
        }
    }
    CLOSED.to_string()
}

/// No `Debug`: the task holds the authenticated connection.
pub(crate) struct Session {
    seq: u64,
    commands: mpsc::Sender<Vec<u8>>,
    shared: Arc<Shared>,
    scheme: SchemeCell,
    next_sid: u32,
    /// `cmdSettings` has not gone out yet.
    fresh: bool,
    /// What the server gets to answer a stream opened on the session once
    /// it is reused.
    synack_timeout: Duration,
    _task: AbortOnDrop,
}

impl Session {
    /// `io` is past the authentication. Spawns the session's task.
    pub(crate) fn start(seq: u64, io: BoxedStream, scheme: SchemeCell) -> Session {
        let (commands, mut queue) = mpsc::channel::<Vec<u8>>(QUEUE);
        let shared = Arc::new(Shared::default());
        let task = {
            let (shared, scheme, replies) = (shared.clone(), scheme.clone(), commands.clone());
            tokio::spawn(async move {
                let (reader, writer) = tokio::io::split(io);
                let why = tokio::select! {
                    // an alert that arrives together with a write error is the
                    // better story, and an answer that is in is read before
                    // its time is found to be up
                    biased;
                    why = read_loop(reader, &shared, &replies, &scheme) => why,
                    why = write_loop(writer, &mut queue, &scheme) => why,
                    () = unanswered(&shared) => {
                        tracing::debug!(
                            "anytls: no answer to a new stream on a reused session; the session is closed"
                        );
                        CLOSED.to_string()
                    }
                };
                if why != CLOSED {
                    // an alert: the text is already stripped and bounded
                    tracing::warn!("{why}");
                }
                shared.close(why);
            })
        };
        Session {
            seq,
            commands,
            shared,
            scheme,
            next_sid: 0,
            fresh: true,
            synack_timeout: SYNACK_TIMEOUT,
            _task: AbortOnDrop(task),
        }
    }

    /// The session with another time than `SYNACK_TIMEOUT` for the server
    /// to answer a new stream in: tests shorten it.
    pub(crate) fn with_synack_timeout(self, synack_timeout: Duration) -> Session {
        Session {
            synack_timeout,
            ..self
        }
    }

    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// Opens the next stream: `cmdSYN` and the target address leave in one
    /// write (with `cmdSettings` in front on a new session). The server's
    /// `cmdSYNACK` is not awaited; a refusal shows up on the first read, and
    /// on a reused session so does an answer that does not come: the
    /// session is closed.
    pub(crate) async fn open(
        mut self,
        address: &[u8],
        pool: Weak<Pool>,
    ) -> io::Result<AnyTlsStream> {
        let closed = || io::Error::new(io::ErrorKind::BrokenPipe, CLOSED);
        let sid = self.next_sid.checked_add(1).ok_or_else(closed)?;
        self.next_sid = sid;
        let (incoming, receiver) = mpsc::channel(QUEUE);
        let state = Arc::new(StreamState::default());
        *self.shared.slot.lock().expect("slot") = Some(Slot {
            sid,
            incoming,
            state: state.clone(),
        });
        let mut packet = Vec::new();
        if self.fresh {
            let md5 = self.scheme.lock().expect("scheme").md5().to_string();
            let settings = format!(
                "v=2\nclient=rurge/{}\npadding-md5={md5}",
                env!("CARGO_PKG_VERSION")
            );
            frame::push(&mut packet, frame::SETTINGS, 0, settings.as_bytes());
            self.fresh = false;
        }
        frame::push(&mut packet, frame::SYN, sid, &[]);
        frame::push(&mut packet, frame::PSH, sid, address);
        // A version 2 server answers every `cmdSYN`. The answer to the
        // session's first is not waited for, as in the reference: no time
        // in the pool lies behind that stream. Noted before the `cmdSYN` is
        // queued, so that its answer finds what it answers.
        if sid >= 2 && self.shared.peer_version.load(Ordering::SeqCst) >= 2 {
            self.shared
                .owed
                .send_replace(Some((sid, Instant::now() + self.synack_timeout)));
        }
        self.commands.send(packet).await.map_err(|_| closed())?;
        // The task may have ended between the pool's `is_closed()` look and
        // the slot assignment above: its `close()` has then already taken the
        // old slot, this send still found room, and nobody would ever end the
        // slot installed here. Checked after the send: had `close()` run later
        // than this look, it would find our slot and end it itself.
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(closed());
        }
        Ok(AnyTlsStream {
            sid,
            commands: PollSender::new(self.commands.clone()),
            session: Some(self),
            pool,
            incoming: receiver,
            state,
            chunk: Vec::new(),
            pos: 0,
            closed: false,
            reader: None,
        })
    }
}

pub(crate) struct AnyTlsStream {
    sid: u32,
    /// Back to the pool when the stream is dropped.
    session: Option<Session>,
    pool: Weak<Pool>,
    commands: PollSender<Vec<u8>>,
    incoming: mpsc::Receiver<Vec<u8>>,
    state: Arc<StreamState>,
    chunk: Vec<u8>,
    pos: usize,
    /// We ended it: reads are over and writes fail.
    closed: bool,
    /// A reader parked on the queue, to be told when we close.
    reader: Option<Waker>,
}

impl AnyTlsStream {
    fn close_locally(&mut self) {
        self.closed = true;
        if let Some(session) = &self.session {
            session.shared.release(self.sid);
        }
        self.incoming.close();
        if let Some(reader) = self.reader.take() {
            reader.wake();
        }
    }
}

impl AsyncRead for AnyTlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.closed {
                return Poll::Ready(Ok(()));
            }
            if this.pos < this.chunk.len() {
                let n = out.remaining().min(this.chunk.len() - this.pos);
                out.put_slice(&this.chunk[this.pos..this.pos + n]);
                this.pos += n;
                return Poll::Ready(Ok(()));
            }
            match this.incoming.poll_recv(cx) {
                Poll::Ready(Some(data)) => {
                    this.chunk = data;
                    this.pos = 0;
                }
                Poll::Ready(None) => {
                    return Poll::Ready(match this.state.error() {
                        Some(text) => Err(io::Error::other(text)),
                        None => Ok(()),
                    });
                }
                Poll::Pending => {
                    this.reader = Some(cx.waker().clone());
                    return Poll::Pending;
                }
            }
        }
    }
}

impl AsyncWrite for AnyTlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.closed || this.state.is_over() {
            let text = this
                .state
                .error()
                .unwrap_or_else(|| "anytls: the stream is closed".to_string());
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, text)));
        }
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if ready!(this.commands.poll_reserve(cx)).is_err() {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, CLOSED)));
        }
        let n = data.len().min(MAX_DATA);
        if this
            .commands
            .send_item(frame::frame(frame::PSH, this.sid, &data[..n]))
            .is_err()
        {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, CLOSED)));
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // the session's task flushes what it writes
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.closed {
            // the server's own FIN needs no answer; a dead session takes none
            if !this.state.finished_by_peer() && ready!(this.commands.poll_reserve(cx)).is_ok() {
                let _ = this
                    .commands
                    .send_item(frame::frame(frame::FIN, this.sid, &[]));
            }
            this.close_locally();
        }
        Poll::Ready(Ok(()))
    }
}

impl Drop for AnyTlsStream {
    fn drop(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        let mut reusable = !session.is_closed();
        if !self.closed {
            // dropped without a shutdown: the FIN is still owed, and a
            // session that cannot take it right now is not worth keeping
            if !self.state.finished_by_peer() {
                reusable &= session
                    .commands
                    .try_send(frame::frame(frame::FIN, self.sid, &[]))
                    .is_ok();
            }
            session.shared.release(self.sid);
        }
        if reusable && let Some(pool) = self.pool.upgrade() {
            pool.put(session);
        }
        // otherwise the session is dropped here: its task is aborted and the connection closes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anytls::padding::Scheme;

    /// The state the race leaves behind: the task has run `close()`, its
    /// queue still has room. `open` must not return a stream nobody will end.
    #[tokio::test]
    async fn a_session_that_closed_a_moment_ago_does_not_open_a_stream() {
        let (near, _far) = tokio::io::duplex(4096);
        let scheme: SchemeCell = Arc::new(Mutex::new(Arc::new(Scheme::default_scheme())));
        let session = Session::start(1, Box::new(near), scheme);
        session.shared.closed.store(true, Ordering::SeqCst);
        let err = session
            .open(&[1, 127, 0, 0, 1, 0, 80], Weak::new())
            .await
            .err()
            .expect("a dead session must not hand out a stream");
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(err.to_string(), CLOSED);
    }

    /// A target, as it follows `cmdSYN`.
    const ADDRESS: &[u8] = &[1, 127, 0, 0, 1, 0, 80];

    /// A session over a pipe whose far end has named its version, 2,
    /// unasked, and that far end. The tests that use it run on tokio's
    /// paused clock, which moves when everything waits: what they see
    /// happens at the second they say.
    async fn with_a_version_two_server() -> (Session, tokio::io::DuplexStream) {
        let (near, mut far) = tokio::io::duplex(1 << 16);
        let scheme: SchemeCell = Arc::new(Mutex::new(Arc::new(Scheme::default_scheme())));
        let session = Session::start(1, Box::new(near), scheme);
        far.write_all(&frame::frame(frame::SERVER_SETTINGS, 0, b"v=2"))
            .await
            .unwrap();
        // a turn or two of the session's task
        for _ in 0..100 {
            if session.shared.peer_version.load(Ordering::SeqCst) == 2 {
                return (session, far);
            }
            tokio::task::yield_now().await;
        }
        panic!("the server's settings were never read");
    }

    #[test]
    fn the_servers_version_is_the_v_of_its_settings() {
        assert_eq!(version_in(b"v=2"), Some(2));
        assert_eq!(version_in(b"padding-md5=0af1\nv=3\nx=y"), Some(3));
        assert_eq!(version_in(b"v=2\r\npadding-md5=0af1\r\n"), Some(2));
        // no such line, or a value that is no number, or not even text: the
        // version it had
        for settings in [
            &b""[..],
            b"x=2",
            b"dev=2",
            b"v",
            b"v=",
            b"v=two",
            b"v=-2",
            b"v=2\xff",
        ] {
            assert_eq!(
                version_in(settings),
                None,
                "{}",
                String::from_utf8_lossy(settings)
            );
        }
        // as long as a frame can make it: read to its end, and no number
        let long = [&b"v="[..], &[b'9'; MAX_DATA - 2]].concat();
        assert_eq!(version_in(&long), None);
    }

    /// Not the session's first stream, even when the server is known by
    /// then to answer: no pooled session, dead for who knows how long, is
    /// behind it.
    #[tokio::test(start_paused = true)]
    async fn the_first_stream_is_not_watched_whatever_the_servers_version() {
        let (session, mut far) = with_a_version_two_server().await;
        let shared = session.shared.clone();
        let mut stream = session.open(ADDRESS, Weak::new()).await.unwrap();
        tokio::time::sleep(SYNACK_TIMEOUT * 2).await;
        assert!(!shared.closed.load(Ordering::SeqCst));
        far.write_all(&frame::frame(frame::PSH, 1, b"late"))
            .await
            .unwrap();
        let mut got = [0u8; 4];
        tokio::time::timeout(Duration::from_secs(60), stream.read_exact(&mut got))
            .await
            .expect("bounded")
            .unwrap();
        assert_eq!(&got, b"late");
    }

    /// The session waits for the answer to the stream it carries, from the
    /// moment that stream was opened: the answer to an earlier one, let go
    /// of unanswered, is not it.
    #[tokio::test(start_paused = true)]
    async fn only_the_answer_to_the_stream_in_hand_counts_and_from_when_it_was_opened() {
        let (session, mut far) = with_a_version_two_server().await;
        let shared = session.shared.clone();
        let pool = Arc::new(Pool::default());
        let most_of_it = SYNACK_TIMEOUT * 2 / 3;
        // the first stream is not watched
        drop(session.open(ADDRESS, Arc::downgrade(&pool)).await.unwrap());
        let session = pool.take().expect("pooled");
        // the second is, and is let go of unanswered
        drop(session.open(ADDRESS, Arc::downgrade(&pool)).await.unwrap());
        let session = pool.take().expect("pooled");
        tokio::time::sleep(most_of_it).await;
        // the third starts the count again, and then the second's answer comes
        let mut third = session.open(ADDRESS, Arc::downgrade(&pool)).await.unwrap();
        far.write_all(&frame::frame(frame::SYNACK, 2, &[]))
            .await
            .unwrap();
        tokio::time::sleep(most_of_it).await;
        assert!(
            !shared.closed.load(Ordering::SeqCst),
            "closed when the second stream's time was up"
        );
        let mut buf = [0u8; 1];
        let err = tokio::time::timeout(most_of_it, third.read(&mut buf))
            .await
            .expect("the second stream's answer was taken for the third's")
            .unwrap_err();
        assert_eq!(err.to_string(), CLOSED);
    }
}
