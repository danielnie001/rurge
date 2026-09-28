//! A running tunnel (phase 2 M4 design 6.1, 6.5): the stack behind one lock
//! and the task that drives it. Only the task touches the peers' carriers,
//! and nothing sends, receives or waits with the lock held.

use crate::stack::{Outgoing, Stack};
use crate::stream::TunnelStream;
use crate::wire;
use rurge_config::wireguard::WireGuardSection;
use rurge_net::connector::{BoxedDatagram, ConnectOpts, Connector, Target};
use rurge_proto::OutboundError;
use smoltcp::iface::SocketHandle;
use std::future::{Future, poll_fn};
use std::io;
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
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
/// How often the endpoints written as names, and the peers that could not
/// be reached, are dialled again (M4 design 6.5).
pub(crate) const REDIAL: Duration = Duration::from_secs(300);

/// The tunnels of this process and their sections. Two tunnels with one
/// private key at one peer would take each other's packets — a peer answers
/// wherever the key last wrote from — so the policies that name a section
/// share its tunnel, and a tunnel that starts ends the one of an earlier
/// configuration: its key with a peer in common.
static TUNNELS: Mutex<Vec<(WireGuardSection, Weak<Device>)>> = Mutex::new(Vec::new());
/// Tunnels start one at a time.
static STARTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Whether a tunnel of `a` and one of `b` would take each other's packets.
fn conflict(a: &WireGuardSection, b: &WireGuardSection) -> bool {
    a.private_key == b.private_key
        && a.peers
            .iter()
            .any(|p| b.peers.iter().any(|q| q.public_key == p.public_key))
}

pub(crate) struct Shared {
    pub(crate) stack: Mutex<Stack>,
    /// Wakes the task: a stream read, wrote or closed, a connection opened.
    kick: Notify,
    /// Set by `Device::network_changed`, taken by the task.
    network_changed: AtomicBool,
    /// A tunnel of a later configuration took over.
    closed: AtomicBool,
}

impl Shared {
    pub(crate) fn kick(&self) {
        self.kick.notify_one();
    }
}

