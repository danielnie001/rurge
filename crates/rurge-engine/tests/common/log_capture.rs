//! What a test's thread logs, kept for the test to read. A test that reads the
//! log runs on the single-threaded runtime (`#[tokio::test]`): everything the
//! engine logs for it happens on the thread that installed the capture.

use std::io;
use std::sync::{Arc, Mutex};

/// Everything this thread logs while the capture lives, down to DEBUG or, for
/// a test that asks for it, down to TRACE, as the default formatter writes it
/// without time and target: one event to a line,
/// `LEVEL message field=value field=value ...`.
pub struct LogCapture {
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
    /// Hold what this returns across the test's awaits, to its end.
    pub fn start() -> LogCapture {
        LogCapture::down_to(tracing::Level::DEBUG)
    }

    /// The same down to TRACE, where the libraries say the most: for a test
    /// that looks for something that is to be in no line at all.
    pub fn everything() -> LogCapture {
        LogCapture::down_to(tracing::Level::TRACE)
    }

    fn down_to(level: tracing::Level) -> LogCapture {
        let text = Arc::new(Mutex::new(Vec::new()));
        let writer = text.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(level)
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
    pub fn lines(&self) -> Vec<String> {
        String::from_utf8_lossy(&self.text.lock().unwrap())
            .lines()
            .map(|line| line.trim_start().to_string())
            .collect()
    }
}
