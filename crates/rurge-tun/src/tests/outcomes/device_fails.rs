//! A device that fails ends the TUN the way a stop does, with the failure as
//! the ending and one warning; and what `join` answers, and which ending stays.

use super::assert_nothing_left;
use super::stop::Gated;
use crate::device::{Device, MemoryDevice};
use crate::testing::{AppStack, FakeDialer, Script, Upstream};
use crate::tests::log_capture::LogCapture;
use crate::tests::support::{
    Extras, WITHIN, addr, eventually, hello, in_five_seconds, rig, rig_with, tun_dialing, within,
};
use crate::{Counters, Ending, Life, TunOptions};
use rurge_net::BoxFuture;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio::time::timeout;

/// The warnings among the lines the capture has, whole.
fn warnings(log: &LogCapture) -> Vec<String> {
    log.lines()
        .into_iter()
        .filter(|line| line.starts_with("WARN"))
        .collect()
}

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

/// `ending` answers as soon as the TUN is told to end, without waiting for a
/// dial that is under way: what watches for a device that failed must not wait
/// for a dial that never comes back, while a close, which does wait for the
/// TUN's tasks, gives its wait for `join` a limit of its own (the phase 3 / M4a
/// plan's execution record row 100).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ending_answers_before_a_dial_under_way_comes_back() {
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
    let _tcp = within("the connection to be made", app.connect(dst))
        .await
        .expect("connect");
    eventually("the dial to be under way", WITHIN, || {
        gated.open.receiver_count() == 1
    })
    .await;

    // the device fails under the TUN while the dial is still held
    app.close();
    let ending = in_five_seconds("the TUN to say why it ended", tun.ending()).await;
    assert!(matches!(ending, Ending::DeviceFailed(_)), "{ending:?}");
    // the one fixed wait of this test, for what must not happen: `join` coming
    // back with the dial still held. It ends in milliseconds once the dial is
    // let through; 300 ms is generous for that and short for the test.
    assert!(
        timeout(Duration::from_millis(300), tun.join())
            .await
            .is_err(),
        "join came back with a dial under way"
    );
    gated.open.send_replace(true);
    let joined = in_five_seconds("the TUN to end", tun.join()).await;
    assert!(matches!(joined, Ending::DeviceFailed(_)), "{joined:?}");
    assert_nothing_left(&tun);
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
