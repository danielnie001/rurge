//! HTTP/3 frames (RFC 9114 7), the variable-length integers they are made
//! of (RFC 9000 16), the server's SETTINGS, and the reading of a stream
//! frame by frame.

use super::{H3_FRAME_ERROR, H3_SETTINGS_ERROR, H3Error};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use quinn::{ReadError, RecvStream, VarInt};
use std::collections::HashSet;
use std::task::{Context, Poll, ready};
use tokio::io::ReadBuf;

// frame types (RFC 9114 7.2)
pub(crate) const DATA: u64 = 0x00;
pub(crate) const HEADERS: u64 = 0x01;
pub(crate) const CANCEL_PUSH: u64 = 0x03;
pub(crate) const SETTINGS: u64 = 0x04;
pub(crate) const PUSH_PROMISE: u64 = 0x05;
pub(crate) const GOAWAY: u64 = 0x07;
pub(crate) const MAX_PUSH_ID: u64 = 0x0d;

/// Frame types of HTTP/2 that HTTP/3 has none of: receiving one is
/// H3_FRAME_UNEXPECTED (RFC 9114 7.2.8).
pub(crate) fn is_http2_only(ty: u64) -> bool {
    matches!(ty, 0x02 | 0x06 | 0x08 | 0x09)
}

/// The largest variable-length integer.
pub(crate) const VARINT_MAX: u64 = (1 << 62) - 1;

/// The variable-length integer at the start of `buf` and how many bytes it
/// takes; `None` while `buf` holds only part of it. It may be longer than
/// its value needs.
pub(crate) fn get_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let len = varint_len(buf);
    let bytes = buf.get(..len)?;
    let value = bytes[1..]
        .iter()
        .fold(u64::from(bytes[0] & 0x3f), |value, &byte| {
            (value << 8) | u64::from(byte)
        });
    Some((value, len))
}

/// How many bytes the variable-length integer at the start of `buf` takes,
/// as its first two bits say; 1 for an empty `buf`.
fn varint_len(buf: &[u8]) -> usize {
    buf.first().map_or(1, |&first| 1 << (first >> 6))
}

/// `value`, at most `VARINT_MAX`, in its shortest form.
pub(crate) fn put_varint(out: &mut impl BufMut, value: u64) {
    debug_assert!(value <= VARINT_MAX, "{value} is past QUIC's integers");
    match value {
        0..=0x3f => out.put_u8(value as u8),
        0x40..=0x3fff => out.put_u16(0x4000 | value as u16),
        0x4000..=0x3fff_ffff => out.put_u32(0x8000_0000 | value as u32),
        _ => out.put_u64(0xc000_0000_0000_0000 | value),
    }
}

/// A whole frame: its type, the length of `payload`, `payload`.
pub(crate) fn frame(ty: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 16);
    put_varint(&mut out, ty);
    put_varint(&mut out, payload.len() as u64);
    out.extend_from_slice(payload);
    out
}

/// The start of a DATA frame of `len` bytes, and how much of the array it
/// takes.
pub(crate) fn data_header(len: usize) -> ([u8; 9], usize) {
    let mut header = [0u8; 9];
    let mut rest = &mut header[..];
    put_varint(&mut rest, DATA);
    put_varint(&mut rest, len as u64);
    let unused = rest.len();
    (header, header.len() - unused)
}

/// A peer's SETTINGS (RFC 9114 7.2.4): each identifier it sent, with the
/// value, in the order sent — the unknown ones too, for the extensions that
/// look them up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings(Vec<(u64, u64)>);

impl Settings {
    /// The value sent for `id`; `None` when the peer did not send it, and
    /// it has its default.
    pub fn get(&self, id: u64) -> Option<u64> {
        self.0
            .iter()
            .find(|&&(sent, _)| sent == id)
            .map(|&(_, value)| value)
    }

