//! The task that reads the device (phase 3 M3 design 4.3): each packet goes
//! to where it is handled, or is dropped and counted. It never waits for a
//! queue: a queue that is full drops the packet, and everything it hands on
//! to a shard, a session, the DNS responder or the writer is handed on without
//! waiting. What waits for a shard holds the bytes it takes up out of the
//! budget of those queues (`queue_bytes`), which it gives back when the shard
//! has it (`Ingress`).

use crate::bytes::{QueuedBytes, Reserved};
use crate::context::Context;
use crate::device::Device;
use crate::icmp;
use crate::packet::{self, Classified, Hit, Parsed, Reason, Transport};
use crate::rewrite;
use crate::stats::Stats;
use crate::tcp::Tuple;
use crate::udp;
use rurge_config::IpRewrite;
use std::hash::{BuildHasher, RandomState};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

/// The most packets taken from the device at a time.
pub(crate) const RECV_BATCH: usize = 256;

pub(crate) async fn run(
    device: Arc<dyn Device>,
    shards: Vec<mpsc::Sender<Ingress>>,
    ctx: Arc<Context>,
    queue_bytes: Arc<QueuedBytes>,
    shard_key: RandomState,
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
        // the table of the generation in use is read once for the batch, so
        // every packet of it is handled as the same profile asked
        let settings = ctx.hooks.settings();
        for packet in batch.drain(..) {
            let Some(routed) = route(
                packet,
                ctx.options.ipv6,
                &settings.ip_rewrite,
                &shards,
                &ctx.stats,
                &queue_bytes,
                &shard_key,
            ) else {
                continue;
            };
            match routed {
                Routed::Local(parsed, packet) => match parsed.transport {
                    Transport::Udp { .. } => udp::receive(&ctx, &parsed, &packet),
                    Transport::Icmp { .. } => icmp::receive(&ctx, &parsed, &packet),
                    // `route` gives a TCP segment to its shard and hands none
                    // back. Debug builds assert, so that a test notices;
                    // release builds do nothing, for a panic here would stop
                    // all reception.
                    Transport::Tcp(_) => {
                        debug_assert!(false, "route gives a TCP segment to a shard")
                    }
                },
                Routed::Rewrite(hit, packet) => rewrite::handle(&ctx, &hit, packet),
            }
        }
    }
}

/// A packet on its way to a shard, with what it holds of the queues' bytes
/// (`queue_bytes`) while it waits: it is dropped, and gives them back, once the
/// shard has taken the packet into its stack.
pub(crate) struct Ingress {
    pub(crate) packet: Vec<u8>,
    pub(crate) _held: Reserved,
}

impl Ingress {
    /// A packet that holds none of the budget: for a test that hands a shard
    /// its packets itself.
    #[cfg(test)]
    pub(crate) fn unheld(packet: Vec<u8>) -> Ingress {
        Ingress {
            packet,
            _held: Reserved::none(),
        }
    }
}

/// Where a packet the device gave goes.
pub(crate) enum Routed {
    /// A UDP datagram or an echo request, with what [`packet::classify`] read
    /// of it and the packet cut to its length: for a session or the TUN's own
    /// echo requests.
    Local(Parsed, Vec<u8>),
    /// A packet `[IP Rewrite]` matched, with what the table asked of it and the
    /// packet cut to its length: for [`rewrite::handle`], and no stack.
    Rewrite(Hit, Vec<u8>),
}

/// Sends `packet` where it is handled once it has passed the checks of
/// [`packet::classify`], or drops it and says why. What follows the IP packet
/// is cut off, so a shard is given the packet and no more. A packet `[IP
/// Rewrite]` matched comes back whole for its action, cut to its length. A TCP
/// segment goes to its shard, holding the bytes it takes up out of
/// `queue_bytes` until the shard has it; a segment that those bytes have no
/// room for, or that finds the shard's queue full, is counted `queue_full`. A
/// UDP datagram and an echo request, which are not for a shard, are given back
/// with what `classify` read of them (and nothing is counted for them here).
pub(crate) fn route(
    mut packet: Vec<u8>,
    ipv6: bool,
    rewrite: &IpRewrite,
    shards: &[mpsc::Sender<Ingress>],
    stats: &Stats,
    queue_bytes: &Arc<QueuedBytes>,
    shard_key: &RandomState,
) -> Option<Routed> {
    let classified = match packet::classify(&packet, ipv6, rewrite) {
        Ok(classified) => classified,
        Err(dropped) => {
            let (src, dst) = dropped.ends.unzip();
            stats.drop(dropped.reason, src, dst);
            return None;
        }
    };
    let parsed = match classified {
        Classified::Passed(parsed) => parsed,
        Classified::Rewrite(hit) => {
            packet.truncate(hit.len);
            return Some(Routed::Rewrite(hit, packet));
        }
    };
    packet.truncate(parsed.len);
    match parsed.transport {
        Transport::Tcp(segment) => {
            // the bytes the packet takes up, counted before it is given to the
            // shard, so that the queues in all cannot hold more than the budget
            let Some(held) = queue_bytes.reserve(packet.capacity()) else {
                stats.drop(Reason::QueueFull, Some(parsed.src), Some(parsed.dst));
                return None;
            };
            let ingress = Ingress {
                packet,
                _held: held,
            };
            match shards[shard_of(&segment.tuple, shards.len(), shard_key)].try_send(ingress) {
                Ok(()) => {}
                Err(TrySendError::Full(ingress)) => {
                    // the bytes go back with the packet
                    drop(ingress);
                    stats.drop(Reason::QueueFull, Some(parsed.src), Some(parsed.dst));
                }
                // the shard has ended, and so has the TUN
                Err(TrySendError::Closed(_)) => {}
            }
            None
        }
        Transport::Udp { .. } | Transport::Icmp { .. } => Some(Routed::Local(parsed, packet)),
    }
}

/// Every segment of a connection goes to the same shard. `key` is the TUN's
/// own (`std::hash::RandomState::new`, one for each TUN): two TUNs do not hand
/// the same connections to the same shards, and a connection keeps its shard
/// for as long as the TUN runs.
pub(crate) fn shard_of(tuple: &Tuple, shards: usize, key: &RandomState) -> usize {
    (key.hash_one(tuple) % shards as u64) as usize
}
