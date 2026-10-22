//! What becomes of a TUN as its sessions are rejected, as it is told to stop and
//! as its device fails (phase 3 M3 design 5.6). First the rejections that are
//! left to the session: REJECT-DROP holds the connection and then closes it (and
//! lets it go when the connection ends first), the others reset it. Then the
//! stop: every connection is reset and the app is told, a relay stuck on its
//! upstream is cancelled, the resets reach the device even when the device is
//! slow, and a device that takes nothing cannot hold the stop up for longer than
//! a grace. Then a device that fails. The last tests run one shard's task by
//! hand, with channels of their own in place of the reader and the writer.

use super::hazards::{PSH_ACK, app4, open_by_hand};
use super::log_capture::LogCapture;
use super::support::{
    Lone, WITHIN, addr, data_segment_v4, eventually, hello, is_fin, is_reset, is_syn, lone_shard,
    rig, rig_with, syn_v4, tun_dialing, tun_on, tun_with, within,
};
use crate::device::{Device, MemoryDevice};
use crate::packet::segment;
use crate::session::Holds;
use crate::tcp::shard::{self, Records};
use crate::tcp::{Outcome, PANIC_MARKER, Tuple};
use crate::testing::{AppStack, Direction, FakeDialer, Script, Upstream};
use crate::writer::STOP_GRACE;
use crate::{Counters, Ending, Life, TunHandle, TunOptions};
use rurge_config::SessionInfo;
use rurge_inbound::{DialError, Dialed, Dialer, SessionHandle};
use rurge_net::BoxFuture;
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;
use tokio_util::task::TaskTracker;

const FIVE_SECONDS: Duration = Duration::from_secs(5);

/// What a stop is given to finish in beyond its grace: the tasks of the TUN
/// unwind in milliseconds, and this is for a machine that is busy.
const MARGIN: Duration = Duration::from_secs(3);

/// `future`, which has five seconds to finish; `what` is what it waits for.
async fn in_five_seconds<T>(what: &str, future: impl Future<Output = T>) -> T {
    timeout(FIVE_SECONDS, future)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// Counts the resets the TUN sends the app from now on. Every packet goes on.
fn count_resets(app: &AppStack) -> Arc<AtomicUsize> {
    let resets = Arc::new(AtomicUsize::new(0));
    let counting = resets.clone();
    app.set_filter(move |direction, packet| {
        if direction == Direction::FromTun && is_reset(packet) {
            counting.fetch_add(1, Ordering::SeqCst);
        }
        true
    });
    resets
}

/// Nothing is left of the TUN's connections: no socket, no count, no buffer,
/// no entry in any table of any shard.
fn assert_nothing_left(tun: &TunHandle) {
    assert_eq!(tun.sockets_in_use(), 0, "sockets");
    assert_eq!(tun.sockets_counted(), 0, "sockets counted against the cap");
    assert_eq!(tun.half_open(), 0, "half-open connections counted");
    assert_eq!(tun.budget_in_use(), 0, "bytes of buffers");
    assert_eq!(tun.records(), Records::default(), "the shards' tables");
}

/// The warnings among the lines the capture has, whole.
fn warnings(log: &LogCapture) -> Vec<String> {
    log.lines()
        .into_iter()
        .filter(|line| line.starts_with("WARN"))
        .collect()
}

// ---- what a session leaves of a connection it rejects

/// REJECT-DROP after the handshake: the connection is held for `drop_hold`,
/// neither read nor written, and then the TUN closes its side, a FIN, which the
/// app reads as the end of the stream. Nothing is sent in the meantime, and no
/// reset at any time; the app ends its own side and nothing is left.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reject_drop_holds_then_closes() {
    let hold = Duration::from_millis(300);
    let rig = rig(TunOptions {
        drop_hold: hold,
        ..TunOptions::default()
    });
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Reject(RejectKind::Drop));
    let resets = count_resets(&rig.app);
    // taken before the connect: the hold begins after the handshake
    let started = Instant::now();
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    let read = in_five_seconds("the end of the stream", tcp.read(&mut [0; 16]))
        .await
        .expect("the end of the stream, not a reset");
    assert_eq!(read, 0, "no data comes in the meantime, only the FIN");
    assert!(
        started.elapsed() >= hold,
        "the FIN came after {:?}, before the hold of {hold:?} was over",
        started.elapsed()
    );
    tcp.shutdown().await.expect("the app's FIN");
    eventually("the connection to go", FIVE_SECONDS, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.sockets_counted() == 0
    })
    .await;
    let again = in_five_seconds("the end of the stream", tcp.read(&mut [0; 16]))
        .await
        .expect("no reset after the FIN either");
    assert_eq!(again, 0);
    assert_eq!(resets.load(Ordering::SeqCst), 0, "the TUN reset nothing");
    assert!(rig.dialer.relay_ends().is_empty(), "nothing was relayed");
    assert_nothing_left(&rig.tun);
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// Every rejection but REJECT-DROP, and a dial that fails, let the stream go at
/// once: the connection is reset, whatever the hold is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_other_rejections_reset() {
    let rig = rig(TunOptions {
        drop_hold: Duration::from_secs(10),
        ..TunOptions::default()
    });
    let scripts = [
        Script::Reject(RejectKind::Reject),
        Script::Reject(RejectKind::NoDrop),
        Script::Reject(RejectKind::TinyGif),
        Script::Fail,
    ];
    for (n, script) in scripts.into_iter().enumerate() {
        let dst = addr(&format!("192.0.2.{}:80", n + 1));
        let what = format!("{script:?}");
        rig.dialer.script(dst, script);
        let mut tcp = within("the connection to be made", rig.app.connect(dst))
            .await
            .expect("connect");
        let error = timeout(Duration::from_secs(1), tcp.read(&mut [0; 1]))
            .await
            .unwrap_or_else(|_| panic!("{what}: no reset within a second"))
            .expect_err("the connection is reset");
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset, "{what}");
    }
    eventually("the TUN to let the connections go", FIVE_SECONDS, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.sockets_counted() == 0
    })
    .await;
    assert_nothing_left(&rig.tun);
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// The stop ends a hold at once: with a hold of 30 s, the TUN ends well within
/// 5 s of the stop, and the app is told.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_during_a_drop_hold_ends_it() {
    let rig = rig(TunOptions {
        drop_hold: Duration::from_secs(30),
        ..TunOptions::default()
    });
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Reject(RejectKind::Drop));
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    eventually("the session to be dialed", WITHIN, || {
        rig.dialer.dialed().len() == 1
    })
    .await;
    rig.tun.stop();
    let ending = in_five_seconds("the TUN to end", rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    let error = in_five_seconds("the reset", tcp.read(&mut [0; 1]))
        .await
        .expect_err("the stop resets the connection");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert_nothing_left(&rig.tun);
}

