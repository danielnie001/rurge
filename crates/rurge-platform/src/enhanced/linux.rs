//! Linux's half of the system side of the enhanced mode (M4 design 5.2, 6.1,
//! 6.3, 7.3): the adapter's addresses and routes through `ip -batch`, the
//! system DNS through systemd-resolved's per-link settings, `net.ipv4.conf.
//! all.rp_filter` for rurge's own connections' replies, and the servers of the
//! network outside the enhanced mode.
//!
//! Nothing here is `cfg`-guarded (M4c plan P2): the programs go through the
//! `CommandRunner` this system is built with, and `/etc/resolv.conf`, `/proc`
//! and `/run` through a root directory, so all of it is built and tested on
//! Windows too. What is Linux's own — the device — is [`TunDevice`]'s.
//!
//! Programs are given as absolute paths by [`LinuxTools::find`], never looked
//! up in `PATH`; every step of the translation is run through
//! [`crate::route::exec`], which gives each program at most
//! [`crate::enhanced::STEP_TIMEOUT`]. Interface names and servers are DEBUG
//! only (design 13): what INFO says is that rurge changed `rp_filter`, and
//! that the system's DNS was left alone.

use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::command::CommandRunner;
use crate::dns;
use crate::enhanced::{Adapter, Saved, System};
use crate::route::linux::LinuxTools;
use crate::route::steps::Step;
use crate::route::{MTU, RouteSet, exec, linux as translation};
use crate::tun::{DeviceSpec, TunDevice};

/// The one server of systemd-resolved's stub resolver: what `/etc/resolv.conf`
/// holds on a system whose resolver is resolved (design 5.2).
const STUB: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 53));

/// Whether `resolv.conf` is systemd-resolved's stub: its only server is 127.0.0.53.
pub fn is_resolved_stub(resolv_conf: &str) -> bool {
    let (servers, _) = dns::parse_resolv_conf(resolv_conf);
    servers == [STUB]
}

/// Linux's system half (M4 design 5.2, 6.1, 6.3, 7.3), over the programs
/// `runner` runs and the files under `root` — `/` on a real system.
pub struct LinuxSystem {
    runner: Arc<dyn CommandRunner>,
    root: PathBuf,
    scratch: PathBuf,
    /// The programs, or why there are none: `open` reports it, and rurge runs
    /// without the enhanced mode.
    tools: Result<LinuxTools, String>,
    /// Whether the system DNS is rurge's for now.
    took_dns: AtomicBool,
    /// Whether this open has already warned that the system's DNS is left as
    /// it is: one line an open, not one a reload.
    warned: AtomicBool,
}

impl LinuxSystem {
    /// Never fails: a missing `ip` is the error of `open` (rurge runs without
    /// the enhanced mode).
    pub fn new(runner: Arc<dyn CommandRunner>, root: PathBuf, scratch: PathBuf) -> LinuxSystem {
        let tools = LinuxTools::find(&root);
        LinuxSystem {
            runner,
            root,
            scratch,
            tools,
            took_dns: AtomicBool::new(false),
            warned: AtomicBool::new(false),
        }
    }

    /// The programs, or the reason there are none.
    fn tools(&self) -> Result<&LinuxTools, String> {
        self.tools.as_ref().map_err(Clone::clone)
    }

    /// Runs a translation's steps, giving each program its time
    /// ([`crate::route::exec::run`]). `Ok(false)`: the IPv6 half failed.
    fn run(&self, steps: &[Step]) -> Result<bool, String> {
        exec::run(steps, &*self.runner, &self.scratch)
    }

    /// Whether `root/etc/resolv.conf` is resolved's stub. A file that cannot be
    /// read is not one, and why is a DEBUG line: the reader is told by the
    /// warning instead.
    fn behind_the_stub(&self) -> bool {
        match fs::read_to_string(self.root.join("etc/resolv.conf")) {
            Ok(text) => is_resolved_stub(&text),
            Err(e) => {
                tracing::debug!(error = %e, "cannot read /etc/resolv.conf");
                false
            }
        }
    }

