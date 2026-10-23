//! What the responder answers, query by query, in the order it decides (phase 3
//! M3 design 11.2): a message that is no query, a class or a type it does not
//! do, the canary, a refusal by a pre-matching rule, the family of the
//! transport, `always-real-ip`, a fake address, and everything else, which goes
//! to the resolver.

use super::{
    A, AAAA, HTTPS, Lab, SVCB, TXT, assert_empty, authoritative, counts, echoed, id_of,
    is_response, lab, lab_with, parsed, query, query_opt, question_of, rcode_of,
    recursion_available, tcp4, tcp6, truncated, udp4, udp6,
};
use crate::dns::{Ask, MAX_FORWARDS, Responder};
use crate::space::REFUSED_V4;
use crate::stats::Stats;
use crate::testing::{FakeHooks, ForwardScript};
use crate::tests::log_capture::LogCapture;
use crate::tests::support::{Extras, WITHIN, eventually, within};
use crate::{Hooks, TunSettings};
use rurge_config::{HostName, ListenerKind, PreMatchLayer, SessionInfo};
use rurge_dns::fakeip::{FakeIpPool, index_of, v4_of, v6_of};
use rurge_dns::message::{Rcode, opt_size};
use rurge_proto::RejectKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

// ---- a message that is no query to answer

/// A reply is not answered, and neither is anything shorter than a header.
#[tokio::test]
async fn a_reply_or_a_runt_gets_no_answer() {
    let lab = lab();
    let q = query(1, "a.test", A);
    let mut reply = q.clone();
    reply[2] |= 0x80;
    assert_eq!(lab.ask(&reply, &udp4()).await, None, "a reply");
    for len in [0, 1, 11] {
        assert_eq!(lab.ask(&q[..len], &udp4()).await, None, "{len} bytes");
    }
    lab.assert_untouched();
}

