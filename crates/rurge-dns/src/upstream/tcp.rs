//! DNS over TCP and over TLS (design §7.2): one persistent connection per
//! upstream, RFC 1035 §4.2.2 two-byte length framing, reconnect after an error.

use super::{Upstream, UpstreamError};
use crate::message::wire_id;
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
///
/// The reply's DNS transaction id is checked against the query's: a mismatch,
/// or a reply too short to carry a header at all, is reported as
/// `UpstreamError::BadResponse` so the caller drops the connection.
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
    let sent_id = wire_id(wire);
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
    let resp = match tokio::time::timeout_at(deadline, io).await {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => return Err(UpstreamError::Io(e.to_string())),
        Err(_) => return Err(UpstreamError::Timeout),
    };
    let got_id = wire_id(&resp);
    if got_id.is_none() || got_id != sent_id {
        return Err(UpstreamError::BadResponse(format!(
            "response id mismatch: sent {sent_id:?}, got {got_id:?}"
        )));
    }
    Ok(resp)
}

/// Derives a DoT client config from the shared HTTP TLS config: DoT (RFC 8310
/// §8.1) requires ALPN `dot`, whereas the shared config advertises `h2` /
/// `http/1.1` for the HTTP client, which an ALPN-strict DoT server would
/// reject with `no_application_protocol`.
fn dot_config(base: &ClientConfig) -> Arc<ClientConfig> {
    let mut cfg = base.clone();
    cfg.alpn_protocols = vec![b"dot".to_vec()];
    Arc::new(cfg)
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
            tls: Some(dot_config(&config)),
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
        let connecting = self.connector.connect(&target, &opts);
        let stream = match tokio::time::timeout_at(deadline, connecting).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => return Err(UpstreamError::Io(e.to_string())),
            Err(_) => return Err(UpstreamError::Timeout),
        };
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
            // querying several upstreams at once. The wait for the lock is
            // itself bounded, so a query queued behind a slow one cannot run
            // past its own deadline.
            let mut guard = tokio::time::timeout_at(deadline, self.conn.lock())
                .await
                .map_err(|_| UpstreamError::Timeout)?;
            // The connection is taken out of the slot for the exchange and only
            // put back when it completes successfully. A caller cancelled
            // mid-exchange (the fanout's `abort_all` does this to losing
            // queries) therefore drops a stream left with a half-written or
            // half-read frame instead of handing it to the next query.
            let mut stream = match guard.take() {
                Some(stream) => stream,
                None => self.connect(deadline).await?,
            };
            let result = exchange_framed(&mut stream, wire, deadline).await;
            if result.is_ok() {
                *guard = Some(stream);
            }
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio::net::TcpListener;

    /// A fake wire message long enough to carry a DNS header (`wire_id` needs
    /// at least 12 bytes): a 2-byte id, 10 bytes of zeroed header, then `tail`.
    fn fake_wire(id: u16, tail: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(12 + tail.len());
        v.extend_from_slice(&id.to_be_bytes());
        v.extend_from_slice(&[0u8; 10]);
        v.extend_from_slice(tail);
        v
    }

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
        let query = fake_wire(0x1234, b"hello");
        let reply = fake_wire(0x1234, b"abc");
        let query_check = query.clone();
        let reply_send = reply.clone();
        let server = tokio::spawn(async move {
            let mut len = [0u8; 2];
            b.read_exact(&mut len).await.unwrap();
            let mut msg = vec![0u8; usize::from(u16::from_be_bytes(len))];
            b.read_exact(&mut msg).await.unwrap();
            assert_eq!(msg, query_check);
            let mut out = (reply_send.len() as u16).to_be_bytes().to_vec();
            out.extend_from_slice(&reply_send);
            b.write_all(&out).await.unwrap();
        });
        let resp = exchange_framed(&mut a, &query, deadline(1000))
            .await
            .unwrap();
        assert_eq!(resp, reply);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn plain_tcp_reuses_the_connection_and_reconnects_after_close() {
        let addr = framed_echo(2).await;
        let up = TcpUpstream::plain("127.0.0.1", addr.port(), connector());
        assert_eq!(up.name(), format!("tcp://127.0.0.1:{}", addr.port()));
        let one = fake_wire(1, b"one");
        let two = fake_wire(2, b"two");
        let three = fake_wire(3, b"three");
        let four = fake_wire(4, b"four");
        assert_eq!(up.query(&one, deadline(1000)).await.unwrap(), one);
        assert_eq!(up.query(&two, deadline(1000)).await.unwrap(), two);
        // The server closes after two replies: the third query fails once, then reconnects.
        let third = up.query(&three, deadline(1000)).await;
        assert!(third.is_err() || third.as_deref() == Ok(three.as_slice()));
        assert_eq!(up.query(&four, deadline(1000)).await.unwrap(), four);
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
    async fn id_mismatch_is_bad_response_and_triggers_reconnect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // First connection replies with a well-formed frame carrying the
        // wrong id; every later connection echoes back verbatim.
        let first = Arc::new(AtomicBool::new(true));
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let first = first.clone();
                tokio::spawn(async move {
                    let mut len = [0u8; 2];
                    if s.read_exact(&mut len).await.is_err() {
                        return;
                    }
                    let n = usize::from(u16::from_be_bytes(len));
                    let mut msg = vec![0u8; n];
                    if s.read_exact(&mut msg).await.is_err() {
                        return;
                    }
                    let reply = if first.swap(false, Ordering::SeqCst) {
                        fake_wire(0xffff, b"wrong")
                    } else {
                        msg.clone()
                    };
                    let mut out = (reply.len() as u16).to_be_bytes().to_vec();
                    out.extend_from_slice(&reply);
                    let _ = s.write_all(&out).await;
                });
            }
        });
        let up = TcpUpstream::plain("127.0.0.1", addr.port(), connector());
        let bad = up.query(&fake_wire(0x1111, b"q"), deadline(1000)).await;
        assert!(matches!(bad, Err(UpstreamError::BadResponse(_))));
        let good = fake_wire(0x2222, b"ok");
        assert_eq!(up.query(&good, deadline(1000)).await.unwrap(), good);
    }

    #[tokio::test]
    async fn a_cancelled_query_does_not_poison_the_connection() {
        // The server reads one frame per connection and holds the reply until
        // `release` flips. The first query is cancelled while it is held, so a
        // connection left in the slot would hand the second query the first
        // one's answer (an id mismatch) instead of its own.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (release_tx, release_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut release = release_rx.clone();
                tokio::spawn(async move {
                    let mut len = [0u8; 2];
                    if s.read_exact(&mut len).await.is_err() {
                        return;
                    }
                    let n = usize::from(u16::from_be_bytes(len));
                    let mut msg = vec![0u8; n];
                    if s.read_exact(&mut msg).await.is_err() {
                        return;
                    }
                    while !*release.borrow_and_update() {
                        if release.changed().await.is_err() {
                            return;
                        }
                    }
                    let mut out = len.to_vec();
                    out.extend_from_slice(&msg);
                    let _ = s.write_all(&out).await;
                    // Hold the connection open (and answer nothing more) so a
                    // reused one fails with the id mismatch rather than with a
                    // reset from the closing socket.
                    let mut sink = Vec::new();
                    let _ = s.read_to_end(&mut sink).await;
                });
            }
        });
        let up = TcpUpstream::plain("127.0.0.1", addr.port(), connector());
        let first = fake_wire(0xaaaa, b"first");
        let cancelled = tokio::time::timeout(
            Duration::from_millis(100),
            up.query(&first, deadline(5_000)),
        )
        .await;
        assert!(
            cancelled.is_err(),
            "the first query must still be in flight when it is cancelled"
        );
        release_tx.send(true).unwrap();
        let second = fake_wire(0xbbbb, b"second");
        assert_eq!(
            up.query(&second, deadline(2_000)).await.unwrap(),
            second,
            "the next query must not inherit the cancelled query's connection"
        );
    }

    #[tokio::test]
    async fn lock_wait_respects_the_callers_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut len = [0u8; 2];
            s.read_exact(&mut len).await.unwrap();
            let n = usize::from(u16::from_be_bytes(len));
            let mut msg = vec![0u8; n];
            s.read_exact(&mut msg).await.unwrap();
            tokio::time::sleep(Duration::from_millis(400)).await;
            let mut out = len.to_vec();
            out.extend_from_slice(&msg);
            s.write_all(&out).await.unwrap();
        });
        let up = TcpUpstream::plain("127.0.0.1", addr.port(), connector());
        let a_wire = fake_wire(0xaaaa, b"a");
        let b_wire = fake_wire(0xbbbb, b"b");
        let started = Instant::now();
        let a_task = async { up.query(&a_wire, deadline(1000)).await };
        let b_task = async {
            let r = up.query(&b_wire, deadline(100)).await;
            (r, started.elapsed())
        };
        let (a, (b, b_elapsed)) = tokio::join!(a_task, b_task);
        assert_eq!(a.unwrap(), a_wire);
        assert_eq!(b, Err(UpstreamError::Timeout));
        assert!(
            b_elapsed < Duration::from_millis(300),
            "expected the lock wait to time out well under 400ms, took {b_elapsed:?}"
        );
    }

    #[test]
    fn dot_config_uses_the_rfc8310_alpn_protocol() {
        let base = rurge_net::http::tls_client_config(true).unwrap();
        let dot = dot_config(&base);
        assert_eq!(dot.alpn_protocols, vec![b"dot".to_vec()]);
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
        let secure = fake_wire(0xabcd, b"secure");
        assert_eq!(up.query(&secure, deadline(2000)).await.unwrap(), secure);
        let strict = rurge_net::http::tls_client_config(false).unwrap();
        let strict_up = TcpUpstream::tls("127.0.0.1", addr.port(), connector(), strict);
        assert!(matches!(
            strict_up.query(b"\x00\x02x", deadline(2000)).await,
            Err(UpstreamError::Tls(_))
        ));
    }
}