/// A hold ends with its connection: when the app resets the connection during
/// the hold the shard lets it go, and the session ends then, not when the hold
/// does (30 s here), with its boxed future and its stream held for all that
/// time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drop_hold_ends_with_its_connection() {
    let rig = rig(TunOptions {
        drop_hold: Duration::from_secs(30),
        ..TunOptions::default()
    });
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Reject(RejectKind::Drop));
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    // the session is there from the SYN on; once it is dialed it holds
    eventually("the session to be dialed and held", WITHIN, || {
        rig.dialer.dialed().len() == 1 && rig.tun.session_tasks() == 1
    })
    .await;
    tcp.abort();
    eventually(
        "the session to end with its connection",
        FIVE_SECONDS,
        || rig.tun.session_tasks() == 0,
    )
    .await;
    assert_nothing_left(&rig.tun);
}

/// After its FIN the TUN reads what the app still sends and throws it away,
/// until the app's own FIN: the app may send after it has read the FIN, and the
/// connection ends well. Were the stream let go at once after its `shutdown`,
/// the connection would be reset (it is dropped while the app may still send)
/// and the app would never read the FIN.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_connection_is_read_to_the_end_of_what_the_app_sends() {
    let rig = rig(TunOptions {
        drop_hold: Duration::from_millis(50),
        ..TunOptions::default()
    });
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Reject(RejectKind::Drop));
    let resets = count_resets(&rig.app);
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    let read = in_five_seconds("the end of the stream", tcp.read(&mut [0; 1]))
        .await
        .expect("the end of the stream, not a reset");
    assert_eq!(read, 0);
    // three thousand bytes, more than a segment's worth, then the app's FIN
    tcp.write_all(&[0x5a; 3000])
        .await
        .expect("the app writes after the TUN's FIN");
    tcp.shutdown().await.expect("the app's FIN");
    // the default linger is a minute: it is the bytes taken and the FIN that
    // let the connection go
    eventually("the connection to go", FIVE_SECONDS, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.sockets_counted() == 0
    })
    .await;
    assert_eq!(resets.load(Ordering::SeqCst), 0, "the TUN reset nothing");
    let again = in_five_seconds("the end of the stream", tcp.read(&mut [0; 1]))
        .await
        .expect("no reset");
    assert_eq!(again, 0);
    assert_nothing_left(&rig.tun);
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// The wait for the app's end is over when the app ends its side: the read that
/// throws away what the app sends stops at the end of the stream, and the
/// session ends then, not when `linger` (a minute, by default) is over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drop_hold_ends_when_the_app_ends_its_side() {
    let rig = rig(TunOptions {
        drop_hold: Duration::from_millis(20),
        ..TunOptions::default()
    });
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Reject(RejectKind::Drop));
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    let read = in_five_seconds("the end of the stream", tcp.read(&mut [0; 1]))
        .await
        .expect("the end of the stream, not a reset");
    assert_eq!(read, 0);
    tcp.shutdown().await.expect("the app's FIN");
    eventually("the session to end", FIVE_SECONDS, || {
        rig.tun.session_tasks() == 0
    })
    .await;
    eventually("the connection to go", FIVE_SECONDS, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.sockets_counted() == 0
    })
    .await;
    assert_nothing_left(&rig.tun);
}

