//! Bootstrap (design §7.2): the hostnames inside URL-type upstreams
//! (`tcp://` / `tls://` / `https://`) are resolved only through traditional
//! upstreams — the plain UDP servers, or the system's — with a small cache
//! (minimum TTL 60 s). `BootstrapConnector` wraps the injected `Connector` so
//! DoT / DoH connections dial the pre-resolved address while TLS still sees
//! the hostname.

use crate::cache::{CacheHit, CachedAddrs, DnsCache};
use crate::fanout::{FanoutOpts, resolve_name};
use crate::upstream::{UpstreamError, UpstreamRef};
use arc_swap::ArcSwap;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target, interleave};
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

pub const BOOTSTRAP_MIN_TTL: Duration = Duration::from_secs(60);

pub struct Bootstrap {
    upstreams: ArcSwap<Vec<UpstreamRef>>,
    cache: DnsCache,
    want_v6: bool,
    opts: FanoutOpts,
}

impl Bootstrap {
    pub fn new(upstreams: Vec<UpstreamRef>, want_v6: bool, opts: FanoutOpts) -> Arc<Bootstrap> {
        Arc::new(Bootstrap {
            upstreams: ArcSwap::from_pointee(upstreams),
            cache: DnsCache::new(64),
            want_v6,
            opts,
        })
    }

    pub fn set_upstreams(&self, upstreams: Vec<UpstreamRef>) {
        self.upstreams.store(Arc::new(upstreams));
    }

    pub fn upstream_names(&self) -> Vec<String> {
        self.upstreams
            .load()
            .iter()
            .map(|u| u.name().to_string())
            .collect()
    }

    pub fn flush(&self) {
        self.cache.flush();
    }

    pub async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, UpstreamError> {
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = bare.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        let name = bare.trim_end_matches('.').to_ascii_lowercase();
        if let Some(CacheHit::Fresh(a)) = self.cache.get(&name) {
            return Ok(to_list(&a.v4, &a.v6));
        }
        // `load_full` (an owned `Arc` clone), not `load` (a `Guard`): a `Guard` is a
        // borrowed fast-path handle that arc-swap's own docs say must not cross an
        // async yield point, and `resolve_name` below is awaited.
        let upstreams = self.upstreams.load_full();
        if upstreams.is_empty() {
            return Err(UpstreamError::Bootstrap(format!(
                "no traditional upstream to resolve `{host}`"
            )));
        }
        let answers = resolve_name(&upstreams, &name, self.want_v6, &self.opts)
            .await
            .map_err(|e| UpstreamError::Bootstrap(format!("{host}: {e}")))?;
        let min_ttl = answers
            .v4
            .iter()
            .map(|(_, t)| *t)
            .chain(answers.v6.iter().map(|(_, t)| *t))
            .min()
            .unwrap_or(0);
        let ttl = Duration::from_secs(u64::from(min_ttl)).max(BOOTSTRAP_MIN_TTL);
        let v4: Vec<_> = answers.v4.iter().map(|(ip, _)| *ip).collect();
        let v6: Vec<_> = answers.v6.iter().map(|(ip, _)| *ip).collect();
        self.cache.put(
            &name,
            CachedAddrs {
                v4: v4.clone(),
                v6: v6.clone(),
                ttl,
                source: answers.upstream,
            },
        );
        Ok(to_list(&v4, &v6))
    }
}

fn to_list(v4: &[std::net::Ipv4Addr], v6: &[std::net::Ipv6Addr]) -> Vec<IpAddr> {
    v4.iter()
        .map(|a| IpAddr::V4(*a))
        .chain(v6.iter().map(|a| IpAddr::V6(*a)))
        .collect()
}

pub struct BootstrapConnector {
    inner: Arc<dyn Connector>,
    bootstrap: Arc<Bootstrap>,
}

impl BootstrapConnector {
    pub fn new(inner: Arc<dyn Connector>, bootstrap: Arc<Bootstrap>) -> BootstrapConnector {
        BootstrapConnector { inner, bootstrap }
    }
}

impl Connector for BootstrapConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
            let host = match &target.host {
                HostName::Ip(_) => return self.inner.connect(target, opts).await,
                HostName::Domain(d) => d.clone(),
            };
            let ips = self
                .bootstrap
                .resolve(&host)
                .await
                .map_err(|e| io::Error::other(e.to_string()))?;
            let mut last =
                io::Error::new(io::ErrorKind::NotFound, format!("no addresses for {host}"));
            for ip in interleave(ips, opts.prefer_v6) {
                let t = Target::new(HostName::Ip(ip), target.port);
                match self.inner.connect(&t, opts).await {
                    Ok(stream) => return Ok(stream),
                    Err(e) => last = e,
                }
            }
            Err(last)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Qtype;
    use crate::testing::MockDns;
    use crate::upstream::udp::UdpUpstream;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn fast() -> FanoutOpts {
        FanoutOpts {
            resend: Duration::from_millis(100),
            attempts: 3,
        }
    }

    #[tokio::test]
    async fn resolves_through_traditional_upstreams_and_caches() {
        let s = MockDns::spawn().await;
        s.set("dns.example", &["127.0.0.1"], &[], 5);
        let b = Bootstrap::new(vec![Arc::new(UdpUpstream::new(s.addr()))], false, fast());
        assert_eq!(
            b.resolve("dns.example").await.unwrap(),
            vec!["127.0.0.1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(b.resolve("DNS.example.").await.unwrap().len(), 1);
        assert_eq!(
            s.query_count("dns.example", Qtype::A),
            1,
            "second lookup served from the bootstrap cache"
        );
        assert_eq!(
            b.resolve("10.0.0.1").await.unwrap(),
            vec!["10.0.0.1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            b.resolve("[::1]").await.unwrap(),
            vec!["::1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(b.upstream_names(), vec![format!("udp://{}", s.addr())]);
        b.flush();
        b.resolve("dns.example").await.unwrap();
        assert_eq!(s.query_count("dns.example", Qtype::A), 2);
    }

    #[tokio::test]
    async fn errors_without_upstreams_or_answers() {
        let b = Bootstrap::new(Vec::new(), false, fast());
        assert!(matches!(
            b.resolve("dns.example").await,
            Err(UpstreamError::Bootstrap(_))
        ));
        let s = MockDns::spawn().await;
        s.set_empty("dns.example");
        let b = Bootstrap::new(vec![Arc::new(UdpUpstream::new(s.addr()))], false, fast());
        let err = b.resolve("dns.example").await.unwrap_err();
        assert!(
            matches!(err, UpstreamError::Bootstrap(ref m) if m.contains("empty")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn connector_dials_the_resolved_address() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let _ = s.write_all(b"ok").await;
                });
            }
        });
        let dns = MockDns::spawn().await;
        dns.set("dot.example", &["127.0.0.1"], &[], 60);
        let bootstrap = Bootstrap::new(vec![Arc::new(UdpUpstream::new(dns.addr()))], false, fast());
        let connector = BootstrapConnector::new(
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            bootstrap,
        );
        let mut stream = connector
            .connect(
                &Target::new(HostName::parse("dot.example"), port),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        let mut buf = [0u8; 2];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ok");
        // IP targets bypass bootstrap entirely
        let mut direct = connector
            .connect(
                &Target::new(HostName::parse("127.0.0.1"), port),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        direct.read_exact(&mut buf).await.unwrap();
        assert_eq!(dns.query_count("dot.example", Qtype::A), 1);
        let err = connector
            .connect(
                &Target::new(HostName::parse("nx.example"), port),
                &ConnectOpts::default(),
            )
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("nx.example"));
    }
}
