//! The external resources through the engine: what is listed and in which
//! order, and what an update of one key or of all of them comes to. Two
//! holders keep them, the sources of the profile and the generation in use,
//! and one address can be a source in both. Real files and a loopback server;
//! the generation downloads, with its GeoIP databases asked of that server,
//! which has none.

mod common;

use common::*;
use rurge_config::{RemoteRef, RemoteRole, SourceInfo};
use rurge_engine::profile::ProfileSources;
use rurge_engine::{ExternalResource, UnusedLines};
use rurge_net::resource::{
    ResourceKind, ResourceManager, ResourceSource, ResourceSpec, cache_file, resource_key,
};
use rurge_net::testing::seed_cache;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};
use url::Url;

const RULES: &str = "DOMAIN,rules.example\n";
const DOMAINS: &str = "domains.example\n";
const NODES: &str = "N1 = http, n1.test, 80\n";
const LOCAL: &str = "DOMAIN,local.example\n";
/// The file the profile includes, which is also a rule set where a test says so.
const INCLUDED: &str = "[Rule]\nDOMAIN,included.example,DIRECT\n";

/// What a captive portal or an error page answers.
const HTML: &str =
    "<html><head><title>Sign in</title></head><body>Welcome to the Wi-Fi</body></html>";

/// Why an update of a profile is not taken while a source it refers to has not
/// been tried.
const WAITING: &str = "waiting for the sources this version refers to";

/// What a manager that is offline says of an update.
const OFFLINE: &str = "offline mode: nothing is downloaded";

/// A token that the link to a subscription carries.
const TOKEN: &str = "S3CR3T";

/// What the server has of the rule set once it has changed.
const NEWER: &str = "DOMAIN,newer.example\n";

/// The longest a request of these tests is at a loopback server after it was
/// asked for, with a wide margin: what a test that says "nothing more is
/// asked for" waits before it counts.
const NOTHING_MORE: Duration = Duration::from_millis(300);

/// How long the server takes over a download that is to be under way while
/// the generations change. The change follows the request by milliseconds;
/// the tests that use it say so themselves if a download landed first.
const UNDER_WAY: Duration = Duration::from_secs(2);

const LIMIT: Duration = Duration::from_secs(10);

fn at(age_in_seconds: u64) -> SystemTime {
    ago(Duration::from_secs(age_in_seconds))
}

fn secs(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The sets, the subscription and the hosts file of a generation, as the files
/// of the data directory `dir` hold them and as the server of `w` gives them.
/// The copies were checked ten, twenty and five minutes ago and a set is
/// updated daily, so nothing is due when the generation is built.
struct Seeded {
    /// When the copies were checked, in UNIX seconds.
    rules: u64,
    domains: u64,
    nodes: u64,
    hosts: PathBuf,
}

fn seed_generation(w: &World, dir: &Path) -> Seeded {
    let (rules, domains, nodes) = (at(600), at(1200), at(300));
    for (path, body, checked) in [
        ("/rules.list", RULES, rules),
        ("/domains.txt", DOMAINS, domains),
    ] {
        w.server.set(path, body);
        seed_cache(dir, &w.url(path), body.as_bytes(), checked);
    }
    // the route has no query: the server tells paths apart, not tokens
    w.server.set("/nodes", NODES);
    seed_cache(dir, &subscription(w), NODES.as_bytes(), nodes);
    std::fs::write(dir.join("local.list"), LOCAL).unwrap();
    let hosts = dir.join("hosts");
    std::fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();
    Seeded {
        rules: secs(rules),
        domains: secs(domains),
        nodes: secs(nodes),
        hosts,
    }
}

/// The link of the subscription, which carries a token.
fn subscription(w: &World) -> Url {
    w.url(&format!("/nodes?token={TOKEN}"))
}

/// The profile of a generation: a rule set and a domain set by address, a rule
/// set by file, a subscription, and `extra_rules`.
async fn profile_text(w: &World, extra_rules: &str) -> (String, MockDns) {
    let dns = MockDns::spawn().await;
    let rules = format!(
        "RULE-SET,{},DIRECT\nRULE-SET,local.list,DIRECT\nDOMAIN-SET,{},DIRECT\n{extra_rules}",
        w.url("/rules.list"),
        w.url("/domains.txt")
    );
    let groups = format!("Sub = select, policy-path={}", subscription(w));
    let text = Profile {
        groups: &groups,
        rules: &rules,
        ..Profile::default()
    }
    .text(dns.addr());
    (text, dns)
}

struct Generation {
    engine: Arc<Engine>,
    seeded: Seeded,
    /// The profile it was built from.
    text: String,
    /// The data directory, with the profile and the files of the sets.
    dir: tempfile::TempDir,
    _dns: MockDns,
}

/// What the profile of a generation that downloads cannot say: the hosts file
/// it watches, and that its first downloads are waited for.
fn watching(hosts: PathBuf) -> impl FnOnce(&mut StackOptions) {
    move |stack| {
        stack.wait = Duration::from_secs(5);
        stack.system = Arc::new(StaticSystemDns {
            hosts_path: Some(hosts),
            ..StaticSystemDns::default()
        });
    }
}

/// A generation that downloads and watches a hosts file. It does not return
/// before every first download has been tried: the GeoIP databases, which the
/// server does not have, have failed by then.
async fn generation(w: &World, extra_rules: &str) -> Generation {
    let dir = tempfile::tempdir().unwrap();
    let seeded = seed_generation(w, dir.path());
    let (text, dns) = profile_text(w, extra_rules).await;
    let runtime = online_runtime_with(
        dir.path(),
        &text,
        &w.server,
        EngineShared::default(),
        watching(seeded.hosts.clone()),
    )
    .await;
    Generation {
        engine: Engine::new(runtime),
        seeded,
        text,
        dir,
        _dns: dns,
    }
}

/// One more generation of `g`, not yet in use: the same profile over the same
/// data directory, whose cache it reads as it is now.
async fn one_more(w: &World, g: &Generation) -> Runtime {
    online_runtime_with(
        g.dir.path(),
        &g.text,
        &w.server,
        g.engine.shared(),
        watching(g.seeded.hosts.clone()),
    )
    .await
}

/// The same generation, which cannot go to the network.
async fn offline_generation(w: &World) -> Generation {
    let dir = tempfile::tempdir().unwrap();
    let seeded = seed_generation(w, dir.path());
    let (text, dns) = profile_text(w, "").await;
    let runtime = runtime(dir.path(), &text, EngineShared::default()).await;
    Generation {
        engine: Engine::new(runtime),
        seeded,
        text,
        dir,
        _dns: dns,
    }
}

/// The text of the managed profile `/managed` as the server gives it.
fn managed_copy(w: &World) -> String {
    w.managed_copy("/managed", "interval=3600")
}

/// The sources of a profile that is managed from `/managed` and includes
/// `/inc`, both checked ten minutes ago. The local file is newer than the
/// copies, so it is the profile in effect, and it names both.
fn profile_sources(w: &World, offline: bool) -> Arc<ProfileSources> {
    profile_sources_saying(w, offline, "interval=3600")
}

/// `profile_sources`, with `line` as what the local file says after the
/// address of the managed profile.
fn profile_sources_saying(w: &World, offline: bool, line: &str) -> Arc<ProfileSources> {
    w.server.set("/managed", managed_copy(w));
    w.server.set("/inc", INCLUDED);
    w.seed("/managed", &managed_copy(w), at(600));
    w.seed("/inc", INCLUDED, at(600));
    w.write(&format!(
        "#!MANAGED-CONFIG {} {line}\n[Rule]\n#!include {}\nFINAL,DIRECT\n",
        w.url("/managed"),
        w.url("/inc")
    ));
    let sources = w.sources(offline);
    sources.track(&w.load());
    sources
}

fn item<'a>(listing: &'a [ExternalResource], key: &str) -> &'a ExternalResource {
    let mut named = listing.iter().filter(|resource| resource.key == key);
    let one = named
        .next()
        .unwrap_or_else(|| panic!("no item has the key {key}: {listing:?}"));
    assert!(named.next().is_none(), "two items have the key {key}");
    one
}

