//! The life of a UDP session and its limits: the queue, the admission, the idle
//! time, the destinations a session holds and the leases it has on their fake
//! addresses, and the ways an association ends.

use super::{APP4, FAR4, lab, lab_with, named, target, target_of};
use crate::device::MemoryDevice;
use crate::testing::AppStack;
use crate::testing::packets;
use crate::tests::support::{Extras, WITHIN, addr, eventually, in_five_seconds, sock, tun_dialing};
use crate::{Counters, Ending, TunOptions};
use rurge_config::SessionInfo;
use rurge_dns::fakeip::{AssignError, FakeIpPool, v4_of};
use rurge_inbound::{
    DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome, UdpAdmission, UdpClient,
};
use rurge_net::BoxFuture;
use rurge_net::connector::BoxedStream;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// The session's queue holds 256 datagrams for the dialer to take: the others
/// are dropped and counted, and the 256 are taken in order once it takes any.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_datagram_queue_holds_256() {
    let lab = lab(TunOptions::default());
    lab.dialer().hold_udp();
    let (app, dst) = (addr(APP4), addr(FAR4));
    for number in 0..300u16 {
        lab.send(app, dst, &number.to_be_bytes());
    }
    eventually("the datagrams to be counted", WITHIN, || {
        lab.counters().queue_full == 44
    })
    .await;
    lab.dialer().release_udp();
    eventually("the queue to be taken", WITHIN, || {
        lab.dialer().udp_sent().len() == 256
    })
    .await;
    for (number, (to, bytes)) in lab.dialer().udp_sent().iter().enumerate() {
        assert_eq!(to, &target_of(dst));
        assert_eq!(bytes, &(number as u16).to_be_bytes(), "datagram {number}");
    }
    assert_eq!(
        lab.counters(),
        Counters {
            queue_full: 44,
            ..Counters::default()
        }
    );
}

/// The engine that has no association to give, or carries no UDP, takes the
/// datagram no further: dropped and counted, and no session is made for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_admission_drops_and_counts() {
    let lab = lab(TunOptions::default());
    let (app, dst) = (addr(APP4), addr(FAR4));
    lab.dialer().admit(UdpAdmission::Busy);
    for _ in 0..3 {
        lab.send(app, dst, b"x");
    }
    eventually("the datagrams to be counted", WITHIN, || {
        lab.counters().busy == 3
    })
    .await;
    lab.dialer().admit(UdpAdmission::NotSupported);
    for _ in 0..2 {
        lab.send(app, dst, b"x");
    }
    eventually("the datagrams to be counted", WITHIN, || {
        lab.counters().unsupported == 2
    })
    .await;
    assert!(lab.dialer().associations().is_empty());
    assert_eq!(
        lab.counters(),
        Counters {
            busy: 3,
            unsupported: 2,
            ..Counters::default()
        }
    );
    // and once it takes associations again the next datagram has one
    lab.dialer().admit(UdpAdmission::Accepted);
    lab.send(app, dst, b"x");
    lab.associations_open(1).await;
}

/// A session that has had nothing either way for `udp_idle` ends, however it
/// began, and the next datagram of the app begins another.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_ends_after_the_idle_time() {
    let idle = Duration::from_millis(300);
    let lab = lab(TunOptions {
        udp_idle: idle,
        ..TunOptions::default()
    });
    let (app, dst) = (addr(APP4), addr(FAR4));
    let begun = Instant::now();
    lab.send(app, dst, b"x");
    lab.associations_open(1).await;
    assert_eq!(lab.rig.tun.session_tasks(), 1);
    eventually("the session to end", WITHIN, || {
        lab.dialer().associations_open() == 0 && lab.rig.tun.session_tasks() == 0
    })
    .await;
    assert!(
        begun.elapsed() >= idle,
        "the session ended after {:?}, before the idle time",
        begun.elapsed()
    );
    lab.send(app, dst, b"y");
    lab.associations_open(1).await;
    assert_eq!(lab.dialer().associations().len(), 2);
}

