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
    /// The SHA-256 of the body the validators and the time belong to, in
    /// lower-case hex. `store` sets it, and `load` gives nothing when the
    /// body on disk is another: validators sent for a body that is not the
    /// one they were given for would be answered with "nothing has changed"
    /// for as long as the server keeps its version. A cache from before it
    /// was added has none, and its body is taken as it is.
    #[serde(default)]
    pub sha256: Option<String>,
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

/// The URL by where it goes: its path and query often carry a token. The
/// digest by whether there is one: it is made from a body that can hold
/// credentials.
impl std::fmt::Debug for Meta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Meta {
            url,
            etag,
            last_modified,
            fetched_at,
            fetched_subsec_nanos,
            sha256,
        } = self;
        f.debug_struct("Meta")
            .field("url", &shown(url))
            .field("etag", etag)
            .field("last_modified", last_modified)
            .field("fetched_at", fetched_at)
            .field("fetched_subsec_nanos", fetched_subsec_nanos)
            .field("sha256", &sha256.as_ref().map(|_| "…"))
            .finish()
    }
}

/// Where a URL goes, for a `Debug` and a log line.
fn shown(url: &str) -> String {
    url::Url::parse(url)
        .map(|url| super::origin(&url))
        .unwrap_or_else(|_| "…".to_string())
}

