//! The resolver (design §7): upstream selection, bootstrap, cache, `[Host]`
//! chain, hosts file, special hostnames, AAAA suppression and in-flight
//! coalescing. Implements `LazyResolver` (rule engine) and `Resolve`
//! (connectors) so every other crate resolves through it.

use crate::bootstrap::{Bootstrap, BootstrapConnector};
use crate::cache::{CacheEntry, CacheHit, CachedAddrs, DEFAULT_CAPACITY, DnsCache};
use crate::fanout::{Answers, FanoutError, FanoutOpts, resolve_name};
use crate::hosts::{HostAction, HostMap, parse_hosts_file};
use crate::system::SystemDns;
use crate::upstream::doh::DohUpstream;
use crate::upstream::doh3::Doh3Upstream;
use crate::upstream::doq::DoqUpstream;
use crate::upstream::tcp::TcpUpstream;
use crate::upstream::udp::UdpUpstream;
use crate::upstream::{UpstreamRef, UpstreamSpec};
use arc_swap::ArcSwap;
use rurge_config::general::{DnsServer, EncryptedDns};
use rurge_config::host::HostEntry;
use rurge_config::{Config, Diagnostic, Diagnostics, codes};
use rurge_net::BoxFuture;
use rurge_net::connector::{Connector, Resolve, Via};
use rurge_net::http::{HttpClient, HttpClientConfig, tls_client_config};
use rurge_net::resource::{
    ResourceKind, ResourceManager, ResourceSource, ResourceSpec, ResourceState,
};
use rurge_rules::engine::{LazyResolver, ResolveError};
use rurge_rules::matcher::ResolvedAddrs;
use rurge_rules::registry::SetRegistry;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;

pub const MAX_ALIAS_HOPS: usize = 8;
pub const AAAA_SUPPRESS_AFTER: u32 = 5;

#[derive(Clone, Debug)]
pub struct ResolverConfig {
    pub servers: Vec<DnsServer>,
    pub encrypted: Vec<EncryptedDns>,
    pub skip_cert_verification: bool,
    pub ipv6: bool,
    pub hosts: Vec<HostEntry>,
    pub read_etc_hosts: bool,
    pub proxy_hostnames: HashSet<String>,
    pub cache_capacity: usize,
    pub fanout: FanoutOpts,
}

impl ResolverConfig {
    pub fn from_config(cfg: &Config) -> ResolverConfig {
        ResolverConfig {
            servers: cfg.general.dns_server.clone(),
            encrypted: cfg.general.encrypted_dns_server.clone(),
            skip_cert_verification: cfg.general.encrypted_dns_skip_cert_verification,
            ipv6: cfg.general.ipv6,
            hosts: cfg.hosts.clone(),
            read_etc_hosts: cfg.general.read_etc_hosts,
            proxy_hostnames: cfg.proxy_hostnames(),
            cache_capacity: DEFAULT_CAPACITY,
            fanout: FanoutOpts::default(),
        }
    }
}

/// Who dials for the upstreams that go through a connector, one per kind of
/// upstream: whoever stands behind them knows what a connection carries.
#[derive(Clone)]
pub struct DnsConnectors {
    /// `tcp://` servers.
    pub tcp: Arc<dyn Connector>,
    /// `tls://` servers.
    pub tls: Arc<dyn Connector>,
    /// `https://` servers: the connections of the DoH client.
    pub https: Arc<dyn Connector>,
    /// `quic://` servers: the carriers of their QUIC connections.
    pub quic: Arc<dyn Connector>,
    /// `h3://` servers: the carriers of their QUIC connections.
    pub h3: Arc<dyn Connector>,
}

impl DnsConnectors {
    /// The same connector for every kind.
    pub fn all(connector: Arc<dyn Connector>) -> DnsConnectors {
        DnsConnectors {
            tcp: connector.clone(),
            tls: connector.clone(),
            https: connector.clone(),
            quic: connector.clone(),
            h3: connector,
        }
    }
}

pub struct ResolverDeps {
    pub connectors: DnsConnectors,
    pub sets: Arc<SetRegistry>,
    pub system: Arc<dyn SystemDns>,
    pub resources: Arc<ResourceManager>,
}

#[derive(Clone, Debug, Default)]
pub struct LookupOpts {
    pub bypass_cache: bool,
    /// `None` = follow the profile (`ipv6`) and the network (`has_ipv6`).
    pub want_v6: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKind {
    Ip,
    Alias,
    Server,
    System,
    EtcHosts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Literal,
    Loopback,
    Host(HostKind),
    Cache { stale: bool },
    Upstream(String),
    System,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Literal => f.write_str("literal"),
            Source::Loopback => f.write_str("loopback"),
            Source::Host(k) => match k {
                HostKind::Ip => f.write_str("host(ip)"),
                HostKind::Alias => f.write_str("host(alias)"),
                HostKind::Server => f.write_str("host(server)"),
                HostKind::System => f.write_str("host(system)"),
                HostKind::EtcHosts => f.write_str("hosts-file"),
            },
            Source::Cache { stale: false } => f.write_str("cache"),
            Source::Cache { stale: true } => f.write_str("cache(stale)"),
            Source::Upstream(u) => write!(f, "upstream({u})"),
            Source::System => f.write_str("system"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DnsResult {
    pub v4: Vec<Ipv4Addr>,
    pub v6: Vec<Ipv6Addr>,
    pub ttl: Duration,
    pub source: Source,
    pub elapsed: Duration,
}

impl DnsResult {
    pub fn addrs(&self) -> Vec<IpAddr> {
        self.v4
            .iter()
            .map(|a| IpAddr::V4(*a))
            .chain(self.v6.iter().map(|a| IpAddr::V6(*a)))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnsError {
    Timeout,
    EmptyAnswer,
    AllFailed(Vec<(String, String)>),
    Bootstrap(String),
    NoUpstream,
    AliasLoop(String),
    Unsupported(String),
}

impl fmt::Display for DnsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DnsError::Timeout => f.write_str("timeout"),
            DnsError::EmptyAnswer => f.write_str("empty answer"),
            DnsError::AllFailed(list) => {
                let parts: Vec<String> = list.iter().map(|(u, e)| format!("{u}: {e}")).collect();
                write!(f, "all upstreams failed ({})", parts.join("; "))
            }
            DnsError::Bootstrap(m) => write!(f, "bootstrap: {m}"),
            DnsError::NoUpstream => f.write_str("no upstream configured"),
            DnsError::AliasLoop(n) => write!(f, "[Host] alias chain too long at `{n}`"),
            DnsError::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for DnsError {}

impl From<FanoutError> for DnsError {
    fn from(e: FanoutError) -> DnsError {
        match e {
            FanoutError::EmptyAnswer => DnsError::EmptyAnswer,
            FanoutError::Timeout => DnsError::Timeout,
            FanoutError::AllFailed(list) if list.is_empty() => DnsError::NoUpstream,
            FanoutError::AllFailed(list) => DnsError::AllFailed(list),
        }
    }
}

#[derive(Clone, Debug)]
pub struct UpstreamDelay {
    pub upstream: String,
    pub result: Result<Duration, String>,
}

type Shared = Result<Answers, DnsError>;
type InflightMap = Mutex<HashMap<String, watch::Sender<Option<Shared>>>>;

/// Owns one in-flight entry for the duration of the query. `publish` hands the
/// answer to the waiters and removes the entry; `Drop` removes an entry that was
/// never published, so a caller whose future is cancelled mid-query drops the
/// sender instead of leaving the waiters parked forever.
struct Inflight<'a> {
    map: &'a InflightMap,
    key: String,
    published: bool,
}

impl Inflight<'_> {
    fn publish(&mut self, result: &Shared) {
        self.published = true;
        if let Some(tx) = self.map.lock().expect("inflight").remove(&self.key) {
            let _ = tx.send(Some(result.clone()));
        }
    }
}

impl Drop for Inflight<'_> {
    fn drop(&mut self) {
        // After `publish` the entry is gone and another caller may already have
        // inserted its own under the same key, so only an unpublished query
        // removes anything here.
        if !self.published {
            self.map.lock().expect("inflight").remove(&self.key);
        }
    }
}

/// What a caller does with an AAAA answer that timed out, for AAAA
/// suppression. The manual (DNS Overview: "5 consecutive lookups receive an
/// A answer while the AAAA answer times out") draws its conclusion about the
/// network from five observations of it, so a timeout is counted once, by
/// the caller whose query it was. It is another's to count in two cases:
///
/// - The query is the background one that asks again for the AAAA answer a
///   lookup did not wait for: the lookup counted its own timeout.
/// - The result is that of a query another caller sent and this one waited
///   for (`query_coalesced`). Lookups of one name at once share a query and
///   so a timeout; counted by each of them, one slow AAAA answer to a browser
///   that opens several connections to a host it has not looked up yet would
///   turn AAAA off for the whole process.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AaaaTimeout {
    /// The query was sent for its caller's own sake and is the only one made
    /// for its lookup, so its timeout is the caller's to count.
    Count,
    /// The timeout is not this caller's to count. Its owner counts it — except
    /// where the owner is itself a query that counts nothing (a lookup that
    /// comes to wait on the background query that asks again, or on a lookup
    /// by the system's servers): the count is then one short.
    Ignore,
}

/// Whose query a result of `query_coalesced` came from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Query {
    /// The caller's: it sent the query.
    Own,
    /// Another caller's, whose answer the caller waited for.
    Another,
}

impl Query {
    /// What a caller does with the AAAA timeout of a result of this query,
    /// given what it does with one of its own.
    fn timeout(self, own: AaaaTimeout) -> AaaaTimeout {
        match self {
            Query::Own => own,
            Query::Another => AaaaTimeout::Ignore,
        }
    }
}

/// What the lookups through one `Via` ask (`dns-follow-interface`, phase 2
/// M5 design 8.5): the plain servers, through its connector, with answers of
/// their own.
struct ViaSet {
    upstreams: Arc<Vec<UpstreamRef>>,
    cache: DnsCache,
}

pub struct Resolver {
    cfg: ResolverConfig,
    /// The connector of each kind of upstream, behind the bootstrap.
    connectors: DnsConnectors,
    system: Arc<dyn SystemDns>,
    bootstrap: Arc<Bootstrap>,
    /// `None` when the internal HTTP client could not be built (W0026 was
    /// recorded); DoH upstreams are then skipped, like DoT without `tls`.
    http: Option<Arc<HttpClient>>,
    tls: Option<Arc<rustls::ClientConfig>>,
    /// The client configuration of the DoQ upstreams; `None` when it could
    /// not be built (W0026 was recorded): they are skipped then, as DoH is.
    doq: Option<quinn::ClientConfig>,
    /// The same of the DoH3 upstreams.
    doh3: Option<quinn::ClientConfig>,
    /// The `dns-server` UDP entries that survived the IPv6 filter.
    configured_udp: Vec<UpstreamSpec>,
    /// The profile wants the platform's servers: it wrote `dns-server = system`,
    /// or it configured no traditional server at all. The system list is
    /// re-expanded on every network change rather than snapshotted here.
    wants_system: bool,
    /// The encrypted subsystem (`https://` / `h3://` / `quic://` / `tls://` /
    /// `tcp://`). It owns the primary set whenever it is non-empty and never
    /// depends on the network.
    encrypted_specs: Vec<UpstreamSpec>,
    primary: ArcSwap<Vec<UpstreamRef>>,
    system_upstreams: ArcSwap<Vec<UpstreamRef>>,
    host_upstreams: Mutex<HashMap<Vec<UpstreamSpec>, Arc<Vec<UpstreamRef>>>>,
    cache: DnsCache,
    hosts: HostMap,
    inflight: InflightMap,
    aaaa_failures: AtomicU32,
    aaaa_suppressed: AtomicBool,
    has_ipv6: AtomicBool,
    self_weak: Mutex<Weak<Resolver>>,
    /// The upstream sets of the lookups through a `Via`, by its key.
    vias: Mutex<HashMap<String, Arc<ViaSet>>>,
    /// Said once: lookups through a `Via` do not follow it with encrypted
    /// DNS configured.
    via_unfollowed: AtomicBool,
    /// Said once: lookups through a `Via` go as usual when there is no
    /// plain server to ask through it.
    via_serverless: AtomicBool,
}

impl Resolver {
    /// Must be called inside a tokio runtime: it arms the hosts-file watcher.
    pub fn new(cfg: ResolverConfig, deps: ResolverDeps) -> (Arc<Resolver>, Diagnostics) {
        let mut diags = Diagnostics::default();
        let has_ipv6 = deps.system.has_ipv6();

        // Traditional (plain UDP) servers; `system` expands to the platform's
        // list, which `on_network_change` re-expands.
        let mut configured_udp: Vec<UpstreamSpec> = Vec::new();
        let mut wants_system = false;
        for s in &cfg.servers {
            match UpstreamSpec::from_dns_server(s) {
                Some(UpstreamSpec::Udp(addr)) if addr.is_ipv6() && !cfg.ipv6 => {
                    tracing::info!(server = %addr, "IPv6 DNS server ignored because ipv6 is off");
                }
                Some(UpstreamSpec::Udp(addr)) => configured_udp.push(UpstreamSpec::Udp(addr)),
                // `from_dns_server` only ever yields a plain UDP server or
                // `None`; the arm keeps the invariant `build_plain_udp` relies
                // on enforced by the match rather than by convention.
                Some(other) => {
                    tracing::warn!(server = ?other, "ignoring non-UDP dns-server entry")
                }
                None => wants_system = true,
            }
        }
        // No usable traditional server of its own: fall back to the platform's.
        wants_system |= configured_udp.is_empty();
        let system_specs: Vec<UpstreamSpec> = deps
            .system
            .servers()
            .into_iter()
            .map(UpstreamSpec::Udp)
            .collect();

        // Encrypted subsystem: https:// h3:// quic:// tls:// tcp://
        let mut encrypted: Vec<UpstreamSpec> = Vec::new();
        for e in &cfg.encrypted {
            match UpstreamSpec::from_encrypted(e) {
                Ok(spec) => encrypted.push(spec),
                Err(msg) => diags.push(Diagnostic::warning(codes::W_DNS_UPSTREAM_UNSUPPORTED, msg)),
            }
        }
        if cfg.skip_cert_verification {
            tracing::warn!(
                "encrypted-dns-skip-cert-verification is on: encrypted DNS certificates are not verified"
            );
        }

        // Populated below by `apply_system_servers`, the single place that
        // derives every network-dependent upstream set.
        let bootstrap = Bootstrap::new(Vec::new(), cfg.ipv6 && has_ipv6, cfg.fanout.clone());
        // One bootstrap for all of them: a server written as a name is
        // resolved the same way, whichever kind of server it is.
        let behind_bootstrap = |inner: &Arc<dyn Connector>| -> Arc<dyn Connector> {
            Arc::new(BootstrapConnector::new(inner.clone(), bootstrap.clone()))
        };
        let connectors = DnsConnectors {
            tcp: behind_bootstrap(&deps.connectors.tcp),
            tls: behind_bootstrap(&deps.connectors.tls),
            https: behind_bootstrap(&deps.connectors.https),
            quic: behind_bootstrap(&deps.connectors.quic),
            h3: behind_bootstrap(&deps.connectors.h3),
        };
        let http = match HttpClient::new(
            connectors.https.clone(),
            HttpClientConfig {
                skip_cert_verification: cfg.skip_cert_verification,
                ..HttpClientConfig::default()
            },
        ) {
            Ok(c) => Some(Arc::new(c)),
            Err(e) => {
                diags.push(Diagnostic::warning(
                    codes::W_DNS_UPSTREAM_UNSUPPORTED,
                    format!("DoH client unavailable: {e}"),
                ));
                None
            }
        };
        let tls = match tls_client_config(cfg.skip_cert_verification) {
            Ok(c) => Some(c),
            Err(e) => {
                diags.push(Diagnostic::warning(
                    codes::W_DNS_UPSTREAM_UNSUPPORTED,
                    format!("DoT unavailable: {e}"),
                ));
                None
            }
        };
        let doq = available(
            DoqUpstream::client_config(cfg.skip_cert_verification),
            "DoQ",
            &mut diags,
        );
        let doh3 = available(
            Doh3Upstream::client_config(cfg.skip_cert_verification),
            "DoH3",
            &mut diags,
        );

        let hosts = HostMap::build(&cfg.hosts, deps.sets.as_ref(), &mut diags);

        let resolver = Arc::new(Resolver {
            connectors,
            system: deps.system.clone(),
            bootstrap,
            http,
            tls,
            doq,
            doh3,
            primary: ArcSwap::from_pointee(Vec::new()),
            system_upstreams: ArcSwap::from_pointee(Vec::new()),
            host_upstreams: Mutex::new(HashMap::new()),
            cache: DnsCache::new(cfg.cache_capacity),
            hosts,
            inflight: Mutex::new(HashMap::new()),
            aaaa_failures: AtomicU32::new(0),
            aaaa_suppressed: AtomicBool::new(false),
            has_ipv6: AtomicBool::new(has_ipv6),
            self_weak: Mutex::new(Weak::new()),
            vias: Mutex::new(HashMap::new()),
            via_unfollowed: AtomicBool::new(false),
            via_serverless: AtomicBool::new(false),
            configured_udp,
            wants_system,
            encrypted_specs: encrypted,
            cfg,
        });
        *resolver.self_weak.lock().expect("weak") = Arc::downgrade(&resolver);
        resolver.apply_system_servers(&system_specs);
        if resolver.cfg.read_etc_hosts
            && let Some(path) = deps.system.hosts_path()
        {
            resolver.watch_etc_hosts(&deps.resources, path);
        }
        (resolver, diags)
    }

