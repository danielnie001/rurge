//! The benchmark gate of the TUN's TCP (phase 3 M3 design 6), and the tests of
//! what it is built from.
//!
//! Two measurements, not gates: `b1_single_stream_throughput` and
//! `b2_ten_thousand_connections` are ignored. They decide whether smoltcp 0.12
//! is fast enough and small enough to be the TUN's TCP stack, and which
//! defaults of `TunOptions` it is run with. Each is run on its own, in
//! release, with `--nocapture`; the only runs of this crate that use `-p`:
//!
//! ```text
//! TUN_BENCH_FAR=memory cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b1_single_stream_throughput --nocapture
//! cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b1_single_stream_throughput --nocapture
//! TUN_BENCH_SHARDS=256 TUN_BENCH_MIN=4 TUN_BENCH_TX_MAX=256 TUN_BENCH_BUDGET=64 TUN_BENCH_MTU=1500 \
//!   cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b2_ten_thousand_connections --nocapture
//! ```
//!
//! B1 prints a table of single-stream transfers of 64 MiB (`TUN_BENCH_MIB`
//! sets the MiB), one row for each MTU, `tx_max` and direction, and a row
//! for the fixture's own ceiling for each MTU and direction. The numbers to
//! read are each row's Gbit/s (of the application's bytes, 10^9 bit/s)
//! against 0.7 and against 80% of the ceiling, and its four counts, which
//! must be 0 for the row to count: a segment dropped makes smoltcp's sender
//! send the whole window again, and what is measured is no longer the stack.
//!
//! B1 is judged on the rows run with `TUN_BENCH_FAR=memory`: the far end is in
//! memory, on the TUN's thread, so a row is the TUN and the relay's two 8 KiB
//! buffers and nothing else, and every such row is to reach 0.7 Gbit/s. Run
//! without it the far end is a TCP connection on the loopback, as design 6
//! asks (this is the default), and those rows stay in the table as the ceiling
//! of that hop, not as a judgment of the TUN: a loopback socket written 8 KiB
//! at a time, which is what the relay does, is a limit of about 0.7 to
//! 1.2 Gbit/s on the development machine with no TUN in front of it, and the
//! limit moves with the machine's state.
//!
//! B2 is one setting of the stack for each process, given by `TUN_BENCH_SHARDS`
//! (a number), `TUN_BENCH_MIN` and `TUN_BENCH_TX_MAX` (KiB), `TUN_BENCH_BUDGET`
//! (MiB) and `TUN_BENCH_MTU` (bytes); what is not set is the default of
//! `TunOptions` and 1500. It opens ten thousand connections that take 8 KiB
//! and then lie idle, runs a hundred more that carry 4 MiB each way, and
//! prints the peak working set of the process (`PeakWorkingSet64`, read by
//! asking PowerShell, on Windows; `VmHWM` in `/proc/self/status` on Linux).
//! The peak is the process's, and it has the test side's own memory in it.
//!
//! B2 is judged on rurge's own memory: the peak at the end less the test
//! side's own memory, against 429,000,000 bytes, with the five counts, which
//! must be 0. The test side is kept small and reported, and the run prints it
//! item by item: the app stack's socket buffers are counted exactly (a hundred
//! sockets), the pipes and the far ends' own buffers are upper bounds, and the
//! scripted client's table is an estimate.
//!
//! A measurement is the wall clock of one run on one machine; the CPU time of
//! a process is not read (it is not to be trusted on the development machine).

use super::support::{
    WITHIN, addr, checksum, data_segment_v4, eventually, is_syn, rig, segment_v4, syn_v4,
    tcp_header, tcp_payload_len, tun_on, within,
};
use crate::device::{AppEnd, MemoryDevice};
use crate::testing::{AppStack, AppTcp, Script, ScriptedTcp, Upstream};
use crate::{Counters, TunOptions};
use rurge_config::{HostName, ListenerKind};
use smoltcp::wire::{IpAddress, IpCidr};
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio::task::JoinSet;

