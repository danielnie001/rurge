//! Which process owns the socket a session came from (phase 3 / M2 design §3):
//! the tables of open TCP connections and UDP sockets that the system keeps,
//! and the finding of the row that is the client's end of a session.
//!
//! Best effort and silent: what the system does not say — a table it will not
//! give, a process it will not describe, a socket that is not on this machine —
//! is an owner that is not known (`None`), and nothing fails. What a platform
//! reads is a `Source`; the rest is the same on every platform and is tested
//! against a fake one: which row is the client's, how recent a read has to be to
//! answer, how often a table is read, whether an address is this machine's.
//!
//! This crate knows no other crate of rurge, so the types here are its own.

mod linux;
mod macos;
mod windows;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How long a list of the machine's own addresses is believed.
pub(crate) const ADDRESS_TTL: Duration = Duration::from_secs(2);

/// A TCP connection is in the table before rurge accepts it, and the port it
/// uses does not go to another process within a couple of seconds: a snapshot
/// that lists the connection answers for sessions that began up to this long
/// after the snapshot was taken.
pub(crate) const TCP_GRACE: Duration = Duration::from_secs(2);

/// The least time between the end of one read of a table and the start of the
/// next read of the same table: a stream of new sessions makes a table be read
/// fifty times a second at most.
pub(crate) const READ_GAP: Duration = Duration::from_millis(20);

/// Which table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    fn name(self) -> &'static str {
        match self {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
        }
    }
}

/// Which process owns the socket a session came from.
#[derive(Clone, Copy, Debug)]
pub struct Query {
    pub protocol: Protocol,
    /// The client's own endpoint: the session's source address.
    pub local: SocketAddr,
    /// TCP only: the port the client connected to (rurge's listening port).
    pub remote_port: Option<u16>,
    /// When the session began; decides which reads of the table can answer.
    pub since: Instant,
}

/// The process that owns a socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub pid: u32,
    /// The executable's file name — or what the system calls the process, when
    /// it will not say where the file is.
    pub name: String,
    /// The full path of the executable, when the system says.
    pub path: Option<String>,
}

/// One socket of a table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub local: SocketAddr,
    /// TCP: the other end. A UDP socket has none.
    pub remote: Option<SocketAddr>,
    pub owner: RowOwner,
}

/// What a row says of who has its socket open.
#[cfg_attr(not(test), allow(dead_code))] // `Pid`: the Windows and macOS readers; `Inode`: the Linux one
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RowOwner {
    /// The table names the process.
    Pid(u32),
    /// The table names the socket by its inode, and who has it open is found
    /// by looking at the processes (Linux).
    Inode(u64),
}

/// Why a table could not be read.
#[cfg_attr(not(test), allow(dead_code))] // `Os`: every reader; `Layout`: the macOS one
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReadError {
    /// The system said no, in its own words: they go to the log, so they hold
    /// no path and no address.
    Os(String),
    /// The table is not laid out the way this build reads it (macOS: records
    /// of a size or an order that is not known).
    Layout,
}

/// What a platform knows: its tables of sockets and its processes. The
/// [`Finder`] decides what to ask and when.
///
/// The finder serialises what it asks about one protocol — the read of the
/// table and the `pid` and `describe` of the rows it found — so that one read,
/// one scan and one description serve all the queries that queued behind a
/// lookup. TCP and UDP have a lock each: calls for the two can overlap.
/// `addresses` is called with no lock held.
pub(crate) trait Source: Send + Sync {
    /// Every row of the table of `protocol`: the IPv4 ones first, then the IPv6
    /// ones.
    fn read(&self, protocol: Protocol) -> Result<Vec<Row>, ReadError>;

    /// The process a row belongs to; `None`: nobody that can be seen has its
    /// socket open. For a row that names its process that is the process; for
    /// one that names an inode it can be a scan of every process, so the
    /// finder asks once per row and snapshot.
    fn pid(&self, owner: RowOwner) -> Option<u32> {
        match owner {
            RowOwner::Pid(pid) => Some(pid),
            RowOwner::Inode(_) => None,
        }
    }

    /// The name of a process and the path of its executable. `None`: the
    /// process is not known — it has gone, or the system will not say.
    fn describe(&self, pid: u32) -> Option<(String, Option<String>)>;

    /// The addresses of the machine's interfaces.
    fn addresses(&self) -> Vec<IpAddr>;
}

/// The addresses of the machine's interfaces, loopback ones included. An empty
/// list when the system will not give them.
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "macos")),
    allow(dead_code)
)] // only the readers of Windows, Linux and macOS ask
pub(crate) fn interface_addresses() -> Vec<IpAddr> {
    match if_addrs::get_if_addrs() {
        Ok(interfaces) => interfaces.iter().map(if_addrs::Interface::ip).collect(),
        Err(error) => {
            tracing::debug!(reason = ?error.kind(), "process lookup: cannot list the network interfaces");
            Vec::new()
        }
    }
}

/// A machine whose sockets are not listed: every table is empty, so no process
/// is ever found.
#[cfg_attr(
    all(any(windows, target_os = "linux", target_os = "macos"), not(test)),
    allow(dead_code)
)] // a finder uses it where nothing reads the sockets
pub(crate) struct NoTables;

impl Source for NoTables {
    fn read(&self, _: Protocol) -> Result<Vec<Row>, ReadError> {
        Ok(Vec::new())
    }

    fn describe(&self, _: u32) -> Option<(String, Option<String>)> {
        None
    }

    fn addresses(&self) -> Vec<IpAddr> {
        Vec::new()
    }
}

/// Finds the process that owns the socket a session came from. Many threads
/// may ask at once: the queries for one table queue behind each other and are
/// served from the same read of it.
pub struct Finder {
    source: Box<dyn Source>,
    address_ttl: Duration,
    addresses: Mutex<Option<Addresses>>,
    tcp: Mutex<Table>,
    udp: Mutex<Table>,
    warnings: Warnings,
}

impl Finder {
    /// A finder for the platform this runs on. On Windows, Linux and macOS it
    /// reads the tables the system keeps. On any other system nothing reads the
    /// sockets, and it never finds a process.
    pub fn new() -> Finder {
        #[cfg(windows)]
        let source: Box<dyn Source> = Box::new(windows::Tables);
        #[cfg(target_os = "linux")]
        let source: Box<dyn Source> = Box::new(linux::Tables::new(linux::RealFs));
        #[cfg(target_os = "macos")]
        let source: Box<dyn Source> = Box::new(macos::Tables);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        let source: Box<dyn Source> = Box::new(NoTables); // no reader for the sockets of this system
        Finder::with_source(source, ADDRESS_TTL)
    }

    pub(crate) fn with_source(source: Box<dyn Source>, address_ttl: Duration) -> Finder {
        Finder {
            source,
            address_ttl,
            addresses: Mutex::new(None),
            tcp: Mutex::new(Table::default()),
            udp: Mutex::new(Table::default()),
            warnings: Warnings::default(),
        }
    }

