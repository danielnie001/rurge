//! The engine behind the virtual adapter (phase 3 M3 design 9): what the TUN
//! asks of it as its `Hooks`, and `Engine::start_tun`, which starts a TUN on a
//! device with the engine as its dialer, opens the pool of fake addresses in
//! the data directory and keeps it saved.

use crate::engine::Engine;
use arc_swap::ArcSwapOption;
use rurge_config::{PreMatchLayer, SessionInfo};
use rurge_dns::fakeip::{FILE_NAME, FakeIpPool, Loaded, QUARANTINE, QUARANTINE_COUNT};
use rurge_inbound::Dialer;
use rurge_proto::RejectKind;
use rurge_tun::device::Device;
use rurge_tun::{Hooks, Pinger, Tun, TunHandle, TunOptions, TunSettings};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The MTU a device of the TUN needs at the least: smoltcp 0.12 cannot work
/// with less, and the MTU is rurge's own choice (as [`Tun::start`] asserts).
const MIN_MTU: usize = 1280;

/// How often the pool's file is written at most while mappings change.
const POOL_SAVE_EVERY: Duration = Duration::from_secs(10);

/// Takes `pool` out of `slot`, when it is the pool there: a TUN that was started
/// after the one the pool belonged to has put its own there by now, and keeps
/// it.
fn let_go(slot: &ArcSwapOption<FakeIpPool>, pool: &Arc<FakeIpPool>) {
    slot.rcu(|stored| match stored {
        Some(stored) if Arc::ptr_eq(stored, pool) => None,
        other => other.clone(),
    });
}

impl Hooks for Engine {
    /// The settings of the generation in use now: a reload that comes while the
    /// TUN runs is seen by the next thing the TUN asks.
    fn settings(&self) -> Arc<TunSettings> {
        self.runtime().tun.clone()
    }

    /// The pre-matching rules of the generation in use (`pre_match_refusal`).
    fn pre_match(&self, layer: PreMatchLayer, session: &SessionInfo) -> Option<RejectKind> {
        self.pre_match_refusal(layer, session)
    }
}

impl Engine {
    /// Opens the fake-address pool in the data directory (on a blocking thread),
    /// starts a TUN on `device` with this engine as its dialer and hooks, and keeps
    /// the pool saved until the TUN has ended. The TUN stops when the engine stops
    /// accepting. While it runs, the proxy listeners turn the fake addresses that
    /// come in at them back into names with the pool.
    ///
    /// The pool's file is one pool's at a time: another TUN is started on this
    /// data directory only once the first has ended and its last save is written
    /// (`clean 1` on the file's second line; the engine's tracker waits for it).
    ///
    /// # Errors
    ///
    /// Options that [`TunOptions::check`] refuses, a device whose MTU is below
    /// 1280, and a TUN that still runs on this engine (or whose pool's last save
    /// is still being written: see [`Engine::tun_ended`]). Nothing is opened or
    /// started then.
    pub async fn start_tun(
        self: &Arc<Self>,
        device: Arc<dyn Device>,
        pinger: Arc<dyn Pinger>,
        options: TunOptions,
    ) -> Result<TunHandle, String> {
        options.check()?;
        if device.mtu() < MIN_MTU {
            return Err("the device's MTU is below 1280".to_string());
        }
        // One TUN to an engine: the permit is taken before anything is opened,
        // so that a second TUN neither runs beside this one nor writes its
        // pool's file. It ends with the task that writes the last save and
        // lets go of the pool; any way out of this function before then drops
        // it.
        let permitted = self
            .tun_permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| "a TUN is already running on this engine".to_string())?;
        let path = self.data_dir().join(FILE_NAME);
        let (pool, loaded) = tokio::task::spawn_blocking({
            let (path, now) = (path.clone(), Instant::now());
            move || FakeIpPool::open(&path, now)
        })
        .await
        .expect("opening the pool of fake addresses does not panic");
        // said here, on the caller's thread, not by the pool: the file's
        // content is no business of a log (the reason quotes none of it)
        match loaded {
            Loaded::NotRead(error) => tracing::warn!(
                %error,
                "fake-ip: the saved addresses could not be read; starting with none"
            ),
            Loaded::Unclean => tracing::info!(
                "fake-ip: rurge did not stop cleanly last time; {} fake addresses are held back for {} seconds",
                QUARANTINE_COUNT,
                QUARANTINE.as_secs()
            ),
            Loaded::Missing | Loaded::Clean => {}
        }
        let dialer: Arc<dyn Dialer> = self.clone();
        let hooks: Arc<dyn Hooks> = self.clone();
        // the proxy listeners turn the fake addresses that come in at them
        // back into names with this pool, as long as the TUN runs
        self.fake_pool.store(Some(pool.clone()));
        let tun = Tun::start(device, dialer, hooks, pinger, Some(pool.clone()), options);
        // The pool is written while its mappings change, and once more when the
        // TUN has ended: the engine's tracker waits for that last save. After
        // it the engine has no pool of the TUN's, unless a TUN that started in
        // the meantime has put its own there.
        let ended = tun.clone();
        let engine = Arc::downgrade(self);
        let kept = pool.clone();
        self.tracker().spawn(async move {
            pool.keep_saved(path, POOL_SAVE_EVERY, async move {
                let _ = ended.join().await;
            })
            .await;
            if let Some(engine) = engine.upgrade() {
                let_go(&engine.fake_pool, &kept);
            }
            // The last save is written and the pool is let go: the engine has
            // no TUN now (`Engine::tun_ended`).
            drop(permitted);
        });
        // The TUN stops with the engine's accepting, as the listeners do; and
        // this task ends with the TUN when that comes first.
        let stopped = self.accepting_token();
        let stopping = tun.clone();
        self.tracker().spawn(async move {
            tokio::select! {
                () = stopped.cancelled() => stopping.stop(),
                _ = stopping.join() => {}
            }
        });
        Ok(tun)
    }

    /// Resolves once no TUN runs on this engine: the last one has ended and its
    /// pool's last save is written.
    pub async fn tun_ended(&self) {
        // The semaphore has one permit and is never closed, so this waits for
        // the task that keeps the pool saved (`start_tun`) and takes the permit
        // back at once.
        let permitted = self
            .tun_permits
            .acquire()
            .await
            .expect("the semaphore of the TUN's place is never closed");
        drop(permitted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pool is let go by the end of its own TUN only: a TUN that has been
    /// started since keeps its pool, and an empty place stays empty.
    #[test]
    fn a_pool_is_let_go_by_its_own_end_only() {
        let (first, second) = (Arc::new(FakeIpPool::new()), Arc::new(FakeIpPool::new()));
        let slot = ArcSwapOption::empty();
        slot.store(Some(first.clone()));
        slot.store(Some(second.clone()));
        let_go(&slot, &first);
        let kept = slot.load_full();
        assert!(
            kept.is_some_and(|pool| Arc::ptr_eq(&pool, &second)),
            "the pool of the TUN that started later stays"
        );
        let_go(&slot, &second);
        assert!(slot.load_full().is_none());
        let_go(&slot, &first);
        assert!(slot.load_full().is_none());
    }
}
