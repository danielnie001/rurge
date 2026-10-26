//! What the manager asks of the system (M4 design 5.1, 9.1): abstract actions
//! only, each of them blocking, and the backends that answer them.
//!
//! Nothing here runs a program or changes anything: in this version the
//! platform backend answers every action with the same "not yet", and only the
//! file backend (`file:<path>`, tests) does anything at all.

use super::file::FileBackend;
use super::undo::Undo;
use rurge_platform::route::RouteSet;
use rurge_tun::device::Device;
use std::sync::Arc;

/// `RURGE_ENHANCED_BACKEND`: unset → the platform's; `off`; `file:<path>`.
pub const BACKEND_ENV: &str = "RURGE_ENHANCED_BACKEND";

/// What the platform backend answers while it exists and does nothing, and
/// what `off` answers.
const NOT_YET: &str = "the enhanced mode is not available on this platform in this version";
const OFF: &str = "the enhanced mode backend is off (RURGE_ENHANCED_BACKEND=off)";

/// The device a backend opened: what the TUN reads and writes, and how the
/// system names it.
#[derive(Clone)]
pub struct Opened {
    pub device: Arc<dyn Device>,
    /// The interface's name: for the debug log and the egress table.
    pub name: String,
    /// Windows: the interface index the steps name the adapter by.
    #[allow(dead_code)] // wired up by M4d
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
    /// `Ok(false)`: the IPv6 half of `add` could not be added (warned).
    fn routes(&self, dev: &Opened, add: &RouteSet, remove: &RouteSet) -> Result<bool, String>;
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
    fn adjust(&self) -> Result<(), String>;
    /// Puts one saved thing back: on a close, and after a crash.
    fn undo(&self, undo: &Undo) -> Result<(), String>;
    fn close(&self, dev: Opened);
}

/// The enhanced mode's backend, unless `RURGE_ENHANCED_BACKEND` says otherwise.
/// An unknown value is an error: silently falling back would let a typo in a
/// test take over the machine's routes and DNS.
pub fn backend() -> anyhow::Result<Arc<dyn Backend>> {
    match std::env::var_os(BACKEND_ENV) {
        None => Ok(Arc::new(Platform)),
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

/// The system's own backend: M4c and M4d add the steps and the device. Until
/// then every action says so, and nothing here touches a machine.
struct Platform;

impl Backend for Platform {
    fn open(&self) -> Result<Opened, String> {
        Err(NOT_YET.to_string())
    }
    fn configure(&self, _dev: &Opened, _ipv6: bool) -> Result<bool, String> {
        Err(NOT_YET.to_string())
    }
    fn configure_ipv6(&self, _dev: &Opened) -> Result<bool, String> {
        Err(NOT_YET.to_string())
    }
    fn unconfigure_ipv6(&self, _dev: &Opened) -> Result<(), String> {
        Err(NOT_YET.to_string())
    }
    fn routes(&self, _dev: &Opened, _add: &RouteSet, _remove: &RouteSet) -> Result<bool, String> {
        Err(NOT_YET.to_string())
    }
    fn dns_undo(&self, _dev: &Opened, _saved: &[Undo]) -> Result<Vec<Undo>, String> {
        Err(NOT_YET.to_string())
    }
    fn take_dns(&self, _dev: &Opened, _ipv6: bool) -> Result<bool, String> {
        Err(NOT_YET.to_string())
    }
    fn give_dns_back(&self, _dev: &Opened) -> Result<(), String> {
        Err(NOT_YET.to_string())
    }
    fn adjust_undo(&self, _primary: Option<&str>, _saved: &[Undo]) -> Result<Option<Undo>, String> {
        Err(NOT_YET.to_string())
    }
    fn adjust(&self) -> Result<(), String> {
        Err(NOT_YET.to_string())
    }
    fn undo(&self, _undo: &Undo) -> Result<(), String> {
        Err(NOT_YET.to_string())
    }
    fn close(&self, _dev: Opened) {}
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
    fn close(&self, _dev: Opened) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::general::{RURGE_SPACE_V4, RURGE_SPACE_V6};
    use rurge_platform::route::{SPACE_V4, SPACE_V6};
    use std::net::IpAddr;

    /// This version's platform backend does nothing but say so: not one of its
    /// actions reaches a machine (M4a: the steps are data, the device is M4c's).
    #[test]
    fn the_platform_backend_is_not_available_in_this_version() {
        let backend = Platform;
        let Err(error) = backend.open() else {
            panic!("the platform backend opened a device")
        };
        assert_eq!(error, NOT_YET);
        let dev = Opened {
            device: Arc::new(Nothing),
            name: "rurge-test".to_string(),
            index: None,
        };
        let routes = RouteSet::default();
        assert_eq!(backend.configure(&dev, true).unwrap_err(), NOT_YET);
        assert_eq!(backend.configure_ipv6(&dev).unwrap_err(), NOT_YET);
        assert_eq!(backend.unconfigure_ipv6(&dev).unwrap_err(), NOT_YET);
        assert_eq!(backend.routes(&dev, &routes, &routes).unwrap_err(), NOT_YET);
        assert_eq!(backend.dns_undo(&dev, &[]).unwrap_err(), NOT_YET);
        assert_eq!(backend.take_dns(&dev, false).unwrap_err(), NOT_YET);
        assert_eq!(backend.give_dns_back(&dev).unwrap_err(), NOT_YET);
        assert_eq!(backend.adjust_undo(None, &[]).unwrap_err(), NOT_YET);
        assert_eq!(backend.adjust().unwrap_err(), NOT_YET);
        assert_eq!(
            backend.undo(&Undo::RpFilter { value: 1 }).unwrap_err(),
            NOT_YET
        );
        backend.close(dev);
    }

    /// A device of no use to anyone: `Platform` never opens one.
    struct Nothing;

    impl Device for Nothing {
        fn recv<'a>(
            &'a self,
            _into: &'a mut Vec<Vec<u8>>,
            _max: usize,
        ) -> rurge_net::BoxFuture<'a, std::io::Result<()>> {
            Box::pin(std::future::pending())
        }
        fn send<'a>(
            &'a self,
            _packets: &'a mut Vec<Vec<u8>>,
        ) -> rurge_net::BoxFuture<'a, std::io::Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn mtu(&self) -> usize {
            1500
        }
    }

    #[test]
    fn the_off_backend_says_it_is_off() {
        let Err(error) = Off.open() else {
            panic!("the off backend opened a device")
        };
        assert_eq!(error, OFF);
        assert!(OFF.contains("RURGE_ENHANCED_BACKEND=off"), "{OFF}");
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
