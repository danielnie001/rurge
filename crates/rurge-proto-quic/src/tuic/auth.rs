//! The authentication of every new connection (TUIC v5 SPEC,
//! "Authenticate"; phase 2 M7 design 6.2): a unidirectional stream of its
//! own carrying `05 00 ‖ UUID ‖ TOKEN` — the user's UUID, its 16 bytes as
//! they are, and the 32 bytes the connection's TLS session exports with the
//! UUID's bytes as the label and the password as the context (RFC 5705, RFC
//! 8446 7.5), which only the two ends of that session can work out.
//!
//! - The server answers nothing, and holds the commands that come before it
//!   checked the token (SPEC): the command goes out as the connection comes
//!   up, and the connection's requests follow without waiting.
//! - A server that refuses it closes the connection — tuic-server and
//!   sing-box with code 0 and no reason —, which the connection's tunnels
//!   then tell (`Authenticated::ended`).

use super::VERSION;
use crate::session::Protocol;
use quinn::{Connection, ConnectionError, ReadError, VarInt, WriteError};
use rurge_net::BoxFuture;
use rurge_net::quic::connection_error;
use rurge_proto::OutboundError;
use rurge_proto::outbound::untrusted_text;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

/// The command's type.
const AUTHENTICATE: u8 = 0x00;

/// The code TUIC's implementations close a connection with, whatever the
/// cause — tuic-server 1.0.0, tuic-client 1.0.0 and sing-box alike: the v5
/// SPEC defines none.
const CLOSE: VarInt = VarInt::from_u32(0);

/// How much of the server's reason for a close an error text quotes.
const REASON_MAX: usize = 200;

/// What authenticates the policy's connections. No `Debug`: it holds the
/// credentials.
pub(crate) struct Auth {
    uuid: [u8; 16],
    password: String,
}

/// What a connection keeps once its authentication went out.
#[derive(Default)]
pub(crate) struct Authenticated {
    /// The server relayed something on the connection: it took the
    /// authentication.
    relayed: AtomicBool,
}

impl Auth {
    pub(crate) fn new(uuid: [u8; 16], password: &str) -> Auth {
        Auth {
            uuid,
            password: password.to_string(),
        }
    }

    /// The token of `connection`'s TLS session.
    fn token(&self, connection: &Connection) -> [u8; 32] {
        let mut token = [0u8; 32];
        connection
            .export_keying_material(&mut token, &self.uuid, self.password.as_bytes())
            .expect("a connection is up once its TLS handshake is done, which exports 32 bytes");
        token
    }
}

/// `05 00 ‖ UUID ‖ TOKEN`.
fn command(uuid: &[u8; 16], token: &[u8; 32]) -> [u8; 50] {
    let mut command = [0u8; 50];
    command[..2].copy_from_slice(&[VERSION, AUTHENTICATE]);
    command[2..18].copy_from_slice(uuid);
    command[18..].copy_from_slice(token);
    command
}

impl Protocol for Auth {
    type State = Authenticated;

    const NAME: &'static str = "tuic";

    const NO_ERROR: VarInt = CLOSE;

    /// The command on a stream of its own, finished: handed to QUIC, it
    /// takes no round trip, and nothing comes back to wait for. Not left to
    /// a task of its own (as tuic-client 1.0.0 and sing-box leave it): it
    /// goes before the connection's first request, and a connection that
    /// cannot take it fails the dial.
    fn handshake<'a>(
        &'a self,
        connection: &'a Connection,
    ) -> BoxFuture<'a, Result<Authenticated, OutboundError>> {
        Box::pin(async move {
            let command = command(&self.uuid, &self.token(connection));
            let state = Authenticated::default();
            let mut stream = connection.open_uni().await.map_err(|e| state.ended(&e))?;
            stream
                .write_all(&command)
                .await
                .map_err(|e| state.write_error(e))?;
            // fails only for a stream finished or reset already
            let _ = stream.finish();
            Ok(state)
        })
    }
}

impl Authenticated {
    /// Something the server relayed came on the connection: a byte from a
    /// tunnel's target.
    pub(super) fn mark_relayed(&self) {
        if !self.relayed.load(Ordering::Relaxed) {
            self.relayed.store(true, Ordering::Relaxed);
        }
    }

    /// How the connection ended, as its requests and tunnels tell it
    /// (design 3.6, 6.2). The server's own close is TUIC's, and v5 defines
    /// no codes: `tuic: the server closed the connection (code …)`, its
    /// reason cleaned and cut short. Code 0 and no reason before the server
    /// relayed anything on the connection is how tuic-server and sing-box
    /// refuse the authentication: `tuic: authentication failed` (sing-box
    /// closes so whatever failed, its session's end included; once the
    /// server relayed, the authentication went through). Any other end as
    /// QUIC tells it.
    pub(super) fn ended(&self, e: &ConnectionError) -> io::Error {
        let ConnectionError::ApplicationClosed(close) = e else {
            return connection_error(e);
        };
        if close.error_code == CLOSE
            && close.reason.is_empty()
            && !self.relayed.load(Ordering::Relaxed)
        {
            return io::Error::new(
                io::ErrorKind::PermissionDenied,
                "tuic: authentication failed",
            );
        }
        let code = close.error_code.into_inner();
        let reason = untrusted_text(&String::from_utf8_lossy(&close.reason), REASON_MAX);
        let text = if reason.is_empty() {
            format!("tuic: the server closed the connection (code {code:#x})")
        } else {
            format!("tuic: the server closed the connection (code {code:#x}: {reason})")
        };
        io::Error::new(io::ErrorKind::ConnectionAborted, text)
    }

