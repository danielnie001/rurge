//! What outlives a config generation (M1 design 6.2, 6.3; M2 design 7.2).

use arc_swap::ArcSwapOption;
use rurge_net::BoxFuture;
use rurge_net::connector::Resolve;
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

    /// Stops every program still running, all at once; returns when they
    /// are gone.
    pub async fn stop_all(&self) {
        let live: Vec<Arc<ExternalOutbound>> = self
            .list
            .lock()
            .expect("external program list")
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
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
    use rurge_net::BoxFuture;
    use rurge_net::connector::Resolve;
    use std::io;
    use std::net::IpAddr;

    struct Fixed(IpAddr);

    impl Resolve for Fixed {
        fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(std::future::ready(Ok(vec![self.0])))
        }
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
}
