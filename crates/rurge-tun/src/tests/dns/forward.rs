//! What becomes of a reply on its way out, and how many queries are forwarded
//! at once (phase 3 M3 design 11.2): the size of a datagram and of a frame, the
//! limit of 256 forwards in flight, the OPT record of an answer made here, how
//! often the settings are taken, and what is said of a query in the log.

use super::{
    A, AAAA, Lab, SVCB, TXT, assert_empty, authoritative, echoed, lab, parsed, query, query_opt,
    tcp4, truncated, udp4, udp6,
};
use crate::dns::{MAX_FORWARDS, Responder};
use crate::testing::{FakeHooks, ForwardScript};
use crate::tests::log_capture::LogCapture;
use crate::tests::support::{Extras, WITHIN, eventually, within};
use crate::{Hooks, TunSettings};
use rurge_config::{HostName, PreMatchLayer, SessionInfo};
use rurge_dns::message::{Rcode, opt_size};
use rurge_proto::RejectKind;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---- the size of a reply

/// A reply longer than the datagram the client can take is TC without its
/// records, with the response code the upstream gave, and the client takes the
/// whole of it over TCP.
#[tokio::test]
async fn a_large_udp_answer_is_truncated() {
    let lab = lab();
    lab.extras
        .dns
        .script("big.test", ForwardScript::Echo { pad: 2000 });
    // the size the query announces: none (a limit of 512), 4096 (1232), 300 (512)
    for size in [None, Some(4096), Some(300)] {
        let q = match size {
            Some(size) => query_opt(5, "big.test", TXT, size),
            None => query(5, "big.test", TXT),
        };
        let reply = lab.reply(&q, &udp4()).await;
        assert!(truncated(&reply), "{size:?}");
        assert!(!authoritative(&reply), "{size:?}");
        assert_empty(&reply, &q, Rcode::NoError);
        // the header, the question and the OPT the query had
        assert_eq!(reply.len(), q.len(), "{size:?}");
        // the same over TCP, whole
        assert_eq!(lab.reply(&q, &tcp4()).await, echoed(&q, 2000), "{size:?}");
    }
    // the response code is the upstream's
    let mut nx = query(6, "big.test", TXT);
    nx[3] |= 3;
    let reply = lab.reply(&nx, &udp4()).await;
    assert!(truncated(&reply));
    assert_empty(&reply, &nx, Rcode::NxDomain);
}

/// The limit of a datagram is the size the query announces, no less than 512
/// and no more than 1232: a reply as long as the limit is whole, one byte more
/// is TC.
#[tokio::test]
async fn the_udp_limit_is_the_announced_size_between_512_and_1232() {
    let lab = lab();
    let cases = [
        (None, 512),
        (Some(0), 512),
        (Some(300), 512),
        (Some(512), 512),
        (Some(900), 900),
        (Some(1232), 1232),
        (Some(4096), 1232),
        (Some(u16::MAX), 1232),
    ];
    for (size, limit) in cases {
        let q = match size {
            Some(size) => query_opt(5, "edge.test", TXT, size),
            None => query(5, "edge.test", TXT),
        };
        let pad = limit - q.len();
        lab.extras
            .dns
            .script("edge.test", ForwardScript::Echo { pad });
        let reply = lab.reply(&q, &udp4()).await;
        assert!(!truncated(&reply), "{size:?}: {limit} bytes are whole");
        assert_eq!(reply.len(), limit, "{size:?}");
        lab.extras
            .dns
            .script("edge.test", ForwardScript::Echo { pad: pad + 1 });
        let reply = lab.reply(&q, &udp4()).await;
        assert!(truncated(&reply), "{size:?}: {} bytes are not", limit + 1);
    }
}

/// A two-byte length frames a message over TCP: a reply of more than 65,535
/// bytes cannot be sent there and is SERVFAIL; over UDP the limit truncates it
/// as it does any other.
#[tokio::test]
async fn a_reply_too_long_for_tcp_gets_servfail() {
    let lab = lab();
    let q = query(3, "huge.test", TXT);
    let pad_to = |len: usize| ForwardScript::Echo { pad: len - q.len() };
    lab.extras.dns.script("huge.test", pad_to(65_535));
    assert_eq!(lab.reply(&q, &tcp4()).await.len(), 65_535);
    lab.extras.dns.script("huge.test", pad_to(65_536));
    assert_empty(&lab.reply(&q, &tcp4()).await, &q, Rcode::ServFail);
    lab.extras
        .dns
        .script("huge.test", ForwardScript::Echo { pad: 70_000 });
    assert_empty(&lab.reply(&q, &tcp4()).await, &q, Rcode::ServFail);
    lab.extras
        .dns
        .script("huge.test", ForwardScript::Echo { pad: 2_000 });
    assert_eq!(lab.reply(&q, &tcp4()).await, echoed(&q, 2_000));
    // over UDP it is TC, not SERVFAIL
    lab.extras
        .dns
        .script("huge.test", ForwardScript::Echo { pad: 70_000 });
    let reply = lab.reply(&q, &udp4()).await;
    assert!(truncated(&reply));
    assert_empty(&reply, &q, Rcode::NoError);
}

