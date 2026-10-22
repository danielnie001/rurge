//! The packets of a test: what an app sends through the adapter, and what the
//! adapter sends back, made and read with `smoltcp::wire` and nothing of the
//! TUN's own code, so that a mistake in one is not made again in the other.
//! Every packet that is made has right checksums, and every reader checks them
//! and answers `None` for a packet whose checksum does not hold.

use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    Icmpv4Message, Icmpv4Packet, Icmpv4Repr, Icmpv6Message, Icmpv6Packet, Icmpv6Repr, IpProtocol,
    Ipv4Packet, Ipv4Repr, Ipv6Packet, Ipv6Repr, UdpPacket, UdpRepr,
};
use std::net::{IpAddr, SocketAddr};

/// The time to live and the hop limit of what an app sends.
const HOP_LIMIT: u8 = 64;

/// The ICMPv4 code of "communication administratively prohibited" (type 3),
/// and the ICMPv6 code of "communication with destination administratively
/// prohibited" (type 1).
const PROHIBITED_V4: u8 = 13;
const PROHIBITED_V6: u8 = 1;

/// The length of the header of an IPv6 packet, and of its fragment header.
const IPV6_HEADER: usize = 40;
const IPV6_FRAGMENT: usize = 8;

/// A UDP datagram from `src` to `dst` in an IP packet, the checksums right.
/// The two endpoints are of one family.
pub fn udp(src: SocketAddr, dst: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let repr = UdpRepr {
        src_port: src.port(),
        dst_port: dst.port(),
    };
    let mut bytes = vec![0; 8 + payload.len()];
    repr.emit(
        &mut UdpPacket::new_unchecked(&mut bytes[..]),
        &src.ip().into(),
        &dst.ip().into(),
        payload.len(),
        |buffer| buffer.copy_from_slice(payload),
        &ChecksumCapabilities::default(),
    );
    ip_around(IpProtocol::Udp, src.ip(), dst.ip(), &bytes)
}

/// An ICMP echo request from `src` to `dst` (ICMPv4 or ICMPv6 as the addresses
/// are of the one family or the other), the checksums right.
pub fn echo_request(src: IpAddr, dst: IpAddr, ident: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let caps = ChecksumCapabilities::default();
    match (src, dst) {
        (IpAddr::V4(_), IpAddr::V4(_)) => {
            let repr = Icmpv4Repr::EchoRequest {
                ident,
                seq_no: seq,
                data: payload,
            };
            let mut bytes = vec![0; repr.buffer_len()];
            repr.emit(&mut Icmpv4Packet::new_unchecked(&mut bytes[..]), &caps);
            ip_around(IpProtocol::Icmp, src, dst, &bytes)
        }
        (IpAddr::V6(from), IpAddr::V6(to)) => {
            let repr = Icmpv6Repr::EchoRequest {
                ident,
                seq_no: seq,
                data: payload,
            };
            let mut bytes = vec![0; repr.buffer_len()];
            repr.emit(
                &from,
                &to,
                &mut Icmpv6Packet::new_unchecked(&mut bytes[..]),
                &caps,
            );
            ip_around(IpProtocol::Icmpv6, src, dst, &bytes)
        }
        _ => panic!("the addresses of a packet are of one family"),
    }
}

/// An IP packet of the addresses' family around `payload`, with a header
/// that has no options, a time to live of 64 and a right checksum.
fn ip_around(protocol: IpProtocol, src: IpAddr, dst: IpAddr, payload: &[u8]) -> Vec<u8> {
    match (src, dst) {
        (IpAddr::V4(src_addr), IpAddr::V4(dst_addr)) => {
            let ip = Ipv4Repr {
                src_addr,
                dst_addr,
                next_header: protocol,
                payload_len: payload.len(),
                hop_limit: HOP_LIMIT,
            };
            let mut packet = vec![0; ip.buffer_len() + payload.len()];
            ip.emit(
                &mut Ipv4Packet::new_unchecked(&mut packet[..]),
                &ChecksumCapabilities::default(),
            );
            packet[ip.buffer_len()..].copy_from_slice(payload);
            packet
        }
        (IpAddr::V6(src_addr), IpAddr::V6(dst_addr)) => {
            let ip = Ipv6Repr {
                src_addr,
                dst_addr,
                next_header: protocol,
                payload_len: payload.len(),
                hop_limit: HOP_LIMIT,
            };
            let mut packet = vec![0; ip.buffer_len() + payload.len()];
            ip.emit(&mut Ipv6Packet::new_unchecked(&mut packet[..]));
            packet[ip.buffer_len()..].copy_from_slice(payload);
            packet
        }
        _ => panic!("the addresses of a packet are of one family"),
    }
}

