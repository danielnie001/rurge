//! The life of a TUN's stream: what its reads, writes and shutdown answer as
//! its connection ends, what dropping it does, and what is left of a
//! connection once it is over. A test at the shard's level drives one shard by
//! hand: it hands the shard the app's segments itself and takes the shard's
//! turns at times of its own, so what it checks does not hang on how fast
//! anything runs. The others run a whole TUN on a memory device.

use super::hazards::{ACK, PSH_ACK, app4, open_by_hand};
use super::support::{
    WITHIN, addr, data_segment_v4, eventually, hello, is_fin, is_reset, is_syn, lone_shard,
    read_pattern, read_pattern_from, rig, segment_v4, syn_v4, tcp_header, tcp_payload_len, within,
};
use crate::tcp::shard::Records;
use crate::tcp::{ConnId, ConnShared, Outcome, Shard, TunStream};
use crate::testing::{Direction, Script, Upstream, pattern};
use crate::{Counters, TunOptions};
use smoltcp::socket::tcp;
use std::collections::HashSet;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

const FIN_ACK: u8 = 0x11;
const RST_ACK: u8 = 0x14;

/// The TCP payload of every packet in `out`, in order.
fn payloads(out: &[Vec<u8>]) -> Vec<u8> {
    out.iter()
        .flat_map(|packet| {
            packet[packet.len() - tcp_payload_len(packet)..]
                .iter()
                .copied()
        })
        .collect()
}

/// One turn of `shard` at `at`, handed `packets`: what the stack sent in it.
fn turn(shard: &Shard, at: Instant, packets: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    shard.state().turn(at, &mut { packets }).out
}

/// The state of the socket of connection `id`, if the shard still has it.
fn state_of(shard: &Shard, id: ConnId) -> Option<tcp::State> {
    let state = shard.state();
    let conn = state.conns.get(&id)?;
    Some(state.sockets.get::<tcp::Socket>(conn.handle).state())
}

/// The app's end of a connection made by hand on a shard: where it is and the
/// numbers its segments need.
struct ByHand {
    id: ConnId,
    shared: Arc<ConnShared>,
    app: SocketAddr,
    dst: SocketAddr,
    /// The sequence number of the TUN's SYN-ACK.
    isn: u32,
}

impl ByHand {
    /// The connection from port `port` of the app, made at `ms` and a
    /// millisecond later, and the stream a session would have of it.
    async fn make(shard: &Arc<Shard>, epoch: Instant, port: u16, ms: u64) -> (ByHand, TunStream) {
        let (app, dst) = (app4(port), addr("192.0.2.1:80"));
        let (opened, isn) = open_by_hand(shard, epoch, app, dst, ms);
        let by_hand = ByHand {
            id: opened.id,
            shared: opened.shared.clone(),
            app,
            dst,
            isn,
        };
        let stream = TunStream::new(shard.clone(), opened.id, opened.shared);
        within("the handshake", stream.established())
            .await
            .expect("the connection is established");
        (by_hand, stream)
    }

    /// A segment of the app's. The app's SYN was numbered 1, so its first byte
    /// is 2 and `seq` counts from there. `acked` is how many of the TUN's
    /// sequence numbers, bytes and FIN, it acknowledges, counted from the
    /// TUN's SYN-ACK.
    fn segment(&self, seq: u32, acked: u32, flags: u8, payload: &[u8]) -> Vec<u8> {
        data_segment_v4(
            self.app,
            self.dst,
            seq,
            self.isn.wrapping_add(1 + acked),
            flags,
            payload,
        )
    }
}

/// Every read, write and shutdown of `stream` is a reset.
async fn answers_reset(stream: &mut TunStream) {
    let read = within("a read", stream.read(&mut [0; 1]))
        .await
        .expect_err("a read of a reset connection is an error");
    let write = within("a write", stream.write_all(b"x"))
        .await
        .expect_err("a write to a reset connection is an error");
    let shutdown = within("a shutdown", stream.shutdown())
        .await
        .expect_err("a shutdown of a reset connection is an error");
    for error in [read, write, shutdown] {
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    }
}

