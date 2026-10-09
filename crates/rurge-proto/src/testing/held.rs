//! Hops below an outbound's own handshake, for the tests of what the
//! handshake does to the stream it is given: one that keeps what is written
//! to it until it is flushed, one whose far end is gone already, and one that
//! fails in words of its own.

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

/// What a connection of `Breaks` fails at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Read,
    Write,
    Flush,
}

/// Every connection fails at `stage` with an error of `kind` and `text` for
/// its words, as a hop below does that knows what went wrong: a policy that
/// keeps the kind of the error and says why (`ss: the server reset the
/// connection …` is a reset), or a TLS layer reporting an alert it received
/// (`InvalidData`). Everything else goes through: writes vanish, and a read
/// finds the end.
pub(crate) struct Breaks {
    pub(crate) stage: Stage,
    pub(crate) kind: io::ErrorKind,
    pub(crate) text: &'static str,
}

impl Connector for Breaks {
    fn connect<'a>(
        &'a self,
        _: &'a Target,
        _: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        let stream = Broken {
            stage: self.stage,
            kind: self.kind,
            text: self.text,
        };
        Box::pin(std::future::ready(Ok(Box::new(stream) as BoxedStream)))
    }
}

struct Broken {
    stage: Stage,
    kind: io::ErrorKind,
    text: &'static str,
}

impl Broken {
    /// The failure, when this is the stage it was set for.
    fn at(&self, stage: Stage) -> io::Result<()> {
        if self.stage == stage {
            Err(io::Error::new(self.kind, self.text))
        } else {
            Ok(())
        }
    }
}

impl AsyncRead for Broken {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(self.at(Stage::Read))
    }
}

impl AsyncWrite for Broken {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(self.at(Stage::Write).map(|()| data.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.at(Stage::Flush))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
