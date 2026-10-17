//! The process behind a session (phase 3 M2 design 4.2): a slot the session
//! carries, settled once by a lookup running in the background, and waited
//! for by whoever needs the answer — the rules, for as long as they care to.

use rurge_config::session::ProcessInfo;
use std::fmt;
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tokio::sync::Notify;

/// Where the process behind a session is put when it is found. Clones are
/// the same slot: the session, its request record and, for the flows of a
/// UDP association, every flow hold one.
///
/// A slot is settled once. `Some(Some(process))` is a process found,
/// `Some(None)` a lookup that found none (or that nobody ran: a session
/// rurge started itself, one that did not come from this machine), and the
/// slot is not settled while its lookup is under way.
#[derive(Clone)]
pub struct ProcessSlot(Arc<Inner>);

struct Inner {
    /// When the lookup that settles the slot was launched; `None` for a slot
    /// that was settled to begin with.
    launched: Option<Instant>,
    /// Set by `settle`, once.
    value: OnceLock<Option<ProcessInfo>>,
    /// Woken, for every waiter, after `value` is set.
    settled: Notify,
}

impl ProcessSlot {
    fn new(launched: Option<Instant>, value: OnceLock<Option<ProcessInfo>>) -> ProcessSlot {
        ProcessSlot(Arc::new(Inner {
            launched,
            value,
            settled: Notify::new(),
        }))
    }

    fn settled_with(process: Option<ProcessInfo>) -> ProcessSlot {
        ProcessSlot::new(None, OnceLock::from(process))
    }

    /// Settled, with no process; nothing is looked for.
    pub fn unknown() -> ProcessSlot {
        ProcessSlot::settled_with(None)
    }

    /// Settled with `process`; nothing is looked for.
    pub fn known(process: ProcessInfo) -> ProcessSlot {
        ProcessSlot::settled_with(Some(process))
    }

    /// Not settled: a lookup is launched now.
    pub fn pending() -> ProcessSlot {
        ProcessSlot::pending_at(Instant::now())
    }

    /// Not settled: a lookup was launched at `launched`.
    pub fn pending_at(launched: Instant) -> ProcessSlot {
        ProcessSlot::new(Some(launched), OnceLock::new())
    }

    /// Settles the slot with what the lookup found (`None`: nothing) and
    /// wakes whoever waits for it. The first settle wins; later ones change
    /// nothing and wake nobody.
    pub fn settle(&self, process: Option<ProcessInfo>) {
        if self.0.value.set(process).is_ok() {
            self.0.settled.notify_waiters();
        }
    }

    /// What the slot holds: `None` while its lookup is under way.
    pub fn get(&self) -> Option<&Option<ProcessInfo>> {
        self.0.value.get()
    }

    /// The process, once it is found.
    pub fn found(&self) -> Option<&ProcessInfo> {
        self.0.value.get().and_then(Option::as_ref)
    }

    /// When the lookup that settles the slot was launched; `None` for a slot
    /// that never had one.
    pub fn launched(&self) -> Option<Instant> {
        self.0.launched
    }

    /// Waits for the slot to be settled, until `deadline`. `None`: the
    /// deadline came first. A slot that is settled answers at once, whatever
    /// the deadline.
    pub async fn settled_by(&self, deadline: Instant) -> Option<Option<ProcessInfo>> {
        // Made and registered before the slot is read: a `settle` that comes
        // between the two wakes this wait instead of passing it by.
        let settled = self.0.settled.notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        if let Some(value) = self.0.value.get() {
            return Some(value.clone());
        }
        let _ = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), settled).await;
        // woken, the slot holds its answer; at the deadline, whatever it
        // holds by now — a settle that came along with the deadline counts
        self.0.value.get().cloned()
    }
}

