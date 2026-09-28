//! `WireGuardOutbound` (phase 2 M4 design §6): the tunnel starts with the
//! first dial and runs while the outbound, or a connection through it,
//! lives.

use crate::device::Device;
use crate::stack::Refusal;
use rurge_config::HostName;
use rurge_config::spec::WireGuardSpec;
use rurge_config::wireguard::WireGuardSection;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Resolve, Target};
use rurge_proto::{Outbound, OutboundError};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct WireGuardOutbound {
    name: String,
    section: WireGuardSection,
    /// Destination names, without a `dns-server` (M4-D9).
    resolver: Arc<dyn Resolve>,
    /// What the carriers to the peers come from.
    connector: Arc<dyn Connector>,
    /// The running tunnel; the first dial starts it, the others wait.
    device: Mutex<Option<Arc<Device>>>,
}

impl WireGuardOutbound {
    /// Nothing is opened or resolved before the first dial (M4 design 6.5).
    pub fn new(
        name: &str,
        spec: &WireGuardSpec,
        resolver: Arc<dyn Resolve>,
        connector: Arc<dyn Connector>,
    ) -> WireGuardOutbound {
        WireGuardOutbound {
            name: name.to_string(),
            section: spec.section.clone(),
            resolver,
            connector,
            device: Mutex::new(None),
        }
    }

    pub(crate) async fn device(&self, opts: &ConnectOpts) -> Result<Arc<Device>, OutboundError> {
        let mut slot = self.device.lock().await;
        if let Some(device) = slot.as_ref() {
            return Ok(device.clone());
        }
        let device = Device::start(&self.name, &self.section, &self.connector, opts).await?;
        *slot = Some(device.clone());
        Ok(device)
    }

    /// Where a connection to `target` goes: its address, or its name
    /// resolved on this machine (M4-D9).
    async fn address(&self, target: &Target) -> Result<IpAddr, OutboundError> {
        let name = match &target.host {
            HostName::Ip(ip) => return Ok(*ip),
            HostName::Domain(name) => name,
        };
        let addrs =
            self.resolver.resolve(name).await.map_err(|_| {
                OutboundError::Dns(format!("wireguard: dns lookup of {name} failed"))
            })?;
        pick(&addrs, &self.section).map_err(|refusal| OutboundError::Proxy(refusal.to_string()))
    }

    async fn dial(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        let device = self.device(opts).await?;
        let ip = self.address(target).await?;
        let stream = device.connect(SocketAddr::new(ip, target.port)).await?;
        Ok(Box::new(stream))
    }
}

/// The address to connect to among `addrs`: of a family the tunnel has an
/// address of, IPv6 first with `prefer-ipv6`.
fn pick(addrs: &[IpAddr], section: &WireGuardSection) -> Result<IpAddr, Refusal> {
    let usable = |ip: &IpAddr| match ip {
        IpAddr::V4(_) => section.self_ip.is_some(),
        IpAddr::V6(_) => section.self_ip_v6.is_some(),
    };
    let mut ordered: Vec<IpAddr> = addrs.iter().copied().filter(usable).collect();
    // stable: the preferred family first, each in the order answered
    ordered.sort_by_key(|ip| ip.is_ipv6() != section.prefer_ipv6);
    match (ordered.first(), addrs.first()) {
        (Some(ip), _) => Ok(*ip),
        (None, Some(ip)) => Err(Refusal::NoAddress(*ip)),
        (None, None) => Err(Refusal::NoAddress(IpAddr::V4(
            std::net::Ipv4Addr::UNSPECIFIED,
        ))),
    }
}

