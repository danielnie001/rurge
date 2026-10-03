//! Sessions that leave through `tuic-v5` and `tuic` (phase 2 M7 design 6):
//! TCP and UDP through the engine to the loopback `FakeTuic` — the policy's
//! one QUIC connection in either version, UDP in QUIC datagrams (v5's in
//! fragments, v4's whole or not at all) and on streams where the server
//! takes no datagrams, a refused password and a refused token, port
//! hopping, `underlying-proxy` (a SOCKS5 proxy's UDP), policy tests and
//! reloads.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::spec::ProtoSpec;
use rurge_net::connector::Target;
use rurge_net::quic::testing::MultiPort;
use rurge_proto::testing::{echo_server, udp_echo_server};

/// The manual's example user, and its 16 bytes.
const ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
const UUID: [u8; 16] = [
    0x02, 0x33, 0xd1, 0x1c, 0x15, 0xa4, 0x47, 0xd3, 0xad, 0xe3, 0x48, 0xff, 0xca, 0x0c, 0xe1, 0x19,
];
/// What the fakes let in: v5's password, v4's token.
const PASSWORD: &str = "s3same";
const TOKEN: &str = "t0kenV4";

/// The TUIC a test speaks.
#[derive(Clone, Copy, Debug)]
enum Version {
    /// `tuic-v5`.
    V5,
    /// `tuic`: tuic 0.8.5's.
    V4,
}

impl Version {
    /// The line's keyword.
    fn keyword(self) -> &'static str {
        match self {
            Version::V5 => "tuic-v5",
            Version::V4 => "tuic",
        }
    }

    /// The credentials its fake lets in, as a line writes them.
    fn credentials(self) -> String {
        match self {
            Version::V5 => format!("uuid={ID}, password={PASSWORD}"),
            Version::V4 => format!("token={TOKEN}"),
        }
    }

    /// A fake that speaks it and lets `credentials` in.
    fn script(self) -> TuicScript {
        match self {
            Version::V5 => TuicScript {
                uuid: UUID,
                password: PASSWORD.into(),
                ..TuicScript::default()
            },
            Version::V4 => TuicScript::v4(TOKEN),
        }
    }
}

/// `T = <version's keyword>, 127.0.0.1, <the fake's port>, <params>`.
fn line(version: Version, fake: &FakeTuic, params: &str) -> String {
    format!(
        "T = {}, 127.0.0.1, {}, {params}",
        version.keyword(),
        fake.addr().port()
    )
}

/// `line` with the credentials the fake lets in and nothing else.
fn plain_line(version: Version, fake: &FakeTuic) -> String {
    line(version, fake, &version.credentials())
}

/// A profile that sends the name `target.test` and the loopback through
/// `T`.
fn profile(proxies: &str) -> Profile<'_> {
    Profile {
        proxies,
        rules: "DOMAIN,target.test,T\nIP-CIDR,127.0.0.1/32,T,no-resolve",
        ..Profile::default()
    }
}

/// What makes an engine trust `fake`'s certificate.
fn trusting(fake: &FakeTuic) -> EngineShared {
    EngineShared {
        roots: Some(fake.roots()),
        ..EngineShared::default()
    }
}

/// An engine on `proxies` (`profile`) that trusts `fake`.
async fn through(fake: &FakeTuic, proxies: &str) -> Harness {
    harness_with(profile(proxies), trusting(fake)).await
}

/// A CONNECT through the HTTP listener and one through the SOCKS5 listener
/// each leave as a stream of the policy's one QUIC connection, after one
/// authentication, in either version; the server is asked for the name.
#[tokio::test]
async fn tcp_leaves_through_either_version_on_one_connection() {
    let echo = echo_server().await;
    for version in [Version::V5, Version::V4] {
        let fake = FakeTuic::spawn(TuicScript {
            connect_to: Some(echo),
            ..version.script()
        });
        let h = through(&fake, &plain_line(version, &fake)).await;
        let mut http = connect_via_http(h.http(), "target.test:7").await;
        echo_through(&mut http, b"through tuic").await;
        echo_through(&mut http, &vec![0x5a; 300_000]).await;
        let mut socks = connect_via_socks5(h.socks(), "target.test", 7).await;
        echo_through(&mut socks, b"and from SOCKS5").await;
        let connects: Vec<(usize, String)> = fake
            .connects()
            .into_iter()
            .map(|c| (c.connection, c.address))
            .collect();
        assert_eq!(
            connects,
            vec![(0, "target.test:7".to_string()); 2],
            "{version:?}"
        );
        assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
        let auths = fake.auths();
        assert_eq!((fake.connections(), auths.len()), (1, 1), "{version:?}");
        assert!(auths[0].accepted, "{version:?}");
        drop((http, socks));
        for record in the_records(&h, 2).await {
            assert_eq!(record.policy, ["T"], "{record:?}");
            assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
        }
    }
}

