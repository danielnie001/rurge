//! Gecko (Hysteria 2 protocol, "Gecko"; phase 2 M7 design 5.4): Salamander
//! with the handshake reshaped (`extras/obfs/gecko.go`,
//! `extras/obfs/gecko_frame.go` in the reference).
//!
//! - A packet with a short header — its first byte's top bit clear — goes
//!   as Salamander alone.
//! - One with a long header — the handshake's, and only they — goes in 2 to
//!   8 fragments, each a frame sealed apart: `0x80 ‖ message ‖ index << 4 |
//!   count ‖ u16 padding ‖ padding ‖ chunk`. The padding is random, so that
//!   the fragment's datagram comes to 512 to 1200 bytes (the reference's
//!   defaults: Surge has no parameters for them).
//! - Whatever opens to a first byte with the top bit set is such a frame: a
//!   long-header packet never goes whole. A frame that breaks a rule is
//!   dropped; the others wait, by source and message, for the rest of their
//!   packet, which then goes on whole.

use super::salamander::{SALT, Salamander};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// The top bit of the first byte: a QUIC packet's long header, or a frame.
const TOP: u8 = 0x80;
/// `0x80 ‖ message ‖ index << 4 | count ‖ u16 padding`.
const HEADER: usize = 5;
const MIN_FRAGMENTS: u8 = 2;
const MAX_FRAGMENTS: u8 = 8;
/// What a fragment's datagram is padded to, its salt included: at least,
/// and at most.
const MIN_SIZE: usize = 512;
const MAX_SIZE: usize = 1200;
/// Packets put together at a time: one more drops the oldest.
const PENDING: usize = 8;
/// How long the fragments of a packet wait for the rest.
const TTL: Duration = Duration::from_secs(8);

/// Whether `packet` has a long header: one of QUIC's handshake.
pub(super) fn is_long_header(packet: &[u8]) -> bool {
    packet.first().is_some_and(|first| first & TOP != 0)
}

/// How many fragments a long-header packet goes in: 2 to 8, at random.
pub(super) fn fragment_count() -> u8 {
    MIN_FRAGMENTS + below(usize::from(MAX_FRAGMENTS - MIN_FRAGMENTS) + 1) as u8
}

/// A random number below `n` (`n > 0`); 0 without a random source.
fn below(n: usize) -> usize {
    getrandom::u32().map_or(0, |random| random as usize % n)
}

/// The datagrams of `packet`, a long-header one, in `count` fragments of
/// message `message`, each sealed apart. The parts are `len / count` bytes
/// long, the last taking the rest, as the reference splits them.
pub(super) fn fragments(
    salamander: &Salamander,
    packet: &[u8],
    count: u8,
    message: u8,
) -> Vec<Vec<u8>> {
    let size = packet.len() / usize::from(count);
    (0..count)
        .map(|index| {
            let start = usize::from(index) * size;
            let end = if index + 1 == count {
                packet.len()
            } else {
                start + size
            };
            let chunk = &packet[start..end];
            let padding = padding(chunk.len());
            let mut datagram = Vec::with_capacity(SALT + HEADER + padding + chunk.len());
            datagram.resize(SALT, 0);
            datagram.extend_from_slice(&[TOP, message, index << 4 | count]);
            datagram.extend_from_slice(&(padding as u16).to_be_bytes());
            let at = datagram.len();
            datagram.resize(at + padding, 0);
            // seen through the mask, a padding of zeros would show the key
            let _ = getrandom::fill(&mut datagram[at..]);
            datagram.extend_from_slice(chunk);
            salamander.seal(&mut datagram);
            datagram
        })
        .collect()
}

/// The padding of a chunk of `len` bytes: its datagram comes to a random
/// size of 512 to 1200 bytes, never less than salt, header and chunk; none
/// when they alone pass 1200.
fn padding(len: usize) -> usize {
    let bare = SALT + HEADER + len;
    let least = bare.max(MIN_SIZE);
    if least > MAX_SIZE {
        return 0;
    }
    least - bare + below(MAX_SIZE - least + 1)
}

/// A frame as it came.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Frame<'a> {
    pub(super) message: u8,
    pub(super) index: u8,
    pub(super) count: u8,
    pub(super) chunk: &'a [u8],
}

