//! What `ProfileSources` does with a body the manager has downloaded before it
//! takes it: the whole profile has to load with it. Real files are loaded, the
//! server is a loopback one, and the cache the downloads land in is a
//! temporary directory.

mod common;

use common::{COPY, HOUR, LogCapture, QUIET, World, ago, fast, stays_quiet, text_of, wait_until};
use rurge_config::Remote;
use rurge_config::codes;
use rurge_config::config::Loaded;
use rurge_engine::profile::{ProfileSources, disk_remote};
use rurge_net::resource::{ResourceOptions, ResourceSource, resource_key};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::time::timeout;
use url::Url;

/// What a captive portal or an error page answers.
const HTML: &str =
    "<html><head><title>Sign in</title></head><body>Welcome to the Wi-Fi</body></html>";

/// The included file of the waiting tests.
const X_BODY: &str = "[Rule]\nDOMAIN,x.example,DIRECT\n";

/// The text of a managed profile: the line that keeps it updated from
/// `/managed`, then `body`.
fn managed(w: &World, body: &str) -> String {
    format!(
        "#!MANAGED-CONFIG {} interval=3600\n{body}",
        w.url("/managed")
    )
}

/// A local file that is only a pointer to the managed profile: the downloaded
/// copy is always the profile.
fn point_at_managed(w: &World, line: &str) {
    w.write(&format!("#!MANAGED-CONFIG {} {line}\n", w.url("/managed")));
}

/// The managed profile's update, awaited: the manager's answer.
async fn update(sources: &ProfileSources, w: &World, path: &str) -> Result<(), String> {
    sources
        .update(&w.key(path))
        .await
        .expect("a tracked source")
}

/// A report of `changed`, and no second one.
async fn reported_once(sources: &ProfileSources) {
    timeout(Duration::from_secs(4), sources.changed())
        .await
        .expect("the change is reported");
    // the quiet second is over by the time a second report would come, and
    // half a second more is the margin
    stays_quiet(sources, QUIET + Duration::from_millis(500)).await;
}

/// What the loader reads for `path` is `text`.
fn reads(sources: &Arc<ProfileSources>, w: &World, path: &str, text: &str) -> bool {
    matches!(sources.remote().get(&w.url(path)), Remote::Text { text: read, .. } if read == text)
}

fn loaded(w: &World, sources: &Arc<ProfileSources>) -> Loaded {
    w.loaded(sources.remote())
}

// ---------------------------------------------------------------------------
// bodies that are no profile

/// The managed profile is served as `body`, which is no profile: it is not
/// taken, the copy that was there stays, and the reason is the source's.
async fn a_body_that_is_no_profile_is_not_taken(body: &str) {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    let old = managed(&w, COPY);
    w.seed("/managed", &old, ago(HOUR));
    w.server.set("/managed", body.to_string());
    let sources = w.sources(false);
    sources.track(&w.load());
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("the body is refused");
    assert!(reason.starts_with("does not load: "), "{reason}");
    let status = &sources.statuses()[0];
    assert_eq!(
        (status.state, status.last_error.as_deref()),
        ("failed", Some(reason.as_str()))
    );
    assert!(reads(&sources, &w, "/managed", &old));
    assert_eq!(text_of(disk_remote(&w.data()).get(&w.url("/managed"))), old);
    // No version moved: a report that was due would come a quiet second after
    // the move, and 300 ms more is the margin.
    stays_quiet(&sources, QUIET + Duration::from_millis(300)).await;
}

#[tokio::test]
async fn a_page_is_not_taken_as_the_managed_profile() {
    a_body_that_is_no_profile_is_not_taken(HTML).await;
}

#[tokio::test]
async fn an_empty_body_is_not_taken_as_the_managed_profile() {
    a_body_that_is_no_profile_is_not_taken("").await;
}

#[tokio::test]
async fn a_profile_without_final_is_not_taken_as_the_managed_profile() {
    a_body_that_is_no_profile_is_not_taken("[Rule]\nDOMAIN,x.example,DIRECT\n").await;
}

