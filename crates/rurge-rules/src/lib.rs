//! Rule engine, rule-set indexes and GeoIP / ASN lookups (M2 design §6).

pub mod domain_index;
pub mod geoip;
pub mod ip_index;
pub mod matcher;
pub mod set;
pub mod set_format;

pub use geoip::{DbKind, GeoDb, GeoDbInfo};
