//! `socks5` / `socks5-tls` proxy outbound (RFC 1928, RFC 1929): CONNECT and UDP ASSOCIATE.

use crate::build::{shadow_tls_client, tls_client};
use crate::task::AbortOnDrop;
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
use rurge_config::spec::{PolicySpec, ProtoSpec};
use rurge_config::{HostName, KeystoreItem};
use rurge_net::BoxFuture;
use rurge_net::connector::{
    BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, PacketSocket, Target,
};
use rustls::RootCertStore;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const VERSION: u8 = 5;
const NO_AUTH: u8 = 0;
const USER_PASS: u8 = 2;
const NO_ACCEPTABLE: u8 = 0xff;
/// RFC 1929: the user name and the password are each length-prefixed by one byte.
const MAX_CREDENTIAL: usize = 255;

/// `UDP ASSOCIATE` with no address of its own: the relay takes datagrams
/// from wherever the association's first one comes from (RFC 1928 §7).
const UDP_ASSOCIATE: [u8; 10] = [VERSION, 3, 0, 1, 0, 0, 0, 0, 0, 0];

pub struct Socks5Outbound {
    name: String,
    stack: Stack,
    credentials: Option<(String, String)>,
    /// `udp-relay`: the server takes `UDP ASSOCIATE` (the manual: it must
    /// be switched on, many servers do not).
    udp_relay: bool,
    /// The server as written, for a relay that answers with the
    /// unspecified address.
    server: Target,
    /// Where the relayed datagrams leave from: the same way the control
    /// connection goes (DIRECT, or `underlying-proxy`).
    connector: Arc<dyn Connector>,
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

/// The CONNECT request for `target`, built before any connection is opened.
pub(crate) fn connect_request(target: &Target) -> Result<Vec<u8>, OutboundError> {
    let mut request = vec![VERSION, 1, 0];
    request.extend(crate::addr::socks_addr(target).map_err(|e| match e {
        // the proxy resolves the name (remote resolution); an IDN goes out as A-labels
        crate::addr::AddrError::Unsendable => {
            proxy("the host name cannot be sent to a SOCKS5 proxy")
        }
        crate::addr::AddrError::TooLong => proxy("the host name is longer than 255 bytes"),
    })?);
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
            .is_some_and(|u| u.expose().len() > MAX_CREDENTIAL)
            || socks
                .password
                .as_ref()
                .is_some_and(|p| p.expose().len() > MAX_CREDENTIAL)
        {
            return Err(BuildError::new(format!(
                "policy `{}`: the socks5 user name and password must be at most 255 bytes each",
                spec.name
            )));
        }
        let shadow_tls = shadow_tls_client(
            spec.shadow_tls.as_ref(),
            socks.tls.as_ref(),
            host,
            roots.clone(),
        )?;
        let tls = tls_client(socks.tls.as_ref(), host, &[], keystore, roots)?;
        let credentials = socks.username.as_ref().map(|user| {
            let password = socks.password.as_ref().map(|p| p.expose().clone());
            (user.expose().clone(), password.unwrap_or_default())
        });
        Ok(Socks5Outbound {
            name: spec.name.clone(),
            stack: Stack::new(
                connector.clone(),
                Target::new(host.clone(), port),
                shadow_tls,
                tls,
                None,
            ),
            credentials,
            udp_relay: socks.udp_relay,
            server: Target::new(host.clone(), port),
            connector,
        })
    }

    async fn associate(&self, opts: &ConnectOpts) -> Result<BoxedPacketSocket, OutboundError> {
        let control = self.stack.open(opts).await?;
        let (control, relay) =
            negotiate_bound(control, &UDP_ASSOCIATE, self.credentials.as_ref()).await?;
        let socket = self.connector.open_udp(opts).await?;
        Ok(Box::new(Socks5Udp::new(
            control,
            relay_of(relay, &self.server.host),
            socket,
        )))
    }

    async fn handshake(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        // checked first: no connection is opened for a request we cannot send
        let request = connect_request(target)?;
        let stream = self.stack.open(opts).await?;
        negotiate(stream, &request, self.credentials.as_ref()).await
    }
}

/// Where the relay said to send datagrams; a relay that answered with the
/// unspecified address listens where the control connection went, which
/// is `server` (the usual reading of RFC 1928 §6).
pub(crate) fn relay_of(bound: Target, server: &HostName) -> Target {
    match bound.host {
        HostName::Ip(ip) if ip.is_unspecified() => Target::new(server.clone(), bound.port),
        _ => bound,
    }
}

