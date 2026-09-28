//! A running tunnel (phase 2 M4 design 6.1, 6.5): the stack behind one lock
//! and the task that drives it. Only the task touches the peers' carriers,
//! and nothing sends, receives or waits with the lock held.

use crate::stack::{Outgoing, Stack};
use crate::stream::TunnelStream;
use crate::wire;
use rurge_config::wireguard::WireGuardSection;
use rurge_net::connector::{BoxedDatagram, ConnectOpts, Connector, Target};
use rurge_proto::OutboundError;
use std::future::{Future, poll_fn};
use std::io;
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};
use tokio::io::ReadBuf;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

/// How often WireGuard's timers run (M4 design 6.1).
const TICK: Duration = Duration::from_millis(250);
/// How many datagrams the carriers may hand in before the stack runs.
const BATCH: usize = 256;

pub(crate) struct Shared {
    pub(crate) stack: Mutex<Stack>,
    /// Wakes the task: a stream read, wrote or closed, a connection opened.
    kick: Notify,
}

impl Shared {
    pub(crate) fn kick(&self) {
        self.kick.notify_one();
    }
}

/// The tunnel while it runs: as long as the outbound or a connection
/// through it holds it.
pub struct Device {
    pub(crate) shared: Arc<Shared>,
    _task: AbortOnDropHandle<()>,
}

struct Carrier {
    datagram: BoxedDatagram,
    /// Handshake initiations go out marked (`set_tos` has not failed yet).
    marks: bool,
}

/// What a peer's carrier failing to come up means for the dial.
fn unreachable(e: io::Error) -> OutboundError {
    if e.kind() == io::ErrorKind::Unsupported {
        // a chain carries no UDP before M5 (M4-D7)
        OutboundError::Unsupported("wireguard over underlying-proxy".to_string())
    } else {
        OutboundError::from(e)
    }
}

impl Device {
    /// A carrier to every peer, then the task and a handshake with each
    /// peer. Fails only when no peer can be reached at all: one that cannot
    /// is left without a carrier.
    pub(crate) async fn start(
        policy: &str,
        section: &WireGuardSection,
        connector: &Arc<dyn Connector>,
        opts: &ConnectOpts,
    ) -> Result<Arc<Device>, OutboundError> {
        let mut dials = JoinSet::new();
        for (i, peer) in section.peers.iter().enumerate() {
            let endpoint = Target::new(peer.endpoint.host.clone(), peer.endpoint.port);
            let (connector, opts) = (connector.clone(), opts.clone());
            dials.spawn(async move { (i, connector.connect_udp(&endpoint, &opts).await) });
        }
        let mut carriers: Vec<Option<Carrier>> = section.peers.iter().map(|_| None).collect();
        let mut failure = None;
        while let Some(joined) = dials.join_next().await {
            let Ok((i, dialled)) = joined else {
                continue;
            };
            match dialled {
                Ok(datagram) => {
                    carriers[i] = Some(Carrier {
                        datagram,
                        marks: true,
                    })
                }
                Err(e) => {
                    tracing::warn!(policy, peer = i + 1, error = %e, "wireguard: the peer cannot be reached");
                    failure = Some(e);
                }
            }
        }
        if carriers.iter().all(Option::is_none) {
            return Err(unreachable(failure.unwrap_or_else(|| {
                io::Error::other("wireguard: the tunnel has no peer")
            })));
        }
        let shared = Arc::new(Shared {
            stack: Mutex::new(Stack::new(section)),
            kick: Notify::new(),
        });
        let mut out = Vec::new();
        shared.stack.lock().expect("the tunnel").initiate(&mut out);
        let task = tokio::spawn(run(shared.clone(), carriers, out));
        Ok(Arc::new(Device {
            shared,
            _task: AbortOnDropHandle::new(task),
        }))
    }

