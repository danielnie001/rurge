//! Where a new SYN goes (phase 3 M3 design 5.2) and what limits it. First the
//! parts that stand alone: the addresses of rurge's space, the limiter, the
//! reset rurge makes, the caps shards take places under at once. Then one
//! shard at a time, its turns taken by hand, where what a turn gives out and
//! what it keeps can be read exactly. Then the whole TUN, with SYNs the test
//! sends itself (`inject`) and the answers it takes off the app's stack
//! (`divert`), so that a reset is seen as the packet it is.

use super::filter::warning;
use super::log_capture::LogCapture;
use super::support::{
    WITHIN, addr, checksum, eventually, hello, is_reset, is_syn, lone_shard, rig, segment_v4, sock,
    syn_v4, syn_v6, tcp_header, tcp_packet, tcp_repr, with_ip_options, within,
};
use crate::limit::Limiter;
use crate::packet::{Reason, rst_for, segment};
use crate::space::{ADAPTER_V4, ADAPTER_V6, Place, REFUSED_V4, STACK_V4, STACK_V6, place};
use crate::stats::Stats;
use crate::tcp::{Admission, Outcome, Tuple};
use crate::testing::{Direction, Script, Upstream};
use crate::{Counters, Ending, Life, TunOptions};
use smoltcp::wire::{
    IpProtocol, Ipv4Packet, Ipv6Packet, TcpControl, TcpPacket, TcpRepr, TcpSeqNumber,
    TcpTimestampRepr,
};
use std::collections::HashSet;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

// ---- the parts that stand alone

fn ip(text: &str) -> IpAddr {
    text.parse().expect("an IP address")
}

#[test]
fn places_are_told_by_address() {
    let cases = [
        // IPv4: the responder's eight addresses, the rest of the /24 is
        // rurge's own, the rest of the /15 stands for names
        ("198.17.255.255", Place::Real),
        ("198.18.0.0", Place::Own),
        ("198.18.0.1", Place::Own),
        ("198.18.0.2", Place::Responder),
        ("198.18.0.9", Place::Responder),
        ("198.18.0.10", Place::Own),
        ("198.18.0.244", Place::Own),
        ("198.18.0.253", Place::Own),
        ("198.18.0.255", Place::Own),
        ("198.18.1.0", Place::Fake),
        ("198.18.1.1", Place::Fake),
        ("198.19.255.254", Place::Fake),
        ("198.19.255.255", Place::Fake),
        ("198.20.0.0", Place::Real),
        ("192.0.2.1", Place::Real),
        ("10.0.0.1", Place::Real),
        // IPv6: one responder address, the /120 is rurge's own, the rest of
        // the /64 stands for names
        ("fd00:6152::", Place::Own),
        ("fd00:6152::1", Place::Own),
        ("fd00:6152::2", Place::Responder),
        ("fd00:6152::3", Place::Own),
        ("fd00:6152::fd", Place::Own),
        ("fd00:6152::ff", Place::Own),
        ("fd00:6152::100", Place::Fake),
        ("fd00:6152::c612:101", Place::Fake),
        ("fd00:6152::ffff:ffff", Place::Fake),
        ("fd00:6152:0:0:1::", Place::Fake),
        ("fd00:6152:0:1::", Place::Real),
        ("fd00:6153::1", Place::Real),
        ("fd01:6152::1", Place::Real),
        ("2001:db8::1", Place::Real),
        ("::1", Place::Real),
    ];
    for (address, expected) in cases {
        assert_eq!(place(ip(address)), expected, "{address}");
    }
    // the addresses the code names are rurge's own
    for own in [
        ADAPTER_V4.into(),
        STACK_V4.into(),
        REFUSED_V4.into(),
        ADAPTER_V6.into(),
        STACK_V6.into(),
    ] {
        assert_eq!(place(own), Place::Own, "{own}");
    }
}

#[test]
fn a_limiter_lets_max_through_in_any_window() {
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut limiter = Limiter::new(100, Duration::from_secs(3));
    // a hundred, one in each of the first hundred milliseconds
    for ms in 0..100 {
        assert!(limiter.allow(at(ms)), "number {} of 100", ms + 1);
    }
    // not the 101st, however often it asks, until the first has left the
    // window; what is refused is not remembered
    assert!(!limiter.allow(at(100)));
    assert!(!limiter.allow(at(1_500)));
    assert!(!limiter.allow(at(2_999)));
    // at 3 s the first is 3 s old, out of the window: one more
    assert!(limiter.allow(at(3_000)));
    assert!(!limiter.allow(at(3_000)));
    // the window slides a call at a time: the second leaves at 3.001 s
    assert!(limiter.allow(at(3_001)));
    assert!(!limiter.allow(at(3_001)));
    // after a window with nothing in it all of a hundred are let through again
    for ms in 0..100 {
        assert!(limiter.allow(at(20_000 + ms)), "number {} of 100", ms + 1);
    }
    assert!(!limiter.allow(at(20_100)));
    // and a limiter of none lets none through
    assert!(!Limiter::new(0, Duration::from_secs(3)).allow(start));
}

/// A SYN with the options a real stack sends, and `payload` after them (a
/// SYN may carry data).
fn syn_with_options(app: SocketAddr, dst: SocketAddr, seq: u32, payload: &[u8]) -> Vec<u8> {
    let repr = TcpRepr {
        control: TcpControl::Syn,
        seq_number: TcpSeqNumber(seq as i32),
        max_seg_size: Some(1460),
        window_scale: Some(7),
        sack_permitted: true,
        timestamp: Some(TcpTimestampRepr::new(1, 0)),
        ..tcp_repr(app, dst, payload)
    };
    tcp_packet(app, dst, &repr)
}

