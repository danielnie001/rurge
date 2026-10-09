//! VMess UDP (M5-D5): command 2, one connection per target, each chunk one
//! datagram. The server answers on the target's connection only, so every
//! answer counts as the target's (symmetric; no XUDP). A connection is
//! opened with its target's first datagram; one that ends is opened again
//! by the next datagram.

use super::Dialer;
use super::chunk::MAX_PAYLOAD;
use super::header::COMMAND_UDP;
use crate::task::AbortOnDrop;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, PacketSocket, Target};
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, WriteHalf};
use tokio::sync::{Mutex, OnceCell, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Answers waiting for the engine; more are dropped, as a socket would.
const INBOX: usize = 64;

/// A target's connection closes after this long with neither a read nor a
/// write: the engine's flow idle (M5-D7), so a long association that
/// contacts many targets does not pile up connections.
pub(super) const IDLE: Duration = Duration::from_secs(60);

/// A server chunk of any legal size comes back whole in one read.
const READ_BUF: usize = 65536;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

type Conns = std::sync::Mutex<HashMap<Target, Slot>>;

/// One target's connection.
struct Conn {
    id: u64,
    /// The last read or write; the idle clock.
    last: Arc<std::sync::Mutex<Instant>>,
    writer: Mutex<WriteHalf<BoxedStream>>,
    /// Fires when the connection's reading ends: it is dialled again.
    ended: CancellationToken,
    _reader: AbortOnDrop,
}

type Slot = Arc<OnceCell<Arc<Conn>>>;

pub(crate) struct VmessUdp {
    dialer: Arc<Dialer>,
    opts: ConnectOpts,
    idle: Duration,
    conns: Arc<Conns>,
    answers: mpsc::Sender<(Vec<u8>, Target)>,
    inbox: Mutex<mpsc::Receiver<(Vec<u8>, Target)>>,
}

impl VmessUdp {
    pub(super) fn new(dialer: Arc<Dialer>, opts: ConnectOpts, idle: Duration) -> VmessUdp {
        let (answers, inbox) = mpsc::channel(INBOX);
        VmessUdp {
            dialer,
            opts,
            idle,
            conns: Arc::default(),
            answers,
            inbox: Mutex::new(inbox),
        }
    }

    /// `to`'s slot; a slot whose connection has ended is replaced.
    fn slot(&self, to: &Target) -> Slot {
        let mut conns = self.conns.lock().expect("conns");
        let slot = conns.entry(to.clone()).or_default();
        if slot.get().is_some_and(|c| c.ended.is_cancelled()) {
            *slot = Slot::default();
        }
        slot.clone()
    }

    #[cfg(test)]
    pub(super) fn tracked(&self) -> usize {
        self.conns.lock().expect("conns").len()
    }

    /// Forgets `slot` for `to`, unless another has taken its place.
    fn forget(&self, to: &Target, slot: &Slot) {
        let mut conns = self.conns.lock().expect("conns");
        if conns.get(to).is_some_and(|s| Arc::ptr_eq(s, slot)) {
            conns.remove(to);
        }
    }

    /// `to`'s connection and the slot it is in: `dial` opens one when there
    /// is none, and one more when that one has ended already.
    async fn connection<F, Fut>(&self, to: &Target, dial: F) -> io::Result<(Slot, Arc<Conn>)>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = io::Result<Arc<Conn>>>,
    {
        let mut redialled = false;
        loop {
            let slot = self.slot(to);
            let conn = match slot.get_or_try_init(&dial).await {
                Ok(conn) => conn.clone(),
                Err(e) => {
                    self.forget(to, &slot);
                    return Err(e);
                }
            };
            // A sender waiting on a cell whose dialler failed dials
            // again, after that dialler forgot the slot: the connection
            // must be reachable from the table or its answers are lost.
            let mut conns = self.conns.lock().expect("conns");
            // an ended connection is not revived: dial again
            if conn.ended.is_cancelled() {
                // once only: a server may end every connection at once
                if redialled {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "vmess: the server ended the UDP connection",
                    ));
                }
                redialled = true;
                continue;
            }
            match conns.get(to) {
                None => {
                    conns.insert(to.clone(), slot.clone());
                    return Ok((slot, conn));
                }
                Some(s) if Arc::ptr_eq(s, &slot) => return Ok((slot, conn)),
                // another slot took its place: use that one
                Some(_) => {}
            }
        }
    }

    async fn dial(&self, to: &Target) -> io::Result<Arc<Conn>> {
        let stream = self
            .dialer
            .connect(COMMAND_UDP, to, &self.opts)
            .await
            .map_err(|e| io::Error::other(e.to_string()))?;
        let (mut reader, writer) = tokio::io::split(stream);
        let ended = CancellationToken::new();
        let (answers, from, done) = (self.answers.clone(), to.clone(), ended.clone());
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let last = Arc::new(std::sync::Mutex::new(Instant::now()));
        let (clock, idle, conns) = (last.clone(), self.idle, Arc::downgrade(&self.conns));
        let task = tokio::spawn(async move {
            // a read returns one chunk when the buffer holds a whole one
            let mut buf = vec![0u8; READ_BUF];
            let why = loop {
                let deadline = *clock.lock().expect("clock") + idle;
                tokio::select! {
                    read = reader.read(&mut buf) => match read {
                        Ok(0) => break "the server closed it".to_string(),
                        Err(e) => break format!("read error: {:?}", e.kind()),
                        Ok(n) => {
                            *clock.lock().expect("clock") = Instant::now();
                            // a full inbox drops the answer
                            let _ = answers.try_send((buf[..n].to_vec(), from.clone()));
                        }
                    },
                    // a write may have moved the deadline: look again
                    _ = tokio::time::sleep_until(deadline) => {
                        if *clock.lock().expect("clock") + idle <= Instant::now() {
                            break "idle".to_string();
                        }
                    }
                }
            };
            tracing::debug!("vmess: a UDP connection ended: {why}");
            done.cancel();
            // leave the table, unless a newer connection has taken the place
            if let Some(conns) = Weak::upgrade(&conns) {
                let mut conns = conns.lock().expect("conns");
                if conns
                    .get(&from)
                    .is_some_and(|s| s.get().is_some_and(|c| c.id == id))
                {
                    conns.remove(&from);
                }
            }
        });
        Ok(Arc::new(Conn {
            id,
            last,
            writer: Mutex::new(writer),
            ended,
            _reader: AbortOnDrop(task),
        }))
    }
}

