//! One config generation's shared objects (M2 design §5, M3 design §7.1):
//! resource manager → set registry → GeoIP → resolver. `rurge run`,
//! `rule match` and `dns lookup` all build it here.

use rurge_config::{Config, Diagnostics};
use rurge_dns::system::SystemDns;
use rurge_dns::{Resolver, ResolverConfig, ResolverDeps};
use rurge_net::connector::{Connector, DirectConnector, SystemResolve};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions};
use rurge_rules::{GeoDb, GeoUpdater, GeoUrls, SetRegistry};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub struct StackOptions {
    pub data_dir: PathBuf,
    pub no_network: bool,
    pub geo_urls: GeoUrls,
    pub dns_cache_size: usize,
    /// Platform DNS facts (the bin injects `rurge-platform`; tests use `StaticSystemDns`).
    pub system: Arc<dyn SystemDns>,
    /// Wait this long for the first fetch of every external resource (zero = don't wait).
    pub wait: Duration,
    /// Connector for the resolver's TCP / DoT / DoH upstreams; `None` = plain
    /// direct. `Runtime::build` injects the pipeline connector here when
    /// `encrypted-dns-follow-outbound-mode` is on.
    pub dns_connector: Option<Arc<dyn Connector>>,
}

pub struct Stack {
    /// Kept alive so the manager's background refresh tasks keep running;
    /// callers reach individual resources through `registry` and `geo`.
    pub resources: Arc<ResourceManager>,
    pub registry: Arc<SetRegistry>,
    pub geo: Arc<GeoDb>,
    /// Kept alive so its install tasks are not aborted by `Drop`.
    pub geo_updater: Option<GeoUpdater>,
    pub resolver: Arc<Resolver>,
    pub diagnostics: Diagnostics,
}

/// Builds resources → set registry → GeoIP → resolver, then waits up to
/// `opts.wait` for the first fetch of every resource (skipped in `--no-network` mode).
pub async fn build_stack(cfg: &Config, opts: &StackOptions) -> anyhow::Result<Stack> {
    build_stack_with(cfg, opts, |_| {}).await
}

/// `build_stack` with a hook that edits the resolver configuration before
/// the resolver is built (`dns lookup --server`).
pub async fn build_stack_with(
    cfg: &Config,
    opts: &StackOptions,
    customize: impl FnOnce(&mut ResolverConfig),
) -> anyhow::Result<Stack> {
    let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
    let client = Arc::new(HttpClient::new(
        connector.clone(),
        HttpClientConfig::default(),
    )?);
    let resources = ResourceManager::with_options(
        opts.data_dir.clone(),
        client,
        ResourceOptions {
            offline: opts.no_network,
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
    let (geo, geo_diags) = GeoDb::open(&opts.data_dir.join("geoip"));
    for d in geo_diags {
        diagnostics.push(d);
    }
    let geo_updater = (!opts.no_network).then(|| {
        GeoUpdater::spawn(
            geo.clone(),
            resources.clone(),
            opts.geo_urls.clone(),
            !cfg.general.disable_geoip_db_auto_update,
        )
    });
    let mut resolver_cfg = ResolverConfig::from_config(cfg);
    resolver_cfg.cache_capacity = opts.dns_cache_size;
    customize(&mut resolver_cfg);
    // `connector` is `Arc<DirectConnector>`; coerce explicitly so both arms unify.
    let resolver_connector: Arc<dyn Connector> = match &opts.dns_connector {
        Some(c) => c.clone(),
        None => connector.clone() as Arc<dyn Connector>,
    };
    let (resolver, dns_diags) = Resolver::new(
        resolver_cfg,
        ResolverDeps {
            connector: resolver_connector,
            sets: registry.clone(),
            system: opts.system.clone(),
            resources: resources.clone(),
        },
    );
    diagnostics.extend(dns_diags);
    if !opts.no_network && !opts.wait.is_zero() {
        resources.wait_initial(opts.wait).await;
        settle(&registry, &geo, &resources).await;
    }
    Ok(Stack {
        resources,
        registry,
        geo,
        geo_updater,
        resolver,
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
