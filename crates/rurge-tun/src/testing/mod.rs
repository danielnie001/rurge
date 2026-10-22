//! Fixtures for everything built on the adapter's core: the operating
//! system's side of the device (an app-side TCP/IP stack), a fake `Dialer` that
//! hands out loopback or in-memory upstreams and serves UDP associations from
//! scripts, fake hooks and a fake `Pinger`, the UDP and ICMP packets an app
//! sends and reads (`packets`), and loopback test servers.

mod app;
mod dialer;
mod hooks;
pub mod packets;
mod scripted;
mod servers;

pub use app::{AppStack, AppTcp, Direction};
pub use dialer::{FakeDialer, Script, UdpScript};
pub use hooks::{FakeHooks, FakePinger, FakeResolve, PingScript};
pub use scripted::ScriptedTcp;
pub use servers::{Upstream, pattern};
