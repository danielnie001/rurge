//! The thread that reads the adapter's packets and passes them on, with no
//! platform in it (M4 design 8.4).
//!
//! The ring is a Wintun session's: [`Ring::receive`] takes the packet that is
//! waiting, [`Ring::wait`] blocks on the session's read event. Both block, so
//! they run on a thread of their own, and a bounded channel carries the
//! packets over to the asynchronous side: a full channel makes the reader
//! wait, and an empty ring makes it wait on the event, looking at the stop
//! flag again after every wait.
//!
//! The loop is not in a tokio runtime — it is the thread's whole work — so it
//! sends with [`tokio::sync::mpsc::Sender::blocking_send`], which panics in
//! one. It ends when `stop` is set (within one wait of it), when the receiving
//! half of the channel is closed, or after passing on an error of the ring.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;

/// How long a read waits on the ring before it looks at the stop flag again
/// (M4 design 8.4): short enough to close promptly, long enough not to wake
/// up for nothing.
pub const READ_WAIT: Duration = Duration::from_millis(100);

/// What the reader needs of a Wintun session; a fake one has the same two
/// calls in the tests.
pub trait Ring {
    /// The packet that is waiting: `Ok(None)` when there is none now.
    fn receive(&self) -> io::Result<Option<Vec<u8>>>;
    /// Waits at most `timeout` for a packet to arrive.
    fn wait(&self, timeout: Duration);
}

