//! `[IP Rewrite]` at the TUN's receive side (phase 3 M5 design 8.3 – 8.9): the
//! table of a generation, where in the checks it is asked, and what `reflect`,
//! `reject` and `drop` do through the memory device and the app's end of it.
//! The tables are read from profile text, as the loader reads a profile, so a
//! table written wrong here is a test that fails rather than an empty table
//! that quietly matches nothing.

use super::udp::{APP4, APP6, NOTHING, lab_and_extras};
use crate::packet::{Refusal, refusal, segment};
use crate::space::ADAPTER_V4;
use crate::testing::packets;
use crate::testing::{FakeResolve, UdpScript};
use crate::tests::log_capture::LogCapture;
use crate::tests::support::{
    Extras, WITHIN, addr, eventually, ipv4_packet, ipv6_packet, segment_v4, settings, sock, syn_v4,
    syn_v6, with_fragment_field, within,
};
use crate::{Counters, TunOptions, TunSettings};
use rurge_config::text::{Origin, Section, parse_str};
use rurge_config::{HostName, IpRewrite};
use rurge_dns::fakeip::v4_of;
use rurge_net::connector::Target;
use rurge_proto::RejectKind;
use smoltcp::wire::{Ipv4Packet, TcpPacket};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// The two app ends' addresses (the endpoints are `APP4` and `APP6`).
const APP4_IP: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 1);
const APP6_IP: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0, 1);

/// The bits of the IPv4 header's flags and fragment offset field for "more
/// fragments" and for a later fragment.
const MORE_FRAGMENTS: u16 = 0x2000;
const LATER_FRAGMENT: u16 = 185;

/// The `[IP Rewrite]` table that `lines` makes, read as the loader reads it.
/// A line that does not read is a test that fails here, not an empty table.
fn table(lines: &str) -> Arc<IpRewrite> {
    let text = format!("[IP Rewrite]\n{lines}");
    let (profile, mut diags) = parse_str(&text, Arc::from(Path::new("rewrite.conf")), Origin::Main);
    let table = rurge_config::ip_rewrite::parse(
        profile
            .section("IP Rewrite")
            .into_iter()
            .flat_map(Section::active_entries),
        &mut diags,
    );
    assert!(
        diags.is_empty(),
        "the table's lines read cleanly: {:?}",
        diags.into_vec()
    );
    Arc::new(table)
}

/// `extras` set to this generation's table, with the DNS the extras began with.
fn set_table(extras: &Extras, lines: &str) {
    extras.hooks.set(TunSettings {
        ip_rewrite: table(lines),
        dns: extras.dns.clone(),
        ..settings(FakeResolve::new(), true)
    });
}

/// Extras of the default kind whose generation has the table `lines` makes.
fn with_table(lines: &str) -> Extras {
    let extras = Extras::default();
    set_table(&extras, lines);
    extras
}

/// A rig whose generation has the table `lines` makes.
fn lab_with_table(lines: &str) -> super::udp::Lab {
    lab_and_extras(TunOptions::default(), with_table(lines)).0
}

/// `packet` with the two addresses of its IP header swapped, as `reflect`
/// sends it back: every other byte as it was.
fn swapped(packet: &[u8]) -> Vec<u8> {
    let mut swapped = packet.to_vec();
    let (at, width) = match swapped[0] >> 4 {
        4 => (12, 4),
        6 => (8, 16),
        version => panic!("a version of {version}"),
    };
    let (_, rest) = swapped.split_at_mut(at);
    let (src, rest) = rest.split_at_mut(width);
    let (dst, _) = rest.split_at_mut(width);
    src.swap_with_slice(dst);
    swapped
}

/// `endpoint` as the target the dialer is scripted for.
fn target(endpoint: SocketAddr) -> Target {
    Target::new(HostName::Ip(endpoint.ip()), endpoint.port())
}

