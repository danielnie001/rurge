//! A scriptable CONNECT proxy over HTTP/3 — MASQUE's, or the TrustTunnel
//! endpoint's (phase 2 M7 design 7.4): QUIC with ALPN `h3` on a loopback
//! port of its own (`ServerPort`: it can restart) or on a socket it is
//! given (`MultiPort` for port hopping), HTTP/3 by the server side of
//! `rurge_net`, every classic CONNECT relayed to its target over TCP, the
//! half-closes too, and every CONNECT-UDP over UDP.
//!
//! - Its SETTINGS are masque-server's — field sections of at most 8 KiB,
//!   extended CONNECT, HTTP Datagrams — unless the script says otherwise.
//!   Scripted so, it greases as quiche does by default (masque-server and
//!   the TrustTunnel endpoint are quiche's).
//! - As masque-server and the TrustTunnel endpoint, it wants Basic
//!   credentials in `proxy-authorization` on every request when its script
//!   names users: a request without them, or with others, is answered 407
//!   with `proxy-authenticate` — masque-server's challenge, or the
//!   script's. Scripted so, a request without `user-agent` is answered 400.
//!   Every refusal is a final status with `content-length: 0`, then the end
//!   of the stream.
//! - A request whose head is not well formed — its pseudo-header fields out
//!   of place, unknown or twice, a classic CONNECT with `:scheme` or
//!   `:path`, an authority without a port — is answered 400; one with
//!   another method or with a protocol it does not serve, 501; one whose
//!   target it cannot reach, 502.
//! - A CONNECT-UDP (RFC 9298) names its target in the default template's
//!   path. As masque-server, the fake answers `200` with `capsule-protocol:
//!   ?1` once its socket to the target is up, then relays the client's
//!   HTTP/3 datagrams with context id 0 to the target and the target's
//!   answers back in HTTP/3 datagrams — or, scripted so, in DATAGRAM
//!   capsules on the stream —; it reads no capsule of the client's. It
//!   records every datagram of the client's, the bytes of DATA the client
//!   sent on such streams, and how the client ended each of them.
//! - A test may take the next request's stream as it came, to answer it by
//!   hand or reset it (`hold_next_request`).
//!
//! What the client sends is read by code that is not the client's: the
//! field sections by the HTTP/3 server side of `rurge_net`, the tunnels'
//! frames, the datagrams and the credentials by the code here. It records
//! every request, and never resolves a name.

use super::ServerPort;
use quinn::{AsyncUdpSocket, Connection, Endpoint, ServerConfig, TransportConfig, VarInt};
use rurge_net::quic::h3::testing::{H3Server, ServerStream};
use rurge_net::quic::testing::{TestCert, endpoint};
use rustls::RootCertStore;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

// the wire as the fake knows it
const DATA: u64 = 0x00;
const HEADERS: u64 = 0x01;
const SETTINGS_MAX_FIELD_SECTION_SIZE: u64 = 0x06;
const SETTINGS_ENABLE_CONNECT_PROTOCOL: u64 = 0x08;
const SETTINGS_H3_DATAGRAM: u64 = 0x33;
/// The DATAGRAM capsule (RFC 9297 3.5).
const DATAGRAM_CAPSULE: u64 = 0x00;
/// A capsule type reserved for greasing (RFC 9297 5.4: 0x29 × N + 0x17).
const RESERVED_CAPSULE: u64 = 0x29 * 7 + 0x17;
/// H3_NO_ERROR: how the fake closes its connections.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);
/// H3_REQUEST_REJECTED: a request past the fake's GOAWAY, never served.
const H3_REQUEST_REJECTED: u64 = 0x10b;
/// The largest field section masque-server takes, as its SETTINGS say.
const MAX_FIELD_SECTION_SIZE: u64 = 8192;
/// masque-server's `proxy-authenticate` on a 407.
const MASQUE_CHALLENGE: &str = "Basic realm=\"masque\"";
/// The pseudo-header fields of a request (RFC 9114 4.3.1, RFC 8441 4).
const PSEUDO: [&str; 5] = [":method", ":protocol", ":scheme", ":authority", ":path"];
/// What quiche writes in its reserved frame and on its reserved stream.
const GREASE: &[u8] = b"GREASE is the word";
/// How much of a TCP tunnel the fake relays at a time, each way.
const RELAY: usize = 64 << 10;
/// The longest frame of the client's the fake reads: past any it sends.
const MAX_FRAME: u64 = 1 << 20;

