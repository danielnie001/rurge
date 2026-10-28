//! Connection establishment (M2 design §5.1). M2 ships `DirectConnector`; M3
//! injects a connector that runs the session pipeline behind the same trait.

use crate::BoxFuture;
use crate::socket::{Family, NoopSocketHook, SocketHook, SocketOpts, plan_addresses, race};
use rurge_config::HostName;
use rurge_config::spec::IpVersion;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpStream, UdpSocket};

/// What a UDP flow's socket asks for as its buffers each way: what
/// wireguard-go asks for its tunnels.
const UDP_BUFFER: usize = 7 << 20;

pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}
pub type BoxedStream = Box<dyn AsyncStream>;

/// One UDP flow to a fixed peer (phase 2 M4 design 6.6): a WireGuard
/// tunnel's carrier to one of its peers. Polled rather than awaited, so that
/// one task can wait on several carriers at once.
pub trait Datagram: Send + Sync {
    /// Sends `buf` as one datagram.
    fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>;
    /// Waits until a send is worth trying: for a sender that keeps its
    /// datagram while the carrier cannot take it (a QUIC connection through
    /// the carrier). A carrier whose `poll_send` may be `Pending` says here
    /// when, and one that wraps another passes it on; the others — whose
    /// `poll_send` always takes or drops — are always ready.
    fn poll_send_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let _ = cx;
        Poll::Ready(Ok(()))
    }
    /// Receives one datagram into `buf`.
    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>>;
    /// Where the datagrams go, when the carrier knows.
    fn peer_addr(&self) -> Option<SocketAddr> {
        None
    }
    /// The IP TOS (IPv6 traffic class) byte of the datagrams sent from now
    /// on; `0` goes back to what the carrier started with. A carrier that
    /// cannot mark its datagrams ignores it.
    fn set_tos(&self, tos: u8) -> io::Result<()> {
        let _ = tos;
        Ok(())
    }
}

pub type BoxedDatagram = Box<dyn Datagram>;

/// One client association's UDP carrier on one outbound (phase 2 M5 design
/// 4.1): datagrams go to, and come back from, any address — the carrier of
/// a full-cone association. `send_to` and `recv_from` may run at the same
/// time, from two tasks.
pub trait PacketSocket: Send + Sync {
    /// Where the datagrams for `to` really go. A carrier that looks names up
    /// itself (DIRECT) answers with an address; the others hand `to` back:
    /// their server resolves it.
    fn resolve<'a>(&'a self, to: &'a Target) -> BoxFuture<'a, io::Result<Target>> {
        Box::pin(std::future::ready(Ok(to.clone())))
    }
    /// Sends `buf` as one datagram to `to`.
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>>;
    /// Receives one datagram into `buf`: its length, and where it came from.
    /// `buf` should hold 64 KiB: Windows fails the receive (`WSAEMSGSIZE`)
    /// when a datagram does not fit, and a carrier that strips a header
    /// (SOCKS5) receives the header too.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>>;
}

pub type BoxedPacketSocket = Box<dyn PacketSocket>;

/// Whether a `PacketSocket::send_to` that failed loses only its datagram,
/// as any hop may lose one: the carrier will not take it (too long for its
/// framing, a name it cannot write) or the system turned it away (no buffer
/// space, too long for it, no route for now). Anything else says the
/// carrier cannot reach the target.
pub fn lost_alone(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::InvalidInput || e.raw_os_error().is_some()
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub host: HostName,
    pub port: u16,
}

impl Target {
    pub fn new(host: HostName, port: u16) -> Target {
        Target { host, port }
    }
}

#[derive(Clone, Debug)]
pub struct ConnectOpts {
    /// For `DirectConnector`: one budget covering name resolution and the
    /// whole race across every address. `BootstrapConnector` (`rurge-dns`)
    /// applies it again to each resolved address in turn instead.
    pub timeout: Duration,
}

impl Default for ConnectOpts {
    fn default() -> Self {
        ConnectOpts {
            timeout: Duration::from_secs(10),
        }
    }
}

pub trait Connector: Send + Sync {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>>;

    /// A UDP flow to `target` (phase 2 M4 design 6.6); `Unsupported` from a
    /// connector that carries none.
    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        let _ = (target, opts);
        Box::pin(std::future::ready(Err(no_udp())))
    }

    /// A UDP carrier that sends to any address (phase 2 M5 design 4.3);
    /// `Unsupported` from a connector that carries none.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedPacketSocket>> {
        let _ = opts;
        Box::pin(std::future::ready(Err(no_udp())))
    }

    /// `connect` to `target`, which is an address `name` resolved to (phase 3
    /// / M5 design 9.2). A connector that tells its sessions by name — the
    /// engine's DNS pipeline — takes the name along; every other dials
    /// `target` as `connect` would.
    fn connect_resolved<'a>(
        &'a self,
        name: &'a str,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        let _ = name;
        self.connect(target, opts)
    }

    /// `connect_udp` to `target`, which is an address `name` resolved to.
    fn connect_udp_resolved<'a>(
        &'a self,
        name: &'a str,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        let _ = name;
        self.connect_udp(target, opts)
    }

    /// Where this connector's connections go now, when that can change
    /// under it (a policy group's member as the underlying proxy): a
    /// connection made before it changed goes elsewhere. `None`: always the
    /// same way.
    ///
    /// A connector that dials directly answers with the egress table its own
    /// sockets are bound to while the enhanced mode is on (`Route::egress`,
    /// M4 design 7.1): the sockets it made before the table changed are the
    /// ones that have to be made again (the phase 3 / M4b plan's execution
    /// record row 68).
    fn route(&self) -> Option<Route> {
        None
    }

    /// This connector, when it dials directly: a QUIC connection then sends
    /// from UDP sockets of its own, made the way this connector makes them
    /// (phase 2 M7 design 3.3); through any other connector it goes over
    /// `connect_udp`'s carrier.
    fn as_direct(&self) -> Option<&DirectConnector> {
        None
    }
}

/// Stands for the object a connector's connections go through now — a
/// policy's outbound. Two are equal when they stand for the same object;
/// one keeps its object alive, so no newer one can take its place in memory
/// while it is compared.
///
/// One kind of object is not an outbound: the egress table of the enhanced
/// mode (`Route::egress`). Such a route says which table rurge's own sockets
/// are bound to, for a socket that outlives one connection — a WireGuard
/// carrier, a QUIC connection — to be made again when that table changes
/// (M4 design 7.1, 7.2; the phase 3 / M4b plan's execution record row 68).
#[derive(Clone)]
pub struct Route {
    at: usize,
    /// Whether this route is an egress table, not an outbound.
    egress: bool,
    _alive: Arc<dyn Send + Sync>,
}

impl Route {
    pub fn of<T: ?Sized + Send + Sync + 'static>(object: &Arc<T>) -> Route {
        Route {
            at: Arc::as_ptr(object) as *const () as usize,
            egress: false,
            _alive: Arc::new(object.clone()),
        }
    }

    /// The egress table rurge's own sockets are bound to now (`SocketHook::
    /// egress_route`): kept by pointer, as `of` keeps its object, so a table
    /// the manager writes again is another route and the same one is not.
    pub fn egress<T: ?Sized + Send + Sync + 'static>(table: &Arc<T>) -> Route {
        Route {
            at: Arc::as_ptr(table) as *const () as usize,
            egress: true,
            _alive: Arc::new(table.clone()),
        }
    }

    /// Whether this route stands for the egress table rurge's own sockets are
    /// bound to, rather than for an outbound.
    pub fn is_egress(&self) -> bool {
        self.egress
    }
}

impl PartialEq for Route {
    /// The object, not how the route was made: two routes to the same table
    /// are equal whether they name it as an egress or as an outbound.
    fn eq(&self, other: &Route) -> bool {
        self.at == other.at
    }
}

impl std::fmt::Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Route({:#x})", self.at)
    }
}

fn no_udp() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "this connection cannot carry UDP",
    )
}

pub trait Resolve: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>>;

    /// `host`'s addresses, the questions leaving the way `via` says (a
    /// policy's `interface` with `dns-follow-interface`, phase 2 M5 design
    /// 8.5). A resolver that sends no questions of its own resolves as
    /// usual.
    fn resolve_via<'a>(
        &'a self,
        host: &'a str,
        via: &'a Via,
    ) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        let _ = via;
        self.resolve(host)
    }
}

/// How a lookup's DNS questions leave: through `connector`, their answers
/// kept apart from the others' under `key` (an interface's name).
#[derive(Clone)]
pub struct Via {
    pub key: String,
    pub connector: Arc<dyn Connector>,
}

/// `inner`, every lookup of it through `via`.
pub struct ResolveVia {
    inner: Arc<dyn Resolve>,
    via: Via,
}

impl ResolveVia {
    pub fn new(inner: Arc<dyn Resolve>, via: Via) -> ResolveVia {
        ResolveVia { inner, via }
    }
}