/// A message that cannot be read to its question is FORMERR, the header and
/// nothing else, with the ID and RD it came with.
#[tokio::test]
async fn a_message_that_does_not_parse_gets_formerr() {
    let lab = lab();
    let formerr = |reply: &[u8], id: u16| {
        assert_eq!(reply.len(), 12, "the header only: {reply:?}");
        assert_eq!(id_of(reply), id);
        assert!(
            is_response(reply) && recursion_available(reply),
            "{reply:?}"
        );
        assert_eq!(rcode_of(reply), Rcode::FormErr);
        assert_eq!(counts(reply), [0; 4]);
        assert_eq!(reply[2] & 0x01, 1, "RD as the query had it");
    };
    // the header says there is a question, and there is none
    let header = [0xab, 0xcd, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    formerr(&lab.reply(&header, &udp4()).await, 0xabcd);
    // it says there is none
    let mut none = query(7, "a.test", A);
    none[4..6].copy_from_slice(&0u16.to_be_bytes());
    formerr(&lab.reply(&none, &udp4()).await, 7);
    // a name that stops in the middle of a label
    let mut cut = query(8, "abcdef.test", A);
    cut.truncate(12 + 4);
    formerr(&lab.reply(&cut, &udp4()).await, 8);
    // a question without its class
    let mut short = query(9, "a.test", A);
    short.truncate(short.len() - 3);
    formerr(&lab.reply(&short, &tcp4()).await, 9);
    lab.assert_untouched();
}

#[tokio::test]
async fn two_questions_get_formerr() {
    let lab = lab();
    let mut two = query(9, "a.test", A);
    two[4..6].copy_from_slice(&2u16.to_be_bytes());
    two.extend_from_slice(&query(0, "b.test", A)[12..]);
    let reply = lab.reply(&two, &udp4()).await;
    assert_eq!(reply.len(), 12, "the header only: {reply:?}");
    assert_eq!(id_of(&reply), 9);
    assert_eq!(rcode_of(&reply), Rcode::FormErr);
    lab.assert_untouched();
}

/// Only the header is read to tell an opcode other than QUERY: NOTIMP, the
/// header only, with the opcode the query had.
#[tokio::test]
async fn another_opcode_gets_notimp() {
    let lab = lab();
    // IQUERY, STATUS, NOTIFY, UPDATE
    for opcode in [1u8, 2, 4, 5] {
        let mut q = query(u16::from(opcode), "a.test", A);
        q[2] = (q[2] & 0x87) | (opcode << 3);
        let reply = lab.reply(&q, &udp4()).await;
        assert_eq!(reply.len(), 12, "the header only: {reply:?}");
        assert_eq!(id_of(&reply), u16::from(opcode));
        assert_eq!(rcode_of(&reply), Rcode::NotImp, "opcode {opcode}");
        assert_eq!((reply[2] >> 3) & 0x0f, opcode, "the query's opcode");
        assert!(is_response(&reply) && recursion_available(&reply));
        assert_eq!(counts(&reply), [0; 4]);
    }
    // what follows the header is not looked at
    let header = [0, 3, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let reply = lab.reply(&header, &udp4()).await;
    assert_eq!(rcode_of(&reply), Rcode::NotImp);
    lab.assert_untouched();
}

/// A class other than IN is NOTIMP, and the answer has the question.
#[tokio::test]
async fn another_class_gets_notimp_with_the_question() {
    let lab = lab();
    let mut q = query(11, "Class.test", A);
    let end = q.len();
    // CH
    q[end - 2..].copy_from_slice(&3u16.to_be_bytes());
    let reply = lab.reply(&q, &udp4()).await;
    assert_empty(&reply, &q, Rcode::NotImp);
    lab.assert_untouched();
}

// ---- the names and types that are answered before anything else

#[tokio::test]
async fn the_canary_is_nxdomain_for_every_type() {
    let lab = lab();
    for (id, qtype) in [A, AAAA, TXT].into_iter().enumerate() {
        for ask in [udp4(), tcp6()] {
            let q = query(id as u16, "USE-Application-DNS.net", qtype);
            assert_empty(&lab.reply(&q, &ask).await, &q, Rcode::NxDomain);
        }
    }
    lab.assert_untouched();
}

/// SVCB and HTTPS are NOTIMP while `allow-dns-svcb` is off, and forwarded as
/// they came when it is on; the setting is the one in use when the query comes.
#[tokio::test]
async fn svcb_and_https_are_notimp_unless_allowed() {
    let lab = lab();
    let queries = [query(1, "svc.test", SVCB), query(2, "svc.test", HTTPS)];
    for q in &queries {
        assert_empty(&lab.reply(q, &udp4()).await, q, Rcode::NotImp);
    }
    lab.assert_untouched();
    lab.configure("", true);
    for q in &queries {
        assert_eq!(lab.reply(q, &udp4()).await, echoed(q, 0));
    }
    assert_eq!(lab.extras.dns.forwarded(), queries);
    // they are not questions for the rules
    assert!(lab.extras.hooks.pre_matches_asked().is_empty());
    lab.configure("", false);
    assert_empty(
        &lab.reply(&queries[0], &udp4()).await,
        &queries[0],
        Rcode::NotImp,
    );
}

// ---- A and AAAA

/// A name gets the fake address of its index: the IPv4 one for an A query, the
/// IPv6 one of the same index for AAAA, with a TTL of 30 seconds, authoritative,
/// and asking again gets the same.
#[tokio::test]
async fn a_and_aaaa_get_fake_addresses() {
    let lab = lab();
    let a = query(1, "a.test", A);
    let reply = lab.reply(&a, &udp4()).await;
    assert!(is_response(&reply) && authoritative(&reply) && recursion_available(&reply));
    assert_eq!(question_of(&reply), question_of(&a));
    let answer = parsed(&reply);
    assert_eq!((answer.id, answer.rcode), (1, Rcode::NoError));
    assert!(answer.v6.is_empty());
    let [(v4, ttl)] = answer.v4[..] else {
        panic!("one A record: {answer:?}")
    };
    assert_eq!(ttl, 30);
    // it is the address the pool has for the name
    assert_eq!(lab.fake_of("a.test", false), Some(IpAddr::V4(v4)));
    // asked again, over TCP too, it is the same
    for ask in [udp4(), tcp4()] {
        let again = parsed(&lab.reply(&query(2, "a.test", A), &ask).await);
        assert_eq!(again.v4, answer.v4);
    }
    // AAAA is the IPv6 address of the same index
    let reply = lab.reply(&query(3, "a.test", AAAA), &udp6()).await;
    assert!(authoritative(&reply));
    let answer6 = parsed(&reply);
    assert_eq!((answer6.id, answer6.rcode), (3, Rcode::NoError));
    assert!(answer6.v4.is_empty());
    let [(v6, ttl)] = answer6.v6[..] else {
        panic!("one AAAA record: {answer6:?}")
    };
    assert_eq!(ttl, 30);
    let index = index_of(IpAddr::V4(v4)).expect("an address of the pool");
    assert_eq!(index_of(IpAddr::V6(v6)), Some(index));
    assert_eq!(v6, v6_of(index));
    // another name has another address
    let b = parsed(&lab.reply(&query(4, "b.test", A), &udp4()).await);
    assert_ne!(b.v4[0].0, v4);
    assert!(lab.extras.dns.forwarded().is_empty());
}

/// A resolver that randomises the case of the names it asks (0x20) gets its
/// own case back in the question, and the name has one address whatever case
/// it is written in.
#[tokio::test]
async fn a_mixed_case_query_gets_its_own_case_back() {
    let lab = lab();
    let mixed = query(21, "ExAmPle.TEST", A);
    let plain = query(22, "example.test", A);
    let from_mixed = lab.reply(&mixed, &udp4()).await;
    let from_plain = lab.reply(&plain, &udp4()).await;
    assert_eq!(parsed(&from_mixed).v4.len(), 1);
    assert_eq!(parsed(&from_mixed).v4, parsed(&from_plain).v4);
    // each answer repeats the question of its own query to the byte
    assert_eq!(question_of(&from_mixed), question_of(&mixed));
    assert_eq!(question_of(&from_plain), question_of(&plain));
    assert_ne!(question_of(&from_mixed), question_of(&from_plain));
    // the pool has the name in lower case only
    assert!(lab.fake_of("example.test", false).is_some());
    // AAAA the same
    let mixed6 = query(23, "eXaMpLe.test", AAAA);
    let reply6 = lab.reply(&mixed6, &udp6()).await;
    assert_eq!(question_of(&reply6), question_of(&mixed6));
    assert_eq!(parsed(&reply6).v6.len(), 1);
}

/// An A query that comes over IPv6 and an AAAA query that comes over IPv4 get
/// an empty answer, and the pool gives the name nothing.
#[tokio::test]
async fn the_family_must_match_the_transport() {
    let lab = lab();
    for ask in [udp6(), tcp6()] {
        let a = query(1, "fam.test", A);
        assert_empty(&lab.reply(&a, &ask).await, &a, Rcode::NoError);
    }
    for ask in [udp4(), tcp4()] {
        let aaaa = query(2, "fam.test", AAAA);
        assert_empty(&lab.reply(&aaaa, &ask).await, &aaaa, Rcode::NoError);
    }
    assert_eq!(lab.fake_of("fam.test", false), None);
    lab.assert_no_address_given();
    assert!(lab.extras.dns.forwarded().is_empty());
    // the pairs that fit get addresses
    let a = parsed(&lab.reply(&query(3, "fam.test", A), &udp4()).await);
    assert_eq!(a.v4.len(), 1);
    let aaaa = parsed(&lab.reply(&query(4, "fam.test", AAAA), &udp6()).await);
    assert_eq!(aaaa.v6.len(), 1);
}

// ---- pre-matching

/// The session a query is at the DNS layer: the app's endpoint, no port, the
/// adapter as the listener, the transport of the query, and the endpoint the
/// query was sent to as rurge's own.
fn dns_session(ask: &Ask, name: &str) -> SessionInfo {
    SessionInfo {
        src: ask.src,
        in_port: 0,
        local: Some(ask.server),
        listener: ListenerKind::Tun,
        transport: ask.transport,
        ..SessionInfo::tcp(HostName::Domain(name.to_owned()), 0)
    }
}

/// What each kind a rule refuses with does to an A query and to an AAAA query:
/// REJECT and REJECT-TINYGIF an empty answer, REJECT-DROP none, REJECT-NO-DROP
/// an address of rurge's own for A and an empty answer for AAAA. The session
/// the rule is asked about is the one the query makes.
#[tokio::test]
async fn pre_matching_answers_by_kind() {
    let lab = lab();
    let blocked = HostName::Domain("blocked.test".to_owned());
    let mut expected = Vec::new();
    for kind in [
        RejectKind::Reject,
        RejectKind::TinyGif,
        RejectKind::Drop,
        RejectKind::NoDrop,
    ] {
        lab.extras
            .hooks
            .script_pre_match(PreMatchLayer::Dns, blocked.clone(), kind);
        let a = query(1, "Blocked.TEST", A);
        let aaaa = query(2, "Blocked.TEST", AAAA);
        let from_a = lab.ask(&a, &udp4()).await;
        let from_aaaa = lab.ask(&aaaa, &udp6()).await;
        expected.push((PreMatchLayer::Dns, dns_session(&udp4(), "blocked.test")));
        expected.push((PreMatchLayer::Dns, dns_session(&udp6(), "blocked.test")));
        match kind {
            RejectKind::Reject | RejectKind::TinyGif => {
                assert_empty(&from_a.expect("an answer"), &a, Rcode::NoError);
                assert_empty(&from_aaaa.expect("an answer"), &aaaa, Rcode::NoError);
            }
            RejectKind::Drop => {
                assert_eq!(from_a, None, "REJECT-DROP says nothing to A");
                assert_eq!(from_aaaa, None, "REJECT-DROP says nothing to AAAA");
            }
            RejectKind::NoDrop => {
                let from_a = from_a.expect("an answer");
                assert!(authoritative(&from_a));
                let answer = parsed(&from_a);
                assert_eq!(answer.rcode, Rcode::NoError);
                assert_eq!(answer.v4, [(REFUSED_V4, 30)]);
                assert_empty(&from_aaaa.expect("an answer"), &aaaa, Rcode::NoError);
            }
        }
    }
    assert_eq!(lab.extras.hooks.pre_matches_asked(), expected);
    // the transport is the query's: over TCP the session says so
    let a = query(3, "blocked.test", A);
    lab.reply(&a, &tcp4()).await;
    let asked = lab.extras.hooks.pre_matches_asked();
    assert_eq!(
        asked.last(),
        Some(&(PreMatchLayer::Dns, dns_session(&tcp4(), "blocked.test")))
    );
    // a refused name is forwarded nowhere and is given no address
    assert!(lab.extras.dns.forwarded().is_empty());
    assert_eq!(lab.fake_of("blocked.test", false), None);
    lab.assert_no_address_given();
}

/// A rule is asked about A and AAAA queries and no others, and a name it would
/// refuse is not refused for a type it is not asked about.
#[tokio::test]
async fn pre_matching_is_asked_only_for_a_and_aaaa() {
    let lab = lab();
    lab.configure("", true);
    for name in ["t.test", "use-application-dns.net"] {
        lab.extras.hooks.script_pre_match(
            PreMatchLayer::Dns,
            HostName::Domain(name.to_owned()),
            RejectKind::Reject,
        );
    }
    // MX and ANY, TXT, SVCB and HTTPS: forwarded, though a rule would refuse
    for (id, qtype) in [TXT, 15, 255, SVCB, HTTPS].into_iter().enumerate() {
        let q = query(id as u16, "t.test", qtype);
        assert_eq!(lab.reply(&q, &udp4()).await, echoed(&q, 0), "type {qtype}");
    }
    // the canary, whatever the type
    let canary = query(1, "use-application-dns.net", A);
    assert_empty(&lab.reply(&canary, &udp4()).await, &canary, Rcode::NxDomain);
    // a class other than IN
    let mut chaos = query(2, "t.test", A);
    let end = chaos.len();
    chaos[end - 2..].copy_from_slice(&3u16.to_be_bytes());
    assert_empty(&lab.reply(&chaos, &udp4()).await, &chaos, Rcode::NotImp);
    assert!(lab.extras.hooks.pre_matches_asked().is_empty());
    // A and AAAA are asked
    let a = query(3, "t.test", A);
    assert_empty(&lab.reply(&a, &udp4()).await, &a, Rcode::NoError);
    let aaaa = query(4, "t.test", AAAA);
    assert_empty(&lab.reply(&aaaa, &udp6()).await, &aaaa, Rcode::NoError);
    assert_eq!(lab.extras.hooks.pre_matches_asked().len(), 2);
}

/// A rule is asked before the family of the transport is looked at: the
/// refusal that says nothing is not turned into an empty answer.
#[tokio::test]
async fn a_refusal_comes_before_the_family_check() {
    let lab = lab();
    lab.extras.hooks.script_pre_match(
        PreMatchLayer::Dns,
        HostName::Domain("x.test".to_owned()),
        RejectKind::Drop,
    );
    assert_eq!(lab.ask(&query(1, "x.test", AAAA), &udp4()).await, None);
    assert_eq!(lab.ask(&query(2, "x.test", A), &udp6()).await, None);
    assert_eq!(lab.extras.hooks.pre_matches_asked().len(), 2);
}

// ---- real addresses and the pool

/// A name on `always-real-ip` that `[Host]` maps is answered here with the
/// addresses of the family asked for; one that it does not map is forwarded;
/// one that is not on the list, or whose entry names a port, has a fake address
/// whatever `[Host]` says of it.
#[tokio::test]
async fn always_real_ip_answers_from_the_host_section_or_forwards() {
    let lab = lab();
    lab.configure(
        "mapped.test, unmapped.test, v6only.test, port.test:443",
        false,
    );
    let (real4, real6) = (
        Ipv4Addr::new(192, 0, 2, 7),
        Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 7),
    );
    lab.extras
        .dns
        .map("mapped.test", &[IpAddr::V4(real4), IpAddr::V6(real6)]);
    lab.extras.dns.map("v6only.test", &[IpAddr::V6(real6)]);
    lab.extras.dns.map("other.test", &[IpAddr::V4(real4)]);
    lab.extras.dns.map("port.test", &[IpAddr::V4(real4)]);

    let a = lab.reply(&query(1, "Mapped.TEST", A), &udp4()).await;
    assert!(authoritative(&a));
    assert_eq!(parsed(&a).rcode, Rcode::NoError);
    assert_eq!(parsed(&a).v4, [(real4, 30)]);
    assert!(parsed(&a).v6.is_empty(), "only the family asked for");
    let aaaa = lab.reply(&query(2, "mapped.test", AAAA), &udp6()).await;
    assert_eq!(parsed(&aaaa).v6, [(real6, 30)]);
    assert!(parsed(&aaaa).v4.is_empty());
    // mapped to addresses of the other family only: an answer without records,
    // not a question for the upstreams
    let v6only = query(3, "v6only.test", A);
    assert_empty(&lab.reply(&v6only, &udp4()).await, &v6only, Rcode::NoError);
    assert!(lab.extras.dns.forwarded().is_empty());
    assert_eq!(lab.fake_of("mapped.test", false), None);

    // on the list, not mapped: forwarded
    let unmapped = query(4, "unmapped.test", A);
    assert_eq!(lab.reply(&unmapped, &udp4()).await, echoed(&unmapped, 0));
    assert_eq!(lab.extras.dns.forwarded(), [unmapped]);
    assert_eq!(lab.fake_of("unmapped.test", false), None);

    // not on the list, or the entry has a port, which a query has not
    for name in ["other.test", "port.test"] {
        let answer = parsed(&lab.reply(&query(5, name, A), &udp4()).await);
        assert_eq!(answer.v4.len(), 1, "{name}");
        assert_eq!(
            Some(IpAddr::V4(answer.v4[0].0)),
            lab.fake_of(name, false),
            "{name}"
        );
    }
    assert_eq!(lab.extras.dns.forwarded().len(), 1);
}

/// A name the pool does not take (a label with a character other than a letter,
/// a digit, `-` and `_`, the root) is a question for the upstreams.
#[tokio::test]
async fn a_name_the_pool_does_not_take_is_forwarded() {
    let lab = lab();
    for (id, name) in ["*.wild.test", "with space.test", "."]
        .into_iter()
        .enumerate()
    {
        let q = query(id as u16, name, A);
        assert_eq!(lab.reply(&q, &udp4()).await, echoed(&q, 0), "{name}");
    }
    assert_eq!(lab.extras.dns.forwarded().len(), 3);
    lab.assert_no_address_given();
}

#[tokio::test]
async fn without_a_pool_a_and_aaaa_are_forwarded() {
    let lab = Lab::without_a_pool();
    let a = query(1, "a.test", A);
    let aaaa = query(2, "a.test", AAAA);
    assert_eq!(lab.reply(&a, &udp4()).await, echoed(&a, 0));
    assert_eq!(lab.reply(&aaaa, &udp6()).await, echoed(&aaaa, 0));
    assert_eq!(lab.extras.dns.forwarded(), [a, aaaa]);
}

/// With every address leased a new name gets SERVFAIL, and the first time the
/// TUN says so at WARN, without the name; a name that has an address keeps it,
/// and the pool is used again once an address is let go.
#[tokio::test]
async fn an_exhausted_pool_gets_servfail_and_one_warn() {
    let log = LogCapture::start();
    let pool = Arc::new(FakeIpPool::with_count(2));
    let lab = lab_with(Extras {
        pool: Some(pool.clone()),
        ..Extras::default()
    });
    // both addresses taken, and a lease on each, as two connections would hold
    let now = Instant::now();
    let mut leases = Vec::new();
    for name in ["held-a.test", "held-b.test"] {
        let index = pool.assign(name, now).expect("an address");
        let (_, lease) = pool.lease(IpAddr::V4(v4_of(index))).expect("a lease");
        leases.push(lease);
    }
    for (id, name) in ["c.test", "d.test"].into_iter().enumerate() {
        for (qtype, ask) in [(A, udp4()), (AAAA, udp6())] {
            let q = query(id as u16, name, qtype);
            assert_empty(&lab.reply(&q, &ask).await, &q, Rcode::ServFail);
        }
    }
    // a name that has its address keeps it
    let held = parsed(&lab.reply(&query(5, "held-a.test", A), &udp4()).await);
    assert_eq!(held.v4.len(), 1);
    let above_debug = |log: &LogCapture| -> Vec<String> {
        log.lines()
            .into_iter()
            .filter(|line| !line.starts_with("DEBUG") && !line.starts_with("TRACE"))
            .collect()
    };
    assert_eq!(
        above_debug(&log),
        ["WARN tun: every fake address is in use; new names get SERVFAIL"]
    );
    // the names are said at debug level only
    let lines = log.lines();
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("DEBUG") && line.contains("c.test")),
        "{lines:?}"
    );
    // an address let go is used again; running out again is not said again
    drop(leases.remove(0));
    let e = parsed(&lab.reply(&query(6, "e.test", A), &udp4()).await);
    assert_eq!(e.v4.len(), 1, "the pool has an address again");
    let (_, lease) = pool
        .lease(IpAddr::V4(e.v4[0].0))
        .expect("the lease on the new name");
    leases.push(lease);
    let f = query(7, "f.test", A);
    assert_empty(&lab.reply(&f, &udp4()).await, &f, Rcode::ServFail);
    assert_eq!(
        above_debug(&log).len(),
        1,
        "once for the TUN: {:?}",
        log.lines()
    );
}

