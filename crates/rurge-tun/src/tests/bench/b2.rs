//! The machinery of B2, the memory measurement with ten thousand idle
//! connections: the setting it is run with, what the test side holds of the
//! memory it measures, and the traffic of the active connections.

use super::{blast, drain};
use crate::TunOptions;
use crate::testing::AppTcp;
use std::io;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// What a phase of B2 is bounded by.
pub(super) const PHASE_LIMIT: Duration = Duration::from_secs(600);

/// The setting of B2: the stack's options and the device's MTU.
pub(super) struct Setting {
    pub(super) options: TunOptions,
    pub(super) mtu: usize,
}

/// The setting `var` (the environment, or a stand-in for it) gives: what is
/// not there is the default of `TunOptions` and an MTU of 1500.
pub(super) fn setting_from(var: impl Fn(&str) -> Option<String>) -> Setting {
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
pub(super) const WAVE: u16 = 1000;
pub(super) const IDLE_FIRST: u16 = 10_000;
pub(super) const IDLE_COUNT: u16 = 10_000;
pub(super) const ACTIVE_FIRST: u16 = 40_000;
pub(super) const ACTIVE_COUNT: u16 = 100;
/// What each active connection carries each way.
pub(super) const ACTIVE_BYTES: u64 = 4 << 20;
/// What the far end of an idle connection sends, and what its pipe holds.
pub(super) const IDLE_SENT: usize = 8192;
pub(super) const PIPE: usize = 1024;
/// What the benchmark may use of memory: 300 MB / 0.7, in decimal.
pub(super) const MEMORY_LIMIT: u64 = 429_000_000;

/// What the test side of B2 holds, item by item, in bytes.
pub(super) struct TestSide {
    /// The sockets of the app's stack: counted exactly.
    pub(super) app_buffers: u64,
    /// The scripted client's table: an estimate.
    pub(super) table: u64,
    /// The memory pipes: at most.
    pub(super) pipes: u64,
    /// The far ends' own buffers: at most.
    pub(super) far_ends: u64,
}

impl TestSide {
    /// Rurge's own memory as the gate reads it: the peak less every item of the
    /// test side (design 6: the test side is kept small and reported).
    pub(super) fn gate(&self, peak: u64) -> u64 {
        peak.saturating_sub(self.app_buffers + self.table + self.pipes + self.far_ends)
    }

    /// The same taking off only the item that is exact, printed because the
    /// margin of this reading is thin.
    pub(super) fn conservative(&self, peak: u64) -> u64 {
        peak.saturating_sub(self.app_buffers)
    }
}

/// Against the line of memory: `within` it or `OVER` it.
pub(super) fn against_the_line(bytes: u64) -> &'static str {
    if bytes <= MEMORY_LIMIT {
        "within"
    } else {
        "OVER"
    }
}

#[test]
fn b2_takes_the_test_side_off_the_peak() {
    // the numbers of a run
    let side = TestSide {
        app_buffers: 32_767_900,
        table: 702_464,
        pipes: 20_480_000,
        far_ends: 20_480_000,
    };
    let peak = 460_713_984;
    assert_eq!(side.gate(peak), 386_283_620, "every item off");
    assert_eq!(
        side.conservative(peak),
        427_946_084,
        "only the exact one off"
    );
    // never less than nothing
    assert_eq!(side.gate(1_000), 0);
    assert_eq!(side.conservative(1_000), 0);
    // the line is inclusive
    assert_eq!(against_the_line(MEMORY_LIMIT), "within");
    assert_eq!(against_the_line(MEMORY_LIMIT + 1), "OVER");
}

/// A hundred of these run at once: the bytes written come back from the echo
/// server, and are read.
pub(super) async fn echo_round_trip(tcp: AppTcp, bytes: u64) -> io::Result<()> {
    let (mut read, mut write) = tokio::io::split(tcp);
    let up = async {
        blast(&mut write, bytes).await?;
        write.shutdown().await
    };
    tokio::try_join!(up, drain(&mut read, bytes))?;
    Ok(())
}