impl Resolve for ResolveVia {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        self.inner.resolve_via(host, &self.via)
    }
}

/// The operating system resolver (`getaddrinfo` through tokio).
pub struct SystemResolve;

impl Resolve for SystemResolve {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            let addrs: Vec<IpAddr> = tokio::net::lookup_host((host, 0))
                .await?
                .map(|sa| sa.ip())
                .collect();
            if addrs.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no addresses for {host}"),
                ));
            }
            Ok(addrs)
        })
    }
}

/// Alternates address families, starting with the preferred one.
pub fn interleave(addrs: Vec<IpAddr>, prefer_v6: bool) -> Vec<IpAddr> {
    let (v6, v4): (Vec<IpAddr>, Vec<IpAddr>) = addrs.into_iter().partition(|a| a.is_ipv6());
    let (mut first, mut second) = if prefer_v6 {
        (v6.into_iter(), v4.into_iter())
    } else {
        (v4.into_iter(), v6.into_iter())
    };
    let mut out = Vec::new();
    loop {
        match (first.next(), second.next()) {
            (None, None) => break,
            (a, b) => {
                out.extend(a);
                out.extend(b);
            }
        }
    }
    out
}

/// Plain TCP: resolves through `Resolve`, then races the addresses the
/// policy's `ip-version` allows (`crate::socket::race`). Plain UDP too: the
/// first of those addresses, nothing to race.
#[derive(Clone)]
pub struct DirectConnector {
    resolver: Arc<dyn Resolve>,
    opts: SocketOpts,
    hook: Arc<dyn SocketHook>,
    /// The `allow-other-interface` fallback is logged once per connector.
    fallback_logged: Arc<AtomicBool>,
}

impl DirectConnector {
    pub fn new(resolver: Arc<dyn Resolve>) -> DirectConnector {
        DirectConnector::with_opts(resolver, SocketOpts::default(), Arc::new(NoopSocketHook))
    }

    pub fn with_opts(
        resolver: Arc<dyn Resolve>,
        opts: SocketOpts,
        hook: Arc<dyn SocketHook>,
    ) -> DirectConnector {
        DirectConnector {
            resolver,
            opts,
            hook,
            fallback_logged: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// A non-blocking socket of `family` with the options that have to go on it
/// before anything binds it: the IP TOS, and a UDP socket of IPv6 that takes
/// IPv6 only. The one place rurge makes an outbound socket (M4 design 3.2,
/// 7.2).
fn new_socket(
    family: Family,
    kind: socket2::Type,
    opts: &SocketOpts,
    hook: &dyn SocketHook,
) -> io::Result<socket2::Socket> {
    let domain = match family {
        Family::V4 => socket2::Domain::IPV4,
        Family::V6 => socket2::Domain::IPV6,
    };
    let protocol = if kind == socket2::Type::DGRAM {
        socket2::Protocol::UDP
    } else {
        socket2::Protocol::TCP
    };
    let socket = socket2::Socket::new(domain, kind, Some(protocol))?;
    socket.set_nonblocking(true)?;
    // a UDP socket of one family only, set before `interface`: on Windows
    // that binds the socket, and a bound socket takes the option no more
    if kind == socket2::Type::DGRAM && family == Family::V6 {
        socket.set_only_v6(true)?;
    }
    if opts.tos != 0
        && let Err(e) = hook.set_tos(&socket, family, opts.tos)
    {
        tracing::debug!(error = %e, tos = opts.tos, "cannot set the IP TOS; connecting without it");
    }
    Ok(socket)
}

/// Binds `socket` to the policy's `interface`, when it names one. `Ok(true)`:
/// bound to it, which is the interface the user chose. `Ok(false)`: nothing to
/// bind, or the binding failed and `allow-other-interface` took the failure —
/// the caller then asks for an egress. `Err`: the interface cannot be used and
/// may not be left.
fn bind_policy_interface(
    socket: &socket2::Socket,
    family: Family,
    opts: &SocketOpts,
    hook: &dyn SocketHook,
    fallback_logged: &AtomicBool,
) -> io::Result<bool> {
    let Some(interface) = &opts.interface else {
        return Ok(false);
    };
    match hook.bind_interface(socket, interface, family) {
        Ok(()) => Ok(true),
        Err(e) if opts.allow_other_interface => {
            if !fallback_logged.swap(true, Ordering::Relaxed) {
                tracing::warn!(interface = %interface, error = %e, "interface unavailable; using the default one (allow-other-interface)");
            }
            Ok(false)
        }
        Err(e) => Err(io::Error::new(
            e.kind(),
            format!("cannot use interface {interface}: {e}"),
        )),
    }
}

/// Binds `socket` to the interface `egress` answered for `dst`: rurge's own
/// connection must not go into the virtual adapter of the enhanced mode. A
/// binding that fails fails the connection — not binding would send it there
/// (M4 plan P2).
fn bind_egress(
    socket: &socket2::Socket,
    family: Family,
    dst: IpAddr,
    interface: &str,
    hook: &dyn SocketHook,
) -> io::Result<()> {
    match hook.bind_interface(socket, interface, family) {
        Ok(()) => {
            tracing::debug!(%dst, interface, "rurge's own connection leaves through the egress interface");
            Ok(())
        }
        Err(e) => {
            // the names and the system's own words stay in DEBUG; what goes
            // into request records and the API is only the kind of failure
            // (M4 plan P3)
            tracing::debug!(%dst, interface, error = %e, "rurge's own connection cannot leave through the egress interface");
            Err(io::Error::new(
                e.kind(),
                format!(
                    "cannot bind to the network outside the enhanced mode: {}",
                    e.kind()
                ),
            ))
        }
    }
}

/// A non-blocking socket of `family` with the policy's socket options on it,
/// bound to the policy's `interface` or to the egress the hook answers for
/// `dst` — the interface rurge's own connection to `dst` leaves through.
fn open_socket(
    family: Family,
    dst: Option<IpAddr>,
    kind: socket2::Type,
    opts: &SocketOpts,
    hook: &dyn SocketHook,
    fallback_logged: &AtomicBool,
) -> io::Result<socket2::Socket> {
    let socket = new_socket(family, kind, opts, hook)?;
    if bind_policy_interface(&socket, family, opts, hook, fallback_logged)? {
        return Ok(socket);
    }
    let Some(dst) = dst else {
        return Ok(socket);
    };
    if let Some(interface) = hook.egress(dst)? {
        bind_egress(&socket, family, dst, &interface, hook)?;
    }
    Ok(socket)
}

/// The unspecified address of `family`, port 0.
fn unspecified(family: Family) -> SocketAddr {
    match family {
        Family::V4 => SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0),
        Family::V6 => SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0),
    }
}

/// An unconnected UDP socket out of `socket`: room for a burst, and bound to
/// the unspecified address unless binding it to an interface bound it already
/// (Windows binds to an interface by binding to its address).
fn finish_udp(socket: socket2::Socket, family: Family) -> io::Result<std::net::UdpSocket> {
    make_room(&socket);
    if !is_bound(&socket) {
        socket.bind(&unspecified(family).into())?;
    }
    Ok(socket.into())
}

async fn connect_one(
    addr: SocketAddr,
    opts: &SocketOpts,
    hook: &dyn SocketHook,
    fallback_logged: &AtomicBool,
) -> io::Result<TcpStream> {
    let socket = open_socket(
        Family::of(&addr.ip()),
        Some(addr.ip()),
        socket2::Type::STREAM,
        opts,
        hook,
        fallback_logged,
    )?;
    let std_stream: std::net::TcpStream = socket.into();
    let stream = tokio::net::TcpSocket::from_std_stream(std_stream)
        .connect(addr)
        .await?;
    let _ = stream.set_nodelay(true);
    crate::socket::keep_alive(&stream);
    Ok(stream)
}

/// `host:port` the way it is dialled: an IPv6 literal goes in brackets.
fn display_target(target: &Target) -> String {
    match &target.host {
        HostName::Ip(IpAddr::V6(v6)) => format!("[{v6}]:{}", target.port),
        host => format!("{host}:{}", target.port),
    }
}

impl DirectConnector {
    /// The addresses to try first and the ones that join later
    /// (`plan_addresses`); the first list is never empty.
    async fn plan(&self, target: &Target) -> io::Result<(Vec<IpAddr>, Vec<IpAddr>)> {
        match &target.host {
            // `ip-version` only means something for a host name (manual)
            HostName::Ip(ip) => Ok((vec![*ip], Vec::new())),
            HostName::Domain(d) => {
                let addrs = self.resolver.resolve(d).await?;
                // Guards a `Resolve` implementation that answers with
                // an empty list instead of an error; the wording
                // matches what the two production resolvers
                // (`SystemResolve`, `rurge_dns::Resolver`) already
                // return as an `Err` themselves in that case.
                if addrs.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("no addresses for {d}"),
                    ));
                }
                let planned = plan_addresses(addrs, self.opts.ip_version, self.opts.v6_first);
                if planned.0.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "no usable address for {d}: every answer was filtered out by ip-version"
                        ),
                    ));
                }
                Ok(planned)
            }
        }
    }

    /// Where a UDP flow to `target` goes: nothing answers a UDP
    /// "connection", so the first address `plan` gives.
    pub(crate) async fn udp_peer(&self, target: &Target) -> io::Result<SocketAddr> {
        let (primary, _) = self.plan(target).await?;
        Ok(SocketAddr::new(primary[0], target.port))
    }

    /// An unconnected UDP socket of `family` with the policy's socket options
    /// and room for a burst, bound to the unspecified address — unless binding
    /// to the policy's interface bound it already (Windows binds to an
    /// interface by binding to its address). `dst`, when given, is the
    /// destination the socket is made for: the hook is asked for the egress
    /// that does not go into the virtual adapter — an unconnected socket for
    /// DIRECT's carrier asks per datagram instead (`DirectPacket`).
    pub(crate) fn bind_udp(
        &self,
        family: Family,
        dst: Option<IpAddr>,
    ) -> io::Result<std::net::UdpSocket> {
        let socket = open_socket(
            family,
            dst,
            socket2::Type::DGRAM,
            &self.opts,
            self.hook.as_ref(),
            &self.fallback_logged,
        )?;
        finish_udp(socket, family)
    }

    /// The policy's `tos`; `0` leaves the kernel default.
    pub(crate) fn tos(&self) -> u8 {
        self.opts.tos
    }
}

