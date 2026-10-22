//! The destinations of one UDP session (phase 3 M3 design 7.2): each is a flow
//! of the dialer's, found two ways, by the endpoint the app sent to (a datagram
//! comes) and by the target the dialer names (a reply or a refusal comes), both
//! at once and in constant time.

use rurge_dns::fakeip::Lease;
use rurge_net::connector::Target;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// The most an IP header and a UDP header take: an IPv4 header with all its
/// options is 60 bytes, IPv6's is 40.
const QUOTE_MAX: usize = 60 + 8;

/// The IP header and the UDP header of a datagram, which an ICMP message
/// quotes: kept in the destination as the latest datagram to it had them,
/// without an allocation for each datagram.
#[derive(Clone, Copy)]
pub(crate) struct Quote {
    bytes: [u8; QUOTE_MAX],
    len: usize,
}

impl Quote {
    /// The headers a packet begins with: `headers` is as long as they are
    /// (what a packet that has passed the checks has, an IP header and the 8
    /// bytes of a UDP header); a longer one is cut to the most they can be.
    pub(crate) fn of(headers: &[u8]) -> Quote {
        let len = headers.len().min(QUOTE_MAX);
        let mut bytes = [0; QUOTE_MAX];
        bytes[..len].copy_from_slice(&headers[..len]);
        Quote { bytes, len }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// One destination of a session.
pub(crate) struct Entry {
    /// The target the dialer is given: the destination as the engine's rules
    /// see it, a name for a fake address.
    pub(crate) target: Target,
    /// The endpoint the app sent to, which is where it expects replies to come
    /// from, and the source of an ICMP message about it.
    pub(crate) endpoint: SocketAddr,
    /// The headers of the latest datagram to it.
    pub(crate) quote: Quote,
    /// The hold on the fake address of the name, for as long as the entry is
    /// there, so that the address is not given to another name meanwhile.
    _lease: Option<Lease>,
    /// When a datagram last went to it, and when one last came from it.
    up: Instant,
    down: Instant,
}

impl Entry {
    pub(crate) fn new(
        endpoint: SocketAddr,
        target: Target,
        lease: Option<Lease>,
        quote: Quote,
        now: Instant,
    ) -> Entry {
        Entry {
            target,
            endpoint,
            quote,
            _lease: lease,
            up: now,
            down: now,
        }
    }

    /// A datagram went to it.
    pub(crate) fn sent(&mut self, quote: Quote, now: Instant) {
        self.quote = quote;
        self.up = now;
    }

    /// A reply came from it.
    pub(crate) fn answered(&mut self, now: Instant) {
        self.down = now;
    }
}

/// The destinations of a session, at most `max`.
pub(crate) struct Targets {
    max: usize,
    /// By the endpoint the app sent to.
    by_endpoint: HashMap<SocketAddr, Entry>,
    /// The endpoint of each target.
    endpoints: HashMap<Target, SocketAddr>,
}

impl Targets {
    pub(crate) fn new(max: usize) -> Targets {
        Targets {
            max,
            by_endpoint: HashMap::new(),
            endpoints: HashMap::new(),
        }
    }

    /// Whether the session has all the destinations it can have.
    pub(crate) fn is_full(&self) -> bool {
        self.by_endpoint.len() >= self.max
    }

    /// The destination for a datagram sent to `endpoint`.
    pub(crate) fn by_endpoint(&mut self, endpoint: SocketAddr) -> Option<&mut Entry> {
        self.by_endpoint.get_mut(&endpoint)
    }

    /// The destination of the flow the dialer calls `target`.
    pub(crate) fn by_target(&mut self, target: &Target) -> Option<&mut Entry> {
        let endpoint = self.endpoints.get(target)?;
        self.by_endpoint.get_mut(endpoint)
    }

    /// Adds a destination whose endpoint and target the session has none of,
    /// and which it has room for (the caller has looked).
    pub(crate) fn insert(&mut self, entry: Entry) {
        debug_assert!(!self.is_full(), "a session has room for the destination");
        self.endpoints.insert(entry.target.clone(), entry.endpoint);
        self.by_endpoint.insert(entry.endpoint, entry);
    }

    /// Lets go of the destinations that have had no datagram either way for
    /// `idle`, and of the holds they have on fake addresses.
    pub(crate) fn sweep(&mut self, now: Instant, idle: Duration) {
        let endpoints = &mut self.endpoints;
        self.by_endpoint.retain(|_, entry| {
            let quiet = now.saturating_duration_since(entry.up.max(entry.down));
            let keep = quiet < idle;
            if !keep {
                endpoints.remove(&entry.target);
            }
            keep
        });
    }

    /// Lets go of every destination.
    pub(crate) fn clear(&mut self) {
        self.endpoints.clear();
        self.by_endpoint.clear();
    }
}
