//! TCP through the TUN (phase 3 M3 design 5): the shards that advance the
//! userspace stack, the streams the sessions read and write, and the budget
//! their buffers come out of.
//!
//! A connection is known by its [`ConnId`], never by its smoltcp
//! `SocketHandle`: smoltcp 0.12 hands out the lowest free handle again, with
//! no generation, so a handle kept past its socket names another connection
//! (or none, and `get` panics). Only the shard's tables turn an id into a
//! handle, and they lose the id when the socket goes.

pub(crate) mod admission;
pub(crate) mod budget;
pub(crate) mod device;
pub(crate) mod guard;
pub(crate) mod rebuild;
pub(crate) mod shard;
pub(crate) mod stream;

pub(crate) use admission::Admission;
pub(crate) use budget::Budget;
pub(crate) use shard::Shard;
pub(crate) use stream::TunStream;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::task::Waker;
use std::time::Duration;

/// What makes a shard of the test build panic, to see what becomes of a
/// shard whose stack does: a segment the shard advances whose payload begins
/// with it panics in the shard's turn, and a write to a stream that begins
/// with it panics with the shard's lock held. Nothing of the sort is in any
/// other build (there is no packet that stably makes smoltcp 0.12 panic once
/// the filter and the window's guard are in front of it).
#[cfg(test)]
pub(crate) const PANIC_MARKER: &[u8] = b"rurge-tun test: panic";

/// The most packets a shard takes into its stack in one turn.
pub(crate) const INGRESS_PER_WAKE: usize = 256;
/// The most egress passes after them in one turn; each pass sends at most one
/// segment of each socket.
pub(crate) const EGRESS_PASSES: usize = 64;
/// How long a socket stays once it is first seen in TIME-WAIT: smoltcp holds
/// the ACK of the other side's last segment back for up to 10 ms (its default
/// ACK delay), and a socket removed before it is sent leaves the app without
/// it, to retransmit its FIN and be answered with a reset.
pub(crate) const TIME_WAIT_GRACE: Duration = Duration::from_millis(20);
/// How often a shard looks at every connection it has, besides those a turn
/// touched: smoltcp's own timers close sockets with no packet to say so.
pub(crate) const SWEEP: Duration = Duration::from_secs(1);

/// A connection's number: increasing, never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ConnId(u64);

impl ConnId {
    pub(crate) fn next() -> ConnId {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        ConnId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// A connection's two ends: the app's, which is the source of its packets,
/// and the one it connected to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Tuple {
    pub(crate) src: SocketAddr,
    pub(crate) dst: SocketAddr,
}

/// How a connection ended, which is what its stream answers once the shard
/// has let the connection go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Both ends closed: reads are the end of the stream, writes `BrokenPipe`.
    Closed,
    /// The app reset it, or it was taken for another reason: reads and
    /// writes are `ConnectionReset`.
    Reset,
}

/// What a connection's stream shares with its shard: the outcome once the
/// shard lets the connection go, and the wakers to wake then. The shard
/// lock is taken before this one, never the other way round.
#[derive(Default)]
pub(crate) struct ConnShared {
    inner: Mutex<SharedInner>,
}

#[derive(Default)]
struct SharedInner {
    outcome: Option<Outcome>,
    read: Option<Waker>,
    write: Option<Waker>,
}

impl ConnShared {
    fn inner(&self) -> MutexGuard<'_, SharedInner> {
        self.inner.lock().expect("a connection's shared state")
    }

    /// The task that waits to read, or for the handshake.
    pub(crate) fn register_read(&self, waker: &Waker) {
        register(&mut self.inner().read, waker);
    }

    pub(crate) fn register_write(&self, waker: &Waker) {
        register(&mut self.inner().write, waker);
    }

    /// Records the outcome (the first one stays) and wakes whoever waits.
    pub(crate) fn end(&self, outcome: Outcome) {
        let (read, write) = {
            let mut inner = self.inner();
            inner.outcome.get_or_insert(outcome);
            (inner.read.take(), inner.write.take())
        };
        for waker in read.into_iter().chain(write) {
            waker.wake();
        }
    }

    pub(crate) fn outcome(&self) -> Option<Outcome> {
        self.inner().outcome
    }
}

fn register(slot: &mut Option<Waker>, waker: &Waker) {
    match slot {
        Some(old) if old.will_wake(waker) => {}
        _ => *slot = Some(waker.clone()),
    }
}
