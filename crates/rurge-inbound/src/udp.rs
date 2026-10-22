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
    /// Where the answers go: `client_ip`, mapped to IPv6 for an IPv4 client
    /// when the socket is an IPv6 one (a dual-stack listener's) — macOS and
    /// Windows send to no other form from it.
    reply_ip: IpAddr,
    client_port: OnceLock<u16>,
}

impl Socks5UdpClient {
    pub(crate) fn new(socket: UdpSocket, client_ip: IpAddr, declared_port: u16) -> Socks5UdpClient {
        let client_port = OnceLock::new();
        if declared_port != 0 {
            let _ = client_port.set(declared_port);
        }
        let client_ip = canonical(client_ip);
        let reply_ip = match (client_ip, socket.local_addr()) {
            (IpAddr::V4(v4), Ok(local)) if local.is_ipv6() => IpAddr::V6(v4.to_ipv6_mapped()),
            _ => client_ip,
        };
        Socks5UdpClient {
            socket,
            client_ip,
            reply_ip,
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
    Some((Target::new(host.canonical(), port), end + 2))
}

/// The header of a datagram to the client, as coming from `from` (an IPv4
/// address in IPv6 form as the IPv4 address it names); `None` for a name too
/// long to write.
pub(crate) fn header(from: &Target) -> Option<Vec<u8>> {
    let mut out = vec![0, 0, 0];
    match &from.host {
        HostName::Ip(ip) => match canonical(*ip) {
            IpAddr::V4(v4) => {
                out.push(1);
                out.extend_from_slice(&v4.octets());
            }
            IpAddr::V6(v6) => {
                out.push(4);
                out.extend_from_slice(&v6.octets());
            }
        },
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

    fn send<'a>(
        &'a self,
        payload: &'a [u8],
        from: &'a Target,
        _answers: Option<&'a Target>,
    ) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // nothing has come from the client yet: nowhere to send to
            let (Some(port), Some(mut datagram)) = (self.client_port.get(), header(from)) else {
                return Ok(());
            };
            datagram.extend_from_slice(payload);
            self.socket
                .send_to(&datagram, SocketAddr::new(self.reply_ip, *port))
                .await?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

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

    /// A target in IPv6 form (`::ffff:a.b.c.d`) is the IPv4 address it names
    /// — as an address or as a name —, and an answer from it comes back from
    /// that IPv4 address, whichever form the outbound reported it in.
    #[test]
    fn an_ipv4_mapped_address_is_the_ipv4_address_both_ways() {
        let v4 = Target::new(HostName::parse("10.0.0.1"), 53);
        let mut by_address = vec![0, 0, 0, 4];
        by_address.extend_from_slice(&"::ffff:10.0.0.1".parse::<Ipv6Addr>().unwrap().octets());
        by_address.extend_from_slice(&53u16.to_be_bytes());
        let mut by_name = vec![0, 0, 0, 3, 15];
        by_name.extend_from_slice(b"::ffff:10.0.0.1");
        by_name.extend_from_slice(&53u16.to_be_bytes());
        for datagram in [by_address, by_name] {
            assert_eq!(parse_header(&datagram), Some((v4.clone(), datagram.len())));
        }
        let mapped = Target::new(HostName::parse("::ffff:10.0.0.1"), 53);
        assert_eq!(header(&mapped), header(&v4));
        assert_eq!(header(&mapped).unwrap()[3], 1, "address type 1");
        // a real IPv6 address stays one
        let v6 = Target::new(HostName::parse("2001:db8::1"), 53);
        assert_eq!(header(&v6).unwrap()[3], 4);
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

    /// On a dual-stack listener the association's port is an IPv6 socket
    /// that takes IPv4 too: an IPv4 client's answers leave it to the
    /// client's IPv4-mapped address — the only form macOS sends to from an
    /// IPv6 socket.
    #[tokio::test]
    async fn an_ipv4_client_hears_back_through_a_dual_stack_port() {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        socket.set_only_v6(false).unwrap();
        socket.set_nonblocking(true).unwrap();
        socket
            .bind(&SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0)).into())
            .unwrap();
        let socket = UdpSocket::from_std(socket.into()).unwrap();
        let port = socket.local_addr().unwrap().port();
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let ours = Socks5UdpClient::new(socket, client.local_addr().unwrap().ip(), 0);
        let to = Target::new(HostName::parse("10.0.0.1"), 53);
        let mut datagram = header(&to).unwrap();
        datagram.extend_from_slice(b"ask");
        client
            .send_to(&datagram, ("127.0.0.1", port))
            .await
            .unwrap();
        let mut buf = [0u8; 64];
        let (n, asked) = ours.recv(&mut buf).await.unwrap();
        assert_eq!((&buf[..n], asked), (&b"ask"[..], to.clone()));
        ours.send(b"answer", &to, None).await.unwrap();
        let (n, _) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.recv_from(&mut buf),
        )
        .await
        .expect("the answer comes back")
        .unwrap();
        assert!(buf[..n].ends_with(b"answer"));
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
