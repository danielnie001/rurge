//! DNS over UDP (design §7.2): one datagram flow per upstream, opened through
//! a connector — a direct one with the platform's socket hook for a plain
//! server, so its questions leave on the network outside the virtual interface
//! (M4 design 7.2); the policy interface's for an upstream `via` it
//! (`dns-follow-interface`, phase 2 M5 design 8.5). A receive loop routes
//! answers to waiters by message ID (so several queries can be in flight at
//! once, each under an ID of its own: a query that comes with an ID another
//! one in flight has is sent under another), and one retry over TCP when the
//! answer is truncated, on a connection of the same connector.

use super::tcp::exchange_framed;
use super::{Upstream, UpstreamError};
use crate::message::{random_id, set_wire_id, wire_id, wire_truncated};
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedDatagram, ConnectOpts, Connector, Target};
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::poll_fn;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::ReadBuf;
use tokio::sync::{OnceCell, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

pub const UDP_BUFFER: usize = 4096;

/// Pause between receive attempts once `recv` keeps failing, so a socket that
/// is permanently broken (the interface went down) cannot spin the receive
/// loop. The first failure is not delayed: on Windows a connected UDP socket
/// reports an ICMP port-unreachable from an earlier datagram as
/// `WSAECONNRESET`, which is a normal one-off and must not hold up an answer
/// already sitting in the socket buffer.
const RECV_ERROR_BACKOFF: Duration = Duration::from_millis(50);

/// The queries in flight, by the ID they were sent under. An ID is taken from
/// before the question is sent until its query is over, whichever way it ends
/// (`Claim`); the sender is gone from its slot once the answer was handed
/// over, and the slot stays taken.
#[derive(Default)]
struct Pending(Mutex<HashMap<u16, Option<oneshot::Sender<Vec<u8>>>>>);

impl Pending {
    /// The claim on an ID for a query that came with `wanted`: that one when no
    /// query in flight has it, else the first that none has, counting up from a
    /// random one. The ID is given back when the claim is dropped. All 65,536
    /// taken: an error.
    fn claim(
        &self,
        wanted: u16,
        waiter: oneshot::Sender<Vec<u8>>,
    ) -> Result<Claim<'_>, UpstreamError> {
        let mut taken = self.0.lock().expect("udp pending lock");
        let id = if taken.contains_key(&wanted) {
            let start = random_id();
            (0..=u16::MAX)
                .map(|step| start.wrapping_add(step))
                .find(|id| !taken.contains_key(id))
                .ok_or_else(|| UpstreamError::Io("too many queries in flight".to_string()))?
        } else {
            wanted
        };
        taken.insert(id, Some(waiter));
        Ok(Claim { pending: self, id })
    }

    /// Hands `answer` to the query that waits under `id`: the first answer
    /// only, and none when no query does.
    fn deliver(&self, id: u16, answer: Vec<u8>) {
        let waiter = self
            .0
            .lock()
            .expect("udp pending lock")
            .get_mut(&id)
            .and_then(Option::take);
        if let Some(waiter) = waiter {
            let _ = waiter.send(answer);
        }
    }

    fn release(&self, id: u16) {
        self.0.lock().expect("udp pending lock").remove(&id);
    }
}

/// An ID claimed, given back when this is dropped: when the query is over,
/// however it ends — and when its future is dropped, as a fanout does to the
/// queries still waiting when it has an answer, though no answer ever comes.
struct Claim<'a> {
    pending: &'a Pending,
    id: u16,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.pending.release(self.id);
    }
}

struct Shared {
    datagram: BoxedDatagram,
    /// Held while a datagram is sent: the socket's write readiness keeps one
    /// waiter, so a send parked there would never be woken once another query
    /// sends. The questions of one upstream do go out one at a time.
    sending: tokio::sync::Mutex<()>,
    pending: Pending,
}

async fn send(datagram: &BoxedDatagram, wire: &[u8]) -> std::io::Result<()> {
    poll_fn(|cx| datagram.poll_send(cx, wire)).await.map(|_| ())
}

