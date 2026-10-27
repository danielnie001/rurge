//! The virtual adapter as the TUN drives it: `rurge-platform`'s `TunDevice`
//! (M4 design 8.1) behind `rurge-tun`'s `Device`, with its two futures boxed.
//!
//! Only the platform backend opens one, and on a platform without the enhanced
//! mode in this version that open fails, so nothing here is reached there
//! (M4c plan P19: no default test opens a device).

use rurge_net::BoxFuture;
use rurge_platform::tun::TunDevice;
use rurge_tun::device::Device;
use std::io;

/// The platform's virtual adapter: packets in and out, the MTU it carries, and
/// the one warning about a packet the system had no room for (the `TunDevice`'s
/// own).
pub struct PlatformDevice(TunDevice);

impl PlatformDevice {
    pub fn new(tun: TunDevice) -> PlatformDevice {
        PlatformDevice(tun)
    }
}

impl Device for PlatformDevice {
    fn recv<'a>(&'a self, into: &'a mut Vec<Vec<u8>>, max: usize) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(self.0.recv(into, max))
    }

    fn send<'a>(&'a self, packets: &'a mut Vec<Vec<u8>>) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(self.0.send(packets))
    }

    fn mtu(&self) -> usize {
        self.0.mtu()
    }
}
