//! `snell` outbound, versions 1 to 5 (manual: Policies › Snell; phase 2 M6
//! design 4, M8 design 4.3): optionally behind Shadow TLS and / or
//! simple-obfs, the version's record stream (`record`) keyed per direction
//! by Argon2id of the PSK (`kdf`). v1 to v3 write the chunks of the original
//! protocol (`chunked`), v4 its own records, and v5 speaks v4's wire format
//! over TCP.
//!
//! A request is `01 command 00 host-length host port`, the host as text
//! (an IP literal too, an IDN as its A-labels) and no client id; it waits
//! for the first payload and goes out in the same record (`tunnel`). The
//! command is Connect (`01`), or ConnectV2 (`05`) where connections are
//! reused — always on v2, never on v1 and v3, with `reuse=true` on v4 and
//! v5: a request whose two sides both ended hands its connection back to
//! the outbound's pool (`pool`), and the next request goes out on it.
//!
//! UDP came with v3 and needs no parameter: a connection of its own, never
//! pooled, to the policy's port through the same layers, carries every
//! datagram (`udp`). `udp-port` — the manual: "When performing UDP
//! forwarding, use another server port number" — is not followed.

mod chunked;
pub(crate) mod kdf;
mod pool;
pub(crate) mod record;
mod tunnel;
mod udp;

use crate::build::shadow_tls_client;
use crate::task::AbortOnDrop;
use crate::transport::Stack;
use crate::transport::obfs::ObfsClient;
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
use kdf::Psk;
use pool::Pool;
use record::{Format, SnellStream};
use rurge_config::HostName;
use rurge_config::spec::{ObfsMode, ShadowTlsOpts, SnellSpec, SnellVersion};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::sync::{Arc, OnceLock};
use tunnel::SnellTunnel;
use udp::SnellUdp;

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
    format: Format,
}

impl Dialer {
    pub(crate) async fn fresh(&self, opts: &ConnectOpts) -> Result<SnellStream, OutboundError> {
        let transport = self.stack.open(opts).await?;
        Ok(SnellStream::open(transport, self.psk.clone(), self.format).await?)
    }
}