/// Whether the checksum of the TCP segment of `packet` holds, worked out
/// from the RFC 1071 sum over the pseudo-header and the segment, not by
/// smoltcp.
fn tcp_checksum_holds(packet: &[u8]) -> bool {
    let (header, total) = tcp_header(packet).expect("a TCP packet");
    let tcp = &packet[header..total];
    let mut summed = Vec::new();
    if packet[0] >> 4 == 4 {
        // the addresses, a zero byte and the protocol, the segment's length
        summed.extend(&packet[12..20]);
        summed.extend([0, 6]);
        summed.extend((tcp.len() as u16).to_be_bytes());
    } else {
        // the addresses, the segment's length, three zero bytes and the protocol
        summed.extend(&packet[8..40]);
        summed.extend((tcp.len() as u32).to_be_bytes());
        summed.extend([0, 0, 0, 6]);
    }
    summed.extend(tcp);
    checksum(&summed) == 0
}

/// `reset` is what answers a SYN from `app` to `dst`: from `dst` to `app`,
/// RST and ACK and nothing else, sequence number 0, acknowledging `ack`,
/// window 0, a TCP header of 20 bytes and nothing after it, a hop limit of
/// 64, both checksums right.
fn assert_answers(reset: &[u8], app: SocketAddr, dst: SocketAddr, ack: u32) {
    let payload = match dst.ip() {
        IpAddr::V4(_) => {
            let ip = Ipv4Packet::new_checked(reset).expect("an IPv4 packet");
            assert_eq!(
                reset.len(),
                40,
                "an IPv4 header and a TCP header, 20 bytes each"
            );
            assert_eq!((ip.header_len(), usize::from(ip.total_len())), (20, 40));
            assert_eq!(IpAddr::from(ip.src_addr()), dst.ip());
            assert_eq!(IpAddr::from(ip.dst_addr()), app.ip());
            assert_eq!(ip.next_header(), IpProtocol::Tcp);
            assert_eq!(ip.hop_limit(), 64);
            assert!(ip.dont_frag() && !ip.more_frags() && ip.frag_offset() == 0);
            assert!(ip.verify_checksum(), "the IPv4 header checksum");
            assert_eq!(checksum(&reset[..20]), 0, "the IPv4 header sums to zero");
            ip.payload()
        }
        IpAddr::V6(_) => {
            let ip = Ipv6Packet::new_checked(reset).expect("an IPv6 packet");
            assert_eq!(reset.len(), 60, "an IPv6 header and a TCP header");
            assert_eq!(ip.payload_len(), 20);
            assert_eq!(IpAddr::from(ip.src_addr()), dst.ip());
            assert_eq!(IpAddr::from(ip.dst_addr()), app.ip());
            assert_eq!(ip.next_header(), IpProtocol::Tcp);
            assert_eq!(ip.hop_limit(), 64);
            ip.payload()
        }
    };
    let header = TcpPacket::new_checked(payload).expect("a TCP segment");
    assert_eq!(
        (header.src_port(), header.dst_port()),
        (dst.port(), app.port())
    );
    assert!(header.rst() && header.ack());
    assert!(!header.syn() && !header.fin() && !header.psh() && !header.urg());
    assert_eq!(header.seq_number().0 as u32, 0);
    assert_eq!(header.ack_number().0 as u32, ack);
    assert_eq!(header.window_len(), 0);
    assert_eq!(header.urgent_at(), 0);
    assert_eq!(
        (header.header_len(), payload.len()),
        (20, 20),
        "no options, no data"
    );
    assert!(
        header.verify_checksum(&dst.ip().into(), &app.ip().into()),
        "the TCP checksum"
    );
    assert!(tcp_checksum_holds(reset), "the TCP checksum sums to zero");
}

#[test]
fn the_reset_answers_the_syn() {
    let v4 = (addr("198.18.0.1:40000"), addr("192.0.2.1:80"));
    let v6 = (addr("[fd00:6152::1]:40000"), addr("[2001:db8::1]:443"));
    // the SYN's sequence number and the bytes it carries: the reset
    // acknowledges the SYN itself and both, wrapping round the end
    let numbers = [
        (1, 0),
        (0x1234_5678, 0),
        (0x1234_5678, 7),
        (u32::MAX, 0),
        (u32::MAX, 5),
        (u32::MAX - 3, 1000),
    ];
    for (app, dst) in [v4, v6] {
        for (seq, carried) in numbers {
            let syn = syn_with_options(app, dst, seq, &vec![0xaa; carried]);
            let ack = seq.wrapping_add(1).wrapping_add(carried as u32);
            assert_answers(&rst_for(&syn), app, dst, ack);
        }
    }
    // the IP options of an IPv4 SYN are not the reset's
    let (app, dst) = v4;
    let syn = with_ip_options(&syn_with_options(app, dst, 9, &[]), &[1, 1, 1, 0]);
    assert_answers(&rst_for(&syn), app, dst, 10);
}

