//! The operating system's side of the device, in a test: a TCP/IP stack
//! (smoltcp 0.12) on an [`AppEnd`] that connects to the adapter and accepts
//! from it, with the packets of both directions in the hands of the test.
//!
//! The stack lives behind one lock, and a task of its own advances it: what
//! arrived is handed to it one packet at a time, then it is advanced until
//! it has nothing more to send, as the stack of a WireGuard tunnel is
//! (`rurge-proto-wireguard`). Its clock is tokio's, so a paused test clock
//! moves it too.

use crate::RX_MAX;
use crate::device::AppEnd;
use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{self, DeviceCapabilities, Medium};
use smoltcp::socket::{AnySocket, tcp};
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint};
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::error::{SendError, TrySendError};
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// The send buffer of every connection (the receive buffer is `RX_MAX`).
const TX_BUFFER: usize = 256 * 1024;
/// How long `connect` waits for the handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a connection whose `AppTcp` is gone may take to close before it is reset.
const LINGER: Duration = Duration::from_secs(30);
/// How long a connection whose `AppTcp` is gone stays once it is in TIME-WAIT:
/// smoltcp holds the ACK of the other side's last segment back for up to
/// 10 ms, and a socket forgotten before it is sent leaves the other side
/// without it.
const TIME_WAIT_GRACE: Duration = Duration::from_millis(20);
/// The ports `connect` picks from.
const PORTS: RangeInclusive<u16> = 49152..=65535;
/// The most packets handed to the stack in one turn of its task.
const BATCH: usize = 256;

/// Which way a packet goes, from the app's side of the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Out of the app's stack, towards the TUN.
    ToTun,
    /// From the TUN, towards the app's stack.
    FromTun,
}

type Filter = Box<dyn FnMut(Direction, &mut Vec<u8>) -> bool + Send>;

/// The operating system's TCP/IP stack on one end of a device. Cloning gives
/// another handle to the same stack; it runs until `close` or until the last
/// handle and the last connection of it are gone.
#[derive(Clone)]
pub struct AppStack {
    guard: Arc<Guard>,
}

/// Ends the stack's task when the last handle to it goes.
struct Guard {
    shared: Arc<Shared>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.shared.stop.cancel();
    }
}

struct Shared {
    state: Mutex<State>,
    /// The stack has something to do that no packet told it of.
    wake: Notify,
    stop: CancellationToken,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("the app stack's state")
    }

    fn wake(&self) {
        self.wake.notify_one();
    }
}

