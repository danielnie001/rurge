//! What outlives a config generation (M1 design 6.2, 6.3; M2 design 7.2).

use crate::owner::OwnerLookup;
use arc_swap::ArcSwapOption;
use rurge_config::{NetworkCell, NetworkFacts};
use rurge_dns::system::SystemDns;
use rurge_net::BoxFuture;
use rurge_net::connector::{Resolve, Via};
use rurge_policy::auto::AutoGroups;
use rurge_policy::testbook::TestBook;
use rurge_policy::{EmptyGroup, GroupSelections, RegistryCell, SelectionTable};
use rurge_proto::external::{ExternalOutbound, LocalPorts, NoProcessGroups, ProcessHook};
use rustls::RootCertStore;
use std::io;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, Weak};

/// Where an outbound's connectors find the current generation's resolver.
/// Every reload builds a new resolver (`[Host]`, the upstreams and the cache
/// policy may all have changed), while an outbound that a reload left alone
/// keeps the connectors it was built with: they hold this cell, not any one
/// generation's resolver. Switched together with the registry
/// (`Engine::publish_generation`).
#[derive(Default)]
pub struct ResolverCell(ArcSwapOption<Arc<dyn Resolve>>);

impl ResolverCell {
    pub fn new() -> Arc<ResolverCell> {
        Arc::new(ResolverCell::default())
    }

    pub fn store(&self, resolver: Arc<dyn Resolve>) {
        self.0.store(Some(Arc::new(resolver)));
    }

    /// Whether a generation has published its resolver yet.
    pub fn active(&self) -> bool {
        self.0.load().is_some()
    }
}

/// How rurge looks up the names it dials itself — downloads, the profile's
/// sources: through `library` (the system's), except while the enhanced
/// mode is on, when the library may answer with fake addresses and the
/// current generation's resolver is asked instead (M4 design 7.6). The
/// generation's resolver is not asked before one is published.
pub struct OwnResolve {
    system: Arc<dyn SystemDns>,
    cell: Arc<ResolverCell>,
    library: Arc<dyn Resolve>,
}

impl OwnResolve {
    pub fn new(
        system: Arc<dyn SystemDns>,
        cell: Arc<ResolverCell>,
        library: Arc<dyn Resolve>,
    ) -> OwnResolve {
        OwnResolve {
            system,
            cell,
            library,
        }
    }
}

impl Resolve for OwnResolve {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            if self.system.enhanced_mode() && self.cell.active() {
                self.cell.resolve(host).await
            } else {
                self.library.resolve(host).await
            }
        })
    }
}

impl Resolve for ResolverCell {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            // nothing dials before the first generation is published
            let Some(current) = self.0.load_full() else {
                return Err(io::Error::other("no resolver is active"));
            };
            current.resolve(host).await
        })
    }

    fn resolve_via<'a>(
        &'a self,
        host: &'a str,
        via: &'a Via,
    ) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            let Some(current) = self.0.load_full() else {
                return Err(io::Error::other("no resolver is active"));
            };
            current.resolve_via(host, via).await
        })
    }
}

/// Every `external` outbound built for the engine that still exists, in
/// whatever generation: the exit flow stops their programs (phase 2 M4
/// design 8.2). Their local ports are kept track of across generations too:
/// a rebuilt policy's program takes its port from the older one.
#[derive(Default)]
pub struct ExternalPrograms {
    list: Mutex<Vec<Weak<ExternalOutbound>>>,
    ports: Arc<LocalPorts>,
}

impl ExternalPrograms {
    /// What every `external` outbound of the engine shares its ports through.
    pub(crate) fn local_ports(&self) -> Arc<LocalPorts> {
        self.ports.clone()
    }

    pub(crate) fn add(&self, outbound: &Arc<ExternalOutbound>) {
        let mut list = self.list.lock().expect("external program list");
        list.retain(|o| o.strong_count() > 0);
        list.push(Arc::downgrade(outbound));
    }

    /// The outbounds of every generation that still exist.
    fn live(&self) -> Vec<Arc<ExternalOutbound>> {
        self.list
            .lock()
            .expect("external program list")
            .iter()
            .filter_map(Weak::upgrade)
            .collect()
    }

    /// Whether a program of any of them runs now: only then is a session's
    /// process waited for (M4 design 11, M4b plan P16).
    pub fn any_running(&self) -> bool {
        self.live().iter().any(|o| o.running())
    }

