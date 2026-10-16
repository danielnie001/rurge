//! The automatic groups — `url-test`, `fallback`, `load-balance` (phase 2
//! M3 design 6.3 – 6.5): how each one picks from its members' test results,
//! and what they keep across config generations — the temporary overrides,
//! the member `url-test` holds on to, when each group was last tested — plus
//! the way the registry asks the engine for a new round of tests. A `subnet`
//! group has no tests, but its temporary override is kept here with the
//! others (phase 3 M1 design 6.2).

use crate::smart::SmartBook;
use crate::testbook::TestBook;
use rurge_config::spec::{GroupSpec, TestOpts};
use rurge_config::{GroupKind, Span};
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
    /// How many times the network has changed (phase 3 M1 design 5).
    epoch: u64,
    /// For each group a network change took the time of the last round from,
    /// the epoch that change began: a round of the group that began in an
    /// earlier epoch says nothing of the network the machine is on now.
    cleared: HashMap<String, u64>,
}

/// The automatic groups' state, one per engine: it outlives the config
/// generations, as the test results do.
pub struct AutoGroups {
    pub tests: Arc<TestBook>,
    /// What the `smart` groups know of their members; it hears of every
    /// result `tests` keeps (phase 2 M3c design 5.3).
    pub smart: Arc<SmartBook>,
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
        let smart = Arc::new(SmartBook::new());
        tests.sink(smart.clone());
        AutoGroups {
            tests,
            smart,
            state: Mutex::default(),
            wake: Mutex::default(),
            rounds: watch::Sender::new(0),
        }
    }

    /// Where requests for a round of tests go from now on: the engine's
    /// scheduler, which runs `PolicyRegistry::test_round` for each name it
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
    pub fn wake(&self, group: &str) {
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

    /// The network epoch: how many times `network_changed` has been called.
    /// A round of tests notes it as it begins and hands it back to
    /// `round_ended`.
    pub(crate) fn epoch(&self) -> u64 {
        self.state.lock().expect("auto groups").epoch
    }

    /// A round of tests of `groups` that began in network epoch `began` has
    /// ended. It is the last round of each group — unless a network change
    /// has taken the last round from that group since the round began: it
    /// tested the network the machine was on, and the group is to test
    /// again (phase 3 M1 design 5). Either way the request is over, and the
    /// ones who wait for a round are woken to look again.
    pub(crate) fn round_ended(&self, groups: &[String], began: u64) {
        {
            let mut state = self.state.lock().expect("auto groups");
            let now = Instant::now();
            for group in groups {
                if state
                    .cleared
                    .get(group)
                    .is_none_or(|&cleared| cleared <= began)
                {
                    state.rounds.insert(group.clone(), now);
                }
                state.requested.remove(group);
            }
        }
        self.rounds.send_modify(|n| *n += 1);
    }

    /// A round of tests of `groups` that began just now has ended: what the
    /// tests of this crate mean by a round that ended.
    #[cfg(test)]
    pub(crate) fn round_done(&self, groups: &[String]) {
        self.round_ended(groups, self.epoch());
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

    /// Makes `member` the choice of the automatic or `subnet` group `spec`
    /// until it is cleared, the group changes in a reload, or the process
    /// ends (M3 design 6.5; phase 3 M1 design 6.2: a `subnet` group's goes
    /// when the network changes too). The caller has checked that it is a
    /// member.
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
        let gone = {
            let mut state = self.state.lock().expect("auto groups");
            let o = state.overrides.get_mut(group)?;
            if members.contains(&o.member) {
                return Some(o.member.clone());
            }
            if o.warned {
                return None;
            }
            o.warned = true;
            o.member.clone()
        };
        // said with the lock released: every dial through an automatic
        // group takes it, and the writer behind `tracing` is I/O
        tracing::warn!(group, member = %gone, "the overriding member is gone from the group; the override has no effect");
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
        state
            .cleared
            .retain(|group, _| by_name.contains_key(group.as_str()));
    }

    /// The network has changed (phase 3 M1 design 5). The groups that pick
    /// by their tests lose the time of their last round, so that they test
    /// again when they are used next — an `evaluate-before-use` group is
    /// waited for again; the results themselves stay, and the groups pick by
    /// them until the new round is in. A round that is under way began on the
    /// old network: when it ends it is no round of the new one (`round_ended`).
    /// A `subnet` group loses its override: it said what to use on the
    /// network it was set on. A `smart` group keeps its rounds (the manual
    /// says nothing of it), and so do the overrides of the groups that test
    /// and the member `url-test` holds.
    pub fn network_changed(&self, groups: &[GroupSpec]) {
        let mut state = self.state.lock().expect("auto groups");
        state.epoch += 1;
        let epoch = state.epoch;
        for group in groups {
            match group.kind {
                GroupKind::UrlTest | GroupKind::Fallback | GroupKind::LoadBalance => {
                    state.rounds.remove(&group.name);
                    state.cleared.insert(group.name.clone(), epoch);
                }
                GroupKind::Subnet => {
                    state.overrides.remove(&group.name);
                }
                GroupKind::Select | GroupKind::Smart => {}
            }
        }
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

    /// A `subnet` group's override is kept the same way: while the group is
    /// defined as it was (phase 3 M1 design 6.2).
    #[test]
    fn a_subnet_override_lasts_while_the_group_stays_the_same() {
        let a = "[Proxy]\nA = direct\nB = direct\n[Proxy Group]\nN = subnet, TYPE:WIRED = A, default = B\n[Rule]\nFINAL,N\n";
        let moved = "[Proxy]\nA = direct\nB = direct\n\n\n[Proxy Group]\nN = subnet, TYPE:WIRED = A, default = B\n[Rule]\nFINAL,N\n";
        let changed = "[Proxy]\nA = direct\nB = direct\n[Proxy Group]\nN = subnet, TYPE:WIFI = A, default = B\n[Rule]\nFINAL,N\n";
        let auto = auto();
        let members = ["A".to_string(), "B".to_string()];
        auto.set_override(&spec_of(a, "N"), "A");
        auto.retain(&[spec_of(moved, "N")]);
        assert_eq!(auto.override_of("N", &members).as_deref(), Some("A"));
        auto.retain(&[spec_of(changed, "N")]);
        assert_eq!(auto.override_of("N", &members), None);
    }

    /// Phase 3 M1 design 5: a network change takes the time of the last round
    /// from the groups that pick by their tests — they test again when they
    /// are used next — and the overrides from the `subnet` groups. A `smart`
    /// group's rounds, the overrides of the other groups and the member
    /// `url-test` holds stay.
    #[test]
    fn a_network_change_clears_the_rounds_of_the_tested_groups_and_the_subnet_overrides() {
        const TEXT: &str = "[Proxy]\nA = direct\nB = direct\n[Proxy Group]\n\
U = url-test, A, B\nF = fallback, A, B\nL = load-balance, A, B\nS = smart, A, B\n\
N = subnet, TYPE:WIRED = A, default = B\n[Rule]\nFINAL,U\n";
        let groups = ["U", "F", "L", "S", "N"];
        let specs: Vec<GroupSpec> = groups.iter().map(|g| spec_of(TEXT, g)).collect();
        let members = ["A".to_string(), "B".to_string()];
        let auto = auto();
        auto.round_done(&groups.map(String::from));
        auto.set_override(&specs[0], "B");
        auto.set_override(&specs[4], "A");
        auto.set_pick("U", "A");
        auto.network_changed(&specs);
        for group in ["U", "F", "L"] {
            assert_eq!(auto.last_round(group), None, "{group}");
        }
        assert!(auto.last_round("S").is_some());
        assert_eq!(auto.override_of("N", &members), None);
        assert_eq!(auto.override_of("U", &members).as_deref(), Some("B"));
        assert_eq!(auto.pick("U").as_deref(), Some("A"));
    }

    /// Phase 3 M1 design 5: a round that began before a network change says
    /// nothing of the network the machine is on now, so when it ends it does
    /// not stand for the retest the change asked of the groups that pick by
    /// their tests. Its request is over all the same, and what waits for a
    /// round is woken to look again. A `smart` group is not asked for a
    /// retest by a network change: its round counts as it did.
    #[test]
    fn a_round_that_began_before_a_network_change_does_not_count() {
        const TEXT: &str = "[Proxy]\nA = direct\nB = direct\n[Proxy Group]\n\
U = url-test, A, B\nF = fallback, A, B\nL = load-balance, A, B\nS = smart, A, B\n\
[Rule]\nFINAL,U\n";
        let groups = ["U", "F", "L", "S"];
        let specs: Vec<GroupSpec> = groups.iter().map(|g| spec_of(TEXT, g)).collect();
        let covered = groups.map(String::from);
        let auto = auto();
        let rounds = auto.rounds();
        auto.wake("U");
        let began = auto.epoch();
        auto.network_changed(&specs);
        auto.round_ended(&covered, began);
        for group in ["U", "F", "L"] {
            assert_eq!(auto.last_round(group), None, "{group}");
        }
        assert!(auto.last_round("S").is_some());
        assert!(auto.requested().is_empty(), "the request is over");
        assert!(rounds.has_changed().unwrap(), "waiters are woken");
        // a round that begins after the change counts
        auto.round_ended(&covered, auto.epoch());
        for group in groups {
            assert!(auto.last_round(group).is_some(), "{group}");
        }
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

    /// The `smart` groups hear of every result the tests keep (M3c-D7).
    #[tokio::test]
    async fn the_smart_book_hears_of_every_kept_test() {
        let auto = auto();
        let outbound: rurge_proto::OutboundRef =
            Arc::new(rurge_proto::Reject::new(rurge_proto::RejectKind::Reject));
        let case = crate::testbook::TestCase {
            policy: "A".to_string(),
            outbound: outbound.clone(),
            mode: crate::testbook::TestMode::Url(url::Url::parse("http://127.0.0.1:9/").unwrap()),
            timeout: Duration::from_secs(5),
            key: 1,
            roots: Arc::new(rustls::RootCertStore::empty()),
        };
        auto.tests.test(case).await;
        assert_eq!(
            auto.smart.health("A", &outbound, Instant::now()),
            crate::smart::Health::Unknown { failures: 1 }
        );
    }
}