/// A session that is only sent to, a server pushing and the app only listening,
/// does not end for as long as the pushes come.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_with_traffic_stays() {
    let mut lab = lab(TunOptions {
        udp_idle: Duration::from_secs(1),
        ..TunOptions::default()
    });
    let (app, dst) = (addr(APP4), addr(FAR4));
    let from = target_of(dst);
    lab.send(app, dst, b"subscribe");
    lab.associations_open(1).await;
    // 25 pushes at the pace of 100 ms last 2.5 s, two and a half times the idle
    // time: the pushes alone, and nothing else, keep the session that long
    for push in 0..25 {
        // the pace of the server, shorter than the idle time
        tokio::time::sleep(Duration::from_millis(100)).await;
        lab.dialer()
            .send_to_app(app, from.clone(), Some(from.clone()), b"push".to_vec())
            .await
            .expect("the push is sent");
        lab.datagram().await;
        assert_eq!(
            lab.dialer().associations_open(),
            1,
            "the session is there after push {push}"
        );
    }
    eventually("the session to end", WITHIN, || {
        lab.dialer().associations_open() == 0 && lab.rig.tun.session_tasks() == 0
    })
    .await;
}

/// The same when what is pushed is from a third party, which is no flow's
/// answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_third_party_push_keeps_the_session_too() {
    let mut lab = lab(TunOptions {
        udp_idle: Duration::from_secs(1),
        ..TunOptions::default()
    });
    let (app, dst) = (addr(APP4), addr(FAR4));
    lab.send(app, dst, b"hello");
    lab.associations_open(1).await;
    // 25 pushes 100 ms apart last 2.5 s, two and a half times the idle time
    for push in 0..25 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        lab.dialer()
            .send_to_app(app, target("192.0.2.99", 3478), None, b"push".to_vec())
            .await
            .expect("the push is sent");
        lab.datagram().await;
        assert_eq!(
            lab.dialer().associations_open(),
            1,
            "the session is there after push {push}"
        );
    }
    eventually("the session to end", WITHIN, || {
        lab.dialer().associations_open() == 0
    })
    .await;
}

/// A destination that only the server talks to (the app sent it one datagram and
/// listens) is kept for as long as the server does, with its hold on the fake
/// address: the address is not given to another name while the replies come.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_target_that_only_the_server_talks_to_keeps_its_lease() {
    let pool = Arc::new(FakeIpPool::with_count(1));
    let mut lab = lab_with(
        TunOptions {
            udp_idle: Duration::from_secs(1),
            ..TunOptions::default()
        },
        Extras {
            pool: Some(pool.clone()),
            ..Extras::default()
        },
    );
    let index = pool
        .assign("a.test", Instant::now())
        .expect("an address for a.test");
    let (app, fake, name) = (addr(APP4), sock(v4_of(index), 53), named("a.test", 53));
    lab.send(app, fake, b"subscribe");
    lab.associations_open(1).await;
    // 25 pushes 100 ms apart last 2.5 s, two and a half times the idle time
    for push in 0..25 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        lab.dialer()
            .send_to_app(app, name.clone(), Some(name.clone()), b"push".to_vec())
            .await
            .expect("the push is sent");
        assert_eq!(lab.datagram().await.src, fake);
        assert_eq!(
            pool.assign("b.test", Instant::now()),
            Err(AssignError::Exhausted),
            "the address is still a.test's after push {push}"
        );
    }
}

/// A session has `max_udp_targets` destinations at most; a datagram to one
/// more is dropped and counted, while those it has go on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn targets_are_at_most_max_udp_targets() {
    let lab = lab(TunOptions {
        max_udp_targets: 4,
        ..TunOptions::default()
    });
    let app = addr(APP4);
    for port in 1..=5 {
        lab.send(app, sock([192, 0, 2, 10], port), b"x");
    }
    eventually("the fifth to be counted", WITHIN, || {
        lab.counters().busy == 1
    })
    .await;
    eventually("the others to be taken", WITHIN, || {
        lab.dialer().udp_sent().len() == 4
    })
    .await;
    // a destination it has takes more; one more is turned away again
    lab.send(app, sock([192, 0, 2, 10], 1), b"again");
    lab.send(app, sock([192, 0, 2, 10], 6), b"x");
    eventually("the datagrams to be counted", WITHIN, || {
        lab.counters().busy == 2 && lab.dialer().udp_sent().len() == 5
    })
    .await;
    assert_eq!(lab.dialer().associations().len(), 1);
}

