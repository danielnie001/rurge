//! What a session leaves of a connection it rejects: REJECT-DROP holds the
//! connection and then closes it, and lets it go when the connection ends first;
//! the other rejections reset it.

use super::assert_nothing_left;
use crate::testing::{AppStack, Direction, Script};
use crate::tests::support::{
    FIVE_SECONDS, WITHIN, addr, eventually, in_five_seconds, is_reset, rig, within,
};
use crate::{Counters, Ending, TunOptions};
use rurge_proto::RejectKind;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

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
