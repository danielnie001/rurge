//! What the receive task, the UDP sessions, the echo requests and the DNS
//! queries of a TUN hold in common: [`Context`]. It lives in a module of its own
//! so that `dispatch`, `udp`, `icmp` and `dns` all take it from here, and none
//! of them has to name another to get at it.

use crate::bytes::QueuedBytes;
use crate::dns::Responder;
use crate::dns::udp::Query;
use crate::fragment::Ids;
use crate::limit::Limiter;
use crate::stats::Stats;
use crate::tcp::Admission;
use crate::udp::Sessions;
use crate::writer::{Class, Outlet};
use crate::{Hooks, Life, Pinger, TunOptions};
use rurge_dns::fakeip::FakeIpPool;
use rurge_inbound::Dialer;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::task::TaskTracker;

/// What the receive task hands the datagrams and the echo requests it reads on
/// with, and what the sessions it starts live by: the TUN's own state besides
/// the TCP shards. The task, each UDP session's task and each session's client
/// hold it.
pub(crate) struct Context {
    /// Where the UDP sessions go.
    pub(crate) dialer: Arc<dyn Dialer>,
    pub(crate) pool: Option<Arc<FakeIpPool>>,
    /// Where the settings of the generation in use come from, asked for each
    /// echo request.
    pub(crate) hooks: Arc<dyn Hooks>,
    /// Sends the echo requests that go out.
    pub(crate) pinger: Arc<dyn Pinger>,
    /// The writer's two queues, for what the TUN makes itself: the replies to
    /// the apps' datagrams and to their echo requests, the responder's answers
    /// and the ICMP messages. Which one a batch goes to is `send`'s to choose.
    pub(crate) writer: Outlet,
    pub(crate) stats: Arc<Stats>,
    pub(crate) life: Arc<Life>,
    pub(crate) options: TunOptions,
    /// The TUN's tasks, to which the sessions are added.
    pub(crate) tasks: TaskTracker,
    /// The device's MTU: the longest IP packet the TUN writes.
    pub(crate) mtu: usize,
    /// The UDP sessions, by the app endpoint each is for.
    pub(crate) sessions: Sessions,
    /// The identifications of the fragments the TUN writes.
    pub(crate) ids: Ids,
    /// The TUN's admission to new sockets and to its own resets, the one every
    /// shard holds: the limit on resets is the TUN's, so a reset `[IP Rewrite]`
    /// answers a SYN with comes out of the same limit.
    pub(crate) admission: Arc<Admission>,
    /// The limit on the ICMP messages for the UDP flows that were refused.
    pub(crate) icmp_limit: Mutex<Limiter>,
    /// How many echo requests wait for their replies: the places taken out of
    /// `max_echoes` (see `icmp`).
    pub(crate) echoes: AtomicUsize,
    /// The DNS responder: one for the TUN, so that what it forwards at once is
    /// one limit for the queries of every transport.
    pub(crate) responder: Arc<Responder>,
    /// The queue the receive task offers the DNS queries of the apps to (see
    /// `dns::udp`).
    pub(crate) dns_queue: mpsc::Sender<Query>,
    /// The bytes of the datagrams that wait in the UDP sessions' queues (see
    /// `crate::bytes`, which holds the budget of the shards' queues too).
    pub(crate) udp_bytes: Arc<QueuedBytes>,
}

impl Context {
    /// Sends `batch` to the writer's queue for `class` in this generation,
    /// waiting for room: the priority queue for `Class::Udp` while
    /// `udp_priority` is on, the normal one otherwise.
    pub(crate) async fn send(
        &self,
        class: Class,
        batch: Vec<Vec<u8>>,
    ) -> Result<(), mpsc::error::SendError<Vec<Vec<u8>>>> {
        self.queue(class).send(batch).await
    }

    /// The same without waiting.
    pub(crate) fn try_send(
        &self,
        class: Class,
        batch: Vec<Vec<u8>>,
    ) -> Result<(), mpsc::error::TrySendError<Vec<Vec<u8>>>> {
        self.queue(class).try_send(batch)
    }

    /// The queue `class` goes to in this generation. The setting is read on
    /// every batch, so a reload applies to the next: the datagrams of one UDP
    /// flow may change queues as it is reloaded, and UDP promises no order
    /// between them anyway (design 7.2).
    fn queue(&self, class: Class) -> &mpsc::Sender<Vec<Vec<u8>>> {
        if class == Class::Udp && self.hooks.settings().udp_priority {
            &self.writer.priority
        } else {
            &self.writer.normal
        }
    }
}
