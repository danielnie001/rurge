//! macOS's half of the system side of the enhanced mode (M4 design 5.3, 6.1,
//! 6.3): the utun's addresses and routes through `ifconfig` and `route`, the
//! system DNS through `networksetup` for every enabled network service — read
//! before it is changed, and written back at the close — and the servers of the
//! network outside the enhanced mode.
//!
//! Nothing here is `cfg`-guarded (M4c plan P2): the programs go through the
//! `CommandRunner` this system is built with, and `/etc/resolv.conf` through a
//! path, so all of it is built and tested on Windows too. What is macOS's own —
//! the device — is [`crate::tun::TunDevice`]'s.

use std::fs;
use std::io;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::command::CommandRunner;
use crate::dns;
use crate::enhanced::{Adapter, Saved, System};
use crate::route::macos::{IPCONFIG, NETWORKSETUP};
use crate::route::steps::{OnFailure, Step};
use crate::route::{MTU, RESPONDER_V4, RESPONDER_V6, RouteSet, exec, macos as translation};
use crate::tun::{DeviceSpec, TunDevice};

/// What `networksetup` prints above the services of `-listallnetworkservices`,
/// and what `-listnetworkserviceorder` prints too: the line that explains the
/// asterisk, recognised by this much of its text rather than by its place.
const ASTERISK: &str = "denotes that a network service is disabled";

/// What `networksetup -getdnsservers` prints for a service that has none set.
const NONE_SET: &str = "There aren't any DNS Servers set on";

/// `ipconfig getpacket`'s line for the servers DHCP gave.
const DHCP_SERVERS: &str = "domain_name_server (ip_mult):";

/// `networksetup -listnetworkserviceorder`: each service with its device
/// (`(1) Wi-Fi` then `(Hardware Port: Wi-Fi, Device: en0)`; `(*)` for a
/// disabled one); a service without a device is left out.
pub fn service_devices(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut service: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = numbered_service(line) {
            service = Some(name);
            continue;
        }
        // the last `, Device: `, not the first: a hardware port may hold one
        let Some((_, rest)) = line.rsplit_once(", Device: ") else {
            continue;
        };
        let device = rest.split(')').next().unwrap_or_default().trim();
        let Some(name) = service.take() else { continue };
        if !device.is_empty() {
            pairs.push((name, device.to_string()));
        }
    }
    pairs
}

/// A line of `-listnetworkserviceorder` that starts a service: `(1) Wi-Fi`,
/// `(*) Bluetooth PAN`. What is in the brackets is a number or `*` and nothing
/// else: a hardware port whose name holds a `)` — `(Hardware Port: Foo (bar),
/// Device: en7)` — is not a service line, though its `(` and `)` would suggest
/// it.
fn numbered_service(line: &str) -> Option<String> {
    let (number, name) = line.strip_prefix('(')?.split_once(')')?;
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit() || c == '*') {
        return None;
    }
    let name = name.trim();
    match name.is_empty() {
        true => None,
        false => Some(name.to_string()),
    }
}

/// `networksetup -listallnetworkservices`: the services, without the line that
/// explains the asterisk; a disabled one (`*Name`) only when `disabled`, without
/// the `*`.
pub fn services(text: &str, disabled: bool) -> Vec<String> {
    let mut services = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.contains(ASTERISK) {
            continue;
        }
        match line.strip_prefix('*') {
            Some(_) if !disabled => continue,
            Some(name) => services.push(name.trim().to_string()),
            None => services.push(line.to_string()),
        }
    }
    services
}

/// `networksetup -getdnsservers <service>`: the servers set by hand; empty for
/// "There aren't any DNS Servers set on <service>."; `None` for anything else
/// (a line that is not an address — a scoped one too).
pub fn dns_servers(text: &str) -> Option<Vec<IpAddr>> {
    let mut servers = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with(NONE_SET) {
            return Some(Vec::new());
        }
        // a scoped address (`fe80::1%en0`) is not one the system DNS can be
        // given, and an error message is not a list
        servers.push(line.parse::<IpAddr>().ok()?);
    }
    Some(servers)
}

/// `ipconfig getpacket <device>`: `domain_name_server (ip_mult): {a, b}`; empty
/// when there is none (the tool prints nothing without DHCP).
pub fn dhcp_dns(text: &str) -> Vec<IpAddr> {
    for line in text.lines() {
        let Some((_, listed)) = line.split_once(DHCP_SERVERS) else {
            continue;
        };
        return listed
            .trim()
            .trim_matches(|c| c == '{' || c == '}')
            .split(',')
            .filter_map(|server| server.trim().parse::<IpAddr>().ok())
            .collect();
    }
    Vec::new()
}

