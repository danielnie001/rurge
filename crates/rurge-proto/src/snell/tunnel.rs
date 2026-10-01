//! One request on a Snell connection (phase 2 M6 design 4.3): the request
//! head waits in a `LazyHead` for the first payload, and the server's
//! answer — the first byte of its first payload of this request — is read
//! in front of the data:
//!
//! - `00` (tunnel): the rest is the target's data;
//! - `02 code length message` (error): `snell: the server refused: …`;
//! - anything else is an error.
//!
//! The server answers only once the target has sent something, so the
//! answer surfaces at the first read, never before a write.
//!
//! With `reuse=true` the application's shutdown sends only our empty
//! record, and a request whose two sides both ended cleanly hands its
//! connection back to the pool when dropped. Dropped earlier, the
//! connection finishes in the background: our end, then the server's data
//! discarded (at most `MAX_DISCARD` bytes, as Surge) up to its end.
//!
//! A pooled connection may have been closed by the server while it idled.
//! A request on one that fails before any answer arrived — a write error, or
//! a read that ends or fails — goes again, once, on a fresh connection,
//! with the head and every byte written so far (at most `MAX_REPLAY`).

use super::Dialer;
use super::pool::Pool;
use super::record::SnellStream;
use crate::OutboundError;
use crate::transport::lazy_head::LazyHead;
use rurge_net::BoxFuture;
use rurge_net::connector::ConnectOpts;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, ready};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::time::Instant;

pub(super) const TUNNEL: u8 = 0x00;
pub(super) const ERROR: u8 = 0x02;

/// The longest part of an error message that is quoted.
const MAX_MESSAGE: usize = 200;
/// What a request may have written before its answer and still go again
/// on a fresh connection.
const MAX_REPLAY: usize = 64 * 1024;
/// How long after leaving the pool a failure may still be a stale socket: one
/// that the server closed while it idled fails within about a round trip of
/// the first write. Later the server may already have forwarded the request,
/// and sending it again could deliver it twice.
pub(super) const STALE_WINDOW: Duration = Duration::from_secs(1);
/// Surge's limit on the server's data discarded while waiting for its end.
const MAX_DISCARD: usize = 0x80001;
/// How long a dropped request's connection may take to finish cleanly.
const FINISH_TIMEOUT: Duration = Duration::from_secs(10);

const NO_ANSWER: &str = "snell: the server closed the connection without answering";
pub(super) const UNKNOWN_REPLY: &str = "snell: the server answered with an unknown reply";

pub(super) fn no_answer() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, NO_ANSWER)
}

/// `code length message`, complete: the error for the application. The
/// message comes from the far end: printable ASCII only, and bounded.
pub(super) fn refused(answer: &[u8]) -> io::Error {
    let message: String = answer[2..]
        .iter()
        .filter(|b| b.is_ascii_graphic() || **b == b' ')
        .take(MAX_MESSAGE)
        .map(|b| char::from(*b))
        .collect();
    let message = message.trim();
    let text = if message.is_empty() {
        format!("snell: the server refused: error {}", answer[0])
    } else {
        format!("snell: the server refused: {message}")
    };
    io::Error::new(io::ErrorKind::ConnectionRefused, text)
}

enum Conn {
    /// Boxed: the stream's buffers are large and a redial is rare.
    Open(Box<LazyHead<SnellStream>>),
    /// The request going again on a fresh connection: the dial, and what
    /// the old one carried (the head and the payload written since).
    /// The tunnel assumes both halves are polled from one task: the redial
    /// future keeps only the last poller's waker.
    Redial(BoxFuture<'static, io::Result<SnellStream>>, Vec<u8>),
    /// The redial failed.
    Gone,
}

enum Reply {
    Waiting,
    /// An error answer: its code, message length and message so far.
    Refused(Vec<u8>),
    Tunnel,
}

/// A pooled connection's request that may still go again.
struct Retry {
    /// When the connection left the pool.
    taken: Instant,
    /// The head and every payload byte written since.
    sent: Vec<u8>,
    dialer: Arc<Dialer>,
    opts: ConnectOpts,
}

/// No `Debug`: it holds the connection's keys.
pub(crate) struct SnellTunnel {
    conn: Conn,
    reply: Reply,
    /// `Some` while the request may go again on a fresh connection.
    retry: Option<Retry>,
    /// The application shut its side down: a fresh connection owes our end too.
    shut: bool,
    /// Our end still has to go out on the current connection.
    end_owed: bool,
    /// The connection is not in a state to be reused.
    broken: bool,
    /// `reuse=true`: where the connection goes after a clean end.
    pool: Option<Weak<Pool>>,
}

impl SnellTunnel {
    /// A request on a connection of its own, pooled afterwards with `pool`.
    pub(crate) fn new(stream: SnellStream, head: Vec<u8>, pool: Option<Weak<Pool>>) -> SnellTunnel {
        SnellTunnel {
            conn: Conn::Open(Box::new(LazyHead::new(stream, head))),
            reply: Reply::Waiting,
            retry: None,
            shut: false,
            end_owed: false,
            broken: false,
            pool,
        }
    }