/// A destination that has had nothing either way for `udp_idle` is let go
/// while the session goes on: it gives its place and its lease back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_target_gives_back_its_lease_while_the_session_goes_on() {
    let pool = Arc::new(FakeIpPool::with_count(2));
    let lab = lab_with(
        TunOptions {
            udp_idle: Duration::from_millis(300),
            max_udp_targets: 2,
            ..TunOptions::default()
        },
        Extras {
            pool: Some(pool.clone()),
            ..Extras::default()
        },
    );
    let now = Instant::now();
    let a = pool.assign("a.test", now).expect("an address for a.test");
    let b = pool.assign("b.test", now).expect("an address for b.test");
    let app = addr(APP4);
    // both destinations are in the session and leased
    lab.send(app, sock(v4_of(a), 53), b"x");
    lab.send(app, sock(v4_of(b), 53), b"x");
    eventually("both datagrams to be taken", WITHIN, || {
        lab.dialer().udp_sent().len() == 2
    })
    .await;
    assert_eq!(pool.assign("c.test", now), Err(AssignError::Exhausted));
    lab.send(app, sock([192, 0, 2, 10], 7), b"x");
    eventually("the third destination to be counted", WITHIN, || {
        lab.counters().busy == 1
    })
    .await;
    // b is kept up by traffic; a is not, and goes
    let mut released = false;
    for _ in 0..40 {
        lab.send(app, sock(v4_of(b), 53), b"keep");
        if pool.assign("c.test", now).is_ok() {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(released, "a's address was given back");
    assert_eq!(
        lab.dialer().associations_open(),
        1,
        "the session went on, kept up by b"
    );
    // and its place: the session takes a destination again
    lab.send(app, sock([192, 0, 2, 10], 7), b"x");
    eventually("the destination to be taken", WITHIN, || {
        lab.dialer()
            .udp_sent()
            .iter()
            .any(|(to, _)| to == &target("192.0.2.10", 7))
    })
    .await;
}

/// A session holds the fake addresses of its destinations, so that they are not
/// given to other names while it lives, and gives them back when it ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_releases_its_leases() {
    let pool = Arc::new(FakeIpPool::with_count(1));
    let lab = lab_with(
        TunOptions {
            udp_idle: Duration::from_millis(300),
            ..TunOptions::default()
        },
        Extras {
            pool: Some(pool.clone()),
            ..Extras::default()
        },
    );
    let index = pool
        .assign("a.test", Instant::now())
        .expect("an address for a.test");
    lab.send(addr(APP4), sock(v4_of(index), 53), b"x");
    lab.associations_open(1).await;
    assert_eq!(
        pool.assign("b.test", Instant::now()),
        Err(AssignError::Exhausted),
        "the session holds a.test's address"
    );
    eventually("the session to end", WITHIN, || {
        lab.dialer().associations_open() == 0 && lab.rig.tun.session_tasks() == 0
    })
    .await;
    assert_eq!(
        pool.assign("b.test", Instant::now()),
        Ok(0),
        "the address was given back"
    );
}

/// A stop ends every session, whatever it is doing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_ends_every_udp_session() {
    let lab = lab(TunOptions::default());
    for port in 5000..5003 {
        lab.send(sock([198, 18, 0, 1], port), addr(FAR4), b"x");
    }
    lab.associations_open(3).await;
    lab.rig.tun.stop();
    let ending = in_five_seconds("the TUN to end", lab.rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    assert_eq!(lab.dialer().associations_open(), 0);
}

// ---- associations that end in their own ways

/// A dialer whose associations end on their own, or linger after they are told
/// to end, and which writes down the datagrams each one got.
struct Ends {
    /// The associations started, and the ones that have returned.
    started: AtomicUsize,
    returned: AtomicUsize,
    /// The datagrams of each association, by the order they started in.
    received: Mutex<Vec<Vec<Vec<u8>>>>,
    /// Whether the first association has been told to end.
    told: AtomicBool,
    /// Whether an association returns at once; else it serves until it is told
    /// to end, and the first lingers for `linger` after that.
    at_once: bool,
    linger: Duration,
}

impl Ends {
    fn new(at_once: bool, linger: Duration) -> Arc<Ends> {
        Arc::new(Ends {
            started: AtomicUsize::new(0),
            returned: AtomicUsize::new(0),
            received: Mutex::new(Vec::new()),
            told: AtomicBool::new(false),
            at_once,
            linger,
        })
    }

    fn received(&self, association: usize) -> usize {
        self.received.lock().expect("the dialer's book")[association].len()
    }
}

