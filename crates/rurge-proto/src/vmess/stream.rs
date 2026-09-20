//! The byte stream a VMess connection becomes once the request head is
//! queued: writes are sealed into chunks, reads open the response head and
//! then chunks.
//!
//! A write reports success only after its whole chunk has been handed to the
//! layer below, so the stream's own correctness never depends on anyone
//! calling `flush`. A chunk that could not be written in one go stays parked
//! (it is already sealed with its nonce) and is finished by the next write,
//! flush or shutdown.

use super::chunk::{ChunkCipher, MAX_PAYLOAD};
use super::header::{self, ResponseError, Security, Session, TAG};
use rurge_net::connector::BoxedStream;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

enum Reading {
    /// The 18 sealed bytes holding the response head's length.
    HeadLen {
        buf: [u8; 18],
        filled: usize,
    },
    Head {
        buf: Vec<u8>,
        filled: usize,
    },
    Len {
        buf: [u8; 2],
        filled: usize,
    },
    Body {
        buf: Vec<u8>,
        filled: usize,
    },
    Payload {
        buf: Vec<u8>,
        pos: usize,
        end: usize,
    },
    Eof,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Closing {
    Open,
    /// The end-of-stream chunk is sealed into `out`.
    Sealed,
    Done,
}

/// No `Debug`: it holds the connection's keys.
pub(crate) struct VmessStream {
    inner: BoxedStream,
    session: Session,
    response_key: [u8; 16],
    response_iv: [u8; 16],
    up: ChunkCipher,
    down: ChunkCipher,
    /// The chunk being written, how much of it is out, and how many payload
    /// bytes it carries.
    out: Vec<u8>,
    out_pos: usize,
    accepted: usize,
    closing: Closing,
    reading: Reading,
}

fn protocol(e: ResponseError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.text())
}

fn invalid(text: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

fn cut_short() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "vmess: the connection ended in the middle of a chunk",
    )
}

/// Fills `buf[*filled..]`. `Ok(false)`: the peer closed before the first byte.
fn poll_fill(
    inner: &mut BoxedStream,
    cx: &mut Context<'_>,
    buf: &mut [u8],
    filled: &mut usize,
) -> Poll<io::Result<bool>> {
    while *filled < buf.len() {
        let mut space = ReadBuf::new(&mut buf[*filled..]);
        ready!(Pin::new(&mut *inner).poll_read(cx, &mut space))?;
        let n = space.filled().len();
        if n == 0 {
            return Poll::Ready(if *filled == 0 {
                Ok(false)
            } else {
                Err(cut_short())
            });
        }
        *filled += n;
    }
    Poll::Ready(Ok(true))
}

impl VmessStream {
    /// `inner` already carries (or lazily owes) the sealed request head.
    pub(crate) fn new(inner: BoxedStream, session: Session, security: Security) -> VmessStream {
        let (response_key, response_iv) = header::response_secrets(&session);
        VmessStream {
            up: ChunkCipher::new(security, &session.body_key, &session.body_iv),
            down: ChunkCipher::new(security, &response_key, &response_iv),
            inner,
            session,
            response_key,
            response_iv,
            out: Vec::new(),
            out_pos: 0,
            accepted: 0,
            closing: Closing::Open,
            reading: Reading::HeadLen {
                buf: [0; 18],
                filled: 0,
            },
        }
    }

    fn poll_out(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.out_pos < self.out.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.out[self.out_pos..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.out_pos += n;
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for VmessStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            match &mut this.reading {
                Reading::HeadLen { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        // a wrong id, or clocks too far apart: the server
                        // just closes, and nothing tells the two apart
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "vmess: the server closed the connection without answering",
                        )));
                    }
                    let rest =
                        header::open_response_len(&this.response_key, &this.response_iv, *buf)
                            .map_err(protocol)?;
                    this.reading = Reading::Head {
                        buf: vec![0; rest],
                        filled: 0,
                    };
                }
                Reading::Head { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(cut_short()));
                    }
                    header::open_response(
                        &this.response_key,
                        &this.response_iv,
                        &this.session,
                        buf,
                    )
                    .map_err(protocol)?;
                    this.reading = Reading::Len {
                        buf: [0; 2],
                        filled: 0,
                    };
                }
                Reading::Len { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        // closed between chunks: the end, without the courtesy chunk
                        this.reading = Reading::Eof;
                        continue;
                    }
                    let len = this.down.open_len(*buf);
                    if len < TAG {
                        return Poll::Ready(Err(invalid("vmess: a chunk shorter than its tag")));
                    }
                    this.reading = Reading::Body {
                        buf: vec![0; len],
                        filled: 0,
                    };
                }
                Reading::Body { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(cut_short()));
                    }
                    let Some(end) = this.down.open(buf) else {
                        return Poll::Ready(Err(invalid("vmess: a chunk cannot be authenticated")));
                    };
                    this.reading = if end == 0 {
                        Reading::Eof
                    } else {
                        Reading::Payload {
                            buf: std::mem::take(buf),
                            pos: 0,
                            end,
                        }
                    };
                }
                Reading::Payload { buf, pos, end } => {
                    let n = out.remaining().min(*end - *pos);
                    out.put_slice(&buf[*pos..*pos + n]);
                    *pos += n;
                    if pos == end {
                        this.reading = Reading::Len {
                            buf: [0; 2],
                            filled: 0,
                        };
                    }
                    return Poll::Ready(Ok(()));
                }
                Reading::Eof => return Poll::Ready(Ok(())),
            }
        }
    }
}

impl AsyncWrite for VmessStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.closing != Closing::Open {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if data.is_empty() {
            // an empty chunk would tell the server the stream is over
            return Poll::Ready(Ok(0));
        }
        if this.out_pos == this.out.len() {
            let n = data.len().min(MAX_PAYLOAD);
            this.out.clear();
            this.out_pos = 0;
            this.up.seal(&data[..n], &mut this.out);
            this.accepted = n;
        }
        ready!(this.poll_out(cx))?;
        Poll::Ready(Ok(this.accepted.min(data.len())))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_out(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.closing == Closing::Open {
            // a parked chunk first: it already consumed its nonce
            ready!(this.poll_out(cx))?;
            this.out.clear();
            this.out_pos = 0;
            this.up.seal(&[], &mut this.out);
            this.closing = Closing::Sealed;
        }
        if this.closing == Closing::Sealed {
            ready!(this.poll_out(cx))?;
            this.closing = Closing::Done;
        }
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}
