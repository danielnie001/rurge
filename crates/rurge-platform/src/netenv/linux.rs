//! Linux (and the other Unix systems that are not macOS): files under `/proc`
//! and `/sys` and the interface table; a program is run only for the Wi-Fi
//! link.

use super::host::{Ask, Host, say_once};
use super::{
    NetKind, Primary, Quick, ReadOpts, Wifi, network_addresses, parse, primary_first,
    unique_in_order,
};
use std::net::IpAddr;

/// Whether `interface` is a wireless one: the kernel makes a directory of that
/// name under it (`wireless` for the old interface, `phy80211` for cfg80211).
fn is_wireless(host: &dyn Host, interface: &str) -> bool {
    host.exists(&format!("/sys/class/net/{interface}/wireless"))
        || host.exists(&format!("/sys/class/net/{interface}/phy80211"))
}

/// Wi-Fi when the interface is wireless, wired when it is an Ethernet one (its
/// ARP hardware type is 1; every wireless interface has that type too, which
/// is why wireless is asked first), otherwise neither.
fn kind_of(host: &dyn Host, interface: &str) -> NetKind {
    if is_wireless(host, interface) {
        NetKind::Wifi
    } else if host
        .file(&format!("/sys/class/net/{interface}/type"))
        .is_some_and(|hardware_type| hardware_type.trim() == "1")
    {
        NetKind::Wired
    } else {
        NetKind::Other
    }
}

pub(crate) fn quick(host: &dyn Host, opts: &ReadOpts) -> Quick {
    let v4 = host
        .file("/proc/net/route")
        .and_then(|text| parse::proc_net_route(&text, &opts.exclude));
    let v6 = host
        .file("/proc/net/ipv6_route")
        .and_then(|text| parse::proc_ipv6_route(&text, &opts.exclude));
    // the interface of the IPv4 default route, else of the IPv6 one
    let interface = match (&v4, &v6) {
        (Some((interface, _)), _) | (None, Some((interface, _))) => Some(interface.clone()),
        (None, None) => None,
    };
    let primary = interface.as_ref().map(|interface| {
        let mut routers = Vec::new();
        if let Some((on, gateway)) = &v4
            && on == interface
        {
            routers.push(IpAddr::V4(*gateway));
        }
        // the IPv6 route is the one of the best metric, which may leave by
        // another interface than the IPv4 one
        if let Some((on, gateway)) = &v6
            && on == interface
        {
            routers.push(IpAddr::V6(*gateway));
        }
        Primary {
            kind: kind_of(host, interface),
            interface: interface.clone(),
            routers,
        }
    });
    // systemd-resolved keeps the servers it was told of here; `resolv.conf`
    // is then the address of its own stub
    let resolv = host
        .file("/run/systemd/resolve/resolv.conf")
        .or_else(|| host.file("/etc/resolv.conf"));
    let dns = resolv.map_or_else(Vec::new, |text| {
        unique_in_order(&crate::dns::parse_resolv_conf(&text).0)
    });
    let addresses = network_addresses(
        host.addresses()
            .into_iter()
            .filter(|(name, _)| Some(name) == interface.as_ref())
            .map(|(_, ip)| ip),
    );
    let wireless = host
        .dir("/sys/class/net")
        .into_iter()
        .filter(|name| !opts.exclude.contains(name) && is_wireless(host, name))
        .filter(|name| {
            host.file(&format!("/sys/class/net/{name}/operstate"))
                .is_some_and(|state| state.trim() == "up")
        })
        .collect();
    Quick {
        primary,
        dns,
        addresses,
        wireless,
    }
}

