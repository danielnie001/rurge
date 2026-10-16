//! macOS: `route` for the default route, `networksetup` for what kind of
//! hardware an interface is, `ipconfig getsummary` for the Wi-Fi link.

use super::host::{Host, say_once};
use super::{
    NetKind, Primary, Quick, ReadOpts, Wifi, network_addresses, parse, primary_first,
    unique_in_order,
};
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

/// Each device with the kind of hardware port it is.
type Ports = Vec<(String, NetKind)>;

/// The table of hardware ports as it was read, and the interfaces there were
/// then.
struct Kept {
    interfaces: Vec<String>,
    ports: Ports,
}

/// The table of hardware ports changes only when an interface appears or goes
/// away, and a program has to be run to read it: it is read again then.
pub(crate) struct PortCache(Mutex<Option<Kept>>);

impl PortCache {
    pub(crate) const fn new() -> PortCache {
        PortCache(Mutex::new(None))
    }

    /// The table as it was read for these interfaces. `load` reads it when the
    /// interfaces are not the ones of the last look; a table that could not be
    /// read (an empty one) is not kept.
    fn get(&self, interfaces: &[String], load: impl FnOnce() -> Ports) -> Ports {
        let mut kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(kept) = kept.as_ref()
            && kept.interfaces == interfaces
        {
            return kept.ports.clone();
        }
        let ports = load();
        if !ports.is_empty() {
            *kept = Some(Kept {
                interfaces: interfaces.to_vec(),
                ports: ports.clone(),
            });
        }
        ports
    }
}

pub(crate) static PORTS: PortCache = PortCache::new();

/// The gateway and interface of a default route, unless the interface is left
/// out.
fn default_route(host: &dyn Host, cmd: &[&str], opts: &ReadOpts) -> Option<(String, IpAddr)> {
    host.run(cmd)
        .and_then(|text| parse::route_get_default(&text))
        .filter(|(interface, _)| !opts.exclude.contains(interface))
}

/// `route` names the best default route only: when its interface is left out
/// there is no next one to take, and the network of that kind is not known.
pub(crate) fn quick(host: &dyn Host, opts: &ReadOpts, ports: &PortCache) -> Quick {
    let v4 = default_route(host, &["route", "-n", "get", "default"], opts);
    let v6 = default_route(host, &["route", "-n", "get", "-inet6", "default"], opts);
    // the interface of the IPv4 default route, else of the IPv6 one
    let interface = v4
        .as_ref()
        .or(v6.as_ref())
        .map(|(interface, _)| interface.clone());
    let addresses = host.addresses();
    let mut interfaces: Vec<String> = addresses.iter().map(|(name, _)| name.clone()).collect();
    interfaces.sort();
    interfaces.dedup();
    let table = ports.get(&interfaces, || {
        host.run(&["networksetup", "-listallhardwareports"])
            .map(|text| parse::hardware_ports(&text))
            .unwrap_or_default()
    });
    let primary = interface.as_ref().map(|interface| {
        let routers = [&v4, &v6]
            .into_iter()
            .flatten()
            .filter(|(on, _)| on == interface)
            .map(|(_, gateway)| *gateway)
            .collect();
        Primary {
            kind: table
                .iter()
                .find(|(device, _)| device == interface)
                .map_or(NetKind::Other, |(_, kind)| *kind),
            interface: interface.clone(),
            routers,
        }
    });
    let dns = host.file("/etc/resolv.conf").map_or_else(Vec::new, |text| {
        unique_in_order(&crate::dns::parse_resolv_conf(&text).0)
    });
    // a Wi-Fi device that is off has no address and is not asked about
    let mut wireless: Vec<String> = table
        .iter()
        .filter(|(device, kind)| {
            *kind == NetKind::Wifi && interfaces.contains(device) && !opts.exclude.contains(device)
        })
        .map(|(device, _)| device.clone())
        .collect();
    wireless.sort();
    wireless.dedup();
    Quick {
        primary,
        dns,
        addresses: network_addresses(
            addresses
                .into_iter()
                .filter(|(name, _)| Some(name) == interface.as_ref())
                .map(|(_, ip)| ip),
        ),
        wireless,
    }
}

