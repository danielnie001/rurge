//! What the CONNECT proxies over HTTP/3 share — `masque`, and
//! `trust-tunnel` with `h3=true` (phase 2 M7 design 7.1, 7.2): HTTP/3 on
//! each new connection (`Http3`), the client a connection keeps
//! (`Connected`), how a request on it fails (`failure`), and the tunnel of
//! a classic CONNECT (RFC 9114 4.4) on a stream of its own (`tunnel`).
//!
//! - The request goes out at once: a classic CONNECT needs nothing of the
//!   server's SETTINGS.
//! - Nothing of the caller's goes before the response: a 2xx makes the
//!   stream the tunnel (RFC 9110 9.3.6); any other final status is the
//!   proxy's refusal, which the protocol words.
//! - Until the response came, a request the server never processed is
//!   `Lost` — one whose connection is gone (closed, lost by a server that
//!   restarted), one the client refused for the server's GOAWAY, one the
//!   server rejected (H3_REQUEST_REJECTED), one on a stream at or past the
//!   GOAWAY's limit (RFC 9114 4.1.1, 5.2) —: on another connection it may
//!   yet go through (`Session::request`). Any other failure is final, that
//!   of a request below the GOAWAY's limit on its own stream too: the
//!   server may have processed it.

use crate::session::{Failure, Lease, Protocol, Session};
use http::{Request, StatusCode};
use quinn::{Connection, VarInt};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts};
use rurge_net::quic::h3::{self, RequestStream};
use rurge_proto::OutboundError;
use std::io;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// H3_NO_ERROR (RFC 9114 8.1): a connection let go of.
pub(crate) const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);

/// A CONNECT proxy over HTTP/3, as its connections need it.
pub(crate) trait Proxy: Send + Sync + 'static {
    /// As its log lines begin (`masque`).
    const NAME: &'static str;

    /// Whether its HTTP/3 announces HTTP Datagrams and reads the
    /// connection's QUIC datagrams as them (`h3::Client::with_datagrams`).
    const DATAGRAMS: bool;
}

/// What every new connection of the proxy `X` runs before its requests:
/// HTTP/3, nothing more — each request carries its own credentials.
pub(crate) struct Http3<X>(PhantomData<X>);

impl<X> Http3<X> {
    pub(crate) const fn new() -> Http3<X> {
        Http3(PhantomData)
    }
}

impl<X: Proxy> Protocol for Http3<X> {
    type State = Connected;

    const NAME: &'static str = X::NAME;

    const NO_ERROR: VarInt = H3_NO_ERROR;

    /// Our SETTINGS go out, and requests may follow at once: a classic
    /// CONNECT waits for nothing of the server's (a CONNECT-UDP, for its
    /// SETTINGS alone).
    fn handshake<'a>(
        &'a self,
        connection: &'a Connection,
    ) -> BoxFuture<'a, Result<Connected, OutboundError>> {
        Box::pin(async move {
            let h3 = if X::DATAGRAMS {
                h3::Client::with_datagrams(connection.clone())
            } else {
                h3::Client::new(connection.clone())
            };
            Ok(Connected { h3 })
        })
    }

    /// The server sent GOAWAY: it is going down.
    fn is_going_away(&self, state: &Connected) -> bool {
        state.h3.is_going_away()
    }
}

/// What a connection keeps for its requests.
pub(crate) struct Connected {
    /// Lives as long as the connection: dropped, it would end our HTTP/3
    /// control stream, which a server may take for the end of HTTP/3 on the
    /// connection.
    pub(crate) h3: h3::Client,
}

/// The tunnel `request` — a classic CONNECT — asks for through one of
/// `session`'s connections (`open`), `refused` wording a final status
/// other than 2xx: one budget, `opts.timeout`, for the connection (when one
/// is dialed), QUIC's handshake and the proxy's response.
pub(crate) async fn tunnel<P: Protocol<State = Connected>>(
    session: &Session<P>,
    request: Request<()>,
    refused: fn(StatusCode) -> OutboundError,
    opts: &ConnectOpts,
) -> Result<BoxedStream, OutboundError> {
    let tunnel = session.request(opts, |lease| {
        Box::pin(open(lease, request.clone(), refused))
    });
    match tokio::time::timeout(opts.timeout, tunnel).await {
        Ok(tunnel) => Ok(Box::new(tunnel?) as BoxedStream),
        Err(_) => Err(OutboundError::Timeout),
    }
}

