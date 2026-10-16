//! The network the machine is on, as the daemon and the offline commands look
//! at it (phase 3 M1 design 4).
//!
//! `rurge-platform` reads the network (`netenv`) and says when it may have
//! changed. This module turns what it reads into what the engine looks at,
//! decides what is a change and when to tell of it, and hands the changes to
//! the main loop of `rurge run`, which applies them one at a time beside the
//! reloads and the commands of the API.

use rurge_config::{NetKind, NetworkFacts, Primary, WifiLink};
use rurge_platform::netenv::{self, Backend, ReadOpts, Snapshot};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;
use tokio_util::task::AbortOnDropHandle;

/// The network is read no more than once in this time: the hints that come
/// meanwhile are one, answered by a read when the time is over. A read can run
/// several programs, and a switch of network is many hints. A change that a
/// read finds is told at once, so the first change after a quiet time is told
/// as it comes, and the ones that follow within the time are one, told when it
/// is over.
const WINDOW: Duration = Duration::from_secs(1);

/// The backend the command asks for: the machine when nothing says otherwise.
/// `option` is `--network-backend`, which `RURGE_NETWORK_BACKEND` stands
/// behind: on the command line it takes the place of the variable. A value
/// rurge does not know is an error, never the machine, and it is refused in the
/// same words whichever way it came.
pub fn backend(option: Option<&str>) -> anyhow::Result<Backend> {
    match option {
        Some(value) => netenv::parse_backend(value),
        // Neither, as far as the option's `env` can tell. The variable is asked
        // for all the same: a start that lost it would read the machine.
        None => netenv::backend_from_env(),
    }
    .map_err(anyhow::Error::msg)
}

/// What the engine looks at, out of what the platform says. The DNS servers
/// and the addresses of the interface are left out: they tell a change, and
/// nothing is matched against them.
pub fn facts_of(snapshot: &Snapshot) -> NetworkFacts {
    NetworkFacts {
        primary: snapshot.primary.as_ref().map(|primary| Primary {
            interface: primary.interface.clone(),
            kind: match primary.kind {
                netenv::NetKind::Wifi => NetKind::Wifi,
                netenv::NetKind::Wired => NetKind::Wired,
                netenv::NetKind::Other => NetKind::Other,
            },
            routers: primary.routers.clone(),
        }),
        wifi: snapshot.wifi.as_ref().map(|wifi| WifiLink {
            interface: wifi.interface.clone(),
            ssid: wifi.ssid.clone(),
            bssid: wifi.bssid.clone(),
        }),
    }
}

/// Reads the network on a thread of the blocking pool: it may run programs,
/// each for two seconds at most. `None` is a read that did not happen (the
/// reader of the platform panicked), which says nothing of the network: it is
/// not a network that is not known.
async fn look(backend: Backend) -> Option<Snapshot> {
    match tokio::task::spawn_blocking(move || netenv::read(&backend, &ReadOpts::default())).await {
        Ok(snapshot) => Some(snapshot),
        Err(e) => {
            tracing::debug!(panicked = e.is_panic(), "cannot read the network");
            None
        }
    }
}

/// The network as the commands that run without a daemon look at it: read
/// once, and as it is then.
pub async fn read_once(backend: &Backend) -> NetworkFacts {
    look(backend.clone())
        .await
        .map(|snapshot| facts_of(&snapshot))
        .unwrap_or_default()
}

/// What a daemon begins with: the first look, and what listens for the
/// changes that follow it.
pub struct Started {
    hints: Arc<Notify>,
    /// The network as it was when the daemon began.
    pub snapshot: Snapshot,
}

/// Begins to listen, then looks. In this order: what changes between the two
/// is a hint, and not lost. Call it once per process.
pub async fn start(backend: &Backend) -> Started {
    let hints = forward_hints(netenv::hints(backend));
    let snapshot = look(backend.clone()).await.unwrap_or_default();
    Started { hints, snapshot }
}

/// The hints of the platform, which come on a channel a thread waits on, as a
/// `Notify` a task can wait on. Hints that come while nobody looks are one
/// permit: a hint is only a reason to look.
///
/// The thread ends when the `Notify` is gone, at the next hint, as the one of
/// the platform ends at its next hint when this one has gone.
fn forward_hints(hints: std::sync::mpsc::Receiver<()>) -> Arc<Notify> {
    let notify = Arc::new(Notify::new());
    let listener = Arc::downgrade(&notify);
    let spawned = std::thread::Builder::new()
        .name("rurge-network-notify".to_string())
        .spawn(move || {
            while hints.recv().is_ok() {
                let Some(notify) = listener.upgrade() else {
                    break;
                };
                notify.notify_one();
            }
        });
    if let Err(e) = spawned {
        tracing::debug!(
            reason = ?e.kind(),
            "cannot start the thread that passes on the hints of a change of network"
        );
    }
    notify
}

