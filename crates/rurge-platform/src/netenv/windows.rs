//! Windows: the adapters the system lists (the `ipconfig` crate asks
//! `GetAdaptersAddresses`, which takes no program and is cheap), and `netsh`
//! for the Wi-Fi link. Everything but the call into the system is compiled on
//! every host.

use super::host::{Host, say_once};
use super::{NetKind, Primary, Quick, ReadOpts, Wifi, network_addresses, parse, unique_in_order};
use std::net::IpAddr;

/// What an adapter is, from its interface type.
#[cfg_attr(not(any(windows, test)), allow(dead_code))] // told apart on Windows only
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdapterKind {
    /// IEEE 802.11.
    Wifi,
    /// Ethernet.
    Wired,
    Loopback,
    Tunnel,
    Other,
}

/// What the reader takes from an adapter of the system's list.
#[cfg_attr(not(any(windows, test)), allow(dead_code))] // listed on Windows only
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdapterView {
    /// The name Windows shows: `Wi-Fi`, `Ethernet`.
    pub name: String,
    pub kind: AdapterKind,
    /// Its operational status is up.
    pub up: bool,
    /// The default gateways, IPv4 and IPv6 together.
    pub gateways: Vec<IpAddr>,
    pub dns: Vec<IpAddr>,
    /// The unicast addresses.
    pub addresses: Vec<IpAddr>,
    pub metric_v4: u32,
    pub metric_v6: u32,
}

/// The default gateways of an adapter that are addresses.
fn gateways(adapter: &AdapterView) -> impl Iterator<Item = IpAddr> + '_ {
    adapter
        .gateways
        .iter()
        .copied()
        .filter(|gateway| !gateway.is_unspecified())
}

/// The adapter the traffic leaves by: among those that are up, not a loopback
/// or a tunnel and not in `exclude`, the one with an IPv4 default gateway and
/// the lowest IPv4 metric — the first of those when several have it —, or,
/// when none has a gateway, the same by the IPv6 gateway and metric.
pub(crate) fn pick_primary<'a>(
    adapters: &'a [AdapterView],
    exclude: &[String],
) -> Option<&'a AdapterView> {
    let candidates = || {
        adapters.iter().filter(|adapter| {
            adapter.up
                && !matches!(adapter.kind, AdapterKind::Loopback | AdapterKind::Tunnel)
                && !exclude.contains(&adapter.name)
        })
    };
    candidates()
        .filter(|adapter| gateways(adapter).any(|gateway| gateway.is_ipv4()))
        .min_by_key(|adapter| adapter.metric_v4)
        .or_else(|| {
            candidates()
                .filter(|adapter| gateways(adapter).any(|gateway| gateway.is_ipv6()))
                .min_by_key(|adapter| adapter.metric_v6)
        })
}

/// The gateways of the primary adapter: the IPv4 ones first, then the IPv6
/// ones.
fn routers(adapter: &AdapterView) -> Vec<IpAddr> {
    let mut routers: Vec<IpAddr> = Vec::new();
    for gateway in gateways(adapter)
        .filter(IpAddr::is_ipv4)
        .chain(gateways(adapter).filter(IpAddr::is_ipv6))
    {
        if !routers.contains(&gateway) {
            routers.push(gateway);
        }
    }
    routers
}

pub(crate) fn quick(host: &dyn Host, opts: &ReadOpts) -> Quick {
    let adapters = host.adapters();
    let primary = pick_primary(&adapters, &opts.exclude);
    let mut wireless: Vec<String> = adapters
        .iter()
        .filter(|adapter| {
            adapter.kind == AdapterKind::Wifi && adapter.up && !opts.exclude.contains(&adapter.name)
        })
        .map(|adapter| adapter.name.clone())
        .collect();
    wireless.sort();
    wireless.dedup();
    Quick {
        primary: primary.map(|adapter| Primary {
            interface: adapter.name.clone(),
            kind: match adapter.kind {
                AdapterKind::Wifi => NetKind::Wifi,
                AdapterKind::Wired => NetKind::Wired,
                AdapterKind::Loopback | AdapterKind::Tunnel | AdapterKind::Other => NetKind::Other,
            },
            routers: routers(adapter),
        }),
        dns: primary.map_or_else(Vec::new, |adapter| unique_in_order(&adapter.dns)),
        addresses: network_addresses(
            primary
                .into_iter()
                .flat_map(|a| a.addresses.iter().copied()),
        ),
        wireless,
    }
}

