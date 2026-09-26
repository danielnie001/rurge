//! What the `smart` groups know of their members (phase 2 M3c design §5): a
//! score per policy from the first-byte times of real sessions and from the
//! connectivity tests, a penalty for every failure, and whether a member is
//! healthy, failed or not known yet. It is kept per policy, so groups that
//! share a member share what is known of it (M3c-D2), and it outlives config
//! generations; a policy whose definition changed starts over. Beside it,
//! what happened at each site lately and how often each group used each
//! member — and how a dial ranks a group's members by all that (M3c design
//! §6).

use crate::testbook::{TestCase, TestResult, TestSink};
use rurge_proto::{Outbound, OutboundRef};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// How fast what is known fades: a sample, or a penalty, counts half as much
/// after this long (M3c design §9).
pub const HALF_LIFE: Duration = Duration::from_secs(5 * 60);
/// What each failure adds to the score.
pub const PENALTY: Duration = Duration::from_millis(800);
/// The score (before the group's factor) from which a member counts as
/// failed.
pub const FAILED_SCORE: Duration = Duration::from_millis(3000);
/// Failures in a row that make a member failed.
pub const FAILED_IN_A_ROW: u32 = 3;
/// Members whose score is within this factor of the best one's form the
/// preferred set, which a dial picks from at random (M3c design 6.2).
pub const PREFERRED: f64 = 1.2;
/// A member that worked at the session's site lately is picked while its
/// score is within this factor of the best one's.
pub const SITE_PREFERRED: f64 = 2.0;
/// How long what happened at a site is remembered (M3c design 6.3).
pub const SITE_MEMORY: Duration = Duration::from_secs(60 * 60);
/// Sites remembered at most; the one used least recently goes first.
pub const SITES: usize = 4096;
/// Policies remembered per site at most; the oldest goes first.
pub const POLICIES_PER_SITE: usize = 16;
/// The window of the usage counts (M3c design 8.2), kept a minute a bucket.
pub const USAGE_WINDOW: Duration = Duration::from_secs(10 * 60);
const USAGE_BUCKET: Duration = Duration::from_secs(60);
/// A `smart` group's round of tests is due this long after its last one;
/// `interval` has no effect on it (M3c design 8.1).
pub const ROUND_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// What is known of a member (M3c design 5.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Health {
    /// It has samples and has not failed: its score, before the group's
    /// factor.
    Healthy(Duration),
    /// No sample yet, and fewer than `FAILED_IN_A_ROW` failures in a row.
    Unknown { failures: u32 },
    /// `FAILED_IN_A_ROW` failures in a row, or a score of at least
    /// `FAILED_SCORE`; the score, when it has samples.
    Failed(Option<Duration>),
}

/// How much of what was known at `since` still counts at `now`.
fn fade(since: Instant, now: Instant) -> f64 {
    let age = now.saturating_duration_since(since).as_secs_f64();
    (-age / HALF_LIFE.as_secs_f64()).exp2()
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

struct Record {
    /// What the numbers are about: another outbound under the same name —
    /// the definition changed, and with it the outbound (M2 design 7.1) —
    /// starts over. Holding a `Weak` keeps the allocation, so its address
    /// cannot come back as another outbound.
    outbound: Weak<dyn Outbound>,
    /// The time-weighted sum of the samples (ms) and of their weights, both
    /// as of `sampled`: their ratio does not move with time alone.
    sum: f64,
    weight: f64,
    sampled: Option<Instant>,
    /// The penalty (ms) as of `penalized`.
    penalty: f64,
    penalized: Option<Instant>,
    failures: u32,
}

impl Record {
    fn new(outbound: &OutboundRef) -> Record {
        Record {
            outbound: Arc::downgrade(outbound),
            sum: 0.0,
            weight: 0.0,
            sampled: None,
            penalty: 0.0,
            penalized: None,
            failures: 0,
        }
    }

    fn is_of(&self, outbound: &OutboundRef) -> bool {
        std::ptr::addr_eq(self.outbound.as_ptr(), Arc::as_ptr(outbound))
    }

    fn sample(&mut self, ms: f64, now: Instant) {
        if let Some(at) = self.sampled {
            let f = fade(at, now);
            self.sum *= f;
            self.weight *= f;
        }
        self.sum += ms;
        self.weight += 1.0;
        self.sampled = Some(now);
        self.failures = 0;
    }

    fn failure(&mut self, now: Instant) {
        self.penalty = self.penalty_at(now) + millis(PENALTY);
        self.penalized = Some(now);
        self.failures = self.failures.saturating_add(1);
    }

    fn penalty_at(&self, now: Instant) -> f64 {
        self.penalized
            .map_or(0.0, |at| self.penalty * fade(at, now))
    }

    fn health(&self, now: Instant) -> Health {
        let score = (self.weight > 0.0).then(|| {
            Duration::from_secs_f64((self.sum / self.weight + self.penalty_at(now)) / 1000.0)
        });
        if self.failures >= FAILED_IN_A_ROW || score.is_some_and(|s| s >= FAILED_SCORE) {
            return Health::Failed(score);
        }
        match score {
            Some(score) => Health::Healthy(score),
            None => Health::Unknown {
                failures: self.failures,
            },
        }
    }
}

/// What happened lately at one site, as a dial through a `smart` group
/// reads it (M3c design 6.3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SiteMemory {
    /// The policies whose last session at the site worked.
    pub worked: Vec<String>,
    /// The policies whose last session at the site failed.
    pub failed: Vec<String>,
}