/// The shards of a TUN take their places at the same moment, on threads of
/// their own. However the claims fall, exactly as many succeed as the lower
/// cap allows, and both counts end at it: a raise that reads the count
/// before it adds would let more through. The threads wait until all of them
/// are there so that the claims do fall together.
#[test]
fn claims_at_once_keep_to_the_caps() {
    const ROUNDS: usize = 100;
    const THREADS: usize = 16;
    for round in 0..ROUNDS {
        for (max_sockets, max_half_open) in [(5, 8), (8, 5), (6, 6)] {
            let options = TunOptions {
                max_sockets,
                max_half_open,
                ..TunOptions::default()
            };
            let admission = Admission::new(
                Arc::new(Life::default()),
                Arc::new(Stats::default()),
                &options,
            );
            let ready = AtomicUsize::new(0);
            let go = AtomicBool::new(false);
            let claimed: usize = std::thread::scope(|scope| {
                let claimers: Vec<_> = (0..THREADS)
                    .map(|_| {
                        scope.spawn(|| {
                            ready.fetch_add(1, Ordering::AcqRel);
                            while !go.load(Ordering::Acquire) {
                                std::thread::yield_now();
                            }
                            usize::from(admission.claim())
                        })
                    })
                    .collect();
                while ready.load(Ordering::Acquire) < THREADS {
                    std::thread::yield_now();
                }
                go.store(true, Ordering::Release);
                claimers
                    .into_iter()
                    .map(|claimer| claimer.join().expect("a claimer"))
                    .sum()
            });
            let cap = max_sockets.min(max_half_open);
            let what = format!("round {round}, caps {max_sockets} and {max_half_open}");
            assert_eq!(claimed, cap, "{what}: the claims that succeeded");
            assert_eq!(admission.sockets(), cap, "{what}: the sockets counted");
            assert_eq!(admission.half_open(), cap, "{what}: the half-open counted");
        }
    }
}

/// The same for the places among the connections to the responder: shards
/// that claim at once are given as many as `dns_tcp_max` allows, and a place
/// that is dropped is given back to the count.
#[test]
fn claims_of_dns_places_at_once_keep_to_the_cap() {
    const ROUNDS: usize = 100;
    const THREADS: usize = 16;
    for round in 0..ROUNDS {
        let options = TunOptions {
            dns_tcp_max: 5,
            ..TunOptions::default()
        };
        let admission = Arc::new(Admission::new(
            Arc::new(Life::default()),
            Arc::new(Stats::default()),
            &options,
        ));
        let ready = AtomicUsize::new(0);
        let go = AtomicBool::new(false);
        let places: Vec<_> = std::thread::scope(|scope| {
            let claimers: Vec<_> = (0..THREADS)
                .map(|_| {
                    scope.spawn(|| {
                        ready.fetch_add(1, Ordering::AcqRel);
                        while !go.load(Ordering::Acquire) {
                            std::thread::yield_now();
                        }
                        admission.claim_dns_tcp()
                    })
                })
                .collect();
            while ready.load(Ordering::Acquire) < THREADS {
                std::thread::yield_now();
            }
            go.store(true, Ordering::Release);
            claimers
                .into_iter()
                .filter_map(|claimer| claimer.join().expect("a claimer"))
                .collect()
        });
        assert_eq!(places.len(), 5, "round {round}: the claims that succeeded");
        assert_eq!(admission.dns_tcp(), 5, "round {round}: the places counted");
        drop(places);
        assert_eq!(admission.dns_tcp(), 0, "round {round}: all given back");
        assert!(admission.claim_dns_tcp().is_some(), "round {round}");
    }
}

// ---- one shard, its turns taken by hand

const DST: &str = "192.0.2.1:80";

fn app4(port: u16) -> SocketAddr {
    sock(ADAPTER_V4, port)
}

/// The reset that answers a SYN of sequence number 1 from `app` to `dst`.
fn assert_reset(packet: &[u8], dst: SocketAddr, app: SocketAddr) {
    let reset = segment(packet).expect("a TCP segment");
    assert_eq!(reset.tuple, Tuple { src: dst, dst: app });
    assert!(
        reset.rst && reset.ack && !reset.syn && !reset.fin,
        "{reset:?}"
    );
    assert_eq!(reset.seq, 0);
    assert_eq!(reset.ack_number, 2, "the SYN's sequence number plus one");
}

#[test]
fn stopping_answers_a_syn_with_a_reset() {
    let epoch = Instant::now();
    let lone = lone_shard(&TunOptions::default(), epoch);
    lone.life.end(Ending::Stopped);
    let (app, dst) = (app4(40000), addr(DST));
    let turn = lone.shard.state().turn(epoch, &mut vec![syn_v4(app, dst)]);
    assert!(turn.opened.is_empty(), "no connection is made");
    assert_eq!(turn.out.len(), 1, "one packet is sent");
    assert_reset(&turn.out[0], dst, app);
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.admission.half_open(), 0);
    assert_eq!(
        lone.stats.counters(),
        Counters::default(),
        "nothing to count"
    );
}

/// The reset of a stopping TUN is one of rurge's own like the others: it is
/// let through 100 times in 3 s, and not the 101st.
#[test]
fn the_resets_of_a_stopping_tun_are_limited_too() {
    let epoch = Instant::now();
    let lone = lone_shard(&TunOptions::default(), epoch);
    lone.life.end(Ending::Stopped);
    let dst = addr(DST);
    let mut syns: Vec<Vec<u8>> = (0..101).map(|i| syn_v4(app4(40000 + i), dst)).collect();
    let turn = lone.shard.state().turn(epoch, &mut syns);
    assert!(syns.is_empty(), "all 101 were taken in this turn");
    assert_eq!(turn.out.len(), 100);
    for (i, reset) in turn.out.iter().enumerate() {
        assert_reset(reset, dst, app4(40000 + i as u16));
    }
    assert_eq!(
        lone.stats.counters(),
        Counters {
            rst_limited: 1,
            ..Counters::default()
        }
    );
}

