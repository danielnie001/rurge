//! Port hopping (phase 2 M7 design 3.3): the server is reached on one port
//! of its set at a time, at first a random one; every interval a random
//! other port, from a new local socket. The previous socket stays until
//! the next hop: what the server sent before it saw the change still
//! arrives. quinn never learns: what it sends goes to the current port
//! whatever the destination, and everything received comes from the
//! address quinn knows the server by (a client drops packets from any
//! other).

use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, SocketAddr};
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::task::JoinHandle;

/// Makes the socket of a hop: the same kind each time.
pub(crate) type Open = Box<dyn Fn() -> io::Result<Arc<dyn AsyncUdpSocket>> + Send + Sync>;

/// The ports a connection hops among: each once, in ascending runs.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PortSet(Vec<RangeInclusive<u16>>);

impl PortSet {
    /// `ranges` as written, overlapping or not.
    fn new(ranges: &[RangeInclusive<u16>]) -> PortSet {
        let mut sorted = ranges.to_vec();
        sorted.sort_by_key(|range| *range.start());
        let mut runs: Vec<RangeInclusive<u16>> = Vec::new();
        for range in sorted {
            match runs.last_mut() {
                Some(last) if u32::from(*range.start()) <= u32::from(*last.end()) + 1 => {
                    if range.end() > last.end() {
                        *last = *last.start()..=*range.end();
                    }
                }
                _ => runs.push(range),
            }
        }
        PortSet(runs)
    }

    fn len(&self) -> u32 {
        self.0
            .iter()
            .map(|run| u32::from(*run.end()) - u32::from(*run.start()) + 1)
            .sum()
    }

    /// The port at `index` in ascending order; `index < len()`.
    fn nth(&self, mut index: u32) -> u16 {
        for run in &self.0 {
            let len = u32::from(*run.end()) - u32::from(*run.start()) + 1;
            if index < len {
                return run.start() + index as u16;
            }
            index -= len;
        }
        unreachable!("an index past the set")
    }

    fn index_of(&self, port: u16) -> Option<u32> {
        let mut before = 0;
        for run in &self.0 {
            if run.contains(&port) {
                return Some(before + u32::from(port - run.start()));
            }
            before += u32::from(*run.end()) - u32::from(*run.start()) + 1;
        }
        None
    }

    /// Any port of the set, all alike likely.
    fn random(&self) -> u16 {
        self.nth(below(self.len()))
    }

    /// Any port of the set but `port`, all alike likely; `port` itself
    /// when it is the only one.
    fn other_than(&self, port: u16) -> u16 {
        match self.index_of(port) {
            Some(at) if self.len() > 1 => {
                let pick = below(self.len() - 1);
                self.nth(if pick >= at { pick + 1 } else { pick })
            }
            _ => self.random(),
        }
    }
}

/// A random number below `n` (`n > 0`).
fn below(n: u32) -> u32 {
    getrandom::u32().unwrap_or(0) % n
}

#[derive(Debug)]
struct State {
    /// The server port in use, and the socket that sends to it.
    port: u16,
    current: Arc<dyn AsyncUdpSocket>,
    /// The socket before the last hop: still heard.
    previous: Option<Arc<dyn AsyncUdpSocket>>,
    /// Counts the hops: a poller sees that its socket was replaced.
    hops: u64,
    /// The endpoint's receive loop, woken by a hop to listen on the new
    /// socket.
    reader: Option<Waker>,
}

#[derive(Debug)]
struct Shared {
    server: IpAddr,
    /// What quinn knows the server by: the first port's address.
    remote: SocketAddr,
    ports: PortSet,
    state: Mutex<State>,
}

impl Shared {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("the hopping state")
    }

    /// Moves to a random other port, sending from `socket` from now on.
    fn hop(&self, socket: Arc<dyn AsyncUdpSocket>) {
        let (_retired, reader) = {
            let mut state = self.state();
            state.port = self.ports.other_than(state.port);
            let previous = std::mem::replace(&mut state.current, socket);
            state.hops += 1;
            (state.previous.replace(previous), state.reader.take())
        };
        if let Some(reader) = reader {
            reader.wake();
        }
    }
}

#[derive(Debug)]
pub(crate) struct Hopping {
    shared: Arc<Shared>,
    /// Hops every interval; ends with the socket.
    timer: JoinHandle<()>,
}

