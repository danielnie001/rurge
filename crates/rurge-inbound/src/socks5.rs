//! SOCKS5 (RFC 1928) listener: no authentication, CONNECT (M3 design §6.3)
//! and UDP ASSOCIATE (phase 2 M5 design 5.1).

use crate::listener::{ListenerOpts, Running, bind, serve};
use crate::session::{DialError, Dialer, FailKind, SessionOutcome, UdpAdmission};
use crate::udp::Socks5UdpClient;
use rurge_config::HostName;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_proto::RejectKind;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio_util::sync::CancellationToken;

const VERSION: u8 = 0x05;
const METHOD_NONE: u8 = 0x00;
const METHOD_UNACCEPTABLE: u8 = 0xff;
const CMD_CONNECT: u8 = 0x01;
const CMD_UDP_ASSOCIATE: u8 = 0x03;
const ATYP_V4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_V6: u8 = 0x04;

pub const REP_SUCCESS: u8 = 0x00;
pub const REP_GENERAL_FAILURE: u8 = 0x01;
pub const REP_NOT_ALLOWED: u8 = 0x02;
pub const REP_HOST_UNREACHABLE: u8 = 0x04;
pub const REP_CONNECTION_REFUSED: u8 = 0x05;
pub const REP_COMMAND_NOT_SUPPORTED: u8 = 0x07;
pub const REP_ADDR_TYPE_NOT_SUPPORTED: u8 = 0x08;

pub struct Socks5Listener;

impl Socks5Listener {
    pub async fn bind(
        addr: SocketAddr,
        dialer: Arc<dyn Dialer>,
        opts: ListenerOpts,
        stop: CancellationToken,
    ) -> io::Result<Running> {
        let listener = bind(addr).await?;
        let local = listener.local_addr()?;
        let opts = Arc::new(opts);
        Ok(serve(
            listener,
            "socks5",
            opts.restrict_to_lan,
            stop,
            move |stream, peer| {
                let dialer = dialer.clone();
                let opts = opts.clone();
                async move {
                    if let Err(e) = handle(stream, peer, local, dialer, opts).await {
                        tracing::debug!(listener = "socks5", source = %peer, error = %e, "socks5 session ended with an error");
                    }
                }
            },
        ))
    }
}

fn reply(code: u8) -> [u8; 10] {
    [VERSION, code, 0x00, ATYP_V4, 0, 0, 0, 0, 0, 0]
}

/// A success reply naming `bound`.
fn reply_bound(bound: SocketAddr) -> Vec<u8> {
    let mut out = vec![VERSION, REP_SUCCESS, 0x00];
    match bound.ip() {
        IpAddr::V4(v4) => {
            out.push(ATYP_V4);
            out.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            out.push(ATYP_V6);
            out.extend_from_slice(&v6.octets());
        }
    }
    out.extend_from_slice(&bound.port().to_be_bytes());
    out
}

/// A request the listener serves: the command and its address.
struct Request {
    command: u8,
    host: HostName,
    port: u16,
}

async fn read_request(stream: &mut TcpStream) -> io::Result<Result<Request, u8>> {
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    if head[0] != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad socks version in request",
        ));
    }
    let host = match head[3] {
        ATYP_V4 => {
            let mut b = [0u8; 4];
            stream.read_exact(&mut b).await?;
            HostName::Ip(IpAddr::V4(Ipv4Addr::from(b)))
        }
        ATYP_DOMAIN => {
            let len = stream.read_u8().await? as usize;
            if len == 0 {
                // Nothing to dial; drain the port and answer instead of
                // resolving the empty name.
                stream.read_u16().await?;
                return Ok(Err(REP_GENERAL_FAILURE));
            }
            let mut b = vec![0u8; len];
            stream.read_exact(&mut b).await?;
            let s = String::from_utf8(b)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 domain"))?;
            // A name with a control character or whitespace is never a host
            // name. Refuse it here, once, instead of trusting every outbound
            // that writes names into a text protocol to catch it.
            match HostName::from_wire(&s) {
                Some(host) => host,
                None => {
                    // the length only: the name itself is unsanitized client text
                    tracing::debug!(
                        listener = "socks5",
                        len = s.len(),
                        "refused a host name with control characters or whitespace"
                    );
                    stream.read_u16().await?;
                    return Ok(Err(REP_GENERAL_FAILURE));
                }
            }
        }
        ATYP_V6 => {
            let mut b = [0u8; 16];
            stream.read_exact(&mut b).await?;
            HostName::Ip(IpAddr::V6(Ipv6Addr::from(b)))
        }
        _ => return Ok(Err(REP_ADDR_TYPE_NOT_SUPPORTED)),
    };
    let port = stream.read_u16().await?;
    if head[1] != CMD_CONNECT && head[1] != CMD_UDP_ASSOCIATE {
        return Ok(Err(REP_COMMAND_NOT_SUPPORTED));
    }
    Ok(Ok(Request {
        command: head[1],
        host,
        port,
    }))
}