    /// Whether process `pid` is a running program or one it started that is
    /// still in its group: its sessions are DIRECT (M4 design 11).
    pub fn runs(&self, pid: u32) -> bool {
        self.live().iter().any(|o| o.runs(pid))
    }

    /// Stops every program still running, all at once; returns when they
    /// are gone.
    pub async fn stop_all(&self) {
        let live = self.live();
        let stops: Vec<_> = live
            .iter()
            .map(|o| {
                tokio::spawn({
                    let o = o.clone();
                    async move { o.stop().await }
                })
            })
            .collect();
        for stop in stops {
            let _ = stop.await;
        }
    }
}

/// Created once per engine — before the first `Runtime::build`, because the
/// registry built there already needs them — and handed to every later
/// `Runtime::build` of the same engine (`Engine::shared`).
#[derive(Clone)]
pub struct EngineShared {
    /// Where chain connectors find the current generation's registry.
    pub cell: Arc<RegistryCell>,
    /// The live `select` choices of the running profile.
    pub selections: Arc<SelectionTable>,
    /// Where direct connectors find the current generation's resolver.
    pub resolver: Arc<ResolverCell>,
    /// The trust anchors of every outbound's TLS. `None`: the operating
    /// system's. They belong here because they must not change while the
    /// engine lives: a reload reuses outbounds by a fingerprint the roots are
    /// not part of. Tests bring their own CA this way.
    pub roots: Option<Arc<RootCertStore>>,
    /// What a group without members resolves to (M3-D3):
    /// `--empty-group-reject` sets it once, for the engine's lifetime.
    pub empty_group: EmptyGroup,
    /// The automatic groups' test results and state (phase 2 M3 design 6.2,
    /// 6.5): kept across generations, as the selections are.
    pub auto: Arc<AutoGroups>,
    /// How `external` programs are started and stopped: the bin injects
    /// `rurge-platform::process`; everything else uses `NoProcessGroups`.
    pub processes: Arc<dyn ProcessHook>,
    /// The `external` outbounds built so far.
    pub externals: Arc<ExternalPrograms>,
    /// The network the machine is on now (phase 3 M1 design 2), for the
    /// `SUBNET` rules and `subnet` groups of every generation, and for the DNS
    /// that `[SSID Setting]` sets when one is built. Unknown until somebody
    /// stores what they read off the machine; once there is an engine,
    /// `Engine::network_changed` does, and tells the generation in use.
    pub network: Arc<NetworkCell>,
    /// What finds the process behind a session that comes in through a proxy
    /// port (phase 3 M2 design 4). `None` looks nothing up: the default, and
    /// what every test that is not about processes uses; `rurge run` puts
    /// the platform's here.
    pub owners: Option<Arc<dyn OwnerLookup>>,
}

impl EngineShared {
    pub fn new(initial: GroupSelections) -> EngineShared {
        EngineShared {
            cell: RegistryCell::new(),
            selections: Arc::new(SelectionTable::new(initial)),
            resolver: ResolverCell::new(),
            roots: None,
            empty_group: EmptyGroup::Direct,
            auto: Arc::new(AutoGroups::new(Arc::new(TestBook::new()))),
            processes: Arc::new(NoProcessGroups),
            externals: Arc::new(ExternalPrograms::default()),
            network: Arc::new(NetworkCell::new(NetworkFacts::default())),
            owners: None,
        }
    }
}

impl Default for EngineShared {
    fn default() -> EngineShared {
        EngineShared::new(GroupSelections::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_dns::system::StaticSystemDns;
    use rurge_net::BoxFuture;
    use rurge_net::connector::Resolve;
    use std::io;
    use std::net::IpAddr;
    use std::sync::atomic::Ordering;

    struct Fixed(IpAddr);

    impl Resolve for Fixed {
        fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(std::future::ready(Ok(vec![self.0])))
        }
    }

    /// A resolver that notes every name it is asked, and answers one address.
    struct Recording {
        asked: Arc<Mutex<Vec<String>>>,
        answer: IpAddr,
    }

    impl Recording {
        fn new(answer: &str) -> (Recording, Arc<Mutex<Vec<String>>>) {
            let asked = Arc::new(Mutex::new(Vec::new()));
            (
                Recording {
                    asked: asked.clone(),
                    answer: answer.parse().expect("an address"),
                },
                asked,
            )
        }
    }

    impl Resolve for Recording {
        fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(async move {
                self.asked.lock().expect("asked").push(host.to_string());
                Ok(vec![self.answer])
            })
        }
    }

