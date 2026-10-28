//! What the TUN reads of an IP packet before it decides where the packet goes
//! (phase 3 M3 design 4.4): [`classify`] runs the checks every packet passes
//! before any stack sees it, in the order of the design's table. smoltcp 0.12
//! panics on or misreads several things a packet can hold (design 5.3), so
//! nothing it would take reaches it without going through here first. And
//! two packets the TUN makes itself: [`rst_for`], the reset it answers a SYN
//! with when it turns the SYN away, and [`udp_datagram`], the UDP datagram of a
//! reply to an app, with [`udp_packets`], which puts it in IP packets.

use crate::fragment::{Ids, ip_packets};
use crate::tcp::Tuple;
use rurge_config::{IpRewrite, IpRewriteAction};
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    IPV4_HEADER_LEN, Icmpv4Packet, Icmpv6Packet, IpAddress, IpProtocol, Ipv4Packet, Ipv4Repr,
    Ipv6Packet, Ipv6Repr, TcpControl, TcpPacket, TcpRepr, TcpSeqNumber, UDP_HEADER_LEN, UdpPacket,
    UdpRepr,
};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// ICMP's type, code and checksum, and the identifier and sequence number an
/// echo has after them: the shortest header of a message whose type is read.
const ICMP_HEADER: usize = 8;
/// The length of IPv6's fixed header: where its payload begins, and the length
/// of the IP header of an IPv6 packet.
const IPV6_HEADER_LEN: usize = 40;
const ICMPV4_ECHO_REQUEST: u8 = 8;
const ICMPV6_ECHO_REQUEST: u8 = 128;

/// The time to live of IPv4 and the hop limit of IPv6 on the resets rurge
/// makes: what operating systems send by default.
const RESET_HOP_LIMIT: u8 = 64;

/// The kinds of the TCP options that are read as they are written down: the
/// end of the list, no operation, and the offer of selective acknowledgements.
const OPTION_END: u8 = 0;
const OPTION_NOP: u8 = 1;
const OPTION_SACK_PERMITTED: u8 = 4;

/// Why a packet is dropped. Each reason is the counter of the same name in
/// [`crate::Counters`], and that name is what the logs call it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    /// A packet that cannot be read: a version other than 4 and 6, lengths
    /// that do not fit, an IPv4 header checksum that does not hold, a
    /// transport header that is cut short or has port 0, the checksum of a UDP
    /// datagram or of an ICMP echo request that does not hold (or that a UDP
    /// datagram over IPv6 does not have).
    Malformed,
    /// A fragment, which the TUN does not reassemble.
    Fragment,
    /// An address no packet through the adapter has: unspecified, loopback,
    /// multicast, the limited broadcast, an IPv4-mapped IPv6 address.
    Martian,
    /// A packet nothing handles: IPv6 while it is off, extension headers,
    /// protocols other than TCP, UDP and ICMP, an ICMP that is no echo
    /// request, an echo request while `icmp-forwarding` is off; a UDP
    /// datagram when the dialer carries no UDP, and a reply from an address
    /// of the other family than the app's.
    Unsupported,
    /// The queue the packet goes to was full: a shard's, a UDP session's (or
    /// the bytes the sessions' queues may hold were spent), the DNS responder's,
    /// the writer's (for an ICMP message or an answer of the responder).
    QueueFull,
    /// A packet for an address of rurge's own space that names nothing: a SYN,
    /// a UDP datagram, an echo request, or a reply from a name that has no
    /// address.
    UnknownFake,
    /// A packet turned away because of a limit: of sockets and half-open
    /// connections for a SYN, and of connections to the responder's port 53
    /// for a SYN to it, of associations and of destinations for a UDP
    /// datagram, of echo requests in flight for an echo request.
    Busy,
    /// A segment with data past the window the TUN last told of, dropped
    /// because the receive buffer had any room at all, however long the
    /// segment: the stack would take in the part that fits, past the edge it
    /// told of. Into a full buffer the stack takes in nothing and answers, so
    /// that segment is not dropped and not counted.
    BeyondWindow,
    /// A reset not sent because of the limit on resets.
    RstLimited,
    /// An ICMP message for a refused UDP flow held back by the limit on ICMP.
    IcmpLimited,
    /// A packet `[IP Rewrite]` rejected or dropped: what the profile asked
    /// for, so it is counted and said at DEBUG, never at WARN.
    IpRewrite,
}

