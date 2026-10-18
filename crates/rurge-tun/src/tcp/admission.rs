//! What lets a new SYN in (phase 3 M3 design 5.2, 5.4): whether the TUN is
//! stopping, how many TCP sockets and half-open connections it may have, and
//! how many resets of its own it may send. All of it is the TUN's, not a
//! shard's: every shard holds the same `Admission`, so the caps and the limit
//! hold however the connections fall among the shards.

use crate::limit::Limiter;
use crate::packet::Reason;
use crate::stats::Stats;
use crate::{Life, TunOptions};
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// rurge's own resets are let through this often (the compatibility matrix:
/// after 100 in 3 s the answer to a SYN is none).
const RESETS: usize = 100;
const RESETS_WINDOW: Duration = Duration::from_secs(3);

pub(crate) struct Admission {
    life: Arc<Life>,
    stats: Arc<Stats>,
    resets: Mutex<Limiter>,
    max_sockets: usize,
    max_half_open: usize,
    /// The TCP sockets of every shard, and among them the connections whose
    /// handshake is not over. A shard raises both when a SYN gets a socket
    /// and lowers them as the connection goes (the half-open one sooner, once
    /// the handshake is over).
    sockets: AtomicUsize,
    half_open: AtomicUsize,
}

impl Admission {
    pub(crate) fn new(life: Arc<Life>, stats: Arc<Stats>, options: &TunOptions) -> Admission {
        Admission {
            life,
            stats,
            resets: Mutex::new(Limiter::new(RESETS, RESETS_WINDOW)),
            max_sockets: options.max_sockets,
            max_half_open: options.max_half_open,
            sockets: AtomicUsize::new(0),
            half_open: AtomicUsize::new(0),
        }
    }

    /// The TUN is stopping: a new SYN is answered with a reset.
    pub(crate) fn stopping(&self) -> bool {
        self.life.is_stopped()
    }

    /// The places for a new socket, which starts half-open: both counts go up
    /// by one, or neither does and this answers `false`. Each is raised in
    /// one atomic step that tests its cap, so shards that claim at the same
    /// moment never take more places than the caps give. The price is that a
    /// place claimed for a moment and then given back (for a SYN that is
    /// turned away, or none that smoltcp takes) can make another shard's
    /// claim fail at the edge of a cap.
    pub(crate) fn claim(&self) -> bool {
        // half-open first: the cap a flood of SYNs meets
        if raise(&self.half_open, self.max_half_open).is_err() {
            return false;
        }
        if raise(&self.sockets, self.max_sockets).is_err() {
            self.release_half_open();
            return false;
        }
        true
    }

    /// The places claimed, given back: no connection came of the SYN.
    pub(crate) fn unclaim(&self) {
        self.release_half_open();
        self.release_socket();
    }

    /// The handshake of a connection claimed is over, or the connection is
    /// gone before it was.
    pub(crate) fn release_half_open(&self) {
        self.half_open.fetch_sub(1, Ordering::Relaxed);
    }

    /// A socket claimed is gone.
    pub(crate) fn release_socket(&self) {
        self.sockets.fetch_sub(1, Ordering::Relaxed);
    }

    /// Whether one more reset of rurge's own may be sent at `now`. The limit
    /// is the TUN's, so a lock of it that a panic poisoned is taken all the
    /// same: what is behind it is a queue of times, whole after any step of
    /// it, and not worth every shard's next SYN.
    pub(crate) fn allow_reset(&self, now: Instant) -> bool {
        self.resets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .allow(now)
    }

    /// A shard's stack panicked and the shard was rebuilt.
    pub(crate) fn stack_panicked(&self) {
        Stats::count(&self.stats.stack_panics);
    }

    /// Counts and says a packet turned away for `reason`: a SYN, or the
    /// reset that would have answered one. `src` and `dst` are the SYN's.
    pub(crate) fn count(&self, reason: Reason, src: IpAddr, dst: IpAddr) {
        // not `self.stats.drop(..)`: on an `Arc` that is `Drop::drop`
        Stats::drop(&self.stats, reason, Some(src), Some(dst));
    }

    /// Panics on another thread with the limit on resets held, which poisons
    /// its lock.
    #[cfg(test)]
    pub(crate) fn poison_the_limit_on_resets(&self) {
        let poisoning = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _held = self.resets.lock();
                    panic!("poisoning the limit on resets");
                })
                .join()
        });
        assert!(poisoning.is_err());
    }

    #[cfg(test)]
    pub(crate) fn sockets(&self) -> usize {
        self.sockets.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn half_open(&self) -> usize {
        self.half_open.load(Ordering::Relaxed)
    }
}

/// Adds one to `count` if it is under `cap`, in one atomic step.
fn raise(count: &AtomicUsize, cap: usize) -> Result<usize, usize> {
    count.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        (count < cap).then_some(count + 1)
    })
}
