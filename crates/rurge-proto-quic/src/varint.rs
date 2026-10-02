//! QUIC's variable-length integers (RFC 9000 16), as the protocols over
//! QUIC write their own fields with them (Hysteria 2's requests).

/// `value` as a variable-length integer, as short as it goes. At most
/// 2^62 - 1: every value written here is a length or a type.
pub(crate) fn put(out: &mut Vec<u8>, value: u64) {
    debug_assert!(value < 1 << 62, "{value} is no QUIC integer");
    match value {
        0..0x40 => out.push(value as u8),
        0x40..0x4000 => out.extend_from_slice(&(0x4000 | value as u16).to_be_bytes()),
        0x4000..0x4000_0000 => out.extend_from_slice(&(0x8000_0000 | value as u32).to_be_bytes()),
        _ => out.extend_from_slice(&(0xc000_0000_0000_0000 | value).to_be_bytes()),
    }
}

/// How many bytes the integer that starts with `first` takes.
pub(crate) fn len(first: u8) -> usize {
    1 << (first >> 6)
}

/// The integer at the start of `bytes`, in any of its lengths, and how
/// many bytes it took; `None` while `bytes` holds only part of it.
pub(crate) fn get(bytes: &[u8]) -> Option<(u64, usize)> {
    let len = len(*bytes.first()?);
    let bytes = bytes.get(..len)?;
    let value = bytes[1..]
        .iter()
        .fold(u64::from(bytes[0] & 0x3f), |value, &b| {
            value << 8 | u64::from(b)
        });
    Some((value, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_one(value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        put(&mut out, value);
        out
    }

    /// RFC 9000 A.1, and the edges of each length.
    #[test]
    fn integers_go_out_shortest_and_come_in_any_length() {
        for (bytes, value) in [
            (
                &[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c][..],
                151_288_809_941_952_652,
            ),
            (&[0x9d, 0x7f, 0x3e, 0x7d], 494_878_333),
            (&[0x7b, 0xbd], 15_293),
            (&[0x25], 37),
        ] {
            assert_eq!(get(bytes), Some((value, bytes.len())));
            assert_eq!(put_one(value), bytes);
        }
        // the same 37 in two bytes: read, never written so
        assert_eq!(get(&[0x40, 0x25]), Some((37, 2)));
        for (value, len) in [
            (0, 1),
            (63, 1),
            (64, 2),
            (16_383, 2),
            (16_384, 4),
            ((1 << 30) - 1, 4),
            (1 << 30, 8),
            ((1 << 62) - 1, 8),
        ] {
            let bytes = put_one(value);
            assert_eq!(bytes.len(), len, "{value}");
            assert_eq!(get(&bytes), Some((value, len)));
        }
        // a Hysteria 2 TCP request starts so
        assert_eq!(put_one(0x401), [0x44, 0x01]);
    }

    #[test]
    fn an_integer_needs_all_its_bytes() {
        assert_eq!(get(&[]), None);
        assert_eq!(get(&[0x44]), None);
        assert_eq!(get(&[0x9d, 0x7f, 0x3e]), None);
        // what follows is not the integer's
        assert_eq!(get(&[0x44, 0x01, 0xff]), Some((0x401, 2)));
    }
}
