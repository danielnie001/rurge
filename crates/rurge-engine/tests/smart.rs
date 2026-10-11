//! `smart` groups through the whole engine (phase 2 M3c design §4, §7, §11):
//! a dial that does not connect through the member the group picked goes on
//! to the next ones in line, and every session tells the group's book how the
//! member it used did.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::session::{SessionInfo, Transport};
use rurge_dns::testing::{DoqScript, FakeDoq};
use rurge_dns::{LookupOpts, Source};
use rurge_inbound::{DialError, Dialer};
use rurge_policy::smart::{Health, SiteMemory};
use std::time::Instant;

fn session(host: &str) -> SessionInfo {
    SessionInfo::tcp(HostName::parse(host), 80)
}

/// The chain of a dial of `host` that connects.
async fn chain_of(h: &Harness, host: &str) -> Vec<String> {
    match h.engine.dial(session(host)).await {
        Ok(dialed) => dialed.handle.policy_chain(),
        Err(DialError::Failed { message, .. }) => panic!("{host}: {message}"),
        Err(DialError::Reject { kind, .. }) => panic!("{host}: rejected by {}", kind.name()),
    }
}

/// A SOCKS5 upstream that reaches `origin` whatever it is asked for.
async fn upstream(origin: &TestServer) -> FakeSocks5 {
    FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(origin)),
        ..Socks5Script::default()
    })
    .await
}

/// What the book knows of each of `members` now.
fn healths(h: &Harness, members: &[&str]) -> Vec<Health> {
    let registry = h.engine.registry();
    let now = Instant::now();
    members
        .iter()
        .map(|m| registry.auto().smart.health(m, &outbound_now(h, m), now))
        .collect()
}

/// A dial of `host` that fails: what it failed with, and its session.
async fn failure_of(h: &Harness, host: &str) -> (String, Arc<rurge_inbound::SessionHandle>) {
    match h.engine.dial(session(host)).await {
        Err(DialError::Failed {
            message, handle, ..
        }) => (message, handle),
        Err(DialError::Reject { kind, .. }) => panic!("{host}: rejected by {}", kind.name()),
        Ok(_) => panic!("{host}: connected"),
    }
}

/// A member that does not connect hands the session over to the next one in
/// line: the session goes through that one, and the note says who failed
/// (M3c design §7).
#[tokio::test]
async fn a_member_that_does_not_connect_hands_over_to_the_next() {
    let origin = TestServer::spawn().await;
    let good = upstream(&origin).await;
    let proxies = format!(
        "Dead = socks5, 127.0.0.1, {}\nGood = socks5, 127.0.0.1, {}",
        closed_tcp_port(),
        good.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, Dead, Good",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    // nothing known yet: Dead comes first, in member order
    let dialed = match h.engine.dial(session("target.test")).await {
        Ok(dialed) => dialed,
        Err(_) => panic!("the dial goes through Good"),
    };
    assert_eq!(dialed.handle.policy_chain(), ["S", "Good"]);
    assert_eq!(
        dialed.handle.error().as_deref(),
        Some("smart group `S`: `Dead` failed to connect, used `Good`")
    );
    let site = h
        .engine
        .registry()
        .auto()
        .smart
        .site("target.test", Instant::now());
    assert_eq!(site.failed, ["Dead"]);
}

/// With no member connecting, the session fails once the one picked and the
/// next two have been tried — the fourth is not — and the note names them.
#[tokio::test]
async fn when_no_member_connects_the_session_fails_naming_them() {
    let mut proxies = String::new();
    for name in ["A", "B", "C", "D"] {
        proxies += &format!("{name} = socks5, 127.0.0.1, {}\n", closed_tcp_port());
    }
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, A, B, C, D",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let begun = Instant::now();
    let handle = match h.engine.dial(session("target.test")).await {
        Err(DialError::Failed { handle, .. }) => handle,
        Err(DialError::Reject { .. }) => panic!("rejected"),
        Ok(_) => panic!("nothing listens"),
    };
    assert!(
        begun.elapsed() < Duration::from_secs(11),
        "{:?}",
        begun.elapsed()
    );
    let note = handle.error().unwrap_or_default();
    assert!(
        note.starts_with("smart group `S`: tried `A`, `B`, `C`; "),
        "{note}"
    );
    assert_eq!(handle.policy_chain(), ["S", "C"]);
}

/// A site that is down is no fault of the members (phase 2 M8 design 5.5):
/// every session to it goes through the three of them and fails, time and
/// again, and one member answers for it — the first that failed there. The
/// others are remembered at the site, and nothing is held against them.
#[tokio::test]
async fn a_site_that_is_down_costs_one_member_one_penalty() {
    let mut proxies = String::new();
    let mut upstreams = Vec::new();
    for name in ["A", "B", "C"] {
        // reaches its proxy, which says the target refused the connection
        let upstream = FakeSocks5::spawn(Socks5Script {
            reply: 5,
            ..Socks5Script::default()
        })
        .await;
        proxies += &format!(
            "{name} = socks5, 127.0.0.1, {}, {UNTESTED}\n",
            upstream.addr().port()
        );
        upstreams.push(upstream);
    }
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, A, B, C",
        rules: "DOMAIN,down.test,S",
        ..Profile::default()
    })
    .await;
    for _ in 0..3 {
        let (error, _) = failure_of(&h, "down.test").await;
        assert_eq!(error, "socks5: connection refused");
    }
    for upstream in &upstreams {
        assert_eq!(upstream.requests().len(), 3, "every session tried it");
    }
    assert_eq!(
        healths(&h, &["A", "B", "C"]),
        [
            Health::Unknown { failures: 1 },
            Health::Unknown { failures: 0 },
            Health::Unknown { failures: 0 },
        ]
    );
    let registry = h.engine.registry();
    let mut failed = registry
        .auto()
        .smart
        .site("down.test", Instant::now())
        .failed;
    failed.sort();
    assert_eq!(failed, ["A", "B", "C"]);
}

