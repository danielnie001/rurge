//! ICMP echo through the TUN (phase 3 M3 design 8): an echo request to an
//! address of rurge's own is answered here, any other goes out through the
//! pinger (one to a fake address to the address its name has), and the reply
//! comes back as from the address the app sent the request to. The app sends
//! with `AppStack::inject` and reads what the TUN writes through
//! `divert_icmp`, so that nothing it reads has been through a stack of its own.

use super::filter::warning;
use super::log_capture::LogCapture;
use super::support::{
    Extras, GatedDevice, Rig, WITHIN, eventually, in_five_seconds, tun_dialing, with_ip_options,
    within,
};
use crate::device::{AppEnd, Device, MemoryDevice};
use crate::packet::Reason;
use crate::testing::packets::{self, IcmpSeen};
use crate::testing::{AppStack, FakeDialer, FakeResolve, PingScript};
use crate::writer::STOP_GRACE;
use crate::{Counters, Ending, TunOptions, TunSettings};
use rurge_dns::fakeip::{FakeIpPool, v4_of, v6_of};
use rurge_net::BoxFuture;
use rurge_net::connector::Resolve;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, mpsc::error::TryRecvError};

/// The one fixed wait of these tests, for what must not happen: a reply for
/// the app. It would go through the writer and the app's stack in well under a
/// millisecond; 200 ms is a wide margin.
const NOTHING: Duration = Duration::from_millis(200);

/// How long after the stop what ends at the stop is to have ended: six tenths
/// of the grace that a writer stuck in its write is given (`STOP_GRACE`), so
/// well within it.
const STOP_WELL_BEFORE: Duration = Duration::from_millis(STOP_GRACE.as_millis() as u64 * 6 / 10);

/// The app's addresses and the places it pings, one family each; the far ones
/// are documentation addresses.
const APP4: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1));
const APP6: IpAddr = IpAddr::V6(Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0, 1));
const FAR4: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
const FAR6: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 10));

/// An address of rurge's own, and the identifier of the echo request to it
/// that is the marker (see `Lab::nothing_came_first`).
const OWN4: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 18, 0, 253));
const MARKER: u16 = 0xfeed;

fn ip(text: &str) -> IpAddr {
    text.parse().expect("an address")
}

/// The echo reply the app is to read.
fn echoed(src: IpAddr, dst: IpAddr, ident: u16, seq: u16, payload: &[u8]) -> IcmpSeen {
    IcmpSeen::EchoReply {
        src,
        dst,
        ident,
        seq,
        payload: payload.to_vec(),
    }
}

/// A rig whose app end has its ICMP packets in the test's hands, with what the
/// TUN was started with.
struct Lab {
    rig: Rig,
    extras: Extras,
    /// The resolver of the settings, which the test teaches names.
    resolver: Arc<FakeResolve>,
    icmp: mpsc::UnboundedReceiver<Vec<u8>>,
}

fn lab(options: TunOptions) -> Lab {
    lab_with(options, Extras::default())
}

fn lab_with(options: TunOptions, extras: Extras) -> Lab {
    let (device, end) = MemoryDevice::pair(1500, 4096);
    lab_on(Arc::new(device), end, options, extras)
}

/// A lab on a device of the test's own, with the app's stack on `end`.
fn lab_on(device: Arc<dyn Device>, end: AppEnd, options: TunOptions, extras: Extras) -> Lab {
    let resolver = FakeResolve::new();
    extras.hooks.set(TunSettings {
        resolver: resolver.clone(),
        icmp_forwarding: true,
    });
    let dialer = FakeDialer::new();
    let tun = tun_dialing(device, dialer.clone(), &extras, options);
    let app = AppStack::start(end);
    let icmp = app.divert_icmp();
    Lab {
        rig: Rig { tun, app, dialer },
        extras,
        resolver,
        icmp,
    }
}

impl Lab {
    fn counters(&self) -> Counters {
        self.rig.tun.counters()
    }

    /// Gives `name` an address of the pool and says which index it has.
    fn assign(&self, name: &str) -> u32 {
        self.rig
            .tun
            .pool()
            .expect("the lab's TUN has a pool")
            .assign(name, Instant::now())
            .expect("an address for the name")
    }

    /// Every echo request the pinger was asked for.
    fn pinged(&self) -> Vec<(IpAddr, Vec<u8>)> {
        self.extras.pinger.pinged()
    }