fn kinds(listing: &[ExternalResource]) -> Vec<&'static str> {
    listing.iter().map(|resource| resource.kind).collect()
}

/// The text the manager of the generation holds of the rule set at `url`, if
/// it holds any.
fn copy_held_by_the_manager(engine: &Engine, url: &Url) -> Option<String> {
    let handle = engine.runtime().stack.resources.get(&ResourceSpec {
        source: ResourceSource::Url(url.clone()),
        update_interval: None,
        kind: ResourceKind::RuleSet,
    });
    let (data, _) = handle.current().data()?;
    Some(String::from_utf8_lossy(&data).into_owned())
}

/// The text the manager of the generation holds of the rule set at `url`.
fn held_by_the_manager(engine: &Engine, url: &Url) -> String {
    copy_held_by_the_manager(engine, url).expect("the manager holds a copy")
}

/// Whether `manager` is at a download of the resource `key` names.
fn is_updating(manager: &ResourceManager, key: &str) -> bool {
    manager
        .statuses()
        .iter()
        .any(|status| status.key == key && status.updating)
}

/// The text of `path` the sources of the profile have on disk.
fn read_by_the_sources(sources: &Arc<ProfileSources>, w: &World, path: &str) -> String {
    text_of(sources.remote().get(&w.url(path)))
}

// ---------------------------------------------------------------------------
// the listing

/// One item for each kind of resource, named as the control plane names them.
/// The hosts file is a resource of the manager and is not listed.
#[tokio::test]
async fn the_listing_names_each_kind_and_leaves_the_hosts_file_out() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    // a listing without the hosts file proves nothing unless the manager has it
    let hosts = resource_key(&ResourceSource::File(g.seeded.hosts.clone()));
    let held = g.engine.runtime().stack.resources.statuses();
    assert!(
        held.iter()
            .any(|s| s.key == hosts && s.kind == ResourceKind::System && s.local),
        "the manager does not watch the hosts file: {held:?}"
    );

    let listing = g.engine.external_resources(None);
    assert_eq!(listing.len(), 6, "{listing:?}");
    assert!(listing.iter().all(|resource| resource.key != hosts));
    assert!(listing.iter().all(|resource| !resource.suspended));

    let rules = item(&listing, &w.key("/rules.list"));
    assert_eq!(rules.path, w.url("/rules.list").to_string());
    assert_eq!(rules.kind, "ruleset");
    assert!(!rules.local && rules.ready && !rules.updating);
    assert_eq!(rules.updated_at, Some(g.seeded.rules));
    assert_eq!(rules.error, None);

    let domains = item(&listing, &w.key("/domains.txt"));
    assert_eq!(domains.path, w.url("/domains.txt").to_string());
    assert_eq!(domains.kind, "domainset");
    assert!(!domains.local && domains.ready);
    assert_eq!(domains.updated_at, Some(g.seeded.domains));

    // a file: no time of its own, and it is read
    let local = listing
        .iter()
        .find(|resource| resource.local)
        .expect("the rule set that is a file");
    assert_eq!(local.kind, "ruleset");
    assert!(local.ready && !local.updating);
    assert_eq!(local.updated_at, None);
    assert_eq!(local.error, None);
    assert!(local.path.ends_with("local.list"), "{}", local.path);

    // a subscription is called by its label: the address holds a token
    let subscription = item(
        &listing,
        &resource_key(&ResourceSource::Url(subscription(&w))),
    );
    assert_eq!(subscription.path, "policy-path of `Sub`");
    assert_eq!(subscription.kind, "policy-group");
    assert!(subscription.ready);
    assert_eq!(subscription.updated_at, Some(g.seeded.nodes));

    // the databases the server does not have: never downloaded, and why
    for (file, path) in [
        ("GeoLite2-Country.mmdb", "/geo/country.mmdb"),
        ("GeoLite2-ASN.mmdb", "/geo/asn.mmdb"),
    ] {
        let database = item(&listing, &w.key(path));
        assert_eq!(database.path, file);
        assert_eq!(database.kind, "geoip");
        assert!(!database.local && !database.ready && !database.updating);
        assert_eq!(database.updated_at, Some(0));
        assert_eq!(database.error.as_deref(), Some("http status 404"));
    }

    for resource in &listing {
        assert!(!resource.path.contains(TOKEN), "{resource:?}");
        assert!(
            !resource.error.iter().any(|error| error.contains(TOKEN)),
            "{resource:?}"
        );
    }
    // the generation's resources are by name
    let names: Vec<(&str, &str)> = listing
        .iter()
        .map(|resource| (resource.path.as_str(), resource.key.as_str()))
        .collect();
    assert!(names.windows(2).all(|pair| pair[0] <= pair[1]), "{names:?}");
}