impl Hopping {
    /// Starts on a random port of `ports` at `server`, from a socket `open`
    /// makes, and hops every `interval`. A hop whose socket cannot be made
    /// is skipped, as the reference implementation does.
    pub(crate) fn start(
        server: IpAddr,
        ports: &[RangeInclusive<u16>],
        interval: Duration,
        open: Open,
    ) -> io::Result<Hopping> {
        let ports = PortSet::new(ports);
        if ports.len() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "quic: no port to hop among",
            ));
        }
        let port = ports.random();
        let shared = Arc::new(Shared {
            server,
            remote: SocketAddr::new(server, port),
            ports,
            state: Mutex::new(State {
                port,
                current: open()?,
                previous: None,
                hops: 0,
                reader: None,
            }),
        });
        let timer = tokio::spawn({
            let shared = shared.clone();
            async move {
                loop {
                    tokio::time::sleep(interval).await;
                    match open() {
                        Ok(socket) => shared.hop(socket),
                        Err(e) => {
                            tracing::debug!(error = %e, "no socket for the next hop; staying on this port");
                        }
                    }
                }
            }
        });
        Ok(Hopping { shared, timer })
    }

    /// What quinn knows the server by.
    pub(crate) fn remote(&self) -> SocketAddr {
        self.shared.remote
    }
}

impl Drop for Hopping {
    fn drop(&mut self) {
        self.timer.abort();
    }
}

impl AsyncUdpSocket for Hopping {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(HopPoller {
            shared: self.shared.clone(),
            inner: None,
        })
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        let (socket, port) = {
            let state = self.shared.state();
            (state.current.clone(), state.port)
        };
        socket.try_send(&Transmit {
            destination: SocketAddr::new(self.shared.server, port),
            ..transmit.clone()
        })
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let (current, previous) = {
            let mut state = self.shared.state();
            if !state
                .reader
                .as_ref()
                .is_some_and(|reader| reader.will_wake(cx.waker()))
            {
                state.reader = Some(cx.waker().clone());
            }
            (state.current.clone(), state.previous.clone())
        };
        for socket in [Some(current), previous].into_iter().flatten() {
            if let Poll::Ready(received) = socket.poll_recv(cx, bufs, meta) {
                let n = received?;
                for meta in &mut meta[..n] {
                    meta.addr = self.shared.remote;
                }
                return Poll::Ready(Ok(n));
            }
        }
        Poll::Pending
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.shared.state().current.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.shared.state().current.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.shared.state().current.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.shared.state().current.may_fragment()
    }
}

/// Waits until the current socket can send, following it across hops.
#[derive(Debug)]
struct HopPoller {
    shared: Arc<Shared>,
    /// The current socket's own poller, and the hop it belongs to.
    inner: Option<(u64, Pin<Box<dyn UdpPoller>>)>,
}

