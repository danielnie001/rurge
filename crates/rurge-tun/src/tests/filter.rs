//! What the TUN does with a packet before any stack sees it (phase 3 M3
//! design 4.4). First `classify` on its own, with packets built field by
//! field so that a test breaks exactly one thing in one; then the same
//! checks through the TUN, with what they count and what they say.

use super::log_capture::LogCapture;
use super::support::{
    Rig, WITHIN, addr, echo_bytes, echo_packet, eventually, ip_around, ipv4_packet, ipv6_packet,
    refresh, rig, sock, syn_v4, syn_v6, tcp_bytes, tcp_packet, tcp_repr, udp_bytes, udp_packet,
    with_fragment_field, with_ip_options, within,
};
use crate::dispatch::route;
use crate::packet::{Parsed, Reason, Segment, Transport, classify, segment};
use crate::stats::{Stats, Warnings};
use crate::tcp::Tuple;
use crate::testing::{Script, Upstream};
use crate::{Counters, TunOptions};
use smoltcp::wire::{TcpControl, TcpRepr, TcpSeqNumber};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// The app's addresses and addresses it sends to, the documentation ones for
/// the far end.
const APP4: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 1);
const FAR4: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);
const APP6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0, 1);
const FAR6: Ipv6Addr = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
const PORT: u16 = 40000;
const SERVICE: u16 = 80;

/// The bits of the IPv4 header's flags and fragment offset field.
const MORE_FRAGMENTS: u16 = 0x2000;
const DONT_FRAGMENT: u16 = 0x4000;

const ALL_REASONS: [Reason; 10] = [
    Reason::Malformed,
    Reason::Fragment,
    Reason::Martian,
    Reason::Unsupported,
    Reason::QueueFull,
    Reason::UnknownFake,
    Reason::Busy,
    Reason::BeyondWindow,
    Reason::RstLimited,
    Reason::IcmpLimited,
];

/// A SYN from `src` to `dst` (any addresses), the checksums right.
fn syn4(src: Ipv4Addr, dst: Ipv4Addr) -> Vec<u8> {
    syn_v4(sock(src, PORT), sock(dst, SERVICE))
}

fn syn6(src: Ipv6Addr, dst: Ipv6Addr) -> Vec<u8> {
    syn_v6(sock(src, PORT), sock(dst, SERVICE))
}

fn good4() -> Vec<u8> {
    syn4(APP4, FAR4)
}

fn good6() -> Vec<u8> {
    syn6(APP6, FAR6)
}

/// The same transport bytes in an IPv4 packet and an IPv6 one.
fn in_both(protocol: u8, transport: &[u8]) -> [Vec<u8>; 2] {
    [
        ipv4_packet(protocol, APP4, FAR4, transport),
        ipv6_packet(protocol, APP6, FAR6, transport),
    ]
}

/// A SYN whose IPv4 header is four words long (design 5.3, H4). smoltcp takes such a
/// header, and reads the TCP header from inside it: from the destination
/// address on, which here reads as the ports 49152 and 513 as well.
fn short_header_syn() -> Vec<u8> {
    let mut packet = vec![0x44, 0];
    // 16 bytes of IP header and a TCP header of 20
    packet.extend(36u16.to_be_bytes());
    packet.extend([0, 0, 0x40, 0, 64, 6, 0, 0]);
    packet.extend(APP4.octets());
    packet.extend(FAR4.octets());
    packet.extend(1u32.to_be_bytes());
    packet.extend(0u32.to_be_bytes());
    // a header of 20 bytes, SYN, a window of 65535, no checksum, no urgent pointer
    packet.extend([0x50, 0x02, 0xff, 0xff, 0, 0, 0, 0]);
    refresh(packet)
}

/// Why `classify` drops `packet`, with IPv6 handled.
fn dropped(packet: &[u8]) -> Reason {
    classify(packet, true)
        .expect_err("the packet is dropped")
        .reason
}

/// What `classify` makes of `packet`, with its drop as the reason alone.
fn classified(packet: &[u8], ipv6: bool) -> Result<Parsed, Reason> {
    classify(packet, ipv6).map_err(|dropped| dropped.reason)
}

/// The two addresses a drop of `packet` carries, if it does, with IPv6
/// handled.
fn ends_of(packet: &[u8]) -> Option<(IpAddr, IpAddr)> {
    classify(packet, true)
        .expect_err("the packet is dropped")
        .ends
}

/// What `classify` reads of `packet`, which it lets through.
fn passes(packet: &[u8]) -> Parsed {
    classify(packet, true).expect("the packet passes")
}

