//! What the tests of this crate log, captured: credentials and payloads
//! must show in no line of it.

use std::io;
use std::sync::{Arc, Mutex};

/// Captures what is logged, formatted.
#[derive(Clone, Default)]
pub(crate) struct Log(Arc<Mutex<Vec<u8>>>);

impl Log {
    /// What this thread logs, at every level, while the guard and the
    /// second dispatcher live. While one dispatcher alone is registered,
    /// tracing-core asks only the thread that first reaches a call site
    /// whether it is wanted: a site another test reaches first would stay
    /// shut on this thread too. With a second one it asks every dispatcher.
    pub(crate) fn capture() -> (Log, tracing::subscriber::DefaultGuard, tracing::Dispatch) {
        let log = Log::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let logging = tracing::subscriber::set_default(subscriber);
        let second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        (log, logging, second)
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
