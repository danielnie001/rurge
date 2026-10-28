//! The writer's two queues (phase 3 M5 design 7.2): a batch for the UDP
//! datagrams the TUN writes back is taken before the rest of what waits, a
//! queue that has closed does not hold the other up, and the writer ends only
//! once both have closed. The first tests start `writer::run` by hand and fill
//! its queues themselves; the ones after them look at where the TUN's own
//! senders put what they send.

use crate::device::{AppEnd, Device, MemoryDevice};
use crate::testing::packets::IcmpSeen;
use crate::testing::{FakeDialer, FakeResolve, packets};
use crate::tests::hazards::app4;
use crate::tests::support::{
    Extras, WITHIN, addr, eventually, in_five_seconds, settings, syn_v4, tcp_header, tun_dialing,
};
use crate::writer::{self, STOP_GRACE};
use crate::{Counters, Ending, Life, TunHandle, TunOptions, TunSettings};
use rurge_config::{HostName, PreMatchLayer};
use rurge_dns::testing::query_wire;
use rurge_net::BoxFuture;
use rurge_net::connector::Target;
use std::io;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, mpsc::error::TrySendError, watch};

/// A device that records the packets of each write it is given, in the order it
/// takes them, and says that a write began before it takes it. What it reads
/// comes from the app end of the memory device it wraps; nothing reaches that
/// end, so a test sees what was written in [`Recording::writes`] alone. With a
/// gate it takes nothing until the gate is opened.
struct Recording {
    inner: MemoryDevice,
    writes: Mutex<Vec<Vec<Vec<u8>>>>,
    begun: mpsc::UnboundedSender<()>,
    gate: Option<watch::Sender<bool>>,
}

impl Recording {
    /// A device with an MTU of 1500 and room for 4096 packets each way, the app
    /// end of it, and what a write that begins is told to; `gated` for a device
    /// that takes nothing until [`Recording::open`].
    fn pair(gated: bool) -> (Arc<Recording>, AppEnd, mpsc::UnboundedReceiver<()>) {
        let (inner, end) = MemoryDevice::pair(1500, 4096);
        let (begun, began) = mpsc::unbounded_channel();
        let device = Recording {
            inner,
            writes: Mutex::new(Vec::new()),
            begun,
            gate: gated.then(|| watch::channel(false).0),
        };
        (Arc::new(device), end, began)
    }

    /// The packets of each write it has taken, in order.
    fn writes(&self) -> Vec<Vec<Vec<u8>>> {
        self.writes.lock().expect("the writes").clone()
    }

    /// From now on it takes the writes it is given and the ones it held back.
    fn open(&self) {
        if let Some(gate) = &self.gate {
            gate.send_replace(true);
        }
    }
}

impl Device for Recording {
    fn recv<'a>(&'a self, into: &'a mut Vec<Vec<u8>>, max: usize) -> BoxFuture<'a, io::Result<()>> {
        self.inner.recv(into, max)
    }

    fn send<'a>(&'a self, packets: &'a mut Vec<Vec<u8>>) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // said before the gate: a test that waits for it knows the write
            // has begun, and everything it sends from then on waits in the
            // queues
            let _ = self.begun.send(());
            if let Some(gate) = &self.gate {
                let mut opened = gate.subscribe();
                if opened.wait_for(|open| *open).await.is_err() {
                    return Err(io::Error::other("the gate is gone"));
                }
            }
            self.writes
                .lock()
                .expect("the writes")
                .push(std::mem::take(packets));
            Ok(())
        })
    }

    fn mtu(&self) -> usize {
        self.inner.mtu()
    }
}

/// The sending ends of a writer's two queues, in the test's hands.
struct Queues {
    priority: mpsc::Sender<Vec<Vec<u8>>>,
    normal: mpsc::Sender<Vec<Vec<u8>>>,
}

/// A writer started by hand on a recording device, and the two queues it takes
/// from.
struct Writer {
    device: Arc<Recording>,
    begun: mpsc::UnboundedReceiver<()>,
    life: Arc<Life>,
    task: tokio::task::JoinHandle<()>,
}

impl Writer {
    /// A writer on a device that takes every write, or on one that takes
    /// nothing until it is opened, with two queues of `capacity` batches and at
    /// most `limit` of them taken at a time.
    fn start(capacity: usize, limit: usize, gated: bool) -> (Writer, Queues) {
        let (device, _end, begun) = Recording::pair(gated);
        let (priority, from_priority) = mpsc::channel(capacity);
        let (normal, from_normal) = mpsc::channel(capacity);
        let life = Arc::new(Life::default());
        let task = tokio::spawn(writer::run(
            device.clone(),
            from_priority,
            from_normal,
            limit,
            life.clone(),
        ));
        let writer = Writer {
            device,
            begun,
            life,
            task,
        };
        (writer, Queues { priority, normal })
    }

