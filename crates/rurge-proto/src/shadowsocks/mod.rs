//! `ss` outbound (manual: Policies › Shadowsocks; phase 2 M6 design 3.3):
//! optionally behind Shadow TLS and / or simple-obfs, the AEAD stream
//! (`aead`) — or, with `none`, the bytes as they are. The request header is
//! the target as a SOCKS5 address; it waits in a `LazyHead` above the
//! stream for the first payload, so both are sealed into the first chunk.
//!
//! The server never answers the header: a wrong password or method only
//! shows once the relay reads, as a connection closed without an answer or
//! as data that does not decrypt.

pub(crate) mod aead;
pub(crate) mod cipher;
pub(crate) mod kdf;

use crate::addr::{AddrError, socks_addr};
use crate::build::shadow_tls_client;
use crate::transport::Stack;
use crate::transport::lazy_head::LazyHead;
use crate::transport::obfs::ObfsClient;
use crate::{BuildError, Outbound, OutboundError};
use aead::AeadStream;
use cipher::{AeadKind, MasterKey};
use rurge_config::spec::{ShadowTlsOpts, SsSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::sync::Arc;

/// No `Debug`: the master key is as good as the password.
pub struct ShadowsocksOutbound {
    name: String,
    stack: Stack,
    /// `None`: the method is `none`.
    key: Option<Arc<MasterKey>>,
}

impl ShadowsocksOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &SsSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<ShadowsocksOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and the
        // dry build both prefix it
        if spec.method.is_2022() {
            return Err(BuildError::new(format!(
                "ss: `{}` is not supported yet",
                spec.method.name()
            )));
        }
        let key = match AeadKind::of(spec.method) {
            None => None,
            Some(_) if spec.password.expose().is_empty() => {
                return Err(BuildError::new("`password` is empty"));
            }
            Some(kind) => Some(Arc::new(MasterKey::from_password(
                kind,
                spec.password.expose(),
            ))),
        };
        // `ss` has no TLS of its own: the camouflage certificate is checked
        // against the server's name
        let shadow_tls = shadow_tls_client(shadow_tls, None, &server.host, roots)?;
        let obfs = spec
            .obfs
            .as_ref()
            .map(|obfs| ObfsClient::new(obfs, &server))
            .transpose()?;
        let mut stack = Stack::new(connector, server, shadow_tls, None, None);
        if let Some(obfs) = obfs {
            stack = stack.with_obfs(obfs);
        }
        Ok(ShadowsocksOutbound {
            name: name.to_string(),
            stack,
            key,
        })
    }
}

fn head(target: &Target) -> Result<Vec<u8>, OutboundError> {
    socks_addr(target).map_err(|e| {
        OutboundError::Proxy(
            match e {
                AddrError::Unsendable => "ss: the host name cannot be sent to the server",
                AddrError::TooLong => "ss: the host name is longer than 255 bytes",
            }
            .to_string(),
        )
    })
}