/// What is between one `dns_saved` and the close: which services' DNS the
/// takeover changes, the servers `/etc/resolv.conf` held before it, and whether
/// this open has already said that no service could be taken.
#[derive(Default)]
struct MacState {
    /// The enabled services whose DNS `take_dns` points at the responder.
    to_take: Vec<String>,
    /// `/etc/resolv.conf`'s servers just before the takeover, less rurge's
    /// space (design 6.3): the physical network's servers until the first
    /// network change.
    before: Option<Vec<IpAddr>>,
    /// Whether this open has already warned that no service's DNS could be
    /// taken: one line an open, not one a reload.
    warned: bool,
}

/// macOS's system half (M4 design 5.3, 6.1, 6.3), over the programs `runner`
/// runs and `resolv_conf` — `/etc/resolv.conf` on a real system.
///
/// The lock on the state is held in sync code only: nothing here awaits.
pub struct MacSystem {
    runner: Arc<dyn CommandRunner>,
    resolv_conf: PathBuf,
    scratch: PathBuf,
    state: Mutex<MacState>,
}

impl MacSystem {
    pub fn new(
        runner: Arc<dyn CommandRunner>,
        resolv_conf: PathBuf,
        scratch: PathBuf,
    ) -> MacSystem {
        MacSystem {
            runner,
            resolv_conf,
            scratch,
            state: Mutex::new(MacState::default()),
        }
    }

    /// `networksetup` with `args`: its output, or what it failed with.
    fn networksetup(&self, args: &[&str]) -> io::Result<String> {
        self.command(NETWORKSETUP, args)
    }

    /// `ipconfig` with `args`.
    fn ipconfig(&self, args: &[&str]) -> io::Result<String> {
        self.command(IPCONFIG, args)
    }

    /// One program of this file with its arguments. The program and what it
    /// was given are DEBUG only: a service's name may be the user's own
    /// (design 13).
    fn command(&self, program: &str, args: &[&str]) -> io::Result<String> {
        tracing::debug!(
            program,
            args = %args.join(" "),
            "running a command of the enhanced mode"
        );
        let mut cmd = Vec::with_capacity(args.len() + 1);
        cmd.push(program.to_string());
        cmd.extend(args.iter().map(|arg| (*arg).to_string()));
        self.runner.run(&cmd)
    }

    /// Runs a translation's steps ([`exec::run`]). `Ok(false)`: the IPv6 half
    /// failed.
    fn run(&self, steps: &[Step]) -> Result<bool, String> {
        exec::run(steps, &*self.runner, &self.scratch)
    }

    /// One step run as it is, not through [`exec::run`]: the DNS commands are
    /// not steps whose failure is weighed, and what one failed with — and the
    /// service's name — is DEBUG only (M4c plan P7).
    fn run_step(&self, step: &Step) -> Result<(), String> {
        let mut cmd = Vec::with_capacity(step.args.len() + 1);
        cmd.push(step.program.to_string_lossy().into_owned());
        cmd.extend(step.args.iter().cloned());
        tracing::debug!(
            program = %step.program.display(),
            args = %step.args.join(" "),
            "running a step of the enhanced mode"
        );
        self.runner.run(&cmd).map(drop).map_err(|e| e.to_string())
    }

    /// The network services, enabled ones alone or all of them, each without
    /// its `*`. The one failure that may show what the tool printed: the
    /// command carries no service's name (M4c plan P7).
    fn list_services(&self, disabled: bool) -> Result<Vec<String>, String> {
        let text = self
            .networksetup(&["-listallnetworkservices"])
            .map_err(|e| format!("cannot list the network services: {e}"))?;
        Ok(services(&text, disabled))
    }

    /// Says that one service's DNS is left as it is (P8): one line, without its
    /// name — why is in the DEBUG line the caller writes.
    fn cannot_read_a_services_dns(&self) {
        tracing::warn!(
            "the DNS servers of a network service could not be read; \
             its DNS is left as it is"
        );
    }
}

/// A [`Saved`] as the network service it is about, when it is one of macOS's.
fn mac_dns(saved: &Saved) -> Option<(&String, &Vec<IpAddr>)> {
    match saved {
        Saved::MacDns { service, servers } => Some((service, servers)),
        Saved::RpFilter { .. } => None,
    }
}

impl System for MacSystem {
    /// The kernel picks the utun's name (`utunN`). What a run that died left in
    /// the scratch directory goes first (M4c plan P12, R4).
    fn open(&self) -> io::Result<TunDevice> {
        exec::remove_leftovers(&self.scratch);
        TunDevice::open(&DeviceSpec {
            name: "utun".to_string(),
            mtu: usize::from(MTU),
        })
    }

    fn configure(&self, dev: &Adapter, ipv6: bool) -> Result<bool, String> {
        let mut steps = vec![translation::configure(&dev.name)];
        if ipv6 {
            steps.push(translation::configure_ipv6(&dev.name));
        }
        self.run(&steps)
    }