/// The Wi-Fi link: the first Wi-Fi device that is on a network, the primary
/// one asked first.
pub(crate) fn wifi(host: &dyn Host, quick: &Quick) -> Option<Wifi> {
    let primary = quick
        .primary
        .as_ref()
        .map(|primary| primary.interface.as_str());
    primary_first(&quick.wireless, primary)
        .into_iter()
        .find_map(|interface| {
            let link = host
                .run(&["ipconfig", "getsummary", &interface])
                .and_then(|text| parse::ipconfig_summary(&text, &interface))?;
            if link.ssid.is_none() {
                say_once("wifi-name", || {
                    tracing::debug!(
                        "a Wi-Fi device is on a network but macOS does not tell this program its \
                         name (it wants the permission to see where the machine is); SSID and \
                         BSSID conditions will not match"
                    );
                });
            }
            Some(Wifi {
                interface,
                ssid: link.ssid,
                bssid: link.bssid,
            })
        })
}

#[cfg(test)]
mod tests {
    use super::super::host::testing::FakeHost;
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn exclude(names: &[&str]) -> ReadOpts {
        ReadOpts {
            exclude: names.iter().map(|name| name.to_string()).collect(),
        }
    }

    fn route(gateway: &str, interface: &str) -> String {
        format!(
            "   route to: default\ndestination: default\n       mask: default\n    gateway: {gateway}\n  interface: {interface}\n      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING,GLOBAL>\n"
        )
    }

    const ROUTE_V4: &str = "route -n get default";
    const ROUTE_V6: &str = "route -n get -inet6 default";
    const PORTS_COMMAND: &str = "networksetup -listallhardwareports";
    const PORTS: &str = "\
Hardware Port: Wi-Fi\nDevice: en0\nEthernet Address: 00:00:5e:00:53:03\n\n\
Hardware Port: Ethernet Adapter (en5)\nDevice: en5\nEthernet Address: 00:00:5e:00:53:05\n\n\
Hardware Port: Wi-Fi 2\nDevice: en1\nEthernet Address: 00:00:5e:00:53:01\n\n\
VLAN Configurations\n===================\n";

    /// A Mac on Wi-Fi (`en0`), with an Ethernet adapter (`en5`) and a second
    /// Wi-Fi one (`en1`) that is not on a network.
    fn base() -> FakeHost {
        FakeHost::default()
            .with_command(ROUTE_V4, &route("192.0.2.1", "en0"))
            .with_command(ROUTE_V6, &route("fe80::1%en0", "en0"))
            .with_command(PORTS_COMMAND, PORTS)
            .with_file(
                "/etc/resolv.conf",
                "nameserver 192.0.2.53\nnameserver 2001:db8::53\n",
            )
            .with_address("lo0", "127.0.0.1")
            .with_address("en0", "192.0.2.10")
            .with_address("en0", "fe80::aaaa:1")
            .with_address("en0", "2001:db8:0:7:1111:2222:3333:4444")
            .with_address("en5", "198.51.100.10")
    }

    #[test]
    fn the_interface_of_the_default_route_is_primary() {
        let host = base();
        let quick = quick(&host, &ReadOpts::default(), &PortCache::new());
        assert_eq!(
            quick.primary,
            Some(Primary {
                interface: "en0".to_string(),
                kind: NetKind::Wifi,
                // the IPv4 gateway first, the IPv6 one without the interface it is reached by
                routers: vec![ip("192.0.2.1"), ip("fe80::1")],
            })
        );
        assert_eq!(quick.dns, [ip("192.0.2.53"), ip("2001:db8::53")]);
        assert_eq!(quick.addresses, [ip("192.0.2.10"), ip("2001:db8:0:7::")]);
        // the Wi-Fi ports that have an address: en1 has none (it is off)
        assert_eq!(quick.wireless, ["en0"]);
    }

