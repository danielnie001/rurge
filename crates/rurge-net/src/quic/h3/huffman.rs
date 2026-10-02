//! The Huffman code of HPACK (RFC 7541 Appendix B), which QPACK string
//! literals may use (RFC 9204 4.1.2). The client decodes it and never
//! encodes with it.

/// The code of each octet and then of EOS, as (code, length in bits),
/// transcribed from RFC 7541 Appendix B. The code is complete and
/// canonical: the codes of one length follow each other in octet order,
/// and those of the next length start right after them. `decode` relies on
/// it; a test checks it.
pub(crate) const CODES: [(u32, u8); 257] = [
    (0x1ff8, 13),     // (0)
    (0x7fffd8, 23),   // (1)
    (0xfffffe2, 28),  // (2)
    (0xfffffe3, 28),  // (3)
    (0xfffffe4, 28),  // (4)
    (0xfffffe5, 28),  // (5)
    (0xfffffe6, 28),  // (6)
    (0xfffffe7, 28),  // (7)
    (0xfffffe8, 28),  // (8)
    (0xffffea, 24),   // (9)
    (0x3ffffffc, 30), // (10)
    (0xfffffe9, 28),  // (11)
    (0xfffffea, 28),  // (12)
    (0x3ffffffd, 30), // (13)
    (0xfffffeb, 28),  // (14)
    (0xfffffec, 28),  // (15)
    (0xfffffed, 28),  // (16)
    (0xfffffee, 28),  // (17)
    (0xfffffef, 28),  // (18)
    (0xffffff0, 28),  // (19)
    (0xffffff1, 28),  // (20)
    (0xffffff2, 28),  // (21)
    (0x3ffffffe, 30), // (22)
    (0xffffff3, 28),  // (23)
    (0xffffff4, 28),  // (24)
    (0xffffff5, 28),  // (25)
    (0xffffff6, 28),  // (26)
    (0xffffff7, 28),  // (27)
    (0xffffff8, 28),  // (28)
    (0xffffff9, 28),  // (29)
    (0xffffffa, 28),  // (30)
    (0xffffffb, 28),  // (31)
    (0x14, 6),        // ' ' (32)
    (0x3f8, 10),      // '!' (33)
    (0x3f9, 10),      // '"' (34)
    (0xffa, 12),      // '#' (35)
    (0x1ff9, 13),     // '$' (36)
    (0x15, 6),        // '%' (37)
    (0xf8, 8),        // '&' (38)
    (0x7fa, 11),      // ''' (39)
    (0x3fa, 10),      // '(' (40)
    (0x3fb, 10),      // ')' (41)
    (0xf9, 8),        // '*' (42)
    (0x7fb, 11),      // '+' (43)
    (0xfa, 8),        // ',' (44)
    (0x16, 6),        // '-' (45)
    (0x17, 6),        // '.' (46)
    (0x18, 6),        // '/' (47)
    (0x0, 5),         // '0' (48)
    (0x1, 5),         // '1' (49)
    (0x2, 5),         // '2' (50)
    (0x19, 6),        // '3' (51)
    (0x1a, 6),        // '4' (52)
    (0x1b, 6),        // '5' (53)
    (0x1c, 6),        // '6' (54)
    (0x1d, 6),        // '7' (55)
    (0x1e, 6),        // '8' (56)
    (0x1f, 6),        // '9' (57)
    (0x5c, 7),        // ':' (58)
    (0xfb, 8),        // ';' (59)
    (0x7ffc, 15),     // '<' (60)
    (0x20, 6),        // '=' (61)
    (0xffb, 12),      // '>' (62)
    (0x3fc, 10),      // '?' (63)
    (0x1ffa, 13),     // '@' (64)
    (0x21, 6),        // 'A' (65)
    (0x5d, 7),        // 'B' (66)
    (0x5e, 7),        // 'C' (67)
    (0x5f, 7),        // 'D' (68)
    (0x60, 7),        // 'E' (69)
    (0x61, 7),        // 'F' (70)
    (0x62, 7),        // 'G' (71)
    (0x63, 7),        // 'H' (72)
    (0x64, 7),        // 'I' (73)
    (0x65, 7),        // 'J' (74)
    (0x66, 7),        // 'K' (75)
    (0x67, 7),        // 'L' (76)
    (0x68, 7),        // 'M' (77)
    (0x69, 7),        // 'N' (78)
    (0x6a, 7),        // 'O' (79)
    (0x6b, 7),        // 'P' (80)
    (0x6c, 7),        // 'Q' (81)
    (0x6d, 7),        // 'R' (82)
    (0x6e, 7),        // 'S' (83)
    (0x6f, 7),        // 'T' (84)
    (0x70, 7),        // 'U' (85)
    (0x71, 7),        // 'V' (86)
    (0x72, 7),        // 'W' (87)
    (0xfc, 8),        // 'X' (88)
    (0x73, 7),        // 'Y' (89)
    (0xfd, 8),        // 'Z' (90)
    (0x1ffb, 13),     // '[' (91)
    (0x7fff0, 19),    // '\' (92)
    (0x1ffc, 13),     // ']' (93)
    (0x3ffc, 14),     // '^' (94)
    (0x22, 6),        // '_' (95)
    (0x7ffd, 15),     // '`' (96)
    (0x3, 5),         // 'a' (97)
    (0x23, 6),        // 'b' (98)
    (0x4, 5),         // 'c' (99)
    (0x24, 6),        // 'd' (100)
    (0x5, 5),         // 'e' (101)
    (0x25, 6),        // 'f' (102)
    (0x26, 6),        // 'g' (103)
    (0x27, 6),        // 'h' (104)
    (0x6, 5),         // 'i' (105)
    (0x74, 7),        // 'j' (106)
    (0x75, 7),        // 'k' (107)
    (0x28, 6),        // 'l' (108)
    (0x29, 6),        // 'm' (109)
    (0x2a, 6),        // 'n' (110)
    (0x7, 5),         // 'o' (111)
    (0x2b, 6),        // 'p' (112)
    (0x76, 7),        // 'q' (113)
    (0x2c, 6),        // 'r' (114)
    (0x8, 5),         // 's' (115)
    (0x9, 5),         // 't' (116)
    (0x2d, 6),        // 'u' (117)
    (0x77, 7),        // 'v' (118)
    (0x78, 7),        // 'w' (119)
    (0x79, 7),        // 'x' (120)
    (0x7a, 7),        // 'y' (121)
    (0x7b, 7),        // 'z' (122)
    (0x7ffe, 15),     // '{' (123)
    (0x7fc, 11),      // '|' (124)
    (0x3ffd, 14),     // '}' (125)
    (0x1ffd, 13),     // '~' (126)
    (0xffffffc, 28),  // (127)
    (0xfffe6, 20),    // (128)
    (0x3fffd2, 22),   // (129)
    (0xfffe7, 20),    // (130)
    (0xfffe8, 20),    // (131)
    (0x3fffd3, 22),   // (132)
    (0x3fffd4, 22),   // (133)
    (0x3fffd5, 22),   // (134)
    (0x7fffd9, 23),   // (135)
    (0x3fffd6, 22),   // (136)
    (0x7fffda, 23),   // (137)
    (0x7fffdb, 23),   // (138)
    (0x7fffdc, 23),   // (139)
    (0x7fffdd, 23),   // (140)
    (0x7fffde, 23),   // (141)
    (0xffffeb, 24),   // (142)
    (0x7fffdf, 23),   // (143)
    (0xffffec, 24),   // (144)
    (0xffffed, 24),   // (145)
    (0x3fffd7, 22),   // (146)
    (0x7fffe0, 23),   // (147)
    (0xffffee, 24),   // (148)
    (0x7fffe1, 23),   // (149)
    (0x7fffe2, 23),   // (150)
    (0x7fffe3, 23),   // (151)
    (0x7fffe4, 23),   // (152)
    (0x1fffdc, 21),   // (153)
    (0x3fffd8, 22),   // (154)
    (0x7fffe5, 23),   // (155)
    (0x3fffd9, 22),   // (156)
    (0x7fffe6, 23),   // (157)
    (0x7fffe7, 23),   // (158)
    (0xffffef, 24),   // (159)
    (0x3fffda, 22),   // (160)
    (0x1fffdd, 21),   // (161)
    (0xfffe9, 20),    // (162)
    (0x3fffdb, 22),   // (163)
    (0x3fffdc, 22),   // (164)
    (0x7fffe8, 23),   // (165)
    (0x7fffe9, 23),   // (166)
    (0x1fffde, 21),   // (167)
    (0x7fffea, 23),   // (168)
    (0x3fffdd, 22),   // (169)
    (0x3fffde, 22),   // (170)
    (0xfffff0, 24),   // (171)
    (0x1fffdf, 21),   // (172)
    (0x3fffdf, 22),   // (173)
    (0x7fffeb, 23),   // (174)
    (0x7fffec, 23),   // (175)
    (0x1fffe0, 21),   // (176)
    (0x1fffe1, 21),   // (177)
    (0x3fffe0, 22),   // (178)
    (0x1fffe2, 21),   // (179)
    (0x7fffed, 23),   // (180)
    (0x3fffe1, 22),   // (181)
    (0x7fffee, 23),   // (182)
    (0x7fffef, 23),   // (183)
    (0xfffea, 20),    // (184)
    (0x3fffe2, 22),   // (185)
    (0x3fffe3, 22),   // (186)
    (0x3fffe4, 22),   // (187)
    (0x7ffff0, 23),   // (188)
    (0x3fffe5, 22),   // (189)
    (0x3fffe6, 22),   // (190)
    (0x7ffff1, 23),   // (191)
    (0x3ffffe0, 26),  // (192)
    (0x3ffffe1, 26),  // (193)
    (0xfffeb, 20),    // (194)
    (0x7fff1, 19),    // (195)
    (0x3fffe7, 22),   // (196)
    (0x7ffff2, 23),   // (197)
    (0x3fffe8, 22),   // (198)
    (0x1ffffec, 25),  // (199)
    (0x3ffffe2, 26),  // (200)
    (0x3ffffe3, 26),  // (201)
    (0x3ffffe4, 26),  // (202)
    (0x7ffffde, 27),  // (203)
    (0x7ffffdf, 27),  // (204)
    (0x3ffffe5, 26),  // (205)
    (0xfffff1, 24),   // (206)
    (0x1ffffed, 25),  // (207)
    (0x7fff2, 19),    // (208)
    (0x1fffe3, 21),   // (209)
    (0x3ffffe6, 26),  // (210)
    (0x7ffffe0, 27),  // (211)
    (0x7ffffe1, 27),  // (212)
    (0x3ffffe7, 26),  // (213)
    (0x7ffffe2, 27),  // (214)
    (0xfffff2, 24),   // (215)
    (0x1fffe4, 21),   // (216)
    (0x1fffe5, 21),   // (217)
    (0x3ffffe8, 26),  // (218)
    (0x3ffffe9, 26),  // (219)
    (0xffffffd, 28),  // (220)
    (0x7ffffe3, 27),  // (221)
    (0x7ffffe4, 27),  // (222)
    (0x7ffffe5, 27),  // (223)
    (0xfffec, 20),    // (224)
    (0xfffff3, 24),   // (225)
    (0xfffed, 20),    // (226)
    (0x1fffe6, 21),   // (227)
    (0x3fffe9, 22),   // (228)
    (0x1fffe7, 21),   // (229)
    (0x1fffe8, 21),   // (230)
    (0x7ffff3, 23),   // (231)
    (0x3fffea, 22),   // (232)
    (0x3fffeb, 22),   // (233)
    (0x1ffffee, 25),  // (234)
    (0x1ffffef, 25),  // (235)
    (0xfffff4, 24),   // (236)
    (0xfffff5, 24),   // (237)
    (0x3ffffea, 26),  // (238)
    (0x7ffff4, 23),   // (239)
    (0x3ffffeb, 26),  // (240)
    (0x7ffffe6, 27),  // (241)
    (0x3ffffec, 26),  // (242)
    (0x3ffffed, 26),  // (243)
    (0x7ffffe7, 27),  // (244)
    (0x7ffffe8, 27),  // (245)
    (0x7ffffe9, 27),  // (246)
    (0x7ffffea, 27),  // (247)
    (0x7ffffeb, 27),  // (248)
    (0xffffffe, 28),  // (249)
    (0x7ffffec, 27),  // (250)
    (0x7ffffed, 27),  // (251)
    (0x7ffffee, 27),  // (252)
    (0x7ffffef, 27),  // (253)
    (0x7fffff0, 27),  // (254)
    (0x3ffffee, 26),  // (255)
    (0x3fffffff, 30), // EOS (256)
];

