//! UDP over one byte stream (M5 design §7): `trojan`'s UDP ASSOCIATE and
//! AnyTLS's UDP over TCP (sing's `uot`, version 2, not in connect mode)
//! carry every datagram, each way, as its address, its length and the
//! payload. The request head goes out with the first datagram and names that
//! datagram's target, as the reference clients do; the server answers
//! nothing until then.

use crate::addr::{AddrError, parse_socks_addr, socks_addr};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, PacketSocket, Target};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::Mutex;

/// How a datagram is framed on the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Framing {
    /// `ATYP ADDR PORT LENGTH CRLF PAYLOAD` (trojan-gfw's protocol document).
    Trojan,
    /// `TYPE ADDR PORT LENGTH PAYLOAD`, the types numbered 0 = IPv4,
    /// 1 = IPv6, 2 = name (sing's `uot.AddrParser`); the head's own address
    /// keeps SOCKS5's numbers.
    Uot,
}

impl Framing {
    /// The protocol's name in error texts.
    fn label(self) -> &'static str {
        match self {
            Framing::Trojan => "trojan",
            Framing::Uot => "anytls",
        }
    }

    /// A datagram's address type on the wire, for SOCKS5's `atyp`.
    fn wire_type(self, atyp: u8) -> u8 {
        match (self, atyp) {
            (Framing::Uot, 1) => 0,
            (Framing::Uot, 4) => 1,
            (Framing::Uot, 3) => 2,
            _ => atyp,
        }
    }

    /// SOCKS5's `atyp` for a datagram's address type on the wire.
    fn socks_type(self, wire: u8) -> Option<u8> {
        match (self, wire) {
            (Framing::Trojan, 1 | 3 | 4) => Some(wire),
            (Framing::Uot, 0) => Some(1),
            (Framing::Uot, 1) => Some(4),
            (Framing::Uot, 2) => Some(3),
            _ => None,
        }
    }

    fn unsendable(self, e: AddrError) -> io::Error {
        let what = match e {
            AddrError::Unsendable => "the host name cannot be sent to the server",
            AddrError::TooLong => "the host name is longer than 255 bytes",
        };
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: {what}", self.label()),
        )
    }

    /// What follows the head's fixed part: the first datagram's target.
    fn head_tail(self, first: &Target, out: &mut Vec<u8>) -> io::Result<()> {
        out.extend(socks_addr(first).map_err(|e| self.unsendable(e))?);
        match self {
            Framing::Trojan => out.extend_from_slice(b"\r\n"),
            Framing::Uot => {}
        }
        Ok(())
    }

    /// Appends `payload` for `to`, framed.
    fn encode(self, to: &Target, payload: &[u8], out: &mut Vec<u8>) -> io::Result<()> {
        let len = u16::try_from(payload.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{}: a datagram longer than 65535 bytes", self.label()),
            )
        })?;
        let mut addr = socks_addr(to).map_err(|e| self.unsendable(e))?;
        addr[0] = self.wire_type(addr[0]);
        out.extend(addr);
        out.extend_from_slice(&len.to_be_bytes());
        match self {
            Framing::Trojan => out.extend_from_slice(b"\r\n"),
            Framing::Uot => {}
        }
        out.extend_from_slice(payload);
        Ok(())
    }

    /// Bytes between the length and the payload.
    fn gap(self) -> usize {
        match self {
            Framing::Trojan => 2,
            Framing::Uot => 0,
        }
    }
}

struct Writer {
    half: WriteHalf<BoxedStream>,
    /// The head's fixed part, until the first datagram takes it along.
    head: Option<Vec<u8>>,
}

/// A carrier on one stream to the server: every target through it (full
/// cone, as far as the server goes).
pub(crate) struct StreamUdp {
    framing: Framing,
    writer: Mutex<Writer>,
    reader: Mutex<ReadHalf<BoxedStream>>,
}

