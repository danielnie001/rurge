//! Reading a TLS ClientHello out of the handshake bytes that TLS records
//! carry (phase 4 M1 design 6.2).
//!
//! The two steps are kept apart: [`reassemble`] puts the payloads of the
//! handshake records at the start of a prefix together, and
//! [`parse_client_hello`] reads one whole handshake message on its own — which
//! is also what QUIC's CRYPTO frames hand it (phase 4 M5).

/// The handshake message type of a ClientHello.
const CLIENT_HELLO: u8 = 1;

/// The content type of a TLS handshake record.
const HANDSHAKE_RECORD: u8 = 0x16;

/// The most a record's payload may be (RFC 8446 5.1).
const MAX_RECORD: usize = 16384;

/// The bytes before a handshake message's body: its type and its 24-bit
/// length.
const HANDSHAKE_HEADER: usize = 4;

/// The most a `legacy_session_id` may be.
const MAX_SESSION_ID: usize = 32;

/// The extension type of `server_name` (RFC 6066).
const EXT_SERVER_NAME: u16 = 0;

/// The extension type of `application_layer_protocol_negotiation` (RFC 7301).
const EXT_ALPN: u16 = 16;

/// What the engine takes from a TLS ClientHello (phase 4 M1 design 6.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientHello {
    /// The first `host_name` of `server_name`, lowercased. `None` when the
    /// extension is not there, or its name is empty or not all ASCII.
    pub sni: Option<String>,
    /// Every protocol name of `application_layer_protocol_negotiation`, in
    /// the client's order; empty without the extension.
    pub alpn: Vec<Vec<u8>>,
}

/// Reads a ClientHello from one whole handshake message — its type (1), its
/// 24-bit length and its body — as TLS records carry it once they are put
/// together, and as QUIC's CRYPTO frames do (phase 4 M5). `None` when the
/// message is not a ClientHello or its lengths do not add up.
pub fn parse_client_hello(message: &[u8]) -> Option<ClientHello> {
    let mut reader = Reader::new(message);
    if reader.u8()? != CLIENT_HELLO {
        return None;
    }
    let len = reader.u24()?;
    if len + HANDSHAKE_HEADER != message.len() {
        return None;
    }
    let body = reader.take(len)?;
    read_client_hello(body)
}

/// A cursor over bytes the peer gave: every read is checked against what is
/// left, and a length the peer wrote is only ever a number to check against,
/// never an index to trust.
struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(rest: &'a [u8]) -> Reader<'a> {
        Reader { rest }
    }

    fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    fn u8(&mut self) -> Option<u8> {
        let (first, rest) = self.rest.split_first()?;
        self.rest = rest;
        Some(*first)
    }

    fn u16(&mut self) -> Option<u16> {
        let bytes: [u8; 2] = self.take(2)?.try_into().ok()?;
        Some(u16::from_be_bytes(bytes))
    }

    fn u24(&mut self) -> Option<usize> {
        let bytes = self.take(3)?;
        Some((usize::from(bytes[0]) << 16) | (usize::from(bytes[1]) << 8) | usize::from(bytes[2]))
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (taken, rest) = self.rest.split_at_checked(n)?;
        self.rest = rest;
        Some(taken)
    }
}

/// Reads the ClientHello body: the fixed fields, then the extensions (P17 of
/// the phase 4 / M1a plan).
fn read_client_hello(body: &[u8]) -> Option<ClientHello> {
    let mut reader = Reader::new(body);
    reader.take(2)?; // legacy_version
    reader.take(32)?; // random
    let session = reader.u8()? as usize;
    if session > MAX_SESSION_ID {
        return None;
    }
    reader.take(session)?; // legacy_session_id
    let suites = reader.u16()? as usize;
    if suites < 2 || suites % 2 != 0 {
        return None;
    }
    reader.take(suites)?; // cipher_suites
    let compression = reader.u8()? as usize;
    if compression < 1 {
        return None;
    }
    reader.take(compression)?; // legacy_compression_methods

    let mut hello = ClientHello::default();
    if reader.is_empty() {
        return Some(hello);
    }
    let extensions = reader.u16()? as usize;
    let block = reader.take(extensions)?;
    if !reader.is_empty() {
        return None;
    }
    read_extensions(block, &mut hello)?;
    Some(hello)
}