/// A SYN to a `reflect` address is sent back with the two addresses swapped,
/// its ports as they were, and its IPv4 and TCP checksums still holding (a
/// swap is a sum of the same words in another order); nothing is dialed:
/// `reflect` is the TUN's own answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reflect_sends_a_syn_back_with_the_addresses_swapped() {
    let lab = lab_with_table("203.0.113.9 = reflect\n");
    // the ports are not swapped: the packet goes back to the app's port 80
    let mut replies = lab.rig.app.divert(80..=80);
    let syn = syn_v4(addr(APP4), addr("203.0.113.9:80"));
    lab.rig.app.inject(syn.clone());
    let reply = within("the reflected SYN", replies.recv())
        .await
        .expect("the app's stack is there");
    assert_eq!(reply, swapped(&syn), "every byte but the two addresses");
    let ip = Ipv4Packet::new_checked(&reply[..]).expect("an IPv4 packet");
    assert_eq!(ip.src_addr(), Ipv4Addr::new(203, 0, 113, 9));
    assert_eq!(ip.dst_addr(), APP4_IP);
    assert!(ip.verify_checksum());
    let tcp = TcpPacket::new_checked(ip.payload()).expect("a TCP segment");
    assert!(tcp.verify_checksum(&ip.src_addr().into(), &ip.dst_addr().into()));
    assert_eq!((tcp.src_port(), tcp.dst_port()), (5000, 80));
    assert_eq!(
        lab.counters(),
        Counters {
            reflected: 1,
            ..Counters::default()
        }
    );
    assert!(lab.dialer().dialed().is_empty(), "nothing was dialed");
}

/// Every kind of packet goes back: a UDP datagram, an echo request, an IPv6
/// datagram and a fragment, each with its two addresses swapped and every other
/// byte as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reflect_sends_udp_echo_ipv6_and_fragments_back() {
    let mut lab = lab_with_table("203.0.113.0/24 = reflect\n2001:db8::/32 = reflect\n");
    let app = addr(APP4);
    let app6 = addr(APP6);
    let udp4 = packets::udp(app, addr("203.0.113.5:9000"), b"hello");
    let echo = packets::echo_request(
        IpAddr::V4(APP4_IP),
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 6)),
        0x1234,
        7,
        b"ping!",
    );
    let udp6 = packets::udp(app6, addr("[2001:db8::5]:9000"), b"hello");
    let fragment = with_fragment_field(
        packets::udp(app, addr("203.0.113.7:9000"), b"a fragment"),
        MORE_FRAGMENTS,
    );
    for packet in [udp4.clone(), echo.clone(), udp6.clone(), fragment.clone()] {
        lab.rig.app.inject(packet);
    }
    let back = lab.packet().await;
    assert_eq!(back, swapped(&udp4));
    assert_eq!(
        packets::read_udp(&back).expect("a whole datagram"),
        packets::UdpSeen {
            src: addr("203.0.113.5:5000"),
            dst: addr("198.18.0.1:9000"),
            payload: b"hello".to_vec(),
        }
    );
    let back = lab.packet().await;
    assert_eq!(back, swapped(&udp6));
    assert_eq!(
        packets::read_udp(&back).expect("a whole datagram"),
        packets::UdpSeen {
            src: addr("[2001:db8::5]:5000"),
            dst: addr("[fd00:6152::1]:9000"),
            payload: b"hello".to_vec(),
        }
    );
    let back = lab.packet().await;
    assert_eq!(back, swapped(&fragment), "a fragment goes back as it came");
    let echoed = within("the reflected echo request", lab.icmp.recv())
        .await
        .expect("the app's stack is there");
    assert_eq!(echoed, swapped(&echo));
    let ip = Ipv4Packet::new_checked(&echoed[..]).expect("an IPv4 packet");
    assert_eq!(ip.payload()[0], 8, "still an echo request");
    assert!(ip.verify_checksum());
    assert_eq!(
        lab.counters(),
        Counters {
            reflected: 4,
            ..Counters::default()
        }
    );
}

