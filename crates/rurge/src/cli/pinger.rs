//! `rurge_tun::Pinger` on the platform's own ICMP sockets (M4 design §10):
//! every echo request goes out where rurge's own sockets go — the egress of
//! `PlatformSockets` — so that a ping an app sent through the TUN did not come
//! back into the TUN (M4-D20).

use super::runtime::PlatformSockets;
use rurge_net::BoxFuture;
use rurge_net::socket::SocketHook;
use rurge_platform::icmp::{self, Kind};
use rurge_platform::socket::Family;
use rurge_tun::Pinger;
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// What reading one reply may take: more than any ICMP message, which is one
/// datagram's worth.
const RECV_BUFFER: usize = 64 * 1024;

/// How long the receive task holds off after a failure, when the one before
/// it failed too: the first is not delayed, a one-off (Windows reports an
/// earlier datagram's ICMP error as `WSAECONNRESET`) must not hold up an
/// answer sitting in the buffer (`rurge-dns`'s UDP upstream does the same).
const RECV_ERROR_BACKOFF: Duration = Duration::from_millis(50);

/// `rurge_tun::Pinger` on the platform's ICMP sockets, each bound where rurge's
/// own sockets go (the egress of `PlatformSockets`).
pub struct PlatformPinger {
    sockets: Arc<PlatformSockets>,
    waiters: Arc<Mutex<Waiters>>,
    carriers: Mutex<Vec<Carrier>>,
    failed: AtomicBool,
}

impl PlatformPinger {
    pub fn new(sockets: Arc<PlatformSockets>) -> PlatformPinger {
        // the identifier is this pinger's own: a raw socket sees every ICMP
        // message the machine gets, and only these answers are its (P14)
        let ident = getrandom::u32().map_or(0, |random| random as u16);
        PlatformPinger {
            sockets,
            waiters: Arc::new(Mutex::new(Waiters::new(ident))),
            carriers: Mutex::new(Vec::new()),
            failed: AtomicBool::new(false),
        }
    }

    /// The socket an echo request to `family` goes out on, bound to `egress`
    /// like every other socket rurge makes: made on first use, and a failure
    /// is not kept, so the next echo tries again.
    fn carrier(&self, family: Family, egress: Option<&str>) -> io::Result<Arc<UdpSocket>> {
        let mut carriers = self.carriers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(carrier) = carriers
            .iter()
            .find(|carrier| carrier.family == family && carrier.egress.as_deref() == egress)
        {
            return Ok(carrier.socket.clone());
        }
        let (socket, kind) = match self.make(family, egress) {
            Ok(made) => made,
            Err(error) => {
                // one line a run: every echo would otherwise repeat it
                if self.failed.swap(true, Ordering::SeqCst) {
                    tracing::debug!(family = ?family, %error, "icmp: no socket for an echo request");
                } else {
                    tracing::warn!(
                        family = ?family,
                        reason = %error.kind(),
                        "icmp: echo requests are unavailable, so pings go unanswered"
                    );
                }
                return Err(error);
            }
        };
        let socket = Arc::new(socket);
        let task = tokio::spawn(receive(self.waiters.clone(), socket.clone(), family, kind));
        carriers.push(Carrier {
            family,
            egress: egress.map(str::to_owned),
            socket: socket.clone(),
            task,
        });
        Ok(socket)
    }

    /// A socket for echo requests of `family`, bound where rurge's own sockets
    /// to `egress` go, in non-blocking mode.
    fn make(&self, family: Family, egress: Option<&str>) -> io::Result<(UdpSocket, Kind)> {
        let (socket, kind) = icmp::open(family)?;
        match egress {
            // the interface's binding binds: on Windows that is where the
            // source address comes from, and a second bind is refused
            Some(interface) => {
                self.sockets
                    .bind_interface(&socket, interface, net_family(family))
                    .map_err(|error| {
                        tracing::debug!(interface, %error, "icmp: the echo socket could not be bound");
                        io::Error::new(
                            error.kind(),
                            format!(
                                "cannot bind to the network outside the enhanced mode: {}",
                                error.kind()
                            ),
                        )
                    })?;
                tracing::debug!(interface, family = ?family, "icmp: echo requests leave through an interface");
            }
            // Windows refuses to read from a socket that has neither been
            // bound nor sent anything (`WSAEINVAL`), so an unbound one is bound
            // to the unspecified address of its family first; Linux and macOS
            // do not mind
            None => socket.bind(&unspecified(family).into())?,
        }
        socket.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(std::net::UdpSocket::from(socket))?;
        Ok((socket, kind))
    }
}

impl Drop for PlatformPinger {
    fn drop(&mut self) {
        // nothing reads these sockets once the pinger is gone, and a task left
        // running would hold them
        let carriers = self.carriers.lock().unwrap_or_else(|e| e.into_inner());
        for carrier in carriers.iter() {
            carrier.task.abort();
        }
    }
}

