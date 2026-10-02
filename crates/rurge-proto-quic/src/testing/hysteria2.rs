//! A scriptable Hysteria 2 server (phase 2 M7 design 5.5): QUIC with ALPN
//! `h3` on a loopback port of its own (`ServerPort`: it can restart) or on
//! a socket it is given (`MultiPort` for port hopping), HTTP/3 for the
//! authentication, then TCP requests relayed to their targets. What the
//! client sends is read by code that is not the client's: the
//! authentication's fields by the HTTP/3 server side of `rurge_net`, the
//! TCP requests by the code here, which also writes the answers — every
//! length in them in two bytes, as QUIC allows. It never resolves a name.

use super::ServerPort;
use quinn::{AsyncUdpSocket, Connection, Endpoint, VarInt};
use rurge_net::quic::h3::testing::{H3Server, ServerStream};
use rurge_net::quic::testing::{TestCert, endpoint};
use rustls::RootCertStore;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

// the wire as the fake knows it
const HEADERS: u64 = 0x01;
const TCP_REQUEST: u64 = 0x401;
/// H3_NO_ERROR: how the fake closes its connections.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);
/// H3_REQUEST_REJECTED: a stream the fake does not serve.
const H3_REQUEST_REJECTED: u64 = 0x10b;

#[derive(Clone, Debug, Default)]
pub struct Hysteria2Script {
    /// The password it lets in; any other authentication is answered as a
    /// web site would answer it (404).
    pub password: String,
    /// Answer `Hysteria-UDP: false`.
    pub no_udp: bool,
    /// Refuse every TCP request with this message.
    pub refuse: Option<String>,
    /// Take TCP requests and never answer them.
    pub silent: bool,
    /// Relay here whatever the client asked for (needed for a name).
    pub connect_to: Option<SocketAddr>,
}

/// An authentication request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedAuth {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// Every field, in order: the pseudo-header fields first.
    pub fields: Vec<(String, String)>,
}

impl RecordedAuth {
    /// The first field called `name`.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A TCP request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedTcp {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// `host:port`, as the request carried it.
    pub address: String,
    /// How long its padding was.
    pub padding: usize,
}

pub struct FakeHysteria2 {
    inner: Arc<Inner>,
}

struct Inner {
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
    script: Hysteria2Script,
    /// The server's side of every connection accepted, in order.
    connections: Mutex<Vec<Connection>>,
    auths: Mutex<Vec<RecordedAuth>>,
    requests: Mutex<Vec<RecordedTcp>>,
    /// Restart once the next TCP request came.
    restart_at_request: AtomicBool,
    /// The fake, to restart it.
    fake: Weak<Inner>,
}

impl Shared {
    fn connections(&self) -> MutexGuard<'_, Vec<Connection>> {
        self.connections.lock().expect("connections")
    }
}

impl FakeHysteria2 {
    /// On a loopback port of its own, which it can take over again
    /// (`restart`).
    pub fn spawn(script: Hysteria2Script) -> FakeHysteria2 {
        let port = ServerPort::bind();
        let fake = FakeHysteria2::new(script, port.addr(), Some(port));
        fake.inner.take_over();
        fake
    }

    /// On `socket` — a `MultiPort` for port hopping —, which it cannot
    /// take over again.
    pub fn spawn_on(script: Hysteria2Script, socket: Arc<dyn AsyncUdpSocket>) -> FakeHysteria2 {
        let addr = socket.local_addr().expect("the socket's address");
        let fake = FakeHysteria2::new(script, addr, None);
        let config = fake.inner.cert.server_config(&["h3"]);
        fake.inner.listen(endpoint(config, socket));
        fake
    }

    fn new(script: Hysteria2Script, addr: SocketAddr, port: Option<ServerPort>) -> FakeHysteria2 {
        let inner = Arc::new_cyclic(|fake| Inner {
            cert: TestCert::new(),
            addr,
            port,
            shared: Arc::new(Shared {
                script,
                connections: Mutex::default(),
                auths: Mutex::default(),
                requests: Mutex::default(),
                restart_at_request: AtomicBool::new(false),
                fake: fake.clone(),
            }),
            endpoints: Mutex::default(),
        });
        FakeHysteria2 { inner }
    }

