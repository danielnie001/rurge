//! Windows: what `netsh` is asked to do to bring the adapter up, route into
//! it and point the system DNS at the responder (design 5.4), and how what
//! `netsh` answers is read back.
//!
//! A translation and a reader, and nothing more: the adapter is named by its
//! index (a leftover adapter with the same name is never mistaken for it),
//! each step that changes something is one `netsh -f` script or one program's
//! call, each readback is one `netsh` call, and no program is run here. Every
//! platform compiles and tests this file.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use ipnet::IpNet;

use super::set::RouteSet;
use super::steps::{OnFailure, SCRIPT, Step, script};
use super::{ADAPTER_V4, ADAPTER_V6, MTU, RESPONDER_V4, RESPONDER_V6, SPACE_V4, SPACE_V6};

/// The metric every interface and route gets (plan P2): lower than what
/// Windows gives a physical adapter on its own, and no `0`, whose meaning in
/// `netsh` is not documented.
const METRIC: u8 = 1;

/// Where the programs are: `<system root>\System32`, never looked up in
/// `PATH` (design 5.1).
pub struct WindowsTools {
    pub netsh: PathBuf,
    pub ipconfig: PathBuf,
}

impl WindowsTools {
    /// `netsh.exe` and `ipconfig.exe` in `<system root>\System32`.
    pub fn under(system_root: &Path) -> WindowsTools {
        let system32 = system_root.join("System32");
        WindowsTools {
            netsh: system32.join("netsh.exe"),
            ipconfig: system32.join("ipconfig.exe"),
        }
    }
}

/// Bring the adapter up with its address and MTU (design 5.4, 9.3 step 2):
/// forwarding and weak host send / receive are off, which is half of keeping
/// rurge's own traffic out of the adapter (design 7.4), and the interface
/// metric goes on a line of its own (plan P9).
pub fn configure(t: &WindowsTools, index: u32) -> Step {
    let lines = [
        format!(
            "interface ipv4 add address name={index} address={ADAPTER_V4} mask={} store=active",
            SPACE_V4.netmask()
        ),
        interface_line(index, "ipv4"),
        metric_line(index, "ipv4"),
        subinterface_line(index, "ipv4"),
    ];
    netsh_step(t, &lines, OnFailure::Fail)
}

/// The adapter's IPv6 address, MTU and settings: a script of its own, so that
/// IPv6 failing to come up leaves the rest of the open alone (M4-D17).
pub fn configure_ipv6(t: &WindowsTools, index: u32) -> Step {
    let lines = [
        format!(
            "interface ipv6 add address interface={index} address={ADAPTER_V6}/{} store=active",
            SPACE_V6.prefix_len()
        ),
        interface_line(index, "ipv6"),
        metric_line(index, "ipv6"),
        subinterface_line(index, "ipv6"),
    ];
    netsh_step(t, &lines, OnFailure::Ipv6Off)
}

/// The undo of [`configure_ipv6`].
pub fn unconfigure_ipv6(t: &WindowsTools, index: u32) -> Step {
    let lines = [format!(
        "interface ipv6 delete address interface={index} address={ADAPTER_V6} store=active"
    )];
    netsh_step(t, &lines, OnFailure::Ignore)
}

/// The routes into the adapter, one script for each family (design 5.4).
pub fn add_routes(t: &WindowsTools, index: u32, set: &RouteSet) -> Vec<Step> {
    let mut steps = Vec::with_capacity(2);
    if !set.v4.is_empty() {
        steps.push(netsh_step(
            t,
            &route_lines(&set.v4, index, "ipv4", true),
            OnFailure::Fail,
        ));
    }
    if !set.v6.is_empty() {
        steps.push(netsh_step(
            t,
            &route_lines(&set.v6, index, "ipv6", true),
            OnFailure::Ipv6Off,
        ));
    }
    steps
}