/// What the responder makes itself is bound by the limit as well: forty
/// addresses of `always-real-ip` do not fit 512 bytes, so over UDP they are TC
/// and without records, and over TCP, or to a client that announces more, they
/// are all there.
#[tokio::test]
async fn a_large_always_real_ip_answer_is_truncated_over_udp() {
    let lab = lab();
    lab.configure("many.test", false);
    let addrs: Vec<IpAddr> = (1..=40)
        .map(|n| IpAddr::V4(Ipv4Addr::new(192, 0, 2, n)))
        .collect();
    lab.extras.dns.map("many.test", &addrs);
    let a = query(1, "many.test", A);
    let reply = lab.reply(&a, &udp4()).await;
    assert!(truncated(&reply));
    assert_empty(&reply, &a, Rcode::NoError);
    // 4096 announced: the limit is 1232, and they fit
    let big = query_opt(2, "many.test", A, 4096);
    let reply = lab.reply(&big, &udp4()).await;
    assert!(!truncated(&reply));
    assert_eq!(parsed(&reply).v4.len(), 40);
    // over TCP there is no limit but the frame
    let reply = lab.reply(&a, &tcp4()).await;
    assert!(!truncated(&reply));
    let answer = parsed(&reply);
    assert_eq!(answer.rcode, Rcode::NoError);
    assert_eq!(answer.v4.len(), 40);
    assert!(answer.v4.iter().all(|&(_, ttl)| ttl == 30));
    assert!(lab.extras.dns.forwarded().is_empty());
}

// ---- the limit on what is forwarded

/// Starts `count` queries for `name` of type TXT, each in a task of its own,
/// over UDP; the tasks give the reply they got.
fn start_forwards(
    lab: &Lab,
    name: &str,
    count: usize,
) -> Vec<tokio::task::JoinHandle<Option<Vec<u8>>>> {
    (0..count)
        .map(|n| {
            let responder = lab.responder.clone();
            let q = query(n as u16, name, TXT);
            tokio::spawn(async move { responder.answer(&q, &udp4()).await })
        })
        .collect()
}

/// At most 256 queries are forwarded at once; the next gets SERVFAIL at once
/// and the resolver is not asked, and once the held ones are answered the
/// places are free again.
#[tokio::test]
async fn forwards_in_flight_are_capped_at_256() {
    let lab = lab();
    lab.extras.dns.script("held.test", ForwardScript::Hold);
    let held = start_forwards(&lab, "held.test", MAX_FORWARDS);
    eventually("256 queries to reach the resolver", WITHIN, || {
        lab.extras.dns.forwarded().len() == MAX_FORWARDS
    })
    .await;
    let late = query(999, "held.test", TXT);
    assert_empty(&lab.reply(&late, &udp4()).await, &late, Rcode::ServFail);
    assert_eq!(
        lab.extras.dns.forwarded().len(),
        MAX_FORWARDS,
        "the resolver was not asked"
    );
    assert!(held.iter().all(|task| !task.is_finished()), "all are held");
    lab.extras.dns.release();
    for (n, task) in held.into_iter().enumerate() {
        let reply = within("a held query to be answered", task)
            .await
            .expect("the task ends")
            .expect("an answer");
        assert_eq!(reply, echoed(&query(n as u16, "held.test", TXT), 0));
    }
    // the places are back
    let after = query(1000, "held.test", TXT);
    assert_eq!(lab.reply(&after, &udp4()).await, echoed(&after, 0));
}

