//! DNS over TCP and over TLS (design §7.2): one persistent connection per
//! upstream, RFC 1035 §4.2.2 two-byte length framing, reconnect after an error.

use super::{Upstream, UpstreamError};
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_rustls::TlsConnector;

pub const MAX_MESSAGE: usize = 65_535;

/// Writes one length-prefixed message and reads one length-prefixed reply.
pub async fn exchange_framed<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    wire: &[u8],
    deadline: Instant,
) -> Result<Vec<u8>, UpstreamError> {
    if wire.len() > MAX_MESSAGE {
        return Err(UpstreamError::BadResponse(
            "query larger than 65535 bytes".to_string(),
        ));
    }
    let mut buf = Vec::with_capacity(wire.len() + 2);
    buf.extend_from_slice(&(wire.len() as u16).to_be_bytes());
    buf.extend_from_slice(wire);
    let io = async {
        stream.write_all(&buf).await?;
        let mut len = [0u8; 2];
        stream.read_exact(&mut len).await?;
        let n = usize::from(u16::from_be_bytes(len));
        let mut resp = vec![0u8; n];
        stream.read_exact(&mut resp).await?;
        Ok::<Vec<u8>, std::io::Error>(resp)
    };
    match tokio::time::timeout_at(deadline, io).await {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(UpstreamError::Io(e.to_string())),
        Err(_) => Err(UpstreamError::Timeout),
    }
}

pub struct TcpUpstream {
    name: String,
    host: String,
    port: u16,
    tls: Option<Arc<ClientConfig>>,
    connector: Arc<dyn Connector>,
    conn: Mutex<Option<BoxedStream>>,
}

impl TcpUpstream {
    pub fn plain(host: &str, port: u16, connector: Arc<dyn Connector>) -> TcpUpstream {
        TcpUpstream {
            name: format!("tcp://{host}:{port}"),
            host: host.to_string(),
            port,
            tls: None,
            connector,
            conn: Mutex::new(None),
        }
    }

    pub fn tls(
        host: &str,
        port: u16,
        connector: Arc<dyn Connector>,
        config: Arc<ClientConfig>,
    ) -> TcpUpstream {
        TcpUpstream {
            name: format!("tls://{host}:{port}"),
            host: host.to_string(),
            port,
            tls: Some(config),
            connector,
            conn: Mutex::new(None),
        }
    }

    async fn connect(&self, deadline: Instant) -> Result<BoxedStream, UpstreamError> {
        let timeout = deadline.saturating_duration_since(Instant::now());
        let target = Target::new(HostName::parse(&self.host), self.port);
        let opts = ConnectOpts {
            timeout,
            prefer_v6: false,
        };
        let stream = self
            .connector
            .connect(&target, &opts)
            .await
            .map_err(|e| UpstreamError::Io(e.to_string()))?;
        let Some(config) = &self.tls else {
            return Ok(stream);
        };
        let name = ServerName::try_from(self.host.clone())
            .map_err(|e| UpstreamError::Tls(e.to_string()))?;
        let handshake = TlsConnector::from(config.clone()).connect(name, stream);
        match tokio::time::timeout_at(deadline, handshake).await {
            Ok(Ok(tls)) => Ok(Box::new(tls) as BoxedStream),
            Ok(Err(e)) => Err(UpstreamError::Tls(e.to_string())),
            Err(_) => Err(UpstreamError::Timeout),
        }
    }
}

impl Upstream for TcpUpstream {
    fn name(&self) -> &str {
        &self.name
    }

