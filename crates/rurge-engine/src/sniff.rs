//! TLS ClientHello SNI parsing (M3b, observability only). Best-effort: any
//! malformed, truncated, or non-TLS input returns `None`. And QUIC Initial
//! recognition for UDP flows (phase 2 M5 design 6.1).

/// QUIC v2's version number (RFC 9369), whose Initial packets carry type 1.
const QUIC_V2: u32 = 0x6b33_43cf;

/// Whether `datagram` is a QUIC Initial packet: a long header with the
/// fixed bit, a version other than 0 (version negotiation), the Initial
/// type — 0 in v1 and the drafts, 1 in v2 — and the 1200 bytes a client's
/// Initial is padded to (RFC 9000 §14.1, §17.2.2; RFC 9369 §3.2).
pub fn is_quic_initial(datagram: &[u8]) -> bool {
    let Some(&first) = datagram.first() else {
        return false;
    };
    let Some(version) = datagram.get(1..5) else {
        return false;
    };
    let version = u32::from_be_bytes([version[0], version[1], version[2], version[3]]);
    let kind = (first >> 4) & 0b11;
    datagram.len() >= 1200
        && first & 0xc0 == 0xc0
        && match version {
            0 => false,
            QUIC_V2 => kind == 0b01,
            _ => kind == 0b00,
        }
}

/// Extracts the SNI host_name from a single TLS ClientHello record, if present.
pub fn parse_sni(record: &[u8]) -> Option<String> {
    // TLS record header: content_type(0x16 handshake) + version(2) + length(2)
    let rec = record.get(..5)?;
    if rec[0] != 0x16 {
        return None;
    }
    let rec_len = u16::from_be_bytes([rec[3], rec[4]]) as usize;
    let body = record.get(5..5 + rec_len)?;
    // Handshake header: type(0x01 ClientHello) + length(3, big-endian)
    if *body.first()? != 0x01 {
        return None;
    }
    let hs_len = ((*body.get(1)? as usize) << 16)
        | ((*body.get(2)? as usize) << 8)
        | (*body.get(3)? as usize);
    let hs = body.get(4..4 + hs_len)?;
    let mut p = 0usize;
    p += 2; // client_version
    p += 32; // random
    let sid_len = *hs.get(p)? as usize;
    p += 1 + sid_len;
    let cs_len = u16::from_be_bytes([*hs.get(p)?, *hs.get(p + 1)?]) as usize;
    p += 2 + cs_len;
    let comp_len = *hs.get(p)? as usize;
    p += 1 + comp_len;
    let ext_total = u16::from_be_bytes([*hs.get(p)?, *hs.get(p + 1)?]) as usize;
    p += 2;
    let exts = hs.get(p..p + ext_total)?;
    let mut q = 0usize;
    while q + 4 <= exts.len() {
        let etype = u16::from_be_bytes([exts[q], exts[q + 1]]);
        let elen = u16::from_be_bytes([exts[q + 2], exts[q + 3]]) as usize;
        let ebody = exts.get(q + 4..q + 4 + elen)?;
        if etype == 0x0000 {
            // server_name list: list_len(2) then entries of type(1)+len(2)+name
            let list_len = u16::from_be_bytes([*ebody.first()?, *ebody.get(1)?]) as usize;
            let list = ebody.get(2..2 + list_len)?;
            let mut r = 0usize;
            while r + 3 <= list.len() {
                let ntype = list[r];
                let nlen = u16::from_be_bytes([list[r + 1], list[r + 2]]) as usize;
                let name = list.get(r + 3..r + 3 + nlen)?;
                if ntype == 0 {
                    return std::str::from_utf8(name).ok().map(str::to_string);
                }
                r += 3 + nlen;
            }
            return None;
        }
        q += 4 + elen;
    }
    None
}

#[cfg(test)]
mod quic_tests {
    use super::*;

    fn initial(first: u8, version: u32, len: usize) -> Vec<u8> {
        let mut out = vec![first];
        out.extend_from_slice(&version.to_be_bytes());
        out.resize(len, 0);
        out
    }

    #[test]
    fn a_quic_initial_is_recognised() {
        assert!(is_quic_initial(&initial(0xc3, 1, 1200)), "v1");
        assert!(is_quic_initial(&initial(0xd3, QUIC_V2, 1250)), "v2");
        assert!(
            is_quic_initial(&initial(0xc0, 0xff00_001d, 1200)),
            "draft 29"
        );
        assert!(!is_quic_initial(&initial(0xc3, 1, 1199)), "unpadded");
        assert!(
            !is_quic_initial(&initial(0xe3, 1, 1200)),
            "a handshake packet"
        );
        assert!(
            !is_quic_initial(&initial(0xc3, QUIC_V2, 1200)),
            "v2 type 0 is 0-RTT"
        );
        assert!(
            !is_quic_initial(&initial(0xc3, 0, 1200)),
            "version negotiation"
        );
        assert!(!is_quic_initial(&initial(0x43, 1, 1200)), "a short header");
        assert!(!is_quic_initial(&[0xc3, 0, 0]));
        assert!(!is_quic_initial(&[]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal TLS ClientHello record carrying one SNI host_name.
    fn client_hello(host: &str) -> Vec<u8> {
        let sni = host.as_bytes();
        // server_name extension body: list_len(2) + type(1) + name_len(2) + name
        let mut server_name = Vec::new();
        server_name.extend_from_slice(&((sni.len() + 3) as u16).to_be_bytes());
        server_name.push(0); // host_name
        server_name.extend_from_slice(&(sni.len() as u16).to_be_bytes());
        server_name.extend_from_slice(sni);
        // extension: type 0x0000 + len + body
        let mut ext = Vec::new();
        ext.extend_from_slice(&0u16.to_be_bytes());
        ext.extend_from_slice(&(server_name.len() as u16).to_be_bytes());
        ext.extend_from_slice(&server_name);
        // handshake body
        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]); // client_version TLS1.2
        body.extend_from_slice(&[0u8; 32]); // random
        body.push(0); // session_id len
        body.extend_from_slice(&2u16.to_be_bytes()); // cipher suites len
        body.extend_from_slice(&[0x13, 0x01]);
        body.push(1); // compression len
        body.push(0);
        body.extend_from_slice(&(ext.len() as u16).to_be_bytes()); // extensions len
        body.extend_from_slice(&ext);
        // handshake header: type 0x01 + 3-byte len
        let mut hs = vec![0x01];
        let bl = body.len();
        hs.extend_from_slice(&[(bl >> 16) as u8, (bl >> 8) as u8, bl as u8]);
        hs.extend_from_slice(&body);
        // TLS record: type 0x16, version, len
        let mut rec = vec![0x16, 0x03, 0x01];
        rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
        rec.extend_from_slice(&hs);
        rec
    }

    #[test]
    fn parses_the_sni_host() {
        assert_eq!(
            parse_sni(&client_hello("api.example.com")).as_deref(),
            Some("api.example.com")
        );
    }

    #[test]
    fn rejects_non_tls_and_truncated_input() {
        assert_eq!(parse_sni(b"GET / HTTP/1.1\r\n"), None);
        let full = client_hello("x.test");
        assert_eq!(parse_sni(&full[..20]), None); // truncated
        assert_eq!(parse_sni(&[]), None);
        assert_eq!(parse_sni(&[0x16, 0x03, 0x01]), None);
    }
}