/// The header of a datagram to or from `target` (RFC 1928 §7): RSV RSV
/// FRAG, then the address.
fn udp_header(target: &Target) -> Result<Vec<u8>, OutboundError> {
    let mut out = vec![0, 0, 0];
    out.extend(crate::addr::socks_addr(target).map_err(|e| match e {
        crate::addr::AddrError::Unsendable => {
            proxy("the host name cannot be sent to a SOCKS5 proxy")
        }
        crate::addr::AddrError::TooLong => proxy("the host name is longer than 255 bytes"),
    })?);
    Ok(out)
}

/// A datagram's source and where its payload starts; `None` for one that
/// is fragmented (FRAG ≠ 0: never reassembled) or malformed.
fn parse_udp(datagram: &[u8]) -> Option<(Target, usize)> {
    if datagram.get(..3)? != [0, 0, 0] {
        return None;
    }
    let (from, len) = crate::addr::parse_socks_addr(&datagram[3..])?;
    Some((from, 3 + len))
}

/// A SOCKS5 UDP association (`socks5`, `socks5-tls`, `external`): the
/// control connection, held open and watched, and the relay's datagrams
/// through `socket`. The association ends when either side closes the
/// control connection (RFC 1928 §7): this carrier then fails.
pub(crate) struct Socks5Udp {
    relay: Target,
    /// The relay as the carrier resolves it, looked up on first receive.
    relay_ip: tokio::sync::OnceCell<Option<IpAddr>>,
    socket: BoxedPacketSocket,
    closed: CancellationToken,
    _control: AbortOnDrop,
}

impl Socks5Udp {
    pub(crate) fn new(
        mut control: BoxedStream,
        relay: Target,
        socket: BoxedPacketSocket,
    ) -> Socks5Udp {
        let closed = CancellationToken::new();
        let watch = closed.clone();
        // nothing more comes on the control connection: its end is the
        // association's end
        let task = tokio::spawn(async move {
            let mut sink = [0u8; 64];
            while matches!(control.read(&mut sink).await, Ok(n) if n > 0) {}
            watch.cancel();
        });
        Socks5Udp {
            relay,
            relay_ip: tokio::sync::OnceCell::new(),
            socket,
            closed,
            _control: AbortOnDrop(task),
        }
    }
}

/// Whether a datagram from `sender` may be the relay's: when the relay is
/// known by address its source must be that address (the port may differ);
/// a relay still known by name (a chained carrier) is not filtered.
fn from_relay(relay: Option<IpAddr>, sender: &Target) -> bool {
    match (relay, &sender.host) {
        (Some(relay), HostName::Ip(ip)) => relay == *ip,
        (Some(_), HostName::Domain(_)) => false,
        (None, _) => true,
    }
}

fn association_closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "socks5: the proxy closed the UDP association",
    )
}

impl PacketSocket for Socks5Udp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            if self.closed.is_cancelled() {
                return Err(association_closed());
            }
            let mut datagram = udp_header(to).map_err(|e| io::Error::other(e.to_string()))?;
            datagram.extend_from_slice(buf);
            self.socket.send_to(&datagram, &self.relay).await
        })
    }

    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            loop {
                let (n, sender) = tokio::select! {
                    _ = self.closed.cancelled() => return Err(association_closed()),
                    got = self.socket.recv_from(buf) => got?,
                };
                let relay_ip = self
                    .relay_ip
                    .get_or_init(|| async {
                        match self.socket.resolve(&self.relay).await {
                            Ok(Target {
                                host: HostName::Ip(ip),
                                ..
                            }) => Some(ip),
                            _ => None,
                        }
                    })
                    .await;
                // only the relay speaks on this association
                if !from_relay(*relay_ip, &sender) {
                    continue;
                }
                // what the relay cannot have meant (a fragment, garbage) is dropped
                if let Some((from, start)) = parse_udp(&buf[..n]) {
                    buf.copy_within(start..n, 0);
                    return Ok((n - start, from));
                }
            }
        })
    }
}

/// The SOCKS5 handshake on `stream`, a connection to the proxy: method
/// selection, the user name and password when there are any, then
/// `request` (from `connect_request`). The stream carries the tunnel after.
pub(crate) async fn negotiate(
    stream: BoxedStream,
    request: &[u8],
    credentials: Option<&(String, String)>,
) -> Result<BoxedStream, OutboundError> {
    // BND.ADDR is of no use to a tunnel: read past it, whatever it says
    exchange(stream, request, credentials)
        .await
        .map(|(stream, _)| stream)
}

