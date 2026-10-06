//! TCP over MASQUE (manual: Policies › MASQUE; phase 2 M7 design 7.1):
//! every connection a classic HTTP/3 CONNECT of its own on the policy's
//! QUIC connection (RFC 9114 4.4).
//!
//! - The request is `:method CONNECT` and `:authority host:port` — an IPv6
//!   literal in brackets, a name as its A-labels —, with
//!   `proxy-authorization` when the policy has credentials. It goes out at
//!   once: a classic CONNECT needs nothing of the server's SETTINGS.
//! - Nothing of the caller's goes before the response: a 2xx makes the
//!   stream the tunnel (RFC 9110 9.3.6); 407 is a refused login, any other
//!   final status the proxy's refusal, quoted by its number.
//! - Until the response came, a request that finds its connection gone —
//!   closed, lost by a server that restarted, or going away — is `Lost`:
//!   nothing of the caller's went out, and on another connection it may yet
//!   go through (`Session::request`).

use super::{Connected, failure, proxy, refused};
use crate::session::{Failure, Lease};
use http::Request;
use http::header::{HeaderValue, PROXY_AUTHORIZATION};
use rurge_net::connector::Target;
use rurge_net::quic::h3::RequestStream;
use rurge_proto::OutboundError;
use rurge_proto::http::wire_host;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The CONNECT for `target`, carrying the policy's `authorization`: refused,
/// before anything is dialed, for a name that cannot be sent.
pub(super) fn request(
    target: &Target,
    authorization: Option<&HeaderValue>,
) -> Result<Request<()>, OutboundError> {
    let unsendable = || proxy("the host name cannot be sent to the server");
    let host = wire_host(target).ok_or_else(unsendable)?;
    let mut request = Request::connect(format!("{host}:{}", target.port))
        .body(())
        // a name longer than an authority may be
        .map_err(|_| unsendable())?;
    if let Some(authorization) = authorization {
        request
            .headers_mut()
            .insert(PROXY_AUTHORIZATION, authorization.clone());
    }
    Ok(request)
}

/// The tunnel `request` asks for, on the connection of `lease`: the request
/// on a stream of its own, then the proxy's response. The stream handed
/// back is the caller's.
pub(super) async fn open(lease: Lease<Connected>, request: Request<()>) -> Result<Tunnel, Failure> {
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
pub(super) struct Tunnel {
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

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;
    use rurge_config::HostName;

    #[test]
    fn a_request_names_its_target_as_host_and_port_and_nothing_else() {
        for (host, authority) in [
            (HostName::parse("192.0.2.1"), "192.0.2.1:443"),
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:443"),
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:443",
            ),
        ] {
            let request = request(&Target::new(host, 443), None).unwrap();
            assert_eq!(request.method(), Method::CONNECT);
            let uri = request.uri();
            assert_eq!(uri.authority().map(|a| a.as_str()), Some(authority));
            // a classic CONNECT: no scheme, no path (RFC 9114 4.4)
            assert!(uri.scheme().is_none(), "{uri}");
            assert!(uri.path_and_query().is_none(), "{uri}");
            assert!(request.headers().is_empty());
        }
        // longer than an authority may be
        let long = format!("{}.test", "a".repeat(70_000));
        for name in ["x@blocked.test", "a b.test", "", long.as_str()] {
            let target = Target::new(HostName::Domain(name.into()), 443);
            let err = request(&target, None).unwrap_err();
            assert_eq!(
                err.to_string(),
                "masque: the host name cannot be sent to the server"
            );
        }
    }

    #[test]
    fn the_credentials_go_marked_sensitive() {
        let mut authorization = HeaderValue::from_static("Basic dTpw");
        authorization.set_sensitive(true);
        let target = Target::new(HostName::parse("192.0.2.1"), 443);
        let request = request(&target, Some(&authorization)).unwrap();
        let headers = request.headers();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[PROXY_AUTHORIZATION], "Basic dTpw");
        // QPACK never indexes it (RFC 9204 7.1.3)
        assert!(headers[PROXY_AUTHORIZATION].is_sensitive());
        assert!(!format!("{request:?}").contains("dTpw"));
    }
}
