//! WebSocket as a byte stream (`ws=true`): a client handshake on top of any
//! stream, then binary frames in both directions.

use crate::{BuildError, OutboundError};
use bytes::{Buf, Bytes};
use futures_util::{Sink, Stream};
use http::header::{HeaderName, HeaderValue};
use rurge_config::spec::WsOpts;
use rurge_net::connector::{BoxedStream, Target};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// Largest frame and largest message accepted from the peer.
pub const MAX_INCOMING: usize = 1 << 20;
/// Largest payload put into one outgoing frame.
const MAX_OUTGOING: usize = 64 * 1024;

/// tungstenite's own error texts can quote header values, so none of them is
/// ever passed on: every variant maps onto a fixed text.
fn ws_io(e: WsError) -> io::Error {
    match e {
        WsError::Io(e) => e,
        WsError::ConnectionClosed | WsError::AlreadyClosed => {
            io::Error::new(io::ErrorKind::BrokenPipe, "ws: the connection is closed")
        }
        WsError::Capacity(_) => io::Error::new(
            io::ErrorKind::InvalidData,
            "ws: the server sent a frame larger than the limit",
        ),
        _ => io::Error::new(io::ErrorKind::InvalidData, "ws: protocol error"),
    }
}

/// Bytes over an established WebSocket, for either role.
pub struct WsByteStream {
    inner: WebSocketStream<BoxedStream>,
    /// What is left of the binary message being read.
    pending: Bytes,
    eof: bool,
}

impl WsByteStream {
    pub(crate) fn new(inner: WebSocketStream<BoxedStream>) -> WsByteStream {
        WsByteStream {
            inner,
            pending: Bytes::new(),
            eof: false,
        }
    }
}

impl AsyncRead for WsByteStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if !self.pending.is_empty() {
                let n = self.pending.len().min(buf.remaining());
                buf.put_slice(&self.pending[..n]);
                self.pending.advance(n);
                return Poll::Ready(Ok(()));
            }
            if self.eof {
                return Poll::Ready(Ok(()));
            }
            match ready!(Pin::new(&mut self.inner).poll_next(cx)) {
                Some(Ok(Message::Binary(data))) => self.pending = data,
                // the library answers a ping on its own; nothing here is payload
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Text(_))) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "ws: the server sent a text frame",
                    )));
                }
                Some(Ok(Message::Close(_))) | None => self.eof = true,
                Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed)) => self.eof = true,
                Some(Err(e)) => return Poll::Ready(Err(ws_io(e))),
            }
        }
    }
}

impl AsyncWrite for WsByteStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        ready!(Pin::new(&mut self.inner).poll_ready(cx)).map_err(ws_io)?;
        let n = data.len().min(MAX_OUTGOING);
        Pin::new(&mut self.inner)
            .start_send(Message::Binary(Bytes::copy_from_slice(&data[..n])))
            .map_err(ws_io)?;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx).map_err(ws_io)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // sends Close once and flushes; a peer that is already gone is fine
        match ready!(Pin::new(&mut self.inner).poll_close(cx)) {
            Ok(()) | Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(ws_io(e))),
        }
    }
}

/// Headers the handshake writes itself; a caller-supplied one would make
/// tungstenite refuse the request as a duplicate.
fn is_managed(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "connection" || lower == "upgrade" || lower.starts_with("sec-websocket-")
}

