//! What `ProfileSources` does with a body the manager has downloaded before it
//! takes it: the whole profile has to load with it. Real files are loaded, the
//! server is a loopback one, and the cache the downloads land in is a
//! temporary directory.

mod common;

use common::{
    COPY, Engine, EngineShared, HOUR, LogCapture, OutboundMode, QUIET, Runtime, RuntimeOptions,
    World, ago, closed_tcp_port, fast, stack_options, stays_quiet, text_of, wait_until,
};
use rurge_config::codes;
use rurge_config::config::Loaded;
use rurge_config::rule::PolicyRef;
use rurge_config::{Remote, RemoteRef, RemoteRole, SourceInfo};
use rurge_engine::profile::{Boot, BootLimits, ProfileSources, boot_need, disk_remote};
use rurge_net::resource::{ResourceOptions, ResourceSource, resource_key};
use rurge_policy::TerminalKind;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::time::timeout;
use url::Url;

/// What a captive portal or an error page answers.
const HTML: &str =
    "<html><head><title>Sign in</title></head><body>Welcome to the Wi-Fi</body></html>";

/// The included file of the waiting tests.
const X_BODY: &str = "[Rule]\nDOMAIN,x.example,DIRECT\n";

/// An included file whose rule uses a policy that nothing defines: the
/// profile does not load with it.
const BROKEN: &str = "[Rule]\nDOMAIN,a.example,Nowhere\n";

/// How long a file that is not there takes to say so, where the order of
/// things is all that matters.
const SOON: Duration = Duration::from_millis(300);

/// A back-off so long that no source is asked a second time while a test
/// runs, however many times in a row it fails.
fn asked_once() -> ResourceOptions {
    ResourceOptions {
        min_backoff: Duration::from_secs(60),
        max_backoff: Duration::from_secs(60),
        ..fast()
    }
}

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

