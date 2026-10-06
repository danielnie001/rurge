//! The client's side of an HTTP/3 connection (phase 2 M7 design 4): our
//! control stream, the server's unidirectional streams, and requests.

use super::datagram::{self, Flows};
use super::frame::{self, FrameReader, Header, ReadFailure, Settings};
use super::stream::{Kind, RequestStream};
use super::{
    CONTROL_STREAM, H3_CLOSED_CRITICAL_STREAM, H3_EXCESSIVE_LOAD, H3_FRAME_ERROR,
    H3_FRAME_UNEXPECTED, H3_ID_ERROR, H3_MISSING_SETTINGS, H3_STREAM_CREATION_ERROR, H3Error,
    MAX_FIELD_SECTION_SIZE, PUSH_STREAM, QPACK_DECODER_STREAM, QPACK_ENCODER_STREAM,
    SETTINGS_ENABLE_CONNECT_PROTOCOL, SETTINGS_H3_DATAGRAM, SETTINGS_MAX_FIELD_SECTION_SIZE,
    SETTINGS_QPACK_BLOCKED_STREAMS, SETTINGS_QPACK_MAX_TABLE_CAPACITY, ServerSaid, code_name,
    qpack,
};
use crate::quic::connection_error;
use bytes::Bytes;
use http::{Method, Request};
use quinn::{Connection, ConnectionError, ReadError, RecvStream, WriteError};
use std::convert::Infallible;
use std::future::poll_fn;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

/// Our SETTINGS: no dynamic table for the server's QPACK encoder and no
/// stream blocked on one, field sections of at most
/// `MAX_FIELD_SECTION_SIZE`. A client made `with_datagrams` adds
/// `H3_DATAGRAM = 1`.
const OUR_SETTINGS: [(u64, u64); 3] = [
    (SETTINGS_QPACK_MAX_TABLE_CAPACITY, 0),
    (SETTINGS_MAX_FIELD_SECTION_SIZE, MAX_FIELD_SECTION_SIZE),
    (SETTINGS_QPACK_BLOCKED_STREAMS, 0),
];

/// The most a frame of the server's control stream that is read whole
/// (SETTINGS, GOAWAY) may take.
const MAX_CONTROL_FRAME: u64 = 16 << 10;

/// The protocol of an extended CONNECT (RFC 9220, RFC 8441 4), carried as
/// a request extension: a CONNECT with one says `:protocol`, `:scheme` and
/// `:path` too, and goes only once the server's SETTINGS allowed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Protocol(&'static str);

impl Protocol {
    /// Proxying UDP in HTTP (RFC 9298).
    pub const CONNECT_UDP: Protocol = Protocol("connect-udp");
}

/// HTTP/3 on one QUIC connection, as its client. A background task reads
/// the server's unidirectional streams — and, with datagrams, the
/// connection's QUIC datagrams — and holds our control stream open; it
/// ends with the client. No `Debug`: nothing in it is worth printing.
pub struct Client {
    inner: Arc<Inner>,
    driver: JoinHandle<()>,
}

/// What the client, its task and its requests share.
pub(super) struct Inner {
    pub(super) quic: Connection,
    state: watch::Sender<State>,
    /// The critical streams the server opened, a bit each (control, QPACK
    /// encoder, QPACK decoder): there is one of each at most.
    critical: AtomicU8,
    /// The request streams' HTTP/3 datagrams, on a client made
    /// `with_datagrams`.
    pub(super) flows: Option<Flows>,
}

#[derive(Default)]
struct State {
    /// The server's SETTINGS, once they came.
    settings: Option<Settings>,
    /// The lowest stream ID the server's latest GOAWAY leaves unprocessed.
    goaway: Option<u64>,
    /// What this client closed the connection for.
    failure: Option<H3Error>,
    /// How the connection ended, once the task saw it.
    ended: Option<ConnectionError>,
}

