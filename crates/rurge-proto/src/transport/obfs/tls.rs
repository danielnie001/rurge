//! `obfs=tls`: the first client packet is a fake TLS 1.2 ClientHello with the
//! first payload in its session ticket; the server answers with a fake
//! ServerHello, a ChangeCipherSpec and a handshake record carrying its first
//! data; every later packet either way is an application data record (phase
//! 2 M6 design 3.2). The template is our own, written from the protocol
//! facts (M6-D3): nothing here is a real TLS handshake.

use std::time::{SystemTime, UNIX_EPOCH};

/// The largest record body either side accepts (the reference server
/// refuses a longer one).
pub(super) const MAX_RECORD: usize = 16 * 1024;
pub(super) const HEADER_LEN: usize = 5;

pub(super) const HANDSHAKE: u8 = 0x16;
pub(super) const CHANGE_CIPHER_SPEC: u8 = 0x14;
pub(super) const APPLICATION_DATA: u8 = 0x17;

/// The hello's size without the ticket and the host name.
pub(super) const HELLO_OVERHEAD: usize = 217;

/// What the reference server reads for the whole hello: the first payload
/// is cut to fit in it.
pub(super) fn max_first_payload(host: &str) -> usize {
    MAX_RECORD - HELLO_OVERHEAD - host.len()
}

/// 28 suites, the last one the renegotiation SCSV.
const CIPHER_SUITES: [u16; 28] = [
    0xc02c, 0xc030, 0x009f, 0xcca9, 0xcca8, 0xccaa, 0xc02b, 0xc02f, 0x009e, 0xc024, 0xc028, 0x006b,
    0xc023, 0xc027, 0x0067, 0xc00a, 0xc014, 0x0039, 0xc009, 0xc013, 0x0033, 0x009d, 0x009c, 0x003d,
    0x003c, 0x0035, 0x002f, 0x00ff,
];

/// ec_point_formats, supported_groups, signature_algorithms, encrypt_then_mac
/// and extended_master_secret: the extensions after the server name, fixed.
const TAIL_EXTENSIONS: [u8; 66] = [
    0x00, 0x0b, 0x00, 0x04, 0x03, 0x01, 0x00, 0x02, // ec_point_formats
    0x00, 0x0a, 0x00, 0x0a, 0x00, 0x08, 0x00, 0x1d, 0x00, 0x17, 0x00, 0x19, 0x00,
    0x18, // supported_groups: x25519, secp256r1, secp521r1, secp384r1
    0x00, 0x0d, 0x00, 0x20, 0x00, 0x1e, 0x06, 0x01, 0x06, 0x02, 0x06, 0x03, 0x05, 0x01, 0x05, 0x02,
    0x05, 0x03, 0x04, 0x01, 0x04, 0x02, 0x04, 0x03, 0x03, 0x01, 0x03, 0x02, 0x03, 0x03, 0x02, 0x01,
    0x02, 0x02, 0x02, 0x03, // signature_algorithms
    0x00, 0x16, 0x00, 0x00, // encrypt_then_mac
    0x00, 0x17, 0x00, 0x00, // extended_master_secret
];

fn put_u16(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&(n as u16).to_be_bytes());
}

/// The first packet: `ticket` is the first payload (at most
/// `max_first_payload(host)` bytes), `host` the server name.
pub(super) fn client_hello(host: &str, ticket: &[u8]) -> Vec<u8> {
    let total = HELLO_OVERHEAD + ticket.len() + host.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&[HANDSHAKE, 0x03, 0x01]);
    put_u16(&mut out, total - HEADER_LEN);
    // ClientHello, a 24-bit length whose top byte is always 0 here
    out.extend_from_slice(&[0x01, 0x00]);
    put_u16(&mut out, total - HEADER_LEN - 4);
    out.extend_from_slice(&[0x03, 0x03]);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or_default();
    out.extend_from_slice(&now.to_be_bytes());
    // 28 random bytes, then a 32-byte session id: camouflage, not secrets
    let mut random = [0u8; 28 + 32];
    let _ = getrandom::fill(&mut random);
    out.extend_from_slice(&random[..28]);
    out.push(32);
    out.extend_from_slice(&random[28..]);
    put_u16(&mut out, CIPHER_SUITES.len() * 2);
    for suite in CIPHER_SUITES {
        out.extend_from_slice(&suite.to_be_bytes());
    }
    // one compression method: null
    out.extend_from_slice(&[0x01, 0x00]);
    put_u16(&mut out, total - 138);
    // session_ticket
    out.extend_from_slice(&[0x00, 0x23]);
    put_u16(&mut out, ticket.len());
    out.extend_from_slice(ticket);
    // server_name: one host_name entry
    out.extend_from_slice(&[0x00, 0x00]);
    put_u16(&mut out, host.len() + 5);
    put_u16(&mut out, host.len() + 3);
    out.push(0x00);
    put_u16(&mut out, host.len());
    out.extend_from_slice(host.as_bytes());
    out.extend_from_slice(&TAIL_EXTENSIONS);
    debug_assert_eq!(out.len(), total);
    out
}

