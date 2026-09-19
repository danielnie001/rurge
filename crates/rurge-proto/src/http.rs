//! `http` / `https` proxy outbound: CONNECT tunnels, and absolute-form
//! forwarding of plain HTTP requests (manual: Policies › HTTP and HTTP/2).

use crate::build::tls_client;
use crate::outbound::untrusted_text;
use crate::transport::head::read_head;
use crate::transport::prefixed;
use crate::transport::tls::TlsClient;
use crate::{BuildError, HttpForward, Outbound, OutboundError};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rurge_config::spec::{HeaderPart, HeaderTemplate, PolicySpec, ProtoSpec};
use rurge_config::{HostName, KeystoreItem};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

/// Largest CONNECT response head we accept.
pub const MAX_HEAD: usize = 16 * 1024;

/// 64 URL-safe symbols: a random byte maps onto them without bias.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub struct HttpOutbound {
    name: String,
    server: Target,
    connector: Arc<dyn Connector>,
    tls: Option<TlsClient>,
    /// The `Proxy-Authorization` value, ready to send.
    authorization: Option<String>,
    headers: Vec<HeaderTemplate>,
    forward: bool,
}

fn random_string(min: usize, max: usize) -> String {
    // defence in depth: no arithmetic underflow / out-of-range slice below,
    // even if an invalid template ever got past `from_spec`
    let max = max.max(min);
    let mut pick = [0u8; 4];
    let mut bytes = vec![0u8; max];
    if getrandom::fill(&mut pick).is_err() || getrandom::fill(&mut bytes).is_err() {
        // padding, not a secret: degrade instead of failing the connection
        tracing::warn!("no random source; <random-string> placeholders are not random");
    }
    let len = min + (u32::from_le_bytes(pick) as usize) % (max - min + 1);
    bytes[..len]
        .iter()
        .map(|b| char::from(ALPHABET[usize::from(b & 63)]))
        .collect()
}

fn render(templates: &[HeaderTemplate]) -> Vec<(String, String)> {
    templates
        .iter()
        .map(|t| {
            let value = t
                .value
                .iter()
                .map(|part| match part {
                    HeaderPart::Literal(text) => text.clone(),
                    HeaderPart::Random { min, max } => random_string(*min, *max),
                })
                .collect();
            (t.name.clone(), value)
        })
        .collect()
}

/// A configured header replaces one of ours with the same name, `Host` included (manual).
fn merge(base: &mut Vec<(String, String)>, custom: Vec<(String, String)>) {
    for (name, value) in custom {
        base.retain(|(existing, _)| !existing.eq_ignore_ascii_case(&name));
        base.push((name, value));
    }
}

fn authority(target: &Target) -> String {
    match &target.host {
        HostName::Ip(IpAddr::V6(v6)) => format!("[{v6}]:{}", target.port),
        host => format!("{host}:{}", target.port),
    }
}

/// Whether `target`'s host name is safe to write verbatim into an HTTP
/// request line or a `Host` header: an IP literal, or a non-empty domain made
/// only of bytes `0x21..=0x7e` (printable, non-space ASCII).
///
/// A domain name can be caller-supplied (it may come from a client's SOCKS5
/// / HTTP CONNECT request) and must not be trusted: anything outside that
/// range — a CR or LF above all — could break out of the request line or the
/// `Host` header of a text protocol, smuggling a second request into the
/// proxy connection and carrying our `Proxy-Authorization` with it. An IP
/// literal's `Display` can never contain such bytes, so it is always fine.
pub fn valid_target(target: &Target) -> bool {
    match &target.host {
        HostName::Ip(_) => true,
        HostName::Domain(name) => {
            !name.is_empty() && name.bytes().all(|b| (0x21..=0x7e).contains(&b))
        }
    }
}

