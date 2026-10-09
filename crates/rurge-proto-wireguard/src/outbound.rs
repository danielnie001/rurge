//! `WireGuardOutbound` (phase 2 M4 design §6): the tunnel starts with the
//! first dial and runs while the outbound, or a connection through it,
//! lives. UDP goes through a socket of the tunnel's own stack (`udp`).

use crate::device::{Device, REDIAL};
use crate::dns::{self, Cache, Family};
use crate::stack::Refusal;
use crate::udp::TunnelUdp;
use rurge_config::HostName;
use rurge_config::spec::WireGuardSpec;
use rurge_config::wireguard::{TunnelDns, WireGuardSection};
use rurge_net::BoxFuture;
use rurge_net::connector::{
    BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Resolve, Target,
};
use rurge_proto::{Outbound, OutboundError, UdpSupport};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// How long one `dns-server` has to answer before the next is asked.
const DNS_WAIT: Duration = Duration::from_secs(2);

/// The generation of the next outbound: a reload builds its outbounds after
/// the ones they replace.
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub struct WireGuardOutbound {
    name: String,
    /// Later configurations have higher ones: a tunnel they start ends
    /// this one's, never the other way round.
    generation: u64,
    /// What the policy dials carriers with: distinguishes tunnels that
    /// share a section but must not share carriers (`with_carrier`). Empty
    /// until set.
    carrier: String,
    /// What the carriers to the peers come from.
    connector: Arc<dyn Connector>,
    /// The running tunnel; the first dial starts it, the others wait.
    device: Mutex<Option<Arc<Device>>>,
    /// The section, and how destination names are looked up: shared with
    /// the UDP carriers.
    names: Arc<Names>,
    /// `REDIAL` (shorter in the tests).
    redial: Duration,
}

/// Where destination names go (M4-D9): the section's `dns-server`s through
/// the tunnel, or this machine.
pub(crate) struct Names {
    section: WireGuardSection,
    /// Destination names, without a `dns-server` (M4-D9) and for its
    /// `system` entries.
    resolver: Arc<dyn Resolve>,
    /// What the tunnel's `dns-server`s answered, for their TTL.
    cache: StdMutex<Cache>,
    /// `DNS_WAIT` (shorter in the tests).
    dns_wait: Duration,
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
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            carrier: String::new(),
            connector,
            device: Mutex::new(None),
            names: Arc::new(Names {
                section: spec.section.clone(),
                resolver,
                cache: StdMutex::new(Cache::default()),
                dns_wait: DNS_WAIT,
            }),
            redial: REDIAL,
        }
    }

    /// A tunnel is shared only between policies whose carrier is alike: the
    /// engine's factory gives the policy's socket options and its
    /// `underlying-proxy` chain here (M4 design 6.5 / P10).
    pub fn with_carrier(mut self, carrier: String) -> WireGuardOutbound {
        self.carrier = carrier;
        self
    }

    /// The network changed: a running tunnel dials every carrier anew and
    /// greets the peers again (M4 design 6.5). Nothing detects it before
    /// phase 3; one that is starting uses the new network anyway.
    pub fn network_changed(&self) {
        if let Ok(slot) = self.device.try_lock()
            && let Some(device) = slot.as_ref()
        {
            device.network_changed();
        }
    }

    /// The policy's tunnel: the one it has, while that runs; else another
    /// — the first time, and once the one it had is over (a later
    /// configuration took it over, its task died, its stack's lock was
    /// poisoned).
    pub(crate) async fn device(&self, opts: &ConnectOpts) -> Result<Arc<Device>, OutboundError> {
        let mut slot = self.device.lock().await;
        if let Some(device) = slot.as_ref()
            && !device.is_closed()
        {
            return Ok(device.clone());
        }
        // a tunnel that ended on its own is said so, once: the slot stays
        // empty until another has started. One that a later configuration
        // took over is no surprise, and one that nobody holds any more is
        // never found here
        if let Some(ended) = slot.take()
            && ended.ended_unexpectedly()
        {
            tracing::warn!(policy = %self.name, "wireguard: the tunnel ended unexpectedly; the next connection starts a new one");
        }
        let device = Device::start(
            &self.name,
            &self.names.section,
            &self.carrier,
            self.generation,
            &self.connector,
            opts,
            self.redial,
        )
        .await?;
        *slot = Some(device.clone());
        Ok(device)
    }

    async fn dial(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        let device = self.device(opts).await?;
        let ip = self.names.address(target, &device).await?;
        let stream = device.connect(SocketAddr::new(ip, target.port)).await?;
        Ok(Box::new(stream))
    }

    /// `DNS_WAIT`, before anything shares the names (a test's).
    #[cfg(test)]
    fn set_dns_wait(&mut self, wait: Duration) {
        Arc::get_mut(&mut self.names)
            .expect("nothing shares the names yet")
            .dns_wait = wait;
    }
}

impl Names {
    /// Where a connection or a datagram to `target` goes: its address, or
    /// its name's.
    pub(crate) async fn address(
        &self,
        target: &Target,
        device: &Arc<Device>,
    ) -> Result<IpAddr, OutboundError> {
        let name = match &target.host {
            HostName::Ip(ip) => return Ok(*ip),
            HostName::Domain(name) => name,
        };
        let addrs = self
            .lookup(name, device)
            .await?
            .ok_or_else(|| OutboundError::Dns(format!("wireguard: dns lookup of {name} failed")))?;
        pick(&addrs, &self.section).map_err(|refusal| OutboundError::Proxy(refusal.to_string()))
    }

    /// `name`'s addresses: from the section's `dns-server`s in order through
    /// the tunnel — the first that answers ends the search, `system` asks
    /// this machine — or, without any, from this machine (M4-D9). A server
    /// in the tunnel is asked for the name in its ASCII form, an IDN as its
    /// A-labels, and passed over for a name without one. An error when the
    /// search ends without an answer and one was passed over so, and when
    /// the tunnel is over: none of its servers would answer.
    async fn lookup(
        &self,
        name: &str,
        device: &Arc<Device>,
    ) -> Result<Option<Vec<IpAddr>>, OutboundError> {
        if self.section.dns_servers.is_empty() {
            return Ok(self.resolver.resolve(name).await.ok());
        }
        if let Some(addrs) = self
            .cache
            .lock()
            .expect("the cache")
            .get(name, Instant::now())
        {
            return Ok(Some(addrs));
        }
        let asked = rurge_config::hostname::to_ascii(name);
        let mut passed_over = false;
        for server in &self.section.dns_servers {
            let server = match server {
                TunnelDns::System => match self.resolver.resolve(name).await {
                    Ok(addrs) => return Ok(Some(addrs)),
                    Err(_) => continue,
                },
                TunnelDns::Server(server) => *server,
            };
            // no question can carry the name: `system`, further down the
            // list, may still know it
            let Some(asked) = &asked else {
                passed_over = true;
                continue;
            };
            // a question sent before the tunnel's first handshake waits for
            // it in boringtun, whose retry of a lost initiation comes after
            // `dns_wait`: the handshake first, within the dial's own time
            device.established().await?;
            let Some(answer) = self.ask(device, server, asked).await? else {
                continue;
            };
            let ttl = answer.iter().map(|(_, ttl)| *ttl).min().unwrap_or(0);
            let addrs: Vec<IpAddr> = answer.into_iter().map(|(ip, _)| ip).collect();
            self.cache.lock().expect("the cache").put(
                name,
                addrs.clone(),
                Duration::from_secs(ttl.into()),
                Instant::now(),
            );
            return Ok((!addrs.is_empty()).then_some(addrs));
        }
        if passed_over {
            return Err(OutboundError::Dns(
                "wireguard: the host name cannot be looked up".to_string(),
            ));
        }
        Ok(None)
    }

    /// `server`'s answer for `name`: A and AAAA at once, for the families
    /// the tunnel has an address of. `None` when it has not answered
    /// (`dns::answered`) within `dns_wait`, or no peer takes it; an error
    /// when the tunnel is over.
    async fn ask(
        &self,
        device: &Arc<Device>,
        server: SocketAddr,
        name: &str,
    ) -> Result<Option<Vec<(IpAddr, u32)>>, OutboundError> {
        // `None` for a family not asked, else what came of its question
        let ask = |family: Family, wanted: bool| async move {
            if !wanted {
                return Ok(None);
            }
            let id = getrandom::u32().unwrap_or(0) as u16;
            let Some(question) = dns::question(id, name, family) else {
                return Ok(Some(None));
            };
            device
                .query(server, &question, self.dns_wait, |reply| {
                    dns::answer(reply, id, name, family)
                })
                .await
                .map(Some)
        };
        let (v4, v6) = tokio::join!(
            ask(Family::V4, self.section.self_ip.is_some()),
            ask(Family::V6, self.section.self_ip_v6.is_some())
        );
        Ok(dns::answered(v4?.into_iter().chain(v6?).collect()))
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

    /// A handshake with the peers (phase 2 M4 design 6.7); the tunnel starts
    /// first when it has not.
    fn native_test(&self) -> Option<BoxFuture<'_, Result<Duration, OutboundError>>> {
        Some(Box::pin(async move {
            let device = self.device(&ConnectOpts::default()).await?;
            device.handshake().await
        }))
    }

    fn udp(&self) -> UdpSupport {
        UdpSupport::Native
    }