/// Appends one application data record carrying `data` (at most
/// `MAX_RECORD` bytes).
pub(super) fn app_data(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&[APPLICATION_DATA, 0x03, 0x03]);
    put_u16(out, data.len());
    out.extend_from_slice(data);
}

/// A record header the client accepts: the expected type, a TLS 1.x
/// version and a body of at most `MAX_RECORD` bytes. Returns the body length.
pub(super) fn record_len(header: &[u8; HEADER_LEN], kind: u8) -> Option<usize> {
    let len = usize::from(u16::from_be_bytes([header[3], header[4]]));
    (header[0] == kind
        && header[1] == 0x03
        && (0x01..=0x04).contains(&header[2])
        && len <= MAX_RECORD)
        .then_some(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(data: &[u8], at: usize) -> usize {
        usize::from(u16::from_be_bytes([data[at], data[at + 1]]))
    }

    #[test]
    fn the_hello_has_the_facts_layout() {
        let host = "cdn.example";
        let ticket: Vec<u8> = (0..300u32).map(|i| i as u8).collect();
        let hello = client_hello(host, &ticket);
        let (p, h) = (ticket.len(), host.len());
        assert_eq!(hello.len(), 217 + p + h);
        assert_eq!(&hello[..3], &[0x16, 0x03, 0x01]);
        assert_eq!(u16_at(&hello, 3), hello.len() - 5);
        assert_eq!(&hello[5..7], &[0x01, 0x00]);
        assert_eq!(u16_at(&hello, 7), hello.len() - 9);
        assert_eq!(&hello[9..11], &[0x03, 0x03]);
        assert_eq!(hello[43], 32, "a 32-byte session id");
        assert_eq!(u16_at(&hello, 76), 0x38);
        assert_eq!(&hello[78..80], &[0xc0, 0x2c]);
        assert_eq!(
            &hello[132..134],
            &[0x00, 0xff],
            "the SCSV is the last suite"
        );
        assert_eq!(&hello[134..136], &[0x01, 0x00]);
        assert_eq!(u16_at(&hello, 136), 79 + p + h);
        // session_ticket first, carrying the payload verbatim
        assert_eq!(&hello[138..140], &[0x00, 0x23]);
        assert_eq!(u16_at(&hello, 140), p);
        assert_eq!(&hello[142..142 + p], &ticket[..]);
        // then server_name
        let sni = 142 + p;
        assert_eq!(&hello[sni..sni + 2], &[0x00, 0x00]);
        assert_eq!(u16_at(&hello, sni + 2), h + 5);
        assert_eq!(u16_at(&hello, sni + 4), h + 3);
        assert_eq!(hello[sni + 6], 0);
        assert_eq!(u16_at(&hello, sni + 7), h);
        assert_eq!(&hello[sni + 9..sni + 9 + h], host.as_bytes());
        // then the fixed extensions, in order, and nothing after them
        let mut at = sni + 9 + h;
        for (kind, len) in [
            (0x000b, 4),
            (0x000a, 10),
            (0x000d, 32),
            (0x0016, 0),
            (0x0017, 0),
        ] {
            assert_eq!(u16_at(&hello, at), kind);
            assert_eq!(u16_at(&hello, at + 2), len);
            at += 4 + len;
        }
        assert_eq!(at, hello.len());
    }

    #[test]
    fn an_empty_ticket_and_the_largest_one_fit() {
        let hello = client_hello("h", &[]);
        assert_eq!(hello.len(), 218);
        assert_eq!(u16_at(&hello, 140), 0);
        let big = vec![7u8; max_first_payload("h")];
        assert_eq!(client_hello("h", &big).len(), MAX_RECORD);
    }

    #[test]
    fn records_are_checked_by_type_version_and_length() {
        let mut out = Vec::new();
        app_data(&mut out, b"abc");
        assert_eq!(out, [0x17, 0x03, 0x03, 0x00, 0x03, b'a', b'b', b'c']);
        let header = |b: [u8; 5]| b;
        assert_eq!(
            record_len(&header([0x17, 3, 3, 0x40, 0]), 0x17),
            Some(16384)
        );
        assert_eq!(record_len(&header([0x16, 3, 1, 0, 0x5b]), 0x16), Some(91));
        assert_eq!(record_len(&header([0x17, 3, 3, 0x40, 1]), 0x17), None);
        assert_eq!(record_len(&header([0x16, 3, 3, 0, 1]), 0x17), None);
        assert_eq!(record_len(&header([0x17, 2, 0, 0, 1]), 0x17), None);
    }
}