#[derive(Clone, Debug, Default)]
pub struct H3ProxyScript {
    /// Want the Basic credentials of one of these (user, password) on every
    /// request; with none, anyone gets in.
    pub users: Vec<(String, String)>,
    /// The `proxy-authenticate` of a 407, masque-server's unless set (the
    /// TrustTunnel endpoint's is `Basic realm=Authorization Required`).
    pub challenge: Option<String>,
    /// Answer a request without a `user-agent` 400, as an endpoint that
    /// follows the TrustTunnel document to the letter would (v1.1.0 only
    /// records it).
    pub require_user_agent: bool,
    /// Answer every request with this status instead of serving it.
    pub refuse: Option<u16>,
    /// Take requests and never answer them.
    pub silent: bool,
    /// Tunnel here whatever the client asked for (needed for a name).
    pub connect_to: Option<SocketAddr>,
    /// Send GOAWAY once a connection has taken this many requests: those run
    /// to their end; a later one on that connection is rejected
    /// (H3_REQUEST_REJECTED).
    pub goaway_after: Option<usize>,
    /// The most bidirectional streams the client may have open at once on a
    /// connection (QUIC's `initial_max_streams_bidi`): quinn's 100 unless
    /// set.
    pub max_streams: Option<u32>,
    /// The most the client may send on a stream before the fake reads it
    /// (QUIC's `stream_receive_window`): quinn's 1.25 MB unless set.
    pub stream_window: Option<u32>,
    /// Leave extended CONNECT out of its SETTINGS.
    pub no_extended_connect: bool,
    /// Say `H3_DATAGRAM = 0` in its SETTINGS.
    pub no_h3_datagram: bool,
    /// Take no QUIC datagrams at all: the transport parameter that allows
    /// them is not sent.
    pub no_datagrams: bool,
    /// Grease as quiche does: a setting of a reserved identifier, a
    /// unidirectional stream of a reserved type, and two frames of reserved
    /// types before every response head (RFC 9114 7.2.4.1, 6.2.3, 7.2.8).
    pub grease: bool,
    /// Answer a CONNECT-UDP request with this status and these fields in
    /// place of `200` and `capsule-protocol: ?1`, and serve it as ever.
    pub udp_response: Option<(u16, Vec<(String, String)>)>,
    /// End a CONNECT-UDP stream as soon as its head went out.
    pub udp_end_at_once: bool,
    /// Send what a CONNECT-UDP target answers in DATAGRAM capsules on the
    /// stream rather than in HTTP/3 datagrams, each after a capsule of a
    /// reserved type and a DATAGRAM capsule with context id 2, all of it
    /// across two DATA frames.
    pub udp_capsules: bool,
    /// Follow the head of a CONNECT-UDP response with a DATAGRAM capsule
    /// that has no room for a context id: a malformed one.
    pub udp_bad_capsule: bool,
}

/// A request, as the fake read its head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedH3Request {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// `:method`; empty without one.
    pub method: String,
    /// `:protocol` (extended CONNECT).
    pub protocol: Option<String>,
    pub scheme: Option<String>,
    pub authority: Option<String>,
    pub path: Option<String>,
    /// The other fields, in order.
    pub headers: Vec<(String, String)>,
}

impl RecordedH3Request {
    /// The pseudo-header fields of `fields` — the first of each name — and
    /// the others.
    fn new(connection: usize, fields: &[(String, String)]) -> RecordedH3Request {
        let pseudo = |name: &str| {
            fields
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
        };
        RecordedH3Request {
            connection,
            method: pseudo(":method").unwrap_or_default(),
            protocol: pseudo(":protocol"),
            scheme: pseudo(":scheme"),
            authority: pseudo(":authority"),
            path: pseudo(":path"),
            headers: fields
                .iter()
                .filter(|(name, _)| !name.starts_with(':'))
                .cloned()
                .collect(),
        }
    }

