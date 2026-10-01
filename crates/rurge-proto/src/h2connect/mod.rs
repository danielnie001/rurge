//! `h2-connect` outbound (manual: Policies › HTTP and HTTP/2; phase 2 M6
//! design 5.3): every connection is a CONNECT stream on a pooled TLS +
//! HTTP/2 connection (`h2pool`), `max-streams` of them per connection.
//!
//! - The request is `:method CONNECT` and `:authority host:port` (an IPv6
//!   literal in brackets, an IDN as its A-labels), then
//!   `proxy-authorization: Basic …` when the policy has credentials, then
//!   the configured `headers`, rendered anew for every request. A
//!   configured header replaces one of ours with the same name, as on
//!   `http` / `https`: `headers=Proxy-Authorization:…` wins over the
//!   credentials. No `user-agent` unless `headers` adds one.
//! - A 2xx answer turns the stream into the tunnel; any other status is
//!   the proxy's refusal.
//! - With `udp-relay=true`, UDP goes as CONNECT-UDP (RFC 9298) over
//!   extended CONNECT (RFC 8441), one stream per target (`udp`).

mod capsule;
mod udp;

use crate::build::{shadow_tls_client, tls_client};
use crate::h2pool::{H2Pool, H2Stream, StackDial};
use crate::http::{merge, render, wire_host};
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use h2::ext::Protocol;
use http::header::{HeaderName, HeaderValue};
use http::{Method, Request, Response, StatusCode, Version};
use rurge_config::KeystoreItem;
use rurge_config::spec::{H2ConnectSpec, HeaderTemplate, ShadowTlsOpts};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::sync::Arc;
use udp::H2Udp;

const LABEL: &str = "h2-connect";

/// No `Debug`: it holds the credentials.
pub struct H2ConnectOutbound {
    name: String,
    /// Shared with the UDP carriers.
    inner: Arc<Inner>,
}

struct Inner {
    pool: H2Pool,
    /// The `proxy-authorization` value, ready to send.
    authorization: Option<String>,
    headers: Vec<HeaderTemplate>,
    /// `udp-relay=true`: the server's own `:authority` for CONNECT-UDP.
    udp_authority: Option<String>,
}

/// `Basic base64(user:password)` (RFC 9110 11.7.2, RFC 7617).
pub(crate) fn basic_authorization(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}

/// The CONNECT request for `target`: `ours` (lowercase names), replaced
/// field by field by the rendered `templates`. Fails, before anything is
/// dialed, for a target whose name cannot be sent (the alphabet of
/// `http::wire_host`).
pub(crate) fn connect_request(
    label: &str,
    target: &Target,
    ours: Vec<(String, String)>,
    templates: &[HeaderTemplate],
) -> Result<Request<()>, OutboundError> {
    let Some(host) = wire_host(target) else {
        return Err(unsendable(label));
    };
    request(label, format!("{host}:{}", target.port), ours, templates)
}

fn unsendable(label: &str) -> OutboundError {
    OutboundError::Proxy(format!(
        "{label}: the host name cannot be sent to the server"
    ))
}

/// A CONNECT request for `uri` carrying `ours`, replaced field by field by
/// the rendered `templates`.
fn request(
    label: &str,
    uri: String,
    ours: Vec<(String, String)>,
    templates: &[HeaderTemplate],
) -> Result<Request<()>, OutboundError> {
    let mut fields = ours;
    merge(&mut fields, render(templates));
    let mut request = Request::builder()
        .method(Method::CONNECT)
        .uri(uri)
        .version(Version::HTTP_2);
    for (name, value) in fields {
        // `HeaderName` lowercases, as HTTP/2 wants; valid templates always
        // convert (`HeaderTemplate::is_valid`, checked at build time)
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) else {
            return Err(OutboundError::Proxy(format!(
                "{label}: a custom header is not valid"
            )));
        };
        request = request.header(name, value);
    }
    request
        .body(())
        .map_err(|_| OutboundError::Proxy(format!("{label}: the CONNECT request is not valid")))
}