    /// The payload of the server's SETTINGS frame: pairs of identifier and
    /// value. A pair cut short is H3_FRAME_ERROR; an identifier twice, or
    /// one of HTTP/2's that HTTP/3 reserved (0x02 to 0x05), is
    /// H3_SETTINGS_ERROR (RFC 9114 7.2.4, 7.2.4.1).
    pub(crate) fn decode(mut payload: &[u8]) -> Result<Settings, H3Error> {
        let cut = || H3Error::new(H3_FRAME_ERROR, "the server's SETTINGS frame is cut short");
        let mut pairs = Vec::new();
        let mut seen = HashSet::new();
        while !payload.is_empty() {
            let (id, n) = get_varint(payload).ok_or_else(cut)?;
            let (value, m) = get_varint(&payload[n..]).ok_or_else(cut)?;
            payload = &payload[n + m..];
            if (0x02..=0x05).contains(&id) {
                return Err(H3Error::new(
                    H3_SETTINGS_ERROR,
                    format!("the server sent the HTTP/2 setting {id:#x}"),
                ));
            }
            if !seen.insert(id) {
                return Err(H3Error::new(
                    H3_SETTINGS_ERROR,
                    format!("the server sent the setting {id:#x} twice"),
                ));
            }
            pairs.push((id, value));
        }
        Ok(Settings(pairs))
    }
}

/// A frame's start: its type and the length of its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) ty: u64,
    pub(crate) len: u64,
}

/// Why a stream could not be read frame by frame.
#[derive(Debug)]
pub(crate) enum ReadFailure {
    /// The stream failed: reset, already stopped, or its connection lost.
    Stream(ReadError),
    /// It ended within a frame: H3_FRAME_ERROR (RFC 9114 7.1).
    Truncated,
}

/// The most one read takes from the stream when nothing says how much.
const CHUNK: usize = 16 << 10;

/// Reads a stream frame by frame. Frame starts and the payloads taken whole
/// are read into a buffer, exactly as much as they need; a DATA payload
/// goes from the stream straight into the caller's buffer.
pub(crate) struct FrameReader {
    recv: RecvStream,
    /// Read from the stream and not yet taken.
    buf: BytesMut,
    /// The stream ended after `buf`.
    end: bool,
}

impl FrameReader {
    pub(crate) fn new(recv: RecvStream) -> FrameReader {
        FrameReader {
            recv,
            buf: BytesMut::new(),
            end: false,
        }
    }