#[derive(Default)]
struct Sites {
    hosts: HashMap<String, Site>,
    /// Orders the uses of the sites: the least recently used one goes when
    /// there are too many.
    clock: u64,
}

struct Site {
    /// `Sites::clock` when the site was last read or written.
    used: u64,
    /// The last outcome of each policy at the site, oldest first.
    policies: Vec<(String, bool, Instant)>,
}

impl Sites {
    fn remember(&mut self, host: &str, policy: &str, worked: bool, now: Instant) {
        self.clock += 1;
        let clock = self.clock;
        if !self.hosts.contains_key(host) && self.hosts.len() >= SITES {
            let oldest = self
                .hosts
                .iter()
                .min_by_key(|(_, site)| site.used)
                .map(|(host, _)| host.clone());
            if let Some(oldest) = oldest {
                self.hosts.remove(&oldest);
            }
        }
        let site = self.hosts.entry(host.to_string()).or_insert_with(|| Site {
            used: clock,
            policies: Vec::new(),
        });
        site.used = clock;
        site.policies
            .retain(|(p, _, at)| p != policy && now.saturating_duration_since(*at) < SITE_MEMORY);
        site.policies.push((policy.to_string(), worked, now));
        if site.policies.len() > POLICIES_PER_SITE {
            site.policies.remove(0);
        }
    }

    fn read(&mut self, host: &str, now: Instant) -> SiteMemory {
        self.clock += 1;
        let clock = self.clock;
        let mut memory = SiteMemory::default();
        if let Some(site) = self.hosts.get_mut(host) {
            site.used = clock;
            for (policy, worked, at) in &site.policies {
                if now.saturating_duration_since(*at) < SITE_MEMORY {
                    let list = if *worked {
                        &mut memory.worked
                    } else {
                        &mut memory.failed
                    };
                    list.push(policy.clone());
                }
            }
        }
        memory
    }
}

/// A group's uses of its members, a bucket a minute over `USAGE_WINDOW`.
#[derive(Default)]
struct Usage {
    buckets: VecDeque<(Instant, HashMap<String, u32>)>,
}

impl Usage {
    fn add(&mut self, member: &str, now: Instant) {
        while self
            .buckets
            .front()
            .is_some_and(|(start, _)| now.saturating_duration_since(*start) >= USAGE_WINDOW)
        {
            self.buckets.pop_front();
        }
        if self
            .buckets
            .back()
            .is_none_or(|(start, _)| now.saturating_duration_since(*start) >= USAGE_BUCKET)
        {
            self.buckets.push_back((now, HashMap::new()));
        }
        let (_, bucket) = self.buckets.back_mut().expect("just pushed");
        *bucket.entry(member.to_string()).or_default() += 1;
    }

