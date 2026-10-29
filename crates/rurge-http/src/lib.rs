//! The HTTP engine of phase 4. M1a brings the prefix stream (`prefix`), which
//! keeps the bytes read to look at them and gives them back to whoever reads
//! the connection next, and sniffing (`sniff`, `tls`), which tells TLS from
//! plain HTTP from everything else. M1b brings the upstream connections one
//! client connection keeps between its requests (`pool`) and, still to come,
//! the HTTP service that serves them.
//!
//! The engine stays outside this crate: which policy a request takes and how
//! its outbound connection is made are asked of it through `Upstreams`, so
//! this crate never rules, never dials and never opens a socket of its own.

pub mod pool;
pub mod prefix;
pub mod sniff;
pub mod tls;

pub use pool::{Meter, Metered, Pool, Reusable};
pub use prefix::Prefixed;
pub use sniff::{Judged, MAX_PREFIX, SNIFF_WAIT, Sniffed, judge, sniff};
pub use tls::ClientHello;
