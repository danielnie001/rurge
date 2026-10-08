//! `hysteria2` outbound (manual: Policies › Hysteria 2; phase 2 M7 design
//! 5): the policy's QUIC connection, which its requests share (`session`),
//! authenticated over HTTP/3 as it comes up (`auth`).
//!
//! - A TCP connection is a bidirectional stream of its own: the request
//!   names the target as `host:port` (an IPv6 literal in brackets, an IDN
//!   as its A-labels), and the server's answer lets the stream through or
//!   refuses it with a message (`tcp`).
//! - UDP goes in QUIC datagrams of the same connection, each client
//!   association a session of its own there (`udp`).
//! - Requests and the authentication are padded as the reference client
//!   pads them: letters and digits, of a random length.
//! - With `salamander-password` or `gecko-password`, every UDP packet of the
//!   connection is obfuscated (`obfs`).

mod auth;
mod obfs;
mod tcp;
mod udp;

use crate::session::{Dialer, Session};
use auth::Auth;
use obfs::Obfuscation;
use rurge_config::KeystoreItem;
use rurge_config::spec::{Hysteria2Spec, Tristate};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::http::wire_host;
use rurge_proto::{BuildError, Outbound, OutboundError, UdpSupport};
use rustls::RootCertStore;
use std::ops::RangeInclusive;
use std::sync::Arc;
use udp::{Hysteria2Udp, SessionIds};

/// The characters of padding: letters and digits, as the reference's
/// (`core/internal/protocol/padding.go`).
const PADDING: &[u8; 62] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// The longest address the reference reads, in a TCP request and in a UDP
/// message alike (`core/internal/protocol/proxy.go`).
const MAX_ADDRESS: usize = 2048;

/// `host:port` as Hysteria 2 names a target: an IPv6 literal in brackets,
/// a name as its A-labels (`http::wire_host`); `None` for a name that
/// cannot be sent, or an address longer than the server reads.
fn address(target: &Target) -> Option<String> {
    let address = format!("{}:{}", wire_host(target)?, target.port);
    (address.len() <= MAX_ADDRESS).then_some(address)
}

/// Padding of a random length in `lengths`, of random letters and digits.
/// Padding, not a secret: without a random source it is the shortest, all
/// `a`.
fn padding(lengths: RangeInclusive<usize>) -> Vec<u8> {
    let (min, max) = lengths.into_inner();
    let span = (max - min + 1) as u32;
    let len = min + getrandom::u32().map_or(0, |pick| (pick % span) as usize);
    let mut padding = vec![0u8; len];
    let _ = getrandom::fill(&mut padding);
    for byte in &mut padding {
        *byte = PADDING[usize::from(*byte) % PADDING.len()];
    }
    padding
}

/// No `Debug`: it holds the password.
pub struct Hysteria2Outbound {
    name: String,
    /// Shared with its UDP carriers, which may outlive it.
    session: Arc<Session<Auth>>,
    /// The session ids of its UDP carriers.
    ids: Arc<SessionIds>,
}

impl Hysteria2Outbound {
    /// The connection goes as `connector` dials (design 3.3), with the
    /// policy's `ecn` and obfuscation; `client-cert` comes from `keystore`.
    pub fn new(
        name: &str,
        server: Target,
        spec: &Hysteria2Spec,
        ecn: Tristate,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<Hysteria2Outbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it; the configuration checked the
        // password, but a spec made by hand may not be
        let password = spec.password.expose();
        if password.is_empty() {
            return Err(BuildError::new("`password` is empty"));
        }
        let Some(auth) = Auth::new(password, spec.download_bandwidth) else {
            return Err(BuildError::new(
                "`password` cannot be sent in an HTTP field",
            ));
        };
        let mut dialer = Dialer::new(
            connector, server, &spec.tls, &spec.quic, ecn, keystore, roots,
        )?;
        if let Some(obfs) = &spec.obfs {
            let obfuscation = Obfuscation::new(obfs)?;
            dialer = dialer.with_layer(Box::new(move |socket| obfuscation.over(socket)));
        }
        Ok(Hysteria2Outbound {
            name: name.to_string(),
            session: Arc::new(Session::new(name, dialer, auth)),
            ids: Arc::default(),
        })
    }
}

impl Outbound for Hysteria2Outbound {
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
            // handshake, the authentication and the server's answer
            let tunnel = self
                .session
                .request(opts, |lease| Box::pin(tcp::open(lease, &request)));
            match tokio::time::timeout(opts.timeout, tunnel).await {
                Ok(tunnel) => Ok(Box::new(tunnel?) as BoxedStream),
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }

    /// Whatever the policy says: Hysteria 2 always relays UDP (manual). A
    /// server that relays none refuses the carriers (`open_udp`).
    fn udp(&self) -> UdpSupport {
        UdpSupport::Native
    }

    /// A session of its own on the policy's connection, dialed when there
    /// is none. The server's word on UDP comes with the authentication: a
    /// server that relays none refuses the carrier here (design 5.3).
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            // one budget for the connection (when one is dialed), QUIC's
            // handshake and the authentication
            let open = Hysteria2Udp::open(self.session.clone(), self.ids.clone(), opts);
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
    use crate::test_log::Log;
    use crate::testing::{FakeHysteria2, Hysteria2Script, RecordedUdp, UdpMessage};
    use quinn::{ConnectionError, VarInt};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::hysteria2::read_hysteria2;
    use rurge_config::spec::{CommonOpts, Hysteria2Obfs, ParamReader, PortHopping};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{
        BoxedDatagram, DirectConnector, PacketSocket, Route, SystemResolve,
    };
    use rurge_net::quic::testing::MultiPort;
    use rurge_proto::testing::{echo_server, udp_echo_server};
    use std::collections::{BTreeSet, HashSet};
    use std::io;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::task::JoinSet;
    use tokio::time::{Instant, timeout};

