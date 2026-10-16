//! Hints that the network may have changed: a thread that looks at the cheap
//! sources and says so on a channel. A hint is no change — the reader of the
//! channel reads the network and compares —, so a hint too many costs a read
//! and a hint too few costs time until the next.

use super::host::say_once;
use super::{Backend, Quick, ReadOpts, read_quick};
use crate::dirs::Os;
use socket2::{Domain, Socket, Type};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often the file of the test backend is looked at.
const FILE_POLL: Duration = Duration::from_millis(100);
/// How often the cheap sources are looked at on Windows and Linux.
const QUICK_POLL: Duration = Duration::from_secs(2);
/// How often the Wi-Fi link is looked at though nothing else has changed
/// (another access point of the same network); macOS looks at everything this
/// often, in case a message of the kernel was missed.
const SLOW_POLL: Duration = Duration::from_secs(30);
/// macOS: how long the kernel must have been quiet after a message that
/// matters before it is a hint, and the longest a hint is held back while it
/// is not quiet.
const SETTLE: Duration = Duration::from_millis(300);
const HOLD_BACK: Duration = Duration::from_secs(1);

/// A hint every time the network may have changed. The thread that sends them
/// ends when the receiver is dropped — it finds out at its next hint.
///
/// What a hint stands for is taken from the moment this is called, so call it
/// before the first [`read`](super::read): a change between the two is then a
/// hint, not lost.
///
/// - the test backend: the file is not what it was (looked at 10 times a
///   second);
/// - Windows and Linux: [`read_quick`](super::read_quick) is not what it was
///   (every 2 seconds), and, while a wireless interface is up, every 30
///   seconds, for the Wi-Fi link is not in it;
/// - macOS: the kernel says that an address, an interface or a route has
///   changed, a burst of such messages being one hint, and every 30 seconds.
///
/// The cheap look leaves out nothing ([`ReadOpts::default`]): a hint for an
/// interface that [`read`](super::read) is asked to leave out is a read that
/// finds no change.
pub fn hints(backend: &Backend) -> mpsc::Receiver<()> {
    spawn(backend).0
}

/// What the thread looks at, with what it saw when it began.
enum Watcher {
    File {
        path: PathBuf,
        last: Option<Vec<u8>>,
    },
    Polling(Poll),
    /// `None`: no routing socket could be opened, and only the clock is left.
    Routes(Option<Socket>),
}

/// [`hints`], and the thread, which a test waits for.
fn spawn(backend: &Backend) -> (mpsc::Receiver<()>, Option<JoinHandle<()>>) {
    let (tx, rx) = mpsc::channel();
    let watcher = match backend {
        Backend::File(path) => Watcher::File {
            last: std::fs::read(path).ok(),
            path: path.clone(),
        },
        Backend::Platform if Os::current() == Os::MacOs => Watcher::Routes(open_route_socket()),
        Backend::Platform => Watcher::Polling(Poll::new(
            read_quick(&Backend::Platform, &ReadOpts::default()),
            Instant::now(),
        )),
    };
    let thread = std::thread::Builder::new()
        .name("rurge-network-hints".to_string())
        .spawn(move || match watcher {
            Watcher::File { path, last } => watch_file(&path, last, &tx),
            Watcher::Polling(poll) => watch_polling(poll, &tx),
            Watcher::Routes(socket) => watch_routes(socket.as_ref(), &tx),
        });
    match thread {
        Ok(thread) => (rx, Some(thread)),
        Err(e) => {
            say_once("hints-thread", || {
                tracing::debug!(reason = ?e.kind(), "cannot start the thread that watches the network");
            });
            // the sender went with the closure: the receiver says so
            (rx, None)
        }
    }
}

fn watch_file(path: &Path, mut last: Option<Vec<u8>>, tx: &mpsc::Sender<()>) {
    loop {
        std::thread::sleep(FILE_POLL);
        let now = std::fs::read(path).ok();
        if now != last {
            last = now;
            if tx.send(()).is_err() {
                return;
            }
        }
    }
}

fn watch_polling(mut poll: Poll, tx: &mpsc::Sender<()>) {
    let opts = ReadOpts::default();
    loop {
        std::thread::sleep(QUICK_POLL);
        let quick = read_quick(&Backend::Platform, &opts);
        if poll.looked(Instant::now(), quick) && tx.send(()).is_err() {
            return;
        }
    }
}

/// `AF_ROUTE`, the family of the routing socket, from macOS's `<sys/socket.h>`
/// (the value the `libc` crate has for it).
const AF_ROUTE: i32 = 17;

