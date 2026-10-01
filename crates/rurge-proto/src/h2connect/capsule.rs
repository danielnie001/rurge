//! Capsules (RFC 9297 3.2) as CONNECT-UDP carries them over HTTP/2 (RFC
//! 9298 5): `type (varint), length (varint), value`, back to back across
//! the stream's DATA frames. A DATAGRAM capsule (type 0) holds a context id
//! (a varint, 0 for a UDP payload) and the datagram. Integers are QUIC
//! varints (RFC 9000 16), which need not be minimal.

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

/// The DATAGRAM capsule type (RFC 9297 3.5).
const DATAGRAM: u64 = 0;

/// The longest UDP payload with context id 0 (RFC 9298 5): no longer one
/// is sent, and a longer one received ends the stream.
pub(super) const MAX_PAYLOAD: usize = 65527;

/// The longest DATAGRAM capsule value read: an 8-byte context id and the
/// longest payload. Nothing longer is buffered.
const MAX_VALUE: u64 = 8 + MAX_PAYLOAD as u64;

/// `value` as a minimal varint. Values from 2^62 on cannot be written; the
/// callers write lengths of at most 64 KiB.
fn put_varint(out: &mut Vec<u8>, value: u64) {
    match value {
        0..=0x3f => out.push(value as u8),
        0x40..=0x3fff => out.extend_from_slice(&(value as u16 | 0x4000).to_be_bytes()),
        0x4000..=0x3fff_ffff => out.extend_from_slice(&(value as u32 | 0x8000_0000).to_be_bytes()),
        _ => {
            debug_assert!(value < 1 << 62, "a varint holds 62 bits");
            out.extend_from_slice(&(value | 0xc000_0000_0000_0000).to_be_bytes());
        }
    }
}

/// The varint at the start of `bytes` and its length; `None` when `bytes`
/// ends inside it.
fn parse_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let len = 1usize << (bytes.first()? >> 6);
    let bytes = bytes.get(..len)?;
    let value = bytes[1..]
        .iter()
        .fold(u64::from(bytes[0] & 0x3f), |v, &b| v << 8 | u64::from(b));
    Some((value, len))
}

fn malformed(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("h2-connect: a malformed capsule ({what})"),
    )
}

/// A varint from `r`; `None` at the end of the stream before its first
/// byte (`at_start`: the end is clean there).
async fn read_varint<R: AsyncRead + Unpin>(r: &mut R, at_start: bool) -> io::Result<Option<u64>> {
    let mut bytes = [0u8; 8];
    if r.read(&mut bytes[..1]).await? == 0 {
        return if at_start {
            Ok(None)
        } else {
            Err(malformed("truncated"))
        };
    }
    let len = 1usize << (bytes[0] >> 6);
    r.read_exact(&mut bytes[1..len])
        .await
        .map_err(|_| malformed("truncated"))?;
    Ok(parse_varint(&bytes[..len]).map(|(value, _)| value))
}

/// A DATAGRAM capsule with context id 0 carrying `payload` (at most
/// `MAX_PAYLOAD` bytes).
pub(super) fn datagram(payload: &[u8]) -> Vec<u8> {
    debug_assert!(payload.len() <= MAX_PAYLOAD);
    let mut out = Vec::with_capacity(payload.len() + 5);
    put_varint(&mut out, DATAGRAM);
    // the context id is one byte
    put_varint(&mut out, payload.len() as u64 + 1);
    put_varint(&mut out, 0);
    out.extend_from_slice(payload);
    out
}

