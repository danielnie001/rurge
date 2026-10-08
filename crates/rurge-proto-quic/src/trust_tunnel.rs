//! `trust-tunnel` outbound over HTTP/3 (manual: Policies › Trust Tunnel,
//! `h3`; TrustTunnel protocol 3.2, 5.1; phase 2 M7 design 7.2): every TCP
//! connection a classic HTTP/3 CONNECT of its own (`connect`) on one of the
//! policy's QUIC connections, each of which carries `max-streams` of them
//! at most (`Session::with_stream_cap`): more at once open another. A
//! connection receives into a window with room for all of its tunnels:
//! those nobody reads leave the others theirs, as over HTTP/2.
//!
//! - The request and its refusals are the HTTP/2 transport's
//!   (`rurge_proto::trust_tunnel`): `:method CONNECT` and `:authority
//!   host:port` alone — the endpoint answers 400 to a `:scheme` or a
//!   `:path` —, `proxy-authorization: Basic …` and `user-agent`, then the
//!   configured `headers`; 407 is a refused login, any other final status
//!   is quoted by its number.
//! - ALPN `h3`, and HTTP/3 without HTTP Datagrams: the endpoint takes no
//!   QUIC datagrams, and a classic CONNECT needs none. No port hopping.
//! - Past the server's own limit on the streams of a connection (QUIC's,
//!   which quinn keeps without telling it), a tunnel waits for a stream
//!   within its time, as on MASQUE: the session counts `max-streams`
//!   alone, where the HTTP/2 pool keeps the smaller of the two.
//! - TCP only, as over HTTP/2, and no fallback to HTTP/2: with `h3=true`
//!   the tunnel runs "over HTTP/3 (QUIC) instead of HTTP/2 over TLS"
//!   (manual).

