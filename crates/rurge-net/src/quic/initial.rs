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
//! Only the first packet of the datagram is judged here. The frames of a
//! packet that opens are read by `read_first`, and every Initial packet a
//! datagram carries is read, not only the first (phase 4 M5 design 4.4):
//! their CRYPTO frames are joined by offset across the datagrams that
//! follow, and what comes out is the ClientHello whose SNI the rules want.
//! A packet that does not open is skipped; a packet of another version, a
//! Retry, a short header or the datagram's padding ends the walk.

use rustls::Side;
use rustls::quic::{DirectionalKeys, Version};
use std::fmt;

/// The smallest datagram a client's Initial comes in (RFC 9000 §14.1):
/// one that carries an Initial is padded to 1200 bytes.
const MIN_DATAGRAM: usize = 1200;

/// The bytes of a header protection sample (RFC 9001 §5.4.2), and so,
/// together with a 4-byte packet number, the least a Length may be.
const SAMPLE: usize = 16;
const MIN_LENGTH: usize = 4 + SAMPLE;

/// The most of a ClientHello this keeps (phase 4 M5 design 4.5), the same
/// as the prefix TCP sniffing reads.
pub const MAX_HELLO: usize = 64 * 1024;

// Why a ClientHello could not be read, as `Hello::GaveUp` carries it: for
// DEBUG only (the phase 4 / M5a plan's table of reasons).
const CLOSED: &str = "the client closed the connection";
const NOT_ALLOWED: &str = "a frame an Initial may not carry";
const CUT_SHORT: &str = "a frame cut short";
const BEYOND: &str = "CRYPTO data beyond 64 KiB";
const CHANGED: &str = "CRYPTO data that changed when it came again";
const TOO_MANY: &str = "CRYPTO data in more than 32 pieces";
const NO_CLIENT_HELLO: &str = "the first handshake message is no ClientHello";
const TOO_LONG: &str = "a ClientHello longer than 64 KiB";

/// Whether `datagram` begins with a client's QUIC Initial packet (design
/// 4.2): the first packet in it, as far as its Length reaches, has a long
/// header (the QUIC bit may be clear, RFC 9287 §3.1), a version this
/// recognises (v1, v2, drafts 29–32), the Initial type of that version,
/// connection IDs of lengths QUIC allows, and opens with the client Initial
/// keys of its own Destination Connection ID. The frames are read by
/// `read_first`, whose `Some` this is: a ClientHello that cannot be read
/// still makes a QUIC flow (design 4.3). A datagram the header checks turn
/// away costs no allocation and no key.
pub fn is_client_initial(datagram: &[u8]) -> bool {
    read_first(datagram).is_some()
}

/// What reading a datagram found (phase 4 M5 design 4.4): the ClientHello
/// whole, the reader waiting for the rest of it, or why it could not be
/// read — the last for DEBUG only.
#[derive(Debug)]
pub enum Hello {
    /// type 1, the 24-bit length and the body: 4 + length bytes
    Whole(Vec<u8>),
    Partial(HelloReader),
    /// why, for DEBUG only
    GaveUp(&'static str),
}

/// Read the first datagram of a QUIC flow (phase 4 M5 design 4.4). `None`
/// where the datagram's first packet is not a client Initial: the same
/// answer the header checks and the tag give, which is what makes a flow
/// QUIC or not. Where it is one, the first packet's frames are read and so
/// are the frames of every Initial after it in the same datagram, until the
/// datagram ends or a packet says to stop.
///
/// A datagram the header checks turn away costs no allocation and no key.
pub fn read_first(datagram: &[u8]) -> Option<Hello> {
    let header = header(datagram)?;
    let keys = initial_suite()
        .keys(header.dcid, Side::Server, header.version)
        .remote;
    let end = header.pn_offset + header.length;
    let (number, plaintext) = open(&datagram[..end], header.pn_offset, &keys, None)?;
    let wire_version: [u8; 4] = datagram.get(1..5)?.try_into().ok()?;
    let mut reader = HelloReader {
        wire_version,
        version: header.version,
        keys,
        largest: Some(number),
        pieces: Pieces::default(),
    };
    if let Err(why) = reader.pieces.read_frames(&plaintext) {
        return Some(Hello::GaveUp(why));
    }
    Some(reader.walk(datagram, end))
}

/// The rest of a ClientHello whose first datagram did not carry all of it:
/// the keys are the first datagram's, and they open the Initial packets of
/// the datagrams that bring the rest (phase 4 M5 design 4.4).
pub struct HelloReader {
    /// The version as it came on the wire, compared byte for byte: a packet
    /// of another version is where a datagram stops.
    wire_version: [u8; 4],
    version: Version,
    keys: DirectionalKeys,
    /// The largest packet number opened so far (RFC 9000 §A.3).
    largest: Option<u64>,
    pieces: Pieces,
}

impl HelloReader {
    /// Read the next datagram of the flow, from its first packet on.
    pub fn read(self, datagram: &[u8]) -> Hello {
        self.walk(datagram, 0)
    }