/// Everything that can be prepared ahead of a connection.
pub struct WsClient {
    /// `ws://<host><path>`: tungstenite insists on the scheme; only the path
    /// and query reach the wire.
    uri: http::Uri,
    host: String,
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl WsClient {
    /// `server` is the proxy's own address; `tls` tells which default port
    /// the layer below has (443 with TLS, 80 without). No error text quotes a
    /// path, a header value or a host: all three are used as shared secrets.
    pub fn new(opts: &WsOpts, server: &Target, tls: bool) -> Result<WsClient, BuildError> {
        let mut host = None;
        let mut headers = Vec::new();
        for (n, (name, value)) in opts.headers.iter().enumerate() {
            if name.eq_ignore_ascii_case("host") {
                host = Some(value.clone());
                continue;
            }
            if is_managed(name) {
                return Err(BuildError::new(format!(
                    "`ws-headers` entry #{} is a header of the WebSocket handshake itself",
                    n + 1
                )));
            }
            let (Ok(name), Ok(value)) = (
                HeaderName::try_from(name.as_str()),
                HeaderValue::from_str(value),
            ) else {
                return Err(BuildError::new(format!(
                    "`ws-headers` entry #{} cannot be sent",
                    n + 1
                )));
            };
            headers.push((name, value));
        }
        let host = match host {
            Some(host) => host,
            None => {
                let name = crate::http::wire_host(server).ok_or_else(|| {
                    BuildError::new(
                        "the server's host name cannot be written into a WebSocket request",
                    )
                })?;
                let default_port = if tls { 443 } else { 80 };
                if server.port == default_port {
                    name
                } else {
                    format!("{name}:{}", server.port)
                }
            }
        };
        let uri = format!("ws://{host}{}", opts.path)
            .parse::<http::Uri>()
            .map_err(|_| {
                BuildError::new("`ws-path` and the `Host` header do not form a valid request URI")
            })?;
        Ok(WsClient { uri, host, headers })
    }

    pub async fn wrap(&self, stream: BoxedStream) -> Result<BoxedStream, OutboundError> {
        let mut request = http::Request::builder()
            .method("GET")
            .uri(self.uri.clone())
            .header("Host", self.host.as_str())
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key());
        for (name, value) in &self.headers {
            request = request.header(name.clone(), value.clone());
        }
        let request = request
            .body(())
            .map_err(|_| OutboundError::Proxy("ws: the request cannot be built".to_string()))?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_INCOMING))
            .max_frame_size(Some(MAX_INCOMING));
        match tokio_tungstenite::client_async_with_config(request, stream, Some(config)).await {
            Ok((socket, _response)) => Ok(Box::new(WsByteStream::new(socket))),
            Err(WsError::Http(response)) => Err(OutboundError::Proxy(format!(
                "ws: handshake failed: HTTP {}",
                response.status().as_u16()
            ))),
            Err(WsError::Io(e)) => Err(OutboundError::from(e)),
            // never the library's text (see `ws_io`)
            Err(_) => Err(OutboundError::Proxy("ws: handshake failed".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeWs, WsScript};
    use rurge_config::HostName;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    fn opts(path: &str, headers: &[(&str, &str)]) -> WsOpts {
        WsOpts {
            path: path.to_string(),
            headers: headers
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn server(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn open(fake: &FakeWs, client: &WsClient) -> Result<BoxedStream, OutboundError> {
        let tcp = TcpStream::connect(fake.addr()).await.unwrap();
        client.wrap(Box::new(tcp)).await
    }

    #[tokio::test]
    async fn bytes_cross_in_both_directions_whatever_the_slicing() {
        let fake = FakeWs::spawn(WsScript::default()).await;
        let client = WsClient::new(&opts("/", &[]), &server(fake.addr()), false).unwrap();
        // (payload length, bytes per write, bytes per read): one-byte frames,
        // odd sizes, and writes larger than one outgoing frame; the echo
        // comes back in frames of at most 1 KiB
        for (len, write_chunk, read_chunk) in [
            (4_000usize, 1usize, 3usize),
            (50_000, 1_000, 777),
            (200_000, 70_000, 4_096),
        ] {
            let stream = open(&fake, &client).await.unwrap();
            let payload: Vec<u8> = (0..len as u32).map(|i| (i % 251) as u8).collect();
            let (mut rd, mut wr) = tokio::io::split(stream);
            let to_send = payload.clone();
            let writer = tokio::spawn(async move {
                for chunk in to_send.chunks(write_chunk) {
                    wr.write_all(chunk).await.unwrap();
                }
                wr.flush().await.unwrap();
                wr
            });
            let mut back = vec![0u8; payload.len()];
            let mut got = 0;
            while got < back.len() {
                let end = (got + read_chunk).min(back.len());
                let n = rd.read(&mut back[got..end]).await.unwrap();
                assert!(n > 0, "EOF after {got} of {len} bytes");
                got += n;
            }
            assert_eq!(back, payload, "{len} / {write_chunk} / {read_chunk}");
            let mut stream = rd.unsplit(writer.await.unwrap());
            stream.shutdown().await.unwrap();
            let mut rest = Vec::new();
            stream.read_to_end(&mut rest).await.unwrap();
            assert!(rest.is_empty(), "the peer's Close is an EOF");
        }
    }

    #[tokio::test]
    async fn the_request_carries_the_path_the_host_and_the_extra_headers() {
        let fake = FakeWs::spawn(WsScript::default()).await;
        let custom = WsClient::new(
            &opts("/ray?ed=1", &[("Host", "edge.example"), ("X-Key", "v 1")]),
            &server(fake.addr()),
            false,
        )
        .unwrap();
        drop(open(&fake, &custom).await.unwrap());
        let default = WsClient::new(&opts("/", &[]), &server(fake.addr()), false).unwrap();
        drop(open(&fake, &default).await.unwrap());
        let seen = fake.seen();
        assert_eq!(seen[0].path, "/ray?ed=1");
        assert_eq!(seen[0].header("host"), Some("edge.example"));
        assert_eq!(seen[0].header("x-key"), Some("v 1"));
        assert_eq!(seen[0].header("upgrade"), Some("websocket"));
        assert_eq!(seen[1].path, "/");
        // not the layer's default port (80 without TLS): the port is written
        assert_eq!(
            seen[1].header("host"),
            Some(format!("127.0.0.1:{}", fake.addr().port()).as_str())
        );
    }

    #[test]
    fn the_default_host_follows_the_servers_name_and_the_layers_port() {
        let host = |name: &str, port: u16, tls: bool| {
            WsClient::new(
                &opts("/", &[]),
                &Target::new(HostName::parse(name), port),
                tls,
            )
            .unwrap()
            .host
        };
        assert_eq!(host("edge.example", 443, true), "edge.example");
        assert_eq!(host("edge.example", 80, false), "edge.example");
        assert_eq!(host("edge.example", 8443, true), "edge.example:8443");
        assert_eq!(host("bücher.example", 443, true), "xn--bcher-kva.example");
        assert_eq!(host("2001:db8::1", 443, true), "[2001:db8::1]");
        assert_eq!(host("2001:db8::1", 8080, true), "[2001:db8::1]:8080");
    }

    #[test]
    fn what_cannot_be_sent_is_a_build_error_that_quotes_nothing() {
        let target = Target::new(HostName::parse("edge.example"), 443);
        for (o, expected) in [
            (
                opts("/ok", &[("Host", "bad host")]),
                "`ws-path` and the `Host` header do not form a valid request URI",
            ),
            (
                opts("/ok", &[("X-A", "line\nbreak")]),
                "`ws-headers` entry #1 cannot be sent",
            ),
            (
                opts("/ok", &[("X-A", "1"), ("Upgrade", "h2c")]),
                "`ws-headers` entry #2 is a header of the WebSocket handshake itself",
            ),
        ] {
            let err = WsClient::new(&o, &target, true).err().expect("refused");
            assert_eq!(err.message, expected);
        }
        let err = WsClient::new(
            &opts("/", &[]),
            &Target::new(HostName::Domain("a@b.test".into()), 443),
            true,
        )
        .err()
        .expect("refused");
        assert_eq!(
            err.message,
            "the server's host name cannot be written into a WebSocket request"
        );
    }

    #[tokio::test]
    async fn a_refused_handshake_names_the_status_code_only() {
        let fake = FakeWs::spawn(WsScript {
            refuse: Some(403),
            ..WsScript::default()
        })
        .await;
        let client = WsClient::new(&opts("/", &[]), &server(fake.addr()), false).unwrap();
        let err = open(&fake, &client).await.err().expect("refused");
        assert!(
            matches!(&err, OutboundError::Proxy(m) if m == "ws: handshake failed: HTTP 403"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_text_frame_and_an_oversized_frame_end_the_stream_with_an_error() {
        for (script, expected) in [
            (
                WsScript {
                    send_text: true,
                    ..WsScript::default()
                },
                "ws: the server sent a text frame",
            ),
            (
                WsScript {
                    send_frame: Some(MAX_INCOMING + 1),
                    ..WsScript::default()
                },
                "ws: the server sent a frame larger than the limit",
            ),
        ] {
            let fake = FakeWs::spawn(script).await;
            let client = WsClient::new(&opts("/", &[]), &server(fake.addr()), false).unwrap();
            let mut stream = open(&fake, &client).await.unwrap();
            let mut buf = [0u8; 16];
            let err =
                tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf))
                    .await
                    .expect("the frame arrives")
                    .expect_err("an error, not data");
            assert_eq!(err.to_string(), expected);
        }
    }
}
