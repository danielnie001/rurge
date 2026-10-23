//! The DNS responder (phase 3 M3 design 11): what it answers to each query,
//! asked of `Responder::answer` directly, with the doubles that stand behind it
//! (`FakeDns` for the resolver, `FakeHooks` for the pre-matching rules, a pool
//! of fake addresses). The query is made with `query_wire`; a reply is read
//! byte by byte where its bytes are the point and with `parse_response` where
//! its records are.
//!
//! `answer` follows the order the responder decides in, step by step; `forward`
//! is about the forwards in flight, what becomes of a reply on its way out (the
//! size of a datagram and of a frame, the OPT record), how often the settings
//! are taken, and what is said. `udp` is about the datagrams of the apps: which
//! of them go to the responder (its own addresses, the servers `hijack-dns`
//! names), through the memory device and the app's end of it, with the queue
//! and the tasks that answer them. `tcp` is about the connections of the apps
//! to its port 53: the queries on them, when they are closed, and the
//! connections to a server `hijack-dns` names, which are not its own.

mod answer;
mod forward;
mod tcp;
mod udp;

use crate::TunSettings;
use crate::dns::{Ask, DNS_PORT, Responder};
use crate::testing::FakeResolve;
use crate::tests::support::{Extras, addr, settings, sock, within};
use rurge_config::{HostList, Transport};
use rurge_dns::message::{Answer, Rcode, parse_response, wire_rcode, wire_truncated};
use rurge_dns::testing::query_wire;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

/// The types of the queries these tests make.
const A: u16 = 1;
const TXT: u16 = 16;
const AAAA: u16 = 28;
const SVCB: u16 = 64;
const HTTPS: u16 = 65;

/// The app's endpoint and the responder's, one family each. A query over
/// IPv4 is sent to the responder's IPv4 address.
fn ask(transport: Transport, v6: bool) -> Ask {
    let (src, server) = if v6 {
        (
            addr("[fd00:6152::1]:5000"),
            sock(Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0, 2), DNS_PORT),
        )
    } else {
        (
            addr("198.18.0.1:5000"),
            sock(Ipv4Addr::new(198, 18, 0, 2), DNS_PORT),
        )
    };
    Ask {
        src,
        server,
        transport,
    }
}

fn udp4() -> Ask {
    ask(Transport::Udp, false)
}

fn udp6() -> Ask {
    ask(Transport::Udp, true)
}

fn tcp4() -> Ask {
    ask(Transport::Tcp, false)
}

fn tcp6() -> Ask {
    ask(Transport::Tcp, true)
}

/// A query for `name` of type `qtype`, class IN, RD set, without an OPT record.
fn query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
    query_wire(id, name, qtype, None)
}

/// The same with an OPT record that announces `size`.
fn query_opt(id: u16, name: &str, qtype: u16, size: u16) -> Vec<u8> {
    query_wire(id, name, qtype, Some(size))
}

/// What `FakeDns` answers a forwarded query with unless a test scripted
/// otherwise: the query as a reply, QR and RA set, `pad` zeros after it.
fn echoed(query: &[u8], pad: usize) -> Vec<u8> {
    let mut reply = query.to_vec();
    reply.resize(query.len() + pad, 0);
    reply[2] |= 0x80;
    reply[3] |= 0x80;
    reply
}

fn id_of(message: &[u8]) -> u16 {
    u16::from_be_bytes([message[0], message[1]])
}

fn is_response(reply: &[u8]) -> bool {
    reply[2] & 0x80 != 0
}

fn authoritative(reply: &[u8]) -> bool {
    reply[2] & 0x04 != 0
}

fn truncated(reply: &[u8]) -> bool {
    wire_truncated(reply)
}

fn recursion_available(reply: &[u8]) -> bool {
    reply[3] & 0x80 != 0
}

fn rcode_of(reply: &[u8]) -> Rcode {
    wire_rcode(reply).expect("a header")
}

