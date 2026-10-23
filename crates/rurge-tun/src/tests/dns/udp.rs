//! Where a UDP datagram that an app sends goes (phase 3 M3 design 11.4 and
//! 11.5): to the responder when it is sent to port 53 of one of the responder's
//! addresses, or to a server that `hijack-dns` names and it is a DNS query; to
//! an ICMP message when it is sent to another port of an address of rurge's
//! own; and to a UDP session in every other case. The app sends with
//! `AppStack::inject` and reads what the TUN writes through `divert_udp` and
//! `divert_icmp`, so that nothing it reads has been through a stack of its own.
//! The doubles behind the responder are the ones of the other tests here
//! (`FakeDns`, a pool of fake addresses, `FakeHooks`), and a datagram that goes
//! to a session goes to the fake `Dialer`, whose UDP is in memory and opens no
//! socket: the servers of the `hijack-dns` tests are addresses of the
//! documentation ranges.

use super::{A, AAAA, TXT, parsed, query};
use crate::dns::udp::{Query, offer};
use crate::dns::{Ask, DNS_PORT};
use crate::stats::Stats;
use crate::testing::packets::{self, UdpSeen};
use crate::testing::{AppStack, FakeDialer, FakeResolve, ForwardScript};
use crate::tests::support::{
    Extras, GatedDevice, Rig, WITHIN, addr, eventually, in_five_seconds, settings, sock,
    tun_dialing,
};
use crate::tests::udp::{APP4, APP6, Lab, NOTHING, lab, lab_and_extras, taking};
use crate::{Counters, Ending, TunOptions, TunSettings};
use rurge_config::{HijackTarget, Transport};
use rurge_dns::fakeip::{AssignError, FakeIpPool, v4_of};
use rurge_dns::message::Rcode;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, mpsc::error::TryRecvError};

/// The responder's addresses, and a server that `hijack-dns` may name (a
/// documentation address), one family each where there is one. The rig, the
/// app's endpoints and the wait for what must not happen are the ones of the
/// other UDP tests (`tests::udp`).
const RESPONDER4: &str = "198.18.0.2:53";
const RESPONDER6: &str = "[fd00:6152::2]:53";
const SERVER: &str = "192.0.2.53:53";

/// What these tests wait for besides what every test of the UDP rig does.
impl Lab {
    /// The datagrams that went to the responder were answered and that is all
    /// that came of them: the tasks of the queries are over, no session was
    /// begun for any, nothing was dropped, nothing else is on its way to the
    /// app. (The wait is the one for what must not happen.)
    async fn only_answers_came(&mut self) {
        eventually("the tasks of the queries to end", WITHIN, || {
            self.rig.tun.session_tasks() == 0
        })
        .await;
        tokio::time::sleep(NOTHING).await;
        assert!(self.quiet(), "a datagram or an ICMP message for the app");
        assert!(
            self.rig.dialer.associations().is_empty(),
            "a session was begun"
        );
        assert_eq!(self.counters(), Counters::default());
    }

    /// Waits for the fake dialer to have been handed `count` datagrams.
    async fn sent_times(&self, count: usize) {
        eventually("the datagrams to reach the dialer", WITHIN, || {
            self.sent().len() == count
        })
        .await;
    }
}

/// The datagram is the answer to the query with this ID that gives one address,
/// the fake address `wanted`, with the TTL of a fake address (30 seconds).
fn assert_fake_answer(seen: &UdpSeen, id: u16, wanted: IpAddr) {
    let answer = parsed(&seen.payload);
    assert_eq!(answer.id, id, "the ID of the query");
    assert_eq!(answer.rcode, Rcode::NoError);
    match wanted {
        IpAddr::V4(wanted) => assert_eq!(answer.v4, [(wanted, 30)]),
        IpAddr::V6(wanted) => assert_eq!(answer.v6, [(wanted, 30)]),
    }
}

