//! A minimal HTTP/3 client (phase 2 M7 design 4): what the QUIC-based
//! protocols need of HTTP/3 — Hysteria 2's authentication request now,
//! MASQUE's and `trust-tunnel`'s CONNECT and DNS over HTTP/3 in M7c.
//!
//! - Our control stream carries our SETTINGS: no dynamic QPACK table
//!   (capacity 0, no blocked streams) and field sections of at most 64 KiB.
//!   We open no QPACK streams: an encoder that never uses a dynamic table
//!   and a decoder that has none have nothing to say on them (RFC 9204
//!   4.2).
//! - Of the server's streams, its control stream must start with SETTINGS
//!   and may then bring GOAWAY; its QPACK streams are read and their bytes
//!   dropped; a push stream is a connection error (we never send
//!   MAX_PUSH_ID); a stream of any other type is stopped.
//! - Each request is a bidirectional stream of its own: HEADERS, then the
//!   content as DATA frames written through the stream; the response head
//!   comes back (interim responses passed over), then the response content
//!   as the stream's bytes. The trailers are ignored.
//! - QPACK uses the static table alone: an exact entry, else a literal
//!   value with the entry of the name, else a literal name and value.
//!   Nothing sent is Huffman-encoded; what comes back may be.
//! - An error of the connection closes it with its RFC 9114 or RFC 9204
//!   code; an error of a response ends its stream both ways.
//!
//! The client opens bidirectional streams for its requests and takes none
//! of the connection's others: a protocol may open streams of its own on
//! the same connection (Hysteria 2's TCP requests).
//!
//! Extended CONNECT (`:protocol`) and HTTP Datagrams come in M7c: the
//! server's SETTINGS are kept for them (`Settings::get`).

mod client;
mod frame;
mod huffman;
mod qpack;
mod stream;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use client::Client;
pub use frame::Settings;
pub use stream::RequestStream;

use quinn::VarInt;
use std::borrow::Cow;
use std::io;

/// The dynamic table capacity a peer's QPACK encoder may use (RFC 9204 5).
pub const SETTINGS_QPACK_MAX_TABLE_CAPACITY: u64 = 0x01;
/// The largest field section a peer accepts (RFC 9114 7.2.4.1).
pub const SETTINGS_MAX_FIELD_SECTION_SIZE: u64 = 0x06;
/// How many streams a peer's QPACK decoder lets wait for the dynamic table
/// (RFC 9204 5).
pub const SETTINGS_QPACK_BLOCKED_STREAMS: u64 = 0x07;
/// Extended CONNECT (RFC 9220 3).
pub const SETTINGS_ENABLE_CONNECT_PROTOCOL: u64 = 0x08;
/// HTTP Datagrams (RFC 9297 2.1.1).
pub const SETTINGS_H3_DATAGRAM: u64 = 0x33;

/// The largest field section this client takes — names and values plus 32
/// bytes per field (RFC 9114 4.2.2) — as our SETTINGS announce. It also
/// bounds the payload of a HEADERS frame: an encoder that compresses at
/// all never needs more bytes than the section's size.
pub(crate) const MAX_FIELD_SECTION_SIZE: u64 = 64 << 10;

// unidirectional stream types (RFC 9114 6.2, RFC 9204 4.2)
pub(crate) const CONTROL_STREAM: u64 = 0x00;
pub(crate) const PUSH_STREAM: u64 = 0x01;
pub(crate) const QPACK_ENCODER_STREAM: u64 = 0x02;
pub(crate) const QPACK_DECODER_STREAM: u64 = 0x03;

// error codes (RFC 9114 8.1, RFC 9204 6)
pub(crate) const H3_STREAM_CREATION_ERROR: VarInt = VarInt::from_u32(0x103);
pub(crate) const H3_CLOSED_CRITICAL_STREAM: VarInt = VarInt::from_u32(0x104);
pub(crate) const H3_FRAME_UNEXPECTED: VarInt = VarInt::from_u32(0x105);
pub(crate) const H3_FRAME_ERROR: VarInt = VarInt::from_u32(0x106);
pub(crate) const H3_EXCESSIVE_LOAD: VarInt = VarInt::from_u32(0x107);
pub(crate) const H3_ID_ERROR: VarInt = VarInt::from_u32(0x108);
pub(crate) const H3_SETTINGS_ERROR: VarInt = VarInt::from_u32(0x109);
pub(crate) const H3_MISSING_SETTINGS: VarInt = VarInt::from_u32(0x10a);
pub(crate) const H3_REQUEST_CANCELLED: VarInt = VarInt::from_u32(0x10c);
pub(crate) const H3_MESSAGE_ERROR: VarInt = VarInt::from_u32(0x10e);
pub(crate) const QPACK_DECOMPRESSION_FAILED: VarInt = VarInt::from_u32(0x200);

