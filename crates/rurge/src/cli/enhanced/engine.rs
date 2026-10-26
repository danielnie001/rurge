//! The TUN the manager runs in `rurge run` (P6): the engine's own, started
//! through `Engine::start_tun`. Nothing here starts a virtual adapter by
//! itself; the platform backend hands the manager the device.

use super::{NoPinger, RunningTun, Starter};
use rurge_engine::{Engine, TunCounters};
use rurge_net::BoxFuture;
use rurge_tun::device::Device;
use rurge_tun::{Ending, TunHandle, TunOptions};
use std::sync::Arc;

/// The starter `rurge run` gives its manager: the engine's TUN, with no echo
/// requests going out in this version (P15).
pub fn starter(engine: Arc<Engine>) -> Arc<dyn Starter> {
    Arc::new(EngineStarter { engine })
}

struct EngineStarter {
    engine: Arc<Engine>,
}

impl Starter for EngineStarter {
    fn start(&self, device: Arc<dyn Device>) -> BoxFuture<'_, Result<Arc<dyn RunningTun>, String>> {
        let engine = self.engine.clone();
        Box::pin(async move {
            let handle = engine
                .start_tun(device, Arc::new(NoPinger), TunOptions::default())
                .await?;
            let tun: Arc<dyn RunningTun> = Arc::new(EngineTun { handle, engine });
            Ok(tun)
        })
    }
}

/// The engine's TUN as the manager holds it. `ended` is the one a close waits
/// for, and it includes the engine's last save of its fake-address pool; the
/// task that watches for a failed device waits for `ending` alone (the phase 3 /
/// M4a plan's execution record row 62).
struct EngineTun {
    handle: TunHandle,
    engine: Arc<Engine>,
}

impl RunningTun for EngineTun {
    fn stop(&self) {
        self.handle.stop();
    }

    fn ended(&self) -> BoxFuture<'_, Ending> {
        Box::pin(async move {
            let ending = self.handle.join().await;
            self.engine.tun_ended().await;
            ending
        })
    }

    fn ending(&self) -> BoxFuture<'_, Ending> {
        Box::pin(async move { self.handle.ending().await })
    }

    fn counters(&self) -> TunCounters {
        self.handle.counters()
    }
}
