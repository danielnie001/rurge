//! Fake addresses that come in at a proxy port (phase 3 M3 design 13): while a
//! TUN runs, an address of rurge's space that a client of the HTTP or SOCKS5
//! listener names (an app that kept the address of a DNS answer, a browser with
//! a cached one) is turned back into the name behind it before the rules see
//! the session, as the TUN does for the sessions it makes.

use crate::engine::Engine;
use rurge_config::HostName;
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_dns::fakeip::{Lease, in_space};

/// What became of the destination of a session at a proxy port (`turn_back`).
pub(crate) enum Turned {
    /// It is left as it was: the session is not one of a proxy port's, its
    /// destination is a name or an address outside rurge's space, or no TUN
    /// runs (an address of rurge's space is an address like any other then).
    Unchanged,
    /// The destination is the name behind the fake address now. The lease keeps
    /// the address for the name until it is dropped, which the session's end is
    /// to do.
    Name(Lease),
    /// An address of rurge's space that no name is behind. The session is to
    /// fail (`FAKE_ADDRESS_NOT_KNOWN`) and to dial nothing.
    Unknown,
}

impl Engine {
    /// Turns the destination of `session` back into the name behind it, if it
    /// is a session of the HTTP or SOCKS5 listener (a flow of a SOCKS5
    /// association is) whose destination is an address of rurge's space, and
    /// the engine has a pool of fake addresses. The sessions of the TUN come
    /// with their names already, and rurge's own are never to a fake address.
    pub(crate) fn turn_back(&self, session: &mut SessionInfo) -> Turned {
        if !matches!(session.listener, ListenerKind::Http | ListenerKind::Socks5) {
            return Turned::Unchanged;
        }
        let HostName::Ip(ip) = session.dst_host else {
            return Turned::Unchanged;
        };
        if !in_space(ip) {
            return Turned::Unchanged;
        }
        let Some(pool) = self.fake_pool.load_full() else {
            return Turned::Unchanged;
        };
        match pool.lease(ip.to_canonical()) {
            Some((name, lease)) => {
                session.dst_host = HostName::Domain(name);
                Turned::Name(lease)
            }
            None => Turned::Unknown,
        }
    }
}
