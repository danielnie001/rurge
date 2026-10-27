//! Windows's half of the system side of the enhanced mode (M4 design 5.4, 6.3,
//! 7.4): the adapter's address, MTU, routes and the system DNS through `netsh
//! -f` scripts, and what `netsh` answers read back.
//!
//! Nothing here is `cfg`-guarded (M4c plan P2): the programs go through the
//! `CommandRunner` this system is built with, and the scripts to a scratch
//! directory of their own, so all of it is built and tested on every platform.
//! What is Windows's own — the device — is [`crate::tun::TunDevice`]'s.
//!
//! `netsh -f`'s exit status and its words are not what decides (M4d plan P10):
//! whether the script stops at a line that failed, and what it prints, are not
//! documented and differ with the system's language. Every step that changes
//! something therefore runs as a step that may fail, and what it was to change
//! is read back afterwards — an address, an MTU, a route whose row carries the
//! adapter's index, a DNS server. What a step failed with, what the readback
//! printed and what was not found are DEBUG only (design 13).
//!
//! `system` gives this system out on Windows: the steps name the adapter by
//! the interface index the device opened, and `wintun.dll` comes from
//! `--wintun-dll` or sits next to `rurge.exe`.

use std::io;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

use ipnet::{IpNet, Ipv4Net, Ipv6Net};

use crate::command::CommandRunner;
use crate::enhanced::{Adapter, Saved, System};
use crate::route::steps::{OnFailure, Step};
use crate::route::windows::WindowsTools;
use crate::route::{
    ADAPTER_V4, ADAPTER_V6, MTU, RESPONDER_V4, RESPONDER_V6, RouteSet, exec, windows as translation,
};
use crate::tun::{DeviceSpec, TunDevice};

/// Windows's system half (M4 design 5.4, 6.3, 7.4), over the programs `runner`
/// runs and the scripts under `scratch` (the data directory's `enhanced`).
pub struct WindowsSystem {
    runner: Arc<dyn CommandRunner>,
    tools: WindowsTools,
    scratch: PathBuf,
    /// Where `wintun.dll` is, or why there is none: `open` reports it, and
    /// rurge runs without the enhanced mode.
    dll: Result<PathBuf, String>,
}

impl WindowsSystem {
    pub fn new(
        runner: Arc<dyn CommandRunner>,
        tools: WindowsTools,
        scratch: PathBuf,
        dll: Result<PathBuf, String>,
    ) -> WindowsSystem {
        WindowsSystem {
            runner,
            tools,
            scratch,
            dll,
        }
    }

    /// Runs a translation's steps as steps that may fail, whatever the
    /// translation said about them: `netsh -f`'s exit status and its words are
    /// not what decides (plan P10). What a step failed with — a script that
    /// could not be written too — is one DEBUG line from [`exec::run`], and the
    /// readback that follows finds the setting missing, which is what the
    /// caller reports. The run's own answer is not read: it says whether an
    /// `Ipv6Off` step failed, and after this there are none left.
    fn run_ignoring(&self, steps: &[Step]) {
        let ignored: Vec<Step> = steps
            .iter()
            .map(|step| Step {
                on_failure: OnFailure::Ignore,
                ..step.clone()
            })
            .collect();
        drop(exec::run(&ignored, &*self.runner, &self.scratch));
    }