/// smoltcp closes a socket that has had nothing from the app for its
/// timeout, with no segment to say so; the shard sees it when it next looks
/// at every connection, up to a `SWEEP` later. A shutdown in between answers
/// the reset the read answers, which is the outcome the shard records when
/// it lets the connection go.
#[tokio::test]
async fn a_shutdown_after_the_stacks_own_timeout_is_a_reset() {
    let options = TunOptions {
        timeout: Duration::from_millis(100),
        ..TunOptions::default()
    };
    let epoch = Instant::now();
    let at = |ms| epoch + Duration::from_millis(ms);
    let shard = lone_shard(&options, epoch).shard;
    let (app, dst) = (addr("198.18.0.1:40000"), addr("192.0.2.1:80"));
    let mut turn = shard.state().turn(at(0), &mut vec![syn_v4(app, dst)]);
    let opened = turn.opened.pop().expect("the SYN opens a connection");
    let syn_ack = turn
        .out
        .iter()
        .find(|packet| is_syn(packet))
        .expect("the SYN-ACK");
    let (header, _) = tcp_header(syn_ack).expect("a TCP segment");
    let isn = u32::from_be_bytes(
        syn_ack[header + 4..header + 8]
            .try_into()
            .expect("four bytes"),
    );
    // the app's ACK ends the handshake
    let ack = segment_v4(app, dst, 2, isn.wrapping_add(1), 0x10);
    shard.state().turn(at(1), &mut vec![ack]);
    let mut stream = TunStream::new(shard.clone(), opened.id, opened.shared);
    within("the handshake", stream.established())
        .await
        .expect("the connection is established");
    // past the timeout, before the shard's next look at every connection
    let turn = shard.state().turn(at(200), &mut Vec::new());
    assert!(
        turn.out.iter().any(|packet| is_reset(packet)),
        "smoltcp's timeout resets the connection"
    );
    assert_eq!(
        shard.state().socket_count(),
        1,
        "the shard has not let the connection go yet"
    );
    let error = within("the shutdown", stream.shutdown())
        .await
        .expect_err("a reset, as the read answers");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    let error = within("the read", stream.read(&mut [0; 1]))
        .await
        .expect_err("a reset");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    // the next look at every connection lets it go, a reset
    shard.state().turn(at(1_100), &mut Vec::new());
    assert_eq!(
        shard.state().socket_count(),
        0,
        "the shard has let the connection go"
    );
    let error = within("the shutdown", stream.shutdown())
        .await
        .expect_err("the outcome, a reset");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
}

// ---- what a stream answers, one shard

