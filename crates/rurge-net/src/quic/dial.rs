//! The dial (phase 2 M7 design 3.2): a quinn endpoint of its own per
//! connection, the transport parameters every QUIC policy uses, and quinn's
//! errors as I/O errors (design 3.6).

use super::Path;
use quinn::congestion::CubicConfig;
use quinn::crypto::rustls::QuicClientConfig;
use quinn::{
    ClientConfig, Connection, ConnectionError, Endpoint, EndpointConfig, IdleTimeout, TokioRuntime,
    TransportConfig, TransportErrorCode, VarInt,
};
use std::io;
use std::sync::Arc;
use std::time::Duration;

/// The transport parameters of a client connection. `STANDARD` is what
/// every QUIC policy uses, `DNS` what a DNS server's connection uses; tests
/// shorten their times.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transport {
    /// Closes the connection when nothing came from the server for this
    /// long: a path that died (sleep, a network change) is noticed.
    pub idle_timeout: Duration,
    /// How often an otherwise idle connection proves it is alive; `None`
    /// leaves it to idle out.
    pub keep_alive: Option<Duration>,
    /// What a stream may receive before its reader catches up.
    pub stream_window: u32,
    /// What the connection's streams together may receive.
    pub connection_window: u32,
}

impl Transport {
    /// Idle timeout 30 seconds, keep-alive every 10; receive windows of
    /// 8 MiB per stream and 20 MiB per connection, as the Hysteria
    /// reference client's.
    pub const STANDARD: Transport = Transport {
        idle_timeout: Duration::from_secs(30),
        keep_alive: Some(Duration::from_secs(10)),
        stream_window: 8 << 20,
        connection_window: 20 << 20,
    };

    /// A DNS server's connection (DoQ, DoH3): as `STANDARD`, but nothing
    /// keeps it alive — unused, it idles out at the shorter of both ends'
    /// idle timeouts, and the next query dials again.
    pub const DNS: Transport = Transport {
        keep_alive: None,
        ..Transport::STANDARD
    };

    fn config(&self) -> TransportConfig {
        let mut config = TransportConfig::default();
        config
            // a timeout too long to say is none
            .max_idle_timeout(IdleTimeout::try_from(self.idle_timeout).ok())
            .keep_alive_interval(self.keep_alive)
            .stream_receive_window(VarInt::from_u32(self.stream_window))
            .receive_window(VarInt::from_u32(self.connection_window))
            // no protocol over QUIC here has the server open a
            // bidirectional stream; unidirectional ones stay as quinn has
            // them (HTTP/3's control and QPACK streams, TUIC's packets)
            .max_concurrent_bidi_streams(VarInt::from_u32(0))
            // quinn's own default, and its only congestion control not
            // marked experimental (M7-D10)
            .congestion_controller_factory(Arc::new(CubicConfig::default()));
        config
    }
}

/// The client configuration of a QUIC policy: `tls` — which must allow TLS
/// 1.3 (quinn panics on a handshake without it) and offers the protocol's
/// ALPN — with `transport`. Built once per outbound; never 0-RTT.
pub fn client_config(
    tls: Arc<rustls::ClientConfig>,
    transport: &Transport,
) -> io::Result<ClientConfig> {
    let crypto = QuicClientConfig::try_from(tls)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("tls: {e}")))?;
    let mut config = ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport.config()));
    Ok(config)
}

/// A connection to the server behind `path`, on an endpoint of its own
/// that ends with the connection. `server_name` is what TLS verifies and
/// sends as SNI (none for an IP address). Nothing bounds the handshake
/// here but the idle timeout: the caller's request timeout does (design
/// 3.5). A handshake that ends because the path's carrier failed fails
/// with the carrier's own error.
pub async fn connect(
    path: Path,
    config: &ClientConfig,
    server_name: &str,
) -> io::Result<Connection> {
    let carrier = path.carrier;
    let endpoint = Endpoint::new_with_abstract_socket(
        EndpointConfig::default(),
        None,
        path.socket,
        Arc::new(TokioRuntime),
    )?;
    let connecting = endpoint
        .connect_with(config.clone(), path.remote, server_name)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("quic: {e}")))?;
    connecting.await.map_err(|e| {
        // quinn knows only that the endpoint is gone (`DRIVER_LOST`): the
        // carrier knows why
        let failure = carrier.and_then(|carrier| carrier.failure());
        failure.unwrap_or_else(|| describe(&e, true))
    })
}

/// A connection's end as an I/O error: `quic: …`, or `tls: …` for a failed
/// handshake. Neither quotes anything sent; what the server sent along is
/// cleaned and cut short.
pub fn connection_error(e: &ConnectionError) -> io::Error {
    describe(e, false)
}