impl Reason {
    /// Martians are counted and never said: a real adapter sees multicast and
    /// broadcast all the time (design 14).
    pub(crate) fn is_silent(self) -> bool {
        self == Reason::Martian
    }

    /// A packet dropped because `[IP Rewrite]` asked for it: counted and said
    /// at DEBUG every time, and never at WARN, because it is what the profile
    /// asked for rather than a packet that something could not handle (M5
    /// design 8.7).
    pub(crate) fn is_wanted(self) -> bool {
        self == Reason::IpRewrite
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
            Reason::IpRewrite => "ip_rewrite",
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

/// What [`classify`] makes of a packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Classified {
    /// Every check passed, as before: the transport header is read.
    Passed(Parsed),
    /// `[IP Rewrite]` has a line for the destination: what is past the IP
    /// header was not read.
    Rewrite(Hit),
}

/// A packet `[IP Rewrite]` matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hit {
    pub(crate) action: IpRewriteAction,
    pub(crate) src: IpAddr,
    pub(crate) dst: IpAddr,
    /// Where the IP packet ends, as [`Parsed::len`].
    pub(crate) len: usize,
    /// The IP header's length: IPv4's IHL × 4, IPv6's 40.
    pub(crate) header_len: usize,
}

/// What a packet `reject` matched is answered with (M5 design 8.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    Nothing,
    Reset,
    Prohibited,
}

/// Runs the checks on `packet`, in this order, the first that holds deciding:
/// IPv6 while `ipv6` is off (an IPv6 packet is turned away on its version
/// nibble, however damaged, so that check is really the first); a packet that
/// cannot be read; an address no packet has; a line of `[IP Rewrite]` for the
/// destination, which answers what the profile asks and stops there; a
/// fragment; a protocol that is not TCP, UDP or ICMP; a transport header that
/// is cut short or names port 0; the checksum of a UDP datagram, which does
/// not hold or, over IPv6, is left out; an ICMP message that is no echo
/// request; the checksum of an echo request, which does not hold. The checksum
/// of TCP is not looked at (M3 design 4.4), and nor is that of an ICMP message
/// that is no echo request, which is dropped as it is. Whatever is after the
/// end of the IP packet is left out of [`Parsed::len`]. A packet turned away
/// comes back with the reason and, once its IP header passed its own checks,
/// its two addresses ([`Dropped`]).
pub(crate) fn classify(
    packet: &[u8],
    ipv6: bool,
    rewrite: &IpRewrite,
) -> Result<Classified, Dropped> {
    // IPv6 is dropped before any of its header is read, however damaged
    if !ipv6 && packet.first().is_some_and(|byte| byte >> 4 == 6) {
        return Err(Dropped::unread(Reason::Unsupported));
    }
    parse(packet, rewrite)
}

/// [`classify`] with an empty `[IP Rewrite]`: every check a packet passes on
/// its way to a stack, with none of the lines that would take it instead. For
/// the tests of the checks themselves.
#[cfg(test)]
pub(crate) fn checks(packet: &[u8], ipv6: bool) -> Result<Parsed, Dropped> {
    match classify(packet, ipv6, &IpRewrite::default()) {
        Ok(Classified::Passed(parsed)) => Ok(parsed),
        Ok(Classified::Rewrite(_)) => unreachable!("an empty [IP Rewrite] has no line"),
        Err(dropped) => Err(dropped),
    }
}

/// The TCP segment of a packet that [`classify`] let through, read once more
/// for the shard it was sent to: the shard's queue and turns take plain
/// packets. `None` for a packet that is no TCP segment, which no shard is
/// sent; nothing is counted here, and `[IP Rewrite]` is not asked: a segment
/// that is asked of this is not one the table takes.
pub(crate) fn segment(packet: &[u8]) -> Option<Segment> {
    match parse(packet, &IpRewrite::default()) {
        Ok(Classified::Passed(Parsed {
            transport: Transport::Tcp(segment),
            ..
        })) => Some(segment),
        _ => None,
    }
}