/// Every route gone, one script for each family; a route that is not there is
/// no error, and what goes wrong here goes on with the rest.
pub fn remove_routes(t: &WindowsTools, index: u32, set: &RouteSet) -> Vec<Step> {
    let mut steps = Vec::with_capacity(2);
    if !set.v4.is_empty() {
        steps.push(netsh_step(
            t,
            &route_lines(&set.v4, index, "ipv4", false),
            OnFailure::Ignore,
        ));
    }
    if !set.v6.is_empty() {
        steps.push(netsh_step(
            t,
            &route_lines(&set.v6, index, "ipv6", false),
            OnFailure::Ignore,
        ));
    }
    steps
}

/// Point the adapter's DNS at the responder, then drop what the resolver
/// cached (design 5.4). The setting lives in the adapter's own configuration,
/// so the adapter going away takes it with it, and it is no undo step.
///
/// Windows keeps the IPv4 and IPv6 server lists apart: `set dnsservers`
/// changes only its own family's. With IPv6 off, the IPv6 list is cleared in a
/// step of its own, so that an address the adapter no longer has is not left
/// being asked (the phase 3 / M4a plan's execution record row 96). That step
/// ignores a failure: there is nothing to clear the first time, and an adapter
/// that has no IPv6 binding may not take the line, with no IPv6 list to leave
/// behind either.
pub fn take_dns(t: &WindowsTools, index: u32, ipv6: bool) -> Vec<Step> {
    let ipv4 = format!(
        "interface ipv4 set dnsservers name={index} source=static address={RESPONDER_V4} register=none validate=no"
    );
    let mut steps = Vec::with_capacity(3);
    if ipv6 {
        steps.push(netsh_step(
            t,
            &[
                ipv4,
                format!(
                    "interface ipv6 set dnsservers name={index} source=static address={RESPONDER_V6} register=none validate=no"
                ),
            ],
            OnFailure::Fail,
        ));
    } else {
        steps.push(netsh_step(t, &[ipv4], OnFailure::Fail));
        steps.push(netsh_step(
            t,
            &[format!(
                "interface ipv6 set dnsservers name={index} source=static address=none register=none validate=no"
            )],
            OnFailure::Ignore,
        ));
    }
    steps.push(Step {
        program: t.ipconfig.clone(),
        args: vec!["/flushdns".to_owned()],
        script: None,
        on_failure: OnFailure::Ignore,
    });
    steps
}

/// `netsh interface ipv4|ipv6 show route`: every route of that family (design
/// 5.4, plan P10). `show route` takes no adapter, so the caller keeps the rows
/// whose index is the adapter's own.
pub fn show_routes(t: &WindowsTools, v6: bool) -> Step {
    readback(t, &["interface", family(v6), "show", "route"])
}

/// The addresses the adapter has, read back: `show addresses` for the family.
/// The IPv4 command takes the adapter as `name=`, the IPv6 one as `interface=`
/// (plan P20).
pub fn show_addresses(t: &WindowsTools, index: u32, v6: bool) -> Step {
    let adapter = format!("{}={index}", if v6 { "interface" } else { "name" });
    readback(t, &["interface", family(v6), "show", "addresses", &adapter])
}

/// The adapter's MTU, read back: `show subinterfaces` takes `interface=` for
/// either family (plan P11).
pub fn show_mtu(t: &WindowsTools, index: u32, v6: bool) -> Step {
    let adapter = format!("interface={index}");
    readback(
        t,
        &["interface", family(v6), "show", "subinterfaces", &adapter],
    )
}

/// The adapter's DNS servers, read back: `show dnsservers` takes `name=` for
/// either family (plan P20).
pub fn show_dns(t: &WindowsTools, index: u32, v6: bool) -> Step {
    let adapter = format!("name={index}");
    readback(
        t,
        &["interface", family(v6), "show", "dnsservers", &adapter],
    )
}

/// `netsh interface ipv4|ipv6 show route`: (prefix, interface index) of each
/// row. A row is any line with a token that is a prefix (`a.b.c.d/n`, `x::/n`)
/// followed by one that is a number; headers and words are never read, so the
/// system's language does not matter.
pub fn routes_in(text: &str) -> Vec<(IpNet, u32)> {
    let mut routes = Vec::new();
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let row = tokens.windows(2).find_map(|pair| {
            let net = pair[0].parse::<IpNet>().ok()?;
            let index = pair[1].parse::<u32>().ok()?;
            Some((net, index))
        });
        if let Some(route) = row {
            routes.push(route);
        }
    }
    routes
}

