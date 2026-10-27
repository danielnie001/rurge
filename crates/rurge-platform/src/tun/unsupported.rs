//! The adapter on a platform that has none in this version: Windows takes
//! Wintun in M4d, and this is what is left.
//!
//! There is no device to have, so [`Device`] has no values at all: every
//! method is reached only through a `TunDevice`, and no `TunDevice` is ever
//! made here, because `open` fails.

use std::io;

use super::DeviceSpec;
use crate::enhanced::{Adapter, NOT_AVAILABLE};

pub(super) enum Device {}

impl Device {
    /// Opens the adapter: the device and the adapter the steps name. There is
    /// no adapter on this platform in this version, so this always fails.
    pub(super) fn open(_spec: &DeviceSpec) -> io::Result<(Device, Adapter)> {
        Err(io::Error::new(io::ErrorKind::Unsupported, NOT_AVAILABLE))
    }

    /// Waits until a packet is there, then reads it into `buf`.
    pub(super) async fn read(&self, _buf: &mut [u8]) -> io::Result<usize> {
        match *self {}
    }

    /// Reads a packet without waiting.
    pub(super) fn try_read(&self, _buf: &mut [u8]) -> io::Result<usize> {
        match *self {}
    }

    /// Writes one packet without waiting.
    pub(super) fn try_write(&self, _packet: &[u8]) -> io::Result<()> {
        match *self {}
    }
}

/// Whether a failed write means the system had no room for the packet (M4c
/// plan P14): dropped and counted, and the TUN goes on. There is no write
/// here, so nothing has no room.
pub(super) fn no_room(_err: &io::Error) -> bool {
    false
}