impl AppStack {
    /// The operating system's side: 198.18.0.1/32 and fd00:6152::1/128, a default route through each.
    pub fn start(end: AppEnd) -> AppStack {
        let own = [
            IpCidr::new(IpAddress::v4(198, 18, 0, 1), 32),
            IpCidr::new(
                IpAddress::Ipv6(Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0, 1)),
                128,
            ),
        ];
        AppStack::start_with(end, &own)
    }

    /// The same with other addresses, at most one of each family: the second
    /// stack of a crossed pair has addresses of its own.
    pub fn start_with(end: AppEnd, addresses: &[IpCidr]) -> AppStack {
        let (mtu, tx, rx) = end.into_parts();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::new(mtu, addresses)),
            wake: Notify::new(),
            stop: CancellationToken::new(),
        });
        tokio::spawn(drive(shared.clone(), tx, rx));
        AppStack {
            guard: Arc::new(Guard { shared }),
        }
    }

    fn shared(&self) -> &Arc<Shared> {
        &self.guard.shared
    }

    /// From the next free port of `dst`'s family (49152 up); resolves once established.
    /// `ConnectionRefused` when the SYN is answered with a reset, `TimedOut` after 5 s.
    /// Once the SYN-ACK is taken the connect has succeeded, as on a real system: a reset
    /// right after it shows on the first read or write (`ConnectionReset`).
    pub async fn connect(&self, dst: SocketAddr) -> io::Result<AppTcp> {
        self.open(None, dst).await
    }

    pub async fn connect_from(&self, port: u16, dst: SocketAddr) -> io::Result<AppTcp> {
        self.open(Some(port), dst).await
    }

    async fn open(&self, port: Option<u16>, dst: SocketAddr) -> io::Result<AppTcp> {
        let tcp = {
            let mut state = self.shared().state();
            let (handle, local) = state.connect(port, dst)?;
            AppTcp {
                guard: self.guard.clone(),
                handle,
                local,
            }
        };
        self.shared().wake();
        let established = std::future::poll_fn(|cx| tcp.poll_established(cx, true));
        let outcome = tokio::time::timeout(CONNECT_TIMEOUT, established).await;
        match outcome {
            Ok(Ok(())) => Ok(tcp),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the connection attempt timed out",
            )),
        }
    }

    /// Listens on `local` at once (before the future is first polled) and resolves
    /// with the first connection. Dropping the future stops listening.
    pub fn accept(
        &self,
        local: SocketAddr,
    ) -> impl Future<Output = io::Result<AppTcp>> + Send + 'static {
        // the socket is made here, not in the future: a SYN that comes before
        // the future runs finds it
        let listening = self.shared().state().listen(local).map(|handle| AppTcp {
            guard: self.guard.clone(),
            handle,
            local,
        });
        async move {
            let mut tcp = listening?;
            std::future::poll_fn(|cx| tcp.poll_established(cx, false)).await?;
            // the address the connection came to, which a listener on any address did not name
            let own = tcp.guard.shared.state().local_of(tcp.handle);
            if let Some(own) = own {
                tcp.local = own;
            }
            Ok(tcp)
        }
    }

    /// Packets from the other end whose TCP or UDP destination port is in `ports`
    /// go to the receiver instead of the stack.
    pub fn divert(&self, ports: RangeInclusive<u16>) -> mpsc::UnboundedReceiver<Vec<u8>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.shared().state().diverts.push((ports, tx));
        rx
    }

    /// Sends `packet` to the other end as it is: not through the stack, not
    /// through the filter.
    pub fn inject(&self, packet: Vec<u8>) {
        self.shared().state().outbox.push_back(packet);
        self.shared().wake();
    }

    /// Sees every packet both ways before anything else: `false` drops it; it may change it.
    /// Replaces any filter set before. It runs inside the stack's lock, so it
    /// must not call this stack.
    pub fn set_filter(&self, filter: impl FnMut(Direction, &mut Vec<u8>) -> bool + Send + 'static) {
        self.shared().state().filter = Some(Box::new(filter));
    }

    /// Packets the stack took in and gave out so far: those the filter let
    /// through and the diverts left to it, and those it sent before the
    /// filter saw them. `inject`ed packets are neither.
    pub fn packets(&self) -> (u64, u64) {
        let state = self.shared().state();
        (state.taken, state.given)
    }

    /// Ends the driver now and drops the AppEnd (the memory device then fails);
    /// open `AppTcp`s fail with `ConnectionAborted`.
    pub fn close(&self) {
        self.shared().state().close();
        self.shared().stop.cancel();
    }

    /// The sockets the stack still holds.
    #[cfg(test)]
    pub(crate) fn socket_count(&self) -> usize {
        self.shared().state().sockets.iter().count()
    }
}

fn aborted(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, why)
}

fn reset() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionReset, "the connection was reset")
}

/// One of the stack's connections: a TCP socket, ending when it is dropped.
/// Dropped without a `shutdown` it is reset.
pub struct AppTcp {
    guard: Arc<Guard>,
    handle: SocketHandle,
    local: SocketAddr,
}

impl std::fmt::Debug for AppTcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppTcp")
            .field("local", &self.local)
            .finish_non_exhaustive()
    }
}

impl AppTcp {
    pub fn local(&self) -> SocketAddr {
        self.local
    }

    /// Sends a reset now.
    pub fn abort(&mut self) {
        let mut guard = self.guard.shared.state();
        let state = &mut *guard;
        if state.closed {
            return;
        }
        if let Some(conn) = state.conns.get_mut(&self.handle) {
            conn.aborted = true;
        }
        state.sockets.get_mut::<tcp::Socket>(self.handle).abort();
        drop(guard);
        self.guard.shared.wake();
    }