/// What a packet `reject` matched is answered with, the first rule that holds
/// deciding (M5 design 8.5 and 21, plan P11 and its execution record row 70): a later fragment, an ICMP error or
/// redirect, an extension header chain that is not walked, and a type that
/// cannot be read answer nothing (RFC 1122 3.2.2, RFC 4443 2.4); an
/// unfragmented TCP segment answers a reset only for a SYN without ACK, and
/// nothing for a reset or a header that cannot be read; everything else
/// answers with "administratively prohibited". The two addresses were read by
/// [`classify`], and `packet` is cut to its length: what cannot be read there
/// answers nothing rather than failing.
pub(crate) fn refusal(packet: &[u8]) -> Refusal {
    match packet.first().map(|byte| byte >> 4) {
        Some(4) => refusal_v4(packet),
        Some(6) => refusal_v6(packet),
        _ => Refusal::Nothing,
    }
}

fn refusal_v4(packet: &[u8]) -> Refusal {
    let Ok(ip) = Ipv4Packet::new_checked(packet) else {
        return Refusal::Nothing;
    };
    // a later fragment has no transport header to read: only the first answers
    if ip.frag_offset() != 0 {
        return Refusal::Nothing;
    }
    // ICMPv4's errors are 3, 4, 5, 11 and 12; a first fragment is looked at too
    if ip.next_header() == IpProtocol::Icmp {
        let Some(&kind) = ip.payload().first() else {
            return Refusal::Nothing;
        };
        if matches!(kind, 3 | 4 | 5 | 11 | 12) {
            return Refusal::Nothing;
        }
    }
    // only an unfragmented TCP segment is read for a reset
    if !ip.more_frags() && ip.next_header() == IpProtocol::Tcp {
        return refusal_tcp(packet);
    }
    Refusal::Prohibited
}

fn refusal_v6(packet: &[u8]) -> Refusal {
    let Ok(ip) = Ipv6Packet::new_checked(packet) else {
        return Refusal::Nothing;
    };
    let payload = ip.payload();
    let mut fragmented = false;
    let mut next_header = ip.next_header();
    if next_header == IpProtocol::Ipv6Frag {
        // next header, reserved, the offset in units of 8 bytes and the more
        // fragments bit, and the identification
        if payload.len() < 8 {
            return Refusal::Nothing;
        }
        if u16::from_be_bytes([payload[2], payload[3]]) >> 3 != 0 {
            return Refusal::Nothing;
        }
        fragmented = true;
        next_header = IpProtocol::from(payload[0]);
    }
    // a chain of extension headers — anything beyond the one fragment header
    // read above — is not walked: the message after it cannot be read, so
    // nothing is answered
    if is_ipv6_extension_header(next_header) {
        return Refusal::Nothing;
    }
    // ICMPv6's errors are types below 128, and its redirects (137) are not
    // answered either (RFC 4443 2.4(e)); a first fragment says its type after
    // its fragment header
    let kind = if next_header == IpProtocol::Icmpv6 {
        let at = if fragmented { 8 } else { 0 };
        match payload.get(at) {
            Some(kind) => Some(*kind),
            None => return Refusal::Nothing,
        }
    } else {
        None
    };
    if kind.is_some_and(is_icmpv6_error) {
        return Refusal::Nothing;
    }
    // only an unfragmented TCP segment is read for a reset
    if !fragmented && next_header == IpProtocol::Tcp {
        return refusal_tcp(packet);
    }
    Refusal::Prohibited
}

/// Whether `kind` is an ICMPv6 error or redirect: an error (below 128) or a
/// redirect (137), for either of which no error is answered (RFC 4443 2.4(e),
/// M5 design 8.5).
fn is_icmpv6_error(kind: u8) -> bool {
    kind < 128 || kind == 137
}

/// Whether `protocol` names an IPv6 extension header (IANA's "IPv6 Extension
/// Header Types": Hop-by-Hop, Routing, Fragment, ESP, AH, Destination
/// Options, Mobility, HIP, Shim6 and the two experimental ones). [`refusal_v6`]
/// reads one fragment header and does not walk a chain of the rest, so what
/// comes after an extension header cannot be read and is answered with
/// nothing (M5 design 8.5 and 21, plan P11 and its execution record row 70).
fn is_ipv6_extension_header(protocol: IpProtocol) -> bool {
    matches!(
        u8::from(protocol),
        0 | 43 | 44 | 50 | 51 | 60 | 135 | 139 | 140 | 253 | 254
    )
}

fn refusal_tcp(packet: &[u8]) -> Refusal {
    match segment(packet) {
        // a header that cannot be read, or port 0
        None => Refusal::Nothing,
        // never a reset for a reset
        Some(segment) if segment.rst => Refusal::Nothing,
        Some(segment) if segment.syn && !segment.ack => Refusal::Reset,
        Some(_) => Refusal::Prohibited,
    }
}

