//! mihomo v1.19.32 (MetaCubeX/mihomo, GPL-3.0) as a child process on the
//! loopback: the reference for `snell` versions 1 to 3 and for the `ss`
//! stream cipher `chacha20`, and the one reference that speaks simple-obfs
//! (phase 2 M8 design 4.1, 4.3). It only ever runs as a process of its own:
//! nothing of its code is here. The same rules as for sing-box apply: nothing
//! is downloaded or installed, and every listener of the rendered
//! configuration is on 127.0.0.1 — a `shadowsocks` listener's UDP port of
//! the same number too. With `mode: direct` and neither rule, DNS section
//! nor provider it fetches nothing, and what it keeps (`cache.db`) it keeps
//! in the directory it is started with.
//!
//! A listener that accepts is not yet a mihomo that relays: it opens its
//! listeners first and lets its tunnel run a little later, and whatever a
//! listener hands to the tunnel in between is closed. So the configuration
//! has one listener more, a `socks` one, and mihomo counts as up once a
//! connection asked for through it has arrived.

use crate::{READY_TIMEOUT, REQUIRED_ENV, Reference, free_port, free_tcp_and_udp_port};
use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const BINARY_ENV: &str = "RURGE_TEST_MIHOMO";

/// The camouflage host in a `snell` listener's `obfs-opts`. mihomo never
/// looks at it, but its decoder refuses an `obfs-opts` without one.
pub const OBFS_HOST: &str = "obfs.rurge.test";

/// Every variable mihomo's `main.go` takes the default of a flag from. One
/// would replace the configuration; others open a control port, fetch a
/// dashboard or run a script.
const FLAG_DEFAULTS: [&str; 13] = [
    "CLASH_HOME_DIR",
    "CLASH_CONFIG_FILE",
    "CLASH_CONFIG_STRING",
    "CLASH_AGE_SECRET_KEY",
    "CLASH_OVERRIDE_EXTERNAL_UI_DIR",
    "CLASH_OVERRIDE_EXTERNAL_CONTROLLER",
    "CLASH_OVERRIDE_EXTERNAL_CONTROLLER_TLS",
    "CLASH_OVERRIDE_EXTERNAL_CONTROLLER_UNIX",
    "CLASH_OVERRIDE_EXTERNAL_CONTROLLER_PIPE",
    "CLASH_OVERRIDE_EXTERNAL_CONTROLLER_ROUTING_MARK",
    "CLASH_OVERRIDE_SECRET",
    "CLASH_POST_UP",
    "CLASH_POST_DOWN",
];

/// How long one request through the `socks` listener is given to arrive.
const ATTEMPT: Duration = Duration::from_millis(200);

/// `RURGE_TEST_MIHOMO`, else the first `mihomo` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) {
        "mihomo.exe"
    } else {
        "mihomo"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn mihomo_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no mihomo was found ({BINARY_ENV} or PATH)");
    }
    eprintln!("skipping {test}: no mihomo ({BINARY_ENV} or PATH); see tests/interop/README.md");
    None
}

/// simple-obfs in front of a listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Obfs {
    Http,
    Tls,
}

impl Obfs {
    fn mode(self) -> &'static str {
        match self {
            Obfs::Http => "http",
            Obfs::Tls => "tls",
        }
    }
}

/// One entry of `listeners`.
#[derive(Clone)]
pub enum Listener {
    /// `type: snell`, which speaks the one `version` it is given (1 to 5)
    /// and, told `udp: true`, serves the UDP command too.
    Snell {
        version: u8,
        psk: String,
        obfs: Option<Obfs>,
    },
    /// `type: shadowsocks`, on a TCP port and the UDP port of the same
    /// number. A `cipher` that is neither AEAD nor 2022 is served by the
    /// older of mihomo's two implementations, which knows the stream ciphers.
    Shadowsocks {
        cipher: &'static str,
        password: String,
        obfs: Option<Obfs>,
    },
}

