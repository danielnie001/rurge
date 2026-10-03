//! `tuic-v5` outbound (manual: Policies › TUIC; phase 2 M7 design 6): the
//! policy's QUIC connection, which its requests share (`session`),
//! authenticated as it comes up (`auth`).
//!
//! - Every command is `u8 version ‖ u8 type ‖ …`, big endian (TUIC v5
//!   SPEC); the server answers none of them.
//! - A TCP connection is a bidirectional stream of its own, the tunnel as
//!   soon as the Connect naming its target (`addr`) is written (`tcp`).

mod addr;
mod auth;
mod tcp;

use crate::session::{Dialer, Session};
use auth::Auth;
use rurge_config::KeystoreItem;
use rurge_config::spec::{Tristate, TuicSpec, TuicVersion};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::{BuildError, Outbound, OutboundError};
use rustls::RootCertStore;
use std::sync::Arc;

/// TUIC v5's version, first in every command.
const VERSION: u8 = 0x05;

/// No `Debug`: it holds the credentials.
pub struct TuicOutbound {
    name: String,
    session: Session<Auth>,
}

impl TuicOutbound {
    /// The connection goes as `connector` dials (design 3.3), with the
    /// policy's `ecn`; `client-cert` comes from `keystore`.
    pub fn new(
        name: &str,
        server: Target,
        spec: &TuicSpec,
        ecn: Tristate,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<TuicOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it
        let TuicVersion::V5 { uuid, password } = &spec.version else {
            return Err(BuildError::new("TUIC v4 (`tuic`) is not supported yet"));
        };
        // the configuration checked it, but a spec made by hand may not be
        if password.expose().is_empty() {
            return Err(BuildError::new("`password` is empty"));
        }
        let dialer = Dialer::new(
            connector, server, &spec.tls, &spec.quic, ecn, keystore, roots,
        )?;
        let auth = Auth::new(*uuid.expose(), password.expose());
        Ok(TuicOutbound {
            name: name.to_string(),
            session: Session::new(name, dialer, auth),
        })
    }
}

