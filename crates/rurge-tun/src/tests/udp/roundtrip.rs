//! A datagram there and back: a datagram to a real address or to a fake one
//! goes to the dialer, and what comes back is shown to the app as coming from
//! where the app can place it.

use super::{APP4, APP6, FAR4, FAR6, lab, named, target, target_of};
use crate::testing::UdpScript;
use crate::testing::packets::UdpSeen;
use crate::tests::support::{WITHIN, addr, eventually, sock};
use crate::{Counters, TunOptions};
use rurge_config::{HostName, ListenerKind, SessionInfo, Transport};
use rurge_dns::fakeip::{v4_of, v6_of};
use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

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
