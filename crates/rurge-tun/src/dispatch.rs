//! The task that reads the device (phase 3 M3 design 4.3): each packet goes
//! to where it is handled, or is dropped and counted. It never waits for a
//! queue: a queue that is full drops the packet, and everything it hands on
//! to a session or to the writer is handed on without waiting.

use crate::device::Device;
use crate::fragment::Ids;
use crate::icmp;
use crate::limit::Limiter;
use crate::packet::{self, Parsed, Reason, Transport};
use crate::stats::Stats;
use crate::tcp::Tuple;
use crate::udp::{self, Sessions};
use crate::{Hooks, Life, Pinger, TunOptions};
use rurge_dns::fakeip::FakeIpPool;
use rurge_inbound::Dialer;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_util::task::TaskTracker;

/// The most packets taken from the device at a time.
pub(crate) const RECV_BATCH: usize = 256;

/// What the receive task hands the datagrams and the echo requests it reads on
/// with, and what the sessions it starts live by: the TUN's own state besides
/// the TCP shards. The task, each UDP session's task and each session's client
/// hold it.
pub(crate) struct Context {
    /// Where the UDP sessions go.
    pub(crate) dialer: Arc<dyn Dialer>,
    pub(crate) pool: Option<Arc<FakeIpPool>>,
    /// Where the settings of the generation in use come from, asked for each
    /// echo request.
    pub(crate) hooks: Arc<dyn Hooks>,
    /// Sends the echo requests that go out.
    pub(crate) pinger: Arc<dyn Pinger>,
    /// The writer's queue, for what the TUN makes itself: replies to the apps'
    /// datagrams and to their echo requests, and ICMP messages.
    pub(crate) writer: mpsc::Sender<Vec<Vec<u8>>>,
    pub(crate) stats: Arc<Stats>,
    pub(crate) life: Arc<Life>,
    pub(crate) options: TunOptions,
    /// The TUN's tasks, to which the sessions are added.
    pub(crate) tasks: TaskTracker,
    /// The device's MTU: the longest IP packet the TUN writes.
    pub(crate) mtu: usize,
    /// The UDP sessions, by the app endpoint each is for.
    pub(crate) sessions: Sessions,
    /// The identifications of the fragments the TUN writes.
    pub(crate) ids: Ids,
    /// The limit on the ICMP messages for the UDP flows that were refused.
    pub(crate) icmp_limit: Mutex<Limiter>,
    /// How many echo requests wait for their replies: the places taken out of
    /// `max_echoes` (see `icmp`).
    pub(crate) echoes: AtomicUsize,
}

pub(crate) async fn run(
    device: Arc<dyn Device>,
    shards: Vec<mpsc::Sender<Vec<u8>>>,
    ctx: Arc<Context>,
) {
    let mut batch = Vec::with_capacity(RECV_BATCH);
    loop {
        let received = tokio::select! {
            biased;
            _ = ctx.life.stopped() => return,
            received = device.recv(&mut batch, RECV_BATCH) => received,
        };
        if let Err(error) = received {
            ctx.life.device_failed(&error);
            return;
        }
        for packet in batch.drain(..) {
            let Some((parsed, packet)) = route(packet, ctx.options.ipv6, &shards, &ctx.stats)
            else {
                continue;
            };
            match parsed.transport {
                Transport::Udp { .. } => udp::receive(&ctx, &parsed, &packet),
                Transport::Icmp { .. } => icmp::receive(&ctx, &parsed, &packet),
                // `route` gives a TCP segment to its shard and hands none back.
                // Debug builds assert, so that a test notices; release builds do
                // nothing, for a panic here would stop all reception.
                Transport::Tcp(_) => debug_assert!(false, "route gives a TCP segment to a shard"),
            }
        }
    }
}

/// Sends `packet` where it is handled once it has passed the checks of
/// [`packet::classify`], or drops it and says why. What follows the IP packet
/// is cut off, so a shard is given the packet and no more. A TCP segment goes
/// to its shard; a UDP datagram and an echo request, which are not for a shard,
/// are given back with what `classify` read of them (and nothing is counted
/// for them here).
pub(crate) fn route(
    mut packet: Vec<u8>,
    ipv6: bool,
    shards: &[mpsc::Sender<Vec<u8>>],
    stats: &Stats,
) -> Option<(Parsed, Vec<u8>)> {
    let parsed = match packet::classify(&packet, ipv6) {
        Ok(parsed) => parsed,
        Err(dropped) => {
            let (src, dst) = dropped.ends.unzip();
            stats.drop(dropped.reason, src, dst);
            return None;
        }
    };
    packet.truncate(parsed.len);
    match parsed.transport {
        Transport::Tcp(segment) => {
            match shards[shard_of(&segment.tuple, shards.len())].try_send(packet) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    stats.drop(Reason::QueueFull, Some(parsed.src), Some(parsed.dst));
                }
                // the shard has ended, and so has the TUN
                Err(TrySendError::Closed(_)) => {}
            }
            None
        }
        Transport::Udp { .. } | Transport::Icmp { .. } => Some((parsed, packet)),
    }
}

/// Every segment of a connection goes to the same shard.
fn shard_of(tuple: &Tuple, shards: usize) -> usize {
    let mut hasher = DefaultHasher::new();
    tuple.hash(&mut hasher);
    (hasher.finish() % shards as u64) as usize
}
