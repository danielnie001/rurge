//! rurge-specific runtime options (FR-CFG-17): command-line flags and
//! environment variables only, never profile keys. Also assembles the shared
//! objects (resource manager, set registry, GeoIP, resolver) the offline
//! commands need.

use super::switch;
use anyhow::Context;
use clap::Args;
use rurge_config::{Config, NetworkFacts};
use rurge_dns::cache::DEFAULT_CAPACITY;
use rurge_dns::system::SystemDns;
use rurge_net::BoxFuture;
use rurge_rules::GeoUrls;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

#[derive(Args, Clone, Debug)]
pub struct RuntimeArgs {
    /// Data directory for caches and databases (default: the platform data dir)
    #[arg(long, env = "RURGE_DATA_DIR", value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
    /// GeoIP country database URL (overrides geoip-maxmind-url)
    #[arg(
        long,
        env = "RURGE_GEOIP_URL",
        hide_env_values = true,
        value_name = "URL"
    )]
    pub geoip_url: Option<Url>,
    /// GeoIP ASN database URL
    #[arg(
        long,
        env = "RURGE_GEOIP_ASN_URL",
        hide_env_values = true,
        value_name = "URL"
    )]
    pub geoip_asn_url: Option<Url>,
    /// Never touch the network: use cached resources only
    #[arg(long, env = "RURGE_NO_NETWORK", value_parser = switch())]
    pub no_network: bool,
    /// DNS cache capacity in entries (default 2000)
    #[arg(long, env = "RURGE_DNS_CACHE_SIZE", value_name = "N")]
    pub dns_cache_size: Option<usize>,
    /// Where the network is read from, `file:<path>` (a JSON file instead of
    /// the machine): for automated tests, hence not in `--help`. It is
    /// `RURGE_NETWORK_BACKEND` as an option, for a `rurge` that an `external`
    /// policy starts, which gets no variable of the `RURGE_` kind: the
    /// policy's arguments can carry it. On the command line it takes the place
    /// of the variable, and a value that names nothing is refused as the
    /// variable's is (`network::backend`).
    #[arg(
        long,
        env = "RURGE_NETWORK_BACKEND",
        hide = true,
        value_name = "BACKEND"
    )]
    pub network_backend: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Runtime {
    pub data_dir: PathBuf,
    pub geo_urls: GeoUrls,
    pub no_network: bool,
    pub dns_cache_size: usize,
}

impl RuntimeArgs {
    /// Where the data directory is, whether or not it exists: `resolve`
    /// creates it, and needs a profile to do the rest of its work.
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir
            .clone()
            .unwrap_or_else(rurge_platform::dirs::data_dir)
    }

    pub fn resolve(&self, cfg: &Config) -> anyhow::Result<Runtime> {
        let data_dir = self.data_dir();
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("cannot create data dir {}", data_dir.display()))?;
        let mut geo_urls = GeoUrls::default();
        if let Some(u) = &cfg.general.geoip_maxmind_url {
            match Url::parse(u) {
                Ok(parsed) => geo_urls.country = parsed,
                // not quoted: MaxMind's own link carries the account's licence key
                Err(e) => eprintln!("warning: ignoring geoip-maxmind-url: {e}"),
            }
        }
        if let Some(u) = &self.geoip_url {
            geo_urls.country = u.clone();
        }
        if let Some(u) = &self.geoip_asn_url {
            geo_urls.asn = u.clone();
        }
        Ok(Runtime {
            data_dir,
            geo_urls,
            no_network: self.no_network,
            dns_cache_size: self.dns_cache_size.unwrap_or(DEFAULT_CAPACITY).max(1),
        })
    }
}

pub use rurge_engine::stack::Stack;
use rurge_engine::stack::{DnsOverride, StackOptions};

