//! How the registry gets real outbounds without knowing how they are made
//! (M1 design 6.1). `rurge-engine` implements this over the resolver, the
//! socket hook, the keystore and the root certificates; tests use a fake.

use rurge_config::spec::{CommonOpts, PolicySpec};
use rurge_net::connector::Connector;
use rurge_proto::OutboundRef;
use std::sync::Arc;

pub use rurge_proto::BuildError;

pub trait OutboundFactory: Send + Sync {
    /// What a policy without `underlying-proxy` dials through: a direct
    /// connector carrying that policy's own socket options.
    fn direct_connector(&self, common: &CommonOpts) -> Arc<dyn Connector>;

    /// The outbound of `spec`, reaching its server through `connector`.
    /// Synchronous and offline: whatever can fail without the network
    /// (a broken p12, an unusable name) fails here, not at dial time.
    fn build(
        &self,
        spec: &PolicySpec,
        connector: Arc<dyn Connector>,
    ) -> Result<OutboundRef, BuildError>;

    /// Whatever the factory captures **by value** that may differ between
    /// config generations: an outbound built under another environment is
    /// never reused. What is read through a cell at dial time (the resolver,
    /// the registry) does not belong here.
    fn environment(&self) -> String;
}