/// QDCOUNT, ANCOUNT, NSCOUNT and ARCOUNT.
fn counts(message: &[u8]) -> [u16; 4] {
    [4, 6, 8, 10].map(|at| u16::from_be_bytes([message[at], message[at + 1]]))
}

/// The question section of a message that has one question, whole and as it is
/// written (the case of the name too).
fn question_of(message: &[u8]) -> &[u8] {
    let mut at = 12;
    while message[at] != 0 {
        at += 1 + usize::from(message[at]);
    }
    // the root label, then the type and the class
    &message[12..at + 1 + 4]
}

/// The records of a reply, as the codec of the resolver reads them.
fn parsed(reply: &[u8]) -> Answer {
    parse_response(reply).expect("a reply that reads")
}

/// `reply` answers `query` with `rcode` and no record: it is a response with
/// the query's ID and RD, RA set, and it carries the query's question byte for
/// byte.
fn assert_empty(reply: &[u8], query: &[u8], rcode: Rcode) {
    assert!(is_response(reply), "QR: {reply:?}");
    assert!(recursion_available(reply), "RA: {reply:?}");
    assert_eq!(id_of(reply), id_of(query), "the ID");
    assert_eq!(reply[2] & 0x01, query[2] & 0x01, "RD");
    assert_eq!(rcode_of(reply), rcode, "{reply:?}");
    assert_eq!(counts(reply)[1], 0, "no record: {reply:?}");
    assert_eq!(question_of(reply), question_of(query), "the question");
}

/// A responder on the fake hooks of `Extras`, with the doubles behind it.
struct ResponderRig {
    responder: Arc<Responder>,
    extras: Extras,
}

fn responder_rig() -> ResponderRig {
    responder_rig_with(Extras::default())
}

fn responder_rig_with(extras: Extras) -> ResponderRig {
    let responder = Arc::new(Responder::new(extras.hooks.clone(), extras.pool.clone()));
    ResponderRig { responder, extras }
}

impl ResponderRig {
    /// A lab whose responder has no pool of fake addresses.
    fn without_a_pool() -> ResponderRig {
        responder_rig_with(Extras {
            pool: None,
            ..Extras::default()
        })
    }

    /// From now on the generation in use has `always-real-ip` as `entries`
    /// says (a list as the profile writes it) and `allow-dns-svcb` as `svcb`
    /// says; the lab's `FakeDns` stays what it is.
    fn configure(&self, entries: &str, svcb: bool) {
        self.extras.hooks.set(TunSettings {
            dns: self.extras.dns.clone(),
            always_real_ip: HostList::parse(entries, None),
            allow_dns_svcb: svcb,
            ..settings(FakeResolve::new(), true)
        });
    }

    /// The reply to `query` as the responder gives it, which has to come in
    /// time.
    async fn ask(&self, query: &[u8], ask: &Ask) -> Option<Vec<u8>> {
        within("the responder's answer", self.responder.answer(query, ask)).await
    }

    /// The same for a query that has to be answered.
    async fn reply(&self, query: &[u8], ask: &Ask) -> Vec<u8> {
        self.ask(query, ask)
            .await
            .expect("the responder answers the query")
    }

    /// The fake address the pool has for `name`, if the responder gave it one.
    fn fake_of(&self, name: &str, v6: bool) -> Option<IpAddr> {
        self.extras.pool.as_ref()?.address_of(name, v6)
    }

    /// No name has an address: the pool, which the lab started with none, names
    /// nothing. Every test that asks this starts its lab afresh and gives no
    /// address itself first.
    fn assert_no_address_given(&self) {
        if let Some(pool) = &self.extras.pool {
            assert_eq!(pool.named(), 0, "the responder gave an address");
        }
    }

    /// Nothing came of the queries: nothing was forwarded, no rule was asked,
    /// and no name was given an address.
    fn assert_untouched(&self) {
        assert!(self.extras.dns.forwarded().is_empty(), "forwarded");
        assert!(
            self.extras.hooks.pre_matches_asked().is_empty(),
            "a rule was asked"
        );
        self.assert_no_address_given();
    }
}
