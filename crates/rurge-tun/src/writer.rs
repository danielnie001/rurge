//! The task that writes to the device (phase 3 M3 design 4.5): what every
//! shard's stack sent comes to it in batches through one bounded queue, and
//! goes to the device in the order it came.

use crate::device::Device;
use crate::{Ending, Life};
use std::sync::Arc;
use tokio::sync::mpsc;

pub(crate) async fn run(
    device: Arc<dyn Device>,
    mut batches: mpsc::Receiver<Vec<Vec<u8>>>,
    limit: usize,
    life: Arc<Life>,
) {
    let mut taken = Vec::with_capacity(limit);
    let mut packets = Vec::new();
    loop {
        let received = tokio::select! {
            biased;
            _ = life.stopped() => return,
            received = batches.recv_many(&mut taken, limit) => received,
        };
        // every shard has ended
        if received == 0 {
            return;
        }
        for batch in taken.drain(..) {
            packets.extend(batch);
        }
        let sent = tokio::select! {
            biased;
            _ = life.stopped() => return,
            sent = device.send(&mut packets) => sent,
        };
        if let Err(error) = sent {
            life.end(Ending::DeviceFailed(error.to_string()));
            return;
        }
        packets.clear();
    }
}
