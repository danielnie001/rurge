//! The set of routes into the adapter (design 4.7): what a profile's
//! `tun-*` items, the external policies' own addresses and the platform's way
//! of bringing rurge's space add up to, and what changes when one of them does.
//!
//! A pure function of its inputs, so every platform tests it with data: the
//! shape of the base prefixes, what is always subtracted, the complement of the
//! holes, included winning over excluded, and rurge's own space staying in.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ipnet::{IpNet, Ipv4Net, Ipv6Net};

use super::{SPACE_V4, SPACE_V6};

/// What a route set is computed from (design 4.4, 4.5).
pub struct RouteInputs<'a> {
    /// `ipv6-vif` is on for this network: the IPv6 base prefixes, holes and
    /// included routes are used.
    pub ipv6: bool,
    /// `tun-excluded-routes`.
    pub excluded: &'a [IpNet],
    /// `tun-included-routes`.
    pub included: &'a [IpNet],
    /// The `addresses` of every `external` policy.
    pub external: &'a [IpAddr],
    /// The platform installs rurge's space as a route of its own (macOS:
    /// utun is point-to-point and its address brings no subnet route).
    /// Elsewhere the adapter's address brings it.
    pub explicit_space: bool,
}

/// The routes into the adapter: each family sorted, merged, each once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteSet {
    pub v4: Vec<Ipv4Net>,
    pub v6: Vec<Ipv6Net>,
}

/// The base prefixes traffic into the adapter starts from (design 4.2): eight
/// prefixes rather than a pair of `/1`s, so that the adapter does not hold a
/// route longer than the ones the system computes for its own links.
const BASE_V4: [Ipv4Net; 8] = [
    Ipv4Net::new_assert(Ipv4Addr::new(1, 0, 0, 0), 8),
    Ipv4Net::new_assert(Ipv4Addr::new(2, 0, 0, 0), 7),
    Ipv4Net::new_assert(Ipv4Addr::new(4, 0, 0, 0), 6),
    Ipv4Net::new_assert(Ipv4Addr::new(8, 0, 0, 0), 5),
    Ipv4Net::new_assert(Ipv4Addr::new(16, 0, 0, 0), 4),
    Ipv4Net::new_assert(Ipv4Addr::new(32, 0, 0, 0), 3),
    Ipv4Net::new_assert(Ipv4Addr::new(64, 0, 0, 0), 2),
    Ipv4Net::new_assert(Ipv4Addr::new(128, 0, 0, 0), 1),
];

/// The same for IPv6, with the NAT64 prefix the only piece of `::/8` that goes
/// in (design 4.2).
const BASE_V6: [Ipv6Net; 9] = [
    Ipv6Net::new_assert(Ipv6Addr::new(0x0100, 0, 0, 0, 0, 0, 0, 0), 8),
    Ipv6Net::new_assert(Ipv6Addr::new(0x0200, 0, 0, 0, 0, 0, 0, 0), 7),
    Ipv6Net::new_assert(Ipv6Addr::new(0x0400, 0, 0, 0, 0, 0, 0, 0), 6),
    Ipv6Net::new_assert(Ipv6Addr::new(0x0800, 0, 0, 0, 0, 0, 0, 0), 5),
    Ipv6Net::new_assert(Ipv6Addr::new(0x1000, 0, 0, 0, 0, 0, 0, 0), 4),
    Ipv6Net::new_assert(Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0), 3),
    Ipv6Net::new_assert(Ipv6Addr::new(0x4000, 0, 0, 0, 0, 0, 0, 0), 2),
    Ipv6Net::new_assert(Ipv6Addr::new(0x8000, 0, 0, 0, 0, 0, 0, 0), 1),
    Ipv6Net::new_assert(Ipv6Addr::new(0x0064, 0xff9b, 0, 0, 0, 0, 0, 0), 96),
];

/// The last IPv4 address: the broadcast, which never goes in (design 4.3).
const BROADCAST_V4: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 255);