    /// `all`'s `rp_filter` and the main interface's, both read: the effective
    /// value is the larger of them (ip-sysctl).
    fn read_rp_filter(&self, primary: &str) -> Result<(u8, u8), String> {
        // the name is a path component: `..` would leave the `conf` directory
        if primary.is_empty() || primary == "." || primary == ".." || primary.contains('/') {
            return Err("cannot read rp_filter: not an interface name".to_string());
        }
        Ok((
            self.read_rp_filter_value("all")?,
            self.read_rp_filter_value(primary)?,
        ))
    }

    /// One `rp_filter`: read as 0, 1 or 2, and anything else is an error. The
    /// path stays out of the text: the reader needs what went wrong.
    fn read_rp_filter_value(&self, name: &str) -> Result<u8, String> {
        let path = self.rp_filter(name);
        let text = fs::read_to_string(&path).map_err(|e| format!("cannot read rp_filter: {e}"))?;
        match text.trim() {
            "0" => Ok(0),
            "1" => Ok(1),
            "2" => Ok(2),
            _ => Err("cannot read rp_filter: not 0, 1 or 2".to_string()),
        }
    }

    fn write_rp_filter(&self, name: &str, value: u8) -> Result<(), String> {
        fs::write(self.rp_filter(name), format!("{value}\n"))
            .map_err(|e| format!("cannot write rp_filter: {e}"))
    }

    /// `/proc/sys/net/ipv4/conf/<name>/rp_filter` under `root`: the file is
    /// read and written directly, not through `sysctl`, so a dot in a name needs
    /// no other spelling.
    fn rp_filter(&self, name: &str) -> PathBuf {
        self.root
            .join("proc/sys/net/ipv4/conf")
            .join(name)
            .join("rp_filter")
    }
}

impl System for LinuxSystem {
    fn open(&self) -> io::Result<TunDevice> {
        // before the device is touched: without `ip` no step can run, and the
        // reader is told what is missing rather than what the device says
        self.tools().map_err(io::Error::other)?;
        exec::remove_leftovers(&self.scratch);
        TunDevice::open(&DeviceSpec {
            name: "rurge".to_string(),
            mtu: usize::from(MTU),
        })
    }

    fn configure(&self, dev: &Adapter, ipv6: bool) -> Result<bool, String> {
        let tools = self.tools()?;
        let mut steps = vec![translation::configure(tools, &dev.name)];
        if ipv6 {
            steps.push(translation::configure_ipv6(tools, &dev.name));
        }
        self.run(&steps)
    }

    fn configure_ipv6(&self, dev: &Adapter) -> Result<bool, String> {
        let steps = [translation::configure_ipv6(self.tools()?, &dev.name)];
        self.run(&steps)
    }

    fn unconfigure_ipv6(&self, dev: &Adapter) -> Result<(), String> {
        let steps = [translation::unconfigure_ipv6(self.tools()?, &dev.name)];
        self.run(&steps).map(drop)
    }

    fn routes(&self, dev: &Adapter, add: &RouteSet, remove: &RouteSet) -> Result<bool, String> {
        let tools = self.tools()?;
        super::run_routes(
            add,
            remove,
            |steps| self.run(steps),
            |set| translation::add_routes(tools, &dev.name, set),
            |set| translation::remove_routes(tools, &dev.name, set),
        )
    }

    /// resolved's per-link settings go with the device: nothing outlives rurge,
    /// and nothing is backed up.
    fn dns_saved(&self, _dev: &Adapter, _saved: &[Saved]) -> Result<Vec<Saved>, String> {
        Ok(Vec::new())
    }

