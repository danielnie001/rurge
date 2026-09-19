//! The `Outbound` trait and its error type.

use rurge_config::policy::Builtin;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
use std::fmt;
use std::io;
use std::sync::Arc;

/// The four REJECT flavours (M3 design §8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RejectKind {
    Reject,
    Drop,
    NoDrop,
    TinyGif,
}

impl RejectKind {
    pub fn from_builtin(b: Builtin) -> Option<RejectKind> {
        match b {
            Builtin::Reject => Some(RejectKind::Reject),
            Builtin::RejectDrop => Some(RejectKind::Drop),
            Builtin::RejectNoDrop => Some(RejectKind::NoDrop),
            Builtin::RejectTinyGif => Some(RejectKind::TinyGif),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RejectKind::Reject => "REJECT",
            RejectKind::Drop => "REJECT-DROP",
            RejectKind::NoDrop => "REJECT-NO-DROP",
            RejectKind::TinyGif => "REJECT-TINYGIF",
        }
    }

    /// Kinds that count towards the automatic escalation to DROP (M3b).
    pub fn escalates(self) -> bool {
        matches!(self, RejectKind::Reject | RejectKind::TinyGif)
    }
}

#[derive(Debug)]
pub enum OutboundError {
    Reject(RejectKind),
    /// The policy's protocol is not implemented in this version.
    Unsupported(String),
    Dns(String),
    Io(io::Error),
    Timeout,
    /// The proxy refused or broke the handshake (`socks5: authentication failed`).
    Proxy(String),
    Tls(String),
    /// The policy exists but cannot be used (M3: a broken subscription item).
    Unavailable(String),
}

impl fmt::Display for OutboundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutboundError::Reject(k) => write!(f, "rejected by {}", k.name()),
            OutboundError::Unsupported(t) => write!(f, "policy protocol not implemented: {t}"),
            OutboundError::Dns(m) => write!(f, "dns: {m}"),
            OutboundError::Io(e) => write!(f, "{e}"),
            OutboundError::Timeout => f.write_str("connect timed out"),
            OutboundError::Proxy(m) => f.write_str(m),
            OutboundError::Tls(m) => write!(f, "tls: {m}"),
            OutboundError::Unavailable(m) => write!(f, "policy unavailable: {m}"),
        }
    }
}

impl std::error::Error for OutboundError {}

impl From<io::Error> for OutboundError {
    fn from(e: io::Error) -> OutboundError {
        if e.kind() == io::ErrorKind::TimedOut {
            OutboundError::Timeout
        } else {
            OutboundError::Io(e)
        }
    }
}

/// An HTTP proxy that takes plain requests in absolute form
/// (`always-use-connect = false`, the manual's default).
pub trait HttpForward: Send + Sync {
    /// Connects to the proxy itself (TCP, then TLS for `https`): no CONNECT.
    fn connect<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>>;
    /// `Proxy-Authorization` and the configured `headers`, rendered for one request.
    fn request_headers(&self) -> Vec<(String, String)>;
}

/// A way to reach a destination. Phase 1 ships `Direct` and `Reject`; every
/// proxy protocol of phase 2 implements this trait too.
pub trait Outbound: Send + Sync {
    /// Display name: `DIRECT`, `REJECT-TINYGIF`, or the policy name.
    fn name(&self) -> &str;
    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>>;
    /// `Some` when plain HTTP requests may be sent to this outbound in
    /// absolute form instead of through a CONNECT tunnel.
    fn http_forward(&self) -> Option<&dyn HttpForward> {
        None
    }
}

pub type OutboundRef = Arc<dyn Outbound>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Direct, Reject};
    use rurge_net::connector::SystemResolve;

    #[test]
    fn the_new_error_variants_render_for_the_session_log() {
        assert_eq!(
            OutboundError::Proxy("socks5: authentication failed".into()).to_string(),
            "socks5: authentication failed"
        );
        assert_eq!(
            OutboundError::Tls("certificate fingerprint mismatch".into()).to_string(),
            "tls: certificate fingerprint mismatch"
        );
        assert_eq!(
            OutboundError::Unavailable("subscription item is broken".into()).to_string(),
            "policy unavailable: subscription item is broken"
        );
    }

    #[test]
    fn only_http_proxies_forward_plain_requests() {
        let direct = Direct::with_resolver(Arc::new(SystemResolve));
        assert!(direct.http_forward().is_none());
        assert!(Reject::new(RejectKind::Reject).http_forward().is_none());
    }
}