    /// The first header field called `name`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// One of the client's HTTP/3 datagrams on a CONNECT-UDP stream, as the
/// fake read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedH3Datagram {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// The QUIC stream ID of its stream.
    pub stream: u64,
    pub context: u64,
    /// The length of what follows the context id: the UDP payload.
    pub len: usize,
}

/// How the client ended its side of a CONNECT-UDP stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedUdpEnd {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// Its QUIC stream ID.
    pub stream: u64,
    /// With a FIN, gracefully; else with a reset, or with the connection.
    pub fin: bool,
}

pub struct FakeH3Proxy {
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
    script: H3ProxyScript,
    /// The server's side of every connection accepted, in order.
    connections: Mutex<Vec<Connection>>,
    requests: Mutex<Vec<RecordedH3Request>>,
    datagrams: Mutex<Vec<RecordedH3Datagram>>,
    /// The bytes of DATA the client sent on CONNECT-UDP streams.
    udp_data: AtomicUsize,
    udp_ends: Mutex<Vec<RecordedUdpEnd>>,
    /// Restart once the next request came.
    restart_at_request: AtomicBool,
    /// Where the next request's stream goes, once read and recorded,
    /// instead of being answered (`hold_next_request`).
    hold_next: Mutex<Option<oneshot::Sender<ServerStream>>>,
    /// The fake, to restart it.
    fake: Weak<Inner>,
}

impl Shared {
    fn connections(&self) -> MutexGuard<'_, Vec<Connection>> {
        self.connections.lock().expect("connections")
    }

    /// Its SETTINGS: masque-server's as the script leaves them, and a
    /// reserved one when it greases.
    fn settings(&self) -> Vec<(u64, u64)> {
        let script = &self.script;
        let mut settings = vec![(SETTINGS_MAX_FIELD_SECTION_SIZE, MAX_FIELD_SECTION_SIZE)];
        if !script.no_extended_connect {
            settings.push((SETTINGS_ENABLE_CONNECT_PROTOCOL, 1));
        }
        settings.push((SETTINGS_H3_DATAGRAM, u64::from(!script.no_h3_datagram)));
        if script.grease {
            settings.push((grease_value(), grease_value()));
        }
        settings
    }

    /// Whether `request` carries one of the script's users in
    /// `proxy-authorization`, once and exactly so; anyone gets in when the
    /// script names none.
    fn authorized(&self, request: &RecordedH3Request) -> bool {
        let users = &self.script.users;
        let mut given = request
            .headers
            .iter()
            .filter(|(name, _)| name == "proxy-authorization")
            .map(|(_, value)| value);
        match (given.next(), given.next()) {
            _ if users.is_empty() => true,
            (Some(value), None) => users
                .iter()
                .any(|(user, password)| *value == basic(user, password)),
            _ => false,
        }
    }
}

impl FakeH3Proxy {
    /// On a loopback port of its own, which it can take over again
    /// (`restart`).
    pub fn spawn(script: H3ProxyScript) -> FakeH3Proxy {
        let port = ServerPort::bind();
        let fake = FakeH3Proxy::new(script, port.addr(), Some(port));
        fake.inner.take_over();
        fake
    }

    /// On `socket` — a `MultiPort` for port hopping —, which it cannot
    /// take over again.
    pub fn spawn_on(script: H3ProxyScript, socket: Arc<dyn AsyncUdpSocket>) -> FakeH3Proxy {
        let addr = socket.local_addr().expect("the socket's address");
        let fake = FakeH3Proxy::new(script, addr, None);
        fake.inner
            .listen(endpoint(fake.inner.server_config(), socket));
        fake
    }

    fn new(script: H3ProxyScript, addr: SocketAddr, port: Option<ServerPort>) -> FakeH3Proxy {
        let inner = Arc::new_cyclic(|fake| Inner {
            cert: TestCert::new(),
            addr,
            port,
            shared: Arc::new(Shared {
                script,
                connections: Mutex::default(),
                requests: Mutex::default(),
                datagrams: Mutex::default(),
                udp_data: AtomicUsize::new(0),
                udp_ends: Mutex::default(),
                restart_at_request: AtomicBool::new(false),
                hold_next: Mutex::default(),
                fake: fake.clone(),
            }),
            endpoints: Mutex::default(),
        });
        FakeH3Proxy { inner }
    }

