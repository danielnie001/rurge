//! The echo request goes out and the reply comes back; what is not answered;
//! and a reply larger than the device takes.

use super::{APP4, APP6, FAR4, FAR6, NOTHING, OWN4, echoed, ip, lab, lab_with};
use crate::testing::PingScript;
use crate::testing::packets::{self, IcmpSeen};
use crate::tests::support::{Extras, WITHIN, eventually, with_ip_options};
use crate::{Counters, TunOptions};
use rurge_dns::fakeip::{FakeIpPool, v4_of, v6_of};
use std::net::IpAddr;
use std::sync::Arc;

/// An echo request to a real address goes to the pinger as it is, and what the
/// pinger answers comes back from that address with the identifier and the
/// sequence number of the app's.
async fn a_real_address_is_pinged(app: IpAddr, far: IpAddr) {
    let mut lab = lab(TunOptions::default());
    lab.extras.pinger.script(far, PingScript::Echo);
    let requests: [(u16, u16, &[u8]); 2] = [(0x1234, 7, b"hello, ping"), (0xbeef, 65535, b"")];
    for (ident, seq, payload) in requests {
        lab.ping(app, far, ident, seq, payload);
        assert_eq!(lab.reply().await, echoed(far, app, ident, seq, payload));
    }
    assert_eq!(
        lab.pinged(),
        [(far, b"hello, ping".to_vec()), (far, Vec::new())]
    );
    assert_eq!(lab.counters(), Counters::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ping_to_a_real_address_goes_through_the_pinger_v4() {
    a_real_address_is_pinged(APP4, FAR4).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ping_to_a_real_address_goes_through_the_pinger_v6() {
    a_real_address_is_pinged(APP6, FAR6).await;
}

/// The reply is the pinger's: what it answers comes back, not what the app
/// sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_reply_carries_the_payload_the_pinger_gave() {
    let mut lab = lab(TunOptions::default());
    lab.extras
        .pinger
        .script(FAR4, PingScript::Reply(b"what the pinger heard"));
    lab.extras.pinger.script(FAR6, PingScript::Reply(b""));
    lab.ping(APP4, FAR4, 5, 6, b"what the app said");
    assert_eq!(
        lab.reply().await,
        echoed(FAR4, APP4, 5, 6, b"what the pinger heard")
    );
    lab.ping(APP6, FAR6, 7, 8, b"what the app said");
    assert_eq!(lab.reply().await, echoed(FAR6, APP6, 7, 8, b""));
    assert_eq!(lab.counters(), Counters::default());
}

/// A request to a fake address goes to an address of its name, the same
/// family as the request when the name has one, else the other; the reply
/// comes from the fake address.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ping_to_a_fake_address_goes_to_its_names_address() {
    let mut lab = lab(TunOptions::default());
    let (a, b, c) = (
        lab.assign("a.test"),
        lab.assign("b.test"),
        lab.assign("c.test"),
    );
    // a has both kinds, an IPv6 address first; b only IPv4, c only IPv6
    lab.resolver.insert(
        "a.test",
        vec![ip("2001:db8::55"), ip("192.0.2.55"), ip("192.0.2.56")],
    );
    lab.resolver.insert("b.test", vec![ip("192.0.2.77")]);
    lab.resolver.insert("c.test", vec![ip("2001:db8::77")]);
    let cases = [
        // the first address of the request's own family
        (APP4, IpAddr::V4(v4_of(a)), "192.0.2.55"),
        (APP6, IpAddr::V6(v6_of(a)), "2001:db8::55"),
        // only addresses of the other family
        (APP6, IpAddr::V6(v6_of(b)), "192.0.2.77"),
        (APP4, IpAddr::V4(v4_of(c)), "2001:db8::77"),
    ];
    for (number, (app, fake, address)) in cases.into_iter().enumerate() {
        let address = ip(address);
        lab.extras.pinger.script(address, PingScript::Echo);
        let ident = 100 + number as u16;
        lab.ping(app, fake, ident, 1, b"hello, name");
        assert_eq!(
            lab.reply().await,
            echoed(fake, app, ident, 1, b"hello, name"),
            "to {fake}"
        );
        assert_eq!(
            lab.pinged().last(),
            Some(&(address, b"hello, name".to_vec())),
            "to {fake}"
        );
    }
    assert_eq!(lab.pinged().len(), 4);
    assert_eq!(lab.counters(), Counters::default());
}

/// The addresses of rurge's own answer here: the pinger is not asked, and the
/// reply is from the address that was pinged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ping_to_rurge_itself_is_answered_here() {
    let mut lab = lab(TunOptions::default());
    let cases = [
        (APP4, "198.18.0.2"),
        (APP4, "198.18.0.244"),
        (APP4, "198.18.0.253"),
        (APP6, "fd00:6152::2"),
        (APP6, "fd00:6152::fd"),
    ];
    for (number, (app, own)) in cases.into_iter().enumerate() {
        let own = ip(own);
        let (ident, seq) = (0x0100 + number as u16, 0x0200 + number as u16);
        lab.ping(app, own, ident, seq, b"are you there");
        assert_eq!(
            lab.reply().await,
            echoed(own, app, ident, seq, b"are you there"),
            "to {own}"
        );
    }
    assert!(lab.pinged().is_empty());
    lab.tasks_end().await;
    assert_eq!(lab.counters(), Counters::default());
}

