//! The stop: every connection is reset and the app is told, a relay stuck on
//! its upstream is cancelled, the resets reach the device even when the device
//! is slow, and a device that takes nothing cannot hold the stop up for longer
//! than a grace. And a SYN that is on its way as the TUN stops.

use super::assert_nothing_left;
use crate::device::{Device, MemoryDevice};
use crate::packet::segment;
use crate::tcp::Tuple;
use crate::testing::{AppStack, Direction, FakeDialer, Script, Upstream};
use crate::tests::hazards::app4;
use crate::tests::support::{
    Extras, WITHIN, addr, eventually, hello, in_five_seconds, is_fin, is_reset, rig, syn_v4,
    tun_dialing, tun_on, tun_with, within,
};
use crate::writer::STOP_GRACE;
use crate::{Counters, Ending, TunHandle, TunOptions};
use rurge_config::SessionInfo;
use rurge_inbound::{DialError, Dialed, Dialer, SessionHandle};
use rurge_net::BoxFuture;
use rurge_net::connector::BoxedStream;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use tokio::time::timeout;

/// What a stop is given to finish in beyond its grace: the tasks of the TUN
/// unwind in milliseconds, and this is for a machine that is busy.
const MARGIN: Duration = Duration::from_secs(3);

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

/// `session_tasks` counts the sessions of a TUN that is running, by taking the
/// shards, the writer and the reader off the tasks on the tracker. Once the TUN
/// has stopped those end, one after the other, and what is left counts nothing:
/// a test that wants the tasks after a stop reads `live_tasks`. Asking for the
/// count after a stop is a mistake in the test, and `session_tasks` says so in
/// a debug build. This asks straight after the stop, on a runtime of one
/// thread, where no task has run yet and so none has ended: the subtraction
/// would still work, and only the assertion can make the call panic.
#[cfg(debug_assertions)]
#[tokio::test]
#[should_panic(expected = "session_tasks is asked of a TUN that has stopped")]
async fn session_tasks_is_not_read_after_the_stop() {
    let rig = rig(TunOptions::default());
    rig.tun.stop();
    let _ = rig.tun.session_tasks();
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
pub(super) struct Gated {
    pub(super) inner: Arc<FakeDialer>,
    pub(super) open: watch::Sender<bool>,
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
    let tun = tun_dialing(
        Arc::new(device),
        gated.clone(),
        &Extras::default(),
        TunOptions::default(),
    );
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
    // The stop comes first: the session's token is cancelled by it, and the
    // relay ends as cancelled, not as a copy that met the reset of its
    // connection. The token that the session waits on is made after the stop
    // here, and is cancelled already.
    let ends = fake.relay_ends();
    assert_eq!(
        ends,
        [Err("cancelled".to_string())],
        "the relay ran once and the stop cancelled it"
    );
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
