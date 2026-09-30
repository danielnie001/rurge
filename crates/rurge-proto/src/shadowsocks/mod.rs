//! `ss` outbound (manual: Policies › Shadowsocks; phase 2 M6 design 3.3):
//! optionally behind Shadow TLS and / or simple-obfs, the AEAD stream
//! (`aead`) — in its SS 2022 form for the `2022-blake3-*` methods, with the
//! identity headers of a multi-user key (`s2022`) — or, with `none`, the
//! bytes as they are. The request header is the target as a SOCKS5 address;
//! it waits in a `LazyHead` above the stream for the first payload, so both
//! are sealed into the first chunk (SS 2022: the two header chunks).
//!
//! The server never answers the header: a wrong password or method only
//! shows once the relay reads, as a connection closed without an answer or
//! as data that does not decrypt. SS 2022's answer names the request it
//! belongs to and the server's time, which the stream checks.
//!
//! With `udp-relay=true` datagrams go to the server by themselves (`udp`),
//! past Shadow TLS and obfs, which are TCP's.

pub(crate) mod aead;
pub(crate) mod cipher;
pub(crate) mod kdf;
pub(crate) mod s2022;
pub(crate) mod udp;

use crate::addr::{AddrError, socks_addr};
use crate::build::shadow_tls_client;
use crate::transport::Stack;
use crate::transport::lazy_head::LazyHead;
use crate::transport::obfs::ObfsClient;
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
use aead::{AeadStream, Request2022};
use cipher::{AeadKind, MasterKey};
use rurge_config::spec::{ShadowTlsOpts, SsSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use s2022::Identity;
use std::sync::Arc;
use udp::{Keys2022, Packets, SsUdp};

/// No `Debug`: the master key is as good as the password.
pub struct ShadowsocksOutbound {
    name: String,
    stack: Stack,
    /// `None`: the method is `none`. SS 2022: the user key's.
    key: Option<Arc<MasterKey>>,
    /// `Some` for SS 2022, with no layers for a single key.
    identity: Option<Identity>,
    /// Seconds since the Unix epoch, for SS 2022's timestamps.
    now: fn() -> u64,
    /// `udp-relay=true`: where the datagrams go and how they are sealed.
    udp: Option<UdpRelay>,
    /// Where the datagrams leave from: the way the TCP connections go
    /// (DIRECT, or `underlying-proxy`).
    connector: Arc<dyn Connector>,
}

struct UdpRelay {
    /// `udp-port`, else the server's port.
    server: Target,
    packets: Arc<Packets>,
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
        let (key, identity) = match AeadKind::of(spec.method) {
            None => (None, None),
            Some(kind) if spec.method.is_2022() => {
                // the configuration checked them; a spec made by hand may not be
                let keys = spec.keys.expose();
                let Some(user) = keys.last() else {
                    return Err(BuildError::new("`password` is empty"));
                };
                if keys.iter().any(|key| key.len() != kind.key_len()) {
                    return Err(BuildError::new(format!(
                        "`password` is not Base64 keys of the length `{}` requires",
                        spec.method.name()
                    )));
                }
                (
                    Some(Arc::new(MasterKey::from_psk(kind, user))),
                    Some(Identity::new(keys)),
                )
            }
            Some(_) if spec.password.expose().is_empty() => {
                return Err(BuildError::new("`password` is empty"));
            }
            Some(kind) => (
                Some(Arc::new(MasterKey::from_password(
                    kind,
                    spec.password.expose(),
                ))),
                None,
            ),
        };
        // `ss` has no TLS of its own: the camouflage certificate is checked
        // against the server's name
        let shadow_tls = shadow_tls_client(shadow_tls, None, &server.host, roots)?;
        let obfs = spec
            .obfs
            .as_ref()
            .map(|obfs| ObfsClient::new(obfs, &server))
            .transpose()?;
        let udp = spec.udp_relay.then(|| UdpRelay {
            server: Target::new(server.host.clone(), spec.udp_port.unwrap_or(server.port)),
            packets: Arc::new(match (&key, AeadKind::of(spec.method)) {
                (Some(_), Some(kind)) if spec.method.is_2022() => {
                    Packets::S2022(Keys2022::new(kind, spec.keys.expose()))
                }
                (Some(key), _) => Packets::Aead(key.clone()),
                (None, _) => Packets::Plain,
            }),
        });
        let mut stack = Stack::new(connector.clone(), server, shadow_tls, None, None);
        if let Some(obfs) = obfs {
            stack = stack.with_obfs(obfs);
        }
        Ok(ShadowsocksOutbound {
            name: name.to_string(),
            stack,
            key,
            identity,
            now: s2022::unix_now,
            udp,
            connector,
        })
    }

    /// Runs SS 2022's timestamps off another clock.
    #[cfg(test)]
    fn with_clock(self, now: fn() -> u64) -> ShadowsocksOutbound {
        ShadowsocksOutbound { now, ..self }
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
            let stream: BoxedStream = match (&self.key, &self.identity) {
                (None, _) => transport,
                (Some(key), None) => Box::new(AeadStream::new(
                    transport,
                    key.clone(),
                    salt,
                    aead::MAX_PAYLOAD,
                )),
                (Some(key), Some(identity)) => {
                    let request = Request2022 {
                        identity: identity.headers(&salt),
                        addr_len: head.len(),
                        now: self.now,
                    };
                    Box::new(AeadStream::new_2022(transport, key.clone(), salt, request))
                }
            };
            Ok(Box::new(LazyHead::new(stream, head)) as BoxedStream)
        })
    }

    fn udp(&self) -> UdpSupport {
        if self.udp.is_some() {
            UdpSupport::Native
        } else {
            UdpSupport::Unsupported
        }
    }

    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            let Some(relay) = &self.udp else {
                return Err(OutboundError::Unsupported(
                    "UDP without `udp-relay=true`".to_string(),
                ));
            };
            let open = async {
                let socket = self.connector.open_udp(opts).await?;
                let carrier = SsUdp::open(socket, &relay.server, relay.packets.clone(), self.now);
                Ok(Box::new(carrier.await?) as BoxedPacketSocket)
            };
            match tokio::time::timeout(opts.timeout, open).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr::socks_addr;
    use crate::testing::{FakeShadowsocks, ShadowsocksScript, echo_server, udp_echo_server};
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::spec::ss::read_ss;
    use rurge_config::spec::{ObfsMode, ParamReader, Secret, SsMethod};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, PacketSocket, SystemResolve};
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

    const SS_2022: [SsMethod; 2] = [SsMethod::Blake3Aes128Gcm, SsMethod::Blake3Aes256Gcm];

    /// A Base64 key of `method`'s length, every byte `byte`.
    fn key_2022(method: SsMethod, byte: u8) -> String {
        STANDARD.encode(vec![byte; method.key_len()])
    }

    /// An outbound of `method` whose `password` is `password`.
    fn outbound_2022(
        fake: &FakeShadowsocks,
        method: SsMethod,
        password: &str,
    ) -> ShadowsocksOutbound {
        outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method={}, password={password}",
            fake.addr().port(),
            method.name()
        ))
    }

    #[tokio::test]
    async fn ss_2022_round_trips_with_one_key_or_as_one_of_several_users() {
        let echo = echo_server().await;
        for method in SS_2022 {
            let (server, user) = (key_2022(method, 1), key_2022(method, 2));
            // a single key; the server's key and one of its users' behind it,
            // also behind obfs
            let setups = [
                (
                    ShadowsocksScript::new(method, &server),
                    server.clone(),
                    None,
                ),
                (
                    ShadowsocksScript {
                        users: vec![key_2022(method, 3), user.clone()],
                        ..ShadowsocksScript::new(method, &server)
                    },
                    format!("{server}:{user}"),
                    Some(1),
                ),
                (
                    ShadowsocksScript {
                        users: vec![user.clone()],
                        obfs: Some(ObfsMode::Tls),
                        ..ShadowsocksScript::new(method, &server)
                    },
                    format!("{server}:{user}, obfs=tls, obfs-host=cdn.example"),
                    Some(0),
                ),
            ];
            for (script, password, user) in setups {
                let fake = FakeShadowsocks::spawn(script).await;
                let out = outbound_2022(&fake, method, &password);
                let mut stream = out
                    .connect_tcp(&target(echo), &ConnectOpts::default())
                    .await
                    .unwrap();
                roundtrip(&mut stream, b"hello through ss 2022").await;
                roundtrip(&mut stream, b"and again").await;
                let seen = fake.requests();
                assert_eq!(seen.len(), 1, "{method:?} {password}");
                assert_eq!(
                    (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
                    (1, "127.0.0.1", echo.port())
                );
                assert_eq!(seen[0].early, b"hello through ss 2022", "one write");
                assert_eq!((seen[0].padding, seen[0].user), (0, user));
                assert_eq!(seen[0].salt.len(), method.key_len());
            }
        }
    }

    #[tokio::test]
    async fn ss_2022_sends_names_and_crosses_chunks_of_0xffff() {
        let echo = echo_server().await;
        let method = SsMethod::Blake3Aes256Gcm;
        let key = key_2022(method, 7);
        let fake = FakeShadowsocks::spawn(ShadowsocksScript {
            connect_to: Some(echo),
            ..ShadowsocksScript::new(method, &key)
        })
        .await;
        let out = outbound_2022(&fake, method, &key);
        let stream = out
            .connect_tcp(
                &Target::new(HostName::Domain("bücher.example".into()), 443),
                &ConnectOpts::default(),
            )
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
        let seen = fake.requests();
        assert_eq!(
            (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
            (3, "xn--bcher-kva.example", 443)
        );
        assert_eq!(fake.largest_chunk(), s2022::MAX_PAYLOAD, "full chunks");
    }

    #[tokio::test]
    async fn a_silent_client_pads_its_request_and_hears_the_target_first() {
        // a target that speaks first, then echoes
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let greeter = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                tokio::spawn(async move {
                    tcp.write_all(b"220 ready").await?;
                    let (mut r, mut w) = tcp.split();
                    tokio::io::copy(&mut r, &mut w).await
                });
            }
        });
        let method = SsMethod::Blake3Aes128Gcm;
        let key = key_2022(method, 5);
        let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(method, &key)).await;
        let out = outbound_2022(&fake, method, &key);
        let mut stream = out
            .connect_tcp(&target(greeter), &ConnectOpts::default())
            .await
            .unwrap();
        let mut greeting = [0u8; 9];
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut greeting))
            .await
            .expect("the greeting arrives")
            .unwrap();
        assert_eq!(&greeting, b"220 ready");
        roundtrip(&mut stream, b"HELO").await;
        let seen = fake.requests();
        assert!(seen[0].early.is_empty());
        assert!((1..=900).contains(&seen[0].padding), "{}", seen[0].padding);
    }

    #[tokio::test]
    async fn a_key_the_server_does_not_know_is_a_connection_closed_without_an_answer() {
        let echo = echo_server().await;
        let method = SsMethod::Blake3Aes128Gcm;
        let (server, user) = (key_2022(method, 1), key_2022(method, 2));
        let stranger = key_2022(method, 9);
        for (script, password) in [
            (ShadowsocksScript::new(method, &server), stranger.clone()),
            (
                ShadowsocksScript {
                    users: vec![user.clone()],
                    ..ShadowsocksScript::new(method, &server)
                },
                format!("{server}:{stranger}"),
            ),
            (
                ShadowsocksScript {
                    users: vec![user.clone()],
                    ..ShadowsocksScript::new(method, &server)
                },
                // a user key without the identity header
                user.clone(),
            ),
        ] {
            let fake = FakeShadowsocks::spawn(script).await;
            let out = outbound_2022(&fake, method, &password);
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
                "ss: the server closed the connection without answering",
                "{password}"
            );
            assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
        }
    }

    fn an_hour_behind() -> u64 {
        s2022::unix_now() - 3600
    }

    #[tokio::test]
    async fn a_client_clock_an_hour_off_is_refused_without_an_answer() {
        let echo = echo_server().await;
        let method = SsMethod::Blake3Aes256Gcm;
        let key = key_2022(method, 4);
        let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(method, &key)).await;
        let out = outbound_2022(&fake, method, &key).with_clock(an_hour_behind);
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
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
        assert_eq!(fake.rejected(), 1);
    }

    #[tokio::test]
    async fn a_replayed_request_salt_is_refused_without_an_answer() {
        let echo = echo_server().await;
        let method = SsMethod::Blake3Aes128Gcm;
        let key = vec![6u8; 16];
        let fake =
            FakeShadowsocks::spawn(ShadowsocksScript::new(method, &STANDARD.encode(&key))).await;
        let head = socks_addr(&target(echo)).unwrap();
        let salt = vec![0x42u8; 16];
        let mut outcomes = Vec::new();
        for _ in 0..2 {
            let tcp = tokio::net::TcpStream::connect(fake.addr()).await.unwrap();
            let request = Request2022 {
                identity: Vec::new(),
                addr_len: head.len(),
                now: s2022::unix_now,
            };
            let mut stream = AeadStream::new_2022(
                Box::new(tcp),
                Arc::new(MasterKey::from_psk(AeadKind::Aes128Gcm, &key)),
                salt.clone(),
                request,
            );
            stream
                .write_all(&[&head[..], b"once"].concat())
                .await
                .unwrap();
            let mut answer = [0u8; 4];
            let outcome =
                tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut answer))
                    .await
                    .expect("an answer or the close")
                    .map(|_| answer.to_vec())
                    .map_err(|e| e.to_string());
            outcomes.push(outcome);
        }
        assert_eq!(outcomes[0], Ok(b"once".to_vec()));
        assert_eq!(
            outcomes[1],
            Err("ss: the server closed the connection without answering".to_string())
        );
        assert_eq!((fake.requests().len(), fake.rejected()), (1, 1));
    }

    #[tokio::test]
    async fn an_answer_off_the_clock_or_for_another_request_is_an_error() {
        let echo = echo_server().await;
        let method = SsMethod::Blake3Aes128Gcm;
        let key = key_2022(method, 8);
        for script in [
            ShadowsocksScript {
                answer_skew: 3600,
                ..ShadowsocksScript::new(method, &key)
            },
            ShadowsocksScript {
                wrong_request_salt: true,
                ..ShadowsocksScript::new(method, &key)
            },
        ] {
            let skewed = script.answer_skew != 0;
            let fake = FakeShadowsocks::spawn(script).await;
            let out = outbound_2022(&fake, method, &key);
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            stream.write_all(b"hello").await.unwrap();
            let mut answer = [0u8; 5];
            let err = tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut answer))
                .await
                .expect("the answer arrives")
                .unwrap_err();
            let text = err.to_string();
            if skewed {
                // a second may tick between the two clocks
                let seconds = text
                    .strip_prefix("ss: the server's clock differs from ours by ")
                    .and_then(|rest| rest.strip_suffix(" seconds (at most 30 are allowed)"))
                    .and_then(|n| n.parse::<u64>().ok());
                assert!(
                    seconds.is_some_and(|n| (3599..=3601).contains(&n)),
                    "{text}"
                );
            } else {
                assert_eq!(text, "ss: the server's answer is not for this request");
            }
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        }
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_roundtrip(carrier: &dyn PacketSocket, to: SocketAddr, payload: &[u8]) {
        carrier.send_to(payload, &target(to)).await.unwrap();
        assert_eq!(udp_answer(carrier).await, (payload.to_vec(), target(to)));
    }

    /// Nothing comes back within a short while.
    async fn no_udp_answer(carrier: &dyn PacketSocket) {
        let mut buf = vec![0u8; 65536];
        let got =
            tokio::time::timeout(Duration::from_millis(300), carrier.recv_from(&mut buf)).await;
        assert!(got.is_err(), "no answer: {got:?}");
    }

    /// Every method: the script, the line's `password` and the user the
    /// fake should find.
    fn udp_setups() -> Vec<(ShadowsocksScript, String, Option<usize>)> {
        let mut setups: Vec<_> = AEAD
            .into_iter()
            .chain([SsMethod::None])
            .map(|method| (ShadowsocksScript::new(method, "pw"), "pw".to_string(), None))
            .collect();
        for method in SS_2022 {
            let (server, user) = (key_2022(method, 1), key_2022(method, 2));
            setups.push((
                ShadowsocksScript::new(method, &server),
                server.clone(),
                None,
            ));
            setups.push((
                ShadowsocksScript {
                    users: vec![key_2022(method, 3), user.clone()],
                    ..ShadowsocksScript::new(method, &server)
                },
                format!("{server}:{user}"),
                Some(1),
            ));
        }
        setups
    }

    fn udp_outbound(
        fake: &FakeShadowsocks,
        method: SsMethod,
        password: &str,
    ) -> ShadowsocksOutbound {
        outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method={}, password={password}, udp-relay=true",
            fake.addr().port(),
            method.name()
        ))
    }

    /// A password for `method`: a key for SS 2022.
    fn udp_password(method: SsMethod) -> String {
        if method.is_2022() {
            key_2022(method, 1)
        } else {
            "pw".to_string()
        }
    }

    #[tokio::test]
    async fn udp_round_trips_with_every_method() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        for (script, password, user) in udp_setups() {
            let method = script.method;
            let fake = FakeShadowsocks::spawn(script).await;
            let out = udp_outbound(&fake, method, &password);
            assert_eq!(out.udp(), UdpSupport::Native);
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            udp_roundtrip(carrier.as_ref(), one, b"to one").await;
            udp_roundtrip(carrier.as_ref(), two, b"to two").await;
            // another carrier is another client session
            let other = out.open_udp(&ConnectOpts::default()).await.unwrap();
            udp_roundtrip(other.as_ref(), one, b"again").await;
            let seen = fake.datagrams();
            let targets: Vec<_> = seen.iter().map(|d| d.target.clone()).collect();
            assert_eq!(
                targets,
                [one.to_string(), two.to_string(), one.to_string()],
                "{method:?}"
            );
            assert_eq!(seen[1].payload, b"to two");
            if method.is_2022() {
                let ids: Vec<(u64, u64)> = seen.iter().map(|d| d.session.unwrap()).collect();
                assert_eq!(ids[0].0, ids[1].0, "one session");
                assert_eq!((ids[0].1, ids[1].1), (0, 1), "counting from 0");
                assert_ne!(ids[2].0, ids[0].0, "{method:?}");
                assert_eq!(ids[2].1, 0);
                assert!(seen.iter().all(|d| d.user == user && d.padding == 0));
            } else {
                assert!(seen.iter().all(|d| d.session.is_none()));
            }
        }
    }

    #[tokio::test]
    async fn udp_goes_to_udp_port_when_it_is_written() {
        let echo = udp_echo_server().await;
        for method in [SsMethod::Aes128Gcm, SsMethod::Blake3Aes256Gcm] {
            let password = udp_password(method);
            let fake = FakeShadowsocks::spawn(ShadowsocksScript {
                udp_apart: true,
                ..ShadowsocksScript::new(method, &password)
            })
            .await;
            assert_ne!(fake.udp_addr().port(), fake.addr().port());
            let out = outbound(&format!(
                "ss, 127.0.0.1, {}, encrypt-method={}, password={password}, udp-relay=true, udp-port={}",
                fake.addr().port(),
                method.name(),
                fake.udp_addr().port()
            ));
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            udp_roundtrip(carrier.as_ref(), echo, b"to the other port").await;
            assert_eq!(fake.connections(), 0, "no TCP");
        }
    }

    /// Whoever sends to the server's socket for this client reaches it, with
    /// their own address.
    #[tokio::test]
    async fn udp_is_full_cone() {
        let echo = udp_echo_server().await;
        for method in [SsMethod::ChaCha20IetfPoly1305, SsMethod::Blake3Aes128Gcm] {
            let password = udp_password(method);
            let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(method, &password)).await;
            let out = udp_outbound(&fake, method, &password);
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            udp_roundtrip(carrier.as_ref(), echo, b"hello").await;
            let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            stranger
                .send_to(b"from a stranger", fake.udp_outside()[0])
                .await
                .unwrap();
            assert_eq!(
                udp_answer(carrier.as_ref()).await,
                (
                    b"from a stranger".to_vec(),
                    target(stranger.local_addr().unwrap())
                ),
                "{method:?}"
            );
        }
    }

    #[tokio::test]
    async fn udp_sends_names_to_the_server() {
        let echo = udp_echo_server().await;
        let method = SsMethod::Blake3Aes256Gcm;
        let key = key_2022(method, 1);
        let fake = FakeShadowsocks::spawn(ShadowsocksScript {
            connect_to: Some(echo),
            ..ShadowsocksScript::new(method, &key)
        })
        .await;
        let out = udp_outbound(&fake, method, &key);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let name = Target::new(HostName::Domain("bücher.example".into()), 53);
        carrier.send_to(b"a query", &name).await.unwrap();
        // the answer names where it really came from
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"a query".to_vec(), target(echo))
        );
        assert_eq!(fake.datagrams()[0].target, "xn--bcher-kva.example:53");
        let err = carrier
            .send_to(b"x", &Target::new(HostName::Domain("a@b.test".into()), 53))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "ss: the host name cannot be sent to the server"
        );
        assert_eq!(fake.datagrams().len(), 1);
    }

    #[tokio::test]
    async fn a_garbled_or_replayed_answer_is_dropped_and_the_next_one_arrives() {
        let echo = udp_echo_server().await;
        let garbled = |method: SsMethod| ShadowsocksScript {
            udp_garbled_first: true,
            ..ShadowsocksScript::new(method, &udp_password(method))
        };
        let method = SsMethod::Blake3Aes128Gcm;
        for script in [
            garbled(SsMethod::Aes256Gcm),
            garbled(method),
            ShadowsocksScript {
                udp_twice: true,
                ..ShadowsocksScript::new(method, &udp_password(method))
            },
        ] {
            let method = script.method;
            let fake = FakeShadowsocks::spawn(script).await;
            let out = udp_outbound(&fake, method, &udp_password(method));
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            for payload in [&b"one"[..], b"two", b"three"] {
                udp_roundtrip(carrier.as_ref(), echo, payload).await;
            }
        }
    }

    #[tokio::test]
    async fn a_2022_answer_off_the_clock_or_for_another_session_is_dropped() {
        let echo = udp_echo_server().await;
        let method = SsMethod::Blake3Aes256Gcm;
        let key = key_2022(method, 1);
        for script in [
            ShadowsocksScript {
                answer_skew: 3600,
                ..ShadowsocksScript::new(method, &key)
            },
            ShadowsocksScript {
                wrong_request_salt: true,
                ..ShadowsocksScript::new(method, &key)
            },
        ] {
            let fake = FakeShadowsocks::spawn(script).await;
            let out = udp_outbound(&fake, method, &key);
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            carrier.send_to(b"hello", &target(echo)).await.unwrap();
            no_udp_answer(carrier.as_ref()).await;
            assert_eq!(fake.datagrams().len(), 1, "the server relayed it");
        }
    }

    #[tokio::test]
    async fn the_server_drops_a_wrong_key_or_a_clock_an_hour_off() {
        let echo = udp_echo_server().await;
        let method = SsMethod::Blake3Aes128Gcm;
        let key = key_2022(method, 1);
        let setups = [
            (
                ShadowsocksScript::new(method, &key),
                key_2022(method, 9),
                false,
            ),
            (ShadowsocksScript::new(method, &key), key.clone(), true),
            (
                ShadowsocksScript::new(SsMethod::Aes128Gcm, "right"),
                "wrong".to_string(),
                false,
            ),
        ];
        for (script, password, behind) in setups {
            let method = script.method;
            let fake = FakeShadowsocks::spawn(script).await;
            let mut out = udp_outbound(&fake, method, &password);
            if behind {
                out = out.with_clock(an_hour_behind);
            }
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            carrier.send_to(b"hello", &target(echo)).await.unwrap();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while fake.udp_rejected() == 0 {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the server drops it"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(fake.datagrams().is_empty());
            no_udp_answer(carrier.as_ref()).await;
        }
    }

    /// Without `udp-relay=true` the policy carries no UDP (the manual: the
    /// server must allow it).
    #[tokio::test]
    async fn no_udp_without_udp_relay() {
        let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(SsMethod::Aes128Gcm, "pw")).await;
        let out = outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method=aes-128-gcm, password=pw",
            fake.addr().port()
        ));
        assert_eq!(out.udp(), UdpSupport::Unsupported);
        let err = out.open_udp(&ConnectOpts::default()).await.err().unwrap();
        assert_eq!(
            err.to_string(),
            "policy protocol not implemented: UDP without `udp-relay=true`"
        );
    }

    #[test]
    fn what_cannot_be_built_is_a_build_error_that_quotes_nothing() {
        let build_keyed = |method: SsMethod, password: &str, keys: Vec<Vec<u8>>| {
            ShadowsocksOutbound::new(
                "S",
                Target::new(HostName::parse("127.0.0.1"), 8388),
                &SsSpec {
                    method,
                    password: Secret::from(password),
                    keys: Secret::new(keys),
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
        let build = |method: SsMethod, password: &str| build_keyed(method, password, Vec::new());
        assert_eq!(build(SsMethod::Aes128Gcm, ""), "`password` is empty");
        // SS 2022 takes its keys, not the password (the spec has none here)
        assert_eq!(
            build(SsMethod::Blake3Aes128Gcm, "secret"),
            "`password` is empty"
        );
        assert_eq!(
            build_keyed(
                SsMethod::Blake3Aes256Gcm,
                "x",
                vec![vec![0; 32], vec![0; 16]]
            ),
            "`password` is not Base64 keys of the length `2022-blake3-aes-256-gcm` requires"
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
