//! `h2-connect` UDP (phase 2 M6 design 5.3, M6-D8): CONNECT-UDP (RFC 9298)
//! over HTTP/2, one stream per target, each datagram one DATAGRAM capsule.
//! A stream is bound to its target, so every datagram it brings back counts
//! as the target's (symmetric, as VMess UDP). A stream is opened with its
//! target's first datagram, on the outbound's pooled connections; one that
//! ends is opened again by the next datagram.

use super::{Inner, LABEL};
use crate::OutboundError;
use crate::connect_udp::{self, MAX_PAYLOAD};
use crate::h2pool::H2Stream;
use crate::task::AbortOnDrop;
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, PacketSocket, Target};
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader, WriteHalf};
use tokio::sync::{Mutex, OnceCell, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Answers waiting for the engine; more are dropped, as a socket would.
const INBOX: usize = 64;

/// A target's stream closes after this long with neither a read nor a
/// write: the engine's flow idle (M5-D7), as VMess UDP. Its place on the
/// pooled connection is freed with it.
pub(super) const IDLE: Duration = Duration::from_secs(60);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

type Streams = std::sync::Mutex<HashMap<Target, Slot>>;

/// One target's stream.
struct Stream {
    id: u64,
    /// The last read or write; the idle clock.
    last: Arc<std::sync::Mutex<Instant>>,
    writer: Mutex<WriteHalf<H2Stream>>,
    /// Fires when the stream's reading ends: it is opened again.
    ended: CancellationToken,
    _reader: AbortOnDrop,
}

type Slot = Arc<OnceCell<Arc<Stream>>>;

pub(super) struct H2Udp {
    inner: Arc<Inner>,
    /// The server's own `:authority`.
    authority: String,
    opts: ConnectOpts,
    idle: Duration,
    streams: Arc<Streams>,
    answers: mpsc::Sender<(Vec<u8>, Target)>,
    inbox: Mutex<mpsc::Receiver<(Vec<u8>, Target)>>,
}

impl H2Udp {
    pub(super) fn new(
        inner: Arc<Inner>,
        authority: String,
        opts: ConnectOpts,
        idle: Duration,
    ) -> H2Udp {
        let (answers, inbox) = mpsc::channel(INBOX);
        H2Udp {
            inner,
            authority,
            opts,
            idle,
            streams: Arc::default(),
            answers,
            inbox: Mutex::new(inbox),
        }
    }

    /// `to`'s slot; a slot whose stream has ended is replaced.
    fn slot(&self, to: &Target) -> Slot {
        let mut streams = self.streams.lock().expect("streams");
        let slot = streams.entry(to.clone()).or_default();
        if slot.get().is_some_and(|s| s.ended.is_cancelled()) {
            *slot = Slot::default();
        }
        slot.clone()
    }

    #[cfg(test)]
    pub(super) fn tracked(&self) -> usize {
        self.streams.lock().expect("streams").len()
    }

    /// Forgets `slot` for `to`, unless another has taken its place.
    fn forget(&self, to: &Target, slot: &Slot) {
        let mut streams = self.streams.lock().expect("streams");
        if streams.get(to).is_some_and(|s| Arc::ptr_eq(s, slot)) {
            streams.remove(to);
        }
    }

    /// `to`'s stream and the slot it is in: `open` opens one when there is
    /// none, and one more when that one has ended already.
    async fn stream<F, Fut>(&self, to: &Target, open: F) -> io::Result<(Slot, Arc<Stream>)>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = io::Result<Arc<Stream>>>,
    {
        let mut reopened = false;
        loop {
            let slot = self.slot(to);
            let stream = match slot.get_or_try_init(&open).await {
                Ok(stream) => stream.clone(),
                Err(e) => {
                    self.forget(to, &slot);
                    return Err(e);
                }
            };
            // A sender waiting on a cell whose opener failed opens
            // again, after that opener forgot the slot: the stream must
            // be reachable from the table or its answers are lost.
            let mut streams = self.streams.lock().expect("streams");
            // an ended stream is not revived: open again
            if stream.ended.is_cancelled() {
                // once only: a proxy may end every stream at once
                if reopened {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "h2-connect: the server ended the UDP stream",
                    ));
                }
                reopened = true;
                continue;
            }
            match streams.get(to) {
                None => {
                    streams.insert(to.clone(), slot.clone());
                    return Ok((slot, stream));
                }
                Some(s) if Arc::ptr_eq(s, &slot) => return Ok((slot, stream)),
                // another slot took its place: use that one
                Some(_) => {}
            }
        }
    }

    async fn open(&self, to: &Target) -> io::Result<Arc<Stream>> {
        // one budget for the connection (when one is dialed), TLS, the
        // HTTP/2 handshake and the CONNECT-UDP exchange
        let opened = tokio::time::timeout(
            self.opts.timeout,
            self.inner.udp_stream(&self.authority, to, &self.opts),
        )
        .await
        .unwrap_or(Err(OutboundError::Timeout));
        let stream = opened.map_err(|e| io::Error::other(e.to_string()))?;
        let (reader, writer) = tokio::io::split(stream);
        let ended = CancellationToken::new();
        let (answers, from, done) = (self.answers.clone(), to.clone(), ended.clone());
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let last = Arc::new(std::sync::Mutex::new(Instant::now()));
        let (clock, idle, streams) = (last.clone(), self.idle, Arc::downgrade(&self.streams));
        let task = tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            let why = 'stream: loop {
                // a capsule is read whole: the idle clock must not cut one
                // in two, so the read lives on while the deadline moves
                let mut next = std::pin::pin!(connect_udp::read_datagram(&mut reader, LABEL));
                let read = loop {
                    let deadline = *clock.lock().expect("clock") + idle;
                    tokio::select! {
                        read = &mut next => break read,
                        // a write may have moved the deadline: look again
                        _ = tokio::time::sleep_until(deadline) => {
                            if *clock.lock().expect("clock") + idle <= Instant::now() {
                                break 'stream "idle".to_string();
                            }
                        }
                    }
                };
                match read {
                    Ok(None) => break "the server ended it".to_string(),
                    // the text names the fault, never the payload
                    Err(e) => break format!("read error: {e}"),
                    Ok(Some(datagram)) => {
                        *clock.lock().expect("clock") = Instant::now();
                        // a full inbox drops the answer
                        let _ = answers.try_send((datagram, from.clone()));
                    }
                }
            };
            tracing::debug!("h2-connect: a UDP stream ended: {why}");
            done.cancel();
            // leave the table, unless a newer stream has taken the place
            if let Some(streams) = Weak::upgrade(&streams) {
                let mut streams = streams.lock().expect("streams");
                if streams
                    .get(&from)
                    .is_some_and(|s| s.get().is_some_and(|s| s.id == id))
                {
                    streams.remove(&from);
                }
            }
        });
        Ok(Arc::new(Stream {
            id,
            last,
            writer: Mutex::new(writer),
            ended,
            _reader: AbortOnDrop(task),
        }))
    }
}

