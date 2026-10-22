//! UDP through the TUN (phase 3 M3 design 7): a session for each source
//! endpoint of an app, served by the dialer's association; the destinations in
//! it; what comes back, whole or in fragments; and the ICMP message an app gets
//! for a flow that was refused. The app sends through `AppStack::inject` and
//! reads what the TUN writes through `divert_udp` and `divert_icmp`, so that
//! nothing it reads has been through a stack of its own.

use super::support::{
    Extras, GatedDevice, Rig, WITHIN, addr, eventually, in_five_seconds, rig_extras, rig_with,
    sock, tun_dialing, udp_bytes, with_ip_options, within,
};
use crate::device::MemoryDevice;
use crate::fragment::{Ids, ip_packets};
use crate::icmp::prohibited;
use crate::testing::packets::{self, IcmpSeen, UdpSeen};
use crate::testing::{AppStack, FakeDialer, UdpScript};
use crate::{Counters, Ending, TunOptions};
use rurge_config::{HostName, ListenerKind, SessionInfo, Transport};
use rurge_dns::fakeip::{AssignError, FakeIpPool, v4_of, v6_of};
use rurge_inbound::{
    DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome, UdpAdmission, UdpClient,
};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, Target};
use rurge_proto::RejectKind;
use smoltcp::wire::{
    Icmpv4Message, Icmpv4Packet, Icmpv6Message, Icmpv6Packet, IpProtocol, Ipv4Packet, Ipv6Packet,
};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, mpsc::error::TryRecvError};
use tokio_util::sync::CancellationToken;

/// The one fixed wait of these tests, for what must not happen: an ICMP
/// message or a datagram for the app. It would go through the writer and the
/// app's stack in well under a millisecond; 200 ms is a wide margin.
const NOTHING: Duration = Duration::from_millis(200);

/// The app's endpoints and the places it sends to, one family each.
const APP4: &str = "198.18.0.1:5000";
const APP6: &str = "[fd00:6152::1]:5000";
const FAR4: &str = "192.0.2.10:7";
const FAR6: &str = "[2001:db8::10]:7";

fn target(ip: &str, port: u16) -> Target {
    Target::new(HostName::Ip(ip.parse().unwrap()), port)
}

fn named(name: &str, port: u16) -> Target {
    Target::new(HostName::Domain(name.to_owned()), port)
}

fn target_of(endpoint: SocketAddr) -> Target {
    Target::new(HostName::Ip(endpoint.ip()), endpoint.port())
}

/// What the dialer is given for the session of the app at `app`: where it is
/// from, the TUN as the listener, no endpoint of ours it came to, and the
/// stack's own address of its family as a destination that each flow replaces
/// with its own.
fn template(app: SocketAddr) -> SessionInfo {
    let own: IpAddr = if app.is_ipv4() {
        "198.18.0.253".parse().unwrap()
    } else {
        "fd00:6152::fd".parse().unwrap()
    };
    SessionInfo {
        src: app,
        in_port: 0,
        local: None,
        listener: ListenerKind::Tun,
        dst_host: HostName::Ip(own),
        dst_port: 0,
        transport: Transport::Udp,
        protocol: None,
        sni: None,
        http_host: None,
        user_agent: None,
        url: None,
        process: None,
        device: None,
    }
}

/// A rig whose app end has its UDP and ICMP packets in the test's hands.
struct Lab {
    rig: Rig,
    udp: mpsc::UnboundedReceiver<Vec<u8>>,
    icmp: mpsc::UnboundedReceiver<Vec<u8>>,
}

fn lab(options: TunOptions) -> Lab {
    lab_with(options, Extras::default())
}

fn lab_with(options: TunOptions, extras: Extras) -> Lab {
    let (rig, _) = rig_extras(options, extras);
    taking(rig)
}

/// `rig`, with the app's UDP and ICMP taken.
fn taking(rig: Rig) -> Lab {
    let udp = rig.app.divert_udp();
    let icmp = rig.app.divert_icmp();
    Lab { rig, udp, icmp }
}

/// The options of a test that counts ICMP messages: room in the writer's queue
/// for all of them, since the receive task and `refused` hand them over
/// without waiting and a full queue loses them.
fn counting_icmp() -> TunOptions {
    TunOptions {
        writer_queue: 400,
        ..TunOptions::default()
    }
}

impl Lab {
    fn dialer(&self) -> &FakeDialer {
        &self.rig.dialer
    }

    fn counters(&self) -> Counters {
        self.rig.tun.counters()
    }

    fn pool(&self) -> Arc<FakeIpPool> {
        self.rig.tun.pool().expect("the rig's TUN has a pool")
    }

    /// The app sends a datagram.
    fn send(&self, from: SocketAddr, to: SocketAddr, payload: &[u8]) {
        self.rig.app.inject(packets::udp(from, to, payload));
    }

    /// The next packet of UDP for the app, as it came.
    async fn packet(&mut self) -> Vec<u8> {
        within("a datagram for the app", self.udp.recv())
            .await
            .expect("the app's stack is there")
    }

