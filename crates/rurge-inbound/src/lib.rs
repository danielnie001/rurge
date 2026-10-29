//! Inbound listeners (M3 design §6): HTTP/1.1 proxy on hyper and SOCKS5 by
//! hand. The HTTP listener keeps what belongs to it — authentication, the
//! `CONNECT` tunnel — and hands its absolute-form requests to the HTTP engine
//! through `Dialer::plain_requests`. Listeners only speak the protocol; rules,
//! policies, outbound connections, relaying and the plain requests belong to
//! the engine, reached through the `Dialer` trait.

mod hold;
pub mod http;
pub mod listener;
mod process;
pub mod responses;
pub mod restrict;
pub mod session;
pub mod socks5;
#[cfg(test)]
pub(crate) mod testing;
mod udp;

/// The types of a request a `PlainRequests` is handed, re-exported as the
/// trait names them: an implementation (`rurge-engine`) takes a connection's
/// requests through them without a dependency of its own on `http` and
/// `hyper`.
pub use ::http::Request;
pub use ::http::Response;
pub use hold::hold_dropped;
pub use http::HttpListener;
pub use hyper::body::Incoming;
pub use listener::{HttpAuth, ListenerOpts, Running};
pub use process::ProcessSlot;
pub use responses::{BoxError, ResponseBody};
pub use session::{
    ClientConn, Close, Counting, DialError, Dialed, Dialer, FailKind, PlainRequests, ServeOpts,
    SessionHandle, SessionOutcome, UdpAdmission, UdpClient,
};
pub use socks5::Socks5Listener;