impl Client {
    /// HTTP/3 on `connection`, whose handshake chose ALPN `h3`: our control
    /// stream goes out with our SETTINGS, and the server's streams are read
    /// until the connection ends or the client is dropped.
    ///
    /// The client must live as long as the connection serves anything —
    /// HTTP/3 or a protocol's own streams: dropping it ends our control
    /// stream, which the server may take for the end of the connection
    /// (H3_CLOSED_CRITICAL_STREAM, RFC 9114 6.2.1).
    ///
    /// The client neither announces nor reads HTTP Datagrams: the
    /// connection's QUIC datagrams are left to the protocol (Hysteria 2's
    /// UDP, which a server taking them for HTTP/3's would break).
    pub fn new(connection: Connection) -> Client {
        Client::start(connection, None)
    }

    /// HTTP/3 on `connection` with HTTP Datagrams (RFC 9297), as `new`
    /// otherwise: our SETTINGS also say `H3_DATAGRAM = 1`, and the client
    /// reads the connection's QUIC datagrams — all of them: nothing else
    /// may — each for the CONNECT request stream it names
    /// (`RequestStream::datagrams`).
    pub fn with_datagrams(connection: Connection) -> Client {
        Client::start(connection, Some(Flows::default()))
    }

    fn start(connection: Connection, flows: Option<Flows>) -> Client {
        let inner = Arc::new(Inner {
            quic: connection,
            state: watch::Sender::new(State::default()),
            critical: AtomicU8::new(0),
            flows,
        });
        let driver = tokio::spawn(drive(inner.clone()));
        Client { inner, driver }
    }

    /// The server's SETTINGS, once they came: extended CONNECT and HTTP
    /// Datagrams need them first. The caller bounds the wait; an error when
    /// the connection ended before.
    pub async fn settings(&self) -> io::Result<Settings> {
        self.inner.settings().await
    }

    /// Whether the server sent GOAWAY: no new request goes on this
    /// connection; those under way run to their end (RFC 9114 5.2).
    pub fn is_going_away(&self) -> bool {
        self.inner.state.borrow().goaway.is_some()
    }

    /// Whether the request on stream `id` (`RequestStream::id`) is one the
    /// server's latest GOAWAY leaves unprocessed — its stream at or past
    /// the ID the GOAWAY names —: it may go again on another connection
    /// (RFC 9114 5.2). Before a GOAWAY, none is.
    pub fn unprocessed(&self, id: u64) -> bool {
        self.inner
            .state
            .borrow()
            .goaway
            .is_some_and(|limit| id >= limit)
    }

    /// Sends `request`'s head on a new bidirectional stream, as one
    /// HEADERS frame: its pseudo-header fields — `:method` and `:authority`
    /// alone for a CONNECT (RFC 9114 4.4); `:protocol`, `:scheme` and
    /// `:path` too for an extended CONNECT, one carrying a `Protocol` (RFC
    /// 8441 4); else `:scheme`, `:authority` and `:path` — then its header
    /// fields, a sensitive value never to be indexed. `:scheme` is `https`
    /// and `:path` `/` unless the URI says; the authority goes without
    /// userinfo (RFC 9114 4.3.1). The request's content, if any, is written
    /// to the stream and ended by its shutdown; the response comes back on
    /// it.
    ///
    /// An extended CONNECT first waits for the server's SETTINGS (the
    /// caller bounds the wait) and is refused, nothing sent, unless they
    /// enable it (RFC 8441 3, RFC 9220 3). Refused once the server sent
    /// GOAWAY (`going_away`).
    pub async fn request(&self, request: Request<()>) -> io::Result<RequestStream> {
        let head = frame::frame(frame::HEADERS, &head(&request)?);
        if request.extensions().get::<Protocol>().is_some() {
            let settings = self.inner.settings().await?;
            if settings.get(SETTINGS_ENABLE_CONNECT_PROTOCOL) != Some(1) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "h3: the server does not support extended CONNECT",
                ));
            }
        }
        {
            let state = self.inner.state.borrow();
            if let Some(failure) = &state.failure {
                return Err(failure.io());
            }
            if state.goaway.is_some() {
                return Err(ServerSaid::GoingAway.io());
            }
        }
        let (send, recv) = self
            .inner
            .quic
            .open_bi()
            .await
            .map_err(|e| self.inner.lost(&e))?;
        // from here on, a request given up on is cancelled by the stream
        let kind = Kind::of(request.method());
        let mut stream =
            RequestStream::new(self.inner.clone(), send, recv, Bytes::from(head), kind);
        stream.flush().await?;
        Ok(stream)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

