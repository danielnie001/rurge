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
mod limit;
mod packet;
mod session;
mod space;
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
use tcp::{Admission, Budget, Shard};
use tokio::sync::mpsc;
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};
use tokio_util::task::TaskTracker;

/// The largest receive buffer of any TCP socket on either side of the
/// adapter. With 65535 bytes the window scale is 0 (smoltcp tcp.rs 538: the
/// scale is the bit length of the buffer's size less 16), which keeps smoltcp
/// 0.12's window-edge arithmetic from underflowing (design 5.3, H2).
///
/// The guard at the edge of the window (`tcp::guard`) rests on it as well: the
/// edge it keeps moves only forward while the scale is 0. At a scale of 1 or
/// more the window is told in units of 2^scale bytes, and the edge can move
/// back by up to 2^scale - 1 bytes as the buffer fills, so the guard would drop
/// segments a sender sent inside an earlier edge (the last of a burst that
/// filled the window), and the sender would send them again after a timeout.
/// A larger buffer is a change of the guard before it is a change of this
/// number. The assertion below, and `the_tun_never_scales_its_window`, see to
/// it that nobody raises it without reading this.
pub const RX_MAX: usize = 65_535;

const _: () = assert!(RX_MAX <= 65_535, "a larger buffer scales the window");

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
        let admission = Arc::new(Admission::new(life.clone(), stats.clone(), &options));
        let tasks = TaskTracker::new();
        let writer_queue = options.writer_queue.max(1);
        let (to_writer, from_shards) = mpsc::channel(writer_queue);
        #[cfg(test)]
        let mut shards = Vec::new();
        let mut to_shards = Vec::new();
        let mut random_failure = None;
        for index in 0..options.shards.max(1) {
            let seed = getrandom::u64().unwrap_or_else(|error| {
                random_failure.get_or_insert(error);
                0x5eed
            });
            let shard = Arc::new(Shard::new(
                index,
                mtu,
                epoch,
                seed,
                budget.clone(),
                admission.clone(),
                &options,
            ));
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
        // once for the TUN, however many shards took the fallback
        if let Some(error) = random_failure {
            tracing::warn!(
                %error,
                "tun: the system's random source failed; the TUN's initial sequence numbers are predictable"
            );
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
            options.ipv6,
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
                #[cfg(test)]
                admission,
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TunOptions {
    /// Whether IPv6 packets are handled; with it off they are dropped as unsupported.
    pub ipv6: bool,
    pub drop_hold: Duration,
    /// How many shards the TCP stack is divided into, each with a stack and a task of its own. The default was chosen by the benchmark gate; see the phase 3 / M3a plan's execution record row 10.
    pub shards: usize,
    /// The smallest buffer of a TCP socket, each way: what every socket made once the budget is spent gets. The default was chosen by the benchmark gate; see the phase 3 / M3a plan's execution record row 10.
    pub buffer_min: usize,
    pub tx_max: usize,
    pub budget: usize,
    /// The most TCP sockets the whole TUN has; a SYN over it is answered with a reset.
    pub max_sockets: usize,
    /// The most connections of the whole TUN that have not completed their handshake.
    pub max_half_open: usize,
    /// How long a connection has to complete its handshake before it is reset.
    pub half_open_timeout: Duration,
    /// The most that the connection of a stream that was dropped stays, waiting for the app to acknowledge its FIN; past that it is reset.
    pub linger: Duration,
    /// A connection idle for this long sends the app a keep-alive.
    pub keep_alive: Duration,
    /// A connection that has had nothing from the app for this long is reset.
    pub timeout: Duration,
    pub shard_queue: usize,
    pub writer_queue: usize,
}

impl Default for TunOptions {
    fn default() -> TunOptions {
        TunOptions {
            ipv6: true,
            drop_hold: Duration::from_secs(30),
            shards: 256,
            buffer_min: 4 * 1024,
            tx_max: 256 * 1024,
            budget: 64 << 20,
            max_sockets: 16_384,
            max_half_open: 2_048,
            half_open_timeout: Duration::from_secs(10),
            linger: Duration::from_secs(60),
            keep_alive: Duration::from_secs(30),
            timeout: Duration::from_secs(600),
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
    #[cfg(test)]
    admission: Arc<Admission>,
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

    /// The sockets the TUN counts against `max_sockets`: what
    /// `sockets_in_use` counts, as the shards add and take sockets.
    #[cfg(test)]
    pub(crate) fn sockets_counted(&self) -> usize {
        self.inner.admission.sockets()
    }

    /// The connections the TUN counts against `max_half_open`: those whose
    /// handshake is not over.
    #[cfg(test)]
    pub(crate) fn half_open(&self) -> usize {
        self.inner.admission.half_open()
    }

    /// What the shards keep of their connections besides the sockets, added
    /// up: every count is 0 once no connection is left behind.
    #[cfg(test)]
    pub(crate) fn records(&self) -> tcp::shard::Records {
        self.inner
            .shards
            .iter()
            .map(|shard| shard.state().records())
            .fold(tcp::shard::Records::default(), |all, one| all + one)
    }

    /// The bytes waiting in the send buffers of every TCP socket, and the room
    /// those buffers have in all.
    #[cfg(test)]
    pub(crate) fn send_buffers(&self) -> (usize, usize) {
        self.inner
            .shards
            .iter()
            .map(|shard| shard.state().send_buffers())
            .fold((0, 0), |all, one| (all.0 + one.0, all.1 + one.1))
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
