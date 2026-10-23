//! DNS over TCP (phase 3 M3 design 11.4): a connection to port 53 of the
//! responder is made as any connection is, the handshake being the stack's, and
//! is then the responder's and not the engine's. The TUN reads the queries on
//! it, each a two-byte length and the message, and answers them one at a time in
//! the order they came, with nothing cut short as it is over UDP. It closes the
//! connection when the app does, when no query has come for `dns_tcp_idle` (the
//! wait for an answer being forwarded is no part of that), and at a message that
//! gets no answer at all. The app is the stack of `AppStack`, which reads and
//! writes the connection; the doubles behind the responder are those of the
//! other tests here. A connection to a server `hijack-dns` names is an ordinary
//! one, dialed by the fake dialer.

use super::udp::extras_hijacking;
use super::{A, AAAA, TXT, counts, echoed, id_of, parsed, query, rcode_of, truncated};
use crate::testing::{AppTcp, ForwardScript, Script, Upstream};
use crate::tests::log_capture::LogCapture;
use crate::tests::support::{
    Extras, WITHIN, addr, eventually, hello, in_five_seconds, sock, within,
};
use crate::tests::udp::{APP4, Lab, lab_and_extras};
use crate::{Counters, Ending, TunOptions};
use rurge_config::{HostName, PreMatchLayer};
use rurge_dns::fakeip::{FakeIpPool, v4_of};
use rurge_dns::message::Rcode;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const RESPONDER4: &str = "198.18.0.2:53";
const RESPONDER6: &str = "[fd00:6152::2]:53";

/// `message` as DNS over TCP frames it: a two-byte length, then the message.
fn framed(message: &[u8]) -> Vec<u8> {
    let length = u16::try_from(message.len()).expect("a message of 65,535 bytes at most");
    let mut out = length.to_be_bytes().to_vec();
    out.extend_from_slice(message);
    out
}

async fn connect(lab: &Lab, to: &str) -> AppTcp {
    within("the connection to be made", lab.rig.app.connect(addr(to)))
        .await
        .expect("connect")
}

/// The next message the TUN wrote on `tcp`, which has to come in time: its
/// length, and that many bytes after it.
async fn next_message(tcp: &mut AppTcp) -> Vec<u8> {
    let mut length = [0u8; 2];
    within("the length of a message", tcp.read_exact(&mut length))
        .await
        .expect("the length of a message");
    let mut message = vec![0u8; usize::from(u16::from_be_bytes(length))];
    within("a message", tcp.read_exact(&mut message))
        .await
        .expect("a message of the length it said");
    message
}

/// The TUN has closed its side of the connection, and wrote nothing before it:
/// the end of the stream, not an error. It does so in five seconds, which is
/// far longer than the idle times these tests close a connection for (300 ms
/// at the longest), and less than the default one.
async fn assert_closed(tcp: &mut AppTcp) {
    let mut byte = [0u8; 1];
    let read = in_five_seconds("the TUN to close the connection", tcp.read(&mut byte))
        .await
        .expect("the end of the stream, not an error");
    assert_eq!(read, 0, "bytes before the end of the stream");
}

/// `reply` answers the query with this ID with the fake address `wanted`, the
/// TTL of a fake address (30 seconds) on it.
fn assert_fake_reply(reply: &[u8], id: u16, wanted: IpAddr) {
    let answer = parsed(reply);
    assert_eq!(answer.id, id, "the ID of the query");
    assert_eq!(answer.rcode, Rcode::NoError);
    match wanted {
        IpAddr::V4(wanted) => assert_eq!(answer.v4, [(wanted, 30)]),
        IpAddr::V6(wanted) => assert_eq!(answer.v6, [(wanted, 30)]),
    }
}

