//! `trust-tunnel` outbound over HTTP/2 (manual: Policies › Trust Tunnel;
//! TrustTunnel protocol 5.1; phase 2 M6 design 5.4): every connection is a
//! CONNECT stream on a pooled TLS + HTTP/2 connection, as on `h2-connect`.
//!
//! - The request is `:method CONNECT`, `:authority host:port`,
//!   `proxy-authorization: Basic …` and `user-agent`, then the configured
//!   `headers`, rendered anew for every request; a configured header
//!   replaces one of ours with the same name.
//! - The document marks `user-agent` required (as `<platform> <app_name>`);
//!   the reference endpoint only records it. Unless `headers` sets one, the
//!   fixed `USER_AGENT` goes: the same for every platform and version.
//! - 200 turns the stream into the tunnel; 407 is a refused login; the
//!   endpoint answers 502 for a target it could not reach.
//! - TCP only: no UDP (`_udp2`), ICMP (`_icmp`) or health check (`_check`);
//!   a policy test is a URL test.

use crate::build::{shadow_tls_client, tls_client};
use crate::h2connect::{basic_authorization, connect_request};
use crate::h2pool::{H2Pool, StackDial};
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError};
use http::StatusCode;
use rurge_config::KeystoreItem;
use rurge_config::spec::{HeaderTemplate, ShadowTlsOpts, TrustTunnelSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::sync::Arc;

const LABEL: &str = "trust-tunnel";

/// The `user-agent` when `headers` has none: names the client, nothing
/// about the platform or the version.
pub const USER_AGENT: &str = "rurge";

/// No `Debug`: it holds the credentials.
pub struct TrustTunnelOutbound {
    name: String,
    pool: H2Pool,
    /// The `proxy-authorization` value, ready to send.
    authorization: String,
    headers: Vec<HeaderTemplate>,
}

impl TrustTunnelOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &TrustTunnelSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<TrustTunnelOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it. A spec made by hand may carry
        // headers `rurge-config` never let through: never echo them.
        if let Some(n) = spec.headers.iter().position(|t| !t.is_valid()) {
            return Err(BuildError::new(format!(
                "custom header #{} is not valid",
                n + 1
            )));
        }
        let shadow_tls =
            shadow_tls_client(shadow_tls, Some(&spec.tls), &server.host, roots.clone())?;
        // the SNI is the server's name or `sni=`: the endpoint picks its
        // host by the exact SNI. The spec pins `alpn` to `h2`.
        let tls = tls_client(Some(&spec.tls), &server.host, &["h2"], keystore, roots)?;
        let dial = StackDial {
            label: LABEL,
            stack: Stack::new(connector, server, shadow_tls, tls, None),
        };
        Ok(TrustTunnelOutbound {
            name: name.to_string(),
            pool: H2Pool::new(LABEL, spec.max_streams.max(1), Arc::new(dial)),
            authorization: basic_authorization(spec.username.expose(), spec.password.expose()),
            headers: spec.headers.clone(),
        })
    }

    async fn tunnel(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        let ours = vec![
            (
                "proxy-authorization".to_string(),
                self.authorization.clone(),
            ),
            ("user-agent".to_string(), USER_AGENT.to_string()),
        ];
        // never dial for a target whose name cannot be sent
        let request = connect_request(LABEL, target, ours, &self.headers)?;
        let response = self.pool.open(request, opts).await?;
        match response.status() {
            status if status.is_success() => Ok(Box::new(response.into_body()) as BoxedStream),
            StatusCode::PROXY_AUTHENTICATION_REQUIRED => Err(OutboundError::Proxy(format!(
                "{LABEL}: authentication failed"
            ))),
            status => Err(OutboundError::Proxy(format!(
                "{LABEL}: the server answered {}",
                status.as_u16()
            ))),
        }
    }
}

