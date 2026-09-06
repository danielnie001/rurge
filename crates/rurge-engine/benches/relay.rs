use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use rurge_config::HostName;
use rurge_config::session::SessionInfo;
use rurge_engine::relay::pump;
use rurge_inbound::SessionHandle;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BYTES: usize = 16 * 1024 * 1024;
const CHUNK: usize = 64 * 1024;

fn relay_throughput(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let mut g = c.benchmark_group("relay");
    g.throughput(Throughput::Bytes(BYTES as u64));
    g.bench_function("pump_16MiB_over_duplex", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (mut client_a, client_b) = tokio::io::duplex(CHUNK);
                let (mut upstream_a, upstream_b) = tokio::io::duplex(CHUNK);
                let h = SessionHandle::new(1, SessionInfo::tcp(HostName::parse("bench.test"), 80));
                let relay = tokio::spawn(pump(
                    Box::new(client_b),
                    Box::new(upstream_b),
                    h,
                    Duration::from_secs(60),
                ));
                let writer = tokio::spawn(async move {
                    let chunk = vec![0u8; CHUNK];
                    let mut sent = 0;
                    while sent < BYTES {
                        client_a.write_all(&chunk).await.expect("write");
                        sent += chunk.len();
                    }
                    client_a.shutdown().await.expect("shutdown");
                });
                let mut buf = vec![0u8; CHUNK];
                let mut got = 0;
                while got < BYTES {
                    let n = upstream_a.read(&mut buf).await.expect("read");
                    if n == 0 {
                        break;
                    }
                    got += n;
                }
                drop(upstream_a);
                writer.await.expect("writer");
                relay.await.expect("relay");
            })
        });
    });
    g.finish();
}

criterion_group!(benches, relay_throughput);
criterion_main!(benches);
