//! What the subnet expressions look at: the network the machine is on now
//! (phase 3 M1 design 2).
//!
//! The expressions are evaluated by the rule matcher (`rurge-rules`) and by
//! the policy registry (`rurge-policy`), which do not depend on each other;
//! this crate reads them, so the data type is here. Who reads the network off
//! the machine, and how often, is the platform layer's and the daemon's
//! business.

use std::net::IpAddr;
use std::sync::{Arc, PoisonError, RwLock};

/// What kind of link the primary interface is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetKind {
    Wifi,
    Wired,
    /// Anything else: a tunnel, a modem, an interface of a kind the platform
    /// would not name.
    Other,
}

/// The interface the traffic leaves by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Primary {
    pub interface: String,
    pub kind: NetKind,
    /// The default gateways: the IPv4 one first, then the IPv6 one.
    pub routers: Vec<IpAddr>,
}

/// A Wi-Fi adapter that is associated with an access point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiLink {
    pub interface: String,
    /// `None`: the platform would not say (no permission to see it, or it is
    /// redacted).
    pub ssid: Option<String>,
    pub bssid: Option<String>,
}

/// The network as far as it is known. Its `Debug` shows the SSID, the BSSID
/// and the gateways in full: they are no credentials, but they say where the
/// machine is, so a caller keeps them out of the logs above DEBUG.
///
/// `NetworkFacts::default()` knows nothing — no primary interface, no Wi-Fi
/// link — and no expression matches it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkFacts {
    /// The physical interface of the IPv4 default route — of the IPv6 one
    /// when there is no IPv4 default route; `None` when there is neither.
    pub primary: Option<Primary>,
    /// The Wi-Fi link, when an adapter is associated with an access point.
    pub wifi: Option<WifiLink>,
}

/// The network now, for the readers that evaluate expressions to look at.
/// One cell lives as long as the engine does, across the generations of the
/// profile; a reader takes a snapshot with `load` and keeps working on it
/// when the network changes meanwhile.
#[derive(Debug)]
pub struct NetworkCell(RwLock<Arc<NetworkFacts>>);

impl NetworkCell {
    pub fn new(facts: NetworkFacts) -> NetworkCell {
        NetworkCell(RwLock::new(Arc::new(facts)))
    }

    /// The network as it is now. A snapshot never changes.
    pub fn load(&self) -> Arc<NetworkFacts> {
        // a lock a thread died holding still holds a whole `Arc`
        Arc::clone(&self.0.read().unwrap_or_else(PoisonError::into_inner))
    }

    pub fn store(&self, facts: NetworkFacts) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(facts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wired(interface: &str) -> NetworkFacts {
        NetworkFacts {
            primary: Some(Primary {
                interface: interface.to_string(),
                kind: NetKind::Wired,
                routers: vec!["10.0.0.1".parse().unwrap()],
            }),
            wifi: None,
        }
    }

    /// Nothing is known until somebody reads the network: no primary
    /// interface, no Wi-Fi link.
    #[test]
    fn nothing_is_known_by_default() {
        let facts = NetworkFacts::default();
        assert_eq!(facts.primary, None);
        assert_eq!(facts.wifi, None);
        assert_eq!(*NetworkCell::new(NetworkFacts::default()).load(), facts);
    }

    #[test]
    fn the_network_cell_hands_out_what_was_stored_last() {
        let cell = NetworkCell::new(wired("eth0"));
        assert_eq!(*cell.load(), wired("eth0"));
        let before = cell.load();
        cell.store(wired("eth1"));
        assert_eq!(*cell.load(), wired("eth1"));
        cell.store(NetworkFacts::default());
        assert_eq!(*cell.load(), NetworkFacts::default());
        // what was handed out stays what it was: a lookup in flight finishes
        // on the network it started on
        assert_eq!(*before, wired("eth0"));
    }

    /// A thread that panics while it holds the lock must not take the cell
    /// down with it: the value inside is a whole `Arc`, never half written.
    #[test]
    fn a_poisoned_cell_still_hands_out_and_takes_facts() {
        let cell = Arc::new(NetworkCell::new(wired("eth0")));
        let held = cell.clone();
        let panicked = std::thread::spawn(move || {
            let _guard = held.0.write().unwrap();
            panic!("the thread dies holding the lock");
        })
        .join();
        assert!(panicked.is_err());
        assert!(cell.0.is_poisoned());
        assert_eq!(*cell.load(), wired("eth0"));
        cell.store(wired("eth1"));
        assert_eq!(*cell.load(), wired("eth1"));
    }
}