/// A hundred connections of a `ScriptedTcp` through a TUN to far ends that
/// send 8 KiB each: every one is dialed, every hello is read, and `take`
/// returns once every connection has had its 8 KiB.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripted_tcp_opens_and_takes() {
    let rig = rig(TunOptions::default());
    let dst = addr("192.0.2.10:80");
    rig.dialer.script(
        dst,
        Script::Memory {
            buffer: 1024,
            send: 8192,
        },
    );
    let ports = 10_000..=10_099;
    let mut scripted = ScriptedTcp::new(&rig.app, ports.clone());
    within(
        "a hundred connections to open",
        scripted.open(ports.clone(), dst, 1024),
    )
    .await
    .expect("open");
    within("every connection to take its 8 KiB", scripted.take(8192))
        .await
        .expect("take");
    let dialed = rig.dialer.dialed();
    assert_eq!(dialed.len(), 100, "a session for each connection");
    let mut sources: Vec<u16> = dialed.iter().map(|session| session.src.port()).collect();
    sources.sort_unstable();
    assert_eq!(sources, ports.collect::<Vec<_>>(), "each source port once");
    for session in &dialed {
        assert_eq!(session.src.ip(), addr("198.18.0.1:0").ip());
        assert_eq!(session.dst_host, HostName::Ip(dst.ip()));
        assert_eq!(session.dst_port, 80);
        assert_eq!(session.listener, ListenerKind::Tun);
    }
    // the hellos arrived: the far ends have read them
    eventually("the far ends to read every hello", WITHIN, || {
        rig.dialer.received(dst) == 100 * 1024
    })
    .await;
    assert_eq!(rig.tun.counters(), Counters::default());
}

fn seq_number(packet: &[u8]) -> u32 {
    let (header, _) = tcp_header(packet).expect("a TCP segment");
    u32::from_be_bytes(
        packet[header + 4..header + 8]
            .try_into()
            .expect("four bytes"),
    )
}

fn ack_number(packet: &[u8]) -> u32 {
    let (header, _) = tcp_header(packet).expect("a TCP segment");
    u32::from_be_bytes(
        packet[header + 8..header + 12]
            .try_into()
            .expect("four bytes"),
    )
}

