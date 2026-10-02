//! Hysteria 2's obfuscation (phase 2 M7 design 5.4): with
//! `salamander-password` every UDP packet of the policy's connections goes
//! through Salamander (`salamander`); with `gecko-password` as well, the
//! handshake's packets in fragments first (`gecko`). It is the outermost
//! layer of a connection's socket (`Obfuscation::over`): over port hopping,
//! and over an underlying proxy's carrier.
//!
//! - One packet at a time: quinn hands the layer no batch to send, and gets
//!   one packet per buffer.
//! - What does not open — no more than a salt, a frame that breaks a rule,
//!   more than quinn's buffer holds — is dropped, as are the fragments of a
//!   packet that never comes whole: quinn never hears of them.
//! - A send that fails other than by blocking loses its datagram, as a full
//!   socket would: an error would end the connection's driver. A Gecko
//!   packet whose first fragment went out is lost, not sent again, when a
//!   later one would block.

mod gecko;
mod salamander;

use gecko::Reassembly;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use rurge_config::spec::Hysteria2Obfs;
use rurge_proto::BuildError;
use salamander::{SALT, Salamander};
use std::borrow::Cow;
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::Instant;

/// The longest datagram read whole, as the reference reads them
/// (`udpBufferSize`): no QUIC packet comes near it. A longer one is cut,
/// and opens to nothing QUIC takes.
const READ: usize = 2048;

/// A policy's obfuscation: what each of its connections' sockets is laid
/// in. No `Debug`: it holds the password.
#[derive(Clone)]
pub(super) struct Obfuscation {
    salamander: Salamander,
    gecko: bool,
}

impl Obfuscation {
    /// Refused when the password is empty: the configuration checks it, a
    /// spec made by hand may not be.
    pub(super) fn new(obfs: &Hysteria2Obfs) -> Result<Obfuscation, BuildError> {
        let (key, password, gecko) = match obfs {
            Hysteria2Obfs::Salamander(password) => ("salamander-password", password, false),
            Hysteria2Obfs::Gecko(password) => ("gecko-password", password, true),
        };
        if password.expose().is_empty() {
            return Err(BuildError::new(format!("`{key}` is empty")));
        }
        Ok(Obfuscation {
            salamander: Salamander::new(password.expose().as_bytes()),
            gecko,
        })
    }

    /// `socket`, one connection's, obfuscated.
    pub(super) fn over(&self, socket: Arc<dyn AsyncUdpSocket>) -> Arc<dyn AsyncUdpSocket> {
        let read = READ * socket.max_receive_segments();
        Arc::new(Obfuscated {
            salamander: self.salamander.clone(),
            messages: self.gecko.then(AtomicU8::default),
            received: Mutex::new(Received {
                buf: vec![0; read].into_boxed_slice(),
                read: RecvMeta::default(),
                at: 0,
                reassembly: Reassembly::default(),
            }),
            inner: socket,
        })
    }
}

/// One connection's socket, obfuscated.
struct Obfuscated {
    inner: Arc<dyn AsyncUdpSocket>,
    salamander: Salamander,
    /// Gecko's: the message the next long-header packet goes in. `None`
    /// for Salamander alone.
    messages: Option<AtomicU8>,
    /// Taken by the endpoint's receive loop alone.
    received: Mutex<Received>,
}

/// What a read of the inner socket brought and quinn has not had yet: a
/// datagram, or several the system put together (`stride` apart), handed on
/// one by one.
struct Received {
    buf: Box<[u8]>,
    read: RecvMeta,
    /// Where the read's next datagram begins.
    at: usize,
    /// Gecko's fragments waiting for the rest of their packet.
    reassembly: Reassembly,
}

impl Obfuscated {
    /// `datagram` in place of what `transmit` carries. A failure other than
    /// blocking loses the datagram.
    fn send(&self, transmit: &Transmit<'_>, datagram: &[u8]) -> io::Result<()> {
        let sent = self.inner.try_send(&Transmit {
            contents: datagram,
            segment_size: None,
            ..transmit.clone()
        });
        match sent {
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => {
                tracing::trace!(error = %e, "an obfuscated QUIC datagram was not sent");
                Ok(())
            }
            sent => sent,
        }
    }

