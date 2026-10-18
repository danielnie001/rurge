//! The task that reads the device (phase 3 M3 design 4.3): each packet goes
//! to where it is handled, or is dropped and counted. It never waits for a
//! queue: a queue that is full drops the packet.

use crate::device::Device;
use crate::packet::{self, Kind};
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
            route(packet, &shards, &stats);
        }
    }
}

fn route(packet: Vec<u8>, shards: &[mpsc::Sender<Vec<u8>>], stats: &Stats) {
    match packet::kind(&packet) {
        Kind::Malformed => Stats::count(&stats.malformed),
        Kind::Other => Stats::count(&stats.unsupported),
        Kind::Tcp(segment) => match shards[shard_of(&segment.tuple, shards.len())].try_send(packet)
        {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => Stats::count(&stats.queue_full),
            // the shard has ended, and so has the TUN
            Err(TrySendError::Closed(_)) => {}
        },
    }
}

/// Every segment of a connection goes to the same shard.
fn shard_of(tuple: &Tuple, shards: usize) -> usize {
    let mut hasher = DefaultHasher::new();
    tuple.hash(&mut hasher);
    (hasher.finish() % shards as u64) as usize
}
