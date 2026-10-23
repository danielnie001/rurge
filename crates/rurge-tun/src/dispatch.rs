//! The task that reads the device (phase 3 M3 design 4.3): each packet goes
//! to where it is handled, or is dropped and counted. It never waits for a
//! queue: a queue that is full drops the packet, and everything it hands on
//! to a shard, a session, the DNS responder or the writer is handed on without
//! waiting.

use crate::context::Context;
use crate::device::Device;
use crate::icmp;
use crate::packet::{self, Parsed, Reason, Transport};
use crate::stats::Stats;
use crate::tcp::Tuple;
use crate::udp;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

/// The most packets taken from the device at a time.
pub(crate) const RECV_BATCH: usize = 256;

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