    /// From now on the settings of the generation in use have `icmp-forwarding`
    /// as `on` says.
    fn forwarding(&self, on: bool) {
        self.extras.hooks.set(TunSettings {
            resolver: self.resolver.clone(),
            icmp_forwarding: on,
        });
    }

    /// The app sends an echo request.
    fn ping(&self, from: IpAddr, to: IpAddr, ident: u16, seq: u16, payload: &[u8]) {
        self.rig
            .app
            .inject(packets::echo_request(from, to, ident, seq, payload));
    }

    /// The next packet of ICMP for the app, as it came.
    async fn packet(&mut self) -> Vec<u8> {
        within("an ICMP packet for the app", self.icmp.recv())
            .await
            .expect("the app's stack is there")
    }

    /// The packets of the next reply for the app: one, or the fragments of one.
    async fn reply_packets(&mut self) -> Vec<Vec<u8>> {
        let mut pieces = vec![self.packet().await];
        loop {
            // a packet that is whole says what it is
            if pieces.len() == 1 && packets::read_icmp(&pieces[0]).is_some() {
                return pieces;
            }
            if packets::reassemble(&pieces).is_some() {
                return pieces;
            }
            pieces.push(self.packet().await);
        }
    }

    /// The next reply for the app, whole (put together when it came in
    /// fragments), with right checksums.
    async fn reply(&mut self) -> IcmpSeen {
        let pieces = self.reply_packets().await;
        let whole = match &pieces[..] {
            [only] => only.clone(),
            _ => packets::reassemble(&pieces).expect("the fragments of one reply"),
        };
        packets::read_icmp(&whole).expect("an echo reply with right checksums")
    }

    /// Whether no packet is waiting for the app.
    fn no_reply(&mut self) -> bool {
        matches!(self.icmp.try_recv(), Err(TryRecvError::Empty))
    }

    /// Waits for the pinger to have been asked `count` times in all.
    async fn pinged_times(&self, count: usize) {
        eventually("the pinger to be asked", WITHIN, || {
            self.pinged().len() == count
        })
        .await;
    }

    /// Waits for the tasks of the echo requests to end. Only once they have
    /// begun, which `pinged_times` or `dealt_with` is the proof of: before,
    /// there is nothing to wait for.
    async fn tasks_end(&self) {
        eventually("the echo tasks to end", WITHIN, || {
            self.rig.tun.session_tasks() == 0
        })
        .await;
    }

    /// Nothing was answered before this: an echo request to an address of
    /// rurge's own is answered by the receive task at once, so its reply
    /// comes after whatever the writer was handed before it, and is the next
    /// one if there was nothing.
    async fn nothing_came_first(&mut self) {
        self.ping(APP4, OWN4, MARKER, 0, b"marker");
        assert_eq!(self.reply().await, echoed(OWN4, APP4, MARKER, 0, b"marker"));
    }

    /// Every echo request sent so far has been dealt with, and none of them
    /// answered but the ones the test has read. The first marker shows that the
    /// receive task has read them all, and so has begun their tasks; the
    /// second, that none of the tasks, once they have ended, wrote a reply.
    async fn dealt_with(&mut self) {
        self.nothing_came_first().await;
        self.tasks_end().await;
        self.nothing_came_first().await;
    }
}

// ---- the request goes out and the reply comes back