    /// The process that owns the socket a session came from. `None`: it is not
    /// known — the source is not an address of this machine, the connection
    /// has gone, the process has exited, the system would not say.
    ///
    /// Blocking: the caller runs it on a blocking thread.
    pub fn owner(&self, query: &Query) -> Option<Owner> {
        let local = endpoint(query.local);
        // not a socket of this machine: there is nothing to look up
        if !self.is_this_machine(local.0) {
            return None;
        }
        let mut table = lock(self.table(query.protocol));
        let table = &mut *table;

        // The snapshot there is, when it can answer: a query that was queued
        // behind the read that made it is served from it.
        if let Some(snapshot) = table.snapshot.as_mut() {
            let seen = snapshot.find(&*self.source, query, local);
            if snapshot.covers(query.protocol, seen != Seen::Missing, query.since) {
                return snapshot.owner(&*self.source, seen);
            }
        }

        // Otherwise one read, for this query at most; the snapshot it makes
        // answers whatever its time.
        if !self.read_table(table, query.protocol) {
            return None;
        }
        let snapshot = table.snapshot.as_mut()?;
        let seen = snapshot.find(&*self.source, query, local);
        snapshot.owner(&*self.source, seen)
    }

    /// Whether `ip` is this machine's, when that is known without asking the
    /// system: loopback addresses are, and the others are as the list of the
    /// machine's addresses says while it is no older than the time it is
    /// believed. `None`: the list is stale, or there is none yet; [`owner`]
    /// lists the addresses again.
    ///
    /// Never blocks.
    ///
    /// [`owner`]: Finder::owner
    pub fn is_local(&self, ip: IpAddr) -> Option<bool> {
        let ip = ip.to_canonical();
        if ip.is_loopback() {
            return Some(true);
        }
        let addresses = lock(&self.addresses);
        let addresses = addresses
            .as_ref()
            .filter(|addresses| addresses.listed_at.elapsed() < self.address_ttl)?;
        Some(addresses.list.contains(&ip))
    }

    /// Whether `ip` is this machine's: [`is_local`] when that knows, otherwise
    /// after the machine's addresses have been listed again. No lock is held
    /// while the system is asked.
    ///
    /// [`is_local`]: Finder::is_local
    fn is_this_machine(&self, ip: IpAddr) -> bool {
        if let Some(known) = self.is_local(ip) {
            return known;
        }
        let list: Vec<IpAddr> = self
            .source
            .addresses()
            .into_iter()
            .map(|address| address.to_canonical())
            .collect();
        let known = list.contains(&ip.to_canonical());
        *lock(&self.addresses) = Some(Addresses {
            listed_at: Instant::now(),
            list,
        });
        known
    }

    fn table(&self, protocol: Protocol) -> &Mutex<Table> {
        match protocol {
            Protocol::Tcp => &self.tcp,
            Protocol::Udp => &self.udp,
        }
    }

    /// Reads a table again, once `READ_GAP` has passed since the last read of
    /// it ended — the wait is here, with the table's lock held, so that the
    /// queries behind this one are served by this read and not by reads of
    /// their own. Whether it worked; if it did the table has a new snapshot.
    fn read_table(&self, table: &mut Table, protocol: Protocol) -> bool {
        if let Some(ended) = table.last_read_ended {
            // a sleep can end early: go by the clock again after each one
            while let Some(wait) = READ_GAP
                .checked_sub(ended.elapsed())
                .filter(|wait| !wait.is_zero())
            {
                std::thread::sleep(wait);
            }
        }
        let at = Instant::now();
        let result = self.source.read(protocol);
        table.last_read_ended = Some(Instant::now());
        match result {
            Ok(rows) => {
                table.snapshot = Some(Snapshot::new(at, rows));
                true
            }
            Err(error) => {
                self.report(protocol, &error);
                false
            }
        }
    }

    /// Says that a table could not be read: a warning the first time that
    /// failure happens, a debug line after. The line holds the system's words
    /// and no address, process or path.
    fn report(&self, protocol: Protocol, error: &ReadError) {
        let table = protocol.name();
        let (failure, message) = match error {
            ReadError::Os(reason) => (
                Failure::Read(protocol),
                format!("process lookup: the {table} table could not be read: {reason}"),
            ),
            ReadError::Layout => (
                Failure::Layout(protocol),
                format!("process lookup: the {table} table has a layout this build does not know"),
            ),
        };
        if self.warnings.first_time(failure) {
            tracing::warn!("{message}");
        } else {
            tracing::debug!("{message}");
        }
    }
}

impl Default for Finder {
    fn default() -> Finder {
        Finder::new()
    }
}

/// The machine's addresses, and when they were listed.
struct Addresses {
    listed_at: Instant,
    list: Vec<IpAddr>,
}

/// What is known of one protocol's table.
#[derive(Default)]
struct Table {
    snapshot: Option<Snapshot>,
    /// When the last read of the table ended, whether or not it worked.
    last_read_ended: Option<Instant>,
}

/// One read of a table, and what has been asked of the system about it since.
struct Snapshot {
    /// When the read began: a connection that was open then, and is still, is
    /// in `rows`.
    at: Instant,
    rows: Vec<Row>,
    /// Who has a row's socket open. `None` is an answer too: the system
    /// would not say.
    pids: HashMap<RowOwner, Option<u32>>,
    /// What a process is. PIDs are reused, so this is the snapshot's, not the
    /// finder's.
    described: HashMap<u32, Option<(String, Option<String>)>>,
}

impl Snapshot {
    fn new(at: Instant, rows: Vec<Row>) -> Snapshot {
        Snapshot {
            at,
            rows,
            pids: HashMap::new(),
            described: HashMap::new(),
        }
    }

    /// What this snapshot says of the socket a query is about; `local` is the
    /// query's `local`, as `endpoint` gives it. The first of the source's rows
    /// whose process is known wins.
    ///
    /// TCP: a row of PID 0, the system's own, is no row: it is a connection
    /// that is closing, and does not hide the ones behind it. A row whose
    /// process cannot be told is a row that nobody is known to own.
    ///
    /// UDP: the first sort of address that has a socket decides. If no process
    /// is known for any of those sockets, the socket of a more general address
    /// is not the one the datagram came from.
    fn find(&mut self, source: &dyn Source, query: &Query, local: (IpAddr, u16)) -> Seen {
        let Snapshot { rows, pids, .. } = self;
        // `None`: nobody that can be seen has the socket open
        let mut pid_of = |owner: RowOwner| *pids.entry(owner).or_insert_with(|| source.pid(owner));
        match query.protocol {
            Protocol::Tcp => {
                let mut unowned = false;
                let connections = rows
                    .iter()
                    .filter(|row| endpoint(row.local) == local)
                    .filter(|row| {
                        let port = row.remote.map(|remote| remote.port());
                        query.remote_port.is_none_or(|wanted| port == Some(wanted))
                    });
                for row in connections {
                    match pid_of(row.owner) {
                        Some(0) => {}
                        Some(pid) => return Seen::By(pid),
                        None => unowned = true,
                    }
                }
                if unowned {
                    Seen::Unowned
                } else {
                    Seen::Missing
                }
            }
            Protocol::Udp => {
                for ip in udp_bindings(local.0) {
                    let mut sockets = rows
                        .iter()
                        .filter(|row| endpoint(row.local) == (ip, local.1))
                        .peekable();
                    if sockets.peek().is_some() {
                        let owned =
                            sockets.find_map(|row| pid_of(row.owner).filter(|&pid| pid != 0));
                        return owned.map_or(Seen::Unowned, Seen::By);
                    }
                }
                Seen::Missing
            }
        }
    }

