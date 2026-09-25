//! Connectivity test results, kept across config generations (phase 2 M3
//! design 6.2): at most one test of a policy at a time, at most
//! `MAX_CONCURRENT_TESTS` tests at once, every test a session in the
//! request log through `TestObserver`.

use crate::probe::{Probed, probe};
use rurge_proto::OutboundRef;
use rustls::RootCertStore;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::{Semaphore, watch};
use url::Url;

/// How many tests may run at the same time, whoever asked for them.
pub const MAX_CONCURRENT_TESTS: usize = 8;

/// One test's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestResult {
    /// The score, or why the test failed.
    pub outcome: Result<Duration, String>,
    /// When the test ended, for telling how old the result is.
    pub at: Instant,
    /// The same moment on the wall clock, for the control plane.
    pub when: SystemTime,
}

/// What to test, as the registry in use has it.
#[derive(Clone)]
pub struct TestCase {
    pub policy: String,
    pub outbound: OutboundRef,
    pub url: Url,
    pub timeout: Duration,
    /// What a result is good for: a result of the policy under another
    /// definition, test URL or timeout is no result (M3 design 6.2).
    pub key: u64,
    /// What verifies an `https` test URL: the outbounds' own trust anchors
    /// (`OutboundFactory::roots`).
    pub roots: Arc<RootCertStore>,
}

/// Where a test shows up while it runs: the engine writes a session into
/// the request log (M3 design 6.2).
pub trait TestObserver: Send + Sync {
    fn begin(&self, policy: &str, url: &Url) -> Box<dyn TestRecord>;
}

/// The running test's record, told how the test ended.
pub trait TestRecord: Send {
    fn end(self: Box<Self>, outcome: &Result<Duration, String>);
}

type Running = (u64, watch::Receiver<Option<TestResult>>);

pub struct TestBook {
    results: RwLock<HashMap<String, (u64, TestResult)>>,
    running: Mutex<HashMap<String, Running>>,
    permits: Arc<Semaphore>,
    observer: OnceLock<Arc<dyn TestObserver>>,
    /// Test URLs already said to be imprecise (the server does not keep the
    /// connection): once each.
    warned: Mutex<HashSet<String>>,
}

impl Default for TestBook {
    fn default() -> TestBook {
        TestBook::new()
    }
}