    /// Waits for a write to begin, which says that the writer has taken what
    /// its queues held up to now.
    async fn begun(&mut self) {
        in_five_seconds("a write to begin", self.begun.recv())
            .await
            .expect("the writer's device is there");
    }

    /// Waits for the writer to end, as it must once both queues have closed.
    async fn ended(self) {
        in_five_seconds("the writer to end", self.task)
            .await
            .expect("the writer's task");
    }
}

/// Batches in both queues before the writer first looks: they come to one
/// write, the priority queue's packets first and then the normal queue's in the
/// order they were put in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn priority_batches_go_first_in_the_next_write() {
    let (writer, queues) = Writer::start(8, 8, false);
    for batch in [b"n1", b"n2", b"n3"] {
        queues
            .normal
            .send(vec![batch.to_vec()])
            .await
            .expect("the queue has room");
    }
    queues
        .priority
        .send(vec![b"p1".to_vec()])
        .await
        .expect("the queue has room");
    eventually("the first write", WITHIN, || {
        !writer.device.writes().is_empty()
    })
    .await;
    assert_eq!(
        writer.device.writes(),
        [vec![
            b"p1".to_vec(),
            b"n1".to_vec(),
            b"n2".to_vec(),
            b"n3".to_vec()
        ]],
        "one write, the priority queue's batch first"
    );
    drop(queues);
    writer.ended().await;
}

/// One queue alone keeps the order the batches came in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_queue_alone_keeps_the_order_of_arrival() {
    let (writer, queues) = Writer::start(8, 8, false);
    for batch in [b"n1", b"n2", b"n3"] {
        queues
            .normal
            .send(vec![batch.to_vec()])
            .await
            .expect("the queue has room");
    }
    eventually("the first write", WITHIN, || {
        !writer.device.writes().is_empty()
    })
    .await;
    assert_eq!(
        writer.device.writes(),
        [vec![b"n1".to_vec(), b"n2".to_vec(), b"n3".to_vec()]],
        "the three batches in the order they came"
    );
    drop(queues);
    writer.ended().await;
}

/// The two queues are filled apart: with the writer stuck in a write and the
/// normal queue full, the priority queue still takes a batch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_normal_queue_leaves_room_in_the_priority_queue() {
    let (mut writer, queues) = Writer::start(1, 1, true);
    queues
        .normal
        .send(vec![b"n1".to_vec()])
        .await
        .expect("the queue has room");
    // the writer has taken the first batch and waits in the write for the gate
    writer.begun().await;
    // the normal queue has room for one more batch, and no more
    queues
        .normal
        .send(vec![b"n2".to_vec()])
        .await
        .expect("the queue has room");
    assert!(
        matches!(
            queues.normal.try_send(vec![b"n3".to_vec()]),
            Err(TrySendError::Full(_))
        ),
        "the normal queue is full"
    );
    queues
        .priority
        .try_send(vec![b"p1".to_vec()])
        .expect("the priority queue has room of its own");
    writer.device.open();
    eventually("the second write", WITHIN, || {
        writer.device.writes().len() == 2
    })
    .await;
    assert_eq!(
        writer.device.writes(),
        [vec![b"n1".to_vec()], vec![b"p1".to_vec(), b"n2".to_vec()]],
        "the priority queue's batch first in the write it is in"
    );
    drop(queues);
    writer.ended().await;
}

/// A queue that has closed does not end the writer while the other is open: it
/// writes what comes to the other one, and its task goes on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_writer_goes_on_with_one_queue_closed() {
    let (writer, queues) = Writer::start(4, 4, false);
    let Queues { priority, normal } = queues;
    // the only sender of the priority queue
    drop(priority);
    for (batch, written) in [(b"n1", 1), (b"n2", 2)] {
        normal
            .send(vec![batch.to_vec()])
            .await
            .expect("the queue has room");
        eventually("the batch to be written", WITHIN, || {
            writer.device.writes().len() == written
        })
        .await;
    }
    assert_eq!(
        writer.device.writes(),
        [vec![b"n1".to_vec()], vec![b"n2".to_vec()]],
        "both batches of the open queue"
    );
    assert!(
        !writer.task.is_finished(),
        "the writer ended with a queue still open"
    );
    drop(normal);
    writer.ended().await;
}

