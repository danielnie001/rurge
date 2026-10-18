//! The buffers of the TUN's TCP sockets come out of one budget (phase 3 M3
//! design 5.4): a socket gets its two buffers when it is made, sized by what
//! is left of the budget then, and keeps them until it goes.

use crate::RX_MAX;
use std::sync::atomic::{AtomicUsize, Ordering};

/// What is left of the budget is shared out in this many parts: a new
/// socket gets one part for each of its buffers.
pub(crate) const DIVISOR: usize = 32;

/// The two buffers of one socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sizes {
    pub(crate) rx: usize,
    pub(crate) tx: usize,
}

pub(crate) struct Budget {
    budget: usize,
    min: usize,
    tx_max: usize,
    /// The buffers of every socket there is, in bytes.
    used: AtomicUsize,
}

impl Budget {
    pub(crate) fn new(budget: usize, min: usize, tx_max: usize) -> Budget {
        Budget {
            budget,
            min,
            tx_max,
            used: AtomicUsize::new(0),
        }
    }

    /// The buffers of a new socket, taken from the budget: each is what is
    /// left divided by `DIVISOR`, at least `min`, and the receive buffer at
    /// most `RX_MAX` (a window scale of 0, phase 3 M3 design 5.3 H2), the
    /// send buffer at most `tx_max`. When the budget is spent every new
    /// socket gets `min`, so the worst total is the budget and two times
    /// `min` for each socket. What is left is read and the share added in
    /// one atomic step, so the bound holds when shards take at once too:
    /// each take sees what every take before it took.
    pub(crate) fn take(&self) -> Sizes {
        let sizes = |used: usize| {
            let left = self.budget.saturating_sub(used);
            let share = (left / DIVISOR).max(self.min);
            Sizes {
                rx: share.min(RX_MAX),
                tx: share.min(self.tx_max),
            }
        };
        // the update always gives a value, so both answers are what it replaced
        let (Ok(used) | Err(used)) =
            self.used
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                    let taken = sizes(used);
                    Some(used + taken.rx + taken.tx)
                });
        sizes(used)
    }

    pub(crate) fn give_back(&self, sizes: Sizes) {
        self.used.fetch_sub(sizes.rx + sizes.tx, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }
}
