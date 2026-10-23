//! What the receive task, the UDP sessions and the echo requests of a TUN hold
//! in common: [`Context`]. It lives in a module of its own so that `dispatch`,
//! `udp` and `icmp` all take it from here, and none of the three has to name
//! another to get at it.

use crate::fragment::Ids;
use crate::limit::Limiter;
use crate::stats::Stats;
use crate::udp::Sessions;
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
    /// The writer's queue, for what the TUN makes itself: replies to the apps'
    /// datagrams and to their echo requests, and ICMP messages.
    pub(crate) writer: mpsc::Sender<Vec<Vec<u8>>>,
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
    /// The limit on the ICMP messages for the UDP flows that were refused.
    pub(crate) icmp_limit: Mutex<Limiter>,
    /// How many echo requests wait for their replies: the places taken out of
    /// `max_echoes` (see `icmp`).
    pub(crate) echoes: AtomicUsize,
}