/// Two addresses that differ in a user name and a password only are called the
/// same, neither name nor password is in what they are called, and they are
/// listed by key.
#[tokio::test]
async fn items_that_are_called_the_same_are_listed_by_key() {
    let w = World::new().await;
    let with_credentials = |name: String| {
        let mut url = w.url("/same.list");
        url.set_username(&name).unwrap();
        url.set_password(Some(&name)).unwrap();
        url
    };
    let key = |url: &Url| resource_key(&ResourceSource::Url(url.clone()));
    // Names that put the addresses in one order as text, which is how the
    // manager lists its resources, and in the other by key: so the order of
    // the keys is what makes the order, and nothing else does.
    let (one, two) = (1..)
        .map(|n| {
            (
                with_credentials(format!("USERA{n}")),
                with_credentials(format!("USERB{n}")),
            )
        })
        .find(|(a, b)| (a.as_str() < b.as_str()) != (key(a) < key(b)))
        .unwrap();
    w.server.set("/same.list", RULES);
    let g = generation(&w, &format!("RULE-SET,{one},DIRECT\nRULE-SET,{two},DIRECT")).await;
    let listing = g.engine.external_resources(None);
    let keys = [key(&one), key(&two)];
    let at: Vec<usize> = listing
        .iter()
        .enumerate()
        .filter(|(_, resource)| keys.contains(&resource.key))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(at.len(), 2, "{listing:?}");
    assert_eq!(at[1], at[0] + 1, "they are listed side by side");
    let (first, second) = (&listing[at[0]], &listing[at[1]]);
    assert_eq!(first.path, second.path);
    assert!(first.key < second.key);
    assert!(
        !first.path.contains("USER"),
        "the name is in what it is called: {}",
        first.path
    );
}

/// The sources of the profile come first, the managed profile before the
/// includes, and the rest by name.
#[tokio::test]
async fn the_sources_of_the_profile_are_listed_first() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    let sources = profile_sources(&w, false);
    let listing = g.engine.external_resources(Some(&sources));
    assert_eq!(listing.len(), 8, "{listing:?}");
    assert_eq!(kinds(&listing)[..2], ["profile", "include"]);

    let managed = &listing[0];
    assert_eq!(managed.key, w.key("/managed"));
    let origin = format!("http://127.0.0.1:{}/…", w.url("/").port().unwrap());
    assert_eq!(managed.path, format!("managed profile ({origin})"));
    assert!(!managed.local && managed.ready && !managed.updating);
    assert_eq!(managed.error, None);
    // checked ten minutes ago, as the copy was seeded
    let checked = managed.updated_at.expect("a download has a time");
    assert!(
        secs(at(660)) <= checked && checked <= secs(at(600)),
        "{checked}"
    );

    let include = &listing[1];
    assert_eq!(include.key, w.key("/inc"));
    assert_eq!(
        include.path,
        format!("remote include {} ({origin})", &w.key("/inc")[..8])
    );

    let rest = &listing[2..];
    assert!(
        rest.iter()
            .all(|resource| resource.kind != "profile" && resource.kind != "include"),
        "{rest:?}"
    );
    let names: Vec<(&str, &str)> = rest
        .iter()
        .map(|resource| (resource.path.as_str(), resource.key.as_str()))
        .collect();
    assert!(names.windows(2).all(|pair| pair[0] <= pair[1]), "{names:?}");
}

/// The order of the sources is the order they were tracked in; the listing
/// puts the managed profile first whatever it is.
#[tokio::test]
async fn the_managed_profile_is_first_whatever_order_the_sources_are_in() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    w.seed("/managed", &managed_copy(&w), at(600));
    w.seed("/inc", INCLUDED, at(600));
    let sources = w.sources(false);
    let remote = |path: &str, role| RemoteRef {
        url: w.url(path),
        role,
        path: Some(cache_file(&w.data(), &w.url(path))),
        interval: Duration::from_secs(3600),
        strict: false,
        lacks_section: false,
    };
    sources.track(&SourceInfo {
        main: w.profile(),
        includes: Vec::new(),
        main_text: w.profile(),
        remote: vec![
            remote("/inc", RemoteRole::Include),
            remote("/managed", RemoteRole::Managed),
        ],
    });
    let tracked: Vec<_> = sources.statuses().iter().map(|s| s.kind).collect();
    assert_eq!(
        tracked,
        [ResourceKind::Include, ResourceKind::Profile],
        "the sources keep the order they were tracked in"
    );
    let listing = g.engine.external_resources(Some(&sources));
    assert_eq!(kinds(&listing)[..2], ["profile", "include"]);
}

/// The flag is on the managed profile, and on nothing else, for as long as
/// the engine refuses sessions; and so is the reason, which no update of
/// these sources has failed to give.
#[tokio::test]
async fn the_managed_profile_is_flagged_while_the_engine_is_suspended() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    let sources = profile_sources(&w, false);
    let why = "the update has not finished";
    let listed = || g.engine.external_resources(Some(&sources));
    let flagged = || -> Vec<&'static str> {
        listed()
            .iter()
            .filter(|resource| resource.suspended)
            .map(|resource| resource.kind)
            .collect()
    };
    let giving_the_reason = || -> Vec<&'static str> {
        listed()
            .iter()
            .filter(|resource| resource.error.as_deref() == Some(why))
            .map(|resource| resource.kind)
            .collect()
    };
    assert!(flagged().is_empty());
    assert!(giving_the_reason().is_empty());
    g.engine.suspend(Some(why.to_string()));
    assert_eq!(flagged(), ["profile"]);
    assert_eq!(giving_the_reason(), ["profile"]);
    g.engine.suspend(None);
    assert!(flagged().is_empty());
    assert!(giving_the_reason().is_empty());
}

/// An update of the managed profile that failed is what its item says, also
/// while sessions are refused for a reason that was given before it.
#[tokio::test]
async fn a_failed_update_is_what_the_managed_profile_says_while_sessions_are_refused() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    let sources = profile_sources(&w, false);
    let key = w.key("/managed");
    w.server.set_status("/managed", 500);
    assert_eq!(
        g.engine
            .update_external_resources(Some(&sources), Some(&key), LIMIT)
            .await,
        Some(vec![(key.clone(), Err("http status 500".to_string()))])
    );
    g.engine
        .suspend(Some("the update has not finished".to_string()));
    let listing = g.engine.external_resources(Some(&sources));
    let managed = item(&listing, &key);
    assert!(managed.suspended, "{managed:?}");
    assert_eq!(managed.error.as_deref(), Some("http status 500"));
}

/// Offline no update is made, so none fails: a managed profile whose update
/// `strict` requires is due says why sessions are refused all the same, for as
/// long as they are.
#[tokio::test]
async fn offline_a_strict_profile_that_is_due_says_why_sessions_are_refused() {
    let w = World::new().await;
    let g = offline_generation(&w).await;
    // every minute, and the copy was checked ten minutes ago
    let sources = profile_sources_saying(&w, true, "interval=60 strict=true");
    let why = sources
        .strict()
        .expect("the update is due and cannot be made")
        .why;
    g.engine.suspend(Some(why));
    let listing = g.engine.external_resources(Some(&sources));
    let managed = item(&listing, &w.key("/managed"));
    assert!(managed.suspended && managed.ready, "{managed:?}");
    assert!(!managed.updating, "{managed:?}");
    assert_eq!(
        managed.error.as_deref(),
        Some("offline mode: the update cannot be attempted")
    );
    assert_eq!(w.server.requests().len(), 0, "offline asks nobody");
}

