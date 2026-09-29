//! UDP through the TLS family (phase 2 M5b): a SOCKS5 UDP association into
//! rurge, out through `trojan` (UDP ASSOCIATE), `anytls` (UDP over TCP) and
//! `vmess` (command 2, one connection per target) to loopback fakes.

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

async fn through(policy: &str) -> Harness {
    harness(Profile {
        proxies: policy,
        rules: "IP-CIDR,127.0.0.1/32,P,no-resolve",
        ..Profile::default()
    })
    .await
}

/// Two echoes, asked three times through `policy` (a `P = …` line); the
/// flows leave through `P` and end with the association.
async fn two_echoes_through(policy: &str) {
    let h = through(policy).await;
    let ((one, _), (two, _)) = (udp_echo().await, udp_echo().await);
    let association = udp_associate(h.socks()).await;
    for (echo, payload) in [(one, &b"one"[..]), (two, b"two"), (one, b"again")] {
        association.send("127.0.0.1", echo.port(), payload).await;
        assert_eq!(association.recv().await, (echo, payload.to_vec()));
    }
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    for r in udp_records(&h) {
        assert_eq!(r.policy, ["P"], "{r:?}");
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
    }
}

/// After one exchange through `policy`, a stranger writes to the server's
/// end of the association (`server_end`, known once the association is
/// open); returns where the client says the stranger's datagram came from,
/// the stranger's own address and the echo's.
async fn a_stranger_writes(
    policy: &str,
    server_end: impl Fn() -> std::net::SocketAddr,
) -> (
    std::net::SocketAddr,
    std::net::SocketAddr,
    std::net::SocketAddr,
) {
    let h = through(policy).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"hello").await;
    assert_eq!(association.recv().await, (echo, b"hello".to_vec()));
    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    stranger.send_to(b"unasked", server_end()).await.unwrap();
    let (from, payload) = association.recv().await;
    assert_eq!(payload, b"unasked");
    (from, stranger.local_addr().unwrap(), echo)
}

#[tokio::test]
async fn udp_goes_through_trojan() {
    let origin = TestServer::spawn().await;
    let (upstream, params) = trojan_upstream(false, origin_addr(&origin)).await;
    two_echoes_through(&format!(
        "P = trojan, 127.0.0.1, {}, {params}",
        upstream.addr().port()
    ))
    .await;
    let seen = upstream.requests();
    assert_eq!(
        (seen.len(), seen[0].command),
        (1, 3),
        "one UDP ASSOCIATE carries both flows"
    );
    assert_eq!(upstream.datagrams().len(), 3);
}

/// Full cone: whoever reaches the server's end of the association reaches
/// the client, under its own address.
#[tokio::test]
async fn anyone_may_answer_through_trojan() {
    let origin = TestServer::spawn().await;
    let (upstream, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let (from, stranger, _) = a_stranger_writes(
        &format!(
            "P = trojan, 127.0.0.1, {}, {params}",
            upstream.addr().port()
        ),
        || upstream.udp_outside()[0],
    )
    .await;
    assert_eq!(from, stranger);
}

#[tokio::test]
async fn udp_goes_through_anytls() {
    let origin = TestServer::spawn().await;
    let (upstream, params) = anytls_upstream(origin_addr(&origin)).await;
    two_echoes_through(&format!(
        "P = anytls, 127.0.0.1, {}, {params}",
        upstream.addr().port()
    ))
    .await;
    assert_eq!(
        upstream.uot_requests().len(),
        1,
        "one stream carries both flows"
    );
    assert_eq!(upstream.datagrams().len(), 3);
}

#[tokio::test]
async fn anyone_may_answer_through_anytls() {
    let origin = TestServer::spawn().await;
    let (upstream, params) = anytls_upstream(origin_addr(&origin)).await;
    let (from, stranger, _) = a_stranger_writes(
        &format!(
            "P = anytls, 127.0.0.1, {}, {params}",
            upstream.addr().port()
        ),
        || upstream.udp_outside()[0],
    )
    .await;
    assert_eq!(from, stranger);
}

#[tokio::test]
async fn udp_goes_through_vmess_one_connection_per_target() {
    let origin = TestServer::spawn().await;
    let (upstream, params) = vmess_upstream(true, true, origin_addr(&origin)).await;
    two_echoes_through(&format!(
        "P = vmess, 127.0.0.1, {}, {params}",
        upstream.addr().port()
    ))
    .await;
    let commands: Vec<u8> = upstream.requests().iter().map(|r| r.command).collect();
    assert_eq!(commands, [2, 2], "one connection per target");
}

/// Symmetric through vmess: an answer can only come back on its target's
/// connection, and counts as the target's.
#[tokio::test]
async fn through_vmess_every_answer_is_the_targets() {
    let origin = TestServer::spawn().await;
    let (upstream, params) = vmess_upstream(false, false, origin_addr(&origin)).await;
    let (from, _, echo) = a_stranger_writes(
        &format!("P = vmess, 127.0.0.1, {}, {params}", upstream.addr().port()),
        || upstream.udp_outside()[0],
    )
    .await;
    assert_eq!(from, echo);
}
