//! A scriptable SOCKS5 server (RFC 1928 / 1929): CONNECT and UDP ASSOCIATE.

use super::{AbortOnDrop, TlsFixture};
use rurge_config::HostName;
use rurge_net::connector::{BoxedStream, Target};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

#[derive(Clone, Debug, Default)]
pub struct Socks5Script {
    /// Require these credentials (user, password).
    pub auth: Option<(String, String)>,
    /// Reply code for the CONNECT request (0 = succeeded).
    pub reply: u8,
    /// The raw ATYP + BND.ADDR + BND.PORT bytes of the reply; `None` =
    /// `[1, 0,0,0,0, 0,0]` (IPv4, all-zero).
    pub reply_bound: Option<Vec<u8>>,
    /// Connect here whatever the client asked for (the fake never resolves names).
    pub connect_to: Option<SocketAddr>,
    /// Wait this long before replying to the request.
    pub delay: Duration,
    /// Close right after the method selection.
    pub hang_up_after_greeting: bool,
    /// Select this method even if the client did not offer it.
    pub force_method: Option<u8>,
    /// Answer `UDP ASSOCIATE` with the unspecified address (and the relay's
    /// port): the client is to send where the control connection went.
    pub udp_unspecified: bool,
    /// Close the control connection after relaying this many answers.
    pub udp_close_after: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedSocks5 {
    pub methods: Vec<u8>,
    pub credentials: Option<(String, String)>,
    /// 1 = CONNECT, 3 = UDP ASSOCIATE.
    pub command: u8,
    pub atyp: u8,
    /// The address as text: an IP literal or the domain name.
    pub host: String,
    pub port: u16,
}

pub struct FakeSocks5 {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedSocks5>>>,
    datagrams: Arc<Mutex<Vec<Target>>>,
    _task: AbortOnDrop,
}

async fn read_vec(stream: &mut BoxedStream, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

/// ATYP + BND.ADDR + BND.PORT of a default (IPv4, all-zero) reply.
const DEFAULT_BOUND: [u8; 7] = [1, 0, 0, 0, 0, 0, 0];

async fn reply(stream: &mut BoxedStream, code: u8, bound: &[u8]) -> io::Result<()> {
    let mut out = vec![5, code, 0];
    out.extend_from_slice(bound);
    stream.write_all(&out).await
}

/// A UDP association: datagrams from the client go out by their address
/// (names are not resolved: they are dropped), answers from anywhere come
/// back with the sender's address, until the control connection closes.
async fn relay_udp(
    mut control: BoxedStream,
    script: &Socks5Script,
    datagrams: Arc<Mutex<Vec<Target>>>,
) -> io::Result<()> {
    let relay = UdpSocket::bind("127.0.0.1:0").await?;
    let outside = UdpSocket::bind("127.0.0.1:0").await?;
    let port = relay.local_addr()?.port();
    let mut bound = vec![1];
    bound.extend_from_slice(&if script.udp_unspecified {
        [0, 0, 0, 0]
    } else {
        [127, 0, 0, 1]
    });
    bound.extend_from_slice(&port.to_be_bytes());
    reply(&mut control, 0, &bound).await?;
    let (mut client, mut answered) = (None, 0);
    let (mut up, mut down, mut sink) = ([0u8; 2048], [0u8; 2048], [0u8; 64]);
    loop {
        tokio::select! {
            read = control.read(&mut sink) => {
                if !matches!(read, Ok(n) if n > 0) {
                    return Ok(());
                }
            }
            got = relay.recv_from(&mut up) => {
                // Windows reports an ICMP "unreachable" on the next receive
                let Ok((n, from)) = got else { continue };
                client = Some(from);
                let Some((to, start)) = parse(&up[..n]) else { continue };
                datagrams.lock().expect("datagrams").push(to.clone());
                if let HostName::Ip(ip) = to.host {
                    let _ = outside.send_to(&up[start..n], SocketAddr::new(ip, to.port)).await;
                }
            }
            got = outside.recv_from(&mut down) => {
                let Ok((n, from)) = got else { continue };
                let Some(client) = client else { continue };
                let mut datagram = vec![0, 0, 0];
                datagram.extend(address(&from));
                datagram.extend_from_slice(&down[..n]);
                relay.send_to(&datagram, client).await?;
                answered += 1;
                if script.udp_close_after == Some(answered) {
                    return control.shutdown().await;
                }
            }
        }
    }
}

/// RSV RSV FRAG ATYP ADDR PORT, then the payload; only unfragmented
/// datagrams are taken.
fn parse(datagram: &[u8]) -> Option<(Target, usize)> {
    if datagram.get(..3)? != [0, 0, 0] {
        return None;
    }
    let (host, rest) = match *datagram.get(3)? {
        1 => {
            let b: [u8; 4] = datagram.get(4..8)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(b)), 8)
        }
        4 => {
            let b: [u8; 16] = datagram.get(4..20)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(b)), 20)
        }
        3 => {
            let len = usize::from(*datagram.get(4)?);
            let name = String::from_utf8_lossy(datagram.get(5..5 + len)?).into_owned();
            (HostName::parse(&name), 5 + len)
        }
        _ => return None,
    };
    let port = u16::from_be_bytes(datagram.get(rest..rest + 2)?.try_into().ok()?);
    Some((Target::new(host, port), rest + 2))
}

