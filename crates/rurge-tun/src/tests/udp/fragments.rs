//! Replies larger than the device's MTU come back in the fragments of their
//! family; and the pure parts that cut a datagram into fragments and check
//! them.

use super::{APP4, APP6, FAR4, FAR6, lab, target_of};
use crate::fragment::{Ids, ip_packets};
use crate::testing::UdpScript;
use crate::testing::packets::{self, UdpSeen};
use crate::tests::support::{addr, udp_bytes};
use crate::{Counters, TunOptions};
use smoltcp::wire::{IpProtocol, Ipv4Packet, Ipv6Packet};

// ---- replies larger than the MTU

/// The identification and the checks of the fragments of one datagram of
/// `datagram` bytes in IPv4 packets of at most 1500 bytes: all with a header
/// that holds and no options, the identification the same, DF cleared, "more
/// fragments" on all but the last, offsets that begin where the last ended and
/// are multiples of 8, and the data of all but the last a multiple of 8 bytes.
fn check_fragments_v4(fragments: &[Vec<u8>], datagram: usize) -> u16 {
    let ident = Ipv4Packet::new_checked(&fragments[0][..])
        .expect("an IPv4 packet")
        .ident();
    let mut next = 0;
    for (index, packet) in fragments.iter().enumerate() {
        assert!(packet.len() <= 1500, "fragment {index} fits the MTU");
        let ip = Ipv4Packet::new_checked(&packet[..]).expect("a whole IPv4 packet");
        assert!(ip.verify_checksum(), "fragment {index}'s header holds");
        assert_eq!(ip.header_len(), 20, "fragment {index} has no options");
        assert_eq!(ip.next_header(), IpProtocol::Udp);
        assert_eq!(ip.hop_limit(), 64);
        assert_eq!(ip.ident(), ident, "one identification");
        assert!(!ip.dont_frag(), "fragment {index} may be fragmented");
        assert_eq!(usize::from(ip.frag_offset()), next, "fragment {index}");
        assert_eq!(next % 8, 0);
        let last = index + 1 == fragments.len();
        assert_eq!(ip.more_frags(), !last, "fragment {index}");
        if !last {
            assert_eq!(ip.payload().len() % 8, 0, "fragment {index} but the last");
        }
        next += ip.payload().len();
    }
    assert_eq!(next, datagram, "the fragments hold the datagram");
    ident
}

/// The same for IPv6: the fragment header after the base header, whose next
/// header is the datagram's protocol, and an offset in units of 8 bytes.
fn check_fragments_v6(fragments: &[Vec<u8>], datagram: usize) -> u32 {
    let ident_of = |packet: &Vec<u8>| {
        let ip = Ipv6Packet::new_checked(&packet[..]).expect("an IPv6 packet");
        let payload = ip.payload();
        u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]])
    };
    let ident = ident_of(&fragments[0]);
    let mut next = 0;
    for (index, packet) in fragments.iter().enumerate() {
        assert!(packet.len() <= 1500, "fragment {index} fits the MTU");
        let ip = Ipv6Packet::new_checked(&packet[..]).expect("a whole IPv6 packet");
        assert_eq!(ip.next_header(), IpProtocol::Ipv6Frag);
        assert_eq!(ip.hop_limit(), 64);
        let payload = ip.payload();
        assert_eq!(IpProtocol::from(payload[0]), IpProtocol::Udp);
        assert_eq!(payload[1], 0, "the reserved byte");
        let field = u16::from_be_bytes([payload[2], payload[3]]);
        assert_eq!(usize::from(field >> 3) * 8, next, "fragment {index}");
        let last = index + 1 == fragments.len();
        assert_eq!(field & 1 != 0, !last, "fragment {index}");
        assert_eq!(field & 0b110, 0, "the reserved bits");
        assert_eq!(ident_of(packet), ident, "one identification");
        let data = &payload[8..];
        if !last {
            assert_eq!(data.len() % 8, 0, "fragment {index} but the last");
        }
        next += data.len();
    }
    assert_eq!(next, datagram, "the fragments hold the datagram");
    ident
}

