//! The TCP side of the responder (phase 3 M3 design 11.4): a connection of an
//! app to port 53 of the responder, with the queries on it.

use super::{Ask, Responder};
use crate::tcp::TunStream;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

/// Serves DNS over TCP on a connection to the responder: a two-byte length,
/// then the message, answered in order (never truncated); closes after `idle`
/// without a new query, when the app closes, or at a message that gets no
/// answer at all.
///
/// A query is answered, and the answer written, before the next is read, so
/// queries that the app sends without waiting for the answers are answered in
/// the order they came. The time an answer takes to come (a query that is
/// forwarded can take seconds) is no part of `idle`; the time the app takes to
/// read it is: an app that reads nothing for `idle` has its connection reset.
/// A message of length 0 gets no answer, as a message that is no query does
/// (`Responder::answer` says `None`), and so does a query a `REJECT-DROP` rule
/// refuses.
///
/// The close is `TunStream::close_and_wait_for_the_app` with `idle` as its
/// limit: a FIN, and up to `idle` for the app to close its side too, before the
/// stream is let go (that method says why it is not let go at once).
pub(crate) async fn serve_dns(
    mut stream: TunStream,
    responder: Arc<Responder>,
    ask: Ask,
    idle: Duration,
) {
    while let Some(query) = next_query(&mut stream, idle).await {
        let Some(reply) = responder.answer(&query, &ask).await else {
            break;
        };
        // the responder answers SERVFAIL, not what a length cannot frame
        let Ok(length) = u16::try_from(reply.len()) else {
            debug_assert!(false, "an answer over TCP is longer than a length frames");
            break;
        };
        let mut framed = Vec::with_capacity(2 + reply.len());
        framed.extend_from_slice(&length.to_be_bytes());
        framed.extend_from_slice(&reply);
        if !matches!(timeout(idle, stream.write_all(&framed)).await, Ok(Ok(()))) {
            // the app is gone, or reads nothing: there is no closing gently
            return;
        }
    }
    stream.close_and_wait_for_the_app(idle).await;
}

/// The next query on the connection: a two-byte length, then that many bytes.
/// `None` when there is none to answer: the app closed its side (or the
/// connection failed) before the whole of it came, it said a length of 0, or a
/// whole query did not come in `idle`, counted from the last answer (from the
/// handshake for the first).
async fn next_query(stream: &mut TunStream, idle: Duration) -> Option<Vec<u8>> {
    let read = async {
        let mut length = [0u8; 2];
        stream.read_exact(&mut length).await.ok()?;
        let length = u16::from_be_bytes(length);
        if length == 0 {
            return None;
        }
        // The buffer grows with the bytes that come, not with the length the
        // app says: an app that says 65,535 and sends nothing holds nothing but
        // its connection, whatever the number of connections.
        let mut message = Vec::new();
        let got = stream
            .take(u64::from(length))
            .read_to_end(&mut message)
            .await
            .ok()?;
        (got == usize::from(length)).then_some(message)
    };
    timeout(idle, read).await.ok().flatten()
}
