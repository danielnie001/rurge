//! Test doubles shared by this crate's unit tests.

use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
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
