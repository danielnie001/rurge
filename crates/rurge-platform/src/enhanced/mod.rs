//! The system half of the enhanced mode (M4 design 5, 6, 7.3, 8): the virtual
//! adapter, the adapter's addresses and routes, the system's DNS, the Linux
//! `rp_filter`, and the DNS servers of the network outside the enhanced mode.
//!
//! Nothing here belongs to one platform. The steps a platform runs are data
//! (`route`), the programs go through a `CommandRunner` and the files through a
//! root directory, so Linux's and macOS's answers are built and tested on every
//! platform (M4c plan P2). What a platform has of its own is its device
//! (`tun`) and the `system` below.
//!
//! This task's `system` answers [`Unsupported`] on every platform: M4c's tasks
//! 6 and 7 put Linux's and macOS's in its `cfg`s, and M4d Window's.

pub mod linux;
pub mod macos;

use std::io;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::route::RouteSet;
use crate::route::steps::Step;
use crate::tun::TunDevice;

/// How the steps name the virtual adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    /// Windows: the interface index (M4d).
    pub index: Option<u32>,
}

/// A change that outlives rurge, and what putting it back takes (design 9.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Saved {
    /// macOS: a network service's DNS servers before the takeover (empty: none were set).
    MacDns {
        service: String,
        servers: Vec<IpAddr>,
    },
    /// Linux: `net.ipv4.conf.all.rp_filter` before it was loosened.
    RpFilter { value: u8 },
}

/// What a platform without the enhanced mode answers, and the words its
/// `open` fails with.
pub const NOT_AVAILABLE: &str =
    "the enhanced mode is not available on this platform in this version";

/// Each program the steps run gets this long (M4c plan P11).
pub const STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// The system half of the enhanced mode (M4 design 5, 6, 7.3, 8). Every method
/// blocks (programs, files) and is called on a blocking thread; an error is
/// English text for the log and the API's 500, without the names of the
/// user's networks (design 13).
pub trait System: Send + Sync {
    fn open(&self) -> io::Result<TunDevice>;
    /// The address, MTU and link, and with `ipv6` the IPv6 address. `Ok(false)`:
    /// IPv6 was asked for and could not be had (warned); `Ok(true)` otherwise.
    fn configure(&self, dev: &Adapter, ipv6: bool) -> Result<bool, String>;
    fn configure_ipv6(&self, dev: &Adapter) -> Result<bool, String>;
    fn unconfigure_ipv6(&self, dev: &Adapter) -> Result<(), String>;
    /// Adds `add`, then removes `remove` (going on past errors). `Ok(false)`:
    /// the IPv6 half of `add` could not be added; what of it was is removed again.
    fn routes(&self, dev: &Adapter, add: &RouteSet, remove: &RouteSet) -> Result<bool, String>;
    /// What `take_dns` will change that outlives rurge and is not in `saved`,
    /// read before it is changed.
    fn dns_saved(&self, dev: &Adapter, saved: &[Saved]) -> Result<Vec<Saved>, String>;
    /// `Ok(false)`: not on this system (warned, once an open).
    fn take_dns(&self, dev: &Adapter, ipv6: bool) -> Result<bool, String>;
    fn give_dns_back(&self, dev: &Adapter) -> Result<(), String>;
    fn adjust_saved(&self, primary: Option<&str>, saved: &[Saved])
    -> Result<Option<Saved>, String>;
    fn adjust(&self) -> Result<(), String>;
    /// Puts one saved thing back: on a close, and after a crash. A kind this
    /// platform never saves is dropped with a warning (M4c plan P9).
    fn undo(&self, saved: &Saved) -> Result<(), String>;
    /// The DNS servers of the network outside the enhanced mode, for
    /// `server:system` while it is on (design 6.3). `None`: the system's own
    /// list is right (no takeover).
    fn physical_dns(
        &self,
        primary: Option<&str>,
        saved: &[Saved],
        network_changed: bool,
    ) -> Option<Vec<IpAddr>>;
}

/// This platform's (Linux and macOS: M4c; Windows: M4d); [`Unsupported`] elsewhere.
/// Scripts go to `scratch` (the data directory's `enhanced`).
///
/// In this task it is [`Unsupported`] on every platform: M4c's tasks 6 and 7
/// put Linux's and macOS's here.
pub fn system(_scratch: PathBuf) -> Box<dyn System> {
    Box::new(Unsupported)
}

