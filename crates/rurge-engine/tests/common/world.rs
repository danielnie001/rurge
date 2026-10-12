//! What the tests of the downloaded sources of a profile share: a data
//! directory, a local profile and a loopback server for one test, and the
//! small helpers around them.

use rurge_config::config::{LoadOptions, Loaded, load};
use rurge_config::{Remote, RemoteSource, SourceInfo};
use rurge_engine::profile::{ProfileSources, SourcesOptions, disk_remote};
use rurge_engine::stack::resource_client;
use rurge_net::resource::{ResourceOptions, ResourceSource, resource_key};
use rurge_net::testing::{TestServer, seed_cache};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::time::timeout;
use url::Url;

/// What a report of `changed` waits for after the last move.
pub const QUIET: Duration = Duration::from_secs(1);

pub fn fast() -> ResourceOptions {
    ResourceOptions {
        fetch_timeout: Duration::from_secs(5),
        min_backoff: Duration::from_millis(50),
        max_backoff: Duration::from_millis(200),
        debounce: Duration::from_millis(50),
        ..ResourceOptions::default()
    }
}

pub fn ago(d: Duration) -> SystemTime {
    SystemTime::now() - d
}

pub const HOUR: Duration = Duration::from_secs(3600);

/// The body of a downloaded profile, whatever it says.
pub const COPY: &str = "[Rule]\nDOMAIN,copy.example,DIRECT\nFINAL,DIRECT\n";

/// A data directory, a local profile and a server for one test.
pub struct World {
    pub dir: tempfile::TempDir,
    pub server: TestServer,
}

impl World {
    pub async fn new() -> World {
        World {
            dir: tempfile::tempdir().unwrap(),
            server: TestServer::spawn().await,
        }
    }

    pub fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    pub fn profile(&self) -> PathBuf {
        self.dir.path().join("main.conf")
    }

    pub fn url(&self, path: &str) -> Url {
        self.server.url(path)
    }

    /// The key the manager and the API know the resource at `path` by.
    pub fn key(&self, path: &str) -> String {
        resource_key(&ResourceSource::Url(self.url(path)))
    }

    /// `body` is the copy of `path` that was checked at `fetched_at`.
    pub fn seed(&self, path: &str, body: &str, fetched_at: SystemTime) {
        seed_cache(&self.data(), &self.url(path), body.as_bytes(), fetched_at);
    }

    pub fn write(&self, text: &str) {
        std::fs::write(self.profile(), text).unwrap();
    }

    /// A local profile that is managed from `path`: `line` is what follows
    /// the address, and it has rules of its own.
    pub fn write_managed(&self, path: &str, line: &str) {
        self.write(&format!(
            "#!MANAGED-CONFIG {} {line}\n[General]\nloglevel = notify\n[Rule]\nDOMAIN,local.example,DIRECT\nFINAL,DIRECT\n",
            self.url(path)
        ));
    }

    /// A downloaded managed profile, whose own line keeps it updated from
    /// `path`.
    pub fn managed_copy(&self, path: &str, line: &str) -> String {
        format!("#!MANAGED-CONFIG {} {line}\n{COPY}", self.url(path))
    }

    /// A local profile that includes the files at `paths`.
    pub fn write_including(&self, paths: &[&str]) {
        let mut text = String::from("[Rule]\n");
        for path in paths {
            text += &format!("#!include {}\n", self.url(path));
        }
        text += "FINAL,DIRECT\n";
        self.write(&text);
    }

    /// The local file was last changed at `at`.
    pub fn touch(&self, at: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(self.profile())
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    /// What loading the local profile comes across, with the downloads there
    /// are on disk.
    pub fn load(&self) -> SourceInfo {
        self.loaded(disk_remote(&self.data())).config.source
    }

    /// The local profile loaded with `remote` as the place downloaded text
    /// comes from.
    pub fn loaded(&self, remote: Arc<dyn RemoteSource>) -> Loaded {
        let mut opts = LoadOptions::for_tests();
        opts.remote = Some(remote);
        load(&self.profile(), &opts).unwrap()
    }

    pub fn sources(&self, offline: bool) -> Arc<ProfileSources> {
        self.sources_with(offline, fast())
    }

    pub fn sources_with(&self, offline: bool, resources: ResourceOptions) -> Arc<ProfileSources> {
        ProfileSources::new(SourcesOptions {
            profile: self.profile(),
            load: LoadOptions::for_tests(),
            data_dir: self.data(),
            offline,
            client: resource_client().unwrap(),
            resources,
        })
    }

    /// The resource at `path` is served as `body` from now on, and updated.
    pub async fn move_to(&self, sources: &ProfileSources, path: &str, body: &str) {
        self.server.set(path, body.to_string());
        sources
            .update(&self.key(path))
            .await
            .expect("a tracked source")
            .expect("the update succeeds");
    }
}

/// The text a copy on disk is read as.
pub fn text_of(remote: Remote) -> String {
    match remote {
        Remote::Text { text, .. } => text,
        other => panic!("expected a text, got {other:?}"),
    }
}

pub fn why_of(remote: Remote) -> Option<String> {
    match remote {
        Remote::Missing { why } => why,
        other => panic!("expected nothing, got {other:?}"),
    }
}

/// A call of `changed` that does not return within `window`. The window is
/// all there is to wait for: a report that is not due comes a quiet second
/// after the move that causes it.
pub async fn stays_quiet(sources: &ProfileSources, window: Duration) {
    assert!(
        timeout(window, sources.changed()).await.is_err(),
        "a change was reported"
    );
}