    /// The next datagram for the app, which comes whole and with right
    /// checksums.
    async fn datagram(&mut self) -> UdpSeen {
        let packet = self.packet().await;
        packets::read_udp(&packet).expect("a datagram with right checksums")
    }

    /// The next `count` packets of UDP for the app, as they came.
    async fn packets(&mut self, count: usize) -> Vec<Vec<u8>> {
        let mut got = Vec::new();
        for _ in 0..count {
            got.push(self.packet().await);
        }
        got
    }

    /// The next ICMP message for the app, which has right checksums.
    async fn icmp(&mut self) -> IcmpSeen {
        let packet = within("an ICMP message for the app", self.icmp.recv())
            .await
            .expect("the app's stack is there");
        packets::read_icmp(&packet).expect("an ICMP message of the two kinds, checksums right")
    }

    /// The next ICMP message, which is the one that says "administratively
    /// prohibited": from where, to where, and what of the packet it quotes.
    async fn prohibited(&mut self) -> (IpAddr, IpAddr, Vec<u8>) {
        match self.icmp().await {
            IcmpSeen::Prohibited { src, dst, quoted } => (src, dst, quoted),
            other => panic!("an administratively prohibited message, not {other:?}"),
        }
    }

    /// Whether no ICMP message is waiting for the app.
    fn no_icmp(&mut self) -> bool {
        matches!(self.icmp.try_recv(), Err(TryRecvError::Empty))
    }

    /// Waits for `count` associations to be open.
    async fn associations_open(&self, count: usize) {
        eventually("the associations to be open", WITHIN, || {
            self.dialer().associations_open() == count
        })
        .await;
    }
}

// ---- the datagram there and back

/// A datagram to a real address goes to the dialer as a datagram for that
/// address, from a session of the app's endpoint, and what the address answers
/// comes back whole, from where it came from.
async fn goes_out_and_comes_back(app: SocketAddr, dst: SocketAddr) {
    let mut lab = lab(TunOptions::default());
    let to = target_of(dst);
    lab.dialer().script_udp(to.clone(), UdpScript::Echo);
    let payloads: [&[u8]; 2] = [b"ping", b""];
    for payload in payloads {
        lab.send(app, dst, payload);
        assert_eq!(
            lab.datagram().await,
            UdpSeen {
                src: dst,
                dst: app,
                payload: payload.to_vec()
            }
        );
    }
    assert_eq!(
        lab.dialer().udp_sent(),
        [(to.clone(), b"ping".to_vec()), (to, Vec::new())]
    );
    assert_eq!(lab.dialer().associations(), [template(app)]);
    assert_eq!(lab.dialer().associations_open(), 1);
    assert_eq!(lab.counters(), Counters::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_datagram_goes_out_and_its_answer_comes_back_v4() {
    goes_out_and_comes_back(addr(APP4), addr(FAR4)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_datagram_goes_out_and_its_answer_comes_back_v6() {
    goes_out_and_comes_back(addr(APP6), addr(FAR6)).await;
}

/// A datagram to a fake address is a datagram for the name behind it, and the
/// answer comes from the fake address, whichever family the app is of.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_datagram_to_a_fake_address_goes_to_its_name_and_comes_back_from_it() {
    let mut lab = lab(TunOptions::default());
    let index = lab
        .pool()
        .assign("a.test", Instant::now())
        .expect("an address for the name");
    let name = named("a.test", 53);
    lab.dialer().script_udp(name.clone(), UdpScript::Echo);
    for (app, fake) in [
        (addr(APP4), sock(v4_of(index), 53)),
        (addr(APP6), sock(v6_of(index), 53)),
    ] {
        lab.send(app, fake, b"query");
        assert_eq!(
            lab.datagram().await,
            UdpSeen {
                src: fake,
                dst: app,
                payload: b"query".to_vec()
            },
            "to {fake}"
        );
    }
    assert_eq!(
        lab.dialer().udp_sent(),
        [(name.clone(), b"query".to_vec()), (name, b"query".to_vec())]
    );
    assert_eq!(lab.counters(), Counters::default());
}

/// What answers is not always the address that was asked: a STUN server's other
/// address, a peer. The app is told who it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_third_party_reply_keeps_its_source() {
    let mut lab = lab(TunOptions::default());
    let (app, dst) = (addr(APP4), addr("192.0.2.10:3478"));
    lab.dialer().script_udp(
        target_of(dst),
        UdpScript::EchoFrom(target("192.0.2.99", 3478)),
    );
    lab.send(app, dst, b"binding request");
    assert_eq!(
        lab.datagram().await,
        UdpSeen {
            src: addr("192.0.2.99:3478"),
            dst: app,
            payload: b"binding request".to_vec()
        }
    );
    assert_eq!(lab.counters(), Counters::default());
}

/// A reply that comes from a name (a flow the client named by a name) comes
/// from the fake address of the name in the app's family; one from a name that
/// has no address is lost and counted, and the replies after it are not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_from_a_name_comes_from_its_fake_address() {
    let mut lab = lab(TunOptions::default());
    let index = lab
        .pool()
        .assign("b.test", Instant::now())
        .expect("an address for b.test");
    let mut unknown = 0;
    for (opened, (app, far, fake)) in [
        (addr(APP4), addr(FAR4), sock(v4_of(index), 53)),
        (addr(APP6), addr(FAR6), sock(v6_of(index), 53)),
    ]
    .into_iter()
    .enumerate()
    {
        // a datagram first, for there to be a session to reply through
        lab.send(app, far, b"hello");
        lab.associations_open(opened + 1).await;
        lab.dialer()
            .send_to_app(app, named("b.test", 53), None, b"from b".to_vec())
            .await
            .expect("the reply is sent");
        assert_eq!(
            lab.datagram().await,
            UdpSeen {
                src: fake,
                dst: app,
                payload: b"from b".to_vec()
            },
            "to {app}"
        );
        // the flow a reply answers is named: the name wins over where it came from
        lab.dialer()
            .send_to_app(
                app,
                target("192.0.2.5", 53),
                Some(named("b.test", 53)),
                b"answers b".to_vec(),
            )
            .await
            .expect("the reply is sent");
        assert_eq!(lab.datagram().await.src, fake);
        // a name with no address: lost and counted; the next reply is the next datagram
        lab.dialer()
            .send_to_app(app, named("c.test", 53), None, b"from c".to_vec())
            .await
            .expect("a reply that is lost is not an error");
        unknown += 1;
        lab.dialer()
            .send_to_app(app, named("b.test", 53), None, b"again".to_vec())
            .await
            .expect("the reply is sent");
        assert_eq!(lab.datagram().await.payload, b"again");
        assert_eq!(lab.counters().unknown_fake, unknown);
    }
    assert_eq!(
        lab.counters(),
        Counters {
            unknown_fake: 2,
            ..Counters::default()
        }
    );
}

/// A reply from an address of the other family than the app's cannot be written
/// to it: lost and counted as unsupported.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_from_the_other_address_family_is_dropped() {
    let mut lab = lab(TunOptions::default());
    let mut unsupported = 0;
    for (app, far, other, same) in [
        (addr(APP4), addr(FAR4), "2001:db8::99", "192.0.2.99"),
        (addr(APP6), addr(FAR6), "192.0.2.99", "2001:db8::99"),
    ] {
        lab.send(app, far, b"hello");
        lab.associations_open(if app.is_ipv4() { 1 } else { 2 })
            .await;
        lab.dialer()
            .send_to_app(app, target(other, 3478), None, b"wrong family".to_vec())
            .await
            .expect("a reply that is lost is not an error");
        unsupported += 1;
        // the same family is replied
        lab.dialer()
            .send_to_app(app, target(same, 3478), None, b"right family".to_vec())
            .await
            .expect("the reply is sent");
        let seen = lab.datagram().await;
        assert_eq!(seen.payload, b"right family");
        assert_eq!(lab.counters().unsupported, unsupported);
    }
}