    /// A TCP connection to `to` through the tunnel, once it is established.
    pub(crate) async fn connect(
        self: &Arc<Device>,
        to: SocketAddr,
    ) -> Result<TunnelStream, OutboundError> {
        let handle = self
            .shared
            .stack
            .lock()
            .expect("the tunnel")
            .connect(to)
            .map_err(|refusal| OutboundError::Proxy(refusal.to_string()))?;
        self.shared.kick();
        // the stream owns the connection from here: dropped, it goes away
        let stream = TunnelStream::new(self.clone(), handle);
        poll_fn(|cx| stream.poll_established(cx)).await?;
        Ok(stream)
    }
}

/// Every carrier's next datagram, whichever comes first; the carriers take
/// turns at being asked first.
fn recv_any<'a>(
    carriers: &'a [Option<Carrier>],
    buf: &'a mut [u8],
    first: &'a mut usize,
) -> impl Future<Output = (usize, io::Result<usize>)> + 'a {
    poll_fn(move |cx| {
        let n = carriers.len();
        for k in 0..n {
            let i = (*first + k) % n;
            let Some(carrier) = &carriers[i] else {
                continue;
            };
            let mut read = ReadBuf::new(&mut *buf);
            if let Poll::Ready(received) = carrier.datagram.poll_recv(cx, &mut read) {
                *first = (i + 1) % n;
                return Poll::Ready((i, received.map(|()| read.filled().len())));
            }
        }
        Poll::Pending
    })
}

/// A datagram some carrier holds already, without waiting for one.
fn ready(
    carriers: &[Option<Carrier>],
    buf: &mut [u8],
    first: &mut usize,
) -> Option<(usize, io::Result<usize>)> {
    let mut cx = Context::from_waker(Waker::noop());
    match pin!(recv_any(carriers, buf, first)).poll(&mut cx) {
        Poll::Ready(next) => Some(next),
        Poll::Pending => None,
    }
}

async fn send(carriers: &mut [Option<Carrier>], message: Outgoing) {
    let Some(Some(carrier)) = carriers.get_mut(message.peer) else {
        // a peer without a carrier: nothing reaches it
        return;
    };
    let marked =
        carrier.marks && wire::message_type(&message.datagram) == Some(wire::HANDSHAKE_INITIATION);
    if marked && carrier.datagram.set_tos(wire::HANDSHAKE_TOS).is_err() {
        carrier.marks = false;
    }
    let datagram = &carrier.datagram;
    if let Err(e) = poll_fn(|cx| datagram.poll_send(cx, &message.datagram)).await {
        tracing::trace!(peer = message.peer, error = %e, "wireguard: a message was not sent");
    }
    if marked && carrier.marks {
        let _ = carrier.datagram.set_tos(0);
    }
}

async fn run(shared: Arc<Shared>, mut carriers: Vec<Option<Carrier>>, mut out: Vec<Outgoing>) {
    let mut buf = vec![0u8; 65536];
    let mut next_tick = Instant::now() + TICK;
    let mut first = 0;
    loop {
        let deadline = {
            let mut stack = shared.stack.lock().expect("the tunnel");
            let now = Instant::now();
            if now >= next_tick {
                stack.tick(&mut out);
                next_tick = now + TICK;
            }
            let wait = stack.advance(now, &mut out);
            wait.map_or(next_tick, |wait| (now + wait).min(next_tick))
        };
        for message in out.drain(..) {
            send(&mut carriers, message).await;
        }
        tokio::select! {
            _ = shared.kick.notified() => {}
            (peer, received) = recv_any(&carriers, &mut buf, &mut first) => {
                let mut arrived = Some((peer, received));
                let mut taken = 0;
                while let Some((peer, received)) = arrived {
                    match received {
                        // the lock is for the stack alone: the carriers are
                        // read outside it
                        Ok(n) => shared.stack.lock().expect("the tunnel").receive(
                            peer,
                            &mut buf[..n],
                            Instant::now(),
                            &mut out,
                        ),
                        // an ICMP error the system reports on a connected socket
                        Err(e) => tracing::trace!(peer, error = %e, "wireguard: a carrier failed to receive"),
                    }
                    taken += 1;
                    // what else has arrived goes in before the stack runs
                    arrived = if taken < BATCH {
                        ready(&carriers, &mut buf, &mut first)
                    } else {
                        None
                    };
                }
            }
            _ = tokio::time::sleep_until(deadline.into()) => {}
        }
    }
}
