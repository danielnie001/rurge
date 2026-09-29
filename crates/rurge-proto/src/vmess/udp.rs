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
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, WriteHalf};
use tokio::sync::{Mutex, OnceCell, mpsc};
use tokio_util::sync::CancellationToken;

/// Answers waiting for the engine; more are dropped, as a socket would.
const INBOX: usize = 64;

/// One target's connection.
struct Conn {
    writer: Mutex<WriteHalf<BoxedStream>>,
    /// Fires when the connection's reading ends: it is dialled again.
    ended: CancellationToken,
    _reader: AbortOnDrop,
}

type Slot = Arc<OnceCell<Arc<Conn>>>;

pub(crate) struct VmessUdp {
    dialer: Arc<Dialer>,
    opts: ConnectOpts,
    conns: std::sync::Mutex<HashMap<Target, Slot>>,
    answers: mpsc::Sender<(Vec<u8>, Target)>,
    inbox: Mutex<mpsc::Receiver<(Vec<u8>, Target)>>,
}

impl VmessUdp {
    pub(super) fn new(dialer: Arc<Dialer>, opts: ConnectOpts) -> VmessUdp {
        let (answers, inbox) = mpsc::channel(INBOX);
        VmessUdp {
            dialer,
            opts,
            conns: std::sync::Mutex::default(),
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

    /// Forgets `slot` for `to`, unless another has taken its place.
    fn forget(&self, to: &Target, slot: &Slot) {
        let mut conns = self.conns.lock().expect("conns");
        if conns.get(to).is_some_and(|s| Arc::ptr_eq(s, slot)) {
            conns.remove(to);
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
        let task = tokio::spawn(async move {
            // a read returns one chunk when the buffer holds a whole one
            let mut buf = vec![0u8; MAX_PAYLOAD];
            while let Ok(n) = reader.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                // a full inbox drops the answer
                let _ = answers.try_send((buf[..n].to_vec(), from.clone()));
            }
            done.cancel();
        });
        Ok(Arc::new(Conn {
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
            let slot = self.slot(to);
            let conn = match slot.get_or_try_init(|| self.dial(to)).await {
                Ok(conn) => conn.clone(),
                Err(e) => {
                    self.forget(to, &slot);
                    return Err(e);
                }
            };
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
