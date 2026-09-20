//! `ATYP ADDR PORT` as SOCKS5 writes it (RFC 1928 §5). Trojan uses the same
//! encoding.

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

#[cfg(test)]
mod tests {
    use super::*;

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