impl Outbound for TrustTunnelOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    // `udp()` stays `Unsupported`: the engine applies
    // `udp-policy-not-supported-behaviour`

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // one budget for the connection (when one is dialed), TLS, the
            // HTTP/2 handshake and the CONNECT exchange
            match tokio::time::timeout(opts.timeout, self.tunnel(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UdpSupport;
    use crate::testing::{FakeH2Proxy, H2ProxyScript, TlsFixture, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::h2::read_trust_tunnel;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The outbound for `definition` (a `trust-tunnel, host, port, ...` line).
    fn outbound_with(definition: &str, fixture: &TlsFixture) -> TrustTunnelOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("T", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_trust_tunnel(&mut r, &[]).spec;
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        TrustTunnelOutbound::new(
            "T",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            shadow_tls.as_ref(),
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// `trust-tunnel, 127.0.0.1, <the fake's port>, username=u,
    /// password=p<extra>`.
    fn outbound(fake: &FakeH2Proxy, extra: &str, fixture: &TlsFixture) -> TrustTunnelOutbound {
        outbound_with(
            &format!(
                "trust-tunnel, 127.0.0.1, {}, username=u, password=p{extra}",
                fake.addr().port()
            ),
            fixture,
        )
    }

    /// A fake that lets `u` / `p` in, with a certificate for `names`.
    async fn proxy(names: &[&str], script: H2ProxyScript) -> (Arc<TlsFixture>, FakeH2Proxy) {
        let fixture = TlsFixture::new(names);
        let script = H2ProxyScript {
            users: vec![("u".into(), "p".into())],
            ..script
        };
        let fake = FakeH2Proxy::spawn(script, fixture.clone()).await;
        (fixture, fake)
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn round_trip(stream: &mut BoxedStream, payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        let mut back = vec![0u8; payload.len()];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut back))
            .await
            .expect("the echo arrives")
            .unwrap();
        assert_eq!(back, payload);
    }

    async fn refusal(out: &TrustTunnelOutbound, to: SocketAddr) -> String {
        let Err(e) = out.connect_tcp(&target(to), &ConnectOpts::default()).await else {
            panic!("the server let us through");
        };
        assert!(matches!(e, OutboundError::Proxy(_)), "{e}");
        e.to_string()
    }

    #[tokio::test]
    async fn a_tunnel_echoes_with_basic_credentials_and_a_user_agent() {
        let echo = echo_server().await;
        let (fixture, fake) = proxy(
            &["127.0.0.1"],
            H2ProxyScript {
                require_user_agent: true,
                ..H2ProxyScript::default()
            },
        )
        .await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(out.name(), "T");
        assert!(out.http_forward().is_none());
        assert_eq!(out.udp(), UdpSupport::Unsupported);
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"through the tunnel").await;
        let seen = fake.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (seen[0].method.as_str(), seen[0].authority.as_str()),
            ("CONNECT", echo.to_string().as_str())
        );
        // base64("u:p")
        assert_eq!(seen[0].header("proxy-authorization"), Some("Basic dTpw"));
        assert_eq!(seen[0].header("user-agent"), Some(USER_AGENT));
        assert_eq!(
            fixture.seen_at_least(1).await[0].alpn.as_deref(),
            Some("h2")
        );
    }

    #[tokio::test]
    async fn headers_may_set_the_user_agent_and_add_fields() {
        let echo = echo_server().await;
        let (fixture, fake) = proxy(&["127.0.0.1"], H2ProxyScript::default()).await;
        let out = outbound(
            &fake,
            ", headers=User-Agent:Linux client<random-string(4)>;X-Note:n",
            &fixture,
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"x").await;
        let seen = &fake.requests()[0];
        let agents: Vec<&str> = seen
            .headers
            .iter()
            .filter(|(n, _)| n == "user-agent")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(agents.len(), 1, "one user-agent: {agents:?}");
        let rest = agents[0].strip_prefix("Linux client").unwrap();
        assert_eq!(rest.len(), 4, "{}", agents[0]);
        assert_eq!(seen.header("x-note"), Some("n"));
        assert_eq!(seen.header("proxy-authorization"), Some("Basic dTpw"));
    }

    #[tokio::test]
    async fn a_refused_login_says_authentication_failed() {
        let echo = echo_server().await;
        let (fixture, fake) = proxy(&["127.0.0.1"], H2ProxyScript::default()).await;
        let out = outbound_with(
            &format!(
                "trust-tunnel, 127.0.0.1, {}, username=u, password=wrong",
                fake.addr().port()
            ),
            &fixture,
        );
        assert_eq!(
            refusal(&out, echo).await,
            "trust-tunnel: authentication failed"
        );
    }

    #[tokio::test]
    async fn an_unreachable_target_and_other_statuses_are_quoted_by_number() {
        // a port nothing listens on: the server cannot connect, 502
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let (fixture, fake) = proxy(&["127.0.0.1"], H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(
            refusal(&out, closed).await,
            "trust-tunnel: the server answered 502"
        );
        let (fixture, fake) = proxy(
            &["127.0.0.1"],
            H2ProxyScript {
                refuse: Some(403),
                ..H2ProxyScript::default()
            },
        )
        .await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(
            refusal(&out, closed).await,
            "trust-tunnel: the server answered 403"
        );
    }

    #[tokio::test]
    async fn the_sni_is_the_server_name_or_the_sni_parameter() {
        let echo = echo_server().await;
        // the server named by its host name
        let (fixture, fake) = proxy(&["localhost"], H2ProxyScript::default()).await;
        let out = outbound_with(
            &format!(
                "trust-tunnel, localhost, {}, username=u, password=p",
                fake.addr().port()
            ),
            &fixture,
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"by name").await;
        assert_eq!(
            fixture.seen_at_least(1).await[0].sni.as_deref(),
            Some("localhost")
        );
        // by address, with the endpoint's host name in `sni=`
        let (fixture, fake) = proxy(&["tt.test"], H2ProxyScript::default()).await;
        let out = outbound(&fake, ", sni=tt.test", &fixture);
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"by sni").await;
        assert_eq!(
            fixture.seen_at_least(1).await[0].sni.as_deref(),
            Some("tt.test")
        );
    }

    #[tokio::test]
    async fn concurrent_tunnels_do_not_wait_behind_the_servers_stream_limit() {
        let echo = echo_server().await;
        let (fixture, fake) = proxy(
            &["127.0.0.1"],
            H2ProxyScript {
                max_concurrent_streams: Some(1),
                ..H2ProxyScript::default()
            },
        )
        .await;
        // `max-streams` 3 by default, but the server takes one at a time
        let out = outbound(&fake, "", &fixture);
        let (to, opts) = (target(echo), ConnectOpts::default());
        let (a, b, c) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                out.connect_tcp(&to, &opts),
                out.connect_tcp(&to, &opts),
                out.connect_tcp(&to, &opts),
            )
        })
        .await
        .expect("a tunnel waited behind the server's limit");
        for mut stream in [a.unwrap(), b.unwrap(), c.unwrap()] {
            round_trip(&mut stream, b"one each").await;
        }
        assert_eq!(fake.connections(), 3);
    }
}
