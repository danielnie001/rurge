//! Surge-compatible profile parser.
//!
//! Two layers: the *text layer* (`text::Profile`) keeps every section and line
//! with its origin; the *semantic layer* (`config::Config`) is the typed view
//! consumed by the engine. Parsing never panics; problems are reported as
//! `diagnostic::Diagnostic` values.

pub mod diagnostic;
pub mod general;
pub mod glob;
pub mod hostlist;
pub mod policy;
pub mod requirement;
pub mod rule;
pub mod span;
pub mod text;
pub mod types;
pub mod value;

pub use diagnostic::{Diagnostic, Diagnostics, ParseError, Severity, codes};
pub use general::General;
pub use glob::{Glob, GlobOptions};
pub use hostlist::HostList;
pub use policy::{Builtin, GroupKind, PolicyGroup, PolicyKind, ProxyPolicy, SubnetExpr};
pub use requirement::Environment;
pub use rule::{ParseCtx, PolicyRef, ResourceRef, Rule, RuleKind, RuleParams, SubRule};
pub use span::Span;
pub use text::include::IncludeOptions;
pub use text::{Entry, Origin, Profile, Section, SectionKind};
pub use types::HostName;
pub use value::ParamMap;

pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
