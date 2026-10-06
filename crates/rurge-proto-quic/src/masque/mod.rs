//! `masque` outbound (manual: Policies › MASQUE; phase 2 M7 design 7.1):
//! the policy's QUIC connection, which its requests share (`session`), and
//! HTTP/3 on it, HTTP Datagrams announced: RFC 9297 recommends announcing
//! them always (2.1.1), and CONNECT-UDP rides them.
//!
//! - A TCP connection is a classic HTTP/3 CONNECT of its own (`tcp`), as
//!   Surge relays TCP ("HTTP/3 CONNECT tunnels multiplexed over a single
//!   QUIC connection"): the template-driven `connect-tcp` is still a draft,
//!   and masque-server answers it 404.
//! - With `username` and `password`, every request carries them as
//!   `proxy-authorization: Basic …`, which QPACK never indexes. The manual
//!   names no field: this is the one a proxy reads them from (RFC 9110
//!   11.7.2), and masque-server's. Without them no authorization goes.
//!   Nothing is authenticated per connection.

mod tcp;

use crate::session::{Dialer, Protocol, Session};
use http::header::HeaderValue;
use quinn::{Connection, VarInt};
use rurge_config::KeystoreItem;
use rurge_config::spec::{MasqueSpec, Tristate};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_net::quic::h3;
use rurge_proto::http::basic_authorization;
use rurge_proto::{BuildError, Outbound, OutboundError};
use rustls::RootCertStore;
use std::sync::Arc;

/// H3_NO_ERROR (RFC 9114 8.1): a connection let go of.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);

/// No `Debug`: it holds the credentials.
pub struct MasqueOutbound {
    name: String,
    session: Session<Http3>,
    /// `proxy-authorization`, marked sensitive: QPACK never indexes it.
    authorization: Option<HeaderValue>,
}

impl MasqueOutbound {
    /// The connection goes as `connector` dials (design 3.3), with the
    /// policy's `ecn`; `client-cert` comes from `keystore`.
    pub fn new(
        name: &str,
        server: Target,
        spec: &MasqueSpec,
        ecn: Tristate,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<MasqueOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it
        let dialer = Dialer::new(
            connector, server, &spec.tls, &spec.quic, ecn, keystore, roots,
        )?;
        let authorization = spec.credentials.as_ref().map(|credentials| {
            let basic =
                basic_authorization(credentials.username.expose(), credentials.password.expose());
            let mut value = HeaderValue::try_from(basic).expect("Base64 makes a field value");
            value.set_sensitive(true);
            value
        });
        Ok(MasqueOutbound {
            name: name.to_string(),
            session: Session::new(name, dialer, Http3),
            authorization,
        })
    }
}

/// What every new connection runs before its requests: HTTP/3, nothing
/// more — each request carries its own credentials.
struct Http3;

/// What a connection keeps for its requests.
struct Connected {
    /// Lives as long as the connection: dropped, it would end our HTTP/3
    /// control stream, which a server may take for the end of HTTP/3 on the
    /// connection.
    h3: h3::Client,
}

impl Protocol for Http3 {
    type State = Connected;

    const NAME: &'static str = "masque";

    const NO_ERROR: VarInt = H3_NO_ERROR;

    /// Our SETTINGS go out, and requests may follow at once: a classic
    /// CONNECT waits for nothing of the server's.
    fn handshake<'a>(
        &'a self,
        connection: &'a Connection,
    ) -> BoxFuture<'a, Result<Connected, OutboundError>> {
        Box::pin(async move {
            Ok(Connected {
                h3: h3::Client::with_datagrams(connection.clone()),
            })
        })
    }

    /// The server sent GOAWAY: it is going down.
    fn is_going_away(&self, state: &Connected) -> bool {
        state.h3.is_going_away()
    }
}