// ---------------------------------------------------------------------------
// the update of one key

#[tokio::test]
async fn an_update_of_a_key_says_what_the_attempt_came_to() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    let sources = profile_sources(&w, false);
    let update = async |key: &str| {
        g.engine
            .update_external_resources(Some(&sources), Some(key), LIMIT)
            .await
    };

    // the new content is what the manager holds
    let rules = w.key("/rules.list");
    w.server.set("/rules.list", "DOMAIN,newer.example\n");
    assert_eq!(update(&rules).await, Some(vec![(rules.clone(), Ok(()))]));
    assert_eq!(
        held_by_the_manager(&g.engine, &w.url("/rules.list")),
        "DOMAIN,newer.example\n"
    );

    // a server that answers 500: the manager's own words, and the copy stays
    w.server.set_status("/rules.list", 500);
    assert_eq!(
        update(&rules).await,
        Some(vec![(rules.clone(), Err("http status 500".to_string()))])
    );
    assert_eq!(
        held_by_the_manager(&g.engine, &w.url("/rules.list")),
        "DOMAIN,newer.example\n"
    );

    // a source of the profile is found in the other holder
    let included = w.key("/inc");
    w.server
        .set("/inc", "[Rule]\nDOMAIN,included-later.example,DIRECT\n");
    assert_eq!(
        update(&included).await,
        Some(vec![(included.clone(), Ok(()))])
    );
    assert_eq!(
        read_by_the_sources(&sources, &w, "/inc"),
        "[Rule]\nDOMAIN,included-later.example,DIRECT\n"
    );

    // a local file is read again, and that is all there is to say
    let listing = g.engine.external_resources(Some(&sources));
    let local = listing
        .iter()
        .find(|r| r.local)
        .expect("a file")
        .key
        .clone();
    assert_eq!(update(&local).await, Some(vec![(local.clone(), Ok(()))]));
}

#[tokio::test]
async fn a_key_that_names_nothing_listed_is_not_found() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    let sources = profile_sources(&w, false);
    let hosts = resource_key(&ResourceSource::File(g.seeded.hosts.clone()));
    for key in ["no-such-key", "", hosts.as_str()] {
        assert_eq!(
            g.engine
                .update_external_resources(Some(&sources), Some(key), LIMIT)
                .await,
            None,
            "{key:?}"
        );
    }
    // and without the sources of the profile a key of theirs names nothing
    assert_eq!(
        g.engine
            .update_external_resources(None, Some(&w.key("/managed")), LIMIT)
            .await,
        None
    );
}

// ---------------------------------------------------------------------------
// the update of all of them

/// Every resource that is downloaded, of both holders, in the order of the
/// listing; the file is left alone.
#[tokio::test]
async fn an_update_of_all_asks_every_remote_resource_of_both_holders() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    let sources = profile_sources(&w, false);
    let listing = g.engine.external_resources(Some(&sources));
    let remote: Vec<String> = listing
        .iter()
        .filter(|resource| !resource.local)
        .map(|resource| resource.key.clone())
        .collect();

    let answer = g
        .engine
        .update_external_resources(Some(&sources), None, LIMIT)
        .await
        .expect("all is always found");
    let keys: Vec<&String> = answer.iter().map(|(key, _)| key).collect();
    assert_eq!(keys, remote.iter().collect::<Vec<_>>());
    assert_eq!(
        remote.len(),
        7,
        "the managed profile, the include and five more"
    );
    let results: Vec<(&str, Result<(), &str>)> = answer
        .iter()
        .map(|(key, result)| {
            let name = listing
                .iter()
                .find(|r| &r.key == key)
                .unwrap()
                .path
                .as_str();
            (name, result.as_ref().map(|_| ()).map_err(String::as_str))
        })
        .collect();
    for (name, result) in &results {
        let is_a_database = name.starts_with("GeoLite2-");
        assert_eq!(
            *result,
            if is_a_database {
                Err("http status 404")
            } else {
                Ok(())
            },
            "{name}"
        );
    }
    // the managed profile and the include are asked first
    assert_eq!(
        keys[..2],
        [&w.key("/managed"), &w.key("/inc")],
        "the sources of the profile come first"
    );
}

/// An address that is a source of the profile and a set at once is one key:
/// it is updated in both, once.
#[tokio::test]
async fn one_address_in_both_holders_is_one_key_updated_in_both() {
    let w = World::new().await;
    let extra = format!("RULE-SET,{},DIRECT", w.url("/inc"));
    w.server.set("/inc", INCLUDED);
    let g = generation(&w, &extra).await;
    let sources = profile_sources(&w, false);
    let shared = w.key("/inc");

    let listing = g.engine.external_resources(Some(&sources));
    let named: Vec<&str> = listing
        .iter()
        .filter(|resource| resource.key == shared)
        .map(|resource| resource.kind)
        .collect();
    assert_eq!(named, ["include", "ruleset"]);

    // one key in the answer, whichever way it is asked
    let newer = "[Rule]\nDOMAIN,newer.example,DIRECT\n";
    w.server.set("/inc", newer);
    let answer = g
        .engine
        .update_external_resources(Some(&sources), Some(&shared), LIMIT)
        .await;
    assert_eq!(answer, Some(vec![(shared.clone(), Ok(()))]));
    assert_eq!(held_by_the_manager(&g.engine, &w.url("/inc")), newer);
    assert_eq!(read_by_the_sources(&sources, &w, "/inc"), newer);

    // a page where an included file is meant to be: the profile refuses it,
    // the set takes it, and the key says the profile's no
    w.server.set("/inc", HTML);
    let answer = g
        .engine
        .update_external_resources(Some(&sources), Some(&shared), LIMIT)
        .await;
    assert_eq!(
        answer,
        Some(vec![(
            shared.clone(),
            Err("lacks the section it is included into".to_string())
        )])
    );
    assert_eq!(held_by_the_manager(&g.engine, &w.url("/inc")), HTML);
    assert_eq!(read_by_the_sources(&sources, &w, "/inc"), newer);

    // all of them: the address is asked once. This comes last: a check of the
    // managed profile that succeeds puts the downloaded copy, which does not
    // include the file, in effect over the local file, which does.
    let all = g
        .engine
        .update_external_resources(Some(&sources), None, LIMIT)
        .await
        .unwrap();
    assert_eq!(all.iter().filter(|(key, _)| *key == shared).count(), 1);
}