impl Outbound for ShadowsocksOutbound {
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
            let head = head(target)?;
            let salt = match &self.key {
                None => Vec::new(),
                Some(key) => {
                    let mut salt = vec![0u8; key.salt_len()];
                    getrandom::fill(&mut salt).map_err(|_| {
                        OutboundError::Proxy("ss: no randomness available".to_string())
                    })?;
                    salt
                }
            };
            // one budget for the connection, Shadow TLS and obfs; the
            // server's first word comes with its first payload, in the relay
            let transport = match tokio::time::timeout(opts.timeout, self.stack.open(opts)).await {
                Ok(result) => result?,
                Err(_) => return Err(OutboundError::Timeout),
            };
            let stream: BoxedStream = match &self.key {
                None => transport,
                Some(key) => Box::new(AeadStream::new(
                    transport,
                    key.clone(),
                    salt,
                    aead::MAX_PAYLOAD,
                )),
            };
            Ok(Box::new(LazyHead::new(stream, head)) as BoxedStream)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UdpSupport;
    use crate::testing::{FakeShadowsocks, ShadowsocksScript, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::spec::ss::read_ss;
    use rurge_config::spec::{ObfsMode, ParamReader, Secret, SsMethod};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const AEAD: [SsMethod; 5] = [
        SsMethod::Aes128Gcm,
        SsMethod::Aes192Gcm,
        SsMethod::Aes256Gcm,
        SsMethod::ChaCha20IetfPoly1305,
        SsMethod::XChaCha20IetfPoly1305,
    ];

    /// The outbound for `definition` (an `ss, host, port, ...` line).
    fn outbound(definition: &str) -> ShadowsocksOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("S", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let read = read_ss(&mut r);
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        ShadowsocksOutbound::new(
            "S",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &read.spec,
            shadow_tls.as_ref(),
            Arc::new(RootCertStore::empty()),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn roundtrip(stream: &mut BoxedStream, text: &[u8]) {
        // no explicit `flush()`: `write_all` alone must deliver
        stream.write_all(text).await.unwrap();
        let mut buf = vec![0u8; text.len()];
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut buf))
            .await
            .expect("the echo arrives within the bound")
            .unwrap();
        assert!(buf == text, "the echo differs");
    }

    #[tokio::test]
    async fn every_method_carries_the_head_with_the_first_payload() {
        let echo = echo_server().await;
        for method in AEAD.into_iter().chain([SsMethod::None]) {
            let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(method, "pw")).await;
            let out = outbound(&format!(
                "ss, 127.0.0.1, {}, encrypt-method={}, password=pw",
                fake.addr().port(),
                method.name()
            ));
            assert_eq!(out.name(), "S");
            assert_eq!(out.udp(), UdpSupport::Unsupported);
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"hello through ss").await;
            roundtrip(&mut stream, b"and again").await;
            let seen = fake.requests();
            assert_eq!(seen.len(), 1, "{method:?}");
            assert_eq!(
                (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
                (1, "127.0.0.1", echo.port())
            );
            assert_eq!(seen[0].early, b"hello through ss", "{method:?}: one chunk");
            assert_eq!(seen[0].salt.len(), method.key_len());
        }
    }