/// A page where an included file is meant to be would be an empty list of
/// rules, and the rules would silently be gone.
#[tokio::test]
async fn a_page_is_not_taken_as_an_included_file() {
    let w = World::new().await;
    w.write_including(&["/inc"]);
    let old = "[Rule]\nDOMAIN,one.example,DIRECT\n";
    w.seed("/inc", old, ago(HOUR));
    w.server.set("/inc", HTML);
    let sources = w.sources(false);
    sources.track(&w.load());
    let reason = update(&sources, &w, "/inc")
        .await
        .expect_err("the page is refused");
    assert_eq!(reason, "lacks the section it is included into");
    let status = &sources.statuses()[0];
    assert_eq!(
        (status.state, status.last_error.as_deref()),
        ("failed", Some(reason.as_str()))
    );
    assert!(reads(&sources, &w, "/inc", old));
    // No version moved: a report that was due would come a quiet second after
    // the move, and 300 ms more is the margin.
    stays_quiet(&sources, QUIET + Duration::from_millis(300)).await;
}

/// The local file stopped naming the address, and nobody has reloaded yet.
#[tokio::test]
async fn an_address_the_profile_does_not_name_any_more_is_not_taken() {
    let w = World::new().await;
    w.write_including(&["/inc"]);
    let old = "[Rule]\nDOMAIN,one.example,DIRECT\n";
    w.seed("/inc", old, ago(HOUR));
    let sources = w.sources(false);
    sources.track(&w.load());
    w.write("[Rule]\nFINAL,DIRECT\n");
    w.server.set("/inc", "[Rule]\nDOMAIN,two.example,DIRECT\n");
    let reason = update(&sources, &w, "/inc")
        .await
        .expect_err("nothing refers to it");
    assert_eq!(reason, "the profile no longer refers to this address");
    assert!(reads(&sources, &w, "/inc", old));
}

// ---------------------------------------------------------------------------
// sources that have to change together

/// The managed profile of the pair: its group uses `policy`, which the
/// included file defines.
fn pair_managed(w: &World, policy: &str) -> String {
    managed(
        w,
        &format!(
            "[Proxy]\n#!include {}\n[Proxy Group]\nG = select, {policy}\n[Rule]\nFINAL,G\n",
            w.url("/inc")
        ),
    )
}

fn pair_included(policy: &str) -> String {
    format!("[Proxy]\n{policy} = direct\n")
}

/// A managed profile and a file it includes, both downloaded, whose next
/// versions load together and not one without the other.
async fn the_pair() -> (World, Arc<ProfileSources>) {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    w.seed("/managed", &pair_managed(&w, "P1"), ago(HOUR));
    w.seed("/inc", &pair_included("P1"), ago(HOUR));
    let sources = w.sources(false);
    sources.track(&w.load());
    w.server.set("/managed", pair_managed(&w, "P2"));
    w.server.set("/inc", pair_included("P2"));
    (w, sources)
}

/// What the loader reads is the new pair, and it loads.
fn assert_the_pair_moved(w: &World, sources: &Arc<ProfileSources>) {
    assert!(reads(sources, w, "/managed", &pair_managed(w, "P2")));
    assert!(reads(sources, w, "/inc", &pair_included("P2")));
    let loaded = loaded(w, sources);
    assert!(!loaded.diagnostics.has_errors());
    let names: Vec<_> = loaded
        .config
        .policies
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, ["P2"]);
}

#[tokio::test]
async fn the_managed_profile_that_needs_its_include_waits_for_it() {
    let (w, sources) = the_pair().await;
    // alone, the group uses a policy that nothing defines yet
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("it does not load by itself");
    assert!(reason.starts_with("does not load: "), "{reason}");
    // the error is in the file that was downloaded: the reason says where
    assert!(
        reason.ends_with(')') && reason.contains(" (line "),
        "{reason}"
    );
    assert!(!reason.contains("resources"), "{reason}");
    assert!(reads(&sources, &w, "/managed", &pair_managed(&w, "P1")));
    // the other half arrives, and both are taken
    update(&sources, &w, "/inc")
        .await
        .expect("together they load");
    reported_once(&sources).await;
    assert_the_pair_moved(&w, &sources);
}

#[tokio::test]
async fn the_include_that_the_managed_profile_needs_waits_for_it() {
    let (w, sources) = the_pair().await;
    // alone, the group of the old version loses its policy
    let reason = update(&sources, &w, "/inc")
        .await
        .expect_err("it does not load by itself");
    assert!(reason.starts_with("does not load: "), "{reason}");
    // the error is in a file that was not downloaded just now
    assert!(!reason.contains("(line"), "{reason}");
    assert!(reads(&sources, &w, "/inc", &pair_included("P1")));
    update(&sources, &w, "/managed")
        .await
        .expect("together they load");
    reported_once(&sources).await;
    assert_the_pair_moved(&w, &sources);
}

