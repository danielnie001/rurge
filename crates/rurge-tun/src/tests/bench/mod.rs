//! The benchmark gate of the TUN's TCP (phase 3 M3 design 6), and the tests of
//! what it is built from.
//!
//! Three measurements, not gates: `b1_single_stream_throughput`,
//! `b2_ten_thousand_connections` and `b3_fake_answer_latency` are ignored. The
//! first two decide whether smoltcp 0.12 is fast enough and small enough to be
//! the TUN's TCP stack, and which defaults of `TunOptions` it is run with; the
//! third measures what a fake DNS answer costs (see `b3`). Each is run on its
//! own, in release, with `--nocapture`; the only runs of this crate that use
//! `-p`:
//!
//! ```text
//! TUN_BENCH_FAR=memory cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b1_single_stream_throughput --nocapture
//! cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b1_single_stream_throughput --nocapture
//! TUN_BENCH_SHARDS=256 TUN_BENCH_MIN=4 TUN_BENCH_TX_MAX=256 TUN_BENCH_BUDGET=64 TUN_BENCH_MTU=1500 \
//!   cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b2_ten_thousand_connections --nocapture
//! cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b3_fake_answer_latency --nocapture
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
//! buffers and nothing else, and every such row is to reach 0.7 Gbit/s; only
//! this mode prints the `>= 0.7` column and a verdict for each setting. Run
//! without it the far end is a TCP connection on the loopback, as design 6
//! asks (this is the default), and those rows stay in the table as the ceiling
//! of that hop, not as a judgment of the TUN: the engine's relay loop over
//! plain TCP, which also moves 8 KiB at a time, carried 110 MiB/s (0.92
//! Gbit/s) on one thread on the development machine (phase 2 / M8c plan,
//! execution record row 40), with no TUN in front of it.
//!
//! B2 is one setting of the stack for each process, given by `TUN_BENCH_SHARDS`
//! (a number), `TUN_BENCH_MIN` and `TUN_BENCH_TX_MAX` (KiB), `TUN_BENCH_BUDGET`
//! (MiB) and `TUN_BENCH_MTU` (bytes); what is not set is the default of
//! `TunOptions` and 1500. It opens ten thousand connections that take 8 KiB
//! and then lie idle, runs a hundred more that carry 4 MiB each way, and
//! prints the peak working set of the process (`PeakWorkingSet64`, read by
//! asking PowerShell, on Windows; `VmHWM` in `/proc/self/status` on Linux).
//! The peak is the process's, and it has the test side's own memory in it: it
//! is printed, and it is not the judged number.
//!
//! B2 is judged on the first of the two readings of rurge's own memory that
//! the run prints: the peak at the end less every item of the test side,
//! against 429,000,000 bytes, with the five counts, which must be 0. The test
//! side is kept small and reported (design 6), and the run prints it item by
//! item: the app stack's socket buffers are counted exactly (a hundred
//! sockets), the pipes and the far ends' own buffers are upper bounds, and the
//! scripted client's table is an estimate. The second reading takes off only
//! the exact item, and is printed because its margin against the line is thin.
//!
//! B3 asks the responder for the fake address of a hundred thousand different
//! names, times each query from `Responder::answer` to the IP packet of the
//! answer, and prints the median, the 99th percentile and the longest against
//! NFR-01's 0.2 ms. It judges nothing.
//!
//! The readings and the numbers they gave are recorded in the phase 3 / M3a
//! plan, execution record rows 8 (B1) and 9 (B2), and in the phase 3 / M3c plan
//! for B3 (and for B2 run again).
//!
//! A measurement is the wall clock of one run on one machine; the CPU time of
//! a process is not read (it is not to be trusted on the development machine).

mod b1;
mod b2;
mod b3;
mod peak;
mod scripted;

use crate::device::MemoryDevice;
use crate::testing::{AppStack, Script, ScriptedTcp, Upstream};
use crate::tests::support::{addr, tun_on};
use b1::{Far, Way, fixture_ceiling, through_the_tun};
use b2::{
    ACTIVE_BYTES, ACTIVE_COUNT, ACTIVE_FIRST, IDLE_COUNT, IDLE_FIRST, IDLE_SENT, MEMORY_LIMIT,
    PHASE_LIMIT, PIPE, Setting, TestSide, WAVE, against_the_line, echo_round_trip, setting_from,
};
use peak::peak_resident_bytes;
use std::io;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio::task::JoinSet;

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
    // the 0.7 column and the verdicts are printed where B1 is judged
    let judged = far == Far::Memory;
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
                    match (counted, judged, gbit >= 0.7) {
                        (false, _, _) => "not counted",
                        (true, false, _) => "-",
                        (true, true, true) => "yes",
                        (true, true, false) => "no",
                    },
                    100.0 * gbit / ceiling_gbit,
                    row.counts()
                );
                settings.push((mtu, tx_max, way, passes));
            }
        }
    }
    if !judged {
        println!(
            "\nthe loopback rows are the ceiling of that hop; B1 is judged on the rows run with TUN_BENCH_FAR=memory"
        );
        return;
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
            "{ACTIVE_COUNT} active connections, {} MiB each way each, in {:.2} s: {packets} packets through the device, {per_second:.0} a second, {:.2} us a packet (whole path, relative only)",
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
        println!(
            "at the end:                             {}  (includes the test side; not the judged number)",
            show(peak)
        );
        let idle = usize::from(IDLE_COUNT);
        let side = TestSide {
            app_buffers: app_buffers as u64,
            table: scripted.table_bytes() as u64,
            pipes: (idle * 2 * PIPE) as u64,
            far_ends: (idle * 2 * PIPE) as u64,
        };
        println!(
            "test side, about: app stack socket buffers {} bytes ({ACTIVE_COUNT} sockets); scripted client's table {} bytes; memory pipes {} bytes at most ({idle} x 2 x {PIPE}); far ends' own buffers {} bytes at most ({idle} x 2 x {PIPE})",
            side.app_buffers, side.table, side.pipes, side.far_ends
        );
        match peak {
            Some(peak) => {
                let (gate, conservative) = (side.gate(peak), side.conservative(peak));
                println!(
                    "rurge's own memory, peak less every test-side item above: {}, limit {MEMORY_LIMIT}: {}  <- the gate",
                    show(Some(gate)),
                    against_the_line(gate)
                );
                println!(
                    "conservative, peak less only the app stack's socket buffers (exact): {}, limit {MEMORY_LIMIT}: {}",
                    show(Some(conservative)),
                    against_the_line(conservative)
                );
            }
            None => println!(
                "the peak is not read on this system, so rurge's own memory cannot be computed"
            ),
        }
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

#[test]
#[ignore = "a measurement, not a gate: run on its own, in release"]
fn b3_fake_answer_latency() {
    let latencies = director().block_on(b3::measure(b3::NAMES));
    b3::report(&latencies);
}