/// An echo request to a real address goes to the pinger as it is, and what the
/// pinger answers comes back from that address with the identifier and the
/// sequence number of the app's.
async fn a_real_address_is_pinged(app: IpAddr, far: IpAddr) {
    let mut lab = lab(TunOptions::default());
    lab.extras.pinger.script(far, PingScript::Echo);
    let requests: [(u16, u16, &[u8]); 2] = [(0x1234, 7, b"hello, ping"), (0xbeef, 65535, b"")];
    for (ident, seq, payload) in requests {
        lab.ping(app, far, ident, seq, payload);
        assert_eq!(lab.reply().await, echoed(far, app, ident, seq, payload));
    }
    assert_eq!(
        lab.pinged(),
        [(far, b"hello, ping".to_vec()), (far, Vec::new())]
    );
    assert_eq!(lab.counters(), Counters::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ping_to_a_real_address_goes_through_the_pinger_v4() {
    a_real_address_is_pinged(APP4, FAR4).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ping_to_a_real_address_goes_through_the_pinger_v6() {
    a_real_address_is_pinged(APP6, FAR6).await;
}

/// The reply is the pinger's: what it answers comes back, not what the app
/// sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_reply_carries_the_payload_the_pinger_gave() {
    let mut lab = lab(TunOptions::default());
    lab.extras
        .pinger
        .script(FAR4, PingScript::Reply(b"what the pinger heard"));
    lab.extras.pinger.script(FAR6, PingScript::Reply(b""));
    lab.ping(APP4, FAR4, 5, 6, b"what the app said");
    assert_eq!(
        lab.reply().await,
        echoed(FAR4, APP4, 5, 6, b"what the pinger heard")
    );
    lab.ping(APP6, FAR6, 7, 8, b"what the app said");
    assert_eq!(lab.reply().await, echoed(FAR6, APP6, 7, 8, b""));
    assert_eq!(lab.counters(), Counters::default());
}

/// A request to a fake address goes to an address of its name, the same
/// family as the request when the name has one, else the other; the reply
/// comes from the fake address.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ping_to_a_fake_address_goes_to_its_names_address() {
    let mut lab = lab(TunOptions::default());
    let (a, b, c) = (
        lab.assign("a.test"),
        lab.assign("b.test"),
        lab.assign("c.test"),
    );
    // a has both kinds, an IPv6 address first; b only IPv4, c only IPv6
    lab.resolver.insert(
        "a.test",
        vec![ip("2001:db8::55"), ip("192.0.2.55"), ip("192.0.2.56")],
    );
    lab.resolver.insert("b.test", vec![ip("192.0.2.77")]);
    lab.resolver.insert("c.test", vec![ip("2001:db8::77")]);
    let cases = [
        // the first address of the request's own family
        (APP4, IpAddr::V4(v4_of(a)), "192.0.2.55"),
        (APP6, IpAddr::V6(v6_of(a)), "2001:db8::55"),
        // only addresses of the other family
        (APP6, IpAddr::V6(v6_of(b)), "192.0.2.77"),
        (APP4, IpAddr::V4(v4_of(c)), "2001:db8::77"),
    ];
    for (number, (app, fake, address)) in cases.into_iter().enumerate() {
        let address = ip(address);
        lab.extras.pinger.script(address, PingScript::Echo);
        let ident = 100 + number as u16;
        lab.ping(app, fake, ident, 1, b"hello, name");
        assert_eq!(
            lab.reply().await,
            echoed(fake, app, ident, 1, b"hello, name"),
            "to {fake}"
        );
        assert_eq!(
            lab.pinged().last(),
            Some(&(address, b"hello, name".to_vec())),
            "to {fake}"
        );
    }
    assert_eq!(lab.pinged().len(), 4);
    assert_eq!(lab.counters(), Counters::default());
}

/// The addresses of rurge's own answer here: the pinger is not asked, and the
/// reply is from the address that was pinged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ping_to_rurge_itself_is_answered_here() {
    let mut lab = lab(TunOptions::default());
    let cases = [
        (APP4, "198.18.0.2"),
        (APP4, "198.18.0.244"),
        (APP4, "198.18.0.253"),
        (APP6, "fd00:6152::2"),
        (APP6, "fd00:6152::fd"),
    ];
    for (number, (app, own)) in cases.into_iter().enumerate() {
        let own = ip(own);
        let (ident, seq) = (0x0100 + number as u16, 0x0200 + number as u16);
        lab.ping(app, own, ident, seq, b"are you there");
        assert_eq!(
            lab.reply().await,
            echoed(own, app, ident, seq, b"are you there"),
            "to {own}"
        );
    }
    assert!(lab.pinged().is_empty());
    lab.tasks_end().await;
    assert_eq!(lab.counters(), Counters::default());
}

/// A request that carries IP options has its ICMP message after them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_echo_request_with_ip_options_is_read_after_them() {
    let mut lab = lab(TunOptions::default());
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    for to in [FAR4, OWN4] {
        let request = packets::echo_request(APP4, to, 0x0a0b, 0x0c0d, b"with options");
        lab.rig.app.inject(with_ip_options(&request, &[1, 1, 1, 1]));
        assert_eq!(
            lab.reply().await,
            echoed(to, APP4, 0x0a0b, 0x0c0d, b"with options"),
            "to {to}"
        );
    }
    assert_eq!(lab.pinged(), [(FAR4, b"with options".to_vec())]);
}

