//! One shard of the TUN's TCP (phase 3 M3 design 5.1): a smoltcp
//! `Interface` and `SocketSet` of its own behind one lock, the connections on
//! them, and the task that advances them.
//!
//! The lock is held for a turn of the task and, briefly, for a read or a
//! write of a stream; never across an await. The task waits for packets, a
//! kick of a stream, its timer or the stop without it, and hands what the
//! stack sent to the writer without it.

#[cfg(test)]
use super::PANIC_MARKER;
use super::budget::Sizes;
use super::device::ShardDevice;
use super::guard::Edges;
use super::rebuild::{Cause, next_seed, panic_text};
use super::{
    Admission, Budget, ConnId, ConnShared, EGRESS_PASSES, INGRESS_PER_WAKE, Outcome, SWEEP,
    TIME_WAIT_GRACE, TunStream, Tuple,
};
use crate::packet::{self, Reason, Segment};
use crate::session::{self, Holds};
use crate::space::{self, Place, STACK_V4, STACK_V6};
use crate::{Life, TunOptions};
use rurge_inbound::Dialer;
use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpListenEndpoint};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc};
use tokio_util::task::TaskTracker;

/// How far off a deadline is put when the time to the deadline is too long to
/// add to a time: some thirty years.
const FAR_OFF: Duration = Duration::from_secs(30 * 365 * 24 * 60 * 60);

pub(crate) struct Shard {
    /// Which of the TUN's shards this is, for what it says.
    index: usize,
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
        index: usize,
        mtu: usize,
        epoch: Instant,
        seed: u64,
        budget: Arc<Budget>,
        admission: Arc<Admission>,
        options: &TunOptions,
    ) -> Shard {
        Shard {
            index,
            state: Mutex::new(ShardState::new(
                mtu,
                epoch,
                seed,
                budget,
                admission,
                Timers::from(options),
            )),
            kick: Notify::new(),
        }
    }

    /// The state, locked. A lock that a panic poisoned has behind it a stack
    /// that stopped in the middle of what it was doing: whoever finds it so
    /// starts the shard over first, so a stream's read, write or drop never
    /// meets that stack (and none panics for the poison).
    pub(crate) fn state(&self) -> MutexGuard<'_, ShardState> {
        self.lock(Cause::Poisoned)
    }

    /// `state()`, with `cause` for the shard being rebuilt if the lock is
    /// poisoned. The poison is cleared with the rebuild, so a poisoning is
    /// rebuilt for once, by the first to come to the lock.
    fn lock(&self, cause: Cause<'_>) -> MutexGuard<'_, ShardState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                self.rebuild(&mut guard, cause);
                self.state.clear_poison();
                guard
            }
        }
    }

    /// Starts the shard's stack over: every connection is reset, all it held
    /// is given back, and the stack is a new one with a seed of its own.
    /// Counted, and said at WARN with the shard's number and no more; what
    /// the panic said is said at DEBUG.
    fn rebuild(&self, state: &mut ShardState, cause: Cause<'_>) {
        let drawn = getrandom::u64().ok();
        state.rebuild(next_seed(state.seed, drawn));
        state.admission.stack_panicked();
        tracing::warn!(
            shard = self.index,
            "tun: the TCP stack of a shard panicked; its connections are reset"
        );
        tracing::debug!(
            shard = self.index,
            %cause,
            fresh_seed = drawn.is_some(),
            "tun: the TCP stack of a shard was started over"
        );
    }

    pub(crate) fn kick(&self) {
        self.kick.notify_one();
    }
}

/// The times a shard holds its connections to, from `TunOptions`.
#[derive(Clone, Copy)]
struct Timers {
    /// Every socket's keep-alive and timeout.
    keep_alive: Duration,
    timeout: Duration,
    /// How long a connection has to complete its handshake.
    half_open: Duration,
    /// How long a connection stays once its stream is gone.
    linger: Duration,
}

impl From<&TunOptions> for Timers {
    fn from(options: &TunOptions) -> Timers {
        Timers {
            keep_alive: options.keep_alive,
            timeout: options.timeout,
            half_open: options.half_open_timeout,
            linger: options.linger,
        }
    }
}

