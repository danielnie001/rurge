//! The session of a TCP connection through the TUN (phase 3 M3 design 5.6):
//! once the handshake with the app is over, the `Dialer` dials where the app
//! connected to, and relays between the two. A rejection is answered with a
//! reset, except REJECT-DROP, which is answered with nothing for a while. A
//! stop cancels the session of a relay and ends the hold of REJECT-DROP.

use crate::tcp::{TunStream, Tuple};
use crate::{Life, TunOptions};
use rurge_config::{HostName, ListenerKind, SessionInfo};
use rurge_inbound::{DialError, Dialer};
use rurge_proto::RejectKind;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How long a session keeps a connection it neither relays nor resets.
#[derive(Clone, Copy)]
pub(crate) struct Holds {
    /// REJECT-DROP: how long the connection is held, read and written by
    /// nobody, before the TUN closes its side.
    pub(crate) drop_hold: Duration,
    /// REJECT-DROP: after that, the longest the TUN waits for the app to close
    /// its side, reading and discarding what it sends.
    pub(crate) linger: Duration,
}

impl From<&TunOptions> for Holds {
    fn from(options: &TunOptions) -> Holds {
        Holds {
            drop_hold: options.drop_hold,
            linger: options.linger,
        }
    }
}

pub(crate) async fn serve(
    stream: TunStream,
    dialer: Arc<dyn Dialer>,
    tuple: Tuple,
    holds: Holds,
    life: Arc<Life>,
) {
    if stream.established().await.is_err() {
        return;
    }
    let session = SessionInfo {
        src: tuple.src,
        in_port: 0,
        local: Some(tuple.dst),
        listener: ListenerKind::Tun,
        ..SessionInfo::tcp(HostName::Ip(tuple.dst.ip()), tuple.dst.port())
    };
    // The dial is not raced with the stop: it runs in the dialer, which holds
    // the session's handle, and a dial dropped half way would leave the record
    // of the session open for good. It ends within its own time limits.
    match dialer.dial(session).await {
        Ok(dialed) => {
            let token = dialed.handle.token().clone();
            let mut relay = dialer.relay(Box::new(stream), dialed.stream, dialed.handle);
            // The stop gives the connection a reset, which reaches a relay that
            // reads or writes its stream. One that waits on its upstream in
            // both directions touches the stream in neither, so the stop
            // cancels the session as well, which a relay ends for whatever it
            // waits on. The relay is then waited for to its end, not dropped:
            // it is what finishes the session's handle, and a handle left
            // unfinished leaves the session's record open.
            let stopped = tokio::select! {
                biased;
                _ = life.stopped() => true,
                _ = &mut relay => false,
            };
            if stopped {
                token.cancel();
                relay.await;
            }
        }
        // Boxed: the future of this rare path (two timers and a buffer of 512
        // bytes) would make the future of every session larger, by some 700
        // bytes (in a debug build).
        Err(DialError::Reject {
            kind: RejectKind::Drop,
            ..
        }) => Box::pin(hold_in_silence(stream, holds, &life)).await,
        // any other rejection and a failed dial let the stream go, which
        // resets the connection
        Err(_) => {}
    }
}

/// REJECT-DROP (design 5.6): the connection is held for `drop_hold` with
/// nobody reading or writing it, as the HTTP listener holds the socket of a
/// request it drops, and then the TUN closes its side, a FIN. A stop ends the
/// hold at once, and so does the end of the connection: once the shard has let
/// it go (the app reset it, say) there is nothing left to hold, and the session
/// does not stay for the rest of `drop_hold` with its future and its stream.
///
/// The stream cannot be let go right after its `shutdown`: the drop of a stream
/// resets a connection the app may still send on
/// (`ShardState::stream_dropped`), so the FIN would be turned into a reset
/// before the app read it. So the TUN reads what the app sends, and throws it
/// away, until the app's own FIN, for `linger` at the most. Then it lets the
/// stream go: that ends well if the app's FIN came, and resets the connection
/// if it did not. A stop during the wait resets the connection, which ends the
/// read with an error.
async fn hold_in_silence(mut stream: TunStream, holds: Holds, life: &Life) {
    tokio::select! {
        biased;
        _ = life.stopped() => return,
        _ = stream.closed() => return,
        _ = tokio::time::sleep(holds.drop_hold) => {}
    }
    if stream.shutdown().await.is_err() {
        return;
    }
    // What the app sends is thrown away as it is read, a few bytes at a time:
    // the future of this path is boxed, so the array is held only by the
    // sessions that come this way.
    let mut scrap = [0u8; 512];
    let _ = tokio::time::timeout(holds.linger, async {
        loop {
            match stream.read(&mut scrap).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
}
