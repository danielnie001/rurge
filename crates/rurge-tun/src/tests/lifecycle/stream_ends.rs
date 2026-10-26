//! What a stream answers, one shard: its reads, writes and shutdown as the
//! connection ends in each of its ways, and what `closed` waits for.

use super::{ByHand, FIN_ACK, RST_ACK, answers_reset, state_of, turn};
use crate::TunOptions;
use crate::tcp::shard::Records;
use crate::tcp::{Outcome, TunStream};
use crate::tests::hazards::{ACK, PSH_ACK};
use crate::tests::support::{
    addr, ingress, is_reset, is_syn, lone_shard, segment_v4, syn_v4, tcp_header, within,
};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
    let mut turn = shard
        .state()
        .turn(at(0), &mut ingress(vec![syn_v4(app, dst)]));
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
    shard.state().turn(at(1), &mut ingress(vec![ack]));
    let mut stream = TunStream::new(shard.clone(), opened.id, opened.shared);
    within("the handshake", stream.established())
        .await
        .expect("the connection is established");
    // past the timeout, before the shard's next look at every connection
    let turn = shard.state().turn(at(200), &mut ingress(Vec::new()));
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
    shard.state().turn(at(1_100), &mut ingress(Vec::new()));
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

/// The end of what the app sent is the end of the stream, however often it is
/// asked; the app having closed its side leaves the stream its own to write on
/// until it shuts it down, and then a write is a broken pipe. All of it stays
/// so once the connection has ended well and the shard has let it go, which it
/// does at once even if the stream has not read what the app sent: the stream
/// reads that first. When the app resets the connection, or the shard lets it
/// go for another reason (the stop, a rebuild), every read, write and shutdown
/// is a reset (P12).
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
    // the shard lets it go at once and keeps the bytes for the stream to read,
    // and a write is a broken pipe, not a reset, before and after the read
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
        None,
        "the shard lets the connection go, though the stream has not read what the app sent"
    );
    assert_eq!(app.shared.outcome(), Some(Outcome::Closed));
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
    let error = within("a write", stream.write_all(b"y"))
        .await
        .expect_err("the connection is closed for writing");
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

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

/// A waker that notes it was woken.
struct Woken(AtomicBool);

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// `closed` waits for the shard to let the connection go, is woken when it
/// does and answers how the connection ended, whether or not the stream has
/// read what the app sent: the bytes the shard kept are still there to read.
#[tokio::test]
async fn closed_waits_for_the_shard_to_let_the_connection_go() {
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let shard = &lone.shard;
    let woken = Arc::new(Woken(AtomicBool::new(false)));
    let waker = Waker::from(woken.clone());
    let mut cx = Context::from_waker(&waker);

    // the app resets the connection
    let (app, stream) = ByHand::make(shard, epoch, 40000, 0).await;
    let mut closed = Box::pin(stream.closed());
    assert_eq!(closed.as_mut().poll(&mut cx), Poll::Pending);
    assert!(!woken.0.load(Ordering::SeqCst), "nothing has ended yet");
    turn(shard, at(10), vec![app.segment(2, 0, RST_ACK, &[])]);
    assert!(
        woken.0.swap(false, Ordering::SeqCst),
        "the shard wakes the task that waits for the end"
    );
    assert_eq!(closed.as_mut().poll(&mut cx), Poll::Ready(Outcome::Reset));
    drop(closed);

    // the connection ends well, with the bytes the app sent not read: it is
    // over for `closed`, which does not wait for them
    let (app, mut stream) = ByHand::make(shard, epoch, 40001, 100).await;
    turn(shard, at(110), vec![app.segment(2, 0, PSH_ACK, b"abc")]);
    turn(shard, at(111), vec![app.segment(5, 0, FIN_ACK, &[])]);
    within("the shutdown", stream.shutdown())
        .await
        .expect("the shutdown");
    turn(shard, at(120), vec![]);
    let mut closed = Box::pin(stream.closed());
    assert_eq!(
        closed.as_mut().poll(&mut cx),
        Poll::Pending,
        "the app has not acknowledged the TUN's FIN"
    );
    turn(shard, at(130), vec![app.segment(6, 1, ACK, &[])]);
    assert!(
        woken.0.swap(false, Ordering::SeqCst),
        "the shard wakes the task that waits for the end"
    );
    assert_eq!(closed.as_mut().poll(&mut cx), Poll::Ready(Outcome::Closed));
    drop(closed);
    let mut got = [0; 3];
    within("the bytes before the FIN", stream.read_exact(&mut got))
        .await
        .expect("the bytes are read");
    assert_eq!(&got, b"abc");
}