    /// Whether the handshake is over: ready when the connection is
    /// established (or already ending), an error when it will not be.
    fn poll_established(&self, cx: &mut Context<'_>, connecting: bool) -> Poll<io::Result<()>> {
        let mut guard = self.guard.shared.state();
        let state = &mut *guard;
        if state.closed {
            return Poll::Ready(Err(aborted("the app stack was closed")));
        }
        let conn = &state.conns[&self.handle];
        // the connect has succeeded even if a reset came before this task looked
        if connecting && conn.handshake_done {
            return Poll::Ready(Ok(()));
        }
        let was_reset = conn.reset;
        let socket = state.sockets.get_mut::<tcp::Socket>(self.handle);
        match socket.state() {
            tcp::State::Listen | tcp::State::SynSent | tcp::State::SynReceived => {
                socket.register_recv_waker(cx.waker());
                socket.register_send_waker(cx.waker());
                Poll::Pending
            }
            tcp::State::Closed => Poll::Ready(Err(match (was_reset, connecting) {
                (true, true) => io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "the connection attempt was answered with a reset",
                ),
                (true, false) => reset(),
                (false, _) => aborted("the connection ended before it was established"),
            })),
            _ => Poll::Ready(Ok(())),
        }
    }

    /// Why the connection can no longer be used, when it cannot.
    fn failure(state: &State, handle: SocketHandle) -> Option<io::Error> {
        if state.closed {
            return Some(aborted("the app stack was closed"));
        }
        let conn = &state.conns[&handle];
        if conn.reset {
            Some(reset())
        } else if conn.aborted {
            Some(aborted("this end aborted the connection"))
        } else {
            None
        }
    }
}

impl Drop for AppTcp {
    fn drop(&mut self) {
        let Ok(mut guard) = self.guard.shared.state.lock() else {
            return;
        };
        let state = &mut *guard;
        if state.closed {
            return;
        }
        // the socket is forgotten once it is over, and its handle may be another's then
        state.connecting.retain(|&handle| handle != self.handle);
        if let Some(conn) = state.conns.get(&self.handle) {
            let socket = state.sockets.get_mut::<tcp::Socket>(self.handle);
            // not shut down: reset (a socket that is over has nothing to reset)
            if !conn.shut && !conn.aborted && socket.is_open() {
                socket.abort();
            }
            state.released.push(Released {
                handle: self.handle,
                since: Instant::now(),
                time_wait: None,
            });
        }
        drop(guard);
        self.guard.shared.wake();
    }
}

impl AsyncRead for AppTcp {
    /// The end of the other side's data is `Ok(0)`; a reset it sent is `ConnectionReset`.
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut guard = self.guard.shared.state();
        let state = &mut *guard;
        if let Some(error) = AppTcp::failure(state, self.handle) {
            return Poll::Ready(Err(error));
        }
        let socket = state.sockets.get_mut::<tcp::Socket>(self.handle);
        match socket.recv_slice(buf.initialize_unfilled()) {
            Ok(0) => {
                socket.register_recv_waker(cx.waker());
                Poll::Pending
            }
            Ok(n) => {
                buf.advance(n);
                drop(guard);
                // room in the window to tell the other side of
                self.guard.shared.wake();
                Poll::Ready(Ok(()))
            }
            Err(tcp::RecvError::Finished) => Poll::Ready(Ok(())),
            // taken for another reason: a timeout, say
            Err(tcp::RecvError::InvalidState) => Poll::Ready(Err(reset())),
        }
    }
}

impl AsyncWrite for AppTcp {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut guard = self.guard.shared.state();
        let state = &mut *guard;
        if let Some(error) = AppTcp::failure(state, self.handle) {
            return Poll::Ready(Err(error));
        }
        let shut = state.conns[&self.handle].shut;
        let socket = state.sockets.get_mut::<tcp::Socket>(self.handle);
        match socket.send_slice(data) {
            Ok(0) => {
                socket.register_send_waker(cx.waker());
                Poll::Pending
            }
            Ok(n) => {
                drop(guard);
                self.guard.shared.wake();
                Poll::Ready(Ok(n))
            }
            Err(tcp::SendError::InvalidState) => Poll::Ready(Err(
                if shut
                    || matches!(
                        socket.state(),
                        tcp::State::FinWait1
                            | tcp::State::FinWait2
                            | tcp::State::Closing
                            | tcp::State::LastAck
                            | tcp::State::TimeWait
                    )
                {
                    io::Error::new(io::ErrorKind::BrokenPipe, "the write side was shut down")
                } else {
                    reset()
                },
            )),
        }
    }

    /// Immediately: a write is the stack's as soon as it is accepted.
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    /// Closes the write side, a FIN once what was written is sent; immediately.
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut guard = self.guard.shared.state();
        let state = &mut *guard;
        if state.closed {
            return Poll::Ready(Ok(()));
        }
        if let Some(conn) = state.conns.get_mut(&self.handle) {
            conn.shut = true;
        }
        state.sockets.get_mut::<tcp::Socket>(self.handle).close();
        drop(guard);
        self.guard.shared.wake();
        Poll::Ready(Ok(()))
    }
}

