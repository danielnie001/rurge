//! Pre-matching at the SYN layer (phase 3 M3 design 5.2 and 12): a new SYN is
//! asked of the pre-matching rules after the TUN has stopped or the caps are
//! met, the address looked at and, for an address that stands for a name, the
//! name found and a lease taken on the address, and before a socket is made.
//! A rule that refuses it has it answered with a reset, through the limit on
//! rurge's own resets, or (`REJECT-DROP`) dropped without a word; the SYN keeps
//! nothing, neither a place in the caps nor the lease. First one shard, its turns
//! taken by hand, where what a turn gives out and what it keeps can be read
//! exactly; then the whole TUN, with the app's stack (`connect`) and with SYNs
//! the test sends itself (`inject`).

use super::hazards::{app4, open_by_hand};
use super::log_capture::LogCapture;
use super::support::{
    Extras, Lone, WITHIN, addr, eventually, hello, is_reset, lone_shard, lone_shard_with_pool,
    rig_extras, sock, syn_v4, syn_v6, tcp_packet, tcp_repr, within,
};
use super::udp::NOTHING;
use crate::packet::segment;
use crate::space::{ADAPTER_V4, ADAPTER_V6};
use crate::tcp::Tuple;
use crate::testing::{FakeHooks, Script, Upstream};
use crate::{Counters, Ending, TunOptions};
use rurge_config::{HostName, ListenerKind, PreMatchLayer, SessionInfo, Transport};
use rurge_dns::fakeip::{AssignError, FakeIpPool, v4_of, v6_of};
use rurge_proto::RejectKind;
use smoltcp::wire::{TcpControl, TcpRepr, TcpSeqNumber};
use std::collections::HashSet;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

/// The sessions the rules were asked about at the SYN layer, in order.
fn asked_at_syn(hooks: &FakeHooks) -> Vec<SessionInfo> {
    hooks
        .pre_matches_asked()
        .into_iter()
        .filter(|(layer, _)| *layer == PreMatchLayer::Syn)
        .map(|(_, session)| session)
        .collect()
}

fn syn_layer(hooks: &FakeHooks, dst: HostName, kind: RejectKind) {
    hooks.script_pre_match(PreMatchLayer::Syn, dst, kind);
}

fn named(name: &str) -> HostName {
    HostName::Domain(name.to_owned())
}

/// The kinds a rule refuses with.
const KINDS: [RejectKind; 4] = [
    RejectKind::Reject,
    RejectKind::NoDrop,
    RejectKind::TinyGif,
    RejectKind::Drop,
];

// ---- one shard, its turns taken by hand

/// A lone shard whose pool has one address only, and that address given to
/// `name`: the pool, the fake endpoint at port 80 and the shard.
fn one_address(name: &str, epoch: Instant) -> (Arc<FakeIpPool>, SocketAddr, Lone) {
    let pool = Arc::new(FakeIpPool::with_count(1));
    let index = pool.assign(name, epoch).expect("an address for the name");
    let lone = lone_shard_with_pool(&TunOptions::default(), epoch, Some(pool.clone()));
    (pool, sock(v4_of(index), 80), lone)
}

/// A SYN that a rule refuses, with any kind of refusal, has nothing of its
/// own after the turn: no socket, no place in the caps, no budget, and no lease
/// on the address of the name, which the pool has to give to another name, there
/// being no other address. The control: with the connection of a SYN no rule
/// refused open, the pool has no address to give.
#[test]
fn a_refused_syn_holds_no_lease() {
    {
        let epoch = Instant::now();
        let (pool, fake, lone) = one_address("a.test", epoch);
        let _connection = open_by_hand(&lone.shard, epoch, app4(39_999), fake, 0);
        assert_eq!(
            pool.assign("b.test", epoch),
            Err(AssignError::Exhausted),
            "a connection holds its address: without this the test would not tell a lease kept from one given back"
        );
    }
    for kind in KINDS {
        let epoch = Instant::now();
        let (pool, fake, lone) = one_address("a.test", epoch);
        syn_layer(&lone.hooks, named("a.test"), kind);
        let turn = lone
            .shard
            .state()
            .turn(epoch, &mut vec![syn_v4(app4(40_000), fake)]);
        assert!(turn.opened.is_empty(), "{kind:?}: a connection was made");
        assert_eq!(
            turn.out.len(),
            usize::from(kind != RejectKind::Drop),
            "{kind:?}"
        );
        assert_eq!(lone.shard.state().socket_count(), 0, "{kind:?}");
        assert_eq!(lone.admission.sockets(), 0, "{kind:?}");
        assert_eq!(lone.admission.half_open(), 0, "{kind:?}");
        assert_eq!(lone.budget.used(), 0, "{kind:?}");
        assert_eq!(
            pool.assign("b.test", epoch),
            Ok(0),
            "{kind:?}: the lease on the address was kept"
        );
        assert_eq!(lone.stats.counters(), Counters::default(), "{kind:?}");
    }
}