fn address(addr: &SocketAddr) -> Vec<u8> {
    let mut out = match addr.ip() {
        IpAddr::V4(v4) => [&[1][..], &v4.octets()].concat(),
        IpAddr::V6(v6) => [&[4][..], &v6.octets()].concat(),
    };
    out.extend_from_slice(&addr.port().to_be_bytes());
    out
}

async fn serve(
    mut stream: BoxedStream,
    script: Socks5Script,
    requests: Arc<Mutex<Vec<RecordedSocks5>>>,
    datagrams: Arc<Mutex<Vec<Target>>>,
) -> io::Result<()> {
    let greeting = read_vec(&mut stream, 2).await?;
    let methods = read_vec(&mut stream, usize::from(greeting[1])).await?;
    let wanted = if script.auth.is_some() { 2 } else { 0 };
    let method = script.force_method.unwrap_or(if methods.contains(&wanted) {
        wanted
    } else {
        0xff
    });
    stream.write_all(&[5, method]).await?;
    if method == 0xff || script.hang_up_after_greeting {
        return stream.shutdown().await;
    }
    let mut credentials = None;
    if method == 2 {
        let head = read_vec(&mut stream, 2).await?;
        let user = String::from_utf8_lossy(&read_vec(&mut stream, usize::from(head[1])).await?)
            .into_owned();
        let plen = read_vec(&mut stream, 1).await?[0];
        let password =
            String::from_utf8_lossy(&read_vec(&mut stream, usize::from(plen)).await?).into_owned();
        let ok = script.auth == Some((user.clone(), password.clone()));
        credentials = Some((user, password));
        stream.write_all(&[1, u8::from(!ok)]).await?;
        if !ok {
            return stream.shutdown().await;
        }
    }
    let request = read_vec(&mut stream, 4).await?;
    let atyp = request[3];
    let (host, literal) = match atyp {
        1 => {
            let b = read_vec(&mut stream, 4).await?;
            let ip = IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3]));
            (ip.to_string(), Some(ip))
        }
        4 => {
            let b: [u8; 16] = read_vec(&mut stream, 16)
                .await?
                .try_into()
                .expect("sixteen bytes");
            let ip = IpAddr::V6(Ipv6Addr::from(b));
            (ip.to_string(), Some(ip))
        }
        _ => {
            let len = read_vec(&mut stream, 1).await?[0];
            let name = read_vec(&mut stream, usize::from(len)).await?;
            (String::from_utf8_lossy(&name).into_owned(), None)
        }
    };
    let port_bytes = read_vec(&mut stream, 2).await?;
    let port = u16::from_be_bytes([port_bytes[0], port_bytes[1]]);
    requests.lock().expect("requests").push(RecordedSocks5 {
        methods,
        credentials,
        command: request[1],
        atyp,
        host,
        port,
    });
    if request[1] == 3 {
        return relay_udp(stream, &script, datagrams).await;
    }
    tokio::time::sleep(script.delay).await;
    let bound: &[u8] = script.reply_bound.as_deref().unwrap_or(&DEFAULT_BOUND);
    if script.reply != 0 {
        reply(&mut stream, script.reply, bound).await?;
        return stream.shutdown().await;
    }
    let target = script
        .connect_to
        .or_else(|| literal.map(|ip| SocketAddr::new(ip, port)));
    let Some(target) = target else {
        // a domain name and nowhere to send it: host unreachable
        reply(&mut stream, 4, bound).await?;
        return stream.shutdown().await;
    };
    let Ok(mut upstream) = TcpStream::connect(target).await else {
        reply(&mut stream, 5, bound).await?;
        return stream.shutdown().await;
    };
    reply(&mut stream, 0, bound).await?;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
    Ok(())
}

impl FakeSocks5 {
    pub async fn spawn(script: Socks5Script) -> FakeSocks5 {
        FakeSocks5::start(script, None).await
    }

    /// The same server behind TLS (`socks5-tls` policies).
    pub async fn spawn_tls(
        script: Socks5Script,
        fixture: Arc<TlsFixture>,
        require_client_cert: bool,
    ) -> FakeSocks5 {
        FakeSocks5::start(script, Some((fixture, require_client_cert))).await
    }

    async fn start(script: Socks5Script, tls: Option<(Arc<TlsFixture>, bool)>) -> FakeSocks5 {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let requests: Arc<Mutex<Vec<RecordedSocks5>>> = Arc::default();
        let datagrams: Arc<Mutex<Vec<Target>>> = Arc::default();
        let (log, udp_log) = (requests.clone(), datagrams.clone());
        let tls = tls.map(|(fixture, require)| {
            let acceptor = fixture.acceptor(require);
            (fixture, acceptor)
        });
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (script, log, udp_log, tls) =
                    (script.clone(), log.clone(), udp_log.clone(), tls.clone());
                tokio::spawn(async move {
                    let stream: BoxedStream = match &tls {
                        Some((fixture, acceptor)) => match fixture.accept(acceptor, tcp).await {
                            Ok(s) => s,
                            Err(_) => return,
                        },
                        None => Box::new(tcp),
                    };
                    let _ = serve(stream, script, log, udp_log).await;
                });
            }
        });
        FakeSocks5 {
            addr,
            requests,
            datagrams,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Every request (CONNECT, UDP ASSOCIATE) seen so far, in arrival order.
    pub fn requests(&self) -> Vec<RecordedSocks5> {
        self.requests.lock().expect("requests").clone()
    }

    /// Where every datagram relayed so far was addressed, in arrival order.
    pub fn datagrams(&self) -> Vec<Target> {
        self.datagrams.lock().expect("datagrams").clone()
    }
}
