//! `tuic` and `tuic-v5` outbounds (manual: Policies › TUIC; phase 2 M7
//! design 6): the policy's QUIC connection, which its requests share
//! (`session`), authenticated as it comes up (`auth`) — TUIC v5 for
//! `tuic-v5`, TUIC v4 as tuic 0.8.5 speaks it for `tuic`.
//!
//! - Every command is `u8 version ‖ u8 type ‖ …`, big endian (TUIC v5
//!   SPEC; v4's protocol README). The server answers none, but for v4's
//!   Connect.
//! - A TCP connection is a bidirectional stream of its own, beginning with
//!   the Connect that names its target (`addr`): the tunnel as soon as that
//!   is written in v5, once the server answered that it reached the target
//!   in v4 (`tcp`).
//! - UDP goes in QUIC datagrams of the same connection, or in streams of
//!   its own where the server takes no datagrams, each client association
//!   an association of its own there (`udp`): v5 splits a packet that does
//!   not fit a datagram, v4 cannot.
//! - While anything is relayed, a heartbeat keeps the connection up
//!   (`heartbeat`): a datagram in v5, a stream in v4.

mod addr;
mod auth;
mod heartbeat;
mod tcp;
mod udp;

use crate::session::{Dialer, Session};
use auth::Auth;
use rurge_config::KeystoreItem;
use rurge_config::spec::{Tristate, TuicSpec, TuicVersion};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::{BuildError, Outbound, OutboundError, UdpSupport};
use rustls::RootCertStore;
use std::sync::Arc;
use udp::TuicUdp;

/// The TUIC a policy speaks (design 6.2, 6.3), which every command names
/// first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Version {
    /// `tuic`: tuic 0.8.5's protocol.
    V4,
    /// `tuic-v5`: the SPEC's.
    V5,
}

impl Version {
    /// The version `credentials` are for.
    fn of(credentials: &TuicVersion) -> Version {
        match credentials {
            TuicVersion::V4 { .. } => Version::V4,
            TuicVersion::V5 { .. } => Version::V5,
        }
    }

    /// The first byte of every command.
    const fn byte(self) -> u8 {
        match self {
            Version::V4 => 0x04,
            Version::V5 => 0x05,
        }
    }
}

/// No `Debug`: it holds the credentials.
pub struct TuicOutbound {
    name: String,
    version: Version,
    /// Shared with its UDP carriers, which may outlive it.
    session: Arc<Session<Auth>>,
}

