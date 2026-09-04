//! Access to the operating system's resolver configuration (design §7.1).
//! The binary implements it through `rurge-platform`; tests use the static kinds.

use std::net::SocketAddr;
use std::path::PathBuf;

pub trait SystemDns: Send + Sync {
    /// System DNS servers (port 53 unless the platform says otherwise).
    fn servers(&self) -> Vec<SocketAddr>;
    fn search_domains(&self) -> Vec<String>;
    fn hosts_path(&self) -> Option<PathBuf>;
    /// True when some interface has a global (non link-local, non unique-local) IPv6 address.
    fn has_ipv6(&self) -> bool;
}

/// No system information at all (tests, sandboxes).
pub struct NoSystemDns;

impl SystemDns for NoSystemDns {
    fn servers(&self) -> Vec<SocketAddr> {
        Vec::new()
    }
    fn search_domains(&self) -> Vec<String> {
        Vec::new()
    }
    fn hosts_path(&self) -> Option<PathBuf> {
        None
    }
    fn has_ipv6(&self) -> bool {
        false
    }
}

/// Fixed values (tests and CLI overrides).
#[derive(Clone, Debug, Default)]
pub struct StaticSystemDns {
    pub servers: Vec<SocketAddr>,
    pub search_domains: Vec<String>,
    pub hosts_path: Option<PathBuf>,
    pub has_ipv6: bool,
}

impl SystemDns for StaticSystemDns {
    fn servers(&self) -> Vec<SocketAddr> {
        self.servers.clone()
    }
    fn search_domains(&self) -> Vec<String> {
        self.search_domains.clone()
    }
    fn hosts_path(&self) -> Option<PathBuf> {
        self.hosts_path.clone()
    }
    fn has_ipv6(&self) -> bool {
        self.has_ipv6
    }
}
