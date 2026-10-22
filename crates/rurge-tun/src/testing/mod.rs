//! Fixtures for everything built on the adapter's core: the operating
//! system's side of the device (an app-side TCP/IP stack), a fake `Dialer` that
//! hands out loopback or in-memory upstreams, fake hooks and a fake `Pinger`,
//! and loopback test servers.

mod app;
mod dialer;
mod hooks;
mod scripted;
mod servers;

pub use app::{AppStack, AppTcp, Direction};
pub use dialer::{FakeDialer, Script};
pub use hooks::{FakeHooks, FakePinger, FakeResolve, PingScript};
pub use scripted::ScriptedTcp;
pub use servers::{Upstream, pattern};
