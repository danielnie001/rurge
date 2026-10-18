//! A TCP client that is a table and nothing else, for the benchmark of ten
//! thousand idle connections (phase 3 M3 design 6): a connection of it costs
//! a few dozen bytes where a connection of the app's stack costs its buffers.
//!
//! It opens connections to the adapter from IPv4 `198.18.0.1`, sends a
//! hello on each, and acknowledges whatever the adapter sends back. It is
//! driven by its two calls and by nothing else: a packet is handled only
//! while `open` or `take` runs. Sending is `AppStack::inject`, receiving is
//! `AppStack::divert`, so the app's stack never sees these connections.
//!
//! What it does is what a kernel does with the same segments, to the extent
//! the adapter's side can tell: a segment that is not new (sent again, or a
//! keep-alive probe a byte below the next one expected) is answered with an
//! ACK of where the stream is, and what has been taken is how far that went,
//! never the sum of the payloads that came.

use super::app::AppStack;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    IpAddress, IpProtocol, Ipv4Packet, Ipv4Repr, TcpControl, TcpPacket, TcpRepr, TcpSeqNumber,
};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::ops::RangeInclusive;
use std::time::Duration;
use tokio::sync::mpsc;

/// The adapter's own address, which the app's stack has.
const SOURCE: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 1);
/// Advertised on every segment: nothing it takes is kept, so the window never shrinks.
const WINDOW: u16 = 65_535;
const MSS: u16 = 1460;
/// How long a call waits for the next packet before it gives up.
const IDLE: Duration = Duration::from_secs(10);
/// The most packets handled before the ACKs they call for are sent.
const BATCH: usize = 256;

