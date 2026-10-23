//! The DNS responder (phase 3 M3 design 11): what the TUN answers to a DNS
//! query that comes to it, whether the app sent it to one of the responder's
//! addresses or to a server that `hijack-dns` names, and over UDP or TCP.
//!
//! `Responder::answer` is the one place the answer is decided, in the order of
//! the design: a message that is no query, a class other than IN, the canary,
//! SVCB and HTTPS, and for A and AAAA a rule that refuses the name, the family
//! of the transport, `always-real-ip` and the fake address; every other query
//! is forwarded to the resolver of the generation in use. A reply made here or
//! forwarded is fitted to the transport it goes back on.

use crate::hooks::{Hooks, TunSettings};
use crate::space::REFUSED_V4;
use rurge_config::{HostName, ListenerKind, PreMatchLayer, SessionInfo, Transport};
use rurge_dns::fakeip::{AssignError, FAKE_TTL, FakeIpPool, v4_of, v6_of};
use rurge_dns::message::{self, ANSWER_UDP_SIZE, Rcode, Request, RequestError};
use rurge_proto::RejectKind;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::Semaphore;

/// The port DNS is on.
pub(crate) const DNS_PORT: u16 = 53;

/// The name that tells a browser to leave DNS over HTTPS off (RFC 8484's
/// "canary domain" of Firefox): answered NXDOMAIN, whatever the type.
pub(crate) const CANARY: &str = "use-application-dns.net";

/// The most queries forwarded at once, UDP and TCP together.
pub(crate) const MAX_FORWARDS: usize = 256;

/// The types the responder tells apart, and the class it answers.
const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const TYPE_SVCB: u16 = 64;
const TYPE_HTTPS: u16 = 65;
const CLASS_IN: u16 = 1;

/// The TTL of every record the responder makes, the fake addresses' (design
/// 10.2): addresses `[Host]` maps and the address of REJECT-NO-DROP have it too.
const TTL: u32 = FAKE_TTL.as_secs() as u32;

/// The longest a reply may be over UDP when the query announces no size, and the
/// shortest a size the query announces is taken for (RFC 6891 6.2.3).
const UDP_MIN: usize = 512;

/// What a message can be as long as when it is framed by a two-byte length, as
/// over TCP.
const TCP_MAX: usize = u16::MAX as usize;

/// Who asked whom, and over what.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ask {
    /// The app's endpoint.
    pub(crate) src: SocketAddr,
    /// The endpoint the query was sent to: the responder's, or the server
    /// `hijack-dns` names. A reply is written as from it.
    pub(crate) server: SocketAddr,
    pub(crate) transport: Transport,
}

/// The responder of a TUN: one for the TUN, shared by the UDP and the TCP side,
/// so that the limit on what is forwarded is one limit, and the warning that
/// the pool is used up is said once.
pub(crate) struct Responder {
    hooks: Arc<dyn Hooks>,
    pool: Option<Arc<FakeIpPool>>,
    /// The places among the `MAX_FORWARDS` queries that can be forwarded at
    /// once. A query that finds none is answered SERVFAIL, not made to wait.
    forwards: Semaphore,
    /// The warning that every fake address is in use has been said.
    exhaustion_said: AtomicBool,
}

impl Responder {
    pub(crate) fn new(hooks: Arc<dyn Hooks>, pool: Option<Arc<FakeIpPool>>) -> Responder {
        Responder {
            hooks,
            pool,
            forwards: Semaphore::new(MAX_FORWARDS),
            exhaustion_said: AtomicBool::new(false),
        }
    }

