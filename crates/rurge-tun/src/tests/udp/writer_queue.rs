//! What the TUN does when the writer's queue is full: a message it makes without
//! waiting is dropped and counted, a reply waits for room, and the wait ends at
//! the stop.

use super::{APP4, FAR4, Lab, NOTHING, taking, target_of};
use crate::testing::UdpScript;
use crate::testing::packets;
use crate::tests::support::{GatedDevice, WITHIN, addr, eventually, in_five_seconds, rig_with};
use crate::writer::STOP_GRACE;
use crate::{Counters, Ending, TunOptions};
use rurge_proto::RejectKind;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A rig on a device that writes nothing until `open`, and a writer queue of
/// one batch.
fn gated() -> (Lab, Arc<GatedDevice>) {
    let (gate, end) = GatedDevice::pair();
    let rig = rig_with(
        gate.clone(),
        end,
        TunOptions {
            writer_queue: 1,
            ..TunOptions::default()
        },
    );
    (taking(rig), gate)
}

/// The receive task, and the dialer's client when it refuses a flow, hand the
/// writer an ICMP message without waiting: when its queue is full the message is
/// dropped and counted, and nothing else stops.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_writer_queue_drops_the_icmp_and_counts_it() {
    for from_the_client in [false, true] {
        let (mut lab, gate) = gated();
        let (app, marker) = (addr(APP4), addr(FAR4));
        lab.dialer()
            .script_udp(target_of(marker), UdpScript::Reply(b"marker".to_vec()));
        let refused = addr("192.0.2.11:9");
        lab.dialer()
            .script_udp(target_of(refused), UdpScript::Refuse(RejectKind::NoDrop));
        let senders = 20;
        for _ in 0..senders {
            let dst = if from_the_client {
                refused
            } else {
                addr("198.18.0.244:9")
            };
            lab.send(app, dst, b"x");
        }
        // a datagram that comes after them all, and is answered when the
        // writer takes anything again
        lab.send(app, marker, b"marker");
        eventually("the marker to be taken", WITHIN, || {
            lab.dialer()
                .udp_sent()
                .iter()
                .any(|(to, _)| to == &target_of(marker))
        })
        .await;
        // the writer holds one batch in its write and the queue one more
        let lost = lab.counters().queue_full;
        assert!(
            lost >= senders - 2,
            "{lost} messages were lost, at least {} were to be (the client: {from_the_client})",
            senders - 2
        );
        gate.open();
        assert_eq!(lab.datagram().await.payload, b"marker");
        // the writer wrote in order: every message it took came before the marker
        let mut messages = 0;
        while let Ok(packet) = lab.icmp.try_recv() {
            packets::read_icmp(&packet).expect("an ICMP message");
            messages += 1;
        }
        assert_eq!(
            messages + lost,
            senders,
            "every message was written or counted (the client: {from_the_client})"
        );
        assert_eq!(
            lab.counters(),
            Counters {
                queue_full: lost,
                ..Counters::default()
            }
        );
    }
}

/// The client waits for room in the writer's queue to send a reply: nothing
/// is lost, and the engine's task that gave it the reply waits (that is the
/// back pressure on the upstream).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_waits_for_room_in_the_writer_queue() {
    let (mut lab, gate) = gated();
    let (app, far) = (addr(APP4), addr(FAR4));
    lab.send(app, far, b"hello");
    lab.associations_open(1).await;
    let dialer = lab.rig.dialer.clone();
    let from = target_of(far);
    let sending = tokio::spawn(async move {
        for number in 0..5u8 {
            dialer
                .send_to_app(app, from.clone(), Some(from.clone()), vec![number])
                .await
                .expect("the reply is sent");
        }
    });
    // the writer holds one reply in its write and the queue one more: the third
    // has to wait, however long that is
    tokio::time::sleep(NOTHING).await;
    assert!(!sending.is_finished(), "the replies did not wait for room");
    gate.open();
    in_five_seconds("the replies to be sent", sending)
        .await
        .expect("the sending task");
    for number in 0..5u8 {
        assert_eq!(lab.datagram().await.payload, [number]);
    }
    assert_eq!(lab.counters(), Counters::default());
}

/// A reply that waits for room in the writer's queue does not wait on past the
/// stop: the writer, stuck in a write the device never finishes, gives it up
/// `STOP_GRACE` after the stop and lets go of its queue, and the wait ends with
/// `BrokenPipe`, which is what ends the engine's task that was sending the
/// reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_waiting_for_room_ends_at_the_stop() {
    let (lab, _gate) = gated();
    let (app, far) = (addr(APP4), addr(FAR4));
    lab.send(app, far, b"hello");
    lab.associations_open(1).await;
    let dialer = lab.rig.dialer.clone();
    let from = target_of(far);
    // replies one after the other until one fails: the writer holds the first
    // in its write and the queue the second, so the third is the one that waits
    let sending = tokio::spawn(async move {
        for number in 0..5u8 {
            if let Err(error) = dialer
                .send_to_app(app, from.clone(), Some(from.clone()), vec![number])
                .await
            {
                return (number, error);
            }
        }
        panic!("every reply was sent, though the device takes nothing");
    });
    tokio::time::sleep(NOTHING).await;
    assert!(!sending.is_finished(), "the replies did not wait for room");
    let stopped = Instant::now();
    lab.rig.tun.stop();
    let (number, error) = in_five_seconds("the waiting reply to end", sending)
        .await
        .expect("the sending task");
    let waited = stopped.elapsed();
    assert_eq!(number, 2, "the third reply is the one that waited");
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe, "{error}");
    // the writer's grace, and half a second more
    assert!(
        waited < STOP_GRACE + Duration::from_millis(500),
        "the wait ended {waited:?} after the stop"
    );
    let ending = in_five_seconds("the TUN to end", lab.rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
}