/// A YAML double-quoted scalar: JSON's strings are written the same way.
fn quoted(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// The whole configuration: `listeners`, each on its loopback port, and the
/// `socks` listener on `ready` that tells when the tunnel relays. With
/// `mode: direct` what comes in goes out directly, whatever the rules —
/// there are none. A `snell` listener's `obfs-opts` always names a host:
/// mihomo's decoder takes the missing key for an error.
pub fn render(listeners: &[(Listener, u16)], ready: u16) -> String {
    let mut text = String::from("mode: direct\nipv6: false\nlog-level: info\nlisteners:\n");
    for (index, (listener, port)) in listeners.iter().enumerate() {
        match listener {
            Listener::Snell { version, psk, obfs } => {
                assert!(
                    (1..=5).contains(version),
                    "mihomo's snell listener speaks versions 1 to 5"
                );
                text += &format!(
                    "  - name: snell-{index}\n    type: snell\n    listen: 127.0.0.1\n    port: {port}\n    psk: {}\n    version: {version}\n    udp: true\n",
                    quoted(psk)
                );
                if let Some(obfs) = obfs {
                    text += &format!(
                        "    obfs-opts:\n      mode: {}\n      host: {}\n",
                        obfs.mode(),
                        quoted(OBFS_HOST)
                    );
                }
            }
            Listener::Shadowsocks {
                cipher,
                password,
                obfs,
            } => {
                text += &format!(
                    "  - name: shadowsocks-{index}\n    type: shadowsocks\n    listen: 127.0.0.1\n    port: {port}\n    password: {}\n    cipher: {}\n    udp: true\n",
                    quoted(password),
                    quoted(cipher)
                );
                if let Some(obfs) = obfs {
                    text += &format!(
                        "    simple-obfs:\n      enable: true\n      mode: {}\n",
                        obfs.mode()
                    );
                }
            }
        }
    }
    text += &format!(
        "  - name: ready\n    type: socks\n    listen: 127.0.0.1\n    port: {ready}\n    udp: false\n"
    );
    text
}

/// What mihomo is started with: its home directory, where it keeps
/// `cache.db`, and the configuration — and nothing of the environment that
/// would add to either.
fn command(binary: &Path, dir: &Path, config: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .arg("-d")
        .arg(dir)
        .arg("-f")
        .arg(config)
        .current_dir(dir);
    for name in FLAG_DEFAULTS {
        command.env_remove(name);
    }
    // mihomo refuses to relay for a client whose address is the local one of
    // a connection it has made itself: its guard against being asked to
    // connect to itself. Here every client is on 127.0.0.1, as mihomo's own
    // connections are, so the guard has nothing to catch and a port number
    // in common to trip over.
    command.env("DISABLE_LOOPBACK_DETECTOR", "true");
    command
}

/// Whether a connection asked for through the `socks` listener on `ready`
/// arrives within `bound`. Asked again and again: before its tunnel runs
/// mihomo closes the client instead.
fn relays(ready: u16, bound: Duration) -> bool {
    let target = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
    target
        .set_nonblocking(true)
        .expect("a listener that does not block");
    let port = target
        .local_addr()
        .expect("the listener's address")
        .port()
        .to_be_bytes();
    // no authentication, then CONNECT to 127.0.0.1 on that port: a SOCKS5
    // server reads the two messages one after the other
    let request = [5, 1, 0, 5, 1, 0, 1, 127, 0, 0, 1, port[0], port[1]];
    let socks = SocketAddr::from(([127, 0, 0, 1], ready));
    let deadline = Instant::now() + bound;
    while Instant::now() < deadline {
        // held while the connection it asks for is awaited
        let _asking = TcpStream::connect_timeout(&socks, ATTEMPT)
            .and_then(|mut client| client.write_all(&request).map(|()| client));
        let attempt = Instant::now() + ATTEMPT;
        while Instant::now() < attempt {
            if target.accept().is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    false
}

/// The client of each TCP connection `log` says mihomo relayed to `target`,
/// in the log's order: the address the request's connection came from. In
/// `direct` mode mihomo logs `[TCP] <client> --> <target> using DIRECT` for
/// every request it has connected for.
fn clients_of(log: &str, target: SocketAddr) -> Vec<String> {
    let to = format!("{target} using DIRECT");
    log.lines()
        .filter_map(|line| {
            let (_, relayed) = line.split_once("[TCP] ")?;
            let (client, rest) = relayed.split_once(" --> ")?;
            rest.starts_with(&to).then(|| client.to_string())
        })
        .collect()
}

/// A running mihomo; killed and reaped on drop.
pub struct Mihomo(Reference);

impl Mihomo {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until every listener accepts connections and the tunnel relays them.
    pub fn spawn(binary: &Path, dir: &Path, listeners: Vec<Listener>) -> Mihomo {
        let with_ports: Vec<(Listener, u16)> = listeners
            .into_iter()
            .map(|listener| {
                let port = match listener {
                    Listener::Snell { .. } => free_port(),
                    Listener::Shadowsocks { .. } => free_tcp_and_udp_port(),
                };
                (listener, port)
            })
            .collect();
        let ready = free_port();
        let config = dir.join("mihomo.yaml");
        std::fs::write(&config, render(&with_ports, ready)).expect("write the config");
        let mut ports: Vec<u16> = with_ports.iter().map(|(_, port)| *port).collect();
        ports.push(ready);
        let server = Reference::start(
            "mihomo",
            command(binary, dir, &config),
            ports,
            dir.join("mihomo.log"),
        );
        assert!(
            relays(ready, READY_TIMEOUT),
            "mihomo listens but relayed nothing:\n{}",
            server.log_text()
        );
        Mihomo(server)
    }

    /// The loopback port of the `index`-th listener.
    pub fn port(&self, index: usize) -> u16 {
        self.0.port(index)
    }

    pub fn log_text(&self) -> String {
        self.0.log_text()
    }

    /// The client of each TCP connection mihomo has relayed to `target`, in
    /// order: requests that came on one connection name one client.
    pub fn clients_of(&self, target: SocketAddr) -> Vec<String> {
        clients_of(&self.log_text(), target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::io::Read;

    fn snell(version: u8, obfs: Option<Obfs>) -> Listener {
        Listener::Snell {
            version,
            psk: "sn3ll".into(),
            obfs,
        }
    }

    fn shadowsocks(cipher: &'static str, obfs: Option<Obfs>) -> Listener {
        Listener::Shadowsocks {
            cipher,
            password: "s3same".into(),
            obfs,
        }
    }

    /// Both kinds of listener, each bare, behind `http` and behind `tls`.
    fn every_kind() -> Vec<(Listener, u16)> {
        vec![
            (snell(1, None), 5001),
            (snell(4, Some(Obfs::Http)), 5002),
            (snell(2, Some(Obfs::Tls)), 5003),
            (shadowsocks("chacha20", None), 5004),
            (shadowsocks("aes-128-gcm", Some(Obfs::Http)), 5005),
            (shadowsocks("aes-256-cfb", Some(Obfs::Tls)), 5006),
        ]
    }

    /// The keys at an indentation of `depth` spaces, in order.
    fn keys(text: &str, depth: usize) -> Vec<&str> {
        text.lines()
            .filter(|line| line.len() - line.trim_start().len() == depth)
            .filter_map(|line| line.trim_start().split_once(':'))
            .map(|(key, _)| key)
            .collect()
    }

    /// Seven listeners, each on 127.0.0.1; no key that reaches further.
    #[test]
    fn the_configuration_stays_on_the_loopback() {
        let text = render(&every_kind(), 5999);
        assert_eq!(keys(&text, 0), ["mode", "ipv6", "log-level", "listeners"]);
        assert!(
            text.starts_with("mode: direct\nipv6: false\nlog-level: info\nlisteners:\n"),
            "{text}"
        );
        let listens: Vec<&str> = text.lines().filter(|l| l.contains("listen:")).collect();
        assert_eq!(listens, ["    listen: 127.0.0.1"; 7]);
        assert_eq!(text.matches("  - name: ").count(), 7);
        for forbidden in [
            "0.0.0.0",
            "::",
            "allow-lan",
            "bind-address",
            "external-",
            "secret",
            "tun",
            "dns",
            "rule",
            "provider",
            "proxies",
            "proxy-groups",
            "geo",
            "iptables",
            "redir",
            "tproxy",
            "ntp",
            "sniffer",
            "interface-name",
        ] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
    }

    #[test]
    fn listeners_are_rendered_as_mihomo_reads_them() {
        let text = render(&every_kind(), 5999);
        let expected = [
            "mode: direct",
            "ipv6: false",
            "log-level: info",
            "listeners:",
            // a name of its own each: mihomo keeps its listeners by name
            "  - name: snell-0",
            "    type: snell",
            "    listen: 127.0.0.1",
            "    port: 5001",
            "    psk: \"sn3ll\"",
            "    version: 1",
            // the default of a listener written here, said all the same
            "    udp: true",
            "  - name: snell-1",
            "    type: snell",
            "    listen: 127.0.0.1",
            "    port: 5002",
            "    psk: \"sn3ll\"",
            "    version: 4",
            "    udp: true",
            "    obfs-opts:",
            "      mode: http",
            "      host: \"obfs.rurge.test\"",
            "  - name: snell-2",
            "    type: snell",
            "    listen: 127.0.0.1",
            "    port: 5003",
            "    psk: \"sn3ll\"",
            "    version: 2",
            "    udp: true",
            "    obfs-opts:",
            "      mode: tls",
            "      host: \"obfs.rurge.test\"",
            "  - name: shadowsocks-3",
            "    type: shadowsocks",
            "    listen: 127.0.0.1",
            "    port: 5004",
            "    password: \"s3same\"",
            "    cipher: \"chacha20\"",
            "    udp: true",
            "  - name: shadowsocks-4",
            "    type: shadowsocks",
            "    listen: 127.0.0.1",
            "    port: 5005",
            "    password: \"s3same\"",
            "    cipher: \"aes-128-gcm\"",
            "    udp: true",
            "    simple-obfs:",
            "      enable: true",
            "      mode: http",
            "  - name: shadowsocks-5",
            "    type: shadowsocks",
            "    listen: 127.0.0.1",
            "    port: 5006",
            "    password: \"s3same\"",
            "    cipher: \"aes-256-cfb\"",
            "    udp: true",
            "    simple-obfs:",
            "      enable: true",
            "      mode: tls",
            // the probe: TCP alone
            "  - name: ready",
            "    type: socks",
            "    listen: 127.0.0.1",
            "    port: 5999",
            "    udp: false",
        ];
        assert_eq!(text.lines().collect::<Vec<_>>(), expected);
        assert!(text.ends_with("    udp: false\n"), "{text}");
    }

    /// Every listener has the keys mihomo's decoder insists on — `name`,
    /// and `psk` or `password` with `cipher` —, a listen address, which
    /// would default to every address, and one port; and an `obfs-opts`
    /// names its mode and a host, both or the listener is refused.
    #[test]
    fn no_listener_lacks_a_key_mihomo_requires() {
        let text = render(&every_kind(), 5999);
        let entries: Vec<&str> = text
            .split("  - name: ")
            .skip(1)
            .map(|entry| entry.trim_end())
            .collect();
        assert_eq!(entries.len(), 7);
        for entry in &entries {
            let fields = keys(entry, 4);
            let kind = entry
                .lines()
                .find_map(|line| line.trim_start().strip_prefix("type: "))
                .expect("a type");
            let required: &[&str] = match kind {
                "snell" => &["type", "listen", "port", "psk", "version", "udp"],
                "shadowsocks" => &["type", "listen", "port", "password", "cipher", "udp"],
                "socks" => &["type", "listen", "port", "udp"],
                other => panic!("an unexpected listener type `{other}`"),
            };
            assert_eq!(&fields[..required.len()], required, "{entry}");
            let nested = keys(entry, 6);
            match fields.get(required.len()) {
                None => assert!(nested.is_empty(), "{entry}"),
                Some(&"obfs-opts") => assert_eq!(nested, ["mode", "host"], "{entry}"),
                Some(&"simple-obfs") => assert_eq!(nested, ["enable", "mode"], "{entry}"),
                Some(other) => panic!("an unexpected key `{other}` in {entry}"),
            }
        }
        let names: Vec<&str> = entries
            .iter()
            .filter_map(|entry| entry.lines().next())
            .collect();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "{names:?}");
    }

    /// What a test makes up goes in quotes, whatever it holds.
    #[test]
    fn made_up_strings_are_quoted() {
        assert_eq!(quoted("sn3ll"), "\"sn3ll\"");
        assert_eq!(quoted("a: b #c"), "\"a: b #c\"");
        assert_eq!(quoted(r#"p"w\x"#), r#""p\"w\\x""#);
        assert_eq!(quoted("2022"), "\"2022\"");
        let listener = Listener::Snell {
            version: 3,
            psk: "yes: #1 \"psk\"".into(),
            obfs: None,
        };
        let text = render(&[(listener, 5001)], 5999);
        assert!(
            text.contains("    psk: \"yes: #1 \\\"psk\\\"\"\n"),
            "{text}"
        );
    }

    #[test]
    #[should_panic(expected = "versions 1 to 5")]
    fn a_version_mihomo_does_not_speak_is_not_rendered() {
        render(&[(snell(6, None), 5001)], 5999);
    }

    /// The home directory and the configuration by flags, none of the
    /// variables that would stand in for a flag, and the loopback guard off.
    #[test]
    fn the_command_line_is_written_as_mihomo_reads_it() {
        let command = command(
            Path::new("mihomo"),
            Path::new("home"),
            Path::new("home/mihomo.yaml"),
        );
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(args, ["-d", "home", "-f", "home/mihomo.yaml"]);
        assert_eq!(command.get_current_dir(), Some(Path::new("home")));
        let (mut removed, mut set) = (Vec::new(), Vec::new());
        for (name, value) in command.get_envs() {
            let name = name.to_str().expect("an ASCII name");
            match value {
                None => removed.push(name),
                Some(value) => set.push((name, value.to_str().expect("an ASCII value"))),
            }
        }
        removed.sort_unstable();
        let mut expected = FLAG_DEFAULTS.to_vec();
        expected.sort_unstable();
        assert_eq!(removed, expected);
        assert!(FLAG_DEFAULTS.iter().all(|name| name.starts_with("CLASH_")));
        assert_eq!(set, [("DISABLE_LOOPBACK_DETECTOR", "true")]);
    }

    /// What a SOCKS5 server does for one client, as mihomo's does: it
    /// answers the greeting, answers the request and only then connects.
    fn serve(client: &mut TcpStream) -> std::io::Result<()> {
        let (mut greeting, mut request) = ([0u8; 3], [0u8; 10]);
        client.read_exact(&mut greeting)?;
        assert_eq!(greeting, [5, 1, 0], "one method: no authentication");
        client.write_all(&[5, 0])?;
        client.read_exact(&mut request)?;
        assert_eq!(request[..8], [5, 1, 0, 1, 127, 0, 0, 1], "CONNECT, IPv4");
        client.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])?;
        let to = u16::from_be_bytes([request[8], request[9]]);
        TcpStream::connect(("127.0.0.1", to)).map(drop)
    }

    /// A SOCKS5 server of one thread on a loopback port: it closes its first
    /// `closing` clients unanswered, as mihomo does before its tunnel runs,
    /// and is gone once it has connected for a later one. A client that left
    /// before it was served is no reason to stop.
    fn socks_server(closing: usize) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (met, client) in listener.incoming().enumerate() {
                let mut client = client.expect("a client");
                if met >= closing && serve(&mut client).is_ok() {
                    return;
                }
            }
        });
        port
    }

    /// The probe asks as a SOCKS5 client does, and again after a server
    /// that closed it; a server that never connects is not up.
    #[test]
    fn the_probe_tells_a_tunnel_that_relays_from_a_listener_that_accepts() {
        let bound = Duration::from_secs(10);
        assert!(relays(socks_server(0), bound), "relayed at once");
        assert!(relays(socks_server(3), bound), "relayed after three tries");
        // it accepts, as its backlog does, and connects nowhere
        let deaf = TcpListener::bind("127.0.0.1:0").unwrap();
        let started = Instant::now();
        assert!(!relays(
            deaf.local_addr().unwrap().port(),
            Duration::from_millis(500)
        ));
        assert!(started.elapsed() < bound, "the wait is bounded");
    }

    /// The log's lines of relayed TCP requests, in logrus's plain form (the
    /// one a file gets) and its coloured one; other targets, UDP and the
    /// listeners' own lines are not requests to `target`.
    #[test]
    fn clients_are_read_from_the_log() {
        let target: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let log = concat!(
            "time=\"2026-10-08T12:00:00.000000001Z\" level=info msg=\"Snell[snell-0] inbound listening at: 127.0.0.1:5001\"\n",
            "time=\"2026-10-08T12:00:00.100000000Z\" level=info msg=\"[TCP] 127.0.0.1:50001 --> 127.0.0.1:8080 using DIRECT\"\n",
            "time=\"2026-10-08T12:00:00.200000000Z\" level=info msg=\"[TCP] 127.0.0.1:50001 --> 127.0.0.1:8080 using DIRECT\"\n",
            "time=\"2026-10-08T12:00:00.300000000Z\" level=info msg=\"[TCP] 127.0.0.1:50002 --> 127.0.0.1:80 using DIRECT\"\n",
            "time=\"2026-10-08T12:00:00.400000000Z\" level=info msg=\"[UDP] 127.0.0.1:50003 --> 127.0.0.1:8080 using DIRECT\"\n",
            "time=\"2026-10-08T12:00:00.500000000Z\" level=warning msg=\"[TCP] dial DIRECT 127.0.0.1:50004 --> 127.0.0.1:8080 error: connection refused\"\n",
            "\u{1b}[36mINFO\u{1b}[0m[2026-10-08T12:00:00.600000000Z] [TCP] 127.0.0.1:50005 --> 127.0.0.1:8080 using DIRECT\n",
        );
        assert_eq!(
            clients_of(log, target),
            ["127.0.0.1:50001", "127.0.0.1:50001", "127.0.0.1:50005"]
        );
        assert!(clients_of(log, "127.0.0.1:8".parse().unwrap()).is_empty());
    }
}
