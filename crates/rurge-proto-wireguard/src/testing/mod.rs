//! A WireGuard peer for the tests of this crate and of its dependants
//! (feature `testing`). `PeerCore` is the peer without I/O: boringtun
//! answering the client's handshakes, and a smoltcp host of its own that
//! answers on every address routed to it — a TCP echo service on port 7.
//! `FakeWgPeer` puts one on a loopback UDP port.

mod peer;

pub use peer::FakeWgPeer;

use crate::stack::Queues;
use crate::wire;
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use rurge_config::HostName;
use rurge_config::spec::Secret;
use rurge_config::wireguard::{DEFAULT_MTU, PeerEndpoint, WireGuardPeer, WireGuardSection};
use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::socket::tcp;
use smoltcp::wire::{
    HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, IpProtocol, Ipv4Packet,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

/// The TCP echo service of a peer.
pub const ECHO_PORT: u16 = 7;
/// Connections the echo service takes at once: SYNs that arrive together.
const BACKLOG: usize = 8;

/// A fresh key pair: private, public.
pub fn keypair() -> ([u8; 32], [u8; 32]) {
    let mut private = [0u8; 32];
    getrandom::fill(&mut private).expect("randomness");
    let public = PublicKey::from(&StaticSecret::from(private)).to_bytes();
    (private, public)
}

/// A client's section with `peers`, each a public key and its
/// `allowed-ips`; the endpoints are placeholders, for a test to replace
/// where they matter (as any other field).
pub fn section(
    private: [u8; 32],
    self_ip: Ipv4Addr,
    peers: &[([u8; 32], &[&str])],
) -> WireGuardSection {
    WireGuardSection {
        name: "test".to_string(),
        private_key: Secret::new(private),
        self_ip: Some(self_ip),
        self_ip_v6: None,
        dns_servers: Vec::new(),
        prefer_ipv6: false,
        mtu: DEFAULT_MTU,
        peers: peers
            .iter()
            .enumerate()
            .map(|(i, (public_key, allowed))| WireGuardPeer {
                public_key: *public_key,
                allowed_ips: allowed.iter().map(|a| a.parse().unwrap()).collect(),
                endpoint: PeerEndpoint {
                    host: HostName::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                    port: 9 + i as u16,
                },
                preshared_key: None,
                keepalive: None,
                client_id: None,
            })
            .collect(),
    }
}

/// `addr` as a peer's `endpoint`.
pub fn endpoint(addr: SocketAddr) -> PeerEndpoint {
    PeerEndpoint {
        host: HostName::Ip(addr.ip()),
        port: addr.port(),
    }
}

/// How a test peer behaves.
#[derive(Clone, Debug)]
pub struct PeerOpts {
    /// Its own addresses in the tunnel; it answers on every address routed
    /// to it all the same.
    pub address: Ipv4Addr,
    pub address_v6: Ipv6Addr,
    /// Written into every message it sends; a message without it is
    /// ignored (WARP routes by it).
    pub client_id: Option<[u8; 3]>,
    pub preshared_key: Option<[u8; 32]>,
    /// The MTU of its own stack.
    pub mtu: usize,
}

impl Default for PeerOpts {
    fn default() -> PeerOpts {
        PeerOpts {
            address: Ipv4Addr::new(10, 0, 0, 1),
            address_v6: "fd00::1".parse().expect("an address"),
            client_id: None,
            preshared_key: None,
            mtu: 1420,
        }
    }
}

struct Conn {
    handle: SocketHandle,
    /// The client finished its side.
    fin: bool,
}

pub struct PeerCore {
    tunnel: Tunn,
    public_key: [u8; 32],
    iface: Interface,
    sockets: SocketSet<'static>,
    queues: Queues,
    listeners: Vec<SocketHandle>,
    conns: Vec<Conn>,
    scratch: Vec<u8>,
    epoch: Instant,
    client_id: Option<[u8; 3]>,
    /// Handshakes the client started.
    pub handshakes: usize,
    /// The reserved bytes of every message received.
    pub reserved: Vec<[u8; 3]>,
    /// Echo replies received: identifier and data length.
    pub pongs: Vec<(u16, usize)>,
    /// TCP connections accepted.
    pub accepted: usize,
    /// Where each accepted connection went: the address and port the
    /// client connected to.
    pub connected_to: Vec<SocketAddr>,
    /// TCP connections the client reset.
    pub resets: usize,
}

/// `message` with `client_id` in it.
fn marked(message: &mut [u8], client_id: Option<[u8; 3]>) -> Vec<u8> {
    wire::mark(message, client_id);
    message.to_vec()
}

impl PeerCore {
    /// A peer the client of `client_public` may reach.
    pub fn new(client_public: [u8; 32], opts: &PeerOpts) -> PeerCore {
        let (private, public_key) = keypair();
        let tunnel = Tunn::new(
            StaticSecret::from(private),
            PublicKey::from(client_public),
            opts.preshared_key,
            None,
            1,
            None,
        );
        let mut queues = Queues::new(opts.mtu);
        let mut iface = Interface::new(
            Config::new(HardwareAddress::Ip),
            &mut queues,
            smoltcp::time::Instant::ZERO,
        );
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(opts.address), 32));
            let _ = addrs.push(IpCidr::new(IpAddress::Ipv6(opts.address_v6), 128));
        });
        let _ = iface.routes_mut().add_default_ipv4_route(opts.address);
        let _ = iface.routes_mut().add_default_ipv6_route(opts.address_v6);
        // whatever the client sends through it is for it
        iface.set_any_ip(true);
        let mut sockets = SocketSet::new(Vec::new());
        let listeners = (0..BACKLOG)
            .map(|_| listen(&mut sockets, ECHO_PORT))
            .collect();
        PeerCore {
            tunnel,
            public_key,
            iface,
            sockets,
            queues,
            listeners,
            conns: Vec::new(),
            scratch: vec![0; 65536 + 32],
            epoch: Instant::now(),
            client_id: opts.client_id,
            handshakes: 0,
            reserved: Vec::new(),
            pongs: Vec::new(),
            accepted: 0,
            connected_to: Vec::new(),
            resets: 0,
        }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    /// Connections open now.
    pub fn open(&self) -> usize {
        self.conns.len()
    }

    /// A message from the client; the answers go to `out`.
    pub fn receive(&mut self, message: &mut [u8], out: &mut Vec<Vec<u8>>) {
        let Some(reserved) = message.get(1..4) else {
            return;
        };
        let reserved: [u8; 3] = reserved.try_into().expect("three bytes");
        self.reserved.push(reserved);
        if self.client_id.is_some_and(|id| id != reserved) {
            return;
        }
        wire::unmark(message);
        let initiation = wire::message_type(message) == Some(wire::HANDSHAKE_INITIATION);
        let packet = match self.tunnel.decapsulate(None, message, &mut self.scratch) {
            TunnResult::WriteToNetwork(answer) => {
                if initiation {
                    self.handshakes += 1;
                }
                out.push(marked(answer, self.client_id));
                while let TunnResult::WriteToNetwork(more) =
                    self.tunnel.decapsulate(None, &[], &mut self.scratch)
                {
                    out.push(marked(more, self.client_id));
                }
                return;
            }
            TunnResult::WriteToTunnelV4(packet, _) | TunnResult::WriteToTunnelV6(packet, _) => {
                packet.to_vec()
            }
            _ => return,
        };
        if !self.pong(&packet) {
            self.queues.rx.push_back(packet);
        }
    }

    /// An echo reply: noted, not handed to the stack (it would drop it).
    fn pong(&mut self, packet: &[u8]) -> bool {
        let Ok(ip) = Ipv4Packet::new_checked(packet) else {
            return false;
        };
        if ip.next_header() != IpProtocol::Icmp {
            return false;
        }
        let Ok(icmp) = Icmpv4Packet::new_checked(ip.payload()) else {
            return false;
        };
        match Icmpv4Repr::parse(&icmp, &ChecksumCapabilities::default()) {
            Ok(Icmpv4Repr::EchoReply { ident, data, .. }) => {
                self.pongs.push((ident, data.len()));
                true
            }
            _ => false,
        }
    }

    /// Runs its stack and its services; what it sends goes to `out`. How
    /// long until the stack wants to run again, when it has a deadline.
    pub fn advance(&mut self, out: &mut Vec<Vec<u8>>) -> Option<Duration> {
        let at = smoltcp::time::Instant::from_micros(self.epoch.elapsed().as_micros() as i64);
        self.iface.poll(at, &mut self.queues, &mut self.sockets);
        self.serve();
        while self.iface.poll(at, &mut self.queues, &mut self.sockets)
            == PollResult::SocketStateChanged
        {
            self.serve();
        }
        while let Some(packet) = self.queues.tx.pop_front() {
            self.inject(&packet, out);
        }
        self.iface.poll_delay(at, &self.sockets).map(Duration::from)
    }

    /// WireGuard's timers.
    pub fn tick(&mut self, out: &mut Vec<Vec<u8>>) {
        if let TunnResult::WriteToNetwork(message) = self.tunnel.update_timers(&mut self.scratch) {
            out.push(marked(message, self.client_id));
        }
    }

    /// Sends the IP packet `packet` into the tunnel as it is.
    pub fn inject(&mut self, packet: &[u8], out: &mut Vec<Vec<u8>>) {
        if let TunnResult::WriteToNetwork(message) =
            self.tunnel.encapsulate(packet, &mut self.scratch)
        {
            out.push(marked(message, self.client_id));
        }
    }

    /// An echo request with `size` bytes of data from `from` to `to`, in
    /// fragments of `fragment` bytes (a multiple of 8) when given.
    pub fn ping(
        &mut self,
        from: Ipv4Addr,
        to: Ipv4Addr,
        ident: u16,
        size: usize,
        fragment: Option<usize>,
        out: &mut Vec<Vec<u8>>,
    ) {
        let data = vec![0x5a; size];
        let request = Icmpv4Repr::EchoRequest {
            ident,
            seq_no: 1,
            data: &data,
        };
        let mut payload = vec![0; request.buffer_len()];
        request.emit(
            &mut Icmpv4Packet::new_unchecked(&mut payload),
            &ChecksumCapabilities::default(),
        );
        let step = fragment.unwrap_or(payload.len());
        let pieces: Vec<&[u8]> = payload.chunks(step).collect();
        for (k, piece) in pieces.iter().enumerate() {
            let mut packet = vec![0u8; 20 + piece.len()];
            let mut ip = Ipv4Packet::new_unchecked(&mut packet);
            ip.set_version(4);
            ip.set_header_len(20);
            ip.set_total_len((20 + piece.len()) as u16);
            ip.set_ident(ident);
            ip.clear_flags();
            ip.set_more_frags(k + 1 < pieces.len());
            ip.set_frag_offset((k * step) as u16);
            ip.set_hop_limit(64);
            ip.set_next_header(IpProtocol::Icmp);
            ip.set_src_addr(from);
            ip.set_dst_addr(to);
            ip.payload_mut().copy_from_slice(piece);
            ip.fill_checksum();
            self.inject(&packet, out);
        }
    }

    /// Echoes what each connection receives; a listener that took a
    /// connection is replaced, a closed connection forgotten.
    fn serve(&mut self) {
        for k in 0..self.listeners.len() {
            let handle = self.listeners[k];
            let listener = self.sockets.get::<tcp::Socket>(handle);
            if listener.state() == tcp::State::Listen {
                continue;
            }
            if let Some(local) = listener.local_endpoint() {
                self.connected_to
                    .push(SocketAddr::new(local.addr.into(), local.port));
            }
            self.accepted += 1;
            self.conns.push(Conn { handle, fin: false });
            self.listeners[k] = listen(&mut self.sockets, ECHO_PORT);
        }
        let sockets = &mut self.sockets;
        let mut resets = 0;
        self.conns.retain_mut(|conn| {
            let socket = sockets.get_mut::<tcp::Socket>(conn.handle);
            let room = socket.send_capacity() - socket.send_queue();
            let mut buf = vec![0u8; room.min(socket.recv_queue())];
            if let Ok(n) = socket.recv_slice(&mut buf) {
                let _ = socket.send_slice(&buf[..n]);
            }
            conn.fin |= matches!(
                socket.state(),
                tcp::State::CloseWait
                    | tcp::State::LastAck
                    | tcp::State::Closing
                    | tcp::State::TimeWait
            );
            // the client is done and everything went back: done too
            if conn.fin && !socket.may_recv() && socket.send_queue() == 0 {
                socket.close();
            }
            match socket.state() {
                tcp::State::Closed | tcp::State::TimeWait => {
                    if !conn.fin {
                        resets += 1;
                    }
                    sockets.remove(conn.handle);
                    false
                }
                _ => true,
            }
        });
        self.resets += resets;
    }
}

fn listen(sockets: &mut SocketSet<'static>, port: u16) -> SocketHandle {
    let mut socket = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0; 256 * 1024]),
        tcp::SocketBuffer::new(vec![0; 256 * 1024]),
    );
    socket.listen(port).expect("a port to listen on");
    sockets.add(socket)
}
