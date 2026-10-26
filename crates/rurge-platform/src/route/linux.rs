//! Linux: what it takes to bring the adapter up, route into it and point the
//! system resolver at the responder (design 5.2).
//!
//! A translation, and nothing more: the programs' paths are given (M4c looks
//! them up; this file never looks anything up in `PATH`), and no program is
//! run. Every platform compiles and tests this file.

use std::path::{Path, PathBuf};

use super::set::RouteSet;
use super::steps::{OnFailure, SCRIPT, Step, script};
use super::{ADAPTER_V4, ADAPTER_V6, MTU, RESPONDER_V4, RESPONDER_V6, SPACE_V4, SPACE_V6};

/// The programs the steps below run. Where they are is the caller's business
/// (M4c looks them up in `/usr/sbin`, `/sbin`, `/usr/bin`, `/bin`): a step
/// carries an absolute path and nothing is looked up in `PATH`.
pub struct LinuxTools {
    pub ip: PathBuf,
    /// `None` where the system has no `resolvectl` (its resolver is not
    /// systemd-resolved): the system DNS is not taken over, and the steps that
    /// would are empty (design 5.2).
    pub resolvectl: Option<PathBuf>,
}

/// Bring the adapter up with its address and MTU (design 5.2, 9.3 step 2).
pub fn configure(t: &LinuxTools, dev: &str) -> Step {
    let lines = [
        format!("link set dev {dev} mtu {MTU}"),
        format!("addr add {} dev {dev}", adapter_v4()),
        format!("link set dev {dev} up"),
    ];
    ip_step(t, false, &lines, OnFailure::Fail)
}

/// The adapter's IPv6 address: a batch of its own, so that IPv6 failing to
/// come up leaves the rest of the open alone (M4-D17).
pub fn configure_ipv6(t: &LinuxTools, dev: &str) -> Step {
    let lines = [format!("addr add {} dev {dev}", adapter_v6())];
    ip_step(t, false, &lines, OnFailure::Ipv6Off)
}

/// The undo of [`configure_ipv6`], as the device goes away (design 5.2).
pub fn unconfigure_ipv6(t: &LinuxTools, dev: &str) -> Step {
    let lines = [format!("addr del {} dev {dev}", adapter_v6())];
    ip_step(t, true, &lines, OnFailure::Ignore)
}

/// The routes into the adapter, one batch for each family (design 5.2):
/// `replace` rather than `add`, so that a prefix the kernel already has is no
/// error.
pub fn add_routes(t: &LinuxTools, dev: &str, set: &RouteSet) -> Vec<Step> {
    let mut steps = Vec::with_capacity(2);
    if !set.v4.is_empty() {
        steps.push(route_step(t, dev, &set.v4, OnFailure::Fail));
    }
    if !set.v6.is_empty() {
        steps.push(route_step(t, dev, &set.v6, OnFailure::Ipv6Off));
    }
    steps
}

/// Every route gone in one `ip -force -batch` (design 5.2): the device takes
/// its routes with it, so this is the undo of [`add_routes`] after a failure
/// partway, and what goes wrong here goes on with the rest.
pub fn remove_routes(t: &LinuxTools, dev: &str, set: &RouteSet) -> Vec<Step> {
    if set.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::with_capacity(set.v4.len() + set.v6.len());
    for net in &set.v4 {
        lines.push(format!("route del {net} dev {dev}"));
    }
    for net in &set.v6 {
        lines.push(format!("route del {net} dev {dev}"));
    }
    vec![ip_step(t, true, &lines, OnFailure::Ignore)]
}

/// Point the system resolver at the responder, for this link only (design
/// 5.2): nothing else is written, and the settings go with the device.
pub fn take_dns(t: &LinuxTools, dev: &str, ipv6: bool) -> Vec<Step> {
    let Some(resolvectl) = &t.resolvectl else {
        return Vec::new();
    };
    let mut servers = vec!["dns".to_owned(), dev.to_owned(), RESPONDER_V4.to_string()];
    if ipv6 {
        servers.push(RESPONDER_V6.to_string());
    }
    vec![
        resolvectl_step(resolvectl, servers, OnFailure::Fail),
        resolvectl_step(
            resolvectl,
            vec!["domain".to_owned(), dev.to_owned(), "~.".to_owned()],
            OnFailure::Fail,
        ),
        resolvectl_step(
            resolvectl,
            vec!["default-route".to_owned(), dev.to_owned(), "yes".to_owned()],
            OnFailure::Fail,
        ),
        resolvectl_step(resolvectl, vec!["flush-caches".to_owned()], OnFailure::Fail),
    ]
}

