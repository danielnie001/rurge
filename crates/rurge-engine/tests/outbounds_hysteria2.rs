//! Sessions that leave through `hysteria2` (phase 2 M7 design 5): TCP and
//! UDP through the engine to the loopback `FakeHysteria2` — the policy's
//! one QUIC connection, a refused password, `Hysteria-UDP: false`, both
//! obfuscations, port hopping, `underlying-proxy` (a SOCKS5 proxy's UDP, a
//! group whose member changes), the relay of a group, policy tests and
//! reloads.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::spec::Hysteria2Obfs;
use rurge_net::connector::Target;
use rurge_net::quic::testing::MultiPort;
use rurge_proto::testing::{echo_server, udp_echo_server};

/// The password the fake lets in.
const PASSWORD: &str = "s3same";

fn script() -> Hysteria2Script {
    Hysteria2Script {
        password: PASSWORD.into(),
        ..Hysteria2Script::default()
    }
}

/// `H = hysteria2, 127.0.0.1, <the fake's port>, <params>`.
fn line(fake: &FakeHysteria2, params: &str) -> String {
    format!("H = hysteria2, 127.0.0.1, {}, {params}", fake.addr().port())
}

/// `line` with the password and nothing else.
fn plain_line(fake: &FakeHysteria2) -> String {
    line(fake, &format!("password={PASSWORD}"))
}

/// A profile that sends the name `target.test` and the loopback through
/// `H`.
fn profile(proxies: &str) -> Profile<'_> {
    Profile {
        proxies,
        rules: "DOMAIN,target.test,H\nIP-CIDR,127.0.0.1/32,H,no-resolve",
        ..Profile::default()
    }
}

/// What makes an engine trust `fake`'s certificate.
fn trusting(fake: &FakeHysteria2) -> EngineShared {
    EngineShared {
        roots: Some(fake.roots()),
        ..EngineShared::default()
    }
}

/// An engine on `proxies` (`profile`) that trusts `fake`.
async fn through(fake: &FakeHysteria2, proxies: &str) -> Harness {
    harness_with(profile(proxies), trusting(fake)).await
}

/// A CONNECT through the HTTP listener and one through the SOCKS5 listener
/// each leave as a stream of the policy's one QUIC connection, after one
/// authentication; the server is asked for the name.
#[tokio::test]
async fn tcp_leaves_through_hysteria2_on_one_connection() {
    let echo = echo_server().await;
    let fake = FakeHysteria2::spawn(Hysteria2Script {
        connect_to: Some(echo),
        ..script()
    });
    let h = through(&fake, &plain_line(&fake)).await;
    let mut http = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut http, b"through hysteria2").await;
    echo_through(&mut http, &vec![0x5a; 300_000]).await;
    let mut socks = connect_via_socks5(h.socks(), "target.test", 7).await;
    echo_through(&mut socks, b"and from SOCKS5").await;
    let requests: Vec<(usize, String)> = fake
        .tcp_requests()
        .into_iter()
        .map(|r| (r.connection, r.address))
        .collect();
    assert_eq!(requests, vec![(0, "target.test:7".to_string()); 2]);
    assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
    assert_eq!((fake.connections(), fake.auths().len()), (1, 1));
    drop((http, socks));
    for record in the_records(&h, 2).await {
        assert_eq!(record.policy, ["H"], "{record:?}");
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    }
}

/// A password the server refuses fails the session with the text that
/// says so — the CONNECT is answered 502 — and the password is nowhere in
/// the record.
#[tokio::test]
async fn a_refused_password_fails_the_session() {
    let fake = FakeHysteria2::spawn(script());
    let h = through(&fake, &line(&fake, "password=wr0ngPw")).await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(b"CONNECT target.test:7 HTTP/1.1\r\nHost: target.test:7\r\n\r\n")
        .await
        .unwrap();
    let mut answer = [0u8; 12];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut answer))
        .await
        .expect("the CONNECT is answered within the bound")
        .unwrap();
    assert_eq!(&answer, b"HTTP/1.1 502");
    let record = the_records(&h, 1).await[0].clone();
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    assert_eq!(
        record.error.as_deref(),
        Some("hysteria2: authentication failed")
    );
    assert!(!format!("{record:?}").contains("wr0ngPw"));
    assert_eq!(fake.auths()[0].field("hysteria-auth"), Some("wr0ngPw"));
    assert!(fake.tcp_requests().is_empty());
}

