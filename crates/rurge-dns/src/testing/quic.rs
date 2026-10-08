//! DNS servers over QUIC on the loopback: `FakeDoq` (RFC 9250) and
//! `FakeDoh3` (RFC 8484 over RFC 9114). Each answers from the table of a
//! `MockDns` — whose queries it records there too — on a port of its own
//! that can restart (`ServerPort`: the connections before end in a
//! stateless reset, or, `restart_without_resets`, are left unanswered), and
//! records what the client sent. Its certificate is for `localhost` and
//! `127.0.0.1` unless a script names others, which a resolver takes only
//! with `encrypted-dns-skip-cert-verification`.
//!
//! - `FakeDoq` reads a query's stream to its FIN before it answers, as
//!   dnsproxy does, and answers with the FIN after its reply.
//! - `FakeDoh3` reads a request — head, then content to its FIN — and
//!   answers `200` with `content-type: application/dns-message` and a
//!   `content-length`, unless its script says otherwise.
//! - A query the table drops (`MockDns::set_drop_all`) is left unanswered
//!   until the client gives up on it.

use super::{MockDns, State, respond};
use crate::upstream::doh::DNS_MESSAGE;
use quinn::{Connection, Endpoint, IdleTimeout, RecvStream, SendStream, TransportConfig, VarInt};
use rurge_net::quic::h3::testing::{H3Server, ServerStream};
use rurge_net::quic::testing::{ServerPort, TestCert};
use rurge_net::quic::{Transport, client_config};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::task::{JoinHandle, JoinSet};

// the wire as the fakes know it
const DOQ_NO_ERROR: VarInt = VarInt::from_u32(0x0);
const DOQ_INTERNAL_ERROR: VarInt = VarInt::from_u32(0x1);
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);
/// H3_REQUEST_REJECTED: a request past the fake's GOAWAY, never served.
const H3_REQUEST_REJECTED: u64 = 0x10b;
/// More than a client sends on a query's stream: a length and a message.
const MAX_QUERY: usize = 2 + 65_535 + 1;

#[derive(Clone, Debug, Default)]
pub struct DoqScript {
    /// Answer with this message ID instead of the query's.
    pub reply_id: Option<u16>,
    /// Send these bytes, then the FIN, on each query's stream instead of
    /// the answer and its length.
    pub reply_bytes: Option<Vec<u8>>,
    /// Reset each query's stream (DOQ_INTERNAL_ERROR) instead of answering.
    pub reset: bool,
    /// Close a connection that saw nothing for this long (QUIC's idle
    /// timeout): quinn's 30 seconds unless set.
    pub idle_timeout: Option<Duration>,
    /// Read nothing of a query's stream, and let the client write only this
    /// many bytes on it: a client with more to write waits, until it gives
    /// up and resets the stream (`FakeDoq::resets`).
    pub stall: Option<u32>,
    /// Present a certificate for these names, not `localhost` and
    /// `127.0.0.1`.
    pub cert_names: Option<Vec<String>>,
}

/// A query, as the fake read it: everything the client sent on its stream
/// before the FIN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedDoqQuery {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// Its QUIC stream ID.
    pub stream: u64,
    pub bytes: Vec<u8>,
}

impl RecordedDoqQuery {
    /// What follows the 2-byte length: the message.
    pub fn message(&self) -> &[u8] {
        self.bytes.get(2..).unwrap_or_default()
    }
}

/// A query the client gave up on, and the code of its STOP_SENDING.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedStop {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// Its QUIC stream ID.
    pub stream: u64,
    pub code: u64,
}

/// A query's stream the client reset before the fake had read it all, and
/// the code of its RESET_STREAM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedReset {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// Its QUIC stream ID.
    pub stream: u64,
    pub code: u64,
}

/// A DNS over QUIC server (ALPN `doq`).
pub struct FakeDoq {
    port: Port,
    shared: Arc<DoqShared>,
}

struct DoqShared {
    table: Arc<Mutex<State>>,
    script: DoqScript,
    queries: Mutex<Vec<RecordedDoqQuery>>,
    stops: Mutex<Vec<RecordedStop>>,
    resets: Mutex<Vec<RecordedReset>>,
}