    /// Its port: the first of them, on a socket of several.
    pub fn addr(&self) -> SocketAddr {
        self.inner.addr
    }

    /// Roots that trust its certificate (for `localhost` and `127.0.0.1`).
    pub fn roots(&self) -> Arc<RootCertStore> {
        self.inner.cert.roots()
    }

    /// Every authentication request, in arrival order.
    pub fn auths(&self) -> Vec<RecordedAuth> {
        self.inner.shared.auths.lock().expect("auths").clone()
    }

    /// Every TCP request it read, in arrival order.
    pub fn tcp_requests(&self) -> Vec<RecordedTcp> {
        self.inner.shared.requests.lock().expect("requests").clone()
    }

    /// The connections accepted so far: QUIC's handshake done.
    pub fn connections(&self) -> usize {
        self.inner.shared.connections().len()
    }

    /// The server's side of connection `n`, 0 for the first accepted.
    pub fn connection(&self, n: usize) -> Connection {
        self.inner.shared.connections()[n].clone()
    }

    /// Closes every connection accepted so far (H3_NO_ERROR), as a server
    /// that goes down.
    pub fn close_connections(&self) {
        for connection in self.inner.shared.connections().iter() {
            connection.close(H3_NO_ERROR, b"");
        }
    }

    /// A new endpoint takes the port over, as after a restart: the
    /// connections of the one before are gone, and the client learns it
    /// from a stateless reset of its next packet.
    pub fn restart(&self) {
        self.inner.take_over();
    }

    /// The next TCP request finds the server gone: it restarts as `restart`
    /// does once the request came, before answering it.
    pub fn restart_at_next_request(&self) {
        self.inner
            .shared
            .restart_at_request
            .store(true, Ordering::SeqCst);
    }
}