#[test]
fn a_bad_version_or_header_is_malformed() {
    // nothing to read a version from
    assert_eq!(dropped(&[]), Reason::Malformed);
    // a version that is neither 4 nor 6
    for version in [0u8, 1, 3, 5, 7, 8, 15] {
        let mut packet = good4();
        packet[0] = (version << 4) | (packet[0] & 15);
        assert_eq!(dropped(&packet), Reason::Malformed, "version {version}");
    }
    // an IPv4 header of fewer than five words (design 5.3, H4): one that only its length
    // gives away, and the words of any other count, which are malformed for
    // other reasons as well; the checksum is right for the words they have
    assert_eq!(dropped(&short_header_syn()), Reason::Malformed);
    for words in 0u8..5 {
        let mut packet = good4();
        packet[0] = 0x40 | words;
        assert_eq!(
            dropped(&refresh(packet)),
            Reason::Malformed,
            "a header of {words} words"
        );
    }
    // a total length that cannot hold the header, one that is more than was
    // received, and a packet cut short
    let total = good4().len() as u16;
    for wrong in [0, 19, total + 1, u16::MAX] {
        let mut packet = good4();
        packet[2..4].copy_from_slice(&wrong.to_be_bytes());
        assert_eq!(
            dropped(&refresh(packet)),
            Reason::Malformed,
            "a total length of {wrong}"
        );
    }
    let mut cut = good4();
    cut.pop();
    assert_eq!(dropped(&cut), Reason::Malformed, "cut short");
    assert_eq!(dropped(&good4()[..19]), Reason::Malformed, "no header");
    // a header checksum that does not hold
    for byte in [10, 11] {
        let mut packet = good4();
        packet[byte] ^= 0x01;
        assert_eq!(dropped(&packet), Reason::Malformed, "checksum byte {byte}");
    }
    // IPv6: a payload length of more than was received, a packet cut short,
    // a header cut short
    let mut longer = good6();
    longer[5] += 1;
    assert_eq!(dropped(&longer), Reason::Malformed, "a payload length more");
    let mut cut = good6();
    cut.pop();
    assert_eq!(dropped(&cut), Reason::Malformed, "cut short");
    assert_eq!(dropped(&good6()[..39]), Reason::Malformed, "no header");
    // the packets these were made from pass
    passes(&good4());
    passes(&good6());
}

#[test]
fn ipv6_off_is_unsupported() {
    let packet = good6();
    assert_eq!(classified(&packet, false), Err(Reason::Unsupported));
    assert!(classify(&packet, true).is_ok());
    // IPv6 goes before its header is read: a packet too short to be read is
    // no different
    assert_eq!(classified(&packet[..12], false), Err(Reason::Unsupported));
    assert_eq!(classified(&packet[..12], true), Err(Reason::Malformed));
    // IPv4 is as it was
    assert_eq!(classify(&good4(), false), classify(&good4(), true));
    assert!(classify(&good4(), false).is_ok());
}

#[test]
fn fragments_are_dropped() {
    // the first fragment, a middle one, the last, and one at the far end
    for field in [
        MORE_FRAGMENTS,
        MORE_FRAGMENTS | 185,
        185,
        MORE_FRAGMENTS | 0x1fff,
    ] {
        assert_eq!(
            dropped(&with_fragment_field(good4(), field)),
            Reason::Fragment,
            "{field:#06x}"
        );
    }
    // "don't fragment" alone is no fragment
    passes(&with_fragment_field(good4(), DONT_FRAGMENT));
    // a later fragment has no transport header to read: it is a fragment, not
    // a TCP segment that is damaged
    let later = ipv4_packet(6, APP4, FAR4, &[0xff; 12]);
    assert_eq!(dropped(&with_fragment_field(later, 185)), Reason::Fragment);
    // IPv6: a fragment header right after the IPv6 header
    let mut v6 = good6();
    v6[6] = 44;
    assert_eq!(dropped(&v6), Reason::Fragment);
    assert_eq!(
        dropped(&ipv6_packet(44, APP6, FAR6, &[0xff; 12])),
        Reason::Fragment
    );
}

#[test]
fn martians_are_dropped() {
    let mapped = Ipv4Addr::new(192, 0, 2, 1).to_ipv6_mapped();
    let cases = [
        ("an unspecified source", syn4(Ipv4Addr::UNSPECIFIED, FAR4)),
        (
            "an unspecified destination",
            syn4(APP4, Ipv4Addr::UNSPECIFIED),
        ),
        ("a loopback source", syn4(Ipv4Addr::LOCALHOST, FAR4)),
        ("a loopback destination", syn4(APP4, Ipv4Addr::LOCALHOST)),
        (
            "a loopback destination far from 127.0.0.1",
            syn4(APP4, Ipv4Addr::new(127, 255, 255, 254)),
        ),
        (
            "a multicast source",
            syn4(Ipv4Addr::new(224, 0, 0, 1), FAR4),
        ),
        (
            "a multicast destination",
            syn4(APP4, Ipv4Addr::new(224, 0, 0, 251)),
        ),
        (
            "a multicast destination in the upper range",
            syn4(APP4, Ipv4Addr::new(239, 255, 255, 250)),
        ),
        ("the broadcast address", syn4(APP4, Ipv4Addr::BROADCAST)),
        (
            "an unspecified IPv6 source",
            syn6(Ipv6Addr::UNSPECIFIED, FAR6),
        ),
        (
            "an unspecified IPv6 destination",
            syn6(APP6, Ipv6Addr::UNSPECIFIED),
        ),
        ("a loopback IPv6 source", syn6(Ipv6Addr::LOCALHOST, FAR6)),
        (
            "a loopback IPv6 destination",
            syn6(APP6, Ipv6Addr::LOCALHOST),
        ),
        (
            "a multicast IPv6 source",
            syn6(Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1), FAR6),
        ),
        (
            "a multicast IPv6 destination",
            syn6(APP6, Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0xfb)),
        ),
        ("an IPv4-mapped source", syn6(mapped, FAR6)),
        ("an IPv4-mapped destination", syn6(APP6, mapped)),
    ];
    for (what, packet) in &cases {
        assert_eq!(dropped(packet), Reason::Martian, "{what}");
    }
    // addresses that only look odd are addresses like any other
    for (src, dst) in [
        (APP4, FAR4),
        (Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(192, 168, 1, 1)),
        (APP4, Ipv4Addr::new(169, 254, 1, 1)),
        (APP4, Ipv4Addr::new(100, 64, 0, 1)),
        // one of rurge's fake addresses
        (APP4, Ipv4Addr::new(198, 18, 1, 1)),
        // the broadcast address of a subnet is not the limited broadcast
        (APP4, Ipv4Addr::new(192, 0, 2, 255)),
        (APP4, Ipv4Addr::new(240, 0, 0, 1)),
    ] {
        passes(&syn4(src, dst));
    }
    for (src, dst) in [
        (APP6, FAR6),
        (
            APP6,
            Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0xc612, 0x101),
        ),
        (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1), FAR6),
        (APP6, Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0xc000, 0x201)),
    ] {
        passes(&syn6(src, dst));
    }
}

