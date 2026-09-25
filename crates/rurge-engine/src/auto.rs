//! The engine's side of the automatic groups (phase 2 M3 design 6.2, 6.3):
//! the task that runs the rounds of tests the registry asks for, every test
//! a session of the request log, and a session dial's wait for the first
//! round of an `evaluate-before-use` group — a DNS session does not wait:
//! the round it would wait for may itself need that very lookup.

use crate::engine::Engine;
use rurge_config::HostName;
use rurge_config::rule::PolicyRef;
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_inbound::{SessionHandle, SessionOutcome};
use rurge_policy::auto::SelectCtx;
use rurge_policy::testbook::{TestObserver, TestRecord};
use rurge_policy::{PolicyRegistry, Resolution};
use std::sync::{Arc, Weak};
use std::time::Duration;
use url::Url;

/// The rule a test session shows in the request log.
pub(crate) const TEST_RULE: &str = "policy test";

/// Why a session through an `evaluate-before-use` group fails when no
/// member passes the group's first round of tests (M3 design 6.3).
pub(crate) const EVALUATION_FAILED: &str = "policy group evaluation failed";

/// Every connectivity test is a session of the request log (M3 design
/// 6.2): `Internal`, the rule `policy test`, the tested policy for its
/// chain, and for its target the test URL's host and port — never the rest
/// of the URL, which a subscription line may have set (M3-D7).
struct TestSessions(Weak<Engine>);

struct TestSession(Arc<SessionHandle>);

/// A test that begins while the engine goes away.
struct Unrecorded;

impl TestObserver for TestSessions {
    fn begin(&self, policy: &str, url: &Url) -> Box<dyn TestRecord> {
        let Some(engine) = self.0.upgrade() else {
            return Box::new(Unrecorded);
        };
        let host = HostName::parse(url.host_str().unwrap_or_default());
        let mut session = SessionInfo::tcp(host, url.port_or_known_default().unwrap_or(0));
        session.listener = ListenerKind::Internal;
        let handle = engine.new_handle(session);
        handle.set_rule(Some(TEST_RULE.to_string()));
        handle.set_policy_chain(vec![policy.to_string()]);
        Box::new(TestSession(handle))
    }
}

impl TestRecord for TestSession {
    fn end(self: Box<Self>, outcome: &Result<Duration, String>) {
        self.0.finish(match outcome {
            Ok(_) => SessionOutcome::Completed,
            Err(why) => SessionOutcome::Failed(why.clone()),
        });
    }
}

impl TestRecord for Unrecorded {
    fn end(self: Box<Self>, _outcome: &Result<Duration, String>) {}
}

impl Engine {
    /// Starts the task that runs the rounds of tests the registry asks for,
    /// each against the registry in use when the round starts (M3 design
    /// 6.3); it ends with the engine. Called once, by `Engine::new`.
    pub(crate) fn start_tests(self: &Arc<Self>) {
        let auto = self.shared().auto;
        auto.tests
            .observe(Arc::new(TestSessions(Arc::downgrade(self))));
        let mut requests = auto.connect();
        let engine = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(group) = requests.recv().await {
                let Some(registry) = engine.upgrade().map(|e| e.registry()) else {
                    break;
                };
                tokio::spawn(async move {
                    registry.test_group(&group).await;
                });
            }
        });
    }
}

/// `registry.resolve_with`, and — when an `evaluate-before-use` group on
/// the way has not had its first round of tests yet — the same again once
/// that round has ended, waiting at most as long as the round may take (M3
/// design 6.3, 9). `Err`: no member of that group passes after the wait; it
/// carries the chain up to the group.
pub(crate) async fn resolve_ready(
    registry: &PolicyRegistry,
    policy: &PolicyRef,
    ctx: &SelectCtx,
) -> Result<Resolution, Vec<String>> {
    let first = registry.resolve_with(policy, ctx);
    let Some(group) = first.pending.clone() else {
        return Ok(first);
    };
    let auto = registry.auto();
    // subscribed before looking: a round that ends in between is not missed
    let mut rounds = auto.rounds();
    let _ = tokio::time::timeout(registry.round_timeout(&group), async {
        while auto.last_round(&group).is_none() {
            if rounds.changed().await.is_err() {
                break;
            }
        }
    })
    .await;
    if registry.available(&group).is_empty() {
        let mut chain = first.chain;
        if let Some(i) = chain.iter().position(|name| *name == group) {
            chain.truncate(i + 1);
        }
        return Err(chain);
    }
    Ok(registry.resolve_with(policy, ctx))
}
