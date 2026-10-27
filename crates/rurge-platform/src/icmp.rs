//! ICMP (ICMPv6) echo sockets, and the echo requests and replies they carry
//! (M4 design §10). Blocking socket calls only: the binary makes the socket
//! asynchronous, binds it where rurge's own sockets go and matches the replies
//! to what waits for them (P13).

use crate::socket::Family;
use socket2::{Domain, Protocol, Socket, Type};
use std::io;

/// How the socket the echo requests go out on was opened. A `Datagram` one is
/// an unprivileged ICMP socket: the kernel answers for it, and on Linux it
/// rewrites the identifier to one of its own. A `Raw` one is the
/// Administrator's (Windows) or root's, and sees every ICMP message the
/// machine receives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Datagram,
    Raw,
}

impl Kind {
    /// Whether a reply on this kind of socket still carries the identifier its
    /// request went out with, so that the identifier tells this pinger's
    /// replies from another program's. Linux (and Android) rewrite it on their
    /// datagram sockets, and hand each such socket only its own replies. A raw
    /// socket keeps it, and so does macOS's datagram socket: xnu's
    /// `icmp_dgram_send` leaves `icmp_id` as written, and every ICMP socket
    /// hears every reply through `rip_input`.
    pub fn keeps_ident(self) -> bool {
        self == Kind::Raw || !cfg!(any(target_os = "linux", target_os = "android"))
    }
}

/// `socket` in non-blocking mode, which is what `open` promises: the binary
/// makes it asynchronous.
fn nonblocking(socket: Socket) -> io::Result<Socket> {
    socket.set_nonblocking(true)?;
    Ok(socket)
}

/// An ICMP (ICMPv6) socket for echo requests, non-blocking: Linux tries a
/// datagram socket (no privilege, within `net.ipv4.ping_group_range`) and
/// then a raw one; macOS a datagram socket; Windows a raw one (Administrator).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn open(family: Family) -> io::Result<(Socket, Kind)> {
    let (domain, protocol) = domain_and_protocol(family);
    match Socket::new(domain, Type::DGRAM, Some(protocol)) {
        Ok(socket) => nonblocking(socket).map(|socket| (socket, Kind::Datagram)),
        // not within `ping_group_range`: a raw socket is the root's
        Err(_) => Socket::new(domain, Type::RAW, Some(protocol))
            .and_then(nonblocking)
            .map(|socket| (socket, Kind::Raw)),
    }
}

#[cfg(target_os = "macos")]
pub fn open(family: Family) -> io::Result<(Socket, Kind)> {
    let (domain, protocol) = domain_and_protocol(family);
    // macOS's datagram ICMP sockets need no privilege
    Socket::new(domain, Type::DGRAM, Some(protocol))
        .and_then(nonblocking)
        .map(|socket| (socket, Kind::Datagram))
}

#[cfg(windows)]
pub fn open(family: Family) -> io::Result<(Socket, Kind)> {
    let (domain, protocol) = domain_and_protocol(family);
    // Windows has no unprivileged ICMP socket: the enhanced mode's raw one is
    // the Administrator's, as the virtual adapter is (M4 design 8.5)
    Socket::new(domain, Type::RAW, Some(protocol))
        .and_then(nonblocking)
        .map(|socket| (socket, Kind::Raw))
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
pub fn open(_family: Family) -> io::Result<(Socket, Kind)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "ICMP sockets are not supported on this platform",
    ))
}

fn domain_and_protocol(family: Family) -> (Domain, Protocol) {
    match family {
        Family::V4 => (Domain::IPV4, Protocol::ICMPV4),
        Family::V6 => (Domain::IPV6, Protocol::ICMPV6),
    }
}

/// The eight bytes of an ICMP echo message before its payload.
const HEADER_LEN: usize = 8;

/// An echo request: type 8 (ICMPv6: 128), code 0, `ident`, `seq`, `payload`.
/// The ICMPv4 checksum is filled in; the ICMPv6 one is the kernel's.
pub fn request(family: Family, ident: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let (ty, checksummed) = match family {
        Family::V4 => (8, true),
        Family::V6 => (128, false),
    };
    let mut message = vec![0u8; HEADER_LEN + payload.len()];
    message[0] = ty;
    message[4..6].copy_from_slice(&ident.to_be_bytes());
    message[6..8].copy_from_slice(&seq.to_be_bytes());
    message[HEADER_LEN..].copy_from_slice(payload);
    if checksummed {
        let checksum = checksum(&message);
        message[2..4].copy_from_slice(&checksum.to_be_bytes());
    }
    message
}

