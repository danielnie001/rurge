//! `trojan` outbound (manual: Policies › Trojan): TLS (optionally a
//! WebSocket), then `hex(SHA224(password)) CRLF CMD ATYP ADDR PORT CRLF` and
//! the payload. The server never answers the head: a wrong password only
//! shows once the relay starts, as whatever the server's fallback site says.

use crate::addr::{AddrError, socks_addr};
use crate::build::tls_client;
use crate::transport::Stack;
use crate::transport::lazy_head::LazyHead;
use crate::transport::ws::WsClient;
use crate::{BuildError, Outbound, OutboundError};
use rurge_config::KeystoreItem;
use rurge_config::spec::TrojanSpec;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use sha2::{Digest, Sha224};
use std::sync::Arc;

const CONNECT: u8 = 1;

/// No `Debug`: the hash is as good as the password.
pub struct TrojanOutbound {
    name: String,
    stack: Stack,
    /// `hex(SHA224(password))`, what the wire carries; the password itself is not kept.
    hash: [u8; 56],
}

fn wire_hash(password: &str) -> [u8; 56] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha224::digest(password.as_bytes());
    let mut out = [0u8; 56];
    for (i, byte) in digest.iter().enumerate() {
        out[2 * i] = HEX[usize::from(byte >> 4)];
        out[2 * i + 1] = HEX[usize::from(byte & 15)];
    }
    out
}

impl TrojanOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &TrojanSpec,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<TrojanOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and the
        // dry build both prefix it
        if spec.password.is_empty() {
            return Err(BuildError::new("`password` is empty"));
        }
        // no ALPN unless the policy asks for one: a WebSocket below must not
        // be negotiated into h2 (M2 design 4.5)
        let tls = tls_client(Some(&spec.tls), &server.host, &[], keystore, roots)?;
        let ws = spec
            .ws
            .as_ref()
            .map(|ws| WsClient::new(ws, &server, true))
            .transpose()?;
        Ok(TrojanOutbound {
            name: name.to_string(),
            stack: Stack::new(connector, server, tls, ws),
            hash: wire_hash(&spec.password),
        })
    }

    fn head(&self, target: &Target) -> Result<Vec<u8>, OutboundError> {
        let addr = socks_addr(target).map_err(|e| {
            OutboundError::Proxy(
                match e {
                    AddrError::Unsendable => "trojan: the host name cannot be sent to the server",
                    AddrError::TooLong => "trojan: the host name is longer than 255 bytes",
                }
                .to_string(),
            )
        })?;
        let mut head = Vec::with_capacity(56 + 2 + 1 + addr.len() + 2);
        head.extend_from_slice(&self.hash);
        head.extend_from_slice(b"\r\n");
        head.push(CONNECT);
        head.extend_from_slice(&addr);
        head.extend_from_slice(b"\r\n");
        Ok(head)
    }
}

