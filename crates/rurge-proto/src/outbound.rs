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
}

impl fmt::Display for OutboundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutboundError::Reject(k) => write!(f, "rejected by {}", k.name()),
            OutboundError::Unsupported(t) => write!(f, "policy protocol not implemented: {t}"),
            OutboundError::Dns(m) => write!(f, "dns: {m}"),
            OutboundError::Io(e) => write!(f, "{e}"),
            OutboundError::Timeout => f.write_str("connect timed out"),
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
}

pub type OutboundRef = Arc<dyn Outbound>;