/// Nor does it cost more when the sessions come all at once, as a browser
/// opens its connections to a host: they all pick the member that is first
/// in line and all fail through it, and it answers for one of them — a
/// member's own failure at a site excuses its next one there as another's
/// does.
#[tokio::test]
async fn a_burst_of_sessions_to_a_site_that_is_down_costs_one_penalty() {
    let mut proxies = String::new();
    let mut upstreams = Vec::new();
    for name in ["A", "B", "C"] {
        let upstream = FakeSocks5::spawn(Socks5Script {
            reply: 5,
            ..Socks5Script::default()
        })
        .await;
        proxies += &format!(
            "{name} = socks5, 127.0.0.1, {}, {UNTESTED}\n",
            upstream.addr().port()
        );
        upstreams.push(upstream);
    }
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, A, B, C",
        rules: "DOMAIN,down.test,S",
        ..Profile::default()
    })
    .await;
    let dials: Vec<_> = (0..6)
        .map(|_| {
            let engine = h.engine.clone();
            tokio::spawn(async move {
                match engine.dial(session("down.test")).await {
                    Err(DialError::Failed { message, .. }) => message,
                    Err(DialError::Reject { kind, .. }) => format!("rejected by {}", kind.name()),
                    Ok(_) => "connected".to_string(),
                }
            })
        })
        .collect();
    for dial in dials {
        assert_eq!(dial.await.unwrap(), "socks5: connection refused");
    }
    for upstream in &upstreams {
        assert_eq!(upstream.requests().len(), 6, "every session tried it");
    }
    // whichever session told first, it told of the member all of them
    // tried first
    assert_eq!(
        healths(&h, &["A", "B", "C"]),
        [
            Health::Unknown { failures: 1 },
            Health::Unknown { failures: 0 },
            Health::Unknown { failures: 0 },
        ]
    );
    let registry = h.engine.registry();
    let mut failed = registry
        .auto()
        .smart
        .site("down.test", Instant::now())
        .failed;
    failed.sort();
    assert_eq!(failed, ["A", "B", "C"]);
}

