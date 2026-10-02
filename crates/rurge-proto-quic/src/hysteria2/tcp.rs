//! TCP over Hysteria 2 (Hysteria 2 protocol, "TCP"; phase 2 M7 design
//! 5.3): every connection a bidirectional stream of its own on the policy's
//! QUIC connection.
//!
//! - The request goes out at once: `varint 0x401 ‖ varint length ‖
//!   host:port ‖ varint length ‖ padding`.
//! - Nothing of the caller's goes before the server's answer (the
//!   reference client's default; no fast open): `u8 status ‖ varint length
//!   ‖ message ‖ varint length ‖ padding`. Status 0 lets the stream
//!   through; any other is the server's refusal, its message quoted. A
//!   length past the reference's bounds makes the answer malformed.

use super::auth::Authenticated;
use super::{address, padding};
use crate::session::{Failure, Lease};
use crate::varint;
use quinn::{ReadError, ReadExactError, RecvStream, SendStream, WriteError};
use rurge_net::connector::Target;
use rurge_net::quic::connection_error;
use rurge_proto::OutboundError;
use rurge_proto::outbound::untrusted_text;
use std::io;
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The frame type that starts a TCP request.
const TCP_REQUEST: u64 = 0x401;
/// The reference's bounds (`core/internal/protocol/proxy.go`): the longest
/// message and padding of an answer.
const MAX_MESSAGE: u64 = 2048;
const MAX_PADDING: u64 = 4096;
/// The reference's padding of a TCP request.
const REQUEST_PADDING: RangeInclusive<usize> = 64..=511;
/// How much of the server's message an error text quotes.
const MESSAGE_MAX: usize = 200;

/// The TCP request for `target`: refused, before anything is dialed, for a
/// name that cannot be sent.
pub(super) fn request(target: &Target) -> Result<Vec<u8>, OutboundError> {
    let Some(address) = address(target) else {
        return Err(proxy("the host name cannot be sent to the server"));
    };
    let padding = padding(REQUEST_PADDING);
    let mut request = Vec::with_capacity(2 + 2 + address.len() + 2 + padding.len());
    varint::put(&mut request, TCP_REQUEST);
    varint::put(&mut request, address.len() as u64);
    request.extend_from_slice(address.as_bytes());
    varint::put(&mut request, padding.len() as u64);
    request.extend_from_slice(&padding);
    Ok(request)
}

/// The tunnel `request` asks for, on the connection of `lease`: the request
/// on a stream of its own, then the server's answer. A connection found
/// gone before the answer came is `Lost` — nothing of the caller's went
/// out yet; the stream handed back is the caller's.
pub(super) async fn open(lease: Lease<Authenticated>, request: &[u8]) -> Result<Tunnel, Failure> {
    let (mut send, mut recv) = lease
        .connection()
        .open_bi()
        .await
        .map_err(|e| Failure::lost(&e))?;
    send.write_all(request).await.map_err(|e| match e {
        WriteError::ConnectionLost(e) => Failure::lost(&e),
        e => Failure::Failed(write_error(e).into()),
    })?;
    answer(&mut recv).await?;
    Ok(Tunnel {
        send,
        recv,
        _lease: lease,
    })
}

/// The server's answer: the stream goes through, or its refusal.
async fn answer(recv: &mut RecvStream) -> Result<(), Failure> {
    let mut status = [0u8];
    read(recv, &mut status).await?;
    let message = field(recv, MAX_MESSAGE).await?;
    // the padding, dropped
    field(recv, MAX_PADDING).await?;
    if status[0] == 0 {
        return Ok(());
    }
    // the server's text: cleaned and cut short
    let message = untrusted_text(&String::from_utf8_lossy(&message), MESSAGE_MAX);
    Err(Failure::Failed(if message.is_empty() {
        proxy("the server refused the connection")
    } else {
        proxy(&format!("the server refused the connection: {message}"))
    }))
}

/// A field of the answer, its length first: at most `max` bytes.
async fn field(recv: &mut RecvStream, max: u64) -> Result<Vec<u8>, Failure> {
    let mut integer = [0u8; 8];
    read(recv, &mut integer[..1]).await?;
    let n = varint::len(integer[0]);
    read(recv, &mut integer[1..n]).await?;
    let (len, _) = varint::get(&integer[..n]).expect("a whole integer");
    if len > max {
        return Err(Failure::Failed(proxy("the server's answer is malformed")));
    }
    let mut field = vec![0u8; len as usize];
    read(recv, &mut field).await?;
    Ok(field)
}