impl Inner {
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

impl Drop for Inner {
    /// Every connection ends with the fake, and with them the tasks that
    /// serve them.
    fn drop(&mut self) {
        for (endpoint, _) in self.endpoints.get_mut().expect("endpoints").iter() {
            endpoint.close(H3_NO_ERROR, b"");
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

/// One connection: HTTP/3, and its streams, each in a task of its own that
/// ends with the connection.
async fn serve(connection: Connection, shared: Arc<Shared>) {
    let number = {
        let mut connections = shared.connections();
        connections.push(connection.clone());
        connections.len() - 1
    };
    let Ok(h3) = H3Server::start(connection, &[]).await else {
        return;
    };
    let authenticated = Arc::new(AtomicBool::new(false));
    let mut streams = JoinSet::new();
    loop {
        tokio::select! {
            stream = h3.accept() => match stream {
                Some(stream) => {
                    streams.spawn(serve_stream(
                        stream,
                        number,
                        authenticated.clone(),
                        shared.clone(),
                    ));
                }
                None => return,
            },
            // a stream that ended; nothing to collect
            Some(_) = streams.join_next() => {}
        }
    }
}

/// A stream by its first integer: an HTTP/3 request (the authentication)
/// or, once the connection is authenticated, a TCP request.
async fn serve_stream(
    mut stream: ServerStream,
    connection: usize,
    authenticated: Arc<AtomicBool>,
    shared: Arc<Shared>,
) {
    let _ = match stream.peek_varint().await {
        Ok(HEADERS) => authenticate(stream, connection, &authenticated, &shared).await,
        Ok(TCP_REQUEST) if authenticated.load(Ordering::SeqCst) => {
            relay(stream, connection, &shared).await
        }
        // before the authentication, or a stream of another kind
        Ok(_) => {
            stream.reset(H3_REQUEST_REJECTED);
            Ok(())
        }
        Err(_) => Ok(()),
    };
}

/// The authentication: 233 for the password, a web site's 404 for anything
/// else.
async fn authenticate(
    mut stream: ServerStream,
    connection: usize,
    authenticated: &AtomicBool,
    shared: &Shared,
) -> io::Result<()> {
    let fields = stream.read_headers().await?;
    stream.read_body().await?;
    let auth = RecordedAuth { connection, fields };
    let let_in = auth.field(":method") == Some("POST")
        && auth.field(":authority") == Some("hysteria")
        && auth.field(":path") == Some("/auth")
        && auth.field("hysteria-auth") == Some(shared.script.password.as_str());
    shared.auths.lock().expect("auths").push(auth);
    if let_in {
        authenticated.store(true, Ordering::SeqCst);
        let udp = if shared.script.no_udp {
            "false"
        } else {
            "true"
        };
        let fields = [
            ("hysteria-udp", udp),
            ("hysteria-cc-rx", "0"),
            ("hysteria-padding", "server-padding"),
        ];
        stream.send_headers(233, &fields).await?;
    } else {
        // what Go's `http.NotFound` answers
        stream
            .send_headers(404, &[("content-type", "text/plain; charset=utf-8")])
            .await?;
        stream.send_data(b"404 page not found\n").await?;
    }
    stream.finish();
    Ok(())
}

/// A TCP request: read and recorded, then the server restarts, leaves it
/// unanswered, refuses it, or answers it and relays it to its target, the
/// half-closes too. A request outside the reference's bounds ends the
/// stream without an answer, a target it cannot reach is refused with the
/// error's text, as the reference's server does
/// (`core/server/server.go`).
async fn relay(mut stream: ServerStream, connection: usize, shared: &Shared) -> io::Result<()> {
    // the type, peeked already
    read_varint(&mut stream).await?;
    let len = read_varint(&mut stream).await?;
    if !(1..=2048).contains(&len) {
        return Err(invalid("an address length the reference refuses"));
    }
    let mut address = vec![0u8; len as usize];
    stream.read_exact(&mut address).await?;
    let padding = read_varint(&mut stream).await?;
    if padding > 4096 {
        return Err(invalid("a padding length the reference refuses"));
    }
    stream.read_exact(&mut vec![0u8; padding as usize]).await?;
    let address = String::from_utf8_lossy(&address).into_owned();
    shared.requests.lock().expect("requests").push(RecordedTcp {
        connection,
        address: address.clone(),
        padding: padding as usize,
    });
    if shared.restart_at_request.swap(false, Ordering::SeqCst) {
        // gone before it answered: a new endpoint has the port
        if let Some(fake) = shared.fake.upgrade() {
            fake.take_over();
        }
        return Ok(());
    }
    if shared.script.silent {
        // what the client sends is dropped until it gives up
        tokio::io::copy(&mut stream, &mut tokio::io::sink()).await?;
        return Ok(());
    }
    if let Some(message) = &shared.script.refuse {
        stream.write_all(&answer(1, message.as_bytes())).await?;
        return stream.shutdown().await;
    }
    // never resolves: a name without `connect_to` is a dead end
    let upstream = match shared.script.connect_to.or_else(|| address.parse().ok()) {
        Some(to) => TcpStream::connect(to).await,
        None => Err(io::Error::other(format!("no address for {address}"))),
    };
    let mut upstream = match upstream {
        Ok(upstream) => upstream,
        Err(e) => {
            stream
                .write_all(&answer(1, e.to_string().as_bytes()))
                .await?;
            return stream.shutdown().await;
        }
    };
    // with the reference's message, which the client passes over
    stream.write_all(&answer(0, b"Connected")).await?;
    tokio::io::copy_bidirectional(&mut stream, &mut upstream).await?;
    Ok(())
}

/// A QUIC integer, a byte at a time.
async fn read_varint(stream: &mut ServerStream) -> io::Result<u64> {
    let first = stream.read_u8().await?;
    let mut value = u64::from(first & 0x3f);
    for _ in 1..1 << (first >> 6) {
        value = value << 8 | u64::from(stream.read_u8().await?);
    }
    Ok(value)
}

/// `status ‖ length ‖ message ‖ length ‖ padding`, each length in two
/// bytes: the client must read every length QUIC allows, not only the
/// shortest.
fn answer(status: u8, message: &[u8]) -> Vec<u8> {
    let mut out = vec![status];
    for field in [message, &[b'.'; 128][..]] {
        let len = u16::try_from(field.len())
            .ok()
            .filter(|&len| len < 1 << 14)
            .expect("a length of two bytes");
        out.extend_from_slice(&(0x4000 | len).to_be_bytes());
        out.extend_from_slice(field);
    }
    out
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("hysteria2 fake: {what}"),
    )
}