/// The addresses in the output of `show addresses` or `show dnsservers`:
/// every token that is an address (after trimming `,;()[]` and a `%zone`).
pub fn addresses_in(text: &str) -> Vec<IpAddr> {
    let mut addresses = Vec::new();
    for token in text.split_whitespace() {
        let token = token.trim_matches(|c: char| matches!(c, ',' | ';' | '(' | ')' | '[' | ']'));
        let host = token.split_once('%').map_or(token, |(host, _zone)| host);
        if let Ok(address) = host.parse::<IpAddr>() {
            addresses.push(address);
        }
    }
    addresses
}

/// `show subinterfaces interface=<i>`: the MTU, the first number of the first
/// line that starts with one.
pub fn mtu_in(text: &str) -> Option<u32> {
    text.lines()
        .map(str::trim_start)
        .find(|line| line.starts_with(|c: char| c.is_ascii_digit()))
        .and_then(|line| line.split_whitespace().next())
        .and_then(|first| first.parse().ok())
}

/// One `netsh` readback: the parameters as they are typed, one string each, no
/// script, and a failure is not an error — reading back decides the open
/// (plan P10).
fn readback(t: &WindowsTools, parts: &[&str]) -> Step {
    Step {
        program: t.netsh.clone(),
        args: parts.iter().map(|part| (*part).to_owned()).collect(),
        script: None,
        on_failure: OnFailure::Ignore,
    }
}

/// The family's name in `netsh`'s own words.
fn family(v6: bool) -> &'static str {
    if v6 { "ipv6" } else { "ipv4" }
}

/// `set interface` for one family.
fn interface_line(index: u32, family: &str) -> String {
    format!(
        "interface {family} set interface interface={index} forwarding=disabled weakhostsend=disabled weakhostreceive=disabled store=active"
    )
}

/// The interface metric, on a line of its own (plan P9): the current `netsh
/// interface` page lists no `metric` for `set interface`, and one parameter the
/// system does not take may fail the whole line — a line of its own loses the
/// metric and nothing else.
fn metric_line(index: u32, family: &str) -> String {
    format!("interface {family} set interface interface={index} metric={METRIC} store=active")
}

/// `set subinterface` for one family: the MTU.
fn subinterface_line(index: u32, family: &str) -> String {
    format!("interface {family} set subinterface interface={index} mtu={MTU} store=active")
}

/// One line for each prefix, `add route` or `delete route`. The prefix is
/// on-link: the gateway is not written (plan P3).
fn route_lines<T: std::fmt::Display>(
    nets: &[T],
    index: u32,
    family: &str,
    add: bool,
) -> Vec<String> {
    nets.iter()
        .map(|net| {
            if add {
                format!(
                    "interface {family} add route prefix={net} interface={index} metric={METRIC} store=active"
                )
            } else {
                format!(
                    "interface {family} delete route prefix={net} interface={index} store=active"
                )
            }
        })
        .collect()
}