/// Settings of which `hijack-dns` is `servers` (an entry is the address, or
/// `None` for `*`, and the port) and the rest the defaults of `Extras`.
fn hijacking(extras: &Extras, servers: &[(Option<Ipv4Addr>, u16)]) {
    extras.hooks.set(TunSettings {
        dns: extras.dns.clone(),
        hijack_dns: servers
            .iter()
            .map(|&(addr, port)| HijackTarget { addr, port })
            .collect(),
        ..settings(FakeResolve::new(), true)
    });
}

pub(super) fn extras_hijacking(servers: &[(Option<Ipv4Addr>, u16)]) -> Extras {
    let extras = Extras::default();
    hijacking(&extras, servers);
    extras
}

fn ip4(text: &str) -> Ipv4Addr {
    text.parse().expect("an IPv4 address")
}

// ---- the responder's own addresses

/// A query to port 53 of the responder's IPv4 address is answered from there,
/// to the endpoint it came from, with the ID it had; no session is begun.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_query_to_the_responder_is_answered_v4() {
    let mut lab = lab(TunOptions::default());
    let (app, responder) = (addr(APP4), addr(RESPONDER4));
    lab.send(app, responder, &query(0x1234, "a.test", A));
    let seen = lab.datagram().await;
    assert_eq!((seen.src, seen.dst), (responder, app));
    assert_fake_answer(&seen, 0x1234, lab.fake("a.test", false));
    lab.only_answers_came().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_query_to_the_responder_is_answered_v6() {
    let mut lab = lab(TunOptions::default());
    let (app, responder) = (addr(APP6), addr(RESPONDER6));
    lab.send(app, responder, &query(0xbeef, "a.test", AAAA));
    let seen = lab.datagram().await;
    assert_eq!((seen.src, seen.dst), (responder, app));
    assert_fake_answer(&seen, 0xbeef, lab.fake("a.test", true));
    lab.only_answers_came().await;
}

/// All eight addresses of the responder, `198.18.0.2` to `198.18.0.9`, answer,
/// each from itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_responder_address_answers() {
    let mut lab = lab(TunOptions::default());
    let app = addr(APP4);
    for last in 2..=9u8 {
        let responder = sock([198, 18, 0, last], DNS_PORT);
        lab.send(app, responder, &query(u16::from(last), "a.test", A));
        let seen = lab.datagram().await;
        assert_eq!((seen.src, seen.dst), (responder, app), "address {last}");
        assert_fake_answer(&seen, u16::from(last), lab.fake("a.test", false));
    }
    lab.only_answers_came().await;
}

/// Another port of the responder's addresses is no place a datagram goes to:
/// the app is told so with ICMP, as for any address of rurge's own, and the
/// responder is not asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn other_ports_of_the_responder_get_icmp() {
    let mut lab = lab(TunOptions::default());
    let (app4, app6) = (addr(APP4), addr(APP6));
    for (app, dst, headers) in [
        (app4, addr("198.18.0.2:54"), 28),
        (app6, addr("[fd00:6152::2]:54"), 48),
        (app4, addr("198.18.0.9:5353"), 28),
    ] {
        let packet = packets::udp(app, dst, &query(1, "a.test", A));
        lab.rig.app.inject(packet.clone());
        let (src, to, quoted) = lab.prohibited().await;
        assert_eq!((src, to), (dst.ip(), app.ip()), "to {dst}");
        assert_eq!(quoted, packet[..headers], "to {dst}");
    }
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "an answer for the app");
    assert!(lab.rig.dialer.associations().is_empty());
    assert_eq!(lab.counters(), Counters::default());
}

// ---- hijack-dns

/// A query to a server that `hijack-dns` names is answered by the responder as
/// from that server, and the fake dialer is never given an association.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hijacked_query_is_answered_from_the_hijacked_server() {
    let (mut lab, _extras) = lab_and_extras(
        TunOptions::default(),
        extras_hijacking(&[(Some(ip4("192.0.2.53")), 53)]),
    );
    let (app, server) = (addr(APP4), addr(SERVER));
    lab.send(app, server, &query(7, "a.test", A));
    let seen = lab.datagram().await;
    assert_eq!((seen.src, seen.dst), (server, app));
    assert_fake_answer(&seen, 7, lab.fake("a.test", false));
    lab.only_answers_came().await;
}

