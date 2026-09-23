//! TLS records as Shadow TLS sees them: a 5-byte header (type, version,
//! length) and up to 65535 bytes behind it. Nothing here decrypts anything.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, ReadBuf};

pub(crate) const HEADER: usize = 5;
pub(crate) const ALERT: u8 = 21;
pub(crate) const HANDSHAKE: u8 = 22;
pub(crate) const APPLICATION_DATA: u8 = 23;

/// What a data frame may carry. A TLS record holds at most 2^14 bytes of
/// plaintext, and a frame should not look bigger than the records it imitates.
pub(crate) const MAX_DATA: usize = 16384;

pub(crate) fn cut_short() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "shadow-tls: the connection ended in the middle of a record",
    )
}

/// The header of an ApplicationData record carrying `len` bytes.
pub(crate) fn data_header(len: usize) -> [u8; HEADER] {
    let len = u16::try_from(len).expect("a frame fits the 16-bit length field");
    let [hi, lo] = len.to_be_bytes();
    [APPLICATION_DATA, 3, 3, hi, lo]
}

/// Reads whole records, one at a time, across any number of polls. The length
/// field bounds the buffer: a record is never longer than 5 + 65535 bytes.
#[derive(Default)]
pub(crate) struct RecordReader {
    buf: Vec<u8>,
    filled: usize,
}

impl RecordReader {
    /// `Ok(true)`: `record()` holds a whole record, header included.
    /// `Ok(false)`: the peer closed between two records.
    pub(crate) fn poll_record<S: AsyncRead + Unpin + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        stream: &mut S,
    ) -> Poll<io::Result<bool>> {
        if self.buf.len() < HEADER {
            // a fresh record: `filled` bytes of the previous one are gone
            self.buf.clear();
            self.buf.resize(HEADER, 0);
            self.filled = 0;
        }
        loop {
            if self.filled == self.buf.len() {
                if self.buf.len() > HEADER {
                    return Poll::Ready(Ok(true));
                }
                let len = usize::from(u16::from_be_bytes([self.buf[3], self.buf[4]]));
                if len == 0 {
                    return Poll::Ready(Ok(true));
                }
                self.buf.resize(HEADER + len, 0);
            }
            let mut space = ReadBuf::new(&mut self.buf[self.filled..]);
            ready!(Pin::new(&mut *stream).poll_read(cx, &mut space))?;
            let n = space.filled().len();
            if n == 0 {
                return Poll::Ready(if self.filled == 0 {
                    Ok(false)
                } else {
                    Err(cut_short())
                });
            }
            self.filled += n;
        }
    }

    /// The record `poll_record` just completed.
    pub(crate) fn record(&mut self) -> &mut Vec<u8> {
        &mut self.buf
    }

    /// Forgets the completed record: the next poll starts a new one.
    pub(crate) fn consume(&mut self) {
        self.buf.clear();
        self.filled = 0;
    }

    pub(crate) async fn next<S: AsyncRead + Unpin + ?Sized>(
        &mut self,
        stream: &mut S,
    ) -> io::Result<bool> {
        std::future::poll_fn(|cx| self.poll_record(cx, stream)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn records_come_out_whole_however_the_bytes_arrive() {
        let mut wire = Vec::new();
        for (kind, len) in [(HANDSHAKE, 300usize), (APPLICATION_DATA, 0), (ALERT, 2)] {
            wire.extend_from_slice(&[kind, 3, 3]);
            wire.extend_from_slice(&(len as u16).to_be_bytes());
            wire.extend((0..len).map(|i| i as u8));
        }
        for step in [1usize, 2, 3, 5, 7, 64, 4096] {
            let (mut tx, mut rx) = tokio::io::duplex(8);
            let bytes = wire.clone();
            let writer = tokio::spawn(async move {
                for piece in bytes.chunks(step) {
                    tx.write_all(piece).await.unwrap();
                }
            });
            let mut reader = RecordReader::default();
            let mut seen = Vec::new();
            while reader.next(&mut rx).await.unwrap() {
                let record = reader.record();
                seen.push((record[0], record.len() - HEADER));
                assert!(
                    record[HEADER..]
                        .iter()
                        .enumerate()
                        .all(|(i, b)| *b == i as u8)
                );
                reader.consume();
            }
            writer.await.unwrap();
            assert_eq!(
                seen,
                [(HANDSHAKE, 300), (APPLICATION_DATA, 0), (ALERT, 2)],
                "step {step}"
            );
        }
    }

    #[tokio::test]
    async fn an_end_inside_a_record_is_an_error_and_between_records_is_not() {
        for (wire, clean) in [
            (&[][..], true),
            (&[23, 3, 3][..], false),
            (&[23, 3, 3, 0, 4, 1, 2][..], false),
            (&[23, 3, 3, 0, 2, 1, 2][..], true),
        ] {
            let mut rx = wire;
            let mut reader = RecordReader::default();
            let mut result = Ok(());
            loop {
                match reader.next(&mut rx).await {
                    Ok(true) => reader.consume(),
                    Ok(false) => break,
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
            match (result, clean) {
                (Ok(()), true) => {}
                (Err(e), false) => {
                    assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
                    assert_eq!(
                        e.to_string(),
                        "shadow-tls: the connection ended in the middle of a record"
                    );
                }
                (other, _) => panic!("{wire:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn the_data_header_is_an_application_data_record_of_tls_1_2() {
        assert_eq!(data_header(0x1234), [23, 3, 3, 0x12, 0x34]);
    }
}