/// The bytes a `ScriptedTcp` has taken are how far the stream got, not the sum
/// of the payloads that came: a segment sent twice and a keep-alive probe (one
/// byte, one below the next expected) carry bytes that are not new. A TUN
/// driven by hand sends them, and what comes back to each is an ACK of where
/// the stream is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripted_tcp_counts_bytes_by_sequence_number() {
    const PEER: u32 = 5_000;
    let (end, mut tun) = AppEnd::crossed(1500, 64);
    let stack = AppStack::start(end);
    let mut scripted = ScriptedTcp::new(&stack, 10_000..=10_000);
    let (app, dst) = (addr("198.18.0.1:10000"), addr("192.0.2.10:80"));
    // the handshake: a SYN with the MSS and no other option, answered; the
    // ACK of the answer carries the hello; `open` returns once that is answered
    let mut opening = Box::pin(scripted.open(10_000..=10_000, dst, 5));
    let hello_in = async {
        let syn = within("the SYN", tun.recv()).await.expect("a packet");
        assert!(is_syn(&syn));
        assert_eq!(checksum(&syn[..20]), 0, "the IPv4 header checksum is right");
        let (header, _) = tcp_header(&syn).expect("a TCP segment");
        assert_eq!(syn[header + 12] >> 4, 6, "a header of 24 bytes");
        assert_eq!(
            syn[header + 20..header + 24],
            [2, 4, 0x05, 0xb4],
            "the MSS, 1460, and no other option"
        );
        let isn = seq_number(&syn);
        tun.send(segment_v4(dst, app, PEER, isn.wrapping_add(1), 0x12))
            .await
            .expect("send the SYN-ACK");
        let hello = within("the hello", tun.recv()).await.expect("a packet");
        assert_eq!(tcp_payload_len(&hello), 5);
        assert_eq!(seq_number(&hello), isn.wrapping_add(1));
        assert_eq!(ack_number(&hello), PEER + 1);
        isn
    };
    let isn = tokio::select! {
        biased;
        opened = &mut opening => panic!("open returned before its hello was acknowledged: {opened:?}"),
        isn = hello_in => isn,
    };
    let (opened, ()) = tokio::join!(within("the connection to open", &mut opening), async {
        tun.send(segment_v4(dst, app, PEER + 1, isn.wrapping_add(6), 0x10))
            .await
            .expect("send the ACK of the hello");
    });
    opened.expect("open");
    drop(opening);

    let thousand = vec![7u8; 1000];
    let first = PEER + 1;
    let data = |seq: u32| data_segment_v4(dst, app, seq, isn.wrapping_add(6), 0x18, &thousand);
    // one thousand bytes, the same again, a keep-alive probe: 2001 bytes of
    // payload in all, 1000 of them new
    let mut taking = Box::pin(scripted.take(2000));
    let script = async {
        let probe = data_segment_v4(dst, app, first + 999, isn.wrapping_add(6), 0x10, &[0]);
        for segment in [data(first), data(first), probe] {
            tun.send(segment).await.expect("send");
            let ack = within("an ACK", tun.recv()).await.expect("a packet");
            assert_eq!(tcp_payload_len(&ack), 0, "a plain ACK");
            assert_eq!(ack_number(&ack), first + 1000, "of where the stream is");
        }
    };
    tokio::select! {
        biased;
        taken = &mut taking => panic!("2000 bytes taken after 1000 and two repeats: {taken:?}"),
        () = script => {}
    }
    // the next thousand completes it
    let (taken, ack) = tokio::join!(within("2000 bytes to be taken", &mut taking), async {
        tun.send(data(first + 1000)).await.expect("send");
        within("an ACK", tun.recv()).await.expect("a packet")
    });
    taken.expect("take");
    assert_eq!(ack_number(&ack), first + 2000);
}

/// A FIN takes a sequence number but is not a byte of the stream: a segment of
/// a thousand bytes and a FIN has the connection take a thousand, and is
/// acknowledged at a thousand and one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripted_tcp_does_not_count_a_fin_as_a_byte() {
    const PEER: u32 = 5_000;
    let (end, mut tun) = AppEnd::crossed(1500, 64);
    let stack = AppStack::start(end);
    let mut scripted = ScriptedTcp::new(&stack, 10_000..=10_000);
    let (app, dst) = (addr("198.18.0.1:10000"), addr("192.0.2.10:80"));
    // an established connection, with no hello to acknowledge
    let answering = async {
        let syn = within("the SYN", tun.recv()).await.expect("a packet");
        let isn = seq_number(&syn);
        tun.send(segment_v4(dst, app, PEER, isn.wrapping_add(1), 0x12))
            .await
            .expect("send the SYN-ACK");
        // the ACK that ends the handshake, with nothing in it
        let ack = within("the ACK of the SYN-ACK", tun.recv())
            .await
            .expect("a packet");
        assert_eq!(tcp_payload_len(&ack), 0);
        assert_eq!(ack_number(&ack), PEER + 1);
        isn
    };
    let (opened, isn) = tokio::join!(
        within(
            "the connection to open",
            scripted.open(10_000..=10_000, dst, 0)
        ),
        answering
    );
    opened.expect("open");

    let first = PEER + 1;
    // FIN, PSH and ACK
    let last = data_segment_v4(dst, app, first, isn.wrapping_add(1), 0x19, &[7u8; 1000]);
    let mut taking = Box::pin(scripted.take(1001));
    let script = async {
        tun.send(last).await.expect("send");
        let ack = within("an ACK", tun.recv()).await.expect("a packet");
        assert_eq!(ack_number(&ack), first + 1001, "the ACK covers the FIN");
    };
    tokio::select! {
        biased;
        taken = &mut taking => panic!("1001 bytes taken after a thousand and a FIN: {taken:?}"),
        () = script => {}
    }
    drop(taking);
    within("a thousand bytes to be taken", scripted.take(1000))
        .await
        .expect("take");
}

