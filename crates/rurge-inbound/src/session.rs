//! The seam between listeners and the engine (M3 design §6.1).

use rurge_config::rule::ProtocolKind;
use rurge_config::session::SessionInfo;
use rurge_net::BoxFuture;
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionOutcome {
    Completed,
    Rejected(RejectKind),
    Failed(String),
}

type FinishHook = Box<dyn FnOnce(&SessionHandle, &SessionOutcome) + Send>;
type FirstByteHook = Box<dyn FnOnce(&SessionHandle) + Send>;

/// Per-session bookkeeping shared by the listener (bytes) and the engine
/// (rule, policy chain, outcome). Finishing is idempotent.
pub struct SessionHandle {
    id: u64,
    started: Instant,
    session: SessionInfo,
    rule: Mutex<Option<String>>,
    policy_chain: Mutex<Vec<String>>,
    error: Mutex<Option<String>>,
    sni: Mutex<Option<String>>,
    protocol: Mutex<Option<ProtocolKind>>,
    up: AtomicU64,
    down: AtomicU64,
    /// Since `started`: when the outbound was ready (`mark_connected`).
    connected: OnceLock<Duration>,
    /// Since `started`: when the first byte came back from upstream.
    first_byte: OnceLock<Duration>,
    on_first_byte: Mutex<Vec<FirstByteHook>>,
    /// Upstream ended, before sending anything, while the client was still
    /// there (`mark_upstream_failed`).
    upstream_failed: AtomicBool,
    finished: AtomicBool,
    outcome: Mutex<Option<SessionOutcome>>,
    on_finish: Mutex<Vec<FinishHook>>,
    token: CancellationToken,
    killed: AtomicBool,
}

impl SessionHandle {
    pub fn new(id: u64, session: SessionInfo) -> Arc<SessionHandle> {
        SessionHandle::new_with_token(id, session, CancellationToken::new())
    }

    pub fn new_with_token(
        id: u64,
        session: SessionInfo,
        token: CancellationToken,
    ) -> Arc<SessionHandle> {
        Arc::new(SessionHandle {
            id,
            started: Instant::now(),
            session,
            rule: Mutex::new(None),
            policy_chain: Mutex::new(Vec::new()),
            error: Mutex::new(None),
            sni: Mutex::new(None),
            protocol: Mutex::new(None),
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            connected: OnceLock::new(),
            first_byte: OnceLock::new(),
            on_first_byte: Mutex::new(Vec::new()),
            upstream_failed: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            outcome: Mutex::new(None),
            on_finish: Mutex::new(Vec::new()),
            token,
            killed: AtomicBool::new(false),
        })
    }

    /// The session's cancellation token; `relay` stops when it fires.
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// Cancels the session (a `kill` request); `relay` will close and finish.
    pub fn kill(&self) {
        self.killed.store(true, Ordering::Relaxed);
        self.token.cancel();
    }

