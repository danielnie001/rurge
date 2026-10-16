//! One config generation's shared objects (M2 design §5, M3 design §7.1):
//! resource manager → set registry → GeoIP → resolver. `rurge run`,
//! `rule match` and `dns lookup` all build it here.

use rurge_config::general::{DnsServer, EncryptedDns};
use rurge_config::{Config, Diagnostic, Diagnostics, NetworkFacts, codes};
use rurge_dns::system::SystemDns;
use rurge_dns::{DnsConnectors, Resolver, ResolverConfig, ResolverDeps};
use rurge_net::connector::{DirectConnector, SystemResolve};
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
    /// and after it for what arrived to be applied: two seconds, and up to this
    /// long once more while sets that arrived wait to be compiled.
    pub wait: Duration,
    /// Connectors for the resolver's TCP / DoT / DoH upstreams and the QUIC
    /// carriers of its DoQ / DoH3 ones, one per kind; `None` = plain direct.
    /// `Runtime::build` injects those of the pipeline connector here when
    /// `encrypted-dns-follow-outbound-mode` is on.
    pub dns_connector: Option<DnsConnectors>,
    /// Interface binding and TOS for outbound sockets (the bin injects
    /// `rurge-platform`; everything else uses `NoopSocketHook`).
    pub socket_hook: Arc<dyn rurge_net::socket::SocketHook>,
    /// How the network the machine is on changes the DNS the profile asks
    /// for; applied to the resolver's configuration before the hook of
    /// `build_stack_with` sees it. `Runtime::build` replaces it with the one
    /// it works out from the engine's network; a command that builds a stack
    /// without an engine leaves it at the default, which changes nothing, or
    /// works it out itself (`DnsOverride::for_network`).
    pub dns_override: DnsOverride,
}

/// How the network the machine is on changes the DNS of the profile (phase 3
/// M1 design 7.3): the line of `[SSID Setting]` the network matches may list
/// `dns-server`s and `encrypted-dns-server`s, and each list it gives takes the
/// place of the profile's whole list while the machine is on that network.
/// What the line does not list stays the profile's.
///
/// Two overrides are equal when they change the same things in the same way,
/// so whoever holds the override a generation was built with can tell whether
/// another network would change the DNS differently.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DnsOverride {
    /// `Some`: these replace `dns-server`.
    pub servers: Option<Vec<DnsServer>>,
    /// `Some`: these replace `encrypted-dns-server`; `Some` of nothing is
    /// `encrypted-dns-server = off`.
    pub encrypted: Option<Vec<EncryptedDns>>,
}

impl DnsOverride {
    /// The override of the line of `[SSID Setting]` that `facts` match: the
    /// lists it gives. It changes nothing when no line matches, nor for a line
    /// that gives no DNS.
    pub fn for_network(cfg: &Config, facts: &NetworkFacts) -> DnsOverride {
        cfg.effective_ssid_setting(facts)
            .map(|line| DnsOverride {
                servers: line.dns_server.clone(),
                encrypted: line.encrypted_dns_server.clone(),
            })
            .unwrap_or_default()
    }