    /// Its port: the first of them, on a socket of several.
    pub fn addr(&self) -> SocketAddr {
        self.inner.addr
    }

    /// Roots that trust its certificate (for `localhost` and `127.0.0.1`).
    pub fn roots(&self) -> Arc<RootCertStore> {
        self.inner.cert.roots()
    }

    /// Every request it read, in arrival order.
    pub fn requests(&self) -> Vec<RecordedH3Request> {
        self.inner.shared.requests.lock().expect("requests").clone()
    }

    /// The client's datagrams on CONNECT-UDP streams, in arrival order.
    pub fn datagrams(&self) -> Vec<RecordedH3Datagram> {
        self.inner
            .shared
            .datagrams
            .lock()
            .expect("datagrams")
            .clone()
    }

    /// How the client ended its side of CONNECT-UDP streams, in order.
    pub fn udp_ends(&self) -> Vec<RecordedUdpEnd> {
        self.inner.shared.udp_ends.lock().expect("udp ends").clone()
    }

    /// The bytes of DATA the client sent on its CONNECT-UDP streams, all of
    /// them together: DATAGRAM capsules, were it to send its packets in
    /// any. Read as they come; all of a stream's once the client ended it
    /// with a FIN (`udp_ends`).
    pub fn udp_data(&self) -> usize {
        self.inner.shared.udp_data.load(Ordering::SeqCst)
    }

    /// An HTTP/3 datagram of `payload` — a context id first, or whatever a
    /// test wants — for stream `stream` of connection `n`.
    pub fn send_datagram(&self, n: usize, stream: u64, payload: &[u8]) {
        let mut datagram = Vec::with_capacity(8 + payload.len());
        put_varint(&mut datagram, stream / 4);
        datagram.extend_from_slice(payload);
        self.connection(n)
            .send_datagram(datagram.into())
            .expect("the client takes datagrams");
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

    /// The next request finds the server gone: it restarts as `restart`
    /// does once the request came, before answering it.
    pub fn restart_at_next_request(&self) {
        self.inner
            .shared
            .restart_at_request
            .store(true, Ordering::SeqCst);
    }

    /// The next request is not answered: once read and recorded, its
    /// stream comes out of what this returns, for the test to answer or
    /// reset by hand.
    pub fn hold_next_request(&self) -> oneshot::Receiver<ServerStream> {
        let (give, held) = oneshot::channel();
        *self.inner.shared.hold_next.lock().expect("hold") = Some(give);
        held
    }
}

impl Inner {
    /// QUIC with ALPN `h3`: datagrams included unless the script says
    /// none, quinn's limit on the client's streams unless it sets one.
    fn server_config(&self) -> ServerConfig {
        let script = &self.shared.script;
        let mut transport = TransportConfig::default();
        if script.no_datagrams {
            transport.datagram_receive_buffer_size(None);
        }
        if let Some(max) = script.max_streams {
            transport.max_concurrent_bidi_streams(VarInt::from_u32(max));
        }
        if let Some(window) = script.stream_window {
            transport.stream_receive_window(VarInt::from_u32(window));
        }
        let mut config = self.cert.server_config(&["h3"]);
        config.transport_config(Arc::new(transport));
        config
    }

