//! What the tests of the engine behind a virtual adapter share (phase 3 M3
//! design 15.2): an engine with a TUN on a memory device and the app's TCP/IP
//! stack on the other end, the guards the rules end with, and the small things
//! the tests do with them. `tun.rs` is the tests of the adapter's TCP, UDP and
//! ICMP and its pool, `tun_dns.rs` those of its DNS responder, of pre-matching
//! and of the fake addresses that come in at a proxy port.

use super::*;
use rurge_engine::RequestRecord;
use rurge_tun::device::MemoryDevice;
use rurge_tun::testing::{AppStack, AppTcp, FakePinger};
use rurge_tun::{Ending, TunHandle, TunOptions};
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use tokio::sync::mpsc::UnboundedReceiver;

/// What every wait of these tests is bounded by.
pub const WITHIN: Duration = Duration::from_secs(5);

/// The address the app has on the adapter, and a UDP endpoint of it.
pub const APP: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1));
pub const APP_UDP: &str = "198.18.0.1:5000";

pub fn app_udp() -> SocketAddr {
    APP_UDP.parse().unwrap()
}

/// `rules`, then the two guards. A session from the app's address that no rule
/// above them took is refused, so it never reaches the `FINAL,DIRECT` of the
/// harness, which would send it wherever it is going (plan P17).
pub fn guarded(rules: &str) -> String {
    format!("{rules}\nSRC-IP,198.18.0.1/32,REJECT\nSRC-IP,fd00:6152::1/128,REJECT")
}

pub fn profile(rules: &str) -> Profile<'_> {
    Profile {
        rules,
        ..Profile::default()
    }
}

/// An engine with a TUN on a memory device, the app's stack on the other end
/// of it, and the pinger the TUN was started with.
pub struct Rig {
    pub h: Harness,
    pub tun: TunHandle,
    pub app: AppStack,
    pub pinger: Arc<FakePinger>,
}

impl Rig {
    pub async fn new(profile: Profile<'_>) -> Rig {
        Rig::on(harness(profile).await).await
    }

    /// A TUN started by the engine of `h` on a new memory device.
    pub async fn on(h: Harness) -> Rig {
        let (device, end) = MemoryDevice::pair(1500, 4096);
        let pinger = FakePinger::new();
        let tun = h
            .engine
            .start_tun(Arc::new(device), pinger.clone(), TunOptions::default())
            .await
            .expect("the TUN starts");
        Rig {
            h,
            tun,
            app: AppStack::start(end),
            pinger,
        }
    }

    /// The fake addresses of `name`, the IPv4 one and the IPv6 one: the pool
    /// gives the name its address when asked, as the DNS responder will.
    pub fn fakes(&self, name: &str) -> (IpAddr, IpAddr) {
        let pool = self.tun.pool().expect("the engine gave the TUN a pool");
        pool.assign(name, std::time::Instant::now())
            .expect("an address for the name");
        (
            pool.address_of(name, false).expect("an IPv4 address"),
            pool.address_of(name, true).expect("an IPv6 address"),
        )
    }

    pub fn data_dir(&self) -> &Path {
        self.h.dir.path()
    }
}

/// `payload` through `tcp` and back, from the echo at the other end.
pub async fn echoes(tcp: &mut AppTcp, payload: &[u8]) {
    tcp.write_all(payload).await.unwrap();
    let mut back = vec![0u8; payload.len()];
    tokio::time::timeout(WITHIN, tcp.read_exact(&mut back))
        .await
        .expect("the echo comes back")
        .unwrap();
    assert_eq!(back, payload);
}

/// The app closes its side and waits for the other to close: the session ends
/// as completed, not as one the app reset.
pub async fn close(mut tcp: AppTcp) {
    tcp.shutdown().await.unwrap();
    let mut rest = Vec::new();
    tokio::time::timeout(WITHIN, tcp.read_to_end(&mut rest))
        .await
        .expect("the other end closes")
        .unwrap();
    assert!(rest.is_empty(), "{rest:?}");
}

/// The next packet on a receiver that `AppStack::divert_udp` or `divert_icmp`
/// gave: what the TUN wrote for the app, of that kind.
pub async fn next(packets: &mut UnboundedReceiver<Vec<u8>>) -> Vec<u8> {
    tokio::time::timeout(WITHIN, packets.recv())
        .await
        .expect("a packet for the app in time")
        .expect("the app's stack is there")
}

/// Waits for the TUN to end, and says why it did.
pub async fn ends(tun: &TunHandle) -> Ending {
    tokio::time::timeout(WITHIN, tun.join())
        .await
        .expect("the TUN ends")
}

/// The finished records of the sessions that came through the adapter, newest
/// first.
pub fn finished_through_the_tun(h: &Harness) -> Vec<RequestRecord> {
    h.engine
        .request_log()
        .recent(1000)
        .into_iter()
        .filter(|record| record.listener == ListenerKind::Tun)
        .collect()
}

/// The records of the finished sessions of the adapter, oldest first, once
/// there are `n`.
pub async fn tun_records(h: &Harness, n: usize) -> Vec<RequestRecord> {
    wait_until("the sessions of the TUN to finish", || {
        finished_through_the_tun(h).len() >= n
    })
    .await;
    let mut records = finished_through_the_tun(h);
    records.reverse();
    records
}
