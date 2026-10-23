//! One shard's task, run by hand with channels of the test's own in place of the
//! reader's queue and the writer's: the stop is a last turn of the shard.

use crate::packet::segment;
use crate::session::Holds;
use crate::tcp::shard::{self, Records};
use crate::tcp::{Outcome, PANIC_MARKER, Tuple};
use crate::testing::FakeDialer;
use crate::tests::hazards::{PSH_ACK, app4, open_by_hand};
use crate::tests::support::{
    Lone, WITHIN, addr, data_segment_v4, eventually, in_five_seconds, is_reset, is_syn, lone_shard,
    syn_v4,
};
use crate::{Ending, TunOptions};
use std::net::SocketAddr;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::task::TaskTracker;

/// Runs the task of `lone`'s shard on the current runtime, with the channels
/// the test made in place of the reader's queue and the writer's.
fn spawn_shard(
    lone: &Lone,
    options: &TunOptions,
    packets: mpsc::Receiver<Vec<u8>>,
    to_writer: mpsc::Sender<Vec<Vec<u8>>>,
) -> TaskTracker {
    let tasks = TaskTracker::new();
    tasks.spawn(shard::run(
        lone.shard.clone(),
        packets,
        to_writer,
        FakeDialer::new(),
        Holds::from(options),
        lone.life.clone(),
        tasks.clone(),
    ));
    tasks.close();
    tasks
}

/// What the shard hands the writer until its task ends (the channel closes
/// when the task lets go of its end), in the batches it came in.
async fn handed_over(from_shard: &mut mpsc::Receiver<Vec<Vec<u8>>>) -> Vec<Vec<Vec<u8>>> {
    let mut all = Vec::new();
    while let Some(batch) = in_five_seconds("the shard's task to end", from_shard.recv()).await {
        all.push(batch);
    }
    all
}

/// The reset of rurge's own that answers a SYN of sequence number 1 from `app`
/// to `dst`.
fn assert_reset_for(packet: &[u8], dst: SocketAddr, app: SocketAddr) {
    let read = segment(packet).expect("a TCP segment");
    assert_eq!(read.tuple, Tuple { src: dst, dst: app });
    assert!(read.rst && read.ack && !read.syn && !read.fin, "{read:?}");
    assert_eq!(read.ack_number, 2, "the SYN's sequence number plus one");
}

/// The stop is a last turn of the shard: what is queued for it goes in, and a
/// SYN among it is turned away with a reset, which the writer is handed. All
/// the shard has to go on is the queue: the stop is in place before its task
/// first runs.
#[tokio::test]
async fn the_last_turn_takes_in_what_is_queued() {
    let options = TunOptions::default();
    let lone = lone_shard(&options, Instant::now());
    let (to_shard, packets) = mpsc::channel(16);
    let (to_writer, mut from_shard) = mpsc::channel(16);
    let dst = addr("192.0.2.1:80");
    for port in 40_000..40_003u16 {
        to_shard
            .try_send(syn_v4(app4(port), dst))
            .expect("room in the queue");
    }
    lone.life.end(Ending::Stopped);
    let tasks = spawn_shard(&lone, &options, packets, to_writer);
    let all = handed_over(&mut from_shard).await;
    in_five_seconds("the shard's task", tasks.wait()).await;
    let out: Vec<Vec<u8>> = all.into_iter().flatten().collect();
    assert_eq!(out.len(), 3, "a reset for each SYN");
    for (packet, port) in out.iter().zip(40_000..) {
        assert_reset_for(packet, dst, app4(port));
    }
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.admission.half_open(), 0);
    assert_eq!(lone.budget.used(), 0);
    drop(to_shard);
}