#[test]
fn extension_headers_and_other_protocols_are_unsupported() {
    // after the IPv6 header: an extension header, or a protocol that is not
    // TCP, UDP or ICMPv6 (ICMP for IPv4 among them)
    for next_header in [0u8, 1, 4, 43, 47, 50, 51, 59, 60, 255] {
        let mut packet = good6();
        packet[6] = next_header;
        assert_eq!(
            dropped(&packet),
            Reason::Unsupported,
            "IPv6 next header {next_header}"
        );
    }
    // in the IPv4 header: not TCP, UDP or ICMP (ICMPv6 among them)
    for protocol in [2u8, 4, 47, 50, 58, 132, 255] {
        assert_eq!(
            dropped(&ipv4_packet(protocol, APP4, FAR4, &[0; 8])),
            Reason::Unsupported,
            "IPv4 protocol {protocol}"
        );
    }
    // the protocol is looked at before the transport header is: with no
    // header at all, such a packet is still just not handled
    assert_eq!(
        dropped(&ipv4_packet(47, APP4, FAR4, &[])),
        Reason::Unsupported
    );
    assert_eq!(
        dropped(&ipv6_packet(47, APP6, FAR6, &[])),
        Reason::Unsupported
    );
}

#[test]
fn truncated_transport_headers_and_port_0_are_malformed() {
    let (src, dst) = (sock(APP4, PORT), sock(FAR4, SERVICE));
    // TCP: the header stops short, whatever the IP packet says; the data
    // offset is below five words; the data offset reaches past the packet
    let tcp = tcp_bytes(&tcp_repr(src, dst, &[]), src.ip(), dst.ip());
    assert_eq!(tcp.len(), 20);
    for cut in [0, 1, 12, 19] {
        for packet in in_both(6, &tcp[..cut]) {
            assert_eq!(dropped(&packet), Reason::Malformed, "TCP cut to {cut}");
        }
    }
    for words in [0u8, 1, 4, 6, 15] {
        let mut bytes = tcp.clone();
        bytes[12] = (words << 4) | (bytes[12] & 15);
        for packet in in_both(6, &bytes) {
            assert_eq!(
                dropped(&packet),
                Reason::Malformed,
                "TCP data offset of {words} words"
            );
        }
    }
    // port 0, either end, of TCP and of UDP, in both families
    let udp = udp_bytes(src, dst, b"data");
    for (protocol, bytes) in [(6, &tcp), (17, &udp)] {
        for at in [0, 2] {
            let mut zero = bytes.clone();
            zero[at..at + 2].fill(0);
            for packet in in_both(protocol, &zero) {
                assert_eq!(
                    dropped(&packet),
                    Reason::Malformed,
                    "protocol {protocol}, port 0 at {at}"
                );
            }
        }
    }
    // UDP: the header stops short; the length field is below the header's,
    // or beyond the datagram
    for cut in [0, 7] {
        for packet in in_both(17, &udp[..cut]) {
            assert_eq!(dropped(&packet), Reason::Malformed, "UDP cut to {cut}");
        }
    }
    for length in [0u16, 7, 13] {
        let mut bytes = udp.clone();
        bytes[4..6].copy_from_slice(&length.to_be_bytes());
        for packet in in_both(17, &bytes) {
            assert_eq!(
                dropped(&packet),
                Reason::Malformed,
                "UDP length field {length}"
            );
        }
    }
    // a length field that is less than the datagram is what the sender
    // meant: the rest is padding
    let mut padded = udp.clone();
    padded[4..6].copy_from_slice(&10u16.to_be_bytes());
    for packet in in_both(17, &padded) {
        passes(&packet);
    }
    // ICMP: the header stops short
    for (src, dst) in [
        (IpAddr::from(APP4), IpAddr::from(FAR4)),
        (APP6.into(), FAR6.into()),
    ] {
        let echo = echo_bytes(src, dst);
        let protocol = if src.is_ipv4() { 1 } else { 58 };
        for cut in [0, 1, 4, 7] {
            assert_eq!(
                dropped(&ip_around(protocol, src, dst, &echo[..cut])),
                Reason::Malformed,
                "ICMP for {src} cut to {cut}"
            );
        }
        passes(&ip_around(protocol, src, dst, &echo[..8]));
    }
    // the packets these were made from pass
    for packet in in_both(6, &tcp).iter().chain(&in_both(17, &udp)) {
        passes(packet);
    }
}

