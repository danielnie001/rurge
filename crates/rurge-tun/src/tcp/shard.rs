//! One shard of the TUN's TCP (phase 3 M3 design 5.1): a smoltcp
//! `Interface` and `SocketSet` of its own behind one lock, the connections on
//! them, and the task that advances them.
//!
//! The lock is held for a turn of the task and, briefly, for a read or a
//! write of a stream; never across an await. The task waits for packets, a
//! kick of a stream, its timer or the stop without it, and hands what the
//! stack sent to the writer without it.

use super::budget::Sizes;
use super::{
    Admission, Budget, ConnId, ConnShared, EGRESS_PASSES, INGRESS_PER_WAKE, Outcome, SWEEP,
    TIME_WAIT_GRACE, TunStream, Tuple,
};
use crate::packet::{self, Reason, Segment};
use crate::session;
use crate::space::{self, Place, STACK_V4, STACK_V6};
use crate::{Life, TunOptions};
use rurge_inbound::Dialer;
use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{self, Checksum, DeviceCapabilities, Medium};
use smoltcp::socket::tcp;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpListenEndpoint};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc};
use tokio_util::task::TaskTracker;

pub(crate) struct Shard {
    state: Mutex<ShardState>,
    /// A stream did something the shard has to act on: it read (there is
    /// room in the window to tell of), wrote (there is data to send), shut
    /// its write side down, or went.
    kick: Notify,
}

impl Shard {
    /// `seed` is what smoltcp draws the initial sequence numbers of the
    /// shard's connections from.
    pub(crate) fn new(
        mtu: usize,
        epoch: Instant,
        seed: u64,
        budget: Arc<Budget>,
        admission: Arc<Admission>,
        options: &TunOptions,
    ) -> Shard {
        Shard {
            state: Mutex::new(ShardState::new(
                mtu, epoch, seed, budget, admission, options,
            )),
            kick: Notify::new(),
        }
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, ShardState> {
        self.state.lock().expect("a TCP shard's state")
    }

    /// The state, waiting for the lock as `state()` does; where `state()`
    /// panics on a poisoned lock, this is `None`. For a stream's `Drop`,
    /// which must not panic.
    pub(crate) fn state_unless_poisoned(&self) -> Option<MutexGuard<'_, ShardState>> {
        self.state.lock().ok()
    }

    pub(crate) fn kick(&self) {
        self.kick.notify_one();
    }
}

pub(crate) struct ShardState {
    iface: Interface,
    pub(crate) sockets: SocketSet<'static>,
    device: ShardDevice,
    pub(crate) conns: HashMap<ConnId, Conn>,
    pub(crate) by_tuple: HashMap<Tuple, ConnId>,
    /// Connections a turn or a stream touched since the last reap, which
    /// the next reap looks at.
    pub(crate) touched: HashSet<ConnId>,
    /// What smoltcp's clock counts from: the TUN's start.
    epoch: Instant,
    /// When the next reap looks at every connection.
    next_sweep: Instant,
    /// The connections still half-open by the time they run out, soonest
    /// first: what `reap` aborts when the time comes.
    half_open: BTreeSet<(Instant, ConnId)>,
    budget: Arc<Budget>,
    /// The caps and limits of the whole TUN, which a new SYN meets.
    admission: Arc<Admission>,
    /// Every socket's, from `TunOptions`.
    keep_alive: Duration,
    timeout: Duration,
    /// How long a connection has to complete its handshake.
    half_open_timeout: Duration,
}