/// `attempt`, with `timeout` covering name resolution and everything after.
async fn within<T>(
    target: &Target,
    timeout: Duration,
    attempt: impl std::future::Future<Output = io::Result<T>>,
) -> io::Result<T> {
    match tokio::time::timeout(timeout, attempt).await {
        Ok(done) => done,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("connect to {} timed out", display_target(target)),
        )),
    }
}

impl Connector for DirectConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(within(target, opts.timeout, async move {
            let (primary, secondary) = self.plan(target).await?;
            let port = target.port;
            let with_port = |ips: Vec<IpAddr>| -> Vec<SocketAddr> {
                ips.into_iter()
                    .map(|ip| SocketAddr::new(ip, port))
                    .collect()
            };
            let (socket_opts, hook, logged) = (
                self.opts.clone(),
                self.hook.clone(),
                self.fallback_logged.clone(),
            );
            let stream = race(with_port(primary), with_port(secondary), move |addr| {
                let (socket_opts, hook, logged) =
                    (socket_opts.clone(), hook.clone(), logged.clone());
                async move { connect_one(addr, &socket_opts, hook.as_ref(), &logged).await }
            })
            .await?;
            // noted here, not in the attempt: each attempt of the race runs
            // in a task of its own, where no trail is followed
            if let Ok(peer) = stream.peer_addr() {
                crate::trail::connected(peer);
            }
            Ok(Box::new(stream) as BoxedStream)
        }))
    }

    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        Box::pin(within(target, opts.timeout, async move {
            let addr = self.udp_peer(target).await?;
            let socket = open_socket(
                Family::of(&addr.ip()),
                Some(addr.ip()),
                socket2::Type::DGRAM,
                &self.opts,
                self.hook.as_ref(),
                &self.fallback_logged,
            )?;
            make_room(&socket);
            socket.connect(&addr.into())?;
            let socket = UdpSocket::from_std(socket.into())?;
            Ok(Box::new(DirectDatagram {
                socket,
                family: Family::of(&addr.ip()),
                hook: self.hook.clone(),
                tos: self.opts.tos,
            }) as BoxedDatagram)
        }))
    }

    fn open_udp<'a>(
        &'a self,
        _opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedPacketSocket>> {
        Box::pin(std::future::ready(
            self.open_packet().map(|p| Box::new(p) as BoxedPacketSocket),
        ))
    }

    fn as_direct(&self) -> Option<&DirectConnector> {
        Some(self)
    }

    /// The egress table this connector's sockets are bound to now, as its
    /// hook reads it: another table means the sockets it made are bound to
    /// the one that is gone (the phase 3 / M4b plan's execution record row
    /// 68).
    fn route(&self) -> Option<Route> {
        self.hook.egress_route()
    }
}

impl DirectConnector {
    /// One unconnected UDP socket per address family the policy's
    /// `ip-version` allows (`bind_udp`). A family this machine cannot open
    /// a socket of is left out, unless it is the only one.
    fn open_packet(&self) -> io::Result<DirectPacket> {
        let families: &[Family] = match self.opts.ip_version {
            IpVersion::V4Only => &[Family::V4],
            IpVersion::V6Only => &[Family::V6],
            _ => &[Family::V4, Family::V6],
        };
        let (mut v4, mut v6, mut failure) = (None, None, None);
        for family in families {
            let opened = self.open_base(*family);
            match (opened, family) {
                (Ok(socket), Family::V4) => v4 = Some(socket),
                (Ok(socket), Family::V6) => v6 = Some(socket),
                (Err(e), _) => failure = Some(e),
            }
        }
        if v4.is_none() && v6.is_none() {
            return Err(failure.unwrap_or_else(no_udp));
        }
        Ok(DirectPacket {
            inner: Mutex::new(Sockets {
                v4,
                v6,
                bound: Vec::new(),
                waiting: None,
            }),
            hook: self.hook.clone(),
            resolver: self.resolver.clone(),
            opts: self.opts.clone(),
        })
    }

    /// One socket of the carrier for `family`: no egress is asked (the carrier
    /// sends anywhere, so there is no destination yet), and the answer to
    /// whether it is bound to the policy's `interface` — those datagrams ask
    /// for no egress either (M4 plan P2).
    fn open_base(&self, family: Family) -> io::Result<(Arc<UdpSocket>, bool)> {
        let socket = new_socket(family, socket2::Type::DGRAM, &self.opts, self.hook.as_ref())?;
        let bound = bind_policy_interface(
            &socket,
            family,
            &self.opts,
            self.hook.as_ref(),
            &self.fallback_logged,
        )?;
        let socket = finish_udp(socket, family)?;
        Ok((Arc::new(UdpSocket::from_std(socket)?), bound))
    }
}

/// DIRECT's UDP carrier: unconnected sockets, one per address family, and one
/// more for each egress a datagram has to leave through — an interface rurge's
/// own connection is bound to, so that it does not go into the virtual adapter
/// of the enhanced mode (M4 design 3.2).
struct DirectPacket {
    inner: Mutex<Sockets>,
    hook: Arc<dyn SocketHook>,
    resolver: Arc<dyn Resolve>,
    opts: SocketOpts,
}

/// The carrier's sockets under one lock: the ones opened with it and the ones
/// made later for an egress.
struct Sockets {
    /// Each family's socket, bound to no egress; and whether it is bound to the
    /// policy's `interface` (then its datagrams ask no egress).
    v4: Option<(Arc<UdpSocket>, bool)>,
    v6: Option<(Arc<UdpSocket>, bool)>,
    /// The sockets bound to an egress, made on first use, never dropped: the
    /// carrier lives as long as its UDP flow, which the engine ends itself.
    bound: Vec<(Family, String, Arc<UdpSocket>)>,
    /// The `recv_from` waiting for a datagram: woken when a socket is added,
    /// whose datagrams it would not see until another socket's came.
    waiting: Option<Waker>,
}

impl Sockets {
    /// The family's own socket, and whether the policy's `interface` bound it.
    fn base(&self, family: Family) -> Option<(Arc<UdpSocket>, bool)> {
        match family {
            Family::V4 => self.v4.clone(),
            Family::V6 => self.v6.clone(),
        }
    }

    /// The socket bound to `interface` for `family`, when one was made.
    fn bound_to(&self, family: Family, interface: &str) -> Option<Arc<UdpSocket>> {
        self.bound
            .iter()
            .find(|(f, name, _)| *f == family && name == interface)
            .map(|(_, _, socket)| socket.clone())
    }

    /// Adds `socket` for `interface`, waking the `recv_from` waiting for a
    /// datagram that would come on it.
    fn add(&mut self, family: Family, interface: String, socket: Arc<UdpSocket>) {
        self.bound.push((family, interface, socket));
        if let Some(waking) = self.waiting.take() {
            waking.wake();
        }
    }

    /// Every socket the carrier receives on, the opened ones first.
    fn all(&self) -> impl Iterator<Item = &Arc<UdpSocket>> {
        [self.v4.as_ref(), self.v6.as_ref()]
            .into_iter()
            .flatten()
            .map(|(socket, _)| socket)
            .chain(self.bound.iter().map(|(_, _, socket)| socket))
    }
}

