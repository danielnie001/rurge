//! What the enhanced mode should be now (M4 design 4.6, 4.7): the routes into
//! the adapter and whether IPv6 is on, from the generation in use and the
//! network. A pure function: the manager's `follow` (M4a task 9) and the open
//! both start from one of these.

use rurge_config::general::{General, Ipv6Vif};
use rurge_platform::route::{RouteInputs, RouteSet};
use std::net::IpAddr;

/// What the platform does with the routes of the enhanced mode: this module
/// does not know what platform it is on, so the caller says. A struct rather
/// than two more `bool` arguments, so that the two cannot be passed the wrong
/// way round at a call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlatformRoutes {
    /// `RouteInputs::explicit_space`: macOS installs rurge's space itself.
    pub explicit_space: bool,
    /// `RouteInputs::class_e`: the platform routes `240.0.0.0/4` as ordinary
    /// unicast (Linux, macOS); Windows blocks it, so nothing of it goes in.
    pub class_e: bool,
}

/// What the enhanced mode should be now, from the generation in use and the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub routes: RouteSet,
    pub ipv6: bool,
    /// The network's primary interface (`NetworkFacts::primary`), for the egress table.
    pub primary: Option<String>,
}

/// What the enhanced mode should be for this generation: `ipv6-vif` decides
/// whether IPv6 is on (P18: `auto` follows the network), and the routes are
/// the ones `RouteSet::compute` makes of the profile's `tun-*` items, the
/// external policies' addresses and rurge's space.
///
/// `platform` is the caller's own `cfg!`s (P22): this function does not know
/// what platform it is on.
pub fn wanted(
    general: &General,
    external: &[IpAddr],
    ipv6_usable: bool,
    primary: Option<String>,
    platform: PlatformRoutes,
) -> Wanted {
    let ipv6 = match general.ipv6_vif {
        Ipv6Vif::Disabled => false,
        Ipv6Vif::Always => true,
        Ipv6Vif::Auto => ipv6_usable,
    };
    Wanted {
        routes: RouteSet::compute(&RouteInputs {
            ipv6,
            excluded: &general.tun_excluded_routes,
            included: &general.tun_included_routes,
            external,
            explicit_space: platform.explicit_space,
            class_e: platform.class_e,
        }),
        ipv6,
        primary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_platform::route::{ADAPTER_V4, SPACE_V4};

    /// What every platform but Windows does: the space is macOS's business
    /// here, and `240.0.0.0/4` is ordinary unicast.
    fn everywhere(explicit_space: bool) -> PlatformRoutes {
        PlatformRoutes {
            explicit_space,
            class_e: true,
        }
    }

    fn general(ipv6_vif: Ipv6Vif) -> General {
        General {
            ipv6_vif,
            ..General::default()
        }
    }

    /// Three values of `ipv6-vif` against a network that has IPv6 and one that
    /// has not (P18).
    #[test]
    fn wanted_follows_ipv6_vif() {
        for (usable, disabled, auto, always) in
            [(true, false, true, true), (false, false, false, true)]
        {
            for (vif, want) in [
                (Ipv6Vif::Disabled, disabled),
                (Ipv6Vif::Auto, auto),
                (Ipv6Vif::Always, always),
            ] {
                let wanted = wanted(
                    &general(vif),
                    &[],
                    usable,
                    Some("eth0".into()),
                    everywhere(false),
                );
                assert_eq!(wanted.ipv6, want, "{vif:?}, usable {usable}");
                assert_eq!(wanted.primary.as_deref(), Some("eth0"));
                // IPv6 on: the IPv6 base prefixes are there; off: none of them
                assert_eq!(!wanted.routes.v6.is_empty(), want, "{vif:?}");
                assert!(
                    wanted.routes.contains(IpAddr::V4(ADAPTER_V4)),
                    "rurge's own space is always in"
                );
                assert!(wanted.routes.v4.iter().all(|net| !SPACE_V4.contains(net)));
            }
        }
    }

    /// The profile's items and the external policies' addresses come through,
    /// and the space is a route of its own only where the caller says so.
    #[test]
    fn the_profile_and_the_external_addresses_decide_the_routes() {
        let general = General {
            ipv6_vif: Ipv6Vif::Disabled,
            // the whole range, so that rurge's space is the only piece of it
            // that can stay in, and a route of its own only where it is asked
            tun_excluded_routes: vec![
                "8.8.8.8/32".parse().unwrap(),
                "198.0.0.0/8".parse().unwrap(),
            ],
            tun_included_routes: vec!["10.1.0.0/16".parse().unwrap()],
            ..General::default()
        };
        let external = ["1.2.3.4".parse().unwrap()];
        let wanted = self::wanted(&general, &external, true, None, everywhere(true));
        assert!(!wanted.ipv6, "`disabled` is off whatever the network");
        assert!(wanted.primary.is_none());
        for address in ["8.8.8.8", "1.2.3.4", "198.1.0.1"] {
            assert!(
                !wanted.routes.contains(address.parse().unwrap()),
                "{address}"
            );
        }
        for address in ["10.1.2.3", "1.1.1.1"] {
            assert!(
                wanted.routes.contains(address.parse().unwrap()),
                "{address}"
            );
        }
        assert!(wanted.routes.v4.contains(&SPACE_V4), "explicit_space");
        // without it the space is no route of its own, but still only the
        // adapter can take it anywhere
        let plain = self::wanted(&general, &external, true, None, everywhere(false));
        assert!(!plain.routes.v4.contains(&SPACE_V4));
        assert!(plain.routes.contains(IpAddr::V4(ADAPTER_V4)));
    }

    /// `class_e` reaches the route set: where the platform does not route
    /// `240.0.0.0/4` (Windows), nothing of it is in, even when the profile
    /// asks for all of it (the phase 3 / M4d plan's execution record row 87).
    #[test]
    fn class_e_reaches_the_route_set() {
        let general = General {
            ipv6_vif: Ipv6Vif::Disabled,
            tun_included_routes: vec!["0.0.0.0/0".parse().unwrap()],
            ..General::default()
        };
        let unicast = self::wanted(
            &general,
            &[],
            true,
            None,
            PlatformRoutes {
                explicit_space: false,
                class_e: true,
            },
        );
        assert!(unicast.routes.contains("240.0.0.1".parse().unwrap()));
        let blocked = self::wanted(
            &general,
            &[],
            true,
            None,
            PlatformRoutes {
                explicit_space: false,
                class_e: false,
            },
        );
        for address in ["240.0.0.1", "255.0.0.1", "255.255.255.254"] {
            assert!(
                !blocked.routes.contains(address.parse().unwrap()),
                "{address}"
            );
        }
        assert!(blocked.routes.contains("1.1.1.1".parse().unwrap()));
    }
}
