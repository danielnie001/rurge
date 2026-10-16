//! What the tools and files that describe the network say, as plain functions
//! of their text, so that every platform's answer is tested on any host.
//!
//! Nothing here fails: text that is not understood is an answer of "nothing"
//! (`None`, an empty list).

use super::NetKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// A Wi-Fi link as a tool describes it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Link {
    /// The interface, when the tool names it.
    pub name: Option<String>,
    /// `None`: the tool would not say (empty, or `<redacted>`).
    pub ssid: Option<String>,
    pub bssid: Option<String>,
}

/// What is left of a value when the white space around it is gone; `None`
/// when nothing is.
fn value(raw: &str) -> Option<String> {
    let raw = raw.trim();
    (!raw.is_empty()).then(|| raw.to_string())
}

/// A BSSID with its octets of two digits each: macOS writes them without the
/// leading zeros (`0:0:5e:0:53:2`), and one BSSID is to read one way on every
/// platform. Anything that is not six octets of hexadecimal digits is as the
/// tool wrote it.
fn bssid(raw: &str) -> Option<String> {
    let raw = value(raw)?;
    let octets: Vec<&str> = raw.split(':').collect();
    let is_mac = octets.len() == 6
        && octets.iter().all(|octet| {
            (1..=2).contains(&octet.len()) && octet.bytes().all(|b| b.is_ascii_hexdigit())
        });
    if !is_mac {
        return Some(raw);
    }
    let padded: Vec<String> = octets.iter().map(|octet| format!("{octet:0>2}")).collect();
    Some(padded.join(":"))
}

/// The lines of one adapter in the output of `netsh wlan show interfaces`.
#[derive(Default)]
struct WlanBlock {
    name: Option<String>,
    /// `Some` once there is an `SSID` line; what it says may be nothing.
    ssid: Option<Option<String>>,
    bssid: Option<String>,
}

impl WlanBlock {
    /// A block without an `SSID` line is an adapter that is not connected.
    fn link(self) -> Option<Link> {
        Some(Link {
            name: self.name,
            ssid: self.ssid?,
            bssid: self.bssid,
        })
    }
}

/// `netsh wlan show interfaces` (Windows): the first adapter that is
/// connected, which is the first with an `SSID` line. The key is `SSID` and
/// the one that ends in `BSSID` (Windows 10 writes `BSSID`, Windows 11 `AP
/// BSSID`); the name of the adapter is the `Name` line, in English. Other
/// languages translate that word, and then the link has no name.
pub(crate) fn netsh_wlan(text: &str) -> Option<Link> {
    // one block of lines per adapter, a blank line between two of them
    let mut block = WlanBlock::default();
    for line in text.lines() {
        if line.trim().is_empty() {
            if let Some(link) = std::mem::take(&mut block).link() {
                return Some(link);
            }
            continue;
        }
        // a key never holds a colon, a value may
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "Name" if block.name.is_none() => block.name = value(rest),
            "SSID" if block.ssid.is_none() => block.ssid = Some(value(rest)),
            key if key.ends_with("BSSID") && block.bssid.is_none() => block.bssid = bssid(rest),
            _ => {}
        }
    }
    block.link()
}

/// The `RTF_GATEWAY` bit of the flags of a route.
const RTF_GATEWAY: u32 = 0x2;

/// `/proc/net/route` (Linux): the default route that has a gateway and the
/// lowest metric — the first of them when several have it — as the interface
/// and the gateway. Routes on an interface in `exclude` are not looked at.
pub(crate) fn proc_net_route(text: &str, exclude: &[String]) -> Option<(String, Ipv4Addr)> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [iface, destination, gateway, flags, _, _, metric, mask, ..] = fields.as_slice()
            else {
                return None;
            };
            // the header line is not hexadecimal and ends here
            let hex = |field: &str| u32::from_str_radix(field, 16).ok();
            let (destination, gateway, flags, mask) =
                (hex(destination)?, hex(gateway)?, hex(flags)?, hex(mask)?);
            let metric: u32 = metric.parse().ok()?;
            let usable = destination == 0
                && mask == 0
                && flags & RTF_GATEWAY != 0
                && gateway != 0
                && !exclude.iter().any(|name| name == iface);
            // The kernel prints an address as the number its bytes make on
            // this machine, and every machine rurge runs on is little-endian:
            // the first byte of the address is the last two digits.
            usable.then(|| {
                (
                    metric,
                    iface.to_string(),
                    Ipv4Addr::from(gateway.to_le_bytes()),
                )
            })
        })
        .min_by_key(|(metric, ..)| *metric)
        .map(|(_, iface, gateway)| (iface, gateway))
}