/// A range with a port in it that cannot be opened is refused whole: nothing is
/// registered, and no SYN is sent for the ports before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scripted_tcp_checks_every_port_before_it_sends_one() {
    let (end, mut tun) = AppEnd::crossed(1500, 64);
    let stack = AppStack::start(end);
    let mut scripted = ScriptedTcp::new(&stack, 10_000..=10_001);
    let dst = addr("192.0.2.10:80");
    // 10002 is not one of the ports it was given
    let error = scripted
        .open(10_000..=10_002, dst, 0)
        .await
        .expect_err("a port it was not given");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(scripted.table_bytes(), 0, "nothing is registered");
    // what is injected goes out in order: the other end gets this packet
    // first, so no SYN went before it
    let marker = syn_v4(addr("198.18.0.1:50000"), dst);
    stack.inject(marker.clone());
    assert_eq!(within("a packet", tun.recv()).await, Some(marker));
}

/// A tokio runtime of the current-thread kind on a thread of its own. What is
/// spawned on it runs on that thread and on no other, so a measurement puts
/// the TUN on one thread, the app's stack on a second and the far end on a
/// third: the TUN's cost is the cost of one thread, as design 6 asks.
struct Lane {
    handle: tokio::runtime::Handle,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Lane {
    fn new(name: &str) -> Lane {
        let (handle_tx, handle_rx) = std::sync::mpsc::channel();
        let (stop, stopped) = oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("a runtime");
                handle_tx
                    .send(runtime.handle().clone())
                    .expect("the lane's owner");
                // ends when the lane's owner lets go of `stop`
                runtime.block_on(async {
                    let _ = stopped.await;
                });
            })
            .expect("a thread");
        Lane {
            handle: handle_rx.recv().expect("the lane's runtime"),
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.handle.spawn(future)
    }
}

impl Drop for Lane {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The runtime a measurement is directed from: it waits and times nothing.
fn director() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
}

/// What a transfer is bounded by.
const LIMIT: Duration = Duration::from_secs(120);
/// What a phase of B2 is bounded by.
const PHASE_LIMIT: Duration = Duration::from_secs(600);
/// Packets a memory device holds each way.
const DEVICE_ROOM: usize = 8192;
/// Written from: zeros, shared by every writer, so that the test side holds one copy.
static ZEROS: [u8; 64 * 1024] = [0; 64 * 1024];
/// What a reader reads into at a time.
const READ_CHUNK: usize = 16 * 1024;

/// Writes `bytes` bytes.
async fn blast(to: &mut (impl AsyncWrite + Unpin), bytes: u64) -> io::Result<()> {
    let mut left = bytes;
    while left > 0 {
        let n = left.min(ZEROS.len() as u64) as usize;
        to.write_all(&ZEROS[..n]).await?;
        left -= n as u64;
    }
    Ok(())
}

