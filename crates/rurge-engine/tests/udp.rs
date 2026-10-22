//! The UDP pipeline through the engine (phase 2 M5 design §5): SOCKS5 UDP
//! ASSOCIATE in, rules and policies per destination, one carrier per
//! outbound, full cone.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::session::{SessionInfo, Transport};
use rurge_engine::{REFUSED_STRICT, RequestRecord};
use rurge_inbound::{Dialer, UdpClient};
use rurge_net::BoxFuture;
use rurge_net::connector::Target;
use rurge_proto::RejectKind;
use std::io;
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

fn udp_records(h: &Harness) -> Vec<RequestRecord> {
    h.engine
        .request_log()
        .recent(4096)
        .into_iter()
        .filter(|r| r.transport == Transport::Udp)
        .collect()
}

/// Waits until `count` UDP flows have a policy: routed, or already
/// finished. Ending the association earlier would end the flows still
/// being routed.
async fn routed(h: &Harness, count: usize) {
    wait_until("the UDP flows to be routed", || {
        let log = h.engine.request_log();
        log.active()
            .into_iter()
            .chain(log.recent(4096))
            .filter(|r| r.transport == Transport::Udp && !r.policy.is_empty())
            .count()
            >= count
    })
    .await;
}

/// Waits until `count` UDP records are finished, and returns them.
async fn finished(h: &Harness, count: usize) -> Vec<RequestRecord> {
    wait_until("the UDP flows to finish", || udp_records(h).len() >= count).await;
    udp_records(h)
}

/// Full cone: anyone may send to the carrier, also a datagram too long for
/// the client once the SOCKS5 header is added. Only that one is lost; the
/// carrier and the flows on it go on.
#[tokio::test]
async fn an_answer_too_long_for_the_client_is_dropped_alone() {
    let h = harness(Profile::default()).await;
    let remote = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    // macOS sends no datagram larger than the socket's send buffer, 9216 bytes
    // by default; the answer below is 65,500.
    socket2::SockRef::from(&remote)
        .set_send_buffer_size(256 * 1024)
        .expect("a 256 KiB send buffer for the remote's socket");
    let remote_addr = remote.local_addr().unwrap();
    let association = udp_associate(h.socks()).await;
    association
        .send("127.0.0.1", remote_addr.port(), b"hello")
        .await;
    let mut buf = vec![0u8; 70_000];
    let (n, carrier) = tokio::time::timeout(Duration::from_secs(5), remote.recv_from(&mut buf))
        .await
        .expect("the datagram goes out")
        .unwrap();
    assert_eq!(&buf[..n], b"hello");
    // fine for UDP, too long with the 10-byte SOCKS5 header
    remote.send_to(&vec![7u8; 65_500], carrier).await.unwrap();
    remote.send_to(b"small", carrier).await.unwrap();
    assert_eq!(association.recv().await, (remote_addr, b"small".to_vec()));
}