/// A request that carries IP options has its ICMP message after them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_echo_request_with_ip_options_is_read_after_them() {
    let mut lab = lab(TunOptions::default());
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    for to in [FAR4, OWN4] {
        let request = packets::echo_request(APP4, to, 0x0a0b, 0x0c0d, b"with options");
        lab.rig.app.inject(with_ip_options(&request, &[1, 1, 1, 1]));
        assert_eq!(
            lab.reply().await,
            echoed(to, APP4, 0x0a0b, 0x0c0d, b"with options"),
            "to {to}"
        );
    }
    assert_eq!(lab.pinged(), [(FAR4, b"with options".to_vec())]);
}

// ---- what is not answered

/// `icmp-forwarding` is read from the settings of the generation in use for
/// each request. With it off every echo request is dropped and counted as
/// unsupported, the ones to an address of rurge's own among them (the design's
/// order of the checks); the next request sees the setting as it is then.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn icmp_forwarding_off_drops_echo_requests() {
    let mut lab = lab(TunOptions::default());
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    lab.extras.pinger.script(FAR6, PingScript::Echo);
    lab.forwarding(false);
    lab.ping(APP4, FAR4, 1, 1, b"off");
    lab.ping(APP6, FAR6, 1, 2, b"off");
    lab.ping(APP4, OWN4, 1, 3, b"off");
    eventually("the requests to be counted", WITHIN, || {
        lab.counters().unsupported == 3
    })
    .await;
    assert!(lab.pinged().is_empty());
    // the next request is read with the settings of its own time, and nothing
    // was answered before it
    lab.forwarding(true);
    lab.ping(APP4, FAR4, 2, 4, b"on");
    assert_eq!(lab.reply().await, echoed(FAR4, APP4, 2, 4, b"on"));
    assert_eq!(lab.pinged(), [(FAR4, b"on".to_vec())]);
    assert_eq!(
        lab.counters(),
        Counters {
            unsupported: 3,
            ..Counters::default()
        }
    );
}

/// An address of the pool's space that no name has names nothing: the request
/// is dropped and counted, whether the TUN has a pool or not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_fake_address_is_not_pinged() {
    for pool in [Some(Arc::new(FakeIpPool::new())), None] {
        let had_pool = pool.is_some();
        let mut lab = lab_with(
            TunOptions::default(),
            Extras {
                pool,
                ..Extras::default()
            },
        );
        lab.ping(APP4, IpAddr::V4(v4_of(7)), 1, 1, b"who?");
        eventually("the request to be counted", WITHIN, || {
            lab.counters().unknown_fake == 1
        })
        .await;
        lab.ping(APP6, IpAddr::V6(v6_of(7)), 1, 2, b"who?");
        eventually("the request to be counted", WITHIN, || {
            lab.counters().unknown_fake == 2
        })
        .await;
        lab.dealt_with().await;
        assert!(lab.pinged().is_empty(), "(a pool: {had_pool})");
        assert_eq!(
            lab.counters(),
            Counters {
                unknown_fake: 2,
                ..Counters::default()
            },
            "(a pool: {had_pool})"
        );
    }
}

