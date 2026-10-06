//! Wire-format encoding and decoding on top of hickory-proto (design §7.2):
//! only what a stub resolver needs — A / AAAA questions and answers, EDNS
//! payload size, response codes and the TC bit. Transports never look inside
//! messages beyond `wire_id` / `wire_truncated`, and DoQ's `doq_query`.

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
}