impl FakeDoq {
    /// On a loopback port of its own, answering from `dns`'s table.
    pub fn spawn(dns: &MockDns, script: DoqScript) -> FakeDoq {
        FakeDoq::spawn_at(dns, script, SocketAddr::from(([127, 0, 0, 1], 0)))
    }

    /// As `spawn`, on `addr`.
    pub fn spawn_at(dns: &MockDns, script: DoqScript, addr: SocketAddr) -> FakeDoq {
        let cert = script
            .cert_names
            .as_ref()
            .map_or_else(TestCert::new, |names| {
                TestCert::for_names(&names.iter().map(String::as_str).collect::<Vec<_>>())
            });
        let fake = FakeDoq {
            port: Port::bind(addr, cert, "doq", DOQ_NO_ERROR)
                .idle_timeout(script.idle_timeout)
                .stream_window(script.stall),
            shared: Arc::new(DoqShared {
                table: dns.state.clone(),
                script,
                queries: Mutex::default(),
                stops: Mutex::default(),
                resets: Mutex::default(),
            }),
        };
        fake.listen(false);
        fake
    }

    fn listen(&self, new_key: bool) {
        let shared = self.shared.clone();
        self.port.listen(
            move |connection, number| serve_doq(connection, number, shared.clone()),
            new_key,
        );
    }

    pub fn addr(&self) -> SocketAddr {
        self.port.addr()
    }

    /// Every query it read, in arrival order.
    pub fn queries(&self) -> Vec<RecordedDoqQuery> {
        self.shared.queries.lock().expect("queries").clone()
    }

    /// The queries the client gave up on while they went unanswered.
    pub fn stops(&self) -> Vec<RecordedStop> {
        self.shared.stops.lock().expect("stops").clone()
    }

    /// The streams the client reset while the script stalled their reading.
    pub fn resets(&self) -> Vec<RecordedReset> {
        self.shared.resets.lock().expect("resets").clone()
    }

    /// The configuration of a client that verifies the fake's certificate:
    /// ALPN `doq`, TLS 1.3 and `Transport::DNS`, as
    /// `DoqUpstream::client_config` makes them with the fake's certificate
    /// among the roots.
    pub fn client_config(&self) -> quinn::ClientConfig {
        self.port.client_config()
    }

    /// The connections accepted so far: QUIC's handshake done.
    pub fn connections(&self) -> usize {
        self.port.connections().len()
    }

    /// The server's side of connection `n`, 0 for the first accepted.
    pub fn connection(&self, n: usize) -> Connection {
        self.port.connections()[n].clone()
    }

    /// Waits until the server has sent nothing for a while: what it has to
    /// say after an answer — acknowledgements, probes — is said, and the
    /// client that was to read it has.
    pub async fn idle(&self) {
        self.port.idle().await;
    }

    /// A new endpoint takes the port over, as after a restart: the
    /// connections of the one before are gone, and the client learns it
    /// from a stateless reset of its next packet.
    pub fn restart(&self) {
        self.listen(false);
    }

    /// As `restart`, of a server that lost the key of its stateless resets:
    /// the client is not told, and the connections of the one before go on
    /// talking to nobody.
    pub fn restart_without_resets(&self) {
        self.listen(true);
    }
}

/// One connection: each of the client's streams a query, in a task of its
/// own, until the connection ends.
async fn serve_doq(connection: Connection, number: usize, shared: Arc<DoqShared>) {
    let mut queries = JoinSet::new();
    loop {
        tokio::select! {
            stream = connection.accept_bi() => match stream {
                Ok((send, recv)) => {
                    queries.spawn(answer_doq(send, recv, number, shared.clone()));
                }
                Err(_) => return,
            },
            // a query that ended; nothing to collect
            Some(_) = queries.join_next() => {}
        }
    }
}