/// A flow whose datagrams cannot go out (here: a v4-only policy asked for
/// an IPv6 address) fails once and drops what comes for `RETRY_AFTER`, as
/// a flow that could not open does — not a new flow, rules and record for
/// every datagram.
#[tokio::test]
async fn a_flow_whose_sends_fail_rests_before_it_tries_again() {
    let h = harness(Profile {
        proxies: "V4 = direct, ip-version=v4-only",
        rules: "IP-CIDR6,::1/128,V4,no-resolve",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("::1", 9, b"x").await;
    let records = finished(&h, 1).await;
    assert_eq!(
        records[0].error.as_deref(),
        Some("this policy sends no UDP to [::1]:9")
    );
    // the client keeps sending after the failure
    for _ in 0..9 {
        association.send("::1", 9, b"x").await;
    }
    // only to see that no other flow starts
    tokio::time::sleep(Duration::from_millis(500)).await;
    let records = udp_records(&h);
    assert_eq!(records.len(), 1, "{records:?}");
}

/// A datagram the system will not send (here: to the broadcast address,
/// which a socket without `SO_BROADCAST` may not reach) is lost alone, as
/// on any hop: the flow goes on, and its record says why.
#[tokio::test]
async fn a_datagram_the_system_turns_away_is_dropped_alone() {
    let h = harness(Profile::default()).await;
    let association = udp_associate(h.socks()).await;
    association.send("255.255.255.255", 9, b"x").await;
    wait_until("the reason on the record", || {
        h.engine
            .request_log()
            .active()
            .iter()
            .any(|r| r.transport == Transport::Udp && r.error.is_some())
    })
    .await;
    association.send("255.255.255.255", 9, b"x").await;
    // only to see that nothing ends the flow
    tokio::time::sleep(Duration::from_millis(300)).await;
    let records = udp_records(&h);
    assert!(records.is_empty(), "{records:?}");
}

#[tokio::test]
async fn a_datagram_goes_direct_and_comes_back() {
    let h = harness(Profile::default()).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"ping").await;
    assert_eq!(association.recv().await, (echo, b"ping".to_vec()));
    // a second datagram rides the same flow
    association.send("127.0.0.1", echo.port(), b"pong").await;
    assert_eq!(association.recv().await, (echo, b"pong".to_vec()));
    drop(association);
    let records = finished(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    let r = &records[0];
    assert_eq!(r.dst, format!("127.0.0.1:{}", echo.port()));
    assert_eq!(r.policy, ["DIRECT"]);
    assert_eq!(r.status, RecordStatus::Completed);
    assert_eq!((r.up, r.down), (8, 8));
    assert!(r.connect_ms.is_some() && r.first_byte_ms.is_some(), "{r:?}");
}

/// A client may write an IPv4 target in IPv6 form (`::ffff:a.b.c.d`). The
/// inbound hands the engine the IPv4 address: the datagram leaves from the
/// IPv4 socket, the answer is credited to the flow that sent it — not to the
/// oldest flow of the carrier — and reaches the client from an IPv4 address.
#[tokio::test]
async fn an_ipv4_mapped_target_is_the_ipv4_address() {
    let h = harness(Profile::default()).await;
    let ((older, _), (mapped, _)) = (udp_echo().await, udp_echo().await);
    let association = udp_associate(h.socks()).await;
    // the older flow: an answer credited to "the oldest flow" would be seen
    association.send("127.0.0.1", older.port(), b"a").await;
    assert_eq!(association.recv().await, (older, b"a".to_vec()));
    association
        .send("::ffff:127.0.0.1", mapped.port(), b"bb")
        .await;
    // `recv` reads the address type: IPv4 equals `mapped`, IPv6 would not
    assert_eq!(association.recv().await, (mapped, b"bb".to_vec()));
    drop(association);
    let records = finished(&h, 2).await;
    let record = |echo: SocketAddr| {
        records
            .iter()
            .find(|r| r.dst == echo.to_string())
            .unwrap_or_else(|| panic!("no record of {echo}: {records:?}"))
    };
    assert_eq!((record(older).up, record(older).down), (1, 1));
    assert_eq!((record(mapped).up, record(mapped).down), (2, 2));
}

/// A reload keeps the built-in DIRECT: the association's flow to a new
/// target leaves from the source port the earlier flows had (full cone).
#[tokio::test]
async fn a_reload_keeps_the_source_port_of_direct() {
    let h = harness(Profile::default()).await;
    let ((one, seen_one), (two, seen_two)) = (udp_echo().await, udp_echo().await);
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", one.port(), b"before").await;
    assert_eq!(association.recv().await, (one, b"before".to_vec()));
    let next = Profile::default().text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &next, h.engine.shared()).await);
    association.send("127.0.0.1", two.port(), b"after").await;
    assert_eq!(association.recv().await, (two, b"after".to_vec()));
    let (from_one, from_two) = (seen_one.lock().unwrap()[0], seen_two.lock().unwrap()[0]);
    assert_eq!(from_one, from_two);
}

/// DIRECT looks a name up here (with the profile's DNS) and sends to the
/// address; the record keeps the name.
#[tokio::test]
async fn a_name_is_looked_up_for_direct() {
    let h = harness(Profile::default()).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association
        .send("target.test", echo.port(), b"by name")
        .await;
    assert_eq!(association.recv().await, (echo, b"by name".to_vec()));
    drop(association);
    let records = finished(&h, 1).await;
    assert_eq!(records[0].dst, format!("target.test:{}", echo.port()));
    assert_eq!((records[0].up, records[0].down), (7, 7));
}

/// Full cone (M5-D2): once the client has written out, anyone may write
/// back to the carrier's address and reach the client.
#[tokio::test]
async fn anyone_may_answer_the_carrier() {
    let h = harness(Profile::default()).await;
    let (echo, seen) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"hello").await;
    association.recv().await;
    let carrier = seen.lock().unwrap()[0];
    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    stranger.send_to(b"from elsewhere", carrier).await.unwrap();
    assert_eq!(
        association.recv().await,
        (stranger.local_addr().unwrap(), b"from elsewhere".to_vec())
    );
    // it is counted on the association's flow
    drop(association);
    let records = finished(&h, 1).await;
    assert_eq!((records[0].up, records[0].down), (5, 5 + 14));
}

/// A datagram to a port nobody listens on (an ICMP "port unreachable"
/// comes back, which Windows reports on the next receive) does not break
/// the carrier the other flows share.
#[tokio::test]
async fn a_closed_port_does_not_break_the_carrier() {
    let h = harness(Profile::default()).await;
    let (echo, _) = udp_echo().await;
    let closed_port = closed_udp_port();
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"one").await;
    association.recv().await;
    association.send("127.0.0.1", closed_port, b"lost").await;
    // a window for the unreachable answer to arrive
    assert!(association.quiet_for(Duration::from_millis(300)).await);
    association.send("127.0.0.1", echo.port(), b"two").await;
    assert_eq!(association.recv().await, (echo, b"two".to_vec()));
}