impl PacketSocket for VmessUdp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // one datagram, one chunk
            if buf.len() > MAX_PAYLOAD {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("vmess: a datagram longer than {MAX_PAYLOAD} bytes"),
                ));
            }
            if buf.is_empty() {
                // an empty chunk would end the connection
                return Ok(());
            }
            let (slot, conn) = self.connection(to, || self.dial(to)).await?;
            *conn.last.lock().expect("clock") = Instant::now();
            let mut writer = conn.writer.lock().await;
            let written = async {
                writer.write_all(buf).await?;
                writer.flush().await
            };
            if let Err(e) = written.await {
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
    use crate::transport::Stack;
    use crate::vmess::header::Security;
    use rurge_config::HostName;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::sync::atomic::AtomicUsize;
    use tokio::net::TcpListener;

    /// A server that ends every UDP connection as soon as it is up: a
    /// datagram that finds its connection ended dials once more, and fails
    /// when that one has ended too — it does not go on dialling for as long
    /// as its sender waits, and the target is not failed for good: the next
    /// datagram dials twice again. The loop is handed each connection after
    /// its reader has seen the end: on its own it may look before that.
    #[tokio::test]
    async fn a_connection_that_has_ended_is_dialled_again_only_once() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });
        let dialer = Arc::new(Dialer {
            stack: Stack::new(
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
                Target::new(HostName::Ip(server.ip()), server.port()),
                None,
                None,
                None,
            ),
            cmd_key: [0; 16],
            security: Security::Aes128Gcm,
        });
        let udp = VmessUdp::new(dialer, ConnectOpts::default(), IDLE);
        let to = Target::new(HostName::parse("target.test"), 53);
        let dials = AtomicUsize::new(0);
        // what the dials may add up to by the end of the datagram in hand
        let allowed = AtomicUsize::new(0);
        let ended = || async {
            let n = dials.fetch_add(1, Ordering::Relaxed) + 1;
            assert!(n <= allowed.load(Ordering::Relaxed), "dialled a third time");
            let conn = udp.dial(&to).await?;
            conn.ended.cancelled().await;
            Ok(conn)
        };
        for datagram in 1..=2 {
            allowed.store(2 * datagram, Ordering::Relaxed);
            let found = tokio::time::timeout(Duration::from_secs(10), udp.connection(&to, &ended))
                .await
                .expect("an end within the bound");
            let err = found.map(drop).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
            assert_eq!(
                err.to_string(),
                "vmess: the server ended the UDP connection"
            );
            assert_eq!(dials.load(Ordering::Relaxed), 2 * datagram);
        }
    }
}