    /// A socket of the tunnel's stack for every destination (full cone,
    /// phase 2 M5 design §7); the tunnel starts first when it has not.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            let device = match tokio::time::timeout(opts.timeout, self.device(opts)).await {
                Ok(device) => device?,
                Err(_) => return Err(OutboundError::Timeout),
            };
            Ok(Box::new(TunnelUdp::new(device, self.names.clone())) as BoxedPacketSocket)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        DNS_ADDRESS, ECHO_PORT, FakeWgPeer, PeerOpts, endpoint, keypair, section,
    };
    use rurge_config::wireguard::PeerEndpoint;
    use rurge_net::connector::{
        BoxedDatagram, Datagram, DirectConnector, PacketSocket, Route, SystemResolve,
    };
    use std::io;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::pin::Pin;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};

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

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// A peer that answers `echo.test` with `addrs`, and whose name server
    /// comes after `before` in the section's `dns-server`.
    async fn with_tunnel_dns(
        addrs: &[&str],
        before: &[TunnelDns],
        edit: impl FnOnce(&mut WireGuardSection),
    ) -> (FakeWgPeer, WireGuardOutbound) {
        let names = Arc::new(Names(vec![("local.test", vec![ip("10.0.0.1")])]));
        let dns = vec![(
            "echo.test".to_string(),
            addrs.iter().map(|a| ip(a)).collect(),
        )];
        let mut servers = before.to_vec();
        servers.push(TunnelDns::Server(SocketAddr::new(DNS_ADDRESS.into(), 53)));
        tunnel_with(
            PeerOpts {
                dns,
                ..PeerOpts::default()
            },
            |s| {
                s.dns_servers = servers;
                edit(s);
            },
            names,
            direct(),
        )
        .await
    }

    /// With a `dns-server`, a name is asked through the tunnel, and the
    /// answer kept for its TTL.
    #[tokio::test]
    async fn a_name_is_resolved_through_the_tunnel() {
        let (peer, wg) = with_tunnel_dns(&["10.0.0.1"], &[], |_| {}).await;
        for _ in 0..2 {
            let mut stream = wg
                .connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
                .await
                .expect("a connection");
            assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        }
        assert_eq!(peer.core().dns_questions, ["echo.test A"], "asked once");
    }

    /// The tunnel's name server is asked for a name in its ASCII form: one
    /// written in Unicode as its A-labels.
    #[tokio::test]
    async fn the_tunnel_dns_is_asked_for_the_a_labels_of_a_name_in_unicode() {
        let dns = vec![("xn--bcher-kva.example".to_string(), vec![ip("10.0.0.1")])];
        let (peer, wg) = tunnel(
            PeerOpts {
                dns,
                ..PeerOpts::default()
            },
            |s| s.dns_servers = vec![TunnelDns::Server(SocketAddr::new(DNS_ADDRESS.into(), 53))],
        )
        .await;
        for _ in 0..2 {
            let mut stream = wg
                .connect_tcp(&at("bücher.example", ECHO_PORT), &within(5))
                .await
                .expect("a connection");
            assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        }
        assert_eq!(
            peer.core().dns_questions,
            ["xn--bcher-kva.example A"],
            "asked once"
        );
    }

    /// A name without such a form can be asked of no server: the lookup
    /// fails at once, before the tunnel's first handshake is waited for.
    #[tokio::test]
    async fn a_name_no_question_can_carry_fails_the_lookup_at_once() {
        let (peer, wg) = with_tunnel_dns(&["10.0.0.1"], &[], |_| {}).await;
        peer.go_silent(true);
        for host in ["a＠b.example", "a b.example"] {
            let started = std::time::Instant::now();
            let e = wg
                .connect_tcp(
                    &Target::new(HostName::Domain(host.into()), ECHO_PORT),
                    &within(5),
                )
                .await
                .map(|_| ())
                .unwrap_err();
            assert_eq!(
                e.to_string(),
                "dns: wireguard: the host name cannot be looked up"
            );
            assert!(started.elapsed() < Duration::from_secs(2), "{host}");
        }
        assert!(peer.core().dns_questions.is_empty());
    }

    /// A tunnel with `servers` as its `dns-server`s, where this machine
    /// knows `known` at 10.0.0.1; and its peer.
    async fn with_dns_servers(
        servers: Vec<TunnelDns>,
        known: &'static str,
    ) -> (FakeWgPeer, WireGuardOutbound) {
        let names = Arc::new(Names(vec![(known, vec![ip("10.0.0.1")])]));
        tunnel_with(
            PeerOpts::default(),
            |s| s.dns_servers = servers,
            names,
            direct(),
        )
        .await
    }

    /// A server in the tunnel passes over a name no question can carry; it
    /// does not end the search. `system`, further down the list, is still
    /// asked, and answers for this machine as it did.
    #[tokio::test]
    async fn a_name_a_tunnel_server_cannot_be_asked_for_still_reaches_system() {
        let in_tunnel = TunnelDns::Server(SocketAddr::new(DNS_ADDRESS.into(), 53));
        let unaskable = |port| Target::new(HostName::Domain("a＠b.example".into()), port);
        let (peer, wg) = with_dns_servers(vec![in_tunnel, TunnelDns::System], "a＠b.example").await;
        let mut stream = wg
            .connect_tcp(&unaskable(ECHO_PORT), &within(5))
            .await
            .expect("resolved on this machine");
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        assert!(peer.core().dns_questions.is_empty());

        // nobody answers: the lookup fails for what the tunnel's server
        // could not be asked ...
        let (_peer, wg) = with_dns_servers(vec![in_tunnel, TunnelDns::System], "other.test").await;
        let e = wg
            .connect_tcp(&unaskable(ECHO_PORT), &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "dns: wireguard: the host name cannot be looked up"
        );
        // ... and a list without one fails as no `dns-server` at all does
        let (_peer, wg) = with_dns_servers(vec![TunnelDns::System], "other.test").await;
        let e = wg
            .connect_tcp(&unaskable(ECHO_PORT), &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "dns: wireguard: dns lookup of a＠b.example failed"
        );
    }

    /// A question that goes unanswered is sent again, the same one, every
    /// third of `dns_wait`: one lost packet does not fail the lookup.
    #[tokio::test]
    async fn a_lost_question_is_answered_by_a_resend() {
        let dns = vec![("echo.test".to_string(), vec![ip("10.0.0.1")])];
        let (peer, mut wg) = tunnel(
            PeerOpts {
                dns,
                dns_ignore: 1,
                ..PeerOpts::default()
            },
            |s| s.dns_servers = vec![TunnelDns::Server(SocketAddr::new(DNS_ADDRESS.into(), 53))],
        )
        .await;
        wg.set_dns_wait(Duration::from_millis(900));
        let mut stream = wg
            .connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        let asked = peer.core().dns_questions.clone();
        assert!(
            asked.len() >= 2 && asked.iter().all(|q| q == "echo.test A"),
            "{asked:?}"
        );
    }

    /// Right after the start, the first handshake initiation lost: a
    /// question sent now would wait in boringtun for its retry, 5 seconds
    /// on, longer than `dns_wait`. The lookup waits for the tunnel's first
    /// handshake instead, within the dial's own time.
    #[tokio::test]
    async fn the_first_lookup_waits_for_the_handshake() {
        let dns = vec![("echo.test".to_string(), vec![ip("10.0.0.1")])];
        let (peer, wg) = tunnel(
            PeerOpts {
                dns,
                handshake_ignore: 1,
                ..PeerOpts::default()
            },
            |s| s.dns_servers = vec![TunnelDns::Server(SocketAddr::new(DNS_ADDRESS.into(), 53))],
        )
        .await;
        let mut stream = wg
            .connect_tcp(&at("echo.test", ECHO_PORT), &within(10))
            .await
            .expect("a connection once the retried handshake completed");
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        assert_eq!(peer.core().handshakes, 1);
        assert_eq!(peer.core().dns_questions, ["echo.test A"]);
    }

    /// Both families asked at once when the tunnel has both addresses;
    /// `prefer-ipv6` picks among the answers.
    #[tokio::test]
    async fn prefer_ipv6_picks_among_what_the_tunnel_dns_answers() {
        for (prefer_ipv6, expected) in [(false, "10.0.0.1:7"), (true, "[fd00::1]:7")] {
            let (peer, wg) = with_tunnel_dns(&["10.0.0.1", "fd00::1"], &[], |s| {
                s.self_ip_v6 = Some("fd00::2".parse().unwrap());
                s.peers[0].allowed_ips.push("fd00::/64".parse().unwrap());
                s.prefer_ipv6 = prefer_ipv6;
            })
            .await;
            wg.connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
                .await
                .expect("a connection");
            let mut asked = peer.core().dns_questions.clone();
            asked.sort();
            assert_eq!(asked, ["echo.test A", "echo.test AAAA"]);
            assert_eq!(
                peer.core().connected_to,
                [expected.parse::<SocketAddr>().unwrap()]
            );
        }
    }

    /// A server that says nothing gives way to the next after `dns_wait`;
    /// one the tunnel cannot reach is passed over at once.
    #[tokio::test]
    async fn the_next_dns_server_is_asked_when_one_cannot_answer() {
        let silent = TunnelDns::Server("10.0.0.54:53".parse().unwrap());
        let (_peer, mut wg) = with_tunnel_dns(&["10.0.0.1"], &[silent], |_| {}).await;
        wg.set_dns_wait(Duration::from_millis(300));
        let started = std::time::Instant::now();
        wg.connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert!(started.elapsed() >= Duration::from_millis(300));

        let unreachable = [
            // no IPv6 address in the tunnel
            TunnelDns::Server("[fd00::53]:53".parse().unwrap()),
            // no peer takes it
            TunnelDns::Server("192.0.2.53:53".parse().unwrap()),
        ];
        let (_peer, mut wg) = with_tunnel_dns(&["10.0.0.1"], &unreachable, |_| {}).await;
        wg.set_dns_wait(Duration::from_secs(5));
        let started = std::time::Instant::now();
        wg.connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert!(started.elapsed() < Duration::from_secs(2), "no waiting");
    }

    /// A server the stack cannot send to (an unspecified address, port 0)
    /// is passed over at once. The dial runs on a thread of its own: should
    /// it ever hang, the test fails instead of hanging with it.
    #[test]
    fn a_dns_server_the_stack_cannot_send_to_is_passed_over() {
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            let elapsed = runtime.block_on(async {
                let unusable = [
                    TunnelDns::Server("0.0.0.0:53".parse().unwrap()),
                    TunnelDns::Server("10.0.0.53:0".parse().unwrap()),
                ];
                let (_peer, mut wg) = with_tunnel_dns(&["10.0.0.1"], &unusable, |s| {
                    // a route for every server, so that each gets a socket
                    s.peers[0].allowed_ips = vec!["0.0.0.0/0".parse().unwrap()];
                })
                .await;
                wg.set_dns_wait(Duration::from_secs(5));
                let started = std::time::Instant::now();
                wg.connect_tcp(&at("echo.test", ECHO_PORT), &within(5))
                    .await
                    .expect("a connection");
                started.elapsed()
            });
            let _ = done.send(elapsed);
        });
        let elapsed = finished
            .recv_timeout(Duration::from_secs(20))
            .expect("the dial ends");
        assert!(elapsed < Duration::from_secs(2), "no waiting: {elapsed:?}");
    }

    /// `system` asks this machine where it stands in the list; an answer
    /// without addresses ends the search all the same.
    #[tokio::test]
    async fn system_asks_this_machine_and_an_empty_answer_is_final() {
        let (peer, wg) = with_tunnel_dns(&[], &[TunnelDns::System], |_| {}).await;
        wg.connect_tcp(&at("local.test", ECHO_PORT), &within(5))
            .await
            .expect("resolved on this machine");
        assert!(peer.core().dns_questions.is_empty());

        let (peer, wg) = with_tunnel_dns(&[], &[], |s| s.dns_servers.push(TunnelDns::System)).await;
        let e = wg
            .connect_tcp(&at("local.test", ECHO_PORT), &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "dns: wireguard: dns lookup of local.test failed"
        );
        assert_eq!(peer.core().dns_questions, ["local.test A"]);
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

    /// Not a gate (M4 design §10): how fast a bulk transfer goes through the
    /// tunnel to a loopback peer, whose own smoltcp echoes it back. In
    /// release: `cargo test -p rurge-proto-wireguard --release throughput --
    /// --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn throughput() {
        const TOTAL: usize = 64 * 1024 * 1024;
        let (_peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        let (mut read, mut write) = tokio::io::split(stream);
        let started = std::time::Instant::now();
        let writer = tokio::spawn(async move {
            let chunk = vec![0x5a; 64 * 1024];
            for _ in 0..TOTAL / chunk.len() {
                write.write_all(&chunk).await.unwrap();
            }
            write
        });
        let mut buf = vec![0u8; 64 * 1024];
        let mut received = 0;
        while received < TOTAL {
            let n = read.read(&mut buf).await.unwrap();
            assert!(n > 0, "the echo ended early");
            received += n;
        }
        let elapsed = started.elapsed();
        let _write = writer.await.unwrap();
        println!(
            "{} MiB through the tunnel and back in {elapsed:.2?}: {:.1} MiB/s each way",
            TOTAL >> 20,
            (TOTAL >> 20) as f64 / elapsed.as_secs_f64()
        );
    }

    /// The native test: a handshake with the peers, however fresh the
    /// session; its time is the result. It starts the tunnel when need be.
    #[tokio::test]
    async fn the_native_test_is_a_handshake_with_the_peers() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let test = || wg.native_test().expect("wireguard has one");
        let rtt = tokio::time::timeout(Duration::from_secs(5), test())
            .await
            .expect("within 5 s")
            .expect("a handshake");
        assert!(rtt < Duration::from_secs(2), "{rtt:?}");
        let before = peer.core().handshakes;
        tokio::time::timeout(Duration::from_secs(5), test())
            .await
            .expect("within 5 s")
            .expect("a handshake");
        assert!(peer.core().handshakes > before, "the session was fresh");
        peer.go_silent(true);
        let silent = tokio::time::timeout(Duration::from_millis(500), test()).await;
        assert!(silent.is_err(), "no answer, no result");
    }

    /// Two policies that name one section: one tunnel, one handshake.
    #[tokio::test]
    async fn policies_that_name_one_section_share_its_tunnel() {
        let (peer, a) = tunnel(PeerOpts::default(), |_| {}).await;
        let spec = WireGuardSpec {
            section: a.names.section.clone(),
        };
        let b = WireGuardOutbound::new("Other", &spec, no_names(), direct());
        for wg in [&a, &b] {
            let mut stream = wg
                .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
                .await
                .expect("a connection");
            assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        }
        assert_eq!(peer.core().handshakes, 1);
        assert_eq!(peer.clients().len(), 1);
    }

    /// A reload that edits the section: the new tunnel ends the old one and
    /// its connections, and the old configuration does not take it back.
    #[tokio::test]
    async fn a_later_configuration_of_a_tunnel_takes_over() {
        let (peer, old) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = old
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"old").await, b"old");
        let mut section = old.names.section.clone();
        section.mtu = 1400;
        let new = WireGuardOutbound::new("WG", &WireGuardSpec { section }, no_names(), direct());
        let mut fresh = new
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection through the new tunnel");
        assert_eq!(echo(&mut fresh, b"new").await, b"new");
        assert_eq!(peer.core().handshakes, 2);
        let mut buf = [0u8; 8];
        let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf))
            .await
            .expect("the old connection ends at once");
        assert!(read.is_err(), "{read:?}");
        let e = old
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "wireguard: a newer configuration of this tunnel is in use"
        );
        assert_eq!(echo(&mut fresh, b"still").await, b"still");
    }

    /// One key at two peers is two tunnels, side by side.
    #[tokio::test]
    async fn one_key_at_two_peers_is_two_tunnels() {
        let (private, public) = keypair();
        let at_peer = |peer: &FakeWgPeer| {
            let mut s = section(
                private,
                Ipv4Addr::new(10, 9, 0, 2),
                &[(peer.public_key(), &["10.0.0.0/8"])],
            );
            s.peers[0].endpoint = endpoint(peer.addr());
            WireGuardOutbound::new("WG", &WireGuardSpec { section: s }, no_names(), direct())
        };
        let first = FakeWgPeer::start(public, PeerOpts::default()).await;
        let second = FakeWgPeer::start(public, PeerOpts::default()).await;
        let (a, b) = (at_peer(&first), at_peer(&second));
        let mut x = a
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        let mut y = b
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut y, b"y").await, b"y");
        assert_eq!(echo(&mut x, b"x").await, b"x");
    }

    /// Waits until `done` holds, 5 seconds at most.
    async fn until(done: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !done() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("in time");
    }

    /// Names to addresses that a test changes as it goes.
    #[derive(Default)]
    struct Table(StdMutex<Vec<(String, IpAddr)>>);

    impl Table {
        fn set(&self, name: &str, ip: IpAddr) {
            self.0.lock().unwrap().push((name.to_string(), ip));
        }
    }

    impl Resolve for Table {
        fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            let table = self.0.lock().unwrap();
            let found: Vec<IpAddr> = table
                .iter()
                .filter(|(name, _)| name == host)
                .map(|(_, ip)| *ip)
                .collect();
            Box::pin(std::future::ready(if found.is_empty() {
                Err(io::Error::new(io::ErrorKind::NotFound, "no such name"))
            } else {
                Ok(found)
            }))
        }
    }

    /// Dials through `inner` and counts the dials; once `moved` is set, the
    /// carriers say they go there, as if the endpoint's name had come to
    /// point elsewhere.
    struct Moving {
        inner: Arc<dyn Connector>,
        moved: Arc<StdMutex<Option<SocketAddr>>>,
        dials: Arc<AtomicUsize>,
    }

    struct MovingDatagram {
        inner: BoxedDatagram,
        moved: Option<SocketAddr>,
    }

    impl Connector for Moving {
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
                self.dials.fetch_add(1, Ordering::SeqCst);
                let inner = self.inner.connect_udp(target, opts).await?;
                let moved = *self.moved.lock().unwrap();
                Ok(Box::new(MovingDatagram { inner, moved }) as BoxedDatagram)
            })
        }
    }

    impl Datagram for MovingDatagram {
        fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            self.inner.poll_send(cx, buf)
        }

        fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            self.inner.poll_recv(cx, buf)
        }

        fn peer_addr(&self) -> Option<SocketAddr> {
            self.moved.or_else(|| self.inner.peer_addr())
        }
    }

    /// An endpoint written as a name is dialled again every `redial`: the
    /// same address keeps its carrier; another gets a new one, on which the
    /// peer is greeted at once.
    #[tokio::test]
    async fn an_endpoint_written_as_a_name_is_followed() {
        let names = Arc::new(Table::default());
        names.set("wg.test", ip("127.0.0.1"));
        let (moved, dials) = (Arc::new(StdMutex::new(None)), Arc::new(AtomicUsize::new(0)));
        let moving: Arc<dyn Connector> = Arc::new(Moving {
            inner: Arc::new(DirectConnector::new(names)),
            moved: moved.clone(),
            dials: dials.clone(),
        });
        let (peer, mut wg) = tunnel_with(
            PeerOpts::default(),
            |s| s.peers[0].endpoint.host = HostName::parse("wg.test"),
            no_names(),
            moving,
        )
        .await;
        wg.redial = Duration::from_millis(100);
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        until(|| dials.load(Ordering::SeqCst) >= 3).await;
        assert_eq!(echo(&mut stream, b"same").await, b"same");
        assert_eq!(
            peer.clients().len(),
            1,
            "the same address keeps its carrier"
        );

        *moved.lock().unwrap() = Some("127.0.0.1:1".parse().unwrap());
        until(|| peer.clients().len() == 2).await;
        assert_eq!(echo(&mut stream, b"moved").await, b"moved");
        assert_eq!(peer.core().handshakes, 2, "greeted on the new carrier");
    }

    /// A network change: every carrier is dialled anew and each peer
    /// greeted again on its new one; the connections carry on.
    #[tokio::test]
    async fn a_network_change_gives_every_peer_a_new_carrier() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"before").await, b"before");
        wg.network_changed();
        until(|| peer.clients().len() == 2).await;
        assert_eq!(echo(&mut stream, b"after").await, b"after");
        assert_eq!(peer.core().handshakes, 2);
    }

    /// Dials through `inner`; its route is what `route` holds now, as the
    /// member of a group that is the underlying proxy.
    struct Rerouted {
        inner: Arc<dyn Connector>,
        route: Arc<StdMutex<Arc<u8>>>,
    }

    impl Connector for Rerouted {
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
            self.inner.connect_udp(target, opts)
        }

        fn route(&self) -> Option<Route> {
            Some(Route::of(&self.route.lock().unwrap()))
        }
    }

    /// The underlying policy leads elsewhere now (a group's member changed):
    /// every carrier is dialled anew and each peer greeted on its new one;
    /// the connections carry on. The manual: "a change of the effective
    /// policy resets the WireGuard transport sockets".
    #[tokio::test]
    async fn a_new_route_underneath_gives_every_peer_a_new_carrier() {
        let route = Arc::new(StdMutex::new(Arc::new(1u8)));
        let rerouted: Arc<dyn Connector> = Arc::new(Rerouted {
            inner: direct(),
            route: route.clone(),
        });
        let (peer, wg) = tunnel_with(PeerOpts::default(), |_| {}, no_names(), rerouted).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"before").await, b"before");
        // only to see that the same route keeps the carrier
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(peer.clients().len(), 1);
        *route.lock().unwrap() = Arc::new(2u8);
        until(|| peer.clients().len() == 2).await;
        assert_eq!(echo(&mut stream, b"after").await, b"after");
        assert_eq!(peer.core().handshakes, 2);
    }

    /// Dials through `inner` and counts the dials; once `broken` is set,
    /// the first carrier fails every send, as a socket whose network went
    /// away, while the carriers dialled after it work.
    struct Breaking {
        inner: Arc<dyn Connector>,
        broken: Arc<AtomicBool>,
        dials: Arc<AtomicUsize>,
        /// What a send on the broken carrier fails with.
        kind: io::ErrorKind,
        /// How many sends failed.
        failures: Arc<AtomicUsize>,
    }

    struct BreakingDatagram {
        inner: BoxedDatagram,
        /// The switch, on the first carrier only.
        broken: Option<Arc<AtomicBool>>,
        kind: io::ErrorKind,
        failures: Arc<AtomicUsize>,
    }

    impl Connector for Breaking {
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
                let first = self.dials.fetch_add(1, Ordering::SeqCst) == 0;
                let inner = self.inner.connect_udp(target, opts).await?;
                let broken = first.then(|| self.broken.clone());
                Ok(Box::new(BreakingDatagram {
                    inner,
                    broken,
                    kind: self.kind,
                    failures: self.failures.clone(),
                }) as BoxedDatagram)
            })
        }
    }

    impl Datagram for BreakingDatagram {
        fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            if self
                .broken
                .as_ref()
                .is_some_and(|b| b.load(Ordering::SeqCst))
            {
                self.failures.fetch_add(1, Ordering::SeqCst);
                return Poll::Ready(Err(self.kind.into()));
            }
            self.inner.poll_send(cx, buf)
        }

        fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            self.inner.poll_recv(cx, buf)
        }

        fn peer_addr(&self) -> Option<SocketAddr> {
            self.inner.peer_addr()
        }
    }

    /// A carrier that can no longer send (the network changed under it) is
    /// replaced by a new one, even to the same address, and the peer is
    /// greeted on it at once: the next connection goes through.
    #[tokio::test]
    async fn a_carrier_that_fails_to_send_is_replaced() {
        let (broken, dials) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicUsize::new(0)),
        );
        let breaking: Arc<dyn Connector> = Arc::new(Breaking {
            inner: direct(),
            broken: broken.clone(),
            dials: dials.clone(),
            kind: io::ErrorKind::NetworkUnreachable,
            failures: Arc::new(AtomicUsize::new(0)),
        });
        let (peer, wg) = tunnel_with(PeerOpts::default(), |_| {}, no_names(), breaking).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"before").await, b"before");

        broken.store(true, Ordering::SeqCst);
        let mut fresh = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection over a new carrier");
        assert_eq!(echo(&mut fresh, b"after").await, b"after");
        assert_eq!(dials.load(Ordering::SeqCst), 2, "a second carrier");
        assert_eq!(peer.core().handshakes, 2, "the peer was greeted on it");
    }

    /// A send that fails for a passing reason (a full send buffer, say)
    /// keeps the carrier: the datagram is lost, TCP sends it again, and no
    /// new carrier is dialled.
    #[tokio::test]
    async fn a_passing_send_error_keeps_the_carrier() {
        let (broken, dials, failures) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        );
        let breaking: Arc<dyn Connector> = Arc::new(Breaking {
            inner: direct(),
            broken: broken.clone(),
            dials: dials.clone(),
            kind: io::ErrorKind::Other,
            failures: failures.clone(),
        });
        let (peer, wg) = tunnel_with(PeerOpts::default(), |_| {}, no_names(), breaking).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"before").await, b"before");

        broken.store(true, Ordering::SeqCst);
        stream.write_all(b"after").await.unwrap();
        until(|| failures.load(Ordering::SeqCst) > 0).await;
        broken.store(false, Ordering::SeqCst);
        let mut back = [0u8; 5];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut back))
            .await
            .expect("sent again in time")
            .expect("the echo");
        assert_eq!(&back, b"after");
        assert_eq!(dials.load(Ordering::SeqCst), 1, "the same carrier");
        assert_eq!(peer.core().handshakes, 1);
    }

    /// Dials through `inner`; once `dead` is set, the first carrier fails
    /// every receive with `BrokenPipe`, as a chain's closed association
    /// does, and counts the polls.
    struct Dying {
        inner: Arc<dyn Connector>,
        dead: Arc<AtomicBool>,
        dials: Arc<AtomicUsize>,
        polls: Arc<AtomicUsize>,
        /// How many of the first carriers die.
        dying: usize,
    }

    struct DyingDatagram {
        inner: BoxedDatagram,
        /// The switch and the poll counter, on the first carrier only.
        first: Option<(Arc<AtomicBool>, Arc<AtomicUsize>)>,
    }

    impl Connector for Dying {
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
                let first = self.dials.fetch_add(1, Ordering::SeqCst) < self.dying;
                let inner = self.inner.connect_udp(target, opts).await?;
                let first = first.then(|| (self.dead.clone(), self.polls.clone()));
                Ok(Box::new(DyingDatagram { inner, first }) as BoxedDatagram)
            })
        }
    }

    impl Datagram for DyingDatagram {
        fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            self.inner.poll_send(cx, buf)
        }

        fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            if let Some((dead, polls)) = &self.first
                && dead.load(Ordering::SeqCst)
            {
                polls.fetch_add(1, Ordering::SeqCst);
                return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
            }
            self.inner.poll_recv(cx, buf)
        }

        fn peer_addr(&self) -> Option<SocketAddr> {
            self.inner.peer_addr()
        }
    }

    /// A carrier whose receiving has ended for good is dropped, not polled
    /// again and again, and dialled anew: the tunnel carries on over the
    /// new one.
    #[tokio::test]
    async fn a_carrier_that_has_ended_is_replaced_and_not_spun_on() {
        let (dead, dials, polls) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        );
        let dying: Arc<dyn Connector> = Arc::new(Dying {
            inner: direct(),
            dead: dead.clone(),
            dials: dials.clone(),
            polls: polls.clone(),
            dying: 1,
        });
        let (peer, wg) = tunnel_with(PeerOpts::default(), |_| {}, no_names(), dying).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"before").await, b"before");

        dead.store(true, Ordering::SeqCst);
        // traffic wakes the driver, which finds the carrier ended
        let mut fresh = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection over a new carrier");
        assert_eq!(echo(&mut fresh, b"after").await, b"after");
        assert_eq!(dials.load(Ordering::SeqCst), 2, "a second carrier");
        assert_eq!(peer.core().handshakes, 2, "the peer was greeted on it");
        assert!(
            polls.load(Ordering::SeqCst) < 20,
            "the ended carrier was polled {} times",
            polls.load(Ordering::SeqCst)
        );
    }

    /// A replacement that ends right away too is not dropped by the
    /// ten-second limit: it is dialled again when the limit has run out,
    /// long before the periodic redial.
    #[tokio::test]
    async fn a_replacement_that_ends_at_once_is_retried_after_the_limit() {
        let (dead, dials) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicUsize::new(0)),
        );
        let dying: Arc<dyn Connector> = Arc::new(Dying {
            inner: direct(),
            dead: dead.clone(),
            dials: dials.clone(),
            polls: Arc::new(AtomicUsize::new(0)),
            dying: 2,
        });
        let (_peer, wg) = tunnel_with(PeerOpts::default(), |_| {}, no_names(), dying).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"before").await, b"before");

        dead.store(true, Ordering::SeqCst);
        // traffic wakes the driver: the first carrier ends, and so does
        // its replacement
        let _ = wg.connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(1)).await;
        tokio::time::timeout(Duration::from_secs(30), async {
            while dials.load(Ordering::SeqCst) < 3 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("a third carrier long before the periodic redial");
        let mut fresh = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection over the third carrier");
        assert_eq!(echo(&mut fresh, b"after").await, b"after");
    }

    /// A peer that cannot be reached when the tunnel starts is dialled
    /// again every `redial`; the others carry on meanwhile.
    #[tokio::test]
    async fn a_peer_unreachable_at_the_start_is_dialled_again() {
        let (private, public) = keypair();
        let a = FakeWgPeer::start(public, PeerOpts::default()).await;
        let b = FakeWgPeer::start(public, PeerOpts::default()).await;
        let mut section = section(
            private,
            Ipv4Addr::new(10, 9, 0, 2),
            &[
                (a.public_key(), &["10.1.0.0/16"]),
                (b.public_key(), &["10.2.0.0/16"]),
            ],
        );
        section.peers[0].endpoint = endpoint(a.addr());
        section.peers[1].endpoint = PeerEndpoint {
            host: HostName::parse("b.test"),
            port: b.addr().port(),
        };
        let names = Arc::new(Table::default());
        let connector: Arc<dyn Connector> = Arc::new(DirectConnector::new(names.clone()));
        let mut wg =
            WireGuardOutbound::new("WG", &WireGuardSpec { section }, no_names(), connector);
        wg.redial = Duration::from_millis(100);
        let mut stream = wg
            .connect_tcp(&at("10.1.0.1", ECHO_PORT), &within(5))
            .await
            .expect("through the peer that can be reached");
        assert_eq!(echo(&mut stream, b"a").await, b"a");

        names.set("b.test", ip("127.0.0.1"));
        let mut stream = wg
            .connect_tcp(&at("10.2.0.1", ECHO_PORT), &within(5))
            .await
            .expect("through the other, once it can be reached");
        assert_eq!(echo(&mut stream, b"b").await, b"b");
        assert_eq!(b.core().accepted, 1);
    }

    /// A connector with no UDP carrier, as if a chain stood in the way of
    /// it (`Connector::connect_udp`'s default `Unsupported`, M4-D7 / P17).
    struct NoUdp;

    impl Connector for NoUdp {
        fn connect<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            Box::pin(std::future::ready(Err(io::Error::other("not used"))))
        }
    }

    /// A policy over a chain never shares a direct policy's tunnel, even
    /// naming the same section: sharing also requires an equal carrier
    /// (P10), so it dials its own carriers, which it cannot, and fails
    /// with what the chain said — the other policy's tunnel keeps running
    /// untouched.
    #[tokio::test]
    async fn a_policy_over_a_chain_never_shares_a_direct_tunnel() {
        let (peer, a) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = a
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        let spec = WireGuardSpec {
            section: a.names.section.clone(),
        };
        let b = WireGuardOutbound::new("Chained", &spec, no_names(), Arc::new(NoUdp))
            .with_carrier("chain".to_string());
        let e = b
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(e, OutboundError::Io(_)), "{e}");
        assert_eq!(e.to_string(), "this connection cannot carry UDP");
        assert_eq!(echo(&mut stream, b"still").await, b"still");
        assert_eq!(peer.core().handshakes, 1);
    }

    /// A reload that only changes the policy's carrier (`ip-version`,
    /// `underlying-proxy`, `[General] ipv6`) — the section itself is
    /// unchanged — still takes the tunnel over, exactly as a changed
    /// section does (P10).
    #[tokio::test]
    async fn a_policy_whose_carriers_changed_takes_the_tunnel_over() {
        let (peer, old) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = old
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"old").await, b"old");
        let spec = WireGuardSpec {
            section: old.names.section.clone(),
        };
        let new =
            WireGuardOutbound::new("WG", &spec, no_names(), direct()).with_carrier("v6".into());
        let mut fresh = new
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection through the new tunnel");
        assert_eq!(echo(&mut fresh, b"new").await, b"new");
        assert_eq!(peer.core().handshakes, 2);
        let mut buf = [0u8; 8];
        let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf))
            .await
            .expect("the old connection ends at once");
        assert!(read.is_err(), "{read:?}");
    }

    /// A connector whose `connect_udp` records that it was asked, then
    /// never completes — as if resolving or dialling an endpoint hung.
    struct Stuck {
        asked: Arc<AtomicBool>,
    }

    impl Connector for Stuck {
        fn connect<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            Box::pin(std::future::ready(Err(io::Error::other("not used"))))
        }

        fn connect_udp<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            self.asked.store(true, Ordering::SeqCst);
            Box::pin(std::future::pending())
        }
    }

    /// A tunnel that is only starting, its dial stuck, never blocks a dial
    /// that only shares an already-running tunnel of another section: no
    /// global start lock (P10).
    #[tokio::test]
    async fn a_running_tunnel_is_shared_while_another_starts() {
        let (peer, a1) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut x = a1
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut x, b"a1").await, b"a1");

        let (other_private, other_public) = keypair();
        let other = section(
            other_private,
            Ipv4Addr::new(10, 9, 0, 3),
            &[(other_public, &["10.3.0.0/16"])],
        );
        let asked = Arc::new(AtomicBool::new(false));
        let stuck: Arc<dyn Connector> = Arc::new(Stuck {
            asked: asked.clone(),
        });
        let b = WireGuardOutbound::new(
            "Stuck",
            &WireGuardSpec { section: other },
            no_names(),
            stuck,
        );
        let stuck_dial = tokio::spawn(async move {
            let _ = b.connect_tcp(&at("10.3.0.1", ECHO_PORT), &within(30)).await;
        });
        until(|| asked.load(Ordering::SeqCst)).await;

        let spec = WireGuardSpec {
            section: a1.names.section.clone(),
        };
        let a2 = WireGuardOutbound::new("A2", &spec, no_names(), direct());
        let mut y = a2
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(2))
            .await
            .expect("shares the running tunnel without waiting on the other's dial");
        assert_eq!(echo(&mut y, b"a2").await, b"a2");
        assert_eq!(peer.core().handshakes, 1);

        stuck_dial.abort();
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    /// Datagrams through the tunnel to the peer's UDP echo and back, from
    /// the tunnel's address: one socket for every destination.
    #[tokio::test]
    async fn udp_goes_through_the_tunnel() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        assert_eq!(wg.udp(), UdpSupport::Native);
        let carrier = wg.open_udp(&within(5)).await.expect("a carrier");
        for (host, payload) in [("10.0.0.1", &b"ping"[..]), ("10.0.0.2", b"pong")] {
            let to = at(host, ECHO_PORT);
            carrier.send_to(payload, &to).await.unwrap();
            assert_eq!(udp_answer(carrier.as_ref()).await, (payload.to_vec(), to));
        }
        let echoed = peer.core().udp_echoed.clone();
        assert_eq!(echoed.len(), 2);
        assert_eq!(echoed[0].0.ip(), IpAddr::V4(Ipv4Addr::new(10, 9, 0, 2)));
        assert_eq!(echoed[0].0, echoed[1].0, "one socket for both");
        assert_eq!(
            (echoed[0].1, echoed[1].1),
            ("10.0.0.1:7".parse().unwrap(), "10.0.0.2:7".parse().unwrap())
        );
    }

    /// Full cone: a host behind the peer that was never written to reaches
    /// the carrier, under its own address.
    #[tokio::test]
    async fn anyone_in_the_tunnel_may_answer() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let carrier = wg.open_udp(&within(5)).await.expect("a carrier");
        carrier
            .send_to(b"hello", &at("10.0.0.1", ECHO_PORT))
            .await
            .unwrap();
        udp_answer(carrier.as_ref()).await;
        let SocketAddr::V4(ours) = peer.core().udp_echoed[0].0 else {
            panic!("the tunnel is IPv4")
        };
        let stranger: SocketAddrV4 = "10.0.0.9:5000".parse().unwrap();
        peer.send_udp(stranger, ours, b"unasked").await;
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), at("10.0.0.9", 5000))
        );
    }

    /// A name is looked up as for a connection (here on this machine), and
    /// the datagram goes to what it gave.
    #[tokio::test]
    async fn a_datagram_to_a_name_goes_where_the_name_says() {
        let names = Arc::new(Names(vec![(
            "echo.test",
            vec!["10.0.0.1".parse().unwrap()],
        )]));
        let (_peer, wg) = tunnel_with(PeerOpts::default(), |_| {}, names, direct()).await;
        let carrier = wg.open_udp(&within(5)).await.expect("a carrier");
        let name = at("echo.test", ECHO_PORT);
        assert_eq!(
            carrier.resolve(&name).await.unwrap(),
            at("10.0.0.1", ECHO_PORT)
        );
        carrier.send_to(b"q", &name).await.unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"q".to_vec(), at("10.0.0.1", ECHO_PORT))
        );
    }

    /// A carrier of a tunnel that a later configuration replaced fails to
    /// receive and to send, at once, instead of going quiet.
    #[tokio::test]
    async fn a_carrier_of_a_replaced_tunnel_fails() {
        let (_peer, old) = tunnel(PeerOpts::default(), |_| {}).await;
        let carrier: Arc<dyn PacketSocket> =
            Arc::from(old.open_udp(&within(5)).await.expect("a carrier"));
        let to = at("10.0.0.1", ECHO_PORT);
        carrier.send_to(b"hello", &to).await.unwrap();
        udp_answer(carrier.as_ref()).await;
        let waiting = tokio::spawn({
            let carrier = carrier.clone();
            async move { carrier.recv_from(&mut [0u8; 64]).await.map(|_| ()) }
        });
        let mut section = old.names.section.clone();
        section.mtu = 1400;
        let new = WireGuardOutbound::new("WG", &WireGuardSpec { section }, no_names(), direct());
        new.connect_tcp(&to, &within(5))
            .await
            .expect("a connection");
        let err = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the receive ends at once")
            .unwrap()
            .unwrap_err();
        assert_eq!(err.to_string(), "wireguard: the tunnel has closed");
        let err = tokio::time::timeout(Duration::from_secs(5), carrier.send_to(b"x", &to))
            .await
            .expect("the send ends at once")
            .unwrap_err();
        assert_eq!(err.to_string(), "wireguard: the tunnel has closed");
    }

    /// A destination no peer takes, or of a family the tunnel has no address
    /// of, is refused at once, saying why.
    #[tokio::test]
    async fn udp_where_the_tunnel_cannot_go_is_refused() {
        let (_peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let carrier = wg.open_udp(&within(5)).await.expect("a carrier");
        for (host, text) in [
            (
                "192.168.1.1",
                "wireguard: no peer's allowed-ips covers 192.168.1.1",
            ),
            ("fd00::1", "wireguard: the tunnel has no IPv6 address"),
        ] {
            let err = carrier.send_to(b"x", &at(host, 53)).await.unwrap_err();
            assert_eq!(err.to_string(), text);
        }
    }

    const SUPERSEDED: &str = "wireguard: a newer configuration of this tunnel is in use";
    const CLOSED: &str = "wireguard: the tunnel has closed";

    /// Dials through `inner`; once `armed` is set, the next receive on one
    /// of its carriers panics, and the device's task with it. Once only: a
    /// carrier dialled after that works.
    struct Fragile {
        inner: Arc<dyn Connector>,
        armed: Arc<AtomicBool>,
    }

    struct FragileDatagram {
        inner: BoxedDatagram,
        armed: Arc<AtomicBool>,
    }

    impl Connector for Fragile {
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
                let armed = self.armed.clone();
                Ok(Box::new(FragileDatagram { inner, armed }) as BoxedDatagram)
            })
        }
    }

    impl Datagram for FragileDatagram {
        fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            self.inner.poll_send(cx, buf)
        }

        fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            if self.armed.swap(false, Ordering::SeqCst) {
                panic!("a carrier that panics when polled (a test's)");
            }
            self.inner.poll_recv(cx, buf)
        }

        fn peer_addr(&self) -> Option<SocketAddr> {
            self.inner.peer_addr()
        }
    }

    /// A tunnel as `tunnel` gives, over `Fragile` carriers, and the switch
    /// that kills its task.
    async fn fragile_tunnel(
        opts: PeerOpts,
        edit: impl FnOnce(&mut WireGuardSection),
    ) -> (FakeWgPeer, WireGuardOutbound, Arc<AtomicBool>) {
        let armed = Arc::new(AtomicBool::new(false));
        let fragile: Arc<dyn Connector> = Arc::new(Fragile {
            inner: direct(),
            armed: armed.clone(),
        });
        let (peer, wg) = tunnel_with(opts, edit, no_names(), fragile).await;
        (peer, wg, armed)
    }

    /// The task of the tunnel whose carriers `armed` belongs to dies of a
    /// panic.
    async fn dies(armed: &AtomicBool) {
        armed.store(true, Ordering::SeqCst);
        // on the tests' current-thread runtime the task has unwound, and
        // ended the tunnel, by the time this one sees the switch taken; on
        // a multi-thread one the switch could be seen before the end
        until(|| !armed.load(Ordering::SeqCst)).await;
    }

    /// A later configuration of `old`'s tunnel, with its own tunnel started:
    /// `old`'s is over.
    async fn superseding(old: &WireGuardOutbound) -> WireGuardOutbound {
        let mut section = old.names.section.clone();
        section.mtu = 1400;
        let new = WireGuardOutbound::new("WG", &WireGuardSpec { section }, no_names(), direct());
        new.device(&within(5))
            .await
            .expect("the later configuration's tunnel starts");
        new
    }

    /// The device's task died — here of a panic: the next connection starts
    /// a new tunnel instead of waiting on the dead one, and the connections
    /// the dead one carried end.
    #[tokio::test]
    async fn a_tunnel_whose_task_died_is_replaced_by_the_next_connection() {
        let (peer, wg, armed) = fragile_tunnel(PeerOpts::default(), |_| {}).await;
        let to = at("10.0.0.1", ECHO_PORT);
        let mut stream = wg.connect_tcp(&to, &within(5)).await.expect("a connection");
        assert_eq!(echo(&mut stream, b"before").await, b"before");
        assert_eq!(peer.core().handshakes, 1);

        dies(&armed).await;
        let mut fresh = wg
            .connect_tcp(&to, &within(5))
            .await
            .expect("a connection through a new tunnel");
        assert_eq!(echo(&mut fresh, b"after").await, b"after");
        assert_eq!(peer.core().handshakes, 2, "a new tunnel greeted the peer");
        let mut buf = [0u8; 8];
        let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf))
            .await
            .expect("the dead tunnel's connection ends at once");
        assert!(read.is_err(), "{read:?}");
    }

    /// A panic with the stack's lock held leaves the lock poisoned. The call
    /// that finds it so fails, saying the tunnel has closed, instead of
    /// panicking in its turn; the next connection starts a new tunnel.
    #[tokio::test]
    async fn a_poisoned_stack_fails_the_call_and_the_next_connection_starts_a_new_tunnel() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let to = at("10.0.0.1", ECHO_PORT);
        let mut stream = wg.connect_tcp(&to, &within(5)).await.expect("a connection");
        assert_eq!(echo(&mut stream, b"before").await, b"before");

        wg.device(&within(5)).await.unwrap().poison();
        // on the tests' current-thread runtime nothing runs between the
        // poisoning and this call, which is the first to find it; on a
        // multi-thread one the device's task could find it first, and this
        // call would start a new tunnel instead of failing
        let e = wg
            .connect_tcp(&to, &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(e.to_string(), CLOSED);
        let mut fresh = wg
            .connect_tcp(&to, &within(5))
            .await
            .expect("a connection through a new tunnel");
        assert_eq!(echo(&mut fresh, b"after").await, b"after");
        assert_eq!(peer.core().handshakes, 2, "a new tunnel greeted the peer");
    }

    /// Whoever holds a tunnel whose stack's lock is poisoned — a connection,
    /// a UDP carrier, a call on the device — gets an error, not a panic;
    /// and letting go of it does not panic either.
    #[tokio::test]
    async fn nothing_panics_on_a_poisoned_stack() {
        let (_peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let to = at("10.0.0.1", ECHO_PORT);
        let mut stream = wg.connect_tcp(&to, &within(5)).await.expect("a connection");
        assert_eq!(echo(&mut stream, b"before").await, b"before");
        let carrier = wg.open_udp(&within(5)).await.expect("a carrier");
        carrier.send_to(b"before", &to).await.unwrap();
        udp_answer(carrier.as_ref()).await;
        let device = wg.device(&within(5)).await.unwrap();

        device.poison();
        let read = stream.read(&mut [0u8; 8]).await.unwrap_err();
        assert_eq!(read.to_string(), CLOSED);
        let written = stream.write(b"x").await.unwrap_err();
        assert_eq!(written.to_string(), CLOSED);
        let shut = stream.shutdown().await.unwrap_err();
        assert_eq!(shut.to_string(), CLOSED);
        let sent = carrier.send_to(b"x", &to).await.unwrap_err();
        assert_eq!(sent.to_string(), CLOSED);
        let received = carrier.recv_from(&mut [0u8; 64]).await.unwrap_err();
        assert_eq!(received.to_string(), CLOSED);
        assert_eq!(refusals(&device).await, [CLOSED; 4]);
        drop((stream, carrier, device));
    }

    /// What each of the device's waits fails with on a tunnel that is over:
    /// each within a second.
    async fn refusals(device: &Arc<Device>) -> [String; 4] {
        let to: SocketAddr = "10.0.0.1:7".parse().unwrap();
        let at_once = Duration::from_secs(1);
        let connect = tokio::time::timeout(at_once, device.connect(to))
            .await
            .expect("the connection fails at once")
            .map(|_| ())
            .unwrap_err();
        let exchange = device.query(to, b"?", Duration::from_secs(30), |_| Some(()));
        let query = tokio::time::timeout(at_once, exchange)
            .await
            .expect("the exchange fails at once")
            .unwrap_err();
        let handshake = tokio::time::timeout(at_once, device.handshake())
            .await
            .expect("the handshake fails at once")
            .unwrap_err();
        let established = tokio::time::timeout(at_once, device.established())
            .await
            .expect("the wait for the first handshake fails at once")
            .unwrap_err();
        [connect, query, handshake, established].map(|e| e.to_string())
    }

    /// A call that got the device before the tunnel was over, and comes to
    /// it after, fails at once: nothing on a tunnel that is over would ever
    /// answer it. It says why the tunnel is over.
    #[tokio::test]
    async fn a_call_on_a_tunnel_that_is_over_fails_at_once() {
        let (_peer, old) = tunnel(PeerOpts::default(), |_| {}).await;
        let device = old.device(&within(5)).await.unwrap();
        let _new = superseding(&old).await;
        assert_eq!(refusals(&device).await, [SUPERSEDED; 4]);

        let (_peer, wg, armed) = fragile_tunnel(PeerOpts::default(), |_| {}).await;
        let device = wg.device(&within(5)).await.unwrap();
        dies(&armed).await;
        assert_eq!(refusals(&device).await, [CLOSED; 4]);
    }

    /// Where a call through the outbound waits on the device.
    #[derive(Clone, Copy)]
    enum Wait {
        /// `Device::connect`: the dial of an address.
        Connect,
        /// `Device::query`: the dial of a name, its question out.
        Query,
        /// `Device::handshake`: the native test.
        Handshake,
        /// `Device::established`: the dial of a name, before its question.
        Established,
    }

    /// A call that waits on the device, on a tunnel whose peer never lets
    /// it go on.
    struct Parked {
        peer: FakeWgPeer,
        wg: Arc<WireGuardOutbound>,
        /// Kills the tunnel's task (`Fragile`).
        armed: Arc<AtomicBool>,
        call: tokio::task::JoinHandle<Result<(), OutboundError>>,
    }

    async fn parked(wait: Wait) -> Parked {
        // the name server never answers; for the other waits, the handshake
        // never completes
        let opts = match wait {
            Wait::Query => PeerOpts {
                dns_ignore: usize::MAX,
                ..PeerOpts::default()
            },
            _ => PeerOpts {
                handshake_ignore: usize::MAX,
                ..PeerOpts::default()
            },
        };
        let by_name = matches!(wait, Wait::Query | Wait::Established);
        let (peer, mut wg, armed) = fragile_tunnel(opts, |s| {
            if by_name {
                s.dns_servers = vec![TunnelDns::Server(SocketAddr::new(DNS_ADDRESS.into(), 53))];
            }
        })
        .await;
        // no round of the lookup runs out while the test waits
        wg.set_dns_wait(Duration::from_secs(30));
        let wg = Arc::new(wg);
        let call = tokio::spawn({
            let wg = wg.clone();
            async move {
                let host = if by_name { "echo.test" } else { "10.0.0.1" };
                match wait {
                    Wait::Handshake => {
                        let test = wg.native_test().expect("wireguard has one");
                        test.await.map(|_| ())
                    }
                    _ => wg
                        .connect_tcp(&at(host, ECHO_PORT), &within(30))
                        .await
                        .map(|_| ()),
                }
            }
        });
        // the peer heard of it: the call is where it waits
        until(|| match wait {
            Wait::Query => !peer.core().dns_questions.is_empty(),
            _ => !peer.core().reserved.is_empty(),
        })
        .await;
        Parked {
            peer,
            wg,
            armed,
            call,
        }
    }

    /// What a parked call fails with once its tunnel is over: within a
    /// second, not when its own time runs out.
    async fn failure(call: tokio::task::JoinHandle<Result<(), OutboundError>>) -> String {
        tokio::time::timeout(Duration::from_secs(1), call)
            .await
            .expect("the call ends at once")
            .expect("the call does not panic")
            .unwrap_err()
            .to_string()
    }

    #[tokio::test]
    async fn a_dial_waiting_to_connect_ends_when_a_later_configuration_takes_over() {
        let parked = parked(Wait::Connect).await;
        let _new = superseding(&parked.wg).await;
        assert_eq!(failure(parked.call).await, SUPERSEDED);
    }

    #[tokio::test]
    async fn a_lookup_waiting_for_its_answer_ends_when_a_later_configuration_takes_over() {
        let parked = parked(Wait::Query).await;
        let _new = superseding(&parked.wg).await;
        assert_eq!(failure(parked.call).await, SUPERSEDED);
    }

    #[tokio::test]
    async fn a_native_test_waiting_for_the_handshake_ends_when_a_later_configuration_takes_over() {
        let parked = parked(Wait::Handshake).await;
        let _new = superseding(&parked.wg).await;
        assert_eq!(failure(parked.call).await, SUPERSEDED);
    }

    #[tokio::test]
    async fn a_lookup_waiting_for_the_first_handshake_ends_when_a_later_configuration_takes_over() {
        let parked = parked(Wait::Established).await;
        let _new = superseding(&parked.wg).await;
        assert_eq!(failure(parked.call).await, SUPERSEDED);
    }

    #[tokio::test]
    async fn a_dial_waiting_to_connect_ends_when_the_tunnel_closes() {
        let parked = parked(Wait::Connect).await;
        dies(&parked.armed).await;
        assert_eq!(failure(parked.call).await, CLOSED);
    }

    #[tokio::test]
    async fn a_lookup_waiting_for_its_answer_ends_when_the_tunnel_closes() {
        let parked = parked(Wait::Query).await;
        dies(&parked.armed).await;
        assert_eq!(failure(parked.call).await, CLOSED);
    }

    #[tokio::test]
    async fn a_native_test_waiting_for_the_handshake_ends_when_the_tunnel_closes() {
        let parked = parked(Wait::Handshake).await;
        dies(&parked.armed).await;
        assert_eq!(failure(parked.call).await, CLOSED);
    }

    #[tokio::test]
    async fn a_lookup_waiting_for_the_first_handshake_ends_when_the_tunnel_closes() {
        let parked = parked(Wait::Established).await;
        dies(&parked.armed).await;
        assert_eq!(failure(parked.call).await, CLOSED);
    }

    /// Reloads S → S′ → S while a connection keeps the first tunnel running:
    /// the last configuration shares that tunnel, which is its own from
    /// then on — what is left of S′ does not take it over.
    #[tokio::test]
    async fn a_shared_tunnel_belongs_to_the_latest_configuration_that_shares_it() {
        let (peer, first) = tunnel(PeerOpts::default(), |_| {}).await;
        let to = at("10.0.0.1", ECHO_PORT);
        let mut kept = first
            .connect_tcp(&to, &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut kept, b"first").await, b"first");
        // the outbounds of the two reloads, in the order they were built
        let section = first.names.section.clone();
        let mut edited = section.clone();
        edited.mtu = 1400;
        let edited = WireGuardSpec { section: edited };
        let between = WireGuardOutbound::new("WG", &edited, no_names(), direct());
        let last = WireGuardOutbound::new("WG", &WireGuardSpec { section }, no_names(), direct());
        // only the connection holds the first tunnel now
        drop(first);
        let mut current = last
            .connect_tcp(&to, &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut current, b"last").await, b"last");
        assert_eq!(peer.core().handshakes, 1, "the tunnel is shared");

        let e = between
            .connect_tcp(&to, &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(e.to_string(), SUPERSEDED);
        assert_eq!(echo(&mut kept, b"kept").await, b"kept");
        assert_eq!(echo(&mut current, b"current").await, b"current");
        assert_eq!(peer.core().handshakes, 1);
    }

    /// Dials through `inner` once `open` lets it: a dial that takes its
    /// time.
    struct Gated {
        inner: Arc<dyn Connector>,
        asked: Arc<AtomicBool>,
        open: Arc<tokio::sync::Notify>,
    }

    impl Connector for Gated {
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
                self.asked.store(true, Ordering::SeqCst);
                self.open.notified().await;
                self.inner.connect_udp(target, opts).await
            })
        }
    }

    /// The same when the last configuration finds the tunnel only after
    /// dialling its own carriers — another policy of the section started it
    /// meanwhile: the tunnel it shares then is its own too.
    #[tokio::test]
    async fn a_tunnel_shared_after_dialling_belongs_to_the_later_configuration_too() {
        let (peer, first) = tunnel(PeerOpts::default(), |_| {}).await;
        let to = at("10.0.0.1", ECHO_PORT);
        let section = first.names.section.clone();
        let mut edited = section.clone();
        edited.mtu = 1400;
        let edited = WireGuardSpec { section: edited };
        let between = WireGuardOutbound::new("WG", &edited, no_names(), direct());
        let (asked, open) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(tokio::sync::Notify::new()),
        );
        let gated: Arc<dyn Connector> = Arc::new(Gated {
            inner: direct(),
            asked: asked.clone(),
            open: open.clone(),
        });
        let last = Arc::new(WireGuardOutbound::new(
            "WG",
            &WireGuardSpec { section },
            no_names(),
            gated,
        ));
        // the last configuration's dial is under way, no tunnel to share yet
        let dialling = tokio::spawn({
            let (last, to) = (last.clone(), to.clone());
            async move { last.connect_tcp(&to, &within(5)).await }
        });
        until(|| asked.load(Ordering::SeqCst)).await;
        let mut kept = first
            .connect_tcp(&to, &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut kept, b"first").await, b"first");
        open.notify_one();
        let mut current = dialling.await.unwrap().expect("a connection");
        assert_eq!(echo(&mut current, b"last").await, b"last");
        assert_eq!(peer.core().handshakes, 1, "the tunnel is shared");

        let e = between
            .connect_tcp(&to, &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(e.to_string(), SUPERSEDED);
        assert_eq!(echo(&mut kept, b"kept").await, b"kept");
        assert_eq!(echo(&mut current, b"current").await, b"current");
        assert_eq!(peer.core().handshakes, 1);
    }

    /// A receive on `carrier` in a task of its own, once it has been
    /// polled: from here on only a wake-up ends it.
    async fn receiving(carrier: BoxedPacketSocket) -> tokio::task::JoinHandle<io::Result<()>> {
        let polled = Arc::new(AtomicBool::new(false));
        let receive = tokio::spawn({
            let polled = polled.clone();
            async move {
                let mut buf = [0u8; 64];
                let mut receive = carrier.recv_from(&mut buf);
                std::future::poll_fn(|cx| {
                    let received = receive.as_mut().poll(cx);
                    polled.store(true, Ordering::SeqCst);
                    received
                })
                .await
                .map(|_| ())
            }
        });
        until(|| polled.load(Ordering::SeqCst)).await;
        receive
    }

    /// What a receive that waits ends with once its tunnel is over: within
    /// a second.
    async fn ended(receive: tokio::task::JoinHandle<io::Result<()>>) -> String {
        tokio::time::timeout(Duration::from_secs(1), receive)
            .await
            .expect("the receive ends at once")
            .expect("the receive does not panic")
            .unwrap_err()
            .to_string()
    }

    /// A UDP carrier that has sent nothing has no socket of the stack to
    /// wait on: its receive is woken all the same when a later
    /// configuration takes the tunnel over. A carrier says that the tunnel
    /// has closed, whatever ended it.
    #[tokio::test]
    async fn a_carrier_that_has_sent_nothing_is_woken_when_a_later_configuration_takes_over() {
        let (_peer, old) = tunnel(PeerOpts::default(), |_| {}).await;
        let carrier = old.open_udp(&within(5)).await.expect("a carrier");
        let receive = receiving(carrier).await;
        let _new = superseding(&old).await;
        assert_eq!(ended(receive).await, CLOSED);
    }

    /// And when the tunnel closes: its task died.
    #[tokio::test]
    async fn a_carrier_that_has_sent_nothing_is_woken_when_the_tunnel_closes() {
        let (_peer, wg, armed) = fragile_tunnel(PeerOpts::default(), |_| {}).await;
        let carrier = wg.open_udp(&within(5)).await.expect("a carrier");
        let receive = receiving(carrier).await;
        dies(&armed).await;
        assert_eq!(ended(receive).await, CLOSED);
    }

    /// Panics when woken (a test's): a panic inside whoever wakes it.
    struct Bomb;

    impl std::task::Wake for Bomb {
        fn wake(self: Arc<Self>) {
            panic!("a waker that panics when woken (a test's)");
        }
    }

    /// A panic inside `send_to` — here of the waker it wakes when the
    /// carrier's first socket opens — leaves the stack's lock and the
    /// carrier's own two poisoned. The carrier then reports the tunnel's
    /// end, as everyone on that tunnel does, instead of panicking on its
    /// own locks; and it can be let go of.
    #[tokio::test]
    async fn a_carrier_whose_send_panicked_reports_the_tunnels_end() {
        let (_peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let device = wg.device(&within(5)).await.unwrap();
        let carrier = Arc::new(TunnelUdp::new(device.clone(), wg.names.clone()));
        let to = at("10.0.0.1", ECHO_PORT);
        // a receiver whose waker panics: the send that opens the first
        // socket wakes it, with the three locks held
        let bomb = Waker::from(Arc::new(Bomb));
        let mut buf = [0u8; 64];
        let waiting = carrier
            .recv_from(&mut buf)
            .as_mut()
            .poll(&mut Context::from_waker(&bomb));
        assert!(waiting.is_pending());
        let sending = tokio::spawn({
            let (carrier, to) = (carrier.clone(), to.clone());
            async move { carrier.send_to(b"x", &to).await }
        });
        assert!(sending.await.unwrap_err().is_panic(), "the send panicked");
        assert!(device.is_poisoned(), "with the stack's lock held");

        let at_once = Duration::from_secs(1);
        let received = tokio::time::timeout(at_once, carrier.recv_from(&mut [0u8; 64]))
            .await
            .expect("the receive ends")
            .unwrap_err();
        assert_eq!(received.to_string(), CLOSED);
        let sent = tokio::time::timeout(at_once, carrier.send_to(b"x", &to))
            .await
            .expect("the send ends")
            .unwrap_err();
        assert_eq!(sent.to_string(), CLOSED);
        // the last holder: the carrier's own `Drop` runs here
        drop(carrier);
    }

    /// The device's task itself panics while it holds the stack's lock —
    /// here in the waker of a connection's reader, which the stack wakes as
    /// the data arrives: unwinding leaves the lock poisoned, and the task's
    /// end ends the tunnel all the same. Whoever waits on it is told that
    /// it has closed, and the next connection starts another.
    #[tokio::test]
    async fn a_task_that_panics_with_the_stack_locked_ends_the_tunnel_all_the_same() {
        // a dial of a name, waiting for an answer its name server never gives
        let parked = parked(Wait::Query).await;
        let to = at("10.0.0.1", ECHO_PORT);
        let mut stream = parked
            .wg
            .connect_tcp(&to, &within(5))
            .await
            .expect("a connection");
        let device = parked.wg.device(&within(5)).await.unwrap();
        let bomb = Waker::from(Arc::new(Bomb));
        let mut buf = [0u8; 8];
        let reading = Pin::new(&mut stream)
            .poll_read(&mut Context::from_waker(&bomb), &mut ReadBuf::new(&mut buf));
        assert!(reading.is_pending());
        // its echo is what the stack wakes the reader for
        stream.write_all(b"x").await.unwrap();
        assert_eq!(failure(parked.call).await, CLOSED);
        assert!(device.is_poisoned(), "the task panicked with the lock held");

        let mut fresh = parked
            .wg
            .connect_tcp(&to, &within(5))
            .await
            .expect("a connection through a new tunnel");
        assert_eq!(echo(&mut fresh, b"after").await, b"after");
        let handshakes = parked.peer.core().handshakes;
        assert_eq!(handshakes, 2, "a new tunnel greeted the peer");
    }

    /// What this thread logs, captured: a line for each event — its level,
    /// then its fields by name. Hand-written: the crate depends on no
    /// subscriber.
    #[derive(Clone, Default)]
    struct Log(Arc<StdMutex<Vec<String>>>);

    impl Log {
        /// Captures while the guard and the second dispatcher live. While
        /// one dispatcher alone is registered, tracing asks only the thread
        /// that first reaches a call site whether it is wanted: a site
        /// another test reached first would stay shut on this thread too.
        fn capture() -> (Log, tracing::subscriber::DefaultGuard, tracing::Dispatch) {
            let log = Log::default();
            let logging = tracing::subscriber::set_default(log.clone());
            let second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
            (log, logging, second)
        }

        /// The lines that have `text` in them.
        fn with(&self, text: &str) -> Vec<String> {
            let lines = self.0.lock().unwrap();
            lines
                .iter()
                .filter(|line| line.contains(text))
                .cloned()
                .collect()
        }
    }

    impl tracing::Subscriber for Log {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut fields = Fields(Vec::new());
            event.record(&mut fields);
            fields.0.sort();
            let line = format!("{} {}", event.metadata().level(), fields.0.join(" "));
            self.0.lock().unwrap().push(line);
        }

        fn enter(&self, _: &tracing::span::Id) {}

        fn exit(&self, _: &tracing::span::Id) {}
    }

    /// An event's fields, each as `name=value`.
    struct Fields(Vec<String>);

    impl tracing::field::Visit for Fields {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push(format!("{}={value:?}", field.name()));
        }
    }

    /// A tunnel that ended on its own — its task died — is said so in the
    /// log when the next connection finds it: once, with the policy's name
    /// and nothing else, whether another tunnel can start or not. One that
    /// a later configuration took over is not.
    #[tokio::test]
    async fn a_tunnel_that_ended_unexpectedly_is_said_so_once() {
        const SAID: &str = "WARN message=wireguard: the tunnel ended unexpectedly; \
                            the next connection starts a new one policy=WG";
        let (log, _logging, _second) = Log::capture();
        let (_peer, wg, armed) = fragile_tunnel(PeerOpts::default(), |_| {}).await;
        let to = at("10.0.0.1", ECHO_PORT);
        wg.connect_tcp(&to, &within(5)).await.expect("a connection");
        dies(&armed).await;
        for _ in 0..2 {
            wg.connect_tcp(&to, &within(5))
                .await
                .expect("a connection through a new tunnel");
        }
        assert_eq!(log.with("ended unexpectedly"), [SAID]);

        // the new tunnel dies too, and this time none can take its place: a
        // later configuration has started its own meanwhile
        dies(&armed).await;
        let _later = superseding(&wg).await;
        for _ in 0..2 {
            let e = wg
                .connect_tcp(&to, &within(5))
                .await
                .map(|_| ())
                .unwrap_err();
            assert_eq!(e.to_string(), SUPERSEDED);
        }
        assert_eq!(log.with("ended unexpectedly"), [SAID, SAID]);

        // a later configuration taking a running tunnel over is no surprise
        let (_peer, old) = tunnel(PeerOpts::default(), |_| {}).await;
        old.connect_tcp(&to, &within(5))
            .await
            .expect("a connection");
        let _newer = superseding(&old).await;
        let e = old
            .connect_tcp(&to, &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(e.to_string(), SUPERSEDED);
        assert_eq!(log.with("ended unexpectedly").len(), 2);
    }

    /// The fake peer told to be silent while its stack has a deadline — a
    /// connection is open, what it echoed not acknowledged yet — waits the
    /// silence out instead of going round and round: the deadline comes due
    /// once, and the stack carries on when the silence ends.
    #[tokio::test]
    async fn a_silent_fake_peer_waits_instead_of_spinning() {
        let (peer, wg) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = wg
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        // the acknowledgement of this echo is still to come: the peer's
        // stack is due to send the echo again
        assert_eq!(echo(&mut stream, b"before").await, b"before");
        peer.go_silent(true);
        until(|| peer.due_while_silent() > 0).await;
        // what is measured: it stays silent a while longer
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(peer.due_while_silent(), 1, "it went round while silent");
        peer.go_silent(false);
        assert_eq!(echo(&mut stream, b"after").await, b"after");
    }
}
