//! `anytls` outbound (manual: Policies › AnyTLS; protocol: anytls-go
//! `docs/protocol.md`, version 2). TLS, then `SHA256(password)` with the
//! scheme's first padding, then a session layer of frames (`frame`) whose
//! first writes are cut and padded the way the padding scheme says
//! (`padding`). A session carries one stream at a time and is reused for the
//! next one (`session`, `pool`); `reuse=false` closes it with its stream.
//!
//! A wrong password cannot be told from a server that closes: it answers
//! like a web site, and the stream ends in the relay.

pub(crate) mod frame;
pub(crate) mod padding;
mod pool;
mod session;

use crate::addr::{AddrError, socks_addr};
use crate::build::{shadow_tls_client, tls_client};
use crate::task::AbortOnDrop;
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError};
use padding::Scheme;
use pool::Pool;
use rurge_config::KeystoreItem;
use rurge_config::spec::{AnyTlsSpec, ShadowTlsOpts};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use session::{SchemeCell, Session, pick};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::io::AsyncWriteExt;

/// No `Debug`: the hash is as good as the password.
pub struct AnyTlsOutbound {
    name: String,
    stack: Stack,
    /// `SHA256(password)`, what the wire carries; the password itself is not kept.
    hash: [u8; 32],
    /// Starts as the protocol's default; the server may push another.
    scheme: SchemeCell,
    /// `None` with `reuse=false`.
    pool: Option<Arc<Pool>>,
    /// Started by the first connection: building an outbound (a dry build
    /// included) leaves no task behind.
    reaper: OnceLock<AbortOnDrop>,
    next_seq: AtomicU64,
}

impl AnyTlsOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &AnyTlsSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<AnyTlsOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and the
        // dry build both prefix it
        if spec.password.expose().is_empty() {
            return Err(BuildError::new("`password` is empty"));
        }
        // no ALPN unless the policy asks for one (M2 design 4.5)
        let shadow_tls =
            shadow_tls_client(shadow_tls, Some(&spec.tls), &server.host, roots.clone())?;
        let tls = tls_client(Some(&spec.tls), &server.host, &[], keystore, roots)?;
        Ok(AnyTlsOutbound {
            name: name.to_string(),
            stack: Stack::new(connector, server, shadow_tls, tls, None),
            hash: Sha256::digest(spec.password.expose().as_bytes()).into(),
            scheme: Arc::new(Mutex::new(Arc::new(Scheme::default_scheme()))),
            pool: spec.reuse.then(Arc::<Pool>::default),
            reaper: OnceLock::new(),
            next_seq: AtomicU64::new(0),
        })
    }

    async fn new_session(&self, opts: &ConnectOpts) -> Result<Session, OutboundError> {
        let mut io = self.stack.open(opts).await?;
        let scheme = self.scheme.lock().expect("scheme").clone();
        let padding = scheme.auth_padding(&mut pick);
        // one write, one TLS record: the reference server takes the whole
        // authentication from a single read
        let mut auth = Vec::with_capacity(34 + padding);
        auth.extend_from_slice(&self.hash);
        auth.extend_from_slice(&(padding as u16).to_be_bytes());
        auth.resize(34 + padding, 0);
        io.write_all(&auth).await?;
        io.flush().await?;
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Session::start(seq, io, self.scheme.clone()))
    }

    async fn open(&self, address: &[u8], opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        let pool = match &self.pool {
            Some(pool) => {
                self.reaper.get_or_init(|| pool::spawn_reaper(pool));
                Arc::downgrade(pool)
            }
            None => Weak::new(),
        };
        // an idle session may have died since it was pooled: try the next one, then dial
        while let Some(session) = self.pool.as_ref().and_then(|p| p.take()) {
            if let Ok(stream) = session.open(address, pool.clone()).await {
                return Ok(Box::new(stream));
            }
        }
        let session = self.new_session(opts).await?;
        match session.open(address, pool).await {
            Ok(stream) => Ok(Box::new(stream)),
            Err(e) => Err(OutboundError::Proxy(e.to_string())),
        }
    }
}