impl Runtime {
    /// Options for `rurge_engine::stack::build_stack` with the platform DNS adapter injected.
    pub fn stack_options(&self, wait: Duration) -> StackOptions {
        StackOptions {
            data_dir: self.data_dir.clone(),
            no_network: self.no_network,
            geo_urls: self.geo_urls.clone(),
            dns_cache_size: self.dns_cache_size,
            system: Arc::new(PlatformSystemDns),
            wait,
            // `Runtime::build` fills this in when the profile asks DNS to
            // follow the outbound mode; `check` / `rule match` / `dns lookup`
            // build the stack without an engine, so they stay direct.
            dns_connector: None,
            socket_hook: Arc::new(PlatformSockets),
            // `Runtime::build` works out the network's own; the commands that
            // build a stack without an engine set nothing unless they work
            // one out themselves.
            dns_override: DnsOverride::default(),
        }
    }
}

/// Builds resources → set registry → GeoIP → resolver, then waits up to
/// `wait` for the first fetch of every resource, and after that for what
/// arrived to be applied (skipped in `--no-network` mode). The resolver asks
/// the DNS servers that `[SSID Setting]` gives for `network`, if it gives any.
/// Delegates to `rurge_engine::stack::build_stack`.
pub async fn build_stack(
    cfg: &Config,
    rt: &Runtime,
    wait: Duration,
    network: &NetworkFacts,
) -> anyhow::Result<Stack> {
    rurge_engine::stack::build_stack(cfg, &stack_options_on(cfg, rt, wait, network)).await
}

/// `build_stack` with a hook that edits the resolver configuration before
/// the resolver is built (`dns lookup --server`). Delegates to
/// `rurge_engine::stack::build_stack_with`.
pub async fn build_stack_with(
    cfg: &Config,
    rt: &Runtime,
    wait: Duration,
    network: &NetworkFacts,
    customize: impl FnOnce(&mut rurge_dns::ResolverConfig),
) -> anyhow::Result<Stack> {
    rurge_engine::stack::build_stack_with(cfg, &stack_options_on(cfg, rt, wait, network), customize)
        .await
}

/// The options of a stack for a command that runs without an engine, on the
/// network it read: the DNS `[SSID Setting]` sets for it is the DNS of the
/// resolver, as it is for a generation of the daemon.
fn stack_options_on(
    cfg: &Config,
    rt: &Runtime,
    wait: Duration,
    network: &NetworkFacts,
) -> StackOptions {
    let mut options = rt.stack_options(wait);
    options.dns_override = DnsOverride::for_network(cfg, network);
    options
}

/// `rurge-platform::dns` behind the `SystemDns` trait, for the configuration
/// (AR-02: platform code stays in rurge-platform; rurge-dns only sees the
/// trait). The library call is tokio's `lookup_host`, not platform code.
pub struct PlatformSystemDns;

impl SystemDns for PlatformSystemDns {
    fn servers(&self) -> Vec<SocketAddr> {
        rurge_platform::dns::servers()
    }

    fn search_domains(&self) -> Vec<String> {
        rurge_platform::dns::search_domains()
    }

    fn hosts_path(&self) -> Option<PathBuf> {
        let path = rurge_platform::dns::hosts_path();
        path.is_file().then_some(path)
    }

    fn has_ipv6(&self) -> bool {
        rurge_platform::dns::has_ipv6()
    }

    fn library_lookup<'a>(&'a self, name: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move { tokio::net::lookup_host((name, 0)).await.map(usable) })
    }
}

/// What the library gave, as the addresses the resolver can hand on: in the
/// order given, each once. An IPv6 address with a scope id is left out — a
/// link-local `fe80::1%en0`, which is what mDNS often answers first: an
/// `IpAddr` has nowhere to keep the scope, and without it the address
/// cannot be dialled, so it would only be the first address to fail.
fn usable(addrs: impl Iterator<Item = SocketAddr>) -> Vec<IpAddr> {
    let mut found = Vec::new();
    for addr in addrs {
        if matches!(addr, SocketAddr::V6(v6) if v6.scope_id() != 0) {
            continue;
        }
        if !found.contains(&addr.ip()) {
            found.push(addr.ip());
        }
    }
    found
}

