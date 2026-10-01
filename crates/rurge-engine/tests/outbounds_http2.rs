//! Sessions that leave through the HTTP/2 family (phase 2 M6 design 5):
//! `h2-connect` and `trust-tunnel` through the engine to the loopback
//! `FakeH2Proxy` — TCP tunnels, `max-streams`, refused credentials, UDP
//! over CONNECT-UDP (and where there is none), `underlying-proxy`, reloads.

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

/// A fake HTTP/2 proxy on loopback, and the parameter that makes rurge
/// trust it: the harness trusts the OS roots, so the leaf is pinned.
async fn h2_upstream(script: H2ProxyScript) -> (FakeH2Proxy, String) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let pin = format!("server-cert-fingerprint-sha256={}", pin_of(&fixture));
    (FakeH2Proxy::spawn(script, fixture).await, pin)
}

/// `P = <kind>, …` to `fake`, with `params` after the port.
fn line(kind: &str, fake: &FakeH2Proxy, params: &str) -> String {
    format!("P = {kind}, 127.0.0.1, {}, {params}", fake.addr().port())
}

async fn through(proxies: &str) -> Harness {
    harness(Profile {
        proxies,
        rules: "DOMAIN,target.test,P\nIP-CIDR,127.0.0.1/32,P,no-resolve",
        ..Profile::default()
    })
    .await
}

/// The first `n` sessions' records, oldest first, once they have finished.
async fn the_records(h: &Harness, n: usize) -> Vec<RequestRecord> {
    let log = h.engine.request_log();
    wait_until("the sessions to finish", || log.recent(10).len() >= n).await;
    let mut records = log.recent(10);
    records.reverse();
    records
}

/// One session to the echo through `h`: a round trip, then the client
/// goes and the session finishes.
async fn one_session(h: &Harness, n: usize, payload: &[u8]) -> RequestRecord {
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, payload).await;
    drop(tunnel);
    the_records(h, n).await[n - 1].clone()
}

/// Opens a session the proxy refuses: the client's CONNECT is answered
/// with 502 within the bound, and the record says why.
async fn a_failed_session(h: &Harness) -> RequestRecord {
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(b"CONNECT target.test:7 HTTP/1.1\r\nHost: target.test:7\r\n\r\n")
        .await
        .unwrap();
    let mut answer = [0u8; 12];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut answer))
        .await
        .expect("the CONNECT is answered within the bound")
        .unwrap();
    assert_eq!(
        &answer,
        b"HTTP/1.1 502",
        "{}",
        String::from_utf8_lossy(&answer)
    );
    let record = the_records(h, 1).await[0].clone();
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    record
}

/// `h2-connect` carries a tunnel to the echo: one CONNECT naming the
/// target, the name left to the proxy, Basic credentials sent.
#[tokio::test]
async fn a_connect_leaves_through_h2_connect() {
    let echo = rurge_proto::testing::echo_server().await;
    let (fake, pin) = h2_upstream(H2ProxyScript {
        users: vec![("alice".into(), "s3cret".into())],
        connect_to: Some(echo),
        ..H2ProxyScript::default()
    })
    .await;
    let h = through(&line("h2-connect", &fake, &format!("alice, s3cret, {pin}"))).await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"through h2-connect").await;
    echo_through(&mut tunnel, &vec![0x5a; 300_000]).await;
    let seen = fake.requests();
    assert_eq!(
        (seen[0].method.as_str(), seen[0].authority.as_str()),
        ("CONNECT", "target.test:7"),
        "the proxy resolves the name"
    );
    assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
    drop(tunnel);
    let record = the_records(&h, 1).await[0].clone();
    assert_eq!(record.policy, ["P"]);
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
}

/// Sessions open at the same time share a connection up to `max-streams`;
/// the next one takes another TLS connection.
#[tokio::test]
async fn max_streams_bounds_the_sessions_on_one_connection() {
    let echo = rurge_proto::testing::echo_server().await;
    let (fake, pin) = h2_upstream(H2ProxyScript {
        connect_to: Some(echo),
        ..H2ProxyScript::default()
    })
    .await;
    let h = through(&line("h2-connect", &fake, &format!("max-streams=2, {pin}"))).await;
    let mut tunnels = Vec::new();
    for n in 0..3u8 {
        let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
        echo_through(&mut tunnel, &[n; 16]).await;
        tunnels.push(tunnel);
    }
    // every tunnel is still open
    for tunnel in &mut tunnels {
        echo_through(tunnel, b"still here").await;
    }
    assert_eq!(fake.connections(), 2);
    let places: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
    assert_eq!(places, [0, 0, 1]);
}

