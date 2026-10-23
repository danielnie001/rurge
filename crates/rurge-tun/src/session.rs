//! The session of a TCP connection through the TUN (phase 3 M3 design 5.6):
//! once the handshake with the app is over, the `Dialer` dials where the app
//! connected to, and relays between the two. A rejection is answered with a
//! reset, except REJECT-DROP, which is answered with nothing for a while. A
//! stop cancels the session of a relay and ends the hold of REJECT-DROP. A
//! connection to port 53 of the responder is no session of the engine's: the
//! responder answers the queries on it (`dns::tcp`).

use crate::dns::tcp::serve_dns;
use crate::dns::{Ask, Responder};
use crate::tcp::{DnsPlace, TunStream, Tuple};
use crate::{Life, TunOptions};
use rurge_config::{HostName, ListenerKind, SessionInfo, Transport};
use rurge_dns::fakeip::Lease;
use rurge_inbound::{DialError, Dialer};
use rurge_proto::RejectKind;
use std::sync::Arc;
use std::time::Duration;

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

/// What the connections of a TUN are served with once their handshake is over,
/// the same for every shard: where the sessions are dialed, the responder that
/// answers the connections to its port 53, what REJECT-DROP holds a connection
/// for, and how long a connection to the responder may be quiet.
pub(crate) struct Serving {
    dialer: Arc<dyn Dialer>,
    /// The TUN's one responder, the same that answers the datagrams: what it
    /// forwards at once is one limit.
    responder: Arc<Responder>,
    holds: Holds,
    /// `TunOptions::dns_tcp_idle`.
    dns_idle: Duration,
}

impl Serving {
    pub(crate) fn new(
        dialer: Arc<dyn Dialer>,
        responder: Arc<Responder>,
        options: &TunOptions,
    ) -> Serving {
        Serving {
            dialer,
            responder,
            holds: Holds::from(options),
            dns_idle: options.dns_tcp_idle,
        }
    }
}

/// What a connection that was let in is for.
pub(crate) enum Purpose {
    /// DNS over TCP: the connection is to port 53 of the responder, which
    /// answers the queries on it itself. The connection's place among the
    /// `dns_tcp_max` the TUN serves at once goes with it, and is given back
    /// when the session ends, or at once if no connection comes of the SYN.
    Dns(DnsPlace),
    /// A session of the engine's, dialed where the app connected to. The name
    /// is the one behind the address the app connected to, with the lease on
    /// that address, if the address is a fake one: the session is for the name
    /// (its `local` is still the endpoint the app connected to), and holds the
    /// lease until it ends, however long that is, so that the address is not
    /// given to another name meanwhile (design 10.2).
    Session(Option<(String, Lease)>),
}

/// The session of the TCP connection `tuple`, as the engine reads it. The one
/// place it is made, for the pre-matching rules the SYN is asked about
/// (`ShardState::refused_by_rules`) and for the dialer the connection is handed
/// to (`serve`), so that the two cannot differ. `name` is the name behind the
/// fake address the app connected to, if it connected to one: the session is
/// for the name, and its `local` is still the endpoint the app connected to.
pub(crate) fn tcp_session(tuple: &Tuple, name: Option<&str>) -> SessionInfo {
    let dst_host = match name {
        Some(name) => HostName::Domain(name.to_owned()),
        None => HostName::Ip(tuple.dst.ip()),
    };
    SessionInfo {
        src: tuple.src,
        in_port: 0,
        local: Some(tuple.dst),
        listener: ListenerKind::Tun,
        ..SessionInfo::tcp(dst_host, tuple.dst.port())
    }
}

/// Serves the connection `tuple`, whose handshake with the app is going on or
/// done, as `purpose` says.
pub(crate) async fn serve(
    stream: TunStream,
    serving: Arc<Serving>,
    tuple: Tuple,
    purpose: Purpose,
    life: Arc<Life>,
) {
    if stream.established().await.is_err() {
        return;
    }
    // A connection whose handshake ended after the stop signal, before the
    // shard's last turn, is being reset by that turn. Dialing it would start a
    // session that the stop cannot cancel (the dial is not raced with the stop,
    // below) and leave `join` waiting for the dial. Dropping the stream resets
    // the connection, which is being reset anyway.
    if life.is_stopped() {
        return;
    }
    match purpose {
        // Boxed: its future would make every session's larger, by some 660 bytes (debug build).
        // The place is held to the end of the arm, which is the end of the serving.
        Purpose::Dns(_place) => Box::pin(answer_dns(stream, &serving, tuple, &life)).await,
        Purpose::Session(name) => dial_and_relay(stream, &serving, tuple, name, &life).await,
    }
}

/// The responder answers the queries on the connection until the connection
/// ends or the TUN stops, and the stop comes first: a query that the upstream
/// holds does not keep the TUN from ending. The future holds no session handle
/// and no record, as a relay does, and is dropped at the stop all the same,
/// with the stream: that resets the connection, which the shard's last turn
/// does anyway. The wait for the stop is on a token of this connection's own
/// (see `dial_and_relay`).
async fn answer_dns(stream: TunStream, serving: &Serving, tuple: Tuple, life: &Life) {
    let ask = Ask {
        src: tuple.src,
        server: tuple.dst,
        transport: Transport::Tcp,
    };
    let stop = life.session_stop();
    tokio::select! {
        biased;
        _ = stop.cancelled() => {}
        _ = serve_dns(stream, serving.responder.clone(), ask, serving.dns_idle) => {}
    }
}

/// The session of the connection is dialed and relayed. `name` is the name
/// behind the address the app connected to, with the lease on that address
/// (see `Purpose::Session`); it is held to the end of the session.
async fn dial_and_relay(
    stream: TunStream,
    serving: &Serving,
    tuple: Tuple,
    name: Option<(String, Lease)>,
    life: &Life,
) {
    let (name, _lease) = match name {
        Some((name, lease)) => (Some(name), Some(lease)),
        None => (None, None),
    };
    let session = tcp_session(&tuple, name.as_deref());
    let dialer = &serving.dialer;
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
            // unfinished leaves the session's record open. The wait for the
            // stop is on a token of this session's own, a child of the stop
            // token (`Life::session_stop`), not on the stop token itself: that
            // one is polled at every wake-up of every relaying session, under
            // a lock that all of them share.
            let stop = life.session_stop();
            tokio::select! {
                biased;
                _ = stop.cancelled() => {
                    token.cancel();
                    relay.await;
                }
                _ = &mut relay => {}
            }
        }
        // Boxed: the future of this rare path (two timers and a buffer of 512
        // bytes) would make the future of every session larger, by some 700
        // bytes (in a debug build).
        Err(DialError::Reject {
            kind: RejectKind::Drop,
            ..
        }) => Box::pin(hold_in_silence(stream, serving.holds, life)).await,
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
/// The close is `TunStream::close_and_wait_for_the_app`, for `linger` at the
/// most, which says why the stream is not just let go after its FIN. A stop
/// during the wait resets the connection, which ends the read with an error.
async fn hold_in_silence(mut stream: TunStream, holds: Holds, life: &Life) {
    tokio::select! {
        biased;
        _ = life.stopped() => return,
        _ = stream.closed() => return,
        _ = tokio::time::sleep(holds.drop_hold) => {}
    }
    stream.close_and_wait_for_the_app(holds.linger).await;
}
