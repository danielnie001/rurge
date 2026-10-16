//! The engine and the network it is on (phase 3 M1 design 5, 7.3): what a
//! change of network does to the generation in use, and what the network the
//! machine is on when a generation is built sets of its DNS.

mod common;
use common::*;
use rurge_config::general::DnsServer;
use rurge_config::{NetKind, NetworkFacts, Primary, WifiLink};
use rurge_dns::LookupOpts;
use rurge_dns::message::Qtype;
use rurge_engine::stack::{DnsOverride, build_stack_with};
use std::path::Path;

/// The machine on Wi-Fi, joined to `ssid`.
fn on_wifi(ssid: &str) -> NetworkFacts {
    NetworkFacts {
        primary: Some(Primary {
            interface: "wlan0".to_string(),
            kind: NetKind::Wifi,
            routers: Vec::new(),
        }),
        wifi: Some(WifiLink {
            interface: "wlan0".to_string(),
            ssid: Some(ssid.to_string()),
            bssid: None,
        }),
    }
}

/// The machine on a cable.
fn on_cable() -> NetworkFacts {
    NetworkFacts {
        primary: Some(Primary {
            interface: "eth0".to_string(),
            kind: NetKind::Wired,
            routers: Vec::new(),
        }),
        wifi: None,
    }
}

/// The name of a plain UDP upstream, as the resolver lists it.
fn udp(dns: &MockDns) -> String {
    format!("udp://{}", dns.addr())
}

/// A generation of `profile`, built while the machine is on `network`.
async fn generation_on(dir: &Path, profile: &str, network: NetworkFacts) -> Runtime {
    let shared = EngineShared::default();
    shared.network.store(network);
    runtime(dir, profile, shared).await
}

/// As `runtime`, on a machine that has IPv6: the resolver asks for AAAA
/// answers as well.
async fn runtime_with_ipv6(dir: &Path, profile: &str) -> Runtime {
    std::fs::write(dir.join("t.conf"), profile).unwrap();
    let loaded = from_text(profile, &dir.join("t.conf"), &LoadOptions::for_tests());
    assert!(!loaded.diagnostics.has_errors());
    let mut stack = stack_options(dir);
    stack.system = Arc::new(StaticSystemDns {
        has_ipv6: true,
        ..StaticSystemDns::default()
    });
    Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack,
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            shared: EngineShared::default(),
            request_log_size: 100,
        },
    )
    .await
    .unwrap()
}

/// One session to `target.test` through the engine's HTTP listener.
async fn visit(h: &Harness) {
    let mut tunnel = connect_via_http(h.http(), "target.test:80").await;
    let response = get(&mut tunnel, "target.test", "/hello").await;
    assert!(response.ends_with("hi there"), "{response}");
}

const NAMES: [&str; 5] = ["h0.test", "h1.test", "h2.test", "h3.test", "h4.test"];

/// A change of network is a new network for what the resolver kept of the old
/// one: its cache, and the conclusion that the servers never answer AAAA.
#[tokio::test]
async fn a_network_change_flushes_the_resolver() {
    let dns = MockDns::spawn().await;
    for name in NAMES.into_iter().chain(["fresh.test"]) {
        dns.set(name, &["10.0.0.1"], &["fd00::1"], 60);
    }
    // the server answers A and never answers AAAA
    dns.set_drop_qtype(Qtype::Aaaa, true);
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\ndns-server = {}\nipv6 = true\n[Rule]\nFINAL,DIRECT\n",
        dns.addr()
    );
    let engine = Engine::new(runtime_with_ipv6(dir.path(), &profile).await);
    let resolver = engine.resolver();

    // five lookups at once, each with an A answer and no AAAA answer: that is
    // five in a row, and AAAA is turned off
    let lookup = |name| resolver.lookup(name, LookupOpts::default());
    let (a, b, c, d, e) = tokio::join!(
        lookup(NAMES[0]),
        lookup(NAMES[1]),
        lookup(NAMES[2]),
        lookup(NAMES[3]),
        lookup(NAMES[4])
    );
    for answer in [a, b, c, d, e] {
        let answer = answer.unwrap();
        assert_eq!(answer.v4.len(), 1);
        assert!(answer.v6.is_empty());
    }
    assert_eq!(resolver.cache_snapshot().len(), 5);
    assert!(resolver.aaaa_suppressed());

    let cable = on_cable();
    engine.network_changed(cable.clone());
    assert_eq!(*engine.network(), cable);
    assert!(resolver.cache_snapshot().is_empty());
    assert!(!resolver.aaaa_suppressed());

    // AAAA is asked for again: of a name nothing is under way for, which the
    // server now answers in full
    dns.set_drop_qtype(Qtype::Aaaa, false);
    let answer = resolver
        .lookup("fresh.test", LookupOpts::default())
        .await
        .unwrap();
    assert_eq!(answer.v6.len(), 1);
    assert_eq!(dns.query_count("fresh.test", Qtype::Aaaa), 1);
}

