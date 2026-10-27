//! rurge-specific runtime options (FR-CFG-17): command-line flags and
//! environment variables only, never profile keys. Also assembles the shared
//! objects (resource manager, set registry, GeoIP, resolver) the offline
//! commands need.

use super::enhanced::EgressSlot;
use super::switch;
use anyhow::Context;
use clap::Args;
use rurge_config::session::{ProcessInfo, Transport};
use rurge_config::{Config, NetworkFacts};
use rurge_dns::cache::DEFAULT_CAPACITY;
use rurge_dns::system::SystemDns;
use rurge_engine::{OwnerLookup, OwnerQuery};
use rurge_net::BoxFuture;
use rurge_rules::GeoUrls;
use std::ffi::OsStr;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
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
    /// Where rurge's own sockets go while the enhanced mode is on: the same
    /// slot every generation's hook reads, and the one the manager writes.
    pub egress: EgressSlot,
    /// `rurge-platform::socket` behind `SocketHook`, shared by every
    /// generation: each one sees the same egress table (M4 design 7.1).
    pub sockets: Arc<PlatformSockets>,
    /// `rurge-platform::dns` behind `SystemDns`, shared by every generation
    /// and by what this run downloads: all of them see the same flag (M4
    /// design 6.3).
    pub system_dns: Arc<PlatformSystemDns>,
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
        let egress = super::enhanced::egress_slot();
        Ok(Runtime {
            data_dir,
            geo_urls,
            no_network: self.no_network,
            dns_cache_size: self.dns_cache_size.unwrap_or(DEFAULT_CAPACITY).max(1),
            sockets: Arc::new(PlatformSockets::new(egress.clone())),
            system_dns: Arc::new(PlatformSystemDns::new()),
            egress,
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
            system: self.system_dns.clone(),
            wait,
            // `Runtime::build` fills this in when the profile asks DNS to
            // follow the outbound mode; `check` / `rule match` / `dns lookup`
            // build the stack without an engine, so they stay direct.
            dns_connector: None,
            socket_hook: self.sockets.clone(),
            resource_resolve: None,
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
///
/// The manager tells it whether the enhanced mode is on: while it is, the
/// system's DNS may be the responder's, and what the system's own servers
/// were is not what a query should ask (M4 design 6.3).
#[derive(Debug, Default)]
pub struct PlatformSystemDns {
    /// Up while the enhanced mode's egress table is written (the manager says).
    enhanced: Arc<AtomicBool>,
    /// The servers the system had before the takeover, once something has
    /// written them: the manager writes them from the platform backend's
    /// `physical_dns` while the enhanced mode is on (M4c plan P13): macOS's
    /// backup of each service's servers or of what DHCP gave, Linux's resolved
    /// upstream list. Windows needs none (M4 design 6.3).
    physical: Arc<RwLock<Option<Vec<SocketAddr>>>>,
}

impl PlatformSystemDns {
    /// Nothing taken over: the machine's own configuration is the answer.
    pub fn new() -> PlatformSystemDns {
        PlatformSystemDns::default()
    }

    /// The flag the manager sets while the enhanced mode's egress table is in
    /// its slot, and clears when it takes the table away.
    pub fn enhanced(&self) -> Arc<AtomicBool> {
        self.enhanced.clone()
    }

    /// The servers of the network outside the enhanced mode, which the manager
    /// writes as the DNS changes hands (M4 design 6.3).
    pub fn physical(&self) -> Arc<RwLock<Option<Vec<SocketAddr>>>> {
        self.physical.clone()
    }
}

impl SystemDns for PlatformSystemDns {
    fn servers(&self) -> Vec<SocketAddr> {
        if self.enhanced.load(Ordering::SeqCst)
            && let Some(physical) = self
                .physical
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        {
            return physical;
        }
        rurge_platform::dns::servers()
    }

    fn enhanced_mode(&self) -> bool {
        self.enhanced.load(Ordering::SeqCst)
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

/// `rurge-platform::socket` behind the `SocketHook` trait, and where rurge's
/// own sockets go while the enhanced mode is on (AR-02: platform code stays in
/// rurge-platform; rurge-net only sees the trait). The manager writes the
/// egress table; every generation's hook reads the same slot.
#[derive(Debug)]
pub struct PlatformSockets {
    egress: EgressSlot,
}

impl PlatformSockets {
    pub fn new(egress: EgressSlot) -> PlatformSockets {
        PlatformSockets { egress }
    }
}

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

    /// The manager's table, read once for this connection. The table is only
    /// in the slot while the enhanced mode is on; no line is logged here, since
    /// `rurge-net`'s `bind_egress` says the destination and the interface at
    /// DEBUG already.
    fn egress(&self, dst: IpAddr) -> io::Result<Option<String>> {
        let table = self.egress.read().unwrap_or_else(|e| e.into_inner());
        match rurge_platform::socket::egress::decide(table.as_deref(), dst) {
            Ok(Some(name)) => Ok(Some(name.to_string())),
            Ok(None) => Ok(None),
            Err(rurge_platform::socket::egress::NoRoute) => Err(io::Error::new(
                io::ErrorKind::NetworkUnreachable,
                format!("no network outside the enhanced mode to reach {dst}"),
            )),
        }
    }

    /// The table `egress` answers from, as a route: `None` while the enhanced
    /// mode is off. The manager writes a table again only when its content
    /// changes, so the same `Arc` says the same egress and a socket that
    /// outlives one connection — a WireGuard carrier, a QUIC connection — is
    /// made again only when it really moved (the phase 3 / M4b plan's
    /// execution record rows 68 and 69).
    fn egress_route(&self) -> Option<rurge_net::connector::Route> {
        let table = self.egress.read().unwrap_or_else(|e| e.into_inner());
        table.as_ref().map(rurge_net::connector::Route::egress)
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

    fn contains(&self, pid: u32) -> bool {
        self.0.contains(pid)
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

/// `rurge-platform::procinfo` behind the `OwnerLookup` trait (AR-02): the
/// process that owns the socket a session came from, found in the tables the
/// system keeps. `rurge-platform` knows no other crate of rurge, so what is
/// asked and what is found are translated here.
#[derive(Default)]
pub struct PlatformOwners(rurge_platform::procinfo::Finder);

impl PlatformOwners {
    pub fn new() -> PlatformOwners {
        PlatformOwners::default()
    }
}

/// The question of the engine, as the platform is asked it.
fn platform_query(query: &OwnerQuery) -> rurge_platform::procinfo::Query {
    rurge_platform::procinfo::Query {
        protocol: match query.transport {
            Transport::Tcp => rurge_platform::procinfo::Protocol::Tcp,
            Transport::Udp => rurge_platform::procinfo::Protocol::Udp,
        },
        local: query.local,
        remote: query.remote,
        since: query.since,
    }
}

/// What the platform found, as the engine knows a process: its PID is known.
fn process_info(owner: rurge_platform::procinfo::Owner) -> ProcessInfo {
    ProcessInfo {
        pid: Some(owner.pid),
        name: owner.name,
        path: owner.path,
    }
}

impl OwnerLookup for PlatformOwners {
    fn is_local(&self, ip: IpAddr) -> Option<bool> {
        self.0.is_local(ip)
    }

    fn owner(&self, query: &OwnerQuery) -> Option<ProcessInfo> {
        self.0.owner(&platform_query(query)).map(process_info)
    }
}

/// Whether the processes behind the sessions are looked up, as
/// `RURGE_PROCESS_BACKEND` says: in the machine's own tables when it is not
/// set, not at all when it says `off`. The variable is for automated tests,
/// so that the daemons they start do not read the tables of the machine for
/// sessions that are not about processes. A value that names nothing is an
/// error, and never the machine's lookup — as a `RURGE_NETWORK_BACKEND` that
/// names nothing is never the machine's network.
pub fn owners_backend(value: Option<&OsStr>) -> Result<bool, String> {
    match value {
        None => Ok(true),
        Some(value) if value == "off" => Ok(false),
        Some(value) => Err(format!(
            "unknown RURGE_PROCESS_BACKEND value `{}` (expected off)",
            value.to_string_lossy()
        )),
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
            PlatformSystemDns::new().library_lookup("localhost"),
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
    /// The hook takes the egress slot, which no manager has written here, so
    /// every decision is the system's own.
    #[test]
    fn platform_sockets_delegates_to_rurge_platform() {
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        let hook = PlatformSockets::new(super::super::enhanced::egress_slot());
        assert!(
            hook.bind_interface(&socket, "rurge-no-such-if0", Family::V4)
                .is_err()
        );
        hook.set_tos(&socket, Family::V4, 0x28).unwrap();
        assert_eq!(socket.tos_v4().unwrap(), 0x28);
        assert_eq!(
            hook.egress("192.0.2.5".parse().unwrap()).unwrap(),
            None,
            "no manager has written a table"
        );
    }

    /// The socket hook's egress is the manager's table: no table while the
    /// enhanced mode is off, the interface the table names inside its routes,
    /// and an error outside them when the table names no primary interface.
    #[test]
    fn platform_sockets_egress_follows_the_table() {
        use rurge_platform::route::RouteSet;
        use rurge_platform::socket::egress::EgressTable;

        let egress = super::super::enhanced::egress_slot();
        let hook = PlatformSockets::new(egress.clone());
        let inside: IpAddr = "10.1.2.3".parse().unwrap();
        let outside: IpAddr = "192.0.2.5".parse().unwrap();
        assert_eq!(
            hook.egress(inside).unwrap(),
            None,
            "the enhanced mode is off"
        );
        let routes = RouteSet {
            v4: vec!["10.0.0.0/8".parse().unwrap()],
            v6: Vec::new(),
        };

        *egress.write().unwrap() = Some(Arc::new(EgressTable {
            routes: routes.clone(),
            primary: Some("eth0".to_string()),
            subnets: Vec::new(),
        }));
        assert_eq!(hook.egress(inside).unwrap().as_deref(), Some("eth0"));
        assert_eq!(hook.egress(outside).unwrap(), None, "outside the routes");

        // a table without a primary interface reaches nothing in its routes
        *egress.write().unwrap() = Some(Arc::new(EgressTable {
            routes,
            primary: None,
            subnets: Vec::new(),
        }));
        let error = hook.egress(inside).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NetworkUnreachable);
        assert_eq!(
            error.to_string(),
            "no network outside the enhanced mode to reach 10.1.2.3"
        );

        // and the empty slot again, as a close leaves it
        *egress.write().unwrap() = None;
        assert_eq!(hook.egress(inside).unwrap(), None);
    }

    /// The hook also says which egress table it answers `egress` from, for a
    /// socket that outlives one connection: nothing while the enhanced mode
    /// is off, one route while it is the same table, another once the manager
    /// has written another (the phase 3 / M4b plan's execution record row
    /// 68).
    #[test]
    fn platform_sockets_say_which_egress_table_they_answer_from() {
        use rurge_platform::route::RouteSet;
        use rurge_platform::socket::egress::EgressTable;

        let egress = super::super::enhanced::egress_slot();
        let hook = PlatformSockets::new(egress.clone());
        assert!(hook.egress_route().is_none(), "the enhanced mode is off");

        let table = |first: &str| {
            Some(Arc::new(EgressTable {
                routes: RouteSet {
                    v4: vec![first.parse().unwrap()],
                    v6: Vec::new(),
                },
                primary: Some("eth0".to_string()),
                subnets: Vec::new(),
            }))
        };
        *egress.write().unwrap() = table("10.0.0.0/8");
        let first = hook.egress_route().expect("the table is the route");
        assert!(first.is_egress());
        assert_eq!(
            Some(first.clone()),
            hook.egress_route(),
            "the same table twice"
        );

        *egress.write().unwrap() = table("10.9.0.0/16");
        assert_ne!(
            hook.egress_route(),
            Some(first.clone()),
            "another table is another route"
        );

        *egress.write().unwrap() = None;
        assert!(hook.egress_route().is_none(), "the table is gone again");
    }

    /// The system DNS adapter is in the enhanced mode while the manager's flag
    /// is up, and the servers it gives are the backup's then and the machine's
    /// own otherwise. Nothing here sends a packet: `servers()` reads this
    /// machine's configuration, and what it holds is not asserted (it differs
    /// from machine to machine).
    #[test]
    fn platform_system_dns_is_in_the_enhanced_mode_while_the_flag_is_up() {
        use std::sync::atomic::Ordering;

        let dns = PlatformSystemDns::new();
        let machine = || rurge_platform::dns::servers();
        assert!(
            !dns.enhanced_mode(),
            "the enhanced mode is off at the start"
        );
        assert_eq!(
            dns.servers(),
            machine(),
            "the machine's own while it is off"
        );

        // what M4c's manager will write: the servers that were there
        let backup = vec!["192.0.2.53:53".parse::<SocketAddr>().unwrap()];
        *dns.physical().write().unwrap() = Some(backup.clone());
        assert_eq!(
            dns.servers(),
            machine(),
            "the backup is not the answer while the mode is off"
        );
        dns.enhanced().store(true, Ordering::SeqCst);
        assert!(dns.enhanced_mode());
        assert_eq!(dns.servers(), backup, "the backup's while it is on");

        // the flag the manager holds is the one this adapter reads
        dns.enhanced().store(false, Ordering::SeqCst);
        assert!(!dns.enhanced_mode());
        assert_eq!(dns.servers(), machine());

        // with no backup, the machine's own even while the mode is on
        *dns.physical().write().unwrap() = None;
        dns.enhanced().store(true, Ordering::SeqCst);
        assert_eq!(dns.servers(), machine());
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

    /// `RURGE_PROCESS_BACKEND` is not set, or it says `off`. Nothing else
    /// names a backend, and a value that does not is an error and never the
    /// machine's own lookup: a typo in a test must not make a start look at
    /// the real processes. Only the exact word counts, and a value that is
    /// not text is written the way it reads as text.
    #[test]
    fn the_process_backend_values() {
        let backend = |value: &str| owners_backend(Some(OsStr::new(value)));

        assert_eq!(owners_backend(None), Ok(true), "not set: the machine's own");
        assert_eq!(backend("off"), Ok(false));
        for value in ["OFF", "Off", "off ", "on", "bogus", ""] {
            assert_eq!(
                backend(value),
                Err(format!(
                    "unknown RURGE_PROCESS_BACKEND value `{value}` (expected off)"
                )),
                "{value:?}"
            );
        }

        #[cfg(unix)]
        let not_text = {
            use std::os::unix::ffi::OsStrExt;
            OsStr::from_bytes(b"of\xff").to_os_string()
        };
        #[cfg(windows)]
        let not_text = {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[0x6f, 0x66, 0xd800])
        };
        assert_eq!(
            owners_backend(Some(&not_text)),
            Err("unknown RURGE_PROCESS_BACKEND value `of\u{fffd}` (expected off)".to_string())
        );
    }

    /// The engine's question reaches the platform as it was asked, for either
    /// table and with or without the other end of the connection the client
    /// made, and what the platform finds is the engine's answer with the
    /// process's PID. A process the platform could only name has no path.
    #[test]
    fn the_owner_lookup_asks_as_it_was_asked_and_answers_with_the_pid() {
        use rurge_platform::procinfo::{Owner, Protocol};

        let since = std::time::Instant::now();
        let local: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        for (transport, protocol) in [
            (Transport::Tcp, Protocol::Tcp),
            (Transport::Udp, Protocol::Udp),
        ] {
            for remote in [Some("127.0.0.1:7890".parse::<SocketAddr>().unwrap()), None] {
                let asked = platform_query(&OwnerQuery {
                    transport,
                    local,
                    remote,
                    since,
                });
                assert_eq!(asked.protocol, protocol, "{transport:?}");
                assert_eq!(asked.local, local);
                assert_eq!(asked.remote, remote);
                assert_eq!(asked.since, since);
            }
        }

        let described = process_info(Owner {
            pid: 4321,
            name: "app.exe".to_string(),
            path: Some("C:\\Apps\\app.exe".to_string()),
        });
        assert_eq!(described.pid, Some(4321));
        assert_eq!(described.name, "app.exe");
        assert_eq!(described.path.as_deref(), Some("C:\\Apps\\app.exe"));
        let named = process_info(Owner {
            pid: 4,
            name: "System".to_string(),
            path: None,
        });
        assert_eq!(named.pid, Some(4));
        assert_eq!(named.name, "System");
        assert_eq!(named.path, None);
    }
}
