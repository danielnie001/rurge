//! A connection of the TUN as the session pipeline sees it (phase 3 M3
//! design 5.5): a byte stream on a socket of a shard's stack.

#[cfg(test)]
use super::PANIC_MARKER;
use super::{ConnId, ConnShared, Outcome, Shard};
use smoltcp::socket::tcp;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Every read and write looks its connection up by its id in the shard's
/// table, holding the shard's lock for as long as it copies and kicking the
/// shard afterwards. Once the shard has let the connection go the stream
/// answers the connection's outcome (P12), so it never reaches a socket that
/// took the connection's handle after it.
pub(crate) struct TunStream {
    shard: Arc<Shard>,
    id: ConnId,
    shared: Arc<ConnShared>,
}

fn reset() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionReset, "the connection was reset")
}

fn shut_down() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "the connection is closed for writing",
    )
}

impl TunStream {
    pub(crate) fn new(shard: Arc<Shard>, id: ConnId, shared: Arc<ConnShared>) -> TunStream {
        TunStream { shard, id, shared }
    }

    /// Once the handshake is over; `ConnectionReset` if the connection
    /// ended before that.
    pub(crate) async fn established(&self) -> io::Result<()> {
        std::future::poll_fn(|cx| self.poll_established(cx)).await
    }

    fn poll_established(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut guard = self.shard.state();
        let state = &mut *guard;
        let Some(conn) = state.conns.get(&self.id) else {
            return Poll::Ready(Err(reset()));
        };
        let socket = state.sockets.get_mut::<tcp::Socket>(conn.handle);
        match socket.state() {
            tcp::State::Listen | tcp::State::SynSent | tcp::State::SynReceived => {
                // smoltcp wakes both of its wakers at every change of state;
                // the shard wakes this one if it lets the connection go
                socket.register_recv_waker(cx.waker());
                self.shared.register_read(cx.waker());
                Poll::Pending
            }
            tcp::State::Closed => Poll::Ready(Err(reset())),
            _ => Poll::Ready(Ok(())),
        }
    }

    /// How the connection ended, once the shard has let it go.
    fn outcome(&self) -> Outcome {
        self.shared.outcome().unwrap_or(Outcome::Reset)
    }
}

impl AsyncRead for TunStream {
    /// The end of what the app sent is `Ok(0)`; a reset is `ConnectionReset`.
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut guard = self.shard.state();
        let state = &mut *guard;
        let Some(conn) = state.conns.get(&self.id) else {
            return Poll::Ready(match self.outcome() {
                Outcome::Closed => Ok(()),
                Outcome::Reset => Err(reset()),
            });
        };
        // smoltcp keeps what was received before a reset: it is not read
        if conn.closed_by == Some(Outcome::Reset) {
            return Poll::Ready(Err(reset()));
        }
        let socket = state.sockets.get_mut::<tcp::Socket>(conn.handle);
        match socket.recv_slice(buf.initialize_unfilled()) {
            Ok(0) => {
                socket.register_recv_waker(cx.waker());
                self.shared.register_read(cx.waker());
                Poll::Pending
            }
            Ok(n) => {
                buf.advance(n);
                // room in the window to tell of, or a closed socket the shard
                // kept for this read and can let go now
                state.touched.insert(self.id);
                drop(guard);
                self.shard.kick();
                Poll::Ready(Ok(()))
            }
            Err(tcp::RecvError::Finished) => Poll::Ready(Ok(())),
            // closed without a FIN: smoltcp's own timeout, say
            Err(tcp::RecvError::InvalidState) => Poll::Ready(Err(reset())),
        }
    }
}

impl AsyncWrite for TunStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut guard = self.shard.state();
        #[cfg(test)]
        if data.starts_with(PANIC_MARKER) {
            panic!("rurge-tun test: panic with the shard's lock held");
        }
        let state = &mut *guard;
        let Some(conn) = state.conns.get(&self.id) else {
            return Poll::Ready(Err(match self.outcome() {
                Outcome::Closed => shut_down(),
                Outcome::Reset => reset(),
            }));
        };
        if conn.closed_by == Some(Outcome::Reset) {
            return Poll::Ready(Err(reset()));
        }
        let socket = state.sockets.get_mut::<tcp::Socket>(conn.handle);
        match socket.send_slice(data) {
            Ok(0) => {
                socket.register_send_waker(cx.waker());
                self.shared.register_write(cx.waker());
                Poll::Pending
            }
            Ok(n) => {
                drop(guard);
                self.shard.kick();
                Poll::Ready(Ok(n))
            }
            Err(tcp::SendError::InvalidState) => Poll::Ready(Err(
                if conn.shut
                    || matches!(
                        socket.state(),
                        tcp::State::FinWait1
                            | tcp::State::FinWait2
                            | tcp::State::Closing
                            | tcp::State::LastAck
                            | tcp::State::TimeWait
                    )
                {
                    shut_down()
                } else {
                    reset()
                },
            )),
        }
    }

    /// At once (P13): a write is the stack's as soon as it is taken, and the
    /// relay flushes after every write.
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    /// Closes the write side, a FIN once what was written is sent; at once.
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut guard = self.shard.state();
        let state = &mut *guard;
        let Some(conn) = state.conns.get_mut(&self.id) else {
            return Poll::Ready(match self.outcome() {
                Outcome::Closed => Ok(()),
                Outcome::Reset => Err(reset()),
            });
        };
        if conn.closed_by == Some(Outcome::Reset) {
            return Poll::Ready(Err(reset()));
        }
        let socket = state.sockets.get_mut::<tcp::Socket>(conn.handle);
        // closed by smoltcp's own timeout, say, and not let go yet: the reset
        // the read answers, and the outcome the shard records when it does
        if socket.state() == tcp::State::Closed && conn.closed_by != Some(Outcome::Closed) {
            return Poll::Ready(Err(reset()));
        }
        conn.shut = true;
        socket.close();
        state.touched.insert(self.id);
        drop(guard);
        self.shard.kick();
        Poll::Ready(Ok(()))
    }
}

impl Drop for TunStream {
    /// Resets the connection unless it is ending well: the write side shut
    /// down (its FIN is on its way after what was written), nothing more to
    /// come from the app and nothing of it left unread. The moment of the drop
    /// is where the connection's `linger` begins (`ShardState::stream_dropped`).
    fn drop(&mut self) {
        // a lock that is poisoned is rebuilt by whoever finds it, a drop among
        // them, which does not panic for it
        let mut guard = self.shard.state();
        guard.stream_dropped(self.id, Instant::now());
        drop(guard);
        self.shard.kick();
    }
}