/// The checks run in the order stop, caps, address: a SYN to an address of
/// rurge's own, or one that stands for a name, finds the caps met first.
#[test]
fn the_caps_are_met_before_the_address_is_looked_at() {
    for options in [
        TunOptions {
            max_sockets: 0,
            ..TunOptions::default()
        },
        TunOptions {
            max_half_open: 0,
            ..TunOptions::default()
        },
    ] {
        let epoch = Instant::now();
        let lone = lone_shard(&options, epoch);
        let destinations = [
            addr("198.18.0.1:80"),
            addr("198.18.1.1:80"),
            addr("[fd00:6152::c612:101]:80"),
            addr(DST),
        ];
        let mut syns = Vec::new();
        for (i, dst) in destinations.iter().enumerate() {
            let port = 40000 + i as u16;
            syns.push(match dst {
                SocketAddr::V4(_) => syn_v4(app4(port), *dst),
                SocketAddr::V6(_) => syn_v6(sock(ADAPTER_V6, port), *dst),
            });
        }
        let turn = lone.shard.state().turn(epoch, &mut syns);
        assert!(turn.opened.is_empty());
        assert_eq!(turn.out.len(), destinations.len());
        assert_eq!(
            lone.stats.counters(),
            Counters {
                busy: destinations.len() as u64,
                ..Counters::default()
            },
            "every one is busy; none is counted as a name nobody knows"
        );
        assert_eq!(lone.shard.state().socket_count(), 0);
    }
}

/// A SYN turned away and a reset held back are counted and said as every
/// other drop is: the first of a reason as a WARN with no address, the next
/// as a DEBUG with the SYN's two addresses.
#[test]
fn syns_turned_away_are_said_like_any_other_drop() {
    let log = LogCapture::start();
    let epoch = Instant::now();
    let (app, far) = (app4(40000), addr(DST));
    let take = |options: TunOptions, syns: Vec<Vec<u8>>| {
        let lone = lone_shard(&options, epoch);
        lone.shard.state().turn(epoch, &mut { syns });
    };
    // two SYNs over the cap, then two to a name nobody knows
    take(
        TunOptions {
            max_sockets: 0,
            ..TunOptions::default()
        },
        vec![syn_v4(app, far), syn_v4(app, far)],
    );
    let fake = addr("198.18.1.1:80");
    take(
        TunOptions::default(),
        vec![syn_v4(app, fake), syn_v4(app, fake)],
    );
    // the 101st and the 102nd reset in a window are held back
    let responder = addr("198.18.0.2:54");
    take(
        TunOptions::default(),
        (0..102).map(|_| syn_v4(app, responder)).collect(),
    );
    let said = |reason: Reason, dst: SocketAddr| {
        [
            warning(reason),
            format!(
                "DEBUG tun: packet dropped reason={reason} src={} dst={}",
                app.ip(),
                dst.ip()
            ),
        ]
    };
    let expected: Vec<String> = [
        said(Reason::Busy, far),
        said(Reason::UnknownFake, fake),
        said(Reason::RstLimited, responder),
    ]
    .concat();
    assert_eq!(log.lines(), expected);
}

/// A SYN that is no plain SYN is no new connection and is not answered: with
/// RST or FIN as well as SYN, smoltcp does not read it either, and a reset
/// is never an answer to a reset.
#[test]
fn a_syn_with_another_flag_is_not_answered() {
    let epoch = Instant::now();
    let lone = lone_shard(&TunOptions::default(), epoch);
    let mut segments = Vec::new();
    for (i, dst) in [addr("198.18.0.1:80"), addr(DST)].into_iter().enumerate() {
        for (j, flags) in [0x03, 0x06].into_iter().enumerate() {
            let app = app4(40000 + (2 * i + j) as u16);
            segments.push(segment_v4(app, dst, 1, 0, flags));
        }
    }
    let turn = lone.shard.state().turn(epoch, &mut segments);
    assert!(turn.opened.is_empty());
    assert!(turn.out.is_empty(), "nothing is sent: {:?}", turn.out);
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.stats.counters(), Counters::default());
}

/// smoltcp does not take a SYN whose MSS option is 0, and a socket that
/// listened for it has no connection: it is gone, the place it took is
/// given back, and nothing is sent.
#[test]
fn a_syn_smoltcp_does_not_take_leaves_nothing() {
    let epoch = Instant::now();
    let lone = lone_shard(&TunOptions::default(), epoch);
    let (app, dst) = (app4(40000), addr(DST));
    let repr = TcpRepr {
        control: TcpControl::Syn,
        seq_number: TcpSeqNumber(1),
        max_seg_size: Some(0),
        ..tcp_repr(app, dst, &[])
    };
    let turn = lone
        .shard
        .state()
        .turn(epoch, &mut vec![tcp_packet(app, dst, &repr)]);
    assert!(turn.opened.is_empty());
    assert!(turn.out.is_empty());
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.admission.half_open(), 0);
    assert_eq!(lone.stats.counters(), Counters::default());
}

/// The connections to the responder's port 53 have a cap of their own,
/// `dns_tcp_max`, met after the caps on sockets and half-open connections: a
/// SYN over it is answered with a reset, counted as busy, and gives back the
/// places it claimed in those caps. A place held by a connection comes back
/// when the session of the connection lets it go (here, when the `Opened` that
/// holds it is dropped), and the next SYN takes it.
#[test]
fn a_syn_to_the_responder_over_dns_tcp_max_is_reset_and_holds_nothing() {
    let epoch = Instant::now();
    let options = TunOptions {
        dns_tcp_max: 2,
        ..TunOptions::default()
    };
    let lone = lone_shard(&options, epoch);
    let responder = addr("198.18.0.2:53");
    let mut first = lone.shard.state().turn(
        epoch,
        &mut vec![
            syn_v4(app4(40000), responder),
            syn_v4(app4(40001), responder),
        ],
    );
    assert_eq!(first.opened.len(), 2);
    assert_eq!(lone.admission.dns_tcp(), 2);
    // the third: a reset, one busy, and nothing is held for it
    let app = app4(40002);
    let turn = lone
        .shard
        .state()
        .turn(epoch, &mut vec![syn_v4(app, responder)]);
    assert!(turn.opened.is_empty());
    assert_eq!(turn.out.len(), 1);
    assert_reset(&turn.out[0], responder, app);
    assert_eq!(
        lone.stats.counters(),
        Counters {
            busy: 1,
            ..Counters::default()
        }
    );
    assert_eq!(lone.admission.dns_tcp(), 2);
    assert_eq!(lone.admission.sockets(), 2, "no socket for the third");
    assert_eq!(lone.admission.half_open(), 2, "no half-open place either");
    assert_eq!(lone.shard.state().socket_count(), 2);
    // a place that is given back is the next SYN's
    drop(first.opened.pop());
    assert_eq!(lone.admission.dns_tcp(), 1);
    let app = app4(40003);
    let turn = lone
        .shard
        .state()
        .turn(epoch, &mut vec![syn_v4(app, responder)]);
    assert_eq!(turn.opened.len(), 1);
    assert_eq!(lone.admission.dns_tcp(), 2);
    assert_eq!(lone.stats.counters().busy, 1);
}