    fn count(&self, member: &str, now: Instant) -> u32 {
        self.buckets
            .iter()
            .filter(|(start, _)| now.saturating_duration_since(*start) < USAGE_WINDOW)
            .filter_map(|(_, bucket)| bucket.get(member))
            .sum()
    }
}

/// A member of a `smart` group as a dial ranks it.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub name: &'a str,
    pub health: Health,
    /// The group's `policy-priority` factor for it.
    pub factor: f64,
}

/// The member a dial through a `smart` group uses, and the others in the
/// order a dial tries them when it does not connect (M3c design 6.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ranking {
    pub pick: String,
    pub retry: Vec<String>,
}

/// Ranks the members of a `smart` group (M3c design 6.2): the healthy ones by
/// score (their factor applied), then those not known yet, then the failed
/// ones; those that failed at the site lately go last of all. A healthy one
/// that worked at the site lately is picked while its score is within
/// `SITE_PREFERRED` of the best; otherwise `pick_at(n)` picks one of the `n`
/// healthy members within `PREFERRED` of the best; with none healthy, the
/// first in line is.
pub fn rank(
    candidates: &[Candidate<'_>],
    site: &SiteMemory,
    pick_at: impl FnOnce(usize) -> usize,
) -> Option<Ranking> {
    let mut healthy: Vec<(usize, f64)> = Vec::new();
    let mut unknown: Vec<(usize, u32)> = Vec::new();
    let mut failed: Vec<(usize, Option<Duration>)> = Vec::new();
    for (i, c) in candidates.iter().enumerate() {
        match c.health {
            Health::Healthy(score) => healthy.push((i, score.as_secs_f64() * c.factor)),
            Health::Unknown { failures } => unknown.push((i, failures)),
            Health::Failed(score) => failed.push((i, score)),
        }
    }
    healthy.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    unknown.sort_by_key(|&(i, failures)| (failures, i));
    // those with a score first, lowest first; then the others, in order
    failed.sort_by_key(|&(i, score)| (score.is_none(), score, i));
    let failed_here = |i: usize| site.failed.iter().any(|p| p == candidates[i].name);
    let worked_here = |i: usize| site.worked.iter().any(|p| p == candidates[i].name);
    let (mut order, last): (Vec<usize>, Vec<usize>) = healthy
        .iter()
        .map(|&(i, _)| i)
        .chain(unknown.iter().map(|&(i, _)| i))
        .chain(failed.iter().map(|&(i, _)| i))
        .partition(|&i| !failed_here(i));
    order.extend(last);
    let usable: Vec<(usize, f64)> = healthy
        .iter()
        .copied()
        .filter(|&(i, _)| !failed_here(i))
        .collect();
    let known = healthy.first().map(|&(_, best)| {
        usable
            .iter()
            .find(|&&(i, score)| worked_here(i) && score <= best * SITE_PREFERRED)
            .map(|&(i, _)| i)
    });
    let pick = match (known.flatten(), usable.first()) {
        (Some(i), _) => i,
        (None, Some(&(_, best))) => {
            let preferred: Vec<usize> = usable
                .iter()
                .take_while(|&&(_, score)| score <= best * PREFERRED)
                .map(|&(i, _)| i)
                .collect();
            preferred[pick_at(preferred.len()).min(preferred.len() - 1)]
        }
        (None, None) => *order.first()?,
    };
    Some(Ranking {
        pick: candidates[pick].name.to_string(),
        retry: order
            .iter()
            .filter(|&&i| i != pick)
            .map(|&i| candidates[i].name.to_string())
            .collect(),
    })
}

/// What the `smart` groups know of their members; one per engine, like the
/// test results (M3c design 5.1). Time is always the caller's.
#[derive(Default)]
pub struct SmartBook {
    records: Mutex<HashMap<String, Record>>,
    sites: Mutex<Sites>,
    /// By group.
    usage: Mutex<HashMap<String, Usage>>,
}

/// The record of `policy`, a fresh one when there is none for `outbound`.
fn record_of<'a>(
    records: &'a mut HashMap<String, Record>,
    policy: &str,
    outbound: &OutboundRef,
) -> &'a mut Record {
    if records.get(policy).is_none_or(|r| !r.is_of(outbound)) {
        records.insert(policy.to_string(), Record::new(outbound));
    }
    records.get_mut(policy).expect("just inserted")
}