/// Each source endpoint of the app is a session of its own; different
/// destinations from one endpoint are the same session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_session_per_source_endpoint() {
    let lab = lab(TunOptions::default());
    let (first, second) = (addr("198.18.0.1:5000"), addr("198.18.0.1:5001"));
    lab.send(first, addr("192.0.2.10:7"), b"a");
    lab.send(second, addr("192.0.2.10:7"), b"b");
    lab.send(second, addr("192.0.2.11:7"), b"c");
    eventually("the datagrams to be taken", WITHIN, || {
        lab.dialer().udp_sent().len() == 3
    })
    .await;
    let mut associations = lab.dialer().associations();
    associations.sort_by_key(|session| session.src.port());
    assert_eq!(associations, [template(first), template(second)]);
    assert_eq!(lab.dialer().associations_open(), 2);
    assert_eq!(lab.counters(), Counters::default());
}

// ---- replies larger than the MTU

/// The identification and the checks of the fragments of one datagram of
/// `datagram` bytes in IPv4 packets of at most 1500 bytes: all with a header
/// that holds and no options, the identification the same, DF cleared, "more
/// fragments" on all but the last, offsets that begin where the last ended and
/// are multiples of 8, and the data of all but the last a multiple of 8 bytes.
fn check_fragments_v4(fragments: &[Vec<u8>], datagram: usize) -> u16 {
    let ident = Ipv4Packet::new_checked(&fragments[0][..])
        .expect("an IPv4 packet")
        .ident();
    let mut next = 0;
    for (index, packet) in fragments.iter().enumerate() {
        assert!(packet.len() <= 1500, "fragment {index} fits the MTU");
        let ip = Ipv4Packet::new_checked(&packet[..]).expect("a whole IPv4 packet");
        assert!(ip.verify_checksum(), "fragment {index}'s header holds");
        assert_eq!(ip.header_len(), 20, "fragment {index} has no options");
        assert_eq!(ip.next_header(), IpProtocol::Udp);
        assert_eq!(ip.hop_limit(), 64);
        assert_eq!(ip.ident(), ident, "one identification");
        assert!(!ip.dont_frag(), "fragment {index} may be fragmented");
        assert_eq!(usize::from(ip.frag_offset()), next, "fragment {index}");
        assert_eq!(next % 8, 0);
        let last = index + 1 == fragments.len();
        assert_eq!(ip.more_frags(), !last, "fragment {index}");
        if !last {
            assert_eq!(ip.payload().len() % 8, 0, "fragment {index} but the last");
        }
        next += ip.payload().len();
    }
    assert_eq!(next, datagram, "the fragments hold the datagram");
    ident
}

