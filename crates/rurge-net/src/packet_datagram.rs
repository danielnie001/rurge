//! A packet carrier used as a datagram to one peer (phase 2 M5 design 4.3):
//! what a `wireguard` tunnel sends to a peer through an `underlying-proxy`
//! chain. Everything it sends goes to the peer; it takes only what comes from
//! the peer, when the peer is known by address — a carrier that hands names
//! to its server (SOCKS5) cannot tell, and then everything is taken.
//!
//! A send is taken at once and goes out later: what the carrier then fails
//! it with — it cannot reach the peer: a proxy that turns out to relay no
//! UDP, a login it refuses — is told by the next receive, once, in the
//! carrier's words. A datagram lost alone (`lost_alone`) is told to nobody.
//!
//! The carrier's end is the datagram's. Why it ended — a proxy that closed
//! the association, say — is told in the carrier's words too, once, by the
//! receive that has read all that arrived before; after that, or behind a
//! carrier that ended without a word, the datagram has just closed. Every
//! error keeps its kind but one: a carrier that ended by being reset is
//! told as aborted (`as_the_end`).

use crate::connector::{
    BoxedDatagram, BoxedPacketSocket, Datagram, PacketSocket, Target, lost_alone,
};
use rurge_config::HostName;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use tokio::io::ReadBuf;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Datagrams waiting either way; more are dropped, as a full socket drops them.
const QUEUE: usize = 256;

struct PacketDatagram {
    to: Target,
    outgoing: mpsc::Sender<Vec<u8>>,
    incoming: Mutex<mpsc::Receiver<Vec<u8>>>,
    failed: Arc<Mutex<Failed>>,
    tasks: [JoinHandle<()>; 2],
}

/// What the carrier last failed a send with, until a receive tells it, and
/// the receive that waits: a send that fails wakes it. And what the carrier
/// ended with, when it said why, until the receive that finds nothing more
/// queued tells it.
#[derive(Default)]
struct Failed {
    error: Option<io::Error>,
    receiver: Option<Waker>,
    ended: Option<io::Error>,
}

/// Whether `e` tells why the carrier ended: in words of the carrier's own,
/// or the system's. An error that is its kind and no more — how a carrier
/// ends that has nothing to tell — does not.
fn says_why(e: &io::Error) -> bool {
    e.get_ref().is_some() || e.raw_os_error().is_some()
}

/// `e`, what the carrier ended with, as the receiver is told it: a reset as
/// an abort, in the same words. Of a live carrier a reset says that one
/// datagram bounced, and what reads a carrier passes over it and reads on
/// (QUIC does): the end of a carrier that was reset — a stream to a proxy
/// the server reset — must not be taken for that. The one kind that is not
/// kept, and only for the end: a reset the carrier reports while it lives —
/// a send it failed — stays one.
fn as_the_end(e: io::Error) -> io::Error {
    if e.kind() == io::ErrorKind::ConnectionReset {
        io::Error::new(io::ErrorKind::ConnectionAborted, e.to_string())
    } else {
        e
    }
}

