//! The least of the DNS wire format the L2 tests need, written by hand.
//!
//! Only the standard library is used: this package takes no dependency of its
//! own beyond what the tests need, and the four functions here are a query's
//! encoding, the name and type a message asks about, the address in a
//! response, and a response to a query. Compression pointers are read
//! (a resolver's own answer may use them); nothing is written but a pointer to
//! the question's name, at offset 12.

use std::net::Ipv4Addr;

/// A query for `name`'s A record (`qtype` 1, `qclass` IN), with `id`.
///
/// `name` is unrooted text — `example.l2` — and each of its labels is 1 to 63
/// bytes; anything else is refused rather than put on the wire.
pub fn query(id: u16, name: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(&id.to_be_bytes());
    // recursion desired, one question, no answers, no authority, no additional
    out.extend_from_slice(&[0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    encode_name(&mut out, name)?;
    out.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]);
    Ok(out)
}

/// The name and type of `msg`'s first question (`None` when it has none, or is
/// not a message). The name is unrooted and keeps the case it was asked in.
pub fn name_and_type(msg: &[u8]) -> Option<(String, u16)> {
    if msg.len() < 12 || u16::from_be_bytes([msg[4], msg[5]]) == 0 {
        return None;
    }
    let (name, after) = read_name(msg, 12)?;
    if msg.len() < after + 4 {
        return None;
    }
    Some((name, u16::from_be_bytes([msg[after], msg[after + 1]])))
}

/// The first IPv4 address in `msg`'s answer section, if it has one.
pub fn answer(msg: &[u8]) -> Option<Ipv4Addr> {
    if msg.len() < 12 {
        return None;
    }
    let count = u16::from_be_bytes([msg[6], msg[7]]);
    // past the question: its name, its type and its class
    let (_, after) = read_name(msg, 12)?;
    let mut at = after + 4;
    for _ in 0..count {
        let (_, after) = read_name(msg, at)?;
        if msg.len() < after + 10 {
            return None;
        }
        let rtype = u16::from_be_bytes([msg[after], msg[after + 1]]);
        let rdlen = usize::from(u16::from_be_bytes([msg[after + 8], msg[after + 9]]));
        let start = after + 10;
        if msg.len() < start + rdlen {
            return None;
        }
        if rtype == 1 && rdlen == 4 {
            return Some(Ipv4Addr::new(
                msg[start],
                msg[start + 1],
                msg[start + 2],
                msg[start + 3],
            ));
        }
        at = start + rdlen;
    }
    None
}

/// A response to `query` that says `address` is what its name resolves to: the
/// query's id and question, one A record, and the recursion-available bit.
/// `None` when `query` has no complete question.
pub fn response(query: &[u8], address: Ipv4Addr) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }
    let (_, after) = read_name(query, 12)?;
    let end = after + 4;
    if query.len() < end {
        return None;
    }
    let mut out = Vec::with_capacity(end + 16);
    out.extend_from_slice(&query[..2]);
    // a response, recursion desired and available, no error, one question and
    // one answer
    out.extend_from_slice(&[0x81, 0x80, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&query[12..end]);
    // the answer's name is the question's, at offset 12
    out.extend_from_slice(&[0xc0, 0x0c, 0x00, 0x01, 0x00, 0x01]);
    out.extend_from_slice(&60u32.to_be_bytes());
    out.extend_from_slice(&[0x00, 0x04]);
    out.extend_from_slice(&address.octets());
    Some(out)
}

/// `name`'s labels and the root's empty one (RFC 1035 4.1.2), as they are
/// written in a query.
fn encode_name(out: &mut Vec<u8>, name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a name has at least one label".to_string());
    }
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(format!(
                "`{name}` is not a name: a label is 1 to 63 bytes, and `..` is no label"
            ));
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    Ok(())
}

