//! Test doubles shared by this crate's unit tests.

use crate::factory::{BuildError, OutboundFactory};
use rurge_config::spec::{CommonOpts, PolicySpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::{Outbound, OutboundError, OutboundRef};
use std::io;
use std::sync::{Arc, Mutex};

/// Records every target it is asked to reach; hands out one end of an
/// in-memory pipe, or refuses when `fail` is set.
#[derive(Default)]
pub(crate) struct RecordingConnector {
    pub log: Arc<Mutex<Vec<String>>>,
    pub fail: bool,
}

impl RecordingConnector {
    pub(crate) fn seen(&self) -> Vec<String> {
        self.log.lock().expect("log").clone()
    }
}

impl Connector for RecordingConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        _opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
            self.log
                .lock()
                .expect("log")
                .push(format!("dial {}:{}", target.host, target.port));
            if self.fail {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "refused by the test",
                ));
            }
            let (near, _far) = tokio::io::duplex(64);
            Ok(Box::new(near) as BoxedStream)
        })
    }
}

/// An outbound that behaves like a real proxy outbound: it reaches its own
/// server through its connector, and records the target it was asked to
/// carry (as a real proxy would forward it).
pub(crate) struct FakeOutbound {
    name: String,
    server: Target,
    connector: Arc<dyn Connector>,
    log: Arc<Mutex<Vec<String>>>,
}

impl Outbound for FakeOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            self.log
                .lock()
                .expect("log")
                .push(format!("{} -> {}:{}", self.name, target.host, target.port));
            self.connector
                .connect(&self.server, opts)
                .await
                .map_err(OutboundError::from)
        })
    }
}

/// Every direct connector it hands out is the same `RecordingConnector`, and
/// every outbound it builds writes into the same log.
pub(crate) struct FakeFactory {
    pub connector: Arc<RecordingConnector>,
    /// The policy whose build fails.
    pub broken: Option<&'static str>,
}

impl FakeFactory {
    pub(crate) fn new() -> FakeFactory {
        FakeFactory {
            connector: Arc::new(RecordingConnector::default()),
            broken: None,
        }
    }
}

impl OutboundFactory for FakeFactory {
    fn direct_connector(&self, _common: &CommonOpts) -> Arc<dyn Connector> {
        self.connector.clone()
    }

    fn build(
        &self,
        spec: &PolicySpec,
        connector: Arc<dyn Connector>,
    ) -> Result<OutboundRef, BuildError> {
        if self.broken == Some(spec.name.as_str()) {
            return Err(BuildError::new("boom"));
        }
        if matches!(spec.proto, rurge_config::spec::ProtoSpec::Direct) {
            return Ok(Arc::new(rurge_proto::Direct::new(connector)));
        }
        let server = Target::new(
            spec.server.clone().expect("a proxy policy has a server"),
            spec.port.expect("and a port"),
        );
        Ok(Arc::new(FakeOutbound {
            name: spec.name.clone(),
            server,
            connector,
            log: self.connector.log.clone(),
        }))
    }
}
