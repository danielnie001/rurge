//! `encrypted-dns-follow-outbound-mode` (M3 design §7.4): connectors that
//! route the resolver's TCP / DoT / DoH upstream connections through the dial
//! pipeline as `Internal` sessions — and the QUIC carriers of its DoQ / DoH3
//! upstreams as `Internal` UDP sessions (phase 2 M7 design 7.3). There is one
//! connector for each kind of upstream (`PipelineConnector::connectors`), and
//! each calls the sessions it dials by what its kind speaks: the scheme of the
//! server tells, the port does not. The resolver wraps each in its
//! `BootstrapConnector`, which resolves a domain-configured upstream name via
//! plain-UDP bootstrap first and hands this connector an IP target — so the
//! pipeline never needs to resolve anything itself and cannot re-enter the
//! resolver. The `Domain` arms below are only a defensive fallback. Plain UDP
//! upstreams never use a connector.

use crate::engine::Engine;
use rurge_config::HostName;
use rurge_config::rule::ProtocolKind;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_dns::DnsConnectors;
use rurge_inbound::{Counting, SessionHandle, SessionOutcome};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedDatagram, BoxedStream, ConnectOpts, Connector, Datagram, Target};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, Weak};
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct PipelineConnector {
    engine: OnceLock<Weak<Engine>>,
    /// Plain direct connector: the reject-bypass path (targets are already IPs)
    /// and the defensive non-IP path.
    fallback: Arc<dyn Connector>,
}

impl PipelineConnector {
    pub fn new(fallback: Arc<dyn Connector>) -> Arc<PipelineConnector> {
        Arc::new(PipelineConnector {
            engine: OnceLock::new(),
            fallback,
        })
    }

    /// Binds the engine after it is built (idempotent; first wins).
    pub fn attach(&self, engine: Weak<Engine>) {
        let _ = self.engine.set(engine);
    }

    /// The resolver's connectors, one per kind of upstream. Each dials through
    /// this pipeline connector and calls the sessions it makes by what its
    /// kind of upstream speaks.
    pub fn connectors(self: &Arc<Self>) -> DnsConnectors {
        let face = |protocol| -> Arc<dyn Connector> {
            Arc::new(Face {
                pipeline: self.clone(),
                protocol,
            })
        };
        DnsConnectors {
            tcp: face(ProtocolKind::Dns),
            tls: face(ProtocolKind::Dot),
            https: face(ProtocolKind::Doh),
            quic: face(ProtocolKind::Doq),
            h3: face(ProtocolKind::Doh3),
        }
    }

    /// A DNS upstream's TCP connection, routed as an `Internal` session
    /// called `protocol`.
    async fn connect_as(
        &self,
        protocol: ProtocolKind,
        target: &Target,
        opts: &ConnectOpts,
    ) -> io::Result<BoxedStream> {
        let ip = match &target.host {
            HostName::Ip(ip) => *ip,
            // Defensive only: the resolver's BootstrapConnector resolves
            // upstream names itself and hands us IP targets. Anything else
            // must not enter the pipeline (it could recurse), so go direct.
            HostName::Domain(_) => return self.fallback.connect(target, opts).await,
        };
        let engine = self.engine.get().and_then(Weak::upgrade);
        let Some(engine) = engine else {
            // Not attached yet (bootstrap phase) → direct.
            return self.fallback.connect(target, opts).await;
        };
        let mut session = SessionInfo::tcp(HostName::Ip(ip), target.port);
        session.listener = ListenerKind::Internal;
        session.transport = Transport::Tcp;
        session.protocol = Some(protocol);
        engine.dial_internal(session, &self.fallback).await
    }

    /// The QUIC carrier of a DoQ / DoH3 upstream, routed as `connect_as`
    /// routes a TCP connection: an `Internal` UDP session called `protocol`.
    async fn connect_udp_as(
        &self,
        protocol: ProtocolKind,
        target: &Target,
        opts: &ConnectOpts,
    ) -> io::Result<BoxedDatagram> {
        let ip = match &target.host {
            HostName::Ip(ip) => *ip,
            // Defensive only, as for `connect_as`.
            HostName::Domain(_) => return self.fallback.connect_udp(target, opts).await,
        };
        let engine = self.engine.get().and_then(Weak::upgrade);
        let Some(engine) = engine else {
            // Not attached yet (bootstrap phase) → direct.
            return self.fallback.connect_udp(target, opts).await;
        };
        let mut session = SessionInfo::udp(HostName::Ip(ip), target.port);
        session.listener = ListenerKind::Internal;
        session.protocol = Some(protocol);
        engine.dial_internal_udp(session, &self.fallback).await
    }
}

