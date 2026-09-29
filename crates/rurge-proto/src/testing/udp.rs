//! The server side of UDP over one stream, for the fakes (trojan's UDP
//! ASSOCIATE): every datagram the client frames leaves by one loopback UDP
//! socket per connection, and whatever reaches that socket goes back framed
//! with its source — full cone, as the reference servers do. Names are never
//! resolved: a datagram for one goes to `connect_to`, or nowhere.

use rurge_net::connector::BoxedStream;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;

/// How the client frames a datagram.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Wire {
    /// `ATYP ADDR PORT LENGTH CRLF PAYLOAD`
    Trojan,
}

/// What the fake saw of its UDP relays.
#[derive(Default)]
pub(crate) struct UdpSeen {
    /// Every datagram's target, `host:port`, the name as it was on the wire.
    pub(crate) targets: Mutex<Vec<String>>,
    /// Each relay's own socket, in the order they opened.
    pub(crate) outside: Mutex<Vec<SocketAddr>>,
}

/// One datagram: `(host, port, payload)`; `None` at the stream's end.
async fn read_datagram<R: AsyncRead + Unpin>(
    reader: &mut R,
    wire: Wire,
) -> io::Result<Option<(String, u16, Vec<u8>)>> {
    let mut atyp = [0u8; 1];
    if reader.read(&mut atyp).await? == 0 {
        return Ok(None);
    }
    let host = match (wire, atyp[0]) {
        (Wire::Trojan, 1) => {
            let mut b = [0u8; 4];
            reader.read_exact(&mut b).await?;
            IpAddr::V4(Ipv4Addr::from(b)).to_string()
        }
        (Wire::Trojan, 4) => {
            let mut b = [0u8; 16];
            reader.read_exact(&mut b).await?;
            IpAddr::V6(Ipv6Addr::from(b)).to_string()
        }
        (Wire::Trojan, 3) => {
            let len = usize::from(reader.read_u8().await?);
            let mut name = vec![0u8; len];
            reader.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "an unknown address type",
            ));
        }
    };
    let port = reader.read_u16().await?;
    let len = usize::from(reader.read_u16().await?);
    match wire {
        Wire::Trojan => {
            let mut crlf = [0u8; 2];
            reader.read_exact(&mut crlf).await?;
        }
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(Some((host, port, payload)))
}

/// `payload` from `from`, framed for the client.
fn frame(wire: Wire, from: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 24);
    match (wire, from.ip()) {
        (Wire::Trojan, IpAddr::V4(v4)) => {
            out.push(1);
            out.extend_from_slice(&v4.octets());
        }
        (Wire::Trojan, IpAddr::V6(v6)) => {
            out.push(4);
            out.extend_from_slice(&v6.octets());
        }
    }
    out.extend_from_slice(&from.port().to_be_bytes());
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    match wire {
        Wire::Trojan => out.extend_from_slice(b"\r\n"),
    }
    out.extend_from_slice(payload);
    out
}

/// Relays the client's datagrams on `stream` until either side ends.
pub(crate) async fn relay(
    stream: BoxedStream,
    wire: Wire,
    connect_to: Option<SocketAddr>,
    seen: Arc<UdpSeen>,
) -> io::Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:0").await?;
    seen.outside
        .lock()
        .expect("outside")
        .push(socket.local_addr()?);
    let (mut reader, mut writer) = tokio::io::split(stream);
    let up = async {
        while let Some((host, port, payload)) = read_datagram(&mut reader, wire).await? {
            seen.targets
                .lock()
                .expect("targets")
                .push(format!("{host}:{port}"));
            let to = match (host.parse::<IpAddr>(), connect_to) {
                (Ok(ip), _) => SocketAddr::new(ip, port),
                (Err(_), Some(addr)) => addr,
                // never resolves: a name without `connect_to` is a dead end
                (Err(_), None) => continue,
            };
            socket.send_to(&payload, to).await?;
        }
        Ok::<(), io::Error>(())
    };
    let down = async {
        let mut buf = vec![0u8; 65536];
        loop {
            let (n, from) = match socket.recv_from(&mut buf).await {
                Ok(got) => got,
                // an ICMP "unreachable" for an earlier datagram (Windows)
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(e) => return Err(e),
            };
            writer.write_all(&frame(wire, from, &buf[..n])).await?;
            writer.flush().await?;
        }
    };
    tokio::select! {
        done = up => done,
        done = down => done,
    }
}
