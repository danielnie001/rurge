//! What dropping a stream does to its connection, one shard: it is reset unless
//! it is ending well, a stream whose connection is gone touches no other, and a
//! connection that waits for the app's acknowledgement is reset when its linger
//! is over.

use super::{ByHand, FIN_ACK, answers_reset, payloads, state_of, turn};
use crate::TunOptions;
use crate::tcp::Outcome;
use crate::tcp::shard::Records;
use crate::tests::hazards::{ACK, PSH_ACK};
use crate::tests::support::{is_fin, is_reset, lone_shard, within};
use smoltcp::socket::tcp;
use std::io;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A stream that is dropped resets its connection unless the connection is
/// ending well: its own side shut down, nothing more to come from the app and
/// nothing of what the app sent left unread (phase 3 M3 design 5.5). Each of
/// the ways it is not ends in a reset to the app; the one way it is leaves the
/// FIN to be acknowledged, and the connection goes with the acknowledgement.
#[tokio::test]
async fn a_dropped_stream_resets_its_connection_unless_it_is_ending_well() {
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let shard = &lone.shard;

    // never shut down
    let (_, stream) = ByHand::make(shard, epoch, 40000, 0).await;
    drop(stream);
    let out = turn(shard, at(10), vec![]);
    assert!(
        out.iter().any(|packet| is_reset(packet)),
        "a stream dropped with no shutdown resets the connection"
    );
    assert_eq!(
        shard.state().socket_count(),
        0,
        "and it goes with its reset"
    );

    // shut down, but the app may still send: its FIN is not in
    let (app, mut stream) = ByHand::make(shard, epoch, 40001, 100).await;
    within("the shutdown", stream.shutdown())
        .await
        .expect("the shutdown");
    let out = turn(shard, at(110), vec![]);
    assert!(out.iter().any(|packet| is_fin(packet)), "the FIN is out");
    assert!(!out.iter().any(|packet| is_reset(packet)));
    drop(stream);
    let out = turn(shard, at(120), vec![]);
    assert!(
        out.iter().any(|packet| is_reset(packet)),
        "the app can still send: the connection is reset"
    );
    assert_eq!(state_of(shard, app.id), None, "and it goes with its reset");

    // shut down, the app's FIN in, but what it sent before the FIN unread
    let (app, mut stream) = ByHand::make(shard, epoch, 40002, 200).await;
    turn(shard, at(210), vec![app.segment(2, 0, PSH_ACK, b"abc")]);
    turn(shard, at(211), vec![app.segment(5, 0, FIN_ACK, &[])]);
    within("the shutdown", stream.shutdown())
        .await
        .expect("the shutdown");
    drop(stream);
    let out = turn(shard, at(220), vec![]);
    assert!(
        out.iter().any(|packet| is_reset(packet)),
        "three bytes are unread: the connection is reset"
    );
    assert_eq!(
        state_of(shard, app.id),
        None,
        "a stream that is gone does not hold the connection for what it did not read"
    );

    // never shut down, the app's FIN in and nothing unread: the app would wait
    // for a FIN that is never sent
    let (app, stream) = ByHand::make(shard, epoch, 40003, 300).await;
    turn(shard, at(310), vec![app.segment(2, 0, FIN_ACK, &[])]);
    drop(stream);
    let out = turn(shard, at(320), vec![]);
    assert!(
        out.iter().any(|packet| is_reset(packet)),
        "a stream that never shut down resets the connection the app has closed its side of"
    );
    assert_eq!(state_of(shard, app.id), None, "and it goes with its reset");

    // shut down, the app's FIN read, nothing unread: the FIN is on its way
    let (app, mut stream) = ByHand::make(shard, epoch, 40004, 400).await;
    turn(shard, at(410), vec![app.segment(2, 0, FIN_ACK, &[])]);
    let end = within("the end of the stream", stream.read(&mut [0; 1]))
        .await
        .expect("the end of the stream");
    assert_eq!(end, 0);
    within("the shutdown", stream.shutdown())
        .await
        .expect("the shutdown");
    drop(stream);
    let out = turn(shard, at(420), vec![]);
    assert!(out.iter().any(|packet| is_fin(packet)), "the FIN goes out");
    assert!(
        !out.iter().any(|packet| is_reset(packet)),
        "nothing is left to reset the connection for"
    );
    assert_eq!(state_of(shard, app.id), Some(tcp::State::LastAck));
    // the app acknowledges the FIN: the connection ends well
    turn(shard, at(430), vec![app.segment(3, 1, ACK, &[])]);
    assert_eq!(state_of(shard, app.id), None);
    assert_eq!(app.shared.outcome(), Some(Outcome::Closed));
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.budget.used(), 0);
    assert_eq!(shard.state().records(), Records::default());
}

