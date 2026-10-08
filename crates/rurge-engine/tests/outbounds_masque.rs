//! Sessions that leave through `masque` (phase 2 M7 design 7.1): TCP and
//! UDP through the engine to the loopback `FakeH3Proxy` — classic CONNECTs
//! and CONNECT-UDP streams on the policy's one QUIC connection, a refused
//! login, a server without CONNECT-UDP, a packet too long for a datagram,
//! port hopping, `underlying-proxy` (a SOCKS5 proxy's UDP), policy tests
//! (URL and UDP) and reloads.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_net::connector::Target;
use rurge_net::quic::testing::MultiPort;
use rurge_proto::testing::{echo_server, udp_echo_server};
use rurge_proto_quic::testing::RecordedH3Request;

/// The credentials the fake lets in, as a line writes them.
const LOGIN: &str = "username=u, password=p";
/// `proxy-authorization` for them: base64("u:p").
const BASIC: &str = "Basic dTpw";

/// A fake that lets the user `u` in with the password `p`.
fn script() -> H3ProxyScript {
    H3ProxyScript {
        users: vec![("u".into(), "p".into())],
        ..H3ProxyScript::default()
    }
}

/// `M = masque, 127.0.0.1, <the fake's port>, <params>`.
fn line(fake: &FakeH3Proxy, params: &str) -> String {
    format!("M = masque, 127.0.0.1, {}, {params}", fake.addr().port())
}

/// `line` with the credentials the fake lets in and nothing else.
fn plain_line(fake: &FakeH3Proxy) -> String {
    line(fake, LOGIN)
}

/// A profile that sends the name `target.test` and the loopback through
/// `M`.
fn profile(proxies: &str) -> Profile<'_> {
    Profile {
        proxies,
        rules: "DOMAIN,target.test,M\nIP-CIDR,127.0.0.1/32,M,no-resolve",
        ..Profile::default()
    }
}

/// What makes an engine trust `fake`'s certificate.
fn trusting(fake: &FakeH3Proxy) -> EngineShared {
    EngineShared {
        roots: Some(fake.roots()),
        ..EngineShared::default()
    }
}

/// An engine on `proxies` (`profile`) that trusts `fake`.
async fn through(fake: &FakeH3Proxy, proxies: &str) -> Harness {
    harness_with(profile(proxies), trusting(fake)).await
}

/// A classic CONNECT to `authority` on connection `n`, with the
/// credentials, as the fake records it.
fn connect(n: usize, authority: &str) -> RecordedH3Request {
    RecordedH3Request {
        connection: n,
        method: "CONNECT".into(),
        protocol: None,
        scheme: None,
        authority: Some(authority.into()),
        path: None,
        headers: vec![("proxy-authorization".into(), BASIC.into())],
    }
}

/// The CONNECT-UDP to `target` on connection 0, with the credentials, as
/// the fake records it: to the server's own authority, the target in the
/// default template's path.
fn connect_udp(fake: &FakeH3Proxy, target: SocketAddr) -> RecordedH3Request {
    RecordedH3Request {
        connection: 0,
        method: "CONNECT".into(),
        protocol: Some("connect-udp".into()),
        scheme: Some("https".into()),
        authority: Some(fake.addr().to_string()),
        path: Some(format!(
            "/.well-known/masque/udp/{}/{}/",
            target.ip(),
            target.port()
        )),
        headers: vec![
            ("proxy-authorization".into(), BASIC.into()),
            ("capsule-protocol".into(), "?1".into()),
        ],
    }
}

/// A CONNECT through the HTTP listener and one through the SOCKS5 listener
/// each leave as a classic CONNECT of their own on the policy's one QUIC
/// connection, both with the credentials; the server is asked for the name.
#[tokio::test]
async fn tcp_leaves_through_masque_on_one_connection() {
    let echo = echo_server().await;
    let fake = FakeH3Proxy::spawn(H3ProxyScript {
        connect_to: Some(echo),
        ..script()
    });
    let h = through(&fake, &plain_line(&fake)).await;
    let mut http = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut http, b"through masque").await;
    echo_through(&mut http, &vec![0x5a; 300_000]).await;
    let mut socks = connect_via_socks5(h.socks(), "target.test", 7).await;
    echo_through(&mut socks, b"and from SOCKS5").await;
    assert_eq!(fake.requests(), vec![connect(0, "target.test:7"); 2]);
    assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
    assert_eq!(fake.connections(), 1);
    drop((http, socks));
    for record in the_records(&h, 2).await {
        assert_eq!(record.policy, ["M"], "{record:?}");
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    }
}

