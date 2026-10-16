//! The network the machine is on now (phase 3 / M1 design §3): the interface
//! the default route leaves by with its gateways and kind, the DNS servers, the
//! Wi-Fi link's SSID and BSSID, and hints that it may have changed.
//!
//! Best effort and silent: what the system does not say, or says in a way that
//! is not understood, is absent — no primary interface, no Wi-Fi link, no name
//! — and nothing fails. As in `sysproxy` and `sysinfo`, what each platform
//! reads is compiled and unit-tested on every host: the readers take a
//! `Host` (programs, files, the interface table), and only the real one
//! touches the system.
//!
//! This crate knows no other crate of rurge, so the types here are its own;
//! the executable turns a [`Snapshot`] into what the engine looks at.

mod host;
mod linux;
mod macos;
mod parse;
mod watch;
mod windows;

use crate::dirs::Os;
use host::{Host, RealHost};
use macos::PortCache;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv6Addr};
use std::path::{Path, PathBuf};

pub use watch::hints;

/// `file:<path>` swaps the machine for a JSON file (end-to-end tests).
pub const BACKEND_ENV: &str = "RURGE_NETWORK_BACKEND";

/// What kind of link the primary interface is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetKind {
    Wifi,
    Wired,
    /// Anything else: a tunnel, a modem, an interface of a kind the platform
    /// would not name.
    #[default]
    Other,
}

/// The interface the traffic leaves by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Primary {
    pub interface: String,
    #[serde(default)]
    pub kind: NetKind,
    /// The default gateways of the interface: the IPv4 one first, then the
    /// IPv6 one.
    #[serde(default)]
    pub routers: Vec<IpAddr>,
}

/// A Wi-Fi adapter that is associated with an access point.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wifi {
    pub interface: String,
    /// `None`: the platform would not say (no permission to see it, or it is
    /// redacted).
    #[serde(default)]
    pub ssid: Option<String>,
    #[serde(default)]
    pub bssid: Option<String>,
}

/// The network as far as it is known. The SSID, the BSSID, the gateways and
/// the addresses say where the machine is: they are for the debug log at
/// most. `Snapshot::default()` knows nothing.
///
/// This is also the shape of the file of the test backend (`file:<path>`);
/// every field may be left out of it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    /// The interface of the IPv4 default route — of the IPv6 one when there is
    /// no IPv4 default route; `None` when there is neither.
    pub primary: Option<Primary>,
    /// The Wi-Fi link, when an adapter is associated with an access point.
    pub wifi: Option<Wifi>,
    /// The DNS servers of the system, the first one the preferred.
    pub dns: Vec<IpAddr>,
    /// What says which network the primary interface is on: its IPv4
    /// addresses, and the /64 prefix of its IPv6 addresses that are not
    /// link-local — the rest of a privacy address changes by itself every few
    /// hours, and that is no change of network. Sorted, each once.
    pub addresses: Vec<IpAddr>,
}

impl Snapshot {
    /// What [`read_quick`] would say of a network that is this one.
    fn quick(&self) -> Quick {
        Quick {
            primary: self.primary.clone(),
            dns: self.dns.clone(),
            addresses: self.addresses.clone(),
            wireless: self
                .wifi
                .iter()
                .map(|wifi| wifi.interface.clone())
                .collect(),
        }
    }
}

/// What the cheap sources say about the network: enough to tell that it has
/// changed, not what the Wi-Fi link is called. Two of them are equal when the
/// system has not told anything different.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Quick {
    pub(crate) primary: Option<Primary>,
    pub(crate) dns: Vec<IpAddr>,
    pub(crate) addresses: Vec<IpAddr>,
    /// The wireless interfaces that are up — macOS: that there are —, where
    /// the Wi-Fi link is looked for. Sorted.
    pub(crate) wireless: Vec<String>,
}

/// What [`read`] and [`read_quick`] leave out.
#[derive(Clone, Debug, Default)]
pub struct ReadOpts {
    /// Interfaces that are neither the primary interface nor the Wi-Fi link,
    /// whatever the system says of them. (The virtual adapter of the enhanced
    /// mode is one; until there is one the list is empty.) The file backend
    /// has no use for it: its file says what the network is.
    pub exclude: Vec<String>,
}