    #[tokio::test]
    async fn each_connection_has_a_salt_of_its_own() {
        let echo = echo_server().await;
        let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(SsMethod::Aes256Gcm, "pw")).await;
        let out = outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method=aes-256-gcm, password=pw",
            fake.addr().port()
        ));
        for _ in 0..2 {
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"x").await;
        }
        let seen = fake.requests();
        assert_ne!(seen[0].salt, seen[1].salt);
        assert_ne!(
            seen[0].salt,
            fake.answer_salts()[0],
            "and one per direction"
        );
    }

    #[tokio::test]
    async fn a_large_payload_crosses_many_chunks_both_ways() {
        let echo = echo_server().await;
        let fake =
            FakeShadowsocks::spawn(ShadowsocksScript::new(SsMethod::ChaCha20IetfPoly1305, "pw"))
                .await;
        let out = outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method=chacha20-ietf-poly1305, password=pw",
            fake.addr().port()
        ));
        let stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        let data: Vec<u8> = (0..1 << 20).map(|i: u32| (i % 251) as u8).collect();
        let (mut read, mut write) = tokio::io::split(stream);
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            write.write_all(&sent).await.unwrap();
        });
        let mut back = vec![0u8; data.len()];
        tokio::time::timeout(Duration::from_secs(30), read.read_exact(&mut back))
            .await
            .expect("the echo arrives within the bound")
            .unwrap();
        assert!(back == data, "the echo differs");
        writer.await.unwrap();
        // the fake refuses anything longer
        assert_eq!(fake.largest_chunk(), aead::MAX_PAYLOAD, "full chunks");
    }

    #[tokio::test]
    async fn names_go_out_as_a_labels_and_an_unsendable_name_never_dials() {
        let echo = echo_server().await;
        let fake = FakeShadowsocks::spawn(ShadowsocksScript {
            connect_to: Some(echo),
            ..ShadowsocksScript::new(SsMethod::Aes128Gcm, "pw")
        })
        .await;
        let out = outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method=aes-128-gcm, password=pw",
            fake.addr().port()
        ));
        let mut stream = out
            .connect_tcp(
                &Target::new(HostName::Domain("bücher.example".into()), 443),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        roundtrip(&mut stream, b"x").await;
        let seen = fake.requests();
        assert_eq!(
            (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
            (3, "xn--bcher-kva.example", 443)
        );
        let before = fake.connections();
        for (name, expected) in [
            (
                "a@b.test".to_string(),
                "ss: the host name cannot be sent to the server",
            ),
            (
                "a".repeat(256),
                "ss: the host name is longer than 255 bytes",
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
    async fn a_wrong_password_is_a_connection_closed_without_an_answer() {
        let echo = echo_server().await;
        let fake =
            FakeShadowsocks::spawn(ShadowsocksScript::new(SsMethod::Aes256Gcm, "right")).await;
        let out = outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method=aes-256-gcm, password=wrong",
            fake.addr().port()
        ));
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .expect("connecting succeeds");
        stream.write_all(b"hello").await.unwrap();
        let mut answer = Vec::new();
        let err = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer))
            .await
            .expect("the server closes")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "ss: the server closed the connection without answering"
        );
        assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
    }

    #[tokio::test]
    async fn through_both_obfs_modes() {
        let echo = echo_server().await;
        for (mode, extra) in [
            (
                ObfsMode::Http,
                "obfs=http, obfs-host=cdn.example, obfs-uri=/a",
            ),
            (ObfsMode::Tls, "obfs=tls, obfs-host=cdn.example"),
        ] {
            let fake = FakeShadowsocks::spawn(ShadowsocksScript {
                obfs: Some(mode),
                ..ShadowsocksScript::new(SsMethod::Aes128Gcm, "pw")
            })
            .await;
            let port = fake.addr().port();
            let out = outbound(&format!(
                "ss, 127.0.0.1, {port}, encrypt-method=aes-128-gcm, password=pw, {extra}"
            ));
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"behind the camouflage").await;
            let data = vec![0xa5u8; 100_000];
            roundtrip(&mut stream, &data).await;
            let hello = &fake.obfs_seen()[0];
            match mode {
                ObfsMode::Http => {
                    assert_eq!(hello.host, format!("cdn.example:{port}"));
                    assert_eq!(hello.uri.as_deref(), Some("/a"));
                }
                ObfsMode::Tls => assert_eq!(hello.host, "cdn.example"),
            }
            assert_eq!(fake.requests()[0].early, b"behind the camouflage");
        }
    }

    #[tokio::test]
    async fn a_half_close_reaches_the_target_and_the_answer_still_arrives() {
        // reads to the end, then answers with how much it got
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let counter = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut all = Vec::new();
                    if tcp.read_to_end(&mut all).await.is_ok() {
                        let _ = tcp.write_all(format!("got {}", all.len()).as_bytes()).await;
                    }
                });
            }
        });
        for method in [SsMethod::XChaCha20IetfPoly1305, SsMethod::None] {
            let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(method, "pw")).await;
            let out = outbound(&format!(
                "ss, 127.0.0.1, {}, encrypt-method={}, password=pw",
                fake.addr().port(),
                method.name()
            ));
            let mut stream = out
                .connect_tcp(&target(counter), &ConnectOpts::default())
                .await
                .unwrap();
            stream.write_all(b"12345").await.unwrap();
            stream.shutdown().await.unwrap();
            let mut answer = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer))
                .await
                .expect("the answer arrives")
                .unwrap();
            assert_eq!(answer, b"got 5", "{method:?}");
        }
    }

    #[test]
    fn what_cannot_be_built_is_a_build_error_that_quotes_nothing() {
        let build = |method: SsMethod, password: &str| {
            ShadowsocksOutbound::new(
                "S",
                Target::new(HostName::parse("127.0.0.1"), 8388),
                &SsSpec {
                    method,
                    password: Secret::from(password),
                    keys: Secret::default(),
                    udp_relay: false,
                    udp_port: None,
                    obfs: None,
                },
                None,
                Arc::new(RootCertStore::empty()),
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            )
            .map(|_| ())
            .unwrap_err()
            .message
        };
        assert_eq!(build(SsMethod::Aes128Gcm, ""), "`password` is empty");
        assert_eq!(
            build(SsMethod::Blake3Aes128Gcm, "secret"),
            "ss: `2022-blake3-aes-128-gcm` is not supported yet"
        );
        // `none` needs no password
        assert!(
            ShadowsocksOutbound::new(
                "S",
                Target::new(HostName::parse("127.0.0.1"), 8388),
                &SsSpec {
                    method: SsMethod::None,
                    password: Secret::default(),
                    keys: Secret::default(),
                    udp_relay: false,
                    udp_port: None,
                    obfs: None,
                },
                None,
                Arc::new(RootCertStore::empty()),
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            )
            .is_ok()
        );
    }
}
