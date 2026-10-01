//! UDP over a Snell connection (phase 2 M6 design 4.3): a fresh connection
//! of its own asks `01 06 00` (UDP, no client id) and the server answers at
//! once — `00`, or an error as for TCP. From then on every record carries
//! one datagram, each way:
//!
//! ```text
//! ours:   01 ‖ host-length host | 00 04 IPv4 | 00 06 IPv6 ‖ port ‖ payload
//! theirs: 04 IPv4 | 06 IPv6 ‖ port ‖ payload
//! ```
//!
//! Every target goes through the one connection and whoever answers the
//! server's socket is heard (full cone). The record boundary is the
//! datagram's, which is why this is not a `stream_udp` framing: that one
//! reads lengths off a byte stream, and Snell's datagrams carry none. A
//! datagram that does not fit one record (`MAX_PAYLOAD` with its address)
//! is refused; a record of the server's that is no datagram is dropped, as
//! Surge does. The connection's end is the carrier's.

use super::REQUEST_VERSION;
use super::record::{MAX_PAYLOAD, SnellStream};
use super::tunnel::{ERROR, TUNNEL, UNKNOWN_REPLY, no_answer, refused};
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{PacketSocket, Target};
use std::future::poll_fn;
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// The request's command.
pub(super) const UDP: u8 = 0x06;
/// A datagram's command, in front of each of ours.
const FORWARD: u8 = 0x01;
const IPV4: u8 = 0x04;
const IPV6: u8 = 0x06;

fn unsendable(text: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("snell: {text}"))
}

/// Our record for `payload` to `to`: a name as its A-labels, an IP after
/// the `00` marker.
fn datagram(to: &Target, payload: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = vec![FORWARD];
    match &to.host {
        HostName::Ip(IpAddr::V4(v4)) => {
            out.extend_from_slice(&[0, IPV4]);
            out.extend_from_slice(&v4.octets());
        }
        HostName::Ip(IpAddr::V6(v6)) => {
            out.extend_from_slice(&[0, IPV6]);
            out.extend_from_slice(&v6.octets());
        }
        HostName::Domain(name) => {
            let name = crate::hostname::to_ascii(name)
                .ok_or_else(|| unsendable("the host name cannot be sent to the server"))?;
            let len = u8::try_from(name.len())
                .map_err(|_| unsendable("the host name is longer than 255 bytes"))?;
            out.push(len);
            out.extend_from_slice(name.as_bytes());
        }
    }
    out.extend_from_slice(&to.port.to_be_bytes());
    let room = MAX_PAYLOAD - out.len();
    if payload.len() > room {
        return Err(unsendable(&format!("a datagram longer than {room} bytes")));
    }
    out.extend_from_slice(payload);
    Ok(out)
}

/// The source at the start of the server's record, and where its payload
/// starts; `Err` says why the record is no datagram.
fn source(record: &[u8]) -> Result<(Target, usize), &'static str> {
    const SHORT: &str = "a datagram cut short";
    let (ip, at) = match record.first() {
        Some(&IPV4) => {
            let b: [u8; 4] = record.get(1..5).ok_or(SHORT)?.try_into().expect("4 bytes");
            (IpAddr::from(b), 5)
        }
        Some(&IPV6) => {
            let b: [u8; 16] = record
                .get(1..17)
                .ok_or(SHORT)?
                .try_into()
                .expect("16 bytes");
            (IpAddr::from(b), 17)
        }
        _ => return Err("an unknown address family"),
    };
    let port = record.get(at..at + 2).ok_or(SHORT)?;
    let port = u16::from_be_bytes([port[0], port[1]]);
    Ok((Target::new(HostName::Ip(ip), port), at + 2))
}

