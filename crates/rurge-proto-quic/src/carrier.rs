//! What the UDP carriers of the QUIC outbounds share — Hysteria 2's, TUIC's
//! and MASQUE's (phase 2 M7 design 5.3, 6.2, 7.1): when a carrier asks
//! where the policy leads (`RouteClock`), the inbox the engine reads its
//! answers from (`Inbox`), and how binding to a connection, or opening a
//! stream, fails as a send does (`bounded`).

use crate::session::{Lease, Protocol, Session};
use bytes::Bytes;
use rurge_net::connector::{ConnectOpts, Target};
use rurge_proto::OutboundError;
use std::io;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Packets waiting for the engine, per carrier; more are dropped, as a
/// socket would.
pub(crate) const INBOX: usize = 64;

/// How often a carrier asks where the policy leads (`Session::serves`): it
/// can cost a group's resolution, and every datagram would ask.
const ROUTE_EVERY: Duration = Duration::from_secs(1);

/// A packet for the engine, and where it came from.
pub(crate) type Packet = (Bytes, Target);

/// When a carrier last compared the policy's route.
#[derive(Default)]
pub(crate) struct RouteClock {
    checked: Mutex<Option<Instant>>,
}

impl RouteClock {
    /// Whether `lease`'s connection still takes what is new
    /// (`Session::serves`): that it is open and not going away is looked at
    /// on every call, where the policy leads at most once a second
    /// (`serves_without_route` in between).
    pub(crate) fn serves<P: Protocol>(
        &self,
        session: &Session<P>,
        lease: &Lease<P::State>,
    ) -> bool {
        if self.due() {
            session.serves(lease)
        } else {
            session.serves_without_route(lease)
        }
    }

    /// Whether the policy's route is to be compared now: it is when it was
    /// not within the last second, or at all, and this then counts as the
    /// comparison.
    fn due(&self) -> bool {
        let mut checked = self.checked.lock().expect("route checked");
        let now = Instant::now();
        if checked.is_some_and(|at| now.duration_since(at) < ROUTE_EVERY) {
            return false;
        }
        *checked = Some(now);
        true
    }
}

/// A carrier's answers, `INBOX` at most, which the engine reads
/// (`recv_from`) and what reads the server fills through `sender`.
pub(crate) struct Inbox {
    sender: mpsc::Sender<Packet>,
    packets: tokio::sync::Mutex<mpsc::Receiver<Packet>>,
}

impl Inbox {
    pub(crate) fn new() -> Inbox {
        let (sender, packets) = mpsc::channel(INBOX);
        Inbox {
            sender,
            packets: tokio::sync::Mutex::new(packets),
        }
    }

    /// What fills the inbox.
    pub(crate) fn sender(&self) -> &mpsc::Sender<Packet> {
        &self.sender
    }

    /// The next packet, in `buf`, and where it came from. A packet longer
    /// than `buf` is dropped.
    pub(crate) async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, Target)> {
        let mut packets = self.packets.lock().await;
        loop {
            // `self` holds a sender: the inbox never closes
            let Some((packet, from)) = packets.recv().await else {
                return Err(io::ErrorKind::BrokenPipe.into());
            };
            if let Some(space) = buf.get_mut(..packet.len()) {
                space.copy_from_slice(&packet);
                return Ok((packet.len(), from));
            }
        }
    }
}

/// `open` — a carrier binding to the session's connection, or a stream
/// opened for a target, a dial included — within `opts`'s budget, failing
/// as a send fails: its error as an I/O error of the same text, the end of
/// the budget as `TimedOut`.
pub(crate) async fn bounded<T>(
    opts: &ConnectOpts,
    open: impl Future<Output = Result<T, OutboundError>>,
) -> io::Result<T> {
    match tokio::time::timeout(opts.timeout, open).await {
        Ok(Ok(opened)) => Ok(opened),
        Ok(Err(e)) => Err(io::Error::other(e.to_string())),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            OutboundError::Timeout.to_string(),
        )),
    }
}
