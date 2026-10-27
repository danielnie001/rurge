//! What the manager asks of the system (M4 design 5.1, 9.1): abstract actions
//! only, each of them blocking, and the backends that answer them.
//!
//! Nothing here runs a program or changes anything by itself. The platform
//! backend hands every action to `rurge_platform::enhanced::System` — Linux's
//! or macOS's, which change the machine; on a platform without the enhanced
//! mode in this version that system is `Unsupported` and answers as it does.
//! The file backend (`file:<path>`) is the one tests set.

use super::device::PlatformDevice;
use super::file::FileBackend;
use super::undo::Undo;
use ipnet::IpNet;
use rurge_platform::enhanced::{Adapter, Saved, System};
use rurge_platform::route::RouteSet;
use rurge_tun::device::Device;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

/// `RURGE_ENHANCED_BACKEND`: unset → the platform's; `off`; `file:<path>`.
pub const BACKEND_ENV: &str = "RURGE_ENHANCED_BACKEND";

/// What `off` answers.
const OFF: &str = "the enhanced mode backend is off (RURGE_ENHANCED_BACKEND=off)";

/// The device a backend opened: what the TUN reads and writes, and how the
/// system names it.
#[derive(Clone)]
pub struct Opened {
    pub device: Arc<dyn Device>,
    /// The interface's name: for the debug log and the egress table.
    pub name: String,
    /// Windows: the interface index the steps name the adapter by.
    pub index: Option<u32>,
}

/// What the manager asks of the system (M4 design 5.1). Every method blocks
/// (programs, files) and is called on a blocking thread; an error is English
/// text for the log and for the API's 500.
pub trait Backend: Send + Sync {
    fn open(&self) -> Result<Opened, String>;
    /// The adapter's address, MTU, link and own settings, and with `ipv6` its
    /// IPv6 address. `Ok(false)`: IPv6 was asked for and could not be had (the
    /// manager logs the WARN, so the backend need not): this open goes on
    /// without it.
    fn configure(&self, dev: &Opened, ipv6: bool) -> Result<bool, String>;
    /// The IPv6 half alone, on an adapter that is up (a reload or a change of
    /// network turned IPv6 on). `Ok(false)` as for `configure`, with the WARN
    /// from the manager as well.
    fn configure_ipv6(&self, dev: &Opened) -> Result<bool, String>;
    fn unconfigure_ipv6(&self, dev: &Opened) -> Result<(), String>;
    /// Adds `add`, then removes `remove`; removing goes on past errors.
    /// `Ok(false)`: the IPv6 half of `add` could not be added (warned). The
    /// backend has taken that half back and removed `remove` all the same, so
    /// what the adapter still has is the old set less `remove` (the phase 3 /
    /// M4a plan's execution record row 95).
    fn routes(&self, dev: &Opened, add: &RouteSet, remove: &RouteSet) -> Result<bool, String>;
    /// The directly connected subnets of the interfaces but `exclude`, longest
    /// prefix first (M4 design 7.1, rule 3). A read, not a change: the file
    /// backend does not record it among its actions, and an error is a WARN
    /// rather than the end of the open (the manager writes the table without
    /// them).
    fn subnets(&self, exclude: &[String]) -> Result<Vec<(IpNet, String)>, String>;
    /// The undo actions `take_dns` will need that are not among `saved` (the
    /// ones in the backup already), read before it is done (macOS: each
    /// enabled service and its servers; on a change of network, the services
    /// that are new).
    fn dns_undo(&self, dev: &Opened, saved: &[Undo]) -> Result<Vec<Undo>, String>;
    /// Points the system's DNS at the responder. `Ok(false)`: not on this
    /// system (Linux without systemd-resolved); the backend has said why.
    fn take_dns(&self, dev: &Opened, ipv6: bool) -> Result<bool, String>;
    /// Gives back what goes away with the device anyway (resolved's settings
    /// of the link): best effort. What outlives rurge goes back by `undo`.
    fn give_dns_back(&self, dev: &Opened) -> Result<(), String>;
    /// Linux: `Some` when `rp_filter` has to be loosened for the network's
    /// primary interface, with what to put back; `None` when it need not be,
    /// or when `saved` has it already.
    fn adjust_undo(&self, primary: Option<&str>, saved: &[Undo]) -> Result<Option<Undo>, String>;
    /// Loosens what `adjust_undo` asked for, or what `saved` holds already —
    /// written again then, so that another program applying the system's
    /// settings over it is undone by the next change of network. Not called
    /// when neither says so (the phase 3 / M4c plan's execution record row 40).
    fn adjust(&self) -> Result<(), String>;
    /// Puts one saved thing back: on a close, and after a crash.
    fn undo(&self, undo: &Undo) -> Result<(), String>;
    /// The DNS servers of the network outside the enhanced mode, for
    /// `server:system` while it is on (design 6.3); `None`: the system's own list.
    fn physical_dns(
        &self,
        primary: Option<&str>,
        saved: &[Undo],
        network_changed: bool,
    ) -> Option<Vec<SocketAddr>>;
    fn close(&self, dev: Opened);
}

