//! A scriptable TUIC v5 server (phase 2 M7 design 6.4): QUIC with ALPN
//! `h3` on a loopback port of its own (`ServerPort`: it can restart) or on
//! a socket it is given (`MultiPort` for port hopping). It checks the
//! authentication against what its own side of the TLS session exports,
//! holds the Connects that come before it (as the SPEC asks), and relays
//! each to its target. What the client sends is read by code that is not
//! the client's: the commands and their addresses by the code here. It
//! never resolves a name.

use super::ServerPort;
use quinn::{AsyncUdpSocket, Connection, Endpoint, RecvStream, SendStream, VarInt};
use rurge_net::quic::testing::{TestCert, endpoint};
use rustls::RootCertStore;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::io::{AsyncReadExt, copy_bidirectional_with_sizes};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

// the wire as the fake knows it
const VERSION: u8 = 0x05;
const AUTHENTICATE: u8 = 0x00;
const CONNECT: u8 = 0x01;
/// How the fake closes a connection, and stops a stream: with 0, as sing-box
/// does.
const CLOSE: VarInt = VarInt::from_u32(0);
/// How much of a TCP tunnel the fake relays at a time, each way.
const RELAY: usize = 64 << 10;

#[derive(Clone, Debug, Default)]
pub struct TuicScript {
    /// The one user it lets in, by its UUID's 16 bytes, and its password.
    pub uuid: [u8; 16],
    pub password: String,
    /// The code it closes a connection with when the authentication
    /// failed, never with a reason: 0 — as tuic-server 1.0.0 and sing-box
    /// close it — unless set.
    pub auth_failure_code: u32,
    /// Relay here whatever the client asked for (needed for a name).
    pub connect_to: Option<SocketAddr>,
}

/// An Authenticate command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedTuicAuth {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    pub uuid: [u8; 16],
    pub token: [u8; 32],
    /// What the fake's own side of the TLS session exports for its user:
    /// the token it lets in.
    pub expected: [u8; 32],
    pub accepted: bool,
}

/// A Connect command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedConnect {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// The target as the command named it: `name:port`, or an address and
    /// its port (IPv6 in brackets).
    pub address: String,
}

pub struct FakeTuic {
    cert: TestCert,
    addr: SocketAddr,
    /// `None` on a socket it was given: it cannot restart.
    port: Option<ServerPort>,
    shared: Arc<Shared>,
    /// Every endpoint so far, with its accept loop: closed with the fake.
    endpoints: Mutex<Vec<(Endpoint, AbortOnDropHandle<()>)>>,
}

/// What the connections of every endpoint share.
struct Shared {
    script: TuicScript,
    /// The server's side of every connection accepted, in order.
    connections: Mutex<Vec<Connection>>,
    auths: Mutex<Vec<RecordedTuicAuth>>,
    connects: Mutex<Vec<RecordedConnect>>,
}

impl Shared {
    fn connections(&self) -> MutexGuard<'_, Vec<Connection>> {
        self.connections.lock().expect("connections")
    }
}

impl FakeTuic {
    /// On a loopback port of its own, which it can take over again
    /// (`restart`).
    pub fn spawn(script: TuicScript) -> FakeTuic {
        let port = ServerPort::bind();
        let fake = FakeTuic::new(script, port.addr(), Some(port));
        fake.take_over();
        fake
    }

    /// On `socket` — a `MultiPort` for port hopping —, which it cannot
    /// take over again.
    pub fn spawn_on(script: TuicScript, socket: Arc<dyn AsyncUdpSocket>) -> FakeTuic {
        let addr = socket.local_addr().expect("the socket's address");
        let fake = FakeTuic::new(script, addr, None);
        fake.listen(endpoint(fake.cert.server_config(&["h3"]), socket));
        fake
    }

    fn new(script: TuicScript, addr: SocketAddr, port: Option<ServerPort>) -> FakeTuic {
        FakeTuic {
            cert: TestCert::new(),
            addr,
            port,
            shared: Arc::new(Shared {
                script,
                connections: Mutex::default(),
                auths: Mutex::default(),
                connects: Mutex::default(),
            }),
            endpoints: Mutex::default(),
        }
    }