impl Dialer for Ends {
    /// Nothing here is TCP: a dial fails, and there is no relay to run.
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>> {
        let handle = SessionHandle::new(0, session);
        let message = "this dialer takes no TCP".to_owned();
        handle.finish(SessionOutcome::Failed(message.clone()));
        Box::pin(std::future::ready(Err(DialError::Failed {
            kind: FailKind::Other,
            message,
            rule: None,
            handle,
        })))
    }

    fn relay<'a>(
        &'a self,
        _: BoxedStream,
        _: BoxedStream,
        _: Arc<SessionHandle>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(std::future::ready(()))
    }

    fn admit_udp(&self) -> UdpAdmission {
        UdpAdmission::Accepted
    }

    fn associate<'a>(
        &'a self,
        client: Arc<dyn UdpClient>,
        _: SessionInfo,
        closed: CancellationToken,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let number = self.started.fetch_add(1, Ordering::SeqCst);
            self.received
                .lock()
                .expect("the dialer's book")
                .push(Vec::new());
            if !self.at_once {
                let mut buf = vec![0; 65536];
                loop {
                    tokio::select! {
                        _ = closed.cancelled() => break,
                        got = client.recv(&mut buf) => match got {
                            Ok((n, _)) => self.received.lock().expect("the dialer's book")[number]
                                .push(buf[..n].to_vec()),
                            Err(_) => break,
                        },
                    }
                }
                if number == 0 {
                    self.told.store(true, Ordering::SeqCst);
                    tokio::time::sleep(self.linger).await;
                }
            }
            self.returned.fetch_add(1, Ordering::SeqCst);
        })
    }
}

/// A session whose association returns by itself (the engine stopped, it was
/// full) is over: the next datagram of the app begins another.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_ends_when_its_association_returns() {
    let (device, end) = MemoryDevice::pair(1500, 4096);
    let ends = Ends::new(true, Duration::ZERO);
    let tun = tun_dialing(
        Arc::new(device),
        ends.clone(),
        &Extras::default(),
        TunOptions::default(),
    );
    let app = AppStack::start(end);
    for round in 1..=3 {
        app.inject(packets::udp(addr(APP4), addr(FAR4), b"x"));
        eventually("the association to have returned", WITHIN, || {
            ends.returned.load(Ordering::SeqCst) == round && tun.session_tasks() == 0
        })
        .await;
    }
    assert_eq!(ends.started.load(Ordering::SeqCst), 3);
}

/// A session that is over but whose association is slow to return is no longer
/// where datagrams go: the next one begins a session, which the old one does
/// not take out of the table when it is done.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_old_session_that_ends_late_does_not_take_its_replacement_with_it() {
    let (device, end) = MemoryDevice::pair(1500, 4096);
    let ends = Ends::new(false, Duration::from_millis(700));
    let tun = tun_dialing(
        Arc::new(device),
        ends.clone(),
        &Extras::default(),
        TunOptions {
            udp_idle: Duration::from_millis(250),
            ..TunOptions::default()
        },
    );
    let app = AppStack::start(end);
    let packet = packets::udp(addr(APP4), addr(FAR4), b"x");
    // the first session idles out, and its association lingers
    app.inject(packet.clone());
    eventually("the first association to be told to end", WITHIN, || {
        ends.told.load(Ordering::SeqCst)
    })
    .await;
    // the next datagram begins the second; it is kept up while the first lingers
    app.inject(packet.clone());
    eventually("the second association", WITHIN, || {
        ends.started.load(Ordering::SeqCst) == 2
    })
    .await;
    let begun = Instant::now();
    while ends.returned.load(Ordering::SeqCst) < 1 {
        assert!(
            begun.elapsed() < WITHIN,
            "timed out waiting for the first association to return"
        );
        // the first session's task is there as long as its association is
        assert_eq!(tun.session_tasks(), 2, "the old and the new session");
        // the pace of an app, shorter than the idle time
        app.inject(packet.clone());
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    eventually("the first session's task to be gone", WITHIN, || {
        tun.session_tasks() == 1
    })
    .await;
    // the second session is still where datagrams go
    let before = ends.received(1);
    app.inject(packet);
    eventually("the datagram to reach the second session", WITHIN, || {
        ends.received(1) > before
    })
    .await;
    assert_eq!(ends.started.load(Ordering::SeqCst), 2);
}
