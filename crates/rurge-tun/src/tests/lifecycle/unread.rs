//! The bytes the app sent that the stream has not read when the shard lets the
//! connection go: the shard keeps them for the stream to read, and the tuple is
//! free at once.

use super::{ByHand, FIN_ACK, RST_ACK, payloads, state_of, turn};
use crate::TunOptions;
use crate::tcp::shard::Records;
use crate::tcp::{Outcome, Shard, TunStream};
use crate::testing::pattern;
use crate::tests::hazards::{ACK, PSH_ACK, open_by_hand};
use crate::tests::support::{
    ingress, is_fin, is_reset, lone_shard, read_pattern, read_pattern_from, syn_v4, within,
};
use smoltcp::socket::tcp;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What the stream and the app do on a connection made by hand at 0 ms to leave
/// three bytes unread in a socket in TIME-WAIT: the stream shuts its side down
/// and reads nothing, while the app acknowledges the FIN, sends `abc` and then
/// its own FIN. A turn 30 ms after the app's FIN is past the grace TIME-WAIT is
/// given, and the shard lets the connection go with the bytes unread, which the
/// stream is left to read.
async fn let_go_with_bytes_unread(
    shard: &Arc<Shard>,
    epoch: Instant,
    app: &ByHand,
    stream: &mut TunStream,
) {
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    within("the shutdown", stream.shutdown())
        .await
        .expect("the shutdown");
    let out = turn(shard, at(10), vec![]);
    assert!(out.iter().any(|packet| is_fin(packet)), "the FIN is out");
    turn(shard, at(11), vec![app.segment(2, 1, ACK, &[])]);
    turn(shard, at(20), vec![app.segment(2, 1, PSH_ACK, b"abc")]);
    turn(shard, at(21), vec![app.segment(5, 1, FIN_ACK, &[])]);
    assert_eq!(
        state_of(shard, app.id),
        Some(tcp::State::TimeWait),
        "the app's FIN is the last segment of the connection"
    );
    turn(shard, at(51), vec![]);
    assert_eq!(
        state_of(shard, app.id),
        None,
        "the shard lets the connection go, though the stream has not read what the app sent"
    );
    assert_eq!(app.shared.outcome(), Some(Outcome::Closed));
}

/// What the app sent before its FIN, and the stream has not read, is the
/// stream's to read however long ago the shard let the connection go. The shard
/// lets the socket, its tuple and the guard's edge go once TIME-WAIT has had its
/// grace; smoltcp would hold the socket for ten seconds and then clear its
/// receive buffer, which the stream would read as a reset of bytes the app saw
/// acknowledged. The receive buffer's share of the budget stays with the bytes
/// until they are read.
#[tokio::test]
async fn unread_bytes_outlive_the_time_wait() {
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let shard = &lone.shard;
    let (app, mut stream) = ByHand::make(shard, epoch, 40000, 0).await;
    let_go_with_bytes_unread(shard, epoch, &app, &mut stream).await;
    assert_eq!(shard.state().socket_count(), 0);
    assert_eq!(shard.state().records(), Records::default());
    assert_eq!(lone.admission.sockets(), 0);
    assert!(
        lone.budget.used() > 0,
        "the receive buffer's share of the budget stays with the bytes"
    );
    // past the ten seconds after which smoltcp would have closed the socket
    turn(shard, at(10_100), vec![]);
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
    assert_eq!(
        lone.budget.used(),
        0,
        "the share goes back when the bytes are read"
    );
    let again = shard
        .state()
        .turn(at(10_200), &mut ingress(vec![syn_v4(app.app, app.dst)]));
    assert_eq!(
        again.opened.len(),
        1,
        "a new SYN of the same tuple is a new connection"
    );
}

/// A stream that is dropped with the bytes it has not read gives back what the
/// shard kept for it: the receive buffer's share of the budget.
#[tokio::test]
async fn a_dropped_stream_gives_back_what_it_kept() {
    let epoch = Instant::now();
    let lone = lone_shard(&TunOptions::default(), epoch);
    let (app, mut stream) = ByHand::make(&lone.shard, epoch, 40000, 0).await;
    let_go_with_bytes_unread(&lone.shard, epoch, &app, &mut stream).await;
    assert!(
        lone.budget.used() > 0,
        "the receive buffer's share of the budget stays with the bytes"
    );
    drop(stream);
    assert_eq!(lone.budget.used(), 0);
    assert_eq!(lone.admission.sockets(), 0);
}

