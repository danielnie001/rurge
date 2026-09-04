//! GeoIP / ASN databases (M2 design §6.4): MaxMind DB readers behind
//! `ArcSwapOption` so an update swaps the file in without rebuilding the engine.

use crate::matcher::GeoLookup;
use arc_swap::ArcSwapOption;
use maxminddb::{Reader, geoip2};
use rurge_config::{Diagnostic, codes};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const COUNTRY_FILE: &str = "GeoLite2-Country.mmdb";
pub const ASN_FILE: &str = "GeoLite2-ASN.mmdb";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbKind {
    Country,
    Asn,
}

impl DbKind {
    pub fn file_name(self) -> &'static str {
        match self {
            DbKind::Country => COUNTRY_FILE,
            DbKind::Asn => ASN_FILE,
        }
    }

    fn type_marker(self) -> &'static str {
        match self {
            DbKind::Country => "Country",
            DbKind::Asn => "ASN",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeoDbInfo {
    pub country_epoch: Option<u64>,
    pub asn_epoch: Option<u64>,
    pub country_path: PathBuf,
    pub asn_path: PathBuf,
}

pub struct GeoDb {
    dir: PathBuf,
    country: ArcSwapOption<Reader<Vec<u8>>>,
    asn: ArcSwapOption<Reader<Vec<u8>>>,
    warned_country: AtomicBool,
    warned_asn: AtomicBool,
}

impl GeoDb {
    /// Opens whatever exists in `dir`; missing files are reported as `I0003`,
    /// unreadable ones as `W0022` (the file is renamed to `*.bad`).
    pub fn open(dir: &Path) -> (Arc<GeoDb>, Vec<Diagnostic>) {
        let db = Arc::new(GeoDb {
            dir: dir.to_path_buf(),
            country: ArcSwapOption::from(None),
            asn: ArcSwapOption::from(None),
            warned_country: AtomicBool::new(false),
            warned_asn: AtomicBool::new(false),
        });
        let mut diags = Vec::new();
        for kind in [DbKind::Country, DbKind::Asn] {
            let path = db.path(kind);
            if !path.exists() {
                diags.push(Diagnostic::info(
                    codes::I_GEOIP_DB_MISSING,
                    format!(
                        "{} not found in {}; GEOIP / IP-ASN rules will not match until it is downloaded",
                        kind.file_name(),
                        dir.display()
                    ),
                ));
                continue;
            }
            if let Err(e) = db.load(kind) {
                let bad = path.with_extension("mmdb.bad");
                let _ = std::fs::rename(&path, &bad);
                diags.push(Diagnostic::warning(
                    codes::W_RESOURCE_UNAVAILABLE,
                    format!("{}: {e}; renamed to {}", path.display(), bad.display()),
                ));
            }
        }
        (db, diags)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, kind: DbKind) -> PathBuf {
        self.dir.join(kind.file_name())
    }

    /// Checks that `bytes` is a MaxMind DB of the expected type; returns `build_epoch`.
    pub fn validate(bytes: &[u8], kind: DbKind) -> Result<u64, String> {
        let reader = Reader::from_source(bytes.to_vec()).map_err(|e| e.to_string())?;
        let meta = reader.metadata();
        if !meta.database_type.contains(kind.type_marker()) {
            return Err(format!(
                "database type `{}` is not a {} database",
                meta.database_type,
                kind.type_marker()
            ));
        }
        Ok(meta.build_epoch)
    }

    /// (Re)opens `dir/<file>` and swaps it in.
    pub fn load(&self, kind: DbKind) -> Result<(), String> {
        let path = self.path(kind);
        let bytes =
            std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::validate(&bytes, kind)?;
        let reader = Reader::from_source(bytes).map_err(|e| e.to_string())?;
        match kind {
            DbKind::Country => self.country.store(Some(Arc::new(reader))),
            DbKind::Asn => self.asn.store(Some(Arc::new(reader))),
        }
        Ok(())
    }

    pub fn info(&self) -> GeoDbInfo {
        GeoDbInfo {
            country_epoch: self
                .country
                .load()
                .as_ref()
                .map(|r| r.metadata().build_epoch),
            asn_epoch: self.asn.load().as_ref().map(|r| r.metadata().build_epoch),
            country_path: self.path(DbKind::Country),
            asn_path: self.path(DbKind::Asn),
        }
    }

    fn warn_missing(&self, kind: DbKind) {
        let flag = match kind {
            DbKind::Country => &self.warned_country,
            DbKind::Asn => &self.warned_asn,
        };
        if !flag.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                file = kind.file_name(),
                "GeoIP database not loaded; rules depending on it do not match"
            );
        }
    }
}