/// A query: read to its FIN and recorded; then reset, answered as the
/// script says or from the table, or left unanswered until the client gives
/// up on it.
async fn answer_doq(
    mut send: SendStream,
    mut recv: RecvStream,
    connection: usize,
    shared: Arc<DoqShared>,
) {
    let stream = u64::from(recv.id());
    if shared.script.stall.is_some() {
        // nothing is read: the client waits on its window until it gives up
        if let Ok(Some(code)) = recv.received_reset().await {
            shared.resets.lock().expect("resets").push(RecordedReset {
                connection,
                stream,
                code: code.into_inner(),
            });
        }
        return;
    }
    let Ok(bytes) = recv.read_to_end(MAX_QUERY).await else {
        return;
    };
    let query = RecordedDoqQuery {
        connection,
        stream,
        bytes,
    };
    shared.queries.lock().expect("queries").push(query.clone());
    if shared.script.reset {
        let _ = send.reset(DOQ_INTERNAL_ERROR);
        return;
    }
    if let Some(bytes) = &shared.script.reply_bytes {
        if send.write_all(bytes).await.is_ok() {
            let _ = send.finish();
        }
        return;
    }
    let Some((delay, mut reply)) = respond(&shared.table, query.message(), "doq") else {
        if let Ok(Some(code)) = send.stopped().await {
            shared.stops.lock().expect("stops").push(RecordedStop {
                connection,
                stream,
                code: code.into_inner(),
            });
        }
        return;
    };
    tokio::time::sleep(delay).await;
    if let Some(id) = shared.script.reply_id {
        reply[..2].copy_from_slice(&id.to_be_bytes());
    }
    let mut framed = (reply.len() as u16).to_be_bytes().to_vec();
    framed.extend_from_slice(&reply);
    if send.write_all(&framed).await.is_ok() {
        let _ = send.finish();
    }
}

#[derive(Clone, Debug, Default)]
pub struct Doh3Script {
    /// Answer with this status instead of 200.
    pub status: Option<u16>,
    /// Answer with this `content-type` instead of `application/dns-message`.
    pub content_type: Option<String>,
    /// Say this `content-length` instead of the answer's.
    pub content_length: Option<u64>,
    /// Answer with these bytes as the content instead of the table's.
    pub reply_bytes: Option<Vec<u8>>,
    /// Say the answer is this many seconds old (`age`).
    pub age: Option<u32>,
    /// Send GOAWAY once a connection has taken this many requests: those
    /// run to their end; a later one on that connection is rejected
    /// (H3_REQUEST_REJECTED).
    pub goaway_after: Option<usize>,
    /// Reject (H3_REQUEST_REJECTED) each request a connection gets once it
    /// has taken this many, without a GOAWAY.
    pub reject_after: Option<usize>,
    /// Close a connection that saw nothing for this long (QUIC's idle
    /// timeout): quinn's 30 seconds unless set.
    pub idle_timeout: Option<Duration>,
}

/// A request, as the fake read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedDoh3Request {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// Its QUIC stream ID.
    pub stream: u64,
    /// Its fields in order, the pseudo-header fields among them.
    pub fields: Vec<(String, String)>,
    /// Its content, to the FIN.
    pub body: Vec<u8>,
}

impl RecordedDoh3Request {
    /// The first field called `name`.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A DNS over HTTP/3 server (ALPN `h3`), whatever the request's path.
pub struct FakeDoh3 {
    port: Port,
    shared: Arc<Doh3Shared>,
}

struct Doh3Shared {
    table: Arc<Mutex<State>>,
    script: Doh3Script,
    requests: Mutex<Vec<RecordedDoh3Request>>,
}

impl FakeDoh3 {
    /// On a loopback port of its own, answering from `dns`'s table.
    pub fn spawn(dns: &MockDns, script: Doh3Script) -> FakeDoh3 {
        let fake = FakeDoh3 {
            port: Port::bind(
                SocketAddr::from(([127, 0, 0, 1], 0)),
                TestCert::new(),
                "h3",
                H3_NO_ERROR,
            )
            .idle_timeout(script.idle_timeout),
            shared: Arc::new(Doh3Shared {
                table: dns.state.clone(),
                script,
                requests: Mutex::default(),
            }),
        };
        fake.listen(false);
        fake
    }

    fn listen(&self, new_key: bool) {
        let shared = self.shared.clone();
        self.port.listen(
            move |connection, number| serve_doh3(connection, number, shared.clone()),
            new_key,
        );
    }

