//! `proxy-restricted-to-lan` (M3 design §6.4): only local sources may use a
//! listener bound to a non-loopback address.

use std::net::{IpAddr, SocketAddr};

/// Loopback, RFC 1918, IPv4 link-local, IPv6 unique-local / link-local; an
/// IPv4-mapped IPv6 address is judged as its IPv4 form.
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_loopback() || a.is_private() || a.is_link_local(),
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_local(IpAddr::V4(v4));
            }
            a.is_loopback() || a.is_unique_local() || a.is_unicast_link_local()
        }
    }
}

/// A loopback listener trusts every source (only local processes reach it).
pub fn source_allowed(listen: SocketAddr, source: IpAddr) -> bool {
    listen.ip().is_loopback() || is_local(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_sources() {
        let ok = |s: &str| is_local(s.parse().unwrap());
        assert!(
            ok("127.0.0.1")
                && ok("10.1.2.3")
                && ok("172.16.5.5")
                && ok("192.168.1.9")
                && ok("169.254.1.1")
        );
        assert!(ok("::1") && ok("fd00::1") && ok("fe80::1") && ok("::ffff:192.168.0.1"));
        assert!(!ok("8.8.8.8") && !ok("2001:db8::1") && !ok("::ffff:1.1.1.1") && !ok("100.64.0.1"));
        let lan: SocketAddr = "0.0.0.0:6152".parse().unwrap();
        let lo: SocketAddr = "127.0.0.1:6152".parse().unwrap();
        assert!(source_allowed(lan, "192.168.1.2".parse().unwrap()));
        assert!(!source_allowed(lan, "8.8.8.8".parse().unwrap()));
        assert!(source_allowed(lo, "8.8.8.8".parse().unwrap()));
    }
}