impl Outbound for WireGuardOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.dial(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ECHO_PORT, FakeWgPeer, PeerOpts, endpoint, keypair, section};
    use rurge_config::wireguard::PeerEndpoint;
    use rurge_net::connector::{BoxedDatagram, Datagram, DirectConnector, SystemResolve};
    use std::io;
    use std::net::Ipv4Addr;
    use std::sync::Mutex as StdMutex;
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadBuf};

    /// Names to addresses, for the destinations resolved on this machine.
    struct Names(Vec<(&'static str, Vec<IpAddr>)>);

    impl Resolve for Names {
        fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            let found = self.0.iter().find(|(name, _)| *name == host);
            Box::pin(std::future::ready(match found {
                Some((_, addrs)) => Ok(addrs.clone()),
                None => Err(io::Error::new(io::ErrorKind::NotFound, "no such name")),
            }))
        }
    }

    fn no_names() -> Arc<dyn Resolve> {
        Arc::new(Names(Vec::new()))
    }

    fn direct() -> Arc<dyn Connector> {
        Arc::new(DirectConnector::new(Arc::new(SystemResolve)))
    }

    /// A tunnel to one peer that takes 10.0.0.0/8, and the peer.
    async fn tunnel(
        opts: PeerOpts,
        edit: impl FnOnce(&mut WireGuardSection),
    ) -> (FakeWgPeer, WireGuardOutbound) {
        tunnel_with(opts, edit, no_names(), direct()).await
    }

    async fn tunnel_with(
        opts: PeerOpts,
        edit: impl FnOnce(&mut WireGuardSection),
        resolver: Arc<dyn Resolve>,
        connector: Arc<dyn Connector>,
    ) -> (FakeWgPeer, WireGuardOutbound) {
        let (private, public) = keypair();
        let peer = FakeWgPeer::start(public, opts).await;
        let mut section = section(
            private,
            Ipv4Addr::new(10, 9, 0, 2),
            &[(peer.public_key(), &["10.0.0.0/8"])],
        );
        section.peers[0].endpoint = endpoint(peer.addr());
        edit(&mut section);
        let outbound =
            WireGuardOutbound::new("WG", &WireGuardSpec { section }, resolver, connector);
        (peer, outbound)
    }

    fn at(host: &str, port: u16) -> Target {
        Target::new(HostName::parse(host), port)
    }

    fn within(secs: u64) -> ConnectOpts {
        ConnectOpts {
            timeout: Duration::from_secs(secs),
        }
    }

    async fn echo(stream: &mut BoxedStream, data: &[u8]) -> Vec<u8> {
        tokio::time::timeout(Duration::from_secs(5), async {
            stream.write_all(data).await.unwrap();
            let mut back = vec![0u8; data.len()];
            stream.read_exact(&mut back).await.unwrap();
            back
        })
        .await
        .expect("the echo came back")
    }

    #[tokio::test]
    async fn a_connection_through_the_tunnel_echoes() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        assert_eq!(peer.core().handshakes, 1);
        assert_eq!(
            peer.core().connected_to,
            ["10.0.0.1:7".parse::<SocketAddr>().unwrap()]
        );
    }

    /// Flow control both ways: more than the buffers hold, written and read
    /// at once; then the end of the stream both ways. The FIN goes last:
    /// the peer is smoltcp 0.12 too, which in CLOSE-WAIT stops
    /// retransmitting what it still has in flight.
    #[tokio::test]
    async fn a_large_transfer_goes_through_whole() {
        let (_peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        let data: Vec<u8> = (0..2 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        let (mut read, mut write) = tokio::io::split(stream);
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            write.write_all(&sent).await.unwrap();
            write
        });
        let mut back = vec![0u8; data.len()];
        tokio::time::timeout(Duration::from_secs(30), read.read_exact(&mut back))
            .await
            .expect("everything came back")
            .unwrap();
        assert!(back == data, "the bytes came back in order");
        let mut write = writer.await.unwrap();
        write.shutdown().await.unwrap();
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), read.read_to_end(&mut rest))
            .await
            .expect("the far end finished too")
            .unwrap();
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn every_message_carries_the_client_id() {
        let id = [83, 12, 235];
        let (peer, wg) = tunnel(
            PeerOpts {
                client_id: Some(id),
                ..PeerOpts::default()
            },
            |s| s.peers[0].client_id = Some(id),
        )
        .await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        let core = peer.core();
        assert!(
            core.reserved.iter().all(|r| *r == id),
            "{:?}",
            core.reserved
        );
    }

    /// Without a `dns-server`, a name is resolved on this machine, with
    /// `[Host]` (M4-D9); the family follows the tunnel's addresses and
    /// `prefer-ipv6`.
    #[tokio::test]
    async fn a_name_is_resolved_on_this_machine() {
        let names = Arc::new(Names(vec![(
            "echo.test",
            vec!["fd00::1".parse().unwrap(), "10.0.0.1".parse().unwrap()],
        )]));
        for (prefer_ipv6, expected) in [(false, "10.0.0.1:7"), (true, "[fd00::1]:7")] {
            let (peer, wg) = tunnel_with(
                PeerOpts::default(),
                |s| {
                    s.self_ip_v6 = Some("fd00::2".parse().unwrap());
                    s.peers[0].allowed_ips.push("fd00::/64".parse().unwrap());
                    s.prefer_ipv6 = prefer_ipv6;
                },
                names.clone(),
                direct(),
            )
            .await;
            let mut stream = wg
                .connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
                .await
                .expect("a connection");
            assert_eq!(echo(&mut stream, b"ping").await, b"ping");
            assert_eq!(
                peer.core().connected_to,
                [expected.parse::<SocketAddr>().unwrap()]
            );
        }
        // an IPv4-only tunnel takes the IPv4 answer whatever `prefer-ipv6` says
        let (peer, wg) = tunnel_with(
            PeerOpts::default(),
            |s| s.prefer_ipv6 = true,
            names.clone(),
            direct(),
        )
        .await;
        wg.connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(
            peer.core().connected_to,
            ["10.0.0.1:7".parse::<SocketAddr>().unwrap()]
        );
    }

    #[tokio::test]
    async fn names_and_addresses_the_tunnel_cannot_use_fail_at_once() {
        let names = Arc::new(Names(vec![("v6.test", vec!["fd00::1".parse().unwrap()])]));
        let (_peer, wg) = tunnel_with(PeerOpts::default(), |_| {}, names, direct()).await;
        let refused = |target: Target| {
            let wg = &wg;
            async move {
                let started = std::time::Instant::now();
                let e = wg
                    .connect_tcp(&target, &within(5))
                    .await
                    .map(|_| ())
                    .unwrap_err();
                assert!(started.elapsed() < Duration::from_secs(2), "{e}");
                e.to_string()
            }
        };
        assert_eq!(
            refused(at("192.0.2.1", 80)).await,
            "wireguard: no peer's allowed-ips covers 192.0.2.1"
        );
        assert_eq!(
            refused(at("v6.test", 80)).await,
            "wireguard: the tunnel has no IPv6 address"
        );
        assert_eq!(
            refused(at("nx.test", 80)).await,
            "dns: wireguard: dns lookup of nx.test failed"
        );
    }

    #[tokio::test]
    async fn a_closed_port_refuses_the_connection() {
        let (_peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let e = wg
            .connect_tcp(&at("10.0.0.1", 1), &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "wireguard: the destination refused the connection"
        );
    }

    #[tokio::test]
    async fn a_peer_that_never_answers_runs_the_dial_out_of_time() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        peer.go_silent(true);
        let e = wg
            .connect_tcp(
                &at("10.0.0.1", ECHO_PORT),
                &ConnectOpts {
                    timeout: Duration::from_millis(500),
                },
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(e, OutboundError::Timeout), "{e}");
    }

    /// One tunnel and one handshake for every dial that comes in together.
    #[tokio::test]
    async fn dials_that_come_together_share_one_tunnel() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let (target, opts) = (at("10.0.0.1", ECHO_PORT), within(5));
        let dial = || wg.connect_tcp(&target, &opts);
        let (a, b, c, d, e) = tokio::join!(dial(), dial(), dial(), dial(), dial());
        for dialled in [a, b, c, d, e] {
            let mut stream = dialled.expect("a connection");
            assert_eq!(echo(&mut stream, b"x").await, b"x");
        }
        assert_eq!(peer.core().handshakes, 1);
        assert_eq!(peer.core().accepted, 5);
    }

    /// The outbound gone and no connection left: the tunnel stops. A
    /// connection keeps it running until it ends.
    #[tokio::test]
    async fn the_tunnel_lives_as_long_as_the_outbound_or_a_connection() {
        let (_peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        let device = Arc::downgrade(&wg.device(&within(5)).await.unwrap());
        drop(wg);
        assert_eq!(echo(&mut stream, b"still").await, b"still");
        assert!(device.upgrade().is_some());
        drop(stream);
        assert!(
            device.upgrade().is_none(),
            "the last holder was the connection"
        );
    }

    /// Handshake initiations go out marked AF41 (manual), the rest not.
    #[tokio::test]
    async fn handshake_initiations_go_out_marked() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let recording: Arc<dyn Connector> = Arc::new(Recording {
            inner: direct(),
            log: log.clone(),
        });
        let (_peer, wg) = tunnel_with(PeerOpts::default(), |_| {}, no_names(), recording).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        let log = log.lock().unwrap().clone();
        assert_eq!(log[..3], ["tos 0x88", "send 1", "tos 0x00"], "{log:?}");
        assert!(
            log[3..].iter().all(|entry| entry == "send 4"),
            "only data after the handshake: {log:?}"
        );
    }

    /// Connects through `inner` and notes every `set_tos` and the type of
    /// every message sent.
    struct Recording {
        inner: Arc<dyn Connector>,
        log: Arc<StdMutex<Vec<String>>>,
    }

    struct RecordingDatagram {
        inner: BoxedDatagram,
        log: Arc<StdMutex<Vec<String>>>,
    }

    impl Connector for Recording {
        fn connect<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            self.inner.connect(target, opts)
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            Box::pin(async move {
                let inner = self.inner.connect_udp(target, opts).await?;
                Ok(Box::new(RecordingDatagram {
                    inner,
                    log: self.log.clone(),
                }) as BoxedDatagram)
            })
        }
    }

    impl Datagram for RecordingDatagram {
        fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            let sent = self.inner.poll_send(cx, buf);
            if sent.is_ready() {
                self.log.lock().unwrap().push(format!("send {}", buf[0]));
            }
            sent
        }

        fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            self.inner.poll_recv(cx, buf)
        }

        fn set_tos(&self, tos: u8) -> io::Result<()> {
            self.log.lock().unwrap().push(format!("tos {tos:#04x}"));
            Ok(())
        }
    }

    /// The endpoint of the only peer cannot be resolved: the dial fails, and
    /// the next one tries again.
    #[tokio::test]
    async fn a_tunnel_whose_peer_cannot_be_reached_does_not_start() {
        let (private, _) = keypair();
        let mut section = section(
            private,
            Ipv4Addr::new(10, 9, 0, 2),
            &[(keypair().1, &["10.0.0.0/8"])],
        );
        section.peers[0].endpoint = PeerEndpoint {
            host: HostName::parse("nx.invalid"),
            port: 51820,
        };
        let failing: Arc<dyn Connector> = Arc::new(DirectConnector::new(no_names()));
        let wg = WireGuardOutbound::new("WG", &WireGuardSpec { section }, no_names(), failing);
        for _ in 0..2 {
            let e = wg
                .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
                .await
                .map(|_| ())
                .unwrap_err();
            assert_eq!(e.to_string(), "no such name");
        }
        assert!(wg.device.lock().await.is_none());
    }
}
