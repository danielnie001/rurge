//! The L2 privileged tests of the enhanced mode (M4 design section 14,
//! M4-D27; M4c plan task 9, P17): a real virtual adapter, real routes and a
//! real `rp_filter`, inside two network namespaces this file makes.
//!
//! `rl2-a-<pid>` is the machine rurge runs on and `rl2-b-<pid>` is the
//! internet: the two are joined by a veth pair that exists in no other
//! namespace, `10.231.0.2/30` on a's end and `10.231.0.1/30` on b's, and the
//! servers a test talks to are on b. The address beyond that pair,
//! `10.231.1.1/32`, is b's own loopback: it is the address of everything on
//! the internet, and a reaches it only through its default route, which is the
//! veth. Nothing here touches the host: the addresses do not exist outside the
//! two namespaces, and the only thing written outside them is the
//! `/etc/netns/<a>/` directory that `ip netns exec` mounts over a's
//! `/etc/resolv.conf`, so that a's resolver is not the machine's.
//!
//! Nothing here runs by default, on this machine or in CI. A test starts by
//! asking for `RURGE_TEST_PRIVILEGED=1` and prints why it did not run when it
//! is not there; only a disposable Linux VM, as root, sets it. These tests have
//! never been run anywhere (M4c plan, execution record).
//!
//! Addresses are the plan's (P17) except where the coordinator's R17 moved
//! them: traffic to `10.231.0.1` would take the kernel's own route for the
//! veth (`10.231.0.0/30`), which is longer than the base prefixes rurge puts
//! into the adapter, so it would never enter the adapter; `10.231.1.1` is in
//! `8.0.0.0/5` and nowhere in a's connected routes, so an application's
//! traffic to it does.

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use rurge_platform::route::linux::LinuxTools;

/// The sentence every test prints and stops at when it is not allowed to run.
const SKIPPED: &str = "skipped: set RURGE_TEST_PRIVILEGED=1 in a disposable Linux VM, as root";

/// The API's key, as the profiles below give it.
const API_KEY: &str = "l2-key";

/// Where a run is told the two GeoIP databases are: a port of a's own loopback
/// that nothing listens on — a's namespace is this test's own, so the only
/// listeners there are the ones the test started — so that no run reaches the
/// internet for them (`CLAUDE.md`, the rules the tests follow). Nothing is kept
/// from the attempt: no rule of these profiles is a `GEOIP` rule.
const GEOIP: &str = "http://127.0.0.1:20001/unreachable";

/// Whether these tests may run: `RURGE_TEST_PRIVILEGED=1`, which only a
/// disposable Linux VM, as root, sets. Nothing is built before this is asked.
fn permitted() -> bool {
    if std::env::var("RURGE_TEST_PRIVILEGED").as_deref() == Ok("1") {
        return true;
    }
    println!("{SKIPPED}");
    false
}

