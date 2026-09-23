//! The fixed ladder between a connector and a protocol's own handshake
//! (phase 2 design §5.4): connect → shadow-tls → tls → ws.

use crate::OutboundError;
use crate::transport::shadow_tls::ShadowTlsClient;
use crate::transport::tls::TlsClient;
use crate::transport::ws::WsClient;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use std::sync::Arc;

pub struct Stack {
    connector: Arc<dyn Connector>,
    server: Target,
    shadow_tls: Option<ShadowTlsClient>,
    tls: Option<TlsClient>,
    ws: Option<WsClient>,
}

impl Stack {
    /// The layers in the order they are passed through.
    pub fn new(
        connector: Arc<dyn Connector>,
        server: Target,
        shadow_tls: Option<ShadowTlsClient>,
        tls: Option<TlsClient>,
        ws: Option<WsClient>,
    ) -> Stack {
        Stack {
            connector,
            server,
            shadow_tls,
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
        if let Some(shadow_tls) = &self.shadow_tls {
            stream = shadow_tls.wrap(stream).await?;
        }
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
    use crate::testing::{
        Camouflage, FakeShadowTls, FakeWs, ShadowTlsScript, TlsFixture, WsScript,
    };
    use crate::transport::shadow_tls::ShadowTlsClient;
    use rurge_config::HostName;
    use rurge_config::spec::{Secret, ShadowTlsOpts, ShadowTlsVersion, TlsOpts, WsOpts};
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
                None,
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
    async fn shadow_tls_sits_below_tls_and_websocket() {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let fixture = TlsFixture::new(&["127.0.0.1", "site.test"]);
            let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 2).await;
            // behind the Shadow TLS server: TLS, and a WebSocket inside it
            let ws = FakeWs::spawn_tls(WsScript::default(), fixture.clone()).await;
            let front = FakeShadowTls::spawn(ShadowTlsScript::new(
                ShadowTlsVersion::V3,
                "pw",
                site.addr(),
                ws.addr(),
            ))
            .await;
            let server = Target::new(HostName::Ip(front.addr().ip()), front.addr().port());
            let shadow_tls = ShadowTlsClient::build(
                &ShadowTlsOpts {
                    password: Secret::from("pw"),
                    sni: Some("site.test".into()),
                    version: ShadowTlsVersion::V3,
                },
                &server.host,
                fixture.roots(),
            )
            .unwrap();
            let tls = TlsClient::build(
                &TlsOpts::default(),
                &server.host,
                &[],
                None,
                fixture.roots(),
            )
            .unwrap();
            let ws_client = WsClient::new(
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
                Some(shadow_tls),
                Some(tls),
                Some(ws_client),
            );
            let mut stream = stack.open(&ConnectOpts::default()).await.unwrap();
            stream.write_all(b"through three layers").await.unwrap();
            let mut buf = [0u8; 20];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"through three layers");
            // the site's handshake first, then the proxy's own TLS
            let seen = fixture.seen_at_least(2).await;
            assert_eq!(seen[0].sni.as_deref(), Some("site.test"));
            assert_eq!(seen[1].sni, None, "an IP literal: no SNI");
            assert_eq!(ws.seen()[0].path, "/tunnel");
            assert!(front.sessions()[0].authenticated);
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
            None,
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
        // and a Shadow TLS client in front of a server that is none
        let shadow_tls = ShadowTlsClient::build(
            &ShadowTlsOpts {
                password: Secret::from("pw"),
                sni: Some("site.test".into()),
                version: ShadowTlsVersion::V2,
            },
            &HostName::Ip(fake.addr().ip()),
            fixture.roots(),
        )
        .unwrap();
        let stack = Stack::new(
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            Target::new(HostName::Ip(fake.addr().ip()), fake.addr().port()),
            Some(shadow_tls),
            None,
            None,
        );
        let err = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stack.open(&ConnectOpts::default()),
        )
        .await
        .expect("the peer answers or closes")
        .err()
        .expect("a TLS handshake against a plain server fails");
        assert!(err.to_string().starts_with("shadow-tls: "), "{err}");
    }
}
