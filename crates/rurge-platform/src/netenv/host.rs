//! What the readers of the network ask of the machine: programs to run, files
//! to read, the interface table and (on Windows) the adapter list. The readers
//! of the three platforms are compiled on every host and take a [`Host`], so
//! each is tested against a fake one; only [`RealHost`] touches the system.
//!
//! Best effort: what cannot be had is `None`, or empty, and the first time a
//! kind of thing cannot be had a line goes to the debug log (never the name of
//! a network: those are in the answers, not in the questions).

use super::windows::AdapterView;
use std::ffi::OsString;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// How long a program that is asked about the network has to answer.
pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// A question put to a program: the words of its command line, and the
/// environment variables it is run with besides the ones it inherits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ask {
    pub words: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Ask {
    pub(crate) fn new(words: &[&str]) -> Ask {
        Ask {
            words: words.iter().map(|word| (*word).to_string()).collect(),
            env: Vec::new(),
        }
    }

    pub(crate) fn with_env(mut self, name: &str, value: &str) -> Ask {
        self.env.push((name.to_string(), value.to_string()));
        self
    }

    /// The question as a line, the variables first and then the words, as a
    /// shell would be told it. The fake machine names its answers by it.
    #[cfg(test)]
    pub(crate) fn line(&self) -> String {
        let variables = self
            .env
            .iter()
            .map(|(name, value)| format!("{name}={value}"));
        let words = self.words.iter().cloned();
        variables.chain(words).collect::<Vec<_>>().join(" ")
    }
}

pub(crate) trait Host {
    /// Runs a program that only looks (the first word of the question), for at
    /// most [`COMMAND_TIMEOUT`]; what it wrote when it ended well, otherwise
    /// `None`.
    fn run(&self, ask: &Ask) -> Option<String>;
    /// An environment variable of this process.
    fn var(&self, name: &str) -> Option<OsString>;
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
    fn run(&self, ask: &Ask) -> Option<String> {
        run_ask(ask, COMMAND_TIMEOUT)
    }

    fn var(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
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

/// Runs the program of a question, with its variables, and gives up on it after
/// `timeout`. A failure is `None` and, the first time for a program, one line
/// to the debug log with the name of the program and the kind of error, never
/// what it wrote.
fn run_ask(ask: &Ask, timeout: Duration) -> Option<String> {
    let env: Vec<(&str, &str)> = ask
        .env
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    match crate::command::run_with_timeout_env(&ask.words, timeout, &env) {
        Ok(text) => Some(text),
        Err(e) => {
            // `cmd` is named by its path, which is long and says where Windows is
            let program = ask
                .words
                .first()
                .and_then(|word| Path::new(word).file_stem())
                .and_then(|name| name.to_str())
                .unwrap_or("a program");
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
    use super::{Ask, Host, IpAddr};
    use crate::netenv::windows::AdapterView;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::sync::Mutex;

    /// A machine that says what the test wrote: the answer of a program by its
    /// question as a line (see [`Ask::line`]: the variables, then the words; a
    /// program that is not there has no answer, like one that fails), the text
    /// of a file by its path. Every program that is run is remembered.
    #[derive(Default)]
    pub(crate) struct FakeHost {
        commands: HashMap<String, String>,
        files: HashMap<String, String>,
        addresses: Vec<(String, IpAddr)>,
        adapters: Vec<AdapterView>,
        vars: HashMap<String, OsString>,
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

        /// An environment variable of the process.
        pub(crate) fn with_var(mut self, name: &str, value: &str) -> Self {
            self.vars.insert(name.to_string(), value.into());
            self
        }

        /// The programs that were run, as command lines, in order.
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Host for FakeHost {
        fn run(&self, ask: &Ask) -> Option<String> {
            let line = ask.line();
            self.calls.lock().unwrap().push(line.clone());
            self.commands.get(&line).cloned()
        }

        fn var(&self, name: &str) -> Option<OsString> {
            self.vars.get(name).cloned()
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

    /// A question is told by its variables, which come first, and its words:
    /// the same question with another variable is another question to the
    /// fake machine, so a test that is answered has the variable right.
    #[test]
    fn a_question_is_named_by_its_variables_and_its_words() {
        let plain = Ask::new(&["nmcli", "-t", "dev"]);
        assert_eq!(plain.line(), "nmcli -t dev");
        let with = plain.clone().with_env("LC_ALL", "C").with_env("A", "b c");
        assert_eq!(with.line(), "LC_ALL=C A=b c nmcli -t dev");
        let host = FakeHost::default()
            .with_command("LC_ALL=C nmcli -t dev", "yes")
            .with_var("SystemRoot", "D:\\Win");
        assert_eq!(host.run(&plain), None);
        assert_eq!(
            host.run(&plain.with_env("LC_ALL", "C")).as_deref(),
            Some("yes")
        );
        assert_eq!(host.var("SystemRoot"), Some("D:\\Win".into()));
        assert_eq!(host.var("Path"), None);
    }

    /// The child half of `the_variables_of_a_question_reach_the_program`.
    #[test]
    #[ignore = "started by the_variables_of_a_question_reach_the_program"]
    fn echo_the_variable() {
        if let Some(value) = std::env::var_os("RURGE_TEST_ECHO") {
            println!("echo:{}", value.to_string_lossy());
        }
    }

    /// The variables a question carries are the program's, over the ones it
    /// inherits (`LC_ALL` for `nmcli`, `NoDefaultCurrentDirectoryInExePath` for
    /// `cmd`). The test binary is the program, as in `command`'s tests; the
    /// time it has is longer than a look at the network gives, for a binary
    /// that is started cold.
    #[test]
    fn the_variables_of_a_question_reach_the_program() {
        let exe = std::env::current_exe().unwrap();
        let exe = exe.to_string_lossy();
        let words = [
            &*exe,
            "--ignored",
            "--exact",
            "netenv::host::tests::echo_the_variable",
            "--nocapture",
        ];
        let sent = Ask::new(&words).with_env("RURGE_TEST_ECHO", "hello there");
        let out = run_ask(&sent, Duration::from_secs(60)).expect("the program answers");
        assert!(out.contains("echo:hello there"), "{out}");
        let not_sent = Ask::new(&words);
        let out = run_ask(&not_sent, Duration::from_secs(60)).expect("the program answers");
        assert!(!out.contains("echo:"), "{out}");
    }

    /// The real machine has an environment, and does not make one up.
    #[test]
    fn the_real_machine_tells_its_environment() {
        assert!(RealHost.var("PATH").is_some());
        assert_eq!(RealHost.var("RURGE_NO_SUCH_VARIABLE_ANYWHERE"), None);
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
            host.run(&Ask::new(&["route", "-n", "get", "default"]))
                .as_deref(),
            Some("gateway: 192.0.2.1")
        );
        assert_eq!(
            host.run(&Ask::new(&["route", "-n", "get", "-inet6", "default"])),
            None
        );
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
