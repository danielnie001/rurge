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

pub use engine::{
    Decision, LazyProcess, LazyResolver, NoProcess, OutboundMode, Outcome, Reason, RuleEngine,
};
pub use geoip::{DbKind, GeoAnswer, GeoDb, GeoDbInfo};
pub use geoip_update::{GeoUpdater, GeoUrls};
pub use matcher::{GeoLookup, GeoResult};
pub use pre_matching::PreMatch;
pub use registry::{SetRegistry, SetStatus};