    /// The reply to `query`, or `None` when nothing is to be sent back. The
    /// settings are those of the generation in use when the query is read, the
    /// same for all of the answer. What is decided, and what became of the
    /// reply on its way out (see `fit`), is said in one line at debug level,
    /// and nowhere else: the name is not said above it.
    pub(crate) async fn answer(&self, query: &[u8], ask: &Ask) -> Option<Vec<u8>> {
        let req = match message::read_request(query) {
            Ok(req) => req,
            Err(RequestError::NotAQuery) => {
                tracing::debug!("tun: a DNS message that is no query is not answered");
                return None;
            }
            Err(RequestError::NotImplemented(header)) => {
                tracing::debug!(
                    opcode = header.opcode,
                    "tun: a DNS message of an opcode other than QUERY is answered NOTIMP"
                );
                return Some(message::answer_header_only(&header, Rcode::NotImp));
            }
            Err(RequestError::Malformed(header)) => {
                tracing::debug!("tun: a DNS message that does not read is answered FORMERR");
                return Some(message::answer_header_only(&header, Rcode::FormErr));
            }
        };
        let settings = self.hooks.settings();
        let (reply, decision) = self.decide(&settings, &req, query, ask).await;
        let (reply, fitted) = match reply.map(|reply| fit(&req, ask.transport, reply)) {
            Some((reply, fitted)) => (Some(reply), fitted),
            None => (None, None),
        };
        tracing::debug!(
            name = %req.name,
            qtype = %TypeName(req.qtype),
            decision,
            fitted,
            "tun: DNS query"
        );
        reply
    }

    /// The reply to `req`, read from `query`, and what became of it, in the
    /// order of the design: the first that holds decides.
    async fn decide(
        &self,
        settings: &TunSettings,
        req: &Request,
        query: &[u8],
        ask: &Ask,
    ) -> (Option<Vec<u8>>, &'static str) {
        if req.qclass != CLASS_IN {
            return (
                Some(empty(req, Rcode::NotImp)),
                "a class other than IN: NOTIMP",
            );
        }
        if req.name == CANARY {
            return (Some(empty(req, Rcode::NxDomain)), "the canary: NXDOMAIN");
        }
        match req.qtype {
            TYPE_SVCB | TYPE_HTTPS if !settings.allow_dns_svcb => (
                Some(empty(req, Rcode::NotImp)),
                "SVCB or HTTPS while they are not allowed: NOTIMP",
            ),
            TYPE_A | TYPE_AAAA => self.address(settings, req, query, ask).await,
            _ => self.forward(settings, req, query).await,
        }
    }

    /// The reply to a query for A or AAAA: refused by a rule, empty for a
    /// transport of the other family, a real address `[Host]` gives, a fake
    /// address, or whatever the upstreams say.
    async fn address(
        &self,
        settings: &TunSettings,
        req: &Request,
        query: &[u8],
        ask: &Ask,
    ) -> (Option<Vec<u8>>, &'static str) {
        let wants_v4 = req.qtype == TYPE_A;
        if let Some(kind) = self
            .hooks
            .pre_match(PreMatchLayer::Dns, &dns_session(req, ask))
        {
            return match kind {
                RejectKind::Reject | RejectKind::TinyGif => (
                    Some(empty(req, Rcode::NoError)),
                    "refused by a rule: an empty answer",
                ),
                RejectKind::Drop => (None, "refused by a rule: no answer"),
                // an A query is sent to an address that refuses what comes to
                // it; `answer` writes the addresses of the family asked for
                // only, so an AAAA query gets none
                RejectKind::NoDrop => (
                    Some(message::answer(
                        req,
                        Rcode::NoError,
                        &[IpAddr::V4(REFUSED_V4)],
                        TTL,
                    )),
                    "refused by a rule: an address of rurge's own",
                ),
            };
        }
        if (wants_v4 && ask.server.is_ipv6()) || (!wants_v4 && ask.server.is_ipv4()) {
            return (
                Some(empty(req, Rcode::NoError)),
                "the family of the transport is the other: an empty answer",
            );
        }
        // a port in an entry never matches: a query has none
        if !settings.always_real_ip.is_empty()
            && settings
                .always_real_ip
                .matches(&HostName::Domain(req.name.clone()), 0)
                == Some(true)
        {
            return match settings.dns.mapped(&req.name) {
                Some(addrs) if !addrs.is_empty() => (
                    Some(message::answer(req, Rcode::NoError, &addrs, TTL)),
                    "always-real-ip: the addresses [Host] maps",
                ),
                _ => self.forward(settings, req, query).await,
            };
        }
        let Some(pool) = &self.pool else {
            return self.forward(settings, req, query).await;
        };
        match pool.assign(&req.name, Instant::now()) {
            Ok(index) => {
                let addr = if wants_v4 {
                    IpAddr::V4(v4_of(index))
                } else {
                    IpAddr::V6(v6_of(index))
                };
                (
                    Some(message::answer(req, Rcode::NoError, &[addr], TTL)),
                    "a fake address",
                )
            }
            Err(AssignError::NotAName) => self.forward(settings, req, query).await,
            Err(AssignError::Exhausted) => {
                if !self.exhaustion_said.swap(true, Ordering::Relaxed) {
                    tracing::warn!("tun: every fake address is in use; new names get SERVFAIL");
                }
                (
                    Some(empty(req, Rcode::ServFail)),
                    "every fake address is in use: SERVFAIL",
                )
            }
        }
    }