async fn recv(datagram: &BoxedDatagram, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut read = ReadBuf::new(buf);
    poll_fn(|cx| datagram.poll_recv(cx, &mut read)).await?;
    Ok(read.filled().len())
}

pub struct UdpUpstream {
    name: String,
    addr: SocketAddr,
    /// What the questions and their retry go through: a direct connector with
    /// the platform's socket hook for a plain server, the interface's for an
    /// upstream `via` it.
    connector: Arc<dyn Connector>,
    state: OnceCell<(Arc<Shared>, JoinHandle<()>)>,
}

fn io_err(e: std::io::Error) -> UpstreamError {
    UpstreamError::Io(e.to_string())
}

impl UdpUpstream {
    /// The server at `addr`, asked directly through `connector`: a direct
    /// connector with the platform's socket hook (M4 design 7.2), never the
    /// pipeline's. Its name is `udp://<addr>`.
    pub fn new(addr: SocketAddr, connector: Arc<dyn Connector>) -> UdpUpstream {
        UdpUpstream {
            name: format!("udp://{addr}"),
            addr,
            connector,
            state: OnceCell::new(),
        }
    }

    /// The server at `addr`, asked through `connector`; `key` (an
    /// interface's name) sets its name apart from the upstream asked
    /// directly.
    pub fn via(addr: SocketAddr, key: &str, connector: Arc<dyn Connector>) -> UdpUpstream {
        UdpUpstream {
            name: format!("udp://{addr} via {key}"),
            addr,
            connector,
            state: OnceCell::new(),
        }
    }

    fn target(&self) -> Target {
        Target::new(HostName::Ip(self.addr.ip()), self.addr.port())
    }

    async fn shared(&self) -> Result<Arc<Shared>, UpstreamError> {
        let (shared, _) = self
            .state
            .get_or_try_init(|| async {
                let datagram = self
                    .connector
                    .connect_udp(&self.target(), &ConnectOpts::default())
                    .await
                    .map_err(io_err)?;
                let shared = Arc::new(Shared {
                    datagram,
                    sending: tokio::sync::Mutex::new(()),
                    pending: Pending::default(),
                });
                let receiver = Arc::clone(&shared);
                let upstream = self.name.clone();
                let task = tokio::spawn(async move {
                    let mut buf = vec![0u8; UDP_BUFFER];
                    let mut consecutive_errors = 0u32;
                    loop {
                        let n = match recv(&receiver.datagram, &mut buf).await {
                            Ok(n) => {
                                consecutive_errors = 0;
                                n
                            }
                            Err(e) => {
                                tracing::debug!(upstream = %upstream, error = %e, "recv failed");
                                consecutive_errors += 1;
                                if consecutive_errors > 1 {
                                    tokio::time::sleep(RECV_ERROR_BACKOFF).await;
                                }
                                continue;
                            }
                        };
                        let Some(id) = wire_id(&buf[..n]) else {
                            continue;
                        };
                        receiver.pending.deliver(id, buf[..n].to_vec());
                    }
                });
                Ok::<(Arc<Shared>, JoinHandle<()>), UpstreamError>((shared, task))
            })
            .await?;
        Ok(Arc::clone(shared))
    }
}

impl Drop for UdpUpstream {
    fn drop(&mut self) {
        if let Some((_, task)) = self.state.get() {
            task.abort();
        }
    }
}

#[cfg(test)]
impl UdpUpstream {
    /// How many IDs are taken by queries in flight.
    fn in_flight(&self) -> usize {
        self.state.get().map_or(0, |(shared, _)| {
            shared.pending.0.lock().expect("udp pending lock").len()
        })
    }
}

impl Upstream for UdpUpstream {
    fn name(&self) -> &str {
        &self.name
    }