/// A connection that smoltcp itself closes (nothing from the app for the
/// timeout: a reset to the app, with no segment of the app's to say so) with
/// bytes the stream has not read: the stream reads the bytes and then the reset,
/// not the end of the stream. The bytes are kept, but how the connection ended
/// is not changed: no FIN came, so it is no clean end.
#[tokio::test]
async fn bytes_unread_at_the_timeout_are_read_and_then_a_reset() {
    let options = TunOptions {
        timeout: Duration::from_millis(100),
        ..TunOptions::default()
    };
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&options, epoch);
    let shard = &lone.shard;
    let (app, mut stream) = ByHand::make(shard, epoch, 40000, 0).await;
    // three bytes and no FIN, which the stream does not read
    turn(shard, at(10), vec![app.segment(2, 0, PSH_ACK, b"abc")]);
    // past the timeout, before the shard's next look at every connection
    let out = turn(shard, at(200), vec![]);
    assert!(
        out.iter().any(|packet| is_reset(packet)),
        "smoltcp's timeout resets the connection"
    );
    assert_eq!(
        state_of(shard, app.id),
        Some(tcp::State::Closed),
        "the shard has not looked at the connection yet"
    );
    // the next look at every connection
    turn(shard, at(1_100), vec![]);
    assert_eq!(
        state_of(shard, app.id),
        None,
        "the shard lets the connection go, though the stream has not read what the app sent"
    );
    assert_eq!(app.shared.outcome(), Some(Outcome::Reset));
    let mut got = [0; 3];
    within("the bytes before the reset", stream.read_exact(&mut got))
        .await
        .expect("the bytes are read");
    assert_eq!(&got, b"abc");
    let error = within("the reset", stream.read(&mut got))
        .await
        .expect_err("the bytes are followed by the reset");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert_eq!(lone.budget.used(), 0);
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(shard.state().records(), Records::default());
}

/// What the app sent before it reset the connection is not read: nothing is kept
/// for the stream, which answers the reset at once, as it did when the shard
/// kept a connection for its stream to read.
#[tokio::test]
async fn bytes_unread_when_the_app_resets_are_not_kept() {
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let shard = &lone.shard;
    let (app, mut stream) = ByHand::make(shard, epoch, 40000, 0).await;
    turn(shard, at(10), vec![app.segment(2, 0, PSH_ACK, b"abc")]);
    turn(shard, at(11), vec![app.segment(5, 0, RST_ACK, &[])]);
    assert_eq!(
        state_of(shard, app.id),
        None,
        "the shard lets a reset connection go at once"
    );
    assert_eq!(app.shared.outcome(), Some(Outcome::Reset));
    assert_eq!(lone.budget.used(), 0, "no share of the budget is kept");
    let error = within("the read", stream.read(&mut [0; 3]))
        .await
        .expect_err("the reset, not the bytes before it");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert_eq!(lone.admission.sockets(), 0);
    assert_eq!(shard.state().records(), Records::default());
}

/// The app sends bytes `from..to` of the pattern, for the stream to read back
/// against it: segments of a thousand bytes at most, each numbered from the
/// start of the app's stream (the app's first byte is sequence number 2, so byte
/// `n` of the pattern is number `2 + n`), and a turn of the shard after each, so
/// that the socket has taken it into its receive buffer. The turns are 20 ms
/// apart; `now` and the result are milliseconds from `epoch`, the first turn is
/// 20 ms after `now` and the result is the time of the last, to go on from.
fn send_pattern(
    shard: &Shard,
    epoch: Instant,
    app: &ByHand,
    mut now: u64,
    from: usize,
    to: usize,
) -> u64 {
    for start in (from..to).step_by(1000) {
        let piece: Vec<u8> = (start..to.min(start + 1000))
            .map(|i| pattern(i as u64))
            .collect();
        now += 20;
        let at = epoch + Duration::from_millis(now);
        turn(
            shard,
            at,
            vec![app.segment(2 + start as u32, 0, PSH_ACK, &piece)],
        );
    }
    now
}

