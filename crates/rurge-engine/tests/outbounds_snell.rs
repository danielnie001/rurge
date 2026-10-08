//! Sessions that leave through `snell` (phase 2 M6 design 4.3, M8 design
//! 4.3): TCP and UDP through the engine to the loopback `FakeSnell` —
//! versions 1 to 5, `reuse`, obfs, the server's refusal, a wrong PSK or
//! version, `underlying-proxy`, reloads, and version 6, which is not
//! supported.

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

/// `S = snell, …` to `fake`, with `params` after the port.
fn snell_line(fake: &FakeSnell, params: &str) -> String {
    format!("S = snell, 127.0.0.1, {}, {params}", fake.addr().port())
}

async fn through(proxies: &str) -> Harness {
    harness(Profile {
        proxies,
        rules: "DOMAIN,target.test,S\nIP-CIDR,127.0.0.1/32,S,no-resolve",
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

/// How many idle connections `S` keeps.
fn idle_now(h: &Harness) -> Option<usize> {
    outbound_now(h, "S").idle_connections()
}

/// One session to the echo through `h`: a round trip, then the client
/// goes and the session finishes.
async fn one_session(h: &Harness, n: usize, payload: &[u8]) -> RequestRecord {
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, payload).await;
    drop(tunnel);
    the_records(h, n).await[n - 1].clone()
}

/// Opens a session whose server never answers with the tunnel: the
/// client's bytes get nothing back, the tunnel closes within the bound,
/// and the record says why.
async fn a_failed_session(h: &Harness) -> RequestRecord {
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    tunnel.write_all(b"hello").await.unwrap();
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), tunnel.read_to_end(&mut rest))
        .await
        .expect("the tunnel closes within the bound");
    assert!(rest.is_empty(), "nothing is passed on");
    let record = the_records(h, 1).await[0].clone();
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    record
}

/// Versions 4 and 5 with and without `reuse`: each carries a tunnel to the
/// echo, and the server is asked for the name.
#[tokio::test]
async fn a_connect_leaves_through_snell_v4_and_v5() {
    let echo = rurge_proto::testing::echo_server().await;
    for params in [
        "psk=s3same, version=4",
        "psk=s3same, version=5",
        "psk=s3same, version=5, reuse=true",
    ] {
        let fake = FakeSnell::spawn(SnellScript {
            connect_to: Some(echo),
            ..SnellScript::new("s3same")
        })
        .await;
        let h = through(&snell_line(&fake, params)).await;
        let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
        echo_through(&mut tunnel, b"through snell").await;
        echo_through(&mut tunnel, &vec![0x5a; 100_000]).await;
        let seen = fake.requests();
        let first = seen.first().expect("the server never saw a request");
        assert_eq!(
            (first.host.as_str(), first.port),
            ("target.test", 7),
            "{params}: the server resolves the name"
        );
        assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
        drop(tunnel);
        let record = the_records(&h, 1).await[0].clone();
        assert_eq!(record.policy, ["S"], "{params}");
        assert!(record.error.is_none(), "{params}: {:?}", record.error);
    }
}

/// With `reuse=true` two sessions one after the other share one Snell
/// connection; without it each has its own.
#[tokio::test]
async fn reuse_carries_sessions_one_after_another_on_one_connection() {
    let echo = rurge_proto::testing::echo_server().await;
    for (reuse, connections) in [("true", 1), ("false", 2)] {
        let fake = FakeSnell::spawn(SnellScript {
            connect_to: Some(echo),
            ..SnellScript::new("s3same")
        })
        .await;
        let h = through(&snell_line(
            &fake,
            &format!("psk=s3same, version=4, reuse={reuse}"),
        ))
        .await;
        for (n, payload) in [(1, &b"first"[..]), (2, b"second")] {
            let record = one_session(&h, n, payload).await;
            assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
            if reuse == "true" {
                // the connection is back in the pool before the next dial
                wait_until("the connection to be pooled", || idle_now(&h) == Some(1)).await;
            }
        }
        assert_eq!(fake.connections(), connections, "reuse={reuse}");
        let places: Vec<(usize, usize)> = fake
            .requests()
            .iter()
            .map(|r| (r.connection, r.tunnel))
            .collect();
        let expected: &[(usize, usize)] = if reuse == "true" {
            &[(0, 0), (0, 1)]
        } else {
            &[(0, 0), (1, 0)]
        };
        assert_eq!(places, expected, "reuse={reuse}");
    }
}

