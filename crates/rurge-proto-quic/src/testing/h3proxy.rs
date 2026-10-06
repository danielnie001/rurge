//! A scriptable MASQUE proxy (phase 2 M7 design 7.4): QUIC with ALPN `h3`
//! on a loopback port of its own (`ServerPort`: it can restart) or on a
//! socket it is given (`MultiPort` for port hopping), HTTP/3 by the server
//! side of `rurge_net`, and every classic CONNECT relayed to its target
//! over TCP, the half-closes too.
//!
//! - Its SETTINGS are masque-server's — field sections of at most 8 KiB,
//!   extended CONNECT, HTTP Datagrams — unless the script says otherwise.
//!   Scripted so, it greases as quiche does by default (masque-server and
//!   the TrustTunnel endpoint are quiche's).
//! - As masque-server, it wants Basic credentials in `proxy-authorization`
//!   on every request when its script names users: a request without them,
//!   or with others, is answered 407 with `proxy-authenticate`. Every
//!   refusal is a final status with `content-length: 0`, then the end of
//!   the stream.
//! - A request whose head is not well formed — its pseudo-header fields out
//!   of place, unknown or twice, a classic CONNECT with `:scheme` or
//!   `:path`, an authority without a port — is answered 400; one with
//!   another method or with a protocol it does not serve, 501; one whose
//!   target it cannot reach, 502.
//!
//! What the client sends is read by code that is not the client's: the
//! field sections by the HTTP/3 server side of `rurge_net`, the tunnels'
//! frames and the credentials by the code here. It records every request,
//! and never resolves a name.

use super::ServerPort;
use quinn::{AsyncUdpSocket, Connection, Endpoint, ServerConfig, TransportConfig, VarInt};
use rurge_net::quic::h3::testing::{H3Server, ServerStream};
use rurge_net::quic::testing::{TestCert, endpoint};
use rustls::RootCertStore;
use std::io;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

// the wire as the fake knows it
const DATA: u64 = 0x00;
const HEADERS: u64 = 0x01;
const SETTINGS_MAX_FIELD_SECTION_SIZE: u64 = 0x06;
const SETTINGS_ENABLE_CONNECT_PROTOCOL: u64 = 0x08;
const SETTINGS_H3_DATAGRAM: u64 = 0x33;
/// H3_NO_ERROR: how the fake closes its connections.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);
/// H3_REQUEST_REJECTED: a request past the fake's GOAWAY, never served.
const H3_REQUEST_REJECTED: u64 = 0x10b;
/// The largest field section masque-server takes, as its SETTINGS say.
const MAX_FIELD_SECTION_SIZE: u64 = 8192;
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
    /// Restart once the next request came.
    restart_at_request: AtomicBool,
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
                restart_at_request: AtomicBool::new(false),
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

/// A request: read and recorded; then the fake restarts, leaves it
/// unanswered, refuses it, or makes the tunnel to its target.
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
    } else if script.refuse.is_some() {
        script.refuse
    } else if request.method != "CONNECT" || request.protocol.is_some() {
        Some(501)
    } else {
        None
    };
    if let Some(status) = refusal {
        return refuse(&mut stream, status, script.grease).await;
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
        return refuse(&mut stream, 502, script.grease).await;
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
/// 0` — after the challenge, for a 407 — and the end of the stream.
async fn refuse(stream: &mut ServerStream, status: u16, grease: bool) -> io::Result<()> {
    let mut fields = Vec::new();
    if status == 407 {
        fields.push(("proxy-authenticate", "Basic realm=\"masque\""));
    }
    fields.push(("content-length", "0"));
    respond(stream, status, &fields, grease).await?;
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