/// A SYN that passes the caps, the address and the rules but that smoltcp does
/// not take (a listening socket ignores a SYN whose MSS option is 0: smoltcp
/// 0.12 tcp.rs, the `(State::Listen, TcpControl::Syn)` arm of `process`, which
/// leaves the socket listening; the filter has nothing against the option, so
/// this reaches the shard as any SYN does) is no connection: the socket made
/// for it goes, and what the SYN held with it, the places in the caps, the
/// budget and the lease on the address of the name behind it, is given back.
#[test]
fn a_syn_the_stack_does_not_take_gives_its_lease_back() {
    let epoch = Instant::now();
    let (pool, fake, lone) = one_address("a.test", epoch);
    let app = app4(40_000);
    let repr = TcpRepr {
        control: TcpControl::Syn,
        seq_number: TcpSeqNumber(1),
        max_seg_size: Some(0),
        ..tcp_repr(app, fake, &[])
    };
    let turn = lone
        .shard
        .state()
        .turn(epoch, &mut vec![tcp_packet(app, fake, &repr)]);
    assert!(turn.opened.is_empty());
    assert!(
        turn.out.is_empty(),
        "smoltcp answers a SYN it does not take with nothing"
    );
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.admission.half_open(), 0);
    assert_eq!(lone.budget.used(), 0);
    assert_eq!(
        pool.assign("b.test", epoch),
        Ok(0),
        "the lease on the address was kept"
    );
    assert_eq!(lone.stats.counters(), Counters::default());
}

/// The rules are asked before the stack is: a SYN that the stack would not take
/// (see above) but that a rule refuses is answered with a reset, which the stack
/// would not have given.
#[test]
fn the_rules_are_asked_before_the_stack_is() {
    let epoch = Instant::now();
    let lone = lone_shard(&TunOptions::default(), epoch);
    let (app, dst) = (app4(40_000), addr("192.0.2.7:443"));
    syn_layer(&lone.hooks, HostName::Ip(dst.ip()), RejectKind::Reject);
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
    assert_eq!(turn.out.len(), 1, "a reset");
    assert!(is_reset(&turn.out[0]));
    assert_eq!(asked_at_syn(&lone.hooks).len(), 1);
}

/// A SYN that `REJECT-DROP` refuses is dropped and counted nowhere, and said
/// once at debug level (the line may carry the name and the endpoints, as the
/// debug lines of the other drops carry the addresses); a reset sent for any
/// other kind is said nowhere.
#[test]
fn a_dropped_syn_is_said_once_at_debug_level_and_counted_nowhere() {
    let log = LogCapture::start();
    let epoch = Instant::now();
    let (_pool, fake, lone) = one_address("a.test", epoch);
    let real = addr("192.0.2.7:443");
    syn_layer(&lone.hooks, named("a.test"), RejectKind::Drop);
    syn_layer(&lone.hooks, HostName::Ip(real.ip()), RejectKind::Drop);
    let turn = lone.shard.state().turn(
        epoch,
        &mut vec![syn_v4(app4(40_000), fake), syn_v4(app4(40_001), real)],
    );
    assert!(turn.opened.is_empty());
    assert!(turn.out.is_empty(), "nothing is sent: {:?}", turn.out);
    assert_eq!(lone.stats.counters(), Counters::default());
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.admission.half_open(), 0);
    let lines = log.lines();
    assert_eq!(lines.len(), 2, "a line for each SYN: {lines:?}");
    for line in &lines {
        assert!(line.starts_with("DEBUG "), "{line}");
    }
    // and a reset for a refusal is no news
    syn_layer(&lone.hooks, HostName::Ip(real.ip()), RejectKind::Reject);
    let turn = lone
        .shard
        .state()
        .turn(epoch, &mut vec![syn_v4(app4(40_002), real)]);
    assert_eq!(turn.out.len(), 1);
    assert!(is_reset(&turn.out[0]));
    assert_eq!(log.lines().len(), 2, "something was said of a reset");
}