/// A reply that does not fit the device's MTU comes back in the fragments of
/// its family, whole once the app has put them together, and each reply is a
/// datagram of its own to the app's stack: another identification.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_larger_than_the_mtu_is_fragmented() {
    let mut lab = lab(TunOptions::default());
    let big: Vec<u8> = (0..4000).map(|i| (i % 251) as u8).collect();
    for (app, dst) in [(addr(APP4), addr(FAR4)), (addr(APP6), addr(FAR6))] {
        lab.dialer()
            .script_udp(target_of(dst), UdpScript::Reply(big.clone()));
        let mut idents = Vec::new();
        for ask in [&b"the big one"[..], b"and again"] {
            lab.send(app, dst, ask);
            // the datagram is 4008 bytes: 3 fragments in either family
            let fragments = lab.packets(3).await;
            let ident = if app.is_ipv4() {
                u32::from(check_fragments_v4(&fragments, 4008))
            } else {
                check_fragments_v6(&fragments, 4008)
            };
            idents.push(ident);
            let whole = packets::reassemble(&fragments).expect("the fragments are one datagram");
            assert_eq!(
                packets::read_udp(&whole),
                Some(UdpSeen {
                    src: dst,
                    dst: app,
                    payload: big.clone()
                }),
                "{app}"
            );
        }
        assert_ne!(
            idents[0], idents[1],
            "another datagram, another identification"
        );
    }
    assert_eq!(lab.counters(), Counters::default());
}

/// The longest datagram a reply can be is the longest the packets can hold; a
/// longer one is refused, not written wrong.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_longest_datagram_is_carried_and_a_longer_one_refused() {
    let mut lab = lab(TunOptions::default());
    // IPv4's total length is 65535 bytes with a header of 20 and the UDP header
    // of 8; IPv6's payload length is 65535 bytes with the UDP header in it
    for (app, far, longest) in [
        (addr(APP4), addr(FAR4), 65507),
        (addr(APP6), addr(FAR6), 65527),
    ] {
        lab.send(app, far, b"hello");
        lab.associations_open(if app.is_ipv4() { 1 } else { 2 })
            .await;
        let from = target_of(far);
        lab.dialer()
            .send_to_app(app, from.clone(), Some(from.clone()), vec![7; longest + 1])
            .await
            .expect_err("a datagram that is too long to carry");
        lab.dialer()
            .send_to_app(app, from.clone(), Some(from), vec![7; longest])
            .await
            .expect("the longest datagram");
        // every fragment of it is in the app's hands, and they are one datagram
        let mut fragments = vec![lab.packet().await];
        let whole = loop {
            if let Some(whole) = packets::reassemble(&fragments) {
                break whole;
            }
            fragments.push(lab.packet().await);
        };
        let seen = packets::read_udp(&whole).expect("a datagram with right checksums");
        assert_eq!(seen.payload.len(), longest, "{app}");
        assert!(seen.payload.iter().all(|byte| *byte == 7));
    }
}

// ---- the pure parts

