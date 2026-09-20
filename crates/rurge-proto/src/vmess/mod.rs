//! `vmess` outbound (manual: Policies › VMess): the AEAD handshake only,
//! optionally under TLS and / or a WebSocket. The sealed request head waits
//! in a `LazyHead` for the first payload; the body is chunked and sealed in
//! both directions (`stream`).
//!
//! The server never says why it refuses: a wrong id, or a clock more than
//! about two minutes off, both end as a connection closed without an answer.

pub(crate) mod chunk;
pub(crate) mod header;
pub(crate) mod kdf;
mod stream;
#[cfg(test)]
pub(crate) mod vectors;

use crate::addr::{AddrError, vmess_addr};
use crate::build::tls_client;
use crate::transport::Stack;
use crate::transport::lazy_head::LazyHead;
use crate::transport::ws::WsClient;
use crate::{BuildError, Outbound, OutboundError};
use header::{Security, Session};
use rurge_config::KeystoreItem;
use rurge_config::spec::{VmessCipher, VmessSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use stream::VmessStream;

/// No `Debug`: the command key is as good as the id.
pub struct VmessOutbound {
    name: String,
    stack: Stack,
    /// `MD5(id ‖ magic)`; the id itself is not kept.
    cmd_key: [u8; 16],
    security: Security,
}

fn random<const N: usize>() -> Result<[u8; N], OutboundError> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out)
        .map_err(|_| OutboundError::Proxy("vmess: no randomness available".to_string()))?;
    Ok(out)
}

impl VmessOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &VmessSpec,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<VmessOutbound, BuildError> {
        // error texts carry no policy name (the registry's `build_one` and
        // the dry build both prefix it). No ALPN unless the policy asks for
        // one: a WebSocket below must not be negotiated into h2 (M2 design 4.5)
        let tls = tls_client(spec.tls.as_ref(), &server.host, &[], keystore, roots)?;
        let ws = spec
            .ws
            .as_ref()
            .map(|ws| WsClient::new(ws, &server, tls.is_some()))
            .transpose()?;
        Ok(VmessOutbound {
            name: name.to_string(),
            stack: Stack::new(connector, server, tls, ws),
            cmd_key: header::cmd_key(spec.uuid.expose()),
            security: match spec.cipher {
                VmessCipher::Aes128Gcm => Security::Aes128Gcm,
                VmessCipher::ChaCha20Poly1305 => Security::ChaCha20Poly1305,
            },
        })
    }

    /// The sealed request head for `target`, and the secrets it announces.
    fn head(&self, target: &Target) -> Result<(Vec<u8>, Session), OutboundError> {
        let address = vmess_addr(target).map_err(|e| {
            OutboundError::Proxy(
                match e {
                    AddrError::Unsendable => "vmess: the host name cannot be sent to the server",
                    AddrError::TooLong => "vmess: the host name is longer than 255 bytes",
                }
                .to_string(),
            )
        })?;
        let session = Session {
            body_iv: random()?,
            body_key: random()?,
            response_v: random::<1>()?[0],
        };
        // the server accepts ±120 s; the reference client spreads its own
        // timestamps over ±30 s so that they say nothing about its clock
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let jitter = i64::from(u16::from_be_bytes(random()?) % 61) - 30;
        let auth_id = header::auth_id(&self.cmd_key, now + jitter, random()?);
        let padding: [u8; 16] = random()?;
        let padding = &padding[..usize::from(padding[15] % 16)];
        let plain = header::request_plain(&session, self.security, &address, padding);
        let head = header::seal_request(&self.cmd_key, &auth_id, &random()?, &plain);
        Ok((head, session))
    }
}

impl Outbound for VmessOutbound {
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
            let (head, session) = self.head(target)?;
            // one budget for the connection, TLS and the WebSocket handshake;
            // the server's answer comes with its first payload, in the relay
            let transport = match tokio::time::timeout(opts.timeout, self.stack.open(opts)).await {
                Ok(result) => result?,
                Err(_) => return Err(OutboundError::Timeout),
            };
            let lazy: BoxedStream = Box::new(LazyHead::new(transport, head));
            Ok(Box::new(VmessStream::new(lazy, session, self.security)) as BoxedStream)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeVmess, TlsFixture, VmessScript, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::vmess::read_vmess;
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";