/// A name the resolver cannot resolve, or resolves to no address, is dropped
/// without a count (only a line at debug level, see below).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_name_that_does_not_resolve_is_not_pinged() {
    let mut lab = lab(TunOptions::default());
    // the resolver knows no such name as lost.test
    let lost = lab.assign("lost.test");
    let empty = lab.assign("empty.test");
    lab.resolver.insert("empty.test", Vec::new());
    lab.ping(APP4, IpAddr::V4(v4_of(lost)), 1, 1, b"anyone?");
    lab.ping(APP6, IpAddr::V6(v6_of(empty)), 1, 2, b"anyone?");
    // what must not happen: a reply
    tokio::time::sleep(NOTHING).await;
    assert!(lab.no_reply());
    assert!(lab.pinged().is_empty());
    lab.tasks_end().await;
    assert_eq!(lab.counters(), Counters::default());
}

/// A pinger that fails answers nothing, and nothing is counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_pinger_answers_nothing() {
    let mut lab = lab(TunOptions::default());
    // FAR6 is not scripted, and the fake pinger fails what it was told nothing about
    lab.extras.pinger.script(FAR4, PingScript::Fail);
    lab.ping(APP4, FAR4, 1, 1, b"anyone?");
    lab.ping(APP6, FAR6, 1, 2, b"anyone?");
    lab.pinged_times(2).await;
    lab.dealt_with().await;
    assert_eq!(lab.counters(), Counters::default());
}

/// A reply that does not fit an IP packet is not written: the longest does,
/// one byte more does not. The pinger is a trait whose implementations are
/// not this crate's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_longest_reply_is_carried_and_a_longer_one_dropped() {
    static BYTES: [u8; 65_528] = [0x5a; 65_528];
    let mut lab = lab(TunOptions::default());
    // an IPv4 packet is at most 65535 bytes with a header of 20 and the ICMP
    // header of 8; IPv6's payload is at most 65535 with the ICMP header in it
    for (app, far, longest) in [(APP4, FAR4, 65_507), (APP6, FAR6, 65_527)] {
        lab.extras
            .pinger
            .script(far, PingScript::Reply(&BYTES[..longest]));
        lab.ping(app, far, 1, 1, b"x");
        let seen = lab.reply().await;
        let IcmpSeen::EchoReply { payload, .. } = seen else {
            panic!("an echo reply, not {seen:?}");
        };
        assert!(
            payload == BYTES[..longest],
            "to {far}: a payload of {}",
            payload.len()
        );
        lab.extras
            .pinger
            .script(far, PingScript::Reply(&BYTES[..longest + 1]));
        lab.ping(app, far, 1, 2, b"x");
        lab.dealt_with().await;
    }
    assert_eq!(lab.counters(), Counters::default());
}

// ---- a reply larger than the device takes

/// A reply, and a request, larger than the device's MTU: the reply is written
/// as the fragments of one packet, which the app puts together.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_larger_than_the_mtu_is_fragmented() {
    let mut lab = lab(TunOptions::default());
    let payload: Vec<u8> = (0..3000).map(|at| (at % 251) as u8).collect();
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    lab.extras.pinger.script(FAR6, PingScript::Echo);
    // an ICMP message of 3008 bytes is 3 fragments on a device of 1500 bytes
    for (app, to) in [
        (APP4, FAR4),
        (APP6, FAR6),
        (APP4, OWN4),
        (APP6, ip("fd00:6152::fd")),
    ] {
        lab.ping(app, to, 9, 9, &payload);
        let pieces = lab.reply_packets().await;
        assert_eq!(pieces.len(), 3, "to {to}");
        for piece in &pieces {
            assert!(piece.len() <= 1500, "to {to}: a packet of {}", piece.len());
        }
        let whole = packets::reassemble(&pieces).expect("the fragments of one reply");
        assert_eq!(
            packets::read_icmp(&whole),
            Some(echoed(to, app, 9, 9, &payload)),
            "to {to}"
        );
    }
    assert_eq!(lab.counters(), Counters::default());
}
