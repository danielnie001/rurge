//! `snell` outbound, versions 4 and 5 (manual: Policies › Snell; phase 2 M6
//! design 4): optionally behind Shadow TLS and / or simple-obfs `http`, the
//! record stream (`record`) keyed per direction by Argon2id of the PSK
//! (`kdf`). v5 speaks v4's wire format over TCP.
//!
//! A request is `01 command 00 host-length host port`, the host as text
//! (an IP literal too, an IDN as its A-labels) and no client id; it waits
//! for the first payload and goes out in the same record (`tunnel`). The
//! command is Connect (`01`), or ConnectV2 (`05`) with `reuse=true`: a
//! request whose two sides both ended hands its connection back to the
//! outbound's pool (`pool`), and the next request goes out on it.

pub(crate) mod kdf;
mod pool;
pub(crate) mod record;
mod tunnel;

use crate::build::shadow_tls_client;
use crate::task::AbortOnDrop;
use crate::transport::Stack;
use crate::transport::obfs::ObfsClient;
use crate::{BuildError, Outbound, OutboundError};
use kdf::Psk;
use pool::Pool;
use record::SnellStream;
use rurge_config::HostName;
use rurge_config::spec::{ObfsMode, ShadowTlsOpts, SnellSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::sync::{Arc, OnceLock};
use tunnel::SnellTunnel;

/// The request's version byte.
const REQUEST_VERSION: u8 = 0x01;
const CONNECT: u8 = 0x01;
/// Connect on a connection that may carry further requests.
const CONNECT_V2: u8 = 0x05;

/// Opens fresh connections: the transport, then a record stream with a salt
/// of its own. No `Debug`: it holds the PSK.
pub(crate) struct Dialer {
    stack: Stack,
    psk: Psk,
}

impl Dialer {
    pub(crate) async fn fresh(&self, opts: &ConnectOpts) -> Result<SnellStream, OutboundError> {
        let transport = self.stack.open(opts).await?;
        Ok(SnellStream::open(transport, self.psk.clone()).await?)
    }
}

/// No `Debug`: the dialer holds the PSK.
pub struct SnellOutbound {
    name: String,
    dialer: Arc<Dialer>,
    /// `CONNECT`, or `CONNECT_V2` with `reuse=true`.
    command: u8,
    /// `Some` with `reuse=true`.
    pool: Option<Arc<Pool>>,
    /// Started by the first connection: building an outbound (a dry build
    /// included) leaves no task behind.
    reaper: OnceLock<AbortOnDrop>,
}

impl SnellOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &SnellSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<SnellOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and the
        // dry build both prefix it; the configuration checked both, but a
        // spec made by hand may not be
        if spec.psk.expose().is_empty() {
            return Err(BuildError::new("`psk` is empty"));
        }
        if spec.obfs.as_ref().is_some_and(|o| o.mode != ObfsMode::Http) {
            return Err(BuildError::new(
                "`snell` versions 4 and 5 take only `obfs=http`",
            ));
        }
        // no TLS of its own: the camouflage certificate is checked against
        // the server's name
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
        Ok(SnellOutbound {
            name: name.to_string(),
            dialer: Arc::new(Dialer {
                stack,
                psk: Psk::new(spec.psk.expose()),
            }),
            command: if spec.reuse { CONNECT_V2 } else { CONNECT },
            pool: spec.reuse.then(Arc::<Pool>::default),
            reaper: OnceLock::new(),
        })
    }

    async fn open(&self, head: Vec<u8>, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        let Some(pool) = &self.pool else {
            let stream = self.dialer.fresh(opts).await?;
            return Ok(Box::new(SnellTunnel::new(stream, head, None)));
        };
        self.reaper.get_or_init(|| pool::spawn_reaper(pool));
        let back = Arc::downgrade(pool);
        if let Some(stream) = pool.take() {
            return Ok(Box::new(SnellTunnel::reused(
                stream,
                head,
                back,
                self.dialer.clone(),
                opts.clone(),
            )));
        }
        let stream = self.dialer.fresh(opts).await?;
        Ok(Box::new(SnellTunnel::new(stream, head, Some(back))))
    }
}