impl Outbound for TuicOutbound {
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
            let request = tcp::request(target)?;
            // one budget for the connection (when one is dialed), QUIC's
            // handshake and the Connect, which nothing answers
            let tunnel = self
                .session
                .request(opts, |lease| Box::pin(tcp::open(lease, &request)));
            match tokio::time::timeout(opts.timeout, tunnel).await {
                Ok(tunnel) => Ok(Box::new(tunnel?) as BoxedStream),
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_log::Log;
    use crate::testing::{FakeTuic, RecordedConnect, TuicScript};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::tuic::read_tuic;
    use rurge_config::spec::{CommonOpts, ParamReader, PortHopping, Secret};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::quic::testing::MultiPort;
    use rurge_proto::testing::echo_server;
    use std::io;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::time::{Instant, timeout};

    const BOUND: Duration = Duration::from_secs(10);
    /// The manual's example user, and its 16 bytes.
    const ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
    const UUID: [u8; 16] = [
        0x02, 0x33, 0xd1, 0x1c, 0x15, 0xa4, 0x47, 0xd3, 0xad, 0xe3, 0x48, 0xff, 0xca, 0x0c, 0xe1,
        0x19,
    ];
    const PASSWORD: &str = "pw";

    /// A fake that lets `ID` in with `PASSWORD`.
    fn script() -> TuicScript {
        TuicScript {
            uuid: UUID,
            password: PASSWORD.to_string(),
            ..TuicScript::default()
        }
    }

    /// `uuid=<uuid>, password=<password>`.
    fn credentials(uuid: &str, password: &str) -> String {
        format!("uuid={uuid}, password={password}")
    }

    /// The spec of `tuic-v5, 127.0.0.1, <port>, <params>`, as the
    /// configuration reads it.
    fn spec(port: u16, params: &str) -> TuicSpec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let line = format!("tuic-v5, 127.0.0.1, {port}, {params}");
        let policy = parse_policy("T", &line, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_tuic(&mut r, &mut CommonOpts::default(), &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        spec
    }

    /// `spec`'s outbound to `127.0.0.1:<port>`, trusting `fake`.
    fn outbound_with(spec: &TuicSpec, port: u16, fake: &FakeTuic) -> TuicOutbound {
        TuicOutbound::new(
            "T",
            Target::new(HostName::parse("127.0.0.1"), port),
            spec,
            Tristate::Auto,
            &[],
            fake.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// `tuic-v5, 127.0.0.1, <the fake's port>, <params>`.
    fn outbound(fake: &FakeTuic, params: &str) -> TuicOutbound {
        let port = fake.addr().port();
        outbound_with(&spec(port, params), port, fake)
    }

    /// The outbound of the fake's user.
    fn user(fake: &FakeTuic) -> TuicOutbound {
        outbound(fake, &credentials(ID, PASSWORD))
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn tunnel(out: &TuicOutbound, to: &Target) -> BoxedStream {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel")
    }

    async fn failure(out: &TuicOutbound, to: &Target) -> OutboundError {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .err()
            .expect("no tunnel")
    }

    async fn round_trip(stream: &mut (impl AsyncRead + AsyncWrite + Unpin), payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        let mut back = vec![0u8; payload.len()];
        timeout(BOUND, stream.read_exact(&mut back))
            .await
            .expect("the echo arrives")
            .unwrap();
        assert_eq!(back, payload);
    }

    /// How a tunnel ends, read out: with an error.
    async fn end_of(stream: &mut BoxedStream) -> io::Error {
        let mut rest = Vec::new();
        timeout(BOUND, stream.read_to_end(&mut rest))
            .await
            .expect("the end of the tunnel")
            .expect_err("an error")
    }

    /// Design 6.2: once per connection, a unidirectional stream carries
    /// the user's UUID and the token its TLS session exports — the fake's
    /// own side of the session exports the same. The Connect goes along
    /// without waiting for a verdict, and is relayed once the server let
    /// the client in. The token is the session's: another connection's
    /// differs.
    #[tokio::test]
    async fn the_authentication_carries_the_uuid_and_a_token_of_the_tls_session() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        assert_eq!(out.name(), "T");
        assert!(out.http_forward().is_none());
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"through tuic").await;
        let auths = fake.auths();
        assert_eq!(auths.len(), 1);
        assert_eq!((auths[0].connection, auths[0].uuid), (0, UUID));
        assert_eq!(auths[0].token, auths[0].expected);
        assert!(auths[0].accepted);
        assert_eq!(
            fake.connects(),
            [RecordedConnect {
                connection: 0,
                address: echo.to_string(),
            }]
        );
        let other = user(&fake);
        let mut stream = tunnel(&other, &target(echo)).await;
        round_trip(&mut stream, b"on another connection").await;
        let auths = fake.auths();
        assert_eq!((auths.len(), auths[1].connection), (2, 1));
        assert!(auths[1].accepted);
        assert_ne!(auths[1].token, auths[0].token);
    }

    /// Design 6.2: the server answers no authentication. One it refuses —
    /// a wrong password, a user it does not know — it closes the
    /// connection with code 0 and no reason, as tuic-server and sing-box
    /// close it, before it relayed anything: a tunnel on it ends with
    /// `tuic: authentication failed`. The next tunnel dials and
    /// authenticates again. A close with another code is no such refusal.
    #[tokio::test]
    async fn a_refused_authentication_ends_the_tunnel_and_the_next_dials_again() {
        const STRANGER: &str = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let echo = echo_server().await;
        for (params, code, expected) in [
            (credentials(ID, "wrong"), 0, "tuic: authentication failed"),
            (
                credentials(STRANGER, PASSWORD),
                0,
                "tuic: authentication failed",
            ),
            (
                credentials(ID, "wrong"),
                0x2a,
                "tuic: the server closed the connection (code 0x2a)",
            ),
        ] {
            let fake = FakeTuic::spawn(TuicScript {
                auth_failure_code: code,
                ..script()
            });
            let out = outbound(&fake, &params);
            for _ in 0..2 {
                // nothing answers the Connect: the tunnel comes, and ends
                let mut stream = tunnel(&out, &target(echo)).await;
                assert_eq!(end_of(&mut stream).await.to_string(), expected, "{params}");
            }
            assert_eq!(fake.connections(), 2);
            let auths = fake.auths();
            assert_eq!(auths.len(), 2);
            assert!(auths.iter().all(|auth| !auth.accepted));
        }
    }

    /// Past both sides' stream windows (8 MiB ours, quinn's 1.25 MB the
    /// fake's); the shutdown reaches the target as a FIN, and the target's
    /// close comes back as the end of the stream.
    #[tokio::test]
    async fn a_tunnel_carries_a_large_payload_and_a_half_close() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let stream = tunnel(&out, &target(echo)).await;
        let data: Vec<u8> = (0..(9 << 20) + 777).map(|i| (i % 253) as u8).collect();
        let (mut r, mut w) = tokio::io::split(stream);
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            w.write_all(&sent).await.unwrap();
            w.shutdown().await.unwrap();
        });
        let mut back = Vec::new();
        timeout(Duration::from_secs(60), r.read_to_end(&mut back))
            .await
            .expect("stalled")
            .unwrap();
        writer.await.unwrap();
        assert!(back == data, "the echo differs");
    }

