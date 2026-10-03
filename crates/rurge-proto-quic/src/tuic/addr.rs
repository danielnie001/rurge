//! The address of TUIC's commands (TUIC v5 SPEC, "Address"; phase 2 M7
//! design 6.2): `u8 type ‖ address ‖ u16 port`, big endian — `0x00` a
//! domain name, its length in a byte first; `0x01` an IPv4 address; `0x02`
//! an IPv6 address. (`0xff`, no address and no port, only follows the first
//! fragment of a UDP packet.)

use rurge_config::HostName;
use rurge_net::connector::Target;
use rurge_proto::http::wire_host;
use std::fmt;
use std::net::IpAddr;

const DOMAIN: u8 = 0x00;
const IPV4: u8 = 0x01;
const IPV6: u8 = 0x02;

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
}
