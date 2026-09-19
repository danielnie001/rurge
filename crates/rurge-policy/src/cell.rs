//! The registry as seen by things that outlive a config generation
//! (M1 design 6.2).

use crate::registry::PolicyRegistry;
use arc_swap::ArcSwapOption;
use rurge_config::rule::PolicyRef;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::OutboundError;
use std::io;
use std::sync::Arc;

/// Where the current generation's registry can be found. The engine stores
/// every new generation here and clears the cell when it goes away: the
/// registry owns outbounds, an outbound may own a `ChainConnector`, and that
/// points back here — clearing is what breaks the cycle.
#[derive(Default)]
pub struct RegistryCell(ArcSwapOption<PolicyRegistry>);

impl RegistryCell {
    pub fn new() -> Arc<RegistryCell> {
        Arc::new(RegistryCell::default())
    }

    pub fn store(&self, registry: Arc<PolicyRegistry>) {
        self.0.store(Some(registry));
    }

    pub fn clear(&self) {
        self.0.store(None);
    }

    pub fn load(&self) -> Option<Arc<PolicyRegistry>> {
        self.0.load_full()
    }
}

/// The connector of a policy with `underlying-proxy = <name>`: reaches the
/// policy's own server through whatever `<name>` resolves to *now* — a group
/// follows its current selection, a reload follows the new generation. The
/// server's host name travels as it is, so the underlying proxy resolves it
/// (FR-OUT-08).
pub struct ChainConnector {
    cell: Arc<RegistryCell>,
    name: String,
}

impl ChainConnector {
    pub fn new(cell: Arc<RegistryCell>, name: impl Into<String>) -> ChainConnector {
        ChainConnector {
            cell,
            name: name.into(),
        }
    }

    fn via(&self, e: OutboundError) -> io::Error {
        match e {
            OutboundError::Io(e) => io::Error::new(e.kind(), format!("via {}: {e}", self.name)),
            OutboundError::Timeout => io::Error::new(
                io::ErrorKind::TimedOut,
                format!("via {}: connect timed out", self.name),
            ),
            other => io::Error::other(format!("via {}: {other}", self.name)),
        }
    }
}

impl Connector for ChainConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
            let Some(registry) = self.cell.load() else {
                return Err(io::Error::other(format!(
                    "via {}: no policy registry is active",
                    self.name
                )));
            };
            if !registry.contains(&self.name) {
                // a reload removed the name this policy was built against
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("via {}: the policy no longer exists", self.name),
                ));
            }
            let resolution = registry.resolve(&PolicyRef::Named(self.name.clone()));
            resolution
                .outbound
                .connect_tcp(target, opts)
                .await
                .map_err(|e| self.via(e))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::RecordingConnector;
    use rurge_config::HostName;
    use rurge_config::config::{LoadOptions, from_text};
    use std::path::Path;

    const PROFILE: &str = "[General]\nloglevel = notify\n[Proxy]\nD = direct\nBlock = reject\n[Proxy Group]\nPick = select, D, DIRECT\n[Rule]\nFINAL,DIRECT\n";

    fn registry(connector: Arc<RecordingConnector>) -> Arc<PolicyRegistry> {
        let loaded = from_text(PROFILE, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        let factory = crate::testing::FakeFactory {
            connector,
            broken: None,
        };
        Arc::new(
            PolicyRegistry::build(
                &loaded.config,
                &factory,
                &RegistryCell::new(),
                Arc::new(crate::selections::SelectionTable::default()),
            )
            .expect("builds"),
        )
    }

    fn server() -> Target {
        Target::new(HostName::parse("proxy.example"), 8080)
    }

    #[tokio::test]
    async fn an_empty_cell_is_an_error() {
        let chain = ChainConnector::new(RegistryCell::new(), "D");
        let e = chain
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("nothing to resolve against");
        assert_eq!(e.to_string(), "via D: no policy registry is active");
    }

    #[tokio::test]
    async fn a_name_that_is_gone_is_an_error() {
        let cell = RegistryCell::new();
        cell.store(registry(Arc::new(RecordingConnector::default())));
        let e = ChainConnector::new(cell, "Ghost")
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("the name does not exist");
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(e.to_string(), "via Ghost: the policy no longer exists");
    }

    /// The policy's own server goes to the underlying policy as it is: the
    /// name is never resolved locally (FR-OUT-08).
    #[tokio::test]
    async fn the_target_is_handed_over_unchanged_also_through_a_group() {
        let connector = Arc::new(RecordingConnector::default());
        let cell = RegistryCell::new();
        cell.store(registry(connector.clone()));
        for name in ["D", "Pick"] {
            ChainConnector::new(cell.clone(), name)
                .connect(&server(), &ConnectOpts::default())
                .await
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        assert_eq!(
            connector.seen(),
            ["dial proxy.example:8080", "dial proxy.example:8080"]
        );
    }

    #[tokio::test]
    async fn failures_name_the_hop_and_keep_their_kind() {
        let cell = RegistryCell::new();
        cell.store(registry(Arc::new(RecordingConnector {
            fail: true,
            ..RecordingConnector::default()
        })));
        let e = ChainConnector::new(cell.clone(), "D")
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("the connector refuses");
        assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
        assert_eq!(e.to_string(), "via D: refused by the test");
        // a reject policy underneath is an error too, not a silent hang
        let e = ChainConnector::new(cell, "Block")
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("REJECT cannot carry a connection");
        assert_eq!(e.to_string(), "via Block: rejected by REJECT");
    }

    #[test]
    fn clearing_the_cell_lets_the_registry_go() {
        let cell = RegistryCell::new();
        let reg = registry(Arc::new(RecordingConnector::default()));
        cell.store(reg.clone());
        assert!(cell.load().is_some());
        cell.clear();
        assert!(cell.load().is_none());
        assert_eq!(Arc::strong_count(&reg), 1);
    }
}