/// A UDP datagram as an app reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UdpSeen {
    pub src: SocketAddr,
    pub dst: SocketAddr,
    pub payload: Vec<u8>,
}

/// The UDP datagram a whole IP packet carries. `None` for anything else: a
/// fragment, another protocol, a packet that is cut short, an IPv4 header
/// whose checksum does not hold, and a datagram whose checksum does not hold or
/// is left out (UDP over IPv4 may leave it out, but nothing the adapter writes
/// does).
pub fn read_udp(packet: &[u8]) -> Option<UdpSeen> {
    let (src, dst, protocol, payload) = read_ip(packet)?;
    if protocol != IpProtocol::Udp {
        return None;
    }
    let udp = UdpPacket::new_checked(payload).ok()?;
    if udp.checksum() == 0 || !udp.verify_checksum(&src.into(), &dst.into()) {
        return None;
    }
    Some(UdpSeen {
        src: SocketAddr::new(src, udp.src_port()),
        dst: SocketAddr::new(dst, udp.dst_port()),
        payload: udp.payload().to_vec(),
    })
}

/// An ICMP message as an app reads it, of the two kinds the adapter sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IcmpSeen {
    EchoReply {
        src: IpAddr,
        dst: IpAddr,
        ident: u16,
        seq: u16,
        payload: Vec<u8>,
    },
    /// Destination unreachable, "administratively prohibited": `quoted` is
    /// what the message carries of the packet it answers.
    Prohibited {
        src: IpAddr,
        dst: IpAddr,
        quoted: Vec<u8>,
    },
}