impl TestBook {
    pub fn new() -> TestBook {
        TestBook {
            results: RwLock::default(),
            running: Mutex::default(),
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_TESTS)),
            observer: OnceLock::new(),
            warned: Mutex::default(),
        }
    }

    /// Where tests are reported from now on; set once, later calls are
    /// ignored.
    pub fn observe(&self, observer: Arc<dyn TestObserver>) {
        let _ = self.observer.set(observer);
    }

    /// The last result of `policy` for the definition `key` stands for.
    pub fn result(&self, policy: &str, key: u64) -> Option<TestResult> {
        self.results
            .read()
            .expect("test results")
            .get(policy)
            .filter(|(k, _)| *k == key)
            .map(|(_, r)| r.clone())
    }

    /// Records a result as if a test had ended with `outcome`.
    #[cfg(test)]
    pub(crate) fn record(&self, policy: &str, key: u64, outcome: Result<Duration, String>) {
        let result = TestResult {
            outcome,
            at: Instant::now(),
            when: SystemTime::now(),
        };
        self.results
            .write()
            .expect("test results")
            .insert(policy.to_string(), (key, result));
    }

    /// Forgets every result: the network is not the one they were made on
    /// (the "network changed" entry of phase 3).
    pub fn invalidate_all(&self) {
        self.results.write().expect("test results").clear();
    }

    /// Tests `case` now, or waits for the test of it already running. The
    /// test itself runs on its own task: whoever asked may stop waiting, the
    /// test still ends and its result is kept.
    pub async fn test(self: &Arc<Self>, case: TestCase) -> TestResult {
        let policy = case.policy.clone();
        let mut rx = {
            let mut running = self.running.lock().expect("running tests");
            match running.get(&case.policy) {
                // a closed channel: that test's task died without a result
                Some((key, rx)) if *key == case.key && rx.has_changed().is_ok() => rx.clone(),
                _ => {
                    let (tx, rx) = watch::channel(None);
                    running.insert(case.policy.clone(), (case.key, rx.clone()));
                    tokio::spawn(self.clone().run(case, tx));
                    rx
                }
            }
        };
        loop {
            if let Some(result) = rx.borrow_and_update().clone() {
                return result;
            }
            if rx.changed().await.is_err() {
                tracing::error!(policy = %policy, "a connectivity test ended without a result");
                return TestResult {
                    outcome: Err("the test did not finish".to_string()),
                    at: Instant::now(),
                    when: SystemTime::now(),
                };
            }
        }
    }

    async fn run(self: Arc<Self>, case: TestCase, tx: watch::Sender<Option<TestResult>>) {
        let outcome = {
            let _permit = self.permits.acquire().await.expect("never closed");
            let record = self
                .observer
                .get()
                .map(|observer| observer.begin(&case.policy, &case.url));
            let outcome = match probe(&case.outbound, &case.url, case.timeout, case.roots.clone())
                .await
            {
                Probed::Passed { score, reused } => {
                    if !reused
                        && self
                            .warned
                            .lock()
                            .expect("warned")
                            .insert(case.url.to_string())
                    {
                        // the URL stays out of the log: a subscription
                        // line may have set it (M3-D7)
                        tracing::warn!(
                            policy = %case.policy,
                            "the test server does not keep the connection: the score includes the dial"
                        );
                    }
                    Ok(score)
                }
                Probed::Failed(why) => Err(why),
            };
            if let Some(record) = record {
                record.end(&outcome);
            }
            outcome
        };
        let result = TestResult {
            outcome,
            at: Instant::now(),
            when: SystemTime::now(),
        };
        // A test superseded by a new definition (key changed) only hands its
        // result to its own waiters; it must not undo the current definition's
        // result, whichever ends last.
        {
            let mut running = self.running.lock().expect("running tests");
            if let Some((_, rx)) = running.get(&case.policy) {
                let rx_from_tx = tx.subscribe();
                if rx_from_tx.same_channel(rx) {
                    running.remove(&case.policy);
                    self.results
                        .write()
                        .expect("test results")
                        .insert(case.policy.clone(), (case.key, result.clone()));
                }
            }
        }
        let _ = tx.send(Some(result));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::BoxFuture;
    use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
    use rurge_proto::{Outbound, OutboundError};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Holds every connection for `hold`, counting how many are open at
    /// once, then refuses it.
    #[derive(Default)]
    struct Gate {
        hold: Duration,
        open: AtomicUsize,
        most: AtomicUsize,
        dials: AtomicUsize,
    }

    impl Outbound for Gate {
        fn name(&self) -> &str {
            "Gate"
        }
        fn connect_tcp<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
            Box::pin(async move {
                self.dials.fetch_add(1, Ordering::SeqCst);
                let now = self.open.fetch_add(1, Ordering::SeqCst) + 1;
                self.most.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(self.hold).await;
                self.open.fetch_sub(1, Ordering::SeqCst);
                Err(OutboundError::Proxy("closed by the gate".to_string()))
            })
        }
    }

    fn book() -> Arc<TestBook> {
        Arc::new(TestBook::new())
    }

    fn case(policy: &str, outbound: OutboundRef, key: u64) -> TestCase {
        TestCase {
            policy: policy.to_string(),
            outbound,
            url: Url::parse("http://127.0.0.1:9/").unwrap(),
            timeout: Duration::from_secs(5),
            key,
            roots: Arc::new(RootCertStore::empty()),
        }
    }

    #[tokio::test]
    async fn a_policy_is_tested_once_at_a_time() {
        let gate = Arc::new(Gate {
            hold: Duration::from_millis(100),
            ..Gate::default()
        });
        let book = book();
        let (a, b) = tokio::join!(
            book.test(case("P", gate.clone(), 1)),
            book.test(case("P", gate.clone(), 1))
        );
        assert_eq!(gate.dials.load(Ordering::SeqCst), 1);
        assert_eq!(a, b);
        assert_eq!(a.outcome, Err("connect: closed by the gate".to_string()));
        assert_eq!(book.result("P", 1), Some(a));
    }

    #[tokio::test]
    async fn no_more_than_eight_tests_run_at_once() {
        let gate = Arc::new(Gate {
            hold: Duration::from_millis(100),
            ..Gate::default()
        });
        let book = book();
        let tests: Vec<_> = (0..12)
            .map(|i| {
                let (book, case) = (book.clone(), case(&format!("P{i}"), gate.clone(), 1));
                tokio::spawn(async move { book.test(case).await })
            })
            .collect();
        for test in tests {
            assert!(test.await.expect("the test task").outcome.is_err());
        }
        assert_eq!(gate.dials.load(Ordering::SeqCst), 12);
        assert_eq!(gate.most.load(Ordering::SeqCst), MAX_CONCURRENT_TESTS);
    }

    /// A result is kept for the definition, URL and timeout it was made
    /// with; `invalidate_all` forgets them all.
    #[tokio::test]
    async fn a_result_is_good_for_what_was_tested() {
        let gate = Arc::new(Gate::default());
        let book = book();
        book.test(case("P", gate.clone(), 1)).await;
        assert!(book.result("P", 1).is_some());
        assert_eq!(book.result("P", 2), None);
        assert_eq!(book.result("Q", 1), None);
        book.invalidate_all();
        assert_eq!(book.result("P", 1), None);
    }

    #[derive(Default)]
    struct Seen(Mutex<Vec<String>>);

    struct Record(Arc<Seen>, String);

    impl TestObserver for Arc<Seen> {
        fn begin(&self, policy: &str, url: &Url) -> Box<dyn TestRecord> {
            self.0.lock().unwrap().push(format!("begin {policy} {url}"));
            Box::new(Record(self.clone(), policy.to_string()))
        }
    }

    impl TestRecord for Record {
        fn end(self: Box<Self>, outcome: &Result<Duration, String>) {
            let how = match outcome {
                Ok(_) => "passed".to_string(),
                Err(why) => why.clone(),
            };
            self.0
                .0
                .lock()
                .unwrap()
                .push(format!("end {} {how}", self.1));
        }
    }

    #[tokio::test]
    async fn every_test_is_seen_from_begin_to_end() {
        let seen = Arc::new(Seen::default());
        let book = book();
        book.observe(Arc::new(seen.clone()));
        book.test(case("P", Arc::new(Gate::default()), 1)).await;
        assert_eq!(
            *seen.0.lock().unwrap(),
            [
                "begin P http://127.0.0.1:9/",
                "end P connect: closed by the gate"
            ]
        );
    }

    /// Panics on every dial.
    struct Broken;

    impl Outbound for Broken {
        fn name(&self) -> &str {
            "Broken"
        }
        fn connect_tcp<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
            panic!("the outbound is broken")
        }
    }

    /// A test whose task dies ends without a result, and does not stand in
    /// the way of the next test of the policy.
    #[tokio::test]
    async fn a_test_that_died_is_run_again() {
        let book = book();
        let died = book.test(case("P", Arc::new(Broken), 1)).await;
        assert_eq!(died.outcome, Err("the test did not finish".to_string()));
        assert_eq!(book.result("P", 1), None);
        let gate = Arc::new(Gate::default());
        book.test(case("P", gate.clone(), 1)).await;
        assert_eq!(gate.dials.load(Ordering::SeqCst), 1);
        assert!(book.result("P", 1).is_some());
    }

    /// A test outlives whoever asked for it: its result is kept.
    #[tokio::test]
    async fn a_test_ends_even_when_nobody_waits_any_more() {
        let gate = Arc::new(Gate {
            hold: Duration::from_millis(100),
            ..Gate::default()
        });
        let book = book();
        let waiting = tokio::time::timeout(
            Duration::from_millis(10),
            book.test(case("P", gate.clone(), 1)),
        )
        .await;
        assert!(waiting.is_err(), "gave up waiting");
        let deadline = Instant::now() + Duration::from_secs(5);
        while book.result("P", 1).is_none() {
            assert!(Instant::now() < deadline, "the test never ended");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(gate.dials.load(Ordering::SeqCst), 1);
    }

    /// A test of a definition that has since changed does not undo the
    /// current one's result, whichever ends last.
    #[tokio::test]
    async fn a_superseded_test_leaves_the_current_result_alone() {
        let slow = Arc::new(Gate {
            hold: Duration::from_millis(200),
            ..Gate::default()
        });
        let fast = Arc::new(Gate::default());
        let book = book();

        let slow_task = {
            let (book, case) = (book.clone(), case("P", slow.clone(), 1));
            tokio::spawn(async move { book.test(case).await })
        };

        // Poll until slow test has started (dial count reaches 1)
        let deadline = Instant::now() + Duration::from_secs(5);
        while slow.dials.load(Ordering::SeqCst) < 1 {
            assert!(Instant::now() < deadline, "slow test never started");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Now test with a different key (simulating definition change)
        let fast_result = book.test(case("P", fast.clone(), 2)).await;
        assert!(fast_result.outcome.is_err());

        // Wait for the slow test to finish
        let _ = slow_task.await;

        // The current key's result should persist, the old key's should not
        assert!(book.result("P", 2).is_some(), "current result should exist");
        assert_eq!(
            book.result("P", 1),
            None,
            "superseded result should not exist"
        );
    }
}
