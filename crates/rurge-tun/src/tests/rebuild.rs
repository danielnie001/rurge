//! What becomes of a shard whose stack panics, or whose lock a panic poisoned
//! (phase 3 M3 design 5.3, "the last line"): the shard is rebuilt, its
//! connections are reset, what they held is given back, and the new stack has
//! a seed of its own. First one shard, its turns taken by hand, with what is a
//! function of its input alone (the seed, the text of a panic); then the whole
//! TUN, where a segment panics the stack, a stream panics with the lock held,
//! or a SYN panics the stack as it is taken.

use super::hazards::{ACK, PSH_ACK, app4, fields, open_by_hand, watch};
use super::log_capture::LogCapture;
use super::support::{
    WITHIN, addr, data_segment_v4, eventually, hello, is_syn, lone_shard, rig, segment_v4, syn_v4,
    tcp_packet, tcp_repr, within,
};
use crate::tcp::{Outcome, PANIC_MARKER, Shard};
use crate::testing::{Script, Upstream};
use crate::{Counters, TunOptions};
use smoltcp::wire::{TcpControl, TcpRepr, TcpSeqNumber};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

// ---- a stack that panics

/// The WARN a rebuild says: with the number of the shard and nothing else.
const REBUILT: &str =
    "WARN tun: the TCP stack of a shard panicked; its connections are reset shard=0";

/// Turns `shard` by hand at `at` with `incoming`, and whether the turn
/// panicked.
fn panics(shard: &Shard, at: Instant, incoming: Vec<Vec<u8>>) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        shard.state().turn(at, &mut { incoming });
    }))
    .is_err()
}

#[test]
fn a_shard_that_panicked_is_rebuilt_by_the_next_to_take_its_lock() {
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let dst = addr("192.0.2.1:80");
    // one connection that is made, one that is half-open
    let (first, _) = open_by_hand(&lone.shard, epoch, app4(40000), dst, 0);
    let mut turn = lone
        .shard
        .state()
        .turn(at(5), &mut vec![syn_v4(app4(40001), dst)]);
    let second = turn
        .opened
        .pop()
        .expect("the second SYN opens a connection");
    assert_eq!(lone.admission.sockets(), 2);
    assert_eq!(lone.admission.half_open(), 1);
    assert!(lone.budget.used() > 0);
    assert_eq!(lone.shard.state().edge_count(), 2);
    // a segment that panics the stack
    let marker = data_segment_v4(app4(40000), dst, 2, 0, PSH_ACK, PANIC_MARKER);
    assert!(panics(&lone.shard, at(10), vec![marker]));
    assert_eq!(
        lone.stats.counters(),
        Counters::default(),
        "nothing is counted until somebody has the lock"
    );
    // whoever takes the lock next rebuilds the shard
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.shard.state().edge_count(), 0);
    assert_eq!(lone.admission.sockets(), 0, "the sockets counted");
    assert_eq!(lone.admission.half_open(), 0, "the half-open counted");
    assert_eq!(lone.budget.used(), 0, "the buffers taken from the budget");
    assert_eq!(first.shared.outcome(), Some(Outcome::Reset));
    assert_eq!(second.shared.outcome(), Some(Outcome::Reset));
    assert_eq!(lone.stats.counters().stack_panics, 1);
    // once: the lock is not poisoned any more
    drop(lone.shard.state());
    assert_eq!(lone.stats.counters().stack_panics, 1);
    // and the shard takes a connection as it did
    open_by_hand(&lone.shard, epoch, app4(40002), dst, 20);
    assert_eq!(lone.shard.state().socket_count(), 1);
    assert_eq!(lone.admission.sockets(), 1);
    assert_eq!(lone.admission.half_open(), 0);
}