/// `/proc/net/ipv6_route` (Linux): the same for IPv6 — the route whose
/// destination has a prefix length of 0 and whose next hop is not `::`.
pub(crate) fn proc_ipv6_route(text: &str, exclude: &[String]) -> Option<(String, Ipv6Addr)> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [_, prefix, _, _, next_hop, metric, _, _, _, iface, ..] = fields.as_slice() else {
                return None;
            };
            let next_hop = u128::from_str_radix(next_hop, 16).ok()?;
            let metric = u32::from_str_radix(metric, 16).ok()?;
            let usable = u8::from_str_radix(prefix, 16).ok()? == 0
                && next_hop != 0
                && !exclude.iter().any(|name| name == iface);
            usable.then(|| (metric, iface.to_string(), Ipv6Addr::from(next_hop)))
        })
        .min_by_key(|(metric, ..)| *metric)
        .map(|(_, iface, next_hop)| (iface, next_hop))
}

/// The fields of a line of `nmcli -t`: separated by `:`, and a `:` or a `\`
/// inside a field has a `\` in front of it.
fn terse_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => field.extend(chars.next()),
            ':' => fields.push(std::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    fields.push(field);
    fields
}

/// `nmcli -t -f ACTIVE,SSID,BSSID,DEVICE dev wifi` (Linux): the access point
/// that is active. A `:` or a `\` in a value is written with a `\` in front.
pub(crate) fn nmcli_wifi(text: &str) -> Option<Link> {
    text.lines().find_map(|line| {
        let fields = terse_fields(line);
        let [active, ssid, bssid_field, device, ..] = fields.as_slice() else {
            return None;
        };
        (active == "yes").then(|| Link {
            name: value(device),
            ssid: value(ssid),
            bssid: bssid(bssid_field),
        })
    })
}

/// `iw dev <interface> link` (Linux): `None` when the interface is not
/// connected.
pub(crate) fn iw_link(text: &str, interface: &str) -> Option<Link> {
    let first = text.lines().find(|line| !line.trim().is_empty())?;
    let connected_to = first.strip_prefix("Connected to ")?;
    let ssid = text
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("SSID:"))
        .and_then(value);
    Some(Link {
        name: Some(interface.to_string()),
        ssid,
        bssid: connected_to.split_whitespace().next().and_then(bssid),
    })
}

/// `route -n get default` and `route -n get -inet6 default` (macOS): the
/// interface and the gateway of the default route. A route without a gateway
/// that is an address (one that is `link#4`, say) gives nothing.
pub(crate) fn route_get_default(text: &str) -> Option<(String, IpAddr)> {
    let mut interface = None;
    let mut gateway = None;
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "interface" => interface = value(rest),
            // an IPv6 gateway is written with the interface it is reached by: fe80::1%en0
            "gateway" => {
                gateway = rest
                    .trim()
                    .split('%')
                    .next()
                    .and_then(|address| address.parse::<IpAddr>().ok());
            }
            _ => {}
        }
    }
    Some((interface?, gateway?))
}

/// The kind of hardware port a name stands for: Wi-Fi (`Wi-Fi`, `Wi-Fi 2`,
/// older macOS's `AirPort`), wired (a word of the name is `Ethernet` or
/// `LAN`; `WLAN` is not `LAN`), or neither.
fn port_kind(name: &str) -> NetKind {
    let lower = name.trim().to_ascii_lowercase();
    let squashed: String = lower.chars().filter(char::is_ascii_alphanumeric).collect();
    if squashed.starts_with("wifi") || squashed.starts_with("airport") {
        return NetKind::Wifi;
    }
    let mut words = lower.split(|c: char| !c.is_ascii_alphanumeric());
    if words.any(|word| word == "ethernet" || word == "lan") {
        NetKind::Wired
    } else {
        NetKind::Other
    }
}