impl Outbound for TrojanOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // never dial for a target whose name cannot be sent
            let head = self.head(target)?;
            // one budget for the connection, TLS and the WebSocket handshake
            let stream = match tokio::time::timeout(opts.timeout, self.stack.open(opts)).await {
                Ok(result) => result?,
                Err(_) => return Err(OutboundError::Timeout),
            };
            Ok(Box::new(LazyHead::new(stream, head)) as BoxedStream)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeTrojan, SeenHandshake, TlsFixture, TrojanScript, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::trojan::read_trojan;
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// The outbound for `definition` (a `trojan, host, port, ...` line).
    fn outbound(definition: &str, fixture: &Arc<TlsFixture>) -> TrojanOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("T", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_trojan(&mut r, &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        TrojanOutbound::new(
            "T",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn fake(
        password: &str,
        ws: bool,
        connect_to: Option<SocketAddr>,
    ) -> (Arc<TlsFixture>, FakeTrojan) {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeTrojan::spawn(
            TrojanScript {
                password: password.to_string(),
                ws,
                connect_to,
            },
            fixture.clone(),
        )
        .await;
        (fixture, fake)
    }

    #[test]
    fn the_wire_form_of_a_password_is_the_known_answer() {
        assert_eq!(
            std::str::from_utf8(&wire_hash("password")).unwrap(),
            "d63dc919e201d7bc4c825630d2cf25fdc93d4b2f0d46706d29038d01"
        );
    }

    #[tokio::test]
    async fn the_head_rides_with_the_first_payload() {
        let echo = echo_server().await;
        let (fixture, fake) = fake("pw", false, None).await;
        let out = outbound(
            &format!("trojan, 127.0.0.1, {}, password=pw", fake.addr().port()),
            &fixture,
        );
        assert_eq!(out.name(), "T");
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"first payload").await.unwrap();
        let mut buf = [0u8; 13];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"first payload");
        let seen = fake.requests();
        assert_eq!(
            (
                seen[0].command,
                seen[0].atyp,
                seen[0].host.as_str(),
                seen[0].port
            ),
            (1, 1, "127.0.0.1", echo.port())
        );
        assert_eq!(seen[0].early, b"first payload", "one TLS record, not two");
        assert!(out.http_forward().is_none());
    }

    #[tokio::test]
    async fn a_target_that_speaks_first_is_reached_without_a_write() {
        // accepts, greets, then echoes
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let greeter = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let _ = tcp.write_all(b"220 hi\r\n").await;
                    let mut buf = [0u8; 64];
                    while let Ok(n) = tcp.read(&mut buf).await {
                        if n == 0 || tcp.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        let (fixture, fake) = fake("pw", false, None).await;
        let out = outbound(
            &format!("trojan, 127.0.0.1, {}, password=pw", fake.addr().port()),
            &fixture,
        );
        let mut stream = out
            .connect_tcp(&target(greeter), &ConnectOpts::default())
            .await
            .unwrap();
        let mut banner = [0u8; 8];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut banner))
            .await
            .expect("the head went out although nothing was written")
            .unwrap();
        assert_eq!(&banner, b"220 hi\r\n");
        assert!(fake.requests()[0].early.is_empty());
    }

    #[tokio::test]
    async fn names_go_out_as_a_labels_and_an_unsendable_name_never_dials() {
        let echo = echo_server().await;
        let (fixture, fake) = fake("pw", false, Some(echo)).await;
        let out = outbound(
            &format!("trojan, 127.0.0.1, {}, password=pw", fake.addr().port()),
            &fixture,
        );
        let mut stream = out
            .connect_tcp(
                &Target::new(HostName::Domain("bücher.example".into()), 443),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        stream.write_all(b"x").await.unwrap();
        let mut one = [0u8; 1];
        stream.read_exact(&mut one).await.unwrap();
        let seen = fake.requests();
        assert_eq!(
            (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
            (3, "xn--bcher-kva.example", 443)
        );
        let before = fake.connections();
        for (name, expected) in [
            (
                "a@b.test".to_string(),
                "trojan: the host name cannot be sent to the server",
            ),
            (
                "a".repeat(256),
                "trojan: the host name is longer than 255 bytes",
            ),
        ] {
            let err = out
                .connect_tcp(
                    &Target::new(HostName::Domain(name), 443),
                    &ConnectOpts::default(),
                )
                .await
                .err()
                .expect("refused");
            assert!(
                matches!(&err, OutboundError::Proxy(m) if m == expected),
                "{err}"
            );
        }
        assert_eq!(fake.connections(), before, "nothing was dialled");
    }

    #[tokio::test]
    async fn a_wrong_password_cannot_be_told_at_connect_time() {
        // the protocol has no reply: the server treats us like a stray web
        // client, and that only shows once the relay starts
        let echo = echo_server().await;
        let (fixture, fake) = fake("right", false, None).await;
        let out = outbound(
            &format!("trojan, 127.0.0.1, {}, password=wrong", fake.addr().port()),
            &fixture,
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .expect("connecting succeeds");
        stream.write_all(b"hello").await.unwrap();
        let mut answer = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer))
            .await
            .expect("the server closes")
            .unwrap();
        assert!(
            answer.starts_with(b"HTTP/1.1 400"),
            "{}",
            String::from_utf8_lossy(&answer)
        );
        assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
    }

    #[tokio::test]
    async fn over_websocket() {
        let echo = echo_server().await;
        let (fixture, fake) = fake("pw", true, None).await;
        let out = outbound(
            &format!(
                "trojan, 127.0.0.1, {}, password=pw, ws=true, ws-path=/t, ws-headers=Host:edge.test|X-K:v",
                fake.addr().port()
            ),
            &fixture,
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"inside a frame").await.unwrap();
        let mut buf = [0u8; 14];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"inside a frame");
        let ws = fake.ws_seen();
        assert_eq!(ws[0].path, "/t");
        assert_eq!(
            (ws[0].header("host"), ws[0].header("x-k")),
            (Some("edge.test"), Some("v"))
        );
        assert_eq!(fake.requests()[0].early, b"inside a frame");
    }

    #[tokio::test]
    async fn the_tls_parameters_apply_and_there_is_no_alpn_by_default() {
        let echo = echo_server().await;
        let (fixture, fake) = fake("pw", false, None).await;
        let port = fake.addr().port();
        for (extra, expected) in [
            (
                "",
                SeenHandshake {
                    sni: None,
                    alpn: None,
                    client_cert: false,
                },
            ),
            (
                ", sni=front.example, server-cert-verify-name=127.0.0.1, alpn=http/1.1",
                SeenHandshake {
                    sni: Some("front.example".into()),
                    alpn: Some("http/1.1".into()),
                    client_cert: false,
                },
            ),
        ] {
            let out = outbound(
                &format!("trojan, 127.0.0.1, {port}, password=pw{extra}"),
                &fixture,
            );
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            stream.write_all(b"x").await.unwrap();
            let mut one = [0u8; 1];
            stream.read_exact(&mut one).await.unwrap();
            assert_eq!(fixture.seen().last(), Some(&expected), "{extra}");
        }
    }

    #[tokio::test]
    async fn one_budget_covers_the_whole_ladder() {
        // accepts TCP and then says nothing: the TLS handshake never ends
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((tcp, _)) = listener.accept().await {
                held.push(tcp);
            }
        });
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let out = outbound(
            &format!("trojan, 127.0.0.1, {}, password=pw", silent.port()),
            &fixture,
        );
        let started = std::time::Instant::now();
        let err = out
            .connect_tcp(
                &target(silent),
                &ConnectOpts {
                    timeout: Duration::from_millis(300),
                },
            )
            .await
            .err()
            .expect("times out");
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
