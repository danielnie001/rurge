//! Surge-compatible profile parser.
//!
//! Two layers: the *text layer* (`text::Profile`) keeps every section and line
//! with its origin; the *semantic layer* (`config::Config`) is the typed view
//! consumed by the engine. Parsing never panics; problems are reported as
//! `diagnostic::Diagnostic` values.

pub mod diagnostic;
pub mod span;

pub use diagnostic::{Diagnostic, Diagnostics, Severity, codes};
pub use span::Span;

pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