/// v5 UDP (design 6.2): the datagrams through one client association go in
/// one association of the policy's connection, in QUIC datagrams — to two
/// echoes; whoever writes to the server's socket for it reaches the client
/// (full cone); one longer than a QUIC datagram goes in fragments, both
/// ways, only the first naming the target. The association goes with the
/// client's: the server is told.
#[tokio::test]
async fn udp_leaves_through_tuic_v5_in_datagrams() {
    let fake = FakeTuic::spawn(Version::V5.script());
    let h = through(&fake, &plain_line(Version::V5, &fake)).await;
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

    let packets = fake.udp_packets();
    assert!(
        packets
            .iter()
            .all(|p| (p.connection, p.association, p.on_stream) == (0, 0, false)),
        "{packets:?}"
    );
    let whole: Vec<Option<String>> = packets
        .iter()
        .filter(|p| p.fragments == 1)
        .map(|p| p.address.clone())
        .collect();
    assert_eq!(whole, [one, two, one].map(|echo| Some(echo.to_string())));
    let fragments: Vec<_> = packets.iter().filter(|p| p.fragments > 1).collect();
    assert!(fragments.len() >= 3, "{fragments:?}");
    assert!(
        fragments.iter().all(|p| p.packet == fragments[0].packet
            && usize::from(p.fragments) == fragments.len()
            && p.address == (p.fragment == 0).then(|| two.to_string())),
        "{fragments:?}"
    );
    assert_eq!(fragments.iter().map(|p| p.len).sum::<usize>(), long.len());
    assert_eq!(fake.connections(), 1);
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    for r in udp_records(&h) {
        assert_eq!(r.policy, ["T"], "{r:?}");
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
    }
    wait_until("the association to be dissociated", || {
        fake.dissociates()
            .iter()
            .map(|d| (d.connection, d.association))
            .eq([(0, 0)])
    })
    .await;
}

/// v4 UDP (design 6.3): whole packets in QUIC datagrams of one association,
/// whose answers come back under their IPv4 source (tuic 0.8.5 names it in
/// IPv6 form); a packet too long for a datagram is dropped alone — v4
/// cannot split one —: the flow goes on, and its record says why.
#[tokio::test]
async fn v4_udp_goes_whole_in_datagrams_or_not_at_all() {
    let fake = FakeTuic::spawn(Version::V4.script());
    let h = through(&fake, &plain_line(Version::V4, &fake)).await;
    let (one, two) = (udp_echo_server().await, udp_echo_server().await);
    let association = udp_associate(h.socks()).await;
    for (echo, payload) in [(one, &b"one"[..]), (two, b"two")] {
        association.send("127.0.0.1", echo.port(), payload).await;
        assert_eq!(association.recv().await, (echo, payload.to_vec()));
    }
    association
        .send("127.0.0.1", one.port(), &vec![0x4c; 3000])
        .await;
    association.send("127.0.0.1", one.port(), b"after").await;
    assert_eq!(association.recv().await, (one, b"after".to_vec()));

    // one association, its id drawn at random; no packet split
    let packets = fake.udp_packets();
    let id = packets[0].association;
    assert!(
        packets
            .iter()
            .all(|p| (p.connection, p.association, p.on_stream, p.fragments) == (0, id, false, 1)),
        "{packets:?}"
    );
    let seen: Vec<(Option<String>, usize)> =
        packets.iter().map(|p| (p.address.clone(), p.len)).collect();
    assert_eq!(
        seen,
        [
            (Some(one.to_string()), 3),
            (Some(two.to_string()), 3),
            (Some(one.to_string()), 5)
        ]
    );
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    let records = udp_records(&h);
    let to_one = records
        .iter()
        .find(|r| r.dst == one.to_string())
        .expect("the flow to the first echo");
    assert_eq!(to_one.status, RecordStatus::Completed, "{to_one:?}");
    assert!(
        to_one
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("tuic: a datagram longer than ")),
        "{to_one:?}"
    );
}

