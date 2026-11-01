//! The store itself (`phase 4 M4 design 4`): one queue of entries from the
//! oldest to the newest, each costed as soon as it is stored, and the bytes
//! of a body reserved while it is still coming.
//!
//! Every byte kept is costed: `kept` is what the queue holds and `reserved`
//! what the bodies still arriving hold, so `kept + reserved` is the store's
//! `used` and it never goes over the total (`phase 4 M4 design 4.3`).

use super::{Body, Capture, ENTRY_OVERHEAD, Entry, Limits, Truncated};
use bytes::{Bytes, BytesMut};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

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
/// design 4.1`): one for the engine's lifetime, kept across reloads.
pub struct CaptureStore {
    limits: Limits,
    inner: Mutex<Inner>,
}

/// What the lock holds: the queue, what is reserved for the bodies still
/// coming, and the four counts.
struct Inner {
    /// From the oldest entry to the newest.
    queue: VecDeque<Kept>,
    /// What the queue costs together.
    kept: u64,
    /// What the bodies still coming reserved.
    reserved: u64,
    stored: u64,
    evicted: u64,
    dropped: u64,
    truncated: u64,
}

/// One stored entry and what it costs.
struct Kept {
    entry: Arc<Entry>,
    cost: u64,
}

impl CaptureStore {
    /// An empty store with `limits`.
    pub fn new(limits: Limits) -> Arc<CaptureStore> {
        Arc::new(CaptureStore {
            limits,
            inner: Mutex::new(Inner {
                queue: VecDeque::new(),
                kept: 0,
                reserved: 0,
                stored: 0,
                evicted: 0,
                dropped: 0,
                truncated: 0,
            }),
        })
    }

    /// The limits this store was made with.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// What the store holds and what it did so far (`phase 4 M4 design 4.4`).
    pub fn status(&self) -> Status {
        let inner = lock(&self.inner);
        Status {
            limits: self.limits,
            used: inner.kept + inner.reserved,
            entries: inner.queue.len(),
            stored: inner.stored,
            evicted: inner.evicted,
            dropped: inner.dropped,
            truncated: inner.truncated,
        }
    }

    /// The sessions that have entries, by name (`phase 4 M4 design 4.6`).
    pub fn sessions(&self) -> Vec<SessionSummary> {
        let inner = lock(&self.inner);
        let mut by_name: BTreeMap<Arc<str>, (usize, u64)> = BTreeMap::new();
        for kept in &inner.queue {
            let slot = by_name.entry(kept.entry.session.clone()).or_default();
            slot.0 += 1;
            slot.1 += kept.cost;
        }
        drop(inner);
        by_name
            .into_iter()
            .map(|(name, (entries, bytes))| SessionSummary {
                name,
                entries,
                bytes,
            })
            .collect()
    }

    /// The entries of `name`, from the oldest to the newest (`phase 4 M4
    /// design 7.3`). The lock is let go once the entries are in hand, so a
    /// caller that is slow to write them out holds no one up.
    pub fn session(&self, name: &str) -> Vec<Arc<Entry>> {
        let inner = lock(&self.inner);
        inner
            .queue
            .iter()
            .filter(|kept| kept.entry.session.as_ref() == name)
            .map(|kept| kept.entry.clone())
            .collect()
    }

    /// The entry whose request had `id`, if it is still kept.
    pub fn entry(&self, id: u64) -> Option<Arc<Entry>> {
        let inner = lock(&self.inner);
        inner
            .queue
            .iter()
            .find(|kept| kept.entry.id == id)
            .map(|kept| kept.entry.clone())
    }

    /// Drop the entries of `name`, or every entry when `session` is `None`,
    /// and say how many went (`phase 4 M4 design 7.6`). The four counts stay.
    pub fn clear(&self, session: Option<&str>) -> usize {
        let mut inner = lock(&self.inner);
        match session {
            None => {
                let gone = inner.queue.len();
                inner.queue.clear();
                inner.kept = 0;
                gone
            }
            Some(name) => {
                let before = inner.queue.len();
                let mut freed = 0;
                inner.queue.retain(|kept| {
                    let keep = kept.entry.session.as_ref() != name;
                    if !keep {
                        freed += kept.cost;
                    }
                    keep
                });
                inner.kept -= freed;
                before - inner.queue.len()
            }
        }
    }