/// How long a reason from the server may be in an error text.
const REASON_MAX: usize = 200;

/// quinn's reason when the endpoint's receive loop is gone: the socket
/// under it failed (a carrier that closed) and every connection with it.
const DRIVER_LOST: &str = "endpoint driver future was dropped";

fn describe(e: &ConnectionError, handshake: bool) -> io::Error {
    use io::ErrorKind::{
        BrokenPipe, ConnectionAborted, ConnectionReset, InvalidData, Other, TimedOut,
    };
    let (kind, text) = match e {
        ConnectionError::TimedOut if handshake => {
            (TimedOut, "quic: the handshake timed out".to_string())
        }
        ConnectionError::TimedOut => (
            TimedOut,
            "quic: the server stopped answering (idle timeout)".to_string(),
        ),
        ConnectionError::Reset => (
            ConnectionReset,
            "quic: the server no longer knows the connection (stateless reset)".to_string(),
        ),
        // closed here — or, once its driver goes, a connection whose
        // endpoint was lost (`DRIVER_LOST` before)
        ConnectionError::LocallyClosed => (
            ConnectionAborted,
            "quic: the connection was closed".to_string(),
        ),
        ConnectionError::VersionMismatch => (
            Other,
            "quic: the server does not speak QUIC version 1".to_string(),
        ),
        ConnectionError::CidsExhausted => (
            Other,
            "quic: no connection ID left for a new connection".to_string(),
        ),
        // what went wrong here; a TLS failure carries the alert it sent
        ConnectionError::TransportError(error) => {
            if alert(error.code.into()).is_some() {
                (
                    InvalidData,
                    format!("tls: {}", untrusted(error.reason.as_bytes())),
                )
            } else if error.code == TransportErrorCode::INTERNAL_ERROR
                && error.reason == DRIVER_LOST
            {
                (
                    BrokenPipe,
                    "quic: the UDP path to the server failed".to_string(),
                )
            } else if error.reason.is_empty() {
                (InvalidData, format!("quic: {}", error.code))
            } else {
                (
                    InvalidData,
                    format!("quic: {}", untrusted(error.reason.as_bytes())),
                )
            }
        }
        ConnectionError::ConnectionClosed(close) => match alert(close.error_code.into()) {
            Some(alert) => (
                InvalidData,
                format!("tls: the server refused the handshake ({alert:?})"),
            ),
            None => (
                ConnectionAborted,
                closed(
                    "transport error",
                    u64::from(close.error_code),
                    &close.reason,
                ),
            ),
        },
        ConnectionError::ApplicationClosed(close) => (
            ConnectionAborted,
            closed("code", close.error_code.into_inner(), &close.reason),
        ),
    };
    io::Error::new(kind, text)
}

/// The TLS alert of a QUIC transport error code: CRYPTO_ERROR is 0x100
/// plus the alert (RFC 9001 4.8).
fn alert(code: u64) -> Option<rustls::AlertDescription> {
    (0x100..0x200)
        .contains(&code)
        .then(|| rustls::AlertDescription::from(code as u8))
}

fn closed(what: &str, code: u64, reason: &[u8]) -> String {
    let reason = untrusted(reason);
    if reason.is_empty() {
        format!("quic: the server closed the connection ({what} {code:#x})")
    } else {
        format!("quic: the server closed the connection ({what} {code:#x}: {reason})")
    }
}