/// Only the servers `hijack-dns` names: a query to another server, or to the
/// named one at another port, is the app's own traffic and is a session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_that_hijack_dns_does_not_name_is_not_hijacked() {
    let (mut lab, _extras) = lab_and_extras(
        TunOptions::default(),
        extras_hijacking(&[(Some(ip4("192.0.2.53")), 53)]),
    );
    let app = addr(APP4);
    let (other_server, other_port) = (addr("192.0.2.54:53"), addr("192.0.2.53:5353"));
    let (one, two) = (query(1, "a.test", A), query(2, "a.test", A));
    lab.send(app, other_server, &one);
    lab.send(app, other_port, &two);
    lab.sent_times(2).await;
    let sent = lab.sent();
    assert_eq!(sent[0].0.port, 53);
    assert_eq!(sent[0].1, one);
    assert_eq!(sent[1].0.port, 5353);
    assert_eq!(sent[1].1, two);
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "an answer for the app");
    assert_eq!(lab.rig.dialer.associations().len(), 1);
    assert_eq!(lab.counters(), Counters::default());
}

/// `*:53` is any server at port 53.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hijack_star_takes_every_server() {
    let (mut lab, _extras) = lab_and_extras(TunOptions::default(), extras_hijacking(&[(None, 53)]));
    let app = addr(APP4);
    for (id, server) in [
        addr("192.0.2.53:53"),
        addr("198.51.100.53:53"),
        addr("203.0.113.53:53"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = id as u16 + 1;
        lab.send(app, server, &query(id, "a.test", A));
        let seen = lab.datagram().await;
        assert_eq!((seen.src, seen.dst), (server, app));
        assert_fake_answer(&seen, id, lab.fake("a.test", false));
    }
    lab.only_answers_came().await;
}

/// An entry with a port other than 53 takes the queries to that port, and not
/// those to 53.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hijack_with_a_port() {
    let (mut lab, _extras) =
        lab_and_extras(TunOptions::default(), extras_hijacking(&[(None, 5353)]));
    let (app, hijacked, untouched) = (addr(APP4), addr("192.0.2.53:5353"), addr(SERVER));
    lab.send(app, hijacked, &query(1, "a.test", A));
    let seen = lab.datagram().await;
    assert_eq!((seen.src, seen.dst), (hijacked, app));
    assert_fake_answer(&seen, 1, lab.fake("a.test", false));
    // the one to port 53 is a session
    let at_53 = query(2, "a.test", A);
    lab.send(app, untouched, &at_53);
    lab.sent_times(1).await;
    assert_eq!(lab.sent()[0].1, at_53);
    assert_eq!(lab.rig.dialer.associations().len(), 1);
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "an answer for the app");
}

/// A datagram to a hijacked server that is no query is the app's own traffic
/// (QUIC, a game, a response from a server the app listens to, a message of
/// another opcode): it is a session's, and the fake dialer gets it as it came.
/// A query that comes after them is the responder's still.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_packet_to_a_hijacked_server_that_is_not_a_query_is_a_session() {
    let (mut lab, _extras) = lab_and_extras(TunOptions::default(), extras_hijacking(&[(None, 53)]));
    let (app, server) = (addr(APP4), addr(SERVER));
    // a response: QR is set
    let mut response = query(1, "a.test", A);
    response[2] |= 0x80;
    // opcode 4 (NOTIFY), RD set
    let mut notify = query(2, "a.test", A);
    notify[2] = 0x21;
    // no question, where a query has one
    let mut empty = query(3, "a.test", A);
    empty[4..6].copy_from_slice(&0u16.to_be_bytes());
    // less than a header
    let runt = vec![0x16, 0xfe, 0xfd];
    let not_queries = [response, notify, empty, runt];
    for payload in &not_queries {
        lab.send(app, server, payload);
    }
    lab.sent_times(not_queries.len()).await;
    for (sent, payload) in lab.sent().iter().zip(&not_queries) {
        assert_eq!((sent.0.port, &sent.1), (53, payload));
    }
    assert_eq!(lab.rig.dialer.associations().len(), 1);
    // and a query is still the responder's
    lab.send(app, server, &query(9, "a.test", A));
    let seen = lab.datagram().await;
    assert_eq!((seen.src, seen.dst), (server, app));
    assert_fake_answer(&seen, 9, lab.fake("a.test", false));
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "something else for the app");
    assert_eq!(
        lab.sent().len(),
        not_queries.len(),
        "the query went to a session"
    );
    assert_eq!(lab.counters(), Counters::default());
}