/// The end of what the app sent is the end of the stream, however often it is
/// asked; the app having closed its side leaves the stream its own to write on
/// until it shuts it down, and then a write is a broken pipe. All of it stays
/// so once the connection has ended well and the shard has let it go. When the
/// app resets the connection, or the shard lets it go for another reason (the
/// stop, a rebuild), every read, write and shutdown is a reset (P12).
#[tokio::test]
async fn a_stream_answers_by_how_its_connection_ended() {
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let shard = &lone.shard;

    // the app sends three bytes and its FIN, and takes the TUN's byte and FIN
    let (app, mut stream) = ByHand::make(shard, epoch, 40000, 0).await;
    turn(shard, at(10), vec![app.segment(2, 0, PSH_ACK, b"abc")]);
    turn(shard, at(11), vec![app.segment(5, 0, FIN_ACK, &[])]);
    let mut got = [0; 3];
    within("the bytes before the FIN", stream.read_exact(&mut got))
        .await
        .expect("the bytes are read");
    assert_eq!(&got, b"abc");
    for _ in 0..2 {
        let end = within("the end of the stream", stream.read(&mut got))
            .await
            .expect("the end of the stream is no error");
        assert_eq!(end, 0, "what the app sent is read, then the end");
    }
    within("a write", stream.write_all(b"x"))
        .await
        .expect("the app has closed its sending side only; the stream can still write");
    within("the shutdown", stream.shutdown())
        .await
        .expect("the shutdown");
    let error = within("a write", stream.write_all(b"y"))
        .await
        .expect_err("nothing is written after the stream's own FIN");
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    // the byte and the FIN go out, and the app acknowledges both
    turn(shard, at(20), vec![]);
    turn(shard, at(30), vec![app.segment(6, 2, ACK, &[])]);
    assert_eq!(shard.state().socket_count(), 0, "the shard has let it go");
    assert_eq!(app.shared.outcome(), Some(Outcome::Closed));
    let end = within("the end of the stream", stream.read(&mut got))
        .await
        .expect("the end of the stream is no error");
    assert_eq!(end, 0);
    let error = within("a write", stream.write_all(b"y"))
        .await
        .expect_err("the connection is closed for writing");
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    within("the shutdown", stream.shutdown())
        .await
        .expect("a shutdown of a connection that ended well is no error");

    // the connection ends well while the stream has not read what the app sent:
    // the shard keeps it for the stream to read, and until then a write is a
    // broken pipe, not a reset
    let (app, mut stream) = ByHand::make(shard, epoch, 40001, 100).await;
    turn(shard, at(110), vec![app.segment(2, 0, PSH_ACK, b"abc")]);
    turn(shard, at(111), vec![app.segment(5, 0, FIN_ACK, &[])]);
    within("the shutdown", stream.shutdown())
        .await
        .expect("the shutdown");
    turn(shard, at(120), vec![]);
    turn(shard, at(130), vec![app.segment(6, 1, ACK, &[])]);
    assert_eq!(
        state_of(shard, app.id),
        Some(tcp::State::Closed),
        "the shard keeps what the stream has not read"
    );
    assert_eq!(app.shared.outcome(), None);
    let error = within("a write", stream.write_all(b"y"))
        .await
        .expect_err("the connection is closed for writing");
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    within("the bytes before the FIN", stream.read_exact(&mut got))
        .await
        .expect("the bytes are read");
    assert_eq!(&got, b"abc");
    let end = within("the end of the stream", stream.read(&mut got))
        .await
        .expect("the end of the stream is no error");
    assert_eq!(end, 0);
    turn(shard, at(140), vec![]);
    assert_eq!(
        state_of(shard, app.id),
        None,
        "and lets it go once it is read"
    );
    assert_eq!(app.shared.outcome(), Some(Outcome::Closed));

    // the app resets the connection
    let (app, mut stream) = ByHand::make(shard, epoch, 40002, 200).await;
    turn(shard, at(210), vec![app.segment(2, 0, RST_ACK, &[])]);
    assert_eq!(shard.state().socket_count(), 0, "the shard has let it go");
    assert_eq!(app.shared.outcome(), Some(Outcome::Reset));
    answers_reset(&mut stream).await;

    // the shard lets the connection go for another reason
    let (app, mut stream) = ByHand::make(shard, epoch, 40003, 300).await;
    shard.state().end_all();
    assert_eq!(app.shared.outcome(), Some(Outcome::Reset));
    answers_reset(&mut stream).await;
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(lone.budget.used(), 0);
    assert_eq!(shard.state().records(), Records::default());
}

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

// ---- a TUN, its app and its upstream