/// `01 command 00 host-length host port`: no client id, the host as text.
fn request_head(command: u8, target: &Target) -> Result<Vec<u8>, OutboundError> {
    let host = match &target.host {
        // an IPv6 literal without brackets
        HostName::Ip(ip) => ip.to_string(),
        HostName::Domain(name) => crate::hostname::to_ascii(name).ok_or_else(|| {
            OutboundError::Proxy("snell: the host name cannot be sent to the server".to_string())
        })?,
    };
    let len = u8::try_from(host.len()).map_err(|_| {
        OutboundError::Proxy("snell: the host name is longer than 255 bytes".to_string())
    })?;
    let mut head = vec![REQUEST_VERSION, command, 0, len];
    head.extend_from_slice(host.as_bytes());
    head.extend_from_slice(&target.port.to_be_bytes());
    Ok(head)
}

impl Outbound for SnellOutbound {
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
            let head = request_head(self.command, target)?;
            // one budget for the connection, Shadow TLS, obfs and the key;
            // the server answers with the target's first data, in the relay
            match tokio::time::timeout(opts.timeout, self.open(head, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeSnell, SnellScript, echo_server};
    use rurge_config::Span;
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::spec::snell::read_snell;
    use rurge_config::spec::{ObfsOpts, ParamReader, Secret, SnellVersion};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// The outbound for `definition` (a `snell, host, port, ...` line).
    fn outbound(definition: &str) -> SnellOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("N", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let read = read_snell(&mut r);
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        assert_eq!(read.not_implemented_version, None);
        SnellOutbound::new(
            "N",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &read.spec,
            shadow_tls.as_ref(),
            Arc::new(RootCertStore::empty()),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// A version 4 outbound to `fake`, with `extra` parameters.
    fn outbound_to(fake: &FakeSnell, extra: &str) -> SnellOutbound {
        outbound(&format!(
            "snell, 127.0.0.1, {}, psk=secret, version=4{extra}",
            fake.addr().port()
        ))
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

    /// One whole request to the echo server: `text` there and back, then
    /// both sides end.
    async fn request(out: &SnellOutbound, echo: SocketAddr, text: &[u8]) {
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, text).await;
        stream.shutdown().await.unwrap();
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut rest))
            .await
            .expect("the server's end arrives")
            .unwrap();
        assert!(rest.is_empty());
    }

    /// (connection, tunnel) of every request the fake saw.
    fn places(fake: &FakeSnell) -> Vec<(usize, usize)> {
        fake.requests()
            .iter()
            .map(|r| (r.connection, r.tunnel))
            .collect()
    }

    #[test]
    fn the_request_head_names_the_host_as_text() {
        let head = |host: &str| request_head(CONNECT_V2, &Target::new(HostName::parse(host), 443));
        // the payload of the byte-level notes' worked example
        assert_eq!(
            head("example.com").unwrap(),
            crate::vmess::vectors::hex("0105000b6578616d706c652e636f6d01bb")
        );
        let v6 = [&[1, 5, 0, 3][..], b"::1", &[1, 0xbb]].concat();
        assert_eq!(head("::1").unwrap(), v6, "no brackets");
        let v4 = request_head(CONNECT, &Target::new(HostName::parse("10.0.0.1"), 80)).unwrap();
        assert_eq!(v4, [&[1, 1, 0, 8][..], b"10.0.0.1", &[0, 80]].concat());
        assert_eq!(
            &head("bücher.example").unwrap()[4..25],
            b"xn--bcher-kva.example"
        );
    }

    #[tokio::test]
    async fn v4_and_v5_carry_the_head_with_the_first_payload() {
        let echo = echo_server().await;
        for version in [4, 5] {
            let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
            let out = outbound(&format!(
                "snell, 127.0.0.1, {}, psk=secret, version={version}",
                fake.addr().port()
            ));
            assert_eq!(out.name(), "N");
            assert_eq!(out.udp(), crate::UdpSupport::Unsupported);
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"hello through snell").await;
            roundtrip(&mut stream, b"and again").await;
            let seen = fake.requests();
            assert_eq!(seen.len(), 1, "v{version}");
            assert_eq!(
                (seen[0].command, seen[0].host.as_str(), seen[0].port),
                (CONNECT, "127.0.0.1", echo.port())
            );
            assert!(seen[0].client_id.is_empty());
            assert_eq!(seen[0].early, b"hello through snell", "one record");
            assert!((256..512).contains(&seen[0].padding), "{}", seen[0].padding);
        }
    }