/// Give the system resolver back: the per-link settings go with the device,
/// so this fails harmlessly once the device is gone.
pub fn give_dns_back(t: &LinuxTools, dev: &str) -> Vec<Step> {
    let Some(resolvectl) = &t.resolvectl else {
        return Vec::new();
    };
    vec![resolvectl_step(
        resolvectl,
        vec!["revert".to_owned(), dev.to_owned()],
        OnFailure::Ignore,
    )]
}

/// The adapter's IPv4 address with its prefix length: `198.18.0.1/15`, which
/// is what brings the subnet route on Linux.
fn adapter_v4() -> String {
    format!("{ADAPTER_V4}/{}", SPACE_V4.prefix_len())
}

/// The adapter's IPv6 address with its prefix length: `fd00:6152::1/64`.
fn adapter_v6() -> String {
    format!("{ADAPTER_V6}/{}", SPACE_V6.prefix_len())
}

/// One `ip -batch` run over `lines`; `force` goes on after a command that
/// fails (`ip -force -batch`), which the undo wants.
fn ip_step(t: &LinuxTools, force: bool, lines: &[String], on_failure: OnFailure) -> Step {
    let mut args = Vec::with_capacity(3);
    if force {
        args.push("-force".to_owned());
    }
    args.push("-batch".to_owned());
    args.push(SCRIPT.to_owned());
    Step {
        program: t.ip.clone(),
        args,
        script: Some(script(lines)),
        on_failure,
    }
}

/// One `ip -batch` run adding `nets`, a `route replace` line each. `ip` reads
/// the address family of each prefix, so a batch of either family is the same
/// shape.
fn route_step<T: std::fmt::Display>(
    t: &LinuxTools,
    dev: &str,
    nets: &[T],
    on_failure: OnFailure,
) -> Step {
    let lines: Vec<String> = nets
        .iter()
        .map(|net| format!("route replace {net} dev {dev}"))
        .collect();
    ip_step(t, false, &lines, on_failure)
}