/// Reads `bytes` bytes, whatever they are.
async fn drain(from: &mut (impl AsyncRead + Unpin), bytes: u64) -> io::Result<()> {
    let mut chunk = vec![0; READ_CHUNK];
    let mut left = bytes;
    while left > 0 {
        let n = from.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        left = left.saturating_sub(n as u64);
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Way {
    /// The app writes, the far end reads.
    Up,
    /// The far end writes, the app reads.
    Down,
}

impl Way {
    fn name(self) -> &'static str {
        match self {
            Way::Up => "up",
            Way::Down => "down",
        }
    }
}

/// One transfer of B1: how long it took, and what the TUN counted while it
/// did (`None` when there was no TUN).
struct Row {
    elapsed: Duration,
    counters: Option<Counters>,
}

impl Row {
    fn gbit(&self, bytes: u64) -> f64 {
        bytes as f64 * 8.0 / self.elapsed.as_secs_f64() / 1e9
    }

    /// The four counts that make a row not count when one is not 0.
    fn counted(&self) -> bool {
        self.counters.is_none_or(|counters| {
            counters.beyond_window == 0
                && counters.queue_full == 0
                && counters.rst_limited == 0
                && counters.stack_panics == 0
        })
    }

    fn counts(&self) -> String {
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
enum Far {
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
    fn from_env() -> Far {
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
fn through_the_tun(mtu: usize, tx_max: usize, way: Way, bytes: u64, far: Far) -> Row {
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
fn fixture_ceiling(mtu: usize, way: Way, bytes: u64) -> Row {
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

#[test]
#[ignore = "a measurement, not a gate: run on its own, in release"]
fn b1_single_stream_throughput() {
    const MIB: u64 = 1 << 20;
    let mib: u64 = std::env::var("TUN_BENCH_MIB")
        .map_or(64, |v| v.parse().expect("TUN_BENCH_MIB is a number of MiB"));
    let bytes = mib * MIB;
    let far = Far::from_env();
    println!(
        "\nB1 single-stream throughput, {mib} MiB each way: the TUN on one thread, the app's stack on a second, {}",
        match far {
            Far::Loopback => "the far end on a third",
            Far::Memory => "the far end in memory, on the TUN's thread (TUN_BENCH_FAR=memory)",
        }
    );
    println!(
        "{:>5} {:>8} {:>5} {:>10} {:>8} {:>8} {:>12} {:>10}  counts",
        "mtu", "tx_max", "way", "bytes", "seconds", "Gbit/s", ">= 0.7", "of ceiling"
    );
    // (mtu, tx_max, way, passes the bar)
    let mut settings = Vec::new();
    for mtu in [1500, 9000] {
        for way in [Way::Up, Way::Down] {
            let ceiling = fixture_ceiling(mtu, way, bytes);
            let ceiling_gbit = ceiling.gbit(bytes);
            println!(
                "{mtu:>5} {:>8} {:>5} {bytes:>10} {:>8.3} {ceiling_gbit:>8.3} {:>12} {:>10}  {}",
                "ceiling",
                way.name(),
                ceiling.elapsed.as_secs_f64(),
                "-",
                "-",
                ceiling.counts()
            );
            for tx_max in [256 * 1024, MIB as usize] {
                let row = through_the_tun(mtu, tx_max, way, bytes, far);
                let gbit = row.gbit(bytes);
                let counted = row.counted();
                let passes = counted && (gbit >= 0.7 || gbit >= 0.8 * ceiling_gbit);
                println!(
                    "{mtu:>5} {:>8} {:>5} {bytes:>10} {:>8.3} {gbit:>8.3} {:>12} {:>9.0}%  {}",
                    format!("{} KiB", tx_max / 1024),
                    way.name(),
                    row.elapsed.as_secs_f64(),
                    match (counted, gbit >= 0.7) {
                        (false, _) => "not counted",
                        (true, true) => "yes",
                        (true, false) => "no",
                    },
                    100.0 * gbit / ceiling_gbit,
                    row.counts()
                );
                settings.push((mtu, tx_max, way, passes));
            }
        }
    }
    println!(
        "\na setting passes when both its directions count and reach 0.7 Gbit/s or 80% of the ceiling:"
    );
    for mtu in [1500, 9000] {
        for tx_max in [256 * 1024, MIB as usize] {
            let passes = |way| {
                settings
                    .iter()
                    .any(|&(m, t, w, passes)| (m, t, w) == (mtu, tx_max, way) && passes)
            };
            println!(
                "  mtu {mtu:>4}, tx_max {:>7}: up {}, down {} -> {}",
                format!("{} KiB", tx_max / 1024),
                if passes(Way::Up) { "yes" } else { "no" },
                if passes(Way::Down) { "yes" } else { "no" },
                if passes(Way::Up) && passes(Way::Down) {
                    "passes"
                } else {
                    "does not pass"
                }
            );
        }
    }
}

/// The setting of B2: the stack's options and the device's MTU.
struct Setting {
    options: TunOptions,
    mtu: usize,
}

/// The setting `var` (the environment, or a stand-in for it) gives: what is
/// not there is the default of `TunOptions` and an MTU of 1500.
fn setting_from(var: impl Fn(&str) -> Option<String>) -> Setting {
    let number = |name: &str| -> Option<usize> {
        var(name).map(|value| {
            value
                .parse()
                .unwrap_or_else(|_| panic!("{name} is a number, not {value:?}"))
        })
    };
    let mut options = TunOptions::default();
    if let Some(shards) = number("TUN_BENCH_SHARDS") {
        options.shards = shards;
    }
    if let Some(kib) = number("TUN_BENCH_MIN") {
        options.buffer_min = kib * 1024;
    }
    if let Some(kib) = number("TUN_BENCH_TX_MAX") {
        options.tx_max = kib * 1024;
    }
    if let Some(mib) = number("TUN_BENCH_BUDGET") {
        options.budget = mib << 20;
    }
    Setting {
        options,
        mtu: number("TUN_BENCH_MTU").unwrap_or(1500),
    }
}

#[test]
fn the_setting_of_b2_comes_from_the_environment() {
    let from = |pairs: &'static [(&'static str, &'static str)]| {
        setting_from(move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    };
    let nothing = from(&[]);
    let default = TunOptions::default();
    assert_eq!(nothing.mtu, 1500);
    assert_eq!(nothing.options.shards, default.shards);
    assert_eq!(nothing.options.buffer_min, default.buffer_min);
    assert_eq!(nothing.options.tx_max, default.tx_max);
    assert_eq!(nothing.options.budget, default.budget);
    // KiB, KiB, MiB, a count, bytes
    let all = from(&[
        ("TUN_BENCH_SHARDS", "16"),
        ("TUN_BENCH_MIN", "4"),
        ("TUN_BENCH_TX_MAX", "1024"),
        ("TUN_BENCH_BUDGET", "128"),
        ("TUN_BENCH_MTU", "9000"),
    ]);
    assert_eq!(all.options.shards, 16);
    assert_eq!(all.options.buffer_min, 4 * 1024);
    assert_eq!(all.options.tx_max, 1 << 20);
    assert_eq!(all.options.budget, 128 << 20);
    assert_eq!(all.mtu, 9000);
}

/// The most half-opens B2 asks of the TUN at a time.
const WAVE: u16 = 1000;
const IDLE_FIRST: u16 = 10_000;
const IDLE_COUNT: u16 = 10_000;
const ACTIVE_FIRST: u16 = 40_000;
const ACTIVE_COUNT: u16 = 100;
/// What each active connection carries each way.
const ACTIVE_BYTES: u64 = 4 << 20;
/// What the far end of an idle connection sends, and what its pipe holds.
const IDLE_SENT: usize = 8192;
const PIPE: usize = 1024;
/// What the benchmark may use of memory: 300 MB / 0.7, in decimal.
const MEMORY_LIMIT: u64 = 429_000_000;

/// A hundred of these run at once: the bytes written come back from the echo
/// server, and are read.
async fn echo_round_trip(tcp: AppTcp, bytes: u64) -> io::Result<()> {
    let (mut read, mut write) = tokio::io::split(tcp);
    let up = async {
        blast(&mut write, bytes).await?;
        write.shutdown().await
    };
    tokio::try_join!(up, drain(&mut read, bytes))?;
    Ok(())
}

#[test]
#[ignore = "a measurement, not a gate: run on its own, in release"]
fn b2_ten_thousand_connections() {
    let Setting { options, mtu } = setting_from(|name| std::env::var(name).ok());
    println!(
        "\nB2 ten thousand idle connections and a hundred active ones: {} shards, buffer_min {} bytes, tx_max {} bytes, budget {} bytes, mtu {mtu}",
        options.shards, options.buffer_min, options.tx_max, options.budget
    );
    let baseline = peak_resident_bytes();
    let idle_dst = addr("192.0.2.10:80");
    let active_dst = addr("192.0.2.20:80");
    director().block_on(async {
        let (rurge, app, upstream) = (
            Lane::new("rurge"),
            Lane::new("app"),
            Lane::new("upstream"),
        );
        let echo = upstream
            .spawn(Upstream::echo())
            .await
            .expect("the echo server");
        let (device, end) = MemoryDevice::pair(mtu, DEVICE_ROOM);
        let (tun, dialer) = rurge
            .spawn(async move { tun_on(device, options) })
            .await
            .expect("the TUN");
        dialer.script(
            idle_dst,
            Script::Memory {
                buffer: PIPE,
                send: IDLE_SENT,
            },
        );
        dialer.script(active_dst, Script::Upstream(echo.addr()));
        let stack = app
            .spawn(async move { AppStack::start(end) })
            .await
            .expect("the app's stack");

        // ten thousand connections, a wave at a time, each wave established
        // before the next is asked for; every one has sent a hello of 1 KiB
        // and taken 8 KiB
        let (scripted, opened_in) = app
            .spawn({
                let stack = stack.clone();
                async move {
                    tokio::time::timeout(PHASE_LIMIT, async {
                        let mut scripted =
                            ScriptedTcp::new(&stack, IDLE_FIRST..=IDLE_FIRST + IDLE_COUNT - 1);
                        let began = Instant::now();
                        for wave in 0..IDLE_COUNT / WAVE {
                            let first = IDLE_FIRST + wave * WAVE;
                            scripted
                                .open(first..=first + WAVE - 1, idle_dst, 1024)
                                .await
                                .expect("a wave to open");
                        }
                        scripted
                            .take(IDLE_SENT)
                            .await
                            .expect("every connection to take its bytes");
                        (scripted, began.elapsed())
                    })
                    .await
                    .expect("the idle connections to be opened in time")
                }
            })
            .await
            .expect("the idle connections");
        let after_idle = peak_resident_bytes();
        println!(
            "{} connections opened and taken in {:.2} s; {} sockets in the TUN, {} bytes of buffers",
            IDLE_COUNT,
            opened_in.as_secs_f64(),
            tun.sockets_in_use(),
            tun.budget_in_use()
        );

        // a hundred connections that carry 4 MiB each way, with the idle ones in place
        let (active_in, packets, app_buffers) = app
            .spawn({
                let stack = stack.clone();
                async move {
                    tokio::time::timeout(PHASE_LIMIT, async {
                        let mut connections = Vec::new();
                        for port in ACTIVE_FIRST..ACTIVE_FIRST + ACTIVE_COUNT {
                            connections.push(
                                stack
                                    .connect_from(port, active_dst)
                                    .await
                                    .expect("an active connection"),
                            );
                        }
                        let app_buffers = stack.buffer_bytes();
                        let before = stack.packets();
                        let began = Instant::now();
                        let mut transfers = JoinSet::new();
                        for tcp in connections {
                            transfers.spawn(echo_round_trip(tcp, ACTIVE_BYTES));
                        }
                        while let Some(done) = transfers.join_next().await {
                            done.expect("a transfer's task").expect("a transfer");
                        }
                        let elapsed = began.elapsed();
                        let after = stack.packets();
                        (
                            elapsed,
                            (after.0 - before.0) + (after.1 - before.1),
                            app_buffers,
                        )
                    })
                    .await
                    .expect("the active connections to carry their bytes in time")
                }
            })
            .await
            .expect("the active connections");
        let peak = peak_resident_bytes();
        let counters = tun.counters();

        let per_second = packets as f64 / active_in.as_secs_f64();
        println!(
            "{ACTIVE_COUNT} active connections, {} MiB each way each, in {:.2} s: {packets} packets through the device, {per_second:.0} a second, {:.2} us a packet",
            ACTIVE_BYTES >> 20,
            active_in.as_secs_f64(),
            1e6 / per_second
        );
        let show = |bytes: Option<u64>| {
            bytes.map_or("unknown".to_owned(), |bytes| {
                format!("{bytes} bytes ({:.1} MiB)", bytes as f64 / (1 << 20) as f64)
            })
        };
        println!("process peak working set before the TUN: {}", show(baseline));
        println!("after the idle connections:             {}", show(after_idle));
        println!("at the end:                             {}", show(peak));
        if let Some(peak) = peak {
            println!(
                "limit {MEMORY_LIMIT} bytes: {}",
                if peak <= MEMORY_LIMIT { "within" } else { "OVER" }
            );
        }
        let idle = usize::from(IDLE_COUNT);
        println!(
            "test side, about: app stack socket buffers {} bytes ({ACTIVE_COUNT} sockets); scripted client's table {} bytes; memory pipes {} bytes at most ({idle} x 2 x {PIPE}); far ends' own buffers {} bytes at most ({idle} x 2 x {PIPE})",
            app_buffers,
            scripted.table_bytes(),
            idle * 2 * PIPE,
            idle * 2 * PIPE
        );
        let counted = counters.queue_full == 0
            && counters.busy == 0
            && counters.rst_limited == 0
            && counters.beyond_window == 0
            && counters.stack_panics == 0;
        println!(
            "counts: queue_full {} busy {} rst_limited {} beyond_window {} stack_panics {}{}",
            counters.queue_full,
            counters.busy,
            counters.rst_limited,
            counters.beyond_window,
            counters.stack_panics,
            if counted { "" } else { "  -- not counted" }
        );
        // kept until here: dropped earlier, the app's stack would answer the
        // packets of the idle connections with resets
        drop(scripted);
    });
}

/// The peak working set of this process so far, in bytes, from the operating
/// system; `None` where this does not read it.
fn peak_resident_bytes() -> Option<u64> {
    #[cfg(windows)]
    {
        // a read-only system tool, started by the test itself
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command"])
            .arg(format!(
                "(Get-Process -Id {}).PeakWorkingSet64",
                std::process::id()
            ))
            .output()
            .ok()?;
        parse_count(&String::from_utf8_lossy(&output.stdout))
    }
    #[cfg(target_os = "linux")]
    {
        parse_vm_hwm(&std::fs::read_to_string("/proc/self/status").ok()?)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

/// The number a command printed on its last line.
fn parse_count(output: &str) -> Option<u64> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .next_back()?
        .parse()
        .ok()
}

/// `VmHWM` of `/proc/<pid>/status`, in bytes: its peak resident set.
fn parse_vm_hwm(status: &str) -> Option<u64> {
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?;
    let kib: u64 = line.trim().strip_suffix("kB")?.trim().parse().ok()?;
    Some(kib * 1024)
}

#[test]
fn the_peak_is_read_from_what_the_system_prints() {
    assert_eq!(parse_count("123456789\r\n"), Some(123_456_789));
    assert_eq!(parse_count("\r\nwarning\r\n42\r\n\r\n"), Some(42));
    assert_eq!(parse_count(""), None);
    assert_eq!(parse_count("not a number"), None);
    let status = "Name:\trurge\nVmPeak:\t  900000 kB\nVmHWM:\t  123456 kB\nVmRSS:\t   100000 kB\n";
    assert_eq!(parse_vm_hwm(status), Some(123_456 * 1024));
    assert_eq!(parse_vm_hwm("Name:\trurge\n"), None);
    assert_eq!(parse_vm_hwm("VmHWM:\tmuch\n"), None);
}