/// Datagrams through one association to two echoes go in one session of
/// the policy's connection (session 1); whoever writes to the server's
/// socket for that session reaches the client (full cone); a datagram
/// longer than a QUIC datagram goes in fragments, both ways.
#[tokio::test]
async fn udp_leaves_through_hysteria2_in_datagrams() {
    let fake = FakeHysteria2::spawn(script());
    let h = through(&fake, &plain_line(&fake)).await;
    let (one, two) = (udp_echo_server().await, udp_echo_server().await);
    let association = udp_associate(h.socks()).await;
    for (echo, payload) in [(one, &b"one"[..]), (two, b"two"), (one, b"again")] {
        association.send("127.0.0.1", echo.port(), payload).await;
        assert_eq!(association.recv().await, (echo, payload.to_vec()));
    }
    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    stranger
        .send_to(b"unasked", fake.udp_outside()[0])
        .await
        .unwrap();
    assert_eq!(
        association.recv().await,
        (stranger.local_addr().unwrap(), b"unasked".to_vec())
    );
    // QUIC datagrams of a loopback path hold about 1,200 to 1,400 bytes
    let long: Vec<u8> = (0..3000u32).map(|i| i as u8).collect();
    association.send("127.0.0.1", two.port(), &long).await;
    assert_eq!(association.recv().await, (two, long.clone()));

    let messages = fake.udp_messages();
    assert!(
        messages.iter().all(|m| (m.connection, m.session) == (0, 1)),
        "{messages:?}"
    );
    let whole: Vec<String> = messages
        .iter()
        .filter(|m| m.fragments == 1)
        .map(|m| m.address.clone())
        .collect();
    assert_eq!(whole, [one, two, one].map(|echo| echo.to_string()));
    let fragments: Vec<_> = messages.iter().filter(|m| m.fragments > 1).collect();
    assert!(fragments.len() >= 3, "{fragments:?}");
    assert!(
        fragments.iter().all(|m| m.packet != 0
            && m.packet == fragments[0].packet
            && usize::from(m.fragments) == fragments.len()
            && m.address == two.to_string()),
        "{fragments:?}"
    );
    assert_eq!(fragments.iter().map(|m| m.len).sum::<usize>(), long.len());
    assert_eq!(fake.connections(), 1);
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    for r in udp_records(&h) {
        assert_eq!(r.policy, ["H"], "{r:?}");
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
    }
}