/// A TCP round trip and a ping, from an application in a to a server in b:
/// both go into the adapter, out of it on rurge's own socket, and out DIRECT.
#[test]
fn a_tcp_round_trip_and_a_ping_go_through_the_adapter_and_out_direct() {
    if !permitted() {
        return;
    }
    let mut lab = Lab::new();
    let profile = lab.write(
        "direct.conf",
        &format!("[General]\nhttp-api = {API_KEY}@127.0.0.1:0\n[Rule]\nFINAL,DIRECT\n"),
    );
    let data = lab.path().join("data");
    let helper = helper();

    let mut echo = lab.in_ns(&lab.b, &helper);
    echo.args(["echo", "10.231.1.1:17000"]);
    let mut server = lab.spawn(echo);
    server
        .stdout
        .wait_for(Duration::from_secs(10), "the echo server's line", |line| {
            line.starts_with("listening on ")
        });

    let mut run = lab.start_rurge(&profile, &data, true);
    run.wait_for_the_enhanced_mode();
    let api = run.api();

    let round_trip = lab
        .in_ns(&lab.a, &helper)
        .args(["connect", "10.231.1.1:17000"])
        .output()
        .expect("the helper could be started");
    assert!(
        round_trip.status.success(),
        "the round trip did not come back:{}\nwhat rurge said:\n{}",
        said(&round_trip),
        run.stdout.seen.join("\n")
    );

    let ping = lab
        .in_ns(&lab.a, Path::new("ping"))
        .args(["-c", "1", "-W", "5", "10.231.1.1"])
        .output()
        .expect("ping could be started");
    assert!(
        ping.status.success(),
        "the ping did not come back:{}\nwhat rurge said:\n{}",
        said(&ping),
        run.stdout.seen.join("\n")
    );

    // the session the round trip made: through the adapter, and DIRECT
    let log = lab.request_log(api);
    let found = log.iter().any(|request| {
        request["dst"].as_str() == Some("10.231.1.1:17000")
            && request["policy"]
                .as_array()
                .is_some_and(|policy| policy.iter().any(|name| name.as_str() == Some("DIRECT")))
    });
    assert!(
        found,
        "no DIRECT session to the echo server is in the request log:\n{}",
        json(&log)
    );
}

/// rurge's own DNS queries leave by the physical interface, not through the
/// adapter: the query to the profile's `dns-server` reaches b from a's veth
/// address, and no session to it was made in the adapter.
#[test]
fn rurges_own_dns_queries_do_not_enter_the_adapter() {
    if !permitted() {
        return;
    }
    let mut lab = Lab::new();
    let profile = lab.write(
        "dns.conf",
        &format!(
            "[General]\nhttp-api = {API_KEY}@127.0.0.1:0\ndns-server = 10.231.1.1\n\
             always-real-ip = example.l2\n[Rule]\nFINAL,DIRECT\n"
        ),
    );
    let data = lab.path().join("data");
    let helper = helper();

    let mut server = lab.in_ns(&lab.b, &helper);
    server.args(["dns", "10.231.1.1:53", "example.l2", "10.231.1.1"]);
    let mut dns = lab.spawn(server);
    dns.stdout
        .wait_for(Duration::from_secs(10), "the DNS server's line", |line| {
            line.starts_with("listening on ")
        });

    let mut run = lab.start_rurge(&profile, &data, true);
    run.wait_for_the_enhanced_mode();
    let api = run.api();

    // the query goes to the responder in the adapter: 198.18.0.2 is rurge's
    // own space, so it is one of the routes the adapter brought
    let asked = lab
        .in_ns(&lab.a, &helper)
        .args(["query", "198.18.0.2", "example.l2"])
        .output()
        .expect("the helper could be started");
    assert!(
        asked.status.success(),
        "the query did not come back:{}\nwhat rurge said:\n{}",
        said(&asked),
        run.stdout.seen.join("\n")
    );
    let answer = String::from_utf8_lossy(&asked.stdout).trim().to_string();
    assert_eq!(
        answer, "10.231.1.1",
        "the answer is not the address the DNS server in b gives"
    );

    // the resolver's query to it came out of the veth, not out of the adapter
    let from = dns.stdout.wait_for(
        Duration::from_secs(10),
        "the DNS server's line for the query",
        |line| line.starts_with("query from "),
    );
    assert!(
        from.contains("10.231.0.2:"),
        "the resolver's query did not come from a's veth address: {from}"
    );

    // and it did not come back into the adapter: a session through the adapter
    // would have entered there, from the adapter's address
    let log = lab.request_log(api);
    let leaked = log.iter().any(|request| {
        request["dst"].as_str() == Some("10.231.1.1:53")
            && request["transport"].as_str() == Some("udp")
    });
    assert!(
        !leaked,
        "rurge's own query came in through the adapter:\n{}",
        json(&log)
    );
}

