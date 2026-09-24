//! What outlives a config generation (M1 design 6.2, 6.3; M2 design 7.2).

use arc_swap::ArcSwapOption;
use rurge_net::BoxFuture;
use rurge_net::connector::Resolve;
use rurge_policy::{EmptyGroup, GroupSelections, RegistryCell, SelectionTable};
use rustls::RootCertStore;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;

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
}

impl EngineShared {
    pub fn new(initial: GroupSelections) -> EngineShared {
        EngineShared {
            cell: RegistryCell::new(),
            selections: Arc::new(SelectionTable::new(initial)),
            resolver: ResolverCell::new(),
            roots: None,
            empty_group: EmptyGroup::Direct,
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