/// Method negotiation plus the request. `Ok(None)` means the client was
/// already answered (unacceptable method) and the session is over.
async fn handshake(stream: &mut TcpStream) -> io::Result<Option<Result<Request, u8>>> {
    let mut hello = [0u8; 2];
    stream.read_exact(&mut hello).await?;
    if hello[0] != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad socks version",
        ));
    }
    let mut methods = vec![0u8; hello[1] as usize];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&METHOD_NONE) {
        stream.write_all(&[VERSION, METHOD_UNACCEPTABLE]).await?;
        return Ok(None);
    }
    stream.write_all(&[VERSION, METHOD_NONE]).await?;
    read_request(stream).await.map(Some)
}

async fn handle(
    mut stream: TcpStream,
    peer: SocketAddr,
    local: SocketAddr,
    dialer: Arc<dyn Dialer>,
    opts: Arc<ListenerOpts>,
) -> io::Result<()> {
    // A client that stalls mid-handshake must not hold the task forever;
    // dialing and relaying are deliberately outside the bound.
    let negotiated = tokio::time::timeout(opts.handshake_timeout, handshake(&mut stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "socks5 handshake timed out"))??;
    let Request {
        command,
        host,
        port,
    } = match negotiated {
        Some(Ok(request)) => request,
        Some(Err(code)) => {
            stream.write_all(&reply(code)).await?;
            return Ok(());
        }
        None => return Ok(()),
    };
    if command == CMD_UDP_ASSOCIATE {
        return associate(stream, peer, dialer, port).await;
    }
    let mut session = SessionInfo::tcp(host, port);
    session.src = peer;
    session.in_port = local.port();
    session.listener = ListenerKind::Socks5;
    session.transport = Transport::Tcp;

    match dialer.dial(session).await {
        Ok(dialed) => {
            if let Err(e) = stream.write_all(&reply(REP_SUCCESS)).await {
                // The client is gone before `relay` ever starts, so nothing
                // else will finish this handle; do it here or it stays
                // listed as active (and killable-but-dead) forever.
                dialed.handle.finish(SessionOutcome::Failed(format!(
                    "client went away before the reply: {e}"
                )));
                return Err(e);
            }
            dialer
                .relay(Box::new(stream), dialed.stream, dialed.handle)
                .await;
            Ok(())
        }
        Err(DialError::Reject {
            kind: RejectKind::Drop,
            ..
        }) => {
            tokio::time::sleep(opts.drop_hold).await;
            Ok(())
        }
        Err(DialError::Reject { .. }) => {
            stream.write_all(&reply(REP_NOT_ALLOWED)).await?;
            Ok(())
        }
        Err(DialError::Failed { kind, .. }) => {
            let code = match kind {
                FailKind::Dns => REP_HOST_UNREACHABLE,
                FailKind::Connect | FailKind::Timeout | FailKind::Other => REP_CONNECTION_REFUSED,
            };
            stream.write_all(&reply(code)).await?;
            Ok(())
        }
    }
}

