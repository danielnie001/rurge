//! What the TUN asks of whoever started it (phase 3 M3 design 3.3), when it
//! needs it: the settings of the generation in use, and an echo request sent
//! out into the world. The engine implements the first; the second is a
//! platform's (M4), and a test's until then.

use rurge_net::BoxFuture;
use rurge_net::connector::Resolve;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;

/// What the TUN asks of whoever started it, when it needs it.
pub trait Hooks: Send + Sync {
    /// The settings of the generation in use now (a cheap clone).
    fn settings(&self) -> Arc<TunSettings>;
}

/// One generation's settings for the TUN: what a reload of the profile can
/// change under a TUN that runs on.
pub struct TunSettings {
    /// Resolves a fake address's name for an echo request.
    pub resolver: Arc<dyn Resolve>,
    /// `icmp-forwarding`, asked for each echo request. When true, an echo
    /// request to an address of rurge's own is answered by the TUN and any
    /// other is forwarded through the [`Pinger`] (one to a fake address, to the
    /// address of its name). When false, every echo request is dropped and
    /// counted `unsupported`, those to rurge's own addresses too.
    pub icmp_forwarding: bool,
}

/// Sends echo requests for the TUN.
pub trait Pinger: Send + Sync {
    /// One echo request to `dst`; resolves with the reply's payload. The
    /// identifier and the sequence number are the pinger's own, not the app's.
    fn echo<'a>(&'a self, dst: IpAddr, payload: &'a [u8]) -> BoxFuture<'a, io::Result<Vec<u8>>>;
}