/// The same for IPv6: the fragment header after the base header, whose next
/// header is the datagram's protocol, and an offset in units of 8 bytes.
fn check_fragments_v6(fragments: &[Vec<u8>], datagram: usize) -> u32 {
    let ident_of = |packet: &Vec<u8>| {
        let ip = Ipv6Packet::new_checked(&packet[..]).expect("an IPv6 packet");
        let payload = ip.payload();
        u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]])
    };
    let ident = ident_of(&fragments[0]);
    let mut next = 0;
    for (index, packet) in fragments.iter().enumerate() {
        assert!(packet.len() <= 1500, "fragment {index} fits the MTU");
        let ip = Ipv6Packet::new_checked(&packet[..]).expect("a whole IPv6 packet");
        assert_eq!(ip.next_header(), IpProtocol::Ipv6Frag);
        assert_eq!(ip.hop_limit(), 64);
        let payload = ip.payload();
        assert_eq!(IpProtocol::from(payload[0]), IpProtocol::Udp);
        assert_eq!(payload[1], 0, "the reserved byte");
        let field = u16::from_be_bytes([payload[2], payload[3]]);
        assert_eq!(usize::from(field >> 3) * 8, next, "fragment {index}");
        let last = index + 1 == fragments.len();
        assert_eq!(field & 1 != 0, !last, "fragment {index}");
        assert_eq!(field & 0b110, 0, "the reserved bits");
        assert_eq!(ident_of(packet), ident, "one identification");
        let data = &payload[8..];
        if !last {
            assert_eq!(data.len() % 8, 0, "fragment {index} but the last");
        }
        next += data.len();
    }
    assert_eq!(next, datagram, "the fragments hold the datagram");
    ident
}

/// A reply that does not fit the device's MTU comes back in the fragments of
/// its family, whole once the app has put them together, and each reply is a
/// datagram of its own to the app's stack: another identification.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_larger_than_the_mtu_is_fragmented() {
    let mut lab = lab(TunOptions::default());
    let big: Vec<u8> = (0..4000).map(|i| (i % 251) as u8).collect();
    for (app, dst) in [(addr(APP4), addr(FAR4)), (addr(APP6), addr(FAR6))] {
        lab.dialer()
            .script_udp(target_of(dst), UdpScript::Reply(big.clone()));
        let mut idents = Vec::new();
        for ask in [&b"the big one"[..], b"and again"] {
            lab.send(app, dst, ask);
            // the datagram is 4008 bytes: 3 fragments in either family
            let fragments = lab.packets(3).await;
            let ident = if app.is_ipv4() {
                u32::from(check_fragments_v4(&fragments, 4008))
            } else {
                check_fragments_v6(&fragments, 4008)
            };
            idents.push(ident);
            let whole = packets::reassemble(&fragments).expect("the fragments are one datagram");
            assert_eq!(
                packets::read_udp(&whole),
                Some(UdpSeen {
                    src: dst,
                    dst: app,
                    payload: big.clone()
                }),
                "{app}"
            );
        }
        assert_ne!(
            idents[0], idents[1],
            "another datagram, another identification"
        );
    }
    assert_eq!(lab.counters(), Counters::default());
}

/// The longest datagram a reply can be is the longest the packets can hold; a
/// longer one is refused, not written wrong.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_longest_datagram_is_carried_and_a_longer_one_refused() {
    let mut lab = lab(TunOptions::default());
    // IPv4's total length is 65535 bytes with a header of 20 and the UDP header
    // of 8; IPv6's payload length is 65535 bytes with the UDP header in it
    for (app, far, longest) in [
        (addr(APP4), addr(FAR4), 65507),
        (addr(APP6), addr(FAR6), 65527),
    ] {
        lab.send(app, far, b"hello");
        lab.associations_open(if app.is_ipv4() { 1 } else { 2 })
            .await;
        let from = target_of(far);
        lab.dialer()
            .send_to_app(app, from.clone(), Some(from.clone()), vec![7; longest + 1])
            .await
            .expect_err("a datagram that is too long to carry");
        lab.dialer()
            .send_to_app(app, from.clone(), Some(from), vec![7; longest])
            .await
            .expect("the longest datagram");
        // every fragment of it is in the app's hands, and they are one datagram
        let mut fragments = vec![lab.packet().await];
        let whole = loop {
            if let Some(whole) = packets::reassemble(&fragments) {
                break whole;
            }
            fragments.push(lab.packet().await);
        };
        let seen = packets::read_udp(&whole).expect("a datagram with right checksums");
        assert_eq!(seen.payload.len(), longest, "{app}");
        assert!(seen.payload.iter().all(|byte| *byte == 7));
    }
}

