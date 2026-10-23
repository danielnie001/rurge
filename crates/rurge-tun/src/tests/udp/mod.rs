//! UDP through the TUN (phase 3 M3 design 7): a session for each source
//! endpoint of an app, served by the dialer's association; the destinations in
//! it; what comes back, whole or in fragments; and the ICMP message an app gets
//! for a flow that was refused. The app sends through `AppStack::inject` and
//! reads what the TUN writes through `divert_udp` and `divert_icmp`, so that
//! nothing it reads has been through a stack of its own.

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
const NOTHING: Duration = Duration::from_millis(200);

/// The app's endpoints and the places it sends to, one family each.
const APP4: &str = "198.18.0.1:5000";
const APP6: &str = "[fd00:6152::1]:5000";
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
struct Lab {
    rig: Rig,
    udp: mpsc::UnboundedReceiver<Vec<u8>>,
    icmp: mpsc::UnboundedReceiver<Vec<u8>>,
}

fn lab(options: TunOptions) -> Lab {
    lab_with(options, Extras::default())
}

fn lab_with(options: TunOptions, extras: Extras) -> Lab {
    let (rig, _) = rig_extras(options, extras);
    taking(rig)
}

/// `rig`, with the app's UDP and ICMP taken.
fn taking(rig: Rig) -> Lab {
    let udp = rig.app.divert_udp();
    let icmp = rig.app.divert_icmp();
    Lab { rig, udp, icmp }
}

impl Lab {
    fn dialer(&self) -> &FakeDialer {
        &self.rig.dialer
    }

    fn counters(&self) -> Counters {
        self.rig.tun.counters()
    }

    fn pool(&self) -> Arc<FakeIpPool> {
        self.rig.tun.pool().expect("the rig's TUN has a pool")
    }

    /// The app sends a datagram.
    fn send(&self, from: SocketAddr, to: SocketAddr, payload: &[u8]) {
        self.rig.app.inject(packets::udp(from, to, payload));
    }

    /// The next packet of UDP for the app, as it came.
    async fn packet(&mut self) -> Vec<u8> {
        within("a datagram for the app", self.udp.recv())
            .await
            .expect("the app's stack is there")
    }

    /// The next datagram for the app, which comes whole and with right
    /// checksums.
    async fn datagram(&mut self) -> UdpSeen {
        let packet = self.packet().await;
        packets::read_udp(&packet).expect("a datagram with right checksums")
    }

    /// The next `count` packets of UDP for the app, as they came.
    async fn packets(&mut self, count: usize) -> Vec<Vec<u8>> {
        let mut got = Vec::new();
        for _ in 0..count {
            got.push(self.packet().await);
        }
        got
    }

    /// The next ICMP message for the app, which has right checksums.
    async fn icmp(&mut self) -> IcmpSeen {
        let packet = within("an ICMP message for the app", self.icmp.recv())
            .await
            .expect("the app's stack is there");
        packets::read_icmp(&packet).expect("an ICMP message of the two kinds, checksums right")
    }

    /// The next ICMP message, which is the one that says "administratively
    /// prohibited": from where, to where, and what of the packet it quotes.
    async fn prohibited(&mut self) -> (IpAddr, IpAddr, Vec<u8>) {
        match self.icmp().await {
            IcmpSeen::Prohibited { src, dst, quoted } => (src, dst, quoted),
            other => panic!("an administratively prohibited message, not {other:?}"),
        }
    }

    /// Whether no ICMP message is waiting for the app.
    fn no_icmp(&mut self) -> bool {
        matches!(self.icmp.try_recv(), Err(TryRecvError::Empty))
    }

    /// Waits for `count` associations to be open.
    async fn associations_open(&self, count: usize) {
        eventually("the associations to be open", WITHIN, || {
            self.dialer().associations_open() == count
        })
        .await;
    }
}
