//! The enhanced mode as `rurge run` drives it (M4 design §9): the manager,
//! what it asks of the system, the backup in `state.json` that lets a crash be
//! undone, and the TUN behind it.
//!
//! Nothing here reaches the machine of its own accord: the backend does what
//! the steps say, and in this version the only backend that does anything is
//! the file one tests set (`RURGE_ENHANCED_BACKEND=file:<path>`). The backend's
//! actions are all blocking and all called on a blocking thread; the manager is
//! owned by the `rurge run` main loop, which calls it one request at a time.

mod backend;
mod file;
mod undo;
pub(crate) mod wanted;

pub use backend::backend;
use backend::{Backend, Opened};
use rurge_engine::state::StateStore;
use rurge_net::BoxFuture;
use rurge_platform::route::RouteSet;
use rurge_platform::socket::egress::EgressTable;
use rurge_tun::device::Device;
use rurge_tun::{Ending, Pinger};
use std::io;
use std::net::IpAddr;
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
#[cfg_attr(not(test), allow(dead_code))] // wired up by M4a task 8
pub fn egress_slot() -> EgressSlot {
    Arc::new(std::sync::RwLock::new(None))
}

fn set_egress(slot: &EgressSlot, table: Option<EgressTable>) {
    let mut slot = slot.write().unwrap_or_else(|e| e.into_inner());
    *slot = table.map(Arc::new);
}

/// Whether the egress table is in the slot: the mock backend and the tests ask
/// (M4b's reader is the one that reads it for a decision).
#[cfg_attr(not(test), allow(dead_code))] // wired up by M4b
fn filled(slot: &EgressSlot) -> bool {
    slot.read().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Starts the TUN the manager runs: the engine's, or a test's (P6: the engine
/// cannot make a TUN that never ends, and a close's limit needs one).
#[cfg_attr(not(test), allow(dead_code))] // wired up by M4a task 8
pub trait Starter: Send + Sync {
    fn start(&self, device: Arc<dyn Device>) -> BoxFuture<'_, Result<Arc<dyn RunningTun>, String>>;
}

/// A TUN the manager runs.
#[cfg_attr(not(test), allow(dead_code))] // wired up by M4a task 8
pub trait RunningTun: Send + Sync {
    fn stop(&self);
    /// Ends when the TUN and all it holds of the engine have ended
    /// (`TunHandle::join`, then `Engine::tun_ended`), and says how: a device
    /// that failed ends the enhanced mode (P24, task 8).
    fn ended(&self) -> BoxFuture<'_, Ending>;
    fn counters(&self) -> rurge_engine::TunCounters;
}

/// Echo requests while the enhanced mode runs: in this version none go out
/// (P15; M4 design §10 puts a real one here in M4b).
#[cfg_attr(not(test), allow(dead_code))] // wired up by M4a task 8
pub struct NoPinger;

impl Pinger for NoPinger {
    fn echo<'a>(&'a self, _dst: IpAddr, _payload: &'a [u8]) -> BoxFuture<'a, io::Result<Vec<u8>>> {
        Box::pin(async {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "ICMP forwarding is not available in this version",
            ))
        })
    }
}

/// For the API: whether the enhanced mode is on, and its TUN's counters.
#[cfg_attr(not(test), allow(dead_code))] // wired up by M4a task 8
pub struct Shared {
    enabled: AtomicBool,
    /// The TUN the counters come from while it runs.
    tun: Mutex<Option<Arc<dyn RunningTun>>>,
}

/// What the API says of the enhanced mode, all zero while it is off. M4a task 8
/// moves this to `rurge_engine::EnhancedStatus`; until then it is the
/// manager's own.
#[cfg_attr(not(test), allow(dead_code))] // wired up by M4a task 8
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub enabled: bool,
    pub counters: rurge_engine::TunCounters,
}