    /// Whether this snapshot can answer a query, or the table has to be read
    /// again; `found` says whether the query's socket is in it. A snapshot
    /// taken before the session began cannot say that there is no such socket.
    /// One that lists the socket answers for TCP when it was taken no more than
    /// `TCP_GRACE` before the session began, for UDP when it was taken no
    /// earlier than that.
    fn covers(&self, protocol: Protocol, found: bool, since: Instant) -> bool {
        let grace = match protocol {
            Protocol::Tcp if found => TCP_GRACE,
            _ => Duration::ZERO,
        };
        // `at + grace >= since` and not `at >= since - grace`: near the origin
        // of the clock there is no such instant as `since - grace`
        self.at
            .checked_add(grace)
            .is_none_or(|covered_until| covered_until >= since)
    }

    /// The owner of what was seen: what the process is, asking the system once
    /// per process and snapshot.
    fn owner(&mut self, source: &dyn Source, seen: Seen) -> Option<Owner> {
        let Seen::By(pid) = seen else {
            return None;
        };
        let (name, path) = self
            .described
            .entry(pid)
            .or_insert_with(|| source.describe(pid))
            .clone()?;
        Some(Owner { pid, name, path })
    }
}

/// What a snapshot says of the socket of a query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    /// The table has no such socket.
    Missing,
    /// It has, and nobody that can be seen has it open.
    Unowned,
    /// It has, and this process does.
    By(u32),
}

/// An address and a port as the tables are compared: an IPv4 address in its
/// IPv6 form is the IPv4 address, and the flow label and the scope of an IPv6
/// address do not count.
fn endpoint(address: SocketAddr) -> (IpAddr, u16) {
    (address.ip().to_canonical(), address.port())
}

/// The addresses a UDP socket can be bound to such that a datagram from `ip`
/// came from it, best first: `ip` itself; then all addresses — `0.0.0.0` for
/// an IPv4 address, and then `::`, which a dual-stack socket is bound to; for
/// an IPv6 address `::` alone.
fn udp_bindings(ip: IpAddr) -> impl Iterator<Item = IpAddr> {
    const ANY_V4: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    const ANY_V6: IpAddr = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
    let all: &[IpAddr] = match ip {
        IpAddr::V4(_) => &[ANY_V4, ANY_V6],
        IpAddr::V6(_) => &[ANY_V6],
    };
    std::iter::once(ip).chain(all.iter().copied())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // a source that panicked has done its harm to its own lookup: the next one
    // is not to panic as well
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The failures of reading a table that are told apart for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    /// The system said no.
    Read(Protocol),
    /// The table is not laid out the way this build reads it.
    Layout(Protocol),
}

impl Failure {
    fn bit(self) -> u8 {
        match self {
            Failure::Read(Protocol::Tcp) => 1,
            Failure::Read(Protocol::Udp) => 2,
            Failure::Layout(Protocol::Tcp) => 4,
            Failure::Layout(Protocol::Udp) => 8,
        }
    }
}

/// Which failures a finder has warned about, so that one that happens at every
/// lookup is a warning once and a debug line after. A process has one finder,
/// so that is once in its life.
#[derive(Default)]
struct Warnings(AtomicU8);

impl Warnings {
    /// True the first time `failure` is asked about.
    fn first_time(&self, failure: Failure) -> bool {
        let bit = failure.bit();
        self.0.fetch_or(bit, Ordering::Relaxed) & bit == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddrV6;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Arc, Barrier, Condvar, mpsc};
    use std::thread;
    use tracing::Level;

    /// The longest a test waits for anything: a wait that long is a failure.
    const LIMIT: Duration = Duration::from_secs(10);
    const HOUR: Duration = Duration::from_secs(3600);

    /// The client's end of a connection to rurge's listening port, and rurge's
    /// own end of it.
    const CLIENT: &str = "127.0.0.1:50000";
    const PROXY: &str = "127.0.0.1:7890";
    const PROXY_PORT: u16 = 7890;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn tcp(local: &str, remote: &str, pid: u32) -> Row {
        Row {
            local: addr(local),
            remote: Some(addr(remote)),
            owner: RowOwner::Pid(pid),
        }
    }

    fn udp(local: &str, pid: u32) -> Row {
        Row {
            local: addr(local),
            remote: None,
            owner: RowOwner::Pid(pid),
        }
    }

    /// The client's row of the connection to rurge.
    fn client_row(pid: u32) -> Row {
        tcp(CLIENT, PROXY, pid)
    }

    fn tcp_query(local: &str, remote_port: u16, since: Instant) -> Query {
        Query {
            protocol: Protocol::Tcp,
            local: addr(local),
            remote_port: Some(remote_port),
            since,
        }
    }

    fn udp_query(local: &str, since: Instant) -> Query {
        Query {
            protocol: Protocol::Udp,
            local: addr(local),
            remote_port: None,
            since,
        }
    }

    /// One read of a table: which, when it began and when it ended.
    #[derive(Clone, Copy, Debug)]
    struct Read {
        protocol: Protocol,
        start: Instant,
        end: Instant,
    }

    #[derive(Default)]
    struct State {
        tcp: Vec<Row>,
        udp: Vec<Row>,
        /// What a read of a protocol fails with, instead of giving its table.
        failures: HashMap<Protocol, ReadError>,
        panic_in_next_read: bool,
        /// Who has the socket of an inode open.
        inodes: HashMap<u64, u32>,
        processes: HashMap<u32, (String, Option<String>)>,
        addresses: Vec<IpAddr>,
        /// How long a read takes.
        read_time: Duration,
        reads: Vec<Read>,
        pid_calls: Vec<RowOwner>,
        described: Vec<u32>,
        address_calls: usize,
    }

    /// Holds the reads of one protocol at the door until the door is opened.
    #[derive(Default)]
    struct Gate {
        state: Mutex<GateState>,
        changed: Condvar,
    }

    #[derive(Default)]
    struct GateState {
        shut_for: Option<Protocol>,
        waiting: usize,
    }

    impl Gate {
        fn shut(&self, protocol: Protocol) {
            lock(&self.state).shut_for = Some(protocol);
        }

        fn open(&self) {
            lock(&self.state).shut_for = None;
            self.changed.notify_all();
        }

        /// A read comes here and waits while the door is shut for its protocol.
        /// The wait is bounded, so that a test that fails with the door shut
        /// does not hang the run.
        fn pass(&self, protocol: Protocol) {
            let mut state = lock(&self.state);
            if state.shut_for != Some(protocol) {
                return;
            }
            state.waiting += 1;
            self.changed.notify_all();
            let (mut state, _) = self
                .changed
                .wait_timeout_while(state, LIMIT * 3, |state| state.shut_for == Some(protocol))
                .unwrap_or_else(PoisonError::into_inner);
            state.waiting -= 1;
        }

        /// Whether a read is at the door, waiting for it to open.
        fn wait_for_a_read(&self) -> bool {
            let state = lock(&self.state);
            let (state, _) = self
                .changed
                .wait_timeout_while(state, LIMIT, |state| state.waiting == 0)
                .unwrap_or_else(PoisonError::into_inner);
            state.waiting > 0
        }
    }

