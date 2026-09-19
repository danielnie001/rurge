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
    /// The TLS handshake with the proxy failed; the text is bounded (see
    /// `OutboundError::tls`).
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

/// Text that came from the other end of a connection, made safe for a log
/// line and the request log: control characters dropped, at most `max`
/// characters kept.
pub(crate) fn untrusted_text(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect()
}

impl OutboundError {
    /// A TLS failure. The text may quote names presented by the server, so
    /// it is bounded like any other text from the far end.
    pub fn tls(error: impl fmt::Display) -> OutboundError {
        OutboundError::Tls(untrusted_text(&error.to_string(), 256))
    }
}

/// An HTTP proxy that takes plain requests in absolute form
/// (`always-use-connect = false`, the manual's default).
///
/// Two obligations fall on the caller (M1b's engine), since this trait only
/// hands back a raw stream:
/// - before writing a request line or a `Host` header for a target, check it
///   with `rurge_proto::http::valid_target` and refuse the request
///   otherwise (the CONNECT path already does this itself, internally);
/// - `request_headers()` must be called exactly once per connection: every
///   call renders the `<random-string(..)>` placeholders anew.
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

    #[test]
    fn untrusted_text_drops_control_characters_and_is_bounded_in_characters() {
        assert_eq!(untrusted_text("a\x1bb\rc\nd\0e", 64), "abcde");
        // a multi-byte character: a byte-based cut would split it or miscount
        let long = "é".repeat(1000);
        let cut = untrusted_text(&long, 64);
        assert_eq!(cut.chars().count(), 64);
        assert_eq!(cut, "é".repeat(64));
    }

    #[test]
    fn outbound_error_tls_bounds_the_text_it_wraps() {
        let err = OutboundError::tls("x".repeat(1000));
        assert_eq!(err.to_string(), format!("tls: {}", "x".repeat(256)));
    }
}