/// What never goes into the adapter, whatever the profile says (design 4.3):
/// loopback, link-local, multicast and broadcast. Multicast and broadcast are
/// dropped there anyway; taking them out keeps the system from sending them.
const ALWAYS_V4: [Ipv4Net; 4] = [
    Ipv4Net::new_assert(Ipv4Addr::new(127, 0, 0, 0), 8),
    Ipv4Net::new_assert(Ipv4Addr::new(169, 254, 0, 0), 16),
    Ipv4Net::new_assert(Ipv4Addr::new(224, 0, 0, 0), 4),
    Ipv4Net::new_assert(BROADCAST_V4, 32),
];

/// The IPv6 half of the same (design 4.3).
const ALWAYS_V6: [Ipv6Net; 2] = [
    Ipv6Net::new_assert(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
    Ipv6Net::new_assert(Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),
];

/// The prefixes traffic into the adapter starts from: [`BASE_V4`], and
/// [`BASE_V6`] when IPv6 is on.
fn base(ipv6: bool) -> (Vec<Ipv4Net>, Vec<Ipv6Net>) {
    let v6 = if ipv6 { BASE_V6.to_vec() } else { Vec::new() };
    (BASE_V4.to_vec(), v6)
}

/// What never goes into the adapter (design 4.3, 4.4): the always-excluded
/// prefixes, `tun-excluded-routes`, and the address of every external program,
/// less the part of each that falls in rurge's own space (design 4.5).
fn holes_v4(inputs: &RouteInputs<'_>) -> Vec<Ipv4Net> {
    let mut holes: Vec<Ipv4Net> = ALWAYS_V4.to_vec();
    for net in inputs.excluded {
        if let IpNet::V4(net) = net {
            holes.push(net.trunc());
        }
    }
    for ip in inputs.external {
        if let IpAddr::V4(ip) = ip {
            holes.push(Ipv4Net::new_assert(*ip, 32));
        }
    }
    strip_space_v4(holes)
}

/// The IPv6 holes, the same way.
fn holes_v6(inputs: &RouteInputs<'_>) -> Vec<Ipv6Net> {
    let mut holes: Vec<Ipv6Net> = ALWAYS_V6.to_vec();
    for net in inputs.excluded {
        if let IpNet::V6(net) = net {
            holes.push(net.trunc());
        }
    }
    for ip in inputs.external {
        if let IpAddr::V6(ip) = ip {
            holes.push(Ipv6Net::new_assert(*ip, 128));
        }
    }
    strip_space_v6(holes)
}

/// Every hole less its part inside rurge's space: stripping that part would
/// send the responder and every fake address out by the physical adapter, and
/// the whole enhanced mode with them (design 4.5).
fn strip_space_v4(holes: Vec<Ipv4Net>) -> Vec<Ipv4Net> {
    let mut stripped = Vec::with_capacity(holes.len());
    for hole in holes {
        complement_v4(hole, &[SPACE_V4], &mut stripped);
    }
    stripped
}

/// The IPv6 half of [`strip_space_v4`].
fn strip_space_v6(holes: Vec<Ipv6Net>) -> Vec<Ipv6Net> {
    let mut stripped = Vec::with_capacity(holes.len());
    for hole in holes {
        complement_v6(hole, &[SPACE_V6], &mut stripped);
    }
    stripped
}

/// The complement of `net` within `holes`: `net` itself when no hole touches
/// it, nothing when a hole covers it, and otherwise its two halves, halved
/// again (design 4.4). Holes are prefixes, so a hole and `net` either are
/// disjoint or one covers the other.
fn complement_v4(net: Ipv4Net, holes: &[Ipv4Net], out: &mut Vec<Ipv4Net>) {
    if holes.iter().any(|hole| hole.contains(&net)) {
        return;
    }
    if !holes.iter().any(|hole| net.contains(hole)) {
        out.push(net);
        return;
    }
    // a hole is strictly inside `net` here, so `net` has room to be halved
    if let Ok(halves) = net.subnets(net.prefix_len() + 1) {
        for half in halves {
            complement_v4(half, holes, out);
        }
    }
}

/// The IPv6 half of [`complement_v4`].
fn complement_v6(net: Ipv6Net, holes: &[Ipv6Net], out: &mut Vec<Ipv6Net>) {
    if holes.iter().any(|hole| hole.contains(&net)) {
        return;
    }
    if !holes.iter().any(|hole| net.contains(hole)) {
        out.push(net);
        return;
    }
    if let Ok(halves) = net.subnets(net.prefix_len() + 1) {
        for half in halves {
            complement_v6(half, holes, out);
        }
    }
}

impl RouteSet {
    /// The routes into the adapter for these inputs (design 4.4, 4.5, 4.7).
    pub fn compute(inputs: &RouteInputs<'_>) -> RouteSet {
        let (base_v4, base_v6) = base(inputs.ipv6);
        let holes_v4 = holes_v4(inputs);
        let mut v4 = Vec::new();
        for net in base_v4 {
            complement_v4(net, &holes_v4, &mut v4);
        }
        let mut v6 = Vec::new();
        if inputs.ipv6 {
            let holes_v6 = holes_v6(inputs);
            for net in base_v6 {
                complement_v6(net, &holes_v6, &mut v6);
            }
        }

        // `tun-included-routes` wins over what is excluded (design 4.4, Q-03).
        for net in inputs.included {
            match net {
                IpNet::V4(net) => v4.push(net.trunc()),
                IpNet::V6(net) if inputs.ipv6 => v6.push(net.trunc()),
                IpNet::V6(_) => {}
            }
        }

        // rurge's space is no route of its own where the adapter's address
        // brings it, and a duplicate of the kernel's connected route breaks
        // `ip -batch` on Linux (design 4.5).
        v4.retain(|net| !SPACE_V4.contains(net));
        v6.retain(|net| !SPACE_V6.contains(net));
        if inputs.explicit_space {
            v4.push(SPACE_V4);
            if inputs.ipv6 {
                v6.push(SPACE_V6);
            }
        }

        // `Ipv4Net::aggregate` cannot tell a set that stops just short of
        // `255.255.255.255` from one that ends there: it adds one to the last
        // address of an interval, and there is nothing to add to `u32::MAX`.
        // So it covers the broadcast address either way; where nothing routes
        // it (nothing routes it unless `tun-included-routes` says so), take it
        // out again.
        let covered = v4.iter().any(|net| net.contains(&BROADCAST_V4));
        let mut v4 = Ipv4Net::aggregate(&v4);
        if !covered {
            let mut carved = Vec::new();
            for net in v4 {
                complement_v4(net, &[Ipv4Net::new_assert(BROADCAST_V4, 32)], &mut carved);
            }
            v4 = carved;
        }

        RouteSet {
            v4,
            // `Ipv6Net::aggregate` has the same edge at `u128::MAX` as the IPv4
            // one: only a `tun-included-routes` prefix that stops at
            // `ffff:…:fffe` meets it, which is the multicast range (the base
            // prefixes always leave `ff00::/8` out), and it is not handled (the
            // phase 3 / M4a plan's deferred row 1).
            v6: Ipv6Net::aggregate(&v6),
        }
    }

    /// What to add and what to remove to go from `old` to `new`: a route is
    /// its prefix, so a prefix on one side is never on the other, although the
    /// two may cover the same addresses.
    pub fn diff(old: &RouteSet, new: &RouteSet) -> (RouteSet, RouteSet) {
        let added = RouteSet {
            v4: sorted(
                new.v4
                    .iter()
                    .copied()
                    .filter(|net| !old.v4.contains(net))
                    .collect(),
            ),
            v6: sorted(
                new.v6
                    .iter()
                    .copied()
                    .filter(|net| !old.v6.contains(net))
                    .collect(),
            ),
        };
        let removed = RouteSet {
            v4: sorted(
                old.v4
                    .iter()
                    .copied()
                    .filter(|net| !new.v4.contains(net))
                    .collect(),
            ),
            v6: sorted(
                old.v6
                    .iter()
                    .copied()
                    .filter(|net| !new.v6.contains(net))
                    .collect(),
            ),
        };
        (added, removed)
    }

    /// Whether traffic to `ip` goes into the adapter: one of the routes, or
    /// rurge's space, which only the adapter can take anywhere (design 4.5).
    pub fn contains(&self, ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(ip) => SPACE_V4.contains(&ip) || self.v4.iter().any(|net| net.contains(&ip)),
            IpAddr::V6(ip) => SPACE_V6.contains(&ip) || self.v6.iter().any(|net| net.contains(&ip)),
        }
    }

    /// How many routes that is, both families together.
    pub fn len(&self) -> usize {
        self.v4.len() + self.v6.len()
    }

    /// Whether nothing goes into the adapter.
    pub fn is_empty(&self) -> bool {
        self.v4.is_empty() && self.v6.is_empty()
    }
}

