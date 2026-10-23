//! Wire-format encoding and decoding on top of hickory-proto (design §7.2):
//! only what a stub resolver needs — A / AAAA questions and answers, EDNS
//! payload size, response codes and the TC bit. Transports never look inside
//! messages beyond `wire_id` / `wire_truncated`, DoQ's `doq_query` and DoH's
//! `lower_ttls`.
//!
//! What answers or forwards queries as they come (the virtual interface's DNS
//! responder, `Resolver::forward`) works on the bytes of any type of query,
//! without parsing a message as a whole: the header, the first question and
//! the OPT record are read where they stand (`wire_header`, `wire_question`,
//! `read_request`, `opt_size`), and an answer is written byte by byte
//! (`answer`), the question in it a copy of the query's.

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
    /// The two the virtual interface's responder writes itself: a query it
    /// cannot read, and one of a kind (an opcode, a class) it does not do.
    FormErr,
    NotImp,
    Other(u8),
}

impl Rcode {
    fn from_hickory(rc: ResponseCode) -> Rcode {
        match rc {
            ResponseCode::NoError => Rcode::NoError,
            ResponseCode::NXDomain => Rcode::NxDomain,
            ResponseCode::ServFail => Rcode::ServFail,
            ResponseCode::Refused => Rcode::Refused,
            ResponseCode::FormErr => Rcode::FormErr,
            ResponseCode::NotImp => Rcode::NotImp,
            other => Rcode::Other(other.low()),
        }
    }

    pub(crate) fn to_hickory(self) -> ResponseCode {
        match self {
            Rcode::NoError => ResponseCode::NoError,
            Rcode::NxDomain => ResponseCode::NXDomain,
            Rcode::ServFail => ResponseCode::ServFail,
            Rcode::Refused => ResponseCode::Refused,
            Rcode::FormErr => ResponseCode::FormErr,
            Rcode::NotImp => ResponseCode::NotImp,
            Rcode::Other(n) => ResponseCode::from_low(n),
        }
    }

    /// The four bits the header holds.
    fn code(self) -> u8 {
        self.to_hickory().low()
    }
}

impl fmt::Display for Rcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rcode::NoError => f.write_str("NOERROR"),
            Rcode::NxDomain => f.write_str("NXDOMAIN"),
            Rcode::ServFail => f.write_str("SERVFAIL"),
            Rcode::Refused => f.write_str("REFUSED"),
            Rcode::FormErr => f.write_str("FORMERR"),
            Rcode::NotImp => f.write_str("NOTIMP"),
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

// The flag bits of a header (RFC 1035 4.1.1): QR, AA, TC and RD in its third
// byte, RA in its fourth.
const QR: u8 = 0x80;
const AA: u8 = 0x04;
const TC: u8 = 0x02;
const RD: u8 = 0x01;
const RA: u8 = 0x80;

// The types of the records an answer made here has, and their class.
const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const CLASS_IN: u16 = 1;

/// What the header of a message says that whoever answers it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub id: u16,
    pub opcode: u8,
    pub recursion_desired: bool,
}

/// The header's fields, or `None` for fewer than 12 bytes.
pub fn wire_header(bytes: &[u8]) -> Option<Header> {
    (bytes.len() >= 12).then(|| Header {
        id: u16::from_be_bytes([bytes[0], bytes[1]]),
        opcode: (bytes[2] >> 3) & 0x0f,
        recursion_desired: bytes[2] & RD != 0,
    })
}

/// Writes `id` over the message's ID (a no-op on fewer than 2 bytes).
pub fn set_wire_id(bytes: &mut [u8], id: u16) {
    if let Some(field) = bytes.get_mut(..2) {
        field.copy_from_slice(&id.to_be_bytes());
    }
}

/// Whether the QR bit says that the message is a response; not for fewer than
/// 12 bytes.
pub(crate) fn wire_is_response(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes[2] & QR != 0
}

/// The header's response code.
pub fn wire_rcode(bytes: &[u8]) -> Option<Rcode> {
    (bytes.len() >= 12).then(|| Rcode::from_hickory(ResponseCode::from_low(bytes[3])))
}

