//! The tunnel's state, all of it behind one lock (phase 2 M4 design 6.1):
//! a smoltcp interface and its sockets, the packet queues between it and
//! the peers, and a boringtun `Tunn` for each peer. Nothing here does I/O or
//! waits: the caller hands in what the carriers received and sends what
//! comes out.

use crate::routes::Routes;
use crate::wire;
use boringtun::noise::errors::WireGuardError;
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use rurge_config::wireguard::WireGuardSection;
use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::socket::{AnySocket, tcp, udp};
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr};
use std::collections::VecDeque;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

/// The buffers of every TCP connection, each way.
const TCP_BUFFER: usize = 256 * 1024;
/// How long a connection whose stream is gone may take to close before it
/// is reset.
const LINGER: Duration = Duration::from_secs(30);
/// The local ports of the tunnel's connections: the dynamic range.
const PORTS: RangeInclusive<u16> = 49152..=65535;
/// Room for the largest UDP datagram and what boringtun adds to a message.
const SCRATCH: usize = 65536 + 32;

/// A message for the carrier of `peer`.
#[derive(Debug)]
pub struct Outgoing {
    pub peer: usize,
    pub datagram: Vec<u8>,
}

/// Why a connection is not opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The tunnel has no address of the destination's family.
    NoAddress(IpAddr),
    /// No peer's `allowed-ips` covers the destination.
    NoRoute(IpAddr),
    /// No connection can go there (`0.0.0.0`).
    Unaddressable(IpAddr),
    /// Every local port is in use.
    NoPort,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NoAddress(IpAddr::V4(_)) => {
                f.write_str("wireguard: the tunnel has no IPv4 address")
            }
            Refusal::NoAddress(IpAddr::V6(_)) => {
                f.write_str("wireguard: the tunnel has no IPv6 address")
            }
            Refusal::NoRoute(ip) => write!(f, "wireguard: no peer's allowed-ips covers {ip}"),
            Refusal::Unaddressable(ip) => write!(f, "wireguard: {ip} cannot be connected to"),
            Refusal::NoPort => f.write_str("wireguard: every local port of the tunnel is in use"),
        }
    }
}

struct Peer {
    tunnel: Tunn,
    client_id: Option<[u8; 3]>,
    /// When a handshake rurge started with it last completed.
    handshake: Option<Instant>,
}

pub struct Stack {
    iface: Interface,
    sockets: SocketSet<'static>,
    queues: Queues,
    peers: Vec<Peer>,
    routes: Routes,
    v4: Option<Ipv4Addr>,
    v6: Option<Ipv6Addr>,
    scratch: Vec<u8>,
    next_port: u16,
    /// Connections whose stream is gone, and since when.
    released: Vec<(SocketHandle, Instant)>,
    /// What smoltcp's clock counts from.
    epoch: Instant,
}

/// A random number; a fixed one if the system has none to give (it only
/// spreads the handshake indices and the TCP sequence numbers).
fn random_u64() -> u64 {
    getrandom::u64().unwrap_or(0x5eed)
}

/// `message` with `client_id` in it, for the carrier of `peer`.
fn outgoing(peer: usize, message: &mut [u8], client_id: Option<[u8; 3]>) -> Outgoing {
    wire::mark(message, client_id);
    Outgoing {
        peer,
        datagram: message.to_vec(),
    }
}

