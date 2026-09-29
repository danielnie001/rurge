//! The UDP pipeline through the engine (phase 2 M5 design §5): SOCKS5 UDP
//! ASSOCIATE in, rules and policies per destination, one carrier per
//! outbound, full cone.

mod common;
use common::*;
use rurge_config::session::Transport;
use rurge_engine::RequestRecord;

fn udp_records(h: &Harness) -> Vec<RequestRecord> {
    h.engine
        .request_log()
        .recent(4096)
        .into_iter()
        .filter(|r| r.transport == Transport::Udp)
        .collect()
}

/// Waits until `count` UDP records are finished, and returns them.
async fn finished(h: &Harness, count: usize) -> Vec<RequestRecord> {
    wait_until("the UDP flows to finish", || udp_records(h).len() >= count).await;
    udp_records(h)
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
    let closed = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let closed_port = closed.local_addr().unwrap().port();
    drop(closed);
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