/// The most a name takes on the wire: its length bytes, its labels and the
/// root (RFC 1035 3.1).
const MAX_NAME: usize = 255;

/// The first question of a message, read where it stands.
struct FirstQuestion {
    name: String,
    qtype: u16,
    qclass: u16,
    /// Where it ends: the offset after its class.
    end: usize,
}

/// The first question of `bytes`. `None` when the header announces none, or
/// when the question does not read to its end: a name that is cut short, a
/// label of a kind there is none of, a name of more than 255 bytes — or a name
/// that points elsewhere (RFC 1035 4.1.4). A question has nothing earlier to
/// point to, and a pointer would mean something else in a message that copies
/// the question to another place.
fn first_question(bytes: &[u8]) -> Option<FirstQuestion> {
    if wire_u16(bytes, 4)? == 0 {
        return None;
    }
    let mut name = String::new();
    let mut at = 12;
    loop {
        let len = usize::from(*bytes.get(at)?);
        at += 1;
        match len {
            0 => break,
            1..=0x3f => {
                let label = bytes.get(at..at + len)?;
                // a label is never empty: the name is empty only before the
                // first
                if !name.is_empty() {
                    name.push('.');
                }
                push_label(&mut name, label);
                at += len;
                // the root is still to come
                if at - 12 >= MAX_NAME {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(FirstQuestion {
        name,
        qtype: wire_u16(bytes, at)?,
        qclass: wire_u16(bytes, at + 2)?,
        end: at + 4,
    })
}

/// `label` as text, lowercased, added to `out`: a dot or a backslash in it
/// with a backslash before, a space or a byte that is no printable character
/// as a backslash and three decimal digits (RFC 1035 5.1) — told apart from
/// the dots between labels, and from the characters of a name that are one.
fn push_label(out: &mut String, label: &[u8]) {
    use std::fmt::Write as _;
    for &byte in label {
        match byte {
            b'.' | b'\\' => {
                out.push('\\');
                out.push(char::from(byte));
            }
            0..=0x20 | 0x7f..=0xff => {
                let _ = write!(out, "\\{byte:03}");
            }
            _ => out.push(char::from(byte.to_ascii_lowercase())),
        }
    }
}

/// The first question: its name lowercased, without the trailing dot, in
/// presentation form; its type; its class.
pub fn wire_question(bytes: &[u8]) -> Option<(String, u16, u16)> {
    first_question(bytes).map(|question| (question.name, question.qtype, question.qclass))
}

/// Where the TYPE field of each record of `wire` stands, the three sections
/// one after the other: walked as `ttl_offsets` walks them, and `None` for a
/// message that does not read to its last record either.
fn record_types(wire: &[u8]) -> Option<Vec<usize>> {
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
    let mut types = Vec::new();
    for _ in 0..records {
        // TYPE, CLASS, TTL and RDLENGTH follow the name, the data follows them
        at = name_end(wire, at)?;
        types.push(at);
        at += 10 + usize::from(wire_u16(wire, at + 8)?);
    }
    (at <= wire.len()).then_some(types)
}

/// Where the OPT record stands among the records `types` of `wire`: the
/// offset of its TYPE field, and its CLASS, which holds the size, two bytes
/// after.
fn opt_in(wire: &[u8], types: &[usize]) -> Option<usize> {
    types
        .iter()
        .copied()
        .find(|&at| wire_u16(wire, at) == Some(OPT))
}

/// The UDP payload size the message's OPT record announces, if it has one.
pub fn opt_size(bytes: &[u8]) -> Option<u16> {
    let at = opt_in(bytes, &record_types(bytes)?)?;
    wire_u16(bytes, at + 2)
}

/// Lowers the OPT record's UDP payload size to `max` in place when it is
/// larger; nothing else in the message changes (no other byte moves).
pub fn cap_udp_size(bytes: &mut [u8], max: u16) {
    let Some(at) = record_types(bytes).and_then(|types| opt_in(bytes, &types)) else {
        return;
    };
    if wire_u16(bytes, at + 2).is_some_and(|size| size > max) {
        bytes[at + 2..at + 4].copy_from_slice(&max.to_be_bytes());
    }
}

/// The UDP payload size the answers made here announce.
pub const ANSWER_UDP_SIZE: u16 = 1232;

/// A query read by `read_request`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u16,
    pub recursion_desired: bool,
    /// Lowercased, without the trailing dot (as `wire_question` gives it).
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
    /// The OPT record's UDP payload size, if the query has one.
    pub udp_size: Option<u16>,
    /// The question section exactly as it came, echoed in answers.
    question: Vec<u8>,
}

/// What `read_request` makes of a message that is no query to answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// Fewer than 12 bytes, or QR is set: not answered at all.
    NotAQuery,
    /// An opcode other than QUERY (0): NOTIMP, header only.
    NotImplemented(Header),
    /// Does not parse, or not exactly one question: FORMERR, header only.
    Malformed(Header),
}

/// The query in `bytes`, if it is one to answer. Checked in this order: fewer
/// than 12 bytes, or QR set: `NotAQuery`; an opcode other than 0:
/// `NotImplemented`, whatever follows the header; not exactly one question,
/// or a message that does not read to its last record: `Malformed`. The class
/// of the question is not looked at.
pub fn read_request(bytes: &[u8]) -> Result<Request, RequestError> {
    let header = match wire_header(bytes) {
        Some(header) if !wire_is_response(bytes) => header,
        _ => return Err(RequestError::NotAQuery),
    };
    if header.opcode != 0 {
        return Err(RequestError::NotImplemented(header));
    }
    let malformed = RequestError::Malformed(header);
    if wire_u16(bytes, 4) != Some(1) {
        return Err(malformed);
    }
    let question = first_question(bytes).ok_or(malformed)?;
    let types = record_types(bytes).ok_or(malformed)?;
    Ok(Request {
        id: header.id,
        recursion_desired: header.recursion_desired,
        name: question.name,
        qtype: question.qtype,
        qclass: question.qclass,
        udp_size: opt_in(bytes, &types).and_then(|at| wire_u16(bytes, at + 2)),
        question: bytes[12..question.end].to_vec(),
    })
}

/// An OPT record that announces `ANSWER_UDP_SIZE`, with no flags and no
/// options: the root name, the type, the size where the class goes, the
/// TTL field and an empty data.
fn opt_record() -> [u8; 11] {
    let size = ANSWER_UDP_SIZE.to_be_bytes();
    [0, 0, 41, size[0], size[1], 0, 0, 0, 0, 0, 0]
}

/// The answer to `req`: QR, RA and `flags` set, RD as the request had it, the
/// code `rcode`, the question as it came, the `count` records in `records`,
/// and the OPT record when the request had one.
fn write_reply(req: &Request, flags: u8, rcode: Rcode, count: u16, records: &[u8]) -> Vec<u8> {
    let opt = req.udp_size.is_some();
    let mut out = Vec::with_capacity(12 + req.question.len() + records.len() + 11);
    out.extend_from_slice(&req.id.to_be_bytes());
    out.push(QR | flags | if req.recursion_desired { RD } else { 0 });
    out.push(RA | rcode.code());
    // QDCOUNT, ANCOUNT, NSCOUNT and ARCOUNT
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&u16::from(opt).to_be_bytes());
    out.extend_from_slice(&req.question);
    out.extend_from_slice(records);
    if opt {
        out.extend_from_slice(&opt_record());
    }
    out
}

/// An answer built here: QR, AA and RA set, RD copied, the question echoed
/// byte for byte, `addrs` as A records for an A question and AAAA records
/// for an AAAA question (an address of the other family is skipped; for any
/// other type none is written), each with `ttl`; an OPT with payload size
/// `ANSWER_UDP_SIZE` when the request had one.
pub fn answer(req: &Request, rcode: Rcode, addrs: &[IpAddr], ttl: u32) -> Vec<u8> {
    let mut records = Vec::new();
    let mut count = 0u16;
    for addr in addrs {
        match (req.qtype, addr) {
            (TYPE_A, IpAddr::V4(a)) => push_record(&mut records, TYPE_A, ttl, &a.octets()),
            (TYPE_AAAA, IpAddr::V6(a)) => push_record(&mut records, TYPE_AAAA, ttl, &a.octets()),
            _ => continue,
        }
        count += 1;
        if count == u16::MAX {
            break;
        }
    }
    write_reply(req, AA, rcode, count, &records)
}

/// One record of class IN added to `out`. Its name is the question's, which
/// stands at the twelfth byte: a pointer to it.
fn push_record(out: &mut Vec<u8>, rtype: u16, ttl: u32, data: &[u8]) {
    out.extend_from_slice(&[0xc0, 0x0c]);
    out.extend_from_slice(&rtype.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    out.extend_from_slice(&ttl.to_be_bytes());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
}

/// TC set, no records, `rcode`, the question echoed, AA not set, RA set,
/// the OPT as `answer` writes it.
pub fn answer_truncated(req: &Request, rcode: Rcode) -> Vec<u8> {
    write_reply(req, TC, rcode, 0, &[])
}

/// Twelve bytes: the ID, QR, the opcode and RD copied, RA set, `rcode`, all
/// counts zero.
pub fn answer_header_only(h: &Header, rcode: Rcode) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.extend_from_slice(&h.id.to_be_bytes());
    out.push(QR | ((h.opcode & 0x0f) << 3) | if h.recursion_desired { RD } else { 0 });
    out.push(RA | rcode.code());
    out.extend_from_slice(&[0; 8]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::query_wire;
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

    fn addr(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// A query of the type A and class IN for a name written label by label,
    /// each of whatever bytes: RD set, no OPT record.
    fn raw_query(labels: &[&[u8]]) -> Vec<u8> {
        let mut wire = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        for label in labels {
            wire.push(u8::try_from(label.len()).unwrap());
            wire.extend_from_slice(label);
        }
        wire.extend_from_slice(&[0, 0, 1, 0, 1]);
        wire
    }

    #[test]
    fn wire_header_reads_the_flags_and_set_wire_id_writes_the_id() {
        assert_eq!(wire_header(&[0; 11]), None);
        let mut wire = query_wire(0xabcd, "a.test", 1, None);
        assert_eq!(
            wire_header(&wire),
            Some(Header {
                id: 0xabcd,
                opcode: 0,
                recursion_desired: true
            })
        );
        // opcode 3, QR set, RD off
        wire[2] = 0x80 | (3 << 3);
        assert_eq!(
            wire_header(&wire),
            Some(Header {
                id: 0xabcd,
                opcode: 3,
                recursion_desired: false
            })
        );
        set_wire_id(&mut wire, 0x0102);
        assert_eq!(wire_id(&wire), Some(0x0102));
        let mut two = [0u8; 2];
        set_wire_id(&mut two, 0x0102);
        assert_eq!(two, [1, 2]);
        let mut one = [9u8];
        set_wire_id(&mut one, 0x0102);
        assert_eq!(one, [9]);
        set_wire_id(&mut [], 0x0102);
    }

    #[test]
    fn read_request_tells_the_three_refusals_apart() {
        let good = query_wire(0x1234, "a.test", 1, None);
        let asked = read_request(&good).unwrap();
        assert_eq!(
            (asked.id, asked.recursion_desired, asked.name.as_str()),
            (0x1234, true, "a.test")
        );
        assert_eq!((asked.qtype, asked.qclass, asked.udp_size), (1, 1, None));

        // no header, or a response: not answered at all
        assert_eq!(read_request(&good[..11]), Err(RequestError::NotAQuery));
        assert_eq!(read_request(&[]), Err(RequestError::NotAQuery));
        let mut response = good.clone();
        response[2] |= 0x80;
        assert_eq!(read_request(&response), Err(RequestError::NotAQuery));

        // another opcode: NOTIMP, said from the header alone — the rest of
        // the message need not read
        let header = Header {
            id: 0x1234,
            opcode: 2,
            recursion_desired: true,
        };
        let mut status = good.clone();
        status[2] |= 2 << 3;
        assert_eq!(
            read_request(&status),
            Err(RequestError::NotImplemented(header))
        );
        assert_eq!(
            read_request(&status[..12]),
            Err(RequestError::NotImplemented(header))
        );
        status[2] &= !0x01;
        assert_eq!(
            read_request(&status),
            Err(RequestError::NotImplemented(Header {
                recursion_desired: false,
                ..header
            }))
        );

        // not exactly one question, or one that does not read: FORMERR
        let malformed = Err(RequestError::Malformed(Header {
            id: 0x1234,
            opcode: 0,
            recursion_desired: true,
        }));
        for questions in [0u8, 2] {
            let mut wrong = good.clone();
            wrong[5] = questions;
            assert_eq!(read_request(&wrong), malformed, "{questions} questions");
        }
        for cut in 12..good.len() {
            assert_eq!(read_request(&good[..cut]), malformed, "cut to {cut} bytes");
        }
        // nor a message whose record does not read: an OPT record cut short
        let with_opt = query_wire(0x1234, "a.test", 1, Some(1232));
        for cut in good.len()..with_opt.len() {
            assert_eq!(
                read_request(&with_opt[..cut]),
                malformed,
                "cut to {cut} bytes"
            );
        }
        assert!(read_request(&with_opt).is_ok());
        // a name that points elsewhere could not be echoed where it stands,
        // and one longer than a name can be is none
        let mut pointer = good[..12].to_vec();
        pointer.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1]);
        assert_eq!(read_request(&pointer), malformed);
        let long = vec!["a".repeat(63); 5].join(".");
        assert_eq!(read_request(&query_wire(0x1234, &long, 1, None)), malformed);
    }

    #[test]
    fn answer_echoes_the_question_byte_for_byte() {
        let wire = query_wire(7, "ExAmPle.COM", 1, Some(4096));
        let request = read_request(&wire).unwrap();
        assert_eq!(request.name, "example.com");
        // the question stands between the header and the OPT record: the name
        // (13 bytes), the type and the class
        let question = &wire[12..wire.len() - 11];
        assert_eq!(question.len(), 13 + 4);
        for reply in [
            answer(&request, Rcode::NoError, &[addr("192.0.2.1")], 30),
            answer_truncated(&request, Rcode::NoError),
        ] {
            assert_eq!(&reply[12..12 + question.len()], question);
            assert_eq!(
                wire_question(&reply),
                Some(("example.com".to_string(), 1, 1))
            );
        }
        // whatever bytes the labels hold are echoed as they came
        let wire = raw_query(&[b"a.b", b"\xe9 \\"]);
        let request = read_request(&wire).unwrap();
        let reply = answer(&request, Rcode::NxDomain, &[], 0);
        assert_eq!(&reply[12..12 + wire.len() - 12], &wire[12..]);
    }

    #[test]
    fn answer_writes_only_the_family_asked() {
        let addrs = [addr("192.0.2.1"), addr("2001:db8::1"), addr("192.0.2.2")];
        let records = |qtype: u16| {
            let request = read_request(&query_wire(1, "a.test", qtype, None)).unwrap();
            let reply = answer(&request, Rcode::NoError, &addrs, 30);
            let message = Message::from_vec(&reply).unwrap();
            assert_eq!(message.queries.len(), 1);
            message
                .answers
                .iter()
                .map(|r| {
                    assert_eq!((r.name.to_ascii(), r.ttl), ("a.test.".to_string(), 30));
                    r.data.clone()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            records(1),
            [
                RData::A(A(Ipv4Addr::new(192, 0, 2, 1))),
                RData::A(A(Ipv4Addr::new(192, 0, 2, 2)))
            ]
        );
        assert_eq!(
            records(28),
            [RData::AAAA(AAAA("2001:db8::1".parse().unwrap()))]
        );
        assert!(records(16).is_empty(), "no TXT record is made up");
    }

    #[test]
    fn answer_sets_aa_and_ra_and_copies_rd() {
        let ask = |rd: bool| {
            let mut wire = query_wire(1, "a.test", 1, None);
            if !rd {
                wire[2] &= !0x01;
            }
            read_request(&wire).unwrap()
        };
        for (rd, flags) in [(true, 0x85u8), (false, 0x84)] {
            // QR, AA and RD (when the query had it); RA and the code
            let reply = answer(&ask(rd), Rcode::NoError, &[], 0);
            assert_eq!((reply[2], reply[3]), (flags, 0x80));
            let message = Message::from_vec(&reply).unwrap();
            assert_eq!(message.id, 1);
            assert_eq!(message.message_type, MessageType::Response);
            assert!(message.metadata.authoritative && message.metadata.recursion_available);
            assert_eq!(message.metadata.recursion_desired, rd);
            assert!(!message.metadata.truncation);
        }
        for (rcode, code) in [
            (Rcode::FormErr, 0x81u8),
            (Rcode::ServFail, 0x82),
            (Rcode::NxDomain, 0x83),
            (Rcode::NotImp, 0x84),
            (Rcode::Refused, 0x85),
        ] {
            assert_eq!(answer(&ask(true), rcode, &[], 0)[3], code, "{rcode}");
        }
    }

    #[test]
    fn answer_has_an_opt_only_when_the_request_had_one() {
        assert_eq!(ANSWER_UDP_SIZE, 1232);
        let with = read_request(&query_wire(1, "a.test", 1, Some(4096))).unwrap();
        let without = read_request(&query_wire(1, "a.test", 1, None)).unwrap();
        for (request, size) in [(&with, Some(1232)), (&without, None)] {
            for reply in [
                answer(request, Rcode::NoError, &[addr("192.0.2.1")], 30),
                answer_truncated(request, Rcode::NoError),
            ] {
                assert_eq!(opt_size(&reply), size);
                let message = Message::from_vec(&reply).unwrap();
                assert_eq!(message.edns.map(|edns| edns.max_payload()), size);
            }
        }
    }

    #[test]
    fn answer_truncated_has_tc_and_no_records() {
        let request = read_request(&query_wire(5, "big.test", 16, Some(512))).unwrap();
        let reply = answer_truncated(&request, Rcode::ServFail);
        // QR, TC and RD; RA and the code — and not AA
        assert_eq!((reply[2], reply[3]), (0x83, 0x82));
        assert!(wire_truncated(&reply));
        let message = Message::from_vec(&reply).unwrap();
        assert_eq!(message.id, 5);
        assert!(message.answers.is_empty() && message.authorities.is_empty());
        assert!(!message.metadata.authoritative && message.metadata.recursion_available);
        assert_eq!(wire_question(&reply), Some(("big.test".to_string(), 16, 1)));
        assert_eq!(wire_rcode(&reply), Some(Rcode::ServFail));
    }

    #[test]
    fn answer_header_only_is_twelve_bytes() {
        let header = Header {
            id: 0xbeef,
            opcode: 2,
            recursion_desired: true,
        };
        // the ID; QR, the opcode and RD; RA and the code; four counts of 0
        assert_eq!(
            answer_header_only(&header, Rcode::NotImp),
            [0xbe, 0xef, 0x91, 0x84, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        let header = Header {
            recursion_desired: false,
            opcode: 0,
            ..header
        };
        assert_eq!(
            answer_header_only(&header, Rcode::FormErr),
            [0xbe, 0xef, 0x80, 0x81, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn cap_udp_size_only_lowers() {
        let query = query_wire(9, "a.test", 1, Some(4096));
        // the OPT record is the last 11 bytes, its class (the size) the two
        // after its name and type
        let at = query.len() - 8;
        assert_eq!(opt_size(&query), Some(4096));
        let mut capped = query.clone();
        cap_udp_size(&mut capped, 1232);
        assert_eq!(opt_size(&capped), Some(1232));
        assert_eq!(capped.len(), query.len());
        assert_eq!(capped[..at], query[..at]);
        assert_eq!(capped[at + 2..], query[at + 2..]);
        // what is not larger stays: smaller, equal, and the cap below it
        for size in [512, 1232] {
            let query = query_wire(9, "a.test", 1, Some(size));
            let mut same = query.clone();
            cap_udp_size(&mut same, 1232);
            assert_eq!(same, query);
        }
        let mut lowered = query_wire(9, "a.test", 1, Some(1232));
        cap_udp_size(&mut lowered, 512);
        assert_eq!(opt_size(&lowered), Some(512));
        // no OPT record: nothing to lower, and none made
        let bare = query_wire(9, "a.test", 1, None);
        let mut same = bare.clone();
        cap_udp_size(&mut same, 512);
        assert_eq!(same, bare);
        assert_eq!(opt_size(&bare), None);
        // a message that does not read to its last record is left alone
        let mut cut = query[..query.len() - 1].to_vec();
        cap_udp_size(&mut cut, 512);
        assert_eq!(cut, query[..query.len() - 1]);
        // the OPT record among others, in a response
        let response = sections([100, 20, 3600, 30]);
        assert_eq!(opt_size(&response), Some(1232));
        let mut capped = response.clone();
        cap_udp_size(&mut capped, 512);
        assert_eq!(opt_size(&capped), Some(512));
        let changed: Vec<usize> = (0..response.len())
            .filter(|&i| response[i] != capped[i])
            .collect();
        assert_eq!(changed.len(), 2, "{changed:?}");
        assert_eq!(changed[1], changed[0] + 1);
    }

    #[test]
    fn wire_question_lowercases_and_drops_the_dot() {
        let ask = |name: &str, qtype: u16| wire_question(&query_wire(1, name, qtype, None));
        assert_eq!(
            ask("WWW.Example.COM.", 28),
            Some(("www.example.com".to_string(), 28, 1))
        );
        assert_eq!(ask("a.test", 1), Some(("a.test".to_string(), 1, 1)));
        // the root has no labels
        assert_eq!(ask(".", 2), Some((String::new(), 2, 1)));
        // presentation form: a dot inside a label, a backslash, a space and a
        // byte that is no character are told apart from the dots between
        // labels
        let wire = raw_query(&[b"A.b", b"x\\y", b"\xe9 z"]);
        assert_eq!(
            wire_question(&wire),
            Some(("a\\.b.x\\\\y.\\233\\032z".to_string(), 1, 1))
        );
        // a reply's question reads the same, and the first of several is the one
        let mut two = query_wire(1, "a.test", 1, None);
        two[2] |= 0x80;
        two[5] = 2;
        two.extend_from_slice(&[1, b'b', 0, 0, 1, 0, 1]);
        assert_eq!(wire_question(&two), Some(("a.test".to_string(), 1, 1)));
        // no question, one that does not read, a name that points elsewhere
        let good = query_wire(1, "a.test", 1, None);
        assert_eq!(wire_question(&good[..11]), None);
        let mut none = good.clone();
        none[5] = 0;
        assert_eq!(wire_question(&none), None);
        for cut in 12..good.len() {
            assert_eq!(wire_question(&good[..cut]), None, "cut to {cut} bytes");
        }
        let mut pointer = good[..12].to_vec();
        pointer.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1]);
        assert_eq!(wire_question(&pointer), None);
    }

    #[test]
    fn formerr_and_notimp_round_trip() {
        for (rcode, text, hickory, code) in [
            (Rcode::FormErr, "FORMERR", ResponseCode::FormErr, 1u8),
            (Rcode::NotImp, "NOTIMP", ResponseCode::NotImp, 4),
        ] {
            assert_eq!(rcode.to_string(), text);
            assert_eq!(rcode.to_hickory(), hickory);
            assert_eq!(Rcode::from_hickory(hickory), rcode);
            // through a message as hickory writes it, and as it reads
            let wire = build_response(1, &q("a.test", Qtype::A), rcode, &[], false).unwrap();
            assert_eq!(wire[3] & 0x0f, code);
            assert_eq!(parse_response(&wire).unwrap().rcode, rcode);
            assert_eq!(wire_rcode(&wire), Some(rcode));
        }
        // every code the header holds
        for (code, rcode) in [
            (0u8, Rcode::NoError),
            (1, Rcode::FormErr),
            (2, Rcode::ServFail),
            (3, Rcode::NxDomain),
            (4, Rcode::NotImp),
            (5, Rcode::Refused),
            (6, Rcode::Other(6)),
            (15, Rcode::Other(15)),
        ] {
            let mut wire = [0u8; 12];
            // the bits above the code are other flags
            wire[3] = 0x80 | code;
            assert_eq!(wire_rcode(&wire), Some(rcode));
        }
        assert_eq!(wire_rcode(&[0; 11]), None);
    }
}