/// Follows the network until the daemon is gone: every change is told to
/// `changes`, as `wrap` makes of it. The task is gone with the handle.
pub fn spawn<C: Send + 'static>(
    backend: Backend,
    started: Started,
    changes: mpsc::Sender<C>,
    wrap: fn(NetworkFacts) -> C,
) -> AbortOnDropHandle<()> {
    let Started { hints, snapshot } = started;
    let again = move || look(backend.clone());
    AbortOnDropHandle::new(tokio::spawn(follow(hints, again, snapshot, changes, wrap)))
}

/// On a hint: reads the network, and tells `changes` when it is not what it
/// was told last (the facts, the DNS servers or the addresses of the interface
/// are different). `told` is the network the daemon began with.
///
/// A hint is answered by a read at once if none was made in the last
/// [`WINDOW`], else by one read when that time is over, however many hints
/// come meanwhile: a burst of hints costs two reads at most. So the first
/// change after a quiet time is told at once, and the ones that come after it
/// within the time are one, told when it is over and with the latest network,
/// as a network that does not settle is told of once a second and one that
/// does is told of when it has. A network that is back to what was told last
/// by then is no change.
async fn follow<C, A, F>(
    hints: Arc<Notify>,
    mut again: A,
    mut told: Snapshot,
    changes: mpsc::Sender<C>,
    wrap: impl Fn(NetworkFacts) -> C,
) where
    A: FnMut() -> F,
    F: Future<Output = Option<Snapshot>>,
{
    // no read is made before this, when one was made lately
    let mut rest: Option<Instant> = None;
    loop {
        wait_for_a_read(&hints, &mut rest).await;
        let seen = again().await;
        rest = Some(Instant::now() + WINDOW);
        let Some(snapshot) = seen else {
            continue;
        };
        if snapshot != told && !tell(&changes, &wrap, &mut told, snapshot).await {
            return;
        }
    }
}

/// Waits until a read of the network is due: a hint has come, and no read was
/// made in the last [`WINDOW`] (`rest` is when that is over). A hint that
/// comes in that time waits for its end; and so does every one after it, for
/// the read that follows answers them all.
async fn wait_for_a_read(hints: &Notify, rest: &mut Option<Instant>) {
    if let Some(end) = *rest {
        tokio::select! {
            () = hints.notified() => {
                tokio::time::sleep_until(end).await;
                *rest = None;
                // the hints that came while it waited are this one
                forget_the_stored_hint(hints).await;
                return;
            }
            () = tokio::time::sleep_until(end) => *rest = None,
        }
    }
    hints.notified().await;
}

/// Takes the hint that is stored, if there is one: a hint is a permit, and
/// the read that is about to be made answers it.
async fn forget_the_stored_hint(hints: &Notify) {
    tokio::select! {
        biased;
        () = hints.notified() => {}
        () = std::future::ready(()) => {}
    }
}