    /// Puts what the override sets into `resolver`, in place of the profile's.
    pub fn apply(&self, resolver: &mut ResolverConfig) {
        if let Some(servers) = &self.servers {
            resolver.servers.clone_from(servers);
        }
        if let Some(encrypted) = &self.encrypted {
            resolver.encrypted.clone_from(encrypted);
        }
    }
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
/// `opts.wait` for the first fetch of every resource, and after that for what
/// arrived to be applied (`settle`, which has a time of its own; both skipped
/// in `--no-network` mode). Sets it could not wait for are a warning among
/// the diagnostics of the stack.
pub async fn build_stack(cfg: &Config, opts: &StackOptions) -> anyhow::Result<Stack> {
    build_stack_with(cfg, opts, |_| {}).await
}

/// `build_stack` with a hook that edits the resolver configuration before
/// the resolver is built (`dns lookup --server`). The hook sees it with the
/// network's override applied (`StackOptions::dns_override`), and has the last
/// word.
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
    opts.dns_override.apply(&mut resolver_cfg);
    customize(&mut resolver_cfg);
    let resolver_connectors = match &opts.dns_connector {
        Some(connectors) => connectors.clone(),
        None => DnsConnectors::all(connector),
    };
    let (resolver, dns_diags) = Resolver::new(
        resolver_cfg,
        ResolverDeps {
            connectors: resolver_connectors,
            sets: registry.clone(),
            system: opts.system.clone(),
            resources: resources.clone(),
        },
    );
    diagnostics.extend(dns_diags);
    if !opts.no_network && !opts.wait.is_zero() {
        resources.wait_initial(opts.wait).await;
        settle(
            &registry,
            &geo,
            &resources,
            SETTLE,
            opts.wait,
            &mut diagnostics,
        )
        .await;
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
/// all the same, how many sets are left behind is a warning in `diagnostics`:
/// whoever waits here prints those, and may have no log to print.
async fn settle(
    registry: &SetRegistry,
    geo: &GeoDb,
    resources: &ResourceManager,
    unsettled: Duration,
    queued: Duration,
    diagnostics: &mut Diagnostics,
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
                diagnostics.push(Diagnostic::warning(
                    codes::W_RESOURCE_UNAVAILABLE,
                    format!(
                        "{behind} set(s) that arrived were still waiting to be compiled when the wait ended: until they are, they match as they did before, and a set that is new matches nothing"
                    ),
                ));
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
    use rurge_config::{NetKind, Primary, WifiLink};
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
        // a set that compiled a version between the look at the manager and
        // the look at the registry is ahead of what was seen, not behind
        assert_eq!(sets_behind(&[set(Some("b"), 2)], &held), 0);
        assert_eq!(sets_behind(&[], &held), 0);
        assert_eq!(sets_behind(&late, &[]), 0);
    }

    /// The time the tests give `settle` for sets that are behind: what `--wait`
    /// is when nobody gives it.
    const QUEUED: Duration = Duration::from_secs(30);

    /// Two looks of `settle` at what it waits for.
    const SLACK: Duration = Duration::from_millis(100);

    /// The registry, the database and the manager of a profile, made where no
    /// runtime is: no task fetches a resource and no task follows a set, so
    /// each stays as it was made, or as a test makes it.
    struct Still {
        registry: Arc<SetRegistry>,
        geo: Arc<GeoDb>,
        resources: Arc<ResourceManager>,
    }

    impl Still {
        /// Of a profile with these lines in `[Rule]`, its files in `dir`.
        fn of(dir: &Path, rules: &str) -> Self {
            assert!(
                tokio::runtime::Handle::try_current().is_err(),
                "made inside a runtime: its tasks would run"
            );
            let loaded = from_text(
                &format!("[Proxy]\nP = direct\n[Rule]\n{rules}\nFINAL,DIRECT\n"),
                &dir.join("t.conf"),
                &LoadOptions::for_tests(),
            );
            assert!(!loaded.diagnostics.has_errors());
            let resources = ResourceManager::with_options(
                dir.to_path_buf(),
                resource_client().unwrap(),
                ResourceOptions::default(),
            );
            let (registry, _) = SetRegistry::build(&loaded.config, resources.clone(), dir);
            let (geo, _) = GeoDb::open(&dir.join("geoip"));
            Still {
                registry,
                geo,
                resources,
            }
        }

        /// The set of the profile whose name ends like this.
        fn set(&self, ends: &str) -> SetStatus {
            self.registry
                .statuses()
                .into_iter()
                .find(|set| set.name.ends_with(ends))
                .expect("the set of the profile")
        }

        /// Runs `settle` with these two times: how long it waited, and what
        /// it said. The clock is the runtime's own: it stands still unless
        /// something sleeps, and the runtime then moves it to the next timer.
        /// So the time is exact to one look of `settle`, and none of it is
        /// the test's.
        fn settle(
            &self,
            unsettled: Duration,
            queued: Duration,
        ) -> (Duration, Vec<(&'static str, String)>) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .start_paused(true)
                .build()
                .unwrap();
            runtime.block_on(async {
                let mut diagnostics = Diagnostics::default();
                let began = tokio::time::Instant::now();
                tokio::time::timeout(
                    Duration::from_secs(600),
                    settle(
                        &self.registry,
                        &self.geo,
                        &self.resources,
                        unsettled,
                        queued,
                        &mut diagnostics,
                    ),
                )
                .await
                .expect("settle went on past every time it was given");
                let said = diagnostics
                    .iter()
                    .map(|d| (d.code, d.message.clone()))
                    .collect();
                (began.elapsed(), said)
            })
        }
    }

    /// With every set level with its resource and nothing that may still
    /// come, `settle` does not wait at all, whatever times it is given: a
    /// command that waits answers as soon as what arrived is applied.
    #[test]
    fn with_nothing_to_wait_for_it_does_not_wait() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("level.list"), "DOMAIN,a.example\n").unwrap();
        let still = Still::of(dir.path(), "RULE-SET,level.list,P");
        // the set has compiled the one version there is of its file
        let set = still.set("level.list");
        let file = still
            .resources
            .statuses()
            .into_iter()
            .find(|file| Some(&file.key) == set.key.as_ref())
            .expect("the file of the set");
        assert_eq!((set.entries, set.version, file.version), (1, 1, 1));

        assert_eq!(still.settle(QUEUED, QUEUED), (Duration::ZERO, vec![]));
    }

