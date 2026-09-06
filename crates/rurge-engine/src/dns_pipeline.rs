//! `encrypted-dns-follow-outbound-mode` (M3 design §7.4): a connector that
//! routes DNS upstream connections through the dial pipeline. Only IP-literal
//! DNS servers take the pipeline — a domain DNS server would need resolving,
//! which is exactly the loop we must not create, so those fall back to the
//! bootstrap-direct connector. UDP upstreams never use a connector.

use crate::engine::Engine;
use rurge_config::HostName;
use rurge_config::rule::ProtocolKind;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_inbound::{Counting, SessionHandle, SessionOutcome};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use std::io;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, Weak};
use std::task::{Context, Poll};
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

    pub fn fallback(&self) -> &Arc<dyn Connector> {
        &self.fallback
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
}

/// Finishes the session handle when the DNS connection is dropped, so the
/// internal session leaves the active index and lands in the request log.
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

/// Wraps a dialed stream so bytes are counted (`Counting`) and the handle is
/// finished on drop (`FinishOnDrop`).
pub(crate) fn wrap_internal(stream: BoxedStream, handle: Arc<SessionHandle>) -> BoxedStream {
    Box::new(FinishOnDrop::new(
        Counting::new(stream, handle.clone()),
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
}