/// One connection: the socket that has it, and what the shard knows of it
/// besides smoltcp's state.
pub(crate) struct Conn {
    pub(crate) handle: SocketHandle,
    pub(crate) tuple: Tuple,
    sizes: Sizes,
    shared: Arc<ConnShared>,
    /// How a segment of the app closed the socket, if one did: smoltcp does
    /// not say that it took a reset, so the shard sees it as it hands the
    /// segment in (P12).
    pub(crate) closed_by: Option<Outcome>,
    /// When the socket was first seen in TIME-WAIT.
    time_wait: Option<Instant>,
    /// While the handshake is not over: the time it runs out. The connection
    /// counts against `max_half_open` as long as this is `Some`, which it is
    /// until the first time the shard sees the socket past SYN-RECEIVED,
    /// aborts it for running out of time, or lets the connection go.
    half_open: Option<Instant>,
    /// The stream shut its write side down.
    pub(crate) shut: bool,
    /// The stream is gone.
    pub(crate) dropped: bool,
}

/// What one turn of a shard gave out.
pub(crate) struct Turn {
    /// The packets the stack sent, in order.
    pub(crate) out: Vec<Vec<u8>>,
    /// The connections whose SYN the stack took, for their sessions.
    pub(crate) opened: Vec<Opened>,
    /// When the shard has something to do next if nothing comes before.
    pub(crate) wake_at: Option<Instant>,
    /// The turn reached one of its bounds: another turn is due at once,
    /// after a yield.
    pub(crate) more: bool,
}

pub(crate) struct Opened {
    pub(crate) id: ConnId,
    pub(crate) shared: Arc<ConnShared>,
    pub(crate) tuple: Tuple,
}

impl ShardState {
    fn new(
        mtu: usize,
        epoch: Instant,
        seed: u64,
        budget: Arc<Budget>,
        admission: Arc<Admission>,
        options: &TunOptions,
    ) -> ShardState {
        let mut device = ShardDevice {
            rx: None,
            tx: Vec::new(),
            mtu,
        };
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = seed;
        let mut iface = Interface::new(config, &mut device, smoltcp::time::Instant::ZERO);
        iface.set_any_ip(true);
        iface.update_ip_addrs(|addresses| {
            for address in [
                IpCidr::new(IpAddress::Ipv4(STACK_V4), 32),
                IpCidr::new(IpAddress::Ipv6(STACK_V6), 128),
            ] {
                addresses
                    .push(address)
                    .expect("an interface takes two addresses");
            }
        });
        let routes = iface.routes_mut();
        routes
            .add_default_ipv4_route(STACK_V4)
            .expect("an interface takes two routes");
        routes
            .add_default_ipv6_route(STACK_V6)
            .expect("an interface takes two routes");
        ShardState {
            iface,
            sockets: SocketSet::new(Vec::new()),
            device,
            conns: HashMap::new(),
            by_tuple: HashMap::new(),
            touched: HashSet::new(),
            epoch,
            next_sweep: epoch + SWEEP,
            half_open: BTreeSet::new(),
            budget,
            admission,
            keep_alive: options.keep_alive,
            timeout: options.timeout,
            half_open_timeout: options.half_open_timeout,
        }
    }

    fn at(&self, now: Instant) -> smoltcp::time::Instant {
        smoltcp::time::Instant::from_micros(
            now.saturating_duration_since(self.epoch).as_micros() as i64
        )
    }

    /// One wake-up of the shard. One egress pass comes first (P21): a
    /// window update for the room a stream's read made since the last turn
    /// goes out before any new segment comes in. Then at most
    /// `INGRESS_PER_WAKE` packets of `incoming`, one at a time; then egress
    /// until the stack has nothing more to send, at most `EGRESS_PASSES`
    /// passes; then the connections that are over go. It starts no task and
    /// reads no clock: `now` is the caller's.
    pub(crate) fn turn(&mut self, now: Instant, incoming: &mut Vec<Vec<u8>>) -> Turn {
        let at = self.at(now);
        self.iface
            .poll_egress(at, &mut self.device, &mut self.sockets);
        let taken = incoming.len().min(INGRESS_PER_WAKE);
        let mut opened = Vec::new();
        for packet in incoming.drain(..taken) {
            self.ingest(packet, now, at, &mut opened);
        }
        let mut drained = false;
        for _ in 0..EGRESS_PASSES {
            // a pass sends at most one segment of each socket
            if self
                .iface
                .poll_egress(at, &mut self.device, &mut self.sockets)
                == PollResult::None
            {
                drained = true;
                break;
            }
        }
        let out = std::mem::take(&mut self.device.tx);
        let reap_at = self.reap(now);
        let stack_at = self
            .iface
            .poll_delay(at, &self.sockets)
            .map(|delay| now + std::time::Duration::from(delay));
        let sweep_at = (!self.conns.is_empty()).then_some(self.next_sweep);
        Turn {
            out,
            opened,
            wake_at: [stack_at, reap_at, sweep_at].into_iter().flatten().min(),
            more: taken == INGRESS_PER_WAKE || !drained,
        }
    }

