//! `policy-path` subscriptions through the whole engine (phase 2 M3 design
//! §5): what the registry holds from the first build on.

mod common;
use common::*;
use rurge_config::session::SessionInfo;
use rurge_engine::EmptyGroup;
use rurge_inbound::{DialError, Dialer};

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