/// A socket on which the kernel says every time an address, an interface or a
/// route changes; `None` when there is no such thing here.
fn open_route_socket() -> Option<Socket> {
    match Socket::new(Domain::from(AF_ROUTE), Type::RAW, None) {
        Ok(socket) => Some(socket),
        Err(e) => {
            say_once("route-socket", || {
                tracing::debug!(
                    reason = ?e.kind(),
                    "cannot open a routing socket; the network is looked at every 30 seconds"
                );
            });
            None
        }
    }
}

fn watch_routes(socket: Option<&Socket>, tx: &mpsc::Sender<()>) {
    let mut burst = Burst::default();
    let mut slow_at = Instant::now() + SLOW_POLL;
    let mut message = [0u8; 2048];
    loop {
        let now = Instant::now();
        let until_slow = slow_at.saturating_duration_since(now);
        let wait = burst
            .wait(now)
            .map_or(until_slow, |until| until.min(until_slow));
        // a timeout of zero is no timeout, and a wait for it is none
        let wait = wait.max(Duration::from_millis(1));
        let read = match socket {
            Some(mut socket) => socket
                .set_read_timeout(Some(wait))
                .and_then(|()| socket.read(&mut message)),
            None => {
                std::thread::sleep(wait);
                Err(std::io::ErrorKind::TimedOut.into())
            }
        };
        match read {
            Ok(length) => {
                if route_message_matters(&message[..length]) {
                    burst.message(Instant::now());
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(e) => {
                say_once("route-socket-read", || {
                    tracing::debug!(reason = ?e.kind(), "cannot read the routing socket");
                });
                // not a loop that spins on a socket that is gone: the clock is left
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let now = Instant::now();
        if burst.due(now) {
            burst.clear();
            if tx.send(()).is_err() {
                return;
            }
        }
        if now >= slow_at {
            slow_at = now + SLOW_POLL;
            if tx.send(()).is_err() {
                return;
            }
        }
    }
}

// The messages of the routing socket, as macOS's `<net/route.h>` has them
// (the values the `libc` crate has for them): the type, in the fourth byte of
// the head, and the flags, a C `int` at the ninth byte, in the byte order of
// the machine.
const RTM_ADD: u8 = 0x1;
const RTM_DELETE: u8 = 0x2;
const RTM_CHANGE: u8 = 0x3;
const RTM_NEWADDR: u8 = 0xc;
const RTM_DELADDR: u8 = 0xd;
const RTM_IFINFO: u8 = 0xe;
const RTF_LLINFO: i32 = 0x400;
const RTF_WASCLONED: i32 = 0x20000;

/// Whether a message of the macOS routing socket is a change worth a hint: an
/// address added or removed, an interface that changed state, a route added,
/// deleted or changed — except the routes the kernel makes for single hosts it
/// has talked to (ARP and neighbour discovery: `RTF_LLINFO`, `RTF_WASCLONED`),
/// which come and go all the time.
pub(crate) fn route_message_matters(message: &[u8]) -> bool {
    // a head that stops before the flags is no message
    let (Some(&kind), Some(flags)) = (message.get(3), message.get(8..12)) else {
        return false;
    };
    match kind {
        RTM_NEWADDR | RTM_DELADDR | RTM_IFINFO => true,
        RTM_ADD | RTM_DELETE | RTM_CHANGE => {
            let Ok(flags) = <[u8; 4]>::try_from(flags) else {
                return false;
            };
            i32::from_ne_bytes(flags) & (RTF_LLINFO | RTF_WASCLONED) == 0
        }
        _ => false,
    }
}

/// What the polling watcher of Windows and Linux makes of one more look.
struct Poll {
    last: Quick,
    /// When the Wi-Fi link was last a reason to look.
    asked: Instant,
}

impl Poll {
    fn new(first: Quick, now: Instant) -> Poll {
        Poll {
            last: first,
            asked: now,
        }
    }

    /// Whether this look is a hint: the sources say something different from
    /// the look before, or a wireless interface is up and the Wi-Fi link has
    /// not been asked about for 30 seconds.
    fn looked(&mut self, now: Instant, quick: Quick) -> bool {
        let changed = quick != self.last;
        let wireless = !quick.wireless.is_empty();
        self.last = quick;
        let hint = changed || (wireless && now.saturating_duration_since(self.asked) >= SLOW_POLL);
        if hint {
            self.asked = now;
        }
        hint
    }
}

/// The messages of the routing socket that matter, a burst of them being one
/// hint.
#[derive(Default)]
struct Burst {
    /// The time of the first and of the last message since the last hint.
    pending: Option<(Instant, Instant)>,
}

impl Burst {
    fn message(&mut self, now: Instant) {
        let first = self.pending.map_or(now, |(first, _)| first);
        self.pending = Some((first, now));
    }

    /// How long until the burst is over: it has been quiet for [`SETTLE`], or
    /// it has gone on for [`HOLD_BACK`]. `None` when no message waits.
    fn wait(&self, now: Instant) -> Option<Duration> {
        self.pending.map(|(first, last)| {
            (last + SETTLE)
                .min(first + HOLD_BACK)
                .saturating_duration_since(now)
        })
    }

    fn due(&self, now: Instant) -> bool {
        self.wait(now).is_some_and(|wait| wait.is_zero())
    }

    fn clear(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::RecvTimeoutError;

    const RTM_ADD: u8 = 0x1;
    const RTM_DELETE: u8 = 0x2;
    const RTM_CHANGE: u8 = 0x3;
    const RTM_GET: u8 = 0x4;
    const RTM_MISS: u8 = 0x7;
    const RTM_NEWADDR: u8 = 0xc;
    const RTM_DELADDR: u8 = 0xd;
    const RTM_IFINFO: u8 = 0xe;
    const RTF_UP: i32 = 0x1;
    const RTF_GATEWAY: i32 = 0x2;
    const RTF_LLINFO: i32 = 0x400;
    const RTF_WASCLONED: i32 = 0x20000;

    /// The head of a message of the routing socket the way `<net/route.h>`
    /// lays it out: the length, the version (5), the type, the index of the
    /// interface, two bytes of padding and the flags, in the byte order of the
    /// machine; then as much of the rest as a test wants.
    fn message(kind: u8, flags: i32, length: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(length as u16).to_ne_bytes());
        bytes.extend_from_slice(&[5, kind]);
        bytes.extend_from_slice(&7u16.to_ne_bytes());
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&flags.to_ne_bytes());
        bytes.resize(length, 0);
        bytes
    }

    #[test]
    fn addresses_and_interfaces_that_change_matter() {
        for kind in [RTM_NEWADDR, RTM_DELADDR, RTM_IFINFO] {
            assert!(route_message_matters(&message(kind, 0, 20)), "{kind:#x}");
            // the flags of the head of these messages are something else
            assert!(
                route_message_matters(&message(kind, RTF_LLINFO | RTF_WASCLONED, 20)),
                "{kind:#x}"
            );
        }
    }

    #[test]
    fn routes_that_come_and_go_matter_except_the_ones_of_single_hosts() {
        let default_route = RTF_UP | RTF_GATEWAY;
        for kind in [RTM_ADD, RTM_DELETE, RTM_CHANGE] {
            assert!(
                route_message_matters(&message(kind, default_route, 92)),
                "{kind:#x}"
            );
            assert!(
                !route_message_matters(&message(kind, default_route | RTF_LLINFO, 92)),
                "{kind:#x}"
            );
            assert!(
                !route_message_matters(&message(kind, RTF_WASCLONED, 92)),
                "{kind:#x}"
            );
            assert!(
                !route_message_matters(&message(kind, RTF_LLINFO | RTF_WASCLONED, 92)),
                "{kind:#x}"
            );
        }
    }

    #[test]
    fn other_messages_and_truncated_ones_do_not() {
        // the kernel's answer to a request, and a lookup that found nothing
        for kind in [RTM_GET, RTM_MISS, 0, 0x10, 0xff] {
            assert!(!route_message_matters(&message(kind, 0, 92)), "{kind:#x}");
        }
        // a head that stops before the flags says nothing
        let whole = message(RTM_NEWADDR, 0, 20);
        for length in 0..12 {
            assert!(!route_message_matters(&whole[..length]), "{length}");
        }
        assert!(route_message_matters(&whole[..12]));
    }

    fn quick(wireless: &[&str], dns: &str) -> Quick {
        Quick {
            dns: vec![dns.parse().unwrap()],
            wireless: wireless.iter().map(|name| name.to_string()).collect(),
            ..Quick::default()
        }
    }

    fn after(start: Instant, seconds: u64) -> Instant {
        start + Duration::from_secs(seconds)
    }

    #[test]
    fn a_change_of_the_cheap_sources_is_a_hint_and_a_quiet_look_is_not() {
        let start = Instant::now();
        let mut poll = Poll::new(quick(&[], "192.0.2.53"), start);
        assert!(!poll.looked(after(start, 2), quick(&[], "192.0.2.53")));
        assert!(poll.looked(after(start, 4), quick(&[], "192.0.2.54")));
        // the change is the new normal
        assert!(!poll.looked(after(start, 6), quick(&[], "192.0.2.54")));
        // without a wireless interface nothing is a reason to look again
        assert!(!poll.looked(after(start, 3600), quick(&[], "192.0.2.54")));
        // and the change back is one
        assert!(poll.looked(after(start, 3602), quick(&[], "192.0.2.53")));
    }

    #[test]
    fn while_a_wireless_interface_is_up_the_wifi_link_is_looked_at_every_thirty_seconds() {
        let start = Instant::now();
        let wireless = || quick(&["wlan0"], "192.0.2.53");
        let mut poll = Poll::new(wireless(), start);
        for second in (2..30).step_by(2) {
            assert!(!poll.looked(after(start, second), wireless()), "{second}");
        }
        assert!(poll.looked(after(start, 30), wireless()));
        // the next thirty seconds count from there
        assert!(!poll.looked(after(start, 58), wireless()));
        assert!(poll.looked(after(start, 60), wireless()));
        // a change is a look at everything, and starts the thirty seconds again
        assert!(poll.looked(after(start, 70), quick(&["wlan0"], "192.0.2.54")));
        assert!(!poll.looked(after(start, 98), quick(&["wlan0"], "192.0.2.54")));
        assert!(poll.looked(after(start, 100), quick(&["wlan0"], "192.0.2.54")));
        // the wireless interface goes: no more looks
        assert!(poll.looked(after(start, 102), quick(&[], "192.0.2.54")));
        assert!(!poll.looked(after(start, 500), quick(&[], "192.0.2.54")));
    }

    fn millis(start: Instant, milliseconds: u64) -> Instant {
        start + Duration::from_millis(milliseconds)
    }

    #[test]
    fn a_burst_of_messages_is_one_hint_after_it_has_gone_quiet() {
        let start = Instant::now();
        let mut burst = Burst::default();
        assert_eq!(burst.wait(start), None);
        assert!(!burst.due(start));
        burst.message(start);
        assert_eq!(burst.wait(start), Some(SETTLE));
        assert!(!burst.due(millis(start, 299)));
        // another message: the quiet is counted from it
        burst.message(millis(start, 200));
        assert!(!burst.due(millis(start, 499)));
        assert_eq!(
            burst.wait(millis(start, 400)),
            Some(Duration::from_millis(100))
        );
        assert!(burst.due(millis(start, 500)));
        assert!(burst.due(millis(start, 5000)));
        burst.clear();
        assert!(!burst.due(millis(start, 5000)));
        assert_eq!(burst.wait(millis(start, 5000)), None);
    }

    /// A kernel that does not stop talking is still heard, once a second.
    #[test]
    fn a_burst_that_does_not_end_is_a_hint_after_a_second() {
        let start = Instant::now();
        let mut burst = Burst::default();
        for at in (0..HOLD_BACK.as_millis() as u64).step_by(100) {
            burst.message(millis(start, at));
            assert!(!burst.due(millis(start, at)), "{at}");
        }
        burst.message(millis(start, 950));
        assert!(!burst.due(millis(start, 999)));
        assert!(burst.due(millis(start, 1000)));
    }

    #[test]
    fn the_file_backend_hints_when_the_file_changes_and_not_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("network.json");
        std::fs::write(&path, "{}").unwrap();
        let (hints, _thread) = spawn(&Backend::File(path.clone()));
        // waits for four looks of the watcher, 100 ms apart, and expects no hint
        assert_eq!(
            hints.recv_timeout(Duration::from_millis(450)),
            Err(RecvTimeoutError::Timeout)
        );
        // a hint for a rewrite — and, when the watcher looked between the
        // truncation and the write, for the emptiness before it — and, once
        // it is quiet again, no more
        std::fs::write(&path, r#"{"dns": ["192.0.2.53"]}"#).unwrap();
        hints
            .recv_timeout(Duration::from_secs(10))
            .expect("a hint after the file was rewritten");
        while hints.recv_timeout(Duration::from_millis(450)).is_ok() {}
        // the file removed is a change; a file made is another
        std::fs::remove_file(&path).unwrap();
        hints
            .recv_timeout(Duration::from_secs(10))
            .expect("a hint after the file was removed");
        while hints.recv_timeout(Duration::from_millis(450)).is_ok() {}
        std::fs::write(&path, "{}").unwrap();
        hints
            .recv_timeout(Duration::from_secs(10))
            .expect("a hint after the file was made");
    }

    /// A file that is not there when the hints begin is a file that was made.
    #[test]
    fn the_file_backend_hints_for_a_file_that_was_not_there() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("network.json");
        let (hints, _thread) = spawn(&Backend::File(path.clone()));
        std::fs::write(&path, "{}").unwrap();
        hints
            .recv_timeout(Duration::from_secs(10))
            .expect("a hint after the file was made");
    }

    /// With nobody to tell, the thread ends at its next hint.
    #[test]
    fn the_thread_ends_when_the_receiver_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("network.json");
        std::fs::write(&path, "{}").unwrap();
        let (hints, thread) = spawn(&Backend::File(path.clone()));
        let thread = thread.expect("the thread started");
        drop(hints);
        std::fs::write(&path, r#"{"dns": ["192.0.2.53"]}"#).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !thread.is_finished() {
            assert!(Instant::now() < deadline, "the thread is still there");
            std::thread::sleep(Duration::from_millis(20));
        }
        thread.join().unwrap();
    }
}
