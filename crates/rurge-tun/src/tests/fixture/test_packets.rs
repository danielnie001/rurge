//! The packets a test makes, and the diverts by protocol: what `packets` makes
//! is read back, the fragments of a datagram are put together, and
//! `divert_udp` and `divert_icmp` take every packet of their kind.

use super::stacks;
use crate::packet::{Transport, classify};
use crate::testing::packets::{self, IcmpSeen, UdpSeen};
use crate::tests::support::{WITHIN, addr, eventually, ip_around, syn_v4, within};
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{Icmpv4Packet, Icmpv4Repr, Icmpv6Packet, Icmpv6Repr, Ipv4Packet};
use std::net::{IpAddr, SocketAddr};

/// The ends of the packets below, one pair for each family.
fn app_and_far_ends() -> [(SocketAddr, SocketAddr); 2] {
    [
        (addr("198.18.0.1:5000"), addr("192.0.2.10:7")),
        (addr("[fd00:6152::1]:5000"), addr("[2001:db8::10]:7")),
    ]
}

/// What `packets` makes is read back; the TUN's own checks let it through; and
/// a packet with a bit changed in anything a checksum covers is not read. (An
/// IPv6 header has no checksum, so its version, traffic class, flow label and
/// hop limit are covered by none.)
#[test]
fn the_packets_a_test_makes_are_read_back_and_a_damaged_one_is_not() {
    for (src, dst) in app_and_far_ends() {
        let packet = packets::udp(src, dst, b"hello");
        assert_eq!(
            packets::read_udp(&packet),
            Some(UdpSeen {
                src,
                dst,
                payload: b"hello".to_vec()
            })
        );
        let parsed = classify(&packet, true).expect("the TUN lets it through");
        assert_eq!(
            parsed.transport,
            Transport::Udp {
                src_port: src.port(),
                dst_port: dst.port()
            }
        );
        assert_eq!(packets::read_icmp(&packet), None, "a datagram is no ICMP");
        for at in 0..packet.len() {
            if src.is_ipv6() && [0, 1, 2, 3, 7].contains(&at) {
                continue;
            }
            let mut damaged = packet.clone();
            damaged[at] ^= 0x01;
            assert_eq!(packets::read_udp(&damaged), None, "byte {at} of {src}'s");
        }
        // an empty datagram, and a checksum that is left out
        let empty = packets::udp(src, dst, b"");
        assert_eq!(packets::read_udp(&empty).expect("a datagram").payload, b"");
        if src.is_ipv4() {
            let mut unsummed = packet.clone();
            unsummed[26..28].fill(0);
            assert_eq!(packets::read_udp(&unsummed), None, "no UDP checksum");
        }
    }
}

/// An echo reply of the addresses' family, made by hand.
fn echo_reply(src: IpAddr, dst: IpAddr, ident: u16, seq: u16, data: &[u8]) -> Vec<u8> {
    let caps = ChecksumCapabilities::default();
    match (src, dst) {
        (IpAddr::V4(_), IpAddr::V4(_)) => {
            let repr = Icmpv4Repr::EchoReply {
                ident,
                seq_no: seq,
                data,
            };
            let mut bytes = vec![0; repr.buffer_len()];
            repr.emit(&mut Icmpv4Packet::new_unchecked(&mut bytes[..]), &caps);
            ip_around(1, src, dst, &bytes)
        }
        (IpAddr::V6(from), IpAddr::V6(to)) => {
            let repr = Icmpv6Repr::EchoReply {
                ident,
                seq_no: seq,
                data,
            };
            let mut bytes = vec![0; repr.buffer_len()];
            repr.emit(
                &from,
                &to,
                &mut Icmpv6Packet::new_unchecked(&mut bytes[..]),
                &caps,
            );
            ip_around(58, src, dst, &bytes)
        }
        _ => panic!("the addresses of a packet are of one family"),
    }
}

/// An echo request made by `packets` is one the TUN lets through and no reply;
/// an echo reply is read, with its numbers and its data, and not once a bit of
/// what its checksums cover is changed.
#[test]
fn the_icmp_a_test_makes_and_reads_is_checked_both_ways() {
    for (src, dst) in app_and_far_ends() {
        let (src, dst) = (src.ip(), dst.ip());
        let request = packets::echo_request(src, dst, 0x1234, 7, b"data");
        let parsed = classify(&request, true).expect("the TUN lets it through");
        assert_eq!(
            parsed.transport,
            Transport::Icmp {
                message_type: if src.is_ipv4() { 8 } else { 128 }
            }
        );
        assert_eq!(packets::read_icmp(&request), None, "a request is no reply");
        let reply = echo_reply(dst, src, 0x1234, 7, b"data");
        assert_eq!(
            packets::read_icmp(&reply),
            Some(IcmpSeen::EchoReply {
                src: dst,
                dst: src,
                ident: 0x1234,
                seq: 7,
                payload: b"data".to_vec()
            })
        );
        assert_eq!(
            packets::read_udp(&reply),
            None,
            "an ICMP message is no datagram"
        );
        for at in 0..reply.len() {
            if src.is_ipv6() && [0, 1, 2, 3, 7].contains(&at) {
                continue;
            }
            let mut damaged = reply.clone();
            damaged[at] ^= 0x01;
            assert_eq!(packets::read_icmp(&damaged), None, "byte {at} for {src}");
        }
    }
}