/// The field section of `request`'s head, as `Client::request` says.
fn head(request: &Request<()>) -> io::Result<Vec<u8>> {
    let uri = request.uri();
    let Some(authority) = uri.authority() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "h3: a request needs an authority",
        ));
    };
    // what follows the last `@`, as `Authority::host` finds the host
    let authority = match authority.as_str().rsplit_once('@') {
        Some((_userinfo, host)) => host,
        None => authority.as_str(),
    }
    .as_bytes();
    let scheme = uri.scheme_str().unwrap_or("https").as_bytes();
    let path = uri
        .path_and_query()
        .map_or("/", |path| path.as_str())
        .as_bytes();
    let protocol = request.extensions().get::<Protocol>();
    let mut fields = vec![(&b":method"[..], request.method().as_str().as_bytes(), false)];
    match (request.method() == Method::CONNECT, protocol) {
        (true, None) => fields.push((&b":authority"[..], authority, false)),
        (true, Some(protocol)) => fields.extend([
            (&b":protocol"[..], protocol.0.as_bytes(), false),
            (&b":scheme"[..], scheme, false),
            (&b":authority"[..], authority, false),
            (&b":path"[..], path, false),
        ]),
        (false, None) => fields.extend([
            (&b":scheme"[..], scheme, false),
            (&b":authority"[..], authority, false),
            (&b":path"[..], path, false),
        ]),
        (false, Some(_)) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "h3: a request with a protocol must be a CONNECT",
            ));
        }
    }
    fields.extend(request.headers().iter().map(|(name, value)| {
        (
            name.as_str().as_bytes(),
            value.as_bytes(),
            value.is_sensitive(),
        )
    }));
    Ok(qpack::encode(fields))
}

impl Inner {
    /// The server's SETTINGS, once they came; an error when the connection
    /// ended before. The caller bounds the wait.
    pub(super) async fn settings(&self) -> io::Result<Settings> {
        let mut state = self.state.subscribe();
        let state = state
            .wait_for(|s| s.settings.is_some() || s.failure.is_some() || s.ended.is_some())
            .await
            .expect("`self` holds the sender");
        match (&state.settings, &state.failure, &state.ended) {
            (Some(settings), _, _) => Ok(settings.clone()),
            (None, Some(failure), _) => Err(failure.io()),
            (None, None, Some(ended)) => Err(connection_error(ended)),
            (None, None, None) => unreachable!("waited for one of them"),
        }
    }

    /// Closes the connection for `error` (RFC 9114 8): its requests fail
    /// with what it says. Once only, and not once the connection ended —
    /// even before the task saw it end: quinn's `close` would put a local
    /// close in place of how it ended (the server's close, a stateless
    /// reset).
    pub(super) fn fail(&self, error: H3Error) {
        let first = self.state.send_if_modified(|state| {
            if state.failure.is_some()
                || state.ended.is_some()
                || self.quic.close_reason().is_some()
            {
                return false;
            }
            state.failure = Some(error.clone());
            true
        });
        if first {
            tracing::debug!(
                code = %code_name(error.code),
                "h3: {}; closing the connection",
                error.text
            );
            self.quic.close(error.code, error.text.as_bytes());
        }
    }

    /// The connection's end as an I/O error: what this client closed it
    /// for, when it did.
    pub(super) fn lost(&self, e: &ConnectionError) -> io::Error {
        match (&self.state.borrow().failure, e) {
            (Some(failure), ConnectionError::LocallyClosed) => failure.io(),
            _ => connection_error(e),
        }
    }

    /// A failed read of a request stream as an I/O error: the server's
    /// reset with its code (`rejected`).
    pub(super) fn read_error(&self, e: ReadError) -> io::Error {
        match e {
            ReadError::Reset(code) => ServerSaid::Reset(code).io(),
            ReadError::ConnectionLost(e) => self.lost(&e),
            ReadError::ClosedStream
            | ReadError::IllegalOrderedRead
            | ReadError::ZeroRttRejected => closed_stream(),
        }
    }