/// Credentials the proxy refuses fail the session with the text that says
/// so — the CONNECT is answered 502 —, and the password is nowhere in the
/// record.
#[tokio::test]
async fn refused_credentials_fail_the_session() {
    let fake = FakeH3Proxy::spawn(script());
    let h = through(&fake, &line(&fake, "username=u, password=wr0ngPw")).await;
    let record = a_failed_session(&h).await;
    assert_eq!(
        record.error.as_deref(),
        Some("masque: proxy authentication required")
    );
    assert!(!format!("{record:?}").contains("wr0ngPw"));
    assert_eq!(fake.requests().len(), 1);
}

/// Datagrams through one association to two echoes (design 7.1): each
/// target has a CONNECT-UDP stream of its own on the policy's one
/// connection, with the credentials; every packet goes in an HTTP/3
/// datagram with context id 0, and every answer counts as its stream's
/// target's. Every flow ends with the association.
#[tokio::test]
async fn udp_goes_through_masque_as_connect_udp() {
    let fake = FakeH3Proxy::spawn(script());
    let h = through(&fake, &plain_line(&fake)).await;
    let (one, two) = (udp_echo_server().await, udp_echo_server().await);
    let association = udp_associate(h.socks()).await;
    for (echo, payload) in [(one, &b"one"[..]), (two, b"two"), (one, b"again")] {
        association.send("127.0.0.1", echo.port(), payload).await;
        assert_eq!(association.recv().await, (echo, payload.to_vec()));
    }
    assert_eq!(
        fake.requests(),
        [connect_udp(&fake, one), connect_udp(&fake, two)]
    );
    let datagrams: Vec<(usize, u64, u64, usize)> = fake
        .datagrams()
        .iter()
        .map(|d| (d.connection, d.stream, d.context, d.len))
        .collect();
    assert_eq!(datagrams, [(0, 0, 0, 3), (0, 4, 0, 3), (0, 0, 0, 5)]);
    assert_eq!(fake.connections(), 1);
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    for r in udp_records(&h) {
        assert_eq!(r.policy, ["M"], "{r:?}");
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
    }
}

/// A server that does not take CONNECT-UDP — no extended CONNECT in its
/// SETTINGS — fails the flow with the text that says so, and no request
/// goes out; TCP goes on over the same connection (design 7.1).
#[tokio::test]
async fn a_server_without_connect_udp_fails_the_flow_and_carries_tcp() {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    let fake = FakeH3Proxy::spawn(H3ProxyScript {
        no_extended_connect: true,
        ..script()
    });
    let h = through(&fake, &plain_line(&fake)).await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", udp_echo.port(), b"q").await;
    wait_until("the flow to finish", || !udp_records(&h).is_empty()).await;
    let records = udp_records(&h);
    assert_eq!(records[0].status, RecordStatus::Failed, "{:?}", records[0]);
    assert_eq!(
        records[0].error.as_deref(),
        Some("masque: the server does not support CONNECT-UDP")
    );
    assert!(fake.requests().is_empty());
    let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
    echo_through(&mut tunnel, b"TCP still").await;
    assert_eq!(fake.requests(), [connect(0, &echo.to_string())]);
    assert_eq!(fake.connections(), 1);
}

/// A packet longer than an HTTP/3 datagram of the connection carries is
/// dropped alone — no DATAGRAM capsule takes it instead (RFC 9297 3.5, RFC
/// 9298 6.1) —: the flow goes on, and its record says why.
#[tokio::test]
async fn a_packet_too_long_for_a_datagram_is_dropped_alone() {
    let fake = FakeH3Proxy::spawn(script());
    let h = through(&fake, &plain_line(&fake)).await;
    let echo = udp_echo_server().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"one").await;
    assert_eq!(association.recv().await, (echo, b"one".to_vec()));
    // QUIC datagrams of a loopback path hold about 1,200 to 1,400 bytes
    association
        .send("127.0.0.1", echo.port(), &vec![0x4c; 3000])
        .await;
    association.send("127.0.0.1", echo.port(), b"after").await;
    assert_eq!(association.recv().await, (echo, b"after".to_vec()));
    let sent: Vec<usize> = fake.datagrams().iter().map(|d| d.len).collect();
    assert_eq!(sent, [3, 5]);
    drop(association);
    wait_until("the flow to finish", || !udp_records(&h).is_empty()).await;
    let record = &udp_records(&h)[0];
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    assert!(
        record
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("masque: a datagram longer than ")),
        "{record:?}"
    );
}