/// The enhanced mode's backend, unless `RURGE_ENHANCED_BACKEND` says otherwise.
/// An unknown value is an error: silently falling back would let a typo in a
/// test take over the machine's routes and DNS. `data_dir` is where the
/// platform's scripts go (its `enhanced/` directory, M4c plan P12).
pub fn backend(data_dir: &Path) -> anyhow::Result<Arc<dyn Backend>> {
    match std::env::var_os(BACKEND_ENV) {
        None => Ok(Arc::new(PlatformBackend {
            // `wintun.dll` is looked for next to rurge.exe; `--wintun-dll`
            // gives its place instead (the M4d plan's task 6)
            system: rurge_platform::enhanced::system(data_dir.join("enhanced"), None),
        })),
        Some(value) => backend_from(&value.to_string_lossy()),
    }
}

fn backend_from(value: &str) -> anyhow::Result<Arc<dyn Backend>> {
    if value == "off" {
        return Ok(Arc::new(Off));
    }
    match value.strip_prefix("file:") {
        Some(path) if !path.is_empty() => Ok(Arc::new(FileBackend::new(path.into()))),
        _ => anyhow::bail!("unknown {BACKEND_ENV} value `{value}` (expected off or file:<path>)"),
    }
}

/// The platform's own name for what an undo puts back (M4 design 9.2): the two
/// kinds both platforms save. `None` for the one this version's system never
/// saves — `Undo::Test`, the file backend's own step.
fn saved_of(undo: &Undo) -> Option<Saved> {
    match undo {
        Undo::MacDns { service, servers } => Some(Saved::MacDns {
            service: service.clone(),
            servers: servers.clone(),
        }),
        Undo::RpFilter { value } => Some(Saved::RpFilter { value: *value }),
        Undo::Test { .. } => None,
    }
}

/// The manager's name for a saved change: what `state.json`'s backup keeps.
fn undo_of(saved: &Saved) -> Undo {
    match saved {
        Saved::MacDns { service, servers } => Undo::MacDns {
            service: service.clone(),
            servers: servers.clone(),
        },
        Saved::RpFilter { value } => Undo::RpFilter { value: *value },
    }
}

/// The system's own backend: `rurge-platform`'s `System` of this platform,
/// which changes the machine where it has one (M4c) and says it has none
/// elsewhere (M4d gives Windows its own). No `cfg`: `system()` picks it.
struct PlatformBackend {
    system: Box<dyn System>,
}

impl PlatformBackend {
    /// The adapter as the steps name it.
    fn adapter(dev: &Opened) -> Adapter {
        Adapter {
            name: dev.name.clone(),
            index: dev.index,
        }
    }
}

