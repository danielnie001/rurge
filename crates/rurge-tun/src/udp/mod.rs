//! UDP through the TUN (phase 3 M3 design 7): a session for each source
//! endpoint of an app, which the dialer serves as an association; each
//! destination the app sends to from that endpoint is a flow of the engine's in
//! it. The receive task finds the session of each datagram (or begins it) and
//! queues the datagram for it; the session's own task runs the association,
//! lets go of the destinations that have gone quiet and ends the session when
//! the whole of it has.

mod client;
mod targets;

use crate::dispatch::Context;
use crate::icmp;
use crate::packet::{Parsed, Reason, Transport};
use crate::space::{self, Place, STACK_V4, STACK_V6};
use crate::stats::Stats;
use client::TunUdpClient;
use rurge_config::{HostName, ListenerKind, SessionInfo};
use rurge_dns::fakeip::Lease;
use rurge_inbound::UdpAdmission;
use rurge_net::connector::Target;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{Ipv4Packet, Ipv6Packet, UDP_HEADER_LEN, UdpPacket, UdpRepr};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use targets::{Entry, Quote, Targets};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_util::sync::CancellationToken;

/// A datagram on its way to the dialer: its bytes, and the target it is for,
/// which the receive task decided (a fake address is a name by then).
type Queued = (Vec<u8>, Target);

/// The sessions of the TUN, by the endpoint of the app each is for. The receive
/// task adds them, and each session takes itself out when it is over.
#[derive(Default)]
pub(crate) struct Sessions {
    table: Mutex<HashMap<SocketAddr, Arc<Session>>>,
}

impl Sessions {
    fn table(&self) -> MutexGuard<'_, HashMap<SocketAddr, Arc<Session>>> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The session for the app at `app`, if the table has one: it may be
    /// ending, which the session's state says.
    fn get(&self, app: SocketAddr) -> Option<Arc<Session>> {
        self.table().get(&app).cloned()
    }

    /// Makes `session` the one for its app, in place of any other (which is
    /// ending).
    fn insert(&self, session: Arc<Session>) {
        self.table().insert(session.app, session);
    }

    /// Takes `session` out of the table unless a newer session of the app has
    /// taken its place.
    fn remove(&self, session: &Arc<Session>) {
        let mut table = self.table();
        if table
            .get(&session.app)
            .is_some_and(|current| Arc::ptr_eq(current, session))
        {
            table.remove(&session.app);
        }
    }
}

/// One session: what the receive task, the session's task and its client share.
pub(crate) struct Session {
    /// The endpoint of the app: where the replies go, and what the session is
    /// for.
    pub(crate) app: SocketAddr,
    /// The datagrams for the dialer; the client holds the other end.
    queue: mpsc::Sender<Queued>,
    state: Mutex<State>,
}

pub(crate) struct State {
    /// The session is over, or ends as soon as its task looks: nothing is
    /// queued for it, and a datagram of the app begins another. Set under the
    /// same lock that a datagram is queued under, so that a datagram is queued
    /// for a session that will take it or for none.
    ending: bool,
    /// When a datagram last went either way.
    pub(crate) last: Instant,
    pub(crate) targets: Targets,
}

impl Session {
    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The session ends: no datagram is queued for it from now on, and a datagram
    /// of the app begins another.
    fn ending(&self) {
        self.state().ending = true;
    }

    /// The session is over: its destinations go, with the holds they have on
    /// fake addresses.
    fn end(&self) {
        let mut state = self.state();
        state.ending = true;
        state.targets.clear();
    }
}

/// What the receive task knows of a datagram.
struct Datagram<'a> {
    app: SocketAddr,
    dst: SocketAddr,
    payload: &'a [u8],
    /// The IP header and the UDP header, for the ICMP message about it.
    quote: Quote,
    now: Instant,
}

/// The UDP datagram, its IP header and the IP header's length, of a packet that
/// passed `classify` as UDP.
fn parts(packet: &[u8]) -> Option<(usize, &[u8])> {
    let ip_payload = match packet.first()? >> 4 {
        4 => Ipv4Packet::new_checked(packet).ok()?.payload(),
        _ => Ipv6Packet::new_checked(packet).ok()?.payload(),
    };
    let payload = UdpPacket::new_checked(ip_payload).ok()?.payload();
    Some((packet.len() - ip_payload.len(), payload))
}

