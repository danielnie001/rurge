//! Where rurge's own sockets go while the enhanced mode is on (M4 design 7.1):
//! the table, the decision and the subnets. The reader is the binary's
//! `PlatformSockets`.

use crate::route::RouteSet;
use ipnet::IpNet;
use std::io;
use std::net::IpAddr;

/// What decides where rurge's own sockets go while the enhanced mode is on
/// (M4 design 7.1). Built by the manager when the enhanced mode opens, closes,
/// follows a reload or a change of network.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EgressTable {
    pub routes: RouteSet,
    /// The interface the network environment calls primary (never the virtual adapter).
    pub primary: Option<String>,
    /// The directly connected subnets of the interfaces but the virtual adapter,
    /// longest prefix first: (subnet, interface).
    pub subnets: Vec<(IpNet, String)>,
}

/// No interface outside the enhanced mode reaches the address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoRoute;

/// The interface rurge's own socket to `dst` is bound to; `None` leaves it to
/// the system's routes.
pub fn decide(table: Option<&EgressTable>, dst: IpAddr) -> Result<Option<&str>, NoRoute> {
    let Some(table) = table else {
        return Ok(None);
    };
    // An IPv4 address written as the IPv6 address it maps to (`::ffff:a.b.c.d`)
    // is an IPv4 address: a dual-stack IPv6 socket that dials it sends IPv4
    // packets, so rules 2 to 4 have to see it as one (the phase 3 / M4b plan's
    // execution record row 71).
    let dst = dst.to_canonical();
    if never_bound(dst) {
        return Ok(None);
    }
    // A directly connected subnet alone names the interface the system's own
    // routes would take, which is not always the primary one (rule 3 before 4).
    let mut best: Option<&(IpNet, String)> = None;
    for entry in &table.subnets {
        if entry.0.contains(&dst)
            && best.is_none_or(|best| entry.0.prefix_len() > best.0.prefix_len())
        {
            best = Some(entry);
        }
    }
    if let Some((_, interface)) = best {
        return Ok(Some(interface));
    }
    if table.routes.contains(dst) {
        return match table.primary.as_deref() {
            Some(primary) => Ok(Some(primary)),
            None => Err(NoRoute),
        };
    }
    Ok(None)
}

/// `subnets` out of addresses: (interface, address, prefix length). Loopback and
/// link-local addresses, a prefix of 0 and the interfaces in `exclude` are left
/// out; each subnet once, longest first. A subnet two adapters share (a cable
/// and Wi-Fi on one LAN) is listed with the one that comes first.
pub fn subnets_of(addrs: &[(String, IpAddr, u8)], exclude: &[String]) -> Vec<(IpNet, String)> {
    let mut out: Vec<(IpNet, String)> = Vec::new();
    for (interface, ip, prefix) in addrs {
        if *prefix == 0 || exclude.iter().any(|name| name == interface) || never_bound(*ip) {
            continue;
        }
        // a prefix longer than the address is no subnet of it
        let Ok(net) = IpNet::new(*ip, *prefix) else {
            continue;
        };
        // each subnet once: the first address that names it keeps it, whether
        // or not the two end up as neighbours once the list is sorted
        let net = net.trunc();
        if !out.iter().any(|(known, _)| *known == net) {
            out.push((net, interface.clone()));
        }
    }
    // A stable sort, so that subnets of the same length keep the addresses'
    // order.
    out.sort_by_key(|(net, _)| std::cmp::Reverse(net.prefix_len()));
    out
}

/// The subnets of this machine's interfaces but `exclude` (`if_addrs`, read only).
pub fn subnets(exclude: &[String]) -> io::Result<Vec<(IpNet, String)>> {
    let mut addrs: Vec<(String, IpAddr, u8)> = Vec::new();
    for interface in if_addrs::get_if_addrs()? {
        let (ip, prefix) = match interface.addr {
            if_addrs::IfAddr::V4(v4) => (IpAddr::V4(v4.ip), v4.prefixlen),
            if_addrs::IfAddr::V6(v6) => (IpAddr::V6(v6.ip), v6.prefixlen),
        };
        addrs.push((interface.name, ip, prefix));
    }
    Ok(subnets_of(&addrs, exclude))
}

