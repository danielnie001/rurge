//! The table behind `block-quic=adaptive` (phase 3 M5 design 6.3; manual:
//! Policies › Parameters, 6.11): QUIC to a host is blocked once, and the
//! table watches whether a TCP session to it follows within a second — the
//! client falling back — in which case QUIC stays blocked; no TCP means the
//! client wanted QUIC, and it goes through from then on.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The hostnames the table remembers (manual: "up to the 200 most recently
/// used hostnames").
pub(crate) const REMEMBERED: usize = 200;
/// How long after a blocked QUIC flow a TCP session to the same host still
/// counts as its fallback.
pub(crate) const WINDOW: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Adaptive {
    /// QUIC was blocked at this moment; waiting to see a TCP session.
    Trying(Instant),
    /// A TCP session came within the window: QUIC stays blocked.
    FellBack,
    /// None came: QUIC goes through from now on.
    Allowed,
}

#[derive(Clone, Copy)]
struct Entry {
    state: Adaptive,
    /// The number of the last QUIC flow that used it: the order the table
    /// forgets in. A counter rather than the `Instant` of that flow, so that
    /// flows which arrive at the same instant are still ordered.
    used: u64,
}

#[derive(Default)]
struct Table {
    entries: HashMap<String, Entry>,
    /// The number the next QUIC flow that adds or uses an entry is marked
    /// with; `on_tcp` does not take one.
    next: u64,
}

impl Table {
    /// A new entry, forgetting the least recently used name when the table is
    /// full.
    fn add(&mut self, key: String, state: Adaptive, used: u64) {
        if self.entries.len() >= REMEMBERED
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(key, Entry { state, used });
    }
}

/// What the engine remembers of the hosts whose QUIC it blocked, while a
/// `block-quic=adaptive` policy decides (design 6.3). One per engine: it
/// outlives a reload, and a change of network does not clear it.
pub(crate) struct QuicAdaptive {
    table: Mutex<Table>,
}

impl QuicAdaptive {
    pub(crate) fn new() -> QuicAdaptive {
        QuicAdaptive {
            table: Mutex::new(Table::default()),
        }
    }

    /// A QUIC flow to `host` that the verdict left to the table, at `now`:
    /// whether it is blocked. The flow counts as a use of the name.
    pub(crate) fn on_quic(&self, host: &str, now: Instant) -> bool {
        let key = key(host);
        let mut table = self.table.lock().unwrap();
        let used = table.next;
        table.next += 1;
        if let Some(entry) = table.entries.get_mut(&key) {
            entry.used = used;
            return match entry.state {
                // still waiting for a TCP session: blocked, and the window is
                // the one the entry began with
                Adaptive::Trying(t) if now.saturating_duration_since(t) <= WINDOW => true,
                Adaptive::Trying(_) => {
                    entry.state = Adaptive::Allowed;
                    tracing::debug!("quic: {key}: no TCP within 1s; QUIC allowed");
                    false
                }
                Adaptive::FellBack => true,
                Adaptive::Allowed => false,
            };
        }
        // the first flow is blocked while the table waits for a TCP session
        table.add(key, Adaptive::Trying(now), used);
        true
    }

    /// A client TCP session to `host`, at `now`: the fallback of a QUIC flow
    /// that was blocked. It settles an entry that is inside its window; one
    /// whose window has gone by is settled the other way, as the flow that
    /// would have found it would have. It does not count as a use of the
    /// name: the table remembers the history of the QUIC flows.
    pub(crate) fn on_tcp(&self, host: &str, now: Instant) {
        let key = key(host);
        let mut table = self.table.lock().unwrap();
        let Some(entry) = table.entries.get_mut(&key) else {
            return;
        };
        match entry.state {
            Adaptive::Trying(t) if now.saturating_duration_since(t) <= WINDOW => {
                entry.state = Adaptive::FellBack;
                tracing::debug!("quic: {key} falls back to TCP; QUIC stays blocked");
            }
            Adaptive::Trying(_) => {
                entry.state = Adaptive::Allowed;
                tracing::debug!("quic: {key}: no TCP within 1s; QUIC allowed");
            }
            Adaptive::FellBack | Adaptive::Allowed => {}
        }
    }

    #[cfg(test)]
    pub(crate) fn state(&self, host: &str) -> Option<Adaptive> {
        let table = self.table.lock().unwrap();
        table.entries.get(&key(host)).map(|entry| entry.state)
    }

    /// Puts an entry where a test wants it, as the newest of the table.
    #[cfg(test)]
    pub(crate) fn set(&self, host: &str, state: Adaptive) {
        let key = key(host);
        let mut table = self.table.lock().unwrap();
        let used = table.next;
        table.next += 1;
        match table.entries.get_mut(&key) {
            Some(entry) => {
                entry.state = state;
                entry.used = used;
            }
            None => table.add(key, state, used),
        }
    }
}