fn check_status(head: &[u8]) -> Result<(), OutboundError> {
    let text = String::from_utf8_lossy(head);
    let line = text.lines().next().unwrap_or_default();
    let mut parts = line.splitn(3, ' ');
    let (version, code) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    // the proxy's own text, so bounded and stripped of control characters
    // before it can reach the session log or the request log
    let reason = untrusted_text(parts.next().unwrap_or("").trim(), 64);
    match code.parse::<u16>() {
        Ok(code) if version.starts_with("HTTP/1.") && (200..300).contains(&code) => Ok(()),
        Ok(code) if version.starts_with("HTTP/1.") => {
            Err(OutboundError::Proxy(if reason.is_empty() {
                format!("http proxy answered {code}")
            } else {
                format!("http proxy answered {code} {reason}")
            }))
        }
        _ => Err(OutboundError::Proxy(
            "http proxy sent a malformed response".to_string(),
        )),
    }
}

impl HttpOutbound {
    pub fn from_spec(
        spec: &PolicySpec,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<HttpOutbound, BuildError> {
        let (ProtoSpec::Http(http), Some(host), Some(port)) =
            (&spec.proto, &spec.server, spec.port)
        else {
            return Err(BuildError::new(format!(
                "policy `{}` is not an http / https policy",
                spec.name
            )));
        };
        // `PolicySpec`'s fields are public, so a caller could hand us headers
        // that never went through `rurge-config`'s own `parse_list`: never
        // echo the name or value, both may carry sensitive or hostile text.
        if let Some((n, _)) = http.headers.iter().enumerate().find(|(_, t)| !t.is_valid()) {
            return Err(BuildError::new(format!(
                "policy `{}`: custom header #{} is not valid",
                spec.name,
                n + 1
            )));
        }
        let tls = tls_client(http.tls.as_ref(), host, &[], keystore, roots)?;
        let authorization = http.username.as_ref().map(|user| {
            let password = http.password.as_deref().unwrap_or("");
            format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
        });
        Ok(HttpOutbound {
            name: spec.name.clone(),
            server: Target::new(host.clone(), port),
            connector,
            tls,
            authorization,
            headers: http.headers.clone(),
            forward: !http.always_use_connect,
        })
    }

    /// TCP to the proxy, then TLS for `https`.
    async fn dial(&self, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        let stream = self.connector.connect(&self.server, opts).await?;
        match &self.tls {
            Some(tls) => tls.wrap(stream).await.map_err(OutboundError::tls),
            None => Ok(stream),
        }
    }

    /// The caller must have already checked `valid_target(target)`: this
    /// never re-checks, and a rejected target must never reach it.
    fn connect_request(&self, target: &Target) -> Vec<u8> {
        let authority = authority(target);
        let mut headers = vec![("Host".to_string(), authority.clone())];
        if let Some(value) = &self.authorization {
            headers.push(("Proxy-Authorization".to_string(), value.clone()));
        }
        merge(&mut headers, render(&self.headers));
        let mut request = format!("CONNECT {authority} HTTP/1.1\r\n");
        for (name, value) in headers {
            request.push_str(&name);
            request.push_str(": ");
            request.push_str(&value);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        request.into_bytes()
    }

    async fn tunnel(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        if !valid_target(target) {
            // never dial or write a byte for a target we cannot safely quote
            return Err(OutboundError::Proxy(
                "the target host name is not valid for an HTTP proxy request".to_string(),
            ));
        }
        let mut stream = self.dial(opts).await?;
        stream.write_all(&self.connect_request(target)).await?;
        let (head, rest) = read_head(&mut stream, MAX_HEAD)
            .await
            .map_err(|e| match e.kind() {
                io::ErrorKind::InvalidData => OutboundError::Proxy(format!(
                    "http proxy sent a response header larger than {MAX_HEAD} bytes"
                )),
                io::ErrorKind::UnexpectedEof => OutboundError::Proxy(
                    "http proxy closed the connection during CONNECT".to_string(),
                ),
                _ => OutboundError::from(e),
            })?;
        check_status(&head)?;
        Ok(prefixed::boxed(rest, stream))
    }
}

impl Outbound for HttpOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // one budget for the connection, TLS and the CONNECT exchange
            match tokio::time::timeout(opts.timeout, self.tunnel(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }

    fn http_forward(&self) -> Option<&dyn HttpForward> {
        self.forward.then_some(self as &dyn HttpForward)
    }
}

impl HttpForward for HttpOutbound {
    fn connect<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.dial(opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }

    fn request_headers(&self) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        if let Some(value) = &self.authorization {
            headers.push(("Proxy-Authorization".to_string(), value.clone()));
        }
        merge(&mut headers, render(&self.headers));
        headers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeHttpProxy, HttpProxyScript, TlsFixture, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::{NameKind, SpecEnv, to_spec};
    use rurge_config::{KeystoreItem, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn spec(definition: &str) -> PolicySpec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("Up", definition, &span).unwrap();
        let lookup = |_: &str| -> Option<NameKind> { None };
        let outcome = to_spec(
            &policy,
            &SpecEnv {
                keystore: &[],
                lookup: &lookup,
            },
        );
        outcome
            .spec
            .unwrap_or_else(|| panic!("{:?}", outcome.diagnostics))
    }

    fn outbound(definition: &str, roots: Arc<RootCertStore>) -> HttpOutbound {
        let keystore: Vec<KeystoreItem> = Vec::new();
        HttpOutbound::from_spec(
            &spec(definition),
            &keystore,
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

    async fn roundtrip(stream: &mut BoxedStream, payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        let mut buf = vec![0u8; payload.len()];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, payload);
    }

    #[tokio::test]
    async fn connect_tunnels_with_credentials() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript {
            auth: Some(("user".into(), "pa:ss".into())),
            ..HttpProxyScript::default()
        })
        .await;
        let out = outbound(
            &format!("http, 127.0.0.1, {}, user, pa:ss", proxy.addr().port()),
            no_roots(),
        );
        assert_eq!(out.name(), "Up");
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"through the tunnel").await;
        let head = &proxy.heads()[0];
        assert_eq!(head.request_line, format!("CONNECT {echo} HTTP/1.1"));
        assert_eq!(head.header("Host"), Some(echo.to_string().as_str()));
        // base64("user:pa:ss")
        assert_eq!(
            head.header("Proxy-Authorization"),
            Some("Basic dXNlcjpwYTpzcw==")
        );
    }