/// The connection has ended and the TUN let go of it: the app closed its side
/// too (it is shut down here), no socket is left and no task of a session.
async fn assert_let_go(lab: &Lab, tcp: &mut AppTcp) {
    tcp.shutdown().await.expect("shutdown");
    eventually("the TUN to let the connection go", WITHIN, || {
        lab.rig.tun.sockets_in_use() == 0 && lab.rig.tun.session_tasks() == 0
    })
    .await;
    assert_eq!(lab.rig.tun.sockets_counted(), 0);
    assert_eq!(lab.rig.tun.half_open(), 0);
    assert_eq!(lab.rig.tun.budget_in_use(), 0);
}

/// Two queries in one write on one connection are answered in the order they
/// came, each behind a length that is its own, from the responder's IPv4
/// address; a query that follows on the same connection, and one to the IPv6
/// address from the app's IPv6 address, are answered as well. No session was
/// dialed for any of it, and the rules were asked about the names, not about
/// the connections: the responder's port 53 is not pre-matched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_query_over_tcp_is_answered() {
    let (lab, extras) = lab_and_extras(TunOptions::default(), Extras::default());
    let mut tcp = connect(&lab, RESPONDER4).await;
    let mut both = framed(&query(1, "a.test", A));
    both.extend(framed(&query(2, "b.test", A)));
    tcp.write_all(&both).await.expect("write");
    for (id, name) in [(1, "a.test"), (2, "b.test")] {
        let reply = next_message(&mut tcp).await;
        assert_fake_reply(&reply, id, lab.fake(name, false));
    }
    tcp.write_all(&framed(&query(3, "a.test", A)))
        .await
        .expect("write");
    let reply = next_message(&mut tcp).await;
    assert_fake_reply(&reply, 3, lab.fake("a.test", false));
    let mut tcp6 = connect(&lab, RESPONDER6).await;
    tcp6.write_all(&framed(&query(4, "a.test", AAAA)))
        .await
        .expect("write");
    let reply = next_message(&mut tcp6).await;
    assert_fake_reply(&reply, 4, lab.fake("a.test", true));
    assert!(lab.rig.dialer.dialed().is_empty(), "nothing is dialed");
    assert!(
        extras
            .hooks
            .pre_matches_asked()
            .iter()
            .all(|(layer, _)| *layer == PreMatchLayer::Dns),
        "a rule was asked about a connection to the responder"
    );
    assert_eq!(lab.rig.tun.counters(), Counters::default());
}

/// A reply of more than a UDP datagram carries is cut short over UDP, to the
/// message that tells the app to ask again over TCP (TC set, no record, the
/// code of the upstream's reply), and over TCP the same query gets the whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_large_answer_is_truncated_and_tcp_gets_it_whole() {
    let (mut lab, extras) = lab_and_extras(TunOptions::default(), Extras::default());
    extras
        .dns
        .script("big.test", ForwardScript::Echo { pad: 2000 });
    let asked = query(7, "big.test", TXT);
    let responder = addr(RESPONDER4);
    lab.send(addr(APP4), responder, &asked);
    let seen = lab.datagram().await;
    assert_eq!((seen.src, seen.dst), (responder, addr(APP4)));
    assert!(truncated(&seen.payload), "TC over UDP");
    assert_eq!(counts(&seen.payload)[1], 0, "no record over UDP");
    assert_eq!(rcode_of(&seen.payload), Rcode::NoError);
    let mut tcp = connect(&lab, RESPONDER4).await;
    tcp.write_all(&framed(&asked)).await.expect("write");
    let whole = next_message(&mut tcp).await;
    assert_eq!(
        whole,
        echoed(&asked, 2000),
        "the reply of the upstream, whole"
    );
    assert!(!truncated(&whole));
    assert_eq!(id_of(&whole), 7);
    assert_eq!(
        extras.dns.forwarded().len(),
        2,
        "forwarded for each transport"
    );
}

