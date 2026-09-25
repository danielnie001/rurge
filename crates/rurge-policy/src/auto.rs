//! The automatic groups — `url-test`, `fallback`, `load-balance` (phase 2
//! M3 design 6.3 – 6.5): how each one picks from its members' test results,
//! and what they keep across config generations — the temporary overrides,
//! the member `url-test` holds on to, when each group was last tested — plus
//! the way the registry asks the engine for a new round of tests.

use crate::testbook::TestBook;
use rurge_config::Span;
use rurge_config::spec::{GroupSpec, TestOpts};
use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

/// What the dial knows about a session that a group may pick by.
#[derive(Clone, Debug, Default)]
pub struct SelectCtx {
    /// The target's host name: `load-balance` with `persistent=true` sends
    /// one host to one member.
    pub host: Option<String>,
}

/// A member's standing in the tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Its last test passed with this score.
    Passed(Duration),
    /// Its last test failed, or it can never pass (a REJECT).
    Failed,
    /// Not tested yet.
    Unknown,
}

impl Standing {
    /// Passed, and below the group's `timeout` when it has one (M3 design
    /// 6.4).
    pub fn passes(self, opts: &TestOpts) -> Option<Duration> {
        match self {
            Standing::Passed(score) if opts.timeout.is_none_or(|t| score < t) => Some(score),
            _ => None,
        }
    }
}

/// `url-test`: the fastest member that passes — but the member it holds
/// (`current`) stays while it passes and the fastest is not quicker than
/// it by more than `tolerance`. The first member when none passes.
pub fn url_test(
    members: &[(String, Standing)],
    current: Option<&str>,
    opts: &TestOpts,
) -> Option<String> {
    let mut best: Option<(&str, Duration)> = None;
    for (name, standing) in members {
        if let Some(score) = standing.passes(opts)
            && best.is_none_or(|(_, b)| score < b)
        {
            best = Some((name, score));
        }
    }
    let Some((best, best_score)) = best else {
        return members.first().map(|(name, _)| name.clone());
    };
    let held = current.and_then(|current| {
        members
            .iter()
            .find(|(name, _)| name == current)
            .and_then(|(name, standing)| standing.passes(opts).map(|score| (name, score)))
    });
    match held {
        Some((name, score)) if score.saturating_sub(best_score) <= opts.tolerance => {
            Some(name.clone())
        }
        _ => Some(best.to_string()),
    }
}

/// `fallback`: the first member, in order, that passes; the first member
/// when none does.
pub fn fallback(members: &[(String, Standing)], opts: &TestOpts) -> Option<String> {
    members
        .iter()
        .find(|(_, standing)| standing.passes(opts).is_some())
        .or(members.first())
        .map(|(name, _)| name.clone())
}

/// `load-balance`: any member that passes — every member when none does —
/// at random, or, with `persistent`, the one the target host hashes to.
pub fn load_balance(
    members: &[(String, Standing)],
    opts: &TestOpts,
    ctx: &SelectCtx,
) -> Option<String> {
    let passing: Vec<&str> = members
        .iter()
        .filter(|(_, standing)| standing.passes(opts).is_some())
        .map(|(name, _)| name.as_str())
        .collect();
    let candidates: Vec<&str> = if passing.is_empty() {
        members.iter().map(|(name, _)| name.as_str()).collect()
    } else {
        passing
    };
    if candidates.is_empty() {
        return None;
    }
    let index = match (&ctx.host, opts.persistent) {
        (Some(host), true) => {
            // a fixed hasher: the same host goes to the same member for as
            // long as the candidates stay the same
            let mut h = DefaultHasher::new();
            host.hash(&mut h);
            h.finish()
        }
        _ => RandomState::new().hash_one(Instant::now()),
    } as usize
        % candidates.len();
    Some(candidates[index].to_string())
}

struct Override {
    member: String,
    /// The group as it was when the override was set, span left out: a
    /// reload that changes the group drops the override (M3 design 6.5).
    spec: GroupSpec,
    warned: bool,
}

