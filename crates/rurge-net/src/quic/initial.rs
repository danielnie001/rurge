//! Whether a datagram begins with a client's QUIC Initial packet (phase 3 /
//! M5 design 4.2).
//!
//! The header alone cannot tell an Initial from some other encrypted UDP
//! protocol that happens to start with the same shape: a random first byte
//! has the long header bit set about one time in two. So the check is two
//! parts. The cheap part reads the header — the version, its Initial type,
//! the lengths of the connection IDs, the token and the Length field — and
//! turns away, without allocating and without deriving a key, anything that
//! could not be an Initial. What is left is copied up to the end of its
//! Length, header protection removed, and opened with the client Initial
//! keys of its own Destination Connection ID: only a real Initial has the
//! tag those keys accept, and a forgery passes about once in 2¹²⁸.
//!
//! Only the first packet of the datagram is judged: what a coalesced
//! packet after it holds is not this check's business. The frames of a
//! packet that opens are not read either; the SNI inside the ClientHello
//! belongs to a later phase.

use rustls::Side;
use rustls::quic::Version;

/// The smallest datagram a client's Initial comes in (RFC 9000 §14.1):
/// one that carries an Initial is padded to 1200 bytes.
const MIN_DATAGRAM: usize = 1200;

/// The bytes of a header protection sample (RFC 9001 §5.4.2), and so,
/// together with a 4-byte packet number, the least a Length may be.
const SAMPLE: usize = 16;
const MIN_LENGTH: usize = 4 + SAMPLE;

/// Whether `datagram` begins with a client's QUIC Initial packet (design
/// 4.2): the first packet in it, as far as its Length reaches, has a long
/// header (the QUIC bit may be clear, RFC 9287 §3.1), a version this
/// recognises (v1, v2, drafts 29–32), the Initial type of that version,
/// connection IDs of lengths QUIC allows, and opens with the client Initial
/// keys of its own Destination Connection ID. A datagram the header checks
/// turn away costs no allocation and no key.
pub fn is_client_initial(datagram: &[u8]) -> bool {
    let Some(header) = header(datagram) else {
        return false;
    };
    opens(datagram, &header)
}

/// What an Initial's header says, once the cheap checks pass: the version,
/// the Destination Connection ID its keys come from, and where its Length
/// ends.
struct Header<'a> {
    version: Version,
    dcid: &'a [u8],
    /// The first byte of the packet number, after the Length field.
    pn_offset: usize,
    /// The Length field's value: the packet number, the payload and the
    /// tag from `pn_offset` on.
    length: usize,
}

/// The rustls version whose Initial keys this knows (design 4.3): v1, v2,
/// and drafts 29 to 32. The salt of drafts before them is another, and
/// gQUIC is not IETF packet protection at all, so neither is recognised;
/// nor is a version that comes later, until rustls knows its salt.
pub(crate) fn quic_version(version: u32) -> Option<Version> {
    match version {
        1 => Some(Version::V1),
        0x6b33_43cf => Some(Version::V2),
        0xff00_001d..=0xff00_0020 => Some(Version::V1Draft),
        _ => None,
    }
}

/// The Initial type of `version` in the first byte's bits 4 and 5: v2
/// numbers its long-header packet types differently (RFC 9369 §3.2).
fn initial_kind(version: Version) -> u8 {
    match version {
        Version::V2 => 0b01,
        _ => 0b00,
    }
}

/// The suite Initial packets are always sealed with (RFC 9001 §5.2).
pub(crate) fn initial_suite() -> rustls::quic::Suite {
    rustls::crypto::ring::cipher_suite::TLS13_AES_128_GCM_SHA256
        .tls13()
        .and_then(|suite| suite.quic_suite())
        .expect("AES-128-GCM is a TLS 1.3 suite with QUIC Initial keys")
}