    /// Store an entry made elsewhere, costed as if it came in through a
    /// `Pending`; false when it did not fit (`phase 4 / M4a plan P6`).
    #[cfg(any(test, feature = "testing"))]
    pub fn put(&self, entry: Entry) -> bool {
        let mut inner = lock(&self.inner);
        inner.insert(entry, 0, self.limits.total)
    }

    /// Reserve `n` bytes for a body still coming; false when not even an
    /// empty store has the room (`phase 4 M4 design 4.3`).
    fn reserve(&self, n: u64) -> bool {
        let mut inner = lock(&self.inner);
        inner.reserve(n, self.limits.total)
    }
}

/// One body on its way into the store (`phase 4 M4 design 4.3`): the chunks
/// that fit, how many bytes went by, and why it stops short.
struct Taking {
    chunks: Vec<Bytes>,
    stored: u64,
    size: u64,
    truncated: Option<Truncated>,
}

/// A body being captured before its entry is stored (`phase 4 M4 design 4.3`,
/// `6.4`). What it reserved is given back when it is dropped uncommitted.
pub(crate) struct Pending {
    store: Arc<CaptureStore>,
    request: Taking,
    response: Taking,
    /// What the two bodies reserved so far.
    held: u64,
}

impl Pending {
    /// An entry of `capture`'s session, capturing nothing yet.
    pub(crate) fn new(capture: &Capture) -> Pending {
        Pending {
            store: capture.store.clone(),
            request: Taking::empty(),
            response: Taking::empty(),
            held: 0,
        }
    }

    /// Take the next chunk of the request's body.
    pub(crate) fn request_chunk(&mut self, data: &[u8]) {
        self.chunk(Axis::Request, data);
    }

    /// Take the next chunk of the response's body.
    pub(crate) fn response_chunk(&mut self, data: &[u8]) {
        self.chunk(Axis::Response, data);
    }

    /// Let `make` build the entry from the two bodies and store it; false
    /// when the entry was dropped (`phase 4 M4 design 4.4`).
    pub(crate) fn commit(mut self, make: impl FnOnce(Body, Body) -> Entry) -> bool {
        let held = std::mem::take(&mut self.held);
        let request = std::mem::replace(&mut self.request, Taking::empty()).finish();
        let response = std::mem::replace(&mut self.response, Taking::empty()).finish();
        let entry = make(request, response);
        let mut inner = lock(&self.store.inner);
        inner.insert(entry, held, self.store.limits.total)
    }

    /// Take `data` into one of the two bodies.
    fn chunk(&mut self, axis: Axis, data: &[u8]) {
        let budget = self.store.limits.body;
        let taking = match axis {
            Axis::Request => &mut self.request,
            Axis::Response => &mut self.response,
        };
        take(taking, &mut self.held, &self.store, data, budget);
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if self.held > 0 {
            let mut inner = lock(&self.store.inner);
            inner.reserved -= self.held;
            self.held = 0;
        }
    }
}

/// Which of the two bodies of a request a chunk goes to.
#[derive(Clone, Copy)]
enum Axis {
    Request,
    Response,
}

/// Take `data` into `taking`: `size` counts every byte that came, at most
/// what is left of `budget` is kept, and the bytes kept are reserved from
/// `store` first, evicting the oldest entries if need be (`phase 4 M4 design
/// 4.3`). A body that is truncated already only counts.
fn take(taking: &mut Taking, held: &mut u64, store: &CaptureStore, data: &[u8], budget: u64) {
    if data.is_empty() {
        return;
    }
    taking.size += data.len() as u64;
    if taking.truncated.is_some() {
        return;
    }
    let room = budget.saturating_sub(taking.stored);
    let n = (data.len() as u64).min(room);
    if n == 0 {
        taking.truncated = Some(Truncated::Limit);
        return;
    }
    if !store.reserve(n) {
        taking.truncated = Some(Truncated::Budget);
        return;
    }
    taking
        .chunks
        .push(Bytes::copy_from_slice(&data[..n as usize]));
    taking.stored += n;
    *held += n;
    if n < data.len() as u64 {
        taking.truncated = Some(Truncated::Limit);
    }
}

