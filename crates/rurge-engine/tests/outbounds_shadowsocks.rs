//! Sessions that leave through `ss` (phase 2 M6 design 3.3, 3.4): TCP and
//! UDP through the engine to the loopback `FakeShadowsocks` — the AEAD
//! methods, `none` and SS 2022 with identity headers, both obfs modes,
//! `udp-port`, `underlying-proxy`, reloads, and what is not implemented.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::session::Transport;
use rurge_config::spec::{ObfsMode, SsMethod};
use rurge_engine::RequestRecord;
use rurge_net::connector::Target;

/// SS 2022 keys in Base64: 16 bytes (the server's identity key, two
/// users') and 32 bytes.
const SERVER_16: &str = "MDEyMzQ1Njc4OWFiY2RlZg==";
const USER_16: &str = "ZmVkY2JhOTg3NjU0MzIxMA==";
const OTHER_16: &str = "dGhlIG90aGVyIHVzZXIhIQ==";
const KEY_32: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

fn udp_records(h: &Harness) -> Vec<RequestRecord> {
    h.engine
        .request_log()
        .recent(4096)
        .into_iter()
        .filter(|r| r.transport == Transport::Udp)
        .collect()
}

/// `S = ss, …` to `fake`, with `params` after the port.
fn ss_line(fake: &FakeShadowsocks, params: &str) -> String {
    format!("S = ss, 127.0.0.1, {}, {params}", fake.addr().port())
}

async fn through(proxies: &str) -> Harness {
    harness(Profile {
        proxies,
        rules: "DOMAIN,target.test,S\nIP-CIDR,127.0.0.1/32,S,no-resolve",
        ..Profile::default()
    })
    .await
}

/// The only session's record, once it has finished.
async fn the_record(h: &Harness) -> RequestRecord {
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    log.recent(10)[0].clone()
}

/// An AEAD method, `none`, and SS 2022 as the second of two users: each
/// carries a tunnel to the echo, and the server is asked for the name.
#[tokio::test]
async fn a_connect_leaves_through_ss_with_every_kind_of_method() {
    let echo = rurge_proto::testing::echo_server().await;
    for (script, params, user) in [
        (
            ShadowsocksScript::new(SsMethod::Aes256Gcm, "s3same"),
            "encrypt-method=aes-256-gcm, password=s3same".to_string(),
            None,
        ),
        (
            ShadowsocksScript::new(SsMethod::None, ""),
            "encrypt-method=none".to_string(),
            None,
        ),
        (
            ShadowsocksScript {
                users: vec![OTHER_16.to_string(), USER_16.to_string()],
                ..ShadowsocksScript::new(SsMethod::Blake3Aes128Gcm, SERVER_16)
            },
            format!("encrypt-method=2022-blake3-aes-128-gcm, password={SERVER_16}:{USER_16}"),
            Some(1),
        ),
    ] {
        let fake = FakeShadowsocks::spawn(ShadowsocksScript {
            connect_to: Some(echo),
            ..script
        })
        .await;
        let h = through(&ss_line(&fake, &params)).await;
        let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
        echo_through(&mut tunnel, b"through ss").await;
        echo_through(&mut tunnel, &vec![0x5a; 100_000]).await;
        let seen = fake.requests();
        let first = seen.first().expect("the server never saw a request");
        assert_eq!(
            (first.atyp, first.host.as_str(), first.port, first.user),
            (3, "target.test", 7, user),
            "{params}: the server resolves the name"
        );
        assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
        drop(tunnel);
        let record = the_record(&h).await;
        assert_eq!(record.policy, ["S"], "{params}");
        assert!(record.error.is_none(), "{params}: {:?}", record.error);
    }
}

#[tokio::test]
async fn both_obfs_modes_carry_the_session() {
    let echo = rurge_proto::testing::echo_server().await;
    for (mode, extra) in [
        (ObfsMode::Http, "obfs=http, obfs-host=cdn.test, obfs-uri=/a"),
        (ObfsMode::Tls, "obfs=tls, obfs-host=cdn.test"),
    ] {
        let fake = FakeShadowsocks::spawn(ShadowsocksScript {
            obfs: Some(mode),
            connect_to: Some(echo),
            ..ShadowsocksScript::new(SsMethod::ChaCha20IetfPoly1305, "s3same")
        })
        .await;
        let h = through(&ss_line(
            &fake,
            &format!("encrypt-method=chacha20-ietf-poly1305, password=s3same, {extra}"),
        ))
        .await;
        let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
        echo_through(&mut tunnel, b"behind the camouflage").await;
        let hello = &fake.obfs_seen()[0];
        match mode {
            ObfsMode::Http => {
                assert_eq!(hello.host, format!("cdn.test:{}", fake.addr().port()));
                assert_eq!(hello.uri.as_deref(), Some("/a"));
            }
            ObfsMode::Tls => assert_eq!(hello.host, "cdn.test"),
        }
    }
}