    /// Runs one readback step and gives what it printed: [`exec::run`] keeps no
    /// output for its steps, so this runs the step itself. The program, what it
    /// was given and what it printed are DEBUG only (design 13); a readback that
    /// cannot run prints nothing, and what it was to show counts as missing.
    fn read_back(&self, step: &Step) -> String {
        let mut cmd = Vec::with_capacity(step.args.len() + 1);
        cmd.push(step.program.to_string_lossy().into_owned());
        cmd.extend(step.args.iter().cloned());
        tracing::debug!(
            program = %step.program.display(),
            args = %step.args.join(" "),
            "reading back a setting of the enhanced mode"
        );
        match self.runner.run(&cmd) {
            Ok(output) => {
                tracing::debug!(output = %output, "what the readback printed");
                output
            }
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "a readback failed; what it was to show is taken for missing"
                );
                String::new()
            }
        }
    }

    /// Whether the adapter has the address of this family ([`ADAPTER_V4`] and
    /// [`ADAPTER_V6`]): the addresses of `show addresses` are read, its words
    /// are not (plan P10).
    fn read_back_address(&self, index: u32, v6: bool) -> bool {
        let text = self.read_back(&translation::show_addresses(&self.tools, index, v6));
        let wanted = match v6 {
            false => IpAddr::V4(ADAPTER_V4),
            true => IpAddr::V6(ADAPTER_V6),
        };
        let there = translation::addresses_in(&text).contains(&wanted);
        if !there {
            tracing::debug!(address = %wanted, "the adapter does not have its address");
        }
        there
    }

    /// Whether the adapter's MTU is the one the steps set (plan P11): what the
    /// system gives it by itself is not documented, and a larger one lets the
    /// system send the stack packets bigger than its MTU.
    fn read_back_mtu(&self, index: u32, v6: bool) -> bool {
        let text = self.read_back(&translation::show_mtu(&self.tools, index, v6));
        let mtu = translation::mtu_in(&text);
        let there = mtu == Some(u32::from(MTU));
        if !there {
            tracing::debug!(
                mtu = ?mtu,
                wanted = MTU,
                "the adapter's MTU is not the one the steps set"
            );
        }
        there
    }

    /// Whether the adapter's DNS servers of one family hold `server`.
    fn read_back_dns(&self, index: u32, v6: bool, server: IpAddr) -> bool {
        let text = self.read_back(&translation::show_dns(&self.tools, index, v6));
        let there = translation::addresses_in(&text).contains(&server);
        if !there {
            tracing::debug!(
                server = %server,
                "the adapter's DNS servers do not hold it"
            );
        }
        there
    }

    /// What the routes of `add` that `netsh` was to add did not make it onto
    /// the adapter: IPv4 first — a missing one takes the open down — and the
    /// IPv6 half after it, `Ok(false)` when one of those is missing (plan P10).
    /// `run_routes` says the one WARN about the IPv6 half and takes it out
    /// again.
    fn read_back_routes(&self, index: u32, add: &RouteSet) -> Result<bool, String> {
        if !add.v4.is_empty() {
            let text = self.read_back(&translation::show_routes(&self.tools, false));
            let rows = routes_of(&text, index);
            let missing: Vec<Ipv4Net> = add
                .v4
                .iter()
                .copied()
                .filter(|net| !rows.contains(&IpNet::V4(*net)))
                .collect();
            if !missing.is_empty() {
                tracing::debug!(
                    missing = %prefixes(&missing),
                    "the routes netsh was to add are not on the adapter"
                );
                // what of them did go in comes out again before the open fails:
                // the manager undoes what it knows, not this set
                self.run_ignoring(&translation::remove_routes(&self.tools, index, add));
                return Err(format!(
                    "{} of the routes are missing after netsh added them",
                    missing.len()
                ));
            }
        }
        // nothing to add is nothing to read: the table of that family is not
        // asked for
        if add.v6.is_empty() {
            return Ok(true);
        }
        let text = self.read_back(&translation::show_routes(&self.tools, true));
        let rows = routes_of(&text, index);
        let missing: Vec<Ipv6Net> = add
            .v6
            .iter()
            .copied()
            .filter(|net| !rows.contains(&IpNet::V6(*net)))
            .collect();
        if missing.is_empty() {
            return Ok(true);
        }
        tracing::debug!(
            missing = %prefixes(&missing),
            "the IPv6 routes netsh was to add are not on the adapter"
        );
        Ok(false)
    }
}

/// The adapter's interface index: every line of this file names the adapter by
/// it (a leftover adapter with the same name is never mistaken for it). An
/// `Adapter` without one is a programming error, not a system's answer.
fn index(dev: &Adapter) -> Result<u32, String> {
    dev.index
        .ok_or_else(|| "the virtual adapter has no interface index".to_string())
}

/// The rows of a `show route` table that are on `index`: another adapter's row
/// with the same prefix is no route into this one.
fn routes_of(text: &str, index: u32) -> Vec<IpNet> {
    translation::routes_in(text)
        .into_iter()
        .filter(|(_, row)| *row == index)
        .map(|(net, _)| net)
        .collect()
}

/// Prefixes as one line of a DEBUG message: nothing of them goes above DEBUG.
fn prefixes<T: std::fmt::Display>(nets: &[T]) -> String {
    nets.iter()
        .map(ToString::to_string)
        .collect::<Vec<String>>()
        .join(" ")
}

impl System for WindowsSystem {
    /// `wintun.dll` is weighed before anything is touched: without it there is
    /// no device to open, and what a run that died left under the scratch
    /// directory is no news to a reader who cannot have the enhanced mode. The
    /// device and its interface index come from [`TunDevice::open`].
    fn open(&self) -> io::Result<TunDevice> {
        let dll = match &self.dll {
            Ok(dll) => dll,
            Err(error) => return Err(io::Error::other(error.clone())),
        };
        exec::remove_leftovers(&self.scratch);
        TunDevice::open(&DeviceSpec {
            name: "rurge".to_string(),
            mtu: usize::from(MTU),
            wintun_dll: Some(dll.clone()),
        })
    }