/// Both queues closed is the end: the writer's task is over at once, and
/// nothing has to be written for it to be.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_writer_ends_when_both_queues_have_closed() {
    let (writer, queues) = Writer::start(4, 4, false);
    drop(queues);
    writer.ended().await;
}

/// The grace is the writer's, not a queue's: after the stop, what waits in
/// either queue is written while the device still takes it, within
/// `STOP_GRACE`, the priority queue's batches first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stop_grace_holds_for_both_queues() {
    let (mut writer, queues) = Writer::start(8, 8, true);
    queues
        .normal
        .send(vec![b"n1".to_vec()])
        .await
        .expect("the queue has room");
    // the writer is stuck in the write of the first batch
    writer.begun().await;
    queues
        .priority
        .send(vec![b"p1".to_vec()])
        .await
        .expect("the queue has room");
    queues
        .normal
        .send(vec![b"n2".to_vec()])
        .await
        .expect("the queue has room");
    queues
        .priority
        .send(vec![b"p2".to_vec()])
        .await
        .expect("the queue has room");
    assert!(writer.life.end(Ending::Stopped), "the TUN was not ending");
    writer.device.open();
    eventually(
        "both queues to be written within the grace",
        STOP_GRACE,
        || writer.device.writes().len() == 2,
    )
    .await;
    assert_eq!(
        writer.device.writes(),
        [
            vec![b"n1".to_vec()],
            vec![b"p1".to_vec(), b"p2".to_vec(), b"n2".to_vec()]
        ],
        "what waited in either queue, the priority queue's first"
    );
    drop(queues);
    writer.ended().await;
}

// ---- where the TUN's own senders put what they send

/// The app's endpoint in the tests below, and the two places it talks to: one
/// only a TCP destination, one only a UDP one.
const APP: &str = "198.18.0.1:5000";
const TCP_FAR: &str = "192.0.2.1:80";
const UDP_FAR: &str = "192.0.2.10:7";

/// A TUN on a device of the test's, which records the packets of every write
/// and takes nothing until it is opened. The app's packets go in with
/// [`Watched::send`], and nothing consumes what the TUN writes but the
/// recording.
struct Watched {
    tun: TunHandle,
    dialer: Arc<FakeDialer>,
    device: Arc<Recording>,
    app: AppEnd,
    begun: mpsc::UnboundedReceiver<()>,
}

impl Watched {
    /// A TUN of `options`'s shards on the recording device, with `extras`.
    fn start(options: TunOptions, extras: &Extras) -> Watched {
        let (device, app, begun) = Recording::pair(true);
        let dialer = FakeDialer::new();
        let tun = tun_dialing(device.clone(), dialer.clone(), extras, options);
        Watched {
            tun,
            dialer,
            device,
            app,
            begun,
        }
    }

    /// The app sends `packet` through the device.
    async fn send(&self, packet: Vec<u8>) {
        self.app
            .send(packet)
            .await
            .expect("the device takes the app's packet");
    }

    /// Waits for a write to begin: the writer has taken what its queues held up
    /// to now.
    async fn begun(&mut self) {
        in_five_seconds("a write to begin", self.begun.recv())
            .await
            .expect("the writer's device is there");
    }

    /// The packets of every write, in the order they were written.
    fn order(&self) -> Vec<Vec<u8>> {
        self.device.writes().into_iter().flatten().collect()
    }
}

/// The port of the app end of `packet`, if it is a TCP segment with the SYN and
/// the ACK flags: the port the TUN answered a SYN from.
fn syn_ack_to(packet: &[u8]) -> Option<u16> {
    let (header, _) = tcp_header(packet)?;
    let flags = packet[header + 13];
    (flags & 0x12 == 0x12).then(|| u16::from_be_bytes([packet[header + 2], packet[header + 3]]))
}

/// The index of the first of `order` for which `find` holds; `what` is what was
/// looked for, for the message when there is none.
fn index_of(order: &[Vec<u8>], what: &str, find: impl Fn(&[u8]) -> bool) -> usize {
    order
        .iter()
        .position(|packet| find(packet))
        .unwrap_or_else(|| panic!("no {what} was written"))
}

/// Whether `packet` is the reply of the session, which is the payload "reply".
fn is_the_reply(packet: &[u8]) -> bool {
    packets::read_udp(packet).is_some_and(|seen| seen.payload == b"reply")
}

