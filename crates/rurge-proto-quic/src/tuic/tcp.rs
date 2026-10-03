//! TCP over TUIC v5 (SPEC, "Connect"; phase 2 M7 design 6.2): every
//! connection a bidirectional stream of its own on the policy's QUIC
//! connection.
//!
//! - The stream begins `05 01 ‖ ADDR`, written as it opens, and is the
//!   tunnel at once: the server answers nothing. A target it cannot reach
//!   shows as the stream ending.
//! - While a tunnel lives, the connection's heartbeat goes (`heartbeat`).
//! - With no answer to wait for, a connection gone unknown to us — its
//!   server restarted — shows only once packets of the tunnel meet the
//!   server. A request goes again on a new connection (`Failure::Lost`)
//!   only when it finds its connection gone as the stream opens or as the
//!   Connect is handed to QUIC; after that the tunnel is the caller's, and
//!   ends with the connection.

use super::VERSION;
use super::addr;
use super::auth::Authenticated;
use super::heartbeat::Relaying;
use crate::session::{Failure, Lease};
use quinn::{RecvStream, SendStream, WriteError};
use rurge_net::connector::Target;
use rurge_proto::OutboundError;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The command's type.
const CONNECT: u8 = 0x01;

/// The Connect for `target`: refused, before anything is dialed, for a
/// target that cannot be named to the server.
pub(super) fn request(target: &Target) -> Result<Vec<u8>, OutboundError> {
    let mut request = vec![VERSION, CONNECT];
    addr::put(&mut request, target).map_err(|e| OutboundError::Proxy(e.to_string()))?;
    Ok(request)
}

/// The tunnel `request` asks for, on the connection of `lease`: the
/// Connect on a stream of its own, handed to QUIC, and the stream is the
/// caller's. A connection found gone by then is `Lost`: nothing of the
/// caller's went out yet.
pub(super) async fn open(lease: Lease<Authenticated>, request: &[u8]) -> Result<Tunnel, Failure> {
    let state = lease.state();
    let (mut send, recv) = lease
        .connection()
        .open_bi()
        .await
        .map_err(|e| Failure::Lost(state.ended(&e).into()))?;
    send.write_all(request).await.map_err(|e| match e {
        WriteError::ConnectionLost(e) => Failure::Lost(state.ended(&e).into()),
        e => Failure::Failed(state.write_error(e).into()),
    })?;
    Ok(Tunnel {
        send,
        recv,
        _relaying: state.relaying(),
        lease,
    })
}

/// A TCP connection through the server: the Connect's stream, on a
/// connection its lease keeps, counted among the connection's relays.
/// Shutdown sends the FIN the target then sees; reading goes on. Dropped,
/// the stream ends both ways (a FIN, and STOP_SENDING for what is left
/// unread). No `Debug`: nothing in it is worth printing.
pub(super) struct Tunnel {
    send: SendStream,
    recv: RecvStream,
    _relaying: Relaying,
    /// Last: the stream ends before its connection may be closed.
    lease: Lease<Authenticated>,
}

impl AsyncRead for Tunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let filled = buf.filled().len();
        let state = this.lease.state();
        match ready!(this.recv.poll_read_buf(cx, buf)) {
            Ok(()) => {
                if buf.filled().len() > filled {
                    state.mark_relayed();
                }
                Poll::Ready(Ok(()))
            }
            Err(e) => Poll::Ready(Err(state.read_error(e))),
        }
    }
}

impl AsyncWrite for Tunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        Pin::new(&mut this.send)
            .poll_write(cx, buf)
            .map_err(|e| this.lease.state().write_error(e))
    }

    /// QUIC sends what the stream took.
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // fails only for a stream finished or reset already
        let _ = self.get_mut().send.finish();
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;

    #[test]
    fn a_connect_is_its_header_and_the_address() {
        let target = Target::new(HostName::parse("192.0.2.1"), 443);
        assert_eq!(
            request(&target).unwrap(),
            [0x05, 0x01, 0x01, 192, 0, 2, 1, 0x01, 0xbb]
        );
        let target = Target::new(HostName::parse("example.test"), 80);
        let mut expected = vec![0x05, 0x01, 0x00, 12];
        expected.extend_from_slice(b"example.test");
        expected.extend_from_slice(&[0, 80]);
        assert_eq!(request(&target).unwrap(), expected);
        let unsendable = Target::new(HostName::Domain("a@b.test".into()), 80);
        let err = request(&unsendable).unwrap_err();
        assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
        assert_eq!(
            err.to_string(),
            "tuic: the host name cannot be sent to the server"
        );
    }
}