// ---- what is not answered

/// `icmp-forwarding` is read from the settings of the generation in use for
/// each request. With it off every echo request is dropped and counted as
/// unsupported, the ones to an address of rurge's own among them (the design's
/// order of the checks); the next request sees the setting as it is then.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn icmp_forwarding_off_drops_echo_requests() {
    let mut lab = lab(TunOptions::default());
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    lab.extras.pinger.script(FAR6, PingScript::Echo);
    lab.forwarding(false);
    lab.ping(APP4, FAR4, 1, 1, b"off");
    lab.ping(APP6, FAR6, 1, 2, b"off");
    lab.ping(APP4, OWN4, 1, 3, b"off");
    eventually("the requests to be counted", WITHIN, || {
        lab.counters().unsupported == 3
    })
    .await;
    assert!(lab.pinged().is_empty());
    // the next request is read with the settings of its own time, and nothing
    // was answered before it
    lab.forwarding(true);
    lab.ping(APP4, FAR4, 2, 4, b"on");
    assert_eq!(lab.reply().await, echoed(FAR4, APP4, 2, 4, b"on"));
    assert_eq!(lab.pinged(), [(FAR4, b"on".to_vec())]);
    assert_eq!(
        lab.counters(),
        Counters {
            unsupported: 3,
            ..Counters::default()
        }
    );
}

/// An address of the pool's space that no name has names nothing: the request
/// is dropped and counted, whether the TUN has a pool or not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_fake_address_is_not_pinged() {
    for pool in [Some(Arc::new(FakeIpPool::new())), None] {
        let had_pool = pool.is_some();
        let mut lab = lab_with(
            TunOptions::default(),
            Extras {
                pool,
                ..Extras::default()
            },
        );
        lab.ping(APP4, IpAddr::V4(v4_of(7)), 1, 1, b"who?");
        eventually("the request to be counted", WITHIN, || {
            lab.counters().unknown_fake == 1
        })
        .await;
        lab.ping(APP6, IpAddr::V6(v6_of(7)), 1, 2, b"who?");
        eventually("the request to be counted", WITHIN, || {
            lab.counters().unknown_fake == 2
        })
        .await;
        lab.dealt_with().await;
        assert!(lab.pinged().is_empty(), "(a pool: {had_pool})");
        assert_eq!(
            lab.counters(),
            Counters {
                unknown_fake: 2,
                ..Counters::default()
            },
            "(a pool: {had_pool})"
        );
    }
}

/// A name the resolver cannot resolve, or resolves to no address, is dropped
/// without a count (only a line at debug level, see below).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_name_that_does_not_resolve_is_not_pinged() {
    let mut lab = lab(TunOptions::default());
    // the resolver knows no such name as lost.test
    let lost = lab.assign("lost.test");
    let empty = lab.assign("empty.test");
    lab.resolver.insert("empty.test", Vec::new());
    lab.ping(APP4, IpAddr::V4(v4_of(lost)), 1, 1, b"anyone?");
    lab.ping(APP6, IpAddr::V6(v6_of(empty)), 1, 2, b"anyone?");
    // what must not happen: a reply
    tokio::time::sleep(NOTHING).await;
    assert!(lab.no_reply());
    assert!(lab.pinged().is_empty());
    lab.tasks_end().await;
    assert_eq!(lab.counters(), Counters::default());
}

/// A pinger that fails answers nothing, and nothing is counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_pinger_answers_nothing() {
    let mut lab = lab(TunOptions::default());
    // FAR6 is not scripted, and the fake pinger fails what it was told nothing about
    lab.extras.pinger.script(FAR4, PingScript::Fail);
    lab.ping(APP4, FAR4, 1, 1, b"anyone?");
    lab.ping(APP6, FAR6, 1, 2, b"anyone?");
    lab.pinged_times(2).await;
    lab.dealt_with().await;
    assert_eq!(lab.counters(), Counters::default());
}