/// `socket` as a datagram to `to`, which it has resolved (`PacketSocket::resolve`).
pub fn packet_datagram(socket: BoxedPacketSocket, to: Target) -> BoxedDatagram {
    let socket: Arc<dyn PacketSocket> = Arc::from(socket);
    let (outgoing, mut queued) = mpsc::channel::<Vec<u8>>(QUEUE);
    let (arrived, incoming) = mpsc::channel::<Vec<u8>>(QUEUE);
    let failed = Arc::new(Mutex::new(Failed::default()));
    let (sender, peer, told) = (socket.clone(), to.clone(), failed.clone());
    let send = tokio::spawn(async move {
        while let Some(datagram) = queued.recv().await {
            if let Err(e) = sender.send_to(&datagram, &peer).await {
                tracing::trace!(error = %e, "a datagram through the chain was not sent");
                if lost_alone(&e) {
                    continue;
                }
                let receiver = {
                    let mut failed = told.lock().expect("the failed send");
                    failed.error = Some(e);
                    failed.receiver.take()
                };
                if let Some(receiver) = receiver {
                    receiver.wake();
                }
            }
        }
    });
    // Compared in the canonical form, on both sides: a relay whose socket is dual-stack reports
    // the source of an IPv4 peer as `::ffff:a.b.c.d`.
    let from_peer = match &to.host {
        HostName::Ip(_) => Some(Target::new(to.host.clone().canonical(), to.port)),
        HostName::Domain(_) => None,
    };
    let ending = failed.clone();
    let receive = tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        let ended = loop {
            match socket.recv_from(&mut buf).await {
                Ok((n, from)) => {
                    let from = Target::new(from.host.canonical(), from.port);
                    if from_peer.as_ref().is_none_or(|peer| *peer == from) {
                        let _ = arrived.try_send(buf[..n].to_vec());
                    }
                }
                Err(e) => break e,
            }
        };
        if says_why(&ended) {
            ending.lock().expect("the failed send").ended = Some(as_the_end(ended));
        }
        // the carrier's end is the datagram's. Closed only now: the receive
        // that finds the queue closed looks for why it is
        drop(arrived);
    });
    Box::new(PacketDatagram {
        to,
        outgoing,
        incoming: Mutex::new(incoming),
        failed,
        tasks: [send, receive],
    })
}

impl Datagram for PacketDatagram {
    fn poll_send(&self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(match self.outgoing.try_send(buf.to_vec()) {
            // a full queue drops it, as a full socket would
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(buf.len()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(io::ErrorKind::BrokenPipe.into()),
        })
    }

    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        {
            let mut failed = self.failed.lock().expect("the failed send");
            if let Some(e) = failed.error.take() {
                return Poll::Ready(Err(e));
            }
            // noted under the lock: a send that fails from now on wakes
            // this receive
            if !failed
                .receiver
                .as_ref()
                .is_some_and(|receiver| receiver.will_wake(cx.waker()))
            {
                failed.receiver = Some(cx.waker().clone());
            }
        }
        let mut incoming = self.incoming.lock().expect("the incoming datagrams");
        match incoming.poll_recv(cx) {
            Poll::Ready(Some(datagram)) => {
                let n = datagram.len().min(buf.remaining());
                buf.put_slice(&datagram[..n]);
                Poll::Ready(Ok(()))
            }
            // the carrier has ended and what had arrived is read: why, in
            // its own words, once
            Poll::Ready(None) => {
                let ended = self.failed.lock().expect("the failed send").ended.take();
                Poll::Ready(Err(ended.unwrap_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "the carrier through the chain has closed",
                    )
                })))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn peer_addr(&self) -> Option<SocketAddr> {
        match self.to.host {
            HostName::Ip(ip) => Some(SocketAddr::new(ip, self.to.port)),
            HostName::Domain(_) => None,
        }
    }
}

impl Drop for PacketDatagram {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BoxFuture;
    use std::collections::VecDeque;
    use std::future::poll_fn;
    use std::time::Duration;

    /// Datagrams with where they came from or went.
    type Log = Arc<Mutex<Vec<(Target, Vec<u8>)>>>;

    /// A carrier whose datagrams come from a script, and which notes what
    /// it sends — after the sends it fails, as told.
    struct Scripted {
        arriving: tokio::sync::Mutex<mpsc::Receiver<(Target, Vec<u8>)>>,
        sent: Log,
        /// What its next sends fail with, in turn.
        failing: Mutex<VecDeque<io::Error>>,
        /// What it ends with once the script is over; without, it ends
        /// with nothing to say.
        ending: Mutex<Option<io::Error>>,
    }

    impl PacketSocket for Scripted {
        fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
            if let Some(e) = self.failing.lock().unwrap().pop_front() {
                return Box::pin(std::future::ready(Err(e)));
            }
            self.sent.lock().unwrap().push((to.clone(), buf.to_vec()));
            Box::pin(std::future::ready(Ok(())))
        }