/// `rurge-platform::socket` behind the `SocketHook` trait (AR-02: platform
/// code stays in rurge-platform; rurge-net only sees the trait).
pub struct PlatformSockets;

fn platform_family(family: rurge_net::socket::Family) -> rurge_platform::socket::Family {
    match family {
        rurge_net::socket::Family::V4 => rurge_platform::socket::Family::V4,
        rurge_net::socket::Family::V6 => rurge_platform::socket::Family::V6,
    }
}

impl rurge_net::socket::SocketHook for PlatformSockets {
    fn bind_interface(
        &self,
        socket: &socket2::Socket,
        interface: &str,
        family: rurge_net::socket::Family,
    ) -> std::io::Result<()> {
        rurge_platform::socket::bind_interface(socket, interface, platform_family(family))
    }

    fn set_tos(
        &self,
        socket: &socket2::Socket,
        family: rurge_net::socket::Family,
        tos: u8,
    ) -> std::io::Result<()> {
        rurge_platform::socket::set_tos(socket, platform_family(family), tos)
    }
}

/// `rurge-platform::process` behind the `ProcessHook` trait (AR-02): an
/// `external` program leads a process group of its own on Unix and runs in
/// a Job Object on Windows, so it is stopped with whatever it started.
pub struct PlatformProcesses;

struct PlatformTree(rurge_platform::process::ProcessTree);

impl rurge_proto::external::ProcessGroup for PlatformTree {
    fn terminate(&mut self) -> std::io::Result<()> {
        self.0.terminate()
    }

    fn kill(&mut self) -> std::io::Result<()> {
        self.0.kill()
    }
}

impl rurge_proto::external::ProcessHook for PlatformProcesses {
    fn prepare(&self, command: &mut std::process::Command) {
        rurge_platform::process::prepare(command);
    }

