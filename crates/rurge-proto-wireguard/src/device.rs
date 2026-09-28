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
/// How often at most a peer's carrier is replaced because sending on it
/// failed.
const REPLACE: Duration = Duration::from_secs(10);
/// How many times at most a question through the tunnel goes out within
/// its wait (`Device::query`).
const SENDS: u32 = 3;

/// The tunnels of this process, their sections and carrier keys. Two
/// tunnels with one private key at one peer would take each other's
/// packets — a peer answers wherever the key last wrote from — so only the
/// policies that name a section *and* a carrier alike share a tunnel
/// (`shared_tunnel`), and a tunnel that starts ends every other conflicting
/// one that is older — its key with a peer in common (`conflict`), whatever
/// its section or carrier. Every lookup, decision and registration that
/// touches this table happens inside one critical section, with no
/// `.await` while the lock is held, so two conflicting tunnels can never
/// both be registered; a dial that only shares an already-running tunnel
/// never waits on another tunnel's own dial (P10).
static TUNNELS: Mutex<Vec<(WireGuardSection, String, Weak<Device>)>> = Mutex::new(Vec::new());

/// Whether a tunnel of `a` and one of `b` would take each other's packets.
fn conflict(a: &WireGuardSection, b: &WireGuardSection) -> bool {
    a.private_key == b.private_key
        && a.peers
            .iter()
            .any(|p| b.peers.iter().any(|q| q.public_key == p.public_key))
}

/// A live tunnel already registered for `section` and `carrier`, if any;
/// also drops the table's dead and closed entries. Brief on its own: a
/// dial that only shares a running tunnel never dials anything, so it never
/// waits behind another tunnel's start (P10).
fn shared_tunnel(section: &WireGuardSection, carrier: &str) -> Option<Arc<Device>> {
    let mut tunnels = TUNNELS.lock().expect("the tunnels");
    tunnels.retain(|(_, _, device)| device.upgrade().is_some_and(|d| !d.is_closed()));
    tunnels
        .iter()
        .find(|(other, key, _)| other == section && key.as_str() == carrier)
        .and_then(|(_, _, device)| device.upgrade())
}