/// The verdicts are made one after the other, whichever body is checked
/// first: one of the two is kept, and the other finds it.
#[tokio::test]
async fn two_updates_at_once_are_taken_together() {
    let (w, sources) = the_pair().await;
    let (managed, included) = tokio::join!(
        update(&sources, &w, "/managed"),
        update(&sources, &w, "/inc")
    );
    assert!(
        managed.is_ok() != included.is_ok(),
        "one waits for the other: {managed:?} {included:?}"
    );
    reported_once(&sources).await;
    assert_the_pair_moved(&w, &sources);
}

/// A version that loads by itself is taken, and the ones that wait for a
/// partner stay where they are.
#[tokio::test]
async fn a_version_that_loads_alone_leaves_the_kept_ones_where_they_are() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    let text = |w: &World, policy: &str| {
        managed(
            w,
            &format!(
                "[Proxy]\n#!include {}\n[Proxy Group]\nG = select, {policy}\n[Rule]\n#!include {}\nFINAL,G\n",
                w.url("/inc"),
                w.url("/more")
            ),
        )
    };
    w.seed("/managed", &text(&w, "P1"), ago(HOUR));
    w.seed("/inc", &pair_included("P1"), ago(HOUR));
    w.seed("/more", "[Rule]\nDOMAIN,one.example,DIRECT\n", ago(HOUR));
    let sources = w.sources(false);
    sources.track(&w.load());
    w.server.set("/managed", text(&w, "P2"));
    w.server.set("/inc", pair_included("P2"));
    w.server.set("/more", "[Rule]\nDOMAIN,two.example,DIRECT\n");

    update(&sources, &w, "/managed")
        .await
        .expect_err("it needs the other file");
    // this one loads with the old versions of the others
    update(&sources, &w, "/more").await.expect("it loads alone");
    assert!(reads(
        &sources,
        &w,
        "/more",
        "[Rule]\nDOMAIN,two.example,DIRECT\n"
    ));
    assert!(reads(&sources, &w, "/managed", &text(&w, "P1")));
    // and the one that waited is found by its partner
    update(&sources, &w, "/inc")
        .await
        .expect("together they load");
    assert!(reads(&sources, &w, "/managed", &text(&w, "P2")));
    assert!(reads(&sources, &w, "/inc", &pair_included("P2")));
}

// ---------------------------------------------------------------------------
// versions that refer to sources never downloaded

fn old_managed(w: &World) -> String {
    managed(w, "[Rule]\nFINAL,DIRECT\n")
}

/// The new version includes a file that has never been downloaded; the
/// profile loads without it.
fn new_managed(w: &World) -> String {
    managed(
        w,
        &format!("[Rule]\n#!include {}\nFINAL,DIRECT\n", w.url("/x")),
    )
}

/// A managed profile that was downloaded, whose next version names `/x`.
async fn the_world_of_x() -> (World, Arc<ProfileSources>) {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    w.seed("/managed", &old_managed(&w), ago(HOUR));
    let sources = w.sources(false);
    sources.track(&w.load());
    w.server.set("/managed", new_managed(&w));
    (w, sources)
}

const WAITING: &str = "waiting for the sources this version refers to";

/// The version is not taken with a piece missing and filled in a few seconds
/// later: it waits for the file, and both are taken together.
#[tokio::test]
async fn a_new_include_that_was_never_downloaded_is_waited_for() {
    let (w, sources) = the_world_of_x().await;
    w.server.set("/x", X_BODY);
    w.server.set_delay("/x", Duration::from_millis(800));
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("it waits for the file");
    assert_eq!(reason, WAITING);
    // the file is followed from now on, after the sources of the profile
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/managed"), w.key("/x")]);
    // until it is here the profile is the old version
    assert!(reads(&sources, &w, "/managed", &old_managed(&w)));
    wait_until("both files", || {
        reads(&sources, &w, "/managed", &new_managed(&w)) && reads(&sources, &w, "/x", X_BODY)
    })
    .await;
    reported_once(&sources).await;
    let loaded = loaded(&w, &sources);
    assert!(!loaded.diagnostics.has_errors());
    assert!(
        loaded
            .config
            .rules
            .iter()
            .any(|r| r.raw == "DOMAIN,x.example,DIRECT")
    );
}