/// The name an entry is kept under: lowercase, without a final dot. The
/// engine's `HostName` is already so; a name from elsewhere may not be.
fn key(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// What this thread logs, down to DEBUG: `tests/common/log_capture.rs` for
    /// a test inside this crate.
    struct LogCapture {
        text: Arc<Mutex<Vec<u8>>>,
        _installed: tracing::subscriber::DefaultGuard,
        // While one dispatcher alone is registered, tracing-core asks only the
        // thread that first reaches a call site whether it is wanted: a site
        // another test reaches first would stay shut on this thread too. With
        // a second one it asks every dispatcher.
        _second: tracing::Dispatch,
    }

    struct Text(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Text {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl LogCapture {
        fn start() -> LogCapture {
            let text = Arc::new(Mutex::new(Vec::new()));
            let writer = text.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .with_ansi(false)
                .without_time()
                .with_target(false)
                .with_writer(move || Text(writer.clone()))
                .finish();
            let installed = tracing::subscriber::set_default(subscriber);
            let second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
            LogCapture {
                text,
                _installed: installed,
                _second: second,
            }
        }

        /// The events so far, each with the level first.
        fn lines(&self) -> Vec<String> {
            String::from_utf8_lossy(&self.text.lock().unwrap())
                .lines()
                .map(|line| line.trim_start().to_string())
                .collect()
        }
    }

    #[test]
    fn a_first_quic_flow_is_blocked_and_tried() {
        let t0 = Instant::now();
        let table = QuicAdaptive::new();
        assert!(table.on_quic("q.test", t0));
        assert_eq!(table.state("q.test"), Some(Adaptive::Trying(t0)));
    }

    #[test]
    fn tcp_within_the_window_keeps_quic_blocked() {
        let t0 = Instant::now();
        let table = QuicAdaptive::new();
        assert!(table.on_quic("q.test", t0));
        // a second is still within it: the table counts both edges as inside
        table.on_tcp("q.test", t0 + Duration::from_millis(1000));
        assert_eq!(table.state("q.test"), Some(Adaptive::FellBack));
        assert!(table.on_quic("q.test", t0 + Duration::from_millis(1001)));
        assert_eq!(table.state("q.test"), Some(Adaptive::FellBack));
    }

    #[test]
    fn tcp_after_the_window_allows_quic() {
        let t0 = Instant::now();
        let table = QuicAdaptive::new();
        assert!(table.on_quic("q.test", t0));
        table.on_tcp("q.test", t0 + Duration::from_millis(1001));
        assert_eq!(table.state("q.test"), Some(Adaptive::Allowed));
        assert!(!table.on_quic("q.test", t0 + Duration::from_millis(1002)));
    }

    #[test]
    fn a_quiet_window_allows_the_next_quic_flow() {
        let t0 = Instant::now();
        let table = QuicAdaptive::new();
        assert!(table.on_quic("q.test", t0));
        // the last moment inside the window blocks, and does not settle it
        assert!(table.on_quic("q.test", t0 + Duration::from_millis(1000)));
        assert_eq!(table.state("q.test"), Some(Adaptive::Trying(t0)));
        // and the first moment after it settles: QUIC goes through
        assert!(!table.on_quic("q.test", t0 + Duration::from_millis(1001)));
        assert_eq!(table.state("q.test"), Some(Adaptive::Allowed));
    }

    #[test]
    fn settled_states_stay() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(60);
        let table = QuicAdaptive::new();
        table.set("fell.test", Adaptive::FellBack);
        table.set("allowed.test", Adaptive::Allowed);
        assert!(table.on_quic("fell.test", later));
        assert!(!table.on_quic("allowed.test", later));
        table.on_tcp("fell.test", later);
        table.on_tcp("allowed.test", later);
        assert_eq!(table.state("fell.test"), Some(Adaptive::FellBack));
        assert_eq!(table.state("allowed.test"), Some(Adaptive::Allowed));
    }

    #[test]
    fn tcp_without_an_entry_records_nothing() {
        let t0 = Instant::now();
        let table = QuicAdaptive::new();
        table.on_tcp("plain.test", t0);
        assert_eq!(table.state("plain.test"), None);
    }

    #[test]
    fn names_are_compared_without_case_or_a_final_dot() {
        let t0 = Instant::now();
        let table = QuicAdaptive::new();
        assert!(table.on_quic("Example.COM.", t0));
        assert_eq!(table.state("example.com"), Some(Adaptive::Trying(t0)));
        table.on_tcp("EXAMPLE.com", t0);
        assert_eq!(table.state("example.com."), Some(Adaptive::FellBack));
    }

    #[test]
    fn the_201st_name_evicts_the_least_recently_used() {
        let t0 = Instant::now();
        let table = QuicAdaptive::new();
        for i in 0..REMEMBERED {
            assert!(table.on_quic(&format!("a{i}"), t0));
        }
        // `a0` is used again, which makes it the newest; `a1` hears of a TCP
        // session, which is not a use of it, so it is now the oldest
        assert!(table.on_quic("a0", t0));
        table.on_tcp("a1", t0);
        assert_eq!(table.state("a1"), Some(Adaptive::FellBack));
        assert!(table.on_quic("a200", t0));
        assert_eq!(table.state("a1"), None, "the least recently used goes");
        assert_eq!(table.state("a0"), Some(Adaptive::Trying(t0)));
        assert_eq!(table.state("a200"), Some(Adaptive::Trying(t0)));
        assert_eq!(table.state("a2"), Some(Adaptive::Trying(t0)));
    }

    #[test]
    fn settling_is_said_at_debug_with_the_name() {
        let log = LogCapture::start();
        let t0 = Instant::now();
        let table = QuicAdaptive::new();
        // a TCP session within the window
        assert!(table.on_quic("fallback.test", t0));
        table.on_tcp("fallback.test", t0 + Duration::from_millis(500));
        // and a window that goes by with none
        assert!(table.on_quic("quiet.test", t0));
        assert!(!table.on_quic("quiet.test", t0 + Duration::from_millis(1001)));
        let lines = log.lines();
        for expected in [
            "DEBUG quic: fallback.test falls back to TCP; QUIC stays blocked",
            "DEBUG quic: quiet.test: no TCP within 1s; QUIC allowed",
        ] {
            assert!(
                lines.iter().any(|line| line == expected),
                "{expected:?} not in {lines:?}"
            );
        }
    }
}