#[test]
fn icmp_other_than_echo_is_unsupported() {
    let (v4, v6) = (
        echo_packet(APP4.into(), FAR4.into()),
        echo_packet(APP6.into(), FAR6.into()),
    );
    // the type is the first byte after the IP header
    let with_type = |packet: &[u8], header: usize, message_type: u8| {
        let mut changed = packet.to_vec();
        changed[header] = message_type;
        changed
    };
    assert_eq!(passes(&v4).transport, Transport::Icmp { message_type: 8 });
    assert_eq!(passes(&v6).transport, Transport::Icmp { message_type: 128 });
    // echo reply, destination unreachable, redirect, time exceeded,
    // timestamp, ...
    for message_type in [0u8, 3, 5, 11, 13, 14, 17, 128, 255] {
        assert_eq!(
            dropped(&with_type(&v4, 20, message_type)),
            Reason::Unsupported,
            "ICMPv4 type {message_type}"
        );
    }
    // echo reply, the errors, router and neighbor discovery, MLD, ... and
    // ICMPv4's echo request, which is no echo request here
    for message_type in [1u8, 2, 3, 4, 8, 129, 130, 133, 134, 135, 136, 143, 255] {
        assert_eq!(
            dropped(&with_type(&v6, 40, message_type)),
            Reason::Unsupported,
            "ICMPv6 type {message_type}"
        );
    }
}

#[test]
fn the_checks_run_in_order() {
    // a header that cannot be read comes before everything: a fragment whose
    // checksum is wrong is malformed
    let mut bad_sum = with_fragment_field(good4(), MORE_FRAGMENTS);
    bad_sum[10] ^= 0x01;
    assert_eq!(dropped(&bad_sum), Reason::Malformed);
    // IPv6 off comes before the fragment check
    let mut v6_fragment = good6();
    v6_fragment[6] = 44;
    assert_eq!(classified(&v6_fragment, false), Err(Reason::Unsupported));
    assert_eq!(classified(&v6_fragment, true), Err(Reason::Fragment));
    // a fragment to a multicast address is a fragment: that check is first
    let to_multicast =
        with_fragment_field(syn4(APP4, Ipv4Addr::new(224, 0, 0, 251)), MORE_FRAGMENTS);
    assert_eq!(dropped(&to_multicast), Reason::Fragment);
    // a martian comes before the protocol being looked at
    assert_eq!(
        dropped(&ipv4_packet(47, APP4, Ipv4Addr::new(224, 0, 0, 1), &[0; 8])),
        Reason::Martian
    );
    // the protocol comes before the transport header being read
    assert_eq!(
        dropped(&ipv4_packet(47, APP4, FAR4, &[])),
        Reason::Unsupported
    );
    // a transport header that is damaged comes before the ICMP type being
    // looked at: a destination unreachable (type 3) cut to four bytes
    assert_eq!(
        dropped(&ipv4_packet(1, APP4, FAR4, &[3, 0, 0, 0])),
        Reason::Malformed
    );
    // and the type is looked at last
    assert_eq!(
        dropped(&ipv4_packet(1, APP4, FAR4, &[3, 0, 0, 0, 0, 0, 0, 0])),
        Reason::Unsupported
    );
}

/// What a drop carries of the packet: its two addresses once the IP header
/// has passed its own checks, whichever check turns the packet away after
/// that; nothing for a drop decided before, and for IPv6 while it is off.
#[test]
fn a_drop_carries_the_addresses_once_the_header_was_read() {
    let v4 = Some((IpAddr::from(APP4), IpAddr::from(FAR4)));
    let v6 = Some((IpAddr::from(APP6), IpAddr::from(FAR6)));
    // decided before the header passed: nothing was read
    let mut bad_sum = good4();
    bad_sum[10] ^= 0x01;
    let mut cut4 = good4();
    cut4.pop();
    let mut cut6 = good6();
    cut6.pop();
    let mut version5 = good4();
    version5[0] = 0x50 | (version5[0] & 15);
    for (what, packet) in [
        ("no packet", Vec::new()),
        ("a version of 5", version5),
        ("a header of four words", short_header_syn()),
        ("a header checksum that fails", bad_sum),
        ("an IPv4 packet cut short", cut4),
        ("an IPv6 packet cut short", cut6),
    ] {
        assert_eq!(ends_of(&packet), None, "{what}");
    }
    // IPv6 while it is off is decided on the version alone, however good
    // the packet is
    assert_eq!(classify(&good6(), false).unwrap_err().ends, None);
    // decided after: the packet's own two addresses
    let mut fragment6 = good6();
    fragment6[6] = 44;
    let mut hop_by_hop = good6();
    hop_by_hop[6] = 0;
    let port0 = |src: SocketAddr, dst: SocketAddr| tcp_packet(src, dst, &tcp_repr(src, dst, &[]));
    for (what, packet, ends) in [
        (
            "a fragment",
            with_fragment_field(good4(), MORE_FRAGMENTS),
            v4,
        ),
        ("an IPv6 fragment", fragment6, v6),
        (
            "a martian",
            syn4(Ipv4Addr::UNSPECIFIED, FAR4),
            Some((Ipv4Addr::UNSPECIFIED.into(), FAR4.into())),
        ),
        (
            "an IPv6 martian",
            syn6(APP6, Ipv6Addr::LOCALHOST),
            Some((APP6.into(), Ipv6Addr::LOCALHOST.into())),
        ),
        (
            "a protocol nothing handles",
            ipv4_packet(47, APP4, FAR4, &[0; 8]),
            v4,
        ),
        ("an extension header", hop_by_hop, v6),
        (
            "a TCP header cut short",
            ipv4_packet(6, APP4, FAR4, &[0; 12]),
            v4,
        ),
        ("TCP port 0", port0(sock(APP4, 0), sock(FAR4, SERVICE)), v4),
        (
            "TCP port 0 over IPv6",
            port0(sock(APP6, PORT), sock(FAR6, 0)),
            v6,
        ),
        (
            "UDP port 0",
            udp_packet(sock(APP6, 0), sock(FAR6, 53), b""),
            v6,
        ),
        (
            "an ICMP header cut short",
            ipv4_packet(1, APP4, FAR4, &[8, 0, 0]),
            v4,
        ),
        (
            "an ICMP message that is no echo request",
            ipv4_packet(1, APP4, FAR4, &[3, 0, 0, 0, 0, 0, 0, 0]),
            v4,
        ),
        (
            "an ICMPv6 message that is no echo request",
            ipv6_packet(58, APP6, FAR6, &[1, 0, 0, 0, 0, 0, 0, 0]),
            v6,
        ),
    ] {
        assert_eq!(ends_of(&packet), ends, "{what}");
    }
}