    /// A new endpoint takes the port over; the one before falls silent.
    fn take_over(&self) {
        let port = self.port.as_ref().expect("a fake on a port of its own");
        self.listen(port.endpoint(self.server_config()));
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

/// One connection: HTTP/3, and each request in a task of its own, all
/// ending with the connection. Once it took `goaway_after` requests it
/// sends GOAWAY, and rejects any request that comes after.
async fn serve(connection: Connection, shared: Arc<Shared>) {
    let number = {
        let mut connections = shared.connections();
        connections.push(connection.clone());
        connections.len() - 1
    };
    let Ok(mut h3) = H3Server::start(connection.clone(), &shared.settings()).await else {
        return;
    };
    let mut tasks = JoinSet::new();
    if shared.script.grease {
        tasks.spawn(grease_stream(connection));
    }
    let mut taken = 0;
    // the first stream its GOAWAY leaves unserved, once it sent one
    let mut unserved = None;
    loop {
        tokio::select! {
            stream = h3.accept() => match stream {
                Some(mut stream) => {
                    if unserved.is_some_and(|id| stream.id() >= id) {
                        stream.reset(H3_REQUEST_REJECTED);
                        continue;
                    }
                    taken += 1;
                    if shared.script.goaway_after == Some(taken) {
                        let id = stream.id() + 4;
                        let _ = h3.goaway(id).await;
                        unserved = Some(id);
                    }
                    let shared = shared.clone();
                    tasks.spawn(async move {
                        let _ = answer(stream, number, shared).await;
                    });
                }
                None => return,
            },
            // a task that ended; nothing to collect
            Some(_) = tasks.join_next() => {}
        }
    }
}

/// A request: read and recorded; then the fake restarts, hands it to the
/// test, leaves it unanswered, refuses it, or makes the tunnel to its
/// target — its UDP relay for a CONNECT-UDP.
async fn answer(
    mut stream: ServerStream,
    connection: usize,
    shared: Arc<Shared>,
) -> io::Result<()> {
    let fields = stream.read_headers().await?;
    let request = RecordedH3Request::new(connection, &fields);
    shared
        .requests
        .lock()
        .expect("requests")
        .push(request.clone());
    if shared.restart_at_request.swap(false, Ordering::SeqCst) {
        // gone before it answered: a new endpoint has the port
        if let Some(fake) = shared.fake.upgrade() {
            fake.take_over();
        }
        return Ok(());
    }
    let held = shared.hold_next.lock().expect("hold").take();
    if let Some(give) = held {
        // the test's to answer
        let _ = give.send(stream);
        return Ok(());
    }
    let script = &shared.script;
    if script.silent {
        // what the client sends is dropped until it gives up
        tokio::io::copy(&mut stream, &mut tokio::io::sink()).await?;
        return Ok(());
    }
    let refusal = if !well_formed(&fields) {
        Some(400)
    } else if !shared.authorized(&request) {
        Some(407)
    } else if script.require_user_agent && request.header("user-agent").is_none() {
        Some(400)
    } else if script.refuse.is_some() {
        script.refuse
    } else if request.method != "CONNECT"
        || request
            .protocol
            .as_deref()
            .is_some_and(|protocol| protocol != "connect-udp")
    {
        Some(501)
    } else {
        None
    };
    if let Some(status) = refusal {
        return refuse(&mut stream, status, script).await;
    }
    if request.protocol.is_some() {
        return connect_udp(stream, &request, &shared).await;
    }
    // never resolves: a name without `connect_to` is a dead end
    let to = script
        .connect_to
        .or_else(|| request.authority.as_deref()?.parse().ok());
    let upstream = match to {
        Some(to) => TcpStream::connect(to).await.ok(),
        None => None,
    };
    // a target it cannot reach, as masque-server answers it
    let Some(upstream) = upstream else {
        return refuse(&mut stream, 502, script).await;
    };
    respond(&mut stream, 200, &[], script.grease).await?;
    tunnel(stream, upstream).await
}

/// Whether the head of a request is well formed as the fake reads it: its
/// pseudo-header fields — a request's, each once — before the others,
/// whose names are lowercase (RFC 9114 4.2, 4.3); a classic CONNECT with
/// an authority `host:port` and neither `:scheme` nor `:path` (RFC 9114
/// 4.4), an extended one with all three (RFC 8441 4), any other request
/// with `:scheme` and `:path`.
fn well_formed(fields: &[(String, String)]) -> bool {
    let first_regular = fields
        .iter()
        .position(|(name, _)| !name.starts_with(':'))
        .unwrap_or(fields.len());
    let (pseudo, regular) = fields.split_at(first_regular);
    let count = |name: &str| pseudo.iter().filter(|(n, _)| n == name).count();
    if pseudo
        .iter()
        .any(|(name, _)| !PSEUDO.contains(&name.as_str()) || count(name) > 1)
        || regular
            .iter()
            .any(|(name, _)| name.starts_with(':') || name.bytes().any(|b| b.is_ascii_uppercase()))
    {
        return false;
    }
    let value = |name: &str| {
        pseudo
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    let (authority, scheme, path) = (value(":authority"), value(":scheme"), value(":path"));
    match (value(":method"), value(":protocol")) {
        (Some("CONNECT"), None) => {
            authority.is_some_and(authority_form) && scheme.is_none() && path.is_none()
        }
        (Some("CONNECT"), Some(_)) => {
            authority.is_some_and(|a| !a.is_empty()) && scheme.is_some() && path.is_some()
        }
        (Some(_), None) => scheme.is_some() && path.is_some(),
        _ => false,
    }
}

/// `host:port`, as a CONNECT names its target (RFC 9112 3.2.3): a port from
/// 1 to 65535 after a host, an IPv6 literal in brackets.
fn authority_form(authority: &str) -> bool {
    let Some((host, port)) = authority.rsplit_once(':') else {
        return false;
    };
    let host_ok = match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        Some(literal) => literal.parse::<Ipv6Addr>().is_ok(),
        None => !host.is_empty() && !host.contains([':', '[', ']', '@', '/']),
    };
    host_ok && port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p > 0)
}

/// A final status and no tunnel, as masque-server refuses: `content-length:
/// 0` — after the script's challenge, or masque-server's, for a 407 — and
/// the end of the stream.
async fn refuse(stream: &mut ServerStream, status: u16, script: &H3ProxyScript) -> io::Result<()> {
    let mut fields = Vec::new();
    if status == 407 {
        let challenge = script.challenge.as_deref();
        fields.push(("proxy-authenticate", challenge.unwrap_or(MASQUE_CHALLENGE)));
    }
    fields.push(("content-length", "0"));
    respond(stream, status, &fields, script.grease).await?;
    stream.finish();
    Ok(())
}

/// The response head: when the fake greases, after a frame of a reserved
/// type without payload and one with, as quiche writes them before its
/// first HEADERS on a stream.
async fn respond(
    stream: &mut ServerStream,
    status: u16,
    fields: &[(&str, &str)],
    grease: bool,
) -> io::Result<()> {
    if grease {
        stream.send_frame(grease_value(), b"").await?;
        stream.send_frame(grease_value(), GREASE).await?;
    }
    stream.send_headers(status, fields).await
}

/// The client's DATA frames to the target, and what the target sends back
/// in DATA frames of the fake's; each side's end is the other's
/// half-close. Either side failing ends both.
async fn tunnel(stream: ServerStream, upstream: TcpStream) -> io::Result<()> {
    let (mut from_client, mut to_client) = tokio::io::split(stream);
    let (mut from_target, mut to_target) = upstream.into_split();
    let up = async {
        while let Some(data) = read_data(&mut from_client).await? {
            to_target.write_all(&data).await?;
        }
        to_target.shutdown().await
    };
    let down = async {
        let mut buf = vec![0u8; RELAY];
        loop {
            match from_target.read(&mut buf).await? {
                0 => return to_client.shutdown().await,
                n => to_client.write_all(&frame(DATA, &buf[..n])).await?,
            }
        }
    };
    tokio::try_join!(up, down).map(|_| ())
}

/// A CONNECT-UDP request (RFC 9298), as masque-server serves one: the
/// target in its path — a name only with `connect_to` —, a UDP socket that
/// hears the target alone, the head once the socket is up, then the
/// client's datagrams with context id 0 to the target and the target's
/// answers back, until the client ends its side of the stream or the
/// connection ends.
async fn connect_udp(
    mut stream: ServerStream,
    request: &RecordedH3Request,
    shared: &Shared,
) -> io::Result<()> {
    let script = &shared.script;
    let Some((host, port)) = request.path.as_deref().and_then(masque_target) else {
        return refuse(&mut stream, 400, script).await;
    };
    // never resolves: a name without `connect_to` is a dead end
    let to = script
        .connect_to
        .or_else(|| Some(SocketAddr::new(host.parse().ok()?, port)));
    let socket = match to {
        Some(to) => udp_socket(to).await.ok(),
        None => None,
    };
    // a target it cannot reach, as masque-server answers it
    let Some(socket) = socket else {
        return refuse(&mut stream, 502, script).await;
    };
    let (connection, id) = (request.connection, stream.id());
    let quic = shared.connections()[connection].clone();
    // taken before the head: whatever the client sends after it is heard
    let mut datagrams = stream.datagrams();
    let (status, fields) = match &script.udp_response {
        Some((status, fields)) => (
            *status,
            fields
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect(),
        ),
        None => (200, vec![("capsule-protocol", "?1")]),
    };
    respond(&mut stream, status, &fields, script.grease).await?;
    if script.udp_bad_capsule {
        stream.send_data(&capsule(DATAGRAM_CAPSULE, b"")).await?;
    }
    if script.udp_end_at_once {
        stream.finish();
        return Ok(());
    }
    let (mut from_client, mut to_client) = tokio::io::split(stream);
    let up = async {
        while let Some(datagram) = datagrams.recv().await {
            // the client sends none without a context id
            let Some((context, n)) = parse_varint(&datagram) else {
                continue;
            };
            shared
                .datagrams
                .lock()
                .expect("datagrams")
                .push(RecordedH3Datagram {
                    connection,
                    stream: id,
                    context,
                    len: datagram.len() - n,
                });
            if context == 0 {
                let _ = socket.send(&datagram[n..]).await;
            }
        }
    };
    let down = async {
        let mut buf = vec![0u8; 65536];
        loop {
            let reply = match socket.recv(&mut buf).await {
                Ok(n) => &buf[..n],
                // an ICMP "unreachable" for an earlier datagram (Windows)
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(_) => return,
            };
            if !script.udp_capsules {
                let mut datagram = Vec::with_capacity(16 + reply.len());
                put_varint(&mut datagram, id / 4);
                put_varint(&mut datagram, 0);
                datagram.extend_from_slice(reply);
                // what QUIC cannot take now is lost, as on any hop
                let _ = quic.send_datagram(datagram.into());
                continue;
            }
            let mut capsules = capsule(RESERVED_CAPSULE, GREASE);
            for context in [2, 0] {
                let mut value = vec![context];
                value.extend_from_slice(reply);
                capsules.extend(capsule(DATAGRAM_CAPSULE, &value));
            }
            // capsules run on from one DATA frame to the next (RFC 9297 3.1)
            let (first, second) = capsules.split_at(capsules.len() / 2);
            for part in [first, second] {
                if to_client.write_all(&frame(DATA, part)).await.is_err() {
                    return;
                }
            }
        }
    };
    let end = async {
        // nothing comes on the stream but its end: DATA is counted
        let fin = loop {
            match read_data(&mut from_client).await {
                Ok(Some(data)) => {
                    shared.udp_data.fetch_add(data.len(), Ordering::SeqCst);
                }
                Ok(None) => break true,
                Err(_) => break false,
            }
        };
        shared
            .udp_ends
            .lock()
            .expect("udp ends")
            .push(RecordedUdpEnd {
                connection,
                stream: id,
                fin,
            });
    };
    // the socket lives as long as the stream (RFC 9298 3.1)
    tokio::select! {
        () = up => {}
        () = down => {}
        () = end => {}
    }
    Ok(())
}

/// The target in `/.well-known/masque/udp/{host}/{port}/`, the default
/// template's path (RFC 9298 2): the host percent-decoded, and the port.
fn masque_target(path: &str) -> Option<(String, u16)> {
    let rest = path
        .strip_prefix("/.well-known/masque/udp/")?
        .strip_suffix('/')?;
    let (host, port) = rest.split_once('/')?;
    let mut decoded = Vec::new();
    let mut bytes = host.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            decoded.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            decoded.push(b);
        }
    }
    Some((String::from_utf8(decoded).ok()?, port.parse().ok()?))
}

