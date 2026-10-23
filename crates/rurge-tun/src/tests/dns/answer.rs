//! What the responder answers, query by query, in the order it decides (phase 3
//! M3 design 11.2): a message that is no query, a class or a type it does not
//! do, the canary, a refusal by a pre-matching rule, the family of the
//! transport, `always-real-ip`, a fake address, and everything else, which goes
//! to the resolver.

use super::{
    A, AAAA, HTTPS, ResponderRig, SVCB, TXT, assert_empty, authoritative, counts, echoed, id_of,
    is_response, parsed, query, query_opt, question_of, rcode_of, recursion_available,
    responder_rig, responder_rig_with, tcp4, tcp6, udp4, udp6,
};
use crate::dns::Ask;
use crate::space::REFUSED_V4;
use crate::testing::ForwardScript;
use crate::tests::log_capture::LogCapture;
use crate::tests::support::Extras;
use rurge_config::{HostName, ListenerKind, PreMatchLayer, SessionInfo};
use rurge_dns::fakeip::{FakeIpPool, index_of, v4_of, v6_of};
use rurge_dns::message::Rcode;
use rurge_proto::RejectKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Instant;

// ---- a message that is no query to answer

/// A reply is not answered, and neither is anything shorter than a header.
#[tokio::test]
async fn a_reply_or_a_runt_gets_no_answer() {
    let lab = responder_rig();
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
    let lab = responder_rig();
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
    let lab = responder_rig();
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
    let lab = responder_rig();
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
    let lab = responder_rig();
    let mut q = query(11, "Class.test", A);
    let end = q.len();
    // CH
    q[end - 2..].copy_from_slice(&3u16.to_be_bytes());
    let reply = lab.reply(&q, &udp4()).await;
    assert_empty(&reply, &q, Rcode::NotImp);
    lab.assert_untouched();
}

/// The class is looked at before the name: the canary asked in a class other
/// than IN is NOTIMP, as any name is.
#[tokio::test]
async fn the_class_comes_before_the_canary() {
    let lab = responder_rig();
    let mut q = query(12, "use-application-dns.net", A);
    let end = q.len();
    // CH
    q[end - 2..].copy_from_slice(&3u16.to_be_bytes());
    assert_empty(&lab.reply(&q, &udp4()).await, &q, Rcode::NotImp);
    lab.assert_untouched();
}

// ---- the names and types that are answered before anything else

#[tokio::test]
async fn the_canary_is_nxdomain_for_every_type() {
    let lab = responder_rig();
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
    let lab = responder_rig();
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

/// The canary is looked at before the type: asked as SVCB or HTTPS while those
/// are not allowed it is NXDOMAIN, not NOTIMP.
#[tokio::test]
async fn the_canary_comes_before_svcb_and_https() {
    let lab = responder_rig();
    for (id, qtype) in [SVCB, HTTPS].into_iter().enumerate() {
        let q = query(id as u16, "use-application-dns.net", qtype);
        assert_empty(&lab.reply(&q, &udp4()).await, &q, Rcode::NxDomain);
    }
    lab.assert_untouched();
}

// ---- A and AAAA

/// A name gets the fake address of its index: the IPv4 one for an A query, the
/// IPv6 one of the same index for AAAA, with a TTL of 30 seconds, authoritative,
/// and asking again gets the same.
#[tokio::test]
async fn a_and_aaaa_get_fake_addresses() {
    let lab = responder_rig();
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
    let lab = responder_rig();
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
    let lab = responder_rig();
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
    let lab = responder_rig();
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
    let lab = responder_rig();
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
    let lab = responder_rig();
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
    let lab = responder_rig();
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

/// The family of the transport is looked at before `always-real-ip`: a name on
/// the list that `[Host]` maps to addresses of both families, asked with the
/// family the transport does not fit, gets the empty answer of the family check,
/// not the addresses, and nothing is forwarded.
#[tokio::test]
async fn the_family_check_comes_before_always_real_ip() {
    let lab = responder_rig();
    lab.configure("real.test", false);
    lab.extras.dns.map(
        "real.test",
        &[
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)),
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 7)),
        ],
    );
    let a_over_v6 = query(1, "real.test", A);
    let reply = lab.reply(&a_over_v6, &udp6()).await;
    assert!(authoritative(&reply));
    assert_empty(&reply, &a_over_v6, Rcode::NoError);
    let aaaa_over_v4 = query(2, "real.test", AAAA);
    let reply = lab.reply(&aaaa_over_v4, &udp4()).await;
    assert!(authoritative(&reply));
    assert_empty(&reply, &aaaa_over_v4, Rcode::NoError);
    assert!(lab.extras.dns.forwarded().is_empty());
}

/// A name the pool does not take (a label with a character other than a letter,
/// a digit, `-` and `_`, the root) is a question for the upstreams.
#[tokio::test]
async fn a_name_the_pool_does_not_take_is_forwarded() {
    let lab = responder_rig();
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
    let lab = ResponderRig::without_a_pool();
    let a = query(1, "a.test", A);
    let aaaa = query(2, "a.test", AAAA);
    assert_eq!(lab.reply(&a, &udp4()).await, echoed(&a, 0));
    assert_eq!(lab.reply(&aaaa, &udp6()).await, echoed(&aaaa, 0));
    assert_eq!(lab.extras.dns.forwarded(), [a, aaaa]);
}

/// `always-real-ip` is looked at before the lack of a pool: with no pool a name
/// on the list that `[Host]` maps still gets its addresses, and is not forwarded.
#[tokio::test]
async fn always_real_ip_comes_before_the_lack_of_a_pool() {
    let lab = ResponderRig::without_a_pool();
    lab.configure("real.test", false);
    let real = Ipv4Addr::new(192, 0, 2, 7);
    lab.extras.dns.map("real.test", &[IpAddr::V4(real)]);
    let reply = lab.reply(&query(1, "real.test", A), &udp4()).await;
    assert!(authoritative(&reply));
    assert_eq!(parsed(&reply).v4, [(real, 30)]);
    assert!(lab.extras.dns.forwarded().is_empty());
}

/// With every address leased a new name gets SERVFAIL, and the first time the
/// TUN says so at WARN, without the name; a name that has an address keeps it,
/// and the pool is used again once an address is let go.
#[tokio::test]
async fn an_exhausted_pool_gets_servfail_and_one_warn() {
    let log = LogCapture::start();
    let pool = Arc::new(FakeIpPool::with_count(2));
    let lab = responder_rig_with(Extras {
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
    let lab = responder_rig();
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
    let lab = responder_rig();
    lab.extras.dns.script("down.test", ForwardScript::Fail);
    let txt = query(1, "down.test", TXT);
    assert_empty(&lab.reply(&txt, &udp4()).await, &txt, Rcode::ServFail);
    // a fake address needs no upstream, a real one does
    lab.configure("down.test", false);
    let a = query(2, "down.test", A);
    assert_empty(&lab.reply(&a, &tcp4()).await, &a, Rcode::ServFail);
}