/// Both sides close their write sides, in both orders, and nothing is left:
/// when the app closes first the TUN's socket is the passive closer (LAST-ACK)
/// and when the far end does it is the active one (TIME-WAIT).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_graceful_close_leaves_no_socket() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let source = Upstream::source(10_000).await;
    let (app_first, far_first) = (addr("192.0.2.1:80"), addr("192.0.2.2:80"));
    // a socket in TIME-WAIT goes within a few milliseconds of the TUN seeing
    // it there; smoltcp would let it go after ten seconds
    let five_seconds = Duration::from_secs(5);
    rig.dialer.script(app_first, Script::Upstream(echo.addr()));
    rig.dialer
        .script(far_first, Script::Upstream(source.addr()));

    // the app closes first; the echo server closes its own side when it has
    // read to the end
    let mut tcp = within("the connection to be made", rig.app.connect(app_first))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    tcp.shutdown().await.expect("shutdown");
    let mut more = [0; 1];
    let end = within("the end of the stream", tcp.read(&mut more))
        .await
        .expect("the end, not an error");
    assert_eq!(end, 0);
    drop(tcp);
    eventually("the first connection to go", five_seconds, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0
    })
    .await;

    // the far end closes first, the app closes when it has read to the end
    let mut tcp = within("the connection to be made", rig.app.connect(far_first))
        .await
        .expect("connect");
    within("the bytes of the source", read_pattern(&mut tcp, 10_000))
        .await
        .expect("read");
    let end = within("the end of the stream", tcp.read(&mut more))
        .await
        .expect("the end, not an error");
    assert_eq!(end, 0);
    tcp.shutdown().await.expect("shutdown");
    drop(tcp);
    eventually("the second connection to go", five_seconds, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0
    })
    .await;

    eventually("both relays to end", WITHIN, || {
        rig.dialer.relay_ends().len() == 2
    })
    .await;
    assert_eq!(rig.dialer.relay_ends(), vec![Ok(()), Ok(())]);
    assert_eq!(rig.tun.sockets_counted(), 0);
    assert_eq!(rig.tun.records(), Records::default());
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// What `value` reports once it has stopped changing: it has not changed for
/// `quiet`, or the test fails after `WITHIN`. This is the one fixed wait of the
/// test that uses it, for what is not to happen (the source writing on): 200 ms
/// of it. Once the TUN's send buffer is full the source's count of written
/// bytes went on changing for 43 ms at the most in thirty runs alone on the
/// development machine; a quiet that came too early would only start the
/// reading earlier, and nothing the test asserts would be different.
async fn settled(what: &str, quiet: Duration, value: impl Fn() -> u64) -> u64 {
    let started = Instant::now();
    let mut last = value();
    let mut since = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let now = value();
        if now != last {
            last = now;
            since = Instant::now();
        } else if since.elapsed() >= quiet {
            return last;
        }
        assert!(started.elapsed() < WITHIN, "timed out waiting for {what}");
    }
}