/// The order of [`System::routes`], the same on Linux and macOS: add, and when
/// the IPv6 half of that failed, take that half out again and go on without
/// IPv6 (M4-D17); then remove, going on past errors. `run` runs a batch of
/// steps, `add` and `remove` translate one set of prefixes into its steps.
fn run_routes(
    add: &RouteSet,
    remove: &RouteSet,
    run: impl Fn(&[Step]) -> Result<bool, String>,
    add_steps: impl Fn(&RouteSet) -> Vec<Step>,
    remove_steps: impl Fn(&RouteSet) -> Vec<Step>,
) -> Result<bool, String> {
    let added = run(&add_steps(add))?;
    if !added {
        // what of the IPv6 half did go in goes out again: the open goes on
        // without IPv6
        let v6 = RouteSet {
            v4: Vec::new(),
            v6: add.v6.clone(),
        };
        run(&remove_steps(&v6))?;
    }
    // removing goes on past errors and is no error of its own
    run(&remove_steps(remove))?;
    Ok(added)
}

/// The system of a platform that has no enhanced mode in this version: every
/// action says so, and nothing here touches a machine.
pub struct Unsupported;

impl System for Unsupported {
    fn open(&self) -> io::Result<TunDevice> {
        Err(io::Error::new(io::ErrorKind::Unsupported, NOT_AVAILABLE))
    }

    fn configure(&self, _dev: &Adapter, _ipv6: bool) -> Result<bool, String> {
        Err(NOT_AVAILABLE.to_string())
    }

    fn configure_ipv6(&self, _dev: &Adapter) -> Result<bool, String> {
        Err(NOT_AVAILABLE.to_string())
    }

    fn unconfigure_ipv6(&self, _dev: &Adapter) -> Result<(), String> {
        Err(NOT_AVAILABLE.to_string())
    }

    fn routes(&self, _dev: &Adapter, _add: &RouteSet, _remove: &RouteSet) -> Result<bool, String> {
        Err(NOT_AVAILABLE.to_string())
    }

    fn dns_saved(&self, _dev: &Adapter, _saved: &[Saved]) -> Result<Vec<Saved>, String> {
        Err(NOT_AVAILABLE.to_string())
    }

    fn take_dns(&self, _dev: &Adapter, _ipv6: bool) -> Result<bool, String> {
        Err(NOT_AVAILABLE.to_string())
    }

    fn give_dns_back(&self, _dev: &Adapter) -> Result<(), String> {
        Err(NOT_AVAILABLE.to_string())
    }

    fn adjust_saved(
        &self,
        _primary: Option<&str>,
        _saved: &[Saved],
    ) -> Result<Option<Saved>, String> {
        Err(NOT_AVAILABLE.to_string())
    }

    fn adjust(&self) -> Result<(), String> {
        Err(NOT_AVAILABLE.to_string())
    }

    fn undo(&self, _saved: &Saved) -> Result<(), String> {
        Err(NOT_AVAILABLE.to_string())
    }

    fn physical_dns(
        &self,
        _primary: Option<&str>,
        _saved: &[Saved],
        _network_changed: bool,
    ) -> Option<Vec<IpAddr>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    /// The system of a platform that has none in this version answers every
    /// action with the same "not available", changes nothing, and opens no
    /// device (M4c plan P19: the test builds it itself, not through `system`).
    #[test]
    fn the_unsupported_system_says_so() {
        let system = Unsupported;
        let Err(error) = system.open() else {
            panic!("the unsupported system opened a device")
        };
        assert_eq!(error.kind(), ErrorKind::Unsupported);
        assert_eq!(error.to_string(), NOT_AVAILABLE);

        let dev = Adapter {
            name: "rurge".to_string(),
            index: None,
        };
        let routes = RouteSet::default();
        assert_eq!(system.configure(&dev, true).unwrap_err(), NOT_AVAILABLE);
        assert_eq!(system.configure_ipv6(&dev).unwrap_err(), NOT_AVAILABLE);
        assert_eq!(system.unconfigure_ipv6(&dev).unwrap_err(), NOT_AVAILABLE);
        assert_eq!(
            system.routes(&dev, &routes, &routes).unwrap_err(),
            NOT_AVAILABLE
        );
        assert_eq!(system.dns_saved(&dev, &[]).unwrap_err(), NOT_AVAILABLE);
        assert_eq!(system.take_dns(&dev, false).unwrap_err(), NOT_AVAILABLE);
        assert_eq!(system.give_dns_back(&dev).unwrap_err(), NOT_AVAILABLE);
        assert_eq!(system.adjust_saved(None, &[]).unwrap_err(), NOT_AVAILABLE);
        assert_eq!(system.adjust().unwrap_err(), NOT_AVAILABLE);
        assert_eq!(
            system.undo(&Saved::RpFilter { value: 1 }).unwrap_err(),
            NOT_AVAILABLE
        );
        // the one action that answers rather than fails: there is no system
        // outside the enhanced mode to read servers from
        assert_eq!(system.physical_dns(None, &[], false), None);
    }
}
