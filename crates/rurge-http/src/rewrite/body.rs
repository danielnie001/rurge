//! A body on its way to a `[Body Rewrite]` rule (`phase 4 M3 design 9`): when
//! it is buffered at all, how much of it is read, and what a compressed one
//! decodes to.
//!
//! Buffering is decided once, when the header is read (`request_buffer`,
//! `response_buffer`, `phase 4 M3 design 9.2`): a request whose body is chunked,
//! expected with `100-continue`, or of no known length over HTTP/2 is forwarded
//! as it came, and the rule is skipped with the reason kept. `read_ahead` reads
//! a whole body up to a limit and a stall and the request's end
//! (`phase 4 M3 design 9.3`); `ReadAhead` puts what was read back in front of
//! the rest of the body, so a body that was not changed goes out with its
//! bytes, its framing and its trailers as they came.
//!
//! `decode` undoes the `Content-Encoding` layers of a body up to the same
//! limit (`phase 4 M3 design 9.4`); it does not put them back. Nothing here
//! touches the network, and no byte of a body goes into a note, a log line or
//! an error text.

use crate::pool::Meter;
use bytes::{BufMut, Bytes, BytesMut};
use flate2::read::{DeflateDecoder, MultiGzDecoder, ZlibDecoder};
use http::header::{EXPECT, TRANSFER_ENCODING};
use http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use http_body_util::BodyExt;
use hyper::body::{Body, Frame, SizeHint};
use std::io::Read;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// How large a body may be and still be buffered for a rewrite, and how that
/// number is written in a note (`phase 4 M3 design 9.3`, the phase 4 / M3b
/// plan's P9): the manual's megabytes read as binary ones (M3-D20).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limit {
    pub(crate) bytes: u64,
    pub(crate) text: &'static str,
}

/// What a request's body may be: the manual's 32 MB (`phase 4 M3 design 9.3`).
pub(crate) const REQUEST: Limit = Limit {
    bytes: 33_554_432,
    text: "32 MiB",
};

/// What a response's body may be: the manual's 10 MB for macOS
/// (`phase 4 M3 design 9.3`).
pub(crate) const RESPONSE: Limit = Limit {
    bytes: 10_485_760,
    text: "10 MiB",
};

/// A body with the bytes read ahead of a rewrite put back in front of it
/// (`phase 4 M3 design 9.3`, the phase 4 / M3b plan's P9).
pub(crate) struct ReadAhead<B> {
    /// The bytes read into a buffer, `None` once they went out or when there
    /// were none: an empty buffer is never held here.
    ahead: Option<Bytes>,
    /// The trailers read along with them, given out after the rest of the
    /// body ended.
    trailers: Option<HeaderMap>,
    /// What is left of the body, `None` once it ended or when there was no
    /// body to begin with.
    rest: Option<B>,
}

impl<B> ReadAhead<B> {
    /// A body nobody read: its frames are `body`'s own.
    pub(crate) fn through(body: B) -> ReadAhead<B> {
        ReadAhead {
            ahead: None,
            trailers: None,
            rest: Some(body),
        }
    }

    /// A body that was read whole and is given back from the buffer.
    pub(crate) fn whole(data: Bytes, trailers: Option<HeaderMap>) -> ReadAhead<B> {
        ReadAhead {
            ahead: (!data.is_empty()).then_some(data),
            trailers,
            rest: None,
        }
    }

    /// A body whose first `ahead` bytes were read and whose rest is `rest`.
    pub(crate) fn resumed(ahead: Bytes, rest: B) -> ReadAhead<B> {
        ReadAhead {
            ahead: (!ahead.is_empty()).then_some(ahead),
            trailers: None,
            rest: Some(rest),
        }
    }

    /// Whether the next frame does not come from `rest`: bytes are still
    /// buffered, or there is no `rest` at all. A frame from `rest` is counted
    /// onto the request again only when this is false (`phase 4 M3 design
    /// 9.3`, the phase 4 / M3b plan's P14).
    pub(crate) fn has_ahead(&self) -> bool {
        self.ahead.is_some() || self.rest.is_none()
    }
}

