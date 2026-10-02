//! A scriptable Hysteria 2 server (phase 2 M7 design 5.5): QUIC with ALPN
//! `h3` on a loopback port of its own (`ServerPort`: it can restart) or on
//! a socket it is given (`MultiPort` for port hopping), HTTP/3 for the
//! authentication, then TCP requests relayed to their targets and UDP
//! sessions, each with a loopback socket of its own (full cone). What the
//! client sends is read by code that is not the client's: the
//! authentication's fields by the HTTP/3 server side of `rurge_net`, the
//! TCP requests and the UDP messages by the code here, which also writes
//! the answers — every length in them in two bytes, as QUIC allows — and
//! splits a UDP answer too long for one datagram into fragments of its
//! own. It never resolves a name. Scripted so, its socket obfuscates as
//! well, Salamander or Gecko, with code of its own too (`obfs`).

use super::ServerPort;
use super::obfs::{RecordedFragment, ServerObfs};
use quinn::{
    AsyncUdpSocket, Connection, Endpoint, SendDatagramError, ServerConfig, TransportConfig, VarInt,
};
use rurge_config::spec::Hysteria2Obfs;
use rurge_net::quic::h3::testing::{H3Server, ServerStream};
use rurge_net::quic::testing::{TestCert, endpoint};
use rustls::RootCertStore;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

// the wire as the fake knows it
const HEADERS: u64 = 0x01;
const TCP_REQUEST: u64 = 0x401;
/// H3_NO_ERROR: how the fake closes its connections.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);
/// H3_REQUEST_REJECTED: a stream the fake does not serve.
const H3_REQUEST_REJECTED: u64 = 0x10b;
/// How much of a TCP tunnel the fake relays at a time, each way.
const RELAY: usize = 64 << 10;

#[derive(Clone, Debug, Default)]
pub struct Hysteria2Script {
    /// The password it lets in; any other authentication is answered as a
    /// web site would answer it (404).
    pub password: String,
    /// Answer `Hysteria-UDP: false`, and take no UDP.
    pub no_udp: bool,
    /// Take no QUIC datagrams at all: the transport parameter that allows
    /// them is not sent.
    pub no_datagrams: bool,
    /// The first UDP answer it splits into fragments goes without this
    /// one.
    pub drop_fragment: Option<u8>,
    /// Refuse every TCP request with this message.
    pub refuse: Option<String>,
    /// Take TCP requests and never answer them.
    pub silent: bool,
    /// Relay here whatever the client asked for, TCP and UDP alike (needed
    /// for a name).
    pub connect_to: Option<SocketAddr>,
    /// Obfuscate every packet so, with this password.
    pub obfs: Option<Hysteria2Obfs>,
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

/// A UDP message from the client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedUdp {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    pub session: u32,
    pub packet: u16,
    pub fragment: u8,
    pub fragments: u8,
    /// `host:port`, as the message carried it.
    pub address: String,
    /// How long its payload was: a fragment's own part, for a fragment.
    pub len: usize,
}

/// A UDP message as the fake reads and writes it, for tests that send
/// their own: `u32 session ‖ u16 packet ‖ u8 fragment ‖ u8 fragments ‖
/// varint length ‖ host:port ‖ payload`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UdpMessage {
    pub session: u32,
    pub packet: u16,
    pub fragment: u8,
    pub fragments: u8,
    /// `host:port`.
    pub address: String,
    pub payload: Vec<u8>,
}

impl UdpMessage {
    /// A datagram that goes whole: packet 0, fragment 0 of 1.
    pub fn whole(session: u32, address: &str, payload: &[u8]) -> UdpMessage {
        UdpMessage {
            session,
            packet: 0,
            fragment: 0,
            fragments: 1,
            address: address.to_string(),
            payload: payload.to_vec(),
        }
    }