    /// A request on a connection from `pool`: it may go again once, on a
    /// connection from `dialer`.
    pub(crate) fn reused(
        stream: SnellStream,
        head: Vec<u8>,
        pool: Weak<Pool>,
        dialer: Arc<Dialer>,
        opts: ConnectOpts,
    ) -> SnellTunnel {
        let retry = Retry {
            taken: Instant::now(),
            sent: head.clone(),
            dialer,
            opts,
        };
        let mut tunnel = SnellTunnel::new(stream, head, Some(pool));
        tunnel.retry = Some(retry);
        tunnel
    }

    /// `error` ended the current connection's request: `Ok` when it goes
    /// again on a fresh connection, else the error, final.
    fn fail(&mut self, error: io::Error) -> io::Result<()> {
        let retry = self
            .retry
            .take()
            .filter(|retry| retry.taken.elapsed() < STALE_WINDOW);
        let Some(retry) = retry else {
            self.broken = true;
            return Err(error);
        };
        let Retry {
            sent, dialer, opts, ..
        } = retry;
        let dial = Box::pin(async move {
            match tokio::time::timeout(opts.timeout, dialer.fresh(&opts)).await {
                Ok(Ok(stream)) => Ok(stream),
                Ok(Err(OutboundError::Io(e))) => Err(e),
                Ok(Err(e)) => Err(io::Error::other(e.to_string())),
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "snell: connecting again timed out",
                )),
            }
        });
        self.conn = Conn::Redial(dial, sent);
        Ok(())
    }

    /// The current connection, once a redial has finished.
    fn poll_open(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.conn {
            Conn::Open(_) => Poll::Ready(Ok(())),
            Conn::Redial(dial, sent) => match ready!(dial.as_mut().poll(cx)) {
                Ok(stream) => {
                    // no grace: the application's bytes are already here
                    let sent = std::mem::take(sent);
                    self.conn =
                        Conn::Open(Box::new(LazyHead::with_grace(stream, sent, Duration::ZERO)));
                    self.end_owed = self.shut;
                    Poll::Ready(Ok(()))
                }
                Err(e) => {
                    self.conn = Conn::Gone;
                    self.broken = true;
                    Poll::Ready(Err(e))
                }
            },
            Conn::Gone => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "snell: the connection is gone",
            ))),
        }
    }

    fn lazy(&mut self) -> &mut LazyHead<SnellStream> {
        match &mut self.conn {
            Conn::Open(lazy) => lazy,
            _ => unreachable!("after poll_open"),
        }
    }

    /// Sends our end: the empty record alone when the connection may be
    /// reused, else the connection's shutdown too.
    fn poll_end(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let reuse = self.pool.is_some();
        let lazy = self.lazy();
        if !reuse {
            return Pin::new(lazy).poll_shutdown(cx);
        }
        ready!(Pin::new(&mut *lazy).poll_flush(cx))?;
        lazy.get_mut().poll_end(cx)
    }

    /// The answer, read ahead of the data. `Ready(Ok)` once it said tunnel.
    fn poll_reply(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            let need = match &self.reply {
                Reply::Tunnel => return Poll::Ready(Ok(())),
                Reply::Waiting => 1,
                Reply::Refused(got) if got.len() < 2 => 2 - got.len(),
                Reply::Refused(got) => 2 + usize::from(got[1]) - got.len(),
            };
            let mut space = [0u8; 256];
            let mut buf = ReadBuf::new(&mut space[..need]);
            let read = ready!(Pin::new(self.lazy()).poll_read(cx, &mut buf));
            let got = buf.filled();
            match (read, &mut self.reply) {
                (Err(e), _) => return Poll::Ready(Err(e)),
                (Ok(()), _) if got.is_empty() => return Poll::Ready(Err(no_answer())),
                (Ok(()), Reply::Waiting) => match got[0] {
                    TUNNEL => {
                        self.reply = Reply::Tunnel;
                        self.retry = None;
                    }
                    ERROR => {
                        self.reply = Reply::Refused(Vec::new());
                        self.retry = None;
                    }
                    _ => {
                        self.retry = None;
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            UNKNOWN_REPLY,
                        )));
                    }
                },
                (Ok(()), Reply::Refused(answer)) => {
                    answer.extend_from_slice(got);
                    if answer.len() >= 2 && answer.len() == 2 + usize::from(answer[1]) {
                        return Poll::Ready(Err(refused(answer)));
                    }
                }
                (Ok(()), Reply::Tunnel) => unreachable!("returned above"),
            }
        }
    }

    /// `f` on the current connection; a failure that lets the request go
    /// again starts the redial and tries again.
    fn drive<T>(
        &mut self,
        cx: &mut Context<'_>,
        mut f: impl FnMut(&mut SnellTunnel, &mut Context<'_>) -> Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        loop {
            ready!(self.poll_open(cx))?;
            if self.end_owed {
                match ready!(self.poll_end(cx)) {
                    Ok(()) => self.end_owed = false,
                    Err(e) => {
                        self.fail(e)?;
                        continue;
                    }
                }
            }
            match ready!(f(self, cx)) {
                Ok(value) => return Poll::Ready(Ok(value)),
                Err(e) => self.fail(e)?,
            }
        }
    }
}