/// The fragment of the IPv4 packet `whole` (a header of 20 bytes) that holds
/// the bytes `from` to `to` of what follows the header, made by hand.
fn fragment_v4(whole: &[u8], from: usize, to: usize, more: bool, ident: u16) -> Vec<u8> {
    let mut packet = whole[..20].to_vec();
    packet.extend(&whole[20 + from..20 + to]);
    let mut ip = Ipv4Packet::new_unchecked(&mut packet[..]);
    ip.set_total_len((20 + to - from) as u16);
    ip.set_ident(ident);
    ip.set_dont_frag(false);
    ip.set_more_frags(more);
    ip.set_frag_offset(from as u16);
    ip.fill_checksum();
    packet
}

/// The same for the IPv6 packet `whole`, with a fragment header after the base
/// header.
fn fragment_v6(whole: &[u8], from: usize, to: usize, more: bool, ident: u32) -> Vec<u8> {
    let mut packet = whole[..40].to_vec();
    let (protocol, length) = (packet[6], 8 + to - from);
    packet[6] = 44;
    packet[4..6].copy_from_slice(&(length as u16).to_be_bytes());
    packet.extend([protocol, 0]);
    packet.extend((((from / 8) as u16) << 3 | u16::from(more)).to_be_bytes());
    packet.extend(ident.to_be_bytes());
    packet.extend(&whole[40 + from..40 + to]);
    packet
}

/// `reassemble` puts the fragments of a datagram together in any order, and
/// gives nothing for anything short of all of them.
#[test]
fn reassemble_wants_every_fragment_of_one_datagram() {
    for (src, dst) in app_and_far_ends() {
        let whole = packets::udp(src, dst, &[1; 40]);
        let v4 = src.is_ipv4();
        let header = if v4 { 20 } else { 40 };
        assert_eq!(whole.len() - header, 48, "the datagram");
        let cut = |from, to, more, ident| {
            if v4 {
                fragment_v4(&whole, from, to, more, ident as u16)
            } else {
                fragment_v6(&whole, from, to, more, ident)
            }
        };
        let (first, second) = (cut(0, 24, true, 7), cut(24, 48, false, 7));
        let wanted = packets::read_udp(&whole);
        assert!(wanted.is_some());
        for fragments in [
            [first.clone(), second.clone()],
            [second.clone(), first.clone()],
        ] {
            let put_together = packets::reassemble(&fragments).expect("the datagram");
            assert_eq!(packets::read_udp(&put_together), wanted);
        }
        let none = |fragments: &[Vec<u8>], why: &str| {
            assert_eq!(packets::reassemble(fragments), None, "{why} ({src})");
        };
        none(&[], "no fragment");
        none(std::slice::from_ref(&first), "the first alone");
        none(std::slice::from_ref(&second), "the last alone");
        none(&[first.clone(), cut(32, 48, false, 7)], "a gap");
        none(&[first.clone(), cut(16, 48, false, 7)], "an overlap");
        none(&[first.clone(), cut(24, 48, false, 8)], "another datagram");
        none(
            &[first.clone(), cut(24, 48, true, 7)],
            "more after the last",
        );
        none(
            &[cut(0, 24, false, 7), second.clone()],
            "no more after the first",
        );
        none(&[first.clone(), first.clone()], "the first twice");
        if v4 {
            let mut damaged = second.clone();
            damaged[10] ^= 0x01;
            none(&[first.clone(), damaged], "a header that does not hold");
        }
        // one that is no fragment is a datagram that is whole already, for IPv4
        // (offset 0, no more); IPv6 has no such thing without a fragment header
        assert_eq!(
            packets::reassemble(std::slice::from_ref(&whole)).is_some(),
            v4
        );
    }
}

/// `divert_udp` and `divert_icmp` take every packet of their protocol before
/// the stack sees it, the fragments of a datagram too, in the order they came;
/// what they do not know goes to the stack.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn divert_udp_and_divert_icmp_take_every_packet_of_their_kind() {
    let (a, b) = stacks();
    let mut udp = b.divert_udp();
    let mut icmp = b.divert_icmp();
    let (src4, dst4) = (addr("198.18.0.1:5000"), addr("192.0.2.1:7"));
    let (src6, dst6) = (addr("[fd00:6152::1]:5000"), addr("[2001:db8::1]:7"));
    let whole4 = packets::udp(src4, dst4, &[1; 40]);
    let whole6 = packets::udp(src6, dst6, &[1; 40]);
    let for_udp = [
        whole4.clone(),
        whole6.clone(),
        fragment_v4(&whole4, 0, 24, true, 9),
        fragment_v4(&whole4, 24, 48, false, 9),
        fragment_v6(&whole6, 0, 24, true, 9),
        fragment_v6(&whole6, 24, 48, false, 9),
    ];
    let for_icmp = [
        packets::echo_request(src4.ip(), dst4.ip(), 1, 1, b"x"),
        packets::echo_request(src6.ip(), dst6.ip(), 1, 1, b"x"),
    ];
    for packet in for_udp.iter().chain(&for_icmp) {
        a.inject(packet.clone());
    }
    for packet in for_udp {
        assert_eq!(
            within("a UDP packet", udp.recv()).await,
            Some(packet),
            "unchanged and in order"
        );
    }
    for packet in for_icmp {
        assert_eq!(within("an ICMP packet", icmp.recv()).await, Some(packet));
    }
    // a SYN is neither: the stack of b takes it, and answers with a reset
    a.inject(syn_v4(src4, addr("192.0.2.1:9999")));
    eventually("b's stack to answer a SYN to a closed port", WITHIN, || {
        b.packets() == (1, 1)
    })
    .await;
    assert!(udp.try_recv().is_err() && icmp.try_recv().is_err());
}