impl Pinger for PlatformPinger {
    fn echo<'a>(&'a self, dst: IpAddr, payload: &'a [u8]) -> BoxFuture<'a, io::Result<Vec<u8>>> {
        Box::pin(async move {
            let family = if dst.is_ipv4() {
                Family::V4
            } else {
                Family::V6
            };
            let egress = self.sockets.egress(dst)?;
            let socket = self.carrier(family, egress.as_deref())?;
            let mut waiting = Waiting::new(&self.waiters)
                .ok_or_else(|| io::Error::other("every ICMP sequence number waits for a reply"))?;
            let request = icmp::request(family, waiting.ident, waiting.seq, payload);
            socket.send_to(&request, SocketAddr::new(dst, 0)).await?;
            waiting.reply().await
        })
    }
}

/// One socket, and the family and interface it was made for.
struct Carrier {
    family: Family,
    egress: Option<String>,
    socket: Arc<UdpSocket>,
    /// Ends when the pinger is dropped.
    task: JoinHandle<()>,
}

/// Reads the replies off one socket until the pinger is gone, and hands each
/// to the echo waiting for its sequence number.
async fn receive(waiters: Arc<Mutex<Waiters>>, socket: Arc<UdpSocket>, family: Family, kind: Kind) {
    let mut buffer = vec![0u8; RECV_BUFFER];
    let mut consecutive_errors = 0u32;
    loop {
        let len = match socket.recv_from(&mut buffer).await {
            Ok((len, _from)) => {
                consecutive_errors = 0;
                len
            }
            Err(error) => {
                tracing::debug!(%error, "icmp: reading a reply failed");
                consecutive_errors += 1;
                if consecutive_errors > 1 {
                    tokio::time::sleep(RECV_ERROR_BACKOFF).await;
                }
                continue;
            }
        };
        // anything that is not an echo reply is another ICMP message: those
        // are the TUN's business, not this socket's
        let Some(reply) = icmp::reply(family, &buffer[..len]) else {
            continue;
        };
        deliver(&waiters, kind, reply);
    }
}

/// The unspecified address of `family`, port 0.
fn unspecified(family: Family) -> SocketAddr {
    match family {
        Family::V4 => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        Family::V6 => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    }
}

fn net_family(family: Family) -> rurge_net::socket::Family {
    match family {
        Family::V4 => rurge_net::socket::Family::V4,
        Family::V6 => rurge_net::socket::Family::V6,
    }
}
/// The echo requests waiting for their replies, by sequence number: what the
/// receive tasks hand replies to. Touches no socket.
struct Waiters {
    ident: u16,
    next: u16,
    waiting: HashMap<u16, oneshot::Sender<Vec<u8>>>,
}

impl Waiters {
    fn new(ident: u16) -> Waiters {
        Waiters {
            ident,
            next: 0,
            waiting: HashMap::new(),
        }
    }

    fn register(&mut self) -> Option<(u16, oneshot::Receiver<Vec<u8>>)> {
        let mut seq = self.next;
        for _ in 0..=u16::MAX {
            if !self.waiting.contains_key(&seq) {
                self.next = seq.wrapping_add(1);
                let (sender, receiver) = oneshot::channel();
                self.waiting.insert(seq, sender);
                return Some((seq, receiver));
            }
            seq = seq.wrapping_add(1);
        }
        None
    }

    /// Hands `reply` to the echo waiting for its sequence number, and to its
    /// identifier too on a raw socket: a datagram socket only ever sees the
    /// answers to its own requests, and Linux gives them an identifier of the
    /// kernel's (P14). `false` when nobody waits for it.
    fn deliver(&mut self, kind: Kind, reply: icmp::Reply) -> bool {
        if kind == Kind::Raw && reply.ident != self.ident {
            return false;
        }
        let Some(sender) = self.waiting.remove(&reply.seq) else {
            return false;
        };
        // the echo may have given up between the table and here
        let _ = sender.send(reply.payload);
        true
    }

    fn remove(&mut self, seq: u16) {
        self.waiting.remove(&seq);
    }
}

/// One echo request's place in the table: the sequence number it goes out
/// under, and the reply to it. Dropping it gives the number back, which is what
/// happens to an echo `rurge-tun` stopped waiting for (M3-D8).
struct Waiting {
    waiters: Arc<Mutex<Waiters>>,
    ident: u16,
    seq: u16,
    reply: Option<oneshot::Receiver<Vec<u8>>>,
}

impl Waiting {
    /// A place for one more echo request, or `None` when all 65,536 sequence
    /// numbers wait for a reply.
    fn new(waiters: &Arc<Mutex<Waiters>>) -> Option<Waiting> {
        let mut table = waiters.lock().unwrap_or_else(|e| e.into_inner());
        let (seq, reply) = table.register()?;
        let ident = table.ident;
        drop(table);
        Some(Waiting {
            waiters: waiters.clone(),
            ident,
            seq,
            reply: Some(reply),
        })
    }

