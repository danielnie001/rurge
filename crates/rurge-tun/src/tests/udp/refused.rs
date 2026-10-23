//! Flows the engine refuses, and the ICMP message "administratively prohibited"
//! an app is told them by.

use super::{APP4, APP6, FAR4, FAR6, NOTHING, lab, lab_with, named, target_of};
use crate::icmp::prohibited;
use crate::testing::UdpScript;
use crate::testing::packets::{self, IcmpSeen};
use crate::tests::support::{Extras, WITHIN, addr, eventually, sock, with_ip_options};
use crate::{Counters, TunOptions};
use rurge_dns::fakeip::{FakeIpPool, v4_of, v6_of};
use rurge_proto::RejectKind;
use smoltcp::wire::{
    Icmpv4Message, Icmpv4Packet, Icmpv6Message, Icmpv6Packet, IpProtocol, Ipv4Packet, Ipv6Packet,
};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;

/// The options of a test that counts ICMP messages: room in the writer's queue
/// for all of them, since the receive task and `refused` hand them over
/// without waiting and a full queue loses them.
fn counting_icmp() -> TunOptions {
    TunOptions {
        writer_queue: 400,
        ..TunOptions::default()
    }
}

/// A flow the engine refuses is answered with an ICMP message, "administratively
/// prohibited", from the address the app sent to (a fake address as well as a
/// real one), quoting the IP header and the UDP header of the datagram; except
/// `REJECT-DROP`, which is answered with nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refused_targets_answer_with_icmp() {
    let mut lab = lab(counting_icmp());
    let index = lab
        .pool()
        .assign("a.test", Instant::now())
        .expect("an address for a.test");
    let name = named("a.test", 53);
    for (app, real, fake, headers) in [
        (addr(APP4), addr(FAR4), sock(v4_of(index), 53), 20 + 8),
        (addr(APP6), addr(FAR6), sock(v6_of(index), 53), 40 + 8),
    ] {
        for kind in [RejectKind::Reject, RejectKind::NoDrop, RejectKind::TinyGif] {
            lab.dialer()
                .script_udp(target_of(real), UdpScript::Refuse(kind));
            lab.dialer()
                .script_udp(name.clone(), UdpScript::Refuse(kind));
            for dst in [real, fake] {
                let packet = packets::udp(app, dst, b"refused");
                lab.rig.app.inject(packet.clone());
                let (src, to, quoted) = lab.prohibited().await;
                assert_eq!((src, to), (dst.ip(), app.ip()), "{kind:?} to {dst}");
                assert_eq!(quoted, packet[..headers], "{kind:?} to {dst}");
            }
        }
    }
    assert_eq!(lab.counters(), Counters::default());
    // a header with options is quoted whole
    let (app, dst) = (addr(APP4), addr(FAR4));
    lab.dialer()
        .script_udp(target_of(dst), UdpScript::Refuse(RejectKind::NoDrop));
    let packet = with_ip_options(&packets::udp(app, dst, b"options"), &[1, 1, 1, 0]);
    lab.rig.app.inject(packet.clone());
    let (_, _, quoted) = lab.prohibited().await;
    assert_eq!(quoted, packet[..24 + 8]);
}

