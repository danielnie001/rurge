//! A `Dialer` for tests: what a destination dials is what the test scripted,
//! on the loopback or in memory, and nothing else.

use super::servers::pattern;
use rurge_config::{HostName, SessionInfo};
use rurge_inbound::{
    DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome, UdpAdmission, UdpClient,
};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, Target};
use rurge_proto::RejectKind;
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

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

/// What the fake dialer does with each datagram a UDP association sends to a
/// target (the target as the app's datagram named it).
#[derive(Clone, Debug)]
pub enum UdpScript {
    /// Sends the datagram back as the target's own reply: from the target, as
    /// the answer of the flow to it.
    Echo,
    /// Sends these bytes back as the target's reply, whatever the datagram was.
    Reply(Vec<u8>),
    /// Sends the datagram back from another target, which is no flow's
    /// answer: a third party answering (STUN's other server, a peer).
    EchoFrom(Target),
    /// Tells the client the flow to the target was refused, for each datagram.
    Refuse(RejectKind),
}

/// One association that is open: whose it is, and the client to send to it.
struct Open {
    id: u64,
    app: SocketAddr,
    client: Arc<dyn UdpClient>,
}

struct UdpBook {
    admission: UdpAdmission,
    scripts: HashMap<Target, UdpScript>,
    /// The template of every association that was served, in order.
    associations: Vec<SessionInfo>,
    open: Vec<Open>,
    /// Every datagram the associations sent: where to, and the bytes.
    sent: Vec<(Target, Vec<u8>)>,
    /// While true no association asks its client for a datagram.
    held: watch::Sender<bool>,
}

impl Default for UdpBook {
    fn default() -> UdpBook {
        UdpBook {
            admission: UdpAdmission::Accepted,
            scripts: HashMap::new(),
            associations: Vec::new(),
            open: Vec::new(),
            sent: Vec::new(),
            held: watch::channel(false).0,
        }
    }
}

/// Takes an association off the open ones when its `associate` ends, however
/// it ends.
struct Closing<'a> {
    dialer: &'a FakeDialer,
    id: u64,
}

impl Drop for Closing<'_> {
    fn drop(&mut self) {
        self.dialer.udp().open.retain(|open| open.id != self.id);
    }
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
    udp: Mutex<UdpBook>,
}

