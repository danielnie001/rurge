//! A stream that first yields bytes a handshake read past its own end.

use rurge_net::connector::BoxedStream;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct Prefixed<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S> Prefixed<S> {
    pub fn new(prefix: Vec<u8>, inner: S) -> Prefixed<S> {
        Prefixed {
            prefix,
            pos: 0,
            inner,
        }
    }
}

/// `inner` itself when there is nothing to put in front of it.
pub fn boxed(prefix: Vec<u8>, inner: BoxedStream) -> BoxedStream {
    if prefix.is_empty() {
        inner
    } else {
        Box::new(Prefixed::new(prefix, inner))
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.pos < self.prefix.len() {
            let n = (self.prefix.len() - self.pos).min(buf.remaining());
            let start = self.pos;
            buf.put_slice(&self.prefix[start..start + n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn the_prefix_is_read_first_and_writes_pass_through() {
        let (client, mut server) = tokio::io::duplex(64);
        let mut stream = boxed(b"early ".to_vec(), Box::new(client));
        server.write_all(b"late").await.unwrap();
        let mut buf = [0u8; 10];
        let mut got = Vec::new();
        while got.len() < 10 {
            let n = stream.read(&mut buf[..3]).await.unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, b"early late");
        stream.write_all(b"ping").await.unwrap();
        let mut echo = [0u8; 4];
        server.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"ping");
    }
}