/// A socket whose `AppTcp` is gone, until it is over.
struct Released {
    handle: SocketHandle,
    since: Instant,
    /// When the socket was first seen in TIME-WAIT.
    time_wait: Option<Instant>,
}

/// What the stack knows of one of its sockets besides smoltcp's state.
struct Conn {
    /// The family (true for IPv6) and port taken from the pool `connect` picks from.
    held: Option<(bool, u16)>,
    /// A reset from the other end was taken.
    reset: bool,
    /// The handshake of a `connect` was seen to complete: the connect has succeeded.
    handshake_done: bool,
    /// `abort` was called on this end.
    aborted: bool,
    /// The write side was shut down here.
    shut: bool,
}

struct State {
    iface: Interface,
    sockets: SocketSet<'static>,
    device: Queues,
    addresses: Vec<IpCidr>,
    /// What smoltcp's clock counts from.
    epoch: Instant,
    conns: HashMap<SocketHandle, Conn>,
    /// The sockets of the `connect`s whose handshake is not over yet.
    connecting: Vec<SocketHandle>,
    /// Sockets whose `AppTcp` is gone.
    released: Vec<Released>,
    /// The ports `connect` has given out, by family.
    ports: HashSet<(bool, u16)>,
    next_port: [u16; 2],
    diverts: Vec<(RangeInclusive<u16>, mpsc::UnboundedSender<Vec<u8>>)>,
    filter: Option<Filter>,
    /// Packets for the other end that the task has not sent yet.
    outbox: VecDeque<Vec<u8>>,
    taken: u64,
    given: u64,
    closed: bool,
}

impl State {
    fn new(mtu: usize, addresses: &[IpCidr]) -> State {
        let mut device = Queues::new(mtu);
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = getrandom::u64().unwrap_or(0x5eed);
        let mut iface = Interface::new(config, &mut device, smoltcp::time::Instant::ZERO);
        iface.update_ip_addrs(|list| {
            for address in addresses {
                list.push(*address)
                    .expect("the app stack takes at most two addresses");
            }
        });
        // everything leaves through the stack's own address of its family
        let (mut v4, mut v6) = (false, false);
        for address in addresses {
            match address.address() {
                IpAddress::Ipv4(gateway) if !v4 => {
                    v4 = true;
                    let _ = iface.routes_mut().add_default_ipv4_route(gateway);
                }
                IpAddress::Ipv6(gateway) if !v6 => {
                    v6 = true;
                    let _ = iface.routes_mut().add_default_ipv6_route(gateway);
                }
                _ => {}
            }
        }
        State {
            iface,
            sockets: SocketSet::new(Vec::new()),
            device,
            addresses: addresses.to_vec(),
            epoch: Instant::now(),
            conns: HashMap::new(),
            connecting: Vec::new(),
            released: Vec::new(),
            ports: HashSet::new(),
            next_port: [*PORTS.start(); 2],
            diverts: Vec::new(),
            filter: None,
            outbox: VecDeque::new(),
            taken: 0,
            given: 0,
            closed: false,
        }
    }

    fn at(&self, now: Instant) -> smoltcp::time::Instant {
        smoltcp::time::Instant::from_micros(
            now.saturating_duration_since(self.epoch).as_micros() as i64
        )
    }

    /// The stack's first address of the family of `to`.
    fn local_for(&self, to: IpAddr) -> Option<IpAddr> {
        self.addresses
            .iter()
            .map(|cidr| IpAddr::from(cidr.address()))
            .find(|own| own.is_ipv4() == to.is_ipv4())
    }