impl Outbound for MasqueOutbound {
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
            let request = tcp::request(target, self.authorization.as_ref())?;
            // one budget for the connection (when one is dialed), QUIC's
            // handshake and the proxy's response
            let tunnel = self
                .session
                .request(opts, |lease| Box::pin(tcp::open(lease, request.clone())));
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
    use crate::testing::{FakeH3Proxy, H3ProxyScript, RecordedH3Request};
    use quinn::ConnectionError;
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::masque::read_masque;
    use rurge_config::spec::{CommonOpts, ParamReader, PortHopping};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::quic::testing::MultiPort;
    use rurge_proto::testing::echo_server;
    use std::io;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::{Instant, timeout};

    const BOUND: Duration = Duration::from_secs(10);

    /// The user `u` with the password `p`.
    fn users() -> Vec<(String, String)> {
        vec![("u".to_string(), "p".to_string())]
    }

    /// The spec of `masque, 127.0.0.1, <port>[, <params>]`, as the
    /// configuration reads it.
    fn spec(port: u16, params: &str) -> MasqueSpec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let mut line = format!("masque, 127.0.0.1, {port}");
        if !params.is_empty() {
            line = format!("{line}, {params}");
        }
        let policy = parse_policy("M", &line, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_masque(&mut r, &mut CommonOpts::default(), &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        spec
    }

    /// `spec`'s outbound to `127.0.0.1:<port>`, trusting `fake`.
    fn outbound_with(spec: &MasqueSpec, port: u16, fake: &FakeH3Proxy) -> MasqueOutbound {
        MasqueOutbound::new(
            "M",
            Target::new(HostName::parse("127.0.0.1"), port),
            spec,
            Tristate::Auto,
            &[],
            fake.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// `masque, 127.0.0.1, <the fake's port>[, <params>]`.
    fn outbound(fake: &FakeH3Proxy, params: &str) -> MasqueOutbound {
        let port = fake.addr().port();
        outbound_with(&spec(port, params), port, fake)
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    /// A classic CONNECT to `authority`, as the fake records it.
    fn connect(connection: usize, authority: &str) -> RecordedH3Request {
        RecordedH3Request {
            connection,
            method: "CONNECT".to_string(),
            protocol: None,
            scheme: None,
            authority: Some(authority.to_string()),
            path: None,
            headers: Vec::new(),
        }
    }

    async fn tunnel(out: &MasqueOutbound, to: &Target) -> BoxedStream {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel")
    }

    async fn failure(out: &MasqueOutbound, to: &Target) -> OutboundError {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .err()
            .expect("no tunnel")
    }

    /// Flushed: a write may leave the rest of its DATA frame for the next.
    async fn round_trip(stream: &mut BoxedStream, payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        stream.flush().await.unwrap();
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

    /// Let go of by the client: closed as its session closes a connection,
    /// with H3_NO_ERROR.
    async fn closed_by_the_client(connection: &Connection) {
        let closed = timeout(BOUND, connection.closed()).await.expect("closed");
        assert!(
            matches!(&closed, ConnectionError::ApplicationClosed(close)
                if close.error_code == H3_NO_ERROR),
            "{closed:?}"
        );
    }

    /// Design 7.1: a TCP connection is a classic CONNECT — `:method` and
    /// `:authority` alone — whose 2xx makes the tunnel.
    #[tokio::test]
    async fn a_tunnel_is_a_classic_connect() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        assert_eq!(out.name(), "M");
        assert!(out.http_forward().is_none());
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"through masque").await;
        assert_eq!(fake.requests(), [connect(0, &echo.to_string())]);
    }

    /// Past both sides' stream windows (8 MiB ours, quinn's 1.25 MB the
    /// fake's); the shutdown reaches the target as a FIN, and the target's
    /// close comes back as the end of the stream.
    #[tokio::test]
    async fn a_tunnel_carries_a_large_payload_and_a_half_close() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
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

    /// Each side's end reaches the other on its own: a target that ends
    /// its side first is read to its end while the tunnel still sends, and
    /// the tunnel's shutdown then reaches the target.
    #[tokio::test]
    async fn the_target_may_end_its_side_first() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let target_side = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut read, mut write) = stream.into_split();
            write.write_all(b"hello, and goodbye").await.unwrap();
            write.shutdown().await.unwrap();
            let mut heard = Vec::new();
            read.read_to_end(&mut heard).await.unwrap();
            heard
        });
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let mut stream = tunnel(&out, &target(addr)).await;
        let mut greeting = Vec::new();
        timeout(BOUND, stream.read_to_end(&mut greeting))
            .await
            .expect("the target's end")
            .unwrap();
        assert_eq!(greeting, b"hello, and goodbye");
        stream.write_all(b"still here").await.unwrap();
        stream.shutdown().await.unwrap();
        let heard = timeout(BOUND, target_side).await.expect("our end").unwrap();
        assert_eq!(heard, b"still here");
    }

    /// Design 7.1: the authority names the target as `host:port` — an IPv6
    /// literal in brackets, a name as its A-labels. A target that cannot
    /// be named is refused before anything is dialed.
    #[tokio::test]
    async fn targets_go_out_as_host_and_port_and_an_unsendable_name_never_dials() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            connect_to: Some(echo),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        for (host, authority) in [
            (HostName::parse("192.0.2.1"), "192.0.2.1:443"),
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:443"),
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:443",
            ),
            (HostName::parse("remote.example"), "remote.example:443"),
        ] {
            let mut stream = tunnel(&out, &Target::new(host, 443)).await;
            round_trip(&mut stream, b"x").await;
            assert_eq!(fake.requests().pop(), Some(connect(0, authority)));
        }
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        // longer than an authority may be
        let long = format!("{}.test", "a".repeat(70_000));
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
                "masque: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.connections(), 0);
    }

    /// With `username` and `password`, every CONNECT carries them as Basic
    /// `proxy-authorization` — not only after a 407 —; a policy without
    /// them sends none, which a server that wants them refuses.
    #[tokio::test]
    async fn every_connect_carries_the_credentials_and_none_goes_without_them() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: users(),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "username=u, password=p");
        for payload in [b"one", b"two"] {
            let mut stream = tunnel(&out, &target(echo)).await;
            round_trip(&mut stream, payload).await;
        }
        let requests = fake.requests();
        assert_eq!(requests.len(), 2);
        for request in &requests {
            // base64("u:p"), and nothing else
            assert_eq!(
                request.headers,
                [("proxy-authorization".to_string(), "Basic dTpw".to_string())]
            );
        }
        let without = outbound(&fake, "");
        let err = failure(&without, &target(echo)).await;
        assert_eq!(err.to_string(), "masque: proxy authentication required");
        assert!(fake.requests()[2].headers.is_empty());

        // a server that wants none lets it through
        let open = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&open, "");
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"open").await;
        assert!(open.requests()[0].headers.is_empty());
    }

    /// Design 7.1: 407 is a refused login; any other final status is the
    /// proxy's refusal, quoted by its number — masque-server answers 502
    /// for a target it cannot reach. A refusal costs its connection
    /// nothing.
    #[tokio::test]
    async fn a_refusal_is_quoted_by_its_status() {
        // a port nothing listens on
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: users(),
            ..H3ProxyScript::default()
        });
        let wrong = outbound(&fake, "username=u, password=wrong");
        let right = outbound(&fake, "username=u, password=p");
        for (out, refusal) in [
            (&wrong, "masque: proxy authentication required"),
            (&right, "masque: the proxy answered 502"),
        ] {
            for _ in 0..2 {
                let err = failure(out, &target(closed)).await;
                assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
                assert_eq!(err.to_string(), refusal);
            }
        }
        assert_eq!(fake.connections(), 2, "one for each outbound");
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            refuse: Some(403),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        let err = failure(&out, &target(closed)).await;
        assert_eq!(err.to_string(), "masque: the proxy answered 403");
    }

    /// Design 3.5: one dial for the tunnels that come together, and the
    /// connection carries those after them too.
    #[tokio::test]
    async fn tunnels_that_come_together_share_one_connection() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let to = target(echo);
        let (a, b, c) = tokio::join!(tunnel(&out, &to), tunnel(&out, &to), tunnel(&out, &to));
        for (mut stream, payload) in [(a, b"one"), (b, b"two"), (c, b"six")] {
            round_trip(&mut stream, payload).await;
        }
        let mut later = tunnel(&out, &to).await;
        round_trip(&mut later, b"later").await;
        assert_eq!(fake.connections(), 1);
        assert_eq!(fake.requests(), vec![connect(0, &echo.to_string()); 4]);
    }

    /// Design 3.5: a server going away (an HTTP/3 GOAWAY) takes no new
    /// tunnel: the next goes on a new connection, while one under way on
    /// the old runs to its end; the client closes the old connection once
    /// nothing holds it.
    #[tokio::test]
    async fn after_a_goaway_new_tunnels_go_on_a_new_connection_and_old_ones_finish() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            goaway_after: Some(1),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        // held from before the first request, to see its GOAWAY come
        let lease = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap();
        let mut old = tunnel(&out, &target(echo)).await;
        round_trip(&mut old, b"before").await;
        let deadline = Instant::now() + BOUND;
        while out.session.serves(&lease) {
            assert!(Instant::now() < deadline, "no GOAWAY heard");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(lease);
        let mut new = tunnel(&out, &target(echo)).await;
        round_trip(&mut new, b"after").await;
        round_trip(&mut old, b"still there").await;
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 1]);
        old.shutdown().await.unwrap();
        let mut rest = Vec::new();
        timeout(BOUND, old.read_to_end(&mut rest))
            .await
            .expect("the end of the tunnel")
            .unwrap();
        assert!(rest.is_empty());
        drop(old);
        closed_by_the_client(&fake.connection(0)).await;
    }

    /// A tunnel under way ends with its connection; the next one dials
    /// again.
    #[tokio::test]
    async fn after_the_server_closes_the_connection_the_next_tunnel_dials_again() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
        let mut first = tunnel(&out, &target(echo)).await;
        round_trip(&mut first, b"first").await;
        fake.close_connections();
        assert_eq!(
            end_of(&mut first).await.to_string(),
            "quic: the server closed the connection (code 0x100)"
        );
        let mut second = tunnel(&out, &target(echo)).await;
        round_trip(&mut second, b"second").await;
        assert_eq!(fake.connections(), 2);
    }

    /// Design 3.5: the server restarts — losing its connections — as a
    /// request comes. The request finds its connection gone (a stateless
    /// reset answers the client's next packet) before the response came,
    /// and goes again, once, on a new connection. A restart between two
    /// requests is no harder.
    #[tokio::test]
    async fn a_request_that_finds_the_server_restarted_goes_again_on_a_new_connection() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript::default());
        let out = outbound(&fake, "");
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
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 0, 1]);
        assert_eq!(
            end_of(&mut before).await.to_string(),
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
        assert_eq!(fake.requests().last().unwrap().connection, 2);
    }

    /// `opts.timeout` bounds the whole request: the dial, QUIC's handshake
    /// and the proxy's response.
    #[tokio::test]
    async fn a_server_that_never_answers_times_out() {
        let echo = echo_server().await;
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        // nothing speaks QUIC on this port
        let quiet = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = quiet.local_addr().unwrap().port();
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            silent: true,
            ..H3ProxyScript::default()
        });
        let out = outbound_with(&spec(port, ""), port, &fake);
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
        // a server that takes the request and never answers it, on a
        // connection that is up already
        let out = outbound(&fake, "");
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
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(fake.requests().len(), 1, "the request went out");
    }

    /// One QUIC connection carries every tunnel: past the server's limit on
    /// the streams open at once, a tunnel waits for one to end — within its
    /// time — rather than dialing another connection.
    #[tokio::test]
    async fn a_tunnel_past_the_servers_stream_limit_waits_for_one_to_end() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            max_streams: Some(1),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "");
        let mut first = tunnel(&out, &target(echo)).await;
        round_trip(&mut first, b"first").await;
        let opts = ConnectOpts {
            timeout: Duration::from_millis(300),
        };
        let err = timeout(BOUND, out.connect_tcp(&target(echo), &opts))
            .await
            .expect("an outcome within the bound")
            .err()
            .unwrap();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        // the first ends both ways: its stream is the server's to reuse
        first.shutdown().await.unwrap();
        let mut rest = Vec::new();
        timeout(BOUND, first.read_to_end(&mut rest))
            .await
            .expect("the end of the tunnel")
            .unwrap();
        drop(first);
        let mut second = tunnel(&out, &target(echo)).await;
        round_trip(&mut second, b"second").await;
        assert_eq!(fake.connections(), 1);
        // the one that waited never went out
        assert_eq!(fake.requests().len(), 2);
    }

    /// quiche greases (masque-server, the TrustTunnel endpoint): a setting
    /// and a unidirectional stream of reserved kinds, and two frames of
    /// reserved types before every response head, a refusal's too. None of
    /// it gets in the way.
    #[tokio::test]
    async fn the_servers_grease_is_passed_over() {
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: users(),
            grease: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, "username=u, password=p");
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"through the grease").await;
        let wrong = outbound(&fake, "username=u, password=wrong");
        let err = failure(&wrong, &target(echo)).await;
        assert_eq!(err.to_string(), "masque: proxy authentication required");
        // the connection goes on
        let mut again = tunnel(&out, &target(echo)).await;
        round_trip(&mut again, b"again").await;
        round_trip(&mut stream, b"and on").await;
        assert_eq!(fake.connections(), 2, "one for each outbound");
    }

    /// Design 3.3: the port changes under the connection — a new local
    /// socket each time — and its tunnels, old and new, go on. The interval
    /// is set past the configuration's minimum of 5 seconds.
    #[tokio::test]
    async fn tunnels_keep_working_while_the_port_hops() {
        let echo = echo_server().await;
        let ports = MultiPort::bind(3);
        let fake = FakeH3Proxy::spawn_on(H3ProxyScript::default(), ports.clone());
        let mut spec = spec(9, "");
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

    /// The credentials go to the server and nowhere else: not into the
    /// error of a refused login, nor into a log line at any level.
    #[tokio::test]
    async fn the_credentials_never_show() {
        const PASSWORD: &str = "s3cret-masque";
        let (log, _logging, _second) = Log::capture();
        let echo = echo_server().await;
        let fake = FakeH3Proxy::spawn(H3ProxyScript {
            users: vec![("u".to_string(), PASSWORD.to_string())],
            ..H3ProxyScript::default()
        });
        let wrong = outbound(&fake, &format!("username=u, password={PASSWORD}-not"));
        let err = failure(&wrong, &target(echo)).await;
        assert_eq!(err.to_string(), "masque: proxy authentication required");
        let out = outbound(&fake, &format!("username=u, password={PASSWORD}"));
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"let in").await;
        let basic = basic_authorization("u", PASSWORD);
        assert_eq!(
            fake.requests()[1].header("proxy-authorization"),
            Some(basic.as_str())
        );
        let shown = format!("{err} {err:?}");
        let log = log.text();
        assert!(!log.is_empty());
        let wrong_basic = basic_authorization("u", &format!("{PASSWORD}-not"));
        for secret in [
            PASSWORD,
            &basic["Basic ".len()..],
            &wrong_basic["Basic ".len()..],
        ] {
            assert!(!shown.contains(secret), "{shown}");
            assert!(!log.contains(secret));
        }
    }
}
