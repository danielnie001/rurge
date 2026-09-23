//! Sessions that leave through the TLS family (trojan, vmess, anytls): the same harness as `outbounds.rs`.

mod common;

use common::*;

#[tokio::test]
async fn a_connect_leaves_through_a_vmess_upstream_with_the_name_unresolved() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    for (tls, ws) in [(false, false), (true, true)] {
        let (upstream, params) = vmess_upstream(tls, ws, origin_addr(&origin)).await;
        let h = harness(Profile {
            proxies: &format!("V = vmess, 127.0.0.1, {}, {params}", upstream.addr().port()),
            rules: "DOMAIN,target.test,V",
            ..Profile::default()
        })
        .await;
        let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
        let response = get(&mut tunnel, "target.test", "/hello").await;
        assert!(
            response.ends_with("hi there"),
            "tls={tls} ws={ws}: {response}"
        );
        let seen = upstream.requests();
        let first = seen.first().expect("the upstream never saw a request");
        assert_eq!(
            (first.command, first.atyp, first.host.as_str(), first.port),
            (1, 2, "target.test", 8080),
            "the server resolves the name: rurge never looked it up"
        );
        assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
        drop(tunnel);
        let log = h.engine.request_log();
        wait_until("the session to finish", || !log.recent(10).is_empty()).await;
        let record = &log.recent(10)[0];
        assert_eq!(record.policy, ["V"]);
        assert!(record.error.is_none(), "{:?}", record.error);
    }
}

#[tokio::test]
async fn a_wrong_vmess_id_ends_the_session_with_a_text_that_says_so() {
    let origin = TestServer::spawn().await;
    let (upstream, _) = vmess_upstream(false, false, origin_addr(&origin)).await;
    let h = harness(Profile {
        proxies: &format!(
            "V = vmess, 127.0.0.1, {}, username=0233d11c-15a4-47d3-ade3-48ffca0ce118, vmess-aead=true",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,V",
        ..Profile::default()
    })
    .await;
    // the tunnel comes up: the server only answers a request it accepts
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    tunnel
        .write_all(b"GET / HTTP/1.1\r\nHost: target.test\r\n\r\n")
        .await
        .unwrap();
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), tunnel.read_to_end(&mut rest))
        .await
        .expect("the tunnel closes within the bound");
    assert_eq!(upstream.rejected(), 1);
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    let error = log.recent(10)[0].error.clone().unwrap_or_default();
    assert_eq!(
        error,
        "vmess: the server closed the connection without answering"
    );
    assert!(!error.contains("0233"));
}

#[tokio::test]
async fn an_anytls_upstream_carries_two_requests_over_one_session() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (upstream, params) = anytls_upstream(origin_addr(&origin)).await;
    let h = harness(Profile {
        proxies: &format!(
            "A = anytls, 127.0.0.1, {}, {params}",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,A",
        ..Profile::default()
    })
    .await;
    let log = h.engine.request_log();
    for round in 1..=2 {
        let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
        let response = get(&mut tunnel, "target.test", "/hello").await;
        assert!(response.ends_with("hi there"), "{response}");
        drop(tunnel);
        // The session's record appears once the relay has let go of the
        // stream, and by then the AnyTLS session is back in the pool. (Not
        // the server's FIN count: `get` asks the origin to close, so it is
        // the server that ends these streams, and a FIN is not answered.)
        wait_until("the session to finish", || log.recent(10).len() == round).await;
    }
    assert_eq!(
        upstream.sessions(),
        1,
        "the second request reused the session"
    );
    assert_eq!(
        upstream.fins(),
        0,
        "both streams were ended by the server (the origin was asked to close, so the \
         server closes first); a peer's cmdFIN is not answered"
    );
    let streams = upstream.streams();
    assert_eq!(
        streams
            .iter()
            .map(|s| (s.sid, s.host.as_str(), s.port))
            .collect::<Vec<_>>(),
        [(1, "target.test", 8080), (2, "target.test", 8080)]
    );
    assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
}

