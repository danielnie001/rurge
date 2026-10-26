//! macOS: what it takes to bring the utun up, route into it and point the
//! system DNS at the responder (design 5.3).
//!
//! A translation, and nothing more: the programs are at fixed absolute paths
//! (design 5.1), the network service's name is given by the caller, and no
//! program is run. Every platform compiles and tests this file.

use std::net::IpAddr;
use std::path::PathBuf;

use super::set::RouteSet;
use super::steps::{OnFailure, Step};
use super::{ADAPTER_V4, ADAPTER_V6, MTU, SPACE_V4, SPACE_V6};

/// The programs at the paths design 5.1 gives them: never looked up in `PATH`.
const IFCONFIG: &str = "/sbin/ifconfig";
const ROUTE: &str = "/sbin/route";
const NETWORKSETUP: &str = "/usr/sbin/networksetup";
const DSCACHEUTIL: &str = "/usr/bin/dscacheutil";
const KILLALL: &str = "/usr/bin/killall";

/// What `networksetup` is given when there is no server to write: its word for
/// an empty list.
const EMPTY: &str = "Empty";

/// Bring the utun up with its address and MTU (design 5.3, 9.3 step 2).
pub fn configure(dev: &str) -> Step {
    step(
        IFCONFIG,
        vec![
            dev.to_owned(),
            "inet".to_owned(),
            ADAPTER_V4.to_string(),
            ADAPTER_V4.to_string(),
            "netmask".to_owned(),
            SPACE_V4.netmask().to_string(),
            "mtu".to_owned(),
            MTU.to_string(),
            "up".to_owned(),
        ],
        OnFailure::Fail,
    )
}

/// The utun's IPv6 address: a step of its own, so that IPv6 failing to come up
/// leaves the rest of the open alone (M4-D17).
pub fn configure_ipv6(dev: &str) -> Step {
    step(
        IFCONFIG,
        vec![
            dev.to_owned(),
            "inet6".to_owned(),
            ADAPTER_V6.to_string(),
            "prefixlen".to_owned(),
            SPACE_V6.prefix_len().to_string(),
        ],
        OnFailure::Ipv6Off,
    )
}

/// The undo of [`configure_ipv6`].
pub fn unconfigure_ipv6(dev: &str) -> Step {
    step(
        IFCONFIG,
        vec![
            dev.to_owned(),
            "inet6".to_owned(),
            ADAPTER_V6.to_string(),
            "delete".to_owned(),
        ],
        OnFailure::Ignore,
    )
}

/// The routes into the utun, a process for each (design 5.3: there is no tool
/// that takes a batch).
pub fn add_routes(dev: &str, set: &RouteSet) -> Vec<Step> {
    let mut steps = Vec::with_capacity(set.v4.len() + set.v6.len());
    for net in &set.v4 {
        steps.push(route_step(
            dev,
            "add",
            &net.to_string(),
            false,
            OnFailure::Fail,
        ));
    }
    for net in &set.v6 {
        steps.push(route_step(
            dev,
            "add",
            &net.to_string(),
            true,
            OnFailure::Ipv6Off,
        ));
    }
    steps
}

/// Every route gone, one at a time; a route that is not there is no error.
pub fn remove_routes(dev: &str, set: &RouteSet) -> Vec<Step> {
    let mut steps = Vec::with_capacity(set.v4.len() + set.v6.len());
    for net in &set.v4 {
        steps.push(route_step(
            dev,
            "delete",
            &net.to_string(),
            false,
            OnFailure::Ignore,
        ));
    }
    for net in &set.v6 {
        steps.push(route_step(
            dev,
            "delete",
            &net.to_string(),
            true,
            OnFailure::Ignore,
        ));
    }
    steps
}