/// A loopback UDP socket that sends to `to` and hears `to` alone.
async fn udp_socket(to: SocketAddr) -> io::Result<UdpSocket> {
    let local = if to.is_ipv4() {
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
    } else {
        SocketAddr::from((Ipv6Addr::LOCALHOST, 0))
    };
    let socket = UdpSocket::bind(local).await?;
    socket.connect(to).await?;
    Ok(socket)
}

/// The payload of the client's next DATA frame on a tunnel, frames of
/// unknown types passed over; `None` at the end of the stream. HEADERS —
/// trailers — have no place there (RFC 9114 4.4).
async fn read_data(from: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<Vec<u8>>> {
    loop {
        let Some(ty) = read_varint(from).await? else {
            return Ok(None);
        };
        let len = read_varint(from).await?.ok_or_else(cut_short)?;
        if len > MAX_FRAME {
            return Err(invalid("a frame longer than the fake reads"));
        }
        let mut payload = vec![0u8; len as usize];
        from.read_exact(&mut payload).await?;
        match ty {
            DATA => return Ok(Some(payload)),
            HEADERS => return Err(invalid("HEADERS on a tunnel")),
            _ => {}
        }
    }
}

/// A QUIC integer, a byte at a time; `None` at the end of the stream
/// before its first byte.
async fn read_varint(from: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<u64>> {
    let mut first = [0u8];
    if from.read(&mut first).await? == 0 {
        return Ok(None);
    }
    let mut value = u64::from(first[0] & 0x3f);
    for _ in 1..1 << (first[0] >> 6) {
        value = value << 8 | u64::from(from.read_u8().await?);
    }
    Ok(Some(value))
}

/// The QUIC integer at the start of `bytes`, and its length; `None` when
/// `bytes` ends within it.
fn parse_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let len = 1 << (bytes.first()? >> 6);
    let bytes = bytes.get(..len)?;
    let value = bytes[1..]
        .iter()
        .fold(u64::from(bytes[0] & 0x3f), |value, &b| {
            value << 8 | u64::from(b)
        });
    Some((value, len))
}

/// `value` as a QUIC integer, as short as it goes.
fn put_varint(out: &mut Vec<u8>, value: u64) {
    match value {
        0..0x40 => out.push(value as u8),
        0x40..0x4000 => out.extend_from_slice(&(0x4000 | value as u16).to_be_bytes()),
        0x4000..0x4000_0000 => out.extend_from_slice(&(0x8000_0000 | value as u32).to_be_bytes()),
        _ => out.extend_from_slice(&(0xc000_0000_0000_0000 | value).to_be_bytes()),
    }
}

fn frame(ty: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + payload.len());
    put_varint(&mut out, ty);
    put_varint(&mut out, payload.len() as u64);
    out.extend_from_slice(payload);
    out
}

