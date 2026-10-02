//! QPACK with the static table alone (RFC 9204): field sections as this
//! client sends them and as it takes them. Our SETTINGS give the server's
//! encoder no dynamic table, so a field section that needs one — a
//! Required Insert Count, a reference to an entry — cannot be decoded.

use super::frame::VARINT_MAX;
use super::huffman;

/// The static table (RFC 9204 Appendix A).
pub(crate) const STATIC_TABLE: [(&str, &str); 99] = [
    (":authority", ""),
    (":path", "/"),
    ("age", "0"),
    ("content-disposition", ""),
    ("content-length", "0"),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("referer", ""),
    ("set-cookie", ""),
    (":method", "CONNECT"),
    (":method", "DELETE"),
    (":method", "GET"),
    (":method", "HEAD"),
    (":method", "OPTIONS"),
    (":method", "POST"),
    (":method", "PUT"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "103"),
    (":status", "200"),
    (":status", "304"),
    (":status", "404"),
    (":status", "503"),
    ("accept", "*/*"),
    ("accept", "application/dns-message"),
    ("accept-encoding", "gzip, deflate, br"),
    ("accept-ranges", "bytes"),
    ("access-control-allow-headers", "cache-control"),
    ("access-control-allow-headers", "content-type"),
    ("access-control-allow-origin", "*"),
    ("cache-control", "max-age=0"),
    ("cache-control", "max-age=2592000"),
    ("cache-control", "max-age=604800"),
    ("cache-control", "no-cache"),
    ("cache-control", "no-store"),
    ("cache-control", "public, max-age=31536000"),
    ("content-encoding", "br"),
    ("content-encoding", "gzip"),
    ("content-type", "application/dns-message"),
    ("content-type", "application/javascript"),
    ("content-type", "application/json"),
    ("content-type", "application/x-www-form-urlencoded"),
    ("content-type", "image/gif"),
    ("content-type", "image/jpeg"),
    ("content-type", "image/png"),
    ("content-type", "text/css"),
    ("content-type", "text/html; charset=utf-8"),
    ("content-type", "text/plain"),
    ("content-type", "text/plain;charset=utf-8"),
    ("range", "bytes=0-"),
    ("strict-transport-security", "max-age=31536000"),
    (
        "strict-transport-security",
        "max-age=31536000; includesubdomains",
    ),
    (
        "strict-transport-security",
        "max-age=31536000; includesubdomains; preload",
    ),
    ("vary", "accept-encoding"),
    ("vary", "origin"),
    ("x-content-type-options", "nosniff"),
    ("x-xss-protection", "1; mode=block"),
    (":status", "100"),
    (":status", "204"),
    (":status", "206"),
    (":status", "302"),
    (":status", "400"),
    (":status", "403"),
    (":status", "421"),
    (":status", "425"),
    (":status", "500"),
    ("accept-language", ""),
    ("access-control-allow-credentials", "FALSE"),
    ("access-control-allow-credentials", "TRUE"),
    ("access-control-allow-headers", "*"),
    ("access-control-allow-methods", "get"),
    ("access-control-allow-methods", "get, post, options"),
    ("access-control-allow-methods", "options"),
    ("access-control-expose-headers", "content-length"),
    ("access-control-request-headers", "content-type"),
    ("access-control-request-method", "get"),
    ("access-control-request-method", "post"),
    ("alt-svc", "clear"),
    ("authorization", ""),
    (
        "content-security-policy",
        "script-src 'none'; object-src 'none'; base-uri 'none'",
    ),
    ("early-data", "1"),
    ("expect-ct", ""),
    ("forwarded", ""),
    ("if-range", ""),
    ("origin", ""),
    ("purpose", "prefetch"),
    ("server", ""),
    ("timing-allow-origin", "*"),
    ("upgrade-insecure-requests", "1"),
    ("user-agent", ""),
    ("x-forwarded-for", ""),
    ("x-frame-options", "deny"),
    ("x-frame-options", "sameorigin"),
];

/// A field line: its name and its value.
pub(crate) type Field = (Vec<u8>, Vec<u8>);

/// Why a field section was not taken.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// It needs a dynamic table, refers past the static table, or is cut
    /// short, too long or badly Huffman-encoded somewhere:
    /// QPACK_DECOMPRESSION_FAILED, an error of the connection (RFC 9204
    /// 4.5).
    Invalid,
    /// Its size is over the limit (RFC 9114 4.2.2).
    TooLarge,
}