    fn configure_ipv6(&self, dev: &Adapter) -> Result<bool, String> {
        self.run(&[translation::configure_ipv6(&dev.name)])
    }

    fn unconfigure_ipv6(&self, dev: &Adapter) -> Result<(), String> {
        self.run(&[translation::unconfigure_ipv6(&dev.name)])
            .map(drop)
    }

    fn routes(&self, dev: &Adapter, add: &RouteSet, remove: &RouteSet) -> Result<bool, String> {
        super::run_routes(
            add,
            remove,
            |steps| self.run(steps),
            |set| translation::add_routes(&dev.name, set),
            |set| translation::remove_routes(&dev.name, set),
        )
    }

    /// Every enabled service's servers, read before any of them is changed: a
    /// service whose servers cannot be read is left alone (P8), and one whose
    /// servers the backup holds already is not read again — after a crash they
    /// are the responder's, not the service's.
    fn dns_saved(&self, _dev: &Adapter, saved: &[Saved]) -> Result<Vec<Saved>, String> {
        let enabled = self.list_services(false)?;
        if !saved.iter().any(|saved| mac_dns(saved).is_some()) {
            // the first takeover of this open: what /etc/resolv.conf holds now
            // is the physical network's servers, less rurge's own space —
            // after a crash it may still name the responder (design 6.3, R33)
            let servers = match fs::read_to_string(&self.resolv_conf) {
                Ok(text) => {
                    let (servers, _) = dns::parse_resolv_conf(&text);
                    dns::without_rurge_space(servers)
                }
                Err(e) => {
                    tracing::debug!(error = %e, "cannot read /etc/resolv.conf");
                    Vec::new()
                }
            };
            self.state.lock().unwrap().before = Some(servers);
        }

        let already: Vec<&str> = saved
            .iter()
            .filter_map(mac_dns)
            .map(|(service, _)| service.as_str())
            .collect();
        let mut taken = Vec::new();
        for service in &enabled {
            if already.contains(&service.as_str()) {
                continue;
            }
            match self.networksetup(&["-getdnsservers", service]) {
                Ok(text) => match dns_servers(&text) {
                    Some(servers) => taken.push(Saved::MacDns {
                        service: service.clone(),
                        servers,
                    }),
                    None => {
                        tracing::debug!(
                            service = %service,
                            output = %text,
                            "the DNS servers of a network service are not a list of addresses"
                        );
                        self.cannot_read_a_services_dns();
                    }
                },
                Err(e) => {
                    tracing::debug!(
                        service = %service,
                        error = %e,
                        "cannot read the DNS servers of a network service"
                    );
                    self.cannot_read_a_services_dns();
                }
            }
        }

        // what this open changes: the enabled services the backup knows, and
        // the ones just read
        let mut to_take: Vec<String> = enabled
            .iter()
            .filter(|service| already.contains(&service.as_str()))
            .cloned()
            .collect();
        to_take.extend(
            taken
                .iter()
                .filter_map(mac_dns)
                .map(|(service, _)| service.clone()),
        );
        self.state.lock().unwrap().to_take = to_take;
        Ok(taken)
    }

    fn take_dns(&self, _dev: &Adapter, ipv6: bool) -> Result<bool, String> {
        let to_take = self.state.lock().unwrap().to_take.clone();
        if to_take.is_empty() {
            // one line an open, not one a reload (the manager asks more than
            // once while it is open)
            let mut state = self.state.lock().unwrap();
            if !state.warned {
                state.warned = true;
                tracing::warn!("no network service's DNS could be pointed at the enhanced mode");
            }
            return Ok(false);
        }
        let mut responders = vec![IpAddr::V4(RESPONDER_V4)];
        if ipv6 {
            responders.push(IpAddr::V6(RESPONDER_V6));
        }
        for service in &to_take {
            let step = translation::set_dns(service, &responders, OnFailure::Fail);
            if let Err(error) = self.run_step(&step) {
                tracing::debug!(
                    service = %service,
                    error = %error,
                    "cannot point a network service's DNS at the enhanced mode"
                );
                return Err("cannot point a network service's DNS at the enhanced mode \
                     (the debug log has the details)"
                    .to_string());
            }
        }
        self.run(&translation::flush_dns())?;
        Ok(true)
    }

    /// Nothing here is given back: what outlives rurge is put back one `Saved`
    /// at a time by [`MacSystem::undo`]. The next open warns again if it has to
    /// leave the DNS alone.
    fn give_dns_back(&self, _dev: &Adapter) -> Result<(), String> {
        self.state.lock().unwrap().warned = false;
        Ok(())
    }

    fn adjust_saved(
        &self,
        _primary: Option<&str>,
        _saved: &[Saved],
    ) -> Result<Option<Saved>, String> {
        // macOS has nothing to adjust: the DNS goes back through `undo`
        Ok(None)
    }