    /// The QUIC packet `datagram` brings, opened in place — or the one it
    /// makes whole, a Gecko fragment —; `None` when it brings none.
    fn open<'a>(
        &self,
        datagram: &'a mut [u8],
        source: SocketAddr,
        reassembly: &mut Reassembly,
    ) -> Option<Cow<'a, [u8]>> {
        let packet = self.salamander.open(datagram)?;
        if self.messages.is_none() || !gecko::is_long_header(packet) {
            return Some(Cow::Borrowed(packet));
        }
        let frame = gecko::parse(packet)?;
        reassembly
            .feed(Instant::now(), source, &frame)
            .map(Cow::Owned)
    }
}

impl fmt::Debug for Obfuscated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Obfuscated")
            .field("gecko", &self.messages.is_some())
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl AsyncUdpSocket for Obfuscated {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.inner.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        let packet = transmit.contents;
        let Some(messages) = self
            .messages
            .as_ref()
            .filter(|_| gecko::is_long_header(packet))
        else {
            let mut datagram = Vec::with_capacity(SALT + packet.len());
            datagram.resize(SALT, 0);
            datagram.extend_from_slice(packet);
            self.salamander.seal(&mut datagram);
            return self.send(transmit, &datagram);
        };
        let message = messages.fetch_add(1, Ordering::Relaxed);
        let fragments =
            gecko::fragments(&self.salamander, packet, gecko::fragment_count(), message);
        for (n, datagram) in fragments.iter().enumerate() {
            if let Err(e) = self.send(transmit, datagram) {
                // before anything went out, quinn tries again once the
                // socket can take it; after, the rest would make nothing
                // whole: the packet is lost
                if n == 0 {
                    return Err(e);
                }
                break;
            }
        }
        Ok(())
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut received = self.received.lock().expect("received");
        let Received {
            buf,
            read,
            at,
            reassembly,
        } = &mut *received;
        loop {
            let mut count = 0;
            while count < bufs.len() && *at < read.len {
                let end = read.len.min(*at + read.stride);
                let datagram = &mut buf[*at..end];
                *at = end;
                let Some(packet) = self.open(datagram, read.addr, reassembly) else {
                    continue;
                };
                // longer than quinn takes: no QUIC packet it would read
                let Some(room) = bufs[count].get_mut(..packet.len()) else {
                    continue;
                };
                room.copy_from_slice(&packet);
                meta[count] = RecvMeta {
                    len: packet.len(),
                    stride: packet.len(),
                    ..*read
                };
                count += 1;
            }
            if count > 0 {
                return Poll::Ready(Ok(count));
            }
            // the last read is all handed on, or dropped: the next one
            *read = RecvMeta::default();
            *at = 0;
            let mut into = [IoSliceMut::new(buf)];
            ready!(
                self.inner
                    .poll_recv(cx, &mut into, std::slice::from_mut(read))
            )?;
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// One datagram per transmit: each has a salt of its own.
    fn max_transmit_segments(&self) -> usize {
        1
    }

    /// One packet per buffer: the layer splits what the inner socket put
    /// together.
    fn max_receive_segments(&self) -> usize {
        1
    }

    /// The inner socket's: quinn's path MTU discovery, where it runs,
    /// measures the obfuscated datagrams.
    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quinn::udp::EcnCodepoint;
    use std::collections::VecDeque;
    use std::task::Waker;

    const PASSWORD: &str = "s3cret-obfs";

    fn server() -> SocketAddr {
        "192.0.2.1:443".parse().unwrap()
    }

    fn obfuscation(gecko: bool) -> Obfuscation {
        let password = PASSWORD.into();
        Obfuscation::new(&if gecko {
            Hysteria2Obfs::Gecko(password)
        } else {
            Hysteria2Obfs::Salamander(password)
        })
        .unwrap()
    }

    /// The inner socket: notes what it sends — unless told to fail —, and
    /// hands out its reads, one per poll.
    #[derive(Debug, Default)]
    struct Scripted {
        sent: Mutex<Vec<Vec<u8>>>,
        /// How the next sends go: `None` goes, `Some` fails so; when none
        /// is left, they go.
        outcomes: Mutex<VecDeque<Option<io::ErrorKind>>>,
        /// Each read's bytes, and how far apart its datagrams are.
        reads: Mutex<VecDeque<(Vec<u8>, usize)>>,
        segments: usize,
    }

    impl Scripted {
        fn sent(&self) -> Vec<Vec<u8>> {
            std::mem::take(&mut *self.sent.lock().unwrap())
        }

        fn read(&self, bytes: Vec<u8>, stride: usize) {
            self.reads.lock().unwrap().push_back((bytes, stride));
        }
    }

    #[derive(Debug)]
    struct Writable;

    impl UdpPoller for Writable {
        fn poll_writable(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncUdpSocket for Scripted {
        fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
            Box::pin(Writable)
        }

        fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
            if let Some(Some(kind)) = self.outcomes.lock().unwrap().pop_front() {
                return Err(kind.into());
            }
            assert_eq!(transmit.segment_size, None);
            self.sent.lock().unwrap().push(transmit.contents.to_vec());
            Ok(())
        }

        fn poll_recv(
            &self,
            _cx: &mut Context<'_>,
            bufs: &mut [IoSliceMut<'_>],
            meta: &mut [RecvMeta],
        ) -> Poll<io::Result<usize>> {
            let Some((bytes, stride)) = self.reads.lock().unwrap().pop_front() else {
                return Poll::Pending;
            };
            bufs[0][..bytes.len()].copy_from_slice(&bytes);
            meta[0] = RecvMeta {
                addr: server(),
                len: bytes.len(),
                stride,
                ecn: Some(EcnCodepoint::Ce),
                dst_ip: None,
            };
            Poll::Ready(Ok(1))
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            Ok("192.0.2.9:4433".parse().unwrap())
        }

        fn max_receive_segments(&self) -> usize {
            self.segments.max(1)
        }

        fn may_fragment(&self) -> bool {
            false
        }
    }

    fn transmit(contents: &[u8]) -> Transmit<'_> {
        Transmit {
            destination: server(),
            ecn: Some(EcnCodepoint::Ect0),
            contents,
            segment_size: None,
            src_ip: None,
        }
    }

    fn short_packet(len: usize) -> Vec<u8> {
        [&[0x40][..], &vec![1; len - 1]].concat()
    }

    fn long_packet(len: usize) -> Vec<u8> {
        [&[0xc0][..], &vec![2; len - 1]].concat()
    }

    /// What quinn gets from one poll offering `room` buffers of 1472 bytes,
    /// as its endpoint's are.
    fn poll(socket: &dyn AsyncUdpSocket, room: usize) -> Vec<(Vec<u8>, RecvMeta)> {
        let mut storage = vec![vec![0u8; 1472]; room];
        let mut bufs: Vec<IoSliceMut<'_>> =
            storage.iter_mut().map(|b| IoSliceMut::new(b)).collect();
        let mut meta = vec![RecvMeta::default(); room];
        let mut cx = Context::from_waker(Waker::noop());
        match socket.poll_recv(&mut cx, &mut bufs, &mut meta) {
            Poll::Pending => Vec::new(),
            Poll::Ready(received) => {
                let n = received.expect("never an error");
                drop(bufs);
                (0..n)
                    .map(|i| (storage[i][..meta[i].len].to_vec(), meta[i]))
                    .collect()
            }
        }
    }

    /// A short-header packet goes as Salamander alone. With Gecko a
    /// long-header one goes in 2 to 8 fragments of one message, the next
    /// one in the next message; with Salamander alone, whole.
    #[test]
    fn what_goes_out_is_sealed_and_the_handshake_with_gecko_in_fragments() {
        let opener = Salamander::new(PASSWORD.as_bytes());
        let open = |mut datagram: Vec<u8>| opener.open(&mut datagram).unwrap().to_vec();
        let (short, long) = (short_packet(100), long_packet(1200));
        let inner = Arc::new(Scripted::default());
        let socket = obfuscation(true).over(inner.clone());
        socket.try_send(&transmit(&short)).unwrap();
        let sent = inner.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].len(), SALT + short.len());
        assert_eq!(open(sent[0].clone()), short);
        for message in 0..2 {
            socket.try_send(&transmit(&long)).unwrap();
            let frames: Vec<Vec<u8>> = inner.sent().into_iter().map(&open).collect();
            assert!((2..=8).contains(&frames.len()), "{}", frames.len());
            let mut whole = Vec::new();
            for (index, frame) in frames.iter().enumerate() {
                let frame = gecko::parse(frame).expect("a frame");
                assert_eq!(
                    (
                        frame.message,
                        usize::from(frame.index),
                        usize::from(frame.count)
                    ),
                    (message, index, frames.len())
                );
                whole.extend_from_slice(frame.chunk);
            }
            assert_eq!(whole, long);
        }

        let socket = obfuscation(false).over(inner.clone());
        socket.try_send(&transmit(&long)).unwrap();
        let sent = inner.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(open(sent[0].clone()), long);
    }

    /// What one side's layer sends, the other's hands quinn as it was: the
    /// handshake's fragments put together again.
    #[test]
    fn what_comes_in_is_opened_and_put_together() {
        let (short, long) = (short_packet(300), long_packet(1252));
        let sender = Arc::new(Scripted::default());
        let out = obfuscation(true).over(sender.clone());
        for packet in [&short, &long, &short] {
            out.try_send(&transmit(packet)).unwrap();
        }
        let inner = Arc::new(Scripted::default());
        let socket = obfuscation(true).over(inner.clone());
        for datagram in sender.sent() {
            let len = datagram.len();
            inner.read(datagram, len);
        }
        let mut packets = Vec::new();
        loop {
            let received = poll(socket.as_ref(), 4);
            if received.is_empty() {
                break;
            }
            for (packet, meta) in received {
                assert_eq!(
                    (meta.addr, meta.len, meta.stride, meta.ecn),
                    (server(), packet.len(), packet.len(), Some(EcnCodepoint::Ce))
                );
                packets.push(packet);
            }
        }
        assert_eq!(packets, [short.clone(), long, short]);
    }

    /// What does not open — no more than a salt, a frame that breaks a
    /// rule, more than quinn's buffer holds — never reaches quinn, and is
    /// no error: the next packet does.
    #[test]
    fn what_does_not_open_never_reaches_quinn() {
        let sealer = Salamander::new(PASSWORD.as_bytes());
        let seal = |payload: &[u8]| {
            let mut datagram = [&[0; SALT][..], payload].concat();
            sealer.seal(&mut datagram);
            datagram
        };
        let inner = Arc::new(Scripted::default());
        let socket = obfuscation(true).over(inner.clone());
        let packet = short_packet(50);
        for datagram in [
            vec![9; SALT],
            // a count of 9
            seal(&[0x80, 1, 0x09, 0, 0, 7, 7]),
            // padding past the end
            seal(&[0x80, 1, 0x02, 0, 9, 7, 7]),
            seal(&short_packet(1473)),
            seal(&packet),
        ] {
            let len = datagram.len();
            inner.read(datagram, len);
        }
        let received = poll(socket.as_ref(), 4);
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].0, packet);
        assert!(poll(socket.as_ref(), 4).is_empty());
    }

    /// A read of several datagrams the system put together goes to quinn
    /// one per buffer, over as many polls as it takes.
    #[test]
    fn datagrams_read_together_go_to_quinn_one_per_buffer() {
        let sealer = Salamander::new(PASSWORD.as_bytes());
        let packets = [short_packet(92), short_packet(92), short_packet(52)];
        let mut read = Vec::new();
        for packet in &packets {
            let mut datagram = [&[0; SALT][..], packet].concat();
            sealer.seal(&mut datagram);
            read.extend_from_slice(&datagram);
        }
        let inner = Arc::new(Scripted {
            segments: 64,
            ..Scripted::default()
        });
        let socket = obfuscation(false).over(inner.clone());
        inner.read(read, 100);
        let first: Vec<Vec<u8>> = poll(socket.as_ref(), 2).into_iter().map(|r| r.0).collect();
        assert_eq!(first, packets[..2]);
        let rest: Vec<Vec<u8>> = poll(socket.as_ref(), 2).into_iter().map(|r| r.0).collect();
        assert_eq!(rest, packets[2..]);
        assert!(poll(socket.as_ref(), 2).is_empty());
    }

    /// A failed send loses its datagram and is no error for quinn; one
    /// that would block is quinn's to wait for — unless a fragment of the
    /// packet went out already, and the packet is lost.
    #[test]
    fn a_failed_send_loses_its_datagram_and_one_that_would_block_waits() {
        let next_sends = |inner: &Scripted, outcomes: &[Option<io::ErrorKind>]| {
            inner
                .outcomes
                .lock()
                .unwrap()
                .extend(outcomes.iter().copied());
        };
        let short = short_packet(100);
        let inner = Arc::new(Scripted::default());
        let socket = obfuscation(false).over(inner.clone());
        next_sends(
            &inner,
            &[Some(io::ErrorKind::Other), Some(io::ErrorKind::WouldBlock)],
        );
        socket.try_send(&transmit(&short)).unwrap();
        let blocked = socket.try_send(&transmit(&short)).unwrap_err();
        assert_eq!(blocked.kind(), io::ErrorKind::WouldBlock);
        socket.try_send(&transmit(&short)).unwrap();
        assert_eq!(inner.sent().len(), 1);

        let long = long_packet(1200);
        let inner = Arc::new(Scripted::default());
        let socket = obfuscation(true).over(inner.clone());
        next_sends(&inner, &[Some(io::ErrorKind::WouldBlock)]);
        let blocked = socket.try_send(&transmit(&long)).unwrap_err();
        assert_eq!(blocked.kind(), io::ErrorKind::WouldBlock);
        assert!(inner.sent().is_empty());
        next_sends(&inner, &[None, Some(io::ErrorKind::WouldBlock)]);
        socket.try_send(&transmit(&long)).unwrap();
        assert_eq!(inner.sent().len(), 1, "the first fragment, and no more");
    }

    /// One packet a time either way, the inner socket's address and
    /// fragmentation; and no password in what it shows of itself.
    #[test]
    fn the_layer_takes_one_packet_at_a_time_and_shows_no_password() {
        let inner = Arc::new(Scripted {
            segments: 64,
            ..Scripted::default()
        });
        for gecko in [false, true] {
            let socket = obfuscation(gecko).over(inner.clone());
            assert_eq!(socket.max_transmit_segments(), 1);
            assert_eq!(socket.max_receive_segments(), 1);
            assert!(!socket.may_fragment());
            assert_eq!(socket.local_addr().unwrap(), inner.local_addr().unwrap());
            let shown = format!("{socket:?}");
            assert!(shown.starts_with("Obfuscated"), "{shown}");
            assert!(!shown.contains(PASSWORD), "{shown}");
        }
        for (obfs, key) in [
            (Hysteria2Obfs::Salamander("".into()), "salamander-password"),
            (Hysteria2Obfs::Gecko("".into()), "gecko-password"),
        ] {
            let err = Obfuscation::new(&obfs).map(|_| ()).unwrap_err();
            assert_eq!(err.message, format!("`{key}` is empty"));
        }
    }
}