impl H2ConnectOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &H2ConnectSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<H2ConnectOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it. A spec made by hand may carry
        // headers `rurge-config` never let through: never echo them.
        if let Some(n) = spec.headers.iter().position(|t| !t.is_valid()) {
            return Err(BuildError::new(format!(
                "custom header #{} is not valid",
                n + 1
            )));
        }
        // CONNECT-UDP names the server itself (RFC 9298 3.4)
        let udp_authority = match spec.udp_relay {
            false => None,
            true => match wire_host(&server) {
                Some(host) => Some(format!("{host}:{}", server.port)),
                None => {
                    return Err(BuildError::new(
                        "`udp-relay`: the server name cannot be sent in a request".to_string(),
                    ));
                }
            },
        };
        let shadow_tls =
            shadow_tls_client(shadow_tls, Some(&spec.tls), &server.host, roots.clone())?;
        // the spec pins `alpn` to `h2`; `h2` is also what an empty one offers
        let tls = tls_client(Some(&spec.tls), &server.host, &["h2"], keystore, roots)?;
        let authorization = spec.username.as_ref().map(|user| {
            let password = spec.password.as_ref().map_or("", |p| p.expose().as_str());
            basic_authorization(user.expose(), password)
        });
        let dial = StackDial {
            label: LABEL,
            stack: Stack::new(connector, server, shadow_tls, tls, None),
        };
        Ok(H2ConnectOutbound {
            name: name.to_string(),
            inner: Arc::new(Inner {
                pool: H2Pool::new(LABEL, spec.max_streams.max(1), Arc::new(dial)),
                authorization,
                headers: spec.headers.clone(),
                udp_authority,
            }),
        })
    }
}

impl Inner {
    /// Our fields of every request: the credentials, if any.
    fn ours(&self) -> Vec<(String, String)> {
        self.authorization
            .iter()
            .map(|value| ("proxy-authorization".to_string(), value.clone()))
            .collect()
    }

    /// The stream of a 2xx `response`; any other status is the refusal.
    fn established(response: Response<H2Stream>) -> Result<H2Stream, OutboundError> {
        match response.status() {
            status if status.is_success() => Ok(response.into_body()),
            StatusCode::PROXY_AUTHENTICATION_REQUIRED => Err(OutboundError::Proxy(format!(
                "{LABEL}: proxy authentication required"
            ))),
            status => Err(OutboundError::Proxy(format!(
                "{LABEL}: the proxy answered {}",
                status.as_u16()
            ))),
        }
    }

    async fn tunnel(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        // never dial for a target whose name cannot be sent
        let request = connect_request(LABEL, target, self.ours(), &self.headers)?;
        let response = self.pool.open(request, opts).await?;
        Ok(Box::new(Self::established(response)?) as BoxedStream)
    }