/// `REJECT-DROP` is answered with nothing at all; the datagram did reach the
/// dialer, which told the client.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_flow_is_answered_with_nothing() {
    let mut lab = lab(counting_icmp());
    for (app, dst) in [(addr(APP4), addr(FAR4)), (addr(APP6), addr(FAR6))] {
        lab.dialer()
            .script_udp(target_of(dst), UdpScript::Refuse(RejectKind::Drop));
        lab.send(app, dst, b"dropped");
    }
    eventually("the datagrams to be taken", WITHIN, || {
        lab.dialer().udp_sent().len() == 2
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert!(lab.no_icmp(), "an ICMP message for the app");
    assert_eq!(lab.counters(), Counters::default());
}

/// An app that goes on sending to flows that are refused (a QUIC client trying
/// again) is answered a hundred times in three seconds, no more, whether the
/// flows are refused with `REJECT` or with `REJECT-TINYGIF`: the limit is one for
/// both. The ICMP of `REJECT-NO-DROP` has no limit, nor does it count against
/// the other's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn icmp_for_refused_flows_is_limited() {
    let mut lab = lab(counting_icmp());
    let app = addr(APP4);
    let (rejected, tiny, unlimited) = (
        addr("192.0.2.10:9"),
        addr("192.0.2.12:9"),
        addr("192.0.2.11:9"),
    );
    lab.dialer()
        .script_udp(target_of(rejected), UdpScript::Refuse(RejectKind::Reject));
    lab.dialer()
        .script_udp(target_of(tiny), UdpScript::Refuse(RejectKind::TinyGif));
    lab.dialer()
        .script_udp(target_of(unlimited), UdpScript::Refuse(RejectKind::NoDrop));
    for _ in 0..60 {
        lab.send(app, rejected, b"again");
    }
    for _ in 0..41 {
        lab.send(app, tiny, b"again");
    }
    eventually("one message to be held back", WITHIN, || {
        lab.counters().icmp_limited == 1
    })
    .await;
    for _ in 0..150 {
        lab.send(app, unlimited, b"again");
    }
    // the messages come in the order of the datagrams: sixty for the first flow,
    // forty for the second (the forty-first was held back), and then a hundred
    // and fifty for the third, with none of the second's between if the limit held
    for number in 0..60 {
        let (src, _, _) = lab.prohibited().await;
        assert_eq!(src, rejected.ip(), "message {number}");
    }
    for number in 0..40 {
        let (src, _, _) = lab.prohibited().await;
        assert_eq!(src, tiny.ip(), "message {number} of the second");
    }
    for number in 0..150 {
        let (src, _, _) = lab.prohibited().await;
        assert_eq!(src, unlimited.ip(), "message {number} of the third");
    }
    assert_eq!(
        lab.counters(),
        Counters {
            icmp_limited: 1,
            ..Counters::default()
        }
    );
}

/// The addresses of rurge's own are not places a datagram goes to: the app is
/// told so, without a limit and without a session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_to_rurge_addresses_is_prohibited_unlimited() {
    let mut lab = lab(counting_icmp());
    let (app4, app6) = (addr(APP4), addr(APP6));
    for (app, dst, headers) in [
        (app4, addr("198.18.0.1:9"), 28),
        (app4, addr("198.18.0.2:54"), 28),
        (app4, addr("198.18.0.244:9"), 28),
        (app4, addr("198.18.0.253:9"), 28),
        (app6, addr("[fd00:6152::fd]:9"), 48),
        (app6, addr("[fd00:6152::2]:54"), 48),
    ] {
        let packet = packets::udp(app, dst, b"hello");
        lab.rig.app.inject(packet.clone());
        let (src, to, quoted) = lab.prohibited().await;
        assert_eq!((src, to), (dst.ip(), app.ip()), "to {dst}");
        assert_eq!(quoted, packet[..headers], "to {dst}");
    }
    for _ in 0..150 {
        lab.send(app4, addr("198.18.0.244:9"), b"again");
    }
    for number in 0..150 {
        let (src, _, _) = lab.prohibited().await;
        assert_eq!(src, addr("198.18.0.244:9").ip(), "message {number}");
    }
    assert!(lab.dialer().associations().is_empty());
    assert_eq!(lab.counters(), Counters::default());
}