#[tokio::test]
async fn obfs_http_carries_the_session() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = FakeSnell::spawn(SnellScript {
        obfs_http: true,
        connect_to: Some(echo),
        ..SnellScript::new("s3same")
    })
    .await;
    let h = through(&snell_line(
        &fake,
        "psk=s3same, version=5, obfs=http, obfs-host=cdn.test, obfs-uri=/a",
    ))
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"behind the camouflage").await;
    let hello = &fake.obfs_seen()[0];
    assert_eq!(hello.host, format!("cdn.test:{}", fake.addr().port()));
    assert_eq!(hello.uri.as_deref(), Some("/a"));
}

/// The server's error answer fails the session, and the record quotes it.
#[tokio::test]
async fn the_servers_refusal_is_the_sessions_failure() {
    let fake = FakeSnell::spawn(SnellScript {
        refuse: Some((0x05, b"no such host".to_vec())),
        ..SnellScript::new("s3same")
    })
    .await;
    let h = through(&snell_line(&fake, "psk=s3same, version=4")).await;
    let record = a_failed_session(&h).await;
    assert_eq!(
        record.error.as_deref(),
        Some("snell: the server refused: no such host")
    );
}

/// A server that cannot decrypt the request closes without a word: the
/// session fails with that, and the PSK is nowhere in the record.
#[tokio::test]
async fn a_wrong_psk_fails_the_session_closed_without_an_answer() {
    let fake = FakeSnell::spawn(SnellScript::new("right")).await;
    let h = through(&snell_line(&fake, "psk=wr0ngPsk, version=5")).await;
    let record = a_failed_session(&h).await;
    assert_eq!(
        record.error.as_deref(),
        Some("snell: the server closed the connection without answering (wrong psk or version?)")
    );
    assert!(!format!("{record:?}").contains("wr0ngPsk"));
    assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
}

/// Versions 1 to 3 each carry a tunnel to the echo, and so does a line that
/// does not say its version, which is version 1 (phase 2 M8 design 4.3).
/// Connect on 1 and 3, ConnectV2 on 2; `tls` camouflage in front of 1.
#[tokio::test]
async fn a_connect_leaves_through_snell_v1_to_v3() {
    let echo = rurge_proto::testing::echo_server().await;
    for (params, version, command) in [
        ("psk=s3same", 1, 1),
        ("psk=s3same, version=1, obfs=tls, obfs-host=cdn.test", 1, 1),
        ("psk=s3same, version=2", 2, 5),
        ("psk=s3same, version=3, obfs=http", 3, 1),
    ] {
        let fake = FakeSnell::spawn(SnellScript {
            version,
            obfs_tls: params.contains("obfs=tls"),
            obfs_http: params.contains("obfs=http"),
            connect_to: Some(echo),
            ..SnellScript::new("s3same")
        })
        .await;
        let h = through(&snell_line(&fake, params)).await;
        let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
        echo_through(&mut tunnel, b"through snell").await;
        echo_through(&mut tunnel, &vec![0x5a; 100_000]).await;
        let seen = fake.requests();
        let first = seen.first().expect("the server never saw a request");
        assert_eq!(
            (first.command, first.host.as_str(), first.port),
            (command, "target.test", 7),
            "{params}: the server resolves the name"
        );
        assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
        drop(tunnel);
        let record = the_records(&h, 1).await[0].clone();
        assert_eq!(record.policy, ["S"], "{params}");
        assert!(record.error.is_none(), "{params}: {:?}", record.error);
    }
}