/// The bytes the stream has not read can lie on both sides of the end of the
/// socket's receive buffer, which is a ring: 60,000 bytes come in, the stream
/// reads 50,000 of them, and 30,000 more come, so the 40,000 left are the last
/// 15,535 bytes of the buffer and the first 24,465. The shard keeps all of
/// them, in order.
#[tokio::test]
async fn unread_bytes_across_the_end_of_the_receive_buffer_are_kept_in_order() {
    const FIRST: usize = 60_000;
    const READ: usize = 50_000;
    const SECOND: usize = 30_000;
    const TOTAL: usize = FIRST + SECOND;
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let shard = &lone.shard;
    let (app, mut stream) = ByHand::make(shard, epoch, 40000, 0).await;
    let now = send_pattern(shard, epoch, &app, 10, 0, FIRST);
    within("the first bytes", read_pattern(&mut stream, READ))
        .await
        .expect("the bytes are read");
    let now = send_pattern(shard, epoch, &app, now, FIRST, TOTAL);
    {
        let state = shard.state();
        let socket = state
            .sockets
            .get::<tcp::Socket>(state.conns[&app.id].handle);
        assert_eq!(socket.recv_queue(), TOTAL - READ);
        assert!(
            socket.recv_capacity() < TOTAL,
            "the bytes cannot all have been in the buffer from its start"
        );
    }
    // the stream's FIN and the app's acknowledgement of it, then the app's FIN
    within("the shutdown", stream.shutdown())
        .await
        .expect("the shutdown");
    turn(shard, at(now + 10), vec![]);
    let fin_seq = 2 + TOTAL as u32;
    turn(shard, at(now + 11), vec![app.segment(fin_seq, 1, ACK, &[])]);
    turn(
        shard,
        at(now + 12),
        vec![app.segment(fin_seq, 1, FIN_ACK, &[])],
    );
    assert_eq!(
        state_of(shard, app.id),
        Some(tcp::State::TimeWait),
        "the app's FIN is the last segment of the connection"
    );
    turn(shard, at(now + 42), vec![]);
    assert_eq!(state_of(shard, app.id), None);
    assert_eq!(app.shared.outcome(), Some(Outcome::Closed));
    within(
        "the bytes the shard kept",
        read_pattern_from(&mut stream, READ, TOTAL - READ),
    )
    .await
    .expect("the bytes are read, every one the pattern's");
    let end = within("the end of the stream", stream.read(&mut [0; 1]))
        .await
        .expect("the end of the stream is no error");
    assert_eq!(end, 0);
    assert_eq!(lone.budget.used(), 0);
}

/// The tuple is free the moment the shard lets the connection go, not when the
/// stream has read what the shard kept: the app connects again from the same
/// port at once, and the new connection has the socket handle the old one had
/// (smoltcp gives the lowest free one again). The old stream reads its own bytes
/// and then the end, and never the new connection's; what it writes and shuts
/// down reaches nothing of the new connection.
#[tokio::test]
async fn a_new_connection_of_the_same_tuple_is_not_read_by_the_old_stream() {
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let shard = &lone.shard;
    let (old, mut old_stream) = ByHand::make(shard, epoch, 40000, 0).await;
    let old_handle = shard.state().conns[&old.id].handle;
    let_go_with_bytes_unread(shard, epoch, &old, &mut old_stream).await;
    // the app connects again from the same port and sends three bytes of its own
    let (opened, isn) = open_by_hand(shard, epoch, old.app, old.dst, 100);
    let new = ByHand {
        id: opened.id,
        shared: opened.shared.clone(),
        app: old.app,
        dst: old.dst,
        isn,
    };
    let mut new_stream = TunStream::new(shard.clone(), opened.id, opened.shared);
    within("the handshake", new_stream.established())
        .await
        .expect("the new connection is established");
    assert_ne!(new.id, old.id);
    assert_eq!(
        shard.state().conns[&new.id].handle,
        old_handle,
        "the new connection has the handle the old one had"
    );
    turn(shard, at(110), vec![new.segment(2, 0, PSH_ACK, b"new")]);
    let mut got = [0; 3];
    within("the old bytes", old_stream.read_exact(&mut got))
        .await
        .expect("the old bytes are read");
    assert_eq!(
        &got, b"abc",
        "the old stream reads the old connection's bytes"
    );
    let end = within("the end of the old stream", old_stream.read(&mut got))
        .await
        .expect("the end of the stream is no error");
    assert_eq!(end, 0, "and then its end, not the new connection's bytes");
    let error = within("a write", old_stream.write_all(b"old"))
        .await
        .expect_err("the old connection is closed for writing");
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    within("the shutdown", old_stream.shutdown())
        .await
        .expect("a shutdown of a connection that ended well is no error");
    let out = turn(shard, at(120), vec![]);
    assert!(
        payloads(&out).is_empty() && !out.iter().any(|packet| is_fin(packet)),
        "nothing of the old stream reached the new connection"
    );
    assert_eq!(state_of(shard, new.id), Some(tcp::State::Established));
    within("the new bytes", new_stream.read_exact(&mut got))
        .await
        .expect("the new bytes are read");
    assert_eq!(&got, b"new", "the new connection's bytes are its own");
}
