//! The heartbeat (v5 SPEC, "Heartbeat"; v4's protocol README; phase 2 M7
//! design 6.2, 6.3): while anything is relayed on a connection — a TCP
//! tunnel, a UDP association — a `VER 04` command every few seconds keeps
//! the server from taking the connection for idle.
//!
//! - v5: a QUIC datagram every 3 seconds, as tuic-client 1.0.0 sends it:
//!   tuic-server's idle timeout is 10 seconds, which QUIC's own keep-alive
//!   every 10 seconds (design 3.2) does not beat. The SPEC sends it as a
//!   datagram only: a server that takes no QUIC datagrams gets none
//!   (sing-box ends a connection on a heartbeat in a stream: an unknown
//!   command there), and QUIC's keep-alive alone keeps the connection up —
//!   against a server whose idle timeout is longer than 10 seconds
//!   (sing-box's: quic-go's 30).
//! - v4: a unidirectional stream of its own every 10 seconds, as tuic
//!   0.8.5's client sends it — its server takes a heartbeat on a stream
//!   only, and closes the connection on a datagram that is no packet —,
//!   whether the server takes QUIC datagrams or not.
//! - Nothing goes while nothing is relayed: an idle connection is let go of
//!   — by the server, or by the session (`IDLE_TIMEOUT`).

use super::Version;
use bytes::Bytes;
use quinn::{Connection, ConnectionError, SendDatagramError};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio_util::task::AbortOnDropHandle;

/// The command's type.
pub(super) const HEARTBEAT: u8 = 0x04;

/// How often, while something is relayed: v5's as tuic-client 1.0.0's,
/// v4's as tuic 0.8.5's client's.
pub(super) fn every(version: Version) -> Duration {
    match version {
        Version::V4 => Duration::from_secs(10),
        Version::V5 => Duration::from_secs(3),
    }
}

/// The relays under way on a connection — its TCP tunnels and the UDP
/// associations on it —, which its heartbeat keeps it up for.
pub(super) struct Relays(watch::Sender<usize>);

impl Relays {
    pub(super) fn new() -> Arc<Relays> {
        Arc::new(Relays(watch::Sender::new(0)))
    }

    /// One more relay under way, until the guard goes. Only the first wakes
    /// the heartbeat.
    pub(super) fn begin(self: &Arc<Self>) -> Relaying {
        self.0.send_if_modified(|relays| {
            *relays += 1;
            *relays == 1
        });
        Relaying(self.clone())
    }

    /// Test support: how many are under way.
    #[cfg(test)]
    pub(super) fn count(&self) -> usize {
        *self.0.borrow()
    }
}

/// A relay under way on a connection, counted while it lives.
pub(super) struct Relaying(Arc<Relays>);

impl Drop for Relaying {
    /// The heartbeat finds out when it is next due: nothing wakes it.
    fn drop(&mut self) {
        self.0.0.send_if_modified(|relays| {
            *relays -= 1;
            false
        });
    }
}

/// Sends `version`'s heartbeat on `connection` every `every` while `relays`
/// has any under way, until the connection ends — or the handle, or
/// `relays`, goes. None for v5 on a connection without QUIC datagrams.
pub(super) fn start(
    connection: &Connection,
    relays: &Relays,
    version: Version,
    every: Duration,
) -> Option<AbortOnDropHandle<()>> {
    if version == Version::V5 && connection.max_datagram_size().is_none() {
        return None;
    }
    let connection = connection.clone();
    let relays = relays.0.subscribe();
    Some(AbortOnDropHandle::new(tokio::spawn(async move {
        tokio::select! {
            _ = connection.closed() => {}
            () = beat(&connection, relays, version, every) => {}
        }
    })))
}

async fn beat(
    connection: &Connection,
    mut relays: watch::Receiver<usize>,
    version: Version,
    every: Duration,
) {
    loop {
        // asleep until a relay begins
        if relays.wait_for(|&relays| relays > 0).await.is_err() {
            return;
        }
        tokio::time::sleep(every).await;
        if *relays.borrow() == 0 {
            continue;
        }
        if send(connection, version).await.is_err() {
            return;
        }
    }
}

/// One heartbeat, failing only once the connection is gone. A v4 stream
/// that breaks takes its heartbeat along; the next goes on another.
async fn send(connection: &Connection, version: Version) -> Result<(), ConnectionError> {
    let heartbeat = [version.byte(), HEARTBEAT];
    match version {
        Version::V5 => match connection.send_datagram(Bytes::copy_from_slice(&heartbeat)) {
            Err(SendDatagramError::ConnectionLost(e)) => Err(e),
            _ => Ok(()),
        },
        Version::V4 => {
            let mut stream = connection.open_uni().await?;
            if stream.write_all(&heartbeat).await.is_ok() {
                // fails only for a stream finished or reset already
                let _ = stream.finish();
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relays_count_while_their_guards_live() {
        let relays = Relays::new();
        let (one, two) = (relays.begin(), relays.begin());
        assert_eq!(relays.count(), 2);
        drop(one);
        assert_eq!(relays.count(), 1);
        drop(two);
        assert_eq!(relays.count(), 0);
    }

    /// Only the first relay wakes the heartbeat; the end of one wakes
    /// nothing.
    #[test]
    fn only_the_first_relay_wakes_the_heartbeat() {
        let relays = Relays::new();
        let mut watching = relays.0.subscribe();
        let first = relays.begin();
        assert!(watching.has_changed().unwrap());
        watching.borrow_and_update();
        let second = relays.begin();
        drop(first);
        drop(second);
        assert!(!watching.has_changed().unwrap());
        let again = relays.begin();
        assert!(watching.has_changed().unwrap());
        drop(again);
    }
}