    fn query<'a>(
        &'a self,
        wire: &'a [u8],
        deadline: Instant,
    ) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>> {
        Box::pin(async move {
            // One exchange at a time per connection; concurrency comes from
            // querying several upstreams at once.
            let mut guard = self.conn.lock().await;
            if guard.is_none() {
                *guard = Some(self.connect(deadline).await?);
            }
            let stream = guard.as_mut().expect("connection present");
            match exchange_framed(stream, wire, deadline).await {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    *guard = None;
                    Err(e)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::net::TcpListener;

    /// Framed echo server: replies with the request bytes, `replies` times per connection.
    async fn framed_echo(replies: usize) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    for _ in 0..replies {
                        let mut len = [0u8; 2];
                        if s.read_exact(&mut len).await.is_err() {
                            return;
                        }
                        let n = usize::from(u16::from_be_bytes(len));
                        let mut msg = vec![0u8; n];
                        if s.read_exact(&mut msg).await.is_err() {
                            return;
                        }
                        let mut out = len.to_vec();
                        out.extend_from_slice(&msg);
                        if s.write_all(&out).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        addr
    }

    fn connector() -> Arc<dyn Connector> {
        Arc::new(DirectConnector::new(Arc::new(SystemResolve)))
    }

    fn deadline(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    #[tokio::test]
    async fn framing_round_trip_over_duplex() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let server = tokio::spawn(async move {
            let mut len = [0u8; 2];
            b.read_exact(&mut len).await.unwrap();
            let mut msg = vec![0u8; usize::from(u16::from_be_bytes(len))];
            b.read_exact(&mut msg).await.unwrap();
            assert_eq!(msg, b"hello");
            b.write_all(&[0, 3, b'a', b'b', b'c']).await.unwrap();
        });
        let resp = exchange_framed(&mut a, b"hello", deadline(1000))
            .await
            .unwrap();
        assert_eq!(resp, b"abc");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn plain_tcp_reuses_the_connection_and_reconnects_after_close() {
        let addr = framed_echo(2).await;
        let up = TcpUpstream::plain("127.0.0.1", addr.port(), connector());
        assert_eq!(up.name(), format!("tcp://127.0.0.1:{}", addr.port()));
        assert_eq!(
            up.query(b"\x12\x34one", deadline(1000)).await.unwrap(),
            b"\x12\x34one"
        );
        assert_eq!(
            up.query(b"\x12\x34two", deadline(1000)).await.unwrap(),
            b"\x12\x34two"
        );
        // The server closes after two replies: the third query fails once, then reconnects.
        let third = up.query(b"\x12\x34three", deadline(1000)).await;
        assert!(third.is_err() || third.as_deref() == Ok(b"\x12\x34three"));
        assert_eq!(
            up.query(b"\x12\x34four", deadline(1000)).await.unwrap(),
            b"\x12\x34four"
        );
    }

    #[tokio::test]
    async fn timeout_when_the_server_never_answers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (_s, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let up = TcpUpstream::plain("127.0.0.1", addr.port(), connector());
        assert_eq!(
            up.query(b"\x00\x01x", deadline(200)).await,
            Err(UpstreamError::Timeout)
        );
    }

    #[tokio::test]
    async fn dot_over_a_self_signed_server() {
        let rcgen::CertifiedKey { cert, signing_key } = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
        ])
        .unwrap();
        let cert_der = cert.der().clone();
        let key_der: rustls::pki_types::PrivateKeyDer<'static> = signing_key.into();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let server_config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let mut tls = acceptor.accept(tcp).await.unwrap();
                    let mut len = [0u8; 2];
                    tls.read_exact(&mut len).await.unwrap();
                    let mut msg = vec![0u8; usize::from(u16::from_be_bytes(len))];
                    tls.read_exact(&mut msg).await.unwrap();
                    let mut out = len.to_vec();
                    out.extend_from_slice(&msg);
                    tls.write_all(&out).await.unwrap();
                });
            }
        });
        let config = rurge_net::http::tls_client_config(true).unwrap();
        let up = TcpUpstream::tls("127.0.0.1", addr.port(), connector(), config);
        assert_eq!(up.name(), format!("tls://127.0.0.1:{}", addr.port()));
        assert_eq!(
            up.query(b"\xab\xcdsecure", deadline(2000)).await.unwrap(),
            b"\xab\xcdsecure"
        );
        let strict = rurge_net::http::tls_client_config(false).unwrap();
        let strict_up = TcpUpstream::tls("127.0.0.1", addr.port(), connector(), strict);
        assert!(matches!(
            strict_up.query(b"\x00\x02x", deadline(2000)).await,
            Err(UpstreamError::Tls(_))
        ));
    }
}