/// Whether `packet` is the SYN-ACK a connection from the app's port `port` is
/// answered with.
fn syn_ack_to_the_app(port: u16, packet: &[u8]) -> bool {
    syn_ack_to(packet) == Some(port)
}

/// The setup of the two tests of a reply's turn: the writer stuck in a
/// connection's SYN-ACK, a second connection's SYN-ACK in its queue and a third
/// one's waiting for room in it, and a UDP session whose reply the test sends.
///
/// The second connection's segment is in the queue once the third's SYN has
/// been taken: the shard hands a turn's output to the writer before it takes
/// the next turn, so with the queue full the segment of the second came first.
async fn clogged(watched: &mut Watched) {
    let far = addr(TCP_FAR);
    watched.send(syn_v4(app4(40_000), far)).await;
    // the first SYN-ACK is the write the writer is stuck in
    watched.begun().await;
    watched.send(syn_v4(app4(40_001), far)).await;
    eventually("the second connection", WITHIN, || {
        watched.tun.sockets_counted() == 2
    })
    .await;
    watched.send(syn_v4(app4(40_002), far)).await;
    eventually("the third connection", WITHIN, || {
        watched.tun.sockets_counted() == 3
    })
    .await;
    // a UDP session, for the reply the test sends
    let (app, far) = (addr(APP), addr(UDP_FAR));
    watched.send(packets::udp(app, far, b"hello")).await;
    eventually("the association to be open", WITHIN, || {
        watched.dialer.associations_open() == 1
    })
    .await;
}

/// The reply the session has for the app, as the dialer hands it over: it waits
/// for room in the queue it goes to, for as long as that queue is full.
async fn reply_to_the_app(watched: &Watched) -> io::Result<()> {
    let (app, far) = (addr(APP), addr(UDP_FAR));
    let target = Target::new(HostName::Ip(far.ip()), far.port());
    watched
        .dialer
        .send_to_app(app, target.clone(), Some(target), b"reply".to_vec())
        .await
}

/// Waits until the reply and the SYN-ACKs of the second and third connections
/// have been written.
async fn the_reply_and_the_segments_are_written(watched: &Watched) {
    eventually(
        "the reply and the two segments to be written",
        WITHIN,
        || {
            let order = watched.order();
            order.iter().any(|packet| is_the_reply(packet))
                && order
                    .iter()
                    .any(|packet| syn_ack_to_the_app(40_001, packet))
                && order
                    .iter()
                    .any(|packet| syn_ack_to_the_app(40_002, packet))
        },
    )
    .await;
}

/// The indexes of the reply and of the second connection's SYN-ACK, in the
/// order the packets were written.
fn reply_and_segment(watched: &Watched) -> (usize, usize) {
    let order = watched.order();
    let reply = index_of(&order, "the reply", is_the_reply);
    let segment = index_of(&order, "the second connection's SYN-ACK", |packet| {
        syn_ack_to_the_app(40_001, packet)
    });
    (reply, segment)
}

/// With `udp-priority` on, a session's reply goes ahead of the TCP segments that
/// were waiting for the writer when it came: it is written before the second
/// connection's SYN-ACK, which was in the writer's queue first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_replies_go_ahead_of_tcp_when_udp_priority_is_on() {
    let extras = Extras::default();
    let mut watched = Watched::start(
        TunOptions {
            shards: 1,
            writer_queue: 1,
            ..TunOptions::default()
        },
        &extras,
    );
    clogged(&mut watched).await;
    // the priority queue has room of its own: the reply does not wait for the
    // segments that fill the other one
    reply_to_the_app(&watched)
        .await
        .expect("the reply is taken");
    watched.device.open();
    the_reply_and_the_segments_are_written(&watched).await;
    let (reply, segment) = reply_and_segment(&watched);
    assert!(
        reply < segment,
        "the reply is written at {reply}, before the segment that waited at {segment}"
    );
}