    /// Opens the door when it is dropped, so that a test that fails does not
    /// leave a read stuck at it.
    struct Shut<'a>(&'a Gate);

    impl Drop for Shut<'_> {
        fn drop(&mut self) {
            self.0.open();
        }
    }

    /// A machine that has the tables, processes and addresses a test gave it,
    /// and remembers what it was asked.
    #[derive(Default)]
    struct Fake {
        state: Mutex<State>,
        gate: Gate,
    }

    impl Fake {
        fn new() -> Arc<Fake> {
            Arc::new(Fake::default())
        }

        fn state(&self) -> MutexGuard<'_, State> {
            lock(&self.state)
        }

        fn set_tcp(&self, rows: Vec<Row>) {
            self.state().tcp = rows;
        }

        fn set_udp(&self, rows: Vec<Row>) {
            self.state().udp = rows;
        }

        fn set_addresses(&self, addresses: &[&str]) {
            self.state().addresses = addresses.iter().map(|text| ip(text)).collect();
        }

        fn process(&self, pid: u32, name: &str, path: Option<&str>) {
            self.state()
                .processes
                .insert(pid, (name.to_string(), path.map(str::to_string)));
        }

        fn forget_process(&self, pid: u32) {
            self.state().processes.remove(&pid);
        }

        fn own_inode(&self, inode: u64, pid: u32) {
            self.state().inodes.insert(inode, pid);
        }

        fn set_read_time(&self, time: Duration) {
            self.state().read_time = time;
        }

        fn fail(&self, protocol: Protocol, error: ReadError) {
            self.state().failures.insert(protocol, error);
        }

        fn mend(&self, protocol: Protocol) {
            self.state().failures.remove(&protocol);
        }

        fn panic_in_next_read(&self) {
            self.state().panic_in_next_read = true;
        }

        /// Shuts the door for the reads of `protocol`.
        fn hold(&self, protocol: Protocol) -> Shut<'_> {
            self.gate.shut(protocol);
            Shut(&self.gate)
        }

        fn reads_of(&self, protocol: Protocol) -> Vec<Read> {
            self.state()
                .reads
                .iter()
                .copied()
                .filter(|read| read.protocol == protocol)
                .collect()
        }

        fn reads(&self, protocol: Protocol) -> usize {
            self.reads_of(protocol).len()
        }

        fn pid_calls(&self) -> Vec<RowOwner> {
            self.state().pid_calls.clone()
        }

        fn described(&self) -> Vec<u32> {
            self.state().described.clone()
        }

        fn address_calls(&self) -> usize {
            self.state().address_calls
        }
    }

    impl Source for Arc<Fake> {
        fn read(&self, protocol: Protocol) -> Result<Vec<Row>, ReadError> {
            let start = Instant::now();
            self.gate.pass(protocol);
            let read_time = self.state().read_time;
            if !read_time.is_zero() {
                thread::sleep(read_time);
            }
            let mut state = self.state();
            if std::mem::take(&mut state.panic_in_next_read) {
                drop(state);
                panic!("the table cannot be read");
            }
            let result = match state.failures.get(&protocol) {
                Some(error) => Err(error.clone()),
                None => Ok(match protocol {
                    Protocol::Tcp => state.tcp.clone(),
                    Protocol::Udp => state.udp.clone(),
                }),
            };
            let end = Instant::now();
            state.reads.push(Read {
                protocol,
                start,
                end,
            });
            result
        }

        fn pid(&self, owner: RowOwner) -> Option<u32> {
            let mut state = self.state();
            state.pid_calls.push(owner);
            match owner {
                RowOwner::Pid(pid) => Some(pid),
                RowOwner::Inode(inode) => state.inodes.get(&inode).copied(),
            }
        }

        fn describe(&self, pid: u32) -> Option<(String, Option<String>)> {
            let mut state = self.state();
            state.described.push(pid);
            state.processes.get(&pid).cloned()
        }

        fn addresses(&self) -> Vec<IpAddr> {
            let mut state = self.state();
            state.address_calls += 1;
            state.addresses.clone()
        }
    }

    fn finder_with(fake: &Arc<Fake>, address_ttl: Duration) -> Finder {
        Finder::with_source(Box::new(fake.clone()), address_ttl)
    }

    /// A finder that believes the machine's addresses for an hour.
    fn finder_for(fake: &Arc<Fake>) -> Finder {
        finder_with(fake, HOUR)
    }

    /// When the finder's latest snapshot of a table was taken.
    fn taken_at(finder: &Finder, protocol: Protocol) -> Instant {
        lock(finder.table(protocol))
            .snapshot
            .as_ref()
            .expect("a snapshot")
            .at
    }

    /// The process a fresh finder finds for a query, on a machine whose
    /// table has these rows, whose processes are `p<pid>`, and one of whose
    /// addresses is the query's.
    fn lookup(rows: Vec<Row>, query: Query) -> Option<u32> {
        let fake = Fake::new();
        for row in &rows {
            if let RowOwner::Pid(pid) = row.owner {
                fake.process(pid, &format!("p{pid}"), None);
            }
        }
        match query.protocol {
            Protocol::Tcp => fake.set_tcp(rows),
            Protocol::Udp => fake.set_udp(rows),
        }
        fake.set_addresses(&[&query.local.ip().to_string()]);
        finder_for(&fake).owner(&query).map(|owner| owner.pid)
    }

    #[test]
    fn a_tcp_hit_in_a_recent_snapshot_needs_no_read() {
        let fake = Fake::new();
        fake.set_tcp(vec![client_row(100)]);
        fake.process(100, "client.exe", Some("C:\\apps\\client.exe"));
        let finder = finder_for(&fake);

        let owner = finder
            .owner(&tcp_query(CLIENT, PROXY_PORT, Instant::now()))
            .expect("the first look reads the table");
        assert_eq!(
            owner,
            Owner {
                pid: 100,
                name: "client.exe".to_string(),
                path: Some("C:\\apps\\client.exe".to_string()),
            }
        );
        assert_eq!(fake.reads(Protocol::Tcp), 1);

        // A session that began 1.5 s after the snapshot was taken is within the
        // 2 s: the connection was in the table before rurge accepted it.
        let at = taken_at(&finder, Protocol::Tcp);
        let later = at + Duration::from_millis(1500);
        assert_eq!(
            finder.owner(&tcp_query(CLIENT, PROXY_PORT, later)),
            Some(owner)
        );
        assert_eq!(fake.reads(Protocol::Tcp), 1);
    }

    #[test]
    fn a_tcp_hit_in_an_old_snapshot_reads_again() {
        let fake = Fake::new();
        fake.set_tcp(vec![client_row(100)]);
        fake.process(100, "client.exe", None);
        fake.process(200, "other.exe", None);
        let finder = finder_for(&fake);
        assert!(
            finder
                .owner(&tcp_query(CLIENT, PROXY_PORT, Instant::now()))
                .is_some()
        );
        let at = taken_at(&finder, Protocol::Tcp);

        // The port went to another process after the snapshot: a session that
        // began 2.5 s after it cannot rely on it, whatever it lists.
        fake.set_tcp(vec![client_row(200)]);
        let later = at + Duration::from_millis(2500);
        let owner = finder.owner(&tcp_query(CLIENT, PROXY_PORT, later));
        assert_eq!(owner.map(|owner| owner.pid), Some(200));
        assert_eq!(fake.reads(Protocol::Tcp), 2);
    }

    #[test]
    fn a_miss_in_a_snapshot_from_after_the_start_is_final() {
        let fake = Fake::new();
        fake.process(100, "client.exe", None);
        let finder = finder_for(&fake);
        let began = Instant::now();
        assert_eq!(finder.owner(&tcp_query(CLIENT, PROXY_PORT, began)), None);
        assert_eq!(fake.reads(Protocol::Tcp), 1);

        // The row is there now, but the snapshot is from after the session
        // began: a connection that was open then was in it.
        fake.set_tcp(vec![client_row(100)]);
        assert_eq!(finder.owner(&tcp_query(CLIENT, PROXY_PORT, began)), None);
        // not earlier than the start: a snapshot taken at the very instant counts
        let at = taken_at(&finder, Protocol::Tcp);
        assert_eq!(finder.owner(&tcp_query(CLIENT, PROXY_PORT, at)), None);
        assert_eq!(fake.reads(Protocol::Tcp), 1);
    }

    #[test]
    fn a_miss_in_an_earlier_snapshot_reads_again() {
        let fake = Fake::new();
        fake.process(100, "client.exe", None);
        let finder = finder_for(&fake);
        assert_eq!(
            finder.owner(&tcp_query(CLIENT, PROXY_PORT, Instant::now())),
            None
        );
        let at = taken_at(&finder, Protocol::Tcp);

        // A session that began a moment after the snapshot may have a
        // connection that is not in it. The 2 s are for connections that are:
        // a miss gets no grace.
        fake.set_tcp(vec![client_row(100)]);
        let later = at + Duration::from_millis(1);
        let owner = finder.owner(&tcp_query(CLIENT, PROXY_PORT, later));
        assert_eq!(owner.map(|owner| owner.pid), Some(100));
        assert_eq!(fake.reads(Protocol::Tcp), 2);
    }

    #[test]
    fn a_udp_hit_needs_a_snapshot_from_after_the_start() {
        let fake = Fake::new();
        fake.set_udp(vec![udp("127.0.0.1:5000", 100)]);
        fake.process(100, "client.exe", None);
        let finder = finder_for(&fake);
        let began = Instant::now();
        assert!(finder.owner(&udp_query("127.0.0.1:5000", began)).is_some());
        assert!(finder.owner(&udp_query("127.0.0.1:5000", began)).is_some());
        assert_eq!(fake.reads(Protocol::Udp), 1);

        // The grace of TCP is not for UDP: a socket has no remote end to check
        // the row against, so only a read from after the session began is
        // believed.
        let at = taken_at(&finder, Protocol::Udp);
        let later = at + Duration::from_millis(1500);
        assert!(finder.owner(&udp_query("127.0.0.1:5000", later)).is_some());
        assert_eq!(fake.reads(Protocol::Udp), 2);
    }

    #[test]
    fn one_read_answers_a_batch() {
        const CLIENTS: u32 = 16;
        let fake = Fake::new();
        // long enough for the others to queue behind the first
        fake.set_read_time(Duration::from_millis(30));
        let local = |i: u32| format!("127.0.0.1:{}", 50000 + i);
        fake.set_tcp(
            (0..CLIENTS)
                .map(|i| tcp(&local(i), PROXY, 100 + i))
                .collect(),
        );
        for i in 0..CLIENTS {
            fake.process(100 + i, &format!("app{i}.exe"), None);
        }
        let finder = finder_for(&fake);

        let began = Instant::now();
        let start = Barrier::new(CLIENTS as usize);
        let owners: Vec<Option<Owner>> = thread::scope(|scope| {
            let queries: Vec<_> = (0..CLIENTS)
                .map(|i| {
                    let (finder, start, local) = (&finder, &start, &local);
                    scope.spawn(move || {
                        start.wait();
                        finder.owner(&tcp_query(&local(i), PROXY_PORT, began))
                    })
                })
                .collect();
            queries
                .into_iter()
                .map(|query| query.join().unwrap())
                .collect()
        });

        for (i, owner) in owners.iter().enumerate() {
            let pid = 100 + i as u32;
            assert_eq!(owner.as_ref().map(|owner| owner.pid), Some(pid));
        }
        let reads = fake.reads(Protocol::Tcp);
        assert!(reads <= 2, "{reads} reads for {CLIENTS} queries");
        // and each process was asked about once
        let mut described = fake.described();
        described.sort_unstable();
        assert_eq!(described, (100..100 + CLIENTS).collect::<Vec<_>>());
    }

    #[test]
    fn reads_of_a_table_are_20_ms_apart() {
        let fake = Fake::new();
        fake.set_tcp(vec![client_row(100)]);
        fake.set_udp(vec![udp("127.0.0.1:5000", 100)]);
        fake.process(100, "client.exe", None);
        // a read that takes a while: the gap is counted from its end
        fake.set_read_time(Duration::from_millis(5));
        let finder = finder_for(&fake);

        // A session that begins an hour from now is not covered by any
        // snapshot, so every look reads.
        let never = Instant::now() + HOUR;
        for _ in 0..5 {
            assert!(
                finder
                    .owner(&tcp_query(CLIENT, PROXY_PORT, never))
                    .is_some()
            );
            assert!(finder.owner(&udp_query("127.0.0.1:5000", never)).is_some());
        }
        for protocol in [Protocol::Tcp, Protocol::Udp] {
            let reads = fake.reads_of(protocol);
            assert_eq!(reads.len(), 5, "{protocol:?}");
            for pair in reads.windows(2) {
                let gap = pair[1].start.duration_since(pair[0].end);
                assert!(gap >= READ_GAP, "{protocol:?}: {gap:?} between two reads");
            }
        }

        // The tables do not wait for each other: while a read of one of them
        // is held, a look at the other is answered.
        for (held, other) in [
            (Protocol::Tcp, Protocol::Udp),
            (Protocol::Udp, Protocol::Tcp),
        ] {
            let fake = Fake::new();
            fake.set_tcp(vec![client_row(100)]);
            fake.set_udp(vec![udp("127.0.0.1:5000", 100)]);
            fake.process(100, "client.exe", None);
            let finder = finder_for(&fake);
            let query = |protocol| match protocol {
                Protocol::Tcp => tcp_query(CLIENT, PROXY_PORT, never),
                Protocol::Udp => udp_query("127.0.0.1:5000", never),
            };
            thread::scope(|scope| {
                let (finder, query) = (&finder, &query);
                let door = fake.hold(held);
                let stuck = scope.spawn(move || finder.owner(&query(held)));
                assert!(
                    fake.gate.wait_for_a_read(),
                    "the read of the {held:?} table never began"
                );
                let (sender, answered) = mpsc::channel();
                scope.spawn(move || {
                    let _ = sender.send(finder.owner(&query(other)));
                });
                let answer = answered.recv_timeout(LIMIT);
                drop(door);
                let answer = answer.unwrap_or_else(|_| {
                    panic!("the {other:?} table waited for a read of the {held:?} table")
                });
                assert_eq!(answer.map(|owner| owner.pid), Some(100));
                assert_eq!(stuck.join().unwrap().map(|owner| owner.pid), Some(100));
            });
        }
    }

    #[test]
    fn udp_falls_back_to_the_unspecified_address() {
        // The rows of the table, in order; the client's address; who is found.
        type Case<'a> = (&'a [(&'a str, u32)], &'a str, Option<u32>);
        let cases: &[Case] = &[
            // the exact endpoint first, whatever comes before it
            (
                &[("0.0.0.0:5000", 2), ("[::]:5000", 3), ("127.0.0.1:5000", 1)],
                "127.0.0.1:5000",
                Some(1),
            ),
            // then 0.0.0.0, and then :: — a dual-stack socket takes IPv4 too
            (
                &[("[::]:5000", 3), ("0.0.0.0:5000", 2)],
                "127.0.0.1:5000",
                Some(2),
            ),
            (&[("[::]:5000", 3)], "127.0.0.1:5000", Some(3)),
            // another port is another socket; so is another address
            (
                &[("0.0.0.0:5001", 2), ("[::]:5001", 3), ("127.0.0.1:5001", 1)],
                "127.0.0.1:5000",
                None,
            ),
            (&[("127.0.0.2:5000", 1)], "127.0.0.1:5000", None),
            // an IPv4 address in its IPv6 form is the exact endpoint
            (
                &[("0.0.0.0:5000", 2), ("[::ffff:127.0.0.1]:5000", 1)],
                "127.0.0.1:5000",
                Some(1),
            ),
            // an IPv6 client: the exact endpoint, then ::, never 0.0.0.0
            (&[("0.0.0.0:5000", 2)], "[::1]:5000", None),
            (
                &[("0.0.0.0:5000", 2), ("[::]:5000", 3)],
                "[::1]:5000",
                Some(3),
            ),
            (
                &[("0.0.0.0:5000", 2), ("[::]:5000", 3), ("[::1]:5000", 1)],
                "[::1]:5000",
                Some(1),
            ),
        ];
        for (rows, client, expected) in cases {
            let rows = rows.iter().map(|(local, pid)| udp(local, *pid)).collect();
            let found = lookup(rows, udp_query(client, Instant::now()));
            assert_eq!(found, *expected, "{client}");
        }
    }

    #[test]
    fn an_ipv4_query_finds_the_mapped_row() {
        // A dual-stack socket that connected to an IPv4 address is in the IPv6
        // table with the address in its mapped form.
        let mapped = tcp("[::ffff:127.0.0.1]:50000", "[::ffff:127.0.0.1]:7890", 100);
        let query = tcp_query(CLIENT, PROXY_PORT, Instant::now());
        assert_eq!(lookup(vec![mapped.clone()], query), Some(100));

        // and the other way round
        let mapped_query = tcp_query("[::ffff:127.0.0.1]:50000", PROXY_PORT, Instant::now());
        assert_eq!(lookup(vec![client_row(100)], mapped_query), Some(100));

        // the IPv4 table is read first: the first row of the source wins
        let rows = vec![client_row(100), mapped.clone()];
        assert_eq!(lookup(rows, query), Some(100));
        let rows = vec![
            tcp("[::ffff:127.0.0.1]:50000", "[::ffff:127.0.0.1]:7890", 200),
            client_row(100),
        ];
        assert_eq!(lookup(rows, query), Some(200));

        // IPv6 loopback is itself
        let ipv6 = tcp("[::1]:50000", "[::1]:7890", 300);
        let query = tcp_query("[::1]:50000", PROXY_PORT, Instant::now());
        assert_eq!(lookup(vec![client_row(100), ipv6], query), Some(300));
    }

    #[test]
    fn the_scope_id_is_not_compared() {
        let with_scope = |ip: &str, port: u16, flowinfo: u32, scope: u32| {
            SocketAddr::V6(SocketAddrV6::new(
                ip.parse().unwrap(),
                port,
                flowinfo,
                scope,
            ))
        };
        let row = Row {
            local: with_scope("fe80::1", 50000, 0, 7),
            remote: Some(with_scope("fe80::2", PROXY_PORT, 0, 7)),
            owner: RowOwner::Pid(100),
        };
        let query = |ip: &str| Query {
            protocol: Protocol::Tcp,
            local: with_scope(ip, 50000, 0x1234, 5),
            remote_port: Some(PROXY_PORT),
            since: Instant::now(),
        };
        assert_eq!(lookup(vec![row.clone()], query("fe80::1")), Some(100));
        // the address is compared, only the scope is not
        assert_eq!(lookup(vec![row], query("fe80::3")), None);
    }

    #[test]
    fn a_tcp_query_finds_the_clients_end_of_the_connection() {
        let proxys_end = tcp(PROXY, CLIENT, 200);
        let rows = || vec![proxys_end.clone(), client_row(100)];
        let now = Instant::now;
        // both ends of a loopback connection are in the table: the client's is
        // the one whose local endpoint is the session's source and whose
        // remote port is the one it connected to
        assert_eq!(
            lookup(rows(), tcp_query(CLIENT, PROXY_PORT, now())),
            Some(100)
        );
        // a connection to another port is not this session
        assert_eq!(lookup(rows(), tcp_query(CLIENT, 7891, now())), None);
        // a TCP query without a remote port goes by the local endpoint alone
        let mut query = tcp_query(CLIENT, PROXY_PORT, now());
        query.remote_port = None;
        assert_eq!(lookup(rows(), query), Some(100));
        // a row that does not say where it is connected to is not the one
        let lonely = Row {
            remote: None,
            ..client_row(100)
        };
        assert_eq!(
            lookup(vec![lonely.clone()], tcp_query(CLIENT, PROXY_PORT, now())),
            None
        );
        let mut query = tcp_query(CLIENT, PROXY_PORT, now());
        query.remote_port = None;
        assert_eq!(lookup(vec![lonely], query), Some(100));
    }

    #[test]
    fn a_source_that_is_not_this_machine_reads_nothing() {
        let fake = Fake::new();
        fake.set_addresses(&["192.0.2.10"]);
        // were the table read, this row would be found
        fake.set_tcp(vec![tcp("192.0.2.99:50000", "192.0.2.10:7890", 100)]);
        fake.set_udp(vec![udp("192.0.2.99:50000", 100)]);
        fake.process(100, "client.exe", None);
        let finder = finder_for(&fake);

        let tcp_from = |local: &str| tcp_query(local, PROXY_PORT, Instant::now());
        assert_eq!(finder.owner(&tcp_from("192.0.2.99:50000")), None);
        assert_eq!(
            finder.owner(&udp_query("192.0.2.99:50000", Instant::now())),
            None
        );
        assert_eq!(fake.reads(Protocol::Tcp), 0);
        assert_eq!(fake.reads(Protocol::Udp), 0);

        // an address that is on the list is looked up, in whichever form
        fake.set_tcp(vec![tcp("192.0.2.10:50000", "192.0.2.10:7890", 100)]);
        let owner = finder.owner(&tcp_from("192.0.2.10:50000"));
        assert_eq!(owner.map(|owner| owner.pid), Some(100));
        assert_eq!(fake.reads(Protocol::Tcp), 1);
        assert!(
            finder
                .owner(&tcp_from("[::ffff:192.0.2.10]:50000"))
                .is_some()
        );
        // the list was asked for once
        assert_eq!(fake.address_calls(), 1);
    }

    #[test]
    fn the_address_list_follows_its_ttl() {
        let machine = || {
            let fake = Fake::new();
            fake.set_addresses(&["192.0.2.10", "2001:db8::10"]);
            fake.set_tcp(vec![tcp("192.0.2.10:50000", "192.0.2.10:7890", 100)]);
            fake.process(100, "client.exe", None);
            fake
        };
        let from_the_machine = || tcp_query("192.0.2.10:50000", PROXY_PORT, Instant::now());
        let loopbacks = ["127.0.0.1", "127.9.9.9", "::1", "::ffff:127.0.0.1"];

        // A list that is not believed for any time: every look lists again, and
        // the peek never knows.
        let fake = machine();
        let finder = finder_with(&fake, Duration::ZERO);
        assert_eq!(finder.is_local(ip("192.0.2.10")), None);
        assert!(finder.owner(&from_the_machine()).is_some());
        assert!(finder.owner(&from_the_machine()).is_some());
        assert_eq!(fake.address_calls(), 2);
        assert_eq!(finder.is_local(ip("192.0.2.10")), None);
        assert_eq!(finder.is_local(ip("192.0.2.99")), None);

        // A list that is believed for an hour: listed once, then the peek
        // answers from it, in either form of an address.
        let fake = machine();
        let finder = finder_for(&fake);
        assert_eq!(finder.is_local(ip("192.0.2.10")), None);
        assert!(finder.owner(&from_the_machine()).is_some());
        assert!(finder.owner(&from_the_machine()).is_some());
        assert_eq!(fake.address_calls(), 1);
        assert_eq!(finder.is_local(ip("192.0.2.10")), Some(true));
        assert_eq!(finder.is_local(ip("::ffff:192.0.2.10")), Some(true));
        assert_eq!(finder.is_local(ip("2001:db8::10")), Some(true));
        assert_eq!(finder.is_local(ip("192.0.2.99")), Some(false));
        assert_eq!(finder.is_local(ip("2001:db8::99")), Some(false));

        // Loopback is the machine's without any list, and asks for none.
        for ttl in [Duration::ZERO, HOUR] {
            let fake = machine();
            let finder = finder_with(&fake, ttl);
            for text in loopbacks {
                assert_eq!(finder.is_local(ip(text)), Some(true), "{text}");
                let mut query = tcp_query(CLIENT, PROXY_PORT, Instant::now());
                query.local = SocketAddr::new(ip(text), 50000);
                let _ = finder.owner(&query);
            }
            assert_eq!(fake.address_calls(), 0);
        }
    }

    #[test]
    fn a_process_is_described_once_per_snapshot() {
        let fake = Fake::new();
        fake.set_tcp(vec![
            client_row(100),
            tcp("127.0.0.1:50001", PROXY, 100),
            tcp("127.0.0.1:50002", PROXY, 300),
        ]);
        fake.process(100, "client.exe", None);
        let finder = finder_for(&fake);
        let began = Instant::now();
        let ask = |local: &str| finder.owner(&tcp_query(local, PROXY_PORT, began));

        // one process, two connections, three looks: asked about once
        assert!(ask(CLIENT).is_some());
        assert!(ask("127.0.0.1:50001").is_some());
        assert!(ask(CLIENT).is_some());
        assert_eq!(fake.described(), [100]);
        assert_eq!(fake.reads(Protocol::Tcp), 1);

        // a process that cannot be described is remembered as such
        assert_eq!(ask("127.0.0.1:50002"), None);
        assert_eq!(ask("127.0.0.1:50002"), None);
        assert_eq!(fake.described(), [100, 300]);

        // A new snapshot asks again: a PID is reused, and the process behind
        // it may be another.
        fake.process(100, "another.exe", Some("/usr/bin/another"));
        let never = Instant::now() + HOUR;
        let owner = finder.owner(&tcp_query(CLIENT, PROXY_PORT, never));
        assert_eq!(
            owner.map(|owner| owner.name),
            Some("another.exe".to_string())
        );
        assert_eq!(fake.described(), [100, 300, 100]);
    }

    #[test]
    fn a_pid_is_looked_up_once_per_row_and_snapshot() {
        let fake = Fake::new();
        let inode_row = |inode: u64, local: &str| Row {
            owner: RowOwner::Inode(inode),
            ..tcp(local, PROXY, 0)
        };
        fake.set_tcp(vec![inode_row(7, CLIENT), inode_row(8, "127.0.0.1:50001")]);
        fake.own_inode(7, 100);
        fake.process(100, "client.exe", None);
        let finder = finder_for(&fake);
        let began = Instant::now();
        let ask = |local: &str| finder.owner(&tcp_query(local, PROXY_PORT, began));

        // Finding who has a socket open can be a scan of every process: it is
        // done once for a row, and a row nobody is known to have is
        // remembered too.
        assert_eq!(ask(CLIENT).map(|owner| owner.pid), Some(100));
        assert_eq!(ask(CLIENT).map(|owner| owner.pid), Some(100));
        assert_eq!(ask("127.0.0.1:50001"), None);
        assert_eq!(ask("127.0.0.1:50001"), None);
        assert_eq!(fake.pid_calls(), [RowOwner::Inode(7), RowOwner::Inode(8)]);

        // a row without a process is no row, for the rows behind it
        fake.set_tcp(vec![inode_row(8, CLIENT), inode_row(7, CLIENT)]);
        let never = Instant::now() + HOUR;
        let owner = finder.owner(&tcp_query(CLIENT, PROXY_PORT, never));
        assert_eq!(owner.map(|owner| owner.pid), Some(100));
    }

    #[test]
    fn a_row_whose_process_cannot_be_seen_is_found_but_has_no_owner() {
        let fake = Fake::new();
        // an inode that nobody that can be seen has open: the process of
        // another user, or one that has gone
        fake.set_tcp(vec![Row {
            owner: RowOwner::Inode(9),
            ..client_row(0)
        }]);
        let finder = finder_for(&fake);
        let ask = |local: &str, since| finder.owner(&tcp_query(local, PROXY_PORT, since));
        assert_eq!(ask(CLIENT, Instant::now()), None);
        assert_eq!(fake.reads(Protocol::Tcp), 1);

        // The socket is in the snapshot, so a session that began a moment
        // after it is within the 2 s: the table is not read again, and the
        // process not looked for again, for every request of the connection.
        let at = taken_at(&finder, Protocol::Tcp);
        let later = at + Duration::from_millis(1500);
        assert_eq!(ask(CLIENT, later), None);
        assert_eq!(fake.reads(Protocol::Tcp), 1);
        assert_eq!(fake.pid_calls(), [RowOwner::Inode(9)]);

        // that is for a socket that is there
        assert_eq!(ask("127.0.0.1:50001", later), None);
        assert_eq!(fake.reads(Protocol::Tcp), 2);
    }

    #[test]
    fn a_row_that_is_gone_is_none() {
        let fake = Fake::new();
        fake.set_tcp(vec![tcp("127.0.0.1:50009", PROXY, 100)]);
        fake.process(100, "client.exe", None);
        let finder = finder_for(&fake);
        let began = Instant::now();

        // the client left before the table was read: its row is not there
        assert_eq!(finder.owner(&tcp_query(CLIENT, PROXY_PORT, began)), None);

        // The row is there and the process has exited since: the system does
        // not know it any more.
        fake.set_tcp(vec![client_row(100)]);
        fake.forget_process(100);
        let never = Instant::now() + HOUR;
        assert_eq!(finder.owner(&tcp_query(CLIENT, PROXY_PORT, never)), None);
        assert_eq!(fake.described(), [100]);
    }

    #[test]
    fn rows_with_pid_0_are_never_an_owner() {
        let fake = Fake::new();
        fake.set_tcp(vec![client_row(0)]);
        fake.process(0, "System Idle Process", None);
        let finder = finder_for(&fake);
        assert_eq!(
            finder.owner(&tcp_query(CLIENT, PROXY_PORT, Instant::now())),
            None
        );
        assert!(!fake.described().contains(&0));
        assert_eq!(fake.reads(Protocol::Tcp), 1);

        // It is not a connection that is there, either: a session that began
        // after the snapshot does not rely on it, whatever the 2 s.
        let at = taken_at(&finder, Protocol::Tcp);
        let later = at + Duration::from_millis(1500);
        assert_eq!(finder.owner(&tcp_query(CLIENT, PROXY_PORT, later)), None);
        assert_eq!(fake.reads(Protocol::Tcp), 2);

        // a row of PID 0 does not hide the live one behind it
        let rows = vec![client_row(0), client_row(100)];
        let query = tcp_query(CLIENT, PROXY_PORT, Instant::now());
        assert_eq!(lookup(rows, query), Some(100));

        // On UDP the first sort of address that has a socket decides: that
        // socket has no owner, and the socket of another address is not it.
        let rows = vec![udp("127.0.0.1:5000", 0), udp("0.0.0.0:5000", 100)];
        let query = udp_query("127.0.0.1:5000", Instant::now());
        assert_eq!(lookup(rows, query), None);
    }

    #[test]
    fn a_row_that_names_its_process_has_that_pid_by_default() {
        assert_eq!(NoTables.pid(RowOwner::Pid(7)), Some(7));
        assert_eq!(NoTables.pid(RowOwner::Inode(7)), None);
    }

    #[test]
    fn a_machine_without_tables_has_no_owners() {
        let finder = Finder::with_source(Box::new(NoTables), ADDRESS_TTL);
        let query = tcp_query(CLIENT, PROXY_PORT, Instant::now());
        assert_eq!(finder.owner(&query), None);
        assert_eq!(finder.owner(&udp_query(CLIENT, Instant::now())), None);
        assert_eq!(finder.is_local(ip("127.0.0.1")), Some(true));
    }

    #[test]
    fn a_failed_read_is_none_and_the_next_look_reads_again() {
        let fake = Fake::new();
        fake.set_tcp(vec![client_row(100)]);
        fake.process(100, "client.exe", None);
        let finder = finder_for(&fake);
        let never = Instant::now() + HOUR;
        let ask = || finder.owner(&tcp_query(CLIENT, PROXY_PORT, never));
        assert!(ask().is_some());

        // The earlier snapshot has the row, but it is too old to answer and
        // the new read fails: not known.
        fake.fail(Protocol::Tcp, ReadError::Os("access denied".to_string()));
        assert_eq!(ask(), None);
        assert_eq!(ask(), None);
        assert_eq!(fake.reads(Protocol::Tcp), 3);

        // the failure was not remembered
        fake.mend(Protocol::Tcp);
        assert!(ask().is_some());
        assert_eq!(fake.reads(Protocol::Tcp), 4);
    }

    #[test]
    fn a_source_that_panics_does_not_break_later_lookups() {
        let fake = Fake::new();
        fake.set_tcp(vec![client_row(100)]);
        fake.process(100, "client.exe", None);
        let finder = finder_for(&fake);
        let ask = || finder.owner(&tcp_query(CLIENT, PROXY_PORT, Instant::now()));
        fake.panic_in_next_read();
        assert!(catch_unwind(AssertUnwindSafe(ask)).is_err());
        // the lock of the table was poisoned, and the finder goes on
        assert_eq!(ask().map(|owner| owner.pid), Some(100));
    }

    #[test]
    fn warnings_say_each_failure_once() {
        let warnings = Warnings::default();
        let failures = [
            Failure::Read(Protocol::Tcp),
            Failure::Read(Protocol::Udp),
            Failure::Layout(Protocol::Tcp),
            Failure::Layout(Protocol::Udp),
        ];
        for failure in failures {
            assert!(warnings.first_time(failure), "{failure:?} the first time");
        }
        for failure in failures {
            assert!(!warnings.first_time(failure), "{failure:?} the second time");
            assert!(!warnings.first_time(failure), "{failure:?} the third time");
        }
    }

    /// The lines that are logged, with their levels.
    #[derive(Clone, Default)]
    struct Lines(Arc<Mutex<Vec<(Level, String)>>>);

    impl tracing::Subscriber for Lines {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            struct Message(String);
            impl tracing::field::Visit for Message {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" {
                        self.0 = format!("{value:?}");
                    }
                }
            }
            let mut message = Message(String::new());
            event.record(&mut message);
            lock(&self.0).push((*event.metadata().level(), message.0));
        }

        fn enter(&self, _: &tracing::span::Id) {}

        fn exit(&self, _: &tracing::span::Id) {}
    }

    #[test]
    fn a_failed_read_is_a_warning_the_first_time_and_a_debug_line_after() {
        let fake = Fake::new();
        let finder = finder_for(&fake);
        let lines = Lines::default();
        // a look at a table that has no snapshot reads it
        let ask = |protocol| {
            let since = Instant::now();
            match protocol {
                Protocol::Tcp => finder.owner(&tcp_query(CLIENT, PROXY_PORT, since)),
                Protocol::Udp => finder.owner(&udp_query(CLIENT, since)),
            }
        };
        tracing::subscriber::with_default(lines.clone(), || {
            fake.fail(Protocol::Tcp, ReadError::Os("access denied".to_string()));
            for _ in 0..3 {
                assert_eq!(ask(Protocol::Tcp), None);
            }
            fake.fail(Protocol::Tcp, ReadError::Layout);
            for _ in 0..2 {
                assert_eq!(ask(Protocol::Tcp), None);
            }
            fake.fail(Protocol::Udp, ReadError::Os("access denied".to_string()));
            assert_eq!(ask(Protocol::Udp), None);
        });

        let tcp_read = "process lookup: the tcp table could not be read: access denied";
        let tcp_layout = "process lookup: the tcp table has a layout this build does not know";
        let udp_read = "process lookup: the udp table could not be read: access denied";
        let expected = [
            (Level::WARN, tcp_read),
            (Level::DEBUG, tcp_read),
            (Level::DEBUG, tcp_read),
            (Level::WARN, tcp_layout),
            (Level::DEBUG, tcp_layout),
            (Level::WARN, udp_read),
        ];
        let said = lock(&lines.0).clone();
        let said: Vec<(Level, &str)> = said
            .iter()
            .map(|(level, text)| (*level, text.as_str()))
            .collect();
        assert_eq!(said, expected);
    }

    #[test]
    fn the_finder_can_be_shared_between_threads() {
        fn shared<T: Send + Sync>() {}
        shared::<Finder>();
    }
}