/// The fields of `section`, whose size — the names and values plus 32
/// bytes per field — must not pass `limit`.
pub(crate) fn decode(mut section: &[u8], limit: u64) -> Result<Vec<Field>, Refused> {
    use Refused::Invalid;
    // without a dynamic table the Required Insert Count is 0 (RFC 9204
    // 4.5.1.1); a Sign bit of 1 would then make the Base negative
    // (4.5.1.2); any other Base serves static references alike
    let (_, required) = get_int(&mut section, 8)?;
    let (sign, _) = get_int(&mut section, 7)?;
    if required != 0 || sign != 0 {
        return Err(Invalid);
    }
    let mut fields = Vec::new();
    let mut size = 0u64;
    while let Some(&first) = section.first() {
        let field = if first & 0x80 != 0 {
            // indexed field line: 1 T index(6+), T = 1 for the static table
            let (flags, index) = get_int(&mut section, 6)?;
            if flags & 0x40 == 0 {
                return Err(Invalid);
            }
            let (name, value) = entry(index)?;
            (name.as_bytes().to_vec(), value.as_bytes().to_vec())
        } else if first & 0x40 != 0 {
            // literal field line with name reference: 01 N T index(4+)
            let (flags, index) = get_int(&mut section, 4)?;
            if flags & 0x10 == 0 {
                return Err(Invalid);
            }
            let (name, _) = entry(index)?;
            (name.as_bytes().to_vec(), get_string(&mut section, 7)?)
        } else if first & 0x20 != 0 {
            // literal field line with literal name: 001 N H length(3+)
            let name = get_string(&mut section, 3)?;
            (name, get_string(&mut section, 7)?)
        } else {
            // 0001: indexed field line with post-base index; 0000: literal
            // field line with post-base name reference — both entries of
            // the dynamic table
            return Err(Invalid);
        };
        size += (field.0.len() + field.1.len()) as u64 + 32;
        if size > limit {
            return Err(Refused::TooLarge);
        }
        fields.push(field);
    }
    Ok(fields)
}

/// The field section of `fields` — (name, value, sensitive), in order —
/// with the static table alone: Required Insert Count 0 and Base 0 (RFC
/// 9204 4.5.1), then each field as its exact entry, else as a literal
/// value with the entry of its name, else as a literal name and value. No
/// string is Huffman-encoded. A sensitive value is always a literal, marked
/// never to be indexed (RFC 9204 7.1.3).
pub(crate) fn encode<'a>(fields: impl IntoIterator<Item = (&'a [u8], &'a [u8], bool)>) -> Vec<u8> {
    let mut out = vec![0, 0];
    for (name, value, sensitive) in fields {
        let exact = STATIC_TABLE
            .iter()
            .position(|&(n, v)| n.as_bytes() == name && v.as_bytes() == value);
        let named = STATIC_TABLE.iter().position(|&(n, _)| n.as_bytes() == name);
        match (exact, named) {
            // 1 T=1 index(6+)
            (Some(index), _) if !sensitive => put_int(&mut out, 0xc0, 6, index as u64),
            // 01 N T=1 index(4+), then the value
            (_, Some(index)) => {
                let never = if sensitive { 0x20 } else { 0 };
                put_int(&mut out, 0x50 | never, 4, index as u64);
                put_string(&mut out, 0, 7, value);
            }
            // 001 N H=0 length(3+) and the name, then the value
            _ => {
                let never = if sensitive { 0x10 } else { 0 };
                put_string(&mut out, 0x20 | never, 3, name);
                put_string(&mut out, 0, 7, value);
            }
        }
    }
    out
}

/// The entry at `index` of the static table.
fn entry(index: u64) -> Result<(&'static str, &'static str), Refused> {
    usize::try_from(index)
        .ok()
        .and_then(|index| STATIC_TABLE.get(index))
        .copied()
        .ok_or(Refused::Invalid)
}

/// `value` as an integer with an `n`-bit prefix (RFC 7541 5.1), `flags`
/// in the bits of the first byte above it.
fn put_int(out: &mut Vec<u8>, flags: u8, n: u32, value: u64) {
    let max = (1u64 << n) - 1;
    if value < max {
        out.push(flags | value as u8);
        return;
    }
    out.push(flags | max as u8);
    let mut rest = value - max;
    while rest >= 0x80 {
        out.push(0x80 | (rest & 0x7f) as u8);
        rest >>= 7;
    }
    out.push(rest as u8);
}