/// `hijack-dns` is for IPv4: a query to port 53 of an IPv6 address is a session
/// whatever the entries say.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ipv6_is_never_hijacked() {
    let (mut lab, _extras) = lab_and_extras(TunOptions::default(), extras_hijacking(&[(None, 53)]));
    let (app, server) = (addr(APP6), addr("[2001:db8::53]:53"));
    let asked = query(1, "a.test", AAAA);
    lab.send(app, server, &asked);
    lab.sent_times(1).await;
    assert_eq!(lab.sent()[0].1, asked);
    assert_eq!(lab.rig.dialer.associations().len(), 1);
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "an answer for the app");
}

/// The order the destinations are decided in: the responder's own port 53 is
/// the responder's whatever `hijack-dns` says; the other addresses of rurge's
/// own are refused with ICMP before `hijack-dns` is looked at; and an address
/// of the pool's space is a place a query is hijacked at, as any other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_destinations_are_decided_in_order() {
    let (mut lab, _extras) = lab_and_extras(TunOptions::default(), extras_hijacking(&[(None, 53)]));
    let app = addr(APP4);
    // the responder's address at 53: its own, from itself
    let responder = addr("198.18.0.3:53");
    lab.send(app, responder, &query(1, "a.test", A));
    let seen = lab.datagram().await;
    assert_eq!((seen.src, seen.dst), (responder, app));
    // an address of rurge's own that is not the responder's: ICMP, no answer
    let own = addr("198.18.0.244:53");
    let packet = packets::udp(app, own, &query(2, "a.test", A));
    lab.rig.app.inject(packet.clone());
    let (src, to, quoted) = lab.prohibited().await;
    assert_eq!((src, to), (own.ip(), app.ip()));
    assert_eq!(quoted, packet[..28]);
    // an address that stands for a name: hijacked, from where it was sent to
    let fake = addr("198.18.1.5:53");
    lab.send(app, fake, &query(3, "a.test", A));
    let seen = lab.datagram().await;
    assert_eq!((seen.src, seen.dst), (fake, app));
    assert_fake_answer(&seen, 3, lab.fake("a.test", false));
    lab.only_answers_came().await;
}

/// What `hijack-dns` is is asked of the generation in use for each datagram:
/// a reload that adds an entry, or takes it away, is seen by the next one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reload_of_hijack_dns_is_seen_by_the_next_datagram() {
    let (mut lab, extras) = lab_and_extras(TunOptions::default(), Extras::default());
    let (app, server) = (addr(APP4), addr(SERVER));
    // no entry: a session
    lab.send(app, server, &query(1, "a.test", A));
    lab.sent_times(1).await;
    // an entry: the responder's
    hijacking(&extras, &[(Some(ip4("192.0.2.53")), 53)]);
    lab.send(app, server, &query(2, "a.test", A));
    let seen = lab.datagram().await;
    assert_eq!((seen.src, seen.dst), (server, app));
    assert_fake_answer(&seen, 2, lab.fake("a.test", false));
    assert_eq!(lab.sent().len(), 1);
    // none again: a session
    hijacking(&extras, &[]);
    lab.send(app, server, &query(3, "a.test", A));
    lab.sent_times(2).await;
    tokio::time::sleep(NOTHING).await;
    assert!(lab.quiet(), "an answer for the app");
    assert_eq!(lab.counters(), Counters::default());
}

// ---- the queue and the tasks of the queries

