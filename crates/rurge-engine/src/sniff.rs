//! TLS ClientHello SNI parsing (M3b, observability only). Best-effort: any
//! malformed, truncated, or non-TLS input returns `None`. And STUN
//! recognition for UDP flows (phase 3 M5 design 5); QUIC's recognition lives
//! in `rurge_net::quic::initial`, since it opens the Initial packet.

/// Whether `datagram` is a STUN message (RFC 5389 / 8489 §5, §6): at
/// least the 20 bytes of the header, the two top bits of the first byte
/// clear, the magic cookie 0x2112A442 in bytes 4–7, and a message length
/// (bytes 2–3) that is a multiple of 4 and is exactly what follows the
/// header — over UDP one datagram is one message. Classic STUN (RFC 3489,
/// no cookie) and TURN ChannelData are not recognised.
pub fn is_stun(datagram: &[u8]) -> bool {
    let Some(header) = datagram.get(..20) else {
        return false;
    };
    // the two top bits are zero in every STUN message: set, the first is RTP
    // (0x80) and the second TURN's ChannelData (0x40)
    if header[0] & 0xc0 != 0 {
        return false;
    }
    if header[4..8] != [0x21, 0x12, 0xa4, 0x42] {
        return false;
    }
    let length = u16::from_be_bytes([header[2], header[3]]) as usize;
    length % 4 == 0 && 20 + length == datagram.len()
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

#[cfg(test)]
mod stun_tests {
    use super::*;

    /// A Binding request (RFC 8489 §6): type 0x0001, no attributes, the
    /// magic cookie, and a transaction ID.
    fn binding() -> Vec<u8> {
        let mut out = vec![0x00, 0x01, 0x00, 0x00, 0x21, 0x12, 0xa4, 0x42];
        out.extend_from_slice(&[0x0f; 12]);
        out
    }

    #[test]
    fn stun_messages_are_recognised() {
        assert!(is_stun(&binding()), "a Binding request");
        let mut with_attribute = binding();
        // SOFTWARE (0x8022), four bytes of value
        with_attribute[2..4].copy_from_slice(&8u16.to_be_bytes());
        with_attribute.extend_from_slice(&[0x80, 0x22, 0x00, 0x04]);
        with_attribute.extend_from_slice(b"rurg");
        assert!(is_stun(&with_attribute), "a request with one attribute");
    }

    #[test]
    fn what_is_not_stun_is_not() {
        let mut wrong_cookie = binding();
        wrong_cookie[7] ^= 0x01;
        assert!(!is_stun(&wrong_cookie), "the magic cookie off by a bit");
        let mut not_multiple = binding();
        not_multiple[2..4].copy_from_slice(&6u16.to_be_bytes());
        not_multiple.extend_from_slice(&[0u8; 6]);
        assert!(
            !is_stun(&not_multiple),
            "a length that is not a multiple of four"
        );
        let mut short = binding();
        short[2..4].copy_from_slice(&8u16.to_be_bytes());
        short.extend_from_slice(&[0u8; 4]);
        assert!(!is_stun(&short), "a length the datagram does not fill");
        let mut rtp = binding();
        rtp[0] = 0x80;
        assert!(!is_stun(&rtp), "RTP");
        let mut channel_data = binding();
        channel_data[0] = 0x40;
        assert!(!is_stun(&channel_data), "TURN ChannelData");
        assert!(!is_stun(&binding()[..19]), "19 bytes");
        assert!(!is_stun(&[]), "an empty datagram");
    }
}
