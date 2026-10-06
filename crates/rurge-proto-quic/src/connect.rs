//! What the CONNECT proxies over HTTP/3 share — `masque`, and
//! `trust-tunnel` with `h3=true` (phase 2 M7 design 7.1, 7.2): the HTTP/3
//! client a connection keeps (`Connected`), how a request on it fails
//! (`failure`), and the tunnel of a classic CONNECT (RFC 9114 4.4) on a
//! stream of its own (`open`).
//!
//! - The request goes out at once: a classic CONNECT needs nothing of the
//!   server's SETTINGS.
//! - Nothing of the caller's goes before the response: a 2xx makes the
//!   stream the tunnel (RFC 9110 9.3.6); any other final status is the
//!   proxy's refusal, which the protocol words.
//! - Until the response came, a request that finds its connection gone —
//!   closed, lost by a server that restarted, or going away — is `Lost`:
//!   nothing of the caller's went out, and on another connection it may yet
//!   go through (`Session::request`).

use crate::session::{Failure, Lease};
use http::{Request, StatusCode};
use quinn::VarInt;
use rurge_net::quic::h3::{self, RequestStream};
use rurge_proto::OutboundError;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// H3_NO_ERROR (RFC 9114 8.1): a connection let go of.
pub(crate) const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);

/// What a connection keeps for its requests.
pub(crate) struct Connected {
    /// Lives as long as the connection: dropped, it would end our HTTP/3
    /// control stream, which a server may take for the end of HTTP/3 on the
    /// connection.
    pub(crate) h3: h3::Client,
}

/// How a request failed before its response came: `Lost` when its
/// connection turned out gone or going away, else final. The error says
/// its layer (`h3: …`, `quic: …`).
pub(crate) fn failure(lease: &Lease<Connected>, e: io::Error) -> Failure {
    if lease.connection().close_reason().is_some() || lease.state().h3.is_going_away() {
        Failure::Lost(e.into())
    } else {
        Failure::Failed(e.into())
    }
}

/// The tunnel `request` — a classic CONNECT — asks for, on the connection
/// of `lease`: the request on a stream of its own, then the proxy's
/// response; a final status other than 2xx is the error `refused` words.
/// The stream handed back is the caller's.
pub(crate) async fn open(
    lease: Lease<Connected>,
    request: Request<()>,
    refused: fn(StatusCode) -> OutboundError,
) -> Result<Tunnel, Failure> {
    let mut stream = lease
        .state()
        .h3
        .request(request)
        .await
        .map_err(|e| failure(&lease, e))?;
    let response = stream.response().await.map_err(|e| failure(&lease, e))?;
    match response.status() {
        status if status.is_success() => Ok(Tunnel {
            stream,
            _lease: lease,
        }),
        status => Err(Failure::Failed(refused(status))),
    }
}

/// A TCP connection through the proxy: the CONNECT's stream — the bytes go
/// as DATA frames —, on a connection its lease keeps. A write that returns
/// may leave the rest of its frame for the next write, flush or shutdown.
/// Shutdown sends the FIN the target then sees; reading goes on to the
/// proxy's. Dropped before both directions ended, the stream is cancelled
/// both ways (RFC 9114 4.1.1). No `Debug`: nothing in it is worth printing.
pub(crate) struct Tunnel {
    stream: RequestStream,
    /// Last: the stream ends before its connection may be closed.
    _lease: Lease<Connected>,
}

impl AsyncRead for Tunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Tunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}
