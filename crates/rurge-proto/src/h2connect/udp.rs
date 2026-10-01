//! `h2-connect` UDP (phase 2 M6 design 5.3, M6-D8): CONNECT-UDP (RFC 9298)
//! over HTTP/2, one stream per target, each datagram one DATAGRAM capsule.
//! A stream is bound to its target, so every datagram it brings back counts
//! as the target's (symmetric, as VMess UDP). A stream is opened with its
//! target's first datagram, on the outbound's pooled connections; one that
//! ends is opened again by the next datagram.

use super::Inner;
use super::capsule::{self, MAX_PAYLOAD};
use crate::OutboundError;
use crate::h2pool::H2Stream;
use crate::task::AbortOnDrop;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, PacketSocket, Target};
use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
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

/// The default URI template's path for `target` (RFC 9298 2, 3): an IPv6
/// literal without brackets and with its colons percent-encoded, an IDN
/// as its A-labels. Names and IPv4 literals hold only unreserved
/// characters, so nothing else needs encoding. `None` for a name that
/// cannot be sent (the alphabet of `http::wire_host`).
pub(super) fn masque_path(target: &Target) -> Option<String> {
    let host = match &target.host {
        HostName::Ip(IpAddr::V6(v6)) => v6.to_string().replace(':', "%3A"),
        HostName::Ip(ip) => ip.to_string(),
        HostName::Domain(name) => crate::hostname::to_ascii(name)?,
    };
    Some(format!("/.well-known/masque/udp/{host}/{}/", target.port))
}

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
                let mut next = std::pin::pin!(capsule::read_datagram(&mut reader));
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
            let mut reopened = false;
            let (slot, stream) = loop {
                let slot = self.slot(to);
                let stream = match slot.get_or_try_init(|| self.open(to)).await {
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
                        break (slot, stream);
                    }
                    Some(s) if Arc::ptr_eq(s, &slot) => break (slot, stream),
                    // another slot took its place: use that one
                    Some(_) => {}
                }
            };
            *stream.last.lock().expect("clock") = Instant::now();
            let capsule = capsule::datagram(buf);
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

    #[test]
    fn the_path_carries_the_target_by_the_default_template() {
        for (host, path) in [
            (
                HostName::parse("192.0.2.6"),
                "/.well-known/masque/udp/192.0.2.6/443/",
            ),
            // RFC 9298 3's example
            (
                HostName::parse("2001:db8::42"),
                "/.well-known/masque/udp/2001%3Adb8%3A%3A42/443/",
            ),
            (
                HostName::parse("::ffff:192.0.2.6"),
                "/.well-known/masque/udp/%3A%3Affff%3A192.0.2.6/443/",
            ),
            (
                HostName::Domain("bücher.example".into()),
                "/.well-known/masque/udp/xn--bcher-kva.example/443/",
            ),
            (
                HostName::Domain("_srv.example.test".into()),
                "/.well-known/masque/udp/_srv.example.test/443/",
            ),
        ] {
            assert_eq!(
                masque_path(&Target::new(host.clone(), 443)).as_deref(),
                Some(path),
                "{host:?}"
            );
        }
        for name in ["x@blocked.test", "a/b.test", "a b.test", ""] {
            let target = Target::new(HostName::Domain(name.into()), 443);
            assert_eq!(masque_path(&target), None, "{name:?}");
        }
    }
}