impl AsyncRead for SnellTunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.drive(cx, |this, cx| this.poll_reply(cx)))?;
        let read = ready!(Pin::new(this.lazy()).poll_read(cx, buf));
        if read.is_err() {
            this.broken = true;
        }
        Poll::Ready(read)
    }
}

impl AsyncWrite for SnellTunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.shut {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        let n = ready!(this.drive(cx, |this, cx| Pin::new(this.lazy()).poll_write(cx, data)))?;
        if let Some(retry) = &mut this.retry {
            if retry.sent.len() + n > MAX_REPLAY {
                this.retry = None;
            } else {
                retry.sent.extend_from_slice(&data[..n]);
            }
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut()
            .drive(cx, |this, cx| Pin::new(this.lazy()).poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.shut {
            this.shut = true;
            this.end_owed = true;
        }
        // `drive` sends the end owed
        this.drive(cx, |_, _| Poll::Ready(Ok(())))
    }
}

impl Drop for SnellTunnel {
    fn drop(&mut self) {
        let Some(pool) = self.pool.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        // without the tunnel answer, or after an error, where the
        // connection stands is unknown
        if self.broken || !matches!(self.reply, Reply::Tunnel) {
            return;
        }
        let Conn::Open(lazy) = std::mem::replace(&mut self.conn, Conn::Gone) else {
            return;
        };
        let stream = (*lazy).into_inner();
        if stream.is_reusable() {
            pool.put(stream);
        } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(finish(stream, Arc::downgrade(&pool)));
        }
    }
}

/// Ends our side (once more is harmless), then reads the server's side to
/// its end; a connection that gets there cleanly goes back to `pool`.
async fn finish(mut stream: SnellStream, pool: Weak<Pool>) {
    let ended = tokio::time::timeout(FINISH_TIMEOUT, async {
        std::future::poll_fn(|cx| stream.poll_end(cx)).await?;
        let mut sink = vec![0u8; 16 * 1024];
        let mut discarded = 0;
        loop {
            // 0: the server's end, or its close (`is_reusable` tells them apart)
            let n = stream.read(&mut sink).await?;
            if n == 0 {
                return Ok(());
            }
            discarded += n;
            if discarded > MAX_DISCARD {
                return Err(io::Error::other("snell: too much data after our end"));
            }
        }
    })
    .await;
    if matches!(ended, Ok(Ok(())))
        && stream.is_reusable()
        && let Some(pool) = pool.upgrade()
    {
        pool.put(stream);
    }
}

#[cfg(test)]
mod tests {
    use super::super::kdf::Psk;
    use super::*;
    use tokio::io::AsyncWriteExt;

