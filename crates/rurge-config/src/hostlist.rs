//! The "Host List" parameter type shared by skip-proxy, always-real-ip,
//! force-http-engine-hosts, always-raw-tcp-hosts and MITM hostname.

use crate::glob::{Glob, GlobOptions};
use crate::types::HostName;
use ipnet::IpNet;
use std::net::IpAddr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostPattern {
    Glob(Glob),
    Cidr(IpNet),
    AnyIp,
    AnyV4,
    AnyV6,
    SimpleHostname,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortSpec {
    Default,
    Any,
    Port(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostListEntry {
    pub negate: bool,
    pub pattern: HostPattern,
    pub port: PortSpec,
    pub raw: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostList {
    pub entries: Vec<HostListEntry>,
    pub default_port: Option<u16>,
    pub invalid: Vec<String>,
}

impl HostList {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn parse(text: &str, default_port: Option<u16>) -> HostList {
        let mut list = HostList {
            entries: Vec::new(),
            default_port,
            invalid: Vec::new(),
        };
        for raw in text.split(',') {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            match parse_entry(raw) {
                Some(e) => list.entries.push(e),
                None => list.invalid.push(raw.to_string()),
            }
        }
        list
    }

    /// First entry whose host and port both match decides: `Some(!negate)`.
    pub fn matches(&self, host: &HostName, port: u16) -> Option<bool> {
        self.decide(host, |e| match e.port {
            PortSpec::Any => true,
            PortSpec::Port(p) => p == port,
            PortSpec::Default => self.default_port.is_none_or(|d| d == port),
        })
    }

    /// The first entry whose host matches and whose port is not asked for
    /// decides: an entry without a port matches on every port (phase 4 M2
    /// design 7.3: a bare entry matches the sniffed SNI regardless of the
    /// connection's port).
    pub fn matches_sni(&self, host: &HostName, port: u16) -> Option<bool> {
        self.decide(host, |e| match e.port {
            PortSpec::Any | PortSpec::Default => true,
            PortSpec::Port(p) => p == port,
        })
    }

    /// The ports the entries that include a host write down, in increasing
    /// order and once each: the ports rurge sniffs on every target (phase 4
    /// M2 design 7.6). An excluded entry widens nothing, and `:0` is every
    /// port already rather than one to add.
    pub fn explicit_ports(&self) -> Vec<u16> {
        let mut ports: Vec<u16> = self
            .entries
            .iter()
            .filter(|e| !e.negate)
            .filter_map(|e| match e.port {
                PortSpec::Port(p) if p != 0 => Some(p),
                _ => None,
            })
            .collect();
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    fn decide(
        &self,
        host: &HostName,
        port_matches: impl Fn(&HostListEntry) -> bool,
    ) -> Option<bool> {
        self.entries
            .iter()
            .find(|e| port_matches(e) && pattern_matches(&e.pattern, host))
            .map(|e| !e.negate)
    }

    /// This list without the entries `other` writes the same way: the same
    /// `-`, and the text after it equal apart from the case (`phase 4 M2
    /// design 4.2` — the port is part of the text, so `example.com` and
    /// `example.com:443` differ). The order and the default port are this
    /// list's.
    pub fn without(&self, other: &HostList) -> HostList {
        HostList {
            entries: self
                .entries
                .iter()
                .filter(|e| !other.entries.iter().any(|o| written_alike(e, o)))
                .cloned()
                .collect(),
            default_port: self.default_port,
            invalid: self.invalid.clone(),
        }
    }
}

/// Whether the two entries are written the same way: the same `-`, and the
/// text after it equal ignoring ASCII case.
fn written_alike(a: &HostListEntry, b: &HostListEntry) -> bool {
    fn body(e: &HostListEntry) -> &str {
        e.raw.strip_prefix('-').unwrap_or(&e.raw).trim()
    }
    a.negate == b.negate && body(a).eq_ignore_ascii_case(body(b))
}

fn pattern_matches(p: &HostPattern, host: &HostName) -> bool {
    match (p, host) {
        (HostPattern::Glob(g), h) => g.matches(&h.to_string()),
        (HostPattern::Cidr(net), HostName::Ip(ip)) => net.contains(ip),
        (HostPattern::AnyIp, HostName::Ip(_)) => true,
        (HostPattern::AnyV4, HostName::Ip(IpAddr::V4(_))) => true,
        (HostPattern::AnyV6, HostName::Ip(IpAddr::V6(_))) => true,
        (HostPattern::SimpleHostname, h) => h.is_simple(),
        _ => false,
    }
}

/// Split `host[:port]`, handling `[v6]:port` and bare IPv6.
fn split_host_port(s: &str) -> Option<(&str, PortSpec)> {
    if let Some(rest) = s.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        return match after.strip_prefix(':') {
            None if after.is_empty() => Some((host, PortSpec::Default)),
            Some(p) => Some((host, parse_port(p)?)),
            _ => None,
        };
    }
    if s.matches(':').count() >= 2 {
        return Some((s, PortSpec::Default)); // bare IPv6 literal
    }
    match s.rsplit_once(':') {
        Some((host, p)) => Some((host, parse_port(p)?)),
        None => Some((s, PortSpec::Default)),
    }
}

fn parse_port(p: &str) -> Option<PortSpec> {
    let n: u16 = p.parse().ok()?;
    Some(if n == 0 {
        PortSpec::Any
    } else {
        PortSpec::Port(n)
    })
}

fn parse_entry(raw: &str) -> Option<HostListEntry> {
    let (negate, body) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest.trim()),
        None => (false, raw),
    };
    if body.is_empty() {
        return None;
    }
    let lower = body.to_ascii_lowercase();
    for (token, pattern) in [
        ("<ip-address>", HostPattern::AnyIp),
        ("<ipv4-address>", HostPattern::AnyV4),
        ("<ipv6-address>", HostPattern::AnyV6),
        ("<simple-hostname>", HostPattern::SimpleHostname),
    ] {
        if let Some(rest) = lower.strip_prefix(token) {
            let port = match rest.strip_prefix(':') {
                None if rest.is_empty() => PortSpec::Default,
                Some(p) => parse_port(p)?,
                _ => return None,
            };
            return Some(HostListEntry {
                negate,
                pattern,
                port,
                raw: raw.to_string(),
            });
        }
    }
    if body.contains('/') {
        let net: IpNet = body.parse().ok()?;
        return Some(HostListEntry {
            negate,
            pattern: HostPattern::Cidr(net),
            port: PortSpec::Default,
            raw: raw.to_string(),
        });
    }
    let (host, port) = split_host_port(body)?;
    if host.is_empty() {
        return None;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(HostListEntry {
            negate,
            pattern: HostPattern::Cidr(IpNet::from(ip)),
            port,
            raw: raw.to_string(),
        });
    }
    let glob = Glob::new(
        host,
        GlobOptions {
            case_insensitive: true,
            classes: false,
        },
    )
    .ok()?;
    Some(HostListEntry {
        negate,
        pattern: HostPattern::Glob(glob),
        port,
        raw: raw.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::HostName;

    fn h(s: &str) -> HostName {
        HostName::parse(s)
    }

    /// With an SNI, an entry without a port matches on every port; the plain
    /// `matches` still asks for the default port. An entry with a port, or
    /// with `:0`, is the same for both.
    #[test]
    fn matches_sni_ignores_the_port_of_a_bare_entry() {
        let bare = HostList::parse("example.com", Some(443));
        assert_eq!(bare.matches_sni(&h("example.com"), 8443), Some(true));
        assert_eq!(bare.matches(&h("example.com"), 8443), None);

        let on_a_port = HostList::parse("example.com:8443", Some(443));
        assert_eq!(on_a_port.matches_sni(&h("example.com"), 8443), Some(true));
        assert_eq!(on_a_port.matches(&h("example.com"), 8443), Some(true));
        assert_eq!(on_a_port.matches_sni(&h("example.com"), 443), None);
        assert_eq!(on_a_port.matches(&h("example.com"), 443), None);

        let on_every_port = HostList::parse("example.com:0", Some(443));
        for port in [1, 443, 8443] {
            assert_eq!(
                on_every_port.matches_sni(&h("example.com"), port),
                Some(true)
            );
            assert_eq!(on_every_port.matches(&h("example.com"), port), Some(true));
        }

        let excluded = HostList::parse("-bad.example.com, *.example.com", Some(443));
        assert_eq!(
            excluded.matches_sni(&h("bad.example.com"), 8443),
            Some(false)
        );
        assert_eq!(excluded.matches(&h("bad.example.com"), 443), Some(false));
    }

    #[test]
    fn explicit_ports_are_the_ports_of_entries_that_include() {
        let list = HostList::parse(
            "a.com:8443, -b.com:9443, c.com, d.com:0, e.com:8443, f.com:25",
            Some(443),
        );
        assert_eq!(list.explicit_ports(), [25, 8443]);
        assert!(
            HostList::parse("a.com, b.com:0", Some(443))
                .explicit_ports()
                .is_empty()
        );
    }

    #[test]
    fn force_http_engine_hosts_examples() {
        let list = HostList::parse(
            "-*.apple.com, www.google.com, www.google.com:8080, api.example.com:0, *:0",
            Some(80),
        );
        assert!(list.invalid.is_empty());
        assert_eq!(list.matches(&h("x.apple.com"), 80), Some(false));
        assert_eq!(list.matches(&h("www.google.com"), 80), Some(true));
        assert_eq!(list.matches(&h("www.google.com"), 8080), Some(true));
        assert_eq!(list.matches(&h("www.google.com"), 443), Some(true)); // *:0 catches it
        assert_eq!(list.matches(&h("api.example.com"), 12345), Some(true));
        let narrow = HostList::parse("www.google.com", Some(80));
        assert_eq!(narrow.matches(&h("www.google.com"), 443), None);
        assert_eq!(narrow.matches(&h("WWW.GOOGLE.COM"), 80), Some(true));
    }

    #[test]
    fn mitm_hostname_example_and_special_tokens() {
        let list = HostList::parse("-*icloud*, -*.mzstatic.com, -<ip-address>, *", Some(443));
        assert_eq!(list.matches(&h("gateway.icloud.com"), 443), Some(false));
        assert_eq!(list.matches(&h("a.mzstatic.com"), 443), Some(false));
        assert_eq!(list.matches(&h("1.2.3.4"), 443), Some(false));
        assert_eq!(list.matches(&h("example.com"), 443), Some(true));
        assert_eq!(list.matches(&h("example.com"), 8443), None);
        let v4 = HostList::parse("<ipv4-address>, <simple-hostname>", None);
        assert_eq!(v4.matches(&h("10.0.0.1"), 1), Some(true));
        assert_eq!(v4.matches(&h("::1"), 1), None);
        assert_eq!(v4.matches(&h("nas"), 1), Some(true));
        assert_eq!(v4.matches(&h("nas.lan"), 1), None);
    }

    /// The four special tokens of the manual each match their own kind of host
    /// and no other; a minus in front excludes it, a port after it limits it,
    /// `:0` takes every port.
    #[test]
    fn each_special_token_matches_its_own_kind_of_host() {
        let hosts = ["10.0.0.1", "fd00::1", "nas", "nas.lan"];
        for (token, matching) in [
            ("<ip-address>", [true, true, false, false]),
            ("<ipv4-address>", [true, false, false, false]),
            ("<ipv6-address>", [false, true, false, false]),
            ("<simple-hostname>", [false, false, true, false]),
        ] {
            let list = HostList::parse(token, Some(443));
            let excluded = HostList::parse(&format!("-{token}, *"), Some(443));
            let on_a_port = HostList::parse(&format!("{token}:8443"), Some(443));
            let on_every_port = HostList::parse(&format!("{token}:0"), Some(443));
            for (host, matches) in hosts.iter().zip(matching) {
                let host = h(host);
                assert_eq!(
                    list.matches(&host, 443),
                    matches.then_some(true),
                    "{token} {host}"
                );
                assert_eq!(
                    excluded.matches(&host, 443),
                    Some(!matches),
                    "-{token} {host}"
                );
                assert_eq!(
                    on_a_port.matches(&host, 8443),
                    matches.then_some(true),
                    "{token}:8443 {host}"
                );
                assert_eq!(on_a_port.matches(&host, 443), None, "{token}:8443 {host}");
                assert_eq!(
                    on_every_port.matches(&host, 1),
                    matches.then_some(true),
                    "{token}:0 {host}"
                );
            }
        }
    }

    #[test]
    fn skip_proxy_with_cidr_and_ipv6() {
        let list = HostList::parse(
            "127.0.0.1, 192.168.0.0/16, 10.0.0.0/8, localhost, *.local, [::1]:0, fe80::/10",
            None,
        );
        assert!(list.invalid.is_empty(), "{:?}", list.invalid);
        assert_eq!(list.matches(&h("127.0.0.1"), 80), Some(true));
        assert_eq!(list.matches(&h("192.168.1.9"), 80), Some(true));
        assert_eq!(list.matches(&h("172.16.0.1"), 80), None);
        assert_eq!(list.matches(&h("localhost"), 80), Some(true));
        assert_eq!(list.matches(&h("printer.local"), 80), Some(true));
        assert_eq!(list.matches(&h("::1"), 80), Some(true));
        assert_eq!(list.matches(&h("fe80::1"), 80), Some(true));
    }

    #[test]
    fn invalid_entries_are_collected() {
        let list = HostList::parse("ok.com, -, :80, 10.0.0.0/99, [::1", Some(80));
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.invalid, ["-", ":80", "10.0.0.0/99", "[::1"]);
    }

    #[test]
    fn hostname_parse() {
        assert_eq!(h("Example.COM."), HostName::Domain("example.com".into()));
        assert!(matches!(
            h("1.2.3.4"),
            HostName::Ip(std::net::IpAddr::V4(_))
        ));
        assert!(h("nas").is_simple());
        assert!(!h("nas.lan").is_simple());
        assert_eq!(h("::1").to_string(), "::1");
    }
}