/// An address of the pool's space that no name has names nothing: the datagram
/// is dropped and counted, with no session made for it and no ICMP message.
/// So is every fake address when the TUN has no pool.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_to_an_unknown_fake_address_is_dropped() {
    for pool in [Some(Arc::new(FakeIpPool::new())), None] {
        let had_pool = pool.is_some();
        let mut lab = lab_with(
            TunOptions::default(),
            Extras {
                pool,
                ..Extras::default()
            },
        );
        lab.send(addr(APP4), sock(v4_of(7), 53), b"who?");
        lab.send(addr(APP6), sock(v6_of(7), 53), b"who?");
        eventually("the datagrams to be counted", WITHIN, || {
            lab.counters().unknown_fake == 2
        })
        .await;
        tokio::time::sleep(NOTHING).await;
        assert!(
            lab.dialer().associations().is_empty(),
            "(a pool: {had_pool})"
        );
        assert!(lab.no_icmp(), "(a pool: {had_pool})");
        assert_eq!(
            lab.counters(),
            Counters {
                unknown_fake: 2,
                ..Counters::default()
            }
        );
    }
}

/// The message "administratively prohibited": ICMPv4's type 3 code 13 and
/// ICMPv6's type 1 code 1, from one address to another, whose body is the
/// quoted header and nothing else, and whose checksums hold (ICMPv6's with the
/// pseudo-header).
#[test]
fn the_prohibited_message_quotes_the_header() {
    // IPv4: the header of 20 bytes and the UDP header of the datagram
    let (app, dst) = (addr(APP4), addr("192.0.2.10:9"));
    let original = packets::udp(app, dst, b"payload");
    let quoted = &original[..28];
    let packet = prohibited(dst.ip(), app.ip(), quoted);
    let ip = Ipv4Packet::new_checked(&packet[..]).expect("an IPv4 packet");
    assert!(ip.verify_checksum());
    assert_eq!(IpAddr::from(ip.src_addr()), dst.ip());
    assert_eq!(IpAddr::from(ip.dst_addr()), app.ip());
    assert_eq!(ip.next_header(), IpProtocol::Icmp);
    assert_eq!(ip.hop_limit(), 64);
    assert_eq!(usize::from(ip.total_len()), 20 + 8 + 28);
    assert!(!ip.more_frags() && ip.frag_offset() == 0);
    let icmp = Icmpv4Packet::new_checked(ip.payload()).expect("an ICMPv4 message");
    assert_eq!(icmp.msg_type(), Icmpv4Message::DstUnreachable);
    assert_eq!(icmp.msg_code(), 13);
    assert!(icmp.verify_checksum());
    assert_eq!(icmp.data(), quoted);
    assert_eq!(
        packets::read_icmp(&packet),
        Some(IcmpSeen::Prohibited {
            src: dst.ip(),
            dst: app.ip(),
            quoted: quoted.to_vec()
        })
    );

    // IPv6: the base header of 40 bytes and the UDP header
    let (app, dst) = (addr(APP6), addr("[2001:db8::10]:9"));
    let original = packets::udp(app, dst, b"payload");
    let quoted = &original[..48];
    let packet = prohibited(dst.ip(), app.ip(), quoted);
    let ip = Ipv6Packet::new_checked(&packet[..]).expect("an IPv6 packet");
    assert_eq!(IpAddr::from(ip.src_addr()), dst.ip());
    assert_eq!(IpAddr::from(ip.dst_addr()), app.ip());
    assert_eq!(ip.next_header(), IpProtocol::Icmpv6);
    assert_eq!(ip.hop_limit(), 64);
    assert_eq!(ip.payload().len(), 8 + 48);
    let (from, to) = match (dst.ip(), app.ip()) {
        (IpAddr::V6(from), IpAddr::V6(to)) => (from, to),
        _ => unreachable!("the addresses are of one family"),
    };
    let icmp = Icmpv6Packet::new_checked(ip.payload()).expect("an ICMPv6 message");
    assert_eq!(icmp.msg_type(), Icmpv6Message::DstUnreachable);
    assert_eq!(icmp.msg_code(), 1);
    assert!(icmp.verify_checksum(&from, &to));
    assert_eq!(icmp.payload(), quoted);
    assert_eq!(
        packets::read_icmp(&packet),
        Some(IcmpSeen::Prohibited {
            src: dst.ip(),
            dst: app.ip(),
            quoted: quoted.to_vec()
        })
    );
}