/// Version 2 always reuses: two sessions one after the other share one
/// Snell connection, whatever `reuse` says. Versions 1 and 3 never do.
#[tokio::test]
async fn v2_shares_one_connection_and_v1_and_v3_never_do() {
    let echo = rurge_proto::testing::echo_server().await;
    for (version, params, connections) in [
        (2, "psk=s3same, version=2", 1),
        (2, "psk=s3same, version=2, reuse=false", 1),
        (1, "psk=s3same, reuse=true", 2),
        (3, "psk=s3same, version=3", 2),
    ] {
        let fake = FakeSnell::spawn(SnellScript {
            version,
            connect_to: Some(echo),
            ..SnellScript::new("s3same")
        })
        .await;
        let h = through(&snell_line(&fake, params)).await;
        for (n, payload) in [(1, &b"first"[..]), (2, b"second")] {
            let record = one_session(&h, n, payload).await;
            assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
            if connections == 1 {
                // the connection is back in the pool before the next dial
                wait_until("the connection to be pooled", || idle_now(&h) == Some(1)).await;
            }
        }
        assert_eq!(fake.connections(), connections, "{params}");
        let places: Vec<(usize, usize)> = fake
            .requests()
            .iter()
            .map(|r| (r.connection, r.tunnel))
            .collect();
        let expected: &[(usize, usize)] = if connections == 1 {
            &[(0, 0), (0, 1)]
        } else {
            &[(0, 0), (1, 0)]
        };
        assert_eq!(places, expected, "{params}");
    }
}

/// A line that never said its version used to be refused; now it dials
/// version 1. Against a server that speaks version 4, or with another PSK,
/// the session fails within the bound and its record says what may be
/// wrong, without the PSK.
#[tokio::test]
async fn a_line_without_version_against_another_server_fails_soon_and_says_why() {
    for (version, psk) in [(4, "s3same"), (5, "s3same"), (1, "0therPsk")] {
        let fake = FakeSnell::spawn(SnellScript {
            version,
            ..SnellScript::new(psk)
        })
        .await;
        let h = through(&snell_line(&fake, "psk=s3same")).await;
        let record = a_failed_session(&h).await;
        assert_eq!(
            record.error.as_deref(),
            Some(
                "snell: the server closed the connection without answering (wrong psk or version?)"
            ),
            "a v{version} server with `{psk}`"
        );
        assert!(!format!("{record:?}").contains("s3same"));
        assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
    }
}

/// A server that cannot read the request may reset the connection instead
/// of closing it: the record then says the same two things may be wrong.
#[tokio::test]
async fn a_server_that_resets_fails_the_session_and_says_psk_or_version() {
    let fake = FakeSnell::spawn(SnellScript {
        reset: true,
        ..SnellScript::new("s3same")
    })
    .await;
    // a line without `version` dials version 1; the server speaks version 4
    let h = through(&snell_line(&fake, "psk=s3same")).await;
    let record = a_failed_session(&h).await;
    assert_eq!(
        record.error.as_deref(),
        Some("snell: the server reset the connection without answering (wrong psk or version?)")
    );
    assert!(!format!("{record:?}").contains("s3same"));
    assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
}

/// Version 6 is parsed and rejects at run time, and the record names the
/// version (phase 2 M8 design 4.4).
#[tokio::test]
async fn snell_v6_rejects_and_says_which() {
    let h = harness(Profile {
        proxies: "V6 = snell, 127.0.0.1, 9, psk=s3same, version=6",
        rules: "DOMAIN,target.test,V6",
        ..Profile::default()
    })
    .await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(b"CONNECT target.test:7 HTTP/1.1\r\nHost: target.test:7\r\n\r\n")
        .await
        .unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut answer)).await;
    assert!(
        !answer.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&answer)
    );
    let record = the_records(&h, 1).await[0].clone();
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(
        record.error.as_deref(),
        Some("policy protocol not implemented: snell v6")
    );
}