impl DirectPacket {
    /// The first address of `name` the policy's `ip-version` allows and
    /// this carrier has a socket for.
    async fn address_of(&self, name: &str) -> io::Result<IpAddr> {
        let addrs = self.resolver.resolve(name).await?;
        let (primary, secondary) = plan_addresses(addrs, self.opts.ip_version, self.opts.v6_first);
        primary
            .into_iter()
            .chain(secondary)
            .find(|ip| {
                self.inner
                    .lock()
                    .expect("the carrier's sockets")
                    .base(Family::of(ip))
                    .is_some()
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no usable address for {name}"),
                )
            })
    }

    /// The socket a datagram to `ip` goes out from: the family's own when the
    /// policy's `interface` bound it, otherwise one bound to the interface the
    /// hook answers for `ip` — asked once for this datagram, the socket made on
    /// first use. `Err`: the datagram is not sent — sending it unbound would
    /// put it into the virtual adapter (M4 plan P2). `Ok(None)`: the policy's
    /// `ip-version` allows no socket of this family.
    fn socket_for(&self, ip: IpAddr) -> io::Result<Option<Arc<UdpSocket>>> {
        let family = Family::of(&ip);
        let Some((base, policy_bound)) = self
            .inner
            .lock()
            .expect("the carrier's sockets")
            .base(family)
        else {
            return Ok(None);
        };
        if policy_bound {
            return Ok(Some(base));
        }
        let Some(interface) = self.hook.egress(ip)? else {
            return Ok(Some(base));
        };
        // one lock span: the egress's socket is looked up and, when it is not
        // there yet, made — so that two datagrams at once make one socket, and
        // its key is the interface it was bound to
        let mut inner = self.inner.lock().expect("the carrier's sockets");
        if let Some(socket) = inner.bound_to(family, &interface) {
            return Ok(Some(socket));
        }
        let socket = new_socket(family, socket2::Type::DGRAM, &self.opts, self.hook.as_ref())?;
        bind_egress(&socket, family, ip, &interface, self.hook.as_ref())?;
        let socket = Arc::new(UdpSocket::from_std(finish_udp(socket, family)?)?);
        inner.add(family, interface, socket.clone());
        Ok(Some(socket))
    }
}

impl PacketSocket for DirectPacket {
    fn resolve<'a>(&'a self, to: &'a Target) -> BoxFuture<'a, io::Result<Target>> {
        Box::pin(async move {
            match &to.host {
                HostName::Ip(_) => Ok(to.clone()),
                HostName::Domain(name) => Ok(Target::new(
                    HostName::Ip(self.address_of(name).await?),
                    to.port,
                )),
            }
        })
    }

    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let ip = match &to.host {
                HostName::Ip(ip) => *ip,
                HostName::Domain(name) => self.address_of(name).await?,
            };
            let Some(socket) = self.socket_for(ip)? else {
                return Err(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    format!("this policy sends no UDP to {}", display_target(to)),
                ));
            };
            socket.send_to(buf, SocketAddr::new(ip, to.port)).await?;
            Ok(())
        })
    }

    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(std::future::poll_fn(move |cx| {
            let mut inner = self.inner.lock().expect("the carrier's sockets");
            let mut read = ReadBuf::new(buf);
            for socket in inner.all() {
                loop {
                    match socket.poll_recv_from(cx, &mut read) {
                        // an ICMP "unreachable" for an earlier datagram, which
                        // Windows reports on the next receive: nothing came
                        Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::ConnectionReset => {}
                        Poll::Ready(from) => {
                            let from = from?;
                            return Poll::Ready(Ok((
                                read.filled().len(),
                                Target::new(HostName::Ip(from.ip()), from.port()),
                            )));
                        }
                        Poll::Pending => break,
                    }
                }
            }
            // nothing to read: a socket added under this lock wakes it
            inner.waiting = Some(cx.waker().clone());
            Poll::Pending
        }))
    }
}

/// The socket has a local port: something bound it already.
fn is_bound(socket: &socket2::Socket) -> bool {
    socket
        .local_addr()
        .ok()
        .and_then(|addr| addr.as_socket())
        .is_some_and(|addr| addr.port() != 0)
}

/// A UDP flow may carry a tunnel at full speed: its socket's buffers take a
/// burst. Best effort: the system may cap them.
fn make_room(socket: &socket2::Socket) {
    let _ = socket.set_recv_buffer_size(UDP_BUFFER);
    let _ = socket.set_send_buffer_size(UDP_BUFFER);
}

/// A connected UDP socket carrying the policy's socket options.
struct DirectDatagram {
    socket: UdpSocket,
    family: Family,
    hook: Arc<dyn SocketHook>,
    /// The policy's own `tos`: what `set_tos(0)` goes back to.
    tos: u8,
}