    fn build_upstream(&self, spec: &UpstreamSpec) -> Option<UpstreamRef> {
        match spec {
            UpstreamSpec::Udp(addr) => Some(Arc::new(UdpUpstream::new(*addr))),
            UpstreamSpec::Tcp { host, port } => Some(Arc::new(TcpUpstream::plain(
                host,
                *port,
                self.connectors.tcp.clone(),
            ))),
            UpstreamSpec::Tls { host, port } => self.tls.as_ref().map(|tls| {
                Arc::new(TcpUpstream::tls(
                    host,
                    *port,
                    self.connectors.tls.clone(),
                    tls.clone(),
                )) as UpstreamRef
            }),
            UpstreamSpec::Https { url, pathless } => self.http.as_ref().map(|http| {
                Arc::new(DohUpstream::new(url.clone(), *pathless, http.clone())) as UpstreamRef
            }),
            UpstreamSpec::Quic { host, port } => self.doq.as_ref().map(|config| {
                Arc::new(DoqUpstream::new(
                    host,
                    *port,
                    self.connectors.quic.clone(),
                    self.bootstrap.clone(),
                    config.clone(),
                )) as UpstreamRef
            }),
            UpstreamSpec::H3(url) => self.doh3.as_ref().map(|config| {
                Arc::new(Doh3Upstream::new(
                    url.clone(),
                    self.connectors.h3.clone(),
                    self.bootstrap.clone(),
                    config.clone(),
                )) as UpstreamRef
            }),
        }
    }

    /// Rebuilds every upstream set that depends on the platform's server list,
    /// from the list as it reads right now. The single derivation path, shared
    /// by `new` and `on_network_change`, so the two can never disagree.
    fn apply_system_servers(&self, system_specs: &[UpstreamSpec]) {
        let traditional = traditional_specs(&self.configured_udp, self.wants_system, system_specs);
        // Encrypted upstreams own the primary set and do not follow the network.
        let primary_specs = if self.encrypted_specs.is_empty() {
            &traditional
        } else {
            &self.encrypted_specs
        };
        self.primary.store(Arc::new(
            primary_specs
                .iter()
                .filter_map(|s| self.build_upstream(s))
                .collect(),
        ));
        self.system_upstreams
            .store(Arc::new(system_specs.iter().map(build_plain_udp).collect()));
        self.bootstrap
            .set_upstreams(bootstrap_set(&traditional, system_specs));
    }

    pub fn primary_upstreams(&self) -> Vec<String> {
        self.primary
            .load()
            .iter()
            .map(|u| u.name().to_string())
            .collect()
    }

    /// The upstreams that resolve the hostnames of URL-type (DoT / DoH /
    /// DoH3 / DoQ / TCP) upstreams. Always plain UDP; see `bootstrap_set`.
    pub fn bootstrap_upstreams(&self) -> Vec<String> {
        self.bootstrap.upstream_names()
    }

    pub fn aaaa_suppressed(&self) -> bool {
        self.aaaa_suppressed.load(Ordering::Relaxed)
    }

    pub fn cache_snapshot(&self) -> Vec<CacheEntry> {
        self.cache.snapshot()
    }

    pub fn flush(&self) {
        self.cache.flush();
        // rebuilt from the servers of the moment by the next lookup
        self.vias.lock().expect("vias").clear();
        self.bootstrap.flush();
        self.aaaa_failures.store(0, Ordering::Relaxed);
        self.aaaa_suppressed.store(false, Ordering::Relaxed);
    }

    /// Flush, re-read the system servers, rebuild sockets.
    pub fn on_network_change(&self) {
        self.flush();
        self.has_ipv6
            .store(self.system.has_ipv6(), Ordering::Relaxed);
        let system_specs: Vec<UpstreamSpec> = self
            .system
            .servers()
            .into_iter()
            .map(UpstreamSpec::Udp)
            .collect();
        self.apply_system_servers(&system_specs);
        self.host_upstreams.lock().expect("host upstreams").clear();
    }

    fn want_v6(&self, opts: &LookupOpts) -> bool {
        let configured = opts
            .want_v6
            .unwrap_or(self.cfg.ipv6 && self.has_ipv6.load(Ordering::Relaxed));
        configured && !self.aaaa_suppressed()
    }

    fn host_upstreams_for(&self, specs: &[UpstreamSpec]) -> Arc<Vec<UpstreamRef>> {
        let mut map = self.host_upstreams.lock().expect("host upstreams");
        if let Some(list) = map.get(specs) {
            return list.clone();
        }
        let list = Arc::new(
            specs
                .iter()
                .filter_map(|s| self.build_upstream(s))
                .collect::<Vec<_>>(),
        );
        map.insert(specs.to_vec(), list.clone());
        list
    }

    pub async fn lookup(&self, host: &str, opts: LookupOpts) -> Result<DnsResult, DnsError> {
        self.lookup_inner(host, opts, None).await
    }

    /// `lookup`, the questions to the plain servers leaving through `via`
    /// with answers kept apart (`dns-follow-interface`, phase 2 M5 design
    /// 8.5). `[Host]`, the hosts file and the system's own lookups go as
    /// usual; with encrypted DNS configured, or no plain server to ask,
    /// nothing follows `via`.
    pub async fn lookup_via(
        &self,
        host: &str,
        opts: LookupOpts,
        via: &Via,
    ) -> Result<DnsResult, DnsError> {
        self.lookup_inner(host, opts, Some(via)).await
    }

    /// The upstream set of `via`, built from the plain servers of the moment.
    fn via_set(&self, via: &Via) -> Arc<ViaSet> {
        let mut vias = self.vias.lock().expect("vias");
        if let Some(set) = vias.get(&via.key) {
            return set.clone();
        }
        let system: Vec<UpstreamSpec> = self
            .system
            .servers()
            .into_iter()
            .map(UpstreamSpec::Udp)
            .collect();
        let upstreams = traditional_specs(&self.configured_udp, self.wants_system, &system)
            .iter()
            .filter_map(|spec| match spec {
                UpstreamSpec::Udp(addr) => Some(Arc::new(UdpUpstream::via(
                    *addr,
                    &via.key,
                    via.connector.clone(),
                )) as UpstreamRef),
                _ => None,
            })
            .collect();
        let set = Arc::new(ViaSet {
            upstreams: Arc::new(upstreams),
            cache: DnsCache::new(self.cfg.cache_capacity),
        });
        vias.insert(via.key.clone(), set.clone());
        set
    }

    /// The upstream step of a lookup through a `Via`: its own cache, then its
    /// own servers.
    async fn lookup_on(
        &self,
        name: &str,
        want_v6: bool,
        opts: &LookupOpts,
        set: &ViaSet,
        started: Instant,
    ) -> Result<DnsResult, DnsError> {
        if !opts.bypass_cache {
            match set.cache.get(name) {
                Some(CacheHit::Fresh(a)) if a.v6_queried || !want_v6 => {
                    return Ok(from_cached(&a, Source::Cache { stale: false }, started));
                }
                Some(CacheHit::Negative) => return Err(DnsError::EmptyAnswer),
                _ => {}
            }
        }
        let (result, _) = self
            .query_coalesced(&set.upstreams, name, want_v6, opts)
            .await;
        match &result {
            Ok(a) => set.cache.put(name, cached_from(a, want_v6)),
            Err(DnsError::EmptyAnswer) => set.cache.put_negative(name),
            Err(_) => {}
        }
        let answers = result?;
        Ok(from_answers(
            &answers,
            Source::Upstream(answers.upstream.clone()),
            started,
        ))
    }

    async fn lookup_inner(
        &self,
        host: &str,
        opts: LookupOpts,
        via: Option<&Via>,
    ) -> Result<DnsResult, DnsError> {
        let started = Instant::now();
        let trimmed = host.trim();
        let bare = trimmed.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = bare.parse::<IpAddr>() {
            return Ok(literal(ip, Source::Literal, started));
        }
        // an address needed no lookup; every other way out of here is one
        let _noted = Noted(started);
        let lower = trimmed.to_ascii_lowercase();
        let no_search = lower.ends_with('.');
        let name = lower.trim_end_matches('.').to_string();
        if name.is_empty() {
            return Err(DnsError::Unsupported("empty hostname".to_string()));
        }
        let want_v6 = self.want_v6(&opts);

        // [Host] chain with alias restarts.
        let mut current = name;
        let mut hops = 0usize;
        loop {
            if current == "localhost" || current.ends_with(".localhost") {
                let mut r = literal(IpAddr::V4(Ipv4Addr::LOCALHOST), Source::Loopback, started);
                if want_v6 {
                    r.v6.push(Ipv6Addr::LOCALHOST);
                }
                return Ok(r);
            }
            if self.cfg.proxy_hostnames.contains(&current) {
                break;
            }
            let Some(hit) = self.hosts.lookup(&current) else {
                break;
            };
            match hit.action {
                HostAction::Ips(ips) => {
                    let kind = if hit.etc_hosts {
                        HostKind::EtcHosts
                    } else {
                        HostKind::Ip
                    };
                    return Ok(from_ips(&ips, Duration::ZERO, Source::Host(kind), started));
                }
                HostAction::Alias(target) => {
                    hops += 1;
                    if hops > MAX_ALIAS_HOPS {
                        return Err(DnsError::AliasLoop(current));
                    }
                    if let Ok(ip) = target.parse::<IpAddr>() {
                        return Ok(literal(ip, Source::Host(HostKind::Alias), started));
                    }
                    current = target;
                    continue;
                }
                HostAction::Servers(specs) => {
                    let ups = self.host_upstreams_for(&specs);
                    let (answers, _) = self.query_coalesced(&ups, &current, want_v6, &opts).await;
                    let answers = answers?;
                    return Ok(from_answers(
                        &answers,
                        Source::Host(HostKind::Server),
                        started,
                    ));
                }
                HostAction::System(_) => {
                    return self
                        .library_lookup(&current, want_v6, Source::Host(HostKind::System), started)
                        .await;
                }
            }
        }

        // Special names.
        if current.ends_with(".local") {
            return self
                .library_lookup(&current, want_v6, Source::System, started)
                .await;
        }
        if !current.contains('.') && !no_search {
            let candidate = match self.system.search_domains().first() {
                Some(d) => format!("{current}.{}", d.trim_matches('.').to_ascii_lowercase()),
                None => current.clone(),
            };
            return self
                .system_lookup(&candidate, want_v6, Source::System, started, &opts)
                .await;
        }

        if let Some(via) = via {
            if self.encrypted_specs.is_empty() {
                let set = self.via_set(via);
                if !set.upstreams.is_empty() {
                    return self
                        .lookup_on(&current, want_v6, &opts, &set, started)
                        .await;
                }
                if !self.via_serverless.swap(true, Ordering::Relaxed) {
                    tracing::info!(
                        "dns-follow-interface: no plain DNS server to ask through the interface; asked as usual"
                    );
                }
            } else if !self.via_unfollowed.swap(true, Ordering::Relaxed) {
                tracing::info!(
                    "dns-follow-interface: the encrypted DNS servers are asked as usual, not through the policy's interface"
                );
            }
        }

        // Cache. An entry recorded without asking for AAAA cannot answer a
        // caller that wants it, so such a hit falls through to the query below,
        // whose `record` then replaces the entry with a v6-aware one.
        if !opts.bypass_cache {
            match self.cache.get(&current) {
                Some(CacheHit::Fresh(a)) if a.v6_queried || !want_v6 => {
                    return Ok(from_cached(&a, Source::Cache { stale: false }, started));
                }
                Some(CacheHit::Stale(a)) if a.v6_queried || !want_v6 => {
                    self.spawn_refresh(current.clone(), want_v6, AaaaTimeout::Count);
                    return Ok(from_cached(&a, Source::Cache { stale: true }, started));
                }
                Some(CacheHit::Negative) => return Err(DnsError::EmptyAnswer),
                _ => {}
            }
        }

        let primary = self.primary.load_full();
        if primary.is_empty() {
            return self
                .system_lookup(&current, want_v6, Source::System, started, &opts)
                .await;
        }
        let (result, query) = self
            .query_coalesced(&primary, &current, want_v6, &opts)
            .await;
        self.record(
            &current,
            &result,
            want_v6,
            query.timeout(AaaaTimeout::Count),
        );
        // Design §7.3: when the resend tick completed the query with A in hand
        // and AAAA still pending, the late AAAA answer must still reach the
        // cache. The fanout has already aborted that query, so the completion
        // runs as a background refresh instead; `DnsCache::begin_refresh` caps
        // it at one per refresh window, and its `record` replaces the partial
        // entry as soon as AAAA arrives.
        if want_v6
            && let Ok(a) = &result
            && a.partial
            && a.aaaa_timed_out
        {
            self.spawn_refresh(current.clone(), want_v6, AaaaTimeout::Ignore);
        }
        let answers = result?;
        Ok(from_answers(
            &answers,
            Source::Upstream(answers.upstream.clone()),
            started,
        ))
    }

    /// What `[Host]` (or the hosts file) says about `name`, without sending a
    /// query: the engine uses it for `use-local-host-item-for-proxy`.
    pub fn host_lookup(&self, name: &str) -> Option<crate::hosts::HostLookup> {
        self.hosts.lookup(name)
    }

    /// Stores the outcome in the cache and feeds AAAA suppression.
    fn record(
        &self,
        name: &str,
        result: &Result<Answers, DnsError>,
        want_v6: bool,
        timeout: AaaaTimeout,
    ) {
        match result {
            Ok(a) => {
                self.cache.put(name, cached_from(a, want_v6));
                if want_v6 {
                    if a.aaaa_timed_out {
                        if timeout == AaaaTimeout::Count {
                            let n = self.aaaa_failures.fetch_add(1, Ordering::Relaxed) + 1;
                            if n >= AAAA_SUPPRESS_AFTER
                                && !self.aaaa_suppressed.swap(true, Ordering::Relaxed)
                            {
                                // Only the flush is named: `on_network_change`
                                // lifts the suppression too, but nothing calls it yet.
                                tracing::warn!(
                                    "AAAA answers timed out in {n} lookups in a row; AAAA queries suppressed until the DNS cache is flushed"
                                );
                            }
                        }
                    } else {
                        self.aaaa_failures.store(0, Ordering::Relaxed);
                    }
                }
            }
            Err(DnsError::EmptyAnswer) => self.cache.put_negative(name),
            Err(_) => {}
        }
    }

