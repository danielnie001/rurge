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
    /// The packet size — the UDP payload — a connection over a carrier
    /// (`underlying-proxy`) starts from; `None` is the 1200 bytes every
    /// path carries. quinn probes no path whose datagrams may be fragmented
    /// on the way: such a connection stays at the size it starts from,
    /// unless its packets of that size turn out to be lost, which brings it
    /// down to 1200 for good (quinn's black-hole detection). A connection
    /// on a socket of its own starts from 1200 whatever this says, and
    /// finds what its path carries by probing.
    pub carried_initial_mtu: Option<u16>,
}

impl Transport {
    /// Idle timeout 30 seconds, keep-alive every 10. A stream receives into
    /// 8 MiB, as the Hysteria reference client's; the connection into 32
    /// times that, 256 MiB. Credit for the connection comes back only as
    /// data is read: the streams nobody reads — a paused download is one —
    /// keep what they were sent, a window each at most, and in that
    /// client's 20 MiB three of them would leave the other streams nothing.
    /// The window is a bound, not memory set aside: a connection holds what
    /// its unread streams were sent. Once they hold more than seven eighths
    /// of it — 29 with their windows full — the others stop too: quinn
    /// announces credit an eighth of the window at a time, and the streams
    /// that are read can no longer bring in that much.
    pub const STANDARD: Transport = Transport {
        idle_timeout: Duration::from_secs(30),
        keep_alive: Some(Duration::from_secs(10)),
        stream_window: 8 << 20,
        connection_window: 32 * (8 << 20),
        carried_initial_mtu: None,
    };

    /// A DNS server's connection (DoQ, DoH3): as `STANDARD`, but nothing
    /// keeps it alive — unused, it idles out at the shorter of both ends'
    /// idle timeouts, and the next query dials again —, and it receives
    /// into 20 MiB: an answer is short and read as it comes, none of its
    /// streams holds the window.
    pub const DNS: Transport = Transport {
        keep_alive: None,
        connection_window: 20 << 20,
        ..Transport::STANDARD
    };

    /// `config` with these parameters in place of the ones it was made
    /// with (`client_config`), as a connection on a socket of its own takes
    /// them.
    pub fn apply(&self, config: &mut ClientConfig) {
        config.transport_config(Arc::new(self.config(false)));
    }

    /// As `apply`, as a connection over a carrier takes them
    /// (`Path::is_carried`): it starts from `carried_initial_mtu`.
    pub fn apply_carried(&self, config: &mut ClientConfig) {
        config.transport_config(Arc::new(self.config(true)));
    }

    /// quinn's configuration of a connection over a carrier (`carried`), or
    /// of one on a socket of its own.
    fn config(&self, carried: bool) -> TransportConfig {
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
        // `min_mtu` stays quinn's 1200 bytes: what the connection comes
        // down to on a path that does not carry the size it started from
        if let Some(mtu) = self.carried_initial_mtu.filter(|_| carried) {
            config.initial_mtu(mtu);
        }
        config
    }
}

/// The client configuration of a QUIC policy: `tls` — which must allow TLS
/// 1.3 (quinn panics on a handshake without it) and offers the protocol's
/// ALPN — with `transport`, as a connection on a socket of its own takes it
/// (a copy with `Transport::apply_carried` is what one over a carrier
/// takes). Built once per outbound; never 0-RTT.
pub fn client_config(
    tls: Arc<rustls::ClientConfig>,
    transport: &Transport,
) -> io::Result<ClientConfig> {
    let crypto = QuicClientConfig::try_from(tls)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("tls: {e}")))?;
    let mut config = ClientConfig::new(Arc::new(crypto));
    transport.apply(&mut config);
    Ok(config)
}

/// A connection to the server behind `path`, on an endpoint of its own
/// that ends with the connection. `server_name` is what TLS verifies and
/// sends as SNI (none for an IP address). Nothing bounds the handshake
/// here but the idle timeout: the caller's request timeout does (design
/// 3.5). A handshake that ends because the path's carrier failed fails
/// with the carrier's own error; one that ends for any other reason says
/// that reason, whatever the carrier reported before.
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
        // quinn knows only that the endpoint is gone: the carrier knows why
        let failure = carrier
            .filter(|_| lost_its_endpoint(&e))
            .and_then(|carrier| carrier.failure());
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

