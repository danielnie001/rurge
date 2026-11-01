//! The one place a request's capture is kept while the request is served
//! (`phase 4 M4 design 6`): where it goes, what was seen of it so far, and when
//! it can be stored.
//!
//! It sits here, beside the store, because the engine has to hand it the
//! request at three moments it already has open code for: when the rules are
//! worked out (`arm`), when the request is about to leave with the headers it
//! really carries (`leaving`, plus `RequestTap` for its body on the way up),
//! and when the answer comes back (`answered`, plus `CaptureBody` around the
//! body on the way down). The engine changes nothing else (`phase 4 / M4a
//! plan P9`).
//!
//! The state machine is one value under one lock: a request is *waiting* for
//! its answer, it has *nothing* coming back (a refused or dropped request, a
//! closed connection, a client that reset the stream), or it was *given* one;
//! the body of a given answer is over when `ResponseTap::done` says so. An
//! entry is stored at the first moment the record has ended and the answer is
//! over -- `try_commit` is called from every path that may be the last of the
//! two, and stores at most once because `pending` is taken.
//!
//! Two locks are ever held here, and in one order: this one, then the store's
//! (`Armed` -> `CaptureStore`), the store never taking the other. While this
//! one is held nothing polls a body, finishes a record or registers a callback
//! on it: `finish` runs the callbacks it took under its own lock, and a body is
//! polled by hyper, so waiting for either of them under this lock would be
//! waiting for something that waits for this lock.
//!
//! The callback `arm` registers holds a strong `Arc` to the state, and the
//! state holds the handle: until the request ends, that is a cycle. It has to
//! be strong: the slot and the body can both be dropped before the handle
//! finishes, and with a weak reference the entry would be lost exactly then.

use super::{Capture, Entry, Head, Outcome, Pending, ResponseHead, Timings};
use bytes::Bytes;
use http::Response;
use http_body_util::BodyExt;
use hyper::body::{Frame, SizeHint};
use rurge_inbound::responses::{BoxError, ResponseBody};
use rurge_inbound::{Close, SessionHandle, SessionOutcome};
use rurge_proto::RejectKind;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;

/// Where one request is captured into, if anywhere (`phase 4 / M4a plan P8`).
pub(crate) struct CaptureSlot {
    /// The state, once the rules said this request is captured. A request
    /// that is not captured never puts anything here, and `arm` does not call
    /// the closure that makes the head then.
    arm: OnceLock<Arc<Mutex<Armed>>>,
}

/// What is known of one captured request so far, under one lock.
struct Armed {
    handle: Arc<SessionHandle>,
    session: Arc<str>,
    /// The request's start: the moment `arm` runs, moved back by as much time
    /// as the handle has been going, so that a held-up request keeps the time
    /// it began at.
    started: std::time::SystemTime,
    /// The bodies as they come; `None` once the entry is stored.
    pending: Option<Pending>,
    /// The request as it last left, or was going to leave.
    request: Option<Head>,
    reused: bool,
    upgrading: bool,
    /// When the record ended and how, once it did.
    finished: Option<(std::time::Duration, rurge_inbound::SessionOutcome)>,
    answer: Answer,
}

/// What came back for the request.
enum Answer {
    /// The answer is not known yet.
    Waiting,
    /// Nothing comes back: the connection is closed, the request was dropped,
    /// or the client is gone.
    Nothing,
    /// An answer, whose body is over at `done`.
    Given {
        head: ResponseHead,
        done: Option<std::time::Duration>,
    },
}

impl CaptureSlot {
    /// A slot nothing is captured into yet.
    pub(crate) fn new() -> CaptureSlot {
        CaptureSlot {
            arm: OnceLock::new(),
        }
    }

    /// Start capturing the request into `capture`, at `head` and under
    /// `handle`'s record (`phase 4 M4 design 6.1`). Nothing is captured when
    /// `capture` is `None` or the slot was armed already, or when the request
    /// ended already.
    pub(crate) fn arm(
        &self,
        capture: Option<Capture>,
        handle: &Arc<SessionHandle>,
        head: impl FnOnce() -> Head,
    ) {
        let Some(capture) = capture else { return };
        if self.arm.get().is_some() {
            return;
        }
        let started = SystemTime::now()
            .checked_sub(handle.elapsed())
            .unwrap_or_else(SystemTime::now);
        let state = Arc::new(Mutex::new(Armed {
            handle: handle.clone(),
            session: capture.session.clone(),
            started,
            pending: Some(capture.begin()),
            request: Some(head()),
            reused: false,
            upgrading: false,
            finished: None,
            answer: Answer::Waiting,
        }));
        if self.arm.set(state.clone()).is_err() {
            return;
        }
        // Outside every lock: the handle may be over already, and then this
        // calls the closure right here.
        handle.when_finished(move |handle, outcome| {
            let mut armed = lock(&state);
            armed.finished = Some((handle.elapsed(), outcome.clone()));
            // The only record that ends before any answer is a dropped one
            // (`phase 4 M4 design 6.4`): it is stored now and not when the
            // hold is over.
            if matches!(outcome, SessionOutcome::Rejected(RejectKind::Drop))
                && matches!(armed.answer, Answer::Waiting)
            {
                armed.answer = Answer::Nothing;
            }
            armed.try_commit();
        });
    }