#[test]
fn trailing_bytes_after_the_ip_packet_are_ignored() {
    let (src, dst) = (sock(APP4, 5353), sock(FAR4, 53));
    let (src6, dst6) = (sock(APP6, 5353), sock(FAR6, 53));
    let packets = [
        good4(),
        good6(),
        udp_packet(src, dst, b"query"),
        udp_packet(src6, dst6, b"query"),
        echo_packet(APP4.into(), FAR4.into()),
        echo_packet(APP6.into(), FAR6.into()),
        with_ip_options(&good4(), &[1, 1, 1, 0]),
    ];
    for packet in &packets {
        let mut padded = packet.clone();
        padded.extend([0xaa; 7]);
        let parsed = passes(packet);
        assert_eq!(parsed.len, packet.len(), "the packet ends where it says");
        assert_eq!(classify(&padded, true), classify(packet, true));
    }
}

/// What a packet that passes is read as, field by field.
#[test]
fn good_packets_are_read_as_they_are() {
    // TCP: every number and flag of the segment, in both families
    for (src, dst) in [
        (sock(APP4, 40000), sock(FAR4, 443)),
        (sock(APP6, 40000), sock(FAR6, 443)),
    ] {
        let cases = [
            (
                TcpControl::Psh,
                Some(0xa0b0_c0d0u32),
                (false, true, false, false),
            ),
            (TcpControl::Syn, None, (true, false, false, false)),
            (TcpControl::Syn, Some(5), (true, true, false, false)),
            (TcpControl::Fin, Some(5), (false, true, true, false)),
            (TcpControl::Rst, None, (false, false, false, true)),
        ];
        for (control, ack, (syn, ack_flag, fin, rst)) in cases {
            let repr = TcpRepr {
                control,
                seq_number: TcpSeqNumber(0x8102_0304u32 as i32),
                ack_number: ack.map(|number| TcpSeqNumber(number as i32)),
                ..tcp_repr(src, dst, b"hello")
            };
            let packet = tcp_packet(src, dst, &repr);
            assert_eq!(
                passes(&packet),
                Parsed {
                    src: src.ip(),
                    dst: dst.ip(),
                    len: packet.len(),
                    transport: Transport::Tcp(Segment {
                        tuple: Tuple { src, dst },
                        syn,
                        ack: ack_flag,
                        fin,
                        rst,
                        seq: 0x8102_0304,
                        ack_number: ack.unwrap_or(0),
                        header_len: 20,
                        payload_len: 5,
                    }),
                },
                "{control:?} with {ack:?} over {dst}"
            );
        }
        // options lengthen the header, not the payload
        let repr = TcpRepr {
            control: TcpControl::Syn,
            max_seg_size: Some(1460),
            window_scale: Some(7),
            sack_permitted: true,
            ..tcp_repr(src, dst, &[])
        };
        let packet = tcp_packet(src, dst, &repr);
        let Transport::Tcp(segment) = passes(&packet).transport else {
            panic!("a TCP segment");
        };
        assert_eq!(segment.header_len, repr.header_len());
        assert!(segment.header_len > 20);
        assert_eq!(segment.payload_len, 0);
    }
    // UDP and ICMP: what the header says of the ends and the kind
    for (src, dst) in [
        (sock(APP4, 5353), sock(FAR4, 53)),
        (sock(APP6, 5353), sock(FAR6, 53)),
    ] {
        let packet = udp_packet(src, dst, b"query");
        assert_eq!(
            passes(&packet),
            Parsed {
                src: src.ip(),
                dst: dst.ip(),
                len: packet.len(),
                transport: Transport::Udp {
                    src_port: 5353,
                    dst_port: 53
                },
            }
        );
        let packet = echo_packet(src.ip(), dst.ip());
        assert_eq!(
            passes(&packet),
            Parsed {
                src: src.ip(),
                dst: dst.ip(),
                len: packet.len(),
                transport: Transport::Icmp {
                    message_type: if src.is_ipv4() { 8 } else { 128 }
                },
            }
        );
    }
}

#[test]
fn ipv4_options_are_accepted() {
    let plain = good4();
    // no-operations and the end of the list, then a record-route option
    for options in [vec![1, 1, 1, 0], vec![7, 7, 4, 0, 0, 0, 0, 0], vec![1; 40]] {
        let packet = with_ip_options(&plain, &options);
        assert_eq!(passes(&packet).transport, passes(&plain).transport);
        assert_eq!(passes(&packet).len, plain.len() + options.len());
        // the lengths hold, but not the checksum
        let mut damaged = packet.clone();
        damaged[10] ^= 0x01;
        assert_eq!(dropped(&damaged), Reason::Malformed);
    }
}

