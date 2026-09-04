//! Downloads and installs the GeoIP databases through the resource manager
//! (M2 design §6.4). Country DB: `geoip-maxmind-url` or the default mirror;
//! ASN DB: `--geoip-asn-url` or the default mirror. Update period 7 days.

use crate::geoip::{DbKind, GeoDb};
use flate2::read::GzDecoder;
use rurge_net::resource::{
    ResourceHandle, ResourceManager, ResourceSource, ResourceSpec, ResourceState,
};
use std::io::Read;
use std::sync::Arc;
use tar::Archive;
use tokio::task::JoinHandle;
use url::Url;

pub const DEFAULT_COUNTRY_URL: &str =
    "https://github.com/P3TERX/GeoLite.mmdb/releases/latest/download/GeoLite2-Country.mmdb";
pub const DEFAULT_ASN_URL: &str =
    "https://github.com/P3TERX/GeoLite.mmdb/releases/latest/download/GeoLite2-ASN.mmdb";
pub const UPDATE_INTERVAL_SECS: i64 = 7 * 86_400;
/// Bound on a `.tar.gz` member's decompressed size, so a small malicious or
/// corrupt archive cannot exhaust memory (a "decompression bomb").
pub const MAX_MMDB_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct GeoUrls {
    pub country: Url,
    pub asn: Url,
}

impl Default for GeoUrls {
    fn default() -> Self {
        GeoUrls {
            country: Url::parse(DEFAULT_COUNTRY_URL).expect("valid default url"),
            asn: Url::parse(DEFAULT_ASN_URL).expect("valid default url"),
        }
    }
}

