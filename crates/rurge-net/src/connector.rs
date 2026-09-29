//! Connection establishment (M2 design §5.1). M2 ships `DirectConnector`; M3
//! injects a connector that runs the session pipeline behind the same trait.

use crate::BoxFuture;
use crate::socket::{Family, NoopSocketHook, SocketHook, SocketOpts, plan_addresses, race};
use rurge_config::HostName;
use rurge_config::spec::IpVersion;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
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
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>>;
}

pub type BoxedPacketSocket = Box<dyn PacketSocket>;

#[derive(Clone, Debug, PartialEq, Eq)]
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
}

fn no_udp() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "this connection cannot carry UDP",
    )
}

pub trait Resolve: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>>;
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

/// A non-blocking socket of `addr`'s family with the policy's socket
/// options on it.
fn open_socket(
    addr: SocketAddr,
    kind: socket2::Type,
    opts: &SocketOpts,
    hook: &dyn SocketHook,
    fallback_logged: &AtomicBool,
) -> io::Result<socket2::Socket> {
    let family = Family::of(&addr.ip());
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
    if opts.tos != 0
        && let Err(e) = hook.set_tos(&socket, family, opts.tos)
    {
        tracing::debug!(error = %e, tos = opts.tos, "cannot set the IP TOS; connecting without it");
    }
    if let Some(interface) = &opts.interface
        && let Err(e) = hook.bind_interface(&socket, interface, family)
    {
        if !opts.allow_other_interface {
            return Err(io::Error::new(
                e.kind(),
                format!("cannot use interface {interface}: {e}"),
            ));
        }
        if !fallback_logged.swap(true, Ordering::Relaxed) {
            tracing::warn!(interface = %interface, error = %e, "interface unavailable; using the default one (allow-other-interface)");
        }
    }
    Ok(socket)
}

async fn connect_one(
    addr: SocketAddr,
    opts: &SocketOpts,
    hook: &dyn SocketHook,
    fallback_logged: &AtomicBool,
) -> io::Result<TcpStream> {
    let socket = open_socket(addr, socket2::Type::STREAM, opts, hook, fallback_logged)?;
    let std_stream: std::net::TcpStream = socket.into();
    let stream = tokio::net::TcpSocket::from_std_stream(std_stream)
        .connect(addr)
        .await?;
    let _ = stream.set_nodelay(true);
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
            Ok(Box::new(stream) as BoxedStream)
        }))
    }

    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        Box::pin(within(target, opts.timeout, async move {
            // nothing answers a UDP "connection": the first address it is
            let (primary, _) = self.plan(target).await?;
            let addr = SocketAddr::new(primary[0], target.port);
            let socket = open_socket(
                addr,
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
}

impl DirectConnector {
    /// One unconnected UDP socket per address family the policy's
    /// `ip-version` allows, each bound to the unspecified address and
    /// carrying the policy's socket options. A family this machine cannot
    /// open a socket of is left out, unless it is the only one.
    fn open_packet(&self) -> io::Result<DirectPacket> {
        let families: &[Family] = match self.opts.ip_version {
            IpVersion::V4Only => &[Family::V4],
            IpVersion::V6Only => &[Family::V6],
            _ => &[Family::V4, Family::V6],
        };
        let (mut v4, mut v6, mut failure) = (None, None, None);
        for family in families {
            let unspecified = match family {
                Family::V4 => IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                Family::V6 => IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
            };
            let opened = open_socket(
                SocketAddr::new(unspecified, 0),
                socket2::Type::DGRAM,
                &self.opts,
                self.hook.as_ref(),
                &self.fallback_logged,
            )
            .and_then(|socket| {
                if *family == Family::V6 {
                    socket.set_only_v6(true)?;
                }
                make_room(&socket);
                socket.bind(&SocketAddr::new(unspecified, 0).into())?;
                UdpSocket::from_std(socket.into())
            });
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
            v4,
            v6,
            resolver: self.resolver.clone(),
            opts: self.opts.clone(),
        })
    }
}

/// DIRECT's UDP carrier: unconnected sockets, one per address family.
struct DirectPacket {
    v4: Option<UdpSocket>,
    v6: Option<UdpSocket>,
    resolver: Arc<dyn Resolve>,
    opts: SocketOpts,
}

impl DirectPacket {
    fn socket_for(&self, ip: &IpAddr) -> Option<&UdpSocket> {
        match Family::of(ip) {
            Family::V4 => self.v4.as_ref(),
            Family::V6 => self.v6.as_ref(),
        }
    }

    /// The first address of `name` the policy's `ip-version` allows and
    /// this carrier has a socket for.
    async fn address_of(&self, name: &str) -> io::Result<IpAddr> {
        let addrs = self.resolver.resolve(name).await?;
        let (primary, secondary) = plan_addresses(addrs, self.opts.ip_version, self.opts.v6_first);
        primary
            .into_iter()
            .chain(secondary)
            .find(|ip| self.socket_for(ip).is_some())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no usable address for {name}"),
                )
            })
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
            let socket = self.socket_for(&ip).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    format!("this policy sends no UDP to {}", display_target(to)),
                )
            })?;
            socket.send_to(buf, SocketAddr::new(ip, to.port)).await?;
            Ok(())
        })
    }

    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(std::future::poll_fn(move |cx| {
            let mut read = ReadBuf::new(buf);
            for socket in [&self.v4, &self.v6].into_iter().flatten() {
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
            Poll::Pending
        }))
    }
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
        for to in [&by_name, &resolved] {
            carrier.send_to(b"ping", to).await.unwrap();
            let (n, from) = carrier.recv_from(&mut buf).await.unwrap();
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
        let (n, carrier_addr) = seen.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"who");
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let stranger_addr = stranger.local_addr().unwrap();
        stranger.send_to(b"hello", carrier_addr).await.unwrap();
        let (n, from) = carrier.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"hello");
        assert_eq!(
            from,
            Target::new(HostName::Ip(stranger_addr.ip()), stranger_addr.port())
        );
    }

    /// A datagram to a port nobody listens on does not stop the carrier:
    /// Windows reports the ICMP "port unreachable" on the next receive.
    #[tokio::test]
    async fn a_closed_port_does_not_stop_a_direct_carrier() {
        let echo = udp_echo().await;
        let closed = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let closed = closed.local_addr().unwrap();
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
}