/// A REJECT rule drops the datagrams; the record says REJECT.
#[tokio::test]
async fn a_reject_rule_drops_the_datagrams() {
    let h = harness(Profile {
        rules: "DOMAIN,blocked.test,REJECT",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("blocked.test", 443, b"x").await;
    association.send("blocked.test", 443, b"y").await;
    assert!(association.quiet_for(Duration::from_millis(300)).await);
    let records = finished(&h, 1).await;
    assert_eq!(records.len(), 1, "one flow however many datagrams");
    assert_eq!(records[0].status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(records[0].policy, ["REJECT"]);
}

/// A policy that carries no UDP (here `http`) rejects the flow and says
/// why (`udp-policy-not-supported-behaviour` defaults to REJECT).
#[tokio::test]
async fn a_policy_without_udp_rejects() {
    let h = harness(Profile {
        proxies: "Web = http, 127.0.0.1, 9",
        rules: "DOMAIN,web.test,Web",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("web.test", 53, b"q").await;
    assert!(association.quiet_for(Duration::from_millis(300)).await);
    let records = finished(&h, 1).await;
    assert_eq!(records[0].status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(
        records[0].error.as_deref(),
        Some("policy does not support UDP")
    );
}

/// `block-quic` (M5 design 6.2): QUIC to UDP 443 is dropped with a note —
/// the browser falls back to TCP — while other UDP to 443 goes through.
#[tokio::test]
async fn block_quic_drops_quic_and_nothing_else() {
    let h = harness(Profile {
        general: "block-quic = all",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("quic.test", 443, &quic_initial()).await;
    association.send("alt.test", 443, b"not quic").await;
    routed(&h, 2).await;
    drop(association);
    let records = finished(&h, 2).await;
    let by_dst = |dst: &str| records.iter().find(|r| r.dst == dst).unwrap();
    let quic = by_dst("quic.test:443");
    assert_eq!(quic.status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(quic.error.as_deref(), Some("QUIC blocked"));
    assert_eq!(quic.protocol, Some(rurge_config::rule::ProtocolKind::Quic));
    let plain = by_dst("alt.test:443");
    assert_eq!(plain.status, RecordStatus::Completed, "{plain:?}");
    assert_eq!(plain.protocol, Some(rurge_config::rule::ProtocolKind::Udp));
}

/// `per-policy`: a proxy's `auto` blocks, its `off` lets QUIC through;
/// DIRECT's `auto` lets it through.
#[tokio::test]
async fn per_policy_block_quic_follows_the_terminal_policy() {
    let up = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Auto = socks5, 127.0.0.1, {0}, udp-relay=true
Off = socks5, 127.0.0.1, {0}, udp-relay=true, block-quic=off",
        up.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        rules: "DOMAIN,auto.test,Auto
DOMAIN,off.test,Off",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    for host in ["auto.test", "off.test", "direct.test"] {
        association.send(host, 443, &quic_initial()).await;
    }
    routed(&h, 3).await;
    drop(association);
    let records = finished(&h, 3).await;
    let blocked = |dst: &str| {
        records
            .iter()
            .find(|r| r.dst == dst)
            .unwrap()
            .error
            .as_deref()
            == Some("QUIC blocked")
    };
    assert!(blocked("auto.test:443"));
    assert!(!blocked("off.test:443"));
    assert!(!blocked("direct.test:443"));
}

/// `PROTOCOL,UDP` matches every UDP flow; `PROTOCOL,QUIC` the QUIC ones.
#[tokio::test]
async fn protocol_rules_see_udp_and_quic() {
    let h = harness(Profile {
        rules: "PROTOCOL,QUIC,REJECT-DROP
PROTOCOL,UDP,REJECT-NO-DROP",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("q.test", 443, &quic_initial()).await;
    association.send("u.test", 53, b"dns?").await;
    routed(&h, 2).await;
    drop(association);
    let records = finished(&h, 2).await;
    let status = |dst: &str| {
        records
            .iter()
            .find(|r| r.dst == dst)
            .unwrap()
            .status
            .clone()
    };
    assert_eq!(
        status("q.test:443"),
        RecordStatus::Rejected("REJECT-DROP".into())
    );
    assert_eq!(
        status("u.test:53"),
        RecordStatus::Rejected("REJECT-NO-DROP".into())
    );
}

/// `udp-policy-not-supported-behaviour = DIRECT`: a policy without UDP
/// sends through DIRECT instead, and the record says so.
#[tokio::test]
async fn a_policy_without_udp_may_fall_back_to_direct() {
    let h = harness(Profile {
        general: "udp-policy-not-supported-behaviour = DIRECT",
        proxies: "Web = http, 127.0.0.1, 9",
        rules: "DOMAIN,web.test,Web",
        ..Profile::default()
    })
    .await;
    let (echo, _) = udp_echo().await;
    h.dns.set("web.test", &["127.0.0.1"], &[], 60);
    let association = udp_associate(h.socks()).await;
    association
        .send("web.test", echo.port(), b"via direct")
        .await;
    assert_eq!(association.recv().await, (echo, b"via direct".to_vec()));
    drop(association);
    let records = finished(&h, 1).await;
    assert_eq!(records[0].policy, ["Web"]);
    assert_eq!(
        records[0].error.as_deref(),
        Some("policy does not support UDP; sent through DIRECT")
    );
}

/// `socks5` with `udp-relay=true` carries UDP through the proxy's own
/// association (M5 design §7).
#[tokio::test]
async fn udp_goes_through_a_socks5_proxy() {
    let up = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Up = socks5, 127.0.0.1, {}, udp-relay=true",
        up.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        rules: "IP-CIDR,127.0.0.1/32,Up",
        ..Profile::default()
    })
    .await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association
        .send("127.0.0.1", echo.port(), b"via socks5")
        .await;
    assert_eq!(association.recv().await, (echo, b"via socks5".to_vec()));
    assert_eq!(
        up.datagrams(),
        [Target::new(HostName::Ip(echo.ip()), echo.port())]
    );
}

/// A UDP flow has no single far end. The control connection of the proxy's
/// association is a TCP socket of the flow that happened to open the
/// carrier, and it is no address of the flow's record.
#[tokio::test]
async fn a_flow_through_a_socks5_proxy_has_no_remote_address() {
    let up = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Up = socks5, 127.0.0.1, {}, udp-relay=true",
        up.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        rules: "IP-CIDR,127.0.0.1/32,Up",
        ..Profile::default()
    })
    .await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association
        .send("127.0.0.1", echo.port(), b"via the proxy")
        .await;
    assert_eq!(association.recv().await, (echo, b"via the proxy".to_vec()));
    let commands: Vec<u8> = up.requests().iter().map(|r| r.command).collect();
    assert_eq!(
        commands,
        [3],
        "the association's control connection was made"
    );
    drop(association);
    let records = finished(&h, 1).await;
    assert_eq!(records[0].policy, ["Up"]);
    assert_eq!(records[0].remote_addr, None, "{:?}", records[0]);
    assert_eq!(records[0].listen_port, h.socks().port());
}

/// The lookup DIRECT makes of the name a flow is addressed to counts, as a
/// TCP session's does; a flow addressed to an address looked nothing up.
#[tokio::test]
async fn a_flow_counts_the_lookup_of_its_name() {
    let h = harness(Profile::default()).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association
        .send("target.test", echo.port(), b"by name")
        .await;
    association
        .send("127.0.0.1", echo.port(), b"by address")
        .await;
    let mut answers = vec![association.recv().await.1, association.recv().await.1];
    answers.sort();
    assert_eq!(answers, [b"by address".to_vec(), b"by name".to_vec()]);
    drop(association);
    let records = finished(&h, 2).await;
    let of = |host: &str| {
        records
            .iter()
            .find(|r| r.dst.starts_with(host))
            .unwrap_or_else(|| panic!("no record of {host} in {records:?}"))
    };
    let (named, addressed) = (of("target.test"), of("127.0.0.1"));
    assert!(named.dns_ms.is_some(), "{named:?}");
    assert_eq!(addressed.dns_ms, None, "{addressed:?}");
}

/// The lookup a rule makes counts for a flow, as it does for a TCP session:
/// an `IP-CIDR` rule without `no-resolve` has the name looked up, while a flow
/// addressed to an address is matched without one. Both flows are rejected,
/// so neither opened a socket.
#[tokio::test]
async fn the_lookup_a_rule_makes_counts_for_a_flow_too() {
    let h = harness(Profile {
        rules: "IP-CIDR,127.0.0.1/32,REJECT",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("target.test", 443, b"by name").await;
    association.send("127.0.0.1", 443, b"by address").await;
    // the association stays open meanwhile: ending it would end the flows
    // still being routed
    let records = finished(&h, 2).await;
    let of = |host: &str| {
        records
            .iter()
            .find(|r| r.dst.starts_with(host))
            .unwrap_or_else(|| panic!("no record of {host} in {records:?}"))
    };
    let (named, addressed) = (of("target.test"), of("127.0.0.1"));
    for r in [named, addressed] {
        assert!(matches!(r.status, RecordStatus::Rejected(_)), "{r:?}");
        assert_eq!(r.remote_addr, None, "{r:?}");
    }
    assert!(named.dns_ms.is_some(), "{named:?}");
    assert_eq!(addressed.dns_ms, None, "{addressed:?}");
}

/// `underlying-proxy` carries UDP (M5 design 4.3): the front proxy's
/// association is set up through the back one, and its datagrams go
/// through the back one's association.
#[tokio::test]
async fn udp_goes_through_a_chain() {
    let (back, front) = (
        FakeSocks5::spawn(Socks5Script::default()).await,
        FakeSocks5::spawn(Socks5Script::default()).await,
    );
    let proxies = format!(
        "Back = socks5, 127.0.0.1, {}, udp-relay=true\nFront = socks5, 127.0.0.1, {}, udp-relay=true, underlying-proxy=Back",
        back.addr().port(),
        front.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        rules: "IP-CIDR,127.0.0.1/32,Front",
        ..Profile::default()
    })
    .await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association
        .send("127.0.0.1", echo.port(), b"two hops")
        .await;
    assert_eq!(association.recv().await, (echo, b"two hops".to_vec()));
    let commands: Vec<u8> = back.requests().iter().map(|r| r.command).collect();
    assert_eq!(
        commands,
        [1, 3],
        "the front's control connection, then the back's association"
    );
    assert_eq!(
        front.datagrams(),
        [Target::new(HostName::Ip(echo.ip()), echo.port())]
    );
    assert_eq!(
        back.datagrams().len(),
        1,
        "the front's datagram, to the front's relay"
    );
}

/// A chain whose underlying policy carries no UDP fails the flow and says
/// why.
#[tokio::test]
async fn a_chain_without_udp_fails_the_flow() {
    let front = FakeSocks5::spawn(Socks5Script::default()).await;
    let back = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(front.addr()),
        ..HttpProxyScript::default()
    })
    .await;
    let proxies = format!(
        "Back = http, 127.0.0.1, {}\nFront = socks5, 127.0.0.1, {}, udp-relay=true, underlying-proxy=Back",
        back.addr().port(),
        front.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        rules: "IP-CIDR,127.0.0.1/32,Front",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", 9, b"x").await;
    let records = finished(&h, 1).await;
    assert_eq!(records[0].status, RecordStatus::Failed);
    assert_eq!(
        records[0].error.as_deref(),
        Some("via Back: the underlying policy cannot carry UDP")
    );
}

/// The association — every flow of it — ends with the control connection.
#[tokio::test]
async fn closing_the_control_connection_ends_every_flow() {
    let h = harness(Profile::default()).await;
    let ((one, _), (two, _)) = (udp_echo().await, udp_echo().await);
    let association = udp_associate(h.socks()).await;
    for echo in [one, two] {
        association.send("127.0.0.1", echo.port(), b"x").await;
        association.recv().await;
    }
    assert_eq!(
        h.engine
            .request_log()
            .active()
            .iter()
            .filter(|r| r.transport == Transport::Udp)
            .count(),
        2
    );
    drop(association.control);
    let records = finished(&h, 2).await;
    assert!(
        records.iter().all(|r| r.status == RecordStatus::Completed),
        "{records:?}"
    );
}

/// At most `FLOWS_PER_ASSOCIATION` flows (M5-D11); the rest are dropped.
/// The datagrams go to closed ports, so every flow stays active.
#[tokio::test]
async fn an_association_has_at_most_1024_flows() {
    let h = harness(Profile::default()).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    let mut port = 20000u16;
    let mut sent = 0;
    while sent < 1025 {
        port += 1;
        if port == echo.port() {
            continue;
        }
        association.send("127.0.0.1", port, b"x").await;
        sent += 1;
    }
    wait_until("1024 flows", || {
        h.engine
            .request_log()
            .active()
            .iter()
            .filter(|r| r.transport == Transport::Udp)
            .count()
            == 1024
    })
    .await;
    // a new destination is not taken
    association.send("127.0.0.1", echo.port(), b"late").await;
    assert!(association.quiet_for(Duration::from_millis(300)).await);
    drop(association);
    wait_until("every flow to finish", || {
        !h.engine
            .request_log()
            .active()
            .iter()
            .any(|r| r.transport == Transport::Udp)
    })
    .await;
    // the log keeps the newest 1000 records: the late destination would be among them
    let late = format!("127.0.0.1:{}", echo.port());
    assert!(!udp_records(&h).iter().any(|r| r.dst == late));
}

/// Killing a flow through the API ends it and finishes its record; the next
/// datagram to the same destination starts a new flow.
#[tokio::test]
async fn a_killed_flow_ends_and_the_next_datagram_starts_a_new_one() {
    let h = harness(Profile::default()).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"one").await;
    association.recv().await;
    let id = h
        .engine
        .request_log()
        .active()
        .iter()
        .find(|r| r.transport == Transport::Udp)
        .expect("the flow is active")
        .id;
    assert!(h.engine.kill(id));
    let records = finished(&h, 1).await;
    assert_eq!(records[0].id, id);
    association.send("127.0.0.1", echo.port(), b"two").await;
    assert_eq!(association.recv().await, (echo, b"two".to_vec()));
    let records = finished(&h, 1).await;
    assert_eq!(records.len(), 1, "the second flow is still active");
}

/// A carrier whose upstream association has closed fails its flows, and the
/// next flow opens a new one.
#[tokio::test]
async fn a_dead_carrier_is_not_used_again() {
    let (echo, _) = udp_echo().await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        udp_close_after: Some(1),
        ..Socks5Script::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Up = socks5, 127.0.0.1, {}, udp-relay=true",
            upstream.addr().port()
        ),
        rules: "IP-CIDR,127.0.0.1/32,Up,no-resolve",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"one").await;
    assert_eq!(association.recv().await, (echo, b"one".to_vec()));
    // the upstream hangs up after that answer: the flow fails
    let records = finished(&h, 1).await;
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    // another destination: a new carrier, and it works
    let (other, _) = udp_echo().await;
    association.send("127.0.0.1", other.port(), b"two").await;
    assert_eq!(association.recv().await, (other, b"two".to_vec()));
}

/// Through a proxy that looks the names up, the answers come from addresses
/// no flow sent to: each counts for the flow sent to a name on its port,
/// not all for the oldest flow on the carrier.
#[tokio::test]
async fn answers_for_names_count_for_their_flows() {
    let ((one, _), (two, _)) = (udp_echo().await, udp_echo().await);
    let upstream = FakeSocks5::spawn(Socks5Script {
        udp_names_to: Some(one.ip()),
        ..Socks5Script::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Up = socks5, 127.0.0.1, {}, udp-relay=true",
            upstream.addr().port()
        ),
        rules: "DOMAIN-SUFFIX,test,Up",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("one.test", one.port(), b"1").await;
    assert_eq!(association.recv().await, (one, b"1".to_vec()));
    association.send("two.test", two.port(), b"22").await;
    assert_eq!(association.recv().await, (two, b"22".to_vec()));
    drop(association);
    let records = finished(&h, 2).await;
    for (dst, down) in [
        (format!("one.test:{}", one.port()), 1),
        (format!("two.test:{}", two.port()), 2),
    ] {
        let r = records.iter().find(|r| r.dst == dst).expect("a record");
        assert_eq!(r.down, down, "{r:?}");
    }
}

/// While their outbound is being opened, the flows of an association hold
/// what comes for them up to 1 MiB in all; more is dropped and starts no
/// flow. Here 60,000 bytes for each of 30 targets, behind a proxy that
/// never answers: 17 fit.
#[tokio::test]
async fn what_waits_for_an_outbound_is_bounded_in_bytes() {
    // takes the connections, and never says a word
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = silent.local_addr().unwrap().port();
    let _silent = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = silent.accept().await {
            held.push(stream);
        }
    });
    let h = harness(Profile {
        proxies: &format!("Up = socks5, 127.0.0.1, {port}, udp-relay=true"),
        rules: "IP-CIDR,127.0.0.1/32,Up,no-resolve",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    let big = vec![7u8; 60_000];
    for target in 1..=30 {
        association.send("127.0.0.1", target, &big).await;
        // rurge takes each before the next comes
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let waiting = || {
        h.engine
            .request_log()
            .active()
            .into_iter()
            .filter(|r| r.transport == Transport::Udp)
            .count()
    };
    wait_until("the first flow", || waiting() > 0).await;
    // only to see that no more flows start
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(waiting(), 17);
}

/// A refused flow stays a minute, dropping what comes for it; what it was
/// sent first is gone with the refusal, and holds no room from the flows
/// after it: 20 times 60,000 bytes start 20 flows.
#[tokio::test]
async fn a_refused_flow_holds_no_room() {
    let h = harness(Profile {
        rules: "IP-CIDR,10.0.0.0/8,REJECT,no-resolve",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    let big = vec![7u8; 60_000];
    for target in 1..=20 {
        association.send("10.0.0.1", target, &big).await;
        // rurge takes each before the next comes
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let records = finished(&h, 20).await;
    assert!(
        records
            .iter()
            .all(|r| matches!(r.status, RecordStatus::Rejected(_))),
        "{records:?}"
    );
}

/// A flow whose outbound fails to open drops what comes for it for a few
/// seconds from the failure only — however much keeps coming — and then the
/// next datagram starts a new flow: a client that keeps sending recovers
/// once the outbound does.
#[tokio::test]
async fn a_failed_flow_is_tried_again_while_datagrams_keep_coming() {
    let (echo, _) = udp_echo().await;
    let upstream = FakeSocks5::spawn(Socks5Script::default()).await;
    // the proxy hangs up on every connection until `open` is set
    let gate = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = gate.local_addr().unwrap().port();
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _gate = tokio::spawn({
        let (open, to) = (open.clone(), upstream.addr());
        async move {
            loop {
                let (mut stream, _) = gate.accept().await.unwrap();
                if !open.load(std::sync::atomic::Ordering::SeqCst) {
                    continue;
                }
                tokio::spawn(async move {
                    let mut up = TcpStream::connect(to).await.unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut up).await;
                });
            }
        }
    });
    let h = harness(Profile {
        proxies: &format!("Up = socks5, 127.0.0.1, {port}, udp-relay=true"),
        rules: "IP-CIDR,127.0.0.1/32,Up,no-resolve",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"one").await;
    let records = finished(&h, 1).await;
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    // right after the failure the flow still drops what comes for it
    association.send("127.0.0.1", echo.port(), b"dropped").await;
    assert!(association.quiet_for(Duration::from_millis(500)).await);
    // keep sending: a new flow gets through, well before the flow would be idle
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no new flow while datagrams kept coming"
        );
        association.send("127.0.0.1", echo.port(), b"again").await;
        if !association.quiet_for(Duration::from_millis(250)).await {
            break;
        }
    }
    assert!(
        udp_records(&h)
            .iter()
            .all(|r| r.status == RecordStatus::Failed),
        "the new flow is still active"
    );
}

/// While the engine is suspended the datagrams of a new flow are dropped,
/// and the record says why. The flow fails — it is not refused — so that it
/// is tried again after `RETRY_AFTER`: a client that keeps sending is served
/// as soon as the suspension ends, not when it has been quiet for a minute.
#[tokio::test]
async fn a_suspended_engine_drops_the_datagrams_and_serves_the_flow_again_when_it_ends() {
    let h = harness(Profile::default()).await;
    let (echo, seen) = udp_echo().await;
    h.engine
        .suspend(Some("the update failed: marker-reason".to_string()));
    // the association itself is taken: the listener is not suspended
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"one").await;
    let records = finished(&h, 1).await;
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(
        records[0].error.as_deref(),
        Some(REFUSED_STRICT),
        "{records:?}"
    );
    assert_eq!(records[0].rule, None, "{records:?}");
    assert!(records[0].policy.is_empty(), "{records:?}");
    // The client goes on, a datagram every 500 ms. Each wait only observes
    // that nothing comes back: the first datagram was dropped before
    // anything was sent, so nothing is on its way that 500 ms could miss.
    for _ in 0..3 {
        association.send("127.0.0.1", echo.port(), b"again").await;
        let back = association.recv_within(Duration::from_millis(500)).await;
        assert_eq!(back, None);
    }
    assert!(seen.lock().unwrap().is_empty(), "the target saw a datagram");
    let records = udp_records(&h);
    assert_eq!(records.len(), 1, "no other flow starts: {records:?}");

    h.engine.suspend(None);
    // the flow is tried again 5 seconds after it failed, by the next datagram
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let back = loop {
        association.send("127.0.0.1", echo.port(), b"again").await;
        if let Some(back) = association.recv_within(Duration::from_millis(500)).await {
            break back;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no answer after the suspension ended"
        );
    };
    assert_eq!(back, (echo, b"again".to_vec()));
}

/// What the engine told the client of a datagram: its payload, where it says
/// it came from, and the target of the flow it says it answers.
type Heard = (Vec<u8>, Target, Option<Target>);

/// A UDP client of the test's own, where a SOCKS5 one shows no more than the
/// datagrams: it keeps what the engine tells it — each answer with the flow it
/// says it is for, and every refusal. What it sends out is what the test
/// writes.
struct OwnClient {
    outgoing: tokio::sync::Mutex<mpsc::UnboundedReceiver<(Target, Vec<u8>)>>,
    heard: Mutex<Vec<Heard>>,
    refused: Mutex<Vec<(Target, RejectKind)>>,
}

impl UdpClient for OwnClient {
    fn recv<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let Some((to, payload)) = self.outgoing.lock().await.recv().await else {
                return Err(io::ErrorKind::BrokenPipe.into());
            };
            buf[..payload.len()].copy_from_slice(&payload);
            Ok((payload.len(), to))
        })
    }

    fn send<'a>(
        &'a self,
        payload: &'a [u8],
        from: &'a Target,
        answers: Option<&'a Target>,
    ) -> BoxFuture<'a, io::Result<()>> {
        self.heard
            .lock()
            .unwrap()
            .push((payload.to_vec(), from.clone(), answers.cloned()));
        Box::pin(std::future::ready(Ok(())))
    }

    fn refused(&self, target: &Target, kind: RejectKind) {
        self.refused.lock().unwrap().push((target.clone(), kind));
    }
}