/// What a SYN that is being taken holds before a `Conn` has it. From the
/// claim of the places in the caps (`new_syn`) to the `Conn` of `open`, a
/// panic would leave them held by nothing the shard has a record of, and the
/// caps and the budget would never get them back. The shard writes them down
/// here before it calls the stack, and takes them off when it has given them
/// back or the `Conn` has them.
#[derive(Default)]
struct Held {
    /// The places in the two caps that `Admission::claim` took.
    places: bool,
    /// The share of the budget the socket's buffers took.
    sizes: Option<Sizes>,
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
    /// What the stack's initial sequence numbers are drawn from.
    seed: u64,
    /// When the next reap looks at every connection.
    next_sweep: Instant,
    /// The connections still half-open by the time they run out, soonest
    /// first: what `reap` aborts when the time comes.
    half_open: BTreeSet<(Instant, ConnId)>,
    /// The connections whose stream is gone, by the time their `linger` runs
    /// out, soonest first: what `reap` aborts when the time comes.
    lingering: BTreeSet<(Instant, ConnId)>,
    budget: Arc<Budget>,
    /// The caps and limits of the whole TUN, which a new SYN meets.
    admission: Arc<Admission>,
    timers: Timers,
    /// What the SYN being taken holds, if one is.
    held: Held,
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
    /// While the stream is gone: the time its `linger` runs out, when the
    /// connection is aborted if it is still here. `Some` from the drop of the
    /// stream until the connection goes.
    pub(crate) dropped: Option<Instant>,
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
        timers: Timers,
    ) -> ShardState {
        let mut device = ShardDevice {
            rx: None,
            tx: Vec::new(),
            mtu,
            edges: Edges::default(),
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
            seed,
            next_sweep: epoch + SWEEP,
            half_open: BTreeSet::new(),
            lingering: BTreeSet::new(),
            budget,
            admission,
            timers,
            held: Held::default(),
        }
    }

    /// Takes the place of the stack with a new one made with `seed`, after a
    /// panic in it. Every connection is reset (its stream answers
    /// `ConnectionReset` from then on, and whoever waits on it wakes), what
    /// each held in the caps and the budget is given back, and so is what a
    /// SYN being taken held; all of it from the shard's own records. Nothing
    /// is asked of the old `Interface` and `SocketSet`, which the panic
    /// stopped in the middle of something: the old state is dropped as it
    /// is, whole, so a field added to it is renewed with the rest. The app's
    /// later segments of the connections are strays to the new stack, which
    /// answers them with a reset.
    fn rebuild(&mut self, seed: u64) {
        for (_, conn) in self.conns.drain() {
            self.budget.give_back(conn.sizes);
            self.admission.release_socket();
            if conn.half_open.is_some() {
                self.admission.release_half_open();
            }
            conn.shared.end(Outcome::Reset);
        }
        self.release_held();
        *self = ShardState::new(
            self.device.mtu,
            self.epoch,
            seed,
            self.budget.clone(),
            self.admission.clone(),
            self.timers,
        );
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

    /// The shard's last turn, taken when the TUN stops: every packet in
    /// `incoming` goes in, though an ordinary turn takes in at most
    /// `INGRESS_PER_WAKE` (nothing waits behind this one), and then every socket
    /// is aborted and the stack is advanced once, so that each sends the reset it
    /// owes its app. What the stack sent comes back, to be written. The
    /// connections are not let go: the caller does that (`end_all`) when it has
    /// handed this over. No connection is made: the TUN is stopping, so a SYN is
    /// turned away.
    pub(crate) fn last_turn(&mut self, now: Instant, incoming: &mut Vec<Vec<u8>>) -> Vec<Vec<u8>> {
        let at = self.at(now);
        let mut opened = Vec::new();
        for packet in incoming.drain(..) {
            self.ingest(packet, now, at, &mut opened);
        }
        debug_assert!(opened.is_empty(), "a stopping TUN makes no connection");
        for conn in self.conns.values() {
            self.sockets.get_mut::<tcp::Socket>(conn.handle).abort();
        }
        self.iface
            .poll_egress(at, &mut self.device, &mut self.sockets);
        std::mem::take(&mut self.device.tx)
    }

    /// One packet into the stack. A segment of a connection goes to its
    /// socket; a SYN of none is looked at (`new_syn`); any other segment
    /// goes in too, and smoltcp answers it with a reset.
    fn ingest(
        &mut self,
        mut packet: Vec<u8>,
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
        // every SYN the stack takes, a new one and one sent again alike: with
        // no SACK offered the stack never makes a SACK block (design 5.3, H3)
        if segment.syn {
            packet::strip_sack_permitted(&mut packet);
        }
        if let Some(&id) = self.by_tuple.get(&segment.tuple) {
            let handle = self
                .conns
                .get(&id)
                .expect("a connection of the tuple table")
                .handle;
            // Data past the edge of the window the stack last told of, with
            // any room in the buffer, however long the segment is: the stack
            // takes in the part that fits (smoltcp tcp.rs 1598-1599,
            // 1660-1672), and its sums on the window then fail and panic
            // (693-701; design 5.3, H2). It is any room, not room for the
            // segment: the part of a longer one that fits would still take the
            // end of the data it holds past the edge it told of. With the
            // buffer full it takes in nothing of the segment (1636-1641) and
            // answers it with an ACK (1433-1446), which is what a sender
            // probing a closed window is kept probing by (RFC 9293 3.8.6.1):
            // that segment goes in.
            if self.device.edges.beyond(&segment) && self.has_room(handle) {
                let (src, dst) = (segment.tuple.src.ip(), segment.tuple.dst.ip());
                self.admission.count(Reason::BeyondWindow, src, dst);
                return;
            }
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

    /// Whether the receive buffer of the socket has room for more.
    fn has_room(&self, handle: SocketHandle) -> bool {
        let socket = self.sockets.get::<tcp::Socket>(handle);
        socket.recv_queue() < socket.recv_capacity()
    }

    fn advance(&mut self, packet: Vec<u8>, at: smoltcp::time::Instant) {
        #[cfg(test)]
        if carries_panic_marker(&packet) {
            panic!("rurge-tun test: panic while a segment is advanced");
        }
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
        self.held.places = true;
        match space::place(segment.tuple.dst.ip()) {
            Place::Real => self.open(packet, segment, now, at, opened),
            // the responder's port 53 is a place for a connection of its own
            // (M3c); every other port of it is not
            Place::Responder | Place::Own => {
                self.release_held();
                self.turn_away(&packet, &segment, now, None);
            }
            // there is no pool of fake addresses yet: each names nothing
            Place::Fake => {
                self.release_held();
                self.turn_away(&packet, &segment, now, Some(Reason::UnknownFake));
            }
        }
    }

    /// Gives back what the SYN being taken holds (`Held`): no connection
    /// came of it.
    fn release_held(&mut self) {
        if let Some(sizes) = self.held.sizes.take() {
            self.budget.give_back(sizes);
        }
        if std::mem::take(&mut self.held.places) {
            self.admission.unclaim();
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
    /// connection takes in the TUN's caps were claimed by `new_syn`, and the
    /// buffers' share of the budget is taken here; both are written down in
    /// `held` before the stack is called, and given back if no connection
    /// comes of the SYN, or if the stack panics.
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
        self.held.sizes = Some(sizes);
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; sizes.rx]),
            tcp::SocketBuffer::new(vec![0; sizes.tx]),
        );
        socket.set_nagle_enabled(false);
        // smoltcp resets a connection that has had nothing from the app for
        // `timeout`, whether or not anything is unacknowledged: the
        // keep-alives have a live app answer, so only a silent one is cut off
        socket.set_keep_alive(Some(self.timers.keep_alive.into()));
        socket.set_timeout(Some(self.timers.timeout.into()));
        socket.set_congestion_control(tcp::CongestionControl::Reno);
        let endpoint = IpListenEndpoint {
            addr: Some(tuple.dst.ip().into()),
            port: tuple.dst.port(),
        };
        if socket.listen(endpoint).is_err() {
            self.release_held();
            return;
        }
        let handle = self.sockets.add(socket);
        self.advance(packet, at);
        if self.sockets.get::<tcp::Socket>(handle).state() != tcp::State::SynReceived {
            self.sockets.remove(handle);
            self.release_held();
            return;
        }
        let id = ConnId::next();
        let shared = Arc::new(ConnShared::default());
        let deadline = ends_at(now, self.timers.half_open);
        let conn = Conn {
            handle,
            tuple,
            sizes,
            shared: shared.clone(),
            closed_by: None,
            time_wait: None,
            half_open: Some(deadline),
            shut: false,
            dropped: None,
        };
        self.conns.insert(id, conn);
        self.by_tuple.insert(tuple, id);
        self.half_open.insert((deadline, id));
        // the connection has them from here: it gives them back when it goes
        self.held = Held::default();
        opened.push(Opened { id, shared, tuple });
    }

    /// Lets go of the connections that are over, of those touched since the
    /// last reap, or of all of them once a `SWEEP`: a socket in TIME-WAIT
    /// `TIME_WAIT_GRACE` after it was first seen there, a closed socket once
    /// it has no ends left (an aborted socket keeps them until its reset is
    /// out). Neither goes while its stream is there to read what the app
    /// sent before its FIN (P12), unless the app reset the connection. First
    /// the connections that ran out of time for their handshake are aborted
    /// (`expire_half_open`), and those whose stream has been gone for `linger`
    /// (`expire_lingering`). When a connection kept for its grace, a handshake
    /// that is still going on or a stream that is gone is to be looked at
    /// next, if any is.
    fn reap(&mut self, now: Instant) -> Option<Instant> {
        let mut next = [self.expire_half_open(now), self.expire_lingering(now)]
            .into_iter()
            .flatten()
            .min();
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
            let unread = conn.dropped.is_none()
                && conn.closed_by != Some(Outcome::Reset)
                && socket.recv_queue() > 0;
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
            // The entry goes whether or not a connection is there for it:
            // `end_half_open` takes it from a connection that is, with the
            // connection's count. One always is (`remove` takes its entry
            // first), but an entry with none is no reason to panic with the
            // shard's lock held.
            self.half_open.pop_first();
            self.end_half_open(id);
            let Some(conn) = self.conns.get(&id) else {
                continue;
            };
            self.sockets.get_mut::<tcp::Socket>(conn.handle).abort();
            self.touched.insert(id);
        }
        None
    }

    /// Aborts the connections whose stream has been gone for `linger`: what
    /// they were waiting for (the app's acknowledgement of a FIN, say) did not
    /// come. As with a handshake that ran out of time, the socket sends its
    /// reset in a later egress pass and `reap` lets the connection go once that
    /// is out and not before. When the soonest of the streams that are gone
    /// runs out its time, if any waits.
    fn expire_lingering(&mut self, now: Instant) -> Option<Instant> {
        while let Some(&(deadline, id)) = self.lingering.first() {
            if deadline > now {
                return Some(deadline);
            }
            self.lingering.pop_first();
            // A connection is always there (`remove` takes its entry with it),
            // but an entry with none is no reason to panic with the shard's
            // lock held.
            let Some(conn) = self.conns.get(&id) else {
                continue;
            };
            self.sockets.get_mut::<tcp::Socket>(conn.handle).abort();
            self.touched.insert(id);
        }
        None
    }

    /// The stream of connection `id` was dropped, at `now`. The connection is
    /// reset unless it is ending well: the stream shut its own side down, the
    /// app can send nothing more, and nothing the app sent is left unread.
    /// (smoltcp's `may_recv` is true while the receive buffer holds a byte, so
    /// the `recv_queue` test says the last of these a second time.) A
    /// connection that is ending well has its FIN on its way and waits for the
    /// app to acknowledge it. Either way nobody reads the connection from now
    /// on, so it stays for `linger` at the most. A stream is dropped once; a
    /// second call for one connection does nothing.
    pub(crate) fn stream_dropped(&mut self, id: ConnId, now: Instant) {
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        // A second deadline would put a second entry in `lingering`, of which
        // `remove` takes the one that `dropped` names: the other would stay
        // behind when the connection went.
        if conn.dropped.is_some() {
            return;
        }
        let deadline = ends_at(now, self.timers.linger);
        conn.dropped = Some(deadline);
        self.lingering.insert((deadline, id));
        let socket = self.sockets.get_mut::<tcp::Socket>(conn.handle);
        if !conn.shut || socket.may_recv() || socket.recv_queue() > 0 {
            socket.abort();
        }
        self.touched.insert(id);
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
    /// The socket goes last, the one call into the stack, after the record of
    /// the connection is gone and what it held is given back: were that call
    /// to panic, the rebuild would not find the connection to give it back.
    fn remove(&mut self, id: ConnId, outcome: Outcome) {
        self.end_half_open(id);
        let Some(conn) = self.conns.remove(&id) else {
            return;
        };
        if let Some(deadline) = conn.dropped {
            self.lingering.remove(&(deadline, id));
        }
        // the tuple's entries go with the connection that has the tuple, if it
        // is this one
        if self.by_tuple.get(&conn.tuple) == Some(&id) {
            self.by_tuple.remove(&conn.tuple);
            self.device.edges.forget(&conn.tuple);
        }
        self.touched.remove(&id);
        self.budget.give_back(conn.sizes);
        self.admission.release_socket();
        conn.shared.end(outcome);
        self.sockets.remove(conn.handle);
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

    /// The connections the window's guard knows an edge of.
    #[cfg(test)]
    pub(crate) fn edge_count(&self) -> usize {
        self.device.edges.len()
    }

    /// What the shard keeps of its connections, counted.
    #[cfg(test)]
    pub(crate) fn records(&self) -> Records {
        Records {
            conns: self.conns.len(),
            by_tuple: self.by_tuple.len(),
            edges: self.device.edges.len(),
            touched: self.touched.len(),
            half_open: self.half_open.len(),
            lingering: self.lingering.len(),
        }
    }

    /// The bytes waiting in the send buffers of the shard's sockets, and the
    /// room those buffers have in all.
    #[cfg(test)]
    pub(crate) fn send_buffers(&self) -> (usize, usize) {
        use smoltcp::socket::AnySocket;
        self.sockets
            .iter()
            .filter_map(|(_, socket)| tcp::Socket::downcast(socket))
            .fold((0, 0), |(queued, room), socket| {
                (queued + socket.send_queue(), room + socket.send_capacity())
            })
    }
}

/// What a shard keeps of its connections besides their sockets: the two
/// tables, the guard's edges, the set of those a turn or a stream touched, and
/// the sets of those still half-open and of those whose stream is gone. Every
/// count is 0 when no connection is left behind.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Records {
    pub(crate) conns: usize,
    pub(crate) by_tuple: usize,
    pub(crate) edges: usize,
    pub(crate) touched: usize,
    pub(crate) half_open: usize,
    pub(crate) lingering: usize,
}