/// Credentials the proxy refuses fail the session with the text that says
/// so, and the password is nowhere in the record.
#[tokio::test]
async fn refused_credentials_fail_the_session() {
    let (fake, pin) = h2_upstream(H2ProxyScript {
        users: vec![("alice".into(), "right".into())],
        ..H2ProxyScript::default()
    })
    .await;
    let h = through(&line(
        "h2-connect",
        &fake,
        &format!("alice, wr0ngPw, {pin}"),
    ))
    .await;
    let record = a_failed_session(&h).await;
    assert_eq!(
        record.error.as_deref(),
        Some("h2-connect: proxy authentication required")
    );
    assert!(!format!("{record:?}").contains("wr0ngPw"));
}

/// `trust-tunnel` carries a tunnel with its credentials and `user-agent`.
#[tokio::test]
async fn a_connect_leaves_through_trust_tunnel() {
    let echo = rurge_proto::testing::echo_server().await;
    let (fake, pin) = h2_upstream(H2ProxyScript {
        users: vec![("u".into(), "s3cret".into())],
        require_user_agent: true,
        connect_to: Some(echo),
        ..H2ProxyScript::default()
    })
    .await;
    let h = through(&line(
        "trust-tunnel",
        &fake,
        &format!("username=u, password=s3cret, {pin}"),
    ))
    .await;
    let record = one_session(&h, 1, b"through trust-tunnel").await;
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    let seen = fake.requests();
    assert_eq!(seen[0].authority, "target.test:7");
    assert_eq!(seen[0].header("user-agent"), Some("rurge"));
}

#[tokio::test]
async fn a_refused_trust_tunnel_login_fails_the_session() {
    let (fake, pin) = h2_upstream(H2ProxyScript {
        users: vec![("u".into(), "right".into())],
        ..H2ProxyScript::default()
    })
    .await;
    let h = through(&line(
        "trust-tunnel",
        &fake,
        &format!("username=u, password=wr0ngPw, {pin}"),
    ))
    .await;
    let record = a_failed_session(&h).await;
    assert_eq!(
        record.error.as_deref(),
        Some("trust-tunnel: authentication failed")
    );
    assert!(!format!("{record:?}").contains("wr0ngPw"));
}

/// `trust-tunnel` carries no UDP: `udp-policy-not-supported-behaviour`
/// (REJECT by default) decides, and nothing reaches the server.
#[tokio::test]
async fn trust_tunnel_carries_no_udp() {
    let (fake, pin) = h2_upstream(H2ProxyScript::default()).await;
    let h = through(&line(
        "trust-tunnel",
        &fake,
        &format!("username=u, password=p, {pin}"),
    ))
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", 53, b"q").await;
    assert!(association.quiet_for(Duration::from_millis(300)).await);
    wait_until("the flow to finish", || !udp_records(&h).is_empty()).await;
    let records = udp_records(&h);
    assert_eq!(records[0].status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(
        records[0].error.as_deref(),
        Some("policy does not support UDP")
    );
    assert_eq!(fake.connections(), 0);
}

/// Datagrams to two echoes through `h2-connect` over one association:
/// each target has its own CONNECT-UDP stream, both on one connection,
/// and every flow ends with the association.
#[tokio::test]
async fn udp_goes_through_h2_connect_as_connect_udp() {
    let (fake, pin) = h2_upstream(H2ProxyScript {
        extended_connect: true,
        ..H2ProxyScript::default()
    })
    .await;
    let h = through(&line(
        "h2-connect",
        &fake,
        &format!("udp-relay=true, {pin}"),
    ))
    .await;
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
    let streams: Vec<(Option<String>, String)> = fake
        .requests()
        .into_iter()
        .map(|r| (r.protocol, r.path))
        .collect();
    assert_eq!(
        streams,
        [one, two].map(|echo| (
            Some("connect-udp".to_string()),
            format!("/.well-known/masque/udp/127.0.0.1/{}/", echo.port())
        ))
    );
    assert_eq!(fake.connections(), 1);
}

/// A server without extended CONNECT carries no UDP: the flow fails with
/// the text that says so, and no request reaches the server.
#[tokio::test]
async fn udp_without_extended_connect_fails_the_flow() {
    let (fake, pin) = h2_upstream(H2ProxyScript::default()).await;
    let h = through(&line(
        "h2-connect",
        &fake,
        &format!("udp-relay=true, {pin}"),
    ))
    .await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"q").await;
    wait_until("the flow to finish", || !udp_records(&h).is_empty()).await;
    let records = udp_records(&h);
    assert_eq!(records[0].status, RecordStatus::Failed, "{:?}", records[0]);
    assert_eq!(
        records[0].error.as_deref(),
        Some("h2-connect: the server does not support extended CONNECT")
    );
    assert!(fake.requests().is_empty());
}