/// A datagram from the app (the receive task's, phase 3 M3 design 7.1 and 7.2).
/// It is queued for the session of the app's endpoint, which is begun if the
/// app has none; or it is refused, or dropped and counted. Nothing here waits.
pub(crate) fn receive(ctx: &Arc<Context>, parsed: &Parsed, packet: &[u8]) {
    let Transport::Udp { src_port, dst_port } = parsed.transport else {
        return;
    };
    let (app, dst) = (
        SocketAddr::new(parsed.src, src_port),
        SocketAddr::new(parsed.dst, dst_port),
    );
    let Some((header, payload)) = parts(packet) else {
        Stats::drop(
            &ctx.stats,
            Reason::Malformed,
            Some(parsed.src),
            Some(parsed.dst),
        );
        return;
    };
    let quote = Quote::of(&packet[..header + UDP_HEADER_LEN]);
    // rurge's own addresses are no places a datagram goes to: not counted, not
    // limited, and no session
    if matches!(space::place(dst.ip()), Place::Own | Place::Responder) {
        icmp::refuse(ctx, dst.ip(), app.ip(), quote.as_slice());
        return;
    }
    let datagram = Datagram {
        app,
        dst,
        payload,
        quote,
        now: Instant::now(),
    };
    let session = ctx.sessions.get(app);
    if let Some(session) = &session {
        let mut state = session.state();
        if !state.ending {
            deliver(ctx, session, &mut state, &datagram);
            return;
        }
    }
    // no session, or one that is ending: this datagram begins another
    let Some((target, lease)) = target_of(ctx, &datagram) else {
        return;
    };
    match ctx.dialer.admit_udp() {
        UdpAdmission::Accepted => begin(ctx, &datagram, target, lease),
        UdpAdmission::Busy => {
            Stats::drop(&ctx.stats, Reason::Busy, Some(app.ip()), Some(dst.ip()));
        }
        UdpAdmission::NotSupported => {
            Stats::drop(
                &ctx.stats,
                Reason::Unsupported,
                Some(app.ip()),
                Some(dst.ip()),
            );
        }
    }
}

/// The target of a datagram, with the lease on the fake address if it is sent
/// to one: that of its name. `None` if it is sent to a fake address that names
/// nothing (counted), or the pool is not there.
fn target_of(ctx: &Context, datagram: &Datagram) -> Option<(Target, Option<Lease>)> {
    let dst = datagram.dst;
    match space::place(dst.ip()) {
        Place::Fake => {
            let named = ctx.pool.as_ref().and_then(|pool| pool.lease(dst.ip()));
            if named.is_none() {
                Stats::drop(
                    &ctx.stats,
                    Reason::UnknownFake,
                    Some(datagram.app.ip()),
                    Some(dst.ip()),
                );
            }
            named
                .map(|(name, lease)| (Target::new(HostName::Domain(name), dst.port()), Some(lease)))
        }
        _ => Some((Target::new(HostName::Ip(dst.ip()), dst.port()), None)),
    }
}

/// Queues a datagram for a session that is there and not ending, whose state
/// is locked: for the destination it has, or for a new one if it has room.
fn deliver(ctx: &Context, session: &Session, state: &mut State, datagram: &Datagram) {
    let Datagram {
        app,
        dst,
        payload,
        quote,
        now,
    } = *datagram;
    let target = match state.targets.by_endpoint(dst) {
        Some(entry) => {
            entry.sent(quote, now);
            entry.target.clone()
        }
        None => {
            let Some((target, lease)) = target_of(ctx, datagram) else {
                return;
            };
            if state.targets.is_full() {
                // the lease goes with the datagram
                Stats::drop(&ctx.stats, Reason::Busy, Some(app.ip()), Some(dst.ip()));
                return;
            }
            state
                .targets
                .insert(Entry::new(dst, target.clone(), lease, quote, now));
            target
        }
    };
    state.last = now;
    match session.queue.try_send((payload.to_vec(), target)) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            Stats::drop(
                &ctx.stats,
                Reason::QueueFull,
                Some(app.ip()),
                Some(dst.ip()),
            );
        }
        // the client has gone: the session is over, and the datagram with it
        Err(TrySendError::Closed(_)) => {}
    }
}