/// What is not done when the time is up is `timed out`, and the keys that are
/// keep what they came to.
#[tokio::test]
async fn a_key_that_is_not_done_by_the_limit_is_timed_out_and_the_others_keep_their_answers() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    w.server.set("/rules.list", "DOMAIN,newer.example\n");
    w.server.set_delay("/domains.txt", Duration::from_secs(8));
    let limit = Duration::from_millis(1500);
    let begun = Instant::now();
    let answer = g
        .engine
        .update_external_resources(None, None, limit)
        .await
        .unwrap();
    let took = begun.elapsed();
    assert!(took >= limit, "returned before the limit: {took:?}");
    assert!(
        took < Duration::from_secs(5),
        "waited for the slow key: {took:?}"
    );
    let result_of = |path: &str| {
        answer
            .iter()
            .find(|(key, _)| *key == w.key(path))
            .unwrap_or_else(|| panic!("no answer for {path}: {answer:?}"))
            .1
            .clone()
    };
    assert_eq!(result_of("/domains.txt"), Err("timed out".to_string()));
    assert_eq!(result_of("/rules.list"), Ok(()));
    assert_eq!(
        result_of("/geo/country.mmdb"),
        Err("http status 404".to_string())
    );
    assert_eq!(
        held_by_the_manager(&g.engine, &w.url("/rules.list")),
        "DOMAIN,newer.example\n"
    );
}

/// A caller that goes away ends the waiting and nothing else: the download it
/// asked for goes on, and lands in the manager.
#[tokio::test]
async fn a_request_that_is_dropped_leaves_the_download_to_the_manager() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    w.server.set("/rules.list", "DOMAIN,newer.example\n");
    w.server
        .set_delay("/rules.list", Duration::from_millis(700));
    let key = w.key("/rules.list");
    let request = g.engine.update_external_resources(None, Some(&key), LIMIT);
    // the caller is gone while the download is under way
    assert!(
        tokio::time::timeout(Duration::from_millis(150), request)
            .await
            .is_err()
    );
    wait_until("the download to land", || {
        held_by_the_manager(&g.engine, &w.url("/rules.list")) == "DOMAIN,newer.example\n"
    })
    .await;
}

// ---------------------------------------------------------------------------
// a generation that comes in

/// A generation reads the cache when it is built. An update that lands after
/// that in the generation before it is asked for again when it comes in: the
/// copy it read is the one from before.
#[tokio::test]
async fn a_copy_checked_after_the_next_generation_was_built_is_asked_for_again() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    let url = w.url("/rules.list");
    let key = w.key("/rules.list");
    let next = one_more(&w, &g).await;
    assert_eq!(held_by_the_manager(&g.engine, &url), RULES);

    w.server.set("/rules.list", NEWER);
    assert_eq!(
        g.engine
            .update_external_resources(None, Some(&key), LIMIT)
            .await,
        Some(vec![(key, Ok(()))])
    );
    let asked = w.server.hits("/rules.list");
    g.engine.swap_runtime(next);
    wait_until("the generation that came in to hold the update", || {
        held_by_the_manager(&g.engine, &url) == NEWER
    })
    .await;
    assert_eq!(w.server.hits("/rules.list"), asked + 1);
}

/// An update that is under way in the generation that goes lands in the cache
/// after the next one read it: the next one makes it too, beside the other.
#[tokio::test]
async fn an_update_under_way_when_the_generation_changes_is_made_by_the_next_one_too() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    let url = w.url("/rules.list");
    let key = w.key("/rules.list");
    let next = one_more(&w, &g).await;
    w.server.set("/rules.list", NEWER);
    // long enough for the generations to change while it is under way
    w.server.set_delay("/rules.list", UNDER_WAY);
    let asked = w.server.hits("/rules.list");

    let request = tokio::spawn({
        let (engine, key) = (g.engine.clone(), key.clone());
        async move {
            engine
                .update_external_resources(None, Some(&key), LIMIT)
                .await
        }
    });
    wait_until("the download to be under way", || {
        w.server.hits("/rules.list") == asked + 1
    })
    .await;
    assert!(
        is_updating(&g.engine.runtime().stack.resources, &key),
        "the download landed before the generations changed"
    );
    g.engine.swap_runtime(next);
    wait_until("the generation that came in to ask too", || {
        w.server.hits("/rules.list") == asked + 2
    })
    .await;
    // Beside the other: the request still waits, so it is the generation
    // that asked as it came in, and not the request as it ended.
    assert!(
        !request.is_finished(),
        "asked only once the request had ended"
    );
    assert_eq!(request.await.unwrap(), Some(vec![(key, Ok(()))]));
    wait_until("the generation that came in to hold the update", || {
        held_by_the_manager(&g.engine, &url) == NEWER
    })
    .await;
}

/// A generation that comes in when nothing was checked since it read the
/// cache asks for nothing.
#[tokio::test]
async fn a_generation_that_comes_in_with_nothing_checked_since_asks_for_nothing() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    let next = one_more(&w, &g).await;
    let asked = w.server.requests().len();
    g.engine.swap_runtime(next);
    // Nothing is to happen: a request would be at the server by now.
    tokio::time::sleep(NOTHING_MORE).await;
    let since: Vec<String> = w.server.requests()[asked..]
        .iter()
        .map(|request| request.path.clone())
        .collect();
    assert!(since.is_empty(), "asked for {since:?}");
}

/// What the generation that comes in is fetching already is not asked for a
/// second time: its attempt began after the cache was read.
#[tokio::test]
async fn a_resource_the_next_generation_is_fetching_already_is_not_asked_for_twice() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    let url = w.url("/rules.list");
    let key = w.key("/rules.list");
    let source = ResourceSource::Url(url.clone());
    let next = one_more(&w, &g).await;
    w.server.set("/rules.list", NEWER);
    // long enough for the generations to change while both are under way
    w.server.set_delay("/rules.list", UNDER_WAY);
    let asked = w.server.hits("/rules.list");

    assert!(g.engine.runtime().stack.resources.force_update(&source));
    assert!(next.stack.resources.force_update(&source));
    wait_until("both downloads to be under way", || {
        w.server.hits("/rules.list") == asked + 2
    })
    .await;
    assert!(
        is_updating(&g.engine.runtime().stack.resources, &key)
            && is_updating(&next.stack.resources, &key),
        "a download landed before the generations changed"
    );
    g.engine.swap_runtime(next);
    wait_until("the generation that came in to hold the update", || {
        held_by_the_manager(&g.engine, &url) == NEWER
    })
    .await;
    // Nothing more is to come: an attempt that was asked for beside the one
    // under way would have followed it at once.
    tokio::time::sleep(NOTHING_MORE).await;
    assert_eq!(w.server.hits("/rules.list"), asked + 2);
}