    #[test]
    fn an_ipv6_default_route_decides_when_there_is_no_ipv4_one() {
        let host =
            base().with_command(ROUTE_V4, "route: writing to routing socket: not in table\n");
        let quick = quick(&host, &ReadOpts::default(), &PortCache::new());
        let primary = quick.primary.unwrap();
        assert_eq!(primary.interface, "en0");
        assert_eq!(primary.routers, [ip("fe80::1")]);
    }

    /// An IPv6 route that leaves by another interface is not a router of
    /// the primary one.
    #[test]
    fn a_gateway_on_another_interface_is_not_a_router_of_the_primary_one() {
        let host = base().with_command(ROUTE_V6, &route("fe80::2%en5", "en5"));
        let primary = quick(&host, &ReadOpts::default(), &PortCache::new())
            .primary
            .unwrap();
        assert_eq!(primary.interface, "en0");
        assert_eq!(primary.routers, [ip("192.0.2.1")]);
    }

    #[test]
    fn the_kind_of_an_interface_comes_from_its_hardware_port() {
        let host = base().with_command(ROUTE_V4, &route("198.51.100.1", "en5"));
        let primary = quick(&host, &ReadOpts::default(), &PortCache::new())
            .primary
            .unwrap();
        assert_eq!(
            (primary.interface.as_str(), primary.kind),
            ("en5", NetKind::Wired)
        );
        // a tunnel, or anything the table does not list
        let host = base().with_command(ROUTE_V4, &route("10.8.0.1", "utun4"));
        let primary = quick(&host, &ReadOpts::default(), &PortCache::new())
            .primary
            .unwrap();
        assert_eq!(primary.kind, NetKind::Other);
    }

    /// A program has to be run to read the table, and it changes only when an
    /// interface comes or goes.
    #[test]
    fn the_table_of_hardware_ports_is_read_again_only_when_the_interfaces_change() {
        let cache = PortCache::new();
        let asked = |host: &FakeHost| {
            host.calls()
                .iter()
                .filter(|call| call.as_str() == PORTS_COMMAND)
                .count()
        };
        let host = base();
        quick(&host, &ReadOpts::default(), &cache);
        quick(&host, &ReadOpts::default(), &cache);
        assert_eq!(asked(&host), 1);
        // an adapter is plugged in
        let host = base().with_address("en7", "203.0.113.10");
        quick(&host, &ReadOpts::default(), &cache);
        quick(&host, &ReadOpts::default(), &cache);
        assert_eq!(asked(&host), 1);
        // and the same machine as before again: the cache holds only the last
        let host = base();
        quick(&host, &ReadOpts::default(), &cache);
        assert_eq!(asked(&host), 1);
    }

    /// A table that could not be read is not kept: the next look tries again.
    #[test]
    fn a_table_that_could_not_be_read_is_asked_for_again() {
        let cache = PortCache::new();
        let host = FakeHost::default().with_address("en0", "192.0.2.10");
        quick(&host, &ReadOpts::default(), &cache);
        quick(&host, &ReadOpts::default(), &cache);
        assert_eq!(
            host.calls().iter().filter(|c| *c == PORTS_COMMAND).count(),
            2
        );
    }

    /// What is left out is not the primary interface. macOS names only the
    /// best default route, so there is no next one: the IPv6 route may be on
    /// another interface and then it is the primary one.
    #[test]
    fn an_excluded_interface_is_not_primary() {
        let host = base().with_command(ROUTE_V6, &route("fe80::2%en5", "en5"));
        let quick = quick(&host, &exclude(&["en0"]), &PortCache::new());
        let primary = quick.primary.unwrap();
        assert_eq!(primary.interface, "en5");
        assert_eq!(primary.routers, [ip("fe80::2")]);
        assert_eq!(quick.addresses, [ip("198.51.100.10")]);
        assert_eq!(quick.wireless, Vec::<String>::new());
        let all = quick_all_excluded();
        assert_eq!(all.primary, None);
    }