/// Whether `e` is quinn's word for a connection whose endpoint is gone.
fn driver_lost(e: &ConnectionError) -> bool {
    matches!(
        e,
        ConnectionError::TransportError(error)
            if error.code == TransportErrorCode::INTERNAL_ERROR && error.reason == DRIVER_LOST
    )
}

/// Whether a handshake that ended with `e` lost its endpoint: the socket
/// under it failed. The connection's driver says so as it goes
/// (`DRIVER_LOST`), and the connection it leaves is closed from here in
/// its wake (`LocallyClosed`): a dial reads either, whichever it finds.
/// Nothing else closes a connection here before its handshake is over.
fn lost_its_endpoint(e: &ConnectionError) -> bool {
    driver_lost(e) || matches!(e, ConnectionError::LocallyClosed)
}

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
            } else if driver_lost(e) {
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
    use crate::connector::Datagram;
    use crate::quic::carrier::{Carried, STAND_IN};
    use bytes::Bytes;
    use quinn::udp::RecvMeta;
    use quinn::{ApplicationClose, AsyncUdpSocket, ConnectionClose};
    use std::io::IoSliceMut;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Waker};
    use tokio::io::ReadBuf;

    /// quinn's settings as its `Debug` shows them: ours, and the defaults
    /// HTTP/3 and TUIC rely on (100 unidirectional streams from the server,
    /// datagram buffers, path MTU discovery from 1200 bytes).
    #[test]
    fn the_transport_parameters_reach_quinn() {
        let config = format!("{:?}", Transport::STANDARD.config(false));
        for expected in [
            "max_concurrent_bidi_streams: 0,",
            "max_concurrent_uni_streams: 100,",
            "max_idle_timeout: Some(30000),",
            "stream_receive_window: 8388608,",
            "receive_window: 268435456,",
            "keep_alive_interval: Some(10s),",
            "initial_mtu: 1200,",
            "upper_bound: 1452,",
            "datagram_receive_buffer_size: Some(1250000),",
            "datagram_send_buffer_size: 1048576,",
        ] {
            assert!(config.contains(expected), "{expected} in {config}");
        }
    }

    /// `apply` puts a transport's parameters in place of the ones a
    /// configuration was made with.
    #[test]
    fn a_transport_replaces_the_one_a_configuration_was_made_with() {
        let tls = crate::quic::testing::TestCert::new().client_tls(&["h3"]);
        let mut config = client_config(tls, &Transport::STANDARD).unwrap();
        let narrower = Transport {
            connection_window: 32 << 20,
            ..Transport::STANDARD
        };
        narrower.apply(&mut config);
        let shown = format!("{config:?}");
        for expected in [
            "receive_window: 33554432,",
            "stream_receive_window: 8388608,",
            "max_idle_timeout: Some(30000),",
        ] {
            assert!(shown.contains(expected), "{expected} in {shown}");
        }
    }

    /// A DNS server's connection is the standard one, but nothing keeps it
    /// alive, and it receives into 20 MiB.
    #[test]
    fn a_dns_connection_is_not_kept_alive() {
        assert_eq!(
            Transport::DNS,
            Transport {
                keep_alive: None,
                connection_window: 20 << 20,
                ..Transport::STANDARD
            }
        );
        let config = format!("{:?}", Transport::DNS.config(false));
        for expected in [
            "keep_alive_interval: None,",
            "max_idle_timeout: Some(30000),",
            "max_concurrent_bidi_streams: 0,",
            "stream_receive_window: 8388608,",
            "receive_window: 20971520,",
        ] {
            assert!(config.contains(expected), "{expected} in {config}");
        }
    }

    /// The size a connection over a carrier starts from reaches quinn for
    /// such a connection alone, and its floor stays at 1200 bytes; on a
    /// socket of its own the connection starts from 1200 as ever. A
    /// transport that names no size starts from 1200 over a carrier too.
    #[test]
    fn only_a_connection_over_a_carrier_starts_from_the_carried_size() {
        let transport = Transport {
            carried_initial_mtu: Some(1350),
            ..Transport::STANDARD
        };
        let tls = crate::quic::testing::TestCert::new().client_tls(&["h3"]);
        let own = client_config(tls, &transport).unwrap();
        let mut carried = own.clone();
        transport.apply_carried(&mut carried);
        let (own, carried) = (format!("{own:?}"), format!("{carried:?}"));
        for expected in ["initial_mtu: 1200,", "min_mtu: 1200,"] {
            assert!(own.contains(expected), "{expected} in {own}");
        }
        for expected in [
            "initial_mtu: 1350,",
            "min_mtu: 1200,",
            "receive_window: 268435456,",
        ] {
            assert!(carried.contains(expected), "{expected} in {carried}");
        }
        for unnamed in [Transport::STANDARD, Transport::DNS] {
            let carried = format!("{:?}", unnamed.config(true));
            assert!(carried.contains("initial_mtu: 1200,"), "{carried}");
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

    /// A handshake whose endpoint is gone ends in either of two ways, and
    /// in no other.
    #[test]
    fn a_handshake_that_lost_its_endpoint_ends_in_either_of_two_ways() {
        assert!(lost_its_endpoint(&found(
            TransportErrorCode::INTERNAL_ERROR,
            DRIVER_LOST
        )));
        assert!(lost_its_endpoint(&ConnectionError::LocallyClosed));
        for other in [
            ConnectionError::TimedOut,
            ConnectionError::Reset,
            found(TransportErrorCode::INTERNAL_ERROR, "another reason"),
            found(TransportErrorCode::PROTOCOL_VIOLATION, DRIVER_LOST),
        ] {
            assert!(!lost_its_endpoint(&other), "{other:?}");
        }
    }

    /// A carrier that takes what it is sent and never answers; its first
    /// receive fails with `failing`.
    struct Silent {
        failing: Mutex<Option<io::Error>>,
    }

    impl Datagram for Silent {
        fn poll_send(&self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_recv(&self, _cx: &mut Context<'_>, _buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            match self.failing.lock().unwrap().take() {
                Some(e) => Poll::Ready(Err(e)),
                None => Poll::Pending,
            }
        }
    }

    /// A handshake that ends for a reason of its own says that reason,
    /// whatever its carrier reported before: a reset, which quinn passed
    /// over, or a failure the endpoint outlived.
    #[tokio::test]
    async fn a_handshake_that_times_out_says_so_whatever_the_carrier_reported_before() {
        let silent = |first: io::Error| {
            Arc::new(Carried::new(Box::new(Silent {
                failing: Mutex::new(Some(first)),
            })))
        };
        let reset = silent(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "reset by the peer",
        ));
        let failed = silent(io::Error::new(
            io::ErrorKind::Unsupported,
            "proxy: the server relays no UDP",
        ));
        // this carrier's failure is taken here: quinn's endpoint never
        // sees it
        let mut storage = [0u8; 64];
        let taken = failed.poll_recv(
            &mut Context::from_waker(Waker::noop()),
            &mut [IoSliceMut::new(&mut storage)],
            &mut [RecvMeta::default()],
        );
        assert!(matches!(taken, Poll::Ready(Err(_))), "{taken:?}");
        // a handshake nothing answers ends at the idle timeout, which QUIC
        // keeps no shorter than three probe timeouts: both wait at once
        let transport = Transport {
            idle_timeout: Duration::from_millis(300),
            ..Transport::STANDARD
        };
        let tls = crate::quic::testing::TestCert::new().client_tls(&["h3"]);
        let config = client_config(tls, &transport).unwrap();
        let dial = |carried: Arc<Carried>| {
            let path = Path {
                socket: carried.clone(),
                remote: STAND_IN,
                carrier: Some(carried),
            };
            connect(path, &config, "localhost")
        };
        let both = async { tokio::join!(dial(reset), dial(failed)) };
        let (after_reset, after_failure) = tokio::time::timeout(Duration::from_secs(10), both)
            .await
            .expect("both outcomes within the bound");
        for err in [after_reset.unwrap_err(), after_failure.unwrap_err()] {
            assert_eq!(
                (err.kind(), err.to_string().as_str()),
                (io::ErrorKind::TimedOut, "quic: the handshake timed out")
            );
        }
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