/// A variable-length integer (RFC 9000 §16): its value and how many bytes
/// it took, or nothing where the datagram ends inside it.
fn varint(buf: &[u8], at: usize) -> Option<(u64, usize)> {
    let first = *buf.get(at)?;
    // the first two bits give the length: one, two, four or eight bytes
    let len = 1usize << (first >> 6);
    let bytes = buf.get(at..at.checked_add(len)?)?;
    let value = bytes[1..]
        .iter()
        .fold(u64::from(first & 0x3f), |value, byte| {
            (value << 8) | u64::from(*byte)
        });
    Some((value, len))
}

/// Steps 1 to 6 of design 4.2: read the header of the first packet, turn
/// away what cannot be an Initial, and find where its Length ends. Nothing
/// is allocated here.
fn header(datagram: &[u8]) -> Option<Header<'_>> {
    if datagram.len() < MIN_DATAGRAM {
        return None;
    }
    let first = *datagram.first()?;
    // a long header; the QUIC bit (RFC 9000 §17.2's fixed bit) may be clear,
    // as RFC 9287 §3.1 lets a client clear it, so only the long header bit is
    // required here
    if first & 0x80 == 0 {
        return None;
    }
    let version = quic_version(u32::from_be_bytes(datagram.get(1..5)?.try_into().ok()?))?;
    // the type, in bits 4 and 5 of the first byte, is not under header
    // protection
    if (first >> 4) & 0b11 != initial_kind(version) {
        return None;
    }
    let dcid_len = usize::from(*datagram.get(5)?);
    if !(1..=20).contains(&dcid_len) {
        return None;
    }
    let dcid = datagram.get(6..6 + dcid_len)?;
    // the source connection ID, then the token, then the Length
    let mut at = 6 + dcid_len;
    let scid_len = usize::from(*datagram.get(at)?);
    if scid_len > 20 {
        return None;
    }
    at += 1 + scid_len;
    let (token_len, used) = varint(datagram, at)?;
    at = at.checked_add(used)?;
    let token_len = usize::try_from(token_len).ok()?;
    at = at.checked_add(token_len)?;
    let (length, used) = varint(datagram, at)?;
    at = at.checked_add(used)?;
    let length = usize::try_from(length).ok()?;
    // the sample and the four packet number bytes have to be there, and the
    // packet has to end inside the datagram
    if length < MIN_LENGTH || at.checked_add(length)? > datagram.len() {
        return None;
    }
    Some(Header {
        version,
        dcid,
        pn_offset: at,
        length,
    })
}