    fn adjust(&self) -> Result<(), String> {
        Ok(())
    }

    fn undo(&self, saved: &Saved) -> Result<(), String> {
        match saved {
            Saved::MacDns { service, servers } => {
                // every service, disabled ones too: a service the user deleted
                // has nowhere to be written back, and the undo is done (P9)
                let all = self.list_services(true)?;
                if !all.iter().any(|name| name == service) {
                    tracing::debug!(service = %service, "the network service is gone");
                    tracing::info!(
                        "a network service the enhanced mode changed is gone; \
                         there is nothing to put back for it"
                    );
                    return Ok(());
                }
                let step = translation::set_dns(service, servers, OnFailure::Fail);
                if let Err(error) = self.run_step(&step) {
                    tracing::debug!(
                        service = %service,
                        error = %error,
                        "cannot put back a network service's DNS servers"
                    );
                    return Err("cannot put back the DNS servers of a network service \
                         (the debug log has the details)"
                        .to_string());
                }
                self.run(&translation::flush_dns())?;
                Ok(())
            }
            Saved::RpFilter { .. } => {
                // another platform's backup (a data directory brought along):
                // nothing here can put it back, and left as it is it fails
                // every start (P9)
                tracing::warn!("an undo of another platform is dropped");
                Ok(())
            }
        }
    }