    /// A request on one end of a pipe and the server's stream on the other
    /// (the record format is symmetric).
    async fn pair(pool: Option<Weak<Pool>>) -> (SnellTunnel, SnellStream) {
        let (near, far) = tokio::io::duplex(64 * 1024);
        let client = SnellStream::open(Box::new(near), Psk::new("psk"))
            .await
            .unwrap();
        let server = SnellStream::open(Box::new(far), Psk::new("psk"))
            .await
            .unwrap();
        (SnellTunnel::new(client, b"HEAD".to_vec(), pool), server)
    }

    /// What the tunnel reads after the server sent `records`, one write each.
    async fn answered(records: &[&[u8]]) -> io::Result<Vec<u8>> {
        let (mut tunnel, mut server) = pair(None).await;
        tunnel.write_all(b"ping").await.unwrap();
        let mut request = [0u8; 8];
        server.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"HEADping", "one record");
        for record in records {
            server.write_all(record).await.unwrap();
        }
        server.shutdown().await.unwrap();
        let mut got = Vec::new();
        tunnel.read_to_end(&mut got).await.map(|_| got)
    }

    #[tokio::test]
    async fn the_tunnel_answer_is_taken_off_the_data() {
        assert_eq!(answered(&[b"\x00pong"]).await.unwrap(), b"pong");
        assert_eq!(answered(&[b"\x00", b"po", b"ng"]).await.unwrap(), b"pong");
        assert_eq!(answered(&[b"\x00"]).await.unwrap(), b"");
    }

    #[tokio::test]
    async fn an_error_answer_quotes_the_servers_message_made_safe() {
        let text = |result: io::Result<Vec<u8>>| result.unwrap_err().to_string();
        assert_eq!(
            text(answered(&[b"\x02\x65\x0aRemote EOF"]).await),
            "snell: the server refused: Remote EOF"
        );
        // cut over several records
        assert_eq!(
            text(answered(&[b"\x02", b"\x01\x03", b"a", b"bc"]).await),
            "snell: the server refused: abc"
        );
        assert_eq!(
            text(answered(&[b"\x02\x07\x00"]).await),
            "snell: the server refused: error 7"
        );
        let mut long = vec![0x02, 0x01, 255, 0x1b];
        long.extend_from_slice(b"[31m");
        long.extend(std::iter::repeat_n(b'x', 250));
        assert_eq!(
            text(answered(&[&long]).await),
            format!("snell: the server refused: [31m{}", "x".repeat(196))
        );
    }

    #[tokio::test]
    async fn anything_else_is_an_unknown_answer_or_none() {
        let err = answered(&[b"\x07data"]).await.unwrap_err();
        assert_eq!(err.to_string(), UNKNOWN_REPLY);
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        // the server's end without an answer
        let err = answered(&[]).await.unwrap_err();
        assert_eq!(err.to_string(), NO_ANSWER);
    }

    #[tokio::test]
    async fn only_a_request_that_ended_cleanly_both_ways_is_pooled() {
        let pool = Arc::new(Pool::default());
        // both ends: pooled at once
        let (mut tunnel, mut server) = pair(Some(Arc::downgrade(&pool))).await;
        tunnel.write_all(b"ping").await.unwrap();
        tunnel.shutdown().await.unwrap();
        let mut request = Vec::new();
        server.read_to_end(&mut request).await.unwrap();
        assert_eq!(request, b"HEADping");
        server.write_all(b"\x00pong").await.unwrap();
        std::future::poll_fn(|cx| server.poll_end(cx))
            .await
            .unwrap();
        let mut got = Vec::new();
        tunnel.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"pong");
        drop(tunnel);
        assert_eq!(pool.len(), 1);
        // no answer yet: dropped
        let (mut tunnel, _server) = pair(Some(Arc::downgrade(&pool))).await;
        tunnel.write_all(b"ping").await.unwrap();
        drop(tunnel);
        // a refusal: dropped
        let (mut tunnel, mut server) = pair(Some(Arc::downgrade(&pool))).await;
        tunnel.write_all(b"ping").await.unwrap();
        server.write_all(b"\x02\x01\x00").await.unwrap();
        let mut buf = [0u8; 4];
        assert!(tunnel.read(&mut buf).await.is_err());
        drop(tunnel);
        assert_eq!(pool.len(), 1);
    }
}