impl SmartBook {
    pub fn new() -> SmartBook {
        SmartBook::default()
    }

    /// A sample of `policy` through `outbound`: the first byte came back
    /// `latency` after the outbound was ready, or a test passed with that
    /// score (M3c design 5.3).
    pub fn sample(&self, policy: &str, outbound: &OutboundRef, latency: Duration, now: Instant) {
        let mut records = self.records.lock().expect("smart records");
        record_of(&mut records, policy, outbound).sample(millis(latency), now);
    }

    /// A failure of `policy` through `outbound`: a dial, a session that got
    /// no answer, a test.
    pub fn failure(&self, policy: &str, outbound: &OutboundRef, now: Instant) {
        let mut records = self.records.lock().expect("smart records");
        record_of(&mut records, policy, outbound).failure(now);
    }

    /// What is known of `policy` through `outbound` at `now`.
    pub fn health(&self, policy: &str, outbound: &OutboundRef, now: Instant) -> Health {
        self.records
            .lock()
            .expect("smart records")
            .get(policy)
            .filter(|r| r.is_of(outbound))
            .map_or(Health::Unknown { failures: 0 }, |r| r.health(now))
    }

    /// The first byte of a session through `policy` came back `latency`
    /// after the outbound was ready (M3c design 4.3): a sample, and `policy`
    /// worked at `host`.
    pub fn report_success(
        &self,
        policy: &str,
        outbound: &OutboundRef,
        host: Option<&str>,
        latency: Duration,
        now: Instant,
    ) {
        self.change(policy, outbound, now, |r| r.sample(millis(latency), now));
        if let Some(host) = host {
            self.sites
                .lock()
                .expect("smart sites")
                .remember(host, policy, true, now);
        }
    }

    /// A dial through `policy` did not connect, or its session got no answer
    /// (M3c design 4.3): a failure, and `policy` failed at `host`.
    pub fn report_failure(
        &self,
        policy: &str,
        outbound: &OutboundRef,
        host: Option<&str>,
        now: Instant,
    ) {
        self.change(policy, outbound, now, |r| r.failure(now));
        if let Some(host) = host {
            self.sites
                .lock()
                .expect("smart sites")
                .remember(host, policy, false, now);
        }
    }

    /// Applies `change` to the record of `policy`; says so when the member
    /// comes to count as failed by it, or stops to (M3c design §10).
    fn change(
        &self,
        policy: &str,
        outbound: &OutboundRef,
        now: Instant,
        change: impl FnOnce(&mut Record),
    ) {
        let (was, is) = {
            let mut records = self.records.lock().expect("smart records");
            let record = record_of(&mut records, policy, outbound);
            let was = matches!(record.health(now), Health::Failed(_));
            change(record);
            (was, matches!(record.health(now), Health::Failed(_)))
        };
        if was != is {
            if is {
                tracing::info!(policy, "smart: the policy counts as failed");
            } else {
                tracing::info!(policy, "smart: the policy works again");
            }
        }
    }

    /// What happened at `host` lately (M3c design 6.3).
    pub fn site(&self, host: &str, now: Instant) -> SiteMemory {
        self.sites.lock().expect("smart sites").read(host, now)
    }

    /// A session of `group` went through `member` (M3c design 8.2).
    pub fn used(&self, group: &str, member: &str, now: Instant) {
        self.usage
            .lock()
            .expect("smart usage")
            .entry(group.to_string())
            .or_default()
            .add(member, now);
    }

    /// `members` that `group` used within `USAGE_WINDOW`, most used first;
    /// a tie keeps the order of `members`.
    pub fn most_used(&self, group: &str, members: &[String], now: Instant) -> Vec<String> {
        let usage = self.usage.lock().expect("smart usage");
        let Some(usage) = usage.get(group) else {
            return Vec::new();
        };
        let mut used: Vec<(u32, usize)> = members
            .iter()
            .enumerate()
            .map(|(i, m)| (usage.count(m, now), i))
            .filter(|&(n, _)| n > 0)
            .collect();
        used.sort_by_key(|&(n, i)| (std::cmp::Reverse(n), i));
        used.into_iter().map(|(_, i)| members[i].clone()).collect()
    }