// ---- limits

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
        udp_idle: Duration::from_millis(300),
        ..TunOptions::default()
    });
    let (app, dst) = (addr(APP4), addr(FAR4));
    let from = target_of(dst);
    lab.send(app, dst, b"subscribe");
    lab.associations_open(1).await;
    for push in 0..10 {
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
        udp_idle: Duration::from_millis(300),
        ..TunOptions::default()
    });
    let (app, dst) = (addr(APP4), addr(FAR4));
    lab.send(app, dst, b"hello");
    lab.associations_open(1).await;
    for push in 0..8 {
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
    let (app, fake, name) = (addr(APP4), sock(v4_of(index), 53), named("a.test", 53));
    lab.send(app, fake, b"subscribe");
    lab.associations_open(1).await;
    for push in 0..8 {
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

// ---- refusals

/// A flow the engine refuses is answered with an ICMP message, "administratively
/// prohibited", from the address the app sent to (a fake address as well as a
/// real one), quoting the IP header and the UDP header of the datagram; except
/// `REJECT-DROP`, which is answered with nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refused_targets_answer_with_icmp() {
    let mut lab = lab(counting_icmp());
    let index = lab
        .pool()
        .assign("a.test", Instant::now())
        .expect("an address for a.test");
    let name = named("a.test", 53);
    for (app, real, fake, headers) in [
        (addr(APP4), addr(FAR4), sock(v4_of(index), 53), 20 + 8),
        (addr(APP6), addr(FAR6), sock(v6_of(index), 53), 40 + 8),
    ] {
        for kind in [RejectKind::Reject, RejectKind::NoDrop, RejectKind::TinyGif] {
            lab.dialer()
                .script_udp(target_of(real), UdpScript::Refuse(kind));
            lab.dialer()
                .script_udp(name.clone(), UdpScript::Refuse(kind));
            for dst in [real, fake] {
                let packet = packets::udp(app, dst, b"refused");
                lab.rig.app.inject(packet.clone());
                let (src, to, quoted) = lab.prohibited().await;
                assert_eq!((src, to), (dst.ip(), app.ip()), "{kind:?} to {dst}");
                assert_eq!(quoted, packet[..headers], "{kind:?} to {dst}");
            }
        }
    }
    assert_eq!(lab.counters(), Counters::default());
    // a header with options is quoted whole
    let (app, dst) = (addr(APP4), addr(FAR4));
    lab.dialer()
        .script_udp(target_of(dst), UdpScript::Refuse(RejectKind::NoDrop));
    let packet = with_ip_options(&packets::udp(app, dst, b"options"), &[1, 1, 1, 0]);
    lab.rig.app.inject(packet.clone());
    let (_, _, quoted) = lab.prohibited().await;
    assert_eq!(quoted, packet[..24 + 8]);
}

/// `REJECT-DROP` is answered with nothing at all; the datagram did reach the
/// dialer, which told the client.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_flow_is_answered_with_nothing() {
    let mut lab = lab(counting_icmp());
    for (app, dst) in [(addr(APP4), addr(FAR4)), (addr(APP6), addr(FAR6))] {
        lab.dialer()
            .script_udp(target_of(dst), UdpScript::Refuse(RejectKind::Drop));
        lab.send(app, dst, b"dropped");
    }
    eventually("the datagrams to be taken", WITHIN, || {
        lab.dialer().udp_sent().len() == 2
    })
    .await;
    tokio::time::sleep(NOTHING).await;
    assert!(lab.no_icmp(), "an ICMP message for the app");
    assert_eq!(lab.counters(), Counters::default());
}

/// An app that goes on sending to flows that are refused (a QUIC client trying
/// again) is answered a hundred times in three seconds, no more, whether the
/// flows are refused with `REJECT` or with `REJECT-TINYGIF`: the limit is one for
/// both. The ICMP of `REJECT-NO-DROP` has no limit, nor does it count against
/// the other's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn icmp_for_refused_flows_is_limited() {
    let mut lab = lab(counting_icmp());
    let app = addr(APP4);
    let (rejected, tiny, unlimited) = (
        addr("192.0.2.10:9"),
        addr("192.0.2.12:9"),
        addr("192.0.2.11:9"),
    );
    lab.dialer()
        .script_udp(target_of(rejected), UdpScript::Refuse(RejectKind::Reject));
    lab.dialer()
        .script_udp(target_of(tiny), UdpScript::Refuse(RejectKind::TinyGif));
    lab.dialer()
        .script_udp(target_of(unlimited), UdpScript::Refuse(RejectKind::NoDrop));
    for _ in 0..60 {
        lab.send(app, rejected, b"again");
    }
    for _ in 0..41 {
        lab.send(app, tiny, b"again");
    }
    eventually("one message to be held back", WITHIN, || {
        lab.counters().icmp_limited == 1
    })
    .await;
    for _ in 0..150 {
        lab.send(app, unlimited, b"again");
    }
    // the messages come in the order of the datagrams: sixty for the first flow,
    // forty for the second (the forty-first was held back), and then a hundred
    // and fifty for the third, with none of the second's between if the limit held
    for number in 0..60 {
        let (src, _, _) = lab.prohibited().await;
        assert_eq!(src, rejected.ip(), "message {number}");
    }
    for number in 0..40 {
        let (src, _, _) = lab.prohibited().await;
        assert_eq!(src, tiny.ip(), "message {number} of the second");
    }
    for number in 0..150 {
        let (src, _, _) = lab.prohibited().await;
        assert_eq!(src, unlimited.ip(), "message {number} of the third");
    }
    assert_eq!(
        lab.counters(),
        Counters {
            icmp_limited: 1,
            ..Counters::default()
        }
    );
}