    /// A set whose resource has not arrived is waited for as what may not
    /// come at all: the short time, however long the other is. Nothing is
    /// said of it here: the registry said it when it was built.
    #[test]
    fn a_set_that_has_not_arrived_is_given_the_short_time_only() {
        let dir = tempfile::tempdir().unwrap();
        // nothing is ever asked of this address
        let still = Still::of(dir.path(), "RULE-SET,http://127.0.0.1:1/absent.list,P");
        assert_eq!(still.set("absent.list").state, "missing");
        assert_eq!(
            sets_behind(&still.registry.statuses(), &still.resources.statuses()),
            0
        );

        let (waited, said) = still.settle(SETTLE, QUEUED);
        assert!(SETTLE <= waited && waited < SETTLE + SLACK, "{waited:?}");
        assert!(said.is_empty(), "{said:?}");
        // and no time at all when the short one is none
        assert_eq!(
            still.settle(Duration::ZERO, QUEUED),
            (Duration::ZERO, vec![])
        );
    }

    /// Sets that stay behind: `settle` gives them the time it was told to
    /// give them and no more, and then says how many there are, in the
    /// diagnostics. The content is handed to the manager, and the set never
    /// compiles it: nothing follows it (`Still`).
    #[test]
    fn sets_that_stay_behind_are_said_when_their_time_is_over() {
        let dir = tempfile::tempdir().unwrap();
        // nothing is ever asked of this address
        let url = "http://127.0.0.1:1/behind.list";
        let still = Still::of(dir.path(), &format!("RULE-SET,{url},P"));
        still
            .resources
            .adopt(
                &ResourceSource::Url(url.parse().unwrap()),
                "DOMAIN,a.example\n".into(),
            )
            .unwrap();
        let behind = vec![(
            codes::W_RESOURCE_UNAVAILABLE,
            "1 set(s) that arrived were still waiting to be compiled when the wait ended: until they are, they match as they did before, and a set that is new matches nothing".to_string(),
        )];
        let no_time = Duration::ZERO;

        let (waited, said) = still.settle(no_time, QUEUED);
        assert!(QUEUED <= waited && waited < QUEUED + SLACK, "{waited:?}");
        assert_eq!(said, behind);
        // the short time is not added to theirs
        let (waited, said) = still.settle(SETTLE, QUEUED);
        assert!(QUEUED <= waited && waited < QUEUED + SLACK, "{waited:?}");
        assert_eq!(said, behind);
        // a time for them below the short one (`--wait 1`): this set has not
        // loaded at all, so it is also waited for as what may not come
        let (waited, said) = still.settle(SETTLE, Duration::from_secs(1));
        assert!(SETTLE <= waited && waited < SETTLE + SLACK, "{waited:?}");
        assert_eq!(said, behind);
        // with no time of either kind: at once, and said all the same
        assert_eq!(still.settle(no_time, no_time), (Duration::ZERO, behind));
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
        let mut diagnostics = Diagnostics::default();
        settle(
            &registry,
            &geo,
            &resources,
            no_time,
            Duration::from_secs(60),
            &mut diagnostics,
        )
        .await;
        // nothing was left behind, so nothing is said
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let key = resource_key(&ResourceSource::Url(url));
        let set = registry
            .statuses()
            .into_iter()
            .find(|set| set.key.as_deref() == Some(key.as_str()))
            .expect("the set of the profile");
        assert_eq!((set.entries, set.state.as_str()), (LINES, "ok"));
    }