/// A SYN to a `reject` address is answered with a reset from where the SYN was
/// going, acknowledging the SYN's sequence number plus one; the packet itself
/// is counted `ip_rewrite`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reject_answers_a_syn_with_a_reset() {
    let lab = lab_with_table("203.0.113.9 = reject\n");
    let mut replies = lab.rig.app.divert(5000..=5000);
    let dst = addr("203.0.113.9:80");
    lab.rig.app.inject(syn_v4(addr(APP4), dst));
    let reply = within("the reset", replies.recv())
        .await
        .expect("the app's stack is there");
    let reset = segment(&reply).expect("a TCP segment");
    assert_eq!(
        reset.tuple,
        crate::tcp::Tuple {
            src: dst,
            dst: addr(APP4)
        }
    );
    assert!(
        reset.rst && reset.ack && !reset.syn && !reset.fin,
        "{reset:?}"
    );
    assert_eq!(reset.seq, 0);
    assert_eq!(reset.ack_number, 2, "the SYN's sequence number plus one");
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 1,
            ..Counters::default()
        }
    );
}

/// Everything else is answered with "administratively prohibited", from where
/// the packet was going, quoting the IPv4 header and the eight bytes after it
/// (RFC 1122 3.2.2) or, over IPv6, as much of the packet as fits in the
/// smallest MTU (RFC 4443 2.4(c)): a UDP datagram, a TCP segment with ACK, an
/// echo request, a TCP first fragment and an IPv6 datagram.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reject_answers_the_rest_with_prohibited() {
    let mut lab = lab_with_table("203.0.113.0/24 = reject\n2001:db8::/32 = reject\n");
    let app = addr(APP4);
    let app6 = addr(APP6);
    let cases = [
        (
            packets::udp(app, addr("203.0.113.10:9000"), b"hello"),
            IpAddr::V4(APP4_IP),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)),
            28,
        ),
        (
            segment_v4(app, addr("203.0.113.11:80"), 11, 12, 0x10),
            IpAddr::V4(APP4_IP),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 11)),
            28,
        ),
        (
            packets::echo_request(
                IpAddr::V4(APP4_IP),
                IpAddr::V4(Ipv4Addr::new(203, 0, 113, 12)),
                1,
                2,
                b"ping",
            ),
            IpAddr::V4(APP4_IP),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 12)),
            28,
        ),
        (
            with_fragment_field(
                segment_v4(app, addr("203.0.113.13:80"), 13, 14, 0x10),
                MORE_FRAGMENTS,
            ),
            IpAddr::V4(APP4_IP),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 13)),
            28,
        ),
        (
            packets::udp(app6, addr("[2001:db8::5]:9000"), b"hello"),
            IpAddr::V6(APP6_IP),
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 5)),
            53,
        ),
    ];
    for (packet, dst, src, quoted) in cases {
        let whole = packet.len();
        lab.rig.app.inject(packet.clone());
        let (from, to, quote) = lab.prohibited().await;
        assert_eq!((from, to), (src, dst), "the message's ends");
        assert_eq!(
            quote,
            packet[..quoted],
            "the quote of a {whole}-byte packet"
        );
    }
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 5,
            ..Counters::default()
        }
    );
}

/// An eight-byte IPv6 extension header whose next header is `next`, its length
/// field zero (one eight-byte unit) and the rest padding.
fn extension(next: u8) -> Vec<u8> {
    vec![next, 0, 0, 0, 0, 0, 0, 0]
}

/// An eight-byte IPv6 fragment header: `next`, the offset in units of eight
/// bytes with the more-fragments bit, and a fixed identification.
fn fragment(next: u8, offset: u16, more: bool) -> Vec<u8> {
    let mut header = vec![next, 0];
    header.extend((offset << 3 | u16::from(more)).to_be_bytes());
    header.extend([0, 0, 0, 1]);
    header
}