/// The cap is on the connections to port 53 of the responder, and on those
/// alone: a connection to another port of it is turned away as it always was,
/// and holds no place; one to port 53 of an address that is not rurge's is no
/// DNS connection (`hijack-dns` is for UDP) and takes none, however many of
/// the others there are.
#[test]
fn only_connections_to_the_responders_port_53_take_a_dns_place() {
    let epoch = Instant::now();
    let options = TunOptions {
        dns_tcp_max: 1,
        ..TunOptions::default()
    };
    let lone = lone_shard(&options, epoch);
    let (responder, other_port, far) = (
        addr("198.18.0.2:53"),
        addr("198.18.0.2:54"),
        addr("192.0.2.53:53"),
    );
    let mut syns = vec![
        syn_v4(app4(40000), other_port),
        syn_v4(app4(40001), far),
        syn_v4(app4(40002), far),
    ];
    let turn = lone.shard.state().turn(epoch, &mut syns);
    assert_eq!(turn.opened.len(), 2, "the two to the far address");
    assert_eq!(lone.admission.dns_tcp(), 0);
    // and with the one place taken, those still go through
    let dns = lone
        .shard
        .state()
        .turn(epoch, &mut vec![syn_v4(app4(40003), responder)]);
    assert_eq!(dns.opened.len(), 1);
    assert_eq!(lone.admission.dns_tcp(), 1);
    let more = lone.shard.state().turn(
        epoch,
        &mut vec![syn_v4(app4(40004), far), syn_v4(app4(40005), other_port)],
    );
    assert_eq!(more.opened.len(), 1, "the far address is still let in");
    assert_eq!(lone.admission.dns_tcp(), 1);
}

/// A SYN to the responder that smoltcp does not take (an MSS of 0) makes no
/// connection, and the place it claimed in the cap on the responder's
/// connections goes back with the rest: the one place there is can still be
/// had.
#[test]
fn a_syn_to_the_responder_smoltcp_does_not_take_gives_its_place_back() {
    let epoch = Instant::now();
    let options = TunOptions {
        dns_tcp_max: 1,
        ..TunOptions::default()
    };
    let lone = lone_shard(&options, epoch);
    let (app, responder) = (app4(40000), addr("198.18.0.2:53"));
    let repr = TcpRepr {
        control: TcpControl::Syn,
        seq_number: TcpSeqNumber(1),
        max_seg_size: Some(0),
        ..tcp_repr(app, responder, &[])
    };
    let turn = lone
        .shard
        .state()
        .turn(epoch, &mut vec![tcp_packet(app, responder, &repr)]);
    assert!(turn.opened.is_empty());
    assert_eq!(lone.admission.dns_tcp(), 0);
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.admission.half_open(), 0);
    assert_eq!(lone.stats.counters(), Counters::default());
    let turn = lone
        .shard
        .state()
        .turn(epoch, &mut vec![syn_v4(app4(40001), responder)]);
    assert_eq!(turn.opened.len(), 1, "the place was given back");
    assert_eq!(lone.admission.dns_tcp(), 1);
}

/// Eight SYNs to one endpoint in one batch: each is a connection of its own,
/// taken one at a time, and each is answered with its own SYN-ACK.
#[test]
fn syns_to_one_endpoint_in_one_batch_open_a_connection_each() {
    let epoch = Instant::now();
    let lone = lone_shard(&TunOptions::default(), epoch);
    let dst = addr("192.0.2.1:443");
    let sources: Vec<SocketAddr> = (0..8).map(|i| app4(40000 + i)).collect();
    let mut syns: Vec<Vec<u8>> = sources.iter().map(|&app| syn_v4(app, dst)).collect();
    let turn = lone.shard.state().turn(epoch, &mut syns);
    let opened: HashSet<Tuple> = turn.opened.iter().map(|opened| opened.tuple).collect();
    let expected: HashSet<Tuple> = sources.iter().map(|&src| Tuple { src, dst }).collect();
    assert_eq!(turn.opened.len(), 8);
    assert_eq!(opened, expected);
    let answered: HashSet<u16> = turn
        .out
        .iter()
        .filter(|packet| is_syn(packet))
        .map(|packet| segment(packet).expect("a segment").tuple.dst.port())
        .collect();
    assert_eq!(answered, sources.iter().map(|app| app.port()).collect());
    assert_eq!(lone.shard.state().socket_count(), 8);
    assert_eq!(lone.admission.sockets(), 8);
    assert_eq!(lone.admission.half_open(), 8);
}