/// Steps 7 to 9 of design 4.2: does the packet open with the client Initial
/// keys of its own Destination Connection ID? A copy holds the packet up to
/// the end of its Length, so that a coalesced packet after it is not read,
/// and is changed in place: header protection off, then the payload
/// decrypted. The tag is what answers.
fn opens(datagram: &[u8], header: &Header<'_>) -> bool {
    let keys = initial_suite().keys(header.dcid, Side::Server, header.version);
    let mut packet = datagram[..header.pn_offset + header.length].to_vec();
    let sample: [u8; SAMPLE] = packet[header.pn_offset + 4..header.pn_offset + 4 + SAMPLE]
        .try_into()
        .expect("as many sample bytes as the Length promised");
    let mut first = packet[0];
    let mut number = [0u8; 4];
    number.copy_from_slice(&packet[header.pn_offset..header.pn_offset + 4]);
    if keys
        .remote
        .header
        .decrypt_in_place(&sample, &mut first, &mut number)
        .is_err()
    {
        return false;
    }
    // the packet number's length is only known once protection is off
    let number_len = usize::from(first & 0x03) + 1;
    packet[0] = first;
    packet[header.pn_offset..header.pn_offset + number_len].copy_from_slice(&number[..number_len]);
    // the first packet's number is short: what its bytes say is the number
    let packet_number = number[..number_len]
        .iter()
        .fold(0u64, |number, byte| (number << 8) | u64::from(*byte));
    let (aad, payload) = packet.split_at_mut(header.pn_offset + number_len);
    keys.remote
        .packet
        .decrypt_in_place(packet_number, aad, payload)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quic::testing::{
        RFC9001_CLIENT_INITIAL, RFC9369_CLIENT_INITIAL, TestCert, client_initial,
        client_initial_with_quic_bit, unhex,
    };
    use crate::quic::{Transport, client_config};
    use std::time::Duration;

    /// The Destination Connection ID both RFCs' examples use.
    const DCID: &[u8] = &[0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08];

    #[test]
    fn the_rfc_9001_client_initial_is_one() {
        assert!(is_client_initial(&unhex(RFC9001_CLIENT_INITIAL)));
    }

    #[test]
    fn the_rfc_9369_client_initial_is_one() {
        assert!(is_client_initial(&unhex(RFC9369_CLIENT_INITIAL)));
    }

    /// Every version the builder makes, with a Destination Connection ID of
    /// the RFCs' length, of one byte, and of the longest QUIC allows.
    #[test]
    fn what_the_builder_makes_is_one() {
        for version in [1, 0x6b33_43cf, 0xff00_001d, 0xff00_0020] {
            for dcid in [DCID, &[0x01u8][..], &[0xabu8; 20][..]] {
                let datagram = client_initial(version, dcid);
                assert_eq!(datagram.len(), 1200, "{version:#x} {}", dcid.len());
                assert!(
                    is_client_initial(&datagram),
                    "{version:#x} with a Destination Connection ID of {} bytes",
                    dcid.len()
                );
            }
        }
    }

    /// RFC 9287 §3.1 lets a client clear the QUIC bit (RFC 9000 §17.2's
    /// fixed bit) in its Initial packets, so one with a clear bit is still an
    /// Initial; setting the bit afterwards changes the header the tag covers.
    #[test]
    fn a_cleared_quic_bit_is_still_one() {
        for version in [1, 0x6b33_43cf, 0xff00_001d] {
            let datagram = client_initial_with_quic_bit(version, DCID, false);
            assert_eq!(datagram[0] & 0x40, 0, "{version:#x}: the QUIC bit clear");
            assert!(is_client_initial(&datagram), "{version:#x}");
            let mut set = datagram;
            set[0] |= 0x40;
            assert!(
                !is_client_initial(&set),
                "{version:#x}: the header the tag covers changed"
            );
        }
    }

    /// The first packet's Length ends before the bytes after it: a
    /// coalesced packet is not read.
    #[test]
    fn a_coalesced_datagram_is_judged_by_its_first_packet() {
        let mut datagram = unhex(RFC9001_CLIENT_INITIAL);
        datagram.extend_from_slice(&[0xaa; 100]);
        assert!(is_client_initial(&datagram));
    }

    /// quinn's own client, its first datagram taken off a loopback socket
    /// that only receives. The `Connecting` stays until the datagram has
    /// come: quinn may close a connection dropped before its first packet
    /// went out. The handshake is not waited for.
    #[tokio::test]
    async fn a_real_quinn_client_initial_is_one() {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("a loopback socket");
        let addr = socket.local_addr().expect("its address");
        let endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().expect("a loopback address"))
            .expect("a client endpoint");
        let config = client_config(TestCert::new().client_tls(&["h3"]), &Transport::STANDARD)
            .expect("a client configuration");
        let connecting = endpoint
            .connect_with(config, addr, "localhost")
            .expect("a connecting endpoint");
        // room for any datagram: the client pads its Initial to 1200
        let mut buf = vec![0u8; 64 * 1024];
        let (len, _) = tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buf))
            .await
            .expect("a datagram within 5 seconds")
            .expect("a datagram");
        let one = is_client_initial(&buf[..len]);
        drop(connecting);
        drop(endpoint);
        assert!(one, "quinn's first datagram, {len} bytes");
    }

    #[test]
    fn a_changed_payload_byte_is_not_one() {
        let mut datagram = unhex(RFC9001_CLIENT_INITIAL);
        datagram[600] ^= 0x01;
        assert!(!is_client_initial(&datagram), "byte 600");

        let mut datagram = unhex(RFC9001_CLIENT_INITIAL);
        let last = datagram.len() - 1;
        datagram[last] ^= 0x01;
        assert!(!is_client_initial(&datagram), "the last byte");
    }

    /// The keys come from the Destination Connection ID: another one does
    /// not open the packet.
    #[test]
    fn a_changed_dcid_byte_is_not_one() {
        let mut datagram = unhex(RFC9001_CLIENT_INITIAL);
        datagram[6] ^= 0x01;
        assert!(!is_client_initial(&datagram));
    }

    /// `datagram` with its two Length bytes set to `0x7fff`: a value far
    /// past what the datagram holds.
    fn length_too_long(datagram: &[u8]) -> Vec<u8> {
        let mut datagram = datagram.to_vec();
        let at = 8 + DCID.len();
        datagram[at..at + 2].copy_from_slice(&[0x7f, 0xff]);
        datagram
    }

    #[test]
    fn headers_that_are_not_an_initials_are_not_ones() {
        let v1 = client_initial(1, DCID);
        let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();

        cases.push(("1199 bytes", v1[..1199].to_vec()));

        let mut short = v1.clone();
        short[0] &= !0x80;
        cases.push(("a short header", short));

        let mut zero_version = v1.clone();
        zero_version[1..5].copy_from_slice(&0u32.to_be_bytes());
        cases.push(("version 0", zero_version));

        let mut unknown_version = v1.clone();
        unknown_version[1..5].copy_from_slice(&0x0a0a_0a0au32.to_be_bytes());
        cases.push(("an unknown version", unknown_version));

        let mut draft_28 = v1.clone();
        draft_28[1..5].copy_from_slice(&0xff00_001cu32.to_be_bytes());
        cases.push(("draft 28", draft_28));

        let mut v2_typed_as_v1 = client_initial(0x6b33_43cf, DCID);
        v2_typed_as_v1[0] &= !0x30;
        cases.push(("v2's version with v1's Initial type", v2_typed_as_v1));

        let mut v1_typed_as_v2 = v1.clone();
        v1_typed_as_v2[0] = (v1_typed_as_v2[0] & !0x30) | 0x10;
        cases.push(("v1's version with v2's Initial type", v1_typed_as_v2));

        cases.push((
            "a Destination Connection ID of length 0",
            client_initial(1, &[]),
        ));
        cases.push((
            "a Destination Connection ID of length 21",
            client_initial(1, &[0xab; 21]),
        ));

        cases.push(("a Length past the datagram", length_too_long(&v1)));

        // `0xc3 0 0 0 1` padded to 1200 bytes: what the engine's tests once
        // counted as a QUIC Initial
        let mut old_fake = vec![0xc3u8, 0, 0, 0, 1];
        old_fake.resize(1200, 0);
        cases.push(("the old fake Initial", old_fake));

        for (what, datagram) in cases {
            assert!(!is_client_initial(&datagram), "{what}");
        }
    }

    /// A datagram too short to be a client's Initial, at every length.
    #[test]
    fn no_prefix_of_an_initial_is_one() {
        let v1 = unhex(RFC9001_CLIENT_INITIAL);
        for len in 0..v1.len() {
            assert!(!is_client_initial(&v1[..len]), "{len} bytes");
        }
    }

    /// Every single-bit change in the first 64 bytes — version, type,
    /// lengths, packet number, the start of the ciphertext — leaves
    /// something that is not an Initial, and none of them panics.
    #[test]
    fn no_bit_flip_in_the_first_64_bytes_makes_one() {
        let v1 = unhex(RFC9001_CLIENT_INITIAL);
        for at in 0..64 {
            for bit in 0..8 {
                let mut datagram = v1.clone();
                datagram[at] ^= 1 << bit;
                assert!(!is_client_initial(&datagram), "byte {at}, bit {bit}");
            }
        }
    }

}