/// `kill -9` with the enhanced mode on: the next start puts back what the dead
/// run left in `state.json` and says so. On Linux that is `rp_filter`, which
/// `/proc/sys` keeps per namespace and which outlives the process.
#[test]
fn a_killed_run_is_undone_by_the_next_start() {
    if !permitted() {
        return;
    }
    let mut lab = Lab::new();
    let profile = lab.write(
        "recover.conf",
        &format!("[General]\nhttp-api = {API_KEY}@127.0.0.1:0\n[Rule]\nFINAL,DIRECT\n"),
    );
    let data = lab.path().join("data");

    // the strict setting rurge loosens. The link's own value is set too: a VM
    // whose `net.ipv4.conf.default.rp_filter` is already 2 would otherwise have
    // an effective value above rurge's 1, and loosening `all` would change
    // nothing — rurge leaves a setting that is not strict alone.
    let (ns, name) = (lab.a.clone(), "l2a");
    lab.set_rp_filter(&ns, "all", 1);
    lab.set_rp_filter(&ns, name, 1);
    assert_eq!(lab.rp_filter(&ns, "all"), "1");

    let mut run = lab.start_rurge(&profile, &data, true);
    run.wait_for_the_enhanced_mode();
    assert_eq!(
        lab.rp_filter(&ns, "all"),
        "2",
        "the enhanced mode did not loosen the strict rp_filter"
    );

    // gone, as a crash leaves it: no chance to put anything back
    lab.kill(run.id);
    lab.await_the_adapter_gone();

    // the next start, this time without the enhanced mode, undoes it
    let mut next = lab.start_rurge(&profile, &data, false);
    next.stderr.wait_for(
        Duration::from_secs(30),
        "the line that says what the dead run left was undone",
        |line| line.contains("a previous run left changes of the enhanced mode; they were undone"),
    );
    assert_eq!(
        lab.rp_filter(&ns, "all"),
        "1",
        "the next start did not put rp_filter back"
    );

    let state = std::fs::read_to_string(data.join("state.json"))
        .expect("the data directory has no state.json");
    let state: serde_json::Value = serde_json::from_str(&state).expect("state.json is not JSON");
    assert!(
        state["enhanced_backup"].is_null(),
        "state.json still has the backup of the dead run: {state}"
    );
}

/// The helper program, where cargo built it for this test package.
fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_l2-helper"))
}

/// The `rurge` these tests start: `RURGE_TEST_RURGE` when it is set, else the
/// one cargo built beside the test binary (`target/<profile>/rurge`).
fn rurge_binary() -> PathBuf {
    if let Some(given) = std::env::var_os("RURGE_TEST_RURGE") {
        let path = PathBuf::from(given);
        assert!(
            path.is_file(),
            "RURGE_TEST_RURGE is `{}`, which is not a file",
            path.display()
        );
        return path;
    }
    let exe = std::env::current_exe().expect("the test's own path");
    // `target/<profile>/deps/<test>`: two directories above it is the profile's
    let beside = exe
        .parent()
        .and_then(Path::parent)
        .map(|dir| dir.join("rurge"));
    match beside {
        Some(path) if path.is_file() => path,
        other => panic!(
            "no rurge to test: build one with `cargo build -p rurge` (looked for `{}`), \
             or give its path in RURGE_TEST_RURGE",
            other.unwrap_or_default().display()
        ),
    }
}