impl UdpPoller for HopPoller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let (hops, current) = {
            let state = this.shared.state();
            (state.hops, state.current.clone())
        };
        if this.inner.as_ref().is_none_or(|(of, _)| *of != hops) {
            this.inner = Some((hops, current.create_io_poller()));
        }
        let (_, inner) = this.inner.as_mut().expect("the current socket's poller");
        inner.as_mut().poll_writable(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quinn::{Runtime, TokioRuntime};
    use std::sync::Weak;
    use tokio::net::UdpSocket;
    use tokio::time::Instant;

    fn set(ranges: &[RangeInclusive<u16>]) -> PortSet {
        PortSet::new(ranges)
    }

    #[test]
    fn the_port_set_merges_what_overlaps_or_touches() {
        assert_eq!(
            set(&[5000..=6000, 1234..=1234, 5500..=6100, 6101..=6101, 80..=80]),
            PortSet(vec![80..=80, 1234..=1234, 5000..=6101])
        );
        let ports = set(&[443..=443, 5000..=5002, 443..=443]);
        assert_eq!(ports.len(), 4);
        assert_eq!(
            (0..4).map(|i| ports.nth(i)).collect::<Vec<_>>(),
            [443, 5000, 5001, 5002]
        );
        assert_eq!(ports.index_of(5001), Some(2));
        assert_eq!(ports.index_of(444), None);
        assert_eq!(set(&[1..=65535]).len(), 65535);
    }

    /// A hop always moves to another port of the set; a set of one stays.
    #[test]
    fn a_hop_moves_to_another_port_of_the_set() {
        let ports = set(&[7000..=7002, 9000..=9000]);
        let mut seen = std::collections::BTreeSet::new();
        let mut port = ports.random();
        for _ in 0..400 {
            let next = ports.other_than(port);
            assert_ne!(next, port);
            assert!(ports.index_of(next).is_some(), "{next}");
            seen.insert(next);
            port = next;
        }
        assert_eq!(
            seen.into_iter().collect::<Vec<_>>(),
            [7000, 7001, 7002, 9000]
        );
        assert_eq!(set(&[443..=443]).other_than(443), 443);
    }

    fn loopback() -> io::Result<Arc<dyn AsyncUdpSocket>> {
        TokioRuntime.wrap_udp_socket(std::net::UdpSocket::bind("127.0.0.1:0")?)
    }

    /// Plain sockets as the server: one per port.
    async fn server(count: usize) -> Vec<UdpSocket> {
        let mut sockets = Vec::new();
        for _ in 0..count {
            sockets.push(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        }
        sockets
    }

    fn ports_of(sockets: &[UdpSocket]) -> Vec<RangeInclusive<u16>> {
        sockets
            .iter()
            .map(|s| {
                let port = s.local_addr().unwrap().port();
                port..=port
            })
            .collect()
    }

    /// Sends as quinn does: waits until the socket can, then tries.
    async fn send(socket: &Arc<Hopping>, bytes: &[u8]) {
        let mut poller = socket.clone().create_io_poller();
        let transmit = Transmit {
            destination: socket.remote(),
            ecn: None,
            contents: bytes,
            segment_size: None,
            src_ip: None,
        };
        tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| {
                loop {
                    if let Err(e) = std::task::ready!(poller.as_mut().poll_writable(cx)) {
                        return Poll::Ready(Err(e));
                    }
                    match socket.try_send(&transmit) {
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        sent => return Poll::Ready(sent),
                    }
                }
            }),
        )
        .await
        .expect("sent within the bound")
        .unwrap();
    }

    async fn recv(socket: &Hopping) -> (Vec<u8>, SocketAddr) {
        let mut storage = [0u8; 1500];
        let mut meta = [RecvMeta::default()];
        let n = tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| {
                let mut bufs = [IoSliceMut::new(&mut storage)];
                socket.poll_recv(cx, &mut bufs, &mut meta)
            }),
        )
        .await
        .expect("a datagram within the bound")
        .unwrap();
        assert_eq!(n, 1);
        (storage[..meta[0].len].to_vec(), meta[0].addr)
    }

    /// Which of `sockets` heard `bytes`, and from where.
    async fn heard(sockets: &[UdpSocket], bytes: &[u8]) -> (usize, SocketAddr) {
        let mut buf = [0u8; 1500];
        let (at, n, from) = tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| {
                for (at, socket) in sockets.iter().enumerate() {
                    let mut read = tokio::io::ReadBuf::new(&mut buf);
                    if let Poll::Ready(from) = socket.poll_recv_from(cx, &mut read) {
                        return Poll::Ready((at, read.filled().len(), from.unwrap()));
                    }
                }
                Poll::Pending
            }),
        )
        .await
        .expect("a datagram within the bound");
        assert_eq!(&buf[..n], bytes);
        (at, from)
    }

    /// What goes out goes to the current port from the current socket,
    /// whatever quinn names; what comes back — on the previous socket too —
    /// comes from the address quinn knows.
    #[tokio::test]
    async fn late_packets_on_the_previous_socket_are_still_heard() {
        let server = server(2).await;
        let socket = Arc::new(
            Hopping::start(
                "127.0.0.1".parse().unwrap(),
                &ports_of(&server),
                Duration::from_secs(3600),
                Box::new(loopback),
            )
            .unwrap(),
        );
        send(&socket, b"first").await;
        let (at, before) = heard(&server, b"first").await;
        assert_eq!(
            server[at].local_addr().unwrap(),
            socket.remote(),
            "starts on the port quinn knows"
        );
        socket.shared.hop(loopback().unwrap());
        send(&socket, b"second").await;
        let (now, after) = heard(&server, b"second").await;
        assert_ne!(now, at, "another port");
        assert_ne!(after, before, "another local socket");
        // the server answers the old socket late, and the new one
        server[at].send_to(b"late", before).await.unwrap();
        assert_eq!(recv(&socket).await, (b"late".to_vec(), socket.remote()));
        server[now].send_to(b"new", after).await.unwrap();
        assert_eq!(recv(&socket).await, (b"new".to_vec(), socket.remote()));
        // one more hop retires the first socket
        socket.shared.hop(loopback().unwrap());
        server[at].send_to(b"gone", before).await.unwrap();
        server[now].send_to(b"kept", after).await.unwrap();
        assert_eq!(recv(&socket).await.0, b"kept");
    }

    /// Hops come by themselves, every interval.
    #[tokio::test]
    async fn the_timer_hops_every_interval() {
        let server = server(3).await;
        let socket = Hopping::start(
            "127.0.0.1".parse().unwrap(),
            &ports_of(&server),
            Duration::from_millis(50),
            Box::new(loopback),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while socket.shared.state().hops < 3 {
            assert!(Instant::now() < deadline, "no hops");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The timer is the socket's: it ends when the socket goes.
    #[tokio::test]
    async fn dropping_the_socket_ends_its_timer() {
        let server = server(2).await;
        let socket = Hopping::start(
            "127.0.0.1".parse().unwrap(),
            &ports_of(&server),
            Duration::from_millis(20),
            Box::new(loopback),
        )
        .unwrap();
        let shared: Weak<Shared> = Arc::downgrade(&socket.shared);
        drop(socket);
        let deadline = Instant::now() + Duration::from_secs(5);
        while shared.upgrade().is_some() {
            assert!(Instant::now() < deadline, "the timer outlived its socket");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[test]
    fn no_port_no_hopping() {
        let err = Hopping::start(
            "127.0.0.1".parse().unwrap(),
            &[],
            Duration::from_secs(30),
            Box::new(loopback),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "quic: no port to hop among");
    }
}
