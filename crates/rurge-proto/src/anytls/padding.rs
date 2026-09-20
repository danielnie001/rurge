//! AnyTLS padding schemes (anytls-go `proxy/padding/padding.go` and
//! `Session.writeConn`): for the first `stop` writes of a session, each write
//! is cut into TLS records of the sizes the scheme lists, and padded with
//! `cmdWaste` frames where the payload runs out.

use super::frame::{self, HEADER};
use md5::{Digest, Md5};
use std::collections::HashMap;

pub(crate) const DEFAULT_SCHEME: &str = "stop=8\n0=30-30\n1=100-400\n2=400-500,c,500-1000,c,500-1000,c,500-1000,c,500-1000\n3=9-9,500-1000\n4=500-1000\n5=500-1000\n6=500-1000\n7=500-1000";

/// Bounds on a scheme a server may push (M2 design 6.3): the text, how far
/// padding may reach into a session, how many records one write may become,
/// and the size of one record (a TLS record holds 2^14 bytes).
const MAX_TEXT: usize = 8192;
const MAX_STOP: u32 = 256;
const MAX_ENTRIES: usize = 64;
const MAX_SIZE: u32 = 16384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    /// A record of `min ..= max - 1` bytes (`min` when the two are equal).
    Size { min: u32, max: u32 },
    /// `c`: stop here when the payload is used up.
    Check,
}

/// One drawn size, or the check mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    Size(usize),
    Check,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Scheme {
    md5: String,
    stop: u32,
    packets: HashMap<u32, Vec<Entry>>,
}

fn parse_entry(text: &str) -> Option<Entry> {
    if text == "c" {
        return Some(Entry::Check);
    }
    let (a, b) = text.split_once('-')?;
    let (a, b): (u32, u32) = (a.parse().ok()?, b.parse().ok()?);
    let (min, max) = (a.min(b), a.max(b));
    (min >= 1 && max <= MAX_SIZE).then_some(Entry::Size { min, max })
}

impl Scheme {
    /// `None` unless every line that matters is well-formed and within the
    /// bounds. Keys that are neither `stop` nor a packet number are ignored.
    pub(crate) fn parse(raw: &[u8]) -> Option<Scheme> {
        if raw.len() > MAX_TEXT {
            return None;
        }
        let text = std::str::from_utf8(raw).ok()?;
        let mut stop = None;
        let mut packets = HashMap::new();
        for line in text.split('\n') {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key == "stop" {
                stop = Some(value.parse::<u32>().ok().filter(|s| *s <= MAX_STOP)?);
            } else if let Ok(packet) = key.parse::<u32>() {
                let entries: Vec<Entry> =
                    value.split(',').map(parse_entry).collect::<Option<_>>()?;
                if entries.len() > MAX_ENTRIES {
                    return None;
                }
                packets.insert(packet, entries);
            }
        }
        Some(Scheme {
            md5: Md5::digest(raw)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            stop: stop?,
            packets,
        })
    }

    pub(crate) fn default_scheme() -> Scheme {
        Scheme::parse(DEFAULT_SCHEME.as_bytes()).expect("the built-in scheme is valid")
    }

    /// Lower-case hex of the MD5 of the scheme's text, as `cmdSettings` reports it.
    pub(crate) fn md5(&self) -> &str {
        &self.md5
    }

    pub(crate) fn stop(&self) -> u32 {
        self.stop
    }

    /// The sizes for the `packet`-th write, drawn with `pick(n)` ∈ `0..n`.
    pub(crate) fn pieces(&self, packet: u32, pick: &mut dyn FnMut(u32) -> u32) -> Vec<Piece> {
        let Some(entries) = self.packets.get(&packet) else {
            return Vec::new();
        };
        entries
            .iter()
            .map(|e| match *e {
                Entry::Check => Piece::Check,
                Entry::Size { min, max } if min == max => Piece::Size(min as usize),
                Entry::Size { min, max } => Piece::Size((min + pick(max - min)) as usize),
            })
            .collect()
    }

    /// The padding that rides with the authentication (packet 0).
    pub(crate) fn auth_padding(&self, pick: &mut dyn FnMut(u32) -> u32) -> usize {
        match self.pieces(0, pick).first() {
            Some(Piece::Size(n)) => *n,
            _ => 0,
        }
    }
}

fn waste(len: usize) -> Vec<u8> {
    frame::frame(frame::WASTE, 0, &vec![0u8; len])
}

