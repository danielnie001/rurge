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
/// The close is a `shutdown`, a FIN behind what was written, and then up to
/// `idle` for the app to close its side too, in which time what it sends is
/// read and thrown away. The stream is not dropped right after its `shutdown`:
/// the drop of a stream resets a connection whose app has not closed its side
/// yet (`ShardState::stream_dropped`), and the reset would take with it what is
/// still to be sent, the last answer and the FIN. Dropped after the app's own
/// FIN the connection ends well, and lingers until the app has acknowledged
/// ours; dropped after `idle` with the app still not closed it is reset, as a
/// connection of the engine's is after REJECT-DROP.
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
    close(stream, idle).await;
}

/// The next query on the connection: a two-byte length, then that many bytes.
/// `None` when there is none to answer: the app closed its side (or the
/// connection failed), it said a length of 0, or a whole query did not come in
/// `idle`, counted from the last answer (from the handshake for the first).
async fn next_query(stream: &mut TunStream, idle: Duration) -> Option<Vec<u8>> {
    let read = async {
        let mut length = [0u8; 2];
        stream.read_exact(&mut length).await.ok()?;
        let length = usize::from(u16::from_be_bytes(length));
        if length == 0 {
            return None;
        }
        let mut message = vec![0u8; length];
        stream.read_exact(&mut message).await.ok()?;
        Some(message)
    };
    timeout(idle, read).await.ok().flatten()
}

/// Closes the connection with a FIN, and gives the app `idle` to close its side
/// too before the stream is let go (see `serve_dns`).
async fn close(mut stream: TunStream, idle: Duration) {
    if stream.shutdown().await.is_err() {
        return;
    }
    // what the app sends meanwhile is thrown away as it is read
    let mut scrap = [0u8; 512];
    let _ = timeout(idle, async {
        loop {
            match stream.read(&mut scrap).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
}