/// A connection with no query on it is closed after `dns_tcp_idle`, with a FIN,
/// not a reset; it is let go of once the app closes its side too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dns_connection_idle_is_closed() {
    let idle = Duration::from_millis(300);
    let (lab, _) = lab_and_extras(
        TunOptions {
            dns_tcp_idle: idle,
            ..TunOptions::default()
        },
        Extras::default(),
    );
    let begun = Instant::now();
    let mut tcp = connect(&lab, RESPONDER4).await;
    assert_closed(&mut tcp).await;
    assert!(
        begun.elapsed() >= idle,
        "closed after {:?}, before its idle time",
        begun.elapsed()
    );
    assert_let_go(&lab, &mut tcp).await;
    assert_eq!(lab.rig.tun.counters(), Counters::default());
}

/// The idle time is that of the wait for the next query, not the age of the
/// connection: a query a moment before the connection would be closed for being
/// idle, again and again, keeps it. The queries come 400 ms apart on a
/// connection that is closed after a second without one; the fourth is on a
/// connection twice that old, which the idle time counted from the connection's
/// start would have closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answered_query_starts_the_idle_time_again() {
    let idle = Duration::from_secs(1);
    let (lab, _) = lab_and_extras(
        TunOptions {
            dns_tcp_idle: idle,
            ..TunOptions::default()
        },
        Extras::default(),
    );
    let mut tcp = connect(&lab, RESPONDER4).await;
    let begun = Instant::now();
    for id in 0..4u16 {
        tcp.write_all(&framed(&query(id, "a.test", A)))
            .await
            .expect("write");
        let reply = next_message(&mut tcp).await;
        assert_fake_reply(&reply, id, lab.fake("a.test", false));
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    assert!(
        begun.elapsed() > idle,
        "the connection is no older than the idle time"
    );
    // and still there to be asked, once more
    tcp.write_all(&framed(&query(9, "a.test", A)))
        .await
        .expect("write");
    assert_fake_reply(&next_message(&mut tcp).await, 9, lab.fake("a.test", false));
}

/// The time an answer takes to come is no idle time: a query the upstream holds
/// for longer than three times `dns_tcp_idle` keeps the connection open, and
/// the answer comes down it when the upstream lets go.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forward_in_progress_is_not_idle_time() {
    let idle = Duration::from_millis(300);
    let (lab, extras) = lab_and_extras(
        TunOptions {
            dns_tcp_idle: idle,
            ..TunOptions::default()
        },
        Extras::default(),
    );
    extras.dns.script("held.test", ForwardScript::Hold);
    let mut tcp = connect(&lab, RESPONDER4).await;
    let asked = query(5, "held.test", TXT);
    tcp.write_all(&framed(&asked)).await.expect("write");
    eventually("the query to be forwarded", WITHIN, || {
        extras.dns.forwarded().len() == 1
    })
    .await;
    // The one fixed wait of this test, for what must not happen: the connection
    // closed, or written to, while the upstream holds the query. Were the wait
    // for the answer counted as idle time the TUN would close it after `idle`,
    // 300 ms; the wait is three times that, so the margin is 600 ms.
    let mut byte = [0u8; 1];
    let waited = tokio::time::timeout(3 * idle, tcp.read(&mut byte)).await;
    assert!(
        waited.is_err(),
        "the connection was closed or written to: {waited:?}"
    );
    extras.dns.release();
    let reply = next_message(&mut tcp).await;
    assert_eq!(reply, echoed(&asked, 0));
}