/// A file that cannot be downloaded holds nothing back for long: the version
/// is taken the next time its source is asked, and the line is not expanded.
#[tokio::test]
async fn a_new_include_that_cannot_be_downloaded_does_not_hold_the_version_back() {
    let (w, sources) = the_world_of_x().await;
    // nothing is served at /x
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("the file has not been tried yet");
    assert_eq!(reason, WAITING);
    wait_until("the new version", || {
        reads(&sources, &w, "/managed", &new_managed(&w))
    })
    .await;
    reported_once(&sources).await;
    let loaded = loaded(&w, &sources);
    assert!(!loaded.diagnostics.has_errors());
    assert!(loaded.diagnostics.iter().any(|d| {
        d.code == codes::W_RESOURCE_UNAVAILABLE && d.message.contains("http status 404")
    }));
    let status = sources
        .statuses()
        .into_iter()
        .find(|s| s.key == w.key("/x"))
        .expect("the file is followed");
    assert_eq!(status.state, "failed");
}

/// Only `settle` takes it, and it does so when the file has been tried.
#[tokio::test]
async fn settling_takes_the_version_that_waited_once_the_file_has_been_tried() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    w.seed("/managed", &old_managed(&w), ago(HOUR));
    // a back-off so long that the source is not asked again meanwhile
    let sources = w.sources_with(
        false,
        ResourceOptions {
            min_backoff: Duration::from_secs(60),
            ..fast()
        },
    );
    sources.track(&w.load());
    w.server.set("/managed", new_managed(&w));
    update(&sources, &w, "/managed")
        .await
        .expect_err("the file has not been tried yet");
    assert!(sources.settle(Duration::from_secs(5)).await);
    assert!(reads(&sources, &w, "/managed", &new_managed(&w)));
    reported_once(&sources).await;
}

/// The moved managed profile and the address it moved to are taken together,
/// and from then on the old address is a stop on the way and nothing more.
#[tokio::test]
async fn a_managed_profile_that_moves_is_taken_with_its_new_address() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    let old = old_managed(&w);
    w.seed("/managed", &old, ago(HOUR));
    let sources = w.sources(false);
    sources.track(&w.load());
    let moved_to =
        |body: &str| format!("#!MANAGED-CONFIG {} interval=3600\n{body}", w.url("/moved"));
    let new = moved_to("[Rule]\nDOMAIN,v2.example,DIRECT\nFINAL,DIRECT\n");
    w.server.set("/managed", new.clone());
    w.server.set(
        "/moved",
        moved_to("[Rule]\nDOMAIN,v3.example,DIRECT\nFINAL,DIRECT\n"),
    );
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("the address it moves to has not been tried");
    assert_eq!(reason, WAITING);
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/managed"), w.key("/moved")]);
    wait_until("both versions", || {
        reads(&sources, &w, "/managed", &new)
            && reads(
                &sources,
                &w,
                "/moved",
                &moved_to("[Rule]\nDOMAIN,v3.example,DIRECT\nFINAL,DIRECT\n"),
            )
    })
    .await;
    reported_once(&sources).await;
    // the generation after it: the old address is a stop on the way
    let next = w.load();
    sources.track(&next);
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/moved")]);
    assert!(
        sources.update(&w.key("/managed")).await.is_none(),
        "the old address is not asked for any more"
    );
    // and the new one is not a change
    stays_quiet(&sources, QUIET + Duration::from_millis(500)).await;
}

/// A reload of the generation in use, while a version waits, does not let go
/// of the sources registered for it.
#[tokio::test]
async fn an_unrelated_track_does_not_drop_the_source_registered_for_the_version() {
    let (w, sources) = the_world_of_x().await;
    w.server.set("/x", X_BODY);
    w.server.set_delay("/x", Duration::from_millis(800));
    update(&sources, &w, "/managed")
        .await
        .expect_err("it waits for the file");
    sources.track(&w.load());
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/managed"), w.key("/x")]);
    wait_until("the new version", || {
        reads(&sources, &w, "/managed", &new_managed(&w))
    })
    .await;
    reported_once(&sources).await;
}

