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
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

/// Every read and write looks its connection up by its id in the shard's
/// table, holding the shard's lock for as long as it copies and kicking the
/// shard afterwards. Once the shard has let the connection go the stream
/// reads what the shard kept of the app's bytes and then answers the
/// connection's outcome (P12; M3b P26), so it never reaches a socket that took
/// the connection's handle after it.
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

    /// Completes once the shard has let the connection go, and answers how it
    /// ended: the app reset it, a timer of the stack closed it, the TUN is
    /// stopping. What the shard kept of what the app sent is still there to
    /// read, and this does not wait for it to be read (M3b P26): the connection
    /// is over for a session that only holds it. The task waits as a reader
    /// does, so it reads nothing from the stream meanwhile.
    pub(crate) async fn closed(&self) -> Outcome {
        std::future::poll_fn(|cx| self.shared.poll_outcome(cx)).await
    }

    /// Ends the connection by closing it, gently: the write side is shut down
    /// (a FIN, behind what was written), and what the app still sends is read
    /// and thrown away until the app closes its side too, the connection
    /// fails, or `limit` has passed. This does not let the stream go: the caller
    /// does, by dropping it once this has returned, and not right after the
    /// `shutdown`. The drop of a stream resets a connection whose app has not
    /// closed its side yet (`ShardState::stream_dropped`), and the reset takes
    /// with it what is still to be sent, the end of the data and the FIN. Let
    /// go after the app's FIN the connection ends well, and lingers until the
    /// app has acknowledged ours; let go after `limit` with the app still not
    /// closed, it is reset.
    pub(crate) async fn close_and_wait_for_the_app(&mut self, limit: Duration) {
        if self.shutdown().await.is_err() {
            return;
        }
        // What the app sends meanwhile is thrown away as it is read, a few
        // bytes at a time. The array is part of the future of this call, whose
        // callers are boxed (in `session.rs`): only the connections that come
        // this way hold it.
        let mut scrap = [0u8; 512];
        let drained = async {
            loop {
                match self.read(&mut scrap).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        };
        let _ = tokio::time::timeout(limit, drained).await;
    }

    /// How the connection ended, once the shard has let it go.
    fn outcome(&self) -> Outcome {
        self.shared.outcome().unwrap_or(Outcome::Reset)
    }
}

impl AsyncRead for TunStream {
    /// The end of what the app sent is `Ok(0)`; a reset is `ConnectionReset`.
    /// Once the shard has let the connection go, what the app sent that this
    /// had not read is read first, and then the outcome is answered.
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
            if self.shared.read_leftover(buf) {
                return Poll::Ready(Ok(()));
            }
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
                // room in the window to tell of
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
        // what the shard kept for the stream to read goes, and its share of
        // the budget goes back
        self.shared.discard_leftover();
        // a lock that is poisoned is rebuilt by whoever finds it, a drop among
        // them, which does not panic for it
        let mut guard = self.shard.state();
        guard.stream_dropped(self.id, Instant::now());
        drop(guard);
        self.shard.kick();
    }
}