    fn spawn_refresh(&self, name: String, want_v6: bool, timeout: AaaaTimeout) {
        if !self.cache.begin_refresh(&name) {
            return;
        }
        let weak = self.self_weak.lock().expect("weak").clone();
        tokio::spawn(async move {
            let Some(me) = weak.upgrade() else {
                return;
            };
            let primary = me.primary.load_full();
            let (result, query) = me
                .query_coalesced(&primary, &name, want_v6, &LookupOpts::default())
                .await;
            match &result {
                Ok(_) | Err(DnsError::EmptyAnswer) => {
                    me.record(&name, &result, want_v6, query.timeout(timeout))
                }
                Err(_) => me.cache.end_refresh(&name),
            }
        });
    }

    /// One in-flight query per (name, family); later callers wait for the
    /// first. The result comes with whose query it is: a waiter has the first
    /// caller's answer, and the timeout that may be in it is that caller's to
    /// count. A waiter that takes over because the first caller vanished
    /// sends a query of its own and is its owner, as is a caller that goes
    /// round the cache.
    async fn query_coalesced(
        &self,
        upstreams: &[UpstreamRef],
        name: &str,
        want_v6: bool,
        opts: &LookupOpts,
    ) -> (Result<Answers, DnsError>, Query) {
        let key = format!(
            "{name}#{}#{}",
            want_v6,
            upstreams
                .iter()
                .map(|u| u.name())
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut rx = {
            let map = self.inflight.lock().expect("inflight");
            map.get(&key).map(|tx| tx.subscribe())
        };
        if let Some(rx) = rx.as_mut()
            && !opts.bypass_cache
        {
            loop {
                let value = rx.borrow().clone();
                if let Some(v) = value {
                    return (v, Query::Another);
                }
                if rx.changed().await.is_err() {
                    break; // the first caller vanished: query ourselves
                }
            }
        }
        let (tx, _keep) = watch::channel(None);
        self.inflight
            .lock()
            .expect("inflight")
            .insert(key.clone(), tx);
        let mut inflight = Inflight {
            map: &self.inflight,
            key,
            published: false,
        };
        let result: Result<Answers, DnsError> =
            resolve_name(upstreams, name, want_v6, &self.cfg.fanout)
                .await
                .map_err(DnsError::from);
        inflight.publish(&result);
        (result, Query::Own)
    }

    /// The system's DNS servers when any is known, the system resolver
    /// library when none is: a single label given its search domain, and any
    /// name of a resolver that has no server to ask.
    async fn system_lookup(
        &self,
        name: &str,
        want_v6: bool,
        source: Source,
        started: Instant,
        opts: &LookupOpts,
    ) -> Result<DnsResult, DnsError> {
        let system = self.system_upstreams.load_full();
        if !system.is_empty() {
            let (answers, _) = self.query_coalesced(&system, name, want_v6, opts).await;
            let answers = answers?;
            return Ok(from_answers(&answers, source, started));
        }
        self.library_lookup(name, want_v6, source, started).await
    }

    /// `name` asked of the system resolver library — what `getaddrinfo`
    /// answers from — for the names only the system knows: the names ending
    /// in `.local` and the `[Host]` values `server:system`, `server:syslib`
    /// and `server:force-syslib` (all three are the library in the normal
    /// mode), and any name when no server is left to ask.
    ///
    /// The library gives no TTL and its answers are not cached here: the
    /// system keeps its own cache. Identical lookups in flight are not
    /// merged for the same reason.
    async fn library_lookup(
        &self,
        name: &str,
        want_v6: bool,
        source: Source,
        started: Instant,
    ) -> Result<DnsResult, DnsError> {
        // What a lookup by servers may take. When the limit passes the system
        // call goes on in a thread of its own and its result is dropped.
        let limit = self.cfg.fanout.resend * self.cfg.fanout.attempts.max(1);
        let asked = Instant::now();
        let outcome = tokio::time::timeout(limit, self.system.library_lookup(name)).await;
        let elapsed_ms = asked.elapsed().as_millis() as u64;
        let ips = match outcome {
            Ok(Ok(ips)) => ips,
            Ok(Err(e)) => {
                tracing::debug!(target: "rurge_dns::resolver", upstream = "system resolver library", error = %e, elapsed_ms, "failed");
                return Err(DnsError::AllFailed(vec![(
                    "system".to_string(),
                    e.to_string(),
                )]));
            }
            Err(_) => {
                tracing::debug!(target: "rurge_dns::resolver", upstream = "system resolver library", error = %DnsError::Timeout, elapsed_ms, "failed");
                return Err(DnsError::Timeout);
            }
        };
        tracing::debug!(target: "rurge_dns::resolver", upstream = "system resolver library", records = ips.len(), elapsed_ms, "answer");
        let ips: Vec<IpAddr> = ips
            .into_iter()
            .filter(|ip| want_v6 || ip.is_ipv4())
            .collect();
        if ips.is_empty() {
            return Err(DnsError::EmptyAnswer);
        }
        Ok(from_ips(&ips, Duration::from_secs(60), source, started))
    }

    pub async fn measure_delay(&self, name: &str) -> Vec<UpstreamDelay> {
        let mut out = Vec::new();
        for up in self.primary.load_full().iter() {
            let started = Instant::now();
            let one = [up.clone()];
            let result = resolve_name(&one, name, false, &self.cfg.fanout)
                .await
                .map(|_| started.elapsed())
                .map_err(|e| e.to_string());
            out.push(UpstreamDelay {
                upstream: up.name().to_string(),
                result,
            });
        }
        out
    }

    fn watch_etc_hosts(
        self: &Arc<Self>,
        resources: &Arc<ResourceManager>,
        path: std::path::PathBuf,
    ) {
        let handle = resources.get(&ResourceSpec {
            source: ResourceSource::File(path.clone()),
            update_interval: None,
            kind: ResourceKind::System,
        });
        // Subscribe before the first read: `watch::Sender::subscribe` marks the
        // current version as seen, so subscribing afterwards would swallow a
        // change that landed between the read and the subscription.
        let mut rx = handle.subscribe();
        if let Some((data, _)) = handle.current().data() {
            self.hosts
                .set_etc_hosts(parse_hosts_file(&String::from_utf8_lossy(&data)));
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let changed = tokio::select! {
                    r = rx.changed() => r.is_ok(),
                    _ = tokio::time::sleep(Duration::from_secs(60)) => {
                        if weak.upgrade().is_none() {
                            return;
                        }
                        continue;
                    }
                };
                if !changed {
                    return;
                }
                let Some(me) = weak.upgrade() else {
                    return;
                };
                if let ResourceState::Available { data, .. } = handle.current() {
                    me.hosts
                        .set_etc_hosts(parse_hosts_file(&String::from_utf8_lossy(&data)));
                    tracing::info!(path = %path.display(), entries = me.hosts.etc_hosts_len(), "hosts file reloaded");
                }
            }
        });
    }
}

/// A client configuration of the encrypted subsystem, or `None` with a
/// W0026 that says `what` is unavailable.
fn available<T>(built: io::Result<T>, what: &str, diags: &mut Diagnostics) -> Option<T> {
    match built {
        Ok(config) => Some(config),
        Err(e) => {
            diags.push(Diagnostic::warning(
                codes::W_DNS_UPSTREAM_UNSUPPORTED,
                format!("{what} unavailable: {e}"),
            ));
            None
        }
    }
}

/// The traditional (plain UDP) upstreams for the system list as it reads now:
/// the profile's own `dns-server` entries, plus the platform's when the profile
/// asked for them (`dns-server = system`, or no traditional server at all).
fn traditional_specs(
    configured_udp: &[UpstreamSpec],
    wants_system: bool,
    system: &[UpstreamSpec],
) -> Vec<UpstreamSpec> {
    // a server named twice, in the profile or again by `system`, is one
    let mut specs: Vec<UpstreamSpec> = Vec::new();
    let system = if wants_system { system } else { &[] };
    for spec in configured_udp.iter().chain(system) {
        if !specs.contains(spec) {
            specs.push(spec.clone());
        }
    }
    specs
}

/// Bootstrap runs on the traditional (plain UDP) upstreams: the ones the
/// profile configured, or the system's when it configured none. Applied both at
/// construction and on every network change, so the two never disagree.
fn bootstrap_set(traditional: &[UpstreamSpec], system: &[UpstreamSpec]) -> Vec<UpstreamRef> {
    let specs = if traditional.is_empty() {
        system
    } else {
        traditional
    };
    specs.iter().map(build_plain_udp).collect()
}

fn build_plain_udp(spec: &UpstreamSpec) -> UpstreamRef {
    match spec {
        UpstreamSpec::Udp(addr) => Arc::new(UdpUpstream::new(*addr)),
        other => unreachable!("build_plain_udp called with {other:?}"),
    }
}

/// Notes a lookup on the trail of the session that is dialling when it ends,
/// however it ends: a dial that gives up on a lookup drops it half way, and
/// the session has spent that time on it all the same.
struct Noted(Instant);

impl Drop for Noted {
    fn drop(&mut self) {
        rurge_net::trail::looked_up(self.0.elapsed());
    }
}

fn literal(ip: IpAddr, source: Source, started: Instant) -> DnsResult {
    from_ips(&[ip], Duration::ZERO, source, started)
}

fn from_ips(ips: &[IpAddr], ttl: Duration, source: Source, started: Instant) -> DnsResult {
    let mut r = DnsResult {
        v4: Vec::new(),
        v6: Vec::new(),
        ttl,
        source,
        elapsed: started.elapsed(),
    };
    for ip in ips {
        match ip {
            IpAddr::V4(a) => r.v4.push(*a),
            IpAddr::V6(a) => r.v6.push(*a),
        }
    }
    r
}

fn min_ttl(a: &Answers) -> Duration {
    let ttl =
        a.v4.iter()
            .map(|(_, t)| *t)
            .chain(a.v6.iter().map(|(_, t)| *t))
            .min()
            .unwrap_or(0);
    Duration::from_secs(u64::from(ttl))
}

fn cached_from(a: &Answers, want_v6: bool) -> CachedAddrs {
    CachedAddrs {
        v4: a.v4.iter().map(|(ip, _)| *ip).collect(),
        v6: a.v6.iter().map(|(ip, _)| *ip).collect(),
        ttl: min_ttl(a),
        v6_queried: want_v6,
        source: a.upstream.clone(),
    }
}

fn from_answers(a: &Answers, source: Source, started: Instant) -> DnsResult {
    DnsResult {
        v4: a.v4.iter().map(|(ip, _)| *ip).collect(),
        v6: a.v6.iter().map(|(ip, _)| *ip).collect(),
        ttl: min_ttl(a),
        source,
        elapsed: started.elapsed(),
    }
}

fn from_cached(a: &CachedAddrs, source: Source, started: Instant) -> DnsResult {
    DnsResult {
        v4: a.v4.clone(),
        v6: a.v6.clone(),
        ttl: a.ttl,
        source,
        elapsed: started.elapsed(),
    }
}

impl LazyResolver for Resolver {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<ResolvedAddrs, ResolveError>> {
        Box::pin(async move {
            match self.lookup(host, LookupOpts::default()).await {
                Ok(r) => Ok(ResolvedAddrs { v4: r.v4, v6: r.v6 }),
                Err(DnsError::Timeout) => Err(ResolveError::Timeout),
                Err(DnsError::EmptyAnswer) => Err(ResolveError::EmptyAnswer),
                Err(e) => Err(ResolveError::Failed(e.to_string())),
            }
        })
    }
}

/// The addresses of a lookup of `host`, or the error a connector reports.
fn addresses(host: &str, found: Result<DnsResult, DnsError>) -> io::Result<Vec<IpAddr>> {
    let addrs = found.map_err(io::Error::other)?.addrs();
    if addrs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no addresses for {host}"),
        ));
    }
    Ok(addrs)
}

