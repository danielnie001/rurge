//! The fake-IP pool (M3 design §10): the addresses the DNS responder hands out
//! in place of real ones, so that a connection that arrives at one of them can
//! be turned back into the name it was for.
//!
//! An address is an index. Index `i` is both `198.18.1.1 + i` and `fd00:6152::`
//! followed by that IPv4 address, so a name takes one index and has both. A
//! name that has an index keeps it; a new name gets the next index nobody has
//! had and, once every index has been had, the one used longest ago. Looking
//! an address up uses it. An index that a [`Lease`] is held on is never taken
//! back, however long ago it was last used: a connection looks its address up
//! once, when it arrives, and may last for days.
//!
//! The table is kept in a file (see `file`), so that an application that
//! remembers an address across a restart finds the same name behind it. When
//! the last run did not stop cleanly the file is behind by an unknown number
//! of assignments, which applications may still hold addresses of: the
//! indices the allocator would hand out next are held back for a while. Any
//! index the file does not name is held back as well, in addition to those
//! and not among them: an earlier run held it back and has not given it out
//! since.

mod file;

use lru::LruCache;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// How many addresses there are: `198.18.1.1` to `198.19.255.254`.
pub const FAKE_COUNT: u32 = 130_814;
/// The TTL of an answer that carries a fake address.
pub const FAKE_TTL: Duration = Duration::from_secs(30);
/// How long what an open holds back stays unused, counted from that open: the
/// indices the file has no name for (those below its cursor, and any once it
/// has wrapped), whatever the last stop was; and after an unclean stop, or a
/// file that could not be read, the next [`QUARANTINE_COUNT`] indices besides.
pub const QUARANTINE: Duration = Duration::from_secs(120);
/// How many indices an unclean stop, or a file that could not be read, holds
/// back: the next ones the allocator would hand out (once it has been through
/// every index, those of the names used longest ago, and the names go). They
/// are held back on top of the indices the file has no name for, not among
/// them. An index held back has no name until it is given out, which waits for
/// [`QUARANTINE`], and every open finds it nameless and holds it back again;
/// so each unclean open before the last one's indices are given out holds back
/// this many more. Some 64 crashes in a row (130,814 / 2,048) take the
/// allocator through every index: from then on each open holds back every
/// index without a name for the first [`QUARANTINE`], in which `assign` can
/// only take over the index of the name used longest ago, and answers
/// `Exhausted` when there is none, until runs past [`QUARANTINE`] have given
/// those indices out. No index is lost for good: all are free [`QUARANTINE`]
/// after the last open.
pub const QUARANTINE_COUNT: u32 = 2_048;
/// The name of the file in the data directory.
pub const FILE_NAME: &str = "fake-ip.txt";

/// `198.18.1.1`, the address of index 0.
const FIRST: u32 = u32::from_be_bytes([198, 18, 1, 1]);

/// The IPv4 address of an index below [`FAKE_COUNT`]: `198.18.1.1 + index`.
pub fn v4_of(index: u32) -> Ipv4Addr {
    Ipv4Addr::from(FIRST + index)
}

/// The IPv6 address of an index below [`FAKE_COUNT`]: `fd00:6152::` and, in
/// the low 32 bits, its IPv4 address.
pub fn v6_of(index: u32) -> Ipv6Addr {
    let [a, b, c, d] = v4_of(index).octets();
    let (high, low) = (u16::from_be_bytes([a, b]), u16::from_be_bytes([c, d]));
    Ipv6Addr::new(0xfd00, 0x6152, 0, 0, 0, 0, high, low)
}

/// The index an address stands for. Only the two forms above count; an
/// IPv4-mapped IPv6 address is not one.
pub fn index_of(ip: IpAddr) -> Option<u32> {
    let v4_bits = match ip {
        IpAddr::V4(v4) => u32::from(v4),
        IpAddr::V6(v6) => match v6.segments() {
            [0xfd00, 0x6152, 0, 0, 0, 0, high, low] => (u32::from(high) << 16) | u32::from(low),
            _ => return None,
        },
    };
    v4_bits
        .checked_sub(FIRST)
        .filter(|&index| index < FAKE_COUNT)
}

/// A name as the pool knows it: one trailing dot off, ASCII lower case, and
/// then non-empty labels of `a`-`z`, `0`-`9`, `-` and `_` only, at most 253
/// bytes in all. Anything else is not a name the pool gives an address.
pub fn normalize(name: &str) -> Option<String> {
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.len() > 253 {
        return None;
    }
    let name = name.to_ascii_lowercase();
    let a_label = |label: &str| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
    };
    name.split('.').all(a_label).then_some(name)
}

/// Why a name was given no address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignError {
    /// The name is not one the pool takes (see [`normalize`]).
    NotAName,
    /// Every address is leased or held back.
    Exhausted,
}

/// What [`FakeIpPool::open`] found in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loaded {
    /// There was no file: an empty pool.
    Missing,
    /// The last run stopped cleanly: its table, as it was. Nothing is held
    /// back but the indices the file has no name for, which an earlier run
    /// held back and did not give out again.
    Clean,
    /// The last run did not stop cleanly: its table, less what may have been
    /// given out after the last save. The next [`QUARANTINE_COUNT`] indices
    /// the allocator would hand out are held back, besides the ones the file
    /// has no name for.
    Unclean,
    /// The file could not be read or made no sense: an empty pool, and the
    /// next [`QUARANTINE_COUNT`] indices are held back, as after an unclean
    /// stop (what the file held may still be in applications' hands). The file
    /// is left as it is until the first save replaces it. The reason quotes
    /// nothing of the file.
    NotRead(String),
}

/// The pool. One lock guards all of it, and it is a leaf: nothing else is
/// locked while it is held and no code of the caller runs.
pub struct FakeIpPool {
    state: Mutex<State>,
    /// Wakes [`FakeIpPool::keep_saved`]: a mapping changed.
    changed: Notify,
}

/// A hold on an address: while one exists the address is not recycled. The
/// address is given back, as the most recently used, when the last lease on it
/// is dropped.
#[must_use = "the address is given back when the lease is dropped"]
pub struct Lease {
    pool: Arc<FakeIpPool>,
    index: u32,
}

struct State {
    /// How many indices there are.
    count: u32,
    /// Index to name; `None` for an index no name has.
    names: Vec<Option<Arc<str>>>,
    /// Name to index.
    indices: HashMap<Arc<str>, u32>,
    /// The indices that have a name and no lease, in the order they were
    /// used.
    idle: LruCache<u32, ()>,
    /// How many leases each leased index has.
    leases: HashMap<u32, u32>,
    /// The next index nobody has had, unless `wrapped`.
    cursor: u32,
    /// The cursor reached the end: every index has been had once, and the next
    /// one is the one used longest ago.
    wrapped: bool,
    /// Indices held back, the one held longest first. They have no name.
    held: VecDeque<u32>,
    /// When what is held back is let go; set when anything is held.
    held_until: Option<Instant>,
    /// A mapping or the cursor changed since the table was last taken.
    dirty: bool,
}

impl State {
    fn new(count: u32) -> State {
        let count = count.min(FAKE_COUNT);
        State {
            count,
            names: vec![None; count as usize],
            indices: HashMap::new(),
            idle: LruCache::unbounded(),
            leases: HashMap::new(),
            cursor: 0,
            wrapped: false,
            held: VecDeque::new(),
            held_until: None,
            dirty: false,
        }
    }