/// The reader thread (M4 design 8.4): passes the packets on in order through a
/// bounded channel, waiting when it is full; waits on the ring when it is
/// empty. Ends when `stop` is set (within one wait), when the receiving half
/// is closed, or after passing on an error of the ring.
pub fn read_loop(
    ring: &dyn Ring,
    tx: &mpsc::Sender<io::Result<Vec<u8>>>,
    stop: &AtomicBool,
    wait: Duration,
) {
    while !stop.load(Ordering::Acquire) {
        match ring.receive() {
            Ok(Some(packet)) => {
                // a full channel holds the thread here; closing the receiving
                // half is what wakes it (M4d plan P13)
                if tx.blocking_send(Ok(packet)).is_err() {
                    return;
                }
            }
            Ok(None) => ring.wait(wait),
            Err(error) => {
                // the adapter is broken: the asynchronous side is told, and the
                // reader is over; a receiving half that is already gone ends
                // it the same way
                let _ = tx.blocking_send(Err(error));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::mpsc::RecvTimeoutError;
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread;
    use std::time::Instant;
    use tokio::sync::mpsc::error::TryRecvError;

    /// Every wait these tests make: ten seconds, far above the loop's own wait
    /// and the one-millisecond polls below, so a loaded machine does not fail
    /// an assertion by being slow.
    const TIMEOUT: Duration = Duration::from_secs(10);

    /// What the fake ring gives, in order: a packet, or the error the ring
    /// answers with.
    enum Next {
        Packet(Vec<u8>),
        Error(io::ErrorKind),
    }

    /// A ring the test fills and looks at: what is waiting for the loop, how
    /// much the loop has taken, how often it has waited.
    struct Fake {
        inner: Mutex<Inner>,
        arrived: Condvar,
    }

    #[derive(Default)]
    struct Inner {
        next: VecDeque<Next>,
        received: usize,
        waits: usize,
    }

    impl Fake {
        fn of(next: impl IntoIterator<Item = Next>) -> Fake {
            Fake {
                inner: Mutex::new(Inner {
                    next: next.into_iter().collect(),
                    ..Inner::default()
                }),
                arrived: Condvar::new(),
            }
        }

        fn of_packets(packets: &[&[u8]]) -> Fake {
            Fake::of(
                packets
                    .iter()
                    .map(|packet| Next::Packet(packet.to_vec()))
                    .collect::<Vec<_>>(),
            )
        }

        /// Puts a packet in the ring, and wakes a wait that is on it: what the
        /// session's read event does.
        fn push(&self, packet: &[u8]) {
            self.lock().next.push_back(Next::Packet(packet.to_vec()));
            self.arrived.notify_all();
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
            self.inner.lock().unwrap()
        }

        /// Whether the loop has waited on the ring at least `count` times,
        /// within `timeout`.
        fn waited_within(&self, count: usize, timeout: Duration) -> bool {
            self.until(timeout, |inner| inner.waits >= count)
        }

        /// Whether the loop has taken at least `count` packets from the ring
        /// within `timeout`.
        fn received_within(&self, count: usize, timeout: Duration) -> bool {
            self.until(timeout, |inner| inner.received >= count)
        }

        /// Waits on the ring's own condition variable until `done` holds, and
        /// gives `false` once `timeout` has passed.
        fn until(&self, timeout: Duration, done: impl Fn(&Inner) -> bool) -> bool {
            let deadline = Instant::now() + timeout;
            let mut inner = self.lock();
            loop {
                if done(&inner) {
                    return true;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return false;
                }
                let (guard, _) = self.arrived.wait_timeout(inner, left).unwrap();
                inner = guard;
            }
        }
    }

    impl Ring for Fake {
        fn receive(&self) -> io::Result<Option<Vec<u8>>> {
            let mut inner = self.lock();
            let answer = match inner.next.pop_front() {
                Some(Next::Packet(packet)) => Ok(Some(packet)),
                Some(Next::Error(kind)) => Err(io::Error::from(kind)),
                None => return Ok(None),
            };
            inner.received += 1;
            // a test waiting for the loop to have taken this one has to wake
            // up now, not when its deadline passes
            self.arrived.notify_all();
            answer
        }

        fn wait(&self, timeout: Duration) {
            let mut inner = self.lock();
            inner.waits += 1;
            self.arrived.notify_all();
            // back as soon as a packet is there, and after `timeout` at the
            // latest: a session's read event is what this stands in for
            let deadline = Instant::now() + timeout;
            while inner.next.is_empty() {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return;
                }
                let (guard, _) = self.arrived.wait_timeout(inner, left).unwrap();
                inner = guard;
            }
        }
    }

    /// One reading of the loop: the thread it runs on, and the signal it sends
    /// as its last act, so that the test waits for the end on its own clock
    /// with a deadline.
    struct Reader {
        stop: Arc<AtomicBool>,
        ended: std::sync::mpsc::Receiver<()>,
    }

    fn start(ring: Arc<Fake>, capacity: usize) -> (Reader, mpsc::Receiver<io::Result<Vec<u8>>>) {
        let (tx, rx) = mpsc::channel(capacity);
        let stop = Arc::new(AtomicBool::new(false));
        let (ended, ended_rx) = std::sync::mpsc::channel();
        let thread_stop = Arc::clone(&stop);
        thread::spawn(move || {
            read_loop(&*ring, &tx, &thread_stop, READ_WAIT);
            let _ = ended.send(());
        });
        (
            Reader {
                stop,
                ended: ended_rx,
            },
            rx,
        )
    }

    impl Reader {
        /// Waits for the loop's thread to end, within `timeout`.
        fn ends_within(&self, timeout: Duration) {
            self.ended
                .recv_timeout(timeout)
                .unwrap_or_else(|_| panic!("the loop did not end within {timeout:?}"));
        }

        /// Whether the loop's thread is still running after `timeout`: the one
        /// fixed wait these tests make, for asserting that nothing happened. A
        /// thread that ended, or that panicked before its last act, is not
        /// still running.
        fn still_running_after(&self, timeout: Duration) -> bool {
            self.ended.recv_timeout(timeout) == Err(RecvTimeoutError::Timeout)
        }

        fn stop(&self) {
            self.stop.store(true, Ordering::Release);
        }
    }

    /// The next item the loop sent, within `timeout`: the receiving half is
    /// asked every millisecond rather than waited on, because waiting on it
    /// has no deadline of its own.
    fn next_within(
        rx: &mut mpsc::Receiver<io::Result<Vec<u8>>>,
        timeout: Duration,
    ) -> Option<io::Result<Vec<u8>>> {
        let deadline = Instant::now() + timeout;
        loop {
            match rx.try_recv() {
                Ok(item) => return Some(item),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return None,
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Everything the ring has is passed on, in the order the ring gave it.
    #[test]
    fn packets_are_passed_on_in_order() {
        let ring = Arc::new(Fake::of_packets(&[b"one", b"two", b"three"]));
        let (reader, mut rx) = start(Arc::clone(&ring), 1024);

        for expected in [&b"one"[..], &b"two"[..], &b"three"[..]] {
            let packet = next_within(&mut rx, TIMEOUT).expect("a packet the ring had");
            assert_eq!(packet.expect("no error from the ring"), expected.to_vec());
        }

        reader.stop();
        reader.ends_within(TIMEOUT);
    }

    /// An empty ring is waited on, and a packet that arrives after the wait is
    /// passed on.
    #[test]
    fn an_empty_ring_is_waited_on() {
        let ring = Arc::new(Fake::of([]));
        let (reader, mut rx) = start(Arc::clone(&ring), 1024);

        assert!(
            ring.waited_within(1, TIMEOUT),
            "the loop waits on a ring with nothing in it"
        );
        ring.push(b"late");
        let packet = next_within(&mut rx, TIMEOUT).expect("the packet pushed after the wait");
        assert_eq!(packet.expect("no error from the ring"), b"late".to_vec());

        reader.stop();
        reader.ends_within(TIMEOUT);
    }

    /// An error of the ring is passed on, after the packets before it, and the
    /// loop ends by itself: nothing sets the stop flag.
    #[test]
    fn an_error_of_the_ring_is_passed_on_and_ends_the_loop() {
        let ring = Arc::new(Fake::of([
            Next::Packet(b"one".to_vec()),
            Next::Error(io::ErrorKind::BrokenPipe),
            Next::Packet(b"after the error".to_vec()),
        ]));
        let (reader, mut rx) = start(Arc::clone(&ring), 1024);

        let packet = next_within(&mut rx, TIMEOUT).expect("the packet before the error");
        assert_eq!(packet.expect("no error from the ring"), b"one".to_vec());

        let error = next_within(&mut rx, TIMEOUT)
            .expect("the error the ring gave")
            .expect_err("the ring's error is passed on as one");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

        reader.ends_within(TIMEOUT);
        assert!(
            !reader.stop.load(Ordering::Acquire),
            "the loop ends without being stopped"
        );
    }

    /// The stop flag ends the loop within one wait on an empty ring. The two
    /// seconds are one wait plus a margin for a loaded machine to run the
    /// thread, far below the ten seconds used where nothing is timed.
    #[test]
    fn stop_ends_the_loop_within_one_wait() {
        let ring = Arc::new(Fake::of([]));
        let (reader, _rx) = start(Arc::clone(&ring), 1024);

        reader.stop();
        reader.ends_within(READ_WAIT + Duration::from_secs(2));
    }

    /// Closing the receiving half wakes a loop that is waiting for room in a
    /// full channel, and ends it (M4d plan P13).
    #[test]
    fn a_closed_receiver_ends_a_loop_blocked_on_a_full_channel() {
        let ring = Arc::new(Fake::of_packets(&[b"one", b"two", b"three"]));
        // room for one: the first packet fills the channel, the second send
        // has to wait
        let (reader, mut rx) = start(Arc::clone(&ring), 1);

        assert!(
            ring.received_within(2, TIMEOUT),
            "the loop takes a second packet while the channel is full"
        );
        // The one fixed wait in these tests, and it asserts that nothing
        // happened: the loop took the second packet and is now held in
        // `blocking_send` for room, so it must not end before the channel is
        // closed. Two hundred milliseconds are orders of magnitude above what
        // that one call takes; waiting too little can only let an early end go
        // unseen, never fail a correct loop.
        assert!(
            reader.still_running_after(Duration::from_millis(200)),
            "the loop ends on its own while the channel is full"
        );
        rx.close();

        reader.ends_within(TIMEOUT);
    }
}