impl Backend for PlatformBackend {
    fn open(&self) -> Result<Opened, String> {
        let tun = self.system.open().map_err(|e| e.to_string())?;
        let adapter = tun.adapter().clone();
        Ok(Opened {
            device: Arc::new(PlatformDevice::new(tun)),
            name: adapter.name,
            index: adapter.index,
        })
    }
    fn configure(&self, dev: &Opened, ipv6: bool) -> Result<bool, String> {
        self.system.configure(&Self::adapter(dev), ipv6)
    }
    fn configure_ipv6(&self, dev: &Opened) -> Result<bool, String> {
        self.system.configure_ipv6(&Self::adapter(dev))
    }
    fn unconfigure_ipv6(&self, dev: &Opened) -> Result<(), String> {
        self.system.unconfigure_ipv6(&Self::adapter(dev))
    }
    fn routes(&self, dev: &Opened, add: &RouteSet, remove: &RouteSet) -> Result<bool, String> {
        self.system.routes(&Self::adapter(dev), add, remove)
    }
    fn subnets(&self, exclude: &[String]) -> Result<Vec<(IpNet, String)>, String> {
        // the one action that reads the machine without a device: a list of
        // interfaces and addresses
        rurge_platform::socket::egress::subnets(exclude).map_err(|e| e.to_string())
    }
    fn dns_undo(&self, dev: &Opened, saved: &[Undo]) -> Result<Vec<Undo>, String> {
        let saved: Vec<Saved> = saved.iter().filter_map(saved_of).collect();
        self.system
            .dns_saved(&Self::adapter(dev), &saved)
            .map(|saved| saved.iter().map(undo_of).collect())
    }
    fn take_dns(&self, dev: &Opened, ipv6: bool) -> Result<bool, String> {
        self.system.take_dns(&Self::adapter(dev), ipv6)
    }
    fn give_dns_back(&self, dev: &Opened) -> Result<(), String> {
        self.system.give_dns_back(&Self::adapter(dev))
    }
    fn adjust_undo(&self, primary: Option<&str>, saved: &[Undo]) -> Result<Option<Undo>, String> {
        let saved: Vec<Saved> = saved.iter().filter_map(saved_of).collect();
        self.system
            .adjust_saved(primary, &saved)
            .map(|saved| saved.as_ref().map(undo_of))
    }
    fn adjust(&self) -> Result<(), String> {
        self.system.adjust()
    }
    fn undo(&self, undo: &Undo) -> Result<(), String> {
        match saved_of(undo) {
            Some(saved) => self.system.undo(&saved),
            None => {
                // A kind no platform saves (a data directory brought over from
                // the file backend): keeping it would make every start fail on
                // it (M4c plan P9).
                tracing::warn!("an undo the platform backend does not know is dropped");
                Ok(())
            }
        }
    }
    fn physical_dns(
        &self,
        primary: Option<&str>,
        saved: &[Undo],
        network_changed: bool,
    ) -> Option<Vec<SocketAddr>> {
        let saved: Vec<Saved> = saved.iter().filter_map(saved_of).collect();
        self.system
            .physical_dns(primary, &saved, network_changed)
            .map(|servers| {
                servers
                    .into_iter()
                    .map(|ip| SocketAddr::new(ip, 53))
                    .collect()
            })
    }
    fn close(&self, dev: Opened) {
        // The manager's close stops the TUN first and waits for it at most
        // `JOIN_LIMIT` (`cli/enhanced`), and it takes the device out of what
        // it held instead of cloning it (the phase 3 / M4c plan's execution
        // record row 43): when the TUN has ended, this is the last owner of
        // the device, and dropping it closes the descriptor — the adapter goes
        // with it. When it did not end within that limit, the TUN's tasks
        // still hold an `Arc`, and the device closes when they let go.
        drop(dev);
    }
}

/// The enhanced mode with the backend taken away: `RURGE_ENHANCED_BACKEND=off`,
/// which is what the tests' helpers give every `rurge` they start.
struct Off;

impl Backend for Off {
    fn open(&self) -> Result<Opened, String> {
        Err(OFF.to_string())
    }
    fn configure(&self, _dev: &Opened, _ipv6: bool) -> Result<bool, String> {
        Err(OFF.to_string())
    }
    fn configure_ipv6(&self, _dev: &Opened) -> Result<bool, String> {
        Err(OFF.to_string())
    }
    fn unconfigure_ipv6(&self, _dev: &Opened) -> Result<(), String> {
        Err(OFF.to_string())
    }
    fn routes(&self, _dev: &Opened, _add: &RouteSet, _remove: &RouteSet) -> Result<bool, String> {
        Err(OFF.to_string())
    }
    fn subnets(&self, _exclude: &[String]) -> Result<Vec<(IpNet, String)>, String> {
        Ok(Vec::new())
    }
    fn dns_undo(&self, _dev: &Opened, _saved: &[Undo]) -> Result<Vec<Undo>, String> {
        Err(OFF.to_string())
    }
    fn take_dns(&self, _dev: &Opened, _ipv6: bool) -> Result<bool, String> {
        Err(OFF.to_string())
    }
    fn give_dns_back(&self, _dev: &Opened) -> Result<(), String> {
        Err(OFF.to_string())
    }
    fn adjust_undo(&self, _primary: Option<&str>, _saved: &[Undo]) -> Result<Option<Undo>, String> {
        Err(OFF.to_string())
    }
    fn adjust(&self) -> Result<(), String> {
        Err(OFF.to_string())
    }
    fn undo(&self, _undo: &Undo) -> Result<(), String> {
        Err(OFF.to_string())
    }
    fn physical_dns(
        &self,
        _primary: Option<&str>,
        _saved: &[Undo],
        _network_changed: bool,
    ) -> Option<Vec<SocketAddr>> {
        None
    }
    fn close(&self, _dev: Opened) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::general::{RURGE_SPACE_V4, RURGE_SPACE_V6};
    use rurge_platform::route::{SPACE_V4, SPACE_V6};
    use std::net::IpAddr;

