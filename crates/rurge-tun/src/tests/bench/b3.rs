//! The machinery of B3, the latency of a fake answer (phase 3 M3 design 15.3):
//! what a query to the responder costs from the moment `Responder::answer` is
//! given it to the moment the IP packet of the answer is made, which is the work
//! the TUN does for a query. The device and the writer's queue are left out (the
//! design measures "between the responder's entrance and the write-back"), and
//! so are the receive task and the queue of the queries. NFR-01 allows a fake
//! answer 0.2 ms.

use crate::dns::{Ask, Responder};
use crate::fragment::Ids;
use crate::packet::udp_packets;
use crate::testing::FakeResolve;
use crate::testing::packets::read_udp;
use crate::tests::support::{addr, settings};
use crate::{Hooks, TunSettings};
use rurge_config::{PreMatchLayer, SessionInfo, Transport};
use rurge_dns::fakeip::FakeIpPool;
use rurge_dns::message::parse_response;
use rurge_dns::testing::query_wire;
use rurge_proto::RejectKind;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How many different names are asked: each is one A query for a name the pool
/// has not seen.
pub(super) const NAMES: usize = 100_000;

/// What NFR-01 allows a fake answer to take.
pub(super) const LINE: Duration = Duration::from_micros(200);

/// The hooks of the measurement: the settings of a profile with no
/// `always-real-ip` and no `hijack-dns`, and rules that refuse nothing. The fake
/// hooks of the other tests write down every session they are asked about,
/// which a hundred thousand queries would make the larger part of the time.
struct Plain(Arc<TunSettings>);

impl Hooks for Plain {
    fn settings(&self) -> Arc<TunSettings> {
        self.0.clone()
    }

    fn pre_match(&self, _: PreMatchLayer, _: &SessionInfo) -> Option<RejectKind> {
        None
    }
}

/// The time each query took, from the shortest to the longest, and which query
/// was the longest (by the order they were asked in).
pub(super) struct Latencies {
    sorted: Vec<Duration>,
    slowest: usize,
}

impl Latencies {
    fn of(mut times: Vec<Duration>) -> Latencies {
        let slowest = times
            .iter()
            .enumerate()
            .max_by_key(|(_, time)| **time)
            .map_or(0, |(index, _)| index);
        times.sort_unstable();
        Latencies {
            sorted: times,
            slowest,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.sorted.len()
    }

    /// The time that `percent` of the queries took no longer than: the
    /// nearest rank, the smallest time that at least that share of them are
    /// not above.
    pub(super) fn percentile(&self, percent: usize) -> Duration {
        let rank = (self.sorted.len() * percent).div_ceil(100).max(1);
        self.sorted[rank - 1]
    }

    pub(super) fn median(&self) -> Duration {
        self.percentile(50)
    }

    pub(super) fn max(&self) -> Duration {
        *self.sorted.last().expect("at least one query")
    }
}

/// Asks the responder of a TUN for the fake address of each of `names`
/// different names, and times each query: `Responder::answer` and the IP packet
/// of the answer, on the current thread. Every answer is looked at, outside the
/// time: it is one fake address, no other name's, in an IP packet that reads.
pub(super) async fn measure(names: usize) -> Latencies {
    let pool = Arc::new(FakeIpPool::new());
    let hooks = Arc::new(Plain(Arc::new(settings(FakeResolve::new(), true))));
    let responder = Responder::new(hooks, Some(pool));
    let ids = Ids::new();
    let ask = Ask {
        src: addr("198.18.0.1:5000"),
        server: addr("198.18.0.2:53"),
        transport: Transport::Udp,
    };
    let mut times = Vec::with_capacity(names);
    let mut given = HashSet::with_capacity(names);
    for index in 0..names {
        let name = format!("n{index}.example.test");
        let query = query_wire(index as u16, &name, 1, None);
        let began = Instant::now();
        let reply = responder.answer(&query, &ask).await;
        let packets = reply
            .as_deref()
            .and_then(|reply| udp_packets(ask.server, ask.src, reply, 1500, &ids));
        let took = began.elapsed();
        let packets = std::hint::black_box(packets).expect("an answer made into packets");
        assert_eq!(packets.len(), 1, "{name}");
        let seen = read_udp(&packets[0]).expect("a datagram with right checksums");
        assert_eq!((seen.src, seen.dst), (ask.server, ask.src), "{name}");
        let answer = parse_response(&seen.payload).expect("an answer that reads");
        assert_eq!(answer.v4.len(), 1, "{name}");
        assert!(
            given.insert(answer.v4[0].0),
            "{name} has an address that another name has"
        );
        times.push(took);
    }
    Latencies::of(times)
}

/// What B3 prints: the median, the 99th percentile and the longest, in
/// microseconds, against the line of NFR-01.
pub(super) fn report(latencies: &Latencies) {
    let micros = |time: Duration| time.as_secs_f64() * 1e6;
    println!(
        "\nB3 the latency of a fake answer: Responder::answer and the IP packet of the answer, {} different names, one thread, the device and the writer left out",
        latencies.len()
    );
    println!(
        "median {:.1} us, p99 {:.1} us, max {:.1} us (the slowest was query {} of {}); NFR-01 allows {:.1} us",
        micros(latencies.median()),
        micros(latencies.percentile(99)),
        micros(latencies.max()),
        latencies.slowest + 1,
        latencies.len(),
        micros(LINE)
    );
}

/// The ranks: the nearest rank of a percentile is the smallest time that at
/// least that share of the queries are not above.
#[test]
fn the_percentiles_are_nearest_ranks() {
    let ms = Duration::from_millis;
    let hundred = Latencies::of((1..=100).rev().map(ms).collect());
    assert_eq!(hundred.median(), ms(50));
    assert_eq!(hundred.percentile(99), ms(99));
    assert_eq!(hundred.percentile(100), ms(100));
    assert_eq!(hundred.max(), ms(100));
    // five queries: the median is the third, p99 the fifth, and the longest
    // was the first asked
    let five = Latencies::of([5, 1, 4, 2, 3].map(ms).into());
    assert_eq!(five.median(), ms(3));
    assert_eq!(five.percentile(99), ms(5));
    assert_eq!(five.percentile(20), ms(1));
    assert_eq!(five.slowest, 0);
    // one query is every percentile
    let one = Latencies::of(vec![ms(7)]);
    assert_eq!(
        (one.median(), one.percentile(99), one.max()),
        (ms(7), ms(7), ms(7))
    );
}

/// The measurement asks what the TUN asks, and finds what it finds: each of the
/// names gets its own fake address (`measure` looks at every answer).
#[test]
fn b3_measures_a_fake_answer_whole() {
    let latencies = super::director().block_on(measure(300));
    assert_eq!(latencies.len(), 300);
    assert!(latencies.median() <= latencies.percentile(99));
    assert!(latencies.percentile(99) <= latencies.max());
}
