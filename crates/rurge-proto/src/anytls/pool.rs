//! Idle AnyTLS sessions (anytls-go `proxy/session/client.go`): the newest one
//! is reused first, and one that has idled for a minute is closed.

use super::session::Session;
use crate::task::AbortOnDrop;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

/// The protocol document's suggestion: look every 30 s, close after 60 s.
pub(crate) const REAP_EVERY: Duration = Duration::from_secs(30);
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct Pool {
    idle: Mutex<Vec<(Session, Instant)>>,
}

impl Pool {
    pub(crate) fn put(&self, session: Session) {
        self.idle
            .lock()
            .expect("pool")
            .push((session, Instant::now()));
    }

    /// The live session with the highest sequence number; dead ones found on
    /// the way are dropped.
    pub(crate) fn take(&self) -> Option<Session> {
        let mut idle = self.idle.lock().expect("pool");
        idle.retain(|(s, _)| !s.is_closed());
        let newest = (0..idle.len()).max_by_key(|i| idle[*i].0.seq())?;
        Some(idle.swap_remove(newest).0)
    }

    pub(crate) fn reap(&self, now: Instant) {
        self.idle
            .lock()
            .expect("pool")
            .retain(|(s, since)| !s.is_closed() && now.duration_since(*since) < IDLE_TIMEOUT);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.idle.lock().expect("pool").len()
    }
}

/// Reaps `pool` until it is gone. Holds it weakly: the pool dies with its outbound.
pub(crate) fn spawn_reaper(pool: &Arc<Pool>) -> AbortOnDrop {
    let pool = Arc::downgrade(pool);
    AbortOnDrop(tokio::spawn(async move {
        let mut tick = tokio::time::interval(REAP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(pool) = pool.upgrade() else {
                return;
            };
            pool.reap(Instant::now());
        }
    }))
}