impl Resolve for Resolver {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move { addresses(host, self.lookup(host, LookupOpts::default()).await) })
    }

    fn resolve_via<'a>(
        &'a self,
        host: &'a str,
        via: &'a Via,
    ) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            addresses(
                host,
                self.lookup_via(host, LookupOpts::default(), via).await,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Qtype;
    use crate::system::StaticSystemDns;
    use crate::testing::MockDns;
    use rurge_config::HostName;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::connector::{
        BoxedDatagram, BoxedStream, ConnectOpts, DirectConnector, SystemResolve, Target,
    };
    use rurge_net::resource::ResourceOptions;
    use rurge_net::testing::{Events, Lines};
    use rurge_net::trail::Trail;
    use std::collections::BTreeMap;
    use std::net::SocketAddr;
    use std::path::PathBuf;

    fn fast() -> FanoutOpts {
        FanoutOpts {
            resend: Duration::from_millis(100),
            attempts: 3,
        }
    }

    struct Env {
        _dir: tempfile::TempDir,
        resources: Arc<ResourceManager>,
        sets: Arc<SetRegistry>,
        cfg: Config,
    }

    fn env(profile: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let loaded = from_text(
            profile,
            &dir.path().join("t.conf"),
            &LoadOptions::for_tests(),
        );
        let codes: Vec<&str> = loaded.diagnostics.iter().map(|d| d.code).collect();
        assert!(!loaded.diagnostics.has_errors(), "{codes:?}");
        let cfg = loaded.config;
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let client = Arc::new(HttpClient::new(connector, HttpClientConfig::default()).unwrap());
        let resources = ResourceManager::with_options(
            dir.path().to_path_buf(),
            client,
            ResourceOptions {
                offline: true,
                debounce: Duration::from_millis(50),
                ..ResourceOptions::default()
            },
        );
        let (sets, _) = SetRegistry::build(&cfg, resources.clone(), dir.path());
        Env {
            _dir: dir,
            resources,
            sets,
            cfg,
        }
    }

    fn resolver(e: &Env, system: StaticSystemDns) -> (Arc<Resolver>, Diagnostics) {
        resolver_with(e, Arc::new(system))
    }

    fn resolver_with(e: &Env, system: Arc<dyn SystemDns>) -> (Arc<Resolver>, Diagnostics) {
        resolver_with_fanout(e, system, fast())
    }

    /// The fanout options of a test that counts the questions it was asked
    /// and in which every question is answered: each upstream is asked once
    /// and never again, however late the answer comes — `fast()` asks again
    /// whenever an answer is 100 ms late, which on a busy machine it can be.
    /// A lookup that gets no answer takes 30 seconds to end, so a test that
    /// expects one is not for it. (`bootstrap::tests::once` is the same.)
    fn once() -> FanoutOpts {
        FanoutOpts {
            resend: Duration::from_secs(30),
            attempts: 1,
        }
    }

    /// `resolver`, its fanout `once()`.
    fn resolver_once(e: &Env, system: StaticSystemDns) -> (Arc<Resolver>, Diagnostics) {
        resolver_with_fanout(e, Arc::new(system), once())
    }

    fn resolver_with_fanout(
        e: &Env,
        system: Arc<dyn SystemDns>,
        fanout: FanoutOpts,
    ) -> (Arc<Resolver>, Diagnostics) {
        let mut cfg = ResolverConfig::from_config(&e.cfg);
        cfg.fanout = fanout;
        Resolver::new(
            cfg,
            ResolverDeps {
                connectors: DnsConnectors::all(Arc::new(DirectConnector::new(Arc::new(
                    SystemResolve,
                )))),
                sets: e.sets.clone(),
                system,
                resources: e.resources.clone(),
            },
        )
    }

    /// The resolver of `e`, its questions put to `mock` in memory
    /// (`MockDns::upstream`) and not over UDP: for a test on a paused clock.
    /// There the only time that passes is the time the resolver waits — for
    /// a resend, for an entry to go stale —, and none while an answer is on
    /// its way: how many questions it asks, and what each lookup comes back
    /// with, is the same however busy the machine is. A sleep in such a test
    /// ends when nothing else is left to run.
    ///
    /// Only the primary set is replaced. The system's servers, the servers of
    /// the `[Host]` section, the per-interface set and the bootstrap stay on
    /// UDP, and `on_network_change` builds a UDP primary set again. A lookup
    /// that reaches one of those on the paused clock is not kept to the
    /// resolver's own timing: each time the runtime waits for the datagram the
    /// clock jumps to the next timer — the resend, in the end the deadline —,
    /// so the lookup may ask again, or time out, whatever the answer would
    /// have been.
    fn resolver_in_memory(e: &Env, mock: &MockDns, system: StaticSystemDns) -> Arc<Resolver> {
        let (r, _) = resolver(e, system);
        r.primary.store(Arc::new(vec![mock.upstream()]));
        r
    }

    /// A `SystemDns` whose server list changes, standing in for a network change.
    struct SwapSystemDns {
        servers: Mutex<Vec<SocketAddr>>,
    }

    impl SwapSystemDns {
        fn new(addr: SocketAddr) -> SwapSystemDns {
            SwapSystemDns {
                servers: Mutex::new(vec![addr]),
            }
        }

        fn set(&self, addr: SocketAddr) {
            *self.servers.lock().expect("servers") = vec![addr];
        }
    }

    impl SystemDns for SwapSystemDns {
        fn servers(&self) -> Vec<SocketAddr> {
            self.servers.lock().expect("servers").clone()
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

    fn profile(general: &str, hosts: &str) -> String {
        format!(
            "[General]\n{general}\n[Proxy]\nProxyA = http, proxy.example, 8080\n[Host]\n{hosts}\n[Rule]\nFINAL,DIRECT\n"
        )
    }

    fn v4(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// The events that lookups by the system resolver library leave: those of
    /// this module that name the library as their upstream. What else the
    /// resolver logs on the way to a lookup is not counted.
    fn library_events(events: &Events) -> Vec<BTreeMap<String, String>> {
        events
            .of("rurge_dns::resolver")
            .into_iter()
            .filter(|fields| {
                fields.get("upstream").map(String::as_str) == Some("system resolver library")
            })
            .collect()
    }

    /// A system whose resolver library never answers.
    struct StuckLibrary;

    impl SystemDns for StuckLibrary {
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
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn literals_loopback_and_trailing_dot() {
        let mock = MockDns::spawn().await;
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let (r, diags) = resolver_once(&e, StaticSystemDns::default());
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| d.code).collect::<Vec<_>>()
        );
        let lit = r.lookup("10.1.2.3", LookupOpts::default()).await.unwrap();
        assert_eq!(
            (lit.v4, lit.source),
            (vec![v4("10.1.2.3")], Source::Literal)
        );
        let v6 = r.lookup("[::1]", LookupOpts::default()).await.unwrap();
        assert_eq!(v6.v6, vec![Ipv6Addr::LOCALHOST]);
        let lo = r.lookup("LocalHost", LookupOpts::default()).await.unwrap();
        assert_eq!(
            (lo.v4, lo.source),
            (vec![Ipv4Addr::LOCALHOST], Source::Loopback)
        );
        assert_eq!(
            r.lookup("app.localhost", LookupOpts::default())
                .await
                .unwrap()
                .source,
            Source::Loopback
        );
        mock.set("dotted.test", &["10.0.0.7"], &[], 60);
        let d = r
            .lookup("Dotted.Test.", LookupOpts::default())
            .await
            .unwrap();
        assert_eq!(d.v4, vec![v4("10.0.0.7")]);
        assert_eq!(mock.query_count("dotted.test", Qtype::A), 1);
        assert!(r.lookup("", LookupOpts::default()).await.is_err());
    }

    /// A lookup made while a session dials is noted on the session's trail,
    /// whoever answers it — `[Host]`, the cache or an upstream — and whether
    /// it succeeds or not. An address is no lookup.
    #[tokio::test]
    async fn lookups_are_noted_on_the_trail_of_whoever_is_dialling() {
        let mock = MockDns::spawn().await;
        mock.set("up.test", &["10.0.0.1"], &[], 60);
        mock.set_empty("nx.test");
        let e = env(&profile(
            &format!("dns-server = {}", mock.addr()),
            "pinned.test = 10.1.2.3\n",
        ));
        let (r, _) = resolver_once(&e, StaticSystemDns::default());
        let spent = |name: &'static str| {
            let r = r.clone();
            async move {
                let trail = Trail::new();
                let _ = trail.follow(r.lookup(name, LookupOpts::default())).await;
                trail.dns()
            }
        };
        assert!(spent("pinned.test").await.is_some(), "[Host]");
        assert!(spent("up.test").await.is_some(), "an upstream");
        assert!(spent("up.test").await.is_some(), "the cache");
        assert_eq!(
            mock.query_count("up.test", Qtype::A),
            1,
            "the second was the cache's"
        );
        assert!(spent("nx.test").await.is_some(), "an empty answer");
        assert!(spent("localhost").await.is_some(), "the loopback name");
        assert_eq!(spent("10.9.9.9").await, None, "an address");
        assert_eq!(spent("[::1]").await, None, "an address in brackets");
    }

    /// A lookup through an interface counts as any other.
    #[tokio::test]
    async fn a_lookup_via_an_interface_is_noted_too() {
        let mock = MockDns::spawn().await;
        mock.set("a.test", &["10.0.0.1"], &[], 60);
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let (r, _) = resolver_once(&e, StaticSystemDns::default());
        let (via, _) = Noting::via("en1");
        let trail = Trail::new();
        trail
            .follow(r.lookup_via("a.test", LookupOpts::default(), &via))
            .await
            .unwrap();
        assert!(trail.dns().is_some());
    }

    /// The connections a lookup makes itself — to a `tcp://` server here —
    /// are made by the tasks the query fans out into, not by the dial that
    /// waits for the answer: the session's trail has the time the lookup took
    /// and no address.
    #[tokio::test]
    async fn the_connections_a_lookup_makes_are_not_the_dialling_sessions() {
        let tcp = MockDns::spawn().await;
        tcp.set("a.test", &["10.0.0.42"], &[], 60);
        let e = env(&profile(
            &format!("encrypted-dns-server = tcp://{}", tcp.addr()),
            "",
        ));
        let (r, diags) = resolver_once(&e, StaticSystemDns::default());
        assert!(diags.is_empty());
        assert_eq!(r.primary_upstreams(), vec![format!("tcp://{}", tcp.addr())]);
        let trail = Trail::new();
        let a = trail
            .follow(r.lookup("a.test", LookupOpts::default()))
            .await
            .unwrap();
        assert_eq!(a.v4, vec![v4("10.0.0.42")]);
        assert_eq!(tcp.query_count("a.test", Qtype::A), 1, "asked over TCP");
        assert!(trail.dns().is_some());
        assert_eq!(trail.peer(), None);
    }

    /// A lookup the dial gives up on — the timeout around it fires — took the
    /// time it took, and the trail has it.
    #[tokio::test(start_paused = true)]
    async fn a_lookup_cut_short_is_noted_for_the_time_it_took() {
        let mock = MockDns::spawn().await;
        mock.set("slow.test", &["10.0.0.1"], &[], 60);
        mock.set_delay(Duration::from_secs(30));
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let r = resolver_in_memory(&e, &mock, StaticSystemDns::default());
        let trail = Trail::new();
        let cut = trail
            .follow(tokio::time::timeout(
                Duration::from_millis(50),
                r.lookup("slow.test", LookupOpts::default()),
            ))
            .await;
        assert!(cut.is_err(), "the timeout fired first");
        assert!(trail.dns().unwrap() >= Duration::from_millis(50));
    }

    #[tokio::test]
    async fn host_chain_ips_aliases_loops_and_proxy_exemption() {
        let mock = MockDns::spawn().await;
        mock.set("proxy.example", &["10.9.9.9"], &[], 60);
        let e = env(&profile(
            &format!("dns-server = {}", mock.addr()),
            "fixed.test = 1.2.3.4, ::2\nalias.test = fixed.test\nloop-a.test = loop-b.test\nloop-b.test = loop-a.test\nproxy.example = 127.0.0.1\nip-alias.test = 9.9.9.9\n",
        ));
        let (r, _) = resolver_once(&e, StaticSystemDns::default());
        let f = r.lookup("fixed.test", LookupOpts::default()).await.unwrap();
        assert_eq!(
            (f.v4.clone(), f.v6.clone(), f.source.clone()),
            (
                vec![v4("1.2.3.4")],
                vec!["::2".parse().unwrap()],
                Source::Host(HostKind::Ip)
            )
        );
        assert_eq!(f.ttl, Duration::ZERO);
        let a = r.lookup("alias.test", LookupOpts::default()).await.unwrap();
        assert_eq!(a.v4, vec![v4("1.2.3.4")]);
        assert!(matches!(
            r.lookup("loop-a.test", LookupOpts::default()).await,
            Err(DnsError::AliasLoop(_))
        ));
        // proxy hostnames bypass [Host] and go upstream
        let p = r
            .lookup("proxy.example", LookupOpts::default())
            .await
            .unwrap();
        assert_eq!(p.v4, vec![v4("10.9.9.9")]);
        assert!(matches!(p.source, Source::Upstream(_)));
        assert_eq!(
            r.lookup("ip-alias.test", LookupOpts::default())
                .await
                .unwrap()
                .v4,
            vec![v4("9.9.9.9")]
        );
    }

    /// Where each kind of name goes: `server:<address>` to that server alone,
    /// `server:system` and `.local` names to the system resolver library, a
    /// single label with the first search domain to the system's servers.
    /// The servers know `sys.test` and `printer.local` as well, and say
    /// something else; they are not asked.
    #[tokio::test]
    async fn host_server_and_system_modes_and_special_names() {
        let primary = MockDns::spawn().await;
        let dedicated = MockDns::spawn().await;
        let system = MockDns::spawn().await;
        dedicated.set("corp.test", &["10.10.0.1"], &[], 60);
        for server in [&primary, &system] {
            server.set("printer.local", &["10.20.0.1"], &[], 60);
            server.set("sys.test", &["10.20.0.3"], &[], 60);
        }
        system.set("nas.home.lan", &["10.20.0.2"], &[], 60);
        let e = env(&profile(
            &format!("dns-server = {}", primary.addr()),
            &format!(
                "corp.test = server:{}\nsys.test = server:system\n",
                dedicated.addr()
            ),
        ));
        let sys = StaticSystemDns {
            servers: vec![system.addr()],
            search_domains: vec!["home.lan".into()],
            library: vec![
                ("printer.local".into(), vec![ip("10.21.0.1")]),
                ("sys.test".into(), vec![ip("10.21.0.3")]),
            ],
            ..StaticSystemDns::default()
        };
        let asked = sys.library_asked.clone();
        let (r, _) = resolver_once(&e, sys);
        let c = r.lookup("corp.test", LookupOpts::default()).await.unwrap();
        assert_eq!(
            (c.v4, c.source),
            (vec![v4("10.10.0.1")], Source::Host(HostKind::Server))
        );
        assert_eq!(primary.query_count("corp.test", Qtype::A), 0);
        let s = r.lookup("sys.test", LookupOpts::default()).await.unwrap();
        assert_eq!(
            (s.v4, s.source),
            (vec![v4("10.21.0.3")], Source::Host(HostKind::System))
        );
        let l = r
            .lookup("printer.local", LookupOpts::default())
            .await
            .unwrap();
        assert_eq!((l.v4, l.source), (vec![v4("10.21.0.1")], Source::System));
        let n = r.lookup("nas", LookupOpts::default()).await.unwrap();
        assert_eq!(
            n.v4,
            vec![v4("10.20.0.2")],
            "single label + first search domain"
        );
        assert_eq!(system.query_count("nas.home.lan", Qtype::A), 1);
        assert_eq!(primary.query_count("nas.home.lan", Qtype::A), 0);
        for name in ["sys.test", "printer.local"] {
            for server in [&primary, &system] {
                assert_eq!(server.query_count(name, Qtype::A), 0, "{name}");
            }
        }
        assert_eq!(*asked.lock().unwrap(), ["sys.test", "printer.local"]);
    }

    /// `.local` names are the system resolver library's (manual, DNS
    /// Overview, "Special Hostnames"): mDNS and Bonjour answer them, and a DNS
    /// server does not. They go there although the system's servers are
    /// known, and the library's answer is not cached.
    #[tokio::test]
    async fn a_local_name_is_asked_of_the_library_and_not_of_the_servers() {
        let primary = MockDns::spawn().await;
        let system = MockDns::spawn().await;
        primary.set("printer.local", &["10.20.0.1"], &[], 60);
        system.set("printer.local", &["10.20.0.2"], &[], 60);
        let e = env(&profile(&format!("dns-server = {}", primary.addr()), ""));
        let sys = StaticSystemDns {
            servers: vec![system.addr()],
            library: vec![("printer.local".into(), vec![ip("10.20.0.3")])],
            ..StaticSystemDns::default()
        };
        let asked = sys.library_asked.clone();
        let (r, _) = resolver_once(&e, sys);
        let found = r
            .lookup("Printer.LOCAL", LookupOpts::default())
            .await
            .unwrap();
        assert_eq!(
            (found.v4, found.source, found.ttl),
            (
                vec![v4("10.20.0.3")],
                Source::System,
                Duration::from_secs(60)
            )
        );
        for server in [&primary, &system] {
            assert_eq!(server.query_count("printer.local", Qtype::A), 0);
            assert_eq!(server.query_count("printer.local", Qtype::Aaaa), 0);
        }
        assert_eq!(*asked.lock().unwrap(), ["printer.local"]);
        r.lookup("printer.local", LookupOpts::default())
            .await
            .unwrap();
        assert_eq!(
            *asked.lock().unwrap(),
            ["printer.local", "printer.local"],
            "the library is asked again; nothing is cached"
        );
        assert!(r.cache_snapshot().is_empty());
    }

    /// `server:system`, `server:syslib` and `server:force-syslib` are one
    /// thing until the virtual interface: in the normal mode all three are the
    /// system resolver library (manual, Local DNS Mapping, "System
    /// Resolution"), whatever the system's servers know.
    #[tokio::test]
    async fn the_three_system_values_of_host_are_all_the_library() {
        let system = MockDns::spawn().await;
        let known = [
            ("a.test", "10.20.0.11"),
            ("b.test", "10.20.0.12"),
            ("c.test", "10.20.0.13"),
        ];
        for (name, _) in known {
            system.set(name, &["10.20.0.1"], &[], 60);
        }
        let e = env(&profile(
            "",
            "a.test = server:system\nb.test = server:syslib\nc.test = server:force-syslib\n",
        ));
        let sys = StaticSystemDns {
            servers: vec![system.addr()],
            library: known
                .iter()
                .map(|(name, address)| (name.to_string(), vec![ip(address)]))
                .collect(),
            ..StaticSystemDns::default()
        };
        let asked = sys.library_asked.clone();
        let (r, _) = resolver_once(&e, sys);
        for (name, address) in known {
            let found = r.lookup(name, LookupOpts::default()).await.unwrap();
            assert_eq!(
                (found.v4, found.source),
                (vec![v4(address)], Source::Host(HostKind::System)),
                "{name}"
            );
            assert_eq!(system.query_count(name, Qtype::A), 0, "{name}");
        }
        assert_eq!(*asked.lock().unwrap(), ["a.test", "b.test", "c.test"]);
    }

    /// A single label gets the first search domain and goes to the system's
    /// servers (manual, DNS Overview, "Special Hostnames"); the library is
    /// not asked.
    #[tokio::test]
    async fn a_simple_host_name_goes_to_the_system_servers_with_the_search_domain() {
        let system = MockDns::spawn().await;
        system.set("nas.home.lan", &["10.20.0.2"], &[], 60);
        let e = env(&profile("", ""));
        let sys = StaticSystemDns {
            servers: vec![system.addr()],
            search_domains: vec!["home.lan".into(), "other.lan".into()],
            library: vec![("nas.home.lan".into(), vec![ip("10.20.0.9")])],
            ..StaticSystemDns::default()
        };
        let asked = sys.library_asked.clone();
        let (r, _) = resolver_once(&e, sys);
        let found = r.lookup("nas", LookupOpts::default()).await.unwrap();
        assert_eq!(
            (found.v4, found.source),
            (vec![v4("10.20.0.2")], Source::System)
        );
        assert_eq!(system.query_count("nas.home.lan", Qtype::A), 1);
        assert!(asked.lock().unwrap().is_empty());
    }

    /// With no system server known there is nobody to forward the query to,
    /// and the library is asked for the name with the search domain.
    #[tokio::test]
    async fn without_a_system_server_a_simple_host_name_is_asked_of_the_library() {
        let e = env(&profile("", ""));
        let sys = StaticSystemDns {
            search_domains: vec!["home.lan".into()],
            library: vec![("nas.home.lan".into(), vec![ip("10.20.0.2")])],
            ..StaticSystemDns::default()
        };
        let asked = sys.library_asked.clone();
        let (r, _) = resolver_once(&e, sys);
        let found = r.lookup("nas", LookupOpts::default()).await.unwrap();
        assert_eq!(
            (found.v4, found.source),
            (vec![v4("10.20.0.2")], Source::System)
        );
        assert_eq!(*asked.lock().unwrap(), ["nas.home.lan"]);
    }

    /// A resolver that has no `dns-server` and learns no system server asks
    /// the library for an ordinary name, the last place left to ask.
    #[tokio::test]
    async fn with_no_server_at_all_an_ordinary_name_is_asked_of_the_library() {
        let e = env(&profile("", ""));
        let sys = StaticSystemDns {
            library: vec![("www.example.test".into(), vec![ip("10.20.0.5")])],
            ..StaticSystemDns::default()
        };
        let asked = sys.library_asked.clone();
        let (r, _) = resolver_once(&e, sys);
        assert!(r.primary_upstreams().is_empty());
        let found = r
            .lookup("www.example.test", LookupOpts::default())
            .await
            .unwrap();
        assert_eq!(
            (found.v4, found.source),
            (vec![v4("10.20.0.5")], Source::System)
        );
        assert_eq!(*asked.lock().unwrap(), ["www.example.test"]);
    }

    /// With no upstream of its own left, a resolver that knows the system's
    /// DNS servers asks them for an ordinary name, and the library only when
    /// it knows none (the test above). For a user the list is empty when
    /// every encrypted DNS server of the profile needs a client that could not
    /// be built (a W0026 warning), because those servers own the list and the
    /// system's are not in it; without a `dns-server` the build makes the
    /// system's servers the list, so the test empties it afterwards.
    #[tokio::test]
    async fn with_no_primary_upstream_an_ordinary_name_is_asked_of_the_system_servers() {
        let system = MockDns::spawn().await;
        system.set("www.example.test", &["10.20.0.6"], &[], 60);
        let e = env(&profile("", ""));
        let sys = StaticSystemDns {
            servers: vec![system.addr()],
            library: vec![("www.example.test".into(), vec![ip("10.20.0.5")])],
            ..StaticSystemDns::default()
        };
        let asked = sys.library_asked.clone();
        let (r, _) = resolver_once(&e, sys);
        r.primary.store(Arc::new(Vec::new()));
        assert!(r.primary_upstreams().is_empty());
        let found = r
            .lookup("www.example.test", LookupOpts::default())
            .await
            .unwrap();
        assert_eq!(
            (found.v4, found.source),
            (vec![v4("10.20.0.6")], Source::System)
        );
        assert_eq!(system.query_count("www.example.test", Qtype::A), 1);
        assert!(asked.lock().unwrap().is_empty());
    }

    /// The library gives both families whatever was asked for; the resolver
    /// keeps the IPv6 addresses only for a caller that wants them, and a
    /// name that has nothing else is an empty answer then.
    #[tokio::test]
    async fn the_librarys_answer_is_cut_to_the_families_wanted() {
        let system = MockDns::spawn().await;
        let e = env(&profile("", ""));
        let sys = StaticSystemDns {
            servers: vec![system.addr()],
            library: vec![
                (
                    "both.local".into(),
                    vec![ip("10.20.0.1"), ip("fd00::1"), ip("10.20.0.2")],
                ),
                ("v6only.local".into(), vec![ip("fd00::2")]),
            ],
            ..StaticSystemDns::default()
        };
        let (r, _) = resolver_once(&e, sys);
        let without_v6 = LookupOpts {
            want_v6: Some(false),
            ..LookupOpts::default()
        };
        let with_v6 = LookupOpts {
            want_v6: Some(true),
            ..LookupOpts::default()
        };

        let found = r.lookup("both.local", without_v6.clone()).await.unwrap();
        assert_eq!(
            (found.v4, found.v6),
            (vec![v4("10.20.0.1"), v4("10.20.0.2")], vec![])
        );
        let found = r.lookup("both.local", with_v6.clone()).await.unwrap();
        assert_eq!(
            (found.v4, found.v6),
            (
                vec![v4("10.20.0.1"), v4("10.20.0.2")],
                vec!["fd00::1".parse::<Ipv6Addr>().unwrap()]
            )
        );
        assert_eq!(
            r.lookup("v6only.local", without_v6).await,
            Err(DnsError::EmptyAnswer)
        );
        let found = r.lookup("v6only.local", with_v6).await.unwrap();
        assert_eq!(found.v6, vec!["fd00::2".parse::<Ipv6Addr>().unwrap()]);
    }

    /// What the library says when it cannot resolve a name is the error of the
    /// lookup, under the name `system`.
    #[tokio::test]
    async fn a_name_the_library_does_not_know_fails_with_its_error() {
        let system = MockDns::spawn().await;
        let e = env(&profile("", ""));
        let sys = StaticSystemDns {
            servers: vec![system.addr()],
            ..StaticSystemDns::default()
        };
        let (r, _) = resolver_once(&e, sys);
        assert_eq!(
            r.lookup("nobody.local", LookupOpts::default()).await,
            Err(DnsError::AllFailed(vec![(
                "system".to_string(),
                "no such host".to_string()
            )]))
        );
    }

    /// A lookup by the library ends when a lookup by servers would: after
    /// `resend` × `attempts`, with a timeout, and the library's own call goes
    /// on without anyone waiting for it. The limit is an event of its own.
    #[tokio::test(start_paused = true)]
    async fn a_lookup_by_the_library_is_given_as_long_as_one_by_servers() {
        let events = Events::default();
        let _logging = events.capture();
        let e = env(&profile("", ""));
        let (r, _) = resolver_with(&e, Arc::new(StuckLibrary));
        let began = Instant::now();
        let ended = tokio::time::timeout(
            Duration::from_secs(3600),
            r.lookup("printer.local", LookupOpts::default()),
        )
        .await
        .expect("the lookup ends by its own limit");
        assert_eq!(ended, Err(DnsError::Timeout));
        assert_eq!(began.elapsed(), fast().resend * fast().attempts);
        let failed = library_events(&events);
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert_eq!(
            (failed[0]["message"].as_str(), failed[0]["error"].as_str()),
            ("failed", "timeout")
        );
        assert_eq!(failed[0]["elapsed_ms"], "300");
    }

    /// A lookup by the library ends with one DEBUG event, in the form of the
    /// fanout's (`rurge dns lookup --trace` prints it): the upstream, how many
    /// addresses the library gave or what it failed with, and how long it
    /// took. It names neither the host nor an address.
    #[tokio::test]
    async fn a_lookup_by_the_library_leaves_one_event_that_names_no_host_and_no_address() {
        let events = Events::default();
        let _logging = events.capture();
        let e = env(&profile("", ""));
        let sys = StaticSystemDns {
            library: vec![(
                "printer.local".into(),
                vec![ip("192.0.2.77"), ip("fd00::77")],
            )],
            ..StaticSystemDns::default()
        };
        let (r, _) = resolver_once(&e, sys);
        let fields = |event: &BTreeMap<String, String>| {
            event
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        };

        r.lookup("printer.local", LookupOpts::default())
            .await
            .unwrap();
        let answered = library_events(&events);
        assert_eq!(answered.len(), 1, "{answered:?}");
        assert_eq!(fields(&answered[0]), "elapsed_ms,message,records,upstream");
        assert_eq!(answered[0]["message"], "answer");
        assert_eq!(answered[0]["upstream"], "system resolver library");
        assert_eq!(answered[0]["records"], "2", "counted before the families");

        r.lookup("nobody.local", LookupOpts::default())
            .await
            .unwrap_err();
        let both = library_events(&events);
        assert_eq!(both.len(), 2, "{both:?}");
        assert_eq!(fields(&both[1]), "elapsed_ms,error,message,upstream");
        assert_eq!(both[1]["message"], "failed");
        assert_eq!(both[1]["error"], "no such host");

        // Not only the library's own events: nothing this module logs on the
        // way to these lookups names the host or an address.
        let all = events.of("rurge_dns::resolver");
        for text in all.iter().flat_map(|event| event.values()) {
            for secret in ["printer", "nobody", "192.0.2.77", "fd00"] {
                assert!(!text.contains(secret), "{text}");
            }
        }
    }

    /// `nas.` (manual, DNS Overview): the trailing dot is dropped and no
    /// search domain is added, so the servers are asked for `nas` itself
    /// where `nas` alone would have been asked for as `nas.home.lan` of the
    /// system's servers.
    #[tokio::test]
    async fn a_name_with_a_trailing_dot_is_not_given_the_search_domain() {
        let primary = MockDns::spawn().await;
        let system = MockDns::spawn().await;
        primary.set("nas", &["10.30.0.1"], &[], 60);
        system.set("nas.home.lan", &["10.20.0.2"], &[], 60);
        let e = env(&profile(&format!("dns-server = {}", primary.addr()), ""));
        let sys = StaticSystemDns {
            servers: vec![system.addr()],
            search_domains: vec!["home.lan".into()],
            ..StaticSystemDns::default()
        };
        let (r, _) = resolver_once(&e, sys);
        let dotted = r.lookup("nas.", LookupOpts::default()).await.unwrap();
        assert_eq!(dotted.v4, vec![v4("10.30.0.1")]);
        assert_eq!(primary.query_count("nas", Qtype::A), 1);
        assert_eq!(system.query_count("nas.home.lan", Qtype::A), 0);
        // the same name without the dot is the search domain's
        let bare = r.lookup("nas", LookupOpts::default()).await.unwrap();
        assert_eq!(bare.v4, vec![v4("10.20.0.2")]);
        assert_eq!(system.query_count("nas.home.lan", Qtype::A), 1);
    }

    /// The cache keeps an answer for the smallest TTL among its records, the
    /// A ones and the AAAA ones together (manual, DNS Overview).
    #[test]
    fn an_answer_is_cached_for_the_smallest_ttl_of_its_records() {
        let answers = Answers {
            v4: vec![(v4("10.0.0.1"), 300), (v4("10.0.0.2"), 120)],
            v6: vec![
                ("fd00::1".parse().unwrap(), 60),
                ("fd00::2".parse().unwrap(), 900),
            ],
            upstream: "udp://192.0.2.1:53".to_string(),
            ..Answers::default()
        };
        assert_eq!(min_ttl(&answers), Duration::from_secs(60));
        assert_eq!(cached_from(&answers, true).ttl, Duration::from_secs(60));
        let source = Source::Upstream(answers.upstream.clone());
        let result = from_answers(&answers, source, Instant::now());
        assert_eq!(result.ttl, Duration::from_secs(60));
        let v4_only = Answers {
            v6: Vec::new(),
            ..answers
        };
        assert_eq!(min_ttl(&v4_only), Duration::from_secs(120));
    }

    /// A resolver that is told nothing about its cache keeps 2000 names
    /// (manual, DNS Overview: the macOS figure), the least recently used
    /// going first.
    #[tokio::test(start_paused = true)]
    async fn by_default_the_cache_holds_two_thousand_names() {
        let e = env(&profile("", ""));
        let capacity = ResolverConfig::from_config(&e.cfg).cache_capacity;
        assert_eq!(capacity, 2000);
        let cache = DnsCache::new(capacity);
        for i in 0..=capacity {
            cache.put(
                &format!("h{i}.test"),
                CachedAddrs {
                    v4: vec![v4("10.0.0.1")],
                    v6: Vec::new(),
                    ttl: Duration::from_secs(60),
                    v6_queried: false,
                    source: "udp://192.0.2.1:53".to_string(),
                },
            );
        }
        assert_eq!(cache.len(), 2000);
        assert!(cache.get("h0.test").is_none(), "the oldest went");
        assert!(cache.get("h2000.test").is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn cache_fresh_stale_negative_and_coalescing() {
        let mock = MockDns::spawn().await;
        mock.set("c.test", &["10.0.0.1"], &[], 1);
        mock.set_empty("nx.test");
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let r = resolver_in_memory(&e, &mock, StaticSystemDns::default());
        let first = r.lookup("c.test", LookupOpts::default()).await.unwrap();
        assert!(matches!(first.source, Source::Upstream(_)));
        let second = r.lookup("c.test", LookupOpts::default()).await.unwrap();
        assert_eq!(second.source, Source::Cache { stale: false });
        assert_eq!(mock.query_count("c.test", Qtype::A), 1);
        // The background refresh below re-caches with a long TTL, so the
        // freshness assertion after it cannot race the original 1 s entry.
        mock.set("c.test", &["10.0.0.1"], &[], 60);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let stale = r.lookup("c.test", LookupOpts::default()).await.unwrap();
        assert_eq!(stale.source, Source::Cache { stale: true });
        assert_eq!(stale.v4, vec![v4("10.0.0.1")]);
        // On the paused clock this ends when every other task has run out of
        // work, the refresh with them.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            mock.query_count("c.test", Qtype::A),
            2,
            "one background refresh"
        );
        assert_eq!(
            r.lookup("c.test", LookupOpts::default())
                .await
                .unwrap()
                .source,
            Source::Cache { stale: false }
        );
        assert_eq!(
            r.lookup("nx.test", LookupOpts::default()).await,
            Err(DnsError::EmptyAnswer)
        );
        assert_eq!(
            r.lookup("nx.test", LookupOpts::default()).await,
            Err(DnsError::EmptyAnswer)
        );
        assert_eq!(
            mock.query_count("nx.test", Qtype::A),
            1,
            "negative answer cached"
        );
        let snap = r.cache_snapshot();
        assert_eq!(
            snap.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["c.test", "nx.test"]
        );
        // coalescing: two concurrent lookups of a new name → one upstream query
        mock.set("co.test", &["10.0.0.2"], &[], 60);
        mock.set_delay(Duration::from_millis(40));
        let r2 = r.clone();
        let (a, b) = tokio::join!(
            r.lookup("co.test", LookupOpts::default()),
            r2.lookup("co.test", LookupOpts::default())
        );
        assert_eq!(a.unwrap().v4, b.unwrap().v4);
        assert_eq!(mock.query_count("co.test", Qtype::A), 1);
        mock.set_delay(Duration::ZERO);
        r.flush();
        assert!(r.cache_snapshot().is_empty());
        let bypass = r
            .lookup(
                "c.test",
                LookupOpts {
                    bypass_cache: true,
                    want_v6: None,
                },
            )
            .await
            .unwrap();
        assert!(matches!(bypass.source, Source::Upstream(_)));
    }

    #[tokio::test]
    async fn a_v4_only_cache_entry_does_not_answer_a_caller_that_wants_aaaa() {
        let mock = MockDns::spawn().await;
        mock.set("dual.test", &["10.0.0.1"], &["fd00::1"], 60);
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let (r, _) = resolver_once(&e, StaticSystemDns::default());
        let v4_only = |bypass| LookupOpts {
            bypass_cache: bypass,
            want_v6: Some(false),
        };
        let dual = || LookupOpts {
            bypass_cache: false,
            want_v6: Some(true),
        };

        let first = r.lookup("dual.test", v4_only(false)).await.unwrap();
        assert!(first.v6.is_empty());
        assert_eq!(mock.query_count("dual.test", Qtype::A), 1);
        assert_eq!(mock.query_count("dual.test", Qtype::Aaaa), 0);

        // The v4-only entry cannot serve a caller that wants AAAA.
        let second = r.lookup("dual.test", dual()).await.unwrap();
        assert_eq!(second.v6, vec!["fd00::1".parse::<Ipv6Addr>().unwrap()]);
        assert!(matches!(second.source, Source::Upstream(_)));
        assert_eq!(
            mock.query_count("dual.test", Qtype::Aaaa),
            1,
            "exactly one AAAA query was sent"
        );

        // The replacement entry is v6-aware, so it serves both kinds of caller.
        let third = r.lookup("dual.test", dual()).await.unwrap();
        assert_eq!(third.source, Source::Cache { stale: false });
        assert_eq!(third.v6.len(), 1);
        assert_eq!(mock.query_count("dual.test", Qtype::Aaaa), 1);
        let fourth = r.lookup("dual.test", v4_only(false)).await.unwrap();
        assert_eq!(
            fourth.source,
            Source::Cache { stale: false },
            "a v6-aware entry still serves a caller that only wants v4"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_partial_result_completes_aaaa_in_the_background() {
        let mock = MockDns::spawn().await;
        mock.set("late.test", &["10.0.0.1"], &["fd00::1"], 60);
        // AAAA never answers, so the first lookup finishes at the resend tick
        // with only A in hand.
        mock.set_drop_qtype(Qtype::Aaaa, true);
        let e = env(&profile(
            &format!("dns-server = {}\nipv6 = true", mock.addr()),
            "",
        ));
        let sys = StaticSystemDns {
            has_ipv6: true,
            ..StaticSystemDns::default()
        };
        let r = resolver_in_memory(&e, &mock, sys);

        let first = r.lookup("late.test", LookupOpts::default()).await.unwrap();
        assert_eq!(first.v4, vec![v4("10.0.0.1")]);
        assert!(first.v6.is_empty(), "AAAA had not arrived yet");
        assert!(matches!(first.source, Source::Upstream(_)));
        // The completion query was spawned but, on this single-threaded
        // runtime, cannot have run before the next await point.
        mock.set_drop_qtype(Qtype::Aaaa, false);
        // On the paused clock this ends when every other task has run out of
        // work, the completion with them.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let second = r.lookup("late.test", LookupOpts::default()).await.unwrap();
        assert_eq!(
            second.source,
            Source::Cache { stale: false },
            "the completed answer is served from the cache"
        );
        assert_eq!(second.v6, vec!["fd00::1".parse::<Ipv6Addr>().unwrap()]);
        assert!(
            second.elapsed < Duration::from_millis(50),
            "a cache hit must not wait on the network, took {:?}",
            second.elapsed
        );
        assert_eq!(
            (
                mock.query_count("late.test", Qtype::A),
                mock.query_count("late.test", Qtype::Aaaa)
            ),
            (2, 2),
            "exactly one background completion query followed the partial result"
        );
    }

    /// A resolver that asks for AAAA and puts its questions to `mock` in
    /// memory, for the tests of what turns AAAA off.
    fn resolver_asking_aaaa(e: &Env, mock: &MockDns) -> Arc<Resolver> {
        let sys = StaticSystemDns {
            has_ipv6: true,
            ..StaticSystemDns::default()
        };
        resolver_in_memory(e, mock, sys)
    }

    /// `name` looked up, then a pause longer than a fanout lasts (300 ms):
    /// the background query that completes the lookup is over when this
    /// returns.
    async fn lookup_and_complete(r: &Resolver, name: &str) -> DnsResult {
        let res = r.lookup(name, LookupOpts::default()).await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        res
    }

    /// How the resolver counts what turns AAAA off (manual, DNS Overview: "If
    /// 5 consecutive lookups receive an A answer while the AAAA answer times
    /// out, Surge concludes that the upstream servers never return AAAA
    /// answers on the current network and stops sending AAAA questions").
    /// A lookup counts the timeout of its own query once: the background
    /// query that asks again for the AAAA answer the lookup did not wait for
    /// counts no second timeout. Names asked one after another, each
    /// completion over before the next name: the fifth name's lookup turns
    /// AAAA off, none before it, and the sixth name is not asked for AAAA.
    /// A flush turns it on again.
    #[tokio::test(start_paused = true)]
    async fn aaaa_is_turned_off_by_the_fifth_lookup_in_a_row_and_a_flush_turns_it_on() {
        let mock = MockDns::spawn().await;
        for i in 0..7 {
            mock.set(&format!("h{i}.test"), &["10.0.0.1"], &["fd00::1"], 60);
        }
        mock.set_drop_qtype(Qtype::Aaaa, true);
        let e = env(&profile(
            &format!("dns-server = {}\nipv6 = true", mock.addr()),
            "",
        ));
        let r = resolver_asking_aaaa(&e, &mock);
        for i in 0..5 {
            let name = format!("h{i}.test");
            let res = r.lookup(&name, LookupOpts::default()).await.unwrap();
            assert_eq!(res.v4.len(), 1);
            assert!(res.v6.is_empty());
            // One counted per name so far: the fifth is the first to turn it off.
            assert_eq!(r.aaaa_suppressed(), i >= 4, "h{i}: when its lookup is over");
            // Longer than a fanout lasts (300 ms): its completion is over too.
            tokio::time::sleep(Duration::from_secs(1)).await;
            assert_eq!(
                r.aaaa_suppressed(),
                i >= 4,
                "h{i}: when its completion is over"
            );
            assert_eq!(
                mock.query_count(&name, Qtype::Aaaa),
                2,
                "h{i}: the lookup asked, and its completion asked again"
            );
        }
        assert!(r.aaaa_suppressed());
        r.lookup("h5.test", LookupOpts::default()).await.unwrap();
        assert_eq!(
            mock.query_count("h5.test", Qtype::Aaaa),
            0,
            "AAAA no longer asked"
        );
        r.flush();
        assert!(!r.aaaa_suppressed());
        mock.set_drop_qtype(Qtype::Aaaa, false);
        let res = r.lookup("h6.test", LookupOpts::default()).await.unwrap();
        assert_eq!(res.v6.len(), 1);
        assert_eq!(mock.query_count("h6.test", Qtype::Aaaa), 1);
    }

    /// Lookups of different names that are under way together count as
    /// lookups one after another do: five of them turn AAAA off, four do not,
    /// whatever the background queries that follow them do. (Lookups of one
    /// name share a query and count once between them: the next test.)
    #[tokio::test(start_paused = true)]
    async fn aaaa_is_turned_off_by_five_lookups_at_once_and_not_by_four() {
        let mock = MockDns::spawn().await;
        for i in 0..5 {
            mock.set(&format!("h{i}.test"), &["10.0.0.1"], &["fd00::1"], 60);
        }
        mock.set_drop_qtype(Qtype::Aaaa, true);
        let e = env(&profile(
            &format!("dns-server = {}\nipv6 = true", mock.addr()),
            "",
        ));

        let four = resolver_asking_aaaa(&e, &mock);
        let answered = tokio::join!(
            four.lookup("h0.test", LookupOpts::default()),
            four.lookup("h1.test", LookupOpts::default()),
            four.lookup("h2.test", LookupOpts::default()),
            four.lookup("h3.test", LookupOpts::default()),
        );
        for res in [answered.0, answered.1, answered.2, answered.3] {
            let res = res.unwrap();
            assert_eq!(res.v4.len(), 1);
            assert!(res.v6.is_empty());
        }
        assert!(!four.aaaa_suppressed(), "four lookups are four");
        // Longer than a fanout lasts (300 ms): their completions are over.
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(
            mock.query_count("h3.test", Qtype::Aaaa),
            2,
            "the completion of a lookup asked again"
        );
        assert!(
            !four.aaaa_suppressed(),
            "four lookups are four, their completions included"
        );

        let five = resolver_asking_aaaa(&e, &mock);
        let answered = tokio::join!(
            five.lookup("h0.test", LookupOpts::default()),
            five.lookup("h1.test", LookupOpts::default()),
            five.lookup("h2.test", LookupOpts::default()),
            five.lookup("h3.test", LookupOpts::default()),
            five.lookup("h4.test", LookupOpts::default()),
        );
        for res in [answered.0, answered.1, answered.2, answered.3, answered.4] {
            let res = res.unwrap();
            assert_eq!(res.v4.len(), 1);
            assert!(res.v6.is_empty());
        }
        assert!(five.aaaa_suppressed(), "five lookups are five");
    }

    /// Lookups of one name that are under way together share one query, so a
    /// slow AAAA answer is one timeout and not as many as there are lookups:
    /// a browser that opens its connections to a host it has not looked up
    /// yet would otherwise turn AAAA off for the whole process with that one
    /// slow answer.
    #[tokio::test(start_paused = true)]
    async fn five_lookups_of_one_name_at_once_count_one_aaaa_timeout() {
        let mock = MockDns::spawn().await;
        for name in ["busy", "h0", "h1", "h2", "h3"] {
            mock.set(&format!("{name}.test"), &["10.0.0.1"], &["fd00::1"], 60);
        }
        mock.set_drop_qtype(Qtype::Aaaa, true);
        let e = env(&profile(
            &format!("dns-server = {}\nipv6 = true", mock.addr()),
            "",
        ));
        let r = resolver_asking_aaaa(&e, &mock);
        let asked = || {
            (
                mock.query_count("busy.test", Qtype::A),
                mock.query_count("busy.test", Qtype::Aaaa),
            )
        };

        let lookup = || r.lookup("busy.test", LookupOpts::default());
        let answered = tokio::join!(lookup(), lookup(), lookup(), lookup(), lookup());
        for res in [answered.0, answered.1, answered.2, answered.3, answered.4] {
            let res = res.unwrap();
            assert_eq!(res.v4.len(), 1);
            assert!(res.v6.is_empty());
        }
        assert!(
            !r.aaaa_suppressed(),
            "one slow answer, seen by five lookups"
        );
        // Longer than a fanout lasts (300 ms): the completion is over.
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(
            asked(),
            (2, 2),
            "one query and one completion for the five, as for a lookup alone"
        );
        assert!(
            !r.aaaa_suppressed(),
            "one slow answer, its completion included"
        );

        // That was one counted: three more names leave AAAA on, and the
        // next turns it off.
        for i in 0..3 {
            lookup_and_complete(&r, &format!("h{i}.test")).await;
            assert!(!r.aaaa_suppressed(), "h{i}: {} counted", i + 2);
        }
        lookup_and_complete(&r, "h3.test").await;
        assert!(r.aaaa_suppressed(), "the fifth counted");
    }

    /// A lookup that gets its AAAA answer shows that the servers do answer
    /// AAAA: the lookups in a row are counted from the start again.
    #[tokio::test(start_paused = true)]
    async fn a_lookup_with_an_aaaa_answer_starts_the_count_again() {
        let mock = MockDns::spawn().await;
        for i in 0..9 {
            mock.set(&format!("h{i}.test"), &["10.0.0.1"], &["fd00::1"], 60);
        }
        mock.set_drop_qtype(Qtype::Aaaa, true);
        let e = env(&profile(
            &format!("dns-server = {}\nipv6 = true", mock.addr()),
            "",
        ));
        let r = resolver_asking_aaaa(&e, &mock);
        for i in 0..3 {
            lookup_and_complete(&r, &format!("h{i}.test")).await;
        }
        assert!(!r.aaaa_suppressed(), "three in a row");

        // This one's AAAA answer arrives.
        mock.set_drop_qtype(Qtype::Aaaa, false);
        let answered = lookup_and_complete(&r, "h3.test").await;
        assert_eq!(answered.v6, vec!["fd00::1".parse::<Ipv6Addr>().unwrap()]);
        mock.set_drop_qtype(Qtype::Aaaa, true);

        for i in 4..8 {
            lookup_and_complete(&r, &format!("h{i}.test")).await;
            assert!(
                !r.aaaa_suppressed(),
                "h{i}: {} in a row since the AAAA answer",
                i - 3
            );
        }
        lookup_and_complete(&r, "h8.test").await;
        assert!(
            r.aaaa_suppressed(),
            "the fifth in a row since the AAAA answer"
        );
    }

    /// The background query that asks again counts no timeout, but when the
    /// AAAA answer comes in it, that shows that the servers do answer AAAA
    /// as well as an answer to a lookup does: the count starts again.
    #[tokio::test(start_paused = true)]
    async fn the_aaaa_answer_of_a_completion_starts_the_count_again() {
        let mock = MockDns::spawn().await;
        for i in 0..9 {
            mock.set(&format!("h{i}.test"), &["10.0.0.1"], &["fd00::1"], 60);
        }
        mock.set_drop_qtype(Qtype::Aaaa, true);
        let e = env(&profile(
            &format!("dns-server = {}\nipv6 = true", mock.addr()),
            "",
        ));
        let r = resolver_asking_aaaa(&e, &mock);
        for i in 0..3 {
            lookup_and_complete(&r, &format!("h{i}.test")).await;
        }

        // The fourth lookup times out, and the servers answer by the time
        // its completion asks again. The completion has been spawned but
        // cannot run before the next await point.
        let first = r.lookup("h3.test", LookupOpts::default()).await.unwrap();
        assert!(first.v6.is_empty(), "AAAA had not arrived yet");
        mock.set_drop_qtype(Qtype::Aaaa, false);
        tokio::time::sleep(Duration::from_secs(1)).await;
        let completed = r.lookup("h3.test", LookupOpts::default()).await.unwrap();
        assert_eq!(completed.source, Source::Cache { stale: false });
        assert_eq!(completed.v6, vec!["fd00::1".parse::<Ipv6Addr>().unwrap()]);
        mock.set_drop_qtype(Qtype::Aaaa, true);

        for i in 4..8 {
            lookup_and_complete(&r, &format!("h{i}.test")).await;
            assert!(
                !r.aaaa_suppressed(),
                "h{i}: {} in a row since the AAAA answer",
                i - 3
            );
        }
        lookup_and_complete(&r, "h8.test").await;
        assert!(
            r.aaaa_suppressed(),
            "the fifth in a row since the AAAA answer"
        );
    }

    /// The refresh of a stale cache entry is the only query made for its
    /// lookup, so it counts its AAAA timeout: five of them turn AAAA off.
    #[tokio::test(start_paused = true)]
    async fn the_refresh_of_a_stale_entry_counts_its_aaaa_timeout() {
        let mock = MockDns::spawn().await;
        for i in 0..5 {
            mock.set(&format!("h{i}.test"), &["10.0.0.1"], &["fd00::1"], 1);
        }
        let e = env(&profile(
            &format!("dns-server = {}\nipv6 = true", mock.addr()),
            "",
        ));
        let r = resolver_asking_aaaa(&e, &mock);
        for i in 0..5 {
            let res = r
                .lookup(&format!("h{i}.test"), LookupOpts::default())
                .await
                .unwrap();
            assert_eq!(res.v6.len(), 1);
        }

        // The entries have expired, and AAAA no longer answers.
        tokio::time::sleep(Duration::from_secs(2)).await;
        mock.set_drop_qtype(Qtype::Aaaa, true);
        for i in 0..5 {
            let name = format!("h{i}.test");
            let stale = r.lookup(&name, LookupOpts::default()).await.unwrap();
            assert_eq!(stale.source, Source::Cache { stale: true });
            // Longer than a fanout lasts (300 ms): the refresh is over.
            tokio::time::sleep(Duration::from_secs(1)).await;
            assert_eq!(mock.query_count(&name, Qtype::Aaaa), 2, "h{i}: refreshed");
            assert_eq!(r.aaaa_suppressed(), i >= 4, "h{i}: after its refresh");
        }
    }

    /// Six names looked up all at once, none of them answered with AAAA: all
    /// six lookups are under way before the first is over, so each starts out
    /// asking for AAAA and each is counted.
    async fn six_at_once(r: &Resolver, names: &[String]) {
        let lookup = |i: usize| r.lookup(&names[i], LookupOpts::default());
        let answered = tokio::join!(
            lookup(0),
            lookup(1),
            lookup(2),
            lookup(3),
            lookup(4),
            lookup(5)
        );
        for res in [
            answered.0, answered.1, answered.2, answered.3, answered.4, answered.5,
        ] {
            let res = res.unwrap();
            assert_eq!(res.v4.len(), 1);
            assert!(res.v6.is_empty());
        }
    }

    /// The line that says AAAA has been turned off is written each time it is
    /// turned off, and once: it says how many lookups in a row timed out and
    /// that a flush turns AAAA on again, and names no host.
    #[tokio::test(start_paused = true)]
    async fn turning_aaaa_off_is_said_once_each_time_without_naming_a_host() {
        let lines = Lines::default();
        // held across the awaits: the lookups run on this thread
        let _capturing = lines.capture();
        let mock = MockDns::spawn().await;
        let names: Vec<String> = (0..12).map(|i| format!("h{i}.test")).collect();
        for name in &names {
            mock.set(name, &["10.0.0.1"], &["fd00::1"], 60);
        }
        mock.set_drop_qtype(Qtype::Aaaa, true);
        let e = env(&profile(
            &format!("dns-server = {}\nipv6 = true", mock.addr()),
            "",
        ));
        let r = resolver_asking_aaaa(&e, &mock);

        // Six counts: the fifth turns AAAA off, the sixth finds it off already.
        six_at_once(&r, &names[..6]).await;
        assert!(r.aaaa_suppressed());
        let said = lines.said("AAAA answers timed out");
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("in 5 lookups in a row"), "{}", said[0]);
        assert!(
            said[0].contains("until the DNS cache is flushed"),
            "{}",
            said[0]
        );
        for name in &names[..6] {
            assert!(!said[0].contains(name.as_str()), "{}", said[0]);
        }

        // A flush turns it on again, and turning it off again is said again.
        r.flush();
        assert!(!r.aaaa_suppressed());
        six_at_once(&r, &names[6..]).await;
        assert!(r.aaaa_suppressed());
        assert_eq!(lines.said("AAAA answers timed out").len(), 2);
    }

    /// A flush starts the count of AAAA timeouts again: the lookups that
    /// timed out before it count for nothing after it. Only more timeouts
    /// after the flush show that, as an AAAA answer clears the count by itself.
    #[tokio::test(start_paused = true)]
    async fn a_flush_starts_the_count_of_aaaa_timeouts_again() {
        let mock = MockDns::spawn().await;
        for i in 0..9 {
            mock.set(&format!("h{i}.test"), &["10.0.0.1"], &["fd00::1"], 60);
        }
        mock.set_drop_qtype(Qtype::Aaaa, true);
        let e = env(&profile(
            &format!("dns-server = {}\nipv6 = true", mock.addr()),
            "",
        ));
        let r = resolver_asking_aaaa(&e, &mock);
        for i in 0..4 {
            lookup_and_complete(&r, &format!("h{i}.test")).await;
            assert!(!r.aaaa_suppressed(), "h{i}: {} counted", i + 1);
        }

        // Without the flush the next lookup would be the fifth.
        r.flush();
        for i in 4..8 {
            lookup_and_complete(&r, &format!("h{i}.test")).await;
            assert!(!r.aaaa_suppressed(), "h{i}: {} since the flush", i - 3);
        }
        lookup_and_complete(&r, "h8.test").await;
        assert!(r.aaaa_suppressed(), "the fifth since the flush");
    }

    #[tokio::test]
    async fn encrypted_upstreams_take_over_and_udp_bootstraps_them() {
        let udp = MockDns::spawn().await;
        let tcp = MockDns::spawn().await;
        udp.set("dns.example", &["127.0.0.1"], &[], 60);
        tcp.set("a.test", &["10.0.0.42"], &[], 60);
        udp.set("a.test", &["10.0.0.1"], &[], 60);
        let e = env(&profile(
            &format!(
                "dns-server = {}\nencrypted-dns-server = tcp://dns.example:{}",
                udp.addr(),
                tcp.addr().port()
            ),
            "",
        ));
        let (r, diags) = resolver_once(&e, StaticSystemDns::default());
        assert!(diags.is_empty());
        assert_eq!(
            r.primary_upstreams(),
            vec![format!("tcp://dns.example:{}", tcp.addr().port())]
        );
        let a = r.lookup("a.test", LookupOpts::default()).await.unwrap();
        assert_eq!(
            a.v4,
            vec![v4("10.0.0.42")],
            "normal queries go to the encrypted subsystem only"
        );
        assert_eq!(
            udp.query_count("dns.example", Qtype::A),
            1,
            "UDP only bootstrapped the tcp:// hostname"
        );
        assert_eq!(udp.query_count("a.test", Qtype::A), 0);
        assert_eq!(tcp.query_count("a.test", Qtype::A), 1);
    }

    /// Encrypted DNS checks the server's certificate unless the profile says
    /// `encrypted-dns-skip-cert-verification = true` (manual, Encrypted DNS):
    /// a DoT or DoH server whose certificate nobody vouches for is refused,
    /// and answers once the check is off.
    #[tokio::test]
    async fn a_certificate_nobody_vouches_for_is_refused_unless_the_profile_skips_the_check() {
        use crate::message::{Question, Rcode, build_response};
        use rurge_net::testing::TestServer;
        let dot = MockDns::spawn_tls().await;
        dot.set("a.test", &["10.0.0.3"], &[], 60);
        let doh = TestServer::spawn_tls().await;
        let question = Question {
            name: "a.test".to_string(),
            qtype: Qtype::A,
        };
        let answer = [("10.0.0.4".parse().unwrap(), 60)];
        doh.set(
            "/dns-query",
            build_response(0, &question, Rcode::NoError, &answer, false).unwrap(),
        );
        doh.set_header("/dns-query", "content-type", "application/dns-message");
        let quick = FanoutOpts {
            resend: Duration::from_millis(100),
            attempts: 2,
        };
        for (server, address) in [
            (format!("tls://{}", dot.addr()), "10.0.0.3"),
            (doh.url("/dns-query").to_string(), "10.0.0.4"),
        ] {
            let system = || -> Arc<dyn SystemDns> { Arc::new(StaticSystemDns::default()) };
            let strict = env(&profile(
                &format!("encrypted-dns-server = {server}\nipv6 = false"),
                "",
            ));
            let (r, _) = resolver_with_fanout(&strict, system(), quick.clone());
            assert!(
                r.lookup("a.test", LookupOpts::default()).await.is_err(),
                "{server} is refused"
            );
            let lax = env(&profile(
                &format!(
                    "encrypted-dns-server = {server}\nencrypted-dns-skip-cert-verification = true\nipv6 = false"
                ),
                "",
            ));
            let (r, _) = resolver_with_fanout(&lax, system(), quick.clone());
            let found = r.lookup("a.test", LookupOpts::default()).await.unwrap();
            assert_eq!(found.v4, vec![v4(address)], "{server} answers");
        }
    }

    /// Duplicate servers are ignored (manual, DNS Servers): a server the
    /// profile names twice, or names and `system` brings in again, is asked
    /// once.
    #[tokio::test]
    async fn a_server_named_twice_is_asked_once() {
        let mock = MockDns::spawn().await;
        let other = MockDns::spawn().await;
        let e = env(&profile(
            &format!(
                "dns-server = {0}, {1}, {0}, system, {1}",
                mock.addr(),
                other.addr()
            ),
            "",
        ));
        let sys = StaticSystemDns {
            servers: vec![mock.addr()],
            ..StaticSystemDns::default()
        };
        let (r, _) = resolver_once(&e, sys);
        assert_eq!(
            r.primary_upstreams(),
            vec![
                format!("udp://{}", mock.addr()),
                format!("udp://{}", other.addr())
            ]
        );
    }

    #[tokio::test]
    async fn network_change_keeps_the_configured_bootstrap_servers() {
        let udp = MockDns::spawn().await;
        let tcp = MockDns::spawn().await;
        let old_system = MockDns::spawn().await;
        let new_system = MockDns::spawn().await;

        // Encrypted upstreams own the primary set, so the profile's UDP server
        // exists only to bootstrap them — a network change must not drop it.
        let e = env(&profile(
            &format!(
                "dns-server = {}\nencrypted-dns-server = tcp://dns.example:{}",
                udp.addr(),
                tcp.addr().port()
            ),
            "",
        ));
        let sys = Arc::new(SwapSystemDns::new(old_system.addr()));
        let (r, _) = resolver_with(&e, sys.clone());
        assert_eq!(
            r.bootstrap_upstreams(),
            vec![format!("udp://{}", udp.addr())]
        );
        sys.set(new_system.addr());
        r.on_network_change();
        assert_eq!(
            r.bootstrap_upstreams(),
            vec![format!("udp://{}", udp.addr())],
            "the profile's UDP servers keep bootstrapping the encrypted upstreams"
        );

        // With no dns-server at all the system list *is* the bootstrap set, and
        // it follows the network.
        let e2 = env(&profile("", ""));
        let sys2 = Arc::new(SwapSystemDns::new(old_system.addr()));
        let (r2, _) = resolver_with(&e2, sys2.clone());
        assert_eq!(
            r2.bootstrap_upstreams(),
            vec![format!("udp://{}", old_system.addr())]
        );
        sys2.set(new_system.addr());
        r2.on_network_change();
        assert_eq!(
            r2.bootstrap_upstreams(),
            vec![format!("udp://{}", new_system.addr())]
        );
        assert_eq!(
            r2.primary_upstreams(),
            vec![format!("udp://{}", new_system.addr())]
        );
    }

    #[tokio::test]
    async fn network_change_re_expands_the_system_keyword() {
        let udp = MockDns::spawn().await;
        let old_system = MockDns::spawn().await;
        let new_system = MockDns::spawn().await;

        // `dns-server = system`: both sets follow the platform's list.
        let e = env(&profile("dns-server = system", ""));
        let sys = Arc::new(SwapSystemDns::new(old_system.addr()));
        let (r, _) = resolver_with(&e, sys.clone());
        assert_eq!(
            r.primary_upstreams(),
            vec![format!("udp://{}", old_system.addr())]
        );
        sys.set(new_system.addr());
        r.on_network_change();
        assert_eq!(
            r.primary_upstreams(),
            vec![format!("udp://{}", new_system.addr())],
            "`system` is re-expanded, not snapshotted at construction"
        );
        assert_eq!(
            r.bootstrap_upstreams(),
            vec![format!("udp://{}", new_system.addr())]
        );

        // `dns-server = <udp>, system`: the configured entry stays, the system
        // half swaps.
        let e2 = env(&profile(
            &format!("dns-server = {}, system", udp.addr()),
            "",
        ));
        let sys2 = Arc::new(SwapSystemDns::new(old_system.addr()));
        let (r2, _) = resolver_with(&e2, sys2.clone());
        assert_eq!(
            r2.primary_upstreams(),
            vec![
                format!("udp://{}", udp.addr()),
                format!("udp://{}", old_system.addr())
            ]
        );
        sys2.set(new_system.addr());
        r2.on_network_change();
        let expected = vec![
            format!("udp://{}", udp.addr()),
            format!("udp://{}", new_system.addr()),
        ];
        assert_eq!(r2.primary_upstreams(), expected);
        assert_eq!(r2.bootstrap_upstreams(), expected);
    }

    #[tokio::test]
    async fn unsupported_upstreams_warn_and_ipv6_servers_drop_when_ipv6_is_off() {
        let mock = MockDns::spawn().await;
        let e = env(&profile(
            &format!(
                "dns-server = {}, [::1]:5353\nencrypted-dns-server = quic://dns.example/dns-query",
                mock.addr()
            ),
            "",
        ));
        let (r, diags) = resolver(&e, StaticSystemDns::default());
        assert!(
            diags
                .iter()
                .any(|d| d.code == codes::W_DNS_UPSTREAM_UNSUPPORTED)
        );
        assert_eq!(
            r.primary_upstreams(),
            vec![format!("udp://{}", mock.addr())]
        );
    }

    /// An IPv6 server of `dns-server` is dropped when `ipv6 = false` and kept,
    /// where the profile wrote it, when `ipv6 = true` (manual, DNS Servers).
    #[tokio::test]
    async fn an_ipv6_dns_server_is_kept_when_ipv6_is_on_and_dropped_when_it_is_off() {
        let mock = MockDns::spawn().await;
        for (ipv6, kept) in [("false", false), ("true", true)] {
            let e = env(&profile(
                &format!("dns-server = {}, [::1]:5353\nipv6 = {ipv6}", mock.addr()),
                "",
            ));
            let (r, _) = resolver(&e, StaticSystemDns::default());
            let mut expected = vec![format!("udp://{}", mock.addr())];
            if kept {
                expected.push("udp://[::1]:5353".to_string());
            }
            assert_eq!(r.primary_upstreams(), expected, "ipv6 = {ipv6}");
        }
    }

    /// A user name and a password written into a server's URL are in no
    /// upstream's name and in no warning: `https://` takes them — DoH sends
    /// them as it always did —, the other schemes refuse them, and a URL
    /// that cannot be used is quoted from its last `@` on.
    #[tokio::test]
    async fn the_credentials_of_a_dns_server_are_in_no_name_and_no_warning() {
        const PASSWORD: &str = "s3cret-doh";
        let e = env(&profile(
            &format!(
                "encrypted-dns-server = https://alice:{PASSWORD}@dns.example/dns-query, \
https://alice:{PASSWORD}@dns.example:99999/q, quic://alice:{PASSWORD}@dns.example, \
h3://alice:123/{PASSWORD}@dns.example"
            ),
            "",
        ));
        let (r, diags) = resolver(&e, StaticSystemDns::default());
        assert_eq!(r.primary_upstreams(), ["https://dns.example/dns-query"]);
        let mut texts: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
        assert_eq!(texts.len(), 3, "{texts:?}");
        texts.extend(r.primary_upstreams());
        for text in &texts {
            assert!(
                !text.contains(PASSWORD) && !text.contains("alice"),
                "{text}"
            );
        }
    }

    /// An `https://` server with an `@` beyond its authority — a `/` in its
    /// password or in its user name — is no upstream, in
    /// `encrypted-dns-server` and in `[Host]`: W0026, and neither a warning
    /// nor an upstream's name says what stands before that `@`.
    #[tokio::test]
    async fn an_https_server_with_an_at_sign_beyond_its_authority_is_no_upstream() {
        const SECRET: &str = "hunt3r2-doh";
        let e = env(&profile(
            &format!(
                "encrypted-dns-server = https://al1ce:123/{SECRET}@dns.example, \
https://al1ce:/{SECRET}@dns.example/q, https://t0k3n/{SECRET}@dns.example/q, \
https://dns.example/dns-query"
            ),
            &format!("leak.test = server:https://al1ce:123/{SECRET}@dns.example"),
        ));
        let (r, diags) = resolver(&e, StaticSystemDns::default());
        assert_eq!(r.primary_upstreams(), ["https://dns.example/dns-query"]);
        assert!(r.host_lookup("leak.test").is_none());
        let mut texts: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
        assert_eq!(texts.len(), 4, "{texts:?}");
        assert!(
            diags
                .iter()
                .all(|d| d.code == codes::W_DNS_UPSTREAM_UNSUPPORTED),
            "{texts:?}"
        );
        texts.extend(r.primary_upstreams());
        for text in &texts {
            for secret in [SECRET, "al1ce", "t0k3n"] {
                assert!(!text.contains(secret), "{text}");
            }
        }
    }

    /// `quic://` and `h3://` servers are upstreams like the others, in
    /// `encrypted-dns-server` and in `[Host]`: no W0026.
    #[tokio::test]
    async fn quic_and_h3_upstreams_are_built_without_a_warning() {
        let e = env(&profile(
            "encrypted-dns-server = quic://127.0.0.1, h3://dns.example:8443",
            "quic.test = server:quic://[::1]:8853",
        ));
        let (r, diags) = resolver(&e, StaticSystemDns::default());
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| d.to_string()).collect::<Vec<_>>()
        );
        assert_eq!(
            r.primary_upstreams(),
            vec![
                "quic://127.0.0.1:853".to_string(),
                "h3://dns.example:8443/dns-query".to_string()
            ]
        );
        assert!(matches!(
            r.host_lookup("quic.test").map(|hit| hit.action),
            Some(HostAction::Servers(specs)) if specs == [UpstreamSpec::Quic {
                host: "::1".into(),
                port: 8853
            }]
        ));
    }

    #[tokio::test]
    async fn no_dns_server_means_the_system_servers_are_the_upstreams() {
        let system = MockDns::spawn().await;
        system.set("s.test", &["10.30.0.1"], &[], 60);
        let e = env(&profile("", ""));
        let (r, _) = resolver_once(
            &e,
            StaticSystemDns {
                servers: vec![system.addr()],
                ..StaticSystemDns::default()
            },
        );
        assert!(
            r.primary_upstreams()
                .contains(&format!("udp://{}", system.addr()))
        );
        let s = r.lookup("s.test", LookupOpts::default()).await.unwrap();
        assert_eq!(s.v4, vec![v4("10.30.0.1")]);
        let (r2, _) = resolver_once(&e, StaticSystemDns::default());
        assert!(r2.primary_upstreams().is_empty());
        let lo = r2.lookup("localhost", LookupOpts::default()).await.unwrap();
        assert_eq!(lo.source, Source::Loopback);
    }

    #[tokio::test]
    async fn etc_hosts_are_read_and_watched() {
        let mock = MockDns::spawn().await;
        let dir = tempfile::tempdir().unwrap();
        let hosts_path = dir.path().join("hosts");
        std::fs::write(&hosts_path, "10.40.0.1 nas.lan\n").unwrap();
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let sys = StaticSystemDns {
            hosts_path: Some(hosts_path.clone()),
            ..StaticSystemDns::default()
        };
        let (r, _) = resolver(&e, sys);
        let n = r.lookup("nas.lan", LookupOpts::default()).await.unwrap();
        assert_eq!(
            (n.v4, n.source),
            (vec![v4("10.40.0.1")], Source::Host(HostKind::EtcHosts))
        );
        std::fs::write(&hosts_path, "10.40.0.2 nas.lan\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let n = r.lookup("nas.lan", LookupOpts::default()).await.unwrap();
            if n.v4 == vec![v4("10.40.0.2")] {
                break;
            }
            assert!(Instant::now() < deadline, "hosts file change not picked up");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The manager knows the hosts file for what it is: something the system
    /// keeps.
    #[tokio::test]
    async fn the_hosts_file_is_a_system_resource() {
        let mock = MockDns::spawn().await;
        let dir = tempfile::tempdir().unwrap();
        let hosts_path = dir.path().join("hosts");
        std::fs::write(&hosts_path, "10.40.0.1 nas.lan\n").unwrap();
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let sys = StaticSystemDns {
            hosts_path: Some(hosts_path.clone()),
            ..StaticSystemDns::default()
        };
        let (_resolver, _) = resolver(&e, sys);
        let statuses = e.resources.statuses();
        assert_eq!(statuses.len(), 1, "{statuses:?}");
        assert_eq!(statuses[0].source, ResourceSource::File(hosts_path));
        assert_eq!(statuses[0].kind, ResourceKind::System);
        assert!(statuses[0].local && statuses[0].ready, "{statuses:?}");
    }

    /// `read-etc-hosts = false` (manual, Local DNS Mapping) turns the hosts
    /// file off: its names go to the servers like any other, and the file is
    /// not even watched.
    #[tokio::test]
    async fn read_etc_hosts_false_leaves_the_hosts_file_alone() {
        let mock = MockDns::spawn().await;
        mock.set("nas.lan", &["10.40.0.9"], &[], 60);
        let dir = tempfile::tempdir().unwrap();
        let hosts_path = dir.path().join("hosts");
        std::fs::write(&hosts_path, "10.40.0.1 nas.lan\n").unwrap();
        let e = env(&profile(
            &format!("dns-server = {}\nread-etc-hosts = false", mock.addr()),
            "",
        ));
        let sys = StaticSystemDns {
            hosts_path: Some(hosts_path),
            ..StaticSystemDns::default()
        };
        let (r, _) = resolver_once(&e, sys);
        let n = r.lookup("nas.lan", LookupOpts::default()).await.unwrap();
        assert_eq!(n.v4, vec![v4("10.40.0.9")], "the server's answer");
        assert!(matches!(n.source, Source::Upstream(_)), "{:?}", n.source);
        assert!(e.resources.statuses().is_empty(), "the file is not watched");
    }

    #[tokio::test]
    async fn host_lookup_reads_the_host_section_without_any_query() {
        let dns = MockDns::spawn().await;
        let e = env(&profile(
            &format!("dns-server = {}", dns.addr()),
            "pinned.test = 10.1.2.3, 10.1.2.4\nalias.test = other.test\n",
        ));
        let (r, _) = resolver(&e, StaticSystemDns::default());
        let hit = r.host_lookup("Pinned.Test.").expect("a [Host] item");
        assert_eq!(
            hit.action,
            HostAction::Ips(vec![
                "10.1.2.3".parse().unwrap(),
                "10.1.2.4".parse().unwrap()
            ])
        );
        assert!(matches!(
            r.host_lookup("alias.test").map(|h| h.action),
            Some(HostAction::Alias(_))
        ));
        assert!(r.host_lookup("unknown.test").is_none());
        assert!(dns.queries().is_empty(), "no query leaves the process");
    }

    #[tokio::test]
    async fn implements_lazy_resolver_and_resolve() {
        let mock = MockDns::spawn().await;
        mock.set("t.test", &["10.0.0.1"], &[], 60);
        mock.set_empty("nx.test");
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let (r, _) = resolver_once(&e, StaticSystemDns::default());
        let lazy: &dyn LazyResolver = r.as_ref();
        assert_eq!(
            lazy.resolve("t.test").await.unwrap().v4,
            vec![v4("10.0.0.1")]
        );
        assert_eq!(
            lazy.resolve("nx.test").await,
            Err(ResolveError::EmptyAnswer)
        );
        let res: &dyn Resolve = r.as_ref();
        assert_eq!(
            res.resolve("t.test").await.unwrap(),
            vec![IpAddr::V4(v4("10.0.0.1"))]
        );
        assert!(res.resolve("nx.test").await.is_err());
        let delays = r.measure_delay("t.test").await;
        assert_eq!(delays.len(), 1);
        assert!(delays[0].result.is_ok());
    }

    /// A direct connector that notes where its UDP flows go: the interface
    /// a `Via` stands for.
    struct Noting {
        inner: DirectConnector,
        udp: Mutex<Vec<Target>>,
    }

    impl Noting {
        fn via(key: &str) -> (Via, Arc<Noting>) {
            let noting = Arc::new(Noting {
                inner: DirectConnector::new(Arc::new(SystemResolve)),
                udp: Mutex::new(Vec::new()),
            });
            let via = Via {
                key: key.to_string(),
                connector: noting.clone(),
            };
            (via, noting)
        }

        fn udp(&self) -> Vec<Target> {
            self.udp.lock().expect("udp").clone()
        }
    }

    impl Connector for Noting {
        fn connect<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            self.inner.connect(target, opts)
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            self.udp.lock().expect("udp").push(target.clone());
            self.inner.connect_udp(target, opts)
        }
    }

    /// Lookups through a `Via` ask the plain servers through its connector
    /// and keep their answers apart; `[Host]` answers them as usual (phase 2
    /// M5 design 8.5).
    #[tokio::test]
    async fn lookups_via_an_interface_go_through_it_with_answers_of_their_own() {
        let mock = MockDns::spawn().await;
        mock.set("a.test", &["10.0.0.1"], &[], 60);
        let e = env(&profile(
            &format!("dns-server = {}", mock.addr()),
            "b.test = 10.0.0.9",
        ));
        let (r, _) = resolver_once(&e, StaticSystemDns::default());
        let (via, noting) = Noting::via("en1");

        let a = r
            .lookup_via("a.test", LookupOpts::default(), &via)
            .await
            .unwrap();
        assert_eq!(a.v4, vec![v4("10.0.0.1")]);
        let server = Target::new(HostName::Ip(mock.addr().ip()), mock.addr().port());
        assert_eq!(noting.udp(), vec![server]);
        assert_eq!(mock.query_count("a.test", Qtype::A), 1);

        // the lookup through the interface filled no answer of the global one
        r.lookup("a.test", LookupOpts::default()).await.unwrap();
        assert_eq!(mock.query_count("a.test", Qtype::A), 2);
        // and its own answer is kept
        r.lookup_via("a.test", LookupOpts::default(), &via)
            .await
            .unwrap();
        assert_eq!(mock.query_count("a.test", Qtype::A), 2);

        let b = r
            .lookup_via("b.test", LookupOpts::default(), &via)
            .await
            .unwrap();
        assert_eq!(b.v4, vec![v4("10.0.0.9")]);
        assert_eq!(mock.query_count("b.test", Qtype::A), 0);
        assert_eq!(noting.udp().len(), 1, "one flow carries every question");
    }

    /// With encrypted DNS configured, a lookup through a `Via` asks the
    /// encrypted servers as usual, not through its connector (phase 2 M5
    /// design 8.5).
    #[tokio::test]
    async fn with_encrypted_dns_lookups_via_an_interface_go_as_usual() {
        let udp = MockDns::spawn().await;
        let tcp = MockDns::spawn().await;
        udp.set("dns.example", &["127.0.0.1"], &[], 60);
        tcp.set("a.test", &["10.0.0.42"], &[], 60);
        let e = env(&profile(
            &format!(
                "dns-server = {}\nencrypted-dns-server = tcp://dns.example:{}",
                udp.addr(),
                tcp.addr().port()
            ),
            "",
        ));
        let (r, _) = resolver_once(&e, StaticSystemDns::default());
        let (via, noting) = Noting::via("en1");
        let a = r
            .lookup_via("a.test", LookupOpts::default(), &via)
            .await
            .unwrap();
        assert_eq!(a.v4, vec![v4("10.0.0.42")]);
        assert!(noting.udp().is_empty());
        assert_eq!(tcp.query_count("a.test", Qtype::A), 1);
    }

    /// With no plain server to ask through the interface (no `dns-server`
    /// and no system server readable), a lookup through a `Via` goes the
    /// usual way instead of asking nothing.
    #[tokio::test]
    async fn with_no_plain_server_lookups_via_an_interface_go_as_usual() {
        let e = env(&profile("", ""));
        let (r, _) = resolver(&e, StaticSystemDns::default());
        assert!(r.primary_upstreams().is_empty());
        r.cache.put(
            "a.test",
            CachedAddrs {
                v4: vec![v4("10.0.0.7")],
                v6: Vec::new(),
                ttl: Duration::from_secs(60),
                v6_queried: true,
                source: "system".to_string(),
            },
        );
        let (via, noting) = Noting::via("en1");
        let a = r
            .lookup_via("a.test", LookupOpts::default(), &via)
            .await
            .unwrap();
        assert_eq!(
            (a.v4, a.source),
            (vec![v4("10.0.0.7")], Source::Cache { stale: false })
        );
        assert!(noting.udp().is_empty());
    }

    /// What a connector that dials nothing was asked: its own kind, the call,
    /// the port.
    #[derive(Debug, PartialEq)]
    struct Asked {
        kind: &'static str,
        call: &'static str,
        port: u16,
    }

    /// A connector that dials nothing: it refuses, and notes what it was
    /// asked in a list its fellows share.
    struct Refusing {
        kind: &'static str,
        asked: Arc<Mutex<Vec<Asked>>>,
    }

    impl Refusing {
        fn refuse(&self, call: &'static str, target: &Target) -> io::Error {
            self.asked.lock().expect("asked").push(Asked {
                kind: self.kind,
                call,
                port: target.port,
            });
            io::Error::new(io::ErrorKind::ConnectionRefused, "refused")
        }
    }

    impl Connector for Refusing {
        fn connect<'a>(
            &'a self,
            target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            Box::pin(std::future::ready(Err(self.refuse("connect", target))))
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            Box::pin(std::future::ready(Err(self.refuse("connect_udp", target))))
        }
    }

    /// Every kind of upstream is dialled through the connector of its kind,
    /// and through no other: a resolver of each kind of server is given five
    /// connectors that refuse, and the only one asked is its own, for the
    /// port of its server. The ports are marks to tell the servers by —
    /// nothing is dialled at them. TCP, TLS and DoH connect; the two QUIC
    /// kinds open a UDP flow.
    #[tokio::test(start_paused = true)]
    async fn each_kind_of_upstream_dials_through_its_own_connector() {
        for (server, kind, call, port) in [
            ("tcp://127.0.0.1:1", "tcp", "connect", 1),
            ("tls://127.0.0.1:2", "tls", "connect", 2),
            ("https://127.0.0.1:3/dns-query", "https", "connect", 3),
            ("quic://127.0.0.1:4", "quic", "connect_udp", 4),
            ("h3://127.0.0.1:5/dns-query", "h3", "connect_udp", 5),
        ] {
            let asked = Arc::new(Mutex::new(Vec::new()));
            let refusing = |kind| -> Arc<dyn Connector> {
                Arc::new(Refusing {
                    kind,
                    asked: asked.clone(),
                })
            };
            let connectors = DnsConnectors {
                tcp: refusing("tcp"),
                tls: refusing("tls"),
                https: refusing("https"),
                quic: refusing("quic"),
                h3: refusing("h3"),
            };
            let e = env(&profile(&format!("encrypted-dns-server = {server}"), ""));
            let mut cfg = ResolverConfig::from_config(&e.cfg);
            cfg.fanout = once();
            let (r, _) = Resolver::new(
                cfg,
                ResolverDeps {
                    connectors,
                    sets: e.sets.clone(),
                    system: Arc::new(StaticSystemDns::default()),
                    resources: e.resources.clone(),
                },
            );
            r.lookup("a.test", LookupOpts::default())
                .await
                .expect_err("every connector refuses");
            assert_eq!(
                *asked.lock().unwrap(),
                [Asked { kind, call, port }],
                "{server}"
            );
        }
    }
}