async fn read(recv: &mut RecvStream, buf: &mut [u8]) -> Result<(), Failure> {
    recv.read_exact(buf).await.map_err(|e| match e {
        ReadExactError::FinishedEarly(_) => {
            Failure::Failed(proxy("the server ended the stream without an answer"))
        }
        ReadExactError::ReadError(ReadError::ConnectionLost(e)) => Failure::lost(&e),
        ReadExactError::ReadError(e) => Failure::Failed(read_error(e).into()),
    })
}

fn proxy(text: &str) -> OutboundError {
    OutboundError::Proxy(format!("hysteria2: {text}"))
}

/// A TCP connection through the server: the request's stream, on a
/// connection its lease keeps. Shutdown sends the FIN the target then
/// sees; reading goes on. Dropped, the stream ends both ways (a FIN, and
/// STOP_SENDING for what is left unread), as the reference client closes
/// it. No `Debug`: nothing in it is worth printing.
pub(super) struct Tunnel {
    send: SendStream,
    recv: RecvStream,
    /// Last: the stream ends before its connection may be closed.
    _lease: Lease<Authenticated>,
}

impl AsyncRead for Tunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.get_mut()
            .recv
            .poll_read_buf(cx, buf)
            .map_err(read_error)
    }
}

impl AsyncWrite for Tunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().send)
            .poll_write(cx, buf)
            .map_err(write_error)
    }

    /// QUIC sends what the stream took.
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // fails only for a stream finished or reset already
        let _ = self.get_mut().send.finish();
        Poll::Ready(Ok(()))
    }
}

/// A failed read as an I/O error: the connection's end as QUIC tells it
/// (`quic: …`), the stream's as the protocol's.
fn read_error(e: ReadError) -> io::Error {
    match e {
        ReadError::ConnectionLost(e) => connection_error(&e),
        ReadError::Reset(code) => io::Error::new(
            io::ErrorKind::ConnectionReset,
            format!(
                "hysteria2: the server reset the stream (code {:#x})",
                code.into_inner()
            ),
        ),
        ReadError::ClosedStream | ReadError::IllegalOrderedRead | ReadError::ZeroRttRejected => {
            closed_stream()
        }
    }
}

fn write_error(e: WriteError) -> io::Error {
    match e {
        WriteError::ConnectionLost(e) => connection_error(&e),
        WriteError::Stopped(code) => io::Error::new(
            io::ErrorKind::ConnectionReset,
            format!(
                "hysteria2: the server stopped the stream (code {:#x})",
                code.into_inner()
            ),
        ),
        WriteError::ClosedStream | WriteError::ZeroRttRejected => closed_stream(),
    }
}

fn closed_stream() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "hysteria2: the stream is closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;

    /// The address and the padding of `request`.
    fn parts(request: &[u8]) -> (&[u8], &[u8]) {
        assert_eq!(&request[..2], [0x44, 0x01], "the type 0x401");
        let (len, n) = varint::get(&request[2..]).expect("the address length");
        let (address, rest) = request[2 + n..].split_at(len as usize);
        let (len, n) = varint::get(rest).expect("the padding length");
        assert_eq!(rest.len(), n + len as usize, "nothing after the padding");
        (address, &rest[n..])
    }

    #[test]
    fn a_request_is_its_type_the_address_and_padding() {
        for (host, address) in [
            ("192.0.2.1", "192.0.2.1:443"),
            ("2001:db8::1", "[2001:db8::1]:443"),
            ("bücher.example", "xn--bcher-kva.example:443"),
        ] {
            let target = Target::new(HostName::parse(host), 443);
            let request = request(&target).unwrap();
            let (sent, padding) = parts(&request);
            assert_eq!(sent, address.as_bytes());
            assert!(
                REQUEST_PADDING.contains(&padding.len()),
                "{}",
                padding.len()
            );
            assert!(padding.iter().all(u8::is_ascii_alphanumeric));
        }
        // a length of 64 or more takes two bytes
        let long = Target::new(HostName::Domain(format!("{}.test", "a".repeat(80))), 80);
        let request = request(&long).unwrap();
        assert_eq!(varint::get(&request[2..]), Some((88, 2)));
        assert_eq!(parts(&request).0.len(), 88);
    }

    #[test]
    fn an_address_must_be_one_the_server_reads() {
        // `name:80` of exactly the longest address the reference reads
        let longest = Target::new(HostName::Domain("a".repeat(2045)), 80);
        assert_eq!(parts(&request(&longest).unwrap()).0.len(), 2048);
        for target in [
            Target::new(HostName::Domain("a".repeat(2046)), 80),
            Target::new(HostName::Domain("x@blocked.test".into()), 80),
            Target::new(HostName::Domain(String::new()), 80),
        ] {
            let err = request(&target).unwrap_err();
            assert_eq!(
                err.to_string(),
                "hysteria2: the host name cannot be sent to the server"
            );
        }
    }
}
