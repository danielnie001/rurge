//! A `Dialer` for tests: what a destination dials is what the test scripted,
//! on the loopback or in memory, and nothing else.

use super::servers::pattern;
use rurge_config::{HostName, SessionInfo};
use rurge_inbound::{DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome};
use rurge_net::BoxFuture;
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::TcpStream;
use tokio::sync::watch;

/// What dialing a destination does.
#[derive(Clone, Debug)]
pub enum Script {
    /// Connect to this address and relay to it; `script` panics unless it is a loopback address.
    Upstream(SocketAddr),
    /// `tokio::io::duplex(buffer)`; the far end writes `send` bytes of `pattern` while it reads and
    /// discards whatever comes, until the session ends.
    Memory {
        buffer: usize,
        send: usize,
    },
    /// `tokio::io::duplex(buffer)`; the far end neither reads nor writes until
    /// `FakeDialer::release(dst)`, then reads and discards until the session ends.
    Hold {
        buffer: usize,
    },
    /// `tokio::io::duplex(buffer)`; the far end sends back what it reads and closes its own
    /// side once the near end has closed its. Like `Upstream::echo`, with no socket of the
    /// system: a test of thousands of connections has no use for thousands of ports.
    MemoryEcho {
        buffer: usize,
    },
    Reject(RejectKind),
    /// `DialError::Failed { kind: FailKind::Connect, message: "scripted failure", .. }`.
    Fail,
}

/// The counters and the gate of the far ends of the sessions to one destination.
struct Far {
    /// Bytes read so far; a change wakes whoever waits for more.
    received: watch::Sender<u64>,
    released: watch::Sender<bool>,
}

#[derive(Default)]
struct Book {
    scripts: HashMap<SocketAddr, Script>,
    far: HashMap<SocketAddr, Arc<Far>>,
    dialed: Vec<SessionInfo>,
    relay_ends: Vec<Result<(), String>>,
}

/// Dials the destinations a test wrote a [`Script`] for. It dials nothing but
/// the loopback addresses it was given; any other destination fails.
pub struct FakeDialer {
    next_id: AtomicU64,
    book: Mutex<Book>,
}

impl FakeDialer {
    pub fn new() -> Arc<FakeDialer> {
        Arc::new(FakeDialer {
            next_id: AtomicU64::new(1),
            book: Mutex::new(Book::default()),
        })
    }

    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().expect("the fake dialer's book")
    }

    /// By the destination the app connected to.
    pub fn script(&self, dst: SocketAddr, script: Script) {
        if let Script::Upstream(upstream) = &script {
            assert!(
                upstream.ip().is_loopback(),
                "the fake dialer dials only the loopback"
            );
        }
        let mut book = self.book();
        if matches!(script, Script::Memory { .. } | Script::Hold { .. }) {
            let far = Far {
                received: watch::channel(0).0,
                released: watch::channel(false).0,
            };
            book.far.insert(dst, Arc::new(far));
        }
        book.scripts.insert(dst, script);
    }

    /// Every session dialed, in order.
    pub fn dialed(&self) -> Vec<SessionInfo> {
        self.book().dialed.clone()
    }

    /// How each relay ended, in order: `Ok(())` or the error's text.
    pub fn relay_ends(&self) -> Vec<Result<(), String>> {
        self.book().relay_ends.clone()
    }

    /// Lets the far ends of `Hold` sessions to `dst` start reading.
    pub fn release(&self, dst: SocketAddr) {
        let book = self.book();
        assert!(
            matches!(book.scripts.get(&dst), Some(Script::Hold { .. })),
            "release needs a Hold script for the destination"
        );
        book.far[&dst].released.send_replace(true);
    }

    /// Bytes the far ends of `Memory` and `Hold` sessions to `dst` have read so far.
    pub fn received(&self, dst: SocketAddr) -> u64 {
        self.book()
            .far
            .get(&dst)
            .map_or(0, |far| *far.received.borrow())
    }

    /// Waits until `received(dst)` is at least `bytes`; false when `within` ran out
    /// first. Panics unless `dst` has a `Memory` or `Hold` script.
    pub async fn wait_for(&self, dst: SocketAddr, bytes: u64, within: Duration) -> bool {
        let far = self.book().far.get(&dst).cloned();
        let far = far.expect("wait_for needs a Memory or Hold script for the destination");
        let mut changes = far.received.subscribe();
        matches!(
            tokio::time::timeout(within, changes.wait_for(|total| *total >= bytes)).await,
            Ok(Ok(_))
        )
    }
}

/// Where the app connected to, when the session names an address.
fn destination(session: &SessionInfo) -> Option<SocketAddr> {
    match &session.dst_host {
        HostName::Ip(ip) => Some(SocketAddr::new(*ip, session.dst_port)),
        HostName::Domain(_) => None,
    }
}