    /// A failed read on one of the connection's streams: the connection's
    /// end as `ended` tells it, the stream's as TUIC's.
    pub(super) fn read_error(&self, e: ReadError) -> io::Error {
        match e {
            ReadError::ConnectionLost(e) => self.ended(&e),
            ReadError::Reset(code) => io::Error::new(
                io::ErrorKind::ConnectionReset,
                format!(
                    "tuic: the server reset the stream (code {:#x})",
                    code.into_inner()
                ),
            ),
            ReadError::ClosedStream
            | ReadError::IllegalOrderedRead
            | ReadError::ZeroRttRejected => closed_stream(),
        }
    }

    /// A failed write on one of the connection's streams, as `read_error`.
    pub(super) fn write_error(&self, e: WriteError) -> io::Error {
        match e {
            WriteError::ConnectionLost(e) => self.ended(&e),
            WriteError::Stopped(code) => io::Error::new(
                io::ErrorKind::ConnectionReset,
                format!(
                    "tuic: the server stopped the stream (code {:#x})",
                    code.into_inner()
                ),
            ),
            WriteError::ClosedStream | WriteError::ZeroRttRejected => closed_stream(),
        }
    }
}

fn closed_stream() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "tuic: the stream is closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use quinn::ApplicationClose;

    #[test]
    fn the_command_is_its_header_the_uuid_and_the_token() {
        let uuid: [u8; 16] = std::array::from_fn(|i| i as u8);
        let command = command(&uuid, &[0xa5; 32]);
        assert_eq!(command[..2], [0x05, 0x00]);
        assert_eq!(command[2..18], uuid);
        assert_eq!(command[18..], [0xa5; 32]);
    }

    fn closed(code: u32, reason: &'static [u8]) -> ConnectionError {
        ConnectionError::ApplicationClosed(ApplicationClose {
            error_code: VarInt::from_u32(code),
            reason: Bytes::from_static(reason),
        })
    }

    /// Design 6.2: code 0 and no reason is the refused authentication only
    /// before the server relayed anything; any other close says its code,
    /// and the transport's ends are QUIC's.
    #[test]
    fn the_end_of_a_connection_reads_as_tuic_tells_it() {
        use io::ErrorKind::*;
        let fresh = Authenticated::default();
        let relayed = Authenticated::default();
        relayed.mark_relayed();
        for (state, error, kind, text) in [
            (
                &fresh,
                closed(0, b""),
                PermissionDenied,
                "tuic: authentication failed",
            ),
            (
                &relayed,
                closed(0, b""),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x0)",
            ),
            (
                &fresh,
                closed(0, b"bye"),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x0: bye)",
            ),
            (
                &fresh,
                closed(0x2a, b"go\r\naway"),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x2a: goaway)",
            ),
            (
                &fresh,
                ConnectionError::Reset,
                ConnectionReset,
                "quic: the server no longer knows the connection (stateless reset)",
            ),
            (
                &fresh,
                ConnectionError::TimedOut,
                TimedOut,
                "quic: the server stopped answering (idle timeout)",
            ),
        ] {
            let ended = state.ended(&error);
            assert_eq!((ended.kind(), ended.to_string().as_str()), (kind, text));
        }
        // as the streams on the connection tell it
        assert_eq!(
            fresh
                .read_error(ReadError::ConnectionLost(closed(0, b"")))
                .to_string(),
            "tuic: authentication failed"
        );
        assert_eq!(
            relayed
                .write_error(WriteError::ConnectionLost(closed(0, b"")))
                .to_string(),
            "tuic: the server closed the connection (code 0x0)"
        );
        assert_eq!(
            fresh
                .read_error(ReadError::Reset(VarInt::from_u32(3)))
                .to_string(),
            "tuic: the server reset the stream (code 0x3)"
        );
        assert_eq!(
            fresh
                .write_error(WriteError::Stopped(VarInt::from_u32(0)))
                .to_string(),
            "tuic: the server stopped the stream (code 0x0)"
        );
        // a reason as long as a datagram is cut short
        let long = ConnectionError::ApplicationClosed(ApplicationClose {
            error_code: VarInt::from_u32(1),
            reason: Bytes::from(vec![b'x'; 1000]),
        });
        assert_eq!(
            fresh.ended(&long).to_string(),
            format!(
                "tuic: the server closed the connection (code 0x1: {})",
                "x".repeat(REASON_MAX)
            )
        );
    }
}