/// The one's complement of the one's complement sum of the message, as a
/// multiple of 16 bits with an odd last byte padded on the right with zeroes.
fn checksum(message: &[u8]) -> u16 {
    let mut sum = 0u32;
    for pair in message.chunks(2) {
        let word = match pair {
            [high, low] => u16::from_be_bytes([*high, *low]),
            [high] => u16::from_be_bytes([*high, 0]),
            _ => 0,
        };
        sum += u32::from(word);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// An echo reply, as a socket handed it over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub ident: u16,
    pub seq: u16,
    pub payload: Vec<u8>,
}

/// The echo reply in what a socket received — after an IPv4 header (raw
/// sockets, and macOS's datagram ones, give it), or without one. Anything that
/// is not an echo reply is `None`.
pub fn reply(family: Family, bytes: &[u8]) -> Option<Reply> {
    let (ty, message) = match family {
        // a leading 4 is the version of an IPv4 header, and never the type of
        // an echo reply (0)
        Family::V4 => {
            let message = match bytes.first() {
                Some(first) if first >> 4 == 4 => {
                    let header_len = usize::from(bytes.first()? & 0x0f) * 4;
                    bytes.get(header_len..)?
                }
                Some(_) => bytes,
                None => return None,
            };
            (0, message)
        }
        // an IPv6 socket never hands over an IPv6 header
        Family::V6 => (129, bytes),
    };
    if message.len() < HEADER_LEN || message[0] != ty || message[1] != 0 {
        return None;
    }
    Some(Reply {
        ident: u16::from_be_bytes([message[4], message[5]]),
        seq: u16::from_be_bytes([message[6], message[7]]),
        payload: message[HEADER_LEN..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request as the answer to itself: the type of an echo reply, the
    /// rest as it was.
    fn as_answer(family: Family, request: &[u8]) -> Vec<u8> {
        let mut answer = request.to_vec();
        answer[0] = match family {
            Family::V4 => 0,
            Family::V6 => 129,
        };
        answer
    }

    #[test]
    fn request_and_reply_round_trip_for_both_families() {
        for (family, ty, payload) in [
            (Family::V4, 8, b"rurge".as_slice()),
            (Family::V6, 128, b"".as_slice()),
        ] {
            let request = request(family, 0x1234, 7, payload);
            assert_eq!(request[0], ty, "an echo request");
            assert_eq!(request[1], 0, "code 0");
            assert_eq!(&request[8..], payload);

            let answer = reply(family, &as_answer(family, &request)).expect("an echo reply");
            assert_eq!(answer.ident, 0x1234);
            assert_eq!(answer.seq, 7);
            assert_eq!(answer.payload, payload);
        }
    }

    #[test]
    fn the_icmpv4_checksum_is_right() {
        // 08 00 0000 1234 0001 "rurge" (padded to 14 bytes), summed by hand
        let packet = request(Family::V4, 0x1234, 1, b"rurge");
        assert_eq!(
            packet,
            [
                0x08, 0x00, 0x9b, 0xed, 0x12, 0x34, 0x00, 0x01, b'r', b'u', b'r', b'g', b'e'
            ]
        );
        // the ICMPv6 checksum is the kernel's: nothing is filled in
        let uncovered = request(Family::V6, 0x1234, 1, b"rurge");
        assert_eq!(&uncovered[2..4], &[0, 0]);
    }

    #[test]
    fn a_reply_after_an_ipv4_header_is_read() {
        let answer = as_answer(Family::V4, &request(Family::V4, 0x1234, 9, b"rurge"));
        for header_len in [20, 24] {
            let mut packet = vec![0u8; header_len];
            packet[0] = 0x40 | (header_len as u8 / 4); // version 4, this IHL
            packet.extend_from_slice(&answer);
            let reply = reply(Family::V4, &packet).expect("an echo reply after its header");
            assert_eq!(reply.seq, 9);
            assert_eq!(reply.payload, b"rurge");
        }
        // IPv6 sockets never hand over an IPv6 header
        let answer = as_answer(Family::V6, &request(Family::V6, 0x1234, 9, b"rurge"));
        assert_eq!(reply(Family::V6, &answer).expect("an echo reply").seq, 9);
    }

    #[test]
    fn only_linux_datagram_sockets_lose_the_identifier() {
        assert!(
            Kind::Raw.keeps_ident(),
            "a raw socket keeps the identifier its request went out with"
        );
        assert_eq!(
            Kind::Datagram.keeps_ident(),
            !cfg!(any(target_os = "linux", target_os = "android")),
            "only Linux's datagram sockets hand out an identifier of the kernel's"
        );
    }

    #[test]
    fn anything_but_an_echo_reply_is_none() {
        let packet = request(Family::V4, 0x1234, 1, b"rurge");
        assert_eq!(reply(Family::V4, &packet), None, "an echo request");
        let packet = request(Family::V6, 0x1234, 1, b"rurge");
        assert_eq!(reply(Family::V6, &packet), None, "an echo request");

        let mut unreachable = as_answer(Family::V4, &request(Family::V4, 0, 1, b""));
        unreachable[0] = 3; // destination unreachable
        assert_eq!(reply(Family::V4, &unreachable), None);

        let mut code = as_answer(Family::V4, &request(Family::V4, 0, 1, b""));
        code[1] = 1;
        assert_eq!(
            reply(Family::V4, &code),
            None,
            "code 1 is not an echo reply"
        );

        assert_eq!(reply(Family::V4, &[0x00, 0x00, 0x00]), None, "too short");
        // an IPv4 header alone: nothing after it
        assert_eq!(reply(Family::V4, &[0x45; 20]), None);
    }
}