/// The frame `bytes` hold — a datagram opened to a first byte with the top
/// bit set —, read as the reference reads one: 2 to 8 fragments, the index
/// below the count, the padding within the frame; `None` for one that
/// breaks a rule. The other bits of the first byte are not looked at.
pub(super) fn parse(bytes: &[u8]) -> Option<Frame<'_>> {
    let header: &[u8; HEADER] = bytes.first_chunk()?;
    let (index, count) = (header[2] >> 4, header[2] & 0x0f);
    if header[0] & TOP == 0 || !(MIN_FRAGMENTS..=MAX_FRAGMENTS).contains(&count) || index >= count {
        return None;
    }
    let padding = usize::from(u16::from_be_bytes([header[3], header[4]]));
    Some(Frame {
        message: header[1],
        index,
        count,
        chunk: bytes.get(HEADER + padding..)?,
    })
}

/// A packet being put together.
struct Pending {
    source: SocketAddr,
    message: u8,
    /// The chunks so far, by index: as many places as fragments.
    chunks: Vec<Option<Vec<u8>>>,
    missing: usize,
    since: Instant,
}

/// The fragments that wait for the rest of their packet (design 5.4): by
/// source and message, at most 8 packets at a time — one more drops the
/// oldest, the likeliest to have lost a fragment —, each for at most 8
/// seconds. A fragment seen already, or one that gives its message another
/// count than the first did, is dropped.
#[derive(Default)]
pub(super) struct Reassembly {
    /// In the order they began: the oldest first.
    pending: Vec<Pending>,
}

impl Reassembly {
    /// The packet `frame`, come from `source` at `now`, makes whole; `None`
    /// while one of its fragments is missing.
    pub(super) fn feed(
        &mut self,
        now: Instant,
        source: SocketAddr,
        frame: &Frame<'_>,
    ) -> Option<Vec<u8>> {
        self.pending
            .retain(|pending| now.duration_since(pending.since) < TTL);
        let count = usize::from(frame.count);
        let at = match self
            .pending
            .iter()
            .position(|pending| pending.source == source && pending.message == frame.message)
        {
            Some(at) => at,
            None => {
                if self.pending.len() == PENDING {
                    self.pending.remove(0);
                }
                self.pending.push(Pending {
                    source,
                    message: frame.message,
                    chunks: vec![None; count],
                    missing: count,
                    since: now,
                });
                self.pending.len() - 1
            }
        };
        let pending = &mut self.pending[at];
        if pending.chunks.len() != count {
            return None;
        }
        let chunk = &mut pending.chunks[usize::from(frame.index)];
        if chunk.is_some() {
            return None;
        }
        *chunk = Some(frame.chunk.to_vec());
        pending.missing -= 1;
        if pending.missing > 0 {
            return None;
        }
        let chunks: Vec<Vec<u8>> = self
            .pending
            .remove(at)
            .chunks
            .into_iter()
            .flatten()
            .collect();
        Some(chunks.concat())
    }