fn read_extensions(block: &[u8], hello: &mut ClientHello) -> Option<()> {
    let mut reader = Reader::new(block);
    let mut server_name = false;
    let mut alpn = false;
    while !reader.is_empty() {
        let kind = reader.u16()?;
        let len = reader.u16()? as usize;
        let body = reader.take(len)?;
        match kind {
            EXT_SERVER_NAME if !server_name => {
                server_name = true;
                hello.sni = read_server_name(body)?;
            }
            EXT_ALPN if !alpn => {
                alpn = true;
                hello.alpn = read_alpn(body)?;
            }
            // A second extension of the same kind is skipped: the first one
            // counts (P17 of the phase 4 / M1a plan).
            _ => {}
        }
    }
    Some(())
}

/// The first `host_name` of a `server_name` list; `Some(None)` when there is
/// none, or its name is empty or not all ASCII, and `None` when the lengths
/// do not add up.
fn read_server_name(body: &[u8]) -> Option<Option<String>> {
    let mut reader = Reader::new(body);
    let names = reader.u16()? as usize;
    let mut list = Reader::new(reader.take(names)?);
    if !reader.is_empty() {
        return None;
    }
    let mut name: Option<&[u8]> = None;
    let mut seen = false;
    while !list.is_empty() {
        let kind = list.u8()?;
        let len = list.u16()? as usize;
        let bytes = list.take(len)?;
        if kind == 0 && !seen {
            seen = true;
            name = Some(bytes);
        }
    }
    let Some(name) = name else {
        return Some(None);
    };
    if name.is_empty() || !name.is_ascii() {
        return Some(None);
    }
    Some(Some(std::str::from_utf8(name).ok()?.to_ascii_lowercase()))
}

/// Every protocol name of an `application_layer_protocol_negotiation` list.
fn read_alpn(body: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut reader = Reader::new(body);
    let names = reader.u16()? as usize;
    let mut list = Reader::new(reader.take(names)?);
    if !reader.is_empty() {
        return None;
    }
    let mut alpn = Vec::new();
    while !list.is_empty() {
        let len = list.u8()? as usize;
        if len == 0 {
            return None;
        }
        alpn.push(list.take(len)?.to_vec());
    }
    Some(alpn)
}

/// What putting the TLS records at the start of a prefix together came to.
pub(crate) enum Reassembled {
    /// The whole handshake message is there and it is a ClientHello.
    Hello(ClientHello),
    /// Telling more needs at least this many bytes in all.
    NeedMore(usize),
    /// Not a ClientHello carried by handshake records.
    Other,
}

/// Puts the handshake bytes that the TLS records at the start of `prefix`
/// carry together and reads the ClientHello out of them (phase 4 M1 design
/// 6.2). `limit` is the most a prefix is read for (`MAX_PREFIX` in
/// [`crate::sniff`]): a number of bytes above it is
/// [`Reassembled::Other`] rather than a wait that could never end.
pub(crate) fn reassemble(prefix: &[u8], limit: usize) -> Reassembled {
    let mut at = 0;
    let mut handshake = Vec::new();
    loop {
        let Some(rest) = prefix.get(at..) else {
            return Reassembled::Other;
        };
        let Some((header, payload)) = rest.split_at_checked(5) else {
            // Fewer than the five header bytes are there: what came must
            // still fit a handshake record, or this is not TLS.
            for (i, &byte) in rest.iter().enumerate() {
                let fits = match i {
                    0 => byte == HANDSHAKE_RECORD,
                    1 => byte == 0x03,
                    2 => byte <= 0x04,
                    _ => true,
                };
                if !fits {
                    return Reassembled::Other;
                }
            }
            return need(at + 5, limit);
        };
        // The header is five bytes, so these indices are in bounds.
        let kind = header[0];
        let major = header[1];
        let minor = header[2];
        let len = usize::from(u16::from_be_bytes([header[3], header[4]]));
        if kind != HANDSHAKE_RECORD
            || major != 0x03
            || minor > 0x04
            || !(1..=MAX_RECORD).contains(&len)
        {
            return Reassembled::Other;
        }

        let record_end = at + 5 + len;
        let before = handshake.len();
        let Some(bytes) = payload.get(..len.min(payload.len())) else {
            return Reassembled::Other;
        };
        handshake.extend_from_slice(bytes);

        let message_end = if handshake.len() >= HANDSHAKE_HEADER {
            // Four handshake bytes are there, so these indices are in bounds.
            if handshake[0] != CLIENT_HELLO {
                return Reassembled::Other;
            }
            let total = HANDSHAKE_HEADER
                + ((usize::from(handshake[1]) << 16)
                    | (usize::from(handshake[2]) << 8)
                    | usize::from(handshake[3]));
            if total > limit {
                return Reassembled::Other;
            }
            if handshake.len() >= total {
                let Some(message) = handshake.get(..total) else {
                    return Reassembled::Other;
                };
                return match parse_client_hello(message) {
                    Some(hello) => Reassembled::Hello(hello),
                    None => Reassembled::Other,
                };
            }
            at + 5 + (total - before)
        } else {
            at + 5 + (HANDSHAKE_HEADER - before)
        };
        if prefix.len() < record_end {
            return need(record_end.min(message_end), limit);
        }
        at = record_end;
    }
}