/// UDP ASSOCIATE: a UDP port on the address the client reached this
/// listener at, served by the dialer until the control connection ends
/// (RFC 1928 §7). `declared_port` is the client's source port when it said
/// (0: learnt from its first datagram).
async fn associate(
    mut control: TcpStream,
    peer: SocketAddr,
    dialer: Arc<dyn Dialer>,
    declared_port: u16,
) -> io::Result<()> {
    match dialer.admit_udp() {
        UdpAdmission::Accepted => {}
        UdpAdmission::NotSupported => {
            return control.write_all(&reply(REP_COMMAND_NOT_SUPPORTED)).await;
        }
        UdpAdmission::Busy => return control.write_all(&reply(REP_GENERAL_FAILURE)).await,
    }
    let here = control.local_addr()?;
    let socket = match UdpSocket::bind(SocketAddr::new(here.ip(), 0)).await {
        Ok(socket) => socket,
        Err(e) => {
            control.write_all(&reply(REP_GENERAL_FAILURE)).await?;
            return Err(e);
        }
    };
    control
        .write_all(&reply_bound(socket.local_addr()?))
        .await?;
    let client = Arc::new(Socks5UdpClient::new(socket, peer.ip(), declared_port));
    let mut session = SessionInfo::udp(HostName::Ip(here.ip()), 0);
    session.src = peer;
    session.in_port = here.port();
    session.listener = ListenerKind::Socks5;
    let closed = CancellationToken::new();
    let serving = dialer.associate(client, session, closed.clone());
    tokio::pin!(serving);
    // whatever else the client writes on the control connection means nothing
    let mut sink = [0u8; 256];
    loop {
        tokio::select! {
            _ = &mut serving => return Ok(()),
            read = control.read(&mut sink) => {
                if !matches!(read, Ok(n) if n > 0) {
                    closed.cancel();
                    serving.await;
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::UdpClient;
    use crate::testing::{FakeDialer, echo_server};
    use rurge_net::BoxFuture;
    use rurge_net::connector::Target;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    async fn listener(drop_hold: Duration) -> (Running, Arc<FakeDialer>) {
        listener_with(drop_hold, Duration::from_secs(30)).await
    }

    async fn listener_with(
        drop_hold: Duration,
        handshake_timeout: Duration,
    ) -> (Running, Arc<FakeDialer>) {
        let echo = echo_server().await;
        let dialer = FakeDialer::new(echo, None);
        let opts = ListenerOpts {
            kind: ListenerKind::Socks5,
            drop_hold,
            handshake_timeout,
            ..ListenerOpts::default()
        };
        let running = Socks5Listener::bind(
            "127.0.0.1:0".parse().unwrap(),
            dialer.clone(),
            opts,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        (running, dialer)
    }

    async fn negotiate(addr: SocketAddr) -> TcpStream {
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(&[VERSION, 1, METHOD_NONE]).await.unwrap();
        let mut r = [0u8; 2];
        s.read_exact(&mut r).await.unwrap();
        assert_eq!(r, [VERSION, METHOD_NONE]);
        s
    }

    fn domain_request(host: &str, port: u16) -> Vec<u8> {
        let mut v = vec![VERSION, CMD_CONNECT, 0, ATYP_DOMAIN, host.len() as u8];
        v.extend_from_slice(host.as_bytes());
        v.extend_from_slice(&port.to_be_bytes());
        v
    }

    async fn read_reply(s: &mut TcpStream) -> [u8; 10] {
        let mut r = [0u8; 10];
        s.read_exact(&mut r).await.unwrap();
        r
    }

    #[tokio::test]
    async fn connect_by_domain_and_by_ipv4_relays_bytes() {
        let (running, dialer) = listener(Duration::from_secs(30)).await;
        let mut s = negotiate(running.local_addr).await;
        s.write_all(&domain_request("echo.test", 7)).await.unwrap();
        let r = read_reply(&mut s).await;
        assert_eq!((r[0], r[1]), (VERSION, REP_SUCCESS));
        s.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        drop(s);
        // IPv4 address type
        let mut s = negotiate(running.local_addr).await;
        let mut req = vec![VERSION, CMD_CONNECT, 0, ATYP_V4, 127, 0, 0, 1];
        req.extend_from_slice(&7u16.to_be_bytes());
        s.write_all(&req).await.unwrap();
        assert_eq!(read_reply(&mut s).await[1], REP_SUCCESS);
        s.write_all(b"x").await.unwrap();
        let mut one = [0u8; 1];
        s.read_exact(&mut one).await.unwrap();
        drop(s);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let sessions = dialer.sessions();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session().listener, ListenerKind::Socks5);
        assert_eq!(sessions[0].session().dst_host, HostName::parse("echo.test"));
        assert_eq!(sessions[0].session().in_port, running.local_addr.port());
        assert!(sessions[0].is_finished());
        assert_eq!(sessions[0].bytes(), (4, 4));
    }

    #[tokio::test]
    async fn rejects_failures_and_unsupported_commands_map_to_reply_codes() {
        let (running, _dialer) = listener(Duration::from_millis(200)).await;
        for (host, port, expected) in [
            ("reject.test", 443, REP_NOT_ALLOWED),
            ("reject.test", 445, REP_NOT_ALLOWED),
            ("reject.test", 446, REP_NOT_ALLOWED),
            ("dns.test", 80, REP_HOST_UNREACHABLE),
            ("fail.test", 80, REP_CONNECTION_REFUSED),
            ("slow.test", 80, REP_CONNECTION_REFUSED),
        ] {
            let mut s = negotiate(running.local_addr).await;
            s.write_all(&domain_request(host, port)).await.unwrap();
            let r = read_reply(&mut s).await;
            assert_eq!(r[1], expected, "{host}:{port}");
            let mut eof = [0u8; 1];
            assert_eq!(
                s.read(&mut eof).await.unwrap(),
                0,
                "connection closed after {host}"
            );
        }
        // REJECT-DROP: no reply until the hold expires, then closed
        let mut s = negotiate(running.local_addr).await;
        s.write_all(&domain_request("reject.test", 444))
            .await
            .unwrap();
        let mut buf = [0u8; 10];
        let started = std::time::Instant::now();
        let n = tokio::time::timeout(Duration::from_secs(2), s.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, 0, "drop must not answer");
        assert!(
            started.elapsed() >= Duration::from_millis(180),
            "{:?}",
            started.elapsed()
        );
        // BIND is not supported
        let mut s = negotiate(running.local_addr).await;
        let mut req = vec![VERSION, 0x02, 0, ATYP_DOMAIN, 9];
        req.extend_from_slice(b"echo.test");
        req.extend_from_slice(&7u16.to_be_bytes());
        s.write_all(&req).await.unwrap();
        assert_eq!(read_reply(&mut s).await[1], REP_COMMAND_NOT_SUPPORTED);
    }

    /// Echoes every datagram back as coming from where it went; records
    /// the association's session and whether it saw `closed`.
    struct UdpEcho {
        admission: UdpAdmission,
        session: std::sync::Mutex<Option<SessionInfo>>,
        ended: AtomicBool,
    }

    impl UdpEcho {
        fn new(admission: UdpAdmission) -> Arc<UdpEcho> {
            Arc::new(UdpEcho {
                admission,
                session: std::sync::Mutex::new(None),
                ended: AtomicBool::new(false),
            })
        }
    }

    impl Dialer for UdpEcho {
        fn dial<'a>(
            &'a self,
            _session: SessionInfo,
        ) -> BoxFuture<'a, Result<crate::session::Dialed, DialError>> {
            unreachable!("UDP only")
        }

        fn relay<'a>(
            &'a self,
            _client: rurge_net::connector::BoxedStream,
            _upstream: rurge_net::connector::BoxedStream,
            _handle: Arc<crate::session::SessionHandle>,
        ) -> BoxFuture<'a, ()> {
            unreachable!("UDP only")
        }

        fn admit_udp(&self) -> UdpAdmission {
            self.admission
        }

        fn associate<'a>(
            &'a self,
            client: Arc<dyn UdpClient>,
            session: SessionInfo,
            closed: CancellationToken,
        ) -> BoxFuture<'a, ()> {
            *self.session.lock().unwrap() = Some(session);
            Box::pin(async move {
                let mut buf = [0u8; 2048];
                loop {
                    tokio::select! {
                        _ = closed.cancelled() => break,
                        got = client.recv(&mut buf) => {
                            let Ok((n, to)) = got else { break };
                            let _ = client.send(&buf[..n], &to).await;
                        }
                    }
                }
                self.ended.store(true, Ordering::SeqCst);
            })
        }
    }

    async fn udp_listener(dialer: Arc<UdpEcho>) -> Running {
        Socks5Listener::bind(
            "127.0.0.1:0".parse().unwrap(),
            dialer,
            ListenerOpts {
                kind: ListenerKind::Socks5,
                ..ListenerOpts::default()
            },
            CancellationToken::new(),
        )
        .await
        .unwrap()
    }

    /// UDP ASSOCIATE with the client's source address; the reply names the
    /// association's port.
    async fn associate_from(addr: SocketAddr, source: SocketAddr) -> (TcpStream, SocketAddr) {
        let mut s = negotiate(addr).await;
        let SocketAddr::V4(v4) = source else {
            panic!("loopback is v4")
        };
        let mut req = vec![VERSION, CMD_UDP_ASSOCIATE, 0, ATYP_V4];
        req.extend_from_slice(&v4.ip().octets());
        req.extend_from_slice(&v4.port().to_be_bytes());
        s.write_all(&req).await.unwrap();
        let r = read_reply(&mut s).await;
        assert_eq!((r[1], r[3]), (REP_SUCCESS, ATYP_V4));
        let bound = SocketAddr::from((
            Ipv4Addr::new(r[4], r[5], r[6], r[7]),
            u16::from_be_bytes([r[8], r[9]]),
        ));
        (s, bound)
    }

    fn datagram(to: &Target, payload: &[u8]) -> Vec<u8> {
        let mut out = crate::udp::header(to).unwrap();
        out.extend_from_slice(payload);
        out
    }

    #[tokio::test]
    async fn udp_associate_carries_datagrams_both_ways() {
        let dialer = UdpEcho::new(UdpAdmission::Accepted);
        let running = udp_listener(dialer.clone()).await;
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (_control, relay) =
            associate_from(running.local_addr, client.local_addr().unwrap()).await;
        assert_eq!(relay.ip(), running.local_addr.ip());
        let to = Target::new(HostName::parse("game.test"), 27015);
        client
            .send_to(&datagram(&to, b"ping"), relay)
            .await
            .unwrap();
        let mut buf = [0u8; 256];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buf))
            .await
            .expect("an answer")
            .unwrap();
        assert_eq!(from, relay);
        assert_eq!(&buf[..n], &datagram(&to, b"ping")[..]);
        let session = dialer.session.lock().unwrap().clone().unwrap();
        assert_eq!(session.transport, Transport::Udp);
        assert_eq!(session.listener, ListenerKind::Socks5);
        assert_eq!(session.src.ip(), client.local_addr().unwrap().ip());
    }

    /// Only the client's own address may use the association; fragments
    /// are dropped.
    #[tokio::test]
    async fn strangers_and_fragments_are_ignored() {
        let dialer = UdpEcho::new(UdpAdmission::Accepted);
        let running = udp_listener(dialer).await;
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (_control, relay) =
            associate_from(running.local_addr, client.local_addr().unwrap()).await;
        let to = Target::new(HostName::parse("10.0.0.1"), 53);
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger.send_to(&datagram(&to, b"x"), relay).await.unwrap();
        let mut fragment = datagram(&to, b"y");
        fragment[2] = 1;
        client.send_to(&fragment, relay).await.unwrap();
        let mut buf = [0u8; 64];
        // a window to observe that nothing comes back
        for socket in [&stranger, &client] {
            assert!(
                tokio::time::timeout(Duration::from_millis(300), socket.recv_from(&mut buf))
                    .await
                    .is_err()
            );
        }
        client.send_to(&datagram(&to, b"z"), relay).await.unwrap();
        let (n, _) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buf))
            .await
            .expect("an answer")
            .unwrap();
        assert_eq!(&buf[..n], &datagram(&to, b"z")[..]);
    }

    /// The association ends with its control connection.
    #[tokio::test]
    async fn closing_the_control_connection_ends_the_association() {
        let dialer = UdpEcho::new(UdpAdmission::Accepted);
        let running = udp_listener(dialer.clone()).await;
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (control, _relay) =
            associate_from(running.local_addr, client.local_addr().unwrap()).await;
        drop(control);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !dialer.ended.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "the association lives on"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// A dialer without UDP answers "command not supported"; one at its
    /// limit "general failure".
    #[tokio::test]
    async fn udp_associate_can_be_refused() {
        for (admission, code) in [
            (UdpAdmission::NotSupported, REP_COMMAND_NOT_SUPPORTED),
            (UdpAdmission::Busy, REP_GENERAL_FAILURE),
        ] {
            let running = udp_listener(UdpEcho::new(admission)).await;
            let mut s = negotiate(running.local_addr).await;
            s.write_all(&[VERSION, CMD_UDP_ASSOCIATE, 0, ATYP_V4, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
            assert_eq!(read_reply(&mut s).await[1], code, "{admission:?}");
        }
    }

    /// An empty ATYP=0x03 name has nothing to dial: answer 0x01 and stop.
    #[tokio::test]
    async fn an_empty_domain_name_is_answered_without_dialing() {
        let (running, dialer) = listener(Duration::from_secs(30)).await;
        let mut s = negotiate(running.local_addr).await;
        let mut req = vec![VERSION, CMD_CONNECT, 0, ATYP_DOMAIN, 0];
        req.extend_from_slice(&80u16.to_be_bytes());
        s.write_all(&req).await.unwrap();
        assert_eq!(read_reply(&mut s).await[1], REP_GENERAL_FAILURE);
        assert!(dialer.sessions().is_empty(), "no dial for an empty name");
    }

    /// A name with a line break or a space is never a host name. Passing it on
    /// would make every text-protocol outbound responsible for it.
    #[tokio::test]
    async fn a_domain_name_with_control_characters_is_answered_without_dialing() {
        let (running, dialer) = listener(Duration::from_secs(30)).await;
        for name in ["echo.test\r\nX-Evil: 1", "echo .test", "echo.test\0"] {
            let mut s = negotiate(running.local_addr).await;
            s.write_all(&domain_request(name, 7)).await.unwrap();
            assert_eq!(read_reply(&mut s).await[1], REP_GENERAL_FAILURE, "{name:?}");
        }
        assert!(dialer.sessions().is_empty(), "nothing was dialled");
    }

    /// A client that connects and then stalls must be cut loose by
    /// `handshake_timeout`, not held until it disconnects.
    #[tokio::test]
    async fn stalled_handshakes_time_out() {
        let (running, _dialer) =
            listener_with(Duration::from_secs(30), Duration::from_millis(200)).await;
        for partial in [&[][..], &[VERSION][..]] {
            let mut s = TcpStream::connect(running.local_addr).await.unwrap();
            s.write_all(partial).await.unwrap();
            let mut buf = [0u8; 16];
            let n = tokio::time::timeout(Duration::from_secs(2), s.read(&mut buf))
                .await
                .unwrap_or_else(|_| panic!("{partial:?} was not timed out by the listener"))
                .unwrap();
            assert_eq!(n, 0, "a timed-out handshake gets no reply");
        }
    }

    #[tokio::test]
    async fn refuses_clients_that_require_authentication() {
        let (running, _dialer) = listener(Duration::from_secs(30)).await;
        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        s.write_all(&[VERSION, 1, 0x02]).await.unwrap(); // username/password only
        let mut r = [0u8; 2];
        s.read_exact(&mut r).await.unwrap();
        assert_eq!(r, [VERSION, METHOD_UNACCEPTABLE]);
        let mut eof = [0u8; 1];
        assert_eq!(s.read(&mut eof).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn stop_drains_in_flight_sessions_and_frees_the_address() {
        let (running, _dialer) = listener(Duration::from_secs(30)).await;
        let addr = running.local_addr;
        // an in-flight tunnel to the echo target
        let mut s = negotiate(addr).await;
        let mut req = vec![5, 1, 0, 3, 9];
        req.extend_from_slice(b"echo.test");
        req.extend_from_slice(&443u16.to_be_bytes());
        s.write_all(&req).await.unwrap();
        assert_eq!(read_reply(&mut s).await[1], REP_SUCCESS);
        s.write_all(b"before").await.unwrap();
        let mut buf = [0u8; 6];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"before");
        // stop accepting: the address frees up while the tunnel keeps relaying
        running.stop();
        tokio::time::timeout(Duration::from_secs(2), running.wait_closed())
            .await
            .expect("socket closed within 2 s");
        assert!(
            TcpStream::connect(addr).await.is_err(),
            "no new connections after stop"
        );
        s.write_all(b"after").await.unwrap();
        let mut buf = [0u8; 5];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"after", "in-flight session still relays after stop");
        // the session ends when the client closes; join then completes
        drop(s);
        tokio::time::timeout(Duration::from_secs(5), running.join())
            .await
            .expect("accept loop drained and exited within 5 s");
    }

    #[tokio::test]
    async fn ipv6_atyp_is_parsed_and_dialed() {
        let (running, dialer) = listener(Duration::from_secs(30)).await;
        let mut s = negotiate(running.local_addr).await;
        // CONNECT ::1 :443 via ATYP=0x04
        let mut req = vec![5, 1, 0, 4];
        req.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
        req.extend_from_slice(&443u16.to_be_bytes());
        s.write_all(&req).await.unwrap();
        // ::1 is not one of FakeDialer's mapped hosts → it fails, but the request
        // must have been parsed and a session recorded with that dst (poll)
        let _ = read_reply(&mut s).await;
        let want = rurge_config::HostName::Ip(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if dialer
                .sessions()
                .iter()
                .any(|h| h.session().dst_host == want)
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "IPv6 session recorded"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn unknown_atyp_is_answered_with_0x08() {
        let (running, _dialer) = listener(Duration::from_secs(30)).await;
        let mut s = negotiate(running.local_addr).await;
        s.write_all(&[5, 1, 0, 0x09]).await.unwrap(); // 0x09 is not a valid ATYP
        assert_eq!(read_reply(&mut s).await[1], REP_ADDR_TYPE_NOT_SUPPORTED);
    }
}