    #[tokio::test]
    async fn the_target_name_is_sent_as_it_is_and_ipv6_is_bracketed() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript {
            connect_to: Some(echo),
            ..HttpProxyScript::default()
        })
        .await;
        let out = outbound(
            &format!("http, 127.0.0.1, {}", proxy.addr().port()),
            no_roots(),
        );
        for (host, line) in [
            ("remote.example", "CONNECT remote.example:443 HTTP/1.1"),
            ("2001:db8::1", "CONNECT [2001:db8::1]:443 HTTP/1.1"),
        ] {
            let mut stream = out
                .connect_tcp(
                    &Target::new(HostName::parse(host), 443),
                    &ConnectOpts::default(),
                )
                .await
                .unwrap();
            roundtrip(&mut stream, b"x").await;
            assert_eq!(proxy.heads().last().unwrap().request_line, line);
        }
        assert!(proxy.heads()[0].header("Proxy-Authorization").is_none());
    }

    #[tokio::test]
    async fn bytes_behind_the_response_head_belong_to_the_tunnel() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript {
            trailing: b"early".to_vec(),
            padding: 3000,
            ..HttpProxyScript::default()
        })
        .await;
        let out = outbound(
            &format!("http, 127.0.0.1, {}", proxy.addr().port()),
            no_roots(),
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        let mut early = [0u8; 5];
        stream.read_exact(&mut early).await.unwrap();
        assert_eq!(&early, b"early");
        roundtrip(&mut stream, b"after").await;
    }

    #[tokio::test]
    async fn refusals_become_proxy_errors() {
        let echo = echo_server().await;
        let cases: Vec<(HttpProxyScript, &str)> = vec![
            (
                HttpProxyScript {
                    auth: Some(("u".into(), "p".into())),
                    ..HttpProxyScript::default()
                },
                "http proxy answered 407 Proxy Authentication Required",
            ),
            (
                HttpProxyScript {
                    refuse: Some((503, "Service Unavailable")),
                    ..HttpProxyScript::default()
                },
                "http proxy answered 503 Service Unavailable",
            ),
            (
                HttpProxyScript {
                    padding: 20_000,
                    ..HttpProxyScript::default()
                },
                "http proxy sent a response header larger than 16384 bytes",
            ),
            (
                HttpProxyScript {
                    truncate: true,
                    ..HttpProxyScript::default()
                },
                "http proxy closed the connection during CONNECT",
            ),
        ];
        for (script, expected) in cases {
            let proxy = FakeHttpProxy::spawn(script).await;
            let out = outbound(
                &format!("http, 127.0.0.1, {}", proxy.addr().port()),
                no_roots(),
            );
            let err = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .map(|_| ())
                .unwrap_err();
            assert!(
                matches!(&err, OutboundError::Proxy(m) if m == expected),
                "{err}"
            );
        }
    }

    #[tokio::test]
    async fn a_silent_proxy_times_out() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript {
            delay: Duration::from_secs(30),
            ..HttpProxyScript::default()
        })
        .await;
        let out = outbound(
            &format!("http, 127.0.0.1, {}", proxy.addr().port()),
            no_roots(),
        );
        let err = out
            .connect_tcp(
                &target(echo),
                &ConnectOpts {
                    timeout: Duration::from_millis(200),
                },
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
    }

    #[tokio::test]
    async fn custom_headers_replace_and_random_strings_are_rendered_per_connection() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
        let out = outbound(
            &format!(
                "http, 127.0.0.1, {}, headers=Host:edge.example;X-Pad:p<random-string(12)>q<random-string(2-6)>",
                proxy.addr().port()
            ),
            no_roots(),
        );
        for _ in 0..2 {
            out.connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
        }
        let heads = proxy.heads();
        let hosts: Vec<&str> = heads[0]
            .headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("host"))
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(hosts, ["edge.example"], "the configured Host replaces ours");
        let pads: Vec<&str> = heads.iter().map(|h| h.header("X-Pad").unwrap()).collect();
        for pad in &pads {
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
        assert_ne!(pads[0], pads[1], "rendered anew for every connection");
    }

    #[tokio::test]
    async fn https_wraps_the_proxy_connection_in_tls() {
        let echo = echo_server().await;
        let fixture = TlsFixture::new(&["localhost"]);
        let proxy =
            FakeHttpProxy::spawn_tls(HttpProxyScript::default(), fixture.clone(), false).await;
        let out = outbound(
            &format!("https, localhost, {}", proxy.addr().port()),
            fixture.roots(),
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"inside tls").await;
        assert_eq!(fixture.seen()[0].sni.as_deref(), Some("localhost"));
        // an untrusted proxy certificate is a TLS error, not a proxy error
        let distrusting = outbound(
            &format!("https, localhost, {}", proxy.addr().port()),
            TlsFixture::new(&["x.test"]).roots(),
        );
        let err = distrusting
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(err, OutboundError::Tls(_)), "{err}");
    }

    #[tokio::test]
    async fn forward_mode_hands_out_the_connection_and_the_headers() {
        let proxy = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
        let out = outbound(
            &format!(
                "http, 127.0.0.1, {}, user, pass, headers=X-Via:rurge",
                proxy.addr().port()
            ),
            no_roots(),
        );
        let forward = out
            .http_forward()
            .expect("always-use-connect defaults to false");
        assert_eq!(
            forward.request_headers(),
            [
                (
                    "Proxy-Authorization".to_string(),
                    "Basic dXNlcjpwYXNz".to_string()
                ),
                ("X-Via".to_string(), "rurge".to_string()),
            ]
        );
        let mut stream = forward.connect(&ConnectOpts::default()).await.unwrap();
        stream
            .write_all(b"GET http://origin.example/path HTTP/1.1\r\nHost: origin.example\r\n\r\n")
            .await
            .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        assert!(
            answer.starts_with("HTTP/1.1 200 OK") && answer.ends_with("forwarded"),
            "{answer}"
        );
        assert_eq!(
            proxy.heads()[0].request_line,
            "GET http://origin.example/path HTTP/1.1"
        );

        let tunnel_only = outbound(
            &format!(
                "http, 127.0.0.1, {}, always-use-connect=true",
                proxy.addr().port()
            ),
            no_roots(),
        );
        assert!(tunnel_only.http_forward().is_none());
    }

    #[test]
    fn only_http_specs_build() {
        let direct = spec("direct");
        let keystore: Vec<KeystoreItem> = Vec::new();
        let err = HttpOutbound::from_spec(
            &direct,
            &keystore,
            no_roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "policy `Up` is not an http / https policy");
    }

    #[tokio::test]
    async fn a_target_with_control_characters_never_reaches_the_proxy() {
        let proxy = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
        let out = outbound(
            &format!("http, 127.0.0.1, {}", proxy.addr().port()),
            no_roots(),
        );
        for name in [
            "a.test\r\nX-Evil: 1\r\n\r\nCONNECT internal.test:22 HTTP/1.1\r\nHost: internal.test:22",
            "a b.test",
            "",
        ] {
            let err = out
                .connect_tcp(
                    &Target::new(HostName::Domain(name.to_string()), 443),
                    &ConnectOpts::default(),
                )
                .await
                .map(|_| ())
                .unwrap_err();
            assert!(
                matches!(&err, OutboundError::Proxy(m) if m == "the target host name is not valid for an HTTP proxy request"),
                "{err}"
            );
            assert!(!err.to_string().contains("X-Evil"), "{err}");
        }
        assert!(proxy.heads().is_empty(), "{:?}", proxy.heads());
    }

    #[test]
    fn invalid_header_templates_are_refused_at_build_time() {
        // a valid spec built the normal way, then a second header hand-built
        // past what `rurge-config` itself would ever let through:
        // `from_spec` must not trust that `PolicySpec` came from there.
        let base = spec("http, 127.0.0.1, 1080, headers=X-Client:rurge");
        let keystore: Vec<KeystoreItem> = Vec::new();
        let bad_templates = [
            HeaderTemplate {
                name: "X-Evil".to_string(),
                value: vec![HeaderPart::Literal("a\r\nX-Evil: 1".to_string())],
            },
            HeaderTemplate {
                name: "X-Bad".to_string(),
                value: vec![HeaderPart::Random { min: 5, max: 2 }],
            },
        ];
        for bad in bad_templates {
            let mut modified = base.clone();
            let ProtoSpec::Http(http) = &mut modified.proto else {
                panic!("expected an http spec");
            };
            http.headers.push(bad);
            let err = HttpOutbound::from_spec(
                &modified,
                &keystore,
                no_roots(),
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(err.message, "policy `Up`: custom header #2 is not valid");
            assert!(!err.message.contains("X-Evil"), "{}", err.message);
            assert!(!err.message.contains("X-Bad"), "{}", err.message);
        }
    }

    #[test]
    fn random_string_does_not_panic_when_min_exceeds_max() {
        assert_eq!(random_string(5, 2).chars().count(), 5);
    }

    #[test]
    fn check_status_rejects_malformed_lines_and_accepts_any_1_x_success() {
        for head in [
            b"HTTP/2 200 OK\r\n\r\n".as_slice(),
            b"garbage\r\n\r\n".as_slice(),
            b"".as_slice(),
        ] {
            let err = check_status(head).map(|_| ()).unwrap_err();
            assert!(
                matches!(&err, OutboundError::Proxy(m) if m == "http proxy sent a malformed response"),
                "{err}"
            );
        }
        check_status(b"HTTP/1.1 204 No Content\r\n\r\n").unwrap();
        check_status(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
    }

    #[test]
    fn check_status_sanitizes_and_bounds_the_reason_phrase() {
        let mut head = b"HTTP/1.1 403 For\x1bbid\rden".to_vec();
        head.extend(std::iter::repeat_n(b'A', 20_000));
        head.extend_from_slice(b"\r\n\r\n");
        let err = check_status(&head).map(|_| ()).unwrap_err();
        let OutboundError::Proxy(m) = &err else {
            panic!("{err}");
        };
        assert!(m.starts_with("http proxy answered 403 Forbidden"), "{m}");
        assert!(!m.chars().any(|c| c.is_control()), "{m}");
        assert!(m.len() <= "http proxy answered 403 ".len() + 64, "{m}");

        let err = check_status(b"HTTP/1.1 407 \r\n\r\n")
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&err, OutboundError::Proxy(m) if m == "http proxy answered 407"),
            "{err}"
        );
    }
}
