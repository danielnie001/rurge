//! The routes into the virtual adapter (phase 3 M4 design 4): rurge's own
//! address space, the set computed from a profile's `tun-*` items, and — in the
//! files this module grows for each platform — the steps that install them.
//!
//! Everything here is data. Nothing installs a route and no program is run: the
//! set is a pure function of the profile's inputs, and the platform steps to
//! come are pure translations, so all three platforms are compiled and tested
//! on every host. Running them belongs to the platform layer (M4c / M4d).

pub mod set;

pub use set::{RouteInputs, RouteSet};

use std::net::{Ipv4Addr, Ipv6Addr};

use ipnet::{Ipv4Net, Ipv6Net};

/// rurge's own space (design 4.5): the adapter, the responder, the stack and
/// the fake addresses.
pub const SPACE_V4: Ipv4Net = Ipv4Net::new_assert(Ipv4Addr::new(198, 18, 0, 0), 15);
/// The IPv6 half of rurge's space (design 4.5), there when `ipv6-vif` is on.
pub const SPACE_V6: Ipv6Net =
    Ipv6Net::new_assert(Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0, 0), 64);

/// The virtual adapter's IPv4 address, the local address of the routes above.
pub const ADAPTER_V4: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 1);
/// The virtual adapter's IPv6 address, there when `ipv6-vif` is on.
pub const ADAPTER_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0, 1);

/// The address the DNS responder listens on: what the system DNS is pointed at.
pub const RESPONDER_V4: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 2);
/// The IPv6 address of the DNS responder.
pub const RESPONDER_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0, 2);

/// The MTU of the adapter (M4 design 4.1).
pub const MTU: u16 = 1500;