/// An SS 2022 answer whose clock is an hour off fails the session, and
/// the record says why (design 3.3).
#[tokio::test]
async fn a_server_clock_an_hour_off_fails_the_session_with_the_reason() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = FakeShadowsocks::spawn(ShadowsocksScript {
        connect_to: Some(echo),
        answer_skew: 3600,
        ..ShadowsocksScript::new(SsMethod::Blake3Aes256Gcm, KEY_32)
    })
    .await;
    let h = through(&ss_line(
        &fake,
        &format!("encrypt-method=2022-blake3-aes-256-gcm, password={KEY_32}"),
    ))
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    tunnel.write_all(b"what time is it").await.unwrap();
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), tunnel.read_to_end(&mut rest))
        .await
        .expect("the tunnel closes within the bound");
    assert!(rest.is_empty(), "nothing of the answer is passed on");
    let record = the_record(&h).await;
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    let error = record.error.unwrap_or_default();
    assert!(
        error.starts_with("ss: the server's clock differs from ours by ")
            && error.ends_with(" seconds (at most 30 are allowed)"),
        "{error}"
    );
}

/// A stream cipher is parsed and rejects at run time, and the record names
/// the cipher (design 3.1).
#[tokio::test]
async fn a_stream_cipher_rejects_and_says_which() {
    let h = harness(Profile {
        proxies: "Rc = ss, 127.0.0.1, 9, encrypt-method=rc4-md5, password=s3same",
        rules: "DOMAIN,target.test,Rc",
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
    let record = the_record(&h).await;
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(
        record.error.as_deref(),
        Some("policy protocol not implemented: ss (rc4-md5)")
    );
}

/// `ss` over a SOCKS5 `underlying-proxy`: the entry is asked for the `ss`
/// server, TCP through its tunnel, UDP through its association.
#[tokio::test]
async fn ss_goes_through_an_underlying_socks5_proxy_for_tcp_and_udp() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = FakeShadowsocks::spawn(ShadowsocksScript {
        connect_to: Some(echo),
        ..ShadowsocksScript::new(SsMethod::Aes128Gcm, "s3same")
    })
    .await;
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let h = through(&format!(
        "Entry = socks5, 127.0.0.1, {}, udp-relay=true\n{}",
        entry.addr().port(),
        ss_line(
            &fake,
            "encrypt-method=aes-128-gcm, password=s3same, udp-relay=true, underlying-proxy=Entry"
        )
    ))
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"socks5 then ss").await;
    let first = entry.requests()[0].clone();
    assert_eq!(
        (first.command, first.host.as_str(), first.port),
        (1, "127.0.0.1", fake.addr().port())
    );
    assert_eq!(fake.requests()[0].host, "target.test");

    let (udp_echo_addr, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association
        .send("127.0.0.1", udp_echo_addr.port(), b"socks5 then ss, by UDP")
        .await;
    assert_eq!(
        association.recv().await,
        (udp_echo_addr, b"socks5 then ss, by UDP".to_vec())
    );
    let commands: Vec<u8> = entry.requests().iter().map(|r| r.command).collect();
    assert_eq!(commands, [1, 3], "the tunnel, then the association");
    assert_eq!(
        entry.datagrams(),
        [Target::new(
            HostName::Ip(fake.udp_addr().ip()),
            fake.udp_addr().port()
        )],
        "the ss datagram went to the server through the entry"
    );
    assert_eq!(fake.datagrams()[0].payload, b"socks5 then ss, by UDP");
}

/// Three datagrams to two echoes through `S` over one association: every flow leaves
/// through `S` and ends with the association.
async fn udp_through(fake: &FakeShadowsocks, params: &str) {
    let h = through(&ss_line(fake, params)).await;
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
    assert_eq!(fake.datagrams().len(), 3, "{params}");
    assert_eq!(fake.connections(), 0, "{params}: no TCP");
}

#[tokio::test]
async fn udp_goes_through_ss_aead_and_2022() {
    let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(SsMethod::Aes256Gcm, "s3same")).await;
    udp_through(
        &fake,
        "encrypt-method=aes-256-gcm, password=s3same, udp-relay=true",
    )
    .await;

    let fake = FakeShadowsocks::spawn(ShadowsocksScript {
        users: vec![OTHER_16.to_string(), USER_16.to_string()],
        ..ShadowsocksScript::new(SsMethod::Blake3Aes128Gcm, SERVER_16)
    })
    .await;
    udp_through(
        &fake,
        &format!(
            "encrypt-method=2022-blake3-aes-128-gcm, password={SERVER_16}:{USER_16}, udp-relay=true"
        ),
    )
    .await;
    let seen = fake.datagrams();
    assert!(seen.iter().all(|d| d.user == Some(1)), "{seen:?}");
    let session = seen[0].session.expect("an SS 2022 session").0;
    assert!(
        seen.iter().all(|d| d.session.map(|s| s.0) == Some(session)),
        "one carrier, one session: {seen:?}"
    );
}