    /// Walk the datagram from `at`, reading every Initial packet it opens
    /// and skipping the rest, then say what the ClientHello looks like now.
    fn walk(mut self, datagram: &[u8], at: usize) -> Hello {
        match self.read_packets(datagram, at) {
            Ok(()) => self.finish(),
            Err(why) => Hello::GaveUp(why),
        }
    }

    /// Steps 1 to 6 of design 4.4: one packet at a time, to the end of the
    /// datagram or the first packet that says this datagram does not go on.
    /// A packet that does not open is skipped by its Length; that is not a
    /// reason to give up (it may be another connection's).
    fn read_packets(&mut self, datagram: &[u8], mut at: usize) -> Result<(), &'static str> {
        while at < datagram.len() {
            let first = datagram[at];
            if first & 0x80 == 0 {
                break;
            }
            let Some(version) = at.checked_add(5).and_then(|end| datagram.get(at + 1..end)) else {
                break;
            };
            if version != self.wire_version.as_slice() {
                break;
            }
            let kind = (first >> 4) & 0b11;
            let retry = match self.version {
                Version::V2 => 0b00,
                _ => 0b11,
            };
            if kind == retry {
                break;
            }
            let Some(&dcid_len) = datagram.get(at + 5) else {
                break;
            };
            if dcid_len > 20 {
                break;
            }
            let Some(mut cursor) = at.checked_add(6 + usize::from(dcid_len)) else {
                break;
            };
            let Some(&scid_len) = datagram.get(cursor) else {
                break;
            };
            if scid_len > 20 {
                break;
            }
            let Some(next) = cursor.checked_add(1 + usize::from(scid_len)) else {
                break;
            };
            cursor = next;
            let initial = kind == initial_kind(self.version);
            if initial {
                // only an Initial has a token
                let Ok(token_len) = next_varint(datagram, &mut cursor) else {
                    break;
                };
                let Ok(token_len) = usize::try_from(token_len) else {
                    break;
                };
                let Some(next) = cursor.checked_add(token_len) else {
                    break;
                };
                cursor = next;
            }
            let Ok(length) = next_varint(datagram, &mut cursor) else {
                break;
            };
            let Ok(length) = usize::try_from(length) else {
                break;
            };
            let Some(end) = cursor.checked_add(length) else {
                break;
            };
            if end > datagram.len() {
                break;
            }
            if initial
                && let Some((number, plaintext)) =
                    open(&datagram[at..end], cursor - at, &self.keys, self.largest)
            {
                self.largest = Some(self.largest.map_or(number, |largest| largest.max(number)));
                self.pieces.read_frames(&plaintext)?;
            }
            at = end;
        }
        Ok(())
    }

    /// The whole ClientHello, or the reader still waiting for it: give up
    /// has already been answered where it happened.
    fn finish(self) -> Hello {
        let HelloReader {
            wire_version,
            version,
            keys,
            largest,
            pieces,
        } = self;
        match pieces.whole {
            Some(whole) => {
                let (_, mut bytes) = pieces
                    .pieces
                    .into_iter()
                    .next()
                    .expect("the whole ClientHello starts at offset zero");
                bytes.truncate(whole);
                Hello::Whole(bytes)
            }
            None => Hello::Partial(HelloReader {
                wire_version,
                version,
                keys,
                largest,
                pieces,
            }),
        }
    }
}

impl fmt::Debug for HelloReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HelloReader")
            .field("version", &self.version)
            .field("largest", &self.largest)
            .field("pieces", &self.pieces.pieces.len())
            .field("bytes", &self.pieces.bytes())
            .finish()
    }
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

/// The packet number a truncated one stands for (RFC 9000 §A.3): the one
/// nearest to what the largest packet number opened so far makes expected.
/// No packet number reaches 2⁶², so the additions here do not overflow.
fn packet_number(largest: Option<u64>, truncated: u64, len: usize) -> u64 {
    let expected = largest.map_or(0, |largest| largest + 1);
    let win = 1u64 << (len * 8);
    let hwin = win / 2;
    let mask = win - 1;
    let candidate = (expected & !mask) | truncated;
    if candidate + hwin <= expected && candidate < (1 << 62) - win {
        candidate + win
    } else if candidate > expected + hwin && candidate >= win {
        candidate - win
    } else {
        candidate
    }
}

/// The next variable-length integer at `*at`, which moves past it.
fn next_varint(buf: &[u8], at: &mut usize) -> Result<u64, &'static str> {
    let (value, used) = varint(buf, *at).ok_or(CUT_SHORT)?;
    *at += used;
    Ok(value)
}

/// The CRYPTO bytes that have arrived, as the fewest pieces that hold them
/// (phase 4 M5 design 4.5): sorted by start, disjoint and not touching.
/// Only the bytes that came are stored, so a piece's cost is its bytes.
#[derive(Default)]
struct Pieces {
    pieces: Vec<(u64, Vec<u8>)>,
    /// The length of the ClientHello once its head says how long it is and
    /// all of it has arrived.
    whole: Option<usize>,
}