/// The same page where the named sets of an include under a wildcard section
/// are meant to be: it would bring no set at all, and every rule that names
/// one of them would silently match nothing.
#[tokio::test]
async fn a_page_is_not_taken_as_a_file_included_under_a_wildcard_section() {
    let w = World::new().await;
    w.write(&format!(
        "[Ruleset *]\n#!include {}\n[Rule]\nRULE-SET,Streaming,DIRECT\nFINAL,DIRECT\n",
        w.url("/inc")
    ));
    let old = "[Ruleset Streaming]\nDOMAIN-SUFFIX,netflix.com\n";
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
    // the set the rule names is still there to be matched
    let names: Vec<_> = loaded(&w, &sources)
        .config
        .rulesets
        .iter()
        .map(|set| set.name.clone())
        .collect();
    assert_eq!(names, ["Streaming"]);
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

/// A managed profile and a file it includes, both downloaded, and served as
/// they were downloaded. Their next versions load together and not one
/// without the other. A test serves each when it is to be found: the managed
/// profile is due, and asked for as soon as it is followed.
async fn the_pair() -> (World, Arc<ProfileSources>) {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    w.seed("/managed", &pair_managed(&w, "P1"), ago(HOUR));
    w.seed("/inc", &pair_included("P1"), ago(HOUR));
    w.server.set("/managed", pair_managed(&w, "P1"));
    w.server.set("/inc", pair_included("P1"));
    let sources = w.sources(false);
    sources.track(&w.load());
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
    w.server.set("/managed", pair_managed(&w, "P2"));
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
    w.server.set("/inc", pair_included("P2"));
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
    w.server.set("/inc", pair_included("P2"));
    let reason = update(&sources, &w, "/inc")
        .await
        .expect_err("it does not load by itself");
    assert!(reason.starts_with("does not load: "), "{reason}");
    // the error is in a file that was not downloaded just now
    assert!(!reason.contains("(line"), "{reason}");
    assert!(reads(&sources, &w, "/inc", &pair_included("P1")));
    w.server.set("/managed", pair_managed(&w, "P2"));
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
    w.server.set("/managed", pair_managed(&w, "P2"));
    w.server.set("/inc", pair_included("P2"));
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

/// The source of the other half may not be due for a day. A version that does
/// not load has the sources it is loaded with asked at once: the pair is whole
/// with the one update that was asked for here.
#[tokio::test]
async fn the_other_half_is_asked_for_when_a_version_does_not_load() {
    let (w, sources) = the_pair().await;
    w.server.set("/managed", pair_managed(&w, "P2"));
    w.server.set("/inc", pair_included("P2"));
    update(&sources, &w, "/managed")
        .await
        .expect_err("it does not load by itself");
    // nobody here asks for the included file, and it is not due
    wait_until("the pair", || {
        reads(&sources, &w, "/managed", &pair_managed(&w, "P2"))
            && reads(&sources, &w, "/inc", &pair_included("P2"))
    })
    .await;
    reported_once(&sources).await;
    assert_the_pair_moved(&w, &sources);
}

/// They are asked once, and not at every attempt of the source whose version
/// does not load, nor for the next version that does not.
#[tokio::test]
async fn the_other_sources_are_not_asked_again_within_a_minute() {
    let (w, sources) = the_pair().await;
    w.server.set("/managed", pair_managed(&w, "P2"));
    update(&sources, &w, "/managed")
        .await
        .expect_err("it does not load by itself");
    wait_until("the included file to be asked for", || {
        w.server.hits("/inc") == 1
    })
    .await;
    w.server.set("/managed", pair_managed(&w, "P3"));
    update(&sources, &w, "/managed")
        .await
        .expect_err("neither does this one");
    // The managed profile is asked for again every 50 to 200 ms, and refused
    // each time. A request for the included file would be made within
    // milliseconds of one of those: half a second is several of them.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(w.server.hits("/inc"), 1);
}

/// A page where the managed profile is meant to be names nothing: a captive
/// portal does not have every source of the profile asked.
#[tokio::test]
async fn a_page_does_not_have_the_other_sources_asked() {
    let (w, sources) = the_pair().await;
    w.server.set("/managed", HTML);
    update(&sources, &w, "/managed")
        .await
        .expect_err("the page is refused");
    // A request that was asked for is made within milliseconds: the server
    // is on the loopback. 300 ms is the margin, and the page is refused a
    // few more times meanwhile.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(w.server.hits("/inc"), 0);
}

/// The managed profile of the three: its group uses `policy`, which `/inc`
/// defines, and its rules include `/more`.
fn trio_managed(w: &World, policy: &str) -> String {
    managed(
        w,
        &format!(
            "[Proxy]\n#!include {}\n[Proxy Group]\nG = select, {policy}\n[Rule]\n#!include {}\nFINAL,G\n",
            w.url("/inc"),
            w.url("/more")
        ),
    )
}

/// The rules `/more` was downloaded as.
const MORE: &str = "[Rule]\nDOMAIN,one.example,DIRECT\n";

/// A managed profile and the two files it includes, all downloaded, and
/// served as they were downloaded.
async fn the_trio() -> (World, Arc<ProfileSources>) {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    w.seed("/managed", &trio_managed(&w, "P1"), ago(HOUR));
    w.seed("/inc", &pair_included("P1"), ago(HOUR));
    w.seed("/more", MORE, ago(HOUR));
    w.server.set("/managed", trio_managed(&w, "P1"));
    w.server.set("/inc", pair_included("P1"));
    w.server.set("/more", MORE);
    let sources = w.sources(false);
    sources.track(&w.load());
    (w, sources)
}

/// A version that loads by itself is taken, and the ones that wait for a
/// partner stay where they are.
#[tokio::test]
async fn a_version_that_loads_alone_leaves_the_kept_ones_where_they_are() {
    let (w, sources) = the_trio().await;
    let more = "[Rule]\nDOMAIN,two.example,DIRECT\n";
    w.server.set("/managed", trio_managed(&w, "P2"));
    update(&sources, &w, "/managed")
        .await
        .expect_err("it needs the other file");
    // this one loads with the old versions of the others
    w.server.set("/more", more);
    update(&sources, &w, "/more").await.expect("it loads alone");
    assert!(reads(&sources, &w, "/more", more));
    assert!(reads(&sources, &w, "/managed", &trio_managed(&w, "P1")));
    // and the one that waited is found by its partner
    w.server.set("/inc", pair_included("P2"));
    update(&sources, &w, "/inc")
        .await
        .expect("together they load");
    assert!(reads(&sources, &w, "/managed", &trio_managed(&w, "P2")));
    assert!(reads(&sources, &w, "/inc", &pair_included("P2")));
}

/// A page where an included file is meant to be is no half of a change: it
/// is not kept, and does not stand in the way of the files that are.
#[tokio::test]
async fn a_page_where_a_file_is_included_does_not_stand_in_the_way_of_a_change() {
    let (w, sources) = the_trio().await;
    w.server.set("/more", HTML);
    let reason = update(&sources, &w, "/more")
        .await
        .expect_err("the page is refused");
    assert_eq!(reason, "lacks the section it is included into");
    // the two that change together
    w.server.set("/managed", trio_managed(&w, "P2"));
    update(&sources, &w, "/managed")
        .await
        .expect_err("it needs the other file");
    w.server.set("/inc", pair_included("P2"));
    update(&sources, &w, "/inc")
        .await
        .expect("together they load");
    assert!(reads(&sources, &w, "/managed", &trio_managed(&w, "P2")));
    assert!(reads(&sources, &w, "/inc", &pair_included("P2")));
    assert!(reads(&sources, &w, "/more", MORE));
}

/// What nothing refers to is told so, whatever else is kept and for whatever
/// reason: the reason of the versions that are kept is theirs.
#[tokio::test]
async fn an_address_nothing_refers_to_says_so_beside_a_version_that_does_not_load() {
    let w = World::new().await;
    let local = |more: &str| {
        format!(
            "[Proxy]\n#!include {}\n[Proxy Group]\nG = select, P1\n[Rule]\n{more}FINAL,G\n",
            w.url("/inc")
        )
    };
    w.write(&local(&format!("#!include {}\n", w.url("/more"))));
    w.seed("/inc", &pair_included("P1"), ago(HOUR));
    w.seed("/more", MORE, ago(HOUR));
    // asked for when the other file is kept, and unchanged then
    w.server.set("/more", MORE);
    let sources = w.sources(false);
    sources.track(&w.load());
    // the policy the group uses is gone from the included file: kept
    w.server.set("/inc", pair_included("P2"));
    let kept = update(&sources, &w, "/inc")
        .await
        .expect_err("it does not load by itself");
    assert!(kept.starts_with("does not load: "), "{kept}");
    // the local file stops naming the other file, and nobody has reloaded
    w.write(&local(""));
    w.server.set("/more", "[Rule]\nDOMAIN,two.example,DIRECT\n");
    let reason = update(&sources, &w, "/more")
        .await
        .expect_err("nothing refers to it");
    assert_eq!(reason, "the profile no longer refers to this address");
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

/// Nothing is at `path`, and the server takes `after` to say so: a verdict
/// that names the address is made while its source has not been tried.
fn not_found(w: &World, path: &str, after: Duration) {
    w.server.set(path, "");
    w.server.set_status(path, 404);
    w.server.set_delay(path, after);
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

/// The same under a wildcard section, for a text that hands its sections over
/// to a file of their own: the new version is a wildcard section and an
/// include, and has no section left once that is expanded with nothing to
/// take in. It is a version that waits for what it refers to, not a body that
/// is no profile: refused for a lack, the file would never be asked for and
/// the version never taken.
#[tokio::test]
async fn a_text_of_wildcard_sections_alone_waits_for_the_file_it_includes() {
    let w = World::new().await;
    w.write(&format!(
        "[Ruleset *]\n#!include {}\n[Rule]\nRULE-SET,Streaming,DIRECT\nFINAL,DIRECT\n",
        w.url("/sets")
    ));
    let sets = "[Ruleset Streaming]\nDOMAIN-SUFFIX,netflix.com\n";
    w.seed("/sets", sets, ago(HOUR));
    let sources = w.sources(false);
    sources.track(&w.load());
    let handed_over = format!("[Ruleset *]\n#!include {}\n", w.url("/streaming"));
    w.server.set("/sets", handed_over.clone());
    w.server.set("/streaming", sets);
    w.server.set_delay("/streaming", Duration::from_millis(800));
    let reason = update(&sources, &w, "/sets")
        .await
        .expect_err("it waits for the file");
    assert_eq!(reason, WAITING);
    // the file is followed from now on, after the sources of the profile
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/sets"), w.key("/streaming")]);
    // until it is here the text is the old version
    assert!(reads(&sources, &w, "/sets", sets));
    wait_until("both files", || {
        reads(&sources, &w, "/sets", &handed_over) && reads(&sources, &w, "/streaming", sets)
    })
    .await;
    reported_once(&sources).await;
    let now = loaded(&w, &sources);
    assert!(!now.diagnostics.has_errors(), "{}", said(&now));
    let names: Vec<_> = now
        .config
        .rulesets
        .iter()
        .map(|set| set.name.clone())
        .collect();
    assert_eq!(names, ["Streaming"]);
}

/// A file that cannot be downloaded holds nothing back for long: the version
/// is taken the next time its source is asked, and the line is not expanded.
#[tokio::test]
async fn a_new_include_that_cannot_be_downloaded_does_not_hold_the_version_back() {
    let (w, sources) = the_world_of_x().await;
    not_found(&w, "/x", SOON);
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

/// The same world, with a back-off so long that no source is asked a second
/// time while a test runs.
async fn the_world_of_x_asked_once() -> (World, Arc<ProfileSources>) {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    w.seed("/managed", &old_managed(&w), ago(HOUR));
    let sources = w.sources_with(false, asked_once());
    sources.track(&w.load());
    w.server.set("/managed", new_managed(&w));
    (w, sources)
}

/// It does not wait for the next time its own source is asked: the version is
/// taken when the file has been tried, with the one request that brought it.
#[tokio::test]
async fn a_version_that_waited_is_taken_once_the_file_has_been_tried() {
    let (w, sources) = the_world_of_x_asked_once().await;
    not_found(&w, "/x", SOON);
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("the file has not been tried yet");
    assert_eq!(reason, WAITING);
    wait_until("the new version", || {
        reads(&sources, &w, "/managed", &new_managed(&w))
    })
    .await;
    assert_eq!(w.server.hits("/managed"), 1);
    reported_once(&sources).await;
}

/// The file the new version refers to arrives and is a page. It is refused
/// for what it is, and the version is taken without it: the line is not
/// expanded.
#[tokio::test]
async fn a_new_include_that_is_refused_says_why_and_does_not_hold_the_version_back() {
    let (w, sources) = the_world_of_x_asked_once().await;
    w.server.set("/x", HTML);
    update(&sources, &w, "/managed")
        .await
        .expect_err("the file has not been tried yet");
    wait_until("the new version", || {
        reads(&sources, &w, "/managed", &new_managed(&w))
    })
    .await;
    assert_eq!(w.server.hits("/managed"), 1);
    let status = sources
        .statuses()
        .into_iter()
        .find(|s| s.key == w.key("/x"))
        .expect("the file is followed");
    let why = "lacks the section it is included into";
    assert_eq!(
        (status.state, status.last_error.as_deref()),
        ("failed", Some(why))
    );
    reported_once(&sources).await;
    let loaded = loaded(&w, &sources);
    assert!(!loaded.diagnostics.has_errors());
    assert!(
        loaded
            .diagnostics
            .iter()
            .any(|d| d.code == codes::W_RESOURCE_UNAVAILABLE && d.message.contains(why))
    );
}

/// `settle` returns when the version that waited has been taken.
#[tokio::test]
async fn settling_takes_the_version_that_waited_once_the_file_has_been_tried() {
    let (w, sources) = the_world_of_x_asked_once().await;
    not_found(&w, "/x", SOON);
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

/// A version waits for `/x`, and the server takes it back before `/x` has
/// answered: the version is not what the server gives and is not taken, and
/// `/x`, which nothing refers to, is let go of whatever it answers.
async fn a_version_the_server_took_back_is_not_taken(w: &World, sources: &Arc<ProfileSources>) {
    update(sources, w, "/managed")
        .await
        .expect_err("it waits for the file");
    // the old version again: the next attempt finds the copy up to date
    w.server.set("/managed", old_managed(w));
    wait_until("the old version to be confirmed", || {
        let status = &sources.statuses()[0];
        status.state == "available" && status.last_error.is_none()
    })
    .await;
    // the file has answered: nothing refers to it, and it is let go of
    wait_until("the file to be let go of", || sources.statuses().len() == 1).await;
    assert!(reads(sources, w, "/managed", &old_managed(w)));
    assert_eq!(sources.statuses()[0].state, "available");
    assert!(sources.update(&w.key("/x")).await.is_none());
    // No version moved: the old version was confirmed, and the file was let
    // go of. A report that was due would come a quiet second after the move,
    // and 300 ms more is the margin.
    stays_quiet(sources, QUIET + Duration::from_millis(300)).await;
}

#[tokio::test]
async fn a_version_the_server_took_back_is_not_taken_when_its_file_arrives() {
    let (w, sources) = the_world_of_x().await;
    w.server.set("/x", X_BODY);
    w.server.set_delay("/x", Duration::from_secs(3));
    a_version_the_server_took_back_is_not_taken(&w, &sources).await;
}

/// The file cannot be downloaded: no body arrives, and so no verdict that
/// would find the version gone.
#[tokio::test]
async fn a_version_the_server_took_back_is_let_go_of_when_its_file_cannot_be_downloaded() {
    let (w, sources) = the_world_of_x().await;
    not_found(&w, "/x", Duration::from_millis(1500));
    a_version_the_server_took_back_is_not_taken(&w, &sources).await;
}

/// The copy was downloaded by this process, so the server is asked whether it
/// changed, and says "not modified" when it has taken the version back.
#[tokio::test]
async fn a_version_the_server_took_back_by_saying_not_modified_is_not_taken() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    w.server.set("/managed", old_managed(&w));
    let sources = w.sources(false);
    sources.track(&w.load());
    // the first download, and the generation that reads it
    timeout(Duration::from_secs(4), sources.changed())
        .await
        .expect("the first download is reported");
    sources.track(&w.load());
    w.server.set("/managed", new_managed(&w));
    w.server.set("/x", X_BODY);
    w.server.set_delay("/x", Duration::from_millis(1500));
    a_version_the_server_took_back_is_not_taken(&w, &sources).await;
    // every request after the first asked whether the copy had changed, and
    // the last ones were answered with the copy the process holds
    let conditional: Vec<bool> = w
        .server
        .requests()
        .iter()
        .filter(|r| r.path == "/managed")
        .map(|r| r.header("if-none-match").is_some())
        .collect();
    assert!(
        conditional.len() >= 3 && !conditional[0] && conditional[1..].iter().all(|c| *c),
        "{conditional:?}"
    );
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

/// A version of the managed profile whose rules include `files`, then have
/// `rule`.
fn including(w: &World, files: &[&str], rule: &str) -> String {
    let mut body = String::from("[Rule]\n");
    for file in files {
        body += &format!("#!include {}\n", w.url(file));
    }
    managed(w, &format!("{body}{rule}FINAL,DIRECT\n"))
}

/// The rules `/a` was downloaded as.
const A_BODY: &str = "[Rule]\nDOMAIN,a.example,DIRECT\n";

/// A managed profile that includes `/a`, where a file that does not load is
/// served for `/a`: it was refused, and is kept. No source is tried again of
/// itself while a test runs, a back-off being a minute. `/managed` has been
/// asked twice by now: when it was followed, being due, and on account of the
/// file that was refused.
async fn the_world_with_a_refused_file() -> (World, Arc<ProfileSources>) {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    let first = including(&w, &["/a"], "");
    w.seed("/managed", &first, ago(HOUR));
    w.seed("/a", A_BODY, ago(HOUR));
    w.server.set("/managed", first);
    w.server.set("/a", BROKEN);
    let sources = w.sources_with(false, asked_once());
    sources.track(&w.load());
    let reason = update(&sources, &w, "/a")
        .await
        .expect_err("it does not load");
    assert!(reason.starts_with("does not load: "), "{reason}");
    (w, sources)
}

/// A file that stays refused does not keep the file of a withdrawn version
/// followed: what the loads of a verdict did not name is let go of, whatever
/// else is kept.
#[tokio::test]
async fn a_source_of_a_withdrawn_version_is_let_go_of_while_another_file_is_refused() {
    let (w, sources) = the_world_with_a_refused_file().await;
    // a version that refers to a file that was never downloaded
    w.server.set("/x", X_BODY);
    w.server.set_delay("/x", Duration::from_secs(3));
    w.server.set("/managed", including(&w, &["/a", "/x"], ""));
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("it waits for the file");
    assert_eq!(reason, WAITING);
    assert_eq!(sources.statuses().len(), 3);
    // and the next one, which does not
    let third = including(&w, &["/a"], "DOMAIN,third.example,DIRECT\n");
    w.server.set("/managed", third.clone());
    update(&sources, &w, "/managed")
        .await
        .expect("this version names nothing that is missing");
    assert!(reads(&sources, &w, "/managed", &third));
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/managed"), w.key("/a")]);
    assert!(sources.update(&w.key("/x")).await.is_none());
}

/// Nor does it hold back a version that waited beside it: once the file the
/// version waited for has been tried, the version is judged without the one
/// that does not load.
#[tokio::test]
async fn a_version_that_waited_is_taken_beside_a_file_that_stays_refused() {
    let (w, sources) = the_world_with_a_refused_file().await;
    not_found(&w, "/x", SOON);
    let second = including(&w, &["/a", "/x"], "");
    w.server.set("/managed", second.clone());
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("it waits for the file");
    assert_eq!(reason, WAITING);
    let asked = w.server.hits("/managed");
    wait_until("the new version", || {
        reads(&sources, &w, "/managed", &second)
    })
    .await;
    // by the last look, and not by its source being asked again
    assert_eq!(w.server.hits("/managed"), asked);
    // the file that does not load is not taken with it
    assert!(reads(&sources, &w, "/a", A_BODY));
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
    assert!(
        reason.starts_with("does not load: the local profile cannot be read ("),
        "{reason}"
    );
    // not where the profile is
    assert!(!reason.contains("main.conf"), "{reason}");
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
    a_strict_world_with(fast()).await
}

async fn a_strict_world_with(resources: ResourceOptions) -> (World, Arc<ProfileSources>) {
    let w = World::new().await;
    point_at_managed(&w, "interval=60 strict=true");
    let strict = |w: &World, body: &str| {
        format!(
            "#!MANAGED-CONFIG {} interval=60 strict=true\n{body}",
            w.url("/managed")
        )
    };
    w.seed("/managed", &strict(&w, COPY), ago(2 * HOUR));
    let sources = w.sources_with(false, resources);
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

/// What `strict` leaves out for it, the update that waits is still owed: it
/// is taken when the file it waits for has been tried.
#[tokio::test]
async fn a_version_that_waits_leaves_the_update_due() {
    let (w, sources) = a_strict_world().await;
    w.server.set("/x", X_BODY);
    w.server.set_delay("/x", Duration::from_secs(3));
    update(&sources, &w, "/managed")
        .await
        .expect_err("it waits for the file");
    assert_eq!(sources.strict(), None);
    assert!(sources.strict_due());
}

/// A version that by itself does not load, and waits together with one that
/// was kept, is one that waits.
#[tokio::test]
async fn a_version_that_waits_with_one_that_was_refused_is_no_failed_update() {
    let w = World::new().await;
    point_at_managed(&w, "interval=60 strict=true");
    let text = |policy: &str, more: &str| {
        format!(
            "#!MANAGED-CONFIG {} interval=60 strict=true\n[Proxy]\n#!include {}\n[Proxy Group]\nG = select, {policy}\n[Rule]\n{more}FINAL,G\n",
            w.url("/managed"),
            w.url("/inc")
        )
    };
    w.seed("/managed", &text("P1", ""), ago(2 * HOUR));
    w.seed("/inc", &pair_included("P1"), ago(HOUR));
    let sources = w.sources(false);
    sources.track(&w.load());
    // the included file changes first, and does not load by itself
    w.server.set("/inc", pair_included("P2"));
    update(&sources, &w, "/inc")
        .await
        .expect_err("it does not load by itself");
    // the profile that goes with it refers to a file that was never downloaded
    w.server.set("/x", X_BODY);
    w.server.set_delay("/x", Duration::from_secs(3));
    w.server.set(
        "/managed",
        text("P2", &format!("#!include {}\n", w.url("/x"))),
    );
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("it waits for the file");
    assert_eq!(reason, WAITING);
    assert_eq!(sources.strict(), None);
}

/// The file the version waited for has been tried, and by then the version
/// does not load: it is no longer one that waits, and the update has failed.
#[tokio::test]
async fn a_version_that_waited_and_does_not_load_any_more_is_a_failed_update() {
    // no source is asked a second time: the last look is all that can say so
    let (w, sources) = a_strict_world_with(asked_once()).await;
    not_found(&w, "/x", Duration::from_secs(1));
    let reason = update(&sources, &w, "/managed")
        .await
        .expect_err("it waits for the file");
    assert_eq!(reason, WAITING);
    let asked = w.server.hits("/managed");
    // the local profile is in the way of the last look
    std::fs::remove_file(w.profile()).unwrap();
    std::fs::create_dir(w.profile()).unwrap();
    wait_until("the update to count as failed", || {
        sources.strict().is_some()
    })
    .await;
    assert_eq!(w.server.hits("/managed"), asked);
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
    let log = LogCapture::everything();
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

// ---------------------------------------------------------------------------
// the start

/// The first load of the profile at the start: what the disk has.
fn first_load(w: &World) -> Loaded {
    w.loaded(disk_remote(&w.data()))
}

fn need(w: &World) -> Boot {
    boot_need(&first_load(w), &w.data())
}

/// What the diagnostics of a load say, one to a line.
fn said(loaded: &Loaded) -> String {
    loaded
        .diagnostics
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn not_downloaded(loaded: &Loaded, why: &str) -> bool {
    loaded
        .diagnostics
        .iter()
        .any(|d| d.code == codes::W_RESOURCE_UNAVAILABLE && d.message.contains(why))
}

#[tokio::test]
async fn a_profile_with_no_remote_source_needs_nothing() {
    let w = World::new().await;
    w.write("[Rule]\nFINAL,DIRECT\n");
    assert_eq!(need(&w), Boot::No);
}

#[tokio::test]
async fn a_source_without_content_is_waited_for() {
    let w = World::new().await;
    w.write_including(&["/inc"]);
    assert_eq!(need(&w), Boot::Wait);
    w.seed("/inc", COPY, ago(HOUR));
    assert_eq!(need(&w), Boot::No);
}

#[tokio::test]
async fn a_managed_profile_without_a_copy_is_waited_for() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    assert_eq!(need(&w), Boot::Wait);
}

#[tokio::test]
async fn a_strict_update_that_is_due_is_waited_for() {
    let w = World::new().await;
    point_at_managed(&w, "interval=60 strict=true");
    let strict = format!(
        "#!MANAGED-CONFIG {} interval=60 strict=true\n{COPY}",
        w.url("/managed")
    );
    w.seed("/managed", &strict, ago(2 * HOUR));
    assert_eq!(need(&w), Boot::Wait);
    // the interval is not over yet
    w.seed("/managed", &strict, ago(Duration::from_secs(10)));
    assert_eq!(need(&w), Boot::No);
    // an update that is due and not `strict` is made in the background
    point_at_managed(&w, "interval=60");
    let lax = format!("#!MANAGED-CONFIG {} interval=60\n{COPY}", w.url("/managed"));
    w.seed("/managed", &lax, ago(2 * HOUR));
    assert_eq!(need(&w), Boot::No);
}

/// What is wrong in the local file is for the user to mend; no update
/// replaces it.
#[tokio::test]
async fn an_error_in_the_local_file_needs_nothing() {
    let w = World::new().await;
    w.seed("/inc", COPY, ago(HOUR));
    w.write(&format!(
        "[Rule]\n#!include {}\nDOMAIN,x.example,Nowhere\nFINAL,DIRECT\n",
        w.url("/inc")
    ));
    let loaded = first_load(&w);
    assert!(loaded.diagnostics.has_errors());
    assert_eq!(boot_need(&loaded, &w.data()), Boot::No);
}

/// A copy that no longer loads is replaced by an update, if the server has
/// one that does.
#[tokio::test]
async fn an_error_in_the_downloaded_copy_needs_a_refresh() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    w.seed(
        "/managed",
        &managed(&w, "[Rule]\nDOMAIN,x.example,Nowhere\nFINAL,DIRECT\n"),
        ago(HOUR),
    );
    let loaded = first_load(&w);
    assert!(loaded.diagnostics.has_errors());
    assert_eq!(boot_need(&loaded, &w.data()), Boot::Refresh);
}

#[tokio::test]
async fn an_error_in_a_cached_include_needs_a_refresh() {
    let w = World::new().await;
    // the other include has no content: the refresh is the more urgent need
    w.write_including(&["/inc", "/other"]);
    w.seed("/inc", "[Rule]\nDOMAIN,x.example,Nowhere\n", ago(HOUR));
    let loaded = first_load(&w);
    assert!(loaded.diagnostics.has_errors());
    assert_eq!(boot_need(&loaded, &w.data()), Boot::Refresh);
}

#[tokio::test]
async fn nothing_is_followed_when_nothing_is_needed() {
    let w = World::new().await;
    w.seed("/inc", COPY, ago(HOUR));
    w.write_including(&["/inc"]);
    w.server.set("/inc", "[Rule]\nDOMAIN,new.example,DIRECT\n");
    let sources = w.sources(false);
    let first = first_load(&w);
    let loaded = sources
        .bootstrap(first, Boot::No, BootLimits::default())
        .await
        .unwrap();
    assert!(!loaded.diagnostics.has_errors());
    assert!(sources.statuses().is_empty());
    assert!(w.server.requests().is_empty());
}

/// A managed profile that includes two files, one of which includes a third:
/// the start leaves with all four in the cache.
#[tokio::test]
async fn an_empty_cache_is_filled_with_everything_the_profile_leads_to() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    let rule = |name: &str| format!("DOMAIN,{name}.example,DIRECT\n");
    w.server.set(
        "/managed",
        managed(
            &w,
            &format!(
                "[Rule]\n#!include {}\n#!include {}\nFINAL,DIRECT\n",
                w.url("/a"),
                w.url("/b")
            ),
        ),
    );
    w.server.set(
        "/a",
        format!("[Rule]\n#!include {}\n{}", w.url("/c"), rule("a")),
    );
    w.server.set("/b", format!("[Rule]\n{}", rule("b")));
    w.server.set("/c", format!("[Rule]\n{}", rule("c")));
    let first = first_load(&w);
    assert!(first.diagnostics.has_errors());
    let need = boot_need(&first, &w.data());
    assert_eq!(need, Boot::Wait);
    let sources = w.sources(false);
    let loaded = sources
        .bootstrap(first, need, BootLimits::default())
        .await
        .unwrap();
    assert!(!loaded.diagnostics.has_errors(), "{}", said(&loaded));
    assert_eq!(loaded.config.source.remote.len(), 4);
    assert!(loaded.config.source.remote.iter().all(|r| r.found()));
    for path in ["/managed", "/a", "/b", "/c"] {
        assert!(
            matches!(
                disk_remote(&w.data()).get(&w.url(path)),
                Remote::Text { .. }
            ),
            "{path}"
        );
    }
    let raws: Vec<_> = loaded.config.rules.iter().map(|r| r.raw.as_str()).collect();
    for domain in ["a", "b", "c"] {
        assert!(raws.contains(&format!("DOMAIN,{domain}.example,DIRECT").as_str()));
    }
}

/// A server that fails ends the start after one round, whatever the limits
/// are; the load that comes back says why.
#[tokio::test]
async fn a_server_that_answers_500_ends_the_start_after_one_round() {
    let log = LogCapture::everything();
    let w = World::new().await;
    let port = w.url("/").port().unwrap();
    let include =
        format!("http://USERMARK:PASSMARK@127.0.0.1:{port}/PATHMARK/inc.conf?token=QUERYMARK");
    w.server.set("/PATHMARK/inc.conf", "[Rule]\nFINAL,DIRECT\n");
    w.server.set_status("/PATHMARK/inc.conf", 500);
    // without the include the profile has no FINAL: an error in the local file
    w.write(&format!("[Rule]\n#!include {include}\n"));
    let first = first_load(&w);
    assert!(first.diagnostics.has_errors());
    let need = boot_need(&first, &w.data());
    assert_eq!(need, Boot::Wait);
    let sources = w.sources(false);
    let limits = BootLimits {
        round: Duration::from_secs(60),
        total: Duration::from_secs(120),
        rounds: 8,
    };
    let begun = Instant::now();
    let loaded = sources.bootstrap(first, need, limits).await.unwrap();
    assert!(
        begun.elapsed() < Duration::from_secs(10),
        "ended after {:?}",
        begun.elapsed()
    );
    assert!(loaded.diagnostics.has_errors());
    assert!(
        not_downloaded(&loaded, "http status 500"),
        "{}",
        said(&loaded)
    );
    assert!(w.server.hits("/PATHMARK/inc.conf") >= 1);
    // nothing of the address but where it goes
    let mut shown = said(&loaded);
    shown += &format!("{:?}", sources.statuses());
    shown += &log.lines().join("\n");
    for marker in ["USERMARK", "PASSMARK", "PATHMARK", "QUERYMARK"] {
        assert!(!shown.contains(marker), "{marker} in: {shown}");
    }
}

/// A source that has not answered after a round, and nothing else that moved:
/// another round would wait for the same thing.
#[tokio::test]
async fn a_round_in_which_nothing_moved_ends_the_start() {
    let w = World::new().await;
    w.server.set("/slow", COPY);
    w.server.set_delay("/slow", Duration::from_secs(10));
    w.write(&format!("[Rule]\n#!include {}\n", w.url("/slow")));
    let first = first_load(&w);
    let sources = w.sources(false);
    let limits = BootLimits {
        round: Duration::from_millis(300),
        total: Duration::from_secs(60),
        rounds: 8,
    };
    let begun = Instant::now();
    let loaded = sources.bootstrap(first, Boot::Wait, limits).await.unwrap();
    // one round; eight of them would take 2.4 seconds
    assert!(
        begun.elapsed() >= Duration::from_millis(250)
            && begun.elapsed() < Duration::from_millis(1500),
        "ended after {:?}",
        begun.elapsed()
    );
    assert!(loaded.diagnostics.has_errors());
}

/// No round waits longer than what is left of the whole.
#[tokio::test]
async fn the_whole_start_is_over_when_its_time_is() {
    let w = World::new().await;
    w.server.set("/slow", COPY);
    w.server.set_delay("/slow", Duration::from_secs(10));
    w.write(&format!("[Rule]\n#!include {}\n", w.url("/slow")));
    let first = first_load(&w);
    let sources = w.sources(false);
    let limits = BootLimits {
        round: Duration::from_secs(60),
        total: Duration::from_millis(400),
        rounds: 8,
    };
    let begun = Instant::now();
    sources.bootstrap(first, Boot::Wait, limits).await.unwrap();
    assert!(
        begun.elapsed() >= Duration::from_millis(350) && begun.elapsed() < Duration::from_secs(3),
        "ended after {:?}",
        begun.elapsed()
    );
}

#[tokio::test]
async fn a_file_that_takes_two_seconds_is_waited_for() {
    let w = World::new().await;
    w.server.set("/inc", "[Rule]\nFINAL,DIRECT\n");
    w.server.set_delay("/inc", Duration::from_secs(2));
    w.write(&format!("[Rule]\n#!include {}\n", w.url("/inc")));
    let first = first_load(&w);
    let need = boot_need(&first, &w.data());
    assert_eq!(need, Boot::Wait);
    let sources = w.sources(false);
    let begun = Instant::now();
    let loaded = sources
        .bootstrap(first, need, BootLimits::default())
        .await
        .unwrap();
    assert!(
        begun.elapsed() >= Duration::from_millis(1900),
        "ended after {:?}",
        begun.elapsed()
    );
    assert!(!loaded.diagnostics.has_errors(), "{}", said(&loaded));
    assert!(reads(&sources, &w, "/inc", "[Rule]\nFINAL,DIRECT\n"));
}

/// The start is the one place that waits for a version that refers to a file
/// that cannot be downloaded: the file has been tried when it is over, and the
/// version is taken with the line not expanded.
#[tokio::test]
async fn the_start_takes_a_version_that_waited_for_a_file_that_cannot_be_downloaded() {
    let w = World::new().await;
    point_at_managed(&w, "interval=60 strict=true");
    let strict = |body: &str| {
        format!(
            "#!MANAGED-CONFIG {} interval=60 strict=true\n{body}",
            w.url("/managed")
        )
    };
    w.seed("/managed", &strict(COPY), ago(2 * HOUR));
    let new = strict(&format!(
        "[Rule]\n#!include {}\nFINAL,DIRECT\n",
        w.url("/x")
    ));
    w.server.set("/managed", new.clone());
    not_found(&w, "/x", SOON);
    let first = first_load(&w);
    let need = boot_need(&first, &w.data());
    assert_eq!(need, Boot::Wait);
    // a back-off so long that its source being asked again cannot be what
    // takes the version
    let sources = w.sources_with(false, asked_once());
    let loaded = sources
        .bootstrap(first, need, BootLimits::default())
        .await
        .unwrap();
    assert!(reads(&sources, &w, "/managed", &new));
    assert!(!loaded.diagnostics.has_errors(), "{}", said(&loaded));
    assert!(
        not_downloaded(&loaded, "http status 404"),
        "{}",
        said(&loaded)
    );
}

/// A copy that no longer loads is replaced by one that does.
#[tokio::test]
async fn a_copy_that_does_not_load_is_replaced_by_one_that_does() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    w.seed(
        "/managed",
        &managed(&w, "[Rule]\nDOMAIN,x.example,Nowhere\nFINAL,DIRECT\n"),
        ago(HOUR),
    );
    let good = managed(&w, COPY);
    w.server.set("/managed", good.clone());
    let first = first_load(&w);
    let need = boot_need(&first, &w.data());
    assert_eq!(need, Boot::Refresh);
    let sources = w.sources(false);
    let loaded = sources
        .bootstrap(first, need, BootLimits::default())
        .await
        .unwrap();
    assert!(!loaded.diagnostics.has_errors(), "{}", said(&loaded));
    assert!(reads(&sources, &w, "/managed", &good));
}

/// The server has nothing better than the copy that does not load: the start
/// leaves with its error, after asking once.
#[tokio::test]
async fn a_copy_that_does_not_load_and_is_served_again_ends_the_start_with_its_error() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    let broken = managed(&w, "[Rule]\nDOMAIN,x.example,Nowhere\nFINAL,DIRECT\n");
    w.seed("/managed", &broken, ago(HOUR));
    w.server.set("/managed", broken.clone());
    let first = first_load(&w);
    let need = boot_need(&first, &w.data());
    assert_eq!(need, Boot::Refresh);
    let sources = w.sources(false);
    let begun = Instant::now();
    let loaded = sources
        .bootstrap(first, need, BootLimits::default())
        .await
        .unwrap();
    assert!(begun.elapsed() < Duration::from_secs(10));
    assert!(loaded.diagnostics.has_errors());
    assert!(reads(&sources, &w, "/managed", &broken));
    assert_eq!(w.server.hits("/managed"), 1);
}

/// The update that fixes the copy refers to a file that has never been
/// downloaded: the round that finds that out is not the last one.
#[tokio::test]
async fn a_refresh_that_needs_a_new_file_gets_it_in_the_next_round() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    w.seed(
        "/managed",
        &managed(&w, "[Rule]\nDOMAIN,x.example,Nowhere\nFINAL,DIRECT\n"),
        ago(HOUR),
    );
    let fixed = managed(
        &w,
        &format!("[Rule]\n#!include {}\nFINAL,DIRECT\n", w.url("/x")),
    );
    w.server.set("/managed", fixed.clone());
    w.server.set("/x", X_BODY);
    w.server.set_delay("/x", Duration::from_millis(300));
    let first = first_load(&w);
    let need = boot_need(&first, &w.data());
    assert_eq!(need, Boot::Refresh);
    let sources = w.sources(false);
    let loaded = sources
        .bootstrap(first, need, BootLimits::default())
        .await
        .unwrap();
    assert!(!loaded.diagnostics.has_errors(), "{}", said(&loaded));
    assert!(reads(&sources, &w, "/managed", &fixed));
    assert!(reads(&sources, &w, "/x", X_BODY));
}

// ---------------------------------------------------------------------------
// what the downloaded copy names on this machine

/// The downloaded copy of a managed profile is the profile while it is in
/// effect, and it is downloaded text all the same: nothing on this machine is
/// built from what it names. The proxies of the local file it includes are
/// not policies, nor members of its group; the rules of the local set it names
/// are not its rules; its program is not started. Of these the set and the
/// program were refused before the include was.
#[tokio::test]
async fn the_copy_of_a_managed_profile_builds_nothing_from_a_local_file() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    // nothing listens at the port of the proxy, which no test dials
    let port = closed_tcp_port();
    std::fs::write(
        w.dir.path().join("other.conf"),
        format!("[Proxy]\nElsewhere = http, 127.0.0.1, {port}\n"),
    )
    .unwrap();
    std::fs::write(
        w.dir.path().join("local.list"),
        "DOMAIN,local-set.example\n",
    )
    .unwrap();
    w.seed(
        "/managed",
        &managed(
            &w,
            &format!(
                "[Proxy]\n#!include other.conf\nOwn = external, exec=/bin/true, local-port={port}\n[Proxy Group]\nAll = select, include-all-proxies=true\n[Rule]\nRULE-SET,local.list,REJECT\nFINAL,All\n"
            ),
        ),
        ago(HOUR),
    );
    let Loaded {
        config,
        diagnostics,
    } = w.loaded(disk_remote(&w.data()));
    assert_ne!(
        config.source.main_text, config.source.main,
        "the copy is in effect"
    );
    let engine = Engine::new(
        Runtime::build(
            config,
            RuntimeOptions {
                stack: stack_options(&w.data()),
                outbound_mode: OutboundMode::Rule,
                idle_timeout: Duration::from_secs(600),
                shared: EngineShared::default(),
                request_log_size: 1000,
            },
        )
        .await
        .unwrap(),
    );

    // what is built: the copy's own policy, and not the one of the file
    let view = engine.policies_view();
    assert!(
        view.proxies.contains(&"Own".to_string()),
        "{:?}",
        view.proxies
    );
    assert!(
        !view.proxies.contains(&"Elsewhere".to_string()),
        "{:?}",
        view.proxies
    );
    let groups = engine.groups_view();
    let all = groups.iter().find(|g| g.name == "All").expect("the group");
    let members: Vec<&str> = all.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(members, ["Own"]);

    // the program is the kind that behaves as REJECT: it is not started
    let own = engine.registry().resolve(&PolicyRef::parse("Own"));
    assert_eq!(own.terminal, TerminalKind::Reject);
    assert_eq!(
        own.note.map(|note| note.to_string()).as_deref(),
        Some("policy protocol not implemented: external (managed profile)")
    );
    // and nothing of the local set is in the rules
    let rules: Vec<String> = engine
        .rules_view()
        .into_iter()
        .map(|rule| rule.rule)
        .collect();
    assert_eq!(rules, ["FINAL,All"]);

    // the load said so, at the lines of the copy: the include, the program
    // and the set
    assert!(!diagnostics.has_errors());
    let mut refused: Vec<u32> = diagnostics
        .iter()
        .filter(|d| d.code == codes::W_REMOTE_LOCAL_ACCESS)
        .map(|d| d.span.as_ref().expect("a position").line)
        .collect();
    refused.sort();
    assert_eq!(refused, [3, 4, 8]);
}

// ---------------------------------------------------------------------------
// sources a load that failed came across, and nobody has downloaded

const MINUTE: Duration = Duration::from_secs(60);

/// A generation in use that follows a pointer to `/managed`, whose copy was
/// checked a minute ago, so that nothing is due. The local file is then
/// rewritten as a pointer to `/second`, with `line` after the address, which
/// nobody has downloaded.
struct Moved {
    w: World,
    sources: Arc<ProfileSources>,
    /// The sources of the generation in use.
    in_use: SourceInfo,
    /// The sources the load of the rewritten file came across. That load has
    /// an error: the profile is nothing without the copy.
    failed: SourceInfo,
}

async fn a_pointer_that_moved(line: &str, resources: ResourceOptions) -> Moved {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    w.seed("/managed", &managed(&w, COPY), ago(MINUTE));
    let sources = w.sources_with(false, resources);
    let in_use = w.load();
    sources.track(&in_use);
    w.write(&format!("#!MANAGED-CONFIG {} {line}\n", w.url("/second")));
    let load = loaded(&w, &sources);
    assert!(load.diagnostics.has_errors(), "{}", said(&load));
    Moved {
        failed: load.config.source,
        w,
        sources,
        in_use,
    }
}

/// What `/second` serves: a managed profile that names itself.
fn second_managed(w: &World) -> String {
    w.managed_copy("/second", "interval=3600")
}

/// The user rewrites the link of the managed profile while rurge runs, to an
/// address nobody has downloaded. The load of the file fails for want of the
/// copy, and nothing else would ever ask for it: the sources that load came
/// across are followed, and the copy is reported when it is here.
#[tokio::test]
async fn the_managed_profile_a_failed_load_names_is_downloaded_and_reported() {
    let Moved {
        w, sources, failed, ..
    } = a_pointer_that_moved("interval=3600", asked_once()).await;
    let body = second_managed(&w);
    w.server.set("/second", body.clone());
    // Until the sources are followed nobody asks for the address and nothing
    // moves: a report that was due would come a quiet second after the move,
    // and 300 ms more is the margin.
    stays_quiet(&sources, QUIET + Duration::from_millis(300)).await;
    assert_eq!(w.server.hits("/second"), 0);

    assert_eq!(sources.follow_missing(&failed), 1);
    reported_once(&sources).await;
    assert_eq!(w.server.hits("/second"), 1);
    assert!(reads(&sources, &w, "/second", &body));
    let now = loaded(&w, &sources);
    assert!(!now.diagnostics.has_errors(), "{}", said(&now));
}

/// The same for a file the profile includes, and a profile that has no rule
/// without it.
#[tokio::test]
async fn the_include_a_failed_load_names_is_downloaded_and_reported() {
    let w = World::new().await;
    let including = |path: &str| format!("[Rule]\n#!include {}\n", w.url(path));
    let body = |name: &str| format!("[Rule]\nDOMAIN,{name}.example,DIRECT\nFINAL,DIRECT\n");
    w.write(&including("/inc"));
    w.seed("/inc", &body("inc"), ago(MINUTE));
    let sources = w.sources_with(false, asked_once());
    sources.track(&w.load());
    assert!(!loaded(&w, &sources).diagnostics.has_errors());
    // the address is changed to one nobody has downloaded
    w.write(&including("/inc2"));
    w.server.set("/inc2", body("inc2"));
    let failed = loaded(&w, &sources);
    assert!(failed.diagnostics.has_errors(), "{}", said(&failed));
    assert_eq!(w.server.hits("/inc2"), 0);

    assert_eq!(sources.follow_missing(&failed.config.source), 1);
    reported_once(&sources).await;
    assert_eq!(w.server.hits("/inc2"), 1);
    assert!(reads(&sources, &w, "/inc2", &body("inc2")));
    let now = loaded(&w, &sources);
    assert!(!now.diagnostics.has_errors(), "{}", said(&now));
    assert!(
        now.config
            .rules
            .iter()
            .any(|r| r.raw == "DOMAIN,inc2.example,DIRECT")
    );
}

/// A source followed for a load that failed suspends nothing, whatever the
/// link says of `strict`: the generation in use goes on as it was. Its update
/// is due at once and fails; for a source of the generation that would be a
/// refusal.
#[tokio::test]
async fn a_source_followed_for_a_failed_load_suspends_nothing() {
    let Moved {
        w, sources, failed, ..
    } = a_pointer_that_moved("interval=3600 strict=true", fast()).await;
    // the file was last changed two hours ago
    w.touch(ago(2 * HOUR));
    not_found(&w, "/second", SOON);
    assert_eq!(sources.strict(), None);
    assert_eq!(sources.follow_missing(&failed), 1);
    // the attempt is on its way, and then it has failed
    assert_eq!(sources.strict(), None);
    wait_until("the attempt to fail", || {
        sources
            .statuses()
            .iter()
            .any(|s| s.key == w.key("/second") && s.state == "failed")
    })
    .await;
    assert_eq!(sources.strict(), None);
    assert!(!sources.strict_due());
}

/// Following the same sources again registers nothing, and the listing has
/// each address once. A source of the generation in use that has no copy is
/// followed already.
#[tokio::test]
async fn following_the_same_sources_twice_registers_them_once() {
    let Moved {
        w, sources, failed, ..
    } = a_pointer_that_moved("interval=3600", asked_once()).await;
    w.server.set("/second", second_managed(&w));
    assert_eq!(sources.follow_missing(&failed), 1);
    assert_eq!(sources.follow_missing(&failed), 0);
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/managed"), w.key("/second")]);

    let w = World::new().await;
    w.write_including(&["/x"]);
    not_found(&w, "/x", SOON);
    let sources = w.sources_with(false, asked_once());
    let in_use = w.load();
    sources.track(&in_use);
    assert!(in_use.remote.iter().any(|r| !r.found()));
    assert_eq!(sources.follow_missing(&in_use), 0);
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/x")]);
}

/// The generation that comes next decides: it keeps a source it names as it
/// keeps any, and lets go of one it does not name.
#[tokio::test]
async fn the_next_generation_decides_whether_a_followed_source_stays() {
    // it names the address: the source stays, and the server is not asked again
    let Moved {
        w, sources, failed, ..
    } = a_pointer_that_moved("interval=3600", asked_once()).await;
    let body = second_managed(&w);
    w.server.set("/second", body.clone());
    assert_eq!(sources.follow_missing(&failed), 1);
    wait_until("the copy", || reads(&sources, &w, "/second", &body)).await;
    reported_once(&sources).await;
    let next = loaded(&w, &sources).config.source;
    sources.track(&next);
    sources.track(&next);
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/second")]);
    // no move: a report that was due would come a quiet second after it, and
    // 300 ms more is the margin
    stays_quiet(&sources, QUIET + Duration::from_millis(300)).await;
    assert_eq!(w.server.hits("/second"), 1);

    // it does not name it: the source is let go of
    let Moved {
        w,
        sources,
        in_use,
        failed,
    } = a_pointer_that_moved("interval=3600", asked_once()).await;
    w.server.set("/second", second_managed(&w));
    assert_eq!(sources.follow_missing(&failed), 1);
    sources.track(&in_use);
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/managed")]);
    assert!(sources.update(&w.key("/second")).await.is_none());
}

/// What `outdated` and `strict_due` say of the generation in use is what they
/// said before a source was followed, and what they say once a check of its
/// own has succeeded: the source that was followed comes after its sources,
/// and plays no part.
#[tokio::test]
async fn following_a_source_leaves_the_generation_in_use_as_it_was() {
    let w = World::new().await;
    // The file has sections of its own and was changed after the copy was
    // last checked, so it is in effect; the update it asks for is long due.
    w.write_managed("/managed", "interval=3600 strict=true");
    w.touch(ago(HOUR / 2));
    let copy = w.managed_copy("/managed", "interval=3600 strict=true");
    w.seed("/managed", &copy, ago(2 * HOUR));
    let sources = w.sources(false);
    sources.track(&w.load());
    wait_until("the update to fail", || {
        sources.statuses()[0].state == "failed"
    })
    .await;
    let before = (sources.outdated(), sources.strict_due());
    assert_eq!(before, (false, true));

    let elsewhere = SourceInfo {
        main: w.profile(),
        includes: Vec::new(),
        main_text: w.profile(),
        remote: vec![RemoteRef {
            url: w.url("/second"),
            role: RemoteRole::Managed,
            path: None,
            interval: HOUR,
            strict: true,
            lacks_section: false,
        }],
    };
    assert_eq!(sources.follow_missing(&elsewhere), 1);
    assert_eq!((sources.outdated(), sources.strict_due()), before);

    // a check of the source of the generation succeeds: the copy takes over
    // from the file, once
    w.server.set("/managed", copy);
    update(&sources, &w, "/managed")
        .await
        .expect("the check succeeds");
    assert!(sources.outdated());
    assert!(!sources.outdated());
    assert!(!sources.strict_due());
}

/// Only a source nobody has downloaded is followed: not one that has a copy,
/// and not a stop on the way to the managed profile, which is read from its
/// copy and never asked for.
#[tokio::test]
async fn only_a_source_without_a_copy_is_followed() {
    let w = World::new().await;
    let sources = w.sources_with(false, asked_once());
    let remote = |path: &str, role: RemoteRole, copy: bool| RemoteRef {
        url: w.url(path),
        role,
        path: copy.then(|| w.dir.path().join("copy")),
        interval: HOUR,
        strict: false,
        lacks_section: false,
    };
    let info = SourceInfo {
        main: w.profile(),
        includes: Vec::new(),
        main_text: w.profile(),
        remote: vec![
            remote("/has-a-copy", RemoteRole::Include, true),
            remote("/on-the-way", RemoteRole::ManagedHop, false),
            remote("/never", RemoteRole::Include, false),
        ],
    };
    assert_eq!(sources.follow_missing(&info), 1);
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/never")]);
}

/// A process that is offline downloads nothing, so it follows nothing: the
/// listing stays what it was, and nobody is told that a copy is on its way.
#[tokio::test]
async fn an_offline_process_follows_nothing() {
    let w = World::new().await;
    point_at_managed(&w, "interval=3600");
    let sources = w.sources(true);
    sources.track(&w.load());
    w.write(&format!(
        "#!MANAGED-CONFIG {} interval=3600\n",
        w.url("/second")
    ));
    let failed = loaded(&w, &sources);
    assert!(failed.diagnostics.has_errors(), "{}", said(&failed));
    assert_eq!(sources.follow_missing(&failed.config.source), 0);
    let keys: Vec<_> = sources.statuses().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, [w.key("/managed")]);
    assert!(w.server.requests().is_empty());
}