impl Outbound for AnyTlsOutbound {
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
            let address = socks_addr(target).map_err(|e| {
                OutboundError::Proxy(
                    match e {
                        AddrError::Unsendable => {
                            "anytls: the host name cannot be sent to the server"
                        }
                        AddrError::TooLong => "anytls: the host name is longer than 255 bytes",
                    }
                    .to_string(),
                )
            })?;
            // one budget for the connection, TLS, the authentication and the
            // stream's first write; the server's SYNACK is not awaited
            match tokio::time::timeout(opts.timeout, self.open(&address, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{AnyTlsScript, FakeAnyTls, TlsFixture, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::anytls::read_anytls;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

    /// The outbound for `definition` (an `anytls, host, port, ...` line).
    fn outbound(definition: &str, fixture: &Arc<TlsFixture>) -> AnyTlsOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("A", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_anytls(&mut r, &[]);
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        AnyTlsOutbound::new(
            "A",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            shadow_tls.as_ref(),
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    fn script(password: &str) -> AnyTlsScript {
        AnyTlsScript {
            password: password.into(),
            ..AnyTlsScript::default()
        }
    }

    async fn server(script: AnyTlsScript) -> (Arc<TlsFixture>, FakeAnyTls) {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeAnyTls::spawn(script, fixture.clone()).await;
        (fixture, fake)
    }

    fn line(fake: &FakeAnyTls, rest: &str) -> String {
        format!(
            "anytls, 127.0.0.1, {}, password=pw{rest}",
            fake.addr().port()
        )
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn connect(out: &AnyTlsOutbound, to: SocketAddr) -> BoxedStream {
        out.connect_tcp(&target(to), &ConnectOpts::default())
            .await
            .unwrap()
    }

    /// Polls `what` until it holds; bounded, and no fixed sleep decides anything.
    async fn eventually(what: impl Fn() -> bool, why: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !what() {
            assert!(tokio::time::Instant::now() < deadline, "{why}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn ping(stream: &mut BoxedStream, text: &[u8]) {
        // no explicit `flush()`: `write_all` alone must deliver
        stream.write_all(text).await.unwrap();
        let mut buf = vec![0u8; text.len()];
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut buf))
            .await
            .expect("the echo arrives within the bound")
            .unwrap();
        assert_eq!(buf, text);
    }

    /// The engine's relay, reduced to what matters to a stream under test:
    /// `read` → `write_all` with no flush, `shutdown` at EOF, both directions
    /// polled from one task through `tokio::io::split`.
    async fn copy_half<R, W>(mut reader: R, mut writer: W) -> std::io::Result<u64>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
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
    async fn a_relay_moves_a_megabyte_each_way_and_the_session_comes_back() {
        tokio::time::timeout(Duration::from_secs(60), async {
            let echo = echo_server().await;
            let (fixture, fake) = server(script("pw")).await;
            let out = outbound(&line(&fake, ""), &fixture);
            let upstream = connect(&out, echo).await;
            let (mut app, near) = tokio::io::duplex(64 * 1024);
            let relay = tokio::spawn(async move {
                let (cr, cw) = tokio::io::split(near);
                let (ur, uw) = tokio::io::split(upstream);
                tokio::join!(copy_half(cr, uw), copy_half(ur, cw))
            });
            let payload: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
            let (mut app_r, mut app_w) = tokio::io::split(&mut app);
            let mut back = vec![0u8; payload.len()];
            tokio::join!(async { app_w.write_all(&payload).await.unwrap() }, async {
                app_r.read_exact(&mut back).await.unwrap()
            });
            assert!(back == payload, "the echo differs");
            drop((app_r, app_w));
            drop(app); // the client goes away: FIN, and the relay winds down by itself
            let (up, down) = relay.await.unwrap();
            assert_eq!((up.unwrap(), down.unwrap()), (1_000_000, 1_000_000));
            let pool = out.pool.as_ref().unwrap();
            eventually(|| pool.len() == 1, "the session never came back").await;
            eventually(|| fake.fins() == 1, "no FIN").await;
            // the authentication's 30 bytes at least, and whatever the first writes were padded with
            assert!(fake.waste() >= 30, "{}", fake.waste());
            let settings = fake.settings();
            assert_eq!(settings.len(), 1);
            assert_eq!(
                settings[0],
                format!(
                    "v=2\nclient=rurge/{}\npadding-md5=75cff2ad89aadf5e257059ee571ebe11",
                    env!("CARGO_PKG_VERSION")
                )
            );
            // TLS as every other outbound does it: no ALPN unless asked for
            assert_eq!(fixture.seen()[0].alpn, None);
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn the_newest_idle_session_is_reused_and_reuse_can_be_turned_off() {
        let echo = echo_server().await;
        let (fixture, fake) = server(script("pw")).await;
        let out = outbound(&line(&fake, ""), &fixture);
        for round in 0..3u8 {
            let mut s = connect(&out, echo).await;
            ping(&mut s, &[round; 8]).await;
            s.shutdown().await.unwrap();
            drop(s);
        }
        assert_eq!(fake.sessions(), 1);
        let streams = fake.streams();
        assert_eq!(
            streams
                .iter()
                .map(|s| (s.session, s.sid))
                .collect::<Vec<_>>(),
            [(0, 1), (0, 2), (0, 3)]
        );
        assert_eq!(fake.settings().len(), 1, "settings go out once per session");
        // two at once need two sessions; the newer one is the one reused
        let mut a = connect(&out, echo).await;
        let mut b = connect(&out, echo).await;
        ping(&mut a, b"a").await;
        ping(&mut b, b"b").await;
        assert_eq!(fake.sessions(), 2);
        drop(a);
        drop(b);
        let pool = out.pool.as_ref().unwrap();
        eventually(|| pool.len() == 2, "both come back").await;
        let mut c = connect(&out, echo).await;
        ping(&mut c, b"c").await;
        assert_eq!(
            fake.streams().last().unwrap().session,
            1,
            "the newest session first"
        );
        // reuse=false: every stream dials, and nothing is kept
        let (fixture, fake) = server(script("pw")).await;
        let once = outbound(&line(&fake, ", reuse=false"), &fixture);
        for _ in 0..2 {
            let mut s = connect(&once, echo).await;
            ping(&mut s, b"x").await;
        }
        assert_eq!(fake.sessions(), 2);
        assert!(once.pool.is_none() && once.reaper.get().is_none());
    }

    #[tokio::test]
    async fn a_refused_stream_fails_on_its_first_read_and_the_session_survives() {
        let (fixture, fake) = server(AnyTlsScript {
            refuse: Some("dial tcp: connection\r\nrefused\x1b[0m".into()),
            ..script("pw")
        })
        .await;
        let out = outbound(&line(&fake, ""), &fixture);
        let mut s = connect(&out, "127.0.0.1:9".parse().unwrap()).await;
        let mut buf = [0u8; 1];
        let err = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf))
            .await
            .expect("bounded")
            .unwrap_err();
        // the server's text, stripped of control characters
        assert_eq!(err.to_string(), "anytls: dial tcp: connectionrefused[0m");
        drop(s);
        let pool = out.pool.as_ref().unwrap();
        eventually(
            || pool.len() == 1,
            "a refused stream does not cost the session",
        )
        .await;
        eventually(
            || fake.fins() == 1,
            "the refused stream is still closed with a FIN",
        )
        .await;
    }

    #[tokio::test]
    async fn an_alert_closes_the_session_and_a_wrong_password_shows_in_the_relay() {
        let (fixture, fake) = server(AnyTlsScript {
            alert: Some("upgrade your\nclient".into()),
            ..script("pw")
        })
        .await;
        let out = outbound(&line(&fake, ""), &fixture);
        let mut s = connect(&out, "127.0.0.1:9".parse().unwrap()).await;
        let mut buf = [0u8; 1];
        let err = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf))
            .await
            .expect("bounded")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "anytls: the server sent an alert: upgrade yourclient"
        );
        drop(s);
        assert_eq!(
            out.pool.as_ref().unwrap().len(),
            0,
            "a dead session is not pooled"
        );
        // a wrong password: the server answers like a web site and closes
        let wrong = outbound(
            &format!("anytls, 127.0.0.1, {}, password=other", fake.addr().port()),
            &fixture,
        );
        let mut s = connect(&wrong, "127.0.0.1:9".parse().unwrap()).await;
        let err = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf))
            .await
            .expect("bounded")
            .unwrap_err();
        assert_eq!(err.to_string(), "anytls: the session is closed");
        assert_eq!(fake.rejected(), 1);
        assert!(!err.to_string().contains("other"));
    }

    #[tokio::test]
    async fn a_pushed_scheme_is_used_by_the_next_session_and_a_bad_one_is_ignored() {
        let echo = echo_server().await;
        let pushed = "stop=3\n0=11-11\n1=50-50\n2=60-60";
        let (fixture, fake) = server(AnyTlsScript {
            scheme: Some(pushed.into()),
            ..script("pw")
        })
        .await;
        let out = outbound(&line(&fake, ", reuse=false"), &fixture);
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"one").await;
        drop(s);
        let expected = Scheme::parse(pushed.as_bytes()).unwrap().md5().to_string();
        eventually(
            || out.scheme.lock().unwrap().md5() == expected,
            "the pushed scheme never took effect",
        )
        .await;
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"two").await;
        let settings = fake.settings();
        assert!(
            settings[1].ends_with(&format!("padding-md5={expected}")),
            "{}",
            settings[1]
        );
        // a scheme out of bounds is refused, and the current one stays
        let (fixture, bad) = server(AnyTlsScript {
            scheme: Some("stop=9999\n1=5-5".into()),
            ..script("pw")
        })
        .await;
        let out = outbound(&line(&bad, ", reuse=false"), &fixture);
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"three").await;
        assert_eq!(
            out.scheme.lock().unwrap().md5(),
            "75cff2ad89aadf5e257059ee571ebe11"
        );
    }

    #[tokio::test]
    async fn a_heartbeat_is_answered_and_a_version_one_server_works() {
        let echo = echo_server().await;
        let (fixture, fake) = server(AnyTlsScript {
            heartbeat: true,
            ..script("pw")
        })
        .await;
        let out = outbound(&line(&fake, ""), &fixture);
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"beat").await;
        eventually(
            || fake.heart_responses() == 1,
            "the heartbeat went unanswered",
        )
        .await;
        let (fixture, v1) = server(AnyTlsScript {
            v1: true,
            ..script("pw")
        })
        .await;
        let out = outbound(&line(&v1, ""), &fixture);
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"version one").await;
    }

    #[tokio::test]
    async fn a_session_the_server_closed_while_idle_is_not_reused() {
        let echo = echo_server().await;
        let (fixture, fake) = server(script("pw")).await;
        let out = outbound(&line(&fake, ""), &fixture);
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"first").await;
        drop(s);
        let pool = out.pool.clone().unwrap();
        eventually(|| pool.len() == 1, "pooled").await;
        fake.kick();
        // the session's task notices on its own, with nobody using the session
        eventually(
            || {
                pool.reap(tokio::time::Instant::now());
                pool.len() == 0
            },
            "the dead session stayed in the pool",
        )
        .await;
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"second").await;
        assert_eq!(fake.sessions(), 2);
    }

    #[tokio::test]
    async fn an_idle_session_is_closed_after_a_minute_and_the_pool_dies_with_the_outbound() {
        let echo = echo_server().await;
        let (fixture, fake) = server(script("pw")).await;
        let out = outbound(&line(&fake, ""), &fixture);
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"once").await;
        drop(s);
        let pool = out.pool.clone().unwrap();
        eventually(|| pool.len() == 1, "pooled").await;
        // real sockets are done: from here on the clock is ours. (A paused
        // clock from the start would auto-advance past every timeout while
        // the test waits for real I/O.)
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(45)).await;
        tokio::task::yield_now().await;
        assert_eq!(pool.len(), 1, "not yet");
        tokio::time::advance(Duration::from_secs(50)).await;
        eventually(|| pool.len() == 0, "never reaped").await;
        let weak = Arc::downgrade(&pool);
        drop(pool);
        drop(out);
        assert!(weak.upgrade().is_none(), "the outbound owned the pool");
    }

    #[tokio::test]
    async fn an_empty_password_is_a_build_error_and_a_bad_name_never_dials() {
        let (fixture, fake) = server(script("pw")).await;
        let spec = AnyTlsSpec {
            tls: rurge_config::spec::TlsOpts::default(),
            password: rurge_config::spec::Secret::new(String::new()),
            reuse: true,
        };
        let err = AnyTlsOutbound::new(
            "A",
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            None,
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "`password` is empty");
        let out = outbound(&line(&fake, ""), &fixture);
        let bad = Target::new(HostName::Domain("a@b.test".into()), 80);
        let err = out
            .connect_tcp(&bad, &ConnectOpts::default())
            .await
            .err()
            .expect("refused");
        assert_eq!(
            err.to_string(),
            "anytls: the host name cannot be sent to the server"
        );
        assert_eq!(fake.sessions(), 0);
    }
}