    /// Test support: how many packets are being put together.
    #[cfg(test)]
    pub(super) fn pending(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn salamander() -> Salamander {
        Salamander::new(b"gecko")
    }

    fn source() -> SocketAddr {
        "192.0.2.1:443".parse().unwrap()
    }

    /// A long-header packet of `len` bytes.
    fn long_packet(len: usize) -> Vec<u8> {
        let mut packet: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        packet[0] = 0xc3;
        packet
    }

    /// What each datagram opens to.
    fn opened(datagrams: &[Vec<u8>]) -> Vec<Vec<u8>> {
        datagrams
            .iter()
            .map(|datagram| {
                let mut datagram = datagram.clone();
                salamander().open(&mut datagram).expect("a frame").to_vec()
            })
            .collect()
    }

    /// Byte for byte, each fragment is `0x80`, the message, its index and
    /// the count, the padding's length, the padding, then its part of the
    /// packet: `len / count` bytes, the last fragment the rest.
    #[test]
    fn a_fragment_is_its_header_its_padding_and_a_part_of_the_packet() {
        let packet = long_packet(1250);
        for count in MIN_FRAGMENTS..=MAX_FRAGMENTS {
            let datagrams = fragments(&salamander(), &packet, count, 0x5a);
            assert_eq!(datagrams.len(), usize::from(count));
            let size = packet.len() / usize::from(count);
            let mut whole = Vec::new();
            for (index, frame) in opened(&datagrams).iter().enumerate() {
                assert_eq!(frame[..3], [0x80, 0x5a, (index as u8) << 4 | count]);
                let padding = usize::from(u16::from_be_bytes([frame[3], frame[4]]));
                let chunk = &frame[5 + padding..];
                let expected = if index + 1 == usize::from(count) {
                    packet.len() - index * size
                } else {
                    size
                };
                assert_eq!(chunk.len(), expected, "{count} fragments, #{index}");
                whole.extend_from_slice(chunk);
            }
            assert_eq!(whole, packet);
        }
    }

    /// Padded to 512 to 1200 bytes, at random; a chunk that leaves no room
    /// goes without padding.
    #[test]
    fn a_fragments_datagram_has_512_to_1200_bytes_unless_its_chunk_is_longer() {
        let mut sizes = BTreeSet::new();
        for _ in 0..50 {
            for datagram in fragments(&salamander(), &long_packet(100), 8, 0) {
                assert!(
                    (MIN_SIZE..=MAX_SIZE).contains(&datagram.len()),
                    "{}",
                    datagram.len()
                );
                sizes.insert(datagram.len());
            }
        }
        assert!(sizes.len() > 10, "{sizes:?}");
        // salt, header and chunk: exactly 1200, then past it
        for (len, size) in [(2 * 1187, 1200), (2 * 1188, 1201), (3000, 1513)] {
            let datagrams = fragments(&salamander(), &long_packet(len), 2, 0);
            assert!(datagrams.iter().all(|d| d.len() == size), "{len}");
            for frame in opened(&datagrams) {
                assert_eq!(frame[3..5], [0, 0]);
            }
        }
    }

    #[test]
    fn a_long_header_packet_goes_in_2_to_8_fragments_at_random() {
        let counts: BTreeSet<u8> = (0..500).map(|_| fragment_count()).collect();
        assert_eq!(counts, (2..=8).collect());
        assert!(is_long_header(&[0xc0]) && is_long_header(&[0x80]));
        assert!(!is_long_header(&[0x40]) && !is_long_header(&[]));
    }

    /// The reference's rules (`decodeFrame`): a count of 2 to 8, the index
    /// below it, the padding within the frame. The other bits of the first
    /// byte are not looked at.
    #[test]
    fn frames_that_break_a_rule_are_dropped() {
        let frame = |head: [u8; 3], padding: u16, rest: &[u8]| {
            [&head[..], &padding.to_be_bytes(), rest].concat()
        };
        assert_eq!(
            parse(&frame([0x80, 7, 0x12], 2, b"..chunk")),
            Some(Frame {
                message: 7,
                index: 1,
                count: 2,
                chunk: b"chunk"
            })
        );
        assert_eq!(
            parse(&frame([0xff, 7, 0x78], 0, b"x")).map(|f| f.index),
            Some(7)
        );
        // the padding to the end: an empty chunk
        assert_eq!(
            parse(&frame([0x80, 7, 0x02], 3, b"...")).map(|f| f.chunk.len()),
            Some(0)
        );
        for bad in [
            frame([0x7f, 7, 0x02], 0, b"x"),
            frame([0x80, 7, 0x00], 0, b"x"),
            frame([0x80, 7, 0x01], 0, b"x"),
            frame([0x80, 7, 0x09], 0, b"x"),
            frame([0x80, 7, 0x22], 0, b"x"),
            frame([0x80, 7, 0x82], 0, b"x"),
            frame([0x80, 7, 0x02], 4, b"..."),
            vec![0x80, 7, 0x02, 0],
            Vec::new(),
        ] {
            assert_eq!(parse(&bad), None, "{bad:?}");
        }
    }

    /// Feeds each datagram of `datagrams` from `source` at `now`, and says
    /// what came whole.
    fn feed(
        reassembly: &mut Reassembly,
        now: Instant,
        source: SocketAddr,
        datagrams: &[Vec<u8>],
    ) -> Vec<Vec<u8>> {
        opened(datagrams)
            .iter()
            .filter_map(|frame| reassembly.feed(now, source, &parse(frame).expect("a frame")))
            .collect()
    }

    #[test]
    fn fragments_make_their_packet_whole_in_any_order() {
        let (one, two) = (long_packet(1200), long_packet(300));
        let first = fragments(&salamander(), &one, 5, 1);
        let second = fragments(&salamander(), &two, 3, 2);
        // backwards, the two packets' fragments taking turns
        let arrivals: Vec<Vec<u8>> = [
            &first[4], &second[2], &first[3], &second[0], &first[2], &second[1], &first[1],
            &first[0],
        ]
        .into_iter()
        .cloned()
        .collect();
        let mut reassembly = Reassembly::default();
        let now = Instant::now();
        assert_eq!(feed(&mut reassembly, now, source(), &arrivals), [two, one]);
        assert_eq!(reassembly.pending(), 0);
    }

    /// The first copy of a fragment counts; a fragment that gives its
    /// message another count is dropped. Once the packet went on, a copy
    /// come late begins another, which never comes whole.
    #[test]
    fn copies_and_fragments_of_another_count_are_dropped() {
        let packet = long_packet(1000);
        let datagrams = fragments(&salamander(), &packet, 2, 9);
        let frames = opened(&datagrams);
        let mut reassembly = Reassembly::default();
        let now = Instant::now();
        let first = parse(&frames[0]).unwrap();
        assert_eq!(reassembly.feed(now, source(), &first), None);
        let mut changed = frames[0].clone();
        *changed.last_mut().unwrap() ^= 0xff;
        assert_eq!(
            reassembly.feed(now, source(), &parse(&changed).unwrap()),
            None
        );
        let mut other_count = frames[1].clone();
        other_count[2] = 0x13;
        assert_eq!(
            reassembly.feed(now, source(), &parse(&other_count).unwrap()),
            None
        );
        let second = parse(&frames[1]).unwrap();
        assert_eq!(reassembly.feed(now, source(), &second), Some(packet));
        assert_eq!(reassembly.feed(now, source(), &second), None);
        assert_eq!(reassembly.pending(), 1);
    }

    /// A packet that misses a fragment waits 8 seconds, then is dropped:
    /// its fragment come later begins another packet.
    #[test]
    fn a_packet_missing_a_fragment_is_dropped_after_8_seconds() {
        let packet = long_packet(800);
        let datagrams = fragments(&salamander(), &packet, 2, 3);
        let start = Instant::now();
        let mut reassembly = Reassembly::default();
        feed(&mut reassembly, start, source(), &datagrams[..1]);
        let late = start + TTL - Duration::from_millis(1);
        assert_eq!(
            feed(&mut reassembly, late, source(), &datagrams[1..]),
            [packet]
        );

        feed(&mut reassembly, start, source(), &datagrams[..1]);
        let too_late = start + TTL;
        assert!(feed(&mut reassembly, too_late, source(), &datagrams[1..]).is_empty());
        assert_eq!(reassembly.pending(), 1, "the late fragment's own");
    }

    /// Eight packets at a time: a ninth drops the oldest, whose last
    /// fragment then makes nothing whole.
    #[test]
    fn a_ninth_packet_under_way_drops_the_oldest() {
        let start = Instant::now();
        let packets: Vec<Vec<u8>> = (0..9).map(|n| long_packet(600 + n)).collect();
        let datagrams: Vec<Vec<Vec<u8>>> = packets
            .iter()
            .enumerate()
            .map(|(message, packet)| fragments(&salamander(), packet, 2, message as u8))
            .collect();
        let mut reassembly = Reassembly::default();
        for (n, datagrams) in datagrams.iter().enumerate() {
            let now = start + Duration::from_millis(n as u64);
            feed(&mut reassembly, now, source(), &datagrams[..1]);
            assert_eq!(reassembly.pending(), (n + 1).min(PENDING));
        }
        let later = start + Duration::from_millis(10);
        for n in [8, 1] {
            assert_eq!(
                feed(&mut reassembly, later, source(), &datagrams[n][1..]),
                [packets[n].clone()]
            );
        }
        assert!(feed(&mut reassembly, later, source(), &datagrams[0][1..]).is_empty());
    }

    /// Two sources' fragments of the same message never mix.
    #[test]
    fn the_fragments_of_two_sources_are_kept_apart() {
        let other: SocketAddr = "192.0.2.2:443".parse().unwrap();
        let (one, two) = (long_packet(700), long_packet(900));
        let (a, b) = (
            fragments(&salamander(), &one, 2, 4),
            fragments(&salamander(), &two, 2, 4),
        );
        let mut reassembly = Reassembly::default();
        let now = Instant::now();
        feed(&mut reassembly, now, source(), &a[..1]);
        feed(&mut reassembly, now, other, &b[..1]);
        assert_eq!(feed(&mut reassembly, now, other, &b[1..]), [two]);
        assert_eq!(feed(&mut reassembly, now, source(), &a[1..]), [one]);
    }
}
