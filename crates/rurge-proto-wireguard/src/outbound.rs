//! `WireGuardOutbound` (phase 2 M4 design §6): the tunnel starts with the
//! first dial and runs while the outbound, or a connection through it,
//! lives.

use crate::device::{Device, REDIAL};
use crate::dns::{self, Cache, Family};
use crate::stack::Refusal;
use rurge_config::HostName;
use rurge_config::spec::WireGuardSpec;
use rurge_config::wireguard::{TunnelDns, WireGuardSection};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Resolve, Target};
use rurge_proto::{Outbound, OutboundError};
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
    section: WireGuardSection,
    /// Later configurations have higher ones: a tunnel they start ends
    /// this one's, never the other way round.
    generation: u64,
    /// What the policy dials carriers with: distinguishes tunnels that
    /// share a section but must not share carriers (`with_carrier`). Empty
    /// until set.
    carrier: String,
    /// Destination names, without a `dns-server` (M4-D9).
    resolver: Arc<dyn Resolve>,
    /// What the carriers to the peers come from.
    connector: Arc<dyn Connector>,
    /// The running tunnel; the first dial starts it, the others wait.
    device: Mutex<Option<Arc<Device>>>,
    /// What the tunnel's `dns-server`s answered, for their TTL.
    cache: StdMutex<Cache>,
    /// `DNS_WAIT` (shorter in the tests).
    dns_wait: Duration,
    /// `REDIAL` (shorter in the tests).
    redial: Duration,
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
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            carrier: String::new(),
            resolver,
            connector,
            device: Mutex::new(None),
            cache: StdMutex::new(Cache::default()),
            dns_wait: DNS_WAIT,
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

    pub(crate) async fn device(&self, opts: &ConnectOpts) -> Result<Arc<Device>, OutboundError> {
        let mut slot = self.device.lock().await;
        if let Some(device) = slot.as_ref()
            && !device.is_closed()
        {
            return Ok(device.clone());
        }
        let device = Device::start(
            &self.name,
            &self.section,
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

    /// Where a connection to `target` goes: its address, or its name's.
    async fn address(
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
            .await
            .ok_or_else(|| OutboundError::Dns(format!("wireguard: dns lookup of {name} failed")))?;
        pick(&addrs, &self.section).map_err(|refusal| OutboundError::Proxy(refusal.to_string()))
    }

    /// `name`'s addresses: from the section's `dns-server`s in order through
    /// the tunnel — the first that answers ends the search, `system` asks
    /// this machine — or, without any, from this machine (M4-D9).
    async fn lookup(&self, name: &str, device: &Arc<Device>) -> Option<Vec<IpAddr>> {
        if self.section.dns_servers.is_empty() {
            return self.resolver.resolve(name).await.ok();
        }
        if let Some(addrs) = self
            .cache
            .lock()
            .expect("the cache")
            .get(name, Instant::now())
        {
            return Some(addrs);
        }
        for server in &self.section.dns_servers {
            let server = match server {
                TunnelDns::System => match self.resolver.resolve(name).await {
                    Ok(addrs) => return Some(addrs),
                    Err(_) => continue,
                },
                TunnelDns::Server(server) => *server,
            };
            let Some(answer) = self.ask(device, server, name).await else {
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
            return (!addrs.is_empty()).then_some(addrs);
        }
        None
    }

    /// `server`'s answer for `name`: A and AAAA at once, for the families
    /// the tunnel has an address of. `None` when it gave none within
    /// `dns_wait`, or no peer takes it.
    async fn ask(
        &self,
        device: &Arc<Device>,
        server: SocketAddr,
        name: &str,
    ) -> Option<Vec<(IpAddr, u32)>> {
        let ask = |family: Family, wanted: bool| async move {
            if !wanted {
                return None;
            }
            let id = getrandom::u32().unwrap_or(0) as u16;
            let question = dns::question(id, name, family)?;
            device
                .query(server, &question, self.dns_wait, |reply| {
                    dns::answer(reply, id, name, family)
                })
                .await
        };
        let (v4, v6) = tokio::join!(
            ask(Family::V4, self.section.self_ip.is_some()),
            ask(Family::V6, self.section.self_ip_v6.is_some())
        );
        if v4.is_none() && v6.is_none() {
            return None;
        }
        Some(v4.into_iter().chain(v6).flatten().collect())
    }

    async fn dial(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        let device = self.device(opts).await?;
        let ip = self.address(target, &device).await?;
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
    use crate::testing::{
        DNS_ADDRESS, ECHO_PORT, FakeWgPeer, PeerOpts, endpoint, keypair, section,
    };
    use rurge_config::wireguard::PeerEndpoint;
    use rurge_net::connector::{BoxedDatagram, Datagram, DirectConnector, SystemResolve};
    use std::io;
    use std::net::Ipv4Addr;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
        wg.dns_wait = Duration::from_millis(300);
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
        wg.dns_wait = Duration::from_secs(5);
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
                wg.dns_wait = Duration::from_secs(5);
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

    /// Two policies that name one section: one tunnel, one handshake.
    #[tokio::test]
    async fn policies_that_name_one_section_share_its_tunnel() {
        let (peer, a) = tunnel(PeerOpts::default(), |_| {}).await;
        let spec = WireGuardSpec {
            section: a.section.clone(),
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
        let mut section = old.section.clone();
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
    /// (P10 / A), so it dials its own carriers, which it cannot, and fails
    /// with the chain's usual refusal — the other policy's tunnel keeps
    /// running untouched.
    #[tokio::test]
    async fn a_policy_over_a_chain_never_shares_a_direct_tunnel() {
        let (peer, a) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = a
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"ping").await, b"ping");
        let spec = WireGuardSpec {
            section: a.section.clone(),
        };
        let b = WireGuardOutbound::new("Chained", &spec, no_names(), Arc::new(NoUdp))
            .with_carrier("chain".to_string());
        let e = b
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(e, OutboundError::Unsupported(_)), "{e}");
        assert_eq!(
            e.to_string(),
            "policy protocol not implemented: wireguard over underlying-proxy"
        );
        assert_eq!(echo(&mut stream, b"still").await, b"still");
        assert_eq!(peer.core().handshakes, 1);
    }

    /// A reload that only changes the policy's carrier (`ip-version`,
    /// `underlying-proxy`, `[General] ipv6`) — the section itself is
    /// unchanged — still takes the tunnel over, exactly as a changed
    /// section does (P10 / A).
    #[tokio::test]
    async fn a_policy_whose_carriers_changed_takes_the_tunnel_over() {
        let (peer, old) = tunnel(PeerOpts::default(), |_| {}).await;
        let mut stream = old
            .connect_tcp(&at("10.0.0.1", ECHO_PORT), &within(5))
            .await
            .expect("a connection");
        assert_eq!(echo(&mut stream, b"old").await, b"old");
        let spec = WireGuardSpec {
            section: old.section.clone(),
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
    /// global start lock (P10 / C).
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
            section: a1.section.clone(),
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
}