/// An answer that needs no resolver does not wait for one: with 256 queries
/// held a name still gets its fake address at once.
#[tokio::test]
async fn a_fake_answer_does_not_wait_for_forwards() {
    let lab = lab();
    lab.extras.dns.script("held.test", ForwardScript::Hold);
    let held = start_forwards(&lab, "held.test", MAX_FORWARDS);
    eventually("256 queries to reach the resolver", WITHIN, || {
        lab.extras.dns.forwarded().len() == MAX_FORWARDS
    })
    .await;
    let answer = parsed(&lab.reply(&query(1, "fast.test", A), &udp4()).await);
    assert_eq!(answer.v4.len(), 1);
    lab.extras.dns.release();
    for task in held {
        within("a held query to be answered", task)
            .await
            .expect("the task ends");
    }
}

/// A forward that is dropped midway, as the task of a query is when the TUN
/// stops, gives its place back: once 256 held forwards are aborted a query is
/// forwarded again and not answered SERVFAIL.
#[tokio::test]
async fn a_forward_dropped_midway_gives_its_place_back() {
    let lab = lab();
    lab.extras.dns.script("held.test", ForwardScript::Hold);
    lab.extras
        .dns
        .script("alive.test", ForwardScript::Echo { pad: 0 });
    let held = start_forwards(&lab, "held.test", MAX_FORWARDS);
    eventually("256 queries to reach the resolver", WITHIN, || {
        lab.extras.dns.forwarded().len() == MAX_FORWARDS
    })
    .await;
    // every place is taken: a query for a name that answers is refused
    let refused = query(998, "alive.test", TXT);
    assert_empty(
        &lab.reply(&refused, &udp4()).await,
        &refused,
        Rcode::ServFail,
    );
    for task in &held {
        task.abort();
    }
    for task in held {
        let ended = within("an aborted query to end", task).await;
        assert!(ended.expect_err("the query was aborted").is_cancelled());
    }
    let alive = query(999, "alive.test", TXT);
    assert_eq!(lab.reply(&alive, &udp4()).await, echoed(&alive, 0));
}

// ---- the answers made here

/// Every kind of answer the responder makes itself has an OPT record that
/// announces 1232 when the query had one, and has none when it had not.
#[tokio::test]
async fn an_answer_to_a_query_with_an_opt_has_one_of_1232() {
    let lab = lab();
    lab.configure("mapped.test", false);
    lab.extras
        .dns
        .map("mapped.test", &[IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7))]);
    for (name, kind) in [
        ("rejected.test", RejectKind::Reject),
        ("refused.test", RejectKind::NoDrop),
    ] {
        lab.extras.hooks.script_pre_match(
            PreMatchLayer::Dns,
            HostName::Domain(name.to_owned()),
            kind,
        );
    }
    // a fake address, the canary, NOTIMP for SVCB, a real address, both refusals
    // and the empty answer of a family that does not fit
    let cases = [
        ("fake.test", A, udp4()),
        ("use-application-dns.net", A, udp4()),
        ("svc.test", SVCB, udp4()),
        ("mapped.test", A, udp4()),
        ("rejected.test", A, udp4()),
        ("refused.test", A, udp4()),
        ("fake.test", AAAA, udp4()),
    ];
    for (name, qtype, ask) in cases {
        let with = lab.reply(&query_opt(1, name, qtype, 4096), &ask).await;
        assert_eq!(opt_size(&with), Some(1232), "{name} type {qtype}");
        let without = lab.reply(&query(1, name, qtype), &ask).await;
        assert_eq!(opt_size(&without), None, "{name} type {qtype}");
    }
    // an answer that is no records at all: NOTIMP for a class other than IN
    let mut chaos = query_opt(2, "fake.test", A, 4096);
    let end = chaos.len() - 11;
    chaos[end - 2..end].copy_from_slice(&3u16.to_be_bytes());
    assert_eq!(opt_size(&lab.reply(&chaos, &udp4()).await), Some(1232));
}

// ---- once per query

/// Hooks that count how often the settings are asked for.
struct Counting {
    inner: Arc<FakeHooks>,
    settings_asked: AtomicUsize,
}

impl Hooks for Counting {
    fn settings(&self) -> Arc<TunSettings> {
        self.settings_asked.fetch_add(1, Ordering::SeqCst);
        self.inner.settings()
    }

    fn pre_match(&self, layer: PreMatchLayer, session: &SessionInfo) -> Option<RejectKind> {
        self.inner.pre_match(layer, session)
    }
}

