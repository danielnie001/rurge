//! Connection establishment (M2 design §5.1). M2 ships `DirectConnector`; M3
//! injects a connector that runs the session pipeline behind the same trait.

use crate::BoxFuture;
use rurge_config::HostName;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}
pub type BoxedStream = Box<dyn AsyncStream>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: HostName,
    pub port: u16,
}

impl Target {
    pub fn new(host: HostName, port: u16) -> Target {
        Target { host, port }
    }
}

#[derive(Clone, Debug)]
pub struct ConnectOpts {
    pub timeout: Duration,
    pub prefer_v6: bool,
}

impl Default for ConnectOpts {
    fn default() -> Self {
        ConnectOpts {
            timeout: Duration::from_secs(10),
            prefer_v6: false,
        }
    }
}

pub trait Connector: Send + Sync {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>>;
}

pub trait Resolve: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>>;
}

/// The operating system resolver (`getaddrinfo` through tokio).
pub struct SystemResolve;

impl Resolve for SystemResolve {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            let addrs: Vec<IpAddr> = tokio::net::lookup_host((host, 0))
                .await?
                .map(|sa| sa.ip())
                .collect();
            if addrs.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no addresses for {host}"),
                ));
            }
            Ok(addrs)
        })
    }
}

/// Alternates address families, starting with the preferred one.
pub fn interleave(addrs: Vec<IpAddr>, prefer_v6: bool) -> Vec<IpAddr> {
    let (v6, v4): (Vec<IpAddr>, Vec<IpAddr>) = addrs.into_iter().partition(|a| a.is_ipv6());
    let (mut first, mut second) = if prefer_v6 {
        (v6.into_iter(), v4.into_iter())
    } else {
        (v4.into_iter(), v6.into_iter())
    };
    let mut out = Vec::new();
    loop {
        match (first.next(), second.next()) {
            (None, None) => break,
            (a, b) => {
                out.extend(a);
                out.extend(b);
            }
        }
    }
    out
}

fn per_attempt(total: Duration, attempts: usize) -> Duration {
    let share = total / u32::try_from(attempts.max(1)).unwrap_or(u32::MAX);
    share.max(Duration::from_secs(2)).min(total)
}

/// Plain TCP: resolves through `Resolve`, then tries each address in turn.
pub struct DirectConnector {
    resolver: Arc<dyn Resolve>,
}

impl DirectConnector {
    pub fn new(resolver: Arc<dyn Resolve>) -> DirectConnector {
        DirectConnector { resolver }
    }
}

impl Connector for DirectConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
            let addrs = match &target.host {
                HostName::Ip(ip) => vec![*ip],
                HostName::Domain(d) => self.resolver.resolve(d).await?,
            };
            let ordered = interleave(addrs, opts.prefer_v6);
            let per = per_attempt(opts.timeout, ordered.len());
            let mut last = io::Error::new(io::ErrorKind::NotFound, "no addresses");
            for ip in ordered {
                let addr = SocketAddr::new(ip, target.port);
                match tokio::time::timeout(per, TcpStream::connect(addr)).await {
                    Ok(Ok(stream)) => {
                        let _ = stream.set_nodelay(true);
                        return Ok(Box::new(stream) as BoxedStream);
                    }
                    Ok(Err(e)) => last = e,
                    Err(_) => {
                        last = io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("connect to {addr} timed out"),
                        )
                    }
                }
            }
            Err(last)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn interleave_alternates_families() {
        let addrs = vec![
            ip("1.1.1.1"),
            ip("2.2.2.2"),
            ip("::1"),
            ip("::2"),
            ip("::3"),
        ];
        assert_eq!(
            interleave(addrs.clone(), false),
            vec![
                ip("1.1.1.1"),
                ip("::1"),
                ip("2.2.2.2"),
                ip("::2"),
                ip("::3")
            ]
        );
        assert_eq!(
            interleave(addrs, true),
            vec![
                ip("::1"),
                ip("1.1.1.1"),
                ip("::2"),
                ip("2.2.2.2"),
                ip("::3")
            ]
        );
        assert!(interleave(vec![], false).is_empty());
    }

    #[test]
    fn per_attempt_shares_the_budget_with_a_floor() {
        assert_eq!(
            per_attempt(Duration::from_secs(10), 2),
            Duration::from_secs(5)
        );
        assert_eq!(
            per_attempt(Duration::from_secs(10), 100),
            Duration::from_secs(2)
        );
        assert_eq!(
            per_attempt(Duration::from_secs(1), 1),
            Duration::from_secs(1)
        );
    }

    struct Fixed(Vec<IpAddr>);
    impl Resolve for Fixed {
        fn resolve<'a>(&'a self, _: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(async move { Ok(self.0.clone()) })
        }
    }

    #[tokio::test]
    async fn connects_to_ip_and_domain_targets() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let _ = s.write_all(b"hi").await;
                });
            }
        });
        let c = DirectConnector::new(Arc::new(Fixed(vec![ip("127.0.0.1")])));
        for host in ["127.0.0.1", "example.test"] {
            let mut stream = c
                .connect(
                    &Target::new(HostName::parse(host), port),
                    &ConnectOpts::default(),
                )
                .await
                .unwrap();
            let mut buf = [0u8; 2];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"hi");
        }
    }

    #[tokio::test]
    async fn refused_connection_is_an_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let c = DirectConnector::new(Arc::new(SystemResolve));
        let err = c
            .connect(
                &Target::new(HostName::parse("127.0.0.1"), port),
                &ConnectOpts::default(),
            )
            .await
            .err()
            .expect("refused");
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn resolver_errors_propagate() {
        struct Failing;
        impl Resolve for Failing {
            fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
                Box::pin(async move {
                    Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("nx {host}"),
                    ))
                })
            }
        }
        let c = DirectConnector::new(Arc::new(Failing));
        let err = c
            .connect(
                &Target::new(HostName::parse("nx.test"), 80),
                &ConnectOpts::default(),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