    fn take_dns(&self, dev: &Adapter, ipv6: bool) -> Result<bool, String> {
        let resolvectl = self
            .tools
            .as_ref()
            .is_ok_and(|tools| tools.resolvectl.is_some());
        if !resolvectl {
            tracing::debug!(
                "no resolvectl in the system directories: the resolver's per-link settings cannot be made"
            );
        }
        if self.behind_the_stub() && resolvectl {
            let steps = translation::take_dns(self.tools()?, &dev.name, ipv6);
            // A takeover that fails partway is undone as if it had happened:
            // the steps before the one that failed have set the link already,
            // and `give_dns_back`'s `revert` leaves a link nothing was set on
            // as it is (`Ignore`) — while leaving this false would keep the
            // failed open's `198.18.0.2` and `~.` until the device goes (the
            // phase 3 / M4c plan's execution record row 42).
            self.took_dns.store(true, Ordering::Relaxed);
            self.run(&steps)?;
            return Ok(true);
        }
        // not a system whose DNS goes through resolved's stub, or too old a
        // resolved to have `resolvectl`: the system's DNS stays as it is, and
        // the reader learns how to send it to the responder anyway
        if !self.warned.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "the system's DNS is not systemd-resolved's stub; it is left as it is: \
                 write hijack-dns = *:53, or point the system's DNS at 198.18.0.2"
            );
        }
        Ok(false)
    }

    fn give_dns_back(&self, dev: &Adapter) -> Result<(), String> {
        let took = self.took_dns.swap(false, Ordering::Relaxed);
        // the next open warns again if it has to leave the DNS alone
        self.warned.store(false, Ordering::Relaxed);
        if took {
            let steps = translation::give_dns_back(self.tools()?, &dev.name);
            self.run(&steps)?;
        }
        Ok(())
    }

    fn adjust_saved(
        &self,
        primary: Option<&str>,
        saved: &[Saved],
    ) -> Result<Option<Saved>, String> {
        // what is saved already is not read again: a backup brought along must
        // close cleanly even where the interface is gone
        if saved
            .iter()
            .any(|saved| matches!(saved, Saved::RpFilter { .. }))
        {
            return Ok(None);
        }
        let Some(primary) = primary else {
            return Ok(None);
        };
        let (all, interface) = self.read_rp_filter(primary)?;
        if all.max(interface) != 1 {
            return Ok(None);
        }
        // loosening `all` keeps the effective value loose whatever network
        // comes next, so the interface's own value is left alone
        Ok(Some(Saved::RpFilter { value: all }))
    }

    fn adjust(&self) -> Result<(), String> {
        self.write_rp_filter("all", 2)?;
        tracing::info!(
            "loosened net.ipv4.conf.all.rp_filter to 2: replies to rurge's own connections \
             come in on another interface than the routes to their senders"
        );
        Ok(())
    }

    fn undo(&self, saved: &Saved) -> Result<(), String> {
        match saved {
            Saved::RpFilter { value } => {
                self.write_rp_filter("all", *value)?;
                tracing::info!("put net.ipv4.conf.all.rp_filter back to {value}");
                Ok(())
            }
            Saved::MacDns { .. } => {
                // another platform's backup (a data directory brought along):
                // nothing here can put it back, and left as it is it fails
                // every start (M4c plan P9)
                tracing::warn!("an undo of another platform is dropped");
                Ok(())
            }
        }
    }

    fn physical_dns(
        &self,
        _primary: Option<&str>,
        _saved: &[Saved],
        _network_changed: bool,
    ) -> Option<Vec<IpAddr>> {
        if !self.took_dns.load(Ordering::Relaxed) {
            return None;
        }
        // what resolved writes for the whole machine: every link's servers, the
        // adapter's among them
        match fs::read_to_string(self.root.join("run/systemd/resolve/resolv.conf")) {
            Ok(text) => {
                let (servers, _) = dns::parse_resolv_conf(&text);
                Some(dns::without_rurge_space(servers))
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "cannot read resolved's resolv.conf; no servers outside the enhanced mode are known"
                );
                Some(Vec::new())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::command::CommandRunner;
    use crate::log_capture::LogCapture;

    /// A root with `ip` and `resolvectl` where a Linux system has them.
    fn with_tools(root: &Path) {
        put(root, "/usr/sbin/ip", "");
        put(root, "/usr/sbin/resolvectl", "");
    }

    /// `path` under `root` (written as an absolute path) made a file holding
    /// `text`, with the directories above it.
    fn put(root: &Path, path: &str, text: &str) {
        let path = root.join(path.trim_start_matches('/'));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// What `path` under `root` holds.
    fn contents(root: &Path, path: &str) -> String {
        fs::read_to_string(root.join(path.trim_start_matches('/'))).unwrap()
    }

    fn system_of(root: &Path, runner: &Arc<Fake>) -> LinuxSystem {
        LinuxSystem::new(runner.clone(), root.to_path_buf(), root.join("enhanced"))
    }

    fn dev() -> Adapter {
        Adapter {
            name: "rurge".to_string(),
            index: None,
        }
    }

    fn set(v4: &[&str], v6: &[&str]) -> RouteSet {
        RouteSet {
            v4: v4.iter().map(|net| net.parse().unwrap()).collect(),
            v6: v6.iter().map(|net| net.parse().unwrap()).collect(),
        }
    }

    /// What systemd-resolved's stub resolver leaves in `/etc/resolv.conf`.
    const STUB_CONF: &str = "nameserver 127.0.0.53\noptions edns0 trust-ad\n";

    /// One command a system ran: what it was, and the script it read (as it
    /// was on disk at that moment — nothing of the steps in this file is a
    /// script, but the `ip` batches are).
    #[derive(Clone)]
    struct Call {
        cmd: Vec<String>,
        script: String,
    }

    impl Call {
        /// The command as it was meant: the script's file — its name carries
        /// the process and a number — stands as `{script}`.
        fn line(&self) -> String {
            self.cmd
                .iter()
                .map(|word| match word.contains("rurge-steps-") {
                    true => crate::route::steps::SCRIPT.to_string(),
                    false => word.clone(),
                })
                .collect::<Vec<String>>()
                .join(" ")
        }
    }

    /// Runs nothing: it records every command, and fails the steps whose
    /// program (by its file name) is `program` and whose script or whose
    /// command line holds `text`. It reads one file and no other: the script
    /// [`exec::run`] writes under the test's `scratch`, whose name carries
    /// `rurge-steps-` — every other argument is a word, not a path to look at.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<Call>>,
        failures: Mutex<Vec<(String, String, String)>>,
    }

    impl Fake {
        fn fail(&self, program: &str, text: &str, message: &str) {
            self.failures.lock().unwrap().push((
                program.to_string(),
                text.to_string(),
                message.to_string(),
            ));
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        /// The commands, the script's path as `{script}`.
        fn lines(&self) -> Vec<String> {
            self.calls().iter().map(Call::line).collect()
        }

        /// What the programs read, one per command (nothing for a step
        /// without a script).
        fn scripts(&self) -> Vec<String> {
            self.calls()
                .iter()
                .map(|call| call.script.clone())
                .collect()
        }
    }

    impl CommandRunner for Fake {
        fn run(&self, cmd: &[String]) -> io::Result<String> {
            // what the program would read: the run put the script's path in
            // the arguments, and the file is there until the program ends.
            // Only that one argument is a path (the others are `dns`, `rurge`,
            // `~.`): nothing outside the test's scratch is opened.
            let script = cmd
                .iter()
                .skip(1)
                .filter(|arg| arg.contains("rurge-steps-"))
                .find_map(|arg| fs::read_to_string(arg).ok())
                .unwrap_or_default();
            let call = Call {
                cmd: cmd.to_vec(),
                script,
            };
            let program = Path::new(&cmd[0])
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let failure = self
                .failures
                .lock()
                .unwrap()
                .iter()
                .find(|(name, text, _)| {
                    *name == program && (call.script.contains(text) || call.line().contains(text))
                })
                .map(|(_, _, message)| message.clone());
            self.calls.lock().unwrap().push(call);
            match failure {
                Some(message) => Err(io::Error::other(message)),
                None => Ok(String::new()),
            }
        }
    }

    /// The two programs are looked for in `/usr/sbin`, `/sbin`, `/usr/bin`,
    /// `/bin`, in that order, and reported as absolute paths; without `ip`
    /// there is no enhanced mode.
    #[test]
    fn tools_are_looked_for_in_the_system_directories_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        put(root, "/bin/ip", "");
        put(root, "/sbin/ip", "");
        put(root, "/usr/bin/ip", "");
        put(root, "/bin/resolvectl", "");
        put(root, "/usr/bin/resolvectl", "");

        let tools = LinuxTools::find(root).unwrap();
        assert_eq!(tools.ip, PathBuf::from("/sbin/ip"));
        assert_eq!(tools.resolvectl, Some(PathBuf::from("/usr/bin/resolvectl")));

        // `/usr/sbin` comes before `/sbin`, and each program is found alone
        put(root, "/usr/sbin/ip", "");
        put(root, "/usr/sbin/resolvectl", "");
        let tools = LinuxTools::find(root).unwrap();
        assert_eq!(tools.ip, PathBuf::from("/usr/sbin/ip"));
        assert_eq!(
            tools.resolvectl,
            Some(PathBuf::from("/usr/sbin/resolvectl"))
        );

        // nothing there
        let empty = tempfile::tempdir().unwrap();
        let error = match LinuxTools::find(empty.path()) {
            Ok(_) => panic!("a system without ip was taken for one with it"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            "cannot find the ip program (iproute2) in /usr/sbin, /sbin, /usr/bin or /bin"
        );
        // a `resolvectl` alone is not enough
        put(empty.path(), "/usr/bin/resolvectl", "");
        assert!(LinuxTools::find(empty.path()).is_err());
    }

    /// A missing `ip` fails the open before the device is touched: what the
    /// reader gets is the missing program, not a device error (and no test
    /// here opens a device, M4c plan P19).
    #[test]
    fn without_ip_the_open_fails_before_the_device() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);
        let error = match system.open() {
            Ok(_) => panic!("a device was opened where ip is missing"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "cannot find the ip program (iproute2) in /usr/sbin, /sbin, /usr/bin or /bin"
        );
        // not "no /dev/net/tun": nothing here is about the device
        assert_ne!(error.kind(), io::ErrorKind::NotFound);
        assert!(runner.calls().is_empty());
    }

    /// The stub is `127.0.0.53` and nothing else: a second server, another
    /// address, an empty file is no stub, and the system's DNS is then left
    /// alone.
    #[test]
    fn the_stub_is_resolved_s_and_nothing_else() {
        assert!(is_resolved_stub(STUB_CONF));
        for text in [
            "nameserver 127.0.0.53\nnameserver 8.8.8.8\n",
            "nameserver 127.0.0.54\n",
            "nameserver 192.168.1.1\n",
            "",
        ] {
            assert!(!is_resolved_stub(text), "{text:?}");
        }
    }

    /// Behind the stub, the resolver is pointed at this link only: the
    /// responder's address, the domain route, the default route and a flushed
    /// cache — the four commands word for word, with the IPv6 address when
    /// IPv6 is on.
    #[test]
    fn the_dns_is_taken_behind_the_stub_with_resolvectl() {
        for (ipv6, dns) in [
            (false, "/usr/sbin/resolvectl dns rurge 198.18.0.2"),
            (
                true,
                "/usr/sbin/resolvectl dns rurge 198.18.0.2 fd00:6152::2",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            with_tools(dir.path());
            put(dir.path(), "/etc/resolv.conf", STUB_CONF);
            let runner = Arc::new(Fake::default());
            let system = system_of(dir.path(), &runner);

            assert_eq!(system.take_dns(&dev(), ipv6), Ok(true));
            assert_eq!(
                runner.lines(),
                [
                    dns,
                    "/usr/sbin/resolvectl domain rurge ~.",
                    "/usr/sbin/resolvectl default-route rurge yes",
                    "/usr/sbin/resolvectl flush-caches",
                ]
            );
            // the per-link settings go with the device: nothing to back up
            assert_eq!(system.dns_saved(&dev(), &[]), Ok(Vec::new()));
        }
    }

    /// A takeover that fails partway is undone as if it had happened: the
    /// steps before the one that failed have set the link already, so
    /// `give_dns_back` reverts it — a link nothing was set on is `Ignore`, and
    /// `revert` leaves it that way (the phase 3 / M4c plan's execution record
    /// row 42).
    #[test]
    fn a_takeover_that_fails_partway_is_given_back() {
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        put(dir.path(), "/etc/resolv.conf", STUB_CONF);
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);
        // `domain` fails: `llmnr` and the rest before it have set the link
        runner.fail("resolvectl", "domain rurge", "the link is gone");

        let error = system.take_dns(&dev(), false).unwrap_err();
        assert!(error.contains("the link is gone"), "{error}");

        system.give_dns_back(&dev()).unwrap();
        let lines = runner.lines();
        assert!(
            lines
                .iter()
                .any(|line| line.ends_with("resolvectl revert rurge")),
            "the half-taken-over link was not reverted: {lines:?}"
        );
    }

    /// Without the stub, the system's DNS is left alone and one open says why
    /// once, however often it asks.
    #[test]
    fn without_the_stub_the_dns_is_left_and_said_once() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        put(dir.path(), "/etc/resolv.conf", "nameserver 192.168.1.1\n");
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);

        assert_eq!(system.take_dns(&dev(), false), Ok(false));
        assert_eq!(system.take_dns(&dev(), false), Ok(false));
        assert!(runner.calls().is_empty());

        let warns: Vec<String> = captured
            .lines()
            .into_iter()
            .filter(|line| line.starts_with("WARN"))
            .collect();
        assert_eq!(warns.len(), 1, "{warns:?}");
        assert!(
            warns[0].contains(
                "the system's DNS is not systemd-resolved's stub; it is left as it is: \
                 write hijack-dns = *:53, or point the system's DNS at 198.18.0.2"
            ),
            "{}",
            warns[0]
        );
    }

    /// A stub but no `resolvectl` (systemd before 239): nothing is changed.
    #[test]
    fn without_resolvectl_the_dns_is_left() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "/usr/sbin/ip", "");
        put(dir.path(), "/etc/resolv.conf", STUB_CONF);
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);

        assert_eq!(system.take_dns(&dev(), true), Ok(false));
        assert!(runner.calls().is_empty());
    }

    /// Giving the DNS back reverts what was taken, once: with no takeover there
    /// is no `revert`, and a second close reverts nothing.
    #[test]
    fn giving_back_reverts_only_what_was_taken() {
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        put(dir.path(), "/etc/resolv.conf", STUB_CONF);
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);

        system.give_dns_back(&dev()).unwrap();
        assert!(runner.calls().is_empty());

        assert_eq!(system.take_dns(&dev(), false), Ok(true));
        let taken = runner.calls().len();
        system.give_dns_back(&dev()).unwrap();
        let lines = runner.lines();
        assert_eq!(lines.len(), taken + 1);
        assert_eq!(lines[taken], "/usr/sbin/resolvectl revert rurge");

        // and only once
        system.give_dns_back(&dev()).unwrap();
        assert_eq!(runner.calls().len(), taken + 1);
    }

    /// `all = 0` and the main interface's `1` make the effective value 1 —
    /// strict: `all` is loosened to 2, and put back to what it was at the
    /// close.
    #[test]
    fn strict_rp_filter_is_loosened_and_put_back() {
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        put(dir.path(), "/proc/sys/net/ipv4/conf/all/rp_filter", "0\n");
        put(dir.path(), "/proc/sys/net/ipv4/conf/eth0/rp_filter", "1\n");
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);

        assert_eq!(
            system.adjust_saved(Some("eth0"), &[]),
            Ok(Some(Saved::RpFilter { value: 0 }))
        );
        system.adjust().unwrap();
        assert_eq!(
            contents(dir.path(), "/proc/sys/net/ipv4/conf/all/rp_filter").trim(),
            "2"
        );
        system.undo(&Saved::RpFilter { value: 0 }).unwrap();
        assert_eq!(
            contents(dir.path(), "/proc/sys/net/ipv4/conf/all/rp_filter").trim(),
            "0"
        );
    }

    /// Only an effective value of 1 needs the loosening. The kernel uses the
    /// larger of `all` and the interface (ip-sysctl).
    #[test]
    fn rp_filter_that_is_loose_or_off_is_left_alone() {
        for (all, interface, expected) in [
            ("0", "0", None),
            ("2", "1", None),
            ("0", "2", None),
            ("1", "2", None),
            ("1", "0", Some(1)),
        ] {
            let dir = tempfile::tempdir().unwrap();
            with_tools(dir.path());
            put(dir.path(), "/proc/sys/net/ipv4/conf/all/rp_filter", all);
            put(
                dir.path(),
                "/proc/sys/net/ipv4/conf/eth0/rp_filter",
                interface,
            );
            let runner = Arc::new(Fake::default());
            let system = system_of(dir.path(), &runner);

            assert_eq!(
                system.adjust_saved(Some("eth0"), &[]),
                Ok(expected.map(|value| Saved::RpFilter { value })),
                "all = {all}, the interface = {interface}"
            );
        }
    }

    /// What is already saved is not read again: a backup brought along into a
    /// system whose interface is gone still closes cleanly.
    #[test]
    fn a_saved_rp_filter_is_not_read_again() {
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);

        assert_eq!(
            system.adjust_saved(Some("eth0"), &[Saved::RpFilter { value: 1 }]),
            Ok(None)
        );
    }

    /// A name that is no interface name is refused rather than made a path
    /// (`..` would leave the `conf` directory), and the error does not carry
    /// it.
    #[test]
    fn an_interface_name_that_is_not_a_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);

        for name in ["", ".", "..", "eth/0"] {
            assert_eq!(
                system.adjust_saved(Some(name), &[]),
                Err("cannot read rp_filter: not an interface name".to_string()),
                "{name:?}"
            );
        }

        // a file that is not there is an error too, and neither the path nor
        // the interface's name is in it (design 13); the name is checked
        // rather than `/proc`, which on Windows would be no check at all
        let error = system.adjust_saved(Some("eth0"), &[]).unwrap_err();
        assert!(error.starts_with("cannot read rp_filter: "), "{error}");
        assert!(!error.contains("eth0"), "{error}");
        assert!(!error.contains("/proc"), "{error}");
    }

    /// A value outside 0 ～ 2 is not read as one.
    #[test]
    fn rp_filter_content_that_is_not_0_1_or_2_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        put(dir.path(), "/proc/sys/net/ipv4/conf/all/rp_filter", "3\n");
        put(dir.path(), "/proc/sys/net/ipv4/conf/eth0/rp_filter", "0\n");
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);

        assert_eq!(
            system.adjust_saved(Some("eth0"), &[]),
            Err("cannot read rp_filter: not 0, 1 or 2".to_string())
        );
    }

    /// The servers outside the enhanced mode are resolved's own list, less
    /// rurge's space — the responder and the fake addresses are not servers of
    /// the network.
    #[test]
    fn the_physical_servers_are_resolved_s_upstreams_without_rurges_space() {
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        put(dir.path(), "/etc/resolv.conf", STUB_CONF);
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);
        assert_eq!(system.take_dns(&dev(), true), Ok(true));

        // resolved has not written its list yet: nothing known, not the
        // system's own list (that one is the responder now)
        assert_eq!(system.physical_dns(None, &[], false), Some(Vec::new()));

        put(
            dir.path(),
            "/run/systemd/resolve/resolv.conf",
            concat!(
                "nameserver 198.18.0.2\n",
                "nameserver 192.168.1.1\n",
                "nameserver fd00:6152::2\n",
            ),
        );
        assert_eq!(
            system.physical_dns(None, &[], false),
            Some(vec!["192.168.1.1".parse::<IpAddr>().unwrap()])
        );
    }

    /// Without a takeover the system's own list is right: no servers from
    /// here.
    #[test]
    fn without_the_takeover_the_system_list_stands() {
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        put(dir.path(), "/etc/resolv.conf", STUB_CONF);
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);

        assert_eq!(system.physical_dns(None, &[], false), None);
    }

    /// The adapter comes up in one batch — or two with IPv6 — and the IPv6
    /// batch failing leaves the open without IPv6 rather than failing it. The
    /// failure itself is no WARN here: the manager writes that line.
    #[test]
    fn configure_with_ipv6_runs_two_batches_and_an_ipv6_failure_is_ok_false() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);

        assert_eq!(system.configure(&dev(), false), Ok(true));
        assert_eq!(runner.lines(), ["/usr/sbin/ip -batch {script}"]);
        assert_eq!(
            runner.scripts(),
            [concat!(
                "link set dev rurge mtu 1500\n",
                "addr add 198.18.0.1/15 dev rurge\n",
                "link set dev rurge up\n",
            )]
        );

        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);
        assert_eq!(system.configure(&dev(), true), Ok(true));
        assert_eq!(runner.lines().len(), 2);
        assert_eq!(runner.scripts()[1], "addr add fd00:6152::1/64 dev rurge\n");

        // the IPv6 batch failing: no IPv6, and the open goes on
        let runner = Arc::new(Fake::default());
        runner.fail("ip", "addr add fd00:6152::1", "no IPv6 on this system");
        let system = system_of(dir.path(), &runner);
        assert_eq!(system.configure(&dev(), true), Ok(false));

        // the IPv6 address goes with the device: one batch that goes on
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);
        system.unconfigure_ipv6(&dev()).unwrap();
        assert_eq!(runner.lines(), ["/usr/sbin/ip -force -batch {script}"]);
        assert_eq!(runner.scripts(), ["addr del fd00:6152::1/64 dev rurge\n"]);

        let warns: Vec<String> = captured
            .lines()
            .into_iter()
            .filter(|line| line.starts_with("WARN"))
            .collect();
        assert!(warns.is_empty(), "the manager warns, not this: {warns:?}");
    }

    /// The IPv6 half of the routes failing takes that half out again, and the
    /// routes to remove go all the same; the open then goes on without IPv6.
    /// The routes are this system's half of the open, so the one WARN about
    /// them is written here — by `run_routes`, once, without a name or a
    /// prefix in it (M4c plan's execution record row 29).
    #[test]
    fn an_ipv6_route_failure_takes_the_ipv6_routes_out_again() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        let runner = Arc::new(Fake::default());
        runner.fail("ip", "route add 100::/8", "File exists");
        let system = system_of(dir.path(), &runner);

        let add = set(&["1.0.0.0/8"], &["100::/8"]);
        let remove = set(&["2.0.0.0/8"], &[]);
        assert_eq!(system.routes(&dev(), &add, &remove), Ok(false));
        assert_eq!(
            runner.lines(),
            [
                "/usr/sbin/ip -batch {script}",
                "/usr/sbin/ip -batch {script}",
                "/usr/sbin/ip -force -batch {script}",
                "/usr/sbin/ip -force -batch {script}",
            ]
        );
        assert_eq!(
            runner.scripts(),
            [
                "route add 1.0.0.0/8 dev rurge\n",
                "route add 100::/8 dev rurge\n",
                "route del 100::/8 dev rurge\n",
                "route del 2.0.0.0/8 dev rurge\n",
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

    /// An IPv4 route that cannot be added is the error of the open: what came
    /// after it does not run, and the manager undoes what came before.
    #[test]
    fn an_ipv4_route_failure_is_the_error() {
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        let runner = Arc::new(Fake::default());
        runner.fail(
            "ip",
            "route add 1.0.0.0/8",
            "`ip -batch …` failed: File exists",
        );
        let system = system_of(dir.path(), &runner);

        let add = set(&["1.0.0.0/8"], &["100::/8"]);
        let remove = set(&["2.0.0.0/8"], &[]);
        assert_eq!(
            system.routes(&dev(), &add, &remove),
            Err("`ip -batch …` failed: File exists".to_string())
        );
        assert_eq!(runner.lines(), ["/usr/sbin/ip -batch {script}"]);
    }

    /// A backup of another platform's (a data directory brought along) cannot
    /// be put back here: it is dropped with a warning, and the warning does not
    /// name what it was about (design 13).
    #[test]
    fn an_undo_of_another_platform_is_dropped() {
        let captured = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        with_tools(dir.path());
        let runner = Arc::new(Fake::default());
        let system = system_of(dir.path(), &runner);

        let saved = Saved::MacDns {
            service: "Wi-Fi".to_string(),
            servers: vec!["8.8.8.8".parse().unwrap()],
        };
        assert_eq!(system.undo(&saved), Ok(()));
        let lines = captured.lines();
        assert!(
            lines.iter().any(|line| line.starts_with("WARN")
                && line.contains("an undo of another platform is dropped")),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains("Wi-Fi")),
            "{lines:?}"
        );
        assert!(runner.calls().is_empty());
    }
}
