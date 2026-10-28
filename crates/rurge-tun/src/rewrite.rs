//! `[IP Rewrite]` at the receive side of the TUN (phase 3 M5 design 8.4 – 8.6):
//! what a packet the table matched becomes. A matching packet is handled as it
//! arrives, before any stack, rule or policy sees it, and is never a request:
//! `reflect` sends it back with its two addresses swapped, `reject` answers it
//! the way design 8.5 says, and `drop` drops it. Nothing here waits: what it
//! writes is offered to the writer's queue, and what finds the queue full is
//! dropped and counted `queue_full`.

use crate::context::Context;
use crate::icmp;
use crate::packet::{Hit, Reason, Refusal, refusal, rst_for};
use crate::stats::Stats;
use crate::writer::Class;
use rurge_config::IpRewriteAction;
use std::net::IpAddr;
use std::sync::PoisonError;
use std::time::Instant;
use tokio::sync::mpsc::error::TrySendError;

/// The most of an IPv6 packet an ICMPv6 error quotes: the smallest MTU every
/// IPv6 link has, less the IPv6 header and the ICMPv6 header, so that the
/// answer stays within an MTU the virtual interface accepts (RFC 4443 2.4(c);
/// Linux's `icmp6_send` uses the same limit). An IPv6 packet can carry a
/// fragment header before its transport header, so the quote cannot assume
/// the transport header follows the IP header, as IPv4's eight bytes do.
const V6_QUOTE_MAX: usize = 1280 - 40 - 8;

/// Does what `[IP Rewrite]` asks with `packet` (design 8.4 – 8.6). Never
/// waits: what it writes is offered to the writer's queue, and what finds the
/// queue full is dropped and counted `queue_full`.
pub(crate) fn handle(ctx: &Context, hit: &Hit, packet: Vec<u8>) {
    match hit.action {
        IpRewriteAction::Reflect => reflect(ctx, hit, packet),
        IpRewriteAction::Reject => reject(ctx, hit, packet),
        IpRewriteAction::Drop => {
            Stats::drop(&ctx.stats, Reason::IpRewrite, Some(hit.src), Some(hit.dst));
        }
    }
}

/// Sends `packet` back with its two addresses swapped and every other byte as
/// it was: the checksums hold, since they are sums of the same words in another
/// order, and the time to live or hop limit is not touched, because rurge is no
/// router of the packet. What the writer's queue takes is counted `reflected`.
fn reflect(ctx: &Context, hit: &Hit, mut packet: Vec<u8>) {
    match hit.dst {
        IpAddr::V4(_) => swap(&mut packet, 12, 4),
        IpAddr::V6(_) => swap(&mut packet, 8, 16),
    }
    if offer(ctx, vec![packet], hit.dst, hit.src) {
        ctx.stats.reflected();
    }
}

/// Swaps the two fields of `width` bytes at `at` and at `at + width`, which are
/// the source and destination addresses of an IP header.
fn swap(packet: &mut [u8], at: usize, width: usize) {
    let (_, rest) = packet.split_at_mut(at);
    let (src, rest) = rest.split_at_mut(width);
    let (dst, _) = rest.split_at_mut(width);
    src.swap_with_slice(dst);
}

/// Counts the packet as one `[IP Rewrite]` took and answers it as
/// [`Refusal`] says (design 8.5): a reset out of the TUN's own limit on resets,
/// or the ICMP message out of the TUN's limit on those; nothing for a packet
/// that is answered with nothing.
fn reject(ctx: &Context, hit: &Hit, packet: Vec<u8>) {
    Stats::drop(&ctx.stats, Reason::IpRewrite, Some(hit.src), Some(hit.dst));
    match refusal(&packet) {
        Refusal::Nothing => {}
        Refusal::Reset => {
            if ctx.admission.allow_reset(Instant::now()) {
                offer(ctx, vec![rst_for(&packet)], hit.dst, hit.src);
            } else {
                ctx.admission.count(Reason::RstLimited, hit.src, hit.dst);
            }
        }
        Refusal::Prohibited => {
            let allowed = ctx
                .icmp_limit
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .allow(Instant::now());
            if allowed {
                // IPv4's quote is its header and what follows it, up to eight
                // bytes (RFC 1122 3.2.2); IPv6's is as much of the packet as
                // fits in the smallest MTU (RFC 4443 2.4(c))
                let quote = match hit.dst {
                    IpAddr::V4(_) => hit.len.min(hit.header_len + 8),
                    IpAddr::V6(_) => hit.len.min(V6_QUOTE_MAX),
                };
                icmp::refuse(ctx, hit.dst, hit.src, &packet[..quote]);
            } else {
                Stats::drop(
                    &ctx.stats,
                    Reason::IcmpLimited,
                    Some(hit.dst),
                    Some(hit.src),
                );
            }
        }
    }
}

/// Hands `packets`, written from `src` to `dst`, to the writer without waiting
/// for room: `true` if it took them, `false` if its queue was full (counted
/// `queue_full`) or it has ended (the TUN is ending, and nothing is counted).
fn offer(ctx: &Context, packets: Vec<Vec<u8>>, src: IpAddr, dst: IpAddr) -> bool {
    match ctx.try_send(Class::Other, packets) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            // not `ctx.stats.drop(..)`: on an `Arc` that is `Drop::drop`
            Stats::drop(&ctx.stats, Reason::QueueFull, Some(src), Some(dst));
            false
        }
        Err(TrySendError::Closed(_)) => false,
    }
}
