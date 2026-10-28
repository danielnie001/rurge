//! QUIC for what runs over it (phase 2 M7 design 3): the UDP layers under
//! a connection, all quinn `AsyncUdpSocket`s, and the dial.
//!
//! - Direct: a socket of the policy's own (its interface, `ip-version`,
//!   buffers) that quinn-udp drives — segmentation offload, per-packet ECN.
//!   With `tos`, one that sends plainly instead, which keeps the DSCP.
//! - `underlying-proxy`: the chain's UDP flow to the server, named as the
//!   line names it for the underlying proxy to resolve.
//! - `port-hopping`: a port of the set at a time, a random other one and a
//!   new local socket every interval.
//! - `ecn=off`: no ECN marks either way.
//!
//! Every connection has an endpoint of its own on such a socket (M7-D9):
//! its hops, and the obfuscation a protocol may layer on it, belong to one
//! connection.

mod carrier;
mod dial;
pub mod h3;
mod hop;
pub mod initial;
mod socket;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use dial::{Transport, client_config, connect, connection_error};

use crate::connector::{ConnectOpts, Connector, Target};
use quinn::AsyncUdpSocket;
use rurge_config::spec::{PortHopping, Tristate};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

/// What a QUIC connection's packets go through to its server, and the
/// address quinn knows the server by — its own, or one that stands for it.
#[derive(Debug)]
pub struct Path {
    pub socket: Arc<dyn AsyncUdpSocket>,
    pub remote: SocketAddr,
    /// The carrier under `socket`, when the path goes over one: should it
    /// fail, it says why (`connect`).
    carrier: Option<Arc<carrier::Carried>>,
}

impl Path {
    /// The path to `server` through `connector`. A direct connector
    /// resolves the server the policy's way (`[Host]`, `dns-follow-
    /// interface`, `ip-version`), once — `port-hopping` keeps the address
    /// and replaces the line's port with its own. Any other connector
    /// carries the packets over its UDP flow to `server` as named, which
    /// cannot hop. `ecn=off` strips ECN marks; `auto` and `on` are quinn's
    /// own (design 3.4). `opts` bounds the carrier's setup only: the caller
    /// bounds the whole dial.
    pub async fn open(
        connector: &dyn Connector,
        server: &Target,
        hopping: Option<&PortHopping>,
        ecn: Tristate,
        opts: &ConnectOpts,
    ) -> io::Result<Path> {
        Path::open_over(connector, None, server, hopping, ecn, opts).await
    }

    /// `open` for a server whose address `server` the name `name` resolved
    /// to (phase 3 / M5 design 9.2, the plan's P15): over a connector that is
    /// not direct, the carrier is asked of `connect_udp_resolved`, so it
    /// learns the name of the server it carries to. A direct connector sends
    /// from sockets of its own to `server` (`DirectConnector::udp_peer`), to
    /// an address already, so the name is not used.
    pub async fn open_resolved(
        connector: &dyn Connector,
        name: &str,
        server: &Target,
        opts: &ConnectOpts,
    ) -> io::Result<Path> {
        Path::open_over(connector, Some(name), server, None, Tristate::Auto, opts).await
    }