/// The cause of the panic can be the shard's own books. A connection whose
/// socket is gone from the stack behind the shard's back is let go of, and the
/// call that takes the socket out of the stack panics; what the connection held
/// is given back before that call, or the rebuild, which looks for the
/// connections in the shard's table, would not find it to give it back.
#[test]
fn a_panic_in_letting_a_connection_go_gives_back_what_it_held() {
    let epoch = Instant::now();
    let lone = lone_shard(&TunOptions::default(), epoch);
    let (first, _) = open_by_hand(&lone.shard, epoch, app4(40000), addr("192.0.2.1:80"), 0);
    {
        let mut state = lone.shard.state();
        let handle = state.conns[&first.id].handle;
        let _ = state.sockets.remove(handle);
    }
    assert_eq!(lone.admission.sockets(), 1);
    assert!(lone.budget.used() > 0);
    assert!(
        catch_unwind(AssertUnwindSafe(|| lone.shard.state().end_all())).is_err(),
        "the socket is not there to be taken out"
    );
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.admission.sockets(), 0, "the sockets counted");
    assert_eq!(lone.budget.used(), 0, "the buffers taken from the budget");
    assert_eq!(first.shared.outcome(), Some(Outcome::Reset));
    assert_eq!(lone.stats.counters().stack_panics, 1);
}

/// The rebuilt stack must not start where the old one did: the same seed
/// gives the same initial sequence numbers in the same order.
#[test]
fn a_rebuilt_shard_does_not_repeat_its_initial_sequence_numbers() {
    let epoch = Instant::now();
    let lone = lone_shard(&TunOptions::default(), epoch);
    let dst = addr("192.0.2.1:80");
    let (_, before) = open_by_hand(&lone.shard, epoch, app4(40000), dst, 0);
    let marker = data_segment_v4(app4(40000), dst, 2, 0, PSH_ACK, PANIC_MARKER);
    assert!(panics(
        &lone.shard,
        epoch + Duration::from_millis(5),
        vec![marker]
    ));
    let (_, after) = open_by_hand(&lone.shard, epoch, app4(40000), dst, 10);
    assert_ne!(before, after, "the first connection of the new stack");
}

#[test]
fn a_seed_is_drawn_again_and_never_the_old_one() {
    use crate::tcp::rebuild::{SEED_STEP, next_seed};
    assert_eq!(next_seed(7, Some(9)), 9);
    // the source failed: the old seed and a fixed odd step, which is never the
    // old seed (an odd step is no multiple of 2^64) and comes round to every
    // seed before it comes round to one it had
    assert_eq!(SEED_STEP % 2, 1);
    assert_eq!(next_seed(7, None), 7u64.wrapping_add(SEED_STEP));
    assert_eq!(next_seed(u64::MAX, None), SEED_STEP - 1);
    let mut seed = 0;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..1000 {
        seed = next_seed(seed, None);
        assert!(seen.insert(seed), "a seed came twice");
    }
    assert!(!seen.contains(&0));
}

/// What a panic made of its arguments is text, a `&str` or a `String`; the
/// payload of any other panic has none, and the DEBUG line says so.
#[test]
fn the_text_of_a_panic_is_said_when_it_has_one() {
    use crate::tcp::rebuild::{Cause, panic_text};
    use std::any::Any;
    let literal: Box<dyn Any + Send> = Box::new("a literal");
    let formatted: Box<dyn Any + Send> = Box::new(String::from("a formatted text"));
    let other: Box<dyn Any + Send> = Box::new(42u32);
    assert_eq!(panic_text(&*literal), Some("a literal"));
    assert_eq!(panic_text(&*formatted), Some("a formatted text"));
    assert_eq!(panic_text(&*other), None);
    assert_eq!(
        Cause::Panic(panic_text(&*literal)).to_string(),
        "panic: a literal"
    );
    assert_eq!(
        Cause::Panic(panic_text(&*other)).to_string(),
        "panic with no text"
    );
    assert!(Cause::Poisoned.to_string().contains("poisoned"));
}

