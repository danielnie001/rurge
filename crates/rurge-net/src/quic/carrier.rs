//! A QUIC connection's packets through a carrier (phase 2 M7 design 3.3):
//! the UDP flow `underlying-proxy` gives to the server, which knows where
//! its datagrams go. quinn knows the server by a stand-in address: what it
//! sends goes to the carrier's peer whatever the destination, and all the
//! carrier receives comes from the stand-in.

use super::socket::NeverBlocks;
use crate::connector::BoxedDatagram;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker, ready};
use tokio::io::ReadBuf;

/// What quinn knows a carried connection's server by: an address of the
/// documentation range (RFC 5737), never dialled.
pub(crate) const STAND_IN: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 443);

pub(crate) struct Carried(pub(crate) BoxedDatagram);

impl fmt::Debug for Carried {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Carried").finish_non_exhaustive()
    }
}

impl AsyncUdpSocket for Carried {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(NeverBlocks)
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        // a carrier queues what it sends and drops it when full, as a full
        // socket would: one that cannot take it now drops it too
        let mut cx = Context::from_waker(Waker::noop());
        if let Poll::Ready(Err(e)) = self.0.poll_send(&mut cx, transmit.contents) {
            tracing::trace!(error = %e, "a QUIC datagram through the chain was not sent");
        }
        Ok(())
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut buf = ReadBuf::new(&mut bufs[0]);
        // a carrier that has closed fails the endpoint, and with it the
        // connection, at once
        ready!(self.0.poll_recv(cx, &mut buf))?;
        let len = buf.filled().len();
        meta[0] = RecvMeta {
            addr: STAND_IN,
            len,
            stride: len,
            ecn: None,
            dst_ip: None,
        };
        Poll::Ready(Ok(1))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0))
    }

    /// Datagrams through a proxy may be fragmented on the way: no path MTU
    /// discovery, quinn stays at the 1200 bytes every path carries.
    fn may_fragment(&self) -> bool {
        true
    }
}