/// `a` is after `b`, in the space of sequence numbers.
fn after(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

pub struct ScriptedTcp {
    app: AppStack,
    packets: mpsc::UnboundedReceiver<Vec<u8>>,
    /// The ports the app's stack hands over: only these can be opened.
    ports: RangeInclusive<u16>,
    conns: HashMap<u16, Conn>,
}

enum Phase {
    SynSent,
    Established,
}

struct Conn {
    dst: SocketAddrV4,
    phase: Phase,
    /// The sequence number of our SYN.
    isn: u32,
    /// The sequence number after the hello: what we send next.
    sent: u32,
    hello: usize,
    /// The hello has been acknowledged.
    acked: bool,
    /// The sequence number of the adapter's SYN.
    peer_isn: u32,
    /// The next sequence number expected from the adapter.
    expected: u32,
    /// A segment came that takes sequence space: an ACK of `expected` is owed.
    ack_due: bool,
}

impl Conn {
    fn settled(&self) -> bool {
        matches!(self.phase, Phase::Established) && self.acked
    }

    /// The bytes of the stream from the adapter that have arrived in order.
    fn taken(&self) -> usize {
        match self.phase {
            Phase::SynSent => 0,
            Phase::Established => {
                self.expected.wrapping_sub(self.peer_isn).wrapping_sub(1) as usize
            }
        }
    }
}

impl ScriptedTcp {
    /// A client for the connections from `ports`, which the app's stack hands
    /// to it from now on.
    pub fn new(app: &AppStack, ports: RangeInclusive<u16>) -> ScriptedTcp {
        ScriptedTcp {
            app: app.clone(),
            packets: app.divert(ports.clone()),
            ports,
            conns: HashMap::new(),
        }
    }

    /// A SYN (MSS 1460, no other option) from each port to `dst`; on its
    /// SYN-ACK an ACK that carries `hello` bytes; resolves once every
    /// connection is established and its bytes are acknowledged. Gives up
    /// when nothing comes for ten seconds, and on a reset. Dropped while it
    /// waits it loses no packet: the connections stay in the table, and
    /// `take` or another `open` handles what comes.
    pub async fn open(
        &mut self,
        ports: RangeInclusive<u16>,
        dst: SocketAddr,
        hello: usize,
    ) -> io::Result<()> {
        let SocketAddr::V4(dst) = dst else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the scripted client has IPv4 only",
            ));
        };
        let mut waiting = HashSet::new();
        for port in ports {
            if !self.ports.contains(&port) || self.conns.contains_key(&port) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("port {port} is not free to open from"),
                ));
            }
            // spread over the space, the same for the same port
            let isn = u32::from(port).wrapping_mul(0x9e37_79b1);
            self.conns.insert(
                port,
                Conn {
                    dst,
                    phase: Phase::SynSent,
                    isn,
                    sent: isn.wrapping_add(1),
                    hello,
                    acked: false,
                    peer_isn: 0,
                    expected: 0,
                    ack_due: false,
                },
            );
            let syn = segment(port, dst, TcpControl::Syn, isn, None, Some(MSS), &[]);
            self.app.inject(syn);
            waiting.insert(port);
        }
        self.drive(waiting, Conn::settled).await
    }

    /// Acknowledges everything from the adapter (window 65535) until every
    /// open connection has had `bytes`. Gives up as `open` does, and is
    /// dropped as safely.
    pub async fn take(&mut self, bytes: usize) -> io::Result<()> {
        let waiting = self
            .conns
            .iter()
            .filter(|(_, conn)| conn.taken() < bytes)
            .map(|(&port, _)| port)
            .collect();
        self.drive(waiting, move |conn| conn.taken() >= bytes).await
    }

    /// What the table of connections takes of memory, about.
    pub fn table_bytes(&self) -> usize {
        self.conns.capacity() * (size_of::<(u16, Conn)>() + 1)
    }

    /// Handles what comes until every connection of `waiting` is `done`:
    /// the packets that are there at once, then the ACKs they call for.
    async fn drive(
        &mut self,
        mut waiting: HashSet<u16>,
        done: impl Fn(&Conn) -> bool,
    ) -> io::Result<()> {
        waiting.retain(|port| !done(&self.conns[port]));
        let mut touched = HashSet::new();
        while !waiting.is_empty() {
            let first = match tokio::time::timeout(IDLE, self.packets.recv()).await {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "the app's stack stopped handing packets over",
                    ));
                }
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "nothing came for {IDLE:?}; {} connections are still waiting",
                            waiting.len()
                        ),
                    ));
                }
            };
            self.handle(&first, &mut touched)?;
            for _ in 1..BATCH {
                let Ok(packet) = self.packets.try_recv() else {
                    break;
                };
                self.handle(&packet, &mut touched)?;
            }
            for port in touched.drain() {
                let conn = self
                    .conns
                    .get_mut(&port)
                    .expect("a connection of the table");
                if conn.ack_due {
                    conn.ack_due = false;
                    let ack = segment(
                        port,
                        conn.dst,
                        TcpControl::None,
                        conn.sent,
                        Some(conn.expected),
                        None,
                        &[],
                    );
                    self.app.inject(ack);
                }
                if done(conn) {
                    waiting.remove(&port);
                }
            }
        }
        Ok(())
    }

    /// One packet of the adapter's: a segment of one of the connections, or
    /// nothing this client knows. `touched` gets the port of the connection
    /// it concerned.
    fn handle(&mut self, packet: &[u8], touched: &mut HashSet<u16>) -> io::Result<()> {
        let Ok(ip) = Ipv4Packet::new_checked(packet) else {
            return Ok(());
        };
        if ip.next_header() != IpProtocol::Tcp {
            return Ok(());
        }
        let Ok(tcp) = TcpPacket::new_checked(ip.payload()) else {
            return Ok(());
        };
        // the adapter's checksums are the adapter's to get right; this is a
        // client that reads what it needs
        let parsed = TcpRepr::parse(
            &tcp,
            &IpAddress::Ipv4(ip.src_addr()),
            &IpAddress::Ipv4(ip.dst_addr()),
            &ChecksumCapabilities::ignored(),
        );
        let Ok(repr) = parsed else {
            return Ok(());
        };
        let port = repr.dst_port;
        let Some(conn) = self.conns.get_mut(&port) else {
            return Ok(());
        };
        if SocketAddrV4::new(ip.src_addr(), repr.src_port) != conn.dst {
            return Ok(());
        }
        let seq = repr.seq_number.0 as u32;
        match conn.phase {
            Phase::SynSent => match (repr.control, repr.ack_number) {
                (TcpControl::Syn, Some(ack)) if ack.0 as u32 == conn.isn.wrapping_add(1) => {
                    conn.peer_isn = seq;
                    conn.expected = seq.wrapping_add(1);
                    conn.phase = Phase::Established;
                    conn.sent = conn.isn.wrapping_add(1).wrapping_add(conn.hello as u32);
                    conn.acked = conn.hello == 0;
                    // the ACK that ends the handshake carries the hello
                    let hello = vec![0; conn.hello];
                    let ack = segment(
                        port,
                        conn.dst,
                        TcpControl::Psh,
                        conn.isn.wrapping_add(1),
                        Some(conn.expected),
                        None,
                        &hello,
                    );
                    self.app.inject(ack);
                    touched.insert(port);
                }
                (TcpControl::Rst, _) => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        format!("the connection from port {port} was answered with a reset"),
                    ));
                }
                _ => {}
            },
            Phase::Established => {
                if repr.control == TcpControl::Rst {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        format!("the connection from port {port} was reset"),
                    ));
                }
                if let Some(ack) = repr.ack_number
                    && !after(conn.sent, ack.0 as u32)
                {
                    conn.acked = true;
                }
                let length = repr.segment_len();
                if length > 0 {
                    // new if it starts where the stream is, or before, and ends after
                    let end = seq.wrapping_add(length as u32);
                    if !after(seq, conn.expected) && after(end, conn.expected) {
                        conn.expected = end;
                    }
                    // new or not, it is answered with where the stream is
                    conn.ack_due = true;
                }
                touched.insert(port);
            }
        }
        Ok(())
    }
}

/// An IPv4 packet from `198.18.0.1:port` to `dst` around one TCP segment,
/// the checksums of both headers right.
fn segment(
    port: u16,
    dst: SocketAddrV4,
    control: TcpControl,
    seq: u32,
    ack: Option<u32>,
    mss: Option<u16>,
    payload: &[u8],
) -> Vec<u8> {
    let tcp = TcpRepr {
        src_port: port,
        dst_port: dst.port(),
        control,
        seq_number: TcpSeqNumber(seq as i32),
        ack_number: ack.map(|ack| TcpSeqNumber(ack as i32)),
        window_len: WINDOW,
        window_scale: None,
        max_seg_size: mss,
        sack_permitted: false,
        sack_ranges: [None; 3],
        timestamp: None,
        payload,
    };
    let ip = Ipv4Repr {
        src_addr: SOURCE,
        dst_addr: *dst.ip(),
        next_header: IpProtocol::Tcp,
        payload_len: tcp.buffer_len(),
        hop_limit: 64,
    };
    let checksums = ChecksumCapabilities::default();
    let mut packet = vec![0; ip.buffer_len() + tcp.buffer_len()];
    ip.emit(&mut Ipv4Packet::new_unchecked(&mut packet[..]), &checksums);
    tcp.emit(
        &mut TcpPacket::new_unchecked(&mut packet[ip.buffer_len()..]),
        &IpAddress::Ipv4(SOURCE),
        &IpAddress::Ipv4(*dst.ip()),
        &checksums,
    );
    packet
}
