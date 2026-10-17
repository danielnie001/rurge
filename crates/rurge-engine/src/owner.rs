//! Finding the process behind a session (phase 3 M2 design 4): what the
//! platform provides to look one up, the lookup that runs in the background
//! for a session, and the way the rules wait for its answer.

use rurge_config::session::{ListenerKind, ProcessInfo, Transport};
use rurge_inbound::ProcessSlot;
use rurge_net::BoxFuture;
use rurge_rules::LazyProcess;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// How long the rules wait for the process of a session, from the moment its
/// lookup was launched.
pub const PROCESS_WAIT: Duration = Duration::from_millis(200);
/// How many lookups use a blocking thread at once.
pub const LOOKUPS: usize = 2;
/// The note on a session whose rules gave up waiting for its process.
pub const NOT_IN_TIME: &str = "Process not known within 200 ms: PROCESS-NAME did not match";
/// What is warned about, once, when the rules first give up waiting.
pub const SLOW: &str =
    "process lookup took longer than 200 ms; PROCESS-NAME rules decided without the process";

/// The client's own connection, for the lookup of the process that owns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerQuery {
    pub transport: Transport,
    /// The client's own endpoint: the session's source address.
    pub local: SocketAddr,
    /// TCP only: the port the client connected to (rurge's listening port).
    pub remote_port: Option<u16>,
    /// When the session began; decides which reads of the system's tables
    /// can answer.
    pub since: Instant,
}

/// What finds the process behind a client's connection (the platform's, for
/// `rurge run`); `None` in `EngineShared` looks nothing up.
pub trait OwnerLookup: Send + Sync {
    /// Never blocks: `Some(false)` when `ip` is surely not this machine's,
    /// `None` when that is not known without a look.
    fn is_local(&self, ip: IpAddr) -> Option<bool>;
    /// Blocking: called on a blocking thread. `None`: the process is not
    /// known, whatever the reason.
    fn owner(&self, query: &OwnerQuery) -> Option<ProcessInfo>;
}

/// Whether the sessions of a listener are looked up: the ones that come in
/// through a proxy port and may be from this machine.
pub(crate) fn looked_up(listener: ListenerKind) -> bool {
    match listener {
        ListenerKind::Http | ListenerKind::Socks5 => true,
        // rurge's own sessions have no process but rurge; the virtual
        // network card's come with phase 3 M3
        ListenerKind::Tun | ListenerKind::Forward | ListenerKind::Internal => false,
    }
}

/// The lookup of one session or association: waits for a place among the
/// `LOOKUPS`, asks `owners` on a blocking thread, and settles `slot` with
/// the answer — whatever happens to the lookup, and however long it took.
/// It holds neither the engine nor the session, so a session that is gone
/// and an engine that is dropped do not wait for it.
pub(crate) async fn find(
    owners: Arc<dyn OwnerLookup>,
    places: Arc<Semaphore>,
    slot: ProcessSlot,
    query: OwnerQuery,
) {
    let (src, listen_port, since) = (query.local, query.remote_port, query.since);
    let found = match places.acquire_owned().await {
        Ok(place) => {
            // the place is the blocking call's until it has returned (or
            // panicked, which ends it as well)
            let blocking = tokio::task::spawn_blocking(move || {
                let found = owners.owner(&query);
                drop(place);
                found
            });
            // a lookup that panicked found nothing
            blocking.await.unwrap_or(None)
        }
        // the places are never closed; a lookup that has none finds nothing
        Err(_) => None,
    };
    slot.settle(found.clone());
    // the lookup is timed from its launch: the wait for a place counts
    let elapsed_ms = since.elapsed().as_millis() as u64;
    let listen_port = listen_port.unwrap_or_default();
    match found {
        Some(process) => tracing::debug!(
            %src,
            listen_port,
            elapsed_ms,
            pid = process.pid,
            name = process.name.as_str(),
            path = process.path.as_deref(),
            "process lookup found the process"
        ),
        None => tracing::debug!(
            %src,
            listen_port,
            elapsed_ms,
            "process lookup found no process"
        ),
    }
}

/// A session's slot as what the rules ask for the process of the session: the
/// answer is the slot's, waited for until `PROCESS_WAIT` after its lookup was
/// launched — a slot that has none is settled, and answers at once. When the
/// wait ends first, the rules decide without the process, and this says so.
pub(crate) struct SlotProcess<'a> {
    slot: &'a ProcessSlot,
    timed_out: AtomicBool,
}

