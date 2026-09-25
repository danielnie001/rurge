//! The automatic groups through the whole engine (phase 2 M3 design 6.3,
//! 6.4, 10): a dial asks for a round of tests, the round runs on its own,
//! every test is a session of the request log, and the groups pick by the
//! results.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::session::SessionInfo;
use rurge_engine::RequestRecord;
use rurge_inbound::{DialError, Dialer};
use std::collections::HashSet;

/// A loopback port nothing listens on.
async fn closed_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

fn session(host: &str) -> SessionInfo {
    SessionInfo::tcp(HostName::parse(host), 80)
}

/// The chain of a dial of `host` that connects.
async fn chain_of(h: &Harness, host: &str) -> Vec<String> {
    match h.engine.dial(session(host)).await {
        Ok(dialed) => dialed.handle.policy_chain(),
        Err(DialError::Failed { message, .. }) => panic!("{host}: {message}"),
        Err(DialError::Reject { kind, .. }) => panic!("{host}: rejected by {}", kind.name()),
    }
}

/// A and B: SOCKS5 upstreams that reach `origin` whatever they are asked
/// for, A tested at `/slow` (300 ms late), B at `/fast`. Dead: its server
/// refuses.
async fn upstreams(origin: &TestServer) -> (FakeSocks5, FakeSocks5, String) {
    origin.set("/slow", "");
    origin.set("/fast", "");
    origin.set_delay("/slow", Duration::from_millis(300));
    let script = || Socks5Script {
        connect_to: Some(origin_addr(origin)),
        ..Socks5Script::default()
    };
    let (a, b) = (
        FakeSocks5::spawn(script()).await,
        FakeSocks5::spawn(script()).await,
    );
    let proxies = format!(
        "A = socks5, 127.0.0.1, {}, test-url={}\nB = socks5, 127.0.0.1, {}, test-url={}\n\
         Dead = socks5, 127.0.0.1, {}",
        a.addr().port(),
        origin.url("/slow"),
        b.addr().port(),
        origin.url("/fast"),
        closed_port().await
    );
    (a, b, proxies)
}

async fn round_of(h: &Harness, group: &str) {
    wait_until(&format!("a round of tests of {group}"), || {
        h.engine.registry().auto().last_round(group).is_some()
    })
    .await;
}

#[tokio::test]
async fn url_test_moves_to_the_quicker_member_after_a_round() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "U = url-test, A, B",
        rules: "DOMAIN,target.test,U",
        ..Profile::default()
    })
    .await;
    // no results yet: the first member; the dial asks for a round
    assert_eq!(chain_of(&h, "target.test").await, ["U", "A"]);
    round_of(&h, "U").await;
    // A answers 300 ms late: B is quicker by more than the tolerance
    assert_eq!(chain_of(&h, "target.test").await, ["U", "B"]);
}

#[tokio::test]
async fn fallback_passes_over_a_member_that_fails_its_test() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "F = fallback, Dead, A, B",
        rules: "DOMAIN,target.test,F",
        ..Profile::default()
    })
    .await;
    // no results yet: the first member, whose server refuses
    assert!(matches!(
        h.engine.dial(session("target.test")).await,
        Err(DialError::Failed { .. })
    ));
    round_of(&h, "F").await;
    assert_eq!(chain_of(&h, "target.test").await, ["F", "A"]);
}

#[tokio::test]
async fn load_balance_with_persistent_keeps_each_host_on_one_member() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "L = load-balance, Dead, A, B, persistent=true",
        rules: "DOMAIN-SUFFIX,lb.test,L",
        ..Profile::default()
    })
    .await;
    // any member may take the first dial: it only has to ask for the round
    let _ = h.engine.dial(session("first.lb.test")).await;
    round_of(&h, "L").await;
    let mut members = HashSet::new();
    for i in 0..20 {
        let host = format!("h{i}.lb.test");
        let chain = chain_of(&h, &host).await;
        for _ in 0..3 {
            assert_eq!(chain_of(&h, &host).await, chain, "{host} moved");
        }
        members.insert(chain[1].clone());
    }
    // Dead never passes; the hosts spread over the two that do
    assert_eq!(members, HashSet::from(["A".to_string(), "B".to_string()]));
}

#[tokio::test]
async fn evaluate_before_use_waits_for_the_first_round() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "E = fallback, Dead, A, evaluate-before-use=true\n\
                 N = fallback, Dead, evaluate-before-use=true",
        rules: "DOMAIN,e.test,E\nDOMAIN,n.test,N",
        ..Profile::default()
    })
    .await;
    // the first dial waits for the round instead of trying Dead
    assert_eq!(chain_of(&h, "e.test").await, ["E", "A"]);
    match h.engine.dial(session("n.test")).await {
        Err(DialError::Failed {
            message, handle, ..
        }) => {
            assert_eq!(message, "policy group evaluation failed");
            assert_eq!(handle.policy_chain(), ["N"]);
        }
        Err(DialError::Reject { .. }) => panic!("expected a failure, got a reject"),
        Ok(_) => panic!("expected a failure, got a stream"),
    }
}