use crate::connect;
use crate::session::{Dialer, Session};
use rurge_config::KeystoreItem;
use rurge_config::spec::{HeaderTemplate, QuicOpts, Tristate, TrustTunnelSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::http::basic_authorization;
use rurge_proto::trust_tunnel::{connect_request, refused};
use rurge_proto::{BuildError, Outbound, OutboundError};
use rustls::RootCertStore;
use std::sync::Arc;

/// No `Debug`: it holds the credentials.
pub struct TrustTunnelH3Outbound {
    name: String,
    session: Session<Http3>,
    /// The `proxy-authorization` value, ready to send.
    authorization: String,
    headers: Vec<HeaderTemplate>,
}

impl TrustTunnelH3Outbound {
    /// The connections go as `connector` dials (design 3.3), with the
    /// policy's `ecn`; `client-cert` comes from `keystore`. `spec` is that
    /// of a line with `h3=true`, its `alpn` pinned to `h3`.
    pub fn new(
        name: &str,
        server: Target,
        spec: &TrustTunnelSpec,
        ecn: Tristate,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<TrustTunnelH3Outbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it. A spec made by hand may carry
        // headers `rurge-config` never let through: never echo them.
        if let Some(n) = spec.headers.iter().position(|t| !t.is_valid()) {
            return Err(BuildError::new(format!(
                "custom header #{} is not valid",
                n + 1
            )));
        }
        // the SNI is the server's name or `sni=`: the endpoint picks its
        // host by the exact SNI
        let dialer = Dialer::new(
            connector,
            server,
            &spec.tls,
            &QuicOpts::default(),
            ecn,
            keystore,
            roots,
        )?;
        let max_streams = spec.max_streams as usize;
        Ok(TrustTunnelH3Outbound {
            name: name.to_string(),
            session: Session::new(name, dialer, Http3::new()).with_stream_cap(max_streams),
            authorization: basic_authorization(spec.username.expose(), spec.password.expose()),
            headers: spec.headers.clone(),
        })
    }
}

/// `trust-tunnel` as a CONNECT proxy over HTTP/3: without HTTP Datagrams.
struct TrustTunnel;

impl connect::Proxy for TrustTunnel {
    const NAME: &'static str = "trust-tunnel";

    const DATAGRAMS: bool = false;
}

/// What every new connection runs before its tunnels (`connect::Http3`).
type Http3 = connect::Http3<TrustTunnel>;

impl Outbound for TrustTunnelH3Outbound {
    fn name(&self) -> &str {
        &self.name
    }

    // `udp()` stays `Unsupported`, as over HTTP/2: the engine applies
    // `udp-policy-not-supported-behaviour`

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // never dial for a target whose name cannot be sent
            let request = connect_request(target, &self.authorization, &self.headers)?;
            connect::tunnel(&self.session, request, refused, opts).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::H3_NO_ERROR;
    use crate::test_log::Log;
    use crate::testing::{FakeH3Proxy, H3ProxyScript};
    use quinn::ConnectionError;
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::h2::read_trust_tunnel;
    use rurge_config::spec::{CommonOpts, ParamReader};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_proto::UdpSupport;
    use rurge_proto::testing::echo_server;
    use rurge_proto::trust_tunnel::USER_AGENT;
    use std::io;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::{Instant, timeout};

    const BOUND: Duration = Duration::from_secs(10);
    /// The credentials the endpoint knows, unless a test says otherwise.
    const LOGIN: &str = "username=u, password=p";

    /// The fake as the TrustTunnel endpoint v1.1.0 answers over HTTP/3,
    /// `script` aside: quiche's grease, neither extended CONNECT nor HTTP
    /// Datagrams, its challenge on a 407, and the user `u` with the
    /// password `p` unless `script` names users.
    fn endpoint(mut script: H3ProxyScript) -> FakeH3Proxy {
        if script.users.is_empty() {
            script.users = vec![("u".to_string(), "p".to_string())];
        }
        FakeH3Proxy::spawn(H3ProxyScript {
            challenge: Some("Basic realm=Authorization Required".to_string()),
            grease: true,
            no_extended_connect: true,
            no_h3_datagram: true,
            no_datagrams: true,
            ..script
        })
    }

    /// The spec of `trust-tunnel, 127.0.0.1, <port>, h3=true, <params>`,
    /// as the configuration reads it.
    fn spec(port: u16, params: &str) -> TrustTunnelSpec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let line = format!("trust-tunnel, 127.0.0.1, {port}, h3=true, {params}");
        let policy = parse_policy("T", &line, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_trust_tunnel(&mut r, &mut CommonOpts::default(), &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        spec
    }

    /// `trust-tunnel, 127.0.0.1, <the fake's port>, h3=true, <params>`.
    fn outbound(fake: &FakeH3Proxy, params: &str) -> TrustTunnelH3Outbound {
        let port = fake.addr().port();
        TrustTunnelH3Outbound::new(
            "T",
            Target::new(HostName::parse("127.0.0.1"), port),
            &spec(port, params),
            Tristate::Auto,
            &[],
            fake.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn tunnel(out: &TrustTunnelH3Outbound, to: &Target) -> BoxedStream {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel")
    }

    async fn failure(out: &TrustTunnelH3Outbound, to: &Target) -> OutboundError {
        timeout(BOUND, out.connect_tcp(to, &ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .err()
            .expect("no tunnel")
    }

    /// Flushed: a write may leave the rest of its DATA frame for the next.
    async fn round_trip(stream: &mut BoxedStream, payload: &[u8]) {
        timeout(BOUND, async {
            stream.write_all(payload).await?;
            stream.flush().await
        })
        .await
        .expect("written within the bound")
        .unwrap();
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

    /// The tunnels each connection the fake accepted carried, fewest first.
    fn carried(fake: &FakeH3Proxy) -> Vec<usize> {
        let mut carried = vec![0; fake.connections()];
        for request in fake.requests() {
            carried[request.connection] += 1;
        }
        carried.sort_unstable();
        carried
    }

    /// The connection that carried the request for `authority`.
    fn connection_of(fake: &FakeH3Proxy, authority: &str) -> usize {
        let requests = fake.requests();
        let request = requests
            .iter()
            .find(|r| r.authority.as_deref() == Some(authority))
            .expect("a request for the authority");
        request.connection
    }

    /// Design 7.2: a classic HTTP/3 CONNECT — `:method` and `:authority`
    /// alone, as the endpoint wants — with the credentials and our
    /// `user-agent`, as over HTTP/2; its 2xx makes the tunnel. No UDP.
    #[tokio::test]
    async fn a_tunnel_is_a_classic_connect_with_the_credentials_and_a_user_agent() {
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript {
            require_user_agent: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, LOGIN);
        assert_eq!(out.name(), "T");
        assert!(out.http_forward().is_none());
        assert_eq!(out.udp(), UdpSupport::Unsupported);
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"through the tunnel").await;
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.method, "CONNECT");
        assert_eq!(request.authority, Some(echo.to_string()));
        assert_eq!(
            (&request.protocol, &request.scheme, &request.path),
            (&None, &None, &None)
        );
        let mut fields = request.headers.clone();
        fields.sort();
        // base64("u:p")
        assert_eq!(
            fields,
            [
                ("proxy-authorization".to_string(), "Basic dTpw".to_string()),
                ("user-agent".to_string(), USER_AGENT.to_string()),
            ]
        );
    }

    /// `headers` as over HTTP/2: a configured field replaces ours by name,
    /// and its placeholders are drawn anew for every tunnel.
    #[tokio::test]
    async fn headers_replace_the_user_agent_and_are_drawn_anew_for_every_tunnel() {
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript::default());
        let out = outbound(
            &fake,
            &format!("{LOGIN}, headers=User-Agent:Linux client<random-string(12)>;X-Note:n"),
        );
        for _ in 0..2 {
            let mut stream = tunnel(&out, &target(echo)).await;
            round_trip(&mut stream, b"x").await;
        }
        let requests = fake.requests();
        assert_eq!(requests.len(), 2);
        let mut drawn = Vec::new();
        for request in &requests {
            let agents: Vec<&str> = request
                .headers
                .iter()
                .filter(|(name, _)| name == "user-agent")
                .map(|(_, value)| value.as_str())
                .collect();
            assert_eq!(agents.len(), 1, "one user-agent: {agents:?}");
            let random = agents[0].strip_prefix("Linux client").expect(agents[0]);
            assert_eq!(random.len(), 12, "{}", agents[0]);
            assert!(
                random
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "{random}"
            );
            drawn.push(random.to_string());
            assert_eq!(request.header("x-note"), Some("n"));
            assert_eq!(request.header("proxy-authorization"), Some("Basic dTpw"));
        }
        assert_ne!(drawn[0], drawn[1], "drawn anew for every tunnel");
    }

    /// 407 — with the endpoint's challenge — is a refused login, worded as
    /// over HTTP/2; it costs the connection nothing.
    #[tokio::test]
    async fn a_refused_login_says_authentication_failed() {
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript::default());
        let wrong = outbound(&fake, "username=u, password=wrong");
        for _ in 0..2 {
            let err = failure(&wrong, &target(echo)).await;
            assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
            assert_eq!(err.to_string(), "trust-tunnel: authentication failed");
        }
        assert_eq!(fake.connections(), 1);
    }

    /// Any other final status is quoted by its number, as over HTTP/2: the
    /// endpoint answers 502 for a target it cannot reach.
    #[tokio::test]
    async fn an_unreachable_target_and_other_statuses_are_quoted_by_number() {
        // a port nothing listens on
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let fake = endpoint(H3ProxyScript::default());
        let out = outbound(&fake, LOGIN);
        let err = failure(&out, &target(closed)).await;
        assert!(matches!(err, OutboundError::Proxy(_)), "{err}");
        assert_eq!(err.to_string(), "trust-tunnel: the server answered 502");
        let fake = endpoint(H3ProxyScript {
            refuse: Some(403),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, LOGIN);
        let err = failure(&out, &target(closed)).await;
        assert_eq!(err.to_string(), "trust-tunnel: the server answered 403");
    }

    /// A target whose name cannot be sent is refused, as over HTTP/2,
    /// before anything is dialed.
    #[tokio::test]
    async fn a_name_that_cannot_be_sent_never_dials() {
        let fake = endpoint(H3ProxyScript::default());
        let out = outbound(&fake, LOGIN);
        for name in ["x@blocked.test", "a b.test", "a.test\r\nx: 1", ""] {
            let to = Target::new(HostName::Domain(name.into()), 443);
            assert_eq!(
                failure(&out, &to).await.to_string(),
                "trust-tunnel: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.connections(), 0);
    }

    /// Design 7.2: `max-streams` tunnels at most on each QUIC connection;
    /// those that would not fit open another. A tunnel let go of frees its
    /// place, which the next takes before another connection is dialed.
    #[tokio::test]
    async fn max_streams_bounds_the_tunnels_of_each_quic_connection() {
        let echo = echo_server().await;
        // each tunnel's own authority tells its request apart
        let fake = endpoint(H3ProxyScript {
            connect_to: Some(echo),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, &format!("{LOGIN}, max-streams=2"));
        // ports 1 to 8 of an address the fake never dials
        let to: Vec<Target> = (1..=8)
            .map(|port| Target::new(HostName::parse("192.0.2.1"), port))
            .collect();
        let (a, b, c, d, e) = tokio::join!(
            tunnel(&out, &to[0]),
            tunnel(&out, &to[1]),
            tunnel(&out, &to[2]),
            tunnel(&out, &to[3]),
            tunnel(&out, &to[4])
        );
        let mut tunnels = vec![a, b, c, d, e];
        for stream in &mut tunnels {
            round_trip(stream, b"five at once").await;
        }
        assert_eq!(carried(&fake), [1, 2, 2]);
        // the one place left
        tunnels.push(tunnel(&out, &to[5]).await);
        assert_eq!(carried(&fake), [2, 2, 2]);
        // the place of the tunnel to port 1, let go of
        let freed = connection_of(&fake, "192.0.2.1:1");
        drop(tunnels.remove(0));
        tunnels.push(tunnel(&out, &to[6]).await);
        assert_eq!(connection_of(&fake, "192.0.2.1:7"), freed);
        assert_eq!(fake.connections(), 3);
        // every place taken: a fourth connection
        tunnels.push(tunnel(&out, &to[7]).await);
        assert_eq!(fake.connections(), 4);
        for stream in &mut tunnels {
            round_trip(stream, b"all still here").await;
        }
    }

    /// A loopback TCP server that sends without end to whoever connects,
    /// and reads nothing.
    async fn flood_server() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let chunk = [0x5a; 64 * 1024];
                    while stream.write_all(&chunk).await.is_ok() {}
                });
            }
        });
        addr
    }

    /// `count` tunnels of `out` to `flood` that nobody reads, megabytes
    /// later: once the server has more for each of them than its window
    /// takes — or, were the connection's too small for them all, more than
    /// that takes.
    async fn unread_tunnels(
        out: &TrustTunnelH3Outbound,
        flood: SocketAddr,
        count: u64,
    ) -> Vec<BoxedStream> {
        let mut unread = Vec::new();
        for _ in 0..count {
            unread.push(tunnel(out, &target(flood)).await);
        }
        // the connection they are on, without a place on it
        let quic = timeout(BOUND, out.session.lease(&ConnectOpts::default()))
            .await
            .unwrap()
            .unwrap()
            .connection()
            .clone();
        let deadline = Instant::now() + 6 * BOUND;
        loop {
            let said = quic.stats().frame_rx;
            if said.stream_data_blocked >= count || said.data_blocked >= 1 {
                break;
            }
            assert!(Instant::now() < deadline, "the windows never filled");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        unread
    }

    /// A connection's receive window has room for the window of every
    /// tunnel it may carry (`max-streams`): tunnels nobody reads — the
    /// server sends into each all its window takes — leave the others
    /// theirs. In 20 MiB, the least a connection receives into, three of
    /// them would take all of it: no tunnel of the connection would hear
    /// from the server again, not even that it was made.
    #[tokio::test]
    async fn tunnels_nobody_reads_leave_the_others_their_window() {
        let (flood, echo) = (flood_server().await, echo_server().await);
        let fake = endpoint(H3ProxyScript::default());
        let out = outbound(&fake, &format!("{LOGIN}, max-streams=4"));
        let unread = unread_tunnels(&out, flood, 3).await;
        let opts = ConnectOpts {
            timeout: Duration::from_secs(5),
        };
        let mut fourth = timeout(BOUND, out.connect_tcp(&target(echo), &opts))
            .await
            .expect("an outcome within the bound")
            .expect("a tunnel beside the unread ones");
        round_trip(&mut fourth, b"past the unread").await;
        assert_eq!(fake.connections(), 1);
        drop(unread);
    }

    /// quinn announces credit for the connection only once an eighth of
    /// its window is waiting to be announced: the window has that eighth to
    /// spare past the windows of its tunnels. With `max-streams=9`, eight
    /// that nobody reads leave the ninth far more than a window of its own.
    /// In a window of nine of them it would stop for good after the first:
    /// what its reader gives back, 8 MiB, is less than an eighth of 72.
    #[tokio::test]
    async fn the_last_tunnel_goes_on_beside_all_the_others_unread() {
        let flood = flood_server().await;
        let fake = endpoint(H3ProxyScript::default());
        let out = outbound(&fake, &format!("{LOGIN}, max-streams=9"));
        let unread = unread_tunnels(&out, flood, 8).await;
        let mut ninth = tunnel(&out, &target(flood)).await;
        // three windows of a stream
        let (mut read, mut buf) = (0, vec![0u8; 64 << 10]);
        let deadline = Instant::now() + 3 * BOUND;
        while read < 24 << 20 {
            let n = tokio::time::timeout_at(deadline, ninth.read(&mut buf))
                .await
                .unwrap_or_else(|_| panic!("the ninth tunnel stopped after {read} bytes"))
                .unwrap();
            assert!(n > 0, "the ninth tunnel ended after {read} bytes");
            read += n;
        }
        assert_eq!(fake.connections(), 1);
        drop(unread);
    }

    /// Design 3.5: a server going away (an HTTP/3 GOAWAY) takes no new
    /// tunnel: the next goes on a new connection, while one under way on
    /// the old runs to its end; the client closes the old connection once
    /// nothing holds it.
    #[tokio::test]
    async fn after_a_goaway_new_tunnels_go_on_a_new_connection_and_old_ones_finish() {
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript {
            goaway_after: Some(1),
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, LOGIN);
        // held from before the first tunnel, to see its GOAWAY come
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
        timeout(BOUND, old.shutdown())
            .await
            .expect("shut down within the bound")
            .unwrap();
        let mut rest = Vec::new();
        timeout(BOUND, old.read_to_end(&mut rest))
            .await
            .expect("the end of the tunnel")
            .unwrap();
        assert!(rest.is_empty());
        drop(old);
        let closed = timeout(BOUND, fake.connection(0).closed())
            .await
            .expect("closed");
        assert!(
            matches!(&closed, ConnectionError::ApplicationClosed(close)
                if close.error_code == H3_NO_ERROR),
            "{closed:?}"
        );
    }

    /// Design 3.5: the server restarts — losing its connections — as a
    /// tunnel is asked for. The request finds its connection gone (a
    /// stateless reset answers the client's next packet) before the
    /// response came, and goes again, once, on a new connection.
    #[tokio::test]
    async fn a_request_that_finds_the_server_restarted_goes_again_on_a_new_connection() {
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript::default());
        let out = outbound(&fake, LOGIN);
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
    }

    /// `opts.timeout` bounds the whole request: here the server's response,
    /// on a connection that is up already.
    #[tokio::test]
    async fn a_server_that_never_answers_times_out() {
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript {
            silent: true,
            ..H3ProxyScript::default()
        });
        let out = outbound(&fake, LOGIN);
        drop(
            timeout(BOUND, out.session.lease(&ConnectOpts::default()))
                .await
                .unwrap()
                .unwrap(),
        );
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
        assert_eq!(fake.requests().len(), 1, "the request went out");
    }

    /// The credentials and the configured values go to the server and
    /// nowhere else: not into the error of a refused login, nor into a log
    /// line at any level.
    #[tokio::test]
    async fn the_credentials_and_header_values_never_show() {
        const PASSWORD: &str = "s3cret-trust";
        const TOKEN: &str = "t0ken-of-the-header";
        let (log, _logging, _second) = Log::capture();
        let echo = echo_server().await;
        let fake = endpoint(H3ProxyScript {
            users: vec![("u".to_string(), PASSWORD.to_string())],
            ..H3ProxyScript::default()
        });
        let wrong = outbound(
            &fake,
            &format!("username=u, password={PASSWORD}-not, headers=X-Token:{TOKEN}"),
        );
        let err = failure(&wrong, &target(echo)).await;
        assert_eq!(err.to_string(), "trust-tunnel: authentication failed");
        let out = outbound(
            &fake,
            &format!("username=u, password={PASSWORD}, headers=X-Token:{TOKEN}"),
        );
        let mut stream = tunnel(&out, &target(echo)).await;
        round_trip(&mut stream, b"let in").await;
        let basic = basic_authorization("u", PASSWORD);
        let requests = fake.requests();
        assert_eq!(
            requests[1].header("proxy-authorization"),
            Some(basic.as_str())
        );
        assert_eq!(requests[1].header("x-token"), Some(TOKEN));
        let shown = format!("{err} {err:?}");
        let log = log.text();
        assert!(!log.is_empty());
        let wrong_basic = basic_authorization("u", &format!("{PASSWORD}-not"));
        for secret in [
            PASSWORD,
            &basic["Basic ".len()..],
            &wrong_basic["Basic ".len()..],
            TOKEN,
        ] {
            assert!(!shown.contains(secret), "{shown}");
            assert!(!log.contains(secret));
        }
    }
}