/// The echo reply or the "administratively prohibited" message a whole IP
/// packet carries. `None` for any other message, any other protocol, a
/// fragment, and a packet whose checksums do not hold.
pub fn read_icmp(packet: &[u8]) -> Option<IcmpSeen> {
    let (src, dst, protocol, payload) = read_ip(packet)?;
    match (protocol, src, dst) {
        (IpProtocol::Icmp, IpAddr::V4(_), IpAddr::V4(_)) => {
            let icmp = Icmpv4Packet::new_checked(payload).ok()?;
            if !icmp.verify_checksum() {
                return None;
            }
            match (icmp.msg_type(), icmp.msg_code()) {
                (Icmpv4Message::EchoReply, _) => Some(IcmpSeen::EchoReply {
                    src,
                    dst,
                    ident: icmp.echo_ident(),
                    seq: icmp.echo_seq_no(),
                    payload: icmp.data().to_vec(),
                }),
                (Icmpv4Message::DstUnreachable, PROHIBITED_V4) => Some(IcmpSeen::Prohibited {
                    src,
                    dst,
                    quoted: icmp.data().to_vec(),
                }),
                _ => None,
            }
        }
        (IpProtocol::Icmpv6, IpAddr::V6(from), IpAddr::V6(to)) => {
            let icmp = Icmpv6Packet::new_checked(payload).ok()?;
            if !icmp.verify_checksum(&from, &to) {
                return None;
            }
            match (icmp.msg_type(), icmp.msg_code()) {
                (Icmpv6Message::EchoReply, _) => Some(IcmpSeen::EchoReply {
                    src,
                    dst,
                    ident: icmp.echo_ident(),
                    seq: icmp.echo_seq_no(),
                    payload: icmp.payload().to_vec(),
                }),
                (Icmpv6Message::DstUnreachable, PROHIBITED_V6) => Some(IcmpSeen::Prohibited {
                    src,
                    dst,
                    quoted: icmp.payload().to_vec(),
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The ends, the protocol and the payload of a whole IP packet that is no
/// fragment, once its header has been looked at: for IPv4 its length and its
/// checksum.
fn read_ip(packet: &[u8]) -> Option<(IpAddr, IpAddr, IpProtocol, &[u8])> {
    match packet.first()? >> 4 {
        4 => {
            let ip = Ipv4Packet::new_checked(packet).ok()?;
            if !ip.verify_checksum() || ip.more_frags() || ip.frag_offset() != 0 {
                return None;
            }
            Some((
                ip.src_addr().into(),
                ip.dst_addr().into(),
                ip.next_header(),
                ip.payload(),
            ))
        }
        6 => {
            let ip = Ipv6Packet::new_checked(packet).ok()?;
            Some((
                ip.src_addr().into(),
                ip.dst_addr().into(),
                ip.next_header(),
                ip.payload(),
            ))
        }
        _ => None,
    }
}

/// What one fragment says of itself and carries.
struct Fragment<'a> {
    src: IpAddr,
    dst: IpAddr,
    protocol: IpProtocol,
    ident: u32,
    /// Where its data begins in the datagram, in bytes.
    offset: usize,
    more: bool,
    data: &'a [u8],
}

impl<'a> Fragment<'a> {
    /// A fragment of IPv4 (its header checksum has to hold) or of IPv6 (the
    /// base header's next header is the fragment header).
    fn read(packet: &'a [u8]) -> Option<Fragment<'a>> {
        match packet.first()? >> 4 {
            4 => {
                let ip = Ipv4Packet::new_checked(packet).ok()?;
                if !ip.verify_checksum() {
                    return None;
                }
                Some(Fragment {
                    src: ip.src_addr().into(),
                    dst: ip.dst_addr().into(),
                    protocol: ip.next_header(),
                    ident: u32::from(ip.ident()),
                    offset: usize::from(ip.frag_offset()),
                    more: ip.more_frags(),
                    data: ip.payload(),
                })
            }
            6 => {
                let ip = Ipv6Packet::new_checked(packet).ok()?;
                if ip.next_header() != IpProtocol::Ipv6Frag {
                    return None;
                }
                let payload = ip.payload();
                if payload.len() < IPV6_FRAGMENT {
                    return None;
                }
                // next header, reserved, offset (in units of 8 bytes) and the
                // more fragments bit, identification
                let field = u16::from_be_bytes([payload[2], payload[3]]);
                Some(Fragment {
                    src: ip.src_addr().into(),
                    dst: ip.dst_addr().into(),
                    protocol: IpProtocol::from(payload[0]),
                    ident: u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]),
                    offset: usize::from(field >> 3) * 8,
                    more: field & 1 != 0,
                    data: &payload[IPV6_FRAGMENT..],
                })
            }
            _ => None,
        }
    }
}

/// The whole IP packet that `fragments` are the fragments of, given in any
/// order, with the header of the first. `None` unless they are all of one
/// datagram (the same ends, protocol and identification), each fragment's
/// header holds, and between them they cover the datagram from its first byte
/// to its last, with no gap and no overlap, and "more fragments" set on all
/// but the last.
pub fn reassemble(fragments: &[Vec<u8>]) -> Option<Vec<u8>> {
    let mut parts = fragments
        .iter()
        .map(|packet| Fragment::read(packet).map(|part| (part, packet)))
        .collect::<Option<Vec<_>>>()?;
    parts.sort_by_key(|(part, _)| part.offset);
    let (first, first_packet) = parts.first()?;
    let mut data = Vec::new();
    for (index, (part, _)) in parts.iter().enumerate() {
        let one_datagram = (part.src, part.dst, part.protocol, part.ident)
            == (first.src, first.dst, first.protocol, first.ident);
        let last = index + 1 == parts.len();
        if !one_datagram || part.offset != data.len() || part.more == last {
            return None;
        }
        data.extend_from_slice(part.data);
    }
    Some(match first.src {
        IpAddr::V4(_) => {
            let header = usize::from(first_packet[0] & 15) * 4;
            let mut packet = first_packet[..header].to_vec();
            packet.extend_from_slice(&data);
            let mut ip = Ipv4Packet::new_unchecked(&mut packet[..]);
            ip.set_total_len((header + data.len()) as u16);
            ip.set_more_frags(false);
            ip.set_frag_offset(0);
            ip.fill_checksum();
            packet
        }
        IpAddr::V6(_) => {
            let mut packet = first_packet[..IPV6_HEADER].to_vec();
            packet.extend_from_slice(&data);
            let mut ip = Ipv6Packet::new_unchecked(&mut packet[..]);
            ip.set_payload_len(data.len() as u16);
            ip.set_next_header(first.protocol);
            packet
        }
    })
}