/// Where the network comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    /// The machine.
    Platform,
    /// A JSON file in the shape of [`Snapshot`], which is read at every look
    /// and never touches the machine. Not there, or not understood, is a
    /// network that is not known.
    File(PathBuf),
}

/// The backend `RURGE_NETWORK_BACKEND` asks for: the machine when it is not
/// set. A value that is not `file:<path>` is an error: falling back to the
/// machine would let a typo in a test read the real network.
pub fn backend_from_env() -> Result<Backend, String> {
    match std::env::var_os(BACKEND_ENV) {
        None => Ok(Backend::Platform),
        Some(value) => parse_backend(&value.to_string_lossy()),
    }
}

/// The backend a value names: `file:<path>`, and nothing else. The value is
/// the one `RURGE_NETWORK_BACKEND` holds, or the one the executable is given by
/// an option that is backed by the variable: an unknown one is refused in the
/// same words either way.
pub fn parse_backend(value: &str) -> Result<Backend, String> {
    match value.strip_prefix("file:") {
        Some(path) if !path.is_empty() => Ok(Backend::File(PathBuf::from(path))),
        _ => Err(format!(
            "unknown {BACKEND_ENV} value `{value}` (expected file:<path>)"
        )),
    }
}

/// Reads the network. Blocking: it may run programs, each for two seconds at
/// most.
pub fn read(backend: &Backend, opts: &ReadOpts) -> Snapshot {
    match backend {
        Backend::Platform => read_on(Os::current(), &RealHost, opts, &macos::PORTS),
        Backend::File(path) => file_snapshot(path),
    }
}

/// Reads the cheap sources only — on Windows and Linux no program is run —,
/// for telling whether the network has changed.
pub fn read_quick(backend: &Backend, opts: &ReadOpts) -> Quick {
    match backend {
        Backend::Platform => quick_on(Os::current(), &RealHost, opts, &macos::PORTS),
        Backend::File(path) => file_snapshot(path).quick(),
    }
}

fn quick_on(os: Os, host: &dyn Host, opts: &ReadOpts, ports: &PortCache) -> Quick {
    match os {
        Os::Windows => windows::quick(host, opts),
        Os::MacOs => macos::quick(host, opts, ports),
        Os::Unix => linux::quick(host, opts),
    }
}

fn read_on(os: Os, host: &dyn Host, opts: &ReadOpts, ports: &PortCache) -> Snapshot {
    let quick = quick_on(os, host, opts, ports);
    let wifi = match os {
        Os::Windows => windows::wifi(host, &quick),
        Os::MacOs => macos::wifi(host, &quick),
        Os::Unix => linux::wifi(host, &quick),
    };
    Snapshot {
        primary: quick.primary,
        wifi,
        dns: quick.dns,
        addresses: quick.addresses,
    }
}

fn file_snapshot(path: &Path) -> Snapshot {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            // a file that is not there is how a test says the network is not known
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!(reason = ?e.kind(), "cannot read the network file");
            }
            return Snapshot::default();
        }
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        // where, not what: the text may be a name that is not to be logged
        tracing::debug!(
            category = ?e.classify(),
            line = e.line(),
            column = e.column(),
            "the network file is not a network"
        );
        Snapshot::default()
    })
}