/// The prefixes collected from an iterator, sorted like a [`RouteSet`].
fn sorted<T: Ord>(mut nets: Vec<T>) -> Vec<T> {
    nets.sort();
    nets
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use ipnet::{IpNet, Ipv4Net, Ipv6Net};

    use super::*;

    fn net(text: &str) -> IpNet {
        text.parse().unwrap()
    }

    fn net4(text: &str) -> Ipv4Net {
        text.parse().unwrap()
    }

    fn net6(text: &str) -> Ipv6Net {
        text.parse().unwrap()
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    /// Nothing excluded, nothing included, no external address, and rurge's
    /// space brought by the adapter's address rather than a route of its own.
    fn inputs(ipv6: bool) -> RouteInputs<'static> {
        RouteInputs {
            ipv6,
            excluded: &[],
            included: &[],
            external: &[],
            explicit_space: false,
        }
    }

    #[test]
    fn the_base_routes_are_eight_prefixes_each_way() {
        let (v4, v6) = base(true);
        assert_eq!(
            v4,
            [
                net4("1.0.0.0/8"),
                net4("2.0.0.0/7"),
                net4("4.0.0.0/6"),
                net4("8.0.0.0/5"),
                net4("16.0.0.0/4"),
                net4("32.0.0.0/3"),
                net4("64.0.0.0/2"),
                net4("128.0.0.0/1"),
            ]
        );
        assert_eq!(
            v6,
            [
                net6("100::/8"),
                net6("200::/7"),
                net6("400::/6"),
                net6("800::/5"),
                net6("1000::/4"),
                net6("2000::/3"),
                net6("4000::/2"),
                net6("8000::/1"),
                net6("64:ff9b::/96"),
            ]
        );
        // `0.0.0.0/8` never goes in, and of `::/8` only the NAT64 prefix does.
        assert!(
            v4.iter()
                .all(|net| !net.contains(&Ipv4Addr::new(0, 0, 0, 1)))
        );
        let early = net6("::/8");
        let inside: Vec<Ipv6Net> = v6
            .iter()
            .copied()
            .filter(|net| early.contains(net))
            .collect();
        assert_eq!(inside, [net6("64:ff9b::/96")]);

        // IPv6 off: the same eight IPv4 prefixes, none of IPv6.
        let (v4, v6) = base(false);
        assert_eq!(v4.len(), 8);
        assert!(v6.is_empty());
    }

    #[test]
    fn loopback_link_local_multicast_and_broadcast_never_go_in() {
        let set = RouteSet::compute(&inputs(true));
        for address in [
            "127.0.0.1",
            "169.254.1.1",
            "224.0.0.251",
            "239.255.255.250",
            "255.255.255.255",
            "fe80::1",
            "ff02::fb",
        ] {
            assert!(!set.contains(ip(address)), "{address} goes in");
        }
        for address in ["128.0.0.1", "1.1.1.1", "2606:4700::1111"] {
            assert!(set.contains(ip(address)), "{address} stays out");
        }
    }

    #[test]
    fn one_excluded_address_leaves_a_hole_and_no_more() {
        let excluded = [net("8.8.8.8/32")];
        let set = RouteSet::compute(&RouteInputs {
            excluded: &excluded,
            ..inputs(true)
        });
        assert!(!set.contains(ip("8.8.8.8")));
        assert!(set.contains(ip("8.8.8.7")));
        assert!(set.contains(ip("8.8.8.9")));
        // 8.8.8.8 lies in the base prefix 8.0.0.0/5. Taking one address out of
        // a /p leaves one prefix at each of the lengths p+1 to 32 — 27 of them
        // out of 8.0.0.0/5 — and no two of them touch, so nothing merges. The
        // rest of the base set is 55 - 1 = 54 prefixes: one each from
        // 1.0.0.0/8 to 32.0.0.0/3, six from 64.0.0.0/2 (127.0.0.0/8 is out) and
        // forty-three from 128.0.0.0/1 (169.254.0.0/16, 224.0.0.0/4 and
        // 255.255.255.255/32 are out).
        assert_eq!(set.v4.len(), 54 + 27);
        // IPv6 has no hole here: its base set is seven prefixes, eight out of
        // 8000::/1 and 64:ff9b::/96 once fe80::/10 and ff00::/8 are out.
        assert_eq!(set.v6.len(), 16);
    }

    #[test]
    fn adjacent_holes_merge() {
        // 8.8.8.8 and 8.8.8.9 are two addresses side by side: the base prefix
        // 8.0.0.0/5 loses one prefix at each of the lengths 6 to 31 (26 of
        // them) and the two halves of a /31 at the bottom meet, so the result
        // is what one /31 hole leaves.
        let two = [net("8.8.8.8/32"), net("8.8.8.9/32")];
        let one = [net("8.8.8.8/31")];
        let merged = RouteSet::compute(&RouteInputs {
            excluded: &two,
            ..inputs(true)
        });
        let single = RouteSet::compute(&RouteInputs {
            excluded: &one,
            ..inputs(true)
        });
        assert_eq!(merged, single);
        assert_eq!(merged.v4.len(), 54 + 26);
        for address in ["8.8.8.8", "8.8.8.9"] {
            assert!(!merged.contains(ip(address)), "{address} goes in");
        }
        for address in ["8.8.8.7", "8.8.8.10"] {
            assert!(merged.contains(ip(address)), "{address} stays out");
        }
    }

    #[test]
    fn a_hole_that_covers_a_base_prefix_takes_it_whole() {
        let excluded = [net("16.0.0.0/4")];
        let set = RouteSet::compute(&RouteInputs {
            excluded: &excluded,
            ..inputs(true)
        });
        assert_eq!(set.v4.len(), 54);
        assert!(!set.contains(ip("16.0.0.1")));
        assert!(set.contains(ip("15.255.255.254")));
        assert!(set.contains(ip("32.0.0.1")));
    }

    #[test]
    fn a_hole_across_two_base_prefixes() {
        // 0.0.0.0/1 takes the base prefixes 1.0.0.0/8 through 64.0.0.0/2 whole
        // — seven of them — and leaves what 128.0.0.0/1 leaves: 43 prefixes.
        let excluded = [net("0.0.0.0/1")];
        let set = RouteSet::compute(&RouteInputs {
            excluded: &excluded,
            ..inputs(true)
        });
        assert_eq!(set.v4.len(), 43);
        assert!(!set.contains(ip("1.1.1.1")));
        assert!(!set.contains(ip("32.0.0.1")));
        assert!(!set.contains(ip("64.0.0.1")));
        assert!(set.contains(ip("128.0.0.1")));
        assert!(set.contains(ip("255.255.255.254")));
    }

    #[test]
    fn included_wins_over_excluded() {
        let excluded = [net("10.0.0.0/8")];
        let included = [net("10.1.0.0/16")];
        let set = RouteSet::compute(&RouteInputs {
            excluded: &excluded,
            included: &included,
            ..inputs(true)
        });
        assert!(set.contains(ip("10.1.2.3")));
        assert!(!set.contains(ip("10.2.0.1")));
        assert!(!set.contains(ip("10.9.9.9")));
        assert!(set.v4.contains(&net4("10.1.0.0/16")));
    }

    #[test]
    fn rurges_space_is_never_stripped_and_pieces_inside_it_are_dropped() {
        let excluded = [net("198.0.0.0/8")];
        let included = [net("198.18.5.0/24")];
        let set = RouteSet::compute(&RouteInputs {
            excluded: &excluded,
            included: &included,
            ..inputs(true)
        });
        // The hole keeps only the part outside rurge's space (design 4.5): the
        // responder still goes in, the rest of 198.0.0.0/8 does not.
        assert!(set.contains(ip("198.18.0.2")));
        assert!(!set.contains(ip("198.1.0.1")));
        // A piece falling inside the space is no route of its own.
        assert!(!set.v4.contains(&net4("198.18.5.0/24")));
        assert!(
            set.v4.iter().all(|net| !SPACE_V4.contains(net)),
            "a route lies inside rurge's space"
        );

        // Where the platform installs the space itself, it is a route of its
        // own; here nothing touches it, so it stays exactly 198.18.0.0/15.
        let space = RouteSet::compute(&RouteInputs {
            excluded: &excluded,
            included: &included,
            explicit_space: true,
            ..inputs(true)
        });
        assert!(space.v4.contains(&SPACE_V4));
        assert!(space.contains(ip("198.18.0.2")));
        assert!(space.v6.iter().any(|net| net.contains(&SPACE_V6)));

        // Where nothing is excluded, aggregation folds it into a larger prefix
        // (192.0.0.0/3 covers 198.18.0.0/15); it is still in either way.
        let folded = RouteSet::compute(&RouteInputs {
            explicit_space: true,
            ..inputs(true)
        });
        assert!(!folded.v4.contains(&SPACE_V4));
        assert!(folded.v4.iter().any(|net| net.contains(&SPACE_V4)));
        assert!(folded.contains(ip("198.18.0.2")));
    }

    #[test]
    fn ipv6_off_uses_no_ipv6_item() {
        let excluded = [net("8.8.8.8/32"), net("2001:db8::/32")];
        let included = [net("10.1.0.0/16"), net("2001:db8:1::/48")];
        let external = [ip("1.2.3.4"), ip("2001:db8::1")];
        let set = RouteSet::compute(&RouteInputs {
            ipv6: false,
            excluded: &excluded,
            included: &included,
            external: &external,
            explicit_space: false,
        });
        assert!(set.v6.is_empty());
        // The IPv4 items still do what they do.
        assert!(!set.contains(ip("8.8.8.8")));
        assert!(set.contains(ip("8.8.8.7")));
        assert!(set.contains(ip("10.1.2.3")));
        assert!(!set.contains(ip("1.2.3.4")));
        // None of the IPv6 items is used.
        assert!(!set.contains(ip("2001:db8::1")));
        assert!(!set.contains(ip("2001:db8:1::1")));
        // rurge's space is in whether or not IPv6 is on: only the adapter can
        // take it there, so a route on the physical adapter changes nothing.
        assert!(set.contains(ip("fd00:6152::2")));
        assert!(set.contains(ip("198.18.0.2")));
    }

    #[test]
    fn external_addresses_are_holes() {
        let external = [ip("1.2.3.4"), ip("2001:db8::1")];
        let set = RouteSet::compute(&RouteInputs {
            external: &external,
            ..inputs(true)
        });
        assert!(!set.contains(ip("1.2.3.4")));
        assert!(set.contains(ip("1.2.3.5")));
        assert!(!set.contains(ip("2001:db8::1")));
        assert!(set.contains(ip("2001:db8::2")));
    }

    #[test]
    fn diff_adds_and_removes_only_what_changed() {
        let old = RouteSet::compute(&inputs(true));
        let excluded = [net("8.0.0.0/6")];
        let new = RouteSet::compute(&RouteInputs {
            excluded: &excluded,
            ..inputs(true)
        });
        let (added, removed) = RouteSet::diff(&old, &new);
        // A prefix is on one side or the other, never both, although the
        // address of the one added lies inside the one removed.
        assert_eq!(added.v4, [net4("12.0.0.0/6")]);
        assert_eq!(removed.v4, [net4("8.0.0.0/5")]);
        assert!(added.v6.is_empty());
        assert!(removed.v6.is_empty());
        assert!(!added.is_empty() && !removed.is_empty());
        assert_eq!(added.len(), added.v4.len() + added.v6.len());

        // The old set, less what is removed, plus what is added, is the new one.
        let mut rebuilt = old.v4.clone();
        rebuilt.retain(|net| !removed.v4.contains(net));
        rebuilt.extend(added.v4.iter().copied());
        rebuilt.sort();
        assert_eq!(rebuilt, new.v4);

        // Nothing changed, nothing to do.
        let (added, removed) = RouteSet::diff(&new, &new);
        assert!(added.is_empty() && removed.is_empty());
        assert_eq!(added, RouteSet::default());
    }

    #[test]
    fn nat64_is_in_and_the_rest_of_colon_colon_slash_8_is_out() {
        let set = RouteSet::compute(&inputs(true));
        assert!(set.contains(ip("64:ff9b::102:304")));
        assert!(set.v6.contains(&net6("64:ff9b::/96")));
        assert!(!set.contains(ip("::1")));
        assert!(!set.contains(ip("::ffff:1.2.3.4")));
    }
}