/// `TunOptions` is public: a time to complete the handshake that cannot be
/// added to a time is no reason for a panic (and a panic that came with every
/// SYN would be a rebuild with every SYN).
#[test]
fn a_half_open_timeout_too_long_to_add_does_not_panic() {
    let options = TunOptions {
        half_open_timeout: Duration::MAX,
        ..TunOptions::default()
    };
    let epoch = Instant::now();
    let lone = lone_shard(&options, epoch);
    let (app, dst) = (app4(40000), addr("192.0.2.1:80"));
    let turn = lone.shard.state().turn(epoch, &mut vec![syn_v4(app, dst)]);
    assert_eq!(turn.opened.len(), 1);
    assert_eq!(lone.admission.half_open(), 1);
    // the handshake still completes, whenever its time would have been
    let isn = turn
        .out
        .iter()
        .find(|packet| is_syn(packet))
        .and_then(|packet| fields(packet))
        .expect("the SYN-ACK")
        .seq;
    lone.shard.state().turn(
        epoch + Duration::from_millis(1),
        &mut vec![segment_v4(app, dst, 2, isn.wrapping_add(1), ACK)],
    );
    assert_eq!(lone.admission.half_open(), 0);
    assert_eq!(lone.admission.sockets(), 1);
}

#[test]
fn a_poisoned_limit_on_resets_still_lets_resets_through() {
    let lone = lone_shard(&TunOptions::default(), Instant::now());
    lone.admission.poison_the_limit_on_resets();
    assert!(lone.admission.allow_reset(Instant::now()));
}

/// A segment of a connection of A's with the marker in it panics the stack
/// the shard has. It is rebuilt: A's connection is reset, and with it every
/// other one of the shard's, and a new connection is as good as the first.
/// The WARN says the shard's number and nothing of the panic, which is in a
/// DEBUG line.
#[tokio::test]
async fn a_panicking_shard_resets_its_connections_and_recovers() {
    let log = LogCapture::start();
    let rig = rig(TunOptions {
        shards: 1,
        ..TunOptions::default()
    });
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let heard = watch(&rig.app);
    let mut a = within("A to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut a).await;
    // sent by hand: the app's stack knows nothing of it, and does not send it
    // again as it would a segment that nobody acknowledged. It begins where
    // the TUN is waiting for the next byte, inside the window, or the guard at
    // the edge of the window would have it before the stack does.
    let told = *heard.lock().expect("what the TUN told");
    rig.app.inject(data_segment_v4(
        a.local(),
        dst,
        told.ack,
        told.seq,
        PSH_ACK,
        PANIC_MARKER,
    ));
    eventually("the shard to be rebuilt", WITHIN, || {
        rig.tun.counters().stack_panics == 1
    })
    .await;
    eventually("A's relay to end", WITHIN, || {
        rig.dialer.relay_ends().len() == 1
    })
    .await;
    let ends = rig.dialer.relay_ends();
    assert!(
        matches!(&ends[0], Err(text) if text.contains("reset")),
        "A's session ends with a reset: {ends:?}"
    );
    eventually("the sockets to be given back", WITHIN, || {
        rig.tun.sockets_counted() == 0
    })
    .await;
    assert_eq!(rig.tun.budget_in_use(), 0);
    // a new connection through the new stack
    let mut b = within("B to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut b).await;
    assert_eq!(rig.tun.sockets_counted(), 1);
    assert_eq!(rig.tun.half_open(), 0);
    assert_eq!(
        rig.tun.counters(),
        Counters {
            stack_panics: 1,
            ..Counters::default()
        }
    );
    let lines = log.lines();
    let warnings: Vec<&String> = lines
        .iter()
        .filter(|line| line.starts_with("WARN"))
        .collect();
    assert_eq!(warnings, [REBUILT], "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("DEBUG") && line.contains("rurge-tun test: panic")),
        "the panic is said at DEBUG: {lines:?}"
    );
}

/// The upstream of A sends the marker, and the relay panics writing it to A's
/// stream with the shard's lock held, which poisons the lock. The next to take
/// the lock rebuilds the shard, and this is A's stream, dropped with its
/// task: every other connection of the shard is reset (B's relay ends), and
/// a new one is made.
#[tokio::test]
async fn a_poisoned_shard_is_rebuilt() {
    let log = LogCapture::start();
    let rig = rig(TunOptions {
        shards: 1,
        ..TunOptions::default()
    });
    let echo = Upstream::echo().await;
    let (a_dst, b_dst) = (addr("192.0.2.1:80"), addr("192.0.2.2:80"));
    rig.dialer.script(b_dst, Script::Upstream(echo.addr()));
    let mut b = within("B to be made", rig.app.connect(b_dst))
        .await
        .expect("connect");
    hello(&mut b).await;
    // A's upstream is a server of the test's, which says the marker at once
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    rig.dialer.script(
        a_dst,
        Script::Upstream(listener.local_addr().expect("address")),
    );
    let serving = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        stream.write_all(PANIC_MARKER).await?;
        // kept open until the test is over
        std::future::pending::<()>().await;
        io::Result::Ok(())
    });
    let _a = within("A to be made", rig.app.connect(a_dst))
        .await
        .expect("connect");
    eventually("the shard to be rebuilt", WITHIN, || {
        rig.tun.counters().stack_panics == 1
    })
    .await;
    eventually("B's relay to end", WITHIN, || {
        rig.dialer.relay_ends().len() == 1
    })
    .await;
    let ends = rig.dialer.relay_ends();
    assert!(
        matches!(&ends[0], Err(text) if text.contains("reset")),
        "B's session ends with a reset: {ends:?}"
    );
    let mut c = within("C to be made", rig.app.connect(b_dst))
        .await
        .expect("connect");
    hello(&mut c).await;
    assert_eq!(rig.tun.sockets_counted(), 1, "C alone");
    assert_eq!(
        rig.tun.counters(),
        Counters {
            stack_panics: 1,
            ..Counters::default()
        }
    );
    let lines = log.lines();
    let warnings: Vec<&String> = lines
        .iter()
        .filter(|line| line.starts_with("WARN"))
        .collect();
    assert_eq!(warnings, [REBUILT], "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("DEBUG") && line.contains("poisoned")),
        "a lock found poisoned is said at DEBUG: {lines:?}"
    );
    serving.abort();
}

