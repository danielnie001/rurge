//! What the readers of the network ask of the machine: programs to run, files
//! to read, the interface table and (on Windows) the adapter list. The readers
//! of the three platforms are compiled on every host and take a [`Host`], so
//! each is tested against a fake one; only [`RealHost`] touches the system.
//!
//! Best effort: what cannot be had is `None`, or empty, and the first time a
//! kind of thing cannot be had a line goes to the debug log (never the name of
//! a network: those are in the answers, not in the questions).

use super::windows::AdapterView;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// How long a program that is asked about the network has to answer.
pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) trait Host {
    /// Runs a program that only looks (the first word of `cmd`), for at most
    /// [`COMMAND_TIMEOUT`]; what it wrote when it ended well, otherwise `None`.
    fn run(&self, cmd: &[&str]) -> Option<String>;
    /// A file's text.
    fn file(&self, path: &str) -> Option<String>;
    fn exists(&self, path: &str) -> bool;
    /// The names in a directory, in order.
    fn dir(&self, path: &str) -> Vec<String>;
    /// Every address of every interface, with the name of its interface.
    fn addresses(&self) -> Vec<(String, IpAddr)>;
    /// Windows: the adapters the system lists. Nothing elsewhere.
    fn adapters(&self) -> Vec<AdapterView>;
}

pub(crate) struct RealHost;

impl Host for RealHost {
    fn run(&self, cmd: &[&str]) -> Option<String> {
        let argv: Vec<String> = cmd.iter().map(|part| (*part).to_string()).collect();
        match crate::command::run_with_timeout(&argv, COMMAND_TIMEOUT) {
            Ok(text) => Some(text),
            Err(e) => {
                // `env LC_ALL=C nmcli …` is a question to `nmcli`
                let program = cmd
                    .iter()
                    .find(|word| **word != "env" && !word.contains('='))
                    .copied()
                    .unwrap_or_default();
                say_once(program, || {
                    tracing::debug!(
                        program,
                        reason = ?e.kind(),
                        "cannot ask this program about the network; what it would say is taken as unknown"
                    );
                });
                None
            }
        }
    }

    fn file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }

    fn dir(&self, path: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn addresses(&self) -> Vec<(String, IpAddr)> {
        match if_addrs::get_if_addrs() {
            Ok(interfaces) => interfaces
                .into_iter()
                .map(|interface| (interface.name.clone(), interface.ip()))
                .collect(),
            Err(e) => {
                say_once("interfaces", || {
                    tracing::debug!(reason = ?e.kind(), "cannot list the network interfaces");
                });
                Vec::new()
            }
        }
    }

    fn adapters(&self) -> Vec<AdapterView> {
        super::windows::system_adapters()
    }
}

/// What has been said, so that a thing which goes wrong at every look is said
/// once: a tool that is not installed is not worth a line every few seconds.
struct Said(Mutex<Vec<String>>);

impl Said {
    const fn new() -> Said {
        Said(Mutex::new(Vec::new()))
    }

    /// True the first time `key` is asked about.
    fn first_time(&self, key: &str) -> bool {
        let mut said = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if said.iter().any(|said| said == key) {
            return false;
        }
        said.push(key.to_string());
        true
    }
}

/// Runs `say` the first time `key` comes up in this process.
pub(crate) fn say_once(key: &str, say: impl FnOnce()) {
    static SAID: Said = Said::new();
    if SAID.first_time(key) {
        say();
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::{Host, IpAddr};
    use crate::netenv::windows::AdapterView;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A machine that says what the test wrote: the answer of a program by its
    /// command line (the words joined by a space; a program that is not there
    /// has no answer, like one that fails), the text of a file by its path.
    /// Every program that is run is remembered.
    #[derive(Default)]
    pub(crate) struct FakeHost {
        commands: HashMap<String, String>,
        files: HashMap<String, String>,
        addresses: Vec<(String, IpAddr)>,
        adapters: Vec<AdapterView>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeHost {
        pub(crate) fn with_command(mut self, line: &str, output: &str) -> Self {
            self.commands.insert(line.to_string(), output.to_string());
            self
        }

        /// A file; an empty text is how a directory or a marker file is told
        /// to exist.
        pub(crate) fn with_file(mut self, path: &str, text: &str) -> Self {
            self.files.insert(path.to_string(), text.to_string());
            self
        }

        pub(crate) fn with_address(mut self, interface: &str, ip: &str) -> Self {
            self.addresses
                .push((interface.to_string(), ip.parse().unwrap()));
            self
        }

        pub(crate) fn with_adapter(mut self, adapter: AdapterView) -> Self {
            self.adapters.push(adapter);
            self
        }

        /// The programs that were run, as command lines, in order.
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Host for FakeHost {
        fn run(&self, cmd: &[&str]) -> Option<String> {
            let line = cmd.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            self.commands.get(&line).cloned()
        }

        fn file(&self, path: &str) -> Option<String> {
            self.files.get(path).cloned()
        }

        fn exists(&self, path: &str) -> bool {
            let below = format!("{path}/");
            self.files
                .keys()
                .any(|file| file == path || file.starts_with(&below))
        }

        fn dir(&self, path: &str) -> Vec<String> {
            let below = format!("{}/", path.trim_end_matches('/'));
            let mut names: Vec<String> = self
                .files
                .keys()
                .filter_map(|file| file.strip_prefix(&below))
                .filter_map(|rest| rest.split('/').next())
                .map(str::to_string)
                .collect();
            names.sort();
            names.dedup();
            names
        }

        fn addresses(&self) -> Vec<(String, IpAddr)> {
            self.addresses.clone()
        }

        fn adapters(&self) -> Vec<AdapterView> {
            self.adapters.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeHost;
    use super::*;

    #[test]
    fn a_thing_is_said_once() {
        let said = Said::new();
        assert!(said.first_time("nmcli"));
        assert!(!said.first_time("nmcli"));
        assert!(said.first_time("iw"));
        assert!(!said.first_time("iw"));
        assert!(!said.first_time("nmcli"));
    }

    /// The fake machine is what the readers' tests stand on.
    #[test]
    fn the_fake_machine_answers_what_it_was_told() {
        let host = FakeHost::default()
            .with_command("route -n get default", "gateway: 192.0.2.1")
            .with_file("/sys/class/net/wlan0/wireless", "")
            .with_file("/sys/class/net/wlan0/operstate", "up\n")
            .with_file("/sys/class/net/eth0/type", "1\n");
        assert_eq!(
            host.run(&["route", "-n", "get", "default"]).as_deref(),
            Some("gateway: 192.0.2.1")
        );
        assert_eq!(host.run(&["route", "-n", "get", "-inet6", "default"]), None);
        assert_eq!(
            host.calls(),
            ["route -n get default", "route -n get -inet6 default"]
        );
        assert!(host.exists("/sys/class/net/wlan0/wireless"));
        assert!(host.exists("/sys/class/net/wlan0"));
        assert!(!host.exists("/sys/class/net/wlan1"));
        assert_eq!(host.dir("/sys/class/net"), ["eth0", "wlan0"]);
        assert_eq!(
            host.file("/sys/class/net/eth0/type").as_deref(),
            Some("1\n")
        );
    }
}