/// Port hopping (design 3.3) with a server on three ports: the connection
/// moves among them — a new local socket each time — and its tunnels, old
/// and new, go on.
#[tokio::test]
async fn tunnels_keep_working_while_the_port_hops() {
    let echo = echo_server().await;
    let ports = MultiPort::bind(3);
    let fake = FakeH3Proxy::spawn_on(script(), ports.clone());
    let set: Vec<String> = ports.ports().iter().map(u16::to_string).collect();
    let proxies = line(&fake, &format!("{LOGIN}, port-hopping={}", set.join(";")));
    let h = harness_edited(profile(&proxies), trusting(&fake), |cfg| {
        hop_every(cfg, Duration::from_millis(100))
    })
    .await;
    let to = echo.to_string();
    let mut early = connect_via_http(h.http(), &to).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while ports.heard().iter().filter(|&&n| n > 0).count() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "heard only on {:?}",
            ports.heard()
        );
        echo_through(&mut early, b"hop").await;
    }
    let mut late = connect_via_http(h.http(), &to).await;
    echo_through(&mut late, b"after a hop").await;
    echo_through(&mut early, b"still there").await;
    assert!(ports.clients() >= 2, "a new local socket per hop");
    assert_eq!(fake.connections(), 1);
}

/// `masque` over a SOCKS5 `underlying-proxy` (design 3.3): the QUIC
/// connection goes in datagrams of the entry's UDP association, to the
/// server as the line names it, and carries TCP and UDP alike.
#[tokio::test]
async fn masque_goes_over_an_underlying_socks5_proxy() {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    let fake = FakeH3Proxy::spawn(script());
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Entry = socks5, 127.0.0.1, {}, udp-relay=true\n{}",
        entry.addr().port(),
        line(&fake, &format!("{LOGIN}, underlying-proxy=Entry"))
    );
    let h = through(&fake, &proxies).await;
    let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
    echo_through(&mut tunnel, b"socks5 then masque").await;
    let association = udp_associate(h.socks()).await;
    association
        .send("127.0.0.1", udp_echo.port(), b"by UDP")
        .await;
    assert_eq!(association.recv().await, (udp_echo, b"by UDP".to_vec()));
    let commands: Vec<u8> = entry.requests().iter().map(|r| r.command).collect();
    assert_eq!(commands, [3], "one UDP association, no TCP");
    let server = Target::new(HostName::Ip(fake.addr().ip()), fake.addr().port());
    let datagrams = entry.datagrams();
    assert!(
        !datagrams.is_empty() && datagrams.iter().all(|to| *to == server),
        "{datagrams:?}"
    );
    assert_eq!(fake.connections(), 1);
}

/// Over an `underlying-proxy` no path MTU is probed, and the connection
/// starts from packets of 1350 bytes: a UDP packet of 1250 bytes — what a
/// browser's QUIC sends — fits a datagram of it and goes there and back,
/// where it was dropped as too long for one.
#[tokio::test]
async fn over_an_underlying_proxy_a_packet_of_1250_bytes_goes_through() {
    let udp_echo = udp_echo_server().await;
    let fake = FakeH3Proxy::spawn(script());
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Entry = socks5, 127.0.0.1, {}, udp-relay=true\n{}",
        entry.addr().port(),
        line(&fake, &format!("{LOGIN}, underlying-proxy=Entry"))
    );
    let h = through(&fake, &proxies).await;
    let association = udp_associate(h.socks()).await;
    association
        .send("127.0.0.1", udp_echo.port(), b"first")
        .await;
    assert_eq!(association.recv().await, (udp_echo, b"first".to_vec()));
    let packet = vec![0x51; 1250];
    // sent again until the answer fits the server's own packets, which grow
    // as it probes its path
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let answer = loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no answer to a packet of 1250 bytes"
        );
        association
            .send("127.0.0.1", udp_echo.port(), &packet)
            .await;
        let window = Duration::from_millis(500);
        if let Some(answer) = association.recv_within(window).await {
            break answer;
        }
    };
    assert!(answer == (udp_echo, packet), "the answer differs");
    assert!(fake.datagrams().iter().any(|d| d.len == 1250));
    drop(association);
    wait_until("the flow to finish", || !udp_records(&h).is_empty()).await;
    let record = &udp_records(&h)[0];
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    assert_eq!(record.error, None, "{record:?}");
}