/// One `resolvectl` run.
fn resolvectl_step(program: &Path, args: Vec<String>, on_failure: OnFailure) -> Step {
    Step {
        program: program.to_path_buf(),
        args,
        script: None,
        on_failure,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The two programs at the paths M4c looks them up at.
    fn tools() -> LinuxTools {
        LinuxTools {
            ip: PathBuf::from("/usr/sbin/ip"),
            resolvectl: Some(PathBuf::from("/usr/sbin/resolvectl")),
        }
    }

    fn set(v4: &[&str], v6: &[&str]) -> RouteSet {
        RouteSet {
            v4: v4.iter().map(|net| net.parse().unwrap()).collect(),
            v6: v6.iter().map(|net| net.parse().unwrap()).collect(),
        }
    }

    fn scripted(program: &str, args: &[&str], script: &str, on_failure: OnFailure) -> Step {
        Step {
            program: PathBuf::from(program),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            script: Some(script.to_owned()),
            on_failure,
        }
    }

    fn plain(program: &str, args: &[&str], on_failure: OnFailure) -> Step {
        Step {
            script: None,
            ..scripted(program, args, "", on_failure)
        }
    }

    #[test]
    fn the_adapter_comes_up_in_one_batch() {
        assert_eq!(
            configure(&tools(), "rurge"),
            scripted(
                "/usr/sbin/ip",
                &["-batch", "{script}"],
                concat!(
                    "link set dev rurge mtu 1500\n",
                    "addr add 198.18.0.1/15 dev rurge\n",
                    "link set dev rurge up\n",
                ),
                OnFailure::Fail,
            )
        );
    }

    #[test]
    fn ipv6_is_a_batch_of_its_own_that_may_fail() {
        assert_eq!(
            configure_ipv6(&tools(), "rurge"),
            scripted(
                "/usr/sbin/ip",
                &["-batch", "{script}"],
                "addr add fd00:6152::1/64 dev rurge\n",
                OnFailure::Ipv6Off,
            )
        );
        assert_eq!(
            unconfigure_ipv6(&tools(), "rurge"),
            scripted(
                "/usr/sbin/ip",
                &["-force", "-batch", "{script}"],
                "addr del fd00:6152::1/64 dev rurge\n",
                OnFailure::Ignore,
            )
        );
    }

    #[test]
    fn each_family_is_one_batch_of_routes() {
        assert_eq!(
            add_routes(
                &tools(),
                "rurge",
                &set(&["1.0.0.0/8", "128.0.0.0/1"], &["100::/8"])
            ),
            [
                scripted(
                    "/usr/sbin/ip",
                    &["-batch", "{script}"],
                    concat!(
                        "route replace 1.0.0.0/8 dev rurge\n",
                        "route replace 128.0.0.0/1 dev rurge\n",
                    ),
                    OnFailure::Fail,
                ),
                scripted(
                    "/usr/sbin/ip",
                    &["-batch", "{script}"],
                    "route replace 100::/8 dev rurge\n",
                    OnFailure::Ipv6Off,
                ),
            ]
        );
    }

    #[test]
    fn a_family_with_no_routes_is_no_batch() {
        assert_eq!(
            add_routes(&tools(), "rurge", &set(&["1.0.0.0/8"], &[])),
            [scripted(
                "/usr/sbin/ip",
                &["-batch", "{script}"],
                "route replace 1.0.0.0/8 dev rurge\n",
                OnFailure::Fail,
            )]
        );
        assert_eq!(
            add_routes(&tools(), "rurge", &set(&[], &["100::/8"])),
            [scripted(
                "/usr/sbin/ip",
                &["-batch", "{script}"],
                "route replace 100::/8 dev rurge\n",
                OnFailure::Ipv6Off,
            )]
        );
    }

    #[test]
    fn an_empty_set_is_no_step() {
        assert!(add_routes(&tools(), "rurge", &RouteSet::default()).is_empty());
        assert!(remove_routes(&tools(), "rurge", &RouteSet::default()).is_empty());
    }

    #[test]
    fn removing_routes_is_one_batch_that_goes_on() {
        assert_eq!(
            remove_routes(&tools(), "rurge", &set(&["1.0.0.0/8"], &["100::/8"])),
            [scripted(
                "/usr/sbin/ip",
                &["-force", "-batch", "{script}"],
                concat!(
                    "route del 1.0.0.0/8 dev rurge\n",
                    "route del 100::/8 dev rurge\n",
                ),
                OnFailure::Ignore,
            )]
        );
    }

    #[test]
    fn the_system_dns_is_taken_over_in_four_steps() {
        assert_eq!(
            take_dns(&tools(), "rurge", true),
            [
                plain(
                    "/usr/sbin/resolvectl",
                    &["dns", "rurge", "198.18.0.2", "fd00:6152::2"],
                    OnFailure::Fail,
                ),
                plain(
                    "/usr/sbin/resolvectl",
                    &["domain", "rurge", "~."],
                    OnFailure::Fail,
                ),
                plain(
                    "/usr/sbin/resolvectl",
                    &["default-route", "rurge", "yes"],
                    OnFailure::Fail,
                ),
                plain("/usr/sbin/resolvectl", &["flush-caches"], OnFailure::Fail),
            ]
        );
        // IPv6 off: the one IPv4 server, and the three steps after it.
        let steps = take_dns(&tools(), "rurge", false);
        assert_eq!(
            steps[0],
            plain(
                "/usr/sbin/resolvectl",
                &["dns", "rurge", "198.18.0.2"],
                OnFailure::Fail,
            )
        );
        assert_eq!(steps.len(), 4);
    }

    #[test]
    fn without_resolvectl_the_system_dns_is_left_alone() {
        let tools = LinuxTools {
            ip: PathBuf::from("/usr/sbin/ip"),
            resolvectl: None,
        };
        assert!(take_dns(&tools, "rurge", true).is_empty());
        assert!(give_dns_back(&tools, "rurge").is_empty());
    }

    #[test]
    fn giving_dns_back_is_one_step_that_goes_on() {
        assert_eq!(
            give_dns_back(&tools(), "rurge"),
            [plain(
                "/usr/sbin/resolvectl",
                &["revert", "rurge"],
                OnFailure::Ignore,
            )]
        );
    }

    #[test]
    fn scripts_are_lines_ending_in_a_newline_and_no_carriage_return() {
        let mut steps = vec![
            configure(&tools(), "rurge"),
            configure_ipv6(&tools(), "rurge"),
            unconfigure_ipv6(&tools(), "rurge"),
        ];
        steps.extend(add_routes(
            &tools(),
            "rurge",
            &set(&["1.0.0.0/8"], &["100::/8"]),
        ));
        steps.extend(remove_routes(
            &tools(),
            "rurge",
            &set(&["1.0.0.0/8"], &["100::/8"]),
        ));
        let mut scripts = 0;
        for step in &steps {
            let Some(script) = &step.script else { continue };
            scripts += 1;
            assert!(!script.contains('\r'), "{script:?}");
            assert!(script.ends_with('\n'), "{script:?}");
            assert!(script.lines().all(|line| !line.is_empty()), "{script:?}");
        }
        assert_eq!(scripts, 6);
    }
}
