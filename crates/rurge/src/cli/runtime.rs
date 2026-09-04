//! rurge-specific runtime options (FR-CFG-17): command-line flags and
//! environment variables only, never profile keys. Also assembles the shared
//! objects (resource manager, set registry, GeoIP) the offline commands need.

use anyhow::Context;
use clap::Args;
use rurge_config::{Config, Diagnostics};
use rurge_net::BoxFuture;
use rurge_net::connector::{DirectConnector, SystemResolve};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions};
use rurge_rules::engine::{LazyResolver, ResolveError};
use rurge_rules::matcher::ResolvedAddrs;
use rurge_rules::{GeoDb, GeoUpdater, GeoUrls, SetRegistry};
use std::path::{Path, PathBuf};
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
}

#[derive(Clone, Debug)]
pub struct Runtime {
    pub data_dir: PathBuf,
    pub geo_urls: GeoUrls,
    pub no_network: bool,
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
        })
    }
}

pub struct Stack {
    /// Kept alive so the manager's background refresh tasks keep running;
    /// callers reach individual resources through `registry` and `geo`.
    #[allow(dead_code)]
    pub resources: Arc<ResourceManager>,
    pub registry: Arc<SetRegistry>,
    pub geo: Arc<GeoDb>,
    /// Kept alive so its install tasks are not aborted by `Drop`.
    #[allow(dead_code)]
    pub geo_updater: Option<GeoUpdater>,
    pub diagnostics: Diagnostics,
}

/// Builds resources → set registry → GeoIP, then waits up to `wait` for the
/// first fetch of every resource (skipped in `--no-network` mode).
pub async fn build_stack(cfg: &Config, rt: &Runtime, wait: Duration) -> anyhow::Result<Stack> {
    let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
    let client = Arc::new(HttpClient::new(connector, HttpClientConfig::default())?);
    let resources = ResourceManager::with_options(
        rt.data_dir.clone(),
        client,
        ResourceOptions {
            offline: rt.no_network,
            ..ResourceOptions::default()
        },
    );
    let base_dir = cfg
        .source
        .main
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let (registry, mut diagnostics) = SetRegistry::build(cfg, resources.clone(), &base_dir);
    let (geo, geo_diags) = GeoDb::open(&rt.data_dir.join("geoip"));
    for d in geo_diags {
        diagnostics.push(d);
    }
    let geo_updater = (!rt.no_network).then(|| {
        GeoUpdater::spawn(
            geo.clone(),
            resources.clone(),
            rt.geo_urls.clone(),
            !cfg.general.disable_geoip_db_auto_update,
        )
    });
    if !rt.no_network && !wait.is_zero() {
        resources.wait_initial(wait).await;
        settle(&registry, &geo, &resources).await;
    }
    Ok(Stack {
        resources,
        registry,
        geo,
        geo_updater,
        diagnostics,
    })
}

/// Gives the background reload / install tasks up to two seconds to apply
/// resources that just arrived.
async fn settle(registry: &SetRegistry, geo: &GeoDb, resources: &ResourceManager) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let sets_pending = registry
            .statuses()
            .iter()
            .any(|s| s.state == "missing" || s.state == "compiling");
        let geo_pending = {
            let info = geo.info();
            let available = |name: &str| {
                resources
                    .statuses()
                    .iter()
                    .any(|s| s.state == "available" && s.source.to_string().ends_with(name))
            };
            (info.country_epoch.is_none() && available("GeoLite2-Country.mmdb"))
                || (info.asn_epoch.is_none() && available("GeoLite2-ASN.mmdb"))
        };
        if (!sets_pending && !geo_pending) || tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// System resolver adapter used until M2b delivers `rurge-dns`.
pub struct SystemLazyResolver;

impl LazyResolver for SystemLazyResolver {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<ResolvedAddrs, ResolveError>> {
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host, 0))
                .await
                .map_err(|e| ResolveError::Failed(e.to_string()))?;
            let mut out = ResolvedAddrs::default();
            for sa in addrs {
                match sa.ip() {
                    std::net::IpAddr::V4(v4) => out.v4.push(v4),
                    std::net::IpAddr::V6(v6) => out.v6.push(v6),
                }
            }
            if out.v4.is_empty() && out.v6.is_empty() {
                return Err(ResolveError::EmptyAnswer);
            }
            Ok(out)
        })
    }
}