/// A capsule (RFC 9297 3.2): its type, its length and its value, as a
/// frame goes.
fn capsule(ty: u64, value: &[u8]) -> Vec<u8> {
    frame(ty, value)
}

/// A unidirectional stream of a reserved type, as quiche opens one: the
/// client stops it, and HTTP/3 goes on (RFC 9114 6.2.3).
async fn grease_stream(connection: Connection) {
    let Ok(mut send) = connection.open_uni().await else {
        return;
    };
    let mut bytes = Vec::new();
    put_varint(&mut bytes, grease_value());
    bytes.extend_from_slice(GREASE);
    if send.write_all(&bytes).await.is_ok() {
        let _ = send.finish();
    }
}

/// A reserved value as quiche draws one: 0x1f × N + 0x21 (RFC 9114 7.2.8),
/// N as large as a QUIC integer allows.
fn grease_value() -> u64 {
    const N: u64 = 148_764_065_110_560_899;
    0x1f * getrandom::u64().map_or(0, |n| n % N) + 0x21
}

/// `Basic base64(user:password)`, standard Base64 with its padding (RFC
/// 7617 2, RFC 4648 4): the fake's own encoding, not the client's.
fn basic(user: &str, password: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::from("Basic ");
    for chunk in format!("{user}:{password}").as_bytes().chunks(3) {
        let bits = chunk
            .iter()
            .enumerate()
            .fold(0u32, |bits, (i, &b)| bits | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                char::from(ALPHABET[(bits >> (18 - 6 * i)) as usize & 0x3f])
            } else {
                '='
            });
        }
    }
    out
}

fn cut_short() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "h3 proxy fake: the stream ended within a frame",
    )
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("h3 proxy fake: {what}"))
}