    /// The adapter's address and MTU, read back, and then — with `ipv6` — the
    /// IPv6 half.
    fn configure(&self, dev: &Adapter, ipv6: bool) -> Result<bool, String> {
        let index = index(dev)?;
        self.run_ignoring(&[translation::configure(&self.tools, index)]);
        if !self.read_back_address(index, false) {
            return Err("the virtual adapter did not get its address".to_string());
        }
        if !self.read_back_mtu(index, false) {
            return Err(format!("the virtual adapter's MTU is not {MTU}"));
        }
        if ipv6 {
            return self.configure_ipv6(dev);
        }
        Ok(true)
    }

    fn configure_ipv6(&self, dev: &Adapter) -> Result<bool, String> {
        let index = index(dev)?;
        self.run_ignoring(&[translation::configure_ipv6(&self.tools, index)]);
        if self.read_back_address(index, true) && self.read_back_mtu(index, true) {
            return Ok(true);
        }
        // the one WARN about IPv6 is the manager's (the phase 3 / M4d plan's
        // execution record row 13): what of the IPv6 half did go in is taken out
        // again, and the open goes on without it
        tracing::debug!("the IPv6 half of the adapter did not come up; it is taken out again");
        self.run_ignoring(&[translation::unconfigure_ipv6(&self.tools, index)]);
        Ok(false)
    }

    /// The undo of [`System::configure_ipv6`]: it goes on past a failure, and
    /// nothing is read back — the address it takes out is one nothing should
    /// have any more.
    fn unconfigure_ipv6(&self, dev: &Adapter) -> Result<(), String> {
        let index = index(dev)?;
        self.run_ignoring(&[translation::unconfigure_ipv6(&self.tools, index)]);
        Ok(())
    }

    fn routes(&self, dev: &Adapter, add: &RouteSet, remove: &RouteSet) -> Result<bool, String> {
        let index = index(dev)?;
        let tools = &self.tools;
        // the batch that adds, as `run_routes` will build it: only after that
        // one is anything read back
        let added = translation::add_routes(tools, index, add);
        super::run_routes(
            add,
            remove,
            |steps| {
                self.run_ignoring(steps);
                if steps != added.as_slice() {
                    return Ok(true);
                }
                self.read_back_routes(index, add)
            },
            |set| translation::add_routes(tools, index, set),
            |set| translation::remove_routes(tools, index, set),
        )
    }

    /// The adapter's DNS settings live in the adapter's own configuration and
    /// go with it when it goes (plan P12): nothing outlives rurge, so nothing is
    /// read before the takeover and nothing is backed up.
    fn dns_saved(&self, _dev: &Adapter, _saved: &[Saved]) -> Result<Vec<Saved>, String> {
        Ok(Vec::new())
    }

    /// The responder for the IPv4 list, read back, and with `ipv6` the IPv6 one
    /// too. The IPv6 half missing is one WARN and no failure: IPv4 lookups take
    /// that route already (the phase 3 / M4d plan's execution record row 13).
    fn take_dns(&self, dev: &Adapter, ipv6: bool) -> Result<bool, String> {
        let index = index(dev)?;
        self.run_ignoring(&translation::take_dns(&self.tools, index, ipv6));
        if !self.read_back_dns(index, false, IpAddr::V4(RESPONDER_V4)) {
            return Err("the system's DNS could not be pointed at the enhanced mode".to_string());
        }
        if ipv6 && !self.read_back_dns(index, true, IpAddr::V6(RESPONDER_V6)) {
            tracing::warn!(
                "the system's IPv6 DNS could not be pointed at the enhanced mode; \
                 it is left as it is"
            );
        }
        Ok(true)
    }

    /// Nothing here outlives rurge (plan P12): the adapter's DNS settings go
    /// with the adapter, so there is nothing to give back.
    fn give_dns_back(&self, _dev: &Adapter) -> Result<(), String> {
        Ok(())
    }

    /// Nothing to adjust either: the one thing Linux adjusts is `rp_filter`,
    /// and Windows has nothing of the kind, so this never asks for an
    /// [`System::adjust`].
    fn adjust_saved(
        &self,
        _primary: Option<&str>,
        _saved: &[Saved],
    ) -> Result<Option<Saved>, String> {
        Ok(None)
    }

    /// Never asked for by [`WindowsSystem::adjust_saved`], which answers
    /// `Ok(None)` (the phase 3 / M4d plan's execution record row 27).
    fn adjust(&self) -> Result<(), String> {
        Ok(())
    }

