//! What the TUN reads of an IP packet before it decides where the packet goes
//! (phase 3 M3 design 4.4): [`classify`] runs the checks every packet passes
//! before any stack sees it, in the order of the design's table. smoltcp 0.12
//! panics on or misreads several things a packet can hold (design 5.3), so
//! nothing it would take reaches it without going through here first.

use crate::tcp::Tuple;
use smoltcp::wire::{IpProtocol, Ipv4Packet, Ipv6Packet, TcpPacket, UdpPacket};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// ICMP's type, code and checksum, and the identifier and sequence number an
/// echo has after them: the shortest header of a message whose type is read.
const ICMP_HEADER: usize = 8;
const ICMPV4_ECHO_REQUEST: u8 = 8;
const ICMPV6_ECHO_REQUEST: u8 = 128;

/// Why a packet is dropped. Each reason is the counter of the same name in
/// [`crate::Counters`], and that name is what the logs call it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    /// A packet that cannot be read: a version other than 4 and 6, lengths
    /// that do not fit, an IPv4 header checksum that does not hold, a
    /// transport header that is cut short or has port 0.
    Malformed,
    /// A fragment, which the TUN does not reassemble.
    Fragment,
    /// An address no packet through the adapter has: unspecified, loopback,
    /// multicast, the limited broadcast, an IPv4-mapped IPv6 address.
    Martian,
    /// A packet nothing handles: IPv6 while it is off, extension headers,
    /// protocols other than TCP, UDP and ICMP, an ICMP that is no echo
    /// request.
    Unsupported,
    /// The queue of the shard the packet goes to was full.
    QueueFull,
    /// A SYN to an address of rurge's own space that names nothing.
    // not built yet: the shard's SYN handling builds it and removes this expectation
    #[cfg_attr(not(test), expect(dead_code))]
    UnknownFake,
    /// A SYN turned away because of the limits on sockets and half-open
    /// connections.
    // not built yet: the shard's SYN handling builds it and removes this expectation
    #[cfg_attr(not(test), expect(dead_code))]
    Busy,
    /// A segment with data past the window the TUN last told of.
    // not built yet: the shard's window guard builds it and removes this expectation
    #[cfg_attr(not(test), expect(dead_code))]
    BeyondWindow,
    /// A reset not sent because of the limit on resets.
    // not built yet: the shard's SYN handling builds it and removes this expectation
    #[cfg_attr(not(test), expect(dead_code))]
    RstLimited,
    /// An echo request or its reply held back by the limit on ICMP.
    // not built yet: the ICMP path builds it and removes this expectation
    #[cfg_attr(not(test), expect(dead_code))]
    IcmpLimited,
}

impl Reason {
    /// Martians are counted and never said: a real adapter sees multicast and
    /// broadcast all the time (design 14).
    pub(crate) fn is_silent(self) -> bool {
        self == Reason::Martian
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Reason::Malformed => "malformed",
            Reason::Fragment => "fragment",
            Reason::Martian => "martian",
            Reason::Unsupported => "unsupported",
            Reason::QueueFull => "queue_full",
            Reason::UnknownFake => "unknown_fake",
            Reason::Busy => "busy",
            Reason::BeyondWindow => "beyond_window",
            Reason::RstLimited => "rst_limited",
            Reason::IcmpLimited => "icmp_limited",
        })
    }
}

/// A packet that was turned away: why, and the two addresses if it got far
/// enough for them to be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Dropped {
    pub(crate) reason: Reason,
    /// The packet's source and destination, for a drop decided after its IP
    /// header passed its own checks. `None` for one decided before that, and
    /// for IPv6 while it is off, which is decided on the version alone.
    pub(crate) ends: Option<(IpAddr, IpAddr)>,
}

impl Dropped {
    /// Turned away before the IP header passed its own checks, or on the
    /// version alone: nothing of the packet is to be believed yet.
    fn unread(reason: Reason) -> Dropped {
        Dropped { reason, ends: None }
    }

    /// Turned away after the IP header passed its own checks: its two
    /// addresses are the packet's, whatever else is wrong with it.
    fn between(reason: Reason, src: IpAddr, dst: IpAddr) -> Dropped {
        Dropped {
            reason,
            ends: Some((src, dst)),
        }
    }
}