/// `bytes` as a string literal (RFC 9204 4.1.2) whose length has an
/// `n`-bit prefix, `flags` above it — and H, the bit right above the
/// prefix, never set.
fn put_string(out: &mut Vec<u8>, flags: u8, n: u32, bytes: &[u8]) {
    put_int(out, flags, n, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// The integer with an `n`-bit prefix at the start of `src`, and the bits
/// of its first byte above the prefix. QPACK's integers have at most 62
/// bits (RFC 9204 4.1.1).
fn get_int(src: &mut &[u8], n: u32) -> Result<(u8, u64), Refused> {
    let (&first, rest) = src.split_first().ok_or(Refused::Invalid)?;
    *src = rest;
    let max = (1u64 << n) - 1;
    let mut value = u64::from(first) & max;
    if value == max {
        // seven bits per byte, the least significant first: nine bytes
        // reach past 62 bits, and their sum still fits 64
        let mut shift = 0;
        loop {
            let (&byte, rest) = src.split_first().ok_or(Refused::Invalid)?;
            *src = rest;
            value += u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 56 {
                return Err(Refused::Invalid);
            }
        }
        if value > VARINT_MAX {
            return Err(Refused::Invalid);
        }
    }
    Ok((first & !(max as u8), value))
}

/// The string literal at the start of `src`: its length has an `n`-bit
/// prefix, and the bit right above it says whether it is
/// Huffman-encoded.
fn get_string(src: &mut &[u8], n: u32) -> Result<Vec<u8>, Refused> {
    let (flags, len) = get_int(src, n)?;
    let len = usize::try_from(len).map_err(|_| Refused::Invalid)?;
    if len > src.len() {
        return Err(Refused::Invalid);
    }
    let (bytes, rest) = src.split_at(len);
    *src = rest;
    if flags & (1 << n) != 0 {
        huffman::decode(bytes).ok_or(Refused::Invalid)
    } else {
        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(list: &[(&str, &str)]) -> Vec<Field> {
        list.iter()
            .map(|(n, v)| (n.as_bytes().to_vec(), v.as_bytes().to_vec()))
            .collect()
    }

    /// The examples of RFC 7541 C.1, which QPACK's integers are.
    #[test]
    fn prefixed_integers_of_rfc7541_c1() {
        for (n, value, bytes) in [
            // C.1.1: 10 with a 5-bit prefix
            (5, 10, &[0x0a][..]),
            // C.1.2: 1337 with a 5-bit prefix
            (5, 1337, &[0x1f, 0x9a, 0x0a]),
            // C.1.3: 42 starting at an octet boundary
            (8, 42, &[0x2a]),
        ] {
            let mut out = Vec::new();
            put_int(&mut out, 0, n, value);
            assert_eq!(out, bytes, "{value}");
            let mut src = bytes;
            assert_eq!(get_int(&mut src, n), Ok((0, value)));
            assert!(src.is_empty());
        }
        // the bits above the prefix are the caller's
        let mut out = Vec::new();
        put_int(&mut out, 0xe0, 5, 1337);
        assert_eq!(out, [0xff, 0x9a, 0x0a]);
        assert_eq!(get_int(&mut &out[..], 5), Ok((0xe0, 1337)));
        // the largest QPACK integer
        let mut out = Vec::new();
        put_int(&mut out, 0, 6, VARINT_MAX);
        assert_eq!(get_int(&mut &out[..], 6), Ok((0, VARINT_MAX)));
    }

    #[test]
    fn integers_cut_short_or_too_long_are_refused() {
        // nothing, a full prefix with nothing after, a byte that says more
        // comes
        for bytes in [&[][..], &[0x1f], &[0x1f, 0x80]] {
            assert_eq!(
                get_int(&mut &bytes[..], 5),
                Err(Refused::Invalid),
                "{bytes:?}"
            );
        }
        // 2^62 does not fit
        let mut out = Vec::new();
        put_int(&mut out, 0, 6, VARINT_MAX + 1);
        assert_eq!(get_int(&mut &out[..], 6), Err(Refused::Invalid));
        // ten more bytes reach past 64 bits
        let long = [
            0x3f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
        ];
        assert_eq!(get_int(&mut &long[..], 6), Err(Refused::Invalid));
    }

    /// The entries whose whitespace the RFC's text wraps, checked against
    /// its HTML table: a space after the semicolon in 52, 57 and 58, none
    /// in 54.
    #[test]
    fn the_static_table_is_rfc9204_appendix_a() {
        assert_eq!(STATIC_TABLE[0], (":authority", ""));
        assert_eq!(STATIC_TABLE[25], (":status", "200"));
        assert_eq!(STATIC_TABLE[31], ("accept-encoding", "gzip, deflate, br"));
        assert_eq!(
            STATIC_TABLE[52],
            ("content-type", "text/html; charset=utf-8")
        );
        assert_eq!(
            STATIC_TABLE[54],
            ("content-type", "text/plain;charset=utf-8")
        );
        assert_eq!(
            STATIC_TABLE[58],
            (
                "strict-transport-security",
                "max-age=31536000; includesubdomains; preload"
            )
        );
        assert_eq!(STATIC_TABLE[72], ("accept-language", ""));
        assert_eq!(STATIC_TABLE[98], ("x-frame-options", "sameorigin"));
        assert!(
            STATIC_TABLE
                .iter()
                .all(|(name, _)| !name.contains(|c: char| c.is_ascii_uppercase()))
        );
    }

    /// RFC 9204 B.1: a literal value with the name of a static entry.
    #[test]
    fn rfc9204_b1_name_reference() {
        let section = [
            0x00, 0x00, 0x51, 0x0b, 0x2f, 0x69, 0x6e, 0x64, 0x65, 0x78, 0x2e, 0x68, 0x74, 0x6d,
            0x6c,
        ];
        assert_eq!(
            decode(&section, 1 << 16),
            Ok(fields(&[(":path", "/index.html")]))
        );
        assert_eq!(
            encode([(&b":path"[..], &b"/index.html"[..], false)]),
            section
        );
    }

    /// Hysteria 2's authentication head: exact entries, names with literal
    /// values (an index past the prefix), literal names (a length past the
    /// prefix), and the never-indexed secret.
    #[test]
    fn requests_use_exact_entries_then_names_then_literals() {
        let section = encode([
            (&b":method"[..], &b"POST"[..], false),
            (b":scheme", b"https", false),
            (b":authority", b"hysteria", false),
            (b":path", b"/auth", false),
            (b"hysteria-auth", b"pw", true),
            (b"hysteria-cc-rx", b"0", false),
            (b"content-type", b"application/dns-message", false),
            (b"user-agent", b"rurge", false),
        ]);
        let mut expected = vec![0x00, 0x00, 0xd4, 0xd7, 0x50, 0x08];
        expected.extend_from_slice(b"hysteria");
        expected.extend_from_slice(&[0x51, 0x05]);
        expected.extend_from_slice(b"/auth");
        expected.extend_from_slice(&[0x37, 0x06]);
        expected.extend_from_slice(b"hysteria-auth");
        expected.extend_from_slice(&[0x02]);
        expected.extend_from_slice(b"pw");
        expected.extend_from_slice(&[0x27, 0x07]);
        expected.extend_from_slice(b"hysteria-cc-rx");
        expected.extend_from_slice(&[0x01, b'0', 0xec, 0x5f, 0x50, 0x05]);
        expected.extend_from_slice(b"rurge");
        assert_eq!(section, expected);
    }

    /// A sensitive value never takes an exact entry and is marked never to
    /// be indexed, with the name of an entry (the first of that name) or
    /// with its own.
    #[test]
    fn a_sensitive_value_is_a_never_indexed_literal() {
        let section = encode([
            (&b":method"[..], &b"GET"[..], true),
            (b"authorization", b"secret", true),
            (b"x-secret", b"s", true),
        ]);
        let mut expected = vec![0x00, 0x00, 0x7f, 0x00, 0x03];
        expected.extend_from_slice(b"GET");
        expected.extend_from_slice(&[0x7f, 0x45, 0x06]);
        expected.extend_from_slice(b"secret");
        expected.extend_from_slice(&[0x37, 0x01]);
        expected.extend_from_slice(b"x-secret");
        expected.extend_from_slice(&[0x01, b's']);
        assert_eq!(section, expected);
    }

    /// Every form that needs no dynamic table: exact entries, literals with
    /// a static name or their own, Huffman-encoded or not, never-indexed or
    /// not, and any Base.
    #[test]
    fn every_static_form_decodes() {
        let mut section = vec![0x00, 0x05, 0xd9];
        // custom-key: custom-value, both Huffman-encoded (RFC 7541 C.4.3)
        section.extend_from_slice(&[0x2f, 0x01, 0x25, 0xa8, 0x49, 0xe9, 0x5b, 0xa9, 0x7d, 0x7f]);
        section.extend_from_slice(&[0x89, 0x25, 0xa8, 0x49, 0xe9, 0x5b, 0xb8, 0xe8, 0xb4, 0xbf]);
        // cache-control (36) with "private" Huffman-encoded (RFC 7541 C.6.1)
        section.extend_from_slice(&[0x5f, 0x15, 0x85, 0xae, 0xc3, 0x77, 0x1a, 0x4b]);
        // a never-indexed literal name and value
        section.extend_from_slice(&[0x33, b'x', b'-', b'n', 0x01, b'1']);
        // a never-indexed value with a static name, x-frame-options (97)
        section.extend_from_slice(&[0x7f, 0x52, 0x02, b'n', b'o']);
        assert_eq!(
            decode(&section, 1 << 16),
            Ok(fields(&[
                (":status", "200"),
                ("custom-key", "custom-value"),
                ("cache-control", "private"),
                ("x-n", "1"),
                ("x-frame-options", "no"),
            ]))
        );
    }

    /// RFC 9204 B.2 and B.4 refer to their dynamic table, and so does each
    /// form with T = 0 or a post-base index.
    #[test]
    fn dynamic_references_are_refused() {
        for section in [
            // B.2, stream 4: Required Insert Count 2, post-base indexes
            &[0x03, 0x81, 0x10, 0x11][..],
            // B.4, stream 8: Required Insert Count 4, dynamic entries
            &[0x05, 0x00, 0x80, 0xc1, 0x81],
            // indexed, T = 0
            &[0x00, 0x00, 0x80],
            // indexed with post-base index
            &[0x00, 0x00, 0x10],
            // literal with a dynamic name reference
            &[0x00, 0x00, 0x40, 0x00],
            // literal with post-base name reference
            &[0x00, 0x00, 0x00, 0x00],
        ] {
            assert_eq!(
                decode(section, 1 << 16),
                Err(Refused::Invalid),
                "{section:02x?}"
            );
        }
    }

    #[test]
    fn a_base_below_zero_is_refused() {
        // Required Insert Count 0, Sign 1, Delta Base 0
        assert_eq!(decode(&[0x00, 0x80, 0xd9], 1 << 16), Err(Refused::Invalid));
    }

    #[test]
    fn an_index_past_the_static_table_is_refused() {
        // indexed 98, then 99
        assert_eq!(
            decode(&[0x00, 0x00, 0xff, 0x23], 1 << 16),
            Ok(fields(&[("x-frame-options", "sameorigin")]))
        );
        assert_eq!(
            decode(&[0x00, 0x00, 0xff, 0x24], 1 << 16),
            Err(Refused::Invalid)
        );
        // a name reference to 99
        assert_eq!(
            decode(&[0x00, 0x00, 0x5f, 0x54, 0x00], 1 << 16),
            Err(Refused::Invalid)
        );
    }

    #[test]
    fn a_bad_huffman_string_is_refused() {
        // a value of one byte: eight bits of padding
        assert_eq!(
            decode(&[0x00, 0x00, 0x5f, 0x1d, 0x81, 0xff], 1 << 16),
            Err(Refused::Invalid)
        );
    }

    /// :status 200 counts 7 + 3 + 32 and x: y 1 + 1 + 32.
    #[test]
    fn the_size_counts_names_values_and_32_per_field() {
        let section = [0x00, 0x00, 0xd9, 0x21, b'x', 0x01, b'y'];
        assert!(decode(&section, 76).is_ok());
        assert_eq!(decode(&section, 75), Err(Refused::TooLarge));
    }

    /// A section cut anywhere is refused, or — cut between field lines —
    /// gives the fields before the cut.
    #[test]
    fn a_section_cut_anywhere_is_refused_or_shorter() {
        let whole = encode([
            (&b":status"[..], &b"200"[..], false),
            (b"user-agent", b"rurge", false),
            (b"a-name-longer-than-the-prefix", b"a value", true),
        ]);
        let all = decode(&whole, 1 << 16).unwrap();
        for cut in 0..whole.len() {
            match decode(&whole[..cut], 1 << 16) {
                Err(e) => assert_eq!(e, Refused::Invalid, "cut at {cut}"),
                Ok(some) => assert!(
                    some.len() < all.len() && all.starts_with(&some),
                    "cut at {cut}"
                ),
            }
        }
    }
}