/// `negotiate`, and the address the proxy's reply named (BND.ADDR,
/// BND.PORT): for `UDP ASSOCIATE`, where the datagrams go.
pub(crate) async fn negotiate_bound(
    stream: BoxedStream,
    request: &[u8],
    credentials: Option<&(String, String)>,
) -> Result<(BoxedStream, Target), OutboundError> {
    let (stream, bound) = exchange(stream, request, credentials).await?;
    let (bound, _) = crate::addr::parse_socks_addr(&bound)
        .ok_or_else(|| proxy("the reply names an address that is no host name"))?;
    Ok((stream, bound))
}

/// The handshake itself; the reply's `ATYP ADDR PORT` comes back as read.
async fn exchange(
    mut stream: BoxedStream,
    request: &[u8],
    credentials: Option<&(String, String)>,
) -> Result<(BoxedStream, Vec<u8>), OutboundError> {
    let offered: &[u8] = if credentials.is_some() {
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
    match (selected[1], credentials) {
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
    stream.write_all(request).await.map_err(handshake_io)?;
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).await.map_err(handshake_io)?;
    if reply[1] != 0 {
        return Err(proxy(reply_text(reply[1])));
    }
    let mut bound = vec![reply[3]];
    let remaining = match reply[3] {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await.map_err(handshake_io)?;
            bound.push(len[0]);
            usize::from(len[0]) + 2
        }
        other => return Err(proxy(format!("unknown address type {other} in the reply"))),
    };
    let start = bound.len();
    bound.resize(start + remaining, 0);
    stream
        .read_exact(&mut bound[start..])
        .await
        .map_err(handshake_io)?;
    Ok((stream, bound))
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

    fn udp(&self) -> UdpSupport {
        if self.udp_relay {
            UdpSupport::Native
        } else {
            UdpSupport::Unsupported
        }
    }

    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            if !self.udp_relay {
                return Err(OutboundError::Unsupported(
                    "UDP without `udp-relay=true`".to_string(),
                ));
            }
            match tokio::time::timeout(opts.timeout, self.associate(opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        Camouflage, FakeShadowTls, FakeSocks5, ShadowTlsScript, Socks5Script, TlsFixture,
        echo_server,
    };
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
                wireguard: &[],
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

    /// Answers every datagram with itself.
    async fn udp_echo() -> SocketAddr {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            while let Ok((n, from)) = socket.recv_from(&mut buf).await {
                let _ = socket.send_to(&buf[..n], from).await;
            }
        });
        addr
    }

    async fn udp_roundtrip(carrier: &dyn PacketSocket, to: SocketAddr, payload: &[u8]) {
        carrier.send_to(payload, &target(to)).await.unwrap();
        let mut buf = [0u8; 1500];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("an answer")
            .unwrap();
        assert_eq!((&buf[..n], from), (payload, target(to)));
    }

    /// `udp-relay=true`: datagrams go through the relay the proxy names,
    /// each with its address; the proxy's answers come back with theirs.
    #[tokio::test]
    async fn udp_goes_through_the_association() {
        let (one, two) = (udp_echo().await, udp_echo().await);
        let proxy = FakeSocks5::spawn(Socks5Script {
            auth: Some(("u".into(), "p".into())),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(
            &format!(
                "socks5, 127.0.0.1, {}, u, p, udp-relay=true",
                proxy.addr().port()
            ),
            no_roots(),
        );
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), one, b"to one").await;
        udp_roundtrip(carrier.as_ref(), two, b"to two").await;
        let seen = proxy.requests();
        assert_eq!((seen.len(), seen[0].command), (1, 3), "one association");
        assert_eq!(proxy.datagrams(), [target(one), target(two)]);
    }

    /// A relay that answers with the unspecified address listens where the
    /// control connection went.
    #[tokio::test]
    async fn an_unspecified_relay_address_means_the_server() {
        let echo = udp_echo().await;
        let proxy = FakeSocks5::spawn(Socks5Script {
            udp_unspecified: true,
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}, udp-relay=true", proxy.addr().port()),
            no_roots(),
        );
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"here").await;
        assert_eq!(
            relay_of(
                Target::new(HostName::parse("::"), 7),
                &HostName::parse("s.test")
            ),
            Target::new(HostName::parse("s.test"), 7)
        );
    }

    /// The association ends with its control connection.
    #[tokio::test]
    async fn a_closed_control_connection_ends_the_association() {
        let echo = udp_echo().await;
        let proxy = FakeSocks5::spawn(Socks5Script {
            udp_close_after: Some(1),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}, udp-relay=true", proxy.addr().port()),
            no_roots(),
        );
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"once").await;
        let mut buf = [0u8; 64];
        let err = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("bounded")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "socks5: the proxy closed the UDP association"
        );
        let err = carrier.send_to(b"late", &target(echo)).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }

    /// Without `udp-relay=true` the policy carries no UDP (the manual: it
    /// must be switched on), and nothing is asked of the server.
    #[tokio::test]
    async fn no_udp_without_udp_relay() {
        let proxy = FakeSocks5::spawn(Socks5Script::default()).await;
        let out = outbound(
            &format!("socks5, 127.0.0.1, {}", proxy.addr().port()),
            no_roots(),
        );
        assert_eq!(out.udp(), UdpSupport::Unsupported);
        let err = out.open_udp(&ConnectOpts::default()).await.err().unwrap();
        assert_eq!(
            err.to_string(),
            "policy protocol not implemented: UDP without `udp-relay=true`"
        );
        assert!(proxy.requests().is_empty());
    }

    #[test]
    fn only_the_relay_may_answer() {
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        let from = |s: &str, port| Target::new(HostName::parse(s), port);
        assert!(from_relay(ip("10.0.0.1"), &from("10.0.0.1", 1)));
        assert!(
            from_relay(ip("10.0.0.1"), &from("10.0.0.1", 9999)),
            "any port"
        );
        assert!(!from_relay(ip("10.0.0.1"), &from("10.0.0.2", 1)));
        assert!(!from_relay(ip("10.0.0.1"), &from("relay.test", 1)));
        assert!(
            from_relay(None, &from("10.0.0.2", 1)),
            "a named relay is not filtered"
        );
    }

    #[tokio::test]
    async fn connect_ignores_an_unreadable_bound_address() {
        let echo = echo_server().await;
        let mut bound = vec![3u8, 3, b'a', b' ', b'b'];
        bound.extend_from_slice(&[0, 53]);
        let server = FakeSocks5::spawn(Socks5Script {
            connect_to: Some(echo),
            reply_bound: Some(bound),
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
        roundtrip(&mut stream, b"still a tunnel").await;
    }

    #[test]
    fn fragments_and_garbage_are_not_datagrams() {
        let mut datagram = udp_header(&Target::new(HostName::parse("10.0.0.1"), 53)).unwrap();
        datagram.extend_from_slice(b"q");
        assert_eq!(
            parse_udp(&datagram),
            Some((
                Target::new(HostName::parse("10.0.0.1"), 53),
                datagram.len() - 1
            ))
        );
        datagram[2] = 1;
        assert_eq!(parse_udp(&datagram), None, "a fragment");
        assert_eq!(parse_udp(&[0, 0]), None);
    }

    #[tokio::test]
    async fn socks5_tls_runs_inside_shadow_tls() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let echo = echo_server().await;
            let fixture = TlsFixture::new(&["127.0.0.1", "site.test"]);
            let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 2).await;
            let proxy = FakeSocks5::spawn_tls(Socks5Script::default(), fixture.clone(), false).await;
            let front = FakeShadowTls::spawn(ShadowTlsScript::new(
                rurge_config::spec::ShadowTlsVersion::V3,
                "pw",
                site.addr(),
                proxy.addr(),
            ))
            .await;
            let out = outbound(
                &format!(
                    "socks5-tls, 127.0.0.1, {}, shadow-tls-password=pw, shadow-tls-version=3, shadow-tls-sni=site.test",
                    front.addr().port()
                ),
                fixture.roots(),
            );
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"three handshakes deep").await;
            assert_eq!(proxy.requests().len(), 1);
            assert!(front.sessions()[0].authenticated);
        })
        .await
        .expect("bounded");
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
    async fn an_idn_target_is_sent_as_its_a_label() {
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
        let t = Target::new(HostName::Domain("bücher.example".into()), 443);
        let mut stream = out.connect_tcp(&t, &ConnectOpts::default()).await.unwrap();
        roundtrip(&mut stream, b"idn").await;
        assert_eq!(server.requests()[0].host, "xn--bcher-kva.example");
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
        // an unconvertible name (not valid ASCII/IDN authority syntax) is
        // refused the same way, before any connection is attempted
        let before = server.requests().len();
        let unconvertible = Target::new(HostName::Domain("x@blocked.test".into()), 443);
        let err = out
            .connect_tcp(&unconvertible, &ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&err, OutboundError::Proxy(m) if m == "socks5: the host name cannot be sent to a SOCKS5 proxy"),
            "{err}"
        );
        assert_eq!(
            server.requests().len(),
            before,
            "nothing was sent to the proxy"
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
                socks.username = Some(long.clone().into());
            } else {
                socks.password = Some(long.clone().into());
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