/// `networksetup -listallhardwareports` (macOS): each device with the kind
/// of hardware port it is.
pub(crate) fn hardware_ports(text: &str) -> Vec<(String, NetKind)> {
    let mut ports = Vec::new();
    let mut kind = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("Hardware Port:") {
            kind = Some(port_kind(name));
        } else if let Some(device) = line.strip_prefix("Device:")
            && let (Some(kind), Some(device)) = (kind.take(), value(device))
        {
            ports.push((device, kind));
        }
    }
    ports
}

/// `ipconfig getsummary <interface>` (macOS): `None` when there is no `SSID`
/// line, which is an interface that is not connected to a network. macOS
/// 14 and later write `<redacted>` where a program has no permission to see
/// the name.
pub(crate) fn ipconfig_summary(text: &str, interface: &str) -> Option<Link> {
    const REDACTED: &str = "<redacted>";
    let mut ssid = None;
    let mut bssid_line = None;
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "SSID" if ssid.is_none() => ssid = Some(value(rest).filter(|name| name != REDACTED)),
            "BSSID" if bssid_line.is_none() => {
                bssid_line = Some(bssid(rest).filter(|address| address != REDACTED));
            }
            _ => {}
        }
    }
    Some(Link {
        name: Some(interface.to_string()),
        ssid: ssid?,
        bssid: bssid_line.flatten(),
    })
}

#[cfg(test)]
mod tests {
    // The fixtures are written after the formats the tools document and print,
    // with the addresses reserved for documentation (RFC 5737, RFC 3849, the
    // MAC range of RFC 7042); none is copied from a machine.
    use super::*;

    fn link(name: Option<&str>, ssid: Option<&str>, bssid: Option<&str>) -> Link {
        Link {
            name: name.map(str::to_string),
            ssid: ssid.map(str::to_string),
            bssid: bssid.map(str::to_string),
        }
    }

