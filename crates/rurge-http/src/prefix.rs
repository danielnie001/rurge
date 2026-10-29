//! The prefix stream: a stream of which the first bytes were read to be looked
//! at, kept so that whoever reads the connection next gets every byte the peer
//! sent, in order (phase 4 M1 design 6.1).

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};

/// The most one `Prefixed::fill` reads (P13 of the phase 4 / M1a plan): the
/// prefix is filled a block at a time, so that a session that sends a few
/// bytes costs a few bytes and no read of a session's buffer is unbounded.
pub const FILL: usize = 16 * 1024;

/// A stream whose first bytes were read to be looked at (phase 4 M1 design
/// 6.1). They stay in a buffer, and reads give them back before anything more
/// of the stream, so whoever reads it next gets every byte the peer sent, in
/// order. Writes and the shutdown go straight through.
pub struct Prefixed<S> {
    inner: S,
    buf: Vec<u8>,
    /// How much of `buf` was already handed back to a reader.
    pos: usize,
}

impl<S> Prefixed<S> {
    pub fn new(inner: S) -> Prefixed<S> {
        Prefixed {
            inner,
            buf: Vec::new(),
            pos: 0,
        }
    }

    /// What was read and has not been read back yet.
    pub fn buffered(&self) -> &[u8] {
        &self.buf[self.pos..]
    }
}

impl<S: AsyncRead + Unpin> Prefixed<S> {
    /// Reads the stream once more, at most [`FILL`] bytes, onto the end of the
    /// buffer: how many came, 0 at the end of the stream.
    pub async fn fill(&mut self) -> io::Result<usize> {
        // The read goes into the spare capacity of the buffer rather than
        // into an array on the stack: an array here would be part of every
        // session's future. `take` bounds the one read, not the buffer.
        self.buf.reserve(FILL);
        (&mut self.inner)
            .take(FILL as u64)
            .read_buf(&mut self.buf)
            .await
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        read_buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let left = this.buf.len() - this.pos;
        if left == 0 {
            return Pin::new(&mut this.inner).poll_read(cx, read_buf);
        }

        let n = left.min(read_buf.remaining());
        read_buf.put_slice(&this.buf[this.pos..this.pos + n]);
        this.pos += n;
        if this.pos == this.buf.len() {
            // The prefix is all handed back: let the memory go rather than
            // keep it for as long as the connection lives (P13).
            this.buf = Vec::new();
            this.pos = 0;
        }
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};

    /// Fills until the prefix holds at least `n` bytes: one `fill` asks for
    /// every byte the peer wrote, but a read may come back short.
    async fn fill_until(prefix: &mut Prefixed<DuplexStream>, n: usize) {
        while prefix.buffered().len() < n {
            let got = prefix.fill().await.expect("fill");
            assert!(got > 0, "the peer closed before {n} bytes arrived");
        }
    }

    #[tokio::test]
    async fn the_prefix_is_read_before_the_stream() {
        let (client, mut peer) = duplex(64);
        peer.write_all(b"hello").await.unwrap();
        let mut prefix = Prefixed::new(client);
        fill_until(&mut prefix, 5).await;
        assert_eq!(prefix.buffered(), b"hello");

        peer.write_all(b"world").await.unwrap();
        peer.shutdown().await.unwrap();
        let mut rest = Vec::new();
        prefix.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"helloworld");
    }

    #[tokio::test]
    async fn a_small_read_takes_the_prefix_in_pieces() {
        let (client, mut peer) = duplex(64);
        peer.write_all(b"0123456789").await.unwrap();
        let mut prefix = Prefixed::new(client);
        fill_until(&mut prefix, 10).await;
        // Written before the prefix is handed back: a read that still has
        // prefix bytes must not reach the stream and take these too.
        peer.write_all(b"later").await.unwrap();

        let mut buf = [0u8; 3];
        for want in [&b"012"[..], &b"345"[..], &b"678"[..], &b"9"[..]] {
            let n = prefix.read(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], want);
        }
        let n = prefix.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"lat");
    }

    #[tokio::test]
    async fn writes_and_the_shutdown_go_straight_through() {
        let (client, mut peer) = duplex(64);
        let mut prefix = Prefixed::new(client);
        prefix.write_all(b"out").await.unwrap();
        prefix.flush().await.unwrap();
        let mut got = [0u8; 3];
        peer.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"out");

        prefix.shutdown().await.unwrap();
        let mut rest = Vec::new();
        peer.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty(), "the peer read {rest:?} after the shutdown");
    }

    #[tokio::test]
    async fn fill_at_the_end_reads_nothing() {
        let (client, mut peer) = duplex(64);
        peer.write_all(b"abc").await.unwrap();
        let mut prefix = Prefixed::new(client);
        fill_until(&mut prefix, 3).await;

        peer.shutdown().await.unwrap();
        assert_eq!(prefix.fill().await.unwrap(), 0);
        assert_eq!(prefix.buffered(), b"abc");
    }
}
