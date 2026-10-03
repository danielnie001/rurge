//! The authentication of every new connection (phase 2 M7 design 6.2,
//! 6.3): a unidirectional stream of its own carrying one command —
//!
//! - v5 (SPEC, "Authenticate"): `05 00 ‖ UUID ‖ TOKEN` — the user's UUID,
//!   its 16 bytes as they are, and the 32 bytes the connection's TLS
//!   session exports with the UUID's bytes as the label and the password as
//!   the context (RFC 5705, RFC 8446 7.5), which only the two ends of that
//!   session can work out;
//! - v4 (tuic 0.8.5's protocol README, "Authenticate"): `04 00 ‖ TOKEN` —
//!   the BLAKE3 hash of the token as written, as the server hashes the
//!   tokens it lets in: the same on every connection.
//!
//! - The server answers nothing, and holds the commands that come before it
//!   checked the token (both protocols): the command goes out as the
//!   connection comes up, and the connection's requests follow without
//!   waiting — UDP packets too, which sing-box leaves in QUIC's queue until
//!   then.
//! - A server that refuses it closes the connection — v5's (tuic-server and
//!   sing-box) with code 0 and no reason, v4's with a code of the protocol's
//!   —, which the connection's tunnels and carriers then tell
//!   (`Authenticated::ended`).

use super::Version;
use super::heartbeat::{self, Relaying, Relays};
use super::udp::Udp;
use crate::session::Protocol;
use quinn::{Connection, ConnectionError, ReadError, VarInt, WriteError};
use rurge_config::spec::TuicVersion;
use rurge_net::BoxFuture;
use rurge_net::quic::connection_error;
use rurge_proto::OutboundError;
use rurge_proto::outbound::untrusted_text;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::task::AbortOnDropHandle;

/// The command's type.
const AUTHENTICATE: u8 = 0x00;

/// The code TUIC v5's implementations close a connection with, whatever the
/// cause — tuic-server 1.0.0, tuic-client 1.0.0 and sing-box alike: the v5
/// SPEC defines none. The session closes v4's connections with it too: v4
/// gives it no meaning.
const CLOSE: VarInt = VarInt::from_u32(0);

// What a v4 server closes a connection with (protocol README, "Error
// Codes"), its error's text as the reason: the protocol's version or a
// command it cannot read, a token it does not know, an authentication that
// came too late, a command on the wrong kind of stream.
const PROTOCOL_ERROR: u64 = 0xffff_fff0;
const AUTHENTICATION_FAILED: u64 = 0xffff_fff1;
const AUTHENTICATION_TIMEOUT: u64 = 0xffff_fff2;
const BAD_COMMAND: u64 = 0xffff_fff3;

/// How much of the server's reason for a close an error text quotes.
const REASON_MAX: usize = 200;

/// What authenticates the policy's connections. No `Debug`: it holds the
/// credentials.
pub(crate) struct Auth {
    credentials: Credentials,
    /// How often the heartbeat goes while something is relayed.
    heartbeat: Duration,
}

/// What the command carries.
enum Credentials {
    /// The token's hash: what lets a client in, as the token itself does.
    V4 { token: [u8; 32] },
    /// The user, and the password its token is exported with.
    V5 { uuid: [u8; 16], password: String },
}

/// What a connection keeps once its authentication went out: its version,
/// what tells how it ended, its relays and their heartbeat, its UDP
/// associations.
pub(crate) struct Authenticated {
    version: Version,
    /// The server relayed something on the connection — a byte from a
    /// tunnel's target, a UDP packet —: it took the authentication.
    relayed: Arc<AtomicBool>,
    relays: Arc<Relays>,
    pub(super) udp: Udp,
    /// Sends the heartbeat; none for v5 on a connection without QUIC
    /// datagrams.
    _heartbeat: Option<AbortOnDropHandle<()>>,
}

impl Auth {
    /// The credentials of a `tuic` (v4) or `tuic-v5` line; the heartbeat
    /// every `heartbeat`.
    pub(crate) fn new(credentials: &TuicVersion, heartbeat: Duration) -> Auth {
        let credentials = match credentials {
            TuicVersion::V4 { token } => Credentials::V4 {
                token: token_hash(token.expose()),
            },
            TuicVersion::V5 { uuid, password } => Credentials::V5 {
                uuid: *uuid.expose(),
                password: password.expose().clone(),
            },
        };
        Auth {
            credentials,
            heartbeat,
        }
    }

    fn version(&self) -> Version {
        match self.credentials {
            Credentials::V4 { .. } => Version::V4,
            Credentials::V5 { .. } => Version::V5,
        }
    }