    /// The table a file held, as of the stop it records. Indices the file
    /// does not name, below the cursor or anywhere once it has wrapped, were
    /// held back when it was written; they are held back again.
    fn restore(count: u32, table: file::Table) -> State {
        let mut state = State::new(count);
        state.cursor = table.cursor;
        state.wrapped = table.wrapped || table.cursor >= state.count;
        for (index, name) in table.entries {
            state.names[index as usize] = Some(Arc::clone(&name));
            state.indices.insert(name, index);
            state.idle.put(index, ());
        }
        let bound = if state.wrapped {
            state.count
        } else {
            state.cursor
        };
        state.held = (0..bound)
            .filter(|&index| state.names[index as usize].is_none())
            .collect();
        state
    }

    /// Holds back the next `want` indices the allocator would hand out, in
    /// the order it would, behind those already held (the holes `restore`
    /// found, which do not count among the `want`), and sets when everything
    /// held is let go.
    fn hold_back(&mut self, want: usize, now: Instant) {
        for _ in 0..want {
            let Some(index) = self.take_fresh().or_else(|| self.take_idle()) else {
                break;
            };
            self.held.push_back(index);
        }
        if !self.held.is_empty() {
            self.held_until = Some(now + QUARANTINE);
        }
    }

    /// The index for a new name: what was held back and is let go, in the
    /// order it was held; then one nobody has had; then the one used longest
    /// ago, whose name goes.
    fn take(&mut self, now: Instant) -> Option<u32> {
        if self.held_until.is_some_and(|until| now >= until)
            && let Some(index) = self.held.pop_front()
        {
            return Some(index);
        }
        self.take_fresh().or_else(|| self.take_idle())
    }

    fn take_fresh(&mut self) -> Option<u32> {
        if self.wrapped || self.cursor >= self.count {
            return None;
        }
        let index = self.cursor;
        self.cursor += 1;
        self.wrapped = self.cursor == self.count;
        Some(index)
    }

    fn take_idle(&mut self) -> Option<u32> {
        let (index, ()) = self.idle.pop_lru()?;
        if let Some(name) = self.names[index as usize].take() {
            self.indices.remove(&*name);
        }
        Some(index)
    }

    /// Gives `name` the free index `index`.
    fn give(&mut self, index: u32, name: &str) {
        let name: Arc<str> = Arc::from(name);
        self.names[index as usize] = Some(Arc::clone(&name));
        self.indices.insert(name, index);
        self.idle.put(index, ());
        self.dirty = true;
    }

    /// Uses an address (a no-op while it is leased: it is not in the order).
    fn refresh(&mut self, index: u32) {
        self.idle.promote(&index);
    }

    fn release(&mut self, index: u32) {
        let Some(held) = self.leases.get_mut(&index) else {
            return;
        };
        *held -= 1;
        if *held == 0 {
            self.leases.remove(&index);
            if self.names[index as usize].is_some() {
                self.idle.put(index, ());
            }
        }
    }

    /// What the file holds: the names nobody holds a lease on, the one used
    /// longest ago first, then those that are leased.
    fn table(&self) -> file::Table {
        let named = |index: u32| self.names[index as usize].clone().map(|name| (index, name));
        let mut entries = Vec::with_capacity(self.indices.len());
        entries.extend(
            self.idle
                .iter()
                .rev()
                .filter_map(|(&index, ())| named(index)),
        );
        let mut leased: Vec<u32> = self.leases.keys().copied().collect();
        leased.sort_unstable();
        entries.extend(leased.into_iter().filter_map(named));
        file::Table {
            cursor: self.cursor,
            wrapped: self.wrapped,
            entries,
        }
    }

    /// The table, if a mapping or the cursor changed since it was last taken.
    fn take_if_dirty(&mut self) -> Option<file::Table> {
        std::mem::take(&mut self.dirty).then(|| self.table())
    }
}

impl Default for FakeIpPool {
    fn default() -> FakeIpPool {
        FakeIpPool::new()
    }
}

impl FakeIpPool {
    /// An empty pool with every address, kept in no file.
    pub fn new() -> FakeIpPool {
        FakeIpPool::with_state(State::new(FAKE_COUNT))
    }

    /// The pool a file left behind, or an empty one (see [`Loaded`]), and what
    /// was found. Reads and writes the file on the calling thread, and
    /// leaves logging to the caller. `now` is when what is held back starts
    /// to count.
    ///
    /// Whatever it found, the file says afterwards that the run it belongs to
    /// has not ended: a crash from here on is told from a clean stop, even
    /// one before the first save. The exception is a file that could not be
    /// read, which is left as it is until the first save replaces it.
    pub fn open(path: &Path, now: Instant) -> (Arc<FakeIpPool>, Loaded) {
        FakeIpPool::open_sized(path, now, FAKE_COUNT, QUARANTINE_COUNT)
    }

    /// An empty pool of `count` indices.
    #[cfg(any(test, feature = "testing"))]
    pub fn with_count(count: u32) -> FakeIpPool {
        FakeIpPool::with_state(State::new(count))
    }

    /// [`FakeIpPool::open`] for a pool of `count` indices that holds back
    /// `quarantine` of them after an unclean stop.
    #[cfg(any(test, feature = "testing"))]
    pub fn open_with(
        path: &Path,
        now: Instant,
        count: u32,
        quarantine: u32,
    ) -> (Arc<FakeIpPool>, Loaded) {
        FakeIpPool::open_sized(path, now, count, quarantine)
    }

    fn with_state(state: State) -> FakeIpPool {
        FakeIpPool {
            state: Mutex::new(state),
            changed: Notify::new(),
        }
    }

