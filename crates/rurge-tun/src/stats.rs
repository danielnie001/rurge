//! What the TUN counts of the packets it drops, one counter for each reason,
//! and what it says of them (phase 3 M3 design 14); [`Counters`] is a
//! snapshot of the counters.

use crate::Counters;
use crate::packet::Reason;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};

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
    pub(crate) ip_rewrite: AtomicU64,
    pub(crate) reflected: AtomicU64,
    pub(crate) stack_panics: AtomicU64,
    warnings: Warnings,
}

/// The reasons that have been said at WARN: each once for a TUN.
#[derive(Default)]
pub(crate) struct Warnings {
    /// A bit for each reason, set when it is said.
    said: AtomicU16,
}

impl Warnings {
    /// Whether `reason` is to be said at WARN now: the first time this is
    /// asked of it. Never for a reason that is not said at all.
    pub(crate) fn first_time(&self, reason: Reason) -> bool {
        let bit = 1 << reason as u16;
        !reason.is_silent() && self.said.fetch_or(bit, Ordering::Relaxed) & bit == 0
    }
}

impl Stats {
    pub(crate) fn count(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts a packet dropped for `reason` and says so: at WARN the first
    /// time the TUN drops one for a reason that is not wanted, without an
    /// address, at DEBUG after that with the ends of the packet where they
    /// were read, and not at all for a martian. A drop that is wanted (the
    /// profile's `[IP Rewrite]` asked for it) is never said at WARN and is
    /// said at DEBUG every time. Every drop goes through here.
    pub(crate) fn drop(&self, reason: Reason, src: Option<IpAddr>, dst: Option<IpAddr>) {
        Stats::count(self.counter(reason));
        if reason.is_silent() {
            return;
        }
        if !reason.is_wanted() && self.warnings.first_time(reason) {
            tracing::warn!(
                "tun: a packet was dropped (reason: {reason}); later drops for this reason are logged at debug level"
            );
        } else {
            tracing::debug!(
                %reason,
                src = src.map(tracing::field::display),
                dst = dst.map(tracing::field::display),
                "tun: packet dropped"
            );
        }
    }

    /// Counts a packet `[IP Rewrite]` sent back: the writer's queue took it.
    /// It is no drop, and nothing is said of it.
    pub(crate) fn reflected(&self) {
        Stats::count(&self.reflected);
    }

    fn counter(&self, reason: Reason) -> &AtomicU64 {
        match reason {
            Reason::Malformed => &self.malformed,
            Reason::Fragment => &self.fragment,
            Reason::Martian => &self.martian,
            Reason::Unsupported => &self.unsupported,
            Reason::QueueFull => &self.queue_full,
            Reason::UnknownFake => &self.unknown_fake,
            Reason::Busy => &self.busy,
            Reason::BeyondWindow => &self.beyond_window,
            Reason::RstLimited => &self.rst_limited,
            Reason::IcmpLimited => &self.icmp_limited,
            Reason::IpRewrite => &self.ip_rewrite,
        }
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
            ip_rewrite: read(&self.ip_rewrite),
            reflected: read(&self.reflected),
            stack_panics: read(&self.stack_panics),
        }
    }
}