impl StreamUdp {
    /// `head` is the request head up to the target, which the first
    /// datagram supplies.
    pub(crate) fn new(stream: BoxedStream, framing: Framing, head: Vec<u8>) -> StreamUdp {
        let (reader, half) = tokio::io::split(stream);
        StreamUdp {
            framing,
            writer: Mutex::new(Writer {
                half,
                head: Some(head),
            }),
            reader: Mutex::new(reader),
        }
    }

    fn closed(&self) -> io::Error {
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "{}: the server closed the UDP connection",
                self.framing.label()
            ),
        )
    }
}

/// Fills `buf` from `reader`; the stream's end, even between datagrams, is
/// the carrier's end.
async fn fill(
    reader: &mut ReadHalf<BoxedStream>,
    buf: &mut [u8],
    closed: impl Fn() -> io::Error,
) -> io::Result<()> {
    match reader.read_exact(buf).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Err(closed()),
        Err(e) => Err(e),
    }
}

impl PacketSocket for StreamUdp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let mut writer = self.writer.lock().await;
            let mut out = Vec::with_capacity(buf.len() + 300);
            if let Some(head) = &writer.head {
                out.extend_from_slice(head);
                self.framing.head_tail(to, &mut out)?;
            }
            self.framing.encode(to, buf, &mut out)?;
            writer.head = None;
            writer.half.write_all(&out).await?;
            writer.half.flush().await
        })
    }

    /// A datagram longer than `buf` is skipped: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let mut reader = self.reader.lock().await;
            let closed = || self.closed();
            loop {
                // ATYP, then as much of the address as it says
                let mut addr = vec![0u8; 2];
                fill(&mut reader, &mut addr, closed).await?;
                let Some(atyp) = self.framing.socks_type(addr[0]) else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "{}: a datagram of an unknown address type",
                            self.framing.label()
                        ),
                    ));
                };
                addr[0] = atyp;
                let rest = match atyp {
                    1 => 4 - 1,
                    4 => 16 - 1,
                    _ => usize::from(addr[1]),
                };
                // the rest of the address, the port, the length and the gap
                let mut tail = vec![0u8; rest + 2 + 2 + self.framing.gap()];
                fill(&mut reader, &mut tail, closed).await?;
                addr.extend_from_slice(&tail[..rest + 2]);
                let len_at = rest + 2;
                let len = usize::from(u16::from_be_bytes([tail[len_at], tail[len_at + 1]]));
                let from = parse_socks_addr(&addr).map(|(from, _)| from);
                if len > buf.len() {
                    // read past what does not fit
                    let mut skip = vec![0u8; len];
                    fill(&mut reader, &mut skip, closed).await?;
                    continue;
                }
                fill(&mut reader, &mut buf[..len], closed).await?;
                // a source that is no host name cannot be answered: dropped
                if let Some(from) = from {
                    return Ok((len, from));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use std::time::Duration;
    use tokio::io::DuplexStream;

    fn carrier() -> (StreamUdp, DuplexStream) {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        (
            StreamUdp::new(Box::new(ours), Framing::Trojan, b"HEAD".to_vec()),
            theirs,
        )
    }

    async fn written(server: &mut DuplexStream, n: usize) -> Vec<u8> {
        let mut buf = vec![0u8; n];
        tokio::time::timeout(Duration::from_secs(5), server.read_exact(&mut buf))
            .await
            .expect("written within the bound")
            .unwrap();
        buf
    }

    #[tokio::test]
    async fn the_head_rides_with_the_first_datagram_and_names_its_target() {
        let (udp, mut server) = carrier();
        let one = Target::new(HostName::parse("1.2.3.4"), 53);
        udp.send_to(b"q", &one).await.unwrap();
        let expected: &[u8] =
            b"HEAD\x01\x01\x02\x03\x04\x00\x35\r\n\x01\x01\x02\x03\x04\x00\x35\x00\x01\r\nq";
        assert_eq!(written(&mut server, expected.len()).await, expected);
        // the head goes out once
        let name = Target::new(HostName::Domain("bücher.example".into()), 443);
        udp.send_to(b"xy", &name).await.unwrap();
        let mut expected = vec![3, 21];
        expected.extend_from_slice(b"xn--bcher-kva.example");
        expected.extend_from_slice(&[1, 187, 0, 2, b'\r', b'\n', b'x', b'y']);
        assert_eq!(written(&mut server, expected.len()).await, expected);
    }

    #[tokio::test]
    async fn datagrams_come_back_with_their_source() {
        let (udp, mut server) = carrier();
        // an IPv6 source, one that does not fit, then a name
        let mut wire = vec![4];
        wire.extend_from_slice(&[0; 15]);
        wire.push(1);
        wire.extend_from_slice(&[0, 7, 0, 3, b'\r', b'\n', b'a', b'b', b'c']);
        wire.extend_from_slice(&[1, 9, 9, 9, 9, 0, 9, 0, 100, b'\r', b'\n']);
        wire.extend_from_slice(&[0u8; 100]);
        wire.extend_from_slice(&[3, 6]);
        wire.extend_from_slice(b"s.test");
        wire.extend_from_slice(&[0, 80, 0, 1, b'\r', b'\n', b'z']);
        server.write_all(&wire).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, from) = udp.recv_from(&mut buf).await.unwrap();
        assert_eq!(
            (&buf[..n], from),
            (&b"abc"[..], Target::new(HostName::parse("::1"), 7))
        );
        let (n, from) = udp.recv_from(&mut buf).await.unwrap();
        assert_eq!(
            (&buf[..n], from),
            (&b"z"[..], Target::new(HostName::parse("s.test"), 80)),
            "the 100-byte datagram did not fit and was skipped"
        );
    }

    #[tokio::test]
    async fn the_servers_end_is_the_carriers_end() {
        let (udp, server) = carrier();
        drop(server);
        let mut buf = [0u8; 64];
        let err = udp.recv_from(&mut buf).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "trojan: the server closed the UDP connection"
        );
    }

    #[tokio::test]
    async fn an_unsendable_name_is_refused_and_the_head_waits() {
        let (udp, mut server) = carrier();
        let err = udp
            .send_to(b"x", &Target::new(HostName::Domain("a@b.test".into()), 53))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "trojan: the host name cannot be sent to the server"
        );
        udp.send_to(b"x", &Target::new(HostName::parse("1.2.3.4"), 53))
            .await
            .unwrap();
        assert_eq!(&written(&mut server, 4).await, b"HEAD");
    }

    /// UDP over TCP numbers a datagram's address types its own way (0 / 1 /
    /// 2) and has no CRLF; the head's address keeps SOCKS5's numbers.
    #[tokio::test]
    async fn udp_over_tcp_has_its_own_address_types() {
        let (ours, mut server) = tokio::io::duplex(1 << 16);
        let udp = StreamUdp::new(Box::new(ours), Framing::Uot, vec![0]);
        udp.send_to(b"q", &Target::new(HostName::parse("1.2.3.4"), 53))
            .await
            .unwrap();
        let expected: &[u8] = &[
            0, 1, 1, 2, 3, 4, 0, 53, // not connect mode, the first target
            0, 1, 2, 3, 4, 0, 53, 0, 1, b'q', // the datagram
        ];
        assert_eq!(written(&mut server, expected.len()).await, expected);
        let mut wire = vec![2, 6];
        wire.extend_from_slice(b"s.test");
        wire.extend_from_slice(&[0, 80, 0, 1, b'z', 1]);
        wire.extend_from_slice(&[0; 15]);
        wire.extend_from_slice(&[1, 0, 7, 0, 2, b'a', b'b']);
        server.write_all(&wire).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, from) = udp.recv_from(&mut buf).await.unwrap();
        assert_eq!(
            (&buf[..n], from),
            (&b"z"[..], Target::new(HostName::parse("s.test"), 80))
        );
        let (n, from) = udp.recv_from(&mut buf).await.unwrap();
        assert_eq!(
            (&buf[..n], from),
            (&b"ab"[..], Target::new(HostName::parse("::1"), 7))
        );
        // SOCKS5's 3 is no type of its own
        server.write_all(&[3, 0]).await.unwrap();
        let err = udp.recv_from(&mut buf).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "anytls: a datagram of an unknown address type"
        );
    }
}