    /// Whether this request is captured.
    pub(crate) fn armed(&self) -> bool {
        self.arm.get().is_some()
    }

    /// The request as it leaves: what it carries and whether it borrowed the
    /// connection (`phase 4 M4 design 6.1`).
    pub(crate) fn leaving(&self, head: Head, reused: bool) {
        let Some(state) = self.arm.get() else { return };
        let mut armed = lock(state);
        armed.request = Some(head);
        armed.reused = reused;
    }

    /// The request's body as it goes up, when the request is captured.
    pub(crate) fn request_tap(&self) -> Option<RequestTap> {
        self.arm.get().map(|state| RequestTap(state.clone()))
    }

    /// The answer is an upgrade or a tunnel: it is stored as it is now, and
    /// nothing of its body is captured (`phase 4 M4 design 6.4`).
    pub(crate) fn upgrading(&self) {
        let Some(state) = self.arm.get() else { return };
        lock(state).upgrading = true;
    }

    /// The answer, or the reason there is none; the body is wrapped when it is
    /// captured (`phase 4 M4 design 6.3`).
    pub(crate) fn answered(
        self,
        result: Result<Response<ResponseBody>, Close>,
        h2: bool,
    ) -> Result<Response<ResponseBody>, Close> {
        let Some(state) = self.arm.get().cloned() else {
            return result;
        };
        match result {
            Err(close) => {
                let mut armed = lock(&state);
                armed.answer = Answer::Nothing;
                armed.try_commit();
                Err(close)
            }
            Ok(resp) => {
                let mut armed = lock(&state);
                if armed.pending.is_none() {
                    return Ok(resp);
                }
                let head = ResponseHead {
                    status: resp.status(),
                    // what the client was told, not what the upstream spoke:
                    // an HTTP/2 client reads an HTTP/1.1 upstream's answer as
                    // HTTP/2 (`M4-D28`)
                    version: if h2 {
                        http::Version::HTTP_2
                    } else {
                        resp.version()
                    },
                    headers: resp.headers().clone(),
                };
                if armed.upgrading {
                    // the HTTP part is over: the 101 or the 2xx is the answer,
                    // its body is empty and nothing after it is captured
                    let at = armed.handle.elapsed();
                    armed.answer = Answer::Given {
                        head,
                        done: Some(at),
                    };
                    armed.try_commit();
                    return Ok(resp);
                }
                armed.answer = Answer::Given { head, done: None };
                drop(armed);
                Ok(resp.map(|body| {
                    CaptureBody {
                        inner: body,
                        tap: ResponseTap {
                            state: state.clone(),
                            done: false,
                        },
                    }
                    .boxed()
                }))
            }
        }
    }
}

/// Takes the request's body as it goes up.
#[derive(Clone)]
pub(crate) struct RequestTap(Arc<Mutex<Armed>>);

/// The last thing to go when the request is served without an answer.
impl Drop for CaptureSlot {
    fn drop(&mut self) {
        // The future of the request is gone without an answer: HTTP/2's client
        // reset the stream, or whoever served it let go. The record is over,
        // so the entry is stored with no response.
        let Some(state) = self.arm.get() else { return };
        let mut armed = lock(state);
        if matches!(armed.answer, Answer::Waiting) {
            armed.answer = Answer::Nothing;
        }
        armed.try_commit();
    }
}

impl RequestTap {
    /// Take the next chunk of the request's body.
    pub(crate) fn chunk(&self, data: &[u8]) {
        let mut armed = lock(&self.0);
        if let Some(pending) = &mut armed.pending {
            pending.request_chunk(data);
        }
    }
}