    /// The outbound for `definition` (a `vmess, host, port, ...` line).
    fn outbound(definition: &str, roots: Arc<RootCertStore>) -> VmessOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("V", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let read = read_vmess(&mut r, &[]);
        assert!(read.aead, "the line asks for the legacy handshake");
        assert!(!r.has_errors(), "{:?}", r.finish());
        VmessOutbound::new(
            "V",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &read.spec,
            &[],
            roots,
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    fn no_roots() -> Arc<RootCertStore> {
        Arc::new(RootCertStore::empty())
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
        assert_eq!(buf, text);
    }

    #[tokio::test]
    async fn plain_vmess_carries_the_head_with_the_first_payload() {
        let echo = echo_server().await;
        let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true",
                fake.addr().port()
            ),
            no_roots(),
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"hello through vmess").await;
        let seen = fake.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (seen[0].command, seen[0].options, seen[0].security),
            (1, 0x05, 3),
            "TCP, ChunkStream + ChunkMasking, aes-128-gcm"
        );
        assert_eq!((seen[0].atyp, seen[0].host.as_str()), (1, "127.0.0.1"));
        assert_eq!(seen[0].port, echo.port());
        assert!(seen[0].early > 0, "the head went out alone");
        assert!(seen[0].skew.abs() <= 31, "{}", seen[0].skew);
        assert!(seen[0].padding < 16);
    }

    #[tokio::test]
    async fn chacha20_a_domain_target_tls_and_a_websocket() {
        let echo = echo_server().await;
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeVmess::spawn(
            VmessScript {
                ws: true,
                connect_to: Some(echo),
                ..VmessScript::new(ID)
            },
            Some(fixture.clone()),
        )
        .await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true, encrypt-method=chacha20-ietf-poly1305, tls=true, ws=true, ws-path=/v",
                fake.addr().port()
            ),
            fixture.roots(),
        );
        let name = Target::new(HostName::Domain("bücher.example".into()), 443);
        let mut stream = out
            .connect_tcp(&name, &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"over tls and a websocket").await;
        let seen = fake.requests();
        assert_eq!(seen[0].security, 4);
        assert_eq!(
            (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
            (2, "xn--bcher-kva.example", 443),
            "a name travels as its A-labels, and the server resolves it"
        );
        assert_eq!(fake.ws_seen()[0].path, "/v");
        // the fixture offers h2 first: an ALPN of ours would have picked it
        assert_eq!(fixture.seen()[0].alpn, None);
    }

    #[tokio::test]
    async fn a_wrong_id_shows_as_a_connection_closed_without_an_answer() {
        let echo = echo_server().await;
        let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username=0233d11c-15a4-47d3-ade3-48ffca0ce118, vmess-aead=true",
                fake.addr().port()
            ),
            no_roots(),
        );
        // the connection itself succeeds: the server only ever answers a request it accepts
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        let err = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
            .await
            .expect("the server closes within the bound")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "vmess: the server closed the connection without answering"
        );
        assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
        // nothing derived from the id is in the text
        assert!(!err.to_string().contains("0233"));
    }

    #[tokio::test]
    async fn a_server_that_speaks_first_is_heard() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let banner = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            s.write_all(b"220 ready\r\n").await.unwrap();
            let mut rest = Vec::new();
            let _ = s.read_to_end(&mut rest).await;
        });
        let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true",
                fake.addr().port()
            ),
            no_roots(),
        );
        let mut stream = out
            .connect_tcp(&target(banner), &ConnectOpts::default())
            .await
            .unwrap();
        let mut buf = [0u8; 11];
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut buf))
            .await
            .expect("the banner arrives within the bound")
            .unwrap();
        assert_eq!(&buf, b"220 ready\r\n");
        assert_eq!(
            fake.requests()[0].early,
            0,
            "nothing was written: the head went alone"
        );
    }

    #[tokio::test]
    async fn a_name_that_cannot_be_sent_never_dials() {
        let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true",
                fake.addr().port()
            ),
            no_roots(),
        );
        let bad = Target::new(HostName::Domain("a@b.test".into()), 80);
        let err = out
            .connect_tcp(&bad, &ConnectOpts::default())
            .await
            .err()
            .expect("refused");
        assert_eq!(
            err.to_string(),
            "vmess: the host name cannot be sent to the server"
        );
        assert_eq!(fake.connections(), 0);
    }

    #[tokio::test]
    async fn a_silent_server_is_a_timeout_of_the_whole_ladder() {
        // accepts and never speaks TLS
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = listener.accept().await {
                held.push(s);
            }
        });
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true, tls=true",
                addr.port()
            ),
            fixture.roots(),
        );
        let opts = ConnectOpts {
            timeout: Duration::from_millis(300),
        };
        let err = out
            .connect_tcp(&target(addr), &opts)
            .await
            .err()
            .expect("times out");
        assert!(matches!(err, OutboundError::Timeout), "{err}");
    }

    /// The engine's relay, reduced to what matters to a stream under test:
    /// `read` → `write_all` with no flush, `shutdown` at EOF, both directions
    /// polled from one task through `tokio::io::split`.
    async fn copy_half<R, W>(mut reader: R, mut writer: W) -> std::io::Result<u64>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        let mut buf = vec![0u8; 8 * 1024];
        let mut total = 0;
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                let _ = writer.shutdown().await;
                return Ok(total);
            }
            writer.write_all(&buf[..n]).await?;
            total += n as u64;
        }
    }

    #[tokio::test]
    async fn a_relay_moves_a_megabyte_each_way_with_either_cipher() {
        tokio::time::timeout(Duration::from_secs(120), async {
            for cipher in ["aes-128-gcm", "chacha20-ietf-poly1305"] {
                let echo = echo_server().await;
                let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
                let out = outbound(
                    &format!(
                        "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true, encrypt-method={cipher}",
                        fake.addr().port()
                    ),
                    no_roots(),
                );
                let upstream = out
                    .connect_tcp(&target(echo), &ConnectOpts::default())
                    .await
                    .unwrap();
                let (mut app, near) = tokio::io::duplex(64 * 1024);
                let relay = tokio::spawn(async move {
                    let (cr, cw) = tokio::io::split(near);
                    let (ur, uw) = tokio::io::split(upstream);
                    tokio::join!(copy_half(cr, uw), copy_half(ur, cw))
                });
                let payload: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
                let (mut app_r, mut app_w) = tokio::io::split(&mut app);
                let mut back = vec![0u8; payload.len()];
                tokio::join!(
                    async { app_w.write_all(&payload).await.unwrap() },
                    async { app_r.read_exact(&mut back).await.unwrap() }
                );
                assert!(back == payload, "{cipher}: the echo differs");
                drop((app_r, app_w));
                drop(app); // the client goes away: the relay must wind down by itself
                let (up, down) = relay.await.unwrap();
                assert_eq!((up.unwrap(), down.unwrap()), (1_000_000, 1_000_000), "{cipher}");
            }
        })
        .await
        .expect("bounded");
    }

    /// A transport that takes seven bytes at a time: every write parks again
    /// and again, with the read half polled in between from the same task.
    #[tokio::test]
    async fn parked_writes_are_finished_exactly_once() {
        use crate::vmess::chunk::ChunkCipher;
        let s = vectors::session();
        let (near, far) = tokio::io::duplex(7);
        let stream: BoxedStream = Box::new(VmessStream::new(
            Box::new(near),
            vectors::session(),
            Security::Aes128Gcm,
        ));
        let (mut far_r, mut far_w) = tokio::io::split(far);
        let server = tokio::spawn(async move {
            // answer first, then read what the client sealed
            let (key, iv) = header::response_secrets(&s);
            far_w
                .write_all(&vectors::hex(vectors::RESPONSE_SEALED))
                .await
                .unwrap();
            let mut down = ChunkCipher::new(Security::Aes128Gcm, &key, &iv);
            let mut out = Vec::new();
            down.seal(b"pong", &mut out);
            far_w.write_all(&out).await.unwrap();
            let mut up = ChunkCipher::new(Security::Aes128Gcm, &s.body_key, &s.body_iv);
            let mut got = Vec::new();
            loop {
                let mut len = [0u8; 2];
                far_r.read_exact(&mut len).await.unwrap();
                let mut sealed = vec![0u8; up.open_len(len)];
                far_r.read_exact(&mut sealed).await.unwrap();
                let n = up
                    .open(&mut sealed)
                    .expect("authentic: nothing was sent twice or out of order");
                if n == 0 {
                    return got;
                }
                got.extend_from_slice(&sealed[..n]);
            }
        });
        let (mut r, mut w) = tokio::io::split(stream);
        let payload: Vec<u8> = (0..50_000u32).map(|i| (i % 253) as u8).collect();
        let write = async {
            for piece in payload.chunks(3000) {
                w.write_all(piece).await.unwrap();
            }
            w.shutdown().await.unwrap();
        };
        let read = async {
            let mut buf = [0u8; 4];
            r.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"pong");
        };
        tokio::time::timeout(Duration::from_secs(30), async { tokio::join!(write, read) })
            .await
            .expect("bounded");
        assert!(server.await.unwrap() == payload);
    }

    #[tokio::test]
    async fn an_empty_write_does_not_end_the_stream() {
        let echo = echo_server().await;
        let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true",
                fake.addr().port()
            ),
            no_roots(),
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(stream.write(&[]).await.unwrap(), 0);
        roundtrip(&mut stream, b"after").await;
    }

    /// What the client reads when the server's bytes are `wire`.
    async fn read_error(wire: Vec<u8>) -> std::io::Error {
        let (near, mut far) = tokio::io::duplex(4096);
        let mut stream = VmessStream::new(Box::new(near), vectors::session(), Security::Aes128Gcm);
        far.write_all(&wire).await.unwrap();
        drop(far); // nothing more is coming
        let mut sink = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut sink))
            .await
            .expect("bounded")
            .expect_err("the stream must not end cleanly")
    }

    #[tokio::test]
    async fn a_damaged_or_truncated_response_is_an_error_not_an_end_of_stream() {
        let head = vectors::hex(vectors::RESPONSE_SEALED);
        let chunks = vectors::hex(vectors::RESPONSE_CHUNKS_AES);
        let with = |tail: &[u8]| [&head[..], tail].concat();
        // not a VMess answer at all (or our id is wrong and this is someone else's)
        let garbage = read_error(vec![0x55; 64]).await;
        assert_eq!(
            garbage.to_string(),
            "vmess: the response cannot be authenticated"
        );
        // one flipped bit inside the first chunk
        let mut flipped = chunks.clone();
        flipped[5] ^= 1;
        let err = read_error(with(&flipped)).await;
        assert_eq!(err.to_string(), "vmess: a chunk cannot be authenticated");
        // the connection ends inside a chunk
        let err = read_error(with(&chunks[..10])).await;
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        assert_eq!(
            err.to_string(),
            "vmess: the connection ended in the middle of a chunk"
        );
        // and inside the response head
        let err = read_error(head[..30].to_vec()).await;
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        // a length that cannot even hold the tag: the vector's first two
        // bytes are `mask ^ 21` (`world` plus its tag), so `^ 21 ^ 15` makes
        // the same mask announce fifteen bytes
        let announced = u16::from_be_bytes([chunks[0], chunks[1]]) ^ 21 ^ 15;
        let short = announced.to_be_bytes();
        let err = read_error(with(&short)).await;
        assert_eq!(err.to_string(), "vmess: a chunk shorter than its tag");
    }

    #[tokio::test]
    async fn a_close_between_chunks_ends_the_stream_cleanly() {
        let head = vectors::hex(vectors::RESPONSE_SEALED);
        let chunks = vectors::hex(vectors::RESPONSE_CHUNKS_AES);
        // `world` alone, without the courtesy end-of-stream chunk
        let first = &chunks[..2 + 5 + 16];
        let (near, mut far) = tokio::io::duplex(4096);
        let mut stream = VmessStream::new(Box::new(near), vectors::session(), Security::Aes128Gcm);
        far.write_all(&[&head[..], first].concat()).await.unwrap();
        drop(far);
        let mut got = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut got))
            .await
            .expect("bounded")
            .unwrap();
        assert_eq!(got, b"world");
    }
}