/// A reply that does not fit an IP packet is not written: the longest does,
/// one byte more does not. The pinger is a trait whose implementations are
/// not this crate's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_longest_reply_is_carried_and_a_longer_one_dropped() {
    static BYTES: [u8; 65_528] = [0x5a; 65_528];
    let mut lab = lab(TunOptions::default());
    // an IPv4 packet is at most 65535 bytes with a header of 20 and the ICMP
    // header of 8; IPv6's payload is at most 65535 with the ICMP header in it
    for (app, far, longest) in [(APP4, FAR4, 65_507), (APP6, FAR6, 65_527)] {
        lab.extras
            .pinger
            .script(far, PingScript::Reply(&BYTES[..longest]));
        lab.ping(app, far, 1, 1, b"x");
        let seen = lab.reply().await;
        let IcmpSeen::EchoReply { payload, .. } = seen else {
            panic!("an echo reply, not {seen:?}");
        };
        assert!(
            payload == BYTES[..longest],
            "to {far}: a payload of {}",
            payload.len()
        );
        lab.extras
            .pinger
            .script(far, PingScript::Reply(&BYTES[..longest + 1]));
        lab.ping(app, far, 1, 2, b"x");
        lab.dealt_with().await;
    }
    assert_eq!(lab.counters(), Counters::default());
}

// ---- time, places and the stop

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
    lab.extras.hooks.set(TunSettings {
        resolver: Arc::new(Slow),
        icmp_forwarding: true,
    });
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

// ---- a reply larger than the device takes

/// A reply, and a request, larger than the device's MTU: the reply is written
/// as the fragments of one packet, which the app puts together.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_larger_than_the_mtu_is_fragmented() {
    let mut lab = lab(TunOptions::default());
    let payload: Vec<u8> = (0..3000).map(|at| (at % 251) as u8).collect();
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    lab.extras.pinger.script(FAR6, PingScript::Echo);
    // an ICMP message of 3008 bytes is 3 fragments on a device of 1500 bytes
    for (app, to) in [
        (APP4, FAR4),
        (APP6, FAR6),
        (APP4, OWN4),
        (APP6, ip("fd00:6152::fd")),
    ] {
        lab.ping(app, to, 9, 9, &payload);
        let pieces = lab.reply_packets().await;
        assert_eq!(pieces.len(), 3, "to {to}");
        for piece in &pieces {
            assert!(piece.len() <= 1500, "to {to}: a packet of {}", piece.len());
        }
        let whole = packets::reassemble(&pieces).expect("the fragments of one reply");
        assert_eq!(
            packets::read_icmp(&whole),
            Some(echoed(to, app, 9, 9, &payload)),
            "to {to}"
        );
    }
    assert_eq!(lab.counters(), Counters::default());
}

// ---- the writer's queue

/// A lab on a device that writes nothing until `open`, and a writer queue of
/// one batch.
fn gated() -> (Lab, Arc<GatedDevice>) {
    let (gate, end) = GatedDevice::pair();
    let options = TunOptions {
        writer_queue: 1,
        ..TunOptions::default()
    };
    let lab = lab_on(gate.clone(), end, options, Extras::default());
    (lab, gate)
}

/// The receive task hands the writer a local answer without waiting: when the
/// queue is full the answer is dropped and counted, and nothing else stops.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_writer_queue_drops_a_local_answer_and_counts_it() {
    let (mut lab, gate) = gated();
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    let senders: u64 = 20;
    for seq in 0..senders {
        lab.ping(APP4, OWN4, 1, seq as u16, b"x");
    }
    // a request that comes after them all, and is answered when the writer
    // takes anything again
    lab.ping(APP4, FAR4, MARKER, 0, b"marker");
    lab.pinged_times(1).await;
    // the writer holds one batch in its write and the queue one more
    let lost = lab.counters().queue_full;
    assert!(
        lost >= senders - 2,
        "{lost} answers were lost, at least {} were to be",
        senders - 2
    );
    gate.open();
    // the writer wrote in order: every answer it took came before the marker
    let mut written = 0;
    loop {
        match lab.reply().await {
            IcmpSeen::EchoReply { ident: MARKER, .. } => break,
            IcmpSeen::EchoReply { src, .. } => {
                assert_eq!(src, OWN4);
                written += 1;
            }
            other => panic!("an echo reply, not {other:?}"),
        }
    }
    assert_eq!(
        written + lost,
        senders,
        "every answer was written or counted"
    );
    assert_eq!(
        lab.counters(),
        Counters {
            queue_full: lost,
            ..Counters::default()
        }
    );
}