#[cfg(test)]
impl std::ops::Add for Records {
    type Output = Records;

    fn add(self, other: Records) -> Records {
        Records {
            conns: self.conns + other.conns,
            by_tuple: self.by_tuple + other.by_tuple,
            edges: self.edges + other.edges,
            touched: self.touched + other.touched,
            half_open: self.half_open + other.half_open,
            lingering: self.lingering + other.lingering,
        }
    }
}

/// `after` from `now`; as far off as can be told (`FAR_OFF`) if that is too
/// much to add to a time, which `TunOptions`, being public, can ask of it.
fn ends_at(now: Instant, after: Duration) -> Instant {
    now.checked_add(after)
        .or_else(|| now.checked_add(FAR_OFF))
        .unwrap_or(now)
}

/// Whether the TCP payload of `packet`, a segment that passed the filter,
/// begins with [`PANIC_MARKER`].
#[cfg(test)]
fn carries_panic_marker(packet: &[u8]) -> bool {
    packet::segment(packet).is_some_and(|segment| {
        packet[packet.len() - segment.payload_len..].starts_with(PANIC_MARKER)
    })
}

/// The task of a shard. It waits for packets, a kick, its timer or the stop;
/// takes a turn in the lock; then, out of it, starts the sessions of the new
/// connections and hands what the stack sent to the writer, waiting while
/// the writer's queue is full. After a turn that reached a bound it goes on
/// at once, after a yield. At the stop it takes a last turn (`last_turn`) and
/// lets go of every connection.
///
/// The turn is where smoltcp runs, and it runs where a panic is caught
/// (design 5.3, "the last line"): the shard is rebuilt, and the task goes on.
/// No hook is set for the panic; the default one prints it, which is as much
/// as is wanted for what is not to happen. The packets the turn was taking in
/// are lost with the connections they were for (a SYN comes again, a segment
/// of a connection that is gone gets a reset); any it had not begun to take in
/// are there for the next turn.
pub(crate) async fn run(
    shard: Arc<Shard>,
    mut packets: mpsc::Receiver<Vec<u8>>,
    writer: mpsc::Sender<Vec<Vec<u8>>>,
    dialer: Arc<dyn Dialer>,
    holds: Holds,
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
        // the closure holds what is not unwind-safe, the stack and `incoming`,
        // and the shard is rebuilt below, which leaves none of it as it was
        let turn = catch_unwind(AssertUnwindSafe(|| {
            shard.state().turn(Instant::now(), &mut incoming)
        }));
        let turn = match turn {
            Ok(turn) => turn,
            Err(panic) => {
                // the turn's hold of the lock ended as the panic unwound,
                // which poisoned it: taking it rebuilds the shard (but for a
                // stream that got to it first, and did)
                drop(shard.lock(Cause::Panic(panic_text(&*panic))));
                drop(panic);
                more = !incoming.is_empty();
                wake_at = None;
                continue;
            }
        };
        for opened in turn.opened {
            let stream = TunStream::new(shard.clone(), opened.id, opened.shared);
            tasks.spawn(session::serve(
                stream,
                dialer.clone(),
                opened.tuple,
                holds,
                life.clone(),
            ));
        }
        // The stop does not give this send up: a reset made for a SYN in a
        // turn that was under way when the stop came would be lost with it.
        // The send fails only once the writer has ended, which it does at the
        // latest a grace after the stop, so this is no wait without an end.
        if !turn.out.is_empty() && writer.send(turn.out).await.is_err() {
            // the writer has ended: so has the TUN
            break;
        }
        more = turn.more;
        wake_at = turn.wake_at;
    }
    // The stop takes one last turn, unless the writer has ended (the device
    // failed, or it ran out of patience with a device that takes nothing): the
    // resets would have no one to write them.
    if !writer.is_closed() {
        last_turn(&shard, &mut packets, &mut incoming, &writer).await;
    }
    shard.state().end_all();
}