/// The addresses of rurge's own are not places a datagram goes to: the app is
/// told so, without a limit and without a session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_to_rurge_addresses_is_prohibited_unlimited() {
    let mut lab = lab(counting_icmp());
    let (app4, app6) = (addr(APP4), addr(APP6));
    for (app, dst, headers) in [
        (app4, addr("198.18.0.1:9"), 28),
        (app4, addr("198.18.0.2:53"), 28),
        (app4, addr("198.18.0.244:9"), 28),
        (app4, addr("198.18.0.253:9"), 28),
        (app6, addr("[fd00:6152::fd]:9"), 48),
        (app6, addr("[fd00:6152::2]:53"), 48),
    ] {
        let packet = packets::udp(app, dst, b"hello");
        lab.rig.app.inject(packet.clone());
        let (src, to, quoted) = lab.prohibited().await;
        assert_eq!((src, to), (dst.ip(), app.ip()), "to {dst}");
        assert_eq!(quoted, packet[..headers], "to {dst}");
    }
    for _ in 0..150 {
        lab.send(app4, addr("198.18.0.244:9"), b"again");
    }
    for number in 0..150 {
        let (src, _, _) = lab.prohibited().await;
        assert_eq!(src, addr("198.18.0.244:9").ip(), "message {number}");
    }
    assert!(lab.dialer().associations().is_empty());
    assert_eq!(lab.counters(), Counters::default());
}

/// An address of the pool's space that no name has names nothing: the datagram
/// is dropped and counted, with no session made for it and no ICMP message.
/// So is every fake address when the TUN has no pool.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_to_an_unknown_fake_address_is_dropped() {
    for pool in [Some(Arc::new(FakeIpPool::new())), None] {
        let had_pool = pool.is_some();
        let mut lab = lab_with(
            TunOptions::default(),
            Extras {
                pool,
                ..Extras::default()
            },
        );
        lab.send(addr(APP4), sock(v4_of(7), 53), b"who?");
        lab.send(addr(APP6), sock(v6_of(7), 53), b"who?");
        eventually("the datagrams to be counted", WITHIN, || {
            lab.counters().unknown_fake == 2
        })
        .await;
        tokio::time::sleep(NOTHING).await;
        assert!(
            lab.dialer().associations().is_empty(),
            "(a pool: {had_pool})"
        );
        assert!(lab.no_icmp(), "(a pool: {had_pool})");
        assert_eq!(
            lab.counters(),
            Counters {
                unknown_fake: 2,
                ..Counters::default()
            }
        );
    }
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

// ---- the writer's queue

/// A rig on a device that writes nothing until `open`, and a writer queue of
/// one batch.
fn gated() -> (Lab, Arc<GatedDevice>) {
    let (gate, end) = GatedDevice::pair();
    let rig = rig_with(
        gate.clone(),
        end,
        TunOptions {
            writer_queue: 1,
            ..TunOptions::default()
        },
    );
    (taking(rig), gate)
}

/// The receive task, and the dialer's client when it refuses a flow, hand the
/// writer an ICMP message without waiting: when its queue is full the message is
/// dropped and counted, and nothing else stops.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_writer_queue_drops_the_icmp_and_counts_it() {
    for from_the_client in [false, true] {
        let (mut lab, gate) = gated();
        let (app, marker) = (addr(APP4), addr(FAR4));
        lab.dialer()
            .script_udp(target_of(marker), UdpScript::Reply(b"marker".to_vec()));
        let refused = addr("192.0.2.11:9");
        lab.dialer()
            .script_udp(target_of(refused), UdpScript::Refuse(RejectKind::NoDrop));
        let senders = 20;
        for _ in 0..senders {
            let dst = if from_the_client {
                refused
            } else {
                addr("198.18.0.244:9")
            };
            lab.send(app, dst, b"x");
        }
        // a datagram that comes after them all, and is answered when the
        // writer takes anything again
        lab.send(app, marker, b"marker");
        eventually("the marker to be taken", WITHIN, || {
            lab.dialer()
                .udp_sent()
                .iter()
                .any(|(to, _)| to == &target_of(marker))
        })
        .await;
        // the writer holds one batch in its write and the queue one more
        let lost = lab.counters().queue_full;
        assert!(
            lost >= senders - 2,
            "{lost} messages were lost, at least {} were to be (the client: {from_the_client})",
            senders - 2
        );
        gate.open();
        assert_eq!(lab.datagram().await.payload, b"marker");
        // the writer wrote in order: every message it took came before the marker
        let mut messages = 0;
        while let Ok(packet) = lab.icmp.try_recv() {
            packets::read_icmp(&packet).expect("an ICMP message");
            messages += 1;
        }
        assert_eq!(
            messages + lost,
            senders,
            "every message was written or counted (the client: {from_the_client})"
        );
        assert_eq!(
            lab.counters(),
            Counters {
                queue_full: lost,
                ..Counters::default()
            }
        );
    }
}