impl FakeDialer {
    pub fn new() -> Arc<FakeDialer> {
        Arc::new(FakeDialer {
            next_id: AtomicU64::new(1),
            book: Mutex::new(Book::default()),
            udp: Mutex::new(UdpBook::default()),
        })
    }

    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().expect("the fake dialer's book")
    }

    fn udp(&self) -> MutexGuard<'_, UdpBook> {
        self.udp.lock().expect("the fake dialer's UDP book")
    }

    /// What `admit_udp` answers from now on (`Accepted` to begin with).
    pub fn admit(&self, admission: UdpAdmission) {
        self.udp().admission = admission;
    }

    /// What each datagram to `target` does from now on; a target with no
    /// script has its datagrams written down and nothing else done.
    pub fn script_udp(&self, target: Target, script: UdpScript) {
        self.udp().scripts.insert(target, script);
    }

    /// The template of every association that was served, in order.
    pub fn associations(&self) -> Vec<SessionInfo> {
        self.udp().associations.clone()
    }

    /// How many associations are being served now.
    pub fn associations_open(&self) -> usize {
        self.udp().open.len()
    }

    /// Every datagram the associations got from their clients: the target it
    /// was for and the bytes, in the order they were taken.
    pub fn udp_sent(&self) -> Vec<(Target, Vec<u8>)> {
        self.udp().sent.clone()
    }

    /// From now on no association asks its client for a datagram, until
    /// `release_udp`: one that was waiting for a datagram stops waiting, and
    /// whatever the client has queued waits.
    pub fn hold_udp(&self) {
        self.udp().held.send_replace(true);
    }

    pub fn release_udp(&self) {
        self.udp().held.send_replace(false);
    }

    /// Sends `payload` to the app at `app` through its open association, as a
    /// reply from `from` that answers the flow to `answers`, if it names one.
    /// Waits as long as the client does for room to send; `NotFound` when no
    /// association of that app is open.
    pub async fn send_to_app(
        &self,
        app: SocketAddr,
        from: Target,
        answers: Option<Target>,
        payload: Vec<u8>,
    ) -> io::Result<()> {
        let client = self
            .udp()
            .open
            .iter()
            .find(|open| open.app == app)
            .map(|open| open.client.clone())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no association of the app"))?;
        client.send(&payload, &from, answers.as_ref()).await
    }

    /// What the script of `to` does with a datagram of `payload`.
    async fn datagram(&self, client: &Arc<dyn UdpClient>, to: Target, payload: &[u8]) {
        let script = {
            let mut udp = self.udp();
            udp.sent.push((to.clone(), payload.to_vec()));
            udp.scripts.get(&to).cloned()
        };
        // a reply the client cannot take is lost, as the engine's is
        let _ = match script {
            None => Ok(()),
            Some(UdpScript::Echo) => client.send(payload, &to, Some(&to)).await,
            Some(UdpScript::Reply(bytes)) => client.send(&bytes, &to, Some(&to)).await,
            Some(UdpScript::EchoFrom(from)) => client.send(payload, &from, None).await,
            Some(UdpScript::Refuse(kind)) => {
                client.refused(&to, kind);
                Ok(())
            }
        };
    }

    /// By the destination the app connected to: for a fake address, the
    /// address and not the name behind it.
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

    /// How each relay ended, in order: `Ok(())` or the error's text. A relay
    /// that its session's token ended is recorded as `Err("cancelled")`.
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

/// Where the app connected to: the session's `local`, which is the endpoint
/// even when `dst_host` is the name behind a fake address; a session with no
/// `local` is where it says it is, if it names an address.
fn destination(session: &SessionInfo) -> Option<SocketAddr> {
    session.local.or(match &session.dst_host {
        HostName::Ip(ip) => Some(SocketAddr::new(*ip, session.dst_port)),
        HostName::Domain(_) => None,
    })
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

    fn admit_udp(&self) -> UdpAdmission {
        self.udp().admission
    }

    /// Serves the association until `closed` fires: takes the client's
    /// datagrams one by one, as the engine's association does, writes each
    /// down and does with it what its target's script says. The template is
    /// written down when the association starts.
    fn associate<'a>(
        &'a self,
        client: Arc<dyn UdpClient>,
        session: SessionInfo,
        closed: CancellationToken,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            {
                let mut udp = self.udp();
                udp.associations.push(session.clone());
                udp.open.push(Open {
                    id,
                    app: session.src,
                    client: client.clone(),
                });
            }
            let _closing = Closing { dialer: self, id };
            let mut held = self.udp().held.subscribe();
            let mut buf = vec![0u8; 65536];
            loop {
                let released = async { held.wait_for(|held| !*held).await.is_ok() };
                tokio::select! {
                    biased;
                    _ = closed.cancelled() => return,
                    open = released => if !open { return },
                }
                let got = tokio::select! {
                    biased;
                    _ = closed.cancelled() => return,
                    // held while it waits: it stops waiting, and what comes
                    // stays where it is
                    taken = async { held.wait_for(|held| *held).await.is_ok() } => {
                        if taken {
                            continue;
                        }
                        return;
                    }
                    got = client.recv(&mut buf) => got,
                };
                let Ok((n, to)) = got else {
                    return;
                };
                // a reply waits for room in the writer's queue, and the end
                // of the association does not wait for it
                tokio::select! {
                    biased;
                    _ = closed.cancelled() => return,
                    () = self.datagram(&client, to, &buf[..n]) => {}
                }
            }
        })
    }
}
