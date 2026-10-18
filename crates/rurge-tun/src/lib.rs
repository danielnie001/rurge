//! The core of rurge's virtual adapter: IP packets come in from a device,
//! TCP connections go out to the session pipeline, on a userspace TCP/IP
//! stack (phase 3 M3 design: `docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md`).
//! Platform-free: the device is a trait, and the one implemented here lives
//! in memory.
//!
//! Its tasks: one reads the device and sends each packet on its way
//! (`dispatch`), one shard of the TCP stack for each part of the
//! connections (`tcp`), one session for each connection (`session`), and
//! one writes to the device what all of them send (`writer`).

pub mod device;
mod dispatch;
mod packet;
mod session;
mod stats;
mod tcp;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod writer;

use device::Device;
use rurge_inbound::Dialer;
use stats::Stats;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tcp::{Budget, Shard};
use tokio::sync::mpsc;
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};
use tokio_util::task::TaskTracker;

/// The largest receive buffer of any TCP socket on either side of the
/// adapter. With 65535 bytes the window scale is 0, which keeps smoltcp 0.12's
/// window-edge arithmetic from underflowing (design 5.3, H2).
pub const RX_MAX: usize = 65_535;

/// The smallest device MTU: IPv6's, above IPv4's 576. smoltcp 0.12 has
/// subtractions that underflow when the MTU is 0 or too small (design 5.3,
/// H8), and the device's MTU is rurge's own choice.
const MIN_MTU: usize = 1280;

pub struct Tun;

impl Tun {
    /// Starts the TUN's tasks on the current tokio runtime.
    pub fn start(
        device: Arc<dyn Device>,
        dialer: Arc<dyn Dialer>,
        options: TunOptions,
    ) -> TunHandle {
        let mtu = device.mtu();
        assert!(mtu >= MIN_MTU, "the device's MTU must be at least 1280");
        let epoch = Instant::now();
        let life = Arc::new(Life::default());
        let stats = Arc::new(Stats::default());
        let budget = Arc::new(Budget::new(
            options.budget,
            options.buffer_min,
            options.tx_max,
        ));
        let tasks = TaskTracker::new();
        let writer_queue = options.writer_queue.max(1);
        let (to_writer, from_shards) = mpsc::channel(writer_queue);
        #[cfg(test)]
        let mut shards = Vec::new();
        let mut to_shards = Vec::new();
        for _ in 0..options.shards.max(1) {
            let shard = Arc::new(Shard::new(mtu, epoch, budget.clone()));
            let (to_shard, packets) = mpsc::channel(options.shard_queue.max(1));
            #[cfg(test)]
            shards.push(shard.clone());
            tasks.spawn(tcp::shard::run(
                shard,
                packets,
                to_writer.clone(),
                dialer.clone(),
                life.clone(),
                tasks.clone(),
            ));
            to_shards.push(to_shard);
        }
        // the shards hold the writer's queue: it ends once they all have
        drop(to_writer);
        tasks.spawn(writer::run(
            device.clone(),
            from_shards,
            writer_queue,
            life.clone(),
        ));
        tasks.spawn(dispatch::run(
            device,
            to_shards,
            stats.clone(),
            life.clone(),
        ));
        // what is spawned later, the sessions, is waited for all the same
        tasks.close();
        TunHandle {
            inner: Arc::new(Inner {
                life,
                tasks,
                stats,
                #[cfg(test)]
                shards,
                #[cfg(test)]
                budget,
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TunOptions {
    pub ipv6: bool,
    pub drop_hold: Duration,
    pub shards: usize,
    pub buffer_min: usize,
    pub tx_max: usize,
    pub budget: usize,
    pub max_sockets: usize,
    pub max_half_open: usize,
    pub half_open_timeout: Duration,
    pub linger: Duration,
    pub shard_queue: usize,
    pub writer_queue: usize,
}

impl Default for TunOptions {
    fn default() -> TunOptions {
        TunOptions {
            ipv6: true,
            drop_hold: Duration::from_secs(30),
            shards: 64,
            buffer_min: 8 * 1024,
            tx_max: 256 * 1024,
            budget: 64 << 20,
            max_sockets: 16_384,
            max_half_open: 2_048,
            half_open_timeout: Duration::from_secs(10),
            linger: Duration::from_secs(60),
            shard_queue: 1_024,
            writer_queue: 256,
        }
    }
}

/// Whether the TUN is ending, and why: the first reason given stays.
#[derive(Default)]
pub(crate) struct Life {
    stop: CancellationToken,
    ending: Mutex<Option<Ending>>,
}

impl Life {
    pub(crate) fn end(&self, ending: Ending) {
        self.ending
            .lock()
            .expect("the TUN's ending")
            .get_or_insert(ending);
        self.stop.cancel();
    }

    pub(crate) fn stopped(&self) -> WaitForCancellationFuture<'_> {
        self.stop.cancelled()
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.stop.is_cancelled()
    }

    fn ending(&self) -> Ending {
        self.ending
            .lock()
            .expect("the TUN's ending")
            .clone()
            .unwrap_or(Ending::Stopped)
    }
}

struct Inner {
    life: Arc<Life>,
    tasks: TaskTracker,
    stats: Arc<Stats>,
    #[cfg(test)]
    shards: Vec<Arc<Shard>>,
    #[cfg(test)]
    budget: Arc<Budget>,
}

#[derive(Clone)]
pub struct TunHandle {
    inner: Arc<Inner>,
}

impl TunHandle {
    pub fn stop(&self) {
        self.inner.life.end(Ending::Stopped);
    }

    /// Every task of the TUN, sessions included, has ended. Answers the same every time.
    pub async fn join(&self) -> Ending {
        self.inner.tasks.wait().await;
        self.inner.life.ending()
    }

    pub fn counters(&self) -> Counters {
        self.inner.stats.counters()
    }

    /// The TCP sockets of every shard.
    #[cfg(test)]
    pub(crate) fn sockets_in_use(&self) -> usize {
        self.inner
            .shards
            .iter()
            .map(|shard| shard.state().socket_count())
            .sum()
    }

    /// The bytes of the buffers of every TCP socket.
    #[cfg(test)]
    pub(crate) fn budget_in_use(&self) -> usize {
        self.inner.budget.used()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ending {
    Stopped,
    DeviceFailed(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub malformed: u64,
    pub fragment: u64,
    pub martian: u64,
    pub unsupported: u64,
    pub queue_full: u64,
    pub unknown_fake: u64,
    pub busy: u64,
    pub beyond_window: u64,
    pub rst_limited: u64,
    pub icmp_limited: u64,
    pub stack_panics: u64,
}

#[cfg(test)]
mod tests;