    /// Its port: the first of them, on a socket of several.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Roots that trust its certificate (for `localhost` and `127.0.0.1`).
    pub fn roots(&self) -> Arc<RootCertStore> {
        self.cert.roots()
    }

    /// Every Authenticate it read, in arrival order.
    pub fn auths(&self) -> Vec<RecordedTuicAuth> {
        self.shared.auths.lock().expect("auths").clone()
    }

    /// Every Connect it read, in arrival order: before the authentication
    /// went through, as the commands that wait for it.
    pub fn connects(&self) -> Vec<RecordedConnect> {
        self.shared.connects.lock().expect("connects").clone()
    }

    /// The connections accepted so far: QUIC's handshake done.
    pub fn connections(&self) -> usize {
        self.shared.connections().len()
    }

    /// Closes every connection accepted so far — code 0, no reason —, as
    /// sing-box ends its sessions.
    pub fn close_connections(&self) {
        for connection in self.shared.connections().iter() {
            connection.close(CLOSE, b"");
        }
    }

    /// A new endpoint takes the port over, as after a restart: the
    /// connections of the one before are gone, and the client learns it
    /// from a stateless reset of its next packet.
    pub fn restart(&self) {
        self.take_over();
    }

    /// A new endpoint takes the port over; the one before falls silent.
    fn take_over(&self) {
        let port = self.port.as_ref().expect("a fake on a port of its own");
        self.listen(port.endpoint(self.cert.server_config(&["h3"])));
    }

    /// Serves on `endpoint` until the fake is dropped.
    fn listen(&self, endpoint: Endpoint) {
        let accepting = tokio::spawn(accept(endpoint.clone(), self.shared.clone()));
        self.endpoints
            .lock()
            .expect("endpoints")
            .push((endpoint, AbortOnDropHandle::new(accepting)));
    }
}

impl Drop for FakeTuic {
    /// Every connection ends with the fake, and with them the tasks that
    /// serve them.
    fn drop(&mut self) {
        for (endpoint, _) in self.endpoints.get_mut().expect("endpoints").iter() {
            endpoint.close(CLOSE, b"");
        }
    }
}

/// The connections of `endpoint`, each served in a task of its own until
/// it ends.
async fn accept(endpoint: Endpoint, shared: Arc<Shared>) {
    while let Some(incoming) = endpoint.accept().await {
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Ok(connection) = incoming.await {
                serve(connection, shared).await;
            }
        });
    }
}

/// One connection: its streams, each in a task of its own that ends with
/// the connection. The Connects wait for the authentication.
async fn serve(connection: Connection, shared: Arc<Shared>) {
    let number = {
        let mut connections = shared.connections();
        connections.push(connection.clone());
        connections.len() - 1
    };
    let authenticated = watch::Sender::new(false);
    let mut streams = JoinSet::new();
    loop {
        tokio::select! {
            stream = connection.accept_uni() => match stream {
                Ok(stream) => {
                    streams.spawn(authenticate(
                        stream,
                        connection.clone(),
                        number,
                        authenticated.clone(),
                        shared.clone(),
                    ));
                }
                Err(_) => return,
            },
            stream = connection.accept_bi() => match stream {
                Ok((send, recv)) => {
                    streams.spawn(relay(
                        send,
                        recv,
                        number,
                        authenticated.subscribe(),
                        shared.clone(),
                    ));
                }
                Err(_) => return,
            },
            // a stream that ended; nothing to collect
            Some(_) = streams.join_next() => {}
        }
    }
}

/// A unidirectional stream: the authentication, read whole — the client
/// finishes the stream after it — and checked against what the fake's own
/// side of the TLS session exports for its user, with the UUID's bytes as
/// the label and the password as the context. Let in, the connection's
/// Connects go on; refused, the connection is closed as the reference
/// servers close it, with no reason.
async fn authenticate(
    mut stream: RecvStream,
    connection: Connection,
    number: usize,
    authenticated: watch::Sender<bool>,
    shared: Arc<Shared>,
) {
    let Ok(command) = stream.read_to_end(64).await else {
        return;
    };
    let [VERSION, AUTHENTICATE, rest @ ..] = command.as_slice() else {
        return;
    };
    let Some((uuid, token)) = rest.split_first_chunk::<16>() else {
        return;
    };
    let Ok(token) = <[u8; 32]>::try_from(token) else {
        return;
    };
    let script = &shared.script;
    let mut expected = [0u8; 32];
    connection
        .export_keying_material(&mut expected, &script.uuid, script.password.as_bytes())
        .expect("the keying material of a session whose handshake is done");
    let accepted = *uuid == script.uuid && token == expected;
    shared.auths.lock().expect("auths").push(RecordedTuicAuth {
        connection: number,
        uuid: *uuid,
        token,
        expected,
        accepted,
    });
    if accepted {
        authenticated.send_replace(true);
    } else {
        connection.close(VarInt::from_u32(script.auth_failure_code), b"");
    }
}