/// `ip_packets` of a UDP datagram: one packet when it fits, else fragments that
/// put together are the datagram, the other way round as well, and each call
/// has its own identification.
#[test]
fn fragments_reassemble_to_the_datagram() {
    let ids = Ids::new();
    for (src, dst) in [
        (addr("192.0.2.10:7"), addr("198.18.0.1:5000")),
        (addr("[2001:db8::10]:7"), addr("[fd00:6152::1]:5000")),
    ] {
        let header = if src.is_ipv4() { 20 } else { 40 };
        for mtu in [1280, 1500, 9000] {
            // what a single packet holds of UDP, and a few sizes around it
            let fits = mtu - header - 8;
            let longest = if src.is_ipv4() { 65507 } else { 65527 };
            for length in [
                0,
                1,
                1000,
                fits - 1,
                fits,
                fits + 1,
                2 * fits,
                3 * fits + 5,
                longest,
            ] {
                let payload: Vec<u8> = (0..length).map(|i| (i % 253) as u8).collect();
                let datagram = udp_bytes(src, dst, &payload);
                let packets = ip_packets(src.ip(), dst.ip(), IpProtocol::Udp, &datagram, mtu, &ids);
                let wanted = UdpSeen {
                    src,
                    dst,
                    payload: payload.clone(),
                };
                assert!(
                    packets.iter().all(|packet| packet.len() <= mtu),
                    "{length} bytes in {mtu}: no packet is over the MTU"
                );
                if header + datagram.len() <= mtu {
                    assert_eq!(packets.len(), 1, "{length} bytes in {mtu} fit");
                    assert_eq!(packets::read_udp(&packets[0]), Some(wanted));
                    continue;
                }
                assert!(packets.len() > 1, "{length} bytes in {mtu} do not fit");
                if src.is_ipv4() {
                    check_fragments_v4_in(&packets, datagram.len(), mtu);
                } else {
                    check_fragments_v6_in(&packets, datagram.len(), mtu);
                }
                let whole = packets::reassemble(&packets).expect("one datagram");
                assert_eq!(packets::read_udp(&whole), Some(wanted.clone()));
                // in any order
                let mut backwards = packets.clone();
                backwards.reverse();
                assert_eq!(packets::reassemble(&backwards), Some(whole));
                // a fragment missing is no datagram
                let mut short = packets;
                short.remove(short.len() / 2);
                assert_eq!(packets::reassemble(&short), None);
            }
        }
        // each call has an identification of its own
        let datagram = udp_bytes(src, dst, &[0; 3000]);
        let first = ip_packets(src.ip(), dst.ip(), IpProtocol::Udp, &datagram, 1500, &ids);
        let second = ip_packets(src.ip(), dst.ip(), IpProtocol::Udp, &datagram, 1500, &ids);
        if src.is_ipv4() {
            assert_ne!(
                Ipv4Packet::new_unchecked(&first[0][..]).ident(),
                Ipv4Packet::new_unchecked(&second[0][..]).ident()
            );
        } else {
            assert_ne!(first[0][44..48], second[0][44..48]);
        }
    }
}

/// `check_fragments_v4` for a device of any MTU.
fn check_fragments_v4_in(fragments: &[Vec<u8>], datagram: usize, mtu: usize) {
    let mut next = 0;
    for (index, packet) in fragments.iter().enumerate() {
        let ip = Ipv4Packet::new_checked(&packet[..]).expect("a whole IPv4 packet");
        assert!(ip.verify_checksum() && !ip.dont_frag(), "fragment {index}");
        assert_eq!(usize::from(ip.frag_offset()), next);
        assert_eq!(ip.hop_limit(), 64);
        let last = index + 1 == fragments.len();
        assert_eq!(ip.more_frags(), !last);
        if !last {
            // as much as fits, in a multiple of 8
            assert_eq!(ip.payload().len(), (mtu - 20) / 8 * 8, "fragment {index}");
        }
        next += ip.payload().len();
    }
    assert_eq!(next, datagram);
}

/// The same for IPv6.
fn check_fragments_v6_in(fragments: &[Vec<u8>], datagram: usize, mtu: usize) {
    let mut next = 0;
    for (index, packet) in fragments.iter().enumerate() {
        let ip = Ipv6Packet::new_checked(&packet[..]).expect("a whole IPv6 packet");
        assert_eq!(ip.next_header(), IpProtocol::Ipv6Frag);
        assert_eq!(ip.hop_limit(), 64);
        let payload = ip.payload();
        let field = u16::from_be_bytes([payload[2], payload[3]]);
        assert_eq!(usize::from(field >> 3) * 8, next, "fragment {index}");
        let last = index + 1 == fragments.len();
        assert_eq!(field & 1 != 0, !last);
        if !last {
            assert_eq!(
                payload.len() - 8,
                (mtu - 40 - 8) / 8 * 8,
                "fragment {index}"
            );
        }
        next += payload.len() - 8;
    }
    assert_eq!(next, datagram);
}