/// A policy test fetches the policy's `test-url` through its connection:
/// one tunnel to the loopback origin, two HEADs on it.
#[tokio::test]
async fn a_policy_test_goes_through_the_connection() {
    let origin = TestServer::spawn().await;
    let fake = FakeH3Proxy::spawn(script());
    let h = through(
        &fake,
        &line(&fake, &format!("{LOGIN}, test-url={}", origin.url("/"))),
    )
    .await;
    let results = h
        .engine
        .test_policies(&["M".to_string()], None)
        .await
        .unwrap();
    let result = results[0].1.as_ref().expect("a masque policy is tested");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome);
    let port = origin.url("/").port().unwrap();
    assert_eq!(fake.requests(), [connect(0, &format!("127.0.0.1:{port}"))]);
    let methods: Vec<String> = origin.requests().into_iter().map(|r| r.method).collect();
    assert_eq!(methods, ["HEAD", "HEAD"]);
}

/// The UDP test (`test-udp`; phase 2 M5 design 8.4) asks its question
/// through CONNECT-UDP, to the server the line names — which the fake
/// relays to a DNS server on loopback —, and the answer counts as that
/// server's.
#[tokio::test]
async fn a_udp_test_asks_through_connect_udp() {
    let dns = MockDns::spawn().await;
    let fake = FakeH3Proxy::spawn(H3ProxyScript {
        connect_to: Some(dns.addr()),
        ..script()
    });
    let h = through(
        &fake,
        &line(&fake, &format!("{LOGIN}, test-udp=probe.test@192.0.2.53")),
    )
    .await;
    let results = h.engine.test_udp(&["M".to_string()]).await;
    assert!(matches!(results[0].1, Some(Ok(_))), "{results:?}");
    let to = SocketAddr::from(([192, 0, 2, 53], 53));
    assert_eq!(fake.requests(), [connect_udp(&fake, to)]);
    let asked: Vec<String> = dns.queries().into_iter().map(|(q, _)| q.name).collect();
    assert_eq!(asked, ["probe.test"]);
}

/// The outbound — and with it its QUIC connection — is kept across a
/// reload that leaves the line alone, and rebuilt when its credentials,
/// `ecn`, port hopping or `sni` change (design 3.5, 8).
#[tokio::test]
async fn a_reload_keeps_an_unchanged_policy_and_its_connection() {
    let echo = echo_server().await;
    let fake = FakeH3Proxy::spawn(H3ProxyScript {
        connect_to: Some(echo),
        ..script()
    });
    let base = plain_line(&fake);
    let h = through(&fake, &base).await;
    let reload = |proxies: String| {
        let next = profile(&proxies).text(h.dns.addr());
        let dir = h.dir.path().to_path_buf();
        let shared = h.engine.shared();
        async move { runtime(&dir, &next, shared).await }
    };
    one_session(&h, 1, b"before the reload").await;
    let before = outbound_now(&h, "M");
    h.engine
        .swap_runtime(reload(format!("{base}\nOther = http, other.example, 8080")).await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "M")),
        "an unrelated reload rebuilt M"
    );
    one_session(&h, 2, b"after the reload").await;
    let places: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
    assert_eq!(places, [0, 0], "the QUIC connection was kept");
    assert_eq!(fake.connections(), 1);

    let mut previous = before;
    for changed in [
        "username=u, password=0ther",
        "username=v, password=0ther",
        "username=v, password=0ther, ecn=off",
        "username=v, password=0ther, ecn=off, port-hopping=5000-6000",
        "username=v, password=0ther, ecn=off, port-hopping=5000-6000, sni=localhost",
        // without credentials
        "ecn=off, port-hopping=5000-6000, sni=localhost",
    ] {
        h.engine.swap_runtime(reload(line(&fake, changed)).await);
        let now = outbound_now(&h, "M");
        assert!(!Arc::ptr_eq(&previous, &now), "kept after: {changed}");
        previous = now;
    }
}