/// The name starting at `at`, and where it ends (`at` itself after the root's
/// empty label, past the pointer after a compression pointer). A pointer may
/// point anywhere before it, as long as the jumps do not chase their tail.
fn read_name(msg: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut name = String::new();
    let mut after: Option<usize> = None;
    // a message of n bytes has no more than n labels in a name, so a name that
    // takes more steps than there are bytes is one that points at itself
    for _ in 0..msg.len() {
        let len = *msg.get(at)?;
        match len {
            0 => return Some((name, after.unwrap_or(at + 1))),
            // the two high bits set: a pointer to the name's rest, which is
            // two bytes wherever the name stops here
            0xc0..=0xff => {
                let low = usize::from(*msg.get(at + 1)?);
                if after.is_none() {
                    after = Some(at + 2);
                }
                at = ((usize::from(len & 0x3f)) << 8) | low;
            }
            // a label: its length, then its bytes; 0x40 to 0xbf are the two
            // extended label types, which nothing uses and this does not read
            1..=63 => {
                let len = usize::from(len);
                let start = at + 1;
                let label = msg.get(start..start + len)?;
                if !name.is_empty() {
                    name.push('.');
                }
                name.push_str(&String::from_utf8_lossy(label));
                at = start + len;
            }
            64..=191 => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `id` `0x1234`, `example.l2`, and nothing else: a query as it looks on
    /// the wire.
    #[test]
    fn a_query_is_the_name_and_the_address_type() {
        let mut want = vec![0x12, 0x34, 0x01, 0x00, 0x00, 0x01];
        want.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        want.push(7);
        want.extend_from_slice(b"example");
        want.push(2);
        want.extend_from_slice(b"l2");
        want.push(0);
        want.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]);
        assert_eq!(query(0x1234, "example.l2").unwrap(), want);
    }

    /// The name of a query comes back as it was asked, with its type.
    #[test]
    fn the_question_of_a_query_reads_back() {
        let msg = query(7, "example.l2").unwrap();
        assert_eq!(name_and_type(&msg), Some(("example.l2".to_string(), 1u16)));
    }

    /// A response this package did not write: the name is a compression
    /// pointer, the answer's name is the question's, and the address is
    /// `10.231.1.1`.
    #[test]
    fn a_handwritten_answer_has_its_address() {
        let msg: Vec<u8> = vec![
            0x42, 0x42, 0x81, 0x80, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, //
            7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 2, b'l', b'2', 0, //
            0x00, 0x01, 0x00, 0x01, // A, IN
            0xc0, 0x0c, // the question's name
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x04, //
            10, 231, 1, 1,
        ];
        assert_eq!(answer(&msg), Some(Ipv4Addr::new(10, 231, 1, 1)));
        assert_eq!(name_and_type(&msg), Some(("example.l2".to_string(), 1u16)));
    }

    /// What the server writes is what the client reads: the answer to a query
    /// this package made carries the address, and the same name.
    #[test]
    fn a_response_to_a_query_reads_back_as_the_address() {
        let asked = query(0x0303, "example.l2").unwrap();
        let address = Ipv4Addr::new(10, 231, 1, 1);
        let reply = response(&asked, address).unwrap();
        assert_eq!(answer(&reply), Some(address));
        assert_eq!(&reply[..2], &asked[..2]);
        assert_eq!(
            name_and_type(&reply),
            Some(("example.l2".to_string(), 1u16))
        );
    }

    /// A name that is not one is refused: empty, empty labels, a label longer
    /// than 63 bytes.
    #[test]
    fn a_name_that_is_not_one_is_refused() {
        for name in ["", "example..l2", ".example"] {
            assert!(query(1, name).is_err(), "{name:?} was accepted");
        }
        let long = "a".repeat(64);
        assert!(query(1, &format!("{long}.test")).is_err());
    }

    /// Nothing that is not a message reads as one.
    #[test]
    fn an_empty_or_cut_message_has_no_question() {
        assert_eq!(name_and_type(&[]), None);
        assert_eq!(answer(&[0x42, 0x42]), None);
        let cut = &query(1, "example.l2").unwrap()[..16];
        assert_eq!(name_and_type(cut), None);
    }
}