/// One `netsh -f` run over `lines`: every parameter is `key=value`, and the
/// adapter is named by its index (plan P4).
fn netsh_step(t: &WindowsTools, lines: &[String], on_failure: OnFailure) -> Step {
    Step {
        program: t.netsh.clone(),
        args: vec!["-f".to_owned(), SCRIPT.to_owned()],
        script: Some(script(lines)),
        on_failure,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    fn tools() -> WindowsTools {
        WindowsTools {
            netsh: PathBuf::from(r"C:\Windows\System32\netsh.exe"),
            ipconfig: PathBuf::from(r"C:\Windows\System32\ipconfig.exe"),
        }
    }

    fn set(v4: &[&str], v6: &[&str]) -> RouteSet {
        RouteSet {
            v4: v4.iter().map(|net| net.parse().unwrap()).collect(),
            v6: v6.iter().map(|net| net.parse().unwrap()).collect(),
        }
    }

    fn netsh(script: &str, on_failure: OnFailure) -> Step {
        Step {
            program: PathBuf::from(r"C:\Windows\System32\netsh.exe"),
            args: vec!["-f".to_owned(), "{script}".to_owned()],
            script: Some(script.to_owned()),
            on_failure,
        }
    }

    fn ipconfig() -> Step {
        Step {
            program: PathBuf::from(r"C:\Windows\System32\ipconfig.exe"),
            args: vec!["/flushdns".to_owned()],
            script: None,
            on_failure: OnFailure::Ignore,
        }
    }

    fn netsh_readback(args: &[&str]) -> Step {
        Step {
            program: PathBuf::from(r"C:\Windows\System32\netsh.exe"),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            script: None,
            on_failure: OnFailure::Ignore,
        }
    }

    #[test]
    fn the_tools_are_the_ones_in_system32() {
        let root = Path::new(r"C:\Windows");
        let tools = WindowsTools::under(root);
        assert_eq!(tools.netsh, root.join("System32").join("netsh.exe"));
        assert_eq!(tools.ipconfig, root.join("System32").join("ipconfig.exe"));
    }

    #[test]
    fn the_adapter_comes_up_in_one_script() {
        assert_eq!(
            configure(&tools(), 12),
            netsh(
                concat!(
                    "interface ipv4 add address name=12 address=198.18.0.1 mask=255.254.0.0 store=active\n",
                    "interface ipv4 set interface interface=12 forwarding=disabled weakhostsend=disabled weakhostreceive=disabled store=active\n",
                    "interface ipv4 set interface interface=12 metric=1 store=active\n",
                    "interface ipv4 set subinterface interface=12 mtu=1500 store=active\n",
                ),
                OnFailure::Fail,
            )
        );
    }

    #[test]
    fn ipv6_is_a_script_of_its_own() {
        assert_eq!(
            configure_ipv6(&tools(), 12),
            netsh(
                concat!(
                    "interface ipv6 add address interface=12 address=fd00:6152::1/64 store=active\n",
                    "interface ipv6 set interface interface=12 forwarding=disabled weakhostsend=disabled weakhostreceive=disabled store=active\n",
                    "interface ipv6 set interface interface=12 metric=1 store=active\n",
                    "interface ipv6 set subinterface interface=12 mtu=1500 store=active\n",
                ),
                OnFailure::Ipv6Off,
            )
        );
        assert_eq!(
            unconfigure_ipv6(&tools(), 12),
            netsh(
                "interface ipv6 delete address interface=12 address=fd00:6152::1 store=active\n",
                OnFailure::Ignore,
            )
        );
    }

    #[test]
    fn each_family_is_one_script_of_routes() {
        assert_eq!(
            add_routes(
                &tools(),
                12,
                &set(&["1.0.0.0/8", "128.0.0.0/1"], &["100::/8"])
            ),
            [
                netsh(
                    concat!(
                        "interface ipv4 add route prefix=1.0.0.0/8 interface=12 metric=1 store=active\n",
                        "interface ipv4 add route prefix=128.0.0.0/1 interface=12 metric=1 store=active\n",
                    ),
                    OnFailure::Fail,
                ),
                netsh(
                    "interface ipv6 add route prefix=100::/8 interface=12 metric=1 store=active\n",
                    OnFailure::Ipv6Off,
                ),
            ]
        );
    }

    #[test]
    fn a_family_with_no_routes_is_no_script() {
        assert_eq!(
            add_routes(&tools(), 12, &set(&["1.0.0.0/8"], &[])),
            [netsh(
                "interface ipv4 add route prefix=1.0.0.0/8 interface=12 metric=1 store=active\n",
                OnFailure::Fail,
            )]
        );
        assert_eq!(
            add_routes(&tools(), 12, &set(&[], &["100::/8"])),
            [netsh(
                "interface ipv6 add route prefix=100::/8 interface=12 metric=1 store=active\n",
                OnFailure::Ipv6Off,
            )]
        );
    }

    #[test]
    fn an_empty_set_is_no_step() {
        assert!(add_routes(&tools(), 12, &RouteSet::default()).is_empty());
        assert!(remove_routes(&tools(), 12, &RouteSet::default()).is_empty());
    }

    #[test]
    fn removing_routes_goes_on_with_what_remains() {
        assert_eq!(
            remove_routes(&tools(), 12, &set(&["1.0.0.0/8"], &["100::/8"])),
            [
                netsh(
                    "interface ipv4 delete route prefix=1.0.0.0/8 interface=12 store=active\n",
                    OnFailure::Ignore,
                ),
                netsh(
                    "interface ipv6 delete route prefix=100::/8 interface=12 store=active\n",
                    OnFailure::Ignore,
                ),
            ]
        );
    }

    #[test]
    fn the_system_dns_is_taken_over_then_the_caches_are_flushed() {
        assert_eq!(
            take_dns(&tools(), 12, true),
            [
                netsh(
                    concat!(
                        "interface ipv4 set dnsservers name=12 source=static address=198.18.0.2 register=none validate=no\n",
                        "interface ipv6 set dnsservers name=12 source=static address=fd00:6152::2 register=none validate=no\n",
                    ),
                    OnFailure::Fail,
                ),
                ipconfig(),
            ]
        );
        // IPv6 off: the one IPv4 server, then the IPv6 list cleared in a step
        // of its own, then the flush.
        assert_eq!(
            take_dns(&tools(), 12, false),
            [
                netsh(
                    "interface ipv4 set dnsservers name=12 source=static address=198.18.0.2 register=none validate=no\n",
                    OnFailure::Fail,
                ),
                netsh(
                    "interface ipv6 set dnsservers name=12 source=static address=none register=none validate=no\n",
                    OnFailure::Ignore,
                ),
                ipconfig(),
            ]
        );
    }

    /// Turning IPv6 off clears the IPv6 DNS list, which Windows keeps apart
    /// from the IPv4 one, after the IPv4 list is set and before the caches are
    /// flushed (the phase 3 / M4a plan's execution record row 96).
    #[test]
    fn turning_ipv6_off_clears_the_ipv6_dns_list() {
        let steps = take_dns(&tools(), 12, false);
        assert_eq!(steps.len(), 3, "{steps:?}");
        assert_eq!(steps[0].on_failure, OnFailure::Fail);
        assert_eq!(steps[1].on_failure, OnFailure::Ignore);
        assert_eq!(
            steps[1].script.as_deref(),
            Some(
                "interface ipv6 set dnsservers name=12 source=static address=none register=none validate=no\n"
            )
        );
        assert_eq!(steps[2].args, ["/flushdns".to_owned()]);
        assert!(
            !steps[0]
                .script
                .as_deref()
                .unwrap_or_default()
                .contains("interface ipv6"),
            "the IPv4 list alone is set in the first step"
        );
    }

    #[test]
    fn scripts_are_lines_ending_in_a_newline_and_no_carriage_return() {
        let mut steps = vec![
            configure(&tools(), 12),
            configure_ipv6(&tools(), 12),
            unconfigure_ipv6(&tools(), 12),
        ];
        steps.extend(add_routes(&tools(), 12, &set(&["1.0.0.0/8"], &["100::/8"])));
        steps.extend(remove_routes(
            &tools(),
            12,
            &set(&["1.0.0.0/8"], &["100::/8"]),
        ));
        steps.extend(take_dns(&tools(), 12, true));
        let mut scripts = 0;
        for step in &steps {
            let Some(script) = &step.script else { continue };
            scripts += 1;
            assert!(!script.contains('\r'), "{script:?}");
            assert!(script.ends_with('\n'), "{script:?}");
            assert!(script.lines().all(|line| !line.is_empty()), "{script:?}");
        }
        assert_eq!(scripts, 8);
    }

    /// The English layout of `netsh interface ipv4 show route` (plan P20): the
    /// columns are `Publish Type Met Prefix Idx Gateway/Interface Name`. The
    /// rows are the default route, the loopback adapter whose name ends in a
    /// number, a long prefix next to a name that has a number of its own, and
    /// one whose prefix and index are a single space apart. Hand-written after
    /// the layout in plan P20, not taken from a machine.
    const ROUTES_EN: &str = "\
Publish  Type      Met  Prefix                     Idx  Gateway/Interface Name
-------  --------  ---  -------------------------  ---  ------------------------
no       Manual    0    0.0.0.0/0                   12  192.168.1.1
no       Manual    0    127.0.0.0/8                  1  Loopback Pseudo-Interface 1
no       Manual    0    192.168.255.192/26          12  Ethernet 2
no       Manual    0    198.18.0.0/15 12 rurge
";

    /// The same table with the headers and the words in Chinese. Hand-written,
    /// not taken from a machine (plan P17).
    const ROUTES_ZH: &str = "\
发布     类型      度量  前缀                     序号  网关/接口名称
-------  --------  ---  -------------------------  ---  ------------------------
否       手动      0    0.0.0.0/0                   12  192.168.1.1
否       手动      0    127.0.0.0/8                  1  Loopback Pseudo-Interface 1
否       手动      0    192.168.255.192/26          12  Ethernet 2
否       手动      0    198.18.0.0/15 12 rurge
";

    /// `netsh interface ipv4 show addresses`. Hand-written after the layout,
    /// not taken from a machine.
    const ADDRESSES_EN: &str = "\
Configuration for interface \"rurge\"
    DHCP enabled:                         No
    IP Address:                           198.18.0.1
    Subnet Prefix:                        198.18.0.0/15 (mask 255.254.0.0)
    Default Gateway:                      none
";

    /// `netsh interface ipv4 show dnsservers`. Hand-written, not taken from a
    /// machine.
    const DNS_EN: &str = "\
Configuration for interface \"rurge\"
    Statically Configured DNS Servers:    198.18.0.2
    Register with which suffix:           Primary only
";

    /// The two outputs above with the words in Chinese. Hand-written, not taken
    /// from a machine (plan P17).
    const ADDRESSES_ZH: &str = "\
接口 \"rurge\" 的配置
    DHCP 已启用:                          否
    IP 地址:                              198.18.0.1
    子网前缀:                             198.18.0.0/15 (掩码 255.254.0.0)
    默认网关:                             无
";

    /// [`ADDRESSES_ZH`]'s DNS half. Hand-written, not taken from a machine
    /// (plan P17).
    const DNS_ZH: &str = "\
接口 \"rurge\" 的配置
    静态配置的 DNS 服务器:                198.18.0.2
    注册后缀:                             仅主后缀
";

    /// `netsh interface ipv4 show subinterfaces interface=12`: the MTU is the
    /// first column of the data row. Hand-written after the layout, not taken
    /// from a machine.
    const MTU_EN: &str = "\
                             MTU  MediaSenseState   Bytes In  Bytes Out  Interface
-------------------------  ---------------  ---------  ---------  -------------
                      1500                1          0          0  rurge
";

    /// [`MTU_EN`] with the words in Chinese. Hand-written, not taken from a
    /// machine (plan P17).
    const MTU_ZH: &str = "\
                             MTU  媒体感知状态  接收字节  发送字节  接口
-------------------------  ---------------  ---------  ---------  -------------
                      1500                1          0          0  rurge
";

    #[test]
    fn routes_are_read_from_the_english_table() {
        let expected: [(&str, u32); 4] = [
            ("0.0.0.0/0", 12),
            ("127.0.0.0/8", 1),
            ("192.168.255.192/26", 12),
            ("198.18.0.0/15", 12),
        ];
        let expected: Vec<(IpNet, u32)> = expected
            .iter()
            .map(|(net, index)| (net.parse().unwrap(), *index))
            .collect();
        assert_eq!(routes_in(ROUTES_EN), expected);
    }

    /// The language, and a table read as if it were GBK and decoded as UTF-8
    /// (every character that is not ASCII becomes U+FFFD), change nothing: the
    /// parser reads prefixes and numbers, never words.
    #[test]
    fn routes_are_read_whatever_the_language() {
        let english = routes_in(ROUTES_EN);
        assert!(!english.is_empty());
        assert_eq!(routes_in(ROUTES_ZH), english);
        let broken: String = ROUTES_ZH
            .chars()
            .map(|c| if c.is_ascii() { c } else { '\u{FFFD}' })
            .collect();
        assert_eq!(routes_in(&broken), english);
    }

    #[test]
    fn a_prefix_with_an_embedded_ipv4_address_is_read() {
        let text = "\
no       Manual    0    fe80::5efe:10.0.0.3/128     12  rurge
";
        assert_eq!(
            routes_in(text),
            [("fe80::5efe:10.0.0.3/128".parse::<IpNet>().unwrap(), 12)]
        );
    }

    #[test]
    fn lines_without_a_prefix_and_a_number_are_skipped() {
        let text = "\
Publish  Type      Met  Prefix                     Idx  Gateway/Interface Name
-------  --------  ---  -------------------------  ---  ------------------------
no       Manual    0    127.0.0.1                         Loopback
Ok.

";
        assert!(routes_in(text).is_empty(), "{:?}", routes_in(text));
    }

    /// The mask of the subnet prefix is an address token too and is read as
    /// one: the readback only asks whether a particular address is there, so
    /// nothing depends on leaving it out.
    #[test]
    fn addresses_are_read_from_the_english_output() {
        assert_eq!(
            addresses_in(ADDRESSES_EN),
            [
                "198.18.0.1".parse::<IpAddr>().unwrap(),
                "255.254.0.0".parse().unwrap(),
            ]
        );
        assert_eq!(
            addresses_in(DNS_EN),
            ["198.18.0.2".parse::<IpAddr>().unwrap()]
        );
    }

    #[test]
    fn addresses_are_read_whatever_the_language() {
        assert!(!addresses_in(ADDRESSES_EN).is_empty());
        assert_eq!(addresses_in(ADDRESSES_ZH), addresses_in(ADDRESSES_EN));
        assert_eq!(addresses_in(DNS_ZH), addresses_in(DNS_EN));
    }

    /// A zone (and the punctuation a token may wear) is not part of the
    /// address.
    #[test]
    fn an_address_with_a_zone_is_read_without_it() {
        assert_eq!(
            addresses_in("    DNS servers: [fe80::1%12], 198.18.0.2;\n"),
            [
                "fe80::1".parse::<IpAddr>().unwrap(),
                "198.18.0.2".parse().unwrap(),
            ]
        );
    }

    #[test]
    fn the_mtu_is_the_first_number_of_the_first_row() {
        assert_eq!(mtu_in(MTU_EN), Some(1500));
        assert_eq!(mtu_in(MTU_ZH), Some(1500));
        assert_eq!(mtu_in(""), None);
        assert_eq!(mtu_in("       MTU  媒体感知状态\n-------  -----\n"), None);
    }

    /// One `netsh` call each, no script, and a failure is not an error here:
    /// reading back decides the open (plan P10).
    #[test]
    fn the_readback_commands_are_one_netsh_call_each() {
        assert_eq!(
            show_routes(&tools(), false),
            netsh_readback(&["interface", "ipv4", "show", "route"])
        );
        assert_eq!(
            show_routes(&tools(), true),
            netsh_readback(&["interface", "ipv6", "show", "route"])
        );
        assert_eq!(
            show_addresses(&tools(), 12, false),
            netsh_readback(&["interface", "ipv4", "show", "addresses", "name=12"])
        );
        assert_eq!(
            show_addresses(&tools(), 12, true),
            netsh_readback(&["interface", "ipv6", "show", "addresses", "interface=12"])
        );
        assert_eq!(
            show_mtu(&tools(), 12, false),
            netsh_readback(&["interface", "ipv4", "show", "subinterfaces", "interface=12"])
        );
        assert_eq!(
            show_mtu(&tools(), 12, true),
            netsh_readback(&["interface", "ipv6", "show", "subinterfaces", "interface=12"])
        );
        assert_eq!(
            show_dns(&tools(), 12, false),
            netsh_readback(&["interface", "ipv4", "show", "dnsservers", "name=12"])
        );
        assert_eq!(
            show_dns(&tools(), 12, true),
            netsh_readback(&["interface", "ipv6", "show", "dnsservers", "name=12"])
        );
    }
}