/// How a request failed before its response came — on `stream` once it has
/// one —: `Lost` when the server never processed it, else final. The error
/// says its layer (`h3: …`, `quic: …`).
pub(crate) fn failure(lease: &Lease<Connected>, stream: Option<u64>, e: io::Error) -> Failure {
    let h3 = &lease.state().h3;
    if lease.connection().close_reason().is_some()
        || h3::going_away(&e)
        || h3::rejected(&e)
        || stream.is_some_and(|id| h3.unprocessed(id))
    {
        Failure::Lost(e.into())
    } else {
        Failure::Failed(e.into())
    }
}

/// The tunnel `request` — a classic CONNECT — asks for, on the connection
/// of `lease`: the request on a stream of its own, then the proxy's
/// response; a final status other than 2xx is the error `refused` words.
/// The stream handed back is the caller's.
async fn open(
    lease: Lease<Connected>,
    request: Request<()>,
    refused: fn(StatusCode) -> OutboundError,
) -> Result<Tunnel, Failure> {
    let mut stream = lease
        .state()
        .h3
        .request(request)
        .await
        .map_err(|e| failure(&lease, None, e))?;
    let id = stream.id();
    let response = stream
        .response()
        .await
        .map_err(|e| failure(&lease, Some(id), e))?;
    match response.status() {
        status if status.is_success() => Ok(Tunnel {
            stream,
            writer: None,
            _lease: lease,
        }),
        status => Err(Failure::Failed(refused(status))),
    }
}

/// A TCP connection through the proxy: the CONNECT's stream — the bytes go
/// as DATA frames —, on a connection its lease keeps. A write that returns
/// may leave the rest of its frame for later: the next write, flush or
/// shutdown sends it, and so does a read, so that a caller that writes and
/// then only reads waits for no answer to bytes that never left. Shutdown
/// sends the FIN the target then sees; reading goes on to the proxy's.
/// Dropped before both directions ended, the stream is cancelled both ways
/// (RFC 9114 4.1.1). No `Debug`: nothing in it is worth printing.
struct Tunnel {
    stream: RequestStream,
    /// The task whose write, flush or shutdown waits for the stream to take
    /// it: it sends the rest of a frame itself.
    writer: Option<Waker>,
    /// Last: the stream ends before its connection may be closed.
    _lease: Lease<Connected>,
}

impl Tunnel {
    /// Notes whether the task of `cx` now waits to write.
    fn waits(&mut self, cx: &Context<'_>, pending: bool) {
        self.writer = pending.then(|| cx.waker().clone());
    }
}

impl AsyncRead for Tunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        // The rest of a frame goes first — unless another task waits to
        // write: quinn wakes one task for a stream's room, which must stay
        // that task's, and it sends the rest itself. What the flush comes to
        // is left aside. Pending, the stream wakes us when it takes more. An
        // error is the sending side's: the frame stays begun, and the next
        // write, flush or shutdown reports it — so nothing is lost —, while
        // reading goes on as far as the receiving side does: a server may
        // stop our side and answer still (RFC 9114 4.1).
        if this
            .writer
            .as_ref()
            .is_none_or(|writer| writer.will_wake(cx.waker()))
        {
            let _ = Pin::new(&mut this.stream).poll_flush(cx);
        }
        Pin::new(&mut this.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Tunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let written = Pin::new(&mut this.stream).poll_write(cx, buf);
        this.waits(cx, written.is_pending());
        written
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let flushed = Pin::new(&mut this.stream).poll_flush(cx);
        this.waits(cx, flushed.is_pending());
        flushed
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let shut = Pin::new(&mut this.stream).poll_shutdown(cx);
        this.waits(cx, shut.is_pending());
        shut
    }
}