impl GeoLookup for GeoDb {
    fn country(&self, ip: IpAddr) -> Option<[u8; 2]> {
        let guard = self.country.load();
        let Some(reader) = guard.as_ref() else {
            self.warn_missing(DbKind::Country);
            return None;
        };
        let result = reader.lookup(ip).ok()?;
        let record = result.decode::<geoip2::Country>().ok().flatten()?;
        let code = record.country.iso_code?;
        let b = code.as_bytes();
        (b.len() == 2).then(|| [b[0].to_ascii_uppercase(), b[1].to_ascii_uppercase()])
    }

    fn asn(&self, ip: IpAddr) -> Option<u32> {
        let guard = self.asn.load();
        let Some(reader) = guard.as_ref() else {
            self.warn_missing(DbKind::Asn);
            return None;
        };
        let result = reader.lookup(ip).ok()?;
        result
            .decode::<geoip2::Asn>()
            .ok()
            .flatten()?
            .autonomous_system_number
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
    }

    fn dir_with_fixtures() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(
            fixtures().join("GeoIP2-Country-Test.mmdb"),
            dir.path().join(COUNTRY_FILE),
        )
        .unwrap();
        std::fs::copy(
            fixtures().join("GeoLite2-ASN-Test.mmdb"),
            dir.path().join(ASN_FILE),
        )
        .unwrap();
        dir
    }

    #[test]
    fn looks_up_country_and_asn_from_the_test_databases() {
        let dir = dir_with_fixtures();
        let (db, diags) = GeoDb::open(dir.path());
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| d.code).collect::<Vec<_>>()
        );
        assert_eq!(db.country("2001:218::1".parse().unwrap()), Some(*b"JP"));
        assert_eq!(db.country("2001:220::1".parse().unwrap()), Some(*b"KR"));
        assert_eq!(db.country("127.0.0.1".parse().unwrap()), None);
        assert_eq!(db.asn("1.0.0.1".parse().unwrap()), Some(15169));
        assert_eq!(db.asn("1.128.0.1".parse().unwrap()), Some(1221));
        assert_eq!(db.asn("127.0.0.1".parse().unwrap()), None);
        let info = db.info();
        assert!(info.country_epoch.is_some() && info.asn_epoch.is_some());
        assert!(info.country_path.ends_with(COUNTRY_FILE));
    }

    #[test]
    fn missing_files_are_info_and_lookups_return_none() {
        let dir = tempfile::tempdir().unwrap();
        let (db, diags) = GeoDb::open(dir.path());
        let codes: Vec<&str> = diags.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            vec![codes::I_GEOIP_DB_MISSING, codes::I_GEOIP_DB_MISSING]
        );
        assert_eq!(db.country("2001:218::1".parse().unwrap()), None);
        assert_eq!(db.asn("1.0.0.1".parse().unwrap()), None);
        assert_eq!(db.info().country_epoch, None);
    }

    #[test]
    fn corrupt_file_is_renamed_and_warned() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(COUNTRY_FILE), b"not a database").unwrap();
        let (db, diags) = GeoDb::open(dir.path());
        let codes: Vec<&str> = diags.iter().map(|d| d.code).collect();
        assert!(codes.contains(&codes::W_RESOURCE_UNAVAILABLE));
        assert!(!dir.path().join(COUNTRY_FILE).exists());
        assert!(dir.path().join("GeoLite2-Country.mmdb.bad").exists());
        assert_eq!(db.country("2001:218::1".parse().unwrap()), None);
    }

    #[test]
    fn validate_checks_the_database_type() {
        let country = std::fs::read(fixtures().join("GeoIP2-Country-Test.mmdb")).unwrap();
        let asn = std::fs::read(fixtures().join("GeoLite2-ASN-Test.mmdb")).unwrap();
        assert!(GeoDb::validate(&country, DbKind::Country).is_ok());
        assert!(GeoDb::validate(&asn, DbKind::Asn).is_ok());
        assert!(GeoDb::validate(&country, DbKind::Asn).is_err());
        assert!(GeoDb::validate(b"garbage", DbKind::Country).is_err());
    }

    #[test]
    fn load_swaps_a_new_file_in() {
        let dir = tempfile::tempdir().unwrap();
        let (db, _) = GeoDb::open(dir.path());
        assert_eq!(db.asn("1.0.0.1".parse().unwrap()), None);
        std::fs::copy(
            fixtures().join("GeoLite2-ASN-Test.mmdb"),
            dir.path().join(ASN_FILE),
        )
        .unwrap();
        db.load(DbKind::Asn).unwrap();
        assert_eq!(db.asn("1.0.0.1".parse().unwrap()), Some(15169));
    }
}
