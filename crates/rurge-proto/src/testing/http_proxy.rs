//! A scriptable HTTP proxy: CONNECT tunnels and absolute-form requests.

use super::{AbortOnDrop, TlsFixture};
use crate::transport::head::read_head;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rurge_net::connector::BoxedStream;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug, Default)]
pub struct HttpProxyScript {
    /// Require these Basic proxy credentials (user, password).
    pub auth: Option<(String, String)>,
    /// Answer every request with this status instead of serving it.
    pub refuse: Option<(u16, &'static str)>,
    /// Pad the CONNECT response head with a header of this many bytes.
    pub padding: usize,
    /// Wait this long before answering.
    pub delay: Duration,
    /// Bytes written right behind the response head of a CONNECT.
    pub trailing: Vec<u8>,
    /// Close the connection in the middle of the CONNECT response head.
    pub truncate: bool,
    /// Tunnel here whatever the client asked for (the fake never resolves names).
    pub connect_to: Option<SocketAddr>,
}

#[derive(Clone, Debug)]
pub struct RecordedHead {
    pub request_line: String,
    /// Names as the client wrote them.
    pub headers: Vec<(String, String)>,
}

impl RecordedHead {
    /// Case-insensitive lookup of the first header called `name`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub struct FakeHttpProxy {
    addr: SocketAddr,
    heads: Arc<Mutex<Vec<RecordedHead>>>,
    _task: AbortOnDrop,
}

fn parse_head(head: &[u8]) -> RecordedHead {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        .collect();
    RecordedHead {
        request_line,
        headers,
    }
}

async fn respond(
    stream: &mut BoxedStream,
    code: u16,
    reason: &str,
    extra: &str,
    body: &[u8],
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.shutdown().await
}

async fn serve(
    mut stream: BoxedStream,
    script: HttpProxyScript,
    heads: Arc<Mutex<Vec<RecordedHead>>>,
) -> io::Result<()> {
    let (head, rest) = read_head(&mut stream, 64 * 1024).await?;
    let recorded = parse_head(&head);
    heads.lock().expect("heads").push(recorded.clone());
    tokio::time::sleep(script.delay).await;
    if let Some((user, password)) = &script.auth {
        let expected = format!("Basic {}", STANDARD.encode(format!("{user}:{password}")));
        if recorded.header("Proxy-Authorization") != Some(expected.as_str()) {
            return respond(
                &mut stream,
                407,
                "Proxy Authentication Required",
                "Proxy-Authenticate: Basic realm=\"fake\"\r\n",
                b"",
            )
            .await;
        }
    }
    if let Some((code, reason)) = script.refuse {
        return respond(&mut stream, code, reason, "", b"").await;
    }
    let mut parts = recorded.request_line.split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method != "CONNECT" {
        // absolute-form request: answered here, tests only inspect what arrived
        return respond(&mut stream, 200, "OK", "", b"forwarded").await;
    }
    let Some(upstream_addr) = script.connect_to.or_else(|| target.parse().ok()) else {
        return respond(&mut stream, 502, "Bad Gateway", "", b"").await;
    };
    let Ok(mut upstream) = TcpStream::connect(upstream_addr).await else {
        return respond(&mut stream, 502, "Bad Gateway", "", b"").await;
    };
    let mut response = String::from("HTTP/1.1 200 Connection established\r\n");
    if script.padding > 0 {
        response.push_str(&format!("X-Padding: {}\r\n", "p".repeat(script.padding)));
    }
    response.push_str("\r\n");
    if script.truncate {
        stream
            .write_all(&response.as_bytes()[..response.len() / 2])
            .await?;
        return stream.shutdown().await;
    }
    stream.write_all(response.as_bytes()).await?;
    stream.write_all(&script.trailing).await?;
    upstream.write_all(&rest).await?;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
    Ok(())
}

impl FakeHttpProxy {
    pub async fn spawn(script: HttpProxyScript) -> FakeHttpProxy {
        FakeHttpProxy::start(script, None).await
    }

    /// The same proxy behind TLS (`https` policies).
    pub async fn spawn_tls(
        script: HttpProxyScript,
        fixture: Arc<TlsFixture>,
        require_client_cert: bool,
    ) -> FakeHttpProxy {
        FakeHttpProxy::start(script, Some((fixture, require_client_cert))).await
    }

    async fn start(script: HttpProxyScript, tls: Option<(Arc<TlsFixture>, bool)>) -> FakeHttpProxy {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let heads: Arc<Mutex<Vec<RecordedHead>>> = Arc::default();
        let log = heads.clone();
        let tls = tls.map(|(fixture, require)| {
            let acceptor = fixture.acceptor(require);
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
        FakeHttpProxy {
            addr,
            heads,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Every request head seen so far, in arrival order.
    pub fn heads(&self) -> Vec<RecordedHead> {
        self.heads.lock().expect("heads").clone()
    }
}