impl Pieces {
    /// Add the CRYPTO frame's bytes at `offset`, merging them with the
    /// pieces they meet, or say why they cannot be kept.
    fn add(&mut self, offset: u64, data: &[u8]) -> Result<(), &'static str> {
        let len = u64::try_from(data.len()).map_err(|_| BEYOND)?;
        let end = offset.checked_add(len).ok_or(BEYOND)?;
        if end > MAX_HELLO as u64 {
            return Err(BEYOND);
        }
        if data.is_empty() {
            return Ok(());
        }
        // the pieces this one overlaps or abuts have to agree where the two
        // overlap, and all of them become one
        let mut first = None;
        let mut start = offset;
        let mut merged_end = end;
        let mut touched = 0usize;
        for (at, (piece_start, bytes)) in self.pieces.iter().enumerate() {
            let piece_end = piece_start + bytes.len() as u64;
            if *piece_start > end || offset > piece_end {
                continue;
            }
            if first.is_none() {
                first = Some(at);
            }
            touched += 1;
            let from = offset.max(*piece_start);
            let to = end.min(piece_end);
            if from < to {
                let ours = &data[(from - offset) as usize..(to - offset) as usize];
                let theirs = &bytes[(from - *piece_start) as usize..(to - *piece_start) as usize];
                if ours != theirs {
                    return Err(CHANGED);
                }
            }
            start = start.min(*piece_start);
            merged_end = merged_end.max(piece_end);
        }
        let at = first.unwrap_or_else(|| {
            self.pieces
                .partition_point(|(piece_start, _)| *piece_start < offset)
        });
        let mut merged = vec![0u8; (merged_end - start) as usize];
        merged[(offset - start) as usize..(end - start) as usize].copy_from_slice(data);
        for (piece_start, bytes) in self.pieces.drain(at..at + touched) {
            let from = (piece_start - start) as usize;
            merged[from..from + bytes.len()].copy_from_slice(&bytes);
        }
        self.pieces.insert(at, (start, merged));
        if self.pieces.len() > 32 {
            return Err(TOO_MANY);
        }
        self.see_head()
    }

    /// Read an opened Initial's plaintext, frame by frame (phase 4 M5
    /// design 4.4): the frames an Initial may carry are stepped over, or,
    /// for CRYPTO, added; anything else says why there is no ClientHello.
    fn read_frames(&mut self, plaintext: &[u8]) -> Result<(), &'static str> {
        let mut at = 0usize;
        while at < plaintext.len() {
            let kind = plaintext[at];
            at += 1;
            match kind {
                0x00 | 0x01 => {}
                0x02 | 0x03 => {
                    next_varint(plaintext, &mut at)?; // largest acknowledged
                    next_varint(plaintext, &mut at)?; // acknowledgment delay
                    let count = next_varint(plaintext, &mut at)?;
                    next_varint(plaintext, &mut at)?; // first range
                    // the count is the client's: read that many pairs, one
                    // at a time, and never allocate by it
                    for _ in 0..count {
                        next_varint(plaintext, &mut at)?; // gap
                        next_varint(plaintext, &mut at)?; // length
                    }
                    if kind == 0x03 {
                        for _ in 0..3 {
                            next_varint(plaintext, &mut at)?;
                        }
                    }
                }
                0x06 => {
                    let offset = next_varint(plaintext, &mut at)?;
                    let len = next_varint(plaintext, &mut at)?;
                    let len = usize::try_from(len).map_err(|_| CUT_SHORT)?;
                    let end = at.checked_add(len).ok_or(CUT_SHORT)?;
                    let data = plaintext.get(at..end).ok_or(CUT_SHORT)?;
                    self.add(offset, data)?;
                    at = end;
                }
                0x1c => return Err(CLOSED),
                _ => return Err(NOT_ALLOWED),
            }
        }
        Ok(())
    }

    /// What the piece that starts at zero says: whether the bytes are a
    /// ClientHello, how long it is, and whether all of it is here. Only
    /// that piece can hold the first handshake message.
    fn see_head(&mut self) -> Result<(), &'static str> {
        let Some((start, bytes)) = self.pieces.first() else {
            return Ok(());
        };
        if *start != 0 {
            return Ok(());
        }
        if bytes[0] != 0x01 {
            return Err(NO_CLIENT_HELLO);
        }
        if bytes.len() < 4 {
            return Ok(());
        }
        let length = u32::from_be_bytes([0, bytes[1], bytes[2], bytes[3]]) as usize;
        let whole = 4 + length;
        if whole > MAX_HELLO {
            return Err(TOO_LONG);
        }
        if bytes.len() >= whole {
            self.whole = Some(whole);
        }
        Ok(())
    }

    /// How many bytes have arrived.
    fn bytes(&self) -> usize {
        self.pieces.iter().map(|(_, bytes)| bytes.len()).sum()
    }
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

