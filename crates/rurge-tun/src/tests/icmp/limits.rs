//! Time, places and the stop: the timeout of an echo request, how many are in
//! flight, the place each takes and gives back, and the stop that ends them.

use super::{APP4, APP6, FAR4, FAR6, echoed, ip, lab};
use crate::testing::PingScript;
use crate::tests::support::{WITHIN, eventually, in_five_seconds, settings};
use crate::{Counters, Ending, TunOptions};
use rurge_dns::fakeip::v4_of;
use rurge_net::BoxFuture;
use rurge_net::connector::Resolve;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A pinger that says nothing is waited for `echo_timeout` and then given up
/// on, and the task is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_pinger_gives_up_after_the_timeout() {
    let mut lab = lab(TunOptions {
        echo_timeout: Duration::from_millis(300),
        ..TunOptions::default()
    });
    lab.extras.pinger.script(FAR4, PingScript::Silent);
    lab.extras.pinger.script(FAR6, PingScript::Echo);
    let begun = Instant::now();
    lab.ping(APP4, FAR4, 1, 1, b"anyone?");
    // the task has begun when the pinger is asked, and nothing else ends it
    lab.pinged_times(1).await;
    lab.tasks_end().await;
    assert!(
        begun.elapsed() >= Duration::from_millis(300),
        "it was given up on after {:?}",
        begun.elapsed()
    );
    // no reply came, and another request is answered as usual
    lab.ping(APP6, FAR6, 2, 2, b"anyone?");
    assert_eq!(lab.reply().await, echoed(FAR6, APP6, 2, 2, b"anyone?"));
    assert_eq!(lab.counters(), Counters::default());
}

/// The timeout counts from the beginning of the task: the resolving of a
/// name is part of the wait. The resolver takes a second to answer; the task
/// is given up on after 300 ms, and the pinger is not asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_timeout_counts_the_resolving_too() {
    struct Slow;

    impl Resolve for Slow {
        fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(vec![FAR4])
            })
        }
    }

    let mut lab = lab(TunOptions {
        echo_timeout: Duration::from_millis(300),
        ..TunOptions::default()
    });
    lab.extras.hooks.set(settings(Arc::new(Slow), true));
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    let slow = lab.assign("slow.test");
    lab.ping(APP4, IpAddr::V4(v4_of(slow)), 1, 1, b"anyone?");
    // the task ends, and when it does the pinger has not been asked: it would
    // be if the resolving did not count, a second from now
    lab.dealt_with().await;
    assert!(lab.pinged().is_empty());
    assert_eq!(lab.counters(), Counters::default());
}

/// At most `max_echoes` echo requests wait for their replies at once; one more
/// is dropped and counted as busy. A fake address that names nothing is
/// counted for the limit that was met first. An address of rurge's own takes no
/// place.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn echoes_in_flight_are_capped() {
    let mut lab = lab(TunOptions {
        max_echoes: 2,
        ..TunOptions::default()
    });
    lab.extras.pinger.script(FAR4, PingScript::Silent);
    lab.extras.pinger.script(FAR6, PingScript::Silent);
    lab.ping(APP4, FAR4, 1, 1, b"one");
    lab.ping(APP6, FAR6, 1, 2, b"two");
    lab.pinged_times(2).await;
    lab.ping(APP4, FAR4, 1, 3, b"three");
    eventually("the third to be counted", WITHIN, || {
        lab.counters().busy == 1
    })
    .await;
    lab.ping(APP4, IpAddr::V4(v4_of(7)), 1, 4, b"four");
    eventually("the fourth to be counted", WITHIN, || {
        lab.counters().busy == 2
    })
    .await;
    lab.nothing_came_first().await;
    assert_eq!(lab.pinged().len(), 2, "the third was not pinged");
    assert_eq!(
        lab.counters(),
        Counters {
            busy: 2,
            ..Counters::default()
        }
    );
}

/// The place of an echo request is given back when its task ends, whichever
/// way it ends: with room for one, each kind of end is followed by a request
/// that is taken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_place_of_an_echo_is_given_back_however_it_ends() {
    let mut lab = lab(TunOptions {
        max_echoes: 1,
        echo_timeout: Duration::from_millis(300),
        ..TunOptions::default()
    });
    let lost = lab.assign("lost.test");
    let (failing, silent) = (ip("2001:db8::66"), ip("192.0.2.66"));
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    lab.extras.pinger.script(failing, PingScript::Fail);
    lab.extras.pinger.script(silent, PingScript::Silent);
    // a reply
    lab.ping(APP4, FAR4, 1, 1, b"reply");
    assert_eq!(lab.reply().await, echoed(FAR4, APP4, 1, 1, b"reply"));
    lab.tasks_end().await;
    // a failure of the pinger
    lab.ping(APP6, failing, 1, 2, b"failure");
    lab.dealt_with().await;
    // a name that does not resolve
    lab.ping(APP4, IpAddr::V4(v4_of(lost)), 1, 3, b"lost");
    lab.dealt_with().await;
    // a fake address that names nothing: the place was taken and goes back
    lab.ping(APP4, IpAddr::V4(v4_of(7)), 1, 4, b"nobody");
    lab.dealt_with().await;
    // the timeout
    lab.ping(APP4, silent, 1, 5, b"silence");
    lab.pinged_times(3).await;
    lab.tasks_end().await;
    // and a request is taken after all of them
    lab.ping(APP4, FAR4, 1, 6, b"last");
    assert_eq!(lab.reply().await, echoed(FAR4, APP4, 1, 6, b"last"));
    lab.tasks_end().await;
    assert_eq!(
        lab.counters(),
        Counters {
            unknown_fake: 1,
            ..Counters::default()
        }
    );
}

/// A stop ends the echo requests that wait for their replies, at once: they
/// have a minute to wait, and the TUN has ended long before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_ends_the_echoes_in_flight() {
    let lab = lab(TunOptions {
        echo_timeout: Duration::from_secs(60),
        ..TunOptions::default()
    });
    lab.extras.pinger.script(FAR4, PingScript::Silent);
    lab.extras.pinger.script(FAR6, PingScript::Silent);
    lab.ping(APP4, FAR4, 1, 1, b"anyone?");
    lab.ping(APP6, FAR6, 1, 2, b"anyone?");
    lab.pinged_times(2).await;
    lab.rig.tun.stop();
    let ending = in_five_seconds("the TUN to end", lab.rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
}
