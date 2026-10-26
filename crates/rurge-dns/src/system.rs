//! Access to the operating system's resolver configuration (design §7.1).
//! The binary implements the configuration (servers, search domains, hosts
//! path, IPv6) through `rurge-platform` and the resolver library through
//! tokio's `lookup_host`; tests use the static kinds.

use rurge_net::BoxFuture;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub trait SystemDns: Send + Sync {
    /// System DNS servers (port 53 unless the platform says otherwise).
    fn servers(&self) -> Vec<SocketAddr>;
    fn search_domains(&self) -> Vec<String>;
    fn hosts_path(&self) -> Option<PathBuf>;
    /// True when some interface has a global (non link-local, non unique-local) IPv6 address.
    fn has_ipv6(&self) -> bool;
    /// The system resolver library — what `getaddrinfo` answers from: the
    /// hosts file, mDNS / Bonjour, NetBIOS, and the system's DNS servers
    /// behind them. Names only the system knows are resolved here.
    ///
    /// Required, with no default body: an implementor that forgets it must
    /// not quietly get the real library. Only the executable's implementation
    /// calls the real one; the kinds here never do.
    fn library_lookup<'a>(&'a self, name: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>>;
    /// The enhanced mode is on: the system resolver library may answer with
    /// fake addresses (the system's DNS points at rurge, or `hijack-dns` takes
    /// its queries). The default is `false`.
    fn enhanced_mode(&self) -> bool {
        false
    }
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
    fn library_lookup<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(std::future::ready(Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no system resolver",
        ))))
    }
}

/// Fixed values (tests and CLI overrides).
#[derive(Clone, Debug, Default)]
pub struct StaticSystemDns {
    pub servers: Vec<SocketAddr>,
    pub search_domains: Vec<String>,
    pub hosts_path: Option<PathBuf>,
    pub has_ipv6: bool,
    /// The names the library knows and what it answers for each; a name is
    /// compared without regard to ASCII case. Any other name is not found.
    pub library: Vec<(String, Vec<IpAddr>)>,
    /// Every name the library was asked, in order: what reached it and what
    /// did not. Shared by the clones of this value, so a test keeps a handle
    /// to it before it hands the value to a resolver.
    pub library_asked: Arc<Mutex<Vec<String>>>,
    /// Whether the enhanced mode is on, as `enhanced_mode` reports it. Shared
    /// by the clones of this value, like `library_asked`.
    pub enhanced: Arc<AtomicBool>,
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
    fn enhanced_mode(&self) -> bool {
        self.enhanced.load(Ordering::Relaxed)
    }
    fn library_lookup<'a>(&'a self, name: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        self.library_asked
            .lock()
            .expect("library asked")
            .push(name.to_string());
        let found = self
            .library
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, addrs)| addrs.clone());
        Box::pin(std::future::ready(found.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no such host")
        })))
    }
}