    fn new_socket() -> tcp::Socket<'static> {
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; RX_MAX]),
            tcp::SocketBuffer::new(vec![0; TX_BUFFER]),
        );
        // what a test writes goes out as it is written, as on a real socket
        socket.set_nagle_enabled(false);
        socket.set_congestion_control(tcp::CongestionControl::Reno);
        socket
    }

    fn free_port(&mut self, v6: bool) -> Option<u16> {
        let span = usize::from(PORTS.end() - PORTS.start()) + 1;
        for _ in 0..span {
            let port = self.next_port[usize::from(v6)];
            self.next_port[usize::from(v6)] = if port == *PORTS.end() {
                *PORTS.start()
            } else {
                port + 1
            };
            if !self.ports.contains(&(v6, port)) {
                return Some(port);
            }
        }
        None
    }

    /// A socket connecting to `dst` from `port`, or from a port of the pool.
    fn connect(
        &mut self,
        port: Option<u16>,
        dst: SocketAddr,
    ) -> io::Result<(SocketHandle, SocketAddr)> {
        if self.closed {
            return Err(aborted("the app stack was closed"));
        }
        let own = self.local_for(dst.ip()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "the app stack has no address of the destination's family",
            )
        })?;
        let v6 = dst.is_ipv6();
        let port = match port {
            Some(port) if self.ports.contains(&(v6, port)) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "the port is taken by another connection",
                ));
            }
            Some(port) => port,
            None => self.free_port(v6).ok_or_else(|| {
                io::Error::new(io::ErrorKind::AddrNotAvailable, "every port is taken")
            })?,
        };
        let local = SocketAddr::new(own, port);
        let mut socket = State::new_socket();
        socket
            .connect(self.iface.context(), dst, local)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the destination cannot be connected to",
                )
            })?;
        let handle = self.sockets.add(socket);
        self.ports.insert((v6, port));
        self.conns.insert(handle, Conn::new(Some((v6, port))));
        self.connecting.push(handle);
        Ok((handle, local))
    }

    /// A socket listening on `local`.
    fn listen(&mut self, local: SocketAddr) -> io::Result<SocketHandle> {
        if self.closed {
            return Err(aborted("the app stack was closed"));
        }
        let endpoint = IpListenEndpoint {
            addr: (!local.ip().is_unspecified()).then(|| local.ip().into()),
            port: local.port(),
        };
        let mut socket = State::new_socket();
        socket.listen(endpoint).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the address cannot be listened on",
            )
        })?;
        let handle = self.sockets.add(socket);
        self.conns.insert(handle, Conn::new(None));
        Ok(handle)
    }

    fn local_of(&self, handle: SocketHandle) -> Option<SocketAddr> {
        let endpoint = self.sockets.get::<tcp::Socket>(handle).local_endpoint()?;
        Some(SocketAddr::new(endpoint.addr.into(), endpoint.port))
    }

    fn close(&mut self) {
        self.closed = true;
        // wakes whoever waits on a socket
        for (_, socket) in self.sockets.iter_mut() {
            if let Some(socket) = tcp::Socket::downcast_mut(socket) {
                socket.abort();
            }
        }
        self.diverts.clear();
        self.outbox.clear();
    }

    fn passes(&mut self, direction: Direction, packet: &mut Vec<u8>) -> bool {
        match self.filter.as_mut() {
            Some(filter) => filter(direction, packet),
            None => true,
        }
    }

    /// A packet of the other end: through the filter and the diverts, then into the stack.
    fn ingest(&mut self, mut packet: Vec<u8>, now: Instant) {
        if !self.passes(Direction::FromTun, &mut packet) {
            return;
        }
        let peek = Peek::of(&packet);
        if let Some(peek) = &peek {
            let position = self
                .diverts
                .iter()
                .position(|(ports, _)| ports.contains(&peek.dst.port()));
            if let Some(position) = position {
                match self.diverts[position].1.send(packet) {
                    Ok(()) => return,
                    // nobody receives any more: the stack has it after all
                    Err(SendError(back)) => {
                        packet = back;
                        self.diverts.remove(position);
                    }
                }
            }
        }
        // smoltcp does not say a reset was taken: see whether the socket it was for ended
        let reset_for = peek
            .filter(|peek| peek.protocol == TCP && peek.flags & RST != 0)
            .and_then(|peek| self.connection_of(peek.dst, peek.src));
        self.taken += 1;
        self.device.rx.push_back(packet);
        let at = self.at(now);
        self.iface
            .poll_ingress_single(at, &mut self.device, &mut self.sockets);
        if let Some(handle) = reset_for
            && self.sockets.get::<tcp::Socket>(handle).state() == tcp::State::Closed
        {
            self.conns.get_mut(&handle).expect("a known socket").reset = true;
        }
        // a connect is done once its SYN-ACK is taken, though the next packet
        // may close the socket before the connect's task looks
        self.connecting.retain(
            |&handle| match self.sockets.get::<tcp::Socket>(handle).state() {
                tcp::State::SynSent | tcp::State::SynReceived => true,
                tcp::State::Closed => false,
                _ => {
                    self.conns
                        .get_mut(&handle)
                        .expect("a known socket")
                        .handshake_done = true;
                    false
                }
            },
        );
    }

    /// The open socket with this local and remote end.
    fn connection_of(&self, local: SocketAddr, remote: SocketAddr) -> Option<SocketHandle> {
        let (local, remote) = (IpEndpoint::from(local), IpEndpoint::from(remote));
        self.sockets.iter().find_map(|(handle, socket)| {
            let socket = tcp::Socket::downcast(socket)?;
            (socket.state() != tcp::State::Closed
                && socket.local_endpoint() == Some(local)
                && socket.remote_endpoint() == Some(remote))
            .then_some(handle)
        })
    }

    /// Sends what the sockets have to send, until they have no more; what
    /// the stack gave out goes through the filter into the outbox. When it
    /// wants to run again, if it does: when smoltcp does, or when a socket
    /// that is gone is next looked at.
    fn advance(&mut self, now: Instant) -> Option<Duration> {
        let at = self.at(now);
        // a poll sends at most one segment of each connection
        while self
            .iface
            .poll_egress(at, &mut self.device, &mut self.sockets)
            == PollResult::SocketStateChanged
        {}
        while let Some(mut packet) = self.device.tx.pop_front() {
            self.given += 1;
            if self.passes(Direction::ToTun, &mut packet) {
                self.outbox.push_back(packet);
            }
        }
        let reap = self.reap(now);
        let wake = self.iface.poll_delay(at, &self.sockets).map(Duration::from);
        match (wake, reap) {
            (Some(wake), Some(reap)) => Some(wake.min(reap)),
            (wake, reap) => wake.or(reap),
        }
    }

    /// Forgets the sockets of the connections that are gone, once they are over:
    /// in TIME-WAIT for `TIME_WAIT_GRACE` (the ACK of the other side's last
    /// segment may still be owed), or closed with the reset sent (smoltcp
    /// keeps the endpoint of an aborted socket until it has sent its reset).
    /// When the soonest of the sockets kept is next to be looked at, if any
    /// are kept.
    fn reap(&mut self, now: Instant) -> Option<Duration> {
        let mut next: Option<Instant> = None;
        for mut gone in std::mem::take(&mut self.released) {
            let socket = self.sockets.get_mut::<tcp::Socket>(gone.handle);
            let state = socket.state();
            let over = match state {
                tcp::State::TimeWait => {
                    let first = *gone.time_wait.get_or_insert(now);
                    now.saturating_duration_since(first) >= TIME_WAIT_GRACE
                }
                tcp::State::Closed => socket.remote_endpoint().is_none(),
                _ => false,
            };
            if over {
                self.sockets.remove(gone.handle);
                if let Some(held) = self.conns.remove(&gone.handle).and_then(|c| c.held) {
                    self.ports.remove(&held);
                }
                continue;
            }
            if now.saturating_duration_since(gone.since) >= LINGER {
                socket.abort();
            }
            let due = match (state, gone.time_wait) {
                (tcp::State::TimeWait, Some(first)) => first + TIME_WAIT_GRACE,
                _ => gone.since + LINGER,
            };
            next = Some(next.map_or(due, |soonest| soonest.min(due)));
            self.released.push(gone);
        }
        next.map(|due| due.saturating_duration_since(now))
    }
}

