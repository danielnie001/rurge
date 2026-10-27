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
//! `system` gives a platform that has one in this version its own system —
//! Linux's is [`linux::LinuxSystem`], macOS's [`macos::MacSystem`], Windows's
//! [`windows::WindowsSystem`] — and [`Unsupported`] to the rest.

pub mod linux;
pub mod macos;
pub mod windows;

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
    /// Loosens what `adjust_saved` asked for, or what `saved` holds already —
    /// written again then, so that another program applying the system's
    /// settings over it is undone by the next change of network. Not called
    /// when neither says so (the phase 3 / M4c plan's execution record row 40).
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

/// The system of this platform: its own where it has one in this version, and
/// [`Unsupported`] elsewhere. Linux's is [`linux::LinuxSystem`], over the
/// programs the system's own runner runs and the files under `/`; macOS's is
/// [`macos::MacSystem`], over the same runner and `/etc/resolv.conf`. Scripts go
/// to `scratch` (the data directory's `enhanced`). `wintun_dll` is where
/// `wintun.dll` is on Windows, and Linux has no use for it (the option is a
/// usage error on the platforms without the device, M4 design 9.6).
#[cfg(target_os = "linux")]
pub fn system(scratch: PathBuf, _wintun_dll: Option<PathBuf>) -> Box<dyn System> {
    Box::new(linux::LinuxSystem::new(
        std::sync::Arc::new(crate::command::TimedRunner(STEP_TIMEOUT)),
        "/".into(),
        scratch,
    ))
}

/// The system of this platform: its own where it has one in this version, and
/// [`Unsupported`] elsewhere. macOS's is [`macos::MacSystem`], over the programs
/// the system's own runner runs and `/etc/resolv.conf`. Scripts go to `scratch`
/// (the data directory's `enhanced`). `wintun_dll` is where `wintun.dll` is on
/// Windows, and macOS has no use for it (the option is a usage error on the
/// platforms without the device, M4 design 9.6).
#[cfg(target_os = "macos")]
pub fn system(scratch: PathBuf, _wintun_dll: Option<PathBuf>) -> Box<dyn System> {
    Box::new(macos::MacSystem::new(
        std::sync::Arc::new(crate::command::TimedRunner(STEP_TIMEOUT)),
        "/etc/resolv.conf".into(),
        scratch,
    ))
}

/// The system of this platform: Windows's is [`windows::WindowsSystem`], over
/// `netsh` and `ipconfig` under the system root (`%SystemRoot%`, or
/// `C:\Windows`), and with `wintun.dll` from `wintun_dll` or next to
/// `rurge.exe`. Scripts go to `scratch` (the data directory's `enhanced`).
#[cfg(windows)]
pub fn system(scratch: PathBuf, wintun_dll: Option<PathBuf>) -> Box<dyn System> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    Box::new(windows::WindowsSystem::new(
        std::sync::Arc::new(crate::command::TimedRunner(STEP_TIMEOUT)),
        crate::route::windows::WindowsTools::under(std::path::Path::new(&root)),
        scratch,
        crate::tun::dll::wintun_dll(wintun_dll.as_deref(), std::env::current_exe()),
    ))
}

/// The system of a platform that has none in this version: [`Unsupported`],
/// which says so to every action. `scratch` (the data directory's `enhanced`)
/// and `wintun_dll` (where Windows's `wintun.dll` would be) have nothing to
/// hold here.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn system(_scratch: PathBuf, _wintun_dll: Option<PathBuf>) -> Box<dyn System> {
    Box::new(Unsupported)
}

/// The order of [`System::routes`], the same on Linux and macOS: add, and when
/// the IPv6 half of that failed, say so (one WARN, without a name or a prefix
/// in it), take that half out again and go on without it (M4-D17); then remove,
/// going on past errors. `run` runs a batch of steps, `add` and `remove`
/// translate one set of prefixes into its steps. The address's own IPv6 step is
/// the manager's to warn about, so `run` only logs its failure at DEBUG.
fn run_routes(
    add: &RouteSet,
    remove: &RouteSet,
    run: impl Fn(&[Step]) -> Result<bool, String>,
    add_steps: impl Fn(&RouteSet) -> Vec<Step>,
    remove_steps: impl Fn(&RouteSet) -> Vec<Step>,
) -> Result<bool, String> {
    let added = run(&add_steps(add))?;
    if !added {
        // What of it went in may still be there: the manager records the old
        // IPv6 routes less the removed ones, so the user is told what is left
        // out rather than that IPv6 is gone — on a reload it is not.
        tracing::warn!(
            "the IPv6 routes could not be added to the virtual adapter; they are left out"
        );
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
