//! What a test's thread logs, kept for the test to read. A test that reads the
//! log is a plain `#[test]`: everything the system half of the enhanced mode
//! logs for it is logged on the thread that installed the capture.

use std::io;
use std::sync::{Arc, Mutex};

/// Everything this thread logs while the capture lives, down to DEBUG, as the
/// default formatter writes it without time and target: one event to a line,
/// `LEVEL message field=value field=value ...`.
pub(crate) struct LogCapture {
    text: Arc<Mutex<Vec<u8>>>,
    _installed: tracing::subscriber::DefaultGuard,
    // While one dispatcher alone is registered, tracing-core asks only the
    // thread that first reaches a call site whether it is wanted: a site
    // another test reaches first would stay shut on this thread too. With a
    // second one it asks every dispatcher.
    _second: tracing::Dispatch,
}

struct Text(Arc<Mutex<Vec<u8>>>);

impl io::Write for Text {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl LogCapture {
    /// Hold what this returns to the test's end.
    pub(crate) fn start() -> LogCapture {
        let text = Arc::new(Mutex::new(Vec::new()));
        let writer = text.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .with_writer(move || Text(writer.clone()))
            .finish();
        let installed = tracing::subscriber::set_default(subscriber);
        let second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        LogCapture {
            text,
            _installed: installed,
            _second: second,
        }
    }

    /// The events so far, each with the level first.
    pub(crate) fn lines(&self) -> Vec<String> {
        String::from_utf8_lossy(&self.text.lock().unwrap())
            .lines()
            .map(|line| line.trim_start().to_string())
            .collect()
    }
}