/// `snell` over a SOCKS5 `underlying-proxy`: the entry is asked for the
/// `snell` server, and the session goes through its tunnel.
#[tokio::test]
async fn snell_goes_through_an_underlying_socks5_proxy() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = FakeSnell::spawn(SnellScript {
        connect_to: Some(echo),
        ..SnellScript::new("s3same")
    })
    .await;
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let h = through(&format!(
        "Entry = socks5, 127.0.0.1, {}\n{}",
        entry.addr().port(),
        snell_line(&fake, "psk=s3same, version=4, underlying-proxy=Entry")
    ))
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"socks5 then snell").await;
    let first = entry.requests()[0].clone();
    assert_eq!(
        (first.command, first.host.as_str(), first.port),
        (1, "127.0.0.1", fake.addr().port())
    );
    assert_eq!(fake.requests()[0].host, "target.test");
}

/// Three datagrams to two echoes through `S` (v5) over one association:
/// every flow leaves through `S`, over one Snell connection, and ends with
/// the association.
#[tokio::test]
async fn udp_goes_through_snell() {
    let fake = FakeSnell::spawn(SnellScript::new("s3same")).await;
    let h = through(&snell_line(&fake, "psk=s3same, version=5, reuse=true")).await;
    let ((one, _), (two, _)) = (udp_echo().await, udp_echo().await);
    let association = udp_associate(h.socks()).await;
    for (echo, payload) in [(one, &b"one"[..]), (two, b"two"), (one, b"again")] {
        association.send("127.0.0.1", echo.port(), payload).await;
        assert_eq!(association.recv().await, (echo, payload.to_vec()));
    }
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    for r in udp_records(&h) {
        assert_eq!(r.policy, ["S"], "{r:?}");
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
    }
    assert_eq!(
        fake.datagrams(),
        [one.to_string(), two.to_string(), one.to_string()]
    );
    let commands: Vec<u8> = fake.requests().iter().map(|r| r.command).collect();
    assert_eq!(commands, [6], "one UDP session");
    assert_eq!(fake.connections(), 1);
}

/// Version 3 carries UDP as 4 and 5 do: datagrams to two echoes over one
/// association leave through `S`, over one Snell connection.
#[tokio::test]
async fn udp_goes_through_snell_v3() {
    let fake = FakeSnell::spawn(SnellScript {
        version: 3,
        ..SnellScript::new("s3same")
    })
    .await;
    let h = through(&snell_line(&fake, "psk=s3same, version=3")).await;
    let ((one, _), (two, _)) = (udp_echo().await, udp_echo().await);
    let association = udp_associate(h.socks()).await;
    for (echo, payload) in [(one, &b"one"[..]), (two, b"two"), (one, b"again")] {
        association.send("127.0.0.1", echo.port(), payload).await;
        assert_eq!(association.recv().await, (echo, payload.to_vec()));
    }
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    for r in udp_records(&h) {
        assert_eq!(r.policy, ["S"], "{r:?}");
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
    }
    assert_eq!(
        fake.datagrams(),
        [one.to_string(), two.to_string(), one.to_string()]
    );
    let commands: Vec<u8> = fake.requests().iter().map(|r| r.command).collect();
    assert_eq!(commands, [6], "one UDP session");
    assert_eq!(fake.connections(), 1);
}