/// What is flushed is the resolver of the generation in use, not the one the
/// engine began with.
#[tokio::test]
async fn a_network_change_flushes_the_resolver_of_the_generation_in_use() {
    let h = harness(Profile::default()).await;
    let first = h.engine.resolver();
    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    h.engine
        .swap_runtime(runtime(h.dir.path(), &text, h.engine.shared()).await);
    let second = h.engine.resolver();
    assert!(!Arc::ptr_eq(&first, &second));

    second
        .lookup("target.test", LookupOpts::default())
        .await
        .unwrap();
    assert_eq!(second.cache_snapshot().len(), 1);
    h.engine.network_changed(on_cable());
    assert!(second.cache_snapshot().is_empty());
}

/// A `subnet` group picks by the network the engine is told it is on, and the
/// override of one does not outlive a change of network (design 5, 6.2).
#[tokio::test]
async fn a_network_change_moves_the_subnet_group() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (a, b, proxies) = two_entries(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "Net = subnet, TYPE:WIFI = A, default = B",
        rules: "DOMAIN,target.test,Net",
        ..Profile::default()
    })
    .await;
    let seen = || (a.requests().len(), b.requests().len());

    // nothing is known of the network yet: the default
    visit(&h).await;
    assert_eq!(seen(), (0, 1));

    // the machine joins a Wi-Fi network: the next session leaves through A
    h.engine.network_changed(on_wifi("Home"));
    assert_eq!(*h.engine.network(), on_wifi("Home"));
    visit(&h).await;
    assert_eq!(seen(), (1, 1));

    // an override stands while the network does
    h.engine.select_group("Net", "B").await.unwrap();
    visit(&h).await;
    assert_eq!(seen(), (1, 2));

    // and goes when it changes, to another Wi-Fi network as well
    h.engine.network_changed(on_wifi("Cafe"));
    assert_eq!(h.engine.group_selection("Net").unwrap(), "A");
    visit(&h).await;
    assert_eq!(seen(), (2, 2));

    // a cable: the default again
    h.engine.network_changed(on_cable());
    visit(&h).await;
    assert_eq!(seen(), (2, 3));
}

/// A generation is built with the DNS of the line of `[SSID Setting]` the
/// network matches (design 7.3): the lists it gives replace the profile's.
#[tokio::test]
async fn the_ssid_setting_of_the_network_sets_the_dns_servers() {
    let global = MockDns::spawn().await;
    let home = MockDns::spawn().await;
    let wifi = MockDns::spawn().await;
    for dns in [&global, &home, &wifi] {
        dns.set("a.test", &["10.0.0.1"], &[], 60);
    }
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\ndns-server = {}\nipv6 = false\n[Rule]\nFINAL,DIRECT\n\
         [SSID Setting]\nSSID:Home dns-server = {}\nTYPE:WIFI dns-server = {}\n",
        global.addr(),
        home.addr(),
        wifi.addr()
    );
    let upstreams = |rt: &Runtime| rt.stack.resolver.primary_upstreams();

    // no line matches a network nobody has read: the profile's servers
    let rt = generation_on(dir.path(), &profile, NetworkFacts::default()).await;
    assert_eq!(upstreams(&rt), [udp(&global)]);
    // nor a cable
    let rt = generation_on(dir.path(), &profile, on_cable()).await;
    assert_eq!(upstreams(&rt), [udp(&global)]);
    // the first line the network matches, and no other
    let rt = generation_on(dir.path(), &profile, on_wifi("Home")).await;
    assert_eq!(upstreams(&rt), [udp(&home)]);
    let rt = generation_on(dir.path(), &profile, on_wifi("Cafe")).await;
    assert_eq!(upstreams(&rt), [udp(&wifi)]);
    // and the resolver asks them
    rt.stack
        .resolver
        .lookup("a.test", LookupOpts::default())
        .await
        .unwrap();
    assert_eq!(wifi.query_count("a.test", Qtype::A), 1);
    assert_eq!(
        (
            global.query_count("a.test", Qtype::A),
            home.query_count("a.test", Qtype::A)
        ),
        (0, 0)
    );

    // `encrypted-dns-server = off` takes the encrypted DNS of the profile
    // away: the plain servers are asked again
    let encrypted_port = closed_tcp_port();
    let profile = format!(
        "[General]\ndns-server = {}\nencrypted-dns-server = tcp://127.0.0.1:{encrypted_port}\n\
         ipv6 = false\n[Rule]\nFINAL,DIRECT\n[SSID Setting]\nTYPE:WIRED encrypted-dns-server = off\n",
        global.addr()
    );
    let rt = generation_on(dir.path(), &profile, NetworkFacts::default()).await;
    assert_eq!(
        upstreams(&rt),
        [format!("tcp://127.0.0.1:{encrypted_port}")]
    );
    let rt = generation_on(dir.path(), &profile, on_cable()).await;
    assert_eq!(upstreams(&rt), [udp(&global)]);
}

