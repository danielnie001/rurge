//! What the `smart` groups know of their members (phase 2 M3c design §5): a
//! score per policy from the first-byte times of real sessions and from the
//! connectivity tests, a penalty for every failure, and whether a member is
//! healthy, failed or not known yet. It is kept per policy, so groups that
//! share a member share what is known of it (M3c-D2), and it outlives config
//! generations; a policy whose definition changed starts over.

use crate::testbook::{TestCase, TestResult, TestSink};
use rurge_proto::{Outbound, OutboundRef};
use std::collections::HashMap;
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

/// What the `smart` groups know of their members; one per engine, like the
/// test results (M3c design 5.1). Time is always the caller's.
#[derive(Default)]
pub struct SmartBook {
    records: Mutex<HashMap<String, Record>>,
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

    /// A new generation: what is kept of the policies it no longer has goes.
    pub fn retain(&self, keep: impl Fn(&str) -> bool) {
        self.records
            .lock()
            .expect("smart records")
            .retain(|policy, _| keep(policy));
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
}