/// A name no member's request can carry says nothing of the members (phase 2
/// M8 design 5.5): every one of them refuses it alike. The session goes
/// through the member the group picked and no further, fails in that
/// member's own words, and counts against nobody.
#[tokio::test]
async fn a_name_no_member_can_send_is_tried_once_and_counts_against_nobody() {
    let (a, b) = (
        FakeHttpProxy::spawn(HttpProxyScript::default()).await,
        FakeHttpProxy::spawn(HttpProxyScript::default()).await,
    );
    let proxies = format!(
        "A = http, 127.0.0.1, {}, {UNTESTED}\nB = http, 127.0.0.1, {}, {UNTESTED}",
        a.addr().port(),
        b.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, A, B",
        rules: "DOMAIN-SUFFIX,test,S",
        ..Profile::default()
    })
    .await;
    // 300 bytes, every label a valid one: an HTTP proxy's request line has
    // room for it — the proxy is asked, and cannot connect —, the address
    // of a SOCKS5 request has not
    let label = "a".repeat(63);
    let long = format!("{}.{}.test", [label.as_str(); 4].join("."), "a".repeat(39));
    assert_eq!(long.len(), 300);
    // no ASCII form: no member writes a request for it at all
    let unwritable = "x@blocked.test";
    for (host, error) in [
        (long.as_str(), "http proxy answered 502 Bad Gateway"),
        (
            unwritable,
            "the target host name is not valid for an HTTP proxy request",
        ),
    ] {
        let (message, handle) = failure_of(&h, host).await;
        assert_eq!(message, error);
        assert_eq!(handle.policy_chain(), ["S", "A"]);
        assert_eq!(handle.error(), None, "no note of members tried");
    }
    // the long name went to the member the group picked, and nothing to
    // the next in line
    let asked = a.heads();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].request_line.contains(&long));
    assert!(b.heads().is_empty(), "the next member was tried");
    assert_eq!(
        healths(&h, &["A", "B"]),
        [Health::Unknown { failures: 0 }; 2]
    );
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    let now = Instant::now();
    for host in [long.as_str(), unwritable] {
        assert_eq!(smart.site(host, now), SiteMemory::default(), "{host}");
    }
    let members = ["A".to_string(), "B".to_string()];
    assert!(smart.most_used("S", &members, now).is_empty());
}

/// A session that gets its first byte back is a sample of the member it
/// used, and the member worked at the site (M3c design 4.3).
#[tokio::test]
async fn the_first_byte_back_is_reported() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi");
    let good = upstream(&origin).await;
    let proxies = format!("Good = socks5, 127.0.0.1, {}", good.addr().port());
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, Good",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:80").await;
    let body = get(&mut tunnel, "target.test", "/hello").await;
    assert!(body.ends_with("hi"), "{body}");
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the report of the first byte", || {
        smart.site("target.test", Instant::now()).worked == ["Good"]
    })
    .await;
    let good = outbound_now(&h, "Good");
    assert!(matches!(
        smart.health("Good", &good, Instant::now()),
        rurge_policy::smart::Health::Healthy(_)
    ));
}

/// A member that never answers the handshake has its share of the ten
/// seconds, and no more: two members, five seconds each (M3c design §7).
#[tokio::test]
async fn a_member_that_never_answers_has_its_share_of_the_time() {
    let origin = TestServer::spawn().await;
    let good = upstream(&origin).await;
    // takes the connection, and answers the CONNECT a minute later
    let hole = FakeHttpProxy::spawn(HttpProxyScript {
        delay: Duration::from_secs(60),
        ..HttpProxyScript::default()
    })
    .await;
    let proxies = format!(
        "Hole = http, 127.0.0.1, {}\nGood = socks5, 127.0.0.1, {}",
        hole.addr().port(),
        good.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, Hole, Good",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let begun = Instant::now();
    assert_eq!(chain_of(&h, "target.test").await, ["S", "Good"]);
    let took = begun.elapsed();
    assert!(
        took >= Duration::from_secs(4) && took < Duration::from_secs(8),
        "{took:?}"
    );
}

/// A session whose first byte does not come back within three seconds of
/// its outbound being ready counts against the member; the session itself
/// goes on (M3c design 4.3).
#[tokio::test]
async fn three_seconds_without_an_answer_count_against_the_member() {
    // takes the connection and never says a word
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent_addr = silent.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = silent.accept().await {
            held.push(s);
        }
    });
    let quiet = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(silent_addr),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("Quiet = socks5, 127.0.0.1, {}", quiet.addr().port());
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, Quiet",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:80").await;
    tunnel
        .write_all(b"GET / HTTP/1.1\r\nHost: target.test\r\n\r\n")
        .await
        .unwrap();
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the member counted as not answering", || {
        smart.site("target.test", Instant::now()).failed == ["Quiet"]
    })
    .await;
    assert!(
        h.engine
            .request_log()
            .active()
            .iter()
            .any(|r| r.dst == "target.test:80"),
        "the session goes on"
    );
}

