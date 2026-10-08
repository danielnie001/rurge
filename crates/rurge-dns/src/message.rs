//! Wire-format encoding and decoding on top of hickory-proto (design §7.2):
//! only what a stub resolver needs — A / AAAA questions and answers, EDNS
//! payload size, response codes and the TC bit. Transports never look inside
//! messages beyond `wire_id` / `wire_truncated`, DoQ's `doq_query` and DoH's
//! `lower_ttls`.

use hickory_proto::op::{
    DEFAULT_MAX_PAYLOAD_LEN, Edns, Message, MessageType, OpCode, Query, ResponseCode,
};
use hickory_proto::rr::rdata::opt::{EdnsCode, EdnsOption};
use hickory_proto::rr::rdata::{A, AAAA};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Qtype {
    A,
    Aaaa,
}

impl Qtype {
    pub fn as_str(self) -> &'static str {
        match self {
            Qtype::A => "A",
            Qtype::Aaaa => "AAAA",
        }
    }

    fn record_type(self) -> RecordType {
        match self {
            Qtype::A => RecordType::A,
            Qtype::Aaaa => RecordType::AAAA,
        }
    }

    fn from_record_type(rt: RecordType) -> Option<Qtype> {
        match rt {
            RecordType::A => Some(Qtype::A),
            RecordType::AAAA => Some(Qtype::Aaaa),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Question {
    pub name: String,
    pub qtype: Qtype,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rcode {
    NoError,
    NxDomain,
    ServFail,
    Refused,
    Other(u8),
}

impl Rcode {
    fn from_hickory(rc: ResponseCode) -> Rcode {
        match rc {
            ResponseCode::NoError => Rcode::NoError,
            ResponseCode::NXDomain => Rcode::NxDomain,
            ResponseCode::ServFail => Rcode::ServFail,
            ResponseCode::Refused => Rcode::Refused,
            other => Rcode::Other(other.low()),
        }
    }

    fn to_hickory(self) -> ResponseCode {
        match self {
            Rcode::NoError => ResponseCode::NoError,
            Rcode::NxDomain => ResponseCode::NXDomain,
            Rcode::ServFail => ResponseCode::ServFail,
            Rcode::Refused => ResponseCode::Refused,
            Rcode::Other(n) => ResponseCode::from_low(n),
        }
    }
}

impl fmt::Display for Rcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rcode::NoError => f.write_str("NOERROR"),
            Rcode::NxDomain => f.write_str("NXDOMAIN"),
            Rcode::ServFail => f.write_str("SERVFAIL"),
            Rcode::Refused => f.write_str("REFUSED"),
            Rcode::Other(n) => write!(f, "RCODE{n}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub id: u16,
    pub rcode: Rcode,
    pub truncated: bool,
    pub question: Option<Question>,
    pub v4: Vec<(Ipv4Addr, u32)>,
    pub v6: Vec<(Ipv6Addr, u32)>,
}

impl Answer {
    fn has_records(&self, qtype: Qtype) -> bool {
        match qtype {
            Qtype::A => !self.v4.is_empty(),
            Qtype::Aaaa => !self.v6.is_empty(),
        }
    }

    /// NOERROR, the question we asked, and at least one record of that type.
    pub fn is_valid_for(&self, q: &Question) -> bool {
        self.rcode == Rcode::NoError
            && self.question.as_ref() == Some(q)
            && self.has_records(q.qtype)
    }

    /// NOERROR or NXDOMAIN without records of the asked type.
    pub fn is_empty_for(&self, q: &Question) -> bool {
        matches!(self.rcode, Rcode::NoError | Rcode::NxDomain)
            && self.question.as_ref().is_none_or(|x| x == q)
            && !self.has_records(q.qtype)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecError(pub String);

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodecError {}

fn codec<E: fmt::Display>(e: E) -> CodecError {
    CodecError(e.to_string())
}

/// Unpredictable message IDs without a `rand` dependency: a per-process
/// random hasher over a counter and the clock.
pub fn random_id() -> u16 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(n);
    h.write_u64(nanos);
    (h.finish() & 0xffff) as u16
}

fn fqdn(name: &str) -> Result<Name, CodecError> {
    let mut s = name.trim().trim_end_matches('.').to_ascii_lowercase();
    if s.is_empty() {
        return Err(CodecError("empty name".to_string()));
    }
    s.push('.');
    Name::from_ascii(&s).map_err(codec)
}

fn question_of(q: &Query) -> Option<Question> {
    let qtype = Qtype::from_record_type(q.query_type())?;
    let name = q
        .name()
        .to_ascii()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    Some(Question { name, qtype })
}

pub fn build_query(id: u16, q: &Question) -> Result<Vec<u8>, CodecError> {
    let mut m = Message::new(id, MessageType::Query, OpCode::Query);
    m.metadata.recursion_desired = true;
    m.add_query(Query::query(fqdn(&q.name)?, q.qtype.record_type()));
    let mut edns = Edns::new();
    edns.set_max_payload(DEFAULT_MAX_PAYLOAD_LEN);
    m.set_edns(edns);
    m.to_vec().map_err(codec)
}

pub fn parse_query(bytes: &[u8]) -> Result<(u16, Question), CodecError> {
    let m = Message::from_vec(bytes).map_err(codec)?;
    let q = m
        .queries
        .first()
        .ok_or_else(|| CodecError("no question".to_string()))?;
    let question = question_of(q)
        .ok_or_else(|| CodecError(format!("unsupported query type {}", q.query_type())))?;
    Ok((m.id, question))
}

pub fn parse_response(bytes: &[u8]) -> Result<Answer, CodecError> {
    let m = Message::from_vec(bytes).map_err(codec)?;
    if m.message_type != MessageType::Response {
        return Err(CodecError("not a response".to_string()));
    }
    let question = m.queries.first().and_then(question_of);
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for r in &m.answers {
        match &r.data {
            RData::A(a) => v4.push((a.0, r.ttl)),
            RData::AAAA(a) => v6.push((a.0, r.ttl)),
            _ => {}
        }
    }
    Ok(Answer {
        id: m.id,
        rcode: Rcode::from_hickory(m.response_code),
        truncated: m.truncation,
        question,
        v4,
        v6,
    })
}

/// A response for mock servers and tests. `truncated` sets the TC bit and,
/// like a real server, sends no answer records.
pub fn build_response(
    id: u16,
    q: &Question,
    rcode: Rcode,
    records: &[(IpAddr, u32)],
    truncated: bool,
) -> Result<Vec<u8>, CodecError> {
    let mut m = Message::new(id, MessageType::Response, OpCode::Query);
    m.metadata.recursion_desired = true;
    m.metadata.recursion_available = true;
    m.metadata.response_code = rcode.to_hickory();
    m.metadata.truncation = truncated;
    let name = fqdn(&q.name)?;
    m.add_query(Query::query(name.clone(), q.qtype.record_type()));
    if !truncated {
        for (ip, ttl) in records {
            let data = match ip {
                IpAddr::V4(a) => RData::A(A(*a)),
                IpAddr::V6(a) => RData::AAAA(AAAA(*a)),
            };
            m.answers.push(Record::from_rdata(name.clone(), *ttl, data));
        }
    }
    m.to_vec().map_err(codec)
}

pub fn wire_id(bytes: &[u8]) -> Option<u16> {
    (bytes.len() >= 12).then(|| u16::from_be_bytes([bytes[0], bytes[1]]))
}

pub fn wire_truncated(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes[2] & 0x02 != 0
}

/// The type of the OPT pseudo-record (RFC 6891 6.1.1).
const OPT: u16 = 41;

/// Lowers the TTL of every record of the response `wire` — in its answer,
/// authority and additional sections — by `seconds`, no further than 0: the
/// response as it reads that long after it was made. The OPT pseudo-record
/// keeps its TTL field, which holds no TTL but its extended RCODE, version
/// and flags (RFC 6891 6.1.3). Only the TTLs are rewritten, where they
/// stand; a message that does not read to its last record keeps them all.
pub fn lower_ttls(wire: &mut [u8], seconds: u32) {
    if seconds == 0 {
        return;
    }
    for at in ttl_offsets(wire).unwrap_or_default() {
        let ttl = u32::from_be_bytes([wire[at], wire[at + 1], wire[at + 2], wire[at + 3]]);
        wire[at..at + 4].copy_from_slice(&ttl.saturating_sub(seconds).to_be_bytes());
    }
}

/// Where the TTLs of `wire`'s records stand, the OPT pseudo-record's field
/// left out; `None` for a message that does not read to its last record.
fn ttl_offsets(wire: &[u8]) -> Option<Vec<usize>> {
    let questions = wire_u16(wire, 4)?;
    // ANCOUNT, NSCOUNT and ARCOUNT
    let records = u32::from(wire_u16(wire, 6)?)
        + u32::from(wire_u16(wire, 8)?)
        + u32::from(wire_u16(wire, 10)?);
    let mut at = 12;
    for _ in 0..questions {
        // QTYPE and QCLASS follow the name
        at = name_end(wire, at)? + 4;
    }
    let mut ttls = Vec::new();
    for _ in 0..records {
        // TYPE, CLASS, TTL and RDLENGTH follow the name, the data follows them
        at = name_end(wire, at)?;
        if wire_u16(wire, at)? != OPT {
            ttls.push(at + 4);
        }
        at += 10 + usize::from(wire_u16(wire, at + 8)?);
    }
    (at <= wire.len()).then_some(ttls)
}

/// Where the name at `at` ends: after its root label, or after the pointer
/// that stands for the rest of it (RFC 1035 4.1.4).
fn name_end(wire: &[u8], mut at: usize) -> Option<usize> {
    loop {
        match *wire.get(at)? {
            0 => return Some(at + 1),
            len @ 1..=0x3f => at += 1 + usize::from(len),
            0xc0.. => return Some(at + 2),
            // a label of a type there is none of
            _ => return None,
        }
    }
}

/// The 16-bit number at `at`.
fn wire_u16(wire: &[u8], at: usize) -> Option<u16> {
    let bytes = wire.get(at..at + 2)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

/// What DNS over QUIC pads its queries to a multiple of: "Clients SHOULD
/// pad queries to the closest multiple of 128 octets" (RFC 8467 4.1, the
/// policy RFC 9250 5.4 points to).
pub const DOQ_PADDING_BLOCK: usize = 128;

/// The query `wire` as DNS over QUIC sends it (RFC 9250): message ID 0
/// (4.2.1), no edns-tcp-keepalive option (5.5.2), and an EDNS(0) Padding
/// option (RFC 7830) of zeros that makes it a multiple of
/// `DOQ_PADDING_BLOCK` bytes long (5.4) — in place of any it had, in the
/// OPT record it has or in one added.
pub fn doq_query(wire: &[u8]) -> Result<Vec<u8>, CodecError> {
    let mut m = Message::from_vec(wire).map_err(codec)?;
    m.metadata.id = 0;
    if let Some(edns) = &mut m.edns {
        edns.options_mut().remove(EdnsCode::Keepalive);
    }
    // an empty option first: its code and length are bytes of the message
    // too
    set_padding(&mut m, 0);
    let unpadded = m.to_vec().map_err(codec)?.len();
    set_padding(
        &mut m,
        (DOQ_PADDING_BLOCK - unpadded % DOQ_PADDING_BLOCK) % DOQ_PADDING_BLOCK,
    );
    m.to_vec().map_err(codec)
}

/// `len` zeros of EDNS(0) padding in `m`, in place of any it had.
fn set_padding(m: &mut Message, len: usize) {
    let options = m
        .edns
        .get_or_insert_with(|| {
            let mut edns = Edns::new();
            edns.set_max_payload(DEFAULT_MAX_PAYLOAD_LEN);
            edns
        })
        .options_mut();
    options.remove(EdnsCode::Padding);
    options.insert(EdnsOption::Unknown(EdnsCode::Padding.into(), vec![0; len]));
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::rr::rdata::NS;

    fn q(name: &str, qtype: Qtype) -> Question {
        Question {
            name: name.to_string(),
            qtype,
        }
    }

    #[test]
    fn query_round_trip_normalizes_the_name() {
        let wire = build_query(0x1234, &q("WWW.Example.COM.", Qtype::A)).unwrap();
        assert_eq!(wire_id(&wire), Some(0x1234));
        assert!(!wire_truncated(&wire));
        let (id, question) = parse_query(&wire).unwrap();
        assert_eq!(id, 0x1234);
        assert_eq!(question, q("www.example.com", Qtype::A));
        let (_, aaaa) = parse_query(&build_query(1, &q("x.test", Qtype::Aaaa)).unwrap()).unwrap();
        assert_eq!(aaaa.qtype, Qtype::Aaaa);
        assert!(build_query(1, &q("", Qtype::A)).is_err());
        assert!(parse_query(b"nope").is_err());
    }

    #[test]
    fn response_round_trip_with_records_and_ttls() {
        let question = q("a.test", Qtype::A);
        let wire = build_response(
            7,
            &question,
            Rcode::NoError,
            &[
                ("10.0.0.1".parse().unwrap(), 300),
                ("10.0.0.2".parse().unwrap(), 60),
                ("fd00::1".parse().unwrap(), 30),
            ],
            false,
        )
        .unwrap();
        let a = parse_response(&wire).unwrap();
        assert_eq!(a.id, 7);
        assert_eq!(a.rcode, Rcode::NoError);
        assert!(!a.truncated);
        assert_eq!(a.question, Some(question.clone()));
        assert_eq!(
            a.v4,
            vec![
                ("10.0.0.1".parse().unwrap(), 300),
                ("10.0.0.2".parse().unwrap(), 60)
            ]
        );
        assert_eq!(a.v6, vec![("fd00::1".parse().unwrap(), 30)]);
        assert!(a.is_valid_for(&question));
        assert!(!a.is_valid_for(&q("a.test", Qtype::Aaaa)) || !a.v6.is_empty());
        assert!(!a.is_empty_for(&question));
        // a query is not a response
        assert!(parse_response(&build_query(1, &question).unwrap()).is_err());
    }

    #[test]
    fn empty_and_error_answers() {
        let question = q("nx.test", Qtype::A);
        let nx =
            parse_response(&build_response(1, &question, Rcode::NxDomain, &[], false).unwrap())
                .unwrap();
        assert_eq!(nx.rcode, Rcode::NxDomain);
        assert!(nx.is_empty_for(&question) && !nx.is_valid_for(&question));
        let noerror_empty =
            parse_response(&build_response(1, &question, Rcode::NoError, &[], false).unwrap())
                .unwrap();
        assert!(noerror_empty.is_empty_for(&question));
        let servfail =
            parse_response(&build_response(1, &question, Rcode::ServFail, &[], false).unwrap())
                .unwrap();
        assert!(!servfail.is_empty_for(&question) && !servfail.is_valid_for(&question));
        assert_eq!(servfail.rcode.to_string(), "SERVFAIL");
        assert_eq!(Rcode::Other(9).to_string(), "RCODE9");
        // wrong question type is neither valid nor empty for the asked type
        let other = parse_response(
            &build_response(1, &q("nx.test", Qtype::Aaaa), Rcode::NoError, &[], false).unwrap(),
        )
        .unwrap();
        assert!(!other.is_valid_for(&question) && !other.is_empty_for(&question));
    }

    #[test]
    fn truncated_responses_set_tc_and_carry_no_records() {
        let question = q("big.test", Qtype::A);
        let wire = build_response(
            3,
            &question,
            Rcode::NoError,
            &[("10.0.0.1".parse().unwrap(), 1)],
            true,
        )
        .unwrap();
        assert!(wire_truncated(&wire));
        let a = parse_response(&wire).unwrap();
        assert!(a.truncated && a.v4.is_empty());
    }

    #[test]
    fn random_ids_vary() {
        let ids: std::collections::HashSet<u16> = (0..64).map(|_| random_id()).collect();
        assert!(ids.len() > 8, "{ids:?}");
    }

    /// The EDNS(0) padding of `wire`: `None` without any.
    fn padding(wire: &[u8]) -> Option<Vec<u8>> {
        let edns = Message::from_vec(wire).unwrap().edns?;
        match edns.option(EdnsCode::Padding)? {
            EdnsOption::Unknown(12, padding) => Some(padding.clone()),
            other => panic!("padding read as {other:?}"),
        }
    }

    /// DoQ's query is the resolver's with ID 0, padded with zeros to the
    /// next multiple of 128 bytes — the option's own four bytes counted,
    /// no padding at all when they make the multiple.
    #[test]
    fn a_doq_query_has_id_0_and_is_padded_to_128_bytes() {
        // 97 bytes of name: 124 bytes of query, 128 with an empty option
        let fits = format!("{}.{}", "a".repeat(63), "b".repeat(31));
        let long = ["c".repeat(63), "d".repeat(63), "e".repeat(63)].join(".");
        for (name, len, padded) in [("a.test", 128, 89), (&fits, 128, 0), (&long, 256, 32)] {
            let question = q(name, Qtype::Aaaa);
            let wire = build_query(0x1234, &question).unwrap();
            let doq = doq_query(&wire).unwrap();
            assert_eq!(doq.len(), len, "{name}");
            assert_eq!(wire_id(&doq), Some(0));
            assert_eq!(parse_query(&doq).unwrap(), (0, question));
            assert_eq!(padding(&doq), Some(vec![0; padded]), "{name}");
        }
    }

    /// Padding the query had is replaced and its edns-tcp-keepalive left
    /// out (RFC 9250 5.5.2); a query without an OPT record gets one.
    #[test]
    fn a_doq_query_replaces_padding_and_keepalive_and_has_an_opt_record() {
        let mut m = Message::new(0x4242, MessageType::Query, OpCode::Query);
        m.add_query(Query::query(fqdn("a.test").unwrap(), RecordType::A));
        let mut edns = Edns::new();
        edns.options_mut().insert(EdnsOption::Unknown(
            EdnsCode::Keepalive.into(),
            vec![0, 100],
        ));
        edns.options_mut().insert(EdnsOption::Unknown(
            EdnsCode::Padding.into(),
            vec![0xff; 300],
        ));
        m.set_edns(edns);
        let doq = doq_query(&m.to_vec().unwrap()).unwrap();
        assert_eq!(doq.len(), 128);
        assert_eq!(padding(&doq).map(|p| p.iter().all(|&b| b == 0)), Some(true));
        let edns = Message::from_vec(&doq).unwrap().edns.unwrap();
        assert!(edns.option(EdnsCode::Keepalive).is_none());

        let mut bare = Message::new(7, MessageType::Query, OpCode::Query);
        bare.add_query(Query::query(fqdn("a.test").unwrap(), RecordType::A));
        let doq = doq_query(&bare.to_vec().unwrap()).unwrap();
        assert_eq!(doq.len(), 128);
        assert!(padding(&doq).is_some());
        assert!(doq_query(b"nope").is_err());
    }

    /// The OPT record a query had says what it said — the UDP payload
    /// size, the DO bit, an option of another kind — through the padding.
    #[test]
    fn a_doq_query_keeps_what_its_opt_record_said() {
        let mut m = Message::new(0x4242, MessageType::Query, OpCode::Query);
        m.add_query(Query::query(fqdn("a.test").unwrap(), RecordType::A));
        let mut edns = Edns::new();
        edns.set_max_payload(4000);
        edns.set_dnssec_ok(true);
        let cookie = EdnsOption::Unknown(EdnsCode::Cookie.into(), vec![7; 8]);
        edns.options_mut().insert(cookie.clone());
        m.set_edns(edns);
        let doq = doq_query(&m.to_vec().unwrap()).unwrap();
        assert_eq!(doq.len() % 128, 0);
        let edns = Message::from_vec(&doq).unwrap().edns.unwrap();
        assert_eq!(edns.max_payload(), 4000);
        assert!(edns.flags().dnssec_ok);
        assert_eq!(edns.option(EdnsCode::Cookie), Some(&cookie));
        assert!(padding(&doq).is_some());
    }

    /// A response as a server writes it, its names compressed: an A record
    /// of each of the first two of `ttls`, a record of the third in its
    /// authority section and one of the fourth in its additional section —
    /// and an OPT record, whose TTL field is no TTL: it holds the DO flag.
    fn sections(ttls: [u32; 4]) -> Vec<u8> {
        let mut m = Message::new(0x4242, MessageType::Response, OpCode::Query);
        let name = fqdn("a.test").unwrap();
        let server = fqdn("ns.test").unwrap();
        m.add_query(Query::query(name.clone(), RecordType::A));
        for (ttl, host) in [(ttls[0], 1), (ttls[1], 2)] {
            let data = RData::A(A(Ipv4Addr::new(10, 0, 0, host)));
            m.answers.push(Record::from_rdata(name.clone(), ttl, data));
        }
        let data = RData::NS(NS(server.clone()));
        m.authorities
            .push(Record::from_rdata(fqdn("test").unwrap(), ttls[2], data));
        let data = RData::AAAA(AAAA(Ipv6Addr::LOCALHOST));
        m.additionals
            .push(Record::from_rdata(server, ttls[3], data));
        let mut edns = Edns::new();
        edns.set_max_payload(1232);
        edns.set_dnssec_ok(true);
        m.set_edns(edns);
        m.to_vec().unwrap()
    }

    /// The TTL of every record is lowered, in each of the three sections,
    /// to 0 at the least — and nothing else of the message is another: not
    /// the OPT record's TTL field, which holds its flags.
    #[test]
    fn ttls_are_lowered_in_every_section_and_nothing_else_changes() {
        let mut wire = sections([100, 20, 3600, 30]);
        lower_ttls(&mut wire, 30);
        assert_eq!(wire, sections([70, 0, 3570, 0]));
        let aged = Message::from_vec(&wire).unwrap();
        let ttls = |records: &[Record]| records.iter().map(|r| r.ttl).collect::<Vec<_>>();
        assert_eq!(ttls(&aged.answers), [70, 0]);
        assert_eq!(ttls(&aged.authorities), [3570]);
        assert_eq!(ttls(&aged.additionals), [0]);
        let edns = aged.edns.unwrap();
        assert!(edns.flags().dnssec_ok);
        assert_eq!(edns.max_payload(), 1232);
        // not by a second more than said, nor by any when none is
        let mut wire = sections([100, u32::MAX, 0, 1]);
        lower_ttls(&mut wire, 0);
        assert_eq!(wire, sections([100, u32::MAX, 0, 1]));
        lower_ttls(&mut wire, 1);
        assert_eq!(wire, sections([99, u32::MAX - 1, 0, 0]));
        lower_ttls(&mut wire, u32::MAX);
        assert_eq!(wire, sections([0; 4]));
    }

    /// One record on the wire: its name, type, class IN, TTL field and
    /// data.
    fn record(name: &[u8], rtype: u16, ttl: u32, data: &[u8]) -> Vec<u8> {
        let mut wire = name.to_vec();
        wire.extend(rtype.to_be_bytes());
        wire.extend([0, 1]);
        wire.extend(ttl.to_be_bytes());
        wire.extend((data.len() as u16).to_be_bytes());
        wire.extend(data);
        wire
    }

    /// A response written by hand, 121 bytes: the question `a.test`, then
    /// five records of the TTLs `ttls` — but for the fourth, an OPT record
    /// before the last of the additional ones, with an extended RCODE and
    /// the DO flag where the others have their TTL. Their names end in a
    /// pointer, in the root, or are the root; the second is of a type
    /// nobody knows, its data what looks like pointers.
    fn handwritten(ttls: [u32; 4]) -> Vec<u8> {
        [
            // one question, one answer, one authority, three additional
            vec![0x12, 0x34, 0x81, 0x80, 0, 1, 0, 1, 0, 1, 0, 3],
            // `a.test` at offset 12, A, IN
            vec![1, b'a', 4, b't', b'e', b's', b't', 0, 0, 1, 0, 1],
            record(&[0xc0, 12], 1, ttls[0], &[10, 0, 0, 1]),
            record(
                &[3, b'w', b'w', b'w', 0xc0, 12],
                0xff00,
                ttls[1],
                &[0xc0; 7],
            ),
            record(&[0], 2, ttls[2], &[]),
            record(&[0], 41, 0x0100_8000, &[0, 10, 0, 2, 7, 7]),
            record(&[1, b'b', 0xc0, 14], 28, ttls[3], &[0; 16]),
        ]
        .concat()
    }

    /// The walk is over the wire, whatever the records hold: a record of a
    /// type nobody knows is passed over by its length, and the OPT record
    /// keeps its field wherever it stands — among the answers and in the
    /// authority section, where it has no business, too. What follows the
    /// last record is none of the walk's business, and a message need not
    /// have a question.
    #[test]
    fn ttls_are_lowered_whatever_the_records_hold() {
        let mut wire = handwritten([300, 45, 60, 86_400]);
        assert_eq!(wire.len(), 121);
        lower_ttls(&mut wire, 60);
        assert_eq!(wire, handwritten([240, 0, 0, 86_340]));
        let mut trailed = [handwritten([300, 45, 60, 86_400]), vec![0xff; 3]].concat();
        lower_ttls(&mut trailed, 60);
        assert_eq!(
            trailed,
            [handwritten([240, 0, 0, 86_340]), vec![0xff; 3]].concat()
        );
        let misplaced = |ttls: [u32; 3]| {
            [
                // one question, two answers, two authority, one additional
                vec![0x12, 0x34, 0x81, 0x80, 0, 1, 0, 2, 0, 2, 0, 1],
                vec![1, b'a', 4, b't', b'e', b's', b't', 0, 0, 1, 0, 1],
                record(&[0xc0, 12], 1, ttls[0], &[10, 0, 0, 1]),
                // OPT, the last of the answers and the first of the
                // authority section: the DO flag, an extended RCODE
                record(&[0], 41, 0x0000_8000, &[]),
                record(&[0], 41, 0x0100_0000, &[0, 10, 0, 2, 7, 7]),
                record(&[0xc0, 14], 2, ttls[1], &[2, b'n', b's', 0xc0, 14]),
                record(&[0xc0, 12], 1, ttls[2], &[10, 0, 0, 2]),
            ]
            .concat()
        };
        let mut wire = misplaced([300, 45, 60]);
        lower_ttls(&mut wire, 60);
        assert_eq!(wire, misplaced([240, 0, 0]));
        let unasked = |ttls: [u32; 2]| {
            [
                // no question, one answer, one additional
                vec![0x12, 0x34, 0x81, 0x80, 0, 0, 0, 1, 0, 0, 0, 1],
                // `a.test` at offset 12
                record(
                    &[1, b'a', 4, b't', b'e', b's', b't', 0],
                    1,
                    ttls[0],
                    &[10, 0, 0, 1],
                ),
                record(&[0xc0, 12], 28, ttls[1], &[0; 16]),
            ]
            .concat()
        };
        let mut wire = unasked([300, 45]);
        lower_ttls(&mut wire, 60);
        assert_eq!(wire, unasked([240, 0]));
        let read = Message::from_vec(&wire).unwrap();
        assert!(read.queries.is_empty());
        assert_eq!((read.answers[0].ttl, read.additionals[0].ttl), (240, 0));
    }

    /// A message that does not read to its last record keeps every TTL it
    /// has, those before the damage too: what is wrong with it is for
    /// whoever reads it next to say.
    #[test]
    fn a_message_that_does_not_read_to_its_last_record_is_left_alone() {
        let unchanged = |damaged: Vec<u8>, what: &str| {
            let mut wire = damaged.clone();
            lower_ttls(&mut wire, 30);
            assert_eq!(wire, damaged, "{what}");
        };
        // cut short anywhere: in the header, in a name, in a record's data
        for whole in [
            sections([100, 20, 3600, 300]),
            handwritten([100, 20, 60, 300]),
        ] {
            for len in 0..whole.len() {
                unchanged(whole[..len].to_vec(), &format!("cut to {len} bytes"));
            }
        }
        let whole = handwritten([100, 20, 60, 300]);
        let damage = |at: usize, byte: u8| {
            let mut damaged = whole.clone();
            damaged[at] = byte;
            damaged
        };
        // ARCOUNT
        unchanged(damage(11, 4), "a record more announced than there is");
        // QDCOUNT: the first record is taken for a question, and what
        // follows it for records that run off the end
        unchanged(damage(5, 2), "a question more announced than there is");
        // of the question's name, of the last record's
        for at in [12, 91] {
            for label in [0x40, 0x80] {
                unchanged(damage(at, label), "a label of a type there is none of");
            }
        }
        // the last record's RDLENGTH
        unchanged(damage(104, 17), "data said longer than the message");
        // and the walk does read these to their end
        let mut wire = whole.clone();
        lower_ttls(&mut wire, 30);
        assert_eq!(wire, handwritten([70, 0, 30, 270]));
    }
}