// ---- what is forwarded

/// A query of a type the responder does not make answers for goes to the
/// resolver as it came, and what the resolver answers is handed back as it is.
#[tokio::test]
async fn other_types_are_forwarded_and_handed_back_as_they_are() {
    let lab = lab();
    lab.extras
        .dns
        .script("txt.test", ForwardScript::Echo { pad: 37 });
    let mut sent = Vec::new();
    // TXT, MX, ANY, NS
    for (id, qtype) in [TXT, 15, 255, 2].into_iter().enumerate() {
        let q = query_opt(id as u16, "TXT.test", qtype, 4096);
        for ask in [udp4(), tcp6()] {
            assert_eq!(lab.reply(&q, &ask).await, echoed(&q, 37), "type {qtype}");
            sent.push(q.clone());
        }
    }
    assert_eq!(lab.extras.dns.forwarded(), sent);
    // forwarding is no question for the rules and takes no address
    assert!(lab.extras.hooks.pre_matches_asked().is_empty());
    lab.assert_no_address_given();
}

#[tokio::test]
async fn a_failed_forward_is_servfail() {
    let lab = lab();
    lab.extras.dns.script("down.test", ForwardScript::Fail);
    let txt = query(1, "down.test", TXT);
    assert_empty(&lab.reply(&txt, &udp4()).await, &txt, Rcode::ServFail);
    // a fake address needs no upstream, a real one does
    lab.configure("down.test", false);
    let a = query(2, "down.test", A);
    assert_empty(&lab.reply(&a, &tcp4()).await, &a, Rcode::ServFail);
}

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
    let responder = Responder::new(
        counting.clone(),
        extras.pool.clone(),
        Arc::new(Stats::default()),
    );
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
/// what became of it, whether it was answered or not; nothing above debug level
/// says a name.
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
    // (the name, the type as the line writes it, the query)
    let queries = [
        ("fake.test", "A", query(1, "fake.test", A)),
        ("forward.test", "TYPE16", query(2, "forward.test", TXT)),
        ("dropped.test", "A", query(3, "dropped.test", A)),
        ("fail.test", "TYPE16", query(4, "fail.test", TXT)),
        (
            "use-application-dns.net",
            "AAAA",
            query(5, "use-application-dns.net", AAAA),
        ),
    ];
    for (_, _, q) in &queries {
        let _ = lab.ask(q, &udp4()).await;
    }
    // and one that is no query, which has no name to say
    let mut reply = query(6, "reply.test", A);
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
    for (name, qtype, _) in &queries {
        let said: Vec<&String> = lines.iter().filter(|line| line.contains(name)).collect();
        assert_eq!(said.len(), 1, "{name}: {lines:?}");
        assert!(said[0].contains(&format!("qtype={qtype} ")), "{}", said[0]);
        let decision = said[0]
            .split("decision=")
            .nth(1)
            .unwrap_or_else(|| panic!("what was decided: {}", said[0]));
        decisions.push(decision.to_owned());
    }
    // five different things became of them
    decisions.sort();
    decisions.dedup();
    assert_eq!(decisions.len(), queries.len(), "{decisions:?}");
}
