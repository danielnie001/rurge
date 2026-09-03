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
}