/// The server's answer to the request: `Ok` with the rest of its record,
/// if any — the first datagram.
async fn answer(stream: &mut SnellStream) -> io::Result<Option<Vec<u8>>> {
    let mut got = Vec::new();
    loop {
        let Some(record) = poll_fn(|cx| stream.poll_record(cx)).await? else {
            return Err(no_answer());
        };
        got.extend_from_slice(&record);
        match got[0] {
            TUNNEL => return Ok((got.len() > 1).then(|| got.split_off(1))),
            // `code length message`, maybe over several records
            ERROR => {
                if let Some(&len) = got.get(2)
                    && got.len() >= 3 + usize::from(len)
                {
                    return Err(refused(&got[1..3 + usize::from(len)]));
                }
            }
            _ => {
                return Err(io::Error::new(io::ErrorKind::InvalidData, UNKNOWN_REPLY));
            }
        }
    }
}

/// A carrier on one Snell connection. No `Debug`: the stream holds the
/// connection's keys.
pub(crate) struct SnellUdp {
    /// Locked only while a send or a receive polls it, as `tokio::io::split`
    /// does: the two directions share the connection.
    stream: std::sync::Mutex<SnellStream>,
    /// One datagram at a time, so that each is one record.
    sending: tokio::sync::Mutex<()>,
    /// One receiver at a time, and the datagram that came with the answer.
    receiving: tokio::sync::Mutex<Option<Vec<u8>>>,
}

impl SnellUdp {
    /// Asks `stream` (a fresh connection) for UDP and waits for the answer.
    pub(super) async fn open(mut stream: SnellStream) -> io::Result<SnellUdp> {
        stream.write_all(&[REQUEST_VERSION, UDP, 0]).await?;
        stream.flush().await?;
        let first = answer(&mut stream).await?;
        Ok(SnellUdp {
            stream: std::sync::Mutex::new(stream),
            sending: tokio::sync::Mutex::new(()),
            receiving: tokio::sync::Mutex::new(first),
        })
    }

    fn with_stream<T>(&self, f: impl FnOnce(&mut SnellStream) -> T) -> T {
        f(&mut self.stream.lock().expect("stream"))
    }
}

fn closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "snell: the server closed the UDP connection",
    )
}