/// An app that never ends its side is waited for `linger` and no longer: then
/// the stream is let go, which resets a connection the app may still send on.
/// Not before the hold and the linger are over, one after the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_connection_is_reset_when_the_app_does_not_end_it() {
    let (hold, linger) = (Duration::from_millis(20), Duration::from_millis(300));
    let rig = rig(TunOptions {
        drop_hold: hold,
        linger,
        ..TunOptions::default()
    });
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Reject(RejectKind::Drop));
    let resets = count_resets(&rig.app);
    let started = Instant::now();
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    let read = in_five_seconds("the end of the stream", tcp.read(&mut [0; 1]))
        .await
        .expect("the end of the stream, not a reset");
    assert_eq!(read, 0);
    eventually("the reset", FIVE_SECONDS, || {
        resets.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(
        started.elapsed() >= hold + linger,
        "reset after {:?}, before the hold and the linger ({:?}) were over",
        started.elapsed(),
        hold + linger
    );
    let error = in_five_seconds("the reset", tcp.read(&mut [0; 1]))
        .await
        .expect_err("the app is reset");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    eventually("the connection to go", FIVE_SECONDS, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.sockets_counted() == 0
    })
    .await;
    assert_nothing_left(&rig.tun);
}

/// The stop while the TUN waits for the app's end: the stop resets the
/// connection, the wait ends with the error, and so does the TUN.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_while_waiting_for_the_apps_end_ends_it() {
    let rig = rig(TunOptions {
        drop_hold: Duration::from_millis(20),
        ..TunOptions::default()
    });
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Reject(RejectKind::Drop));
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    let read = in_five_seconds("the end of the stream", tcp.read(&mut [0; 1]))
        .await
        .expect("the end of the stream, not a reset");
    assert_eq!(
        read, 0,
        "the TUN has closed its side and waits for the app's"
    );
    rig.tun.stop();
    let ending = in_five_seconds("the TUN to end", rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    assert_nothing_left(&rig.tun);
}

/// `TunOptions` is public: a hold or a linger too long to add to a time is no
/// reason for a panic. The session holds the connection as long as it can, and
/// the stop ends it: a hold that never ends, and a linger that never does while
/// the TUN waits for the app's end. What this pins is that the stop ends both;
/// a panic in the session task it cannot see, for `join` counts a task that
/// panicked as ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hold_or_a_linger_that_never_ends_ends_with_the_stop() {
    for (drop_hold, waiting_for_the_app) in
        [(Duration::MAX, false), (Duration::from_millis(20), true)]
    {
        let rig = rig(TunOptions {
            drop_hold,
            linger: Duration::MAX,
            ..TunOptions::default()
        });
        let dst = addr("192.0.2.1:80");
        rig.dialer.script(dst, Script::Reject(RejectKind::Drop));
        let mut tcp = within("the connection to be made", rig.app.connect(dst))
            .await
            .expect("connect");
        if waiting_for_the_app {
            let read = in_five_seconds("the end of the stream", tcp.read(&mut [0; 1]))
                .await
                .expect("the end of the stream, not a reset");
            assert_eq!(read, 0);
        } else {
            eventually("the session to be dialed", WITHIN, || {
                rig.dialer.dialed().len() == 1
            })
            .await;
        }
        rig.tun.stop();
        let ending = in_five_seconds("the TUN to end", rig.tun.join()).await;
        assert_eq!(ending, Ending::Stopped);
    }
}

// ---- the stop