    /// The command for `connection`: v5's token is its TLS session's. A
    /// session that exports none fails the dial: rustls exports nothing once
    /// its state is an error, which a server can bring about after the
    /// handshake — just as QUIC tells the connection is up.
    fn command(&self, connection: &Connection) -> Result<Vec<u8>, OutboundError> {
        match &self.credentials {
            Credentials::V4 { token } => Ok(command_v4(token).to_vec()),
            Credentials::V5 { uuid, password } => {
                let mut token = [0u8; 32];
                connection
                    .export_keying_material(&mut token, uuid, password.as_bytes())
                    .map_err(|_| {
                        OutboundError::Tls("the session exports no keying material".to_string())
                    })?;
                Ok(command_v5(uuid, &token).to_vec())
            }
        }
    }
}

/// v4's token on the wire: the BLAKE3 hash of `token`'s bytes, as tuic
/// 0.8.5's server hashes the tokens it lets in.
fn token_hash(token: &str) -> [u8; 32] {
    *blake3::hash(token.as_bytes()).as_bytes()
}

/// `04 00 ‖ TOKEN`.
fn command_v4(token: &[u8; 32]) -> [u8; 34] {
    let mut command = [0u8; 34];
    command[..2].copy_from_slice(&[Version::V4.byte(), AUTHENTICATE]);
    command[2..].copy_from_slice(token);
    command
}

/// `05 00 ‖ UUID ‖ TOKEN`.
fn command_v5(uuid: &[u8; 16], token: &[u8; 32]) -> [u8; 50] {
    let mut command = [0u8; 50];
    command[..2].copy_from_slice(&[Version::V5.byte(), AUTHENTICATE]);
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
            let version = self.version();
            let command = self.command(connection)?;
            // nothing relayed on a connection that just came up
            let mut stream = connection
                .open_uni()
                .await
                .map_err(|e| ended(version, &e, false))?;
            stream
                .write_all(&command)
                .await
                .map_err(|e| write_error(version, e, false))?;
            // fails only for a stream finished or reset already
            let _ = stream.finish();
            Ok(Authenticated::start(connection, version, self.heartbeat))
        })
    }
}

impl Authenticated {
    /// The state of `connection`, its tasks started: the one that reads the
    /// server's packets, and the heartbeat where it can go.
    fn start(connection: &Connection, version: Version, every: Duration) -> Authenticated {
        let relayed = Arc::new(AtomicBool::new(false));
        let relays = Relays::new();
        Authenticated {
            version,
            udp: Udp::start(connection, version, relayed.clone()),
            _heartbeat: heartbeat::start(connection, &relays, version, every),
            relayed,
            relays,
        }
    }

    /// The TUIC the connection speaks.
    pub(super) fn version(&self) -> Version {
        self.version
    }

    /// Something the server relayed came on the connection.
    pub(super) fn mark_relayed(&self) {
        mark(&self.relayed);
    }

    /// A relay under way on the connection, until the guard goes: the
    /// heartbeat goes meanwhile.
    pub(super) fn relaying(&self) -> Relaying {
        self.relays.begin()
    }

    /// How the connection ended, as its requests, tunnels and carriers
    /// tell it (`ended`).
    pub(super) fn ended(&self, e: &ConnectionError) -> io::Error {
        ended(self.version, e, self.relayed.load(Ordering::Relaxed))
    }

    /// Whether the connection ended as a server ends one whose
    /// authentication it refused (`refused`).
    pub(super) fn refused(&self, e: &ConnectionError) -> bool {
        refused(self.version, e, self.relayed.load(Ordering::Relaxed))
    }

    /// A failed read on one of the connection's streams (`read_error`).
    pub(super) fn read_error(&self, e: ReadError) -> io::Error {
        read_error(self.version, e, self.relayed.load(Ordering::Relaxed))
    }

    /// A failed write on one of the connection's streams (`write_error`).
    pub(super) fn write_error(&self, e: WriteError) -> io::Error {
        write_error(self.version, e, self.relayed.load(Ordering::Relaxed))
    }

    /// Test support: whether the heartbeat's task runs.
    #[cfg(test)]
    pub(super) fn beats(&self) -> bool {
        self._heartbeat
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    }

    /// Test support: how many relays are under way.
    #[cfg(test)]
    pub(super) fn relays(&self) -> usize {
        self.relays.count()
    }
}

/// Marks `relayed`: the server took the authentication.
pub(super) fn mark(relayed: &AtomicBool) {
    if !relayed.load(Ordering::Relaxed) {
        relayed.store(true, Ordering::Relaxed);
    }
}