    fn open_sized(
        path: &Path,
        now: Instant,
        count: u32,
        quarantine: u32,
    ) -> (Arc<FakeIpPool>, Loaded) {
        let count = count.min(FAKE_COUNT);
        let (mut state, loaded) = match std::fs::read(path) {
            Ok(bytes) => match file::parse(&bytes, count) {
                Ok((table, true)) => (State::restore(count, table), Loaded::Clean),
                Ok((table, false)) => (State::restore(count, table), Loaded::Unclean),
                Err(reason) => (State::new(count), Loaded::NotRead(reason)),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (State::new(count), Loaded::Missing),
            Err(e) => (
                State::new(count),
                Loaded::NotRead(format!("the file could not be read: {e}")),
            ),
        };
        // What an unclean run gave out after its last save, or what a file
        // that cannot be read held, may still be in applications' hands: the
        // indices that would be given out next are not, for a while. They are
        // held back besides the holes `restore` already holds, not among them.
        let want = match loaded {
            Loaded::Unclean | Loaded::NotRead(_) => quarantine as usize,
            Loaded::Missing | Loaded::Clean => 0,
        };
        state.hold_back(want, now);
        let saved = !matches!(loaded, Loaded::NotRead(_))
            && file::write(path, &file::render(&state.table(), false)).is_ok();
        state.dirty = !saved;
        (Arc::new(FakeIpPool::with_state(state)), loaded)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The index of `name`, a new one if it has none. Uses the address either
    /// way. `now` is only what lets go of what is held back.
    pub fn assign(&self, name: &str, now: Instant) -> Result<u32, AssignError> {
        let name = normalize(name).ok_or(AssignError::NotAName)?;
        let mut state = self.lock();
        if let Some(&index) = state.indices.get(name.as_str()) {
            state.refresh(index);
            return Ok(index);
        }
        let index = state.take(now).ok_or(AssignError::Exhausted)?;
        state.give(index, &name);
        drop(state);
        self.changed.notify_one();
        Ok(index)
    }

    /// The name behind an address, which is used by it.
    pub fn name_of(&self, ip: IpAddr) -> Option<String> {
        let index = index_of(ip)?;
        let mut state = self.lock();
        let name = state.names.get(index as usize)?.clone()?;
        state.refresh(index);
        Some(name.to_string())
    }

    /// The name behind an address, and a lease that keeps the address from
    /// being recycled. Looking up with a lease is a use, too: it comes into
    /// effect when the last lease is dropped.
    pub fn lease(self: &Arc<Self>, ip: IpAddr) -> Option<(String, Lease)> {
        let index = index_of(ip)?;
        let mut state = self.lock();
        let name = state.names.get(index as usize)?.clone()?;
        *state.leases.entry(index).or_insert(0) += 1;
        state.idle.pop(&index);
        drop(state);
        let lease = Lease {
            pool: Arc::clone(self),
            index,
        };
        Some((name.to_string(), lease))
    }

    /// The address of a name, if it has one. Never assigns and never uses
    /// the address.
    pub fn address_of(&self, name: &str, v6: bool) -> Option<IpAddr> {
        let name = normalize(name)?;
        let index = *self.lock().indices.get(name.as_str())?;
        Some(if v6 {
            IpAddr::V6(v6_of(index))
        } else {
            IpAddr::V4(v4_of(index))
        })
    }

    /// Keeps the file up to date until `done` completes: when a mapping or
    /// the cursor has changed, the table is written, and then not again for
    /// `every`; changes made meanwhile are written together once it is over.
    /// A change that is only a use of an address is not a change. When `done`
    /// completes, the table is written a last time, as having ended cleanly,
    /// with whatever has not been written yet.
    ///
    /// A table that cannot be written is a warning, without a name in it, and
    /// is tried again at the next change.
    pub async fn keep_saved(
        self: Arc<Self>,
        path: PathBuf,
        every: Duration,
        done: impl Future<Output = ()> + Send,
    ) {
        let mut done = std::pin::pin!(done);
        loop {
            tokio::select! {
                () = &mut done => break,
                () = self.changed.notified() => {}
            }
            // nothing: a change that an earlier write took along
            let table = self.lock().take_if_dirty();
            if let Some(table) = table {
                save(&path, table, false).await;
                tokio::select! {
                    () = &mut done => break,
                    () = tokio::time::sleep(every) => {}
                }
            }
        }
        let table = self.lock().table();
        save(&path, table, true).await;
    }
}

/// Writes the table on a blocking thread.
async fn save(path: &Path, table: file::Table, clean: bool) {
    let path = path.to_path_buf();
    let written =
        tokio::task::spawn_blocking(move || file::write(&path, &file::render(&table, clean))).await;
    match written {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "fake-ip: the addresses could not be saved"),
        Err(e) => tracing::warn!(error = %e, "fake-ip: the addresses could not be saved"),
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.pool.lock().release(self.index);
    }
}

impl fmt::Debug for FakeIpPool {
    /// Counts only: no name is shown.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock();
        f.debug_struct("FakeIpPool")
            .field("addresses", &state.count)
            .field("names", &state.indices.len())
            .field("leased", &state.leases.len())
            .field("held", &state.held.len())
            .finish()
    }
}

