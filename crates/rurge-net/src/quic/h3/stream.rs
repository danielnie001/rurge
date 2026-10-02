//! A request's stream (phase 2 M7 design 4): the request's content goes
//! out as DATA frames, the response head comes back, then the response's
//! content as the stream's bytes. A CONNECT's tunnel is the same stream.
//!
//! - A write takes at most 64 KiB, as one DATA frame, and takes nothing
//!   until the stream takes the frame's first byte. The part of the frame
//!   the stream could not take at once is finished — by the next write,
//!   flush or shutdown — before anything new is taken: a write future
//!   dropped (a timeout) never leaves a frame cut short, and never fails
//!   the connection.
//! - Shutdown finishes what was begun and sends the FIN; reading goes on.
//! - Reads hand over the DATA payloads as they come; QUIC's flow control
//!   holds the server back. Interim responses, frames of unknown types and
//!   the trailers are passed over.
//! - A stream dropped before both directions ended cancels its request:
//!   the sending part is reset and the receiving part stopped, with
//!   H3_REQUEST_CANCELLED (RFC 9114 4.1.1).

use super::client::Inner;
use super::frame::{self, FrameReader, ReadFailure};
use super::qpack::{self, Field, Refused};
use super::{
    H3_EXCESSIVE_LOAD, H3_FRAME_ERROR, H3_FRAME_UNEXPECTED, H3_ID_ERROR, H3_MESSAGE_ERROR,
    H3_REQUEST_CANCELLED, H3Error, MAX_FIELD_SECTION_SIZE, QPACK_DECOMPRESSION_FAILED,
};
use bytes::{Buf, Bytes, BytesMut};
use http::header::{HeaderName, HeaderValue};
use http::{Response, StatusCode};
use quinn::{RecvStream, SendStream, VarInt};
use std::fmt;
use std::future::poll_fn;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The most one write takes: one DATA frame.
const MAX_DATA_FRAME: usize = 64 << 10;

/// The fields of HTTP/1.1's connections, which HTTP/3 has no place for
/// (RFC 9114 4.2).
const CONNECTION_FIELDS: [&[u8]; 5] = [
    b"connection",
    b"keep-alive",
    b"proxy-connection",
    b"transfer-encoding",
    b"upgrade",
];

/// One request on its own bidirectional stream: write the content, shut
/// down, read the response head (`response`), then the response's content.
pub struct RequestStream {
    inner: Arc<Inner>,
    send: SendStream,
    /// The rest of a frame begun: out before anything new is taken.
    pending: Bytes,
    /// The FIN went out.
    finished: bool,
    reader: FrameReader,
    recv: Recv,
}

/// Where reading the response is.
enum Recv {
    /// Before the final response head, between frames.
    Head,
    /// In the payload of a HEADERS frame before the final head.
    HeadFields(usize),
    /// After the head, between frames.
    Body,
    /// In a DATA frame: what is left of it.
    Data(u64),
    /// After the trailers, between frames.
    Trailers,
    /// Passing over the rest of a frame, then back between frames.
    Skip { left: u64, then: Between },
    /// The server's side ended after its content.
    End,
    /// Failed: reads fail again, the same way.
    Failed(io::ErrorKind, Arc<str>),
}

/// The places between frames that passing over a frame goes back to.
#[derive(Clone, Copy)]
enum Between {
    Head,
    Body,
    Trailers,
}

impl From<Between> for Recv {
    fn from(between: Between) -> Recv {
        match between {
            Between::Head => Recv::Head,
            Between::Body => Recv::Body,
            Between::Trailers => Recv::Trailers,
        }
    }
}

impl RequestStream {
    /// A request whose head — a whole HEADERS frame — goes out first.
    pub(super) fn new(
        inner: Arc<Inner>,
        send: SendStream,
        recv: RecvStream,
        head: Bytes,
    ) -> RequestStream {
        RequestStream {
            inner,
            send,
            pending: head,
            finished: false,
            reader: FrameReader::new(recv),
            recv: Recv::Head,
        }
    }

    /// The response head; interim (1xx) responses and frames of unknown
    /// types are passed over. A head that is malformed or larger than
    /// 64 KiB ends the stream (H3_MESSAGE_ERROR, H3_EXCESSIVE_LOAD); a
    /// field section that cannot be decoded, or a frame where none may be,
    /// closes the connection.
    pub async fn response(&mut self) -> io::Result<Response<()>> {
        poll_fn(|cx| self.poll_response(cx)).await
    }

