//! Surge-compatible profile parser.
//!
//! Two layers: the *text layer* (`text::Profile`) keeps every section and line
//! with its origin; the *semantic layer* (`config::Config`) is the typed view
//! consumed by the engine. Parsing never panics; problems are reported as
//! `diagnostic::Diagnostic` values.

pub mod diagnostic;
pub mod glob;
pub mod hostlist;
pub mod requirement;
pub mod span;
pub mod text;
pub mod types;
pub mod value;

pub use diagnostic::{Diagnostic, Diagnostics, Severity, codes};
pub use glob::{Glob, GlobOptions};
pub use hostlist::HostList;
pub use requirement::Environment;
pub use span::Span;
pub use text::include::IncludeOptions;
pub use text::{Entry, Origin, Profile, Section, SectionKind};
pub use types::HostName;
pub use value::ParamMap;

pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