    /// One packet into the stack. A segment of a connection goes to its
    /// socket; a SYN of none is looked at (`new_syn`); any other segment
    /// goes in too, and smoltcp answers it with a reset.
    fn ingest(
        &mut self,
        packet: Vec<u8>,
        now: Instant,
        at: smoltcp::time::Instant,
        opened: &mut Vec<Opened>,
    ) {
        // nothing but TCP that passed the filter comes to a shard
        let segment = packet::segment(&packet);
        debug_assert!(
            segment.is_some(),
            "a shard is given only what classify let through"
        );
        let Some(segment) = segment else {
            return;
        };
        if let Some(&id) = self.by_tuple.get(&segment.tuple) {
            let handle = self
                .conns
                .get(&id)
                .expect("a connection of the tuple table")
                .handle;
            let before = self.sockets.get::<tcp::Socket>(handle).state();
            self.advance(packet, at);
            let after = self.sockets.get::<tcp::Socket>(handle).state();
            if after == tcp::State::Listen {
                // A reset in SYN-RECEIVED sends the socket back to listening
                // (smoltcp tcp.rs): without its 4-tuple it would take the next
                // SYN to this destination from any source, and the tuple
                // table would not know whose connection that was. It is no
                // connection any more.
                self.remove(id, Outcome::Reset);
                return;
            }
            if before == tcp::State::SynReceived && after != tcp::State::SynReceived {
                self.end_half_open(id);
            }
            if before != tcp::State::Closed && after == tcp::State::Closed {
                let conn = self
                    .conns
                    .get_mut(&id)
                    .expect("a connection of the tuple table");
                conn.closed_by.get_or_insert(if segment.rst {
                    Outcome::Reset
                } else {
                    Outcome::Closed
                });
            }
            self.touched.insert(id);
        } else if segment.syn && !segment.ack && !segment.rst && !segment.fin {
            // a SYN with RST or FIN as well is none that smoltcp reads, and
            // a reset is never the answer to a reset
            self.new_syn(packet, segment, now, at, opened);
        } else {
            self.advance(packet, at);
        }
    }

    fn advance(&mut self, packet: Vec<u8>, at: smoltcp::time::Instant) {
        self.device.rx = Some(packet);
        self.iface
            .poll_ingress_single(at, &mut self.device, &mut self.sockets);
        self.device.rx = None;
    }

    /// A SYN of no connection (phase 3 M3 design 5.2). In this order: the
    /// TUN is stopping; the caps on sockets and on half-open connections are
    /// met; the destination is an address of rurge's own, or one that stands
    /// for a name. In each case the SYN is turned away with a reset. Any
    /// other SYN is the first of a connection (`open`).
    ///
    /// The places the connection would take in the caps are claimed in one
    /// step, not looked at here and taken later, so that the caps hold when
    /// shards take places at once. A place claimed for a SYN that is turned
    /// away is given back.
    fn new_syn(
        &mut self,
        packet: Vec<u8>,
        segment: Segment,
        now: Instant,
        at: smoltcp::time::Instant,
        opened: &mut Vec<Opened>,
    ) {
        if self.admission.stopping() {
            return self.turn_away(&packet, &segment, now, None);
        }
        if !self.admission.claim() {
            return self.turn_away(&packet, &segment, now, Some(Reason::Busy));
        }
        match space::place(segment.tuple.dst.ip()) {
            Place::Real => self.open(packet, segment, now, at, opened),
            // the responder's port 53 is a place for a connection of its own
            // (M3c); every other port of it is not
            Place::Responder | Place::Own => {
                self.admission.unclaim();
                self.turn_away(&packet, &segment, now, None);
            }
            // there is no pool of fake addresses yet: each names nothing
            Place::Fake => {
                self.admission.unclaim();
                self.turn_away(&packet, &segment, now, Some(Reason::UnknownFake));
            }
        }
    }