impl Conn {
    fn new(held: Option<(bool, u16)>) -> Conn {
        Conn {
            held,
            reset: false,
            handshake_done: false,
            aborted: false,
            shut: false,
        }
    }
}

const TCP: u8 = 6;
const UDP: u8 = 17;
const RST: u8 = 0x04;

/// What the stack looks at of a packet before it takes it in: the ends of a
/// TCP segment or UDP datagram, and the flags of the segment.
struct Peek {
    protocol: u8,
    src: SocketAddr,
    dst: SocketAddr,
    flags: u8,
}

impl Peek {
    /// `None` for anything but the first fragment of an IPv4 or IPv6 packet
    /// (without extension headers) of TCP or UDP.
    fn of(packet: &[u8]) -> Option<Peek> {
        let (protocol, src, dst, header) = match packet.first()? >> 4 {
            4 => {
                let header = usize::from(packet[0] & 15) * 4;
                if header < 20 || packet.len() < header + 4 {
                    return None;
                }
                // the transport header is in the first fragment only
                if u16::from_be_bytes([packet[6], packet[7]]) & 0x1fff != 0 {
                    return None;
                }
                let v4 = |at: usize| {
                    IpAddr::V4(Ipv4Addr::new(
                        packet[at],
                        packet[at + 1],
                        packet[at + 2],
                        packet[at + 3],
                    ))
                };
                (packet[9], v4(12), v4(16), header)
            }
            6 => {
                if packet.len() < 44 {
                    return None;
                }
                let v6 = |at: usize| {
                    let octets: [u8; 16] = packet[at..at + 16].try_into().expect("16 bytes");
                    IpAddr::V6(Ipv6Addr::from(octets))
                };
                (packet[6], v6(8), v6(24), 40)
            }
            _ => return None,
        };
        if protocol != TCP && protocol != UDP {
            return None;
        }
        let port = |at: usize| u16::from_be_bytes([packet[header + at], packet[header + at + 1]]);
        Some(Peek {
            protocol,
            src: SocketAddr::new(src, port(0)),
            dst: SocketAddr::new(dst, port(2)),
            flags: if protocol == TCP {
                packet.get(header + 13).copied().unwrap_or(0)
            } else {
                0
            },
        })
    }
}