    fn poll_response(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<Response<()>>> {
        loop {
            match self.recv {
                Recv::Head => {
                    let header = match ready!(self.reader.poll_header(cx)) {
                        Ok(Some(header)) => header,
                        Ok(None) => {
                            return Poll::Ready(Err(self.fail(
                                io::ErrorKind::UnexpectedEof,
                                "h3: the server ended the stream without a response".to_string(),
                            )));
                        }
                        Err(failure) => return Poll::Ready(Err(self.read_failure(failure))),
                    };
                    match header.ty {
                        frame::HEADERS if header.len > MAX_FIELD_SECTION_SIZE => {
                            return Poll::Ready(Err(self.too_large()));
                        }
                        frame::HEADERS => self.recv = Recv::HeadFields(header.len as usize),
                        frame::DATA => {
                            return Poll::Ready(Err(self.violation(H3Error::new(
                                H3_FRAME_UNEXPECTED,
                                "the server sent DATA before the response head",
                            ))));
                        }
                        ty => self.pass_over(ty, header.len, Between::Head)?,
                    }
                }
                Recv::HeadFields(len) => {
                    let payload = match ready!(self.reader.poll_payload(cx, len)) {
                        Ok(payload) => payload,
                        Err(failure) => return Poll::Ready(Err(self.read_failure(failure))),
                    };
                    let head = match qpack::decode(&payload, MAX_FIELD_SECTION_SIZE) {
                        Ok(fields) => response_head(fields),
                        Err(Refused::Invalid) => {
                            return Poll::Ready(Err(self.violation(H3Error::new(
                                QPACK_DECOMPRESSION_FAILED,
                                "the server's field section cannot be decoded",
                            ))));
                        }
                        Err(Refused::TooLarge) => return Poll::Ready(Err(self.too_large())),
                    };
                    match head {
                        None => {
                            return Poll::Ready(Err(self.stream_error(
                                H3_MESSAGE_ERROR,
                                "h3: the server sent a malformed response".to_string(),
                            )));
                        }
                        Some(head) if head.status().is_informational() => self.recv = Recv::Head,
                        Some(head) => {
                            self.recv = Recv::Body;
                            return Poll::Ready(Ok(head));
                        }
                    }
                }
                Recv::Skip { .. } => ready!(self.poll_skip(cx))?,
                Recv::Failed(kind, ref text) => {
                    return Poll::Ready(Err(io::Error::new(kind, text.to_string())));
                }
                Recv::Body | Recv::Data(_) | Recv::Trailers | Recv::End => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "h3: the response head was read already",
                    )));
                }
            }
        }
    }

    /// A frame neither DATA nor HEADERS: a frame of another stream is an
    /// error of the connection, one of an unknown type is passed over (RFC
    /// 9114 7.2, 9).
    fn pass_over(&mut self, ty: u64, len: u64, then: Between) -> io::Result<()> {
        match ty {
            // we sent no MAX_PUSH_ID: no push ID is allowed (RFC 9114 7.2.5)
            frame::PUSH_PROMISE => {
                Err(self.violation(H3Error::new(H3_ID_ERROR, "the server promised a push")))
            }
            frame::CANCEL_PUSH | frame::SETTINGS | frame::GOAWAY | frame::MAX_PUSH_ID => {
                Err(self.unexpected(ty))
            }
            ty if frame::is_http2_only(ty) => Err(self.unexpected(ty)),
            _ => {
                self.recv = Recv::Skip { left: len, then };
                Ok(())
            }
        }
    }

    /// Passes over the rest of a frame, then goes back between frames.
    fn poll_skip(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let Recv::Skip { left, then } = &mut self.recv else {
            return Poll::Ready(Ok(()));
        };
        let then = *then;
        match ready!(self.reader.poll_skip(cx, left)) {
            Ok(()) => {
                self.recv = then.into();
                Poll::Ready(Ok(()))
            }
            Err(failure) => Poll::Ready(Err(self.read_failure(failure))),
        }
    }

    fn unexpected(&mut self, ty: u64) -> io::Error {
        self.violation(H3Error::new(
            H3_FRAME_UNEXPECTED,
            format!("the server sent a frame of type {ty:#x} on a request stream"),
        ))
    }

    fn too_large(&mut self) -> io::Error {
        self.stream_error(
            H3_EXCESSIVE_LOAD,
            format!("h3: the response head is larger than {MAX_FIELD_SECTION_SIZE} bytes"),
        )
    }

    /// An error of the connection (RFC 9114 8): it closes with `error`,
    /// and the stream fails with what it says.
    fn violation(&mut self, error: H3Error) -> io::Error {
        let e = error.io();
        self.inner.fail(error);
        self.fail(e.kind(), e.to_string())
    }

    /// An error of this stream alone: both directions end with `code`.
    fn stream_error(&mut self, code: VarInt, text: String) -> io::Error {
        // fails only for a part that ended already
        let _ = self.send.reset(code);
        self.reader.stop(code);
        self.fail(io::ErrorKind::InvalidData, text)
    }

    fn read_failure(&mut self, failure: ReadFailure) -> io::Error {
        match failure {
            ReadFailure::Truncated => self.violation(H3Error::new(
                H3_FRAME_ERROR,
                "the server's stream ended within a frame",
            )),
            ReadFailure::Stream(e) => {
                let e = self.inner.read_error(e);
                self.fail(e.kind(), e.to_string())
            }
        }
    }

    /// Reading failed with `text`: it fails so from now on.
    fn fail(&mut self, kind: io::ErrorKind, text: String) -> io::Error {
        let error = io::Error::new(kind, text.clone());
        self.recv = Recv::Failed(kind, text.into());
        error
    }

    /// What is left of a frame begun goes out.
    fn poll_pending(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while !self.pending.is_empty() {
            match ready!(Pin::new(&mut self.send).poll_write(cx, &self.pending)) {
                Ok(n) => self.pending.advance(n),
                Err(e) => return Poll::Ready(Err(self.inner.write_error(e))),
            }
        }
        Poll::Ready(Ok(()))
    }
}