/// A SYN is asked of the rules after every check that turns it away on its
/// own: a stopping TUN, the caps, an address of rurge's that is no place for a
/// connection, an address that stands for a name nobody knows. A rule that
/// would refuse each of them is never asked, and what each is counted as is
/// what it was counted as before there were rules.
#[test]
fn a_syn_turned_away_before_the_rules_is_not_asked_of_them() {
    let epoch = Instant::now();
    let real = addr("192.0.2.7:443");
    let unknown_fake = addr("198.18.1.1:80");
    // the stop
    let lone = lone_shard(&TunOptions::default(), epoch);
    syn_layer(&lone.hooks, HostName::Ip(real.ip()), RejectKind::Drop);
    lone.life.end(Ending::Stopped);
    let turn = lone
        .shard
        .state()
        .turn(epoch, &mut vec![syn_v4(app4(40_000), real)]);
    assert_eq!(turn.out.len(), 1, "a reset, not silence");
    assert!(asked_at_syn(&lone.hooks).is_empty(), "stopping");
    // the caps: the half-open one, and the sockets'
    for options in [
        TunOptions {
            max_half_open: 0,
            ..TunOptions::default()
        },
        TunOptions {
            max_sockets: 0,
            ..TunOptions::default()
        },
    ] {
        let lone = lone_shard(&options, epoch);
        syn_layer(&lone.hooks, HostName::Ip(real.ip()), RejectKind::Drop);
        let turn = lone
            .shard
            .state()
            .turn(epoch, &mut vec![syn_v4(app4(40_000), real)]);
        assert_eq!(turn.out.len(), 1, "a reset, not silence: {options:?}");
        assert!(asked_at_syn(&lone.hooks).is_empty(), "{options:?}");
        assert_eq!(lone.stats.counters().busy, 1, "{options:?}");
    }
    // the addresses of rurge's: its own, the responder's other ports, a name
    // nobody knows
    let lone = lone_shard(&TunOptions::default(), epoch);
    let mut syns = Vec::new();
    for (i, dst) in [
        addr("198.18.0.1:80"),
        addr("198.18.0.244:443"),
        addr("198.18.0.2:54"),
        unknown_fake,
    ]
    .into_iter()
    .enumerate()
    {
        syn_layer(&lone.hooks, HostName::Ip(dst.ip()), RejectKind::Drop);
        syns.push(syn_v4(app4(40_000 + i as u16), dst));
    }
    let turn = lone.shard.state().turn(epoch, &mut syns);
    assert_eq!(turn.out.len(), 4, "a reset for each");
    assert!(asked_at_syn(&lone.hooks).is_empty());
    assert_eq!(
        lone.stats.counters(),
        Counters {
            unknown_fake: 1,
            ..Counters::default()
        }
    );
}

/// The responder's port 53 is where a connection of its own is made, with no
/// rule asked about it, whatever the rules say of the address.
#[test]
fn a_syn_to_the_responders_port_53_is_not_asked_of_the_rules() {
    let epoch = Instant::now();
    let lone = lone_shard(&TunOptions::default(), epoch);
    let responders = [
        addr("198.18.0.2:53"),
        addr("198.18.0.9:53"),
        addr("[fd00:6152::2]:53"),
    ];
    let mut syns = Vec::new();
    for (i, dst) in responders.into_iter().enumerate() {
        syn_layer(&lone.hooks, HostName::Ip(dst.ip()), RejectKind::Reject);
        let port = 40_000 + i as u16;
        syns.push(match dst {
            SocketAddr::V4(_) => syn_v4(sock(ADAPTER_V4, port), dst),
            SocketAddr::V6(_) => syn_v6(sock(ADAPTER_V6, port), dst),
        });
    }
    let turn = lone.shard.state().turn(epoch, &mut syns);
    let opened: HashSet<Tuple> = turn.opened.iter().map(|opened| opened.tuple).collect();
    let wanted: HashSet<Tuple> = turn
        .out
        .iter()
        .filter_map(|packet| segment(packet))
        .map(|syn_ack| Tuple {
            src: syn_ack.tuple.dst,
            dst: syn_ack.tuple.src,
        })
        .collect();
    assert_eq!(opened.len(), 3, "a connection for each");
    assert_eq!(opened, wanted, "and a SYN-ACK for each");
    assert!(asked_at_syn(&lone.hooks).is_empty());
}