    /// The address's length in two bytes, whatever it is: the client must
    /// read every length QUIC allows, not only the shortest.
    pub fn encode(&self) -> Vec<u8> {
        let len = u16::try_from(self.address.len())
            .ok()
            .filter(|&len| len < 1 << 14)
            .expect("an address length of two bytes");
        let mut out = Vec::with_capacity(10 + self.address.len() + self.payload.len());
        out.extend_from_slice(&self.session.to_be_bytes());
        out.extend_from_slice(&self.packet.to_be_bytes());
        out.extend_from_slice(&[self.fragment, self.fragments]);
        out.extend_from_slice(&(0x4000 | len).to_be_bytes());
        out.extend_from_slice(self.address.as_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// `None` for what the reference server would not read
    /// (`ParseUDPMessage`): an address of none or more than 2048 bytes, a
    /// message without a payload.
    fn decode(datagram: &[u8]) -> Option<UdpMessage> {
        let (fixed, rest) = datagram.split_at_checked(8)?;
        let first = *rest.first()?;
        let (integer, rest) = rest.split_at_checked(1 << (first >> 6))?;
        let len = integer[1..]
            .iter()
            .fold(u64::from(first & 0x3f), |len, &b| len << 8 | u64::from(b));
        let len = usize::try_from(len)
            .ok()
            .filter(|len| (1..=2048).contains(len) && *len < rest.len())?;
        let (address, payload) = rest.split_at(len);
        Some(UdpMessage {
            session: u32::from_be_bytes([fixed[0], fixed[1], fixed[2], fixed[3]]),
            packet: u16::from_be_bytes([fixed[4], fixed[5]]),
            fragment: fixed[6],
            fragments: fixed[7],
            address: String::from_utf8_lossy(address).into_owned(),
            payload: payload.to_vec(),
        })
    }
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
    /// What tells each of them to send GOAWAY, in the same order.
    goaways: Mutex<Vec<Arc<Notify>>>,
    auths: Mutex<Vec<RecordedAuth>>,
    requests: Mutex<Vec<RecordedTcp>>,
    udp: Mutex<Vec<RecordedUdp>>,
    /// Each UDP session's own socket, in the order they opened.
    outside: Mutex<Vec<SocketAddr>>,
    /// The client's Gecko fragments, as the endpoints' sockets read them.
    fragments: Arc<Mutex<Vec<RecordedFragment>>>,
    /// The fragment the next UDP answer in fragments goes without.
    drop_fragment: Mutex<Option<u8>>,
    /// Restart once the next TCP request came.
    restart_at_request: AtomicBool,
    /// The fake, to restart it.
    fake: Weak<Inner>,
}

impl Shared {
    fn connections(&self) -> MutexGuard<'_, Vec<Connection>> {
        self.connections.lock().expect("connections")
    }

    /// `message` in datagrams of at most `max` bytes: whole when it fits,
    /// else in fragments under a random packet id. The first datagram split
    /// goes without the fragment the script drops.
    fn split(&self, message: UdpMessage, max: usize) -> Vec<Vec<u8>> {
        let whole = message.encode();
        if whole.len() <= max {
            return vec![whole];
        }
        let room = max - (whole.len() - message.payload.len());
        let fragments = u8::try_from(message.payload.len().div_ceil(room)).expect("255 fragments");
        let packet = getrandom::u32().map_or(1, |random| (random % 0xffff) as u16 + 1);
        let dropped = self.drop_fragment.lock().expect("drop").take();
        message
            .payload
            .chunks(room)
            .zip(0..fragments)
            .filter(|&(_, fragment)| Some(fragment) != dropped)
            .map(|(chunk, fragment)| {
                UdpMessage {
                    session: message.session,
                    packet,
                    fragment,
                    fragments,
                    address: message.address.clone(),
                    payload: chunk.to_vec(),
                }
                .encode()
            })
            .collect()
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
        let config = fake.inner.server_config();
        let socket = fake.inner.obfuscated(socket);
        fake.inner.listen(endpoint(config, socket));
        fake
    }

    fn new(script: Hysteria2Script, addr: SocketAddr, port: Option<ServerPort>) -> FakeHysteria2 {
        let inner = Arc::new_cyclic(|fake| Inner {
            cert: TestCert::new(),
            addr,
            port,
            shared: Arc::new(Shared {
                drop_fragment: Mutex::new(script.drop_fragment),
                script,
                connections: Mutex::default(),
                goaways: Mutex::default(),
                auths: Mutex::default(),
                requests: Mutex::default(),
                udp: Mutex::default(),
                outside: Mutex::default(),
                fragments: Arc::default(),
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

    /// Every UDP message it read, in arrival order: each fragment apart.
    pub fn udp_messages(&self) -> Vec<RecordedUdp> {
        self.inner.shared.udp.lock().expect("udp").clone()
    }

    /// Each UDP session's own socket, in the order they opened: whoever
    /// sends to one is heard by the client.
    pub fn udp_outside(&self) -> Vec<SocketAddr> {
        self.inner.shared.outside.lock().expect("outside").clone()
    }

    /// Every Gecko fragment of the client's it read, in arrival order.
    pub fn gecko_fragments(&self) -> Vec<RecordedFragment> {
        self.inner
            .shared
            .fragments
            .lock()
            .expect("fragments")
            .clone()
    }

    /// Sends `message`, as it is, to the client on connection `n`.
    pub fn send_udp(&self, n: usize, message: &UdpMessage) {
        self.connection(n)
            .send_datagram(message.encode().into())
            .expect("a datagram the client takes");
    }

    /// The connections accepted so far: QUIC's handshake done.
    pub fn connections(&self) -> usize {
        self.inner.shared.connections().len()
    }

    /// The server's side of connection `n`, 0 for the first accepted.
    pub fn connection(&self, n: usize) -> Connection {
        self.inner.shared.connections()[n].clone()
    }

    /// Sends GOAWAY on connection `n`, as a server about to go down: the
    /// client takes nothing new there, and what is under way goes on.
    pub fn goaway(&self, n: usize) {
        self.inner.shared.goaways.lock().expect("goaways")[n].notify_one();
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
    /// QUIC with ALPN `h3`, datagrams included unless the script says
    /// none.
    fn server_config(&self) -> ServerConfig {
        let mut config = self.cert.server_config(&["h3"]);
        if self.shared.script.no_datagrams {
            let mut transport = TransportConfig::default();
            transport.datagram_receive_buffer_size(None);
            config.transport_config(Arc::new(transport));
        }
        config
    }

    /// `socket` under the script's obfuscation, if any.
    fn obfuscated(&self, socket: Arc<dyn AsyncUdpSocket>) -> Arc<dyn AsyncUdpSocket> {
        let (password, gecko) = match &self.shared.script.obfs {
            None => return socket,
            Some(Hysteria2Obfs::Salamander(password)) => (password, false),
            Some(Hysteria2Obfs::Gecko(password)) => (password, true),
        };
        let heard = self.shared.fragments.clone();
        Arc::new(ServerObfs::new(socket, password.expose(), gecko, heard))
    }

    /// A new endpoint takes the port over; the one before falls silent.
    fn take_over(&self) {
        let port = self.port.as_ref().expect("a fake on a port of its own");
        let endpoint = port.endpoint_over(self.server_config(), |socket| self.obfuscated(socket));
        self.listen(endpoint);
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

/// One connection: HTTP/3, its streams and its datagrams, each in a task
/// of its own that ends with the connection.
async fn serve(connection: Connection, shared: Arc<Shared>) {
    let goaway = Arc::new(Notify::new());
    let number = {
        let mut connections = shared.connections();
        connections.push(connection.clone());
        shared.goaways.lock().expect("goaways").push(goaway.clone());
        connections.len() - 1
    };
    let Ok(mut h3) = H3Server::start(connection.clone(), &[]).await else {
        return;
    };
    let authenticated = Arc::new(AtomicBool::new(false));
    let mut streams = JoinSet::new();
    if !shared.script.no_udp {
        streams.spawn(relay_udp(
            connection,
            number,
            authenticated.clone(),
            shared.clone(),
        ));
    }
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
            () = goaway.notified() => {
                // the authentication, on stream 0, is the one HTTP/3
                // request a connection carries
                let _ = h3.goaway(4).await;
            }
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
    // in large pieces, so that a bulk transfer measures the client rather
    // than this relay (the throughput benchmark)
    tokio::io::copy_bidirectional_with_sizes(&mut stream, &mut upstream, RELAY, RELAY).await?;
    Ok(())
}

/// One of the client's UDP sessions on a connection: a loopback socket of
/// its own, and the datagram it is putting together.
struct UdpSession {
    socket: Arc<UdpSocket>,
    /// The packet id and the fragments so far.
    partial: Option<(u16, Vec<Option<Vec<u8>>>)>,
    _answers: AbortOnDropHandle<()>,
}

impl UdpSession {
    /// Its socket, and the task that sends back whatever reaches it.
    fn open(connection: &Connection, session: u32, shared: &Arc<Shared>) -> io::Result<UdpSession> {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0")?;
        socket.set_nonblocking(true)?;
        let socket = Arc::new(UdpSocket::from_std(socket)?);
        shared
            .outside
            .lock()
            .expect("outside")
            .push(socket.local_addr()?);
        let answers = tokio::spawn(relay_answers(
            socket.clone(),
            connection.clone(),
            session,
            shared.clone(),
        ));
        Ok(UdpSession {
            socket,
            partial: None,
            _answers: AbortOnDropHandle::new(answers),
        })
    }

    /// The datagram `message` makes whole: itself when it is no fragment.
    fn put_together(&mut self, message: UdpMessage) -> Option<Vec<u8>> {
        if message.fragments <= 1 {
            return Some(message.payload);
        }
        let (index, count) = (
            usize::from(message.fragment),
            usize::from(message.fragments),
        );
        if index >= count {
            return None;
        }
        if !matches!(&self.partial, Some((packet, parts))
            if *packet == message.packet && parts.len() == count)
        {
            self.partial = Some((message.packet, vec![None; count]));
        }
        let (_, parts) = self.partial.as_mut()?;
        parts[index].get_or_insert(message.payload);
        if parts.iter().any(Option::is_none) {
            return None;
        }
        let (_, parts) = self.partial.take()?;
        Some(parts.into_iter().flatten().flatten().collect())
    }
}

/// The client's datagrams on `connection`, once it is authenticated (as
/// the reference serves UDP only then): each session gets a socket of its
/// own with its first message, which every datagram of the session leaves
/// by, put together from its fragments first.
async fn relay_udp(
    connection: Connection,
    number: usize,
    authenticated: Arc<AtomicBool>,
    shared: Arc<Shared>,
) {
    let mut sessions = HashMap::new();
    while let Ok(datagram) = connection.read_datagram().await {
        if !authenticated.load(Ordering::SeqCst) {
            continue;
        }
        let Some(message) = UdpMessage::decode(&datagram) else {
            continue;
        };
        shared.udp.lock().expect("udp").push(RecordedUdp {
            connection: number,
            session: message.session,
            packet: message.packet,
            fragment: message.fragment,
            fragments: message.fragments,
            address: message.address.clone(),
            len: message.payload.len(),
        });
        let session = match sessions.entry(message.session) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => match UdpSession::open(&connection, message.session, &shared) {
                Ok(session) => entry.insert(session),
                Err(_) => continue,
            },
        };
        // never resolves: a name without `connect_to` is a dead end
        let to = shared
            .script
            .connect_to
            .or_else(|| message.address.parse().ok());
        if let (Some(to), Some(payload)) = (to, session.put_together(message)) {
            let _ = session.socket.send_to(&payload, to).await;
        }
    }
}

/// Whatever reaches a session's socket goes back to the client under its
/// source — full cone —, in fragments when one datagram cannot hold it.
async fn relay_answers(
    socket: Arc<UdpSocket>,
    connection: Connection,
    session: u32,
    shared: Arc<Shared>,
) {
    let mut buf = vec![0u8; 65536];
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(got) => got,
            // an ICMP "unreachable" for an earlier datagram (Windows)
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(_) => return,
        };
        let Some(max) = connection.max_datagram_size() else {
            return;
        };
        let message = UdpMessage::whole(session, &from.to_string(), &buf[..n]);
        for datagram in shared.split(message, max) {
            if let Err(SendDatagramError::ConnectionLost(_)) =
                connection.send_datagram(datagram.into())
            {
                return;
            }
        }
    }
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
