//! UDP through the TUN (phase 3 M3 design 7): a session for each source
//! endpoint of an app, served by the dialer's association; the destinations in
//! it; the bytes of the datagrams that wait in the sessions; what comes back,
//! whole or in fragments; and the ICMP message an app gets for a flow that was
//! refused. The app sends through `AppStack::inject` and
//! reads what the TUN writes through `divert_udp` and `divert_icmp`, so that
//! nothing it reads has been through a stack of its own. The rig that does that
//! is [`Lab`], and the tests of the DNS responder's UDP side (`tests::dns::udp`)
//! use it too.

mod budget;
mod fragments;
mod refused;
mod roundtrip;
mod sessions;
mod writer_queue;

use crate::testing::FakeDialer;
use crate::testing::packets::{self, IcmpSeen, UdpSeen};
use crate::tests::support::{Extras, Rig, WITHIN, eventually, rig_extras, within};
use crate::{Counters, TunOptions};
use rurge_config::HostName;
use rurge_dns::fakeip::FakeIpPool;
use rurge_net::connector::Target;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, mpsc::error::TryRecvError};

/// The one fixed wait of these tests, for what must not happen: an ICMP
/// message or a datagram for the app. It would go through the writer and the
/// app's stack in well under a millisecond; 200 ms is a wide margin.
pub(crate) const NOTHING: Duration = Duration::from_millis(200);

/// The app's endpoints, one family each.
pub(crate) const APP4: &str = "198.18.0.1:5000";
pub(crate) const APP6: &str = "[fd00:6152::1]:5000";

/// The places it sends to, one family each.
const FAR4: &str = "192.0.2.10:7";
const FAR6: &str = "[2001:db8::10]:7";

fn target(ip: &str, port: u16) -> Target {
    Target::new(HostName::Ip(ip.parse().unwrap()), port)
}

fn named(name: &str, port: u16) -> Target {
    Target::new(HostName::Domain(name.to_owned()), port)
}

fn target_of(endpoint: SocketAddr) -> Target {
    Target::new(HostName::Ip(endpoint.ip()), endpoint.port())
}

/// A rig whose app end has its UDP and ICMP packets in the test's hands.
pub(crate) struct Lab {
    pub(crate) rig: Rig,
    pub(crate) udp: mpsc::UnboundedReceiver<Vec<u8>>,
    pub(crate) icmp: mpsc::UnboundedReceiver<Vec<u8>>,
}

pub(crate) fn lab(options: TunOptions) -> Lab {
    lab_with(options, Extras::default())
}

pub(crate) fn lab_with(options: TunOptions, extras: Extras) -> Lab {
    lab_and_extras(options, extras).0
}

/// The same with `extras` given back, for a test to reach what the TUN was
/// started with.
pub(crate) fn lab_and_extras(options: TunOptions, extras: Extras) -> (Lab, Extras) {
    let (rig, extras) = rig_extras(options, extras);
    (taking(rig), extras)
}

/// `rig`, with the app's UDP and ICMP taken.
pub(crate) fn taking(rig: Rig) -> Lab {
    let udp = rig.app.divert_udp();
    let icmp = rig.app.divert_icmp();
    Lab { rig, udp, icmp }
}

impl Lab {
    pub(crate) fn dialer(&self) -> &FakeDialer {
        &self.rig.dialer
    }

    pub(crate) fn counters(&self) -> Counters {
        self.rig.tun.counters()
    }

    pub(crate) fn pool(&self) -> Arc<FakeIpPool> {
        self.rig.tun.pool().expect("the rig's TUN has a pool")
    }

    /// The fake address the pool has for `name`, which something gave one
    /// (the responder, say).
    pub(crate) fn fake(&self, name: &str, v6: bool) -> IpAddr {
        self.pool()
            .address_of(name, v6)
            .unwrap_or_else(|| panic!("{name} has no fake address"))
    }

    /// The datagrams the sessions have handed the fake dialer so far: where to,
    /// and the bytes.
    pub(crate) fn sent(&self) -> Vec<(Target, Vec<u8>)> {
        self.dialer().udp_sent()
    }

    /// The app sends a datagram.
    pub(crate) fn send(&self, from: SocketAddr, to: SocketAddr, payload: &[u8]) {
        self.rig.app.inject(packets::udp(from, to, payload));
    }

    /// The next packet of UDP for the app, as it came.
    pub(crate) async fn packet(&mut self) -> Vec<u8> {
        within("a datagram for the app", self.udp.recv())
            .await
            .expect("the app's stack is there")
    }

    /// The next datagram for the app, which comes whole and with right
    /// checksums.
    pub(crate) async fn datagram(&mut self) -> UdpSeen {
        let packet = self.packet().await;
        packets::read_udp(&packet).expect("a datagram with right checksums")
    }

    /// The next `count` packets of UDP for the app, as they came.
    pub(crate) async fn packets(&mut self, count: usize) -> Vec<Vec<u8>> {
        let mut got = Vec::new();
        for _ in 0..count {
            got.push(self.packet().await);
        }
        got
    }

    /// The next ICMP message for the app, which has right checksums.
    pub(crate) async fn icmp(&mut self) -> IcmpSeen {
        let packet = within("an ICMP message for the app", self.icmp.recv())
            .await
            .expect("the app's stack is there");
        packets::read_icmp(&packet).expect("an ICMP message of the two kinds, checksums right")
    }

    /// The next ICMP message, which is the one that says "administratively
    /// prohibited": from where, to where, and what of the packet it quotes.
    pub(crate) async fn prohibited(&mut self) -> (IpAddr, IpAddr, Vec<u8>) {
        match self.icmp().await {
            IcmpSeen::Prohibited { src, dst, quoted } => (src, dst, quoted),
            other => panic!("an administratively prohibited message, not {other:?}"),
        }
    }

    /// Whether no ICMP message is waiting for the app.
    pub(crate) fn no_icmp(&mut self) -> bool {
        matches!(self.icmp.try_recv(), Err(TryRecvError::Empty))
    }

    /// Whether nothing is waiting for the app, neither a datagram nor an ICMP
    /// message.
    pub(crate) fn quiet(&mut self) -> bool {
        matches!(self.udp.try_recv(), Err(TryRecvError::Empty)) && self.no_icmp()
    }

    /// Waits for `count` associations to be open.
    pub(crate) async fn associations_open(&self, count: usize) {
        eventually("the associations to be open", WITHIN, || {
            self.dialer().associations_open() == count
        })
        .await;
    }
}