pub(crate) struct Shared {
    pub(crate) stack: Mutex<Stack>,
    /// Wakes the task: a stream read, wrote or closed, a connection opened.
    kick: Notify,
    /// Set by `Device::network_changed`, taken by the task.
    network_changed: AtomicBool,
    /// Set by `Device::handshake`, taken by the task: a handshake with
    /// every peer now.
    greet: AtomicBool,
    /// Told of every handshake that completes.
    handshaken: Notify,
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
    /// The tunnel of `section` and `carrier`: the one running, when a
    /// policy naming them both started it; else a carrier to every peer,
    /// then the task and a handshake with each peer. Fails when no peer can
    /// be reached at all — one that cannot is dialled again every `redial`
    /// — and when a tunnel of a later configuration (`generation`) has
    /// taken over. Never waits on another tunnel's own start: sharing is
    /// decided from the table alone, before any dial (P10).
    pub(crate) async fn start(
        policy: &str,
        section: &WireGuardSection,
        carrier: &str,
        generation: u64,
        connector: &Arc<dyn Connector>,
        opts: &ConnectOpts,
        redial: Duration,
    ) -> Result<Arc<Device>, OutboundError> {
        if let Some(device) = shared_tunnel(section, carrier) {
            return Ok(device);
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
        // one critical section: sharing (another dial may have got there
        // first), conflict and registration decided and acted on together,
        // with no `.await` inside it — so two conflicting tunnels can never
        // both be registered
        let mut tunnels = TUNNELS.lock().expect("the tunnels");
        tunnels.retain(|(_, _, device)| device.upgrade().is_some_and(|d| !d.is_closed()));
        for (other, key, device) in tunnels.iter() {
            let Some(device) = device.upgrade() else {
                continue;
            };
            if other == section && key.as_str() == carrier {
                // the carriers just dialled are dropped with this frame
                return Ok(device);
            }
            if conflict(other, section) && device.generation > generation {
                return Err(OutboundError::Proxy(
                    "wireguard: a newer configuration of this tunnel is in use".to_string(),
                ));
            }
        }
        // the tunnel it replaces goes quiet before a peer hears of this one
        tunnels.retain(|(other, _, device)| {
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
            greet: AtomicBool::new(false),
            handshaken: Notify::new(),
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
            replaced: vec![None; peers],
        };
        let task = tokio::spawn(driver.run(out));
        let device = Arc::new(Device {
            shared,
            task: AbortOnDropHandle::new(task),
            generation,
        });
        tunnels.push((
            section.clone(),
            carrier.to_string(),
            Arc::downgrade(&device),
        ));
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

    /// A handshake with every peer now, however fresh their sessions: how
    /// long until the first completed (phase 2 M4 design 6.7).
    pub(crate) async fn handshake(&self) -> Duration {
        let asked = Instant::now();
        self.shared.greet.store(true, Ordering::SeqCst);
        self.shared.kick();
        loop {
            let mut completed = pin!(self.shared.handshaken.notified());
            completed.as_mut().enable();
            let last = self
                .shared
                .stack
                .lock()
                .expect("the tunnel")
                .last_handshake();
            if let Some(at) = last.filter(|at| *at > asked) {
                return at - asked;
            }
            completed.await;
        }
    }

    /// Once a handshake the tunnel started has completed, with any peer: at
    /// once when one has.
    pub(crate) async fn established(&self) {
        loop {
            let mut completed = pin!(self.shared.handshaken.notified());
            completed.as_mut().enable();
            let last = self
                .shared
                .stack
                .lock()
                .expect("the tunnel")
                .last_handshake();
            if last.is_some() {
                return;
            }
            completed.await;
        }
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
    /// within `wait`. Until one comes, `message` goes out again every
    /// third of `wait`, on the same socket: one lost packet, either way,
    /// does not lose the exchange. `None` when none came, or no peer takes
    /// `server`.
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
        let first = tokio::time::Instant::now();
        let mut answer = pin!(poll_fn(|cx| {
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
        }));
        for round in 1..=SENDS {
            if round > 1 {
                // the same question again: its answer is what `accept` takes
                let _ = self
                    .shared
                    .stack
                    .lock()
                    .expect("the tunnel")
                    .udp(socket.handle)
                    .send_slice(message, server);
                self.shared.kick();
            }
            let until = first + wait * round / SENDS;
            if let Ok(answer) = tokio::time::timeout_at(until, answer.as_mut()).await {
                return Some(answer);
            }
        }
        None
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

/// Sends `message` on `carrier`: whether it went out.
async fn send(carrier: &mut Carrier, message: &Outgoing) -> bool {
    let marked =
        carrier.marks && wire::message_type(&message.datagram) == Some(wire::HANDSHAKE_INITIATION);
    if marked && carrier.datagram.set_tos(wire::HANDSHAKE_TOS).is_err() {
        carrier.marks = false;
    }
    let datagram = &carrier.datagram;
    let sent = poll_fn(|cx| datagram.poll_send(cx, &message.datagram)).await;
    if let Err(e) = &sent {
        tracing::trace!(peer = message.peer, error = %e, "wireguard: a message was not sent");
    }
    if marked && carrier.marks {
        let _ = carrier.datagram.set_tos(0);
    }
    sent.is_ok()
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
    /// When a failed send last had the peer's carrier replaced.
    replaced: Vec<Option<Instant>>,
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

    /// The carriers of `peers` failed to send: the network may have changed
    /// under them. Each is dialled anew, unless a failed send had it
    /// replaced less than `REPLACE` ago.
    fn replace_failed(&mut self, dials: &mut JoinSet<Dialled>, peers: Vec<usize>) {
        let now = Instant::now();
        let due: Vec<usize> = peers
            .into_iter()
            .filter(|&peer| self.replaced[peer].is_none_or(|at| now.duration_since(at) >= REPLACE))
            .collect();
        for &peer in &due {
            self.replaced[peer] = Some(now);
        }
        self.dial(dials, due, true);
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

    /// What goes out now, to the peers that have a carrier; the peers whose
    /// carrier failed to send some of it. Nothing more goes out once a
    /// newer tunnel has taken over: what is left in `out` is dropped (P10 /
    /// B).
    async fn send_all(&mut self, out: &mut Vec<Outgoing>) -> Vec<usize> {
        let mut failed = Vec::new();
        for message in std::mem::take(out) {
            if self.shared.closed.load(Ordering::SeqCst) {
                out.clear();
                return Vec::new();
            }
            let Some(Some(carrier)) = self.carriers.get_mut(message.peer) else {
                continue;
            };
            if wire::message_type(&message.datagram) == Some(wire::HANDSHAKE_INITIATION) {
                self.waiting[message.peer] = true;
            }
            if !send(carrier, &message).await && !failed.contains(&message.peer) {
                failed.push(message.peer);
            }
        }
        failed
    }

    async fn run(mut self, mut out: Vec<Outgoing>) {
        let mut buf = vec![0u8; 65536];
        let mut next_tick = Instant::now() + TICK;
        let mut next_redial = Instant::now() + self.redial;
        let mut first = 0;
        let mut dials = JoinSet::new();
        loop {
            // a newer configuration took over: nothing more goes out (P10 / B)
            if self.shared.closed.load(Ordering::SeqCst) {
                return;
            }
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
            if self.shared.greet.swap(false, Ordering::SeqCst) {
                self.shared
                    .stack
                    .lock()
                    .expect("the tunnel")
                    .initiate(&mut out);
            }
            let (deadline, unanswered) = {
                let mut stack = self.shared.stack.lock().expect("the tunnel");
                let mut unanswered = Vec::new();
                if now >= next_tick {
                    for peer in stack.tick(&mut out) {
                        // once for each handshake that went unanswered
                        if std::mem::take(&mut self.waiting[peer]) {
                            self.up[peer] = false;
                            unanswered.push(peer);
                        }
                    }
                    next_tick = now + TICK;
                }
                let wait = stack.advance(now, &mut out);
                let next = next_tick.min(next_redial);
                (wait.map_or(next, |wait| (now + wait).min(next)), unanswered)
            };
            // logged, and dialled for, with the lock released
            for &peer in &unanswered {
                tracing::warn!(policy = %self.policy, peer = peer + 1, "wireguard: the peer did not answer the handshake");
            }
            // its carrier may be what stopped working (the network changed):
            // a new one, whatever it goes to, and a handshake on it
            self.dial(&mut dials, unanswered, true);
            let failed = self.send_all(&mut out).await;
            self.replace_failed(&mut dials, failed);
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
                    if !completed.is_empty() {
                        self.shared.handshaken.notify_waiters();
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