/// Text from the other end, made safe for a log line: control characters
/// dropped, at most `REASON_MAX` characters kept.
fn untrusted(text: &[u8]) -> String {
    String::from_utf8_lossy(text)
        .chars()
        .filter(|c| !c.is_control())
        .take(REASON_MAX)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use quinn::{ApplicationClose, ConnectionClose};

    /// quinn's settings as its `Debug` shows them: ours, and the defaults
    /// HTTP/3 and TUIC rely on (100 unidirectional streams from the server,
    /// datagram buffers, path MTU discovery from 1200 bytes).
    #[test]
    fn the_transport_parameters_reach_quinn() {
        let config = format!("{:?}", Transport::STANDARD.config());
        for expected in [
            "max_concurrent_bidi_streams: 0,",
            "max_concurrent_uni_streams: 100,",
            "max_idle_timeout: Some(30000),",
            "stream_receive_window: 8388608,",
            "receive_window: 20971520,",
            "keep_alive_interval: Some(10s),",
            "initial_mtu: 1200,",
            "upper_bound: 1452,",
            "datagram_receive_buffer_size: Some(1250000),",
            "datagram_send_buffer_size: 1048576,",
        ] {
            assert!(config.contains(expected), "{expected} in {config}");
        }
    }

    /// A DNS server's connection is the standard one, but nothing keeps it
    /// alive.
    #[test]
    fn a_dns_connection_is_not_kept_alive() {
        assert_eq!(
            Transport::DNS,
            Transport {
                keep_alive: None,
                ..Transport::STANDARD
            }
        );
        let config = format!("{:?}", Transport::DNS.config());
        for expected in [
            "keep_alive_interval: None,",
            "max_idle_timeout: Some(30000),",
            "max_concurrent_bidi_streams: 0,",
        ] {
            assert!(config.contains(expected), "{expected} in {config}");
        }
    }

    /// QUIC's first packets are protected with AES-128-GCM: TLS without it
    /// cannot be used.
    #[test]
    fn tls_without_the_initial_cipher_suite_is_refused() {
        let provider = rustls::crypto::CryptoProvider {
            cipher_suites: vec![rustls::crypto::ring::cipher_suite::TLS13_AES_256_GCM_SHA384],
            ..rustls::crypto::ring::default_provider()
        };
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
        let err = client_config(Arc::new(tls), &Transport::STANDARD).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(err.to_string(), "tls: no initial cipher suite found");
    }

    /// A transport error found here, with its reason.
    fn found(code: TransportErrorCode, reason: &str) -> ConnectionError {
        let mut error = ConnectionError::TransportError(code.into());
        if let ConnectionError::TransportError(e) = &mut error {
            e.reason = reason.to_string();
        }
        error
    }

    #[test]
    fn quinns_errors_read_as_quic_or_tls() {
        use io::ErrorKind::*;
        let cases = [
            (
                found(
                    TransportErrorCode::crypto(42),
                    "invalid peer certificate: BadSignature",
                ),
                InvalidData,
                "tls: invalid peer certificate: BadSignature",
            ),
            (
                found(TransportErrorCode::INTERNAL_ERROR, DRIVER_LOST),
                BrokenPipe,
                "quic: the UDP path to the server failed",
            ),
            (
                found(TransportErrorCode::PROTOCOL_VIOLATION, "bad\nframe"),
                InvalidData,
                "quic: badframe",
            ),
            (
                found(TransportErrorCode::FLOW_CONTROL_ERROR, ""),
                InvalidData,
                "quic: received more data than permitted in advertised data limits",
            ),
            (
                ConnectionError::ConnectionClosed(ConnectionClose {
                    error_code: TransportErrorCode::crypto(120),
                    frame_type: None,
                    reason: Bytes::new(),
                }),
                InvalidData,
                "tls: the server refused the handshake (NoApplicationProtocol)",
            ),
            (
                ConnectionError::ConnectionClosed(ConnectionClose {
                    error_code: TransportErrorCode::CONNECTION_REFUSED,
                    frame_type: None,
                    reason: Bytes::new(),
                }),
                ConnectionAborted,
                "quic: the server closed the connection (transport error 0x2)",
            ),
            (
                ConnectionError::ApplicationClosed(ApplicationClose {
                    error_code: VarInt::from_u32(0x100),
                    reason: Bytes::from_static(b"bye"),
                }),
                ConnectionAborted,
                "quic: the server closed the connection (code 0x100: bye)",
            ),
            (
                ConnectionError::Reset,
                ConnectionReset,
                "quic: the server no longer knows the connection (stateless reset)",
            ),
            (
                ConnectionError::TimedOut,
                TimedOut,
                "quic: the server stopped answering (idle timeout)",
            ),
            (
                ConnectionError::LocallyClosed,
                ConnectionAborted,
                "quic: the connection was closed",
            ),
            (
                ConnectionError::VersionMismatch,
                Other,
                "quic: the server does not speak QUIC version 1",
            ),
            (
                ConnectionError::CidsExhausted,
                Other,
                "quic: no connection ID left for a new connection",
            ),
        ];
        for (error, kind, text) in cases {
            let io = connection_error(&error);
            assert_eq!(
                (io.kind(), io.to_string().as_str()),
                (kind, text),
                "{error:?}"
            );
        }
        assert_eq!(
            describe(&ConnectionError::TimedOut, true).to_string(),
            "quic: the handshake timed out"
        );
    }

    /// What the server says is cut short.
    #[test]
    fn a_long_reason_is_cut_short() {
        let error = ConnectionError::ApplicationClosed(ApplicationClose {
            error_code: VarInt::from_u32(1),
            reason: Bytes::from(vec![b'x'; 1000]),
        });
        assert_eq!(
            connection_error(&error).to_string(),
            format!(
                "quic: the server closed the connection (code 0x1: {})",
                "x".repeat(REASON_MAX)
            )
        );
    }
}
