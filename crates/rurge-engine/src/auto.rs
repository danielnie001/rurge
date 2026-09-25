//! The engine's side of the automatic groups (phase 2 M3 design 6.2, 6.3):
//! the task that runs the rounds of tests the registry asks for, every test
//! a session of the request log, and a session dial's wait for the first
//! round of an `evaluate-before-use` group — a DNS session does not wait:
//! the round it would wait for may itself need that very lookup.

use crate::engine::{Engine, UnknownPolicy, policy_known};
use crate::views::SelectError;
use rurge_config::rule::PolicyRef;
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_config::{GroupKind, HostName};
use rurge_inbound::{SessionHandle, SessionOutcome};
use rurge_policy::auto::SelectCtx;
use rurge_policy::testbook::{TestObserver, TestRecord, TestResult};
use rurge_policy::{PolicyRegistry, Resolution};
use std::sync::{Arc, Weak};
use std::time::Duration;
use url::Url;

/// The rule a test session shows in the request log.
pub(crate) const TEST_RULE: &str = "policy test";

/// Why a session through an `evaluate-before-use` group fails when no
/// member passes the group's first round of tests (M3 design 6.3).
pub(crate) const EVALUATION_FAILED: &str = "policy group evaluation failed";

/// Test results by policy (or member), in the order asked for (or
/// listed); `None` where there is none.
pub(crate) type Results = Vec<(String, Option<TestResult>)>;

/// The groups that pick by the tests, and take an override for a
/// selection: `url-test`, `fallback`, `load-balance`.
pub(crate) fn automatic(kind: GroupKind) -> bool {
    matches!(
        kind,
        GroupKind::UrlTest | GroupKind::Fallback | GroupKind::LoadBalance
    )
}

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

    /// Tests `names` now, side by side, whatever their groups' `interval`
    /// (M3 design 6.3, 6.6): each at its own test URL — the groups then
    /// pick by these results — or all at `url`, a one-off whose results
    /// are not kept. `None` for what cannot be tested: a group, a REJECT, a
    /// protocol not implemented yet, a test URL that does not parse.
    pub async fn test_policies(
        &self,
        names: &[String],
        url: Option<Url>,
    ) -> Result<Results, UnknownPolicy> {
        let registry = self.registry();
        if let Some(name) = names.iter().find(|name| !policy_known(&registry, name)) {
            return Err(UnknownPolicy(name.clone()));
        }
        let book = registry.auto().tests.clone();
        let tests: Vec<_> = names
            .iter()
            .map(|name| {
                let (case, book, url) = (registry.test_case(name), book.clone(), url.clone());
                tokio::spawn(async move {
                    let mut case = case?;
                    Some(match url {
                        None => book.test(case).await,
                        Some(url) => {
                            case.url = url;
                            book.test_once(&case).await
                        }
                    })
                })
            })
            .collect();
        let mut out = Vec::with_capacity(names.len());
        for (name, test) in names.iter().zip(tests) {
            out.push((name.clone(), test.await.ok().flatten()));
        }
        Ok(out)
    }

    /// Tests every member of `group` now, whatever its `interval`, and the
    /// members of the groups in it; the members of `group` that pass (M3
    /// design 6.3, 6.6).
    pub async fn test_group(&self, group: &str) -> Result<Vec<String>, SelectError> {
        let registry = self.registry();
        if registry.group(group).is_none() {
            return Err(SelectError::UnknownGroup(group.to_string()));
        }
        Ok(registry.test_group(group).await)
    }

    /// The last test result of every member of every automatic group, the
    /// groups in profile order; `None` for a member without one (not tested
    /// yet, or never tested: a group, a REJECT).
    pub fn test_results(&self) -> Vec<(String, Results)> {
        let registry = self.registry();
        registry
            .group_names()
            .into_iter()
            .filter_map(|name| {
                let group = registry.group(&name)?;
                if !automatic(group.kind) {
                    return None;
                }
                let members = group
                    .members
                    .iter()
                    .map(|m| (m.clone(), registry.test_result(m)))
                    .collect();
                Some((name, members))
            })
            .collect()
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
