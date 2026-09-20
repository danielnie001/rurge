//! A protocol's request head held back until the first payload write, so
//! both leave in one write: a lone head-sized first record is a known
//! traffic signature.
//!
//! A relay polls the read side the moment the tunnel exists, long before the
//! client's first bytes arrive, so a read must not send the head at once.
//! It waits `HEAD_GRACE` for a write instead; only an application that stays
//! silent that long (a server-speaks-first protocol: SSH, SMTP) gets the head
//! sent on its own. The write that sends the head wakes a reader parked on
//! the timer, so the grace never delays a response.

use rurge_net::connector::BoxedStream;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, Waker, ready};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

/// How long a read waits for the first payload before the head goes out alone.
pub const HEAD_GRACE: Duration = Duration::from_millis(100);

pub struct LazyHead {
    inner: BoxedStream,
    head: Option<Vec<u8>>,
    written: usize,
    coalesced: usize,
    grace: Duration,
    /// Armed by the first read that finds the head unsent.
    timer: Option<Pin<Box<Sleep>>>,
    /// A reader parked on the timer; woken as soon as a write sends the head.
    reader: Option<Waker>,
}

impl LazyHead {
    pub fn new(inner: BoxedStream, head: Vec<u8>) -> LazyHead {
        LazyHead::with_grace(inner, head, HEAD_GRACE)
    }

    pub fn with_grace(inner: BoxedStream, head: Vec<u8>, grace: Duration) -> LazyHead {
        LazyHead {
            inner,
            head: Some(head),
            written: 0,
            coalesced: 0,
            grace,
            timer: None,
            reader: None,
        }
    }

    fn poll_head(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while let Some(head) = &self.head {
            if self.written == head.len() {
                self.head = None;
                self.timer = None;
                if let Some(reader) = self.reader.take() {
                    reader.wake();
                }
                break;
            }
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &head[self.written..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.written += n;
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for LazyHead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.head.is_some() {
            if self.written == 0 {
                // nothing sent yet: give the first payload a moment to arrive
                let grace = self.grace;
                let timer = self
                    .timer
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(grace)));
                if timer.as_mut().poll(cx).is_pending() {
                    self.reader = Some(cx.waker().clone());
                    return Poll::Pending;
                }
            }
            ready!(self.poll_head(cx))?;
            ready!(Pin::new(&mut self.inner).poll_flush(cx))?;
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for LazyHead {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.head.is_none() {
            return Pin::new(&mut self.inner).poll_write(cx, data);
        }
        if self.written == 0 && self.coalesced == 0 {
            if let Some(head) = &mut self.head {
                head.extend_from_slice(data);
            }
            self.coalesced = data.len();
        }
        ready!(self.poll_head(cx))?;
        let n = self.coalesced.min(data.len());
        self.coalesced = 0;
        if n == 0 {
            return Pin::new(&mut self.inner).poll_write(cx, data);
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.head.is_some() {
            ready!(self.poll_head(cx))?;
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.head.is_some() {
            ready!(self.poll_head(cx))?;
        }
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// Accepts one connection; returns (what the first read got, the rest).
    /// Says `banner` right after the first read.
    async fn peer() -> (
        std::net::SocketAddr,
        tokio::task::JoinHandle<(Vec<u8>, Vec<u8>)>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut first = vec![0u8; 256];
            let n = tcp.read(&mut first).await.unwrap();
            first.truncate(n);
            // the client may already be gone
            let _ = tcp.write_all(b"banner").await;
            let mut rest = Vec::new();
            let _ = tcp.read_to_end(&mut rest).await;
            (first, rest)
        });
        (addr, task)
    }

    #[tokio::test]
    async fn the_head_leaves_with_the_first_payload() {
        let (addr, task) = peer().await;
        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut lazy = LazyHead::new(Box::new(tcp), b"HEAD|".to_vec());
        lazy.write_all(b"payload").await.unwrap();
        let mut banner = [0u8; 6];
        lazy.read_exact(&mut banner).await.unwrap();
        lazy.write_all(b"+more").await.unwrap();
        lazy.shutdown().await.unwrap();
        drop(lazy);
        let (first, rest) = task.await.unwrap();
        assert_eq!(first, b"HEAD|payload", "one write, so one segment");
        assert_eq!(rest, b"+more");
    }

    #[tokio::test]
    async fn a_read_that_is_already_pending_does_not_send_the_head_alone() {
        // what a relay does: the read half is polled first, the payload follows
        let (addr, task) = peer().await;
        let tcp = TcpStream::connect(addr).await.unwrap();
        // a grace far longer than the test: only the writer can release the reader
        let lazy = LazyHead::with_grace(Box::new(tcp), b"HEAD|".to_vec(), Duration::from_secs(30));
        let (mut rd, mut wr) = tokio::io::split(lazy);
        let reader = tokio::spawn(async move {
            let mut banner = [0u8; 6];
            rd.read_exact(&mut banner).await.unwrap();
            (rd, banner)
        });
        // lets the reader park itself first; whichever half runs first, the
        // result below must be the same
        tokio::time::sleep(Duration::from_millis(20)).await;
        wr.write_all(b"payload").await.unwrap();
        let (rd, banner) = tokio::time::timeout(Duration::from_secs(5), reader)
            .await
            .expect("the write released the parked reader, not the 30 s timer")
            .unwrap();
        assert_eq!(&banner, b"banner");
        let mut lazy = rd.unsplit(wr);
        lazy.shutdown().await.unwrap();
        drop(lazy);
        let (first, _) = task.await.unwrap();
        assert_eq!(
            first, b"HEAD|payload",
            "coalesced although a read was pending"
        );
    }

    #[tokio::test]
    async fn an_application_that_stays_silent_gets_the_head_out_after_the_grace() {
        // a server-speaks-first protocol (SSH, SMTP): without this the two
        // sides would wait for each other forever
        let (addr, task) = peer().await;
        let tcp = TcpStream::connect(addr).await.unwrap();
        let grace = Duration::from_millis(50);
        let mut lazy = LazyHead::with_grace(Box::new(tcp), b"HEAD|".to_vec(), grace);
        let started = std::time::Instant::now();
        let mut banner = [0u8; 6];
        tokio::time::timeout(Duration::from_secs(5), lazy.read_exact(&mut banner))
            .await
            .expect("the head went out, so the peer answered")
            .unwrap();
        assert_eq!(&banner, b"banner");
        assert!(started.elapsed() >= grace, "the grace was honoured");
        lazy.write_all(b"later").await.unwrap();
        lazy.shutdown().await.unwrap();
        drop(lazy);
        let (first, rest) = task.await.unwrap();
        assert_eq!(
            (first.as_slice(), rest.as_slice()),
            (&b"HEAD|"[..], &b"later"[..])
        );
    }

    #[tokio::test]
    async fn a_shutdown_before_anything_else_still_sends_the_head() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut all = Vec::new();
            tcp.read_to_end(&mut all).await.unwrap();
            all
        });
        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut lazy = LazyHead::new(Box::new(tcp), b"HEAD|".to_vec());
        lazy.shutdown().await.unwrap();
        drop(lazy);
        assert_eq!(server.await.unwrap(), b"HEAD|");
    }
}
