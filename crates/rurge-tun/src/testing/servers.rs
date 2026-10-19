//! Servers on the loopback for the far end of a relay.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

/// Byte `i` of a stream that the servers write and the tests check by.
pub fn pattern(i: u64) -> u8 {
    (i % 251) as u8
}

const CHUNK: usize = 16 * 1024;

struct Counters {
    received: watch::Sender<u64>,
    sent: AtomicU64,
    /// Connections accepted and not over yet.
    open: AtomicUsize,
}

/// A connection counts as open for as long as this is alive, wherever the
/// task that serves it ends.
struct Open(Arc<Counters>);

impl Open {
    fn begin(counters: &Arc<Counters>) -> Open {
        counters.open.fetch_add(1, Ordering::Relaxed);
        Open(counters.clone())
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A server on `127.0.0.1` and a port of the system's choosing, for any
/// number of connections. Dropping it ends it and its connections.
pub struct Upstream {
    addr: SocketAddr,
    counters: Arc<Counters>,
    task: JoinHandle<()>,
}

impl Upstream {
    /// Sends back what it reads, and closes its end once the client has closed its own.
    pub async fn echo() -> Upstream {
        Upstream::serve(echo).await
    }

    /// Reads and counts what it reads.
    pub async fn sink() -> Upstream {
        Upstream::serve(sink).await
    }

    /// Writes `bytes` bytes of the pattern to each connection, then closes its end.
    pub async fn source(bytes: u64) -> Upstream {
        Upstream::serve(move |stream, counters| source(bytes, stream, counters)).await
    }

    async fn serve<F, Fut>(handler: F) -> Upstream
    where
        F: Fn(TcpStream, Arc<Counters>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        let addr = listener.local_addr().expect("the listener's address");
        let counters = Arc::new(Counters {
            received: watch::channel(0).0,
            sent: AtomicU64::new(0),
            open: AtomicUsize::new(0),
        });
        let task = tokio::spawn({
            let counters = counters.clone();
            async move {
                // dropped with the task, which ends the connections' tasks too
                let mut connections = JoinSet::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            if let Ok((stream, _)) = accepted {
                                let open = Open::begin(&counters);
                                let served = handler(stream, counters.clone());
                                connections.spawn(async move {
                                    let _open = open;
                                    served.await;
                                });
                            }
                        }
                        // finished connections do not pile up
                        Some(_) = connections.join_next(), if !connections.is_empty() => {}
                    }
                }
            }
        });
        Upstream {
            addr,
            counters,
            task,
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Bytes read from the connections so far.
    pub fn received(&self) -> u64 {
        *self.counters.received.borrow()
    }

    /// Waits until `received()` is at least `bytes`; false when `within` ran out first.
    pub async fn wait_for(&self, bytes: u64, within: Duration) -> bool {
        let mut changes = self.counters.received.subscribe();
        matches!(
            tokio::time::timeout(within, changes.wait_for(|total| *total >= bytes)).await,
            Ok(Ok(_))
        )
    }

    /// Bytes written to the connections so far.
    pub fn sent(&self) -> u64 {
        self.counters.sent.load(Ordering::Relaxed)
    }

    /// Connections accepted whose serving has not ended: the server is done with
    /// (and has dropped) every one that is not counted.
    pub fn open(&self) -> usize {
        self.counters.open.load(Ordering::Relaxed)
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Counters {
    fn read(&self, n: usize) {
        self.received.send_modify(|total| *total += n as u64);
    }
}

async fn echo(stream: TcpStream, counters: Arc<Counters>) {
    let (mut read, mut write) = stream.into_split();
    let mut buf = vec![0; CHUNK];
    loop {
        match read.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                counters.read(n);
                if write.write_all(&buf[..n]).await.is_err() {
                    return;
                }
            }
        }
    }
    let _ = write.shutdown().await;
}

async fn sink(mut stream: TcpStream, counters: Arc<Counters>) {
    let mut buf = vec![0; 4 * CHUNK];
    while let Ok(n) = stream.read(&mut buf).await {
        if n == 0 {
            break;
        }
        counters.read(n);
    }
}

async fn source(bytes: u64, mut stream: TcpStream, counters: Arc<Counters>) {
    let mut written = 0;
    let mut chunk = Vec::with_capacity(CHUNK);
    while written < bytes {
        let n = (bytes - written).min(CHUNK as u64);
        chunk.clear();
        chunk.extend((written..written + n).map(pattern));
        if stream.write_all(&chunk).await.is_err() {
            return;
        }
        written += n;
        counters.sent.fetch_add(n, Ordering::Relaxed);
    }
    let _ = stream.shutdown().await;
    // close once the client has: a connection dropped with unread data resets
    let mut unread = [0; 1024];
    while matches!(stream.read(&mut unread).await, Ok(n) if n > 0) {}
}