/// A set neither generation could get when it was built, which the one that
/// goes got since: the one that comes in has no copy and no time of its own,
/// and asks at once, not when its next attempt is due.
#[tokio::test]
async fn what_the_next_generation_could_not_get_and_the_one_before_got_since_is_asked_for_again() {
    let w = World::new().await;
    // the server does not have it yet
    let extra = format!("RULE-SET,{},DIRECT", w.url("/late.list"));
    let g = generation(&w, &extra).await;
    let url = w.url("/late.list");
    let key = w.key("/late.list");
    let next = one_more(&w, &g).await;
    assert_eq!(copy_held_by_the_manager(&g.engine, &url), None);
    assert!(
        !is_updating(&next.stack.resources, &key),
        "the first attempt of the generation to come has not ended"
    );

    w.server.set("/late.list", NEWER);
    assert_eq!(
        g.engine
            .update_external_resources(None, Some(&key), LIMIT)
            .await,
        Some(vec![(key, Ok(()))])
    );
    let asked = w.server.hits("/late.list");
    g.engine.swap_runtime(next);
    // an attempt that failed is made again after a minute at the earliest
    wait_until("the generation that came in to hold the set", || {
        copy_held_by_the_manager(&g.engine, &url).as_deref() == Some(NEWER)
    })
    .await;
    assert_eq!(w.server.hits("/late.list"), asked + 1);
}

/// A request takes the manager of the generation in use when it begins and
/// asks it a moment later. A generation that comes in between the two looked
/// at the one before it when nothing had been asked of that one: the request
/// has the generation in use ask too, when it ends.
#[tokio::test]
async fn a_request_that_ends_under_another_generation_has_that_one_ask_too() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    let url = w.url("/rules.list");
    let key = w.key("/rules.list");
    let next = one_more(&w, &g).await;
    w.server.set("/rules.list", NEWER);
    let asked = w.server.hits("/rules.list");

    let request = g.engine.update_external_resources(None, Some(&key), LIMIT);
    tokio::pin!(request);
    // Polled once: the request has taken the manager of the generation in
    // use, and what asks it runs when this test next waits. That is so on
    // the current-thread runtime this test runs on and on no other: with
    // worker threads the asking could come before the generations change,
    // and the test would pass without the look it is here for.
    tokio::select! {
        biased;
        answer = &mut request => panic!("answered at once: {answer:?}"),
        _ = std::future::ready(()) => {}
    }
    g.engine.swap_runtime(next);

    assert_eq!(request.await, Some(vec![(key.clone(), Ok(()))]));
    wait_until("the generation in use to hold the update", || {
        held_by_the_manager(&g.engine, &url) == NEWER
    })
    .await;
    // one request of each generation: the one that came in asked once, when
    // the request ended, and not as it came in
    assert_eq!(w.server.hits("/rules.list"), asked + 2);
}

// ---------------------------------------------------------------------------
// a version that waits, and no network

/// The new version of the managed profile names a file that was never
/// downloaded: it waits for it, and the key says so. The file is followed from
/// then on, as an include, although no generation names it.
#[tokio::test]
async fn a_version_that_waits_for_a_source_answers_with_the_waiting_sentence() {
    let w = World::new().await;
    let g = generation(&w, "").await;
    // the local file is only a pointer: the downloaded copy is the profile
    let line = format!("#!MANAGED-CONFIG {} interval=3600\n", w.url("/managed"));
    w.write(&line);
    w.seed(
        "/managed",
        &format!("{line}[Rule]\nFINAL,DIRECT\n"),
        at(600),
    );
    let sources = w.sources(false);
    sources.track(&w.load());
    w.server.set(
        "/managed",
        format!("{line}[Rule]\n#!include {}\nFINAL,DIRECT\n", w.url("/x")),
    );
    w.server.set("/x", INCLUDED);
    w.server.set_delay("/x", Duration::from_millis(400));

    let key = w.key("/managed");
    let answer = g
        .engine
        .update_external_resources(Some(&sources), Some(&key), LIMIT)
        .await;
    assert_eq!(answer, Some(vec![(key.clone(), Err(WAITING.to_string()))]));
    let listing = g.engine.external_resources(Some(&sources));
    assert_eq!(kinds(&listing)[..2], ["profile", "include"], "{listing:?}");
    assert_eq!(listing[0].error.as_deref(), Some(WAITING));
    assert_eq!(listing[1].key, w.key("/x"));
}

/// Offline nothing is asked: every key that is downloaded says so at once.
#[tokio::test]
async fn offline_every_key_that_is_downloaded_is_an_error_at_once() {
    let w = World::new().await;
    let g = offline_generation(&w).await;
    let sources = profile_sources(&w, true);
    let remote: Vec<String> = g
        .engine
        .external_resources(Some(&sources))
        .into_iter()
        .filter(|resource| !resource.local)
        .map(|resource| resource.key)
        .collect();
    // the managed profile, the include, the two sets and the subscription:
    // there is no GeoIP updater without a network
    assert_eq!(remote.len(), 5, "{remote:?}");
    let begun = Instant::now();
    let answer = g
        .engine
        .update_external_resources(Some(&sources), None, LIMIT)
        .await
        .unwrap();
    assert!(begun.elapsed() < Duration::from_secs(3));
    assert_eq!(
        answer,
        remote
            .into_iter()
            .map(|key| (key, Err(OFFLINE.to_string())))
            .collect::<Vec<_>>()
    );
    // and nobody was asked
    let asked: Vec<String> = w.server.requests().into_iter().map(|r| r.path).collect();
    assert!(asked.is_empty(), "{asked:?}");
}

// ---------------------------------------------------------------------------
// the lines of a set that are not used

/// What the log says of a set that was loaded with lines it cannot use.
const UNUSED: &str = "set loaded with lines that are not used";

/// A rule set with three lines nobody can read (a share link, a proxy
/// definition and a `FINAL`) and one rule that carries a parameter nobody
/// knows. The marker is in what these lines carry: none of it may be logged.
const UNREADABLE: &str = "\
# a list
DOMAIN,one.example
trojan://S3CR3T@host.example:443
DOMAIN,two.example,S3CR3T
P = ss, host.example, 8388, password=S3CR3T
FINAL,DIRECT
DOMAIN,three.example
";