/// A thousand queries from one endpoint, a thousand different IDs: a thousand
/// answers, each with the ID and the address of its own query.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_of_queries_is_answered() {
    // an answer is handed to the writer without waiting for room, and a
    // thousand of them can be made before the writer has taken the first few:
    // the writer's queue has room for all of them (the 256 batches it holds by
    // default would lose some). The responder's queue holds 1,024 queries, so
    // it holds the thousand too.
    let mut lab = lab(TunOptions {
        writer_queue: 1_100,
        ..TunOptions::default()
    });
    let (app, responder) = (addr(APP4), addr(RESPONDER4));
    for id in 0..1_000u16 {
        lab.send(app, responder, &query(id, &format!("n{id}.test"), A));
    }
    let mut seen: HashMap<u16, IpAddr> = HashMap::new();
    for _ in 0..1_000 {
        let datagram = lab.datagram().await;
        assert_eq!((datagram.src, datagram.dst), (responder, app));
        let answer = parsed(&datagram.payload);
        assert_eq!(answer.rcode, Rcode::NoError);
        assert_eq!(answer.v4.len(), 1);
        assert!(
            seen.insert(answer.id, IpAddr::V4(answer.v4[0].0)).is_none(),
            "two answers with the ID {}",
            answer.id
        );
    }
    for id in 0..1_000u16 {
        let wanted = lab.fake(&format!("n{id}.test"), false);
        assert_eq!(seen.get(&id), Some(&wanted), "the answer to query {id}");
    }
    lab.only_answers_came().await;
}

/// The hand-off to the responder's queue never waits: a query that finds the
/// queue full is dropped and counted `queue_full`, one that finds it closed
/// (the TUN has stopped) is dropped and not counted.
#[tokio::test]
async fn a_full_responder_queue_counts_queue_full() {
    let (queue, mut taken) = mpsc::channel::<Query>(1);
    let stats = Stats::default();
    let ask = Ask {
        src: addr(APP4),
        server: addr(RESPONDER4),
        transport: Transport::Udp,
    };
    let asked = query(1, "a.test", A);
    offer(&queue, &stats, &asked, ask);
    assert_eq!(stats.counters(), Counters::default(), "the first fits");
    offer(&queue, &stats, &asked, ask);
    assert_eq!(
        stats.counters(),
        Counters {
            queue_full: 1,
            ..Counters::default()
        },
        "the second finds the queue full"
    );
    // what is in the queue is the first query, as it came
    let first = taken.try_recv().expect("the first query is queued");
    assert_eq!(
        (first.payload, first.ask.src, first.ask.server),
        (asked.clone(), ask.src, ask.server)
    );
    assert!(matches!(taken.try_recv(), Err(TryRecvError::Empty)));
    // a queue that is closed drops without counting
    drop(taken);
    offer(&queue, &stats, &asked, ask);
    assert_eq!(
        stats.counters().queue_full,
        1,
        "a closed queue counts nothing"
    );
}

/// The task of a query that waits on its upstream ends at the stop: the TUN
/// ends, though the upstream never answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_query_held_by_the_upstream_ends_at_the_stop() {
    let (mut lab, extras) = lab_and_extras(TunOptions::default(), Extras::default());
    extras.dns.script("held.test", ForwardScript::Hold);
    let (app, responder) = (addr(APP4), addr(RESPONDER4));
    lab.send(app, responder, &query(1, "held.test", TXT));
    eventually("the query to be forwarded", WITHIN, || {
        extras.dns.forwarded().len() == 1
    })
    .await;
    assert_eq!(lab.rig.tun.session_tasks(), 1, "the task of the query");
    lab.rig.tun.stop();
    let ending = in_five_seconds("the TUN to end", lab.rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    assert!(lab.quiet(), "an answer for the app");
}