/// Tells `changes` that the network is `now`, which is what has been told
/// from here on. `false`: nobody listens any more.
async fn tell<C>(
    changes: &mpsc::Sender<C>,
    wrap: &impl Fn(NetworkFacts) -> C,
    told: &mut Snapshot,
    now: Snapshot,
) -> bool {
    let facts = facts_of(&now);
    // what differs, not what it is: the values say where the machine is
    tracing::debug!(
        facts = facts_of(told) != facts,
        dns_servers = told.dns != now.dns,
        addresses = told.addresses != now.addresses,
        "the network is not what it was"
    );
    *told = now;
    changes.send(wrap(facts)).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    /// The network as the platform layer says it: on Wi-Fi, joined to `ssid`.
    fn on_wifi(ssid: &str) -> Snapshot {
        Snapshot {
            primary: Some(netenv::Primary {
                interface: "wlan0".to_string(),
                kind: netenv::NetKind::Wifi,
                routers: vec![ip("192.0.2.1")],
            }),
            wifi: Some(netenv::Wifi {
                interface: "wlan0".to_string(),
                ssid: Some(ssid.to_string()),
                bssid: None,
            }),
            dns: vec![ip("192.0.2.53")],
            addresses: vec![ip("192.0.2.10")],
        }
    }

    /// What the engine is told of `on_wifi(ssid)`.
    fn told_of(ssid: &str) -> NetworkFacts {
        facts_of(&on_wifi(ssid))
    }

    /// Lets the tasks that can run, run.
    async fn settle() {
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }

    /// Time goes by, and what is due in it happens.
    async fn pass(time: Duration) {
        tokio::time::advance(time).await;
        settle().await;
    }

    /// The task, with a machine whose network the test changes and a platform
    /// whose hints the test gives.
    struct World {
        hints: Arc<Notify>,
        now: Arc<Mutex<Option<Snapshot>>>,
        told: mpsc::Receiver<NetworkFacts>,
        task: tokio::task::JoinHandle<()>,
    }

    impl World {
        /// The machine is on `first`, which is also what the daemon began with.
        async fn start(first: Snapshot) -> World {
            World::start_counting(first, Duration::ZERO).await.0
        }

        /// `start`, on a machine that takes `delay` to look at its network, and
        /// the number of looks it has had to make. What a look says is the
        /// network at the moment it begins.
        async fn start_counting(first: Snapshot, delay: Duration) -> (World, Arc<AtomicUsize>) {
            let hints = Arc::new(Notify::new());
            let now = Arc::new(Mutex::new(Some(first.clone())));
            let reads = Arc::new(AtomicUsize::new(0));
            let (tx, told) = mpsc::channel(16);
            let again = {
                let now = now.clone();
                let reads = reads.clone();
                move || {
                    reads.fetch_add(1, Ordering::SeqCst);
                    let seen = now.lock().unwrap().clone();
                    async move {
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        seen
                    }
                }
            };
            let task = tokio::spawn(follow(hints.clone(), again, first, tx, |facts| facts));
            settle().await;
            let world = World {
                hints,
                now,
                told,
                task,
            };
            (world, reads)
        }

        /// The network is `network` now (`None`: it cannot be read), and the
        /// platform says it may have changed.
        async fn change(&self, network: Option<Snapshot>) {
            *self.now.lock().unwrap() = network;
            self.hints.notify_one();
            settle().await;
        }

        /// What the engine has been told since the last call.
        fn told(&mut self) -> Vec<NetworkFacts> {
            let mut all = Vec::new();
            while let Ok(facts) = self.told.try_recv() {
                all.push(facts);
            }
            all
        }
    }

    /// The first change after a quiet time is told at once. The ones that
    /// follow within the second are one: the last, told when the second ends.
    #[tokio::test(start_paused = true)]
    async fn a_burst_of_changes_is_told_at_its_first_and_at_its_end() {
        let mut world = World::start(on_wifi("start")).await;
        world.change(Some(on_wifi("one"))).await;
        assert_eq!(world.told(), [told_of("one")]);

        pass(Duration::from_millis(300)).await;
        world.change(Some(on_wifi("two"))).await;
        pass(Duration::from_millis(300)).await;
        world.change(Some(on_wifi("three"))).await;
        assert_eq!(world.told(), []);

        pass(Duration::from_millis(399)).await;
        assert_eq!(world.told(), [], "the second is not over");
        pass(Duration::from_millis(1)).await;
        assert_eq!(
            world.told(),
            [told_of("three")],
            "the latest, not the one before"
        );

        // that is all of them
        pass(Duration::from_secs(10)).await;
        assert_eq!(world.told(), []);
    }

    /// A second after the last thing was told, the next change is the first of
    /// a new burst: told as it comes.
    #[tokio::test(start_paused = true)]
    async fn a_change_after_a_quiet_second_is_told_at_once() {
        let mut world = World::start(on_wifi("start")).await;
        world.change(Some(on_wifi("one"))).await;
        pass(Duration::from_secs(1)).await;
        let before = tokio::time::Instant::now();
        world.change(Some(on_wifi("two"))).await;
        assert_eq!(world.told(), [told_of("one"), told_of("two")]);
        assert_eq!(tokio::time::Instant::now(), before, "no time passed");

        // what was told at the end of a burst starts the next second
        pass(Duration::from_millis(100)).await;
        world.change(Some(on_wifi("three"))).await;
        pass(Duration::from_millis(100)).await;
        world.change(Some(on_wifi("four"))).await;
        pass(Duration::from_millis(800)).await;
        assert_eq!(world.told(), [told_of("four")]);
        world.change(Some(on_wifi("five"))).await;
        assert_eq!(world.told(), [], "a second has not gone by since the last");
        pass(Duration::from_secs(1)).await;
        assert_eq!(world.told(), [told_of("five")]);
    }

    /// A network that goes and comes back inside the second is no change by the
    /// end of it: the engine has just been told it is on it.
    #[tokio::test(start_paused = true)]
    async fn a_change_that_is_undone_inside_the_second_is_not_told_again() {
        let mut world = World::start(on_wifi("start")).await;
        world.change(Some(on_wifi("one"))).await;
        pass(Duration::from_millis(300)).await;
        world.change(Some(on_wifi("two"))).await;
        pass(Duration::from_millis(100)).await;
        world.change(Some(on_wifi("one"))).await;
        pass(Duration::from_secs(5)).await;
        assert_eq!(world.told(), [told_of("one")]);

        // while one that is not undone is told, whatever came between
        world.change(Some(on_wifi("two"))).await;
        pass(Duration::from_millis(100)).await;
        world.change(Some(on_wifi("three"))).await;
        pass(Duration::from_millis(100)).await;
        world.change(Some(on_wifi("two"))).await;
        pass(Duration::from_secs(5)).await;
        assert_eq!(world.told(), [told_of("two")]);
    }

    /// A hint is no change: the machine is read, and what it says is what the
    /// engine knows already.
    #[tokio::test(start_paused = true)]
    async fn a_hint_that_finds_the_network_as_it_was_tells_nothing() {
        let mut world = World::start(on_wifi("start")).await;
        for _ in 0..3 {
            world.change(Some(on_wifi("start"))).await;
        }
        pass(Duration::from_secs(5)).await;
        assert_eq!(world.told(), []);
    }

    /// The DNS servers and the addresses of the interface are part of the
    /// network: a change of those alone is told, though the engine is told
    /// the same thing it knew.
    #[tokio::test(start_paused = true)]
    async fn a_change_of_the_dns_servers_or_the_addresses_alone_is_told() {
        let mut world = World::start(on_wifi("start")).await;
        let mut moved_dns = on_wifi("start");
        moved_dns.dns = vec![ip("192.0.2.54")];
        world.change(Some(moved_dns.clone())).await;
        assert_eq!(world.told(), [told_of("start")]);

        pass(Duration::from_secs(2)).await;
        let mut moved_address = moved_dns;
        moved_address.addresses = vec![ip("192.0.2.11")];
        world.change(Some(moved_address)).await;
        assert_eq!(world.told(), [told_of("start")]);
    }

    /// A network that cannot be read is not a network that is not known: the
    /// groups would all fall to their defaults for the time of a failure.
    #[tokio::test(start_paused = true)]
    async fn a_look_that_fails_tells_nothing() {
        let mut world = World::start(on_wifi("start")).await;
        world.change(None).await;
        pass(Duration::from_secs(5)).await;
        assert_eq!(world.told(), []);
        // the next one that works is read as usual
        world.change(Some(on_wifi("one"))).await;
        assert_eq!(world.told(), [told_of("one")]);
    }

    /// A read of the network is a program or two, and the platform may say it
    /// has changed many times a second: reads are no closer together than a
    /// second. A burst of hints costs the read that answers the first of them
    /// and one when the second is over, which has whatever came meanwhile.
    #[tokio::test(start_paused = true)]
    async fn a_burst_of_hints_costs_two_reads() {
        let (world, reads) = World::start_counting(on_wifi("start"), Duration::ZERO).await;
        let reads = || reads.load(Ordering::SeqCst);
        // five hints in the first 600 milliseconds, and the network as it was
        for _ in 0..5 {
            world.change(Some(on_wifi("start"))).await;
            pass(Duration::from_millis(150)).await;
        }
        assert_eq!(reads(), 1, "the first is answered at once, the rest wait");
        pass(Duration::from_millis(249)).await;
        assert_eq!(reads(), 1, "the second is not over");
        pass(Duration::from_millis(1)).await;
        assert_eq!(reads(), 2, "one read answers the rest");
        // nothing more was asked for
        pass(Duration::from_secs(10)).await;
        assert_eq!(reads(), 2);

        // a hint after a quiet time is answered at once
        let before = tokio::time::Instant::now();
        world.change(Some(on_wifi("start"))).await;
        assert_eq!(reads(), 3);
        assert_eq!(tokio::time::Instant::now(), before, "no time passed");
    }

    /// A hint that comes while the network is being read may be about a change
    /// that the read did not see: it is answered by a read when the second is
    /// over, and not lost.
    #[tokio::test(start_paused = true)]
    async fn a_hint_that_comes_during_a_read_is_answered_when_the_second_is_over() {
        let (mut world, reads) =
            World::start_counting(on_wifi("start"), Duration::from_millis(400)).await;
        let reads = || reads.load(Ordering::SeqCst);
        // the read begins at once and sees `one`; it ends at 400 ms
        world.change(Some(on_wifi("one"))).await;
        assert_eq!(reads(), 1);
        pass(Duration::from_millis(200)).await;
        world.change(Some(on_wifi("two"))).await;
        pass(Duration::from_millis(200)).await;
        assert_eq!(world.told(), [told_of("one")]);

        // a second after that read: the read that has `two`
        pass(Duration::from_millis(999)).await;
        assert_eq!(reads(), 1, "the second is not over");
        pass(Duration::from_millis(1)).await;
        assert_eq!(reads(), 2);
        pass(Duration::from_millis(400)).await;
        assert_eq!(world.told(), [told_of("two")]);
    }

    /// The task ends when the daemon no longer listens, rather than go on
    /// reading a network nobody is told of.
    #[tokio::test(start_paused = true)]
    async fn the_task_ends_when_nobody_is_told_any_more() {
        let World {
            hints,
            now,
            told,
            task,
        } = World::start(on_wifi("start")).await;
        drop(told);
        *now.lock().unwrap() = Some(on_wifi("one"));
        hints.notify_one();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("the task ends")
            .unwrap();
    }

    #[test]
    fn what_the_platform_says_is_what_the_engine_looks_at() {
        assert_eq!(facts_of(&Snapshot::default()), NetworkFacts::default());

        let facts = facts_of(&on_wifi("Home"));
        assert_eq!(
            facts.primary,
            Some(Primary {
                interface: "wlan0".to_string(),
                kind: NetKind::Wifi,
                routers: vec![ip("192.0.2.1")],
            })
        );
        assert_eq!(
            facts.wifi,
            Some(WifiLink {
                interface: "wlan0".to_string(),
                ssid: Some("Home".to_string()),
                bssid: None,
            })
        );

        for (said, meant) in [
            (netenv::NetKind::Wifi, NetKind::Wifi),
            (netenv::NetKind::Wired, NetKind::Wired),
            (netenv::NetKind::Other, NetKind::Other),
        ] {
            let mut snapshot = on_wifi("Home");
            snapshot.primary.as_mut().unwrap().kind = said;
            assert_eq!(facts_of(&snapshot).primary.unwrap().kind, meant);
        }
        // a Wi-Fi link whose name the platform would not say is a link without one
        let mut hidden = on_wifi("Home");
        hidden.wifi.as_mut().unwrap().ssid = None;
        assert_eq!(facts_of(&hidden).wifi.unwrap().ssid, None);
    }

    /// A hint of the platform reaches the task, and the thread that brings it
    /// ends once the task is gone.
    #[tokio::test]
    async fn the_hints_of_the_platform_reach_the_task() {
        let (hint, hints) = std::sync::mpsc::channel();
        let arrived = forward_hints(hints);
        hint.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), arrived.notified())
            .await
            .expect("a hint arrives");

        // with nobody to tell, the next hint ends the thread: the channel of
        // the platform has no receiver any more
        drop(arrived);
        hint.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while hint.send(()).is_ok() {
            assert!(
                std::time::Instant::now() < deadline,
                "the thread is still there"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The option names a file, and a value that names nothing is refused in
    /// the words of the variable, whichever way it came.
    #[test]
    fn the_option_names_a_file_and_an_unknown_value_is_refused_as_the_variables_is() {
        assert_eq!(
            backend(Some("file:/tmp/n.json")).unwrap(),
            Backend::File("/tmp/n.json".into())
        );
        for value in ["bogus", "", "file", "file:", "FILE:/x.json", "platform"] {
            assert_eq!(
                backend(Some(value)).unwrap_err().to_string(),
                format!("unknown RURGE_NETWORK_BACKEND value `{value}` (expected file:<path>)"),
                "{value:?}"
            );
        }
    }

    /// The first look is of the network the backend names, and a file that
    /// says nothing makes the engine know nothing.
    #[tokio::test]
    async fn the_first_look_is_of_the_network_the_backend_names() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("network.json");
        let backend = Backend::File(file.clone());
        assert_eq!(start(&backend).await.snapshot, Snapshot::default());
        assert_eq!(read_once(&backend).await, NetworkFacts::default());

        std::fs::write(&file, serde_json::to_string(&on_wifi("Home")).unwrap()).unwrap();
        assert_eq!(read_once(&backend).await, told_of("Home"));
        assert_eq!(start(&backend).await.snapshot, on_wifi("Home"));
    }
}
