//! The IP packets the TUN writes around a transport payload it made (phase 3
//! M3 design 7.4): one packet when the payload fits the device's MTU, and the
//! fragments of the payload when it does not. The app's own stack puts them
//! together, as it does for any network card. (What comes in as a fragment is
//! dropped, `packet::classify`; the apps' stacks do not send them, and the TUN
//! does not put them together.)

use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    IpProtocol, Ipv4Packet, Ipv4Repr, Ipv6FragmentHeader, Ipv6FragmentRepr, Ipv6Packet, Ipv6Repr,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicU32, Ordering};

/// The time to live and the hop limit of the packets the TUN makes: what
/// operating systems send by default.
const HOP_LIMIT: u8 = 64;
/// The headers of the packets the TUN makes: IPv4's has no options, IPv6's
/// fragment header goes after its base header.
const IPV4_HEADER: usize = 20;
const IPV6_HEADER: usize = 40;
const IPV6_FRAGMENT_HEADER: usize = 8;

/// The identifications of the fragments the TUN writes: one counter for the
/// whole TUN, begun at a random number, so that two datagrams that are in the
/// air at once have different ones (an app's stack that was given the pieces of
/// both with one would put the wrong pieces together). IPv4's field has 16
/// bits and takes the low ones; IPv6's has 32.
pub(crate) struct Ids(AtomicU32);

impl Ids {
    pub(crate) fn new() -> Ids {
        // a counter that begins anywhere does; one that begins at random does
        // not begin where the last run of the TUN did
        Ids(AtomicU32::new(getrandom::u32().unwrap_or(0x5eed)))
    }