/// Whether the slot is settled and the PID, never the name or the path: a
/// request record is printed in logs and in test output, and the path of a
/// program often has the user's name in it.
impl fmt::Debug for ProcessSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessSlot")
            .field("settled", &self.0.value.get().is_some())
            .field("pid", &self.found().and_then(|p| p.pid))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::session::ProcessInfo;
    use std::time::{Duration, Instant};

    fn app(pid: u32) -> ProcessInfo {
        ProcessInfo {
            pid: Some(pid),
            name: "app.exe".to_string(),
            path: Some("C:\\Apps\\app.exe".to_string()),
        }
    }

    /// A moment that has passed already, for a deadline nobody waits for.
    fn long_ago() -> Instant {
        Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("the machine has been up for a second")
    }

    /// Settled once: the lookup that ends first is the answer, whatever
    /// comes after.
    #[tokio::test]
    async fn the_first_settle_wins() {
        let slot = ProcessSlot::pending();
        assert_eq!(slot.get(), None, "nothing settled it yet");
        assert_eq!(slot.found(), None);
        slot.settle(Some(app(1)));
        slot.settle(Some(app(2)));
        slot.settle(None);
        assert_eq!(slot.get(), Some(&Some(app(1))));
        assert_eq!(slot.found(), Some(&app(1)));
        // "found nothing" is an answer too, and it stands
        let none = ProcessSlot::pending();
        none.settle(None);
        none.settle(Some(app(3)));
        assert_eq!(none.get(), Some(&None));
        assert_eq!(none.found(), None);
        // every holder of the slot sees the same answer
        let copy = slot.clone();
        assert_eq!(copy.found(), Some(&app(1)));
    }

    /// A wait that is under way ends when the slot is settled, not when its
    /// deadline comes.
    #[tokio::test]
    async fn a_waiter_wakes_when_the_slot_settles() {
        let slot = ProcessSlot::pending();
        let waiting = tokio::spawn({
            let slot = slot.clone();
            async move {
                slot.settled_by(Instant::now() + Duration::from_secs(60))
                    .await
            }
        });
        // let it start waiting, so that the settle wakes it
        tokio::task::yield_now().await;
        slot.settle(Some(app(7)));
        let answer = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the waiter wakes when the slot settles, not at its deadline")
            .expect("the waiter ends");
        assert_eq!(answer, Some(Some(app(7))));
        // and every waiter wakes, not one of them
        let slot = ProcessSlot::pending();
        let waiters: Vec<_> = (0..4)
            .map(|_| {
                let slot = slot.clone();
                tokio::spawn(async move {
                    slot.settled_by(Instant::now() + Duration::from_secs(60))
                        .await
                })
            })
            .collect();
        tokio::task::yield_now().await;
        slot.settle(None);
        for waiter in waiters {
            let answer = tokio::time::timeout(Duration::from_secs(5), waiter)
                .await
                .expect("every waiter wakes")
                .expect("the waiter ends");
            assert_eq!(answer, Some(None));
        }
    }

    /// A settle racing the wait that is about to start is never passed by:
    /// whichever comes first, the waiter has the answer long before a
    /// deadline that is a minute away.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_settle_racing_a_waiter_is_never_missed() {
        for round in 0..3000u32 {
            let slot = ProcessSlot::pending();
            let waiting = tokio::spawn({
                let slot = slot.clone();
                async move {
                    slot.settled_by(Instant::now() + Duration::from_secs(60))
                        .await
                }
            });
            if round % 3 == 0 {
                tokio::task::yield_now().await;
            }
            slot.settle(Some(app(round)));
            let answer = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .unwrap_or_else(|_| panic!("round {round}: the settle was missed"))
                .expect("the waiter ends");
            assert_eq!(answer, Some(Some(app(round))), "round {round}");
        }
    }

    /// `None` says the deadline came first; the slot is still waiting for its
    /// lookup, which settles it later all the same.
    #[tokio::test]
    async fn a_deadline_that_comes_first_is_none() {
        let slot = ProcessSlot::pending();
        let begun = Instant::now();
        let answer = slot.settled_by(begun + Duration::from_millis(50)).await;
        assert_eq!(answer, None);
        assert!(begun.elapsed() < Duration::from_secs(5));
        assert_eq!(slot.get(), None, "the slot is still pending");
        // a deadline that is past is answered at once, with the same
        assert_eq!(slot.settled_by(long_ago()).await, None);
        slot.settle(Some(app(9)));
        assert_eq!(slot.get(), Some(&Some(app(9))));
        // the lookup that came late is there for whoever asks again
        assert_eq!(slot.settled_by(long_ago()).await, Some(Some(app(9))));
    }

    /// A slot that is settled answers at once, however the deadline stands:
    /// past, or a minute away.
    #[tokio::test]
    async fn a_settled_slot_answers_whatever_the_deadline() {
        let far = Instant::now() + Duration::from_secs(60);
        let known = ProcessSlot::known(app(5));
        assert_eq!(known.settled_by(long_ago()).await, Some(Some(app(5))));
        assert_eq!(known.settled_by(far).await, Some(Some(app(5))));
        assert_eq!(known.launched(), None, "no lookup was launched for it");
        let unknown = ProcessSlot::unknown();
        assert_eq!(unknown.settled_by(long_ago()).await, Some(None));
        assert_eq!(unknown.settled_by(far).await, Some(None));
        assert_eq!(unknown.launched(), None);
        assert_eq!(unknown.get(), Some(&None));
        // settled by a lookup that was launched: the same
        let launched = ProcessSlot::pending_at(long_ago());
        launched.settle(None);
        assert_eq!(launched.settled_by(long_ago()).await, Some(None));
        assert!(launched.launched().is_some());
        // a settle after the answer changes nothing for a slot that was
        // never pending
        known.settle(None);
        assert_eq!(known.found(), Some(&app(5)));
    }

    /// `pending` launches the lookup now; `pending_at` says when it was.
    #[test]
    fn a_pending_slot_remembers_when_its_lookup_was_launched() {
        let before = Instant::now();
        let slot = ProcessSlot::pending();
        let launched = slot.launched().expect("a lookup was launched");
        assert!(launched >= before && launched <= Instant::now());
        let at = long_ago();
        assert_eq!(ProcessSlot::pending_at(at).launched(), Some(at));
    }

    /// A record is printed in logs and in test output: the slot says whether
    /// it is settled and the PID, never the name or the path (the path of a
    /// program on Windows often has the user's name in it).
    #[test]
    fn the_debug_of_a_slot_shows_no_name_or_path() {
        let found = ProcessSlot::known(ProcessInfo {
            pid: Some(4321),
            name: "secret-name.exe".to_string(),
            path: Some("C:\\Programs\\private-dir\\secret-name.exe".to_string()),
        });
        let text = format!("{found:?}");
        assert!(text.contains("4321"), "{text}");
        assert!(!text.contains("secret"), "{text}");
        assert!(!text.contains("private-dir"), "{text}");
        assert!(!text.contains("Programs"), "{text}");
        let pending = format!("{:?}", ProcessSlot::pending());
        assert!(pending.contains("false"), "settled or not: {pending}");
        let unknown = format!("{:?}", ProcessSlot::unknown());
        assert!(unknown.contains("true"), "settled or not: {unknown}");
        assert!(!unknown.contains("4321"), "{unknown}");
    }
}