fn failed(handle: Arc<SessionHandle>, message: String) -> Result<Dialed, DialError> {
    handle.finish(SessionOutcome::Failed(message.clone()));
    Err(DialError::Failed {
        kind: FailKind::Connect,
        message,
        rule: None,
        handle,
    })
}

/// The far end of an in-memory session: it writes `send` bytes of the
/// pattern while it reads and discards what comes, once it may. It works in
/// pieces of what the pipe holds (`buffer`, at most 8 KiB), so that ten
/// thousand far ends hold little more than their pipes do.
fn far_end(far: Arc<Far>, stream: DuplexStream, buffer: usize, send: usize, hold: bool) {
    let piece = buffer.clamp(1, 8192);
    tokio::spawn(async move {
        let (mut read, mut write) = tokio::io::split(stream);
        let writing = async {
            let mut done = 0;
            while done < send {
                let n = (send - done).min(piece);
                let chunk: Vec<u8> = (done..done + n).map(|i| pattern(i as u64)).collect();
                if write.write_all(&chunk).await.is_err() {
                    return;
                }
                done += n;
            }
        };
        let reading = async {
            if hold {
                let mut gate = far.released.subscribe();
                if gate.wait_for(|released| *released).await.is_err() {
                    return;
                }
            }
            let mut buf = vec![0; piece];
            while let Ok(n) = read.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                far.received.send_modify(|total| *total += n as u64);
            }
        };
        tokio::join!(writing, reading);
    });
}

impl Dialer for FakeDialer {
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>> {
        Box::pin(async move {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let handle = SessionHandle::new(id, session.clone());
            let (script, far) = {
                let mut book = self.book();
                book.dialed.push(session.clone());
                let dst = destination(&session);
                (
                    dst.and_then(|dst| book.scripts.get(&dst).cloned()),
                    dst.and_then(|dst| book.far.get(&dst).cloned()),
                )
            };
            let ready = |stream: BoxedStream| {
                Ok(Dialed {
                    stream,
                    handle: handle.clone(),
                    forward: None,
                })
            };
            match script {
                None => failed(handle, "not scripted".into()),
                Some(Script::Fail) => failed(handle, "scripted failure".into()),
                Some(Script::Reject(kind)) => {
                    handle.finish(SessionOutcome::Rejected(kind));
                    Err(DialError::Reject {
                        kind,
                        rule: None,
                        handle,
                    })
                }
                Some(Script::Upstream(upstream)) => match TcpStream::connect(upstream).await {
                    Ok(stream) => ready(Box::new(stream)),
                    Err(e) => failed(handle, e.to_string()),
                },
                Some(Script::Memory { buffer, send }) => {
                    let (near, far_stream) = tokio::io::duplex(buffer);
                    far_end(
                        far.expect("a far end for a memory script"),
                        far_stream,
                        buffer,
                        send,
                        false,
                    );
                    ready(Box::new(near))
                }
                Some(Script::Hold { buffer }) => {
                    let (near, far_stream) = tokio::io::duplex(buffer);
                    far_end(
                        far.expect("a far end for a hold script"),
                        far_stream,
                        buffer,
                        0,
                        true,
                    );
                    ready(Box::new(near))
                }
                Some(Script::MemoryEcho { buffer }) => {
                    let (near, far_stream) = tokio::io::duplex(buffer);
                    tokio::spawn(async move {
                        let (mut read, mut write) = tokio::io::split(far_stream);
                        // ends with the end of what the near end sends, or an error
                        let _ = tokio::io::copy(&mut read, &mut write).await;
                        let _ = write.shutdown().await;
                    });
                    ready(Box::new(near))
                }
            }
        })
    }

    /// The same two buffers of 8 KiB as the engine's relay, and like it this
    /// ends when the session's token is cancelled, whatever it is waiting on
    /// (a copy that waits to write upstream reads nothing from the client, so
    /// it would not meet the end of the client's connection): the end is
    /// recorded as `Err("cancelled")`, and the handle finishes `Completed`, as
    /// the engine finishes a session that was cancelled and not killed.
    fn relay<'a>(
        &'a self,
        mut client: BoxedStream,
        mut upstream: BoxedStream,
        handle: Arc<SessionHandle>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let (end, outcome) = tokio::select! {
                biased;
                _ = handle.token().cancelled() => {
                    (Err("cancelled".to_string()), SessionOutcome::Completed)
                }
                end = tokio::io::copy_bidirectional_with_sizes(
                    &mut client,
                    &mut upstream,
                    8192,
                    8192,
                ) => match end.map(|_| ()).map_err(|e| e.to_string()) {
                    Ok(()) => (Ok(()), SessionOutcome::Completed),
                    Err(text) => (Err(text.clone()), SessionOutcome::Failed(text)),
                },
            };
            // recorded before the handle finishes, so whoever waits for the
            // handle's outcome finds it
            self.book().relay_ends.push(end);
            handle.finish(outcome);
        })
    }
}
