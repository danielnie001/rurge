//! One TCP connection through the tunnel (phase 2 M4 design 6.1): reads and
//! writes go straight to its smoltcp socket under the lock, park on the
//! socket's wakers when there is nothing to read or no room to write, and
//! wake the device task so the stack runs.

use crate::device::Device;
use rurge_proto::OutboundError;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct TunnelStream {
    device: Arc<Device>,
    handle: SocketHandle,
}

impl TunnelStream {
    pub(crate) fn new(device: Arc<Device>, handle: SocketHandle) -> TunnelStream {
        TunnelStream { device, handle }
    }

    /// Ready once the connection is established; an error when the far end
    /// refused it.
    pub(crate) fn poll_established(&self, cx: &mut Context<'_>) -> Poll<Result<(), OutboundError>> {
        let mut stack = self.device.shared.stack.lock().expect("the tunnel");
        let socket = stack.tcp(self.handle);
        match socket.state() {
            tcp::State::SynSent | tcp::State::SynReceived => {
                socket.register_send_waker(cx.waker());
                Poll::Pending
            }
            // a reset in answer to the SYN
            tcp::State::Closed => Poll::Ready(Err(OutboundError::Io(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "wireguard: the destination refused the connection",
            )))),
            _ => Poll::Ready(Ok(())),
        }
    }
}

impl AsyncRead for TunnelStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut stack = self.device.shared.stack.lock().expect("the tunnel");
        let socket = stack.tcp(self.handle);
        match socket.recv_slice(buf.initialize_unfilled()) {
            Ok(0) => {
                socket.register_recv_waker(cx.waker());
                Poll::Pending
            }
            Ok(n) => {
                buf.advance(n);
                drop(stack);
                // the window opened: the stack may have an update to send
                self.device.shared.kick();
                Poll::Ready(Ok(()))
            }
            // the far end finished: the end of the stream
            Err(tcp::RecvError::Finished) => Poll::Ready(Ok(())),
            Err(tcp::RecvError::InvalidState) => {
                Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()))
            }
        }
    }
}

impl AsyncWrite for TunnelStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut stack = self.device.shared.stack.lock().expect("the tunnel");
        let socket = stack.tcp(self.handle);
        if !socket.may_send() {
            let kind = if socket.state() == tcp::State::Closed {
                io::ErrorKind::ConnectionReset
            } else {
                io::ErrorKind::BrokenPipe
            };
            return Poll::Ready(Err(kind.into()));
        }
        match socket.send_slice(data) {
            Ok(0) if !data.is_empty() => {
                socket.register_send_waker(cx.waker());
                Poll::Pending
            }
            Ok(n) => {
                drop(stack);
                self.device.shared.kick();
                Poll::Ready(Ok(n))
            }
            Err(tcp::SendError::InvalidState) => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
        }
    }

    /// What was written is the stack's to send already.
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    /// A FIN, after what was written.
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.device
            .shared
            .stack
            .lock()
            .expect("the tunnel")
            .tcp(self.handle)
            .close();
        self.device.shared.kick();
        Poll::Ready(Ok(()))
    }
}

impl Drop for TunnelStream {
    fn drop(&mut self) {
        if let Ok(mut stack) = self.device.shared.stack.lock() {
            stack.release(self.handle, Instant::now());
        }
        self.device.shared.kick();
    }
}
