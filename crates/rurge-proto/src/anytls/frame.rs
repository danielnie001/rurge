//! The AnyTLS session layer's frame: `command(1) ‖ stream id(4, BE) ‖
//! length(2, BE) ‖ data` (anytls-go `docs/protocol.md`).

pub(crate) const WASTE: u8 = 0;
pub(crate) const SYN: u8 = 1;
pub(crate) const PSH: u8 = 2;
pub(crate) const FIN: u8 = 3;
pub(crate) const SETTINGS: u8 = 4;
pub(crate) const ALERT: u8 = 5;
pub(crate) const UPDATE_PADDING_SCHEME: u8 = 6;
// since protocol version 2
pub(crate) const SYNACK: u8 = 7;
pub(crate) const HEART_REQUEST: u8 = 8;
pub(crate) const HEART_RESPONSE: u8 = 9;
/// Only a server sends it (here: the fake one); a client reads it like any
/// other frame it has no use for.
#[cfg(any(test, feature = "testing"))]
pub(crate) const SERVER_SETTINGS: u8 = 10;

pub(crate) const HEADER: usize = 7;
/// What the two-byte length can say.
pub(crate) const MAX_DATA: usize = 65535;

/// Appends one frame. `data` is at most `MAX_DATA` bytes.
pub(crate) fn push(out: &mut Vec<u8>, command: u8, stream: u32, data: &[u8]) {
    debug_assert!(data.len() <= MAX_DATA);
    out.push(command);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
}

pub(crate) fn frame(command: u8, stream: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + data.len());
    push(&mut out, command, stream, data);
    out
}

/// `(command, stream id, data length)`.
pub(crate) fn parse_header(header: &[u8; HEADER]) -> (u8, u32, usize) {
    (
        header[0],
        u32::from_be_bytes([header[1], header[2], header[3], header[4]]),
        usize::from(u16::from_be_bytes([header[5], header[6]])),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_seven_bytes_of_header_and_the_data() {
        let f = frame(PSH, 0x0102_0304, b"hi");
        assert_eq!(f, [2, 1, 2, 3, 4, 0, 2, b'h', b'i']);
        let header: [u8; HEADER] = f[..HEADER].try_into().unwrap();
        assert_eq!(parse_header(&header), (PSH, 0x0102_0304, 2));
        assert_eq!(frame(SYN, 1, &[]), [1, 0, 0, 0, 1, 0, 0]);
        let big = frame(PSH, 1, &vec![0u8; MAX_DATA]);
        assert_eq!(&big[5..7], [0xff, 0xff]);
    }
}