/// The server took the version back before the file it refers to arrived:
/// the version that waited is not what the server gives, and is not taken.
#[tokio::test]
async fn a_version_the_server_took_back_is_not_taken_when_its_file_arrives() {
    let (w, sources) = the_world_of_x().await;
    w.server.set("/x", X_BODY);
    w.server.set_delay("/x", Duration::from_secs(3));
    update(&sources, &w, "/managed")
        .await
        .expect_err("it waits for the file");
    // the old version again: the next attempt finds the copy up to date
    w.server.set("/managed", old_managed(&w));
    wait_until("the old version to be confirmed", || {
        let status = &sources.statuses()[0];
        status.state == "available" && status.last_error.is_none()
    })
    .await;
    // the file arrives: nothing refers to it, and it is let go of
    wait_until("the file to be let go of", || sources.statuses().len() == 1).await;
    assert!(reads(&sources, &w, "/managed", &old_managed(&w)));
    assert_eq!(sources.statuses()[0].state, "available");
    assert!(sources.update(&w.key("/x")).await.is_none());
    // No version moved: the old version was confirmed, and what arrived was
    // let go of. A report that was due would come a quiet second after the
    // move, and 300 ms more is the margin.
    stays_quiet(&sources, QUIET + Duration::from_millis(300)).await;
}

/// A version that waited for a file is replaced by one that does not refer
/// to it: the file is not downloaded for ever.
#[tokio::test]
async fn a_source_only_a_withdrawn_version_named_is_let_go_of() {
    let (w, sources) = the_world_of_x().await;
    w.server.set("/x", X_BODY);
    w.server.set_delay("/x", Duration::from_secs(3));
    update(&sources, &w, "/managed")
        .await
        .expect_err("it waits for the file");
    assert_eq!(sources.statuses().len(), 2);
    let third = managed(&w, "[Rule]\nDOMAIN,third.example,DIRECT\nFINAL,DIRECT\n");
    w.server.set("/managed", third.clone());
    update(&sources, &w, "/managed")
        .await
        .expect("this version names nothing that is missing");
    assert!(reads(&sources, &w, "/managed", &third));
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/managed")]);
    assert!(sources.update(&w.key("/x")).await.is_none());
    reported_once(&sources).await;
}

/// The local profile cannot be read when the version is judged: it is not
/// taken, and the next attempt takes it once the file is back.
#[tokio::test]
async fn a_version_is_not_taken_while_the_local_profile_cannot_be_read() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    let old = old_managed(&w);
    w.seed("/managed", &old, ago(HOUR));
    let sources = w.sources(false);
    sources.track(&w.load());
    let new = managed(&w, "[Rule]\nDOMAIN,new.example,DIRECT\nFINAL,DIRECT\n");
    w.server.set("/managed", new.clone());
    std::fs::remove_file(w.profile()).unwrap();
    std::fs::create_dir(w.profile()).unwrap();
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("the profile cannot be read");
    assert!(reason.starts_with("does not load: cannot read"), "{reason}");
    assert!(reads(&sources, &w, "/managed", &old));
    std::fs::remove_dir(w.profile()).unwrap();
    point_at_managed(&w, "interval=3600");
    wait_until("the version to be taken", || {
        reads(&sources, &w, "/managed", &new)
    })
    .await;
    reported_once(&sources).await;
}

// ---------------------------------------------------------------------------
// strict

/// A managed profile that asks for updates every minute and was checked two
/// hours ago.
async fn a_strict_world() -> (World, Arc<ProfileSources>) {
    let w = World::new().await;
    point_at_managed(&w, "interval=60 strict=true");
    let strict = |w: &World, body: &str| {
        format!(
            "#!MANAGED-CONFIG {} interval=60 strict=true\n{body}",
            w.url("/managed")
        )
    };
    w.seed("/managed", &strict(&w, COPY), ago(2 * HOUR));
    let sources = w.sources(false);
    sources.track(&w.load());
    w.server.set(
        "/managed",
        strict(
            &w,
            &format!("[Rule]\n#!include {}\nFINAL,DIRECT\n", w.url("/x")),
        ),
    );
    (w, sources)
}

