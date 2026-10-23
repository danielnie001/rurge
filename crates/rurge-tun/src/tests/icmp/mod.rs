//! ICMP echo through the TUN (phase 3 M3 design 8): an echo request to an
//! address of rurge's own is answered here, any other goes out through the
//! pinger (one to a fake address to the address its name has), and the reply
//! comes back as from the address the app sent the request to. The app sends
//! with `AppStack::inject` and reads what the TUN writes through
//! `divert_icmp`, so that nothing it reads has been through a stack of its own.

mod echo;
mod limits;
mod log;
mod writer_queue;

use crate::device::{AppEnd, Device, MemoryDevice};
use crate::testing::packets::{self, IcmpSeen};
use crate::testing::{AppStack, FakeDialer, FakeResolve};
use crate::tests::support::{Extras, Rig, WITHIN, eventually, settings, tun_dialing, within};
use crate::{Counters, TunOptions};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, mpsc::error::TryRecvError};

/// The one fixed wait of these tests, for what must not happen: a reply for
/// the app. It would go through the writer and the app's stack in well under a
/// millisecond; 200 ms is a wide margin.
const NOTHING: Duration = Duration::from_millis(200);

/// The app's addresses and the places it pings, one family each; the far ones
/// are documentation addresses.
const APP4: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1));
const APP6: IpAddr = IpAddr::V6(Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, 0, 1));
const FAR4: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
const FAR6: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 10));

/// An address of rurge's own, and the identifier of the echo request to it
/// that is the marker (see `Lab::nothing_came_first`).
const OWN4: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 18, 0, 253));
const MARKER: u16 = 0xfeed;

fn ip(text: &str) -> IpAddr {
    text.parse().expect("an address")
}

/// The echo reply the app is to read.
fn echoed(src: IpAddr, dst: IpAddr, ident: u16, seq: u16, payload: &[u8]) -> IcmpSeen {
    IcmpSeen::EchoReply {
        src,
        dst,
        ident,
        seq,
        payload: payload.to_vec(),
    }
}

/// A rig whose app end has its ICMP packets in the test's hands, with what the
/// TUN was started with.
struct Lab {
    rig: Rig,
    extras: Extras,
    /// The resolver of the settings, which the test teaches names.
    resolver: Arc<FakeResolve>,
    icmp: mpsc::UnboundedReceiver<Vec<u8>>,
}

fn lab(options: TunOptions) -> Lab {
    lab_with(options, Extras::default())
}

fn lab_with(options: TunOptions, extras: Extras) -> Lab {
    let (device, end) = MemoryDevice::pair(1500, 4096);
    lab_on(Arc::new(device), end, options, extras)
}

/// A lab on a device of the test's own, with the app's stack on `end`.
fn lab_on(device: Arc<dyn Device>, end: AppEnd, options: TunOptions, extras: Extras) -> Lab {
    let resolver = FakeResolve::new();
    extras.hooks.set(settings(resolver.clone(), true));
    let dialer = FakeDialer::new();
    let tun = tun_dialing(device, dialer.clone(), &extras, options);
    let app = AppStack::start(end);
    let icmp = app.divert_icmp();
    Lab {
        rig: Rig { tun, app, dialer },
        extras,
        resolver,
        icmp,
    }
}

impl Lab {
    fn counters(&self) -> Counters {
        self.rig.tun.counters()
    }

    /// Gives `name` an address of the pool and says which index it has.
    fn assign(&self, name: &str) -> u32 {
        self.rig
            .tun
            .pool()
            .expect("the lab's TUN has a pool")
            .assign(name, Instant::now())
            .expect("an address for the name")
    }

    /// Every echo request the pinger was asked for.
    fn pinged(&self) -> Vec<(IpAddr, Vec<u8>)> {
        self.extras.pinger.pinged()
    }

    /// From now on the settings of the generation in use have `icmp-forwarding`
    /// as `on` says.
    fn forwarding(&self, on: bool) {
        self.extras.hooks.set(settings(self.resolver.clone(), on));
    }

    /// The app sends an echo request.
    fn ping(&self, from: IpAddr, to: IpAddr, ident: u16, seq: u16, payload: &[u8]) {
        self.rig
            .app
            .inject(packets::echo_request(from, to, ident, seq, payload));
    }

    /// The next packet of ICMP for the app, as it came.
    async fn packet(&mut self) -> Vec<u8> {
        within("an ICMP packet for the app", self.icmp.recv())
            .await
            .expect("the app's stack is there")
    }

    /// The packets of the next reply for the app: one, or the fragments of one.
    async fn reply_packets(&mut self) -> Vec<Vec<u8>> {
        let mut pieces = vec![self.packet().await];
        loop {
            // a packet that is whole says what it is
            if pieces.len() == 1 && packets::read_icmp(&pieces[0]).is_some() {
                return pieces;
            }
            if packets::reassemble(&pieces).is_some() {
                return pieces;
            }
            pieces.push(self.packet().await);
        }
    }

    /// The next reply for the app, whole (put together when it came in
    /// fragments), with right checksums.
    async fn reply(&mut self) -> IcmpSeen {
        let pieces = self.reply_packets().await;
        let whole = match &pieces[..] {
            [only] => only.clone(),
            _ => packets::reassemble(&pieces).expect("the fragments of one reply"),
        };
        packets::read_icmp(&whole).expect("an echo reply with right checksums")
    }

    /// Whether no packet is waiting for the app.
    fn no_reply(&mut self) -> bool {
        matches!(self.icmp.try_recv(), Err(TryRecvError::Empty))
    }

    /// Waits for the pinger to have been asked `count` times in all.
    async fn pinged_times(&self, count: usize) {
        eventually("the pinger to be asked", WITHIN, || {
            self.pinged().len() == count
        })
        .await;
    }

    /// Waits for the tasks of the echo requests to end. Only once they have
    /// begun, which `pinged_times` or `dealt_with` is the proof of: before,
    /// there is nothing to wait for.
    async fn tasks_end(&self) {
        eventually("the echo tasks to end", WITHIN, || {
            self.rig.tun.session_tasks() == 0
        })
        .await;
    }

    /// Nothing was answered before this: an echo request to an address of
    /// rurge's own is answered by the receive task at once, so its reply
    /// comes after whatever the writer was handed before it, and is the next
    /// one if there was nothing.
    async fn nothing_came_first(&mut self) {
        self.ping(APP4, OWN4, MARKER, 0, b"marker");
        assert_eq!(self.reply().await, echoed(OWN4, APP4, MARKER, 0, b"marker"));
    }

    /// Every echo request sent so far has been dealt with, and none of them
    /// answered but the ones the test has read. The first marker shows that the
    /// receive task has read them all, and so has begun their tasks; the
    /// second, that none of the tasks, once they have ended, wrote a reply.
    async fn dealt_with(&mut self) {
        self.nothing_came_first().await;
        self.tasks_end().await;
        self.nothing_came_first().await;
    }
}
