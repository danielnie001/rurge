//! What the TUN counts of the packets it drops, one counter for each reason
//! (phase 3 M3 design 14); [`Counters`] is a snapshot of them.

use crate::Counters;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(crate) struct Stats {
    pub(crate) malformed: AtomicU64,
    pub(crate) fragment: AtomicU64,
    pub(crate) martian: AtomicU64,
    pub(crate) unsupported: AtomicU64,
    pub(crate) queue_full: AtomicU64,
    pub(crate) unknown_fake: AtomicU64,
    pub(crate) busy: AtomicU64,
    pub(crate) beyond_window: AtomicU64,
    pub(crate) rst_limited: AtomicU64,
    pub(crate) icmp_limited: AtomicU64,
    pub(crate) stack_panics: AtomicU64,
}

impl Stats {
    pub(crate) fn count(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn counters(&self) -> Counters {
        let read = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        Counters {
            malformed: read(&self.malformed),
            fragment: read(&self.fragment),
            martian: read(&self.martian),
            unsupported: read(&self.unsupported),
            queue_full: read(&self.queue_full),
            unknown_fake: read(&self.unknown_fake),
            busy: read(&self.busy),
            beyond_window: read(&self.beyond_window),
            rst_limited: read(&self.rst_limited),
            icmp_limited: read(&self.icmp_limited),
            stack_panics: read(&self.stack_panics),
        }
    }
}
