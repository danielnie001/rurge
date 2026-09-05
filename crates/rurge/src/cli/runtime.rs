//! rurge-specific runtime options (FR-CFG-17): command-line flags and
//! environment variables only, never profile keys. Also assembles the shared
//! objects (resource manager, set registry, GeoIP, resolver) the offline
//! commands need.

use anyhow::Context;
use clap::Args;
use rurge_config::Config;
use rurge_dns::cache::DEFAULT_CAPACITY;
use rurge_dns::system::SystemDns;
use rurge_rules::GeoUrls;
use std::net::SocketAddr;
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
    #[arg(long, env = "RURGE_GEOIP_URL", value_name = "URL")]
    pub geoip_url: Option<Url>,
    /// GeoIP ASN database URL
    #[arg(long, env = "RURGE_GEOIP_ASN_URL", value_name = "URL")]
    pub geoip_asn_url: Option<Url>,
    /// Never touch the network: use cached resources only
    #[arg(long, env = "RURGE_NO_NETWORK")]
    pub no_network: bool,
    /// DNS cache capacity in entries (default 2000)
    #[arg(long, env = "RURGE_DNS_CACHE_SIZE", value_name = "N")]
    pub dns_cache_size: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Runtime {
    pub data_dir: PathBuf,
    pub geo_urls: GeoUrls,
    pub no_network: bool,
    pub dns_cache_size: usize,
}

impl RuntimeArgs {
    pub fn resolve(&self, cfg: &Config) -> anyhow::Result<Runtime> {
        let data_dir = self
            .data_dir
            .clone()
            .unwrap_or_else(rurge_platform::dirs::data_dir);
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("cannot create data dir {}", data_dir.display()))?;
        let mut geo_urls = GeoUrls::default();
        if let Some(u) = &cfg.general.geoip_maxmind_url {
            match Url::parse(u) {
                Ok(parsed) => geo_urls.country = parsed,
                Err(e) => eprintln!("warning: ignoring geoip-maxmind-url `{u}`: {e}"),
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
use rurge_engine::stack::StackOptions;

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
        }
    }
}

/// Builds resources → set registry → GeoIP → resolver, then waits up to
/// `wait` for the first fetch of every resource (skipped in `--no-network` mode).
/// Delegates to `rurge_engine::stack::build_stack`.
pub async fn build_stack(cfg: &Config, rt: &Runtime, wait: Duration) -> anyhow::Result<Stack> {
    rurge_engine::stack::build_stack(cfg, &rt.stack_options(wait)).await
}

/// `build_stack` with a hook that edits the resolver configuration before
/// the resolver is built (`dns lookup --server`). Delegates to
/// `rurge_engine::stack::build_stack_with`.
pub async fn build_stack_with(
    cfg: &Config,
    rt: &Runtime,
    wait: Duration,
    customize: impl FnOnce(&mut rurge_dns::ResolverConfig),
) -> anyhow::Result<Stack> {
    rurge_engine::stack::build_stack_with(cfg, &rt.stack_options(wait), customize).await
}

/// `rurge-platform::dns` behind the `SystemDns` trait (AR-02: platform code
/// stays in rurge-platform; rurge-dns only sees the trait).
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
}