    /// A CONNECT-UDP stream to `target` (RFC 9298 3.4): `:protocol
    /// connect-udp`, the server's `:authority`, the default template's
    /// `:path` and `capsule-protocol: ?1` (RFC 9297 3.4), then the same
    /// fields as a CONNECT. The pool refuses it on a connection whose server
    /// did not enable extended CONNECT.
    async fn udp_stream(
        &self,
        authority: &str,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<H2Stream, OutboundError> {
        let Some(path) = udp::masque_path(target) else {
            return Err(unsendable(LABEL));
        };
        let mut ours = self.ours();
        ours.push(("capsule-protocol".to_string(), "?1".to_string()));
        let mut request = request(
            LABEL,
            format!("https://{authority}{path}"),
            ours,
            &self.headers,
        )?;
        request
            .extensions_mut()
            .insert(Protocol::from_static("connect-udp"));
        let response = self.pool.open(request, opts).await?;
        Self::established(response)
    }
}

impl Outbound for H2ConnectOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // one budget for the connection (when one is dialed), TLS, the
            // HTTP/2 handshake and the CONNECT exchange
            match tokio::time::timeout(opts.timeout, self.inner.tunnel(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }

    fn udp(&self) -> UdpSupport {
        if self.inner.udp_authority.is_some() {
            UdpSupport::Native
        } else {
            UdpSupport::Unsupported
        }
    }

    /// Nothing is opened yet: each target gets its stream with its first
    /// datagram.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        let carrier = match &self.inner.udp_authority {
            Some(authority) => {
                let udp = H2Udp::new(
                    self.inner.clone(),
                    authority.clone(),
                    opts.clone(),
                    udp::IDLE,
                );
                Ok(Box::new(udp) as BoxedPacketSocket)
            }
            None => Err(OutboundError::Unsupported(
                "UDP without `udp-relay=true`".to_string(),
            )),
        };
        Box::pin(std::future::ready(carrier))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeH2Proxy, H2ProxyScript, TlsFixture, echo_server, udp_echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::h2::read_h2_connect;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::spec::{HeaderPart, ParamReader};
    use rurge_config::{HostName, KeystoreType, Span};
    use rurge_net::connector::{DirectConnector, PacketSocket, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::Instant;

    /// The outbound for `definition` (an `h2-connect, host, port, ...` line).
    fn outbound_with(
        definition: &str,
        fixture: &TlsFixture,
        keystore: &[KeystoreItem],
    ) -> H2ConnectOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("H", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_h2_connect(&mut r, keystore);
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        H2ConnectOutbound::new(
            "H",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            shadow_tls.as_ref(),
            keystore,
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// `h2-connect, 127.0.0.1, <the fake's port><extra>`.
    fn outbound(fake: &FakeH2Proxy, extra: &str, fixture: &TlsFixture) -> H2ConnectOutbound {
        outbound_with(
            &format!("h2-connect, 127.0.0.1, {}{extra}", fake.addr().port()),
            fixture,
            &[],
        )
    }

    async fn fake(script: H2ProxyScript) -> (Arc<TlsFixture>, FakeH2Proxy) {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
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

    async fn refusal(out: &H2ConnectOutbound, to: SocketAddr) -> String {
        let Err(e) = out.connect_tcp(&target(to), &ConnectOpts::default()).await else {
            panic!("the proxy let us through");
        };
        assert!(matches!(e, OutboundError::Proxy(_)), "{e}");
        e.to_string()
    }

    /// Polls `check` until it holds, for at most five seconds.
    async fn eventually(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn a_tunnel_echoes_over_tls_with_alpn_h2() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(out.name(), "H");
        assert!(out.http_forward().is_none());
        assert!(matches!(out.udp(), crate::UdpSupport::Unsupported));
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"through the stream").await;
        let seen = fake.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "CONNECT");
        assert_eq!(seen[0].authority, echo.to_string());
        assert_eq!(
            (seen[0].protocol.as_deref(), seen[0].path.as_str()),
            (None, "")
        );
        assert!(seen[0].headers.is_empty(), "{:?}", seen[0].headers);
        let tls = fixture.seen_at_least(1).await;
        assert_eq!(tls[0].alpn.as_deref(), Some("h2"));
        assert_eq!(tls[0].sni, None, "an IP literal: no SNI");
    }

    #[tokio::test]
    async fn a_large_payload_crosses_both_ways() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        let stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        // several of both sides' stream windows (1 MiB each)
        let data: Vec<u8> = (0..(5 << 20) + 777).map(|i| (i % 253) as u8).collect();
        let (mut r, mut w) = tokio::io::split(stream);
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            w.write_all(&sent).await.unwrap();
            w.shutdown().await.unwrap();
        });
        let mut back = Vec::new();
        tokio::time::timeout(Duration::from_secs(30), r.read_to_end(&mut back))
            .await
            .expect("stalled")
            .unwrap();
        writer.await.unwrap();
        assert!(back == data, "the echo differs");
    }

    #[tokio::test]
    async fn shutdown_reaches_the_target_as_a_half_close() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"last words").await.unwrap();
        stream.shutdown().await.unwrap();
        // the target saw the FIN, echoed what it had and closed: we read all
        let mut back = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut back))
            .await
            .expect("the target closed")
            .unwrap();
        assert_eq!(back, b"last words");
    }

    #[tokio::test]
    async fn credentials_go_as_basic_and_a_refusal_says_so() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            users: vec![("u".into(), "p:w".into())],
            ..H2ProxyScript::default()
        })
        .await;
        let mut stream = outbound(&fake, ", u, p:w", &fixture)
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"let in").await;
        // base64("u:p:w")
        assert_eq!(
            fake.requests()[0].header("proxy-authorization"),
            Some("Basic dTpwOnc=")
        );
        for extra in ["", ", username=u, password=wrong"] {
            let out = outbound(&fake, extra, &fixture);
            assert_eq!(
                refusal(&out, echo).await,
                "h2-connect: proxy authentication required"
            );
        }
        assert!(fake.requests()[1].header("proxy-authorization").is_none());
    }

    #[tokio::test]
    async fn any_other_status_is_quoted_by_its_number() {
        let echo = echo_server().await;
        for code in [403, 502, 503] {
            let (fixture, fake) = fake(H2ProxyScript {
                refuse: Some(code),
                ..H2ProxyScript::default()
            })
            .await;
            let out = outbound(&fake, "", &fixture);
            assert_eq!(
                refusal(&out, echo).await,
                format!("h2-connect: the proxy answered {code}")
            );
        }
    }

    #[tokio::test]
    async fn headers_are_rendered_per_request_and_replace_ours() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(
            &fake,
            ", user, pass, headers=X-Pad:p<random-string(12)>q<random-string(2-6)>;Proxy-Authorization:Bearer abc",
            &fixture,
        );
        for _ in 0..2 {
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            round_trip(&mut stream, b"x").await;
        }
        let seen = fake.requests();
        assert_eq!(fake.connections(), 1, "both on one connection");
        for request in &seen {
            let auth: Vec<&str> = request
                .headers
                .iter()
                .filter(|(n, _)| n == "proxy-authorization")
                .map(|(_, v)| v.as_str())
                .collect();
            assert_eq!(auth, ["Bearer abc"], "the configured header wins");
            assert!(request.header("user-agent").is_none());
            let pad = request.header("x-pad").expect("lowercased by HTTP/2");
            let inner = pad.strip_prefix('p').unwrap();
            let (first, second) = inner.split_at(12);
            let second = second.strip_prefix('q').unwrap();
            assert!((2..=6).contains(&second.len()), "{pad}");
            assert!(
                first
                    .chars()
                    .chain(second.chars())
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "{pad}"
            );
        }
        assert_ne!(
            seen[0].header("x-pad"),
            seen[1].header("x-pad"),
            "drawn anew for every request, not every connection"
        );
    }

    #[tokio::test]
    async fn the_authority_brackets_ipv6_and_carries_a_labels() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            connect_to: Some(echo),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        for (host, authority) in [
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:443"),
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:443",
            ),
            (HostName::parse("remote.example"), "remote.example:443"),
        ] {
            let mut stream = out
                .connect_tcp(&Target::new(host, 443), &ConnectOpts::default())
                .await
                .unwrap();
            round_trip(&mut stream, b"x").await;
            assert_eq!(fake.requests().last().unwrap().authority, authority);
        }
    }

    #[tokio::test]
    async fn a_name_that_cannot_be_sent_never_dials() {
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        for name in ["x@blocked.test", "a b.test", "a.test\r\nx: 1", ""] {
            let Err(e) = out
                .connect_tcp(
                    &Target::new(HostName::Domain(name.to_string()), 443),
                    &ConnectOpts::default(),
                )
                .await
            else {
                panic!("{name:?} was sent");
            };
            assert_eq!(
                e.to_string(),
                "h2-connect: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.connections(), 0);
    }

    #[tokio::test]
    async fn max_streams_bounds_each_connection() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, ", max-streams=2", &fixture);
        let (to, opts) = (target(echo), ConnectOpts::default());
        let (a, b, c) = tokio::join!(
            out.connect_tcp(&to, &opts),
            out.connect_tcp(&to, &opts),
            out.connect_tcp(&to, &opts),
        );
        let mut streams = [a.unwrap(), b.unwrap(), c.unwrap()];
        for stream in &mut streams {
            round_trip(stream, b"three at once").await;
        }
        assert_eq!(fake.connections(), 2);
        let mut carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        carried.sort_unstable();
        assert_eq!(carried, [0, 0, 1]);
    }

    #[tokio::test]
    async fn the_servers_own_stream_limit_is_kept_too() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            max_concurrent_streams: Some(1),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        // all at once: none may queue behind the server's limit, unknown
        // until its SETTINGS arrive
        let (to, opts) = (target(echo), ConnectOpts::default());
        let (a, b) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(out.connect_tcp(&to, &opts), out.connect_tcp(&to, &opts))
        })
        .await
        .expect("a tunnel waited behind the server's limit");
        let (mut first, mut second) = (a.unwrap(), b.unwrap());
        round_trip(&mut first, b"one").await;
        round_trip(&mut second, b"two").await;
        assert_eq!(fake.connections(), 2);
    }

    #[tokio::test]
    async fn after_goaway_the_next_tunnel_takes_a_new_connection() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            goaway_after: Some(1),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        let mut first = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        eventually(|| out.inner.pool.connections() == 0).await;
        let mut second = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        // the stream from before the GOAWAY runs on
        round_trip(&mut first, b"old").await;
        round_trip(&mut second, b"new").await;
        assert_eq!(fake.connections(), 2);
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 1]);
    }

    #[tokio::test]
    async fn a_server_without_h2_is_refused_before_http2() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            no_alpn: true,
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(
            refusal(&out, echo).await,
            "h2-connect: the server does not speak HTTP/2"
        );
        assert!(fake.requests().is_empty());
    }

    #[tokio::test]
    async fn a_client_certificate_comes_from_the_keystore() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            require_client_cert: true,
            ..H2ProxyScript::default()
        })
        .await;
        let keystore = [KeystoreItem {
            name: "mtls".into(),
            kind: KeystoreType::P12,
            base64: fixture.client_p12_base64("rurge"),
            password: Some("pw".into()),
            unknown: Vec::new(),
            span: Span::new(Arc::from(Path::new("p.conf")), 1),
        }];
        let out = outbound_with(
            &format!(
                "h2-connect, 127.0.0.1, {}, client-cert=mtls",
                fake.addr().port()
            ),
            &fixture,
            &keystore,
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"mutual").await;
        assert!(fixture.seen_at_least(1).await[0].client_cert);
    }

    #[tokio::test]
    async fn a_silent_proxy_times_out() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            delay: Duration::from_secs(30),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        let started = Instant::now();
        let Err(e) = out
            .connect_tcp(
                &target(echo),
                &ConnectOpts {
                    timeout: Duration::from_millis(300),
                },
            )
            .await
        else {
            panic!("the proxy never answered");
        };
        assert!(matches!(e, OutboundError::Timeout), "{e}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(fake.requests().len(), 1, "the CONNECT was sent");
    }

    #[test]
    fn invalid_header_templates_are_refused_at_build_time() {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let mut spec = H2ConnectSpec {
            tls: rurge_config::spec::TlsOpts::default(),
            username: None,
            password: None,
            headers: Vec::new(),
            max_streams: 3,
            udp_relay: false,
        };
        spec.headers.push(HeaderTemplate {
            name: "X-Evil".into(),
            value: vec![HeaderPart::Literal("a\r\nb: 1".into())],
        });
        let err = H2ConnectOutbound::new(
            "H",
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            None,
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "custom header #1 is not valid");
    }

    #[test]
    fn basic_authorization_is_base64_of_user_colon_password() {
        // RFC 7617 2
        assert_eq!(
            basic_authorization("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }

    /// A fake that serves CONNECT-UDP.
    async fn udp_fake(script: H2ProxyScript) -> (Arc<TlsFixture>, FakeH2Proxy) {
        fake(H2ProxyScript {
            extended_connect: true,
            ..script
        })
        .await
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(Duration::from_secs(10), carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_roundtrip(carrier: &dyn PacketSocket, to: &Target, payload: &[u8]) {
        carrier.send_to(payload, to).await.unwrap();
        assert_eq!(udp_answer(carrier).await, (payload.to_vec(), to.clone()));
    }

    fn masque(addr: SocketAddr) -> String {
        format!("/.well-known/masque/udp/{}/{}/", addr.ip(), addr.port())
    }

    /// RFC 9298 3.4: extended CONNECT to the server's own authority, the
    /// target in the path, `capsule-protocol: ?1`, our fields as on TCP.
    #[tokio::test]
    async fn a_datagram_crosses_a_connect_udp_stream() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = udp_fake(H2ProxyScript {
            users: vec![("u".into(), "p".into())],
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, ", u, p, udp-relay=true, headers=X-Pad:x", &fixture);
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        assert_eq!(fake.connections(), 0, "nothing is opened before a datagram");
        udp_roundtrip(carrier.as_ref(), &target(echo), b"through a capsule").await;
        udp_roundtrip(carrier.as_ref(), &target(echo), b"").await;
        let seen = fake.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "CONNECT");
        assert_eq!(seen[0].protocol.as_deref(), Some("connect-udp"));
        assert_eq!(seen[0].authority, fake.addr().to_string());
        assert_eq!(seen[0].path, masque(echo));
        assert_eq!(seen[0].header("capsule-protocol"), Some("?1"));
        assert_eq!(seen[0].header("proxy-authorization"), Some("Basic dTpw"));
        assert_eq!(seen[0].header("x-pad"), Some("x"));
        // a refused CONNECT-UDP fails the datagram with the TCP texts
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let err = carrier.send_to(b"x", &target(echo)).await.unwrap_err();
        assert_eq!(err.to_string(), "h2-connect: proxy authentication required");
    }

    /// One stream per target (M6-D8), the streams sharing the pooled
    /// connection; each answer is its stream's target's.
    #[tokio::test]
    async fn every_target_has_its_own_stream() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let (fixture, fake) = udp_fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), &target(one), b"to one").await;
        udp_roundtrip(carrier.as_ref(), &target(two), b"to two").await;
        udp_roundtrip(carrier.as_ref(), &target(one), b"one again").await;
        let seen: Vec<(usize, String)> = fake
            .requests()
            .iter()
            .map(|r| (r.connection, r.path.clone()))
            .collect();
        assert_eq!(seen, [(0, masque(one)), (0, masque(two))]);
        assert_eq!(fake.connections(), 1);
    }

    /// Names as A-labels, IPv6 literals with `%3A` (RFC 9298 3).
    #[tokio::test]
    async fn names_and_ipv6_literals_go_in_the_path() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = udp_fake(H2ProxyScript {
            connect_to: Some(echo),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        for (host, path) in [
            (
                HostName::Domain("bücher.example".into()),
                "/.well-known/masque/udp/xn--bcher-kva.example/53/",
            ),
            (
                HostName::parse("2001:db8::42"),
                "/.well-known/masque/udp/2001%3Adb8%3A%3A42/53/",
            ),
        ] {
            udp_roundtrip(carrier.as_ref(), &Target::new(host, 53), b"q").await;
            assert_eq!(fake.requests().last().unwrap().path, path);
        }
        let err = carrier
            .send_to(
                b"q",
                &Target::new(HostName::Domain("x@blocked.test".into()), 53),
            )
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "h2-connect: the host name cannot be sent to the server"
        );
        assert_eq!(fake.requests().len(), 2);
    }

    /// RFC 8441 3: no extended CONNECT before the server allows it.
    #[tokio::test]
    async fn a_server_without_extended_connect_carries_no_udp() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let err = carrier.send_to(b"x", &target(echo)).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "h2-connect: the server does not support extended CONNECT"
        );
        assert!(fake.requests().is_empty());
        // the connection still carries TCP
        let tcp = echo_server().await;
        let mut stream = out
            .connect_tcp(&target(tcp), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"tcp is fine").await;
        assert_eq!(fake.connections(), 1, "the same connection");
    }

    /// A proxy that answers 200 and ends every stream at once: one reopen
    /// per datagram, then an error.
    #[tokio::test]
    async fn streams_ended_at_once_are_reopened_only_once() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = udp_fake(H2ProxyScript {
            udp_end_at_once: true,
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let result =
            tokio::time::timeout(Duration::from_secs(5), carrier.send_to(b"x", &target(echo)))
                .await
                .expect("send_to ended");
        // the stream may be seen ended by the first or the second look
        if let Err(e) = result {
            assert_eq!(e.kind(), std::io::ErrorKind::ConnectionAborted);
        }
        assert!(fake.requests().len() <= 2, "{}", fake.requests().len());
    }

    /// RFC 9297 3.2, RFC 9298 4: other capsule types and context ids are
    /// skipped; nothing of them reaches the engine.
    #[tokio::test]
    async fn other_capsules_are_skipped() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = udp_fake(H2ProxyScript {
            udp_extra_capsules: true,
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), &target(echo), b"first").await;
        udp_roundtrip(carrier.as_ref(), &target(echo), b"second").await;
    }

    /// RFC 9298 5: at most 65527 bytes with context id 0; a longer datagram
    /// is refused and nothing is opened.
    #[tokio::test]
    async fn a_datagram_too_long_is_refused() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = udp_fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let err = carrier
            .send_to(&vec![0u8; 65528], &target(echo))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "h2-connect: a datagram longer than 65527 bytes"
        );
        assert_eq!(fake.connections(), 0);
    }

    #[tokio::test]
    async fn without_udp_relay_there_is_no_udp() {
        let (fixture, fake) = udp_fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(out.udp(), UdpSupport::Unsupported);
        let Err(e) = out.open_udp(&ConnectOpts::default()).await else {
            panic!("a carrier without udp-relay");
        };
        assert_eq!(
            e.to_string(),
            "policy protocol not implemented: UDP without `udp-relay=true`"
        );
    }

    /// A stream idle past the bound closes and leaves the table; the
    /// target's next datagram opens a new one on the pooled connection.
    #[tokio::test]
    async fn an_idle_stream_closes_and_the_next_datagram_reopens() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = udp_fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = H2Udp::new(
            out.inner.clone(),
            fake.addr().to_string(),
            ConnectOpts::default(),
            Duration::from_millis(200),
        );
        udp_roundtrip(&carrier, &target(echo), b"first").await;
        assert_eq!(carrier.tracked(), 1);
        eventually(|| carrier.tracked() == 0).await;
        udp_roundtrip(&carrier, &target(echo), b"second").await;
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 0]);
    }

    #[test]
    fn udp_relay_needs_a_server_name_that_can_be_sent() {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let spec = H2ConnectSpec {
            tls: rurge_config::spec::TlsOpts::default(),
            username: None,
            password: None,
            headers: Vec::new(),
            max_streams: 3,
            udp_relay: true,
        };
        let err = H2ConnectOutbound::new(
            "H",
            Target::new(HostName::Domain("a b.test".into()), 443),
            &spec,
            None,
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(
            err.message,
            "`udp-relay`: the server name cannot be sent in a request"
        );
    }
}