/// The reset rurge answers a SYN with when it turns the SYN away (plan P7).
/// It goes from where the SYN went to where it came from, with RST
/// and ACK set, sequence number 0 and, as the acknowledgement number, the
/// SYN's sequence number plus one for the SYN itself plus the bytes the SYN
/// carries (wrapping round the end); the window is 0 and there are no
/// options. The time to live or hop limit is 64, and both checksums are worked
/// out, since the operating system's stack verifies them. `syn` is a SYN that
/// passed [`classify`].
pub(crate) fn rst_for(syn: &[u8]) -> Vec<u8> {
    let read = segment(syn).expect("a SYN that passed classify is a TCP segment");
    let (to, from) = (read.tuple.src, read.tuple.dst);
    let reset = TcpRepr {
        src_port: from.port(),
        dst_port: to.port(),
        control: TcpControl::Rst,
        seq_number: TcpSeqNumber(0),
        ack_number: Some(TcpSeqNumber(
            read.seq
                .wrapping_add(1)
                .wrapping_add(read.payload_len as u32) as i32,
        )),
        window_len: 0,
        window_scale: None,
        max_seg_size: None,
        sack_permitted: false,
        sack_ranges: [None; 3],
        timestamp: None,
        payload: &[],
    };
    let checksums = ChecksumCapabilities::default();
    match (from.ip(), to.ip()) {
        (IpAddr::V4(from), IpAddr::V4(to)) => {
            let ip = Ipv4Repr {
                src_addr: from,
                dst_addr: to,
                next_header: IpProtocol::Tcp,
                payload_len: reset.buffer_len(),
                hop_limit: RESET_HOP_LIMIT,
            };
            let mut packet = vec![0; ip.buffer_len() + reset.buffer_len()];
            ip.emit(&mut Ipv4Packet::new_unchecked(&mut packet[..]), &checksums);
            reset.emit(
                &mut TcpPacket::new_unchecked(&mut packet[ip.buffer_len()..]),
                &IpAddress::Ipv4(from),
                &IpAddress::Ipv4(to),
                &checksums,
            );
            packet
        }
        (IpAddr::V6(from), IpAddr::V6(to)) => {
            let ip = Ipv6Repr {
                src_addr: from,
                dst_addr: to,
                next_header: IpProtocol::Tcp,
                payload_len: reset.buffer_len(),
                hop_limit: RESET_HOP_LIMIT,
            };
            let mut packet = vec![0; ip.buffer_len() + reset.buffer_len()];
            ip.emit(&mut Ipv6Packet::new_unchecked(&mut packet[..]));
            reset.emit(
                &mut TcpPacket::new_unchecked(&mut packet[ip.buffer_len()..]),
                &IpAddress::Ipv6(from),
                &IpAddress::Ipv6(to),
                &checksums,
            );
            packet
        }
        _ => unreachable!("the two addresses of a packet are of one family"),
    }
}

/// The UDP datagram from `src` to `dst` that carries `payload`, its checksum
/// worked out. `None` if it is longer than a packet of the family can carry:
/// IPv4's packet is at most 65535 bytes, with a header of 20, and IPv6's
/// payload is at most 65535, with the UDP header in it.
fn udp_datagram(src: SocketAddr, dst: SocketAddr, payload: &[u8]) -> Option<Vec<u8>> {
    let longest = if src.is_ipv4() { 65535 - 20 } else { 65535 };
    if UDP_HEADER_LEN + payload.len() > longest {
        return None;
    }
    let repr = UdpRepr {
        src_port: src.port(),
        dst_port: dst.port(),
    };
    let mut bytes = vec![0; UDP_HEADER_LEN + payload.len()];
    repr.emit(
        &mut UdpPacket::new_unchecked(&mut bytes[..]),
        &src.ip().into(),
        &dst.ip().into(),
        payload.len(),
        |buffer| buffer.copy_from_slice(payload),
        &ChecksumCapabilities::default(),
    );
    Some(bytes)
}

/// The IP packets that carry the UDP datagram from `src` to `dst` with
/// `payload` (see [`udp_datagram`]): one packet if it fits the `mtu`, else its
/// fragments (see [`ip_packets`]). `None` if the payload is too long to be a
/// datagram. The one way the replies of the UDP sessions and the answers of the
/// DNS responder are made into packets.
pub(crate) fn udp_packets(
    src: SocketAddr,
    dst: SocketAddr,
    payload: &[u8],
    mtu: usize,
    ids: &Ids,
) -> Option<Vec<Vec<u8>>> {
    let datagram = udp_datagram(src, dst, payload)?;
    Some(ip_packets(
        src.ip(),
        dst.ip(),
        IpProtocol::Udp,
        &datagram,
        mtu,
        ids,
    ))
}