impl PacketSocket for H2Udp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // one datagram, one capsule (RFC 9298 5)
            if buf.len() > MAX_PAYLOAD {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("h2-connect: a datagram longer than {MAX_PAYLOAD} bytes"),
                ));
            }
            let (slot, stream) = self.stream(to, || self.open(to)).await?;
            *stream.last.lock().expect("clock") = Instant::now();
            let capsule = connect_udp::datagram(buf);
            let mut writer = stream.writer.lock().await;
            if let Err(e) = writer.write_all(&capsule).await {
                self.forget(to, &slot);
                return Err(e);
            }
            Ok(())
        })
    }

    /// An answer longer than `buf` is dropped: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let mut inbox = self.inbox.lock().await;
            loop {
                // `self` holds a sender: the inbox never closes
                let Some((answer, from)) = inbox.recv().await else {
                    return Err(io::ErrorKind::BrokenPipe.into());
                };
                if let Some(space) = buf.get_mut(..answer.len()) {
                    space.copy_from_slice(&answer);
                    return Ok((answer.len(), from));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h2connect::tests::{outbound, target, udp_fake};
    use crate::testing::{H2ProxyScript, udp_echo_server};
    use std::sync::atomic::AtomicUsize;

    /// A proxy that answers 200 and ends every stream at once: a datagram
    /// that finds its stream ended opens one more, and fails when that one
    /// has ended too. The loop is handed each stream after its reader has
    /// seen the end: on its own it may look before that.
    #[tokio::test]
    async fn a_stream_that_has_ended_is_opened_again_only_once() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = udp_fake(H2ProxyScript {
            udp_end_at_once: true,
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let authority = out.inner.udp_authority.clone().expect("`udp-relay=true`");
        let udp = H2Udp::new(out.inner.clone(), authority, ConnectOpts::default(), IDLE);
        let to = target(echo);
        let opens = AtomicUsize::new(0);
        let ended = || async {
            let n = opens.fetch_add(1, Ordering::Relaxed) + 1;
            assert!(n <= 2, "opened a third time");
            let stream = udp.open(&to).await?;
            stream.ended.cancelled().await;
            Ok(stream)
        };
        let found = tokio::time::timeout(Duration::from_secs(10), udp.stream(&to, ended))
            .await
            .expect("an end within the bound");
        let err = found.map(drop).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(
            err.to_string(),
            "h2-connect: the server ended the UDP stream"
        );
        assert_eq!(opens.load(Ordering::Relaxed), 2);
        assert_eq!(fake.requests().len(), 2);
    }
}