impl TuicOutbound {
    /// The connection goes as `connector` dials (design 3.3), with the
    /// policy's `ecn`; `client-cert` comes from `keystore`. The line's
    /// keyword is the version (`spec.version`).
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
        // the dry build both prefix it; the configuration checked the
        // credentials, but a spec made by hand may not be
        let (key, credential) = match &spec.version {
            TuicVersion::V4 { token } => ("token", token.expose()),
            TuicVersion::V5 { password, .. } => ("password", password.expose()),
        };
        if credential.is_empty() {
            return Err(BuildError::new(format!("`{key}` is empty")));
        }
        let dialer = Dialer::new(
            connector, server, &spec.tls, &spec.quic, ecn, keystore, roots,
        )?;
        let version = Version::of(&spec.version);
        let auth = Auth::new(&spec.version, heartbeat::every(version));
        Ok(TuicOutbound {
            name: name.to_string(),
            version,
            session: Arc::new(Session::new(name, dialer, auth)),
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
            let request = tcp::request(self.version, target)?;
            // one budget for the connection (when one is dialed), QUIC's
            // handshake and the Connect — and v4's response to it
            let tunnel = self
                .session
                .request(opts, |lease| Box::pin(tcp::open(lease, &request)));
            match tokio::time::timeout(opts.timeout, tunnel).await {
                Ok(tunnel) => Ok(Box::new(tunnel?) as BoxedStream),
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }

    /// Whatever the policy says: TUIC always relays UDP (manual).
    fn udp(&self) -> UdpSupport {
        UdpSupport::Native
    }

    /// An association of its own on the policy's connection, dialed when
    /// there is none. Nothing answers the authentication: the carrier is
    /// there as soon as the connection is up.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            // one budget for the connection (when one is dialed) and QUIC's
            // handshake
            let open = TuicUdp::open(self.session.clone(), opts);
            match tokio::time::timeout(opts.timeout, open).await {
                Ok(udp) => Ok(Box::new(udp?) as BoxedPacketSocket),
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Lease;
    use crate::test_log::Log;
    use crate::testing::{FakeTuic, RecordedConnect, RecordedDissociate, TuicPacket, TuicScript};
    use auth::Authenticated;
    use quinn::{ConnectionError, VarInt};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::tuic::read_tuic;
    use rurge_config::spec::{CommonOpts, ParamReader, PortHopping, Secret};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{
        BoxedDatagram, DirectConnector, PacketSocket, Route, SystemResolve,
    };
    use rurge_net::quic::testing::MultiPort;
    use rurge_proto::testing::{echo_server, udp_echo_server};
    use std::collections::{BTreeSet, HashMap};
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::io;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::sync::Barrier;
    use tokio::task::JoinSet;
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

    /// The spec of `<keyword>, 127.0.0.1, <port>, <params>`, as the
    /// configuration reads it.
    fn spec_of(keyword: &str, port: u16, params: &str) -> TuicSpec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let line = format!("{keyword}, 127.0.0.1, {port}, {params}");
        let policy = parse_policy("T", &line, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_tuic(&mut r, &mut CommonOpts::default(), &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        spec
    }

    /// A `tuic-v5` line's spec.
    fn spec(port: u16, params: &str) -> TuicSpec {
        spec_of("tuic-v5", port, params)
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
        assert_eq!((auths[0].connection, auths[0].uuid), (0, Some(UUID)));
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
        let request = tcp::request(Version::V5, &target(echo)).unwrap();
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
        // well before QUIC's own time for a handshake, its idle timeout:
        // 30 seconds
        assert!(
            started.elapsed() < Duration::from_secs(5),
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
        let hex = |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() };
        // the token the client sent, which the fake recorded
        let token = fake.auths()[0].token;
        // each as text, in hex, and as `Debug` prints its bytes
        let forms = [
            SECRET.to_string(),
            ID.to_string(),
            hex(&UUID),
            format!("{UUID:?}"),
            hex(&token),
            format!("{token:?}"),
        ];
        let shown = format!("{err} {err:?}");
        for secret in &forms {
            assert!(!shown.contains(secret.as_str()), "{shown}");
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
        for secret in &forms {
            assert!(!log.contains(secret.as_str()));
        }
    }

    fn direct() -> Arc<dyn Connector> {
        Arc::new(DirectConnector::new(Arc::new(SystemResolve)))
    }

    /// The outbound of the fake's v5 user, its connections dialled through
    /// `connector` and its heartbeat every `every`.
    fn built(fake: &FakeTuic, connector: Arc<dyn Connector>, every: Duration) -> TuicOutbound {
        let spec = spec(fake.addr().port(), &credentials(ID, PASSWORD));
        built_from(&spec, fake, connector, every)
    }

    /// `spec`'s outbound to the fake, its connections dialled through
    /// `connector` and its heartbeat every `every`.
    fn built_from(
        spec: &TuicSpec,
        fake: &FakeTuic,
        connector: Arc<dyn Connector>,
        every: Duration,
    ) -> TuicOutbound {
        let dialer = Dialer::new(
            connector,
            Target::new(HostName::parse("127.0.0.1"), fake.addr().port()),
            &spec.tls,
            &spec.quic,
            Tristate::Auto,
            &[],
            fake.roots(),
        )
        .unwrap();
        TuicOutbound {
            name: "T".to_string(),
            version: Version::of(&spec.version),
            session: Arc::new(Session::new("T", dialer, Auth::new(&spec.version, every))),
        }
    }

    /// A lease on the outbound's connection, dialed when there is none.
    async fn held(out: &TuicOutbound) -> Lease<Authenticated> {
        timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a connection")
    }

    async fn udp_carrier(out: &TuicOutbound) -> BoxedPacketSocket {
        timeout(BOUND, out.open_udp(&ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a carrier")
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = timeout(BOUND, carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_round_trip(carrier: &dyn PacketSocket, to: SocketAddr, payload: &[u8]) {
        timeout(BOUND, carrier.send_to(payload, &target(to)))
            .await
            .expect("a send within the bound")
            .unwrap();
        assert_eq!(udp_answer(carrier).await, (payload.to_vec(), target(to)));
    }

    /// Nothing comes for a while: a fixed wait, only to see nothing happen.
    async fn no_udp_answer(carrier: &dyn PacketSocket) {
        let mut buf = vec![0u8; 65536];
        let quiet = timeout(Duration::from_millis(300), carrier.recv_from(&mut buf)).await;
        assert!(quiet.is_err(), "{quiet:?}");
    }

    /// Sends `payload` to the echo at `to` until it comes back: what goes
    /// out on a connection as it ends is lost.
    async fn until_answered(carrier: &dyn PacketSocket, to: SocketAddr, payload: &[u8]) {
        let deadline = Instant::now() + BOUND;
        let mut buf = vec![0u8; 65536];
        loop {
            assert!(Instant::now() < deadline, "no answer within the bound");
            timeout(BOUND, carrier.send_to(payload, &target(to)))
                .await
                .expect("a send within the bound")
                .unwrap();
            let wait = Instant::now() + Duration::from_millis(500);
            while let Ok(got) = tokio::time::timeout_at(wait, carrier.recv_from(&mut buf)).await {
                let (n, _) = got.unwrap();
                if buf[..n] == *payload {
                    return;
                }
            }
        }
    }

    /// Waits for `what` to hold, polling within the bound.
    async fn eventually(what: &str, mut holds: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !holds() {
            assert!(Instant::now() < deadline, "never: {what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// `(connection, association)` of every packet the fake read.
    fn carried(fake: &FakeTuic) -> Vec<(usize, u32)> {
        fake.udp_packets()
            .iter()
            .map(|p| (p.connection, p.association))
            .collect()
    }

    /// Design 6.2: where the server takes QUIC datagrams, a packet goes
    /// whole in one — association 0, the first of the connection; packets
    /// counted from 0; fragment 0 of 1 —, its target named in it. The
    /// carrier reaches any target through its association's one socket on
    /// the server, and hears anyone who sends to that socket (full cone);
    /// TCP goes on beside it.
    #[tokio::test]
    async fn udp_goes_in_datagrams_of_the_connection_to_any_target() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let tcp_echo = echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), one, b"to one").await;
        udp_round_trip(carrier.as_ref(), two, b"to two").await;
        let packets = fake.udp_packets();
        assert_eq!(packets.len(), 2);
        for (packet, (id, to)) in packets.iter().zip([(0, one), (1, two)]) {
            assert_eq!(
                (packet.connection, packet.association, packet.packet),
                (0, 0, id)
            );
            assert_eq!((packet.fragments, packet.fragment), (1, 0));
            assert_eq!(packet.address, Some(to.to_string()));
            assert_eq!(packet.len, 6);
            assert!(!packet.on_stream);
        }
        let outside = fake.udp_outside();
        assert_eq!(outside.len(), 1, "one socket for the association");
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger.send_to(b"unasked", outside[0]).await.unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), target(stranger.local_addr().unwrap()))
        );
        let mut stream = tunnel(&out, &target(tcp_echo)).await;
        round_trip(&mut stream, b"beside").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.auths().len(), 1);
    }

    /// Design 6.2: where the server takes no QUIC datagrams, each packet
    /// goes whole on a unidirectional stream of its own — the long ones too
    /// —, and the server answers on streams, full cone as well. No
    /// heartbeat goes: the SPEC sends it in a datagram only.
    #[tokio::test]
    async fn without_datagrams_each_packet_goes_on_a_stream_of_its_own() {
        let echo = udp_echo_server().await;
        let tcp_echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            ..script()
        });
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"on a stream").await;
        let long: Vec<u8> = (0..9000).map(|i| (i % 251) as u8).collect();
        udp_round_trip(carrier.as_ref(), echo, &long).await;
        let packets = fake.udp_packets();
        assert_eq!(packets.len(), 2);
        for (packet, len) in packets.iter().zip([11, 9000]) {
            assert!(packet.on_stream);
            assert_eq!((packet.fragments, packet.fragment), (1, 0));
            assert_eq!(
                (packet.address.clone(), packet.len),
                (Some(echo.to_string()), len)
            );
        }
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger
            .send_to(b"unasked", fake.udp_outside()[0])
            .await
            .unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), target(stranger.local_addr().unwrap()))
        );
        let mut stream = tunnel(&out, &target(tcp_echo)).await;
        round_trip(&mut stream, b"beside").await;
        let lease = held(&out).await;
        assert!(lease.connection().max_datagram_size().is_none());
        assert!(!lease.state().beats());
        assert_eq!(fake.connections(), 1);
    }

    /// A name goes to the server as a name — an IDN as its A-labels —, an
    /// address as its bytes; an answer names its source as the server
    /// heard it, a name too (as sing-box names one). A target that cannot
    /// be named is refused, and nothing goes out.
    #[tokio::test]
    async fn udp_targets_go_out_as_the_server_reads_them() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            connect_to: Some(echo),
            ..script()
        });
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        for (host, address) in [
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:53",
            ),
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:53"),
        ] {
            timeout(BOUND, carrier.send_to(b"q", &Target::new(host, 53)))
                .await
                .expect("a send within the bound")
                .unwrap();
            assert_eq!(
                udp_answer(carrier.as_ref()).await,
                (b"q".to_vec(), target(echo))
            );
            let last = fake.udp_packets().pop().unwrap();
            assert_eq!(last.address.as_deref(), Some(address));
        }
        let unsendable = "tuic: the host name cannot be sent to the server";
        for (name, text) in [
            ("x@blocked.test".to_string(), unsendable),
            ("a b.test".to_string(), unsendable),
            (String::new(), unsendable),
            (
                "a".repeat(256),
                "tuic: the host name is longer than 255 bytes",
            ),
        ] {
            let err = timeout(
                BOUND,
                carrier.send_to(b"q", &Target::new(HostName::Domain(name), 53)),
            )
            .await
            .expect("an outcome within the bound")
            .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(err.to_string(), text);
        }
        assert_eq!(fake.udp_packets().len(), 2);
        fake.send_udp(0, &TuicPacket::whole(0, 7, "dns.example:53", b"by name"));
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (
                b"by name".to_vec(),
                Target::new(HostName::parse("dns.example"), 53)
            )
        );
    }

    /// Design 6.2: a packet too long for one QUIC datagram goes in
    /// fragments under its packet id, only the first naming the target;
    /// the echo, as long, comes back in fragments of the server's that the
    /// carrier puts together. (At most 9216 bytes: macOS sends no longer
    /// UDP datagram by default.)
    #[tokio::test]
    async fn long_packets_go_in_fragments_both_ways() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        for (id, len) in [(0, 3000), (1, 9000)] {
            let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            udp_round_trip(carrier.as_ref(), echo, &payload).await;
            let packets = fake.udp_packets();
            let count = packets.last().unwrap().fragments;
            assert!(count > 1, "{count}");
            let fragments = &packets[packets.len() - usize::from(count)..];
            for (i, fragment) in fragments.iter().enumerate() {
                assert_eq!(
                    (fragment.association, fragment.packet, fragment.fragment),
                    (0, id, i as u8)
                );
                let address = (i == 0).then(|| echo.to_string());
                assert_eq!(fragment.address, address, "fragment {i}");
            }
            assert_eq!(fragments.iter().map(|f| f.len).sum::<usize>(), len);
        }
    }

    /// Design 6.2: a fragment lost loses its packet and nothing more.
    #[tokio::test]
    async fn a_lost_fragment_loses_its_packet_alone() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            drop_fragment: Some(1),
            ..script()
        });
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        let (lost, kept) = (vec![1u8; 3000], vec![2u8; 3000]);
        for payload in [&lost[..], &kept, b"short"] {
            timeout(BOUND, carrier.send_to(payload, &target(echo)))
                .await
                .expect("a send within the bound")
                .unwrap();
        }
        // in the order they came: the first never came whole
        assert_eq!(udp_answer(carrier.as_ref()).await.0, kept);
        assert_eq!(udp_answer(carrier.as_ref()).await.0, b"short");
        no_udp_answer(carrier.as_ref()).await;
    }

    /// Design 6.2: an association puts at most 8 packets together at a
    /// time; a ninth drops the one whose first fragment came first, which
    /// then never comes whole.
    #[tokio::test]
    async fn an_association_puts_at_most_eight_packets_together_at_a_time() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"hello").await;
        let from = "192.0.2.7:7";
        let fragment = |packet: u16, fragment: u8, payload: &str| TuicPacket {
            association: 0,
            packet,
            fragments: 2,
            fragment,
            address: (fragment == 0).then(|| from.to_string()),
            payload: payload.into(),
        };
        for packet in 1..=9 {
            fake.send_udp(0, &fragment(packet, 0, &format!("{packet}-")));
        }
        for packet in (2..=9).chain([1]) {
            fake.send_udp(0, &fragment(packet, 1, "x"));
        }
        let source = Target::new(HostName::parse("192.0.2.7"), 7);
        for packet in 2..=9 {
            assert_eq!(
                udp_answer(carrier.as_ref()).await,
                (format!("{packet}-x").into_bytes(), source.clone())
            );
        }
        no_udp_answer(carrier.as_ref()).await;
    }

    /// What the digest target answers for `payload`.
    fn digest(payload: &[u8]) -> Vec<u8> {
        let mut hasher = DefaultHasher::new();
        payload.hash(&mut hasher);
        hasher.finish().to_be_bytes().to_vec()
    }

    /// A loopback UDP target that answers each packet with a digest of it:
    /// what comes back is small and whole, so it is lost to nothing but
    /// what went out broken.
    async fn digest_target() -> SocketAddr {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                let (n, from) = match socket.recv_from(&mut buf).await {
                    Ok(got) => got,
                    // an ICMP "unreachable" for an earlier packet (Windows)
                    Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                    Err(_) => return,
                };
                let _ = socket.send_to(&digest(&buf[..n]), from).await;
            }
        });
        addr
    }

    /// Tasks that send through one carrier at once, on threads of their
    /// own, keep each packet's fragments together on the wire — as a server
    /// that puts one packet of an association together at a time needs
    /// (Clash.Meta's first TUIC v5 server did) — and every packet reaches
    /// the target whole.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_sends_keep_each_packets_fragments_together() {
        const TASKS: u8 = 8;
        const ROUNDS: u8 = 5;
        let to = target(digest_target().await);
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let carrier: Arc<dyn PacketSocket> = Arc::from(udp_carrier(&out).await);
        let mut buf = vec![0u8; 65536];
        for round in 0..ROUNDS {
            // 3000 bytes, three fragments or more, unlike any other's
            let payloads: Vec<Vec<u8>> = (0..TASKS)
                .map(|task| {
                    (0..3000u32)
                        .map(|i| (i as u8) ^ (round << 4 | task))
                        .collect()
                })
                .collect();
            // all of them at once
            let start = Arc::new(Barrier::new(payloads.len()));
            let mut sending = JoinSet::new();
            for payload in payloads.clone() {
                let (carrier, start, to) = (carrier.clone(), start.clone(), to.clone());
                sending.spawn(async move {
                    start.wait().await;
                    carrier.send_to(&payload, &to).await.unwrap();
                });
            }
            timeout(BOUND, async {
                while let Some(sent) = sending.join_next().await {
                    sent.unwrap();
                }
            })
            .await
            .expect("every send within the bound");
            let mut expected: Vec<Vec<u8>> = payloads.iter().map(|p| digest(p)).collect();
            let mut got = Vec::new();
            let deadline = Instant::now() + BOUND;
            while got.len() < expected.len() {
                let Ok(answer) =
                    tokio::time::timeout_at(deadline, carrier.recv_from(&mut buf)).await
                else {
                    break;
                };
                let (n, _) = answer.unwrap();
                got.push(buf[..n].to_vec());
            }
            expected.sort();
            got.sort();
            assert_eq!(got, expected, "round {round}");
        }
        // in the order the server read them, each packet's fragments in a
        // row
        let mut positions: HashMap<u16, Vec<usize>> = HashMap::new();
        for (at, packet) in fake.udp_packets().iter().enumerate() {
            positions.entry(packet.packet).or_default().push(at);
        }
        assert_eq!(positions.len(), usize::from(TASKS * ROUNDS));
        for (packet, at) in positions {
            assert!(at.len() > 1, "packet {packet} went whole");
            assert_eq!(
                at[at.len() - 1] - at[0] + 1,
                at.len(),
                "packet {packet}: {at:?}"
            );
        }
    }

    /// Design 6.2: a carrier gone dissociates its association on the
    /// connection it was on, which forgets it: what the server sends for it
    /// afterwards is dropped. The next carrier is an association of its
    /// own, with a socket of its own.
    #[tokio::test]
    async fn a_carrier_gone_dissociates_its_association() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let lease = held(&out).await;
        let udp = &lease.state().udp;
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"here").await;
        assert_eq!(udp.associations(), [0]);
        assert_eq!(lease.state().relays(), 1);
        drop(carrier);
        assert!(udp.associations().is_empty());
        assert_eq!(lease.state().relays(), 0);
        eventually("the Dissociate", || {
            fake.dissociates()
                == [RecordedDissociate {
                    connection: 0,
                    association: 0,
                }]
        })
        .await;
        fake.send_udp(0, &TuicPacket::whole(0, 9, "192.0.2.1:53", b"too late"));
        let next = udp_carrier(&out).await;
        udp_round_trip(next.as_ref(), echo, b"next").await;
        assert_eq!(udp.associations(), [1]);
        assert_eq!(fake.udp_outside().len(), 2);
        assert_eq!(fake.connections(), 1);
    }

    /// No heartbeat comes for a while, past one that may be on its way: a
    /// fixed wait, only to see nothing happen.
    async fn no_heartbeat(fake: &FakeTuic, every: Duration) {
        tokio::time::sleep(every * 3).await;
        let heard = fake.heartbeats();
        tokio::time::sleep(every * 5).await;
        assert_eq!(fake.heartbeats(), heard, "a heartbeat came");
    }

    /// Design 6.2: the heartbeat goes while a relay is under way on the
    /// connection — a TCP tunnel, a UDP carrier —, and not while none is.
    /// Its interval is cut short here (3 seconds otherwise).
    #[tokio::test]
    async fn heartbeats_go_while_a_relay_is_under_way_and_stop_when_none_is() {
        const EVERY: Duration = Duration::from_millis(100);
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        let fake = FakeTuic::spawn(script());
        let out = built(&fake, direct(), EVERY);
        // up, and nothing relayed
        let lease = held(&out).await;
        assert!(lease.state().beats());
        no_heartbeat(&fake, EVERY).await;
        assert_eq!(fake.heartbeats(), 0);
        let mut tcp = tunnel(&out, &target(tcp_echo)).await;
        round_trip(&mut tcp, b"tcp").await;
        eventually("heartbeats with a tunnel", || fake.heartbeats() >= 2).await;
        drop(tcp);
        no_heartbeat(&fake, EVERY).await;
        let before = fake.heartbeats();
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), udp_echo, b"udp").await;
        eventually("heartbeats with a carrier", || {
            fake.heartbeats() >= before + 2
        })
        .await;
        drop(carrier);
        no_heartbeat(&fake, EVERY).await;
        assert_eq!(lease.state().relays(), 0);
        assert_eq!(fake.connections(), 1);
    }

    /// The connection's tasks — the one that reads the server's packets,
    /// the writer, the heartbeat — end with it.
    #[tokio::test]
    async fn the_tasks_of_a_connection_end_with_it() {
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let lease = held(&out).await;
        let state = lease.state();
        assert!(state.udp.is_reading() && state.udp.is_writing() && state.beats());
        // nothing answers the authentication: the server's side may come
        // after the client's
        eventually("the server's side", || fake.connections() == 1).await;
        fake.close_connections();
        eventually("the tasks' end", || {
            !state.udp.is_reading() && !state.udp.is_writing() && !state.beats()
        })
        .await;
    }

    /// Design 6.2: a carrier whose connection ended goes on, on the
    /// connection that replaces it — whether the server closed the
    /// connection, or lost it in a restart (a stateless reset ends it at
    /// the next packet) —, under the id that connection gives it: each
    /// counts its own from 0. A carrier opened later gets an id of its own.
    #[tokio::test]
    async fn a_carrier_moves_to_a_new_connection_after_a_close_or_a_restart() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"first").await;
        fake.close_connections();
        until_answered(carrier.as_ref(), echo, b"after a close").await;
        fake.restart();
        until_answered(carrier.as_ref(), echo, b"after a restart").await;
        let carried = carried(&fake);
        assert!(carried.iter().all(|&(_, association)| association == 0));
        let connections: BTreeSet<usize> =
            carried.iter().map(|&(connection, _)| connection).collect();
        assert_eq!(connections, BTreeSet::from([0, 1, 2]));
        let later = udp_carrier(&out).await;
        udp_round_trip(later.as_ref(), echo, b"later").await;
        assert_eq!(fake.udp_packets().last().unwrap().association, 1);
        // the server lost the old connections: nothing to dissociate there
        assert!(fake.dissociates().is_empty());
    }

    /// A UDP packet from the server tells the authentication went through,
    /// as a byte from a tunnel's target does: a later close with code 0 and
    /// no reason is the server's, not a refusal.
    #[tokio::test]
    async fn an_answer_tells_the_authentication_went_through() {
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        // nothing read from it
        let mut tcp = tunnel(&out, &target(tcp_echo)).await;
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), udp_echo, b"answered").await;
        fake.close_connections();
        assert_eq!(
            end_of(&mut tcp).await.to_string(),
            "tuic: the server closed the connection (code 0x0)"
        );
    }

    /// Design 6.2: the server answers no authentication, so a carrier comes
    /// up all the same; its packets go until the refusal closes the
    /// connection. The send that finds it so fails, once, as Hysteria 2's
    /// carrier fails where its authentication is refused; the next one binds
    /// anew, on a new connection.
    #[tokio::test]
    async fn a_refused_authentication_fails_a_send_once_and_the_next_binds_anew() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = outbound(&fake, &credentials(ID, "wrong"));
        let carrier = udp_carrier(&out).await;
        let deadline = Instant::now() + BOUND;
        let err = loop {
            assert!(Instant::now() < deadline, "no send failed");
            let sent = timeout(BOUND, carrier.send_to(b"x", &target(echo)))
                .await
                .expect("an outcome within the bound");
            match sent {
                Ok(()) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(e) => break e,
            }
        };
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(err.to_string(), "tuic: authentication failed");
        timeout(BOUND, carrier.send_to(b"y", &target(echo)))
            .await
            .expect("a send within the bound")
            .unwrap();
        eventually("the next authentication", || fake.auths().len() == 2).await;
        assert!(fake.auths().iter().all(|auth| !auth.accepted));
        assert!(fake.udp_packets().is_empty());
    }

    /// Dials directly, leads where it is told to, and counts the times it
    /// is asked where it leads, as a group that is the `underlying-proxy`
    /// leads to its member.
    struct Rerouted {
        inner: DirectConnector,
        route: Mutex<Arc<u8>>,
        asked: AtomicUsize,
    }

    impl Rerouted {
        fn new() -> Rerouted {
            Rerouted {
                inner: DirectConnector::new(Arc::new(SystemResolve)),
                route: Mutex::new(Arc::new(1)),
                asked: AtomicUsize::new(0),
            }
        }

        fn asked(&self) -> usize {
            self.asked.load(Ordering::Relaxed)
        }
    }

    impl Connector for Rerouted {
        fn connect<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            self.inner.connect(target, opts)
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            self.inner.connect_udp(target, opts)
        }

        fn route(&self) -> Option<Route> {
            self.asked.fetch_add(1, Ordering::Relaxed);
            Some(Route::of(&self.route.lock().unwrap()))
        }
    }

    /// Let go of by the client: closed as its session closes a connection,
    /// with code 0.
    async fn closed_by_the_client(connection: &quinn::Connection) {
        let closed = timeout(BOUND, connection.closed()).await.expect("closed");
        assert!(
            matches!(&closed, ConnectionError::ApplicationClosed(close)
                if close.error_code == VarInt::from_u32(0)),
            "{closed:?}"
        );
    }

    /// Phase 2 M7 design 3.5 and 6.2: the underlying policy leads elsewhere
    /// now (a group's member changed). Each carrier's next packet goes on a
    /// new connection, dialled the new way, under the id it gives; the
    /// associations left behind are dissociated on the connection before,
    /// which the tunnel under way keeps — closed once nothing holds it. Said
    /// once in the log, which names the policy and nothing secret.
    #[tokio::test]
    async fn carriers_move_off_a_connection_whose_underlying_policy_changed() {
        let (log, _logging, _second) = Log::capture();
        let (udp_echo, tcp_echo) = (udp_echo_server().await, echo_server().await);
        let fake = FakeTuic::spawn(script());
        let rerouted = Arc::new(Rerouted::new());
        let out = built(&fake, rerouted.clone(), heartbeat::every(Version::V5));
        let (one, two) = (udp_carrier(&out).await, udp_carrier(&out).await);
        udp_round_trip(one.as_ref(), udp_echo, b"one").await;
        udp_round_trip(two.as_ref(), udp_echo, b"two").await;
        let mut under_way = tunnel(&out, &target(tcp_echo)).await;
        let lease = held(&out).await;
        *rerouted.route.lock().unwrap() = Arc::new(2);
        // A carrier asks where the policy leads at most once a second (its
        // own look: `a_carrier_finds_a_changed_route_by_itself`). Here
        // someone else looks first, as a request would, and the carriers
        // see the connection off the way at once.
        assert!(!out.session.serves(&lease));
        drop(lease);
        udp_round_trip(one.as_ref(), udp_echo, b"one, moved").await;
        udp_round_trip(two.as_ref(), udp_echo, b"two, moved").await;
        round_trip(&mut under_way, b"still there").await;
        assert_eq!(carried(&fake), [(0, 0), (0, 1), (1, 0), (1, 1)]);
        assert_eq!(fake.connections(), 2);
        eventually("the Dissociates", || fake.dissociates().len() == 2).await;
        let mut dissociated: Vec<(usize, u32)> = fake
            .dissociates()
            .iter()
            .map(|d| (d.connection, d.association))
            .collect();
        dissociated.sort_unstable();
        assert_eq!(dissociated, [(0, 0), (0, 1)]);
        drop(under_way);
        closed_by_the_client(&fake.connection(0)).await;
        let log = log.text();
        let changed = "tuic: the underlying policy changed; a new connection is made";
        assert_eq!(log.matches(changed).count(), 1, "{log}");
        assert!(log.contains("policy=T"), "{log}");
        for secret in [PASSWORD, ID] {
            assert!(!log.contains(secret));
        }
    }

    /// Where the policy leads can cost a group's resolution: a carrier asks
    /// at most once a second, however many packets it sends meanwhile.
    #[tokio::test]
    async fn the_route_is_compared_at_most_once_a_second_on_udp() {
        let to = target(digest_target().await);
        let fake = FakeTuic::spawn(script());
        let rerouted = Arc::new(Rerouted::new());
        let out = built(&fake, rerouted.clone(), heartbeat::every(Version::V5));
        let carrier = udp_carrier(&out).await;
        let before = rerouted.asked();
        for i in 0..50u8 {
            timeout(BOUND, carrier.send_to(&[i], &to))
                .await
                .expect("a send within the bound")
                .unwrap();
        }
        let asked = rerouted.asked() - before;
        assert!(asked <= 3, "asked {asked} times for 50 packets");
    }

    /// A carrier finds out for itself, at its next comparison of the
    /// route, that the policy leads elsewhere now: its packets then go on a
    /// new connection. Until then — a second at most — they go on the one
    /// before.
    #[tokio::test]
    async fn a_carrier_finds_a_changed_route_by_itself() {
        let to = target(digest_target().await);
        let fake = FakeTuic::spawn(script());
        let rerouted = Arc::new(Rerouted::new());
        let out = built(&fake, rerouted.clone(), heartbeat::every(Version::V5));
        let carrier = udp_carrier(&out).await;
        timeout(BOUND, carrier.send_to(b"first", &to))
            .await
            .expect("a send within the bound")
            .unwrap();
        *rerouted.route.lock().unwrap() = Arc::new(2);
        let deadline = Instant::now() + BOUND;
        while !fake.udp_packets().iter().any(|p| p.connection == 1) {
            assert!(
                Instant::now() < deadline,
                "the carrier never left its connection"
            );
            timeout(BOUND, carrier.send_to(b"probe", &to))
                .await
                .expect("a send within the bound")
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(fake.connections(), 2);
    }

    /// A carrier that does not read loses what its inbox cannot hold, and
    /// holds up no other carrier of the connection.
    #[tokio::test]
    async fn a_full_inbox_drops_rather_than_holds_up_the_other_carriers() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let (slow, other) = (udp_carrier(&out).await, udp_carrier(&out).await);
        udp_round_trip(slow.as_ref(), echo, b"slow").await;
        udp_round_trip(other.as_ref(), echo, b"other").await;
        assert_eq!(carried(&fake), [(0, 0), (0, 1)]);
        let from = "192.0.2.9:9";
        for i in 0..udp::INBOX as u16 + 10 {
            fake.send_udp(0, &TuicPacket::whole(0, i, from, &i.to_be_bytes()));
        }
        fake.send_udp(0, &TuicPacket::whole(1, 0, from, b"not held up"));
        assert_eq!(udp_answer(other.as_ref()).await.0, b"not held up");
        for i in 0..udp::INBOX as u16 {
            assert_eq!(udp_answer(slow.as_ref()).await.0, i.to_be_bytes());
        }
        no_udp_answer(slow.as_ref()).await;
    }

    /// Design 6.2: where the server takes no QUIC datagrams, a send hands
    /// its packet to the connection's writer and waits for nothing more, so
    /// that one its caller gives up on — the engine bounds every send, and
    /// cuts one short when its association ends — still goes whole. Cut
    /// short on the wire, it would be a stream sing-box cannot read, for
    /// which it closes the connection, the tunnel beside it too, as the fake
    /// does. Here the server holds the packet's stream unread, its window
    /// shorter than the packet, until the send was given up on.
    #[tokio::test]
    async fn a_send_given_up_on_never_cuts_its_packet_short() {
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            stream_window: Some(8),
            ..script()
        });
        let out = user(&fake);
        let mut tcp = tunnel(&out, &target(tcp_echo)).await;
        round_trip(&mut tcp, b"up").await;
        let carrier = udp_carrier(&out).await;
        let to = target(udp_echo);
        fake.hold_streams();
        let held = || fake.held_streams() == 1;
        // given up on once the server holds its stream, if not done by then
        tokio::select! {
            sent = carrier.send_to(b"whole", &to) => sent.unwrap(),
            () = eventually("the packet's stream held", held) => {}
        }
        eventually("the packet's stream held", held).await;
        fake.release_streams();
        assert_eq!(udp_answer(carrier.as_ref()).await, (b"whole".to_vec(), to));
        round_trip(&mut tcp, b"still up").await;
        assert_eq!(fake.connections(), 1);
    }

    /// Design 6.2: a send never waits for the connection's writer. Behind a
    /// stream the server does not read, the writer holds one packet half
    /// sent and `OUTBOX` more wait for it; the packets past them are
    /// dropped, the send `Ok` all the same — as a datagram the connection
    /// cannot take is lost —, which the log tells without their payload.
    /// The ones that waited go once the server reads on.
    #[tokio::test]
    async fn a_full_outbox_drops_rather_than_holds_up_a_send() {
        const MARK: &[u8] = b"payload-3f81c2";
        let (log, _logging, _second) = Log::capture();
        // takes the packets and answers none
        let quiet = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let to = target(quiet.local_addr().unwrap());
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            stream_window: Some(8),
            ..script()
        });
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        // each its own length, which tells them apart on the server
        let payload = |len: usize| -> Vec<u8> { MARK.iter().copied().cycle().take(len).collect() };
        // the authentication's own stream is not held
        eventually("the authentication", || fake.auths().len() == 1).await;
        fake.hold_streams();
        let first = payload(100);
        timeout(BOUND, carrier.send_to(&first, &to))
            .await
            .expect("the first packet handed over")
            .unwrap();
        eventually("its stream held", || fake.held_streams() == 1).await;
        for len in 1..=udp::OUTBOX + 10 {
            let packet = payload(len);
            let sent = timeout(Duration::from_millis(100), carrier.send_to(&packet, &to)).await;
            sent.expect("a send that does not wait").unwrap();
        }
        fake.release_streams();
        let mut expected: Vec<usize> = (1..=udp::OUTBOX).chain([100]).collect();
        eventually("the packets that waited", || {
            fake.udp_packets().len() == expected.len()
        })
        .await;
        // none of the others after them: a fixed wait, only to see nothing
        // happen
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut lens: Vec<usize> = fake.udp_packets().iter().map(|p| p.len).collect();
        lens.sort_unstable();
        expected.sort_unstable();
        assert_eq!(lens, expected);
        let log = log.text();
        let dropped = "tuic: a message for the server is dropped: too many wait for a stream";
        assert_eq!(log.matches(dropped).count(), 10, "{log}");
        assert!(!log.contains("payload-3f81c2"));
        assert_eq!(fake.connections(), 1);
    }

    /// Carriers that open together share one connection and one
    /// authentication, each an association of its own, with a socket of its
    /// own on the server.
    #[tokio::test]
    async fn carriers_that_come_together_share_one_connection_each_its_own_association() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = Arc::new(user(&fake));
        let mut opening = JoinSet::new();
        for i in 0..16u8 {
            let out = out.clone();
            opening.spawn(async move {
                let carrier = udp_carrier(&out).await;
                udp_round_trip(carrier.as_ref(), echo, &[i; 100]).await;
                carrier
            });
        }
        let mut carriers = Vec::new();
        while let Some(carrier) = opening.join_next().await {
            carriers.push(carrier.unwrap());
        }
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.auths().len(), 1);
        let associations: BTreeSet<u32> = carried(&fake).iter().map(|&(_, a)| a).collect();
        assert_eq!(associations, (0..16).collect());
        assert_eq!(fake.udp_outside().len(), 16);
    }

    /// A packet longer than UDP carries, or one whose target cannot be
    /// named, is refused — `InvalidInput`, which loses that packet alone —,
    /// and nothing goes out, in datagrams and on streams alike; the carrier
    /// goes on.
    #[tokio::test]
    async fn a_packet_too_long_is_refused_and_nothing_goes_out() {
        let echo = udp_echo_server().await;
        for no_datagrams in [false, true] {
            let fake = FakeTuic::spawn(TuicScript {
                no_datagrams,
                ..script()
            });
            let out = user(&fake);
            let carrier = udp_carrier(&out).await;
            let err = timeout(BOUND, carrier.send_to(&vec![0u8; 65_536], &target(echo)))
                .await
                .expect("an outcome within the bound")
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(err.to_string(), "tuic: a datagram longer than 65535 bytes");
            let long = Target::new(HostName::Domain("a".repeat(256)), 53);
            let err = timeout(BOUND, carrier.send_to(b"q", &long))
                .await
                .expect("an outcome within the bound")
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert!(fake.udp_packets().is_empty());
            udp_round_trip(carrier.as_ref(), echo, b"still").await;
        }
    }

    /// `opts.timeout` bounds opening a carrier: the dial and QUIC's
    /// handshake.
    #[tokio::test]
    async fn opening_a_carrier_is_bounded_by_its_time() {
        // nothing speaks QUIC on this port
        let quiet = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = quiet.local_addr().unwrap().port();
        let fake = FakeTuic::spawn(script());
        let out = outbound_with(&spec(port, &credentials(ID, PASSWORD)), port, &fake);
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        let started = Instant::now();
        let err = out.open_udp(&opts).await.err().unwrap();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    /// What packets carry shows nowhere — in no error and in no log line,
    /// not even at the finest level — whether they go through, are refused,
    /// are no packet, find no association, or go out again after a close.
    #[tokio::test]
    async fn payloads_never_show_in_errors_or_logs() {
        const MARK: &[u8] = b"payload-7c1e9a";
        let (log, _logging, _second) = Log::capture();
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(script());
        let out = user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, MARK).await;
        let err = timeout(BOUND, carrier.send_to(&MARK.repeat(5000), &target(echo)))
            .await
            .expect("an outcome within the bound")
            .unwrap_err();
        // no packet (its SIZE says otherwise), then one for no association:
        // read before the echo that follows them
        let mut junk = TuicPacket::whole(0, 9, "192.0.2.1:53", MARK).encode();
        junk.push(0);
        fake.connection(0).send_datagram(junk.into()).unwrap();
        fake.send_udp(0, &TuicPacket::whole(9, 1, "192.0.2.1:53", MARK));
        udp_round_trip(carrier.as_ref(), echo, MARK).await;
        fake.close_connections();
        until_answered(carrier.as_ref(), echo, MARK).await;
        let text = format!("{err} {err:?}");
        assert!(!text.contains("payload-7c1e9a"), "{text}");
        let log = log.text();
        assert!(
            log.contains("tuic: what the server sent is no packet; dropped"),
            "{log}"
        );
        assert!(
            log.contains("tuic: UDP association 0 moves to a new connection, as association 0"),
            "{log}"
        );
        assert!(!log.contains("payload-7c1e9a"));
    }

    /// TUIC v4's (`tuic`): the manual's example token.
    const TOKEN: &str = "pwd";

    /// `tuic, 127.0.0.1, <the fake's port>, <params>`.
    fn v4_outbound(fake: &FakeTuic, params: &str) -> TuicOutbound {
        let port = fake.addr().port();
        outbound_with(&spec_of("tuic", port, params), port, fake)
    }

    /// The v4 outbound the fake lets in.
    fn v4_user(fake: &FakeTuic) -> TuicOutbound {
        v4_outbound(fake, &format!("token={TOKEN}"))
    }

    /// Design 6.3: v4 authenticates each connection with the BLAKE3 hash of
    /// the token, on a stream of its own — the same on every connection,
    /// unlike v5's token.
    #[tokio::test]
    async fn v4_the_authentication_carries_the_hash_of_the_token() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"through tuic v4").await;
        let other = v4_user(&fake);
        let mut stream = tunnel(&other, &target(echo)).await;
        round_trip(&mut stream, b"on another connection").await;
        let auths = fake.auths();
        assert_eq!(auths.len(), 2);
        for (n, auth) in auths.iter().enumerate() {
            assert_eq!((auth.connection, auth.uuid), (n, None));
            assert_eq!(auth.token, auth.expected);
            assert!(auth.accepted);
        }
        assert_eq!(auths[0].token, auths[1].token);
        let carried: Vec<usize> = fake.connects().iter().map(|c| c.connection).collect();
        assert_eq!(carried, [0, 1]);
    }

    /// Design 6.3: v4's Connect waits for the server's response, which
    /// comes once the server reached the target; the tunnel begins after it
    /// — with what a target that speaks first sends.
    #[tokio::test]
    async fn v4_a_tunnel_begins_after_the_response_with_what_the_target_sends() {
        let greeter = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = greeter.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = greeter.accept().await {
                let _ = socket.write_all(b"220 ready").await;
                let mut rest = Vec::new();
                let _ = socket.read_to_end(&mut rest).await;
            }
        });
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        let mut stream = tunnel(&out, &target(at)).await;
        let mut greeting = [0u8; 9];
        timeout(BOUND, stream.read_exact(&mut greeting))
            .await
            .expect("the greeting within the bound")
            .unwrap();
        assert_eq!(&greeting, b"220 ready");
        assert_eq!(fake.connects()[0].address, at.to_string());
    }

    /// Design 6.3: a target the server cannot reach is its `ff` response:
    /// the request fails, saying so, and the connection goes on.
    #[tokio::test]
    async fn v4_a_target_the_server_cannot_reach_fails_the_request_and_the_connection_goes_on() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        // the fake resolves no name
        let err = failure(&out, &Target::new(HostName::parse("unknown.test"), 80)).await;
        assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
        assert_eq!(
            err.to_string(),
            "tuic: the server could not connect to the target"
        );
        let mut next = tunnel(&out, &target(echo)).await;
        round_trip(&mut next, b"still up").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.connects().len(), 2);
    }

    /// Design 6.3: a v4 request reads the response on the Connect's stream;
    /// a stream the server ends before it fails the request, saying so, and
    /// the connection goes on.
    #[tokio::test]
    async fn v4_a_connect_ended_without_a_response_fails_and_the_connection_goes_on() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            no_response: true,
            ..TuicScript::v4(TOKEN)
        });
        let out = v4_user(&fake);
        // the fake resolves no name
        let err = failure(&out, &Target::new(HostName::parse("unknown.test"), 80)).await;
        assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
        assert_eq!(
            err.to_string(),
            "tuic: the server ended the stream without a response"
        );
        let mut next = tunnel(&out, &target(echo)).await;
        round_trip(&mut next, b"still up").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.connects().len(), 2);
    }

    /// Design 6.3: a v4 server that does not know the token closes the
    /// connection with 0xfffffff1, before the Connect's response: the
    /// request fails with `tuic: authentication failed`. The next dials and
    /// authenticates again.
    #[tokio::test]
    async fn v4_a_wrong_token_fails_the_request_and_the_next_dials_again() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_outbound(&fake, "token=wrong");
        for _ in 0..2 {
            let err = failure(&out, &target(echo)).await;
            assert!(
                matches!(&err, OutboundError::Io(e) if e.kind() == io::ErrorKind::PermissionDenied),
                "{err:?}"
            );
            assert_eq!(err.to_string(), "tuic: authentication failed");
        }
        assert_eq!(fake.connections(), 2);
        let auths = fake.auths();
        assert_eq!(auths.len(), 2);
        assert!(auths.iter().all(|auth| !auth.accepted));
    }

    /// Design 6.3: the code a v4 server closes a connection with says what
    /// went wrong, the server's reason aside — its authentication refused,
    /// too late, or a protocol error —; any other code is told as v5's.
    #[tokio::test]
    async fn v4_the_servers_close_codes_say_what_went_wrong() {
        let echo = echo_server().await;
        for (code, reason, expected) in [
            (
                0xffff_fff1,
                "authentication failed",
                "tuic: authentication failed",
            ),
            (
                0xffff_fff2,
                "authentication timeout",
                "tuic: authentication timed out",
            ),
            (
                0xffff_fff0,
                "Unsupported TUIC version: 5",
                "tuic: protocol error",
            ),
            (0xffff_fff3, "bad command", "tuic: protocol error"),
            (
                0x2a,
                "",
                "tuic: the server closed the connection (code 0x2a)",
            ),
        ] {
            let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
            let out = v4_user(&fake);
            let mut stream = tunnel(&out, &target(echo)).await;
            round_trip(&mut stream, b"up").await;
            fake.close_connections_with(code, reason.as_bytes());
            let err = end_of(&mut stream).await.to_string();
            assert_eq!(err, expected, "{code:#x}");
        }
    }

    /// Design 3.5, 6.3: v4's Connect waits for a response, so a request
    /// that finds its connection gone before that came — the server
    /// restarted, and answers with a stateless reset — sent nothing of the
    /// caller's, and goes again, once, on a new connection. The tunnel
    /// under way on the lost connection ends.
    #[tokio::test]
    async fn v4_a_request_that_finds_the_server_restarted_goes_again_on_a_new_connection() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        let mut before = tunnel(&out, &target(echo)).await;
        round_trip(&mut before, b"before").await;
        fake.restart();
        let mut after = tunnel(&out, &target(echo)).await;
        round_trip(&mut after, b"after").await;
        assert_eq!(
            end_of(&mut before).await.to_string(),
            "quic: the server no longer knows the connection (stateless reset)"
        );
        let carried: Vec<usize> = fake.connects().iter().map(|c| c.connection).collect();
        assert_eq!(carried, [0, 1]);
        assert_eq!(fake.auths().len(), 2);
    }

    /// `opts.timeout` bounds a v4 request, its response included: a server
    /// that takes the Connect and never answers it.
    #[tokio::test]
    async fn v4_a_server_that_never_answers_the_connect_times_out() {
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            silent: true,
            ..TuicScript::v4(TOKEN)
        });
        let out = v4_user(&fake);
        // up already
        drop(held(&out).await);
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
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        eventually("the Connect", || fake.connects().len() == 1).await;
    }

    /// Design 6.3: v4's UDP goes whole in datagrams of the connection — no
    /// packet id, no fragments —, under one association id the carrier
    /// draws at random, and comes back so. A v4 server names an IPv4 source
    /// in IPv6 form (tuic 0.8.5's dual-stack socket): it reads as the IPv4
    /// address it is. Full cone.
    #[tokio::test]
    async fn v4_udp_goes_whole_in_datagrams_of_the_connection() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), one, b"to one").await;
        udp_round_trip(carrier.as_ref(), two, b"to two").await;
        let packets = fake.udp_packets();
        assert_eq!(packets.len(), 2);
        let association = packets[0].association;
        for (packet, to) in packets.iter().zip([one, two]) {
            assert_eq!((packet.connection, packet.association), (0, association));
            assert_eq!(
                (packet.packet, packet.fragments, packet.fragment),
                (0, 1, 0)
            );
            assert_eq!(packet.address, Some(to.to_string()));
            assert_eq!(packet.len, 6);
            assert!(!packet.on_stream);
        }
        let outside = fake.udp_outside();
        assert_eq!(outside.len(), 1, "one socket for the association");
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger.send_to(b"unasked", outside[0]).await.unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), target(stranger.local_addr().unwrap()))
        );
        assert_eq!(fake.connections(), 1);
    }

    /// Design 6.3: where the server takes no QUIC datagrams, each v4 packet
    /// goes whole on a unidirectional stream of its own — a long one too —,
    /// and the server answers on streams.
    #[tokio::test]
    async fn v4_without_datagrams_each_packet_goes_on_a_stream_of_its_own() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            ..TuicScript::v4(TOKEN)
        });
        let out = v4_user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"on a stream").await;
        let long: Vec<u8> = (0..9000).map(|i| (i % 251) as u8).collect();
        udp_round_trip(carrier.as_ref(), echo, &long).await;
        let packets = fake.udp_packets();
        let seen: Vec<(bool, u8, usize)> = packets
            .iter()
            .map(|p| (p.on_stream, p.fragments, p.len))
            .collect();
        assert_eq!(seen, [(true, 1, 11), (true, 1, 9000)]);
        assert_eq!(packets[0].association, packets[1].association);
        let lease = held(&out).await;
        assert!(lease.connection().max_datagram_size().is_none());
        assert_eq!(fake.connections(), 1);
    }

    /// Design 6.3: as v5's, a v4 send hands its packet to the connection's
    /// writer and waits for nothing more, so that one its caller gives up
    /// on still goes whole: tuic 0.8.5 closes the connection, the tunnel
    /// beside it too, on a stream that ends within its command's header, as
    /// the fake does. Here the server holds the packet's stream unread, its
    /// window shorter than the header, until the send was given up on.
    #[tokio::test]
    async fn v4_a_send_given_up_on_never_cuts_its_packet_short() {
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            stream_window: Some(8),
            ..TuicScript::v4(TOKEN)
        });
        let out = v4_user(&fake);
        let mut tcp = tunnel(&out, &target(tcp_echo)).await;
        round_trip(&mut tcp, b"up").await;
        let carrier = udp_carrier(&out).await;
        let to = target(udp_echo);
        fake.hold_streams();
        let held = || fake.held_streams() == 1;
        // given up on once the server holds its stream, if not done by then
        tokio::select! {
            sent = carrier.send_to(b"whole", &to) => sent.unwrap(),
            () = eventually("the packet's stream held", held) => {}
        }
        eventually("the packet's stream held", held).await;
        fake.release_streams();
        assert_eq!(udp_answer(carrier.as_ref()).await, (b"whole".to_vec(), to));
        round_trip(&mut tcp, b"still up").await;
        assert_eq!(fake.connections(), 1);
    }

    /// Design 6.3: v4 cannot split a packet. One that no datagram of the
    /// connection holds is refused — `InvalidInput`, which loses that packet
    /// alone: the engine drops it, as any its outbound cannot frame —, and
    /// nothing goes out; the carrier goes on. Without datagrams, the same
    /// packet goes on a stream.
    #[tokio::test]
    async fn v4_a_packet_too_long_for_a_datagram_is_refused_and_nothing_goes_out() {
        // past what any datagram of the connection holds: path MTU discovery
        // stops at 1452 bytes
        let long = vec![7u8; 2000];
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        let carrier = udp_carrier(&out).await;
        let err = timeout(BOUND, carrier.send_to(&long, &target(echo)))
            .await
            .expect("an outcome within the bound")
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(
            err.to_string().starts_with("tuic: a datagram longer than "),
            "{err}"
        );
        assert!(fake.udp_packets().is_empty());
        udp_round_trip(carrier.as_ref(), echo, &[1u8; 1000]).await;
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            ..TuicScript::v4(TOKEN)
        });
        let out = v4_user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, &long).await;
    }

    /// Design 6.3: a v4 carrier gone dissociates its association — `04 03
    /// ‖ ASSOC_ID`, its 32 bits — on the connection it was on; the next
    /// carrier is an association of its own, with a socket of its own.
    #[tokio::test]
    async fn v4_a_carrier_gone_dissociates_its_association() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        let lease = held(&out).await;
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"here").await;
        let association = fake.udp_packets()[0].association;
        assert_eq!(lease.state().udp.associations(), [association]);
        drop(carrier);
        assert!(lease.state().udp.associations().is_empty());
        eventually("the Dissociate", || {
            fake.dissociates()
                == [RecordedDissociate {
                    connection: 0,
                    association,
                }]
        })
        .await;
        let next = udp_carrier(&out).await;
        udp_round_trip(next.as_ref(), echo, b"next").await;
        assert_eq!(fake.udp_outside().len(), 2);
        assert_eq!(fake.connections(), 1);
    }

    /// Design 6.3: v4's heartbeat — `04 04` on a stream of its own; a v4
    /// server closes the connection on one in a datagram, as the fake does
    /// — goes while a relay is under way, and not while none is, whether the
    /// server takes QUIC datagrams or not. Its interval is cut short here
    /// (10 seconds otherwise).
    #[tokio::test]
    async fn v4_heartbeats_go_on_streams_while_a_relay_is_under_way() {
        const EVERY: Duration = Duration::from_millis(100);
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        for no_datagrams in [false, true] {
            let fake = FakeTuic::spawn(TuicScript {
                no_datagrams,
                ..TuicScript::v4(TOKEN)
            });
            let spec = spec_of("tuic", fake.addr().port(), &format!("token={TOKEN}"));
            let out = built_from(&spec, &fake, direct(), EVERY);
            // up, and nothing relayed
            let lease = held(&out).await;
            assert!(lease.state().beats());
            no_heartbeat(&fake, EVERY).await;
            assert_eq!(fake.heartbeats(), 0);
            let mut tcp = tunnel(&out, &target(tcp_echo)).await;
            round_trip(&mut tcp, b"tcp").await;
            eventually("heartbeats with a tunnel", || fake.heartbeats() >= 2).await;
            drop(tcp);
            no_heartbeat(&fake, EVERY).await;
            let before = fake.heartbeats();
            let carrier = udp_carrier(&out).await;
            udp_round_trip(carrier.as_ref(), udp_echo, b"udp").await;
            eventually("heartbeats with a carrier", || {
                fake.heartbeats() >= before + 2
            })
            .await;
            drop(carrier);
            assert_eq!(lease.state().relays(), 0);
            // none came in a datagram: the server kept the connection
            assert!(lease.connection().close_reason().is_none());
            assert_eq!(fake.connections(), 1);
        }
    }

    /// Design 6.3: a v4 carrier whose connection ended goes on, on the
    /// connection that replaces it — whether the server closed it or lost
    /// it in a restart —, under an id drawn anew there.
    #[tokio::test]
    async fn v4_a_carrier_moves_to_a_new_connection_after_a_close_or_a_restart() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_user(&fake);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"first").await;
        fake.close_connections_with(0xffff_fff0, b"protocol error");
        until_answered(carrier.as_ref(), echo, b"after a close").await;
        fake.restart();
        until_answered(carrier.as_ref(), echo, b"after a restart").await;
        let connections: BTreeSet<usize> =
            fake.udp_packets().iter().map(|p| p.connection).collect();
        assert_eq!(connections, BTreeSet::from([0, 1, 2]));
        assert_eq!(fake.auths().len(), 3);
        // the server lost the old connections: nothing to dissociate there
        assert!(fake.dissociates().is_empty());
    }

    /// Design 6.3: as v5's, a v4 carrier comes up before the server's
    /// verdict on the token; the send that finds the connection closed for
    /// a token it did not know fails, once, and the next binds anew, on a
    /// new connection.
    #[tokio::test]
    async fn v4_a_wrong_token_fails_a_send_once_and_the_next_binds_anew() {
        let echo = udp_echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_outbound(&fake, "token=wrong");
        let carrier = udp_carrier(&out).await;
        let deadline = Instant::now() + BOUND;
        let err = loop {
            assert!(Instant::now() < deadline, "no send failed");
            let sent = timeout(BOUND, carrier.send_to(b"x", &target(echo)))
                .await
                .expect("an outcome within the bound");
            match sent {
                Ok(()) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(e) => break e,
            }
        };
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(err.to_string(), "tuic: authentication failed");
        timeout(BOUND, carrier.send_to(b"y", &target(echo)))
            .await
            .expect("a send within the bound")
            .unwrap();
        eventually("the next authentication", || fake.auths().len() == 2).await;
        assert!(fake.udp_packets().is_empty());
    }

    /// The token, and its hash — which lets a client in as the token does
    /// —, go to the server and nowhere else: not into the error of a refused
    /// authentication, nor into a build error, nor into a log line at any
    /// level.
    #[tokio::test]
    async fn v4_the_token_never_shows() {
        const SECRET: &str = "s3cret-tuic-v4";
        // computed apart, with the BLAKE3 from the specification that the
        // known answers of `a_v4_token_is_the_blake3_hash_of_the_token` come
        // from
        const HASH: &str = "557b8d354fae3c1f473c6f2d211cb1d827252b2f2473de4ad3425eb33b3cb1f9";
        let (log, _logging, _second) = Log::capture();
        let echo = echo_server().await;
        let fake = FakeTuic::spawn(TuicScript::v4(TOKEN));
        let out = v4_outbound(&fake, &format!("token={SECRET}"));
        let err = failure(&out, &target(echo)).await;
        assert_eq!(err.to_string(), "tuic: authentication failed");
        let sent: String = fake.auths()[0]
            .token
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(sent, HASH);
        let shown = format!("{err} {err:?}");
        for secret in [SECRET, HASH] {
            assert!(!shown.contains(secret), "{shown}");
        }
        let mut spec = spec_of("tuic", 443, &format!("token={SECRET}"));
        spec.version = TuicVersion::V4 { token: "".into() };
        let err = TuicOutbound::new(
            "T",
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            Tristate::Auto,
            &[],
            fake.roots(),
            direct(),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "`token` is empty");
        let log = log.text();
        assert!(!log.is_empty());
        for secret in [SECRET, HASH] {
            assert!(!log.contains(secret));
        }
    }
}