/// A generation knows what the network set of its DNS, for whoever asks
/// whether the network that is now would set something else: the same for
/// networks that the same line matches, and the profile's own — nothing set —
/// for a line that says nothing of DNS.
#[tokio::test]
async fn the_dns_override_of_a_generation_is_known() {
    let home = MockDns::spawn().await;
    let wifi = MockDns::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\ndns-server = 10.9.9.9\nipv6 = false\n[Rule]\nFINAL,DIRECT\n\
         [SSID Setting]\nSSID:Home dns-server = {}\nTYPE:WIFI dns-server = {}\n\
         TYPE:WIRED suspend = true\n",
        home.addr(),
        wifi.addr()
    );
    let override_on = |network| {
        let dir = dir.path();
        let profile = &profile;
        async move {
            generation_on(dir, profile, network)
                .await
                .dns_override()
                .clone()
        }
    };
    let servers = |dns: &MockDns| DnsOverride {
        servers: Some(vec![DnsServer::Udp(dns.addr())]),
        encrypted: None,
    };

    let at_home = override_on(on_wifi("Home")).await;
    assert_eq!(at_home, servers(&home));
    // the same network: the same override
    assert_eq!(override_on(on_wifi("Home")).await, at_home);
    // another network of another line: another
    let at_the_cafe = override_on(on_wifi("Cafe")).await;
    assert_eq!(at_the_cafe, servers(&wifi));
    assert_ne!(at_the_cafe, at_home);
    // another network of the same line: the same
    assert_eq!(override_on(on_wifi("Library")).await, at_the_cafe);
    // nothing set: a network no line matches, and one whose line sets no DNS
    assert_eq!(
        override_on(NetworkFacts::default()).await,
        DnsOverride::default()
    );
    assert_eq!(override_on(on_cable()).await, DnsOverride::default());
}

/// The options of a stack have the last word: `rurge dns lookup --server`
/// asks the server it is given, whatever the network sets.
#[tokio::test]
async fn the_hook_of_a_stack_is_applied_after_the_override() {
    let global = MockDns::spawn().await;
    let wifi = MockDns::spawn().await;
    let hooked = MockDns::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\ndns-server = {}\nipv6 = false\n[Rule]\nFINAL,DIRECT\n\
         [SSID Setting]\nTYPE:WIFI dns-server = {}\n",
        global.addr(),
        wifi.addr()
    );
    let loaded = from_text(
        &profile,
        &dir.path().join("t.conf"),
        &LoadOptions::for_tests(),
    );
    assert!(!loaded.diagnostics.has_errors());
    let mut opts = stack_options(dir.path());
    opts.dns_override = DnsOverride::for_network(&loaded.config, &on_wifi("Home"));

    let stack = build_stack_with(&loaded.config, &opts, |_| {})
        .await
        .unwrap();
    assert_eq!(stack.resolver.primary_upstreams(), [udp(&wifi)]);
    let stack = build_stack_with(&loaded.config, &opts, |resolver| {
        resolver.servers = vec![DnsServer::Udp(hooked.addr())];
    })
    .await
    .unwrap();
    assert_eq!(stack.resolver.primary_upstreams(), [udp(&hooked)]);
}