/// The SHA-256 of a body, in lower-case hex.
fn digest_of(body: &[u8]) -> String {
    format!("{:x}", Sha256::digest(body))
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

    /// The body and its metadata. `None` when one of the two files is
    /// missing, when `meta.json` cannot be read, and when the body is not the
    /// one the metadata was written for: the two files are written one after
    /// the other, a crash can leave either of them half done, and what is
    /// left then is no entry. Whoever finds none asks for the whole body.
    pub fn load(&self) -> Option<(Bytes, Meta)> {
        let data = std::fs::read(self.dir.join("data")).ok()?;
        let meta_bytes = std::fs::read(self.dir.join("meta.json")).ok()?;
        let meta: Meta = serde_json::from_slice(&meta_bytes).ok()?;
        if meta
            .sha256
            .as_ref()
            .is_some_and(|digest| *digest != digest_of(&data))
        {
            tracing::debug!(
                resource = %shown(&meta.url),
                "the cached body is not the one its metadata was written for; the entry is ignored"
            );
            return None;
        }
        Some((Bytes::from(data), meta))
    }

    /// Whether the two files hold, intact, the body with this digest.
    pub fn holds(&self, digest: &str) -> bool {
        self.load()
            .is_some_and(|(_, meta)| meta.sha256.as_deref() == Some(digest))
    }

    /// Writes the body and then the metadata, in which it records the digest
    /// of the body.
    pub fn store(&self, data: &[u8], meta: &mut Meta) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        meta.sha256 = Some(digest_of(data));
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
        let mut meta = Meta {
            url: "https://example.com/a.list".into(),
            etag: Some("\"x\"".into()),
            last_modified: None,
            fetched_at: 42,
            fetched_subsec_nanos: 123_456_789,
            sha256: None,
        };
        c.store(b"hello", &mut meta).unwrap();
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
        c.store(b"hello", &mut meta).unwrap();
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

    /// The digest `store` records is that of the body, and it is on disk.
    #[test]
    fn store_records_the_digest_of_the_body() {
        let root = tempfile::tempdir().unwrap();
        let c = CacheDir::for_url(root.path(), "https://example.com/a.list");
        let mut meta = Meta::default();
        c.store(b"hello", &mut meta).unwrap();
        // the SHA-256 of "hello"
        let digest = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert_eq!(meta.sha256.as_deref(), Some(digest));
        assert_eq!(c.load().unwrap().1.sha256.as_deref(), Some(digest));
        // a body written over it is recorded in its turn: the SHA-256 of nothing
        c.store(b"", &mut meta).unwrap();
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(meta.sha256.as_deref(), Some(empty));
        let (data, on_disk) = c.load().unwrap();
        assert_eq!(
            (&data[..], on_disk.sha256.as_deref()),
            (&b""[..], Some(empty))
        );
    }

    /// What a crash, a second writer or a hand can leave: a body that is not
    /// the one the metadata was written for. That is no entry.
    #[test]
    fn a_body_the_metadata_was_not_written_for_is_no_entry() {
        let root = tempfile::tempdir().unwrap();
        let c = CacheDir::for_url(root.path(), "https://example.com/a.list");
        let mut meta = Meta::default();
        c.store(b"hello, world", &mut meta).unwrap();
        let body = c.path().join("data");
        let damaged: [(&str, &[u8]); 5] = [
            ("cut short", b"hello"),
            ("empty", b""),
            ("the right length and nothing in it", &[0; 12]),
            ("one byte changed", b"hello, worle"),
            ("one byte more", b"hello, world\n"),
        ];
        for (what, bytes) in damaged {
            std::fs::write(&body, bytes).unwrap();
            assert!(c.load().is_none(), "{what}");
        }
        std::fs::write(&body, b"hello, world").unwrap();
        assert!(c.load().is_some());
        // the metadata of another body beside this one
        let mut other = meta.clone();
        other.sha256 = Some(digest_of(b"another"));
        c.store_meta(&other).unwrap();
        assert!(c.load().is_none());
        // and its own again
        c.store_meta(&meta).unwrap();
        assert_eq!(&c.load().unwrap().0[..], b"hello, world");
    }

    /// A cache written before the digest was kept has none: its body is taken
    /// as it is.
    #[test]
    fn a_cache_from_before_the_digest_is_taken_as_it_is() {
        let root = tempfile::tempdir().unwrap();
        let c = CacheDir::for_url(root.path(), "https://example.com/a.list");
        std::fs::create_dir_all(c.path()).unwrap();
        std::fs::write(c.path().join("data"), "old").unwrap();
        std::fs::write(
            c.path().join("meta.json"),
            r#"{"url":"https://example.com/a.list","etag":"\"x\"","last_modified":null,"fetched_at":42,"fetched_subsec_nanos":7}"#,
        )
        .unwrap();
        let (data, meta) = c.load().expect("an entry");
        assert_eq!((&data[..], meta.sha256), (&b"old"[..], None));
        // which is not to hold a body by its digest
        assert!(!c.holds(&digest_of(b"old")));
    }

    #[test]
    fn holds_is_for_the_intact_body_with_that_digest() {
        let root = tempfile::tempdir().unwrap();
        let c = CacheDir::for_url(root.path(), "https://example.com/a.list");
        let digest = digest_of(b"hello");
        assert!(!c.holds(&digest), "no entry");
        let mut meta = Meta::default();
        c.store(b"hello", &mut meta).unwrap();
        assert!(c.holds(&digest));
        assert!(!c.holds(&digest_of(b"another")), "another body");
        std::fs::write(c.path().join("data"), b"hellp").unwrap();
        assert!(!c.holds(&digest), "a damaged body");
        std::fs::remove_file(c.path().join("data")).unwrap();
        assert!(!c.holds(&digest), "no body");
    }

    /// The digest is made from the body: a `Debug` says that there is one,
    /// and no more.
    #[test]
    fn the_debug_of_the_metadata_does_not_show_the_digest() {
        let mut meta = Meta {
            url: "https://example.com/a.list?token=S3CR3T".into(),
            ..Meta::default()
        };
        assert!(format!("{meta:?}").contains("sha256: None"));
        let root = tempfile::tempdir().unwrap();
        CacheDir::for_url(root.path(), &meta.url)
            .store(b"hello", &mut meta)
            .unwrap();
        let shown = format!("{meta:?}");
        assert!(shown.contains("sha256: Some(\"…\")"), "{shown}");
        assert!(!shown.contains(meta.sha256.as_deref().unwrap()), "{shown}");
        assert!(!shown.contains("S3CR3T"), "{shown}");
    }
}