/// The client waits for room in the writer's queue to send a reply: nothing
/// is lost, and the engine's task that gave it the reply waits (that is the
/// back pressure on the upstream).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_waits_for_room_in_the_writer_queue() {
    let (mut lab, gate) = gated();
    let (app, far) = (addr(APP4), addr(FAR4));
    lab.send(app, far, b"hello");
    lab.associations_open(1).await;
    let dialer = lab.rig.dialer.clone();
    let from = target_of(far);
    let sending = tokio::spawn(async move {
        for number in 0..5u8 {
            dialer
                .send_to_app(app, from.clone(), Some(from.clone()), vec![number])
                .await
                .expect("the reply is sent");
        }
    });
    // the writer holds one reply in its write and the queue one more: the third
    // has to wait, however long that is
    tokio::time::sleep(NOTHING).await;
    assert!(!sending.is_finished(), "the replies did not wait for room");
    gate.open();
    in_five_seconds("the replies to be sent", sending)
        .await
        .expect("the sending task");
    for number in 0..5u8 {
        assert_eq!(lab.datagram().await.payload, [number]);
    }
    assert_eq!(lab.counters(), Counters::default());
}

// ---- the pure parts

/// `ip_packets` of a UDP datagram: one packet when it fits, else fragments that
/// put together are the datagram, the other way round as well, and each call
/// has its own identification.
#[test]
fn fragments_reassemble_to_the_datagram() {
    let ids = Ids::new();
    for (src, dst) in [
        (addr("192.0.2.10:7"), addr("198.18.0.1:5000")),
        (addr("[2001:db8::10]:7"), addr("[fd00:6152::1]:5000")),
    ] {
        let header = if src.is_ipv4() { 20 } else { 40 };
        for mtu in [1280, 1500, 9000] {
            // what a single packet holds of UDP, and a few sizes around it
            let fits = mtu - header - 8;
            let longest = if src.is_ipv4() { 65507 } else { 65527 };
            for length in [
                0,
                1,
                1000,
                fits - 1,
                fits,
                fits + 1,
                2 * fits,
                3 * fits + 5,
                longest,
            ] {
                let payload: Vec<u8> = (0..length).map(|i| (i % 253) as u8).collect();
                let datagram = udp_bytes(src, dst, &payload);
                let packets = ip_packets(src.ip(), dst.ip(), IpProtocol::Udp, &datagram, mtu, &ids);
                let wanted = UdpSeen {
                    src,
                    dst,
                    payload: payload.clone(),
                };
                assert!(
                    packets.iter().all(|packet| packet.len() <= mtu),
                    "{length} bytes in {mtu}: no packet is over the MTU"
                );
                if header + datagram.len() <= mtu {
                    assert_eq!(packets.len(), 1, "{length} bytes in {mtu} fit");
                    assert_eq!(packets::read_udp(&packets[0]), Some(wanted));
                    continue;
                }
                assert!(packets.len() > 1, "{length} bytes in {mtu} do not fit");
                if src.is_ipv4() {
                    check_fragments_v4_in(&packets, datagram.len(), mtu);
                } else {
                    check_fragments_v6_in(&packets, datagram.len(), mtu);
                }
                let whole = packets::reassemble(&packets).expect("one datagram");
                assert_eq!(packets::read_udp(&whole), Some(wanted.clone()));
                // in any order
                let mut backwards = packets.clone();
                backwards.reverse();
                assert_eq!(packets::reassemble(&backwards), Some(whole));
                // a fragment missing is no datagram
                let mut short = packets;
                short.remove(short.len() / 2);
                assert_eq!(packets::reassemble(&short), None);
            }
        }
        // each call has an identification of its own
        let datagram = udp_bytes(src, dst, &[0; 3000]);
        let first = ip_packets(src.ip(), dst.ip(), IpProtocol::Udp, &datagram, 1500, &ids);
        let second = ip_packets(src.ip(), dst.ip(), IpProtocol::Udp, &datagram, 1500, &ids);
        if src.is_ipv4() {
            assert_ne!(
                Ipv4Packet::new_unchecked(&first[0][..]).ident(),
                Ipv4Packet::new_unchecked(&second[0][..]).ident()
            );
        } else {
            assert_ne!(first[0][44..48], second[0][44..48]);
        }
    }
}

