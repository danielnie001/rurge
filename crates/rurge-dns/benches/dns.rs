//! NFR-01: a DNS cache hit completes in under 1 ms. Two numbers: the bare
//! cache lookup and a full `Resolver::lookup` that hits the cache.

use criterion::{Criterion, criterion_group, criterion_main};
use rurge_config::config::{LoadOptions, from_text};
use rurge_dns::cache::{CachedAddrs, DnsCache};
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_dns::{LookupOpts, Resolver, ResolverConfig, ResolverDeps};
use rurge_net::connector::{DirectConnector, SystemResolve};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions};
use rurge_rules::SetRegistry;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

fn cache_get(c: &mut Criterion) {
    let cache = DnsCache::new(2000);
    let names: Vec<String> = (0..2000u32).map(|i| format!("host{i}.example")).collect();
    for (i, name) in names.iter().enumerate() {
        cache.put(
            name,
            CachedAddrs {
                v4: vec![Ipv4Addr::new(10, 0, (i / 256) as u8, (i % 256) as u8)],
                v6: Vec::new(),
                ttl: Duration::from_secs(3600),
                v6_queried: false,
                source: "bench".to_string(),
            },
        );
    }
    let mut i = 0usize;
    c.bench_function("cache_get", |b| {
        b.iter(|| {
            i = (i + 1) % names.len();
            std::hint::black_box(cache.get(&names[i]))
        })
    });
}

fn resolver_cache_hit(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (resolver, _mock) = rt.block_on(async {
        let mock = MockDns::spawn().await;
        mock.set("bench.test", &["10.0.0.1"], &[], 3600);
        let text = format!(
            "[General]\ndns-server = {}\nipv6 = false\n[Proxy]\n[Rule]\nFINAL,DIRECT\n",
            mock.addr()
        );
        let cfg = from_text(&text, &dir.path().join("t.conf"), &LoadOptions::for_tests()).config;
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let client =
            Arc::new(HttpClient::new(connector.clone(), HttpClientConfig::default()).unwrap());
        let resources = ResourceManager::with_options(
            dir.path().to_path_buf(),
            client,
            ResourceOptions {
                offline: true,
                ..ResourceOptions::default()
            },
        );
        let (sets, _) = SetRegistry::build(&cfg, resources.clone(), dir.path());
        let (resolver, _) = Resolver::new(
            ResolverConfig::from_config(&cfg),
            ResolverDeps {
                connector,
                sets,
                system: Arc::new(StaticSystemDns::default()),
                resources,
            },
        );
        resolver
            .lookup("bench.test", LookupOpts::default())
            .await
            .unwrap();
        (resolver, mock)
    });
    c.bench_function("resolver_cache_hit", |b| {
        b.iter(|| {
            rt.block_on(resolver.lookup("bench.test", LookupOpts::default()))
                .unwrap()
        })
    });
}

criterion_group!(benches, cache_get, resolver_cache_hit);
criterion_main!(benches);
