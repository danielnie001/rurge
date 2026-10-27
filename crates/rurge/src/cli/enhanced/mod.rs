//! The enhanced mode as `rurge run` drives it (M4 design §9): the manager,
//! what it asks of the system, the backup in `state.json` that lets a crash be
//! undone, and the TUN behind it.
//!
//! Nothing here reaches the machine of its own accord: the backend does what
//! the steps say — on Linux and macOS the platform's own system, which changes
//! it, elsewhere a system that answers that the enhanced mode is not available
//! — and the file backend (`RURGE_ENHANCED_BACKEND=file:<path>`) is the one
//! tests set. The backend's actions are all blocking and all called on a
//! blocking thread; the manager is owned by the `rurge run` main loop, which
//! calls it one request at a time.

mod backend;
mod device;
mod engine;
mod file;
mod undo;
pub(crate) mod wanted;

use super::network::Exclude;
pub use backend::backend;
use backend::{Backend, Opened};
pub use engine::starter;
use ipnet::IpNet;
use rurge_engine::state::StateStore;
use rurge_net::BoxFuture;
use rurge_platform::route::RouteSet;
use rurge_platform::socket::egress::EgressTable;
use rurge_tun::Ending;
use rurge_tun::device::Device;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::timeout;
use undo::{Backup, Stage, Undo};
use wanted::Wanted;

/// How long a close waits for the TUN's tasks before going on (M4 design 9.4;
/// M3b's delayed item 8: a dial that never comes back holds `join`).
const JOIN_LIMIT: Duration = Duration::from_secs(3);

/// The egress table of an enhanced mode that is on, and `None` while it is off
/// (M4 design 7.1). The main loop writes it and never holds the lock across an
/// await; M4b's `PlatformSockets::egress` reads it.
pub type EgressSlot = Arc<std::sync::RwLock<Option<Arc<EgressTable>>>>;

/// An empty egress slot: what a run that has not turned the enhanced mode on
/// yet has.
pub fn egress_slot() -> EgressSlot {
    Arc::new(std::sync::RwLock::new(None))
}

/// Writes the egress table the manager wants in the slot, unless the slot
/// already holds that table: `false` then, the same `Arc` left in place, and
/// `true` once another table went in. The `Arc` is what a socket that
/// outlives one connection compares (`Route::egress`, by pointer), so a table
/// whose content did not change must not be written again: every WireGuard
/// carrier and every QUIC connection would be made again on every reload
/// otherwise (the phase 3 / M4b plan's execution record rows 68 and 69).
fn set_egress(slot: &EgressSlot, table: Option<EgressTable>) -> bool {
    let mut slot = slot.write().unwrap_or_else(|e| e.into_inner());
    if slot.as_deref() == table.as_ref() {
        return false;
    }
    *slot = table.map(Arc::new);
    true
}

