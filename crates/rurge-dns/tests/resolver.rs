//! End to end: profile text → `Resolver` → in-process upstreams over every
//! transport (UDP, tcp://, tls://, https://), the manual's retry and
//! partial-result timing with the default 1 s / 5 attempts, `[Host]`
//! `server:` URLs bootstrapped by traditional upstreams, and set keys.

use rurge_config::config::{LoadOptions, from_text};
use rurge_dns::message::{Qtype, Question, Rcode, build_response};
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_dns::{HostKind, LookupOpts, Resolver, ResolverConfig, ResolverDeps, Source};
use rurge_net::connector::{DirectConnector, SystemResolve};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions};
use rurge_net::testing::TestServer;
use rurge_rules::SetRegistry;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn profile(general: &str, hosts: &str) -> String {
    format!("[General]\n{general}\n[Proxy]\n[Host]\n{hosts}\n[Rule]\nFINAL,DIRECT\n")
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// Builds a resolver whose profile lives in `dir` (local set files go there too).
fn resolver_in(dir: &Path, profile_text: &str, system: StaticSystemDns) -> Arc<Resolver> {
    let loaded = from_text(profile_text, &dir.join("t.conf"), &LoadOptions::for_tests());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.code)
            .collect::<Vec<_>>()
    );
    let cfg = loaded.config;
    let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
    let client = Arc::new(HttpClient::new(connector.clone(), HttpClientConfig::default()).unwrap());
    let resources = ResourceManager::with_options(
        dir.to_path_buf(),
        client,
        ResourceOptions {
            offline: true,
            ..ResourceOptions::default()
        },
    );
    let (sets, _) = SetRegistry::build(&cfg, resources.clone(), dir);
    let (resolver, diags) = Resolver::new(
        ResolverConfig::from_config(&cfg),
        ResolverDeps {
            connector,
            sets,
            system: Arc::new(system),
            resources,
        },
    );
    assert!(
        diags.is_empty(),
        "{:?}",
        diags.iter().map(|d| d.code).collect::<Vec<_>>()
    );
    resolver
}

fn resolver(profile_text: &str, system: StaticSystemDns) -> (tempfile::TempDir, Arc<Resolver>) {
    let dir = tempfile::tempdir().unwrap();
    let r = resolver_in(dir.path(), profile_text, system);
    (dir, r)
}

async fn lookup(r: &Resolver, name: &str) -> rurge_dns::DnsResult {
    r.lookup(name, LookupOpts::default()).await.unwrap()
}

#[tokio::test]
async fn every_transport_resolves() {
    // UDP
    let udp = MockDns::spawn().await;
    udp.set("a.test", &["10.0.0.1"], &[], 60);
    let (_d, r) = resolver(
        &profile(&format!("dns-server = {}\nipv6 = false", udp.addr()), ""),
        StaticSystemDns::default(),
    );
    let a = lookup(&r, "a.test").await;
    assert_eq!(a.addrs(), vec![ip("10.0.0.1")]);
    assert_eq!(a.source, Source::Upstream(format!("udp://{}", udp.addr())));

    // tcp://
    let tcp = MockDns::spawn().await;
    tcp.set("a.test", &["10.0.0.2"], &[], 60);
    let (_d, r) = resolver(
        &profile(
            &format!("encrypted-dns-server = tcp://{}\nipv6 = false", tcp.addr()),
            "",
        ),
        StaticSystemDns::default(),
    );
    let a = lookup(&r, "a.test").await;
    assert_eq!(a.addrs(), vec![ip("10.0.0.2")]);
    assert_eq!(a.source, Source::Upstream(format!("tcp://{}", tcp.addr())));
    assert_eq!(tcp.queries()[0].1, "tcp");

    // tls://
    let dot = MockDns::spawn_tls().await;
    dot.set("a.test", &["10.0.0.3"], &[], 60);
    let (_d, r) = resolver(
        &profile(
            &format!(
                "encrypted-dns-server = tls://{}\nencrypted-dns-skip-cert-verification = true\nipv6 = false",
                dot.addr()
            ),
            "",
        ),
        StaticSystemDns::default(),
    );
    let a = lookup(&r, "a.test").await;
    assert_eq!(a.addrs(), vec![ip("10.0.0.3")]);
    assert_eq!(a.source, Source::Upstream(format!("tls://{}", dot.addr())));

    // https:// (static body: DoH sends ID 0 and restores the caller's ID)
    let doh = TestServer::spawn_tls().await;
    let q = Question {
        name: "a.test".to_string(),
        qtype: Qtype::A,
    };
    doh.set(
        "/dns-query",
        build_response(0, &q, Rcode::NoError, &[(ip("10.0.0.4"), 60)], false).unwrap(),
    );
    doh.set_header("/dns-query", "content-type", "application/dns-message");
    let url = doh.url("/dns-query");
    let (_d, r) = resolver(
        &profile(
            &format!(
                "encrypted-dns-server = {url}\nencrypted-dns-skip-cert-verification = true\nipv6 = false"
            ),
            "",
        ),
        StaticSystemDns::default(),
    );
    let a = lookup(&r, "a.test").await;
    assert_eq!(a.addrs(), vec![ip("10.0.0.4")]);
    assert_eq!(a.source, Source::Upstream(url.to_string()));
    assert_eq!(doh.hits("/dns-query"), 1);
}