/// The tunnel while it runs: as long as an outbound or a connection
/// through it holds it.
pub struct Device {
    pub(crate) shared: Arc<Shared>,
    task: AbortOnDropHandle<()>,
    /// The `WireGuardOutbound::generation` that started it.
    generation: u64,
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
    /// The tunnel of `section`: the one running, when a policy naming the
    /// section started it; else a carrier to every peer, then the task and a
    /// handshake with each peer. Fails when no peer can be reached at all —
    /// one that cannot is dialled again every `redial` — and when a tunnel
    /// of a later configuration (`generation`) has taken over.
    pub(crate) async fn start(
        policy: &str,
        section: &WireGuardSection,
        generation: u64,
        connector: &Arc<dyn Connector>,
        opts: &ConnectOpts,
        redial: Duration,
    ) -> Result<Arc<Device>, OutboundError> {
        let _one_at_a_time = STARTING.lock().await;
        {
            let mut tunnels = TUNNELS.lock().expect("the tunnels");
            tunnels.retain(|(_, device)| device.upgrade().is_some_and(|d| !d.is_closed()));
            for (other, device) in tunnels.iter() {
                let Some(device) = device.upgrade() else {
                    continue;
                };
                if other == section {
                    return Ok(device);
                }
                if conflict(other, section) && device.generation > generation {
                    return Err(OutboundError::Proxy(
                        "wireguard: a newer configuration of this tunnel is in use".to_string(),
                    ));
                }
            }
        }
        let endpoints: Vec<Target> = section
            .peers
            .iter()
            .map(|p| Target::new(p.endpoint.host.clone(), p.endpoint.port))
            .collect();
        let mut dials = JoinSet::new();
        for (i, endpoint) in endpoints.iter().enumerate() {
            let (connector, endpoint, opts) = (connector.clone(), endpoint.clone(), opts.clone());
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
        // the tunnel it replaces goes quiet before a peer hears of this one
        TUNNELS
            .lock()
            .expect("the tunnels")
            .retain(|(other, device)| {
                let replaced = conflict(other, section);
                if replaced && let Some(device) = device.upgrade() {
                    device.close();
                }
                !replaced
            });
        let shared = Arc::new(Shared {
            stack: Mutex::new(Stack::new(section)),
            kick: Notify::new(),
            network_changed: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        });
        let mut out = Vec::new();
        shared.stack.lock().expect("the tunnel").initiate(&mut out);
        let peers = carriers.len();
        let driver = Driver {
            shared: shared.clone(),
            policy: policy.to_string(),
            carriers,
            endpoints,
            connector: connector.clone(),
            redial,
            waiting: vec![false; peers],
            up: vec![false; peers],
        };
        let task = tokio::spawn(driver.run(out));
        let device = Arc::new(Device {
            shared,
            task: AbortOnDropHandle::new(task),
            generation,
        });
        TUNNELS
            .lock()
            .expect("the tunnels")
            .push((section.clone(), Arc::downgrade(&device)));
        Ok(device)
    }

    /// A tunnel of a later configuration took over: nothing more goes out,
    /// and every connection through this one fails.
    fn close(&self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        self.task.abort();
        if let Ok(mut stack) = self.shared.stack.lock() {
            stack.abort_all();
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// The network changed: every carrier is dialled anew and each peer
    /// greeted again on its new one (M4 design 6.5).
    pub(crate) fn network_changed(&self) {
        self.shared.network_changed.store(true, Ordering::SeqCst);
        self.shared.kick();
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

    /// One exchange with `server` over UDP through the tunnel: `message`
    /// out, then the first datagram from `server` that `accept` takes,
    /// within `wait`. `None` when none came, or no peer takes `server`.
    pub(crate) async fn query<T>(
        self: &Arc<Device>,
        server: SocketAddr,
        message: &[u8],
        wait: Duration,
        accept: impl Fn(&[u8]) -> Option<T>,
    ) -> Option<T> {
        let socket = {
            let mut stack = self.shared.stack.lock().expect("the tunnel");
            let handle = stack.udp_open(server.ip()).ok()?;
            // the exchange takes the socket once the question is out: its
            // drop takes this lock
            if stack.udp(handle).send_slice(message, server).is_err() {
                stack.udp_close(handle);
                return None;
            }
            UdpExchange {
                device: self.clone(),
                handle,
            }
        };
        self.shared.kick();
        let answer = poll_fn(|cx| {
            let mut stack = self.shared.stack.lock().expect("the tunnel");
            let udp = stack.udp(socket.handle);
            while let Ok((datagram, meta)) = udp.recv() {
                let from = SocketAddr::new(meta.endpoint.addr.into(), meta.endpoint.port);
                if from == server
                    && let Some(answer) = accept(datagram)
                {
                    return Poll::Ready(answer);
                }
            }
            udp.register_recv_waker(cx.waker());
            Poll::Pending
        });
        tokio::time::timeout(wait, answer).await.ok()
    }
}

/// The UDP socket of one exchange: gone with the exchange.
struct UdpExchange {
    device: Arc<Device>,
    handle: SocketHandle,
}

impl Drop for UdpExchange {
    fn drop(&mut self) {
        if let Ok(mut stack) = self.device.shared.stack.lock() {
            stack.udp_close(self.handle);
        }
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

async fn send(carrier: &mut Carrier, message: &Outgoing) {
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

/// A dial of `peer`'s carrier: whether it replaces the one there in any
/// case, and what came of it.
type Dialled = (usize, bool, io::Result<BoxedDatagram>);

/// What the task holds besides the stack.
struct Driver {
    shared: Arc<Shared>,
    policy: String,
    carriers: Vec<Option<Carrier>>,
    endpoints: Vec<Target>,
    connector: Arc<dyn Connector>,
    redial: Duration,
    /// A handshake initiation went to the peer and nothing answered yet.
    waiting: Vec<bool>,
    /// A handshake with the peer completed, and it has not failed to answer
    /// one since.
    up: Vec<bool>,
}

impl Driver {
    /// Dials the carriers of `peers` side by side; `anew`: the new carriers
    /// replace the old whatever they go to.
    fn dial(&self, dials: &mut JoinSet<Dialled>, peers: Vec<usize>, anew: bool) {
        for peer in peers {
            let (connector, endpoint) = (self.connector.clone(), self.endpoints[peer].clone());
            dials.spawn(async move {
                let opts = ConnectOpts::default();
                (peer, anew, connector.connect_udp(&endpoint, &opts).await)
            });
        }
    }

    /// A dialled carrier of `peer` takes the place of the one there when
    /// there is none, when it goes to another address, or when `anew`
    /// says so; the peer is greeted on it at once.
    fn land(
        &mut self,
        peer: usize,
        anew: bool,
        dialled: io::Result<BoxedDatagram>,
        out: &mut Vec<Outgoing>,
    ) {
        let datagram = match dialled {
            Ok(datagram) => datagram,
            Err(e) => {
                tracing::debug!(policy = %self.policy, peer = peer + 1, error = %e, "wireguard: the peer cannot be reached");
                return;
            }
        };
        if let Some(carrier) = &self.carriers[peer]
            && !anew
        {
            if carrier.datagram.peer_addr() == datagram.peer_addr() {
                return;
            }
            tracing::info!(policy = %self.policy, peer = peer + 1, "wireguard: the peer's endpoint moved");
        }
        self.carriers[peer] = Some(Carrier {
            datagram,
            marks: true,
        });
        self.shared
            .stack
            .lock()
            .expect("the tunnel")
            .initiate_peer(peer, out);
    }

    /// What goes out now, to the peers that have a carrier.
    async fn send_all(&mut self, out: &mut Vec<Outgoing>) {
        for message in out.drain(..) {
            let Some(Some(carrier)) = self.carriers.get_mut(message.peer) else {
                continue;
            };
            if wire::message_type(&message.datagram) == Some(wire::HANDSHAKE_INITIATION) {
                self.waiting[message.peer] = true;
            }
            send(carrier, &message).await;
        }
    }

    async fn run(mut self, mut out: Vec<Outgoing>) {
        let mut buf = vec![0u8; 65536];
        let mut next_tick = Instant::now() + TICK;
        let mut next_redial = Instant::now() + self.redial;
        let mut first = 0;
        let mut dials = JoinSet::new();
        loop {
            let now = Instant::now();
            if self.shared.network_changed.swap(false, Ordering::SeqCst) {
                dials.abort_all();
                self.dial(&mut dials, (0..self.endpoints.len()).collect(), true);
                next_redial = now + self.redial;
            } else if now >= next_redial {
                let due = (0..self.endpoints.len())
                    .filter(|&p| {
                        self.carriers[p].is_none() || self.endpoints[p].host.as_domain().is_some()
                    })
                    .collect();
                self.dial(&mut dials, due, false);
                next_redial = now + self.redial;
            }
            let deadline = {
                let mut stack = self.shared.stack.lock().expect("the tunnel");
                if now >= next_tick {
                    for peer in stack.tick(&mut out) {
                        // once for each handshake that went unanswered
                        if std::mem::take(&mut self.waiting[peer]) {
                            self.up[peer] = false;
                            tracing::warn!(policy = %self.policy, peer = peer + 1, "wireguard: the peer did not answer the handshake");
                        }
                    }
                    next_tick = now + TICK;
                }
                let wait = stack.advance(now, &mut out);
                let next = next_tick.min(next_redial);
                wait.map_or(next, |wait| (now + wait).min(next))
            };
            self.send_all(&mut out).await;
            tokio::select! {
                _ = self.shared.kick.notified() => {}
                (peer, received) = recv_any(&self.carriers, &mut buf, &mut first) => {
                    let mut completed = Vec::new();
                    let mut arrived = Some((peer, received));
                    let mut taken = 0;
                    while let Some((peer, received)) = arrived {
                        match received {
                            // the lock is for the stack alone: the carriers
                            // are read outside it
                            Ok(n) => {
                                let mut stack = self.shared.stack.lock().expect("the tunnel");
                                if stack.receive(peer, &mut buf[..n], Instant::now(), &mut out) {
                                    completed.push(peer);
                                }
                            }
                            // an ICMP error the system reports on a connected socket
                            Err(e) => tracing::trace!(peer, error = %e, "wireguard: a carrier failed to receive"),
                        }
                        taken += 1;
                        // what else has arrived goes in before the stack runs
                        arrived = if taken < BATCH {
                            ready(&self.carriers, &mut buf, &mut first)
                        } else {
                            None
                        };
                    }
                    for peer in completed {
                        self.waiting[peer] = false;
                        if !std::mem::replace(&mut self.up[peer], true) {
                            tracing::info!(policy = %self.policy, peer = peer + 1, "wireguard: handshake completed");
                        }
                    }
                }
                Some(joined) = dials.join_next(), if !dials.is_empty() => {
                    if let Ok((peer, anew, dialled)) = joined {
                        self.land(peer, anew, dialled, &mut out);
                    }
                }
                _ = tokio::time::sleep_until(deadline.into()) => {}
            }
        }
    }
}
