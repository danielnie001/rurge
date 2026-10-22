//! Finding the process behind a session (phase 3 M2 design 4): what the
//! platform provides to look one up, the lookup that runs in the background
//! for a session, and the way the rules wait for its answer.

use rurge_config::session::{ListenerKind, ProcessInfo, SessionInfo, Transport};
use rurge_inbound::ProcessSlot;
use rurge_net::BoxFuture;
use rurge_rules::LazyProcess;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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
    /// TCP only: the other end of the client's connection, which is rurge's own
    /// end of it: what a listener accepted it on, or what the app connected to
    /// through the virtual adapter. The unspecified address in it stands for
    /// any address: only the port is compared. `None`: the client's own
    /// endpoint alone says which connection is meant.
    pub remote: Option<SocketAddr>,
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

/// The question for the process of `session`, or `None` when nobody is asked:
/// HTTP and SOCKS5 (a SOCKS5 UDP template too: its control connection) and the
/// adapter's TCP → TCP, local = `src`, remote = `local` (a listener's session
/// without it: `0.0.0.0:<in_port>`); the adapter's UDP → UDP, local = `src`, no
/// remote; rurge's own and forwarded sessions → `None`.
pub(crate) fn query_for(session: &SessionInfo, since: Instant) -> Option<OwnerQuery> {
    let (transport, remote) = match (session.listener, session.transport) {
        // The client's own connection to a proxy port, a TCP one: a UDP
        // association is looked up by its control connection (design 4.1).
        (ListenerKind::Http | ListenerKind::Socks5, _) => {
            let unspecified = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), session.in_port);
            (Transport::Tcp, Some(session.local.unwrap_or(unspecified)))
        }
        // through the virtual network card an app's connection is to what it
        // connected to, and its UDP socket has no other end
        (ListenerKind::Tun, Transport::Tcp) => (Transport::Tcp, session.local),
        (ListenerKind::Tun, Transport::Udp) => (Transport::Udp, None),
        // rurge's own sessions have no process but rurge, and what a gateway
        // forwards is not from this machine
        (ListenerKind::Forward | ListenerKind::Internal, _) => return None,
    };
    Some(OwnerQuery {
        transport,
        local: session.src,
        remote,
        since,
    })
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
    let (src, remote, since) = (query.local, query.remote, query.since);
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
    let remote = remote.map(tracing::field::display);
    match found {
        Some(process) => tracing::debug!(
            %src,
            remote,
            elapsed_ms,
            pid = process.pid,
            name = process.name.as_str(),
            path = process.path.as_deref(),
            "process lookup found the process"
        ),
        None => tracing::debug!(
            %src,
            remote,
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
    use rurge_config::HostName;

    fn app() -> ProcessInfo {
        ProcessInfo {
            pid: Some(4321),
            name: "app.exe".to_string(),
            path: Some("C:\\Apps\\app.exe".to_string()),
        }
    }

    /// What is asked of the platform for a session, and whether anything is:
    /// the sessions of the two proxy ports and of the virtual network card are
    /// looked up, each by the connection or the socket the client has, and not
    /// rurge's own, nor what a gateway forwards.
    #[test]
    fn the_query_follows_the_listener_and_the_transport() {
        let since = Instant::now();
        let client: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let rurge: SocketAddr = "127.0.0.1:6152".parse().unwrap();
        let session = |listener, transport, local| SessionInfo {
            src: client,
            in_port: 6152,
            local,
            listener,
            transport,
            ..SessionInfo::tcp(HostName::parse("example.com"), 443)
        };
        let asked = |transport, remote| {
            Some(OwnerQuery {
                transport,
                local: client,
                remote,
                since,
            })
        };

        for listener in [ListenerKind::Http, ListenerKind::Socks5] {
            let tcp = session(listener, Transport::Tcp, Some(rurge));
            // the client's TCP connection to the address it connected to
            assert_eq!(
                query_for(&tcp, since),
                asked(Transport::Tcp, Some(rurge)),
                "{listener:?}"
            );
            // an association (its template is a UDP session) is looked up by
            // its control connection, which is a TCP one
            let udp = session(listener, Transport::Udp, Some(rurge));
            assert_eq!(
                query_for(&udp, since),
                asked(Transport::Tcp, Some(rurge)),
                "{listener:?}"
            );
            // a session that does not say where it was accepted: any address
            // of the listener's port
            let unsaid = session(listener, Transport::Tcp, None);
            assert_eq!(
                query_for(&unsaid, since),
                asked(Transport::Tcp, Some("0.0.0.0:6152".parse().unwrap())),
                "{listener:?}"
            );
        }

        // the virtual network card: a TCP connection to what the app
        // connected to, a UDP socket by its own endpoint alone
        let app_connected_to: SocketAddr = "192.0.2.1:443".parse().unwrap();
        let tcp = session(ListenerKind::Tun, Transport::Tcp, Some(app_connected_to));
        assert_eq!(
            query_for(&tcp, since),
            asked(Transport::Tcp, Some(app_connected_to))
        );
        let udp = session(ListenerKind::Tun, Transport::Udp, None);
        assert_eq!(query_for(&udp, since), asked(Transport::Udp, None));
        // a connection that does not say what the app connected to is found
        // by the app's own endpoint
        let unsaid = session(ListenerKind::Tun, Transport::Tcp, None);
        assert_eq!(query_for(&unsaid, since), asked(Transport::Tcp, None));

        // rurge's own sessions have no process but rurge, and what a gateway
        // forwards is not this machine's: nobody is asked
        for listener in [ListenerKind::Forward, ListenerKind::Internal] {
            for transport in [Transport::Tcp, Transport::Udp] {
                let session = session(listener, transport, Some(rurge));
                assert_eq!(query_for(&session, since), None, "{listener:?}");
            }
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
