//! The task that reads the device (phase 3 M3 design 4.3): each packet goes
//! to where it is handled, or is dropped and counted. It never waits for a
//! queue: a queue that is full drops the packet.

use crate::device::Device;
use crate::packet::{self, Reason, Transport};
use crate::stats::Stats;
use crate::tcp::Tuple;
use crate::{Ending, Life};
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
    ipv6: bool,
    stats: Arc<Stats>,
    life: Arc<Life>,
) {
    let mut batch = Vec::with_capacity(RECV_BATCH);
    loop {
        let received = tokio::select! {
            biased;
            _ = life.stopped() => return,
            received = device.recv(&mut batch, RECV_BATCH) => received,
        };
        if let Err(error) = received {
            life.end(Ending::DeviceFailed(error.to_string()));
            return;
        }
        for packet in batch.drain(..) {
            route(packet, ipv6, &shards, &stats);
        }
    }
}

/// Sends `packet` where it is handled once it has passed the checks of
/// [`packet::classify`], or drops it and says why. What follows the IP packet
/// is cut off, so a shard is given the packet and no more.
pub(crate) fn route(
    mut packet: Vec<u8>,
    ipv6: bool,
    shards: &[mpsc::Sender<Vec<u8>>],
    stats: &Stats,
) {
    let parsed = match packet::classify(&packet, ipv6) {
        Ok(parsed) => parsed,
        Err(reason) => {
            stats.drop(reason, None, None);
            return;
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
        }
        // nothing handles UDP or ICMP yet
        Transport::Udp { .. } | Transport::Icmp { .. } => {
            stats.drop(Reason::Unsupported, Some(parsed.src), Some(parsed.dst));
        }
    }
}

/// Every segment of a connection goes to the same shard.
fn shard_of(tuple: &Tuple, shards: usize) -> usize {
    let mut hasher = DefaultHasher::new();
    tuple.hash(&mut hasher);
    (hasher.finish() % shards as u64) as usize
}
