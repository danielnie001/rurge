//! What the TUN reads of an IP packet before it decides where the packet goes.

use crate::tcp::Tuple;
use smoltcp::wire::{IpProtocol, Ipv4Packet, Ipv6Packet, TcpPacket};
use std::net::{IpAddr, SocketAddr};

/// What a packet is, as far as the way to it goes.
pub(crate) enum Kind {
    Tcp(Segment),
    /// An IP packet of another protocol.
    Other,
    /// Not an IPv4 or IPv6 packet whose headers can be read.
    Malformed,
}

/// The ends of a TCP segment and the flags that say what it opens or ends.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Segment {
    /// From the app (the packet's source) to where it went.
    pub(crate) tuple: Tuple,
    pub(crate) syn: bool,
    pub(crate) ack: bool,
    pub(crate) rst: bool,
}

pub(crate) fn kind(packet: &[u8]) -> Kind {
    match packet.first().map(|byte| byte >> 4) {
        Some(4) => match Ipv4Packet::new_checked(packet) {
            Ok(ip) => transport(
                IpAddr::V4(ip.src_addr()),
                IpAddr::V4(ip.dst_addr()),
                ip.next_header(),
                ip.payload(),
            ),
            Err(_) => Kind::Malformed,
        },
        Some(6) => match Ipv6Packet::new_checked(packet) {
            Ok(ip) => transport(
                IpAddr::V6(ip.src_addr()),
                IpAddr::V6(ip.dst_addr()),
                ip.next_header(),
                ip.payload(),
            ),
            Err(_) => Kind::Malformed,
        },
        _ => Kind::Malformed,
    }
}

fn transport(src: IpAddr, dst: IpAddr, protocol: IpProtocol, payload: &[u8]) -> Kind {
    if protocol != IpProtocol::Tcp {
        return Kind::Other;
    }
    match TcpPacket::new_checked(payload) {
        Ok(tcp) => Kind::Tcp(Segment {
            tuple: Tuple {
                src: SocketAddr::new(src, tcp.src_port()),
                dst: SocketAddr::new(dst, tcp.dst_port()),
            },
            syn: tcp.syn(),
            ack: tcp.ack(),
            rst: tcp.rst(),
        }),
        Err(_) => Kind::Malformed,
    }
}
