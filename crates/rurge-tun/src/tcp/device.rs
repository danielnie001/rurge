use super::guard::Edges;
use smoltcp::phy::{self, Checksum, DeviceCapabilities, Medium};

/// The shard's side of its stack's device: one packet at a time on its way
/// in, everything the stack sends on its way out. Sending never fails, so
/// no ACK is lost to a device that is busy (phase 3 M3 design 4.5).
pub(super) struct ShardDevice {
    pub(super) rx: Option<Vec<u8>>,
    pub(super) tx: Vec<Vec<u8>>,
    pub(super) mtu: usize,
    /// The right edge of the window the stack last told of each connection,
    /// as the stack writes each segment (design 5.3, H2).
    pub(super) edges: Edges,
}

pub(super) struct RxToken(Vec<u8>);

pub(super) struct TxToken<'a> {
    tx: &'a mut Vec<Vec<u8>>,
    edges: &'a mut Edges,
}

impl phy::Device for ShardDevice {
    type RxToken<'a>
        = RxToken
    where
        Self: 'a;
    type TxToken<'a>
        = TxToken<'a>
    where
        Self: 'a;

    fn receive(
        &mut self,
        _timestamp: smoltcp::time::Instant,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let packet = self.rx.take()?;
        Some((
            RxToken(packet),
            TxToken {
                tx: &mut self.tx,
                edges: &mut self.edges,
            },
        ))
    }

    fn transmit(&mut self, _timestamp: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        Some(TxToken {
            tx: &mut self.tx,
            edges: &mut self.edges,
        })
    }

    /// TCP checksums are computed on what goes out and not checked on what
    /// comes in: the operating system's stack computed them, and the link is
    /// in memory. IPv4 header checksums are computed going out too; checking
    /// those coming in is the dispatcher's (phase 3 M3 design 4.4).
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps.checksum.ipv4 = Checksum::Tx;
        caps.checksum.tcp = Checksum::Tx;
        caps
    }
}

impl phy::RxToken for RxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut packet = vec![0; len];
        let done = f(&mut packet);
        // read as it is written: the edge it leaves is the one the app keeps to
        self.edges.sent(&packet);
        self.tx.push(packet);
        done
    }
}