/// A plain request whose upstream hangs up before answering counts against
/// the member (M3c design 4.3).
#[tokio::test]
async fn a_plain_request_whose_upstream_hangs_up_counts_against_the_member() {
    // an HTTP proxy that reads the request and hangs up
    let rude = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = rude.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = rude.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
            });
        }
    });
    let proxies = format!("Rude = http, 127.0.0.1, {port}");
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, Rude",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let _ = plain_get(h.http(), "http://target.test/", "target.test").await;
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the member counted as failing", || {
        smart.site("target.test", Instant::now()).failed == ["Rude"]
    })
    .await;
}

/// A `smart` group is an automatic group to the control plane (M3c design
/// 8.3): `select` sets and clears an override, and the test endpoints take
/// it.
#[tokio::test]
async fn a_smart_group_takes_an_override_and_the_test_endpoints() {
    let origin = TestServer::spawn().await;
    let (a, b) = (upstream(&origin).await, upstream(&origin).await);
    let proxies = format!(
        "A = socks5, 127.0.0.1, {}\nB = socks5, 127.0.0.1, {}",
        a.addr().port(),
        b.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, A, B",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    h.engine.select_group("S", "B").await.unwrap();
    assert_eq!(h.engine.group_selection("S").unwrap(), "B");
    assert_eq!(chain_of(&h, "target.test").await, ["S", "B"]);
    h.engine.select_group("S", "").await.unwrap();
    // both upstreams reach the origin: both pass, and both are healthy
    assert_eq!(h.engine.test_group("S").await.unwrap(), ["A", "B"]);
    let results = h.engine.test_results();
    let (_, members) = results
        .iter()
        .find(|(group, _)| group == "S")
        .expect("a smart group has test results");
    assert!(
        members
            .iter()
            .all(|(_, r)| r.as_ref().is_some_and(|r| r.outcome.is_ok())),
        "{members:?}"
    );
}

/// A round a dial asks for tests twelve members of a big `smart` group
/// (M3c design 8.1).
#[tokio::test]
async fn a_dial_asks_for_a_sampled_round_of_a_big_smart_group() {
    let origin = TestServer::spawn().await;
    let up = upstream(&origin).await;
    let names: Vec<String> = (0..13).map(|i| format!("P{i}")).collect();
    let proxies: String = names
        .iter()
        .map(|n| format!("{n} = socks5, 127.0.0.1, {}\n", up.addr().port()))
        .collect();
    let groups = format!("Big = smart, {}", names.join(", "));
    let h = harness(Profile {
        proxies: &proxies,
        groups: &groups,
        rules: "DOMAIN,target.test,Big",
        ..Profile::default()
    })
    .await;
    let _ = chain_of(&h, "target.test").await;
    wait_until("a round of Big", || {
        h.engine.registry().auto().last_round("Big").is_some()
    })
    .await;
    let registry = h.engine.registry();
    let tested = names
        .iter()
        .filter(|n| registry.test_result(n).is_some())
        .count();
    assert_eq!(tested, 12);
}

/// A reload that drops a policy drops what the book knew of it (M3c design
/// 5.1).
#[tokio::test]
async fn a_reload_forgets_what_the_book_knew_of_the_policies_it_drops() {
    let origin = TestServer::spawn().await;
    let (a, b) = (upstream(&origin).await, upstream(&origin).await);
    let proxies = format!(
        "A = socks5, 127.0.0.1, {}\nB = socks5, 127.0.0.1, {}",
        a.addr().port(),
        b.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, A, B",
        ..Profile::default()
    })
    .await;
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    let b_now = outbound_now(&h, "B");
    let now = Instant::now();
    smart.sample("B", &b_now, Duration::from_millis(50), now);
    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    let fewer = text
        .replace(&format!("B = socks5, 127.0.0.1, {}\n", b.addr().port()), "")
        .replace("S = smart, A, B", "S = smart, A");
    h.engine
        .swap_runtime(runtime(h.dir.path(), &fewer, h.engine.shared()).await);
    assert_eq!(
        smart.health("B", &b_now, now),
        rurge_policy::smart::Health::Unknown { failures: 0 }
    );
}