impl<B> Body for ReadAhead<B>
where
    B: Body<Data = Bytes> + Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, B::Error>>> {
        let this = self.get_mut();
        if let Some(ahead) = this.ahead.take() {
            return Poll::Ready(Some(Ok(Frame::data(ahead))));
        }
        if let Some(rest) = this.rest.as_mut() {
            match Pin::new(rest).poll_frame(cx) {
                Poll::Ready(Some(frame)) => return Poll::Ready(Some(frame)),
                Poll::Ready(None) => this.rest = None,
                Poll::Pending => return Poll::Pending,
            }
        }
        match this.trailers.take() {
            Some(trailers) => Poll::Ready(Some(Ok(Frame::trailers(trailers)))),
            None => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.ahead.is_none()
            && self.trailers.is_none()
            && self.rest.as_ref().is_none_or(Body::is_end_stream)
    }

    fn size_hint(&self) -> SizeHint {
        let ahead = self.ahead.as_ref().map_or(0, |ahead| ahead.len() as u64);
        let mut hint = SizeHint::new();
        match &self.rest {
            Some(rest) => {
                let rest = rest.size_hint();
                hint.set_lower(ahead.saturating_add(rest.lower()));
                if let Some(upper) = rest.upper() {
                    hint.set_upper(ahead.saturating_add(upper));
                }
            }
            // a trailer can be written with an unknown length only: a known
            // one makes hyper frame the body by it
            None if self.trailers.is_some() => hint.set_lower(ahead),
            None => hint.set_exact(ahead),
        }
        hint
    }
}

/// What reading a body ahead came to (`phase 4 M3 design 9.3`).
pub(crate) enum Ahead<B: Body> {
    /// The body ended: everything read is in `data`, and `trailers` is what
    /// came with it, if anything.
    Whole {
        data: Bytes,
        trailers: Option<HeaderMap>,
    },
    /// The body went over the limit: `ahead` holds what was read, `rest` what
    /// was not. A response streams `ahead` and then `rest` through
    /// (`phase 4 M3 design 9.3`).
    Over { ahead: Bytes, rest: B },
    /// Nothing came for longer than `idle`.
    Stalled,
    /// The body ended with an error.
    Failed(B::Error),
    /// The request was ended while the body was being read.
    Cancelled,
}

/// Reads a body ahead for a rewrite (`phase 4 M3 design 9.3`, the phase 4 /
/// M3b plan's P9): a frame's wait is bounded by `idle` and races the request's
/// end, a body whose length is already over `limit` is not read at all, and
/// every byte read counts onto `meter`'s request (`phase 4 M3 design 9.2`).
pub(crate) async fn read_ahead<B: Body<Data = Bytes> + Unpin>(
    mut body: B,
    limit: u64,
    idle: Duration,
    token: &CancellationToken,
    meter: Option<&Meter>,
) -> Ahead<B> {
    if body.size_hint().lower() > limit {
        // the length alone says it is over: not a frame is read
        return Ahead::Over {
            ahead: Bytes::new(),
            rest: body,
        };
    }
    let mut ahead = BytesMut::new();
    let mut trailers = None;
    loop {
        let waited = tokio::select! {
            biased;
            _ = token.cancelled() => None,
            frame = tokio::time::timeout(idle, body.frame()) => Some(frame),
        };
        match waited {
            None => return Ahead::Cancelled,
            Some(Err(_elapsed)) => return Ahead::Stalled,
            Some(Ok(None)) => {
                return Ahead::Whole {
                    data: ahead.freeze(),
                    trailers,
                };
            }
            Some(Ok(Some(Err(e)))) => return Ahead::Failed(e),
            Some(Ok(Some(Ok(frame)))) => match frame.into_data() {
                Ok(data) => {
                    if let Some(meter) = meter {
                        meter.count_down(data.len() as u64);
                    }
                    ahead.put_slice(&data);
                    if ahead.len() as u64 > limit {
                        return Ahead::Over {
                            ahead: ahead.freeze(),
                            rest: body,
                        };
                    }
                }
                Err(frame) => {
                    if let Ok(seen) = frame.into_trailers() {
                        trailers = Some(seen);
                    }
                }
            },
        }
    }
}

/// Whether a body is buffered for a rewrite, and if not, why
/// (`phase 4 M3 design 9.2`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Buffer {
    Yes,
    Skip(&'static str),
}