    fn contain(
        &self,
        pid: u32,
    ) -> std::io::Result<Option<Box<dyn rurge_proto::external::ProcessGroup>>> {
        let tree = rurge_platform::process::ProcessTree::contain(pid)?;
        Ok(Some(Box::new(PlatformTree(tree))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::socket::{Family, SocketHook};
    use rurge_proto::external::ProcessHook;

    /// Where the two GeoIP databases are fetched from: `geoip-maxmind-url`
    /// for the country one (manual, IP Rules), the command line over the
    /// profile, the defaults when nobody says. The profile's key is the only
    /// way to the country database that is not a command-line flag, and no
    /// other test follows it.
    #[test]
    fn the_geoip_links_are_the_profiles_and_the_command_lines_over_it() {
        let dir = tempfile::tempdir().unwrap();
        let load = |general: &str| {
            let text = format!("[General]\n{general}\n[Rule]\nFINAL,DIRECT\n");
            let loaded = rurge_config::config::from_text(
                &text,
                std::path::Path::new("t.conf"),
                &rurge_config::config::LoadOptions::for_tests(),
            );
            assert!(!loaded.diagnostics.has_errors());
            loaded.config
        };
        let args = |country: Option<&str>, asn: Option<&str>| RuntimeArgs {
            data_dir: Some(dir.path().to_path_buf()),
            geoip_url: country.map(|u| Url::parse(u).unwrap()),
            geoip_asn_url: asn.map(|u| Url::parse(u).unwrap()),
            no_network: true,
            dns_cache_size: None,
            network_backend: None,
        };
        let defaults = GeoUrls::default();
        let nobody = args(None, None).resolve(&load("")).unwrap().geo_urls;
        assert_eq!(
            (nobody.country.as_str(), nobody.asn.as_str()),
            (defaults.country.as_str(), defaults.asn.as_str())
        );
        let profile = load("geoip-maxmind-url = https://geo.example/country.tar.gz");
        let from_profile = args(None, None).resolve(&profile).unwrap().geo_urls;
        assert_eq!(
            from_profile.country.as_str(),
            "https://geo.example/country.tar.gz"
        );
        assert_eq!(from_profile.asn, defaults.asn, "the profile has no say");
        let from_line = args(
            Some("https://line.example/country.mmdb"),
            Some("https://line.example/asn.mmdb"),
        )
        .resolve(&profile)
        .unwrap()
        .geo_urls;
        assert_eq!(
            (from_line.country.as_str(), from_line.asn.as_str()),
            (
                "https://line.example/country.mmdb",
                "https://line.example/asn.mmdb"
            )
        );
    }

    /// A test that calls the real system resolver library, and only for the
    /// one name every system answers from its own tables, without a packet.
    /// The wait has a limit so that a slow library fails the test with a
    /// message; the call itself runs on the blocking pool and is not cut
    /// short, so a library that never returned would still hold the test's
    /// runtime at its end.
    #[tokio::test]
    async fn the_platform_library_answers_localhost_with_loopback_addresses() {
        let found = tokio::time::timeout(
            Duration::from_secs(30),
            PlatformSystemDns.library_lookup("localhost"),
        )
        .await
        .expect("the system resolver library did not answer for localhost within 30 seconds")
        .expect("localhost resolves");
        assert!(!found.is_empty());
        assert!(found.iter().all(IpAddr::is_loopback), "{found:?}");
    }

    /// What the library gave, as addresses the resolver can use: in the
    /// order given, each once, and none that cannot be dialled as an
    /// `IpAddr`.
    #[test]
    fn a_library_answer_keeps_its_order_and_loses_scoped_and_repeated_addresses() {
        use std::net::{SocketAddrV4, SocketAddrV6};
        let v4 = |text: &str| SocketAddr::V4(SocketAddrV4::new(text.parse().unwrap(), 0));
        let v6 = |text: &str, scope: u32| {
            SocketAddr::V6(SocketAddrV6::new(text.parse().unwrap(), 0, 0, scope))
        };
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();

        assert_eq!(
            usable([v4("192.0.2.1"), v6("2001:db8::1", 0)].into_iter()),
            [ip("192.0.2.1"), ip("2001:db8::1")]
        );
        assert_eq!(
            usable([v6("2001:db8::1", 0), v4("192.0.2.1")].into_iter()),
            [ip("2001:db8::1"), ip("192.0.2.1")],
            "the order is the library's"
        );
        assert_eq!(
            usable([v6("fe80::1", 4), v4("192.0.2.1"), v6("fe80::2", 4)].into_iter()),
            [ip("192.0.2.1")],
            "a link-local address with its scope id cannot be dialled"
        );
        assert_eq!(
            usable([v4("192.0.2.1"), v6("2001:db8::1", 0), v4("192.0.2.1")].into_iter()),
            [ip("192.0.2.1"), ip("2001:db8::1")],
            "an address given twice comes once"
        );
        assert!(usable([v6("fe80::1", 4), v6("fe80::1", 9)].into_iter()).is_empty());
    }

    /// Socket-level only: nothing about this machine's network is changed.
    #[test]
    fn platform_sockets_delegates_to_rurge_platform() {
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        let hook = PlatformSockets;
        assert!(
            hook.bind_interface(&socket, "rurge-no-such-if0", Family::V4)
                .is_err()
        );
        hook.set_tos(&socket, Family::V4, 0x28).unwrap();
        assert_eq!(socket.tos_v4().unwrap(), 0x28);
    }

    /// The adapter prepares the command, takes in the child it started and
    /// ends it through `rurge-platform::process`; the test starts nothing but
    /// its own child.
    #[test]
    fn platform_processes_delegates_to_rurge_platform() {
        let hook = PlatformProcesses;
        #[cfg(windows)]
        let mut command = {
            let mut c = std::process::Command::new("cmd");
            c.args(["/c", "ping -n 30 127.0.0.1"]);
            c
        };
        #[cfg(unix)]
        let mut command = std::process::Command::new("sleep");
        #[cfg(unix)]
        command.arg("30");
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null());
        hook.prepare(&mut command);
        let mut child = command.spawn().unwrap();
        let mut group = hook.contain(child.id()).unwrap().expect("a group");
        group.kill().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                panic!("the program is still running");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