/// Whether the egress table is in the slot: the mock backend and the tests ask
/// (the reader that decides, `PlatformSockets`, reads the slot itself).
#[cfg_attr(not(test), allow(dead_code))]
fn filled(slot: &EgressSlot) -> bool {
    slot.read().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// What the manager tells the rest of rurge as the enhanced mode opens and
/// closes (M4 design 3.7, 6.3, 7.1). Given once, when the manager is made: the
/// manager writes it and the rest of the run reads it.
pub struct Links {
    /// The egress table, which `PlatformSockets` reads.
    pub egress: EgressSlot,
    /// Up while the egress table is written: `PlatformSystemDns::enhanced_mode`.
    pub enhanced: Arc<AtomicBool>,
    /// The interfaces the network task leaves out of what it reads.
    pub exclude: Exclude,
    /// The system's DNS changed hands: `Engine::system_dns_changed` in a run.
    /// The manager calls it as the enhanced mode takes the DNS over and gives
    /// it back, and after it wrote another egress table without taking the
    /// DNS over again: the resolver's upstreams are bound to the table there
    /// was, so they are built again on the new one (M4 design 7.2; the phase
    /// 3 / M4b plan's execution record row 69).
    pub system_dns_changed: Arc<dyn Fn() + Send + Sync>,
    /// The DNS servers of the network outside the enhanced mode, while it is
    /// on: `PlatformSystemDns::physical`, which the system's DNS reads for
    /// `server:system` (M4 design 6.3). Written after every DNS takeover and
    /// cleared as the DNS is given back; `None` is the system's own list.
    pub physical: Arc<std::sync::RwLock<Option<Vec<SocketAddr>>>>,
}

/// Starts the TUN the manager runs: the engine's, or a test's (P6: the engine
/// cannot make a TUN that never ends, and a close's limit needs one).
pub trait Starter: Send + Sync {
    fn start(&self, device: Arc<dyn Device>) -> BoxFuture<'_, Result<Arc<dyn RunningTun>, String>>;
}

/// A TUN the manager runs.
pub trait RunningTun: Send + Sync {
    fn stop(&self);
    /// Ends when the TUN and all it holds of the engine have ended
    /// (`TunHandle::join`, then `Engine::tun_ended`), and says how: a device
    /// that failed ends the enhanced mode (P24, task 8).
    fn ended(&self) -> BoxFuture<'_, Ending>;
    /// Ends when the TUN itself has been told to end (a stop, or a device that
    /// failed), with its first reason, without waiting for its tasks or for the
    /// engine's last save: what the task that watches for a device that fails
    /// waits for (the phase 3 / M4a plan's execution record rows 62 and 100:
    /// waiting on `Engine::tun_ended` here would hold the engine's one TUN's
    /// place, and an open right after a close would be refused; waiting for all
    /// of the TUN's tasks would hold it for a dial that might never come back).
    fn ending(&self) -> BoxFuture<'_, Ending>;
    fn counters(&self) -> rurge_engine::TunCounters;
}

/// For the API: whether the enhanced mode is on, and its TUN's counters.
pub struct Shared {
    enabled: AtomicBool,
    /// The TUN the counters come from while it runs.
    tun: Mutex<Option<Arc<dyn RunningTun>>>,
}

impl Shared {
    pub fn new() -> Shared {
        Shared {
            enabled: AtomicBool::new(false),
            tun: Mutex::new(None),
        }
    }

    pub fn status(&self) -> rurge_engine::EnhancedStatus {
        let tun = self.tun.lock().unwrap_or_else(|e| e.into_inner());
        let enabled = self.enabled.load(Ordering::SeqCst);
        rurge_engine::EnhancedStatus {
            enabled,
            counters: match (enabled, tun.as_ref()) {
                (true, Some(tun)) => tun.counters(),
                _ => rurge_engine::TunCounters::default(),
            },
        }
    }

    fn set_on(&self, tun: Arc<dyn RunningTun>) {
        *self.tun.lock().unwrap_or_else(|e| e.into_inner()) = Some(tun);
        self.enabled.store(true, Ordering::SeqCst);
    }

    fn set_off(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        *self.tun.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Told the number of an open whose TUN failed (P24; the phase 3 / M4a plan's
/// execution record row 63): the `rurge run` main loop turns the enhanced mode
/// off when the number is the one that is on. Not the main loop's `Command`:
/// this module knows nothing of it.
pub type TunEnded = Arc<dyn Fn(u64) + Send + Sync>;

/// Owned by the `rurge run` main loop, which calls it one request at a time.
pub struct EnhancedMode {
    backend: Arc<dyn Backend>,
    store: Arc<StateStore>,
    starter: Arc<dyn Starter>,
    /// What the rest of rurge is told as the mode opens and closes.
    links: Links,
    shared: Arc<Shared>,
    /// Where a TUN that failed is told (P24).
    ended: TunEnded,
    /// How many opens have succeeded: each is numbered from one (the phase 3 /
    /// M4a plan's execution record row 63).
    opens: u64,
    /// What is on, and what a close has to undo.
    on: Option<On>,
}

/// What the enhanced mode did and holds while it is on; while it opens, the
/// part of that it has got to.
#[derive(Default)]
struct On {
    /// The number of this open: a TUN's failure is only news if it is this
    /// open's (the phase 3 / M4a plan's execution record row 63).
    sequence: u64,
    dev: Option<Opened>,
    tun: Option<Arc<dyn RunningTun>>,
    /// The routes installed into the adapter.
    routes: Option<RouteSet>,
    /// The adapter has its IPv6 half (M4 design 4.6): whether `ipv6-vif` and
    /// the network made it, and `configure_ipv6` could give it.
    ipv6: bool,
    /// The egress table is in the slot.
    egress: bool,
    /// The system's DNS is taken over, or its takeover was attempted (a
    /// takeover that fails may have changed half of it).
    dns: bool,
    /// The system's own adjustments (`rp_filter`) are made or attempted.
    adjust: bool,
}

/// What a close met: the first error, and where in `state.json`'s backup the
/// undo actions that did go back are. What did not go back stays for the next
/// start — the ones that failed, and the ones the close never reached (a close
/// also undoes a failed open, and an open that failed early never got to the
/// stages that undo them).
#[derive(Default)]
struct Closing {
    first: Option<String>,
    /// The positions, in the backup the close read, of what went back.
    undone: Vec<usize>,
}

impl Closing {
    fn keep(&mut self, step: &str, error: String) {
        tracing::error!(
            step,
            error = %error,
            "cannot undo part of the enhanced mode; going on with the rest"
        );
        if self.first.is_none() {
            self.first = Some(error);
        }
    }

    /// What is left of `saved`, in its order: everything no close of this one
    /// undid.
    fn remaining(&self, saved: &[Undo]) -> Vec<Undo> {
        saved
            .iter()
            .enumerate()
            .filter(|(index, _)| !self.undone.contains(index))
            .map(|(_, undo)| undo.clone())
            .collect()
    }
}

/// `state.json`'s `enhanced_backup` for these undo actions: none when there are
/// none, which is what "nothing of it is left" looks like.
fn backup_value(undo: &[Undo]) -> Option<serde_json::Value> {
    if undo.is_empty() {
        return None;
    }
    Some(
        serde_json::to_value(Backup {
            undo: undo.to_vec(),
        })
        .expect("Backup serializes"),
    )
}

/// The egress table for these routes, the network's primary interface and the
/// physical interfaces' subnets (the manager reads them, most specific first).
fn table(routes: &RouteSet, wanted: &Wanted, subnets: &[(IpNet, String)]) -> EgressTable {
    EgressTable {
        routes: routes.clone(),
        primary: wanted.primary.clone(),
        subnets: subnets.to_vec(),
    }
}

/// The routes of `a` and `b` together, each once, sorted: what may be in the
/// adapter while a change of routes is being made (the phase 3 / M4a plan's
/// execution record row 75).
fn union(a: &RouteSet, b: &RouteSet) -> RouteSet {
    let mut together = a.clone();
    for net in &b.v4 {
        if !together.v4.contains(net) {
            together.v4.push(*net);
        }
    }
    for net in &b.v6 {
        if !together.v6.contains(net) {
            together.v6.push(*net);
        }
    }
    together.v4.sort();
    together.v6.sort();
    together
}

/// One backend call, off the runtime: every one of them blocks (programs,
/// files).
async fn call<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    match tokio::task::spawn_blocking(job).await {
        Ok(result) => result,
        Err(e) => Err(format!("the enhanced mode's backend task failed: {e}")),
    }
}

impl EnhancedMode {
    /// Startup, under the data directory's lock (M4 design 9.5): the undo
    /// actions a run that died left in `state.json` are done, last first; each
    /// that is done leaves the backup. `None`: there was nothing to undo.
    pub async fn recover(
        backend: &Arc<dyn Backend>,
        store: &StateStore,
    ) -> Option<Result<(), String>> {
        let state = store.snapshot().await;
        let Some(value) = state.enhanced_backup else {
            // Nothing to undo. A run that died with the flag on and no undo
            // action (Windows, or Linux without `rp_filter` to loosen) left it
            // saying what is no longer true: this start has undone it all (the
            // phase 3 / M4a plan's execution record row 99).
            if state.features.enhanced_mode {
                store.update(|s| s.features.enhanced_mode = false).await;
            }
            return None;
        };
        let backup = match serde_json::from_value::<Backup>(value) {
            Ok(backup) => backup,
            Err(e) => {
                // not this version's shape: leave it as it is rather than
                // guess at undoing something (the next version may read it)
                tracing::error!(
                    error = %e,
                    "state.json's enhanced backup cannot be read; it is left as it is and nothing is undone"
                );
                return Some(Err(format!(
                    "state.json's enhanced backup cannot be read: {e}"
                )));
            }
        };
        let mut closing = Closing::default();
        for (index, undo) in backup.undo.iter().enumerate().rev() {
            let (backend, one) = (backend.clone(), undo.clone());
            match call(move || backend.undo(&one)).await {
                Ok(()) => closing.undone.push(index),
                Err(e) => {
                    tracing::debug!(
                        step = %undo.label(),
                        "the undo action that could not be undone"
                    );
                    closing.keep(&undo.kind(), e);
                }
            }
        }
        let remaining = closing.remaining(&backup.undo);
        let left = backup_value(&remaining);
        let all_back = remaining.is_empty();
        store
            .update(move |s| {
                s.enhanced_backup = left;
                // nothing of it is left out in the world any more
                if all_back {
                    s.features.enhanced_mode = false;
                }
            })
            .await;
        match closing.first {
            None => Some(Ok(())),
            Some(e) => Some(Err(e)),
        }
    }

    pub fn new(
        backend: Arc<dyn Backend>,
        store: Arc<StateStore>,
        starter: Arc<dyn Starter>,
        links: Links,
        ended: TunEnded,
    ) -> EnhancedMode {
        EnhancedMode {
            backend,
            store,
            starter,
            links,
            shared: Arc::new(Shared::new()),
            ended,
            opens: 0,
            on: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.on.is_some()
    }

    pub fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }

    /// Whether `sequence` is the number of the open that is on now. A failed
    /// TUN of an earlier open (or of one a close has happened since) has
    /// nothing left to turn off (the phase 3 / M4a plan's execution record row
    /// 63).
    pub fn is_current_open(&self, sequence: u64) -> bool {
        self.on.as_ref().is_some_and(|on| on.sequence == sequence)
    }

    /// Already on: `Ok` at once.
    pub async fn enable(&mut self, wanted: Wanted) -> Result<(), String> {
        if self.on.is_some() {
            return Ok(());
        }
        let saved = self.saved_undo().await?;
        let mut on = On::default();
        match self.open(&mut on, &wanted, &saved).await {
            Ok(()) => {
                let tun = on.tun.clone().expect("an open that succeeded has a TUN");
                self.shared.set_on(tun.clone());
                self.opens += 1;
                on.sequence = self.opens;
                self.watch(tun, on.sequence);
                self.on = Some(on);
                Ok(())
            }
            Err(e) => {
                // what the open did goes back the way a close undoes it: the
                // undo actions in the backup now, including the ones this open
                // added. An error there is in the log, and whatever did not go
                // back stays in state.json's backup for the next start.
                let (current, rewrite) = match self.saved_undo().await {
                    Ok(current) => (current, true),
                    Err(_) => (saved, false),
                };
                let _ = self.close(on, &current, rewrite).await;
                Err(e)
            }
        }
    }

    /// Watches the TUN of an open that has just succeeded: a device that fails
    /// ends the enhanced mode, and says so (P24). The wait is for `ending`
    /// alone — the TUN being told to end, not all of its tasks — (the phase 3 /
    /// M4a plan's execution record rows 62 and 100): whatever waits on `ended`
    /// is a close, which owns the TUN's place in the engine, and a dial that is
    /// under way holds the TUN's tasks without holding this. Stops the manager
    /// itself makes end as `Stopped`, which is no news.
    fn watch(&self, tun: Arc<dyn RunningTun>, sequence: u64) {
        let ended = self.ended.clone();
        tokio::spawn(async move {
            if let Ending::DeviceFailed(reason) = tun.ending().await {
                // the reason alone: the line the main loop says is the ERROR
                tracing::debug!(error = %reason, "the virtual adapter failed");
                ended(sequence);
            }
        });
    }

    /// Never stops halfway; the first error is what comes back.
    pub async fn disable(&mut self) -> Result<(), String> {
        let Some(on) = self.on.take() else {
            return Ok(());
        };
        let (saved, rewrite) = match self.saved_undo().await {
            Ok(saved) => (saved, true),
            Err(e) => {
                // what a close cannot read it does not guess at: the rest of
                // the close happens, and the backup is left as it is
                tracing::error!(
                    error = %e,
                    "state.json's enhanced backup cannot be read; what outlives the device is left as it is"
                );
                (Vec::new(), false)
            }
        };
        let result = self.close(on, &saved, rewrite).await;
        tracing::info!("enhanced mode off");
        result
    }

    /// What a reload or a change of network asks of an enhanced mode that is
    /// on (M4 design 9.7, 9.8): the IPv6 half on or off as the generation and
    /// the network say, the routes only by what differs, and after a change of
    /// network the DNS and the system's own adjustments once more.
    ///
    /// The order is the one an open and a close go in (design 9.3, 9.4): IPv6
    /// that comes on is the adapter's address, then its routes, then the DNS;
    /// IPv6 that goes is the reverse, the DNS handed back while the address it
    /// points at is still there, then the routes, then the address. The other
    /// order would be a long wait for every lookup: until the DNS is given back,
    /// each query the system sends to the address it no longer has waits out its
    /// timeout, and on a real system every step in between is a program that
    /// takes its time.
    ///
    /// A step that fails is logged, and the rest still goes on: the enhanced
    /// mode keeps what it has and stays on (M4 design 9.7). A set of routes
    /// that did not go in is no reason to go back to one that is out of date,
    /// and `GET` goes on saying `enabled: true`.
    ///
    /// The egress table is always the superset of what goes into the adapter
    /// (the phase 3 / M4a plan's execution record row 75): while the routes
    /// change it is the old set and the new one together, so that a connection
    /// of rurge's to a route that is already in the adapter does not come back
    /// into it (M4b's reader).
    ///
    /// A set of routes whose IPv6 half cannot go in is recorded as what the
    /// adapter still has of it, the old IPv6 routes less the ones removed (the
    /// backend takes back only the half it added); and an IPv6 that was off
    /// before this call goes on as if it were off, M4-D17's rule for a follow
    /// as for an open (the phase 3 / M4a plan's execution record row 95).
    pub async fn follow(&mut self, wanted: Wanted, network_changed: bool) {
        let Some(mut on) = self.on.take() else {
            return;
        };
        let Some(dev) = on.dev.clone() else {
            self.on = Some(on);
            return;
        };

        let was_ipv6 = on.ipv6;
        let mut ipv6 = wanted.ipv6;
        let turning_off = was_ipv6 && !ipv6;
        let turning_on = !was_ipv6 && ipv6;
        let primary = wanted.primary.as_deref();

        // IPv6 going: the DNS first, while the address it points at is still
        // there (design 9.4, 9.7). The address goes at the end, below.
        let dns_taken = turning_off;
        if dns_taken {
            self.take_the_dns_again(&dev, false, primary, network_changed)
                .await;
        }

        // IPv6 coming: the adapter's address first
        if turning_on {
            let backend = self.backend.clone();
            let with = dev.clone();
            match call(move || backend.configure_ipv6(&with)).await {
                Ok(true) => {}
                Ok(false) => {
                    // as at an open: this one goes on without it
                    tracing::warn!(
                        "IPv6 could not be configured; the enhanced mode goes on without it"
                    );
                    ipv6 = false;
                }
                Err(e) => {
                    // what the adapter has of IPv6 is unknown: count it off
                    self.failed("configure_ipv6", &e);
                    ipv6 = false;
                }
            }
        }

        // the routes: only what differs. The subnets are read once for this
        // follow, and every table it writes names the same ones.
        let subnets = self.physical_subnets(&dev.name).await;
        let mut next = wanted.routes.clone();
        if !ipv6 {
            next.v6.clear();
        }
        let old = on.routes.clone().unwrap_or_default();
        let (add, remove) = RouteSet::diff(&old, &next);
        // whether this follow wrote another table, whose sockets are not the
        // ones the resolver's upstreams were bound to
        let mut table_changed = false;
        if add.is_empty() && remove.is_empty() {
            // nothing to add or remove, and the table takes the new primary
            table_changed |= set_egress(&self.links.egress, Some(table(&next, &wanted, &subnets)));
            on.routes = Some(next);
        } else {
            table_changed |= set_egress(
                &self.links.egress,
                Some(table(&union(&old, &next), &wanted, &subnets)),
            );
            // What the adapter still has of the old IPv6 routes if the new
            // half cannot go in: the backend takes back only the half it
            // added, and removes what it was asked to, so what is left is the
            // old set less `remove` (the old and the new together). Taken
            // before `remove` goes to the backend (the phase 3 / M4a plan's
            // execution record row 95).
            let kept_v6: Vec<_> = old
                .v6
                .iter()
                .filter(|net| !remove.v6.contains(net))
                .copied()
                .collect();
            let backend = self.backend.clone();
            let with = dev.clone();
            match call(move || backend.routes(&with, &add, &remove)).await {
                Ok(added) => {
                    if !added {
                        // The IPv6 half could not be added, and the backend
                        // has warned. What is in the adapter is `kept_v6`,
                        // and an IPv6 that was off before this call goes on
                        // as if it were off (M4-D17; the phase 3 / M4a plan's
                        // execution record row 95).
                        next.v6 = kept_v6;
                        if !was_ipv6 {
                            ipv6 = false;
                        }
                    }
                    table_changed |=
                        set_egress(&self.links.egress, Some(table(&next, &wanted, &subnets)));
                    on.routes = Some(next);
                }
                Err(e) => {
                    // what went in is unknown: the set it keeps is the one it
                    // had, and the table keeps the union until the next call
                    self.failed("routes", &e);
                }
            }
        }

        // IPv6 coming, or another network: the DNS, once. IPv6 going has taken
        // it over already, and one follow takes it over at most once.
        let taking_the_dns = !dns_taken && (ipv6 != was_ipv6 || network_changed);
        if taking_the_dns {
            self.take_the_dns_again(&dev, ipv6, primary, network_changed)
                .await;
        } else if table_changed {
            // Another table went in and no takeover follows: the resolver's
            // upstreams are bound to the table there was, and a DNS server's
            // address that the new routes send into the adapter would take a
            // lookup with them. Telling it the system's DNS changed hands
            // makes it build them again on the new table (the phase 3 / M4b
            // plan's execution record row 69).
            (self.links.system_dns_changed)();
        }

        // IPv6 going: the address last, after its routes
        if turning_off {
            let backend = self.backend.clone();
            let with = dev.clone();
            if let Err(e) = call(move || backend.unconfigure_ipv6(&with)).await {
                self.failed("unconfigure_ipv6", &e);
            }
        }

        if network_changed {
            self.adjust_again(wanted.primary.as_deref()).await;
        }
        on.ipv6 = ipv6;
        self.on = Some(on);
    }

    /// The directly connected subnets of the physical interfaces, the virtual
    /// adapter `adapter` left out (M4 design 7.1, rule 3): what an open and a
    /// follow put in the egress table, most specific first. A read that fails
    /// is a WARN and an empty list — the table is written all the same, so
    /// that rule 3 does not apply while rules 4 and 5 do — and the adapter's
    /// name is not the log's.
    async fn physical_subnets(&self, adapter: &str) -> Vec<(IpNet, String)> {
        let backend = self.backend.clone();
        let exclude = vec![adapter.to_string()];
        match call(move || backend.subnets(&exclude)).await {
            Ok(subnets) => subnets,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "cannot read the subnets of the physical interfaces; rurge's own connections to them follow the primary interface"
                );
                Vec::new()
            }
        }
    }

    /// The interfaces the network task leaves out of what it reads: the
    /// virtual adapter while it is up, and nothing once it is gone.
    fn set_exclude(&self, interfaces: Vec<String>) {
        *self
            .links
            .exclude
            .write()
            .unwrap_or_else(|e| e.into_inner()) = interfaces;
    }

    /// One step of a follow that did not happen: the step and its reason are
    /// the log's, and the enhanced mode goes on with what it has (M4 design
    /// 9.7).
    fn failed(&self, step: &str, error: &str) {
        tracing::error!(
            step,
            error = %error,
            "cannot follow the change; the enhanced mode goes on with what it has"
        );
    }

    /// Takes the system's DNS over again, after a change of the IPv6 half or of
    /// the network (design 9.7, 9.8): what taking it over will need goes into
    /// the backup first (design 9.1), and the takeover follows it. A step that
    /// fails stays in the log. A change of network reads the servers outside
    /// the enhanced mode anew, once the takeover is done; a reload does not,
    /// even though the DNS may have been taken over again for IPv6's sake. The
    /// paths that return before `take_dns` read nothing either: the DNS has
    /// not changed hands, so the slot keeps what it had.
    async fn take_the_dns_again(
        &self,
        dev: &Opened,
        ipv6: bool,
        primary: Option<&str>,
        network_changed: bool,
    ) {
        let mut saved = match self.saved_undo().await {
            Ok(saved) => saved,
            Err(e) => {
                self.failed("dns_undo", &e);
                return;
            }
        };
        let backend = self.backend.clone();
        let with = dev.clone();
        let have = saved.clone();
        let save = match call(move || backend.dns_undo(&with, &have)).await {
            Ok(save) => save,
            Err(e) => {
                self.failed("dns_undo", &e);
                return;
            }
        };
        if !save.is_empty() {
            saved.extend_from_slice(&save);
            if let Err(e) = self.save_undo(&saved).await {
                // what the system would keep after a crash would be unknown
                self.failed("dns_backup", &e);
                return;
            }
        }
        let backend = self.backend.clone();
        let with = dev.clone();
        let taken = call(move || backend.take_dns(&with, ipv6)).await;
        // the servers outside are the new network's after a change of network,
        // and the same ones otherwise: read before the callback, so that it
        // already sees them, and whether or not the takeover went well (it may
        // have changed half of it) — the same reason the callback comes anyway
        if network_changed {
            self.read_physical(primary, &saved, true).await;
        }
        // a takeover changes hands whether or not it failed half way
        (self.links.system_dns_changed)();
        if let Err(e) = taken {
            self.failed("take_dns", &e);
        }
    }

    /// Reads the DNS servers of the network outside the enhanced mode and puts
    /// them where the system's DNS reads them while it is on (design 6.3; M4c
    /// plan P10): macOS reads its own backup or what DHCP gave, Linux resolved's
    /// upstream list. The read runs programs on macOS after a change of
    /// network, so it is a blocking call like the others. `None` — the system's
    /// own list — is the backend's to answer, and only where that is the right
    /// list (no takeover on Linux, the file backend without the key, `off`).
    /// A read that does not come back is the empty list, as the two systems'
    /// own answers are when they cannot read theirs: the lookup fails rather
    /// than fall back to the machine's list, which is the responder's while the
    /// DNS is taken over (coordinator's ruling R19).
    async fn read_physical(&self, primary: Option<&str>, saved: &[Undo], network_changed: bool) {
        let backend = self.backend.clone();
        let primary = primary.map(str::to_string);
        let saved = saved.to_vec();
        let read =
            call(move || Ok(backend.physical_dns(primary.as_deref(), &saved, network_changed)))
                .await;
        match read {
            Ok(servers) => self.set_physical(servers),
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "cannot read the DNS servers outside the enhanced mode; the lookup gives up \
                     rather than ask the system's own list"
                );
                self.set_physical(Some(Vec::new()));
            }
        }
    }

    /// Puts the servers outside the enhanced mode in their slot; `None` is the
    /// system's own list (M4 design 6.3).
    fn set_physical(&self, servers: Option<Vec<SocketAddr>>) {
        *self
            .links
            .physical
            .write()
            .unwrap_or_else(|e| e.into_inner()) = servers;
    }

    /// The system's own adjustments once more, after a change of network (M4
    /// design 9.8): the backup first, then the step.
    async fn adjust_again(&self, primary: Option<&str>) {
        let mut saved = match self.saved_undo().await {
            Ok(saved) => saved,
            Err(e) => {
                self.failed("adjust_undo", &e);
                return;
            }
        };
        let backend = self.backend.clone();
        let have = saved.clone();
        let one = primary.map(str::to_string);
        let save = match call(move || backend.adjust_undo(one.as_deref(), &have)).await {
            Ok(save) => save,
            Err(e) => {
                self.failed("adjust_undo", &e);
                return;
            }
        };
        if let Some(undo) = save {
            saved.push(undo);
            if let Err(e) = self.save_undo(&saved).await {
                self.failed("adjust_backup", &e);
                return;
            }
        }
        // the open's rule (the phase 3 / M4c plan's execution record row 40):
        // write it when this network asks for it or the backup has it already,
        // and leave a machine that needs no loosening alone
        if !saved.iter().any(|undo| undo.stage() == Stage::Adjust) {
            return;
        }
        let backend = self.backend.clone();
        if let Err(e) = call(move || backend.adjust()).await {
            self.failed("adjust", &e);
        }
    }

    /// The undo actions `state.json` holds, in the order the changes were made.
    async fn saved_undo(&self) -> Result<Vec<Undo>, String> {
        match self.store.snapshot().await.enhanced_backup {
            None => Ok(Vec::new()),
            Some(value) => serde_json::from_value::<Backup>(value)
                .map(|backup| backup.undo)
                .map_err(|e| format!("state.json's enhanced backup cannot be read: {e}")),
        }
    }

    /// Writes `state.json`'s backup as these undo actions, in the order the
    /// changes were made. A backup that cannot be written stops the step it is
    /// for: what the system would keep after a crash would be unknown (P11,
    /// design 9.2).
    async fn save_undo(&self, undo: &[Undo]) -> Result<(), String> {
        let Some(value) = backup_value(undo) else {
            return Ok(());
        };
        self.store
            .try_update(move |s| s.enhanced_backup = Some(value))
            .await
            .map_err(|e| format!("cannot write state.json: {e}"))?;
        Ok(())
    }

    /// The open (design 9.3): the device, the adapter, the TUN, the egress
    /// table, the routes, then the two things that outlive rurge, each of them
    /// after its undo action is in the backup.
    async fn open(&self, on: &mut On, wanted: &Wanted, saved: &[Undo]) -> Result<(), String> {
        // what `state.json`'s backup holds, as this open adds to it: the steps
        // that read it are told what is there already, so nothing is saved twice
        let mut undo = saved.to_vec();

        // 1. the device, and with it the name the network task leaves out of
        // what it reads (M4 design 3.7)
        let backend = self.backend.clone();
        let dev = call(move || backend.open()).await?;
        on.dev = Some(dev.clone());
        self.set_exclude(vec![dev.name.clone()]);

        // 2. the adapter, with its IPv6 half when it is wanted. It is always
        // asked: its address, MTU and link are wanted whether or not IPv6 is,
        // and the answer is only about the IPv6 half.
        let backend = self.backend.clone();
        let asked = wanted.ipv6;
        let with = dev.clone();
        let configured = call(move || backend.configure(&with, asked)).await?;
        let mut ipv6 = asked && configured;
        on.ipv6 = ipv6;
        if asked && !configured {
            // design 4.6 / M4-D17: IPv6 that cannot be had is no failure of
            // the open, but the user should know their network is IPv4 only
            tracing::warn!("IPv6 could not be configured; the enhanced mode goes on without it");
        }

        // 3. the TUN, so that packets are taken before traffic comes to it
        let tun = self.starter.start(dev.device.clone()).await?;
        on.tun = Some(tun);

        // 4. the egress table, before the routes: once they are in, rurge's
        // own connections must leave by the physical adapter
        let subnets = self.physical_subnets(&dev.name).await;
        set_egress(
            &self.links.egress,
            Some(table(&wanted.routes, wanted, &subnets)),
        );
        // the flag and the callback go with the table: from here on the
        // system's DNS may answer with fake addresses
        self.links.enhanced.store(true, Ordering::SeqCst);
        (self.links.system_dns_changed)();
        on.egress = true;

        // 5. the routes. A set that loses its IPv6 half takes the open on as
        // if IPv6 were off (M4-D17; the phase 3 / M4a plan's execution record
        // row 95): `ipv6` and `on.ipv6` go false before the DNS is taken over.
        // There is no old set here, so nothing is left in the adapter to record.
        let mut routes = wanted.routes.clone();
        if !ipv6 {
            routes.v6.clear();
        }
        let backend = self.backend.clone();
        let with = dev.clone();
        let add = routes.clone();
        let added = call(move || backend.routes(&with, &add, &RouteSet::default())).await?;
        if !added {
            // The IPv6 half could not be added, and the backend has warned.
            // No IPv6 was on before this open, so the open goes on as if it
            // were off (M4-D17): its routes, the DNS it takes over and what
            // `GET` and the log say of it (the phase 3 / M4a plan's execution
            // record row 95).
            routes.v6.clear();
            ipv6 = false;
            on.ipv6 = false;
        }
        on.routes = Some(routes.clone());

        // 6. what taking the DNS over will need, in the backup before it is done
        let backend = self.backend.clone();
        let with = dev.clone();
        let have = undo.clone();
        let saved_dns = call(move || backend.dns_undo(&with, &have)).await?;
        if !saved_dns.is_empty() {
            undo.extend_from_slice(&saved_dns);
            self.save_undo(&undo).await?;
        }

        // 7. the takeover: a takeover that fails may have changed half of it,
        // so it is undone as if it had — and the callback comes whether it
        // succeeded or not. The servers outside the enhanced mode are read for
        // `server:system` before that callback, so that it sees them too.
        on.dns = true;
        let backend = self.backend.clone();
        let with = dev.clone();
        let taken = call(move || backend.take_dns(&with, ipv6)).await;
        self.read_physical(wanted.primary.as_deref(), &undo, false)
            .await;
        (self.links.system_dns_changed)();
        taken?;

        // 8. the system's own adjustments, the same way. `rp_filter` is
        // loosened only where it has to be: `adjust_undo` says this network
        // needs it, or the backup holds a loosening already — written then so
        // that another program applying `sysctl.d` over it is undone by the
        // next change of network (the phase 3 / M4c plan's execution record
        // row 40).
        let backend = self.backend.clone();
        let have = undo.clone();
        let primary = wanted.primary.clone();
        let saved_adjust = call(move || backend.adjust_undo(primary.as_deref(), &have)).await?;
        if let Some(one) = saved_adjust {
            undo.push(one);
            self.save_undo(&undo).await?;
        }
        // the stage was reached whether or not it wrote anything: a close
        // undoes what the backup holds for it
        on.adjust = true;
        if undo.iter().any(|undo| undo.stage() == Stage::Adjust) {
            let backend = self.backend.clone();
            call(move || backend.adjust()).await?;
        }

        // 9. say that it is on (P16: the flag follows all of it, not the TUN)
        self.store.update(|s| s.features.enhanced_mode = true).await;
        tracing::info!(routes = routes.len(), ipv6, "enhanced mode on");
        tracing::debug!(adapter = %dev.name, "the enhanced mode's adapter");
        Ok(())
    }

    /// Undoes what `on` did, in the order a close goes in (P12). Each step that
    /// fails is logged and the rest still happens; the first error is what
    /// comes back. `rewrite` is `false` when the backup could not be read, and
    /// then it is left as it is.
    async fn close(&self, mut on: On, saved: &[Undo], rewrite: bool) -> Result<(), String> {
        let mut closing = Closing::default();
        let Some(dev) = on.dev.clone() else {
            // nothing was done
            self.shared.set_off();
            return Ok(());
        };
        // the DNS: what goes away with the device, then what outlives it
        if on.dns {
            let backend = self.backend.clone();
            let with = dev.clone();
            if let Err(e) = call(move || backend.give_dns_back(&with)).await {
                closing.keep("give_dns_back", e);
            }
            for (index, undo) in saved.iter().enumerate() {
                if undo.stage() == Stage::Dns {
                    self.undo(index, undo, &mut closing).await;
                }
            }
            // The DNS is handed back, whether or not each step went back: from
            // here on what the system answers may be its own again — and the
            // servers outside the enhanced mode are the system's own.
            self.set_physical(None);
            (self.links.system_dns_changed)();
        }
        // the routes: the traffic goes back to the physical adapter first
        if let Some(routes) = on.routes.take() {
            let backend = self.backend.clone();
            let with = dev.clone();
            if let Err(e) = call(move || backend.routes(&with, &RouteSet::default(), &routes)).await
            {
                closing.keep("routes", e);
            }
        }
        if on.egress {
            set_egress(&self.links.egress, None);
            // the table is gone, so the machine's DNS is its own again
            self.links.enhanced.store(false, Ordering::SeqCst);
            (self.links.system_dns_changed)();
        }
        // the TUN: stopped, and waited for, but not for ever
        if let Some(tun) = on.tun.take() {
            tun.stop();
            if timeout(JOIN_LIMIT, tun.ended()).await.is_err() {
                tracing::warn!("the TUN did not end within 3 seconds; going on");
            }
        }
        let backend = self.backend.clone();
        if let Err(e) = call(move || {
            backend.close(dev);
            Ok(())
        })
        .await
        {
            closing.keep("close", e);
        }
        // the system's own adjustments go back after the device is gone
        if on.adjust {
            for (index, undo) in saved.iter().enumerate() {
                if undo.stage() == Stage::Adjust {
                    self.undo(index, undo, &mut closing).await;
                }
            }
        }
        // the backup is what did not go back: the ones that failed and the ones
        // this close never reached. A close also undoes a failed open, and an
        // open that failed early never got to the stages that undo them (P11: a
        // write that fails only reaches the log; the next start undoes the same
        // ones again)
        if rewrite {
            let remaining = closing.remaining(saved);
            let all_back = remaining.is_empty();
            let left = backup_value(&remaining);
            self.store
                .update(move |s| {
                    s.enhanced_backup = left;
                    if all_back {
                        s.features.enhanced_mode = false;
                    }
                })
                .await;
        }
        // the network task reads the virtual adapter again. A close that never
        // got a device returned above and leaves the list as it is.
        self.set_exclude(Vec::new());
        self.shared.set_off();
        match closing.first {
            None => Ok(()),
            Some(e) => Err(e),
        }
    }

    /// One saved change put back; one that goes back is noted, and one that
    /// fails keeps its place in the backup.
    async fn undo(&self, index: usize, undo: &Undo, closing: &mut Closing) {
        let backend = self.backend.clone();
        let one = undo.clone();
        if let Err(e) = call(move || backend.undo(&one)).await {
            tracing::debug!(
                step = %undo.label(),
                "the undo action that could not be undone"
            );
            closing.keep(&undo.kind(), e);
        } else {
            closing.undone.push(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_platform::route::RouteInputs;
    use serde_json::{Value, json};
    use std::io;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    /// The steps taken, in order, as the backend, the starter and the fake
    /// TUN saw them.
    #[derive(Clone, Default)]
    struct Calls(Arc<Mutex<Vec<String>>>);

    impl Calls {
        fn push(&self, step: impl Into<String>) {
            self.0.lock().unwrap().push(step.into());
        }

        fn list(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    /// What the backup held when a step that has an undo action ran: the
    /// step's name, and the names of the undo actions `state.json` had then.
    type BackupsSeen = Arc<Mutex<Vec<(String, Vec<String>)>>>;

    /// A backend that answers as the file backend does and says what it was
    /// asked. `fail` names one action to refuse.
    #[derive(Default)]
    struct Mock {
        calls: Calls,
        fail: Arc<Mutex<Option<String>>>,
        fail_undo: Arc<Mutex<Option<String>>>,
        /// `state.json`, read at `take_dns` and `adjust`: what the backup held
        /// just before the step it is for.
        state: Arc<Mutex<Option<PathBuf>>>,
        backups_seen: BackupsSeen,
        egress: EgressSlot,
        egress_reported: Arc<Mutex<bool>>,
        /// Whether `routes` answers `Ok(false)` when it is given an IPv6 half:
        /// as a real backend, only then, and what went in has no IPv6 half.
        no_ipv6_half: Arc<Mutex<bool>>,
        /// The routes of each `routes` call, as (add, remove).
        routes: Arc<Mutex<Vec<(RouteSet, RouteSet)>>>,
        /// What `configure` was asked for, and what `take_dns` was given.
        configured: Arc<Mutex<Vec<bool>>>,
        dns: Arc<Mutex<Vec<bool>>>,
        closes: Arc<AtomicUsize>,
        /// The subnets `subnets` answers with.
        subnets: Arc<Mutex<Vec<(IpNet, String)>>>,
        /// The `exclude` of each `subnets` call, in order: a read, so not among
        /// `calls`.
        subnets_asked: Arc<Mutex<Vec<Vec<String>>>>,
        /// What `physical_dns` answers with, and the `network_changed` of each
        /// call, in order: a read, so not among `calls` (the order tests above
        /// stay as they were).
        physical: Arc<Mutex<Option<Vec<SocketAddr>>>>,
        physical_asked: Arc<Mutex<Vec<bool>>>,
        /// Whether `physical_dns` panics: a read that does not come back, as a
        /// backend whose task dies would leave it.
        physical_panics: Arc<AtomicBool>,
        /// Whether `adjust_undo` answers `None`: as Linux does when the
        /// interface's effective value is not 1, or when there is no interface
        /// to read yet (the phase 3 / M4c plan's execution record row 40).
        no_adjust: Arc<AtomicBool>,
    }

    impl Mock {
        fn fails(&self, action: &str) -> bool {
            self.fail.lock().unwrap().as_deref() == Some(action)
        }

        /// Runs one action: `said` is what it leaves in the calls, and `fail`
        /// naming `action` makes it fail instead.
        fn step_as(&self, action: &str, said: &str) -> Result<(), String> {
            if self.fails(action) {
                return Err(format!("{action} failed (test)"));
            }
            self.calls.push(said);
            Ok(())
        }

        fn step(&self, action: &str) -> Result<(), String> {
            self.step_as(action, action)
        }

        /// What `state.json`'s backup holds now, as the names of its undo
        /// actions: read from the file, which is what a crash would leave.
        fn backup_steps(&self) -> Vec<String> {
            let Some(path) = self.state.lock().unwrap().clone() else {
                return Vec::new();
            };
            let text = std::fs::read_to_string(path).unwrap_or_default();
            serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|state| {
                    serde_json::from_value::<Backup>(state["enhanced_backup"].clone()).ok()
                })
                .map(|backup| backup.undo.iter().map(Undo::label).collect())
                .unwrap_or_default()
        }

        fn note_backup(&self, step: &str) {
            let steps = self.backup_steps();
            self.backups_seen
                .lock()
                .unwrap()
                .push((step.to_string(), steps));
        }

        fn opened(&self) -> Opened {
            Opened {
                device: Arc::new(Empty),
                name: "rurge-test".to_string(),
                index: None,
            }
        }
    }

    impl Backend for Mock {
        fn open(&self) -> Result<Opened, String> {
            self.step("open")?;
            Ok(self.opened())
        }

        fn configure(&self, _dev: &Opened, ipv6: bool) -> Result<bool, String> {
            self.step("configure")?;
            self.configured.lock().unwrap().push(ipv6);
            Ok(ipv6 && !self.fails("configure_ipv6"))
        }

        fn configure_ipv6(&self, _dev: &Opened) -> Result<bool, String> {
            // as the file backend: no IPv6 to be had is no failure of the step
            if self.fails("configure_ipv6") {
                return Ok(false);
            }
            self.calls.push("configure_ipv6");
            Ok(true)
        }

        fn unconfigure_ipv6(&self, _dev: &Opened) -> Result<(), String> {
            self.step("unconfigure_ipv6")
        }

        fn routes(&self, _dev: &Opened, add: &RouteSet, remove: &RouteSet) -> Result<bool, String> {
            // the egress table is written before the routes that send traffic
            // into the adapter
            if !add.is_empty() && filled(&self.egress) {
                let mut reported = self.egress_reported.lock().unwrap();
                if !*reported {
                    *reported = true;
                    self.calls.push("egress");
                }
            }
            let said = if add.is_empty() {
                "routes off".to_string()
            } else {
                "routes".to_string()
            };
            self.step_as("routes", &said)?;
            self.routes
                .lock()
                .unwrap()
                .push((add.clone(), remove.clone()));
            if *self.no_ipv6_half.lock().unwrap() && !add.v6.is_empty() {
                // the IPv6 half is not in: the backend took it back and
                // removed `remove` all the same
                return Ok(false);
            }
            Ok(true)
        }

        fn subnets(&self, exclude: &[String]) -> Result<Vec<(IpNet, String)>, String> {
            // a read: what is asked is kept apart from what was done
            self.subnets_asked.lock().unwrap().push(exclude.to_vec());
            if self.fails("subnets") {
                return Err("subnets failed (test)".to_string());
            }
            Ok(self.subnets.lock().unwrap().clone())
        }

        fn dns_undo(&self, _dev: &Opened, saved: &[Undo]) -> Result<Vec<Undo>, String> {
            self.step("dns_undo")?;
            let take = Undo::Test {
                step: "take_dns".to_string(),
            };
            Ok(if saved.contains(&take) {
                Vec::new()
            } else {
                vec![take]
            })
        }

        fn take_dns(&self, _dev: &Opened, ipv6: bool) -> Result<bool, String> {
            self.note_backup("take_dns");
            self.step("take_dns")?;
            self.dns.lock().unwrap().push(ipv6);
            Ok(true)
        }

        fn give_dns_back(&self, _dev: &Opened) -> Result<(), String> {
            self.step("give_dns_back")
        }

        fn adjust_undo(
            &self,
            _primary: Option<&str>,
            saved: &[Undo],
        ) -> Result<Option<Undo>, String> {
            self.step("adjust_undo")?;
            if self.no_adjust.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let adjust = Undo::Test {
                step: "adjust".to_string(),
            };
            Ok(if saved.contains(&adjust) {
                None
            } else {
                Some(adjust)
            })
        }

        fn adjust(&self) -> Result<(), String> {
            self.note_backup("adjust");
            self.step("adjust")
        }

        fn undo(&self, undo: &Undo) -> Result<(), String> {
            let label = undo.label();
            if self.fail_undo.lock().unwrap().as_deref() == Some(label.as_str()) {
                return Err(format!("{label} undo failed (test)"));
            }
            self.calls.push(format!("undo {label}"));
            Ok(())
        }

        fn physical_dns(
            &self,
            _primary: Option<&str>,
            _saved: &[Undo],
            network_changed: bool,
        ) -> Option<Vec<SocketAddr>> {
            if self.physical_panics.load(Ordering::SeqCst) {
                panic!("physical_dns panicked (test)");
            }
            self.physical_asked.lock().unwrap().push(network_changed);
            self.physical.lock().unwrap().clone()
        }

        fn close(&self, _dev: Opened) {
            self.closes.fetch_add(1, Ordering::SeqCst);
            self.calls.push("close");
        }
    }

    /// A device of the mock's: it carries nothing.
    struct Empty;

    impl Device for Empty {
        fn recv<'a>(
            &'a self,
            _into: &'a mut Vec<Vec<u8>>,
            _max: usize,
        ) -> BoxFuture<'a, io::Result<()>> {
            Box::pin(std::future::pending())
        }

        fn send<'a>(&'a self, packets: &'a mut Vec<Vec<u8>>) -> BoxFuture<'a, io::Result<()>> {
            Box::pin(async move {
                packets.clear();
                Ok(())
            })
        }

        fn mtu(&self) -> usize {
            1500
        }
    }

    /// The TUN the fake starter hands back.
    struct MockTun {
        calls: Calls,
        stopped: Arc<AtomicBool>,
        /// `stop()` wakes `ending()`, which a TUN that does not fail does not
        /// answer before.
        stopped_notify: Arc<tokio::sync::Notify>,
        /// Whether `ended()` answers at once.
        ends: bool,
        /// Whether `ending()` answers `DeviceFailed` at once (P24).
        failing: bool,
    }

    impl RunningTun for MockTun {
        fn stop(&self) {
            self.stopped.store(true, Ordering::SeqCst);
            self.stopped_notify.notify_one();
            self.calls.push("stop");
        }

        fn ended(&self) -> BoxFuture<'_, Ending> {
            Box::pin(async move {
                if self.ends {
                    Ending::Stopped
                } else {
                    std::future::pending().await
                }
            })
        }

        fn ending(&self) -> BoxFuture<'_, Ending> {
            Box::pin(async move {
                if self.failing {
                    Ending::DeviceFailed("the device failed (test)".to_string())
                } else {
                    // a TUN that is not stopped yet has not ended either
                    self.stopped_notify.notified().await;
                    Ending::Stopped
                }
            })
        }

        fn counters(&self) -> rurge_engine::TunCounters {
            rurge_engine::TunCounters::default()
        }
    }

    /// A starter whose TUN ends at once, or never, and whose device fails when
    /// it is asked to.
    #[derive(Default)]
    struct MockStarter {
        calls: Calls,
        fail: Arc<Mutex<Option<String>>>,
        ends: bool,
        failing: bool,
        stopped: Arc<AtomicBool>,
    }

    impl Starter for MockStarter {
        fn start(
            &self,
            _device: Arc<dyn Device>,
        ) -> BoxFuture<'_, Result<Arc<dyn RunningTun>, String>> {
            let fails = self.fail.lock().unwrap().as_deref() == Some("start");
            Box::pin(async move {
                if fails {
                    return Err("start failed (test)".to_string());
                }
                self.calls.push("start");
                Ok(Arc::new(MockTun {
                    calls: self.calls.clone(),
                    stopped: self.stopped.clone(),
                    stopped_notify: Arc::new(tokio::sync::Notify::new()),
                    ends: self.ends,
                    failing: self.failing,
                }) as Arc<dyn RunningTun>)
            })
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        state: PathBuf,
        store: Arc<StateStore>,
        backend: Arc<Mock>,
        starter: Arc<MockStarter>,
        egress: EgressSlot,
        /// Whether the rest of rurge is told the enhanced mode is on.
        enhanced: Arc<AtomicBool>,
        /// The interfaces the network task leaves out of what it reads.
        exclude: Exclude,
        /// The servers outside the enhanced mode, as `PlatformSystemDns` reads them.
        physical: Arc<std::sync::RwLock<Option<Vec<SocketAddr>>>>,
        /// The open numbers the manager told the loop about (P24).
        told: tokio::sync::mpsc::UnboundedReceiver<u64>,
        mode: EnhancedMode,
    }

    /// A manager over a mock backend and a mock starter, `state.json` in a
    /// temporary directory. `ends` says whether the TUN's `ended()` answers.
    async fn fixture(ends: bool) -> Fixture {
        fixture_on(ends, false, Record::Nothing).await
    }

    /// `fixture`, and whether the TUN's device fails at once (P24).
    async fn fixture_with(ends: bool, failing: bool) -> Fixture {
        fixture_on(ends, failing, Record::Nothing).await
    }

    /// `fixture`, with `system_dns_changed` recorded among the calls the
    /// backend's steps are, so a test can read the order of the two.
    async fn fixture_recording_system_dns(ends: bool) -> Fixture {
        fixture_on(ends, false, Record::Dns).await
    }

    /// `fixture_recording_system_dns`, and what the physical slot held when
    /// the callback ran: a test can see that the servers are written before
    /// the callback that announces the new DNS.
    async fn fixture_recording_physical(ends: bool) -> Fixture {
        fixture_on(ends, false, Record::DnsAndPhysical).await
    }

    /// What the fixture's `system_dns_changed` leaves in the calls.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Record {
        /// Nothing: the callback only marks the DNS changing hands where the
        /// test asks for it.
        Nothing,
        /// `system_dns_changed on` / `off`, as the enhanced flag stands.
        Dns,
        /// `Dns`, and the servers in the physical slot as they are then.
        DnsAndPhysical,
    }

    /// `fixture_with`, and what `system_dns_changed` records.
    async fn fixture_on(ends: bool, failing: bool, record: Record) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state.json");
        let (store, _) = StateStore::open(state.clone()).await;
        let calls = Calls::default();
        let egress = egress_slot();
        let enhanced = Arc::new(AtomicBool::new(false));
        let exclude: Exclude = Exclude::default();
        let physical: Arc<std::sync::RwLock<Option<Vec<SocketAddr>>>> =
            Arc::new(std::sync::RwLock::new(None));
        let backend = Arc::new(Mock {
            calls: calls.clone(),
            egress: egress.clone(),
            state: Arc::new(Mutex::new(Some(state.clone()))),
            ..Mock::default()
        });
        let starter = Arc::new(MockStarter {
            calls: calls.clone(),
            ends,
            failing,
            ..MockStarter::default()
        });
        let (tell, told) = tokio::sync::mpsc::unbounded_channel();
        let system_dns_changed: Arc<dyn Fn() + Send + Sync> = match record {
            Record::Nothing => Arc::new(|| {}),
            Record::Dns | Record::DnsAndPhysical => {
                let calls = calls.clone();
                let enhanced = enhanced.clone();
                let physical = physical.clone();
                Arc::new(move || {
                    let on = if enhanced.load(Ordering::SeqCst) {
                        "on"
                    } else {
                        "off"
                    };
                    let mut said = format!("system_dns_changed {on}");
                    if record == Record::DnsAndPhysical {
                        let held = physical.read().unwrap_or_else(|e| e.into_inner()).clone();
                        said += &match held {
                            None => " physical=None".to_string(),
                            Some(servers) => format!(" physical={servers:?}"),
                        };
                    }
                    calls.push(said);
                })
            }
        };
        let mode = EnhancedMode::new(
            backend.clone(),
            store.clone(),
            starter.clone(),
            Links {
                egress: egress.clone(),
                enhanced: enhanced.clone(),
                exclude: exclude.clone(),
                system_dns_changed,
                physical: physical.clone(),
            },
            Arc::new(move |sequence| {
                let _ = tell.send(sequence);
            }),
        );
        Fixture {
            _dir: dir,
            state,
            store,
            backend,
            starter,
            egress,
            enhanced,
            exclude,
            physical,
            told,
            mode,
        }
    }

    /// A wanted state with both families in it.
    fn wanted() -> Wanted {
        Wanted {
            routes: RouteSet::compute(&RouteInputs {
                ipv6: true,
                excluded: &[],
                included: &[],
                external: &[],
                explicit_space: false,
            }),
            ipv6: true,
            primary: Some("eth0".to_string()),
        }
    }

    /// The same for a profile that does not want IPv6.
    fn without_ipv6() -> Wanted {
        Wanted {
            routes: RouteSet::compute(&RouteInputs {
                ipv6: false,
                excluded: &[],
                included: &[],
                external: &[],
                explicit_space: false,
            }),
            ipv6: false,
            primary: Some("eth0".to_string()),
        }
    }

    fn lines(what: &[&str]) -> Vec<String> {
        what.iter().map(|line| line.to_string()).collect()
    }

    /// A device that fails ends the enhanced mode: the manager tells the loop
    /// which open it belonged to (P24; the phase 3 / M4a plan's execution
    /// record row 63).
    #[tokio::test]
    async fn a_device_that_fails_tells_the_loop_which_open_it_belongs_to() {
        let mut f = fixture_with(true, true).await;
        f.mode.enable(wanted()).await.unwrap();
        let sequence = tokio::time::timeout(Duration::from_secs(1), f.told.recv())
            .await
            .expect("the failure is told")
            .expect("the channel is still open");
        assert_eq!(sequence, 1, "the first open is number one");
        assert!(f.mode.is_current_open(1));
        f.mode.disable().await.unwrap();
        assert!(!f.mode.is_current_open(1), "a close ends that open");
        assert!(f.told.try_recv().is_err(), "the one failure was told once");
    }

    /// A TUN a close stopped is no news: the watch waits for the TUN to end,
    /// the stop ends it as `Stopped`, and the manager tells the loop nothing
    /// (P24).
    #[tokio::test]
    async fn a_tun_a_close_stopped_tells_the_loop_nothing() {
        let mut f = fixture(true).await;
        f.mode.enable(wanted()).await.unwrap();
        f.mode.disable().await.unwrap();
        assert!(
            f.starter.stopped.load(Ordering::SeqCst),
            "the close stopped the TUN"
        );
        // waiting for what must not happen: the watch task has had time to run
        // after the stop, and it has nothing to wait on once `ending()` has
        // answered `Stopped` (200 ms against a task that only needs to be
        // scheduled once)
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            f.told.try_recv().is_err(),
            "a TUN a close stopped is no news"
        );
    }

    /// The egress table names the physical interfaces' subnets, read once for
    /// the open with the virtual adapter itself left out (M4 design 7.1, rule
    /// 3), and a close takes the table away with it.
    #[tokio::test]
    async fn opening_writes_the_egress_table_with_the_subnets_and_closing_clears_it() {
        let mut f = fixture(true).await;
        let lan: Vec<(IpNet, String)> = vec![
            ("192.168.1.0/24".parse().unwrap(), "eth0".to_string()),
            ("10.0.0.0/8".parse().unwrap(), "eth0".to_string()),
        ];
        *f.backend.subnets.lock().unwrap() = lan.clone();

        f.mode.enable(wanted()).await.unwrap();
        let table = f
            .egress
            .read()
            .unwrap()
            .clone()
            .expect("the egress table is written");
        assert_eq!(table.subnets, lan, "the subnets are the ones read");
        assert_eq!(
            f.backend.subnets_asked.lock().unwrap().clone(),
            vec![lines(&["rurge-test"])],
            "read once, with the adapter's own name left out"
        );

        f.mode.disable().await.unwrap();
        assert!(
            f.egress.read().unwrap().is_none(),
            "a close clears the table"
        );
    }

    /// Subnets that cannot be read are a WARN and nothing else: the table is
    /// written without them — rule 3 does not apply, rules 4 and 5 do — and
    /// the enhanced mode opens all the same.
    #[tokio::test]
    async fn subnets_that_cannot_be_read_leave_rule_three_out_and_open_all_the_same() {
        use rurge_net::testing::Lines;

        let lines = Lines::default();
        // held across the awaits: the manager runs on this thread
        let _capturing = lines.capture();
        let mut f = fixture(true).await;
        *f.backend.fail.lock().unwrap() = Some("subnets".to_string());

        f.mode.enable(wanted()).await.unwrap();
        assert!(f.mode.enabled(), "the open goes on");
        let table = f
            .egress
            .read()
            .unwrap()
            .clone()
            .expect("the egress table is written");
        assert!(table.subnets.is_empty(), "rule 3 is left out");
        assert_eq!(table.routes, wanted().routes, "rules 4 and 5 go on");
        assert_eq!(table.primary.as_deref(), Some("eth0"));
        let said = lines.said("cannot read the subnets of the physical interfaces");
        assert_eq!(said.len(), 1, "one WARN: {said:?}");
        assert!(said[0].contains("error="), "{}", said[0]);
        assert!(
            !said[0].contains("rurge-test"),
            "the adapter's name is not the log's: {}",
            said[0]
        );
    }

    /// The enhanced-mode flag is up from the moment the egress table is in its
    /// slot until the close takes it away: what `PlatformSystemDns` reads.
    #[tokio::test]
    async fn the_enhanced_flag_is_up_from_the_egress_table_until_it_is_cleared() {
        let mut f = fixture(true).await;
        assert!(!f.enhanced.load(Ordering::SeqCst), "off at the start");
        f.mode.enable(wanted()).await.unwrap();
        assert!(f.enhanced.load(Ordering::SeqCst), "up with the table");
        f.mode.disable().await.unwrap();
        assert!(!f.enhanced.load(Ordering::SeqCst), "down with the table");
    }

    /// The network task reads the machine without the virtual adapter while the
    /// enhanced mode is on: the manager puts the adapter's name in the list
    /// once the device is opened, and a close empties it again — an open that
    /// failed, undone by the same close, leaves it empty as well.
    #[tokio::test]
    async fn the_virtual_adapter_is_left_out_of_the_network_reads_while_open() {
        let mut f = fixture(true).await;
        assert!(f.exclude.read().unwrap().is_empty(), "nothing is left out");
        f.mode.enable(wanted()).await.unwrap();
        assert_eq!(
            *f.exclude.read().unwrap(),
            ["rurge-test".to_string()],
            "the adapter the device opened"
        );
        f.mode.disable().await.unwrap();
        assert!(f.exclude.read().unwrap().is_empty(), "a close empties it");

        // an open that fails goes back through the same close
        let mut f = fixture(true).await;
        *f.backend.fail.lock().unwrap() = Some("routes".to_string());
        let error = f.mode.enable(wanted()).await.unwrap_err();
        assert_eq!(error, "routes failed (test)");
        assert!(
            f.exclude.read().unwrap().is_empty(),
            "an open that failed leaves the list as it was"
        );
    }

    /// The system's DNS changed hands: the callback comes after the flag goes
    /// up, after the takeover, after the giving back and after the flag goes
    /// down — once for each, in that order (M4 design 6.3, 7.1).
    #[tokio::test]
    async fn system_dns_changed_is_called_after_the_flag_the_takeover_and_the_giving_back() {
        let mut f = fixture_recording_system_dns(true).await;
        f.mode.enable(wanted()).await.unwrap();
        f.mode.disable().await.unwrap();
        assert_eq!(
            f.backend.calls.list(),
            lines(&[
                "open",
                "configure",
                "start",
                // the egress table is in its slot
                "system_dns_changed on",
                "egress",
                "routes",
                "dns_undo",
                "take_dns",
                // the takeover, whatever it did
                "system_dns_changed on",
                "adjust_undo",
                "adjust",
                // the close: the DNS handed back, then the table taken away
                "give_dns_back",
                "undo take_dns",
                "system_dns_changed on",
                "routes off",
                "system_dns_changed off",
                "stop",
                "close",
                "undo adjust",
            ])
        );
    }

    /// The servers outside the enhanced mode are in their slot once the DNS is
    /// taken over, before the callback that announces the new DNS, and gone
    /// once it is given back, again before the callback. The number of
    /// callbacks is M4b's four: writing the servers adds none of its own (M4c
    /// plan's execution record row 30).
    #[tokio::test]
    async fn the_physical_servers_are_set_after_the_takeover_and_cleared_on_close() {
        let mut f = fixture_recording_physical(true).await;
        let servers: Vec<SocketAddr> = vec!["192.0.2.53:53".parse().unwrap()];
        *f.backend.physical.lock().unwrap() = Some(servers.clone());

        f.mode.enable(wanted()).await.unwrap();
        let calls = f.backend.calls.list();
        let takeover = calls
            .iter()
            .position(|call| call == "take_dns")
            .expect("the DNS is taken over");
        assert_eq!(
            calls[takeover + 1],
            format!("system_dns_changed on physical={servers:?}"),
            "the callback after the takeover already sees the servers: {calls:?}"
        );
        assert_eq!(
            f.physical.read().unwrap().clone(),
            Some(servers),
            "the slot holds them while the enhanced mode is on"
        );

        f.mode.disable().await.unwrap();
        let calls = f.backend.calls.list();
        let given_back = calls
            .iter()
            .position(|call| call == "undo take_dns")
            .expect("the takeover is undone");
        assert_eq!(
            calls[given_back + 1],
            "system_dns_changed on physical=None",
            "the callback after the DNS is given back sees none: {calls:?}"
        );
        assert_eq!(f.physical.read().unwrap().clone(), None);
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with("system_dns_changed"))
                .count(),
            4,
            "the four callbacks M4b pinned, none added: {calls:?}"
        );
    }

    /// A change of network reads the servers outside the enhanced mode again,
    /// after the takeover it takes: the other network has others.
    #[tokio::test]
    async fn a_change_of_network_reads_the_physical_servers_again() {
        let mut f = fixture(true).await;
        let first: Vec<SocketAddr> = vec!["192.0.2.53:53".parse().unwrap()];
        *f.backend.physical.lock().unwrap() = Some(first.clone());
        f.mode.enable(wanted()).await.unwrap();
        assert_eq!(f.physical.read().unwrap().clone(), Some(first));

        let second: Vec<SocketAddr> = vec!["198.51.100.53:53".parse().unwrap()];
        *f.backend.physical.lock().unwrap() = Some(second.clone());
        f.mode.follow(wanted(), true).await;

        assert_eq!(
            f.physical.read().unwrap().clone(),
            Some(second),
            "the slot holds the new network's servers"
        );
        assert_eq!(
            *f.backend.physical_asked.lock().unwrap(),
            [false, true],
            "the open read them, and the change of network read them anew"
        );
    }

    /// A reload reads nothing, even when the DNS is taken over again for IPv6's
    /// sake: the network has not changed, and its servers are the same ones.
    #[tokio::test]
    async fn a_reload_does_not_read_them() {
        let mut f = fixture(true).await;
        let written: Vec<SocketAddr> = vec!["192.0.2.53:53".parse().unwrap()];
        *f.backend.physical.lock().unwrap() = Some(written.clone());
        f.mode.enable(wanted()).await.unwrap();
        // what the network would answer now, if it were asked
        *f.backend.physical.lock().unwrap() = Some(vec!["198.51.100.53:53".parse().unwrap()]);

        // IPv6 off: the DNS is taken over again, without a change of network
        f.mode.follow(without_ipv6(), false).await;

        assert_eq!(
            *f.backend.dns.lock().unwrap(),
            [true, false],
            "the DNS was taken over again"
        );
        assert_eq!(
            *f.backend.physical_asked.lock().unwrap(),
            [false],
            "only the open read them"
        );
        assert_eq!(
            f.physical.read().unwrap().clone(),
            Some(written),
            "the slot is left as it was"
        );
    }

    /// A read that does not come back is no servers, not the machine's own list
    /// (coordinator's ruling R19; M4 design 6.3): `None` in the slot means "the
    /// system's own", and during a takeover that list is the responder's —
    /// Linux's resolved stub would send `server:system`'s queries back to the
    /// responder. The backend's read panics here, `spawn_blocking` catches it
    /// and `call` turns it into an error; the panic's message on standard error
    /// is what this test expects, and the test itself does not panic.
    #[tokio::test]
    async fn a_read_that_does_not_come_back_is_no_servers() {
        let mut f = fixture(true).await;
        f.backend.physical_panics.store(true, Ordering::SeqCst);

        f.mode.enable(wanted()).await.unwrap();

        assert_eq!(
            f.physical.read().unwrap().clone(),
            Some(Vec::new()),
            "the lookup fails rather than ask the system's own list"
        );
    }

    #[tokio::test]
    async fn opening_goes_in_the_designed_order() {
        let mut f = fixture(true).await;
        assert!(!f.mode.enabled());
        f.mode.enable(wanted()).await.unwrap();
        assert_eq!(
            f.backend.calls.list(),
            lines(&[
                "open",
                "configure",
                "start",
                "egress",
                "routes",
                "dns_undo",
                "take_dns",
                "adjust_undo",
                "adjust",
            ])
        );
        assert!(f.mode.enabled());
        // the egress table is what was wanted, and the routes are what was added
        let table = f
            .egress
            .read()
            .unwrap()
            .clone()
            .expect("the egress table is written");
        assert_eq!(table.routes, wanted().routes);
        assert_eq!(table.primary.as_deref(), Some("eth0"));
        let (add, remove) = f.backend.routes.lock().unwrap()[0].clone();
        assert_eq!(add, wanted().routes);
        assert!(remove.is_empty());
        assert!(
            f.backend.configured.lock().unwrap()[0],
            "IPv6 was asked for"
        );
        assert!(f.backend.dns.lock().unwrap()[0], "with its IPv6 half");
        // what state.json holds, and that the mode says it is on
        let state = f.store.snapshot().await;
        assert!(state.features.enhanced_mode);
        assert_eq!(
            state.enhanced_backup,
            Some(json!({"undo": [
                {"kind": "test", "step": "take_dns"},
                {"kind": "test", "step": "adjust"},
            ]}))
        );
        let status = f.mode.shared().status();
        assert!(status.enabled);
        assert_eq!(status.counters, rurge_engine::TunCounters::default());
        // a second enable does nothing
        f.mode.enable(wanted()).await.unwrap();
        assert_eq!(f.backend.calls.list().len(), 9);
        f.mode.disable().await.unwrap();
        assert!(!f.mode.enabled());
        assert_eq!(f.store.snapshot().await.enhanced_backup, None);
        assert_eq!(
            f.backend.calls.list().last().unwrap(),
            "undo adjust",
            "a close undoes the adjustments after the device is gone"
        );
    }

    #[tokio::test]
    async fn the_backup_is_saved_before_the_step_it_undoes() {
        let mut f = fixture(true).await;
        f.mode.enable(wanted()).await.unwrap();
        let seen = f.backend.backups_seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![
                ("take_dns".to_string(), lines(&["take_dns"])),
                ("adjust".to_string(), lines(&["take_dns", "adjust"])),
            ],
            "each step finds its undo action in state.json already"
        );
    }

    /// No loosening is asked for (the interface's effective value is not 1):
    /// an open that finds a backup without one leaves `net.ipv4.conf.all`
    /// alone, and keeps no undo action for it (the phase 3 / M4c plan's
    /// execution record row 40).
    #[tokio::test]
    async fn a_machine_that_needs_no_loosening_is_left_alone() {
        let mut f = fixture(true).await;
        f.backend.no_adjust.store(true, Ordering::SeqCst);

        f.mode.enable(wanted()).await.unwrap();

        let calls = f.backend.calls.list();
        assert!(
            calls.contains(&"adjust_undo".to_string()),
            "it is asked whether one is needed: {calls:?}"
        );
        assert!(
            !calls.contains(&"adjust".to_string()),
            "and none is written: {calls:?}"
        );
        assert_eq!(
            f.store.snapshot().await.enhanced_backup,
            Some(json!({"undo": [{"kind": "test", "step": "take_dns"}]})),
            "the backup holds the takeover alone"
        );
    }

    /// The backup says the machine was loosened once: writing it again is
    /// idempotent, and it is what puts it back after another program applied
    /// `sysctl.d` over it — so it is written even when `adjust_undo` asks for
    /// none (the phase 3 / M4c plan's execution record row 40).
    #[tokio::test]
    async fn a_loosening_in_the_backup_is_written_again_when_none_is_asked_for() {
        let mut f = fixture(true).await;
        f.backend.no_adjust.store(true, Ordering::SeqCst);
        f.store
            .update(|s| {
                s.enhanced_backup = Some(json!({"undo": [{"kind": "test", "step": "adjust"}]}));
            })
            .await;

        f.mode.enable(wanted()).await.unwrap();

        let calls = f.backend.calls.list();
        assert!(
            calls.contains(&"adjust".to_string()),
            "what the backup holds is written again: {calls:?}"
        );
    }

    /// A change of network goes the same way as the open: nothing asked for
    /// and nothing in the backup, nothing is written (the phase 3 / M4c
    /// plan's execution record row 40).
    #[tokio::test]
    async fn a_change_of_network_does_not_loosen_what_need_not_be() {
        let mut f = fixture(true).await;
        f.backend.no_adjust.store(true, Ordering::SeqCst);
        f.mode.enable(wanted()).await.unwrap();

        f.mode.follow(wanted(), true).await;

        let calls = f.backend.calls.list();
        assert_eq!(
            calls.iter().filter(|call| *call == "adjust_undo").count(),
            2,
            "the open and the change of network both asked: {calls:?}"
        );
        assert!(
            !calls.contains(&"adjust".to_string()),
            "neither wrote: {calls:?}"
        );
    }

    #[tokio::test]
    async fn a_backup_that_cannot_be_written_stops_the_step_before_it_is_done() {
        let mut f = fixture(true).await;
        // the temporary file's name taken by a directory: no write can go there
        std::fs::create_dir(f.state.with_extension("json.tmp")).unwrap();
        let error = f.mode.enable(wanted()).await.unwrap_err();
        assert!(error.starts_with("cannot write state.json"), "{error}");
        let calls = f.backend.calls.list();
        assert!(
            !calls.contains(&"take_dns".to_string()),
            "the takeover never happened: {calls:?}"
        );
        for step in ["routes off", "stop", "close"] {
            assert!(calls.contains(&step.to_string()), "{step}: {calls:?}");
        }
        assert_eq!(f.backend.closes.load(Ordering::SeqCst), 1);
        assert!(!filled(&f.egress), "the egress table goes back");
        assert!(!f.mode.enabled());
        assert_eq!(f.store.snapshot().await.enhanced_backup, None);
    }

    /// Every action that can fail: what the open did goes back, and the error
    /// is the one that step gave.
    #[tokio::test]
    async fn a_step_that_fails_undoes_the_steps_before_it_in_reverse() {
        let cases: [(&str, Vec<&str>); 8] = [
            ("open", vec![]),
            ("configure", vec!["open", "close"]),
            ("start", vec!["open", "configure", "close"]),
            (
                "routes",
                vec!["open", "configure", "start", "egress", "stop", "close"],
            ),
            (
                "dns_undo",
                vec![
                    "open",
                    "configure",
                    "start",
                    "egress",
                    "routes",
                    "routes off",
                    "stop",
                    "close",
                ],
            ),
            (
                "take_dns",
                vec![
                    "open",
                    "configure",
                    "start",
                    "egress",
                    "routes",
                    "dns_undo",
                    "give_dns_back",
                    "undo take_dns",
                    "routes off",
                    "stop",
                    "close",
                ],
            ),
            (
                "adjust_undo",
                vec![
                    "open",
                    "configure",
                    "start",
                    "egress",
                    "routes",
                    "dns_undo",
                    "take_dns",
                    "give_dns_back",
                    "undo take_dns",
                    "routes off",
                    "stop",
                    "close",
                ],
            ),
            (
                "adjust",
                vec![
                    "open",
                    "configure",
                    "start",
                    "egress",
                    "routes",
                    "dns_undo",
                    "take_dns",
                    "adjust_undo",
                    "give_dns_back",
                    "undo take_dns",
                    "routes off",
                    "stop",
                    "close",
                    "undo adjust",
                ],
            ),
        ];
        for (fails, expected) in cases {
            let mut f = fixture(true).await;
            if fails == "start" {
                *f.starter.fail.lock().unwrap() = Some(fails.to_string());
            } else {
                *f.backend.fail.lock().unwrap() = Some(fails.to_string());
            }
            let error = f.mode.enable(wanted()).await.unwrap_err();
            assert_eq!(error, format!("{fails} failed (test)"), "{fails}");
            assert_eq!(f.backend.calls.list(), lines(&expected), "{fails}");
            assert!(!f.mode.enabled(), "{fails}");
            assert!(!filled(&f.egress), "{fails}");
            assert_eq!(
                f.store.snapshot().await.enhanced_backup,
                None,
                "what went back leaves the backup: {fails}"
            );
            assert!(!f.store.snapshot().await.features.enhanced_mode, "{fails}");
        }
    }

    #[tokio::test]
    async fn closing_goes_in_the_designed_order_and_never_stops_halfway() {
        let mut f = fixture(true).await;
        f.mode.enable(wanted()).await.unwrap();
        *f.backend.fail.lock().unwrap() = Some("give_dns_back".to_string());
        *f.backend.fail_undo.lock().unwrap() = Some("adjust".to_string());
        let error = f.mode.disable().await.unwrap_err();
        assert_eq!(error, "give_dns_back failed (test)");
        let after_open = 9;
        assert_eq!(
            f.backend.calls.list()[after_open..],
            lines(&[
                "undo take_dns",
                "routes off",
                "stop",
                "close",
                // `undo adjust` failed, so it is not among the calls
            ])
        );
        assert_eq!(f.backend.closes.load(Ordering::SeqCst), 1);
        assert!(f.backend.calls.list().contains(&"routes off".to_string()));
        assert!(!filled(&f.egress));
        assert!(!f.mode.enabled());
        // what did not go back stays, and the feature says not everything is
        // undone yet
        let state = f.store.snapshot().await;
        assert_eq!(
            state.enhanced_backup,
            Some(json!({"undo": [{"kind": "test", "step": "adjust"}]})),
            "the undo that failed stays for the next start"
        );
        assert!(state.features.enhanced_mode);
        assert!(!f.mode.shared().status().enabled);
        assert_eq!(
            f.mode.shared().status().counters,
            rurge_engine::TunCounters::default()
        );
        // the next close does the one that is left
        *f.backend.fail.lock().unwrap() = None;
        *f.backend.fail_undo.lock().unwrap() = None;
        f.mode.enable(wanted()).await.unwrap();
        f.mode.disable().await.unwrap();
        assert_eq!(f.store.snapshot().await.enhanced_backup, None);
        assert!(!f.store.snapshot().await.features.enhanced_mode);
    }

    #[tokio::test(start_paused = true)]
    async fn closing_waits_for_the_tun_three_seconds_at_most() {
        let mut f = fixture(false).await;
        f.mode.enable(wanted()).await.unwrap();
        // a TUN whose `ended()` never answers: the clock is paused, and the
        // close goes on when its three seconds are up
        let started = tokio::time::Instant::now();
        f.mode.disable().await.unwrap();
        assert_eq!(
            started.elapsed(),
            JOIN_LIMIT,
            "the close waited exactly its limit"
        );
        assert_eq!(f.backend.closes.load(Ordering::SeqCst), 1);
        assert_eq!(f.backend.calls.list().last().unwrap(), "undo adjust");
    }

    #[tokio::test]
    async fn ipv6_that_cannot_be_had_is_left_off_for_this_open() {
        let mut f = fixture(true).await;
        *f.backend.fail.lock().unwrap() = Some("configure_ipv6".to_string());
        f.mode.enable(wanted()).await.unwrap();
        assert!(
            f.backend.configured.lock().unwrap()[0],
            "IPv6 was asked for"
        );
        let (add, _) = f.backend.routes.lock().unwrap()[0].clone();
        assert!(add.v6.is_empty(), "the routes have no IPv6 half");
        assert!(!add.v4.is_empty());
        assert!(!f.backend.dns.lock().unwrap()[0], "nor does the DNS");
        assert!(f.mode.enabled());
    }

    /// IPv6 off (`ipv6-vif = disabled`, say): the adapter is configured all the
    /// same — its address, MTU and link are nobody's IPv6 — and nothing of
    /// this open is IPv6's.
    #[tokio::test]
    async fn an_open_without_ipv6_still_configures_the_adapter() {
        let mut f = fixture(true).await;
        f.mode.enable(without_ipv6()).await.unwrap();
        assert_eq!(
            f.backend.calls.list(),
            lines(&[
                "open",
                "configure",
                "start",
                "egress",
                "routes",
                "dns_undo",
                "take_dns",
                "adjust_undo",
                "adjust",
            ]),
            "the adapter is configured whether or not IPv6 is"
        );
        assert_eq!(*f.backend.configured.lock().unwrap(), [false]);
        let (add, remove) = f.backend.routes.lock().unwrap()[0].clone();
        assert!(add.v6.is_empty(), "the routes have no IPv6 half");
        assert!(!add.v4.is_empty());
        assert!(remove.is_empty());
        assert_eq!(*f.backend.dns.lock().unwrap(), [false]);
        assert!(f.mode.enabled());
    }

    /// An open that fails must leave the undo actions it never reached: those a
    /// close of an earlier run could not undo are the machine's own settings,
    /// and only `state.json` remembers them.
    #[tokio::test]
    async fn an_open_that_fails_leaves_the_undo_actions_it_never_reached() {
        let rp_filter = json!({"undo": [{"kind": "rp_filter", "value": 1}]});
        let mac_dns = json!({"undo": [{"kind": "mac_dns", "service": "Wi-Fi", "servers": []}]});
        for left in [rp_filter, mac_dns] {
            let mut f = fixture(true).await;
            let backup = left.clone();
            f.store
                .update(move |s| {
                    s.enhanced_backup = Some(backup);
                    // a close that could not undo it says so here
                    s.features.enhanced_mode = true;
                })
                .await;
            *f.backend.fail.lock().unwrap() = Some("routes".to_string());
            let error = f.mode.enable(wanted()).await.unwrap_err();
            assert_eq!(error, "routes failed (test)", "{left}");
            let state = f.store.snapshot().await;
            assert_eq!(
                state.enhanced_backup,
                Some(left.clone()),
                "the open never reached the stage that undoes it: {left}"
            );
            assert!(state.features.enhanced_mode, "{left}");
            assert!(
                !f.backend
                    .calls
                    .list()
                    .iter()
                    .any(|step| step.starts_with("undo ")),
                "{left}: {:?}",
                f.backend.calls.list()
            );
            assert!(!f.mode.enabled(), "{left}");
        }
    }

    /// A backup an open cannot read stops it before it asks the system
    /// anything: appending to a backup this version does not understand would
    /// lose it.
    #[tokio::test]
    async fn an_unreadable_backup_stops_an_open_before_it_starts() {
        let mut f = fixture(true).await;
        let backup = json!({"x": 1});
        let left = backup.clone();
        f.store
            .update(move |s| s.enhanced_backup = Some(left))
            .await;
        let error = f.mode.enable(wanted()).await.unwrap_err();
        assert!(
            error.starts_with("state.json's enhanced backup cannot be read"),
            "{error}"
        );
        assert!(
            f.backend.calls.list().is_empty(),
            "nothing was asked of the system: {:?}",
            f.backend.calls.list()
        );
        assert_eq!(f.store.snapshot().await.enhanced_backup, Some(backup));
        assert!(!f.mode.enabled());
    }

    #[tokio::test]
    async fn an_unreadable_backup_is_left_alone() {
        let f = fixture(true).await;
        let backup = json!({"x": 1});
        let written = backup.clone();
        f.store
            .update(move |s| s.enhanced_backup = Some(written))
            .await;
        let error = EnhancedMode::recover(&(f.backend.clone() as Arc<dyn Backend>), &f.store)
            .await
            .expect("there is something in the backup")
            .unwrap_err();
        assert!(
            error.starts_with("state.json's enhanced backup cannot be read"),
            "{error}"
        );
        assert_eq!(
            f.store.snapshot().await.enhanced_backup,
            Some(backup),
            "the backup is left as it is"
        );
        assert!(
            f.backend.calls.list().is_empty(),
            "nothing is guessed at: {:?}",
            f.backend.calls.list()
        );
    }

    /// A backup a close cannot read is not guessed at: everything else of the
    /// close still happens, and the backup is left as it is (it may be a later
    /// version's).
    #[tokio::test]
    async fn a_backup_a_close_cannot_read_is_left_alone() {
        let mut f = fixture(true).await;
        f.mode.enable(wanted()).await.unwrap();
        let foreign = json!({"x": 1});
        let left = foreign.clone();
        f.store
            .update(move |s| s.enhanced_backup = Some(left))
            .await;
        f.mode.disable().await.unwrap();
        let calls = f.backend.calls.list();
        for step in ["routes off", "stop", "close"] {
            assert!(calls.contains(&step.to_string()), "{step}: {calls:?}");
        }
        assert!(
            !calls.iter().any(|step| step.starts_with("undo ")),
            "nothing is guessed at: {calls:?}"
        );
        assert_eq!(
            f.store.snapshot().await.enhanced_backup,
            Some(foreign),
            "the backup is left as it is"
        );
        assert!(!filled(&f.egress));
        assert!(!f.mode.enabled());
    }

    #[tokio::test]
    async fn recovering_undoes_what_a_dead_run_left_last_first() {
        let f = fixture(true).await;
        f.store
            .update(|s| {
                s.enhanced_backup = Some(json!({"undo": [
                    {"kind": "test", "step": "take_dns"},
                    {"kind": "test", "step": "adjust"},
                ]}))
            })
            .await;
        let recovered =
            EnhancedMode::recover(&(f.backend.clone() as Arc<dyn Backend>), &f.store).await;
        assert_eq!(recovered, Some(Ok(())));
        assert_eq!(
            f.backend.calls.list(),
            lines(&["undo adjust", "undo take_dns"])
        );
        assert_eq!(f.store.snapshot().await.enhanced_backup, None);
        // nothing left: nothing to do
        assert_eq!(
            EnhancedMode::recover(&(f.backend.clone() as Arc<dyn Backend>), &f.store).await,
            None
        );
        assert_eq!(f.backend.calls.list().len(), 2);
    }

    #[tokio::test]
    async fn an_undo_that_fails_stays_in_the_backup_while_recovering() {
        let f = fixture(true).await;
        f.store
            .update(|s| {
                s.enhanced_backup = Some(json!({"undo": [
                    {"kind": "test", "step": "take_dns"},
                    {"kind": "test", "step": "adjust"},
                ]}))
            })
            .await;
        *f.backend.fail_undo.lock().unwrap() = Some("take_dns".to_string());
        let recovered =
            EnhancedMode::recover(&(f.backend.clone() as Arc<dyn Backend>), &f.store).await;
        assert_eq!(
            recovered,
            Some(Err("take_dns undo failed (test)".to_string()))
        );
        assert_eq!(
            f.store.snapshot().await.enhanced_backup,
            Some(json!({"undo": [{"kind": "test", "step": "take_dns"}]}))
        );
        // and the next recovery does the one that is left
        *f.backend.fail_undo.lock().unwrap() = None;
        assert_eq!(
            EnhancedMode::recover(&(f.backend.clone() as Arc<dyn Backend>), &f.store).await,
            Some(Ok(()))
        );
        assert_eq!(f.store.snapshot().await.enhanced_backup, None);
    }

    /// A reload that changes nothing asks the system for nothing: no `routes
    /// +0 -0`, no DNS takeover, no adjustment (the phase 3 / M4a plan's
    /// execution record row 75). The egress table takes the new primary all
    /// the same.
    #[tokio::test]
    async fn a_follow_that_changes_nothing_asks_for_no_routes() {
        let mut f = fixture(true).await;
        f.mode.enable(wanted()).await.unwrap();
        let asked = f.backend.calls.list().len();
        let mut moved = wanted();
        moved.primary = Some("eth1".to_string());
        f.mode.follow(moved.clone(), false).await;
        assert_eq!(
            f.backend.calls.list().len(),
            asked,
            "no step was asked for: {:?}",
            f.backend.calls.list()
        );
        assert_eq!(f.backend.routes.lock().unwrap().len(), 1);
        let table = f
            .egress
            .read()
            .unwrap()
            .clone()
            .expect("the table is there");
        assert_eq!(table.routes, moved.routes);
        assert_eq!(table.primary.as_deref(), Some("eth1"));
        assert!(f.mode.enabled());
    }

    /// A follow that writes the table the slot already holds leaves that
    /// table in it, the same `Arc`: the sockets that outlive one connection
    /// compare routes by pointer, so only a table whose content changed may
    /// have them made again (the phase 3 / M4b plan's execution record row
    /// 69).
    #[tokio::test]
    async fn a_follow_that_writes_the_same_table_keeps_the_one_in_the_slot() {
        let mut f = fixture(true).await;
        f.mode.enable(wanted()).await.unwrap();
        let before = f
            .egress
            .read()
            .unwrap()
            .clone()
            .expect("the table is there");

        f.mode.follow(wanted(), false).await;

        let after = f
            .egress
            .read()
            .unwrap()
            .clone()
            .expect("the table is there");
        assert_eq!(*after, *before, "the same content");
        assert!(
            Arc::ptr_eq(&before, &after),
            "the table the slot held is the one it holds"
        );
    }

    /// A follow whose routes change writes another table, and with no DNS
    /// takeover to come — the network did not change and the IPv6 half did
    /// not —, the resolver is still told the system's DNS changed hands: its
    /// upstreams are bound to the table there was, and the next lookup builds
    /// them on the new one (the phase 3 / M4b plan's execution record row
    /// 69). A follow that changes nothing tells it nothing.
    #[tokio::test]
    async fn a_follow_that_writes_another_table_rebuilds_the_resolvers_upstreams() {
        let mut f = fixture_recording_system_dns(true).await;
        f.mode.enable(wanted()).await.unwrap();
        let after_open = f.backend.calls.list().len();

        // nothing changed: no step, and no callback
        f.mode.follow(wanted(), false).await;
        assert_eq!(
            f.backend.calls.list()[after_open..],
            [] as [String; 0],
            "{:?}",
            f.backend.calls.list()
        );

        // the routes only
        let mut next = wanted();
        let excluded = ["1.2.3.0/24".parse().unwrap()];
        next.routes = RouteSet::compute(&RouteInputs {
            ipv6: true,
            excluded: &excluded,
            included: &[],
            external: &[],
            explicit_space: false,
        });
        assert_ne!(next.routes, wanted().routes);
        f.mode.follow(next, false).await;
        assert_eq!(
            f.backend.calls.list()[after_open..],
            lines(&["routes", "system_dns_changed on"]),
            "the callback comes after the routes that put the table in"
        );
    }

    /// What the egress table holds while the routes cannot be installed: the
    /// old set and the new one together, so that a connection of rurge's to a
    /// route that is already in the adapter does not come back into it (the
    /// phase 3 / M4a plan's execution record row 75).
    #[tokio::test]
    async fn a_follow_that_cannot_install_keeps_the_table_a_superset() {
        let mut f = fixture(true).await;
        f.mode.enable(wanted()).await.unwrap();
        let excluded = ["1.2.3.0/24".parse().unwrap()];
        let mut next = wanted();
        next.routes = RouteSet::compute(&RouteInputs {
            ipv6: true,
            excluded: &excluded,
            included: &[],
            external: &[],
            explicit_space: false,
        });
        assert_ne!(next.routes, wanted().routes);
        *f.backend.fail.lock().unwrap() = Some("routes".to_string());
        f.mode.follow(next.clone(), false).await;
        assert!(f.mode.enabled(), "the enhanced mode goes on");
        let table = f
            .egress
            .read()
            .unwrap()
            .clone()
            .expect("the table is there");
        for net in wanted().routes.v4.iter().chain(next.routes.v4.iter()) {
            assert!(table.routes.v4.contains(net), "{net} left the table");
        }

        // what it kept is the set that went in: the next attempt asks for the
        // difference between that and the one that failed
        *f.backend.fail.lock().unwrap() = None;
        f.mode.follow(next.clone(), false).await;
        let (add, remove) = f.backend.routes.lock().unwrap()[1].clone();
        let (wanted_add, wanted_remove) = RouteSet::diff(&wanted().routes, &next.routes);
        assert_eq!(add, wanted_add);
        assert_eq!(remove, wanted_remove);
        let table = f
            .egress
            .read()
            .unwrap()
            .clone()
            .expect("the table is there");
        assert_eq!(table.routes, next.routes, "the table is what went in");
    }

    /// A change of the IPv6 half and of the network together take the DNS over
    /// once, with the IPv6 the change ended with (the phase 3 / M4a plan's
    /// execution record row 75).
    #[tokio::test]
    async fn a_follow_takes_the_dns_over_once_for_ipv6_and_the_network() {
        let mut f = fixture(true).await;
        f.mode.enable(without_ipv6()).await.unwrap();
        let after_open = f.backend.calls.list().len();
        f.mode.follow(wanted(), true).await;
        assert_eq!(
            f.backend.calls.list()[after_open..],
            lines(&[
                "configure_ipv6",
                "routes",
                "dns_undo",
                "take_dns",
                "adjust_undo",
                "adjust",
            ])
        );
        assert_eq!(
            *f.backend.dns.lock().unwrap(),
            [false, true],
            "one takeover, with the IPv6 the change ended with"
        );
        assert_eq!(f.backend.routes.lock().unwrap().len(), 2);
        let (add, remove) = f.backend.routes.lock().unwrap()[1].clone();
        assert!(!add.v6.is_empty(), "the IPv6 routes went in");
        assert!(remove.is_empty());
        assert!(f.mode.enabled());
    }

    /// A reload that turns IPv6 off hands the DNS back while the address it
    /// points at is still there, removes its routes and gives the adapter's
    /// IPv6 half back last (M4 design 9.4, 9.7; the phase 3 / M4a plan's
    /// execution record row 79).
    #[tokio::test]
    async fn a_follow_that_turns_ipv6_off_gives_the_half_back() {
        let mut f = fixture(true).await;
        f.mode.enable(wanted()).await.unwrap();
        let after_open = f.backend.calls.list().len();
        f.mode.follow(without_ipv6(), false).await;
        assert_eq!(
            f.backend.calls.list()[after_open..],
            // the mock says `routes off` for a call that only removes
            lines(&["dns_undo", "take_dns", "routes off", "unconfigure_ipv6"])
        );
        assert_eq!(
            f.backend.dns.lock().unwrap().last(),
            Some(&false),
            "the DNS is taken over without IPv6"
        );
        let (add, remove) = f.backend.routes.lock().unwrap()[1].clone();
        assert!(add.is_empty(), "nothing is added");
        assert!(remove.v4.is_empty());
        assert!(!remove.v6.is_empty(), "only the IPv6 routes go");
        assert!(f.mode.enabled());
    }

    /// The routes' IPv6 half that cannot be added is taken back by the backend,
    /// which still removes what it was asked to: what the manager records is
    /// the old IPv6 routes less the removed ones, the table holds them, IPv6
    /// stays on, and the DNS is not taken over again (the phase 3 / M4a plan's
    /// execution record row 95).
    #[tokio::test]
    async fn an_ipv6_half_that_cannot_be_added_leaves_the_old_routes_recorded() {
        let mut f = fixture(true).await;
        f.mode.enable(wanted()).await.unwrap();
        let old = wanted().routes;
        let mut moved = old.clone();
        let gone = moved.v6.remove(0);
        moved.v6.push("2001:db8::/32".parse().unwrap());
        moved.v6.sort();
        let mut next = wanted();
        next.routes = moved;
        *f.backend.no_ipv6_half.lock().unwrap() = true;
        let dns_before = f.backend.dns.lock().unwrap().len();

        f.mode.follow(next, false).await;

        let on = f.mode.on.as_ref().expect("the enhanced mode stays on");
        let kept: Vec<_> = old.v6.iter().filter(|net| **net != gone).copied().collect();
        assert_eq!(
            on.routes.as_ref().unwrap().v6,
            kept,
            "what the adapter still has, not the empty set"
        );
        assert!(on.ipv6, "IPv6 was on before and stays on");
        let table = f
            .egress
            .read()
            .unwrap()
            .clone()
            .expect("the table is there");
        for net in &kept {
            assert!(table.routes.v6.contains(net), "{net} left the table");
        }
        assert_eq!(
            f.backend.dns.lock().unwrap().len(),
            dns_before,
            "no second DNS takeover"
        );
    }

    /// IPv6 that comes on and whose routes cannot go in goes on as if IPv6 were
    /// off (M4-D17): the DNS is taken over without IPv6, and only a change of
    /// network takes it over again (the phase 3 / M4a plan's execution record
    /// row 95).
    #[tokio::test]
    async fn a_follow_that_turns_ipv6_on_without_its_half_goes_on_as_if_it_were_off() {
        let mut f = fixture(true).await;
        f.mode.enable(without_ipv6()).await.unwrap();
        *f.backend.no_ipv6_half.lock().unwrap() = true;

        f.mode.follow(wanted(), false).await;
        let on = f.mode.on.as_ref().expect("the enhanced mode stays on");
        assert!(!on.ipv6, "this follow goes on as if IPv6 were off");
        assert_eq!(
            *f.backend.dns.lock().unwrap(),
            [false],
            "no takeover: the IPv6 ended where it began"
        );

        // another network takes the DNS over once, without IPv6
        f.mode.follow(wanted(), true).await;
        assert_eq!(
            *f.backend.dns.lock().unwrap(),
            [false, false],
            "one takeover for the network, without IPv6"
        );
        assert!(!f.mode.on.as_ref().expect("the enhanced mode stays on").ipv6);
    }

    /// An open whose routes lose their IPv6 half takes the DNS over without
    /// IPv6 and says it is off (M4-D17; the phase 3 / M4a plan's execution
    /// record row 95).
    #[tokio::test]
    async fn an_open_without_its_ipv6_half_takes_the_dns_over_without_it() {
        let mut f = fixture(true).await;
        *f.backend.no_ipv6_half.lock().unwrap() = true;
        f.mode.enable(wanted()).await.unwrap();
        assert_eq!(
            *f.backend.dns.lock().unwrap(),
            [false],
            "the DNS is taken over without IPv6"
        );
        let on = f.mode.on.as_ref().expect("the open succeeded");
        assert!(!on.ipv6);
        assert!(
            on.routes.as_ref().unwrap().v6.is_empty(),
            "nothing of the IPv6 half is recorded"
        );
        assert!(f.mode.enabled());
    }

    /// A run that died with the flag on and left no undo action behind
    /// (Windows, whose backup is always empty, or Linux without an `rp_filter`
    /// to loosen): the next start has nothing to undo, and `state.json` stops
    /// saying it is on (the phase 3 / M4a plan's execution record row 99).
    #[tokio::test]
    async fn a_crash_that_left_no_undo_action_puts_the_flag_back() {
        let f = fixture(true).await;
        f.store
            .update(|s| {
                s.features.enhanced_mode = true;
            })
            .await;
        assert_eq!(
            EnhancedMode::recover(&(f.backend.clone() as Arc<dyn Backend>), &f.store).await,
            None,
            "there is nothing to undo"
        );
        let state = f.store.snapshot().await;
        assert!(!state.features.enhanced_mode, "the flag is put back");
        assert_eq!(state.enhanced_backup, None);
        assert!(
            f.backend.calls.list().is_empty(),
            "nothing is asked of the system: {:?}",
            f.backend.calls.list()
        );
    }
}
