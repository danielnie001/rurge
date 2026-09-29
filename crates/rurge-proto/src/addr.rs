//! `ATYP ADDR PORT` as SOCKS5 writes it (RFC 1928 §5; Trojan and AnyTLS use
//! the same encoding), and VMess's own order and type numbers.

use rurge_config::HostName;
use rurge_net::connector::Target;
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddrError {
    /// The name is not one a proxy request may carry (`hostname::to_ascii`).
    Unsendable,
    /// Longer than the one-byte length allows.
    TooLong,
}

/// A domain goes out by name (remote resolution), an IDN as its A-labels.
pub(crate) fn socks_addr(target: &Target) -> Result<Vec<u8>, AddrError> {
    let mut out = Vec::new();
    match &target.host {
        HostName::Ip(IpAddr::V4(v4)) => {
            out.push(1);
            out.extend_from_slice(&v4.octets());
        }
        HostName::Ip(IpAddr::V6(v6)) => {
            out.push(4);
            out.extend_from_slice(&v6.octets());
        }
        HostName::Domain(name) => {
            let name = crate::hostname::to_ascii(name).ok_or(AddrError::Unsendable)?;
            let len = u8::try_from(name.len()).map_err(|_| AddrError::TooLong)?;
            out.push(3);
            out.push(len);
            out.extend_from_slice(name.as_bytes());
        }
    }
    out.extend_from_slice(&target.port.to_be_bytes());
    Ok(out)
}

/// `ATYP ADDR PORT` at the start of `bytes`, and how many bytes it took;
/// `None` when it is cut short, of an unknown type, or a name that is no
/// host name.
pub(crate) fn parse_socks_addr(bytes: &[u8]) -> Option<(Target, usize)> {
    let (host, rest) = match *bytes.first()? {
        1 => {
            let b: [u8; 4] = bytes.get(1..5)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(b)), 5)
        }
        4 => {
            let b: [u8; 16] = bytes.get(1..17)?.try_into().ok()?;
            (HostName::Ip(IpAddr::from(b)), 17)
        }
        3 => {
            let len = usize::from(*bytes.get(1)?);
            let name = std::str::from_utf8(bytes.get(2..2 + len)?).ok()?;
            (HostName::from_wire(name)?, 2 + len)
        }
        _ => return None,
    };
    let port = u16::from_be_bytes(bytes.get(rest..rest + 2)?.try_into().ok()?);
    Some((Target::new(host, port), rest + 2))
}

/// `PORT TYPE ADDR` as VMess writes it: the port first, and the types are
/// 1 = IPv4, 2 = domain, 3 = IPv6 (not SOCKS5's 1 / 3 / 4).
pub(crate) fn vmess_addr(target: &Target) -> Result<Vec<u8>, AddrError> {
    let mut out = target.port.to_be_bytes().to_vec();
    match &target.host {
        HostName::Ip(IpAddr::V4(v4)) => {
            out.push(1);
            out.extend_from_slice(&v4.octets());
        }
        HostName::Ip(IpAddr::V6(v6)) => {
            out.push(3);
            out.extend_from_slice(&v6.octets());
        }
        HostName::Domain(name) => {
            let name = crate::hostname::to_ascii(name).ok_or(AddrError::Unsendable)?;
            let len = u8::try_from(name.len()).map_err(|_| AddrError::TooLong)?;
            out.push(2);
            out.push(len);
            out.extend_from_slice(name.as_bytes());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socks_address_reads_back_as_it_was_written() {
        for host in ["10.1.2.3", "2001:db8::1", "example.com"] {
            let target = Target::new(HostName::parse(host), 853);
            let mut bytes = socks_addr(&target).unwrap();
            let written = bytes.len();
            bytes.extend_from_slice(b"payload");
            assert_eq!(parse_socks_addr(&bytes), Some((target, written)));
        }
        assert_eq!(parse_socks_addr(&[1, 10, 1, 2, 3, 0]), None, "cut short");
        assert_eq!(parse_socks_addr(&[9, 0, 0]), None, "unknown type");
        assert_eq!(
            parse_socks_addr(&[3, 3, b'a', b' ', b'b', 0, 53]),
            None,
            "no host name"
        );
    }

    #[test]
    fn vmess_puts_the_port_first_and_numbers_the_types_its_own_way() {
        let t = |host: &str| Target::new(HostName::parse(host), 0x01bb);
        assert_eq!(
            vmess_addr(&t("10.1.2.3")).unwrap(),
            [0x01, 0xbb, 1, 10, 1, 2, 3]
        );
        let v6 = vmess_addr(&t("2001:db8::1")).unwrap();
        assert_eq!((&v6[..3], v6.len()), (&[0x01, 0xbb, 3][..], 2 + 1 + 16));
        let mut expected = vec![0x01, 0xbb, 2, 11];
        expected.extend_from_slice(b"example.com");
        assert_eq!(vmess_addr(&t("example.com")).unwrap(), expected);
        // the same bytes the reference vectors were made with
        assert_eq!(expected, crate::vmess::vectors::address());
        assert_eq!(
            vmess_addr(&Target::new(HostName::Domain("a@b.test".into()), 1)),
            Err(AddrError::Unsendable)
        );
        assert_eq!(
            vmess_addr(&Target::new(HostName::Domain("a".repeat(256)), 1)),
            Err(AddrError::TooLong)
        );
    }

    #[test]
    fn the_three_address_types() {
        let t = |host: &str| Target::new(HostName::parse(host), 0x1f90);
        assert_eq!(
            socks_addr(&t("10.1.2.3")).unwrap(),
            [1, 10, 1, 2, 3, 0x1f, 0x90]
        );
        let v6 = socks_addr(&t("2001:db8::1")).unwrap();
        assert_eq!((v6[0], v6.len()), (4, 1 + 16 + 2));
        let mut expected = vec![3, 21];
        expected.extend_from_slice(b"xn--bcher-kva.example");
        expected.extend_from_slice(&[0x1f, 0x90]);
        assert_eq!(socks_addr(&t("bücher.example")).unwrap(), expected);
        assert_eq!(
            socks_addr(&Target::new(HostName::Domain("a@b.test".into()), 1)),
            Err(AddrError::Unsendable)
        );
        assert_eq!(
            socks_addr(&Target::new(HostName::Domain("a".repeat(256)), 1)),
            Err(AddrError::TooLong)
        );
    }
}