    fn next(&self) -> u32 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

/// What a fragment says of where it is in its datagram.
struct Fragment {
    ident: u32,
    /// Where its data begins in the datagram, in bytes: a multiple of 8.
    offset: usize,
    /// Whether more of the datagram follows it.
    more: bool,
}

/// The IP packet that carries `transport` (a UDP datagram, an ICMP message)
/// from `src` to `dst`, whole: nothing in it says it is part of a larger
/// datagram. The two addresses are of one family.
pub(crate) fn ip_packet(
    src: IpAddr,
    dst: IpAddr,
    protocol: IpProtocol,
    transport: &[u8],
) -> Vec<u8> {
    match (src, dst) {
        (IpAddr::V4(src), IpAddr::V4(dst)) => ipv4(src, dst, protocol, transport, None),
        (IpAddr::V6(src), IpAddr::V6(dst)) => ipv6(src, dst, protocol, transport),
        _ => unreachable!("the two addresses of a packet are of one family"),
    }
}

/// The IP packets that carry `transport` (its checksum already worked out)
/// from `src` to `dst`: one if it fits the `mtu` with the IP header, and else
/// its fragments, in order, each at most `mtu` long. They have the IPv4 header
/// or the IPv6 fragment header, with an identification from `ids` the same in
/// all of them, and pieces of the data that are multiples of 8 bytes but for
/// the last. The two addresses are of one family, `mtu` is at least 1280 (as
/// `Tun::start` has it), and `transport` is at most as long as a packet of the
/// family can carry (65515 bytes for IPv4, 65535 for IPv6).
pub(crate) fn ip_packets(
    src: IpAddr,
    dst: IpAddr,
    protocol: IpProtocol,
    transport: &[u8],
    mtu: usize,
    ids: &Ids,
) -> Vec<Vec<u8>> {
    let header = if src.is_ipv4() {
        IPV4_HEADER
    } else {
        IPV6_HEADER
    };
    if header + transport.len() <= mtu {
        return vec![ip_packet(src, dst, protocol, transport)];
    }
    // the data of each fragment but the last is a multiple of 8 bytes, since
    // the offset of the next is counted in units of 8
    let room = if src.is_ipv4() {
        mtu.saturating_sub(IPV4_HEADER)
    } else {
        mtu.saturating_sub(IPV6_HEADER + IPV6_FRAGMENT_HEADER)
    };
    let piece = (room / 8 * 8).max(8);
    let ident = ids.next();
    let mut fragments = Vec::with_capacity(transport.len().div_ceil(piece));
    let mut offset = 0;
    for data in transport.chunks(piece) {
        let fragment = Fragment {
            ident,
            offset,
            more: offset + data.len() < transport.len(),
        };
        fragments.push(match (src, dst) {
            (IpAddr::V4(src), IpAddr::V4(dst)) => ipv4(src, dst, protocol, data, Some(fragment)),
            (IpAddr::V6(src), IpAddr::V6(dst)) => {
                ipv6_fragment(src, dst, protocol, data, &fragment)
            }
            _ => unreachable!("the two addresses of a packet are of one family"),
        });
        offset += data.len();
    }
    fragments
}

/// An IPv4 packet around `data`, which is the whole transport payload, or one
/// piece of it if `fragment` says where.
fn ipv4(
    src: Ipv4Addr,
    dst: Ipv4Addr,
    protocol: IpProtocol,
    data: &[u8],
    fragment: Option<Fragment>,
) -> Vec<u8> {
    let repr = Ipv4Repr {
        src_addr: src,
        dst_addr: dst,
        next_header: protocol,
        payload_len: data.len(),
        hop_limit: HOP_LIMIT,
    };
    let mut packet = vec![0; IPV4_HEADER + data.len()];
    // "don't fragment", an identification of 0 and a right checksum: as a
    // packet that is whole should be
    repr.emit(
        &mut Ipv4Packet::new_unchecked(&mut packet[..]),
        &ChecksumCapabilities::default(),
    );
    packet[IPV4_HEADER..].copy_from_slice(data);
    if let Some(fragment) = fragment {
        let mut ip = Ipv4Packet::new_unchecked(&mut packet[..]);
        // the 16 bits the field has
        ip.set_ident(fragment.ident as u16);
        ip.set_dont_frag(false);
        ip.set_more_frags(fragment.more);
        ip.set_frag_offset(fragment.offset as u16);
        ip.fill_checksum();
    }
    packet
}

fn ipv6(src: Ipv6Addr, dst: Ipv6Addr, protocol: IpProtocol, data: &[u8]) -> Vec<u8> {
    let repr = Ipv6Repr {
        src_addr: src,
        dst_addr: dst,
        next_header: protocol,
        payload_len: data.len(),
        hop_limit: HOP_LIMIT,
    };
    let mut packet = vec![0; IPV6_HEADER + data.len()];
    repr.emit(&mut Ipv6Packet::new_unchecked(&mut packet[..]));
    packet[IPV6_HEADER..].copy_from_slice(data);
    packet
}

/// An IPv6 packet whose payload is a fragment header and then `data`, a piece
/// of the transport payload: the header's own next header is the payload's
/// protocol, and the base header's is the fragment header.
fn ipv6_fragment(
    src: Ipv6Addr,
    dst: Ipv6Addr,
    protocol: IpProtocol,
    data: &[u8],
    fragment: &Fragment,
) -> Vec<u8> {
    let repr = Ipv6Repr {
        src_addr: src,
        dst_addr: dst,
        next_header: IpProtocol::Ipv6Frag,
        payload_len: IPV6_FRAGMENT_HEADER + data.len(),
        hop_limit: HOP_LIMIT,
    };
    let mut packet = vec![0; IPV6_HEADER + IPV6_FRAGMENT_HEADER + data.len()];
    repr.emit(&mut Ipv6Packet::new_unchecked(&mut packet[..]));
    // the next header and a reserved byte, and then smoltcp's view of the rest
    // of the fragment header: the offset (in units of 8 bytes), the flag and
    // the identification
    packet[IPV6_HEADER] = u8::from(protocol);
    Ipv6FragmentRepr {
        frag_offset: (fragment.offset / 8) as u16,
        more_frags: fragment.more,
        ident: fragment.ident,
    }
    .emit(&mut Ipv6FragmentHeader::new_unchecked(
        &mut packet[IPV6_HEADER + 2..IPV6_HEADER + IPV6_FRAGMENT_HEADER],
    ));
    packet[IPV6_HEADER + IPV6_FRAGMENT_HEADER..].copy_from_slice(data);
    packet
}
