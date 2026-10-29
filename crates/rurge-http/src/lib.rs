//! The HTTP engine of phase 4. M1a brings the prefix stream (`prefix`), which
//! keeps the bytes read to look at them and gives them back to whoever reads
//! the connection next, and sniffing (`sniff`, `tls`), which tells TLS from
//! plain HTTP from everything else. This crate knows nothing of the engine,
//! the listeners or the sessions: it is handed a stream and gives it back.

pub mod prefix;
pub mod sniff;
pub mod tls;

pub use prefix::Prefixed;
pub use sniff::{Judged, MAX_PREFIX, SNIFF_WAIT, Sniffed, judge, sniff};
pub use tls::ClientHello;