    /// Outside the enhanced mode the system library is what rurge's own names
    /// go to, even with a generation published.
    #[tokio::test]
    async fn own_resolve_asks_the_library_outside_the_enhanced_mode() {
        let cell = ResolverCell::new();
        let (generation, generation_asked) = Recording::new("192.0.2.1");
        cell.store(Arc::new(generation));
        let (library, library_asked) = Recording::new("192.0.2.9");
        let own = OwnResolve::new(
            Arc::new(StaticSystemDns::default()),
            cell,
            Arc::new(library),
        );
        assert_eq!(
            own.resolve("a.test").await.unwrap(),
            ["192.0.2.9".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(*library_asked.lock().unwrap(), ["a.test"]);
        assert!(generation_asked.lock().unwrap().is_empty());
    }

    /// While the enhanced mode is on the library may answer with a fake
    /// address: the generation's resolver is asked instead.
    #[tokio::test]
    async fn own_resolve_asks_the_generation_while_the_enhanced_mode_is_on() {
        let system = StaticSystemDns::default();
        system.enhanced.store(true, Ordering::Relaxed);
        let cell = ResolverCell::new();
        let (generation, generation_asked) = Recording::new("192.0.2.1");
        cell.store(Arc::new(generation));
        let (library, library_asked) = Recording::new("192.0.2.9");
        let own = OwnResolve::new(Arc::new(system), cell, Arc::new(library));
        assert_eq!(
            own.resolve("a.test").await.unwrap(),
            ["192.0.2.1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(*generation_asked.lock().unwrap(), ["a.test"]);
        assert!(library_asked.lock().unwrap().is_empty());
    }

    /// Before the first generation is published there is no resolver to ask:
    /// the library answers, even when the enhanced mode is said to be on.
    #[tokio::test]
    async fn own_resolve_asks_the_library_before_any_generation_is_published() {
        let system = StaticSystemDns::default();
        system.enhanced.store(true, Ordering::Relaxed);
        let (library, library_asked) = Recording::new("192.0.2.9");
        let own = OwnResolve::new(Arc::new(system), ResolverCell::new(), Arc::new(library));
        assert_eq!(
            own.resolve("a.test").await.unwrap(),
            ["192.0.2.9".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(*library_asked.lock().unwrap(), ["a.test"]);
    }

    #[tokio::test]
    async fn the_cell_answers_with_whatever_generation_is_published() {
        let cell = ResolverCell::new();
        let err = cell.resolve("a.test").await.unwrap_err();
        assert_eq!(err.to_string(), "no resolver is active");
        cell.store(Arc::new(Fixed("192.0.2.1".parse().unwrap())));
        assert_eq!(
            cell.resolve("a.test").await.unwrap(),
            ["192.0.2.1".parse::<IpAddr>().unwrap()]
        );
        // the next generation: the same cell, another resolver behind it
        cell.store(Arc::new(Fixed("192.0.2.2".parse().unwrap())));
        assert_eq!(
            cell.resolve("a.test").await.unwrap(),
            ["192.0.2.2".parse::<IpAddr>().unwrap()]
        );
    }

    /// Answers lookups through a `Via` apart from the others.
    struct Marked;

    impl Resolve for Marked {
        fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(std::future::ready(Ok(vec!["192.0.2.1".parse().unwrap()])))
        }

        fn resolve_via<'a>(
            &'a self,
            _host: &'a str,
            via: &'a Via,
        ) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            let ip = if via.key == "en1" {
                "192.0.2.3"
            } else {
                "192.0.2.4"
            };
            Box::pin(std::future::ready(Ok(vec![ip.parse().unwrap()])))
        }
    }

    /// A lookup through a `Via` reaches the current resolver's own
    /// `resolve_via`, with the `Via` it was given (`dns-follow-interface`).
    #[tokio::test]
    async fn the_cell_hands_lookups_through_a_via_on() {
        use rurge_net::connector::{DirectConnector, SystemResolve};
        let via = Via {
            key: "en1".to_string(),
            connector: Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        };
        let cell = ResolverCell::new();
        let err = cell.resolve_via("a.test", &via).await.unwrap_err();
        assert_eq!(err.to_string(), "no resolver is active");
        cell.store(Arc::new(Marked));
        assert_eq!(
            cell.resolve_via("a.test", &via).await.unwrap(),
            ["192.0.2.3".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            cell.resolve("a.test").await.unwrap(),
            ["192.0.2.1".parse::<IpAddr>().unwrap()]
        );
    }
}
