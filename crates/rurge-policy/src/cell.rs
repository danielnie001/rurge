//! The registry as seen by things that outlive a config generation
//! (M1 design 6.2).

use crate::registry::{PolicyRegistry, TerminalKind};
use arc_swap::ArcSwapOption;
use rurge_net::BoxFuture;
use rurge_net::connector::{
    BoxedDatagram, BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Datagram, Route, Target,
};
use rurge_net::packet_datagram::packet_datagram;
use rurge_proto::{OutboundError, UdpSupport};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::ReadBuf;

/// Where the current generation's registry can be found. The engine stores
/// every new generation here and clears the cell when it goes away: the
/// registry owns outbounds, an outbound may own a `ChainConnector`, and that
/// points back here — clearing is what breaks the cycle.
#[derive(Default)]
pub struct RegistryCell(ArcSwapOption<PolicyRegistry>);

impl RegistryCell {
    pub fn new() -> Arc<RegistryCell> {
        Arc::new(RegistryCell::default())
    }

    pub fn store(&self, registry: Arc<PolicyRegistry>) {
        self.0.store(Some(registry));
    }

    pub fn clear(&self) {
        self.0.store(None);
    }

    pub fn load(&self) -> Option<Arc<PolicyRegistry>> {
        self.0.load_full()
    }
}

/// The connector of a policy with `underlying-proxy = <name>`: reaches the
/// policy's own server through whatever `<name>` resolves to *now* — a group
/// follows its current selection, a reload follows the new generation. The
/// server's host name travels as it is, so the underlying proxy resolves it
/// (FR-OUT-08).
pub struct ChainConnector {
    cell: Arc<RegistryCell>,
    name: String,
}

impl ChainConnector {
    pub fn new(cell: Arc<RegistryCell>, name: impl Into<String>) -> ChainConnector {
        ChainConnector {
            cell,
            name: name.into(),
        }
    }

    fn via(&self, e: OutboundError) -> io::Error {
        match e {
            OutboundError::Io(e) => named(&self.name, e),
            OutboundError::Timeout => io::Error::new(
                io::ErrorKind::TimedOut,
                format!("via {}: connect timed out", self.name),
            ),
            other => io::Error::other(format!("via {}: {other}", self.name)),
        }
    }
}

/// `e` as the hop `name` failed with it, its kind kept.
fn named(name: &str, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("via {name}: {e}"))
}

impl Connector for ChainConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
            let Some(registry) = self.cell.load() else {
                return Err(io::Error::other(format!(
                    "via {}: no policy registry is active",
                    self.name
                )));
            };
            if !registry.contains(&self.name) {
                // a reload removed the name this policy was built against
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("via {}: the policy no longer exists", self.name),
                ));
            }
            let resolution = registry.resolve_relay(&self.name);
            if let (TerminalKind::Reject, Some(note)) = (resolution.terminal, &resolution.note) {
                // why the relay refuses says more than "rejected": a group
                // without members, a group cycle, a protocol not implemented
                return Err(io::Error::other(format!("via {}: {note}", self.name)));
            }
            resolution
                .outbound
                .connect_tcp(target, opts)
                .await
                .map_err(|e| self.via(e))
        })
    }

    /// The underlying policy's own UDP carrier (phase 2 M5 design 4.3): what
    /// this hop sends to its server goes through it.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedPacketSocket>> {
        Box::pin(async move {
            let Some(registry) = self.cell.load() else {
                return Err(io::Error::other(format!(
                    "via {}: no policy registry is active",
                    self.name
                )));
            };
            if !registry.contains(&self.name) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("via {}: the policy no longer exists", self.name),
                ));
            }
            let resolution = registry.resolve_relay(&self.name);
            if let (TerminalKind::Reject, Some(note)) = (resolution.terminal, &resolution.note) {
                return Err(io::Error::other(format!("via {}: {note}", self.name)));
            }
            if resolution.outbound.udp() == UdpSupport::Unsupported {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("via {}: the underlying policy cannot carry UDP", self.name),
                ));
            }
            resolution
                .outbound
                .open_udp(opts)
                .await
                .map_err(|e| self.via(e))
        })
    }

    /// The underlying policy's UDP carrier as a datagram to `target` (phase
    /// 2 M5 design 4.3, 8.1): a `wireguard` peer through this hop. `target`
    /// is resolved the carrier's way, once. What the carrier reports once
    /// it is open names the hop as well (`Relayed`).
    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        Box::pin(async move {
            let socket = self.open_udp(opts).await?;
            let to = socket
                .resolve(target)
                .await
                .map_err(|e| self.via(OutboundError::Io(e)))?;
            Ok(Box::new(Relayed {
                carrier: packet_datagram(socket, to),
                via: self.name.clone(),
            }) as BoxedDatagram)
        })
    }

    /// The outbound the underlying policy leads to now — a group's current
    /// member, the policy a reload built: a tunnel over it follows a change
    /// (the manual: "a change of the effective policy resets the WireGuard
    /// transport sockets").
    fn route(&self) -> Option<Route> {
        let registry = self.cell.load()?;
        if !registry.contains(&self.name) {
            return None;
        }
        Some(Route::of(&registry.relay_now(&self.name).outbound))
    }
}

