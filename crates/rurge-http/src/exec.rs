//! The executor hyper's HTTP/2 connections run their tasks on (`phase 4 M2
//! design 10.1`): every task a connection spawns goes onto the engine's
//! tracker, so that a stop can wait for them.

use hyper::rt::Executor;
use std::future::Future;
use tokio_util::task::TaskTracker;

/// Puts what hyper asks it to run onto a tracker (`phase 4 M2 design 10.1`,
/// `the phase 4 / M2b plan's P2`). One `Exec` serves a server connection (a
/// task per stream) and an upstream HTTP/2 connection (the connection's task
/// and each request's pipe); the tracker is the client connection's, so
/// stopping the connection waits for them.
#[derive(Clone)]
pub(crate) struct Exec(pub(crate) TaskTracker);

impl<F> Executor<F> for Exec
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn execute(&self, fut: F) {
        let tracker = self.0.clone();
        // The outer task is the tracker's: it is what a stop waits for. The
        // inner one gives the tracker the task's own result, so that a panic
        // is logged instead of ending the tracker's task silently
        // (`phase 4 M1 design 8.4`).
        tracker.spawn(async move {
            let inner = tokio::spawn(fut);
            if let Err(e) = inner.await
                && e.is_panic()
            {
                tracing::error!("an HTTP/2 task panicked: {e}");
            }
        });
    }
}