/// An association the engine serves for an `OwnClient`, until it is dropped.
struct OwnAssociation {
    client: Arc<OwnClient>,
    outgoing: mpsc::UnboundedSender<(Target, Vec<u8>)>,
    _serving: AbortOnDropHandle<()>,
}

fn own_association(h: &Harness) -> OwnAssociation {
    let (outgoing, waiting) = mpsc::unbounded_channel();
    let client = Arc::new(OwnClient {
        outgoing: tokio::sync::Mutex::new(waiting),
        heard: Mutex::default(),
        refused: Mutex::default(),
    });
    let mut session = SessionInfo::udp(HostName::parse("127.0.0.1"), 0);
    session.listener = ListenerKind::Socks5;
    let serving = tokio::spawn({
        let (engine, client) = (h.engine.clone(), client.clone());
        async move {
            engine
                .associate(client, session, CancellationToken::new())
                .await
        }
    });
    OwnAssociation {
        client,
        outgoing,
        _serving: AbortOnDropHandle::new(serving),
    }
}

impl OwnAssociation {
    /// The client writes `payload` to `host:port` (an IP literal or a name).
    fn write(&self, host: &str, port: u16, payload: &[u8]) {
        let to = Target::new(HostName::parse(host), port);
        self.outgoing.send((to, payload.to_vec())).unwrap();
    }