/// Whether a connection that ended so was refused its authentication. v4
/// says so in a code: the token, or its time. v5 defines none: code 0 and
/// no reason before the server relayed anything (`relayed`) is how
/// tuic-server and sing-box refuse it — sing-box closes so whatever failed,
/// its session's end included; once the server relayed, the authentication
/// went through.
fn refused(version: Version, e: &ConnectionError, relayed: bool) -> bool {
    let ConnectionError::ApplicationClosed(close) = e else {
        return false;
    };
    match version {
        Version::V4 => matches!(
            close.error_code.into_inner(),
            AUTHENTICATION_FAILED | AUTHENTICATION_TIMEOUT
        ),
        Version::V5 => close.error_code == CLOSE && close.reason.is_empty() && !relayed,
    }
}

/// How a connection ended (design 3.6, 6.2, 6.3), `relayed` whether the
/// server relayed anything on it. The server's own close is TUIC's: a
/// refused authentication (`refused`) says so, and so do v4's protocol
/// errors, the server's reason aside; any other close is `tuic: the server
/// closed the connection (code …)`, its reason cleaned and cut short. Any
/// other end as QUIC tells it.
fn ended(version: Version, e: &ConnectionError, relayed: bool) -> io::Error {
    use io::ErrorKind::{ConnectionAborted, PermissionDenied};
    let ConnectionError::ApplicationClosed(close) = e else {
        return connection_error(e);
    };
    let code = close.error_code.into_inner();
    let (kind, text) = match (version, code) {
        (Version::V4, AUTHENTICATION_TIMEOUT) => {
            (PermissionDenied, "tuic: authentication timed out")
        }
        (Version::V4, PROTOCOL_ERROR | BAD_COMMAND) => (ConnectionAborted, "tuic: protocol error"),
        _ if refused(version, e, relayed) => (PermissionDenied, "tuic: authentication failed"),
        _ => {
            let reason = untrusted_text(&String::from_utf8_lossy(&close.reason), REASON_MAX);
            let text = if reason.is_empty() {
                format!("tuic: the server closed the connection (code {code:#x})")
            } else {
                format!("tuic: the server closed the connection (code {code:#x}: {reason})")
            };
            return io::Error::new(ConnectionAborted, text);
        }
    };
    io::Error::new(kind, text)
}

/// A failed read on one of a connection's streams: the connection's end as
/// `ended` tells it, the stream's as TUIC's.
fn read_error(version: Version, e: ReadError, relayed: bool) -> io::Error {
    match e {
        ReadError::ConnectionLost(e) => ended(version, &e, relayed),
        ReadError::Reset(code) => io::Error::new(
            io::ErrorKind::ConnectionReset,
            format!(
                "tuic: the server reset the stream (code {:#x})",
                code.into_inner()
            ),
        ),
        ReadError::ClosedStream | ReadError::IllegalOrderedRead | ReadError::ZeroRttRejected => {
            closed_stream()
        }
    }
}