/// The task of a request that went out waits for room in the writer's queue to
/// write its reply: nothing is lost, and the tasks wait, however long it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forwarded_reply_waits_for_room_in_the_writer_queue() {
    let (mut lab, gate) = gated();
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    for seq in 0..5 {
        lab.ping(APP4, FAR4, 1, seq, b"x");
    }
    lab.pinged_times(5).await;
    // the writer holds one reply in its write and the queue one more: the
    // other three wait
    eventually("three replies to wait for room", WITHIN, || {
        lab.rig.tun.session_tasks() == 3
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert_eq!(
        lab.rig.tun.session_tasks(),
        3,
        "the replies did not wait for room"
    );
    gate.open();
    let mut seqs = Vec::new();
    for _ in 0..5 {
        match lab.reply().await {
            IcmpSeen::EchoReply { seq, .. } => seqs.push(seq),
            other => panic!("an echo reply, not {other:?}"),
        }
    }
    seqs.sort_unstable();
    assert_eq!(seqs, [0, 1, 2, 3, 4]);
    lab.tasks_end().await;
    assert_eq!(lab.counters(), Counters::default());
}

/// A reply that waits for room in the writer's queue does not wait on past the
/// stop: the writer is stuck in its write, which it gives up a second after the
/// stop (`STOP_GRACE`), and the tasks of the replies are gone long before, so
/// that the writer's is the last task there is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_ends_a_reply_that_waits_for_room() {
    let (lab, _gate) = gated();
    lab.extras.pinger.script(FAR4, PingScript::Echo);
    for seq in 0..5 {
        lab.ping(APP4, FAR4, 1, seq, b"x");
    }
    lab.pinged_times(5).await;
    eventually("three replies to wait for room", WITHIN, || {
        lab.rig.tun.session_tasks() == 3
    })
    .await;
    lab.rig.tun.stop();
    eventually(
        "the writer to be the only task left",
        STOP_WELL_BEFORE,
        || lab.rig.tun.live_tasks() == 1,
    )
    .await;
    let ending = in_five_seconds("the TUN to end", lab.rig.tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
}

// ---- what is said

/// What went wrong with an echo request is said at debug level, with the name
/// or the address; nothing above debug level has an address or a name in it,
/// and the only line above it is the one the drop counter says once for a
/// reason.
#[tokio::test]
async fn a_failed_echo_is_said_at_debug_and_no_address_is_said_above() {
    let log = LogCapture::start();
    let lab = lab(TunOptions {
        echo_timeout: Duration::from_millis(100),
        ..TunOptions::default()
    });
    let lost = lab.assign("lost.test");
    let empty = lab.assign("empty.test");
    lab.resolver.insert("empty.test", Vec::new());
    lab.extras.pinger.script(FAR4, PingScript::Fail);
    lab.extras.pinger.script(FAR6, PingScript::Silent);
    lab.ping(APP4, IpAddr::V4(v4_of(lost)), 1, 1, b"x");
    lab.ping(APP6, IpAddr::V6(v6_of(empty)), 1, 2, b"x");
    lab.ping(APP4, FAR4, 1, 3, b"x");
    lab.ping(APP6, FAR6, 1, 4, b"x");
    lab.pinged_times(2).await;
    lab.tasks_end().await;
    // two requests dropped for the setting: the first is said at WARN, the
    // second at debug level
    lab.forwarding(false);
    lab.ping(APP4, FAR4, 1, 5, b"x");
    lab.ping(APP4, FAR4, 1, 6, b"x");
    eventually("the requests to be counted", WITHIN, || {
        lab.counters().unsupported == 2
    })
    .await;
    let said = |wanted: &[&str]| {
        log.lines().into_iter().any(|line| {
            line.starts_with("DEBUG") && wanted.iter().all(|piece| line.contains(piece))
        })
    };
    assert!(said(&["lost.test"]), "{:?}", log.lines());
    assert!(said(&["empty.test"]), "{:?}", log.lines());
    assert!(
        said(&["192.0.2.10", "scripted failure"]),
        "{:?}",
        log.lines()
    );
    assert!(said(&["2001:db8::a"]), "{:?}", log.lines());
    assert!(said(&["unsupported", "192.0.2.10"]), "{:?}", log.lines());
    let lines = log.lines();
    let above: Vec<&String> = lines
        .iter()
        .filter(|line| !line.starts_with("DEBUG") && !line.starts_with("TRACE"))
        .collect();
    assert_eq!(above, [&warning(Reason::Unsupported)], "{lines:?}");
}
