//! `DIRECT`: connect straight to the destination through a `Connector`.

use crate::outbound::{Outbound, OutboundError};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, DirectConnector, Resolve, Target};
use std::sync::Arc;

pub struct Direct {
    connector: Arc<dyn Connector>,
}

impl Direct {
    pub fn new(connector: Arc<dyn Connector>) -> Direct {
        Direct { connector }
    }

    /// Plain TCP, racing the resolved addresses, resolving through `resolver`.
    pub fn with_resolver(resolver: Arc<dyn Resolve>) -> Direct {
        Direct::new(Arc::new(DirectConnector::new(resolver)))
    }
}

impl Outbound for Direct {
    fn name(&self) -> &str {
        "DIRECT"
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.connector.connect(target, opts)).await {
                Ok(Ok(stream)) => Ok(stream),
                Ok(Err(e)) => Err(OutboundError::from(e)),
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use std::io;
    use std::net::IpAddr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    struct Loopback;

    impl Resolve for Loopback {
        fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(async move {
                if host == "echo.test" {
                    Ok(vec!["127.0.0.1".parse().unwrap()])
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("no addresses for {host}"),
                    ))
                }
            })
        }
    }

    async fn echo_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 64];
                    if let Ok(n) = s.read(&mut buf).await {
                        let _ = s.write_all(&buf[..n]).await;
                    }
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn connects_by_ip_literal_and_by_resolved_domain() {
        let port = echo_server().await;
        let direct = Direct::with_resolver(Arc::new(Loopback));
        assert_eq!(direct.name(), "DIRECT");
        for host in ["127.0.0.1", "echo.test"] {
            let mut stream = direct
                .connect_tcp(
                    &Target::new(HostName::parse(host), port),
                    &ConnectOpts::default(),
                )
                .await
                .unwrap();
            stream.write_all(b"ping").await.unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
        }
    }

    #[tokio::test]
    async fn resolution_and_connect_failures_are_io_errors() {
        let direct = Direct::with_resolver(Arc::new(Loopback));
        let err = direct
            .connect_tcp(
                &Target::new(HostName::parse("nx.test"), 80),
                &ConnectOpts::default(),
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(err, OutboundError::Io(_)), "{err}");
        // a port nobody listens on
        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let err = direct
            .connect_tcp(
                &Target::new(HostName::parse("127.0.0.1"), port),
                &ConnectOpts {
                    timeout: Duration::from_secs(3),
                },
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(err, OutboundError::Io(_) | OutboundError::Timeout),
            "{err}"
        );
    }
}