    /// The two kinds both platforms save map to one another in both
    /// directions, so that what `state.json` keeps survives a round trip and
    /// what a platform hands back is what the manager puts in the backup. The
    /// file backend's own step is not one of them.
    #[test]
    fn undo_and_saved_map_both_ways() {
        for undo in [
            Undo::MacDns {
                service: "Wi-Fi".to_string(),
                servers: vec!["9.9.9.9".parse().unwrap(), "192.0.2.53".parse().unwrap()],
            },
            Undo::RpFilter { value: 2 },
        ] {
            let saved = saved_of(&undo).expect("a platform's own undo");
            assert_eq!(undo_of(&saved), undo, "{undo:?}");
        }
        let test = Undo::Test {
            step: "take_dns".to_string(),
        };
        assert_eq!(saved_of(&test), None, "the file backend's step is not ours");
    }

    /// An undo of the file backend's kind is dropped rather than handed to a
    /// system that would fail it on every start (M4c plan P9): the platform
    /// backend answers `Ok(())` without asking the system anything — were it
    /// handed on, `Unsupported` would fail it — and says so once, at WARN.
    #[test]
    fn a_test_undo_on_the_platform_backend_is_dropped() {
        use rurge_net::testing::Lines;

        let lines = Lines::default();
        let _capturing = lines.capture();
        let backend = PlatformBackend {
            system: Box::new(rurge_platform::enhanced::Unsupported),
        };
        let undo = Undo::Test {
            step: "take_dns".to_string(),
        };
        assert_eq!(backend.undo(&undo), Ok(()));
        let said = lines.said("an undo the platform backend does not know is dropped");
        assert_eq!(said.len(), 1, "one WARN: {said:?}");
    }

    #[test]
    fn the_off_backend_says_it_is_off() {
        let Err(error) = Off.open() else {
            panic!("the off backend opened a device")
        };
        assert_eq!(error, OFF);
        assert!(OFF.contains("RURGE_ENHANCED_BACKEND=off"), "{OFF}");
        assert!(
            Off.subnets(&[]).unwrap().is_empty(),
            "no subnets without any system read"
        );
    }

    /// A value the variable does not know never becomes the platform's: it is
    /// the error that ends the start, with the two spellings it does know.
    #[test]
    fn an_unknown_value_never_falls_back_and_the_two_known_ones_are_taken() {
        assert!(backend_from("off").is_ok());
        assert!(backend_from("file:/tmp/enhanced.json").is_ok());
        for value in ["", "bogus", "platform", "file:", "file", "OFF"] {
            let Err(backend) = backend_from(value) else {
                panic!("{value} became a backend")
            };
            let error = backend.to_string();
            assert!(
                error.contains(BACKEND_ENV) && error.contains("off or file:<path>"),
                "{value}: {error}"
            );
        }
    }

    /// rurge's space is one and the same everywhere: the platform's constants,
    /// the copy the profile's checks read, and the test the DNS pool makes.
    #[test]
    fn rurges_space_is_one_and_the_same_everywhere() {
        assert_eq!(SPACE_V4, RURGE_SPACE_V4);
        assert_eq!(SPACE_V6, RURGE_SPACE_V6);
        let in_space = |text: &str| rurge_dns::fakeip::in_space(text.parse::<IpAddr>().unwrap());
        for (address, inside) in [
            ("198.17.255.255", false),
            ("198.18.0.0", true),
            ("198.19.255.255", true),
            ("198.20.0.0", false),
            ("fd00:6151:ffff:ffff:ffff:ffff:ffff:ffff", false),
            ("fd00:6152::", true),
            ("fd00:6152:0:0:ffff:ffff:ffff:ffff", true),
            ("fd00:6152:0:1::", false),
            ("fd00:6153::", false),
        ] {
            let ip = address.parse::<IpAddr>().unwrap();
            let in_constants = if ip.is_ipv4() {
                match ip {
                    IpAddr::V4(v4) => SPACE_V4.contains(&v4),
                    IpAddr::V6(_) => unreachable!(),
                }
            } else {
                match ip {
                    IpAddr::V6(v6) => SPACE_V6.contains(&v6),
                    IpAddr::V4(_) => unreachable!(),
                }
            };
            assert_eq!(in_space(address), inside, "{address}");
            assert_eq!(in_constants, inside, "{address}");
        }
        // and a fake address of the pool's first block is inside all of them
        for address in ["198.18.0.1", "198.19.255.254", "fd00:6152::1"] {
            let ip = address.parse::<IpAddr>().unwrap();
            assert_eq!(
                rurge_dns::fakeip::in_space(ip),
                match ip {
                    IpAddr::V4(v4) => SPACE_V4.contains(&v4),
                    IpAddr::V6(v6) => SPACE_V6.contains(&v6),
                },
                "{address}"
            );
        }
    }
}