/// An answer is handed to the writer without waiting for room, as a reply to
/// an echo request to rurge's own address is: when the writer's queue is full
/// it is dropped and counted `queue_full`, and the tasks of the queries do not
/// wait (answers that took no forward's place would pile up behind a device
/// that takes nothing).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_that_finds_the_writer_queue_full_is_dropped_and_counted() {
    let (gate, end) = GatedDevice::pair();
    let extras = Extras::default();
    let dialer = FakeDialer::new();
    let tun = tun_dialing(
        gate.clone(),
        dialer.clone(),
        &extras,
        TunOptions {
            writer_queue: 1,
            ..TunOptions::default()
        },
    );
    let mut lab = taking(Rig {
        tun,
        app: AppStack::start(end),
        dialer,
    });
    let (app, responder) = (addr(APP4), addr(RESPONDER4));
    let senders: u64 = 20;
    for id in 0..senders as u16 {
        lab.send(app, responder, &query(id, "a.test", A));
    }
    // every query has reached the responder (a rule is asked about each query
    // for an address), and every task has done what it was to: written the
    // answer to the queue, or dropped it; none is waiting for room
    eventually("every query to reach the responder", WITHIN, || {
        extras.hooks.pre_matches_asked().len() == senders as usize
    })
    .await;
    eventually("the tasks of the queries to end", WITHIN, || {
        lab.rig.tun.session_tasks() == 0
    })
    .await;
    // the writer holds one batch in its write and the queue one more
    let lost = lab.counters().queue_full;
    assert!(
        lost >= senders - 2,
        "{lost} answers were lost, at least {} were to be",
        senders - 2
    );
    gate.open();
    let mut written = 0;
    tokio::time::sleep(NOTHING).await;
    while let Ok(packet) = lab.udp.try_recv() {
        packets::read_udp(&packet).expect("an answer");
        written += 1;
    }
    assert_eq!(
        written + lost,
        senders,
        "every answer was written or counted"
    );
    assert_eq!(
        lab.counters(),
        Counters {
            queue_full: lost,
            ..Counters::default()
        }
    );
}

// ---- a session of a resolver

/// A session whose app has sent nothing but to port 53 ends after
/// `udp_dns_idle`; one that has sent to another port as well keeps `udp_idle`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_only_talks_dns_ends_sooner() {
    let dns_idle = Duration::from_millis(300);
    let lab = lab(TunOptions {
        udp_idle: Duration::from_secs(30),
        udp_dns_idle: dns_idle,
        ..TunOptions::default()
    });
    let (only_dns, both) = (addr("198.18.0.1:5000"), addr("198.18.0.1:5001"));
    // no `hijack-dns` entry: port 53 of a server is the app's own traffic
    let (dns_port, other_port) = (addr("192.0.2.10:53"), addr("192.0.2.10:443"));
    let begun = Instant::now();
    lab.send(only_dns, dns_port, b"one");
    lab.send(both, dns_port, b"one");
    lab.send(both, other_port, b"two");
    eventually("both associations to be open", WITHIN, || {
        lab.rig.dialer.associations_open() == 2
    })
    .await;
    eventually("the session of the resolver to end", WITHIN, || {
        !lab.rig.dialer.association_open(only_dns)
    })
    .await;
    assert!(
        begun.elapsed() >= dns_idle,
        "the session ended after {:?}, before its idle time",
        begun.elapsed()
    );
    assert!(
        lab.rig.dialer.association_open(both),
        "the session that sent to port 443 too is still there"
    );
}