/// Which of the wireless adapters that are up the Wi-Fi link is on: the one
/// `netsh` names (the word is in English only), else the primary, else the
/// first.
pub(crate) fn wifi_interface(
    wireless: &[String],
    primary: Option<&str>,
    named: Option<&str>,
) -> Option<String> {
    named
        .and_then(|named| {
            wireless
                .iter()
                .find(|name| name.eq_ignore_ascii_case(named))
        })
        .or_else(|| primary.and_then(|primary| wireless.iter().find(|name| *name == primary)))
        .or_else(|| wireless.first())
        .cloned()
}

/// The question to `netsh`, put through `cmd`. `netsh` writes in the code page
/// of its console and in the language of the system — Chinese in GBK on a
/// Chinese Windows —, which are neither UTF-8 nor the keys read here; a console
/// of the code page 65001 gets English and UTF-8 from it. The console is one
/// of `cmd`'s own (`run_with_timeout` starts a program without ours), so the
/// terminal of the user keeps its code page.
pub(crate) const WLAN_COMMAND: [&str; 4] = [
    "cmd",
    "/d",
    "/c",
    "chcp 65001 >nul & netsh wlan show interfaces",
];

/// The Wi-Fi link: asked about when a wireless adapter is up.
pub(crate) fn wifi(host: &dyn Host, quick: &Quick) -> Option<Wifi> {
    if quick.wireless.is_empty() {
        return None;
    }
    let link = host
        .run(&WLAN_COMMAND)
        .and_then(|text| parse::netsh_wlan(&text));
    let primary = quick
        .primary
        .as_ref()
        .map(|primary| primary.interface.as_str());
    let named = link.as_ref().and_then(|link| link.name.as_deref());
    let interface = wifi_interface(&quick.wireless, primary, named)?;
    if link.as_ref().is_none_or(|link| link.ssid.is_none()) {
        say_once("wifi-name", || {
            tracing::debug!(
                "a Wi-Fi adapter is up but Windows gives no name for the network (it wants the \
                 permission to see where the machine is); SSID and BSSID conditions will not match"
            );
        });
    }
    let (ssid, bssid) = link.map_or((None, None), |link| (link.ssid, link.bssid));
    Some(Wifi {
        interface,
        ssid,
        bssid,
    })
}

/// The adapters Windows lists.
#[cfg(windows)]
pub(super) fn system_adapters() -> Vec<AdapterView> {
    // `ipconfig` panics on an operational status that is not one of the seven
    // it knows, so it runs on a thread of its own and a panic there is a list
    // that could not be read.
    let read = std::thread::Builder::new()
        .name("rurge-adapters".to_string())
        .spawn(|| {
            ipconfig::get_adapters()
                .map(|adapters| adapters.iter().map(view).collect::<Vec<AdapterView>>())
        });
    let outcome = match read {
        Ok(thread) => thread.join(),
        Err(e) => {
            say_once("adapters-thread", || {
                tracing::debug!(reason = ?e.kind(), "cannot start the thread that lists the network adapters");
            });
            return Vec::new();
        }
    };
    match outcome {
        Ok(Ok(adapters)) => adapters,
        Ok(Err(e)) => {
            say_once("adapters", || {
                tracing::debug!(error = %e, "cannot list the network adapters");
            });
            Vec::new()
        }
        Err(_) => {
            say_once("adapters-panic", || {
                tracing::debug!(
                    "the library that lists the network adapters panicked; they are taken as unknown"
                );
            });
            Vec::new()
        }
    }
}

#[cfg(not(windows))]
pub(super) fn system_adapters() -> Vec<AdapterView> {
    Vec::new()
}

