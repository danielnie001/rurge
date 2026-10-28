//! What the TUN asks of whoever started it (phase 3 M3 design 3.3), when it
//! needs it: the settings of the generation in use, whether a rule refuses a
//! DNS query or a connection before it is made, and an echo request sent out
//! into the world. The engine implements the first two; the third is a
//! platform's (M4), and a test's until then.

use rurge_config::{HijackTarget, HostList, IpRewrite, PreMatchLayer, SessionInfo};
use rurge_dns::hosts::HostAction;
use rurge_net::BoxFuture;
use rurge_net::connector::Resolve;
use rurge_proto::RejectKind;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;

/// What the TUN asks of whoever started it, when it needs it.
pub trait Hooks: Send + Sync {
    /// The settings of the generation in use now (a cheap clone).
    fn settings(&self) -> Arc<TunSettings>;

    /// Asked with the session of a DNS query for A or AAAA (`Dns`) or of a new TCP
    /// connection (`Syn`): the kind to refuse it with when a pre-matching rule
    /// refuses it, escalation already applied. Synchronous; never waits.
    fn pre_match(&self, layer: PreMatchLayer, session: &SessionInfo) -> Option<RejectKind>;
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
    /// What the DNS responder asks of this generation's resolver: for the
    /// engine, the resolver that `resolver` is.
    pub dns: Arc<dyn TunDns>,
    /// `always-real-ip`: the names the responder gives real addresses, not fake
    /// ones. An entry that names a port never matches a query, which has none.
    pub always_real_ip: HostList,
    /// `allow-dns-svcb`: whether the responder forwards SVCB and HTTPS queries
    /// (when false it answers them NOTIMP).
    pub allow_dns_svcb: bool,
    /// `hijack-dns`: the DNS servers whose queries the responder answers
    /// (UDP over IPv4 only).
    pub hijack_dns: Vec<HijackTarget>,
    /// `[IP Rewrite]` of this generation: asked with the destination of each
    /// packet whose IP header passed its own checks and that is no martian
    /// (design 8.3). The receive task reads it once for each batch.
    pub ip_rewrite: Arc<IpRewrite>,
}

/// What the responder asks of this generation's resolver.
pub trait TunDns: Send + Sync {
    /// The addresses `[Host]` (or the system hosts file) maps `name` to.
    fn mapped(&self, name: &str) -> Option<Vec<IpAddr>>;
    /// The reply of the upstreams this generation asks for the query's name,
    /// with the query's ID.
    fn forward<'a>(&'a self, query: &'a [u8]) -> BoxFuture<'a, io::Result<Vec<u8>>>;
}

impl TunDns for rurge_dns::Resolver {
    fn mapped(&self, name: &str) -> Option<Vec<IpAddr>> {
        match self.host_lookup(name)?.action {
            HostAction::Ips(addrs) => Some(addrs),
            // an alias, a server or the system's: not an address
            HostAction::Alias(_) | HostAction::Servers(_) | HostAction::System(_) => None,
        }
    }

    fn forward<'a>(&'a self, query: &'a [u8]) -> BoxFuture<'a, io::Result<Vec<u8>>> {
        Box::pin(rurge_dns::Resolver::forward(self, query))
    }
}

/// Sends echo requests for the TUN.
pub trait Pinger: Send + Sync {
    /// One echo request to `dst`; resolves with the reply's payload. The
    /// identifier and the sequence number are the pinger's own, not the app's.
    fn echo<'a>(&'a self, dst: IpAddr, payload: &'a [u8]) -> BoxFuture<'a, io::Result<Vec<u8>>>;
}