/// `[IP Rewrite]`'s `reject` does not walk a chain of IPv6 extension headers:
/// what comes after one (beyond the one fragment header it reads) cannot be
/// read, so nothing is answered, as with any type it cannot read (M5 design
/// 8.5 and 21, plan P11 and its execution record row 70). An ICMPv6
/// redirect (type 137) is answered with nothing too
/// (RFC 4443 2.4(e)). Pure function: no TUN, no network, no files.
#[test]
fn refusal_reads_past_no_ipv6_extension_header() {
    let src = APP6_IP;
    let dst = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 20);
    let udp = [0x13, 0x88, 0x23, 0x28, 0, 9, 0, 0];
    let icmp = |kind: u8| vec![kind, 0, 0, 0, 0, 0, 0, 0];
    let tcp: Vec<u8> = {
        let syn = syn_v6(addr("[fd00:6152::1]:5000"), addr("[2001:db8::23]:80"));
        syn[40..].to_vec()
    };
    let mut hop = extension(58);
    hop.extend(icmp(1));
    let mut options = extension(17);
    options.extend(udp);
    let mut routing = extension(6);
    routing.extend(tcp);
    let mut first_then_options = fragment(60, 0, true);
    first_then_options.extend(extension(17));
    first_then_options.extend(udp);
    let mut experimental = extension(58);
    experimental.extend(icmp(128));
    let mut first_then_redirect = fragment(58, 0, true);
    first_then_redirect.extend(icmp(137));
    let mut first_then_udp = fragment(17, 0, true);
    first_then_udp.extend(udp);
    let cases = [
        (
            "Hop-by-Hop, then an ICMPv6 error",
            ipv6_packet(0, src, dst, &hop),
            Refusal::Nothing,
        ),
        (
            "Destination Options, then UDP",
            ipv6_packet(60, src, dst, &options),
            Refusal::Nothing,
        ),
        (
            "Routing, then a TCP SYN",
            ipv6_packet(43, src, dst, &routing),
            Refusal::Nothing,
        ),
        (
            "a first fragment, then Destination Options",
            ipv6_packet(44, src, dst, &first_then_options),
            Refusal::Nothing,
        ),
        (
            "next header 253",
            ipv6_packet(253, src, dst, &experimental),
            Refusal::Nothing,
        ),
        (
            "an ICMPv6 redirect",
            ipv6_packet(58, src, dst, &icmp(137)),
            Refusal::Nothing,
        ),
        (
            "a first fragment, then an ICMPv6 redirect",
            ipv6_packet(44, src, dst, &first_then_redirect),
            Refusal::Nothing,
        ),
        (
            "a fragment header cut short",
            ipv6_packet(44, src, dst, &[0u8; 4]),
            Refusal::Nothing,
        ),
        (
            "an ICMPv6 echo request",
            ipv6_packet(58, src, dst, &icmp(128)),
            Refusal::Prohibited,
        ),
        (
            "an ICMPv6 error",
            ipv6_packet(58, src, dst, &icmp(1)),
            Refusal::Nothing,
        ),
        (
            "a first fragment, then UDP",
            ipv6_packet(44, src, dst, &first_then_udp),
            Refusal::Prohibited,
        ),
        (
            "an unfragmented UDP datagram",
            ipv6_packet(17, src, dst, &udp),
            Refusal::Prohibited,
        ),
        (
            "No Next Header",
            ipv6_packet(59, src, dst, &[0u8; 8]),
            Refusal::Prohibited,
        ),
    ];
    for (what, packet, expected) in cases {
        assert_eq!(refusal(&packet), expected, "{what}");
    }
}