/// The name of an HTTP/3 or QPACK error code, as error texts say it; an
/// unknown code in hex.
pub(crate) fn code_name(code: VarInt) -> Cow<'static, str> {
    Cow::Borrowed(match u64::from(code) {
        0x100 => "H3_NO_ERROR",
        0x101 => "H3_GENERAL_PROTOCOL_ERROR",
        0x102 => "H3_INTERNAL_ERROR",
        0x103 => "H3_STREAM_CREATION_ERROR",
        0x104 => "H3_CLOSED_CRITICAL_STREAM",
        0x105 => "H3_FRAME_UNEXPECTED",
        0x106 => "H3_FRAME_ERROR",
        0x107 => "H3_EXCESSIVE_LOAD",
        0x108 => "H3_ID_ERROR",
        0x109 => "H3_SETTINGS_ERROR",
        0x10a => "H3_MISSING_SETTINGS",
        0x10b => "H3_REQUEST_REJECTED",
        0x10c => "H3_REQUEST_CANCELLED",
        0x10d => "H3_REQUEST_INCOMPLETE",
        0x10e => "H3_MESSAGE_ERROR",
        0x10f => "H3_CONNECT_ERROR",
        0x110 => "H3_VERSION_FALLBACK",
        0x200 => "QPACK_DECOMPRESSION_FAILED",
        0x201 => "QPACK_ENCODER_STREAM_ERROR",
        0x202 => "QPACK_DECODER_STREAM_ERROR",
        other => return Cow::Owned(format!("{other:#x}")),
    })
}

/// A violation of HTTP/3 or QPACK: the code that closes the connection or
/// ends the stream it happened on, and what happened.
#[derive(Clone, Debug)]
pub(crate) struct H3Error {
    pub(crate) code: VarInt,
    pub(crate) text: Cow<'static, str>,
}

impl H3Error {
    pub(crate) fn new(code: VarInt, text: impl Into<Cow<'static, str>>) -> H3Error {
        H3Error {
            code,
            text: text.into(),
        }
    }