/// A download whose app stops reading: the app's window closes, the TUN's send
/// buffer fills, the relay stops reading its upstream and the source stops
/// writing. When the app reads again the rest comes, every byte of it and in
/// order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_that_stops_and_resumes_loses_nothing() {
    const TOTAL: usize = 4 << 20;
    const FIRST: usize = 64 << 10;
    let rig = rig(TunOptions::default());
    let source = Upstream::source(TOTAL as u64).await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(source.addr()));
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    within("the first 64 KiB", read_pattern(&mut tcp, FIRST))
        .await
        .expect("read");
    // the app reads nothing more, and the TUN's send buffer fills
    eventually("the TUN's send buffer to fill", WITHIN, || {
        let (queued, room) = rig.tun.send_buffers();
        room > 0 && queued == room
    })
    .await;
    // the relay is stuck on a write that the buffer does not take, and nothing
    // reaches the source's end any more: 200 ms with no byte more written
    let stopped = settled("the source to stop", Duration::from_millis(200), || {
        source.sent()
    })
    .await;
    let (queued, room) = rig.tun.send_buffers();
    assert_eq!(queued, room, "the TUN's send buffer is still full");
    assert!(stopped >= FIRST as u64, "the source wrote what was read");
    // the app reads on: the rest is the pattern from where it was
    within(
        "the rest of the download",
        read_pattern_from(&mut tcp, FIRST, TOTAL - FIRST),
    )
    .await
    .expect("read");
    let mut more = [0; 1];
    let end = within("the end of the stream", tcp.read(&mut more))
        .await
        .expect("the end, not an error");
    assert_eq!(end, 0, "nothing comes past the pattern");
    assert_eq!(source.sent(), TOTAL as u64);
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// An upload the app resets half way (the user closed the page, the process was
/// killed): the session ends with the reset, the connection to the sink is
/// closed, and nothing is kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_reset_mid_transfer_ends_the_session() {
    let rig = rig(TunOptions::default());
    let sink = Upstream::sink().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(sink.addr()));
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    within("the upload to get under way", async {
        let chunk = vec![0x5a; 16 * 1024];
        while sink.received() < 256 * 1024 {
            tcp.write_all(&chunk).await.expect("write");
        }
    })
    .await;
    assert_eq!(sink.open(), 1, "the sink has the connection");
    tcp.abort();
    let five_seconds = Duration::from_secs(5);
    eventually("the session to end", five_seconds, || {
        rig.dialer.relay_ends().len() == 1
    })
    .await;
    let ends = rig.dialer.relay_ends();
    assert!(
        matches!(&ends[0], Err(text) if text.contains("reset")),
        "the session ends with the reset: {ends:?}"
    );
    eventually("the TUN to let the connection go", five_seconds, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0
    })
    .await;
    eventually("the sink's connection to end", five_seconds, || {
        sink.open() == 0
    })
    .await;
    assert_eq!(rig.tun.sockets_counted(), 0);
    assert_eq!(rig.tun.records(), Records::default());
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// Short connections one after another, like a client that opens one for each
/// request. With one shard the handles of smoltcp's sockets are used again and
/// again: each connection gets back the bytes it sent, and when the last is
/// over there is nothing left, in the sockets, the budget, the counts or the
/// tables. The far end is an echo in memory: a server on the loopback would
/// cost the machine a port of its own for each connection, held for minutes
/// after the test, and a few runs in a row would leave it none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_thousand_connections_in_a_row_leave_nothing_behind() {
    const CONNECTIONS: usize = 2000;
    const BYTES: usize = 1024;
    let rig = rig(TunOptions {
        shards: 1,
        ..TunOptions::default()
    });
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::MemoryEcho { buffer: 4096 });
    let mut handles = HashSet::new();
    let all = async {
        for n in 0..CONNECTIONS {
            let mut tcp = within("the connection to be made", rig.app.connect(dst))
                .await
                .expect("connect");
            // every connection has bytes of its own
            let sent: Vec<u8> = (0..BYTES)
                .map(|k| pattern((n * BYTES + k) as u64))
                .collect();
            tcp.write_all(&sent).await.expect("write");
            let mut back = vec![0; BYTES];
            within("the bytes to come back", tcp.read_exact(&mut back))
                .await
                .expect("read");
            assert_eq!(back, sent, "connection {n} gets its own bytes back");
            handles.extend(
                rig.tun.inner.shards[0]
                    .state()
                    .conns
                    .values()
                    .map(|conn| conn.handle),
            );
            tcp.shutdown().await.expect("shutdown");
            let mut more = [0; 1];
            let end = within("the end of the stream", tcp.read(&mut more))
                .await
                .expect("the end, not an error");
            assert_eq!(end, 0, "connection {n}");
        }
    };
    timeout(Duration::from_secs(120), all)
        .await
        .expect("two thousand connections within 120 s");
    eventually("the TUN to let every connection go", WITHIN, || {
        rig.tun.sockets_in_use() == 0
            && rig.tun.sockets_counted() == 0
            && rig.tun.half_open() == 0
            && rig.tun.budget_in_use() == 0
            && rig.tun.records() == Records::default()
    })
    .await;
    assert_eq!(rig.dialer.dialed().len(), CONNECTIONS);
    eventually("every relay to end", WITHIN, || {
        rig.dialer.relay_ends().len() == CONNECTIONS
    })
    .await;
    assert!(
        rig.dialer.relay_ends().iter().all(|end| end.is_ok()),
        "every relay ended well"
    );
    // the premise: the handles of the sockets were used again
    assert!(
        handles.len() < CONNECTIONS / 10,
        "{} handles for {CONNECTIONS} connections",
        handles.len()
    );
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// A connection whose stream is dropped while the app's FIN, the TUN's own and
/// the acknowledgement of it are what is left to go: from the TUN's FIN on
/// nothing it sends reaches the app, so the FIN is never acknowledged, and the
/// connection stays in LAST-ACK (`timeout` and `keep_alive` are far longer than
/// anything here) until the `linger` that follows the drop runs out. It goes
/// then, with a reset that the app does not see either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_stream_lingers_then_goes() {
    let rig = rig(TunOptions {
        linger: Duration::from_millis(300),
        ..TunOptions::default()
    });
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let swallowing = Arc::new(AtomicBool::new(false));
    let seen = swallowing.clone();
    rig.app.set_filter(move |direction, packet| {
        if direction == Direction::FromTun && is_fin(packet) {
            seen.store(true, Ordering::SeqCst);
        }
        direction != Direction::FromTun || !seen.load(Ordering::SeqCst)
    });
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    // the app's FIN: the relay passes it on, the echo server closes its own
    // side, and the relay shuts the stream down and drops it
    tcp.shutdown().await.expect("shutdown");
    eventually("the relay to end", WITHIN, || {
        rig.dialer.relay_ends().len() == 1
    })
    .await;
    // the FIN goes out a moment after the stream has shut down and is gone
    eventually("the TUN's FIN to be swallowed", WITHIN, || {
        swallowing.load(Ordering::SeqCst)
    })
    .await;
    eventually(
        "the TUN to let the connection go",
        Duration::from_secs(5),
        || rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0,
    )
    .await;
    assert_eq!(rig.dialer.relay_ends(), vec![Ok(())]);
    assert_eq!(rig.tun.sockets_counted(), 0);
    assert_eq!(rig.tun.records(), Records::default());
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// The app's FIN is the end of the stream for the relay: the echo server reads
/// to the end and closes its side, and the relay ends well. The app's reset is
/// an error for it: the relay ends with it and the upstream connection is
/// closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fin_and_reset_reach_the_relay_as_eof_and_error() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));

    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    tcp.write_all(b"bye").await.expect("write");
    tcp.shutdown().await.expect("shutdown");
    let mut back = [0; 3];
    within("bye to come back", tcp.read_exact(&mut back))
        .await
        .expect("read");
    assert_eq!(&back, b"bye");
    // the echo server read to the end, or it would not have closed its side
    let end = within("the end of the stream", tcp.read(&mut back))
        .await
        .expect("the end, not an error");
    assert_eq!(end, 0);
    eventually("the relay to end", WITHIN, || {
        rig.dialer.relay_ends().len() == 1
    })
    .await;
    assert_eq!(rig.dialer.relay_ends(), vec![Ok(())]);
    assert_eq!(echo.received(), 3);

    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    tcp.abort();
    eventually("the relay to end", WITHIN, || {
        rig.dialer.relay_ends().len() == 2
    })
    .await;
    let ends = rig.dialer.relay_ends();
    assert!(
        matches!(&ends[1], Err(text) if text.contains("reset")),
        "the relay ends with the reset: {ends:?}"
    );
    eventually("the echo server's connection to end", WITHIN, || {
        echo.open() == 0
    })
    .await;
    eventually("the TUN to let the connections go", WITHIN, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0
    })
    .await;
    assert_eq!(rig.tun.counters(), Counters::default());
}