/// The longest code: EOS's, and those of three control octets.
const LONGEST: usize = 30;

/// The canonical code as decoding walks it: for each length, the first
/// code of that length, how many codes have it, and where their symbols
/// start in `symbols` — every symbol, by length and then octet.
struct Canonical {
    first: [u32; LONGEST + 1],
    count: [u16; LONGEST + 1],
    start: [u16; LONGEST + 1],
    symbols: [u16; 257],
}

const CANONICAL: Canonical = canonical();

const fn canonical() -> Canonical {
    let mut count = [0u16; LONGEST + 1];
    let mut symbol = 0;
    while symbol < CODES.len() {
        count[CODES[symbol].1 as usize] += 1;
        symbol += 1;
    }
    let mut first = [0u32; LONGEST + 1];
    let mut start = [0u16; LONGEST + 1];
    let (mut code, mut at) = (0u32, 0u16);
    let mut len = 1;
    while len <= LONGEST {
        code = (code + count[len - 1] as u32) << 1;
        first[len] = code;
        start[len] = at;
        at += count[len];
        len += 1;
    }
    let mut symbols = [0u16; 257];
    let mut next = start;
    let mut symbol = 0;
    while symbol < CODES.len() {
        let len = CODES[symbol].1 as usize;
        symbols[next[len] as usize] = symbol as u16;
        next[len] += 1;
        symbol += 1;
    }
    Canonical {
        first,
        count,
        start,
        symbols,
    }
}