impl PacketSocket for SnellUdp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let record = datagram(to, buf)?;
            let _turn = self.sending.lock().await;
            // a record an abandoned send left parked goes out first: the
            // stream wants a write retried with the same bytes
            poll_fn(|cx| self.with_stream(|s| Pin::new(s).poll_flush(cx))).await?;
            // at most `MAX_PAYLOAD` bytes: one write, one record
            let written =
                poll_fn(|cx| self.with_stream(|s| Pin::new(s).poll_write(cx, &record))).await?;
            debug_assert_eq!(written, record.len(), "one record");
            poll_fn(|cx| self.with_stream(|s| Pin::new(s).poll_flush(cx))).await
        })
    }

    /// A datagram longer than `buf` is dropped: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let mut first = self.receiving.lock().await;
            loop {
                let record = match first.take() {
                    Some(record) => record,
                    None => poll_fn(|cx| self.with_stream(|s| s.poll_record(cx)))
                        .await?
                        .ok_or_else(closed)?,
                };
                match source(&record) {
                    Ok((from, at)) => {
                        let payload = &record[at..];
                        if let Some(space) = buf.get_mut(..payload.len()) {
                            space.copy_from_slice(payload);
                            return Ok((payload.len(), from));
                        }
                    }
                    Err(why) => {
                        tracing::debug!("snell: a UDP record from the server was dropped: {why}")
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::kdf::Psk;
    use super::*;
    use rurge_net::connector::BoxedStream;
    use std::time::Duration;

    fn to(host: &str, port: u16) -> Target {
        Target::new(HostName::parse(host), port)
    }

    #[test]
    fn our_datagrams_name_an_ip_after_a_marker_and_a_name_by_its_length() {
        assert_eq!(
            datagram(&to("1.2.3.4", 53), b"q").unwrap(),
            [1, 0, 4, 1, 2, 3, 4, 0, 53, b'q']
        );
        let mut v6 = vec![1, 0, 6];
        v6.extend_from_slice(&[0; 15]);
        v6.extend_from_slice(&[1, 0x01, 0xbb, b'x', b'y']);
        assert_eq!(datagram(&to("::1", 443), b"xy").unwrap(), v6);
        let name = Target::new(HostName::Domain("bücher.example".into()), 53);
        let mut expected = vec![1, 21];
        expected.extend_from_slice(b"xn--bcher-kva.example");
        expected.extend_from_slice(&[0, 53]);
        assert_eq!(datagram(&name, b"").unwrap(), expected, "an empty datagram");
        let err =
            datagram(&Target::new(HostName::Domain("a@b.test".into()), 53), b"x").unwrap_err();
        assert_eq!(
            err.to_string(),
            "snell: the host name cannot be sent to the server"
        );
    }

    #[test]
    fn a_datagram_fills_at_most_one_record() {
        // 9 bytes of command and IPv4 address
        let largest = vec![0u8; MAX_PAYLOAD - 9];
        assert_eq!(
            datagram(&to("1.2.3.4", 53), &largest).unwrap().len(),
            MAX_PAYLOAD
        );
        let err = datagram(&to("1.2.3.4", 53), &[0u8; MAX_PAYLOAD - 8]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(err.to_string(), "snell: a datagram longer than 16374 bytes");
    }

    #[test]
    fn the_servers_datagrams_start_with_their_source() {
        let record = [4, 9, 8, 7, 6, 0, 53, b'a', b'b'];
        assert_eq!(source(&record), Ok((to("9.8.7.6", 53), 7)));
        let mut v6 = vec![6];
        v6.extend_from_slice(&[0; 15]);
        v6.extend_from_slice(&[1, 0, 7]);
        assert_eq!(source(&v6), Ok((to("::1", 7), 19)), "an empty payload");
        assert_eq!(source(&[5, 1, 2]), Err("an unknown address family"));
        assert_eq!(
            source(&[0, 4, 1, 2, 3, 4, 0, 53]),
            Err("an unknown address family")
        );
        assert_eq!(source(&[4, 1, 2, 3, 4, 0]), Err("a datagram cut short"));
        assert_eq!(source(&v6[..18]), Err("a datagram cut short"));
    }

    /// Our stream and the server's, on the two ends of a pipe.
    async fn pair() -> (SnellStream, SnellStream) {
        let (near, far) = tokio::io::duplex(64 * 1024);
        let open = |end: tokio::io::DuplexStream| {
            SnellStream::open(Box::new(end) as BoxedStream, Psk::new("psk"))
        };
        (open(near).await.unwrap(), open(far).await.unwrap())
    }

    async fn next_record(server: &mut SnellStream) -> Option<Vec<u8>> {
        tokio::time::timeout(Duration::from_secs(5), poll_fn(|cx| server.poll_record(cx)))
            .await
            .expect("a record within the bound")
            .unwrap()
    }

    /// The carrier over `client`, the server answering `answers` (a record
    /// each) to the request.
    async fn opened(
        client: SnellStream,
        server: &mut SnellStream,
        answers: &[&[u8]],
    ) -> io::Result<SnellUdp> {
        let open = tokio::spawn(SnellUdp::open(client));
        assert_eq!(next_record(server).await.unwrap(), [1, 6, 0]);
        for answer in answers {
            server.write_all(answer).await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), open)
            .await
            .expect("opened within the bound")
            .unwrap()
    }

    async fn received(udp: &SnellUdp) -> io::Result<(Vec<u8>, Target)> {
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), udp.recv_from(&mut buf))
            .await
            .expect("received within the bound")?;
        Ok((buf[..n].to_vec(), from))
    }

    #[tokio::test]
    async fn a_datagram_in_the_answers_record_is_the_first_one() {
        let (client, mut server) = pair().await;
        let udp = opened(client, &mut server, &[&[0, 4, 1, 2, 3, 4, 0, 53, b'a']])
            .await
            .unwrap();
        assert_eq!(
            received(&udp).await.unwrap(),
            (b"a".to_vec(), to("1.2.3.4", 53))
        );
        // one record per datagram
        udp.send_to(b"one", &to("1.2.3.4", 53)).await.unwrap();
        udp.send_to(b"two", &to("5.6.7.8", 53)).await.unwrap();
        assert_eq!(
            next_record(&mut server).await.unwrap(),
            [1, 0, 4, 1, 2, 3, 4, 0, 53, b'o', b'n', b'e']
        );
        assert_eq!(
            next_record(&mut server).await.unwrap(),
            [1, 0, 4, 5, 6, 7, 8, 0, 53, b't', b'w', b'o']
        );
    }

    #[tokio::test]
    async fn what_is_no_datagram_is_dropped_and_the_next_arrives() {
        let (client, mut server) = pair().await;
        let udp = opened(client, &mut server, &[&[0]]).await.unwrap();
        // an unknown family, a cut-short address, one too long for `buf`
        server
            .write_all(&[5, 1, 2, 3, 4, 0, 53, b'x'])
            .await
            .unwrap();
        server.write_all(&[4, 1, 2, 3]).await.unwrap();
        let mut long = vec![4, 1, 2, 3, 4, 0, 53];
        long.extend_from_slice(&[0u8; 2000]);
        server.write_all(&long).await.unwrap();
        server
            .write_all(&[4, 1, 2, 3, 4, 0, 53, b'z'])
            .await
            .unwrap();
        let mut buf = vec![0u8; 1500];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), udp.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!((&buf[..n], from), (&b"z"[..], to("1.2.3.4", 53)));
    }

    #[tokio::test]
    async fn the_servers_refusal_or_another_answer_fails_the_open() {
        let (client, mut server) = pair().await;
        // the error's message in a record of its own
        let err = opened(client, &mut server, &[&[2, 9, 7], b"no udp\x07"])
            .await
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
        assert_eq!(err.to_string(), "snell: the server refused: no udp");
        let (client, mut server) = pair().await;
        let err = opened(client, &mut server, &[&[1]]).await.err().unwrap();
        assert_eq!(err.to_string(), UNKNOWN_REPLY);
        let (client, mut server) = pair().await;
        let open = tokio::spawn(SnellUdp::open(client));
        next_record(&mut server).await.unwrap();
        server.shutdown().await.unwrap();
        let err = open.await.unwrap().err().unwrap();
        assert_eq!(
            err.to_string(),
            "snell: the server closed the connection without answering"
        );
    }

    #[tokio::test]
    async fn the_connections_end_is_the_carriers_end() {
        let (client, mut server) = pair().await;
        let udp = opened(client, &mut server, &[&[0]]).await.unwrap();
        // the server's empty record
        poll_fn(|cx| server.poll_end(cx)).await.unwrap();
        let err = received(&udp).await.unwrap_err();
        assert_eq!(
            (err.kind(), err.to_string().as_str()),
            (
                io::ErrorKind::UnexpectedEof,
                "snell: the server closed the UDP connection"
            )
        );
        // the close between records too
        let (client, mut server) = pair().await;
        let udp = opened(client, &mut server, &[&[0]]).await.unwrap();
        drop(server);
        let err = received(&udp).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "snell: the server closed the UDP connection"
        );
    }

    #[tokio::test]
    async fn a_datagram_too_long_for_a_record_is_refused_unsent() {
        let (client, mut server) = pair().await;
        let udp = opened(client, &mut server, &[&[0]]).await.unwrap();
        let err = udp
            .send_to(&[0u8; MAX_PAYLOAD], &to("1.2.3.4", 53))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "snell: a datagram longer than 16374 bytes");
        udp.send_to(b"fits", &to("1.2.3.4", 53)).await.unwrap();
        assert_eq!(
            next_record(&mut server).await.unwrap(),
            [1, 0, 4, 1, 2, 3, 4, 0, 53, b'f', b'i', b't', b's'],
            "nothing went out before it"
        );
    }
}
