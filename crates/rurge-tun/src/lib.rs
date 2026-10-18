//! The core of rurge's virtual adapter: IP packets come in from a device,
//! TCP connections go out to the session pipeline, on a userspace TCP/IP
//! stack (phase 3 M3 design: `docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md`).
//! Platform-free: the device is a trait, and the one implemented here lives
//! in memory.

pub mod device;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// The largest receive buffer of any TCP socket on either side of the
/// adapter. With 65535 bytes the window scale is 0, which keeps smoltcp 0.12's
/// window-edge arithmetic from underflowing (design 5.3, H2).
pub const RX_MAX: usize = 65_535;

#[cfg(test)]
mod tests;