/// An IPv6 packet is quoted up to what fits in the smallest MTU every IPv6
/// link has, 1280 bytes, less the IPv6 and ICMPv6 headers: 1232 bytes (RFC
/// 4443 2.4(c)). A fragment header can sit before the transport header, so the
/// quote cannot stop at the IP header and eight bytes as IPv4's does (the
/// phase 3 / M5 plan's execution record row 71; P11 quoted the header and
/// eight bytes for both families).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reject_quotes_an_ipv6_packet_up_to_the_minimum_mtu() {
    let mut lab = lab_with_table("203.0.113.0/24 = reject\n2001:db8::/32 = reject\n");
    // a first fragment: fragment header (UDP, offset 0, more fragments), then
    // the UDP header's ports at bytes 48 to 52 of the packet, then a payload
    let mut first = vec![17, 0, 0, 1, 0, 0, 0, 1];
    first.extend([0x13, 0x88, 0x23, 0x28, 0, 9, 0, 0]);
    first.extend(b"hello");
    let fragmented = ipv6_packet(
        44,
        APP6_IP,
        Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 20),
        &first,
    );
    // an unfragmented IPv6 datagram longer than the quote
    let mut long6 = vec![0u8; 8];
    long6.extend([0x55u8; 1400]);
    let long = ipv6_packet(
        17,
        APP6_IP,
        Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 21),
        &long6,
    );
    // the same length over IPv4: its quote is still the header and eight bytes
    let mut long4 = vec![0u8; 8];
    long4.extend([0x55u8; 1400]);
    let v4 = ipv4_packet(17, APP4_IP, Ipv4Addr::new(203, 0, 113, 22), &long4);

    lab.rig.app.inject(fragmented.clone());
    let (from, to, quote) = lab.prohibited().await;
    assert_eq!(
        (from, to),
        (
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 20)),
            IpAddr::V6(APP6_IP),
        ),
        "the fragment's ends"
    );
    assert_eq!(quote, fragmented, "a first fragment is quoted whole");
    assert_eq!(
        quote[48..52],
        [0x13, 0x88, 0x23, 0x28],
        "the UDP header's ports are in the quote"
    );

    lab.rig.app.inject(long.clone());
    let (_, _, quote) = lab.prohibited().await;
    assert_eq!(quote.len(), 1232, "the quote of a long IPv6 packet");
    assert_eq!(quote, long[..1232], "the first 1232 bytes of the packet");

    lab.rig.app.inject(v4.clone());
    let (_, _, quote) = lab.prohibited().await;
    assert_eq!(quote, v4[..28], "IPv4's header and eight bytes");

    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "another answer");
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 3,
            ..Counters::default()
        }
    );
}

/// An ICMP error is answered with nothing: an error for an error would be an
/// exchange that never ends (RFC 1122 3.2.2, RFC 4443 2.4). The packets are
/// counted all the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reject_answers_no_error_with_an_error() {
    let mut lab = lab_with_table("203.0.113.0/24 = reject\n2001:db8::/32 = reject\n");
    let error4 = ipv4_packet(
        1,
        APP4_IP,
        Ipv4Addr::new(203, 0, 113, 10),
        &[3, 0, 0, 0, 0, 0, 0, 0],
    );
    let error6 = ipv6_packet(
        58,
        APP6_IP,
        Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 10),
        &[1, 0, 0, 0, 0, 0, 0, 0],
    );
    lab.rig.app.inject(error4);
    lab.rig.app.inject(error6);
    eventually("the two packets to be counted", WITHIN, || {
        lab.counters().ip_rewrite == 2
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "an answer to an error");
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 2,
            ..Counters::default()
        }
    );
}

/// A reset is answered with nothing: never a reset for a reset.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reject_answers_no_reset_with_a_reset() {
    let mut lab = lab_with_table("203.0.113.9 = reject\n");
    lab.rig
        .app
        .inject(segment_v4(addr(APP4), addr("203.0.113.9:80"), 1, 0, 0x04));
    eventually("the reset to be counted", WITHIN, || {
        lab.counters().ip_rewrite == 1
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "an answer to a reset");
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 1,
            ..Counters::default()
        }
    );
}

