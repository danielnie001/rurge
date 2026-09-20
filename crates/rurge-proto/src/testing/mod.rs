//! Scriptable loopback peers for tests (phase 2 M1 design §5.6). Never used
//! by production code. None of them ever resolves a host name.

mod http_proxy;
mod socks5;
mod tls;
mod trojan;
mod vmess;
pub mod ws;

pub use http_proxy::{FakeHttpProxy, HttpProxyScript, RecordedHead};
pub use socks5::{FakeSocks5, RecordedSocks5, Socks5Script};
pub use tls::{SeenHandshake, TlsFixture};
pub use trojan::{FakeTrojan, RecordedTrojan, TrojanScript};
pub use vmess::{FakeVmess, RecordedVmess, VmessScript};
pub use ws::{FakeWs, RecordedWs, WsScript};

use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Aborts the task it holds when dropped.
pub(crate) struct AbortOnDrop(pub(crate) tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Echoes every byte back until the peer closes.
pub async fn echo_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::TcpStream;

    async fn roundtrip<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(stream: &mut S) {
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    }

    #[tokio::test]
    async fn the_fake_http_proxy_tunnels_checks_credentials_and_records() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript {
            auth: Some(("u".into(), "p".into())),
            ..HttpProxyScript::default()
        })
        .await;
        // wrong credentials
        let mut s = TcpStream::connect(proxy.addr()).await.unwrap();
        s.write_all(format!("CONNECT {echo} HTTP/1.1\r\nHost: {echo}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 407 "), "{answer}");
        // dTpw = base64("u:p")
        let mut s = TcpStream::connect(proxy.addr()).await.unwrap();
        s.write_all(
            format!(
                "CONNECT {echo} HTTP/1.1\r\nHost: {echo}\r\nProxy-Authorization: Basic dTpw\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        let (head, rest) = crate::transport::head::read_head(&mut s, 4096)
            .await
            .unwrap();
        assert!(
            head.starts_with(b"HTTP/1.1 200 "),
            "{}",
            String::from_utf8_lossy(&head)
        );
        assert!(rest.is_empty());
        roundtrip(&mut s).await;
        let heads = proxy.heads();
        assert_eq!(heads.len(), 2);
        assert_eq!(heads[1].request_line, format!("CONNECT {echo} HTTP/1.1"));
        assert_eq!(heads[1].header("Proxy-Authorization"), Some("Basic dTpw"));
    }

    #[tokio::test]
    async fn the_fake_http_proxy_never_resolves_names() {
        let proxy = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
        let mut s = TcpStream::connect(proxy.addr()).await.unwrap();
        s.write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
            .await
            .unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 502 "), "{answer}");
    }

    #[tokio::test]
    async fn the_fake_socks5_server_connects_and_records() {
        let echo = echo_server().await;
        let server = FakeSocks5::spawn(Socks5Script {
            auth: Some(("u".into(), "p".into())),
            ..Socks5Script::default()
        })
        .await;
        let mut s = TcpStream::connect(server.addr()).await.unwrap();
        s.write_all(&[5, 2, 0, 2]).await.unwrap();
        let mut method = [0u8; 2];
        s.read_exact(&mut method).await.unwrap();
        assert_eq!(method, [5, 2]);
        s.write_all(&[1, 1, b'u', 1, b'p']).await.unwrap();
        let mut ok = [0u8; 2];
        s.read_exact(&mut ok).await.unwrap();
        assert_eq!(ok, [1, 0]);
        let SocketAddr::V4(v4) = echo else {
            panic!("loopback is v4")
        };
        let mut request = vec![5, 1, 0, 1];
        request.extend_from_slice(&v4.ip().octets());
        request.extend_from_slice(&v4.port().to_be_bytes());
        s.write_all(&request).await.unwrap();
        let mut reply = [0u8; 10];
        s.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[..2], [5, 0]);
        roundtrip(&mut s).await;
        let seen = server.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].methods, [0, 2]);
        assert_eq!(seen[0].credentials, Some(("u".into(), "p".into())));
        assert_eq!(
            (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
            (1, "127.0.0.1", v4.port())
        );
    }

    #[tokio::test]
    async fn the_tls_fixture_records_what_the_client_sent() {
        let fixture = TlsFixture::new(&["localhost"]);
        let addr = fixture.spawn_echo(false).await;
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(fixture.roots())
        .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let tcp = TcpStream::connect(addr).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let mut tls = connector.connect(name, tcp).await.unwrap();
        roundtrip(&mut tls).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while fixture.seen().is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "handshake never recorded"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            fixture.seen(),
            [SeenHandshake {
                sni: Some("localhost".into()),
                alpn: None,
                client_cert: false
            }]
        );
        assert_eq!(fixture.leaf_fingerprint().len(), 32);
        let (cert, key) = fixture.issue_client("client");
        assert!(!cert.is_empty() && !key.is_empty());
    }
}
