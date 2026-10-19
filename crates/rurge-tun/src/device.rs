//! The device the adapter's core reads packets from and writes them to
//! (phase 3 M3 design 4.1), and the one that lives in memory.

use rurge_net::BoxFuture;
use std::io;
use tokio::sync::{Mutex, mpsc};

pub trait Device: Send + Sync {
    /// Waits for at least one packet, then moves at most `max` whole IP packets into `into`.
    /// An error ends the TUN.
    fn recv<'a>(&'a self, into: &'a mut Vec<Vec<u8>>, max: usize) -> BoxFuture<'a, io::Result<()>>;
    /// Writes `packets` in order and leaves it empty. The memory device waits for room.
    fn send<'a>(&'a self, packets: &'a mut Vec<Vec<u8>>) -> BoxFuture<'a, io::Result<()>>;
    /// The largest IP packet the device carries, in bytes; `Tun::start` panics for less than 1280.
    fn mtu(&self) -> usize;
}

/// A device that is two channels: what the other end, an [`AppEnd`], sends is
/// what `recv` gives, and what `send` writes is what the other end receives.
pub struct MemoryDevice {
    mtu: usize,
    from_app: Mutex<mpsc::Receiver<Vec<u8>>>,
    to_app: mpsc::Sender<Vec<u8>>,
}

impl MemoryDevice {
    /// A device with room for `capacity` packets each way, and the end that
    /// stands for the operating system.
    pub fn pair(mtu: usize, capacity: usize) -> (MemoryDevice, AppEnd) {
        let (to_device, from_app) = mpsc::channel(capacity);
        let (to_app, from_device) = mpsc::channel(capacity);
        (
            MemoryDevice {
                mtu,
                from_app: Mutex::new(from_app),
                to_app,
            },
            AppEnd {
                mtu,
                to_peer: to_device,
                from_peer: from_device,
            },
        )
    }
}

fn gone() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "the app end of the memory device is gone",
    )
}

impl Device for MemoryDevice {
    fn recv<'a>(&'a self, into: &'a mut Vec<Vec<u8>>, max: usize) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // `recv_many` answers 0 for a limit of 0 too
            let received = self.from_app.lock().await.recv_many(into, max.max(1)).await;
            if received == 0 { Err(gone()) } else { Ok(()) }
        })
    }

    /// Dropping the future in the middle of a send loses the packets not yet
    /// sent: they are taken out of `packets` as they come to be sent.
    fn send<'a>(&'a self, packets: &'a mut Vec<Vec<u8>>) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            for packet in packets.drain(..) {
                self.to_app.send(packet).await.map_err(|_| gone())?;
            }
            Ok(())
        })
    }

    fn mtu(&self) -> usize {
        self.mtu
    }
}

/// The operating system's end of a memory device: what a test sends through
/// it comes out of the device's `recv`, and what the device sends comes out
/// of `recv` here.
pub struct AppEnd {
    mtu: usize,
    to_peer: mpsc::Sender<Vec<u8>>,
    from_peer: mpsc::Receiver<Vec<u8>>,
}

impl AppEnd {
    pub fn mtu(&self) -> usize {
        self.mtu
    }

    /// Waits until the device takes the packet; `BrokenPipe` when it is gone.
    pub async fn send(&self, packet: Vec<u8>) -> io::Result<()> {
        self.to_peer.send(packet).await.map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the device end of the memory device is gone",
            )
        })
    }

    /// The next packet the device sent; `None` once the device is gone and
    /// what it sent has been received.
    pub async fn recv(&mut self) -> Option<Vec<u8>> {
        self.from_peer.recv().await
    }

    pub fn try_recv(&mut self) -> Option<Vec<u8>> {
        self.from_peer.try_recv().ok()
    }

    /// Two ends joined to each other, with no device between: what one sends
    /// the other receives.
    pub fn crossed(mtu: usize, capacity: usize) -> (AppEnd, AppEnd) {
        let (to_b, from_a) = mpsc::channel(capacity);
        let (to_a, from_b) = mpsc::channel(capacity);
        (
            AppEnd {
                mtu,
                to_peer: to_b,
                from_peer: from_b,
            },
            AppEnd {
                mtu,
                to_peer: to_a,
                from_peer: from_a,
            },
        )
    }

    /// The two directions apart, for a task that must wait for room to send
    /// and for a packet to receive at the same time.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn into_parts(self) -> (usize, mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
        (self.mtu, self.to_peer, self.from_peer)
    }
}
