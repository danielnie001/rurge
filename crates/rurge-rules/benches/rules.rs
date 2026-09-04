use criterion::{Criterion, criterion_group, criterion_main};
use rurge_config::HostName;
use rurge_config::config::{LoadOptions, from_text};
use rurge_config::rule::ResourceRef;
use rurge_config::session::SessionInfo;
use rurge_rules::domain_index::DomainIndexBuilder;
use rurge_rules::engine::NoResolve;
use rurge_rules::ip_index::IpIndexBuilder;
use rurge_rules::matcher::{NoGeo, SetLookup, SetRef};
use rurge_rules::set::{CompiledSet, SetHandle};
use rurge_rules::set_format::{ParsedSet, SetKind, SetLine};
use rurge_rules::{OutboundMode, RuleEngine};
use std::collections::HashMap;
use std::hint::black_box;
use std::path::Path;
use std::sync::Arc;

/// Deterministic pseudo-random domains: `<label>.<label>.<tld>`.
fn domains(n: usize, seed: u64) -> Vec<String> {
    let mut x = seed;
    let mut next = move || {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (x >> 33) as u32
    };
    let tlds = ["com", "net", "org", "io", "cn"];
    (0..n)
        .map(|_| {
            let a = next();
            let b = next();
            format!("h{a:x}.d{b:x}.{}", tlds[(a as usize) % tlds.len()])
        })
        .collect()
}

fn domain_index(c: &mut Criterion) {
    let names = domains(100_000, 1);
    let mut b = DomainIndexBuilder::new();
    for (i, d) in names.iter().enumerate() {
        b.add_suffix(d, i as u32);
    }
    let idx = b.build();
    let hit = format!("www.{}", names[50_000]);
    c.bench_function("domain_index_100k_hit", |bench| {
        bench.iter(|| idx.lookup(black_box(&hit)))
    });
    c.bench_function("domain_index_100k_miss", |bench| {
        bench.iter(|| idx.lookup(black_box("no.such.example")))
    });
}

fn ip_index(c: &mut Criterion) {
    let mut b = IpIndexBuilder::new();
    for i in 0..100_000u32 {
        let net: ipnet::Ipv4Net = format!(
            "{}.{}.{}.0/24",
            10 + (i >> 16) % 100,
            (i >> 8) & 255,
            i & 255
        )
        .parse()
        .unwrap();
        b.add_v4(net, i);
    }
    let idx = b.build();
    let hit: std::net::IpAddr = "10.1.2.3".parse().unwrap();
    let miss: std::net::IpAddr = "203.0.113.1".parse().unwrap();
    c.bench_function("ip_index_100k_hit", |bench| {
        bench.iter(|| idx.lookup(black_box(hit)))
    });
    c.bench_function("ip_index_100k_miss", |bench| {
        bench.iter(|| idx.lookup(black_box(miss)))
    });
}

struct Sets(HashMap<String, SetHandle>);
impl SetLookup for Sets {
    fn lookup(&self, r: &ResourceRef, kind: SetKind) -> SetRef {
        match r {
            ResourceRef::File(p) => {
                Arc::new(self.0[&p.file_name().unwrap().to_string_lossy().to_string()].clone())
            }
            _ => Arc::new(SetHandle::new(CompiledSet::empty("x", kind))),
        }
    }
}

fn engine(c: &mut Criterion) {
    let mut sets = HashMap::new();
    for (i, name) in ["s1.list", "s2.list", "s3.list"].iter().enumerate() {
        let parsed = ParsedSet {
            lines: domains(100_000, 10 + i as u64)
                .into_iter()
                .map(|d| SetLine::Domain {
                    name: d,
                    suffix: true,
                })
                .collect(),
            ..ParsedSet::default()
        };
        sets.insert(
            name.to_string(),
            SetHandle::new(CompiledSet::compile(
                name,
                SetKind::DomainSet,
                &parsed,
                &Sets(HashMap::new()),
                1,
            )),
        );
    }
    let mut text = String::from("[Proxy]\nP = direct\n[Rule]\n");
    for d in domains(1_000, 99) {
        text.push_str(&format!("DOMAIN-SUFFIX,{d},P\n"));
    }
    text.push_str(
        "DOMAIN-SET,s1.list,P\nDOMAIN-SET,s2.list,P\nDOMAIN-SET,s3.list,P\nFINAL,DIRECT\n",
    );
    let cfg = from_text(&text, Path::new("bench.conf"), &LoadOptions::for_tests()).config;
    let engine = RuleEngine::build(&cfg, &Sets(sets), Arc::new(NoGeo)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let miss = SessionInfo::tcp(HostName::parse("no.such.example"), 443);
    c.bench_function("evaluate_1000_rules_3x100k_sets_miss", |bench| {
        bench
            .iter(|| rt.block_on(engine.evaluate(black_box(&miss), OutboundMode::Rule, &NoResolve)))
    });
}

criterion_group!(benches, domain_index, ip_index, engine);
criterion_main!(benches);