    /// The servers outside the enhanced mode (design 6.3, P10): before the first
    /// network change what `/etc/resolv.conf` held just before the takeover;
    /// after it, the servers the main interface's service had set by hand, or
    /// the ones DHCP gave it, or none at all. Never the responder: a service the
    /// user pointed at `198.18.0.2` by hand would send rurge's own lookups to an
    /// address this machine cannot reach (R16).
    fn physical_dns(
        &self,
        primary: Option<&str>,
        saved: &[Saved],
        network_changed: bool,
    ) -> Option<Vec<IpAddr>> {
        {
            let mut state = self.state.lock().unwrap();
            if network_changed {
                // it is only right until the network it was read on changes
                state.before = None;
            } else if let Some(before) = &state.before {
                return Some(before.clone());
            }
        }
        let Some(primary) = primary else {
            // no route to the network: nowhere to read a server from, and the
            // system's own list is the responder's (design 6.3)
            return Some(Vec::new());
        };

        // the service the main interface goes through; a command that fails
        // says nothing about the network (the reason is a DEBUG line)
        let service = match self.networksetup(&["-listnetworkserviceorder"]) {
            Ok(text) => service_devices(&text)
                .into_iter()
                .find(|(_, device)| device == primary)
                .map(|(service, _)| service),
            Err(e) => {
                tracing::debug!(error = %e, "cannot list the network services' devices");
                None
            }
        };
        if let Some(service) = &service
            && let Some(servers) = saved
                .iter()
                .filter_map(mac_dns)
                .find(|(name, _)| *name == service)
                .map(|(_, servers)| dns::without_rurge_space(servers.iter().copied()))
            && !servers.is_empty()
        {
            return Some(servers);
        }

        // no server set by hand — or none outside the enhanced mode: the ones
        // DHCP gave the interface
        match self.ipconfig(&["getpacket", primary]) {
            Ok(text) => Some(dns::without_rurge_space(dhcp_dns(&text))),
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "cannot read the DHCP packet of the main interface; \
                     no servers outside the enhanced mode are known"
                );
                Some(Vec::new())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::IpAddr;
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::command::testing::FakeRunner;
    use crate::log_capture::LogCapture;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    /// not taken from a Mac: the heading is the manual page's (ss64's
    /// transcription of `networksetup`, M4c plan P20), and the service names
    /// are the ones a Mac has, from memory.
    const SERVICES: &str = concat!(
        "An asterisk (*) denotes that a network service is disabled.\n",
        "Wi-Fi\n",
        "Ethernet\n",
        "*Bluetooth PAN\n",
        "USB 10/100/1000 LAN\n",
    );

    /// not taken from a Mac: the sentence is one a Mac printed (P20 quotes
    /// `There aren't any DNS Servers set on Ethernet.`); a list of servers is
    /// one name to a line, the shape `-setdnsservers` takes its arguments in.
    const TWO_SERVERS: &str = "192.168.1.1\n1.1.1.1\n";

    /// not taken from a Mac: there is no sample of this output to quote (P20):
    /// the shape is from memory, and M4d's manual list checks it.
    const SERVICE_ORDER: &str = concat!(
        "An asterisk (*) denotes that a network service is disabled.\n",
        "(1) Wi-Fi\n",
        "(Hardware Port: Wi-Fi, Device: en0)\n",
        "\n",
        "(2) USB 10/100/1000 LAN\n",
        "(Hardware Port: USB 10/100/1000 LAN, Device: en5)\n",
        "\n",
        "(*) Bluetooth PAN\n",
        "(Hardware Port: Bluetooth PAN, Device: en6)\n",
        "\n",
        "(3) USB 2.0 Hub (left)\n",
        "(Hardware Port: USB 2.0 Hub (left), Device: en7)\n",
        "\n",
        "(4) Thunderbolt Bridge\n",
        "(Hardware Port: Thunderbolt Bridge, Device: )\n",
    );

    /// not taken from a Mac: `domain_name_server (ip_mult): {…}` is what
    /// ipconfig(8) prints (apple-oss-distributions/bootp, P20) and what the ISC
    /// dhcp-users list showed for two servers; the lines around it are from
    /// memory.
    const PACKET: &str = concat!(
        "op = BOOTREPLY\n",
        "yiaddr = 192.168.4.100\n",
        "server_identifier (ip): 192.168.4.1\n",
        "lease_time (uint32): 0x1c20\n",
        "domain_name_server (ip_mult): {192.168.4.1}\n",
    );

    /// not taken from a Mac; the two-server form is the one P20 quotes from the
    /// ISC list.
    const PACKET_TWO: &str = concat!(
        "op = BOOTREPLY\n",
        "domain_name_server (ip_mult): {156.26.1.1, 156.26.1.30}\n",
    );

    /// The heading that explains the asterisk is not a service, and a disabled
    /// one is none of the enabled ones.
    #[test]
    fn enabled_services_skip_the_heading_and_the_disabled() {
        assert_eq!(
            services(SERVICES, false),
            ["Wi-Fi", "Ethernet", "USB 10/100/1000 LAN"]
        );
    }

    /// With the disabled ones too, a name keeps its spaces and loses the `*`.
    #[test]
    fn all_services_keep_the_disabled_without_the_asterisk() {
        assert_eq!(
            services(SERVICES, true),
            ["Wi-Fi", "Ethernet", "Bluetooth PAN", "USB 10/100/1000 LAN"]
        );
        // a name with spaces is one name
        assert!(
            services(SERVICES, false)
                .iter()
                .any(|name| name == "USB 10/100/1000 LAN")
        );
    }

    /// A list of addresses, the sentence that says there are none, and `None`
    /// for what is neither — a scoped address would be read as another address
    /// and is not one the system DNS can be given.
    #[test]
    fn dns_servers_read_a_list_and_the_none_set_sentence() {
        assert_eq!(
            dns_servers(TWO_SERVERS),
            Some(vec![ip("192.168.1.1"), ip("1.1.1.1")])
        );
        assert_eq!(
            dns_servers("There aren't any DNS Servers set on Wi-Fi.\n"),
            Some(Vec::new())
        );
        assert_eq!(dns_servers("fe80::1%en0\n"), None);
        assert_eq!(dns_servers("Error: The parameters were not valid.\n"), None);
    }

    /// Each service with the device it goes through, disabled ones too; the
    /// one whose device is empty is left out. A hardware port with a `)` in its
    /// name is still no service line, and its service is still paired.
    #[test]
    fn service_devices_pair_each_service_with_its_device() {
        let pairs = service_devices(SERVICE_ORDER);
        assert_eq!(
            pairs,
            [
                ("Wi-Fi".to_string(), "en0".to_string()),
                ("USB 10/100/1000 LAN".to_string(), "en5".to_string()),
                ("Bluetooth PAN".to_string(), "en6".to_string()),
                ("USB 2.0 Hub (left)".to_string(), "en7".to_string()),
            ]
        );
        assert!(!pairs.iter().any(|(name, _)| name == "Thunderbolt Bridge"));
    }

    /// The servers DHCP gave the interface, either form; no line, or no output
    /// at all, is no server.
    #[test]
    fn dhcp_dns_reads_domain_name_server() {
        assert_eq!(dhcp_dns(PACKET), [ip("192.168.4.1")]);
        assert_eq!(dhcp_dns(PACKET_TWO), [ip("156.26.1.1"), ip("156.26.1.30")]);
        assert_eq!(
            dhcp_dns("op = BOOTREPLY\nyiaddr = 192.168.4.100\n"),
            Vec::<IpAddr>::new()
        );
        assert_eq!(dhcp_dns(""), Vec::<IpAddr>::new());
    }

    /// not taken from a Mac: as above, with two enabled services — one with
    /// servers set by hand, one without any.
    const TWO_SERVICES: &str = concat!(
        "An asterisk (*) denotes that a network service is disabled.\n",
        "Wi-Fi\n",
        "Ethernet\n",
    );

    const LIST: &str = "/usr/sbin/networksetup -listallnetworkservices";
    const GET: &str = "/usr/sbin/networksetup -getdnsservers";
    const FLUSH: [&str; 2] = [
        "/usr/bin/dscacheutil -flushcache",
        "/usr/bin/killall -HUP mDNSResponder",
    ];

    /// What `networksetup -setdnsservers` is run as: `args` are the servers, and
    /// `empty` for a service that had none.
    fn set_line(service: &str, args: &[&str]) -> String {
        let mut line = format!("/usr/sbin/networksetup -setdnsservers {service}");
        for arg in args {
            line.push(' ');
            line.push_str(arg);
        }
        line
    }

    fn system_of(dir: &Path, runner: &Arc<FakeRunner>) -> MacSystem {
        MacSystem::new(
            runner.clone(),
            dir.join("resolv.conf"),
            dir.join("enhanced"),
        )
    }

    fn dev() -> Adapter {
        Adapter {
            name: "utun".to_string(),
            index: None,
        }
    }

    fn saved(service: &str, servers: &[&str]) -> Saved {
        Saved::MacDns {
            service: service.to_string(),
            servers: servers.iter().map(|server| ip(server)).collect(),
        }
    }

    /// The two enabled services' servers are read before either is changed, and
    /// the backup holds what each had.
    #[test]
    fn the_dns_of_each_enabled_service_is_read_before_it_is_set() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(LIST, TWO_SERVICES);
        runner.reply(&format!("{GET} Wi-Fi"), TWO_SERVERS);
        runner.reply(
            &format!("{GET} Ethernet"),
            "There aren't any DNS Servers set on Ethernet.\n",
        );
        let system = system_of(dir.path(), &runner);

        assert_eq!(
            system.dns_saved(&dev(), &[]),
            Ok(vec![
                saved("Wi-Fi", &["192.168.1.1", "1.1.1.1"]),
                saved("Ethernet", &[]),
            ])
        );
        assert_eq!(system.take_dns(&dev(), false), Ok(true));
        assert_eq!(
            runner.calls(),
            [
                LIST.to_string(),
                format!("{GET} Wi-Fi"),
                format!("{GET} Ethernet"),
                set_line("Wi-Fi", &["198.18.0.2"]),
                set_line("Ethernet", &["198.18.0.2"]),
                FLUSH[0].to_string(),
                FLUSH[1].to_string(),
            ]
        );

        // with IPv6 on, the responder's address of that family goes with the
        // others
        let runner = Arc::new(FakeRunner::default());
        runner.reply(LIST, TWO_SERVICES);
        let system = system_of(dir.path(), &runner);
        assert_eq!(
            system.dns_saved(&dev(), &[]),
            Ok(vec![saved("Wi-Fi", &[]), saved("Ethernet", &[])])
        );
        assert_eq!(system.take_dns(&dev(), true), Ok(true));
        assert!(
            runner
                .calls()
                .contains(&set_line("Wi-Fi", &["198.18.0.2", "fd00:6152::2"])),
            "{:?}",
            runner.calls()
        );
    }