    fn excluding(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    const NETSH_WINDOWS_10: &str = r"
There is 1 interface on the system:

    Name                   : Wi-Fi
    Description            : Example Wireless Adapter
    GUID                   : 00000000-0000-0000-0000-000000000000
    Physical address       : 00:00:5e:00:53:01
    State                  : connected
    SSID                   : ExampleNet
    BSSID                  : 00:00:5e:00:53:02
    Network type           : Infrastructure
    Radio type             : 802.11ac
    Authentication         : WPA2-Personal
    Cipher                 : CCMP
    Connection mode        : Auto Connect
    Channel                : 36
    Receive rate (Mbps)    : 866.7
    Transmit rate (Mbps)   : 866.7
    Signal                 : 99%
    Profile                : ExampleNet

    Hosted network status  : Not available
";

    const NETSH_WINDOWS_11: &str = r"
There is 1 interface on the system:

    Name                   : Wi-Fi
    Description            : Example Wireless Adapter
    GUID                   : 00000000-0000-0000-0000-000000000000
    Physical address       : 00:00:5e:00:53:01
    Interface type         : Primary
    State                  : connected
    SSID                   : ExampleNet
    AP BSSID               : 00:00:5e:00:53:02
    Band                   : 5 GHz
    Channel                : 36
    Network type           : Infrastructure
    Radio type             : 802.11ax
    Authentication         : WPA2-Personal
    Cipher                 : CCMP
    Connection mode        : Auto Connect
    Receive rate (Mbps)    : 1201
    Transmit rate (Mbps)   : 1201
    Signal                 : 97%
    Profile                : ExampleNet

    Hosted network status  : Not available
";

    const NETSH_DISCONNECTED: &str = r"
There is 1 interface on the system:

    Name                   : Wi-Fi
    Description            : Example Wireless Adapter
    GUID                   : 00000000-0000-0000-0000-000000000000
    Physical address       : 00:00:5e:00:53:01
    Interface type         : Primary
    State                  : disconnected
    Radio status           : Hardware On
                             Software On

    Hosted network status  : Not available
";

    #[test]
    fn netsh_reads_the_ssid_and_bssid_of_windows_10() {
        assert_eq!(
            netsh_wlan(NETSH_WINDOWS_10),
            Some(link(
                Some("Wi-Fi"),
                Some("ExampleNet"),
                Some("00:00:5e:00:53:02")
            ))
        );
    }

    #[test]
    fn netsh_reads_the_ap_bssid_of_windows_11() {
        assert_eq!(
            netsh_wlan(NETSH_WINDOWS_11),
            Some(link(
                Some("Wi-Fi"),
                Some("ExampleNet"),
                Some("00:00:5e:00:53:02")
            ))
        );
    }

    /// An adapter that is not connected has no `SSID` line.
    #[test]
    fn netsh_without_an_ssid_line_is_not_connected() {
        assert_eq!(netsh_wlan(NETSH_DISCONNECTED), None);
        assert_eq!(netsh_wlan(""), None);
    }

    /// Windows 11 without the permission to see where it is says so instead of
    /// printing the interfaces: no line is the `SSID` one, and the text is not
    /// looked at for anything else.
    #[test]
    fn netsh_with_only_a_notice_has_no_link() {
        let text = "Network shell commands need location permission to access WLAN \
                    information. Turn on Location services in Settings.\n\
                    \n\
                    Hosted network status  : Not available\n";
        assert_eq!(netsh_wlan(text), None);
        // the SSID line is there and empty: a network is joined, its name is not told
        let hidden = NETSH_WINDOWS_11.replace("ExampleNet\n    AP", "\n    AP");
        assert_eq!(
            netsh_wlan(&hidden),
            Some(link(Some("Wi-Fi"), None, Some("00:00:5e:00:53:02")))
        );
    }

    /// The value is what follows the first colon, whatever it holds, without
    /// the white space around it.
    #[test]
    fn netsh_ssid_may_hold_colons_spaces_and_non_ascii_letters() {
        let text = NETSH_WINDOWS_10.replace("ExampleNet\n    BSSID", "Café Wi-Fi: 5G  \n    BSSID");
        let parsed = netsh_wlan(&text).expect("connected");
        assert_eq!(parsed.ssid.as_deref(), Some("Café Wi-Fi: 5G"));
        assert_eq!(parsed.bssid.as_deref(), Some("00:00:5e:00:53:02"));
        // the `Profile` line still says ExampleNet: the name is the SSID line's
        assert_eq!(parsed.name.as_deref(), Some("Wi-Fi"));
    }

    /// With several adapters the first one that is connected is the link, with
    /// its own BSSID.
    #[test]
    fn netsh_takes_the_first_adapter_that_is_connected() {
        let text = r"
There are 3 interfaces on the system:

    Name                   : Local Area Connection* 1
    State                  : disconnected

    Name                   : Wi-Fi 2
    State                  : connected
    SSID                   : SecondNet
    BSSID                  : 00:00:5e:00:53:20

    Name                   : Wi-Fi
    State                  : connected
    SSID                   : ThirdNet
    BSSID                  : 00:00:5e:00:53:30
";
        assert_eq!(
            netsh_wlan(text),
            Some(link(
                Some("Wi-Fi 2"),
                Some("SecondNet"),
                Some("00:00:5e:00:53:20")
            ))
        );
    }

    /// The word `Name` is translated on other languages' Windows; `SSID` is
    /// not. The link is found, and it has no name.
    #[test]
    fn netsh_on_a_translated_windows_has_a_link_without_a_name() {
        let text = r"
    Nom                    : Wi-Fi
    État                   : connecté
    SSID                   : ExampleNet
    BSSID                  : 00:00:5e:00:53:02
";
        assert_eq!(
            netsh_wlan(text),
            Some(link(None, Some("ExampleNet"), Some("00:00:5e:00:53:02")))
        );
    }

    const PROC_NET_ROUTE: &str = concat!(
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n",
        "wlan0\t00000000\t016433C6\t0003\t0\t0\t600\t00000000\t0\t0\t0\n",
        "eth0\t00000000\t010200C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n",
        "eth0\t000200C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n",
        "wlan0\t006433C6\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n",
    );

    #[test]
    fn proc_net_route_takes_the_default_route_with_the_lowest_metric() {
        // the gateways are 192.0.2.1 and 198.51.100.1, in the order the
        // kernel prints them: the first address byte last
        assert_eq!(
            proc_net_route(PROC_NET_ROUTE, &[]),
            Some(("eth0".to_string(), "192.0.2.1".parse().unwrap()))
        );
        // the first of two with the same metric
        let tied = PROC_NET_ROUTE.replace("\t600\t", "\t100\t");
        assert_eq!(
            proc_net_route(&tied, &[]),
            Some(("wlan0".to_string(), "198.51.100.1".parse().unwrap()))
        );
    }

    #[test]
    fn proc_net_route_leaves_out_what_is_excluded() {
        assert_eq!(
            proc_net_route(PROC_NET_ROUTE, &excluding(&["eth0"])),
            Some(("wlan0".to_string(), "198.51.100.1".parse().unwrap()))
        );
        assert_eq!(
            proc_net_route(PROC_NET_ROUTE, &excluding(&["eth0", "wlan0"])),
            None
        );
    }

    /// Only a default route with a gateway counts: the routes of the networks
    /// the machine is on, a default route that leaves by an interface alone
    /// (flags without the gateway bit), and a header with nothing under it do
    /// not.
    #[test]
    fn proc_net_route_without_a_default_route_with_a_gateway_has_none() {
        let no_default = concat!(
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n",
            "eth0\t000200C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n",
            "ppp0\t00000000\t00000000\t0001\t0\t0\t50\t00000000\t0\t0\t0\n",
            // an address in the gateway column, without the gateway flag
            "eth1\t00000000\t010200C0\t0001\t0\t0\t10\t00000000\t0\t0\t0\n",
            // a gateway for one network, not for all of them
            "eth2\t000200C0\t010200C0\t0003\t0\t0\t10\t00FFFFFF\t0\t0\t0\n",
            // 0.0.0.0/8, and a destination that is not 0.0.0.0 with the mask of all of it
            "eth3\t00000000\t010200C0\t0003\t0\t0\t10\t000000FF\t0\t0\t0\n",
            "eth4\t000200C0\t010200C0\t0003\t0\t0\t10\t00000000\t0\t0\t0\n",
        );
        assert_eq!(proc_net_route(no_default, &[]), None);
        assert_eq!(proc_net_route("", &[]), None);
        assert_eq!(proc_net_route("not a routing table\nat all\n", &[]), None);
        // a gateway of 0.0.0.0 is no router
        let zero = no_default.replace(
            "ppp0\t00000000\t00000000\t0001",
            "ppp0\t00000000\t00000000\t0003",
        );
        assert_eq!(proc_net_route(&zero, &[]), None);
    }

    // destination, prefix length, source, prefix length, next hop, metric,
    // reference count, use, flags, interface — hexadecimal, no separators
    const PROC_IPV6_ROUTE: &str = concat!(
        "00000000000000000000000000000000 00 00000000000000000000000000000000 00 ",
        "fe800000000000000000000000000001 00000400 00000001 00000000 00000003     eth0\n",
        "00000000000000000000000000000000 00 00000000000000000000000000000000 00 ",
        "fe8000000000000000000000000000fe 00000100 00000001 00000000 00000003    wlan0\n",
        "20010db8000000000000000000000000 40 00000000000000000000000000000000 00 ",
        "00000000000000000000000000000000 00000100 00000001 00000000 00000001     eth0\n",
        "00000000000000000000000000000000 00 00000000000000000000000000000000 00 ",
        "00000000000000000000000000000000 ffffffff 00000001 00000000 00200200       lo\n",
    );

    #[test]
    fn proc_ipv6_route_takes_the_default_route_with_the_lowest_metric() {
        assert_eq!(
            proc_ipv6_route(PROC_IPV6_ROUTE, &[]),
            Some(("wlan0".to_string(), "fe80::fe".parse().unwrap()))
        );
        assert_eq!(
            proc_ipv6_route(PROC_IPV6_ROUTE, &excluding(&["wlan0"])),
            Some(("eth0".to_string(), "fe80::1".parse().unwrap()))
        );
        // the reject route on `lo` has no next hop, and the prefix route is not a default
        assert_eq!(
            proc_ipv6_route(PROC_IPV6_ROUTE, &excluding(&["wlan0", "eth0"])),
            None
        );
        assert_eq!(proc_ipv6_route("", &[]), None);
    }

    const NMCLI: &str = concat!(
        "no:OtherNet:00\\:00\\:5E\\:00\\:53\\:10:wlan0\n",
        "yes:Café Wi-Fi\\: 5G:00\\:00\\:5E\\:00\\:53\\:02:wlan0\n",
        "no:Guest:00\\:00\\:5E\\:00\\:53\\:11:wlan0\n",
    );

    /// In the terse format a `:` inside a value is written `\:`, and so a
    /// BSSID and an SSID that holds a colon are one field each.
    #[test]
    fn nmcli_reads_the_access_point_that_is_active() {
        assert_eq!(
            nmcli_wifi(NMCLI),
            Some(link(
                Some("wlan0"),
                Some("Café Wi-Fi: 5G"),
                Some("00:00:5E:00:53:02")
            ))
        );
        // a backslash is written twice
        let backslash = "yes:a\\\\b:00\\:00\\:5E\\:00\\:53\\:02:wlan1\n";
        assert_eq!(
            nmcli_wifi(backslash),
            Some(link(Some("wlan1"), Some("a\\b"), Some("00:00:5E:00:53:02")))
        );
    }

    #[test]
    fn nmcli_without_an_active_access_point_has_none() {
        let idle = NMCLI.replace("yes:", "no:");
        assert_eq!(nmcli_wifi(&idle), None);
        assert_eq!(nmcli_wifi(""), None);
        // a line with fewer fields than were asked for is not an answer
        assert_eq!(nmcli_wifi("yes:ExampleNet\n"), None);
    }

    /// A hidden network has an empty SSID.
    #[test]
    fn nmcli_with_an_empty_ssid_says_nothing_of_the_name() {
        let hidden = "yes::00\\:00\\:5E\\:00\\:53\\:02:wlan0\n";
        assert_eq!(
            nmcli_wifi(hidden),
            Some(link(Some("wlan0"), None, Some("00:00:5E:00:53:02")))
        );
    }

    const IW_CONNECTED: &str = concat!(
        "Connected to 00:00:5e:00:53:02 (on wlan0)\n",
        "\tSSID: Example Net: 5G\n",
        "\tfreq: 5180.0\n",
        "\tRX: 123456 bytes (1234 packets)\n",
        "\tTX: 12345 bytes (123 packets)\n",
        "\tsignal: -52 dBm\n",
        "\trx bitrate: 866.7 MBit/s\n",
        "\n",
        "\tbss flags:\tshort-slot-time\n",
        "\tdtim period:\t3\n",
        "\tbeacon int:\t100\n",
    );

    #[test]
    fn iw_reads_a_connected_interface() {
        assert_eq!(
            iw_link(IW_CONNECTED, "wlan0"),
            Some(link(
                Some("wlan0"),
                Some("Example Net: 5G"),
                Some("00:00:5e:00:53:02")
            ))
        );
    }

    #[test]
    fn iw_on_an_interface_that_is_not_connected_has_none() {
        assert_eq!(iw_link("Not connected.\n", "wlan0"), None);
        assert_eq!(iw_link("", "wlan0"), None);
        assert_eq!(
            iw_link("command failed: No such device (-19)\n", "wlan9"),
            None
        );
    }

    const ROUTE_GET_V4: &str = r"
   route to: default
destination: default
       mask: default
    gateway: 192.0.2.1
  interface: en0
      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING,GLOBAL>
 recvpipe  sendpipe  ssthresh  rtt,msec    rttvar  hopcount      mtu     expire
       0         0         0         0         0         0      1500         0
";

    const ROUTE_GET_V6: &str = r"
   route to: default
destination: default
       mask: default
    gateway: fe80::1%en0
  interface: en0
      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING,GLOBAL>
";

    #[test]
    fn route_get_default_reads_the_interface_and_the_gateway() {
        assert_eq!(
            route_get_default(ROUTE_GET_V4),
            Some(("en0".to_string(), "192.0.2.1".parse().unwrap()))
        );
        // the gateway of an IPv6 route is written with the interface it is on
        assert_eq!(
            route_get_default(ROUTE_GET_V6),
            Some(("en0".to_string(), "fe80::1".parse().unwrap()))
        );
    }

    #[test]
    fn route_get_default_without_a_gateway_that_is_an_address_has_none() {
        let on_link = ROUTE_GET_V4.replace("192.0.2.1", "link#4");
        assert_eq!(route_get_default(&on_link), None);
        let none = ROUTE_GET_V4.replace("    gateway: 192.0.2.1\n", "");
        assert_eq!(route_get_default(&none), None);
        assert_eq!(
            route_get_default("route: writing to routing socket: not in table\n"),
            None
        );
        assert_eq!(route_get_default(""), None);
    }

    const HARDWARE_PORTS: &str = r"
Hardware Port: Ethernet Adapter (en4)
Device: en4
Ethernet Address: 00:00:5e:00:53:04

Hardware Port: Thunderbolt Bridge
Device: bridge0
Ethernet Address: N/A

Hardware Port: Wi-Fi
Device: en0
Ethernet Address: 00:00:5e:00:53:03

Hardware Port: USB 10/100/1000 LAN
Device: en7
Ethernet Address: 00:00:5e:00:53:07

Hardware Port: iPhone USB
Device: en9
Ethernet Address: 00:00:5e:00:53:09

VLAN Configurations
===================
";

    #[test]
    fn hardware_ports_tell_wifi_from_ethernet_from_the_rest() {
        assert_eq!(
            hardware_ports(HARDWARE_PORTS),
            vec![
                ("en4".to_string(), NetKind::Wired),
                ("bridge0".to_string(), NetKind::Other),
                ("en0".to_string(), NetKind::Wifi),
                ("en7".to_string(), NetKind::Wired),
                ("en9".to_string(), NetKind::Other),
            ]
        );
        assert_eq!(hardware_ports(""), Vec::new());
    }

    /// The words of the port's name decide: `WLAN` is not `LAN`, and older
    /// macOS calls the Wi-Fi port `AirPort`.
    #[test]
    fn hardware_ports_read_the_name_by_its_words() {
        let text = "Hardware Port: AirPort\nDevice: en1\n\n\
                    Hardware Port: Wi-Fi 2\nDevice: en2\n\n\
                    Hardware Port: WLAN Adapter\nDevice: en3\n\n\
                    Hardware Port: Gigabit Ethernet\nDevice: en5\n";
        assert_eq!(
            hardware_ports(text),
            vec![
                ("en1".to_string(), NetKind::Wifi),
                ("en2".to_string(), NetKind::Wifi),
                ("en3".to_string(), NetKind::Other),
                ("en5".to_string(), NetKind::Wired),
            ]
        );
    }

    const GETSUMMARY: &str = r"
<dictionary> {
  Active : TRUE
  BSSID : 0:0:5e:0:53:2
  IPv4 : <array> {
    0 : <dictionary> {
      Addresses : <array> {
        0 : 192.0.2.10
      }
      Router : 192.0.2.1
    }
  }
  InterfaceType : WiFi
  LinkStatusActive : TRUE
  SSID : Example Net
  Security : WPA2_PSK
}
";

    /// macOS writes the octets of a BSSID without their leading zeros; the
    /// link has them back, so that one BSSID is written one way on every
    /// platform.
    #[test]
    fn ipconfig_summary_reads_the_ssid_and_pads_the_bssid() {
        assert_eq!(
            ipconfig_summary(GETSUMMARY, "en0"),
            Some(link(
                Some("en0"),
                Some("Example Net"),
                Some("00:00:5e:00:53:02")
            ))
        );
    }

    #[test]
    fn ipconfig_summary_takes_redacted_for_a_name_it_would_not_say() {
        let redacted = GETSUMMARY
            .replace("Example Net", "<redacted>")
            .replace("0:0:5e:0:53:2", "<redacted>");
        assert_eq!(
            ipconfig_summary(&redacted, "en0"),
            Some(link(Some("en0"), None, None))
        );
    }

    #[test]
    fn ipconfig_summary_without_an_ssid_line_is_not_connected() {
        let off = "<dictionary> {\n  Active : FALSE\n  InterfaceType : WiFi\n}\n";
        assert_eq!(ipconfig_summary(off, "en0"), None);
        assert_eq!(ipconfig_summary("", "en0"), None);
    }
}