    /// A new generation: what is kept of the policies and groups it no longer
    /// has goes.
    pub fn retain(&self, keep: impl Fn(&str) -> bool) {
        self.records
            .lock()
            .expect("smart records")
            .retain(|policy, _| keep(policy));
        for site in self.sites.lock().expect("smart sites").hosts.values_mut() {
            site.policies.retain(|(policy, _, _)| keep(policy));
        }
        self.usage
            .lock()
            .expect("smart usage")
            .retain(|group, _| keep(group));
    }
}

/// The tests feed the scores too (M3c-D7): a pass is a sample, a failure a
/// failure.
impl TestSink for SmartBook {
    fn tested(&self, case: &TestCase, result: &TestResult) {
        match result.outcome {
            Ok(score) => self.sample(&case.policy, &case.outbound, score, result.at),
            Err(_) => self.failure(&case.policy, &case.outbound, result.at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_proto::{Reject, RejectKind};

    fn outbound() -> OutboundRef {
        Arc::new(Reject::new(RejectKind::Reject))
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Within a millisecond: the scores are computed in floating point.
    fn healthy_near(health: Health, expected: Duration) {
        match health {
            Health::Healthy(score) => assert!(
                score.abs_diff(expected) < ms(1),
                "{score:?} is not {expected:?}"
            ),
            other => panic!("{other:?} is not healthy"),
        }
    }

    /// A sample counts half as much five minutes on (M3c design 5.2): 100
    /// then, 300 now, is (100 × 0.5 + 300) ÷ 1.5.
    #[test]
    fn a_sample_weighs_half_as_much_five_minutes_on() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        book.sample("A", &a, ms(100), t0);
        book.sample("A", &a, ms(300), t0 + HALF_LIFE);
        healthy_near(
            book.health("A", &a, t0 + HALF_LIFE),
            Duration::from_micros(233_333),
        );
    }

    #[test]
    fn the_score_does_not_move_with_time_alone() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        book.sample("A", &a, ms(100), t0);
        book.sample("A", &a, ms(300), t0 + HALF_LIFE);
        let later = t0 + Duration::from_secs(3600);
        assert_eq!(
            book.health("A", &a, t0 + HALF_LIFE),
            book.health("A", &a, later)
        );
    }

    /// Each failure adds 800 ms, which fades like the samples do.
    #[test]
    fn a_failure_adds_a_penalty_that_halves_every_five_minutes() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        book.sample("A", &a, ms(100), t0);
        book.failure("A", &a, t0);
        healthy_near(book.health("A", &a, t0), ms(900));
        healthy_near(book.health("A", &a, t0 + HALF_LIFE), ms(500));
    }

    #[test]
    fn three_failures_in_a_row_fail_a_member_and_a_success_restores_it() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        book.sample("A", &a, ms(100), t0);
        for _ in 0..3 {
            book.failure("A", &a, t0);
        }
        assert!(matches!(book.health("A", &a, t0), Health::Failed(Some(_))));
        book.sample("A", &a, ms(100), t0);
        healthy_near(book.health("A", &a, t0), ms(2500));
    }

    #[test]
    fn a_score_of_three_seconds_fails_a_member() {
        let (book, a, b, t0) = (SmartBook::new(), outbound(), outbound(), Instant::now());
        book.sample("A", &a, FAILED_SCORE, t0);
        assert_eq!(book.health("A", &a, t0), Health::Failed(Some(FAILED_SCORE)));
        book.sample("B", &b, FAILED_SCORE - ms(1), t0);
        healthy_near(book.health("B", &b, t0), FAILED_SCORE - ms(1));
    }