/// `h2-connect` over a SOCKS5 `underlying-proxy`: the entry is asked for
/// the HTTP/2 server, and the session goes through its tunnel.
#[tokio::test]
async fn h2_connect_goes_through_an_underlying_socks5_proxy() {
    let echo = rurge_proto::testing::echo_server().await;
    let (fake, pin) = h2_upstream(H2ProxyScript {
        connect_to: Some(echo),
        ..H2ProxyScript::default()
    })
    .await;
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let h = through(&format!(
        "Entry = socks5, 127.0.0.1, {}\n{}",
        entry.addr().port(),
        line(
            "h2-connect",
            &fake,
            &format!("underlying-proxy=Entry, {pin}")
        )
    ))
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"socks5 then h2").await;
    let first = entry.requests()[0].clone();
    assert_eq!(
        (first.command, first.host.as_str(), first.port),
        (1, "127.0.0.1", fake.addr().port())
    );
    assert_eq!(fake.requests()[0].authority, "target.test:7");
}

/// The outbound — and with it its HTTP/2 connection — is kept across a
/// reload that leaves the line alone, and rebuilt when its credentials,
/// `headers`, `max-streams` or `udp-relay` change (design 6).
#[tokio::test]
async fn a_reload_keeps_an_unchanged_policy_and_rebuilds_a_changed_one() {
    let echo = rurge_proto::testing::echo_server().await;
    let (fake, pin) = h2_upstream(H2ProxyScript {
        connect_to: Some(echo),
        ..H2ProxyScript::default()
    })
    .await;
    let proxies = |params: &str, extra: &str| {
        format!(
            "{}\n{extra}",
            line("h2-connect", &fake, &format!("{params}, {pin}"))
        )
    };
    let base = "alice, s3cret";
    let h = through(&proxies(base, "")).await;
    let reload = |params: &str, extra: &str| {
        let next = Profile {
            proxies: &proxies(params, extra),
            rules: "DOMAIN,target.test,P",
            ..Profile::default()
        }
        .text(h.dns.addr());
        let dir = h.dir.path().to_path_buf();
        let shared = h.engine.shared();
        async move { runtime(&dir, &next, shared).await }
    };
    one_session(&h, 1, b"before the reload").await;
    let before = outbound_now(&h, "P");
    h.engine
        .swap_runtime(reload(base, "Other = http, other.example, 8080").await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "P")),
        "an unrelated reload rebuilt P"
    );
    one_session(&h, 2, b"after the reload").await;
    assert_eq!(fake.connections(), 1, "the HTTP/2 connection was kept");
    let places: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
    assert_eq!(places, [0, 0]);

    let mut previous = before;
    for changed in [
        "alice, 0ther",
        "alice, 0ther, headers=X-Id:1",
        "alice, 0ther, headers=X-Id:1, max-streams=8",
        "alice, 0ther, headers=X-Id:1, max-streams=8, udp-relay=true",
    ] {
        h.engine.swap_runtime(reload(changed, "").await);
        let now = outbound_now(&h, "P");
        assert!(!Arc::ptr_eq(&previous, &now), "kept after: {changed}");
        previous = now;
    }
}