#[derive(Default)]
struct State {
    overrides: HashMap<String, Override>,
    /// The member each `url-test` group holds on to.
    picks: HashMap<String, String>,
    /// When each group's last round of tests ended.
    rounds: HashMap<String, Instant>,
    /// Groups a round was asked for that has not run yet.
    requested: HashSet<String>,
}

/// The automatic groups' state, one per engine: it outlives the config
/// generations, as the test results do.
pub struct AutoGroups {
    pub tests: Arc<TestBook>,
    state: Mutex<State>,
    wake: Mutex<Option<mpsc::UnboundedSender<String>>>,
    rounds: watch::Sender<u64>,
}

fn without_span(spec: &GroupSpec) -> GroupSpec {
    GroupSpec {
        span: Span::new(Arc::from(Path::new("")), 0),
        ..spec.clone()
    }
}

impl AutoGroups {
    pub fn new(tests: Arc<TestBook>) -> AutoGroups {
        AutoGroups {
            tests,
            state: Mutex::default(),
            wake: Mutex::default(),
            rounds: watch::Sender::new(0),
        }
    }

    /// Where requests for a round of tests go from now on: the engine's
    /// scheduler, which runs `PolicyRegistry::test_group` for each name it
    /// receives. Until then requests only pile up in `requested`.
    pub fn connect(&self) -> mpsc::UnboundedReceiver<String> {
        let (tx, rx) = mpsc::unbounded_channel();
        // `state` stays locked from copying `requested` until the sender is
        // installed in `wake` (lock order state -> wake; `wake()` takes the
        // two one after the other and never holds both, so there is no
        // cycle). A `wake` that lands in between is then either already in
        // this copy or finds the sender.
        let state = self.state.lock().expect("auto groups");
        for group in state.requested.iter().cloned() {
            let _ = tx.send(group);
        }
        *self.wake.lock().expect("wake") = Some(tx);
        drop(state);
        rx
    }

    /// Asks for a round of tests of `group`, unless one is asked for
    /// already.
    pub(crate) fn wake(&self, group: &str) {
        if !self
            .state
            .lock()
            .expect("auto groups")
            .requested
            .insert(group.to_string())
        {
            return;
        }
        if let Some(tx) = self.wake.lock().expect("wake").as_ref() {
            let _ = tx.send(group.to_string());
        }
    }