    const BOUND: Duration = Duration::from_secs(10);

    fn script(password: &str) -> Hysteria2Script {
        Hysteria2Script {
            password: password.to_string(),
            ..Hysteria2Script::default()
        }
    }

    /// The spec of `hysteria2, 127.0.0.1, <port>, <params>`, as the
    /// configuration reads it.
    fn spec(port: u16, params: &str) -> Hysteria2Spec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let line = format!("hysteria2, 127.0.0.1, {port}, {params}");
        let policy = parse_policy("H", &line, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_hysteria2(&mut r, &mut CommonOpts::default(), &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        spec
    }

    /// `spec`'s outbound to `127.0.0.1:<port>`, trusting `fake`.
    fn outbound_with(spec: &Hysteria2Spec, port: u16, fake: &FakeHysteria2) -> Hysteria2Outbound {
        Hysteria2Outbound::new(
            "H",
            Target::new(HostName::parse("127.0.0.1"), port),
            spec,
            Tristate::Auto,
            &[],
            fake.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// `hysteria2, 127.0.0.1, <the fake's port>, <params>`.
    fn outbound(fake: &FakeHysteria2, params: &str) -> Hysteria2Outbound {
        let port = fake.addr().port();
        outbound_with(&spec(port, params), port, fake)
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn tunnel(out: &Hysteria2Outbound, to: &Target) -> BoxedStream {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel")
    }

    async fn failure(out: &Hysteria2Outbound, to: &Target) -> OutboundError {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .err()
            .expect("no tunnel")
    }

    async fn round_trip(stream: &mut BoxedStream, payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        let mut back = vec![0u8; payload.len()];
        timeout(BOUND, stream.read_exact(&mut back))
            .await
            .expect("the echo arrives")
            .unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn padding_is_letters_and_digits_of_a_length_in_range() {
        let mut lengths = HashSet::new();
        for _ in 0..200 {
            let padding = padding(64..=511);
            assert!((64..=511).contains(&padding.len()), "{}", padding.len());
            assert!(padding.iter().all(u8::is_ascii_alphanumeric));
            lengths.insert(padding.len());
        }
        assert!(lengths.len() > 10, "{lengths:?}");
        assert_eq!(padding(7..=7).len(), 7);
    }

    /// Design 5.2: the fields the server reads, and what its answer says of
    /// UDP is kept with the connection.
    #[tokio::test]
    async fn the_authentication_carries_the_password_the_bandwidth_and_padding() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw, download-bandwidth=100");
        assert_eq!(out.name(), "H");
        assert!(out.http_forward().is_none());
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"through hysteria").await;
        let auths = fake.auths();
        assert_eq!(auths.len(), 1);
        let auth = &auths[0];
        for (name, value) in [
            (":method", "POST"),
            (":scheme", "https"),
            (":authority", "hysteria"),
            (":path", "/auth"),
            ("hysteria-auth", "pw"),
            // 100 Mbps in bytes per second
            ("hysteria-cc-rx", "12500000"),
        ] {
            assert_eq!(auth.field(name), Some(value), "{name}");
        }
        let padding = auth.field("hysteria-padding").expect("padding");
        assert!((256..=2047).contains(&padding.len()), "{}", padding.len());
        assert!(padding.bytes().all(|b| b.is_ascii_alphanumeric()));
        assert_eq!(auth.fields.len(), 7, "nothing else");
        let requests = fake.tcp_requests();
        assert_eq!(requests[0].address, echo.to_string());
        assert!((64..=511).contains(&requests[0].padding));
        let lease = out.session.lease(&ConnectOpts::default()).await.unwrap();
        assert!(lease.state().udp.is_some());

        // without a bandwidth the rate is unknown; a server that relays no
        // UDP says so
        let fake = FakeHysteria2::spawn(Hysteria2Script {
            no_udp: true,
            ..script("pw")
        });
        let out = outbound(&fake, "password=pw");
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        assert!(lease.state().udp.is_none());
        assert_eq!(fake.auths()[0].field("hysteria-cc-rx"), Some("0"));
    }

    /// The server answers a wrong password as a web site would: the
    /// authentication failed, and the client closes the connection as the
    /// reference client does (H3_GENERAL_PROTOCOL_ERROR). The next request
    /// tries again.
    #[tokio::test]
    async fn a_wrong_password_fails_the_authentication_and_closes_the_connection() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("right"));
        let out = outbound(&fake, "password=wrong");
        for attempt in 0..2 {
            let err = failure(&out, &target(echo)).await;
            assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
            assert_eq!(err.to_string(), "hysteria2: authentication failed");
            let closed = timeout(BOUND, fake.connection(attempt).closed())
                .await
                .expect("closed");
            assert!(
                matches!(&closed, ConnectionError::ApplicationClosed(close)
                    if close.error_code == VarInt::from_u32(0x101)),
                "{closed:?}"
            );
        }
        assert_eq!(fake.auths().len(), 2);
        assert!(fake.tcp_requests().is_empty());
    }

    /// Past both sides' stream windows (8 MiB ours, quinn's 1.25 MB the
    /// server's); the shutdown reaches the target as a FIN, and the
    /// target's close comes back as the end of the stream.
    #[tokio::test]
    async fn a_tunnel_carries_a_large_payload_and_a_half_close() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
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

    /// Design 5.3: the server's message, without control characters and at
    /// most 200 characters; a message longer than the reference reads makes
    /// the answer malformed. A refused stream costs its connection nothing.
    #[tokio::test]
    async fn a_refusal_quotes_the_servers_message_cleaned_and_cut_short() {
        let echo = echo_server().await;
        let refused = "hysteria2: the server refused the connection";
        for (message, expected) in [
            (
                format!("no route\r\n\x1b[31m{}", "é".repeat(300)),
                format!("{refused}: no route[31m{}", "é".repeat(188)),
            ),
            (String::new(), refused.to_string()),
            // the longest message the reference reads, and one byte more
            ("x".repeat(2048), format!("{refused}: {}", "x".repeat(200))),
            (
                "x".repeat(2049),
                "hysteria2: the server's answer is malformed".to_string(),
            ),
        ] {
            let fake = FakeHysteria2::spawn(Hysteria2Script {
                refuse: Some(message),
                ..script("pw")
            });
            let out = outbound(&fake, "password=pw");
            for _ in 0..2 {
                let err = failure(&out, &target(echo)).await;
                assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
                assert_eq!(err.to_string(), expected);
            }
            assert_eq!(fake.connections(), 1);
        }
    }

    #[tokio::test]
    async fn names_go_out_as_a_labels_ipv6_in_brackets_and_an_unsendable_name_never_dials() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(Hysteria2Script {
            connect_to: Some(echo),
            ..script("pw")
        });
        let out = outbound(&fake, "password=pw");
        for (host, address) in [
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:443"),
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:443",
            ),
            (HostName::parse("remote.example"), "remote.example:443"),
        ] {
            let mut stream = tunnel(&out, &Target::new(host, 443)).await;
            round_trip(&mut stream, b"x").await;
            assert_eq!(fake.tcp_requests().last().unwrap().address, address);
        }
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        // `name:80` one byte longer than the reference reads
        let long = "a".repeat(2046);
        for name in [
            "x@blocked.test",
            "a b.test",
            "a.test\r\nx: 1",
            "",
            long.as_str(),
        ] {
            let err = failure(&out, &Target::new(HostName::Domain(name.into()), 80)).await;
            assert_eq!(
                err.to_string(),
                "hysteria2: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.connections(), 0);
    }

    /// One dial and one authentication for the tunnels that come together,
    /// and the connection carries those after them too.
    #[tokio::test]
    async fn tunnels_that_come_together_share_one_connection() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let to = target(echo);
        let (a, b, c) = tokio::join!(tunnel(&out, &to), tunnel(&out, &to), tunnel(&out, &to));
        for (mut stream, payload) in [(a, b"one"), (b, b"two"), (c, b"six")] {
            round_trip(&mut stream, payload).await;
        }
        let mut later = tunnel(&out, &to).await;
        round_trip(&mut later, b"later").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.auths().len(), 1);
        assert_eq!(fake.tcp_requests().len(), 4);
    }

    /// A tunnel under way ends with its connection; the next one dials and
    /// authenticates again.
    #[tokio::test]
    async fn after_the_server_closes_the_connection_the_next_tunnel_dials_again() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let mut first = tunnel(&out, &target(echo)).await;
        round_trip(&mut first, b"first").await;
        fake.close_connections();
        let mut rest = Vec::new();
        let err = timeout(BOUND, first.read_to_end(&mut rest))
            .await
            .expect("the end of the tunnel")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "quic: the server closed the connection (code 0x100)"
        );
        let mut second = tunnel(&out, &target(echo)).await;
        round_trip(&mut second, b"second").await;
        assert_eq!(fake.connections(), 2);
        assert_eq!(fake.auths().len(), 2);
    }

    /// Design 3.5: the server restarts — losing its connections — as a
    /// request comes. The request finds its connection gone (a stateless
    /// reset answers the client's next packet) before any of the caller's
    /// data went out, and goes again, once, on a new connection. A restart
    /// between two requests is no harder.
    #[tokio::test]
    async fn a_request_that_finds_the_server_restarted_goes_again_on_a_new_connection() {
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let mut before = tunnel(&out, &target(echo)).await;
        round_trip(&mut before, b"before").await;
        fake.restart_at_next_request();
        // room for a keep-alive (10 seconds) to bring the reset, should the
        // lost endpoint have acknowledged the request before it fell silent
        let opts = ConnectOpts {
            timeout: Duration::from_secs(25),
        };
        let mut after = timeout(
            Duration::from_secs(30),
            out.connect_tcp(&target(echo), &opts),
        )
        .await
        .expect("an outcome within the bound")
        .expect("a tunnel");
        round_trip(&mut after, b"after").await;
        let carried: Vec<usize> = fake.tcp_requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 0, 1]);
        assert_eq!(fake.auths().len(), 2);
        let mut rest = Vec::new();
        let err = timeout(BOUND, before.read_to_end(&mut rest))
            .await
            .expect("the end of the tunnel")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "quic: the server no longer knows the connection (stateless reset)"
        );

        fake.restart();
        let mut later = timeout(
            Duration::from_secs(30),
            out.connect_tcp(&target(echo), &opts),
        )
        .await
        .expect("an outcome within the bound")
        .expect("a tunnel");
        round_trip(&mut later, b"later").await;
        assert_eq!(fake.connections(), 3);
        assert_eq!(fake.tcp_requests().last().unwrap().connection, 2);
    }

    /// Design 3.3: the port changes under the connection — a new local
    /// socket each time — and its tunnels, old and new, go on. The interval
    /// is set past the configuration's minimum of 5 seconds.
    #[tokio::test]
    async fn tunnels_keep_working_while_the_port_hops() {
        let echo = echo_server().await;
        let ports = MultiPort::bind(3);
        let fake = FakeHysteria2::spawn_on(script("pw"), ports.clone());
        let mut spec = spec(9, "password=pw");
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

    /// `opts.timeout` bounds the whole request: the dial, the handshakes
    /// and the answer.
    #[tokio::test]
    async fn a_server_that_never_answers_times_out() {
        let echo = echo_server().await;
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        // nothing speaks QUIC on this port
        let quiet = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = quiet.local_addr().unwrap().port();
        let fake = FakeHysteria2::spawn(Hysteria2Script {
            silent: true,
            ..script("pw")
        });
        let out = outbound_with(&spec(port, "password=pw"), port, &fake);
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
        // a server that takes the request and never answers it, on a
        // connection that is up already
        let out = outbound(&fake, "password=pw");
        drop(
            timeout(BOUND, out.session.lease(&ConnectOpts::default()))
                .await
                .unwrap()
                .unwrap(),
        );
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
        assert_eq!(fake.tcp_requests().len(), 1, "the request went out");
    }

    /// The password goes to the server and nowhere else: not into the error
    /// of a failed authentication, nor into a build error.
    #[tokio::test]
    async fn the_password_never_shows() {
        const PASSWORD: &str = "s3cret-hy2";
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("another"));
        let out = outbound(&fake, &format!("password={PASSWORD}"));
        let err = failure(&out, &target(echo)).await;
        assert_eq!(err.to_string(), "hysteria2: authentication failed");
        assert!(!format!("{err:?}").contains(PASSWORD));
        assert_eq!(fake.auths()[0].field("hysteria-auth"), Some(PASSWORD));
        for (password, message) in [
            ("", "`password` is empty"),
            ("s3cret\nhy2", "`password` cannot be sent in an HTTP field"),
        ] {
            let mut spec = spec(443, "password=x");
            spec.password = password.into();
            let err = Hysteria2Outbound::new(
                "H",
                Target::new(HostName::parse("127.0.0.1"), 443),
                &spec,
                Tristate::Auto,
                &[],
                fake.roots(),
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(err.message, message);
        }
    }

    async fn udp_carrier(out: &Hysteria2Outbound) -> BoxedPacketSocket {
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
        carrier.send_to(payload, &target(to)).await.unwrap();
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
            carrier.send_to(payload, &target(to)).await.unwrap();
            let wait = Instant::now() + Duration::from_millis(500);
            while let Ok(got) = tokio::time::timeout_at(wait, carrier.recv_from(&mut buf)).await {
                let (n, _) = got.unwrap();
                if buf[..n] == *payload {
                    return;
                }
            }
        }
    }

    /// Design 5.3: a datagram goes whole in one QUIC datagram of the
    /// connection, in session 1. The carrier reaches any target through
    /// its session's one socket on the server, and hears anyone who sends
    /// to that socket (full cone); TCP goes on beside it.
    #[tokio::test]
    async fn udp_goes_in_datagrams_of_the_connection_to_any_target() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let tcp_echo = echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), one, b"to one").await;
        udp_round_trip(carrier.as_ref(), two, b"to two").await;
        let whole = |to: SocketAddr| RecordedUdp {
            connection: 0,
            session: 1,
            packet: 0,
            fragment: 0,
            fragments: 1,
            address: to.to_string(),
            len: 6,
        };
        assert_eq!(fake.udp_messages(), [whole(one), whole(two)]);
        let outside = fake.udp_outside();
        assert_eq!(outside.len(), 1, "one socket for the session");
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

    /// A name goes to the server as its A-labels, an IPv6 address in
    /// brackets; the answer names its source as the server heard it. A
    /// name that cannot be sent is refused, and nothing goes out.
    #[tokio::test]
    async fn udp_names_go_out_as_a_labels_and_ipv6_in_brackets() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(Hysteria2Script {
            connect_to: Some(echo),
            ..script("pw")
        });
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        for (host, address) in [
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:53",
            ),
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:53"),
        ] {
            carrier.send_to(b"q", &Target::new(host, 53)).await.unwrap();
            assert_eq!(
                udp_answer(carrier.as_ref()).await,
                (b"q".to_vec(), target(echo))
            );
            assert_eq!(fake.udp_messages().last().unwrap().address, address);
        }
        // `name:80` one byte longer than the reference reads
        let long = "a".repeat(2046);
        for name in ["x@blocked.test", "a b.test", "", long.as_str()] {
            let err = carrier
                .send_to(b"q", &Target::new(HostName::Domain(name.into()), 80))
                .await
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                "hysteria2: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.udp_messages().len(), 2);
    }

    /// Design 5.3: a datagram too long for one QUIC datagram goes in
    /// fragments, each with the whole header, under one packet id that is
    /// not 0; the echo, as long, comes back in fragments of the server's
    /// that the carrier puts together. (At most 9216 bytes: macOS sends
    /// no longer UDP datagram by default.)
    #[tokio::test]
    async fn long_datagrams_go_in_fragments_both_ways() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        for len in [3000, 9000] {
            let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            udp_round_trip(carrier.as_ref(), echo, &payload).await;
            let messages = fake.udp_messages();
            let count = messages.last().unwrap().fragments;
            assert!(count > 1, "{count}");
            let fragments = &messages[messages.len() - usize::from(count)..];
            let packet = fragments[0].packet;
            assert_ne!(packet, 0);
            for (i, fragment) in fragments.iter().enumerate() {
                assert_eq!(
                    (fragment.session, fragment.packet, fragment.fragment),
                    (1, packet, i as u8)
                );
                assert_eq!(fragment.address, echo.to_string());
            }
            assert_eq!(fragments.iter().map(|f| f.len).sum::<usize>(), len);
        }
    }

    /// Design 5.3: a fragment lost loses its datagram and nothing more:
    /// the next one, its fragments under another packet id, comes whole.
    #[tokio::test]
    async fn a_lost_fragment_loses_its_datagram_alone() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(Hysteria2Script {
            drop_fragment: Some(1),
            ..script("pw")
        });
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        let (lost, kept) = (vec![1u8; 3000], vec![2u8; 3000]);
        for payload in [&lost[..], &kept, b"short"] {
            carrier.send_to(payload, &target(echo)).await.unwrap();
        }
        // in the order they came: the first never came whole
        assert_eq!(udp_answer(carrier.as_ref()).await.0, kept);
        assert_eq!(udp_answer(carrier.as_ref()).await.0, b"short");
        no_udp_answer(carrier.as_ref()).await;
    }

    /// The fragments of a newer datagram drop the one under way: its last
    /// fragment, come late, makes nothing whole, and holds nothing up.
    #[tokio::test]
    async fn a_newer_datagram_drops_an_older_one_under_way() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"hello").await;
        let session = fake.udp_messages()[0].session;
        let from = "192.0.2.7:7";
        for (packet, fragment, payload) in [
            (7, 0, "old-"),
            (8, 0, "new-"),
            (8, 1, "one"),
            (7, 1, "late"),
        ] {
            fake.send_udp(
                0,
                &UdpMessage {
                    session,
                    packet,
                    fragment,
                    fragments: 2,
                    address: from.to_string(),
                    payload: payload.into(),
                },
            );
        }
        fake.send_udp(0, &UdpMessage::whole(session, from, b"after"));
        let source = Target::new(HostName::parse("192.0.2.7"), 7);
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"new-one".to_vec(), source.clone())
        );
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"after".to_vec(), source)
        );
    }

    /// Design 5.3: the server's word on UDP comes with the authentication.
    /// A server that relays none refuses the carrier, as one that takes no
    /// QUIC datagrams does; TCP goes on, on the same connection.
    #[tokio::test]
    async fn a_server_that_relays_no_udp_refuses_the_carrier() {
        let echo = echo_server().await;
        for (server, refusal) in [
            (
                Hysteria2Script {
                    no_udp: true,
                    ..script("pw")
                },
                "hysteria2: the server does not allow UDP",
            ),
            (
                Hysteria2Script {
                    no_datagrams: true,
                    ..script("pw")
                },
                "hysteria2: the server does not support QUIC datagrams",
            ),
        ] {
            let fake = FakeHysteria2::spawn(server);
            let out = outbound(&fake, "password=pw");
            assert_eq!(out.udp(), UdpSupport::Native);
            for _ in 0..2 {
                let err = timeout(BOUND, out.open_udp(&ConnectOpts::default()))
                    .await
                    .expect("an outcome within the bound")
                    .err()
                    .expect("refused");
                assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
                assert_eq!(err.to_string(), refusal);
            }
            let mut stream = tunnel(&out, &target(echo)).await;
            round_trip(&mut stream, b"tcp still").await;
            assert_eq!(fake.connections(), 1);
        }
    }

    /// A carrier that does not read loses what its inbox cannot hold, and
    /// holds up no other carrier of the connection.
    #[tokio::test]
    async fn a_full_inbox_drops_rather_than_holds_up_the_other_carriers() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let (slow, other) = (udp_carrier(&out).await, udp_carrier(&out).await);
        udp_round_trip(slow.as_ref(), echo, b"slow").await;
        udp_round_trip(other.as_ref(), echo, b"other").await;
        let sessions: Vec<u32> = fake.udp_messages().iter().map(|m| m.session).collect();
        assert_eq!(sessions, [1, 2]);
        let from = "192.0.2.9:9";
        for i in 0..udp::INBOX as u32 + 10 {
            fake.send_udp(0, &UdpMessage::whole(1, from, &i.to_be_bytes()));
        }
        fake.send_udp(0, &UdpMessage::whole(2, from, b"not held up"));
        assert_eq!(udp_answer(other.as_ref()).await.0, b"not held up");
        for i in 0..udp::INBOX as u32 {
            assert_eq!(udp_answer(slow.as_ref()).await.0, i.to_be_bytes());
        }
        no_udp_answer(slow.as_ref()).await;
    }

    /// Design 5.3: a carrier whose connection ended goes on, under the
    /// same session id, on the connection that replaces it — whether the
    /// server closed the connection, or lost it in a restart (a stateless
    /// reset ends it at the next datagram). A carrier opened later gets an
    /// id of its own.
    #[tokio::test]
    async fn a_carrier_keeps_its_session_id_on_the_next_connection() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"first").await;
        fake.close_connections();
        until_answered(carrier.as_ref(), echo, b"after a close").await;
        fake.restart();
        until_answered(carrier.as_ref(), echo, b"after a restart").await;
        let messages = fake.udp_messages();
        assert!(messages.iter().all(|m| m.session == 1), "{messages:?}");
        let connections: BTreeSet<usize> = messages.iter().map(|m| m.connection).collect();
        assert_eq!(connections, BTreeSet::from([0, 1, 2]));
        let later = udp_carrier(&out).await;
        udp_round_trip(later.as_ref(), echo, b"later").await;
        assert_eq!(fake.udp_messages().last().unwrap().session, 2);
    }

    /// Let go of by the client: closed as its session closes a connection,
    /// with H3_NO_ERROR.
    async fn closed_by_the_client(connection: &quinn::Connection) {
        let closed = timeout(BOUND, connection.closed()).await.expect("closed");
        assert!(
            matches!(&closed, ConnectionError::ApplicationClosed(close)
                if close.error_code == VarInt::from_u32(0x100)),
            "{closed:?}"
        );
    }

    /// Design 5.3: a server going away (an HTTP/3 GOAWAY) takes no new UDP
    /// either: the carrier's next datagram goes on a new connection, under
    /// the session id it had, and the connection before is closed once
    /// nothing holds it.
    #[tokio::test]
    async fn a_carrier_moves_off_a_connection_that_is_going_away() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"before").await;
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        fake.goaway(0);
        let deadline = Instant::now() + BOUND;
        while out.session.serves(&lease) {
            assert!(Instant::now() < deadline, "no GOAWAY heard");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(lease);
        udp_round_trip(carrier.as_ref(), echo, b"after").await;
        let carried: Vec<(usize, u32)> = fake
            .udp_messages()
            .iter()
            .map(|m| (m.connection, m.session))
            .collect();
        assert_eq!(carried, [(0, 1), (1, 1)]);
        closed_by_the_client(&fake.connection(0)).await;
    }

    /// Dials through `inner`, the connection over its UDP flow as over an
    /// `underlying-proxy`'s; where it leads is what `route` holds now, as a
    /// group that is the underlying proxy leads to its member.
    struct Rerouted {
        inner: DirectConnector,
        route: Mutex<Arc<u8>>,
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
            Some(Route::of(&self.route.lock().unwrap()))
        }
    }

    /// Phase 2 M7 design 3.5 and 5.3: the underlying policy leads elsewhere
    /// now (a group's member changed). Each carrier's next datagram goes on
    /// a new connection, dialled the new way, under the session id it had;
    /// the tunnel under way stays on the connection before, which is closed
    /// once nothing holds it. Said once in the log, which names the policy
    /// and nothing secret.
    #[tokio::test]
    async fn carriers_move_off_a_connection_whose_underlying_policy_changed() {
        const PASSWORD: &str = "s3cret-hy2";
        let (log, _logging, _second) = Log::capture();
        let (udp_echo, tcp_echo) = (udp_echo_server().await, echo_server().await);
        let fake = FakeHysteria2::spawn(script(PASSWORD));
        let rerouted = Arc::new(Rerouted {
            inner: DirectConnector::new(Arc::new(SystemResolve)),
            route: Mutex::new(Arc::new(1)),
        });
        let port = fake.addr().port();
        let out = Hysteria2Outbound::new(
            "H",
            Target::new(HostName::parse("127.0.0.1"), port),
            &spec(port, &format!("password={PASSWORD}")),
            Tristate::Auto,
            &[],
            fake.roots(),
            rerouted.clone(),
        )
        .unwrap();
        let (one, two) = (udp_carrier(&out).await, udp_carrier(&out).await);
        udp_round_trip(one.as_ref(), udp_echo, b"one").await;
        udp_round_trip(two.as_ref(), udp_echo, b"two").await;
        let mut under_way = tunnel(&out, &target(tcp_echo)).await;
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        *rerouted.route.lock().unwrap() = Arc::new(2);
        // A carrier asks where the policy leads at most once a second (its
        // own look: `a_carrier_finds_a_changed_route_by_itself`, in udp.rs).
        // Here someone else looks first, as a request would, and the
        // carriers see the connection off the way at once.
        assert!(!out.session.serves(&lease));
        drop(lease);
        udp_round_trip(one.as_ref(), udp_echo, b"one, moved").await;
        udp_round_trip(two.as_ref(), udp_echo, b"two, moved").await;
        round_trip(&mut under_way, b"still there").await;
        let carried: Vec<(usize, u32)> = fake
            .udp_messages()
            .iter()
            .map(|m| (m.connection, m.session))
            .collect();
        assert_eq!(carried, [(0, 1), (0, 2), (1, 1), (1, 2)]);
        assert_eq!(fake.connections(), 2);
        drop(under_way);
        closed_by_the_client(&fake.connection(0)).await;
        let log = log.text();
        let changed = "hysteria2: the underlying policy changed; a new connection is made";
        assert_eq!(log.matches(changed).count(), 1, "{log}");
        assert!(log.contains("policy=H"), "{log}");
        assert!(!log.contains(PASSWORD));
    }

    /// Carriers that open together share one connection and one
    /// authentication, each a session of its own, with a socket of its own
    /// on the server.
    #[tokio::test]
    async fn carriers_that_come_together_share_one_connection_each_its_own_session() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = Arc::new(outbound(&fake, "password=pw"));
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
        let sessions: BTreeSet<u32> = fake.udp_messages().iter().map(|m| m.session).collect();
        assert_eq!(sessions, (1..=16).collect());
        assert_eq!(fake.udp_outside().len(), 16);
    }

    /// A datagram that 255 fragments cannot carry, or whose name leaves no
    /// room in a datagram, is refused, and nothing goes out; the carrier
    /// goes on.
    #[tokio::test]
    async fn a_datagram_too_long_is_refused_and_nothing_goes_out() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        // past 255 of the longest QUIC datagrams a path carries here
        let err = carrier
            .send_to(&vec![0u8; 400_000], &target(echo))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(
            err.to_string()
                .starts_with("hysteria2: a datagram longer than "),
            "{err}"
        );
        let name = Target::new(HostName::Domain("a".repeat(1500)), 53);
        let err = carrier.send_to(b"q", &name).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "hysteria2: the host name is too long for a datagram"
        );
        assert!(fake.udp_messages().is_empty());
        udp_round_trip(carrier.as_ref(), echo, b"still").await;
    }

    /// Design 5.3: a carrier gone is forgotten by its connection; what the
    /// server sends for its session afterwards is dropped.
    #[tokio::test]
    async fn a_carrier_gone_is_forgotten_by_its_connection() {
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        let udp = lease.state().udp.as_ref().expect("the server relays UDP");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, b"here").await;
        assert_eq!(udp.sessions(), [1]);
        drop(carrier);
        assert!(udp.sessions().is_empty());
        fake.send_udp(0, &UdpMessage::whole(1, "192.0.2.1:53", b"too late"));
        let next = udp_carrier(&out).await;
        udp_round_trip(next.as_ref(), echo, b"next").await;
        assert_eq!(udp.sessions(), [2]);
    }

    /// The task that reads a connection's datagrams ends with it.
    #[tokio::test]
    async fn the_reader_of_datagrams_ends_with_its_connection() {
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        let udp = lease.state().udp.as_ref().expect("the server relays UDP");
        assert!(udp.is_reading());
        fake.close_connections();
        let deadline = Instant::now() + BOUND;
        while udp.is_reading() {
            assert!(Instant::now() < deadline, "still reading");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// `opts.timeout` bounds opening a carrier: the dial and the
    /// authentication.
    #[tokio::test]
    async fn opening_a_carrier_is_bounded_by_its_time() {
        // nothing speaks QUIC on this port
        let quiet = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = quiet.local_addr().unwrap().port();
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound_with(&spec(port, "password=pw"), port, &fake);
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

    /// What datagrams carry shows nowhere — in no error and in no log
    /// line, not even at the finest level — whether they go through, are
    /// refused, are no message, find no session, or go out again after a
    /// close.
    #[tokio::test]
    async fn payloads_never_show_in_errors_or_logs() {
        const MARK: &[u8] = b"payload-7c1e9a";
        let (log, _logging, _second) = Log::capture();
        let echo = udp_echo_server().await;
        let fake = FakeHysteria2::spawn(script("pw"));
        let out = outbound(&fake, "password=pw");
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), echo, MARK).await;
        let err = carrier
            .send_to(&MARK.repeat(30_000), &target(echo))
            .await
            .unwrap_err();
        // no message (an address of no length), then one for no session:
        // read before the echo that follows them
        let mut junk = vec![0, 0, 0, 1, 0, 0, 0, 1, 0];
        junk.extend_from_slice(MARK);
        fake.connection(0).send_datagram(junk.into()).unwrap();
        fake.send_udp(0, &UdpMessage::whole(9, "192.0.2.1:53", MARK));
        udp_round_trip(carrier.as_ref(), echo, MARK).await;
        fake.close_connections();
        until_answered(carrier.as_ref(), echo, MARK).await;
        let text = format!("{err} {err:?}");
        assert!(!text.contains("payload-7c1e9a"), "{text}");
        let log = log.text();
        assert!(
            log.contains("hysteria2: a datagram that is no UDP message is dropped"),
            "{log}"
        );
        assert!(
            log.contains("UDP session 1 moves to a new connection"),
            "{log}"
        );
        assert!(!log.contains("payload-7c1e9a"));
    }

    fn obfuscated(obfs: Hysteria2Obfs) -> Hysteria2Script {
        Hysteria2Script {
            obfs: Some(obfs),
            ..script("pw")
        }
    }

    /// Design 5.4: TCP and UDP through Salamander, and through Gecko, whose
    /// handshake goes in 2 to 8 fragments of 512 to 1200 bytes. A server
    /// that restarts says so through it as well: its stateless reset ends
    /// the tunnel under way, and the next one comes up on a new connection.
    #[tokio::test]
    async fn tcp_and_udp_go_through_either_obfuscation() {
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        for (param, obfs) in [
            (
                "salamander-password=obfs",
                Hysteria2Obfs::Salamander("obfs".into()),
            ),
            ("gecko-password=obfs", Hysteria2Obfs::Gecko("obfs".into())),
        ] {
            let gecko = matches!(obfs, Hysteria2Obfs::Gecko(_));
            let fake = FakeHysteria2::spawn(obfuscated(obfs));
            let out = outbound(&fake, &format!("password=pw, {param}"));
            let mut before = tunnel(&out, &target(tcp_echo)).await;
            let payload: Vec<u8> = (0..1 << 20).map(|i| (i % 251) as u8).collect();
            round_trip(&mut before, &payload).await;
            let carrier = udp_carrier(&out).await;
            udp_round_trip(carrier.as_ref(), udp_echo, b"short").await;
            // in Hysteria's own fragments too, both ways
            udp_round_trip(carrier.as_ref(), udp_echo, &[7; 3000]).await;
            // with Salamander alone the fake reads no fragments and keeps no
            // record of any: that the handshake goes whole is the layer's
            // own test (`what_goes_out_is_sealed_and_the_handshake_with_gecko_in_fragments`)
            if gecko {
                let fragments = fake.gecko_fragments();
                assert!(!fragments.is_empty());
                for fragment in &fragments {
                    assert!((2..=8).contains(&fragment.count), "{fragment:?}");
                    assert!((512..=1200).contains(&fragment.datagram), "{fragment:?}");
                }
            }

            fake.restart();
            // room for a keep-alive (10 seconds) to bring the reset, should
            // the lost endpoint have acknowledged the request
            let opts = ConnectOpts {
                timeout: Duration::from_secs(25),
            };
            let mut after = timeout(
                Duration::from_secs(30),
                out.connect_tcp(&target(tcp_echo), &opts),
            )
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel");
            round_trip(&mut after, b"after a restart").await;
            let mut rest = Vec::new();
            let err = timeout(BOUND, before.read_to_end(&mut rest))
                .await
                .expect("the end of the tunnel")
                .unwrap_err();
            assert_eq!(
                err.to_string(),
                "quic: the server no longer knows the connection (stateless reset)"
            );
            assert_eq!(fake.connections(), 2);
        }
    }

    /// A client and a server that do not obfuscate alike never bring a
    /// connection up: neither reads what the other sends. The request fails
    /// by its time, not later.
    #[tokio::test]
    async fn obfuscation_that_does_not_match_fails_by_the_time_of_the_request() {
        let echo = echo_server().await;
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        let salamander = |password: &str| Some(Hysteria2Obfs::Salamander(password.into()));
        let gecko = |password: &str| Some(Hysteria2Obfs::Gecko(password.into()));
        let mut cases = JoinSet::new();
        for (server, client) in [
            (salamander("right"), ", salamander-password=wrong"),
            (gecko("right"), ", gecko-password=wrong"),
            (salamander("right"), ", gecko-password=right"),
            (gecko("right"), ""),
            (None, ", salamander-password=right"),
        ] {
            let opts = opts.clone();
            cases.spawn(async move {
                let fake = FakeHysteria2::spawn(Hysteria2Script {
                    obfs: server,
                    ..script("pw")
                });
                let out = outbound(&fake, &format!("password=pw{client}"));
                let started = Instant::now();
                let err = timeout(BOUND, out.connect_tcp(&target(echo), &opts))
                    .await
                    .expect("an outcome within the bound")
                    .err()
                    .unwrap();
                (client, err, started.elapsed(), fake.connections())
            });
        }
        while let Some(case) = cases.join_next().await {
            let (client, err, took, connections) = case.unwrap();
            assert!(matches!(err, OutboundError::Timeout), "{client}: {err}");
            assert!(took < Duration::from_secs(5), "{client}: {took:?}");
            assert_eq!(connections, 0, "{client}");
        }
    }

    /// Design 3.3 and 5.4: the obfuscation is the outermost layer; over
    /// port hopping, its datagrams go from each hop's socket in turn.
    #[tokio::test]
    async fn obfuscated_tunnels_keep_working_while_the_port_hops() {
        let echo = echo_server().await;
        let ports = MultiPort::bind(3);
        let fake = FakeHysteria2::spawn_on(
            obfuscated(Hysteria2Obfs::Gecko("obfs".into())),
            ports.clone(),
        );
        let mut spec = spec(9, "password=pw, gecko-password=obfs");
        spec.quic.port_hopping = Some(PortHopping {
            ports: ports.ports().iter().map(|&p| p..=p).collect(),
            interval: Duration::from_millis(100),
        });
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
        assert!(!fake.gecko_fragments().is_empty());
    }

    /// Design 5.4: over an underlying proxy's carrier as well.
    #[tokio::test]
    async fn obfuscation_goes_over_an_underlying_proxys_carrier() {
        let (tcp_echo, udp_echo) = (echo_server().await, udp_echo_server().await);
        let fake = FakeHysteria2::spawn(obfuscated(Hysteria2Obfs::Salamander("obfs".into())));
        let carried = Arc::new(Rerouted {
            inner: DirectConnector::new(Arc::new(SystemResolve)),
            route: Mutex::new(Arc::new(1)),
        });
        let port = fake.addr().port();
        let out = Hysteria2Outbound::new(
            "H",
            Target::new(HostName::parse("127.0.0.1"), port),
            &spec(port, "password=pw, salamander-password=obfs"),
            Tristate::Auto,
            &[],
            fake.roots(),
            carried,
        )
        .unwrap();
        let mut stream = tunnel(&out, &target(tcp_echo)).await;
        round_trip(&mut stream, b"carried, obfuscated").await;
        let carrier = udp_carrier(&out).await;
        udp_round_trip(carrier.as_ref(), udp_echo, b"udp too").await;
    }

    /// The obfuscation password keys the packets and goes nowhere else:
    /// not into an error, nor into a log line at any level.
    #[tokio::test]
    async fn the_obfuscation_password_never_shows() {
        const OBFS: &str = "s3cret-obfs";
        let (log, _logging, _second) = Log::capture();
        let echo = echo_server().await;
        let fake = FakeHysteria2::spawn(obfuscated(Hysteria2Obfs::Gecko(OBFS.into())));
        let out = outbound(&fake, &format!("password=pw, gecko-password={OBFS}"));
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"obfuscated").await;
        let other = outbound(&fake, &format!("password=pw, gecko-password={OBFS}-not"));
        let opts = ConnectOpts {
            timeout: Duration::from_millis(300),
        };
        let err = other.connect_tcp(&target(echo), &opts).await.err().unwrap();
        let shown = format!("{err} {err:?}");
        assert!(!shown.contains(OBFS), "{shown}");
        let log = log.text();
        assert!(!log.is_empty());
        assert!(!log.contains(OBFS));
    }

    /// A TCP echo that keeps up with a bulk transfer: `echo_server` echoes a
    /// kilobyte at a time, and a throughput measured through it would be
    /// its own.
    async fn bulk_echo() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut read = tokio::io::BufReader::with_capacity(256 << 10, read);
                    let _ = tokio::io::copy_buf(&mut read, &mut write).await;
                });
            }
        });
        addr
    }

    /// Not a gate (design 5.5): how fast a bulk transfer goes through a
    /// tunnel to a loopback echo behind `FakeHysteria2`, both ways at once,
    /// plain and with Salamander — the cost of the UDP layers under quinn
    /// (design 10). In release: `cargo test -p rurge-proto-quic --release
    /// throughput -- --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn throughput() {
        const TOTAL: usize = 64 * 1024 * 1024;
        let echo = bulk_echo().await;
        for (what, obfs, params) in [
            ("plain", None, "password=pw"),
            (
                "salamander",
                Some(Hysteria2Obfs::Salamander("obfs".into())),
                "password=pw, salamander-password=obfs",
            ),
        ] {
            let fake = FakeHysteria2::spawn(Hysteria2Script {
                obfs,
                ..script("pw")
            });
            let out = outbound(&fake, params);
            let (mut read, mut write) = tokio::io::split(tunnel(&out, &target(echo)).await);
            let started = std::time::Instant::now();
            let writer = tokio::spawn(async move {
                let chunk = vec![0x5a; 64 * 1024];
                for _ in 0..TOTAL / chunk.len() {
                    write.write_all(&chunk).await.unwrap();
                }
                write
            });
            let mut buf = vec![0u8; 64 * 1024];
            let mut received = 0;
            while received < TOTAL {
                let n = read.read(&mut buf).await.unwrap();
                assert!(n > 0, "the echo ended early");
                received += n;
            }
            let elapsed = started.elapsed();
            let _write = writer.await.unwrap();
            println!(
                "{what}: {} MiB through hysteria2 and back in {elapsed:.2?}: {:.1} MiB/s each way",
                TOTAL >> 20,
                (TOTAL >> 20) as f64 / elapsed.as_secs_f64()
            );
        }
    }
}