    /// Everything the client has been told so far, once it has been told
    /// `count` datagrams.
    async fn heard(&self, count: usize) -> Vec<Heard> {
        wait_until("the client to be told the answers", || {
            self.client.heard.lock().unwrap().len() >= count
        })
        .await;
        self.client.heard.lock().unwrap().clone()
    }

    /// Every refusal the client has been told so far, once it has been told
    /// `count` of them.
    async fn refused(&self, count: usize) -> Vec<(Target, RejectKind)> {
        wait_until("the client to be told the refusals", || {
            self.client.refused.lock().unwrap().len() >= count
        })
        .await;
        self.client.refused.lock().unwrap().clone()
    }
}

/// The record of the flow to `dst` once it has finished. A connection to a
/// closed port is refused after about two seconds on Windows, so this waits
/// longer than `wait_until` does.
async fn flow_record(h: &Harness, dst: &str) -> RequestRecord {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(record) = udp_records(h).into_iter().find(|r| r.dst == dst) {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the flow to {dst} did not finish"))
}

/// An answer says which flow it is for — the target as the client wrote it —
/// when there is only one it can be: the flow DIRECT sent to the address it
/// came from, or the one flow written to a name on its port when the proxy
/// looked the name up. With two flows it may be for either, and the client is
/// told none.
#[tokio::test]
async fn a_reply_says_which_flow_it_answers() {
    // DIRECT sends a name's datagrams to the address it looked the name up
    // to, so the flows to a name and to an address each have an echo of their
    // own: with one, both would be sent to the same address, and no telling
    // which of them an answer is for
    let ((by_name, _), (by_address, _), (looked_up, _)) =
        (udp_echo().await, udp_echo().await, udp_echo().await);
    // `one.test` and `two.test` go through a proxy that looks names up (here:
    // to the same address, whatever the name); everything else goes DIRECT
    let up = FakeSocks5::spawn(Socks5Script {
        udp_names_to: Some(looked_up.ip()),
        ..Socks5Script::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Up = socks5, 127.0.0.1, {}, udp-relay=true",
            up.addr().port()
        ),
        rules: "DOMAIN,one.test,Up\nDOMAIN,two.test,Up",
        ..Profile::default()
    })
    .await;
    let association = own_association(&h);
    let at = |addr: SocketAddr| Target::new(HostName::Ip(addr.ip()), addr.port());
    let written = |host: &str, port: u16| Target::new(HostName::parse(host), port);

    // a name DIRECT looked up and an address: the answers come from the
    // addresses, and say they are for what was written
    association.write("target.test", by_name.port(), b"by name");
    association.write("127.0.0.1", by_address.port(), b"by address");
    let heard = association.heard(2).await;
    let of = |payload: &[u8]| {
        heard
            .iter()
            .find(|(got, _, _)| got == payload)
            .cloned()
            .unwrap_or_else(|| panic!("no answer to {payload:?}: {heard:?}"))
    };
    assert_eq!(
        of(b"by name"),
        (
            b"by name".to_vec(),
            at(by_name),
            Some(written("target.test", by_name.port()))
        )
    );
    assert_eq!(
        of(b"by address"),
        (
            b"by address".to_vec(),
            at(by_address),
            Some(written("127.0.0.1", by_address.port()))
        )
    );

    // a name the proxy looked up: the answer comes from the address it chose,
    // and the one flow written to a name on that port is the one it is for
    association.write("one.test", looked_up.port(), b"first");
    let heard = association.heard(3).await;
    assert_eq!(
        heard[2],
        (
            b"first".to_vec(),
            at(looked_up),
            Some(written("one.test", looked_up.port()))
        )
    );
    // with a second name on the port it may be for either
    association.write("two.test", looked_up.port(), b"second");
    let heard = association.heard(4).await;
    assert_eq!(heard[3], (b"second".to_vec(), at(looked_up), None));
}