    pub fn was_killed(&self) -> bool {
        self.killed.load(Ordering::Relaxed)
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

    /// The sniffed TLS SNI, filled by the engine's relay path (M3b).
    pub fn set_sni(&self, sni: String) {
        *self.sni.lock().expect("session sni") = Some(sni);
    }
    pub fn sni(&self) -> Option<String> {
        self.sni.lock().expect("session sni").clone()
    }
    /// The sniffed application protocol; overrides the (immutable) session's.
    pub fn set_protocol(&self, protocol: ProtocolKind) {
        *self.protocol.lock().expect("session protocol") = Some(protocol);
    }
    pub fn protocol(&self) -> Option<ProtocolKind> {
        *self.protocol.lock().expect("session protocol")
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

    /// The outbound connection is ready. Only the first call counts.
    pub fn mark_connected(&self) {
        let _ = self.connected.set(self.started.elapsed());
    }

    /// The first byte came back from upstream — as it is read, before it
    /// goes on to the client. Only the first call counts; it runs the hooks
    /// of `on_first_byte`.
    pub fn mark_first_byte(&self) {
        if self.first_byte.set(self.started.elapsed()).is_ok() {
            let hooks = std::mem::take(&mut *self.on_first_byte.lock().expect("first byte hooks"));
            for hook in hooks {
                hook(self);
            }
        }
    }

    /// Whether a byte came back from upstream yet.
    pub fn first_byte_seen(&self) -> bool {
        self.first_byte.get().is_some()
    }

    /// Runs `f` once the first byte is back — right away when it is already.
    pub fn on_first_byte(&self, f: impl FnOnce(&SessionHandle) + Send + 'static) {
        let mut hooks = self.on_first_byte.lock().expect("first byte hooks");
        if self.first_byte_seen() {
            drop(hooks);
            f(self);
        } else {
            hooks.push(Box::new(f));
        }
    }

    /// Upstream closed or failed before sending anything back, while the
    /// client was still there: the member of a `smart` group that carried
    /// the session is to blame (phase 2 M3c design 4.3).
    pub fn mark_upstream_failed(&self) {
        self.upstream_failed.store(true, Ordering::Release);
    }

    pub fn upstream_failed(&self) -> bool {
        self.upstream_failed.load(Ordering::Acquire)
    }

    /// From the session's start until its outbound was ready (rule
    /// matching, name resolution and any wait included).
    pub fn connect_time(&self) -> Option<Duration> {
        self.connected.get().copied()
    }

    /// From the outbound being ready until the first byte came back.
    pub fn first_byte_time(&self) -> Option<Duration> {
        let connected = self.connected.get()?;
        let first = self.first_byte.get()?;
        Some(first.saturating_sub(*connected))
    }

    /// Adds a hook `finish` runs, once, after the ones added before it (the
    /// engine's session log first).
    pub fn on_finish(&self, f: impl FnOnce(&SessionHandle, &SessionOutcome) + Send + 'static) {
        self.on_finish
            .lock()
            .expect("finish hooks")
            .push(Box::new(f));
    }

    pub fn finish(&self, outcome: SessionOutcome) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        *self.outcome.lock().expect("outcome") = Some(outcome.clone());
        let hooks = std::mem::take(&mut *self.on_finish.lock().expect("finish hooks"));
        for hook in hooks {
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
            if n > 0 {
                self.handle.mark_first_byte();
            }
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
    /// `Some` when `stream` leads to an HTTP proxy that takes plain requests
    /// in absolute form (no CONNECT was sent): the headers to put on the
    /// request — they replace same-name headers — rendered once for this
    /// connection. Only ever set for a plain request of the HTTP listener.
    pub forward: Option<Vec<(String, String)>>,
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

    /// The two moments of the request log (M3c design 4.1): each is set by
    /// its first mark only, and the first byte is measured from the moment
    /// the outbound was ready.
    #[test]
    fn the_outbound_and_its_first_byte_are_marked_once() {
        let h = SessionHandle::new(3, SessionInfo::tcp(HostName::parse("a.test"), 443));
        assert_eq!((h.connect_time(), h.first_byte_time()), (None, None));
        h.mark_connected();
        let connected = h.connect_time().expect("marked");
        assert_eq!(h.first_byte_time(), None, "nothing came back yet");
        h.mark_first_byte();
        let first = h.first_byte_time().expect("marked");
        std::thread::sleep(Duration::from_millis(5));
        h.mark_connected();
        h.mark_first_byte();
        assert_eq!(h.connect_time(), Some(connected), "the first mark stands");
        assert_eq!(h.first_byte_time(), Some(first), "the first mark stands");
    }

    /// Bytes read from upstream through `Counting` mark the first byte; bytes
    /// written to it, and the end of the stream, do not.
    #[tokio::test]
    async fn counting_marks_the_first_byte_read_from_upstream() {
        let (mut a, b) = tokio::io::duplex(64);
        let h = SessionHandle::new(2, SessionInfo::tcp(HostName::parse("a.test"), 80));
        h.mark_connected();
        let mut counted = Counting::new(b, h.clone());
        counted.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        a.read_exact(&mut buf).await.unwrap();
        assert_eq!(h.first_byte_time(), None, "a write is not a response");
        a.write_all(b"ok").await.unwrap();
        let mut buf2 = [0u8; 2];
        counted.read_exact(&mut buf2).await.unwrap();
        assert!(h.first_byte_time().is_some());
    }

    /// Every hook runs, once, in the order they came (M3c design 4.3: the
    /// engine hangs a `smart` group's report on the session after its log).
    #[test]
    fn every_finish_hook_runs_once_in_order() {
        let h = SessionHandle::new(5, SessionInfo::tcp(HostName::parse("a.test"), 443));
        let order = Arc::new(Mutex::new(Vec::new()));
        for n in 1..=2 {
            let order = order.clone();
            h.on_finish(move |_, _| order.lock().unwrap().push(n));
        }
        h.finish(SessionOutcome::Completed);
        h.finish(SessionOutcome::Completed);
        assert_eq!(*order.lock().unwrap(), [1, 2]);
    }

    /// A first-byte hook runs when the first byte comes back — right away
    /// when it already has — and once.
    #[test]
    fn a_first_byte_hook_runs_once_when_the_byte_is_back() {
        let h = SessionHandle::new(6, SessionInfo::tcp(HostName::parse("a.test"), 443));
        let runs = Arc::new(AtomicUsize::new(0));
        let r = runs.clone();
        h.on_first_byte(move |_| {
            r.fetch_add(1, Ordering::SeqCst);
        });
        assert!(!h.first_byte_seen());
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        h.mark_first_byte();
        h.mark_first_byte();
        assert!(h.first_byte_seen());
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        let r = runs.clone();
        h.on_first_byte(move |_| {
            r.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(runs.load(Ordering::SeqCst), 2, "already back: runs at once");
    }

    #[test]
    fn upstream_failing_is_marked() {
        let h = SessionHandle::new(7, SessionInfo::tcp(HostName::parse("a.test"), 443));
        assert!(!h.upstream_failed());
        h.mark_upstream_failed();
        assert!(h.upstream_failed());
    }

    #[tokio::test]
    async fn the_end_of_the_stream_is_no_first_byte() {
        let (a, b) = tokio::io::duplex(64);
        let h = SessionHandle::new(4, SessionInfo::tcp(HostName::parse("a.test"), 80));
        h.mark_connected();
        let mut counted = Counting::new(b, h.clone());
        drop(a);
        let mut buf = Vec::new();
        counted.read_to_end(&mut buf).await.unwrap();
        assert_eq!(h.first_byte_time(), None);
    }

    #[test]
    fn kill_cancels_the_token_and_marks_the_handle() {
        let tok = tokio_util::sync::CancellationToken::new();
        let h = SessionHandle::new_with_token(
            9,
            SessionInfo::tcp(HostName::parse("a.test"), 443),
            tok.child_token(),
        );
        assert!(!h.token().is_cancelled());
        assert!(!h.was_killed());
        h.kill();
        assert!(h.token().is_cancelled());
        assert!(h.was_killed());
        // cancelling the parent also cancels a plain handle's token
        let h2 = SessionHandle::new(1, SessionInfo::tcp(HostName::parse("b.test"), 80));
        assert!(!h2.token().is_cancelled());
    }

    #[test]
    fn sni_and_protocol_overrides_default_to_none() {
        let h = SessionHandle::new(1, SessionInfo::tcp(HostName::parse("a.test"), 443));
        assert_eq!(h.sni(), None);
        assert_eq!(h.protocol(), None);
        h.set_sni("api.test".into());
        h.set_protocol(rurge_config::rule::ProtocolKind::Https);
        assert_eq!(h.sni().as_deref(), Some("api.test"));
        assert_eq!(h.protocol(), Some(rurge_config::rule::ProtocolKind::Https));
    }
}