/// `check_fragments_v4` for a device of any MTU.
fn check_fragments_v4_in(fragments: &[Vec<u8>], datagram: usize, mtu: usize) {
    let mut next = 0;
    for (index, packet) in fragments.iter().enumerate() {
        let ip = Ipv4Packet::new_checked(&packet[..]).expect("a whole IPv4 packet");
        assert!(ip.verify_checksum() && !ip.dont_frag(), "fragment {index}");
        assert_eq!(usize::from(ip.frag_offset()), next);
        assert_eq!(ip.hop_limit(), 64);
        let last = index + 1 == fragments.len();
        assert_eq!(ip.more_frags(), !last);
        if !last {
            // as much as fits, in a multiple of 8
            assert_eq!(ip.payload().len(), (mtu - 20) / 8 * 8, "fragment {index}");
        }
        next += ip.payload().len();
    }
    assert_eq!(next, datagram);
}

/// The same for IPv6.
fn check_fragments_v6_in(fragments: &[Vec<u8>], datagram: usize, mtu: usize) {
    let mut next = 0;
    for (index, packet) in fragments.iter().enumerate() {
        let ip = Ipv6Packet::new_checked(&packet[..]).expect("a whole IPv6 packet");
        assert_eq!(ip.next_header(), IpProtocol::Ipv6Frag);
        assert_eq!(ip.hop_limit(), 64);
        let payload = ip.payload();
        let field = u16::from_be_bytes([payload[2], payload[3]]);
        assert_eq!(usize::from(field >> 3) * 8, next, "fragment {index}");
        let last = index + 1 == fragments.len();
        assert_eq!(field & 1 != 0, !last);
        if !last {
            assert_eq!(
                payload.len() - 8,
                (mtu - 40 - 8) / 8 * 8,
                "fragment {index}"
            );
        }
        next += payload.len() - 8;
    }
    assert_eq!(next, datagram);
}

/// The message "administratively prohibited": ICMPv4's type 3 code 13 and
/// ICMPv6's type 1 code 1, from one address to another, whose body is the
/// quoted header and nothing else, and whose checksums hold (ICMPv6's with the
/// pseudo-header).
#[test]
fn the_prohibited_message_quotes_the_header() {
    // IPv4: the header of 20 bytes and the UDP header of the datagram
    let (app, dst) = (addr(APP4), addr("192.0.2.10:9"));
    let original = packets::udp(app, dst, b"payload");
    let quoted = &original[..28];
    let packet = prohibited(dst.ip(), app.ip(), quoted);
    let ip = Ipv4Packet::new_checked(&packet[..]).expect("an IPv4 packet");
    assert!(ip.verify_checksum());
    assert_eq!(IpAddr::from(ip.src_addr()), dst.ip());
    assert_eq!(IpAddr::from(ip.dst_addr()), app.ip());
    assert_eq!(ip.next_header(), IpProtocol::Icmp);
    assert_eq!(ip.hop_limit(), 64);
    assert_eq!(usize::from(ip.total_len()), 20 + 8 + 28);
    assert!(!ip.more_frags() && ip.frag_offset() == 0);
    let icmp = Icmpv4Packet::new_checked(ip.payload()).expect("an ICMPv4 message");
    assert_eq!(icmp.msg_type(), Icmpv4Message::DstUnreachable);
    assert_eq!(icmp.msg_code(), 13);
    assert!(icmp.verify_checksum());
    assert_eq!(icmp.data(), quoted);
    assert_eq!(
        packets::read_icmp(&packet),
        Some(IcmpSeen::Prohibited {
            src: dst.ip(),
            dst: app.ip(),
            quoted: quoted.to_vec()
        })
    );

    // IPv6: the base header of 40 bytes and the UDP header
    let (app, dst) = (addr(APP6), addr("[2001:db8::10]:9"));
    let original = packets::udp(app, dst, b"payload");
    let quoted = &original[..48];
    let packet = prohibited(dst.ip(), app.ip(), quoted);
    let ip = Ipv6Packet::new_checked(&packet[..]).expect("an IPv6 packet");
    assert_eq!(IpAddr::from(ip.src_addr()), dst.ip());
    assert_eq!(IpAddr::from(ip.dst_addr()), app.ip());
    assert_eq!(ip.next_header(), IpProtocol::Icmpv6);
    assert_eq!(ip.hop_limit(), 64);
    assert_eq!(ip.payload().len(), 8 + 48);
    let (from, to) = match (dst.ip(), app.ip()) {
        (IpAddr::V6(from), IpAddr::V6(to)) => (from, to),
        _ => unreachable!("the addresses are of one family"),
    };
    let icmp = Icmpv6Packet::new_checked(ip.payload()).expect("an ICMPv6 message");
    assert_eq!(icmp.msg_type(), Icmpv6Message::DstUnreachable);
    assert_eq!(icmp.msg_code(), 1);
    assert!(icmp.verify_checksum(&from, &to));
    assert_eq!(icmp.payload(), quoted);
    assert_eq!(
        packets::read_icmp(&packet),
        Some(IcmpSeen::Prohibited {
            src: dst.ip(),
            dst: app.ip(),
            quoted: quoted.to_vec()
        })
    );
}