/// Versions 1 and 2 carry no UDP: `udp-policy-not-supported-behaviour`
/// (REJECT by default) decides, and nothing reaches the server.
#[tokio::test]
async fn snell_v1_and_v2_carry_no_udp() {
    for (version, params) in [(1, "psk=s3same"), (2, "psk=s3same, version=2")] {
        let fake = FakeSnell::spawn(SnellScript {
            version,
            ..SnellScript::new("s3same")
        })
        .await;
        let h = through(&snell_line(&fake, params)).await;
        let association = udp_associate(h.socks()).await;
        association.send("127.0.0.1", 53, b"q").await;
        assert!(association.quiet_for(Duration::from_millis(300)).await);
        wait_until("the flow to finish", || !udp_records(&h).is_empty()).await;
        let records = udp_records(&h);
        assert_eq!(
            records[0].status,
            RecordStatus::Rejected("REJECT".into()),
            "{params}"
        );
        assert_eq!(
            records[0].error.as_deref(),
            Some("policy does not support UDP")
        );
        assert_eq!(fake.connections(), 0, "{params}");
    }
}

/// `udp-port` serves v5's QUIC proxy mode, which rurge does not implement:
/// UDP keeps to the policy's own port. Behind Shadow TLS `udp-port` names
/// the bare server, which a camouflage handshake cannot reach.
#[tokio::test]
async fn udp_keeps_to_the_policys_port_whatever_udp_port_says() {
    let fake = FakeSnell::spawn(SnellScript::new("s3same")).await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let h = through(&snell_line(
        &fake,
        &format!("psk=s3same, version=5, udp-port={closed}"),
    ))
    .await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"hello").await;
    assert_eq!(association.recv().await, (echo, b"hello".to_vec()));
    assert_eq!(fake.connections(), 1);
}

/// Full cone: whoever reaches the server's socket for this client reaches
/// the client, under its own address.
#[tokio::test]
async fn anyone_may_answer_through_snell() {
    let fake = FakeSnell::spawn(SnellScript::new("s3same")).await;
    let h = through(&snell_line(&fake, "psk=s3same, version=5")).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"hello").await;
    assert_eq!(association.recv().await, (echo, b"hello".to_vec()));
    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    stranger
        .send_to(b"unasked", fake.udp_outside()[0])
        .await
        .unwrap();
    assert_eq!(
        association.recv().await,
        (stranger.local_addr().unwrap(), b"unasked".to_vec())
    );
}

/// The outbound — and with it the idle connection `reuse` keeps — is kept
/// across a reload that leaves the line alone, and rebuilt when its PSK,
/// version, `reuse` or obfs changes (design 6).
#[tokio::test]
async fn a_reload_keeps_an_unchanged_snell_policy_and_rebuilds_a_changed_one() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = FakeSnell::spawn(SnellScript {
        connect_to: Some(echo),
        ..SnellScript::new("s3same")
    })
    .await;
    let base = "psk=s3same, version=4, reuse=true";
    let proxies = |params: &str, extra: &str| format!("{}\n{extra}", snell_line(&fake, params));
    let h = through(&proxies(base, "")).await;
    let reload = |params: &str, extra: &str| {
        let next = Profile {
            proxies: &proxies(params, extra),
            rules: "DOMAIN,target.test,S",
            ..Profile::default()
        }
        .text(h.dns.addr());
        let dir = h.dir.path().to_path_buf();
        let shared = h.engine.shared();
        async move { runtime(&dir, &next, shared).await }
    };
    one_session(&h, 1, b"before the reload").await;
    wait_until("the connection to be pooled", || idle_now(&h) == Some(1)).await;
    let before = outbound_now(&h, "S");
    h.engine
        .swap_runtime(reload(base, "Other = http, other.example, 8080").await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "S")),
        "an unrelated reload rebuilt S"
    );
    one_session(&h, 2, b"after the reload").await;
    assert_eq!(fake.connections(), 1, "the pooled connection was kept");

    let mut previous = before;
    for changed in [
        "psk=0ther, version=4, reuse=true",
        "psk=0ther, version=5, reuse=true",
        "psk=0ther, version=5, reuse=false",
        "psk=0ther, version=5, reuse=false, obfs=http",
    ] {
        h.engine.swap_runtime(reload(changed, "").await);
        let now = outbound_now(&h, "S");
        assert!(!Arc::ptr_eq(&previous, &now), "kept after: {changed}");
        previous = now;
    }
}