/// The same numbers of lines in another file, with two more lines of comment
/// on top: the numbers of the lines are not the same.
const UNREADABLE_AGAIN: &str = "\
# a list, revised
# and one more line
DOMAIN,four.example
vmess://S3CR3T
DOMAIN,five.example,S3CR3T
Q = http, host.example, 80, password=S3CR3T
FINAL,REJECT
DOMAIN,six.example
";

/// Two lines nobody can read, and no parameter nobody knows.
const FEWER: &str = "\
DOMAIN,seven.example
trojan://S3CR3T@host.example:443
FINAL,DIRECT
";

const CLEAN: &str = "DOMAIN,eight.example\n";

/// A generation that downloads, over a data directory `seed` has filled before
/// it starts, whose profile has `rules` and nothing else. It waits for the
/// first downloads.
async fn generation_of(
    w: &World,
    rules: &str,
    seed: impl FnOnce(&Path),
) -> (Arc<Engine>, tempfile::TempDir, MockDns) {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let dns = MockDns::spawn().await;
    let text = Profile {
        rules,
        ..Profile::default()
    }
    .text(dns.addr());
    let runtime = online_runtime_with(
        dir.path(),
        &text,
        &w.server,
        EngineShared::default(),
        |stack| stack.wait = Duration::from_secs(5),
    )
    .await;
    (Engine::new(runtime), dir, dns)
}

/// The version of the set read from the resource `key` that the generation has
/// compiled; 0 when it has none yet.
fn set_version(engine: &Engine, key: &str) -> u64 {
    engine
        .runtime()
        .stack
        .registry
        .statuses()
        .into_iter()
        .find(|status| status.key.as_deref() == Some(key))
        .map_or(0, |status| status.version)
}

/// The server gives `body` for the set at `/sets/a.list` from now on. Returns
/// once the generation has downloaded it and compiled it as the `version` of
/// the set read from the resource `key`. What the registry has to say of that
/// load is in the log by then: it writes its warning before it stores the
/// set, so a caller that counts the warnings next has nothing more to wait
/// for.
async fn serve_the_set_anew(w: &World, engine: &Engine, key: &str, body: &str, version: u64) {
    w.server.set("/sets/a.list", body.to_string());
    let answer = engine
        .update_external_resources(None, Some(key), LIMIT)
        .await;
    assert_eq!(answer, Some(vec![(key.to_string(), Ok(()))]));
    wait_until("the set to be compiled anew", || {
        set_version(engine, key) >= version
    })
    .await;
}

/// The lines of the log that say a set was loaded with lines it cannot use.
fn unused_lines_said(log: &LogCapture) -> Vec<String> {
    log.lines()
        .into_iter()
        .filter(|line| line.starts_with("WARN") && line.contains(UNUSED))
        .collect()
}

/// The value of the field `name` of a line of the log, if the line has it.
fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|word| word.strip_prefix(name)?.strip_prefix('='))
}

/// The fields a warning about the lines of a set has, in the order of the
/// log's own.
const UNUSED_FIELDS: [&str; 6] = [
    "set",
    "skipped",
    "truncated",
    "unknown_params",
    "first_skipped_line",
    "first_unknown_line",
];

/// No line of the log holds what the lines of the sets carry, or the password
/// of the address one of the sets is fetched from.
fn nothing_secret_was_logged(log: &LogCapture) {
    for line in log.lines() {
        assert!(
            !line.contains("S3CR3T") && !line.contains("PASSW0RD"),
            "{line}"
        );
    }
}

/// A set that is downloaded after the start says what it could not use, and
/// says it again only when that changes: a list that is refreshed every day
/// and always has three lines nobody can read is not worth a warning a day.
#[tokio::test]
async fn a_set_that_arrives_after_the_start_says_what_it_could_not_use_when_that_changes() {
    let log = LogCapture::start();
    let w = World::new().await;
    // the address carries a user name and a password
    let mut url = w.url("/sets/a.list");
    url.set_username("al1ce").unwrap();
    url.set_password(Some("PASSW0RD")).unwrap();
    let key = resource_key(&ResourceSource::Url(url.clone()));
    let shown = format!("http://***@127.0.0.1:{}/sets/a.list", url.port().unwrap());
    w.server.set("/sets/a.list", UNREADABLE);
    let (engine, _dir, _dns) = generation_of(&w, &format!("RULE-SET,{url},DIRECT"), |_| {}).await;
    wait_until("the set to load", || set_version(&engine, &key) >= 1).await;

    // nothing was cached, so this is the first time it is read: it says so
    let said = unused_lines_said(&log);
    assert_eq!(said.len(), 1, "{said:?}");
    assert_eq!(
        UNUSED_FIELDS.map(|name| field(&said[0], name)),
        [
            Some(shown.as_str()),
            Some("3"),
            Some("0"),
            Some("1"),
            Some("3"),
            Some("4")
        ],
        "{}",
        said[0]
    );

    let change =
        async |body: &str, version: u64| serve_the_set_anew(&w, &engine, &key, body, version).await;
    // the same numbers in another file, on other lines: nothing more is said
    change(UNREADABLE_AGAIN, 2).await;
    assert_eq!(unused_lines_said(&log).len(), 1, "{:?}", log.lines());

    // other numbers: said again; there is no parameter nobody knows now, and
    // so no line to name for it
    change(FEWER, 3).await;
    let said = unused_lines_said(&log);
    assert_eq!(said.len(), 2, "{said:?}");
    // a number of lines that is none has no field, not a field that says none
    assert!(!said[1].contains("first_unknown_line"), "{}", said[1]);
    assert_eq!(
        UNUSED_FIELDS.map(|name| field(&said[1], name)),
        [
            Some(shown.as_str()),
            Some("2"),
            Some("0"),
            Some("0"),
            Some("2"),
            None
        ],
        "{}",
        said[1]
    );

    // nothing to say, nothing said; the numbers of the first load again are
    // news again, since they are not the ones of the load before
    change(CLEAN, 4).await;
    assert_eq!(unused_lines_said(&log).len(), 2);
    change(UNREADABLE, 5).await;
    assert_eq!(unused_lines_said(&log).len(), 3);
    nothing_secret_was_logged(&log);
}

