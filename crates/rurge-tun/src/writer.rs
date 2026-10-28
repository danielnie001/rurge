//! The task that writes to the device (phase 3 M3 design 4.5, phase 3 M5 design
//! 7.2): what the TUN makes, and what every shard's stack sent, comes to it in
//! batches through two bounded queues, and goes to the device in the order it
//! came — the priority queue's batches first.
//!
//! The priority queue is for the UDP datagrams the TUN writes back, a session's
//! replies and the responder's answers, when `udp-priority` is on; everything
//! else waits in the normal queue: the TCP segments and resets, the ICMP
//! messages, the echo replies and what `[IP Rewrite]` sends back. Which queue a
//! batch goes to is the sender's to choose (see `Context::send`), so a slow
//! device cannot keep a UDP reply behind a few hundred batches of TCP.
//!
//! Each round of the writer waits until either open queue has something, takes
//! from both without waiting for the other (at most `limit` batches each), and
//! gives the device the priority queue's packets first. So neither queue starves
//! and a UDP reply waits at most for the write in hand.
//!
//! The stop does not end it at once. After the stop the shards' last turns hand
//! it a reset for every connection and for each SYN still queued, and the apps
//! are told that their connections are over only if it writes them. It writes
//! what it is handed until both queues have closed — every shard has ended and
//! the context has let go of them — but the device is given [`STOP_GRACE`] from
//! the stop and no longer. A device that takes nothing more would otherwise keep
//! the writer in a write, every shard that waits for room in the writer's queues,
//! and so the end of the TUN, for ever. A queue that has closed is not waited on
//! again, so that the writer does not spin on it, and a write that fails is the
//! last: nothing is written after it.

use crate::Life;
use crate::device::Device;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// How long the writer goes on writing once the TUN is stopping: from the
/// moment it sees the stop to the moment it gives up what is still queued and
/// what is in a write.
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(1);

/// Which of the writer's two queues a batch is for (design 7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    /// UDP datagrams written back: a session's replies (their fragments
    /// too), the responder's answers over UDP.
    Udp,
    /// Everything else: TCP segments and resets, ICMP messages, echo
    /// replies, what `[IP Rewrite]` sends back.
    Other,
}

/// The sending ends of the writer's two queues.
#[derive(Clone)]
pub(crate) struct Outlet {
    pub(crate) priority: mpsc::Sender<Vec<Vec<u8>>>,
    pub(crate) normal: mpsc::Sender<Vec<Vec<u8>>>,
}

/// Which of the writer's two queues a batch was taken from. Not the class it is
/// for: a batch for UDP waits in the normal queue while `udp-priority` is off.
#[derive(Clone, Copy)]
enum Queue {
    Priority,
    Normal,
}

pub(crate) async fn run(
    device: Arc<dyn Device>,
    mut priority: mpsc::Receiver<Vec<Vec<u8>>>,
    mut normal: mpsc::Receiver<Vec<Vec<u8>>>,
    limit: usize,
    life: Arc<Life>,
) {
    let mut from_priority = Vec::with_capacity(limit);
    let mut from_normal = Vec::with_capacity(limit);
    let mut packets = Vec::new();
    // a queue whose senders have all gone and which is empty: it is not waited
    // on again, and both being closed is the end
    let mut priority_open = true;
    let mut normal_open = true;
    // the time the writer has is counted from the moment it sees the stop
    let patience = async {
        life.stopped().await;
        tokio::time::sleep(STOP_GRACE).await;
    };
    tokio::pin!(patience);
    loop {
        // whichever open queue has something first; `recv_many` is cancel safe,
        // so the branch that is not taken loses nothing
        let (from, received) = tokio::select! {
            biased;
            _ = &mut patience => return,
            received = priority.recv_many(&mut from_priority, limit), if priority_open => {
                (Queue::Priority, received)
            }
            received = normal.recv_many(&mut from_normal, limit), if normal_open => {
                (Queue::Normal, received)
            }
        };
        if received == 0 {
            // every sender of this queue has gone, and it is empty
            match from {
                Queue::Priority => priority_open = false,
                Queue::Normal => normal_open = false,
            }
            if !priority_open && !normal_open {
                return;
            }
            continue;
        }
        // the other queue, if it is open, is taken from without waiting: a
        // round is one write, and the priority queue's packets go first in it
        match from {
            Queue::Priority => {
                if normal_open {
                    normal_open = take_now(&mut normal, &mut from_normal, limit);
                }
            }
            Queue::Normal => {
                if priority_open {
                    priority_open = take_now(&mut priority, &mut from_priority, limit);
                }
            }
        }
        for batch in from_priority.drain(..) {
            packets.extend(batch);
        }
        for batch in from_normal.drain(..) {
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

/// Takes up to `limit` batches from `queue` into `into` without waiting for
/// them: `true` while the queue is open, `false` once its senders have all gone
/// and it is empty.
fn take_now(
    queue: &mut mpsc::Receiver<Vec<Vec<u8>>>,
    into: &mut Vec<Vec<Vec<u8>>>,
    limit: usize,
) -> bool {
    while into.len() < limit {
        match queue.try_recv() {
            Ok(batch) => into.push(batch),
            Err(mpsc::error::TryRecvError::Empty) => return true,
            Err(mpsc::error::TryRecvError::Disconnected) => return false,
        }
    }
    true
}