/// The question to NetworkManager: which access point is active.
///
/// It is the list NetworkManager already has (`--rescan no`). By default
/// `nmcli` scans when the list is older than 30 seconds — as it usually is on
/// a laptop that stays on its network, for NetworkManager scans little while
/// it is connected —, and a scan is a request to the radio (time, and battery)
/// for an answer that needs none: `ACTIVE` is the state of the connection.
///
/// `nmcli` writes `yes` in the language of the user, and a network name in
/// the charset of the user's locale, so it is run with `LC_ALL=C.UTF-8`:
/// English (the C locale has no translations) with UTF-8 text, so that a
/// network name with letters beyond ASCII comes out whole. Where a system has
/// no `C.UTF-8` the C locale is what it falls back to, which is English and
/// ASCII.
///
/// `LANGUAGE` is set to nothing: gettext puts the languages it lists before
/// the locale, and a glibc older than 2.39 does so for `C.UTF-8` too (its bug
/// 16621), which would have `nmcli` write `yes` in the language of a session
/// that sets it. An empty `LANGUAGE` lists none.
fn networkmanager_ask() -> Ask {
    Ask::new(&[
        "nmcli",
        "-t",
        "-f",
        "ACTIVE,SSID,BSSID,DEVICE",
        "dev",
        "wifi",
        "list",
        "--rescan",
        "no",
    ])
    .with_env("LC_ALL", "C.UTF-8")
    .with_env("LANGUAGE", "")
}

