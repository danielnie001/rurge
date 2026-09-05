//! Surge-compatible DNS client (M2 design §7): upstream transports, the
//! concurrent query engine, cache, `[Host]` mapping chain and the resolver
//! that every other crate resolves through.

pub mod bootstrap;
pub mod cache;
pub mod fanout;
pub mod hosts;
pub mod message;
pub mod resolver;
pub mod system;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod upstream;

pub use resolver::{
    DnsError, DnsResult, HostKind, LookupOpts, Resolver, ResolverConfig, ResolverDeps, Source,
    UpstreamDelay,
};
pub use system::{NoSystemDns, StaticSystemDns, SystemDns};
pub use upstream::{Upstream, UpstreamError, UpstreamRef, UpstreamSpec};