impl<'a> SlotProcess<'a> {
    pub(crate) fn new(slot: &'a ProcessSlot) -> SlotProcess<'a> {
        SlotProcess {
            slot,
            timed_out: AtomicBool::new(false),
        }
    }

    /// Whether the wait for the process ended without it.
    pub(crate) fn timed_out(&self) -> bool {
        self.timed_out.load(Ordering::Relaxed)
    }
}

impl LazyProcess for SlotProcess<'_> {
    fn process(&self) -> BoxFuture<'_, Option<ProcessInfo>> {
        Box::pin(async move {
            // a slot with no lookup behind it is settled: it answers on the
            // first poll, whatever the deadline is
            let deadline = self
                .slot
                .launched()
                .map_or_else(Instant::now, |launched| launched + PROCESS_WAIT);
            match self.slot.settled_by(deadline).await {
                Some(found) => found,
                None => {
                    self.timed_out.store(true, Ordering::Relaxed);
                    None
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> ProcessInfo {
        ProcessInfo {
            pid: Some(4321),
            name: "app.exe".to_string(),
            path: Some("C:\\Apps\\app.exe".to_string()),
        }
    }

    /// The sessions of the two proxy ports; not rurge's own, not the virtual
    /// network card's, not what a gateway forwards.
    #[test]
    fn only_the_two_proxy_listeners_are_looked_up() {
        for (kind, looked) in [
            (ListenerKind::Http, true),
            (ListenerKind::Socks5, true),
            (ListenerKind::Tun, false),
            (ListenerKind::Forward, false),
            (ListenerKind::Internal, false),
        ] {
            assert_eq!(looked_up(kind), looked, "{kind:?}");
        }
    }

    /// A lookup launched 300 ms ago has had its 200 ms: the rules do not wait
    /// for it again, not even for the time a poll takes. Nothing here depends
    /// on how long anything takes — `timeout` polls what it is given before
    /// it looks at the time, so the answer is `Ok` only if the first poll
    /// gives it.
    #[tokio::test]
    async fn a_window_that_has_passed_is_not_waited_for() {
        let launched = Instant::now()
            .checked_sub(Duration::from_millis(300))
            .expect("the machine has been up for 300 ms");
        let slot = ProcessSlot::pending_at(launched);
        let wait = SlotProcess::new(&slot);
        assert!(!wait.timed_out());
        let answer = tokio::time::timeout(Duration::ZERO, wait.process()).await;
        assert!(matches!(answer, Ok(None)), "{answer:?}");
        assert!(wait.timed_out());
        // the lookup that comes in late settles the slot for whoever asks
        // next: the record shows it, and so would the next session
        slot.settle(Some(app()));
        let answer = tokio::time::timeout(Duration::ZERO, wait.process()).await;
        assert!(matches!(answer, Ok(Some(_))), "{answer:?}");
    }

    /// A slot that is settled answers on the first poll, and that is no
    /// timeout: neither a process that was found, nor one that never was
    /// looked for.
    #[tokio::test]
    async fn a_settled_slot_is_not_a_timeout() {
        let known = ProcessSlot::known(app());
        let wait = SlotProcess::new(&known);
        let answer = tokio::time::timeout(Duration::ZERO, wait.process()).await;
        assert_eq!(answer.expect("answered on the first poll"), Some(app()));
        assert!(!wait.timed_out());
        let unknown = ProcessSlot::unknown();
        let wait = SlotProcess::new(&unknown);
        let answer = tokio::time::timeout(Duration::ZERO, wait.process()).await;
        assert_eq!(answer.expect("answered on the first poll"), None);
        assert!(!wait.timed_out());
        // a lookup that was launched and has finished: the same, long after
        let launched = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .expect("the machine has been up for 5 s");
        let done = ProcessSlot::pending_at(launched);
        done.settle(None);
        let wait = SlotProcess::new(&done);
        let answer = tokio::time::timeout(Duration::ZERO, wait.process()).await;
        assert_eq!(answer.expect("answered on the first poll"), None);
        assert!(!wait.timed_out());
    }

    /// A lookup that settles the slot inside the window is the answer, and
    /// the wait ends with it.
    #[tokio::test]
    async fn a_slot_settled_inside_the_window_is_the_answer() {
        let slot = ProcessSlot::pending();
        let settling = tokio::spawn({
            let slot = slot.clone();
            async move { slot.settle(Some(app())) }
        });
        let wait = SlotProcess::new(&slot);
        let answer = wait.process().await;
        settling.await.expect("the settling ends");
        assert_eq!(answer, Some(app()));
        assert!(!wait.timed_out());
    }
}