#[tokio::test]
async fn the_new_protocols_work_at_either_end_of_a_chain() {
    let echo = rurge_proto::testing::echo_server().await;
    // entry: vmess; exit: anytls, reached by name through the entry
    let (exit, exit_params) = anytls_upstream(echo).await;
    let (entry, entry_params) = vmess_upstream(false, false, exit.addr()).await;
    let h = harness(Profile {
        proxies: &format!(
            "Entry = vmess, 127.0.0.1, {}, {entry_params}\nExit = anytls, exit.example, 443, {exit_params}, underlying-proxy=Entry",
            entry.addr().port()
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"vmess then anytls").await;
    let asked = &entry.requests()[0];
    assert_eq!(
        (asked.host.as_str(), asked.port),
        ("exit.example", 443),
        "the entry is asked for the exit by name"
    );
    assert_eq!(exit.streams()[0].host, "target.test");

    // and the other way round
    let (exit, exit_params) = vmess_upstream(false, false, echo).await;
    let (entry, entry_params) = anytls_upstream(exit.addr()).await;
    let h = harness(Profile {
        proxies: &format!(
            "Entry = anytls, 127.0.0.1, {}, {entry_params}\nExit = vmess, exit.example, 443, {exit_params}, underlying-proxy=Entry",
            entry.addr().port()
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"anytls then vmess").await;
    assert_eq!(entry.streams()[0].host, "exit.example");
    assert_eq!(exit.requests()[0].host, "target.test");
}

#[tokio::test]
async fn a_connect_leaves_through_a_trojan_upstream_with_the_name_unresolved() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (upstream, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let h = harness(Profile {
        proxies: &format!(
            "T = trojan, 127.0.0.1, {}, {params}",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,T",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let response = get(&mut tunnel, "target.test", "/hello").await;
    assert!(response.ends_with("hi there"), "{response}");
    // whether the head rode with the first payload depends on timing here
    // (`HEAD_GRACE`); `LazyHead`'s own tests pin that deterministically
    let seen = upstream.requests();
    let first = seen.first().expect("the upstream never saw a request");
    assert_eq!(
        (first.command, first.atyp, first.host.as_str(), first.port),
        (1, 3, "target.test", 8080),
        "the server resolves the name: rurge never looked it up"
    );
    assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
    drop(tunnel);
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    let record = &log.recent(10)[0];
    assert_eq!(record.policy, ["T"]);
    assert!(record.error.is_none(), "{:?}", record.error);
}

#[tokio::test]
async fn a_plain_request_is_tunnelled_through_trojan_over_websocket() {
    // only an HTTP proxy takes a plain request in absolute form; everything
    // else gets a tunnel
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (upstream, params) = trojan_upstream(true, origin_addr(&origin)).await;
    let h = harness(Profile {
        proxies: &format!(
            "T = trojan, 127.0.0.1, {}, {params}, ws=true, ws-path=/tunnel, ws-headers=Host:edge.test",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,T",
        ..Profile::default()
    })
    .await;
    let response = plain_get(
        h.http(),
        "http://target.test:8080/hello",
        "target.test:8080",
    )
    .await;
    assert!(response.ends_with("hi there"), "{response}");
    let ws = upstream.ws_seen();
    let first = ws
        .first()
        .expect("the upstream never saw a websocket handshake");
    assert_eq!(
        (first.path.as_str(), first.header("host")),
        ("/tunnel", Some("edge.test"))
    );
    assert_eq!(upstream.requests()[0].host, "target.test");
    assert_eq!(origin.hits("/hello"), 1);
}

#[tokio::test]
async fn a_trojan_exit_is_reached_through_a_socks5_entry_by_name() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (exit, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let entry = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(exit.addr()),
        ..Socks5Script::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Entry = socks5, 127.0.0.1, {}\nExit = trojan, exit.example, 443, {params}, underlying-proxy=Entry",
            entry.addr().port()
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    assert!(
        get(&mut tunnel, "target.test", "/hello")
            .await
            .ends_with("hi there")
    );
    // the entry is asked for the exit's server by name; the exit for the target by name
    let entry_seen = entry.requests();
    let first = entry_seen.first().expect("the entry never saw a request");
    assert_eq!(
        (first.atyp, first.host.as_str(), first.port),
        (3, "exit.example", 443)
    );
    let exit_seen = exit.requests();
    let exit_first = exit_seen.first().expect("the exit never saw a request");
    assert_eq!(exit_first.host, "target.test");
    assert!(
        h.dns.queries().is_empty(),
        "nothing on this path is resolved locally"
    );
}

/// The mirror image: trojan is the ENTRY, and the hop above it runs its own
/// handshake on top of the entry's `LazyHead` (M2a acceptance item 4).
#[tokio::test]
async fn a_socks5_exit_is_reached_through_a_trojan_entry() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let exit = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let (entry, params) = trojan_upstream(false, exit.addr()).await;
    let h = harness(Profile {
        proxies: &format!(
            "Entry = trojan, 127.0.0.1, {}, {params}\nExit = socks5, exit.example, 1080, underlying-proxy=Entry",
            entry.addr().port()
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    assert!(
        get(&mut tunnel, "target.test", "/hello")
            .await
            .ends_with("hi there")
    );
    // the entry is asked for the exit's server by name; the exit for the target by name
    let entry_seen = entry.requests();
    let first = entry_seen.first().expect("the entry never saw a request");
    assert_eq!(
        (first.atyp, first.host.as_str(), first.port),
        (3, "exit.example", 1080)
    );
    let exit_seen = exit.requests();
    let exit_first = exit_seen.first().expect("the exit never saw a request");
    assert_eq!(
        (exit_first.atyp, exit_first.host.as_str(), exit_first.port),
        (3, "target.test", 8080)
    );
    assert!(
        h.dns.queries().is_empty(),
        "nothing on this path is resolved locally"
    );
}

#[tokio::test]
async fn an_unrelated_reload_keeps_an_anytls_pool_and_a_change_of_its_own_drops_it() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (upstream, params) = anytls_upstream(origin_addr(&origin)).await;
    let port = upstream.addr().port();
    let profile = |extra: &str, a_extra: &str| {
        format!("A = anytls, 127.0.0.1, {port}, {params}{a_extra}\n{extra}")
    };
    let h = harness(Profile {
        proxies: &profile("", ""),
        rules: "DOMAIN,target.test,A",
        ..Profile::default()
    })
    .await;
    let request = |h: &Harness| {
        let http = h.http();
        async move {
            let mut tunnel = connect_via_http(http, "target.test:8080").await;
            let response = get(&mut tunnel, "target.test", "/hello").await;
            assert!(response.ends_with("hi there"), "{response}");
        }
    };
    let log = h.engine.request_log();
    request(&h).await;
    // the record appears once the relay has dropped the stream: the session is pooled
    wait_until("the first session to finish", || log.recent(10).len() == 1).await;
    let before = outbound_now(&h, "A");

    // a reload that has nothing to do with A
    let next = Profile {
        proxies: &profile("Other = http, other.example, 8080", ""),
        rules: "DOMAIN,target.test,A",
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &next, h.engine.shared()).await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "A")),
        "A was rebuilt"
    );
    request(&h).await;
    wait_until("the second session to finish", || log.recent(10).len() == 2).await;
    assert_eq!(
        upstream.sessions(),
        1,
        "the idle session survived the reload"
    );

    // a reload that changes A itself
    let next = Profile {
        proxies: &profile("Other = http, other.example, 8080", ", reuse=false"),
        rules: "DOMAIN,target.test,A",
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &next, h.engine.shared()).await);
    assert!(!Arc::ptr_eq(&before, &outbound_now(&h, "A")), "A was kept");
    request(&h).await;
    assert_eq!(
        upstream.sessions(),
        2,
        "the new outbound dialled for itself"
    );
}

#[tokio::test]
async fn a_reused_outbound_resolves_through_the_new_generation() {
    let echo = rurge_proto::testing::echo_server().await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("S = socks5, proxy.test, {}", upstream.addr().port());
    let h = harness(Profile {
        proxies: &proxies,
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    h.dns.set("proxy.test", &["127.0.0.1"], &[], 60);
    let before = outbound_now(&h, "S");
    let other = MockDns::spawn().await;
    for name in ["proxy.test", "target.test"] {
        other.set(name, &["127.0.0.1"], &[], 60);
    }
    let next = Profile {
        proxies: &proxies,
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    }
    .text(other.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &next, h.engine.shared()).await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "S")),
        "only `dns-server` changed"
    );
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"through the reused outbound").await;
    let asked = |dns: &MockDns| dns.queries().iter().any(|(q, _)| q.name == "proxy.test");
    assert!(asked(&other) && !asked(&h.dns));
}
