//! A scriptable WebSocket acceptor: records each handshake, then echoes
//! bytes (or misbehaves as scripted).

use super::{AbortOnDrop, TlsFixture};
use crate::transport::ws::WsByteStream;
use bytes::Bytes;
use futures_util::SinkExt;
use rurge_net::connector::BoxedStream;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

#[derive(Clone, Debug, Default)]
pub struct WsScript {
    /// Refuse the handshake with this HTTP status.
    pub refuse: Option<u16>,
    /// After the handshake, send one text frame.
    pub send_text: bool,
    /// After the handshake, send one binary frame of this many bytes.
    pub send_frame: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct RecordedWs {
    /// Path and query as the client wrote them.
    pub path: String,
    /// Names in lower case (as the `http` crate stores them).
    pub headers: Vec<(String, String)>,
}

impl RecordedWs {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub struct FakeWs {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<RecordedWs>>>,
    _task: AbortOnDrop,
}

async fn accept_socket(
    stream: BoxedStream,
    seen: &Mutex<Vec<RecordedWs>>,
    refuse: Option<u16>,
) -> io::Result<tokio_tungstenite::WebSocketStream<BoxedStream>> {
    let callback = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let headers = req
            .headers()
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let path = req
            .uri()
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        seen.lock()
            .expect("seen")
            .push(RecordedWs { path, headers });
        match refuse {
            None => Ok(resp),
            Some(code) => {
                let mut no = ErrorResponse::new(Some("refused by the script".to_string()));
                *no.status_mut() = http::StatusCode::from_u16(code).expect("a status code");
                Err(no)
            }
        }
    };
    tokio_tungstenite::accept_hdr_async(stream, callback)
        .await
        // tungstenite's Display can quote header values (P3): a fixed text,
        // never `e.to_string()`.
        .map_err(|_| io::Error::other("the WebSocket handshake failed"))
}

/// The server side of a WebSocket handshake on `stream`, as a byte stream.
/// `FakeTrojan` puts its protocol on top of this.
pub async fn accept_bytes(
    stream: BoxedStream,
    seen: &Mutex<Vec<RecordedWs>>,
) -> io::Result<BoxedStream> {
    Ok(Box::new(WsByteStream::new(
        accept_socket(stream, seen, None).await?,
    )))
}

async fn serve(
    stream: BoxedStream,
    script: WsScript,
    seen: Arc<Mutex<Vec<RecordedWs>>>,
) -> io::Result<()> {
    let mut socket = accept_socket(stream, &seen, script.refuse).await?;
    if script.send_text {
        let _ = socket.send(Message::text("not binary")).await;
    }
    if let Some(len) = script.send_frame {
        let _ = socket
            .send(Message::Binary(Bytes::from(vec![7u8; len])))
            .await;
    }
    let mut bytes = WsByteStream::new(socket);
    let mut buf = [0u8; 1024];
    loop {
        let n = bytes.read(&mut buf).await?;
        if n == 0 {
            return bytes.shutdown().await;
        }
        // no explicit flush: `write_all` alone must deliver
        bytes.write_all(&buf[..n]).await?;
    }
}

impl FakeWs {
    pub async fn spawn(script: WsScript) -> FakeWs {
        FakeWs::start(script, None).await
    }

    /// The same acceptor behind TLS.
    pub async fn spawn_tls(script: WsScript, fixture: Arc<TlsFixture>) -> FakeWs {
        FakeWs::start(script, Some(fixture)).await
    }

    async fn start(script: WsScript, tls: Option<Arc<TlsFixture>>) -> FakeWs {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let seen: Arc<Mutex<Vec<RecordedWs>>> = Arc::default();
        let log = seen.clone();
        let tls = tls.map(|fixture| {
            let acceptor = fixture.acceptor(false);
            (fixture, acceptor)
        });
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (script, log, tls) = (script.clone(), log.clone(), tls.clone());
                tokio::spawn(async move {
                    let stream: BoxedStream = match &tls {
                        Some((fixture, acceptor)) => match fixture.accept(acceptor, tcp).await {
                            Ok(s) => s,
                            Err(_) => return,
                        },
                        None => Box::new(tcp),
                    };
                    let _ = serve(stream, script, log).await;
                });
            }
        });
        FakeWs {
            addr,
            seen,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Every handshake seen so far, in arrival order.
    pub fn seen(&self) -> Vec<RecordedWs> {
        self.seen.lock().expect("seen").clone()
    }
}