    /// A [`Saved`] this platform ever wrote does not exist (plan P12), so what
    /// is put back here is another platform's — a data directory brought along
    /// — and left as it is it fails every start (the M4c plan's P9).
    fn undo(&self, _saved: &Saved) -> Result<(), String> {
        tracing::warn!("an undo of another platform is dropped");
        Ok(())
    }

    /// The system's own list is right (design 6.3): `dns::servers` walks every
    /// adapter, and the virtual one's servers — the responder — fall in rurge's
    /// space and are dropped from it there, so no takeover needs covering.
    fn physical_dns(
        &self,
        _primary: Option<&str>,
        _saved: &[Saved],
        _network_changed: bool,
    ) -> Option<Vec<IpAddr>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::command::CommandRunner;
    use crate::command::testing::FakeRunner;
    use crate::log_capture::LogCapture;
    use crate::route::steps::SCRIPT;

    /// `netsh interface ipv4 show addresses name=<i>` for the adapter with its
    /// address, and the same output for an adapter that came up with another
    /// one. Hand-written after the layout in the plan's P20, not taken from a
    /// machine.
    const ADDRESSES_V4: &str = "\
Configuration for interface \"rurge\"
    IP Address:                           198.18.0.1
    Subnet Prefix:                        198.18.0.0/15 (mask 255.254.0.0)
";
    const ADDRESSES_V4_OTHER: &str = "\
Configuration for interface \"rurge\"
    IP Address:                           192.168.1.10
";

    /// `netsh interface ipv6 show addresses interface=<i>`, with the address
    /// the steps add and with the link-local one an adapter has before they
    /// run. Hand-written, not taken from a machine.
    const ADDRESSES_V6: &str = "\
Configuration for interface \"rurge\"
    IP Address:                           fd00:6152::1
";
    const ADDRESSES_V6_LINK_LOCAL: &str = "\
Configuration for interface \"rurge\"
    IP Address:                           fe80::1234%12
";

    /// `netsh interface ipv4|ipv6 show subinterfaces interface=<i>`: the MTU is
    /// the first number of the data row. Hand-written, not taken from a
    /// machine.
    const MTU_1500: &str = "\
                             MTU  MediaSenseState   Bytes In  Bytes Out  Interface
-------------------------  ---------------  ---------  ---------  -------------
                      1500                1          0          0  rurge
";
    const MTU_1400: &str = "\
                             MTU  MediaSenseState   Bytes In  Bytes Out  Interface
-------------------------  ---------------  ---------  ---------  -------------
                      1400                1          0          0  rurge
";

    /// `netsh interface ipv4|ipv6 show route`: the columns are `Publish Type
    /// Met Prefix Idx Gateway/Interface Name` (plan P20). The first has one
    /// prefix on the adapter's index (12) and one on another's (7); the second
    /// has a prefix of the adapter's own and `1.0.0.0/8` on another adapter
    /// only; the third is the IPv6 table without `100::/8`. Hand-written, not
    /// taken from a machine.
    const ROUTES_V4: &str = "\
Publish  Type      Met  Prefix                     Idx  Gateway/Interface Name
-------  --------  ---  -------------------------  ---  ------------------------
no       Manual    0    1.0.0.0/8                   12  rurge
no       Manual    0    2.0.0.0/8                    7  Ethernet 2
";
    const ROUTES_V4_OTHER_INDEX: &str = "\
Publish  Type      Met  Prefix                     Idx  Gateway/Interface Name
-------  --------  ---  -------------------------  ---  ------------------------
no       Manual    0    1.0.0.0/8                    7  Ethernet 2
no       Manual    0    198.18.0.0/15               12  rurge
";
    const ROUTES_V6_WITHOUT_THE_ROUTE: &str = "\
Publish  Type      Met  Prefix                     Idx  Gateway/Interface Name
-------  --------  ---  -------------------------  ---  ------------------------
no       Manual    0    fd00:6152::/64              12  rurge
";

    /// `netsh interface ipv4|ipv6 show dnsservers name=<i>`, with the responder
    /// and with a server that is not it. Hand-written, not taken from a
    /// machine.
    const DNS_V4: &str = "\
Configuration for interface \"rurge\"
    Statically Configured DNS Servers:    198.18.0.2
";
    const DNS_V4_OTHER: &str = "\
Configuration for interface \"rurge\"
    Statically Configured DNS Servers:    192.168.1.1
";
    const DNS_V6: &str = "\
Configuration for interface \"rurge\"
    Statically Configured DNS Servers:    fd00:6152::2
";
    const DNS_V6_OTHER: &str = "\
Configuration for interface \"rurge\"
    Statically Configured DNS Servers:    fd00:6151::53
";

    /// The programs at the place a real system has them. Nothing here runs one.
    fn tools() -> WindowsTools {
        WindowsTools::under(Path::new(r"C:\Windows"))
    }