    fn quick_all_excluded() -> Quick {
        quick(&base(), &exclude(&["en0", "en5"]), &PortCache::new())
    }

    const SUMMARY: &str = "<dictionary> {\n  BSSID : 0:0:5e:0:53:2\n  InterfaceType : WiFi\n  SSID : Example Net\n}\n";

    #[test]
    fn the_wifi_link_is_read_from_the_summary_of_the_wifi_device() {
        let host = base().with_command("ipconfig getsummary en0", SUMMARY);
        let quick = quick(&host, &ReadOpts::default(), &PortCache::new());
        assert_eq!(
            wifi(&host, &quick),
            Some(Wifi {
                interface: "en0".to_string(),
                ssid: Some("Example Net".to_string()),
                bssid: Some("00:00:5e:00:53:02".to_string()),
            })
        );
    }

    /// macOS 14 and later show a program that may not see where the machine
    /// is `<redacted>`: the device is on a network, the name is not told.
    #[test]
    fn a_redacted_name_is_a_link_without_a_name() {
        let redacted = SUMMARY
            .replace("Example Net", "<redacted>")
            .replace("0:0:5e:0:53:2", "<redacted>");
        let host = base().with_command("ipconfig getsummary en0", &redacted);
        let quick = quick(&host, &ReadOpts::default(), &PortCache::new());
        assert_eq!(
            wifi(&host, &quick),
            Some(Wifi {
                interface: "en0".to_string(),
                ssid: None,
                bssid: None,
            })
        );
    }

    #[test]
    fn a_device_that_is_not_on_a_network_is_no_link() {
        let host = base().with_command(
            "ipconfig getsummary en0",
            "<dictionary> {\n  InterfaceType : WiFi\n}\n",
        );
        let quick = quick(&host, &ReadOpts::default(), &PortCache::new());
        assert_eq!(wifi(&host, &quick), None);
        // `ipconfig` does not answer: nothing is known
        let host = base();
        assert_eq!(wifi(&host, &quick), None);
    }

    /// With two Wi-Fi devices the primary one is asked first, and the first
    /// that is on a network is the link.
    #[test]
    fn the_wifi_device_of_the_primary_interface_is_asked_first() {
        let host = base()
            .with_address("en1", "203.0.113.10")
            .with_command(ROUTE_V4, &route("203.0.113.1", "en1"))
            .with_command("ipconfig getsummary en1", SUMMARY)
            .with_command(
                "ipconfig getsummary en0",
                SUMMARY.replace("Example Net", "Other Net").as_str(),
            );
        let quick = quick(&host, &ReadOpts::default(), &PortCache::new());
        assert_eq!(quick.wireless, ["en0", "en1"]);
        let link = wifi(&host, &quick).unwrap();
        assert_eq!(link.interface, "en1");
        assert_eq!(link.ssid.as_deref(), Some("Example Net"));
        let asked: Vec<String> = host
            .calls()
            .into_iter()
            .filter(|call| call.starts_with("ipconfig"))
            .collect();
        assert_eq!(asked, ["ipconfig getsummary en1"]);
    }

    #[test]
    fn without_a_wifi_device_with_an_address_no_summary_is_asked_for() {
        let host = FakeHost::default()
            .with_command(ROUTE_V4, &route("198.51.100.1", "en5"))
            .with_command(PORTS_COMMAND, PORTS)
            .with_address("en5", "198.51.100.10");
        let quick = quick(&host, &ReadOpts::default(), &PortCache::new());
        assert_eq!(wifi(&host, &quick), None);
        assert!(
            host.calls()
                .iter()
                .all(|call| !call.starts_with("ipconfig")),
            "{:?}",
            host.calls()
        );
        // a device that is left out is not asked about either
        let host = base().with_command("ipconfig getsummary en0", SUMMARY);
        let quick = super::quick(&host, &exclude(&["en0"]), &PortCache::new());
        assert_eq!(wifi(&host, &quick), None);
    }
}