/// An app that asks and reads nothing is not kept for ever: the connection is
/// reset when an answer that it does not read has waited `dns_tcp_idle` to be
/// written. The sockets of this TUN have 4 KiB for what they send, and the app's
/// stack takes 64 KiB: the second of two answers of 60,000 bytes does not fit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_that_reads_nothing_has_its_connection_reset() {
    let (lab, extras) = lab_and_extras(
        TunOptions {
            budget: 0,
            dns_tcp_idle: Duration::from_millis(300),
            ..TunOptions::default()
        },
        Extras::default(),
    );
    extras
        .dns
        .script("big.test", ForwardScript::Echo { pad: 60_000 });
    let mut tcp = connect(&lab, RESPONDER4).await;
    let mut both = framed(&query(1, "big.test", TXT));
    both.extend(framed(&query(2, "big.test", TXT)));
    tcp.write_all(&both).await.expect("write");
    eventually("the TUN to let the connection go", WITHIN, || {
        lab.rig.tun.sockets_in_use() == 0 && lab.rig.tun.session_tasks() == 0
    })
    .await;
    assert_eq!(extras.dns.forwarded().len(), 2);
    // The app reads what its stack took in before the reset (the TUN has let go
    // of the connection once it has handed the reset on, which the app's stack
    // may not have taken in yet), and then finds the connection reset: it is
    // no end of the stream.
    let mut taken = vec![0u8; 16 * 1024];
    let error = within("the app to find the reset", async {
        loop {
            match tcp.read(&mut taken).await {
                Ok(0) => panic!("the end of the stream, not a reset"),
                Ok(_) => {}
                Err(error) => break error,
            }
        }
    })
    .await;
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
}

/// What is no query closes the connection, without a word and without the
/// responder having anything to forward or an address to give: a message
/// that is a response (QR set), one of fewer bytes than a header, and one with a
/// length of 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn garbage_on_a_dns_connection_closes_it() {
    let (lab, extras) = lab_and_extras(TunOptions::default(), Extras::default());
    let mut response = query(1, "a.test", A);
    response[2] |= 0x80;
    let garbage = [
        ("a response", framed(&response)),
        ("a runt", framed(&[0x16, 0xfe, 0xfd, 0, 0])),
        ("a length of 0", vec![0, 0]),
    ];
    for (what, bytes) in garbage {
        let mut tcp = connect(&lab, RESPONDER4).await;
        tcp.write_all(&bytes).await.expect("write");
        assert_closed(&mut tcp).await;
        assert_let_go(&lab, &mut tcp).await;
        assert!(extras.dns.forwarded().is_empty(), "{what}: forwarded");
        assert_eq!(
            lab.pool().address_of("a.test", false),
            None,
            "{what}: an address was given"
        );
    }
    assert!(extras.hooks.pre_matches_asked().is_empty());
    assert_eq!(lab.rig.tun.counters(), Counters::default());
}

/// The answer to a query comes before the end of the stream that a message
/// that gets no answer brings about, though the two arrive together: the close
/// does not take the reply with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_reply_before_a_bad_message_is_still_delivered() {
    let (lab, _) = lab_and_extras(TunOptions::default(), Extras::default());
    let mut response = query(2, "b.test", A);
    response[2] |= 0x80;
    let mut bytes = framed(&query(1, "a.test", A));
    bytes.extend(framed(&response));
    for _ in 0..10 {
        let mut tcp = connect(&lab, RESPONDER4).await;
        tcp.write_all(&bytes).await.expect("write");
        let reply = next_message(&mut tcp).await;
        assert_fake_reply(&reply, 1, lab.fake("a.test", false));
        assert_closed(&mut tcp).await;
        assert_let_go(&lab, &mut tcp).await;
    }
}

/// A query that stops halfway, the length said and some of the message sent,
/// is no query yet and no reason to wait for ever: the connection is closed
/// after the idle time. A query that the app's close cuts short is no query
/// either, and the connection is closed at once with nothing answered, though
/// what came of it is as long as a header, which would be answered FORMERR were
/// it taken for a whole message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_query_that_stops_halfway_ends_the_connection() {
    let (lab, extras) = lab_and_extras(
        TunOptions {
            dns_tcp_idle: Duration::from_millis(300),
            ..TunOptions::default()
        },
        Extras::default(),
    );
    let whole = query(1, "a.test", A);
    let mut half = framed(&whole);
    half.truncate(2 + whole.len() / 2);
    // it stops, and the app waits
    let mut tcp = connect(&lab, RESPONDER4).await;
    tcp.write_all(&half).await.expect("write");
    assert_closed(&mut tcp).await;
    assert_let_go(&lab, &mut tcp).await;
    // it stops, and the app closes its side
    let mut tcp = connect(&lab, RESPONDER4).await;
    tcp.write_all(&half).await.expect("write");
    tcp.shutdown().await.expect("shutdown");
    assert_closed(&mut tcp).await;
    assert_let_go(&lab, &mut tcp).await;
    assert!(extras.hooks.pre_matches_asked().is_empty());
}