#[test]
fn the_shard_reads_the_segment_classify_found() {
    let packet = good4();
    let Transport::Tcp(found) = passes(&packet).transport else {
        panic!("a TCP segment");
    };
    assert_eq!(segment(&packet), Some(found));
    let packet = good6();
    let Transport::Tcp(found) = passes(&packet).transport else {
        panic!("a TCP segment");
    };
    assert_eq!(segment(&packet), Some(found));
    // anything else that reaches a shard is not read at all
    assert_eq!(
        segment(&udp_packet(sock(APP4, 1), sock(FAR4, 2), b"")),
        None
    );
    assert_eq!(segment(&[]), None);
    assert_eq!(segment(&good4()[..30]), None);
}

/// A shard is given only what `classify` let through. A test at the shard's
/// level hands it packets itself, and one that `classify` turns away (here a
/// SYN to the loopback address) would be ignored without a word: in a build
/// with debug assertions the shard says so.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "a shard is given only what classify let through")]
fn a_shard_given_what_classify_turned_away_says_so() {
    use crate::tcp::{Budget, Shard};
    use std::sync::Arc;
    use std::time::Instant;

    let options = TunOptions::default();
    let budget = Arc::new(Budget::new(
        options.budget,
        options.buffer_min,
        options.tx_max,
    ));
    let shard = Shard::new(1500, Instant::now(), 1, budget, &options);
    let to_loopback = syn_v4(addr("198.18.0.1:40000"), addr("127.0.0.1:80"));
    shard.state().turn(Instant::now(), &mut vec![to_loopback]);
}

/// Nothing a damaged packet holds makes `classify` panic or promise more than
/// the packet has, and what it lets through reads the same once cut to
/// its length (which is what the shards are given). A byte changed in an IPv4
/// header is stopped at its checksum, so each change is made twice there: as
/// it is, and with the checksum worked out again, which lets it reach the
/// checks after it.
#[test]
fn damaged_packets_are_dropped_without_a_panic() {
    let (src, dst) = (sock(APP4, 5353), sock(FAR4, 53));
    let bases = [
        good4(),
        good6(),
        with_ip_options(&good4(), &[1, 1, 1, 0]),
        udp_packet(src, dst, b"query"),
        udp_packet(sock(APP6, 5353), sock(FAR6, 53), b"query"),
        echo_packet(APP4.into(), FAR4.into()),
        echo_packet(APP6.into(), FAR6.into()),
    ];
    for base in &bases {
        let ipv4 = base[0] >> 4 == 4;
        for ipv6 in [true, false] {
            let check = |packet: &[u8]| {
                if let Ok(parsed) = classify(packet, ipv6) {
                    assert!(parsed.len <= packet.len());
                    assert_eq!(classify(&packet[..parsed.len], ipv6), Ok(parsed));
                }
            };
            for len in 0..=base.len() {
                check(&base[..len]);
            }
            for at in 0..base.len() {
                for value in [0x00, 0x01, 0x40, 0x7f, 0x80, 0xff] {
                    let mut packet = base.clone();
                    packet[at] = value;
                    check(&packet);
                    // a header length that reaches past the packet has no
                    // checksum to work out
                    if ipv4 && usize::from(packet[0] & 15) * 4 <= packet.len() {
                        check(&refresh(packet));
                    }
                }
            }
        }
    }
}

/// The dispatcher cuts a packet to its own length before a shard is given it.
#[test]
fn the_bytes_after_the_ip_packet_do_not_reach_the_shard() {
    for packet in [good4(), good6(), with_ip_options(&good4(), &[1, 1, 1, 0])] {
        let (to_shard, mut shard) = mpsc::channel(4);
        let stats = Stats::default();
        let mut padded = packet.clone();
        padded.extend([0xaa; 9]);
        route(padded, true, &[to_shard], &stats);
        assert_eq!(shard.try_recv().expect("the shard is given it"), packet);
        assert_eq!(stats.counters(), Counters::default());
    }
}

#[test]
fn every_reason_is_its_own_counter() {
    for reason in ALL_REASONS {
        let stats = Stats::default();
        stats.drop(reason, None, None);
        let counters = format!("{:?}", stats.counters());
        // `Counters { malformed: 1, fragment: 0, ... }`: the one counter of
        // the reason's name is 1 and every other is 0
        assert!(counters.contains(&format!("{reason}: 1")), "{counters}");
        assert_eq!(counters.matches(": 1").count(), 1, "{counters}");
    }
}

#[test]
fn each_reason_is_first_once() {
    let warnings = Warnings::default();
    for reason in ALL_REASONS {
        if reason == Reason::Martian {
            continue;
        }
        assert!(warnings.first_time(reason), "{reason} the first time");
        assert!(!warnings.first_time(reason), "{reason} the second time");
        assert!(!warnings.first_time(reason), "{reason} the third time");
    }
    // a martian is never said at all
    assert!(!warnings.first_time(Reason::Martian));
    // and each TUN has its own
    assert!(Warnings::default().first_time(Reason::Malformed));
}

/// The one fixed wait of these tests, for what must not happen: an answer to
/// a packet that is dropped. An answer would go through a shard and the
/// writer, which takes well under a millisecond; 200 ms is a wide margin.
async fn assert_unanswered(rig: &Rig) {
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        rig.app.packets(),
        (0, 0),
        "the app has heard nothing from the TUN"
    );
}