    #[test]
    fn without_a_sample_a_member_is_unknown_until_it_fails_three_times() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        assert_eq!(book.health("A", &a, t0), Health::Unknown { failures: 0 });
        book.failure("A", &a, t0);
        book.failure("A", &a, t0);
        assert_eq!(book.health("A", &a, t0), Health::Unknown { failures: 2 });
        book.failure("A", &a, t0);
        assert_eq!(book.health("A", &a, t0), Health::Failed(None));
    }

    /// A policy whose definition changed has another outbound: what was known
    /// of the old one does not count (M3c design 5.1).
    #[test]
    fn another_outbound_under_the_same_name_starts_over() {
        let (book, old, new, t0) = (SmartBook::new(), outbound(), outbound(), Instant::now());
        book.sample("A", &old, ms(100), t0);
        assert_eq!(book.health("A", &new, t0), Health::Unknown { failures: 0 });
        book.failure("A", &new, t0);
        assert_eq!(book.health("A", &new, t0), Health::Unknown { failures: 1 });
        assert_eq!(book.health("A", &old, t0), Health::Unknown { failures: 0 });
    }

    #[test]
    fn retain_forgets_the_policies_that_are_gone() {
        let (book, a, b, t0) = (SmartBook::new(), outbound(), outbound(), Instant::now());
        book.sample("A", &a, ms(100), t0);
        book.sample("B", &b, ms(100), t0);
        book.retain(|p| p == "B");
        assert_eq!(book.health("A", &a, t0), Health::Unknown { failures: 0 });
        healthy_near(book.health("B", &b, t0), ms(100));
    }

    fn healthy(name: &str, ms: u64) -> Candidate<'_> {
        Candidate {
            name,
            health: Health::Healthy(Duration::from_millis(ms)),
            factor: 1.0,
        }
    }

    fn with(name: &str, health: Health) -> Candidate<'_> {
        Candidate {
            name,
            health,
            factor: 1.0,
        }
    }

    fn nowhere() -> SiteMemory {
        SiteMemory::default()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// The preferred set is the healthy members within a fifth of the best
    /// one's score; a dial picks from it at random (M3c design 6.2).
    #[test]
    fn a_dial_picks_at_random_among_those_close_to_the_best() {
        let c = [healthy("A", 100), healthy("B", 110), healthy("C", 200)];
        let mut seen = 0;
        let r = rank(&c, &nowhere(), |n| {
            seen = n;
            1
        })
        .unwrap();
        assert_eq!(seen, 2, "A and B are within 1.2 × 100 ms");
        assert_eq!(r.pick, "B");
        assert_eq!(r.retry, names(&["A", "C"]));
        let r = rank(&c, &nowhere(), |_| 0).unwrap();
        assert_eq!((r.pick.as_str(), r.retry), ("A", names(&["B", "C"])));
    }

    #[test]
    fn a_priority_factor_scales_the_score() {
        let mut slow = healthy("A", 100);
        slow.factor = 2.0;
        let c = [slow, healthy("B", 150)];
        let r = rank(&c, &nowhere(), |n| n - 1).unwrap();
        assert_eq!(r.pick, "B", "200 ms is beyond 1.2 × 150 ms");
        assert_eq!(r.retry, names(&["A"]));
    }

    /// Healthy ones first, then those not known yet — fewer failures first —
    /// then the failed ones, those with a score first.
    #[test]
    fn the_unknown_come_after_the_healthy_and_the_failed_last() {
        let c = [
            with("A", Health::Failed(None)),
            with("B", Health::Unknown { failures: 1 }),
            with("C", Health::Failed(Some(ms(4000)))),
            with("D", Health::Unknown { failures: 0 }),
            healthy("E", 300),
        ];
        let r = rank(&c, &nowhere(), |_| 0).unwrap();
        assert_eq!(r.pick, "E");
        assert_eq!(r.retry, names(&["D", "B", "C", "A"]));
    }

    #[test]
    fn without_a_healthy_member_the_first_in_line_is_picked() {
        let c = [
            with("A", Health::Failed(Some(ms(4000)))),
            with("B", Health::Unknown { failures: 0 }),
        ];
        let r = rank(&c, &nowhere(), |_| unreachable!("nothing to draw from")).unwrap();
        assert_eq!((r.pick.as_str(), r.retry), ("B", names(&["A"])));
        assert_eq!(rank(&[], &nowhere(), |_| 0), None);
    }

    /// A member that worked at the site lately is picked while it is within
    /// twice the best score; one that failed there goes last (M3c design
    /// 6.2).
    #[test]
    fn what_happened_at_the_site_comes_first() {
        let c = [healthy("A", 100), healthy("B", 180), healthy("C", 250)];
        let site = SiteMemory {
            worked: names(&["B", "C"]),
            failed: Vec::new(),
        };
        let r = rank(&c, &site, |_| unreachable!("the site decides")).unwrap();
        assert_eq!((r.pick.as_str(), r.retry), ("B", names(&["A", "C"])));
        let site = SiteMemory {
            worked: names(&["C"]),
            failed: names(&["A"]),
        };
        let r = rank(&c, &site, |n| {
            assert_eq!(n, 1, "only B is left within 1.2 × 180 ms");
            0
        })
        .unwrap();
        assert_eq!((r.pick.as_str(), r.retry), ("B", names(&["C", "A"])));
    }

    #[test]
    fn a_site_is_remembered_for_an_hour() {
        let (book, a, b, t0) = (SmartBook::new(), outbound(), outbound(), Instant::now());
        book.report_success("A", &a, Some("x.test"), ms(50), t0);
        book.report_failure("B", &b, Some("x.test"), t0);
        let memory = book.site("x.test", t0 + Duration::from_secs(60));
        assert_eq!(
            memory,
            SiteMemory {
                worked: names(&["A"]),
                failed: names(&["B"]),
            }
        );
        assert_eq!(book.site("x.test", t0 + SITE_MEMORY), SiteMemory::default());
        assert_eq!(book.site("y.test", t0), SiteMemory::default());
        // the latest outcome of a policy stands
        book.report_failure("A", &a, Some("x.test"), t0);
        assert_eq!(book.site("x.test", t0).failed, names(&["B", "A"]));
    }

    #[test]
    fn the_site_used_least_recently_goes_first() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        for i in 0..SITES {
            book.report_success("A", &a, Some(&format!("{i}.test")), ms(50), t0);
        }
        // reading a site uses it
        assert_eq!(book.site("0.test", t0).worked, names(&["A"]));
        book.report_success("A", &a, Some("new.test"), ms(50), t0);
        assert_eq!(book.site("0.test", t0).worked, names(&["A"]), "used lately");
        assert_eq!(
            book.site("1.test", t0),
            SiteMemory::default(),
            "the least recently used"
        );
        assert_eq!(book.site("new.test", t0).worked, names(&["A"]));
    }

    #[test]
    fn a_site_keeps_its_latest_sixteen_policies() {
        let (book, t0) = (SmartBook::new(), Instant::now());
        let outbounds: Vec<OutboundRef> = (0..=POLICIES_PER_SITE).map(|_| outbound()).collect();
        for (i, o) in outbounds.iter().enumerate() {
            book.report_success(&format!("P{i}"), o, Some("x.test"), ms(50), t0);
        }
        let worked = book.site("x.test", t0).worked;
        assert_eq!(worked.len(), POLICIES_PER_SITE);
        assert!(!worked.contains(&"P0".to_string()), "the oldest went");
    }

    /// The usage counts: the last ten minutes, most used first, a tie in the
    /// group's order (M3c design 8.2).
    #[test]
    fn the_most_used_members_of_the_last_ten_minutes() {
        let (book, t0) = (SmartBook::new(), Instant::now());
        let members = names(&["A", "B", "C"]);
        book.used("G", "C", t0);
        book.used("G", "B", t0 + Duration::from_secs(120));
        book.used("G", "C", t0 + Duration::from_secs(180));
        book.used("G", "A", t0 + Duration::from_secs(240));
        let at = t0 + Duration::from_secs(300);
        assert_eq!(book.most_used("G", &members, at), names(&["C", "A", "B"]));
        assert_eq!(book.most_used("H", &members, at), Vec::<String>::new());
        let later = t0 + USAGE_WINDOW + Duration::from_secs(1);
        assert_eq!(
            book.most_used("G", &members, later),
            names(&["A", "B", "C"])
        );
    }

    #[test]
    fn a_report_scores_the_member_as_well() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        book.report_success("A", &a, None, ms(100), t0);
        book.report_failure("A", &a, None, t0);
        healthy_near(book.health("A", &a, t0), ms(900));
    }
}