/// The stop resets every connection that is open, and the app is told: each of
/// three idle connections reads `ConnectionReset`. Every task ends, the
/// sessions among them, and nothing is left.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_resets_every_open_connection() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let mut open = Vec::new();
    for n in 1..=3 {
        let dst = addr(&format!("192.0.2.{n}:80"));
        rig.dialer.script(dst, Script::Upstream(echo.addr()));
        let mut tcp = within("the connection to be made", rig.app.connect(dst))
            .await
            .expect("connect");
        hello(&mut tcp).await;
        open.push(tcp);
    }
    assert_eq!(rig.tun.sockets_in_use(), 3);
    rig.tun.stop();
    for tcp in &mut open {
        let error = in_five_seconds("the reset", tcp.read(&mut [0; 1]))
            .await
            .expect_err("the stop resets the connection");
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    }
    let ending = in_five_seconds("the TUN to end", rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    let ends = rig.dialer.relay_ends();
    assert_eq!(ends.len(), 3, "every relay ended: {ends:?}");
    assert!(
        ends.iter().all(Result::is_err),
        "ended by the stop, not cleanly: {ends:?}"
    );
    assert_nothing_left(&rig.tun);
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// Waits until `count` has reached `at_least` and then stayed as it is for
/// `quiet`, and answers what it came to; `limit` is all the time it has. What it
/// waits for is an absence, a writer that is stuck, which nothing announces: so
/// it polls, and the quiet is what tells a writer that is stuck from one that
/// is between two writes.
async fn stays_at(count: &AtomicUsize, at_least: usize, quiet: Duration, limit: Duration) -> usize {
    let deadline = Instant::now() + limit;
    let mut last = count.load(Ordering::SeqCst);
    let mut since = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let now = count.load(Ordering::SeqCst);
        if now != last {
            (last, since) = (now, Instant::now());
        } else if now >= at_least && since.elapsed() >= quiet {
            return now;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the count to stay at {at_least} or more (it is {now})"
        );
    }
}

/// The stop cancels the sessions that are relaying: a relay that waits on its
/// upstream in both directions (writing to one that takes nothing, reading
/// from one that says nothing) touches its stream in neither, so the reset the
/// stop gives the connection never reaches it. Here the upstream is a hold, and
/// the app writes a MiB: the way to the upstream fills, the app's write stops
/// part of the way, and the relay is stuck. The stop ends it, and the relay is
/// recorded as cancelled; were the session not cancelled `join` would wait for
/// the upstream to move, or for a relay's own idle timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_ends_a_relay_stuck_on_its_upstream() {
    const MIB: usize = 1 << 20;
    let rig = rig(TunOptions::default());
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Hold { buffer: 64 * 1024 });
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    let written = Arc::new(AtomicUsize::new(0));
    let writing = tokio::spawn({
        let written = written.clone();
        async move {
            let data = vec![0x5a; MIB];
            let mut rest = &data[..];
            while !rest.is_empty() {
                let n = tcp.write(rest).await?;
                written.fetch_add(n, Ordering::SeqCst);
                rest = &rest[n..];
            }
            Ok::<(), io::Error>(())
        }
    });
    // waits for: the app's writes to stop, which means the way to the upstream
    // is full and the relay is stuck on it. The margin, on the development
    // machine: the way fills in 11 to 26 ms alone and in at most 36 ms with the
    // whole test binary running (the longest gap between two changes of the
    // count was 25 ms), so the 300 ms of quiet is more than eight times the
    // longest fill.
    let stuck = stays_at(&written, 64 * 1024, Duration::from_millis(300), WITHIN).await;
    assert!(
        stuck < MIB,
        "the upstream reads nothing, so the MiB cannot all have been taken ({stuck} bytes were)"
    );
    rig.tun.stop();
    let ending = in_five_seconds("the TUN to end", rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    assert_eq!(rig.dialer.relay_ends(), vec![Err("cancelled".to_string())]);
    let error = in_five_seconds("the app's write to fail", writing)
        .await
        .expect("the writer's task")
        .expect_err("the stop resets the connection");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert_nothing_left(&rig.tun);
}

/// A dialer whose dials wait for the test to let them through, and then do what
/// the fake dialer does.
struct Gated {
    inner: Arc<FakeDialer>,
    open: watch::Sender<bool>,
}

impl Dialer for Gated {
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>> {
        Box::pin(async move {
            let mut gate = self.open.subscribe();
            let _ = gate.wait_for(|open| *open).await;
            self.inner.dial(session).await
        })
    }