    /// The groups a round was asked for that has not run yet.
    pub fn requested(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .state
            .lock()
            .expect("auto groups")
            .requested
            .iter()
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// A round of tests of `groups` has ended.
    pub(crate) fn round_done(&self, groups: &[String]) {
        {
            let mut state = self.state.lock().expect("auto groups");
            let now = Instant::now();
            for group in groups {
                state.rounds.insert(group.clone(), now);
                state.requested.remove(group);
            }
        }
        self.rounds.send_modify(|n| *n += 1);
    }

    /// When the last round of tests of `group` ended.
    pub fn last_round(&self, group: &str) -> Option<Instant> {
        self.state
            .lock()
            .expect("auto groups")
            .rounds
            .get(group)
            .copied()
    }

    /// Changes whenever a round ends: what `evaluate-before-use` waits on.
    pub fn rounds(&self) -> watch::Receiver<u64> {
        self.rounds.subscribe()
    }

    /// Makes `member` the choice of the automatic group `spec` until it is
    /// cleared, the group changes in a reload, or the process ends (M3
    /// design 6.5). The caller has checked that it is a member.
    pub fn set_override(&self, spec: &GroupSpec, member: &str) {
        self.state.lock().expect("auto groups").overrides.insert(
            spec.name.clone(),
            Override {
                member: member.to_string(),
                spec: without_span(spec),
                warned: false,
            },
        );
    }

    /// Whether there was an override to clear.
    pub fn clear_override(&self, group: &str) -> bool {
        self.state
            .lock()
            .expect("auto groups")
            .overrides
            .remove(group)
            .is_some()
    }

    /// The override of `group`, while it names one of `members`; one that
    /// no longer does (a subscription update took the member away) is said
    /// once and ignored.
    pub(crate) fn override_of(&self, group: &str, members: &[String]) -> Option<String> {
        let mut state = self.state.lock().expect("auto groups");
        let o = state.overrides.get_mut(group)?;
        if members.contains(&o.member) {
            return Some(o.member.clone());
        }
        if !o.warned {
            o.warned = true;
            tracing::warn!(group, member = %o.member, "the overriding member is gone from the group; the override has no effect");
        }
        None
    }

    pub(crate) fn pick(&self, group: &str) -> Option<String> {
        self.state
            .lock()
            .expect("auto groups")
            .picks
            .get(group)
            .cloned()
    }

    pub(crate) fn set_pick(&self, group: &str, member: &str) {
        let mut state = self.state.lock().expect("auto groups");
        if state.picks.get(group).is_none_or(|m| m != member) {
            state.picks.insert(group.to_string(), member.to_string());
        }
    }

    /// A new generation of the profile: overrides of groups that are gone
    /// or defined differently now go, and so does what is kept for groups
    /// that are gone (M3 design 6.5).
    pub fn retain(&self, groups: &[GroupSpec]) {
        let by_name: HashMap<&str, GroupSpec> = groups
            .iter()
            .map(|g| (g.name.as_str(), without_span(g)))
            .collect();
        let mut state = self.state.lock().expect("auto groups");
        state
            .overrides
            .retain(|group, o| by_name.get(group.as_str()) == Some(&o.spec));
        state
            .picks
            .retain(|group, _| by_name.contains_key(group.as_str()));
        state
            .rounds
            .retain(|group, _| by_name.contains_key(group.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};

    fn ms(n: u64) -> Standing {
        Standing::Passed(Duration::from_millis(n))
    }

    fn members(list: &[(&str, Standing)]) -> Vec<(String, Standing)> {
        list.iter().map(|(n, s)| (n.to_string(), *s)).collect()
    }

    fn opts(tolerance: u64, timeout: Option<u64>) -> TestOpts {
        TestOpts {
            tolerance: Duration::from_millis(tolerance),
            timeout: timeout.map(Duration::from_millis),
            ..TestOpts::default()
        }
    }

    #[test]
    fn url_test_holds_its_member_within_the_tolerance() {
        let m = members(&[("A", ms(120)), ("B", ms(50)), ("C", Standing::Failed)]);
        // no member held yet: the fastest
        assert_eq!(url_test(&m, None, &opts(100, None)).as_deref(), Some("B"));
        // A is 70 ms slower than B: within 100 ms, A stays
        assert_eq!(
            url_test(&m, Some("A"), &opts(100, None)).as_deref(),
            Some("A")
        );
        // with tolerance 0 every change goes to the fastest
        assert_eq!(
            url_test(&m, Some("A"), &opts(0, None)).as_deref(),
            Some("B")
        );
        // a held member that fails is dropped
        assert_eq!(
            url_test(&m, Some("C"), &opts(100, None)).as_deref(),
            Some("B")
        );
        // `timeout` takes B out of the running
        assert_eq!(
            url_test(&m, None, &opts(100, Some(100))).as_deref(),
            Some("B")
        );
        assert_eq!(
            url_test(
                &members(&[("A", ms(120)), ("B", ms(150))]),
                None,
                &opts(100, Some(100))
            )
            .as_deref(),
            Some("A"),
            "none passes: the first member"
        );
        assert_eq!(url_test(&[], None, &opts(100, None)), None);
    }

    #[test]
    fn fallback_takes_the_first_that_passes() {
        let m = members(&[
            ("A", Standing::Failed),
            ("B", Standing::Unknown),
            ("C", ms(300)),
            ("D", ms(10)),
        ]);
        assert_eq!(fallback(&m, &opts(0, None)).as_deref(), Some("C"));
        assert_eq!(
            fallback(&m, &opts(0, Some(200))).as_deref(),
            Some("D"),
            "C is not under the timeout"
        );
        assert_eq!(
            fallback(
                &members(&[("A", Standing::Failed), ("B", Standing::Failed)]),
                &opts(0, None)
            )
            .as_deref(),
            Some("A")
        );
    }

    #[test]
    fn load_balance_spreads_over_those_that_pass() {
        let m = members(&[("A", ms(10)), ("B", Standing::Failed), ("C", ms(20))]);
        let plain = TestOpts::default();
        let any = SelectCtx::default();
        for _ in 0..50 {
            let pick = load_balance(&m, &plain, &any).unwrap();
            assert!(pick == "A" || pick == "C", "{pick}");
        }
        // none passes: every member is a candidate
        let failed = members(&[("A", Standing::Failed), ("B", Standing::Unknown)]);
        let seen: HashSet<String> = (0..200)
            .map(|_| load_balance(&failed, &plain, &any).unwrap())
            .collect();
        assert_eq!(seen.len(), 2);
        // persistent: one host, one member
        let sticky = TestOpts {
            persistent: true,
            ..TestOpts::default()
        };
        let ctx = SelectCtx {
            host: Some("example.com".into()),
        };
        let first = load_balance(&m, &sticky, &ctx).unwrap();
        for _ in 0..20 {
            assert_eq!(load_balance(&m, &sticky, &ctx).unwrap(), first);
        }
        assert_eq!(load_balance(&[], &plain, &any), None);
    }

    fn spec_of(text: &str, group: &str) -> GroupSpec {
        let loaded = from_text(
            text,
            std::path::Path::new("t.conf"),
            &LoadOptions::for_tests(),
        );
        assert!(!loaded.diagnostics.has_errors());
        loaded
            .config
            .group_specs
            .iter()
            .find(|g| g.name == group)
            .cloned()
            .unwrap()
    }

    fn auto() -> AutoGroups {
        AutoGroups::new(Arc::new(TestBook::new()))
    }

    /// An override lasts while the group is defined the same way: a reload
    /// that moves the line (another span) keeps it, one that changes the
    /// group or removes it drops it.
    #[test]
    fn an_override_lasts_while_the_group_stays_the_same() {
        let a =
            "[Proxy]\nA = direct\nB = direct\n[Proxy Group]\nU = url-test, A, B\n[Rule]\nFINAL,U\n";
        let moved = "[Proxy]\nA = direct\nB = direct\n\n\n[Proxy Group]\nU = url-test, A, B\n[Rule]\nFINAL,U\n";
        let changed = "[Proxy]\nA = direct\nB = direct\n[Proxy Group]\nU = url-test, A, B, interval=60\n[Rule]\nFINAL,U\n";
        let auto = auto();
        let members = ["A".to_string(), "B".to_string()];
        auto.set_override(&spec_of(a, "U"), "B");
        auto.retain(&[spec_of(moved, "U")]);
        assert_eq!(auto.override_of("U", &members).as_deref(), Some("B"));
        auto.retain(&[spec_of(changed, "U")]);
        assert_eq!(auto.override_of("U", &members), None);
        auto.set_override(&spec_of(a, "U"), "B");
        auto.retain(&[]);
        assert_eq!(auto.override_of("U", &members), None);
        // an override whose member is gone does nothing
        auto.set_override(&spec_of(a, "U"), "B");
        assert_eq!(auto.override_of("U", &["A".to_string()]), None);
        assert!(auto.clear_override("U"));
        assert!(!auto.clear_override("U"));
    }

    /// A round is asked for once until it has run; the requests wait for
    /// the scheduler to connect.
    #[tokio::test]
    async fn a_round_is_asked_for_once_until_it_runs() {
        let auto = auto();
        auto.wake("U");
        auto.wake("U");
        assert_eq!(auto.requested(), ["U"]);
        let mut rx = auto.connect();
        assert_eq!(rx.recv().await.as_deref(), Some("U"));
        auto.wake("U");
        assert!(rx.try_recv().is_err(), "still asked for");
        let mut rounds = auto.rounds();
        auto.round_done(&["U".to_string()]);
        assert!(auto.requested().is_empty());
        assert!(auto.last_round("U").is_some());
        rounds.changed().await.unwrap();
        auto.wake("U");
        assert_eq!(rx.recv().await.as_deref(), Some("U"));
    }
}
