//! Small value types shared by every layer.

use std::fmt;
use std::net::IpAddr;

/// A connection target: a domain name (lowercase, no trailing dot) or an IP literal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HostName {
    Domain(String),
    Ip(IpAddr),
}

impl HostName {
    pub fn parse(s: &str) -> HostName {
        let s = s.trim();
        let bare = s
            .strip_prefix('[')
            .and_then(|x| x.strip_suffix(']'))
            .unwrap_or(s);
        if let Ok(ip) = bare.parse::<IpAddr>() {
            return HostName::Ip(ip);
        }
        HostName::Domain(bare.trim_end_matches('.').to_ascii_lowercase())
    }
    /// A hostname without a dot, such as `localhost` or `nas`.
    pub fn is_simple(&self) -> bool {
        matches!(self, HostName::Domain(d) if !d.contains('.'))
    }
    pub fn as_domain(&self) -> Option<&str> {
        match self {
            HostName::Domain(d) => Some(d),
            HostName::Ip(_) => None,
        }
    }
    pub fn as_ip(&self) -> Option<IpAddr> {
        match self {
            HostName::Ip(ip) => Some(*ip),
            HostName::Domain(_) => None,
        }
    }

    /// A name that came off the network (a SOCKS5 request, a CONNECT
    /// authority). `None` when it is empty or holds a control character or
    /// whitespace: nothing a resolver, a rule or a proxy request line can
    /// carry safely. Non-ASCII (IDN) names pass through unchanged, so rules
    /// keep matching what the client sent; outbounds convert them to A-labels.
    pub fn from_wire(s: &str) -> Option<HostName> {
        if s.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return None;
        }
        match HostName::parse(s) {
            HostName::Domain(d) if d.is_empty() => None,
            host => Some(host),
        }
    }
}

impl fmt::Display for HostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostName::Domain(d) => f.write_str(d),
            HostName::Ip(ip) => write!(f, "{ip}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed_non_ip_falls_back_to_bare_domain() {
        // The brackets are an IPv6-literal marker (as in `[::1]:port`), not
        // part of the domain; the fallback must parse from `bare`, not `s`,
        // or the brackets survive into the domain string.
        assert_eq!(
            HostName::parse("[example.com]"),
            HostName::Domain("example.com".into())
        );
    }

    #[test]
    fn names_from_the_wire_may_not_hold_control_characters_or_whitespace() {
        for bad in [
            "",
            ".",
            "a.test\r\nX-Evil: 1",
            "a\0b.test",
            "a b.test",
            " a.test",
            "a.test\t",
            "a\u{7f}.test",
            "a\u{85}.test",
            "a\u{2028}.test",
        ] {
            assert_eq!(HostName::from_wire(bad), None, "{bad:?}");
        }
        assert_eq!(
            HostName::from_wire("Example.TEST."),
            Some(HostName::Domain("example.test".into()))
        );
        assert_eq!(
            HostName::from_wire("[::1]"),
            Some(HostName::Ip("::1".parse().unwrap()))
        );
        assert_eq!(
            HostName::from_wire("192.0.2.7"),
            Some(HostName::Ip("192.0.2.7".parse().unwrap()))
        );
        // IDN names pass through: rules keep matching what the client sent
        assert_eq!(
            HostName::from_wire("bücher.example"),
            Some(HostName::Domain("bücher.example".into()))
        );
    }
}
