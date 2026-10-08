//! UDP over MASQUE (manual: Policies › MASQUE; phase 2 M7 design 7.1):
//! CONNECT-UDP (RFC 9298) on the policy's QUIC connection, a request
//! stream of its own for each target, the packets in HTTP/3 datagrams (RFC
//! 9297 2.1).
//!
//! - The request is an extended CONNECT (RFC 9220): `:protocol
//!   connect-udp`, `:scheme https`, `:authority` the server as its TLS
//!   names it, `:path` by the default template
//!   (`/.well-known/masque/udp/{host}/{port}/`), then `proxy-authorization`
//!   when the policy has credentials, and `capsule-protocol: ?1` (RFC 9297
//!   3.4). It goes once the server's SETTINGS allow extended CONNECT and
//!   HTTP Datagrams and the connection carries QUIC datagrams (the manual:
//!   "The server must advertise support for extended CONNECT and HTTP
//!   Datagrams"); without them the datagram fails, and the connection goes
//!   on carrying TCP.
//! - Nothing goes before the proxy's 2xx, which must be one the Capsule
//!   Protocol allows (RFC 9297 3.2): without content, and none of 204, 205
//!   and 206.
//! - A stream is bound to its target, so every packet it brings back counts
//!   as the target's (symmetric, as `h2-connect`'s). Each packet goes in one
//!   HTTP/3 datagram, `varint(0) ‖ payload` (context id 0); one longer than
//!   a datagram of the connection carries now is refused — no DATAGRAM
//!   capsule takes it instead (RFC 9297 3.5, RFC 9298 6.1; masque-server
//!   reads none on HTTP/3).
//! - Coming back, the stream's HTTP/3 datagrams and the DATAGRAM capsules of
//!   its data alike are the target's packets when their context id is 0;
//!   other contexts and capsule types are passed over (RFC 9298 4, RFC 9297
//!   3.2), and a capsule that is malformed ends the stream.
//! - A target's stream is opened with its first datagram, once for those
//!   that come meanwhile, and closes gracefully (FIN) after `IDLE` without
//!   a datagram either way, or once the server ended its side. Once its
//!   connection takes nothing new — ended, going away, or dialled another
//!   way than the policy leads now (`Session::serves`: where the policy
//!   leads is asked at most once a second) —, the target's next datagram
//!   opens it anew on the session's connection. A datagram that meets its
//!   stream ended — the one opened for it, or for those it waited with —
//!   opens one anew, once, then fails: a proxy may end every stream at
//!   once.

use super::{Http3, proxy, refused};
use crate::carrier::{Inbox, Packet, RouteClock, bounded};
use crate::connect::{Connected, failure};
use crate::session::{Failure, Lease, Protocol, Session};
use crate::varint;
use bytes::Bytes;
use http::header::{
    CONTENT_LENGTH, CONTENT_TYPE, HeaderName, HeaderValue, PROXY_AUTHORIZATION, TRANSFER_ENCODING,
};
use http::{Request, Response};
use quinn::Connection;
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, PacketSocket, Target};
use rurge_net::quic::h3::{
    self, Datagrams, RequestStream, SETTINGS_ENABLE_CONNECT_PROTOCOL, SETTINGS_H3_DATAGRAM,
    Settings,
};
use rurge_proto::connect_udp::{self, MAX_PAYLOAD, masque_path};
use std::collections::HashMap;
use std::io;
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::{OnceCell, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

/// The tests count on the inbox's size.
#[cfg(test)]
use crate::carrier::INBOX;

/// A target's stream closes after this long without a datagram either way:
/// the engine's flow idle (M5-D7), as `h2-connect`'s.
pub(super) const IDLE: Duration = Duration::from_secs(60);
/// `capsule-protocol` (RFC 9297 3.4).
const CAPSULE_PROTOCOL: HeaderName = HeaderName::from_static("capsule-protocol");

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A target's stream, opened once for the datagrams that come meanwhile.
type Slot = Arc<OnceCell<Arc<Stream>>>;

type Streams = Mutex<HashMap<Target, Slot>>;

/// The CONNECT-UDP request for `target` (RFC 9298 3.4), to `authority`:
/// refused, nothing opened, for a name that cannot be sent.
fn request(
    authority: &str,
    target: &Target,
    authorization: Option<&HeaderValue>,
) -> io::Result<Request<()>> {
    let unsendable = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "masque: the host name cannot be sent to the server",
        )
    };
    let path = masque_path(target).ok_or_else(unsendable)?;
    let mut request = Request::connect(format!("https://{authority}{path}"))
        .body(())
        // a name longer than a URI may be
        .map_err(|_| unsendable())?;
    request.extensions_mut().insert(h3::Protocol::CONNECT_UDP);
    let headers = request.headers_mut();
    if let Some(authorization) = authorization {
        headers.insert(PROXY_AUTHORIZATION, authorization.clone());
    }
    headers.insert(CAPSULE_PROTOCOL, HeaderValue::from_static("?1"));
    Ok(request)
}

/// Whether the server takes CONNECT-UDP with HTTP Datagrams: extended
/// CONNECT and `H3_DATAGRAM = 1` in its SETTINGS (RFC 9220 3, RFC 9297
/// 2.1.1), and QUIC datagrams on the connection (RFC 9221 3).
fn supports_connect_udp(settings: &Settings, connection: &Connection) -> bool {
    settings.get(SETTINGS_ENABLE_CONNECT_PROTOCOL) == Some(1)
        && settings.get(SETTINGS_H3_DATAGRAM) == Some(1)
        && connection.max_datagram_size().is_some()
}

/// Whether a 2xx breaks the Capsule Protocol's rules (RFC 9297 3.2): a
/// response with content — `content-length`, `content-type` or
/// `transfer-encoding` — or a 204, 205 or 206 is malformed.
fn breaks_capsule_protocol(response: &Response<()>) -> bool {
    matches!(response.status().as_u16(), 204..=206)
        || [CONTENT_LENGTH, CONTENT_TYPE, TRANSFER_ENCODING]
            .iter()
            .any(|name| response.headers().contains_key(name))
}

/// What a CONNECT-UDP request opened.
struct Opened {
    lease: Lease<Connected>,
    stream: RequestStream,
    datagrams: Datagrams,
}

/// The CONNECT-UDP `request` on the connection of `lease`, once the
/// server's SETTINGS allow it, and the proxy's response. Until the response
/// came, a request the server never processed is `Lost`, as a TCP one
/// (`connect::failure`).
async fn connect(lease: Lease<Connected>, request: Request<()>) -> Result<Opened, Failure> {
    let h3 = &lease.state().h3;
    let settings = h3.settings().await.map_err(|e| failure(&lease, None, e))?;
    if !supports_connect_udp(&settings, lease.connection()) {
        return Err(Failure::Failed(proxy(
            "the server does not support CONNECT-UDP",
        )));
    }
    let mut stream = h3
        .request(request)
        .await
        .map_err(|e| failure(&lease, None, e))?;
    let id = stream.id();
    // taken before the response: what the proxy sends right after its 2xx
    // is not lost
    let datagrams = stream
        .datagrams()
        .await
        .map_err(|e| failure(&lease, Some(id), e))?;
    let response = stream
        .response()
        .await
        .map_err(|e| failure(&lease, Some(id), e))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Failure::Failed(refused(status)));
    }
    if breaks_capsule_protocol(&response) {
        return Err(Failure::Failed(proxy(
            "the proxy's response to CONNECT-UDP is malformed",
        )));
    }
    Ok(Opened {
        lease,
        stream,
        datagrams,
    })
}