/// The next UDP payload on the stream: capsules of other types and
/// datagrams with another context id are skipped (RFC 9297 3.2, RFC 9298
/// 4). `None` at a clean end, between two capsules. A capsule cut short,
/// a DATAGRAM capsule without a context id, or a payload longer than
/// `MAX_PAYLOAD` is an error (RFC 9297 3.3, RFC 9298 5): the stream is done.
pub(super) async fn read_datagram<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    loop {
        let Some(kind) = read_varint(r, true).await? else {
            return Ok(None);
        };
        let Some(len) = read_varint(r, false).await? else {
            return Err(malformed("truncated"));
        };
        if kind != DATAGRAM {
            // dropped without being buffered
            let skipped = tokio::io::copy(&mut (&mut *r).take(len), &mut tokio::io::sink()).await?;
            if skipped != len {
                return Err(malformed("truncated"));
            }
            continue;
        }
        if len > MAX_VALUE {
            return Err(malformed("a datagram too long"));
        }
        let mut value = vec![0u8; len as usize];
        r.read_exact(&mut value)
            .await
            .map_err(|_| malformed("truncated"))?;
        let Some((context, at)) = parse_varint(&value) else {
            return Err(malformed("no context id"));
        };
        if context != 0 {
            continue;
        }
        if value.len() - at > MAX_PAYLOAD {
            return Err(malformed("a datagram too long"));
        }
        value.drain(..at);
        return Ok(Some(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_all(mut bytes: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        let mut got = Vec::new();
        while let Some(payload) = read_datagram(&mut bytes).await? {
            got.push(payload);
        }
        Ok(got)
    }

    #[test]
    fn varints_are_written_minimal_and_read_in_every_length() {
        // RFC 9000 A.1
        for (value, wire) in [
            (37u64, &[0x25u8][..]),
            (15293, &[0x7b, 0xbd]),
            (494_878_333, &[0x9d, 0x7f, 0x3e, 0x7d]),
            (
                151_288_809_941_952_652,
                &[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c],
            ),
        ] {
            let mut out = Vec::new();
            put_varint(&mut out, value);
            assert_eq!(out, wire, "{value}");
            assert_eq!(parse_varint(wire), Some((value, wire.len())));
        }
        // 37 in two bytes, as RFC 9000 A.1 shows: not minimal, still 37
        assert_eq!(parse_varint(&[0x40, 0x25]), Some((37, 2)));
        assert_eq!(parse_varint(&[0x80, 0, 0]), None, "cut short");
        assert_eq!(parse_varint(&[]), None);
    }

    #[test]
    fn a_datagram_capsule_is_type_length_context_payload() {
        assert_eq!(datagram(b"abc"), [0x00, 0x04, 0x00, b'a', b'b', b'c']);
        assert_eq!(datagram(b""), [0x00, 0x01, 0x00]);
        let long = datagram(&[7u8; 100]);
        assert_eq!(&long[..4], [0x00, 0x40, 101, 0x00]);
        assert_eq!(long.len(), 104);
        let longest = datagram(&vec![7u8; MAX_PAYLOAD]);
        assert_eq!(&longest[..6], [0x00, 0x80, 0x00, 0xff, 0xf8, 0x00]);
    }

    #[tokio::test]
    async fn datagrams_come_back_whole_and_in_order() {
        let mut wire = datagram(b"one");
        wire.extend(datagram(b""));
        wire.extend(datagram(&vec![9u8; MAX_PAYLOAD]));
        let got = read_all(&wire).await.unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(
            (got[0].as_slice(), got[1].as_slice()),
            (&b"one"[..], &b""[..])
        );
        assert!(got[2] == vec![9u8; MAX_PAYLOAD]);
    }

    #[tokio::test]
    async fn non_minimal_varints_are_read() {
        // type 0 in 8 bytes, length 4 in 4 bytes, context id 0 in 2 bytes
        let wire = [
            0xc0, 0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0, 4, 0x40, 0, b'h', b'i',
        ];
        assert_eq!(read_all(&wire).await.unwrap(), [b"hi".to_vec()]);
    }

    #[tokio::test]
    async fn other_capsule_types_and_context_ids_are_skipped() {
        let mut wire = Vec::new();
        // an unknown type (0x2a2a, two bytes), five bytes of value
        wire.extend([0x6a, 0x2a, 0x05, 1, 2, 3, 4, 5]);
        // context id 2: a client-allocated context nobody registered
        wire.extend([0x00, 0x03, 0x02, b'n', b'o']);
        // an unknown type with an empty value
        wire.extend([0x17, 0x00]);
        wire.extend(datagram(b"yes"));
        assert_eq!(read_all(&wire).await.unwrap(), [b"yes".to_vec()]);
    }

    #[tokio::test]
    async fn malformed_capsules_end_the_stream() {
        let too_long = {
            let mut wire = vec![0x00];
            put_varint(&mut wire, MAX_PAYLOAD as u64 + 2);
            wire.push(0x00);
            wire.extend(vec![0u8; MAX_PAYLOAD + 1]);
            wire
        };
        let over_the_bound = {
            let mut wire = vec![0x00];
            put_varint(&mut wire, MAX_VALUE + 1);
            wire
        };
        for (wire, what) in [
            (vec![0x00, 0x04, 0x00, b'a'], "truncated"),
            (vec![0x00], "truncated"),
            (vec![0x40], "truncated"),
            (vec![0x17, 0x09, 1, 2], "truncated"),
            (vec![0x00, 0x00], "no context id"),
            (vec![0x00, 0x01, 0x40], "no context id"),
            (too_long, "a datagram too long"),
            (over_the_bound, "a datagram too long"),
        ] {
            let err = read_all(&wire).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{wire:?}");
            assert_eq!(
                err.to_string(),
                format!("h2-connect: a malformed capsule ({what})"),
                "{wire:?}"
            );
        }
    }
}
