//! One HTTP/2 stream as a byte stream (phase 2 M6 design 5.2): the tunnel
//! of a CONNECT, or (in the test fakes) the server's side of one.
//!
//! - Writes take the stream's send capacity first: at most what the peer's
//!   flow-control windows allow is handed to `h2`, and a write without
//!   capacity waits for it. `h2` itself would buffer without bound.
//! - Reads hand back the peer's capacity for exactly what they consume, so
//!   at most a window's worth of the peer's data waits unread.
//! - Shutdown sends END_STREAM: the half-close the peer sees as a FIN
//!   (RFC 9113 8.5); reading goes on.
//! - A stream dropped before both directions ended is reset by `h2`
//!   (RST_STREAM CANCEL).

use super::{Lease, describe};
use bytes::Bytes;
use h2::{RecvStream, SendStream};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The most capacity one write asks for.
const MAX_RESERVE: usize = 64 * 1024;

/// No `Debug`: nothing in it is worth printing.
pub(crate) struct H2Stream {
    /// The protocol the error texts start with (`h2-connect`).
    label: &'static str,
    send: SendStream<Bytes>,
    recv: RecvStream,
    /// The rest of the latest DATA frame.
    unread: Bytes,
    read_end: bool,
    write_end: bool,
    /// The pooled connection's count of open streams, while the stream lives.
    _lease: Option<Lease>,
}

impl H2Stream {
    /// Also the server's side of a stream (the test fakes).
    pub(crate) fn new(label: &'static str, send: SendStream<Bytes>, recv: RecvStream) -> H2Stream {
        H2Stream {
            label,
            send,
            recv,
            unread: Bytes::new(),
            read_end: false,
            write_end: false,
            _lease: None,
        }
    }

    pub(super) fn leased(mut self, lease: Lease) -> H2Stream {
        self._lease = Some(lease);
        self
    }

    /// Why sending stopped: the peer's reset, when it sent one.
    fn send_closed(&mut self, cx: &mut Context<'_>) -> io::Error {
        match self.send.poll_reset(cx) {
            Poll::Ready(Ok(reason)) => io::Error::new(
                io::ErrorKind::ConnectionReset,
                format!("{}: the server reset the stream ({reason:?})", self.label),
            ),
            Poll::Ready(Err(e)) => io_error(self.label, &e),
            Poll::Pending => io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("{}: the stream is closed", self.label),
            ),
        }
    }
}

/// An `h2` failure as an I/O error that says what happened, never the
/// GOAWAY's debug data.
pub(super) fn io_error(label: &str, e: &h2::Error) -> io::Error {
    let kind = if e.is_reset() {
        io::ErrorKind::ConnectionReset
    } else if e.is_go_away() {
        io::ErrorKind::ConnectionAborted
    } else {
        e.get_io().map_or(io::ErrorKind::Other, io::Error::kind)
    };
    io::Error::new(kind, format!("{label}: {}", describe(e)))
}

impl AsyncRead for H2Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        // an empty DATA frame is not the end: only END_STREAM is
        while this.unread.is_empty() {
            if this.read_end {
                return Poll::Ready(Ok(()));
            }
            match ready!(this.recv.poll_data(cx)) {
                None => this.read_end = true,
                Some(Err(e)) => return Poll::Ready(Err(io_error(this.label, &e))),
                Some(Ok(data)) => this.unread = data,
            }
        }
        let n = this.unread.len().min(buf.remaining());
        buf.put_slice(&this.unread.split_to(n));
        // fails only on a stream that is gone, which the next read reports
        let _ = this.recv.flow_control().release_capacity(n);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for H2Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.write_end {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("{}: write after shutdown", this.label),
            )));
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            let capacity = this.send.capacity();
            if capacity > 0 {
                let n = capacity.min(buf.len());
                this.send
                    .send_data(Bytes::copy_from_slice(&buf[..n]), false)
                    .map_err(|e| io_error(this.label, &e))?;
                // what is left goes back to the connection: a stream that
                // stops writing holds none of the window its siblings share
                this.send.reserve_capacity(0);
                return Poll::Ready(Ok(n));
            }
            // the total wanted, not an increment; `poll_capacity` reports
            // only a change, so the capacity is read again above
            this.send.reserve_capacity(buf.len().min(MAX_RESERVE));
            match ready!(this.send.poll_capacity(cx)) {
                Some(Ok(_)) => {}
                Some(Err(e)) => return Poll::Ready(Err(io_error(this.label, &e))),
                None => return Poll::Ready(Err(this.send_closed(cx))),
            }
        }
    }

    /// The connection's task writes what `h2` holds.
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.write_end {
            this.write_end = true;
            this.send
                .send_data(Bytes::new(), true)
                .map_err(|e| io_error(this.label, &e))?;
        }
        Poll::Ready(Ok(()))
    }
}
