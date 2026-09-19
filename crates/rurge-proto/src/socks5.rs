//! `socks5` / `socks5-tls` proxy outbound (RFC 1928, RFC 1929), CONNECT only.

use crate::build::tls_client;
use crate::transport::tls::TlsClient;
use crate::{BuildError, Outbound, OutboundError};
use rurge_config::spec::{PolicySpec, ProtoSpec};
use rurge_config::{HostName, KeystoreItem};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const VERSION: u8 = 5;
const NO_AUTH: u8 = 0;
const USER_PASS: u8 = 2;
const NO_ACCEPTABLE: u8 = 0xff;
/// RFC 1929: the user name and the password are each length-prefixed by one byte.
const MAX_CREDENTIAL: usize = 255;

pub struct Socks5Outbound {
    name: String,
    server: Target,
    connector: Arc<dyn Connector>,
    tls: Option<TlsClient>,
    credentials: Option<(String, String)>,
}

fn proxy(message: impl Into<String>) -> OutboundError {
    OutboundError::Proxy(format!("socks5: {}", message.into()))
}

/// A connection the proxy closes mid-handshake is its refusal, not an I/O
/// fault of ours. Depending on timing and platform the close shows up as an
/// EOF, a reset or a broken pipe.
fn handshake_io(e: io::Error) -> OutboundError {
    match e.kind() {
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe => {
            proxy("the proxy closed the connection during the handshake")
        }
        _ => OutboundError::from(e),
    }
}

fn reply_text(code: u8) -> String {
    match code {
        1 => "general failure".to_string(),
        2 => "connection not allowed by the ruleset".to_string(),
        3 => "network unreachable".to_string(),
        4 => "host unreachable".to_string(),
        5 => "connection refused".to_string(),
        6 => "TTL expired".to_string(),
        7 => "command not supported".to_string(),
        8 => "address type not supported".to_string(),
        other => format!("reply code {other}"),
    }
}

fn connect_request(target: &Target) -> Result<Vec<u8>, OutboundError> {
    let mut request = vec![VERSION, 1, 0];
    match &target.host {
        HostName::Ip(IpAddr::V4(v4)) => {
            request.push(1);
            request.extend_from_slice(&v4.octets());
        }
        HostName::Ip(IpAddr::V6(v6)) => {
            request.push(4);
            request.extend_from_slice(&v6.octets());
        }
        // the proxy resolves the name (remote resolution)
        HostName::Domain(name) => {
            let len = u8::try_from(name.len())
                .map_err(|_| proxy("the host name is longer than 255 bytes"))?;
            request.push(3);
            request.push(len);
            request.extend_from_slice(name.as_bytes());
        }
    }
    request.extend_from_slice(&target.port.to_be_bytes());
    Ok(request)
}