/// A stream of a connection the shard has let go reaches no other. smoltcp 0.12
/// gives the lowest free handle again, with nothing to tell the old socket of
/// the handle from the new one, so a stream that kept a handle would read and
/// write the socket of the connection that came after. B is made on the handle
/// A had; A's stream, still in hand, is told of its reset by every call and
/// B gets none of its bytes. A reader that waits as the shard lets A go (the
/// stop, a rebuild or a linger do not change the socket's state) is woken.
#[tokio::test]
async fn a_stream_whose_connection_is_gone_touches_no_other() {
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let shard = &lone.shard;

    let (a, a_stream) = ByHand::make(shard, epoch, 40000, 0).await;
    let a_handle = shard.state().conns[&a.id].handle;
    let reading = tokio::spawn(async move {
        let mut stream = a_stream;
        let read = stream.read(&mut [0; 16]).await;
        (stream, read)
    });
    // the current-thread runtime runs the reader to the point where it waits
    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
    shard.state().end_all();
    let (mut a_stream, read) = within("A's reader to wake", reading)
        .await
        .expect("A's reader");
    assert_eq!(
        read.expect_err("A was let go").kind(),
        io::ErrorKind::ConnectionReset
    );
    assert_eq!(shard.state().socket_count(), 0);

    // B's socket is the one A had
    let (b, mut b_stream) = ByHand::make(shard, epoch, 40001, 10).await;
    assert_eq!(
        shard.state().conns[&b.id].handle,
        a_handle,
        "B has the handle A had"
    );
    turn(shard, at(20), vec![b.segment(2, 0, PSH_ACK, b"B's bytes")]);
    answers_reset(&mut a_stream).await;
    // B's bytes are B's, and what A wrote went nowhere
    let mut got = [0; 9];
    within("B's bytes", b_stream.read_exact(&mut got))
        .await
        .expect("B's bytes are read");
    assert_eq!(&got, b"B's bytes");
    within("B's reply", b_stream.write_all(b"B's reply"))
        .await
        .expect("B's reply is written");
    let out = turn(shard, at(30), vec![]);
    assert_eq!(
        payloads(&out),
        b"B's reply",
        "B sent its own bytes and A's write is none of them"
    );
    assert_eq!(shard.state().socket_count(), 1);
    assert_eq!(lone.admission.sockets(), 1);
}

// ---- the linger, one shard

/// A stream dropped while its connection is waiting for the app to acknowledge
/// its FIN leaves the connection to wait for `linger` and no more: then the
/// shard resets it, and lets it go once its reset is out and not before (the
/// app would be left without the reset). The shard wakes for the deadline on
/// its own, and finds it without waiting for its next look at every
/// connection, which is a second apart. The times are real ones from the drop
/// on, for the drop is where the deadline is taken.
#[tokio::test]
async fn a_dropped_stream_is_given_its_linger_and_then_reset() {
    let linger = Duration::from_millis(100);
    let options = TunOptions {
        linger,
        ..TunOptions::default()
    };
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&options, epoch);
    let shard = &lone.shard;
    let (app, mut stream) = ByHand::make(shard, epoch, 40000, 0).await;
    // the app's FIN is read, the stream's own is sent, and the app never
    // acknowledges it
    turn(shard, at(10), vec![app.segment(2, 0, FIN_ACK, &[])]);
    let end = within("the end of the stream", stream.read(&mut [0; 1]))
        .await
        .expect("the end of the stream");
    assert_eq!(end, 0);
    within("the shutdown", stream.shutdown())
        .await
        .expect("the shutdown");
    let out = turn(shard, at(20), vec![]);
    assert!(out.iter().any(|packet| is_fin(packet)), "the FIN is out");
    // the drop resets nothing: the connection is ending well, but for an
    // acknowledgement that may not come
    let before = Instant::now();
    drop(stream);
    let after = Instant::now();
    assert_eq!(state_of(shard, app.id), Some(tcp::State::LastAck));
    // a millisecond short of the earliest the deadline can be: nothing happens
    // and the shard knows when to come back
    let early = shard
        .state()
        .turn(before + linger - Duration::from_millis(1), &mut Vec::new());
    assert!(!early.out.iter().any(|packet| is_reset(packet)));
    assert_eq!(state_of(shard, app.id), Some(tcp::State::LastAck));
    // the next thing the stack itself has to do is to send the FIN again,
    // and smoltcp's retransmission timeout starts at several hundred
    // milliseconds: the wake is for the linger, or it is too late
    let wake = early.wake_at.expect("the shard has a time to wake for");
    assert!(
        wake <= after + linger,
        "the shard wakes for the linger at the latest when it runs out"
    );
    // the latest the deadline can be: the connection is aborted. Its reset is
    // not out in this turn, so the shard keeps the connection and comes back
    // at once
    let due = after + linger;
    let aborting = shard.state().turn(due, &mut Vec::new());
    assert!(!aborting.out.iter().any(|packet| is_reset(packet)));
    assert_eq!(state_of(shard, app.id), Some(tcp::State::Closed));
    assert_eq!(
        aborting.wake_at,
        Some(due),
        "the reset is to go out at once"
    );
    // the next turn sends the reset, and the connection goes with it
    let reset = shard.state().turn(due, &mut Vec::new());
    assert!(
        reset.out.iter().any(|packet| is_reset(packet)),
        "the app is told"
    );
    assert_eq!(state_of(shard, app.id), None);
    assert_eq!(app.shared.outcome(), Some(Outcome::Reset));
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.budget.used(), 0);
    assert_eq!(shard.state().records(), Records::default());
}