impl Datagram for DirectDatagram {
    fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.socket.poll_send(cx, buf)
    }

    fn poll_send_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.socket.poll_send_ready(cx)
    }

    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        self.socket.poll_recv(cx, buf)
    }

    fn peer_addr(&self) -> Option<SocketAddr> {
        self.socket.peer_addr().ok()
    }

    fn set_tos(&self, tos: u8) -> io::Result<()> {
        let tos = if tos == 0 { self.tos } else { tos };
        self.hook
            .set_tos(&socket2::SockRef::from(&self.socket), self.family, tos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn interleave_alternates_families() {
        let addrs = vec![
            ip("1.1.1.1"),
            ip("2.2.2.2"),
            ip("::1"),
            ip("::2"),
            ip("::3"),
        ];
        assert_eq!(
            interleave(addrs.clone(), false),
            vec![
                ip("1.1.1.1"),
                ip("::1"),
                ip("2.2.2.2"),
                ip("::2"),
                ip("::3")
            ]
        );
        assert_eq!(
            interleave(addrs, true),
            vec![
                ip("::1"),
                ip("1.1.1.1"),
                ip("::2"),
                ip("2.2.2.2"),
                ip("::3")
            ]
        );
        assert!(interleave(vec![], false).is_empty());
    }

    struct Fixed(Vec<IpAddr>);
    impl Resolve for Fixed {
        fn resolve<'a>(&'a self, _: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(async move { Ok(self.0.clone()) })
        }
    }

    #[tokio::test]
    async fn connects_to_ip_and_domain_targets() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let _ = s.write_all(b"hi").await;
                });
            }
        });
        let c = DirectConnector::new(Arc::new(Fixed(vec![ip("127.0.0.1")])));
        for host in ["127.0.0.1", "example.test"] {
            let mut stream = c
                .connect(
                    &Target::new(HostName::parse(host), port),
                    &ConnectOpts::default(),
                )
                .await
                .unwrap();
            let mut buf = [0u8; 2];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"hi");
        }
    }

    #[tokio::test]
    async fn refused_connection_is_an_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let c = DirectConnector::new(Arc::new(SystemResolve));
        let err = c
            .connect(
                &Target::new(HostName::parse("127.0.0.1"), port),
                &ConnectOpts::default(),
            )
            .await
            .err()
            .expect("refused");
        assert!(!err.to_string().is_empty());
    }

    /// The two calls that take the name a connector's target was resolved
    /// from (`Connector::connect_resolved`) are the plain ones for every
    /// connector but the engine's DNS pipeline: the name is dropped, and the
    /// target dialled as it is (phase 3 / M5 design 9.2).
    #[tokio::test]
    async fn the_resolved_calls_default_to_the_plain_ones() {
        #[derive(Default)]
        struct Plain(Mutex<Vec<String>>);

        fn note(calls: &Mutex<Vec<String>>, call: &str, target: &Target) {
            calls
                .lock()
                .unwrap()
                .push(format!("{call} {}:{}", target.host, target.port));
        }

        fn refused() -> io::Error {
            io::Error::new(io::ErrorKind::ConnectionRefused, "refused")
        }

        impl Connector for Plain {
            fn connect<'a>(
                &'a self,
                target: &'a Target,
                _opts: &'a ConnectOpts,
            ) -> BoxFuture<'a, io::Result<BoxedStream>> {
                note(&self.0, "connect", target);
                Box::pin(std::future::ready(Err(refused())))
            }

            fn connect_udp<'a>(
                &'a self,
                target: &'a Target,
                _opts: &'a ConnectOpts,
            ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
                note(&self.0, "connect_udp", target);
                Box::pin(std::future::ready(Err(refused())))
            }
        }

        let plain = Plain::default();
        let opts = ConnectOpts::default();
        let named = Target::new(HostName::parse("dns.test"), 853);
        let at_ip = Target::new(HostName::Ip(ip("127.0.0.1")), 853);
        assert!(
            plain
                .connect_resolved("dns.test", &named, &opts)
                .await
                .is_err()
        );
        assert!(
            plain
                .connect_resolved("dns.test", &at_ip, &opts)
                .await
                .is_err()
        );
        assert!(
            plain
                .connect_udp_resolved("dns.test", &at_ip, &opts)
                .await
                .is_err()
        );
        assert_eq!(
            *plain.0.lock().unwrap(),
            [
                "connect dns.test:853",
                "connect 127.0.0.1:853",
                "connect_udp 127.0.0.1:853",
            ]
        );
    }

    #[tokio::test]
    async fn resolver_errors_propagate() {
        struct Failing;
        impl Resolve for Failing {
            fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
                Box::pin(async move {
                    Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("nx {host}"),
                    ))
                })
            }
        }
        let c = DirectConnector::new(Arc::new(Failing));
        let err = c
            .connect(
                &Target::new(HostName::parse("nx.test"), 80),
                &ConnectOpts::default(),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    use crate::socket::{Family, SocketHook, SocketOpts};
    use crate::trail::Trail;
    use rurge_config::spec::IpVersion;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingHook {
        calls: Mutex<Vec<String>>,
        refuse_interface: bool,
    }

    impl SocketHook for RecordingHook {
        fn bind_interface(
            &self,
            _: &socket2::Socket,
            interface: &str,
            family: Family,
        ) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("bind {interface} {family:?}"));
            if self.refuse_interface {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no such interface"));
            }
            Ok(())
        }
        fn set_tos(&self, _: &socket2::Socket, family: Family, tos: u8) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("tos {tos:#04x} {family:?}"));
            Ok(())
        }
        fn egress(&self, dst: IpAddr) -> io::Result<Option<String>> {
            self.calls.lock().unwrap().push(format!("egress {dst}"));
            Ok(None)
        }
    }

    /// What `EgressHook` answers to `egress`.
    #[derive(Clone, Default, PartialEq)]
    enum Egress {
        /// No binding.
        #[default]
        None,
        /// Bind this interface.
        Interface(String),
        /// Do not make the connection: this error, as it is.
        Fail(String),
    }

    /// Answers `egress` with what the test set, and records every question and
    /// every binding. An interface it is told to refuse fails binding the way
    /// the macOS and Windows hooks do: naming the interface.
    #[derive(Default)]
    struct EgressHook {
        calls: Mutex<Vec<String>>,
        answer: Mutex<Egress>,
        refuse: Mutex<Vec<String>>,
    }

    impl EgressHook {
        fn answer(&self, answer: Egress) {
            *self.answer.lock().unwrap() = answer;
        }
        fn refuse(&self, interface: &str) {
            self.refuse.lock().unwrap().push(interface.into());
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl SocketHook for EgressHook {
        fn bind_interface(
            &self,
            _: &socket2::Socket,
            interface: &str,
            family: Family,
        ) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("bind {interface} {family:?}"));
            if self.refuse.lock().unwrap().iter().any(|x| x == interface) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no such interface: {interface}"),
                ));
            }
            Ok(())
        }
        fn set_tos(&self, _: &socket2::Socket, family: Family, tos: u8) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("tos {tos:#04x} {family:?}"));
            Ok(())
        }
        fn egress(&self, dst: IpAddr) -> io::Result<Option<String>> {
            self.calls.lock().unwrap().push(format!("egress {dst}"));
            match self.answer.lock().unwrap().clone() {
                Egress::None => Ok(None),
                Egress::Interface(interface) => Ok(Some(interface)),
                Egress::Fail(message) => {
                    Err(io::Error::new(io::ErrorKind::NetworkUnreachable, message))
                }
            }
        }
    }

    async fn listener() -> (tokio::net::TcpListener, u16) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        (l, port)
    }

    #[tokio::test]
    async fn the_hook_sees_the_tos_and_the_interface() {
        let (_l, port) = listener().await;
        let hook = Arc::new(RecordingHook::default());
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("test0".into()),
                tos: 0x10,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        connector
            .connect(
                &Target::new(HostName::parse("127.0.0.1"), port),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            *hook.calls.lock().unwrap(),
            ["tos 0x10 V4", "bind test0 V4"]
        );
    }

    #[tokio::test]
    async fn an_unusable_interface_fails_unless_other_interfaces_are_allowed() {
        let (_l, port) = listener().await;
        let target = Target::new(HostName::parse("127.0.0.1"), port);
        let hook = || {
            Arc::new(RecordingHook {
                refuse_interface: true,
                ..RecordingHook::default()
            })
        };
        let strict = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("gone0".into()),
                ..SocketOpts::default()
            },
            hook(),
        );
        let err = strict
            .connect(&target, &ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "cannot use interface gone0: no such interface"
        );
        let lenient = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("gone0".into()),
                allow_other_interface: true,
                ..SocketOpts::default()
            },
            hook(),
        );
        lenient
            .connect(&target, &ConnectOpts::default())
            .await
            .unwrap();
    }

    /// Without an `interface`, rurge asks the hook where its own connection
    /// has to leave, and binds the socket there (M4 design 3.2).
    #[tokio::test]
    async fn open_socket_asks_egress_for_the_destination_and_binds_there() {
        let (_l, port) = listener().await;
        let hook = Arc::new(EgressHook::default());
        hook.answer(Egress::Interface("test0".into()));
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts::default(),
            hook.clone(),
        );
        connector
            .connect(
                &Target::new(HostName::parse("127.0.0.1"), port),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(hook.calls(), ["egress 127.0.0.1", "bind test0 V4"]);
    }

    /// An egress of `None`: the connection is not bound anywhere.
    #[tokio::test]
    async fn no_egress_no_binding() {
        let (_l, port) = listener().await;
        let hook = Arc::new(EgressHook::default());
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts::default(),
            hook.clone(),
        );
        connector
            .connect(
                &Target::new(HostName::parse("127.0.0.1"), port),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(hook.calls(), ["egress 127.0.0.1"]);
    }

    /// The interface the user named wins: with a policy `interface` the egress
    /// is not asked (M4 plan P2).
    #[tokio::test]
    async fn a_policy_interface_is_used_and_egress_is_not_asked() {
        let (_l, port) = listener().await;
        let hook = Arc::new(EgressHook::default());
        hook.answer(Egress::Interface("test0".into()));
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("policy0".into()),
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        connector
            .connect(
                &Target::new(HostName::parse("127.0.0.1"), port),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(hook.calls(), ["bind policy0 V4"]);
    }

    /// `allow-other-interface` gives up the policy's `interface` but not the
    /// binding: the socket takes the egress the hook answers. Not binding at
    /// all would send it into the virtual adapter (M4 plan P2).
    #[tokio::test]
    async fn allow_other_interface_falls_back_to_egress() {
        let (_l, port) = listener().await;
        let hook = Arc::new(EgressHook::default());
        hook.refuse("gone0");
        hook.answer(Egress::Interface("test0".into()));
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("gone0".into()),
                allow_other_interface: true,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        connector
            .connect(
                &Target::new(HostName::parse("127.0.0.1"), port),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            hook.calls(),
            ["bind gone0 V4", "egress 127.0.0.1", "bind test0 V4"]
        );
    }

    /// An egress of `Err` fails the dial with the hook's error, and nothing
    /// reaches the server — not over TCP, not over UDP.
    #[tokio::test]
    async fn an_egress_error_fails_the_dial_and_nothing_reaches_the_server() {
        let (l, port) = listener().await;
        let hook = Arc::new(EgressHook::default());
        hook.answer(Egress::Fail(
            "no network outside the enhanced mode to reach 127.0.0.1".into(),
        ));
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts::default(),
            hook.clone(),
        );
        let target = Target::new(HostName::parse("127.0.0.1"), port);
        let err = connector
            .connect(&target, &ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "no network outside the enhanced mode to reach 127.0.0.1"
        );
        // a window for a connection that was made after all to be accepted
        let wait = tokio::time::timeout(Duration::from_millis(200), l.accept());
        assert!(wait.await.is_err(), "a connection reached the listener");

        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let target = Target::new(HostName::Ip(addr.ip()), addr.port());
        let err = connector
            .connect_udp(&target, &ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NetworkUnreachable);
        // a window for a datagram that went out anyway to arrive
        let mut buf = [0u8; 8];
        let wait = tokio::time::timeout(Duration::from_millis(200), server.recv_from(&mut buf));
        assert!(wait.await.is_err(), "a datagram reached the server");
    }

    /// What the system said about a failed binding goes no further than DEBUG:
    /// the error of a connection is in request records and the API, where the
    /// hook's interface name does not belong (M4 plan P3).
    #[tokio::test]
    async fn a_binding_that_fails_fails_the_dial_with_no_interface_name_in_the_error() {
        let (_l, port) = listener().await;
        let hook = Arc::new(EgressHook::default());
        hook.answer(Egress::Interface("gone1".into()));
        hook.refuse("gone1");
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts::default(),
            hook.clone(),
        );
        let err = connector
            .connect(
                &Target::new(HostName::parse("127.0.0.1"), port),
                &ConnectOpts::default(),
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(
            err.to_string()
                .starts_with("cannot bind to the network outside the enhanced mode: "),
            "{err}"
        );
        assert!(!err.to_string().contains("gone1"), "{err}");
    }

    #[tokio::test]
    async fn ip_version_filters_resolved_addresses_but_not_literals() {
        let (_l, port) = listener().await;
        let both = || {
            Arc::new(Fixed(vec![
                "::1".parse().unwrap(),
                "127.0.0.1".parse().unwrap(),
            ]))
        };
        let with = |version| {
            DirectConnector::with_opts(
                both(),
                SocketOpts {
                    ip_version: version,
                    ..SocketOpts::default()
                },
                Arc::new(crate::socket::NoopSocketHook),
            )
        };
        let name = Target::new(HostName::parse("both.test"), port);
        with(IpVersion::V4Only)
            .connect(&name, &ConnectOpts::default())
            .await
            .unwrap();
        // only 127.0.0.1 listens, so v6-only cannot connect
        assert!(
            with(IpVersion::V6Only)
                .connect(&name, &ConnectOpts::default())
                .await
                .is_err()
        );
        // an IP literal is used as it is
        let literal = Target::new(HostName::parse("127.0.0.1"), port);
        with(IpVersion::V6Only)
            .connect(&literal, &ConnectOpts::default())
            .await
            .unwrap();
        // nothing left after filtering
        let v4_only_name = DirectConnector::with_opts(
            Arc::new(Fixed(vec!["::1".parse().unwrap()])),
            SocketOpts {
                ip_version: IpVersion::V4Only,
                ..SocketOpts::default()
            },
            Arc::new(crate::socket::NoopSocketHook),
        );
        let err = v4_only_name
            .connect(&name, &ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            err.to_string(),
            "no usable address for both.test: every answer was filtered out by ip-version"
        );
    }

    #[tokio::test]
    async fn the_timeout_covers_name_resolution() {
        struct Stuck;
        impl Resolve for Stuck {
            fn resolve<'a>(&'a self, _: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
                Box::pin(std::future::pending())
            }
        }
        let connector = DirectConnector::new(Arc::new(Stuck));
        let err = connector
            .connect(
                &Target::new(HostName::parse("slow.test"), 80),
                &ConnectOpts {
                    timeout: Duration::from_millis(50),
                },
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(err.to_string(), "connect to slow.test:80 timed out");
        let err = connector
            .connect_udp(
                &Target::new(HostName::parse("slow.test"), 80),
                &ConnectOpts {
                    timeout: Duration::from_millis(50),
                },
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.to_string(), "connect to slow.test:80 timed out");
    }

    #[tokio::test]
    async fn an_empty_answer_and_a_filtered_answer_read_differently() {
        let empty = DirectConnector::new(Arc::new(Fixed(Vec::new())));
        let e = empty
            .connect(
                &Target::new(HostName::parse("empty.test"), 80),
                &ConnectOpts::default(),
            )
            .await
            .err()
            .expect("no address, no connection");
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(e.to_string(), "no addresses for empty.test");

        let v6_only = DirectConnector::with_opts(
            Arc::new(Fixed(vec![ip("127.0.0.1")])),
            SocketOpts {
                ip_version: IpVersion::V6Only,
                ..SocketOpts::default()
            },
            Arc::new(NoopSocketHook),
        );
        let e = v6_only
            .connect(
                &Target::new(HostName::parse("v4.test"), 80),
                &ConnectOpts::default(),
            )
            .await
            .err()
            .expect("the only answer is filtered out");
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            e.to_string(),
            "no usable address for v4.test: every answer was filtered out by ip-version"
        );
    }

    async fn send(datagram: &BoxedDatagram, bytes: &[u8]) {
        std::future::poll_fn(|cx| datagram.poll_send(cx, bytes))
            .await
            .unwrap();
    }

    async fn recv(datagram: &BoxedDatagram) -> Vec<u8> {
        let mut buf = [0u8; 64];
        let n = std::future::poll_fn(|cx| {
            let mut read = ReadBuf::new(&mut buf);
            datagram
                .poll_recv(cx, &mut read)
                .map_ok(|()| read.filled().len())
        })
        .await
        .unwrap();
        buf[..n].to_vec()
    }

    /// Answers every datagram with itself.
    async fn udp_echo() -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            while let Ok((n, from)) = socket.recv_from(&mut buf).await {
                let _ = socket.send_to(&buf[..n], from).await;
            }
        });
        addr
    }

    /// Records that it was woken, so that a receive waiting for a socket that
    /// is added later has to be woken by that: polling it again would hide the
    /// missing wake-up.
    #[derive(Default)]
    struct Woken(AtomicBool);

    impl std::task::Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// Answers every datagram with itself and notes where each came from: a
    /// source port per socket the carrier sends from.
    async fn udp_echo_from() -> (SocketAddr, Arc<Mutex<Vec<SocketAddr>>>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<SocketAddr>>> = Arc::default();
        let log = seen.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            while let Ok((n, from)) = socket.recv_from(&mut buf).await {
                log.lock().unwrap().push(from);
                let _ = socket.send_to(&buf[..n], from).await;
            }
        });
        (addr, seen)
    }

    #[tokio::test]
    async fn a_udp_flow_reaches_its_peer_by_address_and_by_name() {
        let echo = udp_echo().await;
        let c = DirectConnector::new(Arc::new(Fixed(vec![ip("127.0.0.1")])));
        for host in ["127.0.0.1", "echo.test"] {
            let datagram = c
                .connect_udp(
                    &Target::new(HostName::parse(host), echo.port()),
                    &ConnectOpts::default(),
                )
                .await
                .unwrap();
            assert_eq!(datagram.peer_addr(), Some(echo));
            send(&datagram, b"ping").await;
            assert_eq!(recv(&datagram).await, b"ping");
        }
    }

    /// The policy's socket options go on a UDP socket too; `set_tos(0)`
    /// goes back to the policy's own `tos`.
    #[tokio::test]
    async fn the_hook_sees_the_tos_and_the_interface_of_a_udp_flow() {
        let echo = udp_echo().await;
        let hook = Arc::new(RecordingHook::default());
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("test0".into()),
                tos: 0x10,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        let datagram = connector
            .connect_udp(
                &Target::new(HostName::Ip(echo.ip()), echo.port()),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        datagram.set_tos(0x88).unwrap();
        datagram.set_tos(0).unwrap();
        assert_eq!(
            *hook.calls.lock().unwrap(),
            ["tos 0x10 V4", "bind test0 V4", "tos 0x88 V4", "tos 0x10 V4"]
        );
    }

    /// The UDP sockets of DIRECT's carrier and of a direct QUIC connection
    /// have the policy's options and are bound to the unspecified address —
    /// when binding to the interface did not bind them already, as it does
    /// on Windows (`a_carrier_keeps_the_binding_of_its_interface`).
    #[test]
    fn a_udp_socket_has_the_policys_options() {
        let hook = Arc::new(RecordingHook::default());
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("test0".into()),
                tos: 0x10,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        let addr = connector
            .bind_udp(Family::V4, None)
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(addr.ip().is_unspecified() && addr.port() != 0, "{addr}");
        assert_eq!(
            *hook.calls.lock().unwrap(),
            ["tos 0x10 V4", "bind test0 V4"]
        );
    }

    /// A socket made for a destination asks the hook for the egress; one made
    /// without one (DIRECT's carrier opens its own before any destination is
    /// known) asks nothing.
    #[test]
    fn bind_udp_passes_the_destination_to_egress() {
        let hook = Arc::new(EgressHook::default());
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts::default(),
            hook.clone(),
        );
        connector.bind_udp(Family::V4, None).unwrap();
        assert!(hook.calls().is_empty());
        connector
            .bind_udp(Family::V4, Some(ip("127.0.0.1")))
            .unwrap();
        assert_eq!(hook.calls(), ["egress 127.0.0.1"]);
    }

    /// Whatever the system allows of `UDP_BUFFER`: more than it gives a
    /// socket of its own accord.
    #[test]
    fn a_udp_flow_has_room_for_a_burst() {
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None).unwrap();
        let before = socket.recv_buffer_size().unwrap();
        make_room(&socket);
        assert!(
            socket.recv_buffer_size().unwrap() > before,
            "{before} bytes before"
        );
    }

    #[tokio::test]
    async fn ip_version_picks_the_address_of_a_udp_flow() {
        let with = |answer: Vec<IpAddr>| {
            DirectConnector::with_opts(
                Arc::new(Fixed(answer)),
                SocketOpts {
                    ip_version: IpVersion::V4Only,
                    ..SocketOpts::default()
                },
                Arc::new(NoopSocketHook),
            )
        };
        let name = Target::new(HostName::parse("both.test"), 9);
        let datagram = with(vec![ip("::1"), ip("127.0.0.1")])
            .connect_udp(&name, &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(datagram.peer_addr(), Some("127.0.0.1:9".parse().unwrap()));
        let err = with(vec![ip("::1")])
            .connect_udp(&name, &ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "no usable address for both.test: every answer was filtered out by ip-version"
        );
    }

    #[tokio::test]
    async fn a_connector_without_udp_says_so() {
        struct TcpOnly;
        impl Connector for TcpOnly {
            fn connect<'a>(
                &'a self,
                _target: &'a Target,
                _opts: &'a ConnectOpts,
            ) -> BoxFuture<'a, io::Result<BoxedStream>> {
                Box::pin(std::future::ready(Err(io::Error::other("unused"))))
            }
        }
        let err = TcpOnly
            .connect_udp(
                &Target::new(HostName::parse("a.test"), 1),
                &ConnectOpts::default(),
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert_eq!(err.to_string(), "this connection cannot carry UDP");
        let err = TcpOnly
            .open_udp(&ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }

    /// What rurge dials keeps itself alive: a peer gone without a word is
    /// noticed, also on a connection that carries nothing (a SOCKS5 UDP
    /// association's).
    #[tokio::test]
    async fn a_direct_connection_keeps_itself_alive() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = connect_one(
            listener.local_addr().unwrap(),
            &SocketOpts::default(),
            &NoopSocketHook,
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
        assert!(socket2::SockRef::from(&stream).keepalive().unwrap());
    }

    /// Binds the socket to the interface's address, as the Windows hook
    /// does (`rurge-platform::socket::bind_interface`).
    struct BindingHook;

    impl SocketHook for BindingHook {
        fn bind_interface(
            &self,
            socket: &socket2::Socket,
            _: &str,
            family: Family,
        ) -> io::Result<()> {
            let source = match family {
                Family::V4 => ip("127.0.0.1"),
                Family::V6 => ip("::1"),
            };
            socket.bind(&SocketAddr::new(source, 0).into())
        }
        fn set_tos(&self, _: &socket2::Socket, _: Family, _: u8) -> io::Result<()> {
            Ok(())
        }
    }

    /// `interface` on Windows binds the socket to the interface's address:
    /// the carrier takes that binding instead of binding a second time.
    #[tokio::test]
    async fn a_carrier_keeps_the_binding_of_its_interface() {
        let echo = udp_echo().await;
        let opts = SocketOpts {
            interface: Some("lo".into()),
            ..SocketOpts::default()
        };
        let c = DirectConnector::with_opts(Arc::new(SystemResolve), opts, Arc::new(BindingHook));
        let carrier = c.open_udp(&ConnectOpts::default()).await.unwrap();
        let to = Target::new(HostName::Ip(echo.ip()), echo.port());
        carrier.send_to(b"bound", &to).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("the echo comes back")
            .unwrap();
        assert_eq!((&buf[..n], from), (&b"bound"[..], to));
    }

    /// DIRECT's carrier sends to any address, by address and by name, and
    /// hears from any address — not only from the one it sent to (full cone).
    #[tokio::test]
    async fn a_direct_carrier_talks_to_any_address() {
        let echo = udp_echo().await;
        let c = DirectConnector::new(Arc::new(Fixed(vec![ip("127.0.0.1")])));
        let carrier = c.open_udp(&ConnectOpts::default()).await.unwrap();
        let by_name = Target::new(HostName::parse("echo.test"), echo.port());
        let resolved = carrier.resolve(&by_name).await.unwrap();
        assert_eq!(resolved, Target::new(HostName::Ip(echo.ip()), echo.port()));
        let mut buf = [0u8; 64];
        let within = Duration::from_secs(5);
        for to in [&by_name, &resolved] {
            carrier.send_to(b"ping", to).await.unwrap();
            let (n, from) = tokio::time::timeout(within, carrier.recv_from(&mut buf))
                .await
                .expect("the echo comes back")
                .unwrap();
            assert_eq!((&buf[..n], &from), (&b"ping"[..], &resolved));
        }
        // a stranger writes to the carrier's port, never having been written to
        let seen = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let seen_addr = seen.local_addr().unwrap();
        carrier
            .send_to(
                b"who",
                &Target::new(HostName::Ip(seen_addr.ip()), seen_addr.port()),
            )
            .await
            .unwrap();
        let (n, carrier_addr) = tokio::time::timeout(within, seen.recv_from(&mut buf))
            .await
            .expect("the datagram arrives")
            .unwrap();
        assert_eq!(&buf[..n], b"who");
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let stranger_addr = stranger.local_addr().unwrap();
        stranger.send_to(b"hello", carrier_addr).await.unwrap();
        let (n, from) = tokio::time::timeout(within, carrier.recv_from(&mut buf))
            .await
            .expect("the stranger is heard")
            .unwrap();
        assert_eq!(&buf[..n], b"hello");
        assert_eq!(
            from,
            Target::new(HostName::Ip(stranger_addr.ip()), stranger_addr.port())
        );
    }

    /// The carrier asks the egress once for every datagram and keeps a socket
    /// per egress: as if the enhanced mode came on and the network changed
    /// between two datagrams (M4 plan P4). A receive that was already waiting
    /// hears the answer that comes on a socket made after it started waiting.
    #[tokio::test]
    async fn direct_packet_opens_a_socket_for_each_egress_and_hears_on_all() {
        let (echo, seen) = udp_echo_from().await;
        let hook = Arc::new(EgressHook::default());
        let c = DirectConnector::with_opts(
            Arc::new(Fixed(vec![ip("127.0.0.1")])),
            SocketOpts::default(),
            hook.clone(),
        );
        let carrier = c.open_udp(&ConnectOpts::default()).await.unwrap();
        let to = Target::new(HostName::Ip(echo.ip()), echo.port());
        let mut buf = [0u8; 64];

        hook.answer(Egress::Interface("a".into()));
        carrier.send_to(b"one", &to).await.unwrap();
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("the echo of the first datagram")
            .unwrap();
        assert_eq!((&buf[..n], from), (&b"one"[..], to.clone()));

        // the network changed: the next datagram has another egress
        hook.answer(Egress::Interface("b".into()));
        let mut other = [0u8; 64];
        let mut waiting = carrier.recv_from(&mut other);
        let woken = Arc::new(Woken::default());
        {
            let waker = Waker::from(woken.clone());
            let mut cx = Context::from_waker(&waker);
            assert!(
                std::pin::Pin::new(&mut waiting).poll(&mut cx).is_pending(),
                "nothing has come yet"
            );
        }
        carrier.send_to(b"two", &to).await.unwrap();
        assert!(
            woken.0.load(Ordering::SeqCst),
            "adding a socket wakes the waiting receive"
        );
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), &mut waiting)
            .await
            .expect("the echo on the socket made after the receive started")
            .unwrap();
        drop(waiting);
        assert_eq!((&other[..n], from), (&b"two"[..], to.clone()));

        assert_eq!(
            hook.calls(),
            [
                "egress 127.0.0.1",
                "bind a V4",
                "egress 127.0.0.1",
                "bind b V4"
            ]
        );
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_ne!(
            seen[0].port(),
            seen[1].port(),
            "a socket per egress: {seen:?}"
        );
    }

    /// An egress of `Err` fails the datagram — it does not go out unbound into
    /// the virtual adapter (M4 plan P2).
    #[tokio::test]
    async fn direct_packet_fails_a_datagram_whose_egress_is_an_error() {
        let (echo, seen) = udp_echo_from().await;
        let hook = Arc::new(EgressHook::default());
        hook.answer(Egress::Fail(
            "no network outside the enhanced mode to reach 127.0.0.1".into(),
        ));
        let c = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts::default(),
            hook.clone(),
        );
        let carrier = c.open_udp(&ConnectOpts::default()).await.unwrap();
        let to = Target::new(HostName::Ip(echo.ip()), echo.port());
        let err = carrier.send_to(b"lost", &to).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "no network outside the enhanced mode to reach 127.0.0.1"
        );
        // a window for a datagram that went out anyway to arrive
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(seen.lock().unwrap().is_empty(), "nothing went out");
    }

    /// The policy's `interface` bound the carrier's sockets: its datagrams ask
    /// no egress and go out from the one socket the carrier opened (M4 plan
    /// P2).
    #[tokio::test]
    async fn direct_packet_keeps_the_policy_interface_and_does_not_ask_egress() {
        let (echo, seen) = udp_echo_from().await;
        let hook = Arc::new(EgressHook::default());
        hook.answer(Egress::Interface("test0".into()));
        let c = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("policy0".into()),
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        let carrier = c.open_udp(&ConnectOpts::default()).await.unwrap();
        let to = Target::new(HostName::Ip(echo.ip()), echo.port());
        let mut buf = [0u8; 64];
        for _ in 0..2 {
            carrier.send_to(b"ping", &to).await.unwrap();
            let (n, from) =
                tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
                    .await
                    .expect("the echo comes back")
                    .unwrap();
            assert_eq!((&buf[..n], from), (&b"ping"[..], to.clone()));
        }
        assert!(
            !hook.calls().iter().any(|c| c.starts_with("egress")),
            "{:?}",
            hook.calls()
        );
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[0], seen[1], "one socket carried both datagrams");
    }

    /// The policy's `interface` cannot be bound and `allow-other-interface`
    /// takes the failure: every datagram asks the egress and goes out bound to
    /// the answer (M4 plan P2).
    #[tokio::test]
    async fn direct_packet_asks_egress_when_the_policy_interface_falls_back() {
        let (echo, seen) = udp_echo_from().await;
        let hook = Arc::new(EgressHook::default());
        hook.refuse("gone0");
        hook.answer(Egress::Interface("test0".into()));
        let c = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("gone0".into()),
                allow_other_interface: true,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        let carrier = c.open_udp(&ConnectOpts::default()).await.unwrap();
        let to = Target::new(HostName::Ip(echo.ip()), echo.port());
        let mut buf = [0u8; 64];
        for _ in 0..2 {
            carrier.send_to(b"ping", &to).await.unwrap();
            let (n, from) =
                tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
                    .await
                    .expect("the echo comes back")
                    .unwrap();
            assert_eq!((&buf[..n], from), (&b"ping"[..], to.clone()));
        }
        let calls = hook.calls();
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.as_str() == "egress 127.0.0.1")
                .count(),
            2,
            "{calls:?}"
        );
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.as_str() == "bind test0 V4")
                .count(),
            1,
            "one egress socket carried both datagrams: {calls:?}"
        );
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[0], seen[1], "one socket carried both datagrams");
    }

    /// A datagram to a port nobody listens on does not stop the carrier:
    /// Windows reports the ICMP "port unreachable" on the next receive.
    #[tokio::test]
    async fn a_closed_port_does_not_stop_a_direct_carrier() {
        let echo = udp_echo().await;
        // the socket goes with the statement: nobody listens on its port
        let closed = UdpSocket::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let c = DirectConnector::new(Arc::new(SystemResolve));
        let carrier = c.open_udp(&ConnectOpts::default()).await.unwrap();
        let to = |addr: SocketAddr| Target::new(HostName::Ip(addr.ip()), addr.port());
        carrier.send_to(b"lost", &to(closed)).await.unwrap();
        // a window for the unreachable answer to arrive
        tokio::time::sleep(Duration::from_millis(200)).await;
        carrier.send_to(b"ping", &to(echo)).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("the echo comes back")
            .unwrap();
        assert_eq!((&buf[..n], from), (&b"ping"[..], to(echo)));
    }

    /// `ip-version` decides which families the carrier sends to.
    #[tokio::test]
    async fn ip_version_limits_a_direct_carrier() {
        let c = DirectConnector::with_opts(
            Arc::new(Fixed(vec![ip("::1"), ip("127.0.0.1")])),
            SocketOpts {
                ip_version: IpVersion::V4Only,
                ..SocketOpts::default()
            },
            Arc::new(NoopSocketHook),
        );
        let carrier = c.open_udp(&ConnectOpts::default()).await.unwrap();
        assert_eq!(
            carrier
                .resolve(&Target::new(HostName::parse("both.test"), 53))
                .await
                .unwrap(),
            Target::new(HostName::parse("127.0.0.1"), 53)
        );
        let err = carrier
            .send_to(b"x", &Target::new(HostName::parse("::1"), 53))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrNotAvailable);
        assert_eq!(err.to_string(), "this policy sends no UDP to [::1]:53");
    }

    /// The policy's socket options go on each socket of the carrier.
    #[tokio::test]
    async fn the_hook_sees_every_socket_of_a_direct_carrier() {
        let hook = Arc::new(RecordingHook::default());
        let c = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("test0".into()),
                tos: 0x10,
                ip_version: IpVersion::V4Only,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        c.open_udp(&ConnectOpts::default()).await.unwrap();
        assert_eq!(
            *hook.calls.lock().unwrap(),
            ["tos 0x10 V4", "bind test0 V4"]
        );
    }

    /// The connection a direct dial makes is noted on the trail of whoever
    /// dials, at the address that answered: the winner of the race, not the
    /// first one tried.
    #[tokio::test]
    async fn a_connection_is_noted_at_the_address_that_answered() {
        let (l, port) = listener().await;
        let connector = DirectConnector::new(Arc::new(Fixed(vec![ip("::1"), ip("127.0.0.1")])));
        let target = Target::new(HostName::parse("both.test"), port);
        let trail = Trail::new();
        let _stream = trail
            .follow(connector.connect(&target, &ConnectOpts::default()))
            .await
            .unwrap();
        assert_eq!(trail.peer(), Some(l.local_addr().unwrap()));
    }

    /// A dial that makes no connection notes none; nor does a UDP flow, which
    /// has no connection to note.
    #[tokio::test]
    async fn a_dial_that_connects_nowhere_notes_no_address() {
        let (_l, port) = listener().await;
        let hook = Arc::new(RecordingHook {
            refuse_interface: true,
            ..RecordingHook::default()
        });
        let refusing = DirectConnector::with_opts(
            Arc::new(Fixed(vec![ip("127.0.0.1")])),
            SocketOpts {
                interface: Some("gone0".into()),
                ..SocketOpts::default()
            },
            hook,
        );
        let target = Target::new(HostName::parse("127.0.0.1"), port);
        let trail = Trail::new();
        let err = trail
            .follow(refusing.connect(&target, &ConnectOpts::default()))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "cannot use interface gone0: no such interface"
        );
        assert_eq!(trail.peer(), None);

        let plain = DirectConnector::new(Arc::new(Fixed(vec![ip("127.0.0.1")])));
        trail
            .follow(plain.connect_udp(&target, &ConnectOpts::default()))
            .await
            .unwrap();
        assert_eq!(trail.peer(), None);
    }

    #[test]
    fn targets_are_displayed_the_way_they_are_dialled() {
        assert_eq!(
            display_target(&Target::new(HostName::parse("::1"), 80)),
            "[::1]:80"
        );
        assert_eq!(
            display_target(&Target::new(HostName::parse("192.0.2.1"), 80)),
            "192.0.2.1:80"
        );
        assert_eq!(
            display_target(&Target::new(HostName::parse("a.test"), 443)),
            "a.test:443"
        );
    }

    /// A route built from an egress table is the one kind that can say what
    /// rurge's own sockets are bound to; equality is still the table it stands
    /// for, not how the route was built (the phase 3 / M4b plan's execution
    /// record row 68).
    #[test]
    fn only_a_route_built_from_an_egress_table_says_it_is_one() {
        let table = Arc::new(7u8);
        assert!(Route::egress(&table).is_egress());
        assert!(!Route::of(&table).is_egress());
        assert_eq!(Route::egress(&table), Route::egress(&table));
        assert_eq!(
            Route::egress(&table),
            Route::of(&table),
            "the same table, however the route was built"
        );
        assert_ne!(Route::egress(&table), Route::egress(&Arc::new(7u8)));
    }

    /// A hook whose table changes under it, as the manager's does: the answer
    /// to `egress_route` is the table there is now.
    #[derive(Default)]
    struct EgressRouting {
        table: Mutex<Option<Arc<u8>>>,
    }

    impl SocketHook for EgressRouting {
        fn bind_interface(&self, _: &socket2::Socket, _: &str, _: Family) -> io::Result<()> {
            Ok(())
        }
        fn set_tos(&self, _: &socket2::Socket, _: Family, _: u8) -> io::Result<()> {
            Ok(())
        }
        fn egress_route(&self) -> Option<Route> {
            self.table.lock().unwrap().as_ref().map(Route::egress)
        }
    }

    /// A direct connector routes to the egress table its hook names: `None`
    /// while there is none, one route while it is the same table, and another
    /// once the hook names another table. A connector without a hook says
    /// nothing, so nothing is rebuilt for it (the phase 3 / M4b plan's
    /// execution record row 68).
    #[test]
    fn a_direct_connector_routes_to_the_egress_table_its_hook_names() {
        let hook = Arc::new(EgressRouting::default());
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts::default(),
            hook.clone(),
        );
        assert!(connector.route().is_none(), "no table, no route");

        let first = Arc::new(1u8);
        *hook.table.lock().unwrap() = Some(first.clone());
        let route = connector.route().expect("the table is the route");
        assert!(route.is_egress());
        assert_eq!(Some(route), connector.route(), "the same table twice");

        *hook.table.lock().unwrap() = Some(Arc::new(2u8));
        assert_ne!(
            connector.route(),
            Some(Route::egress(&first)),
            "another table is another route"
        );

        let plain = DirectConnector::new(Arc::new(SystemResolve));
        assert!(plain.route().is_none(), "a hook that says nothing");
    }
}
