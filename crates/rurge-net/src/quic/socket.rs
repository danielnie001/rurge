//! The sockets under a direct QUIC connection and the `ecn=off` layer
//! (phase 2 M7 design 3.3, 3.4).

use crate::connector::DirectConnector;
use crate::socket::Family;
use quinn::udp::{RecvMeta, Transmit, UdpSocketState};
use quinn::{AsyncUdpSocket, Runtime, TokioRuntime, UdpPoller};
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use tokio::io::Interest;

/// A socket of `family` made as DIRECT's UDP carrier makes its own
/// (`DirectConnector::bind_udp`: the policy's socket options, the binding
/// to its interface kept), ready for quinn: quinn-udp's own — segmentation
/// offload, per-packet ECN — unless the policy sets `tos`.
pub(crate) fn direct_socket(
    connector: &DirectConnector,
    family: Family,
) -> io::Result<Arc<dyn AsyncUdpSocket>> {
    let socket = connector.bind_udp(family)?;
    if connector.tos() == 0 {
        TokioRuntime.wrap_udp_socket(socket)
    } else {
        Ok(Arc::new(TosSocket::new(socket)?))
    }
}

/// A socket that keeps the policy's `tos`. quinn-udp writes the TOS byte of
/// every packet it sends (on Linux and macOS; only the ECN bits), which
/// clears the DSCP set on the socket: what goes out is sent plainly, without
/// ECN marks and one datagram at a time. What comes in still goes through
/// quinn-udp, which also makes the socket's packets unfragmentable: the
/// server's ECN marks are seen and the path MTU is discovered.
#[derive(Debug)]
struct TosSocket {
    io: tokio::net::UdpSocket,
    state: UdpSocketState,
}

impl TosSocket {
    fn new(socket: std::net::UdpSocket) -> io::Result<TosSocket> {
        let state = UdpSocketState::new((&socket).into())?;
        Ok(TosSocket {
            io: tokio::net::UdpSocket::from_std(socket)?,
            state,
        })
    }
}

impl AsyncUdpSocket for TosSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Writable(self))
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        match self.io.try_send_to(transmit.contents, transmit.destination) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(e),
            Err(e) => {
                // as quinn-udp's own sockets: a datagram that cannot go is
                // lost, which QUIC recovers from, not a failed connection
                tracing::trace!(error = %e, "a QUIC datagram was not sent");
                Ok(())
            }
            Ok(_) => Ok(()),
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        // as quinn's own sockets: a failed receive (an ICMP error Windows
        // reports, say) leaves the socket readable and is tried again
        loop {
            ready!(self.io.poll_recv_ready(cx))?;
            if let Ok(n) = self.io.try_io(Interest::READABLE, || {
                self.state.recv((&self.io).into(), bufs, meta)
            }) {
                return Poll::Ready(Ok(n));
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.local_addr()
    }

    fn max_receive_segments(&self) -> usize {
        self.state.gro_segments()
    }

    fn may_fragment(&self) -> bool {
        self.state.may_fragment()
    }
}

#[derive(Debug)]
struct Writable(Arc<TosSocket>);

impl UdpPoller for Writable {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.io.poll_send_ready(cx)
    }
}

/// Never waits: for sockets that drop what they cannot send now, as a
/// full socket buffer would.
#[derive(Debug)]
pub(crate) struct NeverBlocks;

impl UdpPoller for NeverBlocks {
    fn poll_writable(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// `ecn=off` (phase 2 M7 design 3.4): no ECN mark on what goes out, none
/// seen on what comes in. quinn's ECN validation then fails at the first
/// acknowledgement on either side, and both stop marking.
#[derive(Debug)]
pub(crate) struct NoEcn(pub(crate) Arc<dyn AsyncUdpSocket>);

impl AsyncUdpSocket for NoEcn {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.0.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        self.0.try_send(&Transmit {
            ecn: None,
            ..transmit.clone()
        })
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let n = ready!(self.0.poll_recv(cx, bufs, meta))?;
        for meta in &mut meta[..n] {
            meta.ecn = None;
        }
        Poll::Ready(Ok(n))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.0.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.0.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.0.may_fragment()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quinn::udp::EcnCodepoint;
    use std::sync::Mutex;

    /// Notes what it is asked to send; hands out one datagram marked CE.
    #[derive(Debug, Default)]
    struct Scripted {
        sent: Mutex<Vec<Option<EcnCodepoint>>>,
    }

    impl AsyncUdpSocket for Scripted {
        fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
            Box::pin(NeverBlocks)
        }

        fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
            self.sent.lock().unwrap().push(transmit.ecn);
            Ok(())
        }

        fn poll_recv(
            &self,
            _cx: &mut Context<'_>,
            bufs: &mut [IoSliceMut<'_>],
            meta: &mut [RecvMeta],
        ) -> Poll<io::Result<usize>> {
            bufs[0][..2].copy_from_slice(b"hi");
            meta[0] = RecvMeta {
                addr: "192.0.2.1:443".parse().unwrap(),
                len: 2,
                stride: 2,
                ecn: Some(EcnCodepoint::Ce),
                dst_ip: None,
            };
            Poll::Ready(Ok(1))
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            Ok("0.0.0.0:0".parse().unwrap())
        }
    }

    /// `ecn=off` takes the marks off both ways and leaves everything else.
    #[test]
    fn ecn_off_strips_the_marks_both_ways() {
        let inner = Arc::new(Scripted::default());
        let socket = NoEcn(inner.clone());
        socket
            .try_send(&Transmit {
                destination: "192.0.2.1:443".parse().unwrap(),
                ecn: Some(EcnCodepoint::Ect0),
                contents: b"out",
                segment_size: None,
                src_ip: None,
            })
            .unwrap();
        assert_eq!(*inner.sent.lock().unwrap(), [None]);
        let mut storage = [0u8; 16];
        let mut bufs = [IoSliceMut::new(&mut storage)];
        let mut meta = [RecvMeta::default()];
        let mut cx = Context::from_waker(std::task::Waker::noop());
        let Poll::Ready(Ok(n)) = socket.poll_recv(&mut cx, &mut bufs, &mut meta) else {
            panic!("the scripted datagram is there");
        };
        assert_eq!(n, 1);
        assert_eq!(meta[0].ecn, None);
        assert_eq!((meta[0].len, &storage[..2]), (2, &b"hi"[..]));
    }
}