/// The carrier `connect_udp` hands out: the underlying policy's, what it
/// reports under that policy's name, as a failure to open it is
/// (`ChainConnector::via`). A carrier tells of a datagram it could not
/// send, and of its own end, only once it is open; left as they are, those
/// words would read as the failure of what runs over the carrier — the dial
/// of a QUIC policy failing in the underlying policy's words alone.
struct Relayed {
    carrier: BoxedDatagram,
    via: String,
}

impl Datagram for Relayed {
    fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.carrier
            .poll_send(cx, buf)
            .map_err(|e| named(&self.via, e))
    }

    fn poll_send_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.carrier
            .poll_send_ready(cx)
            .map_err(|e| named(&self.via, e))
    }

    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        self.carrier
            .poll_recv(cx, buf)
            .map_err(|e| named(&self.via, e))
    }

    fn peer_addr(&self) -> Option<SocketAddr> {
        self.carrier.peer_addr()
    }

    fn set_tos(&self, tos: u8) -> io::Result<()> {
        self.carrier.set_tos(tos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{RecordingCarrier, RecordingConnector};
    use rurge_config::HostName;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_config::spec::Tristate;
    use rurge_net::connector::PacketSocket;
    use rurge_net::quic;
    use rurge_net::quic::testing::TestCert;
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Wake, Waker};
    use std::time::Duration;

    const PROFILE: &str = "[General]\nloglevel = notify\n[Proxy]\nD = direct\nBlock = reject\n[Proxy Group]\nPick = select, D, DIRECT\nEmpty = select, policy-path=https://sub.example/e\n[Rule]\nFINAL,DIRECT\n";

    fn registry(connector: Arc<RecordingConnector>) -> Arc<PolicyRegistry> {
        let loaded = from_text(PROFILE, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        let factory = crate::testing::FakeFactory {
            connector,
            broken: None,
            environment: "env",
        };
        Arc::new(
            PolicyRegistry::build(
                &loaded.config,
                &crate::assemble(&loaded.config, &crate::Snapshots::new()),
                &factory,
                &RegistryCell::new(),
                Arc::new(crate::selections::SelectionTable::default()),
                None,
                crate::EmptyGroup::Direct,
                &crate::testing::auto_groups(),
            )
            .expect("builds"),
        )
    }

    fn server() -> Target {
        Target::new(HostName::parse("proxy.example"), 8080)
    }

    #[tokio::test]
    async fn an_empty_cell_is_an_error() {
        let chain = ChainConnector::new(RegistryCell::new(), "D");
        let e = chain
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("nothing to resolve against");
        assert_eq!(e.to_string(), "via D: no policy registry is active");
    }

    #[tokio::test]
    async fn a_name_that_is_gone_is_an_error() {
        let cell = RegistryCell::new();
        cell.store(registry(Arc::new(RecordingConnector::default())));
        let e = ChainConnector::new(cell, "Ghost")
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("the name does not exist");
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(e.to_string(), "via Ghost: the policy no longer exists");
    }

    /// The policy's own server goes to the underlying policy as it is: the
    /// name is never resolved locally (FR-OUT-08).
    #[tokio::test]
    async fn the_target_is_handed_over_unchanged_also_through_a_group() {
        let connector = Arc::new(RecordingConnector::default());
        let cell = RegistryCell::new();
        cell.store(registry(connector.clone()));
        for name in ["D", "Pick"] {
            ChainConnector::new(cell.clone(), name)
                .connect(&server(), &ConnectOpts::default())
                .await
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        assert_eq!(
            connector.seen(),
            ["dial proxy.example:8080", "dial proxy.example:8080"]
        );
    }

    /// The same for the UDP path: a name goes to the underlying policy's
    /// carrier as it is, and the datagrams are sent to it by that name —
    /// this machine looks nothing up, the carrier's side does (the endpoint
    /// of a `wireguard` tunnel that is written as a name).
    #[tokio::test]
    async fn a_udp_target_is_handed_over_unchanged() {
        let log = Arc::default();
        let connector = Arc::new(RecordingConnector {
            log: Arc::clone(&log),
            carriers: Mutex::new(vec![Box::new(RecordingCarrier { log })]),
            ..RecordingConnector::default()
        });
        let cell = RegistryCell::new();
        cell.store(registry(connector.clone()));
        let datagram = ChainConnector::new(cell, "D")
            .connect_udp(&server(), &ConnectOpts::default())
            .await
            .expect("the relay's carrier");
        std::future::poll_fn(|cx| datagram.poll_send(cx, b"out"))
            .await
            .unwrap();
        // a datagram goes out on a task of its own
        tokio::time::timeout(Duration::from_secs(5), async {
            while connector.seen().len() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the datagram reaches the carrier");
        assert_eq!(
            connector.seen(),
            ["resolve proxy.example:8080", "send proxy.example:8080"]
        );
    }

    #[tokio::test]
    async fn failures_name_the_hop_and_keep_their_kind() {
        let cell = RegistryCell::new();
        cell.store(registry(Arc::new(RecordingConnector {
            fail: true,
            ..RecordingConnector::default()
        })));
        let e = ChainConnector::new(cell.clone(), "D")
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("the connector refuses");
        assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
        assert_eq!(e.to_string(), "via D: refused by the test");
        // a reject policy underneath is an error too, not a silent hang
        let e = ChainConnector::new(cell, "Block")
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("REJECT cannot carry a connection");
        assert_eq!(e.to_string(), "via Block: rejected by REJECT");
    }

    /// A relay group without members stands for nothing: the connection is
    /// refused, never sent out directly behind the user's back.
    #[tokio::test]
    async fn an_empty_group_as_the_relay_refuses() {
        let connector = Arc::new(RecordingConnector::default());
        let cell = RegistryCell::new();
        cell.store(registry(connector.clone()));
        let e = ChainConnector::new(cell, "Empty")
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("no member to relay through");
        assert_eq!(e.to_string(), "via Empty: policy group has no members");
        assert!(connector.seen().is_empty());
    }

    /// A relay's UDP carrier that fails as told — every send with `send`,
    /// the receive with `recv` — and otherwise takes what it is sent and
    /// hears nothing.
    #[derive(Default)]
    struct Broken {
        send: Option<(io::ErrorKind, &'static str)>,
        recv: Option<(io::ErrorKind, &'static str)>,
    }

    impl PacketSocket for Broken {
        fn send_to<'a>(&'a self, _buf: &'a [u8], _to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
            Box::pin(std::future::ready(match self.send {
                Some((kind, words)) => Err(io::Error::new(kind, words)),
                None => Ok(()),
            }))
        }

        fn recv_from<'a>(
            &'a self,
            _buf: &'a mut [u8],
        ) -> BoxFuture<'a, io::Result<(usize, Target)>> {
            match self.recv {
                Some((kind, words)) => {
                    Box::pin(std::future::ready(Err(io::Error::new(kind, words))))
                }
                None => Box::pin(std::future::pending()),
            }
        }
    }

    /// What a QUIC dial through the relay `D` fails with, the relay's
    /// carrier being `carrier`.
    async fn dial_over(carrier: Broken) -> io::Error {
        let cell = RegistryCell::new();
        cell.store(registry(Arc::new(RecordingConnector {
            carriers: Mutex::new(vec![Box::new(carrier)]),
            ..RecordingConnector::default()
        })));
        let chain = ChainConnector::new(cell, "D");
        let opts = ConnectOpts::default();
        let path = quic::Path::open(&chain, &server(), None, Tristate::Auto, &opts)
            .await
            .expect("the relay's carrier");
        let tls = TestCert::new().client_tls(&["h3"]);
        let config = quic::client_config(tls, &quic::Transport::STANDARD).unwrap();
        let dialled = quic::connect(path, &config, "localhost");
        tokio::time::timeout(Duration::from_secs(10), dialled)
            .await
            .expect("an outcome within the bound")
            .map(|_| ())
            .unwrap_err()
    }

    /// What a relay's carrier reports once it is open — a datagram it
    /// cannot send, its own end — names the hop and keeps its kind, as a
    /// failure to open it does: the dial of a QUIC policy over the relay
    /// does not read as that policy's own failure.
    #[tokio::test]
    async fn a_dial_over_a_relay_fails_under_the_relays_name() {
        let e = dial_over(Broken {
            send: Some((
                io::ErrorKind::Unsupported,
                "proxy: the server relays no UDP",
            )),
            ..Broken::default()
        })
        .await;
        assert_eq!(
            (e.kind(), e.to_string().as_str()),
            (
                io::ErrorKind::Unsupported,
                "via D: proxy: the server relays no UDP"
            )
        );
        let e = dial_over(Broken {
            recv: Some((
                io::ErrorKind::BrokenPipe,
                "socks5: the proxy closed the UDP association",
            )),
            ..Broken::default()
        })
        .await;
        assert_eq!(
            (e.kind(), e.to_string().as_str()),
            (
                io::ErrorKind::BrokenPipe,
                "via D: socks5: the proxy closed the UDP association"
            )
        );
    }

    /// A relay's carrier that ends by being reset — the proxy reset the
    /// stream it runs on — fails the dial in its own words, as any end
    /// does: told as a reset, QUIC would pass over it, and the dial would
    /// know only that the carrier has closed.
    #[tokio::test]
    async fn a_relay_whose_carrier_was_reset_fails_the_dial_in_the_carriers_words() {
        let e = dial_over(Broken {
            recv: Some((
                io::ErrorKind::ConnectionReset,
                "Connection reset by peer (os error 104)",
            )),
            ..Broken::default()
        })
        .await;
        assert_eq!(
            (e.kind(), e.to_string().as_str()),
            (
                io::ErrorKind::ConnectionAborted,
                "via D: Connection reset by peer (os error 104)"
            )
        );
    }

    /// A carrier whose sends and receives all fail, each way in its words.
    /// It knows where its datagrams go, and notes the TOS it is given.
    struct Failing {
        marked: Arc<Mutex<Vec<u8>>>,
    }

    impl Failing {
        const PEER: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)), 51820);
    }

    impl Datagram for Failing {
        fn poll_send(&self, _cx: &mut Context<'_>, _buf: &[u8]) -> Poll<io::Result<usize>> {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::HostUnreachable,
                "no route to the relay",
            )))
        }

        fn poll_recv(&self, _cx: &mut Context<'_>, _buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "reset by the relay",
            )))
        }

        fn peer_addr(&self) -> Option<SocketAddr> {
            Some(Failing::PEER)
        }

        fn set_tos(&self, tos: u8) -> io::Result<()> {
            self.marked.lock().unwrap().push(tos);
            Ok(())
        }
    }

    /// Whatever a relay's carrier fails with, sending or receiving, goes on
    /// under the relay's name and with its kind: what tells failures apart
    /// by kind — QUIC passes over a reset — still can. The rest is the
    /// carrier's own: where its datagrams go, which a WireGuard tunnel
    /// follows its peer by, and the TOS it is given.
    #[test]
    fn a_relays_carrier_fails_under_the_relays_name_both_ways() {
        let marked = Arc::new(Mutex::new(Vec::new()));
        let relayed = Relayed {
            carrier: Box::new(Failing {
                marked: marked.clone(),
            }),
            via: "U".into(),
        };
        assert_eq!(relayed.peer_addr(), Some(Failing::PEER));
        relayed.set_tos(0x88).unwrap();
        assert_eq!(*marked.lock().unwrap(), [0x88]);
        let mut cx = Context::from_waker(Waker::noop());
        let Poll::Ready(Err(sent)) = relayed.poll_send(&mut cx, b"out") else {
            panic!("the send fails");
        };
        assert_eq!(
            (sent.kind(), sent.to_string().as_str()),
            (
                io::ErrorKind::HostUnreachable,
                "via U: no route to the relay"
            )
        );
        let mut storage = [0u8; 16];
        let received = relayed.poll_recv(&mut cx, &mut ReadBuf::new(&mut storage));
        let Poll::Ready(Err(received)) = received else {
            panic!("the receive fails");
        };
        assert_eq!(
            (received.kind(), received.to_string().as_str()),
            (io::ErrorKind::ConnectionReset, "via U: reset by the relay")
        );
    }

    /// A carrier that cannot send until it is opened, and tells whoever
    /// waits to send when it is. Its copies are the same carrier.
    #[derive(Clone, Default)]
    struct Gate {
        open: Arc<AtomicBool>,
        sender: Arc<Mutex<Option<Waker>>>,
    }

    impl Gate {
        fn open(&self) {
            self.open.store(true, Ordering::SeqCst);
            if let Some(sender) = self.sender.lock().unwrap().take() {
                sender.wake();
            }
        }
    }

    impl Datagram for Gate {
        fn poll_send(&self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            if self.open.load(Ordering::SeqCst) {
                Poll::Ready(Ok(buf.len()))
            } else {
                Poll::Pending
            }
        }

        fn poll_send_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            *self.sender.lock().unwrap() = Some(cx.waker().clone());
            if self.open.load(Ordering::SeqCst) {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        }

        fn poll_recv(&self, _cx: &mut Context<'_>, _buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    /// Raised by the waker made of it.
    #[derive(Default)]
    struct Woken(AtomicBool);

    impl Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// What waits to send on a relay's carrier waits for the carrier under
    /// it: a QUIC connection over the relay keeps its datagram while that
    /// one cannot send, and is woken when it can.
    #[test]
    fn a_relays_carrier_can_send_when_the_carrier_under_it_can() {
        let gate = Gate::default();
        let relayed = Relayed {
            carrier: Box::new(gate.clone()),
            via: "U".into(),
        };
        let woken = Arc::new(Woken::default());
        let waker = Waker::from(woken.clone());
        let mut cx = Context::from_waker(&waker);
        assert!(
            relayed.poll_send_ready(&mut cx).is_pending(),
            "ready over a carrier that cannot send"
        );
        assert!(!woken.0.load(Ordering::SeqCst));
        gate.open();
        assert!(
            woken.0.load(Ordering::SeqCst),
            "not woken when the carrier can send"
        );
        assert!(matches!(
            relayed.poll_send_ready(&mut cx),
            Poll::Ready(Ok(()))
        ));
    }

    const GROUPS: &str = "[Proxy]\nA = http, a.example, 80\nB = http, b.example, 80\n\
[Proxy Group]\nPick = select, A, B\nAuto = url-test, A, B\n[Rule]\nFINAL,DIRECT\n";

    /// Where a relay leads now: the same while nothing changes, elsewhere
    /// once the member of its group changes. Nothing changes for asking: no
    /// round of tests is asked for. No registry, or no such name: no route.
    #[test]
    fn the_route_follows_the_member_of_the_group() {
        let loaded = from_text(GROUPS, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        let factory = crate::testing::FakeFactory {
            connector: Arc::new(RecordingConnector::default()),
            broken: None,
            environment: "env",
        };
        let selections = Arc::new(crate::selections::SelectionTable::default());
        let auto = crate::testing::auto_groups();
        let cell = RegistryCell::new();
        let registry = PolicyRegistry::build(
            &loaded.config,
            &crate::assemble(&loaded.config, &crate::Snapshots::new()),
            &factory,
            &cell,
            selections.clone(),
            None,
            crate::EmptyGroup::Direct,
            &auto,
        )
        .expect("builds");
        cell.store(Arc::new(registry));
        let pick = ChainConnector::new(cell.clone(), "Pick");
        let first = pick.route().expect("a route");
        assert_eq!(pick.route(), Some(first.clone()));
        selections.set("Pick", "B");
        assert_ne!(pick.route(), Some(first));
        assert!(ChainConnector::new(cell.clone(), "Auto").route().is_some());
        assert!(auto.requested().is_empty(), "{:?}", auto.requested());
        assert_eq!(ChainConnector::new(cell, "Ghost").route(), None);
        assert_eq!(
            ChainConnector::new(RegistryCell::new(), "Pick").route(),
            None
        );
    }

    #[test]
    fn clearing_the_cell_lets_the_registry_go() {
        let cell = RegistryCell::new();
        let reg = registry(Arc::new(RecordingConnector::default()));
        cell.store(reg.clone());
        assert!(cell.load().is_some());
        cell.clear();
        assert!(cell.load().is_none());
        assert_eq!(Arc::strong_count(&reg), 1);
    }
}