/// Begins the session of the app with its first datagram, which the dialer has
/// said it will serve, and starts its task.
fn begin(ctx: &Arc<Context>, datagram: &Datagram, target: Target, lease: Option<Lease>) {
    let (queue, queued) = mpsc::channel(ctx.options.udp_queue);
    let mut targets = Targets::new(ctx.options.max_udp_targets);
    targets.insert(Entry::new(
        datagram.dst,
        target.clone(),
        lease,
        datagram.quote,
        datagram.now,
    ));
    // the queue is empty: the first datagram fits
    let _ = queue.try_send((datagram.payload.to_vec(), target));
    let session = Arc::new(Session {
        app: datagram.app,
        queue,
        state: Mutex::new(State {
            ending: false,
            last: datagram.now,
            targets,
        }),
    });
    let client = Arc::new(TunUdpClient::new(ctx.clone(), session.clone(), queued));
    ctx.sessions.insert(session.clone());
    ctx.tasks.spawn(serve(ctx.clone(), session, client));
}

/// What the dialer is given for the session of the app at `app`: where it is
/// from, the TUN as the listener, and no endpoint of ours it came to; its
/// destination is the stack's own address of the app's family and port 0, which
/// each flow replaces with its own (design 7.1).
fn template(app: SocketAddr) -> SessionInfo {
    let own = if app.is_ipv4() {
        IpAddr::V4(STACK_V4)
    } else {
        IpAddr::V6(STACK_V6)
    };
    SessionInfo {
        src: app,
        in_port: 0,
        local: None,
        listener: ListenerKind::Tun,
        ..SessionInfo::udp(HostName::Ip(own), 0)
    }
}

/// Takes a session out when its task is over, however that is: nothing is
/// queued for it from then on, its destinations go, and it leaves the table
/// unless the app's newer session is there in its place.
struct Over {
    ctx: Arc<Context>,
    session: Arc<Session>,
}

impl Drop for Over {
    fn drop(&mut self) {
        self.session.end();
        self.ctx.sessions.remove(&self.session);
    }
}

/// The session's task: the association runs in it until the session is over.
/// That is when neither the app nor a flow has sent a datagram for `udp_idle`
/// (a session that is only sent to, a server pushing, stays), when the TUN
/// stops, or when the association returns by itself (the engine stopped, or had
/// no room): the association is then told to end and waited for, and the
/// session leaves the table. Every `udp_idle` / 4 at most, the destinations
/// with nothing either way for `udp_idle` are let go.
async fn serve(ctx: Arc<Context>, session: Arc<Session>, client: Arc<TunUdpClient>) {
    let _over = Over {
        ctx: ctx.clone(),
        session: session.clone(),
    };
    let closed = CancellationToken::new();
    let mut association =
        ctx.dialer
            .associate(client.clone(), template(session.app), closed.clone());
    let idle = ctx.options.udp_idle;
    let every = (idle / 4).max(Duration::from_millis(1));
    let mut swept = Instant::now();
    let mut returned = false;
    loop {
        let due = (session.state().last + idle).min(swept + every);
        tokio::select! {
            biased;
            _ = ctx.life.stopped() => break,
            () = &mut association => {
                returned = true;
                break;
            }
            _ = tokio::time::sleep(due.saturating_duration_since(Instant::now())) => {}
        }
        let now = Instant::now();
        let mut state = session.state();
        if now.saturating_duration_since(state.last) >= idle {
            // under the lock a datagram is queued under: the ones that come
            // after this find the session ending
            state.ending = true;
            break;
        }
        if now.saturating_duration_since(swept) >= every {
            state.targets.sweep(now, idle);
            swept = now;
        }
    }
    session.ending();
    closed.cancel();
    if !returned {
        association.await;
    }
}

/// The UDP datagram from `src` to `dst` that carries `payload`, its checksum
/// worked out. `None` if it is longer than a packet of the family can carry:
/// IPv4's packet is at most 65535 bytes, with a header of 20, and IPv6's
/// payload is at most 65535, with the UDP header in it.
fn datagram(src: SocketAddr, dst: SocketAddr, payload: &[u8]) -> Option<Vec<u8>> {
    let longest = if src.is_ipv4() { 65535 - 20 } else { 65535 };
    if UDP_HEADER_LEN + payload.len() > longest {
        return None;
    }
    let repr = UdpRepr {
        src_port: src.port(),
        dst_port: dst.port(),
    };
    let mut bytes = vec![0; UDP_HEADER_LEN + payload.len()];
    repr.emit(
        &mut UdpPacket::new_unchecked(&mut bytes[..]),
        &src.ip().into(),
        &dst.ip().into(),
        payload.len(),
        |buffer| buffer.copy_from_slice(payload),
        &ChecksumCapabilities::default(),
    );
    Some(bytes)
}