impl Taking {
    /// A body no byte of which came yet.
    fn empty() -> Taking {
        Taking {
            chunks: Vec::new(),
            stored: 0,
            size: 0,
            truncated: None,
        }
    }

    /// The body as it is stored: the chunks joined into one block of exactly
    /// their size, so nothing keeps a larger buffer alive.
    fn finish(self) -> Body {
        let Taking {
            chunks,
            stored,
            size,
            truncated,
        } = self;
        let data = match chunks.len() {
            0 => Bytes::new(),
            1 => chunks.into_iter().next().expect("one chunk"),
            _ => {
                let mut block = BytesMut::with_capacity(stored as usize);
                for chunk in chunks {
                    block.extend_from_slice(&chunk);
                }
                block.freeze()
            }
        };
        Body {
            data,
            size,
            truncated,
        }
    }
}

impl Inner {
    /// Reserve `n` bytes for a body still coming; false when not even an
    /// empty store has the room (`phase 4 M4 design 4.3`).
    fn reserve(&mut self, n: u64, total: u64) -> bool {
        if self.reserved + n > total {
            return false;
        }
        self.room(self.reserved + n, total);
        self.reserved += n;
        true
    }

    /// Evict the oldest entries until `kept + rest` fits in `total`.
    fn room(&mut self, rest: u64, total: u64) {
        while self.kept + rest > total {
            let Some(kept) = self.queue.pop_front() else {
                break;
            };
            self.kept -= kept.cost;
            self.evicted += 1;
        }
    }

    /// Store `entry`, which reserved `held` bytes already; false when it is
    /// dropped (`phase 4 M4 design 4.4`).
    fn insert(&mut self, entry: Entry, held: u64, total: u64) -> bool {
        self.reserved -= held;
        let cost = cost(&entry);
        if self.reserved + cost > total {
            self.dropped += 1;
            return false;
        }
        self.room(self.reserved + cost, total);
        let truncated =
            entry.request_body.truncated.is_some() || entry.response_body.truncated.is_some();
        self.queue.push_back(Kept {
            entry: Arc::new(entry),
            cost,
        });
        self.kept += cost;
        self.stored += 1;
        if truncated {
            self.truncated += 1;
        }
        true
    }
}

/// What one entry costs the store (`phase 4 M4 design 4.3`): every header
/// line as its name, its value and 32 bytes, the URL, the method, the rule,
/// each policy and note, the error, the two bodies as stored, and
/// `ENTRY_OVERHEAD`.
fn cost(entry: &Entry) -> u64 {
    let mut cost = ENTRY_OVERHEAD
        + entry.request.url.len() as u64
        + entry.request.method.as_str().len() as u64
        + head_cost(&entry.request.headers)
        + entry.request_body.data.len() as u64
        + entry.response_body.data.len() as u64;
    if let Some(response) = &entry.response {
        cost += head_cost(&response.headers);
    }
    if let Some(rule) = &entry.rule {
        cost += rule.len() as u64;
    }
    if let Some(error) = &entry.error {
        cost += error.len() as u64;
    }
    for policy in &entry.policy {
        cost += policy.len() as u64;
    }
    for note in &entry.notes {
        cost += note.len() as u64;
    }
    cost
}

/// What one side's header lines cost: name, value and 32 bytes each.
fn head_cost(headers: &http::HeaderMap) -> u64 {
    headers
        .iter()
        .map(|(name, value)| name.as_str().len() as u64 + value.len() as u64 + 32)
        .sum()
}

