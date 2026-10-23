//! UDP through the TUN (phase 3 M3 design 7): a session for each source
//! endpoint of an app, which the dialer serves as an association; each
//! destination the app sends to from that endpoint is a flow of the engine's in
//! it. The receive task finds the session of each datagram (or begins it) and
//! queues the datagram for it; the session's own task runs the association,
//! lets go of the destinations that have gone quiet and ends the session when
//! the whole of it has.
//!
//! Not every datagram is a session's: [`receive`] decides where each goes (see
//! [`destination`]), and the DNS responder takes the queries to its own port 53
//! and to the servers `hijack-dns` names. The bytes that wait in the queues of
//! all the sessions are one budget (`bytes`).

pub(crate) mod bytes;
mod client;
mod targets;

use crate::TunOptions;
use crate::context::Context;
use crate::dns::udp::offer;
use crate::dns::{Ask, DNS_PORT};
use crate::icmp;
use crate::packet::{Parsed, Reason, Transport};
use crate::space::{self, Place, STACK_V4, STACK_V6};
use crate::stats::Stats;
use bytes::Reserved;
use client::TunUdpClient;
use rurge_config::{HostName, ListenerKind, SessionInfo};
use rurge_dns::fakeip::Lease;
use rurge_dns::message;
use rurge_inbound::UdpAdmission;
use rurge_net::connector::Target;
use smoltcp::wire::{Ipv4Packet, Ipv6Packet, UDP_HEADER_LEN, UdpPacket};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use targets::{Entry, Quote, Targets};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_util::sync::CancellationToken;

/// A datagram on its way to the dialer: its bytes, the target it is for, which
/// the receive task decided (a fake address is a name by then), and the bytes
/// it takes out of the budget of the sessions' queues, which go back as the
/// datagram is dropped, whether the dialer has taken it or the queue has gone
/// with it still inside.
pub(crate) struct Queued {
    pub(crate) payload: Vec<u8>,
    pub(crate) target: Target,
    _reserved: Reserved,
}

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
    /// Whether every datagram the app has sent in this session went to port 53.
    /// Such a session is a resolver's, a question and an answer for each new
    /// source port, and ends after `udp_dns_idle` in place of `udp_idle`. It is
    /// false for good from the first datagram to another port: the
    /// destinations of the session are let go of one by one as they go quiet,
    /// and what is left of them says nothing of what the app has sent.
    dns_only: bool,
    pub(crate) targets: Targets,
}

impl State {
    /// How long the session can go without a datagram either way before it
    /// ends.
    fn idle(&self, options: &TunOptions) -> Duration {
        if self.dns_only {
            options.udp_dns_idle
        } else {
            options.udp_idle
        }
    }
}