/// A set that was read from the cache when the generation was built was said
/// in the diagnostics of the build: a refresh with the same numbers is not
/// said again, and one with other numbers is.
#[tokio::test]
async fn a_set_the_start_read_from_the_cache_is_not_said_again_by_a_refresh_with_the_same_numbers()
{
    let log = LogCapture::start();
    let w = World::new().await;
    let url = w.url("/sets/a.list");
    let key = resource_key(&ResourceSource::Url(url.clone()));
    w.server.set("/sets/a.list", UNREADABLE_AGAIN);
    let (engine, _dir, _dns) = generation_of(&w, &format!("RULE-SET,{url},DIRECT"), |dir| {
        seed_cache(dir, &url, UNREADABLE.as_bytes(), at(600))
    })
    .await;
    assert_eq!(set_version(&engine, &key), 1);
    assert_eq!(w.server.hits("/sets/a.list"), 0, "the copy is not due");

    // the start said it, as diagnostics, with the numbers and not the lines
    let runtime = engine.runtime();
    let of_the_set: Vec<(&str, &str, Option<&str>)> = runtime
        .diagnostics()
        .iter()
        .filter(|d| d.message.contains(&format!("set `{url}`")))
        .map(|d| (d.code, d.message.as_str(), d.hint))
        .collect();
    assert_eq!(
        of_the_set
            .iter()
            .map(|(code, _, _)| *code)
            .collect::<Vec<_>>(),
        ["W0023", "W0003"],
        "{of_the_set:?}"
    );
    assert!(
        of_the_set[0].1.contains("3 line(s) skipped (first: line 3"),
        "{of_the_set:?}"
    );
    assert_eq!(
        of_the_set[1].1,
        format!(
            "set `{url}`: 1 line(s) carry a parameter that is not known and is ignored (first: line 4)"
        )
    );
    assert!(
        of_the_set
            .iter()
            .all(|(_, message, hint)| !message.contains("S3CR3T")
                && !hint.unwrap_or_default().contains("S3CR3T")),
        "{of_the_set:?}"
    );
    assert!(unused_lines_said(&log).is_empty());

    let change =
        async |body: &str, version: u64| serve_the_set_anew(&w, &engine, &key, body, version).await;
    // the refresh brings the same numbers on other lines: nothing is said
    change(UNREADABLE_AGAIN, 2).await;
    assert_eq!(w.server.hits("/sets/a.list"), 1);
    assert!(unused_lines_said(&log).is_empty(), "{:?}", log.lines());

    // other numbers are
    change(FEWER, 3).await;
    let said = unused_lines_said(&log);
    assert_eq!(said.len(), 1, "{said:?}");
    assert_eq!(
        UNUSED_FIELDS.map(|name| field(&said[0], name)),
        [
            Some(url.as_str()),
            Some("2"),
            Some("0"),
            Some("0"),
            Some("2"),
            None
        ]
    );
    nothing_secret_was_logged(&log);
}

/// The listing says what the last load of the file of a set could not use,
/// for a rule set and a domain set, whatever their state; no other item says
/// anything of the kind. The numbers follow the next load of the set.
#[tokio::test]
async fn a_set_is_listed_with_the_lines_of_its_file_that_were_not_used() {
    let w = World::new().await;
    w.server.set("/sets/a.list", UNREADABLE);
    // `bad domain` and `*.wild.example` are no names
    w.server.set(
        "/sets/names.txt",
        ".a.example\nbad domain\n*.wild.example\nb.example\n",
    );
    // the server has nothing at /sets/absent.list: that set never loads
    let extra = format!(
        "RULE-SET,{},DIRECT\nDOMAIN-SET,{},DIRECT\nRULE-SET,{},DIRECT",
        w.url("/sets/a.list"),
        w.url("/sets/names.txt"),
        w.url("/sets/absent.list")
    );
    let g = generation(&w, &extra).await;
    let sources = profile_sources(&w, false);
    let key = w.key("/sets/a.list");
    for path in ["/sets/a.list", "/sets/names.txt"] {
        wait_until("the set to load", || {
            set_version(&g.engine, &w.key(path)) >= 1
        })
        .await;
    }

    let listing = g.engine.external_resources(Some(&sources));
    let unused = |path: &str| item(&listing, &w.key(path)).unused;
    let lines = |skipped, unknown_params| {
        Some(UnusedLines {
            skipped,
            truncated: 0,
            unknown_params,
        })
    };
    assert_eq!(unused("/sets/a.list"), lines(3, 1));
    assert_eq!(unused("/sets/names.txt"), lines(2, 0));
    // read whole, and not read at all: none
    for path in ["/rules.list", "/domains.txt", "/sets/absent.list"] {
        assert_eq!(unused(path), lines(0, 0), "{path}");
    }
    // the sets that are files and the sets that are not say it alike; the
    // sources of the profile, the subscription and the databases are not
    // read line by line and do not
    for resource in &listing {
        let a_set = matches!(resource.kind, "ruleset" | "domainset");
        assert_eq!(resource.unused.is_some(), a_set, "{resource:?}");
    }
    assert_eq!(kinds(&listing)[..2], ["profile", "include"]);
    // and a set that has lines it could not use has loaded: there is no error
    assert_eq!(item(&listing, &key).error, None);

    // the next load brings other numbers
    serve_the_set_anew(&w, &g.engine, &key, FEWER, 2).await;
    let listing = g.engine.external_resources(Some(&sources));
    assert_eq!(item(&listing, &key).unused, lines(2, 0));
}

/// `disable-geoip-db-auto-update = true` (manual, IP Rules) stops the
/// downloads of GeoIP databases that are there: a start with both in the data
/// directory asks the download addresses for nothing, where the same start
/// without the key asks for both.
#[tokio::test]
async fn the_geoip_databases_that_are_there_are_fetched_again_unless_the_profile_says_not() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rurge-rules/tests/fixtures");
    for (general, fetched) in [("disable-geoip-db-auto-update = true\n", false), ("", true)] {
        let server = TestServer::spawn().await;
        let dir = tempfile::tempdir().unwrap();
        let geoip = dir.path().join("geoip");
        std::fs::create_dir_all(&geoip).unwrap();
        for (from, to) in [
            ("GeoIP2-Country-Test.mmdb", "GeoLite2-Country.mmdb"),
            ("GeoLite2-ASN-Test.mmdb", "GeoLite2-ASN.mmdb"),
        ] {
            std::fs::copy(fixtures.join(from), geoip.join(to)).unwrap();
        }
        let profile = format!("[General]\n{general}[Proxy]\n[Rule]\nFINAL,DIRECT\n");
        let _runtime = online_runtime(dir.path(), &profile, &server, EngineShared::default()).await;
        if fetched {
            wait_until("both databases to be asked for", || {
                server.hits("/geo/country.mmdb") >= 1 && server.hits("/geo/asn.mmdb") >= 1
            })
            .await;
        } else {
            // the longest a first request takes to arrive, with a margin
            tokio::time::sleep(NOTHING_MORE).await;
            assert_eq!(server.requests().len(), 0, "{general:?}");
        }
    }
}