/// The client is told of every datagram of a refused flow, in the way the flow
/// was refused: by a rule of the REJECT family, by `block-quic`, by a policy
/// that carries no UDP. It is told nothing of a flow that failed (its outbound
/// would not open, its datagrams would not go out, or the engine was
/// suspended): that flow is tried again after a few seconds, and an error for
/// the application would have it give up on a destination that may be
/// reachable by then.
#[tokio::test]
async fn a_refused_flow_tells_the_client_for_every_datagram() {
    let (web, down) = (closed_tcp_port(), closed_tcp_port());
    let h = harness(Profile {
        general: "block-quic = all",
        proxies: &format!(
            "Web = http, 127.0.0.1, {web}
Down = socks5, 127.0.0.1, {down}, udp-relay=true
V4 = direct, ip-version=v4-only"
        ),
        rules: "DOMAIN,rej.test,REJECT
DOMAIN,drop.test,REJECT-DROP
DOMAIN,nodrop.test,REJECT-NO-DROP
DOMAIN,gif.test,REJECT-TINYGIF
DOMAIN,web.test,Web
DOMAIN,down.test,Down
IP-CIDR6,::1/128,V4,no-resolve",
        ..Profile::default()
    })
    .await;
    let association = own_association(&h);

    // a rule: one flow, a refusal for each of its two datagrams
    association.write("rej.test", 7, b"one");
    association.write("rej.test", 7, b"two");
    // the other three kinds
    for host in ["drop.test", "nodrop.test", "gif.test"] {
        association.write(host, 7, b"x");
    }
    // `block-quic`: a QUIC Initial to port 443
    association.write("quic.test", 443, &quic_initial());
    // a policy that carries no UDP (`udp-policy-not-supported-behaviour` is
    // REJECT unless the profile says otherwise)
    association.write("web.test", 53, b"query");
    let refused = association.refused(7).await;
    let told = |host: &str, port: u16| -> Vec<RejectKind> {
        let to = Target::new(HostName::parse(host), port);
        refused
            .iter()
            .filter(|(target, _)| *target == to)
            .map(|(_, kind)| *kind)
            .collect()
    };
    assert_eq!(told("rej.test", 7), [RejectKind::Reject; 2]);
    assert_eq!(told("drop.test", 7), [RejectKind::Drop]);
    assert_eq!(told("nodrop.test", 7), [RejectKind::NoDrop]);
    assert_eq!(told("gif.test", 7), [RejectKind::TinyGif]);
    assert_eq!(told("quic.test", 443), [RejectKind::Reject]);
    assert_eq!(told("web.test", 53), [RejectKind::Reject]);
    assert_eq!(refused.len(), 7, "{refused:?}");

    // a flow whose outbound cannot be opened (nobody listens at the proxy's
    // port), or whose datagrams cannot go out (a v4-only policy asked for an
    // IPv6 address), fails: not refused, so nothing is told of it, nor of what
    // it drops afterwards
    association.write("down.test", 7, b"one");
    association.write("::1", 9, b"one");
    for dst in ["down.test:7", "::1:9"] {
        let record = flow_record(&h, dst).await;
        assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    }
    for datagram in [&b"two"[..], b"three"] {
        association.write("down.test", 7, datagram);
        association.write("::1", 9, datagram);
    }
    // a suspended engine fails a new flow as well
    h.engine
        .suspend(Some("the update failed: marker-reason".to_string()));
    association.write("later.test", 7, b"one");
    let record = flow_record(&h, "later.test:7").await;
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    assert_eq!(record.error.as_deref(), Some(REFUSED_STRICT), "{record:?}");
    association.write("later.test", 7, b"two");
    // only to see that nothing more is told: the engine drops a datagram of a
    // flow the moment it is queued, a few milliseconds, so 300 ms is far more
    // than it takes
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(association.client.refused.lock().unwrap().len(), 7);
}