/// The destinations of a resolver's session are let go of after `udp_idle`, as
/// those of any session are: only the session itself is kept for the shorter
/// `udp_dns_idle`. The app sends to port 53 of two names' fake addresses and
/// nowhere else (no `hijack-dns`, so those are a session's). The pool has two
/// addresses, so the lease on an address shows whether the session still has
/// the destination: a third name is given one only when a destination is let
/// go of. One datagram goes to the first name and then, 100 ms apart, they go
/// to the second, which keeps the session (it is a resolver's, and ends after
/// 300 ms of quiet) and leaves the first destination quiet.
///
/// The times: `udp_dns_idle` 300 ms and `udp_idle` 2 s, so the sweep ticks
/// every 500 ms. A sweep that went by `udp_dns_idle` would let the first
/// destination go at the first tick, about 500 ms after its datagram; one that
/// goes by `udp_idle` does so at the fourth, about 2 s after it, and not
/// sooner. The first destination is checked to be held for 1.2 s after its
/// datagram: 700 ms after the wrong sweep would have let it go, 800 ms before
/// the right one does. The datagrams to the second destination come 200 ms
/// sooner than the session would end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resolvers_destinations_are_kept_for_udp_idle() {
    let pool = Arc::new(FakeIpPool::with_count(2));
    let idle = Duration::from_secs(2);
    let lab = lab_and_extras(
        TunOptions {
            udp_idle: idle,
            udp_dns_idle: Duration::from_millis(300),
            ..TunOptions::default()
        },
        Extras {
            pool: Some(pool.clone()),
            ..Extras::default()
        },
    )
    .0;
    let now = Instant::now();
    let first = pool.assign("a.test", now).expect("an address for a.test");
    let second = pool.assign("b.test", now).expect("an address for b.test");
    let (app, to_first, to_second) = (addr(APP4), sock(v4_of(first), 53), sock(v4_of(second), 53));
    let begun = Instant::now();
    lab.send(app, to_first, b"one");
    lab.send(app, to_second, b"q");
    // both destinations are in the session, each with its lease: the dialer
    // has the datagram to the second, which is queued after its lease is taken
    lab.sent_times(2).await;
    for _ in 0..12 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        lab.send(app, to_second, b"q");
        assert_eq!(
            pool.assign("c.test", Instant::now()),
            Err(AssignError::Exhausted),
            "the first destination was let go of {:?} after its datagram, a resolver's session or not",
            begun.elapsed()
        );
    }
    // it goes once it has been quiet for `udp_idle`, and not before; the second
    // destination keeps the session until then
    let released = loop {
        assert!(
            begun.elapsed() < WITHIN,
            "the first destination was never let go of"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        lab.send(app, to_second, b"q");
        if pool.assign("c.test", Instant::now()).is_ok() {
            break begun.elapsed();
        }
    };
    assert!(
        released >= idle,
        "the first destination was let go of {released:?} after its datagram, before `udp_idle`"
    );
    assert!(lab.rig.dialer.association_open(app), "the session ended");
}

/// A session is the resolver's until its app sends to another port, and then
/// it is not for good: a destination at that port that is let go of, with the
/// session going on, does not make it the resolver's again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swept_destination_does_not_make_a_session_a_resolvers_again() {
    let pool = Arc::new(FakeIpPool::with_count(1));
    // a destination is let go of after `udp_idle` (1.2 s) of quiet, at one of
    // the sweep's ticks, which come every `udp_idle` / 4 (300 ms); the session
    // ends after 200 ms of quiet if it is a resolver's, and after 1.2 s if it
    // is not
    let lab = lab_and_extras(
        TunOptions {
            udp_idle: Duration::from_millis(1_200),
            udp_dns_idle: Duration::from_millis(200),
            ..TunOptions::default()
        },
        Extras {
            pool: Some(pool.clone()),
            ..Extras::default()
        },
    )
    .0;
    let index = pool
        .assign("a.test", Instant::now())
        .expect("an address for a.test");
    let (app, resolver) = (addr(APP4), addr("192.0.2.10:53"));
    // the first datagram goes to port 443 of a name; its fake address is leased
    // for as long as the session has the destination
    lab.send(app, sock(v4_of(index), 443), b"x");
    eventually("the association to be open", WITHIN, || {
        lab.rig.dialer.association_open(app)
    })
    .await;
    // the datagrams that follow go to port 53, 100 ms apart, for 1.8 s: that
    // keeps the session, and lets the destination at 443 go quiet
    let mut released = false;
    for _ in 0..18 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        lab.send(app, resolver, b"q");
        released |= pool.assign("b.test", Instant::now()).is_ok();
    }
    assert!(released, "the destination at port 443 was let go of");
    // now 500 ms of nothing: more than `udp_dns_idle`, less than `udp_idle`
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        lab.rig.dialer.association_open(app),
        "the session ended as a resolver's would"
    );
    eventually("the session to end", WITHIN, || {
        !lab.rig.dialer.association_open(app)
    })
    .await;
}
