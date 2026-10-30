//! What outlives a config generation of MITM (`phase 4 M2 design 5.4`): the
//! last `Authority` a generation put in use. A generation that is built with
//! the same material takes it on rather than reading the CA again, so that its
//! cached leaves and its TLS sessions are the ones the connections before it
//! were served with.

use rurge_http::mitm::Authority;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The kinds of hint a decrypted connection gives that are written at WARN at
/// most once per name and hour (`phase 4 M2 design 8.4`, the phase 4 / M2a
/// plan's P22): a client that does not trust the certificate rurge issued, and
/// a host the client is likely pinning. The two are counted apart, so a name
/// that is both is warned about once for each (`8.2`, `8.3`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Hint {
    Untrusted,
    Pinning,
}

/// How long one kind of hint about one name stays quiet (`8.4`).
const QUIET: Duration = Duration::from_secs(3600);

/// How many names the hints are remembered for (`8.4`): a table this full is
/// dropped whole before the next name is written down. A client that names
/// many hosts then costs a repeated warning, not memory that grows with every
/// name it asks for.
const NAMES: usize = 1024;

/// The last `Authority` a published generation had (`phase 4 M2 design 5.4`,
/// the phase 4 / M2a plan's P17). It is remembered when a generation is put in
/// use, never when one is only built: a generation that a failed reload leaves
/// unused leaves nothing behind.
#[derive(Default)]
pub struct MitmShared {
    last: Mutex<Option<Arc<Authority>>>,
    /// When each hint about each name was last written at WARN (`8.4`).
    hints: Mutex<HashMap<(Hint, String), Instant>>,
}

impl MitmShared {
    pub fn new() -> MitmShared {
        MitmShared::default()
    }

    /// Replaces what is remembered: the generation being published signs with
    /// this authority from now on.
    pub fn remember(&self, authority: &Arc<Authority>) {
        *self.last.lock().expect("the last authority") = Some(Arc::clone(authority));
    }

    /// The authority of the last published generation that had one, or `None`
    /// when none had. A generation with the same material takes it on
    /// (`Runtime::build`); `http-api-tls` is to read it as well (`phase 4 M2
    /// design 14.1`).
    pub fn last_live(&self) -> Option<Arc<Authority>> {
        self.last.lock().expect("the last authority").clone()
    }

    /// Whether a hint about `name` is to be written at WARN now (`phase 4 M2
    /// design 8.4`): `true` for the first one and then at most once an hour
    /// for each kind and name, and the moment is written down when it says so.
    /// `false` leaves the table as it was.
    pub(crate) fn warn_now(&self, kind: Hint, name: &str, now: Instant) -> bool {
        let mut hints = self.hints.lock().expect("the hints");
        let key = (kind, name.to_string());
        if let Some(when) = hints.get(&key)
            && now.saturating_duration_since(*when) < QUIET
        {
            return false;
        }
        if hints.len() >= NAMES {
            hints.clear();
        }
        hints.insert(key, now);
        true
    }
}

/// The MITM counts of an engine (`phase 4 M2 design 8.5`, the plan's P23), as
/// `GET /v1/rurge/http` reads them. They live in the engine and not in a
/// generation: a reload does not clear them.
#[derive(Default)]
pub(crate) struct MitmCounters {
    decrypted: AtomicU64,
    untrusted: AtomicU64,
    failed: AtomicU64,
    no_request: AtomicU64,
}

impl MitmCounters {
    /// A connection whose TLS handshake with the client finished.
    pub(crate) fn decrypted(&self) {
        self.decrypted.fetch_add(1, Ordering::Relaxed);
    }

    /// A client that does not trust the certificate rurge issued (`8.2`).
    pub(crate) fn untrusted(&self) {
        self.untrusted.fetch_add(1, Ordering::Relaxed);
    }

    /// A handshake that failed for any other reason (`8.2`).
    pub(crate) fn failed(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }

    /// A client that finished the handshake and closed without a request,
    /// which is what certificate pinning looks like (`8.3`).
    pub(crate) fn no_request(&self) {
        self.no_request.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> MitmCounts {
        MitmCounts {
            decrypted: self.decrypted.load(Ordering::Relaxed),
            untrusted: self.untrusted.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            no_request: self.no_request.load(Ordering::Relaxed),
        }
    }
}

/// What `GET /v1/rurge/http` reports under `mitm` (`phase 4 M2 design 12.3`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MitmCounts {
    pub decrypted: u64,
    pub untrusted: u64,
    pub failed: u64,
    pub no_request: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    /// A hint about a name is written at WARN once and then kept quiet for an
    /// hour — per kind and per name (`phase 4 M2 design 8.4`).
    #[test]
    fn a_hint_is_warned_again_only_after_an_hour() {
        let shared = MitmShared::new();
        let t0 = Instant::now();
        assert!(shared.warn_now(Hint::Untrusted, "origin.test", t0));
        assert!(!shared.warn_now(Hint::Untrusted, "origin.test", t0 + 3599 * SECOND));
        assert!(shared.warn_now(Hint::Untrusted, "origin.test", t0 + 3600 * SECOND));
        assert!(shared.warn_now(Hint::Pinning, "origin.test", t0 + 3600 * SECOND));
        assert!(shared.warn_now(Hint::Untrusted, "other.test", t0 + 3600 * SECOND));
    }

    /// The table stops at `NAMES`: a new name drops it whole, while a hint
    /// that is still quiet neither grows it nor clears it
    /// (`phase 4 M2 design 8.4`).
    #[test]
    fn a_full_table_is_cleared_before_a_new_key() {
        let shared = MitmShared::new();
        let t0 = Instant::now();
        for i in 0..NAMES {
            let name = format!("host{i}.test");
            assert!(shared.warn_now(Hint::Untrusted, &name, t0), "{name}");
        }
        assert!(!shared.warn_now(Hint::Untrusted, "host0.test", t0 + SECOND));
        assert!(shared.warn_now(Hint::Untrusted, "host1024.test", t0 + 2 * SECOND));
        assert!(shared.warn_now(Hint::Untrusted, "host0.test", t0 + 3 * SECOND));
    }
}