/// Whether a request's body is buffered (`phase 4 M3 design 9.2`, the phase 4 /
/// M3b plan's P10). A request that upgrades or a tunnel is not asked: those
/// never reach this.
pub(crate) fn request_buffer<B: Body>(req: &Request<B>, h2_client: bool) -> Buffer {
    if req.headers().contains_key(TRANSFER_ENCODING) {
        return Buffer::Skip("the request body is chunked");
    }
    let expects_continue = req
        .headers()
        .get_all(EXPECT)
        .iter()
        .any(|value| value.as_bytes().eq_ignore_ascii_case(b"100-continue"));
    if expects_continue {
        return Buffer::Skip("the request expects 100-continue");
    }
    let body = req.body();
    if h2_client && !body.is_end_stream() && body.size_hint().exact().is_none() {
        return Buffer::Skip("the request body has no length");
    }
    Buffer::Yes
}

/// Whether a response's body is buffered (`phase 4 M3 design 9.2`): only one
/// that can have a body (M3-D19).
pub(crate) fn response_buffer(method: &Method, status: StatusCode) -> bool {
    *method != Method::HEAD
        && !status.is_informational()
        && status != StatusCode::NO_CONTENT
        && status != StatusCode::NOT_MODIFIED
}

/// A body with its `Content-Encoding` layers undone (`phase 4 M3 design 9.4`).
/// `Ok` is the bytes to rewrite, which is `body` itself when there was nothing
/// to undo; `Err` is the reason the rule is skipped, for the request's notes
/// (the phase 4 / M3b plan's P11). No layer decodes past `limit`.
pub(crate) fn decode(
    encodings: &[HeaderValue],
    body: &Bytes,
    limit: Limit,
) -> Result<Bytes, String> {
    // what the last layer was written as, the innermost last: the field's
    // order is the order the body was encoded in
    let mut names = Vec::new();
    for value in encodings {
        let text = value
            .to_str()
            .map_err(|_| "the content-encoding is not supported".to_string())?;
        for item in text.split(',') {
            let name = item.trim();
            if name.is_empty() || name.eq_ignore_ascii_case("identity") {
                continue;
            }
            names.push(name);
        }
    }

    let mut data = body.clone();
    for name in names.into_iter().rev() {
        let mut out = Vec::new();
        let bounded = limit.bytes.saturating_add(1);
        let read = if name.eq_ignore_ascii_case("gzip") || name.eq_ignore_ascii_case("x-gzip") {
            MultiGzDecoder::new(&data[..])
                .take(bounded)
                .read_to_end(&mut out)
        } else if name.eq_ignore_ascii_case("deflate") {
            if zlib_header(&data) {
                ZlibDecoder::new(&data[..])
                    .take(bounded)
                    .read_to_end(&mut out)
            } else {
                DeflateDecoder::new(&data[..])
                    .take(bounded)
                    .read_to_end(&mut out)
            }
        } else if is_token(name) {
            return Err(format!("content-encoding {name} is not supported"));
        } else {
            return Err("the content-encoding is not supported".to_string());
        };
        read.map_err(|_| format!("the body cannot be decoded as {name}"))?;
        if out.len() as u64 > limit.bytes {
            return Err(format!("the decoded body is over {}", limit.text));
        }
        data = Bytes::from(out);
    }
    Ok(data)
}

/// Whether `data` starts with a zlib header: the compression method is 8 and
/// the two bytes together are a multiple of 31. A `deflate` layer whose head
/// is not one is a bare deflate stream (`phase 4 M3 design 9.4`).
fn zlib_header(data: &[u8]) -> bool {
    match (data.first(), data.get(1)) {
        (Some(&cmf), Some(&flg)) => {
            cmf & 0x0f == 8 && (u16::from(cmf) * 256 + u16::from(flg)) % 31 == 0
        }
        _ => false,
    }
}

