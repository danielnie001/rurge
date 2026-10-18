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
    Budget, ConnId, ConnShared, EGRESS_PASSES, INGRESS_PER_WAKE, Outcome, SWEEP, TIME_WAIT_GRACE,
    TIMEOUT, TunStream, Tuple,
};
use crate::Life;
use crate::packet::{self, Kind};
use crate::session;
use rurge_inbound::Dialer;
use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{self, Checksum, DeviceCapabilities, Medium};
use smoltcp::socket::tcp;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpListenEndpoint};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use tokio::sync::{Notify, mpsc};
use tokio_util::task::TaskTracker;

/// The stack's own addresses. With `any_ip`, smoltcp takes a packet for an
/// address that is not its own only when the route to that address goes
/// through one of its own (phase 3 M3 design 4.2), so a default route of
/// each family goes through these.
const STACK_V4: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 253);
const STACK_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0, 0xfd);

pub(crate) struct Shard {
    state: Mutex<ShardState>,
    /// A stream did something the shard has to act on: it read (there is
    /// room in the window to tell of), wrote (there is data to send), shut
    /// its write side down, or went.
    kick: Notify,
}

impl Shard {
    pub(crate) fn new(mtu: usize, epoch: Instant, budget: Arc<Budget>) -> Shard {
        Shard {
            state: Mutex::new(ShardState::new(mtu, epoch, budget)),
            kick: Notify::new(),
        }
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, ShardState> {
        self.state.lock().expect("a TCP shard's state")
    }

    /// For a stream's `Drop`, which must not panic.
    pub(crate) fn try_state(&self) -> Option<MutexGuard<'_, ShardState>> {
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
    budget: Arc<Budget>,
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
    fn new(mtu: usize, epoch: Instant, budget: Arc<Budget>) -> ShardState {
        let mut device = ShardDevice {
            rx: None,
            tx: Vec::new(),
            mtu,
        };
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = getrandom::u64().unwrap_or(0x5eed);
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
            budget,
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
            self.ingest(packet, at, &mut opened);
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
    /// socket; a SYN of none opens one; any other segment goes in too, and
    /// smoltcp answers it with a reset.
    fn ingest(&mut self, packet: Vec<u8>, at: smoltcp::time::Instant, opened: &mut Vec<Opened>) {
        // nothing but TCP comes to a shard
        let Kind::Tcp(segment) = packet::kind(&packet) else {
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
        } else if segment.syn && !segment.ack {
            self.open(packet, segment, at, opened);
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

    /// A SYN of no connection (phase 3 M3 design 5.2, step 5): a socket that
    /// listens on the SYN's destination alone, then the SYN. The socket has
    /// the connection if the stack took the SYN; packets go in one at a
    /// time, so no other SYN to the same destination can come between.
    fn open(
        &mut self,
        packet: Vec<u8>,
        segment: packet::Segment,
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
        socket.set_keep_alive(None);
        socket.set_timeout(Some(TIMEOUT.into()));
        socket.set_congestion_control(tcp::CongestionControl::Reno);
        let endpoint = IpListenEndpoint {
            addr: Some(tuple.dst.ip().into()),
            port: tuple.dst.port(),
        };
        if socket.listen(endpoint).is_err() {
            self.budget.give_back(sizes);
            return;
        }
        let handle = self.sockets.add(socket);
        self.advance(packet, at);
        if self.sockets.get::<tcp::Socket>(handle).state() != tcp::State::SynReceived {
            self.sockets.remove(handle);
            self.budget.give_back(sizes);
            return;
        }
        let id = ConnId::next();
        let shared = Arc::new(ConnShared::default());
        let conn = Conn {
            handle,
            tuple,
            sizes,
            shared: shared.clone(),
            closed_by: None,
            time_wait: None,
            shut: false,
            dropped: false,
        };
        self.conns.insert(id, conn);
        self.by_tuple.insert(tuple, id);
        opened.push(Opened { id, shared, tuple });
    }

    /// Lets go of the connections that are over, of those touched since the
    /// last reap, or of all of them once a `SWEEP`: a socket in TIME-WAIT
    /// `TIME_WAIT_GRACE` after it was first seen there, a closed socket once
    /// it has no ends left (an aborted socket keeps them until its reset is
    /// out). Neither goes while its stream is there to read what the app
    /// sent before its FIN (P12), unless the app reset the connection. When
    /// a connection kept for its grace is to be looked at next, if any is.
    fn reap(&mut self, now: Instant) -> Option<Instant> {
        let ids: Vec<ConnId> = if now >= self.next_sweep {
            self.next_sweep = now + SWEEP;
            self.touched.clear();
            self.conns.keys().copied().collect()
        } else {
            self.touched.drain().collect()
        };
        let mut next: Option<Instant> = None;
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

    /// Lets go of a connection: its socket, its entries and its buffers; its
    /// stream answers `outcome` from now on, and whoever waits on it wakes.
    fn remove(&mut self, id: ConnId, outcome: Outcome) {
        let Some(conn) = self.conns.remove(&id) else {
            return;
        };
        if self.by_tuple.get(&conn.tuple) == Some(&id) {
            self.by_tuple.remove(&conn.tuple);
        }
        self.touched.remove(&id);
        self.sockets.remove(conn.handle);
        self.budget.give_back(conn.sizes);
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
