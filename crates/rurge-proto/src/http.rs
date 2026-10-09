//! `http` / `https` proxy outbound: CONNECT tunnels, and absolute-form
//! forwarding of plain HTTP requests (manual: Policies › HTTP and HTTP/2).

use crate::build::{shadow_tls_client, tls_client};
use crate::outbound::untrusted_text;
use crate::transport::Stack;
use crate::transport::head::{is_too_long, read_head};
use crate::transport::prefixed;
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
    stack: Stack,
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

/// The headers of one request: every `<random-string(..)>` drawn anew.
pub(crate) fn render(templates: &[HeaderTemplate]) -> Vec<(String, String)> {
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
pub(crate) fn merge(base: &mut Vec<(String, String)>, custom: Vec<(String, String)>) {
    for (name, value) in custom {
        base.retain(|(existing, _)| !existing.eq_ignore_ascii_case(&name));
        base.push((name, value));
    }
}

/// `Basic base64(user:password)` (RFC 9110 11.7.2, RFC 7617): the
/// `proxy-authorization` value of the CONNECT proxies over HTTP/2 and
/// HTTP/3 (`h2-connect`, `trust-tunnel`, `masque`), standard Base64 with
/// its padding.
pub fn basic_authorization(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}

/// The host text a request line or a `Host` header may carry for `target`:
/// an IP literal (IPv6 in brackets), or a domain in its ASCII form (an IDN
/// becomes its A-labels). `None` means refuse the request: nothing safe to
/// write exists for this target. A caller that builds a request line or a
/// `Host` header of its own must write exactly this string, never
/// `target.host` — see the `HttpForward` obligations.
pub fn wire_host(target: &Target) -> Option<String> {
    match &target.host {
        HostName::Ip(IpAddr::V6(v6)) => Some(format!("[{v6}]")),
        HostName::Ip(ip) => Some(ip.to_string()),
        HostName::Domain(name) => crate::hostname::to_ascii(name),
    }
}

/// `wire_host(target).is_some()`: whether `target` has a host text safe to
/// write into a request line and a `Host` header.
pub fn valid_target(target: &Target) -> bool {
    wire_host(target).is_some()
}

/// A connection the proxy closes during the CONNECT exchange is its refusal,
/// not an I/O fault of ours: depending on timing and platform it shows up as
/// an EOF, or as a reset, abort or broken pipe with no words of its own (an
/// operating-system error or a bare kind), on the write as well as the read.
/// A reset, abort or broken pipe that does carry words is a hop below (an
/// `underlying-proxy`) saying what went wrong, and is reported in them; an
/// EOF stays the proxy's whatever it carries, because the TLS layer of this
/// same policy and `read_exact` word theirs too.
fn handshake_io(e: io::Error) -> OutboundError {
    use io::ErrorKind::{BrokenPipe, ConnectionAborted, ConnectionReset, UnexpectedEof};
    let closed = match e.kind() {
        UnexpectedEof => true,
        ConnectionReset | ConnectionAborted | BrokenPipe => e.get_ref().is_none(),
        _ => false,
    };
    if closed {
        OutboundError::Proxy("http proxy closed the connection during CONNECT".to_string())
    } else {
        OutboundError::from(e)
    }
}

fn check_status(head: &[u8]) -> Result<(), OutboundError> {
    let text = String::from_utf8_lossy(head);
    let line = text.lines().next().unwrap_or_default();
    let mut parts = line.splitn(3, ' ');
    let (version, code) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    // the proxy's own text, so bounded and stripped of control characters
    // before it can reach the session log or the request log
    let reason = parts.next().unwrap_or("");
    let reason = untrusted_text(reason, 64);
    let reason = reason.trim();
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
        let shadow_tls = shadow_tls_client(
            spec.shadow_tls.as_ref(),
            http.tls.as_ref(),
            host,
            roots.clone(),
        )?;
        let tls = tls_client(http.tls.as_ref(), host, &[], keystore, roots)?;
        let authorization = http.username.as_ref().map(|user| {
            let password = http.password.as_ref().map_or("", |p| p.expose().as_str());
            let pair = format!("{}:{password}", user.expose());
            format!("Basic {}", STANDARD.encode(pair))
        });
        Ok(HttpOutbound {
            name: spec.name.clone(),
            stack: Stack::new(
                connector,
                Target::new(host.clone(), port),
                shadow_tls,
                tls,
                None,
            ),
            authorization,
            headers: http.headers.clone(),
            forward: !http.always_use_connect,
        })
    }

    /// TCP to the proxy, then Shadow TLS when the policy has it, then TLS for `https`.
    async fn dial(&self, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        self.stack.open(opts).await
    }

    /// The caller must have already checked `valid_target(target)` (via
    /// `wire_host`): this never re-checks, and a rejected target must never
    /// reach it.
    fn connect_request(&self, host: &str, port: u16) -> Vec<u8> {
        let authority = format!("{host}:{port}");
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
        // never dial or write a byte for a target we cannot safely quote
        let Some(host) = wire_host(target) else {
            return Err(OutboundError::Proxy(
                "the target host name is not valid for an HTTP proxy request".to_string(),
            ));
        };
        let mut stream = self.dial(opts).await?;
        stream
            .write_all(&self.connect_request(&host, target.port))
            .await
            .map_err(handshake_io)?;
        // the answer is waited for next: a hop below that buffers keeps the
        // request until it is flushed
        stream.flush().await.map_err(handshake_io)?;
        let (head, rest) = read_head(&mut stream, MAX_HEAD).await.map_err(|e| {
            if is_too_long(&e) {
                OutboundError::Proxy(format!(
                    "http proxy sent a response header larger than {MAX_HEAD} bytes"
                ))
            } else {
                handshake_io(e)
            }
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
    use crate::testing::held::{Breaks, Gone, HoldingConnector, Stage};
    use crate::testing::{
        Camouflage, FakeHttpProxy, FakeShadowTls, HttpProxyScript, ShadowTlsScript, TlsFixture,
        echo_server,
    };
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::{NameKind, SpecEnv, to_spec};
    use rurge_config::{KeystoreItem, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn spec(definition: &str) -> PolicySpec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("Up", definition, &span).unwrap();
        let lookup = |_: &str| -> Option<NameKind> { None };
        let outcome = to_spec(
            &policy,
            &SpecEnv {
                keystore: &[],
                wireguard: &[],
                lookup: &lookup,
            },
        );
        outcome
            .spec
            .unwrap_or_else(|| panic!("{:?}", outcome.diagnostics))
    }

    fn outbound(definition: &str, roots: Arc<RootCertStore>) -> HttpOutbound {
        outbound_through(
            definition,
            roots,
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
    }

    fn outbound_through(
        definition: &str,
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> HttpOutbound {
        let keystore: Vec<KeystoreItem> = Vec::new();
        HttpOutbound::from_spec(&spec(definition), &keystore, roots, connector).unwrap()
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
    async fn connect_tunnels_through_shadow_tls() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let echo = echo_server().await;
            let proxy = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
            let fixture = TlsFixture::new(&["site.test"]);
            let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 2).await;
            let front = FakeShadowTls::spawn(ShadowTlsScript::new(
                rurge_config::spec::ShadowTlsVersion::V2,
                "pw",
                site.addr(),
                proxy.addr(),
            ))
            .await;
            let out = outbound(
                &format!(
                    "http, 127.0.0.1, {}, shadow-tls-password=pw, shadow-tls-sni=site.test",
                    front.addr().port()
                ),
                fixture.roots(),
            );
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"through the frames").await;
            assert_eq!(
                proxy.heads()[0].request_line,
                format!("CONNECT {echo} HTTP/1.1")
            );
            assert!(front.sessions()[0].authenticated);
            // plain requests in absolute form take the same road
            let forward = out.http_forward().expect("an http proxy forwards");
            let mut stream = forward.connect(&ConnectOpts::default()).await.unwrap();
            stream
                .write_all(b"GET http://origin.test/ HTTP/1.1\r\nHost: origin.test\r\n\r\n")
                .await
                .unwrap();
            let mut answer = [0u8; 12];
            stream.read_exact(&mut answer).await.unwrap();
            assert!(answer.starts_with(b"HTTP/1.1 "), "{answer:?}");
            assert!(front.sessions()[1].authenticated);
        })
        .await
        .expect("bounded");
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
    async fn an_idn_target_is_sent_as_its_a_label() {
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
        let t = Target::new(HostName::Domain("bücher.example".into()), 443);
        let mut stream = out.connect_tcp(&t, &ConnectOpts::default()).await.unwrap();
        roundtrip(&mut stream, b"idn").await;
        let head = &proxy.heads()[0];
        assert_eq!(
            head.request_line,
            "CONNECT xn--bcher-kva.example:443 HTTP/1.1"
        );
        assert_eq!(head.header("Host"), Some("xn--bcher-kva.example:443"));
        assert!(valid_target(&t));
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

    /// The hop below may keep what it is given until it is flushed (an
    /// `underlying-proxy` that buffers): the request has to be on its way
    /// before its answer is waited for.
    #[tokio::test]
    async fn the_request_is_flushed_before_its_answer_is_awaited() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
        let out = outbound_through(
            &format!("http, 127.0.0.1, {}", proxy.addr().port()),
            no_roots(),
            HoldingConnector::new(),
        );
        let mut stream = tokio::time::timeout(
            Duration::from_secs(5),
            out.connect_tcp(&target(echo), &ConnectOpts::default()),
        )
        .await
        .expect("the request stayed behind in the hop below")
        .unwrap();
        assert_eq!(
            proxy.heads()[0].request_line,
            format!("CONNECT {echo} HTTP/1.1")
        );
        // the tunnel is that same hop: what goes through it is the relay's
        // to flush
        stream.write_all(b"through the hop").await.unwrap();
        stream.flush().await.unwrap();
        let mut back = [0u8; 15];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut back))
            .await
            .expect("the echo arrives")
            .unwrap();
        assert_eq!(&back, b"through the hop");
    }

    /// How a proxy ends a connection instead of answering it.
    enum Ends {
        /// Closes with the request lying unread.
        Unread,
        /// Reads the request, then resets.
        Reset,
        /// Closes in the middle of its answer.
        MidAnswer,
    }

    /// Takes one connection and ends it as `how` says.
    async fn a_proxy_that(how: Ends) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let Ok((mut tcp, _)) = listener.accept().await else {
                return;
            };
            match how {
                // dropped over what it never read
                Ends::Unread => {
                    let _ = tcp.readable().await;
                }
                Ends::Reset => {
                    let _ = read_head(&mut tcp, MAX_HEAD).await;
                    // closing resets the connection
                    let _ = tcp.set_zero_linger();
                }
                Ends::MidAnswer => {
                    let _ = read_head(&mut tcp, MAX_HEAD).await;
                    let _ = tcp.write_all(b"HTTP/1.1 200 Connection esta").await;
                    let _ = tcp.shutdown().await;
                }
            }
        });
        addr
    }

    /// A connection the proxy ends during the exchange is its refusal,
    /// whether the end shows as an EOF or as a reset.
    #[tokio::test]
    async fn a_connection_the_proxy_ends_during_connect_is_its_refusal() {
        let echo = echo_server().await;
        for (how, what) in [
            (Ends::Unread, "the request unread"),
            (Ends::Reset, "a reset"),
            (Ends::MidAnswer, "half an answer"),
        ] {
            let proxy = a_proxy_that(how).await;
            let out = outbound(&format!("http, 127.0.0.1, {}", proxy.port()), no_roots());
            let err = tokio::time::timeout(
                Duration::from_secs(5),
                out.connect_tcp(&target(echo), &ConnectOpts::default()),
            )
            .await
            .expect("the proxy ends the connection")
            .map(|_| ())
            .unwrap_err();
            assert!(
                matches!(&err, OutboundError::Proxy(m) if m == "http proxy closed the connection during CONNECT"),
                "{what}: {err}"
            );
        }
    }

    /// The same when the connection is gone before the request is out,
    /// whether the write or the flush is what finds out.
    #[tokio::test]
    async fn a_connection_gone_before_the_request_is_out_is_the_proxys_refusal() {
        for held in [false, true] {
            let out =
                outbound_through("http, 127.0.0.1, 8080", no_roots(), Arc::new(Gone { held }));
            let err = out
                .connect_tcp(
                    &Target::new(HostName::parse("192.0.2.1"), 443),
                    &ConnectOpts::default(),
                )
                .await
                .map(|_| ())
                .unwrap_err();
            assert!(
                matches!(&err, OutboundError::Proxy(m) if m == "http proxy closed the connection during CONNECT"),
                "held = {held}: {err}"
            );
        }
    }

    #[test]
    fn only_a_closed_connection_is_read_as_the_proxys_refusal() {
        use io::ErrorKind::{
            BrokenPipe, ConnectionAborted, ConnectionReset, PermissionDenied, TimedOut,
            UnexpectedEof,
        };
        for kind in [
            UnexpectedEof,
            ConnectionReset,
            ConnectionAborted,
            BrokenPipe,
        ] {
            let err = handshake_io(kind.into());
            assert!(
                matches!(&err, OutboundError::Proxy(m) if m == "http proxy closed the connection during CONNECT"),
                "{kind:?}: {err}"
            );
        }
        // anything else is still what it was
        assert!(matches!(
            handshake_io(PermissionDenied.into()),
            OutboundError::Io(e) if e.kind() == PermissionDenied
        ));
        assert!(matches!(
            handshake_io(TimedOut.into()),
            OutboundError::Timeout
        ));
    }

    /// What a policy below says when it knows what went wrong: a
    /// shadowsocks server that reset the connection without answering.
    const BELOW: &str =
        "ss: the server reset the connection without answering (wrong password or method?)";

    /// An `underlying-proxy` that fails in words of its own is not this
    /// proxy closing the connection: the words are what the user is told,
    /// wherever in the exchange they come.
    #[tokio::test]
    async fn a_hop_below_that_says_what_went_wrong_is_not_overruled() {
        for (stage, kind) in [
            (Stage::Read, io::ErrorKind::ConnectionReset),
            (Stage::Write, io::ErrorKind::BrokenPipe),
            (Stage::Flush, io::ErrorKind::ConnectionAborted),
        ] {
            let out = outbound_through(
                "http, 127.0.0.1, 8080",
                no_roots(),
                Arc::new(Breaks {
                    stage,
                    kind,
                    text: BELOW,
                }),
            );
            let err = out
                .connect_tcp(
                    &Target::new(HostName::parse("192.0.2.1"), 443),
                    &ConnectOpts::default(),
                )
                .await
                .map(|_| ())
                .unwrap_err();
            assert!(
                matches!(&err, OutboundError::Io(e) if e.kind() == kind),
                "{stage:?}: {err}"
            );
            assert_eq!(err.to_string(), BELOW, "{stage:?}");
        }
    }

    /// Words of their own exempt a reset, an abort and a broken pipe from
    /// being the proxy's refusal, and do not exempt an end.
    #[test]
    fn words_of_its_own_exempt_a_reset_but_not_an_end() {
        use io::ErrorKind::{BrokenPipe, ConnectionAborted, ConnectionReset, UnexpectedEof};
        for kind in [ConnectionReset, ConnectionAborted, BrokenPipe] {
            let err = handshake_io(io::Error::new(kind, BELOW));
            assert!(
                matches!(&err, OutboundError::Io(e) if e.kind() == kind),
                "{kind:?}: {err}"
            );
            assert_eq!(err.to_string(), BELOW, "{kind:?}");
        }
        // An end is the proxy's whatever it says: the TLS layer of this very
        // policy words its own (no close_notify), and so does `read_exact`.
        let err = handshake_io(io::Error::new(UnexpectedEof, "early eof"));
        assert!(
            matches!(&err, OutboundError::Proxy(m) if m == "http proxy closed the connection during CONNECT"),
            "{err}"
        );
    }

    /// `InvalidData` is what tokio-rustls reports for every TLS error, an
    /// alert from the proxy included: a TLS 1.3 handshake is complete on
    /// our side before the proxy has looked at the client certificate, so a
    /// proxy that wants one says so on the first read. Only `read_head`
    /// giving up on a head is a head that is too long.
    #[tokio::test]
    async fn an_error_of_the_stream_is_not_a_head_that_is_too_long() {
        let alert = "received fatal alert: CertificateRequired";
        let out = outbound_through(
            "http, 127.0.0.1, 8080",
            no_roots(),
            Arc::new(Breaks {
                stage: Stage::Read,
                kind: io::ErrorKind::InvalidData,
                text: alert,
            }),
        );
        let err = out
            .connect_tcp(
                &Target::new(HostName::parse("192.0.2.1"), 443),
                &ConnectOpts::default(),
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.to_string(), alert);
    }

    /// The same with the real thing: an `https` proxy that wants a client
    /// certificate, and a profile that has none.
    #[tokio::test]
    async fn a_proxy_that_wants_a_client_certificate_says_so() {
        let echo = echo_server().await;
        let fixture = TlsFixture::new(&["localhost"]);
        let proxy =
            FakeHttpProxy::spawn_tls(HttpProxyScript::default(), fixture.clone(), true).await;
        let out = outbound(
            &format!("https, localhost, {}", proxy.addr().port()),
            fixture.roots(),
        );
        let err = tokio::time::timeout(
            Duration::from_secs(5),
            out.connect_tcp(&target(echo), &ConnectOpts::default()),
        )
        .await
        .expect("the proxy answers")
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.to_string(), "received fatal alert: CertificateRequired");
        assert!(proxy.heads().is_empty(), "no request got through");
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

    #[test]
    fn wire_host_renders_ip_literals_a_labels_and_refuses_the_unconvertible() {
        assert_eq!(
            wire_host(&Target::new(HostName::parse("192.0.2.1"), 443)).as_deref(),
            Some("192.0.2.1")
        );
        assert_eq!(
            wire_host(&Target::new(HostName::parse("::1"), 443)).as_deref(),
            Some("[::1]")
        );
        assert_eq!(
            wire_host(&Target::new(HostName::Domain("bücher.example".into()), 443)).as_deref(),
            Some("xn--bcher-kva.example")
        );
        assert_eq!(
            wire_host(&Target::new(HostName::Domain("x@blocked.test".into()), 443)),
            None
        );
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
            "bü\r\ncher.example",
            "x@blocked.test",
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

        // a control character at the very end must not leave a trailing space
        let Err(OutboundError::Proxy(m)) = check_status(b"HTTP/1.1 403 Forbidden \x1b\r\n\r\n")
        else {
            panic!("expected a proxy error");
        };
        assert_eq!(m, "http proxy answered 403 Forbidden");

        // a double space after the status code must not survive either
        let Err(OutboundError::Proxy(m)) = check_status(b"HTTP/1.1 403  Forbidden\r\n\r\n") else {
            panic!("expected a proxy error");
        };
        assert_eq!(m, "http proxy answered 403 Forbidden");
    }

    #[test]
    fn basic_authorization_is_base64_of_user_colon_password() {
        // RFC 7617 2
        assert_eq!(
            basic_authorization("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }
}