    fn relay<'a>(
        &'a self,
        client: BoxedStream,
        upstream: BoxedStream,
        handle: Arc<SessionHandle>,
    ) -> BoxFuture<'a, ()> {
        self.inner.relay(client, upstream, handle)
    }
}

/// The stop does not cancel a dial that is under way: a dial dropped half way
/// would leave a session record that never finishes. The TUN waits for it, and
/// when it comes back the stop is in place already: the session is cancelled
/// and the relay ends at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dial_under_way_is_not_cancelled_by_the_stop() {
    let (device, end) = MemoryDevice::pair(1500, 4096);
    let fake = FakeDialer::new();
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    fake.script(dst, Script::Upstream(echo.addr()));
    let (open, _) = watch::channel(false);
    let gated = Arc::new(Gated {
        inner: fake.clone(),
        open,
    });
    let tun = tun_dialing(Arc::new(device), gated.clone(), TunOptions::default());
    let app = AppStack::start(end);
    let mut tcp = within("the connection to be made", app.connect(dst))
        .await
        .expect("connect");
    eventually("the dial to be under way", WITHIN, || {
        gated.open.receiver_count() == 1
    })
    .await;
    tun.stop();
    // the one fixed wait of this test, for what must not happen: the TUN
    // ending with a dial still under way. It ends in milliseconds once the dial
    // is let through; 300 ms is generous for that and short for the test.
    assert!(
        timeout(Duration::from_millis(300), tun.join())
            .await
            .is_err(),
        "the TUN ended with a dial under way"
    );
    gated.open.send_replace(true);
    let ending = in_five_seconds("the TUN to end", tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    let ends = fake.relay_ends();
    assert_eq!(ends.len(), 1, "the relay ran and ended: {ends:?}");
    assert!(ends[0].is_err(), "ended by the stop, not cleanly: {ends:?}");
    let error = in_five_seconds("the reset", tcp.read(&mut [0; 1]))
        .await
        .expect_err("the stop reset the connection");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert_nothing_left(&tun);
}

/// What the connections held is given back when the TUN stops, whatever state
/// they are in: one established, one half-open (the app's stack never took its
/// SYN-ACK), one whose stream is gone and which waits out its linger for an
/// acknowledgement of its FIN that does not come. Both counts against the caps,
/// the buffers, the tables and the linger table are empty after `join`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_gives_back_what_every_connection_held() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let (open_dst, closing_dst, half_open_dst) = (
        addr("192.0.2.1:80"),
        addr("192.0.2.2:80"),
        addr("192.0.2.3:80"),
    );
    rig.dialer.script(open_dst, Script::Upstream(echo.addr()));
    rig.dialer
        .script(closing_dst, Script::Upstream(echo.addr()));

    // half-open: the SYN-ACK goes to the test, not to the app's stack, which
    // would answer it with a reset
    let _syn_acks = rig.app.divert(40_001..=40_001);
    rig.app.inject(syn_v4(app4(40_001), half_open_dst));
    eventually("the SYN to be taken", WITHIN, || rig.tun.half_open() == 1).await;

    let mut open = within("the connection to be made", rig.app.connect(open_dst))
        .await
        .expect("connect");
    hello(&mut open).await;

    // from the TUN's FIN on, nothing it sends reaches the app
    let swallowing = Arc::new(AtomicBool::new(false));
    let seen = swallowing.clone();
    rig.app.set_filter(move |direction, packet| {
        if direction == Direction::FromTun && is_fin(packet) {
            seen.store(true, Ordering::SeqCst);
        }
        direction != Direction::FromTun || !seen.load(Ordering::SeqCst)
    });
    let mut closing = within("the connection to be made", rig.app.connect(closing_dst))
        .await
        .expect("connect");
    hello(&mut closing).await;
    closing.shutdown().await.expect("the app's FIN");
    eventually("the relay to end", WITHIN, || {
        rig.dialer.relay_ends().len() == 1
    })
    .await;
    eventually("the TUN's FIN to be swallowed", WITHIN, || {
        swallowing.load(Ordering::SeqCst)
    })
    .await;
    eventually("the connection to linger", WITHIN, || {
        rig.tun.records().lingering == 1
    })
    .await;
    // the premises
    assert_eq!(rig.tun.sockets_counted(), 3);
    assert_eq!(rig.tun.half_open(), 1);
    assert!(rig.tun.budget_in_use() > 0);
    assert_eq!(rig.tun.records().conns, 3);

    rig.tun.stop();
    let ending = in_five_seconds("the TUN to end", rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    assert_nothing_left(&rig.tun);
}

