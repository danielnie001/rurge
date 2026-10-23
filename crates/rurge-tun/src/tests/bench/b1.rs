//! The machinery of B1, the single-stream throughput measurement: a transfer
//! one way through a new TUN, the same between two app stacks with no TUN (the
//! fixture's own ceiling), and the row each of them gives.

use super::{DEVICE_ROOM, Lane, blast, director, drain};
use crate::device::{AppEnd, MemoryDevice};
use crate::testing::{AppStack, Script, Upstream};
use crate::tests::support::{addr, tun_on};
use crate::{Counters, TunOptions};
use smoltcp::wire::{IpAddress, IpCidr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What a transfer is bounded by.
const LIMIT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Way {
    /// The app writes, the far end reads.
    Up,
    /// The far end writes, the app reads.
    Down,
}

impl Way {
    pub(super) fn name(self) -> &'static str {
        match self {
            Way::Up => "up",
            Way::Down => "down",
        }
    }
}

/// One transfer of B1: how long it took, and what the TUN counted while it
/// did (`None` when there was no TUN).
pub(super) struct Row {
    pub(super) elapsed: Duration,
    counters: Option<Counters>,
}

impl Row {
    pub(super) fn gbit(&self, bytes: u64) -> f64 {
        bytes as f64 * 8.0 / self.elapsed.as_secs_f64() / 1e9
    }

    /// The four counts that make a row not count when one is not 0.
    pub(super) fn counted(&self) -> bool {
        self.counters.is_none_or(|counters| {
            counters.beyond_window == 0
                && counters.queue_full == 0
                && counters.rst_limited == 0
                && counters.stack_panics == 0
        })
    }

    pub(super) fn counts(&self) -> String {
        match self.counters {
            None => "-".to_owned(),
            Some(counters) => format!(
                "beyond_window {} queue_full {} rst_limited {} stack_panics {}",
                counters.beyond_window,
                counters.queue_full,
                counters.rst_limited,
                counters.stack_panics
            ),
        }
    }
}

/// Where the far end of a transfer through the TUN is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Far {
    /// A sink or a source behind a TCP connection on the loopback, on a thread
    /// of its own: what design 6 asks for.
    Loopback,
    /// The fake `Dialer`'s far end in memory, on the TUN's thread: no loopback
    /// hop, so the TUN's own speed shows. For the way down its pattern is made
    /// on that thread too.
    Memory,
}

impl Far {
    /// `TUN_BENCH_FAR`: `loopback`, or `memory` to leave the loopback hop out.
    pub(super) fn from_env() -> Far {
        match std::env::var("TUN_BENCH_FAR").as_deref() {
            Err(_) | Ok("loopback") => Far::Loopback,
            Ok("memory") => Far::Memory,
            Ok(other) => panic!("TUN_BENCH_FAR is loopback or memory, not {other:?}"),
        }
    }
}

