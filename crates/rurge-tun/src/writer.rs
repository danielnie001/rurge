//! The task that writes to the device (phase 3 M3 design 4.5): what every
//! shard's stack sent comes to it in batches through one bounded queue, and
//! goes to the device in the order it came.
//!
//! The stop does not end it at once. After the stop the shards' last turns hand
//! it a reset for every connection and for each SYN still queued, and the apps
//! are told that their connections are over only if it writes them. It writes
//! what it is handed until every shard has ended and its queue is closed; but
//! the device is given [`STOP_GRACE`] from the stop and no longer. A device that
//! takes nothing more would otherwise keep the writer in a write, every shard
//! that waits for room in the writer's queue, and so the end of the TUN, for
//! ever. A write that fails is the last: nothing is written after it.

use crate::Life;
use crate::device::Device;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// How long the writer goes on writing once the TUN is stopping: from the
/// moment it sees the stop to the moment it gives up what is still queued and
/// what is in a write.
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(1);

pub(crate) async fn run(
    device: Arc<dyn Device>,
    mut batches: mpsc::Receiver<Vec<Vec<u8>>>,
    limit: usize,
    life: Arc<Life>,
) {
    let mut taken = Vec::with_capacity(limit);
    let mut packets = Vec::new();
    // the time the writer has is counted from the moment it sees the stop
    let patience = async {
        life.stopped().await;
        tokio::time::sleep(STOP_GRACE).await;
    };
    tokio::pin!(patience);
    loop {
        let received = tokio::select! {
            biased;
            _ = &mut patience => return,
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
            _ = &mut patience => return,
            sent = device.send(&mut packets) => sent,
        };
        if let Err(error) = sent {
            life.device_failed(&error);
            return;
        }
        packets.clear();
    }
}
