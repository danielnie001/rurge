//! The engine behind the virtual adapter (phase 3 M3 design 9): what the TUN
//! asks of it as its `Hooks`, and `Engine::start_tun`, which starts a TUN on a
//! device with the engine as its dialer, opens the pool of fake addresses in
//! the data directory and keeps it saved.

use crate::engine::Engine;
use rurge_config::{PreMatchLayer, SessionInfo};
use rurge_dns::fakeip::{FILE_NAME, FakeIpPool, Loaded, QUARANTINE, QUARANTINE_COUNT};
use rurge_inbound::Dialer;
use rurge_proto::RejectKind;
use rurge_tun::device::Device;
use rurge_tun::{Hooks, Pinger, Tun, TunHandle, TunOptions, TunSettings};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the pool's file is written at most while mappings change.
const POOL_SAVE_EVERY: Duration = Duration::from_secs(10);

impl Hooks for Engine {
    /// The settings of the generation in use now: a reload that comes while the
    /// TUN runs is seen by the next thing the TUN asks.
    fn settings(&self) -> Arc<TunSettings> {
        self.runtime().tun.clone()
    }

    /// Refuses nothing yet: the engine's pre-matching rules are not asked here
    /// until phase 3 / M3c has wired them.
    fn pre_match(&self, _layer: PreMatchLayer, _session: &SessionInfo) -> Option<RejectKind> {
        None
    }
}

impl Engine {
    /// Opens the fake-address pool in the data directory (on a blocking thread),
    /// starts a TUN on `device` with this engine as its dialer and hooks, and keeps
    /// the pool saved until the TUN has ended. The TUN stops when the engine stops
    /// accepting.
    ///
    /// The pool's file is one pool's at a time: another TUN is started on this
    /// data directory only once the first has ended and its last save is written
    /// (`clean 1` on the file's second line; the engine's tracker waits for it).
    ///
    /// # Panics
    ///
    /// As [`Tun::start`] does: for options that [`TunOptions::check`] refuses,
    /// and for a device whose MTU is below 1280. The pool's file has been opened
    /// by then, and says that the run has not ended: whoever starts a TUN checks
    /// the options first.
    pub async fn start_tun(
        self: &Arc<Self>,
        device: Arc<dyn Device>,
        pinger: Arc<dyn Pinger>,
        options: TunOptions,
    ) -> TunHandle {
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
        let tun = Tun::start(device, dialer, hooks, pinger, Some(pool.clone()), options);
        // The pool is written while its mappings change, and once more when the
        // TUN has ended: the engine's tracker waits for that last save.
        let ended = tun.clone();
        self.tracker()
            .spawn(pool.keep_saved(path, POOL_SAVE_EVERY, async move {
                let _ = ended.join().await;
            }));
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
        tun
    }
}
