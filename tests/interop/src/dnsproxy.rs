//! AdGuard dnsproxy v0.86.0 (Apache-2.0) as a child process on the loopback:
//! a reference for the `quic://` (DoQ) and `h3://` (DoH3) DNS upstreams
//! (phase 2 M7 design 7.4), built for every platform CI runs on. The same
//! rules as for sing-box apply: nothing is downloaded or installed here, and
//! everything it listens on is on 127.0.0.1 — DoQ, DoH3, and the DNS over
//! HTTPS (TCP) and plain DNS (UDP and TCP) it opens alongside, which the
//! tests leave alone. It answers [`DNS_NAME`] from a hosts file of its own;
//! its one upstream is the loopback's discard port, where anything else it
//! is asked goes nowhere. It has no port to probe that says QUIC is up: it
//! is up once its log says so.

use crate::{DNS_NAME, DNS_V4, DNS_V6, REQUIRED_ENV, Reference, TlsFiles, free_ports};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_DNSPROXY";

/// What it logs (at `info`) once its DoQ listener is bound, and once its
/// DoH3 one is.
const LISTENING_QUIC: &str = "listening quic";
const LISTENING_H3: &str = "listening to h3";

/// `RURGE_TEST_DNSPROXY`, else the first `dnsproxy` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) {
        "dnsproxy.exe"
    } else {
        "dnsproxy"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn dnsproxy_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no dnsproxy was found ({BINARY_ENV} or PATH)");
    }
    eprintln!("skipping {test}: no dnsproxy ({BINARY_ENV} or PATH); see tests/interop/README.md");
    None
}

/// The ports dnsproxy listens on, all on 127.0.0.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ports {
    /// Plain DNS, on UDP and TCP: it takes 53 unless told otherwise.
    pub plain: u16,
    /// DNS over QUIC, on UDP.
    pub quic: u16,
    /// DNS over HTTPS on TCP, and DNS over HTTP/3 on the UDP port of the
    /// same number.
    pub https: u16,
}

/// The hosts file: [`DNS_NAME`]'s two addresses, its name in lower case as
/// the resolver asks for it.
pub fn hosts() -> String {
    format!("{DNS_V4} {DNS_NAME}\n{DNS_V6} {DNS_NAME}\n")
}

/// The command line: `ports` on 127.0.0.1, HTTP/3 on the HTTPS port, the
/// certificate and key of `tls` (PEM paths), the hosts file at `hosts` and
/// the discard port as the one upstream it must have.
pub fn args(ports: Ports, tls: &TlsFiles, hosts: &Path) -> Vec<OsString> {
    assert!(
        tls.client_ca.is_none(),
        "dnsproxy asks for no client certificate"
    );
    let mut args: Vec<OsString> = Vec::new();
    for (flag, value) in [
        ("--listen", "127.0.0.1".to_string()),
        ("--port", ports.plain.to_string()),
        ("--quic-port", ports.quic.to_string()),
        ("--https-port", ports.https.to_string()),
        ("--upstream", "127.0.0.1:9".to_string()),
    ] {
        args.extend([flag.into(), value.into()]);
    }
    args.push("--http3".into());
    for (flag, path) in [
        ("--tls-crt", tls.certificate.as_path()),
        ("--tls-key", tls.key.as_path()),
        ("--hosts-files", hosts),
    ] {
        args.extend([flag.into(), path.into()]);
    }
    args
}

/// A running dnsproxy; killed and reaped on drop.
pub struct Dnsproxy {
    server: Reference,
    ports: Ports,
}

impl Dnsproxy {
    /// Writes the hosts file into `dir`, starts `binary` there and waits
    /// until it says DoQ and DoH3 listen.
    pub fn spawn(binary: &Path, dir: &Path, tls: &TlsFiles) -> Dnsproxy {
        // the HTTPS port is TCP's and HTTP/3's (UDP) alike, and so is the
        // plain one; all three before any is let go, so that no two of them
        // come by one number (the QUIC port is free for TCP as well, which
        // nothing needs)
        let taken = free_ports(&[true, true, true]);
        let ports = Ports {
            plain: taken[0],
            quic: taken[1],
            https: taken[2],
        };
        let hosts_file = dir.join("hosts.txt");
        std::fs::write(&hosts_file, hosts()).expect("write the hosts file");
        let mut command = Command::new(binary);
        command.args(args(ports, tls, &hosts_file)).current_dir(dir);
        let mut server =
            Reference::start("dnsproxy", command, Vec::new(), dir.join("dnsproxy.log"));
        server.wait_for_output(LISTENING_QUIC);
        server.wait_for_output(LISTENING_H3);
        Dnsproxy { server, ports }
    }

    /// The loopback UDP port of DNS over QUIC.
    pub fn quic_port(&self) -> u16 {
        self.ports.quic
    }

    /// The loopback UDP port of DNS over HTTP/3.
    pub fn h3_port(&self) -> u16 {
        self.ports.https
    }

    pub fn log_text(&self) -> String {
        self.server.log_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> TlsFiles {
        TlsFiles {
            certificate: "/tmp/dns/leaf.pem".into(),
            key: "/tmp/dns/leaf.key".into(),
            client_ca: None,
        }
    }

    fn ports() -> Ports {
        Ports {
            plain: 4001,
            quic: 4002,
            https: 4003,
        }
    }

    /// Every listener on 127.0.0.1, nothing upstream but the discard port,
    /// no debug endpoint.
    #[test]
    fn the_command_line_stays_on_the_loopback() {
        let args = args(ports(), &files(), Path::new("/tmp/dns/hosts.txt"));
        let text: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let value = |flag: &str| -> Vec<&str> {
            text.windows(2)
                .filter(|pair| pair[0] == flag)
                .map(|pair| pair[1].as_str())
                .collect()
        };
        assert_eq!(value("--listen"), ["127.0.0.1"]);
        assert_eq!(value("--upstream"), ["127.0.0.1:9"]);
        for forbidden in [
            "0.0.0.0",
            "--pprof",
            "--config-path",
            "--bootstrap",
            "--fallback",
        ] {
            assert!(
                !text.iter().any(|a| a.contains(forbidden)),
                "`{forbidden}` in {text:?}"
            );
        }
    }

    #[test]
    fn the_command_line_is_written_as_dnsproxy_reads_it() {
        let expected: Vec<OsString> = [
            "--listen",
            "127.0.0.1",
            "--port",
            "4001",
            "--quic-port",
            "4002",
            "--https-port",
            "4003",
            "--upstream",
            "127.0.0.1:9",
            "--http3",
            "--tls-crt",
            "/tmp/dns/leaf.pem",
            "--tls-key",
            "/tmp/dns/leaf.key",
            "--hosts-files",
            "/tmp/dns/hosts.txt",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(
            args(ports(), &files(), Path::new("/tmp/dns/hosts.txt")),
            expected
        );
        assert_eq!(
            hosts(),
            "192.0.2.10 interop.rurge.test\n2001:db8::10 interop.rurge.test\n"
        );
    }
}