    /// Design 6.2: an address goes to the server as its bytes, a name as a
    /// name — an IDN as its A-labels — for the server to resolve. A target
    /// that cannot be named is refused before anything is dialed.
    #[tokio::test]
    async fn targets_go_out_as_the_server_reads_them_and_an_unsendable_name_never_dials() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            connect_to: Some(echo),
            ..script()
        });
        let out = user(&fake);
        for (host, address) in [
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:443"),
            (HostName::parse("192.0.2.1"), "192.0.2.1:443"),
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:443",
            ),
            (HostName::parse("remote.example"), "remote.example:443"),
        ] {
            let mut stream = tunnel(&out, &Target::new(host, 443)).await;
            round_trip(&mut stream, b"x").await;
            assert_eq!(fake.connects().last().unwrap().address, address);
        }
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let unsendable = "tuic: the host name cannot be sent to the server";
        for (name, text) in [
            ("x@blocked.test".to_string(), unsendable),
            ("a b.test".to_string(), unsendable),
            ("a.test\r\nx: 1".to_string(), unsendable),
            (String::new(), unsendable),
            (
                "a".repeat(256),
                "tuic: the host name is longer than 255 bytes",
            ),
        ] {
            let err = failure(&out, &Target::new(HostName::Domain(name), 80)).await;
            assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
            assert_eq!(err.to_string(), text);
        }
        assert_eq!(fake.connections(), 0);
    }

    /// One dial and one authentication for the tunnels that come together,
    /// and the connection carries those after them too.
    #[tokio::test]
    async fn tunnels_that_come_together_share_one_connection() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let to = target(echo);
        let (a, b, c) = tokio::join!(tunnel(&out, &to), tunnel(&out, &to), tunnel(&out, &to));
        for (mut stream, payload) in [(a, b"one"), (b, b"two"), (c, b"six")] {
            round_trip(&mut stream, payload).await;
        }
        let mut later = tunnel(&out, &to).await;
        round_trip(&mut later, b"later").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.auths().len(), 1);
        assert_eq!(fake.connects().len(), 4);
    }

    /// A tunnel under way ends with its connection — closed by a server
    /// that relayed on it, which is no refused authentication —; the next
    /// tunnel dials and authenticates again.
    #[tokio::test]
    async fn after_the_server_closes_the_connection_the_next_tunnel_dials_again() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let mut first = tunnel(&out, &target(echo)).await;
        round_trip(&mut first, b"first").await;
        fake.close_connections();
        assert_eq!(
            end_of(&mut first).await.to_string(),
            "tuic: the server closed the connection (code 0x0)"
        );
        let mut second = tunnel(&out, &target(echo)).await;
        round_trip(&mut second, b"second").await;
        assert_eq!(fake.connections(), 2);
        assert_eq!(fake.auths().len(), 2);
    }

    /// Design 3.5 and 6.2: the server restarts, losing its connections.
    /// Nothing answers a Connect, so nothing tells the client before the
    /// tunnel's own packets go out: a tunnel opened on the lost connection
    /// after the restart, and the one under way, end with the stateless
    /// reset that answers the first of them. The next tunnel dials anew.
    #[tokio::test]
    async fn after_a_restart_the_tunnels_of_the_lost_connection_end_and_the_next_dials_again() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let mut before = tunnel(&out, &target(echo)).await;
        round_trip(&mut before, b"before").await;
        fake.restart();
        let mut stale = tunnel(&out, &target(echo)).await;
        let reset = "quic: the server no longer knows the connection (stateless reset)";
        assert_eq!(end_of(&mut stale).await.to_string(), reset);
        assert_eq!(end_of(&mut before).await.to_string(), reset);
        let mut after = tunnel(&out, &target(echo)).await;
        round_trip(&mut after, b"after").await;
        let carried: Vec<usize> = fake.connects().iter().map(|c| c.connection).collect();
        assert_eq!(carried, [0, 1]);
        assert_eq!(fake.auths().len(), 2);
    }

    /// Design 3.5: a request that finds its connection gone before its
    /// Connect went out — the server closed it as the request took it —
    /// goes again, once, on a new connection.
    #[tokio::test]
    async fn a_connection_found_gone_before_the_connect_went_out_is_dialed_again_once() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let mut first = tunnel(&out, &target(echo)).await;
        round_trip(&mut first, b"first").await;
        let request = tcp::request(&target(echo)).unwrap();
        let attempts = AtomicUsize::new(0);
        let opts = ConnectOpts::default();
        let reopened = out.session.request(&opts, |lease| {
            let gone = attempts.fetch_add(1, Ordering::SeqCst) == 0;
            let (fake, request) = (&fake, &request);
            Box::pin(async move {
                if gone {
                    fake.close_connections();
                    timeout(BOUND, lease.connection().closed())
                        .await
                        .expect("closed");
                }
                tcp::open(lease, request).await
            })
        });
        let mut second = timeout(BOUND, reopened)
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel");
        round_trip(&mut second, b"second").await;
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(fake.connections(), 2);
        assert_eq!(fake.auths().len(), 2);
    }

    /// Design 3.3: the port changes under the connection — a new local
    /// socket each time — and its tunnels, old and new, go on. The interval
    /// is set past the configuration's minimum of 5 seconds.
    #[tokio::test]
    async fn tunnels_keep_working_while_the_port_hops() {
        let echo = echo_server().await;
        let ports = MultiPort::bind(3);
        let fake = FakeTuic::spawn_on(script(), ports.clone());
        let mut spec = spec(9, &credentials(ID, PASSWORD));
        spec.quic.port_hopping = Some(PortHopping {
            ports: ports.ports().iter().map(|&p| p..=p).collect(),
            interval: Duration::from_millis(100),
        });
        // the line's own port is not used
        let out = outbound_with(&spec, 9, &fake);
        let mut early = tunnel(&out, &target(echo)).await;
        let deadline = Instant::now() + BOUND;
        while ports.heard().iter().filter(|&&n| n > 0).count() < 2 {
            assert!(
                Instant::now() < deadline,
                "heard only on {:?}",
                ports.heard()
            );
            round_trip(&mut early, b"hop").await;
        }
        let mut late = tunnel(&out, &target(echo)).await;
        round_trip(&mut late, b"after a hop").await;
        round_trip(&mut early, b"still there").await;
        assert!(ports.clients() >= 2, "a new local socket per hop");
        assert_eq!(fake.connections(), 1);
    }

    /// `opts.timeout` bounds the request: the dial and QUIC's handshake
    /// with a server that never answers it.
    #[tokio::test]
    async fn a_server_that_never_answers_the_handshake_times_out() {
        let echo = echo_server().await;
        // nothing speaks QUIC on this port
        let quiet = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = quiet.local_addr().unwrap().port();
        let fake = FakeTuic::spawn(script());
        let out = outbound_with(&spec(port, &credentials(ID, PASSWORD)), port, &fake);
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        let started = Instant::now();
        let err = timeout(BOUND, out.connect_tcp(&target(echo), &opts))
            .await
            .expect("an outcome within the bound")
            .err()
            .unwrap();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        // well before QUIC's own handshake timeout, about 3 seconds
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    /// Design 6.2: nothing answers a Connect either. A target the server
    /// cannot reach shows as the stream ending, as sing-box ends it:
    /// nothing to read, and what the caller writes is stopped. The
    /// connection goes on.
    #[tokio::test]
    async fn a_target_the_server_cannot_reach_ends_the_tunnel_and_the_connection_goes_on() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        // the fake resolves no name
        let unknown = Target::new(HostName::parse("unknown.test"), 80);
        let mut stream = tunnel(&out, &unknown).await;
        let mut rest = Vec::new();
        let read = timeout(BOUND, stream.read_to_end(&mut rest))
            .await
            .expect("the end of the stream");
        assert_eq!(read.unwrap(), 0);
        let deadline = Instant::now() + BOUND;
        let stopped = loop {
            assert!(Instant::now() < deadline, "writes still taken");
            if let Err(e) = stream.write_all(b"x").await {
                break e;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(
            stopped.to_string(),
            "tuic: the server stopped the stream (code 0x0)"
        );
        let mut next = tunnel(&out, &target(echo)).await;
        round_trip(&mut next, b"still up").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.connects().len(), 2);
    }

    /// The credentials go to the server and nowhere else: not into the
    /// error of a refused authentication, nor into a build error, nor into
    /// a log line at any level.
    #[tokio::test]
    async fn the_credentials_never_show() {
        const SECRET: &str = "s3cret-tuic";
        let (log, _logging, _second) = Log::capture();
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = outbound(&fake, &credentials(ID, SECRET));
        let mut stream = tunnel(&out, &target(echo)).await;
        let err = end_of(&mut stream).await;
        assert_eq!(err.to_string(), "tuic: authentication failed");
        let hex: String = UUID.iter().map(|b| format!("{b:02x}")).collect();
        let shown = format!("{err} {err:?}");
        for secret in [SECRET, ID, hex.as_str()] {
            assert!(!shown.contains(secret), "{shown}");
        }
        let mut spec = spec(443, &credentials(ID, SECRET));
        spec.version = TuicVersion::V5 {
            uuid: Secret::new(UUID),
            password: "".into(),
        };
        let err = TuicOutbound::new(
            "T",
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            Tristate::Auto,
            &[],
            fake.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "`password` is empty");
        let log = log.text();
        assert!(!log.is_empty());
        for secret in [SECRET, ID, hex.as_str()] {
            assert!(!log.contains(secret));
        }
    }
}