/// `udp-port`: datagrams go to that port, not to the policy's.
#[tokio::test]
async fn udp_goes_to_udp_port() {
    let fake = FakeShadowsocks::spawn(ShadowsocksScript {
        udp_apart: true,
        ..ShadowsocksScript::new(SsMethod::Blake3Aes256Gcm, KEY_32)
    })
    .await;
    assert_ne!(fake.udp_addr().port(), fake.addr().port());
    udp_through(
        &fake,
        &format!(
            "encrypt-method=2022-blake3-aes-256-gcm, password={KEY_32}, udp-relay=true, udp-port={}",
            fake.udp_addr().port()
        ),
    )
    .await;
}

/// Full cone: whoever reaches the server's socket for this client reaches
/// the client, under its own address.
#[tokio::test]
async fn anyone_may_answer_through_ss() {
    let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(
        SsMethod::ChaCha20IetfPoly1305,
        "s3same",
    ))
    .await;
    let h = through(&ss_line(
        &fake,
        "encrypt-method=chacha20-ietf-poly1305, password=s3same, udp-relay=true",
    ))
    .await;
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

/// Without `udp-relay=true` the policy carries no UDP:
/// `udp-policy-not-supported-behaviour` (REJECT by default) decides.
#[tokio::test]
async fn without_udp_relay_a_flow_is_rejected() {
    let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(SsMethod::Aes128Gcm, "s3same")).await;
    let h = through(&ss_line(
        &fake,
        "encrypt-method=aes-128-gcm, password=s3same",
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
    assert!(fake.datagrams().is_empty() && fake.connections() == 0);
}

/// The outbound is kept across a reload that leaves the line alone, and
/// rebuilt when its method, password, obfs or `udp-port` changes (design 6).
#[tokio::test]
async fn a_reload_keeps_an_unchanged_ss_policy_and_rebuilds_a_changed_one() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = FakeShadowsocks::spawn(ShadowsocksScript {
        connect_to: Some(echo),
        ..ShadowsocksScript::new(SsMethod::Aes128Gcm, "s3same")
    })
    .await;
    let base = "encrypt-method=aes-128-gcm, password=s3same, udp-relay=true";
    let proxies = |params: &str, extra: &str| format!("{}\n{extra}", ss_line(&fake, params));
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
    let before = outbound_now(&h, "S");
    h.engine
        .swap_runtime(reload(base, "Other = http, other.example, 8080").await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "S")),
        "an unrelated reload rebuilt S"
    );
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"after the reload").await;

    let mut previous = before;
    for changed in [
        "encrypt-method=aes-256-gcm, password=s3same, udp-relay=true",
        "encrypt-method=aes-256-gcm, password=0ther, udp-relay=true",
        "encrypt-method=aes-256-gcm, password=0ther, udp-relay=true, obfs=http",
        "encrypt-method=aes-256-gcm, password=0ther, udp-relay=true, obfs=http, udp-port=9999",
    ] {
        h.engine.swap_runtime(reload(changed, "").await);
        let now = outbound_now(&h, "S");
        assert!(!Arc::ptr_eq(&previous, &now), "kept after: {changed}");
        previous = now;
    }
}