    /// A failed write of a request stream as an I/O error: the server's
    /// stop with its code (`rejected`).
    pub(super) fn write_error(&self, e: WriteError) -> io::Error {
        match e {
            WriteError::Stopped(code) => ServerSaid::Stopped(code).io(),
            WriteError::ConnectionLost(e) => self.lost(&e),
            WriteError::ClosedStream | WriteError::ZeroRttRejected => closed_stream(),
        }
    }

    /// Why a stream's datagrams no longer flow, as an I/O error: the
    /// connection's end, when it ended; else the stream's.
    pub(super) fn flow_ended(&self) -> io::Error {
        match self.quic.close_reason() {
            Some(e) => self.lost(&e),
            None => closed_stream(),
        }
    }

    /// Notes that the server opened its critical stream of `kind` (`bit`);
    /// a second one is H3_STREAM_CREATION_ERROR (RFC 9114 6.2.1, RFC 9204
    /// 4.2).
    fn opened(&self, bit: u8, kind: &str) -> Result<(), H3Error> {
        if self.critical.fetch_or(bit, Ordering::Relaxed) & bit != 0 {
            return Err(H3Error::new(
                H3_STREAM_CREATION_ERROR,
                format!("the server opened a second {kind} stream"),
            ));
        }
        Ok(())
    }

    /// The server's GOAWAY for `id`: one of our request streams, and none
    /// later than a GOAWAY before (RFC 9114 5.2, 7.2.6).
    fn goaway(&self, id: u64) -> Result<(), H3Error> {
        if id % 4 != 0 {
            return Err(H3Error::new(
                H3_ID_ERROR,
                "the server's GOAWAY names no request stream",
            ));
        }
        let mut grew = false;
        self.state.send_if_modified(|state| match state.goaway {
            Some(before) if id > before => {
                grew = true;
                false
            }
            _ => {
                state.goaway = Some(id);
                true
            }
        });
        if grew {
            return Err(H3Error::new(
                H3_ID_ERROR,
                "the server's GOAWAY named a later stream than its last",
            ));
        }
        Ok(())
    }
}

fn closed_stream() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "h3: the stream is closed")
}

/// Holds our control stream and reads the server's unidirectional streams
/// — and, with datagrams, the connection's QUIC datagrams — each in a task
/// of its own, until the connection ends — or until the client aborts it,
/// and its tasks with it.
async fn drive(inner: Arc<Inner>) {
    let mut tasks = JoinSet::new();
    tasks.spawn(control(inner.clone()));
    if inner.flows.is_some() {
        tasks.spawn(datagram::read(inner.clone()));
    }
    loop {
        tokio::select! {
            accepted = inner.quic.accept_uni() => match accepted {
                Ok(recv) => {
                    tasks.spawn(server_stream(inner.clone(), recv));
                }
                Err(e) => {
                    inner.state.send_modify(|state| state.ended = Some(e));
                    return;
                }
            },
            // a stream that ended; nothing to collect
            Some(_) = tasks.join_next() => {}
        }
    }
}

/// Opens our control stream, sends our SETTINGS as its first frame, and
/// holds it open: closing it would end HTTP/3 on the connection (RFC 9114
/// 6.2.1). A failure is the connection's, which the driver reports.
async fn control(inner: Arc<Inner>) {
    let datagrams = inner.flows.is_some().then_some((SETTINGS_H3_DATAGRAM, 1));
    let mut payload = Vec::new();
    for (id, value) in OUR_SETTINGS.into_iter().chain(datagrams) {
        frame::put_varint(&mut payload, id);
        frame::put_varint(&mut payload, value);
    }
    let mut bytes = Vec::new();
    frame::put_varint(&mut bytes, CONTROL_STREAM);
    bytes.extend_from_slice(&frame::frame(frame::SETTINGS, &payload));
    if let Ok(mut send) = inner.quic.open_uni().await
        && send.write_all(&bytes).await.is_ok()
    {
        std::future::pending::<()>().await;
    }
}