/// A packet that passed every check, as far as the TUN reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Parsed {
    /// The packet's source and destination, of one family: the packet's.
    pub(crate) src: IpAddr,
    pub(crate) dst: IpAddr,
    /// Where the IP packet ends: IPv4's total length, IPv6's 40 plus the
    /// payload length. Whatever follows is not part of it.
    pub(crate) len: usize,
    pub(crate) transport: Transport,
}

/// What the transport header says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Transport {
    Tcp(Segment),
    Udp {
        src_port: u16,
        dst_port: u16,
    },
    /// An echo request, the one ICMP message that passes: type 8 over IPv4,
    /// 128 over IPv6.
    Icmp {
        message_type: u8,
    },
}

/// The ends of a TCP segment, the flags that say what it opens or ends, its
/// numbers and the size of its two parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Segment {
    /// From the app (the packet's source) to where it went.
    pub(crate) tuple: Tuple,
    pub(crate) syn: bool,
    pub(crate) ack: bool,
    pub(crate) fin: bool,
    pub(crate) rst: bool,
    pub(crate) seq: u32,
    pub(crate) ack_number: u32,
    /// The TCP header's length in bytes, options included.
    pub(crate) header_len: usize,
    pub(crate) payload_len: usize,
}

/// Runs the checks on `packet`, in this order, the first that holds deciding:
/// a packet that cannot be read; IPv6 while `ipv6` is off (an IPv6 packet is
/// turned away on its version nibble, however damaged, so that check is
/// really the first); a fragment; an address no packet has; a protocol that
/// is not TCP, UDP or ICMP; a transport header that is cut short or names
/// port 0; an ICMP message that is no echo request. Whatever is after the end
/// of the IP packet is left out of [`Parsed::len`]. A packet turned away
/// comes back with the reason and, once its IP header passed its own checks,
/// its two addresses ([`Dropped`]).
pub(crate) fn classify(packet: &[u8], ipv6: bool) -> Result<Parsed, Dropped> {
    // IPv6 is dropped before any of its header is read, however damaged
    if !ipv6 && packet.first().is_some_and(|byte| byte >> 4 == 6) {
        return Err(Dropped::unread(Reason::Unsupported));
    }
    parse(packet)
}

/// The TCP segment of a packet that [`classify`] let through, read once more
/// for the shard it was sent to: the shard's queue and turns take plain
/// packets. `None` for a packet that is no TCP segment, which no shard is
/// sent; nothing is counted here.
pub(crate) fn segment(packet: &[u8]) -> Option<Segment> {
    match parse(packet) {
        Ok(Parsed {
            transport: Transport::Tcp(segment),
            ..
        }) => Some(segment),
        _ => None,
    }
}

/// Every check of [`classify`] but the one of the `ipv6` option.
fn parse(packet: &[u8]) -> Result<Parsed, Dropped> {
    match packet.first().map(|byte| byte >> 4) {
        Some(4) => read_ipv4(packet),
        Some(6) => read_ipv6(packet),
        _ => Err(Dropped::unread(Reason::Malformed)),
    }
}

fn read_ipv4(packet: &[u8]) -> Result<Parsed, Dropped> {
    let ip = Ipv4Packet::new_checked(packet).map_err(|_| Dropped::unread(Reason::Malformed))?;
    // smoltcp takes a header of fewer than five words, and then reads the
    // TCP header from inside the IP header (design 5.3, H4)
    if ip.header_len() < 20 || !ip.verify_checksum() {
        return Err(Dropped::unread(Reason::Malformed));
    }
    // the header is sound: the addresses are the packet's to give from here on
    let (src, dst) = (ip.src_addr(), ip.dst_addr());
    if ip.more_frags() || ip.frag_offset() != 0 {
        return Err(Dropped::between(Reason::Fragment, src.into(), dst.into()));
    }
    if martian_v4(src, dst) {
        return Err(Dropped::between(Reason::Martian, src.into(), dst.into()));
    }
    transport(
        src.into(),
        dst.into(),
        ip.next_header(),
        ip.payload(),
        usize::from(ip.total_len()),
    )
}