/// A `smart` group over UDP members (`socks5` with `udp-relay`), for the
/// flows to the loopback.
async fn udp_smart(members: &str, proxies: &str) -> Harness {
    harness(Profile {
        proxies,
        groups: &format!("S = smart, {members}"),
        rules: "IP-CIDR,127.0.0.1/32,S,no-resolve",
        ..Profile::default()
    })
    .await
}

/// A UDP flow's first answer is the member's sample, and the member worked
/// at the site (phase 2 M5 design 8.3).
#[tokio::test]
async fn a_udp_answer_is_reported() {
    let good = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Good = socks5, 127.0.0.1, {}, udp-relay=true",
        good.addr().port()
    );
    let h = udp_smart("Good", &proxies).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"hi").await;
    assert_eq!(association.recv().await, (echo, b"hi".to_vec()));
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the report of the answer", || {
        smart.site("127.0.0.1", Instant::now()).worked == ["Good"]
    })
    .await;
    let good = outbound_now(&h, "Good");
    assert!(matches!(
        smart.health("Good", &good, Instant::now()),
        rurge_policy::smart::Health::Healthy(_)
    ));
}

/// A member whose UDP carrier does not open counts against it; UDP tries
/// no other member (phase 2 M5 design 8.3).
#[tokio::test]
async fn a_member_whose_udp_carrier_does_not_open_counts_against_it() {
    let proxies = format!(
        "Dead = socks5, 127.0.0.1, {}, udp-relay=true",
        closed_tcp_port()
    );
    let h = udp_smart("Dead", &proxies).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"hi").await;
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the failure of the member", || {
        smart.site("127.0.0.1", Instant::now()).failed == ["Dead"]
    })
    .await;
}

/// A member whose carrier opens but whose first datagram cannot go out (a
/// VMess server that refuses the connection the datagram dials) counts
/// against it, as one whose carrier does not open.
#[tokio::test]
async fn a_member_whose_udp_send_fails_counts_against_it() {
    let proxies = format!(
        "Refusing = vmess, 127.0.0.1, {}, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true",
        closed_tcp_port()
    );
    let h = udp_smart("Refusing", &proxies).await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"hi").await;
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the failure of the member", || {
        smart.site("127.0.0.1", Instant::now()).failed == ["Refusing"]
    })
    .await;
}