impl Shared {
    fn new() -> Shared {
        Shared {
            enabled: AtomicBool::new(false),
            tun: Mutex::new(None),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))] // wired up by M4a task 8
    pub fn status(&self) -> Status {
        let tun = self.tun.lock().unwrap_or_else(|e| e.into_inner());
        let enabled = self.enabled.load(Ordering::SeqCst);
        Status {
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

/// Owned by the `rurge run` main loop, which calls it one request at a time.
pub struct EnhancedMode {
    backend: Arc<dyn Backend>,
    store: Arc<StateStore>,
    starter: Arc<dyn Starter>,
    egress: EgressSlot,
    shared: Arc<Shared>,
    /// What is on, and what a close has to undo.
    on: Option<On>,
}

/// What the enhanced mode did and holds while it is on; while it opens, the
/// part of that it has got to.
#[derive(Default)]
struct On {
    dev: Option<Opened>,
    tun: Option<Arc<dyn RunningTun>>,
    /// The routes installed into the adapter.
    routes: Option<RouteSet>,
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
        let value = store.snapshot().await.enhanced_backup?;
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
                Err(e) => closing.keep(&undo.label(), e),
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

    #[cfg_attr(not(test), allow(dead_code))] // wired up by M4a task 8
    pub fn new(
        backend: Arc<dyn Backend>,
        store: Arc<StateStore>,
        starter: Arc<dyn Starter>,
        egress: EgressSlot,
    ) -> EnhancedMode {
        EnhancedMode {
            backend,
            store,
            starter,
            egress,
            shared: Arc::new(Shared::new()),
            on: None,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))] // wired up by M4a task 8
    pub fn enabled(&self) -> bool {
        self.on.is_some()
    }

    #[cfg_attr(not(test), allow(dead_code))] // wired up by M4a task 8
    pub fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }

    /// Already on: `Ok` at once.
    #[cfg_attr(not(test), allow(dead_code))] // wired up by M4a task 8
    pub async fn enable(&mut self, wanted: Wanted) -> Result<(), String> {
        if self.on.is_some() {
            return Ok(());
        }
        let saved = self.saved_undo().await?;
        let mut on = On::default();
        match self.open(&mut on, &wanted, &saved).await {
            Ok(()) => {
                let tun = on.tun.clone().expect("an open that succeeded has a TUN");
                self.shared.set_on(tun);
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

    /// Never stops halfway; the first error is what comes back.
    #[cfg_attr(not(test), allow(dead_code))] // wired up by M4a task 8
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

        // 1. the device
        let backend = self.backend.clone();
        let dev = call(move || backend.open()).await?;
        on.dev = Some(dev.clone());

        // 2. the adapter, with its IPv6 half when it is wanted. It is always
        // asked: its address, MTU and link are wanted whether or not IPv6 is,
        // and the answer is only about the IPv6 half.
        let backend = self.backend.clone();
        let asked = wanted.ipv6;
        let with = dev.clone();
        let configured = call(move || backend.configure(&with, asked)).await?;
        let ipv6 = asked && configured;

        // 3. the TUN, so that packets are taken before traffic comes to it
        let tun = self.starter.start(dev.device.clone()).await?;
        on.tun = Some(tun);

        // 4. the egress table, before the routes: once they are in, rurge's
        // own connections must leave by the physical adapter
        set_egress(
            &self.egress,
            Some(EgressTable {
                routes: wanted.routes.clone(),
                primary: wanted.primary.clone(),
            }),
        );
        on.egress = true;

        // 5. the routes
        let mut routes = wanted.routes.clone();
        if !ipv6 {
            routes.v6.clear();
        }
        let backend = self.backend.clone();
        let with = dev.clone();
        let add = routes.clone();
        let added = call(move || backend.routes(&with, &add, &RouteSet::default())).await?;
        if !added {
            // the IPv6 half could not be added, and the backend has warned
            routes.v6.clear();
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
        // so it is undone as if it had
        on.dns = true;
        let backend = self.backend.clone();
        let with = dev.clone();
        call(move || backend.take_dns(&with, ipv6)).await?;

        // 8. the system's own adjustments, the same way
        let backend = self.backend.clone();
        let have = undo.clone();
        let primary = wanted.primary.clone();
        let saved_adjust = call(move || backend.adjust_undo(primary.as_deref(), &have)).await?;
        if let Some(one) = saved_adjust {
            undo.push(one);
            self.save_undo(&undo).await?;
        }
        on.adjust = true;
        let backend = self.backend.clone();
        call(move || backend.adjust()).await?;

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
            set_egress(&self.egress, None);
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
            closing.keep(&undo.label(), e);
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
        /// The routes of each `routes` call, as (add, remove).
        routes: Arc<Mutex<Vec<(RouteSet, RouteSet)>>>,
        /// What `configure` was asked for, and what `take_dns` was given.
        configured: Arc<Mutex<Vec<bool>>>,
        dns: Arc<Mutex<Vec<bool>>>,
        closes: Arc<AtomicUsize>,
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
            Ok(true)
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
        /// Whether `ended()` answers at once.
        ends: bool,
    }

    impl RunningTun for MockTun {
        fn stop(&self) {
            self.stopped.store(true, Ordering::SeqCst);
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

        fn counters(&self) -> rurge_engine::TunCounters {
            rurge_engine::TunCounters::default()
        }
    }

    /// A starter whose TUN ends at once, or never.
    #[derive(Default)]
    struct MockStarter {
        calls: Calls,
        fail: Arc<Mutex<Option<String>>>,
        ends: bool,
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
                    ends: self.ends,
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
        mode: EnhancedMode,
    }

    /// A manager over a mock backend and a mock starter, `state.json` in a
    /// temporary directory. `ends` says whether the TUN's `ended()` answers.
    async fn fixture(ends: bool) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state.json");
        let (store, _) = StateStore::open(state.clone()).await;
        let calls = Calls::default();
        let egress = egress_slot();
        let backend = Arc::new(Mock {
            calls: calls.clone(),
            egress: egress.clone(),
            state: Arc::new(Mutex::new(Some(state.clone()))),
            ..Mock::default()
        });
        let starter = Arc::new(MockStarter {
            calls,
            ends,
            ..MockStarter::default()
        });
        let mode = EnhancedMode::new(
            backend.clone(),
            store.clone(),
            starter.clone(),
            egress.clone(),
        );
        Fixture {
            _dir: dir,
            state,
            store,
            backend,
            starter,
            egress,
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

    /// This version sends no echo requests: every one of them says so at once
    /// (P15; M4b brings a real pinger).
    #[tokio::test]
    async fn no_echo_request_goes_out_in_this_version() {
        let error = NoPinger
            .echo("127.0.0.1".parse().unwrap(), b"ping")
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(
            error.to_string(),
            "ICMP forwarding is not available in this version"
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
}