/// A wait for `n` bytes, unless a prefix is never read that far (P3 of the
/// phase 4 / M1a plan).
fn need(n: usize, limit: usize) -> Reassembled {
    if n > limit {
        Reassembled::Other
    } else {
        Reassembled::NeedMore(n)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use crate::sniff::{Judged, judge};

    use super::*;

    /// The TLS records rustls writes for a ClientHello for `name` with
    /// `alpn`, as a real client sends them (P18 of the phase 4 / M1a plan).
    pub(crate) fn hello_records(name: &str, alpn: &[&[u8]]) -> Vec<u8> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|name| name.to_vec()).collect();
        let name = rustls::pki_types::ServerName::try_from(name.to_owned()).expect("a server name");
        let mut conn = rustls::ClientConnection::new(Arc::new(config), name).expect("client");
        let mut bytes = Vec::new();
        conn.write_tls(&mut bytes).expect("a ClientHello");
        bytes
    }

    /// The handshake bytes that `records` carry, put together.
    pub(crate) fn handshake_bytes(records: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < records.len() {
            let len = usize::from(u16::from_be_bytes([records[at + 3], records[at + 4]]));
            out.extend_from_slice(&records[at + 5..at + 5 + len]);
            at += 5 + len;
        }
        out
    }

    /// `message` cut into handshake records with payloads of `lens` bytes,
    /// the rest in a last record.
    pub(crate) fn into_records(message: &[u8], lens: &[usize]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut rest = message;
        for &len in lens {
            let len = len.min(rest.len());
            let (payload, tail) = rest.split_at(len);
            push_record(&mut out, payload);
            rest = tail;
        }
        if !rest.is_empty() {
            push_record(&mut out, rest);
        }
        out
    }

    fn push_record(out: &mut Vec<u8>, payload: &[u8]) {
        out.extend_from_slice(&[HANDSHAKE_RECORD, 0x03, 0x01]);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend_from_slice(payload);
    }

    fn one_record(message: &[u8]) -> Vec<u8> {
        into_records(message, &[])
    }

    /// A hand-made ClientHello message with `extensions` after the fixed
    /// fields.
    fn hand_made_hello(extensions: &[Vec<u8>]) -> Vec<u8> {
        let mut block = Vec::new();
        for extension in extensions {
            block.extend_from_slice(extension);
        }
        message(&body(&[], &[0x13, 0x01], &[0x00], &block))
    }

    /// A handshake message: type 1, its 24-bit length and `body`.
    fn message(body: &[u8]) -> Vec<u8> {
        let mut out = vec![CLIENT_HELLO];
        out.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
        out.extend_from_slice(body);
        out
    }

    /// A ClientHello body of the fixed fields and `extensions`.
    fn body(session: &[u8], suites: &[u8], compression: &[u8], extensions: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x03, 0x03]);
        out.extend_from_slice(&[0x11; 32]);
        out.push(session.len() as u8);
        out.extend_from_slice(session);
        out.extend_from_slice(&(suites.len() as u16).to_be_bytes());
        out.extend_from_slice(suites);
        out.push(compression.len() as u8);
        out.extend_from_slice(compression);
        out.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        out.extend_from_slice(extensions);
        out
    }

    fn extension(kind: u16, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&kind.to_be_bytes());
        out.extend_from_slice(&(body.len() as u16).to_be_bytes());
        out.extend_from_slice(body);
        out
    }

    /// A `server_name` extension with one `host_name` entry.
    fn server_name(name: &[u8]) -> Vec<u8> {
        let mut list = Vec::new();
        list.push(0);
        list.extend_from_slice(&(name.len() as u16).to_be_bytes());
        list.extend_from_slice(name);
        let mut entry = Vec::new();
        entry.extend_from_slice(&(list.len() as u16).to_be_bytes());
        entry.extend_from_slice(&list);
        extension(EXT_SERVER_NAME, &entry)
    }

    fn at(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn a_whole_hello_is_tls_with_its_sni_and_alpn() {
        let records = hello_records("sni.test", &[b"h2", b"http/1.1"]);
        match judge(&records) {
            Judged::Tls(hello) => {
                assert_eq!(hello.sni.as_deref(), Some("sni.test"));
                assert_eq!(hello.alpn, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
            }
            other => panic!("expected TLS, got {other:?}"),
        }
    }

    #[test]
    fn every_cut_of_a_hello_needs_more() {
        let records = hello_records("sni.test", &[b"h2"]);
        for len in 0..records.len() {
            match judge(&records[..len]) {
                Judged::NeedMore(n) => {
                    assert!(n > len, "a cut of {len} bytes asked for {n}");
                    assert!(n <= records.len(), "a cut of {len} bytes asked for {n}");
                }
                other => panic!("a cut of {len} bytes said {other:?}"),
            }
        }
    }

    #[test]
    fn a_hello_over_several_records_is_put_together() {
        let records = hello_records("sni.test", &[b"h2", b"http/1.1"]);
        let message = handshake_bytes(&records);
        let cut = into_records(&message, &[1, 7, 100]);
        match judge(&cut) {
            Judged::Tls(hello) => {
                assert_eq!(hello.sni.as_deref(), Some("sni.test"));
                assert_eq!(hello.alpn, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
            }
            other => panic!("expected TLS, got {other:?}"),
        }
        for len in 0..cut.len() {
            match judge(&cut[..len]) {
                Judged::NeedMore(n) => {
                    assert!(n > len, "a cut of {len} bytes asked for {n}");
                    assert!(n <= cut.len(), "a cut of {len} bytes asked for {n}");
                }
                other => panic!("a cut of {len} bytes said {other:?}"),
            }
        }
    }

    #[test]
    fn a_hello_larger_than_a_record_is_read_to_its_end() {
        let padding = vec![0u8; 20 * 1024];
        let message = hand_made_hello(&[server_name(b"sni.test"), extension(21, &padding)]);
        assert!(
            message.len() > MAX_RECORD,
            "a hello of {} bytes",
            message.len()
        );
        let records = into_records(&message, &[MAX_RECORD]);
        match judge(&records) {
            Judged::Tls(hello) => assert_eq!(hello.sni.as_deref(), Some("sni.test")),
            other => panic!("expected TLS, got {other:?}"),
        }
    }

    #[test]
    fn an_upper_case_sni_is_lowered() {
        let mut records = hello_records("sni.test", &[b"h2"]);
        let start = at(&records, b"sni.test").expect("the name in the hello");
        records[start..start + 8].copy_from_slice(b"SNI.TEST");
        match judge(&records) {
            Judged::Tls(hello) => assert_eq!(hello.sni.as_deref(), Some("sni.test")),
            other => panic!("expected TLS, got {other:?}"),
        }
    }

    #[test]
    fn a_hello_without_a_name_has_no_sni() {
        let records = hello_records("127.0.0.1", &[b"h2"]);
        match judge(&records) {
            Judged::Tls(hello) => assert_eq!(hello.sni, None),
            other => panic!("expected TLS, got {other:?}"),
        }
    }

    #[test]
    fn a_name_that_is_not_ascii_is_no_sni() {
        let records = one_record(&hand_made_hello(&[server_name(b"sni.t\xc3\xa9st")]));
        match judge(&records) {
            Judged::Tls(hello) => assert_eq!(hello.sni, None),
            other => panic!("expected TLS, got {other:?}"),
        }
    }

    #[test]
    fn alpn_lists_are_read_whole() {
        type Case<'a> = (&'a [&'a [u8]], &'a [&'a [u8]]);
        let cases: &[Case<'_>] = &[
            (&[], &[]),
            (&[b"h2"], &[b"h2"]),
            (
                &[b"h2", b"http/1.1", b"spdy/3.1"],
                &[b"h2", b"http/1.1", b"spdy/3.1"],
            ),
        ];
        for (offered, want) in cases {
            let records = hello_records("sni.test", offered);
            match judge(&records) {
                Judged::Tls(hello) => {
                    let want: Vec<Vec<u8>> = want.iter().map(|name| name.to_vec()).collect();
                    assert_eq!(hello.alpn, want, "offered {offered:?}");
                }
                other => panic!("expected TLS, got {other:?}"),
            }
        }
    }

    #[test]
    fn lengths_that_do_not_add_up_are_other() {
        let extensions = server_name(b"sni.test");
        let good = message(&body(&[], &[0x13, 0x01], &[0x00], &extensions));

        // The extension block's length sits right before the block; 256 bytes
        // is more than what follows it.
        let block_len = good.len() - extensions.len() - 2;
        let mut long_block = good.clone();
        long_block[block_len] = 0x01;
        long_block[block_len + 1] = 0x00;

        // The session id may be at most 32 bytes.
        let mut long_session = good.clone();
        long_session[4 + 2 + 32] = 33;

        // Cipher suites come in pairs.
        let mut odd_suites = good.clone();
        odd_suites[4 + 2 + 32 + 1] = 0x00;
        odd_suites[4 + 2 + 32 + 2] = 0x03;

        // There is at least one compression method.
        let mut no_compression = good.clone();
        no_compression[4 + 2 + 32 + 1 + 2 + 2] = 0x00;

        // The `server_name` list says 20 bytes follow 13.
        let mut list = Vec::new();
        list.extend_from_slice(&[0x00, 0x14]);
        list.extend_from_slice(&[0x00, 0x00, 0x08]);
        list.extend_from_slice(b"sni.test");
        let short_list = one_record(&hand_made_hello(&[extension(EXT_SERVER_NAME, &list)]));

        let good_record = one_record(&good);
        let mut zero_length = good_record.clone();
        zero_length[3] = 0;
        zero_length[4] = 0;

        let mut long_record = good_record.clone();
        long_record[3] = 0x40;
        long_record[4] = 0x01;

        let mut other_version = good_record.clone();
        other_version[2] = 0x05;

        let mut other_handshake = good.clone();
        other_handshake[0] = 2;

        let cases = [
            ("a block longer than what is left", one_record(&long_block)),
            ("a session id of 33 bytes", one_record(&long_session)),
            ("an odd cipher suite length", one_record(&odd_suites)),
            ("no compression method", one_record(&no_compression)),
            ("a server name list of the wrong length", short_list),
            ("a record of length 0", zero_length),
            ("a record of 16385 bytes", long_record),
            ("a record version of 0x0305", other_version),
            ("a handshake type of 2", one_record(&other_handshake)),
        ];
        for (what, records) in cases {
            assert_eq!(judge(&records), Judged::Other, "{what}");
        }
    }

    #[test]
    fn no_damaged_hello_panics() {
        let records = hello_records("sni.test", &[b"h2", b"http/1.1"]);
        for i in 0..records.len() {
            for byte in [0x00, 0xff, records[i] ^ 0x80] {
                let mut damaged = records.clone();
                damaged[i] = byte;
                let _ = judge(&damaged);
            }
        }
    }
}
