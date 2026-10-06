//! rurge's DNS over QUIC (`quic://`) and DNS over HTTP/3 (`h3://`) upstreams
//! against two references (phase 2 M7 design 7.3, 7.4), on every platform:
//! AdGuard dnsproxy v0.86.0, and CoreDNS v1.14.7, the strict one — a message
//! ID other than 0 or an edns-tcp-keepalive option closes its DoQ
//! connection, and its DoH3 server answers on `/dns-query` alone, the path an
//! `h3://` URL without one goes to. Each listens on loopback UDP ports with a
//! certificate for 127.0.0.1 and answers `DNS_NAME` (A and AAAA) by itself.
//! The resolver verifies a server's certificate against the shared roots,
//! which no test CA can join: every profile sets
//! `encrypted-dns-skip-cert-verification`.
//!
//! Each server is asked through two resolvers of one profile: one that
//! dials directly, as `rurge run` does, and one whose QUIC paths are the UDP
//! flows of a connector that counts them, as with
//! `encrypted-dns-follow-outbound-mode` — its two lookups, the second past
//! the cache, ask four questions on one connection.

mod common;

use common::*;
use rurge_dns::system::StaticSystemDns;
use rurge_dns::{LookupOpts, Resolver, ResolverConfig, ResolverDeps, Source};
use rurge_interop::coredns::{CoreDns, coredns_or_skip};
use rurge_interop::dnsproxy::{Dnsproxy, dnsproxy_or_skip};
use rurge_interop::{DNS_NAME, DNS_V4, DNS_V6};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedDatagram, BoxedStream, Connector, DirectConnector};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions};
use rurge_rules::SetRegistry;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// The system's direct connector, offering QUIC no socket of its own: a
/// connection's path is then a UDP flow of `connect_udp`, which counts them.
struct Counted {
    direct: DirectConnector,
    flows: AtomicUsize,
}

impl Counted {
    fn new() -> Counted {
        Counted {
            direct: DirectConnector::new(Arc::new(SystemResolve)),
            flows: AtomicUsize::new(0),
        }
    }

    fn flows(&self) -> usize {
        self.flows.load(Ordering::SeqCst)
    }
}

impl Connector for Counted {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        self.direct.connect(target, opts)
    }

    fn connect_udp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
        self.flows.fetch_add(1, Ordering::SeqCst);
        self.direct.connect_udp(target, opts)
    }
}

/// A certificate for 127.0.0.1 and its key, as PEM files in `dir`.
fn certificate(dir: &Path) -> TlsFiles {
    leaf_files(&TlsFixture::new(&["127.0.0.1"]), dir)
}

/// A resolver whose profile (in `dir`) has `url` for its one server —
/// certificates unverified, A and AAAA asked —, reaching it through
/// `connector`.
fn resolver(dir: &Path, url: &str, connector: Arc<dyn Connector>) -> Arc<Resolver> {
    let profile = format!(
        "[General]\nencrypted-dns-server = {url}\nencrypted-dns-skip-cert-verification = true\nipv6 = true\n[Rule]\nFINAL,DIRECT\n"
    );
    let loaded = from_text(&profile, &dir.join("dns.conf"), &LoadOptions::for_tests());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    let cfg = loaded.config;
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
    let system = StaticSystemDns {
        has_ipv6: true,
        ..StaticSystemDns::default()
    };
    let (resolver, diagnostics) = Resolver::new(
        ResolverConfig::from_config(&cfg),
        ResolverDeps {
            connector,
            sets,
            system: Arc::new(system),
            resources,
        },
    );
    assert!(
        diagnostics.is_empty(),
        "{:?}",
        diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    resolver
}

/// `DNS_NAME` through `resolver`: its A and AAAA records, from the server
/// called `name`. Bounded like `roundtrip`.
async fn resolves(resolver: &Resolver, opts: LookupOpts, name: &str, log: &dyn Fn() -> String) {
    let result = tokio::time::timeout(Duration::from_secs(10), resolver.lookup(DNS_NAME, opts))
        .await
        .unwrap_or_else(|_| panic!("no answer within the bound\n{}", log()))
        .unwrap_or_else(|e| panic!("{e}\n{}", log()));
    assert_eq!(
        (result.v4, result.v6),
        (vec![DNS_V4], vec![DNS_V6]),
        "{}",
        log()
    );
    assert_eq!(
        result.source,
        Source::Upstream(name.to_string()),
        "{}",
        log()
    );
}

/// `DNS_NAME` from the server at `url`, called `name`, through a resolver
/// that dials directly, then twice through one whose connector counts its
/// connections — the second time past the cache —: those four questions on
/// one connection. `log` is the server's.
async fn lookups(dir: &Path, url: &str, name: &str, log: &dyn Fn() -> String) {
    let direct: Arc<dyn Connector> = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
    resolves(
        &resolver(dir, url, direct),
        LookupOpts::default(),
        name,
        log,
    )
    .await;
    let counted = Arc::new(Counted::new());
    let carried = resolver(dir, url, counted.clone());
    resolves(&carried, LookupOpts::default(), name, log).await;
    let past_the_cache = LookupOpts {
        bypass_cache: true,
        ..LookupOpts::default()
    };
    resolves(&carried, past_the_cache, name, log).await;
    assert_eq!(counted.flows(), 1, "one connection\n{}", log());
}

#[tokio::test]
async fn doq_against_dnsproxy() {
    let Some(bin) = dnsproxy_or_skip("doq_against_dnsproxy") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = Dnsproxy::spawn(&bin, dir.path(), &certificate(dir.path()));
    let url = format!("quic://127.0.0.1:{}", server.quic_port());
    lookups(dir.path(), &url, &url, &|| server.log_text()).await;
}

/// No path in the URL: the requests go to `/dns-query`.
#[tokio::test]
async fn doh3_against_dnsproxy() {
    let Some(bin) = dnsproxy_or_skip("doh3_against_dnsproxy") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = Dnsproxy::spawn(&bin, dir.path(), &certificate(dir.path()));
    let url = format!("h3://127.0.0.1:{}", server.h3_port());
    let name = format!("{url}/dns-query");
    lookups(dir.path(), &url, &name, &|| server.log_text()).await;
}

/// CoreDNS answers no query with a message ID other than 0 or an
/// edns-tcp-keepalive option: it closes the connection instead.
#[tokio::test]
async fn doq_against_coredns() {
    let Some(bin) = coredns_or_skip("doq_against_coredns") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = CoreDns::spawn(&bin, dir.path(), &certificate(dir.path()));
    let url = format!("quic://127.0.0.1:{}", server.quic_port());
    lookups(dir.path(), &url, &url, &|| server.log_text()).await;
}

/// CoreDNS answers on `/dns-query` alone, where a URL without a path goes.
#[tokio::test]
async fn doh3_against_coredns() {
    let Some(bin) = coredns_or_skip("doh3_against_coredns") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = CoreDns::spawn(&bin, dir.path(), &certificate(dir.path()));
    let url = format!("h3://127.0.0.1:{}", server.h3_port());
    let name = format!("{url}/dns-query");
    lookups(dir.path(), &url, &name, &|| server.log_text()).await;
}
