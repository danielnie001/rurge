//! A QUIC connection's packets through a carrier (phase 2 M7 design 3.3):
//! the UDP flow `underlying-proxy` gives to the server, which knows where
//! its datagrams go. quinn knows the server by a stand-in address: what it
//! sends goes to the carrier's peer whatever the destination, and all the
//! carrier receives comes from the stand-in.
//!
//! A carrier that cannot send now keeps quinn waiting for it (`Writable`):
//! a new socket's first datagram — the connection's first packet — is not
//! lost to the reactor not having said yet that the socket can send. A
//! carrier that fails takes the endpoint with it, and says why itself
//! (`Carried::failure`).

use crate::connector::BoxedDatagram;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker, ready};
use tokio::io::ReadBuf;

/// What quinn knows a carried connection's server by: an address of the
/// documentation range (RFC 5737), never dialled.
pub(crate) const STAND_IN: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 443);

pub(crate) struct Carried {
    datagram: BoxedDatagram,
    /// What the carrier failed with, until `failure` takes it.
    failed: Mutex<Option<io::Error>>,
}

impl Carried {
    pub(crate) fn new(datagram: BoxedDatagram) -> Carried {
        Carried {
            datagram,
            failed: Mutex::new(None),
        }
    }

    /// What the carrier failed with, in its own words, when it did: quinn
    /// says of it only that the connection's endpoint is gone. Told once.
    pub(crate) fn failure(&self) -> Option<io::Error> {
        self.failed.lock().expect("the carrier's failure").take()
    }
}

impl fmt::Debug for Carried {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Carried").finish_non_exhaustive()
    }
}

impl AsyncUdpSocket for Carried {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Writable(self))
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        let mut cx = Context::from_waker(Waker::noop());
        match self.datagram.poll_send(&mut cx, transmit.contents) {
            Poll::Ready(Ok(_)) => Ok(()),
            // as a socket's own: a datagram that cannot go is lost, which
            // QUIC recovers from, not a failed connection
            Poll::Ready(Err(e)) => {
                tracing::trace!(error = %e, "a QUIC datagram through the chain was not sent");
                Ok(())
            }
            // not now: quinn keeps the datagram until the poller says the
            // carrier can send. One that cannot say when — ready to all it
            // knows — would have quinn try for ever: its datagram is
            // dropped, as a full socket would drop it
            Poll::Pending => match self.datagram.poll_send_ready(&mut cx) {
                Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
                Poll::Ready(_) => Ok(()),
            },
        }
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
        if let Err(e) = ready!(self.datagram.poll_recv(cx, &mut buf)) {
            let kept = io::Error::new(e.kind(), e.to_string());
            *self.failed.lock().expect("the carrier's failure") = Some(kept);
            return Poll::Ready(Err(e));
        }
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

/// Waits until the carrier can send (`Datagram::poll_send_ready`).
#[derive(Debug)]
struct Writable(Arc<Carried>);

impl UdpPoller for Writable {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // a failure is the send's to find, and to drop: quinn would leave
        // the connection hanging on an error from here
        self.0.datagram.poll_send_ready(cx).map(|_| Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::Datagram;
    use std::future::poll_fn;
    use std::pin::pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio::time::timeout;

    /// A carrier that takes datagrams only once it is let: until then its
    /// sends are pending. With `says_when` it tells whoever waits to send;
    /// without, it is ready to all it knows.
    #[derive(Default)]
    struct Gate {
        says_when: bool,
        open: AtomicBool,
        sent: Mutex<Vec<Vec<u8>>>,
        sender: Mutex<Option<Waker>>,
    }

    impl Gate {
        fn open(&self) {
            self.open.store(true, Ordering::SeqCst);
            if let Some(sender) = self.sender.lock().unwrap().take() {
                sender.wake();
            }
        }
    }

    impl Datagram for Arc<Gate> {
        fn poll_send(&self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            if !self.open.load(Ordering::SeqCst) {
                return Poll::Pending;
            }
            self.sent.lock().unwrap().push(buf.to_vec());
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_send_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            if !self.says_when {
                return Poll::Ready(Ok(()));
            }
            *self.sender.lock().unwrap() = Some(cx.waker().clone());
            if self.open.load(Ordering::SeqCst) {
                return Poll::Ready(Ok(()));
            }
            Poll::Pending
        }

        fn poll_recv(&self, _cx: &mut Context<'_>, _buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    fn initial() -> Transmit<'static> {
        Transmit {
            destination: STAND_IN,
            ecn: None,
            contents: b"initial",
            segment_size: None,
            src_ip: None,
        }
    }

    /// A carrier that cannot send now blocks, and its poller wakes quinn
    /// when it can: the datagram goes out then, none was dropped.
    #[tokio::test]
    async fn a_carrier_that_cannot_send_now_has_quinn_wait_for_it() {
        let gate = Arc::new(Gate {
            says_when: true,
            ..Gate::default()
        });
        let socket = Arc::new(Carried::new(Box::new(gate.clone())));
        let blocked = socket.try_send(&initial()).unwrap_err();
        assert_eq!(blocked.kind(), io::ErrorKind::WouldBlock);
        // as quinn sends: once the poller says so, and again while the
        // socket blocks
        let mut poller = socket.clone().create_io_poller();
        let mut send = pin!(poll_fn(|cx| {
            loop {
                ready!(poller.as_mut().poll_writable(cx))?;
                match socket.try_send(&initial()) {
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    sent => return Poll::Ready(sent),
                }
            }
        }));
        let waiting = timeout(Duration::from_millis(100), &mut send).await;
        assert!(waiting.is_err(), "sent through a carrier that cannot send");
        gate.open();
        timeout(Duration::from_secs(5), send)
            .await
            .expect("woken once the carrier can send")
            .unwrap();
        assert_eq!(*gate.sent.lock().unwrap(), [b"initial".to_vec()]);
    }

    /// A carrier that cannot send now and cannot say when drops the
    /// datagram: quinn is not left trying for ever.
    #[test]
    fn a_carrier_that_cannot_say_when_drops_what_it_cannot_send() {
        let gate = Arc::new(Gate::default());
        let socket = Carried::new(Box::new(gate.clone()));
        socket.try_send(&initial()).unwrap();
        gate.open();
        assert!(gate.sent.lock().unwrap().is_empty());
        socket.try_send(&initial()).unwrap();
        assert_eq!(*gate.sent.lock().unwrap(), [b"initial".to_vec()]);
    }
}