/// With `udp-priority` off, a session's reply waits for room in the queue the
/// TCP segments are in, and keeps its turn: the second connection's SYN-ACK is
/// written before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_replies_keep_their_turn_when_udp_priority_is_off() {
    let extras = Extras::default();
    extras.hooks.set(TunSettings {
        udp_priority: false,
        ..settings(FakeResolve::new(), true)
    });
    let mut watched = Watched::start(
        TunOptions {
            shards: 1,
            writer_queue: 1,
            ..TunOptions::default()
        },
        &extras,
    );
    clogged(&mut watched).await;
    // the queue is full: the reply waits for room, so a task sends it
    let dialer = watched.dialer.clone();
    let sending = tokio::spawn(async move {
        let (app, far) = (addr(APP), addr(UDP_FAR));
        let target = Target::new(HostName::Ip(far.ip()), far.port());
        dialer
            .send_to_app(app, target.clone(), Some(target), b"reply".to_vec())
            .await
    });
    watched.device.open();
    in_five_seconds("the reply to be sent", sending)
        .await
        .expect("the sending task")
        .expect("the reply is sent");
    the_reply_and_the_segments_are_written(&watched).await;
    let (reply, segment) = reply_and_segment(&watched);
    assert!(
        segment < reply,
        "the segment at {segment} is written before the reply at {reply}"
    );
}

/// The responder's answers go in the queue for UDP and the messages the TUN
/// makes itself in the other one: a session's reply is the write in hand, the
/// two ICMP messages wait in the normal queue, and the answer, which comes
/// while they wait, is written before both of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn responder_answers_are_udp_and_icmp_is_not() {
    let extras = Extras::default();
    let mut watched = Watched::start(
        TunOptions {
            shards: 1,
            // for the two messages and the answer: the answer takes no room
            // of the normal queue's, but the messages need theirs
            writer_queue: 2,
            ..TunOptions::default()
        },
        &extras,
    );
    let (app, responder) = (addr(APP), addr("198.18.0.2:53"));
    let own = addr("198.18.0.244:53");
    // a session's reply is the write the writer is stuck in
    watched
        .send(packets::udp(app, addr(UDP_FAR), b"hello"))
        .await;
    eventually("the association to be open", WITHIN, || {
        watched.dialer.associations_open() == 1
    })
    .await;
    reply_to_the_app(&watched)
        .await
        .expect("the reply is taken");
    watched.begun().await;
    // the two messages the TUN makes itself fill the normal queue: a datagram
    // to an address of rurge's own is refused with ICMP, and an echo request to
    // one is answered
    watched.send(packets::udp(app, own, b"x")).await;
    watched
        .send(packets::echo_request(app.ip(), own.ip(), 7, 1, b"ping"))
        .await;
    // a query to the responder's own port 53: its answer is for the queue of
    // UDP, which has room though the normal one holds its two batches
    watched
        .send(packets::udp(
            app,
            responder,
            &query_wire(1, "a.test", 1, None),
        ))
        .await;
    eventually("the query to reach the responder", WITHIN, || {
        extras
            .hooks
            .pre_matches_asked()
            .iter()
            .any(|(layer, _)| *layer == PreMatchLayer::Dns)
    })
    .await;
    // the TUN's tasks but the session's are over: the answer was handed over,
    // or dropped and counted
    eventually("the task of the query to end", WITHIN, || {
        watched.tun.session_tasks() == 1
    })
    .await;
    assert_eq!(
        watched.tun.counters(),
        Counters::default(),
        "the answer was dropped, as one for the normal queue would be"
    );
    watched.device.open();
    eventually("the answer and the two messages to be written", WITHIN, || {
        let order = watched.order();
        order.iter().any(|packet| {
            packets::read_udp(packet).is_some_and(|seen| seen.src == responder && seen.dst == app)
        }) && order.iter().any(|packet| {
            matches!(packets::read_icmp(packet), Some(IcmpSeen::Prohibited { src, .. }) if src == own.ip())
        }) && order.iter().any(|packet| {
            matches!(packets::read_icmp(packet), Some(IcmpSeen::EchoReply { src, .. }) if src == own.ip())
        })
    })
    .await;
    let order = watched.order();
    let answer = index_of(&order, "the responder's answer", |packet| {
        packets::read_udp(packet).is_some_and(|seen| seen.src == responder && seen.dst == app)
    });
    let prohibited = index_of(
        &order,
        "the prohibited message",
        |packet| matches!(packets::read_icmp(packet), Some(IcmpSeen::Prohibited { src, .. }) if src == own.ip()),
    );
    let echo = index_of(
        &order,
        "the echo reply",
        |packet| matches!(packets::read_icmp(packet), Some(IcmpSeen::EchoReply { src, .. }) if src == own.ip()),
    );
    assert!(
        answer < prohibited,
        "the answer is written at {answer}, before the prohibited message at {prohibited}"
    );
    assert!(
        answer < echo,
        "the answer is written at {answer}, before the echo reply at {echo}"
    );
}
