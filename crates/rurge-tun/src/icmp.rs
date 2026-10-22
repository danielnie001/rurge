//! The ICMP messages the TUN makes itself (phase 3 M3 design 7.3): "administratively
//! prohibited", which an app is told for a datagram it sent to a flow that was
//! refused, and for one it sent to an address of rurge's own that no flow goes to.

use crate::dispatch::Context;
use crate::fragment::ip_packet;
use crate::limit::Limiter;
use crate::packet::Reason;
use crate::stats::Stats;
use smoltcp::wire::{
    Icmpv4DstUnreachable, Icmpv4Message, Icmpv4Packet, Icmpv6DstUnreachable, Icmpv6Message,
    Icmpv6Packet, IpProtocol,
};
use std::net::IpAddr;
use std::time::Duration;
use tokio::sync::mpsc::error::TrySendError;

/// The limit on the messages for UDP flows that were refused, all of the TUN's
/// together: 100 in 3 seconds, as the limit on resets is.
const LIMIT: usize = 100;
const WINDOW: Duration = Duration::from_secs(3);

/// The type, the code, the checksum and four bytes nothing uses, which the
/// quoted header follows.
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
/// [`prohibited`]). The message goes to the writer without waiting for room, as
/// everything the receive task or a flow's task hands the writer does: if the
/// queue is full it is dropped and counted.
pub(crate) fn refuse(ctx: &Context, src: IpAddr, dst: IpAddr, quoted: &[u8]) {
    match ctx.writer.try_send(vec![prohibited(src, dst, quoted)]) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            // not `ctx.stats.drop(..)`: on an `Arc` that is `Drop::drop`
            Stats::drop(&ctx.stats, Reason::QueueFull, Some(src), Some(dst));
        }
        // the writer has ended, and so has the TUN
        Err(TrySendError::Closed(_)) => {}
    }
}
