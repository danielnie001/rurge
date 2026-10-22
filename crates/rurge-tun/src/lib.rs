//! The core of rurge's virtual adapter: IP packets come in from a device,
//! TCP connections go out to the session pipeline, on a userspace TCP/IP
//! stack (phase 3 M3 design: `docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md`).
//! Platform-free: the device is a trait, and the one implemented here lives
//! in memory.
//!
//! Its tasks: one reads the device and sends each packet on its way
//! (`dispatch`), one shard of the TCP stack for each part of the
//! connections (`tcp`), one session for each TCP connection (`session`) and
//! for each source endpoint that sends UDP (`udp`), and one writes to the
//! device what all of them send (`writer`).

pub mod device;
mod dispatch;
mod fragment;
mod hooks;
mod icmp;
mod limit;
mod packet;
mod session;
mod space;
mod stats;
mod tcp;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod udp;
mod writer;

pub use hooks::{Hooks, Pinger, TunSettings};

use device::Device;
use rurge_dns::fakeip::FakeIpPool;
use rurge_inbound::Dialer;
use session::Holds;
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

/// The core of the virtual adapter; [`Tun::start`] starts one.
pub struct Tun;

impl Tun {
    /// Starts the TUN's tasks on the current tokio runtime. They run until
    /// [`TunHandle::stop`] is called or the device fails. Dropping the handles
    /// does not stop them, and once the last is gone nothing can.
    ///
    /// `dialer` is where the sessions go (a TCP connection once its handshake
    /// is over, a UDP session once a datagram has come), `hooks` and `pinger`
    /// what the TUN asks of whoever started it, and `pool` the fake addresses:
    /// without one every address of the pool's space is one that names
    /// nothing, and a connection to it is reset.
    ///
    /// # Panics
    ///
    /// If the options are ones [`TunOptions::check`] refuses (they are not
    /// clamped to what can run: whoever starts a TUN checks them first and
    /// says what is wrong to whoever gave them), or if the device's MTU is
    /// below 1280 (smoltcp 0.12 cannot work with less, and the MTU is rurge's
    /// own choice), or if there is no tokio runtime to start the tasks on.
    pub fn start(
        device: Arc<dyn Device>,
        dialer: Arc<dyn Dialer>,
        hooks: Arc<dyn Hooks>,
        pinger: Arc<dyn Pinger>,
        pool: Option<Arc<FakeIpPool>>,
        options: TunOptions,
    ) -> TunHandle {
        if let Err(error) = options.check() {
            panic!("the TUN's options: {error}");
        }
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
        let writer_queue = options.writer_queue;
        let (to_writer, from_shards) = mpsc::channel(writer_queue);
        #[cfg(test)]
        let mut shards = Vec::new();
        let mut to_shards = Vec::new();
        let mut random_failure = None;
        for index in 0..options.shards {
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
                pool.clone(),
                &options,
            ));
            let (to_shard, packets) = mpsc::channel(options.shard_queue);
            #[cfg(test)]
            shards.push(shard.clone());
            tasks.spawn(tcp::shard::run(
                shard,
                packets,
                to_writer.clone(),
                dialer.clone(),
                Holds::from(&options),
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
        let ctx = Arc::new(dispatch::Context {
            dialer,
            pool: pool.clone(),
            hooks,
            pinger,
            writer: to_writer.clone(),
            stats: stats.clone(),
            life: life.clone(),
            options,
            tasks: tasks.clone(),
            mtu,
            sessions: udp::Sessions::default(),
            ids: fragment::Ids::new(),
            icmp_limit: Mutex::new(icmp::limiter()),
        });
        // the shards and the context hold the writer's queue: it ends once they
        // all have
        drop(to_writer);
        tasks.spawn(writer::run(
            device.clone(),
            from_shards,
            writer_queue,
            life.clone(),
        ));
        tasks.spawn(dispatch::run(device, to_shards, ctx));
        // what is spawned later, the sessions, is waited for all the same
        tasks.close();
        TunHandle {
            inner: Arc::new(Inner {
                life,
                tasks,
                stats,
                pool,
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

/// What a TUN is started with.
#[derive(Clone, Debug)]
pub struct TunOptions {
    /// Whether IPv6 packets are handled; with it off they are dropped as unsupported.
    pub ipv6: bool,
    /// REJECT-DROP: how long a session holds the connection it was rejected on, read and written by nobody, before the TUN closes its side (a FIN). A stop ends the hold at once, and so does the end of the connection (the app resets it, say). After it the TUN waits for the app to close its side, for `linger` at the most.
    pub drop_hold: Duration,
    /// How many shards the TCP stack is divided into, each with a stack and a task of its own. The default was chosen by the benchmark gate; see the phase 3 / M3a plan's execution record row 10.
    pub shards: usize,
    /// The smallest buffer of a TCP socket, each way: what every socket made once the budget is spent gets. The default was chosen by the benchmark gate; see the phase 3 / M3a plan's execution record row 10.
    pub buffer_min: usize,
    /// The largest send buffer of a TCP socket. A socket's buffers are sized by what is left of `budget` when it is made, at least `buffer_min`; this caps the send buffer, `RX_MAX` the receive buffer. The default was chosen by the benchmark gate; see the phase 3 / M3a plan's execution record row 10.
    pub tx_max: usize,
    /// The bytes of TCP socket buffers, both ways, that the TUN shares out as sockets are made: a new socket takes a share of what is left. Once it is spent every new socket gets `buffer_min` each way, so the buffers in all can come to more than this. The default was chosen by the benchmark gate; see the phase 3 / M3a plan's execution record row 10.
    pub budget: usize,
    /// The most TCP sockets the whole TUN has; a SYN over it is answered with a reset.
    pub max_sockets: usize,
    /// The most connections of the whole TUN that have not completed their handshake.
    pub max_half_open: usize,
    /// How long a connection has to complete its handshake before it is reset.
    pub half_open_timeout: Duration,
    /// The most that the connection of a stream that was dropped stays, waiting for the app to acknowledge its FIN; past that it is reset. Also the most that REJECT-DROP waits, after the hold (`drop_hold`), for the app to close its side while the TUN reads and throws away what it sends; past that the connection is reset.
    pub linger: Duration,
    /// A connection idle for this long sends the app a keep-alive.
    pub keep_alive: Duration,
    /// A connection that has had nothing from the app for this long is reset.
    pub timeout: Duration,
    /// How many packets the device's reader can queue for one shard. A packet for a shard whose queue is full is dropped and counted (`queue_full`).
    pub shard_queue: usize,
    /// How many batches of packets (one for each turn of a shard) can wait for the writer. A shard whose batch finds the queue full waits for room.
    pub writer_queue: usize,
    /// How long a UDP session, and each destination in it, can go without a packet either way before it ends. The default is the engine's own: its flows end after 60 seconds of quiet.
    pub udp_idle: Duration,
    /// How many packets can wait in a UDP session for the engine to take them. A packet for a session whose queue is full is dropped and counted (`queue_full`).
    pub udp_queue: usize,
    /// The most destinations one UDP session has, each a flow of the engine's, which takes 1024 in an association (and so does the default). A datagram to a destination beyond that many is dropped and counted (`busy`).
    pub max_udp_targets: usize,
    /// How long an echo request waits for its reply before it is given up.
    pub echo_timeout: Duration,
    /// The most echo requests waiting for a reply at once, in the whole TUN. One more is dropped and counted (`busy`).
    pub max_echoes: usize,
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
            udp_idle: Duration::from_secs(60),
            udp_queue: 256,
            max_udp_targets: 1_024,
            echo_timeout: Duration::from_secs(5),
            max_echoes: 1_024,
        }
    }
}

impl TunOptions {
    /// The longest `keep_alive`, `timeout`, `udp_idle` and `echo_timeout`
    /// there can be. The stack adds the first two to a clock that counts in
    /// microseconds in an `i64`, and the others are added to times as well: a
    /// day is far below where that overflows, and far above any setting that
    /// is meant.
    const LONGEST: Duration = Duration::from_secs(24 * 60 * 60);

    /// Whether the options can run a TUN: the first that cannot, in the order
    /// of the fields. [`Tun::start`] panics for options that do not pass, so
    /// whoever starts a TUN calls this first and tells whoever gave the
    /// options what is wrong with them. `budget`, `max_sockets` and
    /// `max_half_open` may be 0 (every socket on the smallest buffers, every
    /// SYN answered with a reset, no connection let through its handshake);
    /// `drop_hold`, `half_open_timeout` and `linger` are taken whatever they
    /// are.
    pub fn check(&self) -> Result<(), String> {
        if self.shards == 0 {
            return Err("shards must be more than zero".into());
        }
        if self.buffer_min == 0 {
            return Err("buffer_min must be more than zero".into());
        }
        if self.tx_max == 0 {
            return Err("tx_max must be more than zero".into());
        }
        if self.keep_alive < Duration::from_millis(1) || self.keep_alive > Self::LONGEST {
            return Err("keep_alive must be at least 1 ms and at most 24 hours".into());
        }
        if self.timeout <= self.keep_alive || self.timeout > Self::LONGEST {
            return Err("timeout must be more than keep_alive and at most 24 hours".into());
        }
        if self.shard_queue == 0 {
            return Err("shard_queue must be more than zero".into());
        }
        if self.writer_queue == 0 {
            return Err("writer_queue must be more than zero".into());
        }
        if self.udp_idle < Duration::from_millis(1) || self.udp_idle > Self::LONGEST {
            return Err("udp_idle must be at least 1 ms and at most 24 hours".into());
        }
        if self.udp_queue == 0 {
            return Err("udp_queue must be more than zero".into());
        }
        if self.max_udp_targets == 0 {
            return Err("max_udp_targets must be more than zero".into());
        }
        if self.echo_timeout < Duration::from_millis(1) || self.echo_timeout > Self::LONGEST {
            return Err("echo_timeout must be at least 1 ms and at most 24 hours".into());
        }
        if self.max_echoes == 0 {
            return Err("max_echoes must be more than zero".into());
        }
        Ok(())
    }
}

/// Whether the TUN is ending, and why: the first reason given stays.
#[derive(Default)]
pub(crate) struct Life {
    stop: CancellationToken,
    ending: Mutex<Option<Ending>>,
}

impl Life {
    /// Ends the TUN with `ending`, unless it is ending already: the first
    /// ending stays. Whether this call gave it.
    pub(crate) fn end(&self, ending: Ending) -> bool {
        let first = {
            let mut slot = self.ending.lock().expect("the TUN's ending");
            let first = slot.is_none();
            slot.get_or_insert(ending);
            first
        };
        self.stop.cancel();
        first
    }

    /// The device failed, reading or writing: the TUN ends with the failure,
    /// unless it was ending already (after a stop, say), when that stays the
    /// ending. Only the call that gives the ending says so, so that it is said
    /// once for the TUN, though the reader and the writer may both meet the
    /// failure. The error's text is the device's own and has no address in it.
    pub(crate) fn device_failed(&self, error: &std::io::Error) {
        if self.end(Ending::DeviceFailed(error.to_string())) {
            tracing::warn!(%error, "tun: the device failed; the TUN has ended");
        }
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
    pool: Option<Arc<FakeIpPool>>,
    #[cfg(test)]
    shards: Vec<Arc<Shard>>,
    #[cfg(test)]
    budget: Arc<Budget>,
    #[cfg(test)]
    admission: Arc<Admission>,
}

/// A running TUN. Clone it to share it. It has no `Drop`: dropping every handle
/// does not stop the TUN, and nothing is left that could. Call
/// [`TunHandle::stop`], and [`TunHandle::join`] to wait for the end.
#[derive(Clone)]
#[must_use = "dropping every handle leaves the TUN running with nothing that can stop it"]
pub struct TunHandle {
    inner: Arc<Inner>,
}

impl TunHandle {
    /// Stops the TUN, which takes a moment to end (see [`TunHandle::join`]). A new
    /// SYN is answered with a reset from now on. The device is not read any more.
    /// Each shard takes a last turn: the packets already queued for it go in,
    /// and every connection it has is aborted, so each app is sent a reset,
    /// which the writer writes before it ends (it gives the device a grace for
    /// that, and no more).
    ///
    /// The sessions that have been dialed are cancelled: the token of each
    /// session's handle is cancelled, so its relay ends whatever it is waiting
    /// on, and the session's record is finished as the engine finishes a
    /// cancelled session, as completed. The cancellation is what ends a relay
    /// that waits on its upstream in both directions (writing to one that
    /// takes nothing, reading from one that says nothing), which touches its
    /// stream in neither and would never meet the reset the stop gives the
    /// connection. A REJECT-DROP hold ends at the stop as well. So does every UDP
    /// session: its association is told to end (the token it was given is
    /// cancelled) and is waited for, so [`TunHandle::join`] waits for each
    /// association to return, as the engine's does at once.
    ///
    /// A dial that is under way is not cancelled: it is the dialer's, which
    /// holds the session's handle, and a dial dropped half way would leave the
    /// session's record open. When it comes back the session is cancelled like
    /// the others, with a stream that has been reset, and ends at once. So
    /// [`TunHandle::join`] waits for a dial as long as it takes: the engine's
    /// dials end within their own time limits, and whoever has to stop a TUN
    /// in a bounded time gives its wait for `join` a limit, which is all there
    /// is against a dial that never comes back.
    pub fn stop(&self) {
        self.inner.life.end(Ending::Stopped);
    }

    /// Every task of the TUN, sessions included, has ended: why it ended, the
    /// first reason given. Answers the same every time. The sessions end as
    /// [`TunHandle::stop`] says, so what keeps this waiting after a stop is a
    /// dial that is under way, until it comes back.
    pub async fn join(&self) -> Ending {
        self.inner.tasks.wait().await;
        self.inner.life.ending()
    }

    /// The packets dropped so far, for each reason.
    pub fn counters(&self) -> Counters {
        self.inner.stats.counters()
    }

    /// The pool of fake addresses the TUN was started with, if it had one.
    pub fn pool(&self) -> Option<Arc<FakeIpPool>> {
        self.inner.pool.clone()
    }

    /// The sessions that are running: the TUN's tasks but the ones it starts
    /// itself, a shard's each, the writer and the device's reader. It is right
    /// only while those run, until a stop or a failing device ends them; after
    /// that it means nothing, and it may panic.
    #[cfg(test)]
    pub(crate) fn session_tasks(&self) -> usize {
        self.inner
            .tasks
            .len()
            .checked_sub(self.inner.shards.len() + 2)
            .expect("session_tasks is asked of a TUN that is running")
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

/// Why a TUN ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ending {
    /// `TunHandle::stop` was called.
    Stopped,
    /// The device failed, reading or writing, with an error of this text. The
    /// TUN ends as it does for a stop, but for a write that failed being the
    /// last the writer makes. Said once, at WARN, when it happens.
    DeviceFailed(String),
}

/// What the TUN has counted of the packets it dropped, one count for each
/// reason (the logs call a reason by the name of its field), and of the panics
/// of the TCP stack.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Packets that cannot be read: a version other than 4 and 6, lengths that do not fit, an IPv4 header checksum that does not hold, a transport header that is cut short or has port 0, the checksum of a UDP datagram or of an ICMP echo request that does not hold (or that a UDP datagram over IPv6 does not have).
    pub malformed: u64,
    /// Fragments, which the TUN does not reassemble.
    pub fragment: u64,
    /// Packets with an address that no real packet through the adapter has: an unspecified, loopback or multicast address at either end, the limited broadcast as the destination, an IPv4-mapped IPv6 address. Counted, never logged.
    pub martian: u64,
    /// Packets nothing handles: IPv6 while it is off, extension headers, protocols other than TCP, UDP and ICMP, an ICMP that is no echo request, and ICMP echo requests, which nothing handles yet; UDP datagrams when the dialer carries no UDP; replies from an address of the other family than the app's.
    pub unsupported: u64,
    /// Packets for a shard whose queue was full; datagrams for a UDP session whose queue was full; ICMP messages for refused UDP flows that the writer's queue had no room for.
    pub queue_full: u64,
    /// SYNs and UDP datagrams to an address of rurge's own space that names nothing; replies from a name that has no address.
    pub unknown_fake: u64,
    /// SYNs turned away because of the limits on sockets and on half-open connections; UDP datagrams turned away because the dialer had no association to give, or because the session already had its most destinations.
    pub busy: u64,
    /// Segments with data past the window the TUN last told of, dropped because the receive buffer had room.
    pub beyond_window: u64,
    /// Resets not sent because of the limit on resets.
    pub rst_limited: u64,
    /// ICMP messages for refused UDP flows that the limit on ICMP held back (100 in 3 seconds).
    pub icmp_limited: u64,
    /// Times the TCP stack of a shard panicked and the shard was started over, its connections reset.
    pub stack_panics: u64,
}

#[cfg(test)]
mod tests;