/// One of the connectors `PipelineConnector::connectors` gives out: the
/// pipeline connector, the sessions it dials called `protocol`.
struct Face {
    pipeline: Arc<PipelineConnector>,
    protocol: ProtocolKind,
}

impl Connector for Face {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(self.pipeline.connect_as(self.protocol, target, opts))
    }

    /// `as_direct` stays `None`, so the QUIC path always takes its carrier
    /// from here.
    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        Box::pin(self.pipeline.connect_udp_as(self.protocol, target, opts))
    }
}

/// Finishes the session handle when the DNS connection — or QUIC carrier —
/// is dropped, so the internal session leaves the active index and lands in
/// the request log.
pub(crate) struct FinishOnDrop<S> {
    inner: S,
    handle: Arc<SessionHandle>,
}

impl<S> FinishOnDrop<S> {
    pub(crate) fn new(inner: S, handle: Arc<SessionHandle>) -> FinishOnDrop<S> {
        FinishOnDrop { inner, handle }
    }
}

impl<S> Drop for FinishOnDrop<S> {
    fn drop(&mut self) {
        self.handle.finish(SessionOutcome::Completed);
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for FinishOnDrop<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for FinishOnDrop<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl<S: Datagram> Datagram for FinishOnDrop<S> {
    fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.inner.poll_send(cx, buf)
    }
    fn poll_send_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.poll_send_ready(cx)
    }
    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        self.inner.poll_recv(cx, buf)
    }
    fn peer_addr(&self) -> Option<SocketAddr> {
        self.inner.peer_addr()
    }
    fn set_tos(&self, tos: u8) -> io::Result<()> {
        self.inner.set_tos(tos)
    }
}

/// Wraps a freshly dialed stream so bytes are counted (`Counting`) and the
/// handle is finished on drop (`FinishOnDrop`); the outbound is ready as of
/// now.
pub(crate) fn wrap_internal(stream: BoxedStream, handle: Arc<SessionHandle>) -> BoxedStream {
    handle.mark_connected();
    Box::new(FinishOnDrop::new(
        Counting::new(stream, handle.clone()),
        handle,
    ))
}

/// Counts a carrier's datagrams into a session handle, as `Counting` counts
/// a stream's bytes: what it receives is `down`, what it sends `up`. What
/// the carrier fails with — a datagram it could not send, its own end — is
/// the session's failure, as a flow's whose datagram cannot be sent: it
/// goes on the record, counts against the `smart` member that carried the
/// session, and reaches the resolver under the policy's name. A reset is
/// no such failure: QUIC, which is all these carriers carry, passes over
/// it and uses the carrier on — Windows tells a connected socket so of a
/// datagram nobody took — and the session goes on with it.
struct CountingDatagram {
    inner: BoxedDatagram,
    handle: Arc<SessionHandle>,
    /// The policy the carrier goes through, which its failure names; `None`
    /// for one that goes directly in a policy's place.
    via: Option<String>,
}

impl Datagram for CountingDatagram {
    fn poll_send(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let sent = self.inner.poll_send(cx, buf);
        if let Poll::Ready(Ok(n)) = &sent {
            self.handle.add_up(*n as u64);
        }
        sent
    }
    fn poll_send_ready(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.poll_send_ready(cx)
    }
    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        match ready!(self.inner.poll_recv(cx, buf)) {
            Ok(()) => {
                let n = buf.filled().len() - before;
                self.handle.add_down(n as u64);
                if n > 0 {
                    self.handle.mark_first_byte();
                }
                Poll::Ready(Ok(()))
            }
            // as it is: what reads it tells a reset by its kind
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => Poll::Ready(Err(e)),
            Err(e) => {
                let failed = crate::udp::upstream_failed(&self.handle, e.to_string());
                self.handle.finish(failed);
                Poll::Ready(Err(match &self.via {
                    // the text alone could pass for the DNS server's own
                    Some(via) => io::Error::new(e.kind(), format!("via {via}: {e}")),
                    None => e,
                }))
            }
        }
    }
    fn peer_addr(&self) -> Option<SocketAddr> {
        self.inner.peer_addr()
    }
    fn set_tos(&self, tos: u8) -> io::Result<()> {
        self.inner.set_tos(tos)
    }
}