#[cfg(windows)]
fn view(adapter: &ipconfig::Adapter) -> AdapterView {
    use ipconfig::{IfType, OperStatus};

    AdapterView {
        name: adapter.friendly_name().to_string(),
        kind: match adapter.if_type() {
            IfType::Ieee80211 => AdapterKind::Wifi,
            IfType::EthernetCsmacd => AdapterKind::Wired,
            IfType::SoftwareLoopback => AdapterKind::Loopback,
            IfType::Tunnel => AdapterKind::Tunnel,
            _ => AdapterKind::Other,
        },
        up: adapter.oper_status() == OperStatus::IfOperStatusUp,
        gateways: adapter.gateways().to_vec(),
        dns: adapter.dns_servers().to_vec(),
        addresses: adapter.ip_addresses().to_vec(),
        metric_v4: adapter.ipv4_metric(),
        metric_v6: adapter.ipv6_metric(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::host::testing::FakeHost;
    use super::*;

    fn adapter(
        name: &str,
        kind: AdapterKind,
        up: bool,
        gateways: &[&str],
        metric: u32,
    ) -> AdapterView {
        AdapterView {
            name: name.to_string(),
            kind,
            up,
            gateways: gateways.iter().map(|g| g.parse().unwrap()).collect(),
            dns: Vec::new(),
            addresses: Vec::new(),
            metric_v4: metric,
            metric_v6: metric,
        }
    }

    fn exclude(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn name_of(primary: Option<&AdapterView>) -> Option<&str> {
        primary.map(|adapter| adapter.name.as_str())
    }

    #[test]
    fn the_adapter_with_the_lowest_ipv4_metric_that_has_a_gateway_is_primary() {
        let adapters = [
            adapter("Wi-Fi", AdapterKind::Wifi, true, &["198.51.100.1"], 50),
            adapter("Ethernet", AdapterKind::Wired, true, &["192.0.2.1"], 25),
            adapter("Ethernet 2", AdapterKind::Wired, true, &["203.0.113.1"], 25),
        ];
        // the first of two with the same metric
        assert_eq!(name_of(pick_primary(&adapters, &[])), Some("Ethernet"));
        assert_eq!(
            name_of(pick_primary(
                &adapters,
                &exclude(&["Ethernet", "Ethernet 2"])
            )),
            Some("Wi-Fi")
        );
    }

    /// A loopback, a tunnel, an adapter that is not up, one that is left out
    /// and one that has no gateway are all in the list ahead of the one that
    /// is used, with the lower metrics.
    #[test]
    fn what_cannot_carry_the_traffic_is_not_primary() {
        let adapters = [
            adapter("Loopback", AdapterKind::Loopback, true, &["192.0.2.254"], 1),
            adapter("Teredo", AdapterKind::Tunnel, true, &["192.0.2.253"], 2),
            adapter("Cable", AdapterKind::Wired, false, &["192.0.2.252"], 3),
            adapter("Virtual", AdapterKind::Wired, true, &["192.0.2.251"], 4),
            adapter("Internal", AdapterKind::Wired, true, &[], 5),
            adapter("Wi-Fi", AdapterKind::Wifi, true, &["198.51.100.1"], 50),
        ];
        assert_eq!(
            name_of(pick_primary(&adapters, &exclude(&["Virtual"]))),
            Some("Wi-Fi")
        );
        assert_eq!(pick_primary(&adapters[..5], &exclude(&["Virtual"])), None);
        assert_eq!(pick_primary(&[], &[]), None);
    }

    /// A gateway of `::` or `0.0.0.0` is no gateway.
    #[test]
    fn an_unspecified_gateway_is_no_gateway() {
        let adapters = [adapter(
            "Ethernet",
            AdapterKind::Wired,
            true,
            &["0.0.0.0", "::"],
            25,
        )];
        assert_eq!(pick_primary(&adapters, &[]), None);
    }

    /// The IPv4 default route comes first; the IPv6 one decides only when no
    /// adapter has an IPv4 gateway, and by the IPv6 metric.
    #[test]
    fn without_an_ipv4_gateway_the_ipv6_one_decides() {
        let mut a = adapter("Ethernet", AdapterKind::Wired, true, &["fe80::1"], 10);
        a.metric_v6 = 90;
        let mut b = adapter("Wi-Fi", AdapterKind::Wifi, true, &["fe80::2"], 20);
        b.metric_v6 = 40;
        let both = [a.clone(), b.clone()];
        assert_eq!(name_of(pick_primary(&both, &[])), Some("Wi-Fi"));
        // an IPv4 gateway on the adapter with the worse metric still wins
        let mut c = adapter("Cellular", AdapterKind::Other, true, &["192.0.2.9"], 900);
        c.metric_v6 = 900;
        let three = [a, b, c];
        assert_eq!(name_of(pick_primary(&three, &[])), Some("Cellular"));
    }

    fn with(mut adapter: AdapterView, dns: &[&str], addresses: &[&str]) -> AdapterView {
        adapter.dns = dns.iter().map(|d| d.parse().unwrap()).collect();
        adapter.addresses = addresses.iter().map(|a| a.parse().unwrap()).collect();
        adapter
    }

    #[test]
    fn the_primary_adapter_gives_its_routers_dns_servers_and_addresses() {
        let ethernet = with(
            adapter(
                "Ethernet",
                AdapterKind::Wired,
                true,
                &["fe80::1", "192.0.2.1"],
                25,
            ),
            &["192.0.2.53", "2001:db8::53", "192.0.2.53"],
            &[
                "192.0.2.10",
                "fe80::aaaa:1",
                "2001:db8:0:7:1111:2222:3333:4444",
                "2001:db8:0:7:5555:6666:7777:8888",
            ],
        );
        let wifi = adapter("Wi-Fi", AdapterKind::Wifi, true, &["198.51.100.1"], 50);
        let host = FakeHost::default()
            .with_adapter(ethernet)
            .with_adapter(wifi);
        let quick = quick(&host, &ReadOpts::default());
        assert_eq!(
            quick.primary,
            Some(Primary {
                interface: "Ethernet".to_string(),
                kind: NetKind::Wired,
                // the IPv4 gateway first
                routers: vec!["192.0.2.1".parse().unwrap(), "fe80::1".parse().unwrap()],
            })
        );
        // in the order the adapter lists them, each once
        assert_eq!(
            quick.dns,
            ["192.0.2.53", "2001:db8::53"].map(|d| d.parse::<IpAddr>().unwrap())
        );
        // what says which network it is: the IPv4 address, and the prefix of the IPv6 ones
        assert_eq!(
            quick.addresses,
            ["192.0.2.10", "2001:db8:0:7::"].map(|a| a.parse::<IpAddr>().unwrap())
        );
        // a wireless adapter that is up, though it is not the primary one
        assert_eq!(quick.wireless, ["Wi-Fi"]);
    }

    #[test]
    fn the_kind_of_the_primary_follows_the_adapter() {
        for (kind, expected) in [
            (AdapterKind::Wifi, NetKind::Wifi),
            (AdapterKind::Wired, NetKind::Wired),
            (AdapterKind::Other, NetKind::Other),
        ] {
            let host =
                FakeHost::default().with_adapter(adapter("A", kind, true, &["192.0.2.1"], 1));
            let primary = quick(&host, &ReadOpts::default()).primary.unwrap();
            assert_eq!(primary.kind, expected, "{kind:?}");
        }
    }

    #[test]
    fn no_usable_adapter_is_an_unknown_network() {
        let host = FakeHost::default();
        assert_eq!(quick(&host, &ReadOpts::default()), Quick::default());
        // adapters that are down, or without a gateway, are not a network, and a
        // wireless one that is not up is not a link to ask about
        let host = FakeHost::default()
            .with_adapter(adapter(
                "Wi-Fi",
                AdapterKind::Wifi,
                false,
                &["192.0.2.1"],
                1,
            ))
            .with_adapter(adapter("Ethernet", AdapterKind::Wired, true, &[], 1));
        assert_eq!(quick(&host, &ReadOpts::default()), Quick::default());
    }

    /// What is left out is neither the primary adapter nor a link to ask about.
    #[test]
    fn an_excluded_adapter_is_neither_primary_nor_wireless() {
        let host = FakeHost::default()
            .with_adapter(adapter("Wi-Fi", AdapterKind::Wifi, true, &["192.0.2.1"], 1))
            .with_adapter(adapter("Wi-Fi 2", AdapterKind::Wifi, true, &[], 1));
        let opts = ReadOpts {
            exclude: exclude(&["Wi-Fi"]),
        };
        let quick = quick(&host, &opts);
        assert_eq!(quick.primary, None);
        assert_eq!(quick.wireless, ["Wi-Fi 2"]);
    }

    const NETSH: &str = "\
    Name                   : Wi-Fi\n\
    State                  : connected\n\
    SSID                   : ExampleNet\n\
    BSSID                  : 00:00:5e:00:53:02\n";

    /// The command line the fake machine is asked, as a word list joined.
    fn wlan() -> String {
        WLAN_COMMAND.join(" ")
    }

    /// What `WLAN_COMMAND` says is pinned: without the code page it is a
    /// question that gets the answer of the system's own language and code page.
    #[test]
    fn netsh_is_asked_in_a_console_of_utf_8() {
        assert_eq!(
            wlan(),
            "cmd /d /c chcp 65001 >nul & netsh wlan show interfaces"
        );
    }

    fn wireless_host() -> FakeHost {
        FakeHost::default()
            .with_adapter(adapter("Wi-Fi", AdapterKind::Wifi, true, &["192.0.2.1"], 1))
            .with_command(&wlan(), NETSH)
    }

    #[test]
    fn a_wireless_adapter_that_is_up_is_asked_about_its_network() {
        let host = wireless_host();
        let quick = quick(&host, &ReadOpts::default());
        assert_eq!(
            wifi(&host, &quick),
            Some(Wifi {
                interface: "Wi-Fi".to_string(),
                ssid: Some("ExampleNet".to_string()),
                bssid: Some("00:00:5e:00:53:02".to_string()),
            })
        );
        assert_eq!(host.calls(), [wlan()]);
    }

    /// No wireless adapter is up: there is no link, and `netsh` is not run.
    #[test]
    fn without_a_wireless_adapter_that_is_up_netsh_is_not_run() {
        let host = FakeHost::default()
            .with_adapter(adapter(
                "Ethernet",
                AdapterKind::Wired,
                true,
                &["192.0.2.1"],
                1,
            ))
            .with_adapter(adapter("Wi-Fi", AdapterKind::Wifi, false, &[], 1))
            .with_command(&wlan(), NETSH);
        let quick = quick(&host, &ReadOpts::default());
        assert_eq!(wifi(&host, &quick), None);
        assert_eq!(host.calls(), Vec::<String>::new());
    }

    /// Windows that will not say the name of the network (no permission to see
    /// where the machine is) or a `netsh` that does not run: the adapter is
    /// up, so there is a link, and nothing is known of its name.
    #[test]
    fn a_link_whose_name_cannot_be_read_is_a_link_without_a_name() {
        let silent =
            "Network shell commands need location permission to access WLAN information.\n";
        for host in [
            FakeHost::default()
                .with_adapter(adapter("Wi-Fi", AdapterKind::Wifi, true, &["192.0.2.1"], 1))
                .with_command(&wlan(), silent),
            FakeHost::default().with_adapter(adapter(
                "Wi-Fi",
                AdapterKind::Wifi,
                true,
                &["192.0.2.1"],
                1,
            )),
        ] {
            let quick = quick(&host, &ReadOpts::default());
            assert_eq!(
                wifi(&host, &quick),
                Some(Wifi {
                    interface: "Wi-Fi".to_string(),
                    ssid: None,
                    bssid: None,
                })
            );
        }
    }

    #[test]
    fn the_link_is_on_the_adapter_netsh_names() {
        let wireless = ["Wi-Fi", "Wi-Fi 2", "Wi-Fi 3"].map(String::from);
        // the word is in English only: matched without regard to case
        assert_eq!(
            wifi_interface(&wireless, Some("Wi-Fi"), Some("wi-fi 2")).as_deref(),
            Some("Wi-Fi 2")
        );
        // not named, or named as an adapter that is not up: the primary one
        assert_eq!(
            wifi_interface(&wireless, Some("Wi-Fi 3"), None).as_deref(),
            Some("Wi-Fi 3")
        );
        assert_eq!(
            wifi_interface(&wireless, Some("Wi-Fi 3"), Some("Local Area Connection* 1")).as_deref(),
            Some("Wi-Fi 3")
        );
        // the primary one is not wireless: the first
        assert_eq!(
            wifi_interface(&wireless, Some("Ethernet"), None).as_deref(),
            Some("Wi-Fi")
        );
        assert_eq!(wifi_interface(&[], Some("Wi-Fi"), Some("Wi-Fi")), None);
    }
}
