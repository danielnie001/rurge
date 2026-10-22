//! The ICMP the TUN handles itself (phase 3 M3 design 7.3 and 8). An echo
//! request of an app is answered here when it is for an address of rurge's own;
//! any other goes out through the `Pinger` (one to a fake address, to the
//! address its name has), and the reply comes back as from the address the app
//! sent the request to. And the ICMP message the TUN makes besides:
//! "administratively prohibited", which an app is told for a datagram it sent
//! to a flow that was refused, and for one it sent to an address of rurge's own
//! that no flow goes to.

use crate::TunSettings;
use crate::dispatch::Context;
use crate::fragment::{Ids, ip_packet, ip_packets};
use crate::limit::Limiter;
use crate::packet::{Parsed, Reason};
use crate::space::{self, Place};
use crate::stats::Stats;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    Icmpv4DstUnreachable, Icmpv4Message, Icmpv4Packet, Icmpv4Repr, Icmpv6DstUnreachable,
    Icmpv6Message, Icmpv6Packet, Icmpv6Repr, IpProtocol, Ipv4Packet, Ipv6Packet,
};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::mpsc::error::TrySendError;

/// The limit on the messages for UDP flows that were refused, all of the TUN's
/// together: 100 in 3 seconds, as the limit on resets is.
const LIMIT: usize = 100;
const WINDOW: Duration = Duration::from_secs(3);

/// The type, the code, the checksum and four bytes after them, which the rest
/// of the message follows: the header quoted in "administratively prohibited"
/// (the four bytes are not used there), the data of an echo (they hold its
/// identifier and sequence number).
const HEADER: usize = 8;

/// The limit on the messages for UDP flows that were refused.
pub(crate) fn limiter() -> Limiter {
    Limiter::new(LIMIT, WINDOW)
}

/// The IP packet that says to `dst` that a packet it sent to `src` was refused
/// (ICMPv4's destination unreachable, communication administratively
/// prohibited; ICMPv6's destination unreachable, administratively prohibited),
/// from `src`, with `quoted` after the header: the IP header and the UDP header
/// of the packet, from which the app's stack finds its socket. The checksum is
/// worked out, ICMPv6's with the addresses.
pub(crate) fn prohibited(src: IpAddr, dst: IpAddr, quoted: &[u8]) -> Vec<u8> {
    let mut message = vec![0; HEADER + quoted.len()];
    message[HEADER..].copy_from_slice(quoted);
    let protocol = match (src, dst) {
        (IpAddr::V4(_), IpAddr::V4(_)) => {
            let mut icmp = Icmpv4Packet::new_unchecked(&mut message[..]);
            icmp.set_msg_type(Icmpv4Message::DstUnreachable);
            icmp.set_msg_code(Icmpv4DstUnreachable::CommProhibited.into());
            icmp.fill_checksum();
            IpProtocol::Icmp
        }
        (IpAddr::V6(from), IpAddr::V6(to)) => {
            let mut icmp = Icmpv6Packet::new_unchecked(&mut message[..]);
            icmp.set_msg_type(Icmpv6Message::DstUnreachable);
            icmp.set_msg_code(Icmpv6DstUnreachable::AdminProhibit.into());
            icmp.fill_checksum(&from, &to);
            IpProtocol::Icmpv6
        }
        _ => unreachable!("the two addresses of a packet are of one family"),
    };
    ip_packet(src, dst, protocol, &message)
}

/// Tells the app at `dst` that what it sent to `src` was refused (see
/// [`prohibited`]). The message goes to the writer without waiting for room:
/// the receive task and `UdpClient::refused` must not wait, so a message that
/// finds the queue full is dropped and counted. (What a flow's client sends
/// back, and what an echo request's task writes, wait for room instead.)
pub(crate) fn refuse(ctx: &Context, src: IpAddr, dst: IpAddr, quoted: &[u8]) {
    offer(ctx, vec![prohibited(src, dst, quoted)], src, dst);
}

/// Hands the writer the IP packets `packets` (a message, or the fragments of
/// one) from `src` to `dst`, without waiting for room: if its queue is full
/// they are dropped and counted.
fn offer(ctx: &Context, packets: Vec<Vec<u8>>, src: IpAddr, dst: IpAddr) {
    match ctx.writer.try_send(packets) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            // not `ctx.stats.drop(..)`: on an `Arc` that is `Drop::drop`
            Stats::drop(&ctx.stats, Reason::QueueFull, Some(src), Some(dst));
        }
        // the writer has ended, and so has the TUN
        Err(TrySendError::Closed(_)) => {}
    }
}

