//! HTTP/3 datagrams (RFC 9297 2.1; phase 2 M7 design 4) on a client made
//! `with_datagrams`: each QUIC datagram of the connection carries
//! `varint(stream ID ÷ 4) ‖ payload`, either way.
//!
//! - A request stream's datagrams flow once taken
//!   (`RequestStream::datagrams`) until either direction of the stream
//!   ends — as reading the stream to its end or failure, its shutdown or a
//!   failed write finds —, the stream or the handle is dropped, or the
//!   connection ends; then `recv` hands over what is queued and ends, and
//!   `send` fails.
//! - One task reads the connection's datagrams into the queue of the
//!   stream each names. A datagram too short for its quarter stream ID, or
//!   naming a stream QUIC cannot have, closes the connection
//!   (H3_DATAGRAM_ERROR). One for a stream whose datagrams do not flow —
//!   never opened, ended, or one whose request has no use for datagrams,
//!   which RFC 9297 2 would abort with H3_DATAGRAM_ERROR — is dropped, and
//!   so is one that meets a full queue: the reader waits for no stream.
//! - Sending never waits: a payload a QUIC datagram on the path cannot
//!   carry now is refused, and nothing goes out.

use super::client::Inner;
use super::frame;
use super::{H3_DATAGRAM_ERROR, H3Error};
use bytes::{Buf, Bytes, BytesMut};
use quinn::SendDatagramError;
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::mpsc;

/// The datagrams that wait for a stream's reader; more are dropped, as a
/// socket would.
pub(super) const QUEUE: usize = 64;

/// The largest quarter stream ID: that of QUIC's largest stream ID,
/// 2^62 − 1 (RFC 9297 2.1).
const MAX_QUARTER_STREAM_ID: u64 = (1 << 60) - 1;

/// The streams whose datagrams flow, by quarter stream ID, and the queue
/// of each.
#[derive(Default)]
pub(super) struct Flows(Mutex<Table>);

#[derive(Default)]
struct Table {
    queues: HashMap<u64, mpsc::Sender<Bytes>>,
    /// The reading ended: no stream's datagrams flow any more.
    ended: bool,
}

impl Flows {
    fn table(&self) -> MutexGuard<'_, Table> {
        self.0.lock().expect("flows")
    }

    /// The queue of stream `quarter`'s datagrams, which flow from now on;
    /// `None` once the reading ended.
    pub(super) fn open(&self, quarter: u64) -> Option<mpsc::Receiver<Bytes>> {
        let (queue, incoming) = mpsc::channel(QUEUE);
        let mut table = self.table();
        if table.ended {
            return None;
        }
        table.queues.insert(quarter, queue);
        Some(incoming)
    }

    /// Stream `quarter`'s datagrams flow no more: its queue ends after
    /// what it holds.
    pub(super) fn close(&self, quarter: u64) {
        self.table().queues.remove(&quarter);
    }

    fn is_open(&self, quarter: u64) -> bool {
        self.table().queues.contains_key(&quarter)
    }

    /// Hands `payload` to the queue of stream `quarter`. Dropped when the
    /// stream's datagrams do not flow, or its queue is full.
    fn deliver(&self, quarter: u64, payload: Bytes) {
        if let Some(queue) = self.table().queues.get(&quarter) {
            let _ = queue.try_send(payload);
        }
    }

    /// The reading ended: every queue ends after what it holds.
    fn end(&self) {
        let mut table = self.table();
        table.queues.clear();
        table.ended = true;
    }
}

/// Reads the connection's datagrams into the queues of the streams they
/// name until the connection ends — or the client goes, and its tasks with
/// it. Every flow ends with the reading, however it ends.
pub(super) async fn read(inner: Arc<Inner>) {
    let Some(flows) = &inner.flows else {
        return;
    };
    let _end = EndFlows(flows);
    while let Ok(datagram) = inner.quic.read_datagram().await {
        match parse(datagram) {
            Ok((quarter, payload)) => flows.deliver(quarter, payload),
            Err(error) => {
                inner.fail(error);
                return;
            }
        }
    }
}

/// Ends every flow when dropped: when the reading ends.
struct EndFlows<'a>(&'a Flows);

impl Drop for EndFlows<'_> {
    fn drop(&mut self) {
        self.0.end();
    }
}

/// The quarter stream ID at the start of `datagram`, and the payload after
/// it (RFC 9297 2.1).
fn parse(mut datagram: Bytes) -> Result<(u64, Bytes), H3Error> {
    let Some((quarter, n)) = frame::get_varint(&datagram) else {
        return Err(H3Error::new(
            H3_DATAGRAM_ERROR,
            "the server sent a datagram too short for a quarter stream ID",
        ));
    };
    if quarter > MAX_QUARTER_STREAM_ID {
        return Err(H3Error::new(
            H3_DATAGRAM_ERROR,
            "the server sent a datagram for a stream QUIC cannot have",
        ));
    }
    datagram.advance(n);
    Ok((quarter, datagram))
}

/// The HTTP/3 datagrams of one request stream (`RequestStream::datagrams`).
/// Dropped, the stream's datagrams flow no more.
pub struct Datagrams {
    inner: Arc<Inner>,
    quarter: u64,
    /// `varint(quarter)`, which every datagram sent starts with.
    prefix: Bytes,
    incoming: tokio::sync::Mutex<mpsc::Receiver<Bytes>>,
}

impl Datagrams {
    pub(super) fn new(
        inner: Arc<Inner>,
        quarter: u64,
        incoming: mpsc::Receiver<Bytes>,
    ) -> Datagrams {
        let mut prefix = BytesMut::new();
        frame::put_varint(&mut prefix, quarter);
        Datagrams {
            inner,
            quarter,
            prefix: prefix.freeze(),
            incoming: tokio::sync::Mutex::new(incoming),
        }
    }

