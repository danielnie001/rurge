//! Connection establishment (M2 design §5.1). M2 ships `DirectConnector`; M3
//! injects a connector that runs the session pipeline behind the same trait.

use crate::BoxFuture;
use crate::socket::{Family, NoopSocketHook, SocketHook, SocketOpts, plan_addresses, race};
use rurge_config::HostName;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// For `DirectConnector`: one budget covering name resolution and the
    /// whole race across every address. `BootstrapConnector` (`rurge-dns`)
    /// applies it again to each resolved address in turn instead.
    pub timeout: Duration,
}

impl Default for ConnectOpts {
    fn default() -> Self {
        ConnectOpts {
            timeout: Duration::from_secs(10),
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

/// Plain TCP: resolves through `Resolve`, then races the addresses the
/// policy's `ip-version` allows (`crate::socket::race`).
pub struct DirectConnector {
    resolver: Arc<dyn Resolve>,
    opts: SocketOpts,
    hook: Arc<dyn SocketHook>,
    /// The `allow-other-interface` fallback is logged once per connector.
    fallback_logged: Arc<AtomicBool>,
}

impl DirectConnector {
    pub fn new(resolver: Arc<dyn Resolve>) -> DirectConnector {
        DirectConnector::with_opts(resolver, SocketOpts::default(), Arc::new(NoopSocketHook))
    }

    pub fn with_opts(
        resolver: Arc<dyn Resolve>,
        opts: SocketOpts,
        hook: Arc<dyn SocketHook>,
    ) -> DirectConnector {
        DirectConnector {
            resolver,
            opts,
            hook,
            fallback_logged: Arc::new(AtomicBool::new(false)),
        }
    }
}

async fn connect_one(
    addr: SocketAddr,
    opts: &SocketOpts,
    hook: &dyn SocketHook,
    fallback_logged: &AtomicBool,
) -> io::Result<TcpStream> {
    let family = Family::of(&addr.ip());
    let domain = match family {
        Family::V4 => socket2::Domain::IPV4,
        Family::V6 => socket2::Domain::IPV6,
    };
    let socket = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
    socket.set_nonblocking(true)?;
    if opts.tos != 0
        && let Err(e) = hook.set_tos(&socket, family, opts.tos)
    {
        tracing::debug!(error = %e, tos = opts.tos, "cannot set the IP TOS; connecting without it");
    }
    if let Some(interface) = &opts.interface
        && let Err(e) = hook.bind_interface(&socket, interface, family)
    {
        if !opts.allow_other_interface {
            return Err(io::Error::new(
                e.kind(),
                format!("cannot use interface {interface}: {e}"),
            ));
        }
        if !fallback_logged.swap(true, Ordering::Relaxed) {
            tracing::warn!(interface = %interface, error = %e, "interface unavailable; using the default one (allow-other-interface)");
        }
    }
    let std_stream: std::net::TcpStream = socket.into();
    let stream = tokio::net::TcpSocket::from_std_stream(std_stream)
        .connect(addr)
        .await?;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

impl Connector for DirectConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
            let attempt = async {
                let (primary, secondary) = match &target.host {
                    // `ip-version` only means something for a host name (manual)
                    HostName::Ip(ip) => (vec![*ip], Vec::new()),
                    HostName::Domain(d) => {
                        let addrs = self.resolver.resolve(d).await?;
                        let planned =
                            plan_addresses(addrs, self.opts.ip_version, self.opts.v6_first);
                        if planned.0.is_empty() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                format!("no usable address for {d} (ip-version)"),
                            ));
                        }
                        planned
                    }
                };
                let port = target.port;
                let with_port = |ips: Vec<IpAddr>| -> Vec<SocketAddr> {
                    ips.into_iter()
                        .map(|ip| SocketAddr::new(ip, port))
                        .collect()
                };
                let (socket_opts, hook, logged) = (
                    self.opts.clone(),
                    self.hook.clone(),
                    self.fallback_logged.clone(),
                );
                race(with_port(primary), with_port(secondary), move |addr| {
                    let (socket_opts, hook, logged) =
                        (socket_opts.clone(), hook.clone(), logged.clone());
                    async move { connect_one(addr, &socket_opts, hook.as_ref(), &logged).await }
                })
                .await
            };
            match tokio::time::timeout(opts.timeout, attempt).await {
                Ok(Ok(stream)) => Ok(Box::new(stream) as BoxedStream),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("connect to {}:{} timed out", target.host, target.port),
                )),
            }
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

    use crate::socket::{Family, SocketHook, SocketOpts};
    use rurge_config::spec::IpVersion;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingHook {
        calls: Mutex<Vec<String>>,
        refuse_interface: bool,
    }

    impl SocketHook for RecordingHook {
        fn bind_interface(
            &self,
            _: &socket2::Socket,
            interface: &str,
            family: Family,
        ) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("bind {interface} {family:?}"));
            if self.refuse_interface {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no such interface"));
            }
            Ok(())
        }
        fn set_tos(&self, _: &socket2::Socket, family: Family, tos: u8) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("tos {tos:#04x} {family:?}"));
            Ok(())
        }
    }

    async fn listener() -> (tokio::net::TcpListener, u16) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        (l, port)
    }

    #[tokio::test]
    async fn the_hook_sees_the_tos_and_the_interface() {
        let (_l, port) = listener().await;
        let hook = Arc::new(RecordingHook::default());
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("test0".into()),
                tos: 0x10,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        connector
            .connect(
                &Target::new(HostName::parse("127.0.0.1"), port),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            *hook.calls.lock().unwrap(),
            ["tos 0x10 V4", "bind test0 V4"]
        );
    }

    #[tokio::test]
    async fn an_unusable_interface_fails_unless_other_interfaces_are_allowed() {
        let (_l, port) = listener().await;
        let target = Target::new(HostName::parse("127.0.0.1"), port);
        let hook = || {
            Arc::new(RecordingHook {
                refuse_interface: true,
                ..RecordingHook::default()
            })
        };
        let strict = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("gone0".into()),
                ..SocketOpts::default()
            },
            hook(),
        );
        let err = strict
            .connect(&target, &ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "cannot use interface gone0: no such interface"
        );
        let lenient = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("gone0".into()),
                allow_other_interface: true,
                ..SocketOpts::default()
            },
            hook(),
        );
        lenient
            .connect(&target, &ConnectOpts::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ip_version_filters_resolved_addresses_but_not_literals() {
        let (_l, port) = listener().await;
        let both = || {
            Arc::new(Fixed(vec![
                "::1".parse().unwrap(),
                "127.0.0.1".parse().unwrap(),
            ]))
        };
        let with = |version| {
            DirectConnector::with_opts(
                both(),
                SocketOpts {
                    ip_version: version,
                    ..SocketOpts::default()
                },
                Arc::new(crate::socket::NoopSocketHook),
            )
        };
        let name = Target::new(HostName::parse("both.test"), port);
        with(IpVersion::V4Only)
            .connect(&name, &ConnectOpts::default())
            .await
            .unwrap();
        // only 127.0.0.1 listens, so v6-only cannot connect
        assert!(
            with(IpVersion::V6Only)
                .connect(&name, &ConnectOpts::default())
                .await
                .is_err()
        );
        // an IP literal is used as it is
        let literal = Target::new(HostName::parse("127.0.0.1"), port);
        with(IpVersion::V6Only)
            .connect(&literal, &ConnectOpts::default())
            .await
            .unwrap();
        // nothing left after filtering
        let v4_only_name = DirectConnector::with_opts(
            Arc::new(Fixed(vec!["::1".parse().unwrap()])),
            SocketOpts {
                ip_version: IpVersion::V4Only,
                ..SocketOpts::default()
            },
            Arc::new(crate::socket::NoopSocketHook),
        );
        let err = v4_only_name
            .connect(&name, &ConnectOpts::default())
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            err.to_string(),
            "no usable address for both.test (ip-version)"
        );
    }

    #[tokio::test]
    async fn the_timeout_covers_name_resolution() {
        struct Stuck;
        impl Resolve for Stuck {
            fn resolve<'a>(&'a self, _: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
                Box::pin(std::future::pending())
            }
        }
        let connector = DirectConnector::new(Arc::new(Stuck));
        let err = connector
            .connect(
                &Target::new(HostName::parse("slow.test"), 80),
                &ConnectOpts {
                    timeout: Duration::from_millis(50),
                },
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(err.to_string(), "connect to slow.test:80 timed out");
    }
}
