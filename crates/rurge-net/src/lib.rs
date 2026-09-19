//! Network plumbing shared by every rurge crate: the `Connector` abstraction,
//! the internal HTTP client and the external resource manager (M2 design §5).

use std::future::Future;
use std::pin::Pin;

/// Boxed `Send` future used by object-safe async traits.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub mod connector;
pub mod http;
pub mod resource;
pub mod socket;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod tls;
