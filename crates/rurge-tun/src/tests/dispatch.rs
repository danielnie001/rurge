//! What the task that reads the device does with a packet before a shard has
//! it (phase 3 M3 design 4.3, M4a): which shard a segment goes to, and what the
//! bytes the shards' queues may hold together (`TunOptions::queue_bytes`) do to
//! a packet that would not fit there.

use super::hazards::app4;
use super::support::{GatedDevice, WITHIN, addr, eventually, rig_with, sock, syn_v4};
use crate::bytes::QueuedBytes;
use crate::dispatch::{route, shard_of};
use crate::stats::Stats;
use crate::tcp::Tuple;
use crate::{Counters, TunOptions};
use rurge_config::IpRewrite;
use std::hash::RandomState;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use tokio::sync::mpsc;

/// The bytes the shards' queues may hold together are a budget of their own: a
/// packet that would not fit is not given to the shard, and is counted
/// `queue_full`. A shard that is not taking what it is given (`stopped`, below,
/// is never read) holds the bytes of the packets that fit, and never more than
/// the budget.
#[test]
fn a_shard_queue_beyond_queue_bytes_drops_and_counts_queue_full() {
    let app = app4(40_000);
    let syn = |port: u16| syn_v4(app, addr(format!("192.0.2.1:{port}").as_str()));
    let limit = 3 * syn(1_000).capacity();
    let queue_bytes = Arc::new(QueuedBytes::new(limit));
    let stats = Stats::default();
    let key = RandomState::new();
    // the shard's own limit on the count is far above what is sent, so what
    // drops a packet here is the bytes
    let (to_shard, mut stopped) = mpsc::channel(64);
    let mut held = 0;
    let mut dropped = 0;
    for port in 0..10u16 {
        let packet = syn(1_000 + port);
        let size = packet.capacity();
        // what the budget can still take, to the byte
        if held + size <= limit {
            held += size;
        } else {
            dropped += 1;
        }
        let routed = route(
            packet,
            true,
            &IpRewrite::default(),
            std::slice::from_ref(&to_shard),
            &stats,
            &queue_bytes,
            &key,
        );
        assert!(routed.is_none(), "a TCP segment is handed on by `route`");
        assert!(
            queue_bytes.queued() <= limit,
            "{} bytes wait, and the budget is {limit}",
            queue_bytes.queued()
        );
    }
    assert_eq!(
        stats.counters(),
        Counters {
            queue_full: dropped,
            ..Counters::default()
        }
    );
    assert_eq!(queue_bytes.queued(), held, "what fits holds its bytes");
    // what was given to the shard is taken out of its queue, and each packet
    // gives its bytes back as it goes
    let mut taken = 0;
    while let Ok(ingress) = stopped.try_recv() {
        taken += 1;
        drop(ingress);
    }
    assert_eq!(taken, 10 - dropped, "the shard is given what fitted");
    assert_eq!(queue_bytes.queued(), 0);
}

/// The bytes a packet holds while it waits are given back when a shard takes
/// it: a shard that the writer's queue and a device that writes nothing hold up
/// leaves the packets waiting for it, and its budget is empty again once it has
/// them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_bytes_are_given_back_when_the_shard_takes_the_packets() {
    const BUDGET: usize = 8 * 1024;
    let (gate, end) = GatedDevice::pair();
    let rig = rig_with(
        gate.clone(),
        end,
        TunOptions {
            shards: 1,
            shard_queue: 4_096,
            writer_queue: 1,
            queue_bytes: BUDGET,
            ..TunOptions::default()
        },
    );
    let app = app4(40_000);
    let syn = |port: u16| syn_v4(app, addr(format!("192.0.2.1:{port}").as_str()));
    // far more SYNs than the shard takes before the writer's queue is full and
    // the device, which writes nothing until `gate` is opened, holds it up
    for port in 0..1_024u16 {
        rig.app.inject(syn(1_000 + port));
    }
    eventually("the shards' queue to take up its bytes", WITHIN, || {
        rig.tun.queue_bytes_queued() > 0
    })
    .await;
    assert!(
        rig.tun.queue_bytes_queued() <= BUDGET,
        "{} bytes wait, and the budget is {BUDGET}",
        rig.tun.queue_bytes_queued()
    );
    gate.open();
    eventually("the bytes to come back", WITHIN, || {
        rig.tun.queue_bytes_queued() == 0
    })
    .await;
}

/// The shard a connection goes to is chosen by a key of the TUN's own: one key
/// chooses the same shard for the same connection every time, and two TUNs'
/// keys do not choose the same shards for a run of connections (for 64 of them,
/// the chance that they all agree is 256^-64).
#[test]
fn shards_are_chosen_with_a_key_of_each_tun() {
    const SHARDS: usize = 256;
    let one = RandomState::new();
    let other = RandomState::new();
    let tuples: Vec<Tuple> = (0..64u16)
        .map(|port| Tuple {
            src: sock(IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1)), 40_000 + port),
            dst: sock(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 80),
        })
        .collect();
    let chosen = |key: &RandomState| -> Vec<usize> {
        tuples
            .iter()
            .map(|tuple| shard_of(tuple, SHARDS, key))
            .collect()
    };
    for (tuple, shard) in tuples.iter().zip(chosen(&one)) {
        assert!(shard < SHARDS);
        assert_eq!(shard, shard_of(tuple, SHARDS, &one), "the same key again");
    }
    assert_ne!(
        chosen(&one),
        chosen(&other),
        "a TUN of its own keeps a shard for each connection the other does not"
    );
}