/// `hello` there and back on a new connection to `dst`, which is scripted to
/// an echo server.
async fn hello(rig: &Rig, dst: SocketAddr) {
    let echo = Upstream::echo().await;
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    tcp.write_all(b"hello").await.expect("write");
    let mut back = [0; 5];
    within("hello to come back", tcp.read_exact(&mut back))
        .await
        .expect("read");
    assert_eq!(&back, b"hello");
}

/// Design 5.3, H1: smoltcp takes a SYN from 0.0.0.0, and panics building the SYN-ACK.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_syn_from_the_unspecified_address_is_dropped() {
    let rig = rig(TunOptions::default());
    rig.app
        .inject(syn_v4(addr("0.0.0.0:40000"), addr("192.0.2.1:80")));
    eventually("the packet to be counted", WITHIN, || {
        rig.tun.counters().martian == 1
    })
    .await;
    assert_unanswered(&rig).await;
    assert_eq!(
        rig.tun.counters(),
        Counters {
            martian: 1,
            ..Counters::default()
        }
    );
    assert_eq!(rig.tun.sockets_in_use(), 0);
}

/// Design 5.3, H4: smoltcp takes an IPv4 header of four words, and the TCP header it then
/// reads starts inside the IP header, at its destination address.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_short_ipv4_header_is_dropped() {
    let rig = rig(TunOptions::default());
    rig.app.inject(short_header_syn());
    eventually("the packet to be counted", WITHIN, || {
        rig.tun.counters().malformed == 1
    })
    .await;
    assert_unanswered(&rig).await;
    assert_eq!(
        rig.tun.counters(),
        Counters {
            malformed: 1,
            ..Counters::default()
        }
    );
    assert_eq!(rig.tun.sockets_in_use(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bad_packet_harms_nothing() {
    let rig = rig(TunOptions::default());
    let dst = addr("192.0.2.1:80");
    // one of each kind, all to a destination a connection can be made to
    let mut bad_sum = syn_v4(addr("198.18.0.1:40001"), dst);
    bad_sum[10] ^= 0x01;
    for packet in [
        bad_sum,
        with_fragment_field(syn_v4(addr("198.18.0.1:40002"), dst), MORE_FRAGMENTS),
        syn_v4(addr("0.0.0.0:40003"), dst),
        ipv4_packet(47, APP4, FAR4, &[0; 8]),
    ] {
        rig.app.inject(packet);
    }
    let expected = Counters {
        malformed: 1,
        fragment: 1,
        martian: 1,
        unsupported: 1,
        ..Counters::default()
    };
    eventually("the packets to be counted", WITHIN, || {
        rig.tun.counters() == expected
    })
    .await;
    assert_unanswered(&rig).await;
    assert_eq!(rig.tun.sockets_in_use(), 0);
    // and the TUN goes on as if they had not come
    hello(&rig, dst).await;
    assert_eq!(rig.tun.counters(), expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ipv6_off_serves_ipv4_only() {
    let rig = rig(TunOptions {
        ipv6: false,
        ..TunOptions::default()
    });
    rig.app.inject(syn_v6(
        addr("[fd00:6152::1]:40000"),
        addr("[2001:db8::1]:80"),
    ));
    eventually("the packet to be counted", WITHIN, || {
        rig.tun.counters().unsupported == 1
    })
    .await;
    assert_unanswered(&rig).await;
    // IPv4 is served
    hello(&rig, addr("192.0.2.1:80")).await;
    assert_eq!(
        rig.tun.counters(),
        Counters {
            unsupported: 1,
            ..Counters::default()
        }
    );
}

/// Nothing handles UDP or ICMP yet: a datagram and an echo request, in both
/// families, are dropped as unsupported.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_and_icmp_are_not_handled_yet() {
    let rig = rig(TunOptions::default());
    rig.app
        .inject(udp_packet(sock(APP4, 5353), sock(FAR4, 53), b"query"));
    rig.app.inject(echo_packet(APP4.into(), FAR4.into()));
    rig.app
        .inject(udp_packet(sock(APP6, 5353), sock(FAR6, 53), b"query"));
    rig.app.inject(echo_packet(APP6.into(), FAR6.into()));
    eventually("the packets to be counted", WITHIN, || {
        rig.tun.counters().unsupported == 4
    })
    .await;
    assert_unanswered(&rig).await;
    assert_eq!(
        rig.tun.counters(),
        Counters {
            unsupported: 4,
            ..Counters::default()
        }
    );
}

/// The WARN of a reason: its text, with no address.
fn warning(reason: Reason) -> String {
    format!(
        "WARN tun: a packet was dropped (reason: {reason}); later drops for this reason are logged at debug level"
    )
}

#[tokio::test]
async fn each_reason_warns_once() {
    let log = LogCapture::start();
    let rig = rig(TunOptions::default());
    let dst = addr("192.0.2.1:80");
    let mut bad_sum = syn_v4(addr("198.18.0.1:40001"), dst);
    bad_sum[10] ^= 0x01;
    let mut cut = syn_v4(addr("198.18.0.1:40002"), dst);
    cut.pop();
    let mut v6_fragment = syn_v6(addr("[fd00:6152::1]:40003"), addr("[2001:db8::1]:80"));
    v6_fragment[6] = 44;
    // two packets of each reason, in both families, in this order
    for packet in [
        bad_sum,
        cut,
        with_fragment_field(syn_v4(addr("198.18.0.1:40004"), dst), MORE_FRAGMENTS),
        v6_fragment,
        syn_v4(addr("0.0.0.0:40005"), dst),
        syn_v6(addr("[fd00:6152::1]:40006"), addr("[ff02::1]:80")),
        udp_packet(sock(APP4, 5353), sock(FAR4, 53), b"query"),
        udp_packet(sock(APP6, 5353), sock(FAR6, 53), b"query"),
    ] {
        rig.app.inject(packet);
    }
    let expected = Counters {
        malformed: 2,
        fragment: 2,
        martian: 2,
        unsupported: 2,
        ..Counters::default()
    };
    eventually("the packets to be counted", WITHIN, || {
        rig.tun.counters() == expected
    })
    .await;
    let lines = log.lines();
    let ours: Vec<&String> = lines.iter().filter(|line| line.contains("tun:")).collect();
    // the first of a reason at WARN, the second at DEBUG; a martian at neither
    for reason in [Reason::Malformed, Reason::Fragment, Reason::Unsupported] {
        let warns: Vec<&&String> = ours
            .iter()
            .filter(|line| line.starts_with("WARN") && line.contains(&format!("reason: {reason})")))
            .collect();
        assert_eq!(warns.len(), 1, "one WARN for {reason}: {lines:?}");
        assert_eq!(**warns[0], warning(reason));
        let debugs = ours
            .iter()
            .filter(|line| line.starts_with("DEBUG") && line.contains(&format!("reason={reason}")))
            .count();
        assert_eq!(debugs, 1, "one DEBUG for {reason}: {lines:?}");
    }
    assert_eq!(ours.len(), 6, "{lines:?}");
    assert!(
        lines.iter().all(|line| !line.contains("martian")),
        "a martian is not said: {lines:?}"
    );
    // no address in a WARN
    for line in lines.iter().filter(|line| line.starts_with("WARN")) {
        for prefix in ["192.0.2.", "198.18.", "2001:db8:", "fd00:"] {
            assert!(!line.contains(prefix), "{line}");
        }
    }
    // the DEBUG line of a packet that was read to the end has its ends
    let unsupported = ours
        .iter()
        .find(|line| line.starts_with("DEBUG") && line.contains("reason=unsupported"))
        .expect("a DEBUG line for the second unsupported packet");
    assert!(unsupported.contains("src=fd00:6152::1"), "{unsupported}");
    assert!(unsupported.contains("dst=2001:db8::1"), "{unsupported}");
}

/// The DEBUG line of a drop has the two addresses of the packet when they
/// were read: for a fragment, a protocol nothing handles and a TCP port 0
/// they are there, for a header checksum that fails and for IPv6 while it is
/// off they are not. The first drop of a reason is its WARN, which has none.
#[test]
fn a_debug_line_has_the_addresses_that_were_read() {
    let log = LogCapture::start();
    let (to_shard, _shard) = mpsc::channel(1);
    let stats = Stats::default();
    let mut bad_sum = good4();
    bad_sum[10] ^= 0x01;
    let (src, dst) = (sock(APP4, 0), sock(FAR4, SERVICE));
    let port0 = tcp_packet(src, dst, &tcp_repr(src, dst, &[]));
    let fragment = with_fragment_field(good4(), MORE_FRAGMENTS);
    let unhandled = ipv4_packet(47, APP4, FAR4, &[0; 8]);
    for (packet, ipv6) in [
        (bad_sum.clone(), true),
        (bad_sum, true),
        (port0, true),
        (fragment.clone(), true),
        (fragment, true),
        (unhandled.clone(), true),
        (unhandled, true),
        (good6(), false),
    ] {
        route(packet, ipv6, std::slice::from_ref(&to_shard), &stats);
    }
    let ends = "src=198.18.0.1 dst=192.0.2.1";
    assert_eq!(
        log.lines(),
        [
            warning(Reason::Malformed),
            "DEBUG tun: packet dropped reason=malformed".to_owned(),
            format!("DEBUG tun: packet dropped reason=malformed {ends}"),
            warning(Reason::Fragment),
            format!("DEBUG tun: packet dropped reason=fragment {ends}"),
            warning(Reason::Unsupported),
            format!("DEBUG tun: packet dropped reason=unsupported {ends}"),
            "DEBUG tun: packet dropped reason=unsupported".to_owned(),
        ]
    );
}

/// A shard's queue that is full drops the packet, counts it and says so as
/// every other reason does, with the packet's ends in the DEBUG line.
#[test]
fn a_full_shard_queue_drops_and_says_so() {
    let log = LogCapture::start();
    let (to_shard, mut shard) = mpsc::channel(1);
    let stats = Stats::default();
    let packet = good4();
    for _ in 0..3 {
        route(
            packet.clone(),
            true,
            std::slice::from_ref(&to_shard),
            &stats,
        );
    }
    // the first took the queue's one place, the other two found it full
    assert_eq!(
        stats.counters(),
        Counters {
            queue_full: 2,
            ..Counters::default()
        }
    );
    assert_eq!(shard.try_recv().expect("the first packet"), packet);
    assert!(shard.try_recv().is_err());
    let lines = log.lines();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0], warning(Reason::QueueFull));
    assert!(
        lines[1].starts_with("DEBUG tun: packet dropped"),
        "{lines:?}"
    );
    assert!(lines[1].contains("reason=queue_full"), "{lines:?}");
    assert!(lines[1].contains("src=198.18.0.1"), "{lines:?}");
    assert!(lines[1].contains("dst=192.0.2.1"), "{lines:?}");
}
