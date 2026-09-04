//! Surge-compatible DNS client (M2 design §7): upstream transports, the
//! concurrent query engine, cache, `[Host]` mapping chain and the resolver
//! that every other crate resolves through.

pub mod fanout;
pub mod message;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod upstream;

pub use upstream::{Upstream, UpstreamError, UpstreamRef, UpstreamSpec};