/// What says which network an interface is on, out of its addresses: the IPv4
/// ones, and the /64 prefix of the IPv6 ones that are not link-local. Sorted,
/// each once.
fn network_addresses(addresses: impl IntoIterator<Item = IpAddr>) -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = addresses
        .into_iter()
        .filter_map(|ip| match ip {
            IpAddr::V4(v4) => (!v4.is_loopback() && !v4.is_unspecified()).then_some(ip),
            IpAddr::V6(v6) => {
                let kept = !v6.is_loopback()
                    && !v6.is_unspecified()
                    && !v6.is_unicast_link_local()
                    && !v6.is_multicast();
                kept.then(|| {
                    // the first 64 bits are the network, the rest the interface
                    let mut segments = v6.segments();
                    segments[4..].fill(0);
                    IpAddr::V6(Ipv6Addr::from(segments))
                })
            }
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The addresses of a list, each once, in the order they come.
fn unique_in_order(addresses: &[IpAddr]) -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::new();
    for address in addresses {
        if !out.contains(address) {
            out.push(*address);
        }
    }
    out
}

/// The wireless interfaces with the primary one, when it is one of them,
/// first.
fn primary_first(wireless: &[String], primary: Option<&str>) -> Vec<String> {
    let mut ordered = wireless.to_vec();
    if let Some(at) = ordered
        .iter()
        .position(|name| Some(name.as_str()) == primary)
    {
        let first = ordered.remove(at);
        ordered.insert(0, first);
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            primary: Some(Primary {
                interface: "en0".to_string(),
                kind: NetKind::Wifi,
                routers: vec![ip("192.0.2.1"), ip("fe80::1")],
            }),
            wifi: Some(Wifi {
                interface: "en0".to_string(),
                ssid: Some("ExampleNet".to_string()),
                bssid: None,
            }),
            dns: vec![ip("192.0.2.53")],
            addresses: vec![ip("192.0.2.10")],
        }
    }

    #[test]
    fn the_file_backend_reads_what_the_test_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("network.json");
        std::fs::write(&path, serde_json::to_string(&snapshot()).unwrap()).unwrap();
        let backend = Backend::File(path.clone());
        assert_eq!(read(&backend, &ReadOpts::default()), snapshot());

        // by hand, with only what the test cares about
        std::fs::write(
            &path,
            r#"{"primary": {"interface": "eth0", "kind": "wired", "routers": ["10.0.0.1"]}}"#,
        )
        .unwrap();
        assert_eq!(
            read(&backend, &ReadOpts::default()),
            Snapshot {
                primary: Some(Primary {
                    interface: "eth0".to_string(),
                    kind: NetKind::Wired,
                    routers: vec![ip("10.0.0.1")],
                }),
                ..Snapshot::default()
            }
        );
        // a primary interface that says nothing but its name is of no kind and has no router
        std::fs::write(
            &path,
            r#"{"primary": {"interface": "tun0"}, "wifi": {"interface": "wlan0"}}"#,
        )
        .unwrap();
        let read = read(&backend, &ReadOpts::default());
        assert_eq!(
            read.primary.map(|p| (p.kind, p.routers)),
            Some((NetKind::Other, vec![]))
        );
        assert_eq!(
            read.wifi,
            Some(Wifi {
                interface: "wlan0".to_string(),
                ssid: None,
                bssid: None
            })
        );
    }

    /// The file is read at every look, so a test changes the network by
    /// writing it; the cheap look sees the same change.
    #[test]
    fn the_file_backend_follows_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("network.json");
        let backend = Backend::File(path.clone());
        let opts = ReadOpts::default();
        std::fs::write(&path, serde_json::to_string(&snapshot()).unwrap()).unwrap();
        let before = read_quick(&backend, &opts);
        assert_eq!(before, read_quick(&backend, &opts));
        let mut other = snapshot();
        other.wifi.as_mut().unwrap().ssid = Some("OtherNet".to_string());
        other.dns = vec![ip("192.0.2.54")];
        std::fs::write(&path, serde_json::to_string(&other).unwrap()).unwrap();
        assert_eq!(read(&backend, &opts), other);
        assert_ne!(before, read_quick(&backend, &opts));
    }

    #[test]
    fn a_missing_file_is_an_unknown_network() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("network.json");
        let backend = Backend::File(path.clone());
        assert_eq!(read(&backend, &ReadOpts::default()), Snapshot::default());
        assert_eq!(read_quick(&backend, &ReadOpts::default()), Quick::default());
        // a file that is not a network is the same
        for text in [
            "",
            "not json",
            "[1, 2]",
            r#"{"primary": {"kind": "wired"}}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(
                read(&backend, &ReadOpts::default()),
                Snapshot::default(),
                "{text}"
            );
        }
    }

    #[test]
    fn an_unknown_backend_value_is_refused() {
        for value in ["bogus", "file", "file:", "FILE:/x.json", "platform", ""] {
            let error = parse_backend(value).unwrap_err();
            assert!(error.contains("file:<path>"), "{value}: {error}");
            assert!(error.contains(BACKEND_ENV), "{value}: {error}");
        }
        assert_eq!(
            parse_backend("file:/tmp/n.json"),
            Ok(Backend::File(PathBuf::from("/tmp/n.json")))
        );
        // everything after the first `file:` is the path
        assert_eq!(
            parse_backend("file:C:\\n\\n.json"),
            Ok(Backend::File(PathBuf::from("C:\\n\\n.json")))
        );
    }

    #[test]
    fn the_addresses_that_say_which_network_it_is_leave_out_what_changes_by_itself() {
        let list = [
            "2001:db8:0:7:1111:2222:3333:4444",
            "192.0.2.10",
            "fe80::1",
            "::1",
            "2001:db8:0:7:5555:6666:7777:8888",
            "192.0.2.10",
            "fd00:0:0:9:a:b:c:d",
            "198.51.100.7",
            "::",
        ]
        .map(ip);
        assert_eq!(
            network_addresses(list),
            [
                "192.0.2.10",
                "198.51.100.7",
                "2001:db8:0:7::",
                "fd00:0:0:9::"
            ]
            .map(ip)
        );
        assert_eq!(network_addresses([]), Vec::<IpAddr>::new());
    }

    fn adapter(name: &str, kind: windows::AdapterKind, gateway: &str) -> windows::AdapterView {
        windows::AdapterView {
            name: name.to_string(),
            kind,
            up: true,
            gateways: vec![ip(gateway)],
            dns: vec![ip("192.0.2.53")],
            addresses: vec![ip("192.0.2.10")],
            metric_v4: 25,
            metric_v6: 25,
        }
    }

    /// The reader of a platform is chosen by the platform it is asked to read,
    /// not by the one the test runs on: the whole of each is a snapshot here.
    #[test]
    fn each_platform_is_read_by_its_own_reader() {
        use host::testing::FakeHost;

        let wifi = |interface: &str| Wifi {
            interface: interface.to_string(),
            ssid: Some("Example Net".to_string()),
            bssid: Some("00:00:5e:00:53:02".to_string()),
        };
        let expected = |interface: &str, kind: NetKind, wifi: Option<Wifi>| Snapshot {
            primary: Some(Primary {
                interface: interface.to_string(),
                kind,
                routers: vec![ip("192.0.2.1")],
            }),
            wifi,
            dns: vec![ip("192.0.2.53")],
            addresses: vec![ip("192.0.2.10")],
        };

        let windows = FakeHost::default()
            .with_adapter(adapter("Wi-Fi", windows::AdapterKind::Wifi, "192.0.2.1"))
            .with_command(
                &windows::wlan_ask(None).line(),
                "    Name : Wi-Fi\n    SSID : Example Net\n    BSSID : 00:00:5e:00:53:02\n",
            );
        assert_eq!(
            read_on(
                Os::Windows,
                &windows,
                &ReadOpts::default(),
                &PortCache::new()
            ),
            expected("Wi-Fi", NetKind::Wifi, Some(wifi("Wi-Fi")))
        );

        let linux = FakeHost::default()
            .with_file(
                "/proc/net/route",
                "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\n\
                 wlan0\t00000000\t010200C0\t0003\t0\t0\t10\t00000000\n",
            )
            .with_file("/sys/class/net/wlan0/wireless", "")
            .with_file("/sys/class/net/wlan0/operstate", "up\n")
            .with_file("/etc/resolv.conf", "nameserver 192.0.2.53\n")
            .with_address("wlan0", "192.0.2.10")
            .with_command(
                "iw dev wlan0 link",
                "Connected to 00:00:5e:00:53:02 (on wlan0)\n\tSSID: Example Net\n",
            );
        assert_eq!(
            read_on(Os::Unix, &linux, &ReadOpts::default(), &PortCache::new()),
            expected("wlan0", NetKind::Wifi, Some(wifi("wlan0")))
        );

        let mac = FakeHost::default()
            .with_command(
                "route -n get default",
                "    gateway: 192.0.2.1\n  interface: en0\n",
            )
            .with_command(
                "networksetup -listallhardwareports",
                "Hardware Port: Wi-Fi\nDevice: en0\n",
            )
            .with_command(
                "ipconfig getsummary en0",
                "  SSID : Example Net\n  BSSID : 0:0:5e:0:53:2\n",
            )
            .with_file("/etc/resolv.conf", "nameserver 192.0.2.53\n")
            .with_address("en0", "192.0.2.10");
        assert_eq!(
            read_on(Os::MacOs, &mac, &ReadOpts::default(), &PortCache::new()),
            expected("en0", NetKind::Wifi, Some(wifi("en0")))
        );

        // A machine that is one platform's is nothing to the readers of the
        // others (`resolv.conf` is the one thing macOS and Linux share).
        let nothing = Snapshot::default();
        assert_eq!(
            read_on(Os::Windows, &linux, &ReadOpts::default(), &PortCache::new()),
            nothing
        );
        for os in [Os::Unix, Os::MacOs] {
            assert_eq!(
                read_on(os, &windows, &ReadOpts::default(), &PortCache::new()),
                nothing,
                "{os:?}"
            );
        }
    }

    /// What holds on any machine, whatever its network is. Read-only: on
    /// Windows a Wi-Fi adapter that is up has `netsh wlan show interfaces` run,
    /// on the others the programs of the platform that only look.
    fn holds_anywhere(snapshot: &Snapshot) {
        if let Some(primary) = &snapshot.primary {
            assert!(!primary.interface.is_empty(), "{primary:?}");
            assert!(
                primary
                    .routers
                    .iter()
                    .all(|router| !router.is_unspecified()),
                "{primary:?}"
            );
        }
        if let Some(wifi) = &snapshot.wifi {
            assert!(!wifi.interface.is_empty(), "{wifi:?}");
        }
        // sorted, each once, and the interface-specific half of an IPv6 address is not in them
        assert!(
            snapshot.addresses.windows(2).all(|pair| pair[0] < pair[1]),
            "{:?}",
            snapshot.addresses
        );
        for address in &snapshot.addresses {
            if let IpAddr::V6(v6) = address {
                assert_eq!(v6.segments()[4..], [0; 4], "{v6}");
                assert!(!v6.is_unicast_link_local(), "{v6}");
            }
        }
        let mut dns = snapshot.dns.clone();
        dns.dedup();
        assert_eq!(dns, snapshot.dns);
    }

    #[test]
    fn reading_this_machine_does_not_panic() {
        let opts = ReadOpts::default();
        let first = read(&Backend::Platform, &opts);
        holds_anywhere(&first);
        let _ = read_quick(&Backend::Platform, &opts);
        // What is left out never appears: not as the primary interface, not as
        // the Wi-Fi link.
        if let Some(primary) = &first.primary {
            let opts = ReadOpts {
                exclude: vec![primary.interface.clone()],
            };
            let second = read(&Backend::Platform, &opts);
            holds_anywhere(&second);
            assert!(
                second
                    .primary
                    .as_ref()
                    .is_none_or(|other| other.interface != primary.interface),
                "{second:?}"
            );
            assert!(
                second
                    .wifi
                    .as_ref()
                    .is_none_or(|other| other.interface != primary.interface),
                "{second:?}"
            );
        }
    }

    #[test]
    fn the_primary_interface_is_the_first_to_be_asked_about_the_wifi() {
        let wireless = ["wlan0", "wlan1", "wlan2"].map(String::from);
        assert_eq!(
            primary_first(&wireless, Some("wlan1")),
            ["wlan1", "wlan0", "wlan2"]
        );
        assert_eq!(
            primary_first(&wireless, Some("eth0")),
            ["wlan0", "wlan1", "wlan2"]
        );
        assert_eq!(primary_first(&wireless, None), ["wlan0", "wlan1", "wlan2"]);
        assert_eq!(primary_first(&[], Some("eth0")), Vec::<String>::new());
    }
}