/// A device that takes nothing more cannot hold the stop up: the writer goes on
/// writing what the shards hand it for `STOP_GRACE` from the stop, and gives up
/// then, and the shard that waits for room in the writer's queue is let go
/// with it. Here the app end of the device reads nothing: the device's room is
/// two packets, the third holds the writer, the next fills its queue and the
/// fifth holds the shard. The TUN ends after the grace, not before (the device
/// was given its time) and not long after.
#[tokio::test]
async fn a_stuck_device_holds_the_stop_up_for_the_grace_and_no_longer() {
    let (device, end) = MemoryDevice::pair(1500, 2);
    let (tun, _dialer) = tun_on(
        device,
        TunOptions {
            shards: 1,
            writer_queue: 1,
            ..TunOptions::default()
        },
    );
    let dst = addr("192.0.2.1:80");
    // one SYN at a time, each a turn of its own with a SYN-ACK to hand over
    for n in 1..=5u16 {
        end.send(syn_v4(app4(40_000 + n), dst))
            .await
            .expect("the device takes the SYN");
        eventually("the SYN to be taken", WITHIN, || {
            tun.sockets_counted() == usize::from(n)
        })
        .await;
    }
    let started = Instant::now();
    tun.stop();
    let ending = timeout(STOP_GRACE + MARGIN, tun.join())
        .await
        .unwrap_or_else(|_| {
            panic!("the stop was held up for more than the grace of {STOP_GRACE:?} and {MARGIN:?}")
        });
    let took = started.elapsed();
    assert_eq!(ending, Ending::Stopped);
    assert!(
        took >= STOP_GRACE,
        "the writer gave the device {took:?}, less than its grace of {STOP_GRACE:?}"
    );
    assert_nothing_left(&tun);
    drop(end);
}

/// The same stuck device, freed within the grace: it is given the resets of
/// the shards' last turns, which are written in the time the writer has, and
/// the TUN ends when they are, not when the grace is over. The device's room is
/// two packets and the app end reads nothing until after the stop; then it
/// reads all there is, and there is a reset for each of the five connections.
#[tokio::test]
async fn a_slow_device_gets_the_resets_if_it_frees_up_within_the_grace() {
    let (device, mut end) = MemoryDevice::pair(1500, 2);
    let (tun, _dialer) = tun_on(
        device,
        TunOptions {
            shards: 1,
            writer_queue: 1,
            ..TunOptions::default()
        },
    );
    let dst = addr("192.0.2.1:80");
    for n in 1..=5u16 {
        end.send(syn_v4(app4(40_000 + n), dst))
            .await
            .expect("the device takes the SYN");
        eventually("the SYN to be taken", WITHIN, || {
            tun.sockets_counted() == usize::from(n)
        })
        .await;
    }
    tun.stop();
    // the app end reads from here on: the writer is let go of its write
    let reading = tokio::spawn(async move {
        let mut got = Vec::new();
        while let Some(packet) = end.recv().await {
            got.push(packet);
        }
        got
    });
    let ending = in_five_seconds("the TUN to end", tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    let got = in_five_seconds("the packets", reading)
        .await
        .expect("the reading task");
    let resets = got.iter().filter(|packet| is_reset(packet)).count();
    assert_eq!(
        resets,
        5,
        "a reset for each connection: {} packets",
        got.len()
    );
    assert_nothing_left(&tun);
}

// ---- the stop, a SYN on its way

/// A device that stops the TUN as it hands over a packet: the packet is on its
/// way to a shard when the stop is. The runtime of the test has one thread, so
/// nothing else runs between the stop and the packet being queued for its
/// shard; the shard sees the stop and the SYN together.
struct StopsAsItDelivers {
    inner: MemoryDevice,
    tun: OnceLock<TunHandle>,
}

impl Device for StopsAsItDelivers {
    fn recv<'a>(&'a self, into: &'a mut Vec<Vec<u8>>, max: usize) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            self.inner.recv(into, max).await?;
            if let Some(tun) = self.tun.get() {
                tun.stop();
            }
            Ok(())
        })
    }

    fn send<'a>(&'a self, packets: &'a mut Vec<Vec<u8>>) -> BoxFuture<'a, io::Result<()>> {
        self.inner.send(packets)
    }

    fn mtu(&self) -> usize {
        self.inner.mtu()
    }
}

