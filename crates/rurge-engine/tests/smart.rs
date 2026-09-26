//! `smart` groups through the whole engine (phase 2 M3c design §4, §7, §11):
//! a dial that does not connect through the member the group picked goes on
//! to the next ones in line, and every session tells the group's book how the
//! member it used did.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::session::SessionInfo;
use rurge_inbound::{DialError, Dialer};
use std::time::Instant;

/// A loopback port nothing listens on.
async fn closed_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

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

/// A member that does not connect hands the session over to the next one in
/// line: the session goes through that one, and the note says who failed
/// (M3c design §7).
#[tokio::test]
async fn a_member_that_does_not_connect_hands_over_to_the_next() {
    let origin = TestServer::spawn().await;
    let good = upstream(&origin).await;
    let proxies = format!(
        "Dead = socks5, 127.0.0.1, {}\nGood = socks5, 127.0.0.1, {}",
        closed_port().await,
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
        proxies += &format!("{name} = socks5, 127.0.0.1, {}\n", closed_port().await);
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