/// Steps 7 to 9 of design 4.2: does `packet` open with `keys`? The packet
/// runs from its first byte to the end of its Length; `pn_offset` is where
/// its packet number starts. A copy is changed in place: header protection
/// off, the packet number restored against `largest`, then the payload
/// decrypted. The tag is what answers; the plaintext comes back with the
/// number it was sealed under.
fn open(
    packet: &[u8],
    pn_offset: usize,
    keys: &DirectionalKeys,
    largest: Option<u64>,
) -> Option<(u64, Vec<u8>)> {
    // the sample and the four packet number bytes have to be there, which
    // the first packet's Length has already promised
    if packet.len() < pn_offset.checked_add(MIN_LENGTH)? {
        return None;
    }
    let mut packet = packet.to_vec();
    let sample: [u8; SAMPLE] = packet
        .get(pn_offset + 4..pn_offset + 4 + SAMPLE)?
        .try_into()
        .ok()?;
    let mut first = packet[0];
    let mut number = [0u8; 4];
    number.copy_from_slice(packet.get(pn_offset..pn_offset + 4)?);
    if keys
        .header
        .decrypt_in_place(&sample, &mut first, &mut number)
        .is_err()
    {
        return None;
    }
    // the packet number's length is only known once protection is off
    let number_len = usize::from(first & 0x03) + 1;
    packet[0] = first;
    packet[pn_offset..pn_offset + number_len].copy_from_slice(&number[..number_len]);
    let truncated = number[..number_len]
        .iter()
        .fold(0u64, |number, byte| (number << 8) | u64::from(*byte));
    let number = packet_number(largest, truncated, number_len);
    let (aad, payload) = packet.split_at_mut(pn_offset + number_len);
    let plaintext = keys.packet.decrypt_in_place(number, aad, payload).ok()?;
    Some((number, plaintext.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quic::testing::{
        Frame, Initial, RFC_DCID, RFC9001_CLIENT_INITIAL, RFC9369_CLIENT_INITIAL, TestCert,
        big_client_hello, chaos_frames, chrome_frames, client_hello, client_initial,
        client_initial_with_quic_bit, hello_initials, long_packet, unhex,
    };
    use crate::quic::{Transport, client_config};
    use std::time::Duration;
    /// The Destination Connection ID both RFCs' examples use.
    const DCID: &[u8] = &[0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08];

    #[test]
    fn the_rfc_9001_client_initial_is_one() {
        assert!(is_client_initial(&unhex(RFC9001_CLIENT_INITIAL)));
    }

    /// RFC 9000 §A.3's example, and the ends of the one-byte window.
    #[test]
    fn packet_numbers_decode_as_rfc_9000_a3() {
        assert_eq!(packet_number(Some(0xa82f_30ea), 0x9b32, 2), 0xa82f_9b32);
        assert_eq!(packet_number(None, 2, 4), 2);
        assert_eq!(packet_number(Some(0), 1, 1), 1);
        assert_eq!(packet_number(Some(255), 0, 1), 256);
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

    /// A ClientHello of six bytes: type 1, a length of two, two bytes of
    /// body. Enough for tests that are about the pieces and not the TLS.
    const TINY: &[u8] = &[0x01, 0x00, 0x00, 0x02, 0xaa, 0xbb];

    /// The big ClientHello `held.test` gets, and the two datagrams that
    /// carry it from offset 600 on.
    fn two_datagrams() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let hello = big_client_hello(Some("held.test"));
        let [d1, d2] = hello_initials(RFC_DCID, &hello, 600);
        (hello, d1, d2)
    }

    /// The reader the first of two datagrams leaves waiting.
    fn partial(d1: &[u8]) -> HelloReader {
        match read_first(d1) {
            Some(Hello::Partial(reader)) => reader,
            _ => panic!("the first datagram did not leave a reader waiting"),
        }
    }

    fn why(gave: Hello) -> &'static str {
        match gave {
            Hello::GaveUp(why) => why,
            _ => panic!("reading did not give up"),
        }
    }

    #[test]
    fn the_rfc_9001_client_initial_reads_whole() {
        let Some(Hello::Whole(message)) = read_first(&unhex(RFC9001_CLIENT_INITIAL)) else {
            panic!("the RFC 9001 Initial did not come whole");
        };
        assert_eq!(message.len(), 241);
        assert_eq!(&message[..4], &[0x01, 0x00, 0x00, 0xed]);
        assert!(
            message.windows(11).any(|window| window == b"example.com"),
            "the Server Name is in the message"
        );
    }

    #[test]
    fn the_rfc_9369_client_initial_reads_whole() {
        let whole = |hex: &str| match read_first(&unhex(hex)) {
            Some(Hello::Whole(message)) => message,
            _ => panic!("a sample Initial did not come whole"),
        };
        assert_eq!(whole(RFC9369_CLIENT_INITIAL), whole(RFC9001_CLIENT_INITIAL));
    }

    #[test]
    fn a_hello_across_two_datagrams_comes_whole() {
        let (hello, d1, d2) = two_datagrams();
        let reader = partial(&d1);
        match reader.read(&d2) {
            Hello::Whole(message) => assert_eq!(message, hello),
            _ => panic!("the second datagram did not finish the ClientHello"),
        }
    }

    #[test]
    fn the_second_half_first() {
        let hello = big_client_hello(Some("held.test"));
        let first = Initial::new(
            1,
            RFC_DCID,
            0,
            vec![Frame::Crypto {
                offset: 600,
                data: hello[600..].to_vec(),
            }],
        )
        .seal(Some(1200));
        let second = Initial::new(
            1,
            RFC_DCID,
            1,
            vec![Frame::Crypto {
                offset: 0,
                data: hello[..600].to_vec(),
            }],
        )
        .seal(Some(1200));
        let reader = partial(&first);
        match reader.read(&second) {
            Hello::Whole(message) => assert_eq!(message, hello),
            _ => panic!("the halves did not come together"),
        }
    }

    #[test]
    fn chrome_arrangements_come_whole() {
        for seed in 0..32u64 {
            let hello = big_client_hello(Some("held.test"));
            let [first, second] = chrome_frames(&hello, seed);
            let d1 = Initial::new(1, RFC_DCID, 0, first).seal(Some(1200));
            let d2 = Initial::new(1, RFC_DCID, 1, second).seal(Some(1200));
            let reader = partial(&d1);
            match reader.read(&d2) {
                Hello::Whole(message) => assert_eq!(message, hello, "seed {seed}"),
                _ => panic!("seed {seed}: Chrome's arrangement did not come whole"),
            }
        }
    }

    #[test]
    fn chaos_in_one_packet_comes_whole() {
        let small = client_hello(Some("held.test"), &[b"h3"]);
        for seed in 0..64u64 {
            let frames = chaos_frames(&[(0, &small)], seed);
            let datagram = Initial::new(1, RFC_DCID, 0, frames).seal(Some(1200));
            match read_first(&datagram) {
                Some(Hello::Whole(message)) => assert_eq!(message, small, "seed {seed}"),
                _ => panic!("seed {seed}: the chaos frames did not come whole"),
            }
        }
    }

    #[test]
    fn two_initials_in_one_datagram_come_whole() {
        let small = client_hello(Some("held.test"), &[b"h3"]);
        let first = Initial::new(
            1,
            RFC_DCID,
            0,
            vec![Frame::Crypto {
                offset: 0,
                data: small[..100].to_vec(),
            }],
        )
        .seal(None);
        let second = Initial::new(
            1,
            RFC_DCID,
            1,
            vec![Frame::Crypto {
                offset: 100,
                data: small[100..].to_vec(),
            }],
        )
        .seal(None);
        let datagram = crate::quic::testing::datagram(&[first, second], 1200);
        match read_first(&datagram) {
            Some(Hello::Whole(message)) => assert_eq!(message, small),
            _ => panic!("two Initials in one datagram did not come whole"),
        }
    }

    #[test]
    fn an_initial_then_a_0rtt_packet() {
        let small = client_hello(Some("held.test"), &[b"h3"]);
        let whole = Initial::new(
            1,
            RFC_DCID,
            0,
            vec![Frame::Crypto {
                offset: 0,
                data: small.clone(),
            }],
        )
        .seal(None);
        let datagram =
            crate::quic::testing::datagram(&[whole, long_packet(1, 0b01, RFC_DCID, 100)], 1200);
        match read_first(&datagram) {
            Some(Hello::Whole(message)) => assert_eq!(message, small),
            _ => panic!("the Initial before the 0-RTT packet did not come whole"),
        }

        let first = Initial::new(
            1,
            RFC_DCID,
            0,
            vec![Frame::Crypto {
                offset: 0,
                data: small[..100].to_vec(),
            }],
        )
        .seal(None);
        let last = Initial::new(
            1,
            RFC_DCID,
            1,
            vec![Frame::Crypto {
                offset: 100,
                data: small[100..].to_vec(),
            }],
        )
        .seal(None);
        let datagram = crate::quic::testing::datagram(
            &[first, long_packet(1, 0b01, RFC_DCID, 100), last],
            1200,
        );
        match read_first(&datagram) {
            Some(Hello::Whole(message)) => assert_eq!(message, small),
            _ => panic!("the walk past the 0-RTT packet did not come whole"),
        }
    }

    #[test]
    fn trailing_zeros_end_the_walk() {
        let (hello, _, d2) = two_datagrams();
        let first = Initial::new(
            1,
            RFC_DCID,
            0,
            vec![Frame::Crypto {
                offset: 0,
                data: hello[..600].to_vec(),
            }],
        )
        .seal(None);
        let d1 = crate::quic::testing::datagram(&[first], 1300);
        let reader = partial(&d1);
        match reader.read(&d2) {
            Hello::Whole(message) => assert_eq!(message, hello),
            _ => panic!("the zeros after the packet ended the ClientHello"),
        }
    }

    #[test]
    fn a_later_datagram_with_another_dcid_still_opens() {
        let (hello, d1, _) = two_datagrams();
        let second = Initial {
            dcid: vec![0x42; 8],
            ..Initial::new(
                1,
                RFC_DCID,
                1,
                vec![Frame::Crypto {
                    offset: 600,
                    data: hello[600..].to_vec(),
                }],
            )
        }
        .seal(Some(1200));
        let reader = partial(&d1);
        match reader.read(&second) {
            Hello::Whole(message) => assert_eq!(message, hello),
            _ => panic!("another Destination Connection ID stopped the read"),
        }
    }

    #[test]
    fn a_retransmitted_piece_is_taken() {
        let (hello, d1, d2) = two_datagrams();
        let reader = partial(&d1);
        let reader = match reader.read(&d1) {
            Hello::Partial(reader) => reader,
            _ => panic!("the same datagram again was not taken"),
        };
        match reader.read(&d2) {
            Hello::Whole(message) => assert_eq!(message, hello),
            _ => panic!("the retransmission kept the ClientHello from coming whole"),
        }
    }

    #[test]
    fn overlapping_pieces_that_agree_merge() {
        let hello = big_client_hello(Some("held.test"));
        let first = Initial::new(
            1,
            RFC_DCID,
            0,
            vec![Frame::Crypto {
                offset: 0,
                data: hello[..700].to_vec(),
            }],
        )
        .seal(Some(1200));
        let second = Initial::new(
            1,
            RFC_DCID,
            1,
            vec![Frame::Crypto {
                offset: 600,
                data: hello[600..].to_vec(),
            }],
        )
        .seal(Some(1200));
        let reader = partial(&first);
        match reader.read(&second) {
            Hello::Whole(message) => assert_eq!(message, hello),
            _ => panic!("the overlapping pieces did not merge"),
        }
    }

    #[test]
    fn thirty_two_pieces_are_read_and_the_thirty_third_gives_up() {
        let frames: Vec<Frame> = (0..32u64)
            .map(|at| Frame::Crypto {
                offset: 10 + at * 2,
                data: vec![0x5a],
            })
            .collect();
        let first = Initial::new(1, RFC_DCID, 0, frames).seal(Some(1200));
        let reader = partial(&first);
        let third = Initial::new(
            1,
            RFC_DCID,
            1,
            vec![Frame::Crypto {
                offset: 200,
                data: vec![0x5a],
            }],
        )
        .seal(Some(1200));
        assert_eq!(why(reader.read(&third)), TOO_MANY);
    }

    #[test]
    fn the_ways_to_give_up() {
        let (hello, d1, _) = two_datagrams();

        let mut changed = hello[500..700].to_vec();
        changed[0] ^= 0x01;
        let bad = Initial::new(
            1,
            RFC_DCID,
            1,
            vec![Frame::Crypto {
                offset: 500,
                data: changed,
            }],
        )
        .seal(Some(1200));
        assert_eq!(why(partial(&d1).read(&bad)), CHANGED);

        let closed = Initial::new(
            1,
            RFC_DCID,
            1,
            vec![Frame::ConnectionClose {
                error: 0,
                frame_type: 0,
                reason: b"bye".to_vec(),
            }],
        )
        .seal(Some(1200));
        assert_eq!(why(partial(&d1).read(&closed)), CLOSED);

        let stream = Initial::new(
            1,
            RFC_DCID,
            1,
            vec![Frame::Raw(vec![0x08, 0x00, 0x01, 0x61])],
        )
        .seal(Some(1200));
        assert_eq!(why(partial(&d1).read(&stream)), NOT_ALLOWED);

        // a Ping first, and four bytes of packet number so that seal adds no
        // PADDING: the plaintext ends where the cut frame does
        for cut in [
            vec![0x06, 0x00],
            vec![0x06, 0x00, 0x10, 0x01],
            vec![0x02, 0x05, 0x00],
        ] {
            let bad = Initial {
                number_len: 4,
                ..Initial::new(1, RFC_DCID, 1, vec![Frame::Ping, Frame::Raw(cut.clone())])
            }
            .seal(None);
            assert_eq!(why(partial(&d1).read(&bad)), CUT_SHORT, "{cut:?}");
        }

        let not_a_hello = Initial::new(
            1,
            RFC_DCID,
            0,
            vec![Frame::Crypto {
                offset: 0,
                data: vec![2, 0, 0, 4, 0, 0, 0, 0],
            }],
        )
        .seal(Some(1200));
        assert!(is_client_initial(&not_a_hello), "still QUIC");
        assert_eq!(
            why(read_first(&not_a_hello).expect("an Initial")),
            NO_CLIENT_HELLO
        );

        let too_long = Initial::new(
            1,
            RFC_DCID,
            0,
            vec![Frame::Crypto {
                offset: 0,
                data: vec![1, 0x01, 0x00, 0x00],
            }],
        )
        .seal(Some(1200));
        assert_eq!(why(read_first(&too_long).expect("an Initial")), TOO_LONG);

        let beyond = Initial::new(
            1,
            RFC_DCID,
            1,
            vec![Frame::Crypto {
                offset: 65_530,
                data: vec![0; 10],
            }],
        )
        .seal(Some(1200));
        assert_eq!(why(partial(&d1).read(&beyond)), BEYOND);
    }

    #[test]
    fn an_initial_that_does_not_open_is_skipped() {
        let (hello, d1, d2) = two_datagrams();
        let skipped = Initial {
            keys_from: vec![0x42; 8],
            ..Initial::new(
                1,
                RFC_DCID,
                1,
                vec![Frame::Crypto {
                    offset: 600,
                    data: hello[600..].to_vec(),
                }],
            )
        }
        .seal(Some(1200));
        let reader = partial(&d1);
        let reader = match reader.read(&skipped) {
            Hello::Partial(reader) => reader,
            _ => panic!("a packet sealed with other keys ended the read"),
        };
        match reader.read(&d2) {
            Hello::Whole(message) => assert_eq!(message, hello),
            _ => panic!("the skipped packet kept the ClientHello from coming whole"),
        }
    }

    #[test]
    fn a_packet_of_another_version_ends_the_walk() {
        let (hello, d1, _) = two_datagrams();
        let v2 = Initial {
            version: 0x6b33_43cf,
            ..Initial::new(
                1,
                RFC_DCID,
                1,
                vec![Frame::Crypto {
                    offset: 600,
                    data: hello[600..].to_vec(),
                }],
            )
        }
        .seal(Some(1200));
        assert!(matches!(partial(&d1).read(&v2), Hello::Partial(_)));
    }

    #[test]
    fn a_short_header_datagram_is_nothing() {
        let (_, d1, _) = two_datagrams();
        let mut short = vec![0x40u8];
        short.resize(1200, 0);
        assert!(matches!(partial(&d1).read(&short), Hello::Partial(_)));
    }

    #[test]
    fn a_second_packet_with_a_one_byte_number_opens() {
        let hello = big_client_hello(Some("held.test"));
        let first = Initial {
            number_len: 4,
            ..Initial::new(
                1,
                RFC_DCID,
                0,
                vec![Frame::Crypto {
                    offset: 0,
                    data: hello[..600].to_vec(),
                }],
            )
        }
        .seal(Some(1200));
        let second = Initial {
            number_len: 1,
            ..Initial::new(
                1,
                RFC_DCID,
                1,
                vec![Frame::Crypto {
                    offset: 600,
                    data: hello[600..].to_vec(),
                }],
            )
        }
        .seal(Some(1200));
        let reader = partial(&first);
        match reader.read(&second) {
            Hello::Whole(message) => assert_eq!(message, hello),
            _ => panic!("the one-byte packet number did not open"),
        }
    }

    /// quinn's own client sending a ClientHello too big for one datagram,
    /// taken off a loopback socket that only receives (phase 4 M5 design 16
    /// 3). The `Connecting` stays until the datagrams have come: quinn may
    /// close a connection dropped before its first packet went out. The
    /// handshake is not waited for.
    #[tokio::test]
    async fn a_real_quinn_client_hello_across_two_datagrams() {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("a loopback socket");
        let addr = socket.local_addr().expect("its address");
        let endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().expect("a loopback address"))
            .expect("a client endpoint");
        let names: Vec<String> = (0..64)
            .map(|at| format!("rurge-test-alpn-{at:02}"))
            .collect();
        let alpn: Vec<&str> = names.iter().map(String::as_str).collect();
        let config = client_config(TestCert::new().client_tls(&alpn), &Transport::STANDARD)
            .expect("a client configuration");
        let connecting = endpoint
            .connect_with(config, addr, "localhost")
            .expect("a connecting endpoint");
        // room for any datagram: the client pads its Initial to 1200
        let mut buf = vec![0u8; 64 * 1024];
        let mut reader: Option<HelloReader> = None;
        let mut first_was_partial = false;
        let mut hello = None;
        for count in 0..4 {
            let (len, _) = tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buf))
                .await
                .expect("a datagram within 5 seconds")
                .expect("a datagram");
            let datagram = &buf[..len];
            let read = match reader.take() {
                None => {
                    let read = read_first(datagram);
                    first_was_partial = matches!(&read, Some(Hello::Partial(_)));
                    read
                }
                Some(reader) => Some(reader.read(datagram)),
            };
            match read {
                Some(Hello::Whole(message)) => {
                    hello = Some(message);
                    break;
                }
                Some(Hello::Partial(r)) => reader = Some(r),
                Some(Hello::GaveUp(why)) => {
                    panic!("the ClientHello gave up: {why}, datagram {count}, {len} bytes")
                }
                None => panic!("not an Initial: datagram {count}, {len} bytes"),
            }
        }
        drop(connecting);
        drop(endpoint);
        assert!(
            first_was_partial,
            "the first datagram held the whole ClientHello"
        );
        let hello = hello.expect("the ClientHello within four datagrams");
        assert_eq!(hello[0], 1, "the first handshake message is a ClientHello");
        assert!(
            hello.windows(9).any(|window| window == b"localhost"),
            "the Server Name is in the message"
        );
    }

    #[test]
    fn no_bit_flip_in_a_later_datagram_panics() {
        let (_, d1, d2) = two_datagrams();
        for at in 0..64 {
            for bit in 0..8 {
                let mut bad = d2.clone();
                bad[at] ^= 1 << bit;
                // whatever it answers, it must answer
                let _ = partial(&d1).read(&bad);
            }
        }
    }

    #[test]
    fn no_prefix_of_a_later_datagram_panics() {
        let (_, d1, d2) = two_datagrams();
        for len in 0..d2.len() {
            assert!(
                matches!(partial(&d1).read(&d2[..len]), Hello::Partial(_)),
                "{len} bytes"
            );
        }
    }

    #[test]
    fn the_reader_crosses_await() {
        fn is_send<T: Send>() {}
        is_send::<HelloReader>();
        is_send::<Hello>();
    }

    #[test]
    fn pieces_that_touch_merge_and_the_whole_hello_is_seen() {
        let mut pieces = Pieces::default();
        assert_eq!(pieces.add(4, &TINY[4..]), Ok(()));
        assert_eq!(pieces.pieces, vec![(4u64, TINY[4..].to_vec())]);
        assert_eq!(pieces.whole, None);
        assert_eq!(pieces.add(0, &TINY[..4]), Ok(()));
        assert_eq!(pieces.pieces, vec![(0u64, TINY.to_vec())]);
        assert_eq!(pieces.whole, Some(TINY.len()));
    }

    #[test]
    fn overlapping_pieces_must_agree_byte_for_byte() {
        let mut pieces = Pieces::default();
        assert_eq!(pieces.add(2, &[1, 2, 3]), Ok(()));
        assert_eq!(pieces.add(1, &[0, 1, 2]), Ok(()));
        assert_eq!(pieces.pieces, vec![(1u64, vec![0, 1, 2, 3])]);

        let mut pieces = Pieces::default();
        assert_eq!(pieces.add(0, TINY), Ok(()));
        assert_eq!(pieces.add(1, &[0x00, 0x00, 0x03]), Err(CHANGED));
    }

    #[test]
    fn a_piece_beyond_64_kib_gives_up() {
        let mut pieces = Pieces::default();
        assert_eq!(pieces.add(65_530, &[0u8; 10]), Err(BEYOND));
        assert_eq!(pieces.add(u64::MAX, &[0]), Err(BEYOND));
        assert_eq!(pieces.add(0, &[]), Ok(()));
        assert!(pieces.pieces.is_empty());
    }

    #[test]
    fn more_than_32_pieces_give_up() {
        let mut pieces = Pieces::default();
        for at in 0..32u64 {
            assert_eq!(pieces.add(1 + at * 2, &[0x5a]), Ok(()));
        }
        assert_eq!(pieces.pieces.len(), 32);
        assert_eq!(pieces.add(100, &[0x5a]), Err(TOO_MANY));
    }

    /// PING, CRYPTO and ACK frames, all of them the Initial may carry: the
    /// CRYPTO bytes land, the others are stepped over.
    #[test]
    fn frames_an_initial_may_carry_are_read() {
        let mut plaintext = Vec::new();
        Frame::Ping.encode(&mut plaintext);
        Frame::Padding(3).encode(&mut plaintext);
        Frame::Crypto {
            offset: 0,
            data: TINY.to_vec(),
        }
        .encode(&mut plaintext);
        Frame::Ack {
            largest: 3,
            delay: 2,
            first_range: 1,
            ranges: vec![(0, 0), (1, 2)],
            ecn: Some([1, 2, 3]),
        }
        .encode(&mut plaintext);
        let mut pieces = Pieces::default();
        assert_eq!(pieces.read_frames(&plaintext), Ok(()));
        assert_eq!(pieces.whole, Some(TINY.len()));
        assert_eq!(pieces.pieces, vec![(0u64, TINY.to_vec())]);
    }

    /// A CONNECTION_CLOSE, a frame an Initial may not carry, and a frame
    /// cut short each say why, as DEBUG's reason.
    #[test]
    fn frames_an_initial_may_not_carry_give_up() {
        let mut pieces = Pieces::default();
        let mut close = Vec::new();
        Frame::ConnectionClose {
            error: 0,
            frame_type: 0,
            reason: b"no".to_vec(),
        }
        .encode(&mut close);
        assert_eq!(pieces.read_frames(&close), Err(CLOSED));

        let mut other = Vec::new();
        Frame::Raw(vec![0x08, 0x00, 0x01, 0x61]).encode(&mut other);
        assert_eq!(pieces.read_frames(&other), Err(NOT_ALLOWED));

        for cut in [
            vec![0x06, 0x00],
            vec![0x06, 0x00, 0x10, 0x01],
            vec![0x02, 0x05, 0x00],
        ] {
            let mut pieces = Pieces::default();
            assert_eq!(pieces.read_frames(&cut), Err(CUT_SHORT), "{cut:?}");
        }
    }

    #[test]
    fn the_head_must_be_a_client_hello() {
        let mut pieces = Pieces::default();
        assert_eq!(pieces.add(0, &[0x02, 0, 0, 1, 0]), Err(NO_CLIENT_HELLO));

        let mut pieces = Pieces::default();
        assert_eq!(pieces.add(0, &[0x01, 0x01, 0x00, 0x00]), Err(TOO_LONG));

        // a length of 0xfffc is 4 + 65532 bytes: exactly 64 KiB
        let mut pieces = Pieces::default();
        assert_eq!(pieces.add(0, &[0x01, 0x00, 0xff, 0xfc]), Ok(()));
        assert_eq!(pieces.whole, None);
    }
}