/// A SYN that reaches a shard as the TUN stops gets its reset on the device:
/// the shard's stop takes in what is queued for it, and turns the SYN away
/// (the TUN is stopping), and the writer writes the reset before it ends.
#[tokio::test]
async fn a_syn_queued_as_the_tun_stops_is_reset_on_the_device() {
    let (device, mut end) = MemoryDevice::pair(1500, 16);
    let device = Arc::new(StopsAsItDelivers {
        inner: device,
        tun: OnceLock::new(),
    });
    let (tun, _dialer) = tun_with(device.clone(), TunOptions::default());
    assert!(device.tun.set(tun.clone()).is_ok());
    let (app, dst) = (app4(40_000), addr("192.0.2.1:80"));
    end.send(syn_v4(app, dst))
        .await
        .expect("the device takes the SYN");
    let ending = in_five_seconds("the TUN to end", tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    let reset = end
        .try_recv()
        .expect("the reset is on the device by the time the TUN has ended");
    let read = segment(&reset).expect("a TCP segment");
    assert_eq!(read.tuple, Tuple { src: dst, dst: app });
    assert!(read.rst && read.ack && !read.syn, "{read:?}");
    assert_eq!(read.ack_number, 2, "the SYN's sequence number plus one");
    assert!(end.try_recv().is_none(), "that is all the TUN wrote");
    assert_nothing_left(&tun);
    assert_eq!(tun.counters(), Counters::default());
}

// ---- a device that fails

/// A memory device whose writes the test can make fail, and which notes every
/// write asked of it after one has failed.
struct Flaky {
    inner: MemoryDevice,
    /// From now on every write fails.
    failing: AtomicBool,
    /// One write has failed.
    failed: AtomicBool,
    /// The writes asked of the device after one had failed.
    late: AtomicUsize,
}

impl Flaky {
    fn on(inner: MemoryDevice) -> Arc<Flaky> {
        Arc::new(Flaky {
            inner,
            failing: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            late: AtomicUsize::new(0),
        })
    }

    fn fail_from_now_on(&self) {
        self.failing.store(true, Ordering::SeqCst);
    }

    /// Whether a write has failed.
    fn has_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    /// The writes asked of the device after one had failed.
    fn late_writes(&self) -> usize {
        self.late.load(Ordering::SeqCst)
    }
}

impl Device for Flaky {
    fn recv<'a>(&'a self, into: &'a mut Vec<Vec<u8>>, max: usize) -> BoxFuture<'a, io::Result<()>> {
        self.inner.recv(into, max)
    }

    fn send<'a>(&'a self, packets: &'a mut Vec<Vec<u8>>) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            if self.failed.load(Ordering::SeqCst) {
                self.late.fetch_add(1, Ordering::SeqCst);
            }
            if self.failing.load(Ordering::SeqCst) {
                self.failed.store(true, Ordering::SeqCst);
                packets.clear();
                return Err(io::Error::other("the test's device refuses the write"));
            }
            self.inner.send(packets).await
        })
    }

    fn mtu(&self) -> usize {
        self.inner.mtu()
    }
}