/// A Connect: read and recorded, held until the authentication went
/// through, then relayed to its target, the half-closes too. One it cannot
/// carry out — no Connect, a name it has no `connect_to` for, a target
/// that refuses — it ends as sing-box ends one whose target it cannot
/// reach: STOP_SENDING (code 0) for what the client sends, FIN for what it
/// reads.
async fn relay(
    send: SendStream,
    mut recv: RecvStream,
    number: usize,
    mut authenticated: watch::Receiver<bool>,
    shared: Arc<Shared>,
) {
    let Some(address) = read_connect(&mut recv).await else {
        return refuse(send, recv);
    };
    let connect = RecordedConnect {
        connection: number,
        address: address.to_string(),
    };
    shared.connects.lock().expect("connects").push(connect);
    // ends with the connection when the authentication fails
    if authenticated.wait_for(|&let_in| let_in).await.is_err() {
        return;
    }
    // never resolves: a name without `connect_to` is a dead end
    let to = shared.script.connect_to.or(match address {
        Address::Ip(to) => Some(to),
        Address::Name(..) => None,
    });
    let upstream = match to {
        Some(to) => TcpStream::connect(to).await.ok(),
        None => None,
    };
    let Some(mut upstream) = upstream else {
        return refuse(send, recv);
    };
    let mut client = tokio::io::join(recv, send);
    // in large pieces, so that a bulk transfer measures the client rather
    // than this relay
    let _ = copy_bidirectional_with_sizes(&mut client, &mut upstream, RELAY, RELAY).await;
}

/// Ends a stream the fake does not relay.
fn refuse(mut send: SendStream, mut recv: RecvStream) {
    let _ = recv.stop(CLOSE);
    let _ = send.finish();
}

/// A target as a command names it.
enum Address {
    Name(String, u16),
    Ip(SocketAddr),
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Name(name, port) => write!(f, "{name}:{port}"),
            Address::Ip(addr) => addr.fmt(f),
        }
    }
}

/// The target of a Connect: `05 01 ‖ ADDR`; `None` for anything else.
async fn read_connect(recv: &mut RecvStream) -> Option<Address> {
    let mut header = [0u8; 2];
    recv.read_exact(&mut header).await.ok()?;
    if header != [VERSION, CONNECT] {
        return None;
    }
    read_address(recv).await
}

/// ADDR, a field at a time (SPEC "Address"): `0x00` a name, its length in
/// a byte first; `0x01` an IPv4 address; `0x02` an IPv6 address; then the
/// port. `None` for none (`0xff`), which names no target, and for a type
/// it does not know.
async fn read_address(recv: &mut RecvStream) -> Option<Address> {
    let ip = match recv.read_u8().await.ok()? {
        0x00 => {
            let mut name = vec![0u8; usize::from(recv.read_u8().await.ok()?)];
            recv.read_exact(&mut name).await.ok()?;
            let name = String::from_utf8(name).ok()?;
            let port = recv.read_u16().await.ok()?;
            return Some(Address::Name(name, port));
        }
        0x01 => {
            let mut octets = [0u8; 4];
            recv.read_exact(&mut octets).await.ok()?;
            IpAddr::from(octets)
        }
        0x02 => {
            let mut octets = [0u8; 16];
            recv.read_exact(&mut octets).await.ok()?;
            IpAddr::from(octets)
        }
        _ => return None,
    };
    let port = recv.read_u16().await.ok()?;
    Some(Address::Ip(SocketAddr::new(ip, port)))
}
