//! TCP over TUIC (v5 SPEC, "Connect"; v4's protocol README, "Connect" and
//! "Response"; phase 2 M7 design 6.2, 6.3): every connection a
//! bidirectional stream of its own on the policy's QUIC connection.
//!
//! - The stream begins `VER 01 ‖ ADDR`, written as it opens.
//! - v5's server answers nothing: the stream is the tunnel at once, and a
//!   target it cannot reach shows as the stream ending. v4's answers on the
//!   stream once it tried the target, `04 ff ‖ REP` — `00` it reached it,
//!   `ff` it could not —, and nothing of the caller's goes before.
//! - While a tunnel lives, the connection's heartbeat goes (`heartbeat`).
//! - A request goes again on a new connection (`Failure::Lost`) when it
//!   finds its connection gone — its server restarted, say — while nothing
//!   of the caller's went out yet: as the stream opens or as the Connect is
//!   handed to QUIC, and in v4 until the response came. A v5 tunnel, with no
//!   response to wait for, finds a connection gone unknown to us only once
//!   its packets meet the server: by then it is the caller's, and ends with
//!   the connection.

use super::auth::Authenticated;
use super::heartbeat::Relaying;
use super::{Version, addr};
use crate::session::{Failure, Lease};
use quinn::{ReadError, ReadExactError, RecvStream, SendStream, WriteError};
use rurge_net::connector::Target;
use rurge_proto::OutboundError;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The command's type.
const CONNECT: u8 = 0x01;
/// v4's answer to a Connect, `04 ff ‖ REP`, and its two replies.
const V4: u8 = Version::V4.byte();
const RESPONSE: u8 = 0xff;
const SUCCEEDED: u8 = 0x00;
const FAILED: u8 = 0xff;

/// `version`'s Connect for `target`: refused, before anything is dialed,
/// for a target that cannot be named to the server.
pub(super) fn request(version: Version, target: &Target) -> Result<Vec<u8>, OutboundError> {
    let mut request = vec![version.byte(), CONNECT];
    addr::put(&mut request, target).map_err(|e| OutboundError::Proxy(e.to_string()))?;
    Ok(request)
}

/// The tunnel `request` asks for, on the connection of `lease`: the
/// Connect on a stream of its own, handed to QUIC — and in v4 the server's
/// response —, and the stream is the caller's. A connection found gone by
/// then is `Lost`: nothing of the caller's went out yet.
pub(super) async fn open(lease: Lease<Authenticated>, request: &[u8]) -> Result<Tunnel, Failure> {
    let state = lease.state();
    let (mut send, mut recv) = lease
        .connection()
        .open_bi()
        .await
        .map_err(|e| Failure::Lost(state.ended(&e).into()))?;
    send.write_all(request).await.map_err(|e| match e {
        WriteError::ConnectionLost(e) => Failure::Lost(state.ended(&e).into()),
        e => Failure::Failed(state.write_error(e).into()),
    })?;
    if state.version() == Version::V4 {
        response(state, &mut recv).await?;
    }
    Ok(Tunnel {
        send,
        recv,
        _relaying: state.relaying(),
        lease,
    })
}

/// v4: the server's response on the stream (`reply`).
async fn response(state: &Authenticated, recv: &mut RecvStream) -> Result<(), Failure> {
    let mut response = [0u8; 3];
    recv.read_exact(&mut response).await.map_err(|e| match e {
        ReadExactError::FinishedEarly(_) => {
            Failure::Failed(proxy("the server ended the stream without a response"))
        }
        ReadExactError::ReadError(ReadError::ConnectionLost(e)) => {
            Failure::Lost(state.ended(&e).into())
        }
        ReadExactError::ReadError(e) => Failure::Failed(state.read_error(e).into()),
    })?;
    reply(response).map_err(Failure::Failed)
}

/// What v4's response says: the server reached the target, or could not.
/// Anything but `04 ff 00` and `04 ff ff` is no response.
fn reply(response: [u8; 3]) -> Result<(), OutboundError> {
    match response {
        [V4, RESPONSE, SUCCEEDED] => Ok(()),
        [V4, RESPONSE, FAILED] => Err(proxy("the server could not connect to the target")),
        _ => Err(proxy("the server's response is malformed")),
    }
}

fn proxy(text: &str) -> OutboundError {
    OutboundError::Proxy(format!("tuic: {text}"))
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
            request(Version::V5, &target).unwrap(),
            [0x05, 0x01, 0x01, 192, 0, 2, 1, 0x01, 0xbb]
        );
        // v4's: the same, but for its version
        assert_eq!(
            request(Version::V4, &target).unwrap(),
            [0x04, 0x01, 0x01, 192, 0, 2, 1, 0x01, 0xbb]
        );
        let target = Target::new(HostName::parse("example.test"), 80);
        let mut expected = vec![0x05, 0x01, 0x00, 12];
        expected.extend_from_slice(b"example.test");
        expected.extend_from_slice(&[0, 80]);
        assert_eq!(request(Version::V5, &target).unwrap(), expected);
        let unsendable = Target::new(HostName::Domain("a@b.test".into()), 80);
        let err = request(Version::V4, &unsendable).unwrap_err();
        assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
        assert_eq!(
            err.to_string(),
            "tuic: the host name cannot be sent to the server"
        );
    }

    /// Design 6.3: v4's response `04 ff 00` lets the stream through, `04 ff
    /// ff` is the server's failure to reach the target; anything else is
    /// no response.
    #[test]
    fn a_v4_response_lets_the_stream_through_or_says_why_not() {
        assert!(reply([0x04, 0xff, 0x00]).is_ok());
        let err = reply([0x04, 0xff, 0xff]).unwrap_err();
        assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
        assert_eq!(
            err.to_string(),
            "tuic: the server could not connect to the target"
        );
        for malformed in [
            [0x04, 0xff, 0x01],
            [0x05, 0xff, 0x00],
            [0x04, 0x01, 0x00],
            [0x00, 0x00, 0x00],
        ] {
            assert_eq!(
                reply(malformed).unwrap_err().to_string(),
                "tuic: the server's response is malformed",
                "{malformed:?}"
            );
        }
    }
}