        fn recv_from<'a>(
            &'a self,
            buf: &'a mut [u8],
        ) -> BoxFuture<'a, io::Result<(usize, Target)>> {
            Box::pin(async move {
                match self.arriving.lock().await.recv().await {
                    Some((from, data)) => {
                        buf[..data.len()].copy_from_slice(&data);
                        Ok((data.len(), from))
                    }
                    None => Err(match self.ending.lock().unwrap().take() {
                        Some(ending) => ending,
                        None => io::ErrorKind::BrokenPipe.into(),
                    }),
                }
            })
        }
    }

    type Script = (mpsc::Sender<(Target, Vec<u8>)>, Log);

    fn scripted() -> (BoxedPacketSocket, Script) {
        scripted_with(Vec::new(), None)
    }

    /// `scripted`, its first sends failing with `failures` in turn.
    fn scripted_failing(failures: Vec<io::Error>) -> (BoxedPacketSocket, Script) {
        scripted_with(failures, None)
    }

    /// `scripted_failing`, the carrier ending with `ending`.
    fn scripted_with(
        failures: Vec<io::Error>,
        ending: Option<io::Error>,
    ) -> (BoxedPacketSocket, Script) {
        let (tx, rx) = mpsc::channel(16);
        let sent = Arc::new(Mutex::new(Vec::new()));
        let socket = Scripted {
            arriving: tokio::sync::Mutex::new(rx),
            sent: sent.clone(),
            failing: Mutex::new(failures.into()),
            ending: Mutex::new(ending),
        };
        (Box::new(socket), (tx, sent))
    }

    fn at(host: &str, port: u16) -> Target {
        Target::new(HostName::parse(host), port)
    }

    async fn recv(datagram: &BoxedDatagram) -> io::Result<Vec<u8>> {
        let mut storage = [0u8; 1500];
        let mut buf = ReadBuf::new(&mut storage);
        tokio::time::timeout(
            Duration::from_secs(5),
            poll_fn(|cx| datagram.poll_recv(cx, &mut buf)),
        )
        .await
        .expect("an outcome within the bound")?;
        Ok(buf.filled().to_vec())
    }

    /// What it sends goes to the peer; of what arrives, a peer known by
    /// address is heard alone.
    #[tokio::test]
    async fn a_peer_by_address_is_heard_alone() {
        let (socket, (arrive, sent)) = scripted();
        let peer = at("192.0.2.7", 51820);
        let datagram = packet_datagram(socket, peer.clone());
        assert_eq!(
            datagram.peer_addr(),
            Some("192.0.2.7:51820".parse().unwrap())
        );
        poll_fn(|cx| datagram.poll_send(cx, b"out")).await.unwrap();
        arrive
            .send((at("198.51.100.1", 53), b"stranger".to_vec()))
            .await
            .unwrap();
        arrive.send((peer.clone(), b"back".to_vec())).await.unwrap();
        assert_eq!(recv(&datagram).await.unwrap(), b"back");
        assert_eq!(*sent.lock().unwrap(), [(peer, b"out".to_vec())]);
    }

    /// A relay whose socket is dual-stack reports the source of an answer
    /// from an IPv4 peer as `::ffff:a.b.c.d`: still the peer, whichever of
    /// the two forms the peer was given in. A stranger in that form is not.
    #[tokio::test]
    async fn a_peer_is_heard_from_its_ipv4_address_in_ipv6_form() {
        for (peer, source, stranger) in [
            ("127.0.0.1", "::ffff:127.0.0.1", "::ffff:198.51.100.1"),
            ("::ffff:127.0.0.1", "127.0.0.1", "198.51.100.1"),
        ] {
            let (socket, (arrive, _sent)) = scripted();
            let datagram = packet_datagram(socket, at(peer, 51820));
            let stranger = (at(stranger, 51820), b"stranger".to_vec());
            arrive.send(stranger).await.unwrap();
            let answer = (at(source, 51820), b"back".to_vec());
            arrive.send(answer).await.unwrap();
            assert_eq!(
                recv(&datagram).await.unwrap(),
                b"back",
                "{peer} heard from {source}"
            );
        }
    }

    /// A peer known by name (the chain's server resolves it) cannot be told
    /// from others: everything is heard. The carrier's end is the
    /// datagram's.
    #[tokio::test]
    async fn a_peer_by_name_hears_everything_until_the_carrier_ends() {
        let (socket, (arrive, _sent)) = scripted();
        let datagram = packet_datagram(socket, at("wg.example", 51820));
        assert_eq!(datagram.peer_addr(), None);
        arrive
            .send((at("192.0.2.7", 51820), b"any".to_vec()))
            .await
            .unwrap();
        assert_eq!(recv(&datagram).await.unwrap(), b"any");
        drop(arrive);
        assert_eq!(
            recv(&datagram).await.unwrap_err().to_string(),
            "the carrier through the chain has closed"
        );
    }

    /// A carrier that says why it ended is heard in its own words, with
    /// their kind — after what had arrived before its end, and once: from
    /// then on the datagram has just closed, as it has at once behind a
    /// carrier that ended without a word
    /// (`a_peer_by_name_hears_everything_until_the_carrier_ends`).
    #[tokio::test]
    async fn the_carriers_end_is_told_in_its_own_words_once() {
        let endings = [
            (
                io::ErrorKind::BrokenPipe,
                "socks5: the proxy closed the UDP association",
            ),
            (
                io::ErrorKind::UnexpectedEof,
                "trojan: the server closed the UDP connection",
            ),
        ];
        for (kind, words) in endings {
            let ending = io::Error::new(kind, words);
            let (socket, (arrive, _sent)) = scripted_with(Vec::new(), Some(ending));
            let peer = at("192.0.2.7", 853);
            let datagram = packet_datagram(socket, peer.clone());
            arrive.send((peer, b"last".to_vec())).await.unwrap();
            drop(arrive);
            assert_eq!(recv(&datagram).await.unwrap(), b"last");
            let told = recv(&datagram).await.unwrap_err();
            assert_eq!((told.kind(), told.to_string().as_str()), (kind, words));
            let after = recv(&datagram).await.unwrap_err();
            assert_eq!(
                (after.kind(), after.to_string().as_str()),
                (
                    io::ErrorKind::BrokenPipe,
                    "the carrier through the chain has closed"
                )
            );
        }
    }

    fn no_relay() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "proxy: the server relays no UDP",
        )
    }

    /// A send the carrier fails — it cannot reach the peer — is told by the
    /// next receive, in the carrier's own words: one that waits already, or
    /// one that comes later. Once: what arrives after it is heard.
    #[tokio::test]
    async fn a_failed_send_is_told_by_the_next_receive() {
        let (socket, (arrive, sent)) = scripted_failing(vec![no_relay(), no_relay()]);
        let peer = at("192.0.2.7", 853);
        let datagram = packet_datagram(socket, peer.clone());
        // the receive is waiting when the send fails
        let (told, taken) = tokio::join!(
            recv(&datagram),
            poll_fn(|cx| datagram.poll_send(cx, b"one"))
        );
        // the send itself was taken: the carrier fails it later
        assert_eq!(taken.unwrap(), 3);
        let told = told.unwrap_err();
        assert_eq!(
            (told.kind(), told.to_string()),
            (no_relay().kind(), no_relay().to_string())
        );
        // the receive comes after the send failed
        poll_fn(|cx| datagram.poll_send(cx, b"two")).await.unwrap();
        tokio::task::yield_now().await;
        assert_eq!(
            recv(&datagram).await.unwrap_err().to_string(),
            no_relay().to_string()
        );
        // told once each: the datagram goes on
        poll_fn(|cx| datagram.poll_send(cx, b"three"))
            .await
            .unwrap();
        arrive.send((peer.clone(), b"back".to_vec())).await.unwrap();
        assert_eq!(recv(&datagram).await.unwrap(), b"back");
        assert_eq!(*sent.lock().unwrap(), [(peer, b"three".to_vec())]);
    }

    /// The system's own "connection reset", whatever its number is here.
    fn system_reset() -> io::Error {
        (1..20_000)
            .map(io::Error::from_raw_os_error)
            .find(|e| e.kind() == io::ErrorKind::ConnectionReset)
            .expect("the system knows a reset")
    }

    const CLOSED: &str = "the carrier through the chain has closed";

    /// A reset tells of a live carrier that one datagram bounced — a stream
    /// the server reset, the next datagram opens another — and keeps its
    /// kind: QUIC passes over it and reads on. A carrier that ends by being
    /// reset — the proxy reset the connection it runs on — is no such
    /// thing: its end is told in its own words as any end is, as an abort.
    #[tokio::test]
    async fn a_carrier_that_was_reset_ends_in_its_own_words_as_an_abort() {
        let bounced = "h2-connect: the server reset the stream (CANCEL)";
        let (socket, (arrive, _sent)) = scripted_with(
            vec![io::Error::new(io::ErrorKind::ConnectionReset, bounced)],
            Some(system_reset()),
        );
        let datagram = packet_datagram(socket, at("192.0.2.7", 853));
        poll_fn(|cx| datagram.poll_send(cx, b"one")).await.unwrap();
        let told = recv(&datagram).await.unwrap_err();
        assert_eq!(
            (told.kind(), told.to_string().as_str()),
            (io::ErrorKind::ConnectionReset, bounced)
        );
        drop(arrive);
        let ended = recv(&datagram).await.unwrap_err();
        assert_eq!(
            (ended.kind(), ended.to_string()),
            (io::ErrorKind::ConnectionAborted, system_reset().to_string())
        );
        let after = recv(&datagram).await.unwrap_err();
        assert_eq!(
            (after.kind(), after.to_string().as_str()),
            (io::ErrorKind::BrokenPipe, CLOSED)
        );
    }

    /// A send the carrier failed and the carrier's own end, both waiting
    /// for a receive: each is told once, the send's failure first.
    #[tokio::test]
    async fn a_failed_send_is_told_before_the_carriers_end() {
        let end = "socks5: the proxy closed the UDP association";
        let (socket, (arrive, sent)) = scripted_with(
            vec![no_relay()],
            Some(io::Error::new(io::ErrorKind::BrokenPipe, end)),
        );
        let peer = at("192.0.2.7", 853);
        let datagram = packet_datagram(socket, peer.clone());
        poll_fn(|cx| datagram.poll_send(cx, b"one")).await.unwrap();
        poll_fn(|cx| datagram.poll_send(cx, b"two")).await.unwrap();
        drop(arrive);
        // the second went out: the first has failed, and the carrier, left
        // to run meanwhile, has ended
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while sent.lock().unwrap().is_empty() {
            assert!(tokio::time::Instant::now() < deadline, "nothing sent");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let told: Vec<String> = [
            recv(&datagram).await,
            recv(&datagram).await,
            recv(&datagram).await,
        ]
        .into_iter()
        .map(|told| told.unwrap_err().to_string())
        .collect();
        assert_eq!(told, [no_relay().to_string().as_str(), end, CLOSED]);
    }

    /// A datagram the carrier will not take — too long for it — or the
    /// system turned away is lost alone, as on any hop: no receive tells
    /// of it.
    #[tokio::test]
    async fn a_datagram_lost_alone_is_told_to_nobody() {
        let (socket, (arrive, sent)) = scripted_failing(vec![
            io::Error::new(io::ErrorKind::InvalidInput, "a datagram too long"),
            io::Error::from_raw_os_error(1),
        ]);
        let peer = at("192.0.2.7", 853);
        let datagram = packet_datagram(socket, peer.clone());
        for payload in [&b"long"[..], b"turned away", b"out"] {
            poll_fn(|cx| datagram.poll_send(cx, payload)).await.unwrap();
        }
        // the third went out: both failures are behind
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while sent.lock().unwrap().is_empty() {
            assert!(tokio::time::Instant::now() < deadline, "nothing sent");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(*sent.lock().unwrap(), [(peer.clone(), b"out".to_vec())]);
        arrive.send((peer, b"back".to_vec())).await.unwrap();
        assert_eq!(recv(&datagram).await.unwrap(), b"back");
    }
}