/// An app that closes its side after its queries is answered first and then
/// has its connection closed, and let go of.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_that_closes_its_side_ends_the_connection() {
    let (lab, _) = lab_and_extras(TunOptions::default(), Extras::default());
    let mut tcp = connect(&lab, RESPONDER4).await;
    tcp.write_all(&framed(&query(1, "a.test", A)))
        .await
        .expect("write");
    assert_fake_reply(&next_message(&mut tcp).await, 1, lab.fake("a.test", false));
    tcp.shutdown().await.expect("shutdown");
    assert_closed(&mut tcp).await;
    eventually("the TUN to let the connection go", WITHIN, || {
        lab.rig.tun.sockets_in_use() == 0 && lab.rig.tun.session_tasks() == 0
    })
    .await;
    assert_eq!(lab.rig.tun.counters(), Counters::default());
}

/// An app that reads the TUN's FIN and never closes its own side is not waited
/// for for ever: the TUN gives it `dns_tcp_idle` after the FIN, and then lets
/// the stream go, which resets a connection whose app has not closed. The app
/// sends a length of 0, which gets no answer and has the TUN close at once,
/// reads to the end of the stream, and then does nothing: it does not shut its
/// side down and does not reset. What is looked at is the TUN's side, not what
/// the app's stack reads next (it may be the end of the stream once more). The
/// TUN lets go about 300 ms after its FIN; the wait is `WITHIN`, 10 s, a margin
/// of more than thirty times, so a connection that is kept shows as the wait
/// running out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_that_never_closes_is_let_go_after_the_idle_time() {
    let (lab, _) = lab_and_extras(
        TunOptions {
            dns_tcp_idle: Duration::from_millis(300),
            ..TunOptions::default()
        },
        Extras::default(),
    );
    let mut tcp = connect(&lab, RESPONDER4).await;
    tcp.write_all(&[0, 0]).await.expect("write");
    assert_closed(&mut tcp).await;
    // `tcp` is there to the end of the test, its side open
    eventually("the TUN to let the connection go", WITHIN, || {
        lab.rig.tun.sockets_in_use() == 0 && lab.rig.tun.session_tasks() == 0
    })
    .await;
    assert_eq!(lab.rig.tun.sockets_counted(), 0);
    assert_eq!(lab.rig.tun.half_open(), 0);
    assert_eq!(lab.rig.tun.budget_in_use(), 0);
}

/// Only port 53 of the responder's addresses has a connection of its own: any
/// other port is refused, as any other address of rurge's is, and nothing is
/// asked of the rules or counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_responder_on_another_tcp_port_is_reset() {
    let (lab, extras) = lab_and_extras(TunOptions::default(), Extras::default());
    for to in ["198.18.0.2:54", "[fd00:6152::2]:54", "198.18.0.9:853"] {
        let error = within(
            "the connection to be refused",
            lab.rig.app.connect(addr(to)),
        )
        .await
        .expect_err("a port that is no place for a connection");
        assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused, "{to}");
    }
    assert!(lab.rig.dialer.dialed().is_empty());
    assert!(extras.hooks.pre_matches_asked().is_empty());
    assert_eq!(lab.rig.tun.counters(), Counters::default());
    assert_eq!(lab.rig.tun.sockets_in_use(), 0);
}