/// Hold the store's lock; a panic elsewhere is no reason to stop capturing.
fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    match inner.lock() {
        Ok(inner) => inner,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Head, Outcome, ResponseHead, Timings};
    use bytes::Bytes;
    use http::{HeaderMap, HeaderValue, Method, StatusCode, Version};
    use std::time::{Duration, SystemTime};

    /// An entry of `session` whose request and response heads, texts and
    /// request body are always the same and whose response body is `body`;
    /// its cost is written out in the test below.
    fn made(id: u64, session: &str, body: Body) -> Entry {
        let mut request = HeaderMap::new();
        request.insert("host", HeaderValue::from_static("a.test"));
        request.insert("accept", HeaderValue::from_static("*/*"));
        let mut response = HeaderMap::new();
        response.insert("content-type", HeaderValue::from_static("text/plain"));
        Entry {
            id,
            session: Arc::from(session),
            started: SystemTime::UNIX_EPOCH,
            request: Head {
                method: Method::GET,
                url: "http://a.test/x".to_string(),
                version: Version::HTTP_11,
                headers: request,
            },
            request_body: Body::empty(),
            response: Some(ResponseHead {
                status: StatusCode::OK,
                version: Version::HTTP_11,
                headers: response,
            }),
            response_body: body,
            timings: Timings {
                connect: None,
                dns: None,
                wait: None,
                receive: None,
                total: Duration::ZERO,
                reused: false,
            },
            remote: None,
            outcome: Outcome::Completed,
            error: None,
            rule: Some("DOMAIN,a.test,DIRECT".to_string()),
            policy: vec!["DIRECT".to_string()],
            notes: vec!["m3".to_string()],
        }
    }

    /// An entry of `session` with no body, which costs `PLAIN` bytes.
    fn plain(id: u64, session: &str) -> Entry {
        made(id, session, Body::empty())
    }

    /// What an entry of `made` costs with no body: 183 bytes of heads, texts
    /// and notes, and the overhead. `ENTRY_OVERHEAD` is the measured one (the
    /// phase 4 / M4a plan's `P15`), so the totals below are worked out from it
    /// rather than written down.
    const PLAIN: u64 = 183 + ENTRY_OVERHEAD;

    /// A capture into a fresh store of `limits`, in session `a` by default.
    fn capture(limits: Limits) -> Capture {
        Capture {
            store: CaptureStore::new(limits),
            session: Arc::from("a"),
        }
    }

    /// A body of `n` bytes that came whole.
    fn body(n: usize) -> Body {
        Body {
            data: Bytes::from(vec![b'x'; n]),
            size: n as u64,
            truncated: None,
        }
    }

    /// Every part of an entry is costed, and the overhead once
    /// (`phase 4 M4 design 4.3`).
    #[test]
    fn an_entry_costs_its_headers_texts_bodies_and_the_overhead() {
        let capture = capture(Limits::new(1_000_000, 1_000_000));
        assert!(capture.store.put(made(1, "a", body(12))));

        let expected = 4 + 6 + 32 // host: a.test
            + 6 + 3 + 32          // accept: */*
            + 12 + 10 + 32        // content-type: text/plain
            + 15                  // http://a.test/x
            + 3                   // GET
            + 20                  // DOMAIN,a.test,DIRECT
            + 6                   // DIRECT
            + 2                   // m3
            // the request body is empty; the response body is 12 bytes
            + 12
            + ENTRY_OVERHEAD;
        let status = capture.store.status();
        assert_eq!(status.used, expected);
        assert_eq!(status.entries, 1);
        assert_eq!(capture.store.sessions()[0].bytes, expected);
    }

    /// A body that reaches the body limit keeps its beginning and counts
    /// every byte that went by (`phase 4 M4 design 4.4`).
    #[test]
    fn a_body_over_its_limit_keeps_its_beginning() {
        let capture = capture(Limits::new(1_000_000, 4));
        let mut pending = capture.begin();
        pending.response_chunk(b"012");
        pending.response_chunk(b"345");
        pending.response_chunk(b"6");
        assert!(pending.commit(|_, response| made(1, "a", response)));

        let entry = capture.store.session("a")[0].clone();
        assert_eq!(entry.response_body.data, Bytes::from_static(b"0123"));
        assert_eq!(entry.response_body.size, 7);
        assert_eq!(entry.response_body.truncated, Some(Truncated::Limit));
        assert_eq!(capture.store.status().truncated, 1);
    }

    /// A body limit of zero keeps the heads only, and the entry is stored all
    /// the same (`phase 4 M4 design 4.2`).
    #[test]
    fn a_body_limit_of_zero_keeps_the_heads_only() {
        let capture = capture(Limits::new(1_000_000, 0));
        let mut pending = capture.begin();
        pending.response_chunk(b"abc");
        assert!(pending.commit(|_, response| made(1, "a", response)));

        let entry = capture.store.session("a")[0].clone();
        assert!(entry.response_body.data.is_empty());
        assert_eq!(entry.response_body.size, 3);
        assert_eq!(entry.response_body.truncated, Some(Truncated::Limit));
        assert_eq!(capture.store.status().entries, 1);
    }

    /// The oldest entries go first, whatever their session
    /// (`phase 4 M4 design 4.4`).
    #[test]
    fn the_oldest_entries_go_first_whatever_their_session() {
        let capture = capture(Limits::new(PLAIN * 3, 1000));
        for (id, session) in [(1, "a"), (2, "b"), (3, "a"), (4, "b")] {
            assert!(capture.store.put(plain(id, session)), "{id} fits");
        }

        let status = capture.store.status();
        assert_eq!(status.entries, 3);
        assert_eq!(status.evicted, 1);
        assert_eq!(status.stored, 4);
        assert!(capture.store.entry(1).is_none(), "the oldest went");
        let left: Vec<u64> = capture.store.session("a").iter().map(|e| e.id).collect();
        assert_eq!(left, [3]);
        let left: Vec<u64> = capture.store.session("b").iter().map(|e| e.id).collect();
        assert_eq!(left, [2, 4]);
    }

    /// A chunk makes room for itself by evicting the oldest entry
    /// (`phase 4 M4 design 4.4`).
    #[test]
    fn a_chunk_evicts_older_entries_to_find_room() {
        // two entries fit, and the thousand bytes the chunk reserves do not fit
        // beside them
        let capture = capture(Limits::new(PLAIN * 2 + 500, 2000));
        assert!(capture.store.put(plain(1, "b")));
        assert!(capture.store.put(plain(2, "b")));

        let mut pending = capture.begin();
        pending.response_chunk(&[b'x'; 1000]);
        assert_eq!(capture.store.status().evicted, 1);
        assert!(capture.store.entry(1).is_none(), "the oldest made room");
        assert!(capture.store.entry(2).is_some());

        assert!(pending.commit(|_, response| made(3, "a", response)));
        let entry = capture.store.session("a")[0].clone();
        assert_eq!(entry.response_body.data.len(), 1000, "the chunk whole");
        assert_eq!(entry.response_body.truncated, None);
        assert_eq!(capture.store.status().entries, 1);
        assert_eq!(
            capture.store.status().used,
            PLAIN + 1000,
            "the entry costs its head and the body the chunk reserved"
        );
    }

    /// A chunk the store cannot hold even when empty is not kept from there
    /// on, and nothing is evicted for it (`phase 4 M4 design 4.4`).
    #[test]
    fn a_chunk_that_cannot_fit_even_in_an_empty_store_is_truncated_for_the_budget() {
        // an entry and A's reservation fit with 100 bytes to spare; B asks for
        // more than what is left once A's reservation is held
        let capture = capture(Limits::new(PLAIN + 900, PLAIN + 900));
        assert!(capture.store.put(plain(1, "b")), "PLAIN bytes");

        let mut a = capture.begin();
        a.response_chunk(&[b'a'; 800]);
        assert_eq!(capture.store.status().used, PLAIN + 800);
        assert_eq!(capture.store.status().evicted, 0);

        let mut b = capture.begin();
        b.response_chunk(&[b'b'; PLAIN as usize + 101]);
        let status = capture.store.status();
        assert_eq!(status.evicted, 0, "nothing can be evicted for it");
        assert_eq!(status.used, PLAIN + 800, "A's reservation is untouched");
        assert!(capture.store.entry(1).is_some());

        assert!(b.commit(|_, response| made(2, "b", response)));
        let stored = capture.store.entry(2).expect("B is stored, its body cut");
        assert_eq!(stored.response_body.data.len(), 0);
        assert_eq!(stored.response_body.size, PLAIN + 101);
        assert_eq!(stored.response_body.truncated, Some(Truncated::Budget));
    }

    /// An entry whose head does not fit is dropped and takes nothing with it
    /// (`phase 4 M4 design 4.4`).
    #[test]
    fn an_entry_whose_head_does_not_fit_is_dropped() {
        let none = capture(Limits::new(0, 0));
        assert!(!none.store.put(plain(1, "a")));
        assert_eq!(none.store.status().dropped, 1);
        assert_eq!(none.store.status().used, 0);
        assert_eq!(none.store.status().entries, 0);

        let head_only = capture(Limits::new(PLAIN - 1, 500));
        let mut pending = head_only.begin();
        pending.response_chunk(&[b'x'; 400]);
        assert_eq!(head_only.store.status().used, 400, "the body reserved");
        assert!(!pending.commit(|_, response| made(1, "a", response)));
        let status = head_only.store.status();
        assert_eq!(status.dropped, 1);
        assert_eq!(status.used, 0, "the body's reservation went back");
        assert_eq!(status.stored, 0);
    }

    /// A body still coming gives its reservation back when it is dropped
    /// uncommitted (`phase 4 M4 design 4.3`).
    #[test]
    fn a_pending_entry_dropped_gives_its_reservation_back() {
        let capture = capture(Limits::new(1000, 1000));
        let mut pending = capture.begin();
        pending.response_chunk(&[b'x'; 100]);
        assert_eq!(capture.store.status().used, 100);
        drop(pending);
        assert_eq!(capture.store.status().used, 0);
        assert_eq!(capture.store.status().entries, 0);
    }

    /// However many entries come and go, what is stored and what is reserved
    /// never adds up to more than the total (`phase 4 M4 design 4.3`).
    #[test]
    fn the_store_never_holds_more_than_its_total() {
        let total = PLAIN + 3000;
        let capture = capture(Limits::new(total, 800));
        let mut pendings: Vec<Pending> = Vec::new();
        let mut next = 0u64;
        let mut x = 0x2545_f491_4f6c_dd1du64;
        for step in 0..1000u32 {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let n = (x >> 33) as usize % 1200;
            let data = vec![b'z'; n];
            match (x >> 60) % 4 {
                0 => {
                    let mut pending = capture.begin();
                    pending.response_chunk(&data);
                    pendings.push(pending);
                }
                1 => {
                    if let Some(pending) = pendings.last_mut() {
                        pending.request_chunk(&data);
                    }
                }
                2 => {
                    if let Some(pending) = pendings.pop() {
                        next += 1;
                        let id = next;
                        pending.commit(|request, response| {
                            let mut entry = made(id, "a", response);
                            entry.request_body = request;
                            entry
                        });
                    }
                }
                _ => {
                    if let Some(mut pending) = pendings.pop() {
                        pending.response_chunk(&data);
                    }
                }
            }
            let used = capture.store.status().used;
            assert!(used <= total, "step {step}: {used} over the total");
        }

        drop(pendings);
        capture.store.clear(None);
        assert_eq!(capture.store.status().used, 0);
    }

    /// Entries are listed by session, oldest first, and found by id
    /// (`phase 4 M4 design 4.6`, `7.2`).
    #[test]
    fn entries_are_listed_by_session_and_found_by_id() {
        let capture = capture(Limits::new(PLAIN * 4, 1000));
        assert!(capture.store.put(plain(1, "a")));
        assert!(capture.store.put(plain(2, "b")));
        assert!(capture.store.put(plain(3, "a")));

        let ids: Vec<u64> = capture.store.session("a").iter().map(|e| e.id).collect();
        assert_eq!(ids, [1, 3], "from the oldest to the newest");
        assert!(capture.store.session("nope").is_empty());
        assert!(capture.store.session("a")[0].session.as_ref() == "a");

        let sessions = capture.store.sessions();
        assert_eq!(sessions.len(), 2, "only sessions with entries");
        assert_eq!(sessions[0].name.as_ref(), "a");
        assert_eq!(sessions[0].entries, 2);
        assert_eq!(sessions[0].bytes, PLAIN * 2);
        assert_eq!(sessions[1].name.as_ref(), "b");
        assert_eq!(sessions[1].entries, 1);
        assert_eq!(sessions[1].bytes, PLAIN);

        assert_eq!(capture.store.entry(2).expect("there").id, 2);
        assert!(capture.store.entry(9).is_none());
    }

    /// Clearing one session or all of them keeps the four counts
    /// (`phase 4 M4 design 7.6`).
    #[test]
    fn clearing_one_session_or_all_keeps_the_four_counts() {
        let capture = capture(Limits::new(PLAIN * 4, 1000));
        assert!(capture.store.put(plain(1, "a")));
        assert!(capture.store.put(plain(2, "a")));
        assert!(capture.store.put(plain(3, "b")));

        assert_eq!(capture.store.clear(Some("a")), 2);
        assert_eq!(capture.store.session("b").len(), 1);
        assert_eq!(capture.store.status().used, PLAIN);
        assert_eq!(capture.store.clear(Some("nope")), 0);
        assert_eq!(capture.store.clear(None), 1);
        assert_eq!(capture.store.status().used, 0);
        assert_eq!(capture.store.status().entries, 0);

        let status = capture.store.status();
        assert_eq!(status.stored, 3);
        assert_eq!(status.evicted, 0);
        assert_eq!(status.dropped, 0);
        assert_eq!(status.truncated, 0);
    }

    /// An entry that is held goes on after it was evicted and cleared
    /// (`phase 4 M4 design 4.4`, `8.4`).
    #[test]
    fn an_evicted_entry_lives_on_while_it_is_held() {
        let capture = capture(Limits::new(PLAIN * 2, 1000));
        let mut pending = capture.begin();
        pending.response_chunk(b"the body");
        assert!(pending.commit(|_, response| made(1, "a", response)));

        let held = capture.store.session("a")[0].clone();
        assert!(capture.store.put(plain(2, "b")));
        assert!(capture.store.put(plain(3, "b")));
        assert!(capture.store.entry(1).is_none(), "it was evicted");
        assert_eq!(capture.store.clear(None), 2);

        assert_eq!(held.response_body.data, Bytes::from_static(b"the body"));
        assert_eq!(held.response_body.size, 8);
        assert_eq!(capture.store.status().used, 0);
    }

    /// An entry made elsewhere is costed and counted as one that came in
    /// through a `Pending` (`phase 4 / M4a plan P6`).
    #[test]
    fn put_stores_an_entry_made_elsewhere() {
        let by_hand = capture(Limits::new(1_000_000, 1000));
        let through = capture(Limits::new(1_000_000, 1000));

        assert!(by_hand.store.put(made(7, "a", body(12))));
        let mut pending = through.begin();
        pending.response_chunk(&[b'x'; 12]);
        assert!(pending.commit(|_, response| made(7, "a", response)));

        let one = by_hand.store.status();
        let other = through.store.status();
        assert_eq!(one.used, other.used);
        assert_eq!(one.stored, 1);
        assert_eq!(other.stored, 1);
        assert_eq!(
            by_hand.store.session("a")[0].response_body.data,
            through.store.session("a")[0].response_body.data
        );
    }
}