    /// The command line of one call, as the fake runner records it.
    fn netsh(args: &str) -> String {
        format!("{} {args}", tools().netsh.display())
    }

    /// A scripted `netsh -f`: the file's path is the run's own and carries the
    /// process and a number (M4c's task 2), so it stands as `{script}` here.
    fn netsh_script() -> String {
        netsh(&format!("-f {SCRIPT}"))
    }

    /// The calls the runner was given, a script's path as `{script}`.
    fn lines(calls: &[String]) -> Vec<String> {
        calls
            .iter()
            .map(|call| match call.split_once(" -f ") {
                Some((program, _path)) => format!("{program} -f {SCRIPT}"),
                None => call.clone(),
            })
            .collect()
    }

    /// A system over the test's own scratch directory. No `wintun.dll`: no test
    /// here opens a device (plan P19).
    fn with_runner(dir: &Path, runner: Arc<dyn CommandRunner>) -> WindowsSystem {
        WindowsSystem::new(
            runner,
            tools(),
            dir.join("enhanced"),
            Err("wintun.dll is not signed".to_string()),
        )
    }

    fn system_of(dir: &Path, runner: Arc<FakeRunner>) -> WindowsSystem {
        with_runner(dir, runner)
    }

    fn dev() -> Adapter {
        Adapter {
            name: "rurge".to_string(),
            index: Some(12),
        }
    }

    fn set(v4: &[&str], v6: &[&str]) -> RouteSet {
        RouteSet {
            v4: v4.iter().map(|net| net.parse().unwrap()).collect(),
            v6: v6.iter().map(|net| net.parse().unwrap()).collect(),
        }
    }

    /// Without `wintun.dll` the open fails before anything is touched: no
    /// program runs, and what a run that died left under the scratch directory
    /// is still there — the cleanup is not reached either (plan P19: no test
    /// opens a device).
    #[test]
    fn without_wintun_dll_the_open_fails_before_the_device() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("enhanced");
        fs::create_dir_all(&scratch).unwrap();
        let leftover = scratch.join("rurge-steps-1-0.txt");
        fs::write(
            &leftover,
            "interface ipv4 add address name=12 address=198.18.0.1 mask=255.254.0.0 store=active\n",
        )
        .unwrap();
        let runner = Arc::new(FakeRunner::default());
        let system = WindowsSystem::new(
            runner.clone(),
            tools(),
            scratch,
            Err("wintun.dll is not signed".to_string()),
        );

