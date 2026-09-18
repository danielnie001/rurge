//! System proxy settings (M4 design §8.1). Every backend's logic is compiled
//! on every host — only the thin layer that touches the OS is `cfg`-gated or
//! injected — so each platform is unit-tested everywhere (the approach `dirs`
//! takes with `Os`).

pub mod windows;

use serde::{Deserialize, Serialize};
use std::io;
use std::net::SocketAddr;

/// What the operating system should be pointed at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxySettings {
    pub http: Option<SocketAddr>,
    pub https: Option<SocketAddr>,
    pub socks: Option<SocketAddr>,
    /// Surge `skip-proxy` semantics (macOS): host names / globs, IPs, CIDRs.
    pub bypass: Vec<String>,
    pub exclude_simple: bool,
}

/// The OS settings as they were before `apply`. Opaque to callers; it is
/// stored in `state.json` so the next run can undo a crashed one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Backup(pub serde_json::Value);

pub trait SystemProxy: Send + Sync {
    fn snapshot(&self) -> io::Result<Backup>;
    fn apply(&self, settings: &ProxySettings) -> io::Result<()>;
    fn restore(&self, backup: &Backup) -> io::Result<()>;
}

pub(crate) fn wrong_platform(expected: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("the system proxy backup was not taken by the {expected} backend"),
    )
}