    pub(crate) fn io(&self) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, format!("h3: {}", self.text))
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{H3Server, ServerStream, response_section};
    use super::*;
    use crate::quic::testing::{TestCert, endpoint, loopback_socket};
    use crate::quic::{Path, Transport, client_config, connect};
    use http::{HeaderValue, Request, Response};
    use quinn::{Connection, ConnectionError, Endpoint, SendStream, TransportConfig};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Waker};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::time::{Instant, timeout};

    const ALPN: &[&str] = &["h3"];
    const BOUND: Duration = Duration::from_secs(10);
    /// The fake's SETTINGS unless a test says otherwise.
    const SERVER_SETTINGS: &[(u64, u64)] = &[(SETTINGS_MAX_FIELD_SECTION_SIZE, 1 << 16)];
    /// `:status 200` from the static table.
    const OK: &[u8] = &[0x00, 0x00, 0xd9];

    /// A QUIC connection over the loopback with the client's `transport`
    /// and `tune` applied to the server's: the server's endpoint, the
    /// client's side and the server's side.
    async fn connected_with(
        transport: &Transport,
        tune: impl FnOnce(&mut TransportConfig),
    ) -> (Endpoint, Connection, Connection) {
        let cert = TestCert::new();
        let mut config = cert.server_config(ALPN);
        let mut server_transport = TransportConfig::default();
        tune(&mut server_transport);
        config.transport_config(Arc::new(server_transport));
        let server = endpoint(config, loopback_socket());
        let path = Path {
            socket: loopback_socket(),
            remote: server.local_addr().unwrap(),
        };
        let client = client_config(cert.client_tls(ALPN), transport).unwrap();
        let (connection, accepted) = tokio::join!(connect(path, &client, "localhost"), async {
            server
                .accept()
                .await
                .expect("a connection")
                .await
                .expect("accepted")
        });
        (server, connection.expect("connected"), accepted)
    }

    async fn connected() -> (Endpoint, Connection, Connection) {
        connected_with(&Transport::STANDARD, |_| {}).await
    }

    /// Our client and the fake on one connection, the fake's SETTINGS sent.
    struct Pair {
        client: Client,
        server: H3Server,
        /// The client's side of the connection, for streams of its own.
        quic: Connection,
        server_quic: Connection,
        _endpoint: Endpoint,
    }

    async fn pair() -> Pair {
        let (endpoint, quic, server_quic) = connected().await;
        Pair {
            client: Client::new(quic.clone()),
            server: H3Server::start(server_quic.clone(), SERVER_SETTINGS)
                .await
                .unwrap(),
            quic,
            server_quic,
            _endpoint: endpoint,
        }
    }

    fn get(uri: &str) -> Request<()> {
        Request::get(uri).body(()).unwrap()
    }

    fn tunnel(authority: &str) -> Request<()> {
        Request::connect(authority).body(()).unwrap()
    }

    fn owned(fields: &[(&str, &str)]) -> Vec<(String, String)> {
        fields
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    /// A unidirectional stream of the fake's carrying `bytes`, ended or not.
    async fn open_uni(quic: &Connection, bytes: &[u8], end: bool) -> io::Result<SendStream> {
        let mut send = quic.open_uni().await?;
        send.write_all(bytes).await?;
        if end {
            let _ = send.finish();
        }
        Ok(send)
    }

    /// The code the client closed the connection with, as the server saw it.
    async fn closed_with(server_quic: &Connection) -> u64 {
        match timeout(BOUND, server_quic.closed())
            .await
            .expect("closed within the bound")
        {
            ConnectionError::ApplicationClosed(close) => close.error_code.into_inner(),
            other => panic!("closed otherwise: {other:?}"),
        }
    }

    /// Waits until `check` holds, looking again and again until the bound.
    async fn eventually(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !check() {
            assert!(Instant::now() < deadline, "not within the bound");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// A request without content, answered 204 without content.
    async fn round_trip(client: &Client, server: &H3Server) -> io::Result<u16> {
        let request = async {
            let mut stream = client.request(get("https://example.test/")).await?;
            stream.shutdown().await?;
            Ok(stream.response().await?.status().as_u16())
        };
        let serve = async {
            let mut stream = server.accept().await.expect("a request");
            stream.read_headers().await.unwrap();
            stream.send_headers(204, &[]).await.unwrap();
            stream.finish();
        };
        let (status, ()) = timeout(BOUND, async { tokio::join!(request, serve) })
            .await
            .expect("within the bound");
        status
    }

    /// A request answered with a HEADERS frame of `section`, the server's
    /// side left open: the response, and the server's side of the stream.
    async fn answered_with(p: &Pair, section: &[u8]) -> (io::Result<Response<()>>, ServerStream) {
        let request = async {
            let mut stream = p.client.request(get("https://example.test/")).await?;
            stream.shutdown().await?;
            stream.response().await
        };
        let serve = async {
            let mut stream = p.server.accept().await.expect("a request");
            stream.read_headers().await.unwrap();
            stream.send_frame(0x01, section).await.unwrap();
            stream
        };
        timeout(BOUND, async { tokio::join!(request, serve) })
            .await
            .expect("within the bound")
    }

    /// Hysteria 2's authentication as it goes: the head with its secret, a
    /// content, a response of status 233 with fields the fake encodes
    /// with Huffman, and its content.
    #[tokio::test]
    async fn a_request_and_its_response_with_content() {
        let p = pair().await;
        let mut secret = HeaderValue::from_static("secret");
        secret.set_sensitive(true);
        let request = Request::post("https://hysteria/auth")
            .header("hysteria-auth", secret)
            .header("hysteria-cc-rx", "0")
            .body(())
            .unwrap();
        let client = async {
            let mut stream = p.client.request(request).await.unwrap();
            stream.write_all(b"ping").await.unwrap();
            stream.shutdown().await.unwrap();
            let response = stream.response().await.unwrap();
            let mut content = Vec::new();
            stream.read_to_end(&mut content).await.unwrap();
            (response, content)
        };
        let server = async {
            let mut stream = p.server.accept().await.unwrap();
            let head = stream.read_headers().await.unwrap();
            let content = stream.read_body().await.unwrap();
            stream
                .send_headers(233, &[("hysteria-udp", "true"), ("hysteria-cc-rx", "auto")])
                .await
                .unwrap();
            stream.send_data(b"pong").await.unwrap();
            stream.finish();
            (head, content)
        };
        let ((response, content), (head, request_content)) =
            timeout(BOUND, async { tokio::join!(client, server) })
                .await
                .unwrap();
        assert_eq!(
            head,
            owned(&[
                (":method", "POST"),
                (":scheme", "https"),
                (":authority", "hysteria"),
                (":path", "/auth"),
                ("hysteria-auth", "secret"),
                ("hysteria-cc-rx", "0"),
            ])
        );
        assert_eq!(request_content, b"ping");
        assert_eq!(response.status(), 233);
        assert_eq!(response.headers()["hysteria-udp"], "true");
        assert_eq!(response.headers()["hysteria-cc-rx"], "auto");
        assert_eq!(content, b"pong");
    }

    /// Contents larger than the windows, both ways; the response's in
    /// frames of one byte, a megabyte and the rest.
    #[tokio::test]
    async fn large_contents_cross_both_ways() {
        let p = pair().await;
        let up: Vec<u8> = (0..3usize << 20).map(|i| (i % 251) as u8).collect();
        let down: Vec<u8> = up.iter().rev().copied().collect();
        let client = async {
            let mut stream = p
                .client
                .request(Request::post("https://example.test/").body(()).unwrap())
                .await
                .unwrap();
            stream.write_all(&up).await.unwrap();
            stream.shutdown().await.unwrap();
            assert_eq!(stream.response().await.unwrap().status(), 200);
            let mut content = Vec::new();
            stream.read_to_end(&mut content).await.unwrap();
            content
        };
        let server = async {
            let mut stream = p.server.accept().await.unwrap();
            stream.read_headers().await.unwrap();
            let content = stream.read_body().await.unwrap();
            stream.send_headers(200, &[]).await.unwrap();
            stream.send_data(&down[..1]).await.unwrap();
            stream.send_data(&down[1..1 << 20]).await.unwrap();
            stream.send_data(&down[1 << 20..]).await.unwrap();
            stream.finish();
            content
        };
        let (content, received) = timeout(BOUND, async { tokio::join!(client, server) })
            .await
            .unwrap();
        assert!(received == up, "the request's content");
        assert!(content == down, "the response's content");
    }

    #[tokio::test]
    async fn the_client_sends_its_settings_and_no_qpack_streams() {
        let p = pair().await;
        let settings = timeout(BOUND, p.server.client_settings())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            settings,
            [
                (SETTINGS_QPACK_MAX_TABLE_CAPACITY, 0),
                (SETTINGS_MAX_FIELD_SECTION_SIZE, 65_536),
                (SETTINGS_QPACK_BLOCKED_STREAMS, 0),
            ]
        );
        assert_eq!(round_trip(&p.client, &p.server).await.unwrap(), 204);
        assert_eq!(p.server.client_streams(), [CONTROL_STREAM]);
    }

    /// Requests do not wait for the server's SETTINGS; the wait for them
    /// ends when they come, all of them kept.
    #[tokio::test]
    async fn settings_that_come_late_are_waited_for() {
        let (_endpoint, quic, server_quic) = connected().await;
        let client = Client::new(quic);
        let mut server = H3Server::new(server_quic);
        assert!(
            timeout(Duration::from_millis(200), client.settings())
                .await
                .is_err(),
            "SETTINGS that never came"
        );
        assert_eq!(round_trip(&client, &server).await.unwrap(), 204);
        let (settings, sent) = timeout(BOUND, async {
            tokio::join!(
                client.settings(),
                server.send_settings(&[(SETTINGS_ENABLE_CONNECT_PROTOCOL, 1), (0x21, 7)])
            )
        })
        .await
        .unwrap();
        sent.unwrap();
        let settings = settings.unwrap();
        assert_eq!(settings.get(SETTINGS_ENABLE_CONNECT_PROTOCOL), Some(1));
        assert_eq!(settings.get(0x21), Some(7));
        assert_eq!(settings.get(SETTINGS_H3_DATAGRAM), None);
    }

    #[tokio::test]
    async fn goaway_stops_new_requests_and_lets_those_under_way_finish() {
        let mut p = pair().await;
        timeout(BOUND, async {
            let mut first = p
                .client
                .request(get("https://example.test/a"))
                .await
                .unwrap();
            first.shutdown().await.unwrap();
            let mut served = p.server.accept().await.unwrap();
            served.read_headers().await.unwrap();
            // the first request is on stream 0: GOAWAY 8, then 4, leave it
            // served (a GOAWAY may come down)
            p.server.goaway(8).await.unwrap();
            p.server.goaway(4).await.unwrap();
            eventually(|| p.client.is_going_away()).await;
            let err = p
                .client
                .request(get("https://example.test/b"))
                .await
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
            assert_eq!(err.to_string(), "h3: the server is going away");
            served.send_headers(200, &[]).await.unwrap();
            served.finish();
            assert_eq!(first.response().await.unwrap().status(), 200);
        })
        .await
        .unwrap();
    }

    /// A GOAWAY may only come down, and names a request stream.
    #[tokio::test]
    async fn a_goaway_that_goes_up_or_names_no_request_closes_the_connection() {
        for ids in [&[8, 12][..], &[2]] {
            let mut p = pair().await;
            for &id in ids {
                p.server.goaway(id).await.unwrap();
            }
            assert_eq!(
                closed_with(&p.server_quic).await,
                u64::from(H3_ID_ERROR),
                "GOAWAY {ids:?}"
            );
        }
    }

    /// What the fake does on a fresh connection, to see the client close it.
    enum Act {
        /// SETTINGS on the fake's control stream.
        Settings(&'static [(u64, u64)]),
        /// A frame on the control stream, after the SETTINGS.
        Control(u64, &'static [u8]),
        /// A unidirectional stream of these bytes, left open.
        Open(&'static [u8]),
        /// A unidirectional stream of these bytes, ended.
        Ended(&'static [u8]),
    }

    #[tokio::test]
    async fn the_servers_streams_are_checked() {
        use Act::*;
        // a control stream with an empty SETTINGS frame
        const CONTROL: &[u8] = &[0x00, 0x04, 0x00];
        let cases: [(&str, &[Act], VarInt); 15] = [
            (
                "a push stream",
                &[Settings(&[]), Open(&[0x01, 0x00])],
                H3_ID_ERROR,
            ),
            (
                "a second control stream",
                &[Settings(&[]), Open(CONTROL)],
                H3_STREAM_CREATION_ERROR,
            ),
            (
                "a second QPACK decoder stream",
                &[Settings(&[]), Open(&[0x03]), Open(&[0x03])],
                H3_STREAM_CREATION_ERROR,
            ),
            (
                "a control stream that starts with GOAWAY",
                &[Open(&[0x00, 0x07, 0x01, 0x00])],
                H3_MISSING_SETTINGS,
            ),
            (
                "a second SETTINGS",
                &[Settings(&[]), Control(0x04, &[])],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "an HTTP/2 setting",
                &[Settings(&[(0x02, 1)])],
                H3_SETTINGS_ERROR,
            ),
            (
                "a setting twice",
                &[Settings(&[(0x06, 1), (0x06, 2)])],
                H3_SETTINGS_ERROR,
            ),
            (
                "DATA on the control stream",
                &[Settings(&[]), Control(0x00, b"x")],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "HEADERS on the control stream",
                &[Settings(&[]), Control(0x01, OK)],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "HTTP/2's PING",
                &[Settings(&[]), Control(0x06, &[0; 8])],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "MAX_PUSH_ID, a client's frame",
                &[Settings(&[]), Control(0x0d, &[0x00])],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "CANCEL_PUSH with no push allowed",
                &[Settings(&[]), Control(0x03, &[0x00])],
                H3_ID_ERROR,
            ),
            (
                "a GOAWAY with more than its ID",
                &[Settings(&[]), Control(0x07, &[0x00, 0x00])],
                H3_FRAME_ERROR,
            ),
            (
                "the control stream ended",
                &[Ended(CONTROL)],
                H3_CLOSED_CRITICAL_STREAM,
            ),
            (
                "a QPACK encoder stream ended",
                &[Settings(&[]), Ended(&[0x02])],
                H3_CLOSED_CRITICAL_STREAM,
            ),
        ];
        for (case, acts, code) in cases {
            let (_endpoint, quic, server_quic) = connected().await;
            let _client = Client::new(quic);
            let mut server = H3Server::new(server_quic.clone());
            let mut streams = Vec::new();
            for act in acts {
                // an act after the close finds the connection gone
                let _ = match act {
                    Settings(settings) => server.send_settings(settings).await,
                    Control(ty, payload) => server.control_frame(*ty, payload).await,
                    Open(bytes) => open_uni(&server_quic, bytes, false)
                        .await
                        .map(|s| streams.push(s)),
                    Ended(bytes) => open_uni(&server_quic, bytes, true)
                        .await
                        .map(|s| streams.push(s)),
                };
            }
            assert_eq!(closed_with(&server_quic).await, u64::from(code), "{case}");
        }
    }

    /// The QPACK streams' instructions are dropped, a stream of an unknown
    /// type is stopped, an unknown frame is passed over: the connection
    /// goes on.
    #[tokio::test]
    async fn qpack_streams_and_unknown_types_are_passed_over() {
        let mut p = pair().await;
        timeout(BOUND, async {
            // Set Dynamic Table Capacity 0; Stream Cancellation of stream 0
            let _encoder = open_uni(&p.server_quic, &[0x02, 0x20], false)
                .await
                .unwrap();
            let _decoder = open_uni(&p.server_quic, &[0x03, 0x40], false)
                .await
                .unwrap();
            let reserved = open_uni(&p.server_quic, &[0x21, b'?'], false)
                .await
                .unwrap();
            assert_eq!(
                reserved.stopped().await.unwrap(),
                Some(H3_STREAM_CREATION_ERROR)
            );
            p.server.control_frame(0x21, b"reserved").await.unwrap();
            assert_eq!(round_trip(&p.client, &p.server).await.unwrap(), 204);
        })
        .await
        .unwrap();
        assert!(p.quic.close_reason().is_none());
    }

    #[tokio::test]
    async fn interim_responses_unknown_frames_and_trailers_are_passed_over() {
        let p = pair().await;
        let client = async {
            let mut stream = p
                .client
                .request(get("https://example.test/"))
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
            let response = stream.response().await.unwrap();
            let mut content = Vec::new();
            stream.read_to_end(&mut content).await.unwrap();
            (response, content)
        };
        let server = async {
            let mut stream = p.server.accept().await.unwrap();
            stream.read_headers().await.unwrap();
            stream.send_frame(0x21, b"before").await.unwrap();
            stream
                .send_headers(103, &[("link", "</style.css>; rel=preload")])
                .await
                .unwrap();
            stream.send_headers(100, &[]).await.unwrap();
            stream
                .send_headers(200, &[("content-type", "text/plain")])
                .await
                .unwrap();
            stream.send_data(b"a").await.unwrap();
            stream.send_frame(0x1f * 7 + 0x21, b"").await.unwrap();
            stream.send_data(b"").await.unwrap();
            stream.send_data(b"b").await.unwrap();
            // trailers, then a reserved frame
            stream.send_frame(0x01, &[0x00, 0x00]).await.unwrap();
            stream.send_frame(0x21, b"after").await.unwrap();
            stream.finish();
        };
        let ((response, content), ()) = timeout(BOUND, async { tokio::join!(client, server) })
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "text/plain");
        assert!(response.headers().get("link").is_none());
        assert_eq!(content, b"ab");
    }

    /// A stream window of one byte has the server's streams come a byte at
    /// a time: every frame start, field section and payload is read in
    /// pieces.
    #[tokio::test]
    async fn a_response_that_comes_a_byte_at_a_time() {
        let transport = Transport {
            stream_window: 1,
            ..Transport::STANDARD
        };
        let (_endpoint, quic, server_quic) = connected_with(&transport, |_| {}).await;
        let client = Client::new(quic);
        let server = H3Server::start(server_quic, SERVER_SETTINGS).await.unwrap();
        // a field section and a payload too long for a one-byte length
        let long = "v".repeat(100);
        let request = async {
            let mut stream = client.request(get("https://example.test/")).await.unwrap();
            stream.shutdown().await.unwrap();
            let response = stream.response().await.unwrap();
            let mut content = Vec::new();
            stream.read_to_end(&mut content).await.unwrap();
            (response, content)
        };
        let serve = async {
            let mut stream = server.accept().await.unwrap();
            stream.read_headers().await.unwrap();
            stream.send_frame(0x21, b"reserved").await.unwrap();
            stream.send_headers(103, &[]).await.unwrap();
            stream
                .send_headers(200, &[("x-long", &long)])
                .await
                .unwrap();
            stream.send_data(&[7; 300]).await.unwrap();
            stream.send_frame(0x01, &[0x00, 0x00]).await.unwrap();
            stream.finish();
        };
        let ((response, content), ()) = timeout(BOUND, async { tokio::join!(request, serve) })
            .await
            .unwrap();
        assert_eq!(response.headers()["x-long"], long.as_str());
        assert_eq!(content, [7; 300]);
        let settings = timeout(BOUND, client.settings()).await.unwrap().unwrap();
        assert_eq!(settings.get(SETTINGS_MAX_FIELD_SECTION_SIZE), Some(1 << 16));
    }

    /// RFC 9204 B.4's field section refers to a dynamic table this client
    /// never allowed.
    #[tokio::test]
    async fn a_dynamic_table_reference_closes_the_connection() {
        let p = pair().await;
        let (response, _served) = answered_with(&p, &[0x05, 0x00, 0x80, 0xc1, 0x81]).await;
        let err = response.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            err.to_string(),
            "h3: the server's field section cannot be decoded"
        );
        assert_eq!(
            closed_with(&p.server_quic).await,
            u64::from(QPACK_DECOMPRESSION_FAILED)
        );
        let err = p
            .client
            .request(get("https://example.test/"))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "h3: the server's field section cannot be decoded"
        );
    }

    /// A malformed response ends its own stream (H3_MESSAGE_ERROR); the
    /// connection serves on.
    #[tokio::test]
    async fn a_malformed_response_ends_its_stream_alone() {
        let p = pair().await;
        for fields in [
            &[("content-type", "text/plain")][..],
            &[("x", "1"), (":status", "200")],
            &[(":status", "200"), ("X-Upper", "1")],
            &[(":status", "200"), ("connection", "close")],
        ] {
            let section = qpack::encode(
                fields
                    .iter()
                    .map(|(n, v)| (n.as_bytes(), v.as_bytes(), false)),
            );
            let (response, served) = answered_with(&p, &section).await;
            let err = response.unwrap_err();
            assert_eq!(
                err.to_string(),
                "h3: the server sent a malformed response",
                "{fields:?}"
            );
            assert_eq!(
                timeout(BOUND, served.client_stopped()).await.unwrap(),
                Some(u64::from(H3_MESSAGE_ERROR))
            );
        }
        assert_eq!(round_trip(&p.client, &p.server).await.unwrap(), 204);
    }

    /// A field section of 64 KiB is taken; one byte more, or a HEADERS
    /// frame longer than that, ends the stream (H3_EXCESSIVE_LOAD).
    #[tokio::test]
    async fn an_oversized_response_head_ends_its_stream_alone() {
        let p = pair().await;
        // :status 200 counts 42 bytes, x-big 37 and its value
        let fits = "a".repeat(65_536 - 42 - 37);
        let (response, _served) =
            answered_with(&p, &response_section(200, &[("x-big", &fits)])).await;
        assert_eq!(response.unwrap().headers()["x-big"].len(), fits.len());
        let over = format!("{fits}a");
        for section in [response_section(200, &[("x-big", &over)]), vec![0; 65_537]] {
            let (response, served) = answered_with(&p, &section).await;
            assert_eq!(
                response.unwrap_err().to_string(),
                "h3: the response head is larger than 65536 bytes"
            );
            assert_eq!(
                timeout(BOUND, served.client_stopped()).await.unwrap(),
                Some(u64::from(H3_EXCESSIVE_LOAD))
            );
        }
        assert_eq!(round_trip(&p.client, &p.server).await.unwrap(), 204);
    }

    /// Frames as the fake sends them: type and payload.
    type Frames = &'static [(u64, &'static [u8])];

    #[tokio::test]
    async fn frames_out_of_place_close_the_connection() {
        let cases: [(&str, Frames, VarInt); 6] = [
            ("DATA before the head", &[(0x00, b"x")], H3_FRAME_UNEXPECTED),
            (
                "SETTINGS on a request stream",
                &[(0x04, b"")],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "a push promised",
                &[(0x05, &[0x00, 0x00, 0x00])],
                H3_ID_ERROR,
            ),
            (
                "HTTP/2's WINDOW_UPDATE",
                &[(0x08, &[0; 4])],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "GOAWAY after the head",
                &[(0x01, OK), (0x07, &[0x00])],
                H3_FRAME_UNEXPECTED,
            ),
            (
                "DATA after the trailers",
                &[(0x01, OK), (0x01, &[0x00, 0x00]), (0x00, b"x")],
                H3_FRAME_UNEXPECTED,
            ),
        ];
        for (case, frames, code) in cases {
            let p = pair().await;
            let request = async {
                let mut stream = p.client.request(get("https://example.test/")).await?;
                stream.shutdown().await?;
                stream.response().await?;
                stream.read_to_end(&mut Vec::new()).await
            };
            let serve = async {
                let mut stream = p.server.accept().await.unwrap();
                stream.read_headers().await.unwrap();
                for (ty, payload) in frames {
                    // a frame after the close finds the stream gone
                    let _ = stream.send_frame(*ty, payload).await;
                }
                stream
            };
            let (result, _served) = timeout(BOUND, async { tokio::join!(request, serve) })
                .await
                .unwrap();
            assert!(result.is_err(), "{case}");
            assert_eq!(closed_with(&p.server_quic).await, u64::from(code), "{case}");
        }
    }

    #[tokio::test]
    async fn a_frame_cut_by_the_end_of_the_stream_closes_the_connection() {
        let p = pair().await;
        let request = async {
            let mut stream = p.client.request(get("https://example.test/")).await?;
            stream.shutdown().await?;
            stream.response().await?;
            let mut content = Vec::new();
            stream.read_to_end(&mut content).await.map(|_| content)
        };
        let serve = async {
            let mut stream = p.server.accept().await.unwrap();
            stream.read_headers().await.unwrap();
            stream.send_frame(0x01, OK).await.unwrap();
            // DATA of 10 bytes, 3 of which come before the end
            stream
                .write_all(&[0x00, 0x0a, b'a', b'b', b'c'])
                .await
                .unwrap();
            stream.finish();
            stream
        };
        let (result, _served) = timeout(BOUND, async { tokio::join!(request, serve) })
            .await
            .unwrap();
        assert_eq!(
            result.unwrap_err().to_string(),
            "h3: the server's stream ended within a frame"
        );
        assert_eq!(closed_with(&p.server_quic).await, u64::from(H3_FRAME_ERROR));
    }

    /// The server takes 2000 bytes of a stream before it reads: a frame
    /// larger than that waits half written, the next write is given up (as
    /// a write that times out is), and what follows comes after the whole
    /// frame — nothing of what was given up.
    #[tokio::test]
    async fn a_write_given_up_mid_frame_is_finished_first() {
        let (_endpoint, quic, server_quic) = connected_with(&Transport::STANDARD, |transport| {
            transport.stream_receive_window(VarInt::from_u32(2000));
        })
        .await;
        let client = Client::new(quic);
        let server = H3Server::start(server_quic, SERVER_SETTINGS).await.unwrap();
        timeout(BOUND, async {
            let mut stream = client.request(tunnel("example.test:443")).await.unwrap();
            let mut served = server.accept().await.unwrap();
            served.read_headers().await.unwrap();
            served.send_headers(200, &[]).await.unwrap();
            assert_eq!(stream.response().await.unwrap().status(), 200);
            assert_eq!(stream.write(&[1; 3000]).await.unwrap(), 3000);
            let mut cx = Context::from_waker(Waker::noop());
            assert!(
                Pin::new(&mut stream)
                    .poll_write(&mut cx, &[2; 100])
                    .is_pending()
            );
            let read = async {
                let mut frames = Vec::new();
                while let Some(frame) = served.read_frame().await.unwrap() {
                    frames.push(frame);
                }
                frames
            };
            let write = async {
                stream.write_all(&[3; 50]).await.unwrap();
                stream.shutdown().await.unwrap();
            };
            let (frames, ()) = tokio::join!(read, write);
            assert_eq!(frames, [(0x00, vec![1; 3000]), (0x00, vec![3; 50])]);
        })
        .await
        .unwrap();
    }

    /// A CONNECT has `:method` and `:authority` alone (RFC 9114 4.4); its
    /// shutdown sends the FIN and the tunnel reads on.
    #[tokio::test]
    async fn a_tunnel_reads_on_after_its_shutdown() {
        let p = pair().await;
        let client = async {
            let mut stream = p.client.request(tunnel("example.test:443")).await.unwrap();
            assert_eq!(stream.response().await.unwrap().status(), 200);
            stream.write_all(b"hello").await.unwrap();
            stream.shutdown().await.unwrap();
            let err = stream.write_all(b"more").await.unwrap_err();
            assert_eq!(err.to_string(), "h3: write after shutdown");
            let mut back = Vec::new();
            stream.read_to_end(&mut back).await.unwrap();
            back
        };
        let server = async {
            let mut stream = p.server.accept().await.unwrap();
            let head = stream.read_headers().await.unwrap();
            stream.send_headers(200, &[]).await.unwrap();
            let content = stream.read_body().await.unwrap();
            stream.send_data(b"world").await.unwrap();
            stream.finish();
            (head, content)
        };
        let (back, (head, content)) = timeout(BOUND, async { tokio::join!(client, server) })
            .await
            .unwrap();
        assert_eq!(
            head,
            owned(&[(":method", "CONNECT"), (":authority", "example.test:443")])
        );
        assert_eq!(content, b"hello");
        assert_eq!(back, b"world");
    }

    #[tokio::test]
    async fn a_request_dropped_is_cancelled_both_ways() {
        let p = pair().await;
        timeout(BOUND, async {
            let stream = p
                .client
                .request(get("https://example.test/"))
                .await
                .unwrap();
            let mut served = p.server.accept().await.unwrap();
            served.read_headers().await.unwrap();
            drop(stream);
            let cancelled = Some(u64::from(H3_REQUEST_CANCELLED));
            assert_eq!(served.client_reset().await, cancelled);
            assert_eq!(served.client_stopped().await, cancelled);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_reset_by_the_server_is_reported_with_its_code() {
        let p = pair().await;
        let request = async {
            let mut stream = p.client.request(get("https://example.test/")).await?;
            stream.shutdown().await?;
            stream.response().await
        };
        let serve = async {
            let mut stream = p.server.accept().await.unwrap();
            stream.read_headers().await.unwrap();
            // H3_REQUEST_REJECTED: not served, may be sent again
            stream.reset(0x10b);
            stream
        };
        let (response, _served) = timeout(BOUND, async { tokio::join!(request, serve) })
            .await
            .unwrap();
        let err = response.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(
            err.to_string(),
            "h3: the server reset the stream (H3_REQUEST_REJECTED)"
        );
    }

    /// Hysteria 2 opens streams of its own on the client's connection; the
    /// fake tells them apart by their first integer.
    #[tokio::test]
    async fn other_protocols_open_streams_beside_the_client() {
        let p = pair().await;
        timeout(BOUND, async {
            let mut request = p
                .client
                .request(get("https://example.test/"))
                .await
                .unwrap();
            request.shutdown().await.unwrap();
            let (mut send, mut recv) = p.quic.open_bi().await.unwrap();
            send.write_all(&[0x44, 0x01, b'h', b'i']).await.unwrap();
            send.finish().unwrap();
            let mut http = p.server.accept().await.unwrap();
            let mut other = p.server.accept().await.unwrap();
            assert_eq!(http.peek_varint().await.unwrap(), 0x01);
            assert_eq!(other.peek_varint().await.unwrap(), 0x401);
            // a peek takes nothing: the stream is read from its start
            let mut start = Vec::new();
            other.read_to_end(&mut start).await.unwrap();
            assert_eq!(start, [0x44, 0x01, b'h', b'i']);
            other.write_all(b"raw").await.unwrap();
            AsyncWriteExt::shutdown(&mut other).await.unwrap();
            assert_eq!(recv.read_to_end(16).await.unwrap(), b"raw");
            http.read_headers().await.unwrap();
            http.send_headers(204, &[]).await.unwrap();
            http.finish();
            assert_eq!(request.response().await.unwrap().status(), 204);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_lost_connection_fails_the_wait_and_new_requests() {
        let (_endpoint, quic, server_quic) = connected().await;
        let client = Client::new(quic);
        let _server = H3Server::new(server_quic.clone());
        server_quic.close(VarInt::from_u32(0x100), b"bye");
        let closed = "quic: the server closed the connection (code 0x100: bye)";
        let err = timeout(BOUND, client.settings())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err.to_string(), closed);
        let err = client
            .request(get("https://example.test/"))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), closed);
    }

    /// The client's task holds its control stream: the stream ends with
    /// the client.
    #[tokio::test]
    async fn the_clients_task_ends_with_it() {
        let p = pair().await;
        timeout(BOUND, p.server.client_settings())
            .await
            .unwrap()
            .unwrap();
        assert!(p.server.client_streams_ended().is_empty());
        drop(p.client);
        eventually(|| p.server.client_streams_ended() == [CONTROL_STREAM]).await;
    }

    /// The error codes as the texts name them.
    #[test]
    fn codes_have_their_names() {
        assert_eq!(code_name(H3_REQUEST_CANCELLED), "H3_REQUEST_CANCELLED");
        assert_eq!(
            code_name(QPACK_DECOMPRESSION_FAILED),
            "QPACK_DECOMPRESSION_FAILED"
        );
        assert_eq!(code_name(VarInt::from_u32(0x1234)), "0x1234");
    }
}