/// A later fragment, IPv4 or IPv6, is answered with nothing: its transport
/// header is not in it, and only the first fragment answers (RFC 1122 1812).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reject_answers_no_later_fragment() {
    let mut lab = lab_with_table("203.0.113.0/24 = reject\n2001:db8::/32 = reject\n");
    let later4 = with_fragment_field(
        segment_v4(addr(APP4), addr("203.0.113.9:80"), 1, 0, 0x02),
        LATER_FRAGMENT,
    );
    // next header 17, offset 1 (eight bytes) and no more fragments
    let mut payload = vec![17, 0, 0, 8, 0, 0, 0, 1];
    payload.extend([0u8; 8]);
    let later6 = ipv6_packet(
        44,
        APP6_IP,
        Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 9),
        &payload,
    );
    lab.rig.app.inject(later4);
    lab.rig.app.inject(later6);
    eventually("the two fragments to be counted", WITHIN, || {
        lab.counters().ip_rewrite == 2
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "an answer to a later fragment");
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 2,
            ..Counters::default()
        }
    );
}

/// A TCP header that cannot be read is answered with nothing: there is no SYN
/// to build a reset from.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reject_answers_nothing_to_an_unreadable_tcp_header() {
    let mut lab = lab_with_table("203.0.113.9 = reject\n");
    lab.rig.app.inject(ipv4_packet(
        6,
        APP4_IP,
        Ipv4Addr::new(203, 0, 113, 9),
        &[0xff; 12],
    ));
    eventually("the packet to be counted", WITHIN, || {
        lab.counters().ip_rewrite == 1
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "an answer to an unreadable header");
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 1,
            ..Counters::default()
        }
    );
}

/// The resets `[IP Rewrite]` answers SYNs with come out of the TUN's own limit,
/// the one the shards use: fifty resets the TUN made for its own address leave
/// room for fifty of the table's, and the fifty-first is held back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reject_resets_share_the_tuns_limit() {
    let lab = lab_and_extras(
        TunOptions {
            writer_queue: 400,
            ..TunOptions::default()
        },
        with_table("203.0.113.9 = reject\n"),
    )
    .0;
    let mut replies = lab.rig.app.divert(40000..=40050);
    // fifty SYNs to an address of the TUN's own: it answers them itself
    for port in 40000..40050 {
        lab.rig
            .app
            .inject(syn_v4(sock(ADAPTER_V4, port), addr("198.18.0.1:80")));
    }
    for number in 0..50 {
        let reply = within("a reset for the TUN's own address", replies.recv())
            .await
            .expect("the app's stack is there");
        assert!(segment(&reply).expect("a segment").rst, "reset {number}");
    }
    // fifty-one SYNs to a `reject` address: fifty more resets, the last held
    for port in 40000..40051 {
        lab.rig
            .app
            .inject(syn_v4(sock(ADAPTER_V4, port), addr("203.0.113.9:80")));
    }
    for number in 0..50 {
        let reply = within("a reset for the rejected address", replies.recv())
            .await
            .expect("the app's stack is there");
        assert!(segment(&reply).expect("a segment").rst, "reset {number}");
    }
    eventually("the fifty-first reset to be held back", WITHIN, || {
        lab.counters().rst_limited == 1
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert!(replies.try_recv().is_err(), "no fifty-first reset");
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 51,
            rst_limited: 1,
            ..Counters::default()
        }
    );
}

/// The ICMP messages `[IP Rewrite]` sends come out of the TUN's own limit on
/// them, the one the refused UDP flows use: sixty messages for refused flows
/// leave room for forty of the table's, and the forty-first is held back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reject_messages_share_the_icmp_limit() {
    let mut lab = lab_and_extras(
        TunOptions {
            writer_queue: 400,
            ..TunOptions::default()
        },
        with_table("203.0.113.0/24 = reject\n"),
    )
    .0;
    let app = addr(APP4);
    let refused = addr("192.0.2.10:9");
    lab.dialer()
        .script_udp(target(refused), UdpScript::Refuse(RejectKind::Reject));
    for _ in 0..60 {
        lab.send(app, refused, b"again");
    }
    for number in 0..60 {
        let (src, _, _) = lab.prohibited().await;
        assert_eq!(src, refused.ip(), "message {number}");
    }
    for _ in 0..41 {
        lab.send(app, addr("203.0.113.9:9"), b"again");
    }
    for number in 0..40 {
        let (src, _, _) = lab.prohibited().await;
        assert_eq!(
            src,
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)),
            "message {number}"
        );
    }
    eventually("the forty-first message to be held back", WITHIN, || {
        lab.counters().icmp_limited == 1
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert!(lab.no_icmp(), "a forty-first message");
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 41,
            icmp_limited: 1,
            ..Counters::default()
        }
    );
}