/// No `Debug`: the dialer holds the PSK.
pub struct SnellOutbound {
    name: String,
    dialer: Arc<Dialer>,
    /// `CONNECT`, or `CONNECT_V2` where connections are reused.
    command: u8,
    /// `Some` where connections are reused.
    pool: Option<Arc<Pool>>,
    /// Version 3 and later.
    udp: bool,
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
        // dry build both prefix it; the configuration checked all of this,
        // but a spec made by hand may not be
        if spec.psk.expose().is_empty() {
            return Err(BuildError::new("`psk` is empty"));
        }
        // versions 1 to 3 take `tls` too
        let http_only = !spec.version.is_chunked();
        if http_only && spec.obfs.as_ref().is_some_and(|o| o.mode != ObfsMode::Http) {
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
        let format = match spec.version {
            SnellVersion::V1 => Format::ChunkedChaCha,
            SnellVersion::V2 | SnellVersion::V3 => Format::ChunkedAes,
            SnellVersion::V4 | SnellVersion::V5 => Format::V4,
        };
        // v2 always reuses a connection and v1 and v3 never do, whatever
        // the spec says; on v4 and v5 it decides
        let reuse = match spec.version {
            SnellVersion::V2 => true,
            SnellVersion::V1 | SnellVersion::V3 => false,
            SnellVersion::V4 | SnellVersion::V5 => spec.reuse,
        };
        let dialer = Arc::new(Dialer {
            stack,
            psk: Psk::new(spec.psk.expose()),
            format,
        });
        Ok(SnellOutbound {
            name: name.to_string(),
            dialer,
            command: if reuse { CONNECT_V2 } else { CONNECT },
            pool: reuse.then(Arc::<Pool>::default),
            udp: spec.version.carries_udp(),
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

    #[cfg(any(test, feature = "testing"))]
    fn idle_connections(&self) -> Option<usize> {
        self.pool.as_ref().map(|pool| pool.len())
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

    /// v3 and later carry UDP whatever the policy says.
    fn udp(&self) -> UdpSupport {
        if self.udp {
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
            if !self.udp {
                return Err(OutboundError::Unsupported(
                    "UDP below `snell` version 3".to_string(),
                ));
            }
            // one budget for the connection, its layers, the key and the
            // server's answer, which comes at once
            let open = async {
                let stream = self.dialer.fresh(opts).await?;
                let udp = SnellUdp::open(stream).await?;
                Ok(Box::new(udp) as BoxedPacketSocket)
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
    use crate::testing::{FakeSnell, SnellScript, echo_server, udp_echo_server};
    use rurge_config::Span;
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::spec::snell::read_snell;
    use rurge_config::spec::{ObfsOpts, ParamReader, Secret};
    use rurge_net::connector::{DirectConnector, PacketSocket, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
    use tokio::net::TcpListener;

    /// The outbound for `definition` (a `snell, host, port, ...` line).
    fn outbound(definition: &str) -> SnellOutbound {
        outbound_through(
            definition,
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
    }

    fn outbound_through(definition: &str, connector: Arc<dyn Connector>) -> SnellOutbound {
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
            connector,
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

    /// A server of `version`, and what else `script` says.
    async fn server_of(version: u8, script: SnellScript) -> FakeSnell {
        FakeSnell::spawn(SnellScript { version, ..script }).await
    }

    /// An outbound of `version` to `fake`, with `extra` parameters.
    fn outbound_of(version: u8, fake: &FakeSnell, extra: &str) -> SnellOutbound {
        outbound(&format!(
            "snell, 127.0.0.1, {}, psk=secret, version={version}{extra}",
            fake.addr().port()
        ))
    }

    const NO_ANSWER: &str =
        "snell: the server closed the connection without answering (wrong psk or version?)";
    const RESET: &str =
        "snell: the server reset the connection without answering (wrong psk or version?)";
    const UNDECRYPTABLE: &str =
        "snell: the server's data failed to decrypt (wrong psk or version?)";

    /// Writes `hello` on a fresh request and reads the failure, which must
    /// come within the bound.
    async fn failure(out: &SnellOutbound, to: SocketAddr) -> std::io::Error {
        let mut stream = out
            .connect_tcp(&target(to), &ConnectOpts::default())
            .await
            .expect("connecting succeeds");
        stream.write_all(b"hello").await.unwrap();
        let mut answer = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer))
            .await
            .expect("a failure within the bound, not a wait")
            .unwrap_err()
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
            assert_eq!(out.udp(), UdpSupport::Native);
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

    /// Versions 1 to 3 against a server of the same version; a line
    /// without `version` is version 1. Connect on 1 and 3, ConnectV2 on 2.
    #[tokio::test]
    async fn v1_to_v3_carry_the_head_with_the_first_payload_in_one_chunk() {
        let echo = echo_server().await;
        for (line, version, command) in [
            ("", 1, CONNECT),
            (", version=1", 1, CONNECT),
            (", version=2", 2, CONNECT_V2),
            (", version=3", 3, CONNECT),
        ] {
            let fake = server_of(version, SnellScript::new("secret")).await;
            let out = outbound(&format!(
                "snell, 127.0.0.1, {}, psk=secret{line}",
                fake.addr().port()
            ));
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
                (command, "127.0.0.1", echo.port()),
                "v{version}"
            );
            assert!(seen[0].client_id.is_empty());
            assert_eq!(seen[0].early, b"hello through snell", "v{version}");
            assert_eq!(seen[0].padding, 0, "a chunk has no padding");
        }
    }

    /// Version 2 always reuses, whatever `reuse` says: three requests, one
    /// connection, each ended by a zero-length chunk each way.
    #[tokio::test]
    async fn v2_carries_its_requests_one_after_another_on_one_connection() {
        let echo = echo_server().await;
        for extra in ["", ", reuse=false"] {
            let fake = server_of(2, SnellScript::new("secret")).await;
            let out = outbound_of(2, &fake, extra);
            for text in [&b"one"[..], b"two", b"three"] {
                request(&out, echo, text).await;
            }
            assert_eq!(fake.connections(), 1, "{extra}");
            assert_eq!(places(&fake), [(0, 0), (0, 1), (0, 2)]);
            let seen = fake.requests();
            assert!(seen.iter().all(|r| r.command == CONNECT_V2));
            assert_eq!(seen[1].early, b"two");
            assert_eq!(out.idle_connections(), Some(1), "{extra}");
        }
    }

    /// The server closes a reused connection on the next request: the
    /// request goes again on a fresh one, and its user sees no failure.
    #[tokio::test]
    async fn a_pooled_v2_connection_the_server_closed_is_retried_on_a_fresh_one() {
        let echo = echo_server().await;
        let script = SnellScript {
            tunnels_per_connection: Some(1),
            ..SnellScript::new("secret")
        };
        let fake = server_of(2, script).await;
        let out = outbound_of(2, &fake, "");
        request(&out, echo, b"first").await;
        // taken from the pool; the server closes on the request
        request(&out, echo, b"second").await;
        assert_eq!(fake.unanswered(), 1);
        assert_eq!(fake.connections(), 2);
        assert_eq!(places(&fake), [(0, 0), (1, 0)]);
        // the fresh connection carried the head and the payload again
        assert_eq!(fake.requests()[1].early, b"second");
        // and was pooled in turn
        request(&out, echo, b"third").await;
        assert_eq!(places(&fake), [(0, 0), (1, 0), (2, 0)]);
        assert_eq!(fake.unanswered(), 2);
    }

    /// Dropped before either side ended, a v2 request ends in the
    /// background — our zero-length chunk, then the server's — and its
    /// connection carries the next request.
    #[tokio::test]
    async fn a_v2_request_dropped_before_its_end_finishes_in_the_background_and_is_reused() {
        let echo = echo_server().await;
        let fake = server_of(2, SnellScript::new("secret")).await;
        let out = outbound_of(2, &fake, "");
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"left open").await;
        drop(stream);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while out.idle_connections() != Some(1) {
            assert!(tokio::time::Instant::now() < deadline, "pooled again");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        request(&out, echo, b"next").await;
        assert_eq!(places(&fake), [(0, 0), (0, 1)]);
    }

    /// Versions 1 and 3 never reuse, whatever `reuse` says: every request
    /// has a connection of its own, ended by the close.
    #[tokio::test]
    async fn v1_and_v3_give_every_request_a_connection_of_its_own() {
        let echo = echo_server().await;
        for version in [1, 3] {
            for extra in ["", ", reuse=true"] {
                let fake = server_of(version, SnellScript::new("secret")).await;
                let out = outbound_of(version, &fake, extra);
                for text in [&b"one"[..], b"two"] {
                    request(&out, echo, text).await;
                }
                assert_eq!(fake.connections(), 2, "v{version}{extra}");
                assert_eq!(places(&fake), [(0, 0), (1, 0)]);
                assert!(fake.requests().iter().all(|r| r.command == CONNECT));
                assert_eq!(out.idle_connections(), None, "v{version}{extra}");
            }
        }
    }

    #[tokio::test]
    async fn a_large_payload_crosses_many_chunks_both_ways_on_v1_to_v3() {
        let echo = echo_server().await;
        let data: Vec<u8> = (0..1 << 20).map(|i: u32| (i % 251) as u8).collect();
        for version in 1..=3 {
            let fake = server_of(version, SnellScript::new("secret")).await;
            let out = outbound_of(version, &fake, "");
            let stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
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
            assert!(back == data, "v{version}: the echo differs");
            writer.await.unwrap();
            // the fake refuses anything longer
            assert_eq!(fake.largest_record(), record::MAX_PAYLOAD, "full chunks");
        }
    }

    /// Our end reaches the target as its end-of-file — the zero-length chunk
    /// of version 2, the close of versions 1 and 3 — and the answer the
    /// target then sends still arrives.
    #[tokio::test]
    async fn a_half_close_reaches_the_target_on_v1_to_v3() {
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
        for (version, connections) in [(1, 2), (2, 1), (3, 2)] {
            let fake = server_of(version, SnellScript::new("secret")).await;
            let out = outbound_of(version, &fake, "");
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
                assert_eq!(answer, b"got 5", "v{version}");
            }
            assert_eq!(fake.connections(), connections, "v{version}");
        }
    }

    #[tokio::test]
    async fn a_silent_client_sends_its_head_alone_on_v1_to_v3() {
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
        for version in 1..=3 {
            let fake = server_of(version, SnellScript::new("secret")).await;
            let out = outbound_of(version, &fake, "");
            let mut stream = out
                .connect_tcp(&target(greeter), &ConnectOpts::default())
                .await
                .unwrap();
            let mut greeting = [0u8; 9];
            tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut greeting))
                .await
                .expect("the greeting arrives")
                .unwrap();
            assert_eq!(&greeting, b"220 ready", "v{version}");
            roundtrip(&mut stream, b"HELO").await;
            assert!(fake.requests()[0].early.is_empty(), "v{version}");
        }
    }

    /// `tls` in front of version 1 and of version 2, `http` in front of
    /// version 3: one camouflage per connection.
    #[tokio::test]
    async fn through_obfs_on_v1_to_v3() {
        let echo = echo_server().await;
        for (version, mode, connections) in [(1, "tls", 2), (2, "tls", 1), (3, "http", 2)] {
            let script = SnellScript {
                obfs_tls: mode == "tls",
                obfs_http: mode == "http",
                ..SnellScript::new("secret")
            };
            let fake = server_of(version, script).await;
            let port = fake.addr().port();
            let extra = format!(", obfs={mode}, obfs-host=cdn.example");
            let out = outbound_of(version, &fake, &extra);
            request(&out, echo, b"behind the camouflage").await;
            let data = vec![0x5au8; 100_000];
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, &data).await;
            let hellos = fake.obfs_seen();
            assert_eq!(hellos.len(), connections, "v{version} behind {mode}");
            // a server name carries no port, a `Host` does
            let (host, uri) = match mode {
                "tls" => ("cdn.example".to_string(), None),
                _ => (format!("cdn.example:{port}"), Some("/")),
            };
            assert_eq!(hellos[0].host, host, "v{version}");
            assert_eq!(hellos[0].uri.as_deref(), uri, "v{version}");
            assert_eq!(fake.requests()[0].early, b"behind the camouflage");
        }
    }

    #[tokio::test]
    async fn the_servers_refusal_is_the_error_on_v1_to_v3() {
        let echo = echo_server().await;
        for version in 1..=3 {
            let script = SnellScript {
                refuse: Some((0x05, b"no such host\r\n".to_vec())),
                ..SnellScript::new("secret")
            };
            let fake = server_of(version, script).await;
            let out = outbound_of(version, &fake, "");
            for _ in 0..2 {
                let err = failure(&out, echo).await;
                assert_eq!(err.to_string(), "snell: the server refused: no such host");
                assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
            }
            // a refused request's connection is not reused, on version 2 either
            assert_eq!(fake.connections(), 2, "v{version}");
        }
    }

    /// A server that cannot open the first chunk says nothing and closes:
    /// the error names the two things that can be wrong, and not the PSK.
    #[tokio::test]
    async fn a_wrong_psk_is_a_connection_closed_without_an_answer_on_v1_to_v3() {
        let echo = echo_server().await;
        for version in 1..=3 {
            let fake = server_of(version, SnellScript::new("right")).await;
            let out = outbound(&format!(
                "snell, 127.0.0.1, {}, psk=wr0ngPsk, version={version}",
                fake.addr().port()
            ));
            let err = failure(&out, echo).await;
            assert_eq!(err.to_string(), NO_ANSWER, "v{version}");
            assert!(!format!("{err:?}").contains("wr0ngPsk"));
            assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
        }
    }

    /// The line's version is not the server's — a line without `version`,
    /// which dials version 1, against a server that speaks version 4 above
    /// all. Neither side can read the other, and the request fails within
    /// the bound with the hint, whichever way the server shows it.
    #[tokio::test]
    async fn a_server_of_another_version_fails_soon_and_says_psk_or_version() {
        let echo = echo_server().await;
        for (server, line) in [
            (4, ""),
            (4, ", version=1"),
            (4, ", version=2"),
            (4, ", version=3"),
            (1, ", version=4"),
            (2, ", version=5"),
            (1, ", version=2"),
            (3, ", version=1"),
        ] {
            let fake = server_of(server, SnellScript::new("secret")).await;
            let out = outbound(&format!(
                "snell, 127.0.0.1, {}, psk=secret{line}",
                fake.addr().port()
            ));
            let text = failure(&out, echo).await.to_string();
            assert!(
                text == NO_ANSWER || text == UNDECRYPTABLE,
                "a v{server} server and `{line}`: {text}"
            );
            assert!(text.ends_with("(wrong psk or version?)"), "{text}");
            assert!(!text.contains("secret"), "{text}");
            assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
        }
    }

    /// A target that ends before it sent anything: the server answers with
    /// an error — `Remote EOF`, whichever command asked — and the request's
    /// connection is not kept, on version 2 either, whose server leaves it
    /// open for another request.
    #[tokio::test]
    async fn a_target_that_ends_before_sending_is_the_servers_remote_eof_on_v1_to_v3() {
        // accepts and closes at once
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closer = listener.local_addr().unwrap();
        tokio::spawn(async move { while listener.accept().await.is_ok() {} });
        let echo = echo_server().await;
        for version in 1..=3 {
            let fake = server_of(version, SnellScript::new("secret")).await;
            let out = outbound_of(version, &fake, "");
            let err = failure(&out, closer).await;
            assert_eq!(
                err.to_string(),
                "snell: the server refused: Remote EOF",
                "v{version}"
            );
            assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
            assert_eq!(out.idle_connections().unwrap_or(0), 0, "v{version}");
            request(&out, echo, b"next").await;
            assert_eq!(fake.connections(), 2, "v{version}: the next one dials");
        }
    }

    /// A server that cannot read the request may close with our bytes still
    /// unread, which resets the connection: before anything of its own
    /// arrived that is its answer, as a close is, and says the same hint. A
    /// line without `version` against a server of version 4, and another
    /// PSK on a chunked and on a v4 connection.
    #[tokio::test]
    async fn a_server_that_resets_without_answering_says_psk_or_version() {
        let echo = echo_server().await;
        for (server, psk, line) in [
            (4, "secret", ""),
            (2, "0therPsk", ", version=2"),
            (5, "0therPsk", ", version=5"),
        ] {
            let script = SnellScript {
                reset: true,
                ..SnellScript::new(psk)
            };
            let fake = server_of(server, script).await;
            let out = outbound(&format!(
                "snell, 127.0.0.1, {}, psk=secret{line}",
                fake.addr().port()
            ));
            let err = failure(&out, echo).await;
            assert_eq!(err.to_string(), RESET, "a v{server} server and `{line}`");
            assert!(
                matches!(
                    err.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ),
                "{:?}",
                err.kind()
            );
            assert!(!format!("{err:?}").contains("secret"));
            assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
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

    /// Opens the first connection at once and every later one after
    /// `SLOW`: a redial that both halves of a tunnel come to poll.
    struct SlowAfterFirst {
        direct: DirectConnector,
        dials: std::sync::atomic::AtomicUsize,
    }

    const SLOW: Duration = Duration::from_millis(300);

    impl Connector for SlowAfterFirst {
        fn connect<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, std::io::Result<BoxedStream>> {
            use std::sync::atomic::Ordering;
            Box::pin(async move {
                if self.dials.fetch_add(1, Ordering::SeqCst) > 0 {
                    tokio::time::sleep(SLOW).await;
                }
                self.direct.connect(target, opts).await
            })
        }
    }

    /// The halves of a tunnel polled from tasks of their own (VMess UDP over
    /// this one reads in a task apart from its writes): the redial of a
    /// stale pooled connection wakes them both.
    #[tokio::test]
    async fn a_redial_wakes_both_halves_of_the_tunnel() {
        use std::sync::atomic::Ordering;
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            tunnels_per_connection: Some(1),
            ..SnellScript::new("secret")
        })
        .await;
        let connector = Arc::new(SlowAfterFirst {
            direct: DirectConnector::new(Arc::new(SystemResolve)),
            dials: Default::default(),
        });
        let out = outbound_through(
            &format!(
                "snell, 127.0.0.1, {}, psk=secret, version=4, reuse=true",
                fake.addr().port()
            ),
            connector.clone(),
        );
        request(&out, echo, b"first").await;
        let tunnel = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        let (mut r, mut w) = tokio::io::split(tunnel);
        let reader = tokio::spawn(async move {
            let mut back = [0u8; 10];
            r.read_exact(&mut back).await.map(|_| back)
        });
        // the server closes on the request: the reader starts the redial
        w.write_all(b"hello").await.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while connector.dials.load(Ordering::SeqCst) < 2 {
            assert!(tokio::time::Instant::now() < deadline, "no redial");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // the writer comes to the same redial
        w.write_all(b"world").await.unwrap();
        let back = tokio::time::timeout(Duration::from_secs(5), reader)
            .await
            .expect("the reader was left behind")
            .unwrap()
            .unwrap();
        assert_eq!(&back, b"helloworld");
    }

    #[tokio::test]
    async fn a_pooled_connection_that_breaks_after_the_window_is_not_retried() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            tunnels_per_connection: Some(1),
            ..SnellScript::new("secret")
        })
        .await;
        let out = outbound_to(&fake, ", reuse=true");
        request(&out, echo, b"first").await;
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        tokio::time::sleep(tunnel::STALE_WINDOW + Duration::from_millis(200)).await;
        stream.write_all(b"second").await.unwrap();
        let mut buf = [0u8; 16];
        let err = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("the failure arrives")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "snell: the server closed the connection without answering"
        );
        assert_eq!((fake.connections(), fake.unanswered()), (1, 1));
    }

    #[tokio::test]
    async fn an_answer_on_a_pooled_connection_that_fails_to_decrypt_is_not_retried() {
        let echo = echo_server().await;
        // v4 with `reuse=true`, and v2, which always reuses
        for (version, extra) in [(4, ", reuse=true"), (2, "")] {
            let script = SnellScript {
                garble_after: Some(1),
                ..SnellScript::new("secret")
            };
            let fake = server_of(version, script).await;
            let out = outbound_of(version, &fake, extra);
            request(&out, echo, b"first").await;
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            stream.write_all(b"second").await.unwrap();
            let mut buf = [0u8; 16];
            let err = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
                .await
                .expect("the failure arrives")
                .unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "v{version}");
            assert_eq!(
                err.to_string(),
                "snell: the server's data failed to decrypt (wrong psk or version?)"
            );
            assert_eq!((fake.connections(), fake.unanswered()), (1, 1));
        }
    }