/// The response `fields` make; `None` for a malformed one (RFC 9114 4.1.2):
/// `:status` must come first and alone of the pseudo-header fields, a
/// status from 100 to 599; names must be tokens in lowercase and values
/// field content (RFC 9110 5.1, 5.5); none of HTTP/1.1's connection fields
/// may come (RFC 9114 4.2).
fn response_head(fields: Vec<Field>) -> Option<Response<()>> {
    let mut fields = fields.into_iter();
    let (name, status) = fields.next()?;
    if name != b":status" {
        return None;
    }
    let status = StatusCode::from_bytes(&status)
        .ok()
        .filter(|status| status.as_u16() < 600)?;
    let mut response = Response::new(());
    *response.status_mut() = status;
    for (name, value) in fields {
        // a pseudo-header field among them is no token
        if !is_field_name(&name)
            || !is_field_value(&value)
            || CONNECTION_FIELDS.contains(&&name[..])
        {
            return None;
        }
        response.headers_mut().append(
            HeaderName::from_bytes(&name).ok()?,
            HeaderValue::from_bytes(&value).ok()?,
        );
    }
    Some(response)
}

/// A token in lowercase (RFC 9110 5.1, 5.6.2).
fn is_field_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.iter().all(|&b| {
            matches!(b, b'a'..=b'z' | b'0'..=b'9'
                | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.'
                | b'^' | b'_' | b'`' | b'|' | b'~')
        })
}

/// Field content (RFC 9110 5.5): visible characters and those past ASCII,
/// with spaces and tabs between them but not around them.
fn is_field_value(value: &[u8]) -> bool {
    let blank = |b: Option<&u8>| matches!(b, Some(b' ' | b'\t'));
    value
        .iter()
        .all(|&b| b == b' ' || b == b'\t' || (b >= 0x21 && b != 0x7f))
        && !blank(value.first())
        && !blank(value.last())
}

impl AsyncRead for RequestStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            match this.recv {
                Recv::Body | Recv::Trailers => {
                    let after_trailers = matches!(this.recv, Recv::Trailers);
                    let header = match ready!(this.reader.poll_header(cx)) {
                        Ok(Some(header)) => header,
                        Ok(None) => {
                            this.recv = Recv::End;
                            return Poll::Ready(Ok(()));
                        }
                        Err(failure) => return Poll::Ready(Err(this.read_failure(failure))),
                    };
                    match header.ty {
                        frame::DATA | frame::HEADERS if after_trailers => {
                            return Poll::Ready(Err(this.violation(H3Error::new(
                                H3_FRAME_UNEXPECTED,
                                "the server sent a frame after the trailers",
                            ))));
                        }
                        frame::DATA if header.len > 0 => this.recv = Recv::Data(header.len),
                        frame::DATA => {}
                        // the trailers: ignored
                        frame::HEADERS => {
                            this.recv = Recv::Skip {
                                left: header.len,
                                then: Between::Trailers,
                            };
                        }
                        ty => {
                            let then = if after_trailers {
                                Between::Trailers
                            } else {
                                Between::Body
                            };
                            this.pass_over(ty, header.len, then)?;
                        }
                    }
                }
                Recv::Data(mut left) => {
                    let read = ready!(this.reader.poll_data(cx, buf, &mut left));
                    this.recv = if left == 0 {
                        Recv::Body
                    } else {
                        Recv::Data(left)
                    };
                    return Poll::Ready(read.map_err(|failure| this.read_failure(failure)));
                }
                Recv::Skip { .. } => ready!(this.poll_skip(cx))?,
                Recv::End => return Poll::Ready(Ok(())),
                Recv::Failed(kind, ref text) => {
                    return Poll::Ready(Err(io::Error::new(kind, text.to_string())));
                }
                Recv::Head | Recv::HeadFields(_) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "h3: the response head has not been read",
                    )));
                }
            }
        }
    }
}