/// The shard's last turn, at the stop (design 5.2, step 1): nothing more is
/// queued for the shard from here on, and what is queued goes in, a SYN
/// among it turned away with a reset since the TUN is stopping. Then every
/// socket is aborted and one egress pass sends the reset each one owes its app.
/// What the stack sent is handed to the writer, waiting for room in its queue
/// as long as the writer is there to make it; the stop does not give the send
/// up. The turn is taken where a panic is caught, like any turn: the shard is
/// rebuilt, which resets its connections too.
async fn last_turn(
    shard: &Shard,
    packets: &mut mpsc::Receiver<Vec<u8>>,
    incoming: &mut Vec<Vec<u8>>,
    writer: &mpsc::Sender<Vec<Vec<u8>>>,
) {
    packets.close();
    while let Ok(packet) = packets.try_recv() {
        incoming.push(packet);
    }
    let out = catch_unwind(AssertUnwindSafe(|| {
        shard.state().last_turn(Instant::now(), incoming)
    }));
    match out {
        Ok(out) => {
            // fails only once the writer has ended: nothing is left to do
            if !out.is_empty() {
                let _ = writer.send(out).await;
            }
        }
        Err(panic) => {
            drop(shard.lock(Cause::Panic(panic_text(&*panic))));
            drop(panic);
        }
    }
}