impl Session {
    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The session ends: no datagram is queued for it from now on, and a datagram
    /// of the app begins another. This only marks the session as ending; it
    /// does not clear the destinations (`end` does that, and lets the leases on
    /// the fake addresses go).
    fn mark_ending(&self) {
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

/// Where a datagram of the app goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Destination {
    /// The DNS responder answers it.
    Responder,
    /// It is sent to an address of rurge's own that no datagram goes to: the
    /// app is told so with ICMP.
    Prohibited,
    /// A UDP session carries it.
    Session,
}

/// Where a datagram to `dst` that carries `payload` goes. The first of these
/// that holds decides:
///
/// 1. It is for port 53 of an address of the responder: the responder.
/// 2. It is for another port of an address of the responder, or for any other
///    address of rurge's own: ICMP "administratively prohibited".
/// 3. It is for an IPv4 address of the pool's space or of the Internet, at a
///    port (and, if the entry names one, an address) that `hijack-dns` of the
///    generation in use names, and what it carries is a DNS query: the
///    responder. What reads as no query (a response, another opcode, a
///    message that does not parse) is the app's own traffic to that server,
///    QUIC for one; so is every datagram over IPv6.
/// 4. A session.
fn destination(ctx: &Context, dst: SocketAddr, payload: &[u8]) -> Destination {
    let place = space::place(dst.ip());
    if place == Place::Responder && dst.port() == DNS_PORT {
        return Destination::Responder;
    }
    if matches!(place, Place::Own | Place::Responder) {
        return Destination::Prohibited;
    }
    if hijacked(ctx, dst, payload) {
        return Destination::Responder;
    }
    Destination::Session
}

/// Whether `hijack-dns` takes a datagram to `dst` that carries `payload`, which
/// is not for an address of rurge's own. The settings are asked for here, for
/// this datagram, and not kept: a reload is seen by the next one.
fn hijacked(ctx: &Context, dst: SocketAddr, payload: &[u8]) -> bool {
    let IpAddr::V4(ip) = dst.ip() else {
        return false;
    };
    ctx.hooks
        .settings()
        .hijack_dns
        .iter()
        .any(|entry| entry.port == dst.port() && entry.addr.is_none_or(|addr| addr == ip))
        && message::read_request(payload).is_ok()
}

/// A datagram from the app (the receive task's, phase 3 M3 design 7.1 and 7.2).
/// It goes where [`destination`] says: to the responder; refused, with ICMP; or
/// queued for the session of the app's endpoint, which is begun if the app has
/// none. A datagram can be dropped and counted. Nothing here waits.
pub(crate) fn receive(ctx: &Arc<Context>, parsed: &Parsed, packet: &[u8]) {
    let Transport::Udp { src_port, dst_port } = parsed.transport else {
        return;
    };
    let (app, dst) = (
        SocketAddr::new(parsed.src, src_port),
        SocketAddr::new(parsed.dst, dst_port),
    );
    let Some((header, payload)) = parts(packet) else {
        // `classify` has read the headers of this datagram and checked its
        // checksum, which are what `parts` reads: it cannot come to this. A
        // debug build says so, so that a test notices; a release build drops
        // the datagram as a malformed one, for a panic here would stop all
        // reception.
        debug_assert!(false, "classify has read this datagram");
        Stats::drop(
            &ctx.stats,
            Reason::Malformed,
            Some(parsed.src),
            Some(parsed.dst),
        );
        return;
    };
    let headers = &packet[..header + UDP_HEADER_LEN];
    match destination(ctx, dst, payload) {
        Destination::Responder => {
            let ask = Ask {
                src: app,
                server: dst,
                transport: rurge_config::Transport::Udp,
            };
            offer(&ctx.dns_queue, &ctx.stats, payload, ask);
            return;
        }
        // rurge's own addresses are no places a datagram goes to: not counted,
        // not limited, and no session
        Destination::Prohibited => {
            icmp::refuse(ctx, dst.ip(), app.ip(), Quote::of(headers).as_slice());
            return;
        }
        Destination::Session => {}
    }
    let datagram = Datagram {
        app,
        dst,
        payload,
        quote: Quote::of(headers),
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
    // no session, or one that is ending: this datagram begins another, if its
    // bytes fit the budget of the queues (the dialer is not asked before)
    let Some((target, lease)) = target_of(ctx, &datagram) else {
        return;
    };
    let Some(reserved) = ctx.udp_bytes.reserve(payload.len()) else {
        Stats::drop(
            &ctx.stats,
            Reason::QueueFull,
            Some(app.ip()),
            Some(dst.ip()),
        );
        return;
    };
    match ctx.dialer.admit_udp() {
        UdpAdmission::Accepted => begin(ctx, &datagram, target, lease, reserved),
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
    state.dns_only &= dst.port() == DNS_PORT;
    let Some(reserved) = ctx.udp_bytes.reserve(payload.len()) else {
        Stats::drop(
            &ctx.stats,
            Reason::QueueFull,
            Some(app.ip()),
            Some(dst.ip()),
        );
        return;
    };
    let queued = Queued {
        payload: payload.to_vec(),
        target,
        _reserved: reserved,
    };
    match session.queue.try_send(queued) {
        Ok(()) => {}
        // the datagram comes back with its bytes, which go back to the budget
        // as it is dropped here
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
/// said it will serve and whose bytes (`reserved`) are in the budget, and starts
/// its task.
fn begin(
    ctx: &Arc<Context>,
    datagram: &Datagram,
    target: Target,
    lease: Option<Lease>,
    reserved: Reserved,
) {
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
    let _ = queue.try_send(Queued {
        payload: datagram.payload.to_vec(),
        target,
        _reserved: reserved,
    });
    let session = Arc::new(Session {
        app: datagram.app,
        queue,
        state: Mutex::new(State {
            ending: false,
            last: datagram.now,
            dns_only: datagram.dst.port() == DNS_PORT,
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
/// (a session that is only sent to, a server pushing, stays; `udp_dns_idle` for
/// as long as the app has sent to port 53 and no other port), when the TUN
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
    let options = &ctx.options;
    let every = (options.udp_idle / 4).max(Duration::from_millis(1));
    let mut swept = Instant::now();
    let mut returned = false;
    loop {
        // the idle time is asked afresh at each turn: the app may have sent to
        // another port since
        let due = {
            let state = session.state();
            (state.last + state.idle(options)).min(swept + every)
        };
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
        if now.saturating_duration_since(state.last) >= state.idle(options) {
            // under the lock a datagram is queued under: the ones that come
            // after this find the session ending
            state.ending = true;
            break;
        }
        if now.saturating_duration_since(swept) >= every {
            state.targets.sweep(now, options.udp_idle);
            swept = now;
        }
    }
    session.mark_ending();
    closed.cancel();
    if !returned {
        association.await;
    }
}