/// An echo request of an app, as the TUN keeps it: who sent it and to where,
/// and what its reply repeats.
struct Echo {
    /// The app, which the reply goes to.
    app: IpAddr,
    /// Where the app sent the request, which the reply comes from: a fake
    /// address stays what it is.
    to: IpAddr,
    ident: u16,
    seq: u16,
    payload: Vec<u8>,
}

impl Echo {
    /// The echo request in `packet`, a whole IP packet that `classify` read as
    /// `parsed`. `None` if it does not hold one after all, which it has been
    /// through the same checks for.
    fn read(parsed: &Parsed, packet: &[u8]) -> Option<Echo> {
        let (ident, seq, payload) = match parsed.src {
            IpAddr::V4(_) => {
                let ip = Ipv4Packet::new_checked(packet).ok()?;
                let icmp = Icmpv4Packet::new_checked(ip.payload()).ok()?;
                (icmp.echo_ident(), icmp.echo_seq_no(), icmp.data())
            }
            IpAddr::V6(_) => {
                let ip = Ipv6Packet::new_checked(packet).ok()?;
                let icmp = Icmpv6Packet::new_checked(ip.payload()).ok()?;
                (icmp.echo_ident(), icmp.echo_seq_no(), icmp.payload())
            }
        };
        Some(Echo {
            app: parsed.src,
            to: parsed.dst,
            ident,
            seq,
            payload: payload.to_vec(),
        })
    }

    /// The IP packets of the echo reply that carries `payload`: from where the
    /// app sent the request, to the app, with the identifier and the sequence
    /// number of the request; one packet if it fits the `mtu`, else its
    /// fragments. `None` if the reply is longer than a packet of the family can
    /// carry.
    fn reply(&self, payload: &[u8], mtu: usize, ids: &Ids) -> Option<Vec<Vec<u8>>> {
        // an IPv4 packet is at most 65535 bytes with a header of 20, and
        // IPv6's payload at most 65535 with the ICMP message in it
        let longest = if self.to.is_ipv4() { 65535 - 20 } else { 65535 };
        if HEADER + payload.len() > longest {
            return None;
        }
        let checksums = ChecksumCapabilities::default();
        let (protocol, message) = match (self.to, self.app) {
            (IpAddr::V4(_), IpAddr::V4(_)) => {
                let repr = Icmpv4Repr::EchoReply {
                    ident: self.ident,
                    seq_no: self.seq,
                    data: payload,
                };
                let mut message = vec![0; repr.buffer_len()];
                repr.emit(
                    &mut Icmpv4Packet::new_unchecked(&mut message[..]),
                    &checksums,
                );
                (IpProtocol::Icmp, message)
            }
            (IpAddr::V6(from), IpAddr::V6(to)) => {
                let repr = Icmpv6Repr::EchoReply {
                    ident: self.ident,
                    seq_no: self.seq,
                    data: payload,
                };
                let mut message = vec![0; repr.buffer_len()];
                repr.emit(
                    &from,
                    &to,
                    &mut Icmpv6Packet::new_unchecked(&mut message[..]),
                    &checksums,
                );
                (IpProtocol::Icmpv6, message)
            }
            _ => unreachable!("the two addresses of a packet are of one family"),
        };
        Some(ip_packets(self.to, self.app, protocol, &message, mtu, ids))
    }
}

/// An echo request of an app (the receive task's, phase 3 M3 design 8), which
/// has passed `classify`. The checks come in this order, and the first that
/// holds decides:
///
/// 1. `icmp-forwarding` is off in the generation in use now (asked for each
///    request, so that a change applies to the next): dropped, counted
///    `unsupported`. The addresses of rurge's own are no exception.
/// 2. It is for an address of rurge's own: answered here, at once, without
///    waiting for room in the writer's queue (dropped and counted `queue_full`
///    if there is none).
/// 3. `max_echoes` requests are waiting for their replies already: dropped,
///    counted `busy`.
/// 4. It is for a fake address: its name is asked of the pool (none: dropped,
///    counted `unknown_fake`), and a task resolves the name and has the pinger
///    send the request to the address it finds.
/// 5. Any other address: a task has the pinger send the request there.
pub(crate) fn receive(ctx: &Arc<Context>, parsed: &Parsed, packet: &[u8]) {
    let (app, to) = (parsed.src, parsed.dst);
    let settings = ctx.hooks.settings();
    if !settings.icmp_forwarding {
        Stats::drop(&ctx.stats, Reason::Unsupported, Some(app), Some(to));
        return;
    }
    let Some(echo) = Echo::read(parsed, packet) else {
        // `classify` read these bytes as an echo request
        Stats::drop(&ctx.stats, Reason::Malformed, Some(app), Some(to));
        return;
    };
    let place = space::place(to);
    if matches!(place, Place::Own | Place::Responder) {
        answer_here(ctx, &echo);
        return;
    }
    let Some(slot) = Slot::take(ctx) else {
        Stats::drop(&ctx.stats, Reason::Busy, Some(app), Some(to));
        return;
    };
    let name = if place == Place::Fake {
        let Some(name) = ctx.pool.as_ref().and_then(|pool| pool.name_of(to)) else {
            Stats::drop(&ctx.stats, Reason::UnknownFake, Some(app), Some(to));
            return;
        };
        Some(name)
    } else {
        None
    };
    ctx.tasks
        .spawn(forward(ctx.clone(), settings, echo, name, slot));
}