/// `drop` writes nothing and dials nothing; the packet is counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drop_drops_and_counts() {
    let mut lab = lab_with_table("203.0.113.9 = drop\n");
    lab.send(addr(APP4), addr("203.0.113.9:9000"), b"dropped");
    eventually("the datagram to be counted", WITHIN, || {
        lab.counters().ip_rewrite == 1
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "anything for the app");
    assert!(lab.dialer().udp_sent().is_empty(), "nothing was dialed");
    assert_eq!(lab.dialer().associations_open(), 0);
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 1,
            ..Counters::default()
        }
    );
}

/// The first line of the table whose prefix holds the destination decides: a
/// line of a single address before the network's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_line_decides() {
    let mut lab = lab_with_table("10.0.0.1 = reflect\n10.0.0.0/8 = drop\n");
    let reflected = packets::udp(addr(APP4), addr("10.0.0.1:9000"), b"back");
    lab.rig.app.inject(reflected.clone());
    let back = lab.packet().await;
    assert_eq!(back, swapped(&reflected));
    lab.send(addr(APP4), addr("10.0.0.2:9000"), b"dropped");
    eventually("the second packet to be counted", WITHIN, || {
        lab.counters().ip_rewrite == 1
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "a second packet for the app");
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 1,
            reflected: 1,
            ..Counters::default()
        }
    );
}

/// An IPv6 line never matches while IPv6 is off: the packet is turned away on
/// its version before the table is asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ipv6_lines_never_match_while_ipv6_is_off() {
    let lab = lab_and_extras(
        TunOptions {
            ipv6: false,
            ..TunOptions::default()
        },
        with_table("2001:db8::/32 = drop\n"),
    )
    .0;
    lab.send(addr(APP6), addr("[2001:db8::5]:9000"), b"dropped");
    eventually("the datagram to be counted unsupported", WITHIN, || {
        lab.counters().unsupported == 1
    })
    .await;
    assert_eq!(
        lab.counters(),
        Counters {
            unsupported: 1,
            ..Counters::default()
        }
    );
}

/// A fake address is an address like any other: the table takes it as written,
/// without asking the name behind it, so no session is made and nothing is
/// dialed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fake_address_is_matched_as_an_address() {
    let mut lab = lab_with_table("198.18.0.0/15 = drop\n");
    let index = lab
        .pool()
        .assign("fake.test", Instant::now())
        .expect("an address for fake.test");
    lab.send(addr(APP4), sock(v4_of(index), 9000), b"knock");
    eventually("the datagram to be counted", WITHIN, || {
        lab.counters().ip_rewrite == 1
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "anything for the app");
    assert!(lab.dialer().udp_sent().is_empty(), "nothing was dialed");
    assert_eq!(lab.dialer().associations_open(), 0, "no session was made");
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 1,
            ..Counters::default()
        }
    );
}

/// An address no packet has is a martian before it is a line of the table: a
/// table of `0.0.0.0/0` does not take a packet to a multicast address.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn martians_come_before_ip_rewrite() {
    let lab = lab_with_table("0.0.0.0/0 = drop\n");
    lab.rig.app.inject(syn_v4(addr(APP4), addr("224.0.0.1:80")));
    eventually("the packet to be counted a martian", WITHIN, || {
        lab.counters().martian == 1
    })
    .await;
    assert_eq!(
        lab.counters(),
        Counters {
            martian: 1,
            ..Counters::default()
        }
    );
}

