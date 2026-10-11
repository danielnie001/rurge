//! The sources of a profile that are downloaded: what `ProfileSources` makes of
//! the managed profile and the remote includes a load came across. Real
//! files are loaded for the `SourceInfo`s, the server is a loopback one, and
//! the cache the downloads land in is a temporary directory.

mod common;

use common::LogCapture;
use rurge_config::config::{LoadOptions, load};
use rurge_config::{Remote, SourceInfo};
use rurge_engine::profile::{ProfileSources, SourcesOptions, Suspended, disk_remote};
use rurge_engine::stack::resource_client;
use rurge_net::resource::{ResourceOptions, ResourceSource, resource_key};
use rurge_net::testing::{TestServer, seed_cache};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::time::timeout;
use url::Url;

/// What a report of `changed` waits for after the last move.
const QUIET: Duration = Duration::from_secs(1);

fn fast() -> ResourceOptions {
    ResourceOptions {
        fetch_timeout: Duration::from_secs(5),
        min_backoff: Duration::from_millis(50),
        max_backoff: Duration::from_millis(200),
        debounce: Duration::from_millis(50),
        ..ResourceOptions::default()
    }
}

fn ago(d: Duration) -> SystemTime {
    SystemTime::now() - d
}

const HOUR: Duration = Duration::from_secs(3600);

/// The body of a downloaded profile, whatever it says.
const COPY: &str = "[Rule]\nDOMAIN,copy.example,DIRECT\nFINAL,DIRECT\n";

/// A data directory, a local profile and a server for one test.
struct World {
    dir: tempfile::TempDir,
    server: TestServer,
}

impl World {
    async fn new() -> World {
        World {
            dir: tempfile::tempdir().unwrap(),
            server: TestServer::spawn().await,
        }
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn profile(&self) -> PathBuf {
        self.dir.path().join("main.conf")
    }

    fn url(&self, path: &str) -> Url {
        self.server.url(path)
    }

    /// The key the manager and the API know the resource at `path` by.
    fn key(&self, path: &str) -> String {
        resource_key(&ResourceSource::Url(self.url(path)))
    }

    /// `body` is the copy of `path` that was checked at `fetched_at`.
    fn seed(&self, path: &str, body: &str, fetched_at: SystemTime) {
        seed_cache(&self.data(), &self.url(path), body.as_bytes(), fetched_at);
    }

    fn write(&self, text: &str) {
        std::fs::write(self.profile(), text).unwrap();
    }

    /// A local profile that is managed from `path`: `line` is what follows
    /// the address, and it has rules of its own.
    fn write_managed(&self, path: &str, line: &str) {
        self.write(&format!(
            "#!MANAGED-CONFIG {} {line}\n[General]\nloglevel = notify\n[Rule]\nDOMAIN,local.example,DIRECT\nFINAL,DIRECT\n",
            self.url(path)
        ));
    }

    /// A downloaded managed profile, whose own line keeps it updated from
    /// `path`.
    fn managed_copy(&self, path: &str, line: &str) -> String {
        format!("#!MANAGED-CONFIG {} {line}\n{COPY}", self.url(path))
    }

    /// A local profile that includes the files at `paths`.
    fn write_including(&self, paths: &[&str]) {
        let mut text = String::from("[Rule]\n");
        for path in paths {
            text += &format!("#!include {}\n", self.url(path));
        }
        text += "FINAL,DIRECT\n";
        self.write(&text);
    }

    /// The local file was last changed at `at`.
    fn touch(&self, at: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(self.profile())
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    /// What loading the local profile comes across, with the downloads there
    /// are on disk.
    fn load(&self) -> SourceInfo {
        let mut opts = LoadOptions::for_tests();
        opts.remote = Some(disk_remote(&self.data()));
        load(&self.profile(), &opts).unwrap().config.source
    }

    fn sources(&self, offline: bool) -> Arc<ProfileSources> {
        ProfileSources::new(SourcesOptions {
            profile: self.profile(),
            load: LoadOptions::for_tests(),
            data_dir: self.data(),
            offline,
            client: resource_client().unwrap(),
            resources: fast(),
        })
    }

    /// The resource at `path` is served as `body` from now on, and updated.
    async fn move_to(&self, sources: &ProfileSources, path: &str, body: &str) {
        self.server.set(path, body.to_string());
        sources
            .update(&self.key(path))
            .await
            .expect("a tracked source")
            .expect("the update succeeds");
    }
}

/// The text a copy on disk is read as.
fn text_of(remote: Remote) -> String {
    match remote {
        Remote::Text { text, .. } => text,
        other => panic!("expected a text, got {other:?}"),
    }
}

fn why_of(remote: Remote) -> Option<String> {
    match remote {
        Remote::Missing { why } => why,
        other => panic!("expected nothing, got {other:?}"),
    }
}

/// A call of `changed` that does not return within `window`. The window is
/// all there is to wait for: a report that is not due comes a quiet second
/// after the move that causes it.
async fn stays_quiet(sources: &ProfileSources, window: Duration) {
    assert!(
        timeout(window, sources.changed()).await.is_err(),
        "a change was reported"
    );
}

/// The disk view gives what was downloaded earlier, with the file it is in and
/// the time it was checked.
#[tokio::test]
async fn the_disk_view_gives_what_was_downloaded() {
    let w = World::new().await;
    let url = w.url("/a");
    let checked = ago(HOUR);
    w.seed("/a", COPY, checked);
    match disk_remote(&w.data()).get(&url) {
        Remote::Text {
            text,
            path,
            fetched_at,
        } => {
            assert_eq!(text, COPY);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), COPY);
            assert_eq!(fetched_at, checked);
        }
        other => panic!("{other:?}"),
    }
}