/// Answers an echo request to an address of rurge's own, in the receive task:
/// the reply goes to the writer without waiting, as everything that task makes
/// does.
fn answer_here(ctx: &Context, echo: &Echo) {
    // it fits a packet: it repeats what the request held
    let Some(packets) = echo.reply(&echo.payload, ctx.mtu, &ctx.ids) else {
        return;
    };
    offer(ctx, packets, echo.to, echo.app);
}

/// A place among the echo requests in flight: taken by the receive task before
/// it starts the task of a request, held by that task, and given back when the
/// task is dropped, however it ends (a reply, the timeout, a failure, the stop,
/// a future that never ran).
struct Slot(Arc<Context>);

impl Slot {
    /// A place for one more echo request, unless `max_echoes` are in flight.
    fn take(ctx: &Arc<Context>) -> Option<Slot> {
        ctx.echoes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |in_flight| {
                (in_flight < ctx.options.max_echoes).then_some(in_flight + 1)
            })
            .ok()
            .map(|_| Slot(ctx.clone()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.echoes.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The task of an echo request that goes out: it resolves the name of a fake
/// address, has the pinger send the request, and writes the reply for the app.
/// It ends at the stop, wherever it is; without a reply when the pinger fails
/// and when none has come `echo_timeout` after it began (the resolving counts);
/// and with the reply, which waits for room in the writer's queue, as a flow's
/// replies do, but not past the stop. Whatever goes wrong is said at debug
/// level, with the name or the address.
async fn forward(
    ctx: Arc<Context>,
    settings: Arc<TunSettings>,
    echo: Echo,
    name: Option<String>,
    _slot: Slot,
) {
    let waited = tokio::select! {
        biased;
        _ = ctx.life.stopped() => return,
        waited = tokio::time::timeout(
            ctx.options.echo_timeout,
            exchange(&ctx, &settings, &echo, name.as_deref()),
        ) => waited,
    };
    let payload = match waited {
        Ok(Some(payload)) => payload,
        // said where it failed
        Ok(None) => return,
        Err(_) => {
            tracing::debug!(
                dst = %echo.to,
                name = name.as_deref(),
                "tun: an echo request got no reply in time"
            );
            return;
        }
    };
    let Some(packets) = echo.reply(&payload, ctx.mtu, &ctx.ids) else {
        tracing::debug!(
            dst = %echo.to,
            len = payload.len(),
            "tun: the reply to an echo request is too long to be a packet"
        );
        return;
    };
    tokio::select! {
        biased;
        _ = ctx.life.stopped() => {}
        _ = ctx.writer.send(packets) => {}
    }
}

/// What the pinger answers to `echo`, which it is asked for at its destination,
/// or, when that is a fake address and `name` is the name behind it, at the
/// address the name resolves to. `None`, said at debug level, when the name
/// does not resolve or has no address, and when the pinger fails.
async fn exchange(
    ctx: &Context,
    settings: &TunSettings,
    echo: &Echo,
    name: Option<&str>,
) -> Option<Vec<u8>> {
    let dst = match name {
        None => echo.to,
        Some(name) => match settings.resolver.resolve(name).await {
            Ok(addresses) => {
                let Some(address) = nearest(&addresses, echo.to) else {
                    tracing::debug!(name, "tun: the name of an echo request has no address");
                    return None;
                };
                address
            }
            Err(error) => {
                tracing::debug!(name, %error, "tun: the name of an echo request did not resolve");
                return None;
            }
        },
    };
    match ctx.pinger.echo(dst, &echo.payload).await {
        Ok(payload) => Some(payload),
        Err(error) => {
            tracing::debug!(%dst, %error, "tun: an echo request failed");
            None
        }
    }
}

/// The address of `addresses` to send an echo request that is like `like` to:
/// the first of the same family, or else the first there is.
fn nearest(addresses: &[IpAddr], like: IpAddr) -> Option<IpAddr> {
    addresses
        .iter()
        .find(|address| address.is_ipv4() == like.is_ipv4())
        .or(addresses.first())
        .copied()
}