/// The settings are taken once for a query, however many of them the answer
/// looks at: a reload cannot change them in the middle of one.
#[tokio::test]
async fn the_settings_are_taken_once_for_a_query() {
    let extras = Extras::default();
    extras.hooks.script_pre_match(
        PreMatchLayer::Dns,
        HostName::Domain("rejected.test".to_owned()),
        RejectKind::Reject,
    );
    let counting = Arc::new(Counting {
        inner: extras.hooks.clone(),
        settings_asked: AtomicUsize::new(0),
    });
    let responder = Responder::new(counting.clone(), extras.pool.clone());
    let mut chaos = query(9, "fake.test", A);
    let end = chaos.len();
    chaos[end - 2..].copy_from_slice(&3u16.to_be_bytes());
    let queries = [
        query(1, "fake.test", A),
        query(2, "fake.test", AAAA),
        query(3, "rejected.test", A),
        query(4, "txt.test", TXT),
        query(5, "svc.test", SVCB),
        query(6, "use-application-dns.net", A),
        chaos,
    ];
    for q in &queries {
        let before = counting.settings_asked.load(Ordering::SeqCst);
        for ask in [udp4(), udp6()] {
            let _ = within("an answer", responder.answer(q, &ask)).await;
        }
        let asked = counting.settings_asked.load(Ordering::SeqCst) - before;
        assert_eq!(asked, 2, "once for each of the two queries: {q:?}");
    }
}

// ---- what is said

/// A query is said in one line at debug level, with its name, its type and
/// what became of it, whether it was answered or not, and the line says when
/// the reply had to be cut to go back on its transport; nothing above debug
/// level says a name.
#[tokio::test]
async fn every_query_is_said_in_one_debug_line_and_no_name_above_debug() {
    let log = LogCapture::start();
    let lab = lab();
    lab.extras.hooks.script_pre_match(
        PreMatchLayer::Dns,
        HostName::Domain("dropped.test".to_owned()),
        RejectKind::Drop,
    );
    lab.extras.dns.script("fail.test", ForwardScript::Fail);
    // a reply too long for a datagram, and one too long for a frame
    lab.extras
        .dns
        .script("big.test", ForwardScript::Echo { pad: 2000 });
    lab.extras
        .dns
        .script("huge.test", ForwardScript::Echo { pad: 70_000 });
    // (the name, the type as the line writes it, the query, who asks, and what
    // the line says of the reply being cut, if it does)
    let queries = [
        ("fake.test", "A", query(1, "fake.test", A), udp4(), None),
        (
            "forward.test",
            "TYPE16",
            query(2, "forward.test", TXT),
            udp4(),
            None,
        ),
        (
            "dropped.test",
            "A",
            query(3, "dropped.test", A),
            udp4(),
            None,
        ),
        (
            "fail.test",
            "TYPE16",
            query(4, "fail.test", TXT),
            udp4(),
            None,
        ),
        (
            "use-application-dns.net",
            "AAAA",
            query(5, "use-application-dns.net", AAAA),
            udp4(),
            None,
        ),
        (
            "big.test",
            "TYPE16",
            query(6, "big.test", TXT),
            udp4(),
            Some("fitted=\"truncated"),
        ),
        (
            "huge.test",
            "TYPE16",
            query(7, "huge.test", TXT),
            tcp4(),
            Some("fitted=\"SERVFAIL"),
        ),
    ];
    for (_, _, q, ask, _) in &queries {
        let _ = lab.ask(q, ask).await;
    }
    // and one that is no query, which has no name to say
    let mut reply = query(8, "reply.test", A);
    reply[2] |= 0x80;
    assert_eq!(lab.ask(&reply, &udp4()).await, None);
    let lines = log.lines();
    assert!(
        lines.iter().all(|line| line.starts_with("DEBUG")),
        "nothing above debug level: {lines:?}"
    );
    assert_eq!(
        lines.len(),
        queries.len() + 1,
        "one line for each: {lines:?}"
    );
    let mut decisions = Vec::new();
    for (name, qtype, _, _, cut) in &queries {
        let said: Vec<&String> = lines.iter().filter(|line| line.contains(name)).collect();
        assert_eq!(said.len(), 1, "{name}: {lines:?}");
        let line = said[0];
        assert!(line.contains(&format!("qtype={qtype} ")), "{line}");
        match cut {
            Some(cut) => assert!(line.contains(cut), "said to be cut: {line}"),
            None => assert!(!line.contains("fitted"), "not cut: {line}"),
        }
        let decision = line
            .split("decision=")
            .nth(1)
            .unwrap_or_else(|| panic!("what was decided: {line}"));
        decisions.push(decision.to_owned());
    }
    // different things became of them: what was decided and what was done to
    // the reply tell them apart
    decisions.sort();
    decisions.dedup();
    assert_eq!(decisions.len(), queries.len(), "{decisions:?}");
}
