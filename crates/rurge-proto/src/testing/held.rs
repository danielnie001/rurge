//! Hops below an outbound's own handshake, for the tests of what the
//! handshake does to the stream it is given: one that keeps what is written
//! to it until it is flushed, and one whose far end is gone already.

use rurge_net::BoxFuture;
use rurge_net::connector::{
    BoxedStream, ConnectOpts, Connector, DirectConnector, SystemResolve, Target,
};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Takes every write at once and passes it on only when flushed: what
/// `AsyncWrite` allows any stream to do, and what a hop that buffers does (an
/// `underlying-proxy` below this one). A request that counts on a later write
/// to push it out never leaves.
pub(crate) struct HoldsUntilFlushed {
    inner: BoxedStream,
    held: Vec<u8>,
}

impl HoldsUntilFlushed {
    pub(crate) fn new(inner: BoxedStream) -> HoldsUntilFlushed {
        HoldsUntilFlushed {
            inner,
            held: Vec::new(),
        }
    }
}

impl AsyncRead for HoldsUntilFlushed {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for HoldsUntilFlushed {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.held.extend_from_slice(data);
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        while !this.held.is_empty() {
            let n = ready!(Pin::new(&mut this.inner).poll_write(cx, &this.held))?;
            this.held.drain(..n);
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.as_mut().poll_flush(cx))?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Dials directly; every connection holds what it is given until it is
/// flushed.
pub(crate) struct HoldingConnector(DirectConnector);

impl HoldingConnector {
    pub(crate) fn new() -> Arc<HoldingConnector> {
        Arc::new(HoldingConnector(DirectConnector::new(Arc::new(
            SystemResolve,
        ))))
    }
}

impl Connector for HoldingConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
            let inner = self.0.connect(target, opts).await?;
            Ok(Box::new(HoldsUntilFlushed::new(inner)) as BoxedStream)
        })
    }
}

/// Every connection's far end is gone already: writing to it is a broken
/// pipe, and reading from it the end. `held`: behind a hop that writes only
/// when flushed, so that the flush is what finds out.
pub(crate) struct Gone {
    pub(crate) held: bool,
}

impl Connector for Gone {
    fn connect<'a>(
        &'a self,
        _: &'a Target,
        _: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        let (near, _far) = tokio::io::duplex(64);
        let near: BoxedStream = Box::new(near);
        let stream = if self.held {
            Box::new(HoldsUntilFlushed::new(near))
        } else {
            near
        };
        Box::pin(std::future::ready(Ok(stream)))
    }
}