/// Where the server takes no QUIC datagrams (design 6.2, 6.3: `quic`
/// mode), each packet goes whole on a stream of its own in either version
/// — one too long for a datagram too —, and the answers come back on
/// streams.
#[tokio::test]
async fn udp_goes_on_streams_where_the_server_takes_no_datagrams() {
    let udp_echo = udp_echo_server().await;
    let long: Vec<u8> = (0..3000u32).map(|i| i as u8).collect();
    for version in [Version::V5, Version::V4] {
        let fake = FakeTuic::spawn(TuicScript {
            no_datagrams: true,
            ..version.script()
        });
        let h = through(&fake, &plain_line(version, &fake)).await;
        let association = udp_associate(h.socks()).await;
        for payload in [&b"on a stream"[..], &long] {
            association
                .send("127.0.0.1", udp_echo.port(), payload)
                .await;
            assert_eq!(
                association.recv().await,
                (udp_echo, payload.to_vec()),
                "{version:?}"
            );
        }
        let seen: Vec<(bool, u8, usize)> = fake
            .udp_packets()
            .iter()
            .map(|p| (p.on_stream, p.fragments, p.len))
            .collect();
        assert_eq!(seen, [(true, 1, 11), (true, 1, long.len())], "{version:?}");
        assert_eq!(fake.connections(), 1, "{version:?}");
    }
}

/// A password the server refuses (design 6.2): nothing answers a v5
/// Connect, so the CONNECT is answered and its tunnel then ends; the
/// session fails with the text that says why — the server closed the
/// connection before it relayed anything —, and neither the password nor
/// the UUID is in the record.
#[tokio::test]
async fn a_refused_password_fails_the_session() {
    let fake = FakeTuic::spawn(Version::V5.script());
    let h = through(
        &fake,
        &line(Version::V5, &fake, &format!("uuid={ID}, password=wr0ngPw")),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), tunnel.read_to_end(&mut rest))
        .await
        .expect("the tunnel ends within the bound");
    assert!(rest.is_empty(), "{rest:?}");
    let record = the_records(&h, 1).await[0].clone();
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    assert_eq!(record.error.as_deref(), Some("tuic: authentication failed"));
    let shown = format!("{record:?}");
    assert!(!shown.contains("wr0ngPw") && !shown.contains(ID), "{shown}");
    let auths = fake.auths();
    assert_eq!(auths.len(), 1);
    assert_eq!((auths[0].uuid, auths[0].accepted), (Some(UUID), false));
}

/// A token the server refuses (design 6.3): tuic 0.8.5 closes the
/// connection with its code for that, which the Connect meets while it
/// waits for the server's answer — the CONNECT is answered 502 —, and the
/// session fails with the text the code stands for; the token is nowhere
/// in the record.
#[tokio::test]
async fn a_refused_token_fails_the_session() {
    let fake = FakeTuic::spawn(Version::V4.script());
    let h = through(&fake, &line(Version::V4, &fake, "token=wr0ngTk")).await;
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
    assert_eq!(record.error.as_deref(), Some("tuic: authentication failed"));
    assert!(!format!("{record:?}").contains("wr0ngTk"));
    let auths = fake.auths();
    assert_eq!(auths.len(), 1);
    assert_eq!((auths[0].uuid, auths[0].accepted), (None, false));
}

/// Every `tuic` and `tuic-v5` policy of `cfg` hops every `interval`: the
/// loader takes no `port-hopping-interval` under 5 seconds.
fn hop_every(cfg: &mut rurge_config::Config, interval: Duration) {
    for spec in &mut cfg.specs {
        if let ProtoSpec::Tuic(tuic) = &mut spec.proto
            && let Some(hopping) = &mut tuic.quic.port_hopping
        {
            hopping.interval = interval;
        }
    }
}

