//! The store itself (`phase 4 M4 design 4`): one queue of entries from the
//! oldest to the newest, costed as they arrive, with what is still coming
//! reserved before it is kept.

use super::{Capture, Limits};
use std::sync::Arc;

/// What a store holds and what it did so far (`phase 4 M4 design 4.4`): the
/// four counts are since the engine started and neither reloading a profile
/// nor clearing the store resets them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub limits: Limits,
    /// What the entries kept and the bodies still coming cost together.
    pub used: u64,
    /// How many entries are kept.
    pub entries: usize,
    /// How many entries were stored, evicted, dropped or stored truncated.
    pub stored: u64,
    pub evicted: u64,
    pub dropped: u64,
    pub truncated: u64,
}

/// One named session and what it holds (`phase 4 M4 design 4.6`); only
/// sessions that have entries are listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    pub name: Arc<str>,
    pub entries: usize,
    pub bytes: u64,
}

/// The captured entries of one engine, kept in memory only (`phase 4 M4
/// design 4.1`).
pub struct CaptureStore {
    limits: Limits,
}

impl CaptureStore {
    /// An empty store with `limits`.
    pub fn new(limits: Limits) -> Arc<CaptureStore> {
        Arc::new(CaptureStore { limits })
    }

    /// The limits this store was made with.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// What the store holds and what it did so far.
    pub fn status(&self) -> Status {
        Status {
            limits: self.limits,
            used: 0,
            entries: 0,
            stored: 0,
            evicted: 0,
            dropped: 0,
            truncated: 0,
        }
    }
}

/// One body on its way into the store (`phase 4 M4 design 4.3`): the chunks
/// that fit, how much went by, and the bytes reserved for them.
struct Taking {
    chunks: Vec<bytes::Bytes>,
    stored: u64,
    size: u64,
    truncated: Option<super::Truncated>,
}

/// A body being captured before its entry is stored (`phase 4 M4 design 4.3`,
/// `6.4`). What it reserved is given back when it is dropped without being
/// committed.
pub(crate) struct Pending {}

impl Pending {
    /// An entry of `capture`'s session, capturing nothing yet.
    pub(crate) fn new(capture: &Capture) -> Pending {
        let _ = capture;
        Pending {}
    }
}