/// A target's stream, as the carrier's datagrams take it.
struct Stream {
    /// Tells it from a stream that took its place in the table.
    id: u64,
    /// The connection it is on (`Session::serves`), kept while it lives.
    lease: Lease<Connected>,
    datagrams: Arc<Datagrams>,
    /// The last datagram either way: the idle clock.
    last: Arc<Mutex<Instant>>,
    /// Its reading ended: so did the stream.
    ended: CancellationToken,
    /// Reads the stream (`read`); ends with it.
    _reader: AbortOnDropHandle<()>,
}

/// Why a datagram did not go.
enum Unsent {
    /// It is longer than a datagram of the stream carries now: this long.
    TooLong(usize),
    /// The stream, or its connection, ended.
    Ended,
}

impl Stream {
    /// The longest UDP payload a datagram of the stream carries now: what
    /// its HTTP/3 datagram takes past the context id, at most
    /// `MAX_PAYLOAD`.
    fn room(&self) -> usize {
        // context id 0 takes one byte
        self.datagrams
            .max_payload()
            .saturating_sub(1)
            .min(MAX_PAYLOAD)
    }

    /// `payload` in one HTTP/3 datagram with context id 0 (RFC 9298 5),
    /// never waiting: QUIC sends it when it can, or drops it as any hop
    /// may.
    fn send(&self, payload: &[u8]) -> Result<(), Unsent> {
        if self.ended.is_cancelled() {
            return Err(Unsent::Ended);
        }
        let room = self.room();
        if payload.len() > room {
            return Err(Unsent::TooLong(room));
        }
        let mut datagram = Vec::with_capacity(1 + payload.len());
        // context id 0
        datagram.push(0);
        datagram.extend_from_slice(payload);
        match self.datagrams.send(&datagram) {
            Ok(()) => {
                *self.last.lock().expect("clock") = Instant::now();
                Ok(())
            }
            // the path narrowed since `room` was taken
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => Err(Unsent::TooLong(self.room())),
            // its datagrams flow no more: the stream, or its connection,
            // ended
            Err(_) => Err(Unsent::Ended),
        }
    }
}

fn too_long(max: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("masque: a datagram longer than {max} bytes"),
    )
}

/// One client association's UDP through the outbound (design 7.1): a
/// stream of its own for each target, on the policy's connection. No
/// `Debug`: it holds the credentials.
pub(super) struct MasqueUdp {
    session: Arc<Session<Http3>>,
    /// The server's own `:authority`.
    authority: String,
    /// `proxy-authorization`, marked sensitive.
    authorization: Option<HeaderValue>,
    /// The budget of opening a stream, a dial included.
    opts: ConnectOpts,
    idle: Duration,
    streams: Arc<Streams>,
    /// When the policy's route was last compared.
    route: RouteClock,
    /// What the streams' readers fill.
    inbox: Inbox,
}

impl MasqueUdp {
    /// A carrier that opens nothing yet: each target's stream comes with
    /// its first datagram.
    pub(super) fn new(
        session: Arc<Session<Http3>>,
        authority: String,
        authorization: Option<HeaderValue>,
        opts: ConnectOpts,
        idle: Duration,
    ) -> MasqueUdp {
        MasqueUdp {
            session,
            authority,
            authorization,
            opts,
            idle,
            streams: Arc::default(),
            route: RouteClock::default(),
            inbox: Inbox::new(),
        }
    }

    /// `to`'s slot: the table's, else a new one put there.
    fn slot(&self, to: &Target) -> Slot {
        let mut streams = self.streams.lock().expect("streams");
        streams.entry(to.clone()).or_default().clone()
    }

    /// Forgets `slot` for `to`, unless another has taken its place.
    fn forget(&self, to: &Target, slot: &Slot) {
        let forgotten = {
            let mut streams = self.streams.lock().expect("streams");
            match streams.get(to) {
                Some(s) if Arc::ptr_eq(s, slot) => streams.remove(to),
                _ => None,
            }
        };
        // its stream goes once nothing else holds it, out of the lock
        drop(forgotten);
    }

    /// Whether `stream` takes the target's next datagram: its reading goes
    /// on, and its connection takes what is new (`Session::serves`) — where
    /// the policy leads is asked at most once a second, the rest every
    /// time.
    fn serves(&self, stream: &Stream) -> bool {
        !stream.ended.is_cancelled() && self.route.serves(&self.session, &stream.lease)
    }

    /// `to`'s stream: the one it has while that takes the datagram, else
    /// one opened now, once for every datagram to `to` meanwhile. One
    /// opened for the datagram — or for those it waited with — and found
    /// ended already is opened anew once (`reopen`).
    async fn stream(&self, to: &Target, reopened: &mut bool) -> io::Result<Arc<Stream>> {
        loop {
            let slot = self.slot(to);
            if let Some(stream) = slot.get() {
                if self.serves(stream) {
                    return Ok(stream.clone());
                }
                if !stream.ended.is_cancelled() {
                    tracing::debug!("masque: a UDP stream moves to a new connection");
                }
                // the target's next stream goes on the session's connection
                self.forget(to, &slot);
                continue;
            }
            let stream = match slot.get_or_try_init(|| self.open(to)).await {
                Ok(stream) => stream.clone(),
                Err(e) => {
                    self.forget(to, &slot);
                    return Err(e);
                }
            };
            // A sender waiting on a cell whose opener failed opens again,
            // after that opener forgot the slot: the stream must be the
            // table's, or the next datagram to `to` opens another.
            let mut streams = self.streams.lock().expect("streams");
            // looked at under the lock its reader leaves the table with: an
            // ended stream is never put back
            if stream.ended.is_cancelled() {
                reopen(reopened)?;
                continue;
            }
            match streams.get(to) {
                None => {
                    streams.insert(to.clone(), slot.clone());
                    return Ok(stream);
                }
                Some(s) if Arc::ptr_eq(s, &slot) => return Ok(stream),
                // another slot took its place: use that one
                Some(_) => {}
            }
        }
    }

