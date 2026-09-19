//! A scriptable SOCKS5 server (RFC 1928 / 1929), CONNECT only.

use super::{AbortOnDrop, TlsFixture};
use rurge_net::connector::BoxedStream;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedSocks5 {
    pub methods: Vec<u8>,
    pub credentials: Option<(String, String)>,
    pub atyp: u8,
    /// The address as text: an IP literal or the domain name.
    pub host: String,
    pub port: u16,
}

pub struct FakeSocks5 {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedSocks5>>>,
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

async fn serve(
    mut stream: BoxedStream,
    script: Socks5Script,
    requests: Arc<Mutex<Vec<RecordedSocks5>>>,
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
        atyp,
        host,
        port,
    });
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
        let log = requests.clone();
        let tls = tls.map(|(fixture, require)| {
            let acceptor = fixture.acceptor(require);
            (fixture, acceptor)
        });
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (script, log, tls) = (script.clone(), log.clone(), tls.clone());
                tokio::spawn(async move {
                    let stream: BoxedStream = match &tls {
                        Some((fixture, acceptor)) => match fixture.accept(acceptor, tcp).await {
                            Ok(s) => s,
                            Err(_) => return,
                        },
                        None => Box::new(tcp),
                    };
                    let _ = serve(stream, script, log).await;
                });
            }
        });
        FakeSocks5 {
            addr,
            requests,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Every CONNECT request seen so far, in arrival order.
    pub fn requests(&self) -> Vec<RecordedSocks5> {
        self.requests.lock().expect("requests").clone()
    }
}