    fn on_wifi(ssid: &str) -> NetworkFacts {
        NetworkFacts {
            primary: Some(Primary {
                interface: "wlan0".to_string(),
                kind: NetKind::Wifi,
                routers: Vec::new(),
            }),
            wifi: Some(WifiLink {
                interface: "wlan0".to_string(),
                ssid: Some(ssid.to_string()),
                bssid: None,
            }),
        }
    }

    fn on_cable() -> NetworkFacts {
        NetworkFacts {
            primary: Some(Primary {
                interface: "eth0".to_string(),
                kind: NetKind::Wired,
                routers: Vec::new(),
            }),
            wifi: None,
        }
    }

    /// A profile with DNS of its own and three lines of `[SSID Setting]`: one
    /// lists servers, one turns encrypted DNS off, one says nothing of DNS.
    fn with_ssid_settings() -> Config {
        let loaded = from_text(
            "[General]\ndns-server = 10.0.0.1\n\
             encrypted-dns-server = https://global.example/dns-query\n\
             [Rule]\nFINAL,DIRECT\n\
             [SSID Setting]\nSSID:Home dns-server = 192.168.1.1\n\
             TYPE:WIRED encrypted-dns-server = off\nTYPE:WIFI suspend = true\n",
            Path::new("t.conf"),
            &LoadOptions::for_tests(),
        );
        assert!(!loaded.diagnostics.has_errors());
        loaded.config
    }

    /// What the network sets is what the first line it matches says of DNS:
    /// the lists it gives, and nothing of the rest.
    #[test]
    fn the_override_of_a_network_comes_from_the_first_line_it_matches() {
        let cfg = with_ssid_settings();
        assert_eq!(
            DnsOverride::for_network(&cfg, &on_wifi("Home")),
            DnsOverride {
                servers: Some(vec![DnsServer::Udp("192.168.1.1:53".parse().unwrap())]),
                encrypted: None,
            }
        );
        // `off` is a list, an empty one
        assert_eq!(
            DnsOverride::for_network(&cfg, &on_cable()),
            DnsOverride {
                servers: None,
                encrypted: Some(Vec::new()),
            }
        );
        // a line that says nothing of DNS, and no line at all
        assert_eq!(
            DnsOverride::for_network(&cfg, &on_wifi("Cafe")),
            DnsOverride::default()
        );
        assert_eq!(
            DnsOverride::for_network(&cfg, &NetworkFacts::default()),
            DnsOverride::default()
        );
    }

    /// A list the override gives takes the place of the profile's whole list;
    /// what it does not give stays.
    #[test]
    fn an_override_replaces_the_lists_it_gives_and_leaves_the_rest() {
        let cfg = with_ssid_settings();
        let own = ResolverConfig::from_config(&cfg);
        assert_eq!(
            own.servers,
            [DnsServer::Udp("10.0.0.1:53".parse().unwrap())]
        );
        assert_eq!(own.encrypted.len(), 1);

        let mut at_home = own.clone();
        DnsOverride::for_network(&cfg, &on_wifi("Home")).apply(&mut at_home);
        assert_eq!(
            at_home.servers,
            [DnsServer::Udp("192.168.1.1:53".parse().unwrap())]
        );
        assert_eq!(at_home.encrypted, own.encrypted);

        let mut at_the_desk = own.clone();
        DnsOverride::for_network(&cfg, &on_cable()).apply(&mut at_the_desk);
        assert_eq!(at_the_desk.servers, own.servers);
        assert!(at_the_desk.encrypted.is_empty());

        let mut elsewhere = own.clone();
        DnsOverride::default().apply(&mut elsewhere);
        assert_eq!(elsewhere.servers, own.servers);
        assert_eq!(elsewhere.encrypted, own.encrypted);
    }

    /// The URL of an encrypted DNS server can carry a user name and a
    /// password; an override shows neither.
    #[test]
    fn the_debug_of_an_override_leaves_credentials_out() {
        let over = DnsOverride {
            servers: None,
            encrypted: Some(vec![
                EncryptedDns::parse("https://alice:hunter2@dns.example/dns-query").unwrap(),
            ]),
        };
        let shown = format!("{over:?}\n{over:#?}");
        assert!(shown.contains("dns.example"), "{shown}");
        assert!(
            !shown.contains("alice") && !shown.contains("hunter2"),
            "{shown}"
        );
    }
}