    /// Answers `syn` with a reset, as rurge's own resets are let through
    /// (`Admission::allow_reset`); one that is not is dropped and counted.
    /// `why` is what is counted of the SYN itself, if anything is.
    fn turn_away(&mut self, syn: &[u8], segment: &Segment, now: Instant, why: Option<Reason>) {
        let (src, dst) = (segment.tuple.src.ip(), segment.tuple.dst.ip());
        if let Some(reason) = why {
            self.admission.count(reason, src, dst);
        }
        if self.admission.allow_reset(now) {
            self.device.tx.push(packet::rst_for(syn));
        } else {
            self.admission.count(Reason::RstLimited, src, dst);
        }
    }

    /// A SYN that is let in (design 5.2, step 5): a socket that listens on
    /// the SYN's destination alone, then the SYN. The socket has the
    /// connection if the stack took the SYN; packets go in one at a time, so
    /// no other SYN to the same destination can come between. The places the
    /// connection takes in the TUN's caps were claimed by `new_syn`; they are
    /// given back if no connection comes of the SYN.
    fn open(
        &mut self,
        packet: Vec<u8>,
        segment: Segment,
        now: Instant,
        at: smoltcp::time::Instant,
        opened: &mut Vec<Opened>,
    ) {
        let tuple = segment.tuple;
        let sizes = self.budget.take();
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; sizes.rx]),
            tcp::SocketBuffer::new(vec![0; sizes.tx]),
        );
        socket.set_nagle_enabled(false);
        // smoltcp resets a connection that has had nothing from the app for
        // `timeout`, whether or not anything is unacknowledged: the
        // keep-alives have a live app answer, so only a silent one is cut off
        socket.set_keep_alive(Some(self.keep_alive.into()));
        socket.set_timeout(Some(self.timeout.into()));
        socket.set_congestion_control(tcp::CongestionControl::Reno);
        let endpoint = IpListenEndpoint {
            addr: Some(tuple.dst.ip().into()),
            port: tuple.dst.port(),
        };
        if socket.listen(endpoint).is_err() {
            self.budget.give_back(sizes);
            self.admission.unclaim();
            return;
        }
        let handle = self.sockets.add(socket);
        self.advance(packet, at);
        if self.sockets.get::<tcp::Socket>(handle).state() != tcp::State::SynReceived {
            self.sockets.remove(handle);
            self.budget.give_back(sizes);
            self.admission.unclaim();
            return;
        }
        let id = ConnId::next();
        let shared = Arc::new(ConnShared::default());
        let deadline = now + self.half_open_timeout;
        let conn = Conn {
            handle,
            tuple,
            sizes,
            shared: shared.clone(),
            closed_by: None,
            time_wait: None,
            half_open: Some(deadline),
            shut: false,
            dropped: false,
        };
        self.conns.insert(id, conn);
        self.by_tuple.insert(tuple, id);
        self.half_open.insert((deadline, id));
        opened.push(Opened { id, shared, tuple });
    }

    /// Lets go of the connections that are over, of those touched since the
    /// last reap, or of all of them once a `SWEEP`: a socket in TIME-WAIT
    /// `TIME_WAIT_GRACE` after it was first seen there, a closed socket once
    /// it has no ends left (an aborted socket keeps them until its reset is
    /// out). Neither goes while its stream is there to read what the app
    /// sent before its FIN (P12), unless the app reset the connection. First
    /// the connections that ran out of time for their handshake are aborted
    /// (`expire_half_open`). When a connection kept for its grace or a
    /// handshake that is still going on is to be looked at next, if any is.
    fn reap(&mut self, now: Instant) -> Option<Instant> {
        let mut next = self.expire_half_open(now);
        let ids: Vec<ConnId> = if now >= self.next_sweep {
            self.next_sweep = now + SWEEP;
            self.touched.clear();
            self.conns.keys().copied().collect()
        } else {
            self.touched.drain().collect()
        };
        let mut over = Vec::new();
        for id in ids {
            let Some(conn) = self.conns.get_mut(&id) else {
                continue;
            };
            let socket = self.sockets.get::<tcp::Socket>(conn.handle);
            let unread =
                !conn.dropped && conn.closed_by != Some(Outcome::Reset) && socket.recv_queue() > 0;
            match socket.state() {
                tcp::State::TimeWait => {
                    let due = *conn.time_wait.get_or_insert(now) + TIME_WAIT_GRACE;
                    if now < due {
                        next = Some(next.map_or(due, |soonest| soonest.min(due)));
                        self.touched.insert(id);
                    } else if !unread {
                        over.push((id, Outcome::Closed));
                    }
                }
                // its reset is still to go out
                tcp::State::Closed if socket.remote_endpoint().is_some() => {
                    self.touched.insert(id);
                }
                tcp::State::Closed if !unread => {
                    over.push((id, conn.closed_by.unwrap_or(Outcome::Reset)));
                }
                _ => {}
            }
        }
        for (id, outcome) in over {
            self.remove(id, outcome);
        }
        next
    }

    /// Aborts the connections whose handshake has run out of time (design
    /// 5.4: smoltcp does not limit SYN-RECEIVED itself): its socket sends a
    /// reset in a later egress pass, and `reap` lets the connection go once
    /// that is out and not before (the app would be left without it). From
    /// the abort on the connection no longer counts as half-open. When the
    /// soonest of the handshakes still going on runs out, if any is.
    fn expire_half_open(&mut self, now: Instant) -> Option<Instant> {
        while let Some(&(deadline, id)) = self.half_open.first() {
            if deadline > now {
                return Some(deadline);
            }
            self.end_half_open(id);
            let conn = self
                .conns
                .get(&id)
                .expect("a connection of the half-open table");
            self.sockets.get_mut::<tcp::Socket>(conn.handle).abort();
            self.touched.insert(id);
        }
        None
    }

    /// The handshake of a connection is over, or the connection is going
    /// without it: it stops counting as half-open. Once for each connection.
    fn end_half_open(&mut self, id: ConnId) {
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if let Some(deadline) = conn.half_open.take() {
            self.half_open.remove(&(deadline, id));
            self.admission.release_half_open();
        }
    }

    /// Lets go of a connection: its socket, its entries and its buffers; its
    /// stream answers `outcome` from now on, and whoever waits on it wakes.
    fn remove(&mut self, id: ConnId, outcome: Outcome) {
        self.end_half_open(id);
        let Some(conn) = self.conns.remove(&id) else {
            return;
        };
        if self.by_tuple.get(&conn.tuple) == Some(&id) {
            self.by_tuple.remove(&conn.tuple);
        }
        self.touched.remove(&id);
        self.sockets.remove(conn.handle);
        self.budget.give_back(conn.sizes);
        self.admission.release_socket();
        conn.shared.end(outcome);
    }

    /// Lets go of every connection, each with a reset: the shard is ending.
    pub(crate) fn end_all(&mut self) {
        let ids: Vec<ConnId> = self.conns.keys().copied().collect();
        for id in ids {
            self.remove(id, Outcome::Reset);
        }
    }

    #[cfg(test)]
    pub(crate) fn socket_count(&self) -> usize {
        self.sockets.iter().count()
    }
}

