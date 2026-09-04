//! On-disk cache: `<root>/resources/<sha256(url) hex>/{data,meta.json}`.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Meta {
    pub url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Unix seconds of the last successful fetch or 304.
    pub fetched_at: u64,
}

pub struct CacheDir {
    dir: PathBuf,
}

impl CacheDir {
    pub fn for_url(root: &Path, url: &str) -> CacheDir {
        CacheDir {
            dir: root
                .join("resources")
                .join(format!("{:x}", Sha256::digest(url.as_bytes()))),
        }
    }

    // Only exercised by tests today; kept `pub` as a natural accessor for callers in later tasks.
    #[allow(dead_code)]
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
        };
        c.store(b"hello", &meta).unwrap();
        let (data, m) = c.load().unwrap();
        assert_eq!(&data[..], b"hello");
        assert_eq!(m, meta);
        assert!(c.path().starts_with(root.path().join("resources")));
        assert_eq!(c.path().file_name().unwrap().len(), 64);
    }
}