/// A body that is not text is not a profile: the copy reads as missing, and
/// the reason says why.
#[tokio::test]
async fn the_disk_view_does_not_read_a_body_that_is_not_utf8() {
    let w = World::new().await;
    seed_cache(
        &w.data(),
        &w.url("/b"),
        &[0xff, 0xfe, 0x00, 0x80],
        ago(HOUR),
    );
    assert_eq!(
        why_of(disk_remote(&w.data()).get(&w.url("/b"))).as_deref(),
        Some("the downloaded file is not UTF-8 text")
    );
}

#[tokio::test]
async fn the_disk_view_has_nothing_for_an_address_never_downloaded() {
    let w = World::new().await;
    assert_eq!(why_of(disk_remote(&w.data()).get(&w.url("/c"))), None);
}

/// The view of a tracked source also says why there is nothing, when the
/// manager knows: the reason is what the load shows in its warning.
#[tokio::test]
async fn the_view_of_the_sources_says_why_a_download_failed() {
    let w = World::new().await;
    w.write_including(&["/gone"]);
    let sources = w.sources(false);
    sources.track(&w.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert_eq!(
        why_of(sources.remote().get(&w.url("/gone"))).as_deref(),
        Some("http status 404")
    );
    // the disk knows nothing of it
    assert_eq!(why_of(disk_remote(&w.data()).get(&w.url("/gone"))), None);
    // and a source with a copy gives the copy
    w.server.set("/there", COPY);
    w.write_including(&["/gone", "/there"]);
    sources.track(&w.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert_eq!(text_of(sources.remote().get(&w.url("/there"))), COPY);
}

/// The view of the sources reads a cached body that is not text as missing
/// too, for the same reason.
#[tokio::test]
async fn the_view_of_the_sources_does_not_read_a_body_that_is_not_utf8_either() {
    let w = World::new().await;
    seed_cache(&w.data(), &w.url("/b"), &[0xff, 0xfe], ago(HOUR));
    let sources = w.sources(true);
    assert_eq!(
        why_of(sources.remote().get(&w.url("/b"))).as_deref(),
        Some("the downloaded file is not UTF-8 text")
    );
}

/// A source without a copy is downloaded, and the download is a change.
#[tokio::test]
async fn the_first_download_of_a_managed_profile_is_a_change() {
    let w = World::new().await;
    w.server.set("/managed", COPY);
    w.write_managed("/managed", "interval=3600");
    let sources = w.sources(false);
    sources.track(&w.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert_eq!(
        text_of(disk_remote(&w.data()).get(&w.url("/managed"))),
        COPY
    );
    timeout(Duration::from_secs(4), sources.changed())
        .await
        .expect("the download is reported");
    // the report is made once
    stays_quiet(&sources, Duration::from_millis(300)).await;
}

/// A body that is not text is not taken: the copy that exists stays, and the
/// reason is the source's.
#[tokio::test]
async fn a_download_that_is_not_text_is_not_taken() {
    let w = World::new().await;
    w.server.set("/managed", vec![0xffu8, 0xfe, 0x00]);
    w.write_managed("/managed", "interval=60");
    w.seed("/managed", COPY, ago(HOUR));
    let sources = w.sources(false);
    sources.track(&w.load());
    sources
        .update(&w.key("/managed"))
        .await
        .expect("tracked")
        .expect_err("the body is refused");
    let statuses = sources.statuses();
    assert_eq!(
        statuses[0].last_error.as_deref(),
        Some("the downloaded file is not UTF-8 text")
    );
    assert_eq!(
        text_of(disk_remote(&w.data()).get(&w.url("/managed"))),
        COPY
    );
}

/// Two moves within the quiet second are one report, a second after the last.
#[tokio::test]
async fn two_moves_inside_the_quiet_second_are_reported_once() {
    let w = World::new().await;
    w.seed(
        "/inc",
        "[Rule]\nDOMAIN,one.example,DIRECT\n",
        ago(Duration::from_secs(10)),
    );
    w.write_including(&["/inc"]);
    let sources = w.sources(false);
    sources.track(&w.load());
    w.move_to(&sources, "/inc", "[Rule]\nDOMAIN,two.example,DIRECT\n")
        .await;
    w.move_to(&sources, "/inc", "[Rule]\nDOMAIN,three.example,DIRECT\n")
        .await;
    let last = Instant::now();
    timeout(Duration::from_secs(4), sources.changed())
        .await
        .expect("the moves are reported");
    assert!(
        last.elapsed() >= QUIET - Duration::from_millis(100),
        "reported after {:?}",
        last.elapsed()
    );
    // what a stray second report would be late by is the quiet second
    stays_quiet(&sources, QUIET + Duration::from_millis(500)).await;
}

/// A source that comes with a copy on disk is no change.
#[tokio::test]
async fn a_source_registered_with_a_copy_is_no_change() {
    let w = World::new().await;
    w.seed("/inc", COPY, ago(Duration::from_secs(10)));
    w.write_including(&["/inc"]);
    let sources = w.sources(false);
    sources.track(&w.load());
    // a report that is not due comes a quiet second after the registration
    stays_quiet(&sources, QUIET + Duration::from_millis(500)).await;
}

/// A source whose interval changed is retired and registered again; that is
/// no change either.
#[tokio::test]
async fn a_source_registered_again_is_no_change() {
    let w = World::new().await;
    w.seed("/managed", COPY, ago(Duration::from_secs(10)));
    w.write_managed("/managed", "interval=3600");
    let sources = w.sources(false);
    sources.track(&w.load());
    w.write_managed("/managed", "interval=7200");
    sources.track(&w.load());
    assert_eq!(sources.statuses().len(), 1);
    stays_quiet(&sources, QUIET + Duration::from_millis(500)).await;
}

/// The call is dropped and made again every 200 ms, as the main loop does: the
/// report still comes a second after the last move, because what it waits for
/// is kept by the sources and not by the call.
#[tokio::test]
async fn a_call_dropped_and_made_again_is_reported_a_second_after_the_last_move() {
    let w = World::new().await;
    w.seed("/inc", COPY, ago(Duration::from_secs(10)));
    w.write_including(&["/inc"]);
    let sources = w.sources(false);
    sources.track(&w.load());
    w.move_to(&sources, "/inc", "[Rule]\nDOMAIN,two.example,DIRECT\n")
        .await;
    let last = Instant::now();
    while timeout(Duration::from_millis(200), sources.changed())
        .await
        .is_err()
    {
        assert!(
            last.elapsed() < Duration::from_secs(3),
            "not reported in {:?}",
            last.elapsed()
        );
    }
    assert!(
        last.elapsed() >= QUIET - Duration::from_millis(100),
        "reported after {:?}",
        last.elapsed()
    );
}

/// A version that arrives when nobody is waiting (a reload is under way) is
/// reported by the next call.
#[tokio::test]
async fn a_move_after_a_report_is_reported_by_the_next_call() {
    let w = World::new().await;
    w.seed("/inc", COPY, ago(Duration::from_secs(10)));
    w.write_including(&["/inc"]);
    let sources = w.sources(false);
    sources.track(&w.load());
    w.move_to(&sources, "/inc", "[Rule]\nDOMAIN,two.example,DIRECT\n")
        .await;
    timeout(Duration::from_secs(4), sources.changed())
        .await
        .expect("the first move is reported");
    // nobody calls `changed` now
    w.move_to(&sources, "/inc", "[Rule]\nDOMAIN,three.example,DIRECT\n")
        .await;
    timeout(Duration::from_secs(4), sources.changed())
        .await
        .expect("the second move is reported");
}

/// A source that is registered while a call waits is part of what it waits
/// for.
#[tokio::test]
async fn a_source_registered_while_a_call_waits_is_noticed_by_that_call() {
    let w = World::new().await;
    w.seed("/one", COPY, ago(Duration::from_secs(10)));
    w.seed("/two", COPY, ago(Duration::from_secs(10)));
    w.write_including(&["/one"]);
    let sources = w.sources(false);
    sources.track(&w.load());
    let waiting = tokio::spawn({
        let sources = sources.clone();
        async move { sources.changed().await }
    });
    // let the call begin
    tokio::task::yield_now().await;
    w.write_including(&["/one", "/two"]);
    sources.track(&w.load());
    w.move_to(&sources, "/two", "[Rule]\nDOMAIN,two.example,DIRECT\n")
        .await;
    timeout(Duration::from_secs(4), waiting)
        .await
        .expect("reported")
        .unwrap();
}

/// A source that is retired and registered again while a call waits keeps
/// what it had moved to: the move was made, and the profile in use has not
/// seen it.
#[tokio::test]
async fn a_move_is_not_lost_when_its_source_is_registered_again() {
    let w = World::new().await;
    w.seed("/managed", COPY, ago(Duration::from_secs(10)));
    w.write_managed("/managed", "interval=3600");
    let sources = w.sources(false);
    sources.track(&w.load());
    w.move_to(&sources, "/managed", COPY.replace("copy", "moved").as_str())
        .await;
    let waiting = tokio::spawn({
        let sources = sources.clone();
        async move { sources.changed().await }
    });
    tokio::task::yield_now().await;
    w.write_managed("/managed", "interval=7200");
    sources.track(&w.load());
    timeout(Duration::from_secs(4), waiting)
        .await
        .expect("reported")
        .unwrap();
}

/// A source that is not tracked any more has nothing to report.
#[tokio::test]
async fn a_move_of_a_source_that_is_gone_is_not_reported() {
    let w = World::new().await;
    w.seed("/one", COPY, ago(Duration::from_secs(10)));
    w.seed("/two", COPY, ago(Duration::from_secs(10)));
    w.write_including(&["/one", "/two"]);
    let sources = w.sources(false);
    sources.track(&w.load());
    w.move_to(&sources, "/two", "[Rule]\nDOMAIN,two.example,DIRECT\n")
        .await;
    w.write_including(&["/one"]);
    sources.track(&w.load());
    stays_quiet(&sources, QUIET + Duration::from_millis(500)).await;
}

/// A profile on the way to the one that is updated is read from its copy and
/// never asked for.
#[tokio::test]
async fn a_hop_is_not_registered() {
    let w = World::new().await;
    // the copy of /first names /second, which is where the profile is served
    w.seed(
        "/first",
        &format!(
            "#!MANAGED-CONFIG {} interval=3600\n[Rule]\nFINAL,DIRECT\n",
            w.url("/second")
        ),
        ago(HOUR),
    );
    w.server.set("/first", "must not be asked for");
    w.server.set("/second", COPY);
    w.write_managed("/first", "interval=3600");
    let info = w.load();
    assert_eq!(info.remote.len(), 2);
    let sources = w.sources(false);
    sources.track(&info);
    assert!(sources.settle(Duration::from_secs(5)).await);
    let shown: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(shown, [w.key("/second")]);
    assert_eq!(w.server.hits("/first"), 0);
    assert_eq!(w.server.hits("/second"), 1);
}

/// What the next generation does not name is not tracked, and not asked for.
#[tokio::test]
async fn a_source_the_next_generation_does_not_name_is_dropped() {
    let w = World::new().await;
    w.write_including(&["/one", "/two"]);
    let sources = w.sources(false);
    sources.track(&w.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert_eq!(sources.statuses().len(), 2);
    w.write_including(&["/one"]);
    sources.track(&w.load());
    let shown: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(shown, [w.key("/one")]);
    // neither has anything to download, so both are asked again after a
    // backoff of 200 ms at most: in the time it takes to wait that out three
    // times, the one that is kept is asked for again and the other is not
    let (one, two) = (w.server.hits("/one"), w.server.hits("/two"));
    assert!(two >= 1);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(w.server.hits("/two"), two, "a dropped source was asked for");
    assert!(
        w.server.hits("/one") > one,
        "the other one is not asked for"
    );
}

/// A shorter interval counts at once: a copy that is old by it is refreshed.
#[tokio::test]
async fn a_shorter_interval_refreshes_a_copy_that_is_old_by_it() {
    let w = World::new().await;
    w.server.set("/managed", COPY);
    w.seed("/managed", COPY, ago(HOUR));
    w.write_managed("/managed", "interval=86400");
    let sources = w.sources(false);
    sources.track(&w.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    // nothing is old by a day
    assert_eq!(w.server.hits("/managed"), 0);
    w.write_managed("/managed", "interval=60");
    sources.track(&w.load());
    common::wait_until("the refresh", || w.server.hits("/managed") >= 1).await;
}

/// The local file that was changed after the last update is the profile until
/// the next successful check. A check that finds nothing new (the whole body
/// the copy is, or "not modified") is one: it ends the precedence of the local
/// file, and it is said once for each check.
#[tokio::test]
async fn a_check_that_changes_nothing_ends_the_precedence_of_the_local_file_once() {
    let w = World::new().await;
    let copy = w.managed_copy("/managed", "interval=3600");
    w.server.set("/managed", copy.clone());
    w.seed("/managed", &copy, ago(Duration::from_secs(600)));
    w.write_managed("/managed", "interval=3600");
    let info = w.load();
    assert_eq!(info.main_text, info.main, "the local file is in effect");
    let sources = w.sources(false);
    sources.track(&info);
    assert!(!sources.outdated());
    // the first answer is the whole body, which the copy is: nothing new
    w.move_to(&sources, "/managed", &copy).await;
    assert!(sources.outdated());
    assert!(!sources.outdated(), "once for a check");
    // the second one is a 304
    sources
        .update(&w.key("/managed"))
        .await
        .unwrap()
        .expect("checked");
    assert!(
        w.server
            .requests()
            .last()
            .unwrap()
            .header("if-none-match")
            .is_some()
    );
    assert!(sources.outdated());
    assert!(!sources.outdated());
    // no version moved
    stays_quiet(&sources, QUIET + Duration::from_millis(300)).await;
    // the next generation has the copy in effect
    let next = w.load();
    assert_ne!(next.main_text, next.main);
    sources.track(&next);
    assert!(!sources.outdated());
}

/// Only a local file that has a downloaded copy behind it is ever outdated.
#[tokio::test]
async fn nothing_else_is_outdated() {
    let w = World::new().await;
    w.server.set("/managed", COPY);
    // there was no copy when the generation was tracked: the local file is
    // all there was, and the download that follows does not change that
    w.write_managed("/managed", "interval=3600");
    let sources = w.sources(false);
    sources.track(&w.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert!(!sources.outdated());
    // a profile that is not managed has no copy to give way to
    w.write_including(&["/managed"]);
    sources.track(&w.load());
    w.move_to(&sources, "/managed", COPY).await;
    assert!(!sources.outdated());
}

/// A local file whose time cannot be asked is taken as just changed: it is
/// not outdated.
#[tokio::test]
async fn a_local_file_whose_time_cannot_be_read_is_not_outdated() {
    let w = World::new().await;
    let copy = w.managed_copy("/managed", "interval=3600");
    w.server.set("/managed", copy.clone());
    w.seed("/managed", &copy, ago(Duration::from_secs(600)));
    w.write_managed("/managed", "interval=3600");
    let sources = w.sources(false);
    sources.track(&w.load());
    w.move_to(&sources, "/managed", &copy).await;
    assert!(sources.outdated());
    std::fs::remove_file(w.profile()).unwrap();
    // another check, and the file is gone: it cannot be told to be older
    sources
        .update(&w.key("/managed"))
        .await
        .unwrap()
        .expect("checked");
    assert!(!sources.outdated());
}

/// The copy is old and the update failed, but `strict` is not written.
#[tokio::test]
async fn a_profile_that_is_not_strict_is_never_suspended() {
    let w = World::new().await;
    w.seed("/managed", COPY, ago(2 * HOUR));
    w.write_managed("/managed", "interval=60");
    let sources = w.sources(false);
    sources.track(&w.load());
    sources
        .update(&w.key("/managed"))
        .await
        .unwrap()
        .expect_err("there is nothing at the address");
    assert!(sources.statuses()[0].last_error.is_some());
    assert_eq!(sources.strict(), None);
}

/// Due, and the last attempt failed: suspended since it fell due.
#[tokio::test]
async fn a_strict_profile_that_is_due_and_failed_is_suspended() {
    let w = World::new().await;
    let checked = ago(2 * HOUR);
    w.seed("/managed", COPY, checked);
    w.write_managed("/managed", "interval=60 strict=true");
    let sources = w.sources(false);
    sources.track(&w.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert_eq!(
        sources.strict(),
        Some(Suspended {
            since: checked + Duration::from_secs(60),
            why: "http status 404".to_string(),
        })
    );
}

/// Due, and the update worked: the profile is up to date.
#[tokio::test]
async fn a_strict_profile_that_is_due_and_updated_is_not_suspended() {
    let w = World::new().await;
    w.server.set("/managed", COPY.replace("copy", "new"));
    w.seed("/managed", COPY, ago(2 * HOUR));
    w.write_managed("/managed", "interval=60 strict=true");
    let sources = w.sources(false);
    sources.track(&w.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert_eq!(sources.statuses()[0].last_error, None);
    assert_eq!(sources.strict(), None);
}

/// Due is not enough: the attempt that is under way has not failed yet.
#[tokio::test]
async fn a_strict_profile_that_is_due_is_not_suspended_before_its_attempt_fails() {
    let w = World::new().await;
    w.server.set("/managed", COPY);
    w.server.set_status("/managed", 500);
    w.server.set_delay("/managed", Duration::from_millis(300));
    w.seed("/managed", COPY, ago(2 * HOUR));
    w.write_managed("/managed", "interval=60 strict=true");
    let sources = w.sources(false);
    sources.track(&w.load());
    // not tried yet, then under way
    assert_eq!(sources.strict(), None);
    common::wait_until("the attempt to begin", || w.server.hits("/managed") == 1).await;
    assert_eq!(sources.strict(), None);
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert_eq!(
        sources.strict().map(|suspended| suspended.why),
        Some("http status 500".to_string())
    );
}

/// Not due, and the last attempt failed: the copy is as new as the interval
/// asks it to be.
#[tokio::test]
async fn a_strict_profile_that_is_not_due_is_not_suspended_when_an_update_fails() {
    let w = World::new().await;
    w.seed("/managed", COPY, ago(Duration::from_secs(10)));
    w.write_managed("/managed", "interval=3600 strict=true");
    let sources = w.sources(false);
    sources.track(&w.load());
    sources
        .update(&w.key("/managed"))
        .await
        .unwrap()
        .expect_err("there is nothing at the address");
    assert!(sources.statuses()[0].last_error.is_some());
    assert_eq!(sources.strict(), None);
}

/// The strictness is the latest generation's, and it needs no new registration:
/// a profile that stops saying `strict=true` is not suspended any more.
#[tokio::test]
async fn the_strictness_of_the_latest_generation_counts() {
    let w = World::new().await;
    w.seed("/managed", COPY, ago(2 * HOUR));
    w.write_managed("/managed", "interval=60");
    let sources = w.sources(false);
    sources.track(&w.load());
    sources
        .update(&w.key("/managed"))
        .await
        .unwrap()
        .expect_err("there is nothing at the address");
    assert_eq!(sources.strict(), None);
    w.write_managed("/managed", "interval=60 strict=true");
    sources.track(&w.load());
    assert!(sources.strict().is_some());
    w.write_managed("/managed", "interval=60 strict=false");
    sources.track(&w.load());
    assert_eq!(sources.strict(), None);
}

/// Offline nothing is updated, so a due copy is as good as a failed one.
#[tokio::test]
async fn a_strict_profile_that_is_due_is_suspended_offline() {
    let w = World::new().await;
    let checked = ago(2 * HOUR);
    w.seed("/managed", COPY, checked);
    w.write_managed("/managed", "interval=60 strict=true");
    let sources = w.sources(true);
    sources.track(&w.load());
    assert_eq!(
        sources.strict(),
        Some(Suspended {
            since: checked + Duration::from_secs(60),
            why: "offline mode: the update cannot be attempted".to_string(),
        })
    );
    // not due: not suspended
    w.seed("/managed", COPY, ago(Duration::from_secs(10)));
    let fresh = w.sources(true);
    fresh.track(&w.load());
    assert_eq!(fresh.strict(), None);
    assert_eq!(w.server.requests().len(), 0);
}

/// Without a successful check the time to count from is the local file's,
/// and a file whose time cannot be read is not one to count from.
#[tokio::test]
async fn without_a_check_strict_counts_from_the_local_file() {
    let w = World::new().await;
    w.write_managed("/managed", "interval=60 strict=true");
    let changed = ago(2 * HOUR);
    w.touch(changed);
    let sources = w.sources(false);
    sources.track(&w.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert_eq!(
        sources.strict(),
        Some(Suspended {
            since: changed + Duration::from_secs(60),
            why: "http status 404".to_string(),
        })
    );
    // a file that was just changed is not due
    w.touch(SystemTime::now());
    assert_eq!(sources.strict(), None);
    w.touch(changed);
    assert!(sources.strict().is_some());
    // and one that cannot be asked is not counted from
    std::fs::remove_file(w.profile()).unwrap();
    assert_eq!(sources.strict(), None);
}

/// A check in the future is the clock having been turned back: nothing is
/// due then.
#[tokio::test]
async fn a_check_in_the_future_is_not_due() {
    let w = World::new().await;
    // a copy that was checked in the future is the newest text there is: it is
    // the profile, and it keeps itself updated
    let copy = w.managed_copy("/managed", "interval=60 strict=true");
    w.seed("/managed", &copy, SystemTime::now() + 2 * HOUR);
    w.write_managed("/managed", "interval=60 strict=true");
    let sources = w.sources(false);
    sources.track(&w.load());
    sources
        .update(&w.key("/managed"))
        .await
        .unwrap()
        .expect_err("there is nothing at the address");
    assert!(sources.statuses()[0].last_error.is_some());
    assert_eq!(sources.strict(), None);
}

/// An interval is any number of seconds a `u64` holds. It is not added to a
/// time, and a profile that asks for it is due never.
#[tokio::test]
async fn an_interval_of_the_largest_size_is_never_due() {
    let w = World::new().await;
    w.seed("/managed", COPY, ago(2 * HOUR));
    w.write_managed("/managed", &format!("interval={} strict=true", u64::MAX));
    let sources = w.sources(false);
    sources.track(&w.load());
    sources
        .update(&w.key("/managed"))
        .await
        .unwrap()
        .expect_err("there is nothing at the address");
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert_eq!(sources.strict(), None);
    assert!(!sources.outdated());
    // and without a copy, from the local file's time
    let bare = World::new().await;
    bare.write_managed("/managed", &format!("interval={} strict=true", u64::MAX));
    bare.touch(ago(2 * HOUR));
    let sources = bare.sources(false);
    sources.track(&bare.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert_eq!(sources.strict(), None);
}

/// With nothing to wait for the call is over at once.
#[tokio::test]
async fn settling_with_nothing_to_wait_for_returns_at_once() {
    let w = World::new().await;
    w.seed("/inc", COPY, ago(Duration::from_secs(10)));
    w.write_including(&["/inc"]);
    let sources = w.sources(false);
    assert!(sources.settle(Duration::from_secs(5)).await);
    sources.track(&w.load());
    let begun = Instant::now();
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert!(begun.elapsed() < Duration::from_secs(1));
}

/// A source without content is waited for until the first attempt is over,
/// whichever way it ends.
#[tokio::test]
async fn settling_waits_for_the_first_attempt_of_a_source_without_content() {
    let w = World::new().await;
    w.server.set("/slow", COPY);
    w.server.set_delay("/slow", Duration::from_millis(400));
    w.server.set("/fails", COPY);
    w.server.set_status("/fails", 500);
    w.server.set_delay("/fails", Duration::from_millis(300));
    w.write_including(&["/slow", "/fails"]);
    let sources = w.sources(false);
    sources.track(&w.load());
    let begun = Instant::now();
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert!(
        begun.elapsed() >= Duration::from_millis(350),
        "settled after {:?}",
        begun.elapsed()
    );
    assert_eq!(text_of(disk_remote(&w.data()).get(&w.url("/slow"))), COPY);
    let fails = sources.remote().get(&w.url("/fails"));
    assert_eq!(why_of(fails).as_deref(), Some("http status 500"));
}

/// A copy that is old is refreshed in the background, and nobody waits for
/// that, unless the profile is strict.
#[tokio::test]
async fn settling_does_not_wait_for_a_due_source_that_is_not_strict() {
    let w = World::new().await;
    w.server.set("/managed", COPY);
    w.server.set_delay("/managed", Duration::from_secs(2));
    w.seed("/managed", COPY, ago(2 * HOUR));
    w.write_managed("/managed", "interval=60");
    let sources = w.sources(false);
    sources.track(&w.load());
    let begun = Instant::now();
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert!(
        begun.elapsed() < Duration::from_secs(1),
        "settled after {:?}",
        begun.elapsed()
    );
}

#[tokio::test]
async fn settling_waits_for_a_due_strict_source() {
    let w = World::new().await;
    w.server.set("/managed", COPY);
    w.server.set_delay("/managed", Duration::from_millis(400));
    w.seed("/managed", COPY, ago(2 * HOUR));
    w.write_managed("/managed", "interval=60 strict=true");
    let sources = w.sources(false);
    sources.track(&w.load());
    let begun = Instant::now();
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert!(
        begun.elapsed() >= Duration::from_millis(350),
        "settled after {:?}",
        begun.elapsed()
    );
    // and what it waited for is what `strict` goes by
    assert_eq!(sources.strict(), None);
    assert_eq!(w.server.hits("/managed"), 1);
}

#[tokio::test]
async fn settling_stops_at_the_limit() {
    let w = World::new().await;
    w.server.set("/slow", COPY);
    w.server.set_delay("/slow", Duration::from_secs(3));
    w.write_including(&["/slow"]);
    let sources = w.sources(false);
    sources.track(&w.load());
    let begun = Instant::now();
    assert!(!sources.settle(Duration::from_millis(300)).await);
    assert!(
        begun.elapsed() < Duration::from_millis(1500),
        "gave up after {:?}",
        begun.elapsed()
    );
}

/// A source that is registered while the call waits is waited for too.
#[tokio::test]
async fn settling_waits_for_a_source_registered_meanwhile() {
    let w = World::new().await;
    w.server.set("/quick", COPY);
    w.server.set_delay("/quick", Duration::from_millis(100));
    w.server.set("/late", COPY);
    w.server.set_delay("/late", Duration::from_millis(800));
    w.write_including(&["/quick"]);
    let sources = w.sources(false);
    sources.track(&w.load());
    let begun = Instant::now();
    let settling = tokio::spawn({
        let sources = sources.clone();
        async move { sources.settle(Duration::from_secs(5)).await }
    });
    tokio::task::yield_now().await;
    w.write_including(&["/quick", "/late"]);
    sources.track(&w.load());
    assert!(settling.await.unwrap());
    assert!(
        begun.elapsed() >= Duration::from_millis(700),
        "settled after {:?}",
        begun.elapsed()
    );
    assert_eq!(text_of(disk_remote(&w.data()).get(&w.url("/late"))), COPY);
}

#[tokio::test]
async fn an_update_gives_the_result_of_the_download() {
    let w = World::new().await;
    w.server.set("/inc", COPY);
    w.write_including(&["/inc"]);
    let sources = w.sources(false);
    sources.track(&w.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert!(sources.update("not-a-key").await.is_none());
    assert_eq!(sources.update(&w.key("/inc")).await, Some(Ok(())));
    w.server.set_status("/inc", 503);
    assert_eq!(
        sources.update(&w.key("/inc")).await,
        Some(Err("http status 503".to_string()))
    );
}

/// The address of a source is a credential as a whole: where it goes is all
/// that shows.
#[tokio::test]
async fn nothing_but_where_a_link_goes_is_shown() {
    let log = LogCapture::start();
    let w = World::new().await;
    let managed = Url::parse(&format!(
        "http://USERMARK:PASSMARK@127.0.0.1:{}/PATHMARK/managed.conf?token=QUERYMARK",
        w.url("/").port().unwrap()
    ))
    .unwrap();
    let included = Url::parse(&format!(
        "http://USERMARK:PASSMARK@127.0.0.1:{}/PATHMARK/included.conf?token=QUERYMARK",
        w.url("/").port().unwrap()
    ))
    .unwrap();
    w.server.set("/PATHMARK/managed.conf", COPY);
    w.write(&format!(
        "#!MANAGED-CONFIG {managed} interval=3600 strict=true\n[Rule]\n#!include {included}\nFINAL,DIRECT\n"
    ));
    let sources = w.sources(false);
    sources.track(&w.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    // there is nothing at the address of the include: the update fails, and
    // that is logged
    let _ = sources
        .update(&resource_key(&ResourceSource::Url(included.clone())))
        .await;
    let statuses = sources.statuses();
    assert_eq!(statuses.len(), 2);
    let origin = format!("http://127.0.0.1:{}/…", w.url("/").port().unwrap());
    assert_eq!(
        statuses[0].label.as_deref(),
        Some(format!("managed profile ({origin})").as_str())
    );
    let key = resource_key(&ResourceSource::Url(included.clone()));
    assert_eq!(
        statuses[1].label.as_deref(),
        Some(format!("remote include {} ({origin})", &key[..8]).as_str())
    );
    let mut shown = format!("{statuses:?}");
    shown += &format!("{:?}", sources.remote().get(&managed));
    shown += &format!("{:?}", sources.remote().get(&included));
    shown += &format!("{:?}", sources.strict());
    // every line down to debug level: the manager's, the sources', and what
    // the HTTP library says about its pool of connections, each of which it
    // names by the target of the request
    let lines = log.lines();
    shown += &lines.join("\n");
    for marker in ["USERMARK", "PASSMARK", "PATHMARK", "QUERYMARK"] {
        assert!(!shown.contains(marker), "{marker} in: {shown}");
    }
    // the capture saw both kinds of line
    assert!(
        lines.iter().any(|l| l.contains("managed profile (")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("remote include")),
        "{lines:?}"
    );
}

/// The client asks the way a Surge profile is asked for.
#[tokio::test]
async fn the_resource_client_asks_as_a_surge_client_does() {
    let w = World::new().await;
    w.server.set("/p", COPY);
    resource_client()
        .unwrap()
        .get(&w.url("/p"), &Default::default())
        .await
        .unwrap();
    let agent = w.server.requests()[0]
        .header("user-agent")
        .map(str::to_string);
    assert!(
        agent
            .as_deref()
            .is_some_and(|a| a.contains("Surge-compatible")),
        "{agent:?}"
    );
}

/// The sources of the executable go to the network only when it is online.
#[tokio::test]
async fn the_production_sources_do_not_download_offline() {
    let w = World::new().await;
    w.server.set("/inc", COPY);
    w.write_including(&["/inc"]);
    let sources =
        ProfileSources::production(&w.profile(), &LoadOptions::for_tests(), &w.data(), true)
            .unwrap();
    sources.track(&w.load());
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert_eq!(
        sources.statuses()[0].last_error.as_deref(),
        Some("offline mode: no cached copy")
    );
    assert_eq!(w.server.requests().len(), 0);
}
