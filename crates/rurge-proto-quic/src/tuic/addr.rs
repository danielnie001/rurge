//! The address of TUIC's commands (TUIC v5 SPEC, "Address"; phase 2 M7
//! design 6.2): `u8 type ‖ address ‖ u16 port`, big endian — `0x00` a
//! domain name, its length in a byte first; `0x01` an IPv4 address; `0x02`
//! an IPv6 address; `0xff` none, with no port either: what every fragment of
//! a UDP packet but the first carries.

use rurge_config::HostName;
use rurge_net::connector::Target;
use rurge_proto::http::wire_host;
use std::fmt;
use std::net::IpAddr;

const DOMAIN: u8 = 0x00;
const IPV4: u8 = 0x01;
const IPV6: u8 = 0x02;
/// No address and no port.
pub(super) const NONE: u8 = 0xff;

/// Why a target cannot be named to the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AddrError {
    /// The name is not one a request may carry (`http::wire_host`).
    Unsendable,
    /// Longer than its one-byte length allows.
    TooLong,
}

impl fmt::Display for AddrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AddrError::Unsendable => "tuic: the host name cannot be sent to the server",
            AddrError::TooLong => "tuic: the host name is longer than 255 bytes",
        })
    }
}

/// `target`'s address at the end of `out`: a name goes out as a name —
/// the server resolves it —, an IDN as its A-labels. Nothing is written
/// for a target that cannot be named.
pub(super) fn put(out: &mut Vec<u8>, target: &Target) -> Result<(), AddrError> {
    match &target.host {
        HostName::Ip(IpAddr::V4(v4)) => {
            out.push(IPV4);
            out.extend_from_slice(&v4.octets());
        }
        HostName::Ip(IpAddr::V6(v6)) => {
            out.push(IPV6);
            out.extend_from_slice(&v6.octets());
        }
        HostName::Domain(_) => {
            let name = wire_host(target).ok_or(AddrError::Unsendable)?;
            let len = u8::try_from(name.len()).map_err(|_| AddrError::TooLong)?;
            out.push(DOMAIN);
            out.push(len);
            out.extend_from_slice(name.as_bytes());
        }
    }
    out.extend_from_slice(&target.port.to_be_bytes());
    Ok(())
}

/// The address at the start of `data`, as the server writes the source of
/// a UDP packet, and how many bytes it took: `None` inside for none
/// (`0xff`). `None` for anything else: a type TUIC does not know, an
/// address cut short, a name that is no host name (`HostName::from_wire`).
/// A server may name the source as a name: sing-box answers so for a
/// target that was one.
pub(super) fn get(data: &[u8]) -> Option<(Option<Target>, usize)> {
    let (&kind, rest) = data.split_first()?;
    let (host, len) = match kind {
        NONE => return Some((None, 1)),
        IPV4 => {
            let octets: [u8; 4] = rest.get(..4)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(octets)), 4)
        }
        IPV6 => {
            let octets: [u8; 16] = rest.get(..16)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(octets)), 16)
        }
        DOMAIN => {
            let (&n, name) = rest.split_first()?;
            let name = std::str::from_utf8(name.get(..usize::from(n))?).ok()?;
            (HostName::from_wire(name)?, 1 + usize::from(n))
        }
        _ => return None,
    };
    let port = rest.get(len..len + 2)?;
    let port = u16::from_be_bytes([port[0], port[1]]);
    Some((Some(Target::new(host, port)), 1 + len + 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(target: &Target) -> Result<Vec<u8>, AddrError> {
        let mut out = Vec::new();
        put(&mut out, target).map(|()| out)
    }

    fn at(host: &str) -> Target {
        Target::new(HostName::parse(host), 0x1f90)
    }

    #[test]
    fn the_three_address_types() {
        assert_eq!(
            address(&at("10.1.2.3")),
            Ok(vec![0x01, 10, 1, 2, 3, 0x1f, 0x90])
        );
        let mut v6 = vec![0x02, 0x20, 0x01, 0x0d, 0xb8];
        v6.extend_from_slice(&[0; 11]);
        v6.extend_from_slice(&[1, 0x1f, 0x90]);
        assert_eq!(address(&at("2001:db8::1")), Ok(v6));
        // a name as its A-labels, its length first
        let mut name = vec![0x00, 21];
        name.extend_from_slice(b"xn--bcher-kva.example");
        name.extend_from_slice(&[0x1f, 0x90]);
        assert_eq!(address(&at("bücher.example")), Ok(name));
    }

    #[test]
    fn a_name_must_be_one_the_server_can_read() {
        // the longest a length byte gives
        let longest = address(&at(&"a".repeat(255))).unwrap();
        assert_eq!((longest[1], longest.len()), (255, 1 + 1 + 255 + 2));
        let mut out = Vec::new();
        for (host, error) in [
            ("a".repeat(256), AddrError::TooLong),
            ("x@blocked.test".to_string(), AddrError::Unsendable),
            ("a b.test".to_string(), AddrError::Unsendable),
            (String::new(), AddrError::Unsendable),
        ] {
            let target = Target::new(HostName::Domain(host), 80);
            assert_eq!(put(&mut out, &target), Err(error));
        }
        assert!(out.is_empty(), "nothing written for a target not named");
        assert_eq!(
            AddrError::Unsendable.to_string(),
            "tuic: the host name cannot be sent to the server"
        );
        assert_eq!(
            AddrError::TooLong.to_string(),
            "tuic: the host name is longer than 255 bytes"
        );
    }

    /// What `put` writes reads back, and so does none; whatever follows is
    /// not the address's.
    #[test]
    fn an_address_reads_back_with_its_length() {
        for host in [
            "10.1.2.3",
            "2001:db8::1",
            "dns.example",
            "xn--bcher-kva.example",
        ] {
            let mut data = address(&at(host)).unwrap();
            let len = data.len();
            data.extend_from_slice(b"payload");
            assert_eq!(get(&data), Some((Some(at(host)), len)), "{host}");
        }
        assert_eq!(get(&[NONE, 0x1f, 0x90]), Some((None, 1)));
    }

    #[test]
    fn an_address_that_is_none_a_server_writes_reads_as_nothing() {
        for bad in [
            &[][..],
            // a type TUIC does not know
            &[0x03, 1, 2, 3, 4, 0, 53][..],
            // cut short: in the address, in the port
            &[0x01, 1, 2, 3][..],
            &[0x01, 1, 2, 3, 4, 0][..],
            &[0x02, 0x20, 0x01][..],
            &[0x00, 5, b'a', b'b'][..],
            &[0x00, 1, b'a', 0][..],
            // a name that is no host name: empty, a blank, not UTF-8
            &[0x00, 0, 0, 53][..],
            &[0x00, 3, b'a', b' ', b'b', 0, 53][..],
            &[0x00, 2, 0xff, 0xfe, 0, 53][..],
        ] {
            assert_eq!(get(bad), None, "{bad:?}");
        }
    }
}