/// Whether `name` is a token of RFC 9110: what an encoding name may be, and
/// what decides whether a name nobody supports is named in the note.
fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};
    use rurge_config::HostName;
    use rurge_config::session::SessionInfo;
    use rurge_inbound::SessionHandle;
    use std::collections::VecDeque;
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// How long a frame may take when the test does not care: long enough that
    /// a frame that is there always wins.
    const LONG: Duration = Duration::from_secs(30);

    /// How long a body may take before the test calls it stalled
    /// (`phase 4 M3 design 9.3`).
    const IDLE: Duration = Duration::from_millis(100);

    /// How long a timed test may take before it is a failure: the waits here
    /// are a hundred milliseconds at most.
    const PATIENCE: Duration = Duration::from_secs(5);

    /// A body of the frames given up front, then the end — or, when `never`, a
    /// `Pending` forever. It counts the polls it takes, so a test can see that
    /// a body whose length is over the limit was never read.
    struct Fake {
        frames: VecDeque<Result<Frame<Bytes>, std::io::Error>>,
        never: bool,
        polls: Arc<AtomicUsize>,
        hint: SizeHint,
    }

    impl Fake {
        fn ending(frames: Vec<Result<Frame<Bytes>, std::io::Error>>) -> Fake {
            Fake::new(frames, false)
        }

        fn never() -> Fake {
            Fake::new(Vec::new(), true)
        }

        fn new(frames: Vec<Result<Frame<Bytes>, std::io::Error>>, never: bool) -> Fake {
            Fake {
                frames: frames.into(),
                never,
                polls: Arc::new(AtomicUsize::new(0)),
                hint: SizeHint::new(),
            }
        }

        fn with_hint(mut self, hint: SizeHint) -> Fake {
            self.hint = hint;
            self
        }

        /// A handle on the poll count, kept across the body being moved.
        fn count(&self) -> Arc<AtomicUsize> {
            self.polls.clone()
        }
    }

    impl Body for Fake {
        type Data = Bytes;
        type Error = std::io::Error;

        fn poll_frame(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, std::io::Error>>> {
            let this = self.get_mut();
            this.polls.fetch_add(1, Ordering::Relaxed);
            if let Some(frame) = this.frames.pop_front() {
                return Poll::Ready(Some(frame));
            }
            if this.never {
                return Poll::Pending;
            }
            Poll::Ready(None)
        }

        fn is_end_stream(&self) -> bool {
            !self.never && self.frames.is_empty()
        }

        fn size_hint(&self) -> SizeHint {
            self.hint
        }
    }

    /// A data frame of `text`.
    fn piece(text: &str) -> Result<Frame<Bytes>, std::io::Error> {
        Ok(Frame::data(Bytes::copy_from_slice(text.as_bytes())))
    }

    /// Three data frames and a trailer: a whole body with them.
    #[tokio::test]
    async fn read_ahead_reads_a_whole_body() {
        let mut trailers = HeaderMap::new();
        trailers.append("x-end", HeaderValue::from_static("yes"));
        let body = Fake::ending(vec![
            piece("one"),
            piece("two"),
            piece("three"),
            Ok(Frame::trailers(trailers.clone())),
        ]);
        match read_ahead(body, 100, LONG, &CancellationToken::new(), None).await {
            Ahead::Whole {
                data: bytes,
                trailers: Some(seen),
            } => {
                assert_eq!(&bytes[..], b"onetwothree");
                assert_eq!(seen, trailers);
            }
            _ => panic!("a body that ends is read whole"),
        }
    }

    /// The limit is on what was read: exactly that much is whole, one byte
    /// more is over it and stops there.
    #[tokio::test]
    async fn read_ahead_stops_one_byte_past_the_limit() {
        let exact = Fake::ending(vec![piece("12345"), piece("67890")]);
        match read_ahead(exact, 10, LONG, &CancellationToken::new(), None).await {
            Ahead::Whole {
                data: bytes,
                trailers: None,
            } => assert_eq!(&bytes[..], b"1234567890"),
            _ => panic!("ten bytes are not over the limit"),
        }

        let over = Fake::ending(vec![piece("12345"), piece("678901"), piece("x")]);
        match read_ahead(over, 10, LONG, &CancellationToken::new(), None).await {
            Ahead::Over { ahead, mut rest } => {
                assert_eq!(&ahead[..], b"12345678901");
                let frame = rest.frame().await.expect("a frame").expect("no error");
                assert_eq!(frame.into_data().expect("a data frame").as_ref(), b"x");
            }
            _ => panic!("eleven bytes are over the limit"),
        }
    }

    /// A length already over the limit is not read at all: the header is
    /// enough (`phase 4 M3 design 9.3`).
    #[tokio::test]
    async fn a_length_over_the_limit_is_not_read() {
        let body = Fake::ending(vec![piece("12345678901")]).with_hint(SizeHint::with_exact(11));
        let polls = body.count();
        match read_ahead(body, 10, LONG, &CancellationToken::new(), None).await {
            Ahead::Over { ahead, rest } => {
                assert!(ahead.is_empty(), "nothing was read");
                assert_eq!(polls.load(Ordering::Relaxed), 0, "not a frame was polled");
                assert_eq!(rest.frames.len(), 1, "the body is given back whole");
            }
            _ => panic!("the length says eleven bytes"),
        }
    }

    /// A body that never comes ends the wait after `idle`.
    #[tokio::test]
    async fn read_ahead_stalls_after_idle() {
        let body = Fake::never();
        let token = CancellationToken::new();
        let ahead = tokio::time::timeout(PATIENCE, read_ahead(body, 10, IDLE, &token, None))
            .await
            .expect("the idle wait is bounded");
        assert!(
            matches!(ahead, Ahead::Stalled),
            "a body that never comes stalls"
        );
    }

    /// The request's end stops the read at once, not after `idle`.
    #[tokio::test]
    async fn read_ahead_ends_when_cancelled() {
        let body = Fake::never();
        let token = CancellationToken::new();
        let end = token.clone();
        let cancel = async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            end.cancel();
        };
        let (ahead, ()) = tokio::time::timeout(PATIENCE, async {
            tokio::join!(read_ahead(body, 10, LONG, &token, None), cancel)
        })
        .await
        .expect("the cancellation is bounded");
        assert!(matches!(ahead, Ahead::Cancelled), "the request ended");
    }

    /// An error from the body is the read's outcome, not a stall.
    #[tokio::test]
    async fn read_ahead_passes_a_body_error_on() {
        let body = Fake::ending(vec![
            piece("abc"),
            Err(std::io::Error::other("the origin went away")),
        ]);
        match read_ahead(body, 100, LONG, &CancellationToken::new(), None).await {
            Ahead::Failed(e) => assert_eq!(e.to_string(), "the origin went away"),
            _ => panic!("the error is passed on"),
        }
    }

    /// Bytes read ahead count onto the request the meter is lent to right then,
    /// and nothing counts without one (`phase 4 M3 design 9.2`, `9.3`).
    #[tokio::test]
    async fn read_ahead_counts_on_the_meter() {
        let handle = SessionHandle::new(1, SessionInfo::tcp(HostName::parse("a.test"), 80));
        let meter = Meter::new();
        meter.set(Some(handle.clone()));

        let body = Fake::ending(vec![piece("hello"), piece("!!")]);
        match read_ahead(body, 100, LONG, &CancellationToken::new(), Some(&meter)).await {
            Ahead::Whole { data: bytes, .. } => assert_eq!(&bytes[..], b"hello!!"),
            _ => panic!("the body ends"),
        }
        assert_eq!(handle.bytes(), (0, 7), "the request's down bytes");

        let body = Fake::ending(vec![piece("hello")]);
        let ahead = read_ahead(body, 100, LONG, &CancellationToken::new(), None).await;
        assert!(matches!(ahead, Ahead::Whole { .. }));
        assert_eq!(handle.bytes(), (0, 7), "nothing more was counted");
    }

    /// What a body with bytes read ahead gives back: the bytes first, the rest
    /// of the body after, the trailers last; a length that says only what is
    /// known (`phase 4 M3 design 9.3`, the phase 4 / M3b plan's P9).
    #[tokio::test]
    async fn read_ahead_bodies_give_back_what_was_read() {
        let mut trailers = HeaderMap::new();
        trailers.append("x-end", HeaderValue::from_static("yes"));

        let mut whole =
            ReadAhead::<Fake>::whole(Bytes::from_static(b"abc"), Some(trailers.clone()));
        assert!(whole.has_ahead(), "the bytes are still buffered");
        let hint = whole.size_hint();
        assert_eq!(hint.lower(), 3);
        assert_eq!(
            hint.upper(),
            None,
            "a trailer goes out only with an unknown length"
        );
        let frame = whole.frame().await.expect("a frame").expect("no error");
        assert_eq!(frame.into_data().expect("data").as_ref(), b"abc");
        let frame = whole.frame().await.expect("a frame").expect("no error");
        assert_eq!(frame.into_trailers().expect("trailers"), trailers);
        assert!(whole.frame().await.is_none(), "nothing is left");
        assert!(whole.is_end_stream());

        let without = ReadAhead::<Fake>::whole(Bytes::from_static(b"abc"), None);
        assert_eq!(without.size_hint().exact(), Some(3), "no trailer: a length");

        let empty = ReadAhead::<Fake>::whole(Bytes::new(), None);
        assert!(empty.is_end_stream(), "nothing is in it");

        let rest = Fake::ending(vec![piece("cd")]).with_hint(SizeHint::with_exact(2));
        let mut resumed = ReadAhead::resumed(Bytes::from_static(b"ab"), rest);
        assert!(resumed.has_ahead());
        assert_eq!(resumed.size_hint().lower(), 4, "the buffer plus the rest");
        let frame = resumed.frame().await.expect("a frame").expect("no error");
        assert_eq!(frame.into_data().expect("data").as_ref(), b"ab");
        assert!(!resumed.has_ahead(), "the buffer is out, a rest is left");
        let frame = resumed.frame().await.expect("a frame").expect("no error");
        assert_eq!(frame.into_data().expect("data").as_ref(), b"cd");
        assert!(resumed.frame().await.is_none());
        assert!(resumed.is_end_stream());

        let mut through = ReadAhead::through(Fake::ending(vec![piece("ef")]));
        assert!(!through.has_ahead(), "its frames are the body's own");
        let frame = through.frame().await.expect("a frame").expect("no error");
        assert_eq!(frame.into_data().expect("data").as_ref(), b"ef");
        assert!(!through.has_ahead());
        let whole = ReadAhead::<Fake>::whole(Bytes::from_static(b"ab"), None);
        assert!(whole.has_ahead());
        assert_eq!(whole.size_hint().exact(), Some(2));
    }

    /// A request whose body arrives as it was written is forwarded as it came
    /// (`phase 4 M3 design 9.2`, the phase 4 / M3b plan's P10).
    #[test]
    fn which_requests_are_buffered() {
        let posted = Request::builder()
            .method("POST")
            .header("content-length", "5")
            .body(Fake::ending(vec![piece("hello")]))
            .expect("a request");
        assert_eq!(
            request_buffer(&posted, false),
            Buffer::Yes,
            "a length is known"
        );

        let got = Request::builder()
            .method("GET")
            .body(Fake::ending(Vec::new()))
            .expect("a request");
        assert_eq!(
            request_buffer(&got, false),
            Buffer::Yes,
            "a request without a body is buffered: a rewrite may make one"
        );

        let chunked = Request::builder()
            .header("transfer-encoding", "chunked")
            .body(Fake::ending(Vec::new()))
            .expect("a request");
        assert_eq!(
            request_buffer(&chunked, false),
            Buffer::Skip("the request body is chunked")
        );

        let expected = Request::builder()
            .header("expect", "100-Continue")
            .body(Fake::ending(Vec::new()))
            .expect("a request");
        assert_eq!(
            request_buffer(&expected, false),
            Buffer::Skip("the request expects 100-continue")
        );

        let both = Request::builder()
            .header("transfer-encoding", "chunked")
            .header("expect", "100-continue")
            .body(Fake::ending(Vec::new()))
            .expect("a request");
        assert_eq!(
            request_buffer(&both, false),
            Buffer::Skip("the request body is chunked"),
            "chunked is said first"
        );

        let unknown = Request::builder()
            .method("POST")
            .body(Fake::never())
            .expect("a request");
        assert_eq!(
            request_buffer(&unknown, true),
            Buffer::Skip("the request body has no length")
        );
        assert_eq!(
            request_buffer(&unknown, false),
            Buffer::Yes,
            "HTTP/1.1 reads to the end of the body"
        );

        let known = Request::builder()
            .method("POST")
            .body(Fake::never().with_hint(SizeHint::with_exact(3)))
            .expect("a request");
        assert_eq!(
            request_buffer(&known, true),
            Buffer::Yes,
            "HTTP/2 with a length is buffered like HTTP/1.1"
        );

        let ended = Request::builder()
            .method("POST")
            .body(Fake::ending(Vec::new()))
            .expect("a request");
        assert_eq!(
            request_buffer(&ended, true),
            Buffer::Yes,
            "there is no body to wait for"
        );
    }

    /// Only an answer that can have a body is buffered (`phase 4 M3 design
    /// 9.2`, M3-D19).
    #[test]
    fn which_answers_are_buffered() {
        let get = Method::GET;
        assert!(response_buffer(&get, StatusCode::OK), "a 200 to a GET");
        assert!(response_buffer(
            &Method::POST,
            StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(
            response_buffer(&get, StatusCode::RESET_CONTENT),
            "205 has a body"
        );
        assert!(
            !response_buffer(&Method::HEAD, StatusCode::OK),
            "HEAD has none"
        );
        for status in [100, 101, 204, 304] {
            let status = StatusCode::from_u16(status).expect("a status");
            assert!(!response_buffer(&get, status), "{status} has none");
        }
    }

    /// The layers of `Content-Encoding`, undone from the last one on
    /// (`phase 4 M3 design 9.4`).
    #[test]
    fn decoding_layers() {
        let body = Bytes::from_static(b"hello");
        assert_eq!(decode(&[], &body, REQUEST).expect("nothing to undo"), body);

        let gz = gzipped(b"hello");
        for name in ["gzip", "x-gzip", "GZIP"] {
            assert_eq!(
                decode(&[encoding(name)], &gz, REQUEST).expect("gzip"),
                body,
                "{name}"
            );
        }

        let mut members = Vec::new();
        members.extend_from_slice(&gzipped(b"one"));
        members.extend_from_slice(&gzipped(b"two"));
        let members = Bytes::from(members);
        assert_eq!(
            decode(&[encoding("gzip")], &members, REQUEST)
                .expect("both members")
                .as_ref(),
            b"onetwo"
        );

        let zlib = zlibbed(b"hello");
        assert_eq!(
            decode(&[encoding("deflate")], &zlib, REQUEST).expect("a zlib header"),
            body
        );
        let raw = deflated(b"hello");
        assert_eq!(
            decode(&[encoding("deflate")], &raw, REQUEST).expect("a bare deflate stream"),
            body
        );

        assert_eq!(
            decode(&[encoding("gzip, identity")], &gz, REQUEST).expect("identity is no layer"),
            body
        );

        let twice = gzipped(&zlibbed(b"hello"));
        assert_eq!(
            decode(&[encoding("deflate"), encoding("gzip")], &twice, REQUEST)
                .expect("two lines, undone from the last"),
            body
        );
    }

    /// A layer nobody supports, and one that does not decode, are reasons for
    /// a note (`phase 4 M3 design 9.4`, the phase 4 / M3b plan's P11).
    #[test]
    fn unsupported_and_broken_encodings() {
        let body = Bytes::from_static(b"hello");
        for name in ["br", "zstd", "compress"] {
            assert_eq!(
                decode(&[encoding(name)], &body, REQUEST),
                Err(format!("content-encoding {name} is not supported")),
                "{name}"
            );
        }
        assert_eq!(
            decode(&[encoding("a b")], &body, REQUEST),
            Err("the content-encoding is not supported".to_string()),
            "a name that is no token is not written down"
        );
        assert_eq!(
            decode(
                &[encoding("gzip")],
                &Bytes::from_static(b"\0\x01 rubbish"),
                REQUEST
            ),
            Err("the body cannot be decoded as gzip".to_string())
        );
    }

    /// A layer that decodes to far more than the limit stops at the limit and
    /// is not buffered (`phase 4 M3 design 9.3`, `9.4`).
    #[test]
    fn a_decompression_bomb_stops_at_the_limit() {
        let bomb = gzipped(&vec![0u8; 1024 * 1024]);
        let limit = Limit {
            bytes: 1000,
            text: "1000 B",
        };
        assert_eq!(
            decode(&[encoding("gzip")], &bomb, limit),
            Err("the decoded body is over 1000 B".to_string())
        );
    }

    /// An encoding name as a header value.
    fn encoding(name: &str) -> HeaderValue {
        HeaderValue::from_str(name).expect("an ASCII name")
    }

    /// `data` gzipped, as one member.
    fn gzipped(data: &[u8]) -> Bytes {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("in memory");
        Bytes::from(encoder.finish().expect("in memory"))
    }

    /// `data` in a zlib stream.
    fn zlibbed(data: &[u8]) -> Bytes {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("in memory");
        Bytes::from(encoder.finish().expect("in memory"))
    }

    /// `data` as a bare deflate stream.
    fn deflated(data: &[u8]) -> Bytes {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("in memory");
        Bytes::from(encoder.finish().expect("in memory"))
    }
}