    /// What the upstreams of the generation say to `query`, as they said it; or
    /// SERVFAIL, at once, when `MAX_FORWARDS` queries are being forwarded
    /// already, and when they fail.
    async fn forward(
        &self,
        settings: &TunSettings,
        req: &Request,
        query: &[u8],
    ) -> (Option<Vec<u8>>, &'static str) {
        // held until the reply is in: the place is given back however this ends
        let Ok(_place) = self.forwards.try_acquire() else {
            return (
                Some(empty(req, Rcode::ServFail)),
                "too many queries are being forwarded: SERVFAIL",
            );
        };
        match settings.dns.forward(query).await {
            Ok(reply) => (Some(reply), "forwarded"),
            Err(_) => (
                Some(empty(req, Rcode::ServFail)),
                "the forward failed: SERVFAIL",
            ),
        }
    }
}

/// An answer to `req` with `rcode` and no record.
fn empty(req: &Request, rcode: Rcode) -> Vec<u8> {
    message::answer(req, rcode, &[], 0)
}

/// What the query is to a rule at the DNS layer (the engine reads a session
/// there): the app's endpoint, no port (a query has none), the adapter as the
/// listener, the transport of the query, the name in lower case, and the
/// endpoint it was sent to as rurge's own.
fn dns_session(req: &Request, ask: &Ask) -> SessionInfo {
    SessionInfo {
        src: ask.src,
        in_port: 0,
        local: Some(ask.server),
        listener: ListenerKind::Tun,
        transport: ask.transport,
        ..SessionInfo::tcp(HostName::Domain(req.name.clone()), 0)
    }
}

/// `reply` as the transport it goes back on can carry it, and what was done to
/// it, for the debug line, if anything was. Over UDP a reply of more than the
/// client takes (`udp_limit`) is replaced by one that says so: TC, without
/// records, with the response code it had. Over TCP a reply that a two-byte
/// length cannot frame (an upstream over HTTPS can send one) is replaced by
/// SERVFAIL.
fn fit(req: &Request, transport: Transport, reply: Vec<u8>) -> (Vec<u8>, Option<&'static str>) {
    match transport {
        Transport::Udp if reply.len() > udp_limit(req) => {
            let rcode = message::wire_rcode(&reply).unwrap_or(Rcode::ServFail);
            (
                message::answer_truncated(req, rcode),
                Some("truncated: over the UDP limit"),
            )
        }
        Transport::Tcp if reply.len() > TCP_MAX => (
            empty(req, Rcode::ServFail),
            Some("SERVFAIL: too long for TCP"),
        ),
        _ => (reply, None),
    }
}

/// The most a reply to `req` may be over UDP: the size its OPT record announces,
/// no less than 512 and no more than the 1232 the responder announces itself;
/// 512 for a query without one.
fn udp_limit(req: &Request) -> usize {
    match req.udp_size {
        Some(size) => usize::from(size).clamp(UDP_MIN, usize::from(ANSWER_UDP_SIZE)),
        None => UDP_MIN,
    }
}

/// A query type as the debug line writes it: the ones the responder tells
/// apart by name, the others as `TYPE` and the number (RFC 3597).
struct TypeName(u16);

impl fmt::Display for TypeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            TYPE_A => f.write_str("A"),
            TYPE_AAAA => f.write_str("AAAA"),
            TYPE_SVCB => f.write_str("SVCB"),
            TYPE_HTTPS => f.write_str("HTTPS"),
            other => write!(f, "TYPE{other}"),
        }
    }
}