impl Stack {
    pub fn new(section: &WireGuardSection) -> Stack {
        let mut queues = Queues::new(usize::from(section.mtu));
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = random_u64();
        let mut iface = Interface::new(config, &mut queues, smoltcp::time::Instant::ZERO);
        iface.update_ip_addrs(|addrs| {
            if let Some(v4) = section.self_ip {
                let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(v4), 32));
            }
            if let Some(v6) = section.self_ip_v6 {
                let _ = addrs.push(IpCidr::new(IpAddress::Ipv6(v6), 128));
            }
        });
        // everything leaves through some peer: `advance` picks which one
        if let Some(v4) = section.self_ip {
            let _ = iface.routes_mut().add_default_ipv4_route(v4);
        }
        if let Some(v6) = section.self_ip_v6 {
            let _ = iface.routes_mut().add_default_ipv6_route(v6);
        }
        let private = StaticSecret::from(*section.private_key.expose());
        // boringtun uses 24 bits of it; apart per peer
        let base = random_u64() as u32;
        let peers = section
            .peers
            .iter()
            .enumerate()
            .map(|(i, p)| Peer {
                tunnel: Tunn::new(
                    private.clone(),
                    PublicKey::from(p.public_key),
                    p.preshared_key.as_ref().map(|k| *k.expose()),
                    p.keepalive,
                    base.wrapping_add(i as u32) & 0x00ff_ffff,
                    None,
                ),
                client_id: p.client_id,
                handshake: None,
            })
            .collect();
        let span = PORTS.end() - PORTS.start();
        Stack {
            iface,
            sockets: SocketSet::new(Vec::new()),
            queues,
            peers,
            routes: Routes::new(section.peers.iter().map(|p| p.allowed_ips.as_slice())),
            v4: section.self_ip,
            v6: section.self_ip_v6,
            scratch: vec![0; SCRATCH],
            next_port: PORTS.start() + (random_u64() % u64::from(span)) as u16,
            released: Vec::new(),
            epoch: Instant::now(),
        }
    }

    fn at(&self, now: Instant) -> smoltcp::time::Instant {
        smoltcp::time::Instant::from_micros(
            now.saturating_duration_since(self.epoch).as_micros() as i64
        )
    }

    /// When a handshake rurge started last completed, with whichever peer.
    pub fn last_handshake(&self) -> Option<Instant> {
        self.peers.iter().filter_map(|p| p.handshake).max()
    }

    /// A handshake with every peer, whatever the state of its session: the
    /// tunnel starts, a test asks.
    pub fn initiate(&mut self, out: &mut Vec<Outgoing>) {
        for peer in 0..self.peers.len() {
            self.initiate_peer(peer, out);
        }
    }

    /// A handshake with `peer`, whatever the state of its session: it has a
    /// new carrier.
    pub fn initiate_peer(&mut self, peer: usize, out: &mut Vec<Outgoing>) {
        let Some(p) = self.peers.get_mut(peer) else {
            return;
        };
        if let TunnResult::WriteToNetwork(message) = p
            .tunnel
            .format_handshake_initiation(&mut self.scratch, true)
        {
            out.push(outgoing(peer, message, p.client_id));
        }
    }

    /// What the carrier of `peer` received; the answers it needs go to `out`.
    /// Whether it completed a handshake rurge started.
    pub fn receive(
        &mut self,
        peer: usize,
        datagram: &mut [u8],
        now: Instant,
        out: &mut Vec<Outgoing>,
    ) -> bool {
        let Some(p) = self.peers.get_mut(peer) else {
            return false;
        };
        wire::unmark(datagram);
        let response = wire::message_type(datagram) == Some(wire::HANDSHAKE_RESPONSE);
        let (src, packet) = match p.tunnel.decapsulate(None, datagram, &mut self.scratch) {
            TunnResult::WriteToNetwork(message) => {
                if response {
                    p.handshake = Some(now);
                }
                out.push(outgoing(peer, message, p.client_id));
                // what waited for the handshake
                while let TunnResult::WriteToNetwork(message) =
                    p.tunnel.decapsulate(None, &[], &mut self.scratch)
                {
                    out.push(outgoing(peer, message, p.client_id));
                }
                return response;
            }
            TunnResult::WriteToTunnelV4(packet, src) => (IpAddr::V4(src), packet.to_vec()),
            TunnResult::WriteToTunnelV6(packet, src) => (IpAddr::V6(src), packet.to_vec()),
            TunnResult::Done => return false,
            TunnResult::Err(e) => {
                tracing::trace!(peer, error = ?e, "wireguard: a message was dropped");
                return false;
            }
        };
        // a peer may send only from what routes to it (cryptokey routing)
        if self.routes.lookup(src) == Some(peer) {
            self.queues.rx.push_back(packet);
        } else {
            tracing::trace!(peer, %src, "wireguard: a packet from outside the peer's allowed-ips was dropped");
        }
        false
    }

    /// Runs the IP stack at `now`: what the peers sent reaches the sockets,
    /// what the sockets have to send goes out. How long until it wants to
    /// run again, when it has a deadline.
    pub fn advance(&mut self, now: Instant, out: &mut Vec<Outgoing>) -> Option<Duration> {
        let at = self.at(now);
        // a poll sends at most one segment of each connection: again, until
        // none has anything more to send now
        while self.iface.poll(at, &mut self.queues, &mut self.sockets)
            == PollResult::SocketStateChanged
        {}
        while let Some(packet) = self.queues.tx.pop_front() {
            self.send(&packet, out);
        }
        self.reap(now);
        self.iface.poll_delay(at, &self.sockets).map(Duration::from)
    }

    /// One packet of the stack to the peer its destination routes to.
    fn send(&mut self, packet: &[u8], out: &mut Vec<Outgoing>) {
        // `connect` refuses what no peer covers: what is left is a reply to
        // a packet that came from elsewhere
        let Some(peer) = Tunn::dst_address(packet).and_then(|dst| self.routes.lookup(dst)) else {
            return;
        };
        let p = &mut self.peers[peer];
        match p.tunnel.encapsulate(packet, &mut self.scratch) {
            TunnResult::WriteToNetwork(message) => out.push(outgoing(peer, message, p.client_id)),
            TunnResult::Err(e) => {
                tracing::trace!(peer, error = ?e, "wireguard: a packet was dropped");
            }
            // held until the handshake is done
            _ => {}
        }
    }

    /// WireGuard's timers (retries, keepalives, expiry); every quarter of a
    /// second or so. The peers whose session has run out, or whose
    /// handshake went unanswered for 90 seconds: the next packet to one
    /// starts another.
    pub fn tick(&mut self, out: &mut Vec<Outgoing>) -> Vec<usize> {
        let mut expired = Vec::new();
        for (i, p) in self.peers.iter_mut().enumerate() {
            match p.tunnel.update_timers(&mut self.scratch) {
                TunnResult::WriteToNetwork(message) => out.push(outgoing(i, message, p.client_id)),
                TunnResult::Err(WireGuardError::ConnectionExpired) => expired.push(i),
                TunnResult::Err(e) => {
                    tracing::trace!(peer = i, error = ?e, "wireguard: a timer failed");
                }
                _ => {}
            }
        }
        expired
    }

    /// The tunnel's address of `to`'s family, when some peer takes `to`.
    fn source_for(&self, to: IpAddr) -> Result<IpAddr, Refusal> {
        let local = match to {
            IpAddr::V4(_) => self.v4.map(IpAddr::V4),
            IpAddr::V6(_) => self.v6.map(IpAddr::V6),
        }
        .ok_or(Refusal::NoAddress(to))?;
        if self.routes.lookup(to).is_none() {
            return Err(Refusal::NoRoute(to));
        }
        Ok(local)
    }

    /// A TCP connection to `to`, from the tunnel's address of its family.
    pub fn connect(&mut self, to: SocketAddr) -> Result<SocketHandle, Refusal> {
        let local = self.source_for(to.ip())?;
        let port = self.free_port().ok_or(Refusal::NoPort)?;
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; TCP_BUFFER]),
            tcp::SocketBuffer::new(vec![0; TCP_BUFFER]),
        );
        // what the relay writes goes out as it is written, as on a real socket
        socket.set_nagle_enabled(false);
        // smoltcp 0.12's Cubic counts its window in bytes where RFC 8312
        // counts segments: it keeps the window at a segment or two
        socket.set_congestion_control(tcp::CongestionControl::Reno);
        socket
            .connect(self.iface.context(), to, SocketAddr::new(local, port))
            .map_err(|_| Refusal::Unaddressable(to.ip()))?;
        Ok(self.sockets.add(socket))
    }

    fn free_port(&mut self) -> Option<u16> {
        for _ in PORTS {
            let port = self.next_port;
            self.next_port = if port == *PORTS.end() {
                *PORTS.start()
            } else {
                port + 1
            };
            let taken = self.sockets.iter().any(|(_, s)| {
                let tcp = tcp::Socket::downcast(s)
                    .and_then(tcp::Socket::local_endpoint)
                    .map(|e| e.port);
                let udp = udp::Socket::downcast(s).map(|u| u.endpoint().port);
                tcp.or(udp) == Some(port)
            });
            if !taken {
                return Some(port);
            }
        }
        None
    }

    /// A UDP socket on the tunnel's address of `to`'s family, for an
    /// exchange with `to`.
    pub fn udp_open(&mut self, to: IpAddr) -> Result<SocketHandle, Refusal> {
        let local = self.source_for(to)?;
        let port = self.free_port().ok_or(Refusal::NoPort)?;
        let buffer = || udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 4], vec![0; 4096]);
        let mut socket = udp::Socket::new(buffer(), buffer());
        socket
            .bind(SocketAddr::new(local, port))
            .map_err(|_| Refusal::Unaddressable(to))?;
        Ok(self.sockets.add(socket))
    }

    /// The UDP socket of `handle`.
    pub fn udp(&mut self, handle: SocketHandle) -> &mut udp::Socket<'static> {
        self.sockets.get_mut(handle)
    }

    /// Forgets the UDP socket of `handle`.
    pub fn udp_close(&mut self, handle: SocketHandle) {
        self.sockets.remove(handle);
    }

    /// The TCP connection of `handle`.
    pub fn tcp(&mut self, handle: SocketHandle) -> &mut tcp::Socket<'static> {
        self.sockets.get_mut(handle)
    }

    /// The stream of `handle` is gone. A connection the other side is done
    /// with is closed, anything else reset; `advance` forgets it once it is
    /// over.
    pub fn release(&mut self, handle: SocketHandle, now: Instant) {
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        if socket.may_recv() || socket.recv_queue() > 0 {
            socket.abort();
        } else {
            socket.close();
        }
        self.released.push((handle, now));
    }

    /// Resets every TCP connection at once, without a word to the far end:
    /// the tunnel is ending. Whoever waits on one is woken.
    pub fn abort_all(&mut self) {
        for (_, socket) in self.sockets.iter_mut() {
            if let Some(tcp) = tcp::Socket::downcast_mut(socket) {
                tcp.abort();
            }
        }
    }

    fn reap(&mut self, now: Instant) {
        let sockets = &mut self.sockets;
        self.released.retain(|&(handle, since)| {
            let socket = sockets.get_mut::<tcp::Socket>(handle);
            let over = match socket.state() {
                tcp::State::TimeWait => true,
                // with no endpoint left, the reset went out
                tcp::State::Closed => socket.local_endpoint().is_none(),
                _ => false,
            };
            if over {
                sockets.remove(handle);
                return false;
            }
            if now.saturating_duration_since(since) >= LINGER {
                socket.abort();
            }
            true
        });
    }
}