/// The task of a shard. It waits for packets, a kick, its timer or the stop;
/// takes a turn in the lock; then, out of it, starts the sessions of the new
/// connections and hands what the stack sent to the writer, waiting while
/// the writer's queue is full. After a turn that reached a bound it goes on
/// at once, after a yield.
pub(crate) async fn run(
    shard: Arc<Shard>,
    mut packets: mpsc::Receiver<Vec<u8>>,
    writer: mpsc::Sender<Vec<Vec<u8>>>,
    dialer: Arc<dyn Dialer>,
    life: Arc<Life>,
    tasks: TaskTracker,
) {
    let mut incoming = Vec::with_capacity(INGRESS_PER_WAKE);
    // the dispatcher can still send
    let mut open = true;
    let mut more = false;
    let mut wake_at: Option<Instant> = None;
    loop {
        if more {
            tokio::task::yield_now().await;
            if life.is_stopped() {
                break;
            }
            while incoming.len() < INGRESS_PER_WAKE {
                let Ok(packet) = packets.try_recv() else {
                    break;
                };
                incoming.push(packet);
            }
        } else {
            let sleep = tokio::time::sleep_until(
                wake_at.map_or_else(tokio::time::Instant::now, tokio::time::Instant::from_std),
            );
            tokio::select! {
                biased;
                _ = life.stopped() => break,
                received = packets.recv_many(&mut incoming, INGRESS_PER_WAKE), if open => {
                    if received == 0 {
                        open = false;
                    }
                }
                _ = shard.kick.notified() => {}
                _ = sleep, if wake_at.is_some() => {}
            }
        }
        let turn = shard.state().turn(Instant::now(), &mut incoming);
        for opened in turn.opened {
            let stream = TunStream::new(shard.clone(), opened.id, opened.shared);
            tasks.spawn(session::serve(stream, dialer.clone(), opened.tuple));
        }
        if !turn.out.is_empty() {
            tokio::select! {
                biased;
                _ = life.stopped() => break,
                sent = writer.send(turn.out) => {
                    // the writer has ended: so has the TUN
                    if sent.is_err() {
                        break;
                    }
                }
            }
        }
        more = turn.more;
        wake_at = turn.wake_at;
    }
    shard.state().end_all();
}