    /// `open` and `open_resolved`: the one way a path is opened, `name` being
    /// the name `server` was resolved from, when there was one.
    async fn open_over(
        connector: &dyn Connector,
        name: Option<&str>,
        server: &Target,
        hopping: Option<&PortHopping>,
        ecn: Tristate,
        opts: &ConnectOpts,
    ) -> io::Result<Path> {
        let path = match (connector.as_direct(), hopping) {
            (Some(direct), None) => {
                let remote = direct.udp_peer(server).await?;
                Path {
                    socket: socket::direct_socket(direct, remote)?,
                    remote,
                    carrier: None,
                }
            }
            (Some(direct), Some(hopping)) => {
                let server = direct.udp_peer(server).await?.ip();
                let direct = direct.clone();
                // the egress is the server's, which only the port hops
                let open = move || socket::direct_socket(&direct, SocketAddr::new(server, 0));
                let hopping =
                    hop::Hopping::start(server, &hopping.ports, hopping.interval, Box::new(open))?;
                Path {
                    remote: hopping.remote(),
                    socket: Arc::new(hopping),
                    carrier: None,
                }
            }
            (None, None) => {
                let carrier = match name {
                    Some(name) => connector.connect_udp_resolved(name, server, opts).await?,
                    None => connector.connect_udp(server, opts).await?,
                };
                let carried = Arc::new(carrier::Carried::new(carrier));
                Path {
                    socket: carried.clone(),
                    remote: carrier::STAND_IN,
                    carrier: Some(carried),
                }
            }
            (None, Some(_)) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "quic: `port-hopping` cannot be combined with `underlying-proxy`",
                ));
            }
        };
        Ok(match ecn {
            Tristate::Off => Path {
                socket: Arc::new(socket::NoEcn(path.socket)),
                ..path
            },
            Tristate::Auto | Tristate::On => path,
        })
    }

    /// Whether the path goes over a carrier — the UDP flow of an
    /// `underlying-proxy` — rather than sockets of the connection's own: a
    /// connection on it takes its transport as `Transport::apply_carried`
    /// sets it.
    pub fn is_carried(&self) -> bool {
        self.carrier.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{EchoServer, MultiPort, TestCert, endpoint, loopback_socket};
    use super::*;
    use crate::BoxFuture;
    use crate::connector::{
        BoxedDatagram, BoxedStream, Datagram, DirectConnector, PacketSocket, Resolve,
    };
    use crate::packet_datagram::packet_datagram;
    use crate::socket::{Family, SocketHook, SocketOpts};
    use bytes::Bytes;
    use quinn::udp::{RecvMeta, Transmit};
    use quinn::{Connection, ConnectionError, Runtime, TokioRuntime, UdpPoller};
    use rurge_config::HostName;
    use std::io::IoSliceMut;
    use std::net::IpAddr;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker, ready};
    use std::time::Duration;
    use tokio::io::ReadBuf;
    use tokio::time::{Instant, timeout};

    const ALPN: &[&str] = &["h3"];
    const BOUND: Duration = Duration::from_secs(10);

    /// Answers every name with the loopback.
    struct Loopback;

    impl Resolve for Loopback {
        fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(std::future::ready(Ok(vec![IpAddr::from([127, 0, 0, 1])])))
        }
    }

    fn direct() -> DirectConnector {
        DirectConnector::new(Arc::new(Loopback))
    }

    fn at(host: &str, port: u16) -> Target {
        Target::new(HostName::parse(host), port)
    }

    async fn open(
        connector: &dyn Connector,
        server: &Target,
        hopping: Option<&PortHopping>,
        ecn: Tristate,
    ) -> Path {
        Path::open(connector, server, hopping, ecn, &ConnectOpts::default())
            .await
            .expect("a path")
    }

    async fn dial(path: Path, cert: &TestCert, transport: &Transport) -> io::Result<Connection> {
        let config = client_config(cert.client_tls(ALPN), transport)?;
        timeout(BOUND, connect(path, &config, "localhost"))
            .await
            .expect("an outcome within the bound")
    }

    /// A stream and a datagram there and back.
    async fn echoes(connection: &Connection) {
        timeout(BOUND, async {
            let (mut send, mut recv) = connection.open_bi().await.unwrap();
            send.write_all(b"ping").await.unwrap();
            send.finish().unwrap();
            assert_eq!(recv.read_to_end(64).await.unwrap(), b"ping");
            // a datagram may be lost: sent again until one comes back
            loop {
                connection
                    .send_datagram(Bytes::from_static(b"datagram"))
                    .unwrap();
                let wait = timeout(Duration::from_millis(200), connection.read_datagram());
                if let Ok(back) = wait.await {
                    assert_eq!(&back.unwrap()[..], b"datagram");
                    return;
                }
            }
        })
        .await
        .expect("the echoes within the bound");
    }

    #[tokio::test]
    async fn a_direct_path_carries_streams_and_datagrams() {
        let cert = TestCert::new();
        let server = EchoServer::spawn(cert.server_config(ALPN));
        let port = server.addr().port();
        // by address, and by a name the policy's own resolver answers
        for host in ["127.0.0.1", "quic.test"] {
            let path = open(&direct(), &at(host, port), None, Tristate::Auto).await;
            assert_eq!(path.remote, server.addr());
            let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
            echoes(&connection).await;
        }
    }

    /// Notes the `tos` the sockets get.
    #[derive(Default)]
    struct TosHook(Mutex<Vec<u8>>);

    impl SocketHook for TosHook {
        fn bind_interface(&self, _: &socket2::Socket, _: &str, _: Family) -> io::Result<()> {
            Ok(())
        }

        fn set_tos(&self, _: &socket2::Socket, _: Family, tos: u8) -> io::Result<()> {
            self.0.lock().unwrap().push(tos);
            Ok(())
        }
    }

    /// With `tos` the socket keeps it and sends one datagram at a time, and
    /// still finds the path MTU as quinn's own sockets do.
    #[tokio::test]
    async fn a_path_with_tos_carries_everything() {
        let cert = TestCert::new();
        let server = EchoServer::spawn(cert.server_config(ALPN));
        let hook = Arc::new(TosHook::default());
        let connector = DirectConnector::with_opts(
            Arc::new(Loopback),
            SocketOpts {
                tos: 0x88,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        let path = open(
            &connector,
            &at("127.0.0.1", server.addr().port()),
            None,
            Tristate::Auto,
        )
        .await;
        assert_eq!(path.socket.max_transmit_segments(), 1);
        assert_eq!(path.socket.may_fragment(), loopback_socket().may_fragment());
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        echoes(&connection).await;
        assert_eq!(*hook.0.lock().unwrap(), [0x88]);
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
                Family::V4 => IpAddr::from([127, 0, 0, 1]),
                Family::V6 => IpAddr::from(std::net::Ipv6Addr::LOCALHOST),
            };
            socket.bind(&SocketAddr::new(source, 0).into())
        }

        fn set_tos(&self, _: &socket2::Socket, _: Family, _: u8) -> io::Result<()> {
            Ok(())
        }
    }

    /// `interface` on Windows binds the socket to the interface's address:
    /// the connection sends from that binding, which is not made a second
    /// time — and an IPv6 socket was made IPv6-only before it, which a
    /// bound socket takes no more. IPv6 where this machine has a loopback
    /// of it.
    #[tokio::test]
    async fn a_direct_path_keeps_the_binding_of_its_interface() {
        let cert = TestCert::new();
        let connector = DirectConnector::with_opts(
            Arc::new(Loopback),
            SocketOpts {
                interface: Some("lo".into()),
                ..SocketOpts::default()
            },
            Arc::new(BindingHook),
        );
        let mut servers = vec![std::net::UdpSocket::bind("127.0.0.1:0").unwrap()];
        servers.extend(std::net::UdpSocket::bind("[::1]:0").ok());
        for socket in servers {
            let addr = socket.local_addr().unwrap();
            let socket = TokioRuntime.wrap_udp_socket(socket).unwrap();
            let _server = EchoServer::spawn_on(cert.server_config(ALPN), socket);
            let server = Target::new(HostName::Ip(addr.ip()), addr.port());
            let path = open(&connector, &server, None, Tristate::Auto).await;
            let local = path.socket.local_addr().unwrap();
            assert!(local.ip() == addr.ip() && local.port() != 0, "{local}");
            let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
            echoes(&connection).await;
        }
    }

    /// Notes what it is asked: the egress per destination, and every binding.
    #[derive(Default)]
    struct EgressHook(Mutex<Vec<String>>);

    impl SocketHook for EgressHook {
        fn bind_interface(
            &self,
            _: &socket2::Socket,
            interface: &str,
            family: Family,
        ) -> io::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(format!("bind {interface} {family:?}"));
            Ok(())
        }

        fn set_tos(&self, _: &socket2::Socket, _: Family, _: u8) -> io::Result<()> {
            Ok(())
        }

        fn egress(&self, dst: IpAddr) -> io::Result<Option<String>> {
            self.0.lock().unwrap().push(format!("egress {dst}"));
            Ok(Some("test0".into()))
        }
    }

    /// A direct QUIC connection's socket is bound to the egress of its server,
    /// asked for the server's address before the socket is made (M4 design
    /// 3.2).
    #[tokio::test]
    async fn quic_direct_socket_asks_egress_for_the_server() {
        let cert = TestCert::new();
        let server = EchoServer::spawn(cert.server_config(ALPN));
        let hook = Arc::new(EgressHook::default());
        let connector =
            DirectConnector::with_opts(Arc::new(Loopback), SocketOpts::default(), hook.clone());
        let path = open(
            &connector,
            &at("127.0.0.1", server.addr().port()),
            None,
            Tristate::Auto,
        )
        .await;
        assert_eq!(
            *hook.0.lock().unwrap(),
            ["egress 127.0.0.1", "bind test0 V4"]
        );
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        echoes(&connection).await;
    }

    #[tokio::test]
    async fn a_path_without_ecn_carries_everything() {
        let cert = TestCert::new();
        let server = EchoServer::spawn(cert.server_config(ALPN));
        let path = open(
            &direct(),
            &at("127.0.0.1", server.addr().port()),
            None,
            Tristate::Off,
        )
        .await;
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        echoes(&connection).await;
    }

    /// Carries a direct connector's UDP the way `ChainConnector` carries
    /// the underlying policy's: the carrier is asked for the server as
    /// named and resolves it itself. A carrier can be cut.
    #[derive(Default)]
    struct Chained {
        asked: Mutex<Vec<Target>>,
        cut: Arc<Cut>,
    }

    #[derive(Default)]
    struct Cut {
        done: AtomicBool,
        reader: Mutex<Option<Waker>>,
    }

    impl Cut {
        fn now(&self) {
            self.done.store(true, Ordering::SeqCst);
            if let Some(reader) = self.reader.lock().unwrap().take() {
                reader.wake();
            }
        }
    }

    struct CutDatagram(BoxedDatagram, Arc<Cut>);

    impl Datagram for CutDatagram {
        fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            self.0.poll_send(cx, buf)
        }

        fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            *self.1.reader.lock().unwrap() = Some(cx.waker().clone());
            if self.1.done.load(Ordering::SeqCst) {
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "cut")));
            }
            self.0.poll_recv(cx, buf)
        }
    }

    impl Connector for Chained {
        fn connect<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            Box::pin(std::future::ready(Err(io::Error::other("UDP only"))))
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            self.asked.lock().unwrap().push(target.clone());
            Box::pin(async move {
                let socket = direct().open_udp(opts).await?;
                let to = socket.resolve(target).await?;
                let datagram = packet_datagram(socket, to);
                Ok(Box::new(CutDatagram(datagram, self.cut.clone())) as BoxedDatagram)
            })
        }
    }

    /// Through `underlying-proxy`'s carrier: the server goes to it as
    /// named, quinn knows a stand-in, and no path MTU is probed.
    #[tokio::test]
    async fn a_carried_path_reaches_the_server_by_name() {
        let cert = TestCert::new();
        let server = EchoServer::spawn(cert.server_config(ALPN));
        let chain = Chained::default();
        let named = at("quic.test", server.addr().port());
        let path = open(&chain, &named, None, Tristate::Auto).await;
        assert_eq!(*chain.asked.lock().unwrap(), [named]);
        assert_eq!(path.remote, carrier::STAND_IN);
        assert!(path.socket.may_fragment());
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        echoes(&connection).await;
        assert_eq!(connection.stats().path.sent_plpmtud_probes, 0);
    }

    /// A path says whether it goes over a carrier, whatever lies over it
    /// (`ecn=off`): a connection takes its transport accordingly
    /// (`Transport::apply_carried`). Nothing is sent here.
    #[tokio::test]
    async fn a_path_says_whether_it_goes_over_a_carrier() {
        let server = at("127.0.0.1", 443);
        let hopping = PortHopping {
            ports: vec![5000..=6000],
            interval: Duration::from_secs(30),
        };
        for ecn in [Tristate::Auto, Tristate::Off] {
            assert!(!open(&direct(), &server, None, ecn).await.is_carried());
            let hops = open(&direct(), &server, Some(&hopping), ecn).await;
            assert!(!hops.is_carried());
            let carried = open(&Chained::default(), &server, None, ecn).await;
            assert!(carried.is_carried());
        }
    }

    /// A carrier that ends takes the connection with it at once, not after
    /// the idle timeout. quinn reports the endpoint it lost, and then — as
    /// the connection's driver goes — that it was closed.
    #[tokio::test]
    async fn a_carrier_that_ends_ends_the_connection() {
        let cert = TestCert::new();
        let server = EchoServer::spawn(cert.server_config(ALPN));
        let chain = Chained::default();
        let path = open(
            &chain,
            &at("quic.test", server.addr().port()),
            None,
            Tristate::Auto,
        )
        .await;
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        chain.cut.now();
        let closed = timeout(Duration::from_secs(5), connection.closed())
            .await
            .expect("closed at once");
        assert!(
            matches!(
                closed,
                ConnectionError::LocallyClosed | ConnectionError::TransportError(_)
            ),
            "{closed:?}"
        );
    }

    /// Hands out a direct connector's own UDP flows — connected sockets —
    /// as a chain's, the way the resolver's connector does for a DNS
    /// server over QUIC.
    struct Flows;

    impl Connector for Flows {
        fn connect<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            Box::pin(std::future::ready(Err(io::Error::other("UDP only"))))
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            Box::pin(async move { direct().connect_udp(target, opts).await })
        }
    }

    /// A carrier that cannot send yet keeps quinn waiting, where its
    /// datagram was once dropped: a new socket is not known to be writable
    /// before the reactor has run, which on this runtime it does between
    /// the tasks — after the connection's first packet was to go. The
    /// handshake does not wait for quinn to send that packet again, a
    /// second later.
    #[tokio::test]
    async fn a_carrier_that_cannot_send_yet_keeps_the_first_packet() {
        let cert = TestCert::new();
        let server = EchoServer::spawn(cert.server_config(ALPN));
        let path = open(
            &Flows,
            &at("127.0.0.1", server.addr().port()),
            None,
            Tristate::Auto,
        )
        .await;
        assert_eq!(path.remote, carrier::STAND_IN);
        let started = Instant::now();
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        let took = started.elapsed();
        assert!(took < Duration::from_millis(500), "{took:?}");
        echoes(&connection).await;
    }

    /// A carrier nothing goes out on: every send fails, as through a proxy
    /// that turns out to relay no UDP.
    struct NoRelay;

    impl PacketSocket for NoRelay {
        fn send_to<'a>(&'a self, _buf: &'a [u8], _to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
            Box::pin(std::future::ready(Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "proxy: the server relays no UDP",
            ))))
        }

        fn recv_from<'a>(
            &'a self,
            _buf: &'a mut [u8],
        ) -> BoxFuture<'a, io::Result<(usize, Target)>> {
            Box::pin(std::future::pending())
        }
    }

    /// The chain of `NoRelay`.
    struct Unrelayed;

    impl Connector for Unrelayed {
        fn connect<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            Box::pin(std::future::ready(Err(io::Error::other("UDP only"))))
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            let carrier = packet_datagram(Box::new(NoRelay), target.clone());
            Box::pin(std::future::ready(Ok(carrier)))
        }
    }

    /// A carrier that cannot send at all fails the dial at once, and in its
    /// own words: not in quinn's — which knows only that its endpoint is
    /// gone —, nor by a handshake left to time out.
    #[tokio::test]
    async fn a_carrier_that_cannot_send_fails_the_dial_in_its_own_words() {
        let path = open(&Unrelayed, &at("quic.test", 443), None, Tristate::Auto).await;
        // a handshake nothing answers would end here
        let transport = Transport {
            idle_timeout: Duration::from_secs(1),
            ..Transport::STANDARD
        };
        let err = dial(path, &TestCert::new(), &transport).await.unwrap_err();
        assert_eq!(
            (err.kind(), err.to_string().as_str()),
            (
                io::ErrorKind::Unsupported,
                "proxy: the server relays no UDP"
            )
        );
    }

    #[tokio::test]
    async fn hopping_cannot_go_through_a_carrier() {
        let hopping = PortHopping {
            ports: vec![5000..=6000],
            interval: Duration::from_secs(30),
        };
        let err = Path::open(
            &Chained::default(),
            &at("quic.test", 443),
            Some(&hopping),
            Tristate::Auto,
            &ConnectOpts::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "quic: `port-hopping` cannot be combined with `underlying-proxy`"
        );
    }

    /// The server listens on three ports; the client hops among them every
    /// 200 ms (the loader allows no less than 5 s), from a new local
    /// socket each time, and the connection carries on.
    #[tokio::test]
    async fn a_hopping_path_moves_among_the_ports() {
        let cert = TestCert::new();
        let ports = MultiPort::bind(3);
        let _server = EchoServer::spawn_on(cert.server_config(ALPN), ports.clone());
        let hopping = PortHopping {
            ports: ports.ports().iter().map(|&p| p..=p).collect(),
            interval: Duration::from_millis(200),
        };
        // the line's own port is not used
        let path = open(
            &direct(),
            &at("127.0.0.1", 9),
            Some(&hopping),
            Tristate::Auto,
        )
        .await;
        assert!(ports.ports().contains(&path.remote.port()));
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while ports.heard().iter().filter(|&&n| n > 0).count() < 2 {
            assert!(
                Instant::now() < deadline,
                "heard only on {:?}",
                ports.heard()
            );
            echoes(&connection).await;
        }
        assert!(ports.clients() >= 2, "a new local socket per hop");
        echoes(&connection).await;
    }

    /// Each hop's socket is made as the first one: with the policy's
    /// `tos`, under `ecn=off`.
    #[tokio::test]
    async fn every_hop_has_the_policys_socket() {
        let cert = TestCert::new();
        let ports = MultiPort::bind(2);
        let _server = EchoServer::spawn_on(cert.server_config(ALPN), ports.clone());
        let hook = Arc::new(TosHook::default());
        let connector = DirectConnector::with_opts(
            Arc::new(Loopback),
            SocketOpts {
                tos: 0x88,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        let hopping = PortHopping {
            ports: ports.ports().iter().map(|&p| p..=p).collect(),
            interval: Duration::from_millis(100),
        };
        let path = open(
            &connector,
            &at("127.0.0.1", 9),
            Some(&hopping),
            Tristate::Off,
        )
        .await;
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        // the first socket and two hops'
        let deadline = Instant::now() + BOUND;
        while hook.0.lock().unwrap().len() < 3 {
            assert!(Instant::now() < deadline, "no hops");
            echoes(&connection).await;
        }
        assert!(hook.0.lock().unwrap().iter().all(|&tos| tos == 0x88));
        echoes(&connection).await;
    }

    /// A server socket that counts the datagrams it heard with an ECN mark.
    #[derive(Debug)]
    struct Marks {
        inner: Arc<dyn AsyncUdpSocket>,
        marked: AtomicUsize,
    }

    impl AsyncUdpSocket for Marks {
        fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
            self.inner.clone().create_io_poller()
        }

        fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
            self.inner.try_send(transmit)
        }

        fn poll_recv(
            &self,
            cx: &mut Context<'_>,
            bufs: &mut [IoSliceMut<'_>],
            meta: &mut [RecvMeta],
        ) -> Poll<io::Result<usize>> {
            let n = ready!(self.inner.poll_recv(cx, bufs, meta))?;
            let marked = meta[..n].iter().filter(|meta| meta.ecn.is_some()).count();
            self.marked.fetch_add(marked, Ordering::SeqCst);
            Poll::Ready(Ok(n))
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            self.inner.local_addr()
        }

        fn max_transmit_segments(&self) -> usize {
            self.inner.max_transmit_segments()
        }

        fn max_receive_segments(&self) -> usize {
            self.inner.max_receive_segments()
        }

        fn may_fragment(&self) -> bool {
            self.inner.may_fragment()
        }
    }

    /// How many ECN-marked datagrams the server hears of a short exchange
    /// over a path with `tos` and `ecn`.
    async fn marks_heard(tos: u8, ecn: Tristate) -> usize {
        let cert = TestCert::new();
        let marks = Arc::new(Marks {
            inner: loopback_socket(),
            marked: AtomicUsize::new(0),
        });
        let server = EchoServer::spawn_on(cert.server_config(ALPN), marks.clone());
        let connector = DirectConnector::with_opts(
            Arc::new(Loopback),
            SocketOpts {
                tos,
                ..SocketOpts::default()
            },
            Arc::new(TosHook::default()),
        );
        let path = open(
            &connector,
            &at("127.0.0.1", server.addr().port()),
            None,
            ecn,
        )
        .await;
        let connection = dial(path, &cert, &Transport::STANDARD).await.unwrap();
        echoes(&connection).await;
        marks.marked.load(Ordering::SeqCst)
    }

    /// quinn marks what it sends; `ecn=off` takes the marks off, and with
    /// `tos` packets go out plainly, unmarked. A system that shows no marks
    /// on the loopback at all leaves nothing to tell apart.
    #[tokio::test]
    async fn ecn_marks_go_out_unless_off_or_with_tos() {
        if marks_heard(0, Tristate::Auto).await == 0 {
            return;
        }
        assert_eq!(marks_heard(0, Tristate::Off).await, 0);
        assert_eq!(marks_heard(0x88, Tristate::Auto).await, 0);
    }

    #[test]
    fn the_standard_transport_is_the_designs() {
        assert_eq!(
            Transport::STANDARD,
            Transport {
                idle_timeout: Duration::from_secs(30),
                keep_alive: Some(Duration::from_secs(10)),
                stream_window: 8 * 1024 * 1024,
                connection_window: 256 * 1024 * 1024,
                carried_initial_mtu: None,
            }
        );
    }

    /// A raw server endpoint and the client's connection to it, with the
    /// server's side of it.
    async fn pair(transport: &Transport) -> (quinn::Endpoint, Connection, Connection) {
        let cert = TestCert::new();
        let server = endpoint(cert.server_config(ALPN), loopback_socket());
        let addr = server.local_addr().unwrap();
        let path = open(
            &direct(),
            &at("127.0.0.1", addr.port()),
            None,
            Tristate::Auto,
        )
        .await;
        let (client, accepted) = timeout(BOUND, async {
            tokio::join!(dial(path, &cert, transport), async {
                let incoming = server.accept().await.expect("a connection");
                incoming.await.expect("accepted")
            })
        })
        .await
        .expect("connected within the bound");
        (server, client.unwrap(), accepted)
    }

    /// The server may open unidirectional streams (HTTP/3 needs three), not
    /// bidirectional ones.
    #[tokio::test]
    async fn the_server_opens_no_bidirectional_stream() {
        let (_server, client, server_side) = pair(&Transport::STANDARD).await;
        let mut uni = server_side.open_uni().await.unwrap();
        uni.write_all(b"settings").await.unwrap();
        uni.finish().unwrap();
        let mut accepted = timeout(BOUND, client.accept_uni()).await.unwrap().unwrap();
        assert_eq!(accepted.read_to_end(64).await.unwrap(), b"settings");
        assert!(
            timeout(Duration::from_millis(300), server_side.open_bi())
                .await
                .is_err(),
            "a bidirectional stream from the server"
        );
    }

    #[tokio::test]
    async fn keep_alive_holds_an_idle_connection() {
        let transport = Transport {
            idle_timeout: Duration::from_secs(1),
            keep_alive: Some(Duration::from_millis(250)),
            ..Transport::STANDARD
        };
        let (_server, client, _server_side) = pair(&transport).await;
        // more than two idle timeouts with nothing sent but keep-alives
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(client.close_reason().is_none());
    }

    #[tokio::test]
    async fn an_idle_connection_times_out() {
        let transport = Transport {
            idle_timeout: Duration::from_millis(300),
            // past the idle timeout: no keep-alive in time
            keep_alive: Some(Duration::from_secs(10)),
            ..Transport::STANDARD
        };
        let (_server, client, _server_side) = pair(&transport).await;
        let closed = timeout(BOUND, client.closed()).await.unwrap();
        let err = connection_error(&closed);
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            err.to_string(),
            "quic: the server stopped answering (idle timeout)"
        );
    }

    #[tokio::test]
    async fn a_server_that_never_answers_times_the_handshake_out() {
        // a socket that takes the packets and never answers
        let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = silent.local_addr().unwrap().port();
        let transport = Transport {
            idle_timeout: Duration::from_millis(300),
            ..Transport::STANDARD
        };
        let path = open(&direct(), &at("127.0.0.1", port), None, Tristate::Auto).await;
        let err = dial(path, &TestCert::new(), &transport).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(err.to_string(), "quic: the handshake timed out");
    }

    #[tokio::test]
    async fn an_untrusted_certificate_is_a_tls_error() {
        let server = EchoServer::spawn(TestCert::new().server_config(ALPN));
        let path = open(
            &direct(),
            &at("127.0.0.1", server.addr().port()),
            None,
            Tristate::Auto,
        )
        .await;
        // trusts another certificate, of the same subject
        let err = dial(path, &TestCert::new(), &Transport::STANDARD)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string()
                .starts_with("tls: invalid peer certificate: "),
            "{err}"
        );
    }

    /// QUIC requires ALPN: a server that has none of the client's, or none
    /// at all, refuses the handshake.
    #[tokio::test]
    async fn the_handshake_needs_a_common_alpn() {
        for server_alpn in [&["x"][..], &[]] {
            let cert = TestCert::new();
            let server = EchoServer::spawn(cert.server_config(server_alpn));
            let path = open(
                &direct(),
                &at("127.0.0.1", server.addr().port()),
                None,
                Tristate::Auto,
            )
            .await;
            let err = dial(path, &cert, &Transport::STANDARD).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData);
            assert_eq!(
                err.to_string(),
                "tls: the server refused the handshake (NoApplicationProtocol)"
            );
        }
    }

    /// The server's close and its code; what it says is cleaned.
    #[tokio::test]
    async fn a_close_by_the_server_carries_its_code() {
        let (_server, client, server_side) = pair(&Transport::STANDARD).await;
        server_side.close(0x101u32.into(), b"going\naway");
        let closed = timeout(BOUND, client.closed()).await.unwrap();
        let err = connection_error(&closed);
        assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(
            err.to_string(),
            "quic: the server closed the connection (code 0x101: goingaway)"
        );
    }

    #[tokio::test]
    async fn a_name_tls_cannot_use_is_refused_before_any_packet() {
        let path = open(&direct(), &at("127.0.0.1", 443), None, Tristate::Auto).await;
        let config = client_config(TestCert::new().client_tls(ALPN), &Transport::STANDARD).unwrap();
        let err = connect(path, &config, "no such name!").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(err.to_string(), "quic: invalid server name: no such name!");
    }
}