    /// Dials directly; the reads of every connection dialled before
    /// `break_them` fail from then on, as a reset connection's do.
    struct Breakable {
        direct: DirectConnector,
        dials: std::sync::atomic::AtomicUsize,
        broken: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Breakable {
        fn break_them(&self) {
            use std::sync::atomic::Ordering;
            self.broken
                .store(self.dials.load(Ordering::SeqCst), Ordering::SeqCst);
        }
    }

    struct BreakableStream {
        inner: BoxedStream,
        /// Which dial this is, from 0.
        dial: usize,
        broken: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl AsyncRead for BreakableStream {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let this = self.get_mut();
            if this.dial < this.broken.load(std::sync::atomic::Ordering::SeqCst) {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "reset by the peer",
                )));
            }
            Pin::new(&mut this.inner).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for BreakableStream {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Pin::new(&mut self.get_mut().inner).poll_write(cx, data)
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_flush(cx)
        }

        fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
        }
    }

    impl Connector for Breakable {
        fn connect<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, std::io::Result<BoxedStream>> {
            use std::sync::atomic::Ordering;
            Box::pin(async move {
                let dial = self.dials.fetch_add(1, Ordering::SeqCst);
                let inner = self.direct.connect(target, opts).await?;
                let broken = self.broken.clone();
                Ok(Box::new(BreakableStream {
                    inner,
                    dial,
                    broken,
                }) as BoxedStream)
            })
        }
    }

    /// A pooled connection that is reset, not closed, is a stale one all
    /// the same, and its request goes again on a fresh connection: only
    /// before anything of the server's arrived is a reset an answer, and a
    /// pooled connection is long past that.
    #[tokio::test]
    async fn a_pooled_connection_that_is_reset_is_retried_on_a_fresh_one() {
        use std::sync::atomic::Ordering;
        let echo = echo_server().await;
        for (version, extra) in [(4, ", reuse=true"), (2, "")] {
            let fake = server_of(version, SnellScript::new("secret")).await;
            let connector = Arc::new(Breakable {
                direct: DirectConnector::new(Arc::new(SystemResolve)),
                dials: Default::default(),
                broken: Default::default(),
            });
            let out = outbound_through(
                &format!(
                    "snell, 127.0.0.1, {}, psk=secret, version={version}{extra}",
                    fake.addr().port()
                ),
                connector.clone(),
            );
            request(&out, echo, b"first").await;
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            // the pooled connection is in the request's hands: now it breaks
            assert_eq!(connector.dials.load(Ordering::SeqCst), 1, "v{version}");
            connector.break_them();
            roundtrip(&mut stream, b"second").await;
            assert_eq!(connector.dials.load(Ordering::SeqCst), 2, "v{version}");
            assert_eq!(fake.connections(), 2, "v{version}");
        }
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
                "snell: the server closed the connection without answering (wrong psk or version?)"
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

    /// Version 2 always reuses a connection and versions 1 and 3 never do:
    /// a spec made by hand that says otherwise is not followed. On versions
    /// 4 and 5 the spec decides.
    #[test]
    fn the_version_decides_about_reuse_whatever_a_hand_made_spec_says() {
        for (version, written, command, pooled) in [
            (SnellVersion::V1, true, CONNECT, false),
            (SnellVersion::V1, false, CONNECT, false),
            (SnellVersion::V2, false, CONNECT_V2, true),
            (SnellVersion::V2, true, CONNECT_V2, true),
            (SnellVersion::V3, true, CONNECT, false),
            (SnellVersion::V3, false, CONNECT, false),
            (SnellVersion::V4, true, CONNECT_V2, true),
            (SnellVersion::V4, false, CONNECT, false),
            (SnellVersion::V5, true, CONNECT_V2, true),
            (SnellVersion::V5, false, CONNECT, false),
        ] {
            let spec = SnellSpec {
                version,
                psk: Secret::from("secret"),
                reuse: written,
                obfs: None,
            };
            let out = SnellOutbound::new(
                "N",
                Target::new(HostName::parse("127.0.0.1"), 8388),
                &spec,
                None,
                Arc::new(RootCertStore::empty()),
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            )
            .unwrap_or_else(|e| panic!("{version:?}: {}", e.message));
            assert_eq!(
                (out.command, out.pool.is_some()),
                (command, pooled),
                "{version:?} with reuse={written}"
            );
        }
    }

    /// Versions 1 to 3 take `tls` too; 4 and 5 are checked above.
    #[test]
    fn versions_1_to_3_build_with_either_obfs() {
        for version in [SnellVersion::V1, SnellVersion::V2, SnellVersion::V3] {
            for mode in [ObfsMode::Tls, ObfsMode::Http] {
                let spec = SnellSpec {
                    version,
                    psk: Secret::from("secret"),
                    reuse: version == SnellVersion::V2,
                    obfs: Some(ObfsOpts {
                        mode,
                        host: None,
                        uri: "/".into(),
                    }),
                };
                let built = SnellOutbound::new(
                    "N",
                    Target::new(HostName::parse("127.0.0.1"), 8388),
                    &spec,
                    None,
                    Arc::new(RootCertStore::empty()),
                    Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
                );
                assert!(built.is_ok(), "{version:?} behind {mode:?}");
            }
        }
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(Duration::from_secs(10), carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_roundtrip(carrier: &dyn PacketSocket, to: SocketAddr, payload: &[u8]) {
        carrier.send_to(payload, &target(to)).await.unwrap();
        assert_eq!(udp_answer(carrier).await, (payload.to_vec(), target(to)));
    }

    /// v4 and v5 alike, without `udp-relay`: one connection asks for UDP,
    /// and each datagram names its target.
    #[tokio::test]
    async fn udp_goes_through_one_connection_to_any_target() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        for version in [4, 5] {
            let fake = FakeSnell::spawn(SnellScript {
                connect_to: Some(two),
                ..SnellScript::new("secret")
            })
            .await;
            let out = outbound(&format!(
                "snell, 127.0.0.1, {}, psk=secret, version={version}",
                fake.addr().port()
            ));
            assert_eq!(out.udp(), UdpSupport::Native);
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            udp_roundtrip(carrier.as_ref(), one, b"to one").await;
            // a name goes to the server as its A-labels; the answer names
            // its source by address
            let name = Target::new(HostName::Domain("bücher.example".into()), 53);
            carrier.send_to(b"by name", &name).await.unwrap();
            assert_eq!(
                udp_answer(carrier.as_ref()).await,
                (b"by name".to_vec(), target(two))
            );
            let seen = fake.requests();
            assert_eq!(seen.len(), 1, "v{version}");
            assert_eq!(seen[0].command, udp::UDP);
            assert!(seen[0].client_id.is_empty() && seen[0].early.is_empty());
            assert!((256..512).contains(&seen[0].padding), "{}", seen[0].padding);
            assert_eq!(
                fake.datagrams(),
                [one.to_string(), "xn--bcher-kva.example:53".to_string()]
            );
            assert_eq!(fake.connections(), 1);
        }
    }

    /// Version 3 carries UDP as 4 and 5 do, a datagram a chunk: two
    /// targets through one connection, and whoever reaches the server's
    /// socket is heard under its own address (full cone).
    #[tokio::test]
    async fn v3_carries_udp_through_one_connection_to_any_target() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let script = SnellScript {
            connect_to: Some(two),
            ..SnellScript::new("secret")
        };
        let fake = server_of(3, script).await;
        let out = outbound_of(3, &fake, "");
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), one, b"to one").await;
        let name = Target::new(HostName::Domain("bücher.example".into()), 53);
        carrier.send_to(b"by name", &name).await.unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"by name".to_vec(), target(two))
        );
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger
            .send_to(b"unasked", fake.udp_outside()[0])
            .await
            .unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), target(stranger.local_addr().unwrap()))
        );
        let seen = fake.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].command, udp::UDP);
        assert!(seen[0].client_id.is_empty() && seen[0].early.is_empty());
        assert_eq!(
            fake.datagrams(),
            [one.to_string(), "xn--bcher-kva.example:53".to_string()]
        );
        assert_eq!(fake.connections(), 1);
        // the largest datagram fills a chunk exactly
        let largest = vec![7u8; 16_374];
        udp_roundtrip(carrier.as_ref(), one, &largest).await;
        assert_eq!(fake.largest_record(), record::MAX_PAYLOAD);
    }

    /// UDP came with version 3: an outbound of version 1 or 2 says so
    /// without dialling.
    #[tokio::test]
    async fn v1_and_v2_carry_no_udp() {
        for version in [1, 2] {
            let fake = server_of(version, SnellScript::new("secret")).await;
            let out = outbound_of(version, &fake, "");
            assert_eq!(out.udp(), UdpSupport::Unsupported, "v{version}");
            let err = out
                .open_udp(&ConnectOpts::default())
                .await
                .err()
                .expect("no UDP");
            assert_eq!(
                err.to_string(),
                "policy protocol not implemented: UDP below `snell` version 3"
            );
            assert_eq!(fake.connections(), 0, "nothing was dialled");
        }
    }

    /// A server that answers the UDP request with an error fails the open,
    /// in chunks as in v4's records.
    #[tokio::test]
    async fn the_servers_refusal_of_udp_fails_the_open_on_v3() {
        let script = SnellScript {
            refuse: Some((0x07, b"udp disabled".to_vec())),
            ..SnellScript::new("secret")
        };
        let unwilling = server_of(3, script).await;
        let err = outbound_of(3, &unwilling, "")
            .open_udp(&ConnectOpts::default())
            .await
            .err()
            .expect("refused");
        assert_eq!(err.to_string(), "snell: the server refused: udp disabled");
    }

    /// Full cone: whoever reaches the server's socket is heard, under its
    /// own address.
    #[tokio::test]
    async fn anyone_may_answer_through_snell() {
        let echo = udp_echo_server().await;
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, "");
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"hello").await;
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger
            .send_to(b"unasked", fake.udp_outside()[0])
            .await
            .unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), target(stranger.local_addr().unwrap()))
        );
    }

    #[tokio::test]
    async fn a_servers_record_that_is_no_datagram_is_dropped() {
        let echo = udp_echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            udp_junk: true,
            ..SnellScript::new("secret")
        })
        .await;
        let out = outbound_to(&fake, "");
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"first").await;
        udp_roundtrip(carrier.as_ref(), echo, b"second").await;
    }

    #[tokio::test]
    async fn a_datagram_longer_than_a_record_is_refused_and_nothing_is_sent() {
        let echo = udp_echo_server().await;
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, "");
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let err = carrier
            .send_to(&vec![0u8; 16_375], &target(echo))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "snell: a datagram longer than 16374 bytes");
        // the largest one fits a record exactly
        let largest = vec![7u8; 16_374];
        udp_roundtrip(carrier.as_ref(), echo, &largest).await;
        assert_eq!(fake.datagrams(), [echo.to_string()]);
        assert_eq!(fake.largest_record(), record::MAX_PAYLOAD);
    }

    #[tokio::test]
    async fn udp_has_a_connection_of_its_own_never_pooled() {
        let (tcp_echo, echo) = (echo_server().await, udp_echo_server().await);
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, ", reuse=true");
        request(&out, tcp_echo, b"pooled").await;
        let pool = out.pool.clone().unwrap();
        assert_eq!(pool.len(), 1);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"fresh").await;
        drop(carrier);
        assert_eq!(fake.connections(), 2);
        assert_eq!(
            pool.len(),
            1,
            "UDP took nothing from the pool, gave nothing back"
        );
        assert_eq!(places(&fake), [(0, 0), (1, 0)]);
    }

    #[tokio::test]
    async fn the_servers_refusal_of_udp_fails_the_open() {
        let fake = FakeSnell::spawn(SnellScript {
            refuse: Some((0x07, b"udp disabled".to_vec())),
            ..SnellScript::new("secret")
        })
        .await;
        let out = outbound_to(&fake, "");
        let err = out
            .open_udp(&ConnectOpts::default())
            .await
            .err()
            .expect("refused");
        assert_eq!(err.to_string(), "snell: the server refused: udp disabled");
        // a server that never answers is the open's time-out
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((tcp, _)) = listener.accept().await {
                held.push(tcp);
            }
        });
        let out = outbound(&format!(
            "snell, 127.0.0.1, {}, psk=secret, version=4",
            silent.port()
        ));
        let opts = ConnectOpts {
            timeout: Duration::from_millis(300),
        };
        let err = out.open_udp(&opts).await.err().expect("times out");
        assert!(matches!(err, OutboundError::Timeout), "{err}");
    }
}
