//! The writer's queue: a local answer is dropped and counted when it is full, a
//! forwarded reply waits for room, and a stop ends the wait.

use super::{APP4, FAR4, Lab, MARKER, NOTHING, OWN4, lab_on};
use crate::testing::PingScript;
use crate::testing::packets::IcmpSeen;
use crate::tests::support::{Extras, GatedDevice, WITHIN, eventually, in_five_seconds};
use crate::writer::STOP_GRACE;
use crate::{Counters, Ending, TunOptions};
use std::sync::Arc;
use std::time::Duration;

/// How long after the stop what ends at the stop is to have ended: six tenths
/// of the grace that a writer stuck in its write is given (`STOP_GRACE`), so
/// well within it.
const STOP_WELL_BEFORE: Duration = Duration::from_millis(STOP_GRACE.as_millis() as u64 * 6 / 10);

/// A lab on a device that writes nothing until `open`, and a writer queue of
/// one batch.
fn gated() -> (Lab, Arc<GatedDevice>) {
    let (gate, end) = GatedDevice::pair();
    let options = TunOptions {
        writer_queue: 1,
        ..TunOptions::default()
    };
    let lab = lab_on(gate.clone(), end, options, Extras::default());
    (lab, gate)
}

/// The receive task hands the writer a local answer without waiting: when the
/// queue is full the answer is dropped and counted, and nothing else stops.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_writer_queue_drops_a_local_answer_and_counts_it() {
    let (mut lab, gate) = gated();
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    let senders: u64 = 20;
    for seq in 0..senders {
        lab.ping(APP4, OWN4, 1, seq as u16, b"x");
    }
    // a request that comes after them all, and is answered when the writer
    // takes anything again
    lab.ping(APP4, FAR4, MARKER, 0, b"marker");
    lab.pinged_times(1).await;
    // the writer holds one batch in its write and the queue one more
    let lost = lab.counters().queue_full;
    assert!(
        lost >= senders - 2,
        "{lost} answers were lost, at least {} were to be",
        senders - 2
    );
    gate.open();
    // the writer wrote in order: every answer it took came before the marker
    let mut written = 0;
    loop {
        match lab.reply().await {
            IcmpSeen::EchoReply { ident: MARKER, .. } => break,
            IcmpSeen::EchoReply { src, .. } => {
                assert_eq!(src, OWN4);
                written += 1;
            }
            other => panic!("an echo reply, not {other:?}"),
        }
    }
    assert_eq!(
        written + lost,
        senders,
        "every answer was written or counted"
    );
    assert_eq!(
        lab.counters(),
        Counters {
            queue_full: lost,
            ..Counters::default()
        }
    );
}

/// The task of a request that went out waits for room in the writer's queue to
/// write its reply: nothing is lost, and the tasks wait, however long it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forwarded_reply_waits_for_room_in_the_writer_queue() {
    let (mut lab, gate) = gated();
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    for seq in 0..5 {
        lab.ping(APP4, FAR4, 1, seq, b"x");
    }
    lab.pinged_times(5).await;
    // the writer holds one reply in its write and the queue one more: the
    // other three wait
    eventually("three replies to wait for room", WITHIN, || {
        lab.rig.tun.session_tasks() == 3
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert_eq!(
        lab.rig.tun.session_tasks(),
        3,
        "the replies did not wait for room"
    );
    gate.open();
    let mut seqs = Vec::new();
    for _ in 0..5 {
        match lab.reply().await {
            IcmpSeen::EchoReply { seq, .. } => seqs.push(seq),
            other => panic!("an echo reply, not {other:?}"),
        }
    }
    seqs.sort_unstable();
    assert_eq!(seqs, [0, 1, 2, 3, 4]);
    lab.tasks_end().await;
    assert_eq!(lab.counters(), Counters::default());
}

/// A reply that waits for room in the writer's queue does not wait on past the
/// stop: the writer is stuck in its write, which it gives up `STOP_GRACE` after
/// the stop, and the tasks of the replies are gone long before, so that the
/// writer's is the last task there is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_ends_a_reply_that_waits_for_room() {
    let (lab, _gate) = gated();
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    for seq in 0..5 {
        lab.ping(APP4, FAR4, 1, seq, b"x");
    }
    lab.pinged_times(5).await;
    eventually("three replies to wait for room", WITHIN, || {
        lab.rig.tun.session_tasks() == 3
    })
    .await;
    lab.rig.tun.stop();
    eventually(
        "the writer to be the only task left",
        STOP_WELL_BEFORE,
        || lab.rig.tun.live_tasks() == 1,
    )
    .await;
    let ending = in_five_seconds("the TUN to end", lab.rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
}