/// Point a network service's DNS at the responder, or — with no server — put
/// back what the service had before (design 5.3). The service's name may
/// contain spaces: it is one argument, and no shell reads it.
pub fn set_dns(service: &str, servers: &[IpAddr], on_failure: OnFailure) -> Step {
    let mut args = vec!["-setdnsservers".to_owned(), service.to_owned()];
    if servers.is_empty() {
        args.push(EMPTY.to_owned());
    } else {
        args.extend(servers.iter().map(|server| server.to_string()));
    }
    step(NETWORKSETUP, args, on_failure)
}

/// Drop the caches that would otherwise answer with what the system DNS said
/// a moment ago (design 5.3).
pub fn flush_dns() -> Vec<Step> {
    vec![
        step(
            DSCACHEUTIL,
            vec!["-flushcache".to_owned()],
            OnFailure::Ignore,
        ),
        step(
            KILLALL,
            vec!["-HUP".to_owned(), "mDNSResponder".to_owned()],
            OnFailure::Ignore,
        ),
    ]
}

/// One program to run, with no script.
fn step(program: &str, args: Vec<String>, on_failure: OnFailure) -> Step {
    Step {
        program: PathBuf::from(program),
        args,
        script: None,
        on_failure,
    }
}

/// One `route -n add|delete` for a prefix, through the utun.
fn route_step(dev: &str, action: &str, prefix: &str, ipv6: bool, on_failure: OnFailure) -> Step {
    let mut args = vec!["-n".to_owned(), action.to_owned()];
    if ipv6 {
        args.push("-inet6".to_owned());
    }
    args.extend([
        "-net".to_owned(),
        prefix.to_owned(),
        "-interface".to_owned(),
        dev.to_owned(),
    ]);
    step(ROUTE, args, on_failure)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::path::PathBuf;

    use super::*;

    fn set(v4: &[&str], v6: &[&str]) -> RouteSet {
        RouteSet {
            v4: v4.iter().map(|net| net.parse().unwrap()).collect(),
            v6: v6.iter().map(|net| net.parse().unwrap()).collect(),
        }
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn plain(program: &str, args: &[&str], on_failure: OnFailure) -> Step {
        Step {
            program: PathBuf::from(program),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            script: None,
            on_failure,
        }
    }

    #[test]
    fn the_adapter_comes_up_with_its_address_and_mtu() {
        assert_eq!(
            configure("utun7"),
            plain(
                "/sbin/ifconfig",
                &[
                    "utun7",
                    "inet",
                    "198.18.0.1",
                    "198.18.0.1",
                    "netmask",
                    "255.254.0.0",
                    "mtu",
                    "1500",
                    "up",
                ],
                OnFailure::Fail,
            )
        );
    }

    #[test]
    fn ipv6_comes_and_goes_on_its_own() {
        assert_eq!(
            configure_ipv6("utun7"),
            plain(
                "/sbin/ifconfig",
                &["utun7", "inet6", "fd00:6152::1", "prefixlen", "64"],
                OnFailure::Ipv6Off,
            )
        );
        assert_eq!(
            unconfigure_ipv6("utun7"),
            plain(
                "/sbin/ifconfig",
                &["utun7", "inet6", "fd00:6152::1", "delete"],
                OnFailure::Ignore,
            )
        );
    }

    #[test]
    fn a_route_is_a_step_of_its_own() {
        assert_eq!(
            add_routes(
                "utun7",
                &set(&["1.0.0.0/8", "128.0.0.0/1"], &["100::/8", "64:ff9b::/96"])
            ),
            [
                plain(
                    "/sbin/route",
                    &["-n", "add", "-net", "1.0.0.0/8", "-interface", "utun7"],
                    OnFailure::Fail,
                ),
                plain(
                    "/sbin/route",
                    &["-n", "add", "-net", "128.0.0.0/1", "-interface", "utun7",],
                    OnFailure::Fail,
                ),
                plain(
                    "/sbin/route",
                    &[
                        "-n",
                        "add",
                        "-inet6",
                        "-net",
                        "100::/8",
                        "-interface",
                        "utun7",
                    ],
                    OnFailure::Ipv6Off,
                ),
                plain(
                    "/sbin/route",
                    &[
                        "-n",
                        "add",
                        "-inet6",
                        "-net",
                        "64:ff9b::/96",
                        "-interface",
                        "utun7",
                    ],
                    OnFailure::Ipv6Off,
                ),
            ]
        );
    }

    #[test]
    fn a_family_with_no_routes_is_no_step() {
        assert_eq!(
            add_routes("utun7", &set(&["1.0.0.0/8"], &[])),
            [plain(
                "/sbin/route",
                &["-n", "add", "-net", "1.0.0.0/8", "-interface", "utun7"],
                OnFailure::Fail,
            )]
        );
        assert_eq!(
            add_routes("utun7", &set(&[], &["100::/8"])),
            [plain(
                "/sbin/route",
                &[
                    "-n",
                    "add",
                    "-inet6",
                    "-net",
                    "100::/8",
                    "-interface",
                    "utun7",
                ],
                OnFailure::Ipv6Off,
            )]
        );
    }

    #[test]
    fn an_empty_set_is_no_step() {
        assert!(add_routes("utun7", &RouteSet::default()).is_empty());
        assert!(remove_routes("utun7", &RouteSet::default()).is_empty());
    }

    #[test]
    fn removing_routes_goes_on_with_what_remains() {
        assert_eq!(
            remove_routes("utun7", &set(&["1.0.0.0/8"], &["100::/8"])),
            [
                plain(
                    "/sbin/route",
                    &["-n", "delete", "-net", "1.0.0.0/8", "-interface", "utun7",],
                    OnFailure::Ignore,
                ),
                plain(
                    "/sbin/route",
                    &[
                        "-n",
                        "delete",
                        "-inet6",
                        "-net",
                        "100::/8",
                        "-interface",
                        "utun7",
                    ],
                    OnFailure::Ignore,
                ),
            ]
        );
    }

    #[test]
    fn the_system_dns_points_at_the_responder() {
        assert_eq!(
            set_dns("Wi-Fi", &[ip("198.18.0.2")], OnFailure::Fail),
            plain(
                "/usr/sbin/networksetup",
                &["-setdnsservers", "Wi-Fi", "198.18.0.2"],
                OnFailure::Fail,
            )
        );
        assert_eq!(
            set_dns(
                "Wi-Fi",
                &[ip("198.18.0.2"), ip("fd00:6152::2")],
                OnFailure::Ipv6Off
            ),
            plain(
                "/usr/sbin/networksetup",
                &["-setdnsservers", "Wi-Fi", "198.18.0.2", "fd00:6152::2"],
                OnFailure::Ipv6Off,
            )
        );
    }

    #[test]
    fn a_service_name_with_a_space_stays_one_argument() {
        let step = set_dns("USB 10/100/1000 LAN", &[ip("198.18.0.2")], OnFailure::Fail);
        assert_eq!(
            step.args,
            ["-setdnsservers", "USB 10/100/1000 LAN", "198.18.0.2"]
        );
    }

    #[test]
    fn an_empty_list_of_servers_is_empty() {
        assert_eq!(
            set_dns("Wi-Fi", &[], OnFailure::Ignore),
            plain(
                "/usr/sbin/networksetup",
                &["-setdnsservers", "Wi-Fi", "Empty"],
                OnFailure::Ignore,
            )
        );
    }

    #[test]
    fn the_dns_caches_are_flushed_in_two_steps() {
        assert_eq!(
            flush_dns(),
            [
                plain("/usr/bin/dscacheutil", &["-flushcache"], OnFailure::Ignore),
                plain(
                    "/usr/bin/killall",
                    &["-HUP", "mDNSResponder"],
                    OnFailure::Ignore,
                ),
            ]
        );
    }
}