/// A server that relays no UDP (`Hysteria-UDP: false`) fails the flow with
/// the text that says so; nothing goes out as a datagram, and TCP goes on
/// over the same connection.
#[tokio::test]
async fn a_server_that_relays_no_udp_fails_the_flow() {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    let fake = FakeHysteria2::spawn(Hysteria2Script {
        no_udp: true,
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
        Some("hysteria2: the server does not allow UDP")
    );
    assert!(fake.udp_messages().is_empty());
    let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
    echo_through(&mut tunnel, b"TCP still").await;
    assert_eq!((fake.connections(), fake.auths().len()), (1, 1));
}

/// With `salamander-password` or `gecko-password`, TCP and UDP go through a
/// server that obfuscates the same way; with Gecko, the handshake's packets
/// came in fragments.
#[tokio::test]
async fn either_obfuscation_carries_tcp_and_udp() {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    for (obfs, param) in [
        (
            Hysteria2Obfs::Salamander("0bfsPw".into()),
            "salamander-password=0bfsPw",
        ),
        (
            Hysteria2Obfs::Gecko("0bfsPw".into()),
            "gecko-password=0bfsPw",
        ),
    ] {
        let gecko = matches!(obfs, Hysteria2Obfs::Gecko(_));
        let fake = FakeHysteria2::spawn(Hysteria2Script {
            obfs: Some(obfs),
            ..script()
        });
        let h = through(
            &fake,
            &line(&fake, &format!("password={PASSWORD}, {param}")),
        )
        .await;
        let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
        echo_through(&mut tunnel, b"obfuscated").await;
        let association = udp_associate(h.socks()).await;
        association
            .send("127.0.0.1", udp_echo.port(), b"obfuscated too")
            .await;
        assert_eq!(
            association.recv().await,
            (udp_echo, b"obfuscated too".to_vec())
        );
        assert_eq!(fake.connections(), 1, "{param}");
        assert_eq!(!fake.gecko_fragments().is_empty(), gecko, "{param}");
    }
}

/// Port hopping (design 3.3) with a server on three ports: the connection
/// moves among them — a new local socket each time — and its tunnels, old
/// and new, go on.
#[tokio::test]
async fn tunnels_keep_working_while_the_port_hops() {
    let echo = echo_server().await;
    let ports = MultiPort::bind(3);
    let fake = FakeHysteria2::spawn_on(script(), ports.clone());
    let set: Vec<String> = ports.ports().iter().map(u16::to_string).collect();
    let proxies = line(
        &fake,
        &format!("password={PASSWORD}, port-hopping={}", set.join(";")),
    );
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

/// `hysteria2` over a SOCKS5 `underlying-proxy` (design 3.3): the QUIC
/// connection goes in datagrams of the entry's UDP association, to the
/// server as the line names it, and carries TCP and UDP alike.
#[tokio::test]
async fn hysteria2_goes_over_an_underlying_socks5_proxy() {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    let fake = FakeHysteria2::spawn(script());
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Entry = socks5, 127.0.0.1, {}, udp-relay=true\n{}",
        entry.addr().port(),
        line(
            &fake,
            &format!("password={PASSWORD}, underlying-proxy=Entry")
        )
    );
    let h = through(&fake, &proxies).await;
    let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
    echo_through(&mut tunnel, b"socks5 then hysteria2").await;
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

/// The `underlying-proxy` is a group: once another member is selected, the
/// next session goes on a new QUIC connection through that one, and the
/// tunnel under way goes on, on the connection it has (design 3.5).
#[tokio::test]
async fn a_new_member_of_the_underlying_group_gets_a_new_connection() {
    let echo = echo_server().await;
    let fake = FakeHysteria2::spawn(Hysteria2Script {
        connect_to: Some(echo),
        ..script()
    });
    let (one, two) = (
        FakeSocks5::spawn(Socks5Script::default()).await,
        FakeSocks5::spawn(Socks5Script::default()).await,
    );
    let proxies = format!(
        "{}\nOne = socks5, 127.0.0.1, {}, udp-relay=true\nTwo = socks5, 127.0.0.1, {}, udp-relay=true",
        line(&fake, &format!("password={PASSWORD}, underlying-proxy=Up")),
        one.addr().port(),
        two.addr().port()
    );
    let h = harness_with(
        Profile {
            proxies: &proxies,
            groups: "Up = select, One, Two",
            rules: "DOMAIN,target.test,H",
            ..Profile::default()
        },
        trusting(&fake),
    )
    .await;
    let mut first = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut first, b"through one").await;
    assert!(two.datagrams().is_empty());
    h.engine.select_group("Up", "Two").await.unwrap();
    let mut second = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut second, b"through two").await;
    assert!(!two.datagrams().is_empty());
    echo_through(&mut first, b"still through one").await;
    let places: Vec<usize> = fake.tcp_requests().iter().map(|r| r.connection).collect();
    assert_eq!(places, [0, 1]);
    assert_eq!(fake.connections(), 2);
}

/// A group's `underlying-proxy` chains its members (`M (via R)`, phase 2
/// M3 design 5.4), but port hopping cannot go through a chain: the member
/// that hops is left out — the load says so —, and the group goes on
/// through the others.
#[tokio::test]
async fn a_hopping_member_of_a_relayed_group_is_left_out() {
    let echo = echo_server().await;
    let fake = FakeHysteria2::spawn(Hysteria2Script {
        connect_to: Some(echo),
        ..script()
    });
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let port = fake.addr().port();
    let proxies = format!(
        "Entry = socks5, 127.0.0.1, {}, udp-relay=true\n\
         Hop = hysteria2, 127.0.0.1, {port}, password={PASSWORD}, port-hopping={port}\n\
         Plain = hysteria2, 127.0.0.1, {port}, password={PASSWORD}",
        entry.addr().port()
    );
    let p = Profile {
        proxies: &proxies,
        groups: "G = select, Hop, Plain, underlying-proxy=Entry",
        rules: "DOMAIN,target.test,G",
        ..Profile::default()
    };
    // any DNS address will do for the load alone
    let loaded = from_text(
        &p.text(SocketAddr::from(([127, 0, 0, 1], 53))),
        std::path::Path::new("t.conf"),
        &LoadOptions::for_tests(),
    );
    let said: Vec<&str> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.code == rurge_config::codes::W_SET_LINES_SKIPPED)
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        said,
        [
            "policy group `G` relays its members through `underlying-proxy`: `Hop` uses `port-hopping`, which cannot go through it; `Hop (via Entry)` is left out"
        ]
    );
    let h = harness_with(p, trusting(&fake)).await;
    let registry = h.engine.registry();
    assert!(registry.contains("Hop") && !registry.contains("Hop (via Entry)"));
    assert_eq!(
        registry.members("G"),
        Some(&["Plain (via Entry)".to_string()][..])
    );
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"over the group's relay").await;
    assert!(!entry.datagrams().is_empty());
    drop(tunnel);
    let record = the_records(&h, 1).await[0].clone();
    assert_eq!(record.policy, ["G", "Plain (via Entry)"]);
}