/// Port hopping (design 3.3) with a server on three ports: the connection
/// moves among them — a new local socket each time — and its tunnels, old
/// and new, go on.
#[tokio::test]
async fn tunnels_keep_working_while_the_port_hops() {
    let echo = echo_server().await;
    let ports = MultiPort::bind(3);
    let fake = FakeTuic::spawn_on(Version::V5.script(), ports.clone());
    let set: Vec<String> = ports.ports().iter().map(u16::to_string).collect();
    let proxies = line(
        Version::V5,
        &fake,
        &format!(
            "{}, port-hopping={}",
            Version::V5.credentials(),
            set.join(";")
        ),
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

/// `tuic-v5` over a SOCKS5 `underlying-proxy` (design 3.3): the QUIC
/// connection goes in datagrams of the entry's UDP association, to the
/// server as the line names it, and carries TCP and UDP alike.
#[tokio::test]
async fn tuic_goes_over_an_underlying_socks5_proxy() {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    let fake = FakeTuic::spawn(Version::V5.script());
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Entry = socks5, 127.0.0.1, {}, udp-relay=true\n{}",
        entry.addr().port(),
        line(
            Version::V5,
            &fake,
            &format!("{}, underlying-proxy=Entry", Version::V5.credentials())
        )
    );
    let h = through(&fake, &proxies).await;
    let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
    echo_through(&mut tunnel, b"socks5 then tuic").await;
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

/// A policy test fetches the policy's `test-url` through its connection,
/// in either version: one tunnel to the loopback origin, two HEADs on it.
#[tokio::test]
async fn a_policy_test_goes_through_the_connection() {
    for version in [Version::V5, Version::V4] {
        let origin = TestServer::spawn().await;
        let fake = FakeTuic::spawn(version.script());
        let params = format!("{}, test-url={}", version.credentials(), origin.url("/"));
        let h = through(&fake, &line(version, &fake, &params)).await;
        let results = h
            .engine
            .test_policies(&["T".to_string()], None)
            .await
            .unwrap();
        let result = results[0].1.as_ref().expect("a tuic policy is tested");
        assert!(result.outcome.is_ok(), "{version:?}: {:?}", result.outcome);
        let port = origin.url("/").port().unwrap();
        let tunnels: Vec<String> = fake.connects().into_iter().map(|c| c.address).collect();
        assert_eq!(tunnels, [format!("127.0.0.1:{port}")], "{version:?}");
        let methods: Vec<String> = origin.requests().into_iter().map(|r| r.method).collect();
        assert_eq!(methods, ["HEAD", "HEAD"], "{version:?}");
    }
}

/// The outbound — and with it its QUIC connection — is kept across a
/// reload that leaves the line alone, and rebuilt when its credentials,
/// `ecn`, port hopping or version change (design 3.5, 8).
#[tokio::test]
async fn a_reload_keeps_an_unchanged_policy_and_its_connection() {
    let echo = echo_server().await;
    let fake = FakeTuic::spawn(TuicScript {
        connect_to: Some(echo),
        ..Version::V5.script()
    });
    let base = plain_line(Version::V5, &fake);
    let h = through(&fake, &base).await;
    let reload = |proxies: String| {
        let next = profile(&proxies).text(h.dns.addr());
        let dir = h.dir.path().to_path_buf();
        let shared = h.engine.shared();
        async move { runtime(&dir, &next, shared).await }
    };
    one_session(&h, 1, b"before the reload").await;
    let before = outbound_now(&h, "T");
    h.engine
        .swap_runtime(reload(format!("{base}\nOther = http, other.example, 8080")).await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "T")),
        "an unrelated reload rebuilt T"
    );
    one_session(&h, 2, b"after the reload").await;
    let places: Vec<usize> = fake.connects().iter().map(|c| c.connection).collect();
    assert_eq!(places, [0, 0], "the QUIC connection was kept");
    assert_eq!(fake.connections(), 1);

    let other = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
    let mut previous = before;
    for changed in [
        line(Version::V5, &fake, &format!("uuid={ID}, password=0ther")),
        line(Version::V5, &fake, &format!("uuid={other}, password=0ther")),
        line(
            Version::V5,
            &fake,
            &format!("uuid={other}, password=0ther, ecn=off"),
        ),
        line(
            Version::V5,
            &fake,
            &format!("uuid={other}, password=0ther, ecn=off, port-hopping=5000-6000"),
        ),
        // the other version
        line(Version::V4, &fake, "token=0ther"),
        line(Version::V4, &fake, "token=an0ther"),
    ] {
        h.engine.swap_runtime(reload(changed.clone()).await);
        let now = outbound_now(&h, "T");
        assert!(!Arc::ptr_eq(&previous, &now), "kept after: {changed}");
        previous = now;
    }
}
