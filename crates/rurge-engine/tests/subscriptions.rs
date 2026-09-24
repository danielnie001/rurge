//! `policy-path` subscriptions through the whole engine (phase 2 M3 design
//! §5): what the registry holds from the first build on.

mod common;
use common::*;
use rurge_config::rule::PolicyRef;
use rurge_config::session::SessionInfo;
use rurge_engine::EmptyGroup;
use rurge_inbound::{DialError, Dialer};

/// Like `common::wait_until`, with room for a file watcher's or a refresh
/// interval's delay plus the rebuild's own pause.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn members(engine: &Engine, group: &str) -> Vec<String> {
    engine
        .registry()
        .members(group)
        .unwrap_or_default()
        .to_vec()
}

fn outbound_of(engine: &Engine, name: &str) -> rurge_proto::OutboundRef {
    engine.registry().resolve(&PolicyRef::parse(name)).outbound
}

/// A generation that downloads: its GeoIP updater asks `server` (and gets a
/// 404) instead of the public default URLs.
async fn online_runtime(
    dir: &std::path::Path,
    profile: &str,
    server: &TestServer,
    shared: EngineShared,
) -> Runtime {
    std::fs::write(dir.join("t.conf"), profile).unwrap();
    let loaded = from_text(profile, &dir.join("t.conf"), &LoadOptions::for_tests());
    assert!(!loaded.diagnostics.has_errors());
    let mut stack = stack_options(dir);
    stack.no_network = false;
    stack.geo_urls = GeoUrls {
        country: server.url("/geo/country.mmdb"),
        asn: server.url("/geo/asn.mmdb"),
    };
    let opts = RuntimeOptions {
        stack,
        outbound_mode: OutboundMode::Rule,
        idle_timeout: Duration::from_secs(600),
        shared,
        request_log_size: 1000,
    };
    Runtime::build(loaded.config, opts).await.unwrap()
}

/// A local subscription is read while the generation is built: the group
/// has its members from the first dial on (M3-D5).
#[tokio::test]
async fn a_subscription_file_is_in_from_the_first_generation() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("nodes.txt"),
        "N1 = http, n1.test, 80\nN2 = http, n2.test, 80\n",
    )
    .unwrap();
    let dns = MockDns::spawn().await;
    let text = Profile {
        groups: "Sub = select, policy-path=nodes.txt",
        ..Profile::default()
    }
    .text(dns.addr());
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
    assert_eq!(engine.registry().members("Sub").unwrap(), ["N1", "N2"]);
}

/// An edit of the file rebuilds the registry alone: the members follow, and
/// a line that did not change keeps its outbound (M3 design 5.7).
#[tokio::test]
async fn an_edited_subscription_file_rebuilds_the_registry() {
    let dir = tempfile::tempdir().unwrap();
    let nodes = dir.path().join("nodes.txt");
    std::fs::write(&nodes, "N1 = http, n1.test, 80\nN2 = http, n2.test, 80\n").unwrap();
    let dns = MockDns::spawn().await;
    let text = Profile {
        groups: "Sub = select, policy-path=nodes.txt",
        ..Profile::default()
    }
    .text(dns.addr());
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
    let generation = engine.runtime();
    let n1 = outbound_of(&engine, "N1");
    std::fs::write(&nodes, "N1 = http, n1.test, 80\nN3 = http, n3.test, 80\n").unwrap();
    eventually("the rebuild", || members(&engine, "Sub") == ["N1", "N3"]).await;
    assert!(Arc::ptr_eq(&n1, &outbound_of(&engine, "N1")));
    assert!(!engine.registry().contains("N2"));
    assert!(
        Arc::ptr_eq(&generation, &engine.runtime()),
        "the generation stays"
    );
}

/// Nothing cached on a first start: the group is empty until the download
/// arrives, then follows the server; a reload finds the download in the
/// cache, so it has the members at once, even offline (M3-D5).
#[tokio::test]
async fn a_url_subscription_arrives_after_the_start_and_is_cached() {
    let server = TestServer::spawn().await;
    server.set("/nodes", "N1 = http, n1.test, 80\nN2 = http, n2.test, 80\n");
    let dir = tempfile::tempdir().unwrap();
    let dns = MockDns::spawn().await;
    let groups = format!(
        "Sub = select, policy-path={}, update-interval=1",
        server.url("/nodes")
    );
    let text = Profile {
        groups: &groups,
        ..Profile::default()
    }
    .text(dns.addr());
    let engine =
        Engine::new(online_runtime(dir.path(), &text, &server, EngineShared::default()).await);
    assert!(members(&engine, "Sub").is_empty(), "nothing is cached yet");
    eventually("the first download", || {
        members(&engine, "Sub") == ["N1", "N2"]
    })
    .await;
    let n1 = outbound_of(&engine, "N1");
    server.set("/nodes", "N1 = http, n1.test, 80\nN3 = http, n3.test, 80\n");
    eventually("the update", || members(&engine, "Sub") == ["N1", "N3"]).await;
    assert!(Arc::ptr_eq(&n1, &outbound_of(&engine, "N1")));

    // `runtime` builds offline: only the cache can give the members now
    engine.swap_runtime(runtime(dir.path(), &text, engine.shared()).await);
    assert_eq!(members(&engine, "Sub"), ["N1", "N3"]);
}

