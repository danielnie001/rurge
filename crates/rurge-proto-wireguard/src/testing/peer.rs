//! `FakeWgPeer`: a `PeerCore` on a loopback UDP port.

use super::{PeerCore, PeerOpts};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

/// How often the peer runs WireGuard's timers.
const TICK: Duration = Duration::from_millis(50);
/// The socket's buffers each way: a burst of a whole TCP window arrives at
/// once on loopback.
const SOCKET_BUFFER: usize = 8 << 20;

pub struct FakeWgPeer {
    addr: SocketAddr,
    public_key: [u8; 32],
    core: Arc<Mutex<PeerCore>>,
    silent: Arc<AtomicBool>,
    clients: Arc<Mutex<Vec<SocketAddr>>>,
    task: JoinHandle<()>,
}

impl FakeWgPeer {
    /// A peer for the client of `client_public`, on 127.0.0.1.
    pub async fn start(client_public: [u8; 32], opts: PeerOpts) -> FakeWgPeer {
        let socket = UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let buffers = socket2::SockRef::from(&socket);
        let _ = buffers.set_recv_buffer_size(SOCKET_BUFFER);
        let _ = buffers.set_send_buffer_size(SOCKET_BUFFER);
        let addr = socket.local_addr().expect("its address");
        let core = PeerCore::new(client_public, &opts);
        let public_key = core.public_key();
        let core = Arc::new(Mutex::new(core));
        let silent = Arc::new(AtomicBool::new(false));
        let clients = Arc::new(Mutex::new(Vec::new()));
        let task = tokio::spawn(serve(socket, core.clone(), silent.clone(), clients.clone()));
        FakeWgPeer {
            addr,
            public_key,
            core,
            silent,
            clients,
            task,
        }
    }

    /// Its UDP address: a client's `endpoint`.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    /// What it has seen so far.
    pub fn core(&self) -> MutexGuard<'_, PeerCore> {
        self.core.lock().expect("the peer")
    }

    /// Every address the client's messages came from, in order.
    pub fn clients(&self) -> Vec<SocketAddr> {
        self.clients.lock().expect("the clients").clone()
    }

    /// From now on it drops whatever arrives and sends nothing, as a peer
    /// that is down.
    pub fn go_silent(&self, silent: bool) {
        self.silent.store(silent, Ordering::SeqCst);
    }
}

impl Drop for FakeWgPeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    socket: UdpSocket,
    core: Arc<Mutex<PeerCore>>,
    silent: Arc<AtomicBool>,
    clients: Arc<Mutex<Vec<SocketAddr>>>,
) {
    let mut buf = vec![0u8; 65536];
    let mut client = None;
    let mut timer = tokio::time::interval(TICK);
    // when its stack wants to run again (a retransmission, a delayed ACK)
    let mut due: Option<tokio::time::Instant> = None;
    loop {
        let mut out = Vec::new();
        let stack_due = async {
            match due {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        let ran = tokio::select! {
            received = socket.recv_from(&mut buf) => {
                // an ICMP error the system reports on the socket
                let Ok((n, from)) = received else { continue };
                if silent.load(Ordering::SeqCst) {
                    continue;
                }
                let mut core = core.lock().expect("the peer");
                let mut arrived = Some((n, from));
                while let Some((n, from)) = arrived {
                    // it answers where the client last wrote from (roaming)
                    if client != Some(from) {
                        client = Some(from);
                        let mut clients = clients.lock().expect("the clients");
                        if !clients.contains(&from) {
                            clients.push(from);
                        }
                    }
                    core.receive(&mut buf[..n], &mut out);
                    // what else has arrived goes in before the stack runs
                    arrived = socket.try_recv_from(&mut buf).ok();
                }
                core.advance(&mut out)
            }
            _ = timer.tick() => {
                if silent.load(Ordering::SeqCst) {
                    continue;
                }
                let mut core = core.lock().expect("the peer");
                core.tick(&mut out);
                core.advance(&mut out)
            }
            _ = stack_due => {
                if silent.load(Ordering::SeqCst) {
                    continue;
                }
                core.lock().expect("the peer").advance(&mut out)
            }
        };
        due = ran.map(|wait| tokio::time::Instant::now() + wait);
        if let Some(to) = client {
            for message in out {
                let _ = socket.send_to(&message, to).await;
            }
        }
    }
}