    pub fn addr(&self) -> SocketAddr {
        self.port.addr()
    }

    /// Every request it read, in arrival order.
    pub fn requests(&self) -> Vec<RecordedDoh3Request> {
        self.shared.requests.lock().expect("requests").clone()
    }

    /// The connections accepted so far: QUIC's handshake done.
    pub fn connections(&self) -> usize {
        self.port.connections().len()
    }

    /// The server's side of connection `n`, 0 for the first accepted.
    pub fn connection(&self, n: usize) -> Connection {
        self.port.connections()[n].clone()
    }

    /// Waits until the server has sent nothing for a while: what it has to
    /// say after an answer — acknowledgements, probes — is said, and the
    /// client that was to read it has.
    pub async fn idle(&self) {
        self.port.idle().await;
    }

    /// A new endpoint takes the port over, as after a restart: the
    /// connections of the one before are gone, and the client learns it
    /// from a stateless reset of its next packet.
    pub fn restart(&self) {
        self.listen(false);
    }

    /// As `restart`, of a server that lost the key of its stateless resets:
    /// the client is not told, and the connections of the one before go on
    /// talking to nobody.
    pub fn restart_without_resets(&self) {
        self.listen(true);
    }
}

/// One connection: HTTP/3, and each request in a task of its own, until
/// the connection ends. Once it took `goaway_after` requests it sends
/// GOAWAY, and rejects any request that comes after; `reject_after`
/// rejects them without.
async fn serve_doh3(connection: Connection, number: usize, shared: Arc<Doh3Shared>) {
    let Ok(mut h3) = H3Server::start(connection, &[]).await else {
        return;
    };
    let mut requests = JoinSet::new();
    let mut taken = 0;
    // the first stream its GOAWAY leaves unserved, once it sent one
    let mut unserved = None;
    loop {
        tokio::select! {
            stream = h3.accept() => match stream {
                Some(mut stream) => {
                    if unserved.is_some_and(|id| stream.id() >= id)
                        || shared.script.reject_after.is_some_and(|n| taken >= n)
                    {
                        stream.reset(H3_REQUEST_REJECTED);
                        continue;
                    }
                    taken += 1;
                    if shared.script.goaway_after == Some(taken) {
                        let id = stream.id() + 4;
                        let _ = h3.goaway(id).await;
                        unserved = Some(id);
                    }
                    requests.spawn(answer_doh3(stream, number, shared.clone()));
                }
                None => return,
            },
            // a request that ended; nothing to collect
            Some(_) = requests.join_next() => {}
        }
    }
}

/// A request: its head and its content to the FIN, recorded; then answered
/// from the table, or left unanswered until the client gives up on it.
async fn answer_doh3(
    mut stream: ServerStream,
    connection: usize,
    shared: Arc<Doh3Shared>,
) -> io::Result<()> {
    let fields = stream.read_headers().await?;
    let body = stream.read_body().await?;
    shared
        .requests
        .lock()
        .expect("requests")
        .push(RecordedDoh3Request {
            connection,
            stream: stream.id(),
            fields,
            body: body.clone(),
        });
    let Some((delay, reply)) = respond(&shared.table, &body, "doh3") else {
        stream.client_stopped().await;
        return Ok(());
    };
    tokio::time::sleep(delay).await;
    let script = &shared.script;
    let reply = script.reply_bytes.clone().unwrap_or(reply);
    let length = script
        .content_length
        .unwrap_or(reply.len() as u64)
        .to_string();
    let content_type = script.content_type.as_deref().unwrap_or(DNS_MESSAGE);
    let age = script.age.map(|age| age.to_string());
    let mut head = vec![
        ("content-type", content_type),
        ("content-length", length.as_str()),
    ];
    head.extend(age.as_deref().map(|age| ("age", age)));
    stream
        .send_headers(script.status.unwrap_or(200), &head)
        .await?;
    stream.send_data(&reply).await?;
    stream.finish();
    Ok(())
}

/// A loopback port whose endpoints present a certificate with one ALPN, and
/// the connections they accepted, in order.
struct Port {
    cert: TestCert,
    port: ServerPort,
    alpn: &'static str,
    idle_timeout: Option<Duration>,
    /// What a client may write on a stream before the server reads it:
    /// quinn's window unless set.
    stream_window: Option<u32>,
    /// What an endpoint closes its connections with when the fake goes.
    no_error: VarInt,
    connections: Arc<Mutex<Vec<Connection>>>,
    /// Every endpoint so far, with its accept loop: closed with the fake.
    endpoints: Mutex<Vec<(Endpoint, JoinHandle<()>)>>,
}

impl Port {
    fn bind(addr: SocketAddr, cert: TestCert, alpn: &'static str, no_error: VarInt) -> Port {
        Port {
            cert,
            port: ServerPort::bind_at(addr),
            alpn,
            idle_timeout: None,
            stream_window: None,
            no_error,
            connections: Arc::default(),
            endpoints: Mutex::default(),
        }
    }

