//! System resolver configuration (design §8): DNS servers, search domains,
//! the hosts file path and IPv6 availability. Read-only; on any error the
//! functions return empty values and log at debug level.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

pub fn servers() -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::new();
    for ip in platform::servers() {
        let sa = SocketAddr::new(ip, 53);
        if !out.contains(&sa) {
            out.push(sa);
        }
    }
    out
}

pub fn search_domains() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in platform::search_domains() {
        let d = d.trim().trim_matches('.').to_ascii_lowercase();
        if !d.is_empty() && !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

pub fn hosts_path() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(root)
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts")
    } else {
        PathBuf::from("/etc/hosts")
    }
}

/// True when some non-loopback interface carries a global IPv6 address.
pub fn has_ipv6() -> bool {
    match if_addrs::get_if_addrs() {
        Ok(list) => list.iter().any(|i| {
            !i.is_loopback()
                && match &i.addr {
                    if_addrs::IfAddr::V6(v6) => is_global_v6(&v6.ip),
                    if_addrs::IfAddr::V4(_) => false,
                }
        }),
        Err(e) => {
            tracing::debug!(error = %e, "cannot enumerate interfaces");
            false
        }
    }
}

/// Not loopback, link-local (fe80::/10), unique-local (fc00::/7), multicast or unspecified.
pub fn is_global_v6(ip: &Ipv6Addr) -> bool {
    !ip.is_loopback()
        && !ip.is_unicast_link_local()
        && !ip.is_unique_local()
        && !ip.is_multicast()
        && !ip.is_unspecified()
}

/// `resolv.conf` text → (nameservers, search domains). Usable on every platform (tests).
pub fn parse_resolv_conf(text: &str) -> (Vec<IpAddr>, Vec<String>) {
    match resolv_conf::Config::parse(text) {
        Ok(cfg) => {
            let servers = cfg.nameservers.iter().map(IpAddr::from).collect();
            let mut search: Vec<String> = cfg.get_search().cloned().unwrap_or_default();
            if search.is_empty()
                && let Some(d) = cfg.get_domain()
            {
                search.push(d.clone());
            }
            (servers, search)
        }
        Err(e) => {
            tracing::debug!(error = %e, "cannot parse resolv.conf");
            (Vec::new(), Vec::new())
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::net::IpAddr;

    pub fn servers() -> Vec<IpAddr> {
        match ipconfig::get_adapters() {
            Ok(adapters) => adapters
                .iter()
                .filter(|a| a.oper_status() == ipconfig::OperStatus::IfOperStatusUp)
                .filter(|a| a.if_type() != ipconfig::IfType::SoftwareLoopback)
                .flat_map(|a| a.dns_servers().iter().copied())
                .collect(),
            Err(e) => {
                tracing::debug!(error = %e, "cannot read adapters");
                Vec::new()
            }
        }
    }

    pub fn search_domains() -> Vec<String> {
        let mut out = ipconfig::computer::get_search_list().unwrap_or_default();
        if let Ok(Some(domain)) = ipconfig::computer::get_domain() {
            out.push(domain);
        }
        out
    }
}

#[cfg(not(windows))]
mod platform {
    use std::net::IpAddr;

    fn read() -> (Vec<IpAddr>, Vec<String>) {
        match std::fs::read_to_string("/etc/resolv.conf") {
            Ok(text) => super::parse_resolv_conf(&text),
            Err(e) => {
                tracing::debug!(error = %e, "cannot read /etc/resolv.conf");
                (Vec::new(), Vec::new())
            }
        }
    }

    pub fn servers() -> Vec<IpAddr> {
        read().0
    }

    pub fn search_domains() -> Vec<String> {
        read().1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resolv_conf_servers_and_search() {
        let (servers, search) = parse_resolv_conf(
            "# comment\nnameserver 192.0.2.1\nnameserver 2001:db8::1\nnameserver fe80::1%eth0\nsearch Example.COM. lan\noptions ndots:2\n",
        );
        assert_eq!(servers.len(), 3);
        assert_eq!(servers[0], "192.0.2.1".parse::<IpAddr>().unwrap());
        assert_eq!(servers[1], "2001:db8::1".parse::<IpAddr>().unwrap());
        assert_eq!(search, vec!["Example.COM.".to_string(), "lan".to_string()]);
        let (_, only_domain) = parse_resolv_conf("nameserver 1.1.1.1\ndomain home.arpa\n");
        assert_eq!(only_domain, vec!["home.arpa".to_string()]);
        assert_eq!(parse_resolv_conf("").0.len(), 0);
    }

    #[test]
    fn global_v6_classification() {
        let g = |s: &str| is_global_v6(&s.parse::<Ipv6Addr>().unwrap());
        assert!(g("2001:db8::1"));
        assert!(!g("::1"));
        assert!(!g("fe80::1"));
        assert!(!g("fd00::1"));
        assert!(!g("ff02::1"));
        assert!(!g("::"));
    }

    #[test]
    fn hosts_path_is_platform_specific() {
        let p = hosts_path();
        assert!(p.ends_with("hosts"));
        if cfg!(windows) {
            assert!(p.to_string_lossy().to_ascii_lowercase().contains("drivers"));
        } else {
            assert_eq!(p, PathBuf::from("/etc/hosts"));
        }
    }

    #[test]
    fn live_functions_never_panic() {
        let s = servers();
        assert!(s.iter().all(|sa| sa.port() == 53));
        let _ = search_domains();
        let _ = has_ipv6();
    }
}