#[tokio::test]
async fn every_test_is_a_session_of_the_request_log() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "F = fallback, Dead, A",
        rules: "DOMAIN,target.test,F",
        ..Profile::default()
    })
    .await;
    let _ = h.engine.dial(session("target.test")).await;
    round_of(&h, "F").await;
    let tests: Vec<RequestRecord> = h
        .engine
        .request_log()
        .recent(100)
        .into_iter()
        .filter(|r| r.rule.as_deref() == Some("policy test"))
        .collect();
    let a = tests.iter().find(|r| r.policy == ["A"]).expect("A tested");
    assert_eq!(a.listener, ListenerKind::Internal);
    // the test URL's host and port, nothing of its path
    assert_eq!(a.dst, origin_addr(&origin).to_string());
    assert_eq!(a.status, RecordStatus::Completed);
    let dead = tests
        .iter()
        .find(|r| r.policy == ["Dead"])
        .expect("Dead tested");
    assert_eq!(dead.status, RecordStatus::Failed);
    assert!(
        dead.error
            .as_deref()
            .is_some_and(|e| e.starts_with("connect: ")),
        "{:?}",
        dead.error
    );
}

/// A group's own `underlying-proxy` makes its members `M (via R)`; such a
/// member is tested through the relay, as it is dialled (M3 design 5.4).
#[tokio::test]
async fn a_derived_member_is_tested_through_its_relay() {
    let origin = TestServer::spawn().await;
    let (a, _b, proxies) = upstreams(&origin).await;
    // whatever it is asked for, the relay tunnels to exactly that
    let relay = FakeSocks5::spawn(Socks5Script::default()).await;
    let h = harness(Profile {
        proxies: &format!("{proxies}\nR = socks5, 127.0.0.1, {}", relay.addr().port()),
        groups: "U = url-test, A, underlying-proxy=R",
        rules: "DOMAIN,target.test,U",
        ..Profile::default()
    })
    .await;
    assert_eq!(chain_of(&h, "target.test").await, ["U", "A (via R)"]);
    round_of(&h, "U").await;
    let result = h
        .engine
        .registry()
        .test_result("A (via R)")
        .expect("tested");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome);
    // the relay carried both the dial and the test to A's server
    let to_a = relay
        .requests()
        .iter()
        .filter(|r| r.port == a.addr().port())
        .count();
    assert_eq!(to_a, 2);
}

/// An override is what the group dials, and while it stands the group
/// asks for no round of tests (M3 design 6.3, 6.5); clearing it gives the
/// group back to its tests.
#[tokio::test]
async fn select_on_an_automatic_group_overrides_it_until_cleared() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "U = url-test, A, B",
        rules: "DOMAIN,target.test,U",
        ..Profile::default()
    })
    .await;
    h.engine.select_group("U", "B").await.unwrap();
    assert_eq!(chain_of(&h, "target.test").await, ["U", "B"]);
    let auto = h.engine.registry().auto().clone();
    assert!(auto.requested().is_empty() && auto.last_round("U").is_none());
    h.engine.select_group("U", "").await.unwrap();
    assert_eq!(chain_of(&h, "target.test").await, ["U", "A"]);
    round_of(&h, "U").await;
}

/// An override stands while the group is defined the same way (M3 design
/// 6.5): a reload of the same profile keeps it, one that changes the group
/// drops it.
#[tokio::test]
async fn a_reload_keeps_an_override_while_the_group_stays_the_same() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "U = url-test, A, B",
        ..Profile::default()
    })
    .await;
    let registry = h.engine.registry();
    registry
        .auto()
        .set_override(registry.group_spec("U").unwrap(), "B");
    let current = || h.engine.registry().current_member("U");
    assert_eq!(current().as_deref(), Some("B"));
    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    h.engine
        .swap_runtime(runtime(h.dir.path(), &text, h.engine.shared()).await);
    assert_eq!(current().as_deref(), Some("B"));
    let changed = text.replace("U = url-test, A, B", "U = url-test, A, B, interval=60");
    h.engine
        .swap_runtime(runtime(h.dir.path(), &changed, h.engine.shared()).await);
    assert_eq!(
        current().as_deref(),
        Some("A"),
        "no results: the first member"
    );
}