    fn idle_timeout(mut self, idle_timeout: Option<Duration>) -> Port {
        self.idle_timeout = idle_timeout;
        self
    }

    fn stream_window(mut self, stream_window: Option<u32>) -> Port {
        self.stream_window = stream_window;
        self
    }

    fn addr(&self) -> SocketAddr {
        self.port.addr()
    }

    /// A client that verifies the certificate.
    fn client_config(&self) -> quinn::ClientConfig {
        client_config(self.cert.client_tls(&[self.alpn]), &Transport::DNS)
            .expect("a client configuration")
    }

    fn connections(&self) -> MutexGuard<'_, Vec<Connection>> {
        self.connections.lock().expect("connections")
    }

    /// Waits until the connections have sent nothing for a while.
    async fn idle(&self) {
        let sent = || {
            let connections = self.connections();
            connections
                .iter()
                .map(|connection| connection.stats().udp_tx.datagrams)
                .sum::<u64>()
        };
        let mut last = sent();
        loop {
            tokio::time::sleep(Duration::from_millis(60)).await;
            let now = sent();
            if now == last {
                return;
            }
            last = now;
        }
    }

    /// A new endpoint takes the port over — the one before falls silent —
    /// and serves each connection it accepts, in a task of its own, with
    /// `serve`, given its number. `new_key`: it does not have the key of
    /// the stateless resets of the endpoints before it.
    fn listen<F, S>(&self, serve: S, new_key: bool)
    where
        S: Fn(Connection, usize) -> F + Send + Sync + 'static,
        F: Future<Output = ()> + Send + 'static,
    {
        let mut transport = TransportConfig::default();
        if let Some(idle) = self.idle_timeout {
            transport.max_idle_timeout(Some(
                IdleTimeout::try_from(idle).expect("an idle timeout QUIC can say"),
            ));
        }
        if let Some(window) = self.stream_window {
            transport.stream_receive_window(VarInt::from_u32(window));
        }
        let mut config = self.cert.server_config(&[self.alpn]);
        config.transport_config(Arc::new(transport));
        let endpoint = if new_key {
            self.port.endpoint_with_new_key(config)
        } else {
            self.port.endpoint(config)
        };
        let connections = self.connections.clone();
        let serve = Arc::new(serve);
        let accepting = tokio::spawn({
            let endpoint = endpoint.clone();
            async move {
                while let Some(incoming) = endpoint.accept().await {
                    let (connections, serve) = (connections.clone(), serve.clone());
                    tokio::spawn(async move {
                        let Ok(connection) = incoming.await else {
                            return;
                        };
                        let number = {
                            let mut connections = connections.lock().expect("connections");
                            connections.push(connection.clone());
                            connections.len() - 1
                        };
                        serve(connection, number).await;
                    });
                }
            }
        });
        self.endpoints
            .lock()
            .expect("endpoints")
            .push((endpoint, accepting));
    }
}

impl Drop for Port {
    /// Every connection ends with the fake, and with them the tasks that
    /// serve them.
    fn drop(&mut self) {
        for (endpoint, accepting) in self.endpoints.get_mut().expect("endpoints").iter() {
            accepting.abort();
            endpoint.close(self.no_error, b"");
        }
    }
}