/// A policy test fetches the policy's `test-url` through its connection:
/// one tunnel to the loopback origin, two HEADs on it.
#[tokio::test]
async fn a_policy_test_goes_through_the_connection() {
    let origin = TestServer::spawn().await;
    let fake = FakeHysteria2::spawn(script());
    let h = through(
        &fake,
        &line(
            &fake,
            &format!("password={PASSWORD}, test-url={}", origin.url("/")),
        ),
    )
    .await;
    let results = h
        .engine
        .test_policies(&["H".to_string()], None)
        .await
        .unwrap();
    let result = results[0].1.as_ref().expect("a hysteria2 policy is tested");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome);
    let port = origin.url("/").port().unwrap();
    let tunnels: Vec<String> = fake.tcp_requests().into_iter().map(|r| r.address).collect();
    assert_eq!(tunnels, [format!("127.0.0.1:{port}")]);
    let methods: Vec<String> = origin.requests().into_iter().map(|r| r.method).collect();
    assert_eq!(methods, ["HEAD", "HEAD"]);
}

/// The outbound — and with it its QUIC connection — is kept across a
/// reload that leaves the line alone, and rebuilt when its password,
/// `download-bandwidth`, obfuscation, `ecn` or port hopping changes
/// (design 3.5, 8).
#[tokio::test]
async fn a_reload_keeps_an_unchanged_policy_and_its_connection() {
    let echo = echo_server().await;
    let fake = FakeHysteria2::spawn(Hysteria2Script {
        connect_to: Some(echo),
        ..script()
    });
    let proxies = |params: &str, extra: &str| format!("{}\n{extra}", line(&fake, params));
    let base = format!("password={PASSWORD}");
    let h = through(&fake, &proxies(&base, "")).await;
    let reload = |params: &str, extra: &str| {
        let next = profile(&proxies(params, extra)).text(h.dns.addr());
        let dir = h.dir.path().to_path_buf();
        let shared = h.engine.shared();
        async move { runtime(&dir, &next, shared).await }
    };
    one_session(&h, 1, b"before the reload").await;
    let before = outbound_now(&h, "H");
    h.engine
        .swap_runtime(reload(&base, "Other = http, other.example, 8080").await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "H")),
        "an unrelated reload rebuilt H"
    );
    one_session(&h, 2, b"after the reload").await;
    let places: Vec<usize> = fake.tcp_requests().iter().map(|r| r.connection).collect();
    assert_eq!(places, [0, 0], "the QUIC connection was kept");
    assert_eq!(fake.connections(), 1);

    let mut previous = before;
    for changed in [
        "password=0ther",
        "password=0ther, download-bandwidth=50",
        "password=0ther, download-bandwidth=50, salamander-password=0bfs",
        "password=0ther, download-bandwidth=50, gecko-password=0bfs",
        "password=0ther, download-bandwidth=50, gecko-password=0bfs, ecn=off",
        "password=0ther, download-bandwidth=50, gecko-password=0bfs, ecn=off, port-hopping=5000-6000",
    ] {
        h.engine.swap_runtime(reload(changed, "").await);
        let now = outbound_now(&h, "H");
        assert!(!Arc::ptr_eq(&previous, &now), "kept after: {changed}");
        previous = now;
    }
}