    /// A stream to `to` on the session's connection, and the task that
    /// reads it: one budget for the connection (when one is dialed), QUIC's
    /// handshake, the server's SETTINGS and the CONNECT-UDP exchange. A
    /// request that finds the connection gone goes again, once, on a new
    /// one (`Session::request`).
    async fn open(&self, to: &Target) -> io::Result<Arc<Stream>> {
        // checked first: nothing is dialed for a name that cannot be sent
        let request = request(&self.authority, to, self.authorization.as_ref())?;
        let opening = self.session.request(&self.opts, |lease| {
            Box::pin(connect(lease, request.clone()))
        });
        let Opened {
            lease,
            stream,
            datagrams,
        } = bounded(&self.opts, opening).await?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let datagrams = Arc::new(datagrams);
        let last = Arc::new(Mutex::new(Instant::now()));
        let ended = CancellationToken::new();
        let reader = tokio::spawn({
            let (datagrams, last, ended) = (datagrams.clone(), last.clone(), ended.clone());
            let (answers, to, idle) = (self.inbox.sender().clone(), to.clone(), self.idle);
            let streams = Arc::downgrade(&self.streams);
            async move {
                let why = read(stream, &datagrams, &to, &answers, &last, idle).await;
                tracing::debug!("masque: a UDP stream ended: {why}");
                ended.cancel();
                // the table's no more, unless another took its place
                if let Some(streams) = streams.upgrade() {
                    leave(&streams, &to, id);
                }
            }
        });
        Ok(Arc::new(Stream {
            id,
            lease,
            datagrams,
            last,
            ended,
            _reader: AbortOnDropHandle::new(reader),
        }))
    }

    /// Test support: the targets with a stream, or one being opened.
    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.streams.lock().expect("streams").len()
    }

    /// Test support: the longest payload `to`'s stream carries now.
    #[cfg(test)]
    fn room(&self, to: &Target) -> Option<usize> {
        let slot = self.streams.lock().expect("streams").get(to)?.clone();
        slot.get().map(|stream| stream.room())
    }
}

/// A datagram met its stream ended before it could go: it opens one anew,
/// once — a proxy may end every stream at once —, then fails.
fn reopen(reopened: &mut bool) -> io::Result<()> {
    if std::mem::replace(reopened, true) {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "masque: the server ended the UDP stream",
        ));
    }
    Ok(())
}

/// Takes `to`'s stream `id` out of `streams`, unless another has taken its
/// place.
fn leave(streams: &Streams, to: &Target, id: u64) {
    let left = {
        let mut streams = streams.lock().expect("streams");
        let ours = streams
            .get(to)
            .is_some_and(|slot| slot.get().is_some_and(|stream| stream.id == id));
        if ours { streams.remove(to) } else { None }
    };
    // the stream goes once nothing else holds it, out of the lock
    drop(left);
}

/// How reading a stream ended.
enum End {
    /// Without a datagram either way for the idle time, or with the server's
    /// end of its side: the stream closes gracefully.
    Closed(&'static str),
    /// A capsule that is malformed, or the stream's failure.
    Failed(String),
}

/// Reads `to`'s stream until it ends: its HTTP/3 datagrams and the
/// DATAGRAM capsules of its data with context id 0 are `to`'s packets, for
/// `answers` — which drops them when full: the reader waits for nobody.
/// Once the stream idled for `idle`, or the server ended its side, the FIN
/// goes and nothing more is read; a failure leaves the stream to its drop,
/// which cancels it. Says why it ended.
async fn read(
    stream: RequestStream,
    datagrams: &Datagrams,
    to: &Target,
    answers: &mpsc::Sender<Packet>,
    last: &Mutex<Instant>,
    idle: Duration,
) -> String {
    let deliver = |packet: Bytes| {
        *last.lock().expect("clock") = Instant::now();
        let _ = answers.try_send((packet, to.clone()));
    };
    let mut reader = BufReader::new(stream);
    // the datagrams end with the stream or its connection: reading the
    // stream says which
    let mut flowing = true;
    let end = 'stream: loop {
        // a capsule is read whole: neither a datagram nor the idle clock
        // may cut one in two, so the read lives on across them
        let mut capsule = pin!(connect_udp::read_datagram(&mut reader, Http3::NAME));
        loop {
            let deadline = *last.lock().expect("clock") + idle;
            tokio::select! {
                read = &mut capsule => match read {
                    Ok(Some(payload)) => {
                        deliver(payload.into());
                        break;
                    }
                    Ok(None) => break 'stream End::Closed("the server ended it"),
                    // the text names the fault, never the payload
                    Err(e) => break 'stream End::Failed(format!("read error: {e}")),
                },
                datagram = datagrams.recv(), if flowing => match datagram {
                    Some(datagram) => {
                        if let Some(packet) = udp_payload(datagram) {
                            deliver(packet);
                        }
                    }
                    None => flowing = false,
                },
                () = tokio::time::sleep_until(deadline) => {
                    // a datagram may have moved the deadline: look again
                    if *last.lock().expect("clock") + idle <= Instant::now() {
                        break 'stream End::Closed("idle");
                    }
                }
            }
        }
    };
    match end {
        End::Closed(why) => {
            // what the server sends from now on is not read: the stream's
            // drop stops it
            let _ = reader.shutdown().await;
            why.to_string()
        }
        End::Failed(why) => why,
    }
}

/// The UDP payload of an HTTP/3 datagram of the stream: what follows its
/// context id, when that is 0 (RFC 9298 5). `None` for another context —
/// none is registered (RFC 9298 4) — and for a datagram without one.
fn udp_payload(datagram: Bytes) -> Option<Bytes> {
    let Some((context, n)) = varint::get(&datagram) else {
        tracing::trace!("masque: a datagram without a context id is dropped");
        return None;
    };
    (context == 0).then(|| datagram.slice(n..))
}