    #[tokio::test]
    async fn without_reuse_every_request_has_a_connection_of_its_own() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, "");
        assert!(out.pool.is_none());
        for text in [&b"one"[..], b"two"] {
            request(&out, echo, text).await;
        }
        assert_eq!(fake.connections(), 2);
        assert_eq!(places(&fake), [(0, 0), (1, 0)]);
        assert!(fake.requests().iter().all(|r| r.command == CONNECT));
    }

    #[tokio::test]
    async fn with_reuse_one_connection_carries_the_requests_one_after_another() {
        let echo = echo_server().await;
        for version in [4, 5] {
            let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
            let out = outbound(&format!(
                "snell, 127.0.0.1, {}, psk=secret, version={version}, reuse=true",
                fake.addr().port()
            ));
            for text in [&b"one"[..], b"two", b"three"] {
                request(&out, echo, text).await;
            }
            assert_eq!(fake.connections(), 1, "v{version}");
            assert_eq!(places(&fake), [(0, 0), (0, 1), (0, 2)]);
            let seen = fake.requests();
            assert!(seen.iter().all(|r| r.command == CONNECT_V2));
            assert_eq!(seen[1].early, b"two");
            // only a direction's first record is padded
            assert!((256..512).contains(&seen[0].padding));
            assert_eq!((seen[1].padding, seen[2].padding), (0, 0));
        }
    }

    #[tokio::test]
    async fn a_request_dropped_before_its_end_finishes_in_the_background_and_is_reused() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, ", reuse=true");
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"left open").await;
        drop(stream);
        let pool = out.pool.clone().unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while pool.len() == 0 {
            assert!(tokio::time::Instant::now() < deadline, "pooled again");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        request(&out, echo, b"next").await;
        assert_eq!(places(&fake), [(0, 0), (0, 1)]);
    }

    #[tokio::test]
    async fn a_request_without_its_answer_does_not_go_back_to_the_pool() {
        // accepts and says nothing: the fake answers only once it sends
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((tcp, _)) = listener.accept().await {
                held.push(tcp);
            }
        });
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, ", reuse=true");
        let mut stream = out
            .connect_tcp(&target(silent), &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"hello").await.unwrap();
        drop(stream);
        let echo = echo_server().await;
        request(&out, echo, b"next").await;
        assert_eq!(fake.connections(), 2);
    }

    #[tokio::test]
    async fn a_pooled_connection_the_server_retired_is_retried_on_a_fresh_one() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            tunnels_per_connection: Some(1),
            ..SnellScript::new("secret")
        })
        .await;
        let out = outbound_to(&fake, ", reuse=true");
        request(&out, echo, b"first").await;
        // taken from the pool; the server closes on the request
        request(&out, echo, b"second").await;
        assert_eq!(fake.unanswered(), 1);
        assert_eq!(fake.connections(), 2);
        assert_eq!(places(&fake), [(0, 0), (1, 0)]);
        // the fresh connection carried the head and the payload again
        let seen = fake.requests();
        assert_eq!(seen[1].early, b"second");
        assert!((256..512).contains(&seen[1].padding));
        // and was pooled in turn
        request(&out, echo, b"third").await;
        assert_eq!(places(&fake), [(0, 0), (1, 0), (2, 0)]);
        assert_eq!(fake.unanswered(), 2);
    }

    #[tokio::test]
    async fn the_servers_refusal_is_the_error_and_the_connection_is_not_reused() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            refuse: Some((0x05, b"no such host\r\n".to_vec())),
            ..SnellScript::new("secret")
        })
        .await;
        let out = outbound_to(&fake, ", reuse=true");
        for _ in 0..2 {
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            stream.write_all(b"hello").await.unwrap();
            let mut buf = [0u8; 16];
            let err = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
                .await
                .expect("the answer arrives")
                .unwrap_err();
            assert_eq!(err.to_string(), "snell: the server refused: no such host");
            assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
        }
        assert_eq!(fake.connections(), 2);
    }

    #[tokio::test]
    async fn a_target_that_ends_before_sending_is_the_servers_remote_eof() {
        // accepts and closes at once
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closer = listener.local_addr().unwrap();
        tokio::spawn(async move { while listener.accept().await.is_ok() {} });
        for (reuse, expected) in [
            ("true", "snell: the server refused: Remote EOF"),
            ("false", "snell: the server refused: end of file"),
        ] {
            let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
            let out = outbound_to(&fake, &format!(", reuse={reuse}"));
            let mut stream = out
                .connect_tcp(&target(closer), &ConnectOpts::default())
                .await
                .unwrap();
            stream.write_all(b"hello").await.unwrap();
            let mut buf = Vec::new();
            let err = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf))
                .await
                .expect("the answer arrives")
                .unwrap_err();
            assert_eq!(err.to_string(), expected);
        }
    }

    #[tokio::test]
    async fn a_wrong_psk_is_a_connection_closed_without_an_answer() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript::new("right")).await;
        for reuse in ["false", "true"] {
            let out = outbound(&format!(
                "snell, 127.0.0.1, {}, psk=wrong, version=5, reuse={reuse}",
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
                "snell: the server closed the connection without answering"
            );
        }
        assert_eq!((fake.rejected(), fake.requests().len()), (2, 0));
    }

    #[tokio::test]
    async fn through_obfs_http_once_per_connection() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            obfs_http: true,
            ..SnellScript::new("secret")
        })
        .await;
        let port = fake.addr().port();
        let out = outbound_to(&fake, ", reuse=true, obfs=http, obfs-host=cdn.example");
        request(&out, echo, b"behind the camouflage").await;
        request(&out, echo, b"on the same connection").await;
        let data = vec![0x5au8; 100_000];
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, &data).await;
        let hellos = fake.obfs_seen();
        assert_eq!(hellos.len(), 1);
        assert_eq!(hellos[0].host, format!("cdn.example:{port}"));
        assert_eq!(hellos[0].uri.as_deref(), Some("/"));
        assert_eq!(places(&fake), [(0, 0), (0, 1), (0, 2)]);
        assert_eq!(fake.requests()[0].early, b"behind the camouflage");
    }

    #[tokio::test]
    async fn names_go_out_as_a_labels_and_an_unsendable_name_never_dials() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            connect_to: Some(echo),
            ..SnellScript::new("secret")
        })
        .await;
        let out = outbound_to(&fake, "");
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
            (seen[0].host.as_str(), seen[0].port),
            ("xn--bcher-kva.example", 443)
        );
        let before = fake.connections();
        for (name, expected) in [
            (
                "a@b.test".to_string(),
                "snell: the host name cannot be sent to the server",
            ),
            (
                "a".repeat(256),
                "snell: the host name is longer than 255 bytes",
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
    async fn a_large_payload_crosses_many_records_both_ways() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, "");
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
        assert_eq!(fake.largest_record(), record::MAX_PAYLOAD, "full records");
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
        for (reuse, connections) in [("false", 2), ("true", 1)] {
            let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
            let out = outbound_to(&fake, &format!(", reuse={reuse}"));
            for _ in 0..2 {
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
                assert_eq!(answer, b"got 5", "reuse={reuse}");
            }
            assert_eq!(fake.connections(), connections, "reuse={reuse}");
        }
    }

    #[tokio::test]
    async fn a_silent_client_sends_its_head_alone_and_hears_the_target_first() {
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
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, ", reuse=true");
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
        assert!(fake.requests()[0].early.is_empty());
    }

    #[test]
    fn what_cannot_be_built_is_a_build_error() {
        let build = |psk: &str, obfs: Option<ObfsOpts>| {
            SnellOutbound::new(
                "N",
                Target::new(HostName::parse("127.0.0.1"), 8388),
                &SnellSpec {
                    version: SnellVersion::V5,
                    psk: Secret::from(psk),
                    reuse: false,
                    udp_port: None,
                    obfs,
                },
                None,
                Arc::new(RootCertStore::empty()),
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            )
            .map(|_| ())
            .map_err(|e| e.message)
        };
        assert_eq!(build("", None), Err("`psk` is empty".to_string()));
        let tls = ObfsOpts {
            mode: ObfsMode::Tls,
            host: None,
            uri: "/".into(),
        };
        assert_eq!(
            build("secret", Some(tls)),
            Err("`snell` versions 4 and 5 take only `obfs=http`".to_string())
        );
        assert_eq!(build("secret", None), Ok(()));
    }
}