/// Why reading one of the server's critical streams stopped.
enum Stop {
    /// The server broke HTTP/3: the connection closes.
    Violation(H3Error),
    /// The connection ended, which the driver reports.
    Lost,
}

impl From<H3Error> for Stop {
    fn from(error: H3Error) -> Stop {
        Stop::Violation(error)
    }
}

/// A failed read of a critical stream: its end is the server's error
/// (H3_CLOSED_CRITICAL_STREAM, cut within a frame or not); the connection's
/// end is not.
fn critical_failure(failure: ReadFailure) -> Stop {
    match failure {
        ReadFailure::Truncated | ReadFailure::Stream(ReadError::Reset(_)) => closed_critical(),
        ReadFailure::Stream(_) => Stop::Lost,
    }
}

fn closed_critical() -> Stop {
    Stop::Violation(H3Error::new(
        H3_CLOSED_CRITICAL_STREAM,
        "the server closed a critical stream",
    ))
}

/// One of the server's unidirectional streams, as its type says (RFC 9114
/// 6.2, RFC 9204 4.2).
async fn server_stream(inner: Arc<Inner>, recv: RecvStream) {
    let mut reader = FrameReader::new(recv);
    // a stream that ends before its type says nothing
    let Ok(Some(ty)) = poll_fn(|cx| reader.poll_varint(cx)).await else {
        return;
    };
    let stop = match ty {
        CONTROL_STREAM => match inner.opened(0b001, "control") {
            Ok(()) => match control_stream(&inner, &mut reader).await {
                Err(stop) => stop,
            },
            Err(error) => error.into(),
        },
        PUSH_STREAM => H3Error::new(H3_ID_ERROR, "the server opened a push stream").into(),
        QPACK_ENCODER_STREAM | QPACK_DECODER_STREAM => {
            let (bit, kind) = if ty == QPACK_ENCODER_STREAM {
                (0b010, "QPACK encoder")
            } else {
                (0b100, "QPACK decoder")
            };
            match inner.opened(bit, kind) {
                Ok(()) => qpack_stream(&mut reader).await,
                Err(error) => error.into(),
            }
        }
        // a type this client does not know (RFC 9114 6.2.3)
        _ => {
            reader.stop(H3_STREAM_CREATION_ERROR);
            return;
        }
    };
    if let Stop::Violation(error) = stop {
        inner.fail(error);
    }
}

/// The server's control stream: SETTINGS first, then GOAWAY; frames of
/// unknown types are passed over (RFC 9114 6.2.1, 7.2). It never ends but
/// in an error.
async fn control_stream(inner: &Inner, reader: &mut FrameReader) -> Result<Infallible, Stop> {
    let mut first = true;
    loop {
        let header = match poll_fn(|cx| reader.poll_header(cx)).await {
            Ok(Some(header)) => header,
            Ok(None) => return Err(closed_critical()),
            Err(failure) => return Err(critical_failure(failure)),
        };
        if first && header.ty != frame::SETTINGS {
            return Err(H3Error::new(
                H3_MISSING_SETTINGS,
                "the server's control stream did not start with SETTINGS",
            )
            .into());
        }
        match header.ty {
            frame::SETTINGS if first => {
                let settings = Settings::decode(&whole(reader, header).await?)?;
                inner
                    .state
                    .send_modify(|state| state.settings = Some(settings));
            }
            frame::GOAWAY => {
                let payload = whole(reader, header).await?;
                match frame::get_varint(&payload) {
                    Some((id, n)) if n == payload.len() => inner.goaway(id)?,
                    _ => {
                        return Err(H3Error::new(
                            H3_FRAME_ERROR,
                            "the server's GOAWAY is malformed",
                        )
                        .into());
                    }
                }
            }
            frame::CANCEL_PUSH => {
                return Err(H3Error::new(H3_ID_ERROR, "the server cancelled a push").into());
            }
            ty if matches!(
                ty,
                frame::DATA
                    | frame::HEADERS
                    | frame::SETTINGS
                    | frame::PUSH_PROMISE
                    | frame::MAX_PUSH_ID
            ) || frame::is_http2_only(ty) =>
            {
                return Err(H3Error::new(
                    H3_FRAME_UNEXPECTED,
                    format!("the server sent a frame of type {ty:#x} on its control stream"),
                )
                .into());
            }
            _ => {
                let mut left = header.len;
                poll_fn(|cx| reader.poll_skip(cx, &mut left))
                    .await
                    .map_err(critical_failure)?;
            }
        }
        first = false;
    }
}