/// `src` decoded; `None` for what no encoder makes (RFC 7541 5.2): EOS
/// among the symbols, or padding longer than 7 bits or other than the
/// first bits of EOS (all ones).
pub(crate) fn decode(src: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(src.len() * 8 / 5);
    // the bits of a code not yet complete
    let (mut code, mut len) = (0u32, 0usize);
    for &byte in src {
        for bit in (0..8).rev() {
            code = (code << 1) | u32::from((byte >> bit) & 1);
            len += 1;
            let at = code.wrapping_sub(CANONICAL.first[len]);
            if at < u32::from(CANONICAL.count[len]) {
                let symbol = CANONICAL.symbols[usize::from(CANONICAL.start[len]) + at as usize];
                // EOS, the one symbol past the octets, is not data
                out.push(u8::try_from(symbol).ok()?);
                (code, len) = (0, 0);
            } else if len == LONGEST {
                return None;
            }
        }
    }
    (len <= 7 && code == (1 << len) - 1).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quic::h3::testing::huffman_encode;

    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    /// The codes fill the code space exactly (Kraft), and each is where
    /// the canonical order of (length, octet) puts it.
    #[test]
    fn the_code_is_complete_and_canonical() {
        let kraft: u64 = CODES
            .iter()
            .map(|&(_, len)| 1u64 << (LONGEST - usize::from(len)))
            .sum();
        assert_eq!(kraft, 1 << LONGEST);
        let mut order: Vec<usize> = (0..CODES.len()).collect();
        order.sort_by_key(|&symbol| (CODES[symbol].1, symbol));
        let (mut code, mut len) = (0u32, CODES[order[0]].1);
        for (n, &symbol) in order.iter().enumerate() {
            if n > 0 {
                code = (code + 1) << (CODES[symbol].1 - len);
                len = CODES[symbol].1;
            }
            assert_eq!(CODES[symbol], (code, len), "symbol {symbol}");
        }
        assert_eq!(CODES[256], (0x3fff_ffff, 30), "EOS");
    }

    /// The Huffman-encoded strings of the examples of RFC 7541 Appendix C.4
    /// (requests) and C.6 (responses).
    #[test]
    fn the_strings_of_rfc7541_c4_and_c6() {
        for (encoded, text) in [
            ("f1e3 c2e5 f23a 6ba0 ab90 f4ff", "www.example.com"),
            ("a8eb 1064 9cbf", "no-cache"),
            ("25a8 49e9 5ba9 7d7f", "custom-key"),
            ("25a8 49e9 5bb8 e8b4 bf", "custom-value"),
            ("6402", "302"),
            ("aec3 771a 4b", "private"),
            (
                "d07a be94 1054 d444 a820 0595 040b 8166 e082 a62d 1bff",
                "Mon, 21 Oct 2013 20:13:21 GMT",
            ),
            (
                "9d29 ad17 1863 c78f 0b97 c8e9 ae82 ae43 d3",
                "https://www.example.com",
            ),
            ("640e ff", "307"),
            (
                "d07a be94 1054 d444 a820 0595 040b 8166 e084 a62d 1bff",
                "Mon, 21 Oct 2013 20:13:22 GMT",
            ),
            ("9bd9 ab", "gzip"),
            (
                "94e7 821d d7f2 e6c7 b335 dfdf cd5b 3960 d5af 2708 7f36 72c1 \
                 ab27 0fb5 291f 9587 3160 65c0 03ed 4ee5 b106 3d50 07",
                "foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1",
            ),
        ] {
            assert_eq!(
                decode(&hex(encoded)).as_deref(),
                Some(text.as_bytes()),
                "{text}"
            );
        }
    }

    /// Every octet, the longest codes among them, and nothing at all.
    #[test]
    fn every_octet_decodes() {
        let octets: Vec<u8> = (0..=u8::MAX).collect();
        assert_eq!(decode(&huffman_encode(&octets)), Some(octets));
        assert_eq!(decode(&[]), Some(Vec::new()));
    }

    #[test]
    fn padding_longer_than_7_bits_is_refused() {
        // '0' (00000) and three bits of padding
        assert_eq!(decode(&[0x07]), Some(b"0".to_vec()));
        // '0' and eleven bits of padding
        assert_eq!(decode(&[0x07, 0xff]), None);
        // eight bits of padding and no symbol
        assert_eq!(decode(&[0xff]), None);
    }

    #[test]
    fn padding_is_the_start_of_eos() {
        // '0' (00000), then 000 and 110
        assert_eq!(decode(&[0x00]), None);
        assert_eq!(decode(&[0x06]), None);
    }

    #[test]
    fn an_encoded_eos_is_refused() {
        // EOS, then two bits of padding
        assert_eq!(decode(&[0xff; 4]), None);
        // 'a' (00011), then EOS and its padding
        assert_eq!(decode(&[0x1f, 0xff, 0xff, 0xff, 0xff]), None);
    }
}