/// An address that never goes out of an interface of ours: loopback,
/// unspecified, multicast, link-local, broadcast (rule 2 of design 7.1).
fn never_bound(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_link_local()
                || v4 == std::net::Ipv4Addr::BROADCAST
        }
        IpAddr::V6(v6) => {
            // fe80::/10 is link-local
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::RouteSet;
    use ipnet::{IpNet, Ipv4Net, Ipv6Net};

    fn net(s: &str) -> IpNet {
        s.parse().unwrap()
    }

    fn dst(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn subnets(entries: &[(&str, &str)]) -> Vec<(IpNet, String)> {
        entries
            .iter()
            .map(|(net, interface)| (self::net(net), (*interface).to_string()))
            .collect()
    }

    fn routes(v4: &[&str], v6: &[&str]) -> RouteSet {
        RouteSet {
            v4: v4.iter().map(|s| s.parse::<Ipv4Net>().unwrap()).collect(),
            v6: v6.iter().map(|s| s.parse::<Ipv6Net>().unwrap()).collect(),
        }
    }

    fn table(
        subnets: Vec<(IpNet, String)>,
        routes: RouteSet,
        primary: Option<&str>,
    ) -> EgressTable {
        EgressTable {
            routes,
            primary: primary.map(str::to_string),
            subnets,
        }
    }

    #[test]
    fn nothing_is_bound_while_the_enhanced_mode_is_off() {
        assert_eq!(decide(None, dst("192.168.1.5")), Ok(None));
    }

    #[test]
    fn loopback_unspecified_multicast_link_local_and_broadcast_are_never_bound() {
        // A table that would take every one of them: the decision leaves them
        // alone before it looks at either the subnets or the routes.
        let t = table(
            subnets(&[("0.0.0.0/0", "eth0"), ("::/0", "eth0")]),
            routes(&["0.0.0.0/0"], &["::/0"]),
            Some("eth0"),
        );
        for ip in [
            "127.0.0.1",
            "::1",
            "0.0.0.0",
            "::",
            "224.0.0.251",
            "ff02::fb",
            "169.254.1.1",
            "fe80::1",
            "255.255.255.255",
        ] {
            assert_eq!(decide(Some(&t), dst(ip)), Ok(None), "{ip}");
        }
    }

    #[test]
    fn a_directly_connected_address_goes_out_of_its_interface_before_the_routes_are_asked() {
        let t = table(
            subnets(&[("192.168.1.0/24", "eth0")]),
            routes(&["192.168.0.0/16"], &[]),
            Some("Wi-Fi"),
        );
        assert_eq!(decide(Some(&t), dst("192.168.1.5")), Ok(Some("eth0")));
        // in the routes but not directly connected: the primary interface
        assert_eq!(decide(Some(&t), dst("192.168.2.5")), Ok(Some("Wi-Fi")));
    }

    /// An IPv4 address written as the IPv6 address it maps to is decided as
    /// the IPv4 address it is: a dual-stack IPv6 socket that dials it sends
    /// IPv4 packets, so rules 3 and 4 must see IPv4 (the phase 3 / M4b plan's
    /// execution record row 71).
    #[test]
    fn an_ipv4_address_in_its_mapped_form_is_decided_as_an_ipv4_address() {
        let t = table(
            subnets(&[("192.168.1.0/24", "eth0")]),
            routes(&["0.0.0.0/0"], &["::/0"]),
            Some("Wi-Fi"),
        );
        assert_eq!(
            decide(Some(&t), dst("::ffff:192.168.1.5")),
            Ok(Some("eth0"))
        );
        assert_eq!(decide(Some(&t), dst("::ffff:8.8.8.8")), Ok(Some("Wi-Fi")));
        // and what is never bound in IPv4 is never bound in that form either
        assert_eq!(decide(Some(&t), dst("::ffff:127.0.0.1")), Ok(None));
        assert_eq!(decide(Some(&t), dst("::ffff:169.254.1.1")), Ok(None));
    }

    #[test]
    fn the_longest_subnet_wins() {
        let t = table(
            subnets(&[("10.0.0.0/8", "a"), ("10.1.0.0/16", "b")]),
            routes(&["0.0.0.0/0"], &[]),
            Some("Wi-Fi"),
        );
        assert_eq!(decide(Some(&t), dst("10.1.2.3")), Ok(Some("b")));
        assert_eq!(decide(Some(&t), dst("10.2.3.4")), Ok(Some("a")));
    }

    #[test]
    fn an_address_in_the_routes_goes_out_of_the_primary_interface() {
        let t = table(
            Vec::new(),
            routes(&["1.0.0.0/8"], &["2001:db8::/32"]),
            Some("Wi-Fi"),
        );
        assert_eq!(decide(Some(&t), dst("1.2.3.4")), Ok(Some("Wi-Fi")));
        assert_eq!(decide(Some(&t), dst("2001:db8::5")), Ok(Some("Wi-Fi")));
        // rurge's own space is in the routes whatever the profile says
        assert_eq!(decide(Some(&t), dst("198.18.0.5")), Ok(Some("Wi-Fi")));
        assert_eq!(decide(Some(&t), dst("fd00:6152::5")), Ok(Some("Wi-Fi")));
    }

    #[test]
    fn without_a_primary_interface_it_is_no_route() {
        let t = table(Vec::new(), routes(&["1.0.0.0/8"], &[]), None);
        assert_eq!(decide(Some(&t), dst("1.2.3.4")), Err(NoRoute));
        assert_eq!(decide(Some(&t), dst("198.18.0.5")), Err(NoRoute));
    }

    #[test]
    fn an_excluded_address_is_left_to_the_system() {
        // `tun-excluded-routes` (10.0.0.0/8 here) never enters the routes, and
        // the adapter's subnet is not one of the physical interfaces' either.
        let t = table(
            subnets(&[("192.168.1.0/24", "eth0")]),
            routes(&["1.0.0.0/8"], &[]),
            Some("Wi-Fi"),
        );
        assert_eq!(decide(Some(&t), dst("10.0.0.5")), Ok(None));
        assert_eq!(decide(Some(&t), dst("2001:db8::5")), Ok(None));
    }

    #[test]
    fn subnets_of_leaves_out_loopback_link_local_the_excluded_and_a_zero_prefix() {
        let addrs = [
            ("lo", "127.0.0.1", 8),
            ("lo", "::1", 128),
            ("eth0", "169.254.10.1", 16),
            ("eth0", "fe80::1", 64),
            ("eth0", "192.168.1.5", 0),
            ("eth0", "2001:db8::5", 0),
            ("wlan0", "192.168.1.5", 24),
            ("eth0", "192.168.1.5", 24),
            ("eth0", "2001:db8::5", 64),
            // a prefix longer than the address: nothing to take out of it
            ("eth0", "192.168.2.7", 33),
            ("eth0", "2001:db8:1::7", 129),
        ];
        let addrs: Vec<(String, IpAddr, u8)> = addrs
            .iter()
            .map(|(name, ip, prefix)| ((*name).to_string(), dst(ip), *prefix))
            .collect();
        assert_eq!(
            subnets_of(&addrs, &["wlan0".to_string()]),
            vec![
                (net("2001:db8::/64"), "eth0".to_string()),
                (net("192.168.1.0/24"), "eth0".to_string()),
            ]
        );
    }

    #[test]
    fn subnets_of_lists_each_subnet_once_longest_first() {
        let addrs = [
            // the same subnet on two adapters (a cable and Wi-Fi on one LAN):
            // the one that comes first stays — with another /24 in between, so
            // that the two are no neighbours once the subnets are sorted
            ("eth0", "192.168.1.5", 24),
            ("eth0", "10.9.9.9", 24),
            ("wlan0", "192.168.1.20", 24),
            // the address is written as the subnet's network address
            ("eth0", "10.1.2.3", 16),
            ("eth0", "10.0.0.5", 8),
            ("eth0", "10.0.0.9", 8),
        ];
        let addrs: Vec<(String, IpAddr, u8)> = addrs
            .iter()
            .map(|(name, ip, prefix)| ((*name).to_string(), dst(ip), *prefix))
            .collect();
        assert_eq!(
            subnets_of(&addrs, &[]),
            vec![
                (net("192.168.1.0/24"), "eth0".to_string()),
                (net("10.9.9.0/24"), "eth0".to_string()),
                (net("10.1.0.0/16"), "eth0".to_string()),
                (net("10.0.0.0/8"), "eth0".to_string()),
            ]
        );
    }
}