/// Flows that fail at one address through the same member — UDP tries no
/// other — cost it one penalty: here two associations send there, and the
/// member's carrier opens for neither.
#[tokio::test]
async fn udp_flows_failing_at_one_address_cost_their_member_one_penalty() {
    // takes the control connection and hangs up before the association
    let dead = FakeSocks5::spawn(Socks5Script {
        hang_up_after_greeting: true,
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!(
        "Dead = socks5, 127.0.0.1, {}, udp-relay=true, {UNTESTED}",
        dead.addr().port()
    );
    let h = udp_smart("Dead", &proxies).await;
    let (echo, _) = udp_echo().await;
    let (one, two) = (
        udp_associate(h.socks()).await,
        udp_associate(h.socks()).await,
    );
    one.send("127.0.0.1", echo.port(), b"hi").await;
    two.send("127.0.0.1", echo.port(), b"hi").await;
    wait_until("both flows to fail", || udp_records(&h).len() == 2).await;
    for record in udp_records(&h) {
        assert_eq!(
            record.error.as_deref(),
            Some("socks5: the proxy closed the connection during the handshake")
        );
    }
    assert_eq!(healths(&h, &["Dead"]), [Health::Unknown { failures: 1 }]);
    let registry = h.engine.registry();
    let site = registry.auto().smart.site("127.0.0.1", Instant::now());
    assert_eq!(site.failed, ["Dead"]);
}

/// The same over UDP for a name no member can send (phase 2 M8 design 5.5):
/// the flow fails on its first datagram, which never leaves, and the group
/// hears nothing of it — neither a failure of the member nor a use.
#[tokio::test]
async fn a_udp_flow_to_a_name_no_member_can_send_counts_against_nobody() {
    let good = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Good = socks5, 127.0.0.1, {}, udp-relay=true, {UNTESTED}",
        good.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, Good",
        rules: "DOMAIN-SUFFIX,test,S",
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("x@blocked.test", 4000, b"hi").await;
    wait_until("the flow to fail", || !udp_records(&h).is_empty()).await;
    let record = &udp_records(&h)[0];
    assert_eq!(record.policy, ["S", "Good"]);
    assert_eq!(
        record.error.as_deref(),
        Some("socks5: the host name cannot be sent to a SOCKS5 proxy")
    );
    assert!(good.datagrams().is_empty());
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    let now = Instant::now();
    assert_eq!(smart.site("x@blocked.test", now), SiteMemory::default());
    assert_eq!(healths(&h, &["Good"]), [Health::Unknown { failures: 0 }]);
    assert!(smart.most_used("S", &["Good".to_string()], now).is_empty());
}

/// Three seconds without an answer count against the member only where an
/// answer always comes — here port 443 — never on another port, where a
/// game may only send (phase 2 M5 design 8.3). Both flows go to one site,
/// and a member's second failure at a site is excused by its first: were
/// they sent together, a silence counted wrongly would hide behind the
/// one counted rightly. So the flow that must not count goes first and
/// alone, and the other follows once the first has had its three seconds.
#[tokio::test]
async fn udp_silence_counts_only_where_answers_always_come() {
    let quiet = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Quiet = socks5, 127.0.0.1, {}, udp-relay=true, {UNTESTED}",
        quiet.addr().port()
    );
    let h = udp_smart("Quiet", &proxies).await;
    // a socket that takes datagrams and never answers
    let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let association = udp_associate(h.socks()).await;
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;

    association
        .send(
            "127.0.0.1",
            silent.local_addr().unwrap().port(),
            b"only sends",
        )
        .await;
    // the datagram has got there, so the flow is set up and its three
    // seconds are running
    let mut datagram = [0u8; 64];
    tokio::time::timeout(Duration::from_secs(10), silent.recv_from(&mut datagram))
        .await
        .expect("the datagram reaches the silent socket within the bound")
        .unwrap();
    // The one wait here that is for nothing to happen: the three seconds of
    // the flow's silence and half a second more, since a timer that is late
    // would let a wrong count come after the check.
    tokio::time::sleep(Duration::from_millis(3500)).await;
    assert_eq!(
        smart.site("127.0.0.1", Instant::now()),
        SiteMemory::default(),
        "a silence on another port than 53 and 443 was remembered"
    );
    assert_eq!(
        healths(&h, &["Quiet"]),
        [Health::Unknown { failures: 0 }],
        "a silence on another port than 53 and 443 counted against the member"
    );

    association.send("127.0.0.1", 443, b"not quic").await;
    wait_until("the silence on port 443 counted", || {
        smart.site("127.0.0.1", Instant::now()).failed == ["Quiet"]
    })
    .await;
    assert_eq!(healths(&h, &["Quiet"]), [Health::Unknown { failures: 1 }]);
}

/// A harness in follow mode (`encrypted-dns-follow-outbound-mode`) whose
/// resolver asks the encrypted DNS server `server` and nothing else: the
/// sessions `rules` send to the `smart` group `S` of `members` — `proxies`
/// are theirs — go through one of them.
async fn dns_smart(
    general: &str,
    server: &str,
    proxies: &str,
    members: &str,
    rules: &str,
) -> Harness {
    let general = format!(
        "encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = {server}\n\
encrypted-dns-skip-cert-verification = true\n{general}"
    );
    harness(Profile {
        general: &general,
        proxies,
        groups: &format!("S = smart, {members}"),
        rules,
        ..Profile::default()
    })
    .await
}

/// `target.test` through the harness's resolver, answered within the bound.
async fn lookup_target(h: &Harness) -> rurge_dns::DnsResult {
    tokio::time::timeout(
        Duration::from_secs(10),
        h.engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", LookupOpts::default()),
    )
    .await
    .expect("the lookup within the bound")
    .expect("an answer")
}

/// The internal sessions the engine has recorded so far, finished or not.
fn internal_records(h: &Harness) -> Vec<rurge_engine::RequestRecord> {
    let log = h.engine.request_log();
    let mut all = log.recent(50);
    all.extend(log.active());
    all.retain(|r| r.listener == ListenerKind::Internal);
    all
}

/// An internal session — here the QUIC carrier of a DNS server — tells the
/// `smart` group of the member it went through, as any session does: the
/// first datagram back is the member's sample, and the member worked at the
/// server's address.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dns_servers_carrier_is_reported_to_the_smart_group() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let up = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "Good = socks5, 127.0.0.1, {}, udp-relay=true",
        up.addr().port()
    );
    let server = format!("quic://{}", doq.addr());
    let h = dns_smart("", &server, &proxies, "Good", "PROTOCOL,DOQ,S").await;
    let res = lookup_target(&h).await;
    assert_eq!(res.source, Source::Upstream(server));
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the report of the first datagram", || {
        smart.site("127.0.0.1", Instant::now()).worked == ["Good"]
    })
    .await;
    assert_eq!(
        smart.most_used("S", &["Good".to_string()], Instant::now()),
        ["Good"]
    );
    let good = outbound_now(&h, "Good");
    assert!(matches!(
        smart.health("Good", &good, Instant::now()),
        Health::Healthy(_)
    ));
}

