//! `hysteria2` outbound (manual: Policies › Hysteria 2; phase 2 M7 design
//! 5): the policy's QUIC connection, which its requests share (`session`),
//! authenticated over HTTP/3 as it comes up (`auth`).
//!
//! - A TCP connection is a bidirectional stream of its own: the request
//!   names the target as `host:port` (an IPv6 literal in brackets, an IDN
//!   as its A-labels), and the server's answer lets the stream through or
//!   refuses it with a message (`tcp`).
//! - Requests and the authentication are padded as the reference client
//!   pads them: letters and digits, of a random length.

mod auth;
mod tcp;

use crate::session::{Dialer, Session};
use auth::Auth;
use rurge_config::KeystoreItem;
use rurge_config::spec::{Hysteria2Spec, Tristate};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::{BuildError, Outbound, OutboundError};
use rustls::RootCertStore;
use std::ops::RangeInclusive;
use std::sync::Arc;

/// The characters of padding: letters and digits, as the reference's
/// (`core/internal/protocol/padding.go`).
const PADDING: &[u8; 62] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

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
    session: Session<Auth>,
}

impl Hysteria2Outbound {
    /// The connection goes as `connector` dials (design 3.3), with the
    /// policy's `ecn`; `client-cert` comes from `keystore`.
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
        let dialer = Dialer::new(
            connector, server, &spec.tls, &spec.quic, ecn, keystore, roots,
        )?;
        Ok(Hysteria2Outbound {
            name: name.to_string(),
            session: Session::new(name, dialer, auth),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeHysteria2, Hysteria2Script};
    use quinn::{ConnectionError, VarInt};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::hysteria2::read_hysteria2;
    use rurge_config::spec::{CommonOpts, ParamReader, PortHopping};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::quic::testing::MultiPort;
    use rurge_proto::testing::echo_server;
    use std::collections::HashSet;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
        assert!(lease.state().udp);

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
        assert!(!lease.state().udp);
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
        let err = out.connect_tcp(&target(echo), &opts).await.err().unwrap();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        // well before QUIC's own handshake timeout, about 3 seconds
        assert!(
            started.elapsed() < Duration::from_secs(2),
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
        let err = out.connect_tcp(&target(echo), &opts).await.err().unwrap();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
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
}