// ---- the whole TUN

/// A reset for a SYN a rule refuses, whatever the kind, and for a SYN to a
/// real address as to one to a name: the app's `connect` is refused, nothing is
/// dialed, the SYN is counted as nothing, and what it took of the caps is
/// back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_syn_refused_by_pre_matching_is_reset() {
    let (rig, extras) = rig_extras(TunOptions::default(), Extras::default());
    let pool = rig.tun.pool().expect("the rig's TUN has a pool");
    let mut destinations = Vec::new();
    for (i, kind) in [RejectKind::Reject, RejectKind::NoDrop, RejectKind::TinyGif]
        .into_iter()
        .enumerate()
    {
        let name = format!("refused{i}.test");
        let index = pool
            .assign(&name, Instant::now())
            .expect("an address for the name");
        syn_layer(&extras.hooks, named(&name), kind);
        destinations.push((sock(v4_of(index), 80), kind));
    }
    let real = addr("192.0.2.7:443");
    syn_layer(&extras.hooks, HostName::Ip(real.ip()), RejectKind::Reject);
    destinations.push((real, RejectKind::Reject));
    for (dst, kind) in &destinations {
        let error = within("the connection to be refused", rig.app.connect(*dst))
            .await
            .expect_err("a connection a rule refuses is refused");
        assert_eq!(
            error.kind(),
            io::ErrorKind::ConnectionRefused,
            "{dst} {kind:?}"
        );
    }
    assert!(rig.dialer.dialed().is_empty(), "nothing is dialed");
    assert_eq!(asked_at_syn(&extras.hooks).len(), destinations.len());
    assert_eq!(rig.tun.counters(), Counters::default());
    assert_eq!(rig.tun.sockets_in_use(), 0);
    assert_eq!(rig.tun.sockets_counted(), 0);
    assert_eq!(rig.tun.half_open(), 0);
    assert_eq!(rig.tun.budget_in_use(), 0);
}

/// A SYN that `REJECT-DROP` refuses gets no answer of any kind: the app's
/// stack, which sent its SYN once, has taken in nothing a moment after the
/// rules were asked, and its `connect` is pending still.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_syn_dropped_by_pre_matching_gets_nothing() {
    let (rig, extras) = rig_extras(TunOptions::default(), Extras::default());
    let pool = rig.tun.pool().expect("the rig's TUN has a pool");
    let index = pool
        .assign("dropped.test", Instant::now())
        .expect("an address for the name");
    syn_layer(&extras.hooks, named("dropped.test"), RejectKind::Drop);
    let app = rig.app.clone();
    let fake = sock(v4_of(index), 80);
    let connecting = tokio::spawn(async move { app.connect(fake).await });
    eventually("the SYN to be asked of the rules", WITHIN, || {
        !extras.hooks.pre_matches_asked().is_empty()
    })
    .await;
    // The one fixed wait of these tests, for what must not happen: a reply to
    // the SYN. A reset or a SYN-ACK would have been through the writer and the
    // app's stack in well under a millisecond of the rules being asked. The
    // wait ends long before the app's stack sends its SYN again: it does so
    // after the retransmission timeout smoltcp 0.12 starts from, 700 ms (300 ms
    // of round trip and four times a deviation of 100 ms), so a second SYN is
    // not what the wait could see answered, and the margin is 500 ms.
    tokio::time::sleep(NOTHING).await;
    let (taken, _) = rig.app.packets();
    assert_eq!(taken, 0, "the app was sent a packet");
    assert!(!connecting.is_finished(), "the connect has an answer");
    assert_eq!(asked_at_syn(&extras.hooks).len(), 1, "one SYN");
    assert!(rig.dialer.dialed().is_empty());
    assert_eq!(rig.tun.counters(), Counters::default());
    assert_eq!(rig.tun.sockets_counted(), 0);
    assert_eq!(rig.tun.half_open(), 0);
    connecting.abort();
}