/// The answer's body as it goes down to the client: every frame handed on is
/// taken for the entry, and the body is over when hyper sees the last one
/// (`phase 4 M4 design 6.3`).
///
/// The fields are in the order they have to be dropped in: `inner` first --
/// `Tail`, `LocalFile` and the guard end the record when they go -- and the tap
/// after it, so that the end the record writes is there when the tap stores,
/// whatever order the two happen to be done in. This type implements no `Drop`
/// of its own for exactly that reason.
struct CaptureBody {
    inner: ResponseBody,
    tap: ResponseTap,
}

impl hyper::body::Body for CaptureBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        // not under the lock: hyper polls this from inside `Tail`, and `Tail`
        // ends the record before it hands the last frame on
        let polled = std::pin::Pin::new(&mut this.inner).poll_frame(cx);
        match &polled {
            std::task::Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.tap.chunk(data);
                }
                if this.inner.is_end_stream() {
                    this.tap.done();
                }
            }
            std::task::Poll::Ready(Some(Err(_))) | std::task::Poll::Ready(None) => {
                this.tap.done();
            }
            std::task::Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// Takes the answer's body as it goes down.
struct ResponseTap {
    state: Arc<Mutex<Armed>>,
    /// Whether the body is over already, so that dropping the tap after the
    /// last frame does not end it twice.
    done: bool,
}

impl ResponseTap {
    /// Take the next chunk of the answer's body.
    fn chunk(&mut self, data: &[u8]) {
        let mut armed = lock(&self.state);
        if let Some(pending) = &mut armed.pending {
            pending.response_chunk(data);
        }
    }

    /// The answer's body is over: its end is now, and the entry may be stored.
    fn done(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        let mut armed = lock(&self.state);
        let now = armed.handle.elapsed();
        if let Answer::Given {
            done: at @ None, ..
        } = &mut armed.answer
        {
            *at = Some(now);
        }
        armed.try_commit();
    }
}

impl Drop for ResponseTap {
    fn drop(&mut self) {
        // a body the client never took to its end -- it left, or the
        // connection was closed -- is over here
        self.done();
    }
}

impl Armed {
    /// Store the entry once both halves are over: the record ended, and the
    /// answer either never came or its body is done (`phase 4 M4 design 6.4`).
    /// Stores at most once, because `pending` is taken.
    fn try_commit(&mut self) {
        if self.pending.is_none() {
            return;
        }
        let at = match &self.finished {
            Some((at, _)) => *at,
            // an upgrade is stored as it is, whatever the record does later
            None if self.upgrading => self.handle.elapsed(),
            None => return,
        };
        let ended = match &self.answer {
            Answer::Waiting => return,
            Answer::Given { done: None, .. } => return,
            Answer::Nothing => at,
            Answer::Given {
                done: Some(done), ..
            } => at.max(*done),
        };
        let pending = self.pending.take().expect("checked just above");
        let response = match std::mem::replace(&mut self.answer, Answer::Nothing) {
            Answer::Given { head, .. } => Some(head),
            _ => None,
        };
        let request = self.request.take().expect("the head `arm` made");
        let handle = self.handle.clone();
        let connect = handle.connect_time();
        let wait = handle.first_byte_time();
        let timings = Timings {
            connect,
            dns: handle.trail().dns(),
            wait,
            receive: match (connect, wait) {
                (Some(connect), Some(wait)) => Some(ended.saturating_sub(connect + wait)),
                _ => None,
            },
            total: ended,
            reused: self.reused,
        };
        let outcome = if self.upgrading {
            Outcome::Upgraded
        } else {
            match self.finished.as_ref().map(|(_, outcome)| outcome) {
                Some(SessionOutcome::Completed) => Outcome::Completed,
                Some(SessionOutcome::Rejected(kind)) => Outcome::Rejected(*kind),
                Some(SessionOutcome::Failed(_)) => Outcome::Failed,
                None => return,
            }
        };
        let error = handle.error().or_else(|| match &self.finished {
            Some((_, SessionOutcome::Failed(message))) => Some(message.clone()),
            _ => None,
        });
        let id = handle.id();
        let session = self.session.clone();
        let started = self.started;
        let remote = handle.trail().peer();
        let rule = handle.rule();
        let policy = handle.policy_chain();
        let notes = handle.notes();
        pending.commit(|request_body, response_body| Entry {
            id,
            session,
            started,
            request,
            request_body,
            response,
            response_body,
            timings,
            remote,
            outcome,
            error,
            rule,
            policy,
            notes,
        });
    }
}

/// Hold the armed state's lock; a panic elsewhere is no reason to stop
/// capturing.
fn lock<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    match state.lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    }
}