pub struct GeoUpdater {
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for GeoUpdater {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl GeoUpdater {
    pub fn spawn(
        geo: Arc<GeoDb>,
        resources: Arc<ResourceManager>,
        urls: GeoUrls,
        auto_update: bool,
    ) -> GeoUpdater {
        let mut tasks = Vec::new();
        for (kind, url) in [(DbKind::Country, urls.country), (DbKind::Asn, urls.asn)] {
            if !auto_update && geo.path(kind).exists() {
                continue;
            }
            let spec = ResourceSpec {
                source: ResourceSource::Url(url),
                update_interval: Some(if auto_update {
                    UPDATE_INTERVAL_SECS
                } else {
                    -1
                }),
            };
            let handle = resources.get(&spec);
            tasks.push(tokio::spawn(install_loop(geo.clone(), kind, handle)));
        }
        GeoUpdater { tasks }
    }
}

async fn install_loop(geo: Arc<GeoDb>, kind: DbKind, handle: ResourceHandle) {
    let mut rx = handle.subscribe();
    let mut installed: Option<u64> = None;
    loop {
        if let ResourceState::Available { data, version, .. } = handle.current()
            && installed != Some(version)
        {
            installed = Some(version);
            match install(&geo, kind, &data) {
                Ok(epoch) => tracing::info!(
                    file = kind.file_name(),
                    build_epoch = epoch,
                    "GeoIP database installed"
                ),
                Err(e) => tracing::warn!(
                    file = kind.file_name(),
                    error = %e,
                    "GeoIP download rejected"
                ),
            }
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

/// Validates `data`, writes `<dir>/<file>` atomically and swaps it into `geo`.
pub fn install(geo: &GeoDb, kind: DbKind, data: &[u8]) -> Result<u64, String> {
    let mmdb = extract_mmdb(data)?;
    let epoch = GeoDb::validate(&mmdb, kind)?;
    let path = geo.path(kind);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("mmdb.tmp");
    std::fs::write(&tmp, &mmdb).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    geo.load(kind)?;
    Ok(epoch)
}

/// A raw `.mmdb`, or the first `.mmdb` member of a `.tar.gz`.
pub fn extract_mmdb(data: &[u8]) -> Result<Vec<u8>, String> {
    extract_mmdb_with_limit(data, MAX_MMDB_BYTES)
}

fn extract_mmdb_with_limit(data: &[u8], limit: u64) -> Result<Vec<u8>, String> {
    if !data.starts_with(&[0x1f, 0x8b]) {
        return Ok(data.to_vec());
    }
    let mut archive = Archive::new(GzDecoder::new(data));
    for entry in archive.entries().map_err(|e| format!("tar: {e}"))? {
        let entry = entry.map_err(|e| format!("tar: {e}"))?;
        let is_mmdb = entry
            .path()
            .map(|p| p.extension().is_some_and(|x| x == "mmdb"))
            .unwrap_or(false);
        if is_mmdb {
            let mut buf = Vec::new();
            // Read at most `limit + 1` bytes: reading exactly `limit` would
            // silently accept a member of precisely that size as "under the
            // limit" while giving no way to tell it apart from a truncated
            // larger one, so read one extra byte to detect the overflow.
            entry
                .take(limit + 1)
                .read_to_end(&mut buf)
                .map_err(|e| format!("tar: {e}"))?;
            if buf.len() as u64 > limit {
                return Err(format!(
                    "decompressed database exceeds {} MiB",
                    limit / (1024 * 1024)
                ));
            }
            return Ok(buf);
        }
    }
    Err("archive contains no .mmdb file".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geoip::{ASN_FILE, COUNTRY_FILE};
    use crate::matcher::GeoLookup;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::http::{HttpClient, HttpClientConfig};
    use rurge_net::resource::ResourceOptions;
    use rurge_net::testing::TestServer;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join(name),
        )
        .unwrap()
    }

    fn tar_gz(name: &str, data: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, data).unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn manager(root: &Path) -> Arc<ResourceManager> {
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let client = Arc::new(HttpClient::new(connector, HttpClientConfig::default()).unwrap());
        ResourceManager::with_options(
            root.to_path_buf(),
            client,
            ResourceOptions {
                fetch_timeout: Duration::from_secs(5),
                min_backoff: Duration::from_millis(50),
                max_backoff: Duration::from_millis(200),
                ..ResourceOptions::default()
            },
        )
    }

    async fn wait_until(what: &str, pred: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !pred() {
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[test]
    fn extract_mmdb_accepts_raw_and_tar_gz() {
        let raw = fixture("GeoLite2-ASN-Test.mmdb");
        assert_eq!(extract_mmdb(&raw).unwrap(), raw);
        let archived = tar_gz("GeoLite2-ASN.mmdb", &raw);
        assert_eq!(extract_mmdb(&archived).unwrap(), raw);
        let no_mmdb = tar_gz("README.txt", b"hello");
        assert!(extract_mmdb(&no_mmdb).is_err());
    }

    #[test]
    fn extract_mmdb_with_limit_rejects_a_decompression_bomb() {
        let huge = vec![b'x'; 1000];
        let archived = tar_gz("GeoLite2-ASN.mmdb", &huge);
        let err = extract_mmdb_with_limit(&archived, 100).unwrap_err();
        assert!(err.contains("exceeds"), "{err}");
        // A member at exactly the limit still extracts in full.
        let at_limit = vec![b'y'; 100];
        let ok_archive = tar_gz("GeoLite2-ASN.mmdb", &at_limit);
        assert_eq!(extract_mmdb_with_limit(&ok_archive, 100).unwrap(), at_limit);
    }

    #[tokio::test]
    async fn installs_raw_and_archived_downloads_and_hot_loads() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/country.mmdb", fixture("GeoIP2-Country-Test.mmdb"));
        server.set(
            "/asn.tar.gz",
            tar_gz("GeoLite2-ASN.mmdb", &fixture("GeoLite2-ASN-Test.mmdb")),
        );
        let geo_dir = root.path().join("geoip");
        let (geo, _) = GeoDb::open(&geo_dir);
        let mgr = manager(root.path());
        let _updater = GeoUpdater::spawn(
            geo.clone(),
            mgr.clone(),
            GeoUrls {
                country: server.url("/country.mmdb"),
                asn: server.url("/asn.tar.gz"),
            },
            true,
        );
        wait_until("both databases installed", || {
            let i = geo.info();
            i.country_epoch.is_some() && i.asn_epoch.is_some()
        })
        .await;
        assert_eq!(geo.country("2001:218::1".parse().unwrap()), Some(*b"JP"));
        assert_eq!(geo.asn("1.0.0.1".parse().unwrap()), Some(15169));
        assert!(geo_dir.join(COUNTRY_FILE).exists() && geo_dir.join(ASN_FILE).exists());
        assert!(!geo_dir.join("GeoLite2-Country.mmdb.tmp").exists());
    }

    #[tokio::test]
    async fn invalid_download_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/bad", "not a database");
        server.set("/asn", fixture("GeoLite2-ASN-Test.mmdb"));
        let geo_dir = root.path().join("geoip");
        let (geo, _) = GeoDb::open(&geo_dir);
        let mgr = manager(root.path());
        let _updater = GeoUpdater::spawn(
            geo.clone(),
            mgr.clone(),
            GeoUrls {
                country: server.url("/bad"),
                asn: server.url("/asn"),
            },
            true,
        );
        wait_until("asn installed", || geo.info().asn_epoch.is_some()).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(geo.info().country_epoch, None);
        assert!(!geo_dir.join(COUNTRY_FILE).exists());
    }

    #[tokio::test]
    async fn disabled_auto_update_only_fetches_missing_files() {
        let root = tempfile::tempdir().unwrap();
        let server = TestServer::spawn().await;
        server.set("/country", fixture("GeoIP2-Country-Test.mmdb"));
        server.set("/asn", fixture("GeoLite2-ASN-Test.mmdb"));
        let geo_dir = root.path().join("geoip");
        std::fs::create_dir_all(&geo_dir).unwrap();
        std::fs::write(
            geo_dir.join(COUNTRY_FILE),
            fixture("GeoIP2-Country-Test.mmdb"),
        )
        .unwrap();
        let (geo, _) = GeoDb::open(&geo_dir);
        let mgr = manager(root.path());
        let _updater = GeoUpdater::spawn(
            geo.clone(),
            mgr.clone(),
            GeoUrls {
                country: server.url("/country"),
                asn: server.url("/asn"),
            },
            false,
        );
        wait_until("asn installed", || geo.info().asn_epoch.is_some()).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(server.hits("/country"), 0);
        assert_eq!(server.hits("/asn"), 1);
    }
}
