//! The bytes of the datagrams that wait in the UDP sessions for the engine to
//! take them: one budget for the whole TUN (`udp_bytes`), taken by a datagram as
//! it joins a session's queue and given back when the engine takes it or the
//! queue is dropped with it in it.

use super::{APP4, FAR4, NOTHING, lab};
use crate::tests::support::{WITHIN, addr, eventually};
use crate::udp::bytes::QueuedBytes;
use crate::{Counters, TunOptions};
use rurge_inbound::UdpAdmission;
use std::sync::Arc;
use std::time::Duration;

/// The budget in itself: what fits is what is left, to the byte; a reservation
/// is given back when it is dropped; a reservation of nothing always fits; and
/// nothing overflows.
#[test]
fn a_reservation_fits_up_to_the_limit_to_the_byte() {
    let budget = Arc::new(QueuedBytes::new(10));
    let six = budget.reserve(6).expect("6 of 10 fit");
    assert_eq!(budget.queued(), 6);
    assert!(budget.reserve(5).is_none(), "6 and 5 are not 10");
    assert_eq!(
        budget.queued(),
        6,
        "a reservation that does not fit takes nothing"
    );
    let four = budget.reserve(4).expect("6 and 4 are 10");
    assert_eq!(budget.queued(), 10);
    assert!(budget.reserve(1).is_none());
    let empty = budget.reserve(0).expect("no bytes fit in a full budget");
    assert_eq!(budget.queued(), 10);
    // a reservation that would overflow the counter is one that does not fit
    assert!(budget.reserve(usize::MAX).is_none());
    drop(six);
    assert_eq!(budget.queued(), 4, "dropped, it is given back");
    let again = budget.reserve(6).expect("what is given back fits again");
    assert_eq!(budget.queued(), 10);
    drop((four, again, empty));
    assert_eq!(budget.queued(), 0);
    // a budget of nothing takes nothing but nothing
    let none = Arc::new(QueuedBytes::new(0));
    assert!(none.reserve(1).is_none());
    assert!(none.reserve(0).is_some());
}

/// Datagrams wait in a session for an association that takes none: they fill
/// the budget to the byte, the one that does not fit is dropped and counted
/// `queue_full` as one that finds the queue full is, and once the association
/// takes what waits, the bytes are free again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_udp_is_bounded_by_its_byte_budget() {
    let lab = lab(TunOptions {
        udp_bytes: 4_096,
        ..TunOptions::default()
    });
    // an association that takes nothing, so that what the app sends waits
    lab.dialer().hold_udp();
    let (app, dst) = (addr(APP4), addr(FAR4));
    for _ in 0..4 {
        lab.send(app, dst, &[7; 1_000]);
    }
    eventually("four datagrams to wait", WITHIN, || {
        lab.rig.tun.udp_bytes_queued() == 4_000
    })
    .await;
    // 5,000 bytes do not fit in 4,096
    lab.send(app, dst, &[7; 1_000]);
    eventually("the fifth to be counted", WITHIN, || {
        lab.counters().queue_full == 1
    })
    .await;
    assert_eq!(lab.rig.tun.udp_bytes_queued(), 4_000, "it took no bytes");
    // 96 bytes are what is left, and they fit to the byte
    lab.send(app, dst, &[8; 96]);
    eventually("the small one to wait", WITHIN, || {
        lab.rig.tun.udp_bytes_queued() == 4_096
    })
    .await;
    // the association takes them all, and the bytes come back
    lab.dialer().release_udp();
    eventually("the datagrams to be taken", WITHIN, || {
        lab.dialer().udp_sent().len() == 5
    })
    .await;
    eventually("the bytes to come back", WITHIN, || {
        lab.rig.tun.udp_bytes_queued() == 0
    })
    .await;
    // and a datagram of 1,000 bytes fits again
    lab.send(app, dst, &[9; 1_000]);
    eventually("the next datagram to be taken", WITHIN, || {
        lab.dialer().udp_sent().len() == 6
    })
    .await;
    assert_eq!(
        lab.counters(),
        Counters {
            queue_full: 1,
            ..Counters::default()
        }
    );
}

/// A session that ends with datagrams still waiting in its queue gives their
/// bytes back with the queue.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_bytes_come_back_when_a_session_ends() {
    let lab = lab(TunOptions {
        udp_idle: Duration::from_millis(300),
        udp_dns_idle: Duration::from_millis(300),
        ..TunOptions::default()
    });
    lab.dialer().hold_udp();
    let (app, dst) = (addr(APP4), addr(FAR4));
    for _ in 0..3 {
        lab.send(app, dst, &[7; 1_000]);
    }
    eventually("three datagrams to wait", WITHIN, || {
        lab.rig.tun.udp_bytes_queued() == 3_000
    })
    .await;
    // the session ends of the quiet, with the datagrams in its queue
    eventually("the session to end", WITHIN, || {
        lab.dialer().associations_open() == 0 && lab.rig.tun.session_tasks() == 0
    })
    .await;
    eventually("the bytes to come back", WITHIN, || {
        lab.rig.tun.udp_bytes_queued() == 0
    })
    .await;
    assert!(
        lab.dialer().udp_sent().is_empty(),
        "the association took none of them"
    );
}

/// The first datagram of a session that does not fit begins no session, and the
/// dialer is not asked whether it would serve one (it is told to turn every one
/// away here, and what is counted shows that as well); one that fits and is
/// turned away gives its bytes back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_datagram_that_does_not_fit_begins_no_session() {
    let lab = lab(TunOptions {
        udp_bytes: 1_000,
        ..TunOptions::default()
    });
    let (app, dst) = (addr(APP4), addr(FAR4));
    lab.dialer().admit(UdpAdmission::Busy);
    lab.send(app, dst, &[7; 1_200]);
    eventually("the datagram to be counted", WITHIN, || {
        lab.counters().queue_full == 1
    })
    .await;
    // the wait is the one for what must not happen: the count of `busy` that
    // the dialer's answer would give; the datagram has been through the
    // receive task in well under a millisecond, so 200 ms is a wide margin
    tokio::time::sleep(NOTHING).await;
    assert_eq!(
        lab.counters(),
        Counters {
            queue_full: 1,
            ..Counters::default()
        }
    );
    assert!(lab.dialer().associations().is_empty());
    assert_eq!(lab.dialer().admissions_asked(), 0, "the dialer was asked");
    assert_eq!(lab.rig.tun.session_tasks(), 0);
    assert_eq!(lab.rig.tun.udp_bytes_queued(), 0);
    // one that fits is put to the dialer, which turns it away: its bytes come back
    lab.send(app, dst, &[7; 800]);
    eventually("the datagram to be turned away", WITHIN, || {
        lab.counters().busy == 1
    })
    .await;
    assert_eq!(lab.dialer().admissions_asked(), 1);
    assert_eq!(lab.rig.tun.udp_bytes_queued(), 0, "no session holds them");
    // and once the dialer takes sessions the same datagram begins one
    lab.dialer().admit(UdpAdmission::Accepted);
    lab.send(app, dst, &[7; 800]);
    lab.associations_open(1).await;
    eventually("the datagram to be taken", WITHIN, || {
        lab.dialer().udp_sent().len() == 1
    })
    .await;
    assert_eq!(lab.counters().queue_full, 1);
}
