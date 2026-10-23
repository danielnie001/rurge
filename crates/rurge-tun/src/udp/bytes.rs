//! The bytes of the datagrams that wait in the UDP sessions (phase 3 M3c): one
//! budget for the whole TUN, `udp_bytes`. Each session queues at most
//! `udp_queue` datagrams and there can be thousands of sessions, so the queues
//! alone put no bound on the memory they hold. A datagram takes the bytes of its
//! payload from the budget as it joins a queue, and holds them in a
//! [`Reserved`] that goes with it: the reservation gives them back when it is
//! dropped, so every way a datagram can leave a queue (the engine takes it, the
//! queue is full and gives it back, the session ends and the queue is dropped
//! with it inside) gives its bytes back without keeping a count of its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// How many bytes of payload may wait in the sessions' queues at once, and how
/// many do.
pub(crate) struct QueuedBytes {
    limit: usize,
    queued: AtomicUsize,
}

impl QueuedBytes {
    pub(crate) fn new(limit: usize) -> QueuedBytes {
        QueuedBytes {
            limit,
            queued: AtomicUsize::new(0),
        }
    }

    /// Takes `bytes` out of the budget if they fit in what is left, to the byte;
    /// else takes nothing. Never waits.
    pub(crate) fn reserve(self: &Arc<Self>, bytes: usize) -> Option<Reserved> {
        self.queued
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |queued| {
                queued
                    .checked_add(bytes)
                    .filter(|total| *total <= self.limit)
            })
            .ok()
            .map(|_| Reserved {
                budget: self.clone(),
                bytes,
            })
    }

    /// The bytes that are reserved now.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }
}

/// Bytes taken out of the budget, given back when this is dropped.
pub(crate) struct Reserved {
    budget: Arc<QueuedBytes>,
    bytes: usize,
}

impl Drop for Reserved {
    fn drop(&mut self) {
        self.budget.queued.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}
