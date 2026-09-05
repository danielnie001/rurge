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
use crate::upstream::tcp::TcpUpstream;
use crate::upstream::udp::UdpUpstream;
use crate::upstream::{UpstreamRef, UpstreamSpec};
use arc_swap::ArcSwap;
use rurge_config::general::{DnsServer, EncryptedDns};
use rurge_config::host::HostEntry;
use rurge_config::{Config, Diagnostic, Diagnostics, codes};
use rurge_net::BoxFuture;
use rurge_net::connector::{Connector, Resolve};
use rurge_net::http::{HttpClient, HttpClientConfig, tls_client_config};
use rurge_net::resource::{ResourceManager, ResourceSource, ResourceSpec, ResourceState};
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

pub struct ResolverDeps {
    pub connector: Arc<dyn Connector>,
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

pub struct Resolver {
    cfg: ResolverConfig,
    connector: Arc<dyn Connector>,
    system: Arc<dyn SystemDns>,
    bootstrap: Arc<Bootstrap>,
    http: Arc<HttpClient>,
    tls: Option<Arc<rustls::ClientConfig>>,
    primary_specs: Vec<UpstreamSpec>,
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
}

impl Resolver {
    /// Must be called inside a tokio runtime: it arms the hosts-file watcher.
    pub fn new(cfg: ResolverConfig, deps: ResolverDeps) -> (Arc<Resolver>, Diagnostics) {
        let mut diags = Diagnostics::default();
        let has_ipv6 = deps.system.has_ipv6();

        // Traditional (plain UDP) servers; `system` expands to the platform's list.
        let mut traditional: Vec<UpstreamSpec> = Vec::new();
        let mut wants_system = false;
        for s in &cfg.servers {
            match UpstreamSpec::from_dns_server(s) {
                Some(UpstreamSpec::Udp(addr)) if addr.is_ipv6() && !cfg.ipv6 => {
                    tracing::info!(server = %addr, "IPv6 DNS server ignored because ipv6 is off");
                }
                Some(spec) => traditional.push(spec),
                None => wants_system = true,
            }
        }
        let system_specs: Vec<UpstreamSpec> = deps
            .system
            .servers()
            .into_iter()
            .map(UpstreamSpec::Udp)
            .collect();
        if wants_system {
            traditional.extend(system_specs.iter().cloned());
        }

        // Encrypted subsystem: https:// tls:// tcp://
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

        let bootstrap_specs = if traditional.is_empty() {
            system_specs.clone()
        } else {
            traditional.clone()
        };
        let bootstrap = Bootstrap::new(
            bootstrap_specs.iter().map(build_plain_udp).collect(),
            cfg.ipv6 && has_ipv6,
            cfg.fanout.clone(),
        );
        let bootstrap_connector: Arc<dyn Connector> = Arc::new(BootstrapConnector::new(
            deps.connector.clone(),
            bootstrap.clone(),
        ));
        let http = match HttpClient::new(
            bootstrap_connector.clone(),
            HttpClientConfig {
                skip_cert_verification: cfg.skip_cert_verification,
                ..HttpClientConfig::default()
            },
        ) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                diags.push(Diagnostic::warning(
                    codes::W_DNS_UPSTREAM_UNSUPPORTED,
                    format!("DoH client unavailable: {e}"),
                ));
                // A client that can never connect; DoH upstreams will fail per query.
                Arc::new(
                    HttpClient::new(bootstrap_connector.clone(), HttpClientConfig::default())
                        .expect("default http client"),
                )
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

        let primary_specs = if !encrypted.is_empty() {
            encrypted
        } else if !traditional.is_empty() {
            traditional
        } else {
            system_specs.clone()
        };

        let hosts = HostMap::build(&cfg.hosts, deps.sets.as_ref(), &mut diags);

        let resolver = Arc::new(Resolver {
            connector: bootstrap_connector,
            system: deps.system.clone(),
            bootstrap,
            http,
            tls,
            primary: ArcSwap::from_pointee(Vec::new()),
            system_upstreams: ArcSwap::from_pointee(
                system_specs.iter().map(build_plain_udp).collect(),
            ),
            host_upstreams: Mutex::new(HashMap::new()),
            cache: DnsCache::new(cfg.cache_capacity),
            hosts,
            inflight: Mutex::new(HashMap::new()),
            aaaa_failures: AtomicU32::new(0),
            aaaa_suppressed: AtomicBool::new(false),
            has_ipv6: AtomicBool::new(has_ipv6),
            self_weak: Mutex::new(Weak::new()),
            primary_specs,
            cfg,
        });
        *resolver.self_weak.lock().expect("weak") = Arc::downgrade(&resolver);
        resolver.rebuild_primary();
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
                self.connector.clone(),
            ))),
            UpstreamSpec::Tls { host, port } => self.tls.as_ref().map(|tls| {
                Arc::new(TcpUpstream::tls(
                    host,
                    *port,
                    self.connector.clone(),
                    tls.clone(),
                )) as UpstreamRef
            }),
            UpstreamSpec::Https(url) => {
                Some(Arc::new(DohUpstream::new(url.clone(), self.http.clone())))
            }
        }
    }

    fn rebuild_primary(&self) {
        let list: Vec<UpstreamRef> = self
            .primary_specs
            .iter()
            .filter_map(|s| self.build_upstream(s))
            .collect();
        self.primary.store(Arc::new(list));
    }

    pub fn primary_upstreams(&self) -> Vec<String> {
        self.primary
            .load()
            .iter()
            .map(|u| u.name().to_string())
            .collect()
    }

    pub fn aaaa_suppressed(&self) -> bool {
        self.aaaa_suppressed.load(Ordering::Relaxed)
    }

    pub fn cache_snapshot(&self) -> Vec<CacheEntry> {
        self.cache.snapshot()
    }

    pub fn flush(&self) {
        self.cache.flush();
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
        self.system_upstreams
            .store(Arc::new(system_specs.iter().map(build_plain_udp).collect()));
        let traditional: Vec<UpstreamSpec> = self
            .primary_specs
            .iter()
            .filter(|s| s.is_traditional())
            .cloned()
            .collect();
        let bootstrap_specs = if traditional.is_empty() {
            system_specs
        } else {
            traditional
        };
        self.bootstrap
            .set_upstreams(bootstrap_specs.iter().map(build_plain_udp).collect());
        self.rebuild_primary();
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
        let started = Instant::now();
        let trimmed = host.trim();
        let bare = trimmed.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = bare.parse::<IpAddr>() {
            return Ok(literal(ip, Source::Literal, started));
        }
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
                    let answers = self.query_coalesced(&ups, &current, want_v6, &opts).await?;
                    return Ok(from_answers(
                        &answers,
                        Source::Host(HostKind::Server),
                        started,
                    ));
                }
                HostAction::System(_) => {
                    return self
                        .system_lookup(&current, want_v6, Source::Host(HostKind::System), started)
                        .await;
                }
            }
        }

        // Special names.
        if current.ends_with(".local") {
            return self
                .system_lookup(&current, want_v6, Source::System, started)
                .await;
        }
        if !current.contains('.') && !no_search {
            let candidate = match self.system.search_domains().first() {
                Some(d) => format!("{current}.{}", d.trim_matches('.').to_ascii_lowercase()),
                None => current.clone(),
            };
            return self
                .system_lookup(&candidate, want_v6, Source::System, started)
                .await;
        }

        // Cache.
        if !opts.bypass_cache {
            match self.cache.get(&current) {
                Some(CacheHit::Fresh(a)) => {
                    return Ok(from_cached(&a, Source::Cache { stale: false }, started));
                }
                Some(CacheHit::Stale(a)) => {
                    self.spawn_refresh(current.clone(), want_v6);
                    return Ok(from_cached(&a, Source::Cache { stale: true }, started));
                }
                Some(CacheHit::Negative) => return Err(DnsError::EmptyAnswer),
                None => {}
            }
        }

        let primary = self.primary.load_full();
        if primary.is_empty() {
            return self
                .system_lookup(&current, want_v6, Source::System, started)
                .await;
        }
        let result = self
            .query_coalesced(&primary, &current, want_v6, &opts)
            .await;
        self.record(&current, &result, want_v6);
        let answers = result?;
        Ok(from_answers(
            &answers,
            Source::Upstream(answers.upstream.clone()),
            started,
        ))
    }

    /// Stores the outcome in the cache and feeds AAAA suppression.
    fn record(&self, name: &str, result: &Result<Answers, DnsError>, want_v6: bool) {
        match result {
            Ok(a) => {
                self.cache.put(name, cached_from(a));
                if want_v6 {
                    if a.aaaa_timed_out {
                        let n = self.aaaa_failures.fetch_add(1, Ordering::Relaxed) + 1;
                        if n >= AAAA_SUPPRESS_AFTER
                            && !self.aaaa_suppressed.swap(true, Ordering::Relaxed)
                        {
                            tracing::warn!(
                                "AAAA answers timed out {n} times in a row; AAAA queries suppressed until the next flush or network change"
                            );
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

    fn spawn_refresh(&self, name: String, want_v6: bool) {
        if !self.cache.begin_refresh(&name) {
            return;
        }
        let weak = self.self_weak.lock().expect("weak").clone();
        tokio::spawn(async move {
            let Some(me) = weak.upgrade() else {
                return;
            };
            let primary = me.primary.load_full();
            let result = me
                .query_coalesced(&primary, &name, want_v6, &LookupOpts::default())
                .await;
            match &result {
                Ok(_) | Err(DnsError::EmptyAnswer) => me.record(&name, &result, want_v6),
                Err(_) => me.cache.end_refresh(&name),
            }
        });
    }

    /// One in-flight query per (name, family); later callers wait for the first.
    async fn query_coalesced(
        &self,
        upstreams: &[UpstreamRef],
        name: &str,
        want_v6: bool,
        opts: &LookupOpts,
    ) -> Result<Answers, DnsError> {
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
                    return v;
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
        result
    }

    async fn system_lookup(
        &self,
        name: &str,
        want_v6: bool,
        source: Source,
        started: Instant,
    ) -> Result<DnsResult, DnsError> {
        let system = self.system_upstreams.load_full();
        if !system.is_empty() {
            let answers = self
                .query_coalesced(&system, name, want_v6, &LookupOpts::default())
                .await?;
            return Ok(from_answers(&answers, source, started));
        }
        let addrs = tokio::net::lookup_host((name, 0))
            .await
            .map_err(|e| DnsError::AllFailed(vec![("system".to_string(), e.to_string())]))?;
        let ips: Vec<IpAddr> = addrs.map(|sa| sa.ip()).collect();
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
        });
        if let Some((data, _)) = handle.current().data() {
            self.hosts
                .set_etc_hosts(parse_hosts_file(&String::from_utf8_lossy(&data)));
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut rx = handle.subscribe();
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

fn build_plain_udp(spec: &UpstreamSpec) -> UpstreamRef {
    match spec {
        UpstreamSpec::Udp(addr) => Arc::new(UdpUpstream::new(*addr)),
        other => unreachable!("build_plain_udp called with {other:?}"),
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

fn cached_from(a: &Answers) -> CachedAddrs {
    CachedAddrs {
        v4: a.v4.iter().map(|(ip, _)| *ip).collect(),
        v6: a.v6.iter().map(|(ip, _)| *ip).collect(),
        ttl: min_ttl(a),
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

impl Resolve for Resolver {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            let r = self
                .lookup(host, LookupOpts::default())
                .await
                .map_err(io::Error::other)?;
            let addrs = r.addrs();
            if addrs.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no addresses for {host}"),
                ));
            }
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Qtype;
    use crate::system::StaticSystemDns;
    use crate::testing::MockDns;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::resource::ResourceOptions;

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

    fn resolver(
        e: &Env,
        servers: &str,
        extra: &str,
        system: StaticSystemDns,
    ) -> (Arc<Resolver>, Diagnostics) {
        let _ = (servers, extra);
        let mut cfg = ResolverConfig::from_config(&e.cfg);
        cfg.fanout = fast();
        Resolver::new(
            cfg,
            ResolverDeps {
                connector: Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
                sets: e.sets.clone(),
                system: Arc::new(system),
                resources: e.resources.clone(),
            },
        )
    }

    fn profile(general: &str, hosts: &str) -> String {
        format!(
            "[General]\n{general}\n[Proxy]\nProxyA = http, proxy.example, 8080\n[Host]\n{hosts}\n[Rule]\nFINAL,DIRECT\n"
        )
    }

    fn v4(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[tokio::test]
    async fn literals_loopback_and_trailing_dot() {
        let mock = MockDns::spawn().await;
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let (r, diags) = resolver(&e, "", "", StaticSystemDns::default());
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

    #[tokio::test]
    async fn host_chain_ips_aliases_loops_and_proxy_exemption() {
        let mock = MockDns::spawn().await;
        mock.set("proxy.example", &["10.9.9.9"], &[], 60);
        let e = env(&profile(
            &format!("dns-server = {}", mock.addr()),
            "fixed.test = 1.2.3.4, ::2\nalias.test = fixed.test\nloop-a.test = loop-b.test\nloop-b.test = loop-a.test\nproxy.example = 127.0.0.1\nip-alias.test = 9.9.9.9\n",
        ));
        let (r, _) = resolver(&e, "", "", StaticSystemDns::default());
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

    #[tokio::test]
    async fn host_server_and_system_modes_and_special_names() {
        let primary = MockDns::spawn().await;
        let dedicated = MockDns::spawn().await;
        let system = MockDns::spawn().await;
        dedicated.set("corp.test", &["10.10.0.1"], &[], 60);
        system.set("printer.local", &["10.20.0.1"], &[], 60);
        system.set("nas.home.lan", &["10.20.0.2"], &[], 60);
        system.set("sys.test", &["10.20.0.3"], &[], 60);
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
            ..StaticSystemDns::default()
        };
        let (r, _) = resolver(&e, "", "", sys);
        let c = r.lookup("corp.test", LookupOpts::default()).await.unwrap();
        assert_eq!(
            (c.v4, c.source),
            (vec![v4("10.10.0.1")], Source::Host(HostKind::Server))
        );
        assert_eq!(primary.query_count("corp.test", Qtype::A), 0);
        let s = r.lookup("sys.test", LookupOpts::default()).await.unwrap();
        assert_eq!(
            (s.v4, s.source),
            (vec![v4("10.20.0.3")], Source::Host(HostKind::System))
        );
        let l = r
            .lookup("printer.local", LookupOpts::default())
            .await
            .unwrap();
        assert_eq!((l.v4, l.source), (vec![v4("10.20.0.1")], Source::System));
        let n = r.lookup("nas", LookupOpts::default()).await.unwrap();
        assert_eq!(
            n.v4,
            vec![v4("10.20.0.2")],
            "single label + first search domain"
        );
        assert_eq!(system.query_count("nas.home.lan", Qtype::A), 1);
        assert_eq!(primary.query_count("nas.home.lan", Qtype::A), 0);
    }

    #[tokio::test]
    async fn cache_fresh_stale_negative_and_coalescing() {
        let mock = MockDns::spawn().await;
        mock.set("c.test", &["10.0.0.1"], &[], 1);
        mock.set_empty("nx.test");
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let (r, _) = resolver(&e, "", "", StaticSystemDns::default());
        let first = r.lookup("c.test", LookupOpts::default()).await.unwrap();
        assert!(matches!(first.source, Source::Upstream(_)));
        let second = r.lookup("c.test", LookupOpts::default()).await.unwrap();
        assert_eq!(second.source, Source::Cache { stale: false });
        assert_eq!(mock.query_count("c.test", Qtype::A), 1);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let stale = r.lookup("c.test", LookupOpts::default()).await.unwrap();
        assert_eq!(stale.source, Source::Cache { stale: true });
        assert_eq!(stale.v4, vec![v4("10.0.0.1")]);
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
        mock.set_delay(Duration::from_millis(80));
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
    async fn aaaa_suppression_after_five_timeouts_and_flush_resumes() {
        let mock = MockDns::spawn().await;
        for i in 0..7 {
            mock.set(&format!("h{i}.test"), &["10.0.0.1"], &["fd00::1"], 60);
        }
        mock.set_drop_qtype(Qtype::Aaaa, true);
        let e = env(&profile(
            &format!("dns-server = {}\nipv6 = true", mock.addr()),
            "",
        ));
        let sys = StaticSystemDns {
            has_ipv6: true,
            ..StaticSystemDns::default()
        };
        let (r, _) = resolver(&e, "", "", sys);
        for i in 0..5 {
            let res = r
                .lookup(&format!("h{i}.test"), LookupOpts::default())
                .await
                .unwrap();
            assert_eq!(res.v4.len(), 1);
            assert!(res.v6.is_empty());
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
        let (r, diags) = resolver(&e, "", "", StaticSystemDns::default());
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

    #[tokio::test]
    async fn unsupported_upstreams_warn_and_ipv6_servers_drop_when_ipv6_is_off() {
        let mock = MockDns::spawn().await;
        let e = env(&profile(
            &format!(
                "dns-server = {}, [::1]:5353\nencrypted-dns-server = h3://dns.example/dns-query",
                mock.addr()
            ),
            "",
        ));
        let (r, diags) = resolver(&e, "", "", StaticSystemDns::default());
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

    #[tokio::test]
    async fn no_servers_uses_system_upstreams_then_lookup_host() {
        let system = MockDns::spawn().await;
        system.set("s.test", &["10.30.0.1"], &[], 60);
        let e = env(&profile("", ""));
        let (r, _) = resolver(
            &e,
            "",
            "",
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
        let (r2, _) = resolver(&e, "", "", StaticSystemDns::default());
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
        let (r, _) = resolver(&e, "", "", sys);
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

    #[tokio::test]
    async fn implements_lazy_resolver_and_resolve() {
        let mock = MockDns::spawn().await;
        mock.set("t.test", &["10.0.0.1"], &[], 60);
        mock.set_empty("nx.test");
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let (r, _) = resolver(&e, "", "", StaticSystemDns::default());
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
}