/// The stack's device: packets from the peers on their way in, packets of
/// the stack on their way out.
pub(crate) struct Queues {
    pub(crate) rx: VecDeque<Vec<u8>>,
    pub(crate) tx: VecDeque<Vec<u8>>,
    mtu: usize,
}

impl Queues {
    pub(crate) fn new(mtu: usize) -> Queues {
        Queues {
            rx: VecDeque::new(),
            tx: VecDeque::new(),
            mtu,
        }
    }
}

pub(crate) struct RxToken(Vec<u8>);

pub(crate) struct TxToken<'a>(&'a mut VecDeque<Vec<u8>>);

impl phy::Device for Queues {
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
        let packet = self.rx.pop_front()?;
        Some((RxToken(packet), TxToken(&mut self.tx)))
    }

    fn transmit(&mut self, _timestamp: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        Some(TxToken(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
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
        self.0.push_back(packet);
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{PeerCore, PeerOpts, keypair, section};
    use std::net::Ipv4Addr;

    /// A client stack and its peers, every message delivered at once.
    struct Net {
        client: Stack,
        peers: Vec<PeerCore>,
        /// How many times the last `run` went round.
        rounds: usize,
        /// Handshakes the client saw complete.
        completed: usize,
    }

    impl Net {
        /// `allowed` is each peer's `allowed-ips`.
        fn new(peers: &[(PeerOpts, &[&str])], client_ids: &[Option<[u8; 3]>]) -> Net {
            let (private, public) = keypair();
            let cores: Vec<PeerCore> = peers
                .iter()
                .map(|(opts, _)| PeerCore::new(public, opts))
                .collect();
            let keys: Vec<([u8; 32], &[&str])> = cores
                .iter()
                .zip(peers)
                .map(|(core, (_, allowed))| (core.public_key(), *allowed))
                .collect();
            let mut section = section(private, Ipv4Addr::new(10, 9, 0, 2), &keys);
            for (peer, id) in section.peers.iter_mut().zip(client_ids) {
                peer.client_id = *id;
            }
            let mut net = Net {
                client: Stack::new(&section),
                peers: cores,
                rounds: 0,
                completed: 0,
            };
            let mut out = Vec::new();
            net.client.initiate(&mut out);
            net.run_with(out);
            net
        }

        fn run(&mut self) {
            self.run_with(Vec::new());
        }

        /// Until neither side has anything more to send.
        fn run_with(&mut self, mut to_peers: Vec<Outgoing>) {
            for round in 1..=500 {
                self.rounds = round;
                let now = Instant::now();
                self.client.advance(now, &mut to_peers);
                let mut to_client: Vec<(usize, Vec<u8>)> = Vec::new();
                for Outgoing { peer, mut datagram } in to_peers.drain(..) {
                    let mut back = Vec::new();
                    self.peers[peer].receive(&mut datagram, &mut back);
                    to_client.extend(back.into_iter().map(|d| (peer, d)));
                }
                for (i, core) in self.peers.iter_mut().enumerate() {
                    let mut back = Vec::new();
                    core.advance(&mut back);
                    to_client.extend(back.into_iter().map(|d| (i, d)));
                }
                if to_client.is_empty() {
                    return;
                }
                for (peer, mut datagram) in to_client {
                    if self.client.receive(peer, &mut datagram, now, &mut to_peers) {
                        self.completed += 1;
                    }
                }
            }
            panic!("the tunnel never went quiet");
        }

        fn connect(&mut self, to: &str) -> SocketHandle {
            let handle = self.client.connect(to.parse().unwrap()).unwrap();
            self.run();
            assert_eq!(self.client.tcp(handle).state(), tcp::State::Established);
            handle
        }

        fn echo(&mut self, handle: SocketHandle, data: &[u8]) -> Vec<u8> {
            self.client.tcp(handle).send_slice(data).unwrap();
            self.run();
            let mut buf = vec![0u8; data.len() + 16];
            let n = self.client.tcp(handle).recv_slice(&mut buf).unwrap();
            buf.truncate(n);
            buf
        }
    }

    fn one_peer(opts: PeerOpts) -> Net {
        Net::new(&[(opts, &["10.0.0.0/8"])], &[None])
    }

    #[test]
    fn a_connection_through_the_tunnel_echoes() {
        let mut net = one_peer(PeerOpts::default());
        assert_eq!(net.peers[0].handshakes, 1);
        assert_eq!(net.completed, 1);
        let handle = net.connect("10.0.0.1:7");
        assert_eq!(net.echo(handle, b"ping"), b"ping");
        assert!(net.client.peers[0].handshake.is_some());
        assert_eq!(net.completed, 1, "data completes no handshake");
    }

    /// A poll sends one segment of each connection: an advance sends all a
    /// connection may, and the window grows past a segment or two (Reno:
    /// smoltcp 0.12's Cubic keeps it there).
    #[test]
    fn a_window_of_data_leaves_at_once() {
        let mut net = one_peer(PeerOpts::default());
        let handle = net.connect("10.0.0.1:7");
        assert_eq!(
            net.client.tcp(handle).congestion_control(),
            tcp::CongestionControl::Reno
        );
        let data: Vec<u8> = (0..200 * 1024).map(|i| (i % 251) as u8).collect();
        assert!(net.echo(handle, &data) == data, "all of it came back");
        assert!(net.rounds < 20, "{} rounds", net.rounds);
    }

    /// Every message out carries the id; the peer's own, which it writes
    /// into its answers, is cleared before boringtun reads them (manual).
    #[test]
    fn the_client_id_goes_out_in_every_message_and_is_cleared_coming_in() {
        let id = [83, 12, 235];
        let mut net = Net::new(
            &[(
                PeerOpts {
                    client_id: Some(id),
                    ..PeerOpts::default()
                },
                &["10.0.0.0/8"],
            )],
            &[Some(id)],
        );
        let handle = net.connect("10.0.0.1:7");
        assert_eq!(net.echo(handle, b"ping"), b"ping");
        let reserved = &net.peers[0].reserved;
        assert!(reserved.len() >= 3, "{reserved:?}");
        assert!(reserved.iter().all(|r| *r == id), "{reserved:?}");
    }

    /// A peer that routes by the id ignores what comes without it.
    #[test]
    fn without_the_client_id_such_a_peer_never_answers() {
        let mut net = Net::new(
            &[(
                PeerOpts {
                    client_id: Some([83, 12, 235]),
                    ..PeerOpts::default()
                },
                &["10.0.0.0/8"],
            )],
            &[None],
        );
        assert_eq!(net.peers[0].handshakes, 0);
        let handle = net.client.connect("10.0.0.1:7".parse().unwrap()).unwrap();
        net.run();
        assert_eq!(net.client.tcp(handle).state(), tcp::State::SynSent);
    }

    #[test]
    fn the_longest_prefix_picks_the_peer() {
        let mut net = Net::new(
            &[
                (PeerOpts::default(), &["10.0.0.0/8"]),
                (PeerOpts::default(), &["10.2.0.0/16"]),
            ],
            &[None, None],
        );
        let wide = net.connect("10.1.0.1:7");
        let narrow = net.connect("10.2.0.1:7");
        assert_eq!(net.echo(wide, b"a"), b"a");
        assert_eq!(net.echo(narrow, b"b"), b"b");
        assert_eq!((net.peers[0].accepted, net.peers[1].accepted), (1, 1));
    }

    #[test]
    fn what_the_tunnel_cannot_reach_is_refused() {
        let mut net = one_peer(PeerOpts::default());
        let refused = |net: &mut Net, to: &str| {
            net.client
                .connect(to.parse().unwrap())
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            refused(&mut net, "192.0.2.1:80"),
            "wireguard: no peer's allowed-ips covers 192.0.2.1"
        );
        assert_eq!(
            refused(&mut net, "[2001:db8::1]:80"),
            "wireguard: the tunnel has no IPv6 address"
        );
        let mut all = Net::new(&[(PeerOpts::default(), &["0.0.0.0/0"])], &[None]);
        assert_eq!(
            refused(&mut all, "0.0.0.0:80"),
            "wireguard: 0.0.0.0 cannot be connected to"
        );
    }

    /// Cryptokey routing: what a peer sends from outside its own
    /// `allowed-ips` is dropped.
    #[test]
    fn a_peer_may_only_send_from_its_allowed_ips() {
        let mut net = Net::new(
            &[
                (PeerOpts::default(), &["10.1.0.0/16"]),
                (PeerOpts::default(), &["10.2.0.0/16"]),
            ],
            &[None, None],
        );
        let client = Ipv4Addr::new(10, 9, 0, 2);
        let mut out = Vec::new();
        net.peers[1].ping(Ipv4Addr::new(10, 1, 0, 5), client, 1, 32, None, &mut out);
        net.deliver(1, out);
        // had it been taken, the answer would have gone to the other peer
        assert!(net.peers[0].pongs.is_empty(), "{:?}", net.peers[0].pongs);
        assert!(net.peers[1].pongs.is_empty(), "{:?}", net.peers[1].pongs);
        let mut out = Vec::new();
        net.peers[1].ping(Ipv4Addr::new(10, 2, 0, 5), client, 2, 32, None, &mut out);
        net.deliver(1, out);
        assert_eq!(net.peers[1].pongs, [(2, 32)]);
    }

    /// The stack answers a ping to its own address, and nothing else.
    #[test]
    fn only_echo_requests_to_the_tunnel_address_are_answered() {
        let mut net = one_peer(PeerOpts::default());
        let mut out = Vec::new();
        let from = Ipv4Addr::new(10, 0, 0, 1);
        net.peers[0].ping(from, Ipv4Addr::new(10, 9, 0, 3), 1, 8, None, &mut out);
        net.peers[0].ping(from, Ipv4Addr::new(10, 9, 0, 2), 2, 8, None, &mut out);
        net.deliver(0, out);
        assert_eq!(net.peers[0].pongs, [(2, 8)]);
    }

    #[test]
    fn a_fragmented_packet_is_reassembled() {
        let mut net = one_peer(PeerOpts::default());
        let mut out = Vec::new();
        let (from, to) = (Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(10, 9, 0, 2));
        net.peers[0].ping(from, to, 7, 1000, Some(496), &mut out);
        assert_eq!(out.len(), 3, "the request went in three fragments");
        net.deliver(0, out);
        assert_eq!(net.peers[0].pongs, [(7, 1000)]);
    }

    /// The other side done too: closed and forgotten. Not done: reset.
    #[test]
    fn a_released_connection_is_closed_or_reset_and_forgotten() {
        let mut net = one_peer(PeerOpts::default());
        let done = net.connect("10.0.0.1:7");
        net.client.tcp(done).close();
        net.run();
        // the echo server closes after us; our side has read everything
        let mut buf = [0u8; 8];
        assert_eq!(
            net.client.tcp(done).recv_slice(&mut buf),
            Err(tcp::RecvError::Finished)
        );
        net.client.release(done, Instant::now());
        net.run();
        assert_eq!(net.client.sockets.iter().count(), 0);
        assert_eq!(net.peers[0].open(), 0);

        let busy = net.connect("10.0.0.1:7");
        assert_eq!(net.peers[0].open(), 1);
        net.client.release(busy, Instant::now());
        net.run();
        assert_eq!(net.client.sockets.iter().count(), 0);
        assert_eq!(net.peers[0].open(), 0, "the reset closed the peer's side");
        assert_eq!(net.peers[0].resets, 1);
    }

    impl Net {
        /// Messages `peer` sent on its own.
        fn deliver(&mut self, peer: usize, messages: Vec<Vec<u8>>) {
            let mut out = Vec::new();
            for mut message in messages {
                self.client
                    .receive(peer, &mut message, Instant::now(), &mut out);
            }
            self.run_with(out);
        }
    }
}