/// The shard's side of its stack's device: one packet at a time on its way
/// in, everything the stack sends on its way out. Sending never fails, so
/// no ACK is lost to a device that is busy (phase 3 M3 design 4.5).
struct ShardDevice {
    rx: Option<Vec<u8>>,
    tx: Vec<Vec<u8>>,
    mtu: usize,
}

struct RxToken(Vec<u8>);

struct TxToken<'a>(&'a mut Vec<Vec<u8>>);

impl phy::Device for ShardDevice {
    type RxToken<'a>
        = RxToken
    where
        Self: 'a;
    type TxToken<'a>
        = TxToken<'a>
    where
        Self: 'a;

    fn receive(
        &mut self,
        _timestamp: smoltcp::time::Instant,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let packet = self.rx.take()?;
        Some((RxToken(packet), TxToken(&mut self.tx)))
    }

    fn transmit(&mut self, _timestamp: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        Some(TxToken(&mut self.tx))
    }

    /// TCP checksums are computed on what goes out and not checked on what
    /// comes in: the operating system's stack computed them, and the link is
    /// in memory. IPv4 header checksums are computed going out too; checking
    /// those coming in is the dispatcher's (phase 3 M3 design 4.4).
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps.checksum.ipv4 = Checksum::Tx;
        caps.checksum.tcp = Checksum::Tx;
        caps
    }
}

impl phy::RxToken for RxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut packet = vec![0; len];
        let done = f(&mut packet);
        self.0.push(packet);
        done
    }
}
