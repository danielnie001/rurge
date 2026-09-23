//! Sessions that leave through a policy wrapped in Shadow TLS: the same
//! harness as `outbounds.rs`, with the camouflage site's CA as the engine's
//! trust anchors (the camouflage handshake always verifies certificates).

mod common;

use common::*;
use rurge_config::spec::ShadowTlsVersion;
use rurge_proto::testing::{Camouflage, FakeShadowTls, ShadowTlsScript};

const SITE: &str = "site.test";

/// A camouflage site and a Shadow TLS server (password `st-pw`) in front of `behind`.
async fn shadow_front(
    version: ShadowTlsVersion,
    names: &[&str],
    behind: SocketAddr,
) -> (Arc<TlsFixture>, Camouflage, FakeShadowTls) {
    let fixture = TlsFixture::new(names);
    let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 2).await;
    let front =
        FakeShadowTls::spawn(ShadowTlsScript::new(version, "st-pw", site.addr(), behind)).await;
    (fixture, site, front)
}

#[tokio::test]
async fn a_connect_leaves_through_trojan_wrapped_in_shadow_tls_v3() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (trojan, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let (fixture, _site, front) = shadow_front(ShadowTlsVersion::V3, &[SITE], trojan.addr()).await;
    let h = harness_trusting(
        Profile {
            proxies: &format!(
                "T = trojan, 127.0.0.1, {}, {params}, shadow-tls-password=st-pw, shadow-tls-version=3, shadow-tls-sni={SITE}",
                front.addr().port()
            ),
            rules: "DOMAIN,target.test,T",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let answer = get(&mut tunnel, "target.test", "/hello").await;
    assert!(answer.ends_with("hi there"), "{answer}");
    // the site's handshake came first and carried the configured name
    let seen = fixture.seen_at_least(1).await;
    assert_eq!(seen[0].sni.as_deref(), Some(SITE));
    assert!(front.sessions()[0].authenticated);
    // and the name went to the trojan server unresolved
    let request = trojan
        .requests()
        .first()
        .cloned()
        .expect("a trojan request");
    assert_eq!((request.host.as_str(), request.port), ("target.test", 8080));
    assert!(h.dns.queries().is_empty(), "{:?}", h.dns.queries());
}

#[tokio::test]
async fn an_http_upstream_behind_shadow_tls_v2_needs_no_sni() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let proxy = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(origin_addr(&origin)),
        ..HttpProxyScript::default()
    })
    .await;
    // no `shadow-tls-sni`: no SNI goes out, and the certificate is checked
    // against the policy's own server
    let (fixture, _site, front) =
        shadow_front(ShadowTlsVersion::V2, &["127.0.0.1"], proxy.addr()).await;
    let h = harness_trusting(
        Profile {
            proxies: &format!(
                "Up = http, 127.0.0.1, {}, shadow-tls-password=st-pw",
                front.addr().port()
            ),
            rules: "DOMAIN,target.test,Up",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let answer = get(&mut tunnel, "target.test", "/hello").await;
    assert!(answer.ends_with("hi there"), "{answer}");
    assert_eq!(fixture.seen_at_least(1).await[0].sni, None);
    assert!(front.sessions()[0].authenticated);
    // a plain request in absolute form takes the same road (the fake proxy
    // answers such a request itself)
    let answer = plain_get(
        h.http(),
        "http://target.test:8080/hello",
        "target.test:8080",
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let heads = proxy.heads();
    let forwarded = &heads.last().expect("a forwarded request").request_line;
    assert!(
        forwarded.starts_with("GET http://target.test:8080/hello "),
        "{forwarded}"
    );
    assert!(front.sessions()[1].authenticated);
}

#[tokio::test]
async fn a_wrong_shadow_tls_password_fails_the_session_with_a_text_that_says_so() {
    let origin = TestServer::spawn().await;
    let (trojan, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let (fixture, site, front) = shadow_front(ShadowTlsVersion::V3, &[SITE], trojan.addr()).await;
    let h = harness_trusting(
        Profile {
            proxies: &format!(
                "T = trojan, 127.0.0.1, {}, {params}, shadow-tls-password=an0ther, shadow-tls-version=3, shadow-tls-sni={SITE}",
                front.addr().port()
            ),
            rules: "DOMAIN,target.test,T",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    // `Connection: close`: rurge closes after the 502, so the read below ends
    s.write_all(
        b"CONNECT target.test:8080 HTTP/1.1\r\nHost: target.test:8080\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut answer))
        .await
        .expect("the proxy answers within the bound");
    let answer = String::from_utf8_lossy(&answer).into_owned();
    assert!(answer.starts_with("HTTP/1.1 502 "), "{answer}");
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    let error = log.recent(10)[0].error.clone().unwrap_or_default();
    assert_eq!(error, "shadow-tls: the server did not authenticate itself");
    assert!(!error.contains("an0ther"));
    // the server saw a stranger, and the site got a visitor that asked for a page
    assert!(!front.sessions()[0].authenticated);
    assert!(site.received().starts_with(b"GET / HTTP/1.1\r\n"));
    assert!(trojan.requests().is_empty());
}

#[tokio::test]
async fn shadow_tls_runs_through_an_underlying_proxy() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (trojan, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let (fixture, _site, front) = shadow_front(ShadowTlsVersion::V3, &[SITE], trojan.addr()).await;
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let h = harness_trusting(
        Profile {
            proxies: &format!(
                "Entry = socks5, 127.0.0.1, {}\nExit = trojan, 127.0.0.1, {}, {params}, shadow-tls-password=st-pw, shadow-tls-version=3, shadow-tls-sni={SITE}, underlying-proxy=Entry",
                entry.addr().port(),
                front.addr().port()
            ),
            rules: "DOMAIN,target.test,Exit",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let answer = get(&mut tunnel, "target.test", "/hello").await;
    assert!(answer.ends_with("hi there"), "{answer}");
    // the entry was asked for the Shadow TLS server, not for the target
    let asked = entry.requests().first().cloned().expect("a socks5 request");
    assert_eq!(asked.port, front.addr().port());
    assert!(front.sessions()[0].authenticated);
}

#[tokio::test]
async fn vmess_without_tls_and_anytls_run_inside_shadow_tls_too() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    // vmess without TLS: the protocol runs on the frames directly, and its
    // request head waits for the first payload — the late first frame that
    // v2 likes least
    let (vmess, vmess_params) = vmess_upstream(false, false, origin_addr(&origin)).await;
    let (fixture, _site, vmess_front) =
        shadow_front(ShadowTlsVersion::V2, &[SITE], vmess.addr()).await;
    // anytls: its own TLS inside, and a session that is reused
    let (anytls, anytls_params) = anytls_upstream(origin_addr(&origin)).await;
    let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 2).await;
    let anytls_front = FakeShadowTls::spawn(ShadowTlsScript::new(
        ShadowTlsVersion::V3,
        "st-pw",
        site.addr(),
        anytls.addr(),
    ))
    .await;
    let h = harness_trusting(
        Profile {
            proxies: &format!(
                "V = vmess, 127.0.0.1, {}, {vmess_params}, shadow-tls-password=st-pw, shadow-tls-sni={SITE}\nA = anytls, 127.0.0.1, {}, {anytls_params}, shadow-tls-password=st-pw, shadow-tls-version=3, shadow-tls-sni={SITE}",
                vmess_front.addr().port(),
                anytls_front.addr().port()
            ),
            rules: "DOMAIN,target.test,V\nDOMAIN,alt.test,A",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let answer = get(&mut tunnel, "target.test", "/hello").await;
    assert!(answer.ends_with("hi there"), "{answer}");
    assert!(vmess_front.sessions()[0].authenticated);
    assert_eq!(vmess.requests().len(), 1);
    // our half of the tunnel is still open: let the relay finish
    drop(tunnel);
    let log = h.engine.request_log();
    wait_until("the vmess session to finish", || log.recent(10).len() == 1).await;
    for round in 1..=2 {
        let mut tunnel = connect_via_http(h.http(), "alt.test:8080").await;
        let answer = get(&mut tunnel, "alt.test", "/hello").await;
        assert!(answer.ends_with("hi there"), "{answer}");
        drop(tunnel);
        // one record for the vmess session above, then one per round
        wait_until("the session to finish", || {
            log.recent(10).len() == 1 + round
        })
        .await;
    }
    // both requests went over one AnyTLS session, hence one Shadow TLS connection
    assert_eq!(anytls.sessions(), 1);
    assert_eq!(anytls_front.sessions().len(), 1);
}

#[tokio::test]
async fn the_layer_is_part_of_what_a_reload_compares_and_of_what_the_api_hides() {
    let origin = TestServer::spawn().await;
    let (trojan, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let (fixture, _site, front) = shadow_front(ShadowTlsVersion::V3, &[SITE], trojan.addr()).await;
    let line = |password: &str| {
        format!(
            "T = trojan, 127.0.0.1, {}, {params}, shadow-tls-password={password}, shadow-tls-version=3, shadow-tls-sni={SITE}",
            front.addr().port()
        )
    };
    let h = harness_trusting(
        Profile {
            proxies: &line("st-pw"),
            rules: "DOMAIN,target.test,T",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let detail = h.engine.policy_detail("T").expect("a configured policy");
    assert!(
        detail.contains("shadow-tls-password=***") && !detail.contains("st-pw"),
        "{detail}"
    );
    assert!(
        detail.contains(&format!("shadow-tls-sni={SITE}")),
        "{detail}"
    );
    let before = outbound_now(&h, "T");
    // an unrelated edit: the outbound stays
    let unrelated = Profile {
        proxies: &line("st-pw"),
        rules: "DOMAIN,target.test,T\nDOMAIN,alt.test,DIRECT",
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &unrelated, h.engine.shared()).await);
    assert!(Arc::ptr_eq(&before, &outbound_now(&h, "T")));
    // another Shadow TLS password: a new outbound
    let changed = Profile {
        proxies: &line("an0ther"),
        rules: "DOMAIN,target.test,T",
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &changed, h.engine.shared()).await);
    assert!(!Arc::ptr_eq(&before, &outbound_now(&h, "T")));
}