impl AsyncWrite for RequestStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "h3: write after shutdown",
            )));
        }
        ready!(this.poll_pending(cx))?;
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let payload = &buf[..buf.len().min(MAX_DATA_FRAME)];
        let (header, len) = frame::data_header(payload.len());
        let header = &header[..len];
        // nothing is taken until the stream takes the frame's first byte
        let sent = match ready!(Pin::new(&mut this.send).poll_write(cx, header)) {
            Ok(sent) => sent,
            Err(e) => return Poll::Ready(Err(this.inner.write_error(e))),
        };
        if sent < header.len() {
            let mut rest = BytesMut::with_capacity(header.len() - sent + payload.len());
            rest.extend_from_slice(&header[sent..]);
            rest.extend_from_slice(payload);
            this.pending = rest.freeze();
            return Poll::Ready(Ok(payload.len()));
        }
        // the frame has begun: its payload follows, now or from `pending`
        let sent = match Pin::new(&mut this.send).poll_write(cx, payload) {
            Poll::Ready(Ok(sent)) => sent,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(this.inner.write_error(e))),
            Poll::Pending => 0,
        };
        if sent < payload.len() {
            this.pending = Bytes::copy_from_slice(&payload[sent..]);
        }
        Poll::Ready(Ok(payload.len()))
    }

    /// What was taken goes to the stream; QUIC sends it from there.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().poll_pending(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.finished {
            ready!(this.poll_pending(cx))?;
            // fails only for a stream reset already, which reads report
            let _ = this.send.finish();
            this.finished = true;
        }
        Poll::Ready(Ok(()))
    }
}

impl Drop for RequestStream {
    fn drop(&mut self) {
        // a request given up on ends abruptly both ways (RFC 9114 4.1.1);
        // a frame begun is never cut short by a FIN
        if !self.finished {
            let _ = self.send.reset(H3_REQUEST_CANCELLED);
        }
        if !matches!(self.recv, Recv::End) {
            self.reader.stop(H3_REQUEST_CANCELLED);
        }
    }
}

impl fmt::Debug for RequestStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestStream")
            .field("id", &self.send.id())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(fields: &[(&str, &str)]) -> Option<Response<()>> {
        response_head(
            fields
                .iter()
                .map(|(n, v)| (n.as_bytes().to_vec(), v.as_bytes().to_vec()))
                .collect(),
        )
    }

    #[test]
    fn a_response_head_is_its_status_and_fields() {
        let response = head(&[
            (":status", "233"),
            ("hysteria-udp", "true"),
            ("x", "inner  spaces\tand\u{e9}"),
            ("x", ""),
        ])
        .unwrap();
        assert_eq!(response.status(), 233);
        assert_eq!(response.headers()["hysteria-udp"], "true");
        assert_eq!(response.headers().get_all("x").iter().count(), 2);
    }

    #[test]
    fn malformed_response_heads_are_refused() {
        for fields in [
            &[][..],
            &[("content-type", "text/plain")],
            &[(":status", "200"), (":status", "200")],
            &[("x", "1"), (":status", "200")],
            &[(":status", "200"), (":path", "/")],
            &[(":status", "2000")],
            &[(":status", "099")],
            &[(":status", "600")],
            &[(":status", "20a")],
            &[(":status", "200"), ("X-Upper", "1")],
            &[(":status", "200"), ("a name", "1")],
            &[(":status", "200"), ("", "1")],
            &[(":status", "200"), ("connection", "close")],
            &[(":status", "200"), ("transfer-encoding", "chunked")],
            &[(":status", "200"), ("x", " leading")],
            &[(":status", "200"), ("x", "trailing\t")],
            &[(":status", "200"), ("x", "a\r\nb")],
            &[(":status", "200"), ("x", "a\0b")],
        ] {
            assert!(head(fields).is_none(), "{fields:?}");
        }
    }
}