#[tokio::test]
async fn host_server_url_is_bootstrapped_by_traditional_upstreams() {
    let udp = MockDns::spawn().await;
    udp.set("dns.corp.test", &["127.0.0.1"], &[], 60);
    udp.set("corp.test", &["10.0.0.99"], &[], 60);
    let corp = MockDns::spawn().await;
    corp.set("corp.test", &["10.10.0.1"], &[], 60);
    let (_d, r) = resolver(
        &profile(
            &format!("dns-server = {}\nipv6 = false", udp.addr()),
            &format!(
                "corp.test = server:tcp://dns.corp.test:{}",
                corp.addr().port()
            ),
        ),
        StaticSystemDns::default(),
    );
    let c = lookup(&r, "corp.test").await;
    assert_eq!(c.addrs(), vec![ip("10.10.0.1")]);
    assert_eq!(c.source, Source::Host(HostKind::Server));
    assert_eq!(
        udp.query_count("dns.corp.test", Qtype::A),
        1,
        "bootstrap went to the UDP upstream"
    );
    assert_eq!(
        udp.query_count("corp.test", Qtype::A),
        0,
        "the name itself never went to the UDP upstream"
    );
    assert_eq!(corp.queries()[0].1, "tcp");
}

#[tokio::test]
async fn resend_after_one_second_and_first_answer_wins() {
    let a = MockDns::spawn().await;
    a.set("r.test", &["10.0.0.1"], &[], 60);
    a.set_drop_first(1);
    let b = MockDns::spawn().await;
    b.set("r.test", &["10.0.0.2"], &[], 60);
    b.set_delay(Duration::from_millis(1600));
    let (_d, r) = resolver(
        &profile(
            &format!("dns-server = {}, {}\nipv6 = false", a.addr(), b.addr()),
            "",
        ),
        StaticSystemDns::default(),
    );
    let started = Instant::now();
    let res = lookup(&r, "r.test").await;
    let elapsed = started.elapsed();
    assert_eq!(
        res.addrs(),
        vec![ip("10.0.0.1")],
        "a's resent query wins before b's slow answer"
    );
    assert_eq!(res.source, Source::Upstream(format!("udp://{}", a.addr())));
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_millis(1500),
        "{elapsed:?}"
    );
    assert_eq!(
        a.query_count("r.test", Qtype::A),
        2,
        "one resend after the 1 s timer"
    );
    assert!(b.query_count("r.test", Qtype::A) >= 1);
}

#[tokio::test]
async fn partial_result_when_aaaa_lags() {
    let m = MockDns::spawn().await;
    m.set("p.test", &["10.0.0.1"], &["fd00::1"], 60);
    m.set_drop_qtype(Qtype::Aaaa, true);
    let (_d, r) = resolver(
        &profile(&format!("dns-server = {}\nipv6 = true", m.addr()), ""),
        StaticSystemDns {
            has_ipv6: true,
            ..StaticSystemDns::default()
        },
    );
    let started = Instant::now();
    let res = lookup(&r, "p.test").await;
    let elapsed = started.elapsed();
    assert_eq!(
        res.v4,
        vec!["10.0.0.1".parse::<std::net::Ipv4Addr>().unwrap()]
    );
    assert!(res.v6.is_empty());
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_millis(1500),
        "partial result at the first resend tick: {elapsed:?}"
    );
    m.set_drop_qtype(Qtype::Aaaa, false);
    let full = r
        .lookup(
            "p.test",
            LookupOpts {
                bypass_cache: true,
                want_v6: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(full.v6.len(), 1);
}

#[tokio::test]
async fn domain_set_and_rule_set_host_keys() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("d.txt"), "a.set.test\n.suffix.test\n").unwrap();
    std::fs::write(dir.path().join("r.list"), "DOMAIN,r.set.test\n").unwrap();
    let m = MockDns::spawn().await;
    m.set("other.test", &["10.0.0.9"], &[], 60);
    let r = resolver_in(
        dir.path(),
        &profile(
            &format!("dns-server = {}\nipv6 = false", m.addr()),
            "DOMAIN-SET:d.txt = 10.5.0.1\nRULE-SET:r.list = 10.5.0.2\n",
        ),
        StaticSystemDns::default(),
    );
    // local set files load asynchronously; give the registry a moment
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let a = lookup(&r, "a.set.test").await;
        if a.source == Source::Host(HostKind::Ip) {
            assert_eq!(a.addrs(), vec![ip("10.5.0.1")]);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "DOMAIN-SET host key never matched: {a:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        lookup(&r, "x.suffix.test").await.addrs(),
        vec![ip("10.5.0.1")]
    );
    assert_eq!(lookup(&r, "r.set.test").await.addrs(), vec![ip("10.5.0.2")]);
    let other = lookup(&r, "other.test").await;
    assert_eq!(other.addrs(), vec![ip("10.0.0.9")]);
    assert!(matches!(other.source, Source::Upstream(_)));
}
