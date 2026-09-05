//! The seam between listeners and the engine (M3 design §6.1).

use rurge_config::session::SessionInfo;
use rurge_net::BoxFuture;
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionOutcome {
    Completed,
    Rejected(RejectKind),
    Failed(String),
}

type FinishHook = Box<dyn FnOnce(&SessionHandle, &SessionOutcome) + Send>;

/// Per-session bookkeeping shared by the listener (bytes) and the engine
/// (rule, policy chain, outcome). Finishing is idempotent.
pub struct SessionHandle {
    id: u64,
    started: Instant,
    session: SessionInfo,
    rule: Mutex<Option<String>>,
    policy_chain: Mutex<Vec<String>>,
    error: Mutex<Option<String>>,
    up: AtomicU64,
    down: AtomicU64,
    finished: AtomicBool,
    outcome: Mutex<Option<SessionOutcome>>,
    on_finish: Mutex<Option<FinishHook>>,
}

impl SessionHandle {
    pub fn new(id: u64, session: SessionInfo) -> Arc<SessionHandle> {
        Arc::new(SessionHandle {
            id,
            started: Instant::now(),
            session,
            rule: Mutex::new(None),
            policy_chain: Mutex::new(Vec::new()),
            error: Mutex::new(None),
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            finished: AtomicBool::new(false),
            outcome: Mutex::new(None),
            on_finish: Mutex::new(None),
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn session(&self) -> &SessionInfo {
        &self.session
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn set_rule(&self, rule: Option<String>) {
        *self.rule.lock().expect("session rule") = rule;
    }

    pub fn rule(&self) -> Option<String> {
        self.rule.lock().expect("session rule").clone()
    }

    pub fn set_policy_chain(&self, chain: Vec<String>) {
        *self.policy_chain.lock().expect("policy chain") = chain;
    }

    pub fn policy_chain(&self) -> Vec<String> {
        self.policy_chain.lock().expect("policy chain").clone()
    }

    /// Why the session could not run as asked, when the outcome alone does not
    /// say (a policy naming a protocol rurge has not implemented, say).
    pub fn set_error(&self, msg: impl Into<String>) {
        *self.error.lock().expect("session error") = Some(msg.into());
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().expect("session error").clone()
    }

    pub fn add_up(&self, n: u64) {
        self.up.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_down(&self, n: u64) {
        self.down.fetch_add(n, Ordering::Relaxed);
    }

    /// (client → upstream, upstream → client) bytes so far.
    pub fn bytes(&self) -> (u64, u64) {
        (
            self.up.load(Ordering::Relaxed),
            self.down.load(Ordering::Relaxed),
        )
    }

    /// Installs the hook `finish` runs once (the engine's session log).
    pub fn on_finish(&self, f: impl FnOnce(&SessionHandle, &SessionOutcome) + Send + 'static) {
        *self.on_finish.lock().expect("finish hook") = Some(Box::new(f));
    }

    pub fn finish(&self, outcome: SessionOutcome) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        *self.outcome.lock().expect("outcome") = Some(outcome.clone());
        let hook = self.on_finish.lock().expect("finish hook").take();
        if let Some(hook) = hook {
            hook(self, &outcome);
        }
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    pub fn outcome(&self) -> Option<SessionOutcome> {
        self.outcome.lock().expect("outcome").clone()
    }
}

/// Counts bytes flowing through a stream into a session handle: reads are
/// `down` (upstream → client), writes are `up` (client → upstream).
pub struct Counting<S> {
    inner: S,
    handle: Arc<SessionHandle>,
}

impl<S> Counting<S> {
    pub fn new(inner: S, handle: Arc<SessionHandle>) -> Counting<S> {
        Counting { inner, handle }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Counting<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &res {
            let n = buf.filled().len() - before;
            self.handle.add_down(n as u64);
        }
        res
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Counting<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, data);
        if let Poll::Ready(Ok(n)) = &res {
            self.handle.add_up(*n as u64);
        }
        res
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Why a dial failed (drives the protocol-level error code, design §8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailKind {
    Dns,
    Connect,
    Timeout,
    Other,
}

pub struct Dialed {
    pub stream: BoxedStream,
    pub handle: Arc<SessionHandle>,
}

pub enum DialError {
    /// The policy rejected the session; the handle is already finished.
    Reject {
        kind: RejectKind,
        rule: Option<String>,
        handle: Arc<SessionHandle>,
    },
    /// Resolution or connection failed; the handle is already finished.
    Failed {
        kind: FailKind,
        message: String,
        rule: Option<String>,
        handle: Arc<SessionHandle>,
    },
}

/// Implemented by the engine: rules → policy → outbound (`dial`), then the
/// counted bidirectional copy (`relay`, which finishes the handle).
pub trait Dialer: Send + Sync {
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>>;
    fn relay<'a>(
        &'a self,
        client: BoxedStream,
        upstream: BoxedStream,
        handle: Arc<SessionHandle>,
    ) -> BoxFuture<'a, ()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn finish_runs_the_hook_once_and_keeps_the_outcome() {
        let h = SessionHandle::new(7, SessionInfo::tcp(HostName::parse("a.test"), 443));
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        h.on_finish(move |handle, outcome| {
            assert_eq!(handle.id(), 7);
            assert_eq!(*outcome, SessionOutcome::Completed);
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert!(!h.is_finished());
        h.set_rule(Some("DOMAIN,a.test,DIRECT".into()));
        h.set_policy_chain(vec!["DIRECT".into()]);
        h.finish(SessionOutcome::Completed);
        h.finish(SessionOutcome::Failed("late".into()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(h.outcome(), Some(SessionOutcome::Completed));
        assert_eq!(h.rule().as_deref(), Some("DOMAIN,a.test,DIRECT"));
        assert_eq!(h.policy_chain(), vec!["DIRECT".to_string()]);
    }

    #[tokio::test]
    async fn counting_stream_attributes_bytes() {
        let (mut a, b) = tokio::io::duplex(64);
        let h = SessionHandle::new(1, SessionInfo::tcp(HostName::parse("a.test"), 80));
        let mut counted = Counting::new(b, h.clone());
        counted.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        a.read_exact(&mut buf).await.unwrap();
        a.write_all(b"ok").await.unwrap();
        let mut buf2 = [0u8; 2];
        counted.read_exact(&mut buf2).await.unwrap();
        assert_eq!(h.bytes(), (5, 2));
    }
}