    /// A service that is in the backup is not read again — after a crash its
    /// servers are the responder's, not the service's — but it is set again.
    #[test]
    fn services_saved_already_are_not_read_again_but_set_again() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(LIST, TWO_SERVICES);
        runner.reply(&format!("{GET} Ethernet"), "10.0.0.1\n");
        let system = system_of(dir.path(), &runner);

        let backup = [saved("Wi-Fi", &["1.1.1.1"])];
        assert_eq!(
            system.dns_saved(&dev(), &backup),
            Ok(vec![saved("Ethernet", &["10.0.0.1"])])
        );
        assert_eq!(system.take_dns(&dev(), false), Ok(true));
        assert_eq!(
            runner.calls(),
            [
                LIST.to_string(),
                format!("{GET} Ethernet"),
                set_line("Wi-Fi", &["198.18.0.2"]),
                set_line("Ethernet", &["198.18.0.2"]),
                FLUSH[0].to_string(),
                FLUSH[1].to_string(),
            ]
        );
    }

    /// A service whose servers cannot be read is left as it is: it cannot be
    /// put back, so it is not changed. One WARN says so, without its name.
    #[test]
    fn a_service_that_cannot_be_read_is_left_alone() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(LIST, TWO_SERVICES);
        runner.fail(&format!("{GET} Wi-Fi"), "networksetup: the service is gone");
        runner.reply(&format!("{GET} Ethernet"), "10.0.0.1\n");
        let system = system_of(dir.path(), &runner);

        assert_eq!(
            system.dns_saved(&dev(), &[]),
            Ok(vec![saved("Ethernet", &["10.0.0.1"])])
        );
        let lines = captured.lines();
        let warns: Vec<&String> = lines
            .iter()
            .filter(|line| line.starts_with("WARN"))
            .collect();
        assert_eq!(warns.len(), 1, "{warns:?}");
        assert!(
            warns[0].contains(
                "the DNS servers of a network service could not be read; \
                 its DNS is left as it is"
            ),
            "{}",
            warns[0]
        );
        assert!(!warns[0].contains("Wi-Fi"), "{}", warns[0]);

        assert_eq!(system.take_dns(&dev(), false), Ok(true));
        assert!(!runner.calls().contains(&set_line("Wi-Fi", &["198.18.0.2"])));
        assert!(
            runner
                .calls()
                .contains(&set_line("Ethernet", &["198.18.0.2"]))
        );
    }

    /// With no service to take, the system's DNS stays as it is and one open
    /// says so once — and the open after it says it again.
    #[test]
    fn no_service_to_take_is_ok_false_and_said_once() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(
            LIST,
            "An asterisk (*) denotes that a network service is disabled.\n*Bluetooth PAN\n",
        );
        let system = system_of(dir.path(), &runner);

        assert_eq!(system.dns_saved(&dev(), &[]), Ok(Vec::new()));
        assert_eq!(system.take_dns(&dev(), false), Ok(false));
        assert_eq!(system.take_dns(&dev(), false), Ok(false));
        // nothing was changed, and nothing was flushed
        assert_eq!(runner.calls(), [LIST.to_string()]);

        let warns = |lines: Vec<String>| -> Vec<String> {
            lines
                .into_iter()
                .filter(|line| line.starts_with("WARN"))
                .collect()
        };
        let said = warns(captured.lines());
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].contains("no network service's DNS could be pointed at the enhanced mode"),
            "{}",
            said[0]
        );

        // giving back nothing lets the next open say it again
        system.give_dns_back(&dev()).unwrap();
        assert_eq!(system.take_dns(&dev(), false), Ok(false));
        assert_eq!(warns(captured.lines()).len(), 2);
    }

    /// A failed `setdnsservers` names no service in what the reader gets: the
    /// tool's text and the service are in the debug log.
    #[test]
    fn setting_fails_with_a_text_that_names_no_service() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(LIST, TWO_SERVICES);
        runner.fail(
            &set_line("Wi-Fi", &["198.18.0.2"]),
            "networksetup: the service is gone",
        );
        let system = system_of(dir.path(), &runner);

        assert_eq!(
            system.dns_saved(&dev(), &[]),
            Ok(vec![saved("Wi-Fi", &[]), saved("Ethernet", &[])])
        );
        let error = system.take_dns(&dev(), false).unwrap_err();
        assert_eq!(
            error,
            "cannot point a network service's DNS at the enhanced mode \
             (the debug log has the details)"
        );
        assert!(!error.contains("Wi-Fi"), "{error}");
        let lines = captured.lines();
        assert!(
            lines.iter().any(|line| line.starts_with("DEBUG")
                && line.contains("networksetup: the service is gone")),
            "{lines:?}"
        );
        // the takeover did not finish: the caches were not flushed
        assert!(
            !runner
                .calls()
                .iter()
                .any(|line| line.contains("flushcache"))
        );
    }

    /// Putting a service back writes what it had, `empty` when it had nothing,
    /// and flushes the caches each time.
    #[test]
    fn undo_writes_back_the_servers_or_empty_and_flushes() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(LIST, SERVICES);
        let system = system_of(dir.path(), &runner);

        assert_eq!(
            system.undo(&saved("Wi-Fi", &["192.168.1.1", "1.1.1.1"])),
            Ok(())
        );
        assert_eq!(system.undo(&saved("Ethernet", &[])), Ok(()));
        assert_eq!(
            runner.calls(),
            [
                LIST.to_string(),
                set_line("Wi-Fi", &["192.168.1.1", "1.1.1.1"]),
                FLUSH[0].to_string(),
                FLUSH[1].to_string(),
                LIST.to_string(),
                set_line("Ethernet", &["empty"]),
                FLUSH[0].to_string(),
                FLUSH[1].to_string(),
            ]
        );
    }

    /// A service that is gone has nowhere to be written back: the undo is done,
    /// and INFO says so without its name.
    #[test]
    fn undo_of_a_service_that_is_gone_is_done() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(LIST, TWO_SERVICES);
        let system = system_of(dir.path(), &runner);

        assert_eq!(
            system.undo(&saved("USB 10/100/1000 LAN", &["8.8.8.8"])),
            Ok(())
        );
        assert_eq!(runner.calls(), [LIST.to_string()]);
        let lines = captured.lines();
        assert!(
            lines.iter().any(|line| line.starts_with("INFO")
                && line.contains(
                    "a network service the enhanced mode changed is gone; \
                     there is nothing to put back for it"
                )),
            "{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|line| line.starts_with("INFO") && line.contains("USB")),
            "{lines:?}"
        );
    }

    /// An undo that cannot even list the services fails: it stays in the backup
    /// and is tried again at the next start.
    #[test]
    fn an_undo_that_cannot_list_the_services_stays() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.fail(LIST, "networksetup: cannot list");
        let system = system_of(dir.path(), &runner);

        assert_eq!(
            system.undo(&saved("Wi-Fi", &[])),
            Err("cannot list the network services: networksetup: cannot list".to_string())
        );
        assert_eq!(runner.calls(), [LIST.to_string()]);
    }

    /// A backup of another platform's is dropped with a warning: there is
    /// nothing here that can put it back.
    #[test]
    fn an_undo_of_another_platform_is_dropped() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let system = system_of(dir.path(), &runner);

        assert_eq!(system.undo(&Saved::RpFilter { value: 1 }), Ok(()));
        assert!(runner.calls().is_empty());
        assert!(
            captured.lines().iter().any(|line| line.starts_with("WARN")
                && line.contains("an undo of another platform is dropped")),
            "{:?}",
            captured.lines()
        );
    }

    /// Until the network changes, the servers outside the enhanced mode are the
    /// ones `/etc/resolv.conf` held before the takeover — the physical
    /// interface's, not the responder's this file put there.
    #[test]
    fn physical_at_open_is_resolv_conf_before_the_takeover() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("resolv.conf"),
            "nameserver 192.168.1.1\nnameserver 198.18.0.2\n",
        )
        .unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(LIST, TWO_SERVICES);
        let system = system_of(dir.path(), &runner);

        let backup = system.dns_saved(&dev(), &[]).unwrap();
        assert_eq!(
            system.physical_dns(Some("en0"), &backup, false),
            Some(vec![ip("192.168.1.1")])
        );
    }

    /// After a network change, the main interface's service says where the
    /// servers are: the ones set by hand, else the ones DHCP gave the
    /// interface, else none at all — never the system's own list, which now
    /// holds the responder.
    #[test]
    fn physical_after_a_change_is_the_saved_servers_or_dhcp() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("resolv.conf"), "nameserver 10.0.0.9\n").unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(LIST, TWO_SERVICES);
        runner.reply(
            "/usr/sbin/networksetup -listnetworkserviceorder",
            SERVICE_ORDER,
        );
        runner.reply("/usr/sbin/ipconfig getpacket en0", PACKET);
        let system = system_of(dir.path(), &runner);

        // what was read before the takeover is not used after a change
        let backup = system.dns_saved(&dev(), &[]).unwrap();
        assert_eq!(
            system.physical_dns(Some("en0"), &[saved("Wi-Fi", &["192.168.1.1"])], true),
            Some(vec![ip("192.168.1.1")])
        );
        // nothing set by hand: what DHCP gave the interface
        assert_eq!(
            system.physical_dns(Some("en0"), &backup, true),
            Some(vec![ip("192.168.4.1")])
        );

        // neither: no servers outside the enhanced mode are known
        runner.reply("/usr/sbin/ipconfig getpacket en0", "");
        assert_eq!(
            system.physical_dns(Some("en0"), &backup, true),
            Some(Vec::new())
        );
    }

    /// A service the user pointed at the responder by hand — the way 5.2's
    /// warning suggests — is no server of the network either: rurge's own
    /// lookups would go to an address this machine cannot reach. After a change
    /// the DHCP servers of the main interface are used instead (R16).
    #[test]
    fn a_saved_responder_is_no_physical_server() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(
            "/usr/sbin/networksetup -listnetworkserviceorder",
            SERVICE_ORDER,
        );
        runner.reply("/usr/sbin/ipconfig getpacket en0", PACKET);
        let system = system_of(dir.path(), &runner);

        assert_eq!(
            system.physical_dns(Some("en0"), &[saved("Wi-Fi", &["198.18.0.2"])], true),
            Some(vec![ip("192.168.4.1")])
        );
    }

    /// Without a main interface (no route to the network) there is nowhere to
    /// read servers from.
    #[test]
    fn physical_without_a_primary_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let system = system_of(dir.path(), &runner);

        assert_eq!(system.physical_dns(None, &[], true), Some(Vec::new()));
        assert!(runner.calls().is_empty());
    }

    /// A crash can leave the system DNS pointing at the responder, and then
    /// `/etc/resolv.conf` holds it: the responder is never taken for a server of
    /// the network (R33, design 6.3).
    #[test]
    fn the_responder_is_never_a_physical_server() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("resolv.conf"),
            "nameserver 198.18.0.2\nnameserver fd00:6152::2\n",
        )
        .unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(LIST, TWO_SERVICES);
        let system = system_of(dir.path(), &runner);

        let backup = system.dns_saved(&dev(), &[]).unwrap();
        assert_eq!(backup, vec![saved("Wi-Fi", &[]), saved("Ethernet", &[])]);
        assert_eq!(
            system.physical_dns(Some("en0"), &backup, false),
            Some(Vec::new())
        );
    }
}
