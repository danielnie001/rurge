//! Rule engine, rule-set indexes and GeoIP / ASN lookups (M2 design §6).

pub mod domain_index;
pub mod engine;
pub mod geoip;
pub mod geoip_update;
pub mod ip_index;
pub mod matcher;
pub mod pre_matching;
pub mod registry;
pub mod set;
pub mod set_format;

pub use engine::{Decision, LazyResolver, OutboundMode, Outcome, Reason, RuleEngine};
pub use geoip::{DbKind, GeoDb, GeoDbInfo};
pub use geoip_update::{GeoUpdater, GeoUrls};
pub use registry::{SetRegistry, SetStatus};