    /// The payload of the reply, or an error when the socket that was going to
    /// bring it is gone.
    async fn reply(&mut self) -> io::Result<Vec<u8>> {
        let receiver = self.reply.take().expect("the reply is taken once");
        receiver.await.map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the ICMP socket ended before the reply came",
            )
        })
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(self.seq);
    }
}

/// Hands `reply` to whoever waits for its sequence number, through the lock the
/// table is kept in.
fn deliver(waiters: &Mutex<Waiters>, kind: Kind, reply: icmp::Reply) -> bool {
    waiters
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .deliver(kind, reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(ident: u16, seq: u16, payload: &[u8]) -> icmp::Reply {
        icmp::Reply {
            ident,
            seq,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn replies_go_to_the_waiter_of_their_sequence_number() {
        let waiters = Arc::new(Mutex::new(Waiters::new(7)));
        let mut first = Waiting::new(&waiters).expect("a place for the first");
        let mut second = Waiting::new(&waiters).expect("a place for the second");
        assert_ne!(first.seq, second.seq);

        assert!(
            deliver(&waiters, Kind::Datagram, reply(7, second.seq, b"second")),
            "the second one's reply"
        );
        assert_eq!(
            second.reply.as_mut().unwrap().try_recv().unwrap(),
            b"second"
        );
        assert!(
            first.reply.as_mut().unwrap().try_recv().is_err(),
            "the first one waits on"
        );

        // a reply to a number nobody waits for is dropped
        assert!(!deliver(&waiters, Kind::Datagram, reply(7, 0xbeef, b"no")));
        assert_eq!(first.seq, 0);
    }

    #[test]
    fn a_raw_socket_matches_the_identifier_too() {
        let waiters = Arc::new(Mutex::new(Waiters::new(0x1234)));
        let mut waiting = Waiting::new(&waiters).expect("a place");

        assert!(
            !deliver(&waiters, Kind::Raw, reply(0x4321, waiting.seq, b"not mine")),
            "another pinger's reply"
        );
        assert!(waiting.reply.as_mut().unwrap().try_recv().is_err());

        // a datagram socket sees its own requests' answers only, and Linux
        // gives them an identifier of the kernel's (P14)
        assert!(
            deliver(
                &waiters,
                Kind::Datagram,
                reply(0x4321, waiting.seq, b"mine")
            ),
            "the sequence number alone matches on a datagram socket"
        );
        assert_eq!(waiting.reply.as_mut().unwrap().try_recv().unwrap(), b"mine");
    }

    #[test]
    fn a_dropped_echo_leaves_the_table() {
        let waiters = Arc::new(Mutex::new(Waiters::new(7)));
        let waiting = Waiting::new(&waiters).expect("a place");
        let seq = waiting.seq;
        assert_eq!(waiters.lock().unwrap().waiting.len(), 1);
        drop(waiting);
        assert_eq!(waiters.lock().unwrap().waiting.len(), 0, "given back");
        assert!(!deliver(&waiters, Kind::Datagram, reply(7, seq, b"late")));
    }

    #[test]
    fn sequence_numbers_skip_the_ones_still_waited_for() {
        let waiters = Arc::new(Mutex::new(Waiters::new(7)));
        let first = Waiting::new(&waiters).expect("a place");
        // the table wraps onto the number the first echo still waits under
        waiters.lock().unwrap().next = first.seq;
        let second = Waiting::new(&waiters).expect("a place");
        assert_eq!(second.seq, first.seq.wrapping_add(1));
    }

    /// Both families on the loopback, through the platform's own ICMP sockets:
    /// the reply is the payload that went out. A machine that cannot open one
    /// (Windows without Administrator, Linux without `ping_group_range`) says
    /// why and skips.
    #[tokio::test]
    async fn pings_the_loopback() {
        let pinger = PlatformPinger::new(Arc::new(PlatformSockets::new(
            super::super::enhanced::egress_slot(),
        )));
        for dst in ["127.0.0.1", "::1"] {
            let dst: IpAddr = dst.parse().unwrap();
            let family = if dst.is_ipv4() {
                Family::V4
            } else {
                Family::V6
            };
            if let Err(error) = icmp::open(family) {
                eprintln!("icmp: no {family:?} socket for an echo request ({error}); skipped");
                return;
            }
            let payload = b"rurge";
            let reply =
                tokio::time::timeout(std::time::Duration::from_secs(5), pinger.echo(dst, payload))
                    .await
                    .expect("the reply within five seconds")
                    .expect("the echo reply");
            assert_eq!(reply, payload, "the reply carries what went out");
        }
    }
}