/// The records one write becomes: `payload` cut to the listed sizes, a
/// `cmdWaste` frame filling what the payload leaves of a size, and whatever
/// payload is left after the list in one last record.
pub(crate) fn shape(pieces: &[Piece], mut payload: &[u8]) -> Vec<Vec<u8>> {
    let mut records = Vec::new();
    for piece in pieces {
        let size = match piece {
            Piece::Check if payload.is_empty() => break,
            Piece::Check => continue,
            Piece::Size(size) => *size,
        };
        if payload.len() > size {
            records.push(payload[..size].to_vec());
            payload = &payload[size..];
        } else if !payload.is_empty() {
            let mut record = payload.to_vec();
            // the waste frame's own header counts towards the size
            if let Some(fill) = size.checked_sub(payload.len() + HEADER).filter(|n| *n > 0) {
                record.extend_from_slice(&waste(fill));
            }
            records.push(record);
            payload = &[];
        } else {
            records.push(waste(size));
        }
    }
    if !payload.is_empty() {
        records.push(payload.to_vec());
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    fn low(_: u32) -> u32 {
        0
    }

    #[test]
    fn the_default_scheme_and_its_md5() {
        let s = Scheme::default_scheme();
        assert_eq!(s.md5(), "75cff2ad89aadf5e257059ee571ebe11");
        assert_eq!(s.stop(), 8);
        assert_eq!(s.auth_padding(&mut low), 30);
        assert_eq!(s.pieces(3, &mut low), [Piece::Size(9), Piece::Size(500)]);
        assert_eq!(s.pieces(2, &mut low).len(), 9);
        assert_eq!(s.pieces(9, &mut low), []);
        // the upper bound is exclusive, as in the reference
        assert_eq!(s.pieces(1, &mut |n| n - 1), [Piece::Size(399)]);
    }

    #[test]
    fn a_pushed_scheme_is_validated_within_bounds() {
        assert!(Scheme::parse(b"stop=2\n0=10-20\n1=5-5,c,7-9\nfuture-key=1").is_some());
        for bad in [
            &b"0=10-20"[..],      // no stop
            b"stop=x",            // not a number
            b"stop=257",          // reaches too far
            b"stop=2\n1=0-5",     // a size below one
            b"stop=2\n1=1-16385", // more than a TLS record
            b"stop=2\n1=5",       // not a range
            b"stop=2\n1=a-b",
            b"stop=2\n1=\xff", // not UTF-8
        ] {
            assert_eq!(Scheme::parse(bad), None, "{}", String::from_utf8_lossy(bad));
        }
        let many = format!("stop=2\n1={}", vec!["1-2"; 65].join(","));
        assert_eq!(Scheme::parse(many.as_bytes()), None);
        assert_eq!(Scheme::parse(&vec![b'a'; 8193]), None);
        // reversed bounds are put in order, as the reference does
        let s = Scheme::parse(b"stop=2\n1=9-3").unwrap();
        assert_eq!(s.pieces(1, &mut low), [Piece::Size(3)]);
    }

    #[test]
    fn a_write_is_cut_and_padded_the_way_the_reference_does_it() {
        let payload = vec![1u8; 100];
        // more payload than the size: a record of exactly that size, the rest follows
        assert_eq!(
            shape(&[Piece::Size(30)], &payload)
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            [30, 70]
        );
        // the payload ends inside a size: padded up to it with a waste frame
        let records = shape(&[Piece::Size(30), Piece::Size(200)], &payload);
        assert_eq!(records.iter().map(Vec::len).collect::<Vec<_>>(), [30, 200]);
        assert_eq!(&records[1][..70], &payload[30..]);
        assert_eq!(&records[1][70..77], [frame::WASTE, 0, 0, 0, 0, 0, 123]);
        // too little room for a waste header: the payload goes as it is
        assert_eq!(shape(&[Piece::Size(104)], &payload)[0].len(), 100);
        // nothing left: a record of pure padding, its data as long as the size
        let records = shape(&[Piece::Size(100), Piece::Size(50)], &payload);
        assert_eq!(records[1].len(), HEADER + 50);
        assert_eq!(&records[1][..HEADER], [frame::WASTE, 0, 0, 0, 0, 0, 50]);
        // the check mark stops the padding once the payload is out …
        let records = shape(&[Piece::Size(100), Piece::Check, Piece::Size(50)], &payload);
        assert_eq!(records.len(), 1);
        // … and is skipped while there is payload left
        let records = shape(&[Piece::Size(60), Piece::Check, Piece::Size(60)], &payload);
        assert_eq!(records.iter().map(Vec::len).collect::<Vec<_>>(), [60, 60]);
        assert_eq!(shape(&[], &payload), std::slice::from_ref(&payload));
        assert!(shape(&[Piece::Check], &[]).is_empty());
    }

    #[test]
    fn what_is_cut_still_carries_every_payload_byte_in_order() {
        let payload: Vec<u8> = (0..=255u8).cycle().take(3000).collect();
        let s = Scheme::default_scheme();
        for packet in 0..8 {
            let records = shape(&s.pieces(packet, &mut |n| n / 2), &payload);
            // drop the waste frames: what is left is the payload
            let mut seen = Vec::new();
            for r in &records {
                let cut = r.len().min(payload.len() - seen.len());
                seen.extend_from_slice(&r[..cut]);
            }
            assert!(seen == payload, "packet {packet}");
        }
    }
}