/// A device that fails ends the TUN, the same way a stop does (every
/// connection is let go, the sessions end), with the failure as the ending and
/// one warning, which carries the error's text and no address.
#[tokio::test]
async fn a_device_that_fails_ends_the_tun() {
    let log = LogCapture::start();
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let mut open = Vec::new();
    for n in 1..=2 {
        let dst = addr(&format!("192.0.2.{n}:80"));
        rig.dialer.script(dst, Script::Upstream(echo.addr()));
        let mut tcp = within("the connection to be made", rig.app.connect(dst))
            .await
            .expect("connect");
        hello(&mut tcp).await;
        open.push(tcp);
    }
    rig.app.close();
    let ending = in_five_seconds("the TUN to end", rig.tun.join()).await;
    let text = "the app end of the memory device is gone";
    assert_eq!(ending, Ending::DeviceFailed(text.into()));
    let ends = rig.dialer.relay_ends();
    assert_eq!(ends.len(), 2, "every relay ended: {ends:?}");
    assert_eq!(
        warnings(&log),
        vec![format!(
            "WARN tun: the device failed; the TUN has ended error={text}"
        )],
        "one warning for the TUN, whichever of the reader and the writer met the failure first"
    );
    assert_nothing_left(&rig.tun);
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// A device that fails after the stop changes nothing: the TUN was ending
/// already, so the ending stays what it was and nothing is said. The device is
/// made to fail with the stop's resets still to be written.
#[tokio::test]
async fn a_device_that_fails_after_the_stop_changes_nothing() {
    let log = LogCapture::start();
    let (device, end) = MemoryDevice::pair(1500, 4096);
    let flaky = Flaky::on(device);
    let rig = rig_with(flaky.clone(), end, TunOptions::default());
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    // one thread: the tasks of the TUN run when the test next waits, with the
    // stop and the failure both in place
    rig.tun.stop();
    flaky.fail_from_now_on();
    let ending = in_five_seconds("the TUN to end", rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    assert!(
        flaky.has_failed(),
        "the writer did meet the failure, writing the connection's reset"
    );
    assert_eq!(warnings(&log), Vec::<String>::new());
    assert_nothing_left(&rig.tun);
}

/// A write that fails is the last the writer makes: the resets the shards hand
/// it after that have no one to write them, and none is tried. The device is
/// made to fail while two connections are open, so there are resets to write.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_written_after_a_failed_write() {
    let (device, end) = MemoryDevice::pair(1500, 4096);
    let flaky = Flaky::on(device);
    let rig = rig_with(flaky.clone(), end, TunOptions::default());
    let echo = Upstream::echo().await;
    let mut open = Vec::new();
    for n in 1..=2 {
        let dst = addr(&format!("192.0.2.{n}:80"));
        rig.dialer.script(dst, Script::Upstream(echo.addr()));
        let mut tcp = within("the connection to be made", rig.app.connect(dst))
            .await
            .expect("connect");
        hello(&mut tcp).await;
        open.push(tcp);
    }
    flaky.fail_from_now_on();
    // the TUN's next write is the failed one: an answer to the app
    open[0].write_all(b"x").await.expect("write");
    let ending = in_five_seconds("the TUN to end", rig.tun.join()).await;
    assert!(matches!(ending, Ending::DeviceFailed(_)), "{ending:?}");
    assert!(flaky.has_failed());
    assert_eq!(
        flaky.late_writes(),
        0,
        "the device was asked to write again after a write failed"
    );
    assert_eq!(rig.dialer.relay_ends().len(), 2);
    assert_nothing_left(&rig.tun);
}

/// `join` answers the same every time: for a TUN that was stopped, for one
/// whose device failed (and a stop after that does not change it), and for
/// two joins at once.
#[tokio::test]
async fn join_answers_the_same_again() {
    let stopped = rig(TunOptions::default());
    stopped.tun.stop();
    let first = in_five_seconds("the TUN to end", stopped.tun.join()).await;
    assert_eq!(first, Ending::Stopped);
    let second = in_five_seconds("the same answer", stopped.tun.join()).await;
    assert_eq!(second, first);
    let clone = stopped.tun.clone();
    let (one, other) = in_five_seconds("both joins", async {
        tokio::join!(stopped.tun.join(), clone.join())
    })
    .await;
    assert_eq!((one, other), (first.clone(), first));

    let failed = rig(TunOptions::default());
    failed.app.close();
    let first = in_five_seconds("the TUN to end", failed.tun.join()).await;
    assert!(matches!(first, Ending::DeviceFailed(_)), "{first:?}");
    let second = in_five_seconds("the same answer", failed.tun.join()).await;
    assert_eq!(second, first);
    failed.tun.stop();
    let third = in_five_seconds("the same answer", failed.tun.join()).await;
    assert_eq!(third, first, "a stop after the failure leaves the ending");
}

/// The first ending stays, and the call that gave it knows.
#[test]
fn the_first_ending_stays_and_says_so() {
    let life = Life::default();
    assert!(!life.is_stopped());
    assert!(life.end(Ending::DeviceFailed("first".into())));
    assert!(life.is_stopped());
    assert!(!life.end(Ending::Stopped));
    assert!(!life.end(Ending::DeviceFailed("second".into())));
    assert_eq!(life.ending(), Ending::DeviceFailed("first".into()));
}

/// A device failure is said once for the TUN, at WARN, with the error's text,
/// by the call that ends the TUN with it; a failure met after another ending
/// changes nothing and says nothing.
#[test]
fn a_device_failure_is_said_once_and_only_if_it_ends_the_tun() {
    let (first, second) = (io::Error::other("first"), io::Error::other("second"));
    {
        let log = LogCapture::start();
        let life = Life::default();
        life.device_failed(&first);
        life.device_failed(&second);
        assert_eq!(
            warnings(&log),
            vec!["WARN tun: the device failed; the TUN has ended error=first"]
        );
        assert_eq!(life.ending(), Ending::DeviceFailed("first".into()));
    }
    {
        let log = LogCapture::start();
        let life = Life::default();
        life.end(Ending::Stopped);
        life.device_failed(&second);
        assert_eq!(warnings(&log), Vec::<String>::new());
        assert_eq!(life.ending(), Ending::Stopped);
    }
}

// ---- one shard's task

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