/// `bytes` bytes one way through a new TUN on a new memory device of this MTU:
/// the TUN, the fake `Dialer`'s relay and the connection to the far end on one
/// thread, the app's stack and the transfer on a second, the far end on a
/// third (unless it is in memory). The time starts when the connection is made
/// and ends when the last byte is in: the far end's, for the way up.
pub(super) fn through_the_tun(mtu: usize, tx_max: usize, way: Way, bytes: u64, far: Far) -> Row {
    director().block_on(async {
        let (rurge, app) = (Lane::new("rurge"), Lane::new("app"));
        // kept to the end of the row: its thread is the far end's
        let upstream = (far == Far::Loopback).then(|| Lane::new("upstream"));
        let server = match &upstream {
            Some(lane) => Some(Arc::new(
                lane.spawn(async move {
                    match way {
                        Way::Up => Upstream::sink().await,
                        Way::Down => Upstream::source(bytes).await,
                    }
                })
                .await
                .expect("the far end"),
            )),
            None => None,
        };
        let (device, end) = MemoryDevice::pair(mtu, DEVICE_ROOM);
        let options = TunOptions {
            tx_max,
            ..TunOptions::default()
        };
        let (tun, dialer) = rurge
            .spawn(async move { tun_on(device, options) })
            .await
            .expect("the TUN");
        let dst = addr("192.0.2.1:80");
        dialer.script(
            dst,
            match &server {
                Some(server) => Script::Upstream(server.addr()),
                None => Script::Memory {
                    buffer: 64 * 1024,
                    send: if way == Way::Down { bytes as usize } else { 0 },
                },
            },
        );
        let stack = app
            .spawn(async move { AppStack::start(end) })
            .await
            .expect("the app's stack");
        let elapsed = app
            .spawn({
                let dialer = dialer.clone();
                async move {
                    tokio::time::timeout(LIMIT, async {
                        let mut tcp = stack.connect(dst).await.expect("connect");
                        let began = Instant::now();
                        match way {
                            Way::Up => {
                                blast(&mut tcp, bytes).await.expect("write");
                                match &server {
                                    Some(server) => assert!(
                                        server.wait_for(bytes, LIMIT).await,
                                        "the sink did not get every byte"
                                    ),
                                    None => assert!(
                                        dialer.wait_for(dst, bytes, LIMIT).await,
                                        "the far end did not get every byte"
                                    ),
                                }
                            }
                            Way::Down => drain(&mut tcp, bytes).await.expect("read"),
                        }
                        began.elapsed()
                    })
                    .await
                    .expect("the transfer to end in time")
                }
            })
            .await
            .expect("the transfer");
        Row {
            elapsed,
            counters: Some(tun.counters()),
        }
    })
}

/// The same transfer between two app stacks on a crossed pair, with no TUN
/// between them: the most this fixture can do. The one that connects is on the
/// app's thread, the one that only listens on the TUN's, as in the rows with a
/// TUN.
pub(super) fn fixture_ceiling(mtu: usize, way: Way, bytes: u64) -> Row {
    director().block_on(async {
        let (rurge, app) = (Lane::new("rurge"), Lane::new("app"));
        let (near, far) = AppEnd::crossed(mtu, DEVICE_ROOM);
        let dst = addr("192.0.2.1:80");
        let connecting = app
            .spawn(async move { AppStack::start(near) })
            .await
            .expect("the app's stack");
        let listening = rurge
            .spawn(async move {
                let own = [IpCidr::new(IpAddress::v4(192, 0, 2, 1), 32)];
                AppStack::start_with(far, &own)
            })
            .await
            .expect("the other stack");
        // listening from here on, so the SYN finds the socket; the future that
        // comes back owns all it needs, and goes to another thread
        let accepting = listening.accept(dst);
        let serving = rurge.spawn(async move {
            let mut tcp = accepting.await.expect("accept");
            match way {
                Way::Up => drain(&mut tcp, bytes).await.expect("read"),
                Way::Down => {
                    blast(&mut tcp, bytes).await.expect("write");
                    // a connection dropped with data in its buffer is reset, and
                    // the reset takes the data with it: close, and stay until
                    // the reader has closed its end
                    tcp.shutdown().await.expect("shutdown");
                    let mut rest = [0; 1];
                    while matches!(tcp.read(&mut rest).await, Ok(n) if n > 0) {}
                }
            }
        });
        let elapsed = app
            .spawn(async move {
                tokio::time::timeout(LIMIT, async {
                    let mut tcp = connecting.connect(dst).await.expect("connect");
                    let began = Instant::now();
                    let done = match way {
                        Way::Up => {
                            blast(&mut tcp, bytes).await.expect("write");
                            serving.await.expect("the listening side");
                            Instant::now()
                        }
                        Way::Down => {
                            drain(&mut tcp, bytes).await.expect("read");
                            let done = Instant::now();
                            tcp.shutdown().await.expect("shutdown");
                            serving.await.expect("the listening side");
                            done
                        }
                    };
                    done.saturating_duration_since(began)
                })
                .await
                .expect("the transfer to end in time")
            })
            .await
            .expect("the transfer");
        Row {
            elapsed,
            counters: None,
        }
    })
}