impl fmt::Debug for Lease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Lease").field(&self.index).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::testing::Lines;
    use tokio::sync::oneshot;
    use tokio::time::{sleep, timeout};
    use tracing::Subscriber;

    /// The bound on every wait for something that has to happen.
    const BOUND: Duration = Duration::from_secs(10);

    /// `Lines`, but only for what is logged at WARN or above: `Lines` records
    /// down to INFO and does not say which level a line was.
    struct AtWarn(Lines);

    impl Subscriber for AtWarn {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            *metadata.level() <= tracing::Level::WARN
        }

        fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            self.0.new_span(span)
        }

        fn record(&self, span: &tracing::span::Id, values: &tracing::span::Record<'_>) {
            self.0.record(span, values);
        }

        fn record_follows_from(&self, span: &tracing::span::Id, follows: &tracing::span::Id) {
            self.0.record_follows_from(span, follows);
        }

        fn event(&self, event: &tracing::Event<'_>) {
            self.0.event(event);
        }

        fn enter(&self, span: &tracing::span::Id) {
            self.0.enter(span);
        }

        fn exit(&self, span: &tracing::span::Id) {
            self.0.exit(span);
        }
    }

    /// Routes what this thread logs at WARN or above to `lines` until the
    /// guard, the first element, is dropped. The second, a dispatcher that
    /// records nothing, is kept for as long as the first, as `Lines::capture`
    /// has it: with one dispatcher alone, a call site that another test
    /// reached first would stay shut on this thread.
    fn capture_warnings(lines: &Lines) -> (tracing::subscriber::DefaultGuard, tracing::Dispatch) {
        (
            tracing::subscriber::set_default(AtWarn(lines.clone())),
            tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()),
        )
    }

    fn ip4(index: u32) -> IpAddr {
        IpAddr::V4(v4_of(index))
    }

    fn ip6(index: u32) -> IpAddr {
        IpAddr::V6(v6_of(index))
    }

    /// The name the files below give to address number `index`.
    fn name(index: u32) -> String {
        format!("n{index}.test")
    }

    /// A file as the pool writes it, with `name(i)` for each `i` of `indices`,
    /// the first being the one used longest ago.
    fn file_text(cursor: u32, wrapped: bool, clean: bool, indices: &[u32]) -> String {
        let mut text = format!(
            "rurge-fake-ip 1\ncursor {cursor} wrapped {} clean {}\n",
            u8::from(wrapped),
            u8::from(clean)
        );
        for &index in indices {
            text.push_str(&format!("{index} {}\n", name(index)));
        }
        text
    }

    /// Opens a pool of `count` addresses on a file holding `text`; a file that
    /// was left unclean holds `quarantine` addresses back.
    fn open_text(
        text: &str,
        now: Instant,
        count: u32,
        quarantine: u32,
    ) -> (Arc<FakeIpPool>, Loaded, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), text).unwrap();
        let (pool, loaded) =
            FakeIpPool::open_with(&dir.path().join(FILE_NAME), now, count, quarantine);
        (pool, loaded, dir)
    }

    /// The second line of the file: where the cursor is, and whether the run
    /// it belongs to has ended.
    fn state_line(path: &Path) -> String {
        let text = std::fs::read_to_string(path).unwrap();
        text.lines().nth(1).unwrap().to_string()
    }

    /// Waits, for at most `BOUND`, until `done` says so.
    async fn until(what: &str, mut done: impl FnMut() -> bool) {
        timeout(BOUND, async {
            while !done() {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    /// The file's text, once it holds `what`.
    async fn file_with(path: &Path, what: &str) -> String {
        until(&format!("the file to hold {what}"), || {
            std::fs::read_to_string(path).is_ok_and(|text| text.contains(what))
        })
        .await;
        std::fs::read_to_string(path).unwrap()
    }

    /// A running `keep_saved`, and the way to stop it.
    struct Saver {
        stop: oneshot::Sender<()>,
        task: tokio::task::JoinHandle<()>,
    }

    fn keep(pool: &Arc<FakeIpPool>, path: &Path, every: Duration) -> Saver {
        let (stop, stopped) = oneshot::channel::<()>();
        let done = async move {
            let _ = stopped.await;
        };
        let task = tokio::spawn(Arc::clone(pool).keep_saved(path.to_path_buf(), every, done));
        Saver { stop, task }
    }

    impl Saver {
        async fn stop(self) {
            let _ = self.stop.send(());
            timeout(BOUND, self.task)
                .await
                .expect("keep_saved returns once done has completed")
                .unwrap();
        }
    }

    #[test]
    fn the_pool_and_its_leases_go_between_threads() {
        fn send_and_sync<T: Send + Sync>() {}
        send_and_sync::<FakeIpPool>();
        send_and_sync::<Lease>();
        send_and_sync::<Arc<FakeIpPool>>();
    }

    #[test]
    fn addresses_and_indices_map_both_ways() {
        assert_eq!(v4_of(0), Ipv4Addr::new(198, 18, 1, 1));
        assert_eq!(v4_of(255), Ipv4Addr::new(198, 18, 2, 0));
        assert_eq!(v4_of(130_813), Ipv4Addr::new(198, 19, 255, 254));
        assert_eq!(v6_of(0), "fd00:6152::c612:101".parse::<Ipv6Addr>().unwrap());
        assert_eq!(
            v6_of(130_813),
            "fd00:6152::c613:fffe".parse::<Ipv6Addr>().unwrap()
        );
        for index in [0, 1, 254, 255, 256, 65_535, 65_536, 130_812, 130_813] {
            assert_eq!(index_of(ip4(index)), Some(index), "{index} over IPv4");
            assert_eq!(index_of(ip6(index)), Some(index), "{index} over IPv6");
        }
        for not_ours in [
            "198.18.0.1",
            "198.18.1.0",
            "198.19.255.255",
            "198.20.0.1",
            "10.0.0.1",
            "::ffff:198.18.1.1",
            "fd00:6152::1",
            "fd00:6152::c612:100",
            "fd00:6152::c613:ffff",
            "fd00:6152:0:1::c612:101",
            "fd01:6152::c612:101",
            "::1",
        ] {
            let ip: IpAddr = not_ours.parse().unwrap();
            assert_eq!(index_of(ip), None, "{not_ours}");
        }
        assert_eq!(FAKE_COUNT, 130_814);
        assert_eq!(FAKE_TTL, Duration::from_secs(30));
        assert_eq!(QUARANTINE, Duration::from_secs(120));
        assert_eq!(QUARANTINE_COUNT, 2_048);
        assert_eq!(FILE_NAME, "fake-ip.txt");
    }

    #[test]
    fn names_are_normalized() {
        assert_eq!(normalize("Example.COM.").as_deref(), Some("example.com"));
        assert_eq!(normalize("example.com").as_deref(), Some("example.com"));
        assert_eq!(normalize("a_b-c.test").as_deref(), Some("a_b-c.test"));
        assert_eq!(normalize("localhost").as_deref(), Some("localhost"));
        for not_a_name in [
            "",
            ".",
            "..",
            ".a",
            "a..b",
            "a.b..",
            "ä.test",
            "a b.test",
            "a/b.test",
            "a.test\n",
            "a@b.test",
            "名字.test",
        ] {
            assert_eq!(normalize(not_a_name), None, "{not_a_name:?}");
        }
        // 253 bytes are fine, 254 are not; a trailing dot does not count
        let longest = [63, 63, 63, 61].map(|n| "a".repeat(n)).join(".");
        assert_eq!(longest.len(), 253);
        assert_eq!(normalize(&longest).as_deref(), Some(longest.as_str()));
        assert_eq!(
            normalize(&format!("{longest}.")).as_deref(),
            Some(longest.as_str())
        );
        let too_long = [63, 63, 63, 62].map(|n| "a".repeat(n)).join(".");
        assert_eq!(too_long.len(), 254);
        assert_eq!(normalize(&too_long), None);
        assert_eq!(normalize(&format!("{too_long}.")), None);
    }

    #[test]
    fn a_name_that_is_not_a_name_gets_no_address() {
        let pool = FakeIpPool::with_count(2);
        let now = Instant::now();
        assert_eq!(pool.assign("a..b", now), Err(AssignError::NotAName));
        assert_eq!(pool.assign("", now), Err(AssignError::NotAName));
        assert_eq!(pool.assign("ä.test", now), Err(AssignError::NotAName));
        assert_eq!(pool.address_of("a..b", false), None);
        // nothing was used up
        assert_eq!(pool.assign("a.test", now), Ok(0));
    }

    #[test]
    fn a_name_keeps_its_address() {
        let pool = FakeIpPool::new();
        let now = Instant::now();
        let first = pool.assign("Example.COM.", now).unwrap();
        assert_eq!(pool.assign("example.com", now), Ok(first));
        assert_eq!(pool.assign("EXAMPLE.com.", now), Ok(first));
        assert_eq!(pool.assign("other.test", now), Ok(first + 1));
        assert_eq!(pool.address_of("Example.Com.", false), Some(ip4(first)));
        assert_eq!(pool.address_of("example.com", true), Some(ip6(first)));
        assert_eq!(pool.name_of(ip4(first)).as_deref(), Some("example.com"));
        assert_eq!(pool.name_of(ip6(first)).as_deref(), Some("example.com"));
    }

    #[test]
    fn the_cursor_comes_first_then_the_least_recently_used() {
        let pool = FakeIpPool::with_count(3);
        let now = Instant::now();
        assert_eq!(pool.assign("a.test", now), Ok(0));
        assert_eq!(pool.assign("b.test", now), Ok(1));
        assert_eq!(pool.assign("c.test", now), Ok(2));
        // looking an address up uses it
        assert_eq!(pool.name_of(ip4(0)).as_deref(), Some("a.test"));
        assert_eq!(pool.assign("d.test", now), Ok(1), "b was used longest ago");
        assert_eq!(pool.address_of("b.test", false), None);
        assert_eq!(pool.name_of(ip4(1)).as_deref(), Some("d.test"));
        assert_eq!(pool.address_of("d.test", false), Some(ip4(1)));
        // and so on, in the order of use: c, then a, then d
        assert_eq!(pool.assign("e.test", now), Ok(2));
        assert_eq!(pool.assign("f.test", now), Ok(0));
        assert_eq!(pool.assign("g.test", now), Ok(1));
    }

    #[test]
    fn the_whole_pool_is_used_before_anything_is_recycled() {
        let pool = FakeIpPool::new();
        let now = Instant::now();
        for index in 0..FAKE_COUNT {
            assert_eq!(pool.assign(&format!("h{index}.test"), now), Ok(index));
        }
        assert_eq!(
            pool.name_of(ip4(FAKE_COUNT - 1)),
            Some(format!("h{}.test", FAKE_COUNT - 1))
        );
        assert_eq!(pool.assign("one-more.test", now), Ok(0));
        assert_eq!(pool.address_of("h0.test", false), None);
        assert_eq!(pool.assign("and-another.test", now), Ok(1));
    }

    #[test]
    fn a_leased_address_is_never_recycled() {
        let pool = Arc::new(FakeIpPool::with_count(2));
        let now = Instant::now();
        assert_eq!(pool.assign("a.test", now), Ok(0));
        let (leased, lease) = pool.lease(ip4(0)).unwrap();
        assert_eq!(leased, "a.test");
        for i in 0..20 {
            assert_eq!(pool.assign(&format!("x{i}.test"), now), Ok(1), "x{i}");
            assert_eq!(pool.name_of(ip4(0)).as_deref(), Some("a.test"), "x{i}");
        }
        // looking it up is not what keeps it: it stays however long it goes unused
        for i in 20..40 {
            assert_eq!(pool.assign(&format!("x{i}.test"), now), Ok(1), "x{i}");
        }
        assert_eq!(pool.address_of("a.test", false), Some(ip4(0)));
        drop(lease);
        // the lease gives 0 back as the most recently used: 1 goes first
        assert_eq!(pool.assign("y1.test", now), Ok(1));
        assert_eq!(pool.address_of("a.test", false), Some(ip4(0)));
        assert_eq!(pool.assign("y2.test", now), Ok(0));
        assert_eq!(pool.address_of("a.test", false), None);
    }

    #[test]
    fn every_address_leased_is_exhausted() {
        let pool = Arc::new(FakeIpPool::with_count(2));
        let now = Instant::now();
        assert_eq!(pool.assign("a.test", now), Ok(0));
        assert_eq!(pool.assign("b.test", now), Ok(1));
        let (_, lease_a) = pool.lease(ip4(0)).unwrap();
        let (name_b, _lease_b) = pool.lease(ip6(1)).unwrap();
        assert_eq!(name_b, "b.test");
        assert_eq!(pool.assign("c.test", now), Err(AssignError::Exhausted));
        // a name that has an address keeps it, exhausted or not
        assert_eq!(pool.assign("a.test", now), Ok(0));
        drop(lease_a);
        assert_eq!(pool.assign("c.test", now), Ok(0));
    }

    #[test]
    fn an_address_is_held_until_its_last_lease_is_gone() {
        let pool = Arc::new(FakeIpPool::with_count(1));
        let now = Instant::now();
        assert_eq!(pool.assign("a.test", now), Ok(0));
        let (_, first) = pool.lease(ip4(0)).unwrap();
        let (_, second) = pool.lease(ip6(0)).unwrap();
        drop(first);
        assert_eq!(pool.assign("b.test", now), Err(AssignError::Exhausted));
        drop(second);
        assert_eq!(pool.assign("b.test", now), Ok(0));
    }

    #[test]
    fn the_end_of_a_lease_is_a_use() {
        let pool = Arc::new(FakeIpPool::with_count(3));
        let now = Instant::now();
        for n in ["a.test", "b.test", "c.test"] {
            pool.assign(n, now).unwrap();
        }
        // a leaves the order of use while it is leased
        let (_, lease) = pool.lease(ip4(0)).unwrap();
        assert_eq!(pool.assign("d.test", now), Ok(1), "b was used longest ago");
        // and comes back as the most recently used, after d
        drop(lease);
        assert_eq!(pool.assign("e.test", now), Ok(2), "c");
        assert_eq!(pool.assign("f.test", now), Ok(1), "d");
        assert_eq!(pool.assign("g.test", now), Ok(0), "a");
    }

    #[tokio::test]
    async fn the_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let now = Instant::now();
        let (pool, loaded) = FakeIpPool::open_with(&path, now, 4, 0);
        assert_eq!(loaded, Loaded::Missing);
        let saver = keep(&pool, &path, Duration::from_millis(20));
        for n in ["a.test", "b.test", "c.test", "d.test"] {
            pool.assign(n, now).unwrap();
        }
        // a is the most recently used
        assert_eq!(pool.name_of(ip4(0)).as_deref(), Some("a.test"));
        saver.stop().await;
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "rurge-fake-ip 1\ncursor 4 wrapped 1 clean 1\n1 b.test\n2 c.test\n3 d.test\n0 a.test\n"
        );

        let (again, loaded) = FakeIpPool::open_with(&path, now, 4, 0);
        assert_eq!(loaded, Loaded::Clean);
        for (index, n) in ["a.test", "b.test", "c.test", "d.test"].iter().enumerate() {
            assert_eq!(again.address_of(n, false), Some(ip4(index as u32)), "{n}");
            assert_eq!(again.address_of(n, true), Some(ip6(index as u32)), "{n}");
        }
        // b is still the one used longest ago
        assert_eq!(again.assign("e.test", now), Ok(1));
        // the new run has not ended
        assert_eq!(state_line(&path), "cursor 4 wrapped 1 clean 0");
    }

    #[tokio::test]
    async fn the_file_lists_the_leased_names_last() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let now = Instant::now();
        let (pool, _) = FakeIpPool::open_with(&path, now, 8, 0);
        for n in ["a.test", "b.test", "c.test", "d.test"] {
            pool.assign(n, now).unwrap();
        }
        let (_, lease_a) = pool.lease(ip4(0)).unwrap();
        let (_, lease_c) = pool.lease(ip4(2)).unwrap();
        let saver = keep(&pool, &path, Duration::from_millis(20));
        saver.stop().await;
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "rurge-fake-ip 1\ncursor 4 wrapped 0 clean 1\n1 b.test\n3 d.test\n0 a.test\n2 c.test\n"
        );
        drop((lease_a, lease_c));
    }

    #[tokio::test]
    async fn the_file_is_written_only_when_mappings_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let now = Instant::now();
        let (pool, _) = FakeIpPool::open_with(&path, now, 16, 0);
        let saver = keep(&pool, &path, Duration::from_millis(20));
        std::fs::remove_file(&path).unwrap();
        // Waits for nothing to happen: no mapping has changed, so there is
        // nothing to write; a write would come within `every` (20 ms) and
        // 200 ms is ten times that.
        sleep(Duration::from_millis(200)).await;
        assert!(!path.exists(), "nothing changed, nothing was written");

        pool.assign("a.test", now).unwrap();
        let text = file_with(&path, "a.test").await;
        assert!(text.contains("0 a.test\n"), "{text}");

        // using an address is not a change
        std::fs::remove_file(&path).unwrap();
        assert_eq!(pool.name_of(ip4(0)).as_deref(), Some("a.test"));
        assert_eq!(pool.assign("a.test", now), Ok(0));
        // Waits for nothing to happen, as above.
        sleep(Duration::from_millis(200)).await;
        assert!(!path.exists(), "using an address is not a change");

        pool.assign("b.test", now).unwrap();
        let text = file_with(&path, "b.test").await;
        assert!(text.contains("0 a.test\n1 b.test\n"), "{text}");
        saver.stop().await;
    }

    #[tokio::test]
    async fn a_stop_writes_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let (pool, _) = FakeIpPool::open_with(&path, Instant::now(), 16, 0);
        assert_eq!(state_line(&path), "cursor 0 wrapped 0 clean 0");
        let saver = keep(&pool, &path, Duration::from_millis(20));
        // nothing changed, and the stop still says the run ended well
        saver.stop().await;
        assert_eq!(state_line(&path), "cursor 0 wrapped 0 clean 1");
    }

    #[tokio::test]
    async fn a_stop_writes_what_the_wait_between_saves_held_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let now = Instant::now();
        let (pool, _) = FakeIpPool::open_with(&path, now, 16, 0);
        let saver = keep(&pool, &path, Duration::from_secs(3600));
        pool.assign("a.test", now).unwrap();
        file_with(&path, "a.test").await;
        pool.assign("b.test", now).unwrap();
        // Waits for nothing to happen: the next save is an hour away, and an
        // unthrottled task would have written within a few milliseconds.
        sleep(Duration::from_millis(100)).await;
        assert!(!std::fs::read_to_string(&path).unwrap().contains("b.test"));
        saver.stop().await;
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "rurge-fake-ip 1\ncursor 2 wrapped 0 clean 1\n0 a.test\n1 b.test\n"
        );
    }

    #[tokio::test]
    async fn changes_made_before_the_task_starts_are_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let now = Instant::now();
        let (pool, _) = FakeIpPool::open_with(&path, now, 16, 0);
        pool.assign("a.test", now).unwrap();
        let saver = keep(&pool, &path, Duration::from_secs(3600));
        file_with(&path, "a.test").await;
        saver.stop().await;
    }

    #[tokio::test]
    async fn a_file_that_was_not_read_is_replaced_by_the_first_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "not a table").unwrap();
        let now = Instant::now();
        let (pool, loaded) = FakeIpPool::open_with(&path, now, 16, 4);
        assert!(matches!(loaded, Loaded::NotRead(_)), "{loaded:?}");
        let saver = keep(&pool, &path, Duration::from_millis(20));
        assert_eq!(std::fs::read(&path).unwrap(), b"not a table");
        assert_eq!(pool.assign("a.test", now), Ok(4));
        let text = file_with(&path, "a.test").await;
        assert_eq!(
            text,
            "rurge-fake-ip 1\ncursor 5 wrapped 0 clean 0\n4 a.test\n"
        );
        saver.stop().await;
    }

    #[tokio::test]
    async fn a_failed_save_warns_without_names() {
        let lines = Lines::default();
        // held across the awaits: the task runs on this thread
        let _capturing = capture_warnings(&lines);
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "a file where a directory should be").unwrap();
        let path = blocker.join(FILE_NAME);
        let pool = Arc::new(FakeIpPool::with_count(16));
        let saver = keep(&pool, &path, Duration::from_millis(20));
        pool.assign("secret-name.test", Instant::now()).unwrap();
        until("the warning", || {
            !lines.said("the addresses could not be saved").is_empty()
        })
        .await;
        // Waits for nothing more to happen: one change, one failed save; a
        // second line would come within `every` (20 ms), and 200 ms is ten
        // times that.
        sleep(Duration::from_millis(200)).await;
        let said = lines.said("the addresses could not be saved");
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("fake-ip: the addresses could not be saved"));
        assert!(said[0].contains(" error="), "{}", said[0]);
        assert!(!said[0].contains("secret-name"), "{}", said[0]);
        assert!(!said[0].contains("blocker"), "{}", said[0]);
        // the next change tries again
        pool.assign("another.test", Instant::now()).unwrap();
        until("the second warning", || {
            lines.said("the addresses could not be saved").len() == 2
        })
        .await;
        saver.stop().await;
    }

    #[test]
    fn a_bad_file_gives_an_empty_pool_and_is_left_alone() {
        let now = Instant::now();
        let later = now + Duration::from_secs(121);
        let cases: Vec<(&str, Vec<u8>)> = vec![
            (
                "the first line is wrong",
                b"rurge-fake-ip 2\ncursor 1 wrapped 0 clean 1\n0 secret-a.test\n".to_vec(),
            ),
            ("there is no first line", Vec::new()),
            (
                "the second line has a word for a number",
                b"rurge-fake-ip 1\ncursor one wrapped 0 clean 1\n0 secret-a.test\n".to_vec(),
            ),
            (
                "the second line has a flag that is neither 0 nor 1",
                b"rurge-fake-ip 1\ncursor 1 wrapped 2 clean 1\n0 secret-a.test\n".to_vec(),
            ),
            (
                "the second line is cut short",
                b"rurge-fake-ip 1\ncursor 1 wrapped 0\n0 secret-a.test\n".to_vec(),
            ),
            (
                "the cursor is past the last address",
                b"rurge-fake-ip 1\ncursor 17 wrapped 0 clean 1\n".to_vec(),
            ),
            (
                "an address number is past the last address",
                b"rurge-fake-ip 1\ncursor 16 wrapped 1 clean 1\n16 secret-a.test\n".to_vec(),
            ),
            (
                "an address number is not below the cursor",
                b"rurge-fake-ip 1\ncursor 2 wrapped 0 clean 1\n0 secret-a.test\n2 secret-b.test\n"
                    .to_vec(),
            ),
            (
                "an address number comes twice",
                b"rurge-fake-ip 1\ncursor 2 wrapped 0 clean 1\n0 secret-a.test\n0 secret-b.test\n"
                    .to_vec(),
            ),
            (
                "a name comes twice",
                b"rurge-fake-ip 1\ncursor 2 wrapped 0 clean 1\n0 secret-a.test\n1 secret-a.test\n"
                    .to_vec(),
            ),
            (
                "a name has capitals",
                b"rurge-fake-ip 1\ncursor 1 wrapped 0 clean 1\n0 Secret-A.test\n".to_vec(),
            ),
            (
                "a name has a trailing dot",
                b"rurge-fake-ip 1\ncursor 1 wrapped 0 clean 1\n0 secret-a.test.\n".to_vec(),
            ),
            (
                "a line is not a number and a name",
                b"rurge-fake-ip 1\ncursor 1 wrapped 0 clean 1\nsecret-a.test\n".to_vec(),
            ),
            (
                "a line has a sign before its number",
                b"rurge-fake-ip 1\ncursor 2 wrapped 0 clean 1\n+0 secret-a.test\n".to_vec(),
            ),
            (
                "a line is blank in the middle",
                b"rurge-fake-ip 1\ncursor 2 wrapped 0 clean 1\n0 secret-a.test\n\n1 secret-b.test\n"
                    .to_vec(),
            ),
            (
                "the file is not UTF-8",
                b"rurge-fake-ip 1\ncursor 1 wrapped 0 clean 1\n0 secret-\xff\xfe.test\n".to_vec(),
            ),
        ];
        for (what, bytes) in cases {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(FILE_NAME);
            std::fs::write(&path, &bytes).unwrap();
            let (pool, loaded) = FakeIpPool::open_with(&path, now, 16, 4);
            let Loaded::NotRead(reason) = loaded else {
                panic!("{what}: {loaded:?}");
            };
            assert!(!reason.is_empty(), "{what}");
            assert!(!reason.contains("secret"), "{what}: {reason}");
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "{what}: left alone");
            assert_eq!(pool.address_of("secret-a.test", false), None, "{what}");
            // the pool starts empty, but not as if the old file never was:
            // what an unclean one would have held back is held back
            assert_eq!(pool.assign("first.test", now), Ok(4), "{what}");
            assert_eq!(pool.assign("second.test", later), Ok(0), "{what}");
        }
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        // a directory where the file should be
        let (pool, loaded) = FakeIpPool::open_with(dir.path(), Instant::now(), 16, 4);
        assert!(matches!(loaded, Loaded::NotRead(_)), "{loaded:?}");
        assert_eq!(pool.assign("a.test", Instant::now()), Ok(4));
    }

    #[test]
    fn a_file_with_only_its_header_is_read() {
        let now = Instant::now();
        let (pool, loaded, _dir) = open_text(&file_text(0, false, true, &[]), now, 16, 4);
        assert_eq!(loaded, Loaded::Clean);
        assert_eq!(pool.assign("a.test", now), Ok(0));
    }

    #[test]
    fn the_data_directory_is_made_when_it_is_needed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not").join("yet").join(FILE_NAME);
        let (_pool, loaded) = FakeIpPool::open_with(&path, Instant::now(), 16, 4);
        assert_eq!(loaded, Loaded::Missing);
        assert_eq!(state_line(&path), "cursor 0 wrapped 0 clean 0");
    }

    #[test]
    fn an_unclean_stop_quarantines_the_next_2048() {
        let now = Instant::now();
        let later = now + Duration::from_secs(121);

        // (a) the cursor has not wrapped: the numbers after it are held back
        let (pool, loaded, dir) =
            open_text(&file_text(5, false, false, &[0, 1, 2, 3, 4]), now, 16, 4);
        assert_eq!(loaded, Loaded::Unclean);
        for index in 0..5 {
            assert_eq!(pool.address_of(&name(index), false), Some(ip4(index)));
        }
        assert_eq!(
            state_line(&dir.path().join(FILE_NAME)),
            "cursor 9 wrapped 0 clean 0",
            "the file is rewritten at once, with the cursor past the held numbers"
        );
        assert_eq!(pool.assign("x1.test", now), Ok(9));
        assert_eq!(pool.assign("x2.test", now), Ok(10));
        // once they are let go, they come first, in the order they were held
        for (n, index) in [
            ("y1.test", 5),
            ("y2.test", 6),
            ("y3.test", 7),
            ("y4.test", 8),
        ] {
            assert_eq!(pool.assign(n, later), Ok(index), "{n}");
        }
        assert_eq!(pool.assign("y5.test", later), Ok(11));

        // (b) the cursor has wrapped: the four used longest ago lose their names
        let used_longest_ago_first = [9, 4, 12, 1, 15, 0, 7, 3, 11, 5, 14, 2, 8, 13, 6, 10];
        let (pool, loaded, _dir) = open_text(
            &file_text(16, true, false, &used_longest_ago_first),
            now,
            16,
            4,
        );
        assert_eq!(loaded, Loaded::Unclean);
        for index in [9, 4, 12, 1] {
            assert_eq!(pool.address_of(&name(index), false), None, "{index}");
            assert_eq!(pool.name_of(ip4(index)), None, "{index}");
        }
        for index in [15, 0, 7, 3, 11, 5, 14, 2, 8, 13, 6, 10] {
            assert_eq!(pool.address_of(&name(index), false), Some(ip4(index)));
        }
        // within the 120 s the fifth is the one to go, then the sixth
        assert_eq!(pool.assign("x1.test", now), Ok(15));
        assert_eq!(pool.address_of(&name(15), false), None);
        assert_eq!(pool.assign("x2.test", now), Ok(0));
        // after it, the held ones come back first
        for (n, index) in [
            ("y1.test", 9),
            ("y2.test", 4),
            ("y3.test", 12),
            ("y4.test", 1),
        ] {
            assert_eq!(pool.assign(n, later), Ok(index), "{n}");
        }

        // (c) the cursor is near the end: what is left of it, then the oldest
        let in_use: Vec<u32> = (0..14).collect();
        let (pool, loaded, _dir) = open_text(&file_text(14, false, false, &in_use), now, 16, 4);
        assert_eq!(loaded, Loaded::Unclean);
        assert_eq!(pool.address_of(&name(0), false), None);
        assert_eq!(pool.address_of(&name(1), false), None);
        assert_eq!(pool.address_of(&name(2), false), Some(ip4(2)));
        assert_eq!(pool.assign("x1.test", now), Ok(2));
        for (n, index) in [
            ("y1.test", 14),
            ("y2.test", 15),
            ("y3.test", 0),
            ("y4.test", 1),
        ] {
            assert_eq!(pool.assign(n, later), Ok(index), "{n}");
        }
    }

    #[test]
    fn what_a_stop_that_was_clean_holds_back_is_nothing() {
        let now = Instant::now();
        let in_use: Vec<u32> = (0..5).collect();
        let (pool, loaded, dir) = open_text(&file_text(5, false, true, &in_use), now, 16, 4);
        assert_eq!(loaded, Loaded::Clean);
        assert_eq!(pool.assign("x1.test", now), Ok(5));
        assert_eq!(
            state_line(&dir.path().join(FILE_NAME)),
            "cursor 5 wrapped 0 clean 0",
            "a file read as clean is rewritten as not clean"
        );
    }

    #[test]
    fn holes_in_the_file_are_held_back_with_the_rest() {
        let now = Instant::now();
        let later = now + Duration::from_secs(121);

        // Clean: the cursor is past two numbers nobody has, which an earlier
        // run held back and did not get to give out again.
        let (pool, loaded, _dir) = open_text(&file_text(4, false, true, &[0, 3]), now, 16, 4);
        assert_eq!(loaded, Loaded::Clean);
        assert_eq!(pool.assign("x1.test", now), Ok(4));
        assert_eq!(pool.assign("x2.test", now), Ok(5));
        assert_eq!(pool.assign("y1.test", later), Ok(1));
        assert_eq!(pool.assign("y2.test", later), Ok(2));
        assert_eq!(pool.assign("y3.test", later), Ok(6));

        // Unclean: the holes are held back, and so are the next three the
        // allocator would hand out, 4, 5 and 6, besides them.
        let (pool, loaded, _dir) = open_text(&file_text(4, false, false, &[0, 3]), now, 16, 3);
        assert_eq!(loaded, Loaded::Unclean);
        assert_eq!(
            pool.assign("x1.test", now),
            Ok(7),
            "1, 2 and 4 to 6 are held"
        );
        for (n, index) in [
            ("y1.test", 1),
            ("y2.test", 2),
            ("y3.test", 4),
            ("y4.test", 5),
            ("y5.test", 6),
        ] {
            assert_eq!(pool.assign(n, later), Ok(index), "{n}");
        }
        assert_eq!(pool.assign("y6.test", later), Ok(8));

        // After the cursor wrapped, a number without a name is a hole too.
        let (pool, loaded, _dir) = open_text(&file_text(4, true, true, &[0, 3]), now, 4, 4);
        assert_eq!(loaded, Loaded::Clean);
        assert_eq!(pool.assign("x1.test", now), Ok(0), "used longest ago");
        assert_eq!(pool.assign("y1.test", later), Ok(1));
        assert_eq!(pool.assign("y2.test", later), Ok(2));
    }

    /// A run that was not stopped cleanly holds back the indices after its
    /// cursor, so the file it rewrites has them as holes. If it crashes too,
    /// before it has given them out again and before it saves, the names it
    /// gave out from the cursor in the meantime are in applications' hands,
    /// and the next run has to hold those back as well as the holes.
    #[test]
    fn a_crash_inside_the_quarantine_holds_back_what_the_crashed_run_gave_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, file_text(5, false, false, &[0, 1, 2, 3, 4])).unwrap();
        let now = Instant::now();

        // The first run holds back 5 to 8 (the file now has the cursor at 9
        // and no names for them), gives out 9, 10 and 11, and is killed.
        let (first, loaded) = FakeIpPool::open_with(&path, now, 32, 4);
        assert_eq!(loaded, Loaded::Unclean);
        for (n, index) in [("x1.test", 9), ("x2.test", 10), ("x3.test", 11)] {
            assert_eq!(first.assign(n, now), Ok(index), "{n}");
        }
        drop(first);
        assert_eq!(state_line(&path), "cursor 9 wrapped 0 clean 0");

        // The second run holds back the holes, 5 to 8, and the next four,
        // 9 to 12, which cover what the first gave out.
        let (second, loaded) = FakeIpPool::open_with(&path, now, 32, 4);
        assert_eq!(loaded, Loaded::Unclean);
        assert_eq!(
            second.assign("y1.test", now),
            Ok(13),
            "9, 10 and 11 may be in applications' hands"
        );
        // let go, the holes come first, then the next four
        let later = now + Duration::from_secs(121);
        for (n, index) in [
            ("z1.test", 5),
            ("z2.test", 6),
            ("z3.test", 7),
            ("z4.test", 8),
            ("z5.test", 9),
            ("z6.test", 10),
            ("z7.test", 11),
            ("z8.test", 12),
        ] {
            assert_eq!(second.assign(n, later), Ok(index), "{n}");
        }
    }

    #[test]
    fn the_real_quarantine_is_2048_for_120_seconds() {
        let now = Instant::now();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, file_text(0, false, false, &[])).unwrap();
        let (pool, loaded) = FakeIpPool::open(&path, now);
        assert_eq!(loaded, Loaded::Unclean);
        assert_eq!(pool.assign("a.test", now), Ok(2048));
        assert_eq!(
            pool.assign("b.test", now + Duration::from_secs(119)),
            Ok(2049)
        );
        assert_eq!(pool.assign("c.test", now + Duration::from_secs(121)), Ok(0));
    }

    #[test]
    fn a_crash_before_the_first_save_is_still_unclean() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let now = Instant::now();
        let (pool, loaded) = FakeIpPool::open(&path, now);
        assert_eq!(loaded, Loaded::Missing);
        assert_eq!(state_line(&path), "cursor 0 wrapped 0 clean 0");
        for (n, index) in [("a.test", 0), ("b.test", 1), ("c.test", 2)] {
            assert_eq!(pool.assign(n, now), Ok(index));
        }
        // the crash: nothing was saved after the open
        drop(pool);
        let (pool, loaded) = FakeIpPool::open(&path, now);
        assert_eq!(loaded, Loaded::Unclean);
        assert_eq!(pool.assign("d.test", now), Ok(2048));

        // a file that says clean says not clean from the moment it is read
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, file_text(0, false, true, &[])).unwrap();
        let (_pool, loaded) = FakeIpPool::open(&path, now);
        assert_eq!(loaded, Loaded::Clean);
        assert_eq!(state_line(&path), "cursor 0 wrapped 0 clean 0");
    }

    #[test]
    fn quarantined_addresses_are_unknown() {
        let now = Instant::now();
        let later = now + Duration::from_secs(121);
        let (pool, loaded, _dir) = open_text(&file_text(0, false, false, &[]), now, 16, 4);
        assert_eq!(loaded, Loaded::Unclean);
        for index in [0, 3, 4, 15] {
            assert_eq!(pool.name_of(ip4(index)), None, "{index}");
            assert_eq!(pool.name_of(ip6(index)), None, "{index}");
            assert!(pool.lease(ip4(index)).is_none(), "{index}");
            assert!(pool.lease(ip6(index)).is_none(), "{index}");
        }
        // an address past the pool's own count is unknown, too
        assert_eq!(pool.name_of(ip4(16)), None);
        assert_eq!(pool.name_of(ip4(FAKE_COUNT - 1)), None);
        // let go, an address is known once a name has it
        assert_eq!(pool.assign("a.test", later), Ok(0));
        assert_eq!(pool.name_of(ip4(0)).as_deref(), Some("a.test"));
        assert_eq!(
            pool.lease(ip6(0)).map(|(name, _)| name).as_deref(),
            Some("a.test")
        );
        assert_eq!(pool.name_of(ip4(1)), None, "the others are still held");
    }

    #[test]
    fn what_is_held_back_counts_as_in_use() {
        let now = Instant::now();
        let (pool, loaded, _dir) = open_text(&file_text(0, false, false, &[]), now, 4, 4);
        assert_eq!(loaded, Loaded::Unclean);
        assert_eq!(pool.assign("a.test", now), Err(AssignError::Exhausted));
        assert_eq!(pool.address_of("a.test", false), None);
        // let go, it is free to be given
        let later = now + Duration::from_secs(121);
        assert_eq!(pool.assign("a.test", later), Ok(0));
        assert_eq!(pool.assign("b.test", later), Ok(1));
    }

    #[test]
    fn address_of_never_assigns() {
        let pool = FakeIpPool::with_count(2);
        let now = Instant::now();
        assert_eq!(pool.address_of("nobody.test", false), None);
        assert_eq!(pool.address_of("nobody.test", true), None);
        assert_eq!(pool.address_of("a..b", false), None);
        assert_eq!(
            pool.assign("a.test", now),
            Ok(0),
            "nobody.test took no number"
        );
        assert_eq!(pool.assign("b.test", now), Ok(1));
        // and it does not use an address: c takes a's, the one used longest ago
        assert_eq!(pool.address_of("a.test", false), Some(ip4(0)));
        assert_eq!(pool.assign("c.test", now), Ok(0));
        assert_eq!(pool.address_of("a.test", false), None);
    }

    #[test]
    fn the_debug_of_a_pool_shows_no_names() {
        let pool = Arc::new(FakeIpPool::with_count(4));
        pool.assign("hush-hush.test", Instant::now()).unwrap();
        let (_, lease) = pool.lease(ip4(0)).unwrap();
        let shown = format!("{pool:?}");
        assert!(!shown.contains("hush"), "{shown}");
        assert!(shown.contains('1'), "it counts: {shown}");
        let shown = format!("{lease:?}");
        assert!(!shown.contains("hush"), "{shown}");
        assert!(shown.contains('0'), "the index: {shown}");
    }

    #[tokio::test]
    async fn a_full_pool_survives_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let now = Instant::now();
        let (pool, loaded) = FakeIpPool::open(&path, now);
        assert_eq!(loaded, Loaded::Missing);
        for index in 0..FAKE_COUNT {
            pool.assign(&format!("host-{index}.example.test"), now)
                .unwrap();
        }
        keep(&pool, &path, Duration::from_secs(3600)).stop().await;

        let (again, loaded) = FakeIpPool::open(&path, now);
        assert_eq!(loaded, Loaded::Clean);
        // the order of use came back too: 0 was used longest ago
        assert_eq!(again.assign("one-more.test", now), Ok(0));
        for index in [1, 65_000, FAKE_COUNT - 1] {
            assert_eq!(
                again.name_of(ip4(index)),
                Some(format!("host-{index}.example.test"))
            );
        }
        assert_eq!(again.address_of("host-0.example.test", false), None);
    }
}