/// The task of a stack: hands in what arrives, advances the stack, sends what
/// it gives out. It waits for room to send and for packets to arrive at the
/// same time, so two stacks on a crossed pair cannot wait on each other.
async fn drive(shared: Arc<Shared>, tx: mpsc::Sender<Vec<u8>>, mut rx: mpsc::Receiver<Vec<u8>>) {
    let mut arrived = Vec::new();
    let mut pending: VecDeque<Vec<u8>> = VecDeque::new();
    // the other end can still send, and still take packets
    let (mut receiving, mut sending) = (true, true);
    loop {
        let wait = {
            let mut state = shared.state();
            let now = Instant::now();
            for packet in arrived.drain(..) {
                state.ingest(packet, now);
            }
            let wait = state.advance(now);
            pending.extend(state.outbox.drain(..));
            wait
        };
        while sending {
            let Some(packet) = pending.pop_front() else {
                break;
            };
            match tx.try_send(packet) {
                Ok(()) => {}
                Err(TrySendError::Full(packet)) => {
                    pending.push_front(packet);
                    break;
                }
                Err(TrySendError::Closed(_)) => sending = false,
            }
        }
        if !sending {
            pending.clear();
        }
        let wake_at = wait.map(|wait| Instant::now() + wait);
        tokio::select! {
            biased;
            _ = shared.stop.cancelled() => return,
            // the permit is let go at once: the next turn finds the room
            room = tx.reserve(), if sending && !pending.is_empty() => {
                if room.is_err() {
                    sending = false;
                }
            }
            packet = rx.recv(), if receiving => match packet {
                Some(packet) => {
                    arrived.push(packet);
                    while arrived.len() < BATCH {
                        let Ok(more) = rx.try_recv() else { break };
                        arrived.push(more);
                    }
                }
                None => receiving = false,
            },
            _ = shared.wake.notified() => {}
            _ = tokio::time::sleep_until(wake_at.unwrap_or_else(Instant::now)), if wake_at.is_some() => {}
        }
    }
}

/// The stack's device: packets from the other end on their way in, packets
/// of the stack on their way out.
struct Queues {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
    mtu: usize,
}

impl Queues {
    fn new(mtu: usize) -> Queues {
        Queues {
            rx: VecDeque::new(),
            tx: VecDeque::new(),
            mtu,
        }
    }
}

struct RxToken(Vec<u8>);

struct TxToken<'a>(&'a mut VecDeque<Vec<u8>>);

impl phy::Device for Queues {
    type RxToken<'a>
        = RxToken
    where
        Self: 'a;
    type TxToken<'a>
        = TxToken<'a>
    where
        Self: 'a;

    fn receive(
        &mut self,
        _timestamp: smoltcp::time::Instant,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let packet = self.rx.pop_front()?;
        Some((RxToken(packet), TxToken(&mut self.tx)))
    }

    fn transmit(&mut self, _timestamp: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        Some(TxToken(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps
    }
}

impl phy::RxToken for RxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut packet = vec![0; len];
        let done = f(&mut packet);
        self.0.push_back(packet);
        done
    }
}