/// `hijack-dns` is for UDP: a TCP connection to a server it names, at port 53,
/// is an ordinary connection, dialed as the session it is (the fake dialer has
/// no script for the server, so the dial fails and the app's connection is
/// reset: nothing leaves the process), and the responder has nothing to do with
/// it. The app sends nothing: the reset can be on its way to it before it
/// would.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_to_a_hijacked_server_is_a_connection() {
    let (lab, extras) = lab_and_extras(TunOptions::default(), extras_hijacking(&[(None, 53)]));
    let server = addr("192.0.2.53:53");
    let mut tcp = connect(&lab, "192.0.2.53:53").await;
    let mut byte = [0u8; 1];
    let error = within("the connection to be reset", tcp.read(&mut byte))
        .await
        .expect_err("the failed dial resets the connection");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    let dialed = lab.rig.dialer.dialed();
    assert_eq!(dialed.len(), 1, "one session: {dialed:?}");
    assert_eq!(dialed[0].dst_host, HostName::Ip(server.ip()));
    assert_eq!(dialed[0].dst_port, 53);
    assert_eq!(dialed[0].local, Some(server));
    assert!(extras.dns.forwarded().is_empty());
    // the rules were asked about it as about any connection, and about no name
    let asked = extras.hooks.pre_matches_asked();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].0, PreMatchLayer::Syn);
    assert_eq!(asked[0].1, dialed[0]);
}

/// One responder answers the TUN's queries whatever they come over: the warning
/// that every fake address is in use is said once for the TUN (a responder of
/// each transport would say it once each), though a query over UDP and one over
/// TCP both find the pool used up. (The same one holds the limit on the queries
/// being forwarded.)
#[tokio::test]
async fn one_responder_answers_over_udp_and_tcp() {
    let log = LogCapture::start();
    let pool = Arc::new(FakeIpPool::with_count(1));
    let (mut lab, _) = lab_and_extras(
        TunOptions::default(),
        Extras {
            pool: Some(pool.clone()),
            ..Extras::default()
        },
    );
    // the one address is the connection's, for as long as it stays open
    let index = pool
        .assign("a.test", Instant::now())
        .expect("an address for the name");
    let echo = Upstream::echo().await;
    let fake = sock(v4_of(index), 80);
    lab.rig.dialer.script(fake, Script::Upstream(echo.addr()));
    let mut held = within("the connection to be made", lab.rig.app.connect(fake))
        .await
        .expect("connect");
    hello(&mut held).await;
    // a name with no address to be had, asked over each transport
    lab.send(addr(APP4), addr(RESPONDER4), &query(1, "b.test", A));
    let seen = lab.datagram().await;
    assert_eq!(rcode_of(&seen.payload), Rcode::ServFail, "over UDP");
    let mut tcp = connect(&lab, RESPONDER4).await;
    tcp.write_all(&framed(&query(2, "c.test", A)))
        .await
        .expect("write");
    let reply = next_message(&mut tcp).await;
    assert_eq!(rcode_of(&reply), Rcode::ServFail, "over TCP");
    let warnings: Vec<String> = log
        .lines()
        .into_iter()
        .filter(|line| line.starts_with("WARN "))
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

/// The task of a connection ends at the stop wherever it is, a forward that the
/// upstream holds included: the TUN ends, though the upstream never answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dns_connection_held_by_the_upstream_ends_at_the_stop() {
    let (lab, extras) = lab_and_extras(TunOptions::default(), Extras::default());
    extras.dns.script("held.test", ForwardScript::Hold);
    let mut tcp = connect(&lab, RESPONDER4).await;
    tcp.write_all(&framed(&query(1, "held.test", TXT)))
        .await
        .expect("write");
    eventually("the query to be forwarded", WITHIN, || {
        extras.dns.forwarded().len() == 1
    })
    .await;
    assert_eq!(lab.rig.tun.session_tasks(), 1, "the task of the connection");
    lab.rig.tun.stop();
    let ending = in_five_seconds("the TUN to end", lab.rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
}