/// `wrap_internal` for a freshly opened QUIC carrier: its datagrams are
/// counted (`CountingDatagram`) — its failure told under the name of the
/// policy it goes through, `via` — and the handle is finished on drop
/// (`FinishOnDrop`); the outbound is ready as of now.
pub(crate) fn wrap_internal_udp(
    datagram: BoxedDatagram,
    handle: Arc<SessionHandle>,
    via: Option<&str>,
) -> BoxedDatagram {
    handle.mark_connected();
    Box::new(FinishOnDrop::new(
        CountingDatagram {
            inner: datagram,
            handle: handle.clone(),
            via: via.map(str::to_string),
        },
        handle,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::{RecordStatus, RequestLog, TrafficStats};
    use crate::runtime::{Runtime, RuntimeOptions};
    use crate::shared::EngineShared;
    use crate::stack::StackOptions;
    use rurge_config::HostName;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_dns::system::StaticSystemDns;
    use rurge_net::connector::{ConnectOpts, DirectConnector, PacketSocket, SystemResolve, Target};
    use rurge_net::packet_datagram::packet_datagram;
    use rurge_net::socket::NoopSocketHook;
    use rurge_policy::smart::{Health, SmartBook};
    use rurge_proto::{OutboundRef, Reject, RejectKind};
    use rurge_rules::{GeoUrls, OutboundMode};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Wake, Waker};
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn without_an_engine_it_delegates_to_the_fallback() {
        // A loopback listener the fallback can actually reach: with no engine
        // attached, only the fallback can produce a stream, so a successful
        // connect is what proves the delegation (and that it neither panics
        // nor hangs). No public network is involved.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        let fallback: Arc<dyn Connector> = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let pc = PipelineConnector::new(fallback);
        let face = pc.connectors().tcp;
        let target = Target::new(HostName::parse("127.0.0.1"), port);
        // `BoxedStream` is not `Debug`, so drop the stream from the Ok arm to
        // keep the assertion message printable.
        let res = tokio::time::timeout(
            Duration::from_secs(5),
            face.connect(&target, &ConnectOpts::default()),
        )
        .await
        .map(|r| r.map(|_| ()));
        assert!(
            matches!(res, Ok(Ok(()))),
            "an unattached connector delegates to the fallback: {res:?}"
        );
    }

    /// A running engine that follows the outbound mode for its DNS
    /// connections and sends everything DIRECT, for the connectors of its
    /// pipeline to be driven by hand.
    async fn engine_following_the_outbound_mode(dir: &std::path::Path) -> Arc<Engine> {
        let text = "[General]\nencrypted-dns-follow-outbound-mode = true\n[Rule]\nFINAL,DIRECT\n";
        let loaded = from_text(text, &dir.join("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        let runtime = Runtime::build(
            loaded.config,
            RuntimeOptions {
                stack: StackOptions {
                    data_dir: dir.to_path_buf(),
                    no_network: true,
                    geo_urls: GeoUrls::default(),
                    dns_cache_size: 16,
                    system: Arc::new(StaticSystemDns::default()),
                    wait: Duration::ZERO,
                    dns_connector: None,
                    socket_hook: Arc::new(NoopSocketHook),
                },
                outbound_mode: OutboundMode::Rule,
                idle_timeout: Duration::from_secs(60),
                shared: EngineShared::default(),
                request_log_size: 16,
            },
        )
        .await
        .unwrap();
        Engine::new(runtime)
    }

    /// Each of the five connectors calls the sessions it dials by what its
    /// kind of upstream speaks — the same, at whatever port. They dial to
    /// loopback servers of the test's own and are told apart by the record
    /// of the session each one makes.
    #[tokio::test]
    async fn the_five_faces_carry_the_five_labels() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_following_the_outbound_mode(dir.path()).await;
        let faces = engine
            .runtime()
            .dns_pipeline()
            .expect("the profile follows the outbound mode")
            .connectors();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp = Target::new(
            HostName::parse("127.0.0.1"),
            listener.local_addr().unwrap().port(),
        );
        tokio::spawn(async move {
            let mut accepted = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                accepted.push(stream);
            }
        });
        let udp = Target::new(HostName::parse("127.0.0.1"), udp_echo().await);
        let opts = ConnectOpts::default();

        // held, so that the sessions are still there to be told apart
        let mut streams = Vec::new();
        for face in [&faces.tcp, &faces.tls, &faces.https] {
            streams.push(face.connect(&tcp, &opts).await.unwrap());
        }
        let mut carriers = Vec::new();
        for face in [&faces.quic, &faces.h3] {
            carriers.push(face.connect_udp(&udp, &opts).await.unwrap());
        }
        let mut sessions = engine.request_log().active();
        sessions.sort_by_key(|session| session.id);
        assert!(
            sessions
                .iter()
                .all(|session| session.listener == ListenerKind::Internal),
            "{sessions:?}"
        );
        assert_eq!(
            sessions
                .iter()
                .map(|session| session.protocol)
                .collect::<Vec<_>>(),
            [
                Some(ProtocolKind::Dns),
                Some(ProtocolKind::Dot),
                Some(ProtocolKind::Doh),
                Some(ProtocolKind::Doq),
                Some(ProtocolKind::Doh3),
            ],
            "{sessions:?}"
        );
    }

    /// A loopback UDP socket that sends every datagram back; its port.
    async fn udp_echo() -> u16 {
        let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = echo.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 64];
            while let Ok((n, from)) = echo.recv_from(&mut buf).await {
                let _ = echo.send_to(&buf[..n], from).await;
            }
        });
        port
    }

    /// Sends `payload` on `carrier`; what comes back.
    async fn echoed(carrier: &BoxedDatagram, payload: &[u8]) -> Vec<u8> {
        std::future::poll_fn(|cx| carrier.poll_send(cx, payload))
            .await
            .unwrap();
        let mut storage = [0u8; 64];
        let mut buf = ReadBuf::new(&mut storage);
        tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| carrier.poll_recv(cx, &mut buf)),
        )
        .await
        .expect("the echo within the bound")
        .unwrap();
        buf.filled().to_vec()
    }

    /// Unattached, the connector takes a QUIC carrier from the fallback as
    /// well: the loopback echo answers through it.
    #[tokio::test]
    async fn without_an_engine_a_carrier_comes_from_the_fallback() {
        let port = udp_echo().await;
        let fallback: Arc<dyn Connector> = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let pc = PipelineConnector::new(fallback);
        let face = pc.connectors().quic;
        let target = Target::new(HostName::parse("127.0.0.1"), port);
        let carrier = tokio::time::timeout(
            Duration::from_secs(5),
            face.connect_udp(&target, &ConnectOpts::default()),
        )
        .await
        .expect("within the bound")
        .expect("a carrier from the fallback");
        assert_eq!(echoed(&carrier, b"ping").await, b"ping");
    }

    /// An internal carrier counts what it sends as `up` and what comes back
    /// as `down` — the first datagram back is the first byte —, says where
    /// it sends to, and finishes its session when it is dropped.
    #[tokio::test]
    async fn an_internal_carrier_counts_both_ways_and_finishes_when_dropped() {
        let port = udp_echo().await;
        let target = Target::new(HostName::parse("127.0.0.1"), port);
        let handle = SessionHandle::new(1, SessionInfo::udp(target.host.clone(), port));
        let direct = DirectConnector::new(Arc::new(SystemResolve));
        let datagram = direct
            .connect_udp(&target, &ConnectOpts::default())
            .await
            .unwrap();
        let carrier = wrap_internal_udp(datagram, handle.clone(), None);
        assert!(handle.connect_time().is_some());
        assert_eq!(
            carrier.peer_addr(),
            Some(SocketAddr::from(([127, 0, 0, 1], port)))
        );
        assert_eq!(echoed(&carrier, b"ping!").await, b"ping!");
        assert_eq!(handle.bytes(), (5, 5));
        assert!(handle.first_byte_seen());
        assert!(!handle.is_finished());
        drop(carrier);
        assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
    }

    /// A carrier whose receive tells that it failed.
    struct Failing;

    const NO_RELAY: &str = "proxy: the server relays no UDP";

    impl Datagram for Failing {
        fn poll_send(&self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_recv(&self, _cx: &mut Context<'_>, _buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::new(io::ErrorKind::Unsupported, NO_RELAY)))
        }
    }

    /// What a carrier fails with ends its session as a failure, in the
    /// carrier's words — the failure of the member that carried it —, and
    /// goes on to the resolver under the policy's name; the failure of a
    /// carrier that goes in no policy's name goes on as it is.
    #[test]
    fn a_carrier_that_fails_fails_its_session_and_names_its_policy() {
        for (via, heard) in [
            (Some("Proxy"), format!("via Proxy: {NO_RELAY}")),
            (None, NO_RELAY.to_string()),
        ] {
            let handle = SessionHandle::new(1, SessionInfo::udp(HostName::parse("192.0.2.1"), 853));
            let carrier = wrap_internal_udp(Box::new(Failing), handle.clone(), via);
            let mut storage = [0u8; 64];
            let mut buf = ReadBuf::new(&mut storage);
            let mut cx = Context::from_waker(std::task::Waker::noop());
            let Poll::Ready(Err(e)) = carrier.poll_recv(&mut cx, &mut buf) else {
                panic!("the carrier fails");
            };
            assert_eq!(
                (e.kind(), e.to_string()),
                (io::ErrorKind::Unsupported, heard)
            );
            let failed = Some(SessionOutcome::Failed(NO_RELAY.to_string()));
            assert_eq!(handle.outcome(), failed);
            assert!(handle.upstream_failed());
            // and stays so when the carrier goes
            drop(carrier);
            assert_eq!(handle.outcome(), failed);
        }
    }

    /// A carrier whose receives fail with these, in turn; then nothing
    /// comes.
    struct FailingWith(Mutex<VecDeque<io::Error>>);

    impl Datagram for FailingWith {
        fn poll_send(&self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_recv(&self, _cx: &mut Context<'_>, _buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            match self.0.lock().unwrap().pop_front() {
                Some(e) => Poll::Ready(Err(e)),
                None => Poll::Pending,
            }
        }
    }

    /// What `carrier` receives next, without waiting for it.
    fn received(carrier: &BoxedDatagram) -> Poll<io::Result<()>> {
        let mut storage = [0u8; 64];
        let mut cx = Context::from_waker(Waker::noop());
        carrier.poll_recv(&mut cx, &mut ReadBuf::new(&mut storage))
    }

    /// A reset is no failure of the carrier: QUIC passes over it and uses
    /// the carrier on — Windows tells a connected socket so of a datagram
    /// nobody took, a server whose port was not open yet. It goes on as it
    /// is, the session stays an active one on the record, and the `smart`
    /// member that carries it is told nothing. What the carrier fails with
    /// after that ends the session, and counts, as ever.
    #[tokio::test]
    async fn a_reset_neither_ends_the_session_nor_counts_against_its_member() {
        const SERVER: &str = "192.0.2.1";
        let handle = SessionHandle::new(1, SessionInfo::udp(HostName::parse(SERVER), 853));
        // the record and the member's report, hung on the session as the
        // engine hangs them (`Engine::new_handle`, `report_internal`)
        let (log, traffic) = (Arc::new(RequestLog::new(8)), TrafficStats::new());
        log.mark_active(&handle);
        let finished = log.clone();
        handle.on_finish(move |h, outcome| finished.record_finished(h, outcome, &traffic));
        let book = Arc::new(SmartBook::new());
        let member: OutboundRef = Arc::new(Reject::new(RejectKind::Reject));
        let script = VecDeque::from([
            io::Error::new(io::ErrorKind::ConnectionReset, "reset by the peer"),
            io::Error::new(io::ErrorKind::Unsupported, NO_RELAY),
        ]);
        let carrier = wrap_internal_udp(
            Box::new(FailingWith(Mutex::new(script))),
            handle.clone(),
            Some("M"),
        );
        crate::smart::watch(&handle, book.clone(), "M", &member, SERVER, true);

        let Poll::Ready(Err(reset)) = received(&carrier) else {
            panic!("the reset goes on");
        };
        assert_eq!(handle.outcome(), None);
        assert!(!handle.upstream_failed());
        assert!(log.recent(8).is_empty(), "{:?}", log.recent(8));
        let active = log.active();
        assert_eq!(active.len(), 1);
        assert_eq!(
            (&active[0].status, &active[0].error),
            (&RecordStatus::Active, &None)
        );
        let now = Instant::now();
        assert_eq!(
            book.health("M", &member, now),
            Health::Unknown { failures: 0 }
        );
        assert!(book.site(SERVER, now).failed.is_empty());
        assert_eq!(
            (reset.kind(), reset.to_string().as_str()),
            (io::ErrorKind::ConnectionReset, "reset by the peer")
        );

        let Poll::Ready(Err(failure)) = received(&carrier) else {
            panic!("the carrier fails");
        };
        assert_eq!(
            (failure.kind(), failure.to_string()),
            (io::ErrorKind::Unsupported, format!("via M: {NO_RELAY}"))
        );
        let failed = SessionOutcome::Failed(NO_RELAY.to_string());
        assert_eq!(handle.outcome(), Some(failed));
        let recorded = log.recent(8);
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            (&recorded[0].status, recorded[0].error.as_deref()),
            (&RecordStatus::Failed, Some(NO_RELAY))
        );
        let now = Instant::now();
        assert_eq!(
            book.health("M", &member, now),
            Health::Unknown { failures: 1 }
        );
        assert_eq!(book.site(SERVER, now).failed, ["M"]);
    }

    /// A proxy's carrier that takes what it is sent and ends by being
    /// reset: the server reset the stream it runs on.
    struct ResetCarrier;

    const RESET: &str = "Connection reset by peer (os error 104)";

    impl PacketSocket for ResetCarrier {
        fn send_to<'a>(&'a self, _buf: &'a [u8], _to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
            Box::pin(std::future::ready(Ok(())))
        }
        fn recv_from<'a>(
            &'a self,
            _buf: &'a mut [u8],
        ) -> BoxFuture<'a, io::Result<(usize, Target)>> {
            let reset = io::Error::new(io::ErrorKind::ConnectionReset, RESET);
            Box::pin(std::future::ready(Err(reset)))
        }
    }

    /// A carrier that ends by being reset is not the reset of a datagram
    /// that bounced: it comes as the carrier's end — an abort, in the
    /// carrier's words (`packet_datagram`) — and the session fails with it
    /// as with any other end.
    #[tokio::test]
    async fn a_carrier_that_was_reset_fails_its_session_in_the_carriers_words() {
        let server = Target::new(HostName::parse("192.0.2.1"), 853);
        let handle = SessionHandle::new(1, SessionInfo::udp(server.host.clone(), server.port));
        let carrier = wrap_internal_udp(
            packet_datagram(Box::new(ResetCarrier), server),
            handle.clone(),
            Some("M"),
        );
        let mut storage = [0u8; 64];
        let mut buf = ReadBuf::new(&mut storage);
        let ended = tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| carrier.poll_recv(cx, &mut buf)),
        )
        .await
        .expect("the carrier's end within the bound")
        .unwrap_err();
        assert_eq!(
            (ended.kind(), ended.to_string()),
            (io::ErrorKind::ConnectionAborted, format!("via M: {RESET}"))
        );
        let failed = SessionOutcome::Failed(RESET.to_string());
        assert_eq!(handle.outcome(), Some(failed));
        assert!(handle.upstream_failed());
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

    /// Checks that what waits to send on `outer` waits for `gate`, the
    /// carrier under it: it is pending while the gate is shut, and woken
    /// when it opens. A layer that says "ready" of its own accord has QUIC
    /// drop the first packet of a connection on a socket not known to be
    /// writable yet, and the dial wait for the packet to be sent again.
    fn waits_to_send_as(outer: &dyn Datagram, gate: &Gate) {
        let woken = Arc::new(Woken::default());
        let waker = Waker::from(woken.clone());
        let mut cx = Context::from_waker(&waker);
        assert!(
            outer.poll_send_ready(&mut cx).is_pending(),
            "ready over a carrier that cannot send"
        );
        assert!(!woken.0.load(Ordering::SeqCst));
        gate.open();
        assert!(
            woken.0.load(Ordering::SeqCst),
            "not woken when the carrier can send"
        );
        assert!(matches!(
            outer.poll_send_ready(&mut cx),
            Poll::Ready(Ok(()))
        ));
    }

    fn a_session() -> Arc<SessionHandle> {
        SessionHandle::new(1, SessionInfo::udp(HostName::parse("192.0.2.1"), 853))
    }

    #[test]
    fn a_carrier_that_finishes_its_session_can_send_when_the_one_under_it_can() {
        let gate = Gate::default();
        waits_to_send_as(&FinishOnDrop::new(gate.clone(), a_session()), &gate);
    }

    #[test]
    fn a_carrier_that_counts_can_send_when_the_one_under_it_can() {
        let gate = Gate::default();
        let counting = CountingDatagram {
            inner: Box::new(gate.clone()),
            handle: a_session(),
            via: None,
        };
        waits_to_send_as(&counting, &gate);
    }
}