/// A dial through a group without members: DIRECT stands in by default,
/// REJECT with `--empty-group-reject`; the session says which (M3-D3).
#[tokio::test]
async fn an_empty_group_stands_in_direct_or_rejects() {
    let origin = TestServer::spawn().await;
    let profile = || Profile {
        groups: "Sub = select, policy-path=missing.txt",
        rules: "DOMAIN,empty.test,Sub",
        ..Profile::default()
    };
    let session = || {
        SessionInfo::tcp(
            rurge_config::HostName::parse("empty.test"),
            origin_addr(&origin).port(),
        )
    };

    let h = harness(profile()).await;
    h.dns.set("empty.test", &["127.0.0.1"], &[], 60);
    let Ok(dialed) = h.engine.dial(session()).await else {
        panic!("DIRECT stands in")
    };
    assert_eq!(
        dialed.handle.error().as_deref(),
        Some("policy group has no members; DIRECT substituted")
    );
    assert_eq!(dialed.handle.policy_chain(), ["Sub", "DIRECT"]);

    let shared = EngineShared {
        empty_group: EmptyGroup::Reject,
        ..EngineShared::default()
    };
    let h = harness_with(profile(), shared).await;
    match h.engine.dial(session()).await {
        Err(DialError::Reject { kind, handle, .. }) => {
            assert_eq!(kind, rurge_proto::RejectKind::Reject);
            assert_eq!(
                handle.error().as_deref(),
                Some("policy group has no members")
            );
        }
        Err(DialError::Failed { message, .. }) => panic!("expected a reject, failed: {message}"),
        Ok(_) => panic!("expected a reject, got a stream"),
    }
}

/// The control plane sees what the registry holds: imported and derived
/// policies, with their secrets blanked (M3 design §8, M3-D7).
#[tokio::test]
async fn the_views_show_imported_and_derived_policies() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("nodes.txt"),
        "N1 = trojan, n1.test, 443, password=s3cret\nN2 = http, n2.test, 80\n",
    )
    .unwrap();
    let dns = MockDns::spawn().await;
    let text = Profile {
        proxies: "R = http, r.test, 80",
        groups: "Sub = select, DIRECT, policy-path=nodes.txt\n\
Chained = select, include-other-group=Sub, underlying-proxy=R",
        ..Profile::default()
    }
    .text(dns.addr());
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);

    let proxies = engine.policies_view().proxies;
    assert_eq!(
        proxies[5..],
        ["R", "N1", "N2", "N1 (via R)", "N2 (via R)"],
        "{proxies:?}"
    );
    let groups = engine.groups_view();
    let chained = groups.iter().find(|g| g.name == "Chained").unwrap();
    let members: Vec<(&str, &str)> = chained
        .members
        .iter()
        .map(|m| (m.name.as_str(), m.type_description.as_str()))
        .collect();
    assert_eq!(
        members,
        [
            ("DIRECT", "DIRECT"),
            ("N1 (via R)", "trojan"),
            ("N2 (via R)", "http")
        ]
    );
    assert_eq!(
        engine.policy_detail("N1").as_deref(),
        Some("trojan, n1.test, 443, password=***")
    );
    assert_eq!(
        engine.policy_detail("N1 (via R)").as_deref(),
        Some("trojan, n1.test, 443, password=***, underlying-proxy=R")
    );
    assert_eq!(
        engine.policy_detail("Sub").as_deref(),
        Some("select, DIRECT, policy-path=***")
    );

    engine.select_group("Sub", "N2").await.unwrap();
    assert_eq!(engine.group_selection("Sub").unwrap(), "N2");
    assert_eq!(
        engine.select_group("Sub", "N1 (via R)").await,
        Err(SelectError::NotAMember {
            group: "Sub".into(),
            member: "N1 (via R)".into()
        })
    );
}
