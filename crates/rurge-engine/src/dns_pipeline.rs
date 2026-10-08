//! `encrypted-dns-follow-outbound-mode` (M3 design §7.4): a connector that
//! routes the resolver's TCP / DoT / DoH upstream connections through the dial
//! pipeline as `Internal` sessions — and the QUIC carriers of its DoQ / DoH3
//! upstreams as `Internal` UDP sessions (phase 2 M7 design 7.3). The resolver
//! wraps it in its `BootstrapConnector`, which resolves a domain-configured
//! upstream name via plain-UDP bootstrap first and hands this connector an IP
//! target — so the pipeline never needs to resolve anything itself and cannot
//! re-enter the resolver. The `Domain` arms below are only a defensive
//! fallback. Plain UDP upstreams never use a connector.

use crate::engine::Engine;
use rurge_config::HostName;
use rurge_config::rule::ProtocolKind;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
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
}

/// DNS server port → the protocol tag its session carries (best-effort).
fn protocol_for(port: u16) -> ProtocolKind {
    match port {
        853 => ProtocolKind::Dot,
        443 => ProtocolKind::Doh,
        _ => ProtocolKind::Dns,
    }
}

/// DNS server port → the protocol tag of its QUIC carrier's session: the
/// only UDP this connector carries is DoH3's and DoQ's, told apart by the
/// port (best-effort, as `protocol_for`).
fn quic_protocol_for(port: u16) -> ProtocolKind {
    match port {
        443 => ProtocolKind::Doh3,
        _ => ProtocolKind::Doq,
    }
}

impl Connector for PipelineConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
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
            session.protocol = Some(protocol_for(target.port));
            engine.dial_internal(session, &self.fallback).await
        })
    }

    /// The QUIC carrier of a DoQ / DoH3 upstream, routed as `connect` routes
    /// a TCP connection: an `Internal` UDP session. `as_direct` stays
    /// `None`, so the QUIC path always takes its carrier from here.
    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        Box::pin(async move {
            let ip = match &target.host {
                HostName::Ip(ip) => *ip,
                // Defensive only, as for `connect`.
                HostName::Domain(_) => return self.fallback.connect_udp(target, opts).await,
            };
            let engine = self.engine.get().and_then(Weak::upgrade);
            let Some(engine) = engine else {
                // Not attached yet (bootstrap phase) → direct.
                return self.fallback.connect_udp(target, opts).await;
            };
            let mut session = SessionInfo::udp(HostName::Ip(ip), target.port);
            session.listener = ListenerKind::Internal;
            session.protocol = Some(quic_protocol_for(target.port));
            engine.dial_internal_udp(session, &self.fallback).await
        })
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
/// session, and reaches the resolver under the policy's name.
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
    use rurge_config::HostName;
    use rurge_net::connector::{ConnectOpts, DirectConnector, SystemResolve, Target};
    use std::sync::Arc;
    use std::time::Duration;

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
        let target = Target::new(HostName::parse("127.0.0.1"), port);
        // `BoxedStream` is not `Debug`, so drop the stream from the Ok arm to
        // keep the assertion message printable.
        let res = tokio::time::timeout(
            Duration::from_secs(5),
            pc.connect(&target, &ConnectOpts::default()),
        )
        .await
        .map(|r| r.map(|_| ()));
        assert!(
            matches!(res, Ok(Ok(()))),
            "an unattached connector delegates to the fallback: {res:?}"
        );
    }

    #[test]
    fn protocol_tag_follows_the_port() {
        assert_eq!(protocol_for(853), ProtocolKind::Dot);
        assert_eq!(protocol_for(443), ProtocolKind::Doh);
        assert_eq!(protocol_for(53), ProtocolKind::Dns);
    }

    #[test]
    fn a_quic_carriers_tag_follows_the_port() {
        assert_eq!(quic_protocol_for(443), ProtocolKind::Doh3);
        assert_eq!(quic_protocol_for(853), ProtocolKind::Doq);
        assert_eq!(quic_protocol_for(8853), ProtocolKind::Doq);
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
        let target = Target::new(HostName::parse("127.0.0.1"), port);
        let carrier = tokio::time::timeout(
            Duration::from_secs(5),
            pc.connect_udp(&target, &ConnectOpts::default()),
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
}
