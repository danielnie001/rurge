//! The client's side of a SOCKS5 UDP association (RFC 1928 §7, phase 2 M5
//! design 5.1): one UDP port per association, datagrams taken only from the
//! client that asked for it.

use crate::session::UdpClient;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::Target;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;
use tokio::net::UdpSocket;

/// The association's port. Datagrams count only when they come from the
/// control connection's client address and, once known, its port: the one
/// the request declared, or else the first datagram's.
pub(crate) struct Socks5UdpClient {
    socket: UdpSocket,
    client_ip: IpAddr,
    client_port: OnceLock<u16>,
}

impl Socks5UdpClient {
    pub(crate) fn new(socket: UdpSocket, client_ip: IpAddr, declared_port: u16) -> Socks5UdpClient {
        let client_port = OnceLock::new();
        if declared_port != 0 {
            let _ = client_port.set(declared_port);
        }
        Socks5UdpClient {
            socket,
            client_ip: canonical(client_ip),
            client_port,
        }
    }

    fn is_client(&self, from: SocketAddr) -> bool {
        if canonical(from.ip()) != self.client_ip {
            return false;
        }
        *self.client_port.get_or_init(|| from.port()) == from.port()
    }
}

/// An IPv4 client may show up as `::ffff:a.b.c.d` on a dual-stack socket.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// RSV RSV FRAG ATYP ADDR PORT: where the datagram goes and where its
/// payload starts; `None` for a fragment (FRAG ≠ 0, never reassembled), an
/// unknown address type, or a name that is no host name.
pub(crate) fn parse_header(datagram: &[u8]) -> Option<(Target, usize)> {
    if datagram.get(..3)? != [0, 0, 0] {
        return None;
    }
    let (host, end) = match *datagram.get(3)? {
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
            let name = std::str::from_utf8(datagram.get(5..5 + len)?).ok()?;
            (HostName::from_wire(name)?, 5 + len)
        }
        _ => return None,
    };
    let port = u16::from_be_bytes(datagram.get(end..end + 2)?.try_into().ok()?);
    Some((Target::new(host, port), end + 2))
}

/// The header of a datagram to the client, as coming from `from`; `None`
/// for a name too long to write.
pub(crate) fn header(from: &Target) -> Option<Vec<u8>> {
    let mut out = vec![0, 0, 0];
    match &from.host {
        HostName::Ip(IpAddr::V4(v4)) => {
            out.push(1);
            out.extend_from_slice(&v4.octets());
        }
        HostName::Ip(IpAddr::V6(v6)) => {
            out.push(4);
            out.extend_from_slice(&v6.octets());
        }
        HostName::Domain(name) => {
            out.push(3);
            out.push(u8::try_from(name.len()).ok()?);
            out.extend_from_slice(name.as_bytes());
        }
    }
    out.extend_from_slice(&from.port.to_be_bytes());
    Some(out)
}

impl UdpClient for Socks5UdpClient {
    fn recv<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            loop {
                let (n, from) = match self.socket.recv_from(buf).await {
                    Ok(got) => got,
                    // an ICMP "unreachable" for an earlier answer to the
                    // client, which Windows reports on the next receive
                    Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                    Err(e) => return Err(e),
                };
                if !self.is_client(from) {
                    continue;
                }
                if let Some((to, start)) = parse_header(&buf[..n]) {
                    buf.copy_within(start..n, 0);
                    return Ok((n - start, to));
                }
            }
        })
    }

    fn send<'a>(&'a self, payload: &'a [u8], from: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // nothing has come from the client yet: nowhere to send to
            let (Some(port), Some(mut datagram)) = (self.client_port.get(), header(from)) else {
                return Ok(());
            };
            datagram.extend_from_slice(payload);
            self.socket
                .send_to(&datagram, SocketAddr::new(self.client_ip, *port))
                .await?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_reads_back_as_it_was_written() {
        for host in ["10.0.0.1", "2001:db8::1", "example.com"] {
            let target = Target::new(HostName::parse(host), 443);
            let mut datagram = header(&target).unwrap();
            let start = datagram.len();
            datagram.extend_from_slice(b"quic");
            assert_eq!(parse_header(&datagram), Some((target, start)));
        }
    }

    #[test]
    fn fragments_and_bad_names_are_no_datagrams() {
        let mut datagram = header(&Target::new(HostName::parse("10.0.0.1"), 53)).unwrap();
        datagram[2] = 1;
        assert_eq!(parse_header(&datagram), None, "a fragment");
        let mut bad = vec![0, 0, 0, 3, 3];
        bad.extend_from_slice(b"a b");
        bad.extend_from_slice(&53u16.to_be_bytes());
        assert_eq!(parse_header(&bad), None, "no host name");
        assert_eq!(parse_header(&[0, 0, 0, 1, 10]), None, "cut short");
        assert_eq!(parse_header(&[0, 0, 0, 9, 0, 0]), None, "unknown type");
    }

    #[test]
    fn an_ipv4_mapped_client_is_the_ipv4_client() {
        assert_eq!(
            canonical("::ffff:127.0.0.1".parse().unwrap()),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            canonical("::1".parse().unwrap()),
            "::1".parse::<IpAddr>().unwrap()
        );
    }
}