/// The same for the connection of a DNS server over TCP.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tcp_dns_session_is_reported_to_the_smart_group() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let up = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!("Good = socks5, 127.0.0.1, {}", up.addr().port());
    let server = format!("tcp://127.0.0.1:{}", dns.addr().port());
    let h = dns_smart("", &server, &proxies, "Good", "PROTOCOL,DNS,S").await;
    let res = lookup_target(&h).await;
    assert_eq!(res.source, Source::Upstream(server));
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the report of the first byte", || {
        smart.site("127.0.0.1", Instant::now()).worked == ["Good"]
    })
    .await;
    assert_eq!(
        smart.most_used("S", &["Good".to_string()], Instant::now()),
        ["Good"]
    );
    let good = outbound_now(&h, "Good");
    assert!(matches!(
        smart.health("Good", &good, Instant::now()),
        Health::Healthy(_)
    ));
}

/// A carrier that goes through DIRECT in place of a member that carries no
/// UDP (`udp-policy-not-supported-behaviour = DIRECT`) is no session of that
/// member: the group is told nothing of it, neither of the use nor of the
/// first datagram back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_carrier_gone_through_direct_in_place_of_a_member_is_not_reported() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let server = format!("quic://{}", doq.addr());
    let h = dns_smart(
        "udp-policy-not-supported-behaviour = DIRECT",
        &server,
        "NoUdp = http, 127.0.0.1, 9",
        "NoUdp",
        "PROTOCOL,DOQ,S",
    )
    .await;
    let res = lookup_target(&h).await;
    assert_eq!(res.source, Source::Upstream(server));
    // it did go by the group and its member, and through DIRECT
    let carriers = internal_records(&h);
    assert!(
        carriers.iter().any(|r| r.transport == Transport::Udp
            && r.policy == ["S", "NoUdp"]
            && r.error.as_deref() == Some("policy does not support UDP; sent through DIRECT")
            && r.down > 0),
        "{carriers:?}"
    );
    // a report would have been made by now: the first datagram back is what
    // the answer came in
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    let now = Instant::now();
    assert_eq!(smart.site("127.0.0.1", now), SiteMemory::default());
    assert!(smart.most_used("S", &["NoUdp".to_string()], now).is_empty());
}

/// A carrier that opens and then cannot send — a MASQUE server without
/// CONNECT-UDP says so only once the first packet is to go — is its
/// member's failure at the server, as a flow's that cannot be sent is: the
/// session ends with it, and tells at once what three silent seconds would
/// have told of one that stayed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_carrier_that_cannot_send_counts_against_its_member_at_once() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let doq = FakeDoq::spawn(&dns, DoqScript::default());
    let proxy = FakeH3Proxy::spawn(H3ProxyScript {
        no_extended_connect: true,
        ..H3ProxyScript::default()
    });
    let proxies = format!(
        "M = masque, 127.0.0.1, {}, skip-cert-verify=true",
        proxy.addr().port()
    );
    let server = format!("quic://{}", doq.addr());
    let h = dns_smart("", &server, &proxies, "M", "PROTOCOL,DOQ,S").await;
    let started = Instant::now();
    let resolver = h.engine.runtime().stack.resolver.clone();
    let lookup =
        tokio::spawn(async move { resolver.lookup("target.test", LookupOpts::default()).await });
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the report of the failure", || {
        smart.site("127.0.0.1", Instant::now()).failed == ["M"]
    })
    .await;
    let told = started.elapsed();
    assert!(told < Duration::from_secs(2), "{told:?}");
    let outcome = tokio::time::timeout(Duration::from_secs(10), lookup)
        .await
        .expect("the lookup within the bound")
        .unwrap();
    assert!(outcome.is_err(), "{outcome:?}");
    assert_eq!(doq.connections(), 0);
}