/// A panic in the last turn is caught like one in any turn: the shard is
/// rebuilt, its connections are reset, what they held is given back, and the
/// task ends.
#[tokio::test]
async fn a_panic_in_the_last_turn_rebuilds_the_shard_and_the_task_ends() {
    let options = TunOptions::default();
    let epoch = Instant::now();
    let lone = lone_shard(&options, epoch);
    let (app, dst) = (app4(40_000), addr("192.0.2.1:80"));
    let (opened, _) = open_by_hand(&lone.shard, epoch, app, dst, 0);
    assert_eq!(lone.admission.sockets(), 1);
    let (to_shard, packets) = mpsc::channel(16);
    let (to_writer, mut from_shard) = mpsc::channel(16);
    // a segment of the connection that panics the stack as it is taken in
    to_shard
        .try_send(data_segment_v4(app, dst, 2, 0, PSH_ACK, PANIC_MARKER))
        .expect("room in the queue");
    lone.life.end(Ending::Stopped);
    let tasks = spawn_shard(&lone, &options, packets, to_writer);
    let all = handed_over(&mut from_shard).await;
    in_five_seconds("the shard's task", tasks.wait()).await;
    assert!(all.is_empty(), "the rebuilt shard has nothing to say");
    assert_eq!(lone.stats.counters().stack_panics, 1);
    assert_eq!(opened.shared.outcome(), Some(Outcome::Reset));
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.admission.half_open(), 0);
    assert_eq!(lone.budget.used(), 0);
    assert_eq!(lone.shard.state().records(), Records::default());
    drop(to_shard);
}

/// What the last turn gives the writer waits for room in the writer's queue:
/// the stop does not make the shard give up the send. The queue is full when the
/// stop comes; the shard's task stays until the batch in the queue is taken,
/// and then the connection's reset is handed over.
#[tokio::test]
async fn the_last_turn_waits_for_room_in_the_writers_queue() {
    let options = TunOptions::default();
    let epoch = Instant::now();
    let lone = lone_shard(&options, epoch);
    let (app, dst) = (app4(40_000), addr("192.0.2.1:80"));
    open_by_hand(&lone.shard, epoch, app, dst, 0);
    let (_to_shard, packets) = mpsc::channel(16);
    let (to_writer, mut from_shard) = mpsc::channel(1);
    to_writer
        .try_send(vec![vec![0u8]])
        .expect("room for one batch");
    let tasks = spawn_shard(&lone, &options, packets, to_writer);
    lone.life.end(Ending::Stopped);
    // the current-thread runtime runs the task to the send, which waits
    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
    assert_eq!(tasks.len(), 1, "the task waits for room");
    let batch = in_five_seconds("the batch in the queue", from_shard.recv())
        .await
        .expect("the batch the test put there");
    assert_eq!(batch, vec![vec![0u8]]);
    let rest = handed_over(&mut from_shard).await;
    in_five_seconds("the shard's task", tasks.wait()).await;
    let out: Vec<&Vec<u8>> = rest.iter().flatten().collect();
    assert_eq!(out.len(), 1, "one connection, one reset");
    assert!(is_reset(out[0]), "the connection's reset is handed over");
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.admission.sockets(), 0);
}

/// The same for a send the stop finds in progress: what the shard was handing
/// the writer when the stop came is handed over all the same, and the resets
/// of the last turn follow it. A reset the shard made for a SYN in a turn that
/// began before the stop and ended after it would be lost with the send
/// otherwise.
#[tokio::test]
async fn a_send_the_stop_finds_in_progress_is_not_given_up() {
    let options = TunOptions::default();
    let lone = lone_shard(&options, Instant::now());
    let (app, dst) = (app4(40_000), addr("192.0.2.1:80"));
    let (to_shard, packets) = mpsc::channel(16);
    let (to_writer, mut from_shard) = mpsc::channel(1);
    to_writer
        .try_send(vec![vec![0u8]])
        .expect("room for one batch");
    let tasks = spawn_shard(&lone, &options, packets, to_writer);
    // the SYN is taken, and its SYN-ACK waits for room in the queue
    to_shard
        .try_send(syn_v4(app, dst))
        .expect("room in the queue");
    eventually("the SYN to be taken", WITHIN, || {
        lone.admission.sockets() == 1
    })
    .await;
    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
    lone.life.end(Ending::Stopped);
    let all = handed_over(&mut from_shard).await;
    in_five_seconds("the shard's task", tasks.wait()).await;
    assert_eq!(all.len(), 3, "the batch put there, the SYN-ACK, the resets");
    assert_eq!(all[0], vec![vec![0u8]]);
    assert!(is_syn(&all[1][0]), "the SYN-ACK that was being handed over");
    assert_eq!(all[2].len(), 1);
    assert!(is_reset(&all[2][0]), "the reset of the last turn");
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.admission.half_open(), 0);
    assert_eq!(lone.budget.used(), 0);
    drop(to_shard);
}
