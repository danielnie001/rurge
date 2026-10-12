//! One config generation's shared objects (M2 design §5, M3 design §7.1):
//! resource manager → set registry → GeoIP → resolver. `rurge run`,
//! `rule match` and `dns lookup` all build it here.

use rurge_config::{Config, Diagnostics};
use rurge_dns::system::SystemDns;
use rurge_dns::{Resolver, ResolverConfig, ResolverDeps};
use rurge_net::connector::{Connector, DirectConnector, SystemResolve};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions, ResourceStatus};
use rurge_rules::{GeoDb, GeoUpdater, GeoUrls, SetRegistry, SetStatus};
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
    /// Wait this long for the first fetch of every external resource (zero = don't wait),
    /// and after it as long again at most for the sets that arrived to be compiled.
    pub wait: Duration,
    /// Connector for the resolver's TCP / DoT / DoH upstreams and the QUIC
    /// carriers of its DoQ / DoH3 ones; `None` = plain direct.
    /// `Runtime::build` injects the pipeline connector here when
    /// `encrypted-dns-follow-outbound-mode` is on.
    pub dns_connector: Option<Arc<dyn Connector>>,
    /// Interface binding and TOS for outbound sockets (the bin injects
    /// `rurge-platform`; everything else uses `NoopSocketHook`).
    pub socket_hook: Arc<dyn rurge_net::socket::SocketHook>,
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
/// `opts.wait` for the first fetch of every resource, and for what arrived to
/// be applied (`settle`; both skipped in `--no-network` mode).
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
    let resources = ResourceManager::with_options(
        opts.data_dir.clone(),
        resource_client()?,
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
        settle(&registry, &geo, &resources, SETTLE, opts.wait).await;
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

/// The HTTP client that downloads external resources and the sources of the
/// profile: direct connections, never through the pipeline, so that an
/// engine that refuses sessions can still update.
pub fn resource_client() -> anyhow::Result<Arc<HttpClient>> {
    let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
    Ok(Arc::new(HttpClient::new(
        connector,
        HttpClientConfig {
            // Subscription panels pick the format of what they send by
            // User-Agent; "Surge" in it asks for Surge's.
            user_agent: format!("rurge/{} (Surge-compatible)", env!("CARGO_PKG_VERSION")),
            ..HttpClientConfig::default()
        },
    )?))
}

/// How long `settle` waits for what may not come at all: a set whose resource
/// has not arrived, a database that is being installed.
const SETTLE: Duration = Duration::from_secs(2);

/// Gives the background reload / install tasks up to `unsettled` to apply
/// resources that just arrived, and goes on for up to `queued` while a set
/// has not compiled a version its resource holds: the sets of a registry are
/// compiled one at a time, so those that arrive together take the sum of
/// their times, and the turn of each is sure to come. When that time runs out
/// all the same, it says how many sets it leaves behind.
async fn settle(
    registry: &SetRegistry,
    geo: &GeoDb,
    resources: &ResourceManager,
    unsettled: Duration,
    queued: Duration,
) {
    let began = tokio::time::Instant::now();
    loop {
        let held = resources.statuses();
        let sets = registry.statuses();
        let behind = sets_behind(&sets, &held);
        let sets_pending = sets
            .iter()
            .any(|s| s.state == "missing" || s.state == "compiling");
        let geo_pending = {
            let info = geo.info();
            let available = |name: &str| {
                held.iter()
                    .any(|s| s.state == "available" && s.source.to_string().ends_with(name))
            };
            (info.country_epoch.is_none() && available("GeoLite2-Country.mmdb"))
                || (info.asn_epoch.is_none() && available("GeoLite2-ASN.mmdb"))
        };
        let waited = began.elapsed();
        let go_on = (behind > 0 && waited < queued)
            || ((sets_pending || geo_pending) && waited < unsettled);
        if !go_on {
            if behind > 0 {
                tracing::warn!(
                    sets = behind,
                    "sets that have arrived are not compiled yet: they are used as they were until they are"
                );
            }
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// How many of `sets` have not compiled the version the manager holds of the
/// resource they are read from. Each of them was told of that version and
/// waits for its turn, or is at it.
fn sets_behind(sets: &[SetStatus], held: &[ResourceStatus]) -> usize {
    sets.iter()
        .filter(|set| {
            held.iter().any(|resource| {
                set.key.as_deref() == Some(&resource.key) && resource.version > set.version
            })
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::resource::{ResourceKind, ResourceSource, resource_key};
    use rurge_net::testing::TestServer;
    use rurge_rules::set_format::SetKind;

    fn set(key: Option<&str>, version: u64) -> SetStatus {
        SetStatus {
            name: "a set".to_string(),
            key: key.map(str::to_string),
            kind: SetKind::RuleSet,
            entries: 0,
            version,
            state: "ok".to_string(),
            skipped: 0,
            truncated: 0,
            unknown_params: 0,
        }
    }

    /// What the manager says of a resource it holds this version of; 0 is one
    /// it has no copy of.
    fn resource(key: &str, version: u64) -> ResourceStatus {
        ResourceStatus {
            source: ResourceSource::Url("http://sets.example/a.list".parse().unwrap()),
            state: if version == 0 { "missing" } else { "available" },
            version,
            fetched_at: None,
            next_refresh: None,
            last_error: None,
            kind: ResourceKind::RuleSet,
            label: None,
            key: key.to_string(),
            local: false,
            ready: version != 0,
            updating: false,
        }
    }

    /// A set is behind while the manager holds a version of its resource that
    /// it has not compiled, and only then.
    #[test]
    fn a_set_is_behind_while_its_resource_holds_a_version_it_has_not_compiled() {
        let held = [resource("a", 2), resource("b", 1), resource("c", 0)];
        // each has compiled what there is, or there is nothing yet
        let level = [set(Some("a"), 2), set(Some("b"), 1), set(Some("c"), 0)];
        assert_eq!(sets_behind(&level, &held), 0);
        // one has an older version, one has not loaded at all
        let late = [set(Some("a"), 1), set(Some("b"), 0), set(Some("c"), 0)];
        assert_eq!(sets_behind(&late, &held), 2);
        // a set that has no resource, and one whose resource the manager does
        // not have
        assert_eq!(sets_behind(&[set(None, 0), set(Some("d"), 0)], &held), 0);
        // two sets read from one resource are two sets
        let twice = [set(Some("a"), 1), set(Some("a"), 0), set(Some("a"), 2)];
        assert_eq!(sets_behind(&twice, &held), 2);
        assert_eq!(sets_behind(&[], &held), 0);
        assert_eq!(sets_behind(&late, &[]), 0);
    }

    /// What has arrived is compiled when `settle` returns, however short the
    /// time it has for what may not come: with none at all, a set whose
    /// content arrived a moment ago is still waited for. Several threads, as
    /// the commands that wait have them: the set is compiled beside the task
    /// that waits, and takes far longer than the manager takes to say that it
    /// has arrived.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn what_has_arrived_is_compiled_when_the_wait_is_over() {
        const LINES: usize = 100_000;
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        let mut text = String::with_capacity(LINES * 32);
        for i in 0..LINES {
            text += &format!("DOMAIN-SUFFIX,n{i}.example\n");
        }
        server.set("/big.list", text);
        let url = server.url("/big.list");
        let loaded = from_text(
            &format!("[Proxy]\nP = direct\n[Rule]\nRULE-SET,{url},P\nFINAL,DIRECT\n"),
            &dir.path().join("t.conf"),
            &LoadOptions::for_tests(),
        );
        assert!(!loaded.diagnostics.has_errors());
        let resources = ResourceManager::with_options(
            dir.path().to_path_buf(),
            resource_client().unwrap(),
            ResourceOptions::default(),
        );
        let (registry, _) = SetRegistry::build(&loaded.config, resources.clone(), dir.path());
        let (geo, _) = GeoDb::open(&dir.path().join("geoip"));
        resources.wait_initial(Duration::from_secs(10)).await;

        let no_time = Duration::ZERO;
        settle(
            &registry,
            &geo,
            &resources,
            no_time,
            Duration::from_secs(60),
        )
        .await;
        let key = resource_key(&ResourceSource::Url(url));
        let set = registry
            .statuses()
            .into_iter()
            .find(|set| set.key.as_deref() == Some(key.as_str()))
            .expect("the set of the profile");
        assert_eq!((set.entries, set.state.as_str()), (LINES, "ok"));
    }
}