/// What a command wrote, for a panic message.
fn said(output: &Output) -> String {
    let mut text = format!(" it ended {:?}", output.status);
    if !output.stdout.is_empty() {
        text.push_str(&format!(
            "\nits standard output:\n{}",
            String::from_utf8_lossy(&output.stdout)
        ));
    }
    if !output.stderr.is_empty() {
        text.push_str(&format!(
            "\nits standard error:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    text
}

fn json(requests: &[serde_json::Value]) -> String {
    serde_json::to_string_pretty(requests).unwrap_or_default()
}

/// The two network namespaces, the veth between them, the temporary directory
/// a test's profile and data live in, and every program this lab started. All
/// of it is removed again in [`Drop`].
struct Lab {
    /// The `ip` the platform's own lookup finds, so that the tests drive the
    /// same program rurge does (M4c plan, task 9).
    ip: PathBuf,
    a: String,
    b: String,
    rurge: PathBuf,
    /// `/etc/netns/<a>`, whose `resolv.conf` `ip netns exec` mounts over
    /// `/etc/resolv.conf` in a: without it a's resolver would be the machine's
    /// (and, on a machine whose resolver is systemd-resolved's stub, rurge
    /// would try to take the *host's* DNS over through D-Bus and fail).
    etc: PathBuf,
    dir: tempfile::TempDir,
    children: Vec<Child>,
}

impl Lab {
    fn new() -> Lab {
        let ip = match LinuxTools::find(Path::new("/")) {
            Ok(tools) => tools.ip,
            Err(e) => panic!("the L2 tests need iproute2: {e}"),
        };
        let rurge = rurge_binary();
        let pid = std::process::id();
        let a = format!("rl2-a-{pid}");
        let b = format!("rl2-b-{pid}");
        let etc = PathBuf::from(format!("/etc/netns/{a}"));
        let dir = tempfile::tempdir().expect("a temporary directory");
        // built before its setup runs, so that a failing setup still removes
        // what it made
        let lab = Lab {
            ip,
            a,
            b,
            rurge,
            etc,
            dir,
            children: Vec::new(),
        };
        lab.set_up();
        lab
    }

    fn set_up(&self) {
        self.run(&["netns", "add", self.a.as_str()]);
        self.run(&["netns", "add", self.b.as_str()]);
        // the pair lives in the two namespaces only: nothing of it is the host's
        self.run(&[
            "link",
            "add",
            "l2a",
            "netns",
            self.a.as_str(),
            "type",
            "veth",
            "peer",
            "name",
            "l2b",
            "netns",
            self.b.as_str(),
        ]);
        for (ns, dev) in [
            (self.a.as_str(), "lo"),
            (self.a.as_str(), "l2a"),
            (self.b.as_str(), "lo"),
            (self.b.as_str(), "l2b"),
        ] {
            self.run(&["-n", ns, "link", "set", dev, "up"]);
        }
        self.run(&[
            "-n",
            self.a.as_str(),
            "addr",
            "add",
            "10.231.0.2/30",
            "dev",
            "l2a",
        ]);
        self.run(&[
            "-n",
            self.b.as_str(),
            "addr",
            "add",
            "10.231.0.1/30",
            "dev",
            "l2b",
        ]);
        // a's way out of the veth is its default route; b needs none
        self.run(&[
            "-n",
            self.a.as_str(),
            "route",
            "add",
            "default",
            "via",
            "10.231.0.1",
            "dev",
            "l2a",
        ]);
        // everything on "the internet": b's own loopback, which a can only
        // reach through the veth
        self.run(&[
            "-n",
            self.b.as_str(),
            "addr",
            "add",
            "10.231.1.1/32",
            "dev",
            "lo",
        ]);
        std::fs::create_dir_all(&self.etc).unwrap_or_else(|e| {
            panic!("cannot make {}: {e}", self.etc.display());
        });
        std::fs::write(self.etc.join("resolv.conf"), "nameserver 10.231.1.1\n")
            .unwrap_or_else(|e| panic!("cannot write a's resolv.conf: {e}"));
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// `text` in a file of the lab's temporary directory, whose path is
    /// returned: the profile of a test.
    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.path().join(name);
        std::fs::write(&path, text)
            .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
        path
    }

    /// A command that runs `program` inside `ns`: `ip netns exec` puts it there
    /// and then `exec`s it, so the process is the program's own.
    fn in_ns(&self, ns: &str, program: &Path) -> Command {
        let mut cmd = Command::new(&self.ip);
        cmd.args(["netns", "exec", ns]).arg(program);
        cmd
    }

    /// Runs `ip` with `args`, in whichever namespace the arguments name, and
    /// fails the test with both streams when it fails.
    fn run(&self, args: &[&str]) {
        let output = self.output(args);
        assert!(
            output.status.success(),
            "{} {} failed:{}",
            self.ip.display(),
            args.join(" "),
            said(&output)
        );
    }

    fn output(&self, args: &[&str]) -> Output {
        Command::new(&self.ip)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("cannot run {}: {e}", self.ip.display()))
    }

    /// `rurge run` in a, with `--enhanced-mode` when `enhanced`: the platform
    /// backend and the real network, with every test backend of the
    /// environment left behind.
    fn start_rurge(&mut self, profile: &Path, data: &Path, enhanced: bool) -> Started {
        let (ns, rurge) = (self.a.clone(), self.rurge.clone());
        let mut cmd = self.in_ns(&ns, &rurge);
        cmd.args(["run", "-c"])
            .arg(profile)
            .arg("--data-dir")
            .arg(data)
            .args(["--log-level", "info"])
            .args(["--geoip-url", GEOIP, "--geoip-asn-url", GEOIP]);
        if enhanced {
            cmd.arg("--enhanced-mode");
        }
        for name in [
            "RURGE_ENHANCED_BACKEND",
            "RURGE_NETWORK_BACKEND",
            "RURGE_PROCESS_BACKEND",
            "RURGE_ENHANCED_MODE",
            "RURGE_SYSTEM_PROXY",
            "RURGE_DATA_DIR",
            "RURGE_LOG_FILE",
        ] {
            cmd.env_remove(name);
        }
        self.spawn(cmd)
    }

    /// Starts a prepared command and reads both of its streams line by line.
    fn spawn(&mut self, mut cmd: Command) -> Started {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let program = cmd.get_program().to_owned();
        let mut child = cmd
            .spawn()
            .unwrap_or_else(|e| panic!("cannot start {}: {e}", program.to_string_lossy()));
        let stdout = Lines::forward(child.stdout.take().expect("standard output is piped"));
        let stderr = Lines::forward(child.stderr.take().expect("standard error is piped"));
        self.children.push(child);
        Started {
            id: self.children.len() - 1,
            stdout,
            stderr,
        }
    }

    /// `kill -9`, and its end waited for.
    fn kill(&mut self, id: usize) {
        let Some(child) = self.children.get_mut(id) else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
    }

    /// What `GET /v1/requests/recent` gives, as a list of requests.
    fn request_log(&self, api: SocketAddr) -> Vec<serde_json::Value> {
        let (ns, helper) = (self.a.clone(), helper());
        let url = format!("http://{api}/v1/requests/recent");
        let got = self
            .in_ns(&ns, &helper)
            .args(["get", &url, API_KEY])
            .output()
            .expect("the helper could be started");
        assert!(
            got.status.success(),
            "the request log could not be read:{}",
            said(&got)
        );
        let log: serde_json::Value = serde_json::from_slice(&got.stdout)
            .unwrap_or_else(|e| panic!("what the API gave is not JSON: {e}"));
        log["requests"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("no list of requests in {log}"))
    }

    /// Writes `value` to a `rp_filter` file of `ns`: `/proc/sys/net` is the
    /// namespace's own, so this is read and written through `ip netns exec`.
    fn set_rp_filter(&self, ns: &str, name: &str, value: u8) {
        let line = format!("echo {value} > /proc/sys/net/ipv4/conf/{name}/rp_filter");
        let out = self
            .in_ns(ns, Path::new("sh"))
            .args(["-c", &line])
            .output()
            .expect("sh could be started");
        assert!(
            out.status.success(),
            "cannot write the rp_filter of {name}:{}",
            said(&out)
        );
    }

    fn rp_filter(&self, ns: &str, name: &str) -> String {
        let file = format!("/proc/sys/net/ipv4/conf/{name}/rp_filter");
        let out = self
            .in_ns(ns, Path::new("cat"))
            .arg(&file)
            .output()
            .expect("cat could be started");
        assert!(
            out.status.success(),
            "cannot read the rp_filter of {name}:{}",
            said(&out)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Waits, up to five seconds, for the adapter of a killed run to be gone:
    /// the process has been reaped, but a descriptor closes a moment later.
    fn await_the_adapter_gone(&self) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let out = self.output(&["-n", self.a.as_str(), "link", "show", "rurge"]);
            if !out.status.success() {
                return;
            }
            assert!(
                Instant::now() < until,
                "the adapter is still there five seconds after the run was killed:{}",
                said(&out)
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Lab {
    /// Ends every program it started, then removes the two namespaces and the
    /// `/etc/netns` directory. A step that fails says so on standard error and
    /// does not panic: a panic here would hide the test's own failure.
    fn drop(&mut self) {
        for (index, child) in self.children.iter_mut().enumerate() {
            if let Err(e) = child.kill() {
                eprintln!("l2: cannot end the program it started at {index}: {e}");
            }
            let _ = child.wait();
        }
        for ns in [&self.a, &self.b] {
            let output = Command::new(&self.ip)
                .args(["netns", "del"])
                .arg(ns.as_str())
                .output();
            match output {
                Ok(out) if out.status.success() => {}
                Ok(out) => eprintln!("l2: cannot remove the namespace {ns}:{}", said(&out)),
                Err(e) => eprintln!("l2: cannot run ip to remove the namespace {ns}: {e}"),
            }
        }
        if let Err(e) = std::fs::remove_dir_all(&self.etc) {
            eprintln!("l2: cannot remove {}: {e}", self.etc.display());
        }
    }
}

/// A program a [`Lab`] started, and its two streams.
struct Started {
    id: usize,
    stdout: Lines,
    stderr: Lines,
}

impl Started {
    /// Waits 30 seconds for the one line `rurge run` prints when the enhanced
    /// mode is on: a line that is exactly that, not a log line that says it.
    fn wait_for_the_enhanced_mode(&mut self) {
        self.stdout.wait_for(
            Duration::from_secs(30),
            "the enhanced mode's line",
            |line| line == "enhanced mode on",
        );
    }

    /// The HTTP API's address, from the line rurge prints when it listens.
    fn api(&self) -> SocketAddr {
        let line = self
            .stdout
            .seen
            .iter()
            .find_map(|line| line.strip_prefix("api on http://"))
            .unwrap_or_else(|| {
                panic!(
                    "rurge run did not print the address of its API; it said:\n{}",
                    self.stdout.seen.join("\n")
                )
            });
        line.parse()
            .unwrap_or_else(|e| panic!("`{line}` is not an address: {e}"))
    }
}

/// The lines of one stream, on a channel: a thread of its own reads them, so
/// that a pipe nobody is reading from does not fill up and stop the program.
struct Lines {
    rx: Receiver<String>,
    seen: Vec<String>,
}

impl Lines {
    fn forward(reader: impl Read + Send + 'static) -> Lines {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Lines {
            rx,
            seen: Vec::new(),
        }
    }

    /// Waits at most `limit` for the first line `matches` says yes to; a
    /// timeout panics with every line read so far, so that a start that failed
    /// says why.
    fn wait_for(&mut self, limit: Duration, what: &str, matches: impl Fn(&str) -> bool) -> String {
        let until = Instant::now() + limit;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if matches(&line) {
                        return line;
                    }
                }
                Err(e) => panic!(
                    "{what} did not come within {limit:?} ({e:?}); what was read so far:\n{}",
                    self.seen.join("\n")
                ),
            }
        }
    }
}