    fn query<'a>(
        &'a self,
        wire: &'a [u8],
        deadline: Instant,
    ) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>> {
        Box::pin(async move {
            let asked_id = wire_id(wire).ok_or_else(|| {
                UpstreamError::BadResponse("query shorter than a DNS header".to_string())
            })?;
            let shared = self.shared().await?;
            let (tx, rx) = oneshot::channel();
            let claim = shared.pending.claim(asked_id, tx)?;
            let id = claim.id;
            // under the ID that was claimed, which is the one it came with
            // unless another query in flight has it
            let sent = if id == asked_id {
                Cow::Borrowed(wire)
            } else {
                let mut copy = wire.to_vec();
                set_wire_id(&mut copy, id);
                Cow::Owned(copy)
            };
            {
                let _sending = shared.sending.lock().await;
                send(&shared.datagram, &sent).await.map_err(io_err)?;
            }
            let mut resp = match tokio::time::timeout_at(deadline, rx).await {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(_)) => {
                    return Err(UpstreamError::Io(
                        "waiter dropped without an answer".to_string(),
                    ));
                }
                Err(_) => return Err(UpstreamError::Timeout),
            };
            drop(claim);
            // under the ID it came with again
            set_wire_id(&mut resp, asked_id);
            if !wire_truncated(&resp) {
                return Ok(resp);
            }
            // Truncated: repeat the same query once over TCP to the same server.
            let connecting = async {
                let timeout = deadline.saturating_duration_since(Instant::now());
                self.connector
                    .connect(&self.target(), &ConnectOpts { timeout })
                    .await
            };
            let mut tcp = match tokio::time::timeout_at(deadline, connecting).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => return Err(io_err(e)),
                Err(_) => return Err(UpstreamError::Timeout),
            };
            // asked as it came, under the ID it came with
            exchange_framed(&mut tcp, wire, deadline).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::parse_response;
    use crate::testing::{MockDns, direct, query_wire};
    use rurge_net::connector::{
        BoxedDatagram, BoxedStream, ConnectOpts, Connector, DirectConnector, SystemResolve, Target,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, UdpSocket};

    /// A direct connector that records what it was asked to connect to: the
    /// datagram of a question, the stream of a retry over TCP.
    struct Seen {
        inner: DirectConnector,
        udp: Mutex<Vec<Target>>,
        tcp: Mutex<Vec<Target>>,
    }

    impl Seen {
        fn new() -> Seen {
            Seen {
                inner: DirectConnector::new(Arc::new(SystemResolve)),
                udp: Mutex::new(Vec::new()),
                tcp: Mutex::new(Vec::new()),
            }
        }

        fn udp(&self) -> Vec<Target> {
            self.udp.lock().expect("udp").clone()
        }

        fn tcp(&self) -> Vec<Target> {
            self.tcp.lock().expect("tcp").clone()
        }
    }

    impl Connector for Seen {
        fn connect<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, std::io::Result<BoxedStream>> {
            self.tcp.lock().expect("tcp").push(target.clone());
            self.inner.connect(target, opts)
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, std::io::Result<BoxedDatagram>> {
            self.udp.lock().expect("udp").push(target.clone());
            self.inner.connect_udp(target, opts)
        }
    }

    /// A 12-byte header with the given id and flags, followed by `payload`.
    fn msg(id: u16, flags: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; 12];
        v[0..2].copy_from_slice(&id.to_be_bytes());
        v[2] = flags;
        v.extend_from_slice(payload);
        v
    }

    /// Raw UDP server whose behaviour is a function of the request bytes:
    /// `None` = drop; `Some((delay, reply))` = answer after `delay`.
    async fn raw_udp(
        behaviour: impl Fn(&[u8]) -> Option<(Duration, Vec<u8>)> + Send + Sync + 'static,
    ) -> SocketAddr {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let addr = socket.local_addr().unwrap();
        let behaviour = Arc::new(behaviour);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
                    return;
                };
                if let Some((delay, reply)) = behaviour(&buf[..n]) {
                    let socket = Arc::clone(&socket);
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = socket.send_to(&reply, peer).await;
                    });
                }
            }
        });
        addr
    }

    fn deadline(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    #[tokio::test]
    async fn echoes_and_matches_ids_under_concurrency() {
        let addr = raw_udp(|req| {
            // Answer the first id slowly, everything else at once, echoing the request.
            let delay = if req[0..2] == [0, 1] {
                Duration::from_millis(150)
            } else {
                Duration::ZERO
            };
            Some((delay, req.to_vec()))
        })
        .await;
        let up = Arc::new(UdpUpstream::new(addr, direct()));
        assert_eq!(up.name(), format!("udp://{addr}"));
        let a = {
            let up = Arc::clone(&up);
            tokio::spawn(async move { up.query(&msg(1, 0, b"slow"), deadline(2000)).await })
        };
        let b = {
            let up = Arc::clone(&up);
            tokio::spawn(async move { up.query(&msg(2, 0, b"fast"), deadline(2000)).await })
        };
        assert_eq!(b.await.unwrap().unwrap(), msg(2, 0, b"fast"));
        assert_eq!(a.await.unwrap().unwrap(), msg(1, 0, b"slow"));
    }

    #[tokio::test]
    async fn ignores_answers_with_a_foreign_id() {
        // The only reply the server ever sends carries a flipped id.
        let addr = raw_udp(|req| {
            let mut wrong = req.to_vec();
            wrong[0] ^= 0xff;
            Some((Duration::ZERO, wrong))
        })
        .await;
        let up = UdpUpstream::new(addr, direct());
        let err = up.query(&msg(7, 0, b"x"), deadline(200)).await;
        assert_eq!(
            err,
            Err(UpstreamError::Timeout),
            "a foreign id must not satisfy the waiter"
        );
    }

    #[tokio::test]
    async fn drops_time_out() {
        let addr = raw_udp(|_| None).await;
        let up = UdpUpstream::new(addr, direct());
        assert_eq!(
            up.query(&msg(3, 0, b"x"), deadline(150)).await,
            Err(UpstreamError::Timeout)
        );
    }

    #[tokio::test]
    async fn truncated_answer_is_retried_over_tcp() {
        // TCP echo bound first so the UDP server can share the port number.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut len = [0u8; 2];
                    s.read_exact(&mut len).await.unwrap();
                    let mut m = vec![0u8; usize::from(u16::from_be_bytes(len))];
                    s.read_exact(&mut m).await.unwrap();
                    let mut full = m.clone();
                    full.extend_from_slice(b"-full");
                    let mut out = (full.len() as u16).to_be_bytes().to_vec();
                    out.extend_from_slice(&full);
                    s.write_all(&out).await.unwrap();
                });
            }
        });
        let socket = UdpSocket::bind(("127.0.0.1", port)).await.unwrap();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
                    return;
                };
                let mut reply = buf[..n].to_vec();
                reply[2] |= 0x02; // TC
                let _ = socket.send_to(&reply, peer).await;
            }
        });
        let up = UdpUpstream::new(SocketAddr::from(([127, 0, 0, 1], port)), direct());
        let resp = up.query(&msg(9, 0, b"big"), deadline(2000)).await.unwrap();
        assert_eq!(resp, [msg(9, 0, b"big"), b"-full".to_vec()].concat());
    }

    #[tokio::test]
    async fn rejects_queries_without_a_header() {
        let up = UdpUpstream::new("127.0.0.1:9".parse().unwrap(), direct());
        assert!(matches!(
            up.query(b"short", deadline(100)).await,
            Err(UpstreamError::BadResponse(_))
        ));
    }

    /// How long a test waits for something that is to happen: it ends when
    /// that has, and this is only the bound.
    const BOUND: Duration = Duration::from_secs(10);

    async fn until(done: impl Fn() -> bool) {
        tokio::time::timeout(BOUND, async {
            while !done() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("it happens within the bound");
    }

    /// Two queries that come with the same ID, in flight at once: each is
    /// sent with an ID of its own, and each gets its own answer, under the ID
    /// it came with. The second is sent when the mock has the first and is
    /// to answer it a good while later.
    #[tokio::test]
    async fn udp_upstream_gives_each_query_its_own_id() {
        let mock = MockDns::spawn().await;
        mock.set("a.test", &["10.0.0.1"], &[], 60);
        mock.set("b.test", &["10.0.0.2"], &[], 60);
        mock.set_delay(Duration::from_millis(400));
        let up = Arc::new(UdpUpstream::new(mock.addr(), direct()));
        let ask = |name: &'static str| {
            let up = Arc::clone(&up);
            tokio::spawn(async move {
                up.query(&query_wire(0x4242, name, 1, None), deadline(10_000))
                    .await
            })
        };
        let first = ask("a.test");
        until(|| mock.wires().len() == 1).await;
        let second = ask("b.test");
        let first = parse_response(&first.await.unwrap().unwrap()).unwrap();
        let second = parse_response(&second.await.unwrap().unwrap()).unwrap();
        assert_eq!(
            (first.id, first.v4),
            (0x4242, vec![("10.0.0.1".parse().unwrap(), 60)])
        );
        assert_eq!(
            (second.id, second.v4),
            (0x4242, vec![("10.0.0.2".parse().unwrap(), 60)])
        );
        // the mock was asked under two IDs, the first query's own among them
        let ids: Vec<u16> = mock.wires().iter().map(|w| wire_id(w).unwrap()).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        assert_eq!(ids[0], 0x4242);
        // and neither is left in flight
        assert_eq!(up.in_flight(), 0);
    }

    /// A query that is given up on while it waits — its future dropped, as a
    /// fanout does to those still waiting when it has an answer — gives its
    /// ID back, though no answer ever comes for it.
    #[tokio::test]
    async fn a_query_that_is_dropped_gives_its_id_back() {
        let mock = MockDns::spawn().await;
        mock.set_drop_all(true);
        let up = Arc::new(UdpUpstream::new(mock.addr(), direct()));
        let waiting = {
            let up = Arc::clone(&up);
            tokio::spawn(async move {
                up.query(&query_wire(0x4242, "a.test", 1, None), deadline(60_000))
                    .await
            })
        };
        until(|| mock.wires().len() == 1).await;
        assert_eq!(up.in_flight(), 1);
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());
        assert_eq!(up.in_flight(), 0);
    }

    /// A query that was sent under another ID than it came with, because a
    /// query in flight had the one it came with, is asked again over TCP as it
    /// came: under its own ID, and the answer is handed back under it.
    #[tokio::test]
    async fn a_truncated_answer_to_a_query_sent_under_another_id_is_retried_as_it_came() {
        // TCP echo bound first so the UDP server can share the port number.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let asked_over_tcp = Arc::new(Mutex::new(Vec::new()));
        let asked = Arc::clone(&asked_over_tcp);
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let asked = Arc::clone(&asked);
                tokio::spawn(async move {
                    let mut len = [0u8; 2];
                    s.read_exact(&mut len).await.unwrap();
                    let mut m = vec![0u8; usize::from(u16::from_be_bytes(len))];
                    s.read_exact(&mut m).await.unwrap();
                    asked.lock().unwrap().push(m.clone());
                    let mut full = m;
                    full.extend_from_slice(b"-full");
                    let mut out = (full.len() as u16).to_be_bytes().to_vec();
                    out.extend_from_slice(&full);
                    s.write_all(&out).await.unwrap();
                });
            }
        });
        // every question is answered, a while after, with the TC bit: the
        // second is asked while the first is in flight
        let socket = Arc::new(UdpSocket::bind(("127.0.0.1", port)).await.unwrap());
        let asked_over_udp = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&asked_over_udp);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
                    return;
                };
                seen.lock().unwrap().push(wire_id(&buf[..n]).unwrap());
                let mut reply = buf[..n].to_vec();
                reply[2] |= 0x02; // TC
                let socket = Arc::clone(&socket);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    let _ = socket.send_to(&reply, peer).await;
                });
            }
        });
        let up = Arc::new(UdpUpstream::new(
            SocketAddr::from(([127, 0, 0, 1], port)),
            direct(),
        ));
        let ask = |payload: &'static [u8]| {
            let up = Arc::clone(&up);
            tokio::spawn(async move { up.query(&msg(9, 0, payload), deadline(10_000)).await })
        };
        let (one, two) = (ask(b"one"), ask(b"two"));
        let full = |payload: &[u8]| [msg(9, 0, payload), b"-full".to_vec()].concat();
        assert_eq!(one.await.unwrap().unwrap(), full(b"one"));
        assert_eq!(two.await.unwrap().unwrap(), full(b"two"));
        let ids = asked_over_udp.lock().unwrap().clone();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1], "the second went out under another ID");
        let mut retried = asked_over_tcp.lock().unwrap().clone();
        retried.sort();
        assert_eq!(retried, [msg(9, 0, b"one"), msg(9, 0, b"two")]);
    }

    /// A plain UDP upstream goes through its connector: the question on a
    /// datagram it opens, and the question again over a stream when the
    /// answer is truncated. It opens no socket of its own — in the enhanced
    /// mode the connector is what binds the question to the network outside
    /// the virtual interface (M4 design 7.2).
    #[tokio::test]
    async fn a_udp_upstream_asks_through_its_connector() {
        let mock = MockDns::spawn().await;
        mock.set("a.test", &["10.0.0.1"], &[], 60);
        mock.set_truncate_udp(true);
        let connector = Arc::new(Seen::new());
        let up = UdpUpstream::new(mock.addr(), connector.clone());
        assert_eq!(up.name(), format!("udp://{}", mock.addr()));
        let resp = up
            .query(&query_wire(1, "a.test", 1, None), deadline(10_000))
            .await
            .unwrap();
        assert_eq!(
            parse_response(&resp).unwrap().v4,
            vec![("10.0.0.1".parse().unwrap(), 60)]
        );
        let server = Target::new(HostName::Ip(mock.addr().ip()), mock.addr().port());
        assert_eq!(connector.udp(), vec![server.clone()], "asked on a datagram");
        assert_eq!(connector.tcp(), vec![server], "and again over a stream");
    }

    fn waiter() -> oneshot::Sender<Vec<u8>> {
        oneshot::channel().0
    }

    /// A query keeps the ID it came with unless a query in flight has it;
    /// then it gets another, which none has; and a query that is over gives
    /// its ID back.
    #[test]
    fn a_query_keeps_its_id_unless_one_in_flight_has_it() {
        let pending = Pending::default();
        let first = pending.claim(7, waiter()).unwrap();
        assert_eq!(first.id, 7);
        let other = pending.claim(7, waiter()).unwrap();
        assert_ne!(other.id, 7);
        let third = pending.claim(7, waiter()).unwrap();
        assert!(third.id != 7 && third.id != other.id);
        drop(first);
        assert_eq!(pending.claim(7, waiter()).unwrap().id, 7);
    }

    /// With every one of the 65,536 IDs taken there is none to give; with one
    /// free, wherever the search starts, that one is found.
    #[test]
    fn with_every_id_taken_there_is_none_to_give() {
        let pending = Pending::default();
        let mut held: Vec<Option<Claim<'_>>> = (0..=u16::MAX)
            .map(|id| {
                let claim = pending.claim(id, waiter()).unwrap();
                assert_eq!(claim.id, id);
                Some(claim)
            })
            .collect();
        let none = Some(UpstreamError::Io("too many queries in flight".to_string()));
        assert_eq!(pending.claim(5, waiter()).err(), none);
        let mut found = Vec::new();
        for free in [0, 1, 40_000, u16::MAX] {
            drop(held[usize::from(free)].take());
            let claim = pending.claim(5, waiter()).unwrap();
            assert_eq!(claim.id, free);
            found.push(claim);
            assert_eq!(pending.claim(5, waiter()).err(), none);
        }
    }

    /// An answer goes to the query that holds its ID, the first answer
    /// only; the ID stays taken until the query gives it back.
    #[test]
    fn an_answer_goes_to_the_query_that_holds_its_id_once() {
        let pending = Pending::default();
        let (tx, mut rx) = oneshot::channel();
        let claim = pending.claim(9, tx).unwrap();
        let id = claim.id;
        // an answer for an ID nobody holds goes nowhere
        pending.deliver(id.wrapping_add(1), b"stray".to_vec());
        pending.deliver(id, b"first".to_vec());
        pending.deliver(id, b"again".to_vec());
        assert_eq!(rx.try_recv().unwrap(), b"first");
        assert_ne!(pending.claim(id, waiter()).unwrap().id, id, "still taken");
        drop(claim);
        assert_eq!(pending.claim(id, waiter()).unwrap().id, id);
    }
}