impl Socks5Outbound {
    pub fn from_spec(
        spec: &PolicySpec,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<Socks5Outbound, BuildError> {
        let (ProtoSpec::Socks5(socks), Some(host), Some(port)) =
            (&spec.proto, &spec.server, spec.port)
        else {
            return Err(BuildError::new(format!(
                "policy `{}` is not a socks5 / socks5-tls policy",
                spec.name
            )));
        };
        // `PolicySpec`'s fields are public, so a caller could hand us one
        // whose credentials never went through `rurge-config`'s own length
        // check: re-check here, so the `as u8` casts in the handshake below
        // never truncate silently.
        if socks
            .username
            .as_ref()
            .is_some_and(|u| u.len() > MAX_CREDENTIAL)
            || socks
                .password
                .as_ref()
                .is_some_and(|p| p.len() > MAX_CREDENTIAL)
        {
            return Err(BuildError::new(format!(
                "policy `{}`: the socks5 user name and password must be at most 255 bytes each",
                spec.name
            )));
        }
        let tls = tls_client(socks.tls.as_ref(), host, &[], keystore, roots)?;
        let credentials = socks
            .username
            .clone()
            .map(|user| (user, socks.password.clone().unwrap_or_default()));
        Ok(Socks5Outbound {
            name: spec.name.clone(),
            server: Target::new(host.clone(), port),
            connector,
            tls,
            credentials,
        })
    }

    async fn handshake(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        // checked first: no connection is opened for a request we cannot send
        let request = connect_request(target)?;
        let mut stream = self.connector.connect(&self.server, opts).await?;
        if let Some(tls) = &self.tls {
            stream = tls.wrap(stream).await.map_err(OutboundError::tls)?;
        }
        let offered: &[u8] = if self.credentials.is_some() {
            &[NO_AUTH, USER_PASS]
        } else {
            &[NO_AUTH]
        };
        let mut greeting = vec![VERSION, offered.len() as u8];
        greeting.extend_from_slice(offered);
        stream.write_all(&greeting).await.map_err(handshake_io)?;
        let mut selected = [0u8; 2];
        stream
            .read_exact(&mut selected)
            .await
            .map_err(handshake_io)?;
        match (selected[1], &self.credentials) {
            (NO_ACCEPTABLE, _) => {
                return Err(proxy(
                    "the proxy accepts none of the offered authentication methods",
                ));
            }
            (method, _) if !offered.contains(&method) => {
                return Err(proxy(format!(
                    "the proxy selected authentication method {method}, which was not offered"
                )));
            }
            (USER_PASS, Some((user, password))) => {
                // lengths were checked in from_spec (<= 255 bytes each)
                let mut auth = vec![1, user.len() as u8];
                auth.extend_from_slice(user.as_bytes());
                auth.push(password.len() as u8);
                auth.extend_from_slice(password.as_bytes());
                stream.write_all(&auth).await.map_err(handshake_io)?;
                let mut status = [0u8; 2];
                stream.read_exact(&mut status).await.map_err(handshake_io)?;
                if status[1] != 0 {
                    return Err(proxy("authentication failed"));
                }
            }
            _ => {}
        }
        stream.write_all(&request).await.map_err(handshake_io)?;
        let mut reply = [0u8; 4];
        stream.read_exact(&mut reply).await.map_err(handshake_io)?;
        if reply[1] != 0 {
            return Err(proxy(reply_text(reply[1])));
        }
        // skip the bound address
        let remaining = match reply[3] {
            1 => 4 + 2,
            4 => 16 + 2,
            3 => {
                let mut len = [0u8; 1];
                stream.read_exact(&mut len).await.map_err(handshake_io)?;
                usize::from(len[0]) + 2
            }
            other => return Err(proxy(format!("unknown address type {other} in the reply"))),
        };
        let mut bound = vec![0u8; remaining];
        stream.read_exact(&mut bound).await.map_err(handshake_io)?;
        Ok(stream)
    }
}

impl Outbound for Socks5Outbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.handshake(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeSocks5, Socks5Script, TlsFixture, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::{NameKind, SpecEnv, to_spec};
    use rurge_config::{HostName, Span};
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
                lookup: &lookup,
            },
        );
        outcome
            .spec
            .unwrap_or_else(|| panic!("{:?}", outcome.diagnostics))
    }

    fn outbound(definition: &str, roots: Arc<RootCertStore>) -> Socks5Outbound {
        let keystore: Vec<KeystoreItem> = Vec::new();
        Socks5Outbound::from_spec(
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
    async fn connects_without_and_with_credentials() {
        let echo = echo_server().await;
        let open = FakeSocks5::spawn(Socks5Script::default()).await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}", open.addr().port()),
            no_roots(),
        );
        assert_eq!(out.name(), "Up");
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"no auth").await;
        assert_eq!(
            open.requests()[0].methods,
            [0],
            "only `no authentication` is offered"
        );

        let guarded = FakeSocks5::spawn(Socks5Script {
            auth: Some(("user".into(), "pass".into())),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}, user, pass", guarded.addr().port()),
            no_roots(),
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"with auth").await;
        let seen = &guarded.requests()[0];
        assert_eq!(seen.methods, [0, 2]);
        assert_eq!(seen.credentials, Some(("user".into(), "pass".into())));
        assert_eq!((seen.atyp, seen.port), (1, echo.port()));
    }

    #[tokio::test]
    async fn names_are_resolved_by_the_proxy_and_ipv6_is_its_own_type() {
        let echo = echo_server().await;
        let server = FakeSocks5::spawn(Socks5Script {
            connect_to: Some(echo),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}", server.addr().port()),
            no_roots(),
        );
        for host in ["remote.example", "2001:db8::1"] {
            let mut stream = out
                .connect_tcp(
                    &Target::new(HostName::parse(host), 443),
                    &ConnectOpts::default(),
                )
                .await
                .unwrap();
            roundtrip(&mut stream, b"x").await;
        }
        let seen = server.requests();
        assert_eq!(
            (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
            (3, "remote.example", 443)
        );
        assert_eq!((seen[1].atyp, seen[1].host.as_str()), (4, "2001:db8::1"));
    }

    #[tokio::test]
    async fn refusals_become_proxy_errors() {
        let echo = echo_server().await;
        let cases: Vec<(Socks5Script, &str, &str)> = vec![
            (
                Socks5Script {
                    auth: Some(("u".into(), "p".into())),
                    ..Socks5Script::default()
                },
                "socks5, 127.0.0.1, {port}, u, wrong",
                "socks5: authentication failed",
            ),
            (
                Socks5Script {
                    auth: Some(("u".into(), "p".into())),
                    ..Socks5Script::default()
                },
                "socks5, 127.0.0.1, {port}",
                "socks5: the proxy accepts none of the offered authentication methods",
            ),
            (
                Socks5Script {
                    force_method: Some(2),
                    ..Socks5Script::default()
                },
                "socks5, 127.0.0.1, {port}",
                "socks5: the proxy selected authentication method 2, which was not offered",
            ),
            (
                Socks5Script {
                    reply: 5,
                    ..Socks5Script::default()
                },
                "socks5, 127.0.0.1, {port}",
                "socks5: connection refused",
            ),
            (
                Socks5Script {
                    reply: 9,
                    ..Socks5Script::default()
                },
                "socks5, 127.0.0.1, {port}",
                "socks5: reply code 9",
            ),
            (
                Socks5Script {
                    hang_up_after_greeting: true,
                    ..Socks5Script::default()
                },
                "socks5, 127.0.0.1, {port}",
                "socks5: the proxy closed the connection during the handshake",
            ),
        ];
        for (script, definition, expected) in cases {
            let server = FakeSocks5::spawn(script).await;
            let out = outbound(
                &definition.replace("{port}", &server.addr().port().to_string()),
                no_roots(),
            );
            let err = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .map(|_| ())
                .unwrap_err();
            assert!(
                matches!(&err, OutboundError::Proxy(m) if m == expected),
                "{definition}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn a_silent_proxy_times_out_and_long_names_are_refused_locally() {
        let echo = echo_server().await;
        let server = FakeSocks5::spawn(Socks5Script {
            delay: Duration::from_secs(30),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}", server.addr().port()),
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
        let long = Target::new(HostName::Domain("a".repeat(256)), 80);
        let err = out
            .connect_tcp(&long, &ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&err, OutboundError::Proxy(m) if m == "socks5: the host name is longer than 255 bytes"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn socks5_tls_wraps_the_session_in_tls() {
        let echo = echo_server().await;
        let fixture = TlsFixture::new(&["localhost"]);
        let server = FakeSocks5::spawn_tls(Socks5Script::default(), fixture.clone(), false).await;
        let out = outbound(
            &format!("socks5-tls, localhost, {}", server.addr().port()),
            fixture.roots(),
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"inside tls").await;
        assert_eq!(fixture.seen()[0].sni.as_deref(), Some("localhost"));
    }

    #[test]
    fn credentials_longer_than_255_bytes_are_refused_at_build_time() {
        // a valid spec built the normal way (parse + to_spec), then a field
        // mutated past what `rurge-config` itself would ever let through:
        // `from_spec` must not trust that `PolicySpec` came from there.
        let base = spec("socks5, 127.0.0.1, 1080");
        let keystore: Vec<KeystoreItem> = Vec::new();
        let long = "x".repeat(256);
        for set_username in [true, false] {
            let mut modified = base.clone();
            let ProtoSpec::Socks5(socks) = &mut modified.proto else {
                panic!("expected a socks5 spec");
            };
            if set_username {
                socks.username = Some(long.clone());
            } else {
                socks.password = Some(long.clone());
            }
            let err = Socks5Outbound::from_spec(
                &modified,
                &keystore,
                no_roots(),
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(
                err.message,
                "policy `Up`: the socks5 user name and password must be at most 255 bytes each"
            );
            assert!(!err.message.contains(&long), "{}", err.message);
        }
    }

    /// A raw SOCKS5 responder that claims a 255-byte ATYP=3 (domain) bound
    /// address in its CONNECT reply, sends only the first 10 of those bytes,
    /// then closes: proves the client reports a closed connection instead of
    /// hanging while it waits for bytes that will never arrive.
    async fn spawn_truncated_reply() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut greeting = [0u8; 2];
            if stream.read_exact(&mut greeting).await.is_err() {
                return;
            }
            let mut methods = vec![0u8; usize::from(greeting[1])];
            if stream.read_exact(&mut methods).await.is_err() {
                return;
            }
            if stream.write_all(&[5, 0]).await.is_err() {
                return;
            }
            // the client's CONNECT request for an IPv4 target: 4 header
            // bytes + 4 address bytes + 2 port bytes
            let mut request = [0u8; 10];
            if stream.read_exact(&mut request).await.is_err() {
                return;
            }
            let _ = stream.write_all(&[5, 0, 0, 3, 255]).await;
            let _ = stream.write_all(&[0u8; 10]).await;
            let _ = stream.shutdown().await;
        });
        addr
    }

    #[tokio::test]
    async fn the_bound_address_may_be_a_domain_or_ipv6_and_unknown_types_are_refused() {
        let echo = echo_server().await;

        // ATYP 3: a domain name in the bound address
        let mut domain_bound = vec![3u8, 11];
        domain_bound.extend_from_slice(b"proxy.local");
        domain_bound.extend_from_slice(&[0x1f, 0x90]);
        let server = FakeSocks5::spawn(Socks5Script {
            connect_to: Some(echo),
            reply_bound: Some(domain_bound),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}", server.addr().port()),
            no_roots(),
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"atyp 3").await;

        // ATYP 4: an IPv6 bound address
        let mut ipv6_bound = vec![4u8];
        ipv6_bound.extend_from_slice(&[0u8; 16]);
        ipv6_bound.extend_from_slice(&[0, 0]);
        let server = FakeSocks5::spawn(Socks5Script {
            connect_to: Some(echo),
            reply_bound: Some(ipv6_bound),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}", server.addr().port()),
            no_roots(),
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"atyp 4").await;

        // an unknown ATYP is refused
        let server = FakeSocks5::spawn(Socks5Script {
            connect_to: Some(echo),
            reply_bound: Some(vec![9]),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}", server.addr().port()),
            no_roots(),
        );
        let err = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&err, OutboundError::Proxy(m) if m == "socks5: unknown address type 9 in the reply"),
            "{err}"
        );

        // a reply that claims a 255-byte domain but supplies only 10 bytes,
        // then hangs up: no hang, a proper handshake error
        let addr = spawn_truncated_reply().await;
        let out = outbound(&format!("socks5, 127.0.0.1, {}", addr.port()), no_roots());
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
        assert!(
            matches!(&err, OutboundError::Proxy(m) if m == "socks5: the proxy closed the connection during the handshake"),
            "{err}"
        );
    }
}