/// A fragment is looked at after the table: one the table takes has its
/// action, one it does not is counted `fragment` as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fragments_come_after_ip_rewrite() {
    let mut lab = lab_with_table("203.0.113.1 = drop\n");
    let matched = with_fragment_field(
        packets::udp(addr(APP4), addr("203.0.113.1:9000"), b"x"),
        MORE_FRAGMENTS,
    );
    let other = with_fragment_field(
        segment_v4(addr(APP4), addr("192.0.2.9:80"), 1, 0, 0x02),
        LATER_FRAGMENT,
    );
    lab.rig.app.inject(matched);
    lab.rig.app.inject(other);
    eventually("the two packets to be counted", WITHIN, || {
        let counters = lab.counters();
        counters.ip_rewrite == 1 && counters.fragment == 1
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "anything for the app");
    assert_eq!(
        lab.counters(),
        Counters {
            fragment: 1,
            ip_rewrite: 1,
            ..Counters::default()
        }
    );
}

/// A new generation's table is the one the next batch is handled with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_generation_brings_its_table_with_the_next_batch() {
    let (mut lab, extras) =
        lab_and_extras(TunOptions::default(), with_table("203.0.113.9 = drop\n"));
    lab.send(addr(APP4), addr("203.0.113.9:9000"), b"dropped");
    eventually("the first packet to be counted", WITHIN, || {
        lab.counters().ip_rewrite == 1
    })
    .await;
    set_table(&extras, "203.0.113.9 = reflect\n");
    let reflected = packets::udp(addr(APP4), addr("203.0.113.9:9000"), b"back");
    lab.rig.app.inject(reflected.clone());
    let back = lab.packet().await;
    assert_eq!(back, swapped(&reflected));
    eventually("the second packet to be counted", WITHIN, || {
        lab.counters().reflected == 1
    })
    .await;
    assert_eq!(
        lab.counters(),
        Counters {
            ip_rewrite: 1,
            reflected: 1,
            ..Counters::default()
        }
    );
}

/// A packet `[IP Rewrite]` drops is said at DEBUG every time, with its two
/// addresses, and never at WARN: it is what the profile asked for.
#[tokio::test]
async fn ip_rewrite_drops_are_said_at_debug_never_warned() {
    let log = LogCapture::start();
    let lab = lab_with_table("203.0.113.0/24 = drop\n");
    lab.send(addr(APP4), addr("203.0.113.9:9000"), b"one");
    lab.send(addr(APP4), addr("203.0.113.10:9000"), b"two");
    eventually("the packets to be counted", WITHIN, || {
        lab.counters().ip_rewrite == 2
    })
    .await;
    let lines = log.lines();
    assert!(
        !lines
            .iter()
            .any(|line| line.starts_with("WARN") && line.contains("tun:")),
        "{lines:?}"
    );
    let drops: Vec<&String> = lines
        .iter()
        .filter(|line| line.starts_with("DEBUG") && line.contains("tun: packet dropped"))
        .collect();
    assert_eq!(
        drops,
        [
            "DEBUG tun: packet dropped reason=ip_rewrite src=198.18.0.1 dst=203.0.113.9",
            "DEBUG tun: packet dropped reason=ip_rewrite src=198.18.0.1 dst=203.0.113.10",
        ],
        "{lines:?}"
    );
}

/// A packet `[IP Rewrite]` sends back is no drop and is not logged: the traffic
/// of a `reflect` line may be large.
#[tokio::test]
async fn reflected_packets_are_not_logged() {
    let log = LogCapture::start();
    let mut lab = lab_with_table("203.0.113.9 = reflect\n");
    let packet = packets::udp(addr(APP4), addr("203.0.113.9:9000"), b"hello");
    lab.rig.app.inject(packet);
    let _ = lab.packet().await;
    eventually("the packet to be counted", WITHIN, || {
        lab.counters().reflected == 1
    })
    .await;
    assert!(
        !log.lines().iter().any(|line| line.contains("tun:")),
        "{:?}",
        log.lines()
    );
}