impl PacketSocket for MasqueUdp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // checked first: nothing is opened for what never goes
            if buf.len() > MAX_PAYLOAD {
                return Err(too_long(MAX_PAYLOAD));
            }
            let mut reopened = false;
            loop {
                let stream = self.stream(to, &mut reopened).await?;
                match stream.send(buf) {
                    Ok(()) => return Ok(()),
                    Err(Unsent::TooLong(room)) => return Err(too_long(room)),
                    // it ended under the datagram
                    Err(Unsent::Ended) => {
                        leave(&self.streams, to, stream.id);
                        reopen(&mut reopened)?;
                    }
                }
            }
        })
    }

    /// A packet longer than `buf` is dropped: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(self.inbox.recv_from(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::masque::MasqueOutbound;
    use crate::masque::tests::{
        BOUND, H3_REQUEST_REJECTED, closed_by_the_client, outbound, round_trip, spec, target,
        tunnel, users,
    };
    use crate::test_log::Log;
    use crate::testing::{
        FakeH3Proxy, H3ProxyScript, RecordedH3Datagram, RecordedH3Request, RecordedUdpEnd,
    };
    use http::Method;
    use rurge_config::HostName;
    use rurge_config::spec::Tristate;
    use rurge_net::connector::{
        BoxedDatagram, BoxedPacketSocket, BoxedStream, Connector, DirectConnector, Route,
        SystemResolve,
    };
    use rurge_proto::http::basic_authorization;
    use rurge_proto::testing::{echo_server, udp_echo_server};
    use rurge_proto::{Outbound, UdpSupport};
    use std::collections::BTreeSet;
    use std::net::SocketAddr;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Barrier;
    use tokio::task::JoinSet;
    use tokio::time::timeout;

    /// `out`'s carrier, its targets' streams closing after `idle`.
    fn carrier(out: &MasqueOutbound, idle: Duration) -> MasqueUdp {
        MasqueUdp::new(
            out.session.clone(),
            out.udp_authority.clone(),
            out.authorization.clone(),
            ConnectOpts::default(),
            idle,
        )
    }

    async fn udp_carrier(out: &MasqueOutbound) -> BoxedPacketSocket {
        timeout(BOUND, out.open_udp(&ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a carrier")
    }

    /// The default template's path to `addr`.
    fn path(addr: SocketAddr) -> String {
        format!("/.well-known/masque/udp/{}/{}/", addr.ip(), addr.port())
    }

    /// A CONNECT-UDP to `path` without credentials, as the fake records it.
    fn udp_request(connection: usize, authority: &str, path: &str) -> RecordedH3Request {
        RecordedH3Request {
            connection,
            method: "CONNECT".to_string(),
            protocol: Some("connect-udp".to_string()),
            scheme: Some("https".to_string()),
            authority: Some(authority.to_string()),
            path: Some(path.to_string()),
            headers: vec![("capsule-protocol".to_string(), "?1".to_string())],
        }
    }

    async fn answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = timeout(BOUND, carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_round_trip(carrier: &dyn PacketSocket, to: &Target, payload: &[u8]) {
        timeout(BOUND, carrier.send_to(payload, to))
            .await
            .expect("sent within the bound")
            .unwrap();
        assert_eq!(answer(carrier).await, (payload.to_vec(), to.clone()));
    }

    /// Nothing comes for a while: a fixed wait, only to see nothing happen.
    async fn no_answer(carrier: &dyn PacketSocket) {
        let mut buf = vec![0u8; 65536];
        let quiet = timeout(Duration::from_millis(300), carrier.recv_from(&mut buf)).await;
        assert!(quiet.is_err(), "{quiet:?}");
    }

    /// How a datagram to `to` fails.
    async fn failure(carrier: &dyn PacketSocket, to: &Target) -> io::Error {
        timeout(BOUND, carrier.send_to(b"x", to))
            .await
            .expect("an outcome within the bound")
            .expect_err("refused")
    }

    /// Sends `payload` to the echo at `to` until it comes back: what goes
    /// out on a connection as it ends is lost.
    async fn until_answered(carrier: &dyn PacketSocket, to: &Target, payload: &[u8]) {
        let deadline = Instant::now() + BOUND;
        let mut buf = vec![0u8; 65536];
        loop {
            assert!(Instant::now() < deadline, "no answer within the bound");
            timeout(BOUND, carrier.send_to(payload, to))
                .await
                .expect("sent within the bound")
                .unwrap();
            let wait = Instant::now() + Duration::from_millis(500);
            while let Ok(got) = tokio::time::timeout_at(wait, carrier.recv_from(&mut buf)).await {
                let (n, _) = got.unwrap();
                if buf[..n] == *payload {
                    return;
                }
            }
        }
    }

    /// Polls `check` until it holds, within the bound.
    async fn eventually(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !check() {
            assert!(Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// RFC 9298 3.4: an extended CONNECT to the server's own authority,
    /// the target in the default template's path — an IPv6 literal with
    /// `%3A`, a name as its A-labels —, the credentials, marked sensitive,
    /// then `capsule-protocol: ?1`. A name that cannot be sent is refused
    /// as a datagram that cannot go.
    #[test]
    fn a_connect_udp_request_names_its_target_in_the_path() {
        let mut authorization = HeaderValue::from_static("Basic dTpw");
        authorization.set_sensitive(true);
        for (host, path) in [
            (
                HostName::parse("192.0.2.6"),
                "/.well-known/masque/udp/192.0.2.6/53/",
            ),
            (
                HostName::parse("2001:db8::42"),
                "/.well-known/masque/udp/2001%3Adb8%3A%3A42/53/",
            ),
            (
                HostName::Domain("bücher.example".into()),
                "/.well-known/masque/udp/xn--bcher-kva.example/53/",
            ),
        ] {
            let asked = request(
                "proxy.test:443",
                &Target::new(host, 53),
                Some(&authorization),
            )
            .unwrap();
            assert_eq!(asked.method(), Method::CONNECT);
            assert_eq!(
                asked.extensions().get::<h3::Protocol>(),
                Some(&h3::Protocol::CONNECT_UDP)
            );
            assert_eq!(
                asked.uri().to_string(),
                format!("https://proxy.test:443{path}")
            );
            let fields: Vec<(&str, &str)> = asked
                .headers()
                .iter()
                .map(|(name, value)| (name.as_str(), value.to_str().unwrap()))
                .collect();
            assert_eq!(
                fields,
                [
                    ("proxy-authorization", "Basic dTpw"),
                    ("capsule-protocol", "?1")
                ]
            );
            // QPACK never indexes it (RFC 9204 7.1.3)
            assert!(asked.headers()[PROXY_AUTHORIZATION].is_sensitive());
            assert!(!format!("{asked:?}").contains("dTpw"));
        }
        let to = Target::new(HostName::parse("192.0.2.6"), 53);
        let asked = request("[2001:db8::1]:8443", &to, None).unwrap();
        assert_eq!(
            asked.uri().authority().map(|a| a.as_str()),
            Some("[2001:db8::1]:8443")
        );
        assert_eq!(asked.headers().len(), 1);
        // longer than a URI may be
        let long = format!("{}.test", "a".repeat(70_000));
        for name in ["x@blocked.test", "a b.test", "", long.as_str()] {
            let to = Target::new(HostName::Domain(name.into()), 53);
            let err = request("proxy.test:443", &to, None).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                "masque: the host name cannot be sent to the server"
            );
        }
    }

    /// RFC 9298 4, 5: a packet is what follows context id 0, written in
    /// any length; another context, or none, carries no packet.
    #[test]
    fn a_packet_follows_context_id_0() {
        let payload = |bytes: &[u8]| udp_payload(Bytes::copy_from_slice(bytes));
        assert_eq!(payload(&[0, b'h', b'i']).as_deref(), Some(&b"hi"[..]));
        assert_eq!(payload(&[0]).as_deref(), Some(&b""[..]));
        // 0 in two bytes: not minimal, still 0
        assert_eq!(payload(&[0x40, 0, b'h', b'i']).as_deref(), Some(&b"hi"[..]));
        for other in [&[2, b'h', b'i'][..], &[0x40, 2, b'x'], &[], &[0x40]] {
            assert_eq!(payload(other), None, "{other:02x?}");
        }
    }

    /// RFC 9297 3.2: a 2xx that starts the Capsule Protocol has no content
    /// and is none of 204, 205 and 206.
    #[test]
    fn a_2xx_with_content_or_one_of_204_205_206_breaks_the_capsule_protocol() {
        let response = |status: u16, fields: &[(&str, &str)]| {
            let mut response = Response::builder().status(status);
            for &(name, value) in fields {
                response = response.header(name, value);
            }
            response.body(()).unwrap()
        };
        for fine in [
            response(200, &[]),
            response(200, &[("capsule-protocol", "?1")]),
            response(201, &[("proxy-status", "masque")]),
        ] {
            assert!(!breaks_capsule_protocol(&fine), "{fine:?}");
        }
        for broken in [
            response(200, &[("content-length", "0")]),
            response(200, &[("content-type", "application/octet-stream")]),
            response(200, &[("transfer-encoding", "chunked")]),
            response(204, &[]),
            response(205, &[]),
            response(206, &[("capsule-protocol", "?1")]),
        ] {
            assert!(breaks_capsule_protocol(&broken), "{broken:?}");
        }
    }

    /// Design 7.1: a target's first datagram opens its CONNECT-UDP stream
    /// — nothing before —, each target a stream of its own on the one
    /// connection, which TCP shares. Every packet goes in one HTTP/3
    /// datagram with context id 0, and each answer is its stream's
    /// target's.
    #[tokio::test]
    async fn udp_goes_in_http3_datagrams_a_stream_for_each_target() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = udp_carrier(&out).await;
        assert_eq!(fake.connections(), 0, "nothing is opened before a datagram");
        udp_round_trip(carrier.as_ref(), &target(one), b"to one").await;
        udp_round_trip(carrier.as_ref(), &target(two), b"to two").await;
        udp_round_trip(carrier.as_ref(), &target(one), b"one again").await;
        udp_round_trip(carrier.as_ref(), &target(one), b"").await;
        let authority = fake.addr().to_string();
        assert_eq!(
            fake.requests(),
            [
                udp_request(0, &authority, &path(one)),
                udp_request(0, &authority, &path(two))
            ]
        );
        let datagram = |stream, len| RecordedH3Datagram {
            connection: 0,
            stream,
            context: 0,
            len,
        };
        assert_eq!(
            fake.datagrams(),
            [
                datagram(0, 6),
                datagram(4, 6),
                datagram(0, 9),
                datagram(0, 0)
            ]
        );
        let mut stream = tunnel(&out, &target(echo_server().await)).await;
        round_trip(&mut stream, b"tcp beside").await;
        assert_eq!(fake.connections(), 1);
    }

    /// RFC 9298 2, 3: a name goes in the path as its A-labels, an IPv6
    /// literal with `%3A` for its colons; the answers count as the target's
    /// asked for (symmetric). `:authority` names the server as its TLS
    /// does: by the policy's `sni`, here. A name that cannot be sent is
    /// refused, and nothing goes out.
    #[tokio::test]
    async fn targets_go_in_the_path_and_the_server_by_its_tls_name() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            connect_to: Some(echo),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "sni=localhost");
        let carrier = udp_carrier(&out).await;
        let authority = format!("localhost:{}", fake.addr().port());
        for (host, path) in [
            (
                HostName::Domain("bücher.example".into()),
                "/.well-known/masque/udp/xn--bcher-kva.example/53/",
            ),
            (
                HostName::parse("2001:db8::42"),
                "/.well-known/masque/udp/2001%3Adb8%3A%3A42/53/",
            ),
        ] {
            udp_round_trip(carrier.as_ref(), &Target::new(host, 53), b"q").await;
            assert_eq!(
                fake.requests().pop(),
                Some(udp_request(0, &authority, path))
            );
        }
        for name in ["x@blocked.test", "a b.test", ""] {
            let err = failure(
                carrier.as_ref(),
                &Target::new(HostName::Domain(name.into()), 53),
            )
            .await;
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                "masque: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.requests().len(), 2);
    }

    /// Every CONNECT-UDP carries the credentials, as every CONNECT does. A
    /// refusal fails the datagram — and its flow: no other datagram would
    /// fare better — with the texts of TCP: 407, then any other status by
    /// its number, masque-server's 502 for a target it cannot reach among
    /// them; the next datagram asks again.
    #[tokio::test]
    async fn every_connect_udp_carries_the_credentials_and_a_refusal_is_quoted() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: users(),
            ..H3ProxyScript::default()
        });
        let carrier = udp_carrier(&outbound(&fake, "username=u, password=p")).await;
        udp_round_trip(carrier.as_ref(), &target(echo), b"let in").await;
        // base64("u:p"), then the capsule protocol
        assert_eq!(
            fake.requests()[0].headers,
            [
                ("proxy-authorization".to_string(), "Basic dTpw".to_string()),
                ("capsule-protocol".to_string(), "?1".to_string())
            ]
        );
        let wrong = udp_carrier(&outbound(&fake, "username=u, password=wrong")).await;
        let none = udp_carrier(&outbound(&fake, "")).await;
        // a name, which the fake never resolves
        let name = Target::new(HostName::parse("remote.example"), 53);
        for (carrier, to, refusal) in [
            (
                &wrong,
                target(echo),
                "masque: proxy authentication required",
            ),
            (&none, target(echo), "masque: proxy authentication required"),
            (&carrier, name, "masque: the proxy answered 502"),
        ] {
            for _ in 0..2 {
                let err = failure(carrier.as_ref(), &to).await;
                assert_ne!(err.kind(), io::ErrorKind::InvalidInput, "{err}");
                assert_eq!(err.to_string(), refusal);
            }
        }
        assert_eq!(fake.requests().len(), 7, "each datagram asked again");
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            refuse: Some(403),
            ..H3ProxyScript::default()
        });
        let carrier = udp_carrier(&outbound(&fake, "")).await;
        let err = failure(carrier.as_ref(), &target(echo)).await;
        assert_eq!(err.to_string(), "masque: the proxy answered 403");
    }

    /// RFC 9297 3.2, RFC 9298 3.5: a 2xx with content, or a 204, 205 or
    /// 206, is no answer to CONNECT-UDP; the connection goes on.
    #[tokio::test]
    async fn a_2xx_that_breaks_the_capsule_protocol_is_refused() {
        let (udp, tcp) = (udp_echo_server().await, echo_server().await);
        for (status, fields) in [
            (200, &[("content-length", "0")][..]),
            (
                200,
                &[
                    ("capsule-protocol", "?1"),
                    ("content-type", "application/octet-stream"),
                ],
            ),
            (204, &[]),
            (205, &[]),
            (206, &[("capsule-protocol", "?1")]),
        ] {
            let fields = fields
                .iter()
                .map(|&(name, value)| (name.to_string(), value.to_string()))
                .collect();
            let fake = FakeH3Proxy::spawn(H3ProxyScript {
                udp_response: Some((status, fields)),
                ..H3ProxyScript::default()
            });
            let out = outbound(&fake, "");
            let carrier = udp_carrier(&out).await;
            let err = failure(carrier.as_ref(), &target(udp)).await;
            assert_eq!(
                err.to_string(),
                "masque: the proxy's response to CONNECT-UDP is malformed",
                "{status}"
            );
            let mut stream = tunnel(&out, &target(tcp)).await;
            round_trip(&mut stream, b"tcp still").await;
            assert_eq!(fake.connections(), 1);
        }
    }

    /// The manual: "The server must advertise support for extended CONNECT
    /// and HTTP Datagrams". Without extended CONNECT, with `H3_DATAGRAM =
    /// 0`, or without QUIC datagrams, a datagram fails, and no request goes
    /// out; the connection carries TCP all the same.
    #[tokio::test]
    async fn a_server_without_connect_udp_fails_the_datagrams_and_carries_tcp() {
        let (udp, tcp) = (udp_echo_server().await, echo_server().await);
        for script in [
            H3ProxyScript {
                no_extended_connect: true,
                ..H3ProxyScript::default()
            },
            H3ProxyScript {
                no_h3_datagram: true,
                ..H3ProxyScript::default()
            },
            H3ProxyScript {
                no_datagrams: true,
                ..H3ProxyScript::default()
            },
        ] {
            let fake = FakeH3Proxy::spawn(script.clone());
            let out = outbound(&fake, "");
            let carrier = udp_carrier(&out).await;
            for _ in 0..2 {
                let err = failure(carrier.as_ref(), &target(udp)).await;
                assert_ne!(err.kind(), io::ErrorKind::InvalidInput, "{script:?}");
                assert_eq!(
                    err.to_string(),
                    "masque: the server does not support CONNECT-UDP",
                    "{script:?}"
                );
            }
            assert!(fake.requests().is_empty(), "{script:?}");
            let mut stream = tunnel(&out, &target(tcp)).await;
            round_trip(&mut stream, b"tcp still").await;
            assert_eq!(fake.connections(), 1, "{script:?}");
        }
    }

    /// RFC 9298 5: at most 65527 bytes with context id 0, and no more than
    /// a datagram of the connection carries now — that much goes, in one
    /// datagram —: no DATAGRAM capsule takes what does not fit. Refused,
    /// nothing sent, the datagram alone: the carrier goes on.
    #[tokio::test]
    async fn a_datagram_too_long_is_refused_and_nothing_goes_out() {
        // the stream closes gracefully soon after, once all it carried
        // was read
        const IDLE: Duration = Duration::from_secs(2);
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier = carrier(&out, IDLE);
        let to = target(echo);
        let err = timeout(BOUND, carrier.send_to(&vec![0u8; MAX_PAYLOAD + 1], &to))
            .await
            .expect("an outcome within the bound")
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "masque: a datagram longer than 65527 bytes"
        );
        assert_eq!(fake.connections(), 0, "nothing is opened for it");
        udp_round_trip(&carrier, &to, b"first").await;
        // each taken and used with nothing awaited between: the path's size
        // of a datagram stays put
        let room = carrier.room(&to).expect("the target's stream");
        assert!((1000..MAX_PAYLOAD).contains(&room), "{room}");
        let err = timeout(BOUND, carrier.send_to(&vec![0u8; room + 1], &to))
            .await
            .expect("an outcome within the bound")
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            format!("masque: a datagram longer than {room} bytes")
        );
        let room = carrier.room(&to).expect("the target's stream");
        timeout(BOUND, carrier.send_to(&vec![0u8; room], &to))
            .await
            .expect("sent within the bound")
            .unwrap();
        // the client ended the stream with a FIN: the fake read all of it
        eventually(|| !fake.udp_ends().is_empty()).await;
        let end = RecordedUdpEnd {
            connection: 0,
            stream: 0,
            fin: true,
        };
        assert_eq!(fake.udp_ends(), [end]);
        assert_eq!(fake.udp_data(), 0, "DATA on the stream");
        let sent: Vec<usize> = fake.datagrams().iter().map(|d| d.len).collect();
        assert_eq!(sent, [5, room]);
    }

    /// RFC 9297 3.2, 3.5: the proxy may send a packet in a DATAGRAM capsule
    /// on the stream instead — here across two DATA frames, after a capsule
    /// of a reserved type and a DATAGRAM capsule with context id 2, both
    /// passed over. Each is the target's packet, once.
    #[tokio::test]
    async fn datagram_capsules_on_the_stream_are_packets_and_the_rest_is_passed_over() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            udp_capsules: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        let carrier = udp_carrier(&out).await;
        for payload in [&b"first"[..], b"", &[7u8; 1000]] {
            udp_round_trip(carrier.as_ref(), &target(echo), payload).await;
        }
        no_answer(carrier.as_ref()).await;
    }

    /// RFC 9298 4: an HTTP/3 datagram with another context id — none is
    /// registered — or with none at all carries no packet; context id 0
    /// written long is 0 all the same.
    #[tokio::test]
    async fn datagrams_of_other_contexts_carry_no_packet() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier = udp_carrier(&out).await;
        let to = target(echo);
        udp_round_trip(carrier.as_ref(), &to, b"open").await;
        let stream = fake.datagrams()[0].stream;
        for datagram in [
            &[2, b'n', b'o'][..],
            &[],
            &[0x40, 2, b'n', b'o'],
            &[0x40, 0, b'y', b'e', b's'],
            &[0, b'a', b'n', b'd'],
        ] {
            fake.send_datagram(0, stream, datagram);
        }
        assert_eq!(
            answer(carrier.as_ref()).await,
            (b"yes".to_vec(), to.clone())
        );
        assert_eq!(answer(carrier.as_ref()).await, (b"and".to_vec(), to));
        no_answer(carrier.as_ref()).await;
    }

    /// RFC 9297 3.3: a capsule that is malformed — a DATAGRAM capsule with
    /// no room for a context id — ends the stream as it stands, with no
    /// FIN; the target's next datagram opens another.
    #[tokio::test]
    async fn a_malformed_capsule_ends_the_stream() {
        let (log, _logging, _second) = Log::capture();
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            udp_bad_capsule: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        let carrier = carrier(&out, IDLE);
        let to = target(echo);
        timeout(BOUND, carrier.send_to(b"first", &to))
            .await
            .expect("sent within the bound")
            .unwrap();
        eventually(|| carrier.tracked() == 0).await;
        eventually(|| !fake.udp_ends().is_empty()).await;
        let end = RecordedUdpEnd {
            connection: 0,
            stream: 0,
            fin: false,
        };
        assert_eq!(fake.udp_ends(), [end]);
        assert!(
            log.text().contains(
                "masque: a UDP stream ended: read error: masque: a malformed capsule (no context id)"
            ),
            "{}",
            log.text()
        );
        timeout(BOUND, carrier.send_to(b"second", &to))
            .await
            .expect("sent within the bound")
            .unwrap();
        assert_eq!(fake.requests().len(), 2);
    }

    /// A datagram that meets its stream ended opens one anew, once; met
    /// ended too, the datagram fails.
    #[test]
    fn a_datagram_opens_its_stream_anew_once() {
        let mut reopened = false;
        reopen(&mut reopened).unwrap();
        assert!(reopened);
        let err = reopen(&mut reopened).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(err.to_string(), "masque: the server ended the UDP stream");
    }

    /// A proxy that answers 200 and ends every stream at once, eight
    /// datagrams sent together, round after round. A datagram that meets
    /// its stream ended — the one opened for it, or for those it waited
    /// with — opens one anew, once (`reopen`): a round asks for two streams
    /// at most, and a datagram that fails says why. Which datagrams meet a
    /// stream ended — its reader saw the server's end before they looked —
    /// is the scheduler's.
    #[tokio::test]
    async fn a_stream_the_server_ends_at_once_is_opened_anew_once_per_datagram() {
        const SENDERS: usize = 8;
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            udp_end_at_once: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        let carrier = Arc::new(carrier(&out, IDLE));
        let to = target(echo);
        for round in 0..5 {
            let before = fake.requests().len();
            let mut sending = JoinSet::new();
            for _ in 0..SENDERS {
                let (carrier, to) = (carrier.clone(), to.clone());
                sending.spawn(async move {
                    timeout(BOUND, carrier.send_to(b"x", &to))
                        .await
                        .expect("an outcome within the bound")
                });
            }
            while let Some(sent) = sending.join_next().await {
                if let Err(e) = sent.unwrap() {
                    assert_eq!(e.kind(), io::ErrorKind::ConnectionAborted);
                    assert_eq!(e.to_string(), "masque: the server ended the UDP stream");
                }
            }
            // a stream, and one opened anew for the datagrams that met it
            // ended
            let asked = fake.requests().len() - before;
            assert!((1..=2).contains(&asked), "round {round}: {asked}");
            eventually(|| carrier.tracked() == 0).await;
        }
    }

    /// Design 7.1: a stream without a datagram either way for the idle time
    /// closes gracefully — a FIN, and nothing more is read — and leaves the
    /// carrier; the target's next datagram opens another, on the same
    /// connection. Answers alone keep a stream open, as datagrams sent do.
    #[tokio::test]
    async fn an_idle_stream_closes_gracefully_and_the_next_datagram_opens_another() {
        const IDLE: Duration = Duration::from_secs(1);
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier = carrier(&out, IDLE);
        let to = target(echo);
        udp_round_trip(&carrier, &to, b"first").await;
        for _ in 0..8 {
            tokio::time::sleep(IDLE / 5).await;
            fake.send_datagram(0, 0, &[0, b'k']);
            assert_eq!(answer(&carrier).await, (b"k".to_vec(), to.clone()));
        }
        assert_eq!(carrier.tracked(), 1, "kept open by its answers");
        assert!(fake.udp_ends().is_empty());
        eventually(|| carrier.tracked() == 0).await;
        eventually(|| !fake.udp_ends().is_empty()).await;
        let end = RecordedUdpEnd {
            connection: 0,
            stream: 0,
            fin: true,
        };
        assert_eq!(fake.udp_ends(), [end]);
        udp_round_trip(&carrier, &to, b"second").await;
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 0]);
    }

    /// Design 3.5: a server going away (an HTTP/3 GOAWAY) takes no new
    /// stream: the target's next datagram opens its stream anew on a new
    /// connection; the one before is let go of, and its connection closed
    /// once nothing holds it.
    #[tokio::test]
    async fn after_a_goaway_the_next_datagram_goes_on_a_new_connection() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            goaway_after: Some(1),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        // held from before the first request, to see its GOAWAY come
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), &target(echo), b"before").await;
        let deadline = Instant::now() + BOUND;
        while out.session.serves(&lease) {
            assert!(Instant::now() < deadline, "no GOAWAY heard");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(lease);
        udp_round_trip(carrier.as_ref(), &target(echo), b"after").await;
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 1]);
        closed_by_the_client(&fake.connection(0)).await;
    }

    /// A connection the server closed, or lost in a restart (a stateless
    /// reset answers the next datagram): the target's datagrams go on in a
    /// stream opened anew on a new connection.
    #[tokio::test]
    async fn after_the_connection_ends_the_next_datagrams_go_on_a_new_one() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier = udp_carrier(&out).await;
        let to = target(echo);
        udp_round_trip(carrier.as_ref(), &to, b"first").await;
        fake.close_connections();
        until_answered(carrier.as_ref(), &to, b"after a close").await;
        fake.restart();
        until_answered(carrier.as_ref(), &to, b"after a restart").await;
        let carried: BTreeSet<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, BTreeSet::from([0, 1, 2]));
        assert_eq!(fake.connections(), 3);
    }

    /// RFC 9114 4.1.1: a CONNECT-UDP the server rejects (H3_REQUEST_REJECTED),
    /// with no GOAWAY, was not processed: it goes again, once, on a new
    /// connection, and the datagram with it. Another target's stream goes
    /// on where it is.
    #[tokio::test]
    async fn a_connect_udp_the_server_rejects_goes_again_on_a_new_connection() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), &target(one), b"one").await;
        let held = fake.hold_next_request();
        let reject = async {
            let mut stream = timeout(BOUND, held)
                .await
                .expect("the request within the bound")
                .expect("held");
            stream.reset(H3_REQUEST_REJECTED);
            stream
        };
        let to = target(two);
        let (sent, _rejected) = tokio::join!(timeout(BOUND, carrier.send_to(b"two", &to)), reject);
        sent.expect("sent within the bound").unwrap();
        assert_eq!(answer(carrier.as_ref()).await, (b"two".to_vec(), to));
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 0, 1]);
        assert_eq!(fake.connections(), 2);
        udp_round_trip(carrier.as_ref(), &target(one), b"one still").await;
    }

    /// Dials directly, leads where it is told to, and counts the times it
    /// is asked where it leads.
    struct Counted {
        inner: DirectConnector,
        route: Mutex<Arc<u8>>,
        asked: AtomicUsize,
    }

    impl Counted {
        fn new() -> Counted {
            Counted {
                inner: DirectConnector::new(Arc::new(SystemResolve)),
                route: Mutex::new(Arc::new(1)),
                asked: AtomicUsize::new(0),
            }
        }

        fn asked(&self) -> usize {
            self.asked.load(Ordering::Relaxed)
        }
    }

    impl Connector for Counted {
        fn connect<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            self.inner.connect(target, opts)
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            self.inner.connect_udp(target, opts)
        }

        fn route(&self) -> Option<Route> {
            self.asked.fetch_add(1, Ordering::Relaxed);
            Some(Route::of(&self.route.lock().unwrap()))
        }
    }

    /// The outbound to `fake`, its connections dialled through `counted`.
    fn outbound_via(fake: &FakeH3Proxy, counted: Arc<Counted>) -> MasqueOutbound {
        let port = fake.addr().port();
        MasqueOutbound::new(
            "M",
            Target::new(HostName::parse("127.0.0.1"), port),
            &spec(port, ""),
            Tristate::Auto,
            &[],
            fake.roots(),
            counted,
        )
        .unwrap()
    }

    /// Where the policy leads can cost a group's resolution: a carrier asks
    /// at most once a second, however many datagrams it sends meanwhile.
    #[tokio::test]
    async fn the_route_is_compared_at_most_once_a_second() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let counted = Arc::new(Counted::new());
        let out = outbound_via(&fake, counted.clone());
        let carrier = udp_carrier(&out).await;
        let to = target(echo);
        udp_round_trip(carrier.as_ref(), &to, b"open").await;
        let before = counted.asked();
        for i in 0..50u8 {
            timeout(BOUND, carrier.send_to(&[i], &to))
                .await
                .expect("sent within the bound")
                .unwrap();
        }
        let asked = counted.asked() - before;
        assert!(asked <= 3, "asked {asked} times for 50 datagrams");
    }

    /// A carrier finds out for itself, at its next comparison of the
    /// route, that the policy leads elsewhere now: the target's next
    /// datagram opens its stream anew on a new connection, dialled the new
    /// way. Until then — a second at most — datagrams go on the stream
    /// before.
    #[tokio::test]
    async fn a_carrier_finds_a_changed_route_by_itself() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let counted = Arc::new(Counted::new());
        let out = outbound_via(&fake, counted.clone());
        let carrier = udp_carrier(&out).await;
        let to = target(echo);
        udp_round_trip(carrier.as_ref(), &to, b"first").await;
        *counted.route.lock().unwrap() = Arc::new(2);
        let deadline = Instant::now() + BOUND;
        while !fake.requests().iter().any(|r| r.connection == 1) {
            assert!(
                Instant::now() < deadline,
                "the stream never left its connection"
            );
            timeout(BOUND, carrier.send_to(b"probe", &to))
                .await
                .expect("sent within the bound")
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        until_answered(carrier.as_ref(), &to, b"moved").await;
        assert_eq!(fake.connections(), 2);
    }

    /// A carrier that does not read loses what its inbox cannot hold, and
    /// holds up no other carrier: a stream's reader waits for nobody.
    #[tokio::test]
    async fn a_full_inbox_drops_rather_than_holds_up_the_other_carriers() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let (slow, other) = (udp_carrier(&out).await, udp_carrier(&out).await);
        udp_round_trip(slow.as_ref(), &target(echo), b"slow").await;
        udp_round_trip(other.as_ref(), &target(echo), b"other").await;
        let streams: Vec<u64> = fake.datagrams().iter().map(|d| d.stream).collect();
        assert_eq!(streams, [0, 4]);
        for i in 0..INBOX as u32 + 10 {
            let mut datagram = vec![0];
            datagram.extend_from_slice(&i.to_be_bytes());
            fake.send_datagram(0, 0, &datagram);
        }
        fake.send_datagram(0, 4, b"\0not held up");
        assert_eq!(answer(other.as_ref()).await.0, b"not held up");
        for i in 0..INBOX as u32 {
            assert_eq!(answer(slow.as_ref()).await.0, i.to_be_bytes());
        }
        no_answer(slow.as_ref()).await;
    }

    /// Targets at once, from tasks on threads of their own, two senders to
    /// each: one stream for each target, opened once for both, and each
    /// answer its target's.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn many_targets_at_once_get_a_stream_each() {
        const TARGETS: u8 = 8;
        let mut targets = Vec::new();
        for _ in 0..TARGETS {
            targets.push(target(udp_echo_server().await));
        }
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier: Arc<dyn PacketSocket> = Arc::from(udp_carrier(&out).await);
        for round in 0..3u8 {
            let start = Arc::new(Barrier::new(2 * usize::from(TARGETS)));
            let mut sending = JoinSet::new();
            for (i, to) in (0..TARGETS).zip(&targets) {
                for sender in 0..2u8 {
                    let (carrier, start, to) = (carrier.clone(), start.clone(), to.clone());
                    sending.spawn(async move {
                        start.wait().await;
                        timeout(BOUND, carrier.send_to(&[round, i, sender], &to))
                            .await
                            .expect("sent within the bound")
                            .unwrap();
                    });
                }
            }
            while let Some(sent) = sending.join_next().await {
                sent.unwrap();
            }
            let mut got = Vec::new();
            for _ in 0..2 * TARGETS {
                let (packet, from) = answer(carrier.as_ref()).await;
                assert_eq!(packet.len(), 3);
                assert_eq!(from, targets[usize::from(packet[1])], "round {round}");
                got.push(packet);
            }
            got.sort();
            let expected: Vec<Vec<u8>> = (0..TARGETS)
                .flat_map(|i| (0..2u8).map(move |sender| vec![round, i, sender]))
                .collect();
            assert_eq!(got, expected, "round {round}");
        }
        assert_eq!(fake.requests().len(), usize::from(TARGETS));
        assert_eq!(fake.connections(), 1);
    }

    /// `opts.timeout` bounds opening a stream: here a proxy that takes the
    /// CONNECT-UDP, on a connection that is up, and never answers it.
    #[tokio::test]
    async fn a_connect_udp_the_proxy_never_answers_times_out() {
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            silent: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        drop(
            timeout(BOUND, out.session.lease(&ConnectOpts::default()))
                .await
                .unwrap()
                .unwrap(),
        );
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        let carrier = timeout(BOUND, out.open_udp(&opts)).await.unwrap().unwrap();
        let started = Instant::now();
        let err = failure(carrier.as_ref(), &target(echo)).await;
        // the stream's own time ran out, half a second, the only one that
        // says so: the connection is up, and keeps up
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(err.to_string(), "connect timed out");
        let took = started.elapsed();
        assert!(took < Duration::from_secs(5), "{took:?}");
        assert_eq!(fake.requests().len(), 1, "the request went out");
    }

    /// A carrier gone takes its streams with it — cancelled, no FIN — and
    /// its tasks; the connection stays the session's, and carries TCP.
    #[tokio::test]
    async fn a_carrier_gone_ends_its_streams() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), &target(one), b"one").await;
        udp_round_trip(carrier.as_ref(), &target(two), b"two").await;
        drop(carrier);
        eventually(|| fake.udp_ends().len() == 2).await;
        let mut ends = fake.udp_ends();
        ends.sort_by_key(|end| end.stream);
        let end = |stream| RecordedUdpEnd {
            connection: 0,
            stream,
            fin: false,
        };
        assert_eq!(ends, [end(0), end(4)]);
        let mut stream = tunnel(&out, &target(echo_server().await)).await;
        round_trip(&mut stream, b"tcp still").await;
        assert_eq!(fake.connections(), 1);
    }

    /// The credentials go to the server, the payloads to their targets,
    /// and neither shows anywhere else — in no error and in no log line,
    /// not even at the finest level — whether a datagram goes, comes back
    /// in a capsule, or is refused.
    #[tokio::test]
    async fn payloads_and_credentials_never_show() {
        const PASSWORD: &str = "s3cret-masque-udp";
        const MARK: &[u8] = b"payload-4d2f81";
        let (log, _logging, _second) = Log::capture();
        let echo = udp_echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: vec![("u".to_string(), PASSWORD.to_string())],
            udp_capsules: true,
            ..H3ProxyScript::default()
        });
        let wrong = outbound(&fake, &format!("username=u, password={PASSWORD}-not"));
        let refused = failure(udp_carrier(&wrong).await.as_ref(), &target(echo)).await;
        assert_eq!(refused.to_string(), "masque: proxy authentication required");
        let out = outbound(&fake, &format!("username=u, password={PASSWORD}"));
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), &target(echo), MARK).await;
        let too_long = timeout(BOUND, carrier.send_to(&MARK.repeat(5000), &target(echo)))
            .await
            .expect("an outcome within the bound")
            .unwrap_err();
        let basic = basic_authorization("u", PASSWORD);
        assert_eq!(
            fake.requests()[1].header("proxy-authorization"),
            Some(basic.as_str())
        );
        let shown = format!("{refused} {refused:?} {too_long} {too_long:?}");
        let log = log.text();
        assert!(!log.is_empty());
        for secret in [PASSWORD, &basic["Basic ".len()..], "payload-4d2f81"] {
            assert!(!shown.contains(secret), "{shown}");
            assert!(!log.contains(secret));
        }
    }
}