/// The Wi-Fi link, when a wireless interface is up: NetworkManager's word for
/// it, else `iw`'s for each of the interfaces, the primary one first. An
/// interface that is up is a link though neither says what it is called.
pub(crate) fn wifi(host: &dyn Host, quick: &Quick) -> Option<Wifi> {
    let primary = quick
        .primary
        .as_ref()
        .map(|primary| primary.interface.as_str());
    let ordered = primary_first(&quick.wireless, primary);
    let first = ordered.first()?;
    let from_networkmanager = host
        .run(&networkmanager_ask())
        .and_then(|text| parse::nmcli_wifi(&text))
        // the access point of a device that is not one of the interfaces
        // (it is left out, or it is not up) is no link of ours
        .filter(|link| link.name.as_ref().is_none_or(|name| ordered.contains(name)));
    let link = from_networkmanager.or_else(|| {
        ordered.iter().find_map(|interface| {
            host.run(&Ask::new(&["iw", "dev", interface, "link"]))
                .and_then(|text| parse::iw_link(&text, interface))
        })
    });
    if link.as_ref().is_none_or(|link| link.ssid.is_none()) {
        say_once("wifi-name", || {
            tracing::debug!(
                "a Wi-Fi interface is up but neither NetworkManager nor iw gives a name for the \
                 network; SSID and BSSID conditions will not match"
            );
        });
    }
    Some(match link {
        Some(link) => Wifi {
            interface: link.name.unwrap_or_else(|| first.clone()),
            ssid: link.ssid,
            bssid: link.bssid,
        },
        None => Wifi {
            interface: first.clone(),
            ssid: None,
            bssid: None,
        },
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

    // gateways 192.0.2.1 (eth0) and 198.51.100.1 (wlan0), the way the kernel
    // prints them
    const ROUTE: &str = concat!(
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n",
        "eth0\t00000000\t010200C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n",
        "wlan0\t00000000\t016433C6\t0003\t0\t0\t600\t00000000\t0\t0\t0\n",
        "eth0\t000200C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n",
    );

    // a default route via fe80::1 on eth0 and via fe80::fe on wlan0
    const ROUTE6: &str = concat!(
        "00000000000000000000000000000000 00 00000000000000000000000000000000 00 ",
        "fe800000000000000000000000000001 00000400 00000001 00000000 00000003     eth0\n",
        "00000000000000000000000000000000 00 00000000000000000000000000000000 00 ",
        "fe8000000000000000000000000000fe 00000100 00000001 00000000 00000003    wlan0\n",
    );

    // the question to NetworkManager as the fake machine names it: the
    // variables first, then the words
    const NMCLI: &str =
        "LC_ALL=C.UTF-8 LANGUAGE= nmcli -t -f ACTIVE,SSID,BSSID,DEVICE dev wifi list --rescan no";

    /// NetworkManager scans when its list of access points is older than 30
    /// seconds, unless it is told not to (`--rescan no`); a scan is a request
    /// to the radio — time, and battery —, the full read comes every 30
    /// seconds, and the answer is only the access point it already knows to
    /// be active.
    #[test]
    fn the_question_to_networkmanager_asks_for_no_scan() {
        let ask = networkmanager_ask();
        assert_eq!(
            ask.words,
            [
                "nmcli",
                "-t",
                "-f",
                "ACTIVE,SSID,BSSID,DEVICE",
                "dev",
                "wifi",
                "list",
                "--rescan",
                "no"
            ]
        );
    }

    /// `nmcli` writes `yes` — and whatever else it translates — in the language
    /// of the user, and a network name in the charset of the user's locale.
    /// `LC_ALL=C.UTF-8` is English (the C locale has no translations) with UTF-8
    /// text, so a network name with letters beyond ASCII comes out whole;
    /// `LC_ALL=C` is English too, but its charset is ASCII, and what `nmcli`
    /// prints beyond ASCII is at risk of becoming `?`. Where a system has no
    /// `C.UTF-8` the C locale is what it falls back to. `LANGUAGE`, a list of
    /// languages that gettext puts before the locale, is not ignored for
    /// `C.UTF-8` by a glibc older than 2.39 (its bug 16621): a user whose
    /// session says `LANGUAGE=zh_CN:zh` would be answered `是` for `yes`.
    /// An empty `LANGUAGE` is no list. The variables are the question's own —
    /// the program's environment —, not a program (`env`) in front of it.
    #[test]
    fn the_question_to_networkmanager_is_asked_in_english_and_utf_8() {
        let ask = networkmanager_ask();
        assert_eq!(
            ask.env,
            [
                ("LC_ALL".to_string(), "C.UTF-8".to_string()),
                ("LANGUAGE".to_string(), String::new()),
            ]
        );
        assert_eq!(ask.line(), NMCLI);
    }

    fn base() -> FakeHost {
        FakeHost::default()
            .with_file("/proc/net/route", ROUTE)
            .with_file("/sys/class/net/eth0/type", "1\n")
            .with_file(
                "/etc/resolv.conf",
                "nameserver 192.0.2.53\nsearch example.test\n",
            )
            .with_address("lo", "127.0.0.1")
            .with_address("eth0", "192.0.2.10")
            .with_address("eth0", "fe80::1")
            .with_address("wlan0", "198.51.100.10")
    }

    /// A wireless interface that is up, the way `/sys` shows it.
    fn wireless_up(host: FakeHost, name: &str) -> FakeHost {
        host.with_file(&format!("/sys/class/net/{name}/wireless"), "")
            .with_file(&format!("/sys/class/net/{name}/operstate"), "up\n")
    }

    #[test]
    fn the_interface_of_the_default_route_is_primary() {
        let host = base();
        let quick = quick(&host, &ReadOpts::default());
        assert_eq!(
            quick.primary,
            Some(Primary {
                interface: "eth0".to_string(),
                kind: NetKind::Wired,
                routers: vec![ip("192.0.2.1")],
            })
        );
        assert_eq!(quick.dns, [ip("192.0.2.53")]);
        // the addresses of the primary interface only, and no link-local one
        assert_eq!(quick.addresses, [ip("192.0.2.10")]);
        assert_eq!(quick.wireless, Vec::<String>::new());
        // the cheap look runs no program
        assert_eq!(host.calls(), Vec::<String>::new());
    }

    #[test]
    fn an_ipv6_default_route_decides_when_there_is_no_ipv4_one() {
        let host = FakeHost::default()
            .with_file("/proc/net/route", "Iface\tDestination\tGateway\tFlags\n")
            .with_file("/proc/net/ipv6_route", ROUTE6)
            .with_address("wlan0", "2001:db8:0:7:1:2:3:4");
        let host = wireless_up(host, "wlan0");
        let quick = quick(&host, &ReadOpts::default());
        assert_eq!(
            quick.primary,
            Some(Primary {
                interface: "wlan0".to_string(),
                kind: NetKind::Wifi,
                routers: vec![ip("fe80::fe")],
            })
        );
        assert_eq!(quick.addresses, [ip("2001:db8:0:7::")]);
        assert_eq!(quick.wireless, ["wlan0"]);
    }

    /// The IPv6 gateway is a router of the primary interface when it is the
    /// gateway of a route that leaves by that interface.
    #[test]
    fn the_ipv6_gateway_joins_the_routers_only_on_the_same_interface() {
        let on_eth0 = ROUTE6.replace(
            "00000100 00000001 00000000 00000003    wlan0",
            "00000800 00000001 00000000 00000003    wlan0",
        );
        // the best IPv6 route is eth0's (metric 0x400 against 0x800)
        let host = base().with_file("/proc/net/ipv6_route", &on_eth0);
        let routers = quick(&host, &ReadOpts::default()).primary.unwrap().routers;
        assert_eq!(routers, [ip("192.0.2.1"), ip("fe80::1")]);
        // the best IPv6 route leaves by wlan0: not a router of eth0
        let host = base().with_file("/proc/net/ipv6_route", ROUTE6);
        let routers = quick(&host, &ReadOpts::default()).primary.unwrap().routers;
        assert_eq!(routers, [ip("192.0.2.1")]);
    }

    #[test]
    fn what_systemd_resolved_keeps_is_read_before_resolv_conf() {
        let host = base().with_file(
            "/run/systemd/resolve/resolv.conf",
            "nameserver 198.51.100.53\nnameserver 198.51.100.53\nnameserver 2001:db8::53\n",
        );
        // each once, in order
        assert_eq!(
            quick(&host, &ReadOpts::default()).dns,
            [ip("198.51.100.53"), ip("2001:db8::53")]
        );
        let none = FakeHost::default().with_file("/proc/net/route", ROUTE);
        assert_eq!(quick(&none, &ReadOpts::default()).dns, Vec::<IpAddr>::new());
    }

    #[test]
    fn the_kind_of_an_interface_comes_from_sys() {
        let host = FakeHost::default()
            .with_file("/sys/class/net/wlan0/wireless", "")
            .with_file("/sys/class/net/wlan1/phy80211/index", "1\n")
            .with_file("/sys/class/net/wlan1/type", "1\n")
            .with_file("/sys/class/net/eth0/type", "1\n")
            .with_file("/sys/class/net/tun0/type", "65534\n");
        for (name, kind) in [
            ("wlan0", NetKind::Wifi),
            // wireless though its type is that of Ethernet, as every wireless interface's is
            ("wlan1", NetKind::Wifi),
            ("eth0", NetKind::Wired),
            ("tun0", NetKind::Other),
            ("ppp0", NetKind::Other),
        ] {
            assert_eq!(kind_of(&host, name), kind, "{name}");
        }
    }

    #[test]
    fn the_wireless_interfaces_that_are_up_are_listed() {
        let host = wireless_up(base(), "wlan2");
        let host = wireless_up(host, "wlan0")
            .with_file("/sys/class/net/wlan1/phy80211/index", "1\n")
            .with_file("/sys/class/net/wlan1/operstate", "down\n")
            .with_file("/sys/class/net/wlan3/wireless", "");
        // wlan1 is down; wlan3 says nothing of its state
        assert_eq!(
            quick(&host, &ReadOpts::default()).wireless,
            ["wlan0", "wlan2"]
        );
        assert_eq!(quick(&host, &exclude(&["wlan0"])).wireless, ["wlan2"]);
    }

    /// What is left out is not the primary interface: the next default route
    /// is.
    #[test]
    fn an_excluded_interface_is_not_primary() {
        let host = base();
        let quick = quick(&host, &exclude(&["eth0"]));
        let primary = quick.primary.unwrap();
        assert_eq!(primary.interface, "wlan0");
        assert_eq!(primary.routers, [ip("198.51.100.1")]);
        assert_eq!(quick.addresses, [ip("198.51.100.10")]);
        assert_eq!(
            super::quick(&host, &exclude(&["eth0", "wlan0"])).primary,
            None
        );
    }

    #[test]
    fn the_wifi_link_is_asked_of_networkmanager_first() {
        let host = wireless_up(base(), "wlan0").with_command(
            NMCLI,
            "no:OtherNet:00\\:00\\:5E\\:00\\:53\\:10:wlan0\n\
             yes:Example Net:00\\:00\\:5E\\:00\\:53\\:02:wlan0\n",
        );
        let quick = quick(&host, &ReadOpts::default());
        assert_eq!(
            wifi(&host, &quick),
            Some(Wifi {
                interface: "wlan0".to_string(),
                ssid: Some("Example Net".to_string()),
                bssid: Some("00:00:5E:00:53:02".to_string()),
            })
        );
        assert_eq!(host.calls(), [NMCLI]);
    }

    #[test]
    fn without_networkmanager_the_wifi_link_is_asked_of_iw_primary_interface_first() {
        // wlan1 is the primary interface
        let route = ROUTE
            .replace("wlan0\t00000000", "wlan1\t00000000")
            .replace("600", "50");
        let host = wireless_up(base().with_file("/proc/net/route", &route), "wlan0");
        let host = wireless_up(host, "wlan1")
            .with_command(
                "iw dev wlan1 link",
                "Connected to 00:00:5e:00:53:02 (on wlan1)\n\tSSID: Example Net\n\tfreq: 5180\n",
            )
            .with_command("iw dev wlan0 link", "Not connected.\n");
        let quick = quick(&host, &ReadOpts::default());
        assert_eq!(quick.primary.as_ref().unwrap().interface, "wlan1");
        assert_eq!(
            wifi(&host, &quick),
            Some(Wifi {
                interface: "wlan1".to_string(),
                ssid: Some("Example Net".to_string()),
                bssid: Some("00:00:5e:00:53:02".to_string()),
            })
        );
        // networkmanager did not answer; the primary interface was the first asked, and the only one
        assert_eq!(host.calls(), [NMCLI, "iw dev wlan1 link"]);
    }

    /// `iw` for the next interface when the first is not connected.
    #[test]
    fn iw_is_asked_of_each_wireless_interface_until_one_is_connected() {
        let host = wireless_up(wireless_up(base(), "wlan0"), "wlan1")
            .with_command("iw dev wlan0 link", "Not connected.\n")
            .with_command(
                "iw dev wlan1 link",
                "Connected to 00:00:5e:00:53:02 (on wlan1)\n\tSSID: Example Net\n",
            );
        let quick = quick(&host, &ReadOpts::default());
        let link = wifi(&host, &quick).unwrap();
        assert_eq!(link.interface, "wlan1");
        assert_eq!(
            host.calls(),
            [NMCLI, "iw dev wlan0 link", "iw dev wlan1 link"]
        );
    }

    /// The interface is up, so it is a link; neither program says what the
    /// network is called.
    #[test]
    fn a_link_whose_name_cannot_be_read_is_a_link_without_a_name() {
        let host = wireless_up(base(), "wlan0");
        let quick = quick(&host, &ReadOpts::default());
        assert_eq!(
            wifi(&host, &quick),
            Some(Wifi {
                interface: "wlan0".to_string(),
                ssid: None,
                bssid: None,
            })
        );
    }

    #[test]
    fn without_a_wireless_interface_that_is_up_no_program_runs() {
        let host = base()
            .with_file("/sys/class/net/wlan0/wireless", "")
            .with_file("/sys/class/net/wlan0/operstate", "down\n")
            .with_command(NMCLI, "yes:Example Net:00\\:00\\:5E\\:00\\:53\\:02:wlan0\n");
        let quick = quick(&host, &ReadOpts::default());
        assert_eq!(wifi(&host, &quick), None);
        assert_eq!(host.calls(), Vec::<String>::new());
    }

    /// A device that is left out is not the link, even when networkmanager
    /// says it is the active one.
    #[test]
    fn an_excluded_device_is_no_link() {
        let host = wireless_up(wireless_up(base(), "wlan0"), "wlan1")
            .with_command(NMCLI, "yes:Example Net:00\\:00\\:5E\\:00\\:53\\:02:wlan0\n");
        let opts = exclude(&["wlan0"]);
        let quick = quick(&host, &opts);
        assert_eq!(quick.wireless, ["wlan1"]);
        let link = wifi(&host, &quick).unwrap();
        assert_eq!(link.interface, "wlan1");
        assert_eq!(link.ssid, None);
    }
}