        let error = match system.open() {
            Ok(_) => panic!("a device was opened without a wintun.dll"),
            Err(error) => error,
        };
        assert_eq!(error.to_string(), "wintun.dll is not signed");
        assert!(runner.calls().is_empty(), "{:?}", runner.calls());
        assert!(
            leftover.exists(),
            "the open cleaned up before the dll was weighed"
        );
    }

    /// The adapter comes up by the script, and then its address and its MTU
    /// are read back — in that order, one readback for each (plan P10). With
    /// IPv6 the IPv6 script goes next, and its own address and MTU are read.
    #[test]
    fn configure_runs_the_script_then_reads_back_the_address_and_the_mtu() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(
            &netsh("interface ipv4 show addresses name=12"),
            ADDRESSES_V4,
        );
        runner.reply(
            &netsh("interface ipv4 show subinterfaces interface=12"),
            MTU_1500,
        );
        let system = system_of(dir.path(), runner.clone());

        assert_eq!(system.configure(&dev(), false), Ok(true));
        assert_eq!(
            lines(&runner.calls()),
            [
                netsh_script(),
                netsh("interface ipv4 show addresses name=12"),
                netsh("interface ipv4 show subinterfaces interface=12"),
            ]
        );

        let runner = Arc::new(FakeRunner::default());
        runner.reply(
            &netsh("interface ipv4 show addresses name=12"),
            ADDRESSES_V4,
        );
        runner.reply(
            &netsh("interface ipv4 show subinterfaces interface=12"),
            MTU_1500,
        );
        runner.reply(
            &netsh("interface ipv6 show addresses interface=12"),
            ADDRESSES_V6,
        );
        runner.reply(
            &netsh("interface ipv6 show subinterfaces interface=12"),
            MTU_1500,
        );
        let system = system_of(dir.path(), runner.clone());

        assert_eq!(system.configure(&dev(), true), Ok(true));
        assert_eq!(
            lines(&runner.calls()),
            [
                netsh_script(),
                netsh("interface ipv4 show addresses name=12"),
                netsh("interface ipv4 show subinterfaces interface=12"),
                netsh_script(),
                netsh("interface ipv6 show addresses interface=12"),
                netsh("interface ipv6 show subinterfaces interface=12"),
            ]
        );

        // an adapter without an index is no system's answer, and nothing runs
        let runner = Arc::new(FakeRunner::default());
        let system = system_of(dir.path(), runner.clone());
        let no_index = Adapter {
            name: "rurge".to_string(),
            index: None,
        };
        assert_eq!(
            system.configure(&no_index, false),
            Err("the virtual adapter has no interface index".to_string())
        );
        assert!(runner.calls().is_empty(), "{:?}", runner.calls());
    }

    /// An address that is not the adapter's is the error of the open, and the
    /// MTU is not even read: the address comes first.
    #[test]
    fn a_missing_address_fails_configure() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(
            &netsh("interface ipv4 show addresses name=12"),
            ADDRESSES_V4_OTHER,
        );
        let system = system_of(dir.path(), runner.clone());

        assert_eq!(
            system.configure(&dev(), false),
            Err("the virtual adapter did not get its address".to_string())
        );
        assert_eq!(
            lines(&runner.calls()),
            [
                netsh_script(),
                netsh("interface ipv4 show addresses name=12"),
            ]
        );
    }

    /// An MTU that is not the one the steps set is the error of the open: a
    /// larger one lets the system send packets bigger than the stack's MTU into
    /// the adapter (plan P11).
    #[test]
    fn an_mtu_that_is_not_1500_fails_configure() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(
            &netsh("interface ipv4 show addresses name=12"),
            ADDRESSES_V4,
        );
        runner.reply(
            &netsh("interface ipv4 show subinterfaces interface=12"),
            MTU_1400,
        );
        let system = system_of(dir.path(), runner.clone());

        assert_eq!(
            system.configure(&dev(), false),
            Err("the virtual adapter's MTU is not 1500".to_string())
        );
    }

    /// The IPv6 address not being there is no error: the open goes on without
    /// IPv6, and what of the IPv6 half did go in is taken out again. The one
    /// WARN about it is the manager's, not this (the phase 3 / M4d plan's
    /// execution record row 13).
    #[test]
    fn an_ipv6_address_that_is_missing_is_ok_false_and_taken_out_again() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(
            &netsh("interface ipv4 show addresses name=12"),
            ADDRESSES_V4,
        );
        runner.reply(
            &netsh("interface ipv4 show subinterfaces interface=12"),
            MTU_1500,
        );
        runner.reply(
            &netsh("interface ipv6 show addresses interface=12"),
            ADDRESSES_V6_LINK_LOCAL,
        );
        let system = system_of(dir.path(), runner.clone());

        assert_eq!(system.configure(&dev(), true), Ok(false));
        // the third script has nothing else to be than the IPv6 address going
        // out again: what each script holds is `route/windows.rs`'s to test
        assert_eq!(
            lines(&runner.calls()),
            [
                netsh_script(),
                netsh("interface ipv4 show addresses name=12"),
                netsh("interface ipv4 show subinterfaces interface=12"),
                netsh_script(),
                netsh("interface ipv6 show addresses interface=12"),
                netsh_script(),
            ]
        );

        let warns: Vec<String> = captured
            .lines()
            .into_iter()
            .filter(|line| line.starts_with("WARN"))
            .collect();
        assert!(warns.is_empty(), "the manager warns, not this: {warns:?}");
    }

    /// Only a row on the adapter's own index counts: the same prefix on another
    /// adapter is no route into this one (plan P10).
    #[test]
    fn routes_are_read_back_by_the_adapter_s_index() {
        let dir = tempfile::tempdir().unwrap();
        let add = set(&["1.0.0.0/8"], &[]);

        let runner = Arc::new(FakeRunner::default());
        runner.reply(&netsh("interface ipv4 show route"), ROUTES_V4_OTHER_INDEX);
        let system = system_of(dir.path(), runner.clone());
        assert_eq!(
            system.routes(&dev(), &add, &RouteSet::default()),
            Err("1 of the routes are missing after netsh added them".to_string())
        );

        let runner = Arc::new(FakeRunner::default());
        runner.reply(&netsh("interface ipv4 show route"), ROUTES_V4);
        let system = system_of(dir.path(), runner.clone());
        assert_eq!(system.routes(&dev(), &add, &RouteSet::default()), Ok(true));
    }

    /// IPv4 routes that are not there are the error of the open, and what was
    /// added is taken out again first — the manager undoes what it knows, not
    /// this set. The routes to remove are not removed: the open has failed.
    #[test]
    fn missing_ipv4_routes_take_the_added_ones_out_and_fail() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(&netsh("interface ipv4 show route"), ROUTES_V4);
        let system = system_of(dir.path(), runner.clone());

        let add = set(&["1.0.0.0/8", "2.0.0.0/8"], &[]);
        let remove = set(&["3.0.0.0/8"], &[]);
        assert_eq!(
            system.routes(&dev(), &add, &remove),
            Err("1 of the routes are missing after netsh added them".to_string())
        );
        assert_eq!(
            lines(&runner.calls()),
            [
                netsh_script(),
                netsh("interface ipv4 show route"),
                netsh_script(),
            ]
        );
    }

    /// IPv6 routes that are not there cost IPv6, not the open: the IPv6 half
    /// goes out again, the routes to remove are removed all the same, and the
    /// one WARN about it is `run_routes`' (the phase 3 / M4d plan's execution
    /// record row 13).
    #[test]
    fn missing_ipv6_routes_are_ok_false() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(&netsh("interface ipv4 show route"), ROUTES_V4);
        runner.reply(
            &netsh("interface ipv6 show route"),
            ROUTES_V6_WITHOUT_THE_ROUTE,
        );
        let system = system_of(dir.path(), runner.clone());

        let add = set(&["1.0.0.0/8"], &["100::/8"]);
        let remove = set(&["3.0.0.0/8"], &[]);
        assert_eq!(system.routes(&dev(), &add, &remove), Ok(false));
        assert_eq!(
            lines(&runner.calls()),
            [
                netsh_script(),
                netsh_script(),
                netsh("interface ipv4 show route"),
                netsh("interface ipv6 show route"),
                netsh_script(),
                netsh_script(),
            ]
        );

        let warns: Vec<String> = captured
            .lines()
            .into_iter()
            .filter(|line| line.starts_with("WARN"))
            .collect();
        assert_eq!(warns.len(), 1, "one WARN: {warns:?}");
        assert_eq!(
            warns[0],
            "WARN the IPv6 routes could not be added to the virtual adapter; they are left out"
        );
    }

    /// The system DNS is pointed at the responder and read back: the IPv4 call,
    /// then (IPv6 off) the IPv6 list cleared, then the caches flushed, then the
    /// readback. With IPv6 on both servers are set in one call and both are
    /// read back.
    #[test]
    fn the_dns_is_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let flush = format!("{} /flushdns", tools().ipconfig.display());

        let runner = Arc::new(FakeRunner::default());
        runner.reply(&netsh("interface ipv4 show dnsservers name=12"), DNS_V4);
        let system = system_of(dir.path(), runner.clone());
        assert_eq!(system.take_dns(&dev(), false), Ok(true));
        assert_eq!(
            lines(&runner.calls()),
            [
                netsh_script(),
                netsh_script(),
                flush.clone(),
                netsh("interface ipv4 show dnsservers name=12"),
            ]
        );

        let runner = Arc::new(FakeRunner::default());
        runner.reply(&netsh("interface ipv4 show dnsservers name=12"), DNS_V4);
        runner.reply(&netsh("interface ipv6 show dnsservers name=12"), DNS_V6);
        let system = system_of(dir.path(), runner.clone());
        assert_eq!(system.take_dns(&dev(), true), Ok(true));
        assert_eq!(
            lines(&runner.calls()),
            [
                netsh_script(),
                flush,
                netsh("interface ipv4 show dnsservers name=12"),
                netsh("interface ipv6 show dnsservers name=12"),
            ]
        );
    }

    /// An IPv6 list without the responder after the takeover costs IPv6, not
    /// the open: one WARN, and the call answers `Ok(true)` (the phase 3 / M4d
    /// plan's execution record row 62). The WARN names neither the adapter nor
    /// a server: what a user reads at WARN says nothing of either (M4 design
    /// 13); the readback's own DEBUG lines are where those may be.
    #[test]
    fn a_missing_ipv6_dns_is_one_warn_and_still_ok() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(&netsh("interface ipv4 show dnsservers name=12"), DNS_V4);
        // the IPv6 list holds a server that is not the responder; a readback
        // that printed nothing reads the same way
        runner.reply(
            &netsh("interface ipv6 show dnsservers name=12"),
            DNS_V6_OTHER,
        );
        let system = system_of(dir.path(), runner.clone());

        assert_eq!(system.take_dns(&dev(), true), Ok(true));

        let warns: Vec<String> = captured
            .lines()
            .into_iter()
            .filter(|line| line.starts_with("WARN"))
            .collect();
        assert_eq!(warns.len(), 1, "one WARN: {warns:?}");
        assert_eq!(
            warns[0],
            "WARN the system's IPv6 DNS could not be pointed at the enhanced mode; it is left as \
             it is"
        );
        // the line itself names neither the adapter nor a server: what is said
        // at WARN is read by every user of the enhanced mode, and the names and
        // addresses are DEBUG only (M4 design 13). The DEBUG lines are not
        // checked: they repeat what netsh printed, the interface's name in it.
        for found in [
            "rurge",
            "198.18.0.1",
            "198.18.0.2",
            "fd00:6152::",
            "fd00:6151::",
        ] {
            assert!(
                !warns[0].contains(found),
                "{found} is DEBUG only: {warns:?}"
            );
        }
    }

    /// Servers that are not the responder after the takeover is the error of
    /// the open: DNS would go on being asked the system's own servers.
    #[test]
    fn a_dns_that_did_not_change_fails_take_dns() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.reply(
            &netsh("interface ipv4 show dnsservers name=12"),
            DNS_V4_OTHER,
        );
        let system = system_of(dir.path(), runner.clone());

        assert_eq!(
            system.take_dns(&dev(), false),
            Err("the system's DNS could not be pointed at the enhanced mode".to_string())
        );
    }

    /// `netsh -f`'s exit status decides nothing (plan P10): a script that fails
    /// with the readback right is a success, and a script that goes through
    /// with the readback wrong is a failure.
    #[test]
    fn netsh_s_exit_status_does_not_decide() {
        let dir = tempfile::tempdir().unwrap();

        let runner = Arc::new(FailingScripts::default());
        runner.0.reply(
            &netsh("interface ipv4 show addresses name=12"),
            ADDRESSES_V4,
        );
        runner.0.reply(
            &netsh("interface ipv4 show subinterfaces interface=12"),
            MTU_1500,
        );
        let system = with_runner(dir.path(), runner.clone());
        assert_eq!(system.configure(&dev(), false), Ok(true));
        assert_eq!(lines(&runner.0.calls())[0], netsh_script());

        let runner = Arc::new(FakeRunner::default());
        let system = system_of(dir.path(), runner.clone());
        assert_eq!(
            system.configure(&dev(), false),
            Err("the virtual adapter did not get its address".to_string())
        );
    }

    /// Nothing here outlives rurge: nothing is saved, nothing is given back,
    /// nothing is adjusted, and an undo — which can only be another platform's
    /// (a data directory brought along) — is dropped with a warning (plan P12,
    /// the M4c plan's P9).
    #[test]
    fn the_windows_system_never_saves_and_drops_foreign_undos() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let system = system_of(dir.path(), runner.clone());

        assert_eq!(system.dns_saved(&dev(), &[]), Ok(Vec::new()));
        assert_eq!(system.give_dns_back(&dev()), Ok(()));
        assert_eq!(system.adjust_saved(None, &[]), Ok(None));
        assert_eq!(system.adjust(), Ok(()));
        assert_eq!(
            system.undo(&Saved::MacDns {
                service: "Wi-Fi".to_string(),
                servers: vec!["8.8.8.8".parse().unwrap()],
            }),
            Ok(())
        );
        assert_eq!(system.undo(&Saved::RpFilter { value: 1 }), Ok(()));
        assert!(runner.calls().is_empty(), "{:?}", runner.calls());

        let lines = captured.lines();
        let warns: Vec<&String> = lines
            .iter()
            .filter(|line| line.starts_with("WARN"))
            .collect();
        assert_eq!(warns.len(), 2, "one WARN an undo: {warns:?}");
        for warn in warns {
            assert!(
                warn.contains("an undo of another platform is dropped"),
                "{warn}"
            );
        }
        assert!(
            !lines.iter().any(|line| line.contains("Wi-Fi")),
            "{lines:?}"
        );
    }

    /// The system's own list of DNS servers is right (design 6.3): the
    /// virtual adapter's servers fall in rurge's space and are dropped from it
    /// by `dns::servers`, so there is nothing to answer here.
    #[test]
    fn physical_dns_is_the_system_s_own() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let system = system_of(dir.path(), runner.clone());

        assert_eq!(system.physical_dns(None, &[], false), None);
        assert_eq!(
            system.physical_dns(Some("Ethernet"), &[Saved::RpFilter { value: 1 }], true),
            None
        );
        assert!(runner.calls().is_empty(), "{:?}", runner.calls());
    }

    /// The fake runner of a `netsh` whose scripts fail — their file's path is
    /// not known in advance, so no reply can be keyed on it — and whose
    /// readbacks are the replies it was given.
    #[derive(Default)]
    struct FailingScripts(FakeRunner);

    impl CommandRunner for FailingScripts {
        fn run(&self, cmd: &[String]) -> io::Result<String> {
            // recorded, and answered, as any other call
            let reply = self.0.run(cmd);
            match cmd.iter().any(|arg| arg == "-f") {
                true => Err(io::Error::other("`netsh -f …` failed (exit status: 1)")),
                false => reply,
            }
        }
    }
}