/// A reset in SYN-RECEIVED sends the socket back to listening in smoltcp,
/// where it would take any source's SYN to its destination. It is gone
/// before the next packet is looked at: in the same batch the next SYN to the
/// same destination is a connection of its own, and the old socket is not
/// there to take it.
#[test]
fn a_socket_back_in_listen_is_gone_before_the_next_packet() {
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let (a, b, dst) = (app4(40000), app4(40001), addr(DST));
    let first = lone.shard.state().turn(at(0), &mut vec![syn_v4(a, dst)]);
    assert_eq!(first.opened.len(), 1);
    // A's reset (the sequence number is the SYN's plus one) and B's SYN in
    // one batch
    let second = lone.shard.state().turn(
        at(1),
        &mut vec![segment_v4(a, dst, 2, 0, 0x04), syn_v4(b, dst)],
    );
    assert_eq!(second.opened.len(), 1, "B's SYN made a connection");
    assert_eq!(second.opened[0].tuple, Tuple { src: b, dst });
    let syn_acks: Vec<&Vec<u8>> = second.out.iter().filter(|p| is_syn(p)).collect();
    assert_eq!(syn_acks.len(), 1);
    assert_eq!(
        segment(syn_acks[0]).expect("a segment").tuple,
        Tuple { src: dst, dst: b }
    );
    let state = lone.shard.state();
    assert_eq!(state.socket_count(), 1, "A's socket is gone");
    assert!(!state.by_tuple.contains_key(&Tuple { src: a, dst }));
    assert_eq!(state.conns.len(), 1);
    // whoever waits for A's connection is told it was reset
    assert_eq!(first.opened[0].shared.outcome(), Some(Outcome::Reset));
    assert_eq!(lone.admission.sockets(), 1);
    assert_eq!(lone.admission.half_open(), 1, "B's alone");
}

/// Past `half_open_timeout` a connection that has not completed its
/// handshake is aborted, which makes its socket send a reset in a later
/// egress pass: the shard lets the connection go once that is out, and no
/// sooner (the app would be left without it). It stops counting as half-open
/// when it is aborted, and the shard is told to wake for the time.
#[test]
fn the_half_open_timeout_aborts_the_socket_and_lets_it_go_after_its_reset() {
    let options = TunOptions {
        half_open_timeout: Duration::from_millis(300),
        ..TunOptions::default()
    };
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&options, epoch);
    let (app, dst) = (app4(40000), addr(DST));
    let turn = lone.shard.state().turn(at(0), &mut vec![syn_v4(app, dst)]);
    let opened = &turn.opened[0];
    assert!(turn.out.iter().any(|packet| is_syn(packet)), "the SYN-ACK");
    assert_eq!(
        turn.wake_at,
        Some(at(300)),
        "an idle shard wakes for the time the handshake runs out"
    );
    assert_eq!(lone.admission.half_open(), 1);
    // just before its time nothing happens
    let turn = lone.shard.state().turn(at(299), &mut Vec::new());
    assert!(turn.out.is_empty());
    assert_eq!(turn.wake_at, Some(at(300)));
    assert_eq!(lone.admission.half_open(), 1);
    // its time: the socket is aborted, its reset is not out yet
    let turn = lone.shard.state().turn(at(300), &mut Vec::new());
    assert!(!turn.out.iter().any(|packet| is_reset(packet)));
    assert_eq!(lone.shard.state().socket_count(), 1, "kept for its reset");
    assert_eq!(
        lone.admission.half_open(),
        0,
        "no longer waiting for a handshake"
    );
    assert_eq!(lone.admission.sockets(), 1);
    assert!(
        turn.wake_at.is_some_and(|wake| wake <= at(300)),
        "the shard goes on at once, for the reset"
    );
    assert_eq!(opened.shared.outcome(), None);
    // the next turn sends the reset and lets the connection go
    let turn = lone.shard.state().turn(at(301), &mut Vec::new());
    let resets: Vec<&Vec<u8>> = turn.out.iter().filter(|p| is_reset(p)).collect();
    assert_eq!(resets.len(), 1, "one reset");
    let reset = segment(resets[0]).expect("a segment");
    assert_eq!(reset.tuple, Tuple { src: dst, dst: app });
    assert!(reset.rst && reset.ack);
    assert_eq!(reset.ack_number, 2);
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(opened.shared.outcome(), Some(Outcome::Reset));
    assert_eq!(lone.stats.counters(), Counters::default());
}

// ---- the whole TUN

/// The next packet the TUN sent to the diverted ports.
async fn next(replies: &mut mpsc::UnboundedReceiver<Vec<u8>>) -> Vec<u8> {
    within("a packet from the TUN", replies.recv())
        .await
        .expect("the app's stack goes on handing packets over")
}

/// A SYN of sequence number 1 from the adapter's address of the family of
/// `dst`, and the source it is from.
fn syn_from_adapter(port: u16, dst: SocketAddr) -> (SocketAddr, Vec<u8>) {
    match dst {
        SocketAddr::V4(_) => {
            let app = sock(ADAPTER_V4, port);
            (app, syn_v4(app, dst))
        }
        SocketAddr::V6(_) => {
            let app = sock(ADAPTER_V6, port);
            (app, syn_v6(app, dst))
        }
    }
}

