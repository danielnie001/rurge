//! The bytes that wait in queues for someone to take them (phase 3 M3c, M4a):
//! one budget for the datagrams that wait in the UDP sessions (`udp_bytes`) and
//! one for the packets that wait in the shards' queues (`queue_bytes`). The
//! datagrams' budget counts the length of each datagram's payload and the
//! packets' the capacity of each packet, the memory it holds (`Vec::capacity()`,
//! the phase 3 / M4a plan's P9). Each session queues at most `udp_queue`
//! datagrams and there can be thousands of sessions, and a shard's queue is a
//! count of packets (`shard_queue`), so the queues alone put no bound on the
//! memory they hold. What waits takes its bytes out of the budget as it joins a
//! queue, and holds them in a [`Reserved`] that goes with it: the reservation
//! gives them back when it is dropped, so every way a datagram or a packet can
//! leave a queue (someone takes it, the queue is full and gives it back, the
//! queue is dropped with it inside) gives its bytes back without keeping a
//! count of its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// How many bytes may wait in the queues of a budget at once, and how many do.
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

impl Reserved {
    /// A reservation of no bytes, behind a budget of none: what a test that
    /// hands a packet on without a queue holds it by.
    #[cfg(test)]
    pub(crate) fn none() -> Reserved {
        Reserved {
            budget: Arc::new(QueuedBytes::new(0)),
            bytes: 0,
        }
    }
}

impl Drop for Reserved {
    fn drop(&mut self) {
        self.budget.queued.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}
