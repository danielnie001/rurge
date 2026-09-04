//! Network plumbing shared by every rurge crate: the `Connector` abstraction,
//! the internal HTTP client and the external resource manager (M2 design §5).

use std::future::Future;
use std::pin::Pin;

/// Boxed `Send` future used by object-safe async traits.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
