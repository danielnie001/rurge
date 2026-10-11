//! On-disk cache: `<root>/resources/<sha256(url) hex>/{data,meta.json}`.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Meta {
    pub url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Unix seconds of the last successful fetch or 304.
    pub fetched_at: u64,
    /// The nanoseconds that go with `fetched_at`, so that a check can be told
    /// from a file written in the same second. A cache from before it was
    /// added has none: 0.
    #[serde(default)]
    pub fetched_subsec_nanos: u32,
}

impl Meta {
    /// When the last successful check was, to the nanosecond.
    pub(crate) fn fetched_time(&self) -> SystemTime {
        // A file that says more than a second of nanoseconds is damaged;
        // that is no reason to panic in `Duration::new`
        let nanos = self.fetched_subsec_nanos.min(999_999_999);
        UNIX_EPOCH
            .checked_add(Duration::new(self.fetched_at, nanos))
            .unwrap_or(UNIX_EPOCH)
    }

    /// Records a successful check at `at`.
    pub(crate) fn stamp(&mut self, at: SystemTime) {
        let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
        self.fetched_at = since.as_secs();
        self.fetched_subsec_nanos = since.subsec_nanos();
    }
}

/// The URL by where it goes: its path and query often carry a token.
impl std::fmt::Debug for Meta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Meta {
            url,
            etag,
            last_modified,
            fetched_at,
            fetched_subsec_nanos,
        } = self;
        let shown = url::Url::parse(url)
            .map(|url| super::origin(&url))
            .unwrap_or_else(|_| "…".to_string());
        f.debug_struct("Meta")
            .field("url", &shown)
            .field("etag", etag)
            .field("last_modified", last_modified)
            .field("fetched_at", fetched_at)
            .field("fetched_subsec_nanos", fetched_subsec_nanos)
            .finish()
    }
}

/// What names a resource's cache directory, and what the status calls its
/// key: the SHA-256 of what identifies the resource, in lower-case hex.
pub fn name_of(identity: &str) -> String {
    format!("{:x}", Sha256::digest(identity.as_bytes()))
}

pub struct CacheDir {
    dir: PathBuf,
}

impl CacheDir {
    pub fn for_url(root: &Path, url: &str) -> CacheDir {
        CacheDir {
            dir: root.join("resources").join(name_of(url)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    pub fn load(&self) -> Option<(Bytes, Meta)> {
        let data = std::fs::read(self.dir.join("data")).ok()?;
        let meta_bytes = std::fs::read(self.dir.join("meta.json")).ok()?;
        let meta: Meta = serde_json::from_slice(&meta_bytes).ok()?;
        Some((Bytes::from(data), meta))
    }

    pub fn store(&self, data: &[u8], meta: &Meta) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        write_atomic(&self.dir.join("data"), data)?;
        self.store_meta(meta)
    }

    pub fn store_meta(&self, meta: &Meta) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_vec_pretty(meta).map_err(io::Error::other)?;
        write_atomic(&self.dir.join("meta.json"), &json)
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_data_and_meta() {
        let root = tempfile::tempdir().unwrap();
        let c = CacheDir::for_url(root.path(), "https://example.com/a.list");
        assert!(c.load().is_none());
        let meta = Meta {
            url: "https://example.com/a.list".into(),
            etag: Some("\"x\"".into()),
            last_modified: None,
            fetched_at: 42,
            fetched_subsec_nanos: 123_456_789,
        };
        c.store(b"hello", &meta).unwrap();
        let (data, m) = c.load().unwrap();
        assert_eq!(&data[..], b"hello");
        assert_eq!(m, meta);
        assert!(c.path().starts_with(root.path().join("resources")));
        assert_eq!(c.path().file_name().unwrap().len(), 64);
        assert_eq!(
            c.path().file_name().unwrap(),
            name_of("https://example.com/a.list").as_str()
        );
    }

    /// A cache written before the nanoseconds were kept has no such field.
    #[test]
    fn a_meta_from_before_the_nanoseconds_reads_as_whole_seconds() {
        let old = r#"{"url":"https://example.com/a.list","etag":null,"last_modified":null,"fetched_at":42}"#;
        let meta: Meta = serde_json::from_str(old).unwrap();
        assert_eq!(meta.fetched_subsec_nanos, 0);
        assert_eq!(meta.fetched_time(), UNIX_EPOCH + Duration::from_secs(42));
    }

    #[test]
    fn a_check_is_kept_to_the_nanosecond() {
        let at = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_700);
        let mut meta = Meta::default();
        meta.stamp(at);
        assert_eq!(
            (meta.fetched_at, meta.fetched_subsec_nanos),
            (1_700_000_000, 123_456_700)
        );
        assert_eq!(meta.fetched_time(), at);
        // through the file
        let root = tempfile::tempdir().unwrap();
        let c = CacheDir::for_url(root.path(), "https://example.com/a.list");
        c.store(b"hello", &meta).unwrap();
        assert_eq!(c.load().unwrap().1.fetched_time(), at);
        // a time before 1970 is the epoch
        meta.stamp(UNIX_EPOCH - Duration::from_secs(5));
        assert_eq!(meta.fetched_time(), UNIX_EPOCH);
    }

    /// A damaged file is no reason to panic.
    #[test]
    fn a_time_that_does_not_make_sense_does_not_panic() {
        let mut meta = Meta {
            fetched_at: 5,
            fetched_subsec_nanos: u32::MAX,
            ..Meta::default()
        };
        assert_eq!(
            meta.fetched_time(),
            UNIX_EPOCH + Duration::new(5, 999_999_999)
        );
        meta.fetched_at = u64::MAX;
        assert_eq!(meta.fetched_time(), UNIX_EPOCH);
    }
}