fn read_ipv6(packet: &[u8]) -> Result<Parsed, Dropped> {
    let ip = Ipv6Packet::new_checked(packet).map_err(|_| Dropped::unread(Reason::Malformed))?;
    // the header is sound: the addresses are the packet's to give from here on
    let (src, dst) = (ip.src_addr(), ip.dst_addr());
    if ip.next_header() == IpProtocol::Ipv6Frag {
        return Err(Dropped::between(Reason::Fragment, src.into(), dst.into()));
    }
    if martian_v6(src, dst) {
        return Err(Dropped::between(Reason::Martian, src.into(), dst.into()));
    }
    transport(
        src.into(),
        dst.into(),
        ip.next_header(),
        ip.payload(),
        ip.total_len(),
    )
}

/// An address of the unspecified, loopback or multicast kinds on either end,
/// or the limited broadcast as the destination. smoltcp takes a SYN from
/// `0.0.0.0` and then panics building the SYN-ACK to it (design 5.3, H1).
fn martian_v4(src: Ipv4Addr, dst: Ipv4Addr) -> bool {
    let odd = |address: Ipv4Addr| {
        address.is_unspecified() || address.is_loopback() || address.is_multicast()
    };
    odd(src) || odd(dst) || dst.is_broadcast()
}

/// The same for IPv6, with the IPv4-mapped addresses (`::ffff:a.b.c.d`) in
/// place of the broadcast one.
fn martian_v6(src: Ipv6Addr, dst: Ipv6Addr) -> bool {
    let odd = |address: Ipv6Addr| {
        address.is_unspecified()
            || address.is_loopback()
            || address.is_multicast()
            || address.to_ipv4_mapped().is_some()
    };
    odd(src) || odd(dst)
}

/// The transport header of the IP packet whose payload is `payload`.
fn transport(
    src: IpAddr,
    dst: IpAddr,
    protocol: IpProtocol,
    payload: &[u8],
    len: usize,
) -> Result<Parsed, Dropped> {
    let transport = match (protocol, src) {
        (IpProtocol::Tcp, _) => tcp(src, dst, payload).map(Transport::Tcp),
        (IpProtocol::Udp, _) => udp(payload),
        (IpProtocol::Icmp, IpAddr::V4(_)) => icmp(payload, ICMPV4_ECHO_REQUEST),
        (IpProtocol::Icmpv6, IpAddr::V6(_)) => icmp(payload, ICMPV6_ECHO_REQUEST),
        // an extension header, or a protocol nothing here handles
        _ => Err(Reason::Unsupported),
    }
    .map_err(|reason| Dropped::between(reason, src, dst))?;
    Ok(Parsed {
        src,
        dst,
        len,
        transport,
    })
}

fn tcp(src: IpAddr, dst: IpAddr, payload: &[u8]) -> Result<Segment, Reason> {
    let tcp = TcpPacket::new_checked(payload).map_err(|_| Reason::Malformed)?;
    let (src_port, dst_port) = (tcp.src_port(), tcp.dst_port());
    // port 0 is no port a connection is made to or from
    if src_port == 0 || dst_port == 0 {
        return Err(Reason::Malformed);
    }
    let header_len = usize::from(tcp.header_len());
    Ok(Segment {
        tuple: Tuple {
            src: SocketAddr::new(src, src_port),
            dst: SocketAddr::new(dst, dst_port),
        },
        syn: tcp.syn(),
        ack: tcp.ack(),
        fin: tcp.fin(),
        rst: tcp.rst(),
        seq: tcp.seq_number().0 as u32,
        ack_number: tcp.ack_number().0 as u32,
        header_len,
        payload_len: payload.len() - header_len,
    })
}

fn udp(payload: &[u8]) -> Result<Transport, Reason> {
    let udp = UdpPacket::new_checked(payload).map_err(|_| Reason::Malformed)?;
    let (src_port, dst_port) = (udp.src_port(), udp.dst_port());
    if src_port == 0 || dst_port == 0 {
        return Err(Reason::Malformed);
    }
    Ok(Transport::Udp { src_port, dst_port })
}

fn icmp(payload: &[u8], echo_request: u8) -> Result<Transport, Reason> {
    if payload.len() < ICMP_HEADER {
        return Err(Reason::Malformed);
    }
    if payload[0] != echo_request {
        return Err(Reason::Unsupported);
    }
    Ok(Transport::Icmp {
        message_type: payload[0],
    })
}
