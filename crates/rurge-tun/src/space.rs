//! rurge's own address space (phase 3 M3 design 4.2): which of the addresses a
//! packet is sent to name rurge itself, which stand for a name (fake
//! addresses) and which are a place on the Internet.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The virtual adapter's own address, the source of every packet the
/// operating system sends through it. The platform layer (M4) gives the
/// adapter these; until then only the fixtures of `testing` stand for the
/// system that has them.
#[cfg(any(test, feature = "testing"))]
pub(crate) const ADAPTER_V4: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 1);
#[cfg(any(test, feature = "testing"))]
pub(crate) const ADAPTER_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0, 1);

/// The stack's own addresses. With `any_ip`, smoltcp takes a packet for an
/// address that is not its own only when the route to that address goes
/// through one of its own (design 4.2), so a default route of each family
/// goes through these.
pub(crate) const STACK_V4: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 253);
pub(crate) const STACK_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0, 0xfd);

/// The address `REJECT-NO-DROP` answers an A query with at the DNS layer: one of
/// rurge's own (`Place::Own`), where a connection is reset and a datagram is
/// refused.
pub(crate) const REFUSED_V4: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 244);

/// What the destination of a packet is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// An address of the DNS responder: `198.18.0.2` to `198.18.0.9`,
    /// `fd00:6152::2`.
    Responder,
    /// An address of rurge's own besides those of the responder:
    /// `198.18.0.0/24` and `fd00:6152::/120`, the adapter's, the stack's and
    /// `198.18.0.244` among them.
    Own,
    /// The rest of rurge's space, `198.18.0.0/15` and `fd00:6152::/64`: an
    /// address that stands for a name.
    Fake,
    /// Any other address: a place on the Internet.
    Real,
}

/// Which of the four `ip` is (design 4.2).
pub(crate) fn place(ip: IpAddr) -> Place {
    match ip {
        IpAddr::V4(ip) => match ip.octets() {
            [198, 18, 0, 2..=9] => Place::Responder,
            [198, 18, 0, _] => Place::Own,
            [198, 18 | 19, _, _] => Place::Fake,
            _ => Place::Real,
        },
        IpAddr::V6(ip) => match ip.segments() {
            [0xfd00, 0x6152, 0, 0, 0, 0, 0, 2] => Place::Responder,
            // the /120: all but the last eight bits
            [0xfd00, 0x6152, 0, 0, 0, 0, 0, 0..=0xff] => Place::Own,
            [0xfd00, 0x6152, 0, 0, ..] => Place::Fake,
            _ => Place::Real,
        },
    }
}