    /// The longest payload a datagram carries now: what a QUIC datagram on
    /// the connection's path takes, less the quarter stream ID. It changes
    /// with the path.
    pub fn max_payload(&self) -> usize {
        self.inner
            .quic
            .max_datagram_size()
            .map_or(0, |max| max.saturating_sub(self.prefix.len()))
    }

    /// Sends `payload` in one datagram without waiting: QUIC sends it when
    /// it can — or drops it, as it drops the oldest of those not sent yet
    /// to make room for a new one. Refused, nothing sent, for a payload
    /// longer than `max_payload`, and once the stream's datagrams ended.
    pub fn send(&self, payload: &[u8]) -> io::Result<()> {
        let open = self
            .inner
            .flows
            .as_ref()
            .is_some_and(|flows| flows.is_open(self.quarter));
        if !open {
            return Err(self.inner.flow_ended());
        }
        let max = self.max_payload();
        if payload.len() > max {
            return Err(too_long(max));
        }
        let mut datagram = BytesMut::with_capacity(self.prefix.len() + payload.len());
        datagram.extend_from_slice(&self.prefix);
        datagram.extend_from_slice(payload);
        self.inner
            .quic
            .send_datagram(datagram.freeze())
            .map_err(|e| match e {
                SendDatagramError::ConnectionLost(e) => self.inner.lost(&e),
                // the path narrowed since `max` was taken
                SendDatagramError::TooLarge => too_long(self.max_payload()),
                SendDatagramError::UnsupportedByPeer | SendDatagramError::Disabled => {
                    io::Error::new(io::ErrorKind::Unsupported, format!("h3: {e}"))
                }
            })
    }

    /// The payload of the next datagram the server sent for the stream;
    /// `None` once its datagrams ended and those queued were taken.
    pub async fn recv(&self) -> Option<Bytes> {
        self.incoming.lock().await.recv().await
    }
}

fn too_long(max: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("h3: a datagram longer than {max} bytes"),
    )
}

impl Drop for Datagrams {
    fn drop(&mut self) {
        if let Some(flows) = &self.inner.flows {
            flows.close(self.quarter);
        }
    }
}

impl fmt::Debug for Datagrams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Datagrams")
            .field("stream", &(self.quarter * 4))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A datagram's quarter stream ID in any length; QUIC's last stream is
    /// the last one (RFC 9297 2.1).
    #[test]
    fn a_datagram_starts_with_its_quarter_stream_id() {
        for (bytes, quarter, payload) in [
            (&[0x00][..], 0, &b""[..]),
            (&[0x01, b'u', b'p'], 1, b"up"),
            // 256 longer than it needs
            (&[0x80, 0x00, 0x01, 0x00, b'x'], 256, b"x"),
            (
                &[0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
                (1 << 60) - 1,
                b"",
            ),
        ] {
            let (got, rest) = parse(Bytes::copy_from_slice(bytes)).unwrap();
            assert_eq!((got, &rest[..]), (quarter, payload), "{bytes:02x?}");
        }
    }

    #[test]
    fn a_datagram_without_a_quarter_stream_id_of_quic_is_an_error() {
        let short = "the server sent a datagram too short for a quarter stream ID";
        for (bytes, text) in [
            (&[][..], short),
            (&[0x40], short),
            (&[0xc0, 0, 0, 0], short),
            // 2^60
            (
                &[0xd0, 0, 0, 0, 0, 0, 0, 0],
                "the server sent a datagram for a stream QUIC cannot have",
            ),
        ] {
            let error = parse(Bytes::copy_from_slice(bytes)).unwrap_err();
            assert_eq!(error.code, H3_DATAGRAM_ERROR, "{bytes:02x?}");
            assert_eq!(error.text, text, "{bytes:02x?}");
        }
    }

    /// Each stream's datagrams go to its own queue; one for a stream whose
    /// datagrams do not flow is dropped, and so is one that meets a full
    /// queue — the others' are delivered all the same.
    #[test]
    fn datagrams_go_to_their_streams_queues() {
        let flows = Flows::default();
        let mut a = flows.open(0).unwrap();
        let mut b = flows.open(1).unwrap();
        flows.deliver(7, Bytes::from_static(b"nobody's"));
        for i in 0..QUEUE + 10 {
            flows.deliver(0, Bytes::from(vec![i as u8]));
        }
        flows.deliver(1, Bytes::from_static(b"b"));
        assert_eq!(b.try_recv().unwrap(), &b"b"[..]);
        for i in 0..QUEUE {
            assert_eq!(a.try_recv().unwrap(), vec![i as u8], "{i}");
        }
        assert!(a.try_recv().is_err(), "the full queue's overflow dropped");
        assert!(flows.is_open(0) && !flows.is_open(7));
    }

    /// A flow closed, or every flow once the reading ended: the queue ends
    /// after what it holds, and no flow opens any more.
    #[test]
    fn flows_end() {
        let flows = Flows::default();
        let mut a = flows.open(0).unwrap();
        let mut b = flows.open(1).unwrap();
        flows.deliver(0, Bytes::from_static(b"queued"));
        flows.close(0);
        flows.deliver(0, Bytes::from_static(b"late"));
        assert_eq!(a.try_recv().unwrap(), &b"queued"[..]);
        assert_eq!(a.try_recv(), Err(mpsc::error::TryRecvError::Disconnected));
        assert!(!flows.is_open(0));
        flows.deliver(1, Bytes::from_static(b"queued"));
        flows.end();
        assert_eq!(b.try_recv().unwrap(), &b"queued"[..]);
        assert_eq!(b.try_recv(), Err(mpsc::error::TryRecvError::Disconnected));
        assert!(flows.open(2).is_none());
    }
}