/// Takes the SACK-permitted options out of `packet`, a whole IP packet, if it
/// is a TCP SYN: each, kind 4 and length 2, becomes two no-operations (kind
/// 1), so the packet is as long as it was. smoltcp 0.12 then takes the app
/// as a peer that has no SACK, and never builds a SACK block, which it does
/// with additions that overflow in a debug build (design 5.3, H3). Local
/// links lose next to nothing, so nothing is lost. The checksum of the TCP
/// segment is not worked out again: the TUN does not check it (the device
/// takes it as right). A packet that is no SYN, no TCP, or cut short is left
/// as it is, and so is one whose option table is not sound (an option whose
/// length is under two, or goes past the end of the table): smoltcp drops
/// such a SYN itself.
pub(crate) fn strip_sack_permitted(packet: &mut [u8]) {
    let Some(start) = tcp_start(packet) else {
        return;
    };
    let Some(segment) = packet.get_mut(start..) else {
        return;
    };
    let Ok(mut tcp) = TcpPacket::new_checked(segment) else {
        return;
    };
    if !tcp.syn() {
        return;
    }
    let options = tcp.options_mut();
    // the whole table is sound before any of it is changed
    let mut at = 0;
    while at < options.len() {
        match option_at(options, at) {
            None => return,
            Some((OPTION_END, _)) => break,
            Some((_, length)) => at += length,
        }
    }
    let mut at = 0;
    while at < options.len() {
        let Some((kind, length)) = option_at(options, at) else {
            break;
        };
        if kind == OPTION_END {
            break;
        }
        if kind == OPTION_SACK_PERMITTED && length == 2 {
            options[at..at + 2].fill(OPTION_NOP);
        }
        at += length;
    }
}

/// Where the TCP header of `packet` begins, if the packet is an IPv4 packet
/// of the protocol TCP with a header of the length of 20 bytes or more, or an
/// IPv6 packet of that protocol with no extension header. The length and the
/// protocol are read by smoltcp's own packet types, which take a packet only
/// if it is as long as it says.
fn tcp_start(packet: &[u8]) -> Option<usize> {
    match packet.first()? >> 4 {
        4 => {
            let ip = Ipv4Packet::new_checked(packet).ok()?;
            let header = usize::from(ip.header_len());
            (header >= IPV4_HEADER_LEN && ip.next_header() == IpProtocol::Tcp).then_some(header)
        }
        6 => {
            let ip = Ipv6Packet::new_checked(packet).ok()?;
            (ip.next_header() == IpProtocol::Tcp).then_some(ip.header_len())
        }
        _ => None,
    }
}

/// The kind and the length of the option at `at`, which is in `options`: one
/// byte for the end of the list and a no-operation, the option's own length
/// for any other. `None` for one whose length is under two, or past the end.
fn option_at(options: &[u8], at: usize) -> Option<(u8, usize)> {
    let kind = options[at];
    if kind == OPTION_END || kind == OPTION_NOP {
        return Some((kind, 1));
    }
    let length = usize::from(*options.get(at + 1)?);
    (length >= 2 && at + length <= options.len()).then_some((kind, length))
}

/// Every check of [`classify`] but the one of the `ipv6` option.
fn parse(packet: &[u8], rewrite: &IpRewrite) -> Result<Classified, Dropped> {
    match packet.first().map(|byte| byte >> 4) {
        Some(4) => read_ipv4(packet, rewrite),
        Some(6) => read_ipv6(packet, rewrite),
        _ => Err(Dropped::unread(Reason::Malformed)),
    }
}