/// A SYN that carries the marker panics the stack when it takes the SYN:
/// the shard has claimed the places in the TUN's two caps and taken the
/// buffers' share of the budget by then, and has no connection to show for
/// them. The rebuild gives them back, or the caps fill up for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_panic_while_a_syn_is_taken_gives_everything_back() {
    let rig = rig(TunOptions {
        shards: 1,
        ..TunOptions::default()
    });
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let app = app4(40000);
    let syn = TcpRepr {
        control: TcpControl::Syn,
        seq_number: TcpSeqNumber(1),
        ..tcp_repr(app, dst, PANIC_MARKER)
    };
    rig.app.inject(tcp_packet(app, dst, &syn));
    eventually("the shard to be rebuilt", WITHIN, || {
        rig.tun.counters().stack_panics == 1
    })
    .await;
    assert_eq!(rig.tun.budget_in_use(), 0, "the buffers' share");
    assert_eq!(
        rig.tun.sockets_counted(),
        0,
        "the places in the cap on sockets"
    );
    assert_eq!(rig.tun.half_open(), 0, "the places in the cap on half-open");
    assert_eq!(rig.tun.sockets_in_use(), 0);
    // the TUN takes a connection after it
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    assert_eq!(rig.tun.sockets_counted(), 1);
    assert_eq!(rig.tun.half_open(), 0);
    assert!(rig.tun.budget_in_use() > 0);
    assert_eq!(
        rig.tun.counters(),
        Counters {
            stack_panics: 1,
            ..Counters::default()
        }
    );
}