    /// A variable-length integer of its own (a unidirectional stream's
    /// type); `None` when the stream ended before it began.
    pub(crate) fn poll_varint(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<u64>, ReadFailure>> {
        loop {
            if let Some((value, n)) = get_varint(&self.buf) {
                self.buf.advance(n);
                return Poll::Ready(Ok(Some(value)));
            }
            if !ready!(self.poll_fill(cx, varint_len(&self.buf)))? {
                return Poll::Ready(self.ended());
            }
        }
    }

    /// The next frame's start; `None` when the stream ended between frames.
    pub(crate) fn poll_header(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<Header>, ReadFailure>> {
        loop {
            // both integers are taken together: a read that has to wait
            // leaves the type where it was
            let want = match get_varint(&self.buf) {
                None => varint_len(&self.buf),
                Some((ty, n)) => match get_varint(&self.buf[n..]) {
                    Some((len, m)) => {
                        self.buf.advance(n + m);
                        return Poll::Ready(Ok(Some(Header { ty, len })));
                    }
                    None => n + varint_len(&self.buf[n..]),
                },
            };
            if !ready!(self.poll_fill(cx, want))? {
                return Poll::Ready(self.ended());
            }
        }
    }

    /// The whole payload of `len` bytes.
    pub(crate) fn poll_payload(
        &mut self,
        cx: &mut Context<'_>,
        len: usize,
    ) -> Poll<Result<Bytes, ReadFailure>> {
        if !ready!(self.poll_fill(cx, len))? {
            return Poll::Ready(Err(ReadFailure::Truncated));
        }
        Poll::Ready(Ok(self.buf.split_to(len).freeze()))
    }

    /// Passes over the `left` bytes of a payload, counting them down.
    pub(crate) fn poll_skip(
        &mut self,
        cx: &mut Context<'_>,
        left: &mut u64,
    ) -> Poll<Result<(), ReadFailure>> {
        while *left > 0 {
            let want = usize::try_from(*left).unwrap_or(usize::MAX);
            if self.buf.is_empty() && ready!(self.poll_more(cx, want.min(CHUNK)))? == 0 {
                return Poll::Ready(Err(ReadFailure::Truncated));
            }
            let n = want.min(self.buf.len());
            self.buf.advance(n);
            *left -= n as u64;
        }
        Poll::Ready(Ok(()))
    }

    /// Some of the `left` bytes of a DATA payload into `out`, counting
    /// them down; nothing when `out` has no room.
    pub(crate) fn poll_data(
        &mut self,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
        left: &mut u64,
    ) -> Poll<Result<(), ReadFailure>> {
        let room = usize::try_from(*left)
            .unwrap_or(usize::MAX)
            .min(out.remaining());
        if room == 0 {
            return Poll::Ready(Ok(()));
        }
        let n = if !self.buf.is_empty() {
            let n = room.min(self.buf.len());
            out.put_slice(&self.buf[..n]);
            self.buf.advance(n);
            n
        } else {
            if self.end {
                return Poll::Ready(Err(ReadFailure::Truncated));
            }
            let read = ready!(self.recv.poll_read(cx, out.initialize_unfilled_to(room)));
            match read.map_err(ReadFailure::Stream)? {
                0 => {
                    self.end = true;
                    return Poll::Ready(Err(ReadFailure::Truncated));
                }
                n => {
                    out.advance(n);
                    n
                }
            }
        };
        *left -= n as u64;
        Poll::Ready(Ok(()))
    }

    /// Reads and drops whatever comes, until the stream ends.
    pub(crate) fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), ReadFailure>> {
        loop {
            self.buf.clear();
            if ready!(self.poll_more(cx, CHUNK))? == 0 {
                return Poll::Ready(Ok(()));
            }
        }
    }

    /// Asks the peer to send no more (STOP_SENDING with `code`).
    pub(crate) fn stop(&mut self, code: VarInt) {
        // fails only for a stream already stopped or ended
        let _ = self.recv.stop(code);
    }

    /// The stream ended: cleanly when nothing is left half read.
    fn ended<T>(&self) -> Result<Option<T>, ReadFailure> {
        if self.buf.is_empty() {
            Ok(None)
        } else {
            Err(ReadFailure::Truncated)
        }
    }

    /// At least `want` bytes in the buffer; `false` when the stream ended
    /// first.
    fn poll_fill(&mut self, cx: &mut Context<'_>, want: usize) -> Poll<Result<bool, ReadFailure>> {
        while self.buf.len() < want {
            if ready!(self.poll_more(cx, want - self.buf.len()))? == 0 {
                return Poll::Ready(Ok(false));
            }
        }
        Poll::Ready(Ok(true))
    }

    /// Up to `max` (more than 0) more bytes into the buffer: how many, 0
    /// when the stream ended.
    fn poll_more(&mut self, cx: &mut Context<'_>, max: usize) -> Poll<Result<usize, ReadFailure>> {
        if self.end {
            return Poll::Ready(Ok(0));
        }
        let start = self.buf.len();
        self.buf.resize(start + max, 0);
        let read = self.recv.poll_read(cx, &mut self.buf[start..]);
        let n = match read {
            Poll::Ready(Ok(n)) => n,
            _ => 0,
        };
        self.buf.truncate(start + n);
        match read {
            Poll::Ready(Ok(0)) => {
                self.end = true;
                Poll::Ready(Ok(0))
            }
            Poll::Ready(Ok(n)) => Poll::Ready(Ok(n)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(ReadFailure::Stream(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quic::h3::{
        SETTINGS_ENABLE_CONNECT_PROTOCOL, SETTINGS_MAX_FIELD_SECTION_SIZE,
        SETTINGS_QPACK_MAX_TABLE_CAPACITY,
    };

    #[test]
    fn varints_go_out_shortest_and_come_in_any_length() {
        // the examples of RFC 9000 A.1, the last one longer than it needs
        for (bytes, value) in [
            (
                &[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c][..],
                151_288_809_941_952_652,
            ),
            (&[0x9d, 0x7f, 0x3e, 0x7d], 494_878_333),
            (&[0x7b, 0xbd], 15_293),
            (&[0x25], 37),
            (&[0x40, 0x25], 37),
        ] {
            assert_eq!(get_varint(bytes), Some((value, bytes.len())), "{value}");
        }
        // the edges of each length
        for (value, len) in [
            (0, 1),
            (63, 1),
            (64, 2),
            (16_383, 2),
            (16_384, 4),
            ((1 << 30) - 1, 4),
            (1 << 30, 8),
            (VARINT_MAX, 8),
        ] {
            let mut out = Vec::new();
            put_varint(&mut out, value);
            assert_eq!(out.len(), len, "{value}");
            assert_eq!(get_varint(&out), Some((value, len)), "{value}");
        }
        // Hysteria 2's TCP request (0x401) as it goes on the wire; what
        // follows an integer is left alone
        let mut out = Vec::new();
        put_varint(&mut out, 0x401);
        assert_eq!(out, [0x44, 0x01]);
        assert_eq!(get_varint(&[0x25, 0xff]), Some((37, 1)));
    }

    #[test]
    fn a_varint_needs_all_its_bytes() {
        for bytes in [&[][..], &[0x40], &[0x80, 0, 0], &[0xc0, 0, 0, 0, 0, 0, 0]] {
            assert_eq!(get_varint(bytes), None, "{bytes:02x?}");
        }
    }

    #[test]
    fn frames_are_type_length_payload() {
        assert_eq!(frame(HEADERS, b"abc"), [0x01, 0x03, b'a', b'b', b'c']);
        // a reserved type (0x1f * N + 0x21), a length of two bytes
        let reserved = frame(0x1f * 3 + 0x21, &[0; 300]);
        assert_eq!(reserved[..4], [0x40, 0x7e, 0x41, 0x2c]);
        assert_eq!(reserved.len(), 304);
        let (header, n) = data_header(70_000);
        assert_eq!(header[..n], [0x00, 0x80, 0x01, 0x11, 0x70]);
        let (header, n) = data_header(5);
        assert_eq!(header[..n], [0x00, 0x05]);
        assert!(is_http2_only(0x06) && !is_http2_only(GOAWAY) && !is_http2_only(0x21));
    }

    #[test]
    fn settings_keep_every_pair_in_order() {
        let mut payload = Vec::new();
        for (id, value) in [
            (SETTINGS_QPACK_MAX_TABLE_CAPACITY, 0),
            (SETTINGS_MAX_FIELD_SECTION_SIZE, 65_536),
            // a reserved identifier and 0, reserved but not HTTP/2's
            (0x21, 7),
            (0x00, 1),
        ] {
            put_varint(&mut payload, id);
            put_varint(&mut payload, value);
        }
        let settings = Settings::decode(&payload).unwrap();
        assert_eq!(
            settings,
            Settings(vec![(0x01, 0), (0x06, 65_536), (0x21, 7), (0x00, 1)])
        );
        assert_eq!(settings.get(SETTINGS_MAX_FIELD_SECTION_SIZE), Some(65_536));
        assert_eq!(settings.get(SETTINGS_ENABLE_CONNECT_PROTOCOL), None);
        assert_eq!(Settings::decode(&[]).unwrap(), Settings::default());
    }

    #[test]
    fn bad_settings_are_errors_of_the_connection() {
        for (payload, code) in [
            // HTTP/2's ENABLE_PUSH and MAX_FRAME_SIZE
            (&[0x02, 0x00][..], H3_SETTINGS_ERROR),
            (&[0x05, 0x01], H3_SETTINGS_ERROR),
            (&[0x06, 0x01, 0x06, 0x02], H3_SETTINGS_ERROR),
            (&[0x06], H3_FRAME_ERROR),
            (&[0x06, 0x40], H3_FRAME_ERROR),
        ] {
            assert_eq!(
                Settings::decode(payload).unwrap_err().code,
                code,
                "{payload:02x?}"
            );
        }
    }
}