/// A failed write on one of a connection's streams, as `read_error`.
fn write_error(version: Version, e: WriteError, relayed: bool) -> io::Error {
    match e {
        WriteError::ConnectionLost(e) => ended(version, &e, relayed),
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

fn closed_stream() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "tuic: the stream is closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use quinn::ApplicationClose;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn the_command_is_its_header_the_uuid_and_the_token() {
        let uuid: [u8; 16] = std::array::from_fn(|i| i as u8);
        let command = command_v5(&uuid, &[0xa5; 32]);
        assert_eq!(command[..2], [0x05, 0x00]);
        assert_eq!(command[2..18], uuid);
        assert_eq!(command[18..], [0xa5; 32]);
    }

    /// Design 6.3: v4's token is the BLAKE3 hash of the token's bytes as
    /// written, against hashes computed with a BLAKE3 written in Python from
    /// its specification (checked against the official test vectors of
    /// inputs 0, 1, 3 and 1023 bytes long); the command is `04 00 ‖ TOKEN`.
    #[test]
    fn a_v4_token_is_the_blake3_hash_of_the_token() {
        for (token, hash) in [
            (
                "",
                "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
            ),
            (
                "abc",
                "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85",
            ),
            // the manual's example
            (
                "pwd",
                "4d013614663c2a826d5388d8766d0af9aec887a90f0289ffe333b0a6979f9d59",
            ),
        ] {
            assert_eq!(hex(&token_hash(token)), hash, "{token:?}");
        }
        let command = command_v4(&[0xa5; 32]);
        assert_eq!(command[..2], [0x04, 0x00]);
        assert_eq!(command[2..], [0xa5; 32]);
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
        for (relayed, error, kind, text) in [
            (
                false,
                closed(0, b""),
                PermissionDenied,
                "tuic: authentication failed",
            ),
            (
                true,
                closed(0, b""),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x0)",
            ),
            (
                false,
                closed(0, b"bye"),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x0: bye)",
            ),
            (
                false,
                closed(0x2a, b"go\r\naway"),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x2a: goaway)",
            ),
            // v4's codes mean nothing to v5
            (
                false,
                closed(0xffff_fff1, b"authentication failed"),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0xfffffff1: authentication failed)",
            ),
            (
                false,
                ConnectionError::Reset,
                ConnectionReset,
                "quic: the server no longer knows the connection (stateless reset)",
            ),
            (
                false,
                ConnectionError::TimedOut,
                TimedOut,
                "quic: the server stopped answering (idle timeout)",
            ),
        ] {
            let end = ended(Version::V5, &error, relayed);
            assert_eq!((end.kind(), end.to_string().as_str()), (kind, text));
            assert_eq!(
                refused(Version::V5, &error, relayed),
                kind == PermissionDenied
            );
        }
        // as the streams on the connection tell it
        assert_eq!(
            read_error(
                Version::V5,
                ReadError::ConnectionLost(closed(0, b"")),
                false
            )
            .to_string(),
            "tuic: authentication failed"
        );
        assert_eq!(
            write_error(
                Version::V5,
                WriteError::ConnectionLost(closed(0, b"")),
                true
            )
            .to_string(),
            "tuic: the server closed the connection (code 0x0)"
        );
        assert_eq!(
            read_error(Version::V5, ReadError::Reset(VarInt::from_u32(3)), false).to_string(),
            "tuic: the server reset the stream (code 0x3)"
        );
        assert_eq!(
            write_error(Version::V5, WriteError::Stopped(VarInt::from_u32(0)), false).to_string(),
            "tuic: the server stopped the stream (code 0x0)"
        );
        // a reason as long as a datagram is cut short
        let long = ConnectionError::ApplicationClosed(ApplicationClose {
            error_code: VarInt::from_u32(1),
            reason: Bytes::from(vec![b'x'; 1000]),
        });
        assert_eq!(
            ended(Version::V5, &long, false).to_string(),
            format!(
                "tuic: the server closed the connection (code 0x1: {})",
                "x".repeat(REASON_MAX)
            )
        );
    }

    /// Design 6.3: v4's codes tell a refused authentication — the token,
    /// or its time — and a protocol error, whatever the server's reason and
    /// whether it relayed anything; any other close says its code, code 0
    /// with no reason too (no refusal in v4).
    #[test]
    fn the_end_of_a_v4_connection_reads_as_its_codes_tell_it() {
        use io::ErrorKind::*;
        for (error, kind, text) in [
            (
                closed(0xffff_fff1, b"authentication failed"),
                PermissionDenied,
                "tuic: authentication failed",
            ),
            (
                closed(0xffff_fff2, b"authentication timeout"),
                PermissionDenied,
                "tuic: authentication timed out",
            ),
            (
                closed(0xffff_fff0, b"Unsupported TUIC version: 5"),
                ConnectionAborted,
                "tuic: protocol error",
            ),
            (
                closed(0xffff_fff3, b"bad command"),
                ConnectionAborted,
                "tuic: protocol error",
            ),
            (
                closed(0, b""),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x0)",
            ),
            (
                closed(0x2a, b"bye"),
                ConnectionAborted,
                "tuic: the server closed the connection (code 0x2a: bye)",
            ),
            (
                ConnectionError::Reset,
                ConnectionReset,
                "quic: the server no longer knows the connection (stateless reset)",
            ),
        ] {
            for relayed in [false, true] {
                let end = ended(Version::V4, &error, relayed);
                assert_eq!((end.kind(), end.to_string().as_str()), (kind, text));
                assert_eq!(
                    refused(Version::V4, &error, relayed),
                    kind == PermissionDenied
                );
            }
        }
        // as the streams on the connection tell it
        let refusal = || closed(0xffff_fff1, b"authentication failed");
        assert_eq!(
            read_error(Version::V4, ReadError::ConnectionLost(refusal()), true).to_string(),
            "tuic: authentication failed"
        );
        assert_eq!(
            write_error(Version::V4, WriteError::ConnectionLost(refusal()), false).to_string(),
            "tuic: authentication failed"
        );
    }
}
