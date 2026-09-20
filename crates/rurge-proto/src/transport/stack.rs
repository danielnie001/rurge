//! The fixed ladder between a connector and a protocol's own handshake
//! (phase 2 design §5.4): connect → tls → ws. Shadow TLS joins in M2c.

use crate::OutboundError;
use crate::transport::tls::TlsClient;
use crate::transport::ws::WsClient;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use std::sync::Arc;

pub struct Stack {
    connector: Arc<dyn Connector>,
    server: Target,
    tls: Option<TlsClient>,
    ws: Option<WsClient>,
}

impl Stack {
    pub fn new(
        connector: Arc<dyn Connector>,
        server: Target,
        tls: Option<TlsClient>,
        ws: Option<WsClient>,
    ) -> Stack {
        Stack {
            connector,
            server,
            tls,
            ws,
        }
    }

    pub fn server(&self) -> &Target {
        &self.server
    }

    /// No timeout of its own: the caller wraps the ladder and its own
    /// handshake into one budget.
    pub async fn open(&self, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        let mut stream = self.connector.connect(&self.server, opts).await?;
        if let Some(tls) = &self.tls {
            stream = tls.wrap(stream).await.map_err(OutboundError::tls)?;
        }
        if let Some(ws) = &self.ws {
            stream = ws.wrap(stream).await?;
        }
        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeWs, TlsFixture, WsScript};
    use rurge_config::HostName;
    use rurge_config::spec::{TlsOpts, WsOpts};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn tls_then_websocket_and_no_alpn_unless_asked_for() {
        // bounded so a stall (e.g. a write-through regression) fails the
        // test instead of hanging it
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let fixture = TlsFixture::new(&["127.0.0.1"]);
            let fake = FakeWs::spawn_tls(WsScript::default(), fixture.clone()).await;
            let server = Target::new(HostName::Ip(fake.addr().ip()), fake.addr().port());
            let tls = TlsClient::build(
                &TlsOpts::default(),
                &server.host,
                &[],
                None,
                fixture.roots(),
            )
            .unwrap();
            let ws = WsClient::new(
                &WsOpts {
                    path: "/tunnel".into(),
                    headers: Vec::new(),
                },
                &server,
                true,
            )
            .unwrap();
            let stack = Stack::new(
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
                server,
                Some(tls),
                Some(ws),
            );
            let mut stream = stack.open(&ConnectOpts::default()).await.unwrap();
            // no explicit `flush()`: `write_all` alone must deliver
            stream.write_all(b"through both layers").await.unwrap();
            let mut buf = [0u8; 19];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"through both layers");
            assert_eq!(fake.seen()[0].path, "/tunnel");
            // the fixture offers h2 first: an ALPN of ours would have picked it
            assert_eq!(fixture.seen()[0].alpn, None);
        })
        .await
        .expect("the round trip finished within the bound");
    }

    #[tokio::test]
    async fn each_layer_says_which_one_failed() {
        // a plain WebSocket server behind a TLS client: the TLS layer fails
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeWs::spawn(WsScript::default()).await;
        let server = Target::new(HostName::Ip(fake.addr().ip()), fake.addr().port());
        let tls = TlsClient::build(
            &TlsOpts::default(),
            &server.host,
            &[],
            None,
            fixture.roots(),
        )
        .unwrap();
        let stack = Stack::new(
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            server,
            Some(tls),
            None,
        );
        let err = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stack.open(&ConnectOpts::default()),
        )
        .await
        .expect("the peer answers or closes")
        .err()
        .expect("TLS against a plain server fails");
        assert!(matches!(err, OutboundError::Tls(_)), "{err}");
    }
}