/// `TunOptions` is public: a `linger` that cannot be added to a time is no
/// reason for a panic in a drop, and the connection simply has no deadline in
/// any time to come. It goes as any does when the app acknowledges the FIN, and
/// takes its place in the set of the connections whose stream is gone with it.
#[tokio::test]
async fn a_linger_too_long_to_add_does_not_panic() {
    let options = TunOptions {
        linger: Duration::MAX,
        ..TunOptions::default()
    };
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&options, epoch);
    let shard = &lone.shard;
    let (app, mut stream) = ByHand::make(shard, epoch, 40000, 0).await;
    turn(shard, at(10), vec![app.segment(2, 0, FIN_ACK, &[])]);
    let end = within("the end of the stream", stream.read(&mut [0; 1]))
        .await
        .expect("the end of the stream");
    assert_eq!(end, 0);
    within("the shutdown", stream.shutdown())
        .await
        .expect("the shutdown");
    turn(shard, at(20), vec![]);
    drop(stream);
    assert_eq!(state_of(shard, app.id), Some(tcp::State::LastAck));
    assert_eq!(shard.state().records().lingering, 1);
    turn(shard, at(30), vec![app.segment(3, 1, ACK, &[])]);
    assert_eq!(state_of(shard, app.id), None);
    assert_eq!(shard.state().records(), Records::default());
}

/// A stream is dropped once, by `TunStream::drop`, but the shard does not rely
/// on that: told of a second drop of one connection it does nothing, so the
/// connection has one entry in the set of those whose stream is gone, the one
/// the first drop made. A second entry would outlive the connection (the
/// connection takes only its own deadline with it), and when its time came the
/// shard would panic with its lock held and be started over, every connection
/// it has reset.
#[tokio::test]
async fn a_second_drop_of_one_connection_leaves_no_second_entry() {
    let linger = Duration::from_millis(100);
    let options = TunOptions {
        linger,
        ..TunOptions::default()
    };
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&options, epoch);
    let shard = &lone.shard;
    let (app, mut stream) = ByHand::make(shard, epoch, 40000, 0).await;
    // the app's FIN is read, the stream's own is sent, and the app has yet to
    // acknowledge it
    turn(shard, at(10), vec![app.segment(2, 0, FIN_ACK, &[])]);
    let end = within("the end of the stream", stream.read(&mut [0; 1]))
        .await
        .expect("the end of the stream");
    assert_eq!(end, 0);
    within("the shutdown", stream.shutdown())
        .await
        .expect("the shutdown");
    turn(shard, at(20), vec![]);
    let before = Instant::now();
    drop(stream);
    assert_eq!(state_of(shard, app.id), Some(tcp::State::LastAck));
    assert_eq!(shard.state().records().lingering, 1);
    // a second drop of the same connection, a little later than the first
    shard
        .state()
        .stream_dropped(app.id, before + Duration::from_millis(50));
    assert_eq!(
        shard.state().records().lingering,
        1,
        "the second drop made no second entry"
    );
    assert_eq!(state_of(shard, app.id), Some(tcp::State::LastAck));
    // the app acknowledges the FIN in time: the connection goes, and so does
    // everything the shard has of it
    turn(shard, at(30), vec![app.segment(3, 1, ACK, &[])]);
    assert_eq!(state_of(shard, app.id), None);
    assert_eq!(
        shard.state().records(),
        Records::default(),
        "nothing stays behind for a deadline that comes later"
    );
    // the deadline comes with no connection to find, and the shard has nothing
    // to wake for
    let late = shard.state().turn(before + linger * 3, &mut Vec::new());
    assert!(late.out.is_empty());
    assert_eq!(late.wake_at, None);
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.budget.used(), 0);
}