/// What the rules are asked about is the connection the engine is given: one
/// session, made in one place, so that they cannot differ. Its fields are the
/// app's endpoint as the source, the name a fake address maps back to or else
/// the destination address, the destination port, TCP, the adapter as the
/// listener and the endpoint the app connected to as rurge's end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_syn_layer_session_is_the_connection() {
    let (rig, extras) = rig_extras(TunOptions::default(), Extras::default());
    let pool = rig.tun.pool().expect("the rig's TUN has a pool");
    let index = pool
        .assign("a.test", Instant::now())
        .expect("an address for the name");
    let echo = Upstream::echo().await;
    // a name behind a fake address of each family, and a real address of each
    let to = [
        (sock(v4_of(index), 80), named("a.test")),
        (sock(v6_of(index), 8080), named("a.test")),
        (
            addr("192.0.2.7:443"),
            HostName::Ip(addr("192.0.2.7:443").ip()),
        ),
        (
            addr("[2001:db8::7]:853"),
            HostName::Ip(addr("[2001:db8::7]:853").ip()),
        ),
    ];
    let mut apps = Vec::new();
    for (dst, _) in &to {
        rig.dialer.script(*dst, Script::Upstream(echo.addr()));
        let mut tcp = within("the connection to be made", rig.app.connect(*dst))
            .await
            .expect("connect");
        hello(&mut tcp).await;
        apps.push(tcp);
    }
    let asked = asked_at_syn(&extras.hooks);
    assert_eq!(
        asked.len(),
        to.len(),
        "one ask for each connection: {asked:?}"
    );
    for ((session, tcp), (dst, host)) in asked.iter().zip(&apps).zip(&to) {
        assert_eq!(session.src, tcp.local(), "{dst}");
        assert_eq!(session.in_port, 0, "{dst}");
        assert_eq!(session.local, Some(*dst), "{dst}");
        assert_eq!(session.listener, ListenerKind::Tun, "{dst}");
        assert_eq!(&session.dst_host, host, "{dst}");
        assert_eq!(session.dst_port, dst.port(), "{dst}");
        assert_eq!(session.transport, Transport::Tcp, "{dst}");
    }
    // and it is the session the dialer was given, field for field
    assert_eq!(asked, rig.dialer.dialed());
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// A reset for a refusal is one of rurge's own, so it is let through 100
/// times in 3 s and not the 101st, which is dropped and counted `rst_limited`
/// and nothing else. A SYN that is refused keeps no place in the caps: with
/// room for five sockets and five half-open connections, no SYN of the
/// hundred and one is turned away for being over a cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resets_for_pre_matching_are_limited() {
    let (rig, extras) = rig_extras(
        TunOptions {
            max_sockets: 5,
            max_half_open: 5,
            ..TunOptions::default()
        },
        Extras::default(),
    );
    let pool = rig.tun.pool().expect("the rig's TUN has a pool");
    let index = pool
        .assign("refused.test", Instant::now())
        .expect("an address for the name");
    syn_layer(&extras.hooks, named("refused.test"), RejectKind::Reject);
    let fake = sock(v4_of(index), 80);
    let mut replies = rig.app.divert(40_000..=40_100);
    for port in 40_000..=40_100 {
        rig.app.inject(syn_v4(sock(ADAPTER_V4, port), fake));
    }
    let mut answered = HashSet::new();
    for _ in 0..100 {
        let reply = within("a packet from the TUN", replies.recv())
            .await
            .expect("the app's stack goes on handing packets over");
        assert!(is_reset(&reply), "a reset, not {reply:?}");
        answered.insert(segment(&reply).expect("a segment").tuple.dst.port());
    }
    assert_eq!(answered.len(), 100, "each to a SYN of its own");
    eventually("the 101st reset to be held back", WITHIN, || {
        rig.tun.counters().rst_limited == 1
    })
    .await;
    assert!(replies.try_recv().is_err(), "no 101st reset");
    assert_eq!(
        asked_at_syn(&extras.hooks).len(),
        101,
        "every SYN was asked"
    );
    assert_eq!(
        rig.tun.counters(),
        Counters {
            rst_limited: 1,
            ..Counters::default()
        }
    );
    assert_eq!(rig.tun.sockets_counted(), 0);
    assert_eq!(rig.tun.half_open(), 0);
    assert_eq!(rig.tun.sockets_in_use(), 0);
}
