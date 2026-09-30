//! A packet carrier used as a datagram to one peer (phase 2 M5 design 4.3):
//! what a `wireguard` tunnel sends to a peer through an `underlying-proxy`
//! chain. Everything it sends goes to the peer; it takes only what comes from
//! the peer, when the peer is known by address — a carrier that hands names
//! to its server (SOCKS5) cannot tell, and then everything is taken.

use crate::connector::{BoxedDatagram, BoxedPacketSocket, Datagram, PacketSocket, Target};
use rurge_config::HostName;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::ReadBuf;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Datagrams waiting either way; more are dropped, as a full socket drops them.
const QUEUE: usize = 256;

struct PacketDatagram {
    to: Target,
    outgoing: mpsc::Sender<Vec<u8>>,
    incoming: Mutex<mpsc::Receiver<Vec<u8>>>,
    tasks: [JoinHandle<()>; 2],
}

/// `socket` as a datagram to `to`, which it has resolved (`PacketSocket::resolve`).
pub fn packet_datagram(socket: BoxedPacketSocket, to: Target) -> BoxedDatagram {
    let socket: Arc<dyn PacketSocket> = Arc::from(socket);
    let (outgoing, mut queued) = mpsc::channel::<Vec<u8>>(QUEUE);
    let (arrived, incoming) = mpsc::channel::<Vec<u8>>(QUEUE);
    let (sender, peer) = (socket.clone(), to.clone());
    let send = tokio::spawn(async move {
        while let Some(datagram) = queued.recv().await {
            if let Err(e) = sender.send_to(&datagram, &peer).await {
                tracing::trace!(error = %e, "a datagram through the chain was not sent");
            }
        }
    });
    let from_peer = match &to.host {
        HostName::Ip(_) => Some(to.clone()),
        HostName::Domain(_) => None,
    };
    let receive = tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        // the carrier's end is the datagram's: `arrived` drops with it
        while let Ok((n, from)) = socket.recv_from(&mut buf).await {
            if from_peer.as_ref().is_none_or(|peer| *peer == from) {
                let _ = arrived.try_send(buf[..n].to_vec());
            }
        }
    });
    Box::new(PacketDatagram {
        to,
        outgoing,
        incoming: Mutex::new(incoming),
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
        let mut incoming = self.incoming.lock().expect("the incoming datagrams");
        match incoming.poll_recv(cx) {
            Poll::Ready(Some(datagram)) => {
                let n = datagram.len().min(buf.remaining());
                buf.put_slice(&datagram[..n]);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the carrier through the chain has closed",
            ))),
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
    use std::future::poll_fn;
    use std::time::Duration;

    /// Datagrams with where they came from or went.
    type Log = Arc<Mutex<Vec<(Target, Vec<u8>)>>>;

    /// A carrier whose datagrams come from a script, and which notes what
    /// it sends.
    struct Scripted {
        arriving: tokio::sync::Mutex<mpsc::Receiver<(Target, Vec<u8>)>>,
        sent: Log,
    }

    impl PacketSocket for Scripted {
        fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
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
                    None => Err(io::ErrorKind::BrokenPipe.into()),
                }
            })
        }
    }

    type Script = (mpsc::Sender<(Target, Vec<u8>)>, Log);

    fn scripted() -> (BoxedPacketSocket, Script) {
        let (tx, rx) = mpsc::channel(16);
        let sent = Arc::new(Mutex::new(Vec::new()));
        let socket = Scripted {
            arriving: tokio::sync::Mutex::new(rx),
            sent: sent.clone(),
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
}