/// The payload of a frame of the control stream, read whole.
async fn whole(reader: &mut FrameReader, header: Header) -> Result<Bytes, Stop> {
    if header.len > MAX_CONTROL_FRAME {
        return Err(H3Error::new(
            H3_EXCESSIVE_LOAD,
            format!("the server sent a control frame of {} bytes", header.len),
        )
        .into());
    }
    poll_fn(|cx| reader.poll_payload(cx, header.len as usize))
        .await
        .map_err(critical_failure)
}

/// The server's QPACK encoder or decoder stream. With no dynamic table on
/// either side there is nothing on it to act on: its bytes are dropped. It
/// must stay open (RFC 9204 4.2).
async fn qpack_stream(reader: &mut FrameReader) -> Stop {
    match poll_fn(|cx| reader.poll_drain(cx)).await {
        Ok(()) => closed_critical(),
        Err(failure) => critical_failure(failure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quic::h3::tests::{BOUND, connected};
    use quinn::VarInt;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    /// The server closed the connection, and the client's task has not
    /// seen it end yet: a failure found now does not close it again, which
    /// would put a local close in place of the server's.
    #[tokio::test]
    async fn a_failure_after_the_end_leaves_how_the_connection_ended() {
        let (_endpoint, quic, server_quic) = connected().await;
        server_quic.close(VarInt::from_u32(0x100), b"bye");
        timeout(BOUND, quic.closed())
            .await
            .expect("closed within the bound");
        // no task: nothing told the state that the connection ended
        let inner = Inner {
            quic: quic.clone(),
            state: watch::Sender::new(State::default()),
            critical: AtomicU8::new(0),
            flows: None,
        };
        inner.fail(H3Error::new(H3_FRAME_ERROR, "found late"));
        assert!(inner.state.borrow().failure.is_none());
        assert!(
            matches!(
                quic.close_reason(),
                Some(ConnectionError::ApplicationClosed(_))
            ),
            "{:?}",
            quic.close_reason()
        );
    }

    /// The reading of the connection's datagrams ends every flow however it
    /// ends — also when its task is aborted before it ever ran, as with the
    /// client dropped soon after it was made.
    #[tokio::test]
    async fn a_datagram_reading_dropped_before_it_ran_ends_the_flows() {
        let (_endpoint, quic, _server_quic) = connected().await;
        let inner = Arc::new(Inner {
            quic,
            state: watch::Sender::new(State::default()),
            critical: AtomicU8::new(0),
            flows: Some(Flows::default()),
        });
        let flows = inner.flows.as_ref().expect("made with flows");
        let mut open = flows.open(0).expect("flows open before the reading ends");
        // never polled
        drop(datagram::read(inner.clone()));
        assert_eq!(
            open.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        );
        assert!(flows.open(1).is_none());
    }

    /// A request without an authority cannot be sent: HTTPS wants one
    /// (RFC 9114 4.3.1), and so does a CONNECT.
    #[test]
    fn a_request_needs_an_authority() {
        for request in [
            Request::get("/index.html").body(()).unwrap(),
            Request::connect("/").body(()).unwrap(),
        ] {
            let err = head(&request).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(err.to_string(), "h3: a request needs an authority");
        }
    }

    /// Only a CONNECT carries a protocol (RFC 8441 4).
    #[test]
    fn a_request_with_a_protocol_must_be_a_connect() {
        let mut request = Request::post("https://example.test/").body(()).unwrap();
        request.extensions_mut().insert(Protocol::CONNECT_UDP);
        let err = head(&request).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "h3: a request with a protocol must be a CONNECT"
        );
    }
}
