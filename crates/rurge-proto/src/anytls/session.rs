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

use super::frame::{self, HEADER, MAX_DATA};
use super::padding::{Scheme, shape};
use super::pool::Pool;
use crate::outbound::untrusted_text;
use crate::task::AbortOnDrop;
use rurge_net::connector::BoxedStream;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker, ready};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, ReadHalf, WriteHalf};
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;

/// Writes waiting for the task, and frames waiting for the stream's reader.
const QUEUE: usize = 8;
const CLOSED: &str = "anytls: the session is closed";

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

    fn close(&self, why: String) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(slot) = self.slot.lock().expect("slot").take() {
            slot.state.set(End::Error(why));
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
            frame::SYNACK if !data.is_empty() => {
                let text = untrusted_text(&String::from_utf8_lossy(&data), 256);
                shared.finish(sid, End::Error(format!("anytls: {text}")));
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
            // waste, the server's settings, a heart response, anything newer: read and dropped
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
                    // an alert that arrives together with a write error is the better story
                    biased;
                    why = read_loop(reader, &shared, &replies, &scheme) => why,
                    why = write_loop(writer, &mut queue, &scheme) => why,
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
            _task: AbortOnDrop(task),
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
    /// `cmdSYNACK` is not awaited; a refusal shows up on the first read.
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
}