/// The update was downloaded, and waits for what it refers to: that is no
/// failure of the update.
#[tokio::test]
async fn a_version_that_waits_is_no_failed_update() {
    let (w, sources) = a_strict_world().await;
    w.server.set("/x", X_BODY);
    w.server.set_delay("/x", Duration::from_secs(3));
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("it waits for the file");
    assert_eq!(reason, WAITING);
    assert_eq!(sources.strict(), None);
}

/// The update was downloaded and does not load: the profile requires an
/// update that did not happen.
#[tokio::test]
async fn a_version_that_does_not_load_is_a_failed_update() {
    let (w, sources) = a_strict_world().await;
    w.server.set("/managed", HTML);
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("the page is refused");
    assert!(reason.starts_with("does not load: "), "{reason}");
    let suspended = sources.strict().expect("the update is overdue and failed");
    assert_eq!(suspended.why, reason);
    assert!(suspended.since < SystemTime::now());
}

// ---------------------------------------------------------------------------
// credentials

/// The address of a source is a credential as a whole: where it goes is all
/// that shows, in the reasons too.
#[tokio::test]
async fn no_reason_and_no_log_line_shows_more_of_an_address_than_where_it_goes() {
    let log = LogCapture::start();
    let w = World::new().await;
    let port = w.url("/").port().unwrap();
    let link = |name: &str| {
        Url::parse(&format!(
            "http://USERMARK:PASSMARK@127.0.0.1:{port}/PATHMARK/{name}.conf?token=QUERYMARK"
        ))
        .unwrap()
    };
    let (managed_url, included_url, new_url) = (link("managed"), link("included"), link("new"));
    let key = |url: &Url| resource_key(&ResourceSource::Url(url.clone()));
    let text = |include: &Url, body: &str| {
        format!(
            "#!MANAGED-CONFIG {managed_url} interval=3600\n[Rule]\n#!include {include}\n{body}FINAL,DIRECT\n"
        )
    };
    w.write(&format!("#!MANAGED-CONFIG {managed_url} interval=3600\n"));
    let old = text(&included_url, "");
    rurge_net::testing::seed_cache(&w.data(), &managed_url, old.as_bytes(), ago(HOUR));
    rurge_net::testing::seed_cache(
        &w.data(),
        &included_url,
        b"[Rule]\nDOMAIN,one.example,DIRECT\n",
        ago(HOUR),
    );
    let sources = w.sources(false);
    sources.track(&w.load());
    let mut shown = String::new();

    // a page where the included file is meant to be
    w.server.set("/PATHMARK/included.conf", HTML);
    shown += &format!("{:?}", sources.update(&key(&included_url)).await);
    // a page where the managed profile is meant to be
    w.server.set("/PATHMARK/managed.conf", HTML);
    shown += &format!("{:?}", sources.update(&key(&managed_url)).await);
    // a version that refers to a file that has not been downloaded
    w.server.set("/PATHMARK/managed.conf", text(&new_url, ""));
    w.server.set("/PATHMARK/new.conf", X_BODY);
    w.server
        .set_delay("/PATHMARK/new.conf", Duration::from_millis(500));
    shown += &format!("{:?}", sources.update(&key(&managed_url)).await);
    shown += &format!("{:?}", sources.statuses());
    // a body that nothing refers to
    w.write("[Rule]\nFINAL,DIRECT\n");
    w.server.set(
        "/PATHMARK/included.conf",
        "[Rule]\nDOMAIN,two.example,DIRECT\n",
    );
    shown += &format!("{:?}", sources.update(&key(&included_url)).await);
    shown += &format!("{:?}", sources.statuses());
    for url in [&managed_url, &included_url, &new_url] {
        shown += &format!("{:?}", sources.remote().get(url));
    }
    let lines = log.lines();
    shown += &lines.join("\n");
    for marker in ["USERMARK", "PASSMARK", "PATHMARK", "QUERYMARK"] {
        assert!(!shown.contains(marker), "{marker} in: {shown}");
    }
    // the reasons were given, and logged
    assert!(
        shown.contains("lacks the section it is included into"),
        "{shown}"
    );
    assert!(shown.contains("does not load: "), "{shown}");
    assert!(shown.contains(WAITING), "{shown}");
    assert!(
        shown.contains("the profile no longer refers to this address"),
        "{shown}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("downloaded resource not adopted")),
        "{lines:?}"
    );
}