/// Eight connections opened at the same moment to one endpoint, with one
/// shard so that their SYNs meet in it: each is its own connection, with its
/// own source, and what each writes is what it reads back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_syns_to_one_endpoint_each_get_their_own_connection() {
    let rig = rig(TunOptions {
        shards: 1,
        ..TunOptions::default()
    });
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:443");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let mut tasks = JoinSet::new();
    for i in 0..8u8 {
        let app = rig.app.clone();
        tasks.spawn(async move {
            let mut tcp = within("the connection to be made", app.connect(dst))
                .await
                .expect("connect");
            let sent = vec![i; 1000];
            tcp.write_all(&sent).await.expect("write");
            let mut back = vec![0; 1000];
            within("the bytes to come back", tcp.read_exact(&mut back))
                .await
                .expect("read");
            assert_eq!(back, sent, "connection {i} reads back what it wrote");
            tcp
        });
    }
    let mut connections = Vec::new();
    while let Some(done) = tasks.join_next().await {
        connections.push(done.expect("a connection's task"));
    }
    let ports: HashSet<u16> = connections.iter().map(|tcp| tcp.local().port()).collect();
    assert_eq!(ports.len(), 8, "eight sources");
    let dialed = rig.dialer.dialed();
    assert_eq!(dialed.len(), 8, "{dialed:?}");
    let dialed_ports: HashSet<u16> = dialed.iter().map(|session| session.src.port()).collect();
    assert_eq!(dialed_ports, ports, "each source was dialed once");
    assert_eq!(rig.tun.sockets_in_use(), 8);
    assert_eq!(rig.tun.sockets_counted(), 8);
    assert_eq!(rig.tun.half_open(), 0);
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// The first SYN-ACK is lost on its way to the app. smoltcp on both sides
/// goes on: the app sends its SYN again, which the SYN's socket takes as a
/// segment of its own and does not make a second connection of, and the
/// SYN-ACK is sent again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_syn_ack_is_recovered() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let mut lost = false;
    rig.app.set_filter(move |direction, packet| {
        if direction == Direction::FromTun && is_syn(packet) && !lost {
            lost = true;
            return false;
        }
        true
    });
    // the connect gives up after 5 s
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    assert_eq!(rig.dialer.dialed().len(), 1, "one session");
    assert_eq!(rig.tun.sockets_in_use(), 1);
    assert_eq!(rig.tun.sockets_counted(), 1);
    assert_eq!(rig.tun.half_open(), 0);
    assert_eq!(rig.tun.counters(), Counters::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_socket_cap_resets_new_syns() {
    let rig = rig(TunOptions {
        max_sockets: 4,
        ..TunOptions::default()
    });
    let echo = Upstream::echo().await;
    let dst = addr(DST);
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let mut open = Vec::new();
    for _ in 0..4 {
        let mut tcp = within("the connection to be made", rig.app.connect(dst))
            .await
            .expect("connect");
        hello(&mut tcp).await;
        open.push(tcp);
    }
    let error = within("the fifth connect", rig.app.connect(dst))
        .await
        .expect_err("the fifth SYN is answered with a reset");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
    assert_eq!(
        rig.tun.counters(),
        Counters {
            busy: 1,
            ..Counters::default()
        }
    );
    assert_eq!(rig.tun.sockets_in_use(), 4);
    assert_eq!(rig.tun.sockets_counted(), 4);
    assert_eq!(
        rig.tun.half_open(),
        0,
        "the fifth SYN took no half-open place"
    );
    // the first four go on
    for tcp in &mut open {
        hello(tcp).await;
    }
    // a connection that ends gives its place back
    drop(open.pop());
    eventually("a place to be free", WITHIN, || {
        rig.tun.sockets_counted() == 3
    })
    .await;
    let mut again = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut again).await;
    assert_eq!(rig.tun.sockets_counted(), 4);
    assert_eq!(rig.tun.counters().busy, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_half_open_cap_resets_new_syns() {
    let rig = rig(TunOptions {
        max_half_open: 3,
        ..TunOptions::default()
    });
    let echo = Upstream::echo().await;
    let dst = addr(DST);
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let mut replies = rig.app.divert(40000..=40009);
    // three SYNs nobody completes: each is answered with a SYN-ACK
    let mut isns = Vec::new();
    for i in 0..3 {
        rig.app.inject(syn_v4(app4(40000 + i), dst));
        let reply = next(&mut replies).await;
        assert!(is_syn(&reply), "a SYN-ACK");
        isns.push(segment(&reply).expect("a segment").seq);
    }
    assert_eq!(rig.tun.half_open(), 3);
    // the fourth is turned away
    rig.app.inject(syn_v4(app4(40003), dst));
    assert_reset(&next(&mut replies).await, dst, app4(40003));
    assert_eq!(
        rig.tun.counters(),
        Counters {
            busy: 1,
            ..Counters::default()
        }
    );
    assert_eq!(rig.tun.sockets_in_use(), 3);
    assert_eq!(rig.tun.half_open(), 3);
    // one handshake is completed (the ACK of the SYN-ACK): its place is free
    // again, and a fifth SYN is taken
    rig.app.inject(segment_v4(
        app4(40000),
        dst,
        2,
        isns[0].wrapping_add(1),
        0x10,
    ));
    eventually("a half-open place to be free", WITHIN, || {
        rig.tun.half_open() == 2
    })
    .await;
    rig.app.inject(syn_v4(app4(40004), dst));
    let reply = next(&mut replies).await;
    assert!(is_syn(&reply), "a SYN-ACK for the fifth: {reply:?}");
    assert_eq!(rig.tun.half_open(), 3);
    assert_eq!(rig.tun.counters().busy, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_half_open_connection_times_out() {
    let timeout = Duration::from_millis(300);
    let rig = rig(TunOptions {
        half_open_timeout: timeout,
        ..TunOptions::default()
    });
    let (app, dst) = (app4(40000), addr(DST));
    let mut replies = rig.app.divert(40000..=40000);
    let sent = Instant::now();
    rig.app.inject(syn_v4(app, dst));
    assert!(is_syn(&next(&mut replies).await), "the SYN-ACK");
    assert_eq!(rig.tun.sockets_in_use(), 1);
    assert_eq!(rig.tun.half_open(), 1);
    // nobody completes the handshake: the app is reset (a SYN-ACK sent
    // again may come first)
    let reset = within("the reset", async {
        loop {
            let packet = next(&mut replies).await;
            if is_reset(&packet) {
                break packet;
            }
        }
    })
    .await;
    assert!(sent.elapsed() >= timeout, "not before its time");
    let reset = segment(&reset).expect("a segment");
    assert_eq!(reset.tuple, Tuple { src: dst, dst: app });
    assert!(reset.rst && reset.ack);
    assert_eq!(reset.ack_number, 2, "it acknowledges the SYN");
    eventually("the TUN to let the connection go", WITHIN, || {
        rig.tun.sockets_in_use() == 0
    })
    .await;
    assert_eq!(rig.tun.sockets_counted(), 0);
    assert_eq!(rig.tun.half_open(), 0);
    assert_eq!(rig.tun.budget_in_use(), 0);
    assert_eq!(rig.dialer.dialed().len(), 0, "never dialed");
    assert_eq!(rig.tun.counters(), Counters::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn syns_to_rurge_addresses_are_reset() {
    let rig = rig(TunOptions::default());
    let mut replies = rig.app.divert(40000..=40009);
    let own = [
        "198.18.0.1:80",
        "198.18.0.2:54",
        "198.18.0.244:443",
        "198.18.0.253:80",
        "[fd00:6152::fd]:80",
    ];
    let fake = ["198.18.1.1:80", "[fd00:6152::c612:101]:80"];
    for (i, dst) in own.iter().chain(&fake).enumerate() {
        let dst = addr(dst);
        let (app, syn) = syn_from_adapter(40000 + i as u16, dst);
        rig.app.inject(syn);
        assert_answers(&next(&mut replies).await, app, dst, 2);
        // rurge's own addresses are not counted, a name nobody knows is
        let counted = i.saturating_sub(own.len() - 1) as u64;
        assert_eq!(
            rig.tun.counters(),
            Counters {
                unknown_fake: counted,
                ..Counters::default()
            },
            "after the SYN to {dst}"
        );
    }
    assert_eq!(rig.dialer.dialed().len(), 0, "nothing is dialed");
    assert_eq!(rig.tun.sockets_in_use(), 0);
    assert_eq!(rig.tun.sockets_counted(), 0);
    assert_eq!(rig.tun.half_open(), 0);
    assert_eq!(rig.tun.counters().unknown_fake, 2);
}

/// rurge's own resets are let through 100 times in 3 s by the whole TUN, not
/// by each shard: the 101 SYNs go to different shards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resets_are_limited_to_100_in_3_seconds() {
    let rig = rig(TunOptions::default());
    let mut replies = rig.app.divert(40000..=40100);
    let dst = addr("198.18.0.1:80");
    for port in 40000..=40100 {
        rig.app.inject(syn_v4(sock(ADAPTER_V4, port), dst));
    }
    let mut answered = HashSet::new();
    for _ in 0..100 {
        let reply = next(&mut replies).await;
        let reset = segment(&reply).expect("a segment");
        assert!(reset.rst, "{reset:?}");
        answered.insert(reset.tuple.dst.port());
    }
    assert_eq!(answered.len(), 100, "each to a SYN of its own");
    eventually("the 101st reset to be held back", WITHIN, || {
        rig.tun.counters().rst_limited == 1
    })
    .await;
    // all 101 SYNs have been decided: a hundred answered, one counted
    assert!(replies.try_recv().is_err(), "no 101st reset");
    assert_eq!(
        rig.tun.counters(),
        Counters {
            rst_limited: 1,
            ..Counters::default()
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reset_in_syn_received_removes_the_socket() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let dst = addr(DST);
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let first = app4(40000);
    let mut replies = rig.app.divert(40000..=40000);
    rig.app.inject(syn_v4(first, dst));
    assert!(is_syn(&next(&mut replies).await), "the SYN-ACK");
    assert_eq!(rig.tun.sockets_in_use(), 1);
    assert_eq!(rig.tun.half_open(), 1);
    // the app resets the connection before it is made: the sequence number
    // is the SYN's plus one
    rig.app.inject(segment_v4(first, dst, 2, 0, 0x04));
    eventually("the socket to go", WITHIN, || rig.tun.sockets_in_use() == 0).await;
    assert_eq!(rig.tun.sockets_counted(), 0);
    assert_eq!(rig.tun.half_open(), 0);
    // a SYN of another source to the same destination is a connection of
    // its own
    let mut tcp = within(
        "the connection to be made",
        rig.app.connect_from(50000, dst),
    )
    .await
    .expect("connect");
    hello(&mut tcp).await;
    let dialed = rig.dialer.dialed();
    assert_eq!(dialed.len(), 1, "one session: {dialed:?}");
    assert_eq!(dialed[0].src, tcp.local());
    assert_eq!(dialed[0].src.port(), 50000);
    assert_eq!(rig.tun.sockets_in_use(), 1);
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// smoltcp does not take a SYN whose MSS option is 0. One shard, so that
/// the packets are taken in the order they were sent: when the connection
/// after it has been made, the SYN has been dealt with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_syn_with_mss_zero_gets_no_connection() {
    let rig = rig(TunOptions {
        shards: 1,
        ..TunOptions::default()
    });
    let echo = Upstream::echo().await;
    let dst = addr(DST);
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let app = app4(40000);
    let mut replies = rig.app.divert(40000..=40000);
    let repr = TcpRepr {
        control: TcpControl::Syn,
        seq_number: TcpSeqNumber(1),
        max_seg_size: Some(0),
        ..tcp_repr(app, dst, &[])
    };
    rig.app.inject(tcp_packet(app, dst, &repr));
    // a SYN to the same destination after it is a connection of its own
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    assert!(replies.try_recv().is_err(), "the SYN got no answer");
    let dialed = rig.dialer.dialed();
    assert_eq!(dialed.len(), 1, "one session: {dialed:?}");
    assert_eq!(dialed[0].src, tcp.local());
    assert_eq!(rig.tun.sockets_in_use(), 1);
    assert_eq!(rig.tun.sockets_counted(), 1);
    assert_eq!(rig.tun.counters(), Counters::default());
}