fn read_ipv4(packet: &[u8], rewrite: &IpRewrite) -> Result<Classified, Dropped> {
    let ip = Ipv4Packet::new_checked(packet).map_err(|_| Dropped::unread(Reason::Malformed))?;
    // smoltcp takes a header of fewer than five words, and then reads the
    // TCP header from inside the IP header (design 5.3, H4)
    if ip.header_len() < 20 || !ip.verify_checksum() {
        return Err(Dropped::unread(Reason::Malformed));
    }
    // the header is sound: the addresses are the packet's to give from here on
    let (src, dst) = (ip.src_addr(), ip.dst_addr());
    if martian_v4(src, dst) {
        return Err(Dropped::between(Reason::Martian, src.into(), dst.into()));
    }
    let (src, dst) = (IpAddr::from(src), IpAddr::from(dst));
    let (len, header_len) = (usize::from(ip.total_len()), usize::from(ip.header_len()));
    if let Some(action) = rewrite.action(dst) {
        return Ok(Classified::Rewrite(Hit {
            action,
            src,
            dst,
            len,
            header_len,
        }));
    }
    if ip.more_frags() || ip.frag_offset() != 0 {
        return Err(Dropped::between(Reason::Fragment, src, dst));
    }
    transport(src, dst, ip.next_header(), ip.payload(), len).map(Classified::Passed)
}

fn read_ipv6(packet: &[u8], rewrite: &IpRewrite) -> Result<Classified, Dropped> {
    let ip = Ipv6Packet::new_checked(packet).map_err(|_| Dropped::unread(Reason::Malformed))?;
    // the header is sound: the addresses are the packet's to give from here on
    let (src, dst) = (ip.src_addr(), ip.dst_addr());
    if martian_v6(src, dst) {
        return Err(Dropped::between(Reason::Martian, src.into(), dst.into()));
    }
    let (src, dst) = (IpAddr::from(src), IpAddr::from(dst));
    let len = ip.total_len();
    if let Some(action) = rewrite.action(dst) {
        return Ok(Classified::Rewrite(Hit {
            action,
            src,
            dst,
            len,
            header_len: IPV6_HEADER_LEN,
        }));
    }
    if ip.next_header() == IpProtocol::Ipv6Frag {
        return Err(Dropped::between(Reason::Fragment, src, dst));
    }
    transport(src, dst, ip.next_header(), ip.payload(), len).map(Classified::Passed)
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
        (IpProtocol::Udp, _) => udp(src, dst, payload),
        (IpProtocol::Icmp, IpAddr::V4(_)) => icmp(src, dst, payload, ICMPV4_ECHO_REQUEST),
        (IpProtocol::Icmpv6, IpAddr::V6(_)) => icmp(src, dst, payload, ICMPV6_ECHO_REQUEST),
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

/// A UDP datagram: its header, and its checksum, which is the datagram's own
/// and the addresses' (the pseudo-header). A zero in its place is no checksum:
/// UDP over IPv4 may do without (RFC 768) and the datagram is taken as it is;
/// over IPv6 it may not (RFC 8200, 8.1), and the datagram is turned away.
/// smoltcp's `verify_checksum` takes a zero for right in both, hence the
/// check here.
fn udp(src: IpAddr, dst: IpAddr, payload: &[u8]) -> Result<Transport, Reason> {
    let udp = UdpPacket::new_checked(payload).map_err(|_| Reason::Malformed)?;
    let (src_port, dst_port) = (udp.src_port(), udp.dst_port());
    if src_port == 0 || dst_port == 0 {
        return Err(Reason::Malformed);
    }
    if udp.checksum() == 0 {
        if src.is_ipv6() {
            return Err(Reason::Malformed);
        }
    } else if !udp.verify_checksum(&src.into(), &dst.into()) {
        return Err(Reason::Malformed);
    }
    Ok(Transport::Udp { src_port, dst_port })
}

/// An ICMP message: only an echo request goes on, and then only with a
/// checksum that holds. ICMPv6's covers the addresses as UDP's does, ICMPv4's
/// the message alone. What is no echo request is dropped for that, without
/// the checksum being looked at.
fn icmp(src: IpAddr, dst: IpAddr, payload: &[u8], echo_request: u8) -> Result<Transport, Reason> {
    if payload.len() < ICMP_HEADER {
        return Err(Reason::Malformed);
    }
    if payload[0] != echo_request {
        return Err(Reason::Unsupported);
    }
    let holds = match (src, dst) {
        (IpAddr::V6(src), IpAddr::V6(dst)) => {
            Icmpv6Packet::new_unchecked(payload).verify_checksum(&src, &dst)
        }
        // an ICMP message of IPv4: the two addresses are of one family
        _ => Icmpv4Packet::new_unchecked(payload).verify_checksum(),
    };
    if !holds {
        return Err(Reason::Malformed);
    }
    Ok(Transport::Icmp {
        message_type: payload[0],
    })
}
