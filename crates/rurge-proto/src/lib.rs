//! Outbound abstraction (M3 design §4): the `Outbound` trait every policy
//! implements, plus the phase 1 built-ins `Direct` and `Reject`.

mod addr;
pub mod anytls;
pub mod build;
pub mod direct;
pub mod external;
// the `h2-connect` outbound (M6c task 3) is its first user
#[allow(dead_code)]
pub(crate) mod h2pool;
mod hostname;
pub mod http;
pub mod keystore;
pub mod outbound;
pub mod reject;
pub mod shadowsocks;
pub mod snell;
pub mod socks5;
mod stream_udp;
mod task;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod transport;
pub mod trojan;
pub mod vmess;

pub use build::BuildError;
pub use direct::Direct;
pub use outbound::{HttpForward, Outbound, OutboundError, OutboundRef, RejectKind, UdpSupport};
pub use reject::Reject;
